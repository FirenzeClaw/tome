//! 前导 2 验收：ParagraphMode 的**施加**（FR-018 / US1 场景 4）
//!
//! ## 为什么单独一个文件
//!
//! T017 已把 `ParagraphMode` 的 **schema 与校验**做进 `ReadingConfig`
//! （`tests/reading_config.rs`：默认 2em、负值被拒、互斥）。但"配置里存了"
//! 与"排版时真的按它排"是两件事——**前者全绿不能证明后者**。本文件钉住后者。
//!
//! ## FR-018 的两条可断言性质
//!
//! > 缩进模式：每个自然段首行缩进 2 个字身，且段间无额外间距；
//! > 间距模式：每个自然段首行不缩进、段间有 0.4em 间距。
//!
//! 逐条落到可观测量上：
//!
//! | 断言 | 缩进模式 | 间距模式 | 判据载体 |
//! |---|---|---|---|
//! | 段首行左偏移 | `2 × 字号` px | `0` px | `LineBox::glyphs[0].x_px`（段首行） |
//! | 段内其余行的左偏移 | `0` px | `0` px | 同上（第 2 行起） |
//! | 段间额外竖向间距 | `0` px | `0.4 × 字号` px | 段的**末行** `height_px` 增量 |
//!
//! ## 为什么"末行高度"是间距的载体
//!
//! 竖向间距只有两种记账方式：记在**段前**（下一段首行更高）或记在**段后**
//! （本段末行更高）。本实现取段后——理由是页/屏的装填逻辑（`pack_pages`
//! 与 `slice_into_viewports`）**逐行累加 `height_px`**，间距记进行高即自动
//! 进入分页与滚动流，无需在两处各写一遍。本文件据此断言。
//!
//! ## 反例方向（同样必须断言）
//!
//! "缩进模式有间距"与"间距模式有缩进"都必须是**零**。只断言正方向会让
//! "两个模式都做"或"两个模式都做一半"的实现在测试里看不出来——那是
//! §10.1 记的三次"全绿但没测到"的同一种形态。

use tome_reader::layout::engine::{Geometry as LayoutGeometry, LayoutEngine};
use tome_reader::paginate::config::{
    GeometryConfig, ParagraphMode, ReadingConfig, DEFAULT_PARAGRAPH_GAP_EM,
    DEFAULT_PARAGRAPH_INDENT_EM,
};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::rules::justify::Alignment;
use tome_reader::text::model::TextModel;

const FONT_SIZE_PX: f32 = 19.0;
const LINE_HEIGHT: f32 = 1.85;
const MEASURE_PX: f32 = 418.0;

/// 三段正文（每段够长，保证**每段都不止一行**——首行缩进才有观测面）。
fn sample_chapter() -> String {
    [
        "他的手指在书页边缘停了很久，那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样；她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然。」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
    ]
    .join("\n")
}

fn layout_geometry(mode: ParagraphMode) -> LayoutGeometry {
    LayoutGeometry {
        measure_px: MEASURE_PX,
        font_size_px: FONT_SIZE_PX,
        line_height: LINE_HEIGHT,
        alignment: Alignment::Justified,
        paragraph_mode: mode,
    }
}

fn geometry(mode: ParagraphMode) -> GeometryConfig {
    GeometryConfig {
        font_size_px: FONT_SIZE_PX as u16,
        line_height: LINE_HEIGHT,
        paragraph_mode: mode,
        measure_em: MEASURE_PX / FONT_SIZE_PX,
        ..GeometryConfig::default()
    }
}

/// 排一段，返回（行盒，段内行下标）。
fn layout_one(engine: &mut LayoutEngine, text: &str, mode: ParagraphMode) -> Vec<(f32, f32)> {
    let model = TextModel::new(text);
    let result = engine.layout_paragraph(&model, 0, &layout_geometry(mode));
    result
        .lines
        .iter()
        .map(|line| {
            (
                line.glyphs.first().map_or(0.0, |glyph| glyph.x_px),
                line.height_px,
            )
        })
        .collect()
}

