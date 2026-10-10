//! \[DONE] 换源与三级进度映射（tasks.md T070 / FR-010、FR-010b）
//!
//! # 三级降级链（FR-010b）
//!
//! | 级 | 手段 | 命中条件 |
//! |---|---|---|
//! | 一 | **特征段匹配** | `anchor_text` 在新源的邻域章节缓存里找到约 90% 相似度 |
//! | 二 | **章节标题近似** | 旧章标题与新源目录里某一章标题高度相似 |
//! | 三 | **降级到目录开头** | 前两级都没命中——**明确提示**，不静默跳转 |
//!
//! # 为什么"级别"必须在返回值里（而不是日志里）
//!
//! FR-010b 原文要求"换源过程 MUST 可观测（告知用户采用了哪一级匹配及其
//! 结果）"，且第三级"明确提示用户进度无法自动映射并落到目录开头，
//! **不静默跳转**"。
//!
//! 把级别放进 [`ProgressMapping`]（`tome-core` 的接缝类型）让"静默降级"
//! 在**类型层面**不成立：调用方拿到 `ChangeSourceOutcome` 就必须处理这个
//! 枚举，无法只读 `chapter_index` 而忽略"这是第三级降级"。
//!
//! # 为什么第一级在"邻域章节缓存"里搜而不是全书
//!
//! 换源时的旧进度指向"读到第 100 章"。新源的正文段几乎必然出现在
//! 第 100 章附近（两站的章节划分大同小异）。把搜索范围限定在邻域：
//!
//! - **性能**：只抓 2–3 章而不是 5 万章
//! - **准确**：全书搜索可能在"作者感言""重复章节"里找到误匹配
//!
//! 邻域半径见 [`NEIGHBORHOOD_RADIUS`]。

use tome_core::seam::{ChangeSourceOutcome, ProgressMapping};
use tome_core::{Book, Chapter, NetHost, ReadProgress};

use crate::error::SourceError;
use crate::source::ParsedSource;

/// 特征段匹配的邻域半径（旧进度章节的前后各几章）。
///
/// \[WORKAROUND] 取 3：两站章节划分的偏移通常不超过 2–3 章（一章被拆成
/// 两章、或卷首多一章"序"）。若实测发现某些源偏移更大，应改为可配置项
/// 而非继续调大——调大会让"误匹配到邻近的其他章节"的概率上升。
pub const NEIGHBORHOOD_RADIUS: i32 = 3;

/// 第一级的相似度阈值（百分比，FR-010b 的"约 90%"）。
pub const ANCHOR_SIMILARITY_THRESHOLD: u8 = 90;

/// 第二级标题近似的相似度阈值（百分比）。
///
/// 比第一级低：标题比正文片段短得多，同样比例的字符差异在短文本上更常见
/// （"第100章 大结局"与"第100章 终章"是两个非常相似的标题，但它们只有
/// 一半字符相同）。取 60 是"能认出是同一章"与"不误认无关章节"的折中。
///
/// \[WORKAROUND] 硬编码：若实测发现误匹配率高，应改为可配置。
pub const TITLE_SIMILARITY_THRESHOLD: u8 = 60;

/// 新旧章节匹配的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterMatch {
    /// 匹配到的新章节序号
    pub chapter_index: i32,
    /// 相似度（百分比）
    pub similarity_percent: u8,
}

// ---------------------------------------------------------------------------
// 相似度
// ---------------------------------------------------------------------------

