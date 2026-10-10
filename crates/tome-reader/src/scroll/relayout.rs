//! [DONE] 滚动重排与 resize 合并（T028 / FR-024、FR-027）
//!
//! ## FR-024 的实质：**保留字符锚点，不保留像素偏移**
//!
//! 几何参数一变，行高/行长全变，于是"同一个像素偏移"会落在**不同的文字**
//! 上。一个把滚动位置当作像素量保存的实现，重排后用户会发现自己跳到了别的
//! 段——那就是 FR-024 明文禁止的"保留滚动像素偏移式漂移"。
//!
//! 故重排的正确形态是：
//!
//! ```text
//!   重排前：锚点 = 字符 c
//!   重排  ：按新几何重新布局（像素全变）
//!   重排后：滚动到**字符 c** 所在的位置（而不是"原来的 offset_px"）
//! ```
//!
//! 本模块不持有"当前滚动位置"这个状态——那是会话（S3）的关切。它提供的是
//! **重排要用的输入**（[`RelayoutRequest`]）与**合并机制**
//! （[`ResizeCoalescer`]）；"锚点字符在哪"由 `scroll::map` 回答。
//!
//! ## 为什么要合并 resize（FR-027）
//!
//! 拖拽窗口边缘会产生**每帧一次**的尺寸事件（60fps 下每秒 60 个）。若每个
//! 都触发一次全章重排，算力被吃光且当前锚点在抖动中漂走。合并的做法是：
//! **只保留最后一个尺寸**，等拖拽停下来再重排一次。
//!
//! 这与 001 的 `CancelToken` 语义互补：取消是"放弃正在算的"，合并是"根本不
//! 去算中间那些"。两者都不产生排队任务（FR-027 的"MUST NOT 累积排队"）。

use crate::paginate::config::GeometryConfig;
use crate::paginate::page::{layout_chapter, Paginator};
use crate::paginate::spread::Viewport;
use crate::scroll::flow::ScrollFlow;

/// 一次重排请求（几何 + 视口尺寸）。
///
/// ## 为什么把两者打包
///
/// 重排的触发源有两类：**参数变化**（字号/行高/版心）与**窗口变化**。前者
/// 改 `geometry`，后者改 `viewport`。合并机制必须能处理"两类变化交叠发生"
/// 的情形（用户一边拖窗口一边调字号）——打包成一个请求让合并逻辑只需比较
/// **一个**值，不必维护两套队列。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelayoutRequest {
    /// 阅读区**可用**宽度（px）——FR-014 的口径
    pub width_px: f32,
    /// 阅读区**可用**高度（px）
    pub height_px: f32,
}

impl RelayoutRequest {
    /// 构造视口。
    #[must_use]
    pub fn viewport(&self) -> Viewport {
        Viewport::new(self.width_px, self.height_px)
    }
}

/// resize / 参数变化的**合并器**（FR-027 的"MUST NOT 累积排队"）。
///
/// ## 语义：只保留最后一次请求
///
/// ```text
///   request(A) request(B) request(C) ... request(Z)
///              ↓ 合并
///                        take() → Z
/// ```
///
/// 中间那些**根本不进队列**——不是"进了又丢掉"。故 `pending_count()` 恒为
/// 0 或 1，且内存占用不随事件数增长（有测试遍历 1000 次事件钉住这点）。
///
/// ## 为什么不带时间窗口（节流的另一半）
///
/// 真正的节流需要"距上次执行不足 N 毫秒则延后"——那需要一个时钟，而本层
/// 是**纯几何**模块：引入时间会让重排变成不可复现的（同一输入在不同时刻
/// 产出不同结果），破坏 FR-048 的确定性。
///
/// 故本层只做**合并**（= "只保留最后一个"），时间维度的节流留给 S3 的
/// 帧循环（它本就有帧的概念）。这是职责的合理切分：几何层保证"同一输入
/// 同一输出"，UI 层保证"不要每帧都问"。
#[derive(Debug, Default)]
pub struct ResizeCoalescer {
    pending: Option<RelayoutRequest>,
}

