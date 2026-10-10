//! [DONE] 书页语义（T021 / FR-016，裁决 OQ#5→A、OQ#20→C）
//!
//! ## 书页语义是什么
//!
//! 纸质书翻开时看到的是**对开的两个页码**。中文书的页码约定是：
//!
//! ```text
//!         ┌──────────┬──────────┐
//!         │  左页     │  右页     │
//!         │  偶数     │  奇数     │
//!         └──────────┴──────────┘
//!          4    3      2    1      ← 页码
//! ```
//!
//! 故"章节从偶数页（左页）开始"意味着**翻开新章时它总在左边**——一个读者
//! 翻页找章节时会自然落到的那一侧。若上一章结束在右页（奇），新章就会从
//! 左页之前的位置开始；此时留白补足一页，让新章仍从左页起。
//!
//! ## 两个"必须可关/不可应用"的约束
//!
//! 1. **可关闭**（FR-016）：命名设置项「章节从新页开始」（裁决 OQ#20→C——
//!    桌面端无长按手势语义，MUST NOT 以长按作开关）
//! 2. **单页模式 MUST NOT 应用**（裁决 OQ#5→A）：单页没有"对开"这回事，
//!    留白只会产生莫名其妙的空白页
//!
//! ## 本模块的输入形状（为什么不自己分页）
//!
//! [`apply_book_semantics`] 接受**已有章节的页序列**并产出补足后的序列。
//! 它不做分页——分页是 `page` 模块的职责。这样切分的理由：
//!
//! - 补足逻辑是**纯页序列变换**，可脱离排版引擎单测（无需字体库）
//! - 跨章衔接需要"上一章的页数"，那是调用方（会话状态）持有的信息
//!
//! ## 页码口径的约定（与 `spread` 模块一致）
//!
//! 本层的页序号从 **0** 起。约定：序号**偶数为左页**（见
//! [`crate::paginate::spread::SpreadSide::for_page_index`]）。
//! 故"新章从偶数页开始"= 新章的起始序号是偶数。
//!
//! MASTER §5.2 的表述是"左页为偶数页、右页为奇数页"，若按 1 起页码理解，
//! 第 1 页应是右页——那与"章节从偶数页开始"合起来意味着章节从第 2 页起。
//! 本层的 0 起序号让"偶数序号 = 左页 = 章节起点"一条约定同时满足两者，
//! 且与 `contracts/cli.md` §2.1 的示例（`page_index: 0` ↔ `side: "left"`）
//! 一致。

use crate::paginate::page::Page;
use crate::paginate::spread::{Columns, SpreadSide};

/// 书页语义的配置（FR-016 的两个输入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookSemantics {
    /// 实际生效的栏数（单页下不应用书页语义，裁决 OQ#5→A）。
    pub columns: Columns,
    /// 命名设置项「章节从新页开始」的开关值。
    pub chapter_new_page: bool,
}

/// 书页语义的应用结果。
#[derive(Debug, Clone, PartialEq)]
pub struct BookSemanticsResult {
    /// 补足后的页序列（含留白页）。
    pub pages: Vec<Page>,
    /// 为让新章落到左页而补的留白页数（0 或 1）。
    ///
    /// 消费者可据此显示"本章前有 N 页留白"，或用于推导下一个章节的页码。
    pub padding_before_chapter: usize,
}

/// 上一章占 `previous_chapter_pages` 页时，为让新章从**左页**开始要补几页。
///
/// ## 判据
///
/// 新章的起始页序号 = `previous_chapter_pages`（0 起，紧接在上一章之后）。
/// 要求它为**偶数**（左页），故：
///
/// ```text
///   上一章 n 页 → 新章从序号 n 起
///   n 为偶数 → 已在左页 → 补 0
///   n 为奇数 → 在右页   → 补 1（新章从 n+1 起，n+1 为偶数）
/// ```
///
/// ## 为什么返回值是 `usize` 而不是 `bool`
///
/// 语义上"要不要补"是布尔问题，但**补足量**在更长的章节链上有意义
/// （调用方累加它来推导全书页码）。返回页数让两种用法都能表达；且它让
/// "最多补一页"这条性质可被断言（[`Self::apply`] 的测试钉住了这点）。
#[must_use]
pub fn padding_needed_for_chapter_start(previous_chapter_pages: usize) -> usize {
    if previous_chapter_pages.is_multiple_of(2) {
        0
    } else {
        1
    }
}

