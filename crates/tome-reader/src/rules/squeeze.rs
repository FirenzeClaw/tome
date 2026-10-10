//! [DONE] 标点挤压（T014 / FR-006）
//!
//! ## FR-006 的三条硬约束
//!
//! 1. **优先压缩标点字身**而非增删整行字符间距
//! 2. 压缩 MUST 有**命名常量下限**（不得压到不可辨识）
//! 3. 压缩 MUST NOT 改变**字符的顺序或字符区间归属**
//!
//! 第 3 条是本模块的实现约束来源：挤压只能改写 `GlyphBox::advance_px`
//! （以及随之的 `x_px`），**不得**合并、删除、重排任何字形。这样"痕迹的
//! 字符区间"在任何挤压量下都保持成立（001 §FR-028 的前提）。
//!
//! ## 挤压的度量口径（本模块的定义）
//!
//! 一个标点字形的**天然字身**（full width）是 1 个 em（中文全角标点）。
//! 挤压把它压到 [`MIN_SQUEEZE_RATIO`] × em 以上。故：
//!
//! ```text
//! 可压缩量 = 天然字身 - 压缩下限 = em × (1 - MIN_SQUEEZE_RATIO)
//! ```
//!
//! 19px 字号下：`可压缩量 = 19 × (1 - 0.5) = 9.5px`（半字身）。
//! 这与项目语料（`tests/corpus/kinsoku.json` 的 `squeeze` 用例）以及
//! CLreq §3.1.5 的"行末标点占半字身"一致。
//!
//! ## 为什么用"比例"而非"固定 px"
//!
//! FR-010 与 MASTER §5.1 都要求尺寸随字号缩放、禁硬编码像素。挤压下限
//! 是几何量，若写成 `-9.5px` 则在 24px 字号下只压掉 40%（观感不一致），
//! 在 14px 下压掉 68%（可能不可辨识）。比例制让压缩量随字号等比。

/// 标点可被压缩到的**最小比例**（相对天然字身）。
///
/// ## 取值依据（这是 FR-006 要求的"命名常量下限"）
///
/// **0.5（半字身）**，依据 CLreq §3.1.5「标点挤压」— 中文排版惯例里
/// 行末标点的最紧形态是半字身。取更小（如 0.25）会让标点退化成一个
/// 难以辨认的窄条；取更大（如 0.75）则压缩收益太小、不足以解决
/// 常见的"差一点放不下"。
///
/// 这个值同时与 `tests/corpus/kinsoku.json` 的 `squeeze` 用例期望一致
/// （19px 字号下压缩 9.5px = 半字身）——语料与常量是同一口径的两处表达。
pub const MIN_SQUEEZE_RATIO: f32 = 0.5;

/// 可压缩的标点字符集（FR-006 的适用范围）。
///
/// ## 依据与范围
///
/// CLreq §3.1.5 列出的可压缩标点是**行末/行首的停顿类与括号类**。
/// 不含：
///
/// - 破折号（U+2014）与省略号（U+2026）：它们已占两个字身，压缩会破坏
///   "占两字"的观感约定（CLreq §3.1.2）
/// - 引号的前后半个：它们常与文字紧邻，压缩会挤压字距
///
/// 该集合是**显式枚举**（FR-007 的"显式清单"要求）——不用"是标点就压"
/// 这类宽泛判据，因为那会把不应压缩的字符也压掉。
pub const COMPRESSIBLE_PUNCTUATION: &str = "，。、；：？！）］｝〉》」』】〗〙〛”’";

/// 一个可压缩字形的挤压决策。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SqueezeDecision {
    /// 期望压掉的量（px，**未施加**）
    ///
    /// 调用方须把它钳到 `[0, 可压缩上限]` 之间再施加——本类型只表达
    /// "想压多少"，不表达"实际压了多少"。这样"被几何限制"与"被下限限制"
    /// 两种情形在调用方可见。
    pub desired_px: f32,
    /// 该字形**最多**可压多少（px，由 [`MIN_SQUEEZE_RATIO`] 与字身决定）
    pub max_px: f32,
}

impl SqueezeDecision {
    /// 实际可施加的压缩量（`desired` 被上限钳制后的值）。
    #[must_use]
    pub fn effective_px(&self) -> f32 {
        self.desired_px.clamp(0.0, self.max_px)
    }
}

/// 判断某字符是否可压缩。
#[must_use]
pub fn is_compressible(ch: char) -> bool {
    COMPRESSIBLE_PUNCTUATION.contains(ch)
}

/// 某字形在其字号下的**可压缩上限**（px）。
///
/// 天然字身按 `font_size_px`（全角标点 = 1em），下限为
/// `MIN_SQUEEZE_RATIO × font_size_px`。
#[must_use]
pub fn max_compression_px(font_size_px: f32) -> f32 {
    font_size_px * (1.0 - MIN_SQUEEZE_RATIO)
}

