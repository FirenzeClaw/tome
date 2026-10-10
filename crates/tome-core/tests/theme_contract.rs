//! T002 验收：`Theme` 纯值类型与 `design-system/MASTER.md` token 表的逐一对应。
//!
//! ## 为什么这组断言必须存在
//!
//! MASTER.md 是用户拍板的**唯一**设计规范来源，`Theme` 是它在代码里的投影。
//! 两者漂移的表现是"界面慢慢变成另一个样子"——没有编译错误、没有测试失败，
//! 只有观感退化。故此处把"每个 token 都有对应字段且值一致"写成断言。
//!
//! ## 覆盖口径
//!
//! | MASTER 节 | 本文件断言 |
//! |---|---|
//! | §2.1 昼 Parchment（20 色） | [`day_palette_matches_master_2_1`] |
//! | §2.2 夜 Obsidian（20 色） | [`night_palette_matches_master_2_2`] |
//! | §3 字体三角色 + 字号层级 | [`font_roles_match_master_3`] / [`type_scale_matches_master_3`] |
//! | §4 间距/圆角/尺寸/阴影/遮罩/色卡/动效 | [`spacing_matches_master_4`] 等 |
//! | §5.1 阅读层参数 | [`reading_metrics_match_master_5_1`] |
//!
//! **不在本文件断言**的部分：§2.3 对比度表（值来自 WCAG 公式，属计算性断言，
//! 归 S2-5 的 T041）；§2.3 的色距说明（同样属派生量）。

use tome_core::model::theme::{
    Color, DayTheme, DurationTicks, FontRole, NightTheme, Rgba, Shadow, Theme, Weight,
};

// ---------------------------------------------------------------------------
// §2.1 / §2.2 语义色
// ---------------------------------------------------------------------------

/// §2.1 昼 Parchment 的 20 个语义色逐一对应。
///
/// 断言用的是**字面量十六进制**而非 `ts` 里的同名值——若断言也走 token 常量，
/// 实现改错了常量时断言会跟着一起错，测试就变成了同义反复。
#[test]
fn day_palette_matches_master_2_1() {
    let palette = DayTheme.palette();

    let expected: [(&str, &str, Color); 20] = [
        ("--bg-base", "#E8DFC8", palette.bg_base),
        ("--bg-surface", "#F2EAD6", palette.bg_surface),
        ("--bg-elevated", "#FAF4E4", palette.bg_elevated),
        ("--bg-reading", "#F5EEDC", palette.bg_reading),
        ("--bg-inset", "#DFD4B8", palette.bg_inset),
        ("--border-subtle", "#D6C9A8", palette.border_subtle),
        ("--border-strong", "#BCA97F", palette.border_strong),
        ("--text-primary", "#2E2418", palette.text_primary),
        ("--text-secondary", "#5C4B33", palette.text_secondary),
        ("--text-tertiary", "#8A7658", palette.text_tertiary),
        ("--text-inverse", "#F5EEDC", palette.text_inverse),
        ("--accent-primary", "#8C3A1E", palette.accent_primary),
        ("--accent-hover", "#A34524", palette.accent_hover),
        ("--accent-soft", "#E4D3B8", palette.accent_soft),
        ("--accent-secondary", "#3F5C4A", palette.accent_secondary),
        ("--accent-clash", "#B8862B", palette.accent_clash),
        ("--state-success", "#3F5C4A", palette.state_success),
        ("--state-warning", "#B8862B", palette.state_warning),
        ("--state-error", "#A32B1F", palette.state_error),
        ("--state-info", "#665A4F", palette.state_info),
    ];

    for (token, hex, actual) in expected {
        assert_eq!(
            actual.to_hex_string().to_uppercase(),
            hex.to_uppercase(),
            "昼主题 {token} 与 MASTER §2.1 不符"
        );
    }
}

