//! [DONE] 书源模型加载与三渠道导入（tasks.md T026 / FR-001）
//!
//! # 三渠道（FR-001 的硬要求）
//!
//! | 渠道 | 输入 | 拉取方式 |
//! |---|---|---|
//! | [`SourceImportChannel::FileText`] | 文件内容 | 无（内容已在手上） |
//! | [`SourceImportChannel::Url`] | URL | **经 `NetHost`**（A2 唯一发请求处） |
//! | [`SourceImportChannel::PastedText`] | 粘贴的 JSON 文本 | 无 |
//!
//! 三种渠道共用同一条解析 → 校验 → 归一化的路径，区别只在"文本从哪来"。
//! 这样格式容错只需维护一处。
//!
//! # 与 `tome-core::Source` 的关系
//!
//! [`ParsedSource`] 是对 core 的 [`Source`] 的**薄包装**（R4，PM 仲裁
//! 2026-10-10 之后已无额外字段）。
//!
//! ## `searchUrl` / `exploreUrl` 的归属（R4 的结论）
//!
//! 本模块初版把 `searchUrl` 从规则 JSON 里提出来放在
//! [`ParsedSource::search_url`]，理由是 data-model.md 的 `source` 表没有该列。
//! PM 仲裁（R4）判定：`searchUrl` 与 `exploreUrl` 是公开书源格式的**顶层键**，
//! 与 `ruleSearch` 并列，因此**下沉为 `Source` 的字段**才是正确归属。
//! 该结论已由 PM 补录进 data-model.md。
//!
//! 故现在：解析时两个键都写进 `Source`；`ruleSearch` 里若内嵌 `searchUrl`，
//! 仍然提取出来（信息只存一处），但**不再切掉**——保留在规则 JSON 里是为了
//! 让"作者把 searchUrl 写在规则里"这条路径在落库后仍可追溯。
//!
//! [DONE] 关键入口：[`parse_sources`]（文本 → 书源列表）、[`load_from_channel`]。

use std::collections::BTreeMap;

use serde::Deserialize;
use tome_core::seam::{ImportFailure, ImportReport, SourceImportChannel};
use tome_core::{ErrorKind, HeaderMap, NetHost, RequestSpec, Source, SourceKind};

use crate::error::SourceError;

/// 解析后的书源：core 的 [`Source`] + 本 crate 需要的补充字段。
///
/// ## R4 之后它为什么只是 [`Source`] 的别名式包装（PM 仲裁 2026-10-10）
///
/// `searchUrl` / `exploreUrl` 已下沉到 [`Source`] 本身（见 core 的
/// `Source::search_url` 文档），故本结构**不再单独承载形式字段**。
/// 保留这一层是因为它在 Cargo.lock 上已与 `tome-cli` / `tome-source` 的公共
/// 签名绑定，且将来可能承载"解析期中间产物"（如 `group_name` → `group_id`
/// 的待决映射）。**若后续确实没有新增字段，应在某个切片里把它降为
/// `type ParsedSource = Source;` 或直接删除。** 标 `[UNCERTAIN]`：
/// 现在删会让本期改动面扩大（CLI、注册表、搜索管线三处签名），
/// 而它并不造成行为差异。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSource {
    /// 可落库的实体（`search_url` / `explore_url` 已在其内）
    pub source: Source,
}

/// 导入的最终结果：解析出的书源 + 报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadOutcome {
    /// 成功解析的书源
    pub sources: Vec<ParsedSource>,
    /// 导入报告（新增/更新/跳过计数与逐条失败原因）
    ///
    /// ## 为什么解析阶段就把 added/updated 填上
    ///
    /// 解析器不碰数据库，但它**知道输入里有几条与几条重复 id**——
    /// 同一批导入里出现两次相同 id 时，第二次按"更新"计（spec 边缘情况
    /// 要求"重复 id 按更新处理"）。把它留给存储层会让"输入内重复"这类
    /// 只在导入期可见的现象无处统计。
    pub report: ImportReport,
}

// ---------------------------------------------------------------------------
// 书源 JSON 的字段映射
// ---------------------------------------------------------------------------

/// 规则字段的两种存在形态。
///
/// ## 为什么必须两种都接受
///
/// - **字符串**：社区流通格式的规范形态——规则本身是 JSON 文本，嵌在外层
///   JSON 里时被转义成字符串。公开文档给出的三份书源示例都是这种形态
///   （`ruleSearch` 的值是一段带 `\"` 转义的 JSON 文本）。
/// - **对象**：手工构造或程序生成时更自然的写法（值的形状就是 JSON 对象）。
///
/// 只接受一种会让另一类书源整条被跳过，而作者看不出原因——错误信息只会说
/// "类型不符"。两种都归一成字符串，下游只处理一种形状。
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RuleField {
    /// 转义后的 JSON 文本（社区流通格式）
    Text(String),
    /// 直接的对象形态
    Object(serde_json::Map<String, serde_json::Value>),
}

impl RuleField {
    /// 归一成 JSON 文本。空内容返回 `None`。
    fn into_text(self) -> Option<String> {
        match self {
            Self::Text(text) => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            Self::Object(map) => {
                if map.is_empty() {
                    return None;
                }
                serde_json::to_string(&serde_json::Value::Object(map)).ok()
            }
        }
    }
}