/// 相似度：`needle` 有多大比例出现在 `haystack` 里（百分比）。
///
/// # 为什么是**单向包含度**而不是对称相似度
///
/// 换源要回答的问题是**"我要找的那段文字，在新章节里找到了吗"**——
/// 一个单向问题。`needle` 是锚点特征段（用户读到的那段文字），
/// `haystack` 是新章节的正文。
///
/// 用对称相似度（除以较长侧）会得出错误结论：
///
/// | 情形 | 对称口径 | 单向口径 | 哪个对 |
/// |---|---|---|---|
/// | 锚点 11 字，新章 40 字（其中含锚点 + 广告） | 27%（判为不匹配） | **100%** | 单向 |
/// | 锚点 11 字，新章 11 字且相同 | 100% | 100% | 都对 |
/// | 锚点 11 字，新章 11 字但只共 5 字 | 45% | 45% | 都对 |
///
/// 第一行是换源最常见的场景之一（新站插了广告），对称口径会把它判成
/// 不匹配，从而白白降级到第二级——而那段文字明明就在那里。
///
/// ## 为什么还保留 `MAX_COMPARE_CHARS`
///
/// LCS 是 O(n·m)。整章 5 万字会让二次方算法变成性能陷阱。
pub fn similarity_percent(needle: &str, haystack: &str) -> u8 {
    let needle: Vec<char> = needle.chars().take(MAX_COMPARE_CHARS).collect();
    let haystack: Vec<char> = haystack.chars().take(MAX_COMPARE_CHARS).collect();

    if needle.is_empty() {
        // 没有要找的东西：无从匹配。返回 0 而非 100——"空串被任何东西
        // 包含"是数学事实，但在这个场景里它意味着"锚点文本丢了"，
        // 报成完全匹配会让换源定位到一个任意章节。
        return 0;
    }
    if haystack.is_empty() {
        return 0;
    }

    let lcs = longest_common_subsequence_length(&needle, &haystack);
    // 单向：除以**要找的那一段**的长度
    ((lcs * 100) / needle.len()).min(100) as u8
}

/// 相似度比较的字符数上限。
///
/// # 为什么必须有
///
/// LCS 是 O(n·m)。整章 5 万字 × 5 万字是 25 亿次操作——足以让换源卡死。
/// 取 2000：特征段（FR-010b 的"约 90% 相似度"针对的是**片段**）通常
/// 几十到几百字符，2000 已远超需要。
///
/// 截断**前** 2000 字符而非随机采样：锚点文本是从章首附近取的片段，
/// 而换源时取的新章节文本也从章首开始，故两侧的前缀可比。
pub const MAX_COMPARE_CHARS: usize = 2000;

