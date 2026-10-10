//! T010 验收：规范化文本模型（FR-003）
//!
//! ## 这一层要保证什么
//!
//! **对外位置一律字符下标**（Unicode 标量值序列下标），与 001 的
//! `ReadProgress.char_offset` 同口径。这不是实现细节，而是跨子项目的契约：
//! S1 把进度与痕迹存成字符偏移，S2 若用字节偏移表达任何位置，改字号后
//! 的定位就会整体偏移（中文下字节数是字符数的 3 倍）。
//!
//! 故本文件的测试分两类：
//!
//! 1. **行为断言**：字形簇不在簇内断开、下标换算正确
//! 2. **签名审查**：公开 API 上不得出现字节偏移/UTF-16 码元偏移——
//!    这类"不该有的东西"只能靠扫描 API 面来断言，运行时测不到

use tome_reader::text::model::{CharIndex, TextModel, TextPosition};
use tome_reader::text::paragraph::Paragraph;

// ---------------------------------------------------------------------------
// 字符下标口径
// ---------------------------------------------------------------------------

/// 字符下标 = **Unicode 标量值**序列下标（不是字节、不是 UTF-16 码元）。
///
/// ## 为什么这组用例能区分三种口径
///
/// 用 BMP 内的汉字无法区分（三者一致），必须用**代理对**与**多字节**字符：
///
/// | 字符 | 码点 | UTF-8 字节 | UTF-16 码元 | 字符 |
/// |---|---|---|---|---|
/// | `中` | U+4E2D | 3 | 1 | 1 |
/// | `😀` | U+1F600 | 4 | **2** | **1** |
/// | `é`（e+U+0301） | 2 个码点 | 3 | 2 | **1 字形簇 / 2 字符** |
#[test]
fn char_index_counts_unicode_scalar_values() {
    // "中😀文"：3 个字符、10 字节、4 个 UTF-16 码元
    let model = TextModel::new("中😀文");

    assert_eq!(model.char_len(), 3, "字符数应为 Unicode 标量值个数");
    assert_eq!(model.byte_len(), 10, "字节数（中文 3 + emoji 4 + 中文 3）");
    assert_eq!(
        model.utf16_len(),
        4,
        "UTF-16 码元数（1 + 2 + 1）——emoji 占两个码元，正是要区分的点"
    );
}

/// 三种长度度量在纯 BMP 文本上一致，在含代理对时分离（基准对照）。
#[test]
fn three_length_measures_diverge_only_with_astral_chars() {
    let bmp_only = TextModel::new("中文测试");
    assert_eq!(bmp_only.char_len(), 4);
    assert_eq!(bmp_only.utf16_len(), 4, "BMP 内三者一致");
    assert_eq!(bmp_only.byte_len(), 12);

    let with_astral = TextModel::new("中😀");
    assert_eq!(with_astral.char_len(), 2);
    assert_ne!(
        with_astral.char_len(),
        with_astral.utf16_len(),
        "含代理对时字符数与 UTF-16 码元数必须分离（否则口径可能被搞混）"
    );
}

// ---------------------------------------------------------------------------
// 字形簇边界（FR-003 + AGENTS.md：簇内不得断开）
// ---------------------------------------------------------------------------

/// **组合字符不拆簇**：`é` 写成 `e` + U+0301 时是一个字形簇、两个字符。
///
/// 断行若落在 `e` 与 U+0301 之间，重音符号会跑到下一行开头——那是明显的
/// 排版错误，也是"字符下标"与"字形簇"两个概念的分离点。
#[test]
fn combining_marks_stay_in_one_grapheme_cluster() {
    let model = TextModel::new("e\u{0301}xyz"); // é(组合) + xyz

    assert_eq!(model.char_len(), 5, "字符数：e, ́, x, y, z");
    assert_eq!(model.cluster_count(), 4, "字形簇数：é, x, y, z");

    let clusters = model.clusters();
    assert_eq!(
        clusters[0].char_range,
        0..2,
        "首个簇覆盖字符 0..2（e 与其组合重音）"
    );
    assert_eq!(clusters[0].text, "e\u{0301}");

    // 簇内不得是可断点
    assert!(
        !model.is_breakable(1),
        "字符 1 处（e 与组合重音之间）不得断行"
    );
    assert!(model.is_breakable(2), "字符 2 处（é 与 x 之间）可以断行");
}

/// **变体选择符不拆簇**：`❤️` = U+2764 + U+FE0F，一个簇两个字符。
#[test]
fn variation_selectors_stay_in_one_grapheme_cluster() {
    let model = TextModel::new("\u{2764}\u{FE0F}ab"); // ❤️ + a + b

    assert_eq!(model.char_len(), 4);
    assert_eq!(model.cluster_count(), 3, "簇：❤️, a, b");
    assert_eq!(model.clusters()[0].char_range, 0..2);
    assert!(!model.is_breakable(1), "变体选择符前不得断");
}