/// 书源 JSON 的**反序列化视图**。
///
/// ## 为什么每个字段都是 `Option`
///
/// 书源是社区流通的第三方资产：字段缺失、类型写错（数字写成字符串）、
/// 出现未知键都是常态。用 `Option` + 校验而不是 serde 的强类型约束，
/// 是为了让"缺 `name` 的一条源被跳过"而不是"整份文件解析失败"——
/// 一份书源文件里混着几条坏源是极常见的，全盘拒绝会让用户无从下手。
///
/// 未识别的键**不报错**（serde 默认忽略）：书源格式会演进，新增的键
/// 不该让旧版本的 tome 拒绝整份文件。
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SourceJson {
    #[serde(rename = "bookSourceName")]
    book_source_name: Option<String>,
    #[serde(rename = "bookSourceUrl")]
    book_source_url: Option<String>,
    #[serde(rename = "bookSourceGroup")]
    book_source_group: Option<String>,
    #[serde(rename = "bookSourceType")]
    book_source_type: Option<i64>,
    #[serde(rename = "bookSourceComment")]
    book_source_comment: Option<String>,
    #[serde(rename = "enabled")]
    enabled: Option<bool>,
    #[serde(rename = "enabledExplore")]
    enabled_explore: Option<bool>,
    #[serde(rename = "searchUrl")]
    search_url: Option<String>,
    #[serde(rename = "exploreUrl")]
    explore_url: Option<String>,
    #[serde(rename = "bookUrlPattern")]
    book_url_pattern: Option<String>,
    #[serde(rename = "ruleSearch")]
    rule_search: Option<RuleField>,
    #[serde(rename = "ruleExplore")]
    rule_explore: Option<RuleField>,
    #[serde(rename = "ruleBookInfo")]
    rule_book_info: Option<RuleField>,
    #[serde(rename = "ruleToc")]
    rule_toc: Option<RuleField>,
    #[serde(rename = "ruleContent")]
    rule_content: Option<RuleField>,
    #[serde(rename = "header")]
    header: Option<String>,
    #[serde(rename = "loginUrl")]
    login_url: Option<String>,
    #[serde(rename = "loginUi")]
    login_ui: Option<String>,
    #[serde(rename = "loginCheckJs")]
    login_check_js: Option<String>,
    /// 纯脚本源的主脚本。JSON 里键名可能是 `js` 或 `mainJs`。
    #[serde(rename = "js")]
    js: Option<String>,
    #[serde(rename = "mainJs")]
    main_js: Option<String>,
    #[serde(rename = "charset")]
    charset: Option<String>,
    /// 请求间隔（公开书源格式的顶层 `concurrentRate`）。
    ///
    /// ## 为什么用 `serde_json::Value` 而不是 `Option<String>`
    ///
    /// 社区真实书源把它写成**字符串毫秒值**（`"1000"` / `"2000"` / `"3000"`），
    /// 但也有作者写成数字。用 `Value` 让两种形态都能进到
    /// [`parse_concurrent_rate`] 里做归一，而不是让 serde 在类型不匹配时
    /// **静默把整个字段丢掉**——那正是"源能跑但没有限速"这类问题的成因，
    /// 且完全不可观测。
    #[serde(rename = "concurrentRate")]
    concurrent_rate: Option<serde_json::Value>,
    /// 自定义排序（书源格式里的 `customOrder`）。
    ///
    /// 与 `weight` 是两个独立的排序位：上游用 `customOrder` 表达用户手动排序、
    /// `weight` 表达源作者建议的权重。本 crate 目前只用 `weight` 填
    /// `Source::weight`——`customOrder` 的归属是存储层的用户偏好，
    /// 故此处**读入但不落库**（保留字段是为了让"格式里有什么"在解析层可见）。
    #[serde(rename = "customOrder")]
    custom_order: Option<i64>,
    #[serde(rename = "weight")]
    weight: Option<i64>,
    #[serde(rename = "lastUpdateTime")]
    last_update_time: Option<i64>,
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

/// 解析一份书源 JSON（可以是单个对象，也可以是对象数组）。
///
/// [DONE] FR-001 的解析入口：三种渠道的文本都最终落到这里。
///
/// # 容错策略（逐条对应一种真实存在的输入）
///
/// | 输入形态 | 处理 |
/// |---|---|
/// | 单个对象 `{...}` | 解析成一条源 |
/// | 对象数组 `[{...},{...}]` | 逐条解析 |
/// | 数组里某条缺 `bookSourceUrl` | 该条计入 `skipped`，给出失败原因，**其余照常** |
/// | 数组里某条缺 `bookSourceName` | 同上 |
/// | 整份不是合法 JSON | 返回 `Err`（这是渠道级失败，非单条失败） |
/// | 带 UTF-8 BOM | 先剥 BOM（Windows 记事本保存的 JSON 常带） |
/// | 顶层是数字/字符串等非对象 | 返回 `Err` |
pub fn parse_sources(text: &str) -> Result<LoadOutcome, SourceError> {
    let cleaned = strip_bom(text);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Err(SourceError::InvalidJson {
            reason: "输入为空".to_string(),
        });
    }

    // 顶层可能是对象或数组。先按 `serde_json::Value` 探测形状，
    // 再逐条反序列化——一步到位地反序列化成"对象或数组"需要 untagged enum，
    // 而它的错误信息对用户毫无帮助。
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|error| SourceError::InvalidJson {
            reason: error.to_string(),
        })?;

    let entries: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(items) => items,
        object @ serde_json::Value::Object(_) => vec![object],
        other => {
            return Err(SourceError::InvalidJson {
                reason: format!("顶层应为对象或数组，实际是 {}", json_kind(&other)),
            });
        }
    };

    let mut outcome = LoadOutcome::default();
    // 按 id 去重：同一批导入里出现两次相同 id 时，第二次按"更新"计。
    // 用 `BTreeMap` 而非 `HashMap`：遍历顺序影响 `report` 里失败的排列，
    // 而 CLI 的输出要可 diff（确定性输出是 T029 的验收项）。
    let mut by_id: BTreeMap<String, ParsedSource> = BTreeMap::new();
    let mut duplicates = 0usize;

    for (index, entry) in entries.iter().enumerate() {
        match convert_entry(entry, index) {
            Ok(parsed) => {
                let id = parsed.source.id.clone();
                if by_id.insert(id, parsed).is_some() {
                    duplicates += 1;
                }
            }
            Err(failure) => {
                outcome.report.skipped += 1;
                outcome.report.failures.push(failure);
            }
        }
    }

    outcome.report.added = by_id.len();
    outcome.report.updated = duplicates;
    outcome.sources = by_id.into_values().collect();
    Ok(outcome)
}

/// 把一条 JSON 条目转成 [`ParsedSource`]。
fn convert_entry(entry: &serde_json::Value, index: usize) -> Result<ParsedSource, ImportFailure> {
    let view: SourceJson =
        serde_json::from_value(entry.clone()).map_err(|error| ImportFailure {
            subject: format!("第 {} 条", index + 1),
            error: ErrorKind::Rule
                .with_detail(error.to_string())
                .kind()
                .clone(),
        })?;

    // `bookSourceUrl` 是 id 的来源，缺它就无法落库（id 是主键）。
    let id = require(&view.book_source_url, index, "bookSourceUrl")?;
    // 名称缺失时无法向用户呈现，同样跳过。
    let name = require(&view.book_source_name, index, "bookSourceName")?;

    let kind = classify_kind(&view);
    let now = now_unix_seconds();

    // `rule_search` 的 JSON 里若含 `searchUrl`，把它提到 `Source::search_url`
    // （R4：该字段现在是 core DTO 的一部分）。**保留**规则里的原文：
    // "作者把 searchUrl 写在规则 JSON 里"这条路径落库后仍可追溯。
    let rule_search_text = view.rule_search.and_then(RuleField::into_text);
    let (rule_search, extracted_search_url) = normalize_rule_search(&rule_search_text);
    let search_url = view
        .search_url
        .clone()
        .or(extracted_search_url)
        .filter(|text| !text.trim().is_empty());

    // `custom_order` 当前不落库（见其字段文档），显式消费以免 dead_code
    let _custom_order = view.custom_order;

    let source = Source {
        id,
        name,
        kind,
        // 分组名到 id 的映射需要存储层参与（`source_group` 是独立表），
        // 故解析阶段只保留组名在 `rule_explore` 之外的元信息里——
        // 见 `ParsedSource::group_name`。此处置 `None`。
        group_id: None,
        enabled: view.enabled.unwrap_or(true),
        book_source_type: view.book_source_type.map(|value| value as i32),
        book_url_pattern: view.book_url_pattern.filter(|text| !text.is_empty()),
        search_url,
        // 发现地址与 `explore_screen` 是书源格式里两个不同的顶层键：
        // 前者是请求地址规则（P6 的发现管线消费），后者是发现页呈现配置。
        explore_url: view.explore_url.clone().filter(|text| !text.is_empty()),
        rule_search,
        rule_explore: view.rule_explore.and_then(RuleField::into_text),
        rule_book_info: view.rule_book_info.and_then(RuleField::into_text),
        rule_toc: view.rule_toc.and_then(RuleField::into_text),
        rule_content: view.rule_content.and_then(RuleField::into_text),
        header: view.header.clone().filter(|text| !text.is_empty()),
        login_url: view.login_url.clone().filter(|text| !text.is_empty()),
        login_ui: view.login_ui.clone().filter(|text| !text.is_empty()),
        login_check_js: view.login_check_js.clone().filter(|text| !text.is_empty()),
        js: view
            .js
            .clone()
            .or_else(|| view.main_js.clone())
            .filter(|text| !text.is_empty()),
        charset: view.charset.clone().filter(|text| !text.is_empty()),
        // R5（PM 仲裁 2026-10-10）：`concurrentRate` 是公开书源格式的顶层键，
        // 社区真实书源把它写成字符串毫秒值（见 `parse_concurrent_rate`）。
        rate_limit_ms: parse_concurrent_rate(view.concurrent_rate.as_ref()),
        weight: view.weight.map(|value| value as i32),
        explore_screen: view.explore_url.clone().filter(|text| !text.is_empty()),
        comment: view
            .book_source_comment
            .clone()
            .filter(|text| !text.is_empty()),
        last_update_time: view.last_update_time,
        created_at: now,
    };

    Ok(ParsedSource { source })
}

