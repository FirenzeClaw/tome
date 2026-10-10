//! T017 验收：ReadingConfig schema（FR-013 / FR-028 / FR-018）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T017）
//!
//! - 全字段范围校验测试（字号 0/行高负/版心负宽拒绝）
//! - 序列化往返（作为 `Book.read_config` 的 schema）
//!
//! ## 判据的核心：**白名单校验必须真的拒绝**
//!
//! FR-013 的非法值是"字号 0、行高负、版心负宽"，spec §Edge Cases 要求
//! "必须在接受前校验并拒绝，回退到上一个有效值，不得产生无法渲染的状态"。
//!
//! "拒绝"有两种可能的实现，本文件把两者**都**钉住：
//!
//! 1. **单字段拒绝**：`ReadingConfig::validate_field` 对非法值返回错误，
//!    调用方据此保留上一个有效值（回落语义的落点）
//! 2. **整份拒绝**：`ReadingConfig::from_json` 对含非法字段的输入报错
//!
//! 反例同样重要：**合法值不得被拒**。一个"永远返回错误"的校验器也能让
//! 非法值测试通过——那是 T005/T007/T009 三次踩到的"全绿但没测到"。
//! 故本文件对范围内每一档都断言接受。

use tome_reader::paginate::config::{
    FontFamily, ParagraphMode, ReadingConfig, ReadingMode, SpreadMode, ThemeId,
    DEFAULT_FONT_SIZE_PX, DEFAULT_GUTTER_PX, DEFAULT_LINE_HEIGHT, DEFAULT_MEASURE_EM,
    DEFAULT_MEASURE_MAX_EM, FONT_SIZE_RANGE, LINE_HEIGHT_RANGE,
};

// ===========================================================================
// 默认值（MASTER §5.1）
// ===========================================================================

/// 默认值逐字段等于 MASTER §5.1 与 spec §FR-013 的取值。
#[test]
fn defaults_match_master_5_1() {
    let config = ReadingConfig::default_ma();

    assert_eq!(config.geometry.font_size_px, 19, "字号默认 19px");
    assert!(
        (config.geometry.line_height - 1.85).abs() < 1e-6,
        "行高默认 1.85，实测 {}",
        config.geometry.line_height
    );
    assert!(
        (config.geometry.measure_em - 36.0).abs() < 1e-6,
        "版心目标默认 36em"
    );
    assert!(
        (config.geometry.measure_max_em - 42.0).abs() < 1e-6,
        "单栏上限默认 42em"
    );
    assert_eq!(config.geometry.gutter_px, 64, "中缝默认 64px");

    // FR-018：默认 **段首缩进 2em**（裁决 OQ#12→A）
    assert_eq!(
        config.geometry.paragraph_mode,
        ParagraphMode::Indent { em: 2.0 },
        "默认必须是段首缩进 2em（裁决 OQ#12→A），不是段间距"
    );

    // FR-013：单双页默认自动，阅读形态默认翻页
    assert_eq!(config.geometry.spread_mode, SpreadMode::Auto);
    assert_eq!(config.geometry.reading_mode, ReadingMode::Paged);

    // FR-016 + 裁决 OQ#20：章起新页默认开
    assert!(config.geometry.chapter_new_page, "「章节从新页开始」默认开");
}

/// 命名常量与默认值一致（防"改了默认忘了改常量"）。
#[test]
fn named_constants_agree_with_defaults() {
    let config = ReadingConfig::default_ma();
    assert_eq!(config.geometry.font_size_px, DEFAULT_FONT_SIZE_PX);
    assert!((config.geometry.line_height - DEFAULT_LINE_HEIGHT).abs() < 1e-6);
    assert!((config.geometry.measure_em - DEFAULT_MEASURE_EM).abs() < 1e-6);
    assert!((config.geometry.measure_max_em - DEFAULT_MEASURE_MAX_EM).abs() < 1e-6);
    assert_eq!(config.geometry.gutter_px, DEFAULT_GUTTER_PX);
}

// ===========================================================================
// 范围校验（FR-013 + spec §Edge Cases）
// ===========================================================================

/// **非法值拒绝**：字号 0 / 负数 / 超出 14–30（含边界外一档）。
#[test]
fn invalid_font_size_is_rejected() {
    // 0 与负数（spec §Edge Cases 明文列出的值）
    for bad in [0u16, 1, 13, 31, 100, u16::MAX] {
        let error = ReadingConfig::validate_font_size(bad).expect_err(&format!(
            "字号 {bad} 应被拒绝（合法范围 {FONT_SIZE_RANGE:?}）"
        ));
        assert_eq!(
            error, "font_size",
            "错误须标识非法字段名（供上层定位），实测 {error}"
        );
    }
}

