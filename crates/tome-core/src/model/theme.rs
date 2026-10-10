//! \[DONE] `Theme` —— 设计 token 的纯值类型（T002；裁决 OQ#11→D，FR-029/030/031）
//!
//! `design-system/MASTER.md` 是用户拍板的**唯一**设计规范来源；本模块是它在
//! 代码里的投影。**每个字段都对应 MASTER 的一个 token**，一一对应关系写在
//! 字段文档里，并由 `tests/theme_contract.rs` 逐条断言。
//!
//! ## 为什么放在 `tome-core`（而不是 S2 或 S3）
//!
//! 裁决 OQ#11→D：`Theme` 是**纯值类型**，而 core 是 DTO 之家——纯值类型不构成
//! 对 001 接缝边界的违反。放这里让 `tome-reader` 与 `tome-app` 都能经 core 取
//! 到它，二者**不必互相依赖**（组合根构造昼/夜实例并注入）。
//!
//! ## 零图形依赖（宪法 IV / FR-029）
//!
//! 本模块只有 `std` 的整数/浮点/字符串，**没有任何图形类型**——颜色是自己的
//! [`Color`]/[`Rgba`]，不是 `wgpu::Color`；字体是字符串栈，不是 `fontdb::Source`。
//! 绘制层（S2 渲染 / S3 外壳）负责把本模块的值翻译成各自后端的类型。
//! 这不是洁癖：core 被全体依赖，若它拉进 wgpu，图形栈就进了每一个 crate 的
//! 编译链路，headless 构建随之名存实亡。
//!
//! ## 与 MASTER 的一致性纪律
//!
//! MASTER 开头写明：`tokens.css` 必须忠实投影本文件，本文件不得出现
//! `tokens.css` 未定义的 token。同理，本模块不得出现 MASTER 未定义的 token。
//! 新增 token 时三处（MASTER / tokens.css / 本模块）同改，否则视为漂移。
//!
//! ## 单元一览
//!
//! | 类型 | 对应 MASTER 节 |
//! |---|---|
//! | [`Palette`] | §2.1 / §2.2 语义色（各 20 项） |
//! | [`Fonts`] / [`FontRole`] | §3 字体三角色 |
//! | [`TypeScale`] / [`TypeStep`] | §3 字号层级八档 |
//! | [`SpacingScale`] / [`Radii`] / [`ShellDims`] | §4 间距/圆角/骨架尺寸 |
//! | [`Shadows`] / [`Shadow`] / [`Texture`] / [`Swatches`] | §4 阴影/质感/色卡 |
//! | [`Motion`] / [`DurationTicks`] | §4 动效 |
//! | [`ReadingMetrics`] | §5.1 阅读层参数 + §5.2 双页判据 |
//! | [`ReadingToolbar`] | §5.3 工具栏几何与入口清单 |
//! | [`Theme`] | 组合门面（含 [`DayTheme`] / [`NightTheme`] 两个实例） |

use std::fmt;

// ===========================================================================
// §2 色彩
// ===========================================================================

/// 不透明语义色（MASTER §2.1 / §2.2 的十六进制色值）。
///
/// 语义色**不带 alpha**：MASTER 的两张色表全是 6 位十六进制，透明度只出现在
/// 阴影与遮罩（见 [`Rgba`]）。分开两个类型是为了让"给语义色加透明度"这件事
/// 必须显式转换（[`Color::to_rgba`]），而不是顺手写个 `.5`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// 红分量
    pub red: u8,
    /// 绿分量
    pub green: u8,
    /// 蓝分量
    pub blue: u8,
}

impl Color {
    /// 以 sRGB 三分量构造。
    #[must_use]
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    /// 转 CSS 十六进制串（`#RRGGBB`，大写）。
    ///
    /// 大写是为与 MASTER 的色表逐字符可比——测试直接拿它跟 token 表对。
    #[must_use]
    pub fn to_hex_string(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.red, self.green, self.blue)
    }

    /// 提升为带 alpha 的 RGBA（`alpha = 1.0`）。
    #[must_use]
    pub fn to_rgba(self) -> Rgba {
        Rgba {
            red: self.red,
            green: self.green,
            blue: self.blue,
            alpha: 1.0,
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex_string())
    }
}

/// 带 alpha 的颜色（MASTER §4 的阴影与遮罩底）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    /// 红分量
    pub red: u8,
    /// 绿分量
    pub green: u8,
    /// 蓝分量
    pub blue: u8,
    /// 不透明度（0.0–1.0）
    pub alpha: f32,
}

impl Rgba {
    /// 以 sRGB 三分量与不透明度构造。
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8, alpha: f32) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }
}

/// §2.1 / §2.2 的 20 个语义色。
///
/// **两套主题共用本类型**：MASTER 的一致性纪律要求昼夜两表逐 token 对应，
/// 用同一份类型承载两套值即把这条约束交给编译器——只给一套加字段会
/// 立刻编译失败，而不是等到界面上"某主题漏画一块"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Palette {
    /// `--bg-base` 应用最底层
    pub bg_base: Color,
    /// `--bg-surface` 卡片 / 面板 / 侧栏
    pub bg_surface: Color,
    /// `--bg-elevated` 悬浮层 / 弹窗 / 下拉
    pub bg_elevated: Color,
    /// `--bg-reading` **正文阅读区**（最纯净层；FR-030 强制语义）
    pub bg_reading: Color,
    /// `--bg-inset` 内嵌槽 / 输入框底 / 表格斑马纹
    pub bg_inset: Color,
    /// `--border-subtle` 分割线 / 弱描边
    pub border_subtle: Color,
    /// `--border-strong` 强调描边 / 输入框边框
    pub border_strong: Color,
    /// `--text-primary` 正文墨色（FR-030：正文色不得低于 `text_secondary`）
    pub text_primary: Color,
    /// `--text-secondary` 次要文本 / 标签（正文色的下限）
    pub text_secondary: Color,
    /// `--text-tertiary` 说明 / 元信息
    pub text_tertiary: Color,
    /// `--text-inverse` 深底上的反白字
    pub text_inverse: Color,
    /// `--accent-primary` 主操作 / 当前项 / 主色（focus 环也用它）
    pub accent_primary: Color,
    /// `--accent-hover` 主色 hover
    pub accent_hover: Color,
    /// `--accent-soft` 主色浅底
    pub accent_soft: Color,
    /// `--accent-secondary` 副色（成功 / 已读 / 次要状态）
    pub accent_secondary: Color,
    /// `--accent-clash` 撞色点缀（仅徽记 / 星级 / 进度条高光）
    pub accent_clash: Color,
    /// `--state-success` 成功
    pub state_success: Color,
    /// `--state-warning` 警告
    pub state_warning: Color,
    /// `--state-error` 错误 / 危险
    pub state_error: Color,
    /// `--state-info` 中性信息（**暖中性灰**，见 §2.1 的裁决说明）
    pub state_info: Color,
}

