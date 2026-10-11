//! `[DONE]` 中西文混排（T014 / FR-009）
//!
//! ## FR-009 的两条要求
//!
//! 1. 中文与西文/数字交界处 MUST 按清单定义处理**最小间隙**
//! 2. 西文单词 MUST **优先整词断行**；整词放不下**且单词长于整行**时
//!    MUST 退化为**按字符断行**
//!
//! ## 为什么第 2 条要自己实现（T005 spike 的关键发现）
//!
//! 实测（spikes/s2/BASELINE.md §2.4）：
//!
//! | `Wrap` | 268px 长西文词 @ 60px 版心 |
//! |---|---|
//! | `Word` | **溢出成 268.2px 的行**（不切词） |
//! | `WordOrGlyph` | 7 行，退化为按字形断行 |
//! | `Glyph` | 6 行，无条件按字形断行 |
//!
//! `WordOrGlyph` 看似"正好是我要的"，但它是**同一次调用内的隐式切换**——
//! 本层控制不了切换点，而且它一旦切到 glyph 路径就**无视 word 边界**，
//! 使 T013 的禁则合并手段失效。故：
//!
//! - 引擎固定用 `Wrap::Word`（保留禁则手段的效力）
//! - **超长单词的退化由本模块自行判定并处理**（把超长词拆成多个 word，
//!   让 `Wrap::Word` 自然把它们分到不同行）
//!
//! ## 本模块与 squeeze 的分工
//!
//! | 模块 | 处理的"放不下" | 手段 |
//! |---|---|---|
//! | [`super::squeeze`] | 差一点（< 可压缩量） | 改写标点字身 |
//! | 本模块 | 整词放不下且词长于整行 | 拆词（退化） |
//!
//! 两者在 T015 的硬序里是**第 ① 步与第 ③ 步**的关系。

/// 中西文交界处的**最小间隙**（em 比例）。
///
/// ## 取值依据
///
/// CLreq §3.1.4「中西文混排间隙」：中文与西文之间应有约 1/4 字身的间隙，
/// 使两种文字的视觉密度过渡自然。全角标点已自带间隙（标点本身占满字身，
/// 其字面留白即是间隙），故本常量只用于**汉字与拉丁字母/数字直接相邻**
/// 的情形。
///
/// 取 **0.25em**：19px 字号下约 4.75px，与 MASTER §4 的 4px 间距网格
/// 同量级，观感上"看得出分隔但不显得断裂"。
pub const MIXED_SCRIPT_MIN_GAP_EM: f32 = 0.25;

/// 字符的书写系统归类（只区分本模块关心的两类）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptClass {
    /// 汉字与中文标点
    Cjk,
    /// 拉丁字母与数字
    Latin,
    /// 其他（空白、换行、emoji、西文标点等）
    Other,
}

/// 判定一个字符的书写系统类别。
///
/// ## 判据的边界（刻意保守）
///
/// - **CJK**：CJK 统一表意文字（基本区与扩展 A）+ 中日韩标点 + 全角形式
/// - **Latin**：ASCII 字母与数字 + 拉丁扩展基本区
/// - **Other**：其余（含空格——空格本身已是间隙，不需要再加）
///
/// 刻意**不**把 emoji 与西文标点算进任一类：它们与中文相邻时加间隙
/// 未必好看（emoji 本身宽度已足够视觉分隔）。保守判据的代价是"某些
/// 情形没加间隙"，而激进判据的代价是"加多了显得松散"——后者更明显。
#[must_use]
pub fn classify(ch: char) -> ScriptClass {
    let code = ch as u32;
    let is_cjk = matches!(code,
        0x3000..=0x303F   // CJK 标点
        | 0x4E00..=0x9FFF // CJK 统一表意文字
        | 0x3400..=0x4DBF // 扩展 A
        | 0xF900..=0xFAFF // 兼容表意文字
        | 0xFF00..=0xFFEF // 全角形式
    );
    if is_cjk {
        return ScriptClass::Cjk;
    }

    let is_latin = ch.is_ascii_alphanumeric() || matches!(code, 0x00C0..=0x024F);
    if is_latin {
        return ScriptClass::Latin;
    }

    ScriptClass::Other
}