/// **合法值不得被拒**（防"永远返回错误"的假校验）。
#[test]
fn valid_font_size_is_accepted() {
    for good in 14u16..=30 {
        assert!(
            ReadingConfig::validate_font_size(good).is_ok(),
            "字号 {good} 在 {FONT_SIZE_RANGE:?} 内，不得被拒"
        );
    }
}

/// **非法行高拒绝**：负数、零、超出 1.75–2.2。
///
/// ## 为什么下限是 1.75 而不是 1.4
///
/// MASTER §2.3「正文行高 ≥1.75」是无障碍**强制**条款，§10 把无障碍定为
/// "引擎级强制，非可选皮肤"。§5.1 原表的 1.4 与之冲突，裁决 OQ#19→A
/// 取强制条款——故 1.7 这类"看起来只是略低"的值也**必须**被拒。
#[test]
fn invalid_line_height_is_rejected() {
    for bad in [-1.0f32, 0.0, 0.5, 1.0, 1.74, 1.7, 2.21, 3.0, f32::NAN] {
        let error = ReadingConfig::validate_line_height(bad).expect_err(&format!(
            "行高 {bad} 应被拒绝（合法范围 {LINE_HEIGHT_RANGE:?}，下限是无障碍强制）"
        ));
        assert_eq!(error, "line_height");
    }
}

/// **合法行高三档**（SC-004 矩阵用的 1.75/1.85/2.2）全部接受，含**边界值**。
#[test]
fn valid_line_height_is_accepted() {
    for good in [1.75f32, 1.85, 2.0, 2.2] {
        assert!(
            ReadingConfig::validate_line_height(good).is_ok(),
            "行高 {good} 在 {LINE_HEIGHT_RANGE:?} 内（含边界），不得被拒"
        );
    }
}

/// **非法版心拒绝**：负宽、零、超过单栏上限。
#[test]
fn invalid_measure_is_rejected() {
    for bad in [-1.0f32, -100.0, 0.0] {
        let error =
            ReadingConfig::validate_measure_em(bad).expect_err(&format!("版心 {bad}em 应被拒绝"));
        assert_eq!(error, "measure_em");
    }
    // 版心目标不得超过上限（42em）
    let error =
        ReadingConfig::validate_measure_em(43.0).expect_err("版心 43em 超过 42em 上限，应被拒绝");
    assert_eq!(error, "measure_em");
}

/// 版心边界：36em（默认）与 42em（上限）都接受。
#[test]
fn valid_measure_is_accepted() {
    for good in [1.0f32, 36.0, 41.99, 42.0] {
        assert!(
            ReadingConfig::validate_measure_em(good).is_ok(),
            "版心 {good}em 合法，不得被拒"
        );
    }
}

/// 中缝走白名单校验入口（且**负值在类型上不可能**）。
///
/// ## 为什么这条断言的形式与别的字段不同
///
/// spec §Edge Cases 列的非法值是"字号 0、行高负、版心负宽"——中缝不在其中。
/// 而 `gutter_px` 的类型是 `u16`：负中缝**无法构造**，类型系统已经把它
/// 拦住了。让测试假装给一个负数既编译不过（`-1` 不是 `u16`），也说明不了
/// 什么。
///
/// 故此处断言的是**真实成立的两件事**：① 合法的中缝值被接受；
/// ② 该字段仍有校验入口（与其余字段一致，日后加约束时有落点）。
#[test]
fn gutter_is_accepted_and_checked() {
    for good in [0u16, 16, 64, 240] {
        assert!(
            ReadingConfig::validate_gutter_px(good).is_ok(),
            "中缝 {good}px 合法，不得被拒"
        );
    }
    // 负数在类型上不可能：u16 的最大值是 65535，没有负值可传。
    // 下面这行是编译期事实的运行时表达（若日后类型放宽为 i32，本测试
    // 会因 `validate_gutter_px(0)` 仍通过而**不会**报警——故真正的防线是
    // 类型本身；此处只记录该字段确实进了白名单入口）。
    assert!(ReadingConfig::validate_gutter_px(u16::MAX).is_ok());
}

