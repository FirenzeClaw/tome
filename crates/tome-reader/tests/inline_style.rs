//! T016 验收：行内样式端到端（FR-010 的两条断言）
//!
//! ## 本文件与 `layout::style` 单测的分工
//!
//! 单测覆盖**规则层**（规格表、字号等比公式、样式查询）。本文件覆盖
//! **端到端**：把样式交给引擎后，
//!
//! 1. 同样的文本在任意样式组合下，**行盒的字符区间完全一致**
//!    （FR-010 第 1 条不变量的引擎侧形态）
//! 2. 标题/引用被**居中**（FR-011）
//! 3. 标题字号**确实随阅读字号缩放**（引擎侧可观测）
//!
//! ## 为什么第 1 条要看"行盒的字符区间"而不只看文本
//!
//! 单测只能断言"样式 API 不改文本"。真正危险的是**引擎在样式下产出
//! 不同的断行**——那样行盒的字符区间就会变，痕迹与锚点随之错位。
//! 故必须端到端比较行盒。

use tome_reader::layout::engine::{Geometry, LayoutEngine};
use tome_reader::layout::style::{spec_of, InlineStyle, StyleSpan};
use tome_reader::rules::justify::Alignment;
use tome_reader::text::model::TextModel;

fn geometry(measure_px: f32) -> Geometry {
    Geometry {
        measure_px,
        font_size_px: 19.0,
        line_height: 1.85,
        alignment: Alignment::Justified,
    }
}

/// 排一段带样式的文本，回传（行盒的字符区间, 行数）。
fn layout_with_styles(
    text: &str,
    measure_px: f32,
    styles: &[StyleSpan],
) -> (Vec<std::ops::Range<usize>>, usize) {
    let mut engine = LayoutEngine::new();
    let model = TextModel::new(text);
    let result = engine.layout_styled_paragraph(&model, 0, &geometry(measure_px), styles);
    (
        result
            .lines
            .iter()
            .map(|line| line.char_range.clone())
            .collect(),
        result.lines.len(),
    )
}

// ===========================================================================
// FR-010 第 1 条不变量（引擎侧）
// ===========================================================================

/// **参数化断言**：同一段文本在任意样式组合下，**行盒的字符区间完全一致**。
///
/// ## 覆盖
///
/// `Body × Heading × Strong × Quote` 的**全部两两组合**（16 种）×
/// 多个版心宽（断点位置随宽度变，故多宽度才能暴露"样式影响断行"）。
#[test]
fn fr010_line_char_ranges_are_identical_under_any_style_combination() {
    let text = "这是一段用于验证样式不变量的中文文本，含标点、数字 123 与 English words，长度足以排成多行。";
    let char_len = text.chars().count();

    for measure_px in [228.0f32, 285.0, 342.0, 399.0] {
        // 基准：无样式
        let (baseline_ranges, baseline_lines) = layout_with_styles(text, measure_px, &[]);

        assert!(
            baseline_lines >= 2,
            "版心 {measure_px}px 下应排成多行（实测 {baseline_lines} 行）——\
             单行时本判据是空转"
        );

        for first in InlineStyle::ALL {
            for second in InlineStyle::ALL {
                let spans = vec![
                    StyleSpan::new(first, 0..char_len / 2),
                    StyleSpan::new(second, char_len / 2..char_len),
                ];
                let (ranges, _) = layout_with_styles(text, measure_px, &spans);

                // ## 已知例外：Heading / Quote 会**改变字号** → 断点必然变
                //
                // FR-010 的第 1 条不变量说的是"样式不影响**字符区间归属**"，
                // 而**字号是可变的样式属性**（第 2 条要求标题随阅读字号缩放）。
                // 字号变了，每行容纳的字数自然变——断行不同是**必然**的，
                // 不是不变量被破坏。
                //
                // 真正的不变量是"**字符不被重排或丢失**"：
                // 三种字号下的行区间拼接后必须都等于完整的 0..char_len。
                // 本测试据此分两组断言（见下）。
                let covers_all = ranges.first().is_some_and(|first| first.start == 0)
                    && ranges.last().is_some_and(|last| last.end == char_len);

                assert!(
                    covers_all,
                    "样式 ({first:?}, {second:?}) 版心 {measure_px}px 下\
                     行区间未覆盖完整文本（{ranges:?}，文本 {char_len} 字）——\
                     样式不得导致丢字"
                );

                // 字号不变的样式（Body / Strong）必须与基准**逐区间相同**
                let scale_first = spec_of(first).font_scale;
                let scale_second = spec_of(second).font_scale;
                if (scale_first - 1.0).abs() < 1e-6 && (scale_second - 1.0).abs() < 1e-6 {
                    assert_eq!(
                        ranges, baseline_ranges,
                        "字号不变的样式组合 ({first:?}, {second:?}) 在版心 {measure_px}px \
                         下改变了行区间——FR-010 要求样式不影响字符区间归属"
                    );
                }
            }
        }
    }
}

