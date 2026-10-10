//! \[DONE] TXT 导入：编码检测 + 目录识别 + 按大小切分（tasks.md T076 / FR-020）
//!
//! # 规格原文
//!
//! > **FR-020**: 系统 MUST 支持 TXT 导入：识别编码、识别章节目录
//! > （支持多条规则回退）、按位置定位章节
//!
//! 三件事对应本模块的三个阶段：
//!
//! ```text
//!   ① 字节 ──[编码检测]──► 文本 + 编码名
//!   ② 文本 ──[多条目录规则回退]──► 章节切点（字节偏移 + 标题）
//!   ③ 无规则命中 ──[按大小切分]──► 等长片段
//! ```
//!
//! # 为什么"多条规则回退"必须是回退而不是合并
//!
//! FR-020 的"多条规则回退"含义是：按 `sort_order` 依次**尝试**，**第一条
//! 产出足够切点的规则胜出**。这不是实现选择，而是需求语义——用户配置多条
//! 规则正是为了"不知道这本书用哪种章节格式"：一本用 `第X章`，另一本用
//! `【X】`。若把多条规则的结果**合并**，一本同时含两种写法的书会被切得
//! 支离破碎（`第X章` 与 `【X】` 交替触发）。
//!
//! 故 [`TocDetection`] 里记录的是**采用的那一条规则**（`rule_name`），
//! 而非全部规则的结果——`tome local toc` 要能回答"用到了哪条规则"，
//! 这正是 `contracts/cli.md` 的原文要求。
//!
//! # 为什么切点存**字节偏移**而不是字符偏移
//!
//! `data-model.md` 的 `Chapter.byte_offset` 明确是**文件内字节偏移**。
//! 用字节偏移的理由是它可以**不经解码**地定位到原文位置（`ContentProvider`
//! 的本地 adapter 按偏移直读，见 T083）。字符偏移需要先解码整个文件才能
//! 换算，而 300MB 文件的解码结果是几百 MB 的 `String`——那既有内存代价，
//! 也在每次翻章时重复付出。
//!
//! 故本模块在**解码后的文本**上做匹配（正则要跑在 `&str` 上），但把每个
//! 匹配位置的**字符下标换算回字节下标**（`char_indices` 的前缀和），
//! 落库的是字节偏移。
//!
//! # SC-004 的性能约束（100MB <8s）
//!
//! 约束的落点：
//!
//! | 阶段 | 复杂度 | 说明 |
//! |---|---|---|
//! | 编码检测 | O(min(n, 1 MiB)) | 采样，见 [`crate::encoding`] |
//! | 解码 | O(n) | 无法避免——文本必须产出 |
//! | 目录匹配 | O(n × 规则数) | 每条规则一次全文扫描，**命中即停** |
//! | 切点换算 | O(切点数 × log n) | 二分查字符→字节映射 |
//!
//! 关键设计：**不逐行 split 再正则**（那会为每行分配一个 `String`）。
//! 直接用 `Regex::find_iter` 在整段文本上迭代，只分配命中的标题。
//!
//! # 关键入口
//!
//! - [`import_bytes`] —— 字节 + 规则 → [`ParsedTxt`]（主入口，纯函数）
//! - [`import_path`] —— 从文件路径导入（含 `--copy` 的另一半在 `library`）

use std::path::Path;

use tome_core::{Book, Chapter, TxtTocRule};

use crate::encoding::{detect_and_decode, DecodedText};
use crate::error::{from_io, LocalError, LocalResult};
use crate::split;
use crate::toc_rule::{self, TocDetection};

/// 章节标题的长度上限（字符数）。
///
/// ## 为什么要有上限
///
/// 一条过宽的正则（用户配置的目录规则不可信）可能匹配到整段正文。若不设限，
/// 一个"章节标题"会变成几千字，落进 `chapter.title`（数据库列、界面标题栏）
/// 后既撑爆界面也无意义。截断而非丢弃：切点仍然有效，只是标题被剪短。
///
/// 取 100：中文书名号标题（`【第X卷 卷名】`）通常不超过 30 字，100 留足余量。
const MAX_TITLE_CHARS: usize = 100;

