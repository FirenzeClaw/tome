//! [DONE] 滚动流排布（T025 / FR-020、FR-021）
//!
//! ## 两种形态的差别只有"怎么切"
//!
//! ```text
//!   正文 ──layout_chapter──→ 章内行盒序列（同一份，两种形态共用）
//!                                 │
//!                                 ├── pack_pages ──→ 页序列（翻页）
//!                                 └── 本模块 ──────→ 视口序列（滚动）
//! ```
//!
//! 切法不同，**坐标口径相同**（FR-021 的"不引入第二套坐标"）：两边都是
//! "章内字符区间 + 该区间内的可见行盒"。
//!
//! ## 本模块的核心设计：区间由**步进**切分，不由**可见行**决定
//!
//! SC-006b 的括号里有一句容易被略过的话：
//!
//! > 相邻视口的区间 MUST 连续无缺口、无重叠（**交叠的可见行按同一字符只属于
//! > 一个视口区间的方式切分**）。
//!
//! 一屏能容下的行往往**多于**该屏的字符预算——屏底那行只露出一部分，
//! 而它已完整计入本屏区间（否则会出现缺口）。于是同一行可能同时"可见"于
//! 相邻两屏。
//!
//! 若把"可见行的并集"当作视口区间，相邻屏就会**重叠**，直接违反 SC-006b。
//! 故本模块把两件事分开：
//!
//! | 概念 | 用途 | 是否可重叠 |
//! |---|---|---|
//! | `char_range` | 位置换算、痕迹、阅读记录（FR-021 的"唯一依据"） | **否** |
//! | `lines` | 渲染（哪些行要画） | **是**（跨屏行） |
//!
//! 区间按"从上一屏终点继续"推进；可见行按"哪些行与区间相交"取。两者
//! 各有自己的判据，重叠就无从产生。
//!
//! ## 步进量怎么定
//!
//! 一屏的高度是 `available_height`。步进推进直到"本屏已容纳至少一屏高的
//! 行"。实现取"从区间起点开始，累计行高首次 ≥ 视口高"的那一点作为终点。
//! 若单行就超过视口高，它独占一屏（否则会死循环）。

use crate::paginate::config::GeometryConfig;
use crate::paginate::page::{ChapterLayout, PageLine};
use crate::paginate::spread::Viewport;

/// 形态切换的**过渡事件数参考值**（供上层判断"何时算稳定"）。
///
/// ## 这不是"稳定判据"，而是给上层的参考量
///
/// FR-023 要求"进度以切换**稳定后**的锚点为准"，但"何时算稳定"取决于
/// UI 生命周期（动效播完？首帧渲染完？），那是 S3 的关切。本层**不自行**
/// 用事件数判定——那样会引入一个凭空数字（与 MASTER 被判废的"变化幅度
/// 阈值"同类问题，裁决 OQ#14→C）。
///
/// 这个常量的用途是：上层若需要"大概几次事件之后可以认为动效结束"的
/// 默认值，可以取它；否则自行判定。取 **3** 的理由是它对应一个直觉上
/// 合理的下限（动画开始 + 中段 + 结束），且与 `--dur-slow`（280ms）在
/// 常见帧率下的帧数量级相当。
pub const TRANSCRIPTION_SETTLE_EVENTS: usize = 3;

/// 滚动流中的一个视口（data-model.md §6 的 `ScrollViewport`）。
///
/// ## 与 `Page` 的同构（FR-021）
///
/// | `Page` | `ScrollViewport` | 同构点 |
/// |---|---|---|
/// | `char_range` | `char_range` | **章内字符区间**（位置口径） |
/// | `lines` | `lines` | 可见行盒（渲染口径） |
/// | `page_in_chapter` | `viewport_index` | 序列位置 |
/// | （无） | `top_px` / `height_px` | 滚动流中的几何位置 |
///
/// 最后一行是两者**唯一**的差别：翻页模式的位置由页序号表达（页是离散的），
/// 滚动模式必须额外知道"这一屏在流里的像素位置"——但那只是**表达**的差别，
/// 位置换算的主口径仍是字符区间。
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollViewport {
    /// 屏序号（0 起，按滚动顺序）
    pub viewport_index: usize,
    /// 本屏在滚动流中的顶部偏移（px）
    pub top_px: f32,
    /// 本屏高度（px）——等于阅读区可用高
    pub height_px: f32,
    /// 本屏覆盖的**章内**字符区间（与 `Page::char_range` 同口径）
    pub char_range: std::ops::Range<usize>,
    /// 与本屏相交的行盒（**可跨屏**，见模块文档）
    pub lines: Vec<PageLine>,
}

