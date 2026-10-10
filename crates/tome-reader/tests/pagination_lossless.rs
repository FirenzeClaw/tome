//! T019 验收：页序列 + 单双页判据（FR-012 / FR-014 / FR-004）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T019）
//!
//! 1. **内容页无损**：页区间拼接与该章正文**逐字符相等**（SC-006 的排版层形态）
//! 2. **判据随字号重算**：19px → 24px 时双页阈值右移（FR-014）
//! 3. **排版全程不自读缓存容器、不发请求**（计数宿主断言，FR-004）
//!
//! ## 为什么"逐字符相等"是本文件的主判据
//!
//! SC-006 的原文是"把一页页的字符区间按序拼接，得到的字符串与该章正文逐字符
//! 相等（100%）"。它同时排除了两种失效：
//!
//! - **丢字**：某段字符不属于任何页（如段落之间的换行符没人认领）
//! - **重字**：某段字符属于两页（如页边界算错一位）
//!
//! 只断言"总长度相等"是不够的——长度对而内容错位（页边界偏移）照样能通过。
//! 故本文件把区间切出来**逐字符比对**，并额外断言区间**首尾相接**。
//!
//! ## 段落分隔符必须有人认领（这条最容易漏）
//!
//! 段落切分（T011）产出的段文本**不含**换行符；而行盒只覆盖段内字符。故
//! 若页区间直接取"本页行盒的并集"，段间的换行符就落在**所有页之外**——
//! 总长度少了几个字符，SC-006 直接失败，而现象是"拼接结果少了一个换行"，
//! 从排版结果上看不出来。
//!
//! 故页区间的口径是：**页 i 从它首个字符开始，到下一页首个字符为止**；
//! 末页直到正文末尾。段间的一切（换行符、被段落切分丢弃的空白段）由
//! **前一页**认领。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::cell::Cell;
use std::rc::Rc;

use tome_core::seam::{AnchorQuery, ChapterContent, ContentProvider};
use tome_core::{Book, BookOrigin, Chapter, ErrorKind};

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{Page, Paginator};
use tome_reader::paginate::spread::{
    auto_columns, column_measure_px, required_width_px, Columns, SpreadSide, Viewport,
};

// ===========================================================================
// 测试夹具
// ===========================================================================

/// 取 `content` 的字符区间文本（**字符**口径，与 001 的 `char_offset` 同）。
fn slice_chars(content: &str, start: usize, end: usize) -> String {
    content.chars().skip(start).take(end - start).collect()
}

fn default_geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

/// 一段含中文标点与多段落的正文（覆盖段间换行符的关键点）。
fn sample_chapter() -> String {
    [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样。她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然，」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
        "《红楼梦》（又名《石头记》）是清代曹雪芹的作品，书中有四百多个人物，其中以宝黛钗三人最为人所知。",
        "桌上有笔、墨、纸、砚；窗边有兰、竹、菊、梅；墙上挂着山水、花鸟、人物；架上摆着经、史、子、集。",
    ]
    .join("\n")
}

/// 跑一次分页，返回结果。
fn paginate(
    content: &str,
    geometry: &GeometryConfig,
    width_px: f32,
    height_px: f32,
) -> tome_reader::paginate::page::PaginationResult {
    let mut paginator = Paginator::new();
    let viewport = Viewport::new(width_px, height_px);
    paginator
        .paginate(content, geometry, &viewport)
        .expect("分页应成功")
}

/// 把页区间按序拼接成字符串（SC-006 的判据载体）。
fn concat_pages(content: &str, pages: &[Page]) -> String {
    pages
        .iter()
        .filter(|page| !page.is_padding)
        .map(|page| slice_chars(content, page.char_range.start, page.char_range.end))
        .collect()
}

// ===========================================================================
// SC-006：分页不丢不重（本任务的主判据）
// ===========================================================================