/// 昼 · Parchment（羊皮卷轴）——MASTER §2.1 逐项对应。
pub const PARCHMENT: Palette = Palette {
    bg_base: Color::rgb(0xE8, 0xDF, 0xC8),
    bg_surface: Color::rgb(0xF2, 0xEA, 0xD6),
    bg_elevated: Color::rgb(0xFA, 0xF4, 0xE4),
    bg_reading: Color::rgb(0xF5, 0xEE, 0xDC),
    bg_inset: Color::rgb(0xDF, 0xD4, 0xB8),
    border_subtle: Color::rgb(0xD6, 0xC9, 0xA8),
    border_strong: Color::rgb(0xBC, 0xA9, 0x7F),
    text_primary: Color::rgb(0x2E, 0x24, 0x18),
    text_secondary: Color::rgb(0x5C, 0x4B, 0x33),
    text_tertiary: Color::rgb(0x8A, 0x76, 0x58),
    text_inverse: Color::rgb(0xF5, 0xEE, 0xDC),
    accent_primary: Color::rgb(0x8C, 0x3A, 0x1E),
    accent_hover: Color::rgb(0xA3, 0x45, 0x24),
    accent_soft: Color::rgb(0xE4, 0xD3, 0xB8),
    accent_secondary: Color::rgb(0x3F, 0x5C, 0x4A),
    accent_clash: Color::rgb(0xB8, 0x86, 0x2B),
    state_success: Color::rgb(0x3F, 0x5C, 0x4A),
    state_warning: Color::rgb(0xB8, 0x86, 0x2B),
    state_error: Color::rgb(0xA3, 0x2B, 0x1F),
    state_info: Color::rgb(0x66, 0x5A, 0x4F),
};

/// 夜 · Obsidian（纯黑基底）——MASTER §2.2 逐项对应。
pub const OBSIDIAN: Palette = Palette {
    bg_base: Color::rgb(0x00, 0x00, 0x00),
    bg_surface: Color::rgb(0x0D, 0x0B, 0x0B),
    bg_elevated: Color::rgb(0x17, 0x14, 0x14),
    bg_reading: Color::rgb(0x00, 0x00, 0x00),
    bg_inset: Color::rgb(0x13, 0x10, 0x10),
    border_subtle: Color::rgb(0x23, 0x1E, 0x1E),
    border_strong: Color::rgb(0x3A, 0x32, 0x32),
    text_primary: Color::rgb(0xE6, 0xE4, 0xDE),
    text_secondary: Color::rgb(0x9A, 0x97, 0x90),
    text_tertiary: Color::rgb(0x6A, 0x67, 0x61),
    text_inverse: Color::rgb(0x0D, 0x0B, 0x0B),
    accent_primary: Color::rgb(0xD9, 0x79, 0x4A),
    accent_hover: Color::rgb(0xE8, 0x8F, 0x5F),
    accent_soft: Color::rgb(0x2A, 0x1A, 0x12),
    accent_secondary: Color::rgb(0x7F, 0xA8, 0x8C),
    accent_clash: Color::rgb(0xD9, 0xAE, 0x5C),
    state_success: Color::rgb(0x7F, 0xA8, 0x8C),
    state_warning: Color::rgb(0xD9, 0xAE, 0x5C),
    state_error: Color::rgb(0xE0, 0x6B, 0x5C),
    state_info: Color::rgb(0xB8, 0xAF, 0xA0),
};

// ===========================================================================
// §3 字体
// ===========================================================================

/// 字重（MASTER §3 只允许 400/500/600/650/700）。
///
/// 用枚举而非裸 `u16`：§3 明文「字重仅用 400/500/600/650/700，**禁止 800+**」，
/// 而枚举把这条约束交给编译器——写不出 800。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    /// 400 —— 界面正文
    Regular,
    /// 500 —— 徽记 / 计数
    Medium,
    /// 600 —— 区块标题 / 卡片标题
    SemiBold,
    /// 650 —— 页面标题
    Strong,
    /// 700 —— 空状态标题（上限，禁 800+）
    Bold,
}

impl Weight {
    /// CSS `font-weight` 数值。
    #[must_use]
    pub const fn value(self) -> u16 {
        match self {
            Self::Regular => 400,
            Self::Medium => 500,
            Self::SemiBold => 600,
            Self::Strong => 650,
            Self::Bold => 700,
        }
    }
}

/// 字体角色（MASTER §3 的三个角色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontRole {
    /// 正文衬线（内嵌子集 + 系统回退链）
    Reading,
    /// UI 无衬线（走系统栈）
    Ui,
    /// 书源规则 / 正则等宽（走系统栈）
    Mono,
}

