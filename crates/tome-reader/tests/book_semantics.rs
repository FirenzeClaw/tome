//! T021 验收：书页语义（FR-016，裁决 OQ#5→A / OQ#20→C）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T021）
//!
//! - 双页章起位置测试：上章末单页时留白补足，留白页 `is_padding=true`
//!   显式标记且**不计入内容区间**——**SC-006 的完整口径在此补齐**
//! - 开关关闭时无留白；单页模式断言无留白（裁决 OQ#5）
//!
//! ## 书页语义是什么（FR-016）
//!
//! > 在**双页模式**下遵循书页语义：左页为偶数页、右页为奇数页，且章节
//! > MUST 从偶数页（左页）开始——若上一章结束于单页，系统 MUST 留白补足
//! > 至偶数页再起新章。该行为 MUST 可关闭，关闭入口 MUST 是**命名的设置项
//! > 「章节从新页开始」**。**单页模式下 MUST NOT 应用该留白**（裁决 OQ#5→A）。
//!
//! ## SC-006 的"完整口径"为什么要在这里补
//!
//! SC-006 说的是：
//!
//! > 分页结果不丢字不重字：把一页页的字符区间按序拼接，得到的字符串与该章
//! > 正文逐字符相等（100%）。**留白补足产生的空白页不计入内容区间，但必须
//! > 被显式标记为"书页语义留白"而非内容页**。
//!
//! 括号里那句是本任务独有的：T019 的 `pagination_lossless.rs` 证明了内容页
//! 无损，但那时还没有留白页。留白页引入了**空的字符区间**——若它与内容页
//! 无法区分，SC-006 的拼接就会多出零长度片段（无害）或有人把留白当内容页
//! 而误算字数（有害）。
//!
//! 故本文件断言三件事：
//!
//! 1. 留白页是**空区间**（不认领任何字符）
//! 2. 留白页 `is_padding = true`（可被消费者识别并跳过）
//! 3. **排除留白页后**的区间拼接仍与正文逐字符相等（SC-006 的完整口径）

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_reader::paginate::booksem::{
    apply_book_semantics, padding_needed_for_chapter_start, BookSemantics,
};
use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{Page, Paginator};
use tome_reader::paginate::spread::{Columns, SpreadSide, Viewport};

// ===========================================================================
// 夹具
// ===========================================================================

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

/// 双页几何（默认字号下阈值 1432px，用 1432 恰好双页）。
fn double_geometry(chapter_new_page: bool) -> GeometryConfig {
    let mut config = geometry();
    config.chapter_new_page = chapter_new_page;
    config
}

fn double_viewport() -> Viewport {
    Viewport::new(1432.0, 300.0)
}

fn single_viewport() -> Viewport {
    Viewport::new(800.0, 300.0)
}

/// 一段能排成若干页的正文。
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

fn paginate(
    content: &str,
    geometry: &GeometryConfig,
    viewport: &Viewport,
) -> tome_reader::paginate::page::PaginationResult {
    let mut paginator = Paginator::new();
    paginator
        .paginate(content, geometry, viewport)
        .expect("分页应成功")
}

/// 排除留白页后的区间拼接（SC-006 的完整口径）。
fn concat_content_pages(content: &str, pages: &[Page]) -> String {
    pages
        .iter()
        .filter(|page| !page.is_padding)
        .map(|page| {
            content
                .chars()
                .skip(page.char_range.start)
                .take(page.char_range.end - page.char_range.start)
                .collect::<String>()
        })
        .collect()
}

// ===========================================================================
// 留白补足的计算（纯函数）
// ===========================================================================

/// **上一章占奇数页**（0 起序号为偶）→ 无需补足。
///
/// ## 口径：0 起页序号下，"偶数页"即左页
///
/// 见 `spread::SpreadSide::for_page_index` 的文档。上一章页数 `n` 表示它
/// 占用序号 `0..n`。新章从序号 `n` 开始，它要是**左页**（偶数号）。
#[test]
fn even_chapter_end_needs_no_padding() {
    // 上一章 4 页（序号 0..4），新章从序号 4 开始 —— 4 是偶数，左页
    assert_eq!(padding_needed_for_chapter_start(4), 0);
    assert_eq!(padding_needed_for_chapter_start(2), 0);
    assert_eq!(padding_needed_for_chapter_start(0), 0, "首章无需补足");
}

/// **上一章占偶数页**（序号 0..n，n 为奇）→ 补 1 页让新章落回左页。
#[test]
fn odd_chapter_end_needs_one_padding_page() {
    // 上一章 3 页（序号 0..3），新章会从序号 3 开始 —— 3 是奇数，右页
    // 故补 1 页，新章从序号 4 开始
    assert_eq!(padding_needed_for_chapter_start(3), 1);
    assert_eq!(padding_needed_for_chapter_start(1), 1);
    assert_eq!(padding_needed_for_chapter_start(5), 1);
    assert_eq!(padding_needed_for_chapter_start(99), 1);
}