// ===========================================================================
// 缩进模式：段首行 2 字身，段内其余行不缩进，无额外段距
// ===========================================================================

/// US1 场景 4 的正方向：**段首行左偏移 = 2 字身**。
#[test]
fn indent_mode_shifts_first_line_by_two_em() {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let lines = layout_one(
        &mut engine,
        &sample_chapter(),
        ParagraphMode::Indent { em: 2.0 },
    );

    assert!(
        lines.len() >= 2,
        "样本段应至少两行（首行缩进才有观测面），实测 {} 行",
        lines.len()
    );

    let expected = FONT_SIZE_PX * DEFAULT_PARAGRAPH_INDENT_EM;
    let indent_px = lines[0].0;
    assert!(
        (indent_px - expected).abs() < 0.5,
        "缩进模式段首行左偏移应为 {expected:.2}px（2em），实测 {indent_px:.2}px"
    );
}

/// 缩进模式**只作用于段首行**——第 2 行起必须回到 0。
///
/// 反例方向：若实现把缩进当成了"整段左缩进"（缩进模式下的常见误解），
/// 这条会失败，而上面那条仍然通过。
#[test]
fn indent_mode_does_not_shift_later_lines() {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let lines = layout_one(
        &mut engine,
        &sample_chapter(),
        ParagraphMode::Indent { em: 2.0 },
    );

    for (index, (x, _)) in lines.iter().enumerate().skip(1) {
        assert!(
            x.abs() < 0.5,
            "缩进不该作用于第 {index} 行（只有段首行缩进），实测左偏移 {x:.2}px"
        );
    }
}

/// 缩进模式的**段间无额外间距**：全部行高相等。
#[test]
fn indent_mode_has_no_extra_paragraph_gap() {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let lines = layout_one(
        &mut engine,
        &sample_chapter(),
        ParagraphMode::Indent { em: 2.0 },
    );

    let first = lines[0].1;
    for (index, (_, height)) in lines.iter().enumerate() {
        assert!(
            (height - first).abs() < 1e-3,
            "缩进模式不该有段间距：第 {index} 行高 {height:.2}px ≠ 第 0 行 {first:.2}px"
        );
    }
}

// ===========================================================================
// 间距模式：段首行不缩进，段间 0.4em
// ===========================================================================

/// 间距模式的**首行不缩进**（FR-018："间距模式无首行缩进"）。
#[test]
fn gap_mode_does_not_indent_first_line() {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let lines = layout_one(
        &mut engine,
        &sample_chapter(),
        ParagraphMode::Gap { em: 0.4 },
    );

    for (index, (x, _)) in lines.iter().enumerate() {
        assert!(
            x.abs() < 0.5,
            "间距模式不该有首行缩进：第 {index} 行左偏移 {x:.2}px"
        );
    }
}

/// 间距模式的**段间 0.4em**——落在段末行的行高上。
#[test]
fn gap_mode_adds_four_tenths_em_to_paragraph_tail() {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let lines = layout_one(
        &mut engine,
        &sample_chapter(),
        ParagraphMode::Gap { em: 0.4 },
    );

    assert!(lines.len() >= 2, "样本段应至少两行");

    let base_line_height = FONT_SIZE_PX * LINE_HEIGHT;
    let expected_gap = FONT_SIZE_PX * DEFAULT_PARAGRAPH_GAP_EM;

    // 段内非末行：无间距
    for (index, (_, height)) in lines.iter().enumerate().take(lines.len() - 1) {
        assert!(
            (height - base_line_height).abs() < 1e-3,
            "间距模式的段内第 {index} 行不该带间距：{height:.2}px ≠ {base_line_height:.2}px"
        );
    }

    // 末行：带 0.4em
    let tail = lines.last().map(|(_, height)| *height).unwrap_or_default();
    let expected_tail = base_line_height + expected_gap;
    assert!(
        (tail - expected_tail).abs() < 1e-2,
        "间距模式的段末行行高应为 {expected_tail:.2}px（行高 + 0.4em），实测 {tail:.2}px"
    );
}

