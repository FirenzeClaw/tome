//! T008 验收：主题桥（`crates/tome-app/src/theme_bridge.rs`）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T008）
//!
//! - 消费 `tome_core::theme` 门面，**零色值重声明**（源码无 hex 色值字面量）
//! - R7 四规则落实为常量注释与 API：鎏金禁作文本前景、`text-tertiary` 禁必要
//!   信息、`border-strong` 不作唯一可见边界、badge 重建
//! - `FollowSystem` 在 OS 主题切换时返回对应 Day/Night token（单元测试以注入
//!   探测源模拟）
//!
//! ## 为什么"零色值重声明"要写成 grep 守卫
//!
//! 设计系统的纪律是「配色只从 `tome_core::theme` 取」。而违反它的形态非常
//! 自然：某个组件顺手写 `Color32::from_rgb(0x8C, 0x3A, 0x1E)`——代码能跑、
//! 观感"看起来也对"，**没有任何测试会红**。等到设计系统改一次主色，那一处
//! 就成了不跟随的孤岛，而发现它的代价是有人肉眼比对两个屏幕。
//!
//! 故守卫是**扫描源码文本**（不是断言某个 API 的行为）：它与"有没有人写死
//! 色值"这件事直接对应，而不是间接通过某个可观测后果去推断。

use tome_app::theme_bridge::{
    BadgeStyle, BridgeTheme, FollowSystem, OsTheme, SurfaceEmphasis, ThemeSource,
};
use tome_core::model::theme::{Color, DayTheme, NightTheme, OBSIDIAN, PARCHMENT};
use tome_reader::theme::{contrast_ratio, thresholds};

/// 取一个测试用的探测源（`FollowSystem` 的注入点）。
fn fixed_probe(theme: OsTheme) -> FollowSystem {
    FollowSystem::with_probe(move || theme)
}

// ===========================================================================
// 跟随系统：三态语义
// ===========================================================================

/// `FollowSystem` 探测到昼 → 昼 token；探测到夜 → 夜 token。
#[test]
fn follow_system_maps_probe_result_to_tokens() {
    let day = BridgeTheme::resolve(ThemeSource::FollowSystem, fixed_probe(OsTheme::Light));
    assert_eq!(
        day.theme().palette(),
        PARCHMENT,
        "探测到系统昼主题时应取 Parchment token"
    );

    let night = BridgeTheme::resolve(ThemeSource::FollowSystem, fixed_probe(OsTheme::Dark));
    assert_eq!(
        night.theme().palette(),
        OBSIDIAN,
        "探测到系统夜主题时应取 Obsidian token"
    );
}

/// 探测结果**未知**时回落到昼（而不是猜夜）。
///
/// ## 为什么默认昼而不是夜
///
/// `dark-light` 在探测不出时返回 `Mode::Unspecified`（Linux 上无
/// `prefers-color-scheme` 等价物时常见）。回落到昼的理由是**失败可见性**：
/// 昼主题的深色文字在浅底上，若用户实际环境是暗色桌面，窗口会明显"亮了一
/// 块"，用户立刻知道去手动设一次；回落到夜则在暗色桌面上"看起来正常"，
/// 而用户在亮色环境下会遇到一整屏纯黑——那时更难归因。
#[test]
fn unspecified_probe_falls_back_to_day() {
    let theme = BridgeTheme::resolve(ThemeSource::FollowSystem, fixed_probe(OsTheme::Unspecified));
    assert_eq!(
        theme.theme().palette(),
        PARCHMENT,
        "探测不出系统主题时应回落到昼（失败可见性更好）"
    );
}

/// 显式选择 Day / Night **不**受探测结果影响（探测失败不该改变用户的明确选择）。
#[test]
fn explicit_choice_ignores_the_probe() {
    for probe in [OsTheme::Light, OsTheme::Dark, OsTheme::Unspecified] {
        let day = BridgeTheme::resolve(ThemeSource::Day, fixed_probe(probe));
        assert_eq!(
            day.theme().palette(),
            PARCHMENT,
            "显式选昼时探测结果（{probe:?}）不得影响结果"
        );

        let night = BridgeTheme::resolve(ThemeSource::Night, fixed_probe(probe));
        assert_eq!(
            night.theme().palette(),
            OBSIDIAN,
            "显式选夜时探测结果（{probe:?}）不得影响结果"
        );
    }
}