/// 补足量**恒为 0 或 1**（不会出现"补两页"）。
///
/// ## 为什么这条是必要的
///
/// 书页语义只要求"新章落在左页"，故最多补一页。若实现写成了
/// `2 - (n % 2)` 之类的式子，在 `n` 为偶数时会算出 2——那会多出一页
/// 空白，且它的表现是"每隔一章多一页空白"，很难归因。
#[test]
fn padding_amount_is_never_more_than_one() {
    for pages in 0..50usize {
        let padding = padding_needed_for_chapter_start(pages);
        assert!(
            padding <= 1,
            "上一章 {pages} 页时补足量 {padding} 超过 1（最多补一页）"
        );
    }
}

/// 补足后新章的起始页序号**必为偶数**（左页）——这是补足的目的。
#[test]
fn chapter_start_lands_on_even_page_after_padding() {
    for pages in 0..50usize {
        let padding = padding_needed_for_chapter_start(pages);
        let start = pages + padding;
        assert!(
            start.is_multiple_of(2),
            "上一章 {pages} 页 + 补足 {padding} 页 = 起始序号 {start}，应是偶数（左页）"
        );
        assert_eq!(
            SpreadSide::for_page_index(start as u32),
            SpreadSide::Left,
            "新章首页必须是左页"
        );
    }
}

// ===========================================================================
// 双页模式：留白页真的出现且被标记
// ===========================================================================

/// 双页 + 开关开 → 上一章末为奇数页时，新章前补一页留白。
#[test]
fn double_page_mode_inserts_padding_before_chapter() {
    // 手工构造"上一章占 3 页"的局面：直接对 3 页的页序列应用书页语义
    let pages = vec![
        content_page(0, 0, 0..10),
        content_page(0, 1, 10..20),
        content_page(0, 2, 20..30),
    ];
    assert_eq!(pages.len(), 3, "前置：上一章 3 页（末页序号 2）");

    let semantics = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    let result = apply_book_semantics(&pages, semantics);

    assert_eq!(
        result.padding_before_chapter, 1,
        "3 页后新章会落在序号 3（右页），应补 1 页"
    );
    assert_eq!(result.pages.len(), 4, "补足后共 4 页");
    assert!(result.pages[3].is_padding, "第 4 页（序号 3）应是留白页");
    assert!(
        result.pages.iter().take(3).all(|page| !page.is_padding),
        "原有内容页的留白标记不得被改动"
    );
}

/// 留白页是**空区间**（不认领任何字符）。
#[test]
fn padding_page_has_empty_range() {
    let pages = vec![content_page(0, 0, 0..10)];
    let semantics = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    let result = apply_book_semantics(&pages, semantics);

    let padding = result
        .pages
        .iter()
        .find(|page| page.is_padding)
        .expect("应有留白页");
    assert!(
        padding.char_range.is_empty(),
        "留白页的字符区间必须是空的（不得认领字符）——SC-006 的口径依赖这一点，实测 {:?}",
        padding.char_range
    );
    assert!(padding.lines.is_empty(), "留白页没有行盒");
}

/// 留白页的左右归属由它的**位置**决定（它占一个页序号，参与左右交替）。
#[test]
fn padding_page_takes_its_side_from_position() {
    let pages = vec![content_page(0, 0, 0..10)]; // 1 页（序号 0 已用）
    let semantics = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    let result = apply_book_semantics(&pages, semantics);

    let padding = result
        .pages
        .iter()
        .find(|page| page.is_padding)
        .expect("应有留白页");
    assert_eq!(
        padding.side,
        Some(SpreadSide::for_page_index(1)),
        "留白页占序号 1，归属应按时序位置决定（它是右页）"
    );
}

// ===========================================================================
// 开关关闭 / 单页模式：无留白（裁决 OQ#5→A）
// ===========================================================================

/// **单页模式 MUST NOT 应用留白**（裁决 OQ#5→A），即使开关是开的。
#[test]
fn single_page_mode_never_pads_even_with_switch_on() {
    let pages = vec![content_page(0, 0, 0..10)]; // 奇数页数，双页下会补
    let with_switch_on = BookSemantics {
        columns: Columns::Single,
        chapter_new_page: true,
    };
    let result = apply_book_semantics(&pages, with_switch_on);

    assert_eq!(
        result.padding_before_chapter, 0,
        "单页模式不得应用书页语义（裁决 OQ#5→A）"
    );
    assert!(
        result.pages.iter().all(|page| !page.is_padding),
        "单页模式下不得产出留白页"
    );

    // 对照：同一输入在双页下**会**补——证明本测试不是因为"输入凑巧不触发"
    let double = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    assert_eq!(
        apply_book_semantics(&pages, double).padding_before_chapter,
        1,
        "前置对照：双页下同一输入应补一页（否则本测试是空判据）"
    );
}

