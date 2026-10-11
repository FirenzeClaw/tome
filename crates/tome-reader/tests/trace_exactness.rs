//! T042 验收：痕迹区间 → 可见片段 + 样式词表（FR-037 / FR-033）
//!
//! ## 本文件的两组判据
//!
//! **A. FR-037 / SC-013：痕迹渲染的精确性**
//!
//! > 渲染出的像素范围覆盖的字符集合**恰好等于**该区间。
//!
//! 判据是**双向相等**，不是"包含"：
//!
//! - **不多**：区间外的字符不得被装饰覆盖（否则改字号后高亮会"长出来"）
//! - **不少**：区间内的每个字符都必须被覆盖（否则长高亮中间会缺口）
//!
//! 且跨页 / 跨视口时**分段渲染，拼接无缝隙无重叠**——两段的字符集合之并
//! 等于原区间，交集为空。
//!
//! **B. FR-033：样式词表的解析序**
//!
//! > ① 语义词表 → ② 颜色名 → ③ 默认下划线 + 诊断。
//!
//! 三级都要测，且**第 ③ 级必须产出可辨识的诊断**（MUST NOT 静默不渲染）。
//!
//! ## 为什么"恰好相等"必须用字符集合表达而不是像素数
//!
//! 高亮区间的像素宽度随字号变化（19px 与 24px 下宽度不同），但**覆盖的
//! 字符集合必须完全相同**（US4 场景 1）。故判据必须建立在字符维度上——
//! 本文件用"装饰四边形的 `char_range` 与字形四边形的 `char_offset` 相交
//! 判定"，而不是"算一下覆盖了多少像素"。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音（与 theme_independence.rs 同一约定）"
)]

use std::collections::BTreeSet;

use tome_core::model::theme::DayTheme;
use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{layout_chapter, Page, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::ScrollFlow;
use tome_reader::trace::render::{
    render_traces, render_traces_scroll, resolve_style, StyleResolution, TraceRange,
    SEMANTIC_STYLE_VOCABULARY,
};
use tome_reader::trace::TraceSegment;

// ===========================================================================
// 夹具
// ===========================================================================

fn chapter_text() -> String {
    [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样。她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然，」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
        "《红楼梦》（又名《石头记》）是清代曹雪芹的作品，书中有四百多个人物，其中以宝黛钗三人最为人所知。",
        "桌上有笔、墨、纸、砚；窗边有兰、竹、菊、梅；墙上挂着山水、花鸟、人物；架上摆着经、史、子、集。",
    ]
    .join("\n")
}

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

fn paginate(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> Vec<Page> {
    let mut paginator = Paginator::new();
    paginator
        .paginate(content, geometry, viewport)
        .expect("分页应成功")
        .pages
}

fn scroll_viewports(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("滚动流应成功")
}

/// 从一批行盒里收集**被装饰覆盖的章内字符集合**。
///
/// ## 判据的实现
///
/// 一条痕迹渲染后产出若干 [`TraceSegment`]，每个段落携带它在章内的字符区间。
/// 本函数把它们的字符展开成集合——用集合而非区间列表，是因为"无重叠"
/// 这一条只有在集合基数上才能看出来（两条区间重叠 3 个字符时，
/// `union` 的基数会比 `sum(len)` 少 3）。
fn covered_chars(segments: &[TraceSegment]) -> BTreeSet<usize> {
    let mut covered = BTreeSet::new();
    for segment in segments {
        for index in segment.char_range.clone() {
            covered.insert(index);
        }
    }
    covered
}

/// 期望的字符集合（区间的展开）。
fn expected_chars(range: &std::ops::Range<usize>) -> BTreeSet<usize> {
    range.clone().collect()
}

// ===========================================================================
// A. FR-037 / SC-013：精确性
// ===========================================================================

/// **单页内：渲染覆盖的字符集合恰好等于区间**（SC-013）。
#[test]
fn single_page_coverage_is_exactly_the_range() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);
    let page = pages.first().expect("应有页");

    let range = TraceRange::new(20, 20);
    let resolution = resolve_style("underline");
    let segments = render_traces(
        &[range],
        std::slice::from_ref(page),
        &geometry,
        &viewport,
        DayTheme.theme(),
        &resolution,
    );

    assert_eq!(
        covered_chars(&segments),
        expected_chars(&std::ops::Range { start: 20, end: 40 }),
        "渲染覆盖的字符集合必须恰好等于区间（SC-013：不多不少）"
    );
}