/// OS 主题**切换**时返回对应 token（同一次会话内变化）。
#[test]
fn follow_system_tracks_os_changes() {
    let probe = FollowSystem::with_probe(|| OsTheme::Light);
    let first = BridgeTheme::resolve(ThemeSource::FollowSystem, probe.clone());
    assert_eq!(first.theme().palette(), PARCHMENT);

    // 模拟用户在系统设置里切到暗色
    probe.set_probed(OsTheme::Dark);
    let second = BridgeTheme::resolve(ThemeSource::FollowSystem, probe);
    assert_eq!(
        second.theme().palette(),
        OBSIDIAN,
        "OS 主题切换后 FollowSystem 应返回夜 token"
    );
}

/// 主题切换**不改变几何**（FR-019 / 002 SC-005 的 S3 侧对应）。
#[test]
fn switching_theme_does_not_change_geometry() {
    let day = BridgeTheme::resolve(ThemeSource::Day, fixed_probe(OsTheme::Light));
    let night = BridgeTheme::resolve(ThemeSource::Night, fixed_probe(OsTheme::Dark));

    assert_eq!(
        day.theme().reading(),
        night.theme().reading(),
        "阅读几何在两套主题下必须相同——否则切主题会触发重排"
    );
    assert_eq!(day.theme().spacing(), night.theme().spacing());
    assert_eq!(day.theme().type_scale(), night.theme().type_scale());
    assert_eq!(day.theme().radii(), night.theme().radii());
}

// ===========================================================================
// egui 映射
// ===========================================================================

/// egui 的 Visuals 底色取自 core 的 token（不是自算的近似值）。
#[test]
fn egui_visuals_come_from_core_tokens() {
    let bridge = BridgeTheme::resolve(ThemeSource::Day, fixed_probe(OsTheme::Light));
    let visuals = bridge.visuals();
    let palette = bridge.theme().palette();

    assert_eq!(
        visuals.panel_fill,
        to_color32(palette.bg_base),
        "面板底色应取 --bg-base"
    );
    assert_eq!(
        visuals.window_fill,
        to_color32(palette.bg_elevated),
        "浮层底色应取 --bg-elevated"
    );
    assert_eq!(
        visuals.override_text_color,
        Some(to_color32(palette.text_primary)),
        "界面正文色应取 --text-primary"
    );
}

/// 焦点环用 `--accent-primary`（MASTER §2.3 的强制条款）。
#[test]
fn focus_ring_uses_accent_primary() {
    for source in [ThemeSource::Day, ThemeSource::Night] {
        let bridge = BridgeTheme::resolve(source, fixed_probe(OsTheme::Light));
        let accent = bridge.theme().palette().accent_primary;
        let ring = bridge.focus_ring_color();
        assert_eq!(
            ring,
            to_color32(accent),
            "{source:?} 的焦点环应取 --accent-primary"
        );
    }
}

/// 字号映射取 MASTER 的三元组（字号 + 行高倍数 + 字重）。
#[test]
fn type_scale_maps_size_and_line_height() {
    let bridge = BridgeTheme::resolve(ThemeSource::Day, fixed_probe(OsTheme::Light));
    let scale = bridge.theme().type_scale();

    let body = bridge.font_id_for_step(scale.body);
    assert!(
        (body.size - scale.body.size_px).abs() < f32::EPSILON,
        "FontId 的字号应取 --fs-body 的 {}",
        scale.body.size_px
    );

    let row_height = bridge.row_height_for_step(scale.body);
    let expected = scale.body.size_px * scale.body.line_height;
    assert!(
        (row_height - expected).abs() < 0.01,
        "行高应等于 字号 × 行高倍数（{} × {} = {expected}），实测 {row_height}",
        scale.body.size_px,
        scale.body.line_height
    );
}

