//! `[DONE]` 段落切分（T011 / FR-001、FR-002）
//!
//! ## 默认路径：只认换行符（FR-001）
//!
//! 段落边界**只由原文中的换行符确定**（`\n`、`\r\n`、`\r` 视为同一类）。
//! 此时原文里的前导全角/半角空格与制表符 **MUST NOT** 被当作段落标记，
//! 而 MUST 作为段内文本按空格类处理。
//!
//! 这条的直接后果：一份用 `\n` 分段、段首无缩进的正文，切出来的段落数
//! 等于换行符数 + 1（空行产生空段，见下方"空段"的处置）。
//!
//! ## 启发式路径：前导两个全角空格（FR-002，裁决 OQ#2→B）
//!
//! 有些书源把整章正文放在**一个**字符串里，段落之间**只有两个全角空格**
//! 而没有换行符。这类正文若只按换行符切分，会得到"一段超长正文"，
//! 排版结果与原文意图完全不符。
//!
//! 故 FR-002 定义了一个**双条件门槛**的启发式——两个条件**同时**满足才启用：
//!
//! 1. 该章正文中的换行符数量**低于**「段落数阈值」（命名常量）
//! 2. 检出不少于 **3 处**「前导两个全角空格」
//!
//! 门槛的目的（FR-002 原文"避免正常的 `\n` 分段正文被切出空段"）：
//! 条件 ① 排除"本来就用换行分段"的正文，条件 ② 排除"恰好有一处缩进"的
//! 偶然命中。**两个都必须满足**，故单一条件的误判无法触发它。
//!
//! 启用时 MUST 产出一条**可读的诊断记录**（[`Diagnostic`]），说明启发式
//! 已生效——这是"排版结果与预期不同时能查到原因"的前提。

use super::model::TextModel;

/// 判定启发式是否启用的「段落数阈值」（FR-002 条件 ①）。
///
/// ## 取值依据
///
/// 条件 ① 的原文是"换行符数量低于段落数阈值"。本常量即那个阈值。
///
/// 取 **3** 的理由：
///
/// - 一份**真正**用换行分段的正文，段落数远多于 3（一章至少十几段），
///   故它不会被误判为"缺换行"。取 3 留足安全边际。
/// - 若阈值取 1，那么"整章只有 2 个换行"（如引文块拼在一起）也会被
///   判为"缺换行"，而那种正文用换行分段是**有意的**——误判会把它切碎。
/// - 若阈值取 10，那么一份"8 段但只有 7 个换行"的短章会被判为"够用"
///   从而**错过**启发式，即使它显然是空格分段。
///
/// 该常量是**命名常量**（FR-002 明文要求），不是魔数——它的取值理由
/// 记录在此，改动时须重新论证。
pub const PARAGRAPH_COUNT_THRESHOLD: usize = 3;

/// 「前导两个全角空格」的最小检出数（FR-002 条件 ②）。
///
/// 取 **3** 的理由与条件 ① 同构：单处缩进是常见排版细节（如引文首行），
/// 3 处以上才构成"这是分段约定"的统计证据。FR-002 原文明确写了「不少于 3 处」。
pub const LEADING_IDEOGRAPHIC_SPACE_MIN_HITS: usize = 3;

/// 全角空格（U+3000）。
///
/// 中文正文的段首缩进在源文本里以它表达（半角空格在中文排版里另有用处，
/// 故不参与本启发式的判定）。
pub const IDEOGRAPHIC_SPACE: char = '\u{3000}';

/// 切分模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitMode {
    /// 默认：只认换行符（FR-001）
    NewlineOnly,
    /// 启发式启用：前导两个全角空格也是段落边界（FR-002）
    LeadingIdeographicSpace,
}

/// 启发式判定的诊断记录（FR-002 要求"可读的诊断记录"）。
///
/// ## 为什么是结构而非日志字符串
///
/// "输出一行日志"在无界面环境（FR-052）里无处可看，也无法被测试断言。
/// 结构化的诊断可以被：
/// - CLI 序列化进 `layout` 的输出（`contracts/cli.md` 的 JSON）
/// - 测试逐字段断言（`split_mode == NewlineOnly` 表示**未**启用）
/// - 上层展示为"为什么这段正文排版结果和预期不同"
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 实际采用的切分模式
    pub split_mode: SplitMode,
    /// 检出的换行符数量（条件 ① 的输入）
    pub newline_count: usize,
    /// 检出的「前导两个全角空格」数量（条件 ② 的输入）
    pub leading_space_hits: usize,
    /// 是否因启发式而改变了切分行为
    pub heuristic_applied: bool,
}

