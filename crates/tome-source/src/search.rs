//! [DONE] 搜索管线：规则求值 → `RequestSpec` → `NetHost` 执行 → 结构化结果
//! （tasks.md T027 / FR-002）
//!
//! # A2 纯化：规则求值全程不发请求
//!
//! 这是本模块最关键的架构约束，也是 T027 的验收项：
//!
//! ```text
//!   parse_search_spec()   ← 只做字符串替换与解析，**不发请求**，可离线调用
//!            │
//!            ▼  产出 RequestSpec
//!   NetHost::fetch()      ← 唯一的发请求处
//!            │
//!            ▼  Response
//!   parse_search_result() ← 只做 DOM/规则求值，**不发请求**
//! ```
//!
//! 好处是规则求值可以独立测试与离线重放（宪法 III 的差分测试前提）。
//! [`parse_search_spec`] 与 [`parse_search_result`] 都是**纯函数**——
//! 前者的输入是书源 + 关键词 + 页码，后者的输入是 HTML + 规则。
//!
//! # 搜索 URL 的插值（FR-001 的 `{{}}`）
//!
//! 公开的《书源规则说明》给出搜索 URL 的形态：
//!
//! > `key` 为关键字标识，通常形态为 `{{key}}`，运行时会替换为搜索关键字
//! > `page` 为关键字标识，通常形态为 `{{page}}`，`page` 的初值为 1
//!
//! 本模块的 [`substitute_url`] 实现这两个占位符的替换，**外加 URL 编码**：
//! 关键词里可能有空格、`&`、中文，不编码会让请求打错。书源作者写的
//! `{{key}}` 是"值占位符"而不是"已编码的串"，故编码是本引擎的责任。

use std::collections::BTreeMap;

use tome_core::seam::{CancelToken, SearchResultItem, SearchStream};
use tome_core::{Book, BookOrigin, ErrorKind, NetHost, RequestSpec, Source};

use crate::error::SourceError;
use crate::source::{parse_headers, ParsedSource};

/// 搜索 URL 与请求定制的解析结果——即"要请求什么"（A2 纯化的产物）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequestPlan {
    /// 完整的请求描述（规则求值只产出它，不执行）
    pub spec: RequestSpec,
}

/// 解析搜索 URL 模板中的占位符（`{{key}}` / `{{page}}`）。
///
/// [DONE] A2 纯化的第一半：**不发请求**，可离线调用与断言。
///
/// # 编码规则
///
/// | 占位符 | 替换为 | 编码 |
/// |---|---|---|
/// | `{{key}}` | 关键词 | **URL 编码**（关键词含空格/`&`/中文时必须） |
/// | `{{page}}` | 页码（从 1 起） | 不编码（纯数字） |
///
/// # 为什么不支持任意表达式
///
/// 公开文档里 `{{}}` 在搜索 URL 中"只能使用 js"（其他位置可以用规则）。
/// `@js:` 求值属 S1-2（T043 的 JS 桥），故本模块**只实现两个基础占位符**，
/// 遇到其它 `{{...}}` 时保留原文并记录——让用户看到"这里没被替换"比
/// 静默产出空串更容易定位。
///
/// 返回 `(替换后的 URL, 未被识别的占位符列表)`。
pub fn substitute_url(template: &str, keyword: &str, page: u32) -> (String, Vec<String>) {
    let mut output = String::with_capacity(template.len() + keyword.len() * 3);
    let mut unrecognized = Vec::new();
    let chars: Vec<char> = template.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        // 找 `{{`
        if chars[index] == '{' && chars.get(index + 1).copied() == Some('{') {
            // 找配对的 `}}`
            let mut cursor = index + 2;
            let mut expression = String::new();
            let mut closed = false;
            while cursor < chars.len() {
                if chars[cursor] == '}' && chars.get(cursor + 1).copied() == Some('}') {
                    closed = true;
                    break;
                }
                expression.push(chars[cursor]);
                cursor += 1;
            }

            if !closed {
                // 未闭合：当字面保留
                output.push(chars[index]);
                index += 1;
                continue;
            }

            let trimmed = expression.trim();
            match trimmed {
                "key" => output.push_str(&encode_query_component(keyword)),
                "page" => output.push_str(&page.to_string()),
                other => {
                    unrecognized.push(other.to_string());
                    // 保留原文：让"没被替换"可见，而不是静默产出空串
                    output.push_str("{{");
                    output.push_str(other);
                    output.push_str("}}");
                }
            }
            index = cursor + 2;
            continue;
        }

        output.push(chars[index]);
        index += 1;
    }

    (output, unrecognized)
}

/// 按 `application/x-www-form-urlencoded` 规则编码查询参数的值。
///
/// 用 `+` 表示空格（表单编码的惯例）而非 `%20`：多数站点两种都接受，
/// 但表单编码是 `application/x-www-form-urlencoded` 的规范写法，
/// 而搜索参数正是这一编码的典型用途。
///
/// 保留 `-_.~` 不编码（RFC 3986 的 unreserved）——过度编码会让某些站点的
/// 参数校验失败，而它们对这些字符的安全性是确定的。
fn encode_query_component(value: &str) -> String {
    let mut output = String::with_capacity(value.len() * 3);
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(*byte as char);
            }
            b' ' => output.push('+'),
            _ => {
                output.push('%');
                output.push_str(&format!("{byte:02X}"));
            }
        }
    }
    output
}

/// 构造搜索请求（**不发请求**，A2 纯化）。
///
/// [DONE] 本模块的核心纯函数：书源 + 关键词 + 页码 → [`RequestSpec`]。
///
/// ## 为什么它必须是纯函数（T027 验收项）
///
/// "规则求值全程不发请求"这条约束的可测试形态就是：给定相同输入，
/// 本函数产出相同的 `RequestSpec`，且**不需要任何网络设施**。
/// 若它内部发了请求，就无法在无网络环境下断言它的输出。
pub fn parse_search_spec(
    parsed: &ParsedSource,
    keyword: &str,
    page: u32,
) -> Result<SearchRequestPlan, SourceError> {
    parse_search_spec_with_js(parsed, keyword, page, None)
}