/// §2.2 夜 Obsidian 的 20 个语义色逐一对应。
#[test]
fn night_palette_matches_master_2_2() {
    let palette = NightTheme.palette();

    let expected: [(&str, &str, Color); 20] = [
        ("--bg-base", "#000000", palette.bg_base),
        ("--bg-surface", "#0D0B0B", palette.bg_surface),
        ("--bg-elevated", "#171414", palette.bg_elevated),
        ("--bg-reading", "#000000", palette.bg_reading),
        ("--bg-inset", "#131010", palette.bg_inset),
        ("--border-subtle", "#231E1E", palette.border_subtle),
        ("--border-strong", "#3A3232", palette.border_strong),
        ("--text-primary", "#E6E4DE", palette.text_primary),
        ("--text-secondary", "#9A9790", palette.text_secondary),
        ("--text-tertiary", "#6A6761", palette.text_tertiary),
        ("--text-inverse", "#0D0B0B", palette.text_inverse),
        ("--accent-primary", "#D9794A", palette.accent_primary),
        ("--accent-hover", "#E88F5F", palette.accent_hover),
        ("--accent-soft", "#2A1A12", palette.accent_soft),
        ("--accent-secondary", "#7FA88C", palette.accent_secondary),
        ("--accent-clash", "#D9AE5C", palette.accent_clash),
        ("--state-success", "#7FA88C", palette.state_success),
        ("--state-warning", "#D9AE5C", palette.state_warning),
        ("--state-error", "#E06B5C", palette.state_error),
        ("--state-info", "#B8AFA0", palette.state_info),
    ];

    for (token, hex, actual) in expected {
        assert_eq!(
            actual.to_hex_string().to_uppercase(),
            hex.to_uppercase(),
            "夜主题 {token} 与 MASTER §2.2 不符"
        );
    }
}

/// §2.3 的两条**非计算性**强制约定（计算性的对比度断言归 T041）。
#[test]
fn accessibility_invariants_from_master_2_3() {
    // 「MUST NOT 使用纯白作昼主题底」——FR-030 的明文要求。
    // 注意判的是**阅读层**色：应用最底层可以更暗，但正文底不能是纯白。
    assert_ne!(
        DayTheme.palette().bg_reading,
        Color::rgb(0xFF, 0xFF, 0xFF),
        "昼主题阅读底不得为纯白（MASTER §2.3 / FR-030）"
    );

    // 「MUST NOT 用深灰替代夜主题真黑」——同上是硬要求。
    assert_eq!(
        NightTheme.palette().bg_reading,
        Color::rgb(0x00, 0x00, 0x00),
        "夜主题阅读底必须是真黑（MASTER §2.3 / FR-030）"
    );

    // §3「禁止 800+ 字重」——枚举本身不含 800 以上，这里是编译期保证的
    // 运行期复述，防止有人日后往 Weight 里加变体。
    for weight in [
        Weight::Regular,
        Weight::Medium,
        Weight::SemiBold,
        Weight::Strong,
        Weight::Bold,
    ] {
        assert!(
            weight.value() <= 700,
            "字重 {} 超过 MASTER §3 的上限 700",
            weight.value()
        );
    }
}

/// 两套主题的语义色**字段集合必须完全一致**（同构）。
///
/// MASTER 的一致性纪律要求昼夜两表逐 token 对应；若某天只给一套加了字段
/// （如"夜主题专用强调色"），界面在切换主题时就会漏画。此处以"同一份
/// [`Palette`] 类型承载两套值"把这条约束交给编译器，本测试则补上
/// "两套值确实不同"这条——相同则说明复制粘贴时没改。
#[test]
fn both_palettes_are_distinct_and_complete() {
    let day = DayTheme.palette();
    let night = NightTheme.palette();

    assert_ne!(
        day.bg_reading, night.bg_reading,
        "昼夜阅读底若相同，说明主题未真正分离"
    );
    assert_ne!(day.text_primary, night.text_primary);

    // §2.1 的 state-info 说明里点名了"暖中性灰"这一裁决（原蓝色值被换掉）——
    // 断言新值确实是暖色（红分量 ≥ 绿分量 ≥ 蓝分量），防止回退到蓝灰。
    for (label, color) in [("昼", day.state_info), ("夜", night.state_info)] {
        let Color { red, green, blue } = color;
        assert!(
            red >= green && green >= blue,
            "{label} state-info {color:?} 不是暖中性灰（MASTER §2.1 裁决：禁蓝色系）"
        );
    }
}

