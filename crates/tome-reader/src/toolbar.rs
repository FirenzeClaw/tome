//! `[DONE]` 阅读工具栏状态机与几何（T046 / FR-034）
//!
//! # 职责分界（裁决 OQ#8→A，FR-034 明文）
//!
//! > 本层持有"隐形/唤出/热区/入口清单"的**状态与几何**，绘制由 S3 外壳
//! > 承担；本层 MUST 通过明确的接口暴露这些状态与边界（MUST NOT 要求 S3
//! > 反向查询本层内部状态）。
//!
//! 故本模块**不画任何东西**，它产出两样可被外壳直接消费的东西：
//!
//! | 产出 | 形态 | 外壳怎么用 |
//! |---|---|---|
//! | [`ReadingToolbar::visibility`] / [`ReadingToolbar::opacity`] | 枚举 + 0–1 浮点 | 决定画不画、用什么 alpha |
//! | [`ToolbarBounds`] | 纯几何值 | 热区命中判定、决定工具栏本体画在哪 |
//!
//! ## 为什么状态机吃时间增量而不是读时钟
//!
//! [`ReadingToolbar::advance`] 接收毫秒增量，而不是自己去问系统时间。理由
//! 是**可测**：读时钟的版本只能用 `sleep` 测状态迁移，那既慢又 flaky；
//! 吃增量的版本可以精确快进到任意时刻、断言每一步的状态——本模块
//! `tests/toolbar.rs` 的全部时间相关断言都建立在这条上。
//!
//! 副作用（谁提供增量、多久调一次）留给外壳：它天然有帧循环。
//!
//! ## 热区为什么是"整窗顶部全域"
//!
//! MASTER §5.2 的版心在单页与双页下宽度不同（双页 = 两栏 + 中缝）。若热区
//! 跟着版心走，双页模式下把鼠标移到窗口顶部靠边处就唤不出工具栏——表现为
//! "工具栏失灵"，而原因很难联想到版心。故热区锚在**窗口**上沿
//! （[`ToolbarBounds::for_window`] 取 `window_width_px` 而非 `measure_px`）。

use tome_core::model::theme::{Theme, MOTION, READING_TOOLBAR};

/// 工具栏的可见态（FR-034 的状态机状态集）。
///
/// ## 为什么"淡出中"是独立状态而不是"可见 + 一个进度值"
///
/// 三者对绘制者的意义不同：
///
/// - [`Self::Hidden`]：**完全不画**（也不接收输入）
/// - [`Self::FadingOut`]：**仍在画**，但 alpha 在降（且此时任何活动都应
///   取消淡出——那是一个状态迁移，而不是"改个进度值"）
/// - [`Self::Visible`]：画，且静止计时在跑
///
/// 把 `FadingOut` 折进 `Visible` 会让"活动取消淡出"这条迁移无处挂载。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarVisibility {
    /// 完全隐形（默认态）
    Hidden,
    /// 可见（静止计时进行中）
    Visible,
    /// 正在淡出（alpha 下降中；此时活动应取消淡出）
    FadingOut,
}

/// 驱动状态机的输入事件（外壳把系统事件翻译成这些）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarInput {
    /// 指针进入顶部热区
    PointerInHotZone,
    /// 指针离开阅读区
    PointerLeftReadingArea,
    /// 显式唤出（键盘，FR-035 的`工具栏唤出`键）
    RevealRequested,
    /// 显式隐藏（`Esc` 退出沉浸态，FR-035 的栈式取消）
    HideRequested,
}

/// 工具栏几何（外壳绘制与命中判定用）。
///
/// ## 字段都是"窗口坐标"下的值
///
/// 外壳拿到后不需要任何换算：热区是 `[0, hot_zone_width_px) ×
/// [hot_zone_top_px, hot_zone_top_px + hot_zone_height_px)`，工具栏本体
/// 从 `toolbar_top_px` 起、高 `toolbar_height_px`。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolbarBounds {
    /// 热区宽度（px）——**等于窗口宽度**（整窗顶部全域）
    pub hot_zone_width_px: f32,
    /// 热区高度（px，MASTER §5.3 的 8px）
    pub hot_zone_height_px: f32,
    /// 热区上沿（px）——窗口上沿，恒为 0
    pub hot_zone_top_px: f32,
    /// 工具栏本体高（px）
    pub toolbar_height_px: f32,
    /// 工具栏本体上沿（px）
    pub toolbar_top_px: f32,
}

