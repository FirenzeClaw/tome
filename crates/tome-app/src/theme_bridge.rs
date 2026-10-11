//! `[WIP]` 主题桥：`tome_core::theme` → egui（S3 T008）
//!
//! ## 这一层的唯一职责
//!
//! 把 `tome_core::model::theme` 的**纯值 token** 翻译成 egui 的类型
//! （`Color32` / `FontId` / `CornerRadius` / `Visuals` / `Style`）。
//!
//! ## 零色值重声明（MASTER §10）
//!
//! 本模块**不出现任何十六进制色值**：每个颜色都从
//! [`tome_core::model::theme::Palette`] 取，每个字号都从 `TypeScale` 取。
//! 这条纪律由 `tests/theme_bridge.rs` 的 grep 守卫断言（扫描 `src/**/*.rs`）。
//!
//! 为什么值得一条守卫：违反它的形态非常自然——某个组件顺手写
//! `Color32::from_rgb(0x8C, 0x3A, 0x1E)`，代码能跑、观感"看起来也对"，
//! **没有任何测试会红**；等到设计系统改一次主色，那一处就成了不跟随的孤岛。
//!
//! ## R7 四规则（MASTER 已核实缺陷的处置）
//!
//! | 规则 | 事实（`research-design-system.md` §1.2 复算） | 本模块的处置 |
//! |---|---|---|
//! | ① 鎏金禁作文本前景 | `--accent-clash` 昼 2.20–2.95:1 | 无任何"取文本色"入口返回它 |
//! | ② `text-tertiary` 禁必要信息 | 昼 2.96–3.97 / 夜 3.25–3.73 | [`BridgeTheme::text_color_for`] 的 `Required` 退到 secondary |
//! | ③ `border-strong` 不作唯一边界 | 最大 2.10:1（< 3:1） | [`BadgeStyle::input_field`] 强制"底 + 环" |
//! | ④ badge 不照抄 preview | 夜主题 `.badge` 仅 1.63:1 | [`BridgeTheme::badge_style`] 前景**随主题切换** |
//!
//! ## 跟随系统（R7 的补充裁决）
//!
//! 设计系统**没有**"跟随系统"机制（`tokens.css` 无 `prefers-color-scheme`）。
//! S3 以 OS 探测映射到 Day/Night token 实现——[`FollowSystem`] 把探测源做成
//! **可注入**的，这样"OS 切换时返回对应 token"能被单元测试覆盖，而不需要
//! 真的去改系统设置。

use std::sync::{Arc, Mutex};

use tome_core::model::theme::{Color, DayTheme, NightTheme, Palette, Theme};

/// OS 主题探测结果（`dark-light::Mode` 的本地投影）。
///
/// ## 为什么不直接用 `dark_light::Mode`
///
/// 两点：① 本模块要能在**无 OS 探测能力**的环境下测（注入探测源），而
/// `dark_light::Mode` 是具体 crate 的类型，把它写进签名会把探测实现绑死在
/// API 上；② `Unspecified` 的处置（回落到昼）是一条**产品决策**，它值得一个
/// 有文档的本地枚举，而不是散在调用点的 `match` 里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsTheme {
    /// 系统偏好亮色
    Light,
    /// 系统偏好暗色
    Dark,
    /// 探测不出（Linux 无等价机制时常见）
    Unspecified,
}

impl OsTheme {
    /// 从 `dark-light` 的探测结果映射。
    #[must_use]
    pub fn from_dark_light(mode: dark_light::Mode) -> Self {
        match mode {
            dark_light::Mode::Dark => Self::Dark,
            dark_light::Mode::Light => Self::Light,
            dark_light::Mode::Unspecified => Self::Unspecified,
        }
    }

    /// 探测本机当前的系统主题。
    ///
    /// 这是**唯一**接触真实 OS 的地方；其余代码经 [`FollowSystem`] 注入的
    /// 探测源走，故全部逻辑可测。
    #[must_use]
    pub fn probe() -> Self {
        Self::from_dark_light(dark_light::detect().unwrap_or(dark_light::Mode::Unspecified))
    }
}

/// 主题来源（`data-model.md` 的 `ThemeChoice` 在本层的形态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeSource {
    /// 昼（Parchment）
    Day,
    /// 夜（Obsidian）
    Night,
    /// 跟随系统
    #[default]
    FollowSystem,
}