/// 为一行计算挤压方案：把 `overflow_px` 的超出量分摊给可压缩标点。
///
/// ## 参数
///
/// - `glyphs`：行内字形（按顺序）
/// - `text`：原文（用于查每个字形对应的字符）
/// - `overflow_px`：需要压掉的量（>0 表示超出版心）
/// - `font_size_px`：字号（决定压缩上限）
/// - `prefer_line_end`：优先压缩行末（FR-006 说"行末或行首"，
///   行末优先是中文排版的惯例——行末标点更常见）
///
/// ## 返回
///
/// 逐个可压缩字形的决策（与 `glyphs` 同序，不可压缩者为 `None`）。
///
/// ## 分摊策略
///
/// **从优先端开始，逐个压满再转移到下一个**，直到满足 `overflow_px`
/// 或所有可压缩字形都用尽。这是"最小的改动量"策略：它把压缩集中在
/// 最少的标点上，而非把每处的压力分散（后者会让整行观感均匀地变紧，
/// 反而更明显）。
#[must_use]
pub fn plan_squeeze(
    glyphs: &[crate::layout::linebox::GlyphBox],
    text: &str,
    overflow_px: f32,
    font_size_px: f32,
    prefer_line_end: bool,
) -> Vec<Option<SqueezeDecision>> {
    let mut plan: Vec<Option<SqueezeDecision>> = vec![None; glyphs.len()];

    if overflow_px <= 0.0 {
        return plan;
    }

    let max_px = max_compression_px(font_size_px);
    if max_px <= 0.0 {
        return plan;
    }

    // 确定处理顺序：优先端在前
    let order: Vec<usize> = if prefer_line_end {
        (0..glyphs.len()).rev().collect()
    } else {
        (0..glyphs.len()).collect()
    };

    let mut remaining = overflow_px;

    for index in order {
        if remaining <= 1e-4 {
            break;
        }
        let glyph = &glyphs[index];
        let Some(ch) = char_of_glyph(glyph, text) else {
            continue;
        };
        if !is_compressible(ch) {
            continue;
        }

        let desired = remaining.min(max_px);
        plan[index] = Some(SqueezeDecision {
            desired_px: desired,
            max_px,
        });
        remaining -= desired;
    }

    plan
}

