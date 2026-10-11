//! `[WIP]` 加载骨架（S3 T013/T014；MASTER `LoadingSkeleton`）
//!
//! ## MASTER 的硬要求
//!
//! > `LoadingSkeleton` | 骨架屏，微光扫过（`prefers-reduced-motion` 降级为静态）
//!
//! 两条都在这里落实：微光扫过由相位驱动的渐变实现，降级由
//! [`tome_core::model::theme::Motion::durations`] 的 `effective(reduced_motion)`
//! 决定——即"动效时长归零 ⇒ 不扫光"。
//!
//! ## 为什么骨架比"正在加载…"四个字好
//!
//! 骨架的价值不在美观，而在**它预告了内容将出现在哪里**：用户看到的是"这里
//! 将会有一张卡片 / 一行字"，于是等待期间界面结构是稳定的。而一行居中的
//! "正在加载…"在内容到达时会引发一次整屏跳变——那比多等 200ms 更难受。
//!
//! FR-010 的"不白屏"是最低要求；骨架是它的**具体形态**。

use egui::{Ui, Vec2};

use crate::theme_bridge::BridgeTheme;

/// 画一个骨架块（矩形 + 微光）。
///
/// `phase` 是 0.0–1.0 的动画相位（由调用方按帧时间推进）。
/// `reduced_motion` 为真时**不扫光**（只画静态底色）。
pub fn block(ui: &mut Ui, theme: &BridgeTheme, size: Vec2, phase: f32, reduced_motion: bool) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let palette = theme.theme().palette();
    let base = theme.color(palette.bg_inset);

    if reduced_motion {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(theme.theme().radii().sm_px as u8),
            base,
        );
        return;
    }

    ui.painter().rect_filled(
        rect,
        egui::CornerRadius::same(theme.theme().radii().sm_px as u8),
        base,
    );

    // 微光：一条竖直的亮带从左扫到右。用 `bg_elevated` 而非"更亮的白"——
    // 后者会在夜主题下变成刺眼的亮条（而 token 的 elevated 在两主题下都
    // 是"比底色亮一档"的那个值）。
    let sweep_width = rect.width() * 0.35;
    let travel = rect.width() + sweep_width;
    let left = rect.left() - sweep_width + travel * phase.clamp(0.0, 1.0);
    let sweep = egui::Rect::from_min_max(
        egui::pos2(left.max(rect.left()), rect.top()),
        egui::pos2((left + sweep_width).min(rect.right()), rect.bottom()),
    );
    if sweep.width() > 0.0 {
        ui.painter().rect_filled(
            sweep,
            egui::CornerRadius::same(theme.theme().radii().sm_px as u8),
            theme.color(palette.bg_elevated),
        );
    }
}

/// 微光相位的推进（**纯函数**，便于断言"降级时不推进"）。
///
/// 一个完整扫描周期取 MASTER 的动效中档时长；周期归零（reduced motion）
/// 时相位恒为 0，于是不会画出移动的亮带。
#[must_use]
pub fn advance_phase(phase: f32, elapsed_ms: f32, reduced_motion: bool) -> f32 {
    if reduced_motion {
        return 0.0;
    }
    let period = sweep_period_ms();
    if period <= 0.0 {
        return 0.0;
    }
    let next = phase + elapsed_ms / period;
    next - next.floor()
}

/// 微光扫描周期（毫秒）。
///
/// 取 MASTER 动效的"慢档"量级——扫光比交互反馈慢，否则会显得焦躁。它是
/// **本组件的取值**而非 token（MASTER 未给骨架的时长），故硬编码一处并注明。
#[must_use]
pub fn sweep_period_ms() -> f32 {
    1_200.0
}