/// 可注入的系统主题探测源。
///
/// ## 为什么是"可注入"而不是直接调 `OsTheme::probe()`
///
/// "OS 主题切换时返回对应 token"这条验收要求，若只能靠真的去改系统设置来
/// 触发，就**无法自动化**——测试会退化成"人工点一次看一眼"。把探测源做成
/// 闭包 + 可变的当前值，判据就变成普通的单元断言。
///
/// 真实运行时的构造是 [`FollowSystem::system`]（探测源 = `OsTheme::probe`）。
#[derive(Clone)]
pub struct FollowSystem {
    probe: Arc<dyn Fn() -> OsTheme + Send + Sync>,
    /// 缓存/模拟用的当前值（`with_probe` 时由闭包给出，`set_probed` 可改）
    current: Arc<Mutex<Option<OsTheme>>>,
}

impl std::fmt::Debug for FollowSystem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FollowSystem")
            .field("current", &self.current.lock().map(|guard| *guard))
            .finish_non_exhaustive()
    }
}

impl FollowSystem {
    /// 以给定探测源构造（测试与"自定义探测"用）。
    pub fn with_probe(probe: impl Fn() -> OsTheme + Send + Sync + 'static) -> Self {
        Self {
            probe: Arc::new(probe),
            current: Arc::new(Mutex::new(None)),
        }
    }

    /// 真实系统探测（生产路径）。
    #[must_use]
    pub fn system() -> Self {
        Self::with_probe(OsTheme::probe)
    }

    /// 探测当前系统主题。
    ///
    /// 若 `set_probed` 设过值，则优先返回它——那是"模拟系统主题变化"的入口。
    #[must_use]
    pub fn probe(&self) -> OsTheme {
        if let Ok(guard) = self.current.lock() {
            if let Some(value) = *guard {
                return value;
            }
        }
        (self.probe)()
    }

    /// 覆盖探测结果（模拟用户在系统设置里切换主题）。
    pub fn set_probed(&self, theme: OsTheme) {
        if let Ok(mut guard) = self.current.lock() {
            *guard = Some(theme);
        }
    }
}

/// 文本所处表面的强调层级（决定取哪个文本色 token）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceEmphasis {
    /// 普通正文（`--text-primary`）
    Normal,
    /// 必要信息（标签、按钮文字、进度数值）——**不得**用 tertiary（R7-②）
    Required,
    /// 说明性文字（允许 tertiary：它不承载必要信息）
    Decorative,
}

/// 徽记种类（对应 MASTER §7 的状态药丸四态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeKind {
    /// 中性信息（`--state-info`）
    Info,
    /// 成功（`--state-success`）
    Success,
    /// 警告（`--state-warning` = 鎏金，昼主题对比度低——见 R7-④）
    Warning,
    /// 错误（`--state-error`）
    Error,
}

/// 一个徽记的配色（底 + 前景）。
///
/// ## 为什么底色与前景要成对给出
///
/// R7-④ 的教训正是"底色对、前景错"：原型 `.badge` 的底色取对了 token，
/// 前景写死 `--text-primary`，于是在夜主题下（浅底 + 浅字）崩到 1.63:1。
/// 把两者收进同一个值，使"配错"没有藏身处——取值只能经
/// [`BridgeTheme::badge_style`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeStyle {
    /// 底色
    pub background: egui::Color32,
    /// 前景色
    pub foreground: egui::Color32,
    /// 是否有底色（`false` = 透明底，靠描边）
    background_set: bool,
    /// 是否叠加焦点环
    focus_ring: bool,
}

impl BadgeStyle {
    /// 是否有底色（D2：输入框这类控件必须给底色，不得只靠描边）。
    #[must_use]
    pub fn background_is_set(&self) -> bool {
        self.background_set
    }

    /// 是否叠加焦点环。
    #[must_use]
    pub fn has_focus_ring(&self) -> bool {
        self.focus_ring
    }

    /// 输入框的边界表达（**R7-③ 的落点**）。
    ///
    /// ## 为什么不是"一个描边样式"
    ///
    /// `--border-strong` 在两套主题下最大只有 2.10:1（< WCAG 1.4.11 的 3:1），
    /// 故**不得**作为唯一可见边界。正确的表达是 `--bg-inset` 底 + 焦点环：
    /// 底让控件在静止态就可见，环让聚焦态明确。`preview.html` 的搜索框正是
    /// "bg-inset 底 + 无边框"这一组合。
    #[must_use]
    pub fn input_field() -> Self {
        Self {
            // 具体色值由 BridgeTheme 填（此处只表达"必须给底 + 环"这一形态）
            background: egui::Color32::TRANSPARENT,
            foreground: egui::Color32::TRANSPARENT,
            background_set: true,
            focus_ring: true,
        }
    }
}