impl ScrollViewport {
    /// 本屏覆盖的字符数。
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.char_range.end - self.char_range.start
    }

    /// 本屏是否为空（无行、无字符）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.char_range.is_empty()
    }
}

/// 滚动流（一章的完整视口序列）。
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollFlow {
    viewports: Vec<ScrollViewport>,
    content_height_px: f32,
    measure_px: f32,
    char_len: usize,
    /// 供重排/切换复用的行盒序列（FR-023/024 的锚点换算依据）
    lines: Vec<PageLine>,
}

impl ScrollFlow {
    /// 由一章的排版结果构造滚动流（FR-020/021）。
    ///
    /// ## 输入是 [`ChapterLayout`] 而不是正文
    ///
    /// 这样"两种形态共享同一排版结果"由**类型**保证：调用方必须先在
    /// `paginate::page::layout_chapter` 拿到行盒，才能构造滚动流。
    /// 若本函数吃正文并自行排版，就会存在第二条排版路径（FR-021 的
    /// "第二套坐标"由此潜入）。
    ///
    /// ## 空章
    ///
    /// 产出一个空视口（与分页的"空章一页"对称，spec §Edge Cases）。
    pub fn new(
        chapter: ChapterLayout,
        _geometry: GeometryConfig,
        viewport: Viewport,
    ) -> Result<Self, crate::error::ReaderError> {
        let height_px = viewport.available_height_px.max(1.0);
        let content_height_px: f32 = chapter.lines.iter().map(|line| line.line.height_px).sum();

        if chapter.lines.is_empty() {
            return Ok(Self {
                viewports: vec![ScrollViewport {
                    viewport_index: 0,
                    top_px: 0.0,
                    height_px,
                    char_range: 0..0,
                    lines: Vec::new(),
                }],
                content_height_px: 0.0,
                measure_px: chapter.measure_px,
                char_len: chapter.char_len,
                lines: Vec::new(),
            });
        }

        let viewports = slice_into_viewports(&chapter.lines, height_px, chapter.char_len);

        Ok(Self {
            viewports,
            content_height_px,
            measure_px: chapter.measure_px,
            char_len: chapter.char_len,
            lines: chapter.lines,
        })
    }

    /// 视口序列。
    #[must_use]
    pub fn viewports(&self) -> &[ScrollViewport] {
        &self.viewports
    }

    /// 滚动流总高（px）——**不含分页留白**（FR-020）。
    #[must_use]
    pub fn content_height_px(&self) -> f32 {
        self.content_height_px
    }

    /// 版心宽度（px）——与分页形态共用的几何结果。
    #[must_use]
    pub fn measure_px(&self) -> f32 {
        self.measure_px
    }

    /// 章内字符总数。
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.char_len
    }

    /// 全部行盒（按阅读顺序）——位置换算与重排的依据。
    #[must_use]
    pub fn lines(&self) -> &[PageLine] {
        &self.lines
    }
}