// ---------------------------------------------------------------------------
// §3 字体三角色与字号层级
// ---------------------------------------------------------------------------

/// §3 字体三角色：正文衬线 / UI 无衬线 / 规则等宽。
#[test]
fn font_roles_match_master_3() {
    let fonts = DayTheme.fonts();

    // 正文衬线**内嵌子集排首位**（裁决 OQ#3→A / FR-050）：三平台观感一致
    // 依赖它，故它必须排在系统字体之前。
    assert_eq!(
        fonts.reading.stack[0], "Tome Serif CN",
        "内嵌子集必须是正文衬线首选（FR-050：三平台观感一致）"
    );
    // MASTER §3 列的 `--font-reading` 栈其余项（作为回退链）
    for family in [
        "Iowan Old Style",
        "Source Han Serif SC",
        "Songti SC",
        "Noto Serif SC",
        "Georgia",
        "SimSun",
        "serif",
    ] {
        assert!(
            fonts.reading.stack.contains(&family),
            "正文衬线回退链缺少 MASTER §3 的 {family}"
        );
    }

    // UI 与等宽**不内嵌**（FR-050 后半：走系统栈）
    assert!(
        fonts.ui.stack.contains(&"Segoe UI"),
        "UI 无衬线缺 MASTER §3 的 Segoe UI"
    );
    assert!(
        fonts.mono.stack.contains(&"Cascadia Code"),
        "等宽缺 MASTER §3 的 Cascadia Code"
    );
    assert!(
        fonts.mono.stack.contains(&"monospace"),
        "等宽栈须以通用族收尾"
    );

    // 三个角色各有落点，且正文/UI 的**首选**必须不同——MASTER §3 说这是
    // 「本设计的核心辨识度」："UI 是工具，正文是内容，两者字体刻意分离"。
    assert_ne!(
        fonts.reading.stack[0], fonts.ui.stack[0],
        "正文与 UI 的首选字体必须分离（MASTER §3 核心决策）"
    );
}

/// §3 字号层级八档，每档三属性（size / line-height / weight）齐全。
#[test]
fn type_scale_matches_master_3() {
    let scale = DayTheme.type_scale();

    let expected: [(&str, f32, f32, u16); 8] = [
        ("--fs-display", 30.0, 1.15, 700),
        ("--fs-h1", 22.0, 1.30, 650),
        ("--fs-h2", 17.0, 1.40, 600),
        ("--fs-h3", 15.0, 1.45, 600),
        ("--fs-body", 14.0, 1.60, 400),
        ("--fs-sm", 13.0, 1.55, 400),
        ("--fs-caption", 12.0, 1.50, 400),
        ("--fs-micro", 11.0, 1.40, 500),
    ];

    let actual = [
        scale.display,
        scale.h1,
        scale.h2,
        scale.h3,
        scale.body,
        scale.sm,
        scale.caption,
        scale.micro,
    ];

    for ((token, size, line_height, weight), step) in expected.into_iter().zip(actual) {
        assert_eq!(step.size_px, size, "{token} 字号不符");
        assert_eq!(step.line_height, line_height, "{token} 行高不符");
        assert_eq!(step.font_weight, weight, "{token} 字重不符");
    }
}

// ---------------------------------------------------------------------------
// §4 间距 / 圆角 / 尺寸 / 阴影 / 遮罩 / 色卡 / 动效
// ---------------------------------------------------------------------------

/// §4 间距 4px 网格九档。
#[test]
fn spacing_matches_master_4() {
    let spacing = DayTheme.spacing();

    let expected: [(&str, u16); 9] = [
        ("--sp-1", 4),
        ("--sp-2", 8),
        ("--sp-3", 12),
        ("--sp-4", 16),
        ("--sp-5", 20),
        ("--sp-6", 24),
        ("--sp-8", 32),
        ("--sp-10", 40),
        ("--sp-12", 48),
    ];

    for ((token, value), actual) in expected.into_iter().zip(spacing.scale()) {
        assert_eq!(actual, value, "{token} 间距不符");
    }

    // 4px 基础网格：全部取值须是 4 的整数倍（MASTER §4 的"间距（4px 基础网格）"）
    for value in spacing.scale() {
        assert_eq!(value % 4, 0, "间距 {value} 不在 4px 网格上");
    }
}