/// 主题桥：一个已解析的主题 + egui 侧的翻译。
///
/// ## 为什么携带 `Theme` 而不是只携带 egui 值
///
/// 组件既要 egui 的 `Color32`（画），也要 token 的原始值（如把 `ReadingMetrics`
/// 交给 S2 排版层）。只留 egui 值会迫使组件反查 token；只留 token 则每个
/// 绘制点都要自己换算。两者都持有时，换算集中在 [`Self::color`] 一处。
#[derive(Debug, Clone, Copy)]
pub struct BridgeTheme {
    theme: Theme,
    is_night: bool,
}

impl BridgeTheme {
    /// 解析主题来源 → 一套具体 token。
    ///
    /// ## `FollowSystem` 的三条分支
    ///
    /// - 探测到 `Dark` → 夜
    /// - 探测到 `Light` → 昼
    /// - 探测到 `Unspecified` → **昼**（见 `tests/theme_bridge.rs` 的
    ///   `unspecified_probe_falls_back_to_day`：回落到昼的失败可见性更好）
    ///
    /// 显式选择（`Day`/`Night`）**不**看探测结果——探测失败不该改变用户的
    /// 明确选择。
    #[must_use]
    pub fn resolve(source: ThemeSource, probe: FollowSystem) -> Self {
        match source {
            ThemeSource::Day => Self::day(),
            ThemeSource::Night => Self::night(),
            ThemeSource::FollowSystem => match probe.probe() {
                OsTheme::Dark => Self::night(),
                // `Light` 与 `Unspecified` 都落昼（理由见上）
                OsTheme::Light | OsTheme::Unspecified => Self::day(),
            },
        }
    }

    /// 昼主题。
    #[must_use]
    pub fn day() -> Self {
        Self {
            theme: DayTheme.theme(),
            is_night: false,
        }
    }

    /// 夜主题。
    #[must_use]
    pub fn night() -> Self {
        Self {
            theme: NightTheme.theme(),
            is_night: true,
        }
    }

    /// 底层 token（供排版层取几何/字体栈）。
    #[must_use]
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// 是否夜主题（仅供**呈现**分流；不得用于分页，FR-019）。
    #[must_use]
    pub fn is_night(&self) -> bool {
        self.is_night
    }

    /// token 色 → egui 色（**唯一**的换算点）。
    #[must_use]
    pub fn color(&self, color: Color) -> egui::Color32 {
        egui::Color32::from_rgb(color.red, color.green, color.blue)
    }

    /// 调色板。
    fn palette(&self) -> Palette {
        self.theme.palette()
    }

    /// 某强调层级下的文本色。
    ///
    /// ## R7-①② 在这里落实
    ///
    /// - `Normal` → `--text-primary`
    /// - `Required` → `--text-secondary`（**不是** tertiary：后者两主题均 < 4.5）
    /// - `Decorative` → `--text-tertiary`（它不承载必要信息）
    ///
    /// 鎏金（`--accent-clash`）与警告色**永远不会**从这里返回——它们是填充色。
    #[must_use]
    pub fn text_color_for(&self, emphasis: SurfaceEmphasis) -> egui::Color32 {
        let palette = self.palette();
        match emphasis {
            SurfaceEmphasis::Normal => self.color(palette.text_primary),
            SurfaceEmphasis::Required => self.color(palette.text_secondary),
            SurfaceEmphasis::Decorative => self.color(palette.text_tertiary),
        }
    }

    /// 焦点环颜色（MASTER §2.3 强制：2px `--accent-primary` + 2px offset）。
    #[must_use]
    pub fn focus_ring_color(&self) -> egui::Color32 {
        self.color(self.palette().accent_primary)
    }

    /// 焦点环宽度（px，MASTER 强制 2）。
    #[must_use]
    pub const fn focus_ring_width_px(&self) -> f32 {
        2.0
    }

    /// 焦点环偏移（px，MASTER 强制 2）。
    #[must_use]
    pub const fn focus_ring_offset_px(&self) -> f32 {
        2.0
    }