impl FontRole {
    /// 三个角色的枚举视图（供遍历与完整性断言）。
    pub const ALL: [Self; 3] = [Self::Reading, Self::Ui, Self::Mono];
}

/// 一个字体角色的候选族栈（首个为首选）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontStack {
    /// 按优先级排列的字体族名，末项收于通用族（`serif` / `sans-serif` / `monospace`）
    pub stack: &'static [&'static str],
    /// 是否内嵌（决定渲染层是否需要先注册资产再查找）
    pub embedded: bool,
}

/// 字体三角色（MASTER §3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fonts {
    /// `--font-reading` 正文衬线（**内嵌**首选，见 FR-050）
    pub reading: FontStack,
    /// `--font-ui` UI 无衬线（**不内嵌**）
    pub ui: FontStack,
    /// `--font-mono` 规则等宽（**不内嵌**）
    pub mono: FontStack,
}

impl Fonts {
    /// 取某角色的字体栈。
    #[must_use]
    pub const fn for_role(&self, role: FontRole) -> FontStack {
        match role {
            FontRole::Reading => self.reading,
            FontRole::Ui => self.ui,
            FontRole::Mono => self.mono,
        }
    }
}

/// MASTER §3 的字体三角色。
///
/// ## 正文衬线的两处**有意偏离**字面 token 表
///
/// 1. **`Tome Serif CN` 排首位**：MASTER §3 的 `--font-reading` 栈首项是
///    `"Iowan Old Style"`（macOS 系统字体）。但裁决 OQ#3→A 与 FR-050 要求
///    **内嵌子集为正文首选**——三平台观感一致是其唯一达成手段，若让系统字体
///    抢先命中，同一本书在三个平台就是三种字形。故内嵌子集排首，MASTER 原栈
///    整体后移一位作为回退链（缺字时按序回退，FR-047）。
/// 2. **`Source Han Serif SC` 保留**：它是思源宋体的系统名。虽然内嵌子集
///    已按 OFL RFN 改名去 "Source"（R3），但**系统字体**若装了思源宋体，
///    作为回退目标是合适的——这里引用的是系统字体名，不是我们的子集名。
pub const FONTS: Fonts = Fonts {
    reading: FontStack {
        stack: &[
            "Tome Serif CN",
            "Iowan Old Style",
            "Source Han Serif SC",
            "Songti SC",
            "Noto Serif SC",
            "Georgia",
            "SimSun",
            "serif",
        ],
        embedded: true,
    },
    ui: FontStack {
        stack: &[
            "Segoe UI Variable Text",
            "Segoe UI",
            "-apple-system",
            "PingFang SC",
            "Microsoft YaHei",
            "system-ui",
            "sans-serif",
        ],
        embedded: false,
    },
    mono: FontStack {
        stack: &[
            "Cascadia Code",
            "JetBrains Mono",
            "SF Mono",
            "Consolas",
            "monospace",
        ],
        embedded: false,
    },
};

/// §3 字号层级的一档（字号 / 行高倍数 / 字重）。
///
/// 行高是**倍数**而非绝对值——MASTER 表里写的是 `30px / 1.15 / 700` 这样的
/// 三元组，其中 1.15 是行高倍数。存倍数让字号缩放时行距自动跟随。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeStep {
    /// 字号（px）
    pub size_px: f32,
    /// 行高（字号倍数）
    pub line_height: f32,
    /// 字重（CSS 数值）
    pub font_weight: u16,
}

/// §3 字号层级八档（**UI 层**；阅读层字号独立于它，见 [`ReadingMetrics`]）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeScale {
    /// `--fs-display` 空状态标题 / 页面主标题
    pub display: TypeStep,
    /// `--fs-h1` 页面标题
    pub h1: TypeStep,
    /// `--fs-h2` 区块标题
    pub h2: TypeStep,
    /// `--fs-h3` 卡片标题
    pub h3: TypeStep,
    /// `--fs-body` 界面正文
    pub body: TypeStep,
    /// `--fs-sm` 次要正文 / 表格
    pub sm: TypeStep,
    /// `--fs-caption` 说明 / 标签
    pub caption: TypeStep,
    /// `--fs-micro` 徽记 / 计数
    pub micro: TypeStep,
}

impl TypeScale {
    /// 八档的数组视图（大档在前）。
    #[must_use]
    pub const fn steps(&self) -> [TypeStep; 8] {
        [
            self.display,
            self.h1,
            self.h2,
            self.h3,
            self.body,
            self.sm,
            self.caption,
            self.micro,
        ]
    }
}

/// MASTER §3 字号层级。
pub const TYPE_SCALE: TypeScale = TypeScale {
    display: TypeStep {
        size_px: 30.0,
        line_height: 1.15,
        font_weight: 700,
    },
    h1: TypeStep {
        size_px: 22.0,
        line_height: 1.30,
        font_weight: 650,
    },
    h2: TypeStep {
        size_px: 17.0,
        line_height: 1.40,
        font_weight: 600,
    },
    h3: TypeStep {
        size_px: 15.0,
        line_height: 1.45,
        font_weight: 600,
    },
    body: TypeStep {
        size_px: 14.0,
        line_height: 1.60,
        font_weight: 400,
    },
    sm: TypeStep {
        size_px: 13.0,
        line_height: 1.55,
        font_weight: 400,
    },
    caption: TypeStep {
        size_px: 12.0,
        line_height: 1.50,
        font_weight: 400,
    },
    micro: TypeStep {
        size_px: 11.0,
        line_height: 1.40,
        font_weight: 500,
    },
};

// ===========================================================================
// §4 间距 / 圆角 / 骨架尺寸 / 阴影 / 质感 / 色卡 / 动效
// ===========================================================================

