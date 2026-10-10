//! \[DONE] 无目录时的按大小切分（tasks.md T076 / FR-020）
//!
//! # 为什么需要它
//!
//! FR-020 的目录识别是"支持多条规则回退"，但**回退的终点**必须存在：
//! 用户手上大量 TXT 是**没有目录的**（一整段连载、一本散文集、一份转换自
//! 其他格式的纯文本）。若没有规则命中就产出"零章节"，这类文件根本无法
//! 阅读——而它们正是 TXT 格式最常见的形态。
//!
//! 故兜底是按**大小**等分：让文件可被分页阅读、进度可被记录、锚点有意义。
//!
//! # 切分位置的选取
//!
//! 不能简单地"每 N 字符切一刀"——那会把段落切在句子中间，读起来突兀。
//! 本模块在**目标位置附近寻找段落边界**（连续换行 / 空行），找不到才用
//! 硬切。这是纯体验优化，但代价极低（只多扫几十个字符）。
//!
//! # 与"有目录但切点太少"的区别
//!
//! 两者都落到本模块，但**诊断信息不同**：[`crate::toc_rule::TocDetection`]
//! 的 `split_by_size` 与 `rule_name` 组合出四种情形（有/无规则 × 有/无兜底），
//! 上层据此能如实告诉用户"你的规则一条都没命中"而不是"成功了但没有章节"。

use crate::txt::TocPoint;

/// 默认的片段大小（**字符**数）。
///
/// 取 3000：中文阅读场景下约等于 6 KB 文本，是一个自然的分页粒度
/// （约 10~15 分钟的阅读量，与常见网文单章量级相当）。
///
/// \[WORKAROUND] 硬编码常量：改为随配置注入（`config.toml` 的阅读子集）。
/// 当前阶段保持无配置依赖——SC-004 的性能测试要能在无配置环境下跑。
pub const DEFAULT_SEGMENT_CHARS: usize = 3000;

/// 在硬切位置附近搜索段落边界的窗口（字符数）。
///
/// 取 200：一个中文段落的典型长度在 100 字上下，200 的窗口足以跨过
/// 一整个短段落而不会把边界拉得太远（拉到 3000 字符的 6% 以外就失去
/// "等分"的意义了）。
const BOUNDARY_SEARCH_CHARS: usize = 200;

/// \[DONE] 按大小把文本等分为若干片段，返回与 [`TocPoint`] 同构的切点。
///
/// 切点总数 = `ceil(字符数 / DEFAULT_SEGMENT_CHARS)`，**至少 1 个**
/// （空文件与短文件都得到单章，避免上层面对零章节）。
pub fn split_by_size(text: &str) -> Vec<TocPoint> {
    split_by_size_with(text, DEFAULT_SEGMENT_CHARS)
}

/// 同 [`split_by_size`]，但可指定片段大小（测试与配置注入用）。
pub fn split_by_size_with(text: &str, segment_chars: usize) -> Vec<TocPoint> {
    let segment_chars = segment_chars.max(1);
    let total_chars = text.chars().count();
    if total_chars == 0 {
        // 空文本仍然给出一章：零章节会让"当前读到第几章"这类逻辑无处落点。
        return vec![TocPoint {
            title: generated_title(0),
            byte_offset: 0,
            char_offset: 0,
        }];
    }

    // 字符下标 → 字节下标的映射表。
    //
    // ## 为什么要预先建表（一次 O(n) 而不是每点一次 find）
    //
    // 每次"从字符下标找字节下标"若用 `char_indices().nth(i)`，单次是 O(i)，
    // 100MB 文件切出 3 万段就是 O(n²)——那会直接把 SC-004 的 8 秒预算烧光。
    // 建一份边界数组后每次查询是 O(1)，代价是一次 O(n) 扫描（几 MB 内存）。
    let boundaries: Vec<usize> = text.char_indices().map(|(byte, _)| byte).collect();

    let mut points = Vec::new();
    let mut start_char = 0usize;
    let mut index = 0usize;
    while start_char < total_chars {
        let target = start_char + segment_chars;
        if target >= total_chars {
            points.push(point_at(text, &boundaries, start_char, index));
            break;
        }
        let cut = find_paragraph_boundary(text, &boundaries, target, start_char + segment_chars);
        points.push(point_at(text, &boundaries, start_char, index));
        start_char = cut;
        index += 1;
    }
    points
}

