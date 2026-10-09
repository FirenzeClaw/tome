//! [DONE] mainJs 纯脚本源执行（tasks.md T044）
//!
//! # 什么是"纯脚本源"
//!
//! 书源有两类（data-model.md 的 `source.kind`）：
//!
//! | 类型 | 形态 | 谁来执行 |
//! |---|---|---|
//! | 普通网页源 | 靠 `ruleSearch`/`ruleToc`/… 的规则文本 | tome-rule 的规则引擎 |
//! | **纯脚本源**（JS 源） | 一个 `js`（`mainJs`）脚本，**规则全在脚本里** | 本模块 |
//!
//! 纯脚本源没有规则字段，它的全部抓取逻辑写在脚本里，经四个**入口函数**
//! 与引擎交互。这是书源生态里一类真实存在的源（接口型站点常用它，
//! 因为签名/加密逻辑用规则表达不了）。
//!
//! # 四个入口约定（本模块的核心契约）
//!
//! | 入口 | 参数 | 返回 |
//! |---|---|---|
//! | `search(key, page)` | 关键词、页码 | 书籍数组 |
//! | `bookInfo(url)` | 详情页 URL | 书籍对象 |
//! | `chapterList(url)` | 详情页 URL | 章节数组 |
//! | `chapterContent(url)` | 章节 URL | 正文文本 |
//!
//! ## 依据来源
//!
//! 入口名与参数取自**公开的书源格式说明**（社区流通的《书源规则说明》
//! 与 JS 源编写文档），不是我们的自造命名——既有书源的脚本按这些名字
//! 定义函数，改名会让它们全部失效（兼容性优先，宪法原则 I）。
//!
//! # 三处必须做对的判定
//!
//! ## 1. 函数必须**存在**且是函数
//!
//! 脚本可能只实现了部分入口（例如只有 `search` 的源）。缺失时应报
//! **结构化错误**并指明缺哪个入口，而不是让引擎抛一个 `undefined is
//! not a function`——那个信息对书源作者没有指向性（他不知道是哪个动作
//! 的入口缺失）。
//!
//! ## 2. 返回值必须是**数组**（列表类入口）
//!
//! 书源里常见 `return JSON.parse(json).list` 这类写法。若返回值不是
//! 数组（写错成对象、或返回了 `undefined`），我们必须报错而不是"尽力
//! 转换"——静默产出一个空列表会让用户看到"这个源没有结果"，而真实
//! 原因是脚本写错了。那是排查方向被完全带偏的失效形态。
//!
//! ## 3. `chapterContent` 返回**文本**，`search`/`chapterList` 返回**列表**
//!
//! 两者形状不同，故入口函数的调用与结果的读取路径也不同。
//!
//! # 与 T025/T043 的分工
//!
//! T043 处理的是"**规则文本里**的 JS"（`@js:` / `{{}}`）；
//! 本模块处理的是"**整个源就是一个脚本**"。两者都经同一个 `JobRuntime`，
//! 但调用形态与结果校验不同。

use serde_json::Value as JsonValue;
use tome_js::{Ctx, HostBridge, SandboxError, Value};

use crate::error::RuleError;

/// 纯脚本源的四个入口名（公开书源格式的约定，见模块文档）。
pub mod entry_names {
    /// 搜索：`search(key, page)` → 书籍数组
    pub const SEARCH: &str = "search";
    /// 详情：`bookInfo(url)` → 书籍对象
    pub const BOOK_INFO: &str = "bookInfo";
    /// 目录：`chapterList(url)` → 章节数组
    pub const CHAPTER_LIST: &str = "chapterList";
    /// 正文：`chapterContent(url)` → 正文文本
    pub const CHAPTER_CONTENT: &str = "chapterContent";
}

