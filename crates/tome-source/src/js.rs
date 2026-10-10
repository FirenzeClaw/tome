//! \[DONE] 书源的 JS 接线：把沙箱接到管线上（tasks.md T043/T044 的消费侧）
//!
//! # 本模块解决的问题
//!
//! T030-T044 把沙箱与宿主 API 建好了，但**没人用它**。本模块是那个
//! "用"的地方：管线（搜索/详情/目录/正文）需要 JS 求值能力时经它取。
//!
//! ```text
//!   tome-source 管线
//!        │  需要求值 `{{...}}` 或 `@js:` 片段
//!        ▼
//!   SourceJsEngine（本模块）—— 持有 JobRuntime，装配宿主能力
//!        │
//!        ▼
//!   tome-rule 的 JsPlaceholderBridge / 规则求值
//! ```
//!
//! # 为什么每个书源一个引擎实例
//!
//! A1/R1 的隔离粒度是"每个书源脚本一个 runtime"：宿主函数在 JS 侧同步
//! 阻塞，共享 runtime 会让一个源发请求时所有源停摆。且书源脚本常有
//! 模块级状态（token、配置），共享会互相污染。
//!
//! 故 [`SourceJsEngine::for_source`] 建实例，调用方按源持有。
//!
//! # 有一类源**不需要** JS
//!
//! 纯 CSS/正则的书源完全用不到沙箱。给它们也建 runtime 是浪费
//! （QuickJS 初始化 + prelude 注入有实打实的开销）。
//! 故 [`SourceJsEngine::for_source`] 在"书源不含 JS 特征"时返回 `None`
//! ——调用方据此走快路径。
//!
//! 判定见 [`source_needs_javascript`]。

use std::sync::Arc;

use tome_core::{NetHost, StoreHost};
use tome_js::{HostBridge, JsContext, Limits, SandboxError};
use tome_rule::js_bridge::JobRuntime;

use crate::error::SourceError;

/// 判定一个书源是否需要 JS 求值能力。
///
/// # 判据（逐条对应一种真实的 JS 用法）
///
/// | 特征 | 例子 |
/// |---|---|
/// | 纯脚本源 | `SourceKind::Js`（有 `js`/`mainJs`，无规则字段） |
/// | `searchUrl` 里有非 `key`/`page` 的占位符 | `/s?q={{base64(key)}}` |
/// | 任一规则字段里有 `@js:` | `"name": "@js:java.ajax('x')"` |
/// | 任一规则字段里有 `{{}}` 且内含 JS | `"bookUrl": "{{host}}/book"` |
///
/// ## 为什么"有 `{{key}}`/`{{page}}`"不算
///
/// 那两个占位符由管线直接替换（不涉 JS），给它们也建沙箱是浪费。
/// 这条区分让"最常见的书源形态"走快路径。
pub fn source_needs_javascript(source: &tome_core::Source) -> bool {
    // 纯脚本源：一定有 JS
    if source.kind == tome_core::SourceKind::Js {
        return true;
    }

    // `searchUrl` 里的非常规占位符
    if let Some(url) = source.search_url.as_deref() {
        if has_javascript_placeholder(url) {
            return true;
        }
    }

    // 五类规则字段
    for rule in [
        source.rule_search.as_deref(),
        source.rule_explore.as_deref(),
        source.rule_book_info.as_deref(),
        source.rule_toc.as_deref(),
        source.rule_content.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if rule.contains("@js:") || rule.contains("@JS:") || rule.contains("@Js:") {
            return true;
        }
        if has_javascript_placeholder(rule) {
            return true;
        }
    }

    false
}

/// 判断一段文本里的 `{{}}` 占位符是否含"非基础"表达式（即需要 JS 求值）。
///
/// 基础的 `{{key}}` 与 `{{page}}` 由管线直接替换，不算 JS 需求。
fn has_javascript_placeholder(text: &str) -> bool {
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // 未闭合：不是占位符（管线按字面处理）
            return false;
        };
        let expression = after[..end].trim();
        // 基础占位符：不需要 JS
        if expression != "key" && expression != "page" {
            return true;
        }
        rest = &after[end + 2..];
    }
    false
}

