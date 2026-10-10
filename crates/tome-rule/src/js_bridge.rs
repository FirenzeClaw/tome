//! [DONE] `@js:` 内联脚本 + `{{}}` 插值 + `@webjs` 集成（tasks.md T043）
//!
//! # 本模块在整条链路上的位置
//!
//! T025 已经定义了 `{{}}` 的**解析**（切出占位表达式）与桥接 trait
//! [`crate::replace::PlaceholderBridge`]，但那个 trait 当时没有实现——
//! 因为 JS 沙箱还不存在。本模块补上实现：把 `@js:` 与 `{{}}` 里的表达式
//! 交给 [`JobRuntime`]，把结果拼回字符串。
//!
//! ```text
//! 规则文本 "https://x/s?q={{key}}&t={{now()}}"
//!   → parse_interpolation      （T025，切出两个占位）
//!   → JsPlaceholderBridge      （本模块）
//!   → JobRuntime               （T030 的沙箱）
//!   → "https://x/s?q=斗破&t=1700000000"
//! ```
//!
//! # 三处容易做错的语义（逐条对应公开文档的规定）
//!
//! ## 1. `@js:` 的返回是**值**，不是脚本
//!
//! 公开文档说 `@js:` 后面是"可以在其中使用"的 JavaScript。书源里的写法
//! 有两类，**两类都要支持**：
//!
//! | 写法 | 语义 | 处理 |
//! |---|---|---|
//! | `@js:1+1` | **表达式**，取其值 | 包成 `eval` 的直接求值 |
//! | `@js:var x=1; x+1` | **语句块**，取最后一个表达式的值 | 需要包一层 |
//!
//! 这是最关键的一处：若只按"整段当表达式求值"处理，第二类写法会抛
//! 语法错（`var` 不是表达式）；若只按"包成函数再调用"处理，第一类里
//! 的 `return` 语义又不对。本模块的做法见 [`wrap_inline_script`]。
//!
//! ## 2. `{{}}` 里默认是 js（与普通规则不同）
//!
//! 公开文档明确说 `{{}}` 里"默认为 js"，故无前缀的表达式按 JS 处理——
//! 而**普通规则**里无前缀是 CSS/Default。这个差异是 T025 的 `build_placeholder`
//! 已经处理过的（那里把 `RuleKind::Default` 覆写成 `JavaScript`），
//! 本模块只是消费那个结论。
//!
//! ## 3. `@webjs` 是**明确不支持**的（能力错误）
//!
//! `@webjs` 表示"这段脚本在 WebView 环境里执行"——它依赖内嵌浏览器，
//! 而本产品的架构决定是不含浏览器内核（见 `tome_js::api::unsupported`）。
//! 故它必须抛结构化能力错误（FR-006），让书源的 try/catch 降级，
//! 而不是让脚本在一个"没有 webView 对象"的环境里静默跑错。
//!
//! # 关于依赖方向（tasks.md T043 提出的问题）
//!
//! tasks.md 原文问："tome-rule 可以依赖 tome-js 吗？"
//!
//! 答案是**可以，而且这正是既定设计**：`crates/tome-rule/Cargo.toml` 里
//! `tome-js` 已在依赖表中，`plan.md` 的依赖图也写明 `tome-rule` 依赖
//! `core(RequestSpec) / js(eval)`。故本模块**直接**使用 `tome_js` 的类型，
//! 不需要额外的桥接 trait。
//!
//! 唯一需要反向桥接的是"网页标记解析"（T039 的 `MarkupHost`）——
//! 那里 `tome-js` 需要调用 `tome-rule`，方向相反，故必须经 trait 注入。
//! 两处的方案不同正是因为**依赖方向不同**。

use tome_js::{Ctx, HostBridge, JsValue, Limits, Sandbox, SandboxError};

use crate::error::RuleError;
use crate::replace::PlaceholderBridge;

/// 执行内联脚本片段的运行时。
///
/// # 为什么是"可复用的沙箱"而不是"每次新建"
///
/// `{{}}` 插值可能一条规则里出现十几次（搜索 URL、页码计算、时间戳…）。
/// 每次新建 runtime 的开销（QuickJS 初始化 + prelude 注入）远超脚本本身，
/// 而同一份规则的插值之间**没有状态冲突**（它们各自独立求值）。
///
/// 故用**一个** runtime 跑同一条规则的多个片段。这与"每个书源脚本一个
/// runtime"（A1/R1）不矛盾：那是**书源之间**的隔离粒度，本条是**同一书源
/// 内部**的复用。
///
/// # 为什么不实现 `Clone`
///
/// 沙箱持有 runtime（不可安全克隆），而"共享一个"才是我们要的语义。
/// 调用方用 `Arc<JobRuntime>` 共享。
pub struct JobRuntime {
    sandbox: Sandbox,
    /// 宿主桥（诊断用：是否配了网络/存储能力）
    ///
    /// 存一份供 [`JobRuntime::describe`] 报告配置，避免"忘了注入"这类问题
    /// 只在运行时以 `hostBridge` 能力错误的形式暴露。
    bridge_summary: String,
}

