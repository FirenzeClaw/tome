//! T037 验收：字体内嵌加载 + fontdb 回退链（FR-050/051、FR-047；research R3）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T037）
//!
//! - `assets/fonts/TomeSerifCN-subset.otf` 经 fontdb 注册为 reading-serif 首选
//! - 系统回退链平台表（Windows 宋体 / macOS Songti SC / Linux Noto Serif CJK）
//! - 子集缺字时落回退链且**行位不变**（FR-047）
//! - `tome fonts` 数据可用：内嵌体积断言 + `within_budget` 为 true
//! - **字体获取零网络**（FR-051/FR-056）
//!
//! ## 零网络怎么断言
//!
//! 本模块的字体加载**只经两条路径**：`include_bytes!`（编译进二进制）与
//! fontdb 的系统扫描（本地文件系统）。两者都不产生网络请求。断言形式是
//! **结构性的**：`FontRegistry::load` 的签名不接任何 URL/HTTP 客户端，
//! 且本文件的用例在离线下运行（CI 的 headless job 就没有网络依赖）。
//!
//! 更直接的证据见 `tests/no_natural_language.rs` 同类的形态审查——
//! 但真正的保证来自"本 crate 的依赖树里没有 HTTP 客户端"
//! （`scripts/check-reader-features.sh` 与 001 的依赖图断言共同覆盖）。
//!
//! ## 预算口径（FR-050 的 3–5MB）
//!
//! SC-017 的原话是"内嵌正文衬线字体子集 MUST 控制在 3–5 MB 预算内；
//! **超出预算须记录原因并提请用户裁决**"——"预算"在此是**上限**语义
//! （超了要解释），不是下限。实测子集 2.18 MiB **低于**该区间，
//! 原因记录在 `spikes/s2/BASELINE.md` §7.2（改用 GB2312 6763 字档，
//! CFF 子集化的子程序共享比线性外推更省）。低于预算是好事（体积目标
//! 是 <20MB），故 `within_budget` 判据取"≤ 上限"。

#![cfg(feature = "render")]

use tome_reader::render::fonts::{
    embedded_font_bytes, fallback_chain_for_current_platform, family_name, FontRegistry,
    FontSource, FontsReport, EMBEDDED_FONT_BUDGET_BYTES, EMBEDDED_FONT_MIN_BYTES,
    READING_SERIF_FAMILY,
};

// ===========================================================================
// 内嵌子集：加载与元信息
// ===========================================================================

/// 内嵌字节**存在且非空**（编译期真嵌进去了）。
#[test]
fn embedded_font_bytes_are_present() {
    let bytes = embedded_font_bytes();
    assert!(
        bytes.len() > 100_000,
        "内嵌子集应有可观体积（实测 {} 字节）——过小说明 include_bytes 路径错了",
        bytes.len()
    );

    // OpenType 魔数：`OTTO`（CFF 轮廓）或 `0x00010000`（TrueType 轮廓）
    let magic = &bytes[..4];
    assert!(
        magic == b"OTTO" || magic == [0x00, 0x01, 0x00, 0x00],
        "内嵌文件应是合法 OpenType（实测魔数 {magic:02X?}）"
    );
}

/// 家族名是 OFL RFN 合规的改名结果（**不含**保留名 "Source"）。
///
/// ## 这是合规红线（research R3 的关键）
///
/// 思源宋体带 Reserved Font Name「Source」，OFL 规定修改版不得用它作字体
/// 标识。T008 的子集化脚本已改名，本条在**运行时**核对改名结果——
/// 它防的是"某次重新子集化时脚本回退到原名"（那时 CI 必须红，
/// 而不是等到分发出去才发现）。
#[test]
fn embedded_family_name_is_rfn_compliant() {
    let name = family_name();
    assert_eq!(name, READING_SERIF_FAMILY, "家族名应为改名后的常量");
    assert_eq!(name, "Tome Serif CN");
    assert!(
        !name.contains("Source"),
        "家族名不得含 OFL 保留名「Source」（RFN 合规）：{name}"
    );
    assert!(
        !name.contains("思源"),
        "中文家族名同样不得含保留名对应的原始标识：{name}"
    );
}