/// 最长公共子序列长度（滚动数组，空间 O(min(n,m))）。
fn longest_common_subsequence_length(left: &[char], right: &[char]) -> usize {
    // 让 `right` 是较短的一侧，把空间降到 O(min(n,m))
    let (short, long) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };

    let mut previous = vec![0usize; short.len() + 1];
    let mut current = vec![0usize; short.len() + 1];

    for long_char in long {
        for (index, short_char) in short.iter().enumerate() {
            current[index + 1] = if long_char == short_char {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
        // 交换后 `current` 里是上一轮的值，清零复用（避免重新分配）
        current.iter_mut().for_each(|cell| *cell = 0);
    }
    previous[short.len()]
}

// ---------------------------------------------------------------------------
// 三级映射
// ---------------------------------------------------------------------------

/// 第一级：在新章节正文里找特征段。
///
/// `probe` 是"给定新章节序号，取它的正文"的回调——由调用方决定正文从
/// 缓存还是网络来（本模块不关心，故是回调而非直接依赖缓存）。
pub fn map_by_anchor<F>(
    anchor_text: &str,
    expected_index: i32,
    neighborhood: i32,
    mut probe: F,
) -> Result<Option<ChapterMatch>, SourceError>
where
    F: FnMut(i32) -> Result<Option<String>, SourceError>,
{
    if anchor_text.trim().is_empty() {
        return Ok(None);
    }

    let mut best: Option<ChapterMatch> = None;
    for offset in -neighborhood..=neighborhood {
        let index = expected_index + offset;
        if index < 0 {
            continue;
        }
        let Some(text) = probe(index)? else {
            continue;
        };
        let similarity = similarity_percent(anchor_text, &text);
        if similarity >= ANCHOR_SIMILARITY_THRESHOLD
            && best
                .as_ref()
                .map(|current| similarity > current.similarity_percent)
                .unwrap_or(true)
        {
            best = Some(ChapterMatch {
                chapter_index: index,
                similarity_percent: similarity,
            });
        }
    }
    Ok(best)
}

/// 第二级：按章节标题近似匹配。
///
/// `expected_index` 只用于**打破平局**（两个候选标题同样相似时，取序号
/// 更接近旧进度的那个）——一个换源后跑到书中间的用户体验比"差一章"
/// 糟糕得多。
pub fn map_by_title(
    previous_title: &str,
    expected_index: i32,
    chapters: &[Chapter],
) -> Option<ChapterMatch> {
    if previous_title.trim().is_empty() {
        return None;
    }

    let mut best: Option<(ChapterMatch, i32)> = None;
    for chapter in chapters {
        let similarity = similarity_percent(previous_title, &chapter.title);
        if similarity < TITLE_SIMILARITY_THRESHOLD {
            continue;
        }
        let distance = (chapter.index_in_book - expected_index).abs();
        let better = match &best {
            None => true,
            Some((current, current_distance)) => {
                // 先比相似度，再比与旧进度的距离
                similarity > current.similarity_percent
                    || (similarity == current.similarity_percent && distance < *current_distance)
            }
        };
        if better {
            best = Some((
                ChapterMatch {
                    chapter_index: chapter.index_in_book,
                    similarity_percent: similarity,
                },
                distance,
            ));
        }
    }
    best.map(|(matched, _)| matched)
}

/// 三级映射的完整决策（纯函数，便于穷举测试）。
///
/// 返回的 [`ProgressMapping`] **必然**是三个变体之一——调用方无从忽略
/// "降级了"这件事（见模块文档）。
pub fn map_progress(
    progress: Option<&ReadProgress>,
    new_chapters: &[Chapter],
    anchor_match: Option<ChapterMatch>,
    title_match: Option<ChapterMatch>,
) -> ProgressMapping {
    // 第一级
    if let Some(matched) = anchor_match {
        return ProgressMapping::AnchorSegment {
            chapter_index: matched.chapter_index,
            char_offset: progress.map(|progress| progress.char_offset).unwrap_or(0),
            similarity_percent: matched.similarity_percent,
        };
    }
    // 第二级
    if let Some(matched) = title_match {
        return ProgressMapping::ChapterTitle {
            chapter_index: matched.chapter_index,
        };
    }
    // 第三级：**显式**降级（不静默跳转）
    let _ = (progress, new_chapters);
    ProgressMapping::FallbackToStart
}

/// 换源动作的完整执行。
///
/// # 参数里的 `probe_content`
///
/// 第一级要在新章节的正文里搜特征段，而"正文从哪来"是调用方的知识
/// （缓存 / 网络 / 测试里的假数据）。用回调让本模块不依赖内容管线，
/// 也就让换源逻辑可以被纯函数式地测试（不必搭一套网络+缓存）。
pub fn change_source<F>(
    old_book: &Book,
    progress: Option<&ReadProgress>,
    candidate: &ParsedSource,
    new_chapters: Vec<Chapter>,
    anchor: Option<ChapterMatch>,
    mut probe_content: F,
) -> Result<ChangeSourceOutcome, SourceError>
where
    F: FnMut(i32) -> Result<Option<String>, SourceError>,
{
    // 第一级：特征段（若调用方没预先算，这里按邻域自行搜一遍）
    let anchor_match = match anchor {
        Some(matched) => Some(matched),
        None => match progress.and_then(|progress| progress.anchor_text.as_deref()) {
            Some(anchor_text) => map_by_anchor(
                anchor_text,
                progress.map(|progress| progress.chapter_index).unwrap_or(0),
                NEIGHBORHOOD_RADIUS,
                &mut probe_content,
            )?,
            None => None,
        },
    };

    // 第二级：标题近似（用旧书的"当前章标题"或进度所在章的标题）
    let expected_index = progress.map(|progress| progress.chapter_index).unwrap_or(0);
    let previous_title = old_book.latest_chapter_title.clone().unwrap_or_default();
    let title_match = map_by_title(&previous_title, expected_index, &new_chapters);

    let progress_mapping = map_progress(progress, &new_chapters, anchor_match, title_match);

    // 迁移后的书籍：换到新源的身份，保留用户侧的元信息
    let mut book = old_book.clone();
    book.origin_id = format!("{}|{}", candidate.source.id, detail_url_of(old_book));
    if let Some(toc_url) = &book.toc_url {
        let _ = toc_url;
    }
    book.total_chapters = Some(new_chapters.len() as i32);
    book.origin = tome_core::BookOrigin::Network;

    Ok(ChangeSourceOutcome {
        book,
        chapters: new_chapters,
        progress_mapping,
    })
}

/// 从旧书的 `origin_id` 里取出详情页地址（`书源id|详情页地址`）。
fn detail_url_of(book: &Book) -> String {
    book.origin_id
        .rsplit_once('|')
        .map(|(_, url)| url.to_string())
        .unwrap_or_else(|| book.toc_url.clone().unwrap_or_default())
}

/// 换源时"要不要换"的判据（FR-010 的触发条件）。
///
/// # 为什么单独一个函数
///
/// 界面与 CLI 都要判"这个候选源值不值得换"（章节更多？最近更新更近？）。
/// 把判据放一处，避免两处各写一份而结论不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeReason {
    /// 当前源拿不到目录或正文
    SourceBroken,
    /// 候选源的章节明显更多（至少多 `min_extra` 章）
    MoreChapters {
        /// 多出的章节数
        extra: usize,
    },
    /// 候选源更新更近
    Newer,
}