impl std::fmt::Debug for JobRuntime {
    /// 手写 `Debug`：`Sandbox` 内部含 runtime（不可 `Debug`），
    /// 且诊断需要的只是"配置摘要"，不是整个沙箱。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobRuntime")
            .field("bridge", &self.bridge_summary)
            .finish_non_exhaustive()
    }
}

impl JobRuntime {
    /// 以默认配额新建，并注入兼容前导脚本。
    ///
    /// 前导脚本是**必须**的：书源的 `@js:` 片段会直接引用 `java` 之类的
    /// 宿主全局，缺了它整段脚本会以 `ReferenceError` 失败——而那个错误
    /// 信息（"java is not defined"）会让书源作者以为是自己的写法问题。
    pub fn new(limits: Limits) -> Result<Self, SandboxError> {
        let sandbox = Sandbox::new(limits)?;
        sandbox.install_prelude()?;
        Ok(Self {
            sandbox,
            bridge_summary: "未注入宿主桥".to_string(),
        })
    }

    /// 注入宿主能力（组合根装配）。
    pub fn install_bridge(&mut self, bridge: HostBridge) -> Result<(), SandboxError> {
        self.bridge_summary = format!("{bridge:?}");
        self.sandbox.install_host_bridge(bridge)
    }

    /// 装配**离线的**宿主 API 并装进沙箱（T085 的收口）。
    ///
    /// # 它解决什么问题
    ///
    /// `java.*` 宿主函数必须被注册，`@js:java.base64Encode(...)` 之类的规则
    /// 才可求值。而组装这些注册的是**上层**（组合根 `tome-cli`）——那让
    /// `tome-rule` 自己的语料校验跑不了带宿主调用的用例（它不该依赖上层）。
    ///
    /// 本函数把"按 FR-005 八类注册 `java.*`"这件事下沉到 `tome-rule`：
    /// 它依赖 `tome-js`（本就依赖）且能提供 `RuleMarkupHost`（在 `tome-rule`
    /// 里）。于是**任何**建 `JobRuntime` 的地方都能拿到完整宿主面，
    /// 不必各自重来一遍——也不会出现"CLI 支持、语料台不支持"的漂移。
    ///
    /// # 为什么叫"离线"
    ///
    /// 它用 `HostBridge::bare`：不带网络与存储。语料校验是**纯函数**性质的
    /// 语法点验证，不该发请求或读盘。真实抓取路径仍在 `tome-cli` 装配
    /// （那里注入真实的 `NetHost`/`StoreHost`）。
    pub fn install_host_api(&mut self) -> Result<(), SandboxError> {
        // 已装过就不重复装（重复注册会让 `debug_assert` 的计数虚高）
        if self.bridge_summary != "未注入宿主桥" {
            return Ok(());
        }

        let context = tome_js::JsContext::for_source("<runtime>".to_string());
        self.install_bridge(HostBridge::bare(context))?;

        self.with_context(|ctx| {
            let java = tome_js::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 对象失败：{error}"))?;

            let (net, _store) = {
                let bridged = ctx.userdata::<HostBridge>().ok_or_else(|| {
                    "宿主桥未注入（install_host_api 应在 install_bridge 之后）".to_string()
                })?;
                (bridged.net.clone(), bridged.store.clone())
            };

            let mut registered = 0usize;

            registered += tome_js::api::net::register(&ctx, &java, net)
                .map_err(|error| format!("注册网络类 API 失败：{error}"))?;
            registered += tome_js::api::text::register(&ctx, &java)
                .map_err(|error| format!("注册文本类 API 失败：{error}"))?;
            registered += tome_js::api::crypto::register(&ctx, &java)
                .map_err(|error| format!("注册加密类 API 失败：{error}"))?;
            // 网页标记解析落到本 crate 的 DOM 上（见 `markup_host`）
            let markup: std::sync::Arc<dyn tome_js::api::data::MarkupHost> =
                std::sync::Arc::new(crate::markup_host::RuleMarkupHost::new());
            registered += tome_js::api::data::register(&ctx, &java, Some(markup))
                .map_err(|error| format!("注册数据类 API 失败：{error}"))?;
            registered += tome_js::api::util::register(&ctx, &java)
                .map_err(|error| format!("注册工具类 API 失败：{error}"))?;
            registered += tome_js::api::state::register(&ctx, &java)
                .map_err(|error| format!("注册状态类 API 失败：{error}"))?;
            registered += tome_js::api::register_all_unsupported(&ctx, &java)
                .map_err(|error| format!("注册能力桩失败：{error:?}"))?;

            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局 java 失败：{error}"))?;

            // 至少应注册几十个函数；太少说明某个模块静默失败了
            if registered < 40 {
                return Err(format!(
                    "宿主 API 注册数量异常（{registered}），检查各模块的 register 是否都被调用"
                ));
            }
            Ok(())
        })
        .map_err(|reason| SandboxError::Setup { reason })
    }