/// 纯脚本源的一个动作。
///
/// # 为什么用枚举而不是散落的字符串
///
/// 它同时决定三件事：调用哪个入口、传什么参数、如何校验返回值。
/// 三者集中在枚举的关联函数里，才不会出现"某个动作忘了校验返回值"
/// 这类偏差（而那种偏差的后果是静默的错误数据）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptAction {
    /// 搜索（关键词 + 页码 → 书籍数组）
    Search,
    /// 书籍详情（URL → 书籍对象）
    BookInfo,
    /// 目录（URL → 章节数组）
    ChapterList,
    /// 正文（URL → 文本）
    ChapterContent,
}

impl ScriptAction {
    /// 对应的入口函数名。
    pub fn entry_name(self) -> &'static str {
        match self {
            Self::Search => entry_names::SEARCH,
            Self::BookInfo => entry_names::BOOK_INFO,
            Self::ChapterList => entry_names::CHAPTER_LIST,
            Self::ChapterContent => entry_names::CHAPTER_CONTENT,
        }
    }

    /// 动作名（诊断与错误消息用）。
    pub fn action_name(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::BookInfo => "bookInfo",
            Self::ChapterList => "chapterList",
            Self::ChapterContent => "chapterContent",
        }
    }

    /// 返回值是否应为**列表**。
    ///
    /// `BookInfo` 返回单个对象（不是列表），`ChapterContent` 返回文本。
    /// 见模块文档第 3 条。
    pub fn expects_list(self) -> bool {
        matches!(self, Self::Search | Self::ChapterList)
    }
}

/// 纯脚本源的执行器（每个源一个实例）。
///
/// # 为什么一个源一个实例
///
/// 与"每个书源脚本一个 runtime"（A1/R1）同理：宿主函数在 JS 侧同步阻塞，
/// 共享 runtime 会让一个源发请求时所有源停摆。且纯脚本源的脚本里通常
/// 有模块级的全局状态（缓存的 token、配置对象），共享会互相污染。
pub struct MainJsRunner {
    runtime: crate::js_bridge::JobRuntime,
    /// mainJs 是否已载入（未载入时调用动作会报错，
    /// 而不是得到 `undefined is not a function` 这类误导性信息）
    script_loaded: bool,
}