/// **开关关闭时无留白**（FR-016 的"该行为 MUST 可关闭"）。
#[test]
fn switch_off_disables_padding() {
    let pages = vec![content_page(0, 0, 0..10)];
    let off = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: false,
    };
    let result = apply_book_semantics(&pages, off);

    assert_eq!(result.padding_before_chapter, 0);
    assert!(result.pages.iter().all(|page| !page.is_padding));

    // 对照：开关打开时同一输入会补
    let on = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    assert_eq!(apply_book_semantics(&pages, on).padding_before_chapter, 1);
}

// ===========================================================================
// SC-006 的完整口径（留白页口径在此补齐）
// ===========================================================================

/// **排除留白页后，区间拼接与正文逐字符相等**（SC-006 完整口径）。
#[test]
fn content_pages_still_tile_exactly_with_padding_present() {
    let content = chapter_text();

    // 让分页产出多页，再对结果应用书页语义（双页 + 开关开）
    let result = paginate(&content, &double_geometry(true), &double_viewport());
    assert!(result.pages.len() > 1, "前置：本例应产出多页");

    // 页数若为奇，书页语义会补一页（章节衔接）
    let semantics = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    let applied = apply_book_semantics(&result.pages, semantics);

    let reconstructed = concat_content_pages(&content, &applied.pages);
    assert_eq!(
        reconstructed, content,
        "排除留白页后区间拼接必须与正文逐字符相等（SC-006 完整口径）"
    );
}

/// 留白页**不认领字符**，故它不影响"总字符数"的任何统计。
#[test]
fn padding_does_not_consume_characters() {
    let content = chapter_text();
    let result = paginate(&content, &double_geometry(true), &double_viewport());

    let semantics = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    let applied = apply_book_semantics(&result.pages, semantics);

    let content_chars: usize = applied
        .pages
        .iter()
        .filter(|page| !page.is_padding)
        .map(|page| page.char_range.end - page.char_range.start)
        .sum();
    assert_eq!(
        content_chars,
        content.chars().count(),
        "内容页覆盖的字符总数应等于正文长度（留白页计入 0）"
    );

    let padding_chars: usize = applied
        .pages
        .iter()
        .filter(|page| page.is_padding)
        .map(|page| page.char_range.end - page.char_range.start)
        .sum();
    assert_eq!(padding_chars, 0, "留白页不得认领任何字符");
}

/// 留白页可被**显式识别**（SC-006 要求"必须被显式标记"）。
///
/// ## 这条断言的形式为什么是"数得出来"
///
/// SC-006 的括号句要求留白页"被显式标记为书页语义留白**而非内容页**"。
/// 一个把留白页与内容页混在一起的实现（如用一个零长度区间表示留白），
/// 会让消费者无法区分"这页是刻意为书页语义留的"与"这页恰好没内容"。
///
/// 故断言：存在一个 `is_padding == true` 的页，且**它是可数的**。
#[test]
fn padding_pages_are_explicitly_countable() {
    let result = paginate(&chapter_text(), &double_geometry(true), &double_viewport());
    let semantics = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    let applied = apply_book_semantics(&result.pages, semantics);

    let padding_count = applied.pages.iter().filter(|page| page.is_padding).count();
    let content_count = applied.pages.len() - padding_count;

    assert_eq!(
        padding_count, applied.padding_before_chapter,
        "留白页数应等于报告的补足量"
    );
    assert!(content_count > 0, "内容页必须存在");
    // 每一个不被标记的页都必须有内容区间（不得有"未标记的空页"混入内容页）
    for page in applied.pages.iter().filter(|page| !page.is_padding) {
        assert!(
            !page.char_range.is_empty(),
            "未标记为留白的页必须有内容区间（否则消费者会把它当内容页却读不到字）"
        );
    }
}