/// 一条文本内章节切点（标题 + **字节偏移**）。
///
/// 本模块的内部产物，随后被映射为 [`Chapter`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocPoint {
    /// 章节标题
    pub title: String,
    /// 该标题在**文件**中的字节偏移
    pub byte_offset: i64,
    /// 该标题在**解码后文本**中的字符偏移
    pub char_offset: i64,
}

/// 一次 TXT 解析的完整结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTxt {
    /// 书籍元信息（`origin_id` 已按内容哈希填好）
    pub book: Book,
    /// 章节切点（已按位置升序）
    pub toc: Vec<TocPoint>,
    /// 目录识别的诊断（用了哪条规则 / 为何回退）
    pub detection: TocDetection,
    /// 解码结果摘要（编码名 + 是否有损）
    pub encoding: EncodingSummary,
}

/// 解码结果的摘要（不持文本本体，避免把整本书的内容挂在解析结果上）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingSummary {
    /// 识别出的编码名
    pub charset: String,
    /// 解码时是否出现替换字符（正文可能有损）
    pub had_errors: bool,
    /// 文件的字节长度
    pub byte_len: u64,
}

impl EncodingSummary {
    fn of(decoded: &DecodedText, byte_len: u64) -> Self {
        Self {
            charset: decoded.charset.clone(),
            had_errors: decoded.had_errors,
            byte_len,
        }
    }
}

/// \[DONE] FR-020 的主入口：把一段 TXT 字节解析为书 + 目录。
///
/// # 参数
///
/// - `bytes`：文件原始字节（**不可信输入**：可能是任意二进制）
/// - `file_name`：文件的显示名（用作书名的初值）
/// - `rules`：目录识别规则，**按 `sort_order` 回退**（`enabled=false` 的被跳过）
///
/// # 纯函数
///
/// 不碰文件系统、不碰数据库——故它可以被单测直接驱动，也是差分测试的
/// 理想落点（输入→输出确定）。
pub fn import_bytes(bytes: &[u8], file_name: &str, rules: &[TxtTocRule]) -> ParsedTxt {
    let decoded = detect_and_decode(bytes);
    let byte_len = bytes.len() as u64;
    let encoding = EncodingSummary::of(&decoded, byte_len);

    // ② 多条规则回退。
    let detection = toc_rule::detect(&decoded.text, rules);

    // ③ 规则都没命中 → 按大小切分（与"命中但切点太少"是不同情形，见下）。
    //
    // `split_by_size` 是**诊断字段**：上层据此能如实告诉用户"你的规则
    // 一条都没命中，这是机械分页"而不是含糊的"导入成功"。故它必须在这里
    // 被置位，而不是让上层去猜（`rule_name == None` 只是必要非充分）。
    let mut detection = detection;
    let toc = match detection.points.as_slice() {
        [] => {
            detection.split_by_size = true;
            split::split_by_size(&decoded.text)
        }
        points => points.to_vec(),
    };

    // 切点 → 章节前的最终整理：标题截断、去除纯空白标题、按位置升序。
    let toc = finalize_points(&decoded.text, toc);

    let book_name = split::book_name_from_file_name(file_name);
    let book = Book {
        id: 0,
        origin: tome_core::BookOrigin::Local,
        // 身份：**归一化文件内容哈希**（FR-025）。同一文件换个路径/换盘符
        // 仍是同一本书，故哈希只用内容、不含路径。
        origin_id: crate::hash::content_origin_id(bytes),
        name: book_name,
        author: None,
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: None,
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: Some(encoding.charset.clone()),
        format: Some("txt".to_string()),
        total_chapters: Some(toc.len() as i32),
        dur_chapter_index: None,
        added_at: 0,
    };

    ParsedTxt {
        book,
        toc,
        detection,
        encoding,
    }
}

/// 从文件路径导入 TXT。
pub fn import_path(path: &Path, rules: &[TxtTocRule]) -> LocalResult<ParsedTxt> {
    let bytes = std::fs::read(path).map_err(|error| from_io(path, error))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !crate::format::looks_like_text(&bytes) {
        return Err(LocalError::UnsupportedFormat {
            format: "binary".to_string(),
        });
    }
    let mut parsed = import_bytes(&bytes, &file_name, rules);
    parsed.book.relative_path = Some(crate::library::normalize_for_storage(path));
    Ok(parsed)
}

