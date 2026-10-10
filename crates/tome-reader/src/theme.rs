//! \[DONE] 阅读层语义色映射（T041 / FR-030、FR-031）
//!
//! # 本模块是"设计 token → 阅读层用色"的**唯一**转换点
//!
//! `tome-core` 的 `Theme` 是设计 token 的纯值投影（20 个语义色 + 几何）。
//! 但阅读层要用的不是"token 名"，而是**具体角色的颜色**：阅读底、正文色、
//! 高亮下划线、focus 环……本模块把前者映射成后者，且**在映射过程中施加
//! MASTER 的强制约束**（无障碍项）。
//!
//! ## 为什么要有这一层（而不是各处直接 `theme.palette().bg_reading`）
//!
//! 三件事只有集中在一处才成立：
//!
//! 1. **强制项有落点**：MASTER §2.3 的"正文行高 ≥1.75"、"focus 环 2px"、
//!    "触达 ≥32px" 是**引擎级强制**——它们不该靠每个绘制点自己记得。
//! 2. **语义与 token 名解耦**：绘制点说"我要正文色"，不说
//!    `text_primary`。将来主题若引入第三套配色，改的是这里一处。
//! 3. **可测**：`tests/theme_accessibility.rs` 逐项断言 MASTER §8 的
//!    Don'ts 与 §2.3 的对比度表——若映射散在各处，那些断言无处可施。
//!
//! ## 对比度工具函数为什么放在这里
//!
//! `contrast_ratio` / `relative_luminance` 是 WCAG 2.1 的**定义**，与主题
//! 无关，本可放在别处。放在这里是因为它们是"无障碍强制"的一部分——本模块
//! 的文档与测试都以它们为尺子。它们是纯函数，无状态、无依赖。
//!
//! ## 不做的事
//!
//! 本模块**不产出**任何图形类型（`wgpu::Color` 等）——那是 `render` 模块的
//! 职责（crate 的"排版不依赖渲染"铁律同样适用于此）。`ReadingPalette` 的
//! 字段全是 `tome_core` 的 `Color` 与裸整数。

use tome_core::model::theme::{Color, Theme};

/// 阅读区用色与无障碍强制尺寸。
///
/// ## 字段为什么是"角色名"而不是"token 名"
///
/// `background` 而不是 `bg_reading`：绘制点关心的是"这块区域的底色"，
/// 而不是"它取自哪个 token"。token 的选择是本模块的关切（且是 FR-030
/// 的强制项），不该泄漏给调用点。
///
/// ## 为什么 `Copy` 且无堆分配
///
/// 绘制点按帧取用（每帧调用一次），应当零成本。字段全是 `Color`（3×u8）
/// 与 `u16`/`u32`，天然满足。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadingPalette {
    /// 阅读区底色（**强制取自 `bg_reading`**，FR-030）
    pub background: Color,
    /// 正文墨色（**强制取自 `text_primary`**，且不低于 `text_secondary`）
    pub body_text: Color,
    /// 次要文本色（页脚、行号等）
    pub secondary_text: Color,
    /// 强调色（当前项、进度条、focus 环）
    pub accent: Color,
    /// 高亮区间的下划线色（FR-033 的默认样式）
    pub highlight_underline: Color,
    /// 高亮区间的背景色（半透明层之下的实色底）
    pub highlight_background: Color,
    /// focus 环色（**强制取 `accent_primary`**，FR-031 要求两主题均可见）
    pub focus_ring: Color,
    /// 页脚文字色（比正文弱，但不低于可读门槛）
    pub footer_text: Color,
    /// 正文字重（**强制 ≤700**，MASTER §8 Don'ts）
    pub body_weight: u16,
    /// focus 环宽度（px，**强制 2**，FR-031）
    pub focus_ring_width_px: u16,
    /// focus 环相对元素边界的 offset（px，MASTER §2.3 的 2px）
    pub focus_ring_offset_px: u16,
    /// 最小触达目标尺寸（px，**强制 ≥32**，MASTER §2.3）
    pub min_target_size_px: u16,
    /// 图标按钮的最小触达目标（px，**强制 ≥36**，MASTER §2.3）
    pub min_icon_target_size_px: u16,
}

