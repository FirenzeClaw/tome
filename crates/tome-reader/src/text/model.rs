//! `[DONE]` 规范化文本模型（T010 / FR-003）
//!
//! 本模块是排版层的**位置口径中枢**：它把一段原始文本变成"可作为排版输入"
//! 的规范形式，并保证**对外位置一律是字符下标**。
//!
//! ## 三层结构（FR-003）
//!
//! ```text
//! 段落  Paragraph   ← text/paragraph.rs（T011，用换行符与启发式切分）
//!   └── 字形簇 GraphemeCluster  ← 本模块（不断点不得落在簇内）
//!         └── 字符 char         ← Unicode 标量值（对外位置的单位）
//! ```
//!
//! 对应关系：一个字形簇含 1..N 个字符，一个字符含 1..N 个字节。
//!
//! ## 为什么对外只能是"字符下标"
//!
//! 001 的契约（`ReadProgress.char_offset`、`Highlight.char_offset`）以
//! **Unicode 标量值下标**为单位（其 data-model 明文："字符数，不是字节数——
//! 中文正文下两者差 3 倍，混淆会导致高亮整体偏移"）。
//!
//! S2 若在任何对外位置上用字节偏移，后果是：
//!
//! - 痕迹（高亮/书签）在含 emoji 的段落上整体错位
//! - 跨子项目的位置换算需要一层"谁的口径"的猜测
//!
//! 故本模块的公开面只以 [`CharIndex`] / 字符区间表达位置。字节偏移与
//! UTF-16 码元偏移只作为**内部实现细节**存在（[`TextModel::to_byte_offset`]
//! 是给 cosmic-text 用的，因为它的 `ShapeGlyph::start/end` 是**字节**区间）。
//!
//! ## 三种长度度量的关系（本模块的核心不变量）
//!
//! | 度量 | 含义 | `"中😀文"` 的值 |
//! |---|---|---|
//! | [`TextModel::char_len`] | Unicode 标量值个数（**对外口径**） | 3 |
//! | [`TextModel::utf16_len`] | UTF-16 码元个数（仅供 001 侧校验） | 4 |
//! | [`TextModel::byte_len`] | UTF-8 字节数（供 cosmic-text 对齐） | 10 |
//!
//! 三者只在纯 BMP 文本（不含代理对）上相等。测试用 emoji 把它们分开，
//! 以免把任一种误当成另一种。

use unicode_segmentation::UnicodeSegmentation;

/// 字符下标（Unicode 标量值序列下标）。
///
/// ## 为什么包一层而不直接用 `usize`
///
/// 排版的每个位置参数都可以是 `usize`（字符下标、字节偏移、簇下标、
/// 行号、段号）。全用裸 `usize` 时，把它们互相传错**不会编译失败**，
/// 而这类错误的表现是"位置整体偏移"——极难从现象反推。
/// 包一层让"字符下标"在类型上可见。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CharIndex(usize);

impl CharIndex {
    /// 构造。
    #[must_use]
    pub const fn new(value: usize) -> Self {
        Self(value)
    }

    /// 取值。
    #[must_use]
    pub const fn value(self) -> usize {
        self.0
    }
}

/// 一个带段落归属的文本位置。
///
/// `char_offset` 是**段内**偏移（与 001 的 `ReadProgress.char_offset`
/// 语义一致：章内偏移由段落起始位置加段内偏移得出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    /// 段落序号（0 起）
    pub paragraph_index: usize,
    /// 段内字符偏移（0 起）
    pub char_offset: usize,
}

impl TextPosition {
    /// 构造。
    #[must_use]
    pub const fn new(paragraph_index: usize, char_offset: usize) -> Self {
        Self {
            paragraph_index,
            char_offset,
        }
    }
}

/// 一个字形簇（grapheme cluster）。
///
/// `char_range` 是**字符**区间（不是字节区间）——它与 [`Self::text`] 的关系
/// 是一一对应的：区间长度等于文本的字符数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphemeCluster {
    /// 在所属文本中的字符区间
    pub char_range: std::ops::Range<usize>,
    /// 簇文本
    pub text: String,
    /// 该簇的起始字节偏移（供 cosmic-text 对齐；**不对外暴露为位置单位**）
    pub(crate) byte_start: usize,
}

impl GraphemeCluster {
    /// 簇的字符长度。
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.char_range.end - self.char_range.start
    }
}