/// 圆角映射取 core 的 `Radii`，且 `--r-full`（999）被 **clamp** 到 u8。
///
/// ## 为什么这条要单独断言
///
/// `epaint::CornerRadius` 用 `u8`，而 `--r-full = 999` 超界。直接 `as u8` 会
/// 得到 231（999 & 0xFF）——一个**看起来像**圆角但不是药丸的怪值。这是 R8
/// 点名的落点之一：必须显式 clamp 并记录偏差。
#[test]
fn radii_map_and_clamp_r_full() {
    let bridge = BridgeTheme::resolve(ThemeSource::Day, fixed_probe(OsTheme::Light));
    let radii = bridge.theme().radii();

    assert_eq!(
        bridge.corner_radius(radii.md_px),
        egui::CornerRadius::same(radii.md_px as u8)
    );
    assert_eq!(
        bridge.corner_radius(radii.full_px),
        egui::CornerRadius::same(u8::MAX),
        "--r-full=999 超 u8，必须 clamp 到 u8::MAX 而不是截断（截断会得到 231）"
    );
}

// ===========================================================================
// R7 四规则（MASTER 缺陷的处置）
// ===========================================================================

/// **R7-①：鎏金禁作文本前景**（昼主题下 2.20–2.95:1）。
#[test]
fn rule_1_accent_clash_is_never_a_text_foreground() {
    // API 层：没有任何"取文本色"的入口会返回鎏金
    for source in [ThemeSource::Day, ThemeSource::Night] {
        let bridge = BridgeTheme::resolve(source, fixed_probe(OsTheme::Light));
        let text = bridge.text_color_for(SurfaceEmphasis::Normal);
        assert_ne!(
            text,
            to_color32(bridge.theme().palette().accent_clash),
            "{source:?} 的正文色不得取鎏金（--accent-clash）"
        );
        assert_ne!(
            text,
            to_color32(bridge.theme().palette().state_warning),
            "{source:?} 的正文色不得取警告色（与鎏金同值）"
        );
    }

    // 数值层：把"为什么禁"钉成可复算的事实
    let ratio = contrast_ratio(PARCHMENT.accent_clash, PARCHMENT.bg_reading);
    assert!(
        ratio < thresholds::AA_BODY_TEXT,
        "鎏金在昼主题阅读区上的对比度应低于 AA（实测 {ratio:.2}）——\
         这正是它被禁作文本前景的原因；若哪天它达标了，本规则可以放宽"
    );
}

/// **R7-②：`text-tertiary` 禁用于必要信息**（两主题均 < 4.5）。
#[test]
fn rule_2_text_tertiary_is_never_used_for_required_information() {
    for source in [ThemeSource::Day, ThemeSource::Night] {
        let bridge = BridgeTheme::resolve(source, fixed_probe(OsTheme::Light));
        let required = bridge.text_color_for(SurfaceEmphasis::Required);
        assert_ne!(
            required,
            to_color32(bridge.theme().palette().text_tertiary),
            "{source:?} 的必要信息文本不得用 --text-tertiary"
        );
        assert_eq!(
            required,
            to_color32(bridge.theme().palette().text_secondary),
            "{source:?} 的必要信息文本应退到 --text-secondary（MASTER 承诺的下限）"
        );
    }

    // 数值层：tertiary 在**全部**背景层上都不达 AA
    for (name, palette) in [("昼", PARCHMENT), ("夜", OBSIDIAN)] {
        for background in palette_backgrounds(&palette) {
            let ratio = contrast_ratio(palette.text_tertiary, background);
            assert!(
                ratio < thresholds::AA_BODY_TEXT,
                "{name}主题的 tertiary 在某个背景上达到了 AA（{ratio:.2}）——\
                 若设计系统调亮了它，本规则应重新审视（当前规则来自 R7/D3）"
            );
        }
    }
}

/// **R7-③：`border-strong` 不作唯一可见边界**（< 3:1）。
#[test]
fn rule_3_border_strong_is_not_the_only_boundary() {
    // 输入框的边界表达必须是"底 + 环"，而不是"只靠描边"
    let input = BadgeStyle::input_field();
    assert!(
        input.background_is_set(),
        "输入框必须给底色（--bg-inset），不得只靠 --border-strong 描边（D2：最大仅 2.10:1）"
    );
    assert!(
        input.has_focus_ring(),
        "输入框必须叠加焦点环（R7-③ 的处置：底 + 环）"
    );

    // 数值层
    for (name, palette) in [("昼", PARCHMENT), ("夜", OBSIDIAN)] {
        let ratio = contrast_ratio(palette.border_strong, palette.bg_inset);
        assert!(
            ratio < thresholds::AA_LARGE_TEXT_OR_UI,
            "{name}主题的 border-strong 达到了 3:1（{ratio:.2}）——\
             若设计系统调深了它，本规则应重新审视（当前规则来自 R7/D2）"
        );
    }
}