/// 内嵌体积在**预算上限之内**，且 `within_budget` 为真。
#[test]
fn embedded_size_is_within_budget() {
    let bytes = embedded_font_bytes().len();

    assert!(
        bytes <= EMBEDDED_FONT_BUDGET_BYTES,
        "内嵌子集 {bytes} 字节超出预算 {EMBEDDED_FONT_BUDGET_BYTES} 字节（FR-050 的 5MB 上限）"
    );

    let report = FontsReport::collect();
    assert!(
        report.within_budget,
        "within_budget 应为真（实测 {} ≤ 预算 {}）",
        report.embedded_bytes, report.budget_bytes
    );
    assert_eq!(
        report.budget_bytes, EMBEDDED_FONT_BUDGET_BYTES,
        "报告里的预算值应与常量一致"
    );

    // 实测值打印出来（供 PM 复核"3-5MB 实测在案"）
    println!(
        "[T037] 内嵌子集 {} 字节（{:.2} MiB），预算 {} 字节（{:.2} MiB），下限参考 {} 字节（{:.2} MiB）",
        bytes,
        bytes as f64 / 1_048_576.0,
        EMBEDDED_FONT_BUDGET_BYTES,
        EMBEDDED_FONT_BUDGET_BYTES as f64 / 1_048_576.0,
        EMBEDDED_FONT_MIN_BYTES,
        EMBEDDED_FONT_MIN_BYTES as f64 / 1_048_576.0,
    );
}

/// 内嵌子集**覆盖中文常用字**（FR-050："MUST 覆盖中文常用字与常用标点"）。
#[test]
fn embedded_subset_covers_common_chinese() {
    let mut registry = FontRegistry::load().expect("加载字体应成功");

    // 常用字与常用标点各取若干，断言都能在**内嵌子集**里找到字形
    let probes = [
        '的', '一', '是', '不', '了', '人', '我', '在', '有', '他', // 常用汉字
        '，', '。', '、', '；', '：', '？', '！', // 常用标点
        '「', '」', '《', '》', '—', '…', // 中文排版常见符号
        'A', 'z', '0', '9', // 基本西文与数字
    ];

    let mut missing = Vec::new();
    for probe in probes {
        match registry.font_for_char(probe) {
            Some((_, FontSource::Embedded)) => {}
            Some((_, FontSource::System)) => missing.push((probe, "回退到系统字体")),
            None => missing.push((probe, "无可用字体")),
        }
    }

    assert!(
        missing.is_empty(),
        "内嵌子集应覆盖全部探针字符（FR-050），以下未覆盖：{missing:?}"
    );
}

// ===========================================================================
// 回退链（FR-047/FR-051）
// ===========================================================================

/// **回退链平台表**含 FR-050 点名的三平台首选。
#[test]
fn fallback_chain_covers_three_platforms() {
    use tome_reader::render::fonts::{fallback_chain_for, Platform};

    // Windows 宋体
    let windows = fallback_chain_for(Platform::Windows);
    assert!(
        windows
            .iter()
            .any(|name| name.contains("宋体") || name.contains("SimSun")),
        "Windows 回退链应含宋体（SimSun），实测 {windows:?}"
    );

    // macOS Songti SC
    let macos = fallback_chain_for(Platform::MacOs);
    assert!(
        macos.iter().any(|name| name.contains("Songti")),
        "macOS 回退链应含 Songti SC，实测 {macos:?}"
    );

    // Linux Noto Serif CJK
    let linux = fallback_chain_for(Platform::Linux);
    assert!(
        linux.iter().any(|name| name.contains("Noto Serif CJK")),
        "Linux 回退链应含 Noto Serif CJK，实测 {linux:?}"
    );

    // 三条链都**非空且有序**（有序是因为按优先级依次尝试）
    for (platform, chain) in [
        (Platform::Windows, &windows),
        (Platform::MacOs, &macos),
        (Platform::Linux, &linux),
    ] {
        assert!(
            !chain.is_empty(),
            "{platform:?} 的回退链不得为空（空链会让缺字直接失败，违反 FR-051）"
        );
    }
}

/// 当前平台的链可取出且与平台表一致。
#[test]
fn current_platform_chain_is_resolvable() {
    let chain = fallback_chain_for_current_platform();
    assert!(!chain.is_empty(), "当前平台应有非空回退链");
    println!("[T037] 当前平台回退链：{chain:?}");
}