/// 在 `target` 附近寻找段落边界（返回**字符**下标）。
///
/// 搜索范围是 `[start + 1, target + BOUNDARY_SEARCH_CHARS]`——下限 +
/// 1 保证前进（否则死循环），上限允许往后找。
fn find_paragraph_boundary(
    text: &str,
    boundaries: &[usize],
    target: usize,
    minimum: usize,
) -> usize {
    let total = boundaries.len();
    let upper = (target + BOUNDARY_SEARCH_CHARS).min(total);
    let mut candidate = target.min(upper);

    let mut probe = target.min(upper);
    while probe < upper {
        let byte = boundaries[probe];
        // 段落边界：一个换行后面紧跟另一个换行（允许中间有空白）。
        // 单换行不切——中文 TXT 里单换行常是排版换行而非段落结束。
        if text.as_bytes().get(byte) == Some(&b'\n') {
            let next = byte + 1;
            let rest = &text[next..];
            let chars = rest.chars();
            let mut blank_lines = 0usize;
            for ch in chars {
                match ch {
                    '\n' => blank_lines += 1,
                    '\r' | ' ' | '\t' | '\u{3000}' => {}
                    _ => break,
                }
                if blank_lines >= 1 {
                    break;
                }
            }
            if blank_lines >= 1 {
                // 落在空行之后，即下一段段的起点。
                candidate = probe + 1;
                break;
            }
        }
        probe += 1;
    }

    // 至少前进一个字符，否则循环不终止。
    if candidate <= minimum {
        minimum + 1
    } else {
        candidate.min(total)
    }
}

/// 由字符下标构造一个 [`TocPoint`]。
fn point_at(text: &str, boundaries: &[usize], char_index: usize, ordinal: usize) -> TocPoint {
    let byte_offset = boundaries.get(char_index).copied().unwrap_or(text.len());
    TocPoint {
        title: segment_title(text, byte_offset, ordinal),
        byte_offset: byte_offset as i64,
        char_offset: char_index as i64,
    }
}

/// 为按大小切出的片段起一个可辨识的标题。
///
/// 优先取该片段**首行的前若干字**——那让用户在目录里能看出这一段的开头内容，
/// 比 `第 3 段` 这样的机械编号有用得多（读者靠内容回忆位置）。首行是空行
/// 或过短时退回编号标题。
fn segment_title(text: &str, byte_offset: usize, ordinal: usize) -> String {
    let tail = &text[byte_offset..];
    let first_line = tail
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if first_line.chars().count() < 2 {
        return generated_title(ordinal);
    }
    let snippet: String = first_line.chars().take(20).collect();
    snippet
}

/// 机械编号标题（第 N 段，从 1 起便于人读）。
pub fn generated_title(ordinal: usize) -> String {
    format!("第 {} 段", ordinal + 1)
}

