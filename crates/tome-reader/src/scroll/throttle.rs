//! `[DONE]` 滚动事件节流（T030 / FR-027）
//!
//! ## FR-027 原文
//!
//! > 滚动模式下系统 MUST 合并/节流密集的滚动事件引起的视口重绘，MUST NOT 因
//! > 连续滚动累积排队任务。
//!
//! ## 本模块只做**合并**，不做时间窗节流
//!
//! 与 [`crate::scroll::relayout::ResizeCoalescer`] 同一理由：引入时钟会让
//! "同一输入 → 同一输出"不再成立（FR-048 的确定性），而本层是纯几何模块。
//! 时间维度的节流属 S3 的帧循环（它本就有帧的概念）。
//!
//! 故 [`ScrollEventThrottle`] 的语义是：
//!
//! ```text
//!   事件 × 240（惯性滚动）
//!         ↓ 合并：只保留最后位置
//!   take() → 一次重排（位置 = 最后一次事件的值）
//! ```
//!
//! 中间那些**根本不进队列**——不是"进了又丢"。故 `pending_count()` 恒 ≤ 1。
//!
//! ## 为什么还要"微小位移跳过"
//!
//! 亚像素抖动（触控板的噪声）不该触发视口重算。判据是一个命名常量
//! （[`NEGLIGIBLE_DELTA_PX`]）：位移绝对值小于它时判 `Skip`。
//!
//! ## 节流**不得改变位置**
//!
//! 这条是本节流器最容易出错的地方：一个"顺手把偏移吸附到屏边界"的实现会让
//! 用户每次滚动都偏移几像素，累积起来就是"滚着滚着对不上了"。故 [`take`]
//! 原样传递最后一次事件的偏移（有测试断言逐位相同）。
//!
//! [`take`]: ScrollEventThrottle::take

/// 被判定为"无需重排"的位移阈值（px）。
///
/// ## 取值依据
///
/// 亚像素抖动来自触控板/高精度滚轮的量化噪声，量级在 0.1–0.5px。取 **1.0px**
/// 的理由：
///
/// - 它低于任何肉眼可辨的位置差异（19px 字号下，1px 不到字身的 1/19）
/// - 它高于常见的噪声幅度（0.1–0.5px），故能真的滤掉抖动
/// - 它是**整数**，让判据在浮点比较时不必纠结 epsilon
///
/// 命名常量而非魔数：这个数字是"抖动 vs 真实滚动"的分界，改动它需要重新
/// 论证上面两条。
pub const NEGLIGIBLE_DELTA_PX: f32 = 1.0;

/// 节流的判定结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThrottleDecision {
    /// 需要重排（位置有意义地变了）。
    Relayout {
        /// 目标滚动偏移（**原样传递**，不做任何取整/吸附）
        offset_px: f32,
    },
    /// 无需重排（位移可忽略）。
    Skip {
        /// 被忽略的位移量（诊断用）
        delta_px: f32,
    },
}

impl ThrottleDecision {
    /// 若判为重排，给出目标偏移。
    #[must_use]
    pub fn offset_px(self) -> Option<f32> {
        match self {
            Self::Relayout { offset_px } => Some(offset_px),
            Self::Skip { .. } => None,
        }
    }

    /// 是否需要重排。
    #[must_use]
    pub fn needs_relayout(self) -> bool {
        matches!(self, Self::Relayout { .. })
    }
}

/// 滚动事件节流器（FR-027 的"MUST NOT 累积排队"）。
///
/// ## 语义：只保留最后一次位置
///
/// 见模块文档。`pending_count()` 恒为 0 或 1，内存不随事件数增长。
#[derive(Debug, Default)]
pub struct ScrollEventThrottle {
    /// 待处理的最后位置（`None` = 无待处理）
    pending_offset: Option<f32>,
    /// 上次已处理的位置（用于算位移；首次事件时无基准）
    last_processed: Option<f32>,
}

