//! T025 验收：滚动流排布与完整性（FR-020 / FR-021 / SC-006b）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T025）
//!
//! > SC-006b：视口区间并集与正文逐字符相等；相邻视口连续无缺口无重叠
//!
//! ## SC-006b 原文（含括号里那句关键限定）
//!
//! > 滚动模式下把整个章节按视口移动顺序覆盖的字符区间取并集，MUST 与该章
//! > 正文**逐字符相等**（100%）；相邻视口的区间 MUST 连续无缺口、无重叠
//! > （**交叠的可见行按同一字符只属于一个视口区间的方式切分**）。
//!
//! 括号那句是设计约束而非结果描述：一屏能容下的行往往**多于**该屏的字符
//! 预算（末行只露出一半），故"可见行"是重叠的。若直接取"可见行的并集"作为
//! 视口区间，相邻视口就会重叠——那正是括号要排除的做法。
//!
//! 故本模块的口径是：**视口区间由"滚动步进"切分，不由"可见行"决定**。
//! 可见行是渲染关切（哪些行要画），区间是位置关切（谁拥有这些字符）。
//! 两者分离，重叠就无从产生。这个分离由 [`super::flow`] 的文档承载。
//!
//! ## 与分页的关系（FR-020 的"同一排版结果"）
//!
//! FR-020 说两种形态"在同一组几何参数下可切换"，FR-021 说"不引入第二套
//! 坐标"。故滚动流复用**同一份**行盒序列（`paginate::page::layout_chapter`），
//! 不重新排版——那会引入第二套断行结果，也就有了第二套坐标。
//! 本文件断言两种形态的**行盒完全相同**（同一份数据的证据）。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::{ScrollFlow, ScrollViewport};

// ===========================================================================
// 夹具
// ===========================================================================

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