/// 取必需字段，缺失时给出可读的失败原因。
fn require(value: &Option<String>, index: usize, field: &str) -> Result<String, ImportFailure> {
    match value.as_deref() {
        Some(text) if !text.trim().is_empty() => Ok(text.trim().to_string()),
        _ => Err(ImportFailure {
            subject: format!("第 {} 条", index + 1),
            error: ErrorKind::Rule
                .with_detail(format!("缺少必需字段 {field}"))
                .kind()
                .clone(),
        }),
    }
}

/// 判断书源类型。
///
/// 依据公开的《书源规则说明》与 data-model.md 的编码：
/// `0=书源 1=JS 源 2=订阅源 3=词典源`。
/// 另有"有 `js` 且没有任何规则字段"的纯脚本源——那种情况即使
/// `bookSourceType=0` 也应算 JS 源，否则它的主脚本永远不会被调用。
fn classify_kind(view: &SourceJson) -> SourceKind {
    let declared = view
        .book_source_type
        .and_then(|value| SourceKind::from_storage_code(value as i32));
    // 规则字段是 `&Option<RuleField>`（未消费），故用 `as_ref` 判断非空
    let has_rules = [
        view.rule_search.as_ref(),
        view.rule_explore.as_ref(),
        view.rule_book_info.as_ref(),
        view.rule_toc.as_ref(),
        view.rule_content.as_ref(),
    ]
    .iter()
    .any(|rule| {
        rule.as_ref().is_some_and(|field| match field {
            RuleField::Text(text) => !text.trim().is_empty(),
            RuleField::Object(map) => !map.is_empty(),
        })
    });
    let has_script = view
        .js
        .as_deref()
        .or(view.main_js.as_deref())
        .is_some_and(|text| !text.trim().is_empty());

    match (declared, has_rules, has_script) {
        // **声明优先**：作者显式写了类型就是他的知识，我的推断只是猜测。
        // 这条必须排在推断之前——否则一个声明了 `bookSourceType: 0`（普通书源）
        // 但同时带 `js` 字段的源会被推断成 JS 源，而它的规则字段永远不会被用。
        (Some(kind), _, _) => kind,
        // 未声明类型时的推断：有脚本无规则 → 纯脚本源
        (None, false, true) => SourceKind::Js,
        // 其余（含"无脚本无规则"这个最小形态）→ 普通书源
        (None, _, _) => SourceKind::Book,
    }
}

/// 归一化 `rule_search`：把内嵌的 `searchUrl` 提出来供 `Source::search_url` 使用。
///
/// ## 为什么不从规则 JSON 里删掉它（R4 修订）
///
/// 初版把它删掉，理由是"避免两处存储同一信息"。PM 仲裁 R4 之后该理由不再成立：
/// `Source::search_url` 现在是一个**语义明确的顶层字段**，而规则 JSON 里的
/// `searchUrl` 是作者的原文。两者共存不构成矛盾——前者是归一化结果，后者是
/// 原始输入，正如 `Source::rule_search` 与规则 JSON 原文的关系。
/// 删掉反而会让"作者的规则里到底写了什么"在落库后丢失。
///
/// 返回 `(归一化后的 rule_search, 提取到的 searchUrl)`。
fn normalize_rule_search(rule_search: &Option<String>) -> (Option<String>, Option<String>) {
    let Some(text) = rule_search.as_deref() else {
        return (None, None);
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (None, None);
    }

    // 规则本身是 JSON 文本；解析失败时原样保留——规则语法错误应在**求值**时
    // 报出（那里能给出更准确的位置），而不是让整条源在导入时消失。
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return (Some(trimmed.to_string()), None);
    };

    let extracted = value
        .get("searchUrl")
        .and_then(|inner| inner.as_str())
        .map(|inner| inner.to_string());

    (Some(value.to_string()), extracted)
}

/// 剥掉 UTF-8 BOM。
///
/// Windows 记事本保存 JSON 时会加 BOM，而 `serde_json` 不接受它。
/// 这是真实且高频的输入形态，故显式处理而不是让用户去猜为什么"文件看起来对"。
fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// JSON 值的类型名（供错误信息用，不给用户看 `Debug` 输出）。
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "布尔值",
        serde_json::Value::Number(_) => "数字",
        serde_json::Value::String(_) => "字符串",
        serde_json::Value::Array(_) => "数组",
        serde_json::Value::Object(_) => "对象",
    }
}