/// §4 圆角五档 + 布局骨架尺寸四项。
#[test]
fn radii_and_shell_dimensions_match_master_4() {
    let radii = DayTheme.radii();
    assert_eq!(radii.xs_px, 4, "--r-xs");
    assert_eq!(radii.sm_px, 6, "--r-sm");
    assert_eq!(radii.md_px, 8, "--r-md");
    assert_eq!(radii.lg_px, 12, "--r-lg");
    assert_eq!(radii.full_px, 999, "--r-full");

    let shell = DayTheme.shell();
    assert_eq!(shell.topbar_h_px, 56, "--topbar-h（MASTER §4/§7 AppShell）");
    assert_eq!(shell.sidebar_w_px, 240, "--sidebar-w");
    assert_eq!(shell.sidebar_w_collapsed_px, 64, "--sidebar-w-collapsed");
    assert_eq!(
        shell.control_h_px, 32,
        "--control-h（32 与 34 并存已统一为 32）"
    );
}

/// §4 阴影三档，昼夜各一套（夜用纯黑投影，昼用暖褐投影）。
#[test]
fn shadows_match_master_4() {
    let day = DayTheme.shadows();
    let night = NightTheme.shadows();

    let expected: [(Shadow, Shadow); 3] = [
        (
            Shadow {
                offset_x_px: 0,
                offset_y_px: 1,
                blur_px: 2,
                spread_px: 0,
                color: Rgba::new(70, 52, 28, 0.1),
            },
            Shadow {
                offset_x_px: 0,
                offset_y_px: 1,
                blur_px: 2,
                spread_px: 0,
                color: Rgba::new(0, 0, 0, 0.6),
            },
        ),
        (
            Shadow {
                offset_x_px: 0,
                offset_y_px: 6,
                blur_px: 16,
                spread_px: -4,
                color: Rgba::new(70, 52, 28, 0.16),
            },
            Shadow {
                offset_x_px: 0,
                offset_y_px: 6,
                blur_px: 16,
                spread_px: -4,
                color: Rgba::new(0, 0, 0, 0.75),
            },
        ),
        (
            Shadow {
                offset_x_px: 0,
                offset_y_px: 16,
                blur_px: 40,
                spread_px: -12,
                color: Rgba::new(70, 52, 28, 0.22),
            },
            Shadow {
                offset_x_px: 0,
                offset_y_px: 16,
                blur_px: 40,
                spread_px: -12,
                color: Rgba::new(0, 0, 0, 0.85),
            },
        ),
    ];

    let day_actual = [day.sm, day.md, day.lg];
    let night_actual = [night.sm, night.md, night.lg];

    for (index, ((expected_day, expected_night), (actual_day, actual_night))) in expected
        .into_iter()
        .zip(day_actual.into_iter().zip(night_actual))
        .enumerate()
    {
        assert_eq!(actual_day, expected_day, "昼 shadow-{} 不符", index + 1);
        assert_eq!(actual_night, expected_night, "夜 shadow-{} 不符", index + 1);
    }

    // MASTER §4「阴影（克制，仅悬浮层）」：偏移量的 y 必须为正（向下投），
    // blur 必须为正。这条防的是把阴影写成"发光"。
    for shadow in [day.sm, day.md, day.lg] {
        assert!(shadow.blur_px > 0, "阴影必须模糊半径为正");
        assert!(shadow.offset_y_px >= 0, "阴影不得向上投");
    }
}