impl Diagnostic {
    /// 供 CLI / 日志的一行摘要（**标识式**，不含面向用户的文案）。
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "split_mode={:?} newlines={} leading_ideographic_space_hits={} applied={}",
            self.split_mode, self.newline_count, self.leading_space_hits, self.heuristic_applied
        )
    }
}

/// 一个自然段。
///
/// `char_start` 是它在**全文中的字符起始位置**（不是段内偏移）——
/// 这样"段内偏移 + 段起始"即可换算出全文位置，与 001 的锚点口径一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paragraph {
    /// 段序号（0 起）
    pub index: usize,
    /// 全文中的字符起始位置
    pub char_start: usize,
    /// 段文本（**不含**段落分隔符本身）
    pub text: String,
    /// 该段是否由启发式（前导全角空格）切出
    ///
    /// 启用启发式时，切出的段文本会**去掉**那两格缩进（它们是标记而非内容）。
    /// 故"段文本 + 缩进"才等于原文——上层若要还原原文须看本字段。
    pub from_heuristic: bool,
}

impl Paragraph {
    /// 构造。
    #[must_use]
    pub fn new(index: usize, char_start: usize, text: impl Into<String>) -> Self {
        Self {
            index,
            char_start,
            text: text.into(),
            from_heuristic: false,
        }
    }

    /// 段内字符数。
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.text.chars().count()
    }

    /// 全文中的字符结束位置（**不含**分隔符）。
    #[must_use]
    pub fn char_end(&self) -> usize {
        self.char_start + self.char_len()
    }

    /// 文本模型（排版输入）。
    #[must_use]
    pub fn model(&self) -> TextModel {
        TextModel::new(self.text.clone())
    }
}

/// 段落切分的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitResult {
    /// 段落序列
    pub paragraphs: Vec<Paragraph>,
    /// 判定诊断
    pub diagnostic: Diagnostic,
}

/// 按 FR-001/FR-002 切分段落。
///
/// ## 实现步骤（与需求逐条对应）
///
/// 1. 统计换行符数量与「前导两个全角空格」出现数（供诊断与门槛判定）
/// 2. 双条件门槛：两者**同时**满足才启用启发式（否则默认路径）
/// 3. 按选定模式切分
///
/// ## 换行符的统一处理（FR-001）
///
/// `\r\n` 视为**一个**边界（不是两个）。实现上先做归一化：把 `\r\n` 与
/// 单独的 `\r` 都替换为 `\n`。替换会改变**字节**长度但不改变**字符**数
/// （`\r\n` 两个字符 → `\n` 一个字符会改变字符数！）——故归一化在**切分内部**
/// 进行，而对外报告的 `char_start` 以**原文**为准（见下方实现）。
#[must_use]
pub fn split(text: &str) -> SplitResult {
    // --- 1. 统计（都基于归一化后的视角）---
    let newline_count = count_newlines(text);
    let leading_space_hits = count_leading_ideographic_space_pairs(text);

    // --- 2. 双条件门槛 ---
    let heuristic_applied = newline_count < PARAGRAPH_COUNT_THRESHOLD
        && leading_space_hits >= LEADING_IDEOGRAPHIC_SPACE_MIN_HITS;

    let split_mode = if heuristic_applied {
        SplitMode::LeadingIdeographicSpace
    } else {
        SplitMode::NewlineOnly
    };

    let diagnostic = Diagnostic {
        split_mode,
        newline_count,
        leading_space_hits,
        heuristic_applied,
    };

    let paragraphs = if heuristic_applied {
        split_by_ideographic_space(text)
    } else {
        split_by_newline(text)
    };

    SplitResult {
        paragraphs,
        diagnostic,
    }
}

/// 统计换行符数量（`\r\n` 算**一个**）。
fn count_newlines(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut count = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                // `\r\n` 算一个边界，故跳过紧随的 `\n`
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
                count += 1;
            }
            b'\n' => count += 1,
            _ => {}
        }
        index += 1;
    }
    count
}