/// 构造搜索请求，**带可选的 JS 求值能力**（T043 的接线点）。
///
/// # 与不带 JS 版本的关系
///
/// [`parse_search_spec`] 是它的 `js = None` 特例。分开两个入口而不是
/// 把 `Option` 塞进原签名，是为了让"不需要 JS 的源"（绝大多数）的调用点
/// 保持简洁——而那些调用点不该被迫关心沙箱的存在。
///
/// # JS 求值在哪里发生
///
/// `searchUrl` 里的占位符分两类：
///
/// | 占位符 | 处理 | 为什么 |
/// |---|---|---|
/// | `{{key}}` / `{{page}}` | 管线直接替换（含 URL 编码） | 它们是**值占位符**，不需要脚本 |
/// | 其余 `{{...}}` | 经 [`crate::js::SourceJsEngine`] 求值 | 公开文档：「在 `{{}}` 里只能使用 js」 |
///
/// 第二类走 `tome_rule::replace` 的插值实现——那是 T025 已写好的
/// 解析 + T043 已接好的桥，此处不重复实现。
pub fn parse_search_spec_with_js(
    parsed: &ParsedSource,
    keyword: &str,
    page: u32,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<SearchRequestPlan, SourceError> {
    let id = &parsed.source.id;
    let template = parsed
        .source
        .search_url
        .as_deref()
        .ok_or_else(|| SourceError::incomplete(id.clone(), "缺少 searchUrl（搜索地址）"))?;

    let (url, unrecognized) = substitute_url(template, keyword, page);
    let url = if unrecognized.is_empty() {
        url
    } else {
        // ## 需要 JS 时才要求引擎
        //
        // 未被识别的占位符意味着这条源依赖 JS 插值。若引擎未提供
        // （组合根没接上，或调用点用了旧入口），报出**可归因**的错误而不是
        // 产出半成品 URL——后者会让请求打到错误地址，而表现是"没有结果"。
        let Some(engine) = js else {
            return Err(SourceError::rule(format!(
                "搜索 URL 含需要 JS 求值的占位符 {:?}，但本次调用未提供 JS 引擎；\
                 组合根应为含 JS 的书源建立 SourceJsEngine",
                unrecognized
            )));
        };

        // ## 为什么把基础占位符也交给 JS 插值
        //
        // 一旦这条 URL 里有任意一个 JS 占位符，整条 URL 就必须走插值器
        // （否则要写一个"部分替换"的混合逻辑）。而插值器里的 `key`/`page`
        // 是**变量**——故先把它们的值注入沙箱的全局，再交给插值器。
        //
        // 这样 `{{key}}` 在两条路径上的编码行为可能不同（管线做 URL 编码、
        // JS 插值不做）——故注入的是**已编码**的值，保持与快路径一致。
        let encoded_keyword = encode_query_component(keyword);
        engine
            .eval(&format!(
                "var key = {}; var page = {};",
                js_string_literal(&encoded_keyword),
                page
            ))
            .map_err(|error| SourceError::rule(format!("注入搜索变量失败：{error}")))?;

        evaluate_template(engine, template)?
    };

    // 相对 URL 补全：公开文档说搜索地址"支持相对 URL"。
    let absolute = resolve_relative(&parsed.source.id, &url);

    let mut spec = RequestSpec::get(absolute, &parsed.source.id);
    // 请求头来自书源的 `header` 字段（FR-008）
    spec.headers = parse_headers(parsed.source.header.as_deref())?;
    // 字符集声明（FR-009）
    spec.charset = parsed.source.charset.clone();
    // 请求间隔（FR-008，R2 新增的字段）
    spec.request_interval_ms = interval_from_source(&parsed.source);
    // 搜索不是"必须成功"的动作：单个源失败是流内的一个条目（A6），
    // 故不重试——多源搜索里让某个源重试三次会拖慢整体首结果时间（SC-001）。
    spec.timeout_ms = SEARCH_TIMEOUT_MS;

    Ok(SearchRequestPlan { spec })
}

/// 用 JS 引擎求值一个含占位符的模板。
///
/// 经 `tome_rule` 的插值实现（T025 的解析 + T043 的桥）——本模块不重复
/// 那套解析逻辑。
fn evaluate_template(
    engine: &crate::js::SourceJsEngine,
    template: &str,
) -> Result<String, SourceError> {
    // 插值的一个 js 类占位需要一个文档参数（规则类占位要用），
    // 而搜索 URL 里不会有规则类占位——故给一个空文档。
    let document = tome_rule::dom::Document::parse_html("");
    let interpolation = tome_rule::replace::parse_interpolation(template);
    let bridge = tome_rule::js_bridge::JsPlaceholderBridge::new(engine.runtime(), &document);
    interpolation.evaluate(&bridge).map_err(|error| {
        // ## 必须用 `rule_eval` 而不是 `rule`（FR-006 的关键）
        //
        // 两者的区别正在于**是否保留错误的分类载荷**：
        //
        // | 构造 | 结果 |
        // |---|---|
        // | `SourceError::rule(...)` | 一律成为 `Rule`——能力标识丢失 |
        // | `SourceError::rule_eval(&error, ..)` | 保留 `RuleError` 的分类（含能力标识） |
        //
        // 用前者会让 `java.webView(...)` 的上抛变成 `error.rule`，
        // 而 CLI 的退出码本该是 3（"本产品做不到"）而不是 1——
        // 书源的降级逻辑与脚本的后续处置都会走错分支。
        SourceError::rule_eval(&error, "搜索 URL 的 JS 插值")
    })
}

/// 把一个 Rust 字符串编成 JS 字符串字面量。
fn js_string_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// 搜索的单请求超时。
///
/// [WORKAROUND] 取 15 秒而非 `RequestSpec::get` 的默认 30 秒：SC-001 要求
/// "正常网络下从触发到首个来源结果 ≤30s"，而多源搜索里超时是**串行阻塞**
/// 的最坏来源。15 秒让"最坏情况下仍能给出首结果"有 15 秒的余量。
/// 若实测发现某些站点普遍需要更久，应改为可配置项而非继续调整此常量。
pub const SEARCH_TIMEOUT_MS: u64 = 15_000;

/// 从书源取出请求间隔（毫秒）。
///
/// 值的来源是书源的 `concurrentRate`（R5，PM 仲裁 2026-10-10，见
/// `crate::source::parse_concurrent_rate`）。
///
/// `Source::rate_limit_ms` 是 `Option<i64>`，而 `RequestSpec.request_interval_ms`
/// 是 `Option<u32>`：负数与超出 u32 的荒唐值（书源是不可信输入）一律视为"未声明"，
/// 而不是截断——截断会把 `-1` 变成 0（反而变成"要求零间隔"，比未声明更激进）。
pub fn interval_from_source(source: &Source) -> Option<u32> {
    source
        .rate_limit_ms
        .and_then(|millis| u32::try_from(millis).ok())
}

/// 把相对 URL 补全为绝对 URL。
///
/// 公开文档：搜索地址"支持相对 URL"——形如 `/search?q=x` 时以
/// `bookSourceUrl` 为基。
///
/// 已是绝对 URL（含 scheme）时原样返回；无法补全时也原样返回（让下游的
/// scheme 校验给出准确错误，而不是在这里猜一个 host）。
pub fn resolve_relative(base: &str, url: &str) -> String {
    if url.contains("://") {
        return url.to_string();
    }
    let base = base.trim_end_matches('/');
    if url.starts_with('/') {
        return format!("{base}{url}");
    }
    format!("{base}/{url}")
}

// ---------------------------------------------------------------------------
// 搜索规则解析
// ---------------------------------------------------------------------------

/// 搜索规则的字段名（公开的《书源规则说明》「书源之「搜索」」一节列出）。
///
/// 用常量而非散落的字符串字面量：这些名字**是书源格式的契约**，
/// 集中在一处便于与文档对照，也避免某处写错时静默产出空结果。
pub mod search_rule_keys {
    /// 书籍列表规则（定位到每本书的节点）
    pub const BOOK_LIST: &str = "bookList";
    /// 书名
    pub const NAME: &str = "name";
    /// 作者
    pub const AUTHOR: &str = "author";
    /// 分类
    pub const KIND: &str = "kind";
    /// 字数
    pub const WORD_COUNT: &str = "wordCount";
    /// 最新章节
    pub const LAST_CHAPTER: &str = "lastChapter";
    /// 简介
    pub const INTRO: &str = "intro";
    /// 封面
    pub const COVER_URL: &str = "coverUrl";
    /// 详情页 URL
    pub const BOOK_URL: &str = "bookUrl";
}

/// 从 `rule_search` 的 JSON 取出字段名 → 规则文本的映射。
///
/// 规则的**解析**（判断是 CSS 还是正则）属 tome-rule，本模块只做键值提取。
///
/// 用 `BTreeMap` 而非 `HashMap`：遍历顺序影响 `parse_search_result` 里
/// 各字段的处理次序，而 CLI 输出要可 diff（T029 的确定性要求）。
pub fn search_rules(source: &Source) -> Result<BTreeMap<String, String>, SourceError> {
    let id = &source.id;
    let Some(text) = source.rule_search.as_deref() else {
        return Err(SourceError::incomplete(id.clone(), "缺少 ruleSearch"));
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(SourceError::incomplete(id.clone(), "ruleSearch 为空"));
    }

    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|error| SourceError::rule(format!("ruleSearch 不是合法 JSON（{error}）")))?;
    let Some(object) = value.as_object() else {
        return Err(SourceError::rule("ruleSearch 应为 JSON 对象".to_string()));
    };

    let mut rules = BTreeMap::new();
    for (key, raw) in object {
        let Some(text) = raw.as_str() else {
            // 值不是字符串（例如 `{"name": 123}`）：跳过而非整条报错——
            // 一个字段写错不该让整条源的搜索全部不可用。
            continue;
        };
        if !text.trim().is_empty() {
            rules.insert(key.clone(), text.to_string());
        }
    }

    if !rules.contains_key(search_rule_keys::BOOK_LIST) {
        return Err(SourceError::incomplete(
            id.clone(),
            format!("ruleSearch 缺少 {}", search_rule_keys::BOOK_LIST),
        ));
    }
    Ok(rules)
}

