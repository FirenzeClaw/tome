//! [DONE] 翻页动效 + reduced-motion 降级（T038 / FR-046、FR-032）
//!
//! ## 本模块的职责边界
//!
//! **只产出动效参数与进度曲线**——不绘制、不持有计时器、不碰状态机。
//! 这样动效可以完全脱离 GPU 单测（进度曲线是纯函数），而绘制由
//! [`super::batch`] 的产出去驱动。
//!
//! ## 三条档位依据（MASTER.md §4 / MOTION.md）
//!
//! | 元素 | 时长 | 缓动 | 出处 |
//! |---|---|---|---|
//! | 翻页 | `--dur-slow`（280ms） | `--ease-spring` | MOTION.md §2 的元素映射表 |
//! | 滚动平滑 | `--dur-base` | `--ease-out` | 同上（滚动归"结构变化"） |
//! | 降级档 | 0ms | 不适用 | FR-032 |
//!
//! ## `--ease-spring` 的使用约束（MOTION.md §1，本模块显式表达）
//!
//! > `--ease-spring` 的控制点 `1.3` 会产生约 3% 的过冲——只允许用于
//! > 「从无到有」的入场，**不允许用于位置移动**（过冲会让文字错位一瞬）。
//!
//! 故 [`PageTurn::easing_for`] 对**位移**通道返回 `Out`，只有**淡入/缩放**
//! 通道返回 `Spring`。这不是"保守起见"，是该约束的字面落地：翻页的位移
//! 是下一页内容"从旁边进来"，过冲会让它先超过目标位置再弹回——正文错位
//! 一瞬，正好是该约束要防的现象。
//!
//! ## 为什么进度曲线要乘三控制点（而不是直接 linear）
//!
//! `--ease-spring` 是 `cubic-bezier(.34, 1.3, .64, 1)`——**四**个控制点
//! （两端的 (0,0)/(1,1) 隐含）。本模块用标准的 cubic-bezier 求解器把它
//! 求值成 `t → progress`。这样"动效观感"直接来自设计 token，
//! 而不是实现者另调一条曲线。

use tome_core::model::theme::Theme;

/// 缓动类型（对应 MASTER §4 的两个 token）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EasingKind {
    /// `--ease-out`：`cubic-bezier(.22,.61,.36,1)`，通用
    Out,
    /// `--ease-spring`：`cubic-bezier(.34,1.3,.64,1)`，弹入（含轻微过冲）
    Spring,
    /// 无缓动（线性；降级档与"直接位移"用）
    Linear,
}

impl EasingKind {
    /// 对应的 cubic-bezier 控制点（`[x1, y1, x2, y2]`）。
    #[must_use]
    pub fn control_points(self, theme: Theme) -> [f32; 4] {
        match self {
            Self::Out => theme.motion().ease_out,
            Self::Spring => theme.motion().ease_spring,
            // 线性 = 控制点在 (1/3,1/3) 与 (2/3,2/3)——贝塞尔退化为直线
            Self::Linear => [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
        }
    }
}

/// 受动效影响的通道。
///
/// ## 为什么要区分通道（而不是"一个动效一种缓动"）
///
/// MOTION.md §1 的约束是**按通道**给"差异"的：位移不能用 `--ease-spring`
/// （过冲让文字错位），而淡入可以用。若把"翻页动效"当成一个整体来配缓动，
/// 就无法表达这条约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Animation {
    /// 位移（位置移动）
    Translate,
    /// 淡入/淡出
    Fade,
    /// 缩放
    Scale,
}

/// 减少动效偏好（FR-032）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionPreference {
    /// 正常档位（按 MASTER §4 的时长与缓动）
    #[default]
    Normal,
    /// 减少动效（时长 0、无平滑缓动）
    Reduced,
}

impl MotionPreference {
    /// 是否降级。
    #[must_use]
    pub fn is_reduced(self) -> bool {
        matches!(self, Self::Reduced)
    }
}

/// 减少动效开关（语义与系统设置同名，便于组合根接线）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReducedMotion;

impl ReducedMotion {
    /// 由布尔开关得到偏好（`true` = 开启减少动效）。
    #[must_use]
    pub fn from_flag(enabled: bool) -> MotionPreference {
        if enabled {
            MotionPreference::Reduced
        } else {
            MotionPreference::Normal
        }
    }
}