/// 统计「两个全角空格」在**段标记位置**的出现次数。
///
/// ## 什么算"段标记位置"（本启发式最容易做错的地方）
///
/// FR-002 的用例是"整章正文放在**一个**字符串里，段落之间只有两个全角空格"。
/// 故标记出现在**每一段的起始处**，段之间就是段内容。
///
/// **不能按"行首"筛**：整章无换行时就只有第一处算"行首"，第 2..N 段的标记
/// 全在同一行里。实测这正是初版的 bug——4 处缩进的文本只数出 1 处，
/// 导致双条件门槛的条件 ② 永远不满足、启发式从不启用。
///
/// ## 为什么也不取"任意位置的两个连续全角空格"
///
/// 段中可能刻意留全角空格（如"第 一 章"的字距）。但本启发式的判据不靠
/// 位置筛，而靠**交替模式**：标记 → 段内容 → 标记 → 段内容……
/// 计数即"标记出现次数"，与 [`split_by_ideographic_space`] 的切分行为
/// 严格一致（同样的交替扫描）。
///
/// ## 三个连续全角空格只算一次
///
/// 扫描到一对即计数并**跳过这一对**，第三个归入下一轮的"标记"判定。
/// 若它后面还有一个全角空格，那就是第二对（标记+标记=空段），
/// 与切分逻辑一致。
fn count_leading_ideographic_space_pairs(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut count = 0usize;
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index] == IDEOGRAPHIC_SPACE && chars.get(index + 1) == Some(&IDEOGRAPHIC_SPACE) {
            count += 1;
            index += 2;
            continue;
        }
        index += 1;
    }

    count
}

/// 默认路径：只按换行符切分（FR-001）。
///
/// ## 空段的处置
///
/// `a\n\nb` 会产生一个空段。FR-001 说边界"只由换行符确定"，故空段是
/// **合法的**切分结果（原文确实有两处连续边界）。但**全空的段落**对排版
/// 无意义（它产出一个零高度的行盒），故此处丢弃 `text` 为空的段——
/// 同时**保留**只含空白的段（如全角空格行，那可能是有意的空行排版）。
fn split_by_newline(text: &str) -> Vec<Paragraph> {
    let normalized = normalize_newlines(text);
    let mut paragraphs = Vec::new();

    // 逐字符扫描，同时维护"原文中的字符位置"。
    //
    // 归一化会改变字符数（`\r\n` 两字符 → `\n` 一字符），故不能用归一化后的
    // 下标当 `char_start`——那会让后续的锚点换算整体错位。此处以**原文**
    // 的字符游标为准，归一化只用来判断"这里是不是边界"。
    let mut current = String::new();
    let mut segment_start_char = 0usize;
    let mut char_cursor = 0usize;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        let is_boundary = match ch {
            '\r' => {
                // `\r\n` 只算一处边界，故吃掉后面的 `\n`（它仍占一个字符位置）
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    char_cursor += 1;
                }
                true
            }
            '\n' => true,
            _ => false,
        };

        if is_boundary {
            push_segment(&mut paragraphs, &current, segment_start_char);
            current.clear();
            char_cursor += 1;
            segment_start_char = char_cursor;
        } else {
            current.push(ch);
            char_cursor += 1;
        }
    }
    push_segment(&mut paragraphs, &current, segment_start_char);

    // 段的 char_start 需要重新编号（因为空段被丢弃，index 会与 char_start 脱节）
    for (index, paragraph) in paragraphs.iter_mut().enumerate() {
        paragraph.index = index;
    }
    let _ = normalized;

    paragraphs
}

/// 归一化换行符（`\r\n` 与 `\r` → `\n`）。
///
/// 保留此函数是为了让"归一化"这件事在代码里有明确落点；
/// 当前 `split_by_newline` 用逐字符扫描实现（因为它要同时维护原文字符位置）。
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// 追加一段（丢弃全空段）。
fn push_segment(paragraphs: &mut Vec<Paragraph>, text: &str, char_start: usize) {
    if text.is_empty() {
        return;
    }
    paragraphs.push(Paragraph {
        index: paragraphs.len(),
        char_start,
        text: text.to_owned(),
        from_heuristic: false,
    });
}

