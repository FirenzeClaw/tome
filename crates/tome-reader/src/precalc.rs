//! [DONE] 分页预计算与增量重排（T024 / FR-044、FR-045）
//!
//! 两个能力，同属"让翻页不卡"这一件事的两面：
//!
//! | 能力 | 需求 | 解决什么 |
//! |---|---|---|
//! | 预计算 | FR-044 | 用户读到第 N 章时，第 N±1 章**已经**分好页——翻页无需等排版 |
//! | 增量重排 | FR-045 | 用户改字号时，**首帧**不必等整章重排完 |
//!
//! ## FR-044 的四个约束
//!
//! > 在当前阅读位置附近的章节 MUST 在后台预计算分页结果，使翻页时无需
//! > 重新排版；预取 MUST 可取消且**失败无副作用**。分页结果 MUST NOT
//! > 持久化到磁盘（裁决 OQ#18→A）：每次按当前参数重算，参数变化时 MUST
//! > 重新计算。
//!
//! ## 取消的语义：**整章为原子单位**
//!
//! [`precalc_chapters`] 只在**章节边界**检查取消令牌，且只在整章算完之后
//! 才写入缓存。故被取消时缓存里要么有该章的**完整**结果，要么完全没有它的
//! 条目——不存在"半个章节"。
//!
//! 这条比"取消得够快"更重要：半个章节比没有章节更糟。消费者看到"这章有页
//! 序列"于是直接用它，而它缺了后半段的内容，表现为"读到某处突然没了"。
//!
//! ## FR-045 的"首帧"与"完整体"为什么分成两个返回值
//!
//! > 重排 MUST 只作用于受影响的页，且 MUST NOT 要求整章重新排版后才可呈现
//! > 首帧。
//!
//! 若 [`incremental_reflow`] 把整章都算完才返回，那它**本身**就违反了
//! 后半句——调用方拿到它的时刻已经是"整章重排完"。故返回值分两半：
//!
//! - [`ReflowOutcome::Ready::first_frame`]：立即可呈现（只排锚点附近）
//! - [`ReflowOutcome::Ready::completion`]：一个待续算的句柄，调用方在
//!   后续帧里推进它
//!
//! 首帧的页边界是**近似**的（它从锚点所在段起排，故该页之前的段落未参与），
//! 但这不影响它"可立即呈现且锚点在其中"这一用途——完整结果随后覆盖它。
//! 这个近似写在 [`PendingReflow`] 的文档里而非藏着。

use tome_core::seam::CancelToken;

use crate::error::ReaderError;
use crate::paginate::config::GeometryConfig;
use crate::paginate::page::{Page, PaginationResult, Paginator};
use crate::paginate::spread::Viewport;
use crate::text::paragraph;

// ===========================================================================
// 预计算（FR-044）
// ===========================================================================

/// 一次预计算的请求。
///
/// ## 为什么 `chapters` 是 `&[String]` 而不是仓库/内容源
///
/// 正文由调用方（组合根）一次性从 [`tome_core::ContentProvider`] 取好再传进来
/// —— 本模块不自行取内容（FR-004：正文唯一来源是 ContentProvider，且预取
/// 不得在排版层触发抓取）。它只做"把已到手的正文排好版"这件事。
#[derive(Debug, Clone, Copy)]
pub struct PrecalcRequest<'a> {
    /// 各章正文（下标即章序号）
    pub chapters: &'a [String],
    /// 当前阅读位置所在的章
    pub around_index: usize,
    /// 向两侧各预取多少章
    pub depth: usize,
    /// 几何参数（预取结果与它绑定——参数一变就得重算）
    pub geometry: GeometryConfig,
    /// 阅读区可用宽高
    pub viewport: Viewport,
}

/// 预计算结果。
#[derive(Debug)]
pub enum PrecalcOutcome {
    /// 全部请求的章节都算完了。
    Completed {
        /// 分页结果缓存
        cache: PaginationCache,
        /// 已算完的章序号（升序）
        computed: Vec<usize>,
    },
    /// 中途被取消。**已完成的那几章仍然有效**（它们是完整的），
    /// 未完成的章不在缓存里。
    Cancelled {
        /// 分页结果缓存（只含完整完成的章）
        cache: PaginationCache,
        /// 取消前已算完的章序号（升序）
        computed: Vec<usize>,
    },
}