/// 段量（缩进 em / 段距 em）非法值被拒：段缩进为负、段距为负。
#[test]
fn invalid_paragraph_amount_is_rejected() {
    let error = ReadingConfig::validate_paragraph_mode(&ParagraphMode::Indent { em: -1.0 })
        .expect_err("负段缩进应被拒绝");
    assert_eq!(error, "paragraph_mode");

    let error = ReadingConfig::validate_paragraph_mode(&ParagraphMode::Gap { em: -0.5 })
        .expect_err("负段距应被拒绝");
    assert_eq!(error, "paragraph_mode");

    // 合法值不得被拒
    assert!(ReadingConfig::validate_paragraph_mode(&ParagraphMode::Indent { em: 2.0 }).is_ok());
    assert!(ReadingConfig::validate_paragraph_mode(&ParagraphMode::Gap { em: 0.4 }).is_ok());
}

/// **整份配置的校验**：非法字段被逐一指出（不是"配置非法"这一句笼统话）。
#[test]
fn whole_config_validation_reports_field_name() {
    let mut config = ReadingConfig::default_ma();
    config.geometry.font_size_px = 0;
    let error = config.validate().expect_err("字号 0 的配置应校验失败");
    assert_eq!(error, "font_size", "须指出是哪个字段非法");

    let mut config = ReadingConfig::default_ma();
    config.geometry.line_height = -0.5;
    assert_eq!(config.validate().expect_err("负行高应失败"), "line_height");

    let mut config = ReadingConfig::default_ma();
    config.geometry.measure_em = -3.0;
    assert_eq!(config.validate().expect_err("负版心应失败"), "measure_em");

    // 默认配置必须通过（防"永远失败"的假校验）
    assert!(
        ReadingConfig::default_ma().validate().is_ok(),
        "默认配置必须自身合法"
    );
}

// ===========================================================================
// 几何 / 呈现子集分离（FR-028）
// ===========================================================================

/// **两个子集在类型上是分开的**：分页只吃几何子集（FR-019 / SC-005）。
///
/// ## 这条断言为什么比"文档说它们分离"强
///
/// SC-005 要求"昼/夜主题、不同字体族、不同动效开关产出的页序列 100% 相同"。
/// 若呈现参数与几何参数混在一个结构体里，**没有任何机制**阻止分页函数
/// 顺手读一下 `theme_id`——那时 SC-005 就靠实现者的自觉维持。
///
/// 把呈现参数挪进独立的 `presentation` 字段后，分页函数的签名只能拿到
/// `&GeometryConfig`，读不到呈现参数。本测试断言这两个字段是**独立可构造**
/// 的——即"改呈现不影响几何"在类型层面成立。
#[test]
fn geometry_and_presentation_subsets_are_separate() {
    let mut config = ReadingConfig::default_ma();
    let geometry_before = config.geometry;

    // 把呈现子集全改一遍
    config.presentation.theme_id = ThemeId::Obsidian;
    config.presentation.font_family = FontFamily::SystemFallback;
    config.presentation.reduce_motion = true;
    config.presentation.paper_noise = false;

    assert_eq!(
        config.geometry, geometry_before,
        "改呈现参数不得改动几何参数（FR-028 的子集分离）"
    );
}

/// 呈现子集的默认值：昼主题、内嵌衬线、动效开、噪点开。
#[test]
fn presentation_defaults() {
    let config = ReadingConfig::default_ma();
    assert_eq!(
        config.presentation.theme_id,
        ThemeId::Parchment,
        "默认昼主题"
    );
    assert_eq!(
        config.presentation.font_family,
        FontFamily::EmbeddedSerif,
        "正文字体默认内嵌子集（裁决 OQ#3→A）"
    );
    assert!(!config.presentation.reduce_motion, "默认不做动效降级");
    assert!(config.presentation.paper_noise, "昼主题默认带纸纤维噪点");
}

// ===========================================================================
// 序列化往返（作为 Book.read_config 的 schema）
// ===========================================================================

/// **JSON 往返**：`to_json` → `from_json` 逐字段相等。
#[test]
fn json_roundtrip_preserves_all_fields() {
    let config = ReadingConfig::default_ma();
    let json = config.to_json().expect("应能序列化");
    let restored = ReadingConfig::from_json(&json).expect("应能反序列化");

    assert_eq!(restored, config, "往返后配置应逐字段相等");
}