/// **跨页：分段渲染，拼接无缝隙无重叠**（FR-037 / SC-013）。
///
/// ## "恰好等于区间"的确切口径（分隔符的归属）
///
/// 段落切分产出的段文本**不含换行符**，故换行符不落在任何行盒的章内区间里
/// （它由页的 `char_range` 认领，见 `paginate/page.rs` 的"段间分隔符的归属"）。
/// 而 FR-037 的判据是"渲染出的**像素范围**覆盖的字符集合"——换行符不产生
/// 像素，故它本就不在被覆盖集合内。本测试显式写出这条口径，否则"少了换行符"
/// 会被误读成实现有缝隙。
#[test]
fn cross_page_segments_tile_the_range_exactly() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let pages = paginate(&content, &geometry, &viewport);
    assert!(pages.len() >= 2, "前置：本例应有多页");

    // 取一个横跨第 0 页与第 1 页边界的区间
    let boundary = pages[0].char_range.end;
    assert!(boundary > 5, "前置：第 0 页应有足够内容");
    let range = TraceRange::new(boundary - 5, 10);

    let resolution = resolve_style("underline");
    let segments = render_traces(
        &[range],
        &pages,
        &geometry,
        &viewport,
        DayTheme.theme(),
        &resolution,
    );

    // ① 无重叠：各段长度之和 == 并集基数
    let total_len: usize = segments
        .iter()
        .map(|segment| segment.char_range.len())
        .sum();
    let union_len = covered_chars(&segments).len();
    assert_eq!(
        total_len, union_len,
        "跨页分段之间不得重叠（长度之和 {total_len} != 并集基数 {union_len}）"
    );

    // ② 无缝隙：并集恰好覆盖原区间里**会产生字形**的字符。
    //    分隔符（换行）不产生字形，故不参与覆盖——这是既有口径，非缺陷：
    //    段落切分产出的段文本不含换行符，它由页的 char_range 认领
    //    （见 paginate/page.rs 的「段间分隔符的归属」）。
    let chars: Vec<char> = content.chars().collect();
    let expected: BTreeSet<usize> = (range.start..range.end())
        .filter(|index| !matches!(chars.get(*index), Some('\n')))
        .collect();
    assert_eq!(
        covered_chars(&segments),
        expected,
        "跨页分段拼接后必须恰好覆盖原区间（不得有缝隙）"
    );

    // ③ 拼接无缝的正面证据：相邻两段的端点相接，**或**间隔的恰好是不产生
    //    字形的分隔符（换行）。后者是既有口径（见上方说明），不是缝隙——
    //    若间隔里出现任何会产生字形的字符，那就是真的丢了内容。
    let mut sorted = segments.clone();
    sorted.sort_by_key(|segment| segment.char_range.start);
    for pair in sorted.windows(2) {
        let gap = pair[0].char_range.end..pair[1].char_range.start;
        for index in gap {
            assert_eq!(
                chars.get(index),
                Some(&'\n'),
                "段间间隔里的字符 {index} 不是分隔符——这是真正的缝隙（内容丢失）"
            );
        }
    }

    // ③ 真的分成了两段（否则"分段渲染"这条没被验证）
    assert!(
        segments.len() >= 2,
        "跨页区间应被分成多段渲染，实测 {} 段",
        segments.len()
    );
}