/// §4 间距（4px 基础网格九档）。
///
/// 命名访问器 + [`Self::scale`] 数组视图成对存在：前者让调用点可读
/// （`spacing.sp_4` 比 `spacing.scale()[3]` 明确），后者让测试能整体遍历。
/// 两者的一致性由 `tests/theme_contract.rs` 断言，防两处定义漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpacingScale {
    /// `--sp-1` 4px
    pub sp_1: u16,
    /// `--sp-2` 8px
    pub sp_2: u16,
    /// `--sp-3` 12px
    pub sp_3: u16,
    /// `--sp-4` 16px
    pub sp_4: u16,
    /// `--sp-5` 20px
    pub sp_5: u16,
    /// `--sp-6` 24px
    pub sp_6: u16,
    /// `--sp-8` 32px
    pub sp_8: u16,
    /// `--sp-10` 40px
    pub sp_10: u16,
    /// `--sp-12` 48px
    pub sp_12: u16,
}

impl SpacingScale {
    /// 九档的数组视图（小档在前）。
    #[must_use]
    pub const fn scale(&self) -> [u16; 9] {
        [
            self.sp_1, self.sp_2, self.sp_3, self.sp_4, self.sp_5, self.sp_6, self.sp_8,
            self.sp_10, self.sp_12,
        ]
    }
}

/// MASTER §4 间距。
pub const SPACING: SpacingScale = SpacingScale {
    sp_1: 4,
    sp_2: 8,
    sp_3: 12,
    sp_4: 16,
    sp_5: 20,
    sp_6: 24,
    sp_8: 32,
    sp_10: 40,
    sp_12: 48,
};

/// §4 圆角五档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Radii {
    /// `--r-xs` 徽记 / 小标签
    pub xs_px: u16,
    /// `--r-sm` 按钮 / 输入框
    pub sm_px: u16,
    /// `--r-md` 卡片 / 面板
    pub md_px: u16,
    /// `--r-lg` 大容器 / 弹窗
    pub lg_px: u16,
    /// `--r-full` 药丸 / 头像
    pub full_px: u16,
}

/// MASTER §4 圆角。
pub const RADII: Radii = Radii {
    xs_px: 4,
    sm_px: 6,
    md_px: 8,
    lg_px: 12,
    full_px: 999,
};

/// §4 布局骨架尺寸（§7 AppShell 用到）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellDims {
    /// `--topbar-h` TopBar 高度
    pub topbar_h_px: u16,
    /// `--sidebar-w` 侧栏展开宽
    pub sidebar_w_px: u16,
    /// `--sidebar-w-collapsed` 侧栏折叠为图标栏的宽度
    pub sidebar_w_collapsed_px: u16,
    /// `--control-h` 统一控件高（按钮 / 输入框 / 分段控件）
    pub control_h_px: u16,
}

/// MASTER §4 骨架尺寸。
pub const SHELL: ShellDims = ShellDims {
    topbar_h_px: 56,
    sidebar_w_px: 240,
    sidebar_w_collapsed_px: 64,
    control_h_px: 32,
};

/// §4 阴影（`x / y / blur / spread / color`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    /// 水平偏移（px）
    pub offset_x_px: i32,
    /// 垂直偏移（px）
    pub offset_y_px: i32,
    /// 模糊半径（px）
    pub blur_px: i32,
    /// 扩展半径（px，常为负——MASTER 的三档都是负值）
    pub spread_px: i32,
    /// 投影色（带 alpha）
    pub color: Rgba,
}

/// §4 阴影三档（克制，仅悬浮层）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadows {
    /// `--shadow-sm`
    pub sm: Shadow,
    /// `--shadow-md`
    pub md: Shadow,
    /// `--shadow-lg`
    pub lg: Shadow,
}

/// MASTER §4 昼主题阴影（暖褐投影）。
pub const SHADOWS_DAY: Shadows = Shadows {
    sm: Shadow {
        offset_x_px: 0,
        offset_y_px: 1,
        blur_px: 2,
        spread_px: 0,
        color: Rgba::new(70, 52, 28, 0.10),
    },
    md: Shadow {
        offset_x_px: 0,
        offset_y_px: 6,
        blur_px: 16,
        spread_px: -4,
        color: Rgba::new(70, 52, 28, 0.16),
    },
    lg: Shadow {
        offset_x_px: 0,
        offset_y_px: 16,
        blur_px: 40,
        spread_px: -12,
        color: Rgba::new(70, 52, 28, 0.22),
    },
};

/// MASTER §4 夜主题阴影（纯黑投影；暗场靠描边而非阴影分层）。
pub const SHADOWS_NIGHT: Shadows = Shadows {
    sm: Shadow {
        offset_x_px: 0,
        offset_y_px: 1,
        blur_px: 2,
        spread_px: 0,
        color: Rgba::new(0, 0, 0, 0.60),
    },
    md: Shadow {
        offset_x_px: 0,
        offset_y_px: 6,
        blur_px: 16,
        spread_px: -4,
        color: Rgba::new(0, 0, 0, 0.75),
    },
    lg: Shadow {
        offset_x_px: 0,
        offset_y_px: 16,
        blur_px: 40,
        spread_px: -12,
        color: Rgba::new(0, 0, 0, 0.85),
    },
};

/// §4 质感与遮罩（纸噪点 + 遮罩底）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Texture {
    /// `--paper-noise` 纸纤维噪点的 SVG data-URI（**空串表示 `none`**）
    pub paper_noise_svg_data_uri: &'static str,
    /// `--paper-noise-opacity` 噪点强度（夜主题为 0）
    pub paper_noise_opacity: f32,
    /// `--overlay-bg` 抽屉 / 弹窗的遮罩底
    pub overlay_bg: Rgba,
}