/// **emoji 序列（ZWJ 连接）整体成簇**：`👨‍👩‍👧` 由三个 emoji 用 ZWJ 连接。
///
/// 这是最长的常见簇（家庭 emoji），断在中间会渲染成三个独立人像——
/// 与"一个家庭"的语义不符。
#[test]
fn zwj_emoji_sequences_stay_in_one_cluster() {
    let model = TextModel::new("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}!");

    // 字符数：👨 ZWJ 👩 ZWJ 👧 ! = 6
    assert_eq!(model.char_len(), 6);
    assert_eq!(model.cluster_count(), 2, "簇：整个家庭 emoji, !");
    assert_eq!(model.clusters()[0].char_range, 0..5);

    for inside in 1..5 {
        assert!(
            !model.is_breakable(inside),
            "ZWJ 序列内部（字符 {inside}）不得断行"
        );
    }
    assert!(model.is_breakable(5), "序列末尾与 ! 之间可以断");
}

/// **区域指示符成对成簇**：国旗 emoji `🇨🇳` = 两个区域指示符。
#[test]
fn regional_indicators_pair_into_one_cluster() {
    let model = TextModel::new("\u{1F1E8}\u{1F1F3}x");

    assert_eq!(model.char_len(), 3);
    assert_eq!(model.cluster_count(), 2, "簇：🇨🇳, x");
    assert_eq!(model.clusters()[0].char_range, 0..2);
    assert!(!model.is_breakable(1), "国旗两个指示符之间不得断");
}

/// **汉字逐字成簇**——中文排版的基本粒度（T005 的 spike 也实测过）。
#[test]
fn cjk_chars_are_individual_clusters() {
    let model = TextModel::new("中文测试");

    assert_eq!(model.char_len(), 4);
    assert_eq!(model.cluster_count(), 4, "汉字各自成簇");
    for cluster in model.clusters() {
        assert_eq!(cluster.char_range.end - cluster.char_range.start, 1);
    }
    // 汉字之间处处可断（禁则的约束在 rules 层，不在这层）
    for index in 1..4 {
        assert!(model.is_breakable(index), "汉字 {index} 处应可断");
    }
}

// ---------------------------------------------------------------------------
// 下标换算与区间
// ---------------------------------------------------------------------------

/// 下标换算：字符下标 ↔ 字节偏移（内部用），互逆。
#[test]
fn index_conversions_round_trip() {
    let text = "中😀文";
    let model = TextModel::new(text);

    for char_index in 0..=model.char_len() {
        let byte_offset = model.to_byte_offset(char_index);
        assert_eq!(
            model.to_char_index(byte_offset),
            Some(char_index),
            "字符 {char_index} ↔ 字节 {byte_offset} 往返失败"
        );
    }

    // 已知值（用于确认不是"两边同错"）
    assert_eq!(model.to_byte_offset(0), 0);
    assert_eq!(model.to_byte_offset(1), 3, "「中」占 3 字节");
    assert_eq!(model.to_byte_offset(2), 7, "「😀」再占 4 字节");
    assert_eq!(model.to_byte_offset(3), 10, "「文」再占 3 字节");
}

/// 落在**字符中间**的字节偏移不是合法字符位置 → `None`。
///
/// 这条防的是"把字节偏移当字符偏移用"这类接口误用：若实现静默圆整，
/// 误用会变成一个看不见的偏移错误。
#[test]
fn mid_char_byte_offset_is_rejected() {
    let model = TextModel::new("中😀文");

    // 字节 1、2 落在「中」内部；4、5、6 落在「😀」内部
    for bad in [1, 2, 4, 5, 6, 8, 9] {
        assert_eq!(
            model.to_char_index(bad),
            None,
            "字节 {bad} 落在字符内部，应被拒绝而非圆整"
        );
    }
    // 越界同样拒绝
    assert_eq!(model.to_char_index(11), None);
}

/// 字符区间切片与 `char_range` 一致（区间归属的唯一权威）。
#[test]
fn char_range_slicing_is_consistent() {
    let model = TextModel::new("中文😀测试");
    let all = model.char_range_text(0..model.char_len());
    assert_eq!(all, Some("中文😀测试"));

    assert_eq!(model.char_range_text(0..2), Some("中文"));
    assert_eq!(model.char_range_text(2..3), Some("😀"));
    assert_eq!(model.char_range_text(3..5), Some("测试"));

    // 越界返回 None（不 panic——生产路径不得 panic）
    assert_eq!(model.char_range_text(0..6), None);
    // 反向区间（用变量表达，避免 clippy 把常量空区间当笔误）
    let (start, end) = (3usize, 2usize);
    assert_eq!(model.char_range_text(start..end), None, "反向区间应被拒绝");
}

/// 空文本是合法输入（不应 panic，长度全为 0）。
#[test]
fn empty_text_is_valid() {
    let model = TextModel::new("");
    assert_eq!(model.char_len(), 0);
    assert_eq!(model.byte_len(), 0);
    assert_eq!(model.cluster_count(), 0);
    assert_eq!(model.clusters().len(), 0);
    assert_eq!(model.char_range_text(0..0), Some(""));
}