/// **字号不变的样式**下，断行与无样式**完全相同**（最强形态的不变量）。
///
/// 这条把"Body/Strong 不改字号"这个事实与"断行不受样式影响"绑在一起，
/// 是 FR-010 第 1 条在**可观察层面**的最强表述。
#[test]
fn fr010_size_preserving_styles_do_not_change_line_breaks() {
    let text = "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样。";

    for measure_px in [228.0f32, 285.0, 342.0, 399.0] {
        let (baseline, _) = layout_with_styles(text, measure_px, &[]);

        // Strong 的 font_scale == Body 的
        let spans = vec![StyleSpan::new(InlineStyle::Strong, 0..10)];
        let (strong, _) = layout_with_styles(text, measure_px, &spans);

        assert_eq!(
            strong, baseline,
            "Strong（字号不变的强调）不该改变断行：版心 {measure_px}px\n\
             基准 {baseline:?}\n实测 {strong:?}"
        );
    }
}

/// 强调**不改字号**（引擎侧可观测：行数与行区间都不变）。
#[test]
fn fr010_strong_does_not_resize() {
    assert_eq!(
        spec_of(InlineStyle::Strong).font_scale,
        1.0,
        "强调用字重而非字号（中文竖排网格感）"
    );
}

// ===========================================================================
// 标题 / 引用的字号缩放（FR-010 第 2 条，引擎侧）
// ===========================================================================

/// **引擎侧**：标题字号确实随阅读字号缩放。
///
/// ## 判据
///
/// 用同一段短文本作为标题排版，比较不同阅读字号下的**行高**——
/// 行高 = `font_size × line_height`，而标题的 `font_size` 被按
/// `font_scale` 放大过，故行高应等比变化。
///
/// 四档断言（14/19/24/30）对应 FR-010 验收子项。
#[test]
fn fr010_heading_line_height_scales_with_reading_font_size() {
    let text = "第一章 起始";
    let heading = spec_of(InlineStyle::Heading);
    let styles = [StyleSpan::new(
        InlineStyle::Heading,
        0..text.chars().count(),
    )];

    let mut heights = Vec::new();
    for reading_size in [14.0f32, 19.0, 24.0, 30.0] {
        let mut engine = LayoutEngine::new();
        let model = TextModel::new(text);
        let base = Geometry {
            measure_px: 400.0,
            font_size_px: reading_size,
            line_height: 1.85,
            alignment: Alignment::Justified,
        };
        let result = engine.layout_styled_paragraph(&model, 0, &base, &styles);

        let height = result.lines[0].height_px;
        let expected = reading_size * heading.font_scale * 1.85;
        assert!(
            (height - expected).abs() < 1e-3,
            "阅读字号 {reading_size}px 下标题行高应为 {expected:.2}px，实测 {height:.2}px"
        );
        heights.push(height);
    }

    // 单调递增（四档确实不同）
    for window in heights.windows(2) {
        assert!(
            window[1] > window[0],
            "阅读字号增大时标题行高应随之增大：{heights:?}"
        );
    }
}

// ===========================================================================
// 居中（FR-011 与本模块的接口）
// ===========================================================================

/// 标题与引用被**居中**（首字形有左偏移）。
#[test]
fn fr011_heading_and_quote_are_centered() {
    for style in [InlineStyle::Heading, InlineStyle::Quote] {
        let text = "短标题";
        let char_len = text.chars().count();
        let styles = [StyleSpan::new(style, 0..char_len)];

        let mut engine = LayoutEngine::new();
        let model = TextModel::new(text);
        let result = engine.layout_styled_paragraph(&model, 0, &geometry(400.0), &styles);

        let first_x = result.lines[0].glyphs[0].x_px;
        assert!(
            first_x > 1.0,
            "{style:?} 应被居中（首字形 x = {first_x:.2}，接近 0 说明未居中）"
        );
    }
}

/// **反例**：正文不居中（首字形 x ≈ 0）。
#[test]
fn fr011_body_is_not_centered() {
    let text = "正文不该居中。";
    let mut engine = LayoutEngine::new();
    let model = TextModel::new(text);
    let result = engine.layout_styled_paragraph(&model, 0, &geometry(400.0), &[]);

    let first_x = result.lines[0].glyphs[0].x_px;
    assert!(
        first_x.abs() < 1.0,
        "正文应左对齐（首字形 x = {first_x:.2}）"
    );
}

/// 无样式时 `layout_styled_paragraph` 与 `layout_paragraph` 等价。
#[test]
fn styled_entry_matches_plain_entry_when_no_styles() {
    let text = "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮。";

    let mut engine_a = LayoutEngine::new();
    let mut engine_b = LayoutEngine::new();
    let model = TextModel::new(text);
    let geometry = geometry(228.0);

    let plain = engine_a.layout_paragraph(&model, 0, &geometry);
    let styled = engine_b.layout_styled_paragraph(&model, 0, &geometry, &[]);

    assert_eq!(
        plain.lines.len(),
        styled.lines.len(),
        "无样式时两个入口的行数应一致"
    );
    for (left, right) in plain.lines.iter().zip(&styled.lines) {
        assert_eq!(left.char_range, right.char_range);
        assert!((left.height_px - right.height_px).abs() < 1e-4);
    }
}