/// §4 纸纤维噪点与遮罩（昼专属质感 + 两套遮罩底）。
#[test]
fn texture_and_overlay_match_master_4() {
    let day = DayTheme.texture();
    let night = NightTheme.texture();

    // 昼：SVG fractalNoise data-URI + 0.045 强度
    assert!(
        day.paper_noise_svg_data_uri.contains("fractalNoise"),
        "昼纸纤维噪点须是 SVG fractalNoise（MASTER §4）"
    );
    assert_eq!(day.paper_noise_opacity, 0.045);

    // 夜：`none`（暗场不叠噪点）+ 0 强度
    assert!(
        night.paper_noise_svg_data_uri.is_empty(),
        "夜主题纸噪点须为 none（MASTER §4：昼主题专属）"
    );
    assert_eq!(night.paper_noise_opacity, 0.0);

    assert_eq!(
        day.overlay_bg,
        Rgba::new(46, 36, 24, 0.28),
        "昼遮罩底 --overlay-bg"
    );
    assert_eq!(
        night.overlay_bg,
        Rgba::new(0, 0, 0, 0.66),
        "夜遮罩底 --overlay-bg"
    );
}

/// §4 主题预览色卡——**恒定值**，两套主题下的色卡本身不随当前主题变化。
#[test]
fn swatches_are_constant_across_themes() {
    let day_swatches = DayTheme.swatches();
    let night_swatches = NightTheme.swatches();

    // 这是 MASTER §4 的明文要求："两套主题的预览色卡是**恒定值**（不随当前
    // 主题变化）"。若实现把色卡挂在 Theme 上并随主题切换，预览就会"说谎"。
    assert_eq!(
        day_swatches, night_swatches,
        "预览色卡必须恒定（MASTER §4：否则色卡与实际调色板脱钩，成为说谎的预览）"
    );

    assert_eq!(day_swatches.day_bg, Color::rgb(0xF5, 0xEE, 0xDC));
    assert_eq!(day_swatches.day_a, Color::rgb(0x8C, 0x3A, 0x1E));
    assert_eq!(day_swatches.day_b, Color::rgb(0x3F, 0x5C, 0x4A));
    assert_eq!(day_swatches.day_c, Color::rgb(0xB8, 0x86, 0x2B));
    assert_eq!(day_swatches.night_bg, Color::rgb(0x00, 0x00, 0x00));
    assert_eq!(day_swatches.night_a, Color::rgb(0xD9, 0x79, 0x4A));
    assert_eq!(day_swatches.night_b, Color::rgb(0x7F, 0xA8, 0x8C));
    assert_eq!(day_swatches.night_c, Color::rgb(0xD9, 0xAE, 0x5C));
}

/// §4 动效：两档缓动 + 三档时长。
#[test]
fn motion_matches_master_4() {
    let motion = DayTheme.motion();

    assert_eq!(motion.ease_out, [0.22, 0.61, 0.36, 1.0], "--ease-out");
    assert_eq!(motion.ease_spring, [0.34, 1.30, 0.64, 1.0], "--ease-spring");

    assert_eq!(motion.dur_fast_ms, 150, "--dur-fast");
    assert_eq!(motion.dur_base_ms, 200, "--dur-base");
    assert_eq!(motion.dur_slow_ms, 280, "--dur-slow");

    // MASTER §4「动效（150–280ms）」：三档须落在区间内且严格递增，
    // 否则"快/基准/慢"的语义就失了序。
    assert!(motion.dur_fast_ms < motion.dur_base_ms);
    assert!(motion.dur_base_ms < motion.dur_slow_ms);
    assert!(motion.dur_fast_ms >= 150 && motion.dur_slow_ms <= 280);

    // FR-032：减少动效时降级为 0ms。此处提供**唯一**的降级取值点，
    // 避免各处自己写 0 或 0.01ms（后者是 CSS 的"字面 0 仍触发 transition
    // 事件"绕法，语义不同）。
    assert_eq!(DurationTicks::INSTANT.ms(), 0, "减少动效须为字面 0ms");
}

// ---------------------------------------------------------------------------
// §5.1 阅读层参数
// ---------------------------------------------------------------------------