    /// 在沙箱内装配全局对象（注册宿主函数、上下文对象）。
    ///
    /// 与 `tome_js::Sandbox::with_context` 同源，此处只是转发以便调用方
    /// 不必再接触 `Sandbox` 本体。
    pub fn with_context<T>(
        &self,
        callback: impl FnOnce(Ctx<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.sandbox.with_context(callback)
    }

    /// 求值一段脚本片段。
    pub fn eval(&self, script: &str) -> Result<JsValue, SandboxError> {
        self.sandbox.eval(script)
    }

    /// 沙箱是否已废弃（曾触发 OOM，不得复用）。
    pub fn is_poisoned(&self) -> bool {
        self.sandbox.is_poisoned()
    }

    /// 配置摘要（诊断用）。
    pub fn describe(&self) -> &str {
        &self.bridge_summary
    }
}

/// `{{}}` 插值的桥实现（T025 定义的 [`PlaceholderBridge`] 的落地）。
///
/// # 为什么它同时处理 js 类与规则类占位
///
/// [`PlaceholderBridge`] 有两个方法（`evaluate_js` 与 `evaluate_rule`），
/// 而 `{{}}` 里两类占位都可能出现（`{{@css:.a@text}}` 与 `{{key}}`）。
/// 本实现把 js 类交给运行时、规则类交给 [`crate::css_post`]——这正是
/// trait 设计的意图（一个桥同时服务两类，调用方不必分支）。
pub struct JsPlaceholderBridge<'a> {
    runtime: &'a JobRuntime,
    /// 规则类占位求值所需的文档（`{{@css:...}}` 要在页面上求值）
    document: &'a crate::dom::Document,
}

impl<'a> JsPlaceholderBridge<'a> {
    /// 为某次规则求值构造桥。
    pub fn new(runtime: &'a JobRuntime, document: &'a crate::dom::Document) -> Self {
        Self { runtime, document }
    }
}

impl PlaceholderBridge for JsPlaceholderBridge<'_> {
    fn evaluate_js(&self, expression: &str) -> Result<String, RuleError> {
        let script = wrap_inline_script(expression);
        match self.runtime.eval(&script) {
            Ok(value) => Ok(value.text),
            Err(error) => Err(error.into_rule_error()),
        }
    }

    fn evaluate_rule(&self, expression: &str) -> Result<String, RuleError> {
        // 规则类占位（CSS/Default）在文档上求值，与本模块的 JS 无关。
        // 复用 `evaluate_rule` 的完整语义（后缀、切片、替换链）。
        crate::css_post::evaluate_rule(self.document, expression).map(|values| values.join(""))
    }
}