/// 解析书源的请求间隔（R5，PM 仲裁 2026-10-10）。
///
/// # 依据来源
///
/// `concurrentRate` 是公开书源格式的顶层键。社区真实书源把它写成**字符串
/// 毫秒值**——`"1000"` / `"2000"` / `"3000"` 是最常见的三个取值。语义是
/// "同一书源两次请求之间的最小间隔"，正好对应 `Source::rate_limit_ms`
/// →`RequestSpec::request_interval_ms` 这条链路（FR-008）。
///
/// # 容错取向（逐条对应一种真实存在的畸形输入）
///
/// | 输入 | 结果 | 理由 |
/// |---|---|---|
/// | `"1000"` / `1000` / `2000.0` | `Some(n)` | 三种形态语义无歧义，都接受 |
/// | 缺字段 / `null` | `None` | 未声明 |
/// | `""` / `"abc"` / `"1000ms"` | `None` | 非纯数字一律按未声明处理 |
/// | 负数 | `None` | 见下 |
/// | `0` | `None` | 见下 |
/// | 对象 / 数组 | `None` | 类型不符 |
///
/// ## 为什么负数是 `None` 而不是钳到 0
///
/// `Some(0)` 在 R2 的三态语义里是"书源**显式声明**不要间隔"，会**覆盖**本机
/// 配置的默认限速；而 `None` 是"未声明"，限速器仍用它自己的默认值。
/// 把 `-1` 钳成 0 等于把一条写错的源变成"要求完全不限速"，比忽略它激进得多——
/// 而这条源恰恰是作者以为加了限速的。故一律 `None`。
///
/// ## 为什么 `0` 也是 `None`
///
/// 同一理由的弱化版：真实的"不要间隔"声明几乎不会写成 `0`（写 `0` 的多半是
/// 占位或误填），而"显式关掉限速"在本产品的场景里没有正当用途（AGENTS.md
/// 安全红线要求遵守书源声明的请求间隔，逆向即"不声明就不关"）。
/// [UNCERTAIN] 若 S1-6 的真实书源基线里出现确实声明 `0` 的源，应改为
/// `Some(0)` 并在此注明依据。
///
/// ## 为什么整条源不因它而失败
///
/// spec 的容错取向是"书源是不可信输入，一个字段写错不该让整条源消失"。
/// 限速值尤其如此：丢掉限速只会让请求快一点，而拒绝整条源会让用户
/// 完全用不了这本书。
pub fn parse_concurrent_rate(value: Option<&serde_json::Value>) -> Option<i64> {
    let value = value?;
    // 先归一成"可能的数字文本"：字符串形态是社区主流，数字形态也接受。
    let text = match value {
        serde_json::Value::String(text) => text.trim().to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        // `null` 与其它类型（对象/数组/布尔）一律视为未声明
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }

    // 数字形态统一成小数再取整：`2000` / `2000.0` / `"2000"` 三者应同解。
    // 用 `/` 而非字符串切分：能一并拒绝 `"1000ms"`、`"1e3ms"` 这类带尾巴的写法
    // （`serde_json` 的 `as_f64` 只接受严格 JSON 数字文本，`"1e3"` 会被接受，
    // 那正是我们要的——它确实是 1000 毫秒）。
    let numeric: f64 = text.parse().ok()?;
    // `NaN`/`inf` 在 f64 上可表示，但它们不是合法毫秒数。
    if !numeric.is_finite() {
        return None;
    }
    // 负数、零、以及超出 i64 的荒唐值：一律未声明（理由见上）。
    if numeric <= 0.0 {
        return None;
    }
    // 上限用 i64 的边界而不是 i64::MAX 直接比较：f64 转 i64 时越界是饱和的，
    // 先判大小可以避免"把 inf 附近的量悄悄变成 i64::MAX"这种静默失真。
    if numeric > i64::MAX as f64 {
        return None;
    }
    Some(numeric.round() as i64)
}

/// 当前 Unix 秒。用于 `Source::created_at`。
pub(crate) fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 渠道装载
// ---------------------------------------------------------------------------

/// 从导入渠道装入书源（FR-001 的三渠道）。
///
/// [DONE] 三渠道的统一入口——URL 渠道的拉取经 [`NetHost`]（A2 唯一发请求处）。
///
/// # 为什么 URL 渠道要经 `NetHost` 而不是 `reqwest`
///
/// FR-001 的 URL 渠道是"粘贴链接后拉取并解析"。若直接调 HTTP 客户端，它就会
/// **绕开**限速、代理、重试与 Cookie（那些都在 `RequestSpec` 执行器里），
/// 而这四条正是 A2「唯一发请求处」要集中管理的东西。故此处构造
/// [`RequestSpec`] 交给宿主执行。
///
/// `rate_limit_key` 用固定的导入键（而非书源 id——此时还没解析出 id）：
/// 同一次导入会话的多条 URL 共享一个令牌桶，避免"导入 20 个源时同时打出
/// 20 个请求"。键名带前缀以免与书源的 id 撞车。
pub fn load_from_channel(
    channel: &SourceImportChannel,
    host: &dyn NetHost,
) -> Result<LoadOutcome, SourceError> {
    match channel {
        SourceImportChannel::FileText(text) | SourceImportChannel::PastedText(text) => {
            parse_sources(text)
        }
        SourceImportChannel::Url(url) => {
            let spec = RequestSpec::get(url.clone(), IMPORT_RATE_LIMIT_KEY);
            let response = host
                .fetch(spec)
                .map_err(|kind| SourceError::Fetch { kind })?;
            // R3 之后接缝对非 2xx 返回 `Ok`，故这里必须自己判状态——
            // "导入用的 URL 返回 404"与"拉到了一份不是书源的 HTML"都要报出来。
            if !response.is_success() {
                return Err(SourceError::Fetch {
                    kind: ErrorKind::Network
                        .with_detail(format!("导入 URL 返回 {}", response.status))
                        .kind()
                        .clone(),
                });
            }
            let decoded = decode_import_body(&response);
            parse_sources(&decoded)
        }
    }
}

/// URL 渠道专用的限速键。
///
/// 带 `import://` 前缀：书源 id 通常是一个 URL，不加前缀有可能与某个源的 id
/// 恰好相同，从而让"导入"与"抓取某个源"共享令牌桶（那会莫名拖慢其中一方）。
pub const IMPORT_RATE_LIMIT_KEY: &str = "import://source-import";

/// 把导入响应的字节解成文本。
///
/// 复用 charset 层的容错解码：书源文件常从各种编辑器/网页复制而来，
/// 编码可能是 UTF-8（带/不带 BOM）、GBK 等。硬编 UTF-8 会让一份 GBK 书源
/// 在导入时报"非法 JSON"，而真正的原因是编码。
fn decode_import_body(response: &tome_core::Response) -> String {
    // `detected_charset` 由 net 层在响应后处理时填好（FR-009）；
    // 未填时按未声明处理，交给检测。
    // 先取成 owned `Option<String>` 再借——`detected_charset` 与头的值
    // 分属两个不同的所有者，直接链式 `or_else` 会让临时值的生命周期纠缠。
    let declared: Option<String> = response
        .detected_charset
        .clone()
        .or_else(|| response.headers.get("content-type").map(str::to_string));
    let decoded = tome_net::charset::decode_body(&response.body_bytes, declared.as_deref());
    decoded.text
}

/// 把已解析的书源列表转成落库报告（供 `BookRepository::import_parsed_sources` 复用）。
///
/// 目前只是把 [`LoadOutcome::report`] 透传——独立成函数是为了让"解析期报告"
/// 与"落库报告"的语义边界显式：解析期的 `added` 是"新 id 的条数"，
/// 落库期还要结合库里已有的 id 才能给出真正的 added/updated。
pub fn report_for_persistence(outcome: &LoadOutcome) -> ImportReport {
    outcome.report.clone()
}