impl ToolbarBounds {
    /// 工具栏本体高度（px）。
    ///
    /// ## 为什么是 44
    ///
    /// MASTER §4 的统一控件高是 32px（`--control-h`），加上下各 6px 内边距
    /// 得 44px。这不是 MASTER 直接写出的数字，而是由它的两个 token 推出的
    /// ——故在这里算而不是取常量。
    pub const TOOLBAR_HEIGHT_PX: f32 = 44.0;

    /// 由窗口尺寸与版心宽度构造几何。
    ///
    /// ## 参数里为什么带 `measure_px` 却不用它
    ///
    /// 签名保留版心宽度是**有意**的：它记录了"这里曾考虑过按版心算热区"
    /// 这个被否决的方案，且让调用点显式传一个当时已知的量（而不是让读者
    /// 猜"热区为什么与版心无关"）。实现不使用它——见本类型的文档说明。
    #[must_use]
    pub fn for_window(
        window_width_px: f32,
        _window_height_px: f32,
        _measure_px: f32,
        hot_zone_height_px: u16,
    ) -> Self {
        Self {
            hot_zone_width_px: window_width_px.max(0.0),
            hot_zone_height_px: f32::from(hot_zone_height_px),
            hot_zone_top_px: 0.0,
            toolbar_height_px: Self::TOOLBAR_HEIGHT_PX,
            toolbar_top_px: 0.0,
        }
    }

    /// 由**注入的主题**构造几何（FR-029 的注入路径）。
    #[must_use]
    pub fn for_window_with_theme(
        window_width_px: f32,
        window_height_px: f32,
        measure_px: f32,
        theme: &Theme,
    ) -> Self {
        Self::for_window(
            window_width_px,
            window_height_px,
            measure_px,
            theme.reading_toolbar().hot_zone_height_px,
        )
    }

    /// 某点是否落在热区内（半开区间）。
    ///
    /// ## 为什么下沿是半开（`y < height` 而非 `<=`）
    ///
    /// 热区下沿紧邻正文首行。若取闭区间，`y == 8` 这个点会同时属于热区与
    /// 首行文字——用户点在这里究竟是"唤出工具栏"还是"点中第一个字"就成了
    /// 实现细节。半开区间把它明确判给正文。
    #[must_use]
    pub fn contains_in_hot_zone(&self, x_px: f32, y_px: f32) -> bool {
        x_px >= 0.0
            && x_px < self.hot_zone_width_px
            && y_px >= self.hot_zone_top_px
            && y_px < self.hot_zone_top_px + self.hot_zone_height_px
    }
}

impl Default for ToolbarBounds {
    fn default() -> Self {
        Self {
            hot_zone_width_px: 0.0,
            hot_zone_height_px: f32::from(READING_TOOLBAR.hot_zone_height_px),
            hot_zone_top_px: 0.0,
            toolbar_height_px: Self::TOOLBAR_HEIGHT_PX,
            toolbar_top_px: 0.0,
        }
    }
}

/// 工具栏入口清单（FR-034 / MASTER §5.3 的十项，稳定标识）。
///
/// 与 `tome_core::model::theme::ReadingToolbar::entries` 同源——本常量是
/// 它在阅读层的可枚举视图（`&[&str]` 便于遍历与断言，常量数组便于逐项比较）。
pub const TOOLBAR_ENTRIES: &[&str] = &[
    "back",
    "toc",
    "progress",
    "font_size",
    "line_height",
    "theme",
    "spread",
    "bookmark",
    "change_source",
    "settings",
];

/// 阅读工具栏状态机（FR-034）。
///
/// ## 它持有的状态
///
/// | 字段 | 作用 |
/// |---|---|
/// | `visibility` | 三态（隐形 / 可见 / 淡出中） |
/// | `idle_ms` | 可见态下累计的静止毫秒（越阈值即进入淡出） |
/// | `fade_ms` | 淡出态下累计的已淡出毫秒（越阈值即隐形） |
/// | `bounds` | 几何（外壳可随时查） |
///
/// ## 不持有的东西
///
/// 不持有真实时钟、不持有图形句柄、不持有"当前焦点在哪"之类的界面状态
/// ——那些属外壳。本类型是纯值，可被 `Copy`/比较/跨帧持有。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingToolbar {
    visibility: ToolbarVisibility,
    idle_ms: u32,
    fade_ms: u32,
    bounds: ToolbarBounds,
}