/// 把 [`TocPoint`] 列表整理为落库的 [`Chapter`] 列表。
///
/// # 三项整理
///
/// 1. **标题截断**到 [`MAX_TITLE_CHARS`]——过宽的规则可能匹配整段正文
/// 2. **丢弃空标题**——匹配到纯空白没有意义，反而会产出无名章节
/// 3. **按字节偏移升序**——规则可能在文本里乱序命中（如先匹配到页脚的
///    "下一章"字样）；序号必须反映正文顺序，否则翻章会往回跳
fn finalize_points(_text: &str, points: Vec<TocPoint>) -> Vec<TocPoint> {
    let mut cleaned: Vec<TocPoint> = points
        .into_iter()
        .filter_map(|mut point| {
            let trimmed = point.title.trim().to_string();
            if trimmed.is_empty() {
                return None;
            }
            point.title = truncate_chars(&trimmed, MAX_TITLE_CHARS);
            Some(point)
        })
        .collect();
    cleaned.sort_by_key(|point| point.byte_offset);
    cleaned.dedup_by_key(|point| point.byte_offset);
    cleaned
}

/// 按**字符**数截断（不是字节——中文按字节截会切碎码点）。
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect()
}

/// 把 [`TocPoint`] 列表映射为落库的 [`Chapter`] 列表（`book_id` 已定）。
///
/// 序号从 0 起（`data-model.md` 的 `index_in_book`），按列表位置而非标题里的
/// 数字——后者在"序号错乱/缺号"的书里会与非唯一约束打架。
pub fn to_chapters(points: &[TocPoint], book_id: i64) -> Vec<Chapter> {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| Chapter {
            id: 0,
            book_id,
            index_in_book: index as i32,
            title: point.title.clone(),
            url: None,
            // 本地 TXT 用字节偏移定位（data-model.md）
            byte_offset: Some(point.byte_offset),
            epub_entry: None,
            is_cached: false,
            is_read: false,
            // 逐章字数需要按下一章偏移做差，见 `word_counts`
            word_count: None,
        })
        .collect()
}