/// 规范化文本模型。
///
/// ## 为什么不可变（构造即冻结）
///
/// 位置口径的价值在于**稳定**：若文本可在构造后被修改，所有已算出的
/// 字符下标都可能失效。排版管线是"文本进 → 行盒出"的单向流程，
/// 故本类型一次构造、只读使用，不提供任何 `&mut` 方法。
/// 需要改文本时构造新实例（上层负责重新映射位置）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextModel {
    /// 原始文本（排版不改写原文——FR-004：净化在 001 侧完成）
    text: String,
    /// 逐字形簇（构造时一次性切分）
    clusters: Vec<GraphemeCluster>,
    /// 字符下标 → 字节偏移（长度 = char_len + 1，末项为总字节数）
    char_to_byte: Vec<usize>,
    /// 可断点集合（长度 = char_len + 1；`true` 表示该字符位置可断行）
    ///
    /// 索引 i 表示"第 i 个字符**之前**"这个位置。故 `[0]` 与 `[char_len]`
    /// 恒为 `true`（文本首尾总是可断的边界）。
    breakable: Vec<bool>,
}

impl TextModel {
    /// 从文本构造（切分字形簇、建立下标映射与可断点表）。
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();

        // --- 字符下标 → 字节偏移 ---
        // 用 `char_indices` 天然得到"每个字符的起始字节"，末尾补总长。
        // 这条映射是后续一切区间换算的基础，通不过就会连锁出错。
        let mut char_to_byte: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
        char_to_byte.push(text.len());

        // --- 字形簇切分 ---
        // `grapheme_indices` 给出每个簇的 (字节起始, 簇文本)。
        // 注意：**簇文本的字符数未必是 1**（组合字符/ZWJ 序列/区域指示符对），
        // 故区间长度要按字符数算，不能假定为 1。
        let mut clusters = Vec::new();
        let mut char_cursor = 0usize;
        for (byte_start, cluster_text) in text.grapheme_indices(true) {
            let cluster_chars = cluster_text.chars().count();
            clusters.push(GraphemeCluster {
                char_range: char_cursor..char_cursor + cluster_chars,
                text: cluster_text.to_owned(),
                byte_start,
            });
            char_cursor += cluster_chars;
        }

        // --- 可断点表 ---
        // 只有**簇边界**可断：位置 i 可断 ⟺ i 是某个簇的起点（或文本末尾）。
        let char_len = char_cursor;
        let mut breakable = vec![false; char_len + 1];
        for cluster in &clusters {
            breakable[cluster.char_range.start] = true;
        }
        breakable[char_len] = true;
        // 空文本：位置 0 是唯一的边界
        if char_len == 0 {
            breakable[0] = true;
        }