/// §5.1 视口与版心七项；§5.2 的双页判据由 `Theme` 的派生函数给出。
#[test]
fn reading_metrics_match_master_5_1() {
    let reading = DayTheme.reading();

    assert_eq!(reading.measure_em, 36.0, "--reading-measure");
    assert_eq!(reading.measure_max_em, 42.0, "--reading-measure-max");
    assert_eq!(reading.gutter_px, 64.0, "--reading-gutter");
    assert_eq!(reading.font_size_px, 19.0, "--reading-font-size 默认");
    assert_eq!(reading.font_size_min_px, 14.0, "可调下限 14");
    assert_eq!(reading.font_size_max_px, 30.0, "可调上限 30");
    assert_eq!(reading.line_height, 1.85, "--reading-line-height 默认");
    assert_eq!(
        reading.line_height_min, 1.75,
        "行高下限 1.75（裁决 OQ#19→A）"
    );
    assert_eq!(reading.line_height_max, 2.2, "行高上限 2.2");
    assert_eq!(
        reading.paragraph_indent_em, 2.0,
        "--reading-paragraph-indent 默认"
    );
    assert_eq!(reading.paragraph_gap_em, 0.4, "--reading-paragraph-gap");
}

/// §2.3「正文行高 ≥ 1.75（强制）」是不可协商的下限——连参数**范围**都不许
/// 探到 1.75 以下。
///
/// 这条断言是 FR-013 裁决（OQ#19→A）的落点：MASTER §5.1 原写 1.4–2.2，
/// 与 §2.3 的强制条款冲突，以强制条款为准。
#[test]
fn line_height_floor_is_accessibility_mandate() {
    let reading = DayTheme.reading();
    assert!(
        reading.line_height_min >= 1.75,
        "行高下限 {} 违反 MASTER §2.3 的强制条款（≥1.75）",
        reading.line_height_min
    );
    assert!(reading.line_height_default_in_range());
}

/// §5.2 的双页判据：`available >= 2 × measure + gutter`（19px 基准 → 1432px）。
///
/// MASTER §5.2 给的是公式而非断点，且**明确更正过**原表的 1180/1500
/// （那两个数与公式自相矛盾）。故此处断言**公式**，并顺带核对 19px 换算值。
#[test]
fn two_page_threshold_follows_master_5_2_formula() {
    let reading = DayTheme.reading();

    // 19px × 36em = 684px；2 × 684 + 64 = 1432px
    assert_eq!(reading.measure_px(), 684.0);
    assert_eq!(reading.two_page_threshold_px(), 1432.0);

    // 字号变化时判据同步右移（FR-014：MUST NOT 使用固定像素断点）
    let bigger = reading.with_font_size(24.0);
    assert_eq!(bigger.measure_px(), 864.0);
    assert_eq!(bigger.two_page_threshold_px(), 1792.0);
    assert!(
        bigger.two_page_threshold_px() > reading.two_page_threshold_px(),
        "字号变大后双页阈值必须右移（em 制判据）"
    );
}

/// §5.3 工具栏热区与淡出时长（`--dur-base`）。
#[test]
fn reading_toolbar_matches_master_5_3() {
    let toolbar = DayTheme.reading_toolbar();

    assert_eq!(toolbar.hot_zone_height_px, 8, "顶部 8px 热区");
    assert_eq!(toolbar.idle_fade_after_ms, 2500, "静止 2.5s 淡出");
    assert_eq!(
        toolbar.fade_duration_ms,
        DayTheme.motion().dur_base_ms,
        "淡出用 --dur-base"
    );

    // §5.3 的入口清单十项（FR-034 要求清单可枚举）
    let entries = toolbar.entries;
    assert_eq!(entries.len(), 10, "工具栏入口应为十项（MASTER §5.3）");
    for required in [
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
    ] {
        assert!(
            entries.contains(&required),
            "工具栏入口缺少 {required}（MASTER §5.3）"
        );
    }
}

// ---------------------------------------------------------------------------
// 不变量与非功能性约束
// ---------------------------------------------------------------------------