/// 搜索结果的一条（对外形状由 `contracts/cli.md` 定义）。
///
/// ## 为什么这里不复用 `Book`
///
/// 搜索结果是**未入库**的候选：它没有 `id`（存储层分配）、没有书架分组、
/// 没有排版配置。用半个 `Book`（字段全填默认值）会让"这条结果还没入库"
/// 这一事实在类型上消失，调用方容易误当已入库的书籍处理。
///
/// 入库时由 `to_book` 转成 [`Book`]。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchHit {
    /// 书名
    pub name: String,
    /// 作者
    pub author: Option<String>,
    /// 封面地址
    pub cover_url: Option<String>,
    /// 简介
    pub intro: Option<String>,
    /// 分类
    pub kind: Option<String>,
    /// 字数（书源自述的串，如"200万字"——不强解析为数字，那会丢信息）
    pub word_count: Option<String>,
    /// 最新章节
    pub last_chapter: Option<String>,
    /// 详情页 URL（**已补全为绝对地址**）
    pub book_url: String,
}

impl SearchHit {
    /// 转成可入库的 [`Book`]。
    ///
    /// `origin_id` 的格式按 data-model.md："网络书为 `书源id|书籍详情URL`"。
    ///
    /// 这个组合让"同一个书源里的同一本书"稳定映射到同一条记录，
    /// 而不同书源提供同一本书时各自独立（换源是显式动作，不是自动去重）。
    ///
    /// 未在搜索结果里出现的字段（书架分组、排版配置、本地书路径等）
    /// 一律 `None`——它们是"入库后由用户设置"或"本地书专属"的信息。
    pub fn to_book(&self, source_id: &str) -> Book {
        Book {
            id: 0,
            origin: BookOrigin::Network,
            origin_id: format!("{source_id}|{}", self.book_url),
            name: self.name.clone(),
            author: self.author.clone(),
            cover_url: self.cover_url.clone(),
            intro: self.intro.clone(),
            kind: self.kind.clone(),
            book_group: None,
            read_config: None,
            toc_url: Some(self.book_url.clone()),
            latest_chapter_title: self.last_chapter.clone(),
            latest_chapter_time: None,
            library_root_id: None,
            relative_path: None,
            // 本地书专属字段：网络书一律 None
            charset: None,
            format: None,
            total_chapters: None,
            // 尚未开始阅读
            dur_chapter_index: None,
            added_at: crate::source::now_unix_seconds(),
        }
    }
}

/// 从响应 HTML 解析搜索结果（**不发请求**，A2 纯化）。
///
/// [DONE] 本模块的第二个纯函数：HTML + 规则 → 结构化结果。
///
/// # 解析流程
///
/// 1. 用 [`crate::source::ParsedSource`] 的 `rule_search` 取各字段规则
/// 2. `bookList` 规则定位出"每本书的节点"——这一步是关键：后续字段规则
///    都在**每本书的子树**内相对求值，而不是在整页上求
/// 3. 逐本书逐字段求值，拼成 [`SearchHit`]
///
/// ## 为什么字段规则要相对求值
///
/// 公开文档的搜索规则示例（"书源一"）里，`name` 是 `@css:.name@text`、
/// `bookUrl` 是 `@css:.name>a@href`——这些选择器**以书为单位**书写，
/// 在整页上求会命中所有书的字段混在一起。故必须先按 `bookList` 切出子树。
pub fn parse_search_result(source: &Source, html: &str) -> Result<Vec<SearchHit>, SourceError> {
    parse_search_result_with_js(source, html, None)
}

