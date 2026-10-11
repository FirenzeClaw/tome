//! \[DONE] 词典查词桥（T044 / FR-041）
//!
//! # 本模块做什么，以及**不做什么**
//!
//! FR-041 的核心约束是「词典求值本身 MUST 复用 001 §FR-016 的既有能力，
//! 本层 MUST NOT 实现第二套词典规则求值」。故本模块是**桥**，不是引擎：
//!
//! | 职责 | 归属 |
//! |---|---|
//! | 规则语法解析（`{{word}}` 展开、CSS/JSONPath 提取） | **001**（`tome-source` / `tome-rule`） |
//! | 产出"待查的词 × 适用规则"清单 | **001**（`PipelineOutcome.dict_queries`） |
//! | 触发（选择/光标）→ 请求构造 + 失败诊断 | **本模块** |
//! | 实际发请求 | 001 管线（A2：唯一发请求处是 `tome-source`） |
//! | 结果展示 | S3 外壳 |
//!
//! 本模块的输入是 001 已经产出的 [`DictQueryInput`]（即 `dict_queries` 的
//! 等价形态：词 + 规则 id + 两条规则串），输出是 [`DictLookupRequest`]
//! ——**透传**规则串，不做任何解释。`tests/dict_bridge.rs` 有一条源码扫描
//! 断言本文件不认识 `@css:` 这类规则方言。
//!
//! ## 为什么规则串是"透传"而不是"不传"
//!
//! 请求最终要由发请求的一方执行，它需要规则串才能求值。但**本层不读**它
//! ——只把它从上游搬到下游。这与"不做第二套求值"并不矛盾：搬运不是解释。

use crate::trace::hit::Selection;

// ===========================================================================
// 输入：001 的产物
// ===========================================================================

/// 一条词典规则的**引用**（规则 id + 两条规则串）。
///
/// ## 字段与 001 的 `DictQuery` 一一对应
///
/// `tome_rule::pipeline::DictQuery` 的字段是 `word / rule_id / url_rule /
/// content_rule`。本类型是它的"规则部分"——词已由上层按词分组（见
/// [`DictQueryInput`] 的说明）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictRuleRef {
    /// 规则 id（可追溯"这条释义是哪条规则给的"）
    pub rule_id: String,
    /// 查词地址规则（**透传**，本层不展开其中的占位符）
    pub url_rule: String,
    /// 释义提取规则（**透传**）
    pub content_rule: String,
}

/// 一次查词的输入：一个词 + 001 为它产出的可用规则清单。
///
/// ## 为什么"词"在类型上而不是从规则里推
///
/// 001 的 `run_dict_stage` 是"对每个词，把每条启用的规则各产出一条
/// `DictQuery`"。故同一个词会出现在多条 query 里。本类型把它提出来，
/// 让"一次查词 = 一个词 + N 条规则"成为类型事实，避免下游自己去做分组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictQueryInput {
    /// 待查的词（来自选择或光标处的分词，由上层给出）
    pub word: String,
    /// 001 为该词产出的规则清单（空表示无可用规则）
    pub queries: Vec<DictRuleRef>,
}

// ===========================================================================
// 触发：阅读层的两种交互形态
// ===========================================================================

/// 一次词典查询的**触发源**（FR-041 的"阅读层的触发"）。
///
/// ## 为什么用字符区间表达触发
///
/// 触发点用**章内字符区间**而非屏幕坐标——与 FR-028 同一条纪律：坐标是
/// 输入（命中测试的产物），字符区间才是可持久、可比较的量。选择触发带
/// 完整区间；光标触发是零长区间（其后由上层按分词扩展出词）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictTrigger {
    char_range: std::ops::Range<usize>,
}

impl DictTrigger {
    /// 由**选择**触发（拖选一段文字后查词）。
    #[must_use]
    pub fn from_selection(range: std::ops::Range<usize>) -> Self {
        Self { char_range: range }
    }

    /// 由**光标位置**触发（单击某处后查词；词由上层按分词扩展）。
    #[must_use]
    pub fn at_cursor(char_offset: usize) -> Self {
        Self {
            char_range: char_offset..char_offset,
        }
    }