/// 纯 ASCII：字符数 == 字节数 == 簇数（三种口径重合的基准）。
#[test]
fn ascii_text_has_identical_measures() {
    let model = TextModel::new("hello world");
    assert_eq!(model.char_len(), 11);
    assert_eq!(model.byte_len(), 11);
    assert_eq!(model.utf16_len(), 11);
    assert_eq!(model.cluster_count(), 11);
}

// ---------------------------------------------------------------------------
// FR-003 的签名审查
// ---------------------------------------------------------------------------

/// **签名审查**：公开 API 不得暴露字节偏移或 UTF-16 码元偏移。
///
/// ## 为什么这条只能靠源码扫描
///
/// "公开 API 上有哪些方法"是个**静态事实**，运行期测不到——调用一个不
/// 该存在的方法不会失败，它只是被用上了。故用源码扫描把这条契约定住：
/// `pub fn` 签名里出现 `byte_offset` / `utf16` 之类的**返回类型或参数名**
/// 即判定为潜在违规（`to_byte_offset` 这类**内部换算**是允许的，
/// 它是表达"字符下标"所必需的，但不得有"以字节为输入输出单位"的公开面）。
///
/// 判据是**方法名与签名的词形**，不是关键词黑名单——避免误报注释与文档。
#[test]
fn public_api_does_not_expose_byte_or_utf16_offsets() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/text/model.rs"),
    )
    .expect("应能读到本模块源码");

    // 允许的换算方法（内部口径转换，不把字节当对外单位）
    const ALLOWED: &[&str] = &[
        "to_byte_offset",
        "to_char_index",
        "byte_len",
        "utf16_len",
        "type_byte_offset",
    ];

    let mut offenders = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        // 只看 pub fn 的签名行
        if !trimmed.starts_with("pub fn ") && !trimmed.starts_with("pub const fn ") {
            continue;
        }
        let signature = trimmed;
        let names_offset_in_api = signature.contains("byte_offset")
            || signature.contains("utf16_offset")
            || signature.contains("byte_pos");
        if !names_offset_in_api {
            continue;
        }
        if ALLOWED.iter().any(|allowed| signature.contains(allowed)) {
            continue;
        }
        offenders.push(format!("第 {} 行：{}", index + 1, signature));
    }

    assert!(
        offenders.is_empty(),
        "FR-003 违规：公开 API 出现字节/UTF-16 偏移为对外单位的方法。\n\
         对外位置 MUST 一律是字符下标（与 001 char_offset 同口径）。\n\
         若确为内部换算，请把方法名加入本测试的 ALLOWED 并说明理由。\n{}",
        offenders.join("\n")
    );
}

/// **签名审查的有效性自检**（防扫描器空转——T-005/T-007/T-009 的教训）。
///
/// 合成一段含违规签名的源码，断言扫描逻辑能识别它。
#[test]
fn signature_scanner_detects_violations() {
    let fake_source = "\
    pub fn char_offset(&self) -> usize { 0 }
    pub fn byte_offset_of(&self, index: usize) -> usize { index }
    pub fn to_byte_offset(&self, char_index: usize) -> usize { char_index }
";

    const ALLOWED: &[&str] = &["to_byte_offset", "to_char_index"];
    let mut offenders = Vec::new();
    for line in fake_source.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("pub fn ") && !trimmed.starts_with("pub const fn ") {
            continue;
        }
        let names_offset_in_api = trimmed.contains("byte_offset")
            || trimmed.contains("utf16_offset")
            || trimmed.contains("byte_pos");
        if names_offset_in_api && !ALLOWED.iter().any(|allowed| trimmed.contains(allowed)) {
            offenders.push(trimmed.to_owned());
        }
    }

    assert_eq!(
        offenders.len(),
        1,
        "扫描器应恰好识别出 1 条违规（byte_offset_of），实际：{offenders:?}"
    );
    assert!(offenders[0].contains("byte_offset_of"));
}

// ---------------------------------------------------------------------------
// TextPosition / CharIndex（对外位置类型）
// ---------------------------------------------------------------------------

/// `CharIndex` 与 `TextPosition` 的语义：前者是纯下标，后者带段落归属。
#[test]
fn position_types_carry_expected_fields() {
    let index = CharIndex::new(42);
    assert_eq!(index.value(), 42);

    let position = TextPosition::new(2, 17);
    assert_eq!(position.paragraph_index, 2);
    assert_eq!(position.char_offset, 17);
}

/// `Paragraph` 携带段内文本与其在全文中的**字符**起始位置。
#[test]
fn paragraph_carries_char_offsets() {
    let paragraph = Paragraph::new(0, 0, "第一段");
    assert_eq!(paragraph.index, 0);
    assert_eq!(paragraph.char_start, 0);
    assert_eq!(paragraph.text, "第一段");
    assert_eq!(paragraph.char_len(), 3);

    // 第二段起始 = 第一段长度 + 1（换行符占 1 字符）
    let second = Paragraph::new(1, 4, "第二段");
    assert_eq!(second.char_start, 4);
    assert_eq!(second.char_end(), 7);
}