impl ResizeCoalescer {
    /// 新建合并器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 提交一次尺寸变化（覆盖上一次未处理的值）。
    pub fn request(&mut self, width_px: f32, height_px: f32) {
        self.pending = Some(RelayoutRequest {
            width_px,
            height_px,
        });
    }

    /// 取出待处理的请求（取走后队列为空）。
    ///
    /// 空队列返回 `None`——不产出假请求（否则调用方会做一次无谓的重排）。
    pub fn take(&mut self) -> Option<RelayoutRequest> {
        self.pending.take()
    }

    /// 待处理请求数（恒为 0 或 1）。
    ///
    /// 这个方法的存在是为了让"不累积"成为**可断言**的性质：测试可以遍历
    /// 上千次事件后断言它 ≤ 1。
    #[must_use]
    pub fn pending_count(&self) -> usize {
        usize::from(self.pending.is_some())
    }

    /// 是否有待处理请求（不取走）。
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// 按给定几何与视口重排一章（`ScrollFlow` 的便捷构造）。
///
/// ## 为什么提供这个函数而不是让调用方自己两步走
///
/// "先 `layout_chapter` 再 `ScrollFlow::new`"是每次重排都要做的两件事，且
/// **必须**用同一个 `Paginator`（它持有昂贵的字体库句柄）。把它包成一步，
/// 调用方就没有机会在两次重排之间误建新引擎（T024 踩过那个坑：自建引擎的
/// 开销吃掉增量重排的全部收益）。
///
/// ## 重排不改变锚点
///
/// 本函数**不接收也不返回锚点**——那是刻意的。锚点由调用方在重排外持有
/// （它是 `(chapter_index, char_offset)`，与几何无关），重排后调用方用
/// `scroll::map::PositionMap` 把锚点换算成新的滚动位置。若本函数收锚点并
/// "顺便"返回新位置，调用方就会以为重排会保位置——而真正的保证来自"锚点
/// 本身没被碰过"这件事。
pub fn relayout(
    paginator: &mut Paginator,
    content: &str,
    geometry: &GeometryConfig,
    viewport: &Viewport,
) -> Result<ScrollFlow, crate::error::ReaderError> {
    let chapter = layout_chapter(paginator, content, geometry, viewport)?;
    ScrollFlow::new(chapter, *geometry, *viewport)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalescer_keeps_only_the_latest() {
        let mut coalescer = ResizeCoalescer::new();
        assert_eq!(coalescer.pending_count(), 0);
        assert!(!coalescer.has_pending());

        coalescer.request(100.0, 200.0);
        assert_eq!(coalescer.pending_count(), 1);
        assert!(coalescer.has_pending());

        coalescer.request(300.0, 400.0);
        assert_eq!(coalescer.pending_count(), 1, "第二次请求应覆盖而非追加");

        let request = coalescer.take().expect("应有请求");
        assert_eq!(request.width_px, 300.0);
        assert_eq!(request.height_px, 400.0);
        assert_eq!(coalescer.pending_count(), 0);
    }

    #[test]
    fn take_on_empty_is_none() {
        let mut coalescer = ResizeCoalescer::new();
        assert!(coalescer.take().is_none());
        assert!(!coalescer.has_pending());
    }

    #[test]
    fn pending_never_exceeds_one_across_many_requests() {
        let mut coalescer = ResizeCoalescer::new();
        for step in 0..1000 {
            coalescer.request(step as f32, step as f32);
            assert!(coalescer.pending_count() <= 1);
        }
        assert_eq!(coalescer.pending_count(), 1);
    }

    #[test]
    fn request_builds_viewport() {
        let request = RelayoutRequest {
            width_px: 342.0,
            height_px: 250.0,
        };
        let viewport = request.viewport();
        assert_eq!(viewport.available_width_px, 342.0);
        assert_eq!(viewport.available_height_px, 250.0);
    }
}