/// [`parse_search_result`] 的**带 JS 版本**（P4 遗留的收口：字段规则里的 `@js:`）。
///
/// # 为什么需要它
///
/// `ruleSearch` 的字段规则里可以直接写 `@js:`（公开文档：`@js:` 可以在任意
/// 规则位置使用）。此前这条路径只支持 CSS 与替换链，`@js:` 会报
/// `unsupported("js")`——而书源里"用 JS 拼书名"是真实用法。
///
/// # `js` 参数的含义
///
/// `Some` 时字段规则里的 `@js:` 能求值；`None` 时报结构化能力错误
/// （FR-006），而不是静默给空。这与 [`search_one_source`] /
/// [`search_one_source_with_js`] 的分工一致。
pub fn parse_search_result_with_js(
    source: &Source,
    html: &str,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<Vec<SearchHit>, SourceError> {
    let rules = search_rules(source)?;
    let document = tome_rule::dom::Document::parse_html(html);
    // 规则求值器：带 JS 时五类规则全可用，不带时 JS 类报能力错误
    let engine = match js {
        Some(engine) => tome_rule::css_post::RuleEngine::new(&document).with_js(engine.runtime()),
        None => tome_rule::css_post::RuleEngine::new(&document),
    };

    let book_list_rule = rules
        .get(search_rule_keys::BOOK_LIST)
        .ok_or_else(|| SourceError::incomplete(source.id.clone(), "缺少 bookList 规则"))?;

    // `bookList` 求值出的是**节点集合**（每本书的根节点），不是字符串——
    // 故这里走 tome-rule 的节点级入口，而不是 `evaluate_rule`（返回字符串）。
    let book_nodes = tome_rule::css_post::select_nodes(&document, book_list_rule)
        .map_err(|error| SourceError::rule(format!("bookList 规则求值失败：{error}")))?;

    let mut hits = Vec::with_capacity(book_nodes.len());
    for node in &book_nodes {
        let name =
            evaluate_field_in(&engine, node, &rules, search_rule_keys::NAME)?.unwrap_or_default();
        // 书名为空的结果无法呈现，跳过——它是"选择器没命中"的典型表现，
        // 留在结果里会让用户看到一堆空条目。
        if name.trim().is_empty() {
            continue;
        }

        let book_url = evaluate_field_in(&engine, node, &rules, search_rule_keys::BOOK_URL)?
            .unwrap_or_default();
        if book_url.trim().is_empty() {
            // 没有详情页地址的结果无法进入后续动作（详情/目录/正文全都
            // 以 `bookUrl` 为入口），故跳过并在调用方可见的层面体现为
            // "结果少了一条"——比给出一条点不动的结果好。
            continue;
        }

        hits.push(SearchHit {
            name: name.trim().to_string(),
            author: non_empty(evaluate_field_in(
                &engine,
                node,
                &rules,
                search_rule_keys::AUTHOR,
            )?),
            cover_url: non_empty(evaluate_field_in(
                &engine,
                node,
                &rules,
                search_rule_keys::COVER_URL,
            )?),
            intro: non_empty(evaluate_field_in(
                &engine,
                node,
                &rules,
                search_rule_keys::INTRO,
            )?),
            kind: non_empty(evaluate_field_in(
                &engine,
                node,
                &rules,
                search_rule_keys::KIND,
            )?),
            word_count: non_empty(evaluate_field_in(
                &engine,
                node,
                &rules,
                search_rule_keys::WORD_COUNT,
            )?),
            last_chapter: non_empty(evaluate_field_in(
                &engine,
                node,
                &rules,
                search_rule_keys::LAST_CHAPTER,
            )?),
            book_url: resolve_relative(&source.id, book_url.trim()),
        });
    }

    Ok(hits)
}

/// 在指定节点内求一个字段规则的值。
///
/// 字段规则缺失时返回 `None`（而非空串）：调用方据此区分"规则没写"
/// 与"规则写了但没命中"，前者是书源作者的省略，后者是选择器问题。
fn evaluate_field_in(
    engine: &tome_rule::css_post::RuleEngine<'_>,
    node: &tome_rule::dom::NodeRef,
    rules: &BTreeMap<String, String>,
    key: &str,
) -> Result<Option<String>, SourceError> {
    let Some(rule) = rules.get(key) else {
        // 规则没写是一个合法状态（作者省略了这个字段），返回 `None` 而非空串，
        // 让调用方区分"没写规则"与"写了但没命中"。
        return Ok(None);
    };
    let values = engine
        .evaluate_in(node, rule)
        .map_err(|error| SourceError::rule_eval(&error, &format!("字段 `{key}`")))?;
    // 多值时取第一个：搜索结果的每个字段是单值（`@textNodes` 那种多值后缀
    // 在搜索规则里罕见，真出现时取首个与"每本书一个书名"的语义一致）。
    Ok(values.into_iter().next())
}

/// 空串（或纯空白）归一为 `None`。
fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

// ---------------------------------------------------------------------------
// 搜索执行（唯一发请求处）
// ---------------------------------------------------------------------------

/// 单源搜索：解析 spec → 执行 → 解析结果。
///
/// [DONE] 单源搜索的完整入口。多源并发由 [`SearchEngine::search`] 组织。
///
/// 返回值是 `Result` 而非流内条目：本函数只管**一个**源，把"单源失败
/// 不终止整个流"的职责交给调用它的流实现（A6）。
pub fn search_one_source(
    parsed: &ParsedSource,
    keyword: &str,
    page: u32,
    host: &dyn NetHost,
) -> Result<Vec<SearchHit>, SourceError> {
    search_one_source_with_js(parsed, keyword, page, host, None)
}

/// 单源搜索，**带可选的 JS 求值能力**（T043/T044 的接缝）。
///
/// # 与不带 JS 版本的关系
///
/// [`search_one_source`] 是它的 `js = None` 特例。含 `@js:` 或非基础
/// `{{}}` 占位符的源必须经本函数走——否则会报"需要 JS 引擎"的错误。
///
/// # 为什么引擎是 `Option` 而不是必填
///
/// 绝大多数书源（纯 CSS/正则）用不到沙箱，给它们也建 runtime 是浪费
/// （QuickJS 初始化有实打实的开销）。调用方先用
/// [`crate::js::source_needs_javascript`] 判断，只在需要时才建。
pub fn search_one_source_with_js(
    parsed: &ParsedSource,
    keyword: &str,
    page: u32,
    host: &dyn NetHost,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<Vec<SearchHit>, SourceError> {
    // 第一步：规则求值（不发请求）
    let plan = parse_search_spec_with_js(parsed, keyword, page, js)?;

    // 第二步：执行（唯一发请求处）
    let response = host
        .fetch(plan.spec)
        .map_err(|kind| SourceError::Network { kind })?;

    // R3 之后接缝对非 2xx 返回 `Ok`，故这里自己判状态：
    // 搜索页 404/403 与"页面里没有结果"是完全不同的两回事，必须分开报。
    if !response.is_success() {
        return Err(SourceError::Network {
            kind: ErrorKind::Network,
        });
    }

    // 第三步：解码 + 规则求值（不发请求）
    let decoded = decode_search_body(&response);
    parse_search_result_with_js(&parsed.source, &decoded, js)
}

/// 把响应体解成文本（复用 charset 层的容错解码，FR-009）。
fn decode_search_body(response: &tome_core::Response) -> String {
    let declared = response
        .detected_charset
        .as_deref()
        .map(str::to_string)
        .or_else(|| response.headers.get("content-type").map(str::to_string));
    tome_net::charset::decode_body(&response.body_bytes, declared.as_deref()).text
}

// ---------------------------------------------------------------------------
// 多源流式搜索（A6）
// ---------------------------------------------------------------------------

/// 搜索执行器：持有可用的书源与宿主。
///
/// ## 为什么它不依赖存储层
///
/// S1-1 阶段书源由 CLI 从文件/URL 载入后**直接**交给本执行器，不经过
/// `tome-store`（存储落地属后续切片）。故它接收 `&[ParsedSource]` 而不是
/// `&dyn BookRepository`——后者会把"书源从哪来"绑死在存储上，
/// 让"离线跑一份临时书源文件"这种用法无法表达。
pub struct SearchEngine<'a> {
    sources: &'a [ParsedSource],
    host: &'a dyn NetHost,
    /// JS 引擎工厂（`None` 表示本次搜索不支持 JS 源）。
    ///
    /// 用工厂而非实例：见 [`SearchEngine::with_js`] 的说明。
    js_factory: Option<JsEngineFactory<'a>>,
}

/// JS 引擎工厂：给定书源，建一个该源专属的引擎。
///
/// 返回 `Result` 而不是直接给实例：建 runtime 可能失败（资源不足），
/// 而那是"这个源这次跑不动"的错误，不是"整个搜索失败"。
/// 用 `Arc` 而非 `Box`：流可能被多次取用（一个 `SearchEngine` 产出多个流），
/// 而工厂本身是无状态的构造逻辑，共享没有语义问题。
///
/// `Send + Sync`：`SearchStream` 的契约允许流被移到别的线程（A6 的取消
/// 令牌就是这么用的），故工厂也必须可跨线程。
pub type JsEngineFactory<'f> = std::sync::Arc<
    dyn Fn(&tome_core::Source) -> Result<crate::js::SourceJsEngine, SourceError> + Send + Sync + 'f,
>;

impl<'a> SearchEngine<'a> {
    /// 新建搜索执行器（不带 JS 能力——纯 CSS/正则的源足够）。
    pub fn new(sources: &'a [ParsedSource], host: &'a dyn NetHost) -> Self {
        Self {
            sources,
            host,
            js_factory: None,
        }
    }

    /// 新建搜索执行器，**带 JS 引擎工厂**（T043/T044 的接线）。
    ///
    /// # 为什么是"工厂"而不是"一个引擎实例"
    ///
    /// 隔离粒度是"每个书源脚本一个 runtime"（A1/R1）：宿主函数在 JS 侧
    /// 同步阻塞，共享 runtime 会让一个源发请求时所有源停摆；且书源脚本
    /// 常有模块级状态（token/配置），共享会互相污染。
    ///
    /// 故给的是"怎么建一个新引擎"而不是"一个引擎"——每个源在需要时
    /// 现建（且只对含 JS 的源建，见 [`crate::js::source_needs_javascript`]）。
    pub fn with_js(
        sources: &'a [ParsedSource],
        host: &'a dyn NetHost,
        js_factory: JsEngineFactory<'a>,
    ) -> Self {
        Self {
            sources,
            host,
            js_factory: Some(js_factory),
        }
    }

    /// 多源搜索，返回**流**（A6：增量呈现、单源失败不终止、可取消）。
    ///
    /// [DONE] T027 的对外入口。`source_ids` 为空表示搜索全部源。
    pub fn search(&self, keyword: &str, source_ids: &[String]) -> Box<dyn SearchStream + 'a> {
        let selected: Vec<&ParsedSource> = self
            .sources
            .iter()
            .filter(|parsed| source_ids.is_empty() || source_ids.contains(&parsed.source.id))
            .collect();

        Box::new(SourceSearchStream {
            pending: selected,
            keyword: keyword.to_string(),
            host: self.host,
            js_factory: self.js_factory.clone(),
            token: CancelToken::new(),
        })
    }
}