// ===========================================================================
// 互斥（FR-018 的明文要求）
// ===========================================================================

/// 两模式**互斥**：缩进模式的段末行高不被加长，间距模式的首行不被缩进。
///
/// 这条与上面几条不重复：上面几条各自断言"本模式做对了"，本条断言
/// "没做另一模式的事"。两方向都过才排除"两个都做"的实现。
#[test]
fn modes_are_mutually_exclusive() {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let text = &sample_chapter();
    let base_line_height = FONT_SIZE_PX * LINE_HEIGHT;

    let indent = layout_one(&mut engine, text, ParagraphMode::Indent { em: 2.0 });
    let gap = layout_one(&mut engine, text, ParagraphMode::Gap { em: 0.4 });

    // 缩进模式的段末行 = 基准行高（无段距）
    let indent_tail = indent.last().map(|(_, height)| *height).unwrap_or_default();
    assert!(
        (indent_tail - base_line_height).abs() < 1e-3,
        "缩进模式不该有段间距，段末行 {indent_tail:.2}px ≠ {base_line_height:.2}px"
    );

    // 间距模式的段首行 = 0 偏移（无缩进）
    let gap_head_x = gap.first().map(|(x, _)| *x).unwrap_or_default();
    assert!(
        gap_head_x.abs() < 0.5,
        "间距模式不该有首行缩进，实测 {gap_head_x:.2}px"
    );
}

/// 缩进量的**参数化**：0.5/1/2/3em 各档位偏移量与声明成比例（不是硬编码 2em）。
#[test]
fn indent_amount_is_parametric() {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let text = &sample_chapter();

    for em in [0.5f32, 1.0, 2.0, 3.0] {
        let lines = layout_one(&mut engine, text, ParagraphMode::Indent { em });
        let expected = FONT_SIZE_PX * em;
        assert!(
            (lines[0].0 - expected).abs() < 0.5,
            "{em}em 缩进应为 {expected:.2}px，实测 {:.2}px",
            lines[0].0
        );
    }
}

// ===========================================================================
// 与分页/章节层的一致性（模式必须真的进到管线里）
// ===========================================================================