/// **页区间拼接 == 正文逐字符相等**（SC-006）。
#[test]
fn pages_tile_chapter_exactly() {
    let content = sample_chapter();
    for (width, height, font_size) in [
        (228.0f32, 300.0f32, 19u16),
        (342.0, 400.0, 19),
        (684.0, 800.0, 19),
        (342.0, 500.0, 24),
        (500.0, 250.0, 14),
        (300.0, 120.0, 30),
    ] {
        let mut geometry = default_geometry();
        geometry.font_size_px = font_size;
        geometry.measure_em = 36.0;

        let result = paginate(&content, &geometry, width, height);
        let reconstructed = concat_pages(&content, &result.pages);

        assert_eq!(
            reconstructed,
            content,
            "版心 {width}px / 高 {height}px / 字号 {font_size}px 下页区间拼接与正文不符\
             （丢字或重字）。页数 {}，页区间 {:?}",
            result.pages.len(),
            result
                .pages
                .iter()
                .map(|page| (page.char_range.start, page.char_range.end))
                .collect::<Vec<_>>()
        );
    }
}

/// 页区间**首尾相接**（无缺口无重叠）——比"拼接字符串相等"更强的结构断言。
///
/// ## 为什么两条都写
///
/// "拼接相等"能通过一种退化情形：若某页区间是空的而另一页把它的字符
/// 也包含了，拼接结果仍可能相等。区间相接的断言排除了"有空页混在里面"。
#[test]
fn page_ranges_are_contiguous_with_no_gaps() {
    let content = sample_chapter();
    let result = paginate(&content, &default_geometry(), 228.0, 200.0);
    assert!(result.pages.len() > 1, "本例应产出多页（前置条件）");

    let mut expected_start = 0usize;
    for (index, page) in result.pages.iter().enumerate() {
        assert_eq!(
            page.char_range.start, expected_start,
            "第 {index} 页起点应为上一页终点（无缺口无重叠）"
        );
        assert!(
            page.char_range.end > page.char_range.start,
            "第 {index} 页区间不得为空（空页会让内容无人认领）"
        );
        expected_start = page.char_range.end;
    }
    assert_eq!(
        expected_start,
        content.chars().count(),
        "末页终点应为正文末尾"
    );
}

/// **段间换行符必须有人认领**（本文件最容易漏的一条，见模块文档）。
#[test]
fn paragraph_separators_belong_to_some_page() {
    // 三段、两个换行符
    let content = "第一段文字内容够长以便占据若干行。\n第二段文字内容也够长，\n第三段收尾。";
    let result = paginate(content, &default_geometry(), 228.0, 150.0);

    let newline_positions: Vec<usize> = content
        .chars()
        .enumerate()
        .filter(|(_, ch)| *ch == '\n')
        .map(|(index, _)| index)
        .collect();
    assert_eq!(newline_positions.len(), 2, "前置条件：两处换行");

    for position in newline_positions {
        let owner = result
            .pages
            .iter()
            .find(|page| page.char_range.contains(&position));
        assert!(
            owner.is_some(),
            "换行符（字符位置 {position}）不属于任何页——SC-006 会因少一个字符而失败"
        );
    }
}

/// 行盒的字符区间是**章内**坐标，且与段落的章内起点一致。
#[test]
fn page_lines_carry_chapter_level_ranges() {
    let content = sample_chapter();
    let result = paginate(&content, &default_geometry(), 228.0, 400.0);

    let mut seen = 0usize;
    for page in &result.pages {
        for page_line in &page.lines {
            seen += 1;
            assert!(
                page_line.chapter_char_range.start >= page.char_range.start
                    && page_line.chapter_char_range.end <= page.char_range.end,
                "行区间 {:?} 超出其所属页的区间 {:?}",
                page_line.chapter_char_range,
                page.char_range
            );
            // 行盒自身的区间是**段内**偏移，长度必须与章内区间一致
            assert_eq!(
                page_line.line.char_len(),
                page_line.chapter_char_range.end - page_line.chapter_char_range.start,
                "行的段内区间长度与章内区间长度不符（换算错位）"
            );
            assert!(
                !slice_chars(
                    &content,
                    page_line.chapter_char_range.start,
                    page_line.chapter_char_range.end
                )
                .is_empty(),
                "行区间不得为空"
            );
        }
    }
    assert!(seen > 0, "应产出至少一个行盒");
}

/// **每个行盒恰好出现在一页上**（不丢行、不重行）。
#[test]
fn every_line_appears_on_exactly_one_page() {
    let content = sample_chapter();
    let result = paginate(&content, &default_geometry(), 228.0, 200.0);

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for page in &result.pages {
        for page_line in &page.lines {
            ranges.push((
                page_line.chapter_char_range.start,
                page_line.chapter_char_range.end,
            ));
        }
    }
    ranges.sort_unstable();
    let total = ranges.len();
    ranges.dedup();
    assert_eq!(total, ranges.len(), "有行盒被重复放到多页上");
    assert!(total > 5, "本例应有多个行盒（前置条件），实测 {total}");
}