/// 给一章的页序列应用书页语义（FR-016）。
///
/// ## 输入与输出
///
/// `pages` 是**同一章**的页序列（未补足），`semantics` 是配置。返回补足后的
/// 序列——留白页**插在序列末尾**（它是"这一章之后、下一章之前"的补白）。
///
/// ## 为什么留白页插在末尾而不是开头
///
/// 留白是**上一章**欠下的（它结束在右页，导致下一章起不来）。把它记在
/// 上一章的末尾让"每章的页数与它在书中的位置"保持一对一——若记在下一章
/// 开头，下一章的 `page_in_chapter` 就要从 1 开始，而那个 1 的语义是
/// "本书第 N 页"还是"本章第 N 页"会立刻含混（FR-042 要求章内页码语义明确）。
///
/// ## 不应用的两种情形（都返回原序列）
///
/// 1. `columns == Single`：单页没有对开（裁决 OQ#5→A）
/// 2. `chapter_new_page == false`：用户关闭了该行为（FR-016）
///
/// ## 留白页的三个字段
///
/// | 字段 | 值 | 理由 |
/// |---|---|---|
/// | `char_range` | 空区间 | 不认领字符（SC-006 的口径依赖它） |
/// | `is_padding` | `true` | 显式标记（SC-006 要求"被显式标记而非内容页"） |
/// | `side` | 按时序位置 | 它占一个页序号，参与左右交替——否则它之后的页归属会整体错位 |
#[must_use]
pub fn apply_book_semantics(pages: &[Page], semantics: BookSemantics) -> BookSemanticsResult {
    let should_apply = semantics.columns.is_double() && semantics.chapter_new_page;

    if !should_apply {
        return BookSemanticsResult {
            pages: pages.to_vec(),
            padding_before_chapter: 0,
        };
    }

    let padding = padding_needed_for_chapter_start(pages.len());
    if padding == 0 {
        return BookSemanticsResult {
            pages: pages.to_vec(),
            padding_before_chapter: 0,
        };
    }

    let mut result = pages.to_vec();
    // 补足可能在多页时发生（当前判据最多 1 页，但循环让逻辑对 N 页也成立）
    for offset in 0..padding {
        let index = pages.len() + offset;
        let chapter_index = pages.last().map_or(0, |page| page.chapter_index);
        result.push(Page {
            chapter_index,
            page_in_chapter: index as u32,
            char_range: 0..0,
            lines: Vec::new(),
            side: Some(SpreadSide::for_page_index(index as u32)),
            is_padding: true,
        });
    }

    BookSemanticsResult {
        pages: result,
        padding_before_chapter: padding,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::linebox::LineBox;
    use crate::paginate::page::PageLine;

    /// 造一个内容页（一个假行盒，区间与行盒一致）。
    fn content_page(page_in_chapter: u32, len: usize) -> Page {
        Page {
            chapter_index: 0,
            page_in_chapter,
            char_range: 0..len,
            lines: vec![PageLine {
                line: LineBox {
                    paragraph_index: 0,
                    char_range: 0..len,
                    glyphs: Vec::new(),
                    height_px: 35.15,
                    baseline_px: 28.0,
                    squeeze_px: 0.0,
                    justify_extra_px: 0.0,
                    indent_px: 0.0,
                    rule_exception: None,
                },
                chapter_char_range: 0..len,
            }],
            side: Some(SpreadSide::for_page_index(page_in_chapter)),
            is_padding: false,
        }
    }

    fn pages(count: usize) -> Vec<Page> {
        (0..count)
            .map(|index| content_page(index as u32, 10))
            .collect()
    }

    fn double_on() -> BookSemantics {
        BookSemantics {
            columns: Columns::Double,
            chapter_new_page: true,
        }
    }

    #[test]
    fn padding_rule_is_parity_based() {
        assert_eq!(padding_needed_for_chapter_start(0), 0);
        assert_eq!(padding_needed_for_chapter_start(1), 1);
        assert_eq!(padding_needed_for_chapter_start(2), 0);
        assert_eq!(padding_needed_for_chapter_start(3), 1);
        assert_eq!(padding_needed_for_chapter_start(4), 0);
    }

    #[test]
    fn padding_amount_stays_within_one() {
        for count in 0..100usize {
            assert!(padding_needed_for_chapter_start(count) <= 1);
        }
    }

    #[test]
    fn double_page_with_odd_page_count_pads() {
        let result = apply_book_semantics(&pages(3), double_on());
        assert_eq!(result.padding_before_chapter, 1);
        assert_eq!(result.pages.len(), 4);
        assert!(result.pages[3].is_padding);
    }

    #[test]
    fn double_page_with_even_page_count_does_not_pad() {
        let result = apply_book_semantics(&pages(4), double_on());
        assert_eq!(result.padding_before_chapter, 0);
        assert_eq!(result.pages.len(), 4);
    }

    #[test]
    fn single_page_mode_never_pads() {
        let semantics = BookSemantics {
            columns: Columns::Single,
            chapter_new_page: true,
        };
        let result = apply_book_semantics(&pages(3), semantics);
        assert_eq!(result.padding_before_chapter, 0, "裁决 OQ#5→A");
        assert!(result.pages.iter().all(|page| !page.is_padding));
    }

    #[test]
    fn switch_off_never_pads() {
        let semantics = BookSemantics {
            columns: Columns::Double,
            chapter_new_page: false,
        };
        let result = apply_book_semantics(&pages(3), semantics);
        assert_eq!(result.padding_before_chapter, 0);
        assert!(result.pages.iter().all(|page| !page.is_padding));
    }

    #[test]
    fn padding_page_has_empty_range_and_no_lines() {
        let result = apply_book_semantics(&pages(1), double_on());
        let padding = result
            .pages
            .iter()
            .find(|page| page.is_padding)
            .expect("应有留白页");
        assert!(padding.char_range.is_empty());
        assert!(padding.lines.is_empty());
    }

    /// 留白页占一个序号，故它之后的内容页归属会整体翻转。
    #[test]
    fn padding_page_consumes_a_page_index() {
        let result = apply_book_semantics(&pages(1), double_on());
        // 补了一页后共 2 页：序号 0（内容，左）与序号 1（留白，右）
        assert_eq!(result.pages[0].side, Some(SpreadSide::Left));
        assert_eq!(result.pages[1].side, Some(SpreadSide::Right));
        // 若下一章从序号 2 起，它是左页——补足的目的达成
        assert_eq!(
            SpreadSide::for_page_index(result.pages.len() as u32),
            SpreadSide::Left
        );
    }

    #[test]
    fn empty_input_stays_empty() {
        let result = apply_book_semantics(&[], double_on());
        assert_eq!(result.padding_before_chapter, 0, "首章不补白");
        assert!(result.pages.is_empty());
    }
}