    /// 徽记配色（**R7-④ 的落点**）。
    ///
    /// ## 前景为什么随主题切换
    ///
    /// 底色的 token 名相同（如 `--state-warning`），但**值不同**：昼是
    /// `#B8862B`（中亮度金），夜是 `#D9AE5C`（亮金）。故前景必须分别取：
    ///
    /// | 主题 | 底色 | 前景 | 对比度 |
    /// |---|---|---|---|
    /// | 昼 | `#B8862B` | `--text-primary`（`#2E2418`） | 4.69 ✅ |
    /// | 夜 | `#D9AE5C` | `--text-inverse`（`#0D0B0B`） | ~9.5 ✅ |
    ///
    /// 原型在两种主题下都用 `--text-primary`，夜主题因此只有 1.63:1——那正是
    /// "不可照抄 preview"的具体内容。
    #[must_use]
    pub fn badge_style(&self, kind: BadgeKind) -> BadgeStyle {
        let palette = self.palette();
        let background = match kind {
            BadgeKind::Info => palette.state_info,
            BadgeKind::Success => palette.state_success,
            BadgeKind::Warning => palette.state_warning,
            BadgeKind::Error => palette.state_error,
        };

        // 前景的取法：昼主题下这几个底色都偏深/中，配深墨字最稳；夜主题下
        // 底色偏亮，配反白字。这与 §2.3 的"主操作按钮反白字"同一思路。
        let foreground = if self.is_night {
            palette.text_inverse
        } else {
            palette.text_primary
        };

        BadgeStyle {
            background: self.color(background),
            foreground: self.color(foreground),
            background_set: true,
            focus_ring: false,
        }
    }

    /// egui 的 `Visuals`（取 core 的底色/文字色，不做自算的近似）。
    #[must_use]
    pub fn visuals(&self) -> egui::Visuals {
        let palette = self.palette();
        let mut visuals = if self.is_night {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        visuals.panel_fill = self.color(palette.bg_base);
        visuals.window_fill = self.color(palette.bg_elevated);
        visuals.extreme_bg_color = self.color(palette.bg_inset);
        visuals.faint_bg_color = self.color(palette.bg_surface);
        visuals.override_text_color = Some(self.color(palette.text_primary));
        visuals.window_stroke = egui::Stroke::new(1.0, self.color(palette.border_subtle));
        visuals.selection.bg_fill = self.color(palette.accent_soft);
        visuals.selection.stroke = egui::Stroke::new(1.0, self.color(palette.accent_primary));
        visuals.hyperlink_color = self.color(palette.accent_primary);
        visuals.warn_fg_color = self.color(palette.state_warning);
        visuals.error_fg_color = self.color(palette.state_error);
        visuals
    }

    /// 字号三元组 → egui 的 `FontId`（字号 + 字重）。
    ///
    /// ## 字重怎么表达
    ///
    /// `egui::FontId` 没有字重维度（R8 点名的落点），故字重经 [`Self::font_family`]
    /// 注册成独立 `FontFamily`。本函数只负责字号，族由调用方或
    /// [`Self::style`] 决定。
    #[must_use]
    pub fn font_id_for_step(&self, step: tome_core::model::theme::TypeStep) -> egui::FontId {
        egui::FontId::new(step.size_px, egui::FontFamily::Proportional)
    }

    /// 字号三元组 → 行高（px）= 字号 × 行高倍数。
    #[must_use]
    pub fn row_height_for_step(&self, step: tome_core::model::theme::TypeStep) -> f32 {
        step.size_px * step.line_height
    }

    /// 圆角映射（**clamp** 到 u8）。
    ///
    /// ## 为什么必须 clamp
    ///
    /// `--r-full = 999` 超 `u8`。直接 `as u8` 会得到 231（999 & 0xFF）——一个
    /// 看起来像圆角但不是药丸的怪值，且**不会报错**。R8 已点名此坑。
    #[must_use]
    pub fn corner_radius(&self, px: u16) -> egui::CornerRadius {
        let clamped = u8::try_from(px).unwrap_or(u8::MAX);
        egui::CornerRadius::same(clamped)
    }

    /// 完整的 egui `Style`（Visuals + 间距 + 圆角 + 字体族）。
    ///
    /// ## 为什么由本函数统一装配
    ///
    /// 若让每个视图各自设 `ui.style_mut()`，设计 token 就会散落各处——那正是
    /// MASTER §10「禁止各控件散读常量」要防的。装配一处，视图只消费。
    #[must_use]
    pub fn style(&self) -> egui::Style {
        let spacing = self.theme.spacing();
        let radii = self.theme.radii();
        let mut visuals = self.visuals();
        // 圆角：窗口用 --r-lg，菜单用 --r-md（`Style::default` 的值与设计系统无关）
        visuals.window_corner_radius = self.corner_radius(radii.lg_px);
        visuals.menu_corner_radius = self.corner_radius(radii.md_px);

        let mut style = egui::Style {
            visuals,
            ..egui::Style::default()
        };

        style.spacing.item_spacing = egui::vec2(f32::from(spacing.sp_2), f32::from(spacing.sp_2));
        style.spacing.button_padding = egui::vec2(f32::from(spacing.sp_3), f32::from(spacing.sp_1));
        style.spacing.interact_size.y = f32::from(self.theme.shell().control_h_px);
        style.spacing.window_margin = egui::Margin::same(spacing.sp_4 as i8);
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.bar_inner_margin = 2.0;
        style.spacing.scroll.bar_outer_margin = 2.0;

        style
    }

    /// 动画时长（egui 的 `style.animation_time` 取 MASTER 的 `--dur-base`）。
    ///
    /// 单独成函数是因为它还牵动"减少动效"的降级（`DurationTicks::effective`）
    /// —— 那属 T033 的走查范围，此处只给未降级的基准值。
    #[must_use]
    pub fn animation_time_seconds(&self) -> f32 {
        self.theme.motion().dur_base_ms as f32 / 1000.0
    }

    /// 把本主题装进 egui 上下文（**唯一的装配点**）。
    ///
    /// ## 为什么要给两套主题都设
    ///
    /// egui 0.36 的样式按 `Theme::{Dark, Light}` 分槽存（`set_style_of`），
    /// 而当前生效哪一套由 `ThemePreference` 决定。若只设一套，用户在系统里
    /// 切昼夜时 egui 会切到**另一套默认样式**——我们的 token 就丢了。
    /// 故两套都装成同一份（我们自己的主题已经区分了昼夜），偏好按当前主题设。
    pub fn apply(&self, ctx: &egui::Context) {
        let style = self.style();
        ctx.set_style_of(egui::Theme::Dark, style.clone());
        ctx.set_style_of(egui::Theme::Light, style);
        ctx.set_theme(if self.is_night {
            egui::ThemePreference::Dark
        } else {
            egui::ThemePreference::Light
        });
    }
}

// ===========================================================================
// 平台 UI 字体链（R7 的 UI 字体裁决）
// ===========================================================================

/// 目标平台（字体链与路径按平台不同）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Windows 10+
    Windows,
    /// macOS 12+
    MacOs,
    /// 现代 Linux
    Linux,
}