/// 把行盒序列切成视口（SC-006b 的区间口径）。
///
/// ## 算法：按"累计到一屏高"推进区间终点
///
/// 从当前起点开始，累计行高直到 **≥ 视口高**，取该行**末字符之后**作为
/// 区间终点。下一屏从该终点继续。
///
/// ## 三个保证
///
/// 1. **无缺口**：下一屏的起点就是上一屏的终点（区间首尾相接）
/// 2. **无重叠**：终点取"末字符之后"（半开区间），下一屏不再包含该字符
/// 3. **不丢字**：末屏终点强制推到 `char_len`（覆盖被段落切分丢弃的尾部
///    空白段等其他字符）
///
/// ## 单行超过视口高的处置
///
/// 该行独占一屏。若不留这条，累计行高永远达不到视口高（一行就超了），
/// 推进逻辑会死循环或产出空屏。
fn slice_into_viewports(
    lines: &[PageLine],
    height_px: f32,
    char_len: usize,
) -> Vec<ScrollViewport> {
    let mut viewports = Vec::new();
    let mut cursor = 0usize; // 下一屏的字符起点
    let mut top_px = 0.0f32;

    // 行的"顶部偏移"：按顺序累加行高。用它决定每屏的可见行范围。
    let mut line_tops: Vec<f32> = Vec::with_capacity(lines.len());
    let mut accumulated = 0.0f32;
    for line in lines {
        line_tops.push(accumulated);
        accumulated += line.line.height_px;
    }

    let mut index = 0usize;
    while index < lines.len() {
        // 本屏从第 `index` 行开始（它包含 `cursor`）。累计行高直到够一屏。
        let mut end_index = index;
        let mut used_height = 0.0f32;
        while end_index < lines.len() {
            let line_height = lines[end_index].line.height_px.max(0.0);
            // 至少收一行（否则单行超高时会死循环）
            if end_index > index && used_height + line_height > height_px {
                break;
            }
            used_height += line_height;
            end_index += 1;
            if used_height >= height_px {
                break;
            }
        }

        // 区间终点：**下一屏首个行盒的起点**（`end_index` 行不在本屏）。
        //
        // ## 为什么不是"末行终点的下一个字符"
        //
        // 那个写法（`+1`）会把**下一行首字符**划进本屏——它属于下一屏可见的
        // 那一行，于是本屏的"最后一屏可见行"与区间不自洽（区间说该字符在本屏，
        // 而行在下一屏）。实测它让 `[0,5)/[5,10)` 两行切出 `[0,6)/[6,10)`：
        // 字符 5 被划给第一屏，而字符 5 正在第二屏显示的行上。
        //
        // 取"下一屏首行起点"则与分页的 `fill_page_ranges` **完全同构**：
        // 屏 i 覆盖到屏 i+1 的首字符为止。这样行与行之间被丢弃的段间分隔符
        // （它们不属于任何行盒）自然归前一屏，而**不存在**越界划走下一行首字符。
        let mut end = match lines.get(end_index) {
            Some(next_line) => next_line.chapter_char_range.start,
            // 末屏：终点推到正文末尾（尾部被丢弃的空白段也有归属）
            None => char_len,
        };
        end = end.min(char_len);

        // 区间必须前进，否则外层循环不终止
        if end <= cursor {
            end = (cursor + 1).min(char_len);
            // 已在末尾却仍无进展 → 收尾（理论不可达，防死循环）
            if end <= cursor {
                break;
            }
        }

        // 可见行：与本屏**像素窗口**相交的行（可跨屏，见模块文档）
        let char_range = cursor..end;
        let visible = collect_visible_lines(lines, &line_tops, top_px, height_px);

        viewports.push(ScrollViewport {
            viewport_index: viewports.len(),
            top_px,
            height_px,
            char_range: char_range.clone(),
            lines: visible,
        });

        top_px += height_px;
        cursor = end;
        // 下一屏从"包含 cursor 的那一行"开始（该行可能已在本屏可见——允许）
        while index < lines.len() && lines[index].chapter_char_range.end <= cursor {
            index += 1;
        }
    }

    // 收尾：串接的区间若未覆盖到正文末尾，补一屏（覆盖尾部被丢弃的空白）
    if cursor < char_len {
        viewports.push(ScrollViewport {
            viewport_index: viewports.len(),
            top_px,
            height_px,
            char_range: cursor..char_len,
            lines: Vec::new(),
        });
    }

    viewports
}