impl ReadingPalette {
    /// 从主题派生阅读层用色（**唯一的映射入口**）。
    ///
    /// ## 映射规则与它们的依据
    ///
    /// | 字段 | 取自 | 依据 |
    /// |---|---|---|
    /// | `background` | `bg_reading` | FR-030（"最纯净层"） |
    /// | `body_text` | `text_primary` | FR-030（正文色不低于 `text_secondary`） |
    /// | `secondary_text` | `text_secondary` | —— |
    /// | `accent` | `accent_primary` | §2.3 对比度表 |
    /// | `highlight_underline` | `accent_secondary` | 与正文色相区分且属暖色系 |
    /// | `highlight_background` | `accent_soft` | 高亮底（昼 `#E4D3B8` / 夜 `#2A1A12`） |
    /// | `focus_ring` | `accent_primary` | FR-031（§2.3 要求两主题均可见） |
    /// | `footer_text` | `text_tertiary` | 页脚比正文弱，但仍取主题已定的层级 |
    ///
    /// ## 尺寸为什么是常量而非取主题
    ///
    /// `focus_ring_width_px = 2` 等是 MASTER §2.3 的**强制数值**，不是
    /// 可调 token（主题里没有对应字段，因为不该有）。写成常量让"某主题
    /// 擅自把 focus 环调细"不可能发生——这是"引擎级强制"的实现。
    #[must_use]
    pub fn for_theme(theme: &Theme) -> Self {
        let palette = theme.palette();
        Self {
            background: palette.bg_reading,
            body_text: palette.text_primary,
            secondary_text: palette.text_secondary,
            accent: palette.accent_primary,
            highlight_underline: palette.accent_secondary,
            highlight_background: palette.accent_soft,
            focus_ring: palette.accent_primary,
            footer_text: palette.text_tertiary,
            body_weight: theme.type_scale().body.font_weight,
            // MASTER §2.3 的强制值（非 token）
            focus_ring_width_px: 2,
            focus_ring_offset_px: 2,
            min_target_size_px: 32,
            min_icon_target_size_px: 36,
        }
    }
}

// ===========================================================================
// WCAG 2.1 对比度（无障碍强制项的尺子）
// ===========================================================================

/// sRGB 分量 → 线性光（WCAG 2.1 的定义）。
///
/// 阈值 `0.04045` 与除数 `12.92` 是 WCAG 规范里写死的（低亮度段用线性近似，
/// 高亮度段用 2.4 次幂）。`0.03928` 是更早的草案值——用错会让深色区
/// 的对比度差一点点，在"刚好卡在 4.5"的组合上产生误判。
fn linearize(channel: u8) -> f64 {
    let srgb = f64::from(channel) / 255.0;
    if srgb <= 0.040_45 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

/// 颜色的**相对亮度**（WCAG 2.1，0.0–1.0）。
///
/// 系数 `0.2126 / 0.7152 / 0.0722` 是人眼锥细胞对三原色的敏感度权重
/// （绿最高、蓝最低）——这正是"同样的灰度差，蓝字看着比绿字暗"的量化。
#[must_use]
pub fn relative_luminance(color: Color) -> f64 {
    0.2126f64.mul_add(
        linearize(color.red),
        0.7152f64.mul_add(linearize(color.green), 0.0722 * linearize(color.blue)),
    )
}

/// 两色的**对比度**（WCAG 2.1，1.0–21.0）。
///
/// 公式 `(L_lighter + 0.05) / (L_darker + 0.05)`——加 `0.05` 是为避免
/// 纯黑之间相除的除零，并让"最暗与最亮"的比值为 21 而非无穷。
#[must_use]
pub fn contrast_ratio(first: Color, second: Color) -> f64 {
    let first_luminance = relative_luminance(first);
    let second_luminance = relative_luminance(second);
    let (lighter, darker) = if first_luminance > second_luminance {
        (first_luminance, second_luminance)
    } else {
        (second_luminance, first_luminance)
    };
    (lighter + 0.05) / (darker + 0.05)
}

/// WCAG 门槛（`tests/theme_accessibility.rs` 与上层校验共用）。
///
/// ## 为什么把门槛写成常量而不是散在断言里
///
/// 4.5 / 3.0 这两个数在 WCAG 里是有名字的（AA 正文 / 非文本 UI），
/// 散在各处会让"某处写成 4.0"看起来只是个数字差。命名后，改动者能看出
/// 它对应的是哪一条规范。
pub mod thresholds {
    /// WCAG AA 正文文本门槛。
    pub const AA_BODY_TEXT: f64 = 4.5;
    /// WCAG AA 大字号文本与**非文本 UI 组件**门槛（focus 环等）。
    pub const AA_LARGE_TEXT_OR_UI: f64 = 3.0;
}