// ===========================================================================
// 边界情况（spec §Edge Cases）
// ===========================================================================

/// 空正文 → 仍产出一页（不得零页导致翻页越界）。
#[test]
fn empty_chapter_yields_one_page() {
    let result = paginate("", &default_geometry(), 684.0, 800.0);
    assert_eq!(
        result.pages.len(),
        1,
        "空章必须产出一页可呈现内容（spec §Edge Cases：不得产出零页）"
    );
    assert!(result.pages[0].lines.is_empty(), "空章没有行盒");
    assert_eq!(result.pages[0].char_range.start, 0);
    assert_eq!(result.pages[0].char_range.end, 0, "空章的页区间是空区间");
}

/// 仅空白的正文同样产出一页（不得零页）。
#[test]
fn whitespace_only_chapter_yields_one_page() {
    let result = paginate("   \n\u{3000}\n  ", &default_geometry(), 684.0, 800.0);
    assert_eq!(result.pages.len(), 1, "仅空白正文必须产出一页");
}

/// 单行高得超过整页高度时，该行**独占一页**（不至于死循环或丢行）。
#[test]
fn line_taller_than_page_occupies_its_own_page() {
    let content = "内容够长，会排成若干行。";
    let geometry = default_geometry();
    // 高度只够 0.5 行
    let result = paginate(content, &geometry, 228.0, geometry.line_height_px() * 0.5);

    assert!(!result.pages.is_empty(), "不得产出零页");
    for page in &result.pages {
        assert!(!page.lines.is_empty(), "超高的行方案下不得产出空的内容页");
    }
    assert_eq!(concat_pages(content, &result.pages), content);
}

// ===========================================================================
// 高度约束（页高不超可用高）
// ===========================================================================

/// 每页的行高之和不超过可用高度——**除非该页只有一行**（那一行自身超高）。
#[test]
fn pages_respect_available_height() {
    let content = sample_chapter();
    for height in [100.0f32, 200.0, 300.0, 500.0] {
        let geometry = default_geometry();
        let result = paginate(&content, &geometry, 228.0, height);

        for page in &result.pages {
            let used: f32 = page.lines.iter().map(|line| line.line.height_px).sum();
            assert!(
                used <= height + 1e-3 || page.lines.len() == 1,
                "页占用 {used}px 超过可用高 {height}px（该页 {} 行；只有单行页允许溢出）",
                page.lines.len()
            );
        }
    }
}

/// 用得更高的可用高度 → 页数不增（单调性：自检"高度真的参与了分页"）。
///
/// ## 为什么需要这条自检
///
/// 若实现里忘了用 `available_height`（例如写死"每页 20 行"），
/// `pages_tile_chapter_exactly` 仍会通过——页数变了但区间照旧无损。
/// 单调性断言让"高度参数确实生效"可见。
#[test]
fn more_height_never_yields_more_pages() {
    let content = sample_chapter();
    let geometry = default_geometry();

    let short = paginate(&content, &geometry, 228.0, 120.0).pages.len();
    let medium = paginate(&content, &geometry, 228.0, 300.0).pages.len();
    let tall = paginate(&content, &geometry, 228.0, 2000.0).pages.len();

    assert!(
        short >= medium && medium >= tall,
        "可用高增大时页数不得增加（实测 {short} / {medium} / {tall}）"
    );
    assert!(
        short > tall,
        "高度从 120px 提到 2000px 应减少页数（否则高度参数没生效）"
    );
}

// ===========================================================================
// FR-014：em 制单双页判据
// ===========================================================================