/// **端到端**：窄窗（单页模式）下真实分页 → 应用书页语义 → 零留白。
///
/// ## 与纯函数版测试的区别
///
/// 上面 `single_page_mode_never_pads_even_with_switch_on` 直接构造
/// `BookSemantics { columns: Single }` 调库函数。这条走**真实路径**：让
/// 分页器在窄窗下自己判定出单页，再把它的结果交给书页语义——从而覆盖
/// "分页判定与书页语义对 `columns` 的理解是否一致"。
///
/// 若两条路径对 `Columns` 的解读分叉（如分页说单页而书页语义读到了双页），
/// 纯函数版测试看不见，这条会红。
#[test]
fn end_to_end_single_page_pagination_yields_no_padding() {
    let content = chapter_text();
    let result = paginate(&content, &double_geometry(true), &single_viewport());

    assert_eq!(
        result.spread.columns,
        Columns::Single,
        "前置：800px 窄窗应判定单页（阈值 1432px）"
    );

    let semantics = BookSemantics {
        columns: result.spread.columns,
        // 用配置里真实的值（本例 `double_geometry(true)` 是开）
        chapter_new_page: double_geometry(true).chapter_new_page,
    };
    let applied = apply_book_semantics(&result.pages, semantics);

    assert_eq!(
        applied.padding_before_chapter, 0,
        "单页模式下即使开关打开也不得补白（裁决 OQ#5→A）"
    );
    assert!(applied.pages.iter().all(|page| !page.is_padding));
    // 内容仍无损
    assert_eq!(concat_content_pages(&content, &applied.pages), content);
}

/// **端到端**：宽窗（双页）下真实分页 → 应用书页语义 → 内容仍无损。
#[test]
fn end_to_end_double_page_pagination_keeps_content_lossless() {
    let content = chapter_text();
    let result = paginate(&content, &double_geometry(true), &double_viewport());
    assert_eq!(
        result.spread.columns,
        Columns::Double,
        "前置：1432px 应判定双页"
    );

    let semantics = BookSemantics {
        columns: result.spread.columns,
        chapter_new_page: true,
    };
    let applied = apply_book_semantics(&result.pages, semantics);

    // 双页下若页数为奇会补一页；无论补不补，内容都必须无损
    assert_eq!(concat_content_pages(&content, &applied.pages), content);
    assert_eq!(
        applied.padding_before_chapter,
        usize::from(!result.pages.len().is_multiple_of(2)),
        "补足量应由首章页数的奇偶决定"
    );
}

// ===========================================================================
// 章节衔接的端到端形态
// ===========================================================================

/// 两章衔接：第一章奇数页 → 第二章首页**左页**。
#[test]
fn second_chapter_starts_on_left_page() {
    let first = paginate(&chapter_text(), &double_geometry(true), &double_viewport());
    let semantics = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    let applied = apply_book_semantics(&first.pages, semantics);

    let chapter_two_start = applied.pages.len();
    assert!(
        chapter_two_start.is_multiple_of(2),
        "补足后第二章的起始序号 {chapter_two_start} 应是偶数（左页）"
    );
    assert_eq!(
        SpreadSide::for_page_index(chapter_two_start as u32),
        SpreadSide::Left
    );
    assert_eq!(
        applied.padding_before_chapter,
        usize::from(!first.pages.len().is_multiple_of(2)),
        "补足量应与首章页数的奇偶一致"
    );
}

/// 首章（前面没有别的内容）**不应**被补白——它的首页本就是全书的第 0 页。
///
/// ## 判据的形式
///
/// `apply_book_semantics` 的输入是"**前面章节**的页序列"，故它处理的是
/// "这一章前面要补多少"。首章前面没有章节，补足量必然是 0——由
/// `padding_needed_for_chapter_start(0) == 0` 保证。
#[test]
fn first_chapter_gets_no_leading_padding() {
    assert_eq!(padding_needed_for_chapter_start(0), 0);
    let empty: Vec<Page> = Vec::new();
    let semantics = BookSemantics {
        columns: Columns::Double,
        chapter_new_page: true,
    };
    let applied = apply_book_semantics(&empty, semantics);
    assert_eq!(applied.padding_before_chapter, 0);
    assert!(applied.pages.is_empty());
}

// ===========================================================================
// 辅助：构造页
// ===========================================================================

/// 造一个内容页（带一个假行盒，区间的长度与行盒一致）。
fn content_page(chapter_index: usize, page_in_chapter: u32, range: std::ops::Range<usize>) -> Page {
    use tome_reader::layout::linebox::LineBox;
    use tome_reader::paginate::page::PageLine;

    let line = LineBox {
        paragraph_index: 0,
        char_range: 0..range.len(),
        glyphs: Vec::new(),
        height_px: 35.15,
        baseline_px: 28.0,
        squeeze_px: 0.0,
        justify_extra_px: 0.0,
        indent_px: 0.0,
        rule_exception: None,
    };
    Page {
        chapter_index,
        page_in_chapter,
        char_range: range.clone(),
        lines: vec![PageLine {
            line,
            chapter_char_range: range,
        }],
        side: Some(SpreadSide::for_page_index(page_in_chapter)),
        is_padding: false,
    }
}