/// 两个相邻字符之间是否需要**插入最小间隙**。
///
/// ## 判据
///
/// 一侧是 CJK、另一侧是 Latin 时需要间隙。**两侧同类的组合不需要**：
/// "汉字汉字"之间是自然的字身排列、"abc def"之间是西文的词距。
#[must_use]
pub fn needs_minimum_gap(left: char, right: char) -> bool {
    matches!(
        (classify(left), classify(right)),
        (ScriptClass::Cjk, ScriptClass::Latin) | (ScriptClass::Latin, ScriptClass::Cjk)
    )
}

/// 间隙宽度（px）。
#[must_use]
pub fn minimum_gap_px(font_size_px: f32) -> f32 {
    font_size_px * MIXED_SCRIPT_MIN_GAP_EM
}

/// 一个需要退化的超长单词（字符区间）。
///
/// 不派生 `Eq`——含 `f32` 估算宽度（浮点没有全序，`Eq` 会撒谎）。
#[derive(Debug, Clone, PartialEq)]
pub struct OverlongWord {
    /// 该词在**段内**的字符区间
    pub char_range: std::ops::Range<usize>,
    /// 它有多长（字符数）
    pub char_len: usize,
    /// 它按当前字号的估算宽度（px）
    pub estimated_width_px: f32,
}

impl OverlongWord {
    /// 是否真的"长于整行"（FR-009 的退化条件）。
    #[must_use]
    pub fn exceeds_measure(&self, measure_px: f32) -> bool {
        self.estimated_width_px > measure_px
    }
}

/// 找出文本中**长于整行**的西文单词（FR-009 的退化候选）。
///
/// ## 为什么需要"预先找出"而不是让 cosmic-text 处理
///
/// 见模块文档：`Wrap::Word` 对超宽词是**溢出**而非切分，故本层必须
/// 自己发现这些词并**主动拆开**（把长词切成多个 word 交给 `Wrap::Word`）。
///
/// ## 宽度估算
///
/// 用**平均字宽**估算（`font_size_px × LATIN_AVERAGE_WIDTH_RATIO`）。
/// 精确宽度需要真整形（那正是要避免的开销——退化候选是"粗筛"，
/// 筛出来后再精确处理）。粗筛的方向是**宁多勿漏**：估算偏大一点，
/// 让边界情形也进候选，由后续的精确判断淘汰。
#[must_use]
pub fn find_overlong_words(text: &str, measure_px: f32, font_size_px: f32) -> Vec<OverlongWord> {
    /// 拉丁字母的平均字宽（相对字号）。
    ///
    /// 取自常见西文衬线体的经验值：小写字母平均约 0.5em。用它做**粗筛**，
    /// 故取偏大值（0.6）以免漏掉边界情形。
    const LATIN_AVERAGE_WIDTH_RATIO: f32 = 0.6;

    let mut found = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if classify(chars[index]) != ScriptClass::Latin {
            index += 1;
            continue;
        }
        let word_start = index;
        while index < chars.len() && classify(chars[index]) == ScriptClass::Latin {
            index += 1;
        }
        let word_end = index;

        let char_len = word_end - word_start;
        let estimated_width_px = char_len as f32 * font_size_px * LATIN_AVERAGE_WIDTH_RATIO;
        if estimated_width_px > measure_px {
            found.push(OverlongWord {
                char_range: word_start..word_end,
                char_len,
                estimated_width_px,
            });
        }
    }

    found
}