impl ScrollEventThrottle {
    /// 新建节流器。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一次滚动事件（覆盖上一次未处理的位置）。
    ///
    /// ## 首次事件总是触发重排
    ///
    /// 没有基准位置时无从判断"位移大小"——而用户从静止开始滚动时，第一次
    /// 事件就是有意义的（他确实滚了）。故首次事件记为 `pending`，由 [`take`]
    /// 判为 `Relayout`。
    ///
    /// [`take`]: Self::take
    pub fn on_scroll_event(&mut self, offset_px: f32) {
        self.pending_offset = Some(offset_px);
    }

    /// 取出待处理的重排决策（取走后队列为空）。
    ///
    /// ## 三条判据
    ///
    /// | 情形 | 结果 |
    /// |---|---|
    /// | 无待处理 | `None` |
    /// | 首次事件（无基准） | `Relayout` |
    /// | 位移 ≥ [`NEGLIGIBLE_DELTA_PX`] | `Relayout`（**原样传递偏移**） |
    /// | 位移 < 阈值 | `Skip` |
    pub fn take(&mut self) -> Option<ThrottleDecision> {
        let offset_px = self.pending_offset.take()?;

        let decision = match self.last_processed {
            None => ThrottleDecision::Relayout { offset_px },
            Some(previous) => {
                let delta = offset_px - previous;
                if delta.abs() < NEGLIGIBLE_DELTA_PX {
                    ThrottleDecision::Skip { delta_px: delta }
                } else {
                    ThrottleDecision::Relayout { offset_px }
                }
            }
        };

        // 只有真的重排才更新基准：Skip 时位置没变，基准不该动
        if decision.needs_relayout() {
            self.last_processed = Some(offset_px);
        }

        Some(decision)
    }

    /// 待处理计数（恒为 0 或 1）。
    ///
    /// 这个方法让"不累积"成为**可断言**的性质。
    #[must_use]
    pub fn pending_count(&self) -> usize {
        usize::from(self.pending_offset.is_some())
    }

    /// 上次已处理的位置（诊断用）。
    #[must_use]
    pub fn last_processed_offset(&self) -> Option<f32> {
        self.last_processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_event_always_relayouts() {
        let mut throttle = ScrollEventThrottle::new();
        throttle.on_scroll_event(0.0);
        let decision = throttle.take().expect("应有决策");
        assert!(decision.needs_relayout(), "首次事件应触发重排");
        assert_eq!(decision.offset_px(), Some(0.0));
    }

    #[test]
    fn empty_take_is_none() {
        let mut throttle = ScrollEventThrottle::new();
        assert_eq!(throttle.pending_count(), 0);
        assert!(throttle.take().is_none());
    }

    #[test]
    fn negligible_delta_is_skipped() {
        let mut throttle = ScrollEventThrottle::new();
        throttle.on_scroll_event(100.0);
        let _ = throttle.take();

        throttle.on_scroll_event(100.0 + NEGLIGIBLE_DELTA_PX / 2.0);
        let decision = throttle.take().expect("应有决策");
        assert!(!decision.needs_relayout(), "亚阈值位移应跳过");
        assert_eq!(
            throttle.last_processed_offset(),
            Some(100.0),
            "Skip 时基准不应移动"
        );
    }

    #[test]
    fn exact_threshold_is_not_negligible() {
        let mut throttle = ScrollEventThrottle::new();
        throttle.on_scroll_event(0.0);
        let _ = throttle.take();

        // 恰好等于阈值：判据是 `<` 阈值才跳过，故等于时应重排
        throttle.on_scroll_event(NEGLIGIBLE_DELTA_PX);
        let decision = throttle.take().expect("应有决策");
        assert!(decision.needs_relayout(), "恰好等于阈值应重排（判据是 <）");
    }

    #[test]
    fn pending_never_exceeds_one() {
        let mut throttle = ScrollEventThrottle::new();
        for step in 0..500 {
            throttle.on_scroll_event(step as f32 * 3.0);
            assert!(throttle.pending_count() <= 1);
        }
        assert_eq!(throttle.pending_count(), 1);
    }

    #[test]
    fn offset_is_passed_through_exactly() {
        let mut throttle = ScrollEventThrottle::new();
        throttle.on_scroll_event(1234.5);
        assert_eq!(
            throttle.take().and_then(ThrottleDecision::offset_px),
            Some(1234.5)
        );
    }
}