/// 取字形对应的字符（取其区间的首字符）。
#[must_use]
pub fn char_of_glyph(glyph: &crate::layout::linebox::GlyphBox, text: &str) -> Option<char> {
    text.get(glyph.byte_range.clone())?.chars().next()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::linebox::GlyphBox;

    fn glyph(char_start: usize, byte_start: usize, advance: f32) -> GlyphBox {
        GlyphBox {
            char_range: char_start..char_start + 1,
            byte_range: byte_start..byte_start + 3,
            x_px: 0.0,
            advance_px: advance,
            squeeze_px: 0.0,
            font_id: cosmic_text::fontdb::ID::dummy(),
            glyph_id: 0,
            font_weight: cosmic_text::fontdb::Weight::NORMAL,
        }
    }

    /// 半字身是压缩下限（19px 字号 → 9.5px）。
    #[test]
    fn max_compression_is_half_em_at_19px() {
        let max_px = max_compression_px(19.0);
        assert!(
            (max_px - 9.5).abs() < 1e-4,
            "19px 字号下可压缩上限应为 9.5px（半字身），实测 {max_px}"
        );
    }

    /// 压缩量随字号等比（禁硬编码像素——FR-010 的同源要求）。
    #[test]
    fn max_compression_scales_with_font_size() {
        for font_size in [14.0f32, 19.0, 24.0, 30.0] {
            let expected = font_size * 0.5;
            assert!(
                (max_compression_px(font_size) - expected).abs() < 1e-4,
                "{font_size}px 字号的压缩上限应为 {expected}px"
            );
        }
    }

    #[test]
    fn compressible_set_is_explicit() {
        for ch in "，。、；：？！".chars() {
            assert!(is_compressible(ch), "`{ch}` 应可压缩");
        }
        // 破折号与省略号**不可**压缩（已占两字身，CLreq §3.1.2）
        assert!(!is_compressible('\u{2014}'), "破折号占两字身，不压缩");
        assert!(!is_compressible('\u{2026}'), "省略号占两字身，不压缩");
        // 汉字与西文不可压缩
        assert!(!is_compressible('甲'));
        assert!(!is_compressible('a'));
    }

    /// 无溢出时不做任何挤压（不引入无谓的改动）。
    #[test]
    fn no_overflow_means_no_squeeze() {
        let text = "甲乙，丙";
        let glyphs = vec![
            glyph(0, 0, 19.0),
            glyph(1, 3, 19.0),
            glyph(2, 6, 19.0),
            glyph(3, 9, 19.0),
        ];
        let plan = plan_squeeze(&glyphs, text, 0.0, 19.0, true);

        assert!(
            plan.iter().all(Option::is_none),
            "无溢出时不应有任何挤压决策：{plan:?}"
        );
    }

    /// 溢出时优先压缩**行末**标点（FR-006 的"行末或行首"）。
    #[test]
    fn prefers_line_end_punctuation() {
        let text = "甲，乙。"; // 位置 1 是逗号，位置 3 是句号
        let glyphs = vec![
            glyph(0, 0, 19.0),
            glyph(1, 3, 19.0),
            glyph(2, 6, 19.0),
            glyph(3, 9, 19.0),
        ];
        let plan = plan_squeeze(&glyphs, text, 5.0, 19.0, true);

        assert!(plan[3].is_some(), "行末的句号应优先被压缩，实际 {plan:?}");
        assert!(plan[1].is_none(), "溢出量 5px 只要一处就够，逗号不该被动");
    }

    /// 优先端不够时溢出到下一个可压缩标点。
    #[test]
    fn spills_to_next_compressible_when_needed() {
        let text = "甲，乙。";
        let glyphs = vec![
            glyph(0, 0, 19.0),
            glyph(1, 3, 19.0),
            glyph(2, 6, 19.0),
            glyph(3, 9, 19.0),
        ];
        // 溢出 12px > 单处上限 9.5px → 需要第二个标点
        let plan = plan_squeeze(&glyphs, text, 12.0, 19.0, true);

        let period = plan[3].expect("句号应被压缩");
        assert!((period.effective_px() - 9.5).abs() < 1e-4, "句号压满 9.5px");
        let comma = plan[1].expect("逗号应被压缩（剩余 2.5px）");
        assert!(
            (comma.effective_px() - 2.5).abs() < 1e-4,
            "逗号应承担剩余 2.5px，实测 {}",
            comma.effective_px()
        );
    }

    /// 每个字形的压缩量**不得超过上限**（FR-006 的命名常量下限）。
    #[test]
    fn never_exceeds_compression_limit() {
        let text = "，，，，，";
        let glyphs: Vec<GlyphBox> = (0..5).map(|i| glyph(i, i * 3, 19.0)).collect();
        // 溢出量远超总量（5 × 9.5 = 47.5）
        let plan = plan_squeeze(&glyphs, text, 200.0, 19.0, true);

        for (index, decision) in plan.iter().enumerate() {
            if let Some(decision) = decision {
                assert!(
                    decision.effective_px() <= decision.max_px + 1e-4,
                    "字形 {index} 的压缩量 {} 超过上限 {}",
                    decision.effective_px(),
                    decision.max_px
                );
                assert!(
                    decision.effective_px() > 0.0,
                    "字形 {index} 的决策量为 0（不应产生决策）"
                );
            }
        }
    }

    /// 不可压缩的字形**永远**不产生决策（哪怕溢出量很大）。
    #[test]
    fn non_compressible_glyphs_never_get_decisions() {
        let text = "甲乙丙"; // 无标点
        let glyphs: Vec<GlyphBox> = (0..3).map(|i| glyph(i, i * 3, 19.0)).collect();
        let plan = plan_squeeze(&glyphs, text, 100.0, 19.0, true);

        assert!(
            plan.iter().all(Option::is_none),
            "无标点的行不该有任何挤压决策，实际 {plan:?}"
        );
    }

    /// 挤压计划只描述**字身改写**，不改动字形序列本身（FR-006 第 3 条）。
    ///
    /// 这条是"字符区间归属不变"的静态保证：`plan_squeeze` 只读入参、
    /// 产出等长数组，故它**无法**改变顺序或区间——那由类型形状保证。
    #[test]
    fn squeeze_plan_preserves_glyph_count_and_order() {
        let text = "甲，乙。丙";
        let glyphs: Vec<GlyphBox> = (0..5).map(|i| glyph(i, i * 3, 19.0)).collect();
        let plan = plan_squeeze(&glyphs, text, 20.0, 19.0, true);
        assert_eq!(
            plan.len(),
            glyphs.len(),
            "挤压计划必须与字形序列等长（顺序不得改变）"
        );
    }

    /// 负溢出量（版心有富余）不触发挤压。
    #[test]
    fn negative_overflow_means_no_squeeze() {
        let text = "甲，乙";
        let glyphs: Vec<GlyphBox> = (0..3).map(|i| glyph(i, i * 3, 19.0)).collect();
        let plan = plan_squeeze(&glyphs, text, -5.0, 19.0, true);
        assert!(plan.iter().all(Option::is_none));
    }

    /// 字号为 0 的退化输入不产生上限（不得除零或产生 inf）。
    #[test]
    fn zero_font_size_yields_zero_limit() {
        assert_eq!(max_compression_px(0.0), 0.0);
        let text = "甲，";
        let glyphs: Vec<GlyphBox> = (0..2).map(|i| glyph(i, i * 3, 0.0)).collect();
        let plan = plan_squeeze(&glyphs, text, 10.0, 0.0, true);
        assert!(plan.iter().all(Option::is_none), "字号为 0 时无可压缩空间");
    }

    /// `prefer_line_end = false` 时从行首开始压（FR-006 也允许行首）。
    #[test]
    fn can_prefer_line_start() {
        let text = "，乙。";
        let glyphs: Vec<GlyphBox> = (0..3).map(|i| glyph(i, i * 3, 19.0)).collect();
        let plan = plan_squeeze(&glyphs, text, 5.0, 19.0, false);

        assert!(plan[0].is_some(), "行首的逗号应被压，实际 {plan:?}");
        assert!(plan[2].is_none(), "5px 只要一处就够");
    }
}