/// 昼主题的纸纤维噪点 SVG。
///
/// `fractalNoise` + `baseFrequency=.8` 是 MASTER §4 指定的形态；色相取
/// 纸面暖褐（`#46341C` = 阴影投影色的实色版），配合 `mix-blend-mode: multiply`
/// 与 0.045 的强度后只剩"触感"。
pub const PARCHMENT_NOISE_SVG: &str = "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='.8' numOctaves='3'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)' opacity='.5'/%3E%3C/svg%3E";

/// MASTER §4 昼主题质感。
pub const TEXTURE_DAY: Texture = Texture {
    paper_noise_svg_data_uri: PARCHMENT_NOISE_SVG,
    paper_noise_opacity: 0.045,
    overlay_bg: Rgba::new(46, 36, 24, 0.28),
};

/// MASTER §4 夜主题质感（纸噪点为 `none`，昼主题专属）。
pub const TEXTURE_NIGHT: Texture = Texture {
    paper_noise_svg_data_uri: "",
    paper_noise_opacity: 0.0,
    overlay_bg: Rgba::new(0, 0, 0, 0.66),
};

/// §4 主题预览色卡（设置页「外观主题」用）。
///
/// ## 为什么单独成组且**恒定**
///
/// MASTER §4 明文：「两套主题的预览色卡是**恒定值**（不随当前主题变化），
/// 故单独成组。页面必须引用变量而非内联十六进制——否则色卡会与实际调色板
/// 脱钩，成为「说谎的预览」。」故本类型**不挂在 [`Theme`] 上**，而是独立常量
/// [`SWATCHES`]：放在 `Theme` 里就会随主题切换而变，那正是 MASTER 所禁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Swatches {
    /// `--swatch-day-bg` 羊皮卷轴色卡底
    pub day_bg: Color,
    /// `--swatch-day-a` 昼主题第一色点（主色）
    pub day_a: Color,
    /// `--swatch-day-b` 昼主题第二色点（副色）
    pub day_b: Color,
    /// `--swatch-day-c` 昼主题第三色点（撞色）
    pub day_c: Color,
    /// `--swatch-night-bg` 纯黑基底色卡底
    pub night_bg: Color,
    /// `--swatch-night-a` 夜主题第一色点
    pub night_a: Color,
    /// `--swatch-night-b` 夜主题第二色点
    pub night_b: Color,
    /// `--swatch-night-c` 夜主题第三色点
    pub night_c: Color,
}

/// MASTER §4 预览色卡（**恒定**，两套主题共用同一份）。
pub const SWATCHES: Swatches = Swatches {
    day_bg: Color::rgb(0xF5, 0xEE, 0xDC),
    day_a: Color::rgb(0x8C, 0x3A, 0x1E),
    day_b: Color::rgb(0x3F, 0x5C, 0x4A),
    day_c: Color::rgb(0xB8, 0x86, 0x2B),
    night_bg: Color::rgb(0x00, 0x00, 0x00),
    night_a: Color::rgb(0xD9, 0x79, 0x4A),
    night_b: Color::rgb(0x7F, 0xA8, 0x8C),
    night_c: Color::rgb(0xD9, 0xAE, 0x5C),
};

/// 一段时长（毫秒），供动效档位与「减少动效」降级共同表达。
///
/// ## 为什么要有 [`Self::INSTANT`] 而不直接写 `0`
///
/// FR-032：减少动效开启时时长降为 **0**（立即完成）。CSS 里另有
/// `0.01ms` 的写法，那是为了绕开"字面 0 仍触发 `transitionend`"的浏览器行为，
/// 语义与"立即完成"不同。本类型提供**唯一**的字面 0 取值点，避免各处自己
/// 挑一个近似值——三平台一致性（FR-055）要求这类取值只有一处定义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationTicks(u32);

impl DurationTicks {
    /// 减少动效时的降级值：**字面 0ms**（立即完成）。
    pub const INSTANT: Self = Self(0);

    /// 以毫秒构造。
    #[must_use]
    pub const fn from_ms(ms: u32) -> Self {
        Self(ms)
    }

    /// 取毫秒值。
    #[must_use]
    pub const fn ms(self) -> u32 {
        self.0
    }

    /// 减少动效时的实际取值：开启则 [`Self::INSTANT`]，否则原值。
    ///
    /// 把"降级规则"放在类型上而非各绘制点：FR-032 要求降级**不得**改变分页
    /// 结果与阅读位置，唯一安全的实现方式是**所有**动效取值都经由此处。
    #[must_use]
    pub const fn effective(self, reduced_motion: bool) -> Self {
        if reduced_motion {
            Self::INSTANT
        } else {
            self
        }
    }
}

/// §4 动效（两档缓动 + 三档时长）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// `--ease-out` 通用缓动（cubic-bezier 四控制点）
    pub ease_out: [f32; 4],
    /// `--ease-spring` 弹入 / 翻页
    pub ease_spring: [f32; 4],
    /// `--dur-fast` hover / 状态
    pub dur_fast_ms: u32,
    /// `--dur-base` 展开 / 面板
    pub dur_base_ms: u32,
    /// `--dur-slow` 页面切换 / 翻页
    pub dur_slow_ms: u32,
}

impl Motion {
    /// 三档时长（快 → 慢）。
    #[must_use]
    pub const fn durations(&self) -> [DurationTicks; 3] {
        [
            DurationTicks::from_ms(self.dur_fast_ms),
            DurationTicks::from_ms(self.dur_base_ms),
            DurationTicks::from_ms(self.dur_slow_ms),
        ]
    }
}

/// MASTER §4 动效。
pub const MOTION: Motion = Motion {
    ease_out: [0.22, 0.61, 0.36, 1.0],
    ease_spring: [0.34, 1.30, 0.64, 1.0],
    dur_fast_ms: 150,
    dur_base_ms: 200,
    dur_slow_ms: 280,
};

// ===========================================================================
// §5 阅读层
// ===========================================================================