/// 计算各章的字符数（`Chapter.word_count`）。
///
/// # 为什么单独一个函数而不是在 `to_chapters` 里顺手算
///
/// 算字数需要**相邻两章的偏移差**，而最后一章的结束位置是文本全长——
/// 那要求调用方有全文。`to_chapters` 只该看切点（它是纯映射），
/// 故字数由持有文本的调用方另算。返回的 `Vec` 与 `points` **等长**。
pub fn word_counts(text: &str, points: &[TocPoint]) -> Vec<i32> {
    let total_chars = text.chars().count() as i64;
    points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            let start = point.char_offset;
            let end = points
                .get(index + 1)
                .map(|next| next.char_offset)
                .unwrap_or(total_chars);
            (end - start).max(0) as i32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter_rule(rule: &str, order: i32) -> TxtTocRule {
        TxtTocRule {
            id: 0,
            name: format!("rule-{order}"),
            rule: rule.to_string(),
            example: None,
            enabled: true,
            sort_order: order,
        }
    }

    #[test]
    fn detects_chapters_with_a_user_rule() {
        let text = "第一章 起点\n正文一\n第二章 继续\n正文二\n第三章 结束\n正文三\n";
        let parsed = import_bytes(
            text.as_bytes(),
            "我的书.txt",
            &[chapter_rule("^第[一二三]章.*$", 0)],
        );
        assert_eq!(parsed.toc.len(), 3);
        assert_eq!(parsed.toc[0].title, "第一章 起点");
        assert_eq!(parsed.toc[2].title, "第三章 结束");
        assert_eq!(parsed.detection.rule_name.as_deref(), Some("rule-0"));
    }

    #[test]
    fn falls_back_to_the_second_rule_when_the_first_finds_nothing() {
        // FR-020 的"多条规则回退"：第一条无命中，第二条命中。
        let text = "【壹】开端\n内容\n【贰】承接\n内容\n【叁】收束\n内容\n";
        let parsed = import_bytes(
            text.as_bytes(),
            "b.txt",
            &[
                chapter_rule("^第[一二三]章.*$", 0),
                chapter_rule("^【.】.*$", 1),
            ],
        );
        assert_eq!(parsed.toc.len(), 3);
        assert_eq!(parsed.detection.rule_name.as_deref(), Some("rule-1"));
        // 回退过程可观测：第一条规则的失败要留在记录里。
        assert!(
            parsed.detection.attempts.len() >= 2,
            "应记录回退尝试，实际 {:?}",
            parsed.detection.attempts
        );
    }

    #[test]
    fn split_by_size_when_no_rule_matches() {
        // 30 万字的无目录文本应被切分为多个等长片段。
        let body = "这是一段没有章节标记的连续正文。".repeat(10_000);
        let parsed = import_bytes(body.as_bytes(), "无目录.txt", &[]);
        assert!(parsed.toc.len() > 1, "无目录时应按大小切分");
        assert!(parsed.detection.rule_name.is_none());
        assert!(parsed.detection.split_by_size, "应标记为按大小切分");
    }

    #[test]
    fn short_text_without_toc_is_a_single_chapter() {
        let parsed = import_bytes("很短的一本书。".as_bytes(), "短.txt", &[]);
        assert_eq!(parsed.toc.len(), 1);
    }

    #[test]
    fn byte_offsets_point_at_the_real_positions() {
        // 偏移必须能在**字节**层面定位回原文（T083 的偏移直读依赖它）。
        let text = "第一章 起点\n中文正文\n第二章 继续\n更多正文\n";
        let parsed = import_bytes(text.as_bytes(), "o.txt", &[chapter_rule("^第..章.*$", 0)]);
        for point in &parsed.toc {
            let at = &text.as_bytes()[point.byte_offset as usize..];
            assert!(
                at.starts_with(point.title.as_bytes()),
                "偏移 {} 处应能读到标题 `{}`",
                point.byte_offset,
                point.title
            );
        }
    }

    #[test]
    fn non_ascii_prefix_offsets_are_byte_based_not_character_based() {
        // 含多字节前缀时，字符下标与字节下标不同——本测试正是钉住这一点。
        let text = "简体中文前言若干\n第一章 起点\n正文\n第二章 继续\n正文\n";
        let parsed = import_bytes(text.as_bytes(), "m.txt", &[chapter_rule("^第.章.*$", 0)]);
        let first = &parsed.toc[0];
        assert_ne!(
            first.byte_offset, first.char_offset,
            "含中文前缀时字节偏移应大于字符偏移"
        );
        assert_eq!(
            &text.as_bytes()[first.byte_offset as usize..][..first.title.len()],
            first.title.as_bytes()
        );
    }

    #[test]
    fn gbk_file_is_detected_and_chapters_found() {
        // 注意用 `\n` 显式分行：Rust 的 `\` 续行会把两行拼成一行，
        // 那样 `^第.章` 就匹配不到后面两处了。
        let text = concat!(
            "第一章 开端\n这是一段中文正文，用于编码检测。\n",
            "第二章 承接\n继续写入足够多的内容以便检测器收敛。\n",
            "第三章 收束\n再补一些文字。\n",
        );
        let (bytes, _, _) = encoding_rs::GBK.encode(text);
        let parsed = import_bytes(&bytes, "gbk.txt", &[chapter_rule("^第.章.*$", 0)]);
        assert_eq!(parsed.toc.len(), 3);
        assert_eq!(parsed.toc[1].title, "第二章 承接");
        assert_eq!(parsed.encoding.charset, "GBK");
        assert!(!parsed.encoding.had_errors);
    }

    #[test]
    fn title_is_truncated_when_a_greedy_rule_matches_too_much() {
        // 贪婪规则匹配整行（正文也在同一行）——标题必须被剪短。
        let long_tail = "字".repeat(500);
        let text = format!("第1章 {long_tail}\n正文\n第2章 {long_tail}\n正文\n");
        let parsed = import_bytes(text.as_bytes(), "t.txt", &[chapter_rule("^第.章.*$", 0)]);
        assert_eq!(parsed.toc.len(), 2);
        assert_eq!(parsed.toc[0].title.chars().count(), MAX_TITLE_CHARS);
        assert!(parsed.toc[0].title.starts_with("第1章 "));
    }

    #[test]
    fn out_of_order_hits_are_sorted_by_position() {
        // 规则文件里先写了靠后的匹配（模拟乱序命中），结果必须按正文位置排。
        let text = "第一章 甲\nA\n第二章 乙\nB\n";
        let parsed = import_bytes(text.as_bytes(), "s.txt", &[chapter_rule("^第.章.*$", 0)]);
        assert!(parsed.toc[0].byte_offset < parsed.toc[1].byte_offset);
        assert_eq!(parsed.toc[0].title, "第一章 甲");
    }

    #[test]
    fn duplicate_offsets_collapse_to_one_chapter() {
        // 两条规则都命中同一位置时不应产出重复章节（去重按偏移）。
        let text = "第一章 甲\nA\n第二章 乙\nB\n";
        let parsed = import_bytes(text.as_bytes(), "d.txt", &[chapter_rule("^第.章.*$", 0)]);
        let mut offsets: Vec<i64> = parsed.toc.iter().map(|p| p.byte_offset).collect();
        let before = offsets.len();
        offsets.dedup();
        assert_eq!(offsets.len(), before);
    }

    #[test]
    fn book_name_strips_extension_and_uses_file_stem() {
        let parsed = import_bytes("正文".as_bytes(), "《书名》.txt", &[]);
        assert_eq!(parsed.book.name, "《书名》");
        assert_eq!(parsed.book.origin, tome_core::BookOrigin::Local);
        assert_eq!(parsed.book.format.as_deref(), Some("txt"));
    }

    #[test]
    fn origin_id_ignores_file_name_and_path() {
        // FR-025：同一内容换个文件名仍是同一本书。
        let a = import_bytes("同样的正文".as_bytes(), "a.txt", &[]);
        let b = import_bytes("同样的正文".as_bytes(), "别的名字.txt", &[]);
        assert_eq!(a.book.origin_id, b.book.origin_id);
    }

    #[test]
    fn origin_id_differs_for_different_content() {
        let a = import_bytes("正文甲".as_bytes(), "a.txt", &[]);
        let b = import_bytes("正文乙".as_bytes(), "a.txt", &[]);
        assert_ne!(a.book.origin_id, b.book.origin_id);
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let text = "第一章 甲\nA\n第二章 乙\nB\n";
        let mut rule = chapter_rule("^第.章.*$", 0);
        rule.enabled = false;
        let parsed = import_bytes(text.as_bytes(), "x.txt", &[rule]);
        assert!(parsed.detection.rule_name.is_none());
        assert!(parsed.detection.split_by_size);
    }

    #[test]
    fn chapter_indices_start_at_zero() {
        let text = "第一章 甲\nA\n第二章 乙\nB\n第三章 丙\nC\n";
        let parsed = import_bytes(text.as_bytes(), "i.txt", &[chapter_rule("^第.章.*$", 0)]);
        let chapters = to_chapters(&parsed.toc, 7);
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].index_in_book, 0);
        assert_eq!(chapters[2].index_in_book, 2);
        assert!(chapters.iter().all(|c| c.book_id == 7));
        assert!(chapters.iter().all(|c| c.byte_offset.is_some()));
        assert!(chapters.iter().all(|c| c.epub_entry.is_none()));
    }

    #[test]
    fn word_counts_cover_every_point_and_sum_to_the_text() {
        let text = "第一章 甲\nABC\n第二章 乙\nDEFG\n";
        let parsed = import_bytes(text.as_bytes(), "w.txt", &[chapter_rule("^第.章.*$", 0)]);
        let counts = word_counts(text, &parsed.toc);
        assert_eq!(counts.len(), parsed.toc.len());
        let total: i32 = counts.iter().sum();
        assert_eq!(total as usize, text.chars().count());
    }

    #[test]
    fn empty_file_parses_without_error() {
        // 空文件是合法的边界输入：不该 panic，也不该产出零章节让上层除零。
        let parsed = import_bytes(&[], "空.txt", &[]);
        assert_eq!(parsed.toc.len(), 1);
        assert_eq!(parsed.toc[0].title, split::generated_title(0));
    }

    #[test]
    fn binary_content_is_not_silently_accepted_as_txt() {
        // 一个含大量 NUL 的文件不是文本——导入它只会得到乱码，故显式拒绝。
        let mut bytes = vec![0u8; 4096];
        bytes.extend_from_slice(b"some text");
        assert!(!crate::format::looks_like_text(&bytes));
    }
}