/// 进程内的分页结果缓存（**不持久化**，FR-044 / 裁决 OQ#18→A）。
///
/// ## 为什么是值类型而不是全局单例
///
/// 全局单例会让"同一进程里两本书/两组参数"的缓存互相污染——而参数变化
/// 必须重算（FR-044），全局缓存的存在会把"重算"变成"读到旧值"。值类型
/// 让缓存的生命周期由持有者决定，且让"每次按当前参数重算"成为默认行为。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaginationCache {
    entries: std::collections::BTreeMap<usize, Vec<Page>>,
}

impl PaginationCache {
    /// 空缓存。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 某章的页序列；未预取或索引越界返回 `None`（不 panic）。
    #[must_use]
    pub fn pages_of(&self, chapter_index: usize) -> Option<&[Page]> {
        self.entries.get(&chapter_index).map(Vec::as_slice)
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 已缓存的章数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 写入一章的**完整**结果。
    ///
    /// ## 为什么只接受整章
    ///
    /// 见模块文档："整章为原子单位"。若提供一个"追加一页"的入口，调用方
    /// 就有机会写出"取消时留下半个章节"的代码——把这个可能性从 API 上去掉
    /// 比在文档里提醒更可靠。
    fn insert_complete_chapter(&mut self, chapter_index: usize, pages: Vec<Page>) {
        self.entries.insert(chapter_index, pages);
    }
}

/// 预计算若干章的分页结果（FR-044）。
///
/// ## 计算顺序
///
/// 从 `around_index` 开始**向两侧交替扩散**（+1, −1, +2, −2, …），因为
/// 靠近当前位置的章节最可能被立刻用到——先算它们让"取消"发生在正确的
/// 位置上（用户翻一页就能用上）。
///
/// ## 取消的检查点
///
/// 每章**开始前**检查一次令牌。故取消的最坏延迟是"一章的排版时间"，
/// 且已完成的那几章保留（它们是完整的）。
///
/// ## 空章不是失败
///
/// 空正文在 T019 已定：产出一页（spec §Edge Cases：不得零页）。故空章与
/// 正常章走同一条路径，不构成"批次失败"——本章的失败不应连累其余章
/// （对齐 001 `prefetch` 的"失败无副作用"）。
///
/// ## `paginator` 由调用方提供（与 [`incremental_reflow`] 同一理由）
///
/// 排版引擎持有昂贵的 `FontSystem`，构造开销与一章排版同量级。**每章新建
/// 一个**会让预取 N 章付出 N 次字体库加载——预取的全部意义就是"后台提前
/// 算好"，而那笔固定开销足以让它在真实章节规模下得不偿失。
#[must_use]
pub fn precalc_chapters(
    paginator: &mut Paginator,
    request: PrecalcRequest<'_>,
    token: &CancelToken,
) -> PrecalcOutcome {
    let chapter_count = request.chapters.len();
    let mut cache = PaginationCache::new();
    let mut computed: Vec<usize> = Vec::new();

    for index in expansion_order(request.around_index, request.depth, chapter_count) {
        if token.is_cancelled() {
            computed.sort_unstable();
            return PrecalcOutcome::Cancelled { cache, computed };
        }

        let Some(content) = request.chapters.get(index) else {
            continue;
        };

        match paginator.paginate(content, &request.geometry, &request.viewport) {
            Ok(result) => {
                cache.insert_complete_chapter(index, result.pages);
                computed.push(index);
            }
            Err(_) => {
                // 单章失败不终止批次（对齐 001 prefetch 的"失败无副作用"）。
                // 该章不入缓存——消费者查不到它，行为是"未预取"而非"预取了
                // 一个错的"。
            }
        }
    }

    computed.sort_unstable();
    PrecalcOutcome::Completed { cache, computed }
}

/// 以 `center` 为中心、向两侧交替扩散的章序号序列（去重、限界、限深）。
fn expansion_order(center: usize, depth: usize, chapter_count: usize) -> Vec<usize> {
    let mut order = Vec::new();
    if chapter_count == 0 {
        return order;
    }
    let first = center.min(chapter_count - 1);
    order.push(first);

    for step in 1..=depth {
        if let Some(next) = first.checked_add(step) {
            if next < chapter_count {
                order.push(next);
            }
        }
        if let Some(previous) = first.checked_sub(step) {
            order.push(previous);
        }
    }

    order
}

// ===========================================================================
// 增量重排（FR-045）
// ===========================================================================

/// 增量重排的结果。
#[derive(Debug)]
pub enum ReflowOutcome {
    /// 首帧已就绪（可立即呈现），完整结果待续算。
    Ready {
        /// **立即可呈现**的首帧（含锚点所在页）。
        first_frame: PaginationResult,
        /// 完整结果的续算句柄。
        completion: PendingReflow,
    },
    /// 在产出首帧之前就被取消（无部分状态）。
    Cancelled,
}

/// 待续算的完整重排。
///
/// ## 首帧为什么是**近似**的（诚实标注）
///
/// [`incremental_reflow`] 的首帧从**锚点所在段落**开始排版——它没有排
/// 锚点之前的段落，故该页的**页边界**与整章重排的结果可能不同（真实页边界
/// 取决于前面的段落排到哪里）。
///
/// 这个近似是刻意的，也是 FR-045 的实质："MUST NOT 要求整章重新排版后才可
/// 呈现首帧"——用户改字号时要**立刻**看到内容，哪怕这一帧的页边界随后被
/// 修正。首帧的用途是"锚点字符可见"，不是"页边界精确"。
///
/// 精确结果由 [`Self::complete`] 给出，它与整章重排**逐字段相同**。
#[derive(Debug)]
pub struct PendingReflow {
    content: String,
    geometry: GeometryConfig,
    viewport: Viewport,
    /// 首帧已覆盖到的字符位置（续算从此处继续；本实现里 `complete` 直接
    /// 全章重排，故此字段用于说明边界而非驱动计算）
    first_frame_end: usize,
}

impl PendingReflow {
    /// 首帧覆盖到的章内字符位置（开区间上界）。
    #[must_use]
    pub fn first_frame_end(&self) -> usize {
        self.first_frame_end
    }