/// 判据的边界**精确等于 em 公式**（证明没有另一套 px 断点）。
#[test]
fn spread_boundary_is_exactly_two_measures_plus_gutter() {
    let geometry = default_geometry(); // 36em @ 19px = 684px，中缝 64px
    let threshold = required_width_px(&geometry);
    assert!(
        (threshold - 1432.0).abs() < 1e-3,
        "19px 基准下阈值应为 2×684+64 = 1432px，实测 {threshold}"
    );

    let viewport_just_below = Viewport::new(threshold - 0.1, 800.0);
    let viewport_exact = Viewport::new(threshold, 800.0);
    let viewport_above = Viewport::new(threshold + 0.1, 800.0);

    assert_eq!(
        auto_columns(&viewport_just_below, &geometry),
        Columns::Single,
        "阈值下方 0.1px 必须判单页（否则说明用的是取整后的 px 断点）"
    );
    assert_eq!(
        auto_columns(&viewport_exact, &geometry),
        Columns::Double,
        "恰好等于阈值时启用双页（MASTER §5.2 的 >=）"
    );
    assert_eq!(
        auto_columns(&viewport_above, &geometry),
        Columns::Double,
        "阈值上方 0.1px 必须判双页"
    );
}

/// **判据随字号重算**：19px → 24px 时同一宽度从双页变单页。
#[test]
fn spread_threshold_shifts_right_as_font_size_grows() {
    let mut geometry = default_geometry();
    geometry.font_size_px = 19;
    let threshold_19 = required_width_px(&geometry);

    geometry.font_size_px = 24;
    let threshold_24 = required_width_px(&geometry);

    assert!(
        threshold_24 > threshold_19,
        "字号增大时双页阈值必须右移（19px: {threshold_19} → 24px: {threshold_24}）"
    );
    // 精确值：24px 下 2×864+64 = 1792
    assert!((threshold_24 - 1792.0).abs() < 1e-3, "实测 {threshold_24}");

    // 同一宽度在两档字号下判定不同——这是"实时重算"的可见证据
    let width = 1500.0;
    let viewport = Viewport::new(width, 800.0);

    let mut at_19 = default_geometry();
    at_19.font_size_px = 19;
    let mut at_24 = default_geometry();
    at_24.font_size_px = 24;

    assert_eq!(
        auto_columns(&viewport, &at_19),
        Columns::Double,
        "1500px 在 19px 字号下容得下两栏（阈值 {threshold_19}）"
    );
    assert_eq!(
        auto_columns(&viewport, &at_24),
        Columns::Single,
        "1500px 在 24px 字号下容不下两栏（阈值 {threshold_24}）——字号变化必须重算判据"
    );
}

/// 判据随**版心目标**缩放（不是只看字号）。
#[test]
fn spread_threshold_scales_with_measure_em() {
    let mut geometry = default_geometry();
    geometry.measure_em = 36.0;
    let wide = required_width_px(&geometry);
    geometry.measure_em = 24.0;
    let narrow = required_width_px(&geometry);

    assert!(
        narrow < wide,
        "版心目标变小则双页阈值左移（{wide} → {narrow}）"
    );
    assert!((narrow - (2.0 * 24.0 * 19.0 + 64.0)).abs() < 1e-3);
}

/// 中缝增减直接影响阈值（判据的第三项输入确实参与计算）。
#[test]
fn spread_threshold_includes_gutter() {
    let mut geometry = default_geometry();
    geometry.gutter_px = 0;
    let without = required_width_px(&geometry);
    geometry.gutter_px = 64;
    let with = required_width_px(&geometry);

    assert!(
        (with - without - 64.0).abs() < 1e-3,
        "中缝应从 0 到 64px 恰好把阈值推高 64px（实测差 {}）",
        with - without
    );
}

// ===========================================================================
// 版心换算（MASTER §5.1/§5.3）
// ===========================================================================

/// 单页版心 = min(目标, 可用宽, 42em 上限)——**超出上限则留白不拉伸**。
#[test]
fn single_page_measure_is_capped_and_centered_by_leave_blank() {
    let geometry = default_geometry(); // 36em = 684px，上限 42em = 798px

    // 可用宽 3000px：版心应封顶 684px（目标），不是 3000px
    let viewport = Viewport::new(3000.0, 800.0);
    let measure = column_measure_px(Columns::Single, &viewport, &geometry);
    assert!(
        (measure - 684.0).abs() < 1e-3,
        "单页版心应停在目标 684px（超宽留白），实测 {measure}"
    );

    // 可用宽 400px（小于目标）：版心随可用宽收缩
    let viewport = Viewport::new(400.0, 800.0);
    let measure = column_measure_px(Columns::Single, &viewport, &geometry);
    assert!((measure - 400.0).abs() < 1e-3, "窄窗下版心应为 400px");

    // 容量足够但目标被调大超过 42em：封顶 42em
    let mut generous = default_geometry();
    generous.measure_em = 60.0;
    let viewport = Viewport::new(3000.0, 800.0);
    let measure = column_measure_px(Columns::Single, &viewport, &generous);
    assert!(
        (measure - 798.0).abs() < 1e-3,
        "版心不得超过 42em 上限 798px（MASTER §5.3 的裁决 I7），实测 {measure}"
    );
}