/// §5.1 视口与版心 + §5.2 单双页判据的输入。
///
/// ## 为什么几何参数放在 `Theme` 里（而不只是 `ReadingConfig`）
///
/// 两者关切不同：`Theme` 承载**已拍板的设计默认值**（§5.1 的表格），
/// S2 的 `ReadingConfig`（T017）承载**用户可调值与序列化**。`Theme` 提供
/// 默认值与合法范围，`ReadingConfig` 在其上做白名单校验——`Theme` 是常量源，
/// `ReadingConfig` 是可变实例。这样"参数范围"与"设计规范"只有一处定义。
///
/// 本类型是 `Copy` 的纯值：改字号用 [`Self::with_font_size`] 派生新值，
/// 不做原地可变——几何参数会经 `ReadingConfig` 持久化，可变更容易让
/// 「呈现参数不影响分页」（FR-019）的边界失守。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingMetrics {
    /// `--reading-measure` 单栏理想行长（em）
    pub measure_em: f32,
    /// `--reading-measure-max` 单栏绝对上限（em；超出则必须分栏或封顶留白）
    pub measure_max_em: f32,
    /// `--reading-gutter` 双页中缝宽（px）
    pub gutter_px: f32,
    /// `--reading-font-size` 阅读字号默认值（px）
    pub font_size_px: f32,
    /// 阅读字号可调下限（px）
    pub font_size_min_px: f32,
    /// 阅读字号可调上限（px）
    pub font_size_max_px: f32,
    /// `--reading-line-height` 行高默认值（字号倍数）
    pub line_height: f32,
    /// 行高可调下限（**无障碍强制 ≥1.75**，裁决 OQ#19→A）
    pub line_height_min: f32,
    /// 行高可调上限
    pub line_height_max: f32,
    /// `--reading-paragraph-indent` 段首缩进（em；默认模式）
    pub paragraph_indent_em: f32,
    /// `--reading-paragraph-gap` 段间距（em；与段缩进二选一）
    pub paragraph_gap_em: f32,
}

impl ReadingMetrics {
    /// §5.2：单栏理想版心宽度（px）= `measure_em × font_size`。
    #[must_use]
    pub fn measure_px(&self) -> f32 {
        self.measure_em * self.font_size_px
    }

    /// §5.2 的单栏绝对上限宽度（px）。
    #[must_use]
    pub fn measure_max_px(&self) -> f32 {
        self.measure_max_em * self.font_size_px
    }

    /// §5.2 的双页启用阈值（px）= `2 × 单栏理想版心 + 中缝`。
    ///
    /// 判据的输入是**阅读区可用宽度**（扣除侧栏与内边距后的宽度），
    /// **不是窗口总宽**（裁决 OQ#22→A）。调用方负责算好 `available` 再比。
    ///
    /// 注意这是 `em` 制派生量：字号一改，阈值同步右移（FR-014 要求
    /// MUST NOT 使用固定像素断点）。19px 基准下为 `2×684+64 = 1432px`。
    #[must_use]
    pub fn two_page_threshold_px(&self) -> f32 {
        2.0f32.mul_add(self.measure_px(), self.gutter_px)
    }

    /// §5.2：给定阅读区可用宽度，是否启用双页。
    #[must_use]
    pub fn enables_two_page(&self, available_width_px: f32) -> bool {
        available_width_px >= self.two_page_threshold_px()
    }

    /// 派生一个改了阅读字号的新值（其余保持）。
    ///
    /// 缩放字号的**唯一**入口：em 制判据依赖它同步重算（FR-014）。
    #[must_use]
    pub const fn with_font_size(self, font_size_px: f32) -> Self {
        Self {
            font_size_px,
            ..self
        }
    }

    /// 默认字号是否落在可调区间内（§5.1 的「19px（可调 14–30）」自洽性）。
    #[must_use]
    pub fn font_size_default_in_range(&self) -> bool {
        self.font_size_px >= self.font_size_min_px && self.font_size_px <= self.font_size_max_px
    }

    /// 默认行高是否落在可调区间内（§5.1 的「1.85（可调 1.75–2.2）」自洽性）。
    #[must_use]
    pub fn line_height_default_in_range(&self) -> bool {
        self.line_height >= self.line_height_min && self.line_height <= self.line_height_max
    }
}

/// MASTER §5.1 阅读层参数。
pub const READING: ReadingMetrics = ReadingMetrics {
    measure_em: 36.0,
    measure_max_em: 42.0,
    gutter_px: 64.0,
    font_size_px: 19.0,
    font_size_min_px: 14.0,
    font_size_max_px: 30.0,
    line_height: 1.85,
    line_height_min: 1.75,
    line_height_max: 2.2,
    paragraph_indent_em: 2.0,
    paragraph_gap_em: 0.4,
};

/// §5.3 阅读区工具栏的状态机几何与入口清单。
///
/// **职责分界**（裁决 OQ#8→A / FR-034）：本层持有"隐形 / 唤出 / 热区 / 入口
/// 清单"的**状态与几何**，绘制由 S3 外壳承担。此处只放**几何常量**；
/// 状态机本体属 S2-5 的 T046。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingToolbar {
    /// 顶部热区高度（px）——**整窗顶部全域**，不随单双页版心变化
    pub hot_zone_height_px: u16,
    /// 鼠标静止多久后淡出（ms）
    pub idle_fade_after_ms: u32,
    /// 淡出时长（ms）——取 `--dur-base`
    pub fade_duration_ms: u32,
    /// 入口清单（稳定标识，非文案）
    pub entries: [&'static str; 10],
}

/// MASTER §5.3 工具栏几何与入口。
pub const READING_TOOLBAR: ReadingToolbar = ReadingToolbar {
    hot_zone_height_px: 8,
    idle_fade_after_ms: 2500,
    fade_duration_ms: MOTION.dur_base_ms,
    entries: [
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
    ],
};