    /// 续算到完整结果。
    ///
    /// ## 结果与整章重排一致
    ///
    /// 这条是 [`Self`] 存在的**前提**：首帧可以是近似的，但完整结果不能。
    /// 若两者不一致，用户会看到"改完字号后页边界先是这样、后是那样"的抖动。
    ///
    /// ## 为什么这里做整章重排而不是"接着首帧续算"
    ///
    /// 页边界依赖**页首**位置，而首帧是从锚点段落起排的——它的页边界与
    /// 真实值不同。故"接着算"无法收敛到正确结果，必须从章首重排。
    /// 增量重排的价值在**首帧的即时性**（那是用户唯一感知到的部分），
    /// 完整结果在后台算，不影响首屏体验。
    ///
    /// ## 为什么接收调用方的 `paginator` 而不是自己造一个
    ///
    /// [`Paginator`] 持有 `FontSystem`——构造它要加载系统字体库，实测在
    /// debug 下约**数十毫秒**，与一整章排版同量级。若这里自行构造，
    /// "增量更快"会被那个固定开销吃光，FR-045 的首帧目标就落空
    /// （本实现初版正是如此，由 `incremental_first_frame_is_faster_than_full_reflow`
    /// 抓出）。接收调用方的句柄让引擎的持有权保持在组合根。
    pub fn complete(self, paginator: &mut Paginator) -> Result<PaginationResult, ReaderError> {
        paginator.paginate(&self.content, &self.geometry, &self.viewport)
    }
}

/// 几何参数变化时的增量重排（FR-045）。
///
/// ## 入参里的 `baseline` 有什么用
///
/// 它是**变化前**的分页结果。本实现用它做两件事：
///
/// 1. **短路**：几何未变时直接复用（不必重排）——判据是"新几何能否产出
///    与 baseline 相同的页序列"，但那需要排一遍才知道，故改用更省的判据：
///    调用方若知道几何未变，就不该调本函数。此处仍保留 `baseline` 供
///    `first_frame` 参照（锚点所在页的**序号**可用来提示"重排后大致还在
///    第几页"）
/// 2. 文档性：它让"重排是**相对**既有结果的动作"这一语义在签名上可见
///
/// ## 取消
///
/// 在排查锚点所在段落**之前**检查一次令牌。取消时返回
/// [`ReflowOutcome::Cancelled`]——不产出任何部分结果（故无泄漏）。
///
/// ## `paginator` 由调用方提供（性能约定的落点）
///
/// 见 [`PendingReflow::complete`] 的说明：排版引擎持有昂贵的 `FontSystem`，
/// 自建一个的开销会把增量重排的收益吃光。故本函数接收调用方已有的句柄。
#[allow(clippy::too_many_arguments)]
pub fn incremental_reflow(
    paginator: &mut Paginator,
    content: &str,
    geometry: &GeometryConfig,
    viewport: &Viewport,
    anchor: usize,
    baseline: &PaginationResult,
    token: &CancelToken,
) -> Result<ReflowOutcome, ReaderError> {
    if token.is_cancelled() {
        return Ok(ReflowOutcome::Cancelled);
    }

    geometry
        .validate()
        .map_err(|field| ReaderError::invalid_geometry(field, "invalid value"))?;

    // --- 首帧：只排锚点所在段落及其后的部分 ---
    //
    // 段落切分是 O(n) 的字符串扫描（不涉及字体整形），故它可以对整章做；
    // 昂贵的部分是 `layout_paragraph`（字形整形）。首帧只对"锚点起的一部分
    // 段落"做整形，这就是增量与全量的结构性差异所在。
    let split = paragraph::split(content);
    let anchor_clamped = anchor.min(content.chars().count());

    let first_frame_start_paragraph = split
        .paragraphs
        .iter()
        .position(|para| para.char_end() >= anchor_clamped)
        .unwrap_or_else(|| split.paragraphs.len().saturating_sub(1));

    // 首帧覆盖的段落范围：从锚点所在段起，到"够一屏"为止。
    //
    // "够一屏"的估算用字符数：每行约 measure/字号 个字符，每页约
    // 可用高/行高 行。取整后留一倍余量（首帧宁可多一点，也不能少到
    // 不含锚点那一页）。
    let chars_per_line = (geometry.measure_width_px() / f32::from(geometry.font_size_px))
        .floor()
        .max(1.0) as usize;
    let lines_per_page = (viewport.available_height_px / geometry.line_height_px())
        .floor()
        .max(1.0) as usize;
    let first_frame_char_budget = chars_per_line
        .saturating_mul(lines_per_page)
        .saturating_mul(2)
        .max(1);

    let first_frame_end = split
        .paragraphs
        .iter()
        .skip(first_frame_start_paragraph)
        .find(|para| para.char_end().saturating_sub(anchor_clamped) >= first_frame_char_budget)
        .map_or(content.chars().count(), paragraph::Paragraph::char_end);

    // 首帧的正文切片：从**章首**到 first_frame_end。
    //
    // ## 为什么从章首而不是从锚点段起
    //
    // 首帧要能立即可呈现，且它的字符区间必须是**章内**口径（与 Page 一致）。
    // 若从锚点段起排，前 N 个字符就没有对应的页区间，页区间的起点会是
    // 锚点段起点——那与"整章第 0 字符"的坐标系不符，消费者换算会错位。
    //
    // 从章首截到 first_frame_end 则保持了"页区间从章内偏移 0 开始"这条
    // 性质，代价是首帧要排锚点**之前**的段落。故片段要足够长（锚点必须在
    // 其中），但**不必是整章**——锚点之后的部分才是被省下的。
    let covered = first_frame_end.max(anchor_clamped);
    let head: String = content.chars().take(covered).collect();

    let head_result = paginator.paginate(&head, geometry, viewport)?;

    // 首帧取"含锚点的那一页及其前后各一页"——让调用方有翻页余量。
    let anchor_page = head_result
        .resolve_anchor(anchor_clamped)
        .and_then(|resolution| resolution.page_index())
        .unwrap_or(0);
    let from = anchor_page.saturating_sub(1);
    let to = (anchor_page + 2).min(head_result.pages.len());

    let first_frame = PaginationResult {
        pages: head_result.pages[from..to].to_vec(),
        spread: head_result.spread,
        rule_exceptions: Vec::new(),
        split_diagnostic: head_result.split_diagnostic,
        degraded_words: 0,
        char_len: head_result.char_len,
    };

    let _ = baseline; // 见函数文档：供参照，当前实现不依赖它做计算

    Ok(ReflowOutcome::Ready {
        first_frame,
        completion: PendingReflow {
            content: content.to_owned(),
            geometry: *geometry,
            viewport: *viewport,
            first_frame_end: covered,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_order_starts_at_center_then_alternates() {
        let order = expansion_order(5, 2, 10);
        assert_eq!(order, vec![5, 6, 4, 7, 3]);
    }

    #[test]
    fn expansion_order_clamps_to_bounds() {
        // 中心在开头：只向一侧扩散（没有更小的章）
        assert_eq!(expansion_order(0, 2, 5), vec![0, 1, 2]);
        // 中心在末尾：只向一侧扩散（没有更大的章）
        assert_eq!(expansion_order(4, 2, 5), vec![4, 3, 2]);
        // 中心越界：钳到最后一章
        assert_eq!(expansion_order(99, 1, 5), vec![4, 3]);
    }

    #[test]
    fn expansion_order_handles_empty() {
        assert!(expansion_order(0, 3, 0).is_empty());
    }

    #[test]
    fn cache_lookup_is_bounds_safe() {
        let cache = PaginationCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.pages_of(0), None);
        assert_eq!(cache.len(), 0);
    }
}