/// **R7-④：badge 不照抄 preview，按 token 重建**。
///
/// ## 原型为什么不能照抄
///
/// `preview.html` 的 `.badge`（金底 + `text-primary`）在**夜主题**下只有
/// 1.63:1——因为 `text-primary` 在夜里是浅色，而金底也是浅色。MASTER §9.1
/// 却称状态页「可直接照抄」，故这里必须显式不照抄：前景色**随主题切换**。
#[test]
fn rule_4_badges_are_rebuilt_from_tokens_not_copied_from_the_preview() {
    let day = BridgeTheme::resolve(ThemeSource::Day, fixed_probe(OsTheme::Light));
    let night = BridgeTheme::resolve(ThemeSource::Night, fixed_probe(OsTheme::Dark));

    let day_badge = day.badge_style(BadgeKind::Warning);
    let night_badge = night.badge_style(BadgeKind::Warning);

    // 底色都取 token（与原型一致）
    assert_eq!(
        day_badge.background,
        to_color32(PARCHMENT.state_warning),
        "警告徽记底色取 --state-warning"
    );
    assert_eq!(
        night_badge.background,
        to_color32(OBSIDIAN.state_warning),
        "夜主题警告徽记底色取 --state-warning（值不同，token 同名）"
    );

    // **前景随主题切换**——这是"不照抄"的实质
    assert_eq!(
        day_badge.foreground,
        to_color32(PARCHMENT.text_primary),
        "昼主题：金底配深墨字（4.69:1）"
    );
    assert_eq!(
        night_badge.foreground,
        to_color32(OBSIDIAN.text_inverse),
        "夜主题：金底配深底反白字——原型照抄 text-primary 会得到 1.63:1"
    );

    // 数值层：重建后的两套都达 AA
    for (name, style, palette) in [("昼", day_badge, PARCHMENT), ("夜", night_badge, OBSIDIAN)] {
        let foreground = from_color32(style.foreground);
        let background = from_color32(style.background);
        let ratio = contrast_ratio(foreground, background);
        assert!(
            ratio >= thresholds::AA_BODY_TEXT,
            "{name}主题的警告徽记对比度应 ≥ 4.5（重建后实测 {ratio:.2}）——\
             原型的配色在夜主题下只有 1.63:1，故不可照抄"
        );
        let _ = palette;
    }
}

// ===========================================================================
// 零色值重声明（grep 守卫）
// ===========================================================================

/// `tome-app` 的**源码**里不得出现 hex 色值字面量。
///
/// ## 扫描范围与豁免
///
/// - 扫 `src/**/*.rs`（生产代码）。测试文件（`tests/`）不在此列：它们要断言
///   "某色等于某 token"，引用 token 本身即可，但若将来需要构造反例，测试里
///   出现字面值是合理的。
/// - 本文件所在模块（`theme_bridge.rs`）**不豁免**——它正是最容易顺手写色的
///   地方（把 token 翻成 `Color32` 时）。
#[test]
fn no_hex_color_literals_in_tome_app_sources() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<String> = Vec::new();

    visit(&root, &mut |path, text| {
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            // 注释行不算（文档里引用 MASTER 的色值是**说明**，不是声明）
            if trimmed.starts_with("//") {
                continue;
            }
            if let Some(literal) = hex_literal_in(line) {
                offenders.push(format!(
                    "{}:{} 出现 hex 色值 {literal}",
                    path.display(),
                    index + 1
                ));
            }
        }
    });

    assert!(
        offenders.is_empty(),
        "MASTER §10：配色只从 tome_core::theme 取，禁止在 tome-app 重声明色值。\
         实测违规：{offenders:#?}"
    );
}