/// 启发式路径：前导两个全角空格是段落边界（FR-002）。
///
/// ## 缩进的处理
///
/// 两个全角空格是**标记**而非内容，故切出的段文本**不含**它们——
/// 段首缩进由排版层按 `ReadingConfig.paragraph_indent`（默认 2em）重新施加。
/// 若把原文的缩进也留进段文本，缩进会被施加两次（4em）。
/// 这也是"排版不改写原文"与"渲染不含标记"两者不矛盾的原因：
/// 原文由 001 保存，排版层只产出**视觉**结果。
fn split_by_ideographic_space(text: &str) -> Vec<Paragraph> {
    let marker = format!("{IDEOGRAPHIC_SPACE}{IDEOGRAPHIC_SPACE}");
    let mut paragraphs = Vec::new();

    // 按标记切分，同时累计每段在**原文**中的字符位置。
    //
    // `char_cursor` 是本路径能给出精确 `char_start` 的关键：每切出一段，
    // 游标前进「该段字符数 + 标记字符数（2）」。因为段文本**剥离了标记**，
    // 若不用游标而用"上一段 char_end"，每段都会少算 2 个字符，累积错位。
    let mut char_cursor = 0usize;
    let mut remaining = text;

    loop {
        match remaining.find(&marker) {
            Some(byte_pos) => {
                let before = &remaining[..byte_pos];
                push_heuristic_segment(&mut paragraphs, before, char_cursor);
                char_cursor += before.chars().count() + marker.chars().count();
                remaining = &remaining[byte_pos + marker.len()..];
            }
            None => {
                push_heuristic_segment(&mut paragraphs, remaining, char_cursor);
                break;
            }
        }
    }

    for (index, paragraph) in paragraphs.iter_mut().enumerate() {
        paragraph.index = index;
    }

    paragraphs
}