/// 把 `@js:` 的片段包装成可求值的脚本。
///
/// # 两类写法都要支持（见模块文档）
///
/// | 输入 | 包装后 | 为什么 |
/// |---|---|---|
/// | `"1+1"` | 直接求值 | 表达式，取其值 |
/// | `"var x=1; x+1"` | `(function(){ <片段> })()` | 语句块，取**最后一条语句的值** |
///
/// ## 判定方式为什么是"尝试"而不是"看有没有分号"
///
/// 有分号也可能整体是个表达式（`"f(a);"`），没有分号也可能是语句
/// （`"var x=1"`）。词法判定要写一个能识别语句边界的扫描器，而那是
/// JS 引擎的活。
///
/// 故采用**先试后补**：先按表达式求值，失败则包成函数体再试一次。
/// 代价是失败路径多一次求值——而那条路径本来就是要失败的（书源写了
/// 语句块而我们按表达式看），故这个代价可接受。
///
/// ## `[UNCERTAIN]` 一个边界
///
/// 若片段既不是合法表达式、包成函数体后仍失败（真正的语法错），
/// 会得到**第二次**的错误消息。两处消息都来自同一引擎、同一片段，
/// 内容一致，故不影响归因。
pub fn wrap_inline_script(fragment: &str) -> String {
    let trimmed = fragment.trim();
    // ## 为什么要用"表达式优先 + 函数体兜底"的双形态
    //
    // 一次性包成函数体会让**表达式形态**的返回值丢失：`@js:1+1` 包成
    // `(function(){ 1+1 })()` 得到 `undefined`（没有 `return`）。
    // 而书源里 `@js:` 最常见的写法恰恰就是表达式（`@js:key`、`@js:page-1`）。
    //
    // 故用一段能**同时**处理两种形态的脚本：
    // 先尝试把它当表达式求值（用 `eval`），失败再当函数体。
    // 判定交给引擎（它才知道语句边界在哪），而不是我们做词法猜测。
    format!(
        r#"(function() {{
  try {{
    return (function(){{ return eval({fragment_literal}); }})();
  }} catch (__tome_expr_error) {{
    // ## 能力错误必须原样上抛，不得被这个兜底捕获吃掉
    //
    // 能力错误是 FR-006 的可编程信号，书源的 try/catch 靠它选备选路径。
    // 若在这里被"当成表达式求值失败"吞掉并重试语句形态，那个标识就丢了
    // ——表现是能力错误被压成一个与真实原因无关的语法错，
    // 而 CLI 的退出码会从 3（本产品做不到）变成 1（源坏了）。
    //
    // 判据用 `isUnsupportedCapability`（宿主函数抛的能力错误会打这个标，
    // 见 `tome_js::bridge`）而不是 `capability`：后者是书源也可能
    // 恰好用到的普通属性名。
    if (__tome_expr_error && __tome_expr_error.isUnsupportedCapability) {{
      throw __tome_expr_error;
    }}
    return (function(){{ {fragment} }})();
  }}
}})();"#,
        fragment = trimmed,
        fragment_literal = js_string_literal(trimmed)
    )
}

/// 把一个 Rust 字符串编成 JS 字符串字面量。
///
/// # 为什么必须给出 `"` 与 `\\`
///
/// 片段里大量出现这两种字符（`"字"`、`\\d`）。漏掉任一都会让字面量
/// 提前结束或转义错乱——而表现是"脚本语法错"，书源作者看不出
/// "是我们转义写错了"。U+2028/2029 也必须转：它们在 JS 里是行终止符。
fn js_string_literal(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            other if (other as u32) < 0x20 => {
                output.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => output.push(other),
        }
    }
    output.push('"');
    output
}

/// `@webjs` 的能力错误（明确不支持）。
///
/// 见模块文档的第 3 条：`@webjs` 依赖内嵌浏览器，而本产品不含浏览器内核。
/// 抛结构化能力错误让书源的 try/catch 降级——而不是让脚本在"没有 webView
/// 对象"的环境里静默跑错。
pub fn webjs_unsupported_error(action: &str) -> RuleError {
    RuleError::unsupported("webView", action)
}

/// 判断一条 `@js:` 规则是否其实用了 `@webjs`。
///
/// # 依据
///
/// 公开的规则说明里 `@webjs` 是**独立的前缀**（与 `@js:` 并列），
/// 表示"这段脚本在 WebView 环境里执行"。书源里也有写成
/// `@js:` 但脚本体里调用 `java.webView(...)` 的——那是**运行时**才
/// 暴露的能力缺失（由 `tome_js::api::unsupported` 的桩处理）。
///
/// 本函数只判**前缀形态**，那是本层（规则分派）能看到的全部信息。
pub fn is_webjs_prefix(rule: &str) -> bool {
    let trimmed = rule.trim();
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("@webjs:") || lower.starts_with("@webjs ")
}

/// 剥掉 `@webjs:` 前缀，返回脚本正文。
pub fn strip_webjs_prefix(rule: &str) -> &str {
    let trimmed = rule.trim();
    // 前缀长度固定（`@webjs:` 是 7 个 ASCII 字符），而 `@webjs ` 是 7 个
    // （含空格）。按字节切是安全的：前缀本身全是 ASCII。
    if trimmed.len() >= 7 {
        let (head, tail) = trimmed.split_at(7);
        if head.eq_ignore_ascii_case("@webjs:") || head.eq_ignore_ascii_case("@webjs ") {
            return tail.trim();
        }
    }
    trimmed
}