/// 从一行里找出 hex 色值字面量（`0xRRGGBB` 形态的六位数字）。
///
/// ## 为什么不匹配 `Color32::from_rgb(140, 58, 30)`
///
/// 那种形态无法与普通整数区分（`from_rgb(0, 0, 0)` 与任何三元组都像）。
/// 故守卫覆盖的是**最常见也最像设计 token**的写法：`0x8C3A1E`。这是刻意
/// 划定的边界——一个覆盖 80% 且零误报的守卫，比一个覆盖 100% 但天天误报
/// 最后被 `#[ignore]` 掉的守卫有用。
fn hex_literal_in(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index + 8 <= bytes.len() {
        if &bytes[index..index + 2] == b"0x" {
            let hex = &line[index + 2..index + 8];
            if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                // 排除更长/更短的十六进制（0x1234 是尺寸，不是颜色）
                let after = line[index + 8..].chars().next();
                if !after.is_some_and(|c| c.is_ascii_hexdigit()) {
                    return Some(line[index..index + 8].to_owned());
                }
            }
        }
        index += 1;
    }
    None
}

/// 递归访问目录下的 `.rs` 文件。
fn visit(dir: &std::path::Path, f: &mut impl FnMut(&std::path::Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, f);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                f(&path, &text);
            }
        }
    }
}

// ===========================================================================
// 夹具
// ===========================================================================

/// `BadgeKind` 是本测试用的别名（真实类型在 `theme_bridge`）。
use tome_app::theme_bridge::BadgeKind;

/// `tome_core::theme::Color` → `egui::Color32`（与 bridge 内部同一换算）。
fn to_color32(color: Color) -> egui::Color32 {
    egui::Color32::from_rgb(color.red, color.green, color.blue)
}

/// `egui::Color32` → `tome_core::theme::Color`（取 RGB 三分量）。
fn from_color32(color: egui::Color32) -> Color {
    Color::rgb(color.r(), color.g(), color.b())
}

/// 一套调色板的全部背景层（对比度复算用）。
fn palette_backgrounds(palette: &tome_core::model::theme::Palette) -> Vec<Color> {
    vec![
        palette.bg_base,
        palette.bg_surface,
        palette.bg_elevated,
        palette.bg_reading,
        palette.bg_inset,
    ]
}

/// `DayTheme` / `NightTheme` 的门面在这条测试里也要被覆盖（T008 的"消费门面"）。
#[test]
fn bridge_consumes_the_core_facade() {
    let day = DayTheme.theme();
    let night = NightTheme.theme();
    assert_eq!(day.name(), "parchment");
    assert_eq!(night.name(), "obsidian");

    let bridge = BridgeTheme::resolve(ThemeSource::Day, fixed_probe(OsTheme::Light));
    assert_eq!(
        bridge.theme().name(),
        day.name(),
        "桥的昼主题应就是 core 门面的昼实例"
    );
}

// ===========================================================================
// 平台 UI 字体链（R7 的 UI 字体裁决）
// ===========================================================================

/// 三个平台的字体链都非空且末位是通用回退。
///
/// ## 为什么断言"末位是通用回退"
///
/// 字体链的语义是"按序命中，全不中则用通用族"。若末位是个具体族名（如
/// `"Microsoft YaHei"`），在一台没装它的机器上整条链就**没有兜底**——
/// 表现为界面里一片豆腐块。通用族（`sans-serif`）由 egui 的默认字面保证。
#[test]
fn every_platform_chain_ends_with_a_generic_family() {
    use tome_app::theme_bridge::{ui_font_families, Platform};

    for platform in [Platform::Windows, Platform::MacOs, Platform::Linux] {
        let families = ui_font_families(platform);
        assert!(!families.is_empty(), "{platform:?} 的字体链不得为空");
        assert_eq!(
            families.last().copied(),
            Some("sans-serif"),
            "{platform:?} 的字体链末位应是通用回退（否则缺字时没有兜底）"
        );
    }
}

/// 当前平台的链取到的是本平台那一支。
#[test]
fn current_platform_selects_its_own_chain() {
    use tome_app::theme_bridge::{ui_font_families, Platform};

    let current = Platform::current();
    let families = ui_font_families(current);
    assert!(!families.is_empty());

    // 三平台的链互不相同（否则"按平台取子集"就是空话）
    let all: Vec<&[&str]> = [Platform::Windows, Platform::MacOs, Platform::Linux]
        .into_iter()
        .map(ui_font_families)
        .collect();
    assert_ne!(all[0], all[1]);
    assert_ne!(all[1], all[2]);
    assert_ne!(all[0], all[2]);
}