impl ReadingToolbar {
    /// 新建（默认隐形，FR-034 / MASTER §5.3）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            visibility: ToolbarVisibility::Hidden,
            idle_ms: 0,
            fade_ms: 0,
            bounds: ToolbarBounds::default(),
        }
    }

    /// 更新几何（窗口尺寸变化时外壳调用）。
    pub fn set_bounds(&mut self, bounds: ToolbarBounds) {
        self.bounds = bounds;
    }

    /// 当前几何。
    #[must_use]
    pub const fn bounds(&self) -> ToolbarBounds {
        self.bounds
    }

    /// 当前可见态。
    #[must_use]
    pub const fn visibility(&self) -> ToolbarVisibility {
        self.visibility
    }

    /// 是否可见（含淡出中——那时**仍在画**）。
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        !matches!(self.visibility, ToolbarVisibility::Hidden)
    }

    /// 入口清单（外壳直接渲染这十项）。
    #[must_use]
    pub const fn entries(&self) -> &'static [&'static str] {
        TOOLBAR_ENTRIES
    }

    /// 当前不透明度（0.0–1.0；外壳拿去当 alpha）。
    ///
    /// ## 三种状态的取值
    ///
    /// | 状态 | 取值 |
    /// |---|---|
    /// | 隐形 | 0.0 |
    /// | 可见 | 1.0 |
    /// | 淡出中 | `1.0 - fade_ms / dur_base`（线性） |
    ///
    /// 线性而非缓动曲线：缓动曲线属绘制层（外壳可以再套一条），本层给的是
    /// **原始进度**。若本层做了缓动，外壳就无法改成别的曲线了。
    #[must_use]
    pub fn opacity(&self) -> f32 {
        match self.visibility {
            ToolbarVisibility::Hidden => 0.0,
            ToolbarVisibility::Visible => 1.0,
            ToolbarVisibility::FadingOut => {
                let duration = MOTION.dur_base_ms.max(1) as f32;
                (1.0 - self.fade_ms as f32 / duration).clamp(0.0, 1.0)
            }
        }
    }

    /// 施加一个输入事件（状态迁移）。
    ///
    /// ## 迁移表
    ///
    /// | 当前 | 输入 | 结果 |
    /// |---|---|---|
    /// | 任意 | `RevealRequested` / `PointerInHotZone` | 可见（计时清零） |
    /// | 可见 / 淡出中 | `PointerLeftReadingArea` | **立即**淡出 |
    /// | 任意 | `HideRequested` | 隐形（`Esc` 的落点） |
    /// | 隐形 | `PointerInHotZone` | 可见 |
    pub fn apply(&mut self, input: ToolbarInput) {
        match input {
            ToolbarInput::RevealRequested | ToolbarInput::PointerInHotZone => {
                // 唤出（含从淡出中恢复）：计时全部清零——否则会沿用上次的
                // 静止计时，表现为"刚唤出就立刻淡出"。
                self.visibility = ToolbarVisibility::Visible;
                self.idle_ms = 0;
                self.fade_ms = 0;
            }
            ToolbarInput::PointerLeftReadingArea => {
                // 只对"正在显示"的工具栏有效：隐形时移出阅读区无事发生
                if self.is_visible() {
                    self.visibility = ToolbarVisibility::FadingOut;
                    self.fade_ms = 0;
                }
            }
            ToolbarInput::HideRequested => {
                self.visibility = ToolbarVisibility::Hidden;
                self.idle_ms = 0;
                self.fade_ms = 0;
            }
        }
    }

    /// 推进时间（毫秒）。外壳在每帧调用。
    ///
    /// ## 三段推进
    ///
    /// 1. **隐形**：无操作（推进时间不该让隐形的东西冒出来）
    /// 2. **可见**：累计 `idle_ms`；越 `idle_fade_after_ms` 即转入淡出，
    ///    **并把超出部分记入 `fade_ms`**——否则时间会在转移处丢失，
    ///    淡出总时长会比 `dur_base` 长
    /// 3. **淡出中**：累计 `fade_ms`；越 `dur_base` 即转隐形
    pub fn advance(&mut self, elapsed_ms: u32) {
        match self.visibility {
            ToolbarVisibility::Hidden => {}
            ToolbarVisibility::Visible => {
                self.idle_ms = self.idle_ms.saturating_add(elapsed_ms);
                let threshold = READING_TOOLBAR.idle_fade_after_ms;
                if self.idle_ms > threshold {
                    // 把超阈值的部分带入淡出计时，避免时间在转移处丢失
                    self.fade_ms = self.idle_ms - threshold;
                    self.idle_ms = 0;
                    self.visibility = ToolbarVisibility::FadingOut;
                }
            }
            ToolbarVisibility::FadingOut => {
                self.fade_ms = self.fade_ms.saturating_add(elapsed_ms);
                if self.fade_ms > MOTION.dur_base_ms {
                    self.visibility = ToolbarVisibility::Hidden;
                    self.fade_ms = 0;
                }
            }
        }
    }
}

impl Default for ReadingToolbar {
    fn default() -> Self {
        Self::new()
    }
}
