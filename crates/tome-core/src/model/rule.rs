//! 规则 DTO（data-model.md 的 `replace_rule` / `highlight_rule` / `dict_rule` /
//! `txt_toc_rule` 表）
//!
//! ## 这些 DTO 为什么不承载规则求值
//!
//! 规则的**解释与求值**属于 tome-rule（分派/切分/CSS/JSONPath/XPath/正则）。
//! 此处只承载"规则是什么"的数据形状，不包含"规则怎么跑"。这样规则引擎的
//! 频繁演进不会拖着 core 重编译（宪法 IV：core 变更不应波及全员）。

/// 替换/净化规则的生效范围（data-model.md 的 `replace_rule.scope`）。
///
/// FR-014 要求净化链顺序固定（替换 → 高亮 → 词典），范围决定一条替换规则
/// 作用在哪一类文本上。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleScope {
    /// 正文内容
    Content,
    /// 书名
    BookName,
    /// 章节标题
    ChapterTitle,
    /// 全局（所有文本）
    Global,
}

impl RuleScope {
    /// 转为存储层整数编码。
    pub fn as_storage_code(self) -> i32 {
        match self {
            Self::Content => 0,
            Self::BookName => 1,
            Self::ChapterTitle => 2,
            Self::Global => 3,
        }
    }

    /// 从存储层整数编码还原；未知编码返回 `None`。
    pub fn from_storage_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Content),
            1 => Some(Self::BookName),
            2 => Some(Self::ChapterTitle),
            3 => Some(Self::Global),
            _ => None,
        }
    }
}

/// 内容加工链的层级标识。
///
/// FR-014 规定处理顺序 MUST 为 **替换 → 高亮 → 词典**，且该顺序对所有消费者一致。
/// 把顺序表达成 `Ord` 而非散落在各实现 crate 的常量，是"顺序只有一个定义处"
/// 的落地方式——实现方按 `RuleTier` 排序即可，无从写错。
///
/// `Toc` 不参与内容加工链（它作用于目录识别），故排在最后并单独说明。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleTier {
    /// 替换/净化（FR-014 第一级）
    Replace,
    /// 自动高亮求值（FR-014 第二级）
    Highlight,
    /// 词典查词（FR-014 第三级）
    Dict,
    /// 目录识别（不属内容加工链，仅作分类标签）
    Toc,
}

/// 替换/净化规则（data-model.md 的 `replace_rule` 表）。
///
/// FR-004：支持按正则的查找替换，且可叠加。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceRule {
    /// 主键
    pub id: i64,
    /// 规则名（供界面展示）
    pub name: String,
    /// 匹配模式（正则或字面量，由 `is_regex` 决定）
    pub pattern: String,
    /// 替换为
    pub replacement: String,
    /// 生效范围
    pub scope: RuleScope,
    /// `pattern` 是否为正则
    pub is_regex: bool,
    /// 是否启用
    pub enabled: bool,
    /// 应用顺序（同层内排序）
    pub sort_order: i32,
}

/// 高亮规则（data-model.md 的 `highlight_rule` 表）。
///
/// FR-015：求值产出的必须是**与排版无关的字符区间**。
/// 这里只定义"规则"；产出的区间由 [`crate::model::Highlight`] 承载。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightRule {
    /// 主键
    pub id: i64,
    /// 规则名
    pub name: String,
    /// 匹配模式
    pub pattern: String,
    /// 应用到区间的样式标识（具体渲染由 S2 排版层解释）
    pub style: String,
    /// 是否启用
    pub enabled: bool,
    /// 应用顺序
    pub sort_order: i32,
}

/// 词典规则（data-model.md 的 `dict_rule` 表）。FR-016：给定词语产出释义结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictRule {
    /// 主键
    pub id: i64,
    /// 规则名
    pub name: String,
    /// 查词地址规则（含占位符，由 tome-rule 求值）
    pub url_rule: String,
    /// 释义提取规则
    pub content_rule: String,
    /// 是否启用
    pub enabled: bool,
}

/// TXT 目录识别规则（data-model.md 的 `txt_toc_rule` 表）。
///
/// FR-020：识别章节目录时支持**多条规则回退**。故同层内的 `sort_order`
/// 即回退尝试顺序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxtTocRule {
    /// 主键
    pub id: i64,
    /// 规则名
    pub name: String,
    /// 识别规则（正则或脚本，由内容形态判定）
    pub rule: String,
    /// 示例（供用户核对规则效果）
    pub example: Option<String>,
    /// 是否启用
    pub enabled: bool,
    /// 回退尝试顺序
    pub sort_order: i32,
}