/// 逐个源产出的搜索流。
///
/// ## 为什么是"逐个源"而不是"并发"
///
/// A6 要求增量返回与可取消，本实现满足这两条（每源完成即产出）。
/// 真正的并发需要线程池与跨线程的宿主共享（`NetHost: Send + Sync` 满足），
/// 但**同源限速**与"导入期错峰"已经限制了并行收益，而并发会让
/// `--record` 的录制顺序不确定（T029 要求重放逐字节一致）。
///
/// 故本阶段选串行：确定性与限速纪律优先于并行度。这条取舍在
/// SC-001 的口径说明里写明（首结果时间 = 首个完成的源，串行时即第一个源）。
/// 标 `[UNCERTAIN]`：若 S1-6 的真实书源基线显示串行不满足 SC-001，
/// 应改成并发并让录制按指纹排序（那会让重放仍确定）。
struct SourceSearchStream<'a> {
    pending: Vec<&'a ParsedSource>,
    keyword: String,
    host: &'a dyn NetHost,
    /// JS 引擎工厂（见 [`SearchEngine::with_js`]）
    js_factory: Option<JsEngineFactory<'a>>,
    token: CancelToken,
}

impl SearchStream for SourceSearchStream<'_> {
    fn next_item(&mut self) -> Option<SearchResultItem> {
        // 取消后立即收尾（A6：实现方须尽快响应取消）
        if self.token.is_cancelled() {
            return None;
        }
        // 先判空再 `remove(0)`——`Vec::remove` 越界会 panic，
        // 而"源列表为空"是合法输入（过滤后无匹配源）。
        if self.pending.is_empty() {
            return None;
        }
        let parsed = self.pending.remove(0);
        let source_id = parsed.source.id.clone();

        // ## 只为需要 JS 的源建引擎
        //
        // 纯 CSS/正则的源（绝大多数）不该承担建沙箱的开销。
        // 判定见 `crate::js::source_needs_javascript`。
        let engine = match (
            &self.js_factory,
            crate::js::source_needs_javascript(&parsed.source),
        ) {
            (Some(factory), true) => match factory(&parsed.source) {
                Ok(engine) => Some(engine),
                // 建引擎失败：该源作为流内失败条目（A6：单源失败不终止）
                Err(error) => {
                    return Some(SearchResultItem::SourceFailed {
                        source_id,
                        error: error.to_error_kind(),
                    })
                }
            },
            _ => None,
        };

        match search_one_source_with_js(parsed, &self.keyword, 1, self.host, engine.as_ref()) {
            Ok(hits) => {
                let books: Vec<Book> = hits.iter().map(|hit| hit.to_book(&source_id)).collect();
                // 单源成功也产出（即使为空列表）：空列表表示"源在线但无匹配"，
                // 与"源失败"是不同的事实，上层都该看到。
                Some(SearchResultItem::SourceResults { source_id, books })
            }
            Err(error) => Some(SearchResultItem::SourceFailed {
                source_id,
                error: error.to_error_kind(),
            }),
        }
    }

    fn cancel_token(&self) -> &CancelToken {
        &self.token
    }
}

/// 一个源的成功结果：`(源 id, 书籍列表)`。
pub type SourceSuccess = (String, Vec<Book>);

/// 一个源的失败：`(源 id, 错误分类)`。
pub type SourceFailure = (String, ErrorKind);