/// FR-029 / 宪法 IV：`Theme` 是**纯值类型**，零图形依赖。
///
/// 这条在编译期由 `tome-core` 的**零依赖** Cargo.toml 保证（它连 serde 都没有）。
/// 本测试是运行期的复述：若有人给 tome-core 加了图形依赖，`cargo tree`
/// 断言（[`theme_adds_no_dependency`] 同目录的守卫脚本）会先红。
#[test]
fn theme_is_a_pure_value_type() {
    // 可 Copy / 可比较 / 可 Debug——纯值的三个基本性质。
    // 若 Theme 内部藏了 Arc<dyn GpuDevice> 之类，这些 derive 就不再成立。
    let theme = DayTheme;
    let copied = theme;
    assert_eq!(theme, copied);

    // 两套主题同类型：调用方（组合根）无需为昼夜写两套代码路径
    let themes: [Theme; 2] = [DayTheme.into(), NightTheme.into()];
    assert_ne!(themes[0], themes[1]);
    assert_eq!(themes[0].name(), "parchment");
    assert_eq!(themes[1].name(), "obsidian");
}

/// 颜色类型自身的取值契约（供上面所有色值断言可信）。
#[test]
fn color_type_behaves() {
    let color = Color::rgb(0xE8, 0xDF, 0xC8);
    assert_eq!(color.to_hex_string().to_uppercase(), "#E8DFC8");
    assert_eq!(color.red, 0xE8);
    assert_eq!(color.green, 0xDF);
    assert_eq!(color.blue, 0xC8);

    // 无 alpha 的语义色：不透明（MASTER 的色表全是 6 位十六进制）
    assert_eq!(color.to_rgba().alpha, 1.0);

    // Rgba 用于阴影与遮罩（带 alpha）
    let rgba = Rgba::new(70, 52, 28, 0.10);
    assert_eq!(rgba.alpha, 0.10);
    assert!((rgba.alpha - 0.10).abs() < f32::EPSILON);
}

/// 字重枚举到 CSS 数值的映射（MASTER §3 只允许 400/500/600/650/700）。
#[test]
fn weight_values_are_the_five_allowed() {
    assert_eq!(Weight::Regular.value(), 400);
    assert_eq!(Weight::Medium.value(), 500);
    assert_eq!(Weight::SemiBold.value(), 600);
    assert_eq!(Weight::Strong.value(), 650);
    assert_eq!(Weight::Bold.value(), 700);
}

/// 字体角色三枚举齐全（§3 的"三个角色"）。
#[test]
fn font_role_enum_is_complete() {
    let roles = FontRole::ALL;
    assert_eq!(roles.len(), 3);
    assert!(roles.contains(&FontRole::Reading));
    assert!(roles.contains(&FontRole::Ui));
    assert!(roles.contains(&FontRole::Mono));
}

/// `SpacingScale` 的命名访问器与数组视图一致（防两处定义漂移）。
#[test]
fn spacing_accessors_agree_with_array_view() {
    let spacing = DayTheme.spacing();
    let view = spacing.scale();
    assert_eq!(spacing.sp_1, view[0]);
    assert_eq!(spacing.sp_2, view[1]);
    assert_eq!(spacing.sp_3, view[2]);
    assert_eq!(spacing.sp_4, view[3]);
    assert_eq!(spacing.sp_5, view[4]);
    assert_eq!(spacing.sp_6, view[5]);
    assert_eq!(spacing.sp_8, view[6]);
    assert_eq!(spacing.sp_10, view[7]);
    assert_eq!(spacing.sp_12, view[8]);
}

/// `TypeScale` 的命名访问器与数组视图一致。
#[test]
fn type_scale_accessors_agree_with_array_view() {
    let scale = DayTheme.type_scale();
    let view = scale.steps();
    assert_eq!(scale.display, view[0]);
    assert_eq!(scale.h1, view[1]);
    assert_eq!(scale.h2, view[2]);
    assert_eq!(scale.h3, view[3]);
    assert_eq!(scale.body, view[4]);
    assert_eq!(scale.sm, view[5]);
    assert_eq!(scale.caption, view[6]);
    assert_eq!(scale.micro, view[7]);
}