/// 追加一段（启发式路径）：剥离后全空的段被丢弃。
///
/// `char_start` 由调用方给的游标决定（含被剥离的标记位置），
/// 故段与段之间的 `char_start` 差值是「段长 + 2」而非「段长」。
fn push_heuristic_segment(paragraphs: &mut Vec<Paragraph>, text: &str, char_start: usize) {
    if text.trim().is_empty() {
        return;
    }
    paragraphs.push(Paragraph {
        index: paragraphs.len(),
        char_start,
        text: text.to_owned(),
        from_heuristic: true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // FR-001 默认路径
    // ------------------------------------------------------------------

    #[test]
    fn default_path_splits_on_newlines_only() {
        let result = split("第一段\n第二段\n第三段");
        assert_eq!(result.diagnostic.split_mode, SplitMode::NewlineOnly);
        assert!(!result.diagnostic.heuristic_applied);
        assert_eq!(result.paragraphs.len(), 3);
        assert_eq!(result.paragraphs[0].text, "第一段");
        assert_eq!(result.paragraphs[2].text, "第三段");
    }

    /// `\r\n` 与 `\r` 视为同一类边界（FR-001 明文）。
    #[test]
    fn crlf_and_cr_are_same_kind_of_boundary() {
        for text in ["a\nb", "a\r\nb", "a\rb"] {
            let result = split(text);
            assert_eq!(
                result.paragraphs.len(),
                2,
                "{text:?} 应切成 2 段（换行符种类不影响切分）"
            );
            assert_eq!(result.paragraphs[0].text, "a");
            assert_eq!(result.paragraphs[1].text, "b");
        }
    }

    /// **前导空格在默认路径下是段内文本**（FR-001 明文，不许当段落标记）。
    #[test]
    fn leading_spaces_are_content_in_default_path() {
        // 只有 2 个换行 + 3 处缩进：条件 ①（<3 换行）满足但换行数够不够另说
        // 此处刻意让换行数 ≥ 阈值，使启发式不启用
        let text = "  第一段\n\u{3000}\u{3000}第二段\n  第三段\n  第四段";
        let result = split(text);

        assert_eq!(result.diagnostic.split_mode, SplitMode::NewlineOnly);
        assert!(
            result.paragraphs[0].text.starts_with("  "),
            "默认路径下前导半角空格是段内文本，实测：{:?}",
            result.paragraphs[0].text
        );
        assert!(
            result.paragraphs[1].text.starts_with(IDEOGRAPHIC_SPACE),
            "默认路径下前导全角空格也保留在段内，实测：{:?}",
            result.paragraphs[1].text
        );
    }

    /// 单段（无换行）也是合法输入。
    #[test]
    fn single_paragraph_without_newline() {
        let result = split("只有一段正文，没有任何换行符。");
        assert_eq!(result.paragraphs.len(), 1);
        assert_eq!(result.paragraphs[0].char_start, 0);
    }

    /// 空输入 → 0 段（不 panic）。
    #[test]
    fn empty_input_yields_no_paragraphs() {
        let result = split("");
        assert_eq!(result.paragraphs.len(), 0);
        assert_eq!(result.diagnostic.newline_count, 0);
    }

    /// 连续换行产生空段 → 丢弃（空段对排版无意义）。
    #[test]
    fn blank_lines_are_dropped() {
        let result = split("第一段\n\n\n第二段");
        assert_eq!(result.paragraphs.len(), 2, "空段应被丢弃");
        assert_eq!(result.paragraphs[1].text, "第二段");
    }

    /// `char_start` 必须指向**原文**里的位置（归一化不得让它错位）。
    #[test]
    fn char_start_refers_to_original_text() {
        // "ab\r\ncd"：\r\n 占原文 2 个字符，第二段应从字符 4 开始
        let result = split("ab\r\ncd");
        assert_eq!(result.paragraphs.len(), 2);
        assert_eq!(result.paragraphs[0].char_start, 0);
        assert_eq!(
            result.paragraphs[1].char_start, 4,
            "「cd」在原文中的字符位置是 4（a,b,\\r,\\n）"
        );
    }

    // ------------------------------------------------------------------
    // FR-002 启发式双条件门槛
    // ------------------------------------------------------------------

    /// **条件都满足** → 启用启发式。
    #[test]
    fn heuristic_applies_when_both_conditions_met() {
        // 0 个换行（< 阈值 3）+ 4 处前导全角空格（>= 3）
        let text = "\u{3000}\u{3000}第一段内容\u{3000}\u{3000}第二段内容\u{3000}\u{3000}第三段内容\u{3000}\u{3000}第四段内容";
        let result = split(text);

        assert!(
            result.diagnostic.heuristic_applied,
            "双条件满足时应启用：{:?}",
            result.diagnostic
        );
        assert_eq!(
            result.diagnostic.split_mode,
            SplitMode::LeadingIdeographicSpace
        );
        assert!(
            result.paragraphs.len() >= 3,
            "应切出多段，实测 {} 段",
            result.paragraphs.len()
        );
        // 缩进是标记，不进段文本
        for paragraph in &result.paragraphs {
            assert!(
                !paragraph.text.starts_with(IDEOGRAPHIC_SPACE),
                "段文本不得带上作为标记的缩进：{:?}",
                paragraph.text
            );
            assert!(paragraph.from_heuristic);
        }
    }

    /// **条件 ① 不满足**（换行够多）→ 不启用（防"正常分段正文被切碎"）。
    #[test]
    fn heuristic_not_applied_when_enough_newlines() {
        let text = "第一段\n\u{3000}\u{3000}第二段\n第三段\n\u{3000}\u{3000}第四段\n第五段";
        let result = split(text);

        assert!(
            result.diagnostic.newline_count >= PARAGRAPH_COUNT_THRESHOLD,
            "前置条件：换行数应 >= 阈值"
        );
        assert!(
            !result.diagnostic.heuristic_applied,
            "换行充足时不得启用启发式：{:?}",
            result.diagnostic
        );
        assert_eq!(result.diagnostic.split_mode, SplitMode::NewlineOnly);
    }

    /// **条件 ② 不满足**（缩进出现太少）→ 不启用（防偶然命中）。
    #[test]
    fn heuristic_not_applied_when_too_few_space_pairs() {
        // 1 个换行（< 3）但只有 2 处缩进（< 3）
        let text = "\u{3000}\u{3000}第一段\n\u{3000}\u{3000}第二段";
        let result = split(text);

        assert!(
            result.diagnostic.leading_space_hits < LEADING_IDEOGRAPHIC_SPACE_MIN_HITS,
            "前置条件：缩进命中数应 < 阈值"
        );
        assert!(
            !result.diagnostic.heuristic_applied,
            "缩进不足时不得启用启发式：{:?}",
            result.diagnostic
        );
    }

    /// **不启用时零副作用**：输出与"直接从默认路径走"完全一致。
    #[test]
    fn disabled_heuristic_has_no_side_effects() {
        let text = "第一段内容\n第二段内容\n第三段内容\n第四段内容";
        let with_detection = split(text);
        let default_only = split_by_newline(text);

        assert!(!with_detection.diagnostic.heuristic_applied);
        assert_eq!(
            with_detection.paragraphs, default_only,
            "不启用时两路径的段落序列必须逐字段相同（FR-002：MUST NOT 在条件不满足时生效）"
        );
    }

    /// 段标记计数与切分行为**严格一致**（两处独立实现不得漂移）。
    ///
    /// ## 正确的关系是 `段数 == 标记数 + 1`（不是相等）
    ///
    /// 标记是**分隔符**：`N` 个分隔符把文本切成 `N + 1` 片（只要首尾都有内容）。
    /// 初版断言"相等"，前提就错了——那是把标记当"段的前缀"而非"段间的分隔"。
    ///
    /// 两种视角在**首段有标记**时容易混：若每个段前都有一个标记，
    /// 则 `N` 个段有 `N` 个标记（无前缀的写法）。FR-002 的原文说
    /// "检出不少于 3 处前导两个全角空格"，用"处"这个量词，
    /// 两种视角下都是"标记的出现次数"——故**计数本身**不歧义，
    /// 歧义只在"标记数与段数的关系"上。
    ///
    /// 本测试断言的是**切分后的真实关系**，从而把两个实现的一致性钉住。
    #[test]
    fn marker_count_agrees_with_split_behavior() {
        let text = "第一段\u{3000}\u{3000}段中空隙\n第二段\u{3000}\u{3000}又一处";
        let hits = count_leading_ideographic_space_pairs(text);
        let paragraphs = split_by_ideographic_space(text);

        assert_eq!(
            paragraphs.len(),
            hits + 1,
            "分隔符语义：N 个标记切出 N+1 段（实测 {hits} 个标记、{} 段）",
            paragraphs.len()
        );
        // 前提自检：本例确实有多段（否则上面的关系平凡成立）
        assert!(paragraphs.len() >= 2, "本例应切出至少 2 段");
    }

    /// 三个连续全角空格只算**一次**标记（与切分一致）。
    #[test]
    fn triple_spaces_count_once() {
        let text = "\u{3000}\u{3000}\u{3000}只有一段";
        assert_eq!(
            count_leading_ideographic_space_pairs(text),
            1,
            "三个连续全角空格是第一对标记 + 一个内容字符，不是两对"
        );
    }

    /// 单个全角空格不是标记（FR-002 明文要求"两个"）。
    #[test]
    fn single_ideographic_space_is_not_a_marker() {
        assert_eq!(count_leading_ideographic_space_pairs("\u{3000}单格"), 0);
        assert_eq!(
            count_leading_ideographic_space_pairs("段一\u{3000}段二"),
            0,
            "单格是内容而非标记"
        );
    }

    /// 诊断记录可读（含全部门槛输入——FR-002 要求"可读的诊断记录"）。
    #[test]
    fn diagnostic_is_readable_and_carries_threshold_inputs() {
        let result = split("a\nb\nc\nd");
        let summary = result.diagnostic.summary();

        assert!(summary.contains("split_mode="), "{summary}");
        assert!(summary.contains("newlines="), "{summary}");
        assert!(
            summary.contains("leading_ideographic_space_hits="),
            "{summary}"
        );
        assert!(summary.contains("applied="), "{summary}");
        // 诊断的每个计数必须真的是门槛的输入（不是装饰字段）
        assert_eq!(result.diagnostic.newline_count, 3);
    }

    // ------------------------------------------------------------------
    // 段落模型
    // ------------------------------------------------------------------

    #[test]
    fn paragraph_offsets_are_char_based() {
        let result = split("中文\n😀emoji");
        assert_eq!(result.paragraphs.len(), 2);
        // 第二段起点 = 第一段字符数(2) + 换行(1) = 3
        assert_eq!(result.paragraphs[1].char_start, 3);
        assert_eq!(result.paragraphs[1].char_len(), 6); // 😀 e m o j i
    }

    /// 段落能构造出自己的文本模型（排版输入）。
    #[test]
    fn paragraph_produces_text_model() {
        let paragraph = Paragraph::new(0, 0, "中文😀");
        let model = paragraph.model();
        assert_eq!(model.char_len(), 3);
        assert_eq!(model.utf16_len(), 4);
    }

    #[test]
    fn normalize_newlines_collapses_all_forms() {
        assert_eq!(normalize_newlines("a\r\nb"), "a\nb");
        assert_eq!(normalize_newlines("a\rb"), "a\nb");
        assert_eq!(normalize_newlines("a\nb"), "a\nb");
    }

    #[test]
    fn newline_counter_treats_crlf_as_one() {
        assert_eq!(count_newlines("a\nb"), 1);
        assert_eq!(count_newlines("a\r\nb"), 1, "\\r\\n 是一个边界");
        assert_eq!(count_newlines("a\rb"), 1);
        assert_eq!(count_newlines("a\r\n\r\nb"), 2);
    }
}