/// 把沙箱错误映射为规则错误（跨模块的错误分类转换）。
///
/// # 为什么这个转换在 `tome-js` 侧有对称的实现
///
/// `tome-js` 的 `SandboxError` 经 `to_error_kind` 映射到 core 的分类，
/// 而本模块需要的是 `tome-rule` 的 `RuleError`（更细的分类：语法/正则/
/// 上界/不支持能力）。故在此做一次转换，把"不支持能力"这一类**保留
/// 能力标识**——那是 FR-006 的关键（书源的降级逻辑靠它判断）。
pub trait SandboxErrorExt {
    /// 转成 `RuleError`（保留能力标识）。
    fn into_rule_error(self) -> RuleError;
}

impl SandboxErrorExt for SandboxError {
    fn into_rule_error(self) -> RuleError {
        match &self {
            // ## 能力错误必须**保载荷**地转过去（FR-006）
            //
            // `RuleError::Unsupported` 携带能力标识，而书源的降级逻辑
            // 正是靠它选备选路径。若把它压成 `Syntax`，标识就只剩消息
            // 文本可用——书源只能字符串匹配，措辞一改即失效。
            SandboxError::UnsupportedCapability { capability } => {
                RuleError::unsupported(capability.clone(), "js")
            }
            // 内存上限与超时属"引擎保护"而非"规则写错"，但它们在
            // `RuleError` 里没有更贴切的分类（`Limit` 是"超出处理上界"，
            // 语义最接近）。细节放 detail 里。
            SandboxError::MemoryLimit { .. }
            | SandboxError::Interrupted { .. }
            | SandboxError::Script { .. }
            | SandboxError::Poisoned { .. } => RuleError::syntax(self.to_string()),
            SandboxError::Limits(_) | SandboxError::Setup { .. } => {
                // 组合根的配置问题：报为 `Limit`（"超出处理上界"最接近，
                // 因为它是"沙箱没法用"而不是"规则写错了"）。
                RuleError::limit(self.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webjs_前缀被识别() {
        for rule in [
            "@webjs:1+1",
            "@WEBJS:1+1",
            "@WebJs: x",
            "@webjs x",
            "@webjs:",
        ] {
            assert!(is_webjs_prefix(rule), "`{rule}` 应识别为 @webjs");
        }
        for rule in ["@js:1+1", "1+1", "@css:.a", "@xpath://a", ""] {
            assert!(!is_webjs_prefix(rule), "`{rule}` 不应识别为 @webjs");
        }
    }

    #[test]
    fn webjs_前缀可剥离() {
        assert_eq!(strip_webjs_prefix("@webjs:1+1"), "1+1");
        assert_eq!(strip_webjs_prefix("@WEBJS: 1+1 "), "1+1");
        assert_eq!(strip_webjs_prefix("@webjs 1+1"), "1+1");
        // 非 webjs 前缀原样返回
        assert_eq!(strip_webjs_prefix("@js:1+1"), "@js:1+1");
    }

    #[test]
    fn webjs_报能力错误() {
        let error = webjs_unsupported_error("rule");
        match error.to_error_kind() {
            tome_core::ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "webView");
            }
            other => panic!("应报能力错误，实际 {other:?}"),
        }
    }

    #[test]
    fn 字符串字面量转义() {
        // 含引号与反斜杠的片段（书源里极常见）
        let literal = js_string_literal(r#"say "hi" \d+"#);
        assert_eq!(literal, r#""say \"hi\" \\d+""#);
        // 换行必须转义（否则脚本会断行）
        assert!(!js_string_literal("a\nb").contains('\n'));
        // U+2028/2029 在 JS 里是行终止符，必须转义
        assert!(js_string_literal("a\u{2028}b").contains("\\u2028"));
    }

    #[test]
    fn 包装后的脚本含两类形态() {
        let wrapped = wrap_inline_script("1+1");
        assert!(wrapped.contains("eval"), "应含表达式求值路径");
        assert!(wrapped.contains("1+1"), "应含原片段");
    }

    #[test]
    fn 沙箱错误映射保留可读信息() {
        let error = SandboxError::Script {
            message: "boom".to_string(),
        };
        let rule_error = error.into_rule_error();
        assert!(
            matches!(rule_error, RuleError::Syntax { .. }),
            "脚本错应映射为语法错"
        );
        let rendered = format!("{rule_error}");
        assert!(rendered.contains("boom"), "应保留原始信息：{rendered}");
    }

    #[test]
    fn 配额错误映射为上界错误() {
        let error = SandboxError::Limits(tome_js::LimitsError::ZeroMemoryLimit);
        assert!(matches!(error.into_rule_error(), RuleError::Limit { .. }));
    }
}