/// 一段能滚成多屏的正文（含段落与混排）。
fn long_chapter() -> String {
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                     后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。";
    (0..10)
        .map(|index| format!("第{}段。{paragraph}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 取字符区间的文本（字符口径）。
fn slice_chars(content: &str, start: usize, end: usize) -> String {
    content.chars().skip(start).take(end - start).collect()
}

fn build_flow(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("构造滚动流应成功")
}

// ===========================================================================
// SC-006b 主判据
// ===========================================================================

/// **视口区间并集与正文逐字符相等**（SC-006b 的第一个分句）。
#[test]
fn viewport_union_equals_chapter_exactly() {
    let content = long_chapter();

    for (width, height, font_size) in [
        (342.0f32, 200.0f32, 19u16),
        (342.0, 400.0, 19),
        (228.0, 150.0, 14),
        (684.0, 800.0, 24),
        (1432.0, 300.0, 30),
        (342.0, 100.0, 19),
    ] {
        let mut geometry = geometry();
        geometry.font_size_px = font_size;
        let viewport = Viewport::new(width, height);
        let flow = build_flow(&content, &geometry, &viewport);

        let union: String = flow
            .viewports()
            .iter()
            .map(|v| slice_chars(&content, v.char_range.start, v.char_range.end))
            .collect();

        assert_eq!(
            union,
            content,
            "{width}x{height} / {font_size}px：视口区间并集与正文不符。\
             视口数 {}，区间 {:?}",
            flow.viewports().len(),
            flow.viewports()
                .iter()
                .map(|v| (v.char_range.start, v.char_range.end))
                .collect::<Vec<_>>()
        );
    }
}

/// **相邻视口连续无缺口无重叠**（SC-006b 的第二个分句）。
///
/// ## 判据的形式：`next.start == current.end`
///
/// 区间是半开的（`[start, end)`），故"无缝"= 后者起点 **等于** 前者终点。
/// 若用 `<=` 就允许重叠——那正是括号里排除的做法。
#[test]
fn adjacent_viewports_are_tightly_contiguous() {
    let content = long_chapter();

    for height in [80.0f32, 150.0, 300.0, 800.0] {
        let viewport = Viewport::new(342.0, height);
        let flow = build_flow(&content, &geometry(), &viewport);
        assert!(
            flow.viewports().len() > 1,
            "高度 {height}px 下应产出多屏（前置条件）"
        );

        let mut expected_start = 0usize;
        for (index, viewport_entry) in flow.viewports().iter().enumerate() {
            assert_eq!(
                viewport_entry.char_range.start, expected_start,
                "第 {index} 屏起点应为上一屏终点（无缺口无重叠）"
            );
            assert!(
                viewport_entry.char_range.end >= viewport_entry.char_range.start,
                "第 {index} 屏区间不得反向"
            );
            expected_start = viewport_entry.char_range.end;
        }
        assert_eq!(
            expected_start,
            content.chars().count(),
            "末屏终点应为正文末尾（高度 {height}px）"
        );
    }
}

/// **可见行可以重叠，但区间不可以**（括号那句的设计约束）。
///
/// ## 这条测的是什么
///
/// 一屏的可见行集合与它的字符区间**不是**同一件事：屏底那行可能只露出一
/// 部分，而它已完整计入本屏区间（否则会出现缺口）。下一屏从本屏**区间终点**
/// 之后开始——于是同一行可能同时"可见"于两屏（它横跨边界），但它的字符
/// 只属于一个区间。
///
/// 若实现偷懒用"可见行的并集"当区间，本测试会看到区间重叠。
#[test]
fn visible_lines_may_overlap_but_char_ranges_never_do() {
    let content = long_chapter();
    let viewport = Viewport::new(342.0, 150.0);
    let flow = build_flow(&content, &geometry(), &viewport);

    let mut total_visible_lines = 0usize;
    let mut saw_repeated_line = false;
    let mut previous_line_keys: Vec<(usize, usize)> = Vec::new();

    for viewport_entry in flow.viewports() {
        total_visible_lines += viewport_entry.lines.len();

        let keys: Vec<(usize, usize)> = viewport_entry
            .lines
            .iter()
            .map(|line| (line.chapter_char_range.start, line.chapter_char_range.end))
            .collect();

        // 本屏可见行与上一屏的交集 → 说明行跨了屏边界（合法，见文档）
        let repeated = keys
            .iter()
            .filter(|key| previous_line_keys.contains(key))
            .count();
        if repeated > 0 {
            saw_repeated_line = true;
        }
        previous_line_keys = keys;
    }

    // 各屏可见行总数 ≥ 正文行数（跨屏的行被数了两次 → 重叠的证据）
    let distinct_lines = {
        let mut all: Vec<(usize, usize)> = flow
            .viewports()
            .iter()
            .flat_map(|v| {
                v.lines
                    .iter()
                    .map(|line| (line.chapter_char_range.start, line.chapter_char_range.end))
            })
            .collect();
        all.sort_unstable();
        all.dedup();
        all.len()
    };

    assert!(
        total_visible_lines > distinct_lines,
        "本例应出现跨屏行（前置条件：可见行总数 {total_visible_lines} 应大于去重后的 {distinct_lines}）"
    );
    assert!(saw_repeated_line, "应观察到至少一处跨屏行（前置条件）");

    // 而区间**不重叠**（每个字符只属于一个区间）
    let mut seen_ends = Vec::new();
    for viewport_entry in flow.viewports() {
        seen_ends.push((
            viewport_entry.char_range.start,
            viewport_entry.char_range.end,
        ));
    }
    for pair in seen_ends.windows(2) {
        assert_eq!(
            pair[0].1, pair[1].0,
            "区间必须紧邻（{:?} 与 {:?}）——可见行可重叠但区间不可",
            pair[0], pair[1]
        );
    }
}

/// 视口与页**同构**（FR-021：不引入第二套坐标）。
#[test]
fn viewport_shape_is_isomorphic_to_page() {
    let content = long_chapter();
    let viewport = Viewport::new(342.0, 200.0);
    let flow = build_flow(&content, &geometry(), &viewport);

    for entry in flow.viewports() {
        // 与 `Page` 一样：字符区间 + 可见行盒序列 + 一个"我在流里的位置"
        let _: &std::ops::Range<usize> = &entry.char_range;
        let _: &[tome_reader::paginate::page::PageLine] = &entry.lines;
        let _: f32 = entry.top_px;
        let _: f32 = entry.height_px;

        assert!(
            entry.height_px > 0.0,
            "视口高度应为正（它是位置换算的分母）"
        );
        assert!(
            entry.char_range.start <= entry.char_range.end,
            "区间不得反向"
        );
    }
}

// ===========================================================================
// FR-020：连续流无分页空白
// ===========================================================================

/// **流中无因"分页"产生的额外空白**（FR-020 的后半句）。
///
/// ## 判据：累计流高 ≈ 各行高之和
///
/// 分页模式会在页底留出不足一行的空白（下一条行放不进就换页）。滚动模式
/// 若沿用那个空白，`content_height` 就会显著大于"所有行高之和"。
///
/// 故断言：总流高 **等于** 各行高之和（不给任何页边界留余量）。允许 1e-3
/// 的浮点余量。
#[test]
fn content_height_has_no_pagination_padding() {
    let content = long_chapter();
    let viewport = Viewport::new(342.0, 200.0);
    let flow = build_flow(&content, &geometry(), &viewport);

    let distinct_line_height_sum: f32 = {
        let mut all: Vec<(usize, usize, f32)> = flow
            .viewports()
            .iter()
            .flat_map(|v| {
                v.lines
                    .iter()
                    .map(|line| (line.chapter_char_range.start, line.line.height_px))
                    .map(|(start, height)| (start, 0usize, height))
                    .collect::<Vec<_>>()
            })
            .collect();
        all.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        all.dedup_by_key(|entry| entry.0);
        all.iter().map(|entry| entry.2).sum()
    };

    assert!(
        (flow.content_height_px() - distinct_line_height_sum).abs() < 1e-2,
        "滚动流总高 {:.2}px 应等于各行高之和 {:.2}px（不得含分页留白）",
        flow.content_height_px(),
        distinct_line_height_sum
    );
}

/// 相邻视口的 `top_px` 差 = 前者高度（步进是连续的，无跳跃）。
#[test]
fn viewport_tops_are_contiguous() {
    let content = long_chapter();
    let viewport = Viewport::new(342.0, 200.0);
    let flow = build_flow(&content, &geometry(), &viewport);

    let entries = flow.viewports();
    assert!(entries.len() > 1, "前置：应有多屏");

    for pair in entries.windows(2) {
        assert!(
            (pair[1].top_px - pair[0].top_px - pair[0].height_px).abs() < 1e-2,
            "相邻屏 top 差 {:.2} 应等于前者高度 {:.2}",
            pair[1].top_px - pair[0].top_px,
            pair[0].height_px
        );
    }
    assert!((entries[0].top_px - 0.0).abs() < 1e-3, "首屏 top 应为 0");
}

// ===========================================================================
// 边界（spec §Edge Cases）
// ===========================================================================

/// 空章 → 一个视口（不得零个，否则滚动无内容可呈现）。
#[test]
fn empty_chapter_yields_one_viewport() {
    let flow = build_flow("", &geometry(), &Viewport::new(342.0, 200.0));
    assert_eq!(flow.viewports().len(), 1, "空章应产出一屏");
    assert_eq!(flow.viewports()[0].char_range, 0..0);
    assert!(flow.viewports()[0].lines.is_empty());
}

/// 仅空白正文同样产出一屏。
#[test]
fn whitespace_only_chapter_yields_one_viewport() {
    let flow = build_flow(
        "   \n\u{3000}\n  ",
        &geometry(),
        &Viewport::new(342.0, 200.0),
    );
    assert_eq!(flow.viewports().len(), 1);
}

/// **单行高 > 视口高**时该行独占一屏（不死循环、不丢内容）。
#[test]
fn line_taller_than_viewport_gets_its_own_screen() {
    let content = "内容够长，会排成若干行。";
    let geometry = geometry();
    let viewport = Viewport::new(342.0, geometry.line_height_px() * 0.5);
    let flow = build_flow(content, &geometry, &viewport);

    assert!(!flow.viewports().is_empty(), "不得产出零屏");
    let union: String = flow
        .viewports()
        .iter()
        .map(|v| slice_chars(content, v.char_range.start, v.char_range.end))
        .collect();
    assert_eq!(union, content, "超高行方案下仍不得丢字");
}

/// 极窄窗不 panic、不产出负宽（spec §Edge Cases 的"窗口极窄"）。
#[test]
fn extreme_narrow_viewport_does_not_panic() {
    for width in [0.0f32, 1.0, 10.0] {
        let content = "一些正文内容。";
        let flow = build_flow(content, &geometry(), &Viewport::new(width, 100.0));
        assert!(!flow.viewports().is_empty());
    }
}

/// 视口高度为 0 时不 panic（除零风险点）。
#[test]
fn zero_height_viewport_does_not_panic() {
    let content = "一些正文内容。";
    let flow = build_flow(content, &geometry(), &Viewport::new(342.0, 0.0));
    assert!(!flow.viewports().is_empty());
    assert!(
        flow.content_height_px().is_finite(),
        "总高必须是有限值（不得 NaN/Inf）"
    );
}

// ===========================================================================
// FR-020：两种形态共享**同一份**排版结果
// ===========================================================================

/// 滚动流与分页使用**相同的行盒序列**（FR-020 的"同一排版结果"）。
///
/// ## 为什么这条塞在 T025 而不是 T027
///
/// 它是"不引入第二套坐标"（FR-021）的**前置事实**：若两种形态各自排版，
/// 断行结果可能不同（例如不同的版心），那时"锚点不变"就是巧合而非保证。
/// 故必须在滚动流建立的时候就钉住"行盒来自同一次排版"。
#[test]
fn scroll_flow_reuses_the_same_layout_as_pagination() {
    let content = long_chapter();
    let viewport = Viewport::new(342.0, 300.0);
    let mut paginator = Paginator::new();

    let chapter =
        layout_chapter(&mut paginator, &content, &geometry(), &viewport).expect("排版应成功");
    let chapter_lines: Vec<(usize, usize)> = chapter
        .lines
        .iter()
        .map(|line| (line.chapter_char_range.start, line.chapter_char_range.end))
        .collect();

    // 分页形态的行盒（展平各页）
    let paginated = paginator
        .paginate(&content, &geometry(), &viewport)
        .expect("分页应成功");
    let paginated_lines: Vec<(usize, usize)> = paginated
        .pages
        .iter()
        .flat_map(|page| page.lines.iter())
        .map(|line| (line.chapter_char_range.start, line.chapter_char_range.end))
        .collect();

    assert_eq!(
        chapter_lines, paginated_lines,
        "两种形态必须消费同一份行盒序列（否则是第二套坐标，违 FR-021）"
    );
    assert!(!chapter_lines.is_empty(), "前置：应有行盒");
}

/// 滚动流对同一输入是**确定**的（FR-048 的形态扩展）。
#[test]
fn scroll_flow_is_deterministic() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 200.0);

    let first = build_flow(&content, &geometry, &viewport);
    let second = build_flow(&content, &geometry, &viewport);

    assert_eq!(first.viewports().len(), second.viewports().len());
    for (a, b) in first.viewports().iter().zip(second.viewports()) {
        assert_eq!(a.char_range, b.char_range);
        assert_eq!(a.top_px, b.top_px);
        assert_eq!(a.lines.len(), b.lines.len());
    }
    assert_eq!(first.content_height_px(), second.content_height_px());
}

// ===========================================================================
// 结构自检
// ===========================================================================

/// `ScrollViewport` 可独立构造与比较（它是 DTO，不是内部状态）。
#[test]
fn scroll_viewport_is_a_plain_value() {
    let entry = ScrollViewport {
        viewport_index: 3,
        top_px: 600.0,
        height_px: 200.0,
        char_range: 100..150,
        lines: Vec::new(),
    };
    assert_eq!(entry.char_len(), 50);
    assert_eq!(entry.viewport_index, 3);

    let same = ScrollViewport {
        viewport_index: 3,
        top_px: 600.0,
        height_px: 200.0,
        char_range: 100..150,
        lines: Vec::new(),
    };
    assert_eq!(entry, same, "同字段应相等（DTO 的可比性）");
}