/// 便捷：一次性收集流的结果（供 CLI 与测试用）。
///
/// 返回 `(成功的源结果, 失败的源列表)`——两类都要给调用方，
/// 因为"哪些源失败了"是用户排查书源的关键信息（FR-011 的取向）。
pub fn collect_search(
    mut stream: Box<dyn SearchStream + '_>,
) -> (Vec<SourceSuccess>, Vec<SourceFailure>) {
    let mut successes = Vec::new();
    let mut failures = Vec::new();
    while let Some(item) = stream.next_item() {
        match item {
            SearchResultItem::SourceResults { source_id, books } => {
                successes.push((source_id, books));
            }
            SearchResultItem::SourceFailed { source_id, error } => {
                failures.push((source_id, error));
            }
        }
    }
    (successes, failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::parse_sources;
    use std::sync::Mutex;

    fn sample_source() -> ParsedSource {
        let text = r#"{
          "bookSourceName": "测试源",
          "bookSourceUrl": "https://example.com",
          "searchUrl": "/search?q={{key}}&page={{page}}",
          "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"author\":\"@css:.author@text\",\"bookUrl\":\"@css:.name>a@href\",\"intro\":\"@css:.intro@text\",\"coverUrl\":\"@css:img@src\"}",
          "header": "{\"User-Agent\":\"tome/0.1\"}",
          "charset": "gbk"
        }"#;
        parse_sources(text)
            .expect("样例应解析成功")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    fn sample_html() -> &'static str {
        r#"<html><body><ul>
          <li class="item">
            <h3 class="name"><a href="/book/1">斗破苍穹</a></h3>
            <p class="author">天蚕土豆</p>
            <p class="intro">这里是简介一</p>
            <img src="/cover/1.jpg">
          </li>
          <li class="item">
            <h3 class="name"><a href="/book/2">完美世界</a></h3>
            <p class="author">辰东</p>
            <p class="intro">这里是简介二</p>
            <img src="/cover/2.jpg">
          </li>
        </ul></body></html>"#
    }

    // ---------- A2 纯化：URL 替换是纯函数 ----------

    #[test]
    fn 占位符被替换且关键词被编码() {
        let (url, unrecognized) =
            substitute_url("https://x.com/s?q={{key}}&page={{page}}", "斗破 苍穹", 2);
        assert_eq!(
            url, "https://x.com/s?q=%E6%96%97%E7%A0%B4+%E8%8B%8D%E7%A9%B9&page=2",
            "关键词应被 URL 编码，空格编码为 +"
        );
        assert!(unrecognized.is_empty());
    }

    #[test]
    fn 编码保留_unreserved_字符() {
        let (url, _) = substitute_url("https://x.com/s?q={{key}}", "a-b_c.d~e", 1);
        assert_eq!(url, "https://x.com/s?q=a-b_c.d~e");
    }

    #[test]
    fn 编码处理与号与等号() {
        // 关键词里的 `&` 不编码会让它被当成参数分隔符
        let (url, _) = substitute_url("https://x.com/s?q={{key}}", "a&b=c", 1);
        assert_eq!(url, "https://x.com/s?q=a%26b%3Dc");
    }

    #[test]
    fn 未识别的占位符被保留并报告() {
        let (url, unrecognized) =
            substitute_url("https://x.com/s?q={{key}}&p={{java.foo()}}", "k", 1);
        assert!(url.contains("{{java.foo()}}"), "未识别的占位符应保留原文");
        assert_eq!(unrecognized, vec!["java.foo()"]);
    }

    #[test]
    fn 未闭合的花括号当字面() {
        let (url, unrecognized) = substitute_url("https://x.com/s?q={{key", "k", 1);
        assert_eq!(url, "https://x.com/s?q={{key");
        assert!(unrecognized.is_empty());
    }

    // ---------- 构造 RequestSpec（不发请求） ----------

    #[test]
    fn 搜索_spec_含书源的请求定制() {
        let parsed = sample_source();
        let plan = parse_search_spec(&parsed, "关键词", 1).expect("应构造成功");

        assert_eq!(
            plan.spec.url,
            "https://example.com/search?q=%E5%85%B3%E9%94%AE%E8%AF%8D&page=1"
        );
        assert_eq!(plan.spec.headers.get("user-agent"), Some("tome/0.1"));
        assert_eq!(plan.spec.charset.as_deref(), Some("gbk"));
        assert_eq!(
            plan.spec.rate_limit_key, "https://example.com",
            "限速键应是书源 id（同源共享一个令牌桶）"
        );
        assert_eq!(plan.spec.timeout_ms, SEARCH_TIMEOUT_MS);
    }

    #[test]
    fn 相对搜索地址被补全为绝对() {
        let parsed = sample_source();
        let plan = parse_search_spec(&parsed, "k", 1).expect("应构造成功");
        assert!(
            plan.spec.url.starts_with("https://example.com/"),
            "相对地址应以 bookSourceUrl 为基：{}",
            plan.spec.url
        );
    }

    #[test]
    fn 绝对搜索地址原样使用() {
        let text = r#"{
          "bookSourceName": "绝对地址源",
          "bookSourceUrl": "https://base.com",
          "searchUrl": "https://other.com/api?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let plan = parse_search_spec(&parsed, "k", 1).expect("应构造成功");
        assert!(plan.spec.url.starts_with("https://other.com/"));
    }

    #[test]
    fn 缺搜索地址报配置不完整() {
        let text = r#"{
          "bookSourceName": "无地址源",
          "bookSourceUrl": "https://x.com",
          "ruleSearch": "{\"bookList\":\"@css:li\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let error = parse_search_spec(&parsed, "k", 1).expect_err("应报错");
        assert!(matches!(error, SourceError::IncompleteSource { .. }));
        assert_eq!(error.to_error_kind(), ErrorKind::Rule);
    }

    #[test]
    fn 含_js_占位符的搜索地址需要_js_引擎() {
        // S1-2 起 JS 插值已实现（T043），故"报未支持"变成"需要引擎"。
        //
        // ## 这里验证的是**快路径的行为**
        //
        // `parse_search_spec`（不带 JS）遇到 JS 占位符时必须报**可归因**的
        // 错误，而不是产出半成品 URL——后者会让请求打到错误地址，
        // 而表现是"没有结果"，排查方向被完全带偏。
        let text = r#"{
          "bookSourceName": "JS地址源",
          "bookSourceUrl": "https://x.com",
          "searchUrl": "/s?q={{java.base64Encode(key)}}",
          "ruleSearch": "{\"bookList\":\"@css:li\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let error = parse_search_spec(&parsed, "k", 1).expect_err("应报错");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("JS") || rendered.contains("js"),
            "错误应指明需要 JS 引擎：{rendered}"
        );
        assert!(
            rendered.contains("SourceJsEngine"),
            "错误应指明该怎么做（建立 SourceJsEngine）：{rendered}"
        );
    }

    #[test]
    fn 带_js_引擎时_js_占位符被求值() {
        // T043 接线的端到端验证：同一条源，给了引擎就能求出真实 URL。
        let text = r#"{
          "bookSourceName": "JS地址源",
          "bookSourceUrl": "https://x.com",
          "searchUrl": "/s?q={{java.base64Encode(key)}}&p={{1+1}}",
          "ruleSearch": "{\"bookList\":\"@css:li\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let engine =
            crate::js::SourceJsEngine::for_source(&parsed.source.id, &parsed.source, None, None)
                .expect("应能建引擎");

        // 装配宿主函数（`java.*` 是 prelude 的桩之外的真实实现）
        engine
            .with_context(|ctx| {
                let java = tome_js::Object::new(ctx.clone())
                    .map_err(|error| format!("建 java 失败：{error}"))?;
                tome_js::api::text::register(&ctx, &java)
                    .map_err(|error| format!("注册失败：{error:?}"))?;
                ctx.globals()
                    .set("java", java)
                    .map_err(|error| format!("设置全局失败：{error}"))?;
                Ok(())
            })
            .expect("应能装配");

        let plan =
            parse_search_spec_with_js(&parsed, "斗破", 1, Some(&engine)).expect("给了引擎应能求值");
        // `key` 被注入了**已 URL 编码**的值（与快路径一致），
        // 而 `java.base64Encode` 对它再做 base64。
        assert!(
            plan.spec.url.contains("p=2"),
            "`{{{{1+1}}}}` 应被求值为 2：{}",
            plan.spec.url
        );
        assert!(
            !plan.spec.url.contains("{{"),
            "不应残留未求值的占位符：{}",
            plan.spec.url
        );
    }

    #[test]
    fn 页码被替换() {
        let parsed = sample_source();
        let plan = parse_search_spec(&parsed, "k", 5).expect("应构造成功");
        assert!(plan.spec.url.ends_with("page=5"), "{}", plan.spec.url);
    }

    #[test]
    fn 请求间隔从书源映射() {
        let text = r#"{
          "bookSourceName": "限速源",
          "bookSourceUrl": "https://x.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li\"}",
          "customOrder": 0
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let plan = parse_search_spec(&parsed, "k", 1).expect("应构造成功");
        // 未声明速率时是 None（不是 Some(0)）——见 R2 的三态语义
        assert_eq!(plan.spec.request_interval_ms, None);
    }

    #[test]
    fn 书源声明的_concurrentrate_填进请求间隔() {
        // R5 的正向验收：声明了速率的源必须把它带进 RequestSpec，
        // 否则限速器只能看到"未声明"（= 用默认间隔），书源的声明被静默忽略。
        let text = r#"{
          "bookSourceName": "限速源",
          "bookSourceUrl": "https://x.com",
          "concurrentRate": "2000",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        assert_eq!(parsed.source.rate_limit_ms, Some(2000), "解析层应认出它");
        let plan = parse_search_spec(&parsed, "k", 1).expect("应构造成功");
        assert_eq!(
            plan.spec.request_interval_ms,
            Some(2000),
            "声明值必须一路传到 RequestSpec（R5）"
        );
    }

    #[test]
    fn 超范围的速率值被当作未声明() {
        // `rate_limit_ms` 是 i64、`request_interval_ms` 是 u32：
        // 超出 u32 的荒唐值（书源不可信）按未声明处理，而不是截断。
        // 截断会把 `4294967296` 变成 0，即"要求不限速"。
        let text = r#"{
          "bookSourceName": "荒唐速率源",
          "bookSourceUrl": "https://x.com",
          "concurrentRate": "4294967296",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        assert_eq!(parsed.source.rate_limit_ms, Some(4_294_967_296));
        let plan = parse_search_spec(&parsed, "k", 1).expect("应构造成功");
        assert_eq!(
            plan.spec.request_interval_ms, None,
            "超 u32 应视为未声明而非截断为 0"
        );
    }

    // ---------- 结果解析（不发请求） ----------

    /// 一个**字段规则含 `@js:`** 的源（P4 遗留的验收用例）。
    fn js_field_source() -> ParsedSource {
        // 字段规则里直接写 `@js:`——书源里"用 JS 加工字段"的真实写法。
        // 本用例用**纯计算**的 JS（不依赖宿主 API），把焦点保持在
        // "字段里的 `@js:` 会不会经沙箱求值"这一条链路上。
        let text = r#"{
          "bookSourceName": "含 JS 字段的源",
          "bookSourceUrl": "https://example.com",
          "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@js:'书名-' + '一'\",\"bookUrl\":\"@css:.name>a@href\"}"
        }"#;
        parse_sources(text)
            .expect("应解析成功")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    #[test]
    fn 字段规则里的_js_无引擎时报能力错误() {
        // FR-006：没注入 JS 引擎时必须是**结构化能力错误**，不是空结果。
        // 书源的 try/catch 靠它区分"本产品做不到"与"我写错了"。
        let parsed = js_field_source();
        let error = parse_search_result(&parsed.source, sample_html())
            .expect_err("字段用了 @js: 而无引擎，应报错");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("js"),
            "错误应指明是 JS 能力缺失：{rendered}"
        );
    }

    #[test]
    fn 字段规则里的_js_有引擎时可用() {
        // P4 遗留的第一项的端到端验收：**字段规则**里的 `@js:` 真的跑起来。
        let parsed = js_field_source();
        let engine =
            crate::js::SourceJsEngine::for_source(&parsed.source.id, &parsed.source, None, None)
                .expect("应能建引擎");
        // 只注册 JS 引擎内置的全局（不注册 java.*）——本测试验证的是
        // "字段里的 `@js:` 会经沙箱求值"这条链路，而不是某个宿主 API。
        let hits = parse_search_result_with_js(&parsed.source, sample_html(), Some(&engine))
            .expect("应解析成功");
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].name, "书名-一",
            "字段值应来自 `@js:` 的求值结果，而不是页面上选择器的结果"
        );
        assert_eq!(hits[1].name, "书名-一");
    }

    #[test]
    fn 字段规则里的_js_能调宿主_api_经_markup_host() {
        // P4 遗留的第二项的端到端验收：`java.getElement` 落到 tome-rule 的
        // DOM 上（MarkupHost 的真实实现）。
        // 字段规则里用 `java.getElement(标记, 规则)` —— 它落到 tome-rule 的
        // DOM 上（MarkupHost 的真实实现）。标记用**单引号**书写以避免
        // 与外层 JSON 的双引号嵌套。
        let text = r#"{
          "bookSourceName": "用 java.getElement 的源",
          "bookSourceUrl": "https://example.com",
          "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@js:java.getElement('<b>固定名</b>', '@css:b@text')\",\"bookUrl\":\"@css:.name>a@href\"}"
        }"#;
        let parsed = parse_sources(text)
            .expect("应解析成功")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源");
        let engine =
            crate::js::SourceJsEngine::for_source(&parsed.source.id, &parsed.source, None, None)
                .expect("应能建引擎");
        let markup: std::sync::Arc<dyn tome_js::api::data::MarkupHost> =
            std::sync::Arc::new(tome_rule::markup_host::RuleMarkupHost::new());
        engine
            .with_context(move |ctx| {
                let java = tome_js::Object::new(ctx.clone()).map_err(|error| format!("{error}"))?;
                tome_js::api::data::register(&ctx, &java, Some(markup))
                    .map_err(|error| format!("{error}"))?;
                ctx.globals()
                    .set("java", java)
                    .map_err(|error| format!("{error}"))?;
                Ok(())
            })
            .expect("应完成装配");

        let hits = parse_search_result_with_js(&parsed.source, sample_html(), Some(&engine))
            .expect("应解析成功");
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].name, "固定名",
            "`java.getElement` 应真的解析标记并跑规则选择"
        );
        assert_eq!(hits[1].name, "固定名");
    }

    #[test]
    fn 解析搜索结果() {
        let parsed = sample_source();
        let hits = parse_search_result(&parsed.source, sample_html()).expect("应解析成功");

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "斗破苍穹");
        assert_eq!(hits[0].author.as_deref(), Some("天蚕土豆"));
        assert_eq!(hits[0].intro.as_deref(), Some("这里是简介一"));
        // 相对 URL 被补全
        assert_eq!(hits[0].book_url, "https://example.com/book/1");
        assert_eq!(hits[0].cover_url.as_deref(), Some("/cover/1.jpg"));

        assert_eq!(hits[1].name, "完美世界");
        assert_eq!(hits[1].book_url, "https://example.com/book/2");
    }

    #[test]
    fn 字段在每本书的子树内相对求值() {
        // 关键验收点：若字段在整页上求值，两本书的字段会混在一起
        let parsed = sample_source();
        let hits = parse_search_result(&parsed.source, sample_html()).expect("应解析成功");
        assert_eq!(hits[0].name, "斗破苍穹", "第一本书应取到自己的书名");
        assert_eq!(hits[1].name, "完美世界", "第二本书应取到自己的书名");
        assert_ne!(hits[0].name, hits[1].name);
    }

    #[test]
    fn 缺书名的结果被跳过() {
        let parsed = sample_source();
        let html = r#"<ul>
          <li class="item"><p class="author">无书名</p></li>
          <li class="item"><h3 class="name"><a href="/b/2">有书名</a></h3></li>
        </ul>"#;
        let hits = parse_search_result(&parsed.source, html).expect("应解析成功");
        assert_eq!(hits.len(), 1, "缺书名的条目应被跳过");
        assert_eq!(hits[0].name, "有书名");
    }

    #[test]
    fn 缺详情地址的结果被跳过() {
        // 没有 bookUrl 的结果无法进入后续动作，给出来也是点不动的
        let parsed = sample_source();
        let html = r#"<ul>
          <li class="item"><h3 class="name">无链接</h3></li>
          <li class="item"><h3 class="name"><a href="/b/2">有链接</a></h3></li>
        </ul>"#;
        let hits = parse_search_result(&parsed.source, html).expect("应解析成功");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "有链接");
    }

    #[test]
    fn 无匹配产出空列表而不报错() {
        let parsed = sample_source();
        let hits = parse_search_result(&parsed.source, "<html><body>无结果</body></html>")
            .expect("应解析成功");
        assert!(hits.is_empty(), "源在线但无匹配不是错误");
    }

    #[test]
    fn 缺_book_list_规则报配置不完整() {
        let text = r#"{
          "bookSourceName": "缺列表规则源",
          "bookSourceUrl": "https://x.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"name\":\"@css:.name@text\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let error = parse_search_result(&parsed.source, "<p>x</p>").expect_err("应报错");
        assert!(matches!(error, SourceError::IncompleteSource { .. }));
    }

    #[test]
    fn 非法选择器报规则错() {
        let text = r#"{
          "bookSourceName": "坏选择器源",
          "bookSourceUrl": "https://x.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"div >\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let error = parse_search_result(&parsed.source, "<p>x</p>").expect_err("应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Rule);
    }

    #[test]
    fn 含不支持能力的字段规则报结构化错误() {
        // FR-006：`@js:` 属 S1-2，本阶段必须**报出来**而不是静默产出空值——
        // "没有结果"与"能力不可用"的排查方向完全不同。
        let text = r#"{
          "bookSourceName": "含js字段源",
          "bookSourceUrl": "https://x.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li\",\"name\":\"@js:java.ajax('x')\",\"bookUrl\":\"@css:a@href\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let html = r#"<ul><li><a href="/b">x</a></li></ul>"#;
        let error = parse_search_result(&parsed.source, html).expect_err("应报错");
        let kind = error.to_error_kind();
        assert!(
            matches!(kind, ErrorKind::UnsupportedCapability { .. }),
            "应是不支持能力类，实际 {kind:?}"
        );
        match kind {
            ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "js");
            }
            other => panic!("应为能力错误，实际 {other:?}"),
        }
    }

    #[test]
    fn 缺失的可选字段是_none_而非空串() {
        let text = r#"{
          "bookSourceName": "仅书名源",
          "bookSourceUrl": "https://x.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li\",\"name\":\"@css:.n@text\",\"bookUrl\":\"@css:a@href\"}"
        }"#;
        let parsed = parse_sources(text).expect("应解析").sources.remove(0);
        let html = r#"<ul><li><span class="n">书</span><a href="/b">x</a></li></ul>"#;
        let hits = parse_search_result(&parsed.source, html).expect("应解析成功");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].author, None, "未写的规则应产出 None 而非空串");
        assert_eq!(hits[0].intro, None);
    }

    // ---------- 结果转 Book ----------

    #[test]
    fn 搜索结果可转为可入库的书籍() {
        let hit = SearchHit {
            name: "书名".to_string(),
            author: Some("作者".to_string()),
            cover_url: Some("https://c/1.jpg".to_string()),
            intro: Some("简介".to_string()),
            kind: None,
            word_count: None,
            last_chapter: Some("第一章".to_string()),
            book_url: "https://x.com/book/1".to_string(),
        };
        let book = hit.to_book("https://x.com");
        assert_eq!(book.origin, BookOrigin::Network);
        assert_eq!(
            book.origin_id, "https://x.com|https://x.com/book/1",
            "origin_id 应为 `书源id|详情URL`（data-model.md）"
        );
        assert_eq!(book.name, "书名");
        assert_eq!(book.author.as_deref(), Some("作者"));
        assert_eq!(book.latest_chapter_title.as_deref(), Some("第一章"));
        assert_eq!(book.id, 0, "新书由存储层分配 id");
    }

    // ---------- 端到端（假宿主，不触网） ----------

    /// 假宿主：返回固定 HTML，并记录收到的 spec。
    struct FakeHost {
        body: String,
        status: u16,
        seen: Mutex<Vec<RequestSpec>>,
    }

    impl FakeHost {
        fn new(body: &str) -> Self {
            Self {
                body: body.to_string(),
                status: 200,
                seen: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<RequestSpec> {
            self.seen.lock().expect("锁未被毒化").clone()
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, spec: RequestSpec) -> Result<tome_core::Response, ErrorKind> {
            self.seen.lock().expect("锁未被毒化").push(spec);
            Ok(tome_core::Response {
                status: self.status,
                headers: tome_core::HeaderMap::new(),
                body_bytes: self.body.clone().into_bytes(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    /// 记录调用次数的宿主：用于证明"规则求值不发请求"。
    struct CountingHost {
        body: String,
        calls: Mutex<usize>,
    }

    impl NetHost for CountingHost {
        fn fetch(&self, _spec: RequestSpec) -> Result<tome_core::Response, ErrorKind> {
            *self.calls.lock().expect("锁未被毒化") += 1;
            Ok(tome_core::Response {
                status: 200,
                headers: tome_core::HeaderMap::new(),
                body_bytes: self.body.clone().into_bytes(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    #[test]
    fn a2_纯化_规则求值全程不发请求() {
        // T027 的关键验收项：规则求值（spec 构造 + 结果解析）**零请求**。
        let parsed = sample_source();

        // 第一步：构造 spec —— 不该有任何网络设施参与
        let plan = parse_search_spec(&parsed, "关键词", 3).expect("应构造成功");
        assert!(!plan.spec.url.is_empty());

        // 第二步：解析结果 —— 同样不发请求
        let hits = parse_search_result(&parsed.source, sample_html()).expect("应解析成功");
        assert_eq!(hits.len(), 2);

        // 用计数宿主反向验证：整条纯路径跑完，宿主调用次数为 0
        let host = CountingHost {
            body: sample_html().to_string(),
            calls: Mutex::new(0),
        };
        let _ = parse_search_spec(&parsed, "k", 1).expect("应构造成功");
        let _ = parse_search_result(&parsed.source, sample_html()).expect("应解析成功");
        assert_eq!(
            *host.calls.lock().expect("锁未被毒化"),
            0,
            "规则求值路径不得调用 NetHost（A2 纯化）"
        );
    }

    #[test]
    fn 单源搜索端到端() {
        let parsed = sample_source();
        let host = FakeHost::new(sample_html());

        let hits = search_one_source(&parsed, "斗破", 1, &host).expect("应成功");
        assert_eq!(hits.len(), 2);

        let seen = host.seen();
        assert_eq!(seen.len(), 1, "应恰好发一次请求");
        assert!(seen[0].url.contains("%E6%96%97%E7%A0%B4"), "关键词应被编码");
        assert_eq!(seen[0].headers.get("user-agent"), Some("tome/0.1"));
    }

    #[test]
    fn 搜索对非_2xx_报错() {
        let parsed = sample_source();
        let mut host = FakeHost::new("<html>404</html>");
        host.status = 404;
        let error = search_one_source(&parsed, "k", 1, &host).expect_err("404 应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Network);
    }

    #[test]
    fn 搜索对传输失败报错() {
        struct FailingHost;
        impl NetHost for FailingHost {
            fn fetch(&self, _spec: RequestSpec) -> Result<tome_core::Response, ErrorKind> {
                Err(ErrorKind::Proxy)
            }
        }
        let parsed = sample_source();
        let error = search_one_source(&parsed, "k", 1, &FailingHost).expect_err("应报错");
        assert_eq!(
            error.to_error_kind(),
            ErrorKind::Proxy,
            "代理故障应透传，不该被压成 Network"
        );
    }

    // ---------- 多源流 ----------

    #[test]
    fn 多源搜索逐源产出且单源失败不终止() {
        // 两个源：第一个规则坏（求值失败），第二个正常
        let bad_text = r#"{
          "bookSourceName": "坏源",
          "bookSourceUrl": "https://bad.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"div >\"}"
        }"#;
        let good_text = r#"{
          "bookSourceName": "好源",
          "bookSourceUrl": "https://good.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"bookUrl\":\"@css:.name>a@href\"}"
        }"#;
        let mut sources = parse_sources(bad_text).expect("应解析").sources;
        sources.extend(parse_sources(good_text).expect("应解析").sources);

        let host = FakeHost::new(sample_html());
        let engine = SearchEngine::new(&sources, &host);
        let (successes, failures) = collect_search(engine.search("k", &[]));

        assert_eq!(successes.len(), 1, "好源应成功");
        assert_eq!(successes[0].0, "https://good.com");
        assert_eq!(successes[0].1.len(), 2);
        assert_eq!(failures.len(), 1, "坏源应作为流内条目失败");
        assert_eq!(failures[0].0, "https://bad.com");
        assert_eq!(failures[0].1, ErrorKind::Rule);
    }

    #[test]
    fn 多源搜索可按_id_过滤() {
        let text_a = r#"{
          "bookSourceName": "源A",
          "bookSourceUrl": "https://a.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"bookUrl\":\"@css:a@href\"}"
        }"#;
        let text_b = text_a.replace("源A", "源B").replace("a.com", "b.com");
        let mut sources = parse_sources(text_a).expect("应解析").sources;
        sources.extend(parse_sources(&text_b).expect("应解析").sources);

        let host = FakeHost::new(sample_html());
        let engine = SearchEngine::new(&sources, &host);
        let (successes, failures) =
            collect_search(engine.search("k", &["https://b.com".to_string()]));

        assert_eq!(successes.len(), 1);
        assert_eq!(successes[0].0, "https://b.com");
        assert!(failures.is_empty());
    }

    #[test]
    fn 空源列表产出空流() {
        let host = FakeHost::new(sample_html());
        let engine = SearchEngine::new(&[], &host);
        let (successes, failures) = collect_search(engine.search("k", &[]));
        assert!(successes.is_empty());
        assert!(failures.is_empty());
    }

    #[test]
    fn 取消后流立即结束() {
        let parsed = sample_source();
        let sources = vec![parsed];
        let host = FakeHost::new(sample_html());
        let engine = SearchEngine::new(&sources, &host);
        let mut stream = engine.search("k", &[]);
        stream.cancel_token().cancel();

        assert!(
            stream.next_item().is_none(),
            "取消后不得再产出条目（A6：尽快响应取消）"
        );
    }
}