// ===========================================================================
// Theme 组合门面
// ===========================================================================

/// 一套完整的主题值（昼夜各一实例）。
///
/// ## 为什么 `Copy`
///
/// 纯值类型（FR-029）应当是 `Copy` 的：绘制点随手取一份即可，无需共享所有权、
/// 无需生命周期参数、不会因"借用冲突"逼出 `Rc<Theme>` 之类的间接层。
/// 若某个字段是 `Vec`/`String` 而需要堆分配，`Copy` 就不再成立——那正是
/// 「不得出现图形类型」之外的又一条设计边界（见 [`FontStack`] 用
/// `&'static [&'static str]` 而非 `Vec<String>`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    name: &'static str,
    palette: Palette,
    fonts: Fonts,
    type_scale: TypeScale,
    spacing: SpacingScale,
    radii: Radii,
    shell: ShellDims,
    shadows: Shadows,
    texture: Texture,
    motion: Motion,
    reading: ReadingMetrics,
    reading_toolbar: ReadingToolbar,
}

impl Theme {
    /// 主题稳定标识（`parchment` / `obsidian`）——**不是**面向用户的文案。
    ///
    /// 与错误标识同理（FR-058）：引擎内不做本地化，显示名由上层按本标识查表。
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }

    /// §2.1 / §2.2 语义色。
    #[must_use]
    pub const fn palette(&self) -> Palette {
        self.palette
    }

    /// §3 字体三角色。
    #[must_use]
    pub const fn fonts(&self) -> Fonts {
        self.fonts
    }

    /// §3 字号层级（UI 层；阅读层字号见 [`Self::reading`]）。
    #[must_use]
    pub const fn type_scale(&self) -> TypeScale {
        self.type_scale
    }

    /// §4 间距。
    #[must_use]
    pub const fn spacing(&self) -> SpacingScale {
        self.spacing
    }

    /// §4 圆角。
    #[must_use]
    pub const fn radii(&self) -> Radii {
        self.radii
    }

    /// §4 布局骨架尺寸。
    #[must_use]
    pub const fn shell(&self) -> ShellDims {
        self.shell
    }

    /// §4 阴影。
    #[must_use]
    pub const fn shadows(&self) -> Shadows {
        self.shadows
    }

    /// §4 质感与遮罩。
    #[must_use]
    pub const fn texture(&self) -> Texture {
        self.texture
    }

    /// §4 动效。
    #[must_use]
    pub const fn motion(&self) -> Motion {
        self.motion
    }

    /// §5.1 阅读层几何与范围。
    #[must_use]
    pub const fn reading(&self) -> ReadingMetrics {
        self.reading
    }

    /// §5.3 工具栏几何与入口。
    #[must_use]
    pub const fn reading_toolbar(&self) -> ReadingToolbar {
        self.reading_toolbar
    }

    /// §4 预览色卡。
    ///
    /// **恒定值**，与 `self` 无关——挂在方法上只为调用方便（`theme.swatches()`），
    /// 语义上它是全局常量。见 [`Swatches`] 的文档。
    #[must_use]
    pub const fn swatches(&self) -> Swatches {
        SWATCHES
    }

    /// 是否夜主题（供绘制层做少量分支；**不得**用于分页——FR-019）。
    ///
    /// 名字直白得刻意：它的唯一合法用途是"呈现层分流"（如夜主题不叠纸噪点）。
    /// 若出现 `if theme.is_night() { ... }` 影响页数 / 行盒的代码，那是
    /// FR-019 违规（主题切换 MUST NOT 触发重新分页）。
    #[must_use]
    pub const fn is_night(&self) -> bool {
        self.texture.paper_noise_opacity == 0.0
    }
}

/// 昼主题（Parchment）单例。
///
/// 用**单元结构体 + `const` 实例**而非 `Theme::day()` 构造函数：主题是纯值，
/// 构造无失败路径、无输入，函数式包装只会让 `Copy` 语义变模糊。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayTheme;

/// 夜主题（Obsidian）单例。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NightTheme;

impl DayTheme {
    /// §2.1 语义色。
    #[must_use]
    pub const fn palette(self) -> Palette {
        PARCHMENT
    }

    /// §3 字体三角色（两套主题共用同一字体栈——字体不随主题变）。
    #[must_use]
    pub const fn fonts(self) -> Fonts {
        FONTS
    }

    /// §3 字号层级。
    #[must_use]
    pub const fn type_scale(self) -> TypeScale {
        TYPE_SCALE
    }

    /// §4 间距。
    #[must_use]
    pub const fn spacing(self) -> SpacingScale {
        SPACING
    }

    /// §4 圆角。
    #[must_use]
    pub const fn radii(self) -> Radii {
        RADII
    }

    /// §4 骨架尺寸。
    #[must_use]
    pub const fn shell(self) -> ShellDims {
        SHELL
    }

    /// §4 阴影（昼用暖褐投影）。
    #[must_use]
    pub const fn shadows(self) -> Shadows {
        SHADOWS_DAY
    }

    /// §4 质感（含纸纤维噪点）。
    #[must_use]
    pub const fn texture(self) -> Texture {
        TEXTURE_DAY
    }

    /// §4 预览色卡（恒定）。
    #[must_use]
    pub const fn swatches(self) -> Swatches {
        SWATCHES
    }

    /// §4 动效。
    #[must_use]
    pub const fn motion(self) -> Motion {
        MOTION
    }

    /// §5.1 阅读层参数。
    #[must_use]
    pub const fn reading(self) -> ReadingMetrics {
        READING
    }

    /// §5.3 工具栏几何。
    #[must_use]
    pub const fn reading_toolbar(self) -> ReadingToolbar {
        READING_TOOLBAR
    }