        Self {
            text,
            clusters,
            char_to_byte,
            breakable,
        }
    }

    /// 原文。
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// **对外口径**：字符数（Unicode 标量值个数）。
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.char_to_byte.len() - 1
    }

    /// UTF-8 字节数。
    ///
    /// 供与 cosmic-text 对齐（其字形区间是字节单位）。**不是**对外位置口径。
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.text.len()
    }

    /// UTF-16 码元数。
    ///
    /// 001 侧（若与前端/持久层交互）可能出现该口径，本方法供**校验**
    /// 而非作为位置单位——它的存在是为了让"三种度量确实不同"这件事可断言。
    #[must_use]
    pub fn utf16_len(&self) -> usize {
        self.text.encode_utf16().count()
    }

    /// 字形簇数。
    #[must_use]
    pub fn cluster_count(&self) -> usize {
        self.clusters.len()
    }

    /// 逐字形簇。
    #[must_use]
    pub fn clusters(&self) -> &[GraphemeCluster] {
        &self.clusters
    }

    /// 字符下标 → 字节偏移。
    ///
    /// ## 为什么这个方法可以公开（而 `to_char_index` 的输入仍需谨慎）
    ///
    /// cosmic-text 的 `ShapeGlyph::start/end` 是**字节**区间，本层与它对接时
    /// 必须做一次换算。把换算显式化（而不是让调用方自己数字节）是必要的；
    /// 关键在于**换算是单向的出口**：对外位置仍是字符下标，
    /// 字节只在与第三方库对接的那一层短暂出现。
    ///
    /// 输入越界时钳制到文本末尾（而非 panic）——排版管线的边界处不应崩。
    #[must_use]
    pub fn to_byte_offset(&self, char_index: usize) -> usize {
        *self
            .char_to_byte
            .get(char_index)
            .unwrap_or_else(|| self.char_to_byte.last().unwrap_or(&0))
    }

    /// 字节偏移 → 字符下标；**落在字符内部或越界时返回 `None`**。
    ///
    /// ## 为什么拒绝而不是圆整
    ///
    /// 圆整会把"把字节偏移误当字符偏移用"这个 bug 变成静默的偏移错误。
    /// 返回 `None` 让误用立刻可见。这是本模块唯一以 `Option` 表达"这不是
    /// 一个合法位置"的地方。
    #[must_use]
    pub fn to_char_index(&self, byte_offset: usize) -> Option<usize> {
        self.char_to_byte
            .iter()
            .position(|offset| *offset == byte_offset)
    }

    /// 位置 `char_index`（第 N 个字符**之前**）是否可断行。
    ///
    /// **簇内不可断**：`e` 与其组合重音之间返回 `false`。禁则规则（T012）
    /// 在此之上叠加——本方法是"字形层面能否断"，不是"排版是否允许断"。
    #[must_use]
    pub fn is_breakable(&self, char_index: usize) -> bool {
        self.breakable.get(char_index).copied().unwrap_or(false)
    }

    /// 取字符区间的文本；越界或反向区间返回 `None`。
    #[must_use]
    pub fn char_range_text(&self, range: std::ops::Range<usize>) -> Option<&str> {
        if range.start > range.end || range.end > self.char_len() {
            return None;
        }
        // 字符区间 → 字节区间：两端都是字符起点，故换算安全
        let byte_start = self.to_byte_offset(range.start);
        let byte_end = self.to_byte_offset(range.end);
        self.text.get(byte_start..byte_end)
    }

    /// 包含给定字符位置的**字形簇下标**。
    ///
    /// 用于"把光标/锚点吸附到簇边界"——位置落在簇中间时吸附到该簇。
    #[must_use]
    pub fn cluster_index_at(&self, char_index: usize) -> Option<usize> {
        self.clusters
            .iter()
            .position(|cluster| cluster.char_range.contains(&char_index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_splits_clusters_and_maps_indices() {
        let model = TextModel::new("中文");
        assert_eq!(model.char_len(), 2);
        assert_eq!(model.cluster_count(), 2);
        assert!(model.is_breakable(1));
    }

    /// 空文本不得 panic（生产路径铁律）。
    #[test]
    fn empty_text_builds_without_panic() {
        let model = TextModel::new("");
        assert_eq!(model.char_len(), 0);
        assert!(model.is_breakable(0), "空文本的位置 0 是合法边界");
        assert!(!model.is_breakable(1), "越界位置不可断");
    }

    /// 簇边界表与簇列表必须自洽（防两处独立计算漂移）。
    #[test]
    fn breakable_table_matches_cluster_boundaries() {
        let model = TextModel::new("a\u{0301}b中\u{1F600}");

        for cluster in model.clusters() {
            assert!(
                model.is_breakable(cluster.char_range.start),
                "簇起点 {} 必须可断",
                cluster.char_range.start
            );
        }
        assert!(model.is_breakable(model.char_len()), "文本末尾必须可断");

        // 非簇起点的位置一律不可断
        let starts: Vec<usize> = model
            .clusters()
            .iter()
            .map(|cluster| cluster.char_range.start)
            .collect();
        for index in 0..model.char_len() {
            if !starts.contains(&index) {
                assert!(!model.is_breakable(index), "非簇起点 {index} 不得可断");
            }
        }
    }

    #[test]
    fn cluster_index_at_finds_containing_cluster() {
        let model = TextModel::new("e\u{0301}xy");
        assert_eq!(model.cluster_index_at(0), Some(0));
        assert_eq!(model.cluster_index_at(1), Some(0), "组合重音属于第 0 个簇");
        assert_eq!(model.cluster_index_at(2), Some(1));
        assert_eq!(model.cluster_index_at(99), None);
    }

    #[test]
    fn char_range_text_rejects_bad_ranges() {
        let model = TextModel::new("中文");
        let (start, end) = (2usize, 1usize);
        assert_eq!(model.char_range_text(start..end), None, "反向区间");
        assert_eq!(model.char_range_text(0..3), None, "越界");
        assert_eq!(model.char_range_text(0..2), Some("中文"));
    }
}
