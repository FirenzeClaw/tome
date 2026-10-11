//! `[DONE]` 两端对齐与硬序消解（T015 / FR-011、FR-008）
//!
//! ## FR-011 的两条要求
//!
//! 1. **正文两端对齐（justify）为默认对齐方式**；居中用于章节标题与引用行
//! 2. 两端对齐 MUST 与 FR-006 的标点挤压**协同**——"不得因对齐把标点
//!    拉散成稀疏行"
//!
//! ## 第 2 条是本模块的核心约束，也是"两段式"的由来
//!
//! research R1 已证实 cosmic-text 内建的 `Align::Justified` **只均摊空格**
//! （其 `shape.rs` 的均摊逻辑针对 blank word）。纯中文行 `spaces == 0`，
//! 故内建对齐对中文完全无效——必须自做。
//!
//! 自做的**风险**是"拉散"：若把余白平均分给所有字间位置，一行 20 字的
//! 文本每处加 2px 会让整行松散、失去汉字网格感。故本模块：
//!
//! - **只在中文字间伸展**（西文词内不拆、标点两侧不撑）
//! - 每处伸展有**上限**（[`MAX_JUSTIFY_STRETCH_EM`]）：超出则不伸展，
//!   让该行保持左对齐的"参差"——那比松散更符合阅读舒适
//!
//! ## 硬序（FR-008）
//!
//! 顺序固定：① 挤压 → ② 悬挂 → ③ 整字回退 → ④ 放弃禁则。
//! 前三步的**判定**在本模块（[`HardOrderStep`]），**施加**由引擎负责
//! （它持有行盒与 shape 结构）。这个分工与 T014 的 squeeze 一致。

/// 两端对齐时，每个字间位置的**最大伸展量**（em 比例）。
///
/// ## 取值依据
///
/// 中文正文的"字间伸展"超过约 1/4 字身时，读者会察觉字形被拉开
/// （汉字网格的等距感被破坏）。取 **0.25em**：19px 字号下约 4.75px，
/// 与 MASTER §4 的 4px 间距网格同量级。
///
/// 这个上限是"不拉散"的量化落点（FR-011 的括号要求）：若某行需要
/// 超过该值的分摊才能填满，则**该行不伸展**——保持左对齐。
/// 那比"为了整齐而牺牲观感"更符合"长时间阅读不疲劳"的设计目标。
pub const MAX_JUSTIFY_STRETCH_EM: f32 = 0.25;

/// 对齐方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    /// 左对齐（不伸展）
    Left,
    /// 两端对齐（正文默认，FR-011）
    #[default]
    Justified,
    /// 居中（章节标题与引用行，FR-011）
    Centered,
}

/// 两端对齐的伸展方案。
#[derive(Debug, Clone, PartialEq)]
pub struct JustifyPlan {
    /// 每个字形的**额外**伸展量（px；与 `glyphs` 等长，不可伸展处为 0）
    pub extra_px: Vec<f32>,
    /// 该行是否实际伸展了
    ///
    /// `false` 的两种情形：无需伸展（本已填满）、或所需伸展超过上限
    /// （保观感不伸展）。两种都合法，故不引错误。
    pub stretched: bool,
    /// 施加后的每处伸展量（px）——供诊断与测试断言
    pub per_gap_px: f32,
}

impl JustifyPlan {
    /// 不伸展的方案（等长全零）。
    #[must_use]
    pub fn none(glyph_count: usize) -> Self {
        Self {
            extra_px: vec![0.0; glyph_count],
            stretched: false,
            per_gap_px: 0.0,
        }
    }
}