/// 取在给定**像素窗口**内可见的行盒。
///
/// ## 为什么按像素而不是按字符区间取（这是"可见行可重叠"的来源）
///
/// 一屏的像素窗口是 `[top_px, top_px + height_px)`。行在流里各占一段像素高度，
/// 而**行高之和通常不整除屏高**——例如行高 13px、屏高 40px：第一屏必须收
/// 满 4 行（52px）才够一屏，第二屏从 40px 开始。
///
/// 于是第一屏的末行（占 39..52px）在第二屏的窗口里**露出了下半截**（40..52px）
/// ——它同时可见于两屏。这正是 SC-006b 括号里描述的情形：
///
/// > 交叠的可见行按同一字符只属于一个视口区间的方式切分
///
/// 那个"切分"就是本模块的 `char_range`：区间按行**完整归属**推进（互不重叠），
/// 而可见性按像素相交判定（可以重叠）。渲染层据像素裁剪那一行即可。
///
/// ## 若按字符区间取会怎样
///
/// 末行不会被算进第二屏的可见行（它的字符全属第一屏）——于是滚过边界时
/// 那一行会**整行消失**，直到它完全离开第一屏的窗口。表现是"滚动时某一
/// 行闪烁"。故必须按像素取。
fn collect_visible_lines(
    lines: &[PageLine],
    line_tops: &[f32],
    window_top_px: f32,
    window_height_px: f32,
) -> Vec<PageLine> {
    let window_bottom = window_top_px + window_height_px;

    lines
        .iter()
        .zip(line_tops)
        .filter(|(line, top)| {
            let line_top = **top;
            let line_bottom = line_top + line.line.height_px.max(0.0);
            // 半开区间相交：line_top < window_bottom && line_bottom > window_top
            line_top < window_bottom && line_bottom > window_top_px
        })
        .map(|(line, _)| line.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::linebox::LineBox;

    /// 造一个行盒（区间与高度可控）。
    fn page_line(start: usize, len: usize, height: f32) -> PageLine {
        PageLine {
            line: LineBox {
                paragraph_index: 0,
                char_range: 0..len,
                glyphs: Vec::new(),
                height_px: height,
                baseline_px: height * 0.8,
                squeeze_px: 0.0,
                justify_extra_px: 0.0,
                rule_exception: None,
            },
            chapter_char_range: start..(start + len),
        }
    }

    /// 10 行、每行 10 字符、每行高 10px，视口高 30px → 每屏 3 行。
    #[test]
    fn slices_three_lines_per_screen() {
        let lines: Vec<PageLine> = (0..10)
            .map(|index| page_line(index * 10, 10, 10.0))
            .collect();
        let viewports = slice_into_viewports(&lines, 30.0, 100);

        assert!(viewports.len() >= 4, "实测 {} 屏", viewports.len());
        assert_eq!(viewports[0].char_range.start, 0);
        // 区间首尾相接
        for pair in viewports.windows(2) {
            assert_eq!(pair[0].char_range.end, pair[1].char_range.start);
        }
        // 末屏覆盖到末尾
        assert_eq!(viewports.last().expect("应有末屏").char_range.end, 100);
    }

    /// 单行高于视口 → 独占一屏（不死循环）。
    #[test]
    fn single_overheight_line_gets_own_screen() {
        let lines = vec![page_line(0, 5, 100.0), page_line(5, 5, 100.0)];
        let viewports = slice_into_viewports(&lines, 30.0, 10);
        assert_eq!(viewports.len(), 2, "每行各占一屏");
        // 屏 i 覆盖到屏 i+1 首行起点为止（与分页同构）——故第二屏从 5 起
        assert_eq!(viewports[0].char_range.start, 0);
        assert_eq!(
            viewports[0].char_range.end, 5,
            "首屏终点应是下一屏首行起点（不得划走下一行首字符）"
        );
        assert_eq!(viewports[1].char_range.start, 5);
        assert_eq!(viewports[1].char_range.end, 10, "末屏推到正文末尾");
    }

    /// 区间永不重叠（半开区间 + 相邻终点）。
    #[test]
    fn ranges_never_overlap() {
        let lines: Vec<PageLine> = (0..20).map(|index| page_line(index * 7, 7, 13.0)).collect();
        let viewports = slice_into_viewports(&lines, 40.0, 140);
        for pair in viewports.windows(2) {
            assert!(
                pair[0].char_range.end <= pair[1].char_range.start,
                "区间重叠：{:?} 与 {:?}",
                pair[0].char_range,
                pair[1].char_range
            );
        }
    }

    /// 屏高为 0 时不 panic 且能收尾。
    #[test]
    fn zero_height_terminates() {
        let lines: Vec<PageLine> = (0..5).map(|index| page_line(index * 4, 4, 10.0)).collect();
        let viewports = slice_into_viewports(&lines, 0.0, 20);
        assert!(!viewports.is_empty());
        assert_eq!(viewports.last().expect("末屏").char_range.end, 20);
    }

    #[test]
    fn empty_lines_yield_no_viewports_from_slicer() {
        // 空章由 `ScrollFlow::new` 单独处置（产出一屏），切片函数本身返回空
        let viewports = slice_into_viewports(&[], 100.0, 0);
        assert!(viewports.is_empty());
    }
}