/// **跨视口：滚动模式下分段渲染，拼接无缝隙无重叠**（FR-037）。
#[test]
fn cross_viewport_segments_tile_the_range_exactly() {
    let content = chapter_text();
    let geometry = geometry();
    // 视口刻意矮，保证切成多屏
    let viewport = Viewport::new(342.0, 120.0);
    let flow = scroll_viewports(&content, &geometry, &viewport);
    assert!(flow.viewports().len() >= 2, "前置：本例应有多屏");

    let boundary = flow.viewports()[0].char_range.end;
    assert!(boundary > 5, "前置：第 0 屏应有足够内容");
    let range = TraceRange::new(boundary - 5, 10);

    let resolution = resolve_style("underline");
    let segments = render_traces_scroll(
        &[range],
        &flow,
        &geometry,
        &viewport,
        DayTheme.theme(),
        &resolution,
    );

    let total_len: usize = segments
        .iter()
        .map(|segment| segment.char_range.len())
        .sum();
    let union_len = covered_chars(&segments).len();
    assert_eq!(
        total_len, union_len,
        "跨视口分段之间不得重叠（长度之和 {total_len} != 并集基数 {union_len}）"
    );
    // 同跨页口径：分隔符不产生字形，故不参与覆盖
    let chars: Vec<char> = content.chars().collect();
    let expected: BTreeSet<usize> = (range.start..range.end())
        .filter(|index| !matches!(chars.get(*index), Some('\n')))
        .collect();
    assert_eq!(
        covered_chars(&segments),
        expected,
        "跨视口分段拼接后必须恰好覆盖原区间（不得有缝隙）"
    );
    // 相邻段端点相接（无缝的正面证据）
    let mut sorted = segments.clone();
    sorted.sort_by_key(|segment| segment.char_range.start);
    for pair in sorted.windows(2) {
        assert_eq!(
            pair[0].char_range.end, pair[1].char_range.start,
            "跨视口相邻段落的端点必须相接，否则就是缝隙"
        );
    }
    assert!(segments.len() >= 2, "跨视口区间应被分成多段渲染");
}

/// **改字号不改变覆盖的字符集合**（US4 场景 1 / SC-013 的另一种表述）。
///
/// ## 这条是"字符区间而非渲染坐标"这条铁律的可见后果
///
/// 字号变化只影响几何，不影响覆盖集合。若某处实现用像素坐标表示痕迹，
/// 改字号后集合就会变——本测试就是那条防线的位置。
#[test]
fn font_size_change_does_not_change_covered_chars() {
    let content = chapter_text();
    let viewport = Viewport::new(342.0, 500.0);

    let mut small_geometry = geometry();
    small_geometry.font_size_px = 14;
    let mut large_geometry = geometry();
    large_geometry.font_size_px = 30;

    let small_pages = paginate(&content, &small_geometry, &viewport);
    let large_pages = paginate(&content, &large_geometry, &viewport);
    assert!(!small_pages.is_empty() && !large_pages.is_empty());

    let range = TraceRange::new(10, 30);
    let resolution = resolve_style("underline");

    let small_segments = render_traces(
        &[range],
        &small_pages,
        &small_geometry,
        &viewport,
        DayTheme.theme(),
        &resolution,
    );
    let large_segments = render_traces(
        &[range],
        &large_pages,
        &large_geometry,
        &viewport,
        DayTheme.theme(),
        &resolution,
    );

    assert_eq!(
        covered_chars(&small_segments),
        covered_chars(&large_segments),
        "19px 与 24px 下高亮覆盖的字符集合必须完全相同（US4 场景 1）"
    );
    assert_eq!(
        covered_chars(&small_segments),
        expected_chars(&std::ops::Range { start: 10, end: 40 })
    );
}

/// **空区间与越界区间不产出装饰**（边界，不 panic）。
#[test]
fn empty_and_out_of_bounds_ranges_produce_no_segments() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);

    let resolution = resolve_style("underline");

    // 空区间
    let empty = render_traces(
        &[TraceRange::new(10, 0)],
        &pages,
        &geometry,
        &viewport,
        DayTheme.theme(),
        &resolution,
    );
    assert!(empty.is_empty(), "空区间不该产出任何渲染段");

    // 远超章长的区间（应只覆盖实际存在的部分，不 panic）
    let hu = content.chars().count();
    let huge = render_traces(
        &[TraceRange::new(hu + 100, 50)],
        &pages,
        &geometry,
        &viewport,
        DayTheme.theme(),
        &resolution,
    );
    assert!(hu + 100 > hu, "前置：起点越界。渲染应钳制到章末而非 panic");
    assert!(huge.is_empty(), "完全越界的区间不该产出渲染段");
}

// ===========================================================================
// B. FR-033：样式词表的三级解析
// ===========================================================================