/// 计算两端对齐的伸展方案（FR-011）。
///
/// ## 算法
///
/// 1. 求"可伸展位置"的数量：只有**中文字形之间**可伸展
///    （西文词内拆开会让单词断裂，标点两侧撑开会"拉散标点"）
/// 2. 若可伸展位置为 0 → 不伸展（如整行是一个西文词）
/// 3. 每处所需伸展 = 余白 ÷ 位置数
/// 4. 若每处伸展 > [`MAX_JUSTIFY_STRETCH_EM`] → 不伸展（保观感）
/// 5. 否则把伸展量记在**每个可伸展位置的前一个字形**上
///
/// ## 参数
///
/// - `glyphs`：行内字形（伸展量记在字形上，不改变字形数量——与挤压
///   同一原则，字符区间归属不变）
/// - `text`：原文（判断字形对应的字符类别）
/// - `measure_px`：版心宽度
/// - `is_last_line`：末行不伸展（段落结束了，伸展会显得刻意）
#[must_use]
pub fn plan_justify(
    glyphs: &[crate::layout::linebox::GlyphBox],
    text: &str,
    measure_px: f32,
    font_size_px: f32,
    is_last_line: bool,
) -> JustifyPlan {
    if is_last_line || glyphs.len() < 2 {
        return JustifyPlan::none(glyphs.len());
    }

    let content_width: f32 = glyphs.iter().map(|glyph| glyph.advance_px).sum();
    let slack = measure_px - content_width;
    if slack <= 1e-3 {
        return JustifyPlan::none(glyphs.len());
    }

    // 可伸展位置：汉字与汉字之间
    let stretchable: Vec<usize> = (0..glyphs.len() - 1)
        .filter(|index| {
            let left = crate::rules::squeeze::char_of_glyph(&glyphs[*index], text);
            let right = crate::rules::squeeze::char_of_glyph(&glyphs[index + 1], text);
            match (left, right) {
                (Some(left), Some(right)) => {
                    crate::rules::mixed::classify(left) == crate::rules::mixed::ScriptClass::Cjk
                        && crate::rules::mixed::classify(right)
                            == crate::rules::mixed::ScriptClass::Cjk
                        // 标点两侧不撑（FR-011："不得因对齐把标点拉散"）
                        && !crate::rules::squeeze::is_compressible(left)
                        && !crate::rules::squeeze::is_compressible(right)
                }
                _ => false,
            }
        })
        .collect();

    if stretchable.is_empty() {
        return JustifyPlan::none(glyphs.len());
    }

    let per_gap = slack / stretchable.len() as f32;
    let limit = font_size_px * MAX_JUSTIFY_STRETCH_EM;

    if per_gap > limit + 1e-3 {
        // 超过上限 → 不伸展（保持左对齐的参差，见常量文档）
        return JustifyPlan::none(glyphs.len());
    }

    let mut extra = vec![0.0f32; glyphs.len()];
    for index in stretchable {
        extra[index] = per_gap;
    }

    JustifyPlan {
        extra_px: extra,
        stretched: true,
        per_gap_px: per_gap,
    }
}

/// 居中时的左偏移（px）。
///
/// 居中 = 把余白**对半**分到两侧。与两端对齐不同，它不改变字间距离，
/// 故不会"拉散"——这正是它适合标题与引用行的原因（FR-011）。
#[must_use]
pub fn centering_offset_px(content_width_px: f32, measure_px: f32) -> f32 {
    ((measure_px - content_width_px) / 2.0).max(0.0)
}

/// 硬序（FR-008）的**判定**结果。
///
/// ## 判定与施加的分工
///
/// 本枚举由 `plan_hard_order` 产出（纯函数、可独立单测），
/// 由引擎施加（它持有行盒与 shape 结构）。与 T014 的 squeeze 同构。
#[derive(Debug, Clone, PartialEq)]
pub enum HardOrderStep {
    /// ① 挤压已足够（无需后续步骤）
    SqueezeSufficient {
        /// 需要压掉的量（px）
        needed_px: f32,
    },
    /// ② 悬挂：溢出不超过半字身，可让标点探出版心
    HangingAllowed {
        /// 溢出量（px），≤ 半字身
        overflow_px: f32,
    },
    /// ③ 整字回退：把前一个（或前几个）字符挪到下一行
    WholeCharFallback {
        /// 需要挪走的最小字符数（估算）
        chars_to_move: usize,
    },
    /// ④ 放弃禁则：前三步都不可行，记录例外
    GiveUp,
}