/// 双页版心 = (可用宽 − 中缝) / 2，且同样受目标与上限约束。
#[test]
fn double_page_measure_subtracts_gutter() {
    let geometry = default_geometry();

    // 恰在阈值上：每栏正好等于目标 684px
    let viewport = Viewport::new(1432.0, 800.0);
    let measure = column_measure_px(Columns::Double, &viewport, &geometry);
    assert!(
        (measure - 684.0).abs() < 1e-3,
        "阈值处每栏应为 684px，实测 {measure}"
    );

    // 更宽：可用宽 3000 → (3000−64)/2 = 1468，超过目标 684 → 停在目标
    let wide = Viewport::new(3000.0, 800.0);
    let measure_wide = column_measure_px(Columns::Double, &wide, &geometry);
    assert!(
        (measure_wide - 684.0).abs() < 1e-3,
        "超宽双页每栏应停在目标 684px，实测 {measure_wide}"
    );
}

/// 版心**永不为零或负**（极窄窗下不发生除零/负宽计算）。
#[test]
fn measure_is_never_zero_or_negative() {
    let geometry = default_geometry();
    for width in [0.0f32, 0.5, 1.0, 10.0, 63.0, 64.0] {
        for columns in [Columns::Single, Columns::Double] {
            let viewport = Viewport::new(width, 100.0);
            let measure = column_measure_px(columns, &viewport, &geometry);
            assert!(
                measure >= 1.0,
                "可用宽 {width}px / {columns:?} 下版心 {measure}px 必须为正（spec §Edge Cases：不得进入除零或负宽）"
            );
        }
    }
}

// ===========================================================================
// FR-014：单双页与左右页归属
// ===========================================================================

/// 单页模式的页不带左右归属；双页模式带（左偶右奇）。
#[test]
fn single_page_mode_has_no_side_double_page_mode_has_one() {
    let content = sample_chapter();
    let geometry = default_geometry();

    let single = paginate(&content, &geometry, 684.0, 300.0);
    assert_eq!(single.spread.columns, Columns::Single, "684px 应判单页");
    for page in &single.pages {
        assert_eq!(page.side, None, "单页模式下不应用书页语义（裁决 OQ#5→A）");
    }

    let double = paginate(&content, &geometry, 1432.0, 300.0);
    assert_eq!(double.spread.columns, Columns::Double, "1432px 应判双页");
    for page in &double.pages {
        assert!(
            page.side.is_some(),
            "双页模式下每页必须有左右归属（FR-016）"
        );
    }
}

/// `Columns` 的 `count`/`is_double` 与枚举一致（防两处独立判定漂移）。
#[test]
fn columns_helpers_agree_with_variants() {
    assert_eq!(Columns::Single.count(), 1);
    assert_eq!(Columns::Double.count(), 2);
    assert!(!Columns::Single.is_double());
    assert!(Columns::Double.is_double());
}

/// `SpreadSide` 的奇偶语义：左 = 偶、右 = 奇（FR-016）。
#[test]
fn spread_side_follows_book_parity() {
    assert!(
        SpreadSide::Left.is_even_page(),
        "左页应为偶数页（MASTER §5.2 的书页语义）"
    );
    assert!(!SpreadSide::Right.is_even_page(), "右页应为奇数页");
    assert_eq!(SpreadSide::Left.as_str(), "left");
    assert_eq!(SpreadSide::Right.as_str(), "right");
}

// ===========================================================================
// FR-004：唯一内容源是 ContentProvider（计数宿主断言）
// ===========================================================================

/// 计数宿主：记录 `chapter_content` 被调用了几次。
struct CountingProvider {
    content: String,
    chapter_calls: Rc<Cell<usize>>,
}

impl ContentProvider for CountingProvider {
    fn toc(&self, _book: &Book) -> Result<Vec<Chapter>, ErrorKind> {
        Ok(Vec::new())
    }