/// 把一个超长单词**按最大可用字符数**切成若干片段（FR-009 的退化实现）。
///
/// ## 返回
///
/// 片段的字符区间（相对段内偏移）。它们**不重叠、不留空隙、覆盖原词**——
/// 这是"退化不得丢字"的判据。
///
/// ## 切分粒度
///
/// `chars_per_line` 由调用方按 `版心宽 ÷ 单字符宽度` 给出。取 1 作为下限
/// ——`chars_per_line = 0`（版心窄到放不下任何字符）时退化为逐字符切分，
/// 那是唯一合理的处置（此时无论怎么切都会溢出，逐字符至少保证不丢字）。
#[must_use]
pub fn split_overlong_word(
    word: &OverlongWord,
    chars_per_line: usize,
) -> Vec<std::ops::Range<usize>> {
    let chunk = chars_per_line.max(1);
    let mut pieces = Vec::new();
    let mut start = word.char_range.start;

    while start < word.char_range.end {
        let end = (start + chunk).min(word.char_range.end);
        pieces.push(start..end);
        start = end;
    }

    pieces
}

/// 按单字符宽度估算**每行可容纳的字符数**。
///
/// 用于 [`split_overlong_word`] 的粒度。同样用偏大的字宽（宁少勿溢）。
#[must_use]
pub fn chars_per_line_for_measure(measure_px: f32, font_size_px: f32) -> usize {
    if font_size_px <= 0.0 {
        return 1;
    }
    // 0.6 与 find_overlong_words 的粗筛比例一致（同一口径）
    let width_per_char = font_size_px * 0.6;
    let raw = measure_px / width_per_char;
    // `+ 1e-4` 吸收 f32 的表示误差：`342 / (19 × 0.6)` 在实数上是 30.0，
    // 但 f32 算出 29.999998，`floor()` 会给出 29——差一个字符。
    // 这是**取整方向**问题：它只影响"每行能放几个"，但会让退化粒度
    // 无谓地小一档（多切一刀）。加一个远小于 1 的 epsilon 即可对齐
    // 数学期望，且不会把 29.9 错误地提升为 30（那需要误差 ≥ 0.1）。
    (raw + 1e-4).floor().max(1.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_cjk_latin_and_other() {
        assert_eq!(classify('甲'), ScriptClass::Cjk);
        assert_eq!(classify('。'), ScriptClass::Cjk);
        assert_eq!(classify('，'), ScriptClass::Cjk);
        assert_eq!(classify('a'), ScriptClass::Latin);
        assert_eq!(classify('Z'), ScriptClass::Latin);
        assert_eq!(classify('9'), ScriptClass::Latin);
        assert_eq!(classify(' '), ScriptClass::Other);
        assert_eq!(classify('!'), ScriptClass::Other, "西文标点不算 Latin");
        assert_eq!(classify('😀'), ScriptClass::Other);
    }

    /// 中西文交界需要间隙；同类相邻不需要。
    #[test]
    fn gap_only_at_script_boundary() {
        assert!(needs_minimum_gap('甲', 'a'), "汉字后接西文");
        assert!(needs_minimum_gap('a', '甲'), "西文后接汉字");
        assert!(needs_minimum_gap('甲', '9'), "汉字后接数字");

        assert!(!needs_minimum_gap('甲', '乙'), "汉字之间");
        assert!(!needs_minimum_gap('a', 'b'), "西文之间");
        assert!(!needs_minimum_gap('甲', '，'), "汉字与中文标点");
        assert!(!needs_minimum_gap('甲', ' '), "空格本身已是间隙");
    }

    #[test]
    fn gap_width_scales_with_font_size() {
        assert!((minimum_gap_px(19.0) - 4.75).abs() < 1e-4);
        assert!((minimum_gap_px(24.0) - 6.0).abs() < 1e-4);
        assert_eq!(minimum_gap_px(0.0), 0.0);
    }

    /// 找出长于整行的西文词。
    #[test]
    fn finds_overlong_words() {
        let text = "这是一个 pneumonoultramicroscopicsilicovolcanoconiosis 的词";
        let found = find_overlong_words(text, 228.0, 19.0);

        assert_eq!(found.len(), 1, "应找到 1 个超长词：{found:?}");
        assert_eq!(found[0].char_len, 45);
        assert!(
            found[0].exceeds_measure(228.0),
            "该词应长于 228px 版心（估算 {}px）",
            found[0].estimated_width_px
        );
    }

    /// 普通长度的西文词不被误判（反例）。
    #[test]
    fn short_words_are_not_overlong() {
        let text = "这本书的英文名是 The Shadow of the Wind。";
        let found = find_overlong_words(text, 342.0, 19.0);
        assert!(found.is_empty(), "普通词不该被判为超长：{found:?}");
    }

    /// 中文不被当作西文词（反例）。
    #[test]
    fn cjk_is_not_treated_as_latin_word() {
        let found = find_overlong_words("很长的一段中文字符序列没有任何西文", 100.0, 19.0);
        assert!(found.is_empty(), "中文不该进西文词的候选：{found:?}");
    }

    /// 拆词：片段覆盖原词、不重叠、不留空隙（FR-009 的退化不得丢字）。
    #[test]
    fn split_covers_word_without_gaps_or_overlap() {
        let word = OverlongWord {
            char_range: 10..55,
            char_len: 45,
            estimated_width_px: 513.0,
        };
        let pieces = split_overlong_word(&word, 7);

        assert_eq!(pieces.first().unwrap().start, 10, "首片段起点应为词首");
        assert_eq!(pieces.last().unwrap().end, 55, "末片段终点应为词尾");

        for window in pieces.windows(2) {
            assert_eq!(
                window[0].end, window[1].start,
                "相邻片段必须首尾相接（无空隙无重叠）：{pieces:?}"
            );
        }

        let total: usize = pieces.iter().map(|piece| piece.end - piece.start).sum();
        assert_eq!(total, word.char_len, "片段总长应等于原词长度");
    }

    /// 拆词粒度：每片段不超过给定字符数。
    #[test]
    fn split_respects_chunk_size() {
        let word = OverlongWord {
            char_range: 0..45,
            char_len: 45,
            estimated_width_px: 513.0,
        };
        for chunk in [1usize, 3, 7, 20, 45] {
            let pieces = split_overlong_word(&word, chunk);
            for piece in &pieces {
                assert!(
                    piece.end - piece.start <= chunk,
                    "chunk={chunk} 时片段 {:?} 超长",
                    piece
                );
            }
        }
    }

    /// 单片段情形（词长恰等于粒度）。
    #[test]
    fn split_yields_single_piece_when_word_fits_chunk() {
        let word = OverlongWord {
            char_range: 5..10,
            char_len: 5,
            estimated_width_px: 57.0,
        };
        let pieces = split_overlong_word(&word, 5);
        assert_eq!(pieces, vec![5..10]);
    }

    /// 退化粒度不得为 0（版心极窄时至少逐字符）。
    #[test]
    fn chunk_size_never_zero() {
        let word = OverlongWord {
            char_range: 0..3,
            char_len: 3,
            estimated_width_px: 100.0,
        };
        let pieces = split_overlong_word(&word, 0);
        assert_eq!(pieces.len(), 3, "粒度为 0 时应退化为逐字符");
    }

    /// 每行字符数估算：口径与浮点取整方向（实测踩到过）。
    ///
    /// `342 / (19 × 0.6)` 数学上是 30.0，但 f32 算出 29.999998——
    /// 不加 epsilon 会让 `floor()` 给出 29，退化粒度无谓地小一档。
    #[test]
    fn chars_per_line_estimation() {
        assert_eq!(chars_per_line_for_measure(342.0, 19.0), 30);
        // 非整除的情形按 floor 取整（多出的余量不足以再放一个字符）
        assert_eq!(
            chars_per_line_for_measure(228.0, 19.0),
            20,
            "228 / 11.4 = 20.0"
        );
        assert_eq!(
            chars_per_line_for_measure(230.0, 19.0),
            20,
            "230 / 11.4 = 20.18 → floor 20"
        );
        // 版心极窄时至少 1
        assert_eq!(chars_per_line_for_measure(1.0, 19.0), 1);
        // 字号为 0 时不除零
        assert_eq!(chars_per_line_for_measure(342.0, 0.0), 1);
    }
}