/// 非默认值的往返（防"往返只对默认值成立"的假绿）。
#[test]
fn json_roundtrip_preserves_non_default_values() {
    let mut config = ReadingConfig::default_ma();
    config.geometry.font_size_px = 24;
    config.geometry.line_height = 2.2;
    config.geometry.paragraph_mode = ParagraphMode::Gap { em: 0.4 };
    config.geometry.measure_em = 30.0;
    config.geometry.gutter_px = 48;
    config.geometry.spread_mode = SpreadMode::ForceSpread;
    config.geometry.reading_mode = ReadingMode::Scroll;
    config.geometry.chapter_new_page = false;
    config.presentation.theme_id = ThemeId::Obsidian;
    config.presentation.font_family = FontFamily::SystemFallback;
    config.presentation.reduce_motion = true;
    config.presentation.paper_noise = false;

    let json = config.to_json().expect("应能序列化");
    let restored = ReadingConfig::from_json(&json).expect("应能反序列化");

    assert_eq!(restored, config, "非默认值往返后应逐字段相等");
    assert_eq!(restored.geometry.reading_mode, ReadingMode::Scroll);
    assert_eq!(restored.presentation.theme_id, ThemeId::Obsidian);
}

/// **反序列化时校验**：含非法字段的 JSON 被拒绝（不是"读进来再崩"）。
///
/// spec §Edge Cases："排版参数被设为非法值（字号 0、行高负、版心负宽）：
/// 必须在**接受前**校验并拒绝"。
#[test]
fn deserialization_rejects_invalid_values() {
    // 字号 0
    let bad = r#"{"geometry":{"font_size_px":0,"line_height":1.85,"paragraph_mode":{"Indent":{"em":2.0}},"measure_em":36.0,"measure_max_em":42.0,"gutter_px":64,"spread_mode":"Auto","reading_mode":"Paged","chapter_new_page":true},"presentation":{"theme_id":"Parchment","font_family":"EmbeddedSerif","reduce_motion":false,"paper_noise":true}}"#;
    let error = ReadingConfig::from_json(bad).expect_err("字号 0 的 JSON 应被拒绝");
    assert_eq!(
        error.field(),
        Some("font_size"),
        "错误须指出非法字段，实测：{error}"
    );
    assert!(
        error.to_string().contains("font_size"),
        "错误的字符串形态也须含字段名，实测：{error}"
    );

    // 非 JSON
    assert!(
        ReadingConfig::from_json("not json at all").is_err(),
        "非法 JSON 应被拒绝"
    );

    // 缺字段
    assert!(
        ReadingConfig::from_json("{}").is_err(),
        "缺字段的 JSON 应被拒绝（不得静默用默认值补齐）"
    );
}

/// 错误信息是**标识式**的（不含面向用户的自然语言硬编码，FR-058）。
#[test]
fn validation_error_is_identifier_like() {
    let error = ReadingConfig::validate_font_size(0).expect_err("应报错");
    assert_eq!(error, "font_size");
    // 字段名是 ASCII 标识（配置字段名），不是中文文案
    assert!(
        error.is_ascii(),
        "校验错误须是字段标识而非文案，实测：{error}"
    );
}

// ===========================================================================
// 派生量的计算（供分页消费）
// ===========================================================================

/// 版心宽度（px）= 字号 × measure_em。
#[test]
fn measure_width_px_is_em_scaled() {
    let config = ReadingConfig::default_ma();
    // 36em × 19px = 684px
    assert!(
        (config.geometry.measure_width_px() - 684.0).abs() < 1e-3,
        "实测 {}",
        config.geometry.measure_width_px()
    );

    // 字号变化后同步（FR-014 的"em 制随字号实时重算"）
    let mut config = ReadingConfig::default_ma();
    config.geometry.font_size_px = 24;
    assert!(
        (config.geometry.measure_width_px() - 24.0 * 36.0).abs() < 1e-3,
        "24px 字号下版心应为 864px，实测 {}",
        config.geometry.measure_width_px()
    );
}

/// 单栏上限（px）= 字号 × measure_max_em（42em 裁决）。
#[test]
fn measure_max_width_px_is_em_scaled() {
    let config = ReadingConfig::default_ma();
    assert!(
        (config.geometry.measure_max_width_px() - 42.0 * 19.0).abs() < 1e-3,
        "上限应为 798px，实测 {}",
        config.geometry.measure_max_width_px()
    );
}

/// 行高（px）= 字号 × line_height。
#[test]
fn line_height_px_is_font_size_scaled() {
    let config = ReadingConfig::default_ma();
    assert!(
        (config.geometry.line_height_px() - 19.0 * 1.85).abs() < 1e-3,
        "实测 {}",
        config.geometry.line_height_px()
    );
}