/// **章级接线**：`layout_chapter` 必须把 `GeometryConfig.paragraph_mode` 传下去。
///
/// 这条防的是"引擎做对了但管线没接"——那是最容易发生的半成品形态：
/// 单测全绿而 CLI 出来的结果里缩进根本没生效。
#[test]
fn chapter_layout_applies_paragraph_mode() {
    let content = sample_chapter();
    let viewport = Viewport::new(MEASURE_PX + 64.0, 600.0);
    let mut paginator = Paginator::new();

    let indent_chapter = layout_chapter(
        &mut paginator,
        &content,
        &geometry(ParagraphMode::Indent { em: 2.0 }),
        &viewport,
    )
    .expect("缩进模式排版应成功");

    let gap_chapter = layout_chapter(
        &mut paginator,
        &content,
        &geometry(ParagraphMode::Gap { em: 0.4 }),
        &viewport,
    )
    .expect("间距模式排版应成功");

    // 每段的**首行**（段内第 0 行）——按 `paragraph_index` 分组取每组第一行
    let first_lines_and_tails = |chapter: &tome_reader::paginate::page::ChapterLayout| {
        let mut heads: Vec<f32> = Vec::new();
        let mut tails: Vec<(usize, f32)> = Vec::new();
        let mut last_paragraph: Option<usize> = None;
        for page_line in &chapter.lines {
            let line = &page_line.line;
            if last_paragraph != Some(line.paragraph_index) {
                heads.push(line.glyphs.first().map_or(0.0, |glyph| glyph.x_px));
                last_paragraph = Some(line.paragraph_index);
            }
            tails.push((line.paragraph_index, line.height_px));
        }
        (heads, tails)
    };

    let (indent_heads, indent_tails) = first_lines_and_tails(&indent_chapter);
    let (gap_heads, gap_tails) = first_lines_and_tails(&gap_chapter);

    assert_eq!(indent_heads.len(), 3, "样本有三段，应有三个段首行");

    // 缩进模式：每个段首行都缩进 2em
    for (index, x) in indent_heads.iter().enumerate() {
        let expected = FONT_SIZE_PX * 2.0;
        assert!(
            (x - expected).abs() < 0.5,
            "缩进模式第 {index} 段的段首行应缩进 {expected:.2}px，实测 {x:.2}px"
        );
    }
    // 间距模式：每个段首行都不缩进
    for (index, x) in gap_heads.iter().enumerate() {
        assert!(
            x.abs() < 0.5,
            "间距模式第 {index} 段的段首行不该缩进，实测 {x:.2}px"
        );
    }

    // 间距模式：**章内**除最后一段外，每段末行带 0.4em 间距
    let base_line_height = FONT_SIZE_PX * LINE_HEIGHT;
    let expected_gap = FONT_SIZE_PX * DEFAULT_PARAGRAPH_GAP_EM;
    let last_paragraph = gap_tails.last().map(|(index, _)| *index);
    for (index, (paragraph_index, height)) in gap_tails.iter().enumerate() {
        let is_paragraph_tail = gap_tails
            .get(index + 1)
            .is_none_or(|(next_paragraph, _)| next_paragraph != paragraph_index);
        let is_chapter_tail = Some(*paragraph_index) == last_paragraph;
        if is_paragraph_tail && !is_chapter_tail {
            let expected = base_line_height + expected_gap;
            assert!(
                (height - expected).abs() < 1e-2,
                "间距模式第 {paragraph_index} 段末行应为 {expected:.2}px，实测 {height:.2}px"
            );
        }
    }

    // 缩进模式：没有任何行带间距
    for (_, height) in &indent_tails {
        assert!(
            (height - base_line_height).abs() < 1e-3,
            "缩进模式不该有段间距，实测行高 {height:.2}px"
        );
    }
}

/// 两种模式都**不改变字符归属**（FR-010 的同款不变量，在此作为回归护栏）。
///
/// 缩进与间距都只该改"画在哪"，不该改"哪些字符在哪一行/哪一页"以外的东西
/// ——具体说，**页区间拼接仍必须与正文逐字符相等**（SC-006）。
#[test]
fn both_modes_preserve_chapter_char_tiling() {
    let content = sample_chapter();
    let viewport = Viewport::new(MEASURE_PX + 64.0, 600.0);

    for mode in [
        ParagraphMode::Indent { em: 2.0 },
        ParagraphMode::Gap { em: 0.4 },
    ] {
        let mut paginator = Paginator::new();
        let result = paginator
            .paginate(&content, &geometry(mode), &viewport)
            .expect("分页应成功");

        let rebuilt: String = result
            .pages
            .iter()
            .filter(|page| !page.is_padding)
            .map(|page| {
                content
                    .chars()
                    .skip(page.char_range.start)
                    .take(page.char_range.len())
                    .collect::<String>()
            })
            .collect();

        assert_eq!(
            rebuilt, content,
            "{mode:?} 下页区间拼接必须与正文逐字符相等（SC-006）"
        );
    }
}

/// 默认配置走缩进模式（FR-018 / 裁决 OQ#12→A），且量是 2em。
#[test]
fn default_config_is_indent_two_em() {
    let config = ReadingConfig::default_ma();
    assert_eq!(
        config.geometry.paragraph_mode,
        ParagraphMode::Indent { em: 2.0 },
        "默认必须是段首缩进 2em"
    );
    assert_eq!(
        config.geometry.paragraph_mode,
        ParagraphMode::default(),
        "默认构造与 default_ma 必须一致"
    );
}
