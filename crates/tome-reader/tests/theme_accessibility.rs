//! T041 验收：语义色映射 + 无障碍强制断言（FR-030 / FR-031）
//!
//! ## 本文件的两组判据
//!
//! **A. FR-030 的语义映射强制项**（阅读区用对层的颜色）
//!
//! | 强制项 | 依据 |
//! |---|---|
//! | 阅读区背景 MUST 用主题的 `bg_reading`（不是 `bg_base` / 纯白） | FR-030 |
//! | 昼底 MUST NOT 是纯白 `#FFFFFF` | MASTER §8 Don'ts |
//! | 夜底 MUST NOT 用深灰代替真黑 | MASTER §8 Don'ts |
//! | 正文色 MUST NOT 低于 `text_secondary` | MASTER §2.3 强制条款 |
//! | 字重 MUST NOT 超过 700 | MASTER §8 Don'ts / §3 |
//!
//! **B. FR-031 的无障碍强制项**（引擎级强制，不是"样式建议"）
//!
//! | 强制项 | 阈值 |
//! |---|---|
//! | 六组关键组合的对比度 | ≥ WCAG AA（正文 4.5:1，大字 3:1） |
//! | 正文行高下限 | ≥ 1.75（MASTER §2.3 强制） |
//! | focus 环 | 2px |
//! | 触达目标 | ≥ 32px |
//!
//! ## 为什么这些断言必须存在（而不是"设计规范已经写了"）
//!
//! MASTER 是**文档**，代码不会因为它写了就遵守。若没有断言，"某次改动把
//! 夜主题底色调成 `#222` 会更舒服"会直接进仓库——而它是 MASTER 明文禁止的。
//! 本文件把已拍板的设计约束变成**会失败的测试**，这是"引擎级强制"的实现。
//!
//! ## 对比度算法
//!
//! WCAG 2.1 相对亮度（`L = 0.2126R + 0.7152G + 0.0722B`，各分量先做
//! sRGB → 线性转换），对比度 `(L1 + 0.05) / (L2 + 0.05)`。本文件**独立实现**
//! 该算法（不调用被测代码），否则"算错了"会与"断言通过"一起发生。
//! 已知答案校验见 `contrast_algorithm_matches_wcag_known_values`。

use tome_core::model::theme::{DayTheme, NightTheme, Theme, Weight};
use tome_reader::theme::{contrast_ratio, relative_luminance, ReadingPalette};

// ===========================================================================
// 夹具：WCAG 对比度算法（独立实现，用于交叉校验被测实现）
// ===========================================================================