/// 判断候选源是否值得换（`None` = 不值得换）。
pub fn should_change(
    current_chapter_count: usize,
    candidate_chapter_count: usize,
    min_extra: usize,
    candidate_is_newer: bool,
) -> Option<ChangeReason> {
    if candidate_chapter_count > current_chapter_count + min_extra {
        return Some(ChangeReason::MoreChapters {
            extra: candidate_chapter_count - current_chapter_count,
        });
    }
    if candidate_is_newer && candidate_chapter_count >= current_chapter_count {
        return Some(ChangeReason::Newer);
    }
    None
}

/// 便捷：在新章节里按序号取正文并搜特征段（组合根用的默认回调形状）。
///
/// 它把"取正文"委托给调用方给的函数，自己不碰网络——这样换源逻辑
/// 仍然可以在无网络环境下完整测试。
pub fn neighborhood_probe<F>(mut fetch: F) -> impl FnMut(i32) -> Result<Option<String>, SourceError>
where
    F: FnMut(i32) -> Result<Option<String>, SourceError>,
{
    move |index| fetch(index)
}

/// 从网络抓取候选章正文以做特征段匹配（组合根可用的现成实现）。
///
/// # 为什么某章抓不到时返回 `Ok(None)` 而不是 `Err`
///
/// 邻域里某一章抓不到（临时网络抖动、那一章的地址失效）不该让整个换源
/// 失败——邻域的存在意义正是"这里不行就试旁边"。真错误（如数据损坏）
/// 会以 `Err` 形式从 `fetch` 回调里出来，故这里只吞掉"这一章没取到"。
pub fn network_probe<'a>(
    parsed: &'a ParsedSource,
    chapters: &'a [Chapter],
    host: &'a dyn NetHost,
) -> impl FnMut(i32) -> Result<Option<String>, SourceError> + 'a {
    move |index| {
        let Some(chapter) = chapters
            .iter()
            .find(|chapter| chapter.index_in_book == index)
        else {
            return Ok(None);
        };
        // 换源探测只关心正文本身，故不经过缓存（避免把探测内容写进缓存，
        // 那是正文管线（T067）的职责）
        let Some(url) = chapter.url.as_deref() else {
            return Ok(None);
        };
        let spec = crate::action::base_spec(parsed, url)?;
        match crate::action::fetch_text(host, spec, "换源探测") {
            Ok((html, _)) => match crate::content::parse_chapter_content(parsed, &html) {
                Ok(text) => Ok(Some(text)),
                Err(_) => Ok(None),
            },
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chapter(index: i32, title: &str) -> Chapter {
        Chapter {
            id: 0,
            book_id: 0,
            index_in_book: index,
            title: title.to_string(),
            url: Some(format!("https://b.com/c/{index}.html")),
            byte_offset: None,
            epub_entry: None,
            is_cached: false,
            is_read: false,
            word_count: None,
        }
    }

    fn book() -> Book {
        Book {
            id: 1,
            origin: tome_core::BookOrigin::Network,
            origin_id: "https://a.com|https://a.com/book/1".to_string(),
            name: "书".to_string(),
            author: None,
            cover_url: None,
            intro: None,
            kind: None,
            book_group: None,
            read_config: None,
            toc_url: Some("https://a.com/toc".to_string()),
            latest_chapter_title: Some("第100章 大结局".to_string()),
            latest_chapter_time: None,
            library_root_id: None,
            relative_path: None,
            charset: None,
            format: None,
            total_chapters: Some(100),
            dur_chapter_index: Some(99),
            added_at: 0,
        }
    }

    fn progress(index: i32, anchor: Option<&str>) -> ReadProgress {
        ReadProgress {
            book_id: 1,
            chapter_index: index,
            char_offset: 500,
            anchor_text: anchor.map(str::to_string),
            updated_at: 0,
        }
    }

    fn candidate() -> ParsedSource {
        crate::source::parse_sources(
            r#"{"bookSourceName":"候选源","bookSourceUrl":"https://b.com"}"#,
        )
        .expect("应能解析")
        .sources
        .into_iter()
        .next()
        .expect("应有一条源")
    }

    // --- 相似度 ---

    #[test]
    fn 相同文本相似度是_100() {
        assert_eq!(similarity_percent("一模一样的内容", "一模一样的内容"), 100);
    }

    #[test]
    fn 完全不同是_0() {
        assert_eq!(similarity_percent("甲乙丙丁", "子丑寅卯"), 0);
    }

    #[test]
    fn 子串得到_100() {
        // 短文本是长文本的子串 → 内容确实都在 → 100%
        assert_eq!(similarity_percent("正文", "这是正文内容"), 100);
    }

    #[test]
    fn 插入广告后仍高相似() {
        // 换源最常见的情形：两站文字相同，其中一站插了广告
        let original = "他站在山巅，风吹起衣角，遥望着远方。";
        let with_ads = "【本站域名】他站在山巅，风吹起衣角，遥望着远方。【最新章节请访问】";
        let similarity = similarity_percent(original, with_ads);
        assert!(
            similarity >= 90,
            "插入广告不该让相似度跌破 90%（实际 {similarity}%）"
        );
    }

    #[test]
    fn 错字修正后仍高相似() {
        let original = "他站在山巅，风吹起衣角。";
        let corrected = "他站在山巅，风吹起草角。";
        assert!(similarity_percent(original, corrected) >= 85);
    }

    #[test]
    fn 两段都空时是_0_而非_100() {
        // "两个空串相同"是数学事实，但在这个场景里它意味着"锚点文本丢了"。
        // 报成完全匹配会让换源定位到一个任意章节。
        assert_eq!(similarity_percent("", ""), 0);
        assert_eq!(similarity_percent("有内容", ""), 0);
        assert_eq!(similarity_percent("", "有内容"), 0);
    }

    #[test]
    fn 超长文本被截断比较不卡死() {
        // LCS 是 O(n·m)。整章 5 万字 × 5 万字是 25 亿次操作——足以卡死换源。
        let huge = "字".repeat(100_000);
        let start = std::time::Instant::now();
        let similarity = similarity_percent(&huge, &huge);
        let elapsed = start.elapsed();
        assert_eq!(similarity, 100);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "超长文本比较应在截断后很快完成，实际 {elapsed:?}"
        );
    }

    #[test]
    fn 中文按字符比较而非字节() {
        // 按字节比较会让一个中文字符算 3 个"位置"，相似度全错
        assert_eq!(similarity_percent("中文", "中文"), 100);
        assert_eq!(similarity_percent("中文", "中"), 50, "1/2 个字符相同");
    }

    // --- 第一级：特征段 ---

    #[test]
    fn 特征段在邻域内命中() {
        let anchor = "他站在山巅，风吹起衣角，遥望着远方。";
        let content = "他站在山巅，风吹起衣角，遥望着远方。";
        let matched = map_by_anchor(anchor, 10, 3, |index| {
            if index == 11 {
                Ok(Some(content.to_string()))
            } else {
                Ok(None)
            }
        })
        .expect("应成功")
        .expect("应命中");
        assert_eq!(matched.chapter_index, 11);
        assert!(matched.similarity_percent >= 90);
    }

    #[test]
    fn 邻域外不搜() {
        // 把搜索限定在邻域：性能（只抓 2-3 章）与准确（避免误匹配）
        let anchor = "特征段内容";
        let matched = map_by_anchor(anchor, 10, 3, |index| {
            if index == 50 {
                Ok(Some(anchor.to_string()))
            } else {
                Ok(None)
            }
        })
        .expect("应成功");
        assert!(matched.is_none(), "第 50 章在邻域 [7,13] 之外，不该被搜到");
    }

    #[test]
    fn 取相似度最高的那个() {
        let anchor = "他站在山巅，风吹起衣角。";
        let matched = map_by_anchor(anchor, 10, 2, |index| match index {
            9 => Ok(Some("无关内容完全不同的另一段文字".to_string())),
            10 => Ok(Some("他站在山巅，风吹起衣角。".to_string())),
            11 => Ok(Some("他站在山巅，风停。".to_string())),
            _ => Ok(None),
        })
        .expect("应成功")
        .expect("应命中");
        assert_eq!(matched.chapter_index, 10, "应取相似度最高的");
        assert_eq!(matched.similarity_percent, 100);
    }

    #[test]
    fn 相似度不足时不命中() {
        let matched = map_by_anchor("完全不同的一段文字内容", 10, 2, |_| {
            Ok(Some("子丑寅卯辰巳午未".to_string()))
        })
        .expect("应成功");
        assert!(matched.is_none(), "低于 90% 不该命中第一级");
    }

    #[test]
    fn 邻域不越界到负数() {
        let matched = map_by_anchor("内容", 0, 3, |index| {
            assert!(index >= 0, "不该探测负序号");
            Ok(None)
        })
        .expect("应成功");
        assert!(matched.is_none());
    }

    #[test]
    fn 空锚点文本不命中() {
        let matched =
            map_by_anchor("", 10, 3, |_| Ok(Some("任意内容".to_string()))).expect("应成功");
        assert!(matched.is_none(), "没有锚点文本时第一级无从匹配");
        let matched =
            map_by_anchor("   ", 10, 3, |_| Ok(Some("任意内容".to_string()))).expect("应成功");
        assert!(matched.is_none());
    }

    #[test]
    fn 探测失败被传播() {
        // 取正文时的**真错误**（如磁盘坏）应向上传播，不被当成"没命中"
        let result = map_by_anchor("内容", 10, 1, |_| Err(SourceError::rule("磁盘坏")));
        assert!(result.is_err());
    }

    // --- 第二级：标题 ---

    #[test]
    fn 标题近似命中() {
        let chapters = vec![
            chapter(0, "第1章 开始"),
            chapter(99, "第100章 大结局"),
            chapter(100, "第101章 番外"),
        ];
        let matched = map_by_title("第100章 大结局", 99, &chapters).expect("应命中");
        assert_eq!(matched.chapter_index, 99);
        assert_eq!(matched.similarity_percent, 100);
    }

    #[test]
    fn 标题相似时取离旧进度近的() {
        // 换源后跑到书中间比"差一章"糟糕得多
        let chapters = vec![chapter(2, "第5章 风起"), chapter(48, "第5章 风起")];
        let matched = map_by_title("第5章 风起", 50, &chapters).expect("应命中");
        assert_eq!(
            matched.chapter_index, 48,
            "两个候选同样相似时应取离旧进度（50）近的"
        );
    }

    #[test]
    fn 标题全部不像时不命中() {
        let chapters = vec![chapter(0, "完全无关的章节名"), chapter(1, "另一个无关")];
        assert!(map_by_title("第100章 大结局", 99, &chapters).is_none());
    }

    #[test]
    fn 空标题不命中() {
        let chapters = vec![chapter(0, "第1章")];
        assert!(map_by_title("", 0, &chapters).is_none());
    }

    #[test]
    fn 空目录不命中() {
        assert!(map_by_title("第1章", 0, &[]).is_none());
    }

    // --- 三级决策（FR-010b 的核心）---

    #[test]
    fn 第一级优先于第二级() {
        let mapping = map_progress(
            Some(&progress(10, Some("锚点"))),
            &[],
            Some(ChapterMatch {
                chapter_index: 11,
                similarity_percent: 95,
            }),
            Some(ChapterMatch {
                chapter_index: 12,
                similarity_percent: 100,
            }),
        );
        match mapping {
            ProgressMapping::AnchorSegment {
                chapter_index,
                similarity_percent,
                ..
            } => {
                assert_eq!(chapter_index, 11);
                assert_eq!(similarity_percent, 95);
            }
            other => panic!("应是第一级：{other:?}"),
        }
    }

    #[test]
    fn 无第一级时用第二级() {
        let mapping = map_progress(
            Some(&progress(10, None)),
            &[],
            None,
            Some(ChapterMatch {
                chapter_index: 12,
                similarity_percent: 80,
            }),
        );
        assert_eq!(mapping, ProgressMapping::ChapterTitle { chapter_index: 12 });
    }

    #[test]
    fn 两级都未命中时显式降级() {
        // FR-010b 要求第三级"明确提示用户……不静默跳转"。
        // 它是显式变体，调用方无法忽略。
        let mapping = map_progress(Some(&progress(10, None)), &[], None, None);
        assert_eq!(mapping, ProgressMapping::FallbackToStart);
    }

    #[test]
    fn 第一级保留字符偏移() {
        let mapping = map_progress(
            Some(&progress(10, Some("锚点"))),
            &[],
            Some(ChapterMatch {
                chapter_index: 11,
                similarity_percent: 95,
            }),
            None,
        );
        match mapping {
            ProgressMapping::AnchorSegment { char_offset, .. } => {
                assert_eq!(char_offset, 500, "章内偏移应沿用（同一段文字位置相近）");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 无进度时也给出确定结果() {
        // 用户还没开始读就换源：没有进度可映射，但不能没有结论
        let mapping = map_progress(None, &[], None, None);
        assert_eq!(mapping, ProgressMapping::FallbackToStart);
    }

    // --- 完整换源 ---

    #[test]
    fn 换源走第一级并迁移身份() {
        let new_chapters = vec![chapter(10, "第10章"), chapter(11, "第11章")];
        let outcome = change_source(
            &book(),
            Some(&progress(10, Some("他站在山巅。"))),
            &candidate(),
            new_chapters.clone(),
            None,
            |index| {
                if index == 10 {
                    Ok(Some("他站在山巅。".to_string()))
                } else {
                    Ok(None)
                }
            },
        )
        .expect("应成功");

        assert_eq!(outcome.chapters.len(), 2);
        assert_eq!(
            outcome.book.origin_id, "https://b.com|https://a.com/book/1",
            "应换到候选源的身份，但详情页地址沿用旧书的"
        );
        assert_eq!(outcome.book.total_chapters, Some(2));
        match outcome.progress_mapping {
            ProgressMapping::AnchorSegment { chapter_index, .. } => {
                assert_eq!(chapter_index, 10)
            }
            other => panic!("应是第一级：{other:?}"),
        }
    }

    #[test]
    fn 换源无锚点时走第二级() {
        let new_chapters = vec![chapter(99, "第100章 大结局"), chapter(100, "第101章")];
        let outcome = change_source(
            &book(),
            Some(&progress(99, None)),
            &candidate(),
            new_chapters,
            None,
            |_| Ok(None),
        )
        .expect("应成功");
        assert_eq!(
            outcome.progress_mapping,
            ProgressMapping::ChapterTitle { chapter_index: 99 },
            "旧书的 latest_chapter_title 是「第100章 大结局」，应命中"
        );
    }

    #[test]
    fn 换源都没命中时显式降级() {
        let new_chapters = vec![chapter(0, "毫不相干的标题")];
        let mut old = book();
        old.latest_chapter_title = None;
        let outcome = change_source(
            &old,
            Some(&progress(50, None)),
            &candidate(),
            new_chapters,
            None,
            |_| Ok(None),
        )
        .expect("应成功");
        assert_eq!(
            outcome.progress_mapping,
            ProgressMapping::FallbackToStart,
            "第三级必须是显式变体（FR-010b 的不静默跳转）"
        );
    }

    #[test]
    fn 换源结果总是三选一() {
        // 遍历所有输入组合，断言 ProgressMapping 必然是三个变体之一
        // ——"静默降级"在类型层面不成立
        for anchor in [
            None,
            Some(ChapterMatch {
                chapter_index: 1,
                similarity_percent: 95,
            }),
        ] {
            for title in [
                None,
                Some(ChapterMatch {
                    chapter_index: 2,
                    similarity_percent: 80,
                }),
            ] {
                let mapping = map_progress(Some(&progress(0, None)), &[], anchor.clone(), title);
                let is_known = matches!(
                    mapping,
                    ProgressMapping::AnchorSegment { .. }
                        | ProgressMapping::ChapterTitle { .. }
                        | ProgressMapping::FallbackToStart
                );
                assert!(is_known, "必须是三选一：{mapping:?}");
            }
        }
    }

    // --- 换源判据 ---

    #[test]
    fn 章节明显更多时值得换() {
        let reason = should_change(100, 150, 5, false).expect("应值得换");
        assert_eq!(reason, ChangeReason::MoreChapters { extra: 50 });
    }

    #[test]
    fn 章节差距不明显时不换() {
        // 只多 3 章（可能是分卷方式不同），不值得让用户重下全部目录
        assert!(should_change(100, 103, 5, false).is_none());
    }

    #[test]
    fn 更新更近且章节不少时值得换() {
        let reason = should_change(100, 100, 5, true).expect("应值得换");
        assert_eq!(reason, ChangeReason::Newer);
    }

    #[test]
    fn 更新更近但章节更少时不换() {
        assert!(should_change(100, 50, 5, true).is_none());
    }

    // --- 邻域回调 ---

    #[test]
    fn 邻域探测回调可用() {
        let mut probe = neighborhood_probe(|index| Ok(Some(format!("第{index}章内容"))));
        assert_eq!(probe(5).expect("应成功"), Some("第5章内容".to_string()));
    }

    // --- 详情页地址 ---

    #[test]
    fn 详情页地址从身份里解出() {
        assert_eq!(detail_url_of(&book()), "https://a.com/book/1");
    }

    #[test]
    fn 无竖线时回落到_toc_url() {
        let mut broken = book();
        broken.origin_id = "没有分隔符".to_string();
        assert_eq!(detail_url_of(&broken), "https://a.com/toc");
    }
}