/// 翻页动效的参数（时长 + 各通道缓动 + 偏好）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageTurn {
    duration_ms: u32,
    spring_easing: EasingKind,
    out_easing: EasingKind,
    preference: MotionPreference,
    /// `--ease-spring` 的控制点（求值用）
    spring_points: [f32; 4],
    /// `--ease-out` 的控制点（求值用）
    out_points: [f32; 4],
}

impl PageTurn {
    /// 由主题构造（MASTER §4 的翻页档位：`--dur-slow` + `--ease-spring`）。
    #[must_use]
    pub fn for_theme(theme: Theme) -> Self {
        Self {
            duration_ms: theme.motion().dur_slow_ms,
            spring_easing: EasingKind::Spring,
            out_easing: EasingKind::Out,
            preference: MotionPreference::Normal,
            spring_points: theme.motion().ease_spring,
            out_points: theme.motion().ease_out,
        }
    }

    /// 动效时长（ms）。降级时为 0。
    #[must_use]
    pub fn duration_ms(&self) -> u32 {
        match self.preference {
            MotionPreference::Reduced => 0,
            MotionPreference::Normal => self.duration_ms,
        }
    }

    /// 翻页动效的主缓动（用于淡入/缩放入场）。
    #[must_use]
    pub fn easing(&self) -> EasingKind {
        self.spring_easing
    }

    /// 某通道的缓动。
    ///
    /// ## 位移通道**不用** `--ease-spring`（MOTION.md §1 的约束）
    ///
    /// 见模块文档。降级档下一切通道都是线性（且时长为 0，故实际不产生帧）。
    #[must_use]
    pub fn easing_for(&self, animation: Animation) -> EasingKind {
        if self.preference.is_reduced() {
            return EasingKind::Linear;
        }
        match animation {
            // 位移不得过冲（会让正文错位一瞬）
            Animation::Translate => self.out_easing,
            // 淡入/缩放是「从无到有」的入场，允许弹入
            Animation::Fade | Animation::Scale => self.spring_easing,
        }
    }

    /// 滚动是否有平滑缓动。
    ///
    /// FR-032："滚动 MUST NOT 使用平滑缓动动画（直接位移）"——降级时返回
    /// `None` 表示"直接位移"（无过渡帧）。
    #[must_use]
    pub fn scroll_easing(&self) -> Option<EasingKind> {
        if self.preference.is_reduced() {
            None
        } else {
            Some(self.out_easing)
        }
    }

    /// 当前偏好。
    #[must_use]
    pub fn preference(&self) -> MotionPreference {
        self.preference
    }

    /// 是否瞬时（时长 0）。
    #[must_use]
    pub fn is_instant(&self) -> bool {
        self.duration_ms() == 0
    }

    /// 换用另一档偏好。
    #[must_use]
    pub fn with_preference(mut self, preference: MotionPreference) -> Self {
        self.preference = preference;
        self
    }