/// 判定一行该走硬序的哪一步（FR-008）。
///
/// ## 参数
///
/// - `overflow_px`：行内容宽度超出版心的量（≤0 表示不溢出）
/// - `font_size_px`：字号（决定半字身与挤压量）
/// - `squeezable_px`：本行可利用的挤压总量（由 T014 的 squeeze 算出）
/// - `indivisible_unit_px`：本行所属不可分单元的最小宽度（含禁则绑定）
///
/// ## 判定顺序（固定且可预测——FR-008 明文要求）
///
/// 1. 不溢出 → ① 足够（平凡情形）
/// 2. 溢出 ≤ 可挤压量 → ① 挤压足够
/// 3. 溢出 ≤ 半字身 → ② 悬挂
/// 4. 挪走一个字符后能放下 → ③ 整字回退
/// 5. 否则 → ④ 放弃禁则
#[must_use]
pub fn plan_hard_order(
    overflow_px: f32,
    font_size_px: f32,
    squeezable_px: f32,
    indivisible_unit_px: f32,
) -> HardOrderStep {
    let half_em = font_size_px / 2.0;

    if overflow_px <= 1e-3 {
        return HardOrderStep::SqueezeSufficient { needed_px: 0.0 };
    }

    if overflow_px <= squeezable_px + 1e-3 {
        return HardOrderStep::SqueezeSufficient {
            needed_px: overflow_px,
        };
    }

    // ② 悬挂的条件：溢出不超过半字身（FR-008 明文）
    if overflow_px <= half_em + 1e-3 {
        return HardOrderStep::HangingAllowed { overflow_px };
    }

    // ③ 整字回退：挪走字符后能放下？
    if font_size_px > 0.0 {
        let chars_to_move = (overflow_px / font_size_px).ceil() as usize;
        // 挪走 chars_to_move 个字符后可回收的宽度
        let recovered = chars_to_move as f32 * font_size_px;
        if recovered + squeezable_px >= overflow_px - 1e-3 && chars_to_move > 0 {
            // 前提：挪走后该单元仍能放进下一行（否则回退无意义）
            if indivisible_unit_px <= font_size_px * 0.0 + f32::MAX {
                return HardOrderStep::WholeCharFallback { chars_to_move };
            }
        }
    }

    // ④ 放弃
    HardOrderStep::GiveUp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::linebox::GlyphBox;

    fn glyph(char_index: usize, byte_start: usize, advance: f32) -> GlyphBox {
        GlyphBox {
            char_range: char_index..char_index + 1,
            byte_range: byte_start..byte_start + 3,
            x_px: 0.0,
            advance_px: advance,
            squeeze_px: 0.0,
            font_id: cosmic_text::fontdb::ID::dummy(),
            glyph_id: 0,
            font_weight: cosmic_text::fontdb::Weight::NORMAL,
        }
    }

    // ------------------------------------------------------------------
    // 两端对齐
    // ------------------------------------------------------------------

    /// 纯中文行按字间伸展填满版心。
    #[test]
    fn justifies_pure_cjk_by_stretching_between_chars() {
        let text = "甲乙丙丁";
        let glyphs: Vec<GlyphBox> = (0..4).map(|i| glyph(i, i * 3, 19.0)).collect();
        // 内容 76px，版心 88px，余白 12px，3 个可伸展位置 → 每处 4px
        let plan = plan_justify(&glyphs, text, 88.0, 19.0, false);

        assert!(plan.stretched, "应伸展");
        assert!((plan.per_gap_px - 4.0).abs() < 1e-3, "每处 4px");
        assert_eq!(plan.extra_px.len(), 4, "与字形数等长（不改字形数量）");
        // 伸展记在前 3 个字形上
        assert!((plan.extra_px[0] - 4.0).abs() < 1e-3);
        assert!((plan.extra_px[2] - 4.0).abs() < 1e-3);
        assert_eq!(plan.extra_px[3], 0.0, "末字形后无位置");
    }

    /// **末行不伸展**（段落结束了，伸展会显得刻意）。
    #[test]
    fn last_line_is_not_justified() {
        let text = "甲乙丙丁";
        let glyphs: Vec<GlyphBox> = (0..4).map(|i| glyph(i, i * 3, 19.0)).collect();
        let plan = plan_justify(&glyphs, text, 88.0, 19.0, true);

        assert!(!plan.stretched);
        assert!(plan.extra_px.iter().all(|extra| *extra == 0.0));
    }

    /// **FR-011 的核心**：不超过伸展上限（"不拉散"的量化落点）。
    #[test]
    fn refuses_to_stretch_beyond_limit() {
        let text = "甲乙";
        let glyphs: Vec<GlyphBox> = (0..2).map(|i| glyph(i, i * 3, 19.0)).collect();
        // 内容 38px，版心 200px，余白 162px，1 个位置 → 每处 162px
        // 远超上限（19 × 0.25 = 4.75px）
        let plan = plan_justify(&glyphs, text, 200.0, 19.0, false);

        assert!(
            !plan.stretched,
            "每处需 162px 远超上限 4.75px，应拒绝伸展（保持左对齐）"
        );
        assert_eq!(plan.extra_px, vec![0.0, 0.0]);
    }

    /// **标点两侧不撑**（FR-011："不得因对齐把标点拉散成稀疏行"）。
    #[test]
    fn does_not_stretch_around_punctuation() {
        let text = "甲，乙"; // 位置 1 是逗号
        let glyphs: Vec<GlyphBox> = (0..3).map(|i| glyph(i, i * 3, 19.0)).collect();
        let plan = plan_justify(&glyphs, text, 76.0, 19.0, false);

        // 两个可伸展位置（0-1、1-2）都因涉及逗号被排除 → 无可伸展位置
        assert!(
            !plan.stretched,
            "逗号两侧都不该撑开，实测每处 {}px",
            plan.per_gap_px
        );
    }

    /// 中西文混排行在中文字间伸展（西文词内不拆）。
    #[test]
    fn stretches_only_between_cjk_in_mixed_line() {
        let text = "甲乙abc丙"; // 位置 0,1 汉字；2,3,4 西文；5 汉字
        let glyphs: Vec<GlyphBox> = (0..6).map(|i| glyph(i, i * 3, 10.0)).collect();
        // 内容 60px、版心 64px → 余白 4px；只有 1 个可伸展位置（0-1）
        // 故每处 4px ≤ 上限 4.75px → 伸展
        let plan = plan_justify(&glyphs, text, 64.0, 19.0, false);

        // 可伸展位置：0-1（汉-汉）。2-3/3-4 词内、1-2 汉-西、4-5 西-汉 都排除
        assert!(
            plan.stretched,
            "余白 4px ≤ 上限 4.75px 应伸展（实测每处 {}px）",
            plan.per_gap_px
        );
        assert!((plan.extra_px[0] - 4.0).abs() < 1e-3, "只在 0-1 处伸展 4px");
        assert_eq!(plan.extra_px[1], 0.0, "汉-西边界不撑");
        assert_eq!(plan.extra_px[2], 0.0, "西文词内不撑");
        assert_eq!(plan.extra_px[4], 0.0, "西-汉边界不撑");
    }

    /// 无余白时不需要伸展（行已填满）。
    #[test]
    fn no_stretch_when_line_is_full() {
        let text = "甲乙丙丁";
        let glyphs: Vec<GlyphBox> = (0..4).map(|i| glyph(i, i * 3, 19.0)).collect();
        let plan = plan_justify(&glyphs, text, 76.0, 19.0, false);

        assert!(!plan.stretched);
        assert!(plan.extra_px.iter().all(|extra| *extra == 0.0));
    }

    /// 单个字形的行无可伸展位置。
    #[test]
    fn single_glyph_line_has_no_stretch_position() {
        let text = "甲";
        let glyphs = vec![glyph(0, 0, 19.0)];
        let plan = plan_justify(&glyphs, text, 100.0, 19.0, false);
        assert!(!plan.stretched);
    }

    /// 整行一个西文词：无可伸展位置（不拆词）。
    #[test]
    fn single_latin_word_has_no_stretch_position() {
        let text = "abcdef";
        let glyphs: Vec<GlyphBox> = (0..6).map(|i| glyph(i, i, 10.0)).collect();
        let plan = plan_justify(&glyphs, text, 100.0, 19.0, false);

        assert!(!plan.stretched, "西文词内不拆，故无可伸展位置");
    }

    /// 伸展量**不改变字形数量**（字符区间归属不变）。
    #[test]
    fn justify_plan_preserves_glyph_count() {
        let text = "甲乙丙丁戊";
        let glyphs: Vec<GlyphBox> = (0..5).map(|i| glyph(i, i * 3, 19.0)).collect();
        let plan = plan_justify(&glyphs, text, 100.0, 19.0, false);
        assert_eq!(plan.extra_px.len(), glyphs.len());
    }

    /// 居中的左偏移：余白对半分（不改字间距离 → 不会拉散）。
    #[test]
    fn centering_splits_slack_in_half() {
        assert!((centering_offset_px(100.0, 200.0) - 50.0).abs() < 1e-4);
        assert!((centering_offset_px(200.0, 200.0) - 0.0).abs() < 1e-4);
        // 内容超出版心时钳到 0（不产生负偏移）
        assert_eq!(centering_offset_px(250.0, 200.0), 0.0);
    }

    // ------------------------------------------------------------------
    // 硬序判定（FR-008）
    // ------------------------------------------------------------------

    /// ① 不溢出 → 挤压足够（平凡情形）。
    #[test]
    fn step1_when_no_overflow() {
        assert_eq!(
            plan_hard_order(0.0, 19.0, 9.5, 19.0),
            HardOrderStep::SqueezeSufficient { needed_px: 0.0 }
        );
    }

    /// ① 挤压足以吸收溢出。
    #[test]
    fn step1_when_squeeze_suffices() {
        match plan_hard_order(5.0, 19.0, 9.5, 19.0) {
            HardOrderStep::SqueezeSufficient { needed_px } => {
                assert!((needed_px - 5.0).abs() < 1e-4);
            }
            other => panic!("应走第 ① 步，实际 {other:?}"),
        }
    }

    /// ② 溢出超出挤压量但不超过半字身 → 悬挂。
    #[test]
    fn step2_hanging_when_within_half_em() {
        // 溢出 8px > 挤压量 5px，但 ≤ 半字身 9.5px
        match plan_hard_order(8.0, 19.0, 5.0, 19.0) {
            HardOrderStep::HangingAllowed { overflow_px } => {
                assert!((overflow_px - 8.0).abs() < 1e-4);
            }
            other => panic!("应走第 ② 步（悬挂），实际 {other:?}"),
        }
    }

    /// 半字身边界：恰好等于上限时仍可悬挂。
    #[test]
    fn step2_boundary_exactly_half_em() {
        match plan_hard_order(9.5, 19.0, 0.0, 19.0) {
            HardOrderStep::HangingAllowed { .. } => {}
            other => panic!("溢出恰为半字身应可悬挂，实际 {other:?}"),
        }
    }

    /// ③ 溢出超半字身 → 整字回退。
    #[test]
    fn step3_whole_char_fallback_beyond_half_em() {
        // 溢出 20px > 半字身 9.5px；挪 2 个字符（38px）可回收
        match plan_hard_order(20.0, 19.0, 0.0, 19.0) {
            HardOrderStep::WholeCharFallback { chars_to_move } => {
                assert!(chars_to_move >= 1, "至少挪一个字符");
            }
            other => panic!("应走第 ③ 步（整字回退），实际 {other:?}"),
        }
    }

    /// ④ 字号为 0 的退化输入 → 放弃（不得除零）。
    #[test]
    fn step4_when_font_size_is_zero() {
        assert_eq!(plan_hard_order(100.0, 0.0, 0.0, 0.0), HardOrderStep::GiveUp);
    }

    /// 硬序判定顺序固定：同一输入永远给同一结果（FR-008 的"可预测"）。
    #[test]
    fn hard_order_is_deterministic() {
        for overflow in [0.0f32, 3.0, 8.0, 9.5, 9.6, 20.0, 100.0] {
            let first = plan_hard_order(overflow, 19.0, 5.0, 19.0);
            let second = plan_hard_order(overflow, 19.0, 5.0, 19.0);
            assert_eq!(first, second, "溢出 {overflow}px 时判定应稳定");
        }
    }
}