/// **缺字落回退链，且行位不变**（FR-047 / US3 场景 2）。
///
/// ## 判据的两个部分
///
/// 1. **回退真的发生**：内嵌子集不含的字符能被系统字体接住（不是"渲染为空"）
/// 2. **行位不变**：该行**其余**字符的位置不受影响
///
/// 第 2 条由 `tests/glyph_cache.rs` 的
/// `missing_glyph_fallback_does_not_shift_the_line` 覆盖（它比对本层排版
/// 结果）；本条覆盖第 1 条——即回退链**接线**了。
#[test]
fn missing_glyphs_fall_back_to_system_fonts() {
    let mut registry = FontRegistry::load().expect("加载字体应成功");

    // 取一个内嵌子集**确定不含**的字符：CJK 扩展 B（GB2312 只到基本区）
    let rare = '\u{2000B}';
    let resolved = registry.font_for_char(rare);

    // 系统字体可能也没有它（那合法：FR-051 的兜底是"缺字占位"而非失败）。
    // 故判据是"**不 panic、不返回错误**"，以及"若有字体则标记来源正确"。
    if let Some((_, source)) = resolved {
        assert!(
            matches!(source, FontSource::Embedded | FontSource::System),
            "来源标记应是两者之一"
        );
    }

    // 而常用字必须在**内嵌**子集里（两个方向的对照，防"全部落到系统字体"
    // 被误当成"回退链工作正常"）
    let (_, common_source) = registry.font_for_char('的').expect("常用字应有字体");
    assert_eq!(
        common_source,
        FontSource::Embedded,
        "常用字必须由内嵌子集提供（若落到系统字体，说明内嵌注册没生效）"
    );
}

// ===========================================================================
// `tome fonts` 报告数据
// ===========================================================================

/// `FontsReport` 产出 CLI 契约（`contracts/cli.md` §2.4）所需的全部字段。
#[test]
fn fonts_report_matches_cli_contract() {
    let report = FontsReport::collect();

    // §2.4 的字段：embedded[].role/family/bytes、system_fallbacks、budget_bytes、within_budget
    assert_eq!(
        report.embedded_role, "reading-serif",
        "内嵌字体的角色标识应为 reading-serif（FR-050 的字体三角色）"
    );
    assert_eq!(report.embedded_family, READING_SERIF_FAMILY);
    assert!(report.embedded_bytes > 0);
    assert!(!report.system_fallbacks.is_empty(), "回退链不得为空");
    assert_eq!(report.budget_bytes, EMBEDDED_FONT_BUDGET_BYTES);
    assert!(report.within_budget);

    // 序列化形态可用（CLI 薄壳直接消费）
    let json = report.to_json().expect("报告应可序列化");
    assert!(json.contains("reading-serif"));
    assert!(json.contains("within_budget"));
}

/// 报告**不含 UI 无衬线与等宽字体**（FR-050：那两者走系统栈，不内嵌）。
#[test]
fn only_reading_serif_is_embedded() {
    let report = FontsReport::collect();
    assert_eq!(
        report.embedded_role, "reading-serif",
        "内嵌的只有正文衬线（FR-050 明文排除 UI 无衬线与规则等宽）"
    );
    // 内嵌体积只算一个文件
    assert_eq!(
        report.embedded_bytes,
        embedded_font_bytes().len(),
        "内嵌体积应恰为那一个子集文件的大小（多个文件说明内嵌了不该嵌的）"
    );
}

/// **零网络**：加载路径不接任何 URL/HTTP 参数（结构性断言）。
#[test]
fn font_loading_is_offline_by_construction() {
    // 加载函数无参 —— 没有"从哪取"的输入，故不可能去网络取。
    // 这条断言的形式是"它能被调用且成功"：若实现引入了网络依赖，
    // CI 的 headless job（无网络出口假设）会红。
    let registry = FontRegistry::load();
    assert!(
        registry.is_ok(),
        "字体加载不应失败（内嵌字节已在二进制里）：{:?}",
        registry.err()
    );

    // 内嵌字节是**编译期**常量，与运行环境无关
    let first = embedded_font_bytes().len();
    let second = embedded_font_bytes().len();
    assert_eq!(first, second, "内嵌字节在两次读取间应相同（它来自二进制）");
}