    /// 展开为完整的 [`Theme`] 值。
    #[must_use]
    pub const fn theme(self) -> Theme {
        Theme {
            name: "parchment",
            palette: PARCHMENT,
            fonts: FONTS,
            type_scale: TYPE_SCALE,
            spacing: SPACING,
            radii: RADII,
            shell: SHELL,
            shadows: SHADOWS_DAY,
            texture: TEXTURE_DAY,
            motion: MOTION,
            reading: READING,
            reading_toolbar: READING_TOOLBAR,
        }
    }
}

impl NightTheme {
    /// §2.2 语义色。
    #[must_use]
    pub const fn palette(self) -> Palette {
        OBSIDIAN
    }

    /// §3 字体三角色。
    #[must_use]
    pub const fn fonts(self) -> Fonts {
        FONTS
    }

    /// §3 字号层级。
    #[must_use]
    pub const fn type_scale(self) -> TypeScale {
        TYPE_SCALE
    }

    /// §4 间距。
    #[must_use]
    pub const fn spacing(self) -> SpacingScale {
        SPACING
    }

    /// §4 圆角。
    #[must_use]
    pub const fn radii(self) -> Radii {
        RADII
    }

    /// §4 骨架尺寸。
    #[must_use]
    pub const fn shell(self) -> ShellDims {
        SHELL
    }

    /// §4 阴影（夜用纯黑投影）。
    #[must_use]
    pub const fn shadows(self) -> Shadows {
        SHADOWS_NIGHT
    }

    /// §4 质感（无纸噪点）。
    #[must_use]
    pub const fn texture(self) -> Texture {
        TEXTURE_NIGHT
    }

    /// §4 预览色卡（**与昼主题同一份**——恒定值）。
    #[must_use]
    pub const fn swatches(self) -> Swatches {
        SWATCHES
    }

    /// §4 动效（与昼主题同一份——动效不随主题变）。
    #[must_use]
    pub const fn motion(self) -> Motion {
        MOTION
    }

    /// §5.1 阅读层参数（与昼主题同一份——**几何参数与呈现参数分离**，
    /// FR-028/FR-019：主题切换 MUST NOT 触发重新分页）。
    #[must_use]
    pub const fn reading(self) -> ReadingMetrics {
        READING
    }

    /// §5.3 工具栏几何。
    #[must_use]
    pub const fn reading_toolbar(self) -> ReadingToolbar {
        READING_TOOLBAR
    }

    /// 展开为完整的 [`Theme`] 值。
    #[must_use]
    pub const fn theme(self) -> Theme {
        Theme {
            name: "obsidian",
            palette: OBSIDIAN,
            fonts: FONTS,
            type_scale: TYPE_SCALE,
            spacing: SPACING,
            radii: RADII,
            shell: SHELL,
            shadows: SHADOWS_NIGHT,
            texture: TEXTURE_NIGHT,
            motion: MOTION,
            reading: READING,
            reading_toolbar: READING_TOOLBAR,
        }
    }
}

impl From<DayTheme> for Theme {
    fn from(day: DayTheme) -> Self {
        day.theme()
    }
}

impl From<NightTheme> for Theme {
    fn from(night: NightTheme) -> Self {
        night.theme()
    }
}

impl fmt::Display for Theme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_and_night_derive_distinct_themes() {
        let day: Theme = DayTheme.into();
        let night: Theme = NightTheme.into();
        assert_ne!(day, night);
        assert_ne!(day.is_night(), night.is_night());
    }

    #[test]
    fn reading_geometry_is_shared_across_themes() {
        // FR-019 / FR-028：几何参数与呈现参数分离。主题只该改**颜色与质感**，
        // 任何几何量在两套主题下必须相同——否则切主题会触发重排。
        let day: Theme = DayTheme.into();
        let night: Theme = NightTheme.into();
        assert_eq!(day.reading(), night.reading());
        assert_eq!(day.fonts(), night.fonts());
        assert_eq!(day.motion(), night.motion());
        assert_eq!(day.spacing(), night.spacing());
        assert_eq!(day.type_scale(), night.type_scale());
        assert_eq!(day.reading_toolbar(), night.reading_toolbar());
    }

    #[test]
    fn theme_is_copy_and_comparable() {
        let theme: Theme = DayTheme.into();
        let copied = theme;
        assert_eq!(theme, copied);
        assert_eq!(theme.name(), "parchment");
        assert_eq!(theme.to_string(), "parchment");
    }

    #[test]
    fn duration_ticks_degrade_to_zero_for_reduced_motion() {
        let slow = DurationTicks::from_ms(MOTION.dur_slow_ms);
        assert_eq!(slow.effective(false).ms(), 280);
        assert_eq!(slow.effective(true).ms(), 0);
        assert_eq!(DurationTicks::INSTANT.ms(), 0);
    }

    #[test]
    fn reading_range_self_consistency() {
        assert!(READING.font_size_default_in_range());
        assert!(READING.line_height_default_in_range());

        // 区间上下限的大小关系是**编译期可判定**的不变量（两个都是 `const`），
        // 故用 `const {}` 块断言：它会在编译时求值，写反了直接编译失败，
        // 而不是等某个测试跑到这里才知道。clippy 的 `assertions_on_constants`
        // 也正是这个建议——照做比豁免 lint 更对。
        const {
            assert!(READING.font_size_min_px < READING.font_size_max_px);
            assert!(READING.line_height_min < READING.line_height_max);
        }
    }

    #[test]
    fn two_page_threshold_scales_with_font_size() {
        assert!((READING.two_page_threshold_px() - 1432.0).abs() < f32::EPSILON);
        let big = READING.with_font_size(24.0);
        assert!((big.two_page_threshold_px() - 1792.0).abs() < f32::EPSILON);
        assert!(READING.enables_two_page(1432.0));
        assert!(!READING.enables_two_page(1431.9));
    }
}