impl Platform {
    /// 当前编译目标平台。
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }
}

/// 平台 UI 字体链（**族名**，按优先级；首个为首选）。
///
/// ## 为什么 UI 铬文本走平台链而不内嵌（用户 2026-10-11 批准的决策）
///
/// MASTER §10 建议"打包内嵌字体（衬线正文 + 无衬线 UI + 等宽）确保三平台
/// 一致"，但那要 +1–2MB，而**阅读区不受影响**（正文仍用内嵌 Tome Serif CN）。
/// 铬文本（按钮、标签、表格）的三平台观感差异属可接受范围，桌面惯例也是
/// 用系统 UI 字体（VSCode 等同款）。
///
/// ## 与 MASTER `--font-ui` 栈的关系
///
/// MASTER 的栈是 CSS 写法（跨平台混排一个栈）。桌面实现必须**按平台取子集**
/// ——`"Segoe UI Variable Text"` 在 Linux 上永远命中不了，把它留在链里只会让
/// 日志多一行"找不到"。故本表是 MASTER 栈的**分平台投影**，并在每个平台的
/// 末位保留通用回退。
#[must_use]
pub const fn ui_font_families(platform: Platform) -> &'static [&'static str] {
    match platform {
        // MASTER 栈前两项 + 中文字体（雅黑）+ 通用回退
        Platform::Windows => &[
            "Segoe UI Variable Text",
            "Segoe UI",
            "Microsoft YaHei",
            "sans-serif",
        ],
        // -apple-system 在 macOS 上即系统 UI 字体；PingFang SC 是中文字面
        Platform::MacOs => &[
            "-apple-system",
            "PingFang SC",
            "Helvetica Neue",
            "sans-serif",
        ],
        // 桌面 Linux 的中文首选；Noto Sans 为拉丁字面
        Platform::Linux => &["Noto Sans CJK SC", "Noto Sans", "DejaVu Sans", "sans-serif"],
    }
}