    /// 由 FR-038 的[`Selection`]构造（选择结果可直接作为触发源）。
    #[must_use]
    pub fn from_selection_result(selection: &Selection) -> Self {
        Self {
            char_range: selection.char_range.clone(),
        }
    }

    /// 触发的章内字符区间（与 001 锚点同口径）。
    #[must_use]
    pub fn char_range(&self) -> std::ops::Range<usize> {
        self.char_range.clone()
    }
}

// ===========================================================================
// 输出：请求
// ===========================================================================

/// 一次词典查询请求（交给执行方发出去）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictLookupRequest {
    /// 待查的词（已去首尾空白）
    pub word: String,
    /// 要执行的规则清单（**透传**自上游，含规则 id 以便追溯）
    pub queries: Vec<DictRuleRef>,
}

// ===========================================================================
// 失败：可操作的原因（US4 场景 4）
// ===========================================================================

/// 查词失败的原因（**结构化标识**，不是面向用户的文案）。
///
/// ## 为什么是枚举而不是字符串
///
/// 上层要按原因分流（"无规则"应引导去设置页，"请求失败"应提供重试）。
/// 用字符串让分流只能靠匹配文案——文案一改分流就悄悄失效。枚举把分流
/// 做成编译期穷举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictFailure {
    /// 词为空（用户没选中任何文字就去查词）
    EmptyWord,
    /// 没有可用的词典规则（未配置或全部禁用）
    NoRules,
    /// 请求本身失败（网络/规则求值错误，由执行方报告）
    RequestFailed,
    /// 请求成功但没提取到释义（规则匹配不到内容）
    EmptyResult,
}

impl DictFailure {
    /// 稳定标识（驼峰，书源生态通用叫法；**不是**面向用户的文案）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmptyWord => "emptyWord",
            Self::NoRules => "noDictRules",
            Self::RequestFailed => "dictRequestFailed",
            Self::EmptyResult => "dictEmptyResult",
        }
    }

    /// **可操作**的技术提示（英文，供日志与上层映射；FR-058：引擎内不做本地化）。
    ///
    /// ## "可操作"的判据
    ///
    /// 提示必须指向**下一步能做什么**，而不是复述问题（"查询失败"就不是
    /// 可操作的提示）。四种原因各有一个明确的下一步：
    ///
    /// | 原因 | 下一步 |
    /// |---|---|
    /// | 词为空 | 先选中一段文字 |
    /// | 无规则 | 去配置词典规则 |
    /// | 请求失败 | 检查网络或规则，稍后重试 |
    /// | 无结果 | 换一条规则或换一个词 |
    #[must_use]
    pub const fn guidance(self) -> &'static str {
        match self {
            Self::EmptyWord => "select a word or place the cursor on one before looking it up",
            Self::NoRules => "no enabled dict rule: add or enable one in dict rule settings",
            Self::RequestFailed => "dict request failed: check the rule's url/network and retry",
            Self::EmptyResult => "dict rule returned no content: try another rule or another word",
        }
    }
}

impl std::fmt::Display for DictFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 由 001 的产物构造查词请求（FR-041 的桥）。
///
/// ## 失败路径
///
/// 两种前置条件不满足时返回 [`DictFailure`]（**不静默**）：
/// 词为空、无可用规则。其余两种（请求失败 / 无结果）发生在执行阶段，
/// 由执行方用同一枚举报告——本函数只负责前置校验与构造。
///
/// ## 为什么只去首尾空白而不分词
///
/// 分词是**上游**的事（001 的词典求值是"给定词产出释义"，而"从选择里
/// 拿出哪个词"取决于语言）。本层若自己分词，就又在"词是什么"上做了一次
/// 判断——那正是第二套实现的另一种形态。
pub fn build_lookup_request(input: &DictQueryInput) -> Result<DictLookupRequest, DictFailure> {
    let word = input.word.trim();
    if word.is_empty() {
        return Err(DictFailure::EmptyWord);
    }
    if input.queries.is_empty() {
        return Err(DictFailure::NoRules);
    }
    Ok(DictLookupRequest {
        word: word.to_string(),
        queries: input.queries.clone(),
    })
}