/// 全部候选路径都存在时：报告 `loaded` 齐全、`missing` 为空，且真的注册进了
/// egui 的字体定义（**Proportional 与 Monospace 两个族都追加**）。
#[test]
fn register_ui_fonts_appends_to_both_families() {
    use tome_app::theme_bridge::register_ui_fonts_from;

    let dir = tempfile::tempdir().expect("应能建临时目录");
    let first = dir.path().join("fake-ui.ttf");
    let second = dir.path().join("fake-cjk.ttc");
    // 内容不必是合法字体：egui 只在**使用**时解析，注册阶段只搬字节。
    // 这正是我们要测的那一步（"路径存在 → 字节进 font_data"）。
    std::fs::write(&first, b"not-a-real-font").expect("应能写");
    std::fs::write(&second, b"not-a-real-font-either").expect("应能写");

    let mut fonts = egui::FontDefinitions::default();
    let before_proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .map(Vec::len)
        .unwrap_or(0);

    let report = register_ui_fonts_from(
        &mut fonts,
        &[
            first.to_str().expect("路径应是 UTF-8"),
            second.to_str().expect("路径应是 UTF-8"),
        ],
    );

    assert_eq!(report.loaded.len(), 2, "两条路径都应加载成功");
    assert!(report.missing.is_empty(), "不应有缺失，实测 {report:?}");
    assert!(report.loaded_any());

    let proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .expect("Proportional 族应存在");
    assert_eq!(
        proportional.len(),
        before_proportional + 2,
        "两款字体都应**追加**到 Proportional 族末位（不覆盖内置字面）"
    );
    assert_eq!(
        fonts.font_data.len(),
        2 + egui::FontDefinitions::default().font_data.len()
    );

    // Monospace 同样追加（等宽族也要能显示中文：书源规则/正则里有中文注释）
    let monospace = fonts
        .families
        .get(&egui::FontFamily::Monospace)
        .expect("Monospace 族应存在");
    assert!(monospace.len() >= 2);
}

/// 部分路径缺失：如实报告，**已存在的照常加载**（缺失不阻断）。
#[test]
fn register_ui_fonts_reports_missing_paths_without_blocking() {
    use tome_app::theme_bridge::register_ui_fonts_from;

    let dir = tempfile::tempdir().expect("应能建临时目录");
    let present = dir.path().join("present.ttf");
    std::fs::write(&present, b"bytes").expect("应能写");

    let mut fonts = egui::FontDefinitions::default();
    let report = register_ui_fonts_from(
        &mut fonts,
        &[
            "/definitely/not/a/real/path/ui.ttf",
            present.to_str().expect("路径应是 UTF-8"),
            "/also/missing/cjk.ttc",
        ],
    );

    assert_eq!(report.loaded.len(), 1, "只有存在的那条应被加载");
    assert_eq!(report.missing.len(), 2, "两条不存在的应如实报告");
    assert!(
        report.loaded_any(),
        "部分缺失不阻断——界面仍应能用已加载的字体渲染"
    );
    assert!(
        report.missing[0].contains("not/a/real/path"),
        "缺失报告应带上具体路径（否则归因只能靠猜），实测 {report:?}"
    );
}

/// 全部缺失：`loaded_any()` 为 false，但**不 panic**（egui 自带字面兜底）。
#[test]
fn register_ui_fonts_survives_total_absence() {
    use tome_app::theme_bridge::register_ui_fonts_from;

    let mut fonts = egui::FontDefinitions::default();
    let before = fonts.families.clone();

    let report = register_ui_fonts_from(&mut fonts, &["/nope/a.ttf", "/nope/b.ttc"]);

    assert!(!report.loaded_any(), "全缺失时不应报告加载成功");
    assert_eq!(report.missing.len(), 2);
    assert_eq!(
        fonts.families, before,
        "全缺失时不得改动字体族（保持 egui 的内置定义可用）"
    );
}