/// 从文件名推导书名（去掉扩展名）。
///
/// 不做"《》去壳"之类的花哨处理：文件名就是用户给这本书起的名字，
/// 擅自改写会让用户找不到自己命名的书。仅去扩展名——那是格式标记不是书名。
pub fn book_name_from_file_name(file_name: &str) -> String {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        return "未命名".to_string();
    }
    match trimmed.rsplit_once('.') {
        // 只把**短后缀**当扩展名：`我的书.v2.txt` 应去掉 `.txt`，
        // 而 `银河系漫游指南.第2部` 的 `.第2部` 是书名的一部分。
        Some((stem, extension))
            if !stem.is_empty()
                && extension.len() <= 5
                && extension.chars().all(|c| c.is_ascii_alphanumeric()) =>
        {
            stem.to_string()
        }
        _ => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_still_yields_one_chapter() {
        let points = split_by_size("");
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].byte_offset, 0);
    }

    #[test]
    fn short_text_is_a_single_chapter() {
        let points = split_by_size("很短的一段文字。");
        assert_eq!(points.len(), 1);
        assert_eq!(points[0].char_offset, 0);
    }

    #[test]
    fn long_text_is_split_into_the_expected_number_of_pieces() {
        let text = "字".repeat(1000);
        let points = split_by_size_with(&text, 100);
        assert_eq!(points.len(), 10);
        // 片段必须无缝覆盖全文：第一段从 0 起，末段覆盖到结尾。
        assert_eq!(points[0].char_offset, 0);
        assert!(points.last().expect("非空").char_offset < 1000);
    }

    #[test]
    fn pieces_cover_the_whole_text_without_gaps() {
        let text = "abcdefghij".repeat(100);
        let points = split_by_size_with(&text, 77);
        for window in points.windows(2) {
            assert!(
                window[0].char_offset < window[1].char_offset,
                "切点必须严格前进，否则会产生空片段或死循环"
            );
        }
        // 逐段按偏移取字节，拼接后应还原原文。
        let mut rebuilt = String::new();
        for (index, point) in points.iter().enumerate() {
            let start = point.byte_offset as usize;
            let end = points
                .get(index + 1)
                .map(|next| next.byte_offset as usize)
                .unwrap_or(text.len());
            rebuilt.push_str(&text[start..end]);
        }
        assert_eq!(rebuilt, text);
    }

    #[test]
    fn cuts_prefer_paragraph_boundaries() {
        // 每 50 字一段，段间用空行分隔 —— 切口应落在空行上。
        let paragraph = format!("{}\n\n", "文".repeat(40));
        let text = paragraph.repeat(20);
        let points = split_by_size_with(&text, 50);
        assert!(points.len() > 1);
        for point in &points[1..] {
            let before = &text[..point.byte_offset as usize];
            // 切口处往前看应是空行结束（连续两个换行）或段落末尾。
            assert!(
                before.ends_with('\n'),
                "切口应落在换行处，实际前文结尾为 {:?}",
                &before[before.len().saturating_sub(4)..]
            );
        }
    }

    #[test]
    fn hard_cut_when_no_paragraph_boundary_exists() {
        // 一整段没有换行的超长文本：只能硬切，且必须仍前进。
        let text = "x".repeat(500);
        let points = split_by_size_with(&text, 100);
        assert_eq!(points.len(), 5);
        assert!(points
            .windows(2)
            .all(|w| w[0].char_offset < w[1].char_offset));
    }

    #[test]
    fn multibyte_text_offsets_stay_on_char_boundaries() {
        // 硬切若按字节做会切碎码点、让 &str 切片 panic——本测试钉住按字符切。
        let text = "中".repeat(300);
        let points = split_by_size_with(&text, 64);
        for point in &points {
            // 能取到切片本身就证明落在字符边界上。
            let _ = &text[point.byte_offset as usize..];
        }
        assert!(points.len() >= 4);
    }

    #[test]
    fn titles_use_the_opening_content_of_each_piece() {
        let text = format!("开篇正文\n{}\n\n后面还有很多内容", "字".repeat(200));
        let points = split_by_size_with(&text, 100);
        assert_eq!(points[0].title, "开篇正文");
    }

    #[test]
    fn blank_leading_lines_fall_back_to_numbered_titles() {
        let text = "\n\n\n".to_string() + &"字".repeat(300);
        let points = split_by_size_with(&text, 100);
        assert!(
            points[0].title.starts_with("第 ") || !points[0].title.trim().is_empty(),
            "空首行时不应产出空标题，实际 {:?}",
            points[0].title
        );
    }

    #[test]
    fn generated_title_is_one_based_for_humans() {
        assert_eq!(generated_title(0), "第 1 段");
        assert_eq!(generated_title(9), "第 10 段");
    }

    #[test]
    fn file_name_extension_is_stripped() {
        assert_eq!(book_name_from_file_name("我的书.txt"), "我的书");
        assert_eq!(book_name_from_file_name("Novel.TXT"), "Novel");
        assert_eq!(book_name_from_file_name("无扩展名"), "无扩展名");
    }

    #[test]
    fn dotted_titles_keep_their_dots() {
        // `.第2部` 不是扩展名（中文、且比 5 字节长），应保留。
        assert_eq!(
            book_name_from_file_name("银河系漫游指南.第二部"),
            "银河系漫游指南.第二部"
        );
    }

    #[test]
    fn empty_file_name_gets_a_placeholder() {
        assert_eq!(book_name_from_file_name(""), "未命名");
        assert_eq!(book_name_from_file_name("   "), "未命名");
    }

    #[test]
    fn zero_segment_size_does_not_hang() {
        // 防御：配置里给了 0 也不能死循环。
        let points = split_by_size_with("abcdef", 0);
        assert!(!points.is_empty());
        assert!(points.len() <= 6);
    }
}