/// 平台 UI 字体的**候选文件路径**（按优先级；首个为首选）。
///
/// ## 为什么给一串候选而不是一条路径
///
/// 同一平台的字体落点因发行版/版本而异（如 Noto CJK 在 Debian 系有
/// `opentype/` 与 `truetype/` 两处，且包名不同）。给一串候选 + "取第一个
/// 存在的"，比硬编码一条路径然后报"字体缺失"更稳——后者在 CI 上会变成
/// 与平台无关的失败。
///
/// 路径里的 `.ttc`（TrueType Collection）是**刻意**的：中文字体常以集合形态
/// 分发（雅黑 `msyh.ttc` 含 Regular/Bold，PingFang 含多字重）。
#[must_use]
pub const fn ui_font_paths(platform: Platform) -> &'static [&'static str] {
    match platform {
        Platform::Windows => &[
            // Windows 10 1809+ 的 Segoe UI Variable；老版本回退到 Segoe UI
            "C:/Windows/Fonts/SegUIVar.ttf",
            "C:/Windows/Fonts/segoeui.ttf",
            // 中文：雅黑（Vista 起自带）
            "C:/Windows/Fonts/msyh.ttc",
            "C:/Windows/Fonts/msyh.ttf",
        ],
        Platform::MacOs => &[
            // macOS 10.11+ 的系统 UI 字体
            "/System/Library/Fonts/SFNS.ttf",
            // 中文：苹方
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Helvetica.ttc",
        ],
        Platform::Linux => &[
            // Debian/Ubuntu 的 fonts-noto-cjk 落点
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            // 部分发行版用 Source Han Sans（Noto 的上游名）
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ],
    }
}

/// 一次 UI 字体注册的结果（**如实报告**装上了什么、缺了什么）。
///
/// ## 为什么要有这个返回值
///
/// 字体缺失的后果是**界面里出现豆腐块**——而那看起来像"文本渲染坏了"，
/// 不像"字体没找到"。有这份报告，启动时就能在日志里看到"Linux 上没找到
/// Noto CJK（已尝试 4 条路径）"，归因路径从"猜"变成"看"。
///
/// 它不是错误（缺失不阻断运行：egui 自带拉丁字面，界面照常可用），故不返回
/// `Result`——与 `DataDir::resolve` 的 `Resolved::Unusable` 同一思路：把
/// "部分可用"如实表达为**值**，而不是伪装成失败或静默。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UiFontRegistration {
    /// 成功加载的路径（按加载顺序）
    pub loaded: Vec<String>,
    /// 尝试过但不存在的路径
    pub missing: Vec<String>,
}

impl UiFontRegistration {
    /// 是否至少装上了一款字体。
    #[must_use]
    pub fn loaded_any(&self) -> bool {
        !self.loaded.is_empty()
    }
}

/// 把平台 UI 字体注册进 egui 的字体定义（生产入口）。
///
/// 返回注册报告（见 [`UiFontRegistration`]）。
pub fn register_ui_fonts(
    fonts: &mut egui::FontDefinitions,
    platform: Platform,
) -> UiFontRegistration {
    register_ui_fonts_from(fonts, ui_font_paths(platform))
}

/// 从给定候选路径注册（**可注入**，故可测）。
///
/// ## 为什么把路径列表做成参数
///
/// 测试要验证"加载成功 / 部分缺失 / 全部缺失"三条分支，而真实路径是否存在
/// 取决于机器。把候选列表参数化后，测试用临时目录构造确定性的输入——判据
/// 不再依赖"CI 上恰好装了 Noto CJK 没有"。
///
/// ## 注册形态
///
/// 全部加载成功的字体**追加**到 `Proportional` 与 `Monospace` 两个族的**末位**
/// （egui 内置字面在前）。这样：拉丁字面用 egui 自带的（度量稳定、不随平台
/// 漂移），CJK 由平台字体兜住——正是"缺字回退"该有的顺序。
pub fn register_ui_fonts_from(
    fonts: &mut egui::FontDefinitions,
    candidates: &[&str],
) -> UiFontRegistration {
    let mut report = UiFontRegistration::default();

    for (index, path) in candidates.iter().enumerate() {
        let Ok(bytes) = std::fs::read(path) else {
            report.missing.push((*path).to_owned());
            continue;
        };

        // 名字带序号：同一平台可能加载多款（如 Segoe UI + 雅黑），
        // 键必须唯一（BTreeMap 的键是字体名）。
        let key = format!("tome-ui-{index}");
        fonts.font_data.insert(
            key.clone(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );

        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(key.clone());
        }

        report.loaded.push((*path).to_owned());
    }

    report
}