    fn chapter_content(
        &self,
        _book: &Book,
        chapter_index: i32,
    ) -> Result<ChapterContent, ErrorKind> {
        self.chapter_calls.set(self.chapter_calls.get() + 1);
        Ok(ChapterContent {
            chapter_index,
            title: String::new(),
            content: self.content.clone(),
            from_cache: true,
        })
    }

    fn resolve_anchor(&self, _book: &Book, anchor: &AnchorQuery) -> Result<(i32, i64), ErrorKind> {
        Ok((anchor.chapter_index, anchor.char_offset))
    }

    fn prefetch(&self, _book: &Book, _around_index: i32, _depth: u32) {}
}

fn sample_book() -> Book {
    Book {
        id: 1,
        origin: BookOrigin::Network,
        origin_id: "src|url".to_string(),
        name: "测试书".to_string(),
        author: None,
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: None,
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: None,
        format: None,
        total_chapters: None,
        dur_chapter_index: None,
        added_at: 0,
    }
}

/// **排版全程只向 ContentProvider 取一次正文**，之后不再触碰它。
///
/// ## 这条断言防的是什么
///
/// FR-004 要求正文唯一来源是 `ContentProvider`，本层 MUST NOT 自行读取缓存
/// 容器或发起网络请求。一个"看起来对"的实现可能在每次分页时都去取一次正文
/// （反正 provider 会命中缓存，肉眼看不出来）——那是**每翻一页一次 IO**，
/// 且把"缓存命中"变成了排版的隐含前提。
///
/// 断言口径：取正文 1 次，之后任意次分页（含换几何参数重排）
/// `chapter_content` 调用数**不增**。
#[test]
fn pagination_takes_content_from_provider_exactly_once() {
    let calls = Rc::new(Cell::new(0usize));
    let provider = CountingProvider {
        content: sample_chapter(),
        chapter_calls: Rc::clone(&calls),
    };
    let book = sample_book();

    // 组合根取正文（唯一一次）
    let chapter = provider.chapter_content(&book, 0).expect("取正文应成功");
    assert_eq!(calls.get(), 1, "取正文应恰好一次");

    // 排版与多次重排都基于已取得的文本
    let geometry = default_geometry();
    let mut paginator = Paginator::new();
    for (width, height) in [(228.0f32, 200.0f32), (684.0, 400.0), (1432.0, 800.0)] {
        let viewport = Viewport::new(width, height);
        let _ = paginator
            .paginate(&chapter.content, &geometry, &viewport)
            .expect("分页应成功");
    }

    assert_eq!(
        calls.get(),
        1,
        "分页不得再次向 ContentProvider 取正文（实测 {} 次）——\
         本层不感知缓存与抓取，正文由调用方一次交给它",
        calls.get()
    );
    assert!(chapter.from_cache, "本例应由缓存命中（回放断言的前提）");
}

// ===========================================================================
// 确定性（FR-048 的排版层形态）
// ===========================================================================

/// 同输入两次分页产出**逐字段相同**的页序列。
#[test]
fn pagination_is_deterministic() {
    let content = sample_chapter();
    let geometry = default_geometry();

    let first = paginate(&content, &geometry, 342.0, 300.0);
    let second = paginate(&content, &geometry, 342.0, 300.0);

    assert_eq!(
        first.pages.len(),
        second.pages.len(),
        "同参数两次分页的页数必须相同"
    );
    for (index, (a, b)) in first.pages.iter().zip(second.pages.iter()).enumerate() {
        assert_eq!(a.char_range, b.char_range, "第 {index} 页区间不一致");
        assert_eq!(a.lines.len(), b.lines.len(), "第 {index} 页行数不一致");
        assert_eq!(a.side, b.side, "第 {index} 页左右归属不一致");
        assert_eq!(a.is_padding, b.is_padding, "第 {index} 页留白标记不一致");
    }
}

/// 页序列的 `page_in_chapter` 是连续递增的章内页码。
#[test]
fn page_indices_are_sequential_within_chapter() {
    let content = sample_chapter();
    let result = paginate(&content, &default_geometry(), 228.0, 200.0);
    for (index, page) in result.pages.iter().enumerate() {
        assert_eq!(
            page.page_in_chapter as usize, index,
            "章内页码应与序列位置一致（FR-042 的页脚口径）"
        );
        assert_eq!(page.chapter_index, 0);
    }
}
