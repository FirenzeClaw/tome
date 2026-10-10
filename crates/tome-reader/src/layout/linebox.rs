//! [DONE] 行盒（T013 / data-model.md §4）
//!
//! `LineBox` 是排版核心的**输出单元**：它把"一行文字"表达为字符区间 +
//! 字形位置 + 行盒度量。分页（T019）、滚动（T025）、痕迹渲染（T042）、
//! 命中测试（T043）全部消费它。
//!
//! ## data-model.md §4 的七字段（逐一对齐）
//!
//! | 字段 | 本模块 | 说明 |
//! |---|---|---|
//! | `para_index` | [`LineBox::paragraph_index`] | 所属段 |
//! | `char_range` | [`LineBox::char_range`] | 行内**字符**区间（不是字节） |
//! | `glyphs` | [`LineBox::glyphs`] | 行内字形及相对位置 |
//! | `height` | [`LineBox::height_px`] | 行高 |
//! | `baseline` | [`LineBox::baseline_px`] | 基线 |
//! | `squeeze_amount` | [`LineBox::squeeze_px`] | 本行标点压缩总量 |
//! | `rule_exception` | [`LineBox::rule_exception`] | 硬序第 ④ 步的记录 |
//!
//! ## 为什么 `char_range` 是字符区间而非字节
//!
//! 这层的输出会被 S1 的痕迹系统消费（`Highlight.char_offset` 是**字符**偏移）。
//! 若这里用字节，跨子项目的换算会出现一处看不见的偏移——含 emoji 的段落
//! 直接错位。字节区间只在与 cosmic-text 对接时短暂出现（[`GlyphBox::byte_range`]）。

use std::ops::Range;

/// 行内一个字形的位置与归属。
///
/// ## 三层区间的关系（本类型是三者交汇处）
///
/// - [`Self::byte_range`]：cosmic-text 给的**字节**区间（对接用）
/// - [`Self::char_range`]：换算后的**字符**区间（对外口径）
/// - [`Self::x_px`]：行内水平位置（渲染坐标——但**不用于持久化位置**，
///   001 §FR-028 明文禁止把痕迹存成渲染坐标）
#[derive(Debug, Clone, PartialEq)]
pub struct GlyphBox {
    /// 该字形覆盖的**字符**区间（段内偏移，不含段起始）
    pub char_range: Range<usize>,
    /// 该字形在原文中的**字节**区间（仅供与 cosmic-text 对齐，不对外）
    pub byte_range: Range<usize>,
    /// 行内水平起点（px，相对行盒左边缘）
    pub x_px: f32,
    /// 该字形的步进宽度（px）——被挤压改写过的值
    pub advance_px: f32,
    /// 被施加的压缩量（px，0 表示未压缩）
    ///
    /// 单字形级别的压缩量，与 [`LineBox::squeeze_px`]（行级总量）是
    /// "明细与汇总"的关系——两者必须一致（有测试断言）。
    pub squeeze_px: f32,
}

/// 硬序第 ④ 步（放弃禁则）的例外记录（FR-008）。
///
/// ## 为什么必须有这个记录
///
/// FR-008 允许在几何条件不允许时放弃禁则——但要求**留痕**。理由是
/// SC-002 的判定方式："违反次数为 0（**允许的例外必须落进 FR-008
/// 第 ④ 步的例外记录并可计数**）"。没有这个记录，SC-002 无法区分
/// "实现正确所以没有违反"与"实现根本没做禁则所以没有违反"。
///
/// 这也是 001 §FR-006 的"能力错误必须结构化"的同一种思路：
/// 失败路径要可见，不能与成功路径长得一样。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleExceptionRecord {
    /// 触发例外的**字符位置**（段内偏移；`None` 表示段落级例外）
    pub char_index: usize,
    /// 被放弃的规则标识（取自 [`crate::rules::cjk::KINSOKU_RULES`] 的 id）
    pub rule_id: &'static str,
    /// 硬序走到了哪一步（用于说明"为什么前三步都不行"）
    pub reached_step: HardOrderStep,
}

/// FR-008 的硬序步骤。
///
/// 顺序固定且可预测（①②③④），本枚举的**声明顺序即优先级顺序**——
/// 用 `#[derive(Ord)]` 让它可比较，从而"第几步"在类型上可见。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HardOrderStep {
    /// ① 标点挤压（在压缩下限内）
    Squeeze,
    /// ② 行末标点悬挂（超出容器不超过半个字身）
    Hanging,
    /// ③ 整字回退（把前一个字符挪到下行）
    WholeCharFallback,
    /// ④ 放弃禁则（并记录例外）
    GiveUpKinsoku,
}