    /// 从动效开始算起 `elapsed_ms` 毫秒时的进度（0–1）。
    ///
    /// ## 三个边界条件
    ///
    /// - **降级**：恒为 1（立即完成，FR-032）
    /// - **`elapsed = 0`**：0（动效刚触发）
    /// - **`elapsed ≥ duration`**：1（钳住，不外推）
    ///
    /// 曲线用 `--ease-spring` 的控制点求值——故"弹性"来自设计 token
    /// 而非实现自调。
    #[must_use]
    pub fn progress_at(&self, elapsed_ms: u32) -> f32 {
        let duration = self.duration_ms();
        if duration == 0 {
            // 降级：立即完成
            return 1.0;
        }
        if elapsed_ms >= duration {
            return 1.0;
        }

        let t = elapsed_ms as f32 / duration as f32;
        // 位移是翻页的主体观感，故用**位移通道**的缓动（不得过冲）
        let points = match self.easing_for(Animation::Translate) {
            EasingKind::Out => self.out_points,
            EasingKind::Spring => self.spring_points,
            EasingKind::Linear => [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
        };

        cubic_bezier_y_at_x(points, t.clamp(0.0, 1.0)).clamp(0.0, 1.0)
    }
}

/// 求 `cubic-bezier(p1x, p1y, p2x, p2y)` 在横坐标 `x` 处的纵坐标。
///
/// ## 为什么需要求解而非直接代入
///
/// CSS 的 cubic-bezier 是**参数曲线**：`(x(u), y(u))`，而调用方给的是
/// **横坐标 x**（时间进度），要的是对应的 y（动画进度）。故必须先解出
/// 使 `x(u) = x` 的参数 `u`，再取 `y(u)`。
///
/// ## 求解方法：二分
///
/// 20 次二分把区间缩到 `2⁻²⁰ ≈ 1e-6`，对 280ms 的动效而言是纳秒级精度，
/// 远超可视阈值。用二分而非牛顿法是因为它**无需导数、必定收敛**，
/// 且对控制点引起的非单调情形（`y1 > 1` 的过冲）也稳定——那正是
/// `--ease-spring` 的形态。
///
/// ## 输出**钳在 [0,1]**
///
/// `--ease-spring` 的 `y1 = 1.3` 会让曲线在中段超过 1（那就是"过冲"）。
/// 过冲对**位移**是禁止的（MOTION.md §1），故调用方钳住。本函数自身
/// 也钳——双保险，且让"进度"在任何通道上都是一个 0–1 的量。
fn cubic_bezier_y_at_x(points: [f32; 4], x: f32) -> f32 {
    let [x1, y1, x2, y2] = points;

    // 线性退化：控制点在中线时 `x(u) = u`，无需求解
    let is_linear = (x1 - 1.0 / 3.0).abs() < 1e-6
        && (y1 - 1.0 / 3.0).abs() < 1e-6
        && (x2 - 2.0 / 3.0).abs() < 1e-6
        && (y2 - 2.0 / 3.0).abs() < 1e-6;
    if is_linear {
        return x;
    }

    // 三次贝塞尔的两个分量：B(u) = 3(1-u)²u·p1 + 3(1-u)u²·p2 + u³
    let bezier = |p1: f32, p2: f32, u: f32| -> f32 {
        let one_minus = 1.0 - u;
        3.0 * one_minus * one_minus * u * p1 + 3.0 * one_minus * u * u * p2 + u * u * u
    };

    let mut low = 0.0f32;
    let mut high = 1.0f32;
    for _ in 0..20 {
        let middle = (low + high) / 2.0;
        if bezier(x1, x2, middle) < x {
            low = middle;
        } else {
            high = middle;
        }
    }
    let u = (low + high) / 2.0;
    bezier(y1, y2, u).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::model::theme::DayTheme;

    #[test]
    fn linear_easing_is_the_identity() {
        let points = EasingKind::Linear.control_points(DayTheme.theme());
        for x in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            assert!(
                (cubic_bezier_y_at_x(points, x) - x).abs() < 1e-4,
                "线性缓动应是恒等映射，x={x}"
            );
        }
    }

    #[test]
    fn bezier_endpoints_are_exact() {
        // 任何 cubic-bezier 的两端都是 (0,0) 与 (1,1)
        for kind in [EasingKind::Out, EasingKind::Spring, EasingKind::Linear] {
            let points = kind.control_points(DayTheme.theme());
            assert!(
                cubic_bezier_y_at_x(points, 0.0).abs() < 1e-4,
                "{kind:?} 在 x=0 处应为 0"
            );
            assert!(
                (cubic_bezier_y_at_x(points, 1.0) - 1.0).abs() < 1e-4,
                "{kind:?} 在 x=1 处应为 1"
            );
        }
    }

    /// **`--ease-out` 是"先快后慢"**（在 x=0.5 处已走过一半以上）。
    #[test]
    fn ease_out_advances_faster_than_linear_early_on() {
        let points = EasingKind::Out.control_points(DayTheme.theme());
        let at_half = cubic_bezier_y_at_x(points, 0.5);
        assert!(
            at_half > 0.5,
            "--ease-out 应在中段已走过一半以上（实测 {at_half}）"
        );
    }

    /// 输出恒在 [0,1]（含 `--ease-spring` 的过冲区段）。
    #[test]
    fn output_is_always_clamped() {
        let points = EasingKind::Spring.control_points(DayTheme.theme());
        for step in 0..=50 {
            let x = step as f32 / 50.0;
            let y = cubic_bezier_y_at_x(points, x);
            assert!((0.0..=1.0).contains(&y), "输出应钳在 [0,1]，x={x} 时得 {y}");
        }
    }

    #[test]
    fn page_turn_is_instant_when_reduced() {
        let turn = PageTurn::for_theme(DayTheme.theme());
        assert!(!turn.is_instant());
        let reduced = turn.with_preference(MotionPreference::Reduced);
        assert!(reduced.is_instant());
        assert_eq!(reduced.progress_at(0), 1.0);
    }
}