impl std::fmt::Debug for MainJsRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MainJsRunner")
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl MainJsRunner {
    /// 用一段 mainJs 脚本建立执行器。
    ///
    /// # 调用顺序（重要）
    ///
    /// 正确顺序是：
    ///
    /// ```text
    /// JobRuntime::new  →  install_bridge  →  eval(mainJs)  →  run(action)
    ///                      ↑                                     ↑
    ///                  必须先注入桥                     脚本的顶层代码会用到宿主能力
    /// ```
    ///
    /// ## 为什么构造分两步而不是"new 里一把做完"
    ///
    /// `mainJs` 的**顶层代码会调用宿主函数**（书源的 JS 源里常见：
    /// 顶层算签名密钥、读缓存里的 token）。而宿主桥经 `Ctx::userdata`
    /// 存放、必须在**任何书源脚本求值之前**注入（见
    /// `tome_js::Sandbox::install_host_bridge` 的说明）。
    ///
    /// 若 `new(script, limits)` 在内部先跑脚本，组合根就永远来不及注入
    /// 桥——本想图方便的一步构造，会让"顶层用宿主能力"的源全部失效。
    ///
    /// 故拆成 [`MainJsRunner::new`]（只建 runtime + prelude）与
    /// [`MainJsRunner::load`]（注入桥后再跑脚本）。这个顺序由类型强制：
    /// 只有 `new` 出的实例能装桥，而装桥后必须显式 `load` 才能用。
    pub fn new(limits: tome_js::Limits) -> Result<Self, RuleError> {
        let runtime = crate::js_bridge::JobRuntime::new(limits).map_err(to_rule_error)?;
        Ok(Self {
            runtime,
            script_loaded: false,
        })
    }

    /// 注入宿主能力（组合根装配）。**必须在 [`Self::load`] 之前**。
    pub fn install_bridge(&mut self, bridge: HostBridge) -> Result<(), RuleError> {
        self.runtime.install_bridge(bridge).map_err(to_rule_error)
    }

    /// 载入 mainJs 脚本（执行其顶层代码，完成模块级初始化）。
    ///
    /// 见 [`Self::new`] 的调用顺序说明：桥必须已经注入。
    pub fn load(&mut self, script: &str) -> Result<(), RuleError> {
        self.runtime
            .eval(script)
            .map_err(|error| script_error(&error, "mainJs 脚本初始化失败"))?;
        self.script_loaded = true;
        Ok(())
    }

    /// 便捷构造：建 runtime、注入桥、载入脚本（三步按正确顺序做完）。
    ///
    /// 供"脚本不需要宿主能力"的场景使用（纯计算源、测试）。
    /// 需要宿主能力的源应走显式的三步，以便在装完桥后**再补装**
    /// 宿主函数（注册 `java.*` 也是装配的一部分）。
    pub fn with_script(
        script: &str,
        limits: tome_js::Limits,
        bridge: HostBridge,
    ) -> Result<Self, RuleError> {
        let mut runner = Self::new(limits)?;
        runner.install_bridge(bridge)?;
        // 宿主函数（`java.*`）的注册也必须在脚本求值之前完成——
        // 但注册需要 `Ctx`，故由调用方经 `with_context` 完成。
        // 此处只保证桥已就位。
        runner.load(script)?;
        Ok(runner)
    }

    /// 沙箱是否已废弃（曾触发 OOM，不得复用）。
    ///
    /// 与超时的区别很重要：**超时不废弃沙箱**（脚本下次调用可能就正常了），
    /// 只有内存耗尽才废弃（那时 runtime 的内部状态不可信）。组合根据此
    /// 决定"继续用这个 runner"还是"换一个"。
    pub fn is_poisoned(&self) -> bool {
        self.runtime.is_poisoned()
    }

    /// 在沙箱内装配全局对象（注册宿主函数、上下文对象）。
    pub fn with_context<T>(
        &self,
        callback: impl FnOnce(Ctx<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.runtime.with_context(callback)
    }

    /// 执行一个动作。
    ///
    /// `argument` 的含义随动作变化（关键词 / URL）。
    pub fn run(
        &self,
        action: ScriptAction,
        argument: &str,
        page: u32,
    ) -> Result<ScriptOutcome, RuleError> {
        // 入口必须存在且是函数（见模块文档第 1 条）。
        self.require_entry(action)?;

        let call = build_call(action, argument, page);
        let value = self
            .runtime
            .eval(&call)
            .map_err(|error| script_error(&error, &format!("{} 执行失败", action.action_name())))?;

        // 返回值形状校验（见模块文档第 2/3 条）。
        validate_outcome(action, value)
    }

    /// 检查某个入口是否存在且可调用。
    ///
    /// ## 为什么单独一步而不是让调用直接失败
    ///
    /// 缺入口时引擎抛的是 `undefined is not a function`——书源作者看到
    /// 那个信息不知道是"我的脚本没实现这个动作"还是"引擎调错了名字"。
    /// 故先显式检查并给出指明入口名的错误。
    fn require_entry(&self, action: ScriptAction) -> Result<(), RuleError> {
        if !self.script_loaded {
            return Err(RuleError::syntax(
                "mainJs 脚本尚未载入（组合根需在注入宿主桥后调用 load）；                 未载入时所有入口都不可用"
                    .to_string(),
            ));
        }
        let name = action.entry_name();
        let check = format!("typeof {name}");
        let value = self.runtime.eval(&check).map_err(to_rule_error)?;
        if value.text != "function" {
            return Err(RuleError::syntax(format!(
                "纯脚本源缺少入口函数 `{name}()`（{} 动作不可用）；\
                 实际类型是 `{}`。公开书源格式要求 JS 源按入口名定义函数。",
                action.action_name(),
                value.text
            )));
        }
        Ok(())
    }
}

/// 一次动作的产物。
///
/// # 为什么区分"列表"与"文本"
///
/// 两类结果的后续处理路径完全不同：列表要逐项映射成引擎的 DTO
/// （书籍/章节），文本直接进正文管线。用同一个枚举承载让调用方**必须**
/// 分支处理——若用"统一的字符串"承载，列表会被序列化成 JSON 文本，
/// 而调用方要自己再解析一遍（那一步极易漏掉错误处理）。
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptOutcome {
    /// 列表结果（search / chapterList）：已转成 JSON 值
    ///
    /// 保持 `serde_json::Value` 而不是直接转成引擎的 DTO：DTO 的映射
    /// 需要知道"书源里字段叫什么"（`name`/`author`/…），那是
    /// **调用方**（tome-source）的知识，本模块不该假定。
    List(JsonValue),
    /// 单对象结果（bookInfo）：已转成 JSON 值
    Object(JsonValue),
    /// 文本结果（chapterContent）
    Text(String),
}

impl ScriptOutcome {
    /// 取 JSON 值（文本结果返回 `None`）。
    pub fn as_json(&self) -> Option<&JsonValue> {
        match self {
            Self::List(value) | Self::Object(value) => Some(value),
            Self::Text(_) => None,
        }
    }

    /// 取文本（非文本结果返回 `None`）。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// 构造调用入口的 JS 表达式。
///
/// # 为什么参数经 `JSON.stringify` 注入而不是拼字面量
///
/// 关键词里可能有引号、反斜杠、换行（用户输入的关键词不可信）。
/// 拼接字符串字面量要处理全部转义情形，而漏一处就是语法错——
/// 且那类错误只在特定关键词下出现（很难复现）。
///
/// `JSON.stringify` 产出的字符串字面量是 JS 兼容的（JSON 是 JS 字面量
/// 的子集），且它替我们处理全部转义。
///
/// ## `[UNCERTAIN]` U+2028/U+2029 的边界
///
/// 严格说 `JSON.stringify` 不转义这两个字符（JSON 允许它们裸写），
/// 而它们在 JS 里是行终止符。实践中用户关键词含这两个字符的概率极低，
/// 且本书源格式的 URL 也不接受它们（会被 URL 编码）。故不额外处理；
/// 若发现真实案例，应在 `serde_json` 的输出上再替换一次。
fn build_call(action: ScriptAction, argument: &str, page: u32) -> String {
    let name = action.entry_name();
    let argument = json_string(argument);
    match action {
        // `search(key, page)`：两个参数
        ScriptAction::Search => format!("JSON.stringify({name}({argument}, {page}))"),
        // 其余三个入口只接收 URL
        _ => format!("JSON.stringify({name}({argument}))"),
    }
}

/// 把一个 Rust 字符串编成 JS 字符串字面量（等价于 `JSON.stringify`）。
///
/// 用 `serde_json` 而非手写：手写要覆盖引号/反斜杠/控制字符三类，
/// 漏一处就是语法错。
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// 校验并归一返回值（见模块文档第 2/3 条）。
fn validate_outcome(
    action: ScriptAction,
    value: tome_js::JsValue,
) -> Result<ScriptOutcome, RuleError> {
    // ## 为什么用 JSON 中转而不是逐类型处理
    //
    // 入口函数的返回值是**脚本写的**，形态五花八门：数组字面量、对象、
    // 字符串、数字。而我们要的结构（书籍/章节列表）是 JSON 形态。
    //
    // `JSON.stringify` 在引擎侧把任意值归一成 JSON 文本，我们再解析——
    // 这一路只有两步，且语义由 JSON 规范定义（不会有"我们自己转换时
    // 漏了某个类型"的缺陷）。
    let text = &value.text;

    // 未定义/空：脚本没返回东西
    if value.is_nullish() {
        return Err(RuleError::syntax(format!(
            "纯脚本源的 `{}()` 没有返回内容；公开书源格式要求它返回{}",
            action.entry_name(),
            if action.expects_list() {
                "书籍/章节数组"
            } else if action == ScriptAction::ChapterContent {
                "正文字符串"
            } else {
                "书籍对象（或 JSON 文本）"
            }
        )));
    }

    let parsed: JsonValue = serde_json::from_str(text).map_err(|error| {
        // JSON 解析失败：脚本返回的不是 JSON（例如返回了一个函数）
        RuleError::syntax(format!(
            "纯脚本源的 `{}()` 返回值不是 JSON 形态（{error}）；\
             返回值前 200 字符：{}",
            action.entry_name(),
            preview(text)
        ))
    })?;

    match action {
        ScriptAction::ChapterContent => Ok(ScriptOutcome::Text(match parsed {
            // 合法的情形：脚本返回字符串
            JsonValue::String(text) => text,
            // 兼容：返回对象/数组时取序列化形态（书源里偶见返回
            // `{content: "..."}` 的写法，由调用方决定怎么用）
            other => other.to_string(),
        })),
        ScriptAction::BookInfo => Ok(ScriptOutcome::Object(parsed)),
        ScriptAction::Search | ScriptAction::ChapterList => {
            // ## 列表类入口必须真的返回数组（见模块文档第 2 条）
            //
            // 静默接受非数组会让"脚本写错了"表现为"这个源没有结果"——
            // 排查方向被完全带偏。故报错并带上实际形态。
            match parsed {
                JsonValue::Array(_) => Ok(ScriptOutcome::List(parsed)),
                // 兼容：`{list: [...]}` 这类包装是书源里的常见写法
                JsonValue::Object(ref map) => {
                    for key in ["list", "data", "books", "chapters", "items"] {
                        if let Some(JsonValue::Array(_)) = map.get(key) {
                            return Ok(ScriptOutcome::List(
                                map.get(key).cloned().unwrap_or(JsonValue::Null),
                            ));
                        }
                    }
                    Err(RuleError::syntax(format!(
                        "纯脚本源的 `{}()` 应返回数组（或含 `list`/`data`/`books` 的对象），\
                         实际返回了不含列表的对象：{}",
                        action.entry_name(),
                        preview(text)
                    )))
                }
                other => Err(RuleError::syntax(format!(
                    "纯脚本源的 `{}()` 应返回数组，实际是 `{}`：{}",
                    action.entry_name(),
                    json_kind_name(&other),
                    preview(text)
                ))),
            }
        }
    }
}

/// 取 JSON 值的类型名（错误消息用，给用户看不带 `Debug` 的形态）。
fn json_kind_name(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "布尔值",
        JsonValue::Number(_) => "数字",
        JsonValue::String(_) => "字符串",
        JsonValue::Array(_) => "数组",
        JsonValue::Object(_) => "对象",
    }
}

/// 截取预览（错误消息里带上实际内容，便于定位）。
///
/// 限 200 字符：脚本可能返回几 MB 的内容，而错误消息里全带上会让
/// 日志不可读。
fn preview(text: &str) -> String {
    let mut output: String = text.chars().take(200).collect();
    if text.chars().count() > 200 {
        output.push('…');
    }
    output
}

/// 把沙箱错误转成规则错误并附上前缀。
fn script_error(error: &SandboxError, context: &str) -> RuleError {
    let detail = match error {
        SandboxError::Interrupted { reason } => {
            format!("{context}：脚本被中止（{reason}）")
        }
        SandboxError::MemoryLimit { limit } => {
            format!("{context}：触及内存上限（{limit} 字节）")
        }
        SandboxError::Poisoned { reason } => {
            format!("{context}：沙箱已废弃（{reason}）")
        }
        other => format!("{context}：{other}"),
    };
    RuleError::syntax(detail)
}

/// 沙箱错误 → 规则错误（保留分类语义）。
fn to_rule_error(error: SandboxError) -> RuleError {
    match &error {
        // 配额/构造失败是**组合根的配置问题**，不是书源写错规则。
        // 映射为 `Limit`（"超出处理上界"最接近："沙箱没法用"）。
        SandboxError::Limits(_) | SandboxError::Setup { .. } => RuleError::limit(error.to_string()),
        _ => RuleError::syntax(error.to_string()),
    }
}

/// 让 `Value` 的导入有意义（`Ctx` 的辅助函数签名里用到它）。
///
/// ## 为什么保留这个看似多余的别名
///
/// `with_context` 的回调会拿到 `Ctx`，而注册宿主函数时必然要构造
/// `Value`（参数/返回值）。本别名让调用方写 `mainjs::JsValue` 时
/// 不必再引一遍 `tome_js`——那对这个模块的使用者是最常见的需求。
pub type JsValue<'js> = Value<'js>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 入口名符合公开约定() {
        // 这些名字是**契约**：既有书源按它们定义函数，改名即失效。
        assert_eq!(entry_names::SEARCH, "search");
        assert_eq!(entry_names::BOOK_INFO, "bookInfo");
        assert_eq!(entry_names::CHAPTER_LIST, "chapterList");
        assert_eq!(entry_names::CHAPTER_CONTENT, "chapterContent");
    }

    #[test]
    fn 动作与入口名对应() {
        for (action, name) in [
            (ScriptAction::Search, "search"),
            (ScriptAction::BookInfo, "bookInfo"),
            (ScriptAction::ChapterList, "chapterList"),
            (ScriptAction::ChapterContent, "chapterContent"),
        ] {
            assert_eq!(action.entry_name(), name);
            assert_eq!(action.action_name(), name);
        }
    }

    #[test]
    fn 列表类动作被识别() {
        assert!(ScriptAction::Search.expects_list());
        assert!(ScriptAction::ChapterList.expects_list());
        assert!(!ScriptAction::BookInfo.expects_list());
        assert!(!ScriptAction::ChapterContent.expects_list());
    }

    #[test]
    fn 调用表达式按动作形状生成() {
        // search 带两个参数
        let call = build_call(ScriptAction::Search, "斗破", 2);
        assert!(call.contains("search("));
        assert!(call.contains(", 2)"), "应传页码：{call}");
        assert!(call.contains("JSON.stringify"), "应经 JSON 归一：{call}");

        // 其余只带 URL
        let call = build_call(ScriptAction::ChapterContent, "https://x/1", 1);
        assert!(call.contains("chapterContent("));
        assert!(!call.contains(", 1)"), "不应传页码：{call}");
    }

    #[test]
    fn 参数里的引号被正确转义() {
        // 关键词含引号是真实场景（用户可能搜 `他说"你好"`）
        let call = build_call(ScriptAction::Search, r#"他说"你好""#, 1);
        // 转义后不应有裸露的引号打断字符串
        assert!(call.contains(r#"\"你好\""#), "引号应被转义：{call}");
    }

    #[test]
    fn 参数里的反斜杠与换行被正确转义() {
        let call = build_call(ScriptAction::Search, "a\\b\nc", 1);
        assert!(call.contains(r"\\"), "反斜杠应转义：{call}");
        assert!(!call.contains('\n'), "换行不应裸出现：{call}");
    }

    #[test]
    fn 预览截断长内容() {
        let long = "x".repeat(500);
        let previewed = preview(&long);
        assert!(
            previewed.chars().count() <= 201,
            "应截断：{}",
            previewed.len()
        );
        assert!(previewed.ends_with('…'));

        let short = "abc";
        assert_eq!(preview(short), "abc", "短内容不应加省略号");
    }

    #[test]
    fn 预览按字符截断而非字节() {
        // 中文每字 3 字节，按字节截断会切出半个字符
        let chinese = "中".repeat(300);
        let previewed = preview(&chinese);
        assert_eq!(previewed.chars().count(), 201);
        // 截断后应仍是合法 UTF-8（能被 chars 迭代完）
        assert_eq!(previewed.chars().filter(|c| *c == '中').count(), 200);
    }

    #[test]
    fn json_类型名可读() {
        assert_eq!(json_kind_name(&JsonValue::Null), "null");
        assert_eq!(json_kind_name(&JsonValue::Array(vec![])), "数组");
        assert_eq!(
            json_kind_name(&JsonValue::Object(Default::default())),
            "对象"
        );
        assert_eq!(json_kind_name(&JsonValue::String(String::new())), "字符串");
    }

    #[test]
    fn json_字符串转义() {
        assert_eq!(json_string("abc"), "\"abc\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
    }
}