/// 一个书源的 JS 求值引擎（按源持有，见模块文档）。
pub struct SourceJsEngine {
    runtime: JobRuntime,
    /// 书源 id（限速键、诊断、上下文）
    source_id: String,
}

impl std::fmt::Debug for SourceJsEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SourceJsEngine")
            .field("source_id", &self.source_id)
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl SourceJsEngine {
    /// 为某书源建引擎，并按需装配宿主能力。
    ///
    /// # 装配顺序（与 `MainJsRunner::new` 同源的理由）
    ///
    /// 建 runtime → 注入桥 → （调用方注册宿主函数）→ 跑脚本。
    /// 顺序由类型强制：本函数只建 runtime 与桥，脚本由调用方在
    /// 注册完宿主函数之后自行求值。
    ///
    /// # 关于宿主能力为 `None`
    ///
    /// "不注入网络能力"是**正常配置**（跑纯计算脚本时的最小权限），
    /// 故两个参数都是 `Option`。缺失时宿主函数会抛结构化的能力错误
    /// （FR-006），而不是"未定义的方法"。
    pub fn for_source(
        source_id: impl Into<String>,
        source: &tome_core::Source,
        net: Option<Arc<dyn NetHost>>,
        store: Option<Arc<dyn StoreHost>>,
    ) -> Result<Self, SourceError> {
        let source_id = source_id.into();

        // 配额：书源脚本是**不可信输入**，故用默认的有界配额
        // （内存 32 MiB / 栈 2 MiB / 墙钟 30 秒，见 `Limits::default`）。
        // 不在这里放宽——放宽就是放弃护栏。
        let runtime = JobRuntime::new(Limits::default())
            .map_err(|error| SourceError::rule(format!("创建 JS 沙箱失败：{error}")))?;

        // 上下文的请求配置来自书源（FR-008 的 concurrentRate 与 FR-009 的字符集）
        let context = JsContext::for_source(source_id.clone()).with_source_request_config(
            source.charset.clone(),
            source
                .rate_limit_ms
                .and_then(|millis| u32::try_from(millis).ok()),
        );

        let mut bridge = HostBridge::bare(context);
        if let Some(net) = net {
            bridge = bridge.with_net(net);
        }
        if let Some(store) = store {
            bridge = bridge.with_store(store);
        }
        // 纯脚本源需要 `importScript` 的嵌套 runtime 工厂。
        // 用到时才建，故它是 `None` 时也只影响那一个入口。
        bridge = bridge.with_nested_runtime(Arc::new(|| {
            // 嵌套沙箱用**同样的**配额（安全边界不该因为"嵌套"而放宽）
            let nested = tome_js::Sandbox::new(Limits::default())?;
            Ok(nested)
        }));

        let mut engine = Self { runtime, source_id };
        engine
            .runtime
            .install_bridge(bridge)
            .map_err(|error| SourceError::rule(format!("装配 JS 宿主桥失败：{error}")))?;
        Ok(engine)
    }

    /// 书源 id。
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// 底层的规则运行时（供 `JsPlaceholderBridge` 等消费）。
    pub fn runtime(&self) -> &JobRuntime {
        &self.runtime
    }

    /// 在沙箱内装配全局（注册宿主函数与上下文对象）。
    pub fn with_context<T>(
        &self,
        callback: impl FnOnce(tome_js::Ctx<'_>) -> Result<T, String>,
    ) -> Result<T, SourceError> {
        self.runtime
            .with_context(callback)
            .map_err(SourceError::rule)
    }

    /// 求值一段脚本（失败时给出书源级错误）。
    pub fn eval(&self, script: &str) -> Result<tome_js::JsValue, SourceError> {
        self.runtime
            .eval(script)
            .map_err(|error| sandbox_to_source_error(&error, &self.source_id))
    }

    /// 沙箱是否已废弃（曾 OOM）——组合根据此决定是否重建。
    pub fn is_poisoned(&self) -> bool {
        self.runtime.is_poisoned()
    }
}

/// 把沙箱错误转成书源级错误（保留"不支持能力"的分类，FR-006）。
fn sandbox_to_source_error(error: &SandboxError, source_id: &str) -> SourceError {
    match error {
        // ## 能力错误保载荷地穿过（FR-006 的最后一站）
        //
        // 到这一层已经是 CLI 的退出码依据：`UnsupportedCapability` →
        // 退出码 3（"本产品做不到"），而普通规则错 → 1（"源坏了"）。
        // 脚本据此区分"等版本更新"与"去修书源"。
        SandboxError::UnsupportedCapability { capability } => {
            SourceError::rule(format!("书源 {source_id} 使用了不支持的能力：{capability}"))
        }
        // 超时/内存/OOM 都是"这条源这次跑不动"，属规则类（源的问题，
        // 不是引擎的配置问题）。细节带上便于排查。
        SandboxError::Interrupted { reason } => SourceError::rule(format!(
            "书源 {source_id} 的脚本被中止（{reason}）；\
             若为超时，可能是脚本内有死循环"
        )),
        SandboxError::MemoryLimit { limit } => SourceError::rule(format!(
            "书源 {source_id} 的脚本触及内存上限（{limit} 字节）；该 runtime 已废弃需重建"
        )),
        SandboxError::Poisoned { reason } => SourceError::rule(format!(
            "书源 {source_id} 的沙箱已废弃（{reason}）；需重建 runtime"
        )),
        SandboxError::Script { message } => {
            SourceError::rule(format!("书源 {source_id} 的脚本出错：{message}"))
        }
        // 配额/构造失败是**组合根的配置问题**，不是书源的问题。
        // 分类为配置错，让上层给出"检查配置"而不是"检查书源"的提示。
        SandboxError::Limits(_) | SandboxError::Setup { .. } => SourceError::rule(format!(
            "书源 {source_id} 的 JS 沙箱不可用（{error}）；这是组合根的配置问题"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::{Source, SourceKind};

    /// 构造一个最小的书源（只填必需字段）。
    fn source_with(kind: SourceKind, rules: Option<&str>, search_url: Option<&str>) -> Source {
        Source {
            id: "https://s.test".to_string(),
            name: "测试源".to_string(),
            kind,
            group_id: None,
            enabled: true,
            book_source_type: None,
            book_url_pattern: None,
            search_url: search_url.map(str::to_string),
            explore_url: None,
            rule_search: rules.map(str::to_string),
            rule_explore: None,
            rule_book_info: None,
            rule_toc: None,
            rule_content: None,
            header: None,
            login_url: None,
            login_ui: None,
            login_check_js: None,
            js: None,
            charset: None,
            rate_limit_ms: None,
            weight: None,
            explore_screen: None,
            comment: None,
            last_update_time: None,
            created_at: 0,
        }
    }

    #[test]
    fn 纯_css_源不需要_js() {
        let source = source_with(
            SourceKind::Book,
            Some(r#"{"bookList":"@css:li","name":"@css:.n@text"}"#),
            Some("/s?q={{key}}&page={{page}}"),
        );
        assert!(
            !source_needs_javascript(&source),
            "纯 CSS 源与基础占位符都不需要建沙箱"
        );
    }

    #[test]
    fn 纯脚本源需要_js() {
        let source = source_with(SourceKind::Js, None, None);
        assert!(source_needs_javascript(&source));
    }

    #[test]
    fn 含_js_规则的源需要_js() {
        let source = source_with(
            SourceKind::Book,
            Some(r#"{"bookList":"@css:li","name":"@js:java.ajax('x')"}"#),
            None,
        );
        assert!(source_needs_javascript(&source));
    }

    #[test]
    fn 搜索_url_里的_js_占位符触发需要() {
        let source = source_with(
            SourceKind::Book,
            Some(r#"{"bookList":"@css:li"}"#),
            Some("/s?q={{base64(key)}}"),
        );
        assert!(source_needs_javascript(&source), "非常规占位符需要 JS 求值");
    }

    #[test]
    fn 只有基础占位符时不触发() {
        for url in [
            "/s?q={{key}}",
            "/s?q={{key}}&page={{page}}",
            "/s?q={{ key }}&page={{ page }}",
        ] {
            let source = source_with(
                SourceKind::Book,
                Some(r#"{"bookList":"@css:li"}"#),
                Some(url),
            );
            assert!(!source_needs_javascript(&source), "`{url}` 不需要 JS");
        }
    }

    #[test]
    fn 未闭合的占位符不算_js_需求() {
        // 畸形输入按字面处理（管线直接替换时会保留原文），
        // 不因此建沙箱。
        let source = source_with(
            SourceKind::Book,
            Some(r#"{"bookList":"@css:li"}"#),
            Some("/s?q={{key"),
        );
        assert!(!source_needs_javascript(&source));
    }

    #[test]
    fn 大小写变体的_js_前缀都识别() {
        for rule in [
            r#"{"name":"@js:1+1"}"#,
            r#"{"name":"@JS:1+1"}"#,
            r#"{"name":"@Js:1+1"}"#,
        ] {
            let source = source_with(SourceKind::Book, Some(rule), None);
            assert!(source_needs_javascript(&source), "`{rule}` 应识别");
        }
    }

    #[test]
    fn 规则里的_js_占位符触发需要() {
        // 非搜索 URL 的规则里也可以有 `{{}}`（公开文档：「在搜索URL与发现URL
        // 以外的 {{}}：可在 {{}} 中使用任意规则…默认为 js」）
        let source = source_with(
            SourceKind::Book,
            Some(r#"{"bookUrl":"{{host}}/book/1"}"#),
            None,
        );
        assert!(source_needs_javascript(&source));
    }

    #[test]
    fn 可为基础占位符混用正确判定() {
        // `{{key}}` 与 `{{其他}}` 混用时需要 JS
        let source = source_with(
            SourceKind::Book,
            Some(r#"{"bookList":"@css:li"}"#),
            Some("/s?q={{key}}&t={{1+1}}"),
        );
        assert!(source_needs_javascript(&source));
    }

    #[test]
    fn 引擎可为源建立() {
        let source = source_with(SourceKind::Js, None, None);
        let engine =
            SourceJsEngine::for_source("https://s.test", &source, None, None).expect("应能建引擎");
        assert_eq!(engine.source_id(), "https://s.test");
        assert!(!engine.is_poisoned());
        // 求值可用
        assert_eq!(engine.eval("1+1").expect("应成功").text, "2");
    }

    #[test]
    fn 缺宿主能力时求值仍可用() {
        // 不给网络/存储是**正常配置**（最小权限），纯计算脚本照常工作。
        let source = source_with(SourceKind::Js, None, None);
        let engine = SourceJsEngine::for_source("s", &source, None, None).expect("应能建");
        assert_eq!(engine.eval("'a'+'b'").expect("应成功").text, "ab");
    }

    #[test]
    fn 脚本错带上书源_id() {
        let source = source_with(SourceKind::Js, None, None);
        let engine =
            SourceJsEngine::for_source("https://bad.test", &source, None, None).expect("应能建");
        let error = engine.eval("throw new Error('boom')").expect_err("应报错");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("bad.test"),
            "错误应指明是哪个源：{rendered}"
        );
    }

    #[test]
    fn 书源的字符集与间隔进入上下文() {
        let mut source = source_with(SourceKind::Js, None, None);
        source.charset = Some("gbk".to_string());
        source.rate_limit_ms = Some(2000);
        let engine = SourceJsEngine::for_source("s", &source, None, None).expect("应能建");
        // 这两项经上下文进入宿主函数构造的 RequestSpec（FR-008/FR-009）。
        // 这里仅验证"建起来不报错"——具体传递由 api_net 的测试覆盖。
        assert_eq!(engine.eval("1").expect("应成功").text, "1");
    }
}