/// 一行排好版的结果（data-model.md §4 的七字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    /// 所属段序号（data-model §4 `para_index`）
    pub paragraph_index: usize,
    /// 行内**字符**区间（段内偏移；data-model §4 `char_range`）
    pub char_range: Range<usize>,
    /// 行内字形序列（data-model §4 `glyphs`）
    pub glyphs: Vec<GlyphBox>,
    /// 行高（px；data-model §4 `height`）
    pub height_px: f32,
    /// 基线相对行盒顶的距离（px；data-model §4 `baseline`）
    pub baseline_px: f32,
    /// 本行标点压缩**总量**（px；data-model §4 `squeeze_amount`）
    pub squeeze_px: f32,
    /// 本行两端对齐的**伸展总量**（px；0 表示未伸展）
    ///
    /// 与 [`Self::squeeze_px`] 是"一收一放"的关系：挤压把溢出的行收进版心，
    /// 对齐把不满的行撑满。两者都在行定案后施加，且都只改字身不改区间归属。
    ///
    /// data-model §4 的七字段里没有它——那是设计时未预见对齐需要独立记账。
    /// 保留此字段的理由：`content_width_px()` 是"字身之和"，而"视觉行长"
    /// 是 `字身之和 + 伸展`；没有它就无法从行盒判断"这行是否已对齐"。
    pub justify_extra_px: f32,
    /// 硬序第 ④ 步的例外记录（data-model §4 `rule_exception`）
    pub rule_exception: Option<RuleExceptionRecord>,
}

impl LineBox {
    /// 行的字符长度。
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.char_range.end - self.char_range.start
    }

    /// 行内容宽度（px）——各字形步进之和，即"压缩后的实际宽度"。
    ///
    /// ## 为什么是求和而不是"末字形右边缘 - 首字形左边缘"
    ///
    /// 挤压会改写单个字形的 `advance_px`，两者在**压缩后**应当一致；
    /// 但若实现有 bug（如忘了更新 `x_px`），两种算法会分叉。
    /// 本方法取求和（最直接的定义），并另有测试断言两种算法一致——
    /// 那个断言就是"挤压改写是否自洽"的判据。
    #[must_use]
    pub fn content_width_px(&self) -> f32 {
        self.glyphs.iter().map(|glyph| glyph.advance_px).sum()
    }

    /// 行内字形压缩量之和——必须等于 [`Self::squeeze_px`]。
    #[must_use]
    pub fn summed_glyph_squeeze_px(&self) -> f32 {
        self.glyphs.iter().map(|glyph| glyph.squeeze_px).sum()
    }

    /// 是否含例外记录（SC-002 的计数口径）。
    #[must_use]
    pub fn has_exception(&self) -> bool {
        self.rule_exception.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyph(char_start: usize, byte_start: usize, advance: f32, squeeze: f32) -> GlyphBox {
        GlyphBox {
            char_range: char_start..char_start + 1,
            byte_range: byte_start..byte_start + 3,
            x_px: 0.0,
            advance_px: advance,
            squeeze_px: squeeze,
        }
    }

    #[test]
    fn content_width_is_sum_of_advances() {
        let line = LineBox {
            paragraph_index: 0,
            char_range: 0..3,
            glyphs: vec![
                glyph(0, 0, 19.0, 0.0),
                glyph(1, 3, 9.5, 9.5),
                glyph(2, 6, 19.0, 0.0),
            ],
            height_px: 35.15,
            baseline_px: 28.0,
            squeeze_px: 9.5,
            justify_extra_px: 0.0,
            rule_exception: None,
        };

        assert!((line.content_width_px() - 47.5).abs() < 1e-4);
        assert_eq!(line.char_len(), 3);
        assert!(!line.has_exception());
    }

    /// 行级压缩量必须等于各字形压缩量之和（明细与汇总一致）。
    #[test]
    fn line_squeeze_matches_glyph_squeeze_sum() {
        let line = LineBox {
            paragraph_index: 1,
            char_range: 5..8,
            glyphs: vec![
                glyph(5, 15, 19.0, 0.0),
                glyph(6, 18, 9.5, 9.5),
                glyph(7, 21, 14.25, 4.75),
            ],
            height_px: 35.15,
            baseline_px: 28.0,
            squeeze_px: 14.25,
            justify_extra_px: 0.0,
            rule_exception: None,
        };

        assert!(
            (line.squeeze_px - line.summed_glyph_squeeze_px()).abs() < 1e-4,
            "行级压缩量 {} 与字形明细之和 {} 不符",
            line.squeeze_px,
            line.summed_glyph_squeeze_px()
        );
    }

    /// 硬序步骤的**声明顺序即优先级**（FR-008 的"顺序固定且可预测"）。
    #[test]
    fn hard_order_steps_are_ordered() {
        assert!(HardOrderStep::Squeeze < HardOrderStep::Hanging);
        assert!(HardOrderStep::Hanging < HardOrderStep::WholeCharFallback);
        assert!(HardOrderStep::WholeCharFallback < HardOrderStep::GiveUpKinsoku);
    }

    #[test]
    fn exception_record_carries_char_index_and_rule() {
        let record = RuleExceptionRecord {
            char_index: 42,
            rule_id: "kinsoku.line_start.comma_full_width",
            reached_step: HardOrderStep::GiveUpKinsoku,
        };
        assert_eq!(record.char_index, 42);
        assert!(record.rule_id.starts_with("kinsoku."));
        assert_eq!(record.reached_step, HardOrderStep::GiveUpKinsoku);
    }
}