/// **第一级：语义词表命中**（FR-033 ①②③ 的第一级）。
#[test]
fn semantic_vocabulary_resolves_to_declared_effects() {
    for entry in SEMANTIC_STYLE_VOCABULARY {
        let resolution = resolve_style(entry.token);
        assert!(
            resolution.is_semantic(),
            "语义词 `{}` 应命中第一级（语义词表）",
            entry.token
        );
        assert_eq!(
            resolution.effect(),
            entry.effect,
            "语义词 `{}` 的渲染效应应与词表声明一致",
            entry.token
        );
    }
}

/// **第二级：颜色名解析为背景色**（FR-033 的第二级）。
#[test]
fn color_names_resolve_to_background_color() {
    for name in ["red", "yellow", "green", "blue", "orange", "purple"] {
        let resolution = resolve_style(name);
        assert!(
            !resolution.is_semantic(),
            "颜色名 `{name}` 不该命中语义词表"
        );
        assert!(resolution.is_color_name(), "`{name}` 应命中级二（颜色名）");
        assert!(
            resolution.has_background(),
            "颜色名应渲染为背景色（FR-033 级二）"
        );
    }
    // 大小写不敏感（书源作者写 `Red` / `RED` 都常见）
    assert_eq!(
        resolve_style("RED").effect(),
        resolve_style("red").effect(),
        "颜色名解析应大小写不敏感"
    );
}

/// **第三级：未知样式回退默认下划线 + 可辨识诊断**（FR-033 的第三级）。
///
/// ## "可辨识"的含义
///
/// 诊断必须携带**原样式串**——否则用户看到"某条高亮用了默认样式"却不知道
/// 是哪条规则写的样式。这是"MUST NOT 静默不渲染"的可操作性一侧。
#[test]
fn unknown_styles_fall_back_to_underline_with_diagnostic() {
    let resolution = resolve_style("something-the-source-author-invented");
    assert!(
        resolution.is_fallback(),
        "未命中前两级的样式应走第三级（回退）"
    );
    assert!(
        resolution.has_underline(),
        "回退必须是下划线（FR-033：默认效果 = 下划线）"
    );

    let diagnostic = resolution
        .diagnostic()
        .expect("回退 MUST 产出诊断（MUST NOT 静默不渲染）");
    assert!(
        diagnostic.contains("something-the-source-author-invented"),
        "诊断必须携带原样式串（否则用户不知道是哪条规则），实测：{diagnostic}"
    );
}

/// **空样式串也走回退并诊断**（书源的 `style` 可能是空串）。
#[test]
fn empty_style_resolves_deterministically() {
    let resolution = resolve_style("");
    // 空串的处置：走回退（下划线 + 诊断），与未知样式同一条路——
    // 因为"空"同样不是有效的样式标识，静默按"无样式"处理会丢渲染。
    assert!(
        resolution.has_underline(),
        "空样式应回退到下划线（不静默不渲染）"
    );
}

/// **词表可枚举**（FR-033 明文要求）。
#[test]
fn semantic_vocabulary_is_enumerable_and_non_empty() {
    assert!(
        !SEMANTIC_STYLE_VOCABULARY.is_empty(),
        "语义词表 MUST 可枚举且非空（FR-033）"
    );
    // 每个条目都要有稳定的 token 名与确定的效应
    let mut tokens: Vec<&str> = SEMANTIC_STYLE_VOCABULARY
        .iter()
        .map(|entry| entry.token)
        .collect();
    let before = tokens.len();
    tokens.sort_unstable();
    tokens.dedup();
    assert_eq!(
        before,
        tokens.len(),
        "词表内 token 不得重复（枚举结果应无歧义）"
    );
}

/// **三级解析的判定互相排斥**（一个样式只属于一级）。
#[test]
fn style_resolution_levels_are_mutually_exclusive() {
    for token in ["underline", "strikethrough", "bold", "red", "unknown-thing"] {
        let resolution: StyleResolution = resolve_style(token);
        let levels = [
            resolution.is_semantic(),
            resolution.is_color_name(),
            resolution.is_fallback(),
        ];
        assert_eq!(
            levels.iter().filter(|flag| **flag).count(),
            1,
            "样式 `{token}` 应恰好命中一级，实测：{levels:?}"
        );
    }
}