/// sRGB 分量（0–255）→ 线性光（0.0–1.0）。
fn linearize(channel: u8) -> f64 {
    let srgb = f64::from(channel) / 255.0;
    if srgb <= 0.040_45 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

/// 独立算一遍相对亮度（测试自己的实现）。
fn reference_luminance(red: u8, green: u8, blue: u8) -> f64 {
    0.2126 * linearize(red) + 0.7152 * linearize(green) + 0.0722 * linearize(blue)
}

/// 独立算一遍对比度。
fn reference_contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let la = reference_luminance(a.0, a.1, a.2);
    let lb = reference_luminance(b.0, b.1, b.2);
    let (lighter, darker) = if la > lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

fn rgb_of(color: tome_core::model::theme::Color) -> (u8, u8, u8) {
    (color.red, color.green, color.blue)
}

// ===========================================================================
// 0. 对比度算法自校验（先证明尺子是准的）
// ===========================================================================

/// **对比度算法与 WCAG 已知值一致**。
///
/// ## 为什么这条必须先过
///
/// 后面全部对比度断言都依赖这把尺子。若算法错了（例如漏了 sRGB 线性化，
/// 或用了 `0.03928` 的旧阈值），"六组全达标"可能只是恰好算错的方向上
/// 偏松——那所有断言都是假的。已知值取自 W3C 的两个极端例：
///
/// - 黑 on 白 = **21:1**（最大值，无歧义）
/// - 同色 on 同色 = **1:1**（最小值）
/// - `#777777` on `#FFFFFF` ≈ **4.478:1**（WCAG 文档里的经典边界例，
///   常被用来演示"看似中灰却刚好差一点点未达 4.5"）
#[test]
fn contrast_algorithm_matches_wcag_known_values() {
    // 黑白极值
    assert!(
        (reference_contrast((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 0.001,
        "黑白对比度应为 21:1（WCAG 最大值）"
    );
    assert!(
        (reference_contrast((120, 120, 120), (120, 120, 120)) - 1.0).abs() < 0.001,
        "同色对比度应为 1:1"
    );

    // 被测实现与独立实现必须一致（同一输入逐位相同）
    let black = tome_core::model::theme::Color::rgb(0, 0, 0);
    let white = tome_core::model::theme::Color::rgb(255, 255, 255);
    let gray = tome_core::model::theme::Color::rgb(0x77, 0x77, 0x77);

    assert!(
        (contrast_ratio(black, white) - 21.0).abs() < 0.001,
        "被测实现的黑白对比度应为 21:1，实测 {}",
        contrast_ratio(black, white)
    );
    assert!(
        (relative_luminance(white) - 1.0).abs() < 0.0001,
        "白色的相对亮度应为 1.0"
    );
    assert!(
        (relative_luminance(black)).abs() < 0.0001,
        "黑色的相对亮度应为 0.0"
    );
    // 灰阶对照：被测实现与独立实现一致
    let via_impl = contrast_ratio(gray, white);
    let via_reference = reference_contrast((0x77, 0x77, 0x77), (255, 255, 255));
    assert!(
        (via_impl - via_reference).abs() < 0.001,
        "被测实现（{via_impl:.4}）与独立实现（{via_reference:.4}）应一致"
    );
    // 且该值确实略低于 4.5（这是 WCAG 文档里那个经典边界，用来证明尺子有分辨率）
    assert!(
        (3.5..4.5).contains(&via_impl),
        "#777 on #FFF 应在 3.5–4.5 之间（WCAG 的经典边界例），实测 {via_impl:.4}"
    );
}

// ===========================================================================
// A. FR-030 语义映射强制项
// ===========================================================================

/// **阅读区背景取自主题的 `bg_reading`**（FR-030 的第一条）。
///
/// ## 为什么是 `bg_reading` 而不是 `bg_base`
///
/// MASTER §2 把 `--bg-reading` 单列为"正文阅读区（最纯净层）"，而 `--bg-base`
/// 是"应用最底层"。两者在昼主题里是不同的值（`#F5EEDC` vs `#E8DFC8`）——
/// 混用会让阅读区带上外壳的底色，正文对比度随之变化（§2.3 的 11.8:1 是按
/// `bg_reading` 算的）。
#[test]
fn reading_background_uses_the_bg_reading_token() {
    for (label, theme) in [("昼", DayTheme.theme()), ("夜", NightTheme.theme())] {
        let palette = ReadingPalette::for_theme(&theme);
        assert_eq!(
            palette.background,
            theme.palette().bg_reading,
            "{label}主题的阅读区背景必须取 bg_reading（FR-030）"
        );
    }

    // 昼主题下两个 token 的值确实不同——这条证明"取 bg_reading"不是
    // "碰巧等于 bg_base"（夜主题两者的值都是 #000000，见下条说明）
    assert_ne!(
        DayTheme.theme().palette().bg_reading,
        DayTheme.theme().palette().bg_base,
        "昼主题：bg_reading 与 bg_base 是不同的 token（不得混用）"
    );
}

/// **夜主题的 `bg_reading` 与 `bg_base` 同值是有意为之**（记录设计事实）。
///
/// ## 为什么单独断言这一条
///
/// MASTER §2.2 把夜主题的 `--bg-base` 与 `--bg-reading` 都定义为 `#000000`
/// ——夜主题的"最底层"与"阅读层"在 OLED 上本就该是同一片真黑（这正是
/// "深灰替代真黑会丢失 OLED 价值"那条 Don't 的另一面）。故不能像昼主题那样
/// 用"两者不同"来证明映射取对了 token；这条把该设计事实**钉在测试里**，
/// 免得将来有人看到"两者相同"就以为是 bug 去改 `bg_reading`。
#[test]
fn night_theme_bg_reading_equals_bg_base_by_design() {
    let night = NightTheme.theme().palette();
    assert_eq!(
        night.bg_reading, night.bg_base,
        "夜主题的 bg_reading 与 bg_base 在 MASTER §2.2 里同为 #000000——这是设计事实"
    );
    let (red, green, blue) = rgb_of(night.bg_reading);
    assert_eq!((red, green, blue), (0, 0, 0), "夜主题阅读底是真黑");
}

/// **昼主题底 MUST NOT 是纯白**（MASTER §8 Don'ts）。
#[test]
fn day_reading_background_is_not_pure_white() {
    let day = ReadingPalette::for_theme(&DayTheme.theme());
    assert_ne!(
        rgb_of(day.background),
        (255, 255, 255),
        "禁止纯白 #FFFFFF 作昼主题底（MASTER §8：刺眼）"
    );
    // 更严一点：三通道都不该是 255 满值（"接近纯白"同样刺眼）
    let (red, green, blue) = rgb_of(day.background);
    assert!(
        red < 255 && green < 255 && blue < 255,
        "昼主题底不得有任何通道满值（禁用接近纯白的做旧感缺失）"
    );
}

/// **夜主题底 MUST 是真黑系**（MASTER §8 Don'ts：禁深灰代替真黑）。
#[test]
fn night_reading_background_is_true_black_not_dark_gray() {
    let night = ReadingPalette::for_theme(&NightTheme.theme());
    let (red, green, blue) = rgb_of(night.background);
    assert_eq!(
        (red, green, blue),
        (0, 0, 0),
        "夜主题底必须是真黑（MASTER §8：深灰替代会丢失 OLED 价值）"
    );
}

/// **正文色 MUST NOT 低于 `text_secondary`**（MASTER §2.3 强制条款）。
///
/// ## "不低于"的量化口径
///
/// `text_primary` 在色阶上应比 `text_secondary` **更深/更亮**（即在各自背景上
/// 对比度更高）。若某次改动把正文色换成了 `text_tertiary`，正文对比度会掉到
/// 二级文本之下——那是明确禁止的。故判据是**对比度序关系**：
/// `contrast(text_primary, bg) ≥ contrast(text_secondary, bg)`。
#[test]
fn body_text_is_never_weaker_than_text_secondary() {
    for (label, theme) in [("昼", DayTheme.theme()), ("夜", NightTheme.theme())] {
        let palette = ReadingPalette::for_theme(&theme);
        let background = rgb_of(palette.background);
        let body = rgb_of(palette.body_text);
        let secondary = rgb_of(palette.secondary_text);

        let body_contrast = reference_contrast(body, background);
        let secondary_contrast = reference_contrast(secondary, background);

        assert!(
            body_contrast >= secondary_contrast - 0.001,
            "{label}主题正文对比度（{body_contrast:.2}）不得低于次要文本（{secondary_contrast:.2}）\
             ——MASTER §2.3：正文色永不低于 text-secondary"
        );
        assert_eq!(
            palette.body_text,
            theme.palette().text_primary,
            "{label}主题正文色应取 text_primary"
        );
    }
}

/// **阅读层的字重 MUST NOT 超过 700**（MASTER §8 Don'ts / §3）。
///
/// ## `Weight` 枚举已经禁了 800+
///
/// `Weight` 只有到 `Bold`（700）的变体，故"字重 ≤700"在类型上已成立。
/// 本测试断言的是**阅读层实际用的字重值**：它必须落在允许集合内，且
/// 正文用的是常规字重（不是靠加粗凑对比度——那是另一种掩盖问题的写法）。
#[test]
fn reading_layer_weights_never_exceed_700() {
    // 枚举的全部变体都在 400–700 之间（编译期穷举）
    for weight in [
        Weight::Regular,
        Weight::Medium,
        Weight::SemiBold,
        Weight::Strong,
        Weight::Bold,
    ] {
        assert!(
            weight.value() <= 700,
            "字重 {} 超过 700（MASTER §8 Don'ts：禁止 >700 字重）",
            weight.value()
        );
    }

    // 阅读层的正文与次要文本都应是常规字重——正文若靠加粗提对比度，
    // 那是掩盖配色问题的写法（且会让整页观感变重，与 MASTER 的 calm 气质冲突）
    for (label, theme) in [("昼", DayTheme.theme()), ("夜", NightTheme.theme())] {
        let palette = ReadingPalette::for_theme(&theme);
        assert_eq!(
            palette.body_weight,
            Weight::Regular.value(),
            "{label}主题正文应为常规字重（400）"
        );
        assert!(palette.body_weight <= 700);
    }
}

/// **HIGHLIGHT 语义色不得落入蓝色系**（MASTER §8 Don'ts：禁止蓝色系）。
///
/// ## 判据
///
/// "蓝色系"的量化口径：蓝分量是**最大**分量且**明显**领先红分量
/// （差值 ≥ 24，约 10% 色阶）。这样既挡通用 admin 蓝（`#3B82F6` 之类），
/// 又不误伤暖灰（如 `#665A4F`，红最大；或 `#B8AFA0`，红最大）。
///
/// 阅读区会用到的高亮色全部检查——这是用户可见面积最大的一组色。
#[test]
fn reading_highlight_colors_are_never_blue() {
    for (label, theme) in [("昼", DayTheme.theme()), ("夜", NightTheme.theme())] {
        let palette = ReadingPalette::for_theme(&theme);
        for (name, color) in [
            ("background", palette.background),
            ("body_text", palette.body_text),
            ("secondary_text", palette.secondary_text),
            ("accent", palette.accent),
            ("highlight_underline", palette.highlight_underline),
            ("focus_ring", palette.focus_ring),
            ("footer_text", palette.footer_text),
        ] {
            let (red, green, blue) = rgb_of(color);
            assert!(
                !(blue > red && blue > green && blue.saturating_sub(red) >= 24),
                "{label}主题的 {name}（{color}）落入蓝色系——MASTER §8 明令禁止蓝色系\
                 （本项目暖色系身份不可妥协）"
            );
        }
    }
}

// ===========================================================================
// B. FR-031 无障碍强制项
// ===========================================================================

/// **§2.3 对比度表六组组合全部达标**（FR-031）。
///
/// ## 六组怎么定的
///
/// 取 MASTER §2.3 表格里**阅读区真正用到**的组合。表格给了 8 行，其中
/// 反复出现的是这几个语义角色 × 两个背景：
///
/// | # | 前景 | 背景 | 主题 | 门槛 |
/// |---|---|---|---|---|
/// | 1 | `text_primary` | `bg_reading` | 昼 | 4.5（正文） |
/// | 2 | `text_secondary` | `bg_reading` | 昼 | 4.5 |
/// | 3 | `accent_primary` | `bg_reading` | 昼 | 4.5 |
/// | 4 | `text_primary` | `bg_reading` | 夜 | 4.5 |
/// | 5 | `text_secondary` | `bg_reading` | 夜 | 4.5 |
/// | 6 | `accent_primary` | `bg_reading` | 夜 | 4.5 |
///
/// 全部按 **AA 正文门槛 4.5:1** 要求（不因"字号大"而放宽到 3:1——阅读区
/// 的字号是用户可调的，不能假定它总是大字号）。
#[test]
fn six_key_contrast_pairs_meet_wcag_aa() {
    let pairs = [
        ("昼 · 正文 on 阅读底", DayTheme.theme(), "body"),
        ("昼 · 次文 on 阅读底", DayTheme.theme(), "secondary"),
        ("昼 · 主色 on 阅读底", DayTheme.theme(), "accent"),
        ("夜 · 正文 on 阅读底", NightTheme.theme(), "body"),
        ("夜 · 次文 on 阅读底", NightTheme.theme(), "secondary"),
        ("夜 · 主色 on 阅读底", NightTheme.theme(), "accent"),
    ];
    assert_eq!(pairs.len(), 6, "§2.3 的阅读区关键组合应为 6 组");

    for (label, theme, role) in pairs {
        let palette = ReadingPalette::for_theme(&theme);
        let background = rgb_of(palette.background);
        let foreground = match role {
            "body" => rgb_of(palette.body_text),
            "secondary" => rgb_of(palette.secondary_text),
            "accent" => rgb_of(palette.accent),
            other => panic!("未知角色 {other}"),
        };
        let ratio = reference_contrast(foreground, background);
        assert!(
            ratio >= 4.5,
            "{label} 的对比度 {ratio:.2}:1 未达 WCAG AA（4.5:1）"
        );
    }
}

/// **正文行高下限 ≥ 1.75**（MASTER §2.3 强制条款，裁决 OQ#19→A）。
///
/// ## 为什么这条在 T041（而 T002 已经断言过 `ReadingMetrics`）
///
/// T002 断言的是 `READING` 常量自身的自洽性（`line_height_min = 1.75`）。
/// 本测试断言的是**阅读层实际施加的行高**——`ReadingPalette` 之外，
/// 排版层若在某处把行高压到 1.6（"省版面"），MASTER §2.3 就破了。
/// 两者是"常量定义"与"使用点"的区别。
#[test]
fn reading_line_height_enforces_the_1_75_floor() {
    for theme in [DayTheme.theme(), NightTheme.theme()] {
        let reading = theme.reading();
        assert!(
            reading.line_height_min >= 1.75,
            "行高下限 {} 低于 MASTER §2.3 的强制下限 1.75",
            reading.line_height_min
        );
        assert!(
            reading.line_height >= reading.line_height_min,
            "默认行高 {} 低于下限 {}",
            reading.line_height,
            reading.line_height_min
        );
        // 施加后的行高（几何配置派生）同样不得低于下限
        let geometry = tome_reader::paginate::config::ReadingConfig::default_ma().geometry;
        assert!(
            geometry.line_height >= 1.75,
            "几何配置的默认行高 {} 低于强制下限 1.75",
            geometry.line_height
        );
    }
}

/// **focus 环 2px + 触达目标 ≥32px**（FR-031 / MASTER §2.3）。
///
/// 两者都是"引擎级强制"的无障碍项——不是可由主题覆盖的建议值。
#[test]
fn focus_ring_and_target_size_are_engine_enforced() {
    // focus 环：2px + 2px offset（MASTER §2.3）
    for (label, theme) in [("昼", DayTheme.theme()), ("夜", NightTheme.theme())] {
        let palette = ReadingPalette::for_theme(&theme);
        assert_eq!(
            palette.focus_ring_width_px, 2,
            "{label}主题的 focus 环宽度必须是 2px（FR-031 强制）"
        );
        assert_eq!(
            palette.focus_ring_offset_px, 2,
            "{label}主题的 focus 环 offset 必须是 2px（MASTER §2.3）"
        );
        // focus 环色取主色（§2.3：两主题均保证可见）
        assert_eq!(
            palette.focus_ring,
            theme.palette().accent_primary,
            "{label}主题 focus 环应取 accent_primary"
        );
    }

    // 触达目标：≥32px（桌面），图标按钮 ≥36px
    let palette = ReadingPalette::for_theme(&DayTheme.theme());
    assert!(
        palette.min_target_size_px >= 32,
        "触达目标 {} px 低于 MASTER §2.3 的 32px 下限",
        palette.min_target_size_px
    );
    assert!(
        palette.min_icon_target_size_px >= 36,
        "图标按钮触达目标 {} px 低于 MASTER §2.3 的 36px 下限",
        palette.min_icon_target_size_px
    );
}

/// **focus 环在两套主题上都真的可见**（不是"取了主色"就算数）。
///
/// focus 环与它所在的底色对比度必须 ≥3:1（WCAG 2.1 对非文本 UI 组件的
/// 门槛，1.4.11）。若某天把 focus 环色改得与背景接近，它"存在"却"看不见"。
#[test]
fn focus_ring_is_visible_on_its_background_in_both_themes() {
    for (label, theme) in [("昼", DayTheme.theme()), ("夜", NightTheme.theme())] {
        let palette = ReadingPalette::for_theme(&theme);
        let ratio = reference_contrast(rgb_of(palette.focus_ring), rgb_of(palette.background));
        assert!(
            ratio >= 3.0,
            "{label}主题的 focus 环与阅读底对比度 {ratio:.2}:1 低于 WCAG 2.1 的 3:1\
             （§2.3 要求两主题均保证可见）"
        );
    }
}

/// **两套主题的映射结果必须不同**（防"永远返回同一份"的假绿）。
#[test]
fn day_and_night_produce_distinct_reading_palettes() {
    let day = ReadingPalette::for_theme(&DayTheme.theme());
    let night = ReadingPalette::for_theme(&NightTheme.theme());
    assert_ne!(
        day.background, night.background,
        "两个主题的阅读底必须不同——相同说明映射没读主题"
    );
    assert_ne!(day.body_text, night.body_text, "正文色应随主题变");
}

/// 主题是纯值：同一主题两次映射结果相同（无隐藏状态）。
#[test]
fn mapping_is_deterministic() {
    let theme: Theme = DayTheme.theme();
    assert_eq!(
        ReadingPalette::for_theme(&theme),
        ReadingPalette::for_theme(&theme),
        "同一主题的两次映射必须逐字段相同"
    );
}