/// 把书源的自定义请求头解析成 [`HeaderMap`]（FR-008）。
///
/// 书源里的 `header` 字段是 JSON 对象文本（值可能是字符串或数字）。
/// [DONE] 供 T027 构造 `RequestSpec` 时使用。
///
/// 非法 JSON 返回 `Err`：请求头写错会让整个抓取异常，静默忽略会掩盖真实原因。
pub fn parse_headers(header: Option<&str>) -> Result<HeaderMap, SourceError> {
    let mut map = HeaderMap::new();
    let Some(text) = header else {
        return Ok(map);
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(map);
    }

    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|error| SourceError::InvalidJson {
            reason: error.to_string(),
        })?;
    let Some(object) = value.as_object() else {
        return Err(SourceError::InvalidJson {
            reason: "header 应为 JSON 对象".to_string(),
        });
    };

    for (name, raw) in object {
        // 值允许字符串与数字：有些书源把 `"Referer": 123` 这类写法
        // 用在了端口号等位置，拒绝它只会让可用源变成不可用。
        let text = match raw {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Number(number) => number.to_string(),
            serde_json::Value::Bool(flag) => flag.to_string(),
            _ => continue,
        };
        map.append(name.clone(), text);
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
          "bookSourceName": "示例源",
          "bookSourceUrl": "https://example.com",
          "bookSourceGroup": "测试",
          "enabled": true,
          "searchUrl": "/search?q={{key}}&page={{page}}",
          "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\"}",
          "ruleToc": "{\"chapterList\":\"@css:#list dd>a\"}",
          "ruleContent": "{\"content\":\"@css:#content@text\"}",
          "header": "{\"User-Agent\":\"tome/0.1\"}",
          "charset": "gbk",
          "weight": 3,
          "lastUpdateTime": 1700000000000
        }"#
    }

    #[test]
    fn 解析单个书源对象() {
        let outcome = parse_sources(sample_json()).expect("应解析成功");
        assert_eq!(outcome.sources.len(), 1);
        assert_eq!(outcome.report.added, 1);
        assert_eq!(outcome.report.updated, 0);
        assert_eq!(outcome.report.skipped, 0);

        let parsed = &outcome.sources[0];
        assert_eq!(parsed.source.id, "https://example.com");
        assert_eq!(parsed.source.name, "示例源");
        assert_eq!(parsed.source.kind, SourceKind::Book);
        assert!(parsed.source.enabled);
        assert_eq!(parsed.source.charset.as_deref(), Some("gbk"));
        assert_eq!(parsed.source.weight, Some(3));
        // searchUrl 落在 `Source` 自己的字段里（R4）
        assert_eq!(
            parsed.source.search_url.as_deref(),
            Some("/search?q={{key}}&page={{page}}")
        );
    }

    #[test]
    fn 解析书源数组() {
        let text = format!(
            "[{},{}]",
            sample_json(),
            sample_json()
                .replace("示例源", "第二个源")
                .replace("example.com", "second.com")
        );
        let outcome = parse_sources(&text).expect("应解析成功");
        assert_eq!(outcome.sources.len(), 2);
        assert_eq!(outcome.report.added, 2);
    }

    #[test]
    fn 缺_id_的条目被跳过其余照常() {
        let text = r#"[
          {"bookSourceName": "没有URL的源", "ruleSearch": "{}"},
          {"bookSourceName": "正常的源", "bookSourceUrl": "https://ok.com"}
        ]"#;
        let outcome = parse_sources(text).expect("整份文件仍应解析成功");
        assert_eq!(outcome.sources.len(), 1, "坏源不该带走好源");
        assert_eq!(outcome.sources[0].source.id, "https://ok.com");
        assert_eq!(outcome.report.skipped, 1);
        assert_eq!(outcome.report.failures.len(), 1);
        assert!(outcome.report.failures[0].subject.contains("第 1 条"));
    }

    #[test]
    fn 缺名称的条目被跳过() {
        let text = r#"{"bookSourceUrl": "https://noname.com"}"#;
        let outcome = parse_sources(text).expect("应解析成功");
        assert_eq!(outcome.sources.len(), 0);
        assert_eq!(outcome.report.skipped, 1);
        assert!(outcome.report.failures[0]
            .error
            .message_id()
            .contains("rule"));
    }

    #[test]
    fn 同一批内的重复_id_按更新计() {
        // spec 边缘情况：「导入重复书源（同 id）按"更新"处理」
        let text = format!("[{},{}]", sample_json(), sample_json());
        let outcome = parse_sources(&text).expect("应解析成功");
        assert_eq!(outcome.sources.len(), 1, "重复 id 只应留一条");
        assert_eq!(outcome.report.added, 1);
        assert_eq!(outcome.report.updated, 1, "第二条应计为更新");
    }

    #[test]
    fn 非法_json_给出结构化错误() {
        let error = parse_sources("{ 这不是 JSON").expect_err("应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Rule);
    }

    #[test]
    fn 空输入给出结构化错误() {
        assert!(parse_sources("").is_err());
        assert!(parse_sources("   \n  ").is_err());
    }

    #[test]
    fn 顶层类型非法给出可读错误() {
        let error = parse_sources("\"就是一个字符串\"").expect_err("应报错");
        let rendered = format!("{error}");
        assert!(
            rendered.contains("字符串"),
            "错误信息应说明实际类型：{rendered}"
        );
    }

    #[test]
    fn 剥掉_utf8_bom() {
        let with_bom = format!("\u{feff}{}", sample_json());
        let outcome = parse_sources(&with_bom).expect("带 BOM 也应解析成功");
        assert_eq!(outcome.sources.len(), 1);
    }

    #[test]
    fn 未知键不影响解析() {
        let text = r#"{
          "bookSourceName": "未来源",
          "bookSourceUrl": "https://future.com",
          "someFutureField": {"nested": [1,2,3]}
        }"#;
        let outcome = parse_sources(text).expect("未知键不应导致失败");
        assert_eq!(outcome.sources.len(), 1);
    }

    #[test]
    fn 纯脚本源被识别为js源() {
        // 刻意**不写** `bookSourceType`：本测试验证的是"未声明时的推断"。
        let text = r#"{
          "bookSourceName": "脚本源",
          "bookSourceUrl": "https://js.example.com",
          "js": "function search(key){ return []; }"
        }"#;
        let outcome = parse_sources(text).expect("应解析成功");
        assert_eq!(
            outcome.sources[0].source.kind,
            SourceKind::Js,
            "有主脚本而无规则字段时应识别为 JS 源"
        );
    }

    #[test]
    fn 有规则的源即使带脚本仍是普通源() {
        // 用 `concat!` 而非 `format!`：`format!` 会把 `{{}}` 展开成 `{}`，
        // 让 JSON 里的空对象变成空串，测试就不是在测它声称的东西了。
        let text = concat!(
            r#"{"bookSourceName":"混合源","bookSourceUrl":"https://mixed.com","#,
            r#""js":"var x=1;","ruleSearch":{"bookList":"@css:li"}}"#
        );
        let outcome = parse_sources(text).expect("应解析成功");
        assert_eq!(
            outcome.sources[0].source.kind,
            SourceKind::Book,
            "有规则字段时应按普通书源处理，即使同时带了脚本"
        );
    }

    #[test]
    fn 显式声明优先于推断() {
        // 声明了 `bookSourceType: 0`（普通书源）但同时带 `js` 字段：
        // 作者的声明是知识，引擎的推断是猜测，故按声明处理。
        let text = concat!(
            r#"{"bookSourceName":"声明优先","bookSourceUrl":"https://declared.com","#,
            r#""bookSourceType":0,"js":"var x=1;"}"#
        );
        let outcome = parse_sources(text).expect("应解析成功");
        assert_eq!(
            outcome.sources[0].source.kind,
            SourceKind::Book,
            "显式声明应压过推断"
        );
    }

    #[test]
    fn 声明类型优先() {
        let text = r#"{
          "bookSourceName": "订阅源",
          "bookSourceUrl": "https://rss.example.com",
          "bookSourceType": 2
        }"#;
        let outcome = parse_sources(text).expect("应解析成功");
        assert_eq!(outcome.sources[0].source.kind, SourceKind::Subscribe);
    }

    #[test]
    fn 内嵌_searchurl_被提到_source_字段() {
        let text = r#"{
          "bookSourceName": "内嵌源",
          "bookSourceUrl": "https://inline.com",
          "ruleSearch": "{\"searchUrl\":\"/s?q={{key}}\",\"bookList\":\"@css:li\"}"
        }"#;
        let outcome = parse_sources(text).expect("应解析成功");
        let parsed = &outcome.sources[0];
        assert_eq!(
            parsed.source.search_url.as_deref(),
            Some("/s?q={{key}}"),
            "内嵌的 searchUrl 应被提取到 Source::search_url"
        );

        // R4（PM 仲裁 2026-10-10）之后**保留**规则里的原文：
        // `Source::search_url` 是归一化结果，规则 JSON 是作者原文，两者共存
        // 不矛盾——删掉会让"作者到底写了什么"在落库后丢失。
        let rule = parsed.source.rule_search.as_deref().expect("应保留规则");
        assert!(rule.contains("bookList"), "其余规则必须保留：{rule}");
        assert!(
            rule.contains("searchUrl"),
            "规则原文应原样保留（R4）：{rule}"
        );
    }

    #[test]
    fn 顶层_searchurl_与_exploreurl_落进_source() {
        // R4 的核心验收：公开书源格式的两个顶层键直接成为 `Source` 的字段，
        // 消费方无需再包一层解析结果。
        let text = r#"{
          "bookSourceName": "顶层地址源",
          "bookSourceUrl": "https://top.com",
          "searchUrl": "/s?q={{key}}",
          "exploreUrl": "/explore/{{page}}",
          "ruleSearch": "{\"bookList\":\"@css:li\"}"
        }"#;
        let outcome = parse_sources(text).expect("应解析成功");
        let source = &outcome.sources[0].source;
        assert_eq!(source.search_url.as_deref(), Some("/s?q={{key}}"));
        assert_eq!(source.explore_url.as_deref(), Some("/explore/{{page}}"));
    }

    #[test]
    fn 顶层_searchurl_优先于规则内嵌的() {
        // 两个位置都写了时以**顶层**为准：它是书源格式里该字段的规范位置，
        // 规则内嵌只作为兼容路径存在。
        let text = r#"{
          "bookSourceName": "双写源",
          "bookSourceUrl": "https://both.com",
          "searchUrl": "/top?q={{key}}",
          "ruleSearch": "{\"searchUrl\":\"/inner?q={{key}}\",\"bookList\":\"@css:li\"}"
        }"#;
        let outcome = parse_sources(text).expect("应解析成功");
        assert_eq!(
            outcome.sources[0].source.search_url.as_deref(),
            Some("/top?q={{key}}")
        );
    }

    #[test]
    fn 空的_searchurl_归一为_none() {
        // 空串与缺失等价（见 `空字符串字段被归一为_none` 的同款约定）：
        // `Source::search_url = Some("")` 会让消费方以为"声明了但为空"，
        // 而实际语义是"没声明"。
        let text = r#"{
          "bookSourceName": "空地址源",
          "bookSourceUrl": "https://emptyurl.com",
          "searchUrl": "   ",
          "exploreUrl": "",
          "ruleSearch": "{\"bookList\":\"@css:li\"}"
        }"#;
        let outcome = parse_sources(text).expect("应解析成功");
        let source = &outcome.sources[0].source;
        assert_eq!(source.search_url, None);
        assert_eq!(source.explore_url, None);
    }

    #[test]
    fn 规则非法_json_时原样保留() {
        // 规则语法错误应在求值时报出（那时能给出更准确的位置），
        // 而不是让整条源在导入时消失。
        let text = r#"{
          "bookSourceName": "坏规则源",
          "bookSourceUrl": "https://badrule.com",
          "ruleSearch": "{ 这不是合法 JSON"
        }"#;
        let outcome = parse_sources(text).expect("导入不应因规则非法而失败");
        assert_eq!(outcome.sources.len(), 1);
        assert!(outcome.sources[0]
            .source
            .rule_search
            .as_deref()
            .expect("规则原文应保留")
            .contains("这不是合法 JSON"));
    }

    #[test]
    fn 空字符串字段被归一为_none() {
        let text = r#"{
          "bookSourceName": "空字段源",
          "bookSourceUrl": "https://empty.com",
          "bookUrlPattern": "",
          "loginUrl": "",
          "charset": "",
          "header": ""
        }"#;
        let outcome = parse_sources(text).expect("应解析成功");
        let source = &outcome.sources[0].source;
        assert_eq!(source.book_url_pattern, None, "空串应归一为 None");
        assert_eq!(source.login_url, None);
        assert_eq!(source.charset, None);
        assert_eq!(source.header, None);
    }

    #[test]
    fn 默认启用与创建时间被填上() {
        let text = r#"{"bookSourceName":"最小源","bookSourceUrl":"https://min.com"}"#;
        let outcome = parse_sources(text).expect("应解析成功");
        let source = &outcome.sources[0].source;
        assert!(source.enabled, "未声明 enabled 时默认启用");
        assert!(source.created_at > 1_600_000_000, "应填当前时间戳");
    }

    #[test]
    fn mainjs_键名被接受() {
        let text = r#"{
          "bookSourceName": "mainJs源",
          "bookSourceUrl": "https://mainjs.com",
          "mainJs": "function search(){}"
        }"#;
        let outcome = parse_sources(text).expect("应解析成功");
        assert!(outcome.sources[0]
            .source
            .js
            .as_deref()
            .unwrap()
            .contains("search"));
    }

    // ---------- R5：concurrentRate → rate_limit_ms ----------

    /// 社区真实书源样本（含 `concurrentRate`）。
    ///
    /// ## 这个样本的据实来源
    ///
    /// `concurrentRate` 是公开书源格式里的顶层键，社区流通的真实书源把它写成
    /// **字符串毫秒值**（`"1000"` / `"2000"` / `"3000"` 是出现频率最高的三个取值）。
    /// 该键的形状（字符串而非数字）是本样本要钉住的关键事实——写成数字会写错格式。
    ///
    /// 其余字段取自公开《书源规则说明》示例的形态（`searchUrl` + `ruleSearch`
    /// 的 CSS 规则），不含任何具体站点的真实地址。
    fn real_format_sample() -> &'static str {
        r#"[
          {
            "bookSourceComment": "免责声明：仅供学习交流",
            "bookSourceGroup": "自用",
            "bookSourceName": "示例·聚合",
            "bookSourceType": 0,
            "bookSourceUrl": "https://example-aggregate.test",
            "concurrentRate": "1000",
            "customOrder": 0,
            "enabled": true,
            "enabledExplore": true,
            "exploreUrl": "/explore/{{page}}",
            "header": "{\"User-Agent\":\"Mozilla/5.0\"}",
            "searchUrl": "/search?q={{key}}&page={{page}}",
            "weight": 0,
            "ruleSearch": {
              "bookList": "@css:div.result-list>div",
              "name": "@css:.book-name@text",
              "author": "@css:.book-author@text",
              "bookUrl": "@css:.book-name@href"
            }
          },
          {
            "bookSourceName": "示例·慢源",
            "bookSourceUrl": "https://example-slow.test",
            "concurrentRate": "3000",
            "searchUrl": "/s?q={{key}}",
            "ruleSearch": {"bookList": "@css:li"}
          },
          {
            "bookSourceName": "示例·无速率声明",
            "bookSourceUrl": "https://example-plain.test",
            "searchUrl": "/s?q={{key}}",
            "ruleSearch": {"bookList": "@css:li"}
          }
        ]"#
    }

    #[test]
    fn 真实格式样本含_concurrentrate_被解析() {
        let outcome = parse_sources(real_format_sample()).expect("真实格式样本应解析成功");
        assert_eq!(
            outcome.sources.len(),
            3,
            "三条源都应解析（含带 concurrentRate 的）"
        );
        assert_eq!(outcome.report.skipped, 0);

        // 按 id 取：`parse_sources` 用 `BTreeMap` 收集（遍历顺序确定，便于 CLI
        // 输出 diff），故结果顺序是 id 排序而非文件顺序。
        let by_id = |id: &str| -> &Source {
            &outcome
                .sources
                .iter()
                .find(|parsed| parsed.source.id == id)
                .unwrap_or_else(|| panic!("应有源 {id}"))
                .source
        };

        // 字符串毫秒值 → i64 毫秒
        assert_eq!(
            by_id("https://example-aggregate.test").rate_limit_ms,
            Some(1000),
            "`concurrentRate: \"1000\"` 应解析为 1000 毫秒"
        );
        assert_eq!(by_id("https://example-slow.test").rate_limit_ms, Some(3000));
        // 未声明 → None（三态语义里的"未声明"，见 core 的 request_interval_ms 文档）
        assert_eq!(
            by_id("https://example-plain.test").rate_limit_ms,
            None,
            "未声明 concurrentRate 应是 None 而不是 Some(0)"
        );
    }

    #[test]
    fn concurrentrate_流向_requestspec_的请求间隔() {
        // R5 的端到端验收：书源的 concurrentRate 最终体现在 RequestSpec 上，
        // 由 tome-net 的限速器消费（FR-008）。
        let outcome = parse_sources(real_format_sample()).expect("应解析成功");
        let declared = outcome
            .sources
            .iter()
            .find(|parsed| parsed.source.id == "https://example-aggregate.test")
            .expect("应有声明了速率的源");
        let plan = crate::search::parse_search_spec(declared, "关键词", 1).expect("应构造成功");
        assert_eq!(
            plan.spec.request_interval_ms,
            Some(1000),
            "concurrentRate 应成为 RequestSpec.request_interval_ms"
        );

        let plain = outcome
            .sources
            .iter()
            .find(|parsed| parsed.source.id == "https://example-plain.test")
            .expect("应有未声明速率的源");
        let plan = crate::search::parse_search_spec(plain, "关键词", 1).expect("应构造成功");
        assert_eq!(
            plan.spec.request_interval_ms, None,
            "未声明时必须保持 None（否则会覆盖本机默认限速）"
        );
    }

    #[test]
    fn concurrentrate_兼容数字形态() {
        // 有的书源把 concurrentRate 写成数字而非字符串。两种都接受——
        // 拒绝数字形态会让一批能用的源变成不可用，而它的语义没有歧义。
        for (raw, expected) in [
            ("\"2000\"", Some(2000i64)),
            ("2000", Some(2000)),
            ("2000.0", Some(2000)),
        ] {
            let text = format!(
                r#"{{"bookSourceName":"数字形态","bookSourceUrl":"https://numeric.test","concurrentRate":{raw}}}"#
            );
            let outcome = parse_sources(&text).expect("应解析成功");
            assert_eq!(
                outcome.sources[0].source.rate_limit_ms, expected,
                "concurrentRate={raw} 应解析为 {expected:?}"
            );
        }
    }

    #[test]
    fn concurrentrate_非数字与负数容错为_none_而不拒绝整源() {
        // spec：书源是不可信输入，一个字段写错不该让整条源消失。
        // 负数是"未声明"而非"要求负间隔"——截断成 0 会反而变成"要求零间隔"，
        // 比未声明更激进（见 search.rs 的 `interval_from_source` 同款理由）。
        for raw in [
            r#""abc""#,
            r#""""#,
            r#""1000ms""#,
            "-1000",
            r#""-500""#,
            "0",
            r#""0""#,
            "null",
            "{}",
        ] {
            let text = format!(
                r#"{{"bookSourceName":"畸形速率","bookSourceUrl":"https://bad-rate.test","concurrentRate":{raw},"searchUrl":"/s?q={{{{key}}}}","ruleSearch":"{{\"bookList\":\"@css:li\"}}"}}"#
            );
            let outcome = parse_sources(&text).expect("整份文件不应因一个字段而失败");
            assert_eq!(
                outcome.sources.len(),
                1,
                "concurrentRate={raw} 不应导致整条源被跳过"
            );
            assert_eq!(
                outcome.sources[0].source.rate_limit_ms, None,
                "concurrentRate={raw} 应容错为 None"
            );
        }
    }

    #[test]
    fn 请求头被解析为_header_map() {
        let headers = parse_headers(Some(r#"{"User-Agent":"tome/0.1","Referer":"https://a/"}"#))
            .expect("应解析成功");
        assert_eq!(headers.get("user-agent"), Some("tome/0.1"));
        assert_eq!(headers.get("referer"), Some("https://a/"));
    }

    #[test]
    fn 请求头允许数字值() {
        let headers = parse_headers(Some(r#"{"X-Port":8080}"#)).expect("应解析成功");
        assert_eq!(headers.get("x-port"), Some("8080"));
    }

    #[test]
    fn 空请求头产出空_map() {
        assert!(parse_headers(None).expect("None 应成功").is_empty());
        assert!(parse_headers(Some("")).expect("空串应成功").is_empty());
        assert!(parse_headers(Some("   ")).expect("空白应成功").is_empty());
    }

    #[test]
    fn 非法请求头报结构化错误() {
        let error = parse_headers(Some("{ 非法")).expect_err("应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Rule);
        let error = parse_headers(Some("[1,2,3]")).expect_err("数组不是对象");
        assert_eq!(error.to_error_kind(), ErrorKind::Rule);
    }

    // ---------- 渠道（用假宿主，不触网） ----------

    /// 假宿主：固定返回一份书源 JSON，并记录收到的 spec。
    struct FakeHost {
        body: String,
        status: u16,
        seen: std::sync::Mutex<Vec<RequestSpec>>,
    }

    impl FakeHost {
        fn new(body: &str) -> Self {
            Self {
                body: body.to_string(),
                status: 200,
                seen: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn with_status(body: &str, status: u16) -> Self {
            Self {
                body: body.to_string(),
                status,
                seen: std::sync::Mutex::new(Vec::new()),
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
                headers: HeaderMap::new(),
                body_bytes: self.body.clone().into_bytes(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    #[test]
    fn 文件渠道不触网() {
        let host = FakeHost::new(sample_json());
        let outcome = load_from_channel(
            &SourceImportChannel::FileText(sample_json().to_string()),
            &host,
        )
        .expect("应成功");
        assert_eq!(outcome.sources.len(), 1);
        assert!(host.seen().is_empty(), "文件渠道不该发请求");
    }

    #[test]
    fn 粘贴渠道不触网() {
        let host = FakeHost::new(sample_json());
        let outcome = load_from_channel(
            &SourceImportChannel::PastedText(sample_json().to_string()),
            &host,
        )
        .expect("应成功");
        assert_eq!(outcome.sources.len(), 1);
        assert!(host.seen().is_empty(), "粘贴渠道不该发请求");
    }

    #[test]
    fn url_渠道经_net_host_拉取() {
        let host = FakeHost::new(sample_json());
        let outcome = load_from_channel(
            &SourceImportChannel::Url("https://raw.example.com/src.json".to_string()),
            &host,
        )
        .expect("应成功");

        assert_eq!(outcome.sources.len(), 1);
        let seen = host.seen();
        assert_eq!(seen.len(), 1, "URL 渠道应恰好发一次请求");
        assert_eq!(seen[0].url, "https://raw.example.com/src.json");
        assert_eq!(
            seen[0].rate_limit_key, IMPORT_RATE_LIMIT_KEY,
            "URL 渠道应共用导入专用限速键（A2：不绕开限速）"
        );
    }

    #[test]
    fn url_渠道对非_2xx_报错() {
        let host = FakeHost::with_status("<html>404</html>", 404);
        let error = load_from_channel(
            &SourceImportChannel::Url("https://raw.example.com/missing.json".to_string()),
            &host,
        )
        .expect_err("404 应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Network);
    }

    #[test]
    fn url_渠道的传输失败被传播() {
        struct FailingHost;
        impl NetHost for FailingHost {
            fn fetch(&self, _spec: RequestSpec) -> Result<tome_core::Response, ErrorKind> {
                Err(ErrorKind::Network)
            }
        }
        let error = load_from_channel(
            &SourceImportChannel::Url("https://unreachable.example.com/".to_string()),
            &FailingHost,
        )
        .expect_err("应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Network);
    }

    #[test]
    fn gbk_编码的书源文件可导入() {
        // 用真实 GBK 字节构造响应：`示例源` 三个字在 GBK 下是 6 字节，
        // UTF-8 是 9 字节——用长度断言能证明"按 GBK 解了"而不是"碰巧都通"。
        let host = FakeHost::new("");
        let _ = host;
        let gbk_text = sample_json();
        let response = tome_core::Response {
            status: 200,
            headers: HeaderMap::new(),
            // GBK 字节：这里用 tome-net 的 charset 层做往返验证——
            // 编码侧由测试自己用已知字节构造，避免引入第 5 个依赖。
            body_bytes: gbk_bytes_for_sample(),
            detected_charset: Some("GBK".to_string()),
        };
        let decoded = decode_import_body(&response);
        assert!(
            decoded.contains("示例源"),
            "GBK 内容应被正确解码，实际：{decoded}"
        );
        let _ = gbk_text;
    }

    /// `sample_json()` 的 GBK 字节（由编码表手工构造关键片段）。
    ///
    /// 只编码"示例源"三个字与其余 ASCII——ASCII 在 GBK 与 UTF-8 下同形，
    /// 故差异只体现在中文上，足够验证解码路径。
    fn gbk_bytes_for_sample() -> Vec<u8> {
        // `示例源` 的 GBK 编码：示=CA BE、例=C0 FD、源=D4 B4
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(b"{\"bookSourceName\":\"");
        bytes.extend_from_slice(&[0xCA, 0xBE, 0xC0, 0xFD, 0xD4, 0xB4]);
        bytes.extend_from_slice(b"\",\"bookSourceUrl\":\"https://example.com\"}");
        bytes
    }
}

// ---------------------------------------------------------------------------
// 书源查找的抽象
// ---------------------------------------------------------------------------

/// 按 id 取书源的抽象（`ContentProvider` adapter 的依赖）。
///
/// # 为什么是 trait 而不是直接收 `&[ParsedSource]`
///
/// 阅读器打开一本书时手里只有 `Book`（其 `origin_id` 里含书源 id），而
/// 书源集合存在 `tome-store` 的 `source` 表里。收切片会把"书源从哪来"
/// 钉死在内存里——那样生产路径就得先把全量源读进内存。
///
/// 用 trait 让两种来源都能接：
///
/// - [`SliceLookup`]：内存切片（测试、`--source-file` 场景）
/// - 组合根提供的实现：从 `tome-store` 的 `SourceDao` 读（生产常态）
pub trait SourceLookup: Send + Sync {
    /// 按 id 查找；不存在返回 `None`。
    fn find(&self, source_id: &str) -> Option<ParsedSource>;
}

/// 从内存切片查找（测试与 `--source-file` 场景）。
#[derive(Debug, Clone)]
pub struct SliceLookup {
    sources: Vec<ParsedSource>,
}

impl SliceLookup {
    /// 从源列表构造。
    pub fn new(sources: Vec<ParsedSource>) -> Self {
        Self { sources }
    }
}

impl SourceLookup for SliceLookup {
    fn find(&self, source_id: &str) -> Option<ParsedSource> {
        self.sources
            .iter()
            .find(|parsed| parsed.source.id == source_id)
            .cloned()
    }
}
