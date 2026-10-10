//! [DONE] `ContentProvider` 的网络书 adapter（tasks.md T074 / A4 / SC-008）
//!
//! # 这个 adapter 存在的意义
//!
//! `contracts/seams.md` §2 要求 `ContentProvider` 有**两个** adapter
//! （网络书 / 本地书）。两个实现者才证明它是**真 seam** 而不是假想 seam
//! ——只有一个实现的 trait 与直接调用具体类型没有区别。
//!
//! 本地书 adapter 在 `tome-local`（Phase 7）。本模块是网络书那一半。
//!
//! # 三件事在 seam **之下**统一发生（上层看不到）
//!
//! | 事项 | 在哪发生 | 为什么上层不该看到 |
//! |---|---|---|
//! | 缓存查/写 | [`crate::content`] | 缓存是性能手段，不是语义 |
//! | 净化（替换→高亮→词典） | [`crate::purify`] | FR-014 要求网络书与本地书**一视同仁**；参数泄漏到接缝就会让两类书各写一遍 |
//! | 抓取 | [`crate::content`] | 上层说"我要这一章的正文"，不该关心它从哪来 |
//!
//! # SC-008：锚点解析与排版无关
//!
//! [`NetworkContentProvider::resolve_anchor`] 的**签名里没有任何排版参数**
//! ——那是 SC-008 在类型层面的落地。排版参数（字号/行距/窗口尺寸）只影响
//! "字符 → 像素"的映射，而锚点是字符区间，两者本就不在一个坐标系里。
//!
//! 故"任意排版参数下解析为同一锚点"不是靠实现小心翼翼做到的，而是
//! **结构上不可能不同**（见 `tests/anchor_invariance.rs`）。

use std::sync::Arc;

use tome_core::seam::{AnchorQuery, ChapterContent, ContentProvider};
use tome_core::{Book, Chapter, ErrorKind, NetHost, ReadProgress};

use crate::content::{ChapterCache, ChapterOutcome};
use crate::error::SourceError;
use crate::purify::{Purifier, PurifyTarget, SharedPurifier};
use crate::source::{ParsedSource, SourceLookup};

/// 网络书的 [`ContentProvider`] adapter。
///
/// 持有一份**书源查找能力**（`book.origin_id` 里含书源 id，要据此拿到源定义）。
pub struct NetworkContentProvider {
    /// 书源查找（按 id 取 `ParsedSource`）
    sources: Arc<dyn SourceLookup>,
    /// 网络能力
    host: Arc<dyn NetHost>,
    /// 章节缓存（`None` = 不缓存）
    cache: Option<Arc<dyn ChapterCache>>,
    /// 净化器（阅读路径上共用一份）
    purifier: SharedPurifier,
    /// 章节解析结果的暂存（供 `resolve_anchor` 使用）。
    ///
    /// # 为什么要暂存
    ///
    /// `resolve_anchor` 的语义是"章节序号的**微调**"（换源/重排后找回
    /// 原来的位置），它需要读到邻近章节的正文。若每次都重新抓取，一次
    /// 锚点解析会变成多次网络往返——而锚点解析发生在**每次打开书**时。
    ///
    /// 用 `Mutex<HashMap>` 而不是每次重抓：命中缓存时零成本，未命中时
    /// 回落去抓（见 [`NetworkContentProvider::chapter_text`]）。
    ///
    /// `[UNCERTAIN]` 这个暂存没有上限。真实使用下它最多持有"一本书的
    /// 邻域几章"（几 MB），而进程生命周期内读过的书也不多。若日后发现
    /// 内存增长不可控，应换成 LRU 而非继续放大。
    /// 暂存内容 = **原始正文 + 图片锚点**。
    ///
    /// # 为什么图片锚点也要缓存
    ///
    /// 锚点是相对**原始正文**的字符偏移。若只缓存正文、每次重算锚点，
    /// 就得保留原始 HTML——那比锚点本身大得多。锚点是小结构
    /// （URL + 偏移 + alt），随正文一起缓存最省。
    memo: std::sync::Mutex<
        std::collections::HashMap<(String, i32), (String, Vec<crate::image::ImageAnchor>)>,
    >,
}

impl std::fmt::Debug for NetworkContentProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetworkContentProvider")
            .field("has_cache", &self.cache.is_some())
            .finish_non_exhaustive()
    }
}

impl NetworkContentProvider {
    /// 构造。
    pub fn new(
        sources: Arc<dyn SourceLookup>,
        host: Arc<dyn NetHost>,
        cache: Option<Arc<dyn ChapterCache>>,
        purifier: SharedPurifier,
    ) -> Self {
        Self {
            sources,
            host,
            cache,
            purifier,
            memo: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 不净化、不缓存的实例（测试与"只想拿原始文本"的场景）。
    pub fn bare(sources: Arc<dyn SourceLookup>, host: Arc<dyn NetHost>) -> Self {
        Self::new(sources, host, None, Arc::new(Purifier::empty()))
    }

    /// 从 `book.origin_id` 里解出书源 id。
    ///
    /// `data-model.md` 的格式是 `书源id|书籍详情URL`。书源 id 是 URL，
    /// 其中不含 `|`（URL 里的 `|` 必须百分号编码），故按**第一个** `|`
    /// 切分是安全的。
    pub fn source_id_of(book: &Book) -> Result<&str, ErrorKind> {
        book.origin_id
            .split_once('|')
            .map(|(source_id, _)| source_id)
            .filter(|source_id| !source_id.is_empty())
            .ok_or(ErrorKind::Rule)
    }

    /// 取书源定义。
    fn source_of(&self, book: &Book) -> Result<ParsedSource, ErrorKind> {
        let source_id = Self::source_id_of(book)?;
        self.sources.find(source_id).ok_or(ErrorKind::Rule)
    }

    /// 取书源定义（本 crate 的错误类型，带上下文）。
    ///
    /// 与 [`Self::source_of`] 并存：接缝方法要返回 `ErrorKind`，而管线内部
    /// 的报错需要"是哪本书的书源没找到"这类上下文。两个薄包装让两侧各取
    /// 所需，而不是在一处牺牲表达能力。
    fn source_or_error(&self, book: &Book) -> Result<ParsedSource, SourceError> {
        self.source_of(book)
            .map_err(|_| SourceError::incomplete(book.origin_id.clone(), "找不到这本书对应的书源"))
    }

    /// 取某章的**原始**正文（未净化），并写入暂存。
    ///
    /// # 缓存与网络的顺序
    ///
    /// 与 [`crate::content::chapter_content`] 一致（那是唯一的正文取用路径）
    /// ——本函数只是它的"按章节序号"包装。
    fn chapter_text(&self, book: &Book, chapter_index: i32) -> Result<String, SourceError> {
        let parsed = self.source_or_error(book)?;
        let Some(chapter) = self.chapter_at(book, chapter_index)? else {
            return Err(SourceError::rule(format!(
                "书籍 `{}` 没有第 {chapter_index} 章",
                book.name
            )));
        };

        let outcome = crate::content::chapter_content(
            &parsed,
            book,
            &chapter,
            self.host.as_ref(),
            self.cache.as_deref(),
        )?;
        // 这里只要文本（锚点解析用），故不提取图片锚点——
        // 白算一遍锚点只为丢掉它是浪费。`chapter_content` 那条路径才需要图。
        self.remember(&parsed.source.id, chapter_index, &outcome.raw, &[]);
        Ok(outcome.raw)
    }

    /// 记住一章的文本（供锚点解析复用）。
    fn remember(
        &self,
        source_id: &str,
        chapter_index: i32,
        text: &str,
        anchors: &[crate::image::ImageAnchor],
    ) {
        self.memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                (source_id.to_string(), chapter_index),
                (text.to_string(), anchors.to_vec()),
            );
    }

    /// 取暂存的章节文本与图片锚点。
    fn recall(
        &self,
        source_id: &str,
        chapter_index: i32,
    ) -> Option<(String, Vec<crate::image::ImageAnchor>)> {
        self.memo
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(source_id.to_string(), chapter_index))
            .cloned()
    }

    /// 章节列表。
    ///
    /// # 为什么这里返回 `Err` 而不是"空列表"
    ///
    /// "这本书没有目录"与"目录抓取失败"是完全不同的事实：前者是数据
    /// 问题（用户该换源），后者是网络问题（用户该重试）。返回空列表会
    /// 让两者都表现为"目录是空的"。
    fn chapter_at(&self, book: &Book, chapter_index: i32) -> Result<Option<Chapter>, SourceError> {
        let parsed = self.source_of(book).map_err(|_| {
            SourceError::incomplete(book.origin_id.clone(), "找不到这本书对应的书源")
        })?;
        let chapters = crate::toc::toc(&parsed, book, self.host.as_ref())?;
        Ok(chapters
            .into_iter()
            .find(|chapter| chapter.index_in_book == chapter_index))
    }
}

impl ContentProvider for NetworkContentProvider {
    /// 目录。
    fn toc(&self, book: &Book) -> Result<Vec<Chapter>, ErrorKind> {
        let parsed = self.source_of(book)?;
        crate::toc::toc(&parsed, book, self.host.as_ref()).map_err(|error| error.to_error_kind())
    }

    /// 取某章正文（**净化已应用**）。
    ///
    /// 净化在 seam 之下统一发生（FR-014），故上层看不到净化参数——
    /// 那是本 adapter 与本地书 adapter 共用的语义。
    fn chapter_content(
        &self,
        book: &Book,
        chapter_index: i32,
    ) -> Result<ChapterContent, ErrorKind> {
        let parsed = self.source_of(book)?;
        let source_id = parsed.source.id.clone();

        // 复用暂存（同一章被反复读时不再走缓存/网络）
        let (raw, title, from_cache, anchors) = match self.recall(&source_id, chapter_index) {
            Some((text, anchors)) => (text, String::new(), true, anchors),
            None => {
                let Some(chapter) = self
                    .chapter_at(book, chapter_index)
                    .map_err(|error| error.to_error_kind())?
                else {
                    return Err(ErrorKind::Content);
                };
                let outcome: ChapterOutcome = crate::content::chapter_content(
                    &parsed,
                    book,
                    &chapter,
                    self.host.as_ref(),
                    self.cache.as_deref(),
                )
                .map_err(|error| error.to_error_kind())?;

                // 图片锚点：与正文**同源**产出（同一次 HTML 解析）。
                //
                // ## 为什么取不到就退化为空列表而不是报错
                //
                // SC-014 明确"图片失败 MUST NOT 中断正文获取"。这一条同样
                // 适用于"图片位置算不出来"（书源没写 content 规则、HTML
                // 结构怪异）——读者要的是正文，图是附加物。
                let anchors = crate::image::extract_image_anchors(
                    &parsed,
                    outcome.raw_html(),
                    chapter.url.as_deref().unwrap_or(&source_id),
                )
                .unwrap_or_default();

                self.remember(&source_id, chapter_index, &outcome.raw, &anchors);
                (outcome.raw, outcome.title, outcome.from_cache, anchors)
            }
        };

        // ## 净化在这里发生（seam 之下）
        //
        // 高亮与词典的产物**不在此返回**——`ChapterContent` 只有正文。
        // 高亮区间由 `purify` 单独提供（上层按需取），词典查询在用户
        // 选中词时才发起。把三者塞进返回类型会让每次读正文都白算两份
        // 用不上的数据。
        let purified = self
            .purifier
            .purify_text_only(&raw, PurifyTarget::Content)
            .map_err(|error| error.to_error_kind())?;

        Ok(ChapterContent {
            chapter_index,
            title,
            content: purified.clone(),
            images: crate::image::align_anchors_after_purify(&raw, &purified, &anchors),
            from_cache,
        })
    }

    /// 锚点解析（SC-008）。
    ///
    /// # 签名里没有排版参数（SC-008 的结构性落地）
    ///
    /// 见模块文档。
    ///
    /// # 三级回落
    ///
    /// | 输入 | 行为 |
    /// |---|---|
    /// | 无 `anchor_text` | 直接用 `(chapter_index, char_offset)`（无从校验） |
    /// | 有 `anchor_text` 且命中 | 用它定位（`char_offset` 重算到锚点处） |
    /// | 有 `anchor_text` 但未命中 | 回落到 `(chapter_index, char_offset)` |
    ///
    /// 最后一行**不报错**：锚点文本来自用户读到的那一段，而正文可能因为
    /// 源改版而变了。报错会让用户完全打不开书——回落至少把他送到那一章。
    fn resolve_anchor(&self, book: &Book, anchor: &AnchorQuery) -> Result<(i32, i64), ErrorKind> {
        // 提前校验身份格式（免得后面取正文时才报一个与锚点无关的错）
        let _ = Self::source_id_of(book)?;

        let Some(anchor_text) = anchor.anchor_text.as_deref() else {
            return Ok((anchor.chapter_index, anchor.char_offset));
        };
        if anchor_text.trim().is_empty() {
            return Ok((anchor.chapter_index, anchor.char_offset));
        }

        // 在期望章节里找锚点文本；找不到就原样返回期望位置。
        //
        // ## 为什么不在邻域里搜（与换源的差别）
        //
        // 换源要跨**源**迁移，两站的章节划分可能不同，故需要邻域搜索。
        // 而这里读的是**同一本书**，章节序号没有理由变化——变了说明
        // 目录被抓重排过，那是另一个问题（应由目录冲突处理，不是锚点解析）。
        let text = match self.chapter_text(book, anchor.chapter_index) {
            Ok(text) => text,
            // 取不到这一章的正文：回落而不是报错（见函数文档）
            Err(_) => return Ok((anchor.chapter_index, anchor.char_offset)),
        };

        match locate_text(&text, anchor_text) {
            Some(byte_offset) => Ok((anchor.chapter_index, byte_offset)),
            None => Ok((anchor.chapter_index, anchor.char_offset)),
        }
    }

    /// 预取（顺读预热）。**失败无副作用**——预取是优化。
    fn prefetch(&self, book: &Book, around_index: i32, depth: u32) {
        // ## 为什么整个实现体都是"忽略错误"
        //
        // 预取的语义是"提前把可能要读的几章备好"。失败（网络抖动、
        // 那一章还没发布）只意味着"这次优化没成功"——用户读到时再抓一次
        // 就是了。把预取错误传播出去会让一个可选优化影响主流程。
        //
        // ## 为什么不 spawn 线程
        //
        // `prefetch` 的签名没有返回值（fire-and-forget），但**不该**在
        // 这里起线程：调用方（阅读器）通常已经在自己的工作线程上调用它，
        // 再 spawn 会让"预取"变成不受控的并发来源。顺序预取已经足够
        // （相邻几章很快）。
        let start = around_index;
        for offset in 0..=depth as i32 {
            let index = start + offset;
            if index < 0 {
                continue;
            }
            // 已有暂存则跳过（不重复抓）
            let source_id = match Self::source_id_of(book) {
                Ok(source_id) => source_id.to_string(),
                Err(_) => return,
            };
            if self.recall(&source_id, index).is_some() {
                continue;
            }
            if self.chapter_text(book, index).is_err() {
                // 某一章预取失败即停止（后面几章大概率也会失败——
                // 通常是网络断了）
                break;
            }
        }
    }
}

/// 在文本里定位一段文字，返回它的**字符**偏移。
///
/// # 为什么跳过空白归一化后的比较
///
/// 锚点文本是从渲染出的正文里取的，而渲染层可能已经把换行/缩进归一过。
/// 故先用"完全匹配"，失败后用"忽略空白差异"的匹配——后者能救回
/// "正文里有多余空格"的场景。
///
/// # 返回字符偏移而不是字节偏移
///
/// 与 `ReadProgress::char_offset` 的单位一致（`model/trace.rs` 的说明：
/// 字符区间，不是字节区间）。中文下两者差 3 倍，混淆会让高亮整体偏移。
pub fn locate_text(haystack: &str, needle: &str) -> Option<i64> {
    if needle.is_empty() {
        return None;
    }

    // ① 完全匹配
    if let Some(byte_offset) = haystack.find(needle) {
        return Some(haystack[..byte_offset].chars().count() as i64);
    }

    // ② 忽略空白差异：把两侧的空白都折叠掉再比。
    //
    // ## 为什么要建索引映射
    //
    // 折叠后的位置与原文位置不同，故必须记住"折叠文本的第 i 个字符
    // 对应原文的第几个字符"。否则找到的位置会偏。
    let mut folded = String::with_capacity(haystack.len());
    let mut index_map: Vec<usize> = Vec::with_capacity(haystack.len());
    let mut previous_was_space = false;
    for (char_index, character) in haystack.chars().enumerate() {
        if character.is_whitespace() {
            if previous_was_space {
                continue;
            }
            previous_was_space = true;
            folded.push(' ');
            index_map.push(char_index);
        } else {
            previous_was_space = false;
            folded.push(character);
            index_map.push(char_index);
        }
    }

    let folded_needle: String = {
        let mut result = String::new();
        let mut previous_was_space = false;
        for character in needle.chars() {
            if character.is_whitespace() {
                if previous_was_space {
                    continue;
                }
                previous_was_space = true;
                result.push(' ');
            } else {
                previous_was_space = false;
                result.push(character);
            }
        }
        result
    };

    let byte_offset = folded.find(&folded_needle)?;
    let folded_char_index = folded[..byte_offset].chars().count();
    index_map.get(folded_char_index).map(|index| *index as i64)
}

/// 从 `ReadProgress` 构造 [`AnchorQuery`]（上层便利）。
pub fn anchor_of(progress: &ReadProgress) -> AnchorQuery {
    AnchorQuery {
        chapter_index: progress.chapter_index,
        char_offset: progress.char_offset,
        anchor_text: progress.anchor_text.clone(),
    }
}

/// 把 `(章节序号, 字符偏移)` 写回 `ReadProgress` 的形状（上层便利）。
pub fn progress_of(
    book_id: i64,
    chapter_index: i32,
    char_offset: i64,
    anchor_text: Option<String>,
) -> ReadProgress {
    ReadProgress {
        book_id,
        chapter_index,
        char_offset,
        anchor_text,
        updated_at: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tome_core::{HeaderMap, RequestSpec, Response};

    const TOC_HTML: &str = r#"<html><body><dl>
      <dd><a href="/c/0.html">第0章 起始</a></dd>
      <dd><a href="/c/1.html">第1章 风起</a></dd>
      <dd><a href="/c/2.html">第2章 云涌</a></dd>
      <dd><a href="/c/3.html">第3章 终章</a></dd>
    </dl></body></html>"#;

    fn content_html(index: i32) -> String {
        format!(
            r#"<html><body><div id="c">
              <p>第{index}章 的正文开始。</p>
              <p>这里是第{index}章的中段内容，用来测试锚点定位。</p>
              <p>第{index}章正文结束。</p>
            </div></body></html>"#
        )
    }

    /// 假宿主：按 URL 返回目录或正文。
    struct FakeHost {
        /// 正文是否应失败
        content_fails: bool,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl FakeHost {
        fn new() -> Self {
            Self {
                content_fails: false,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn failing() -> Self {
            Self {
                content_fails: true,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(spec.url.clone());

            // 目录页
            if spec.url.contains("/toc") {
                return Ok(Response {
                    status: 200,
                    headers: HeaderMap::new(),
                    body_bytes: TOC_HTML.as_bytes().to_vec(),
                    detected_charset: Some("utf-8".to_string()),
                });
            }
            if self.content_fails {
                return Err(ErrorKind::Network);
            }
            // 正文页：从 URL 里解出章节序号
            let index = spec
                .url
                .trim_end_matches(".html")
                .rsplit('/')
                .next()
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            Ok(Response {
                status: 200,
                headers: HeaderMap::new(),
                body_bytes: content_html(index).into_bytes(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    /// 书源查找（内存实现）。
    #[derive(Default)]
    struct MapLookup {
        sources: HashMap<String, ParsedSource>,
    }

    impl MapLookup {
        fn with(text: &str) -> Self {
            let parsed = crate::source::parse_sources(text)
                .expect("应能解析")
                .sources
                .into_iter()
                .next()
                .expect("应有一条源");
            let mut sources = HashMap::new();
            sources.insert(parsed.source.id.clone(), parsed);
            Self { sources }
        }
    }

    impl SourceLookup for MapLookup {
        fn find(&self, source_id: &str) -> Option<ParsedSource> {
            self.sources.get(source_id).cloned()
        }
    }

    fn source_text() -> String {
        r#"{"bookSourceName":"网络源","bookSourceUrl":"https://net.test",
            "ruleToc":"{\"chapterList\":\"@css:dd\",\"chapterName\":\"@css:a@text\",\"chapterUrl\":\"@css:a@href\"}",
            "ruleContent":"{\"content\":\"@css:#c p@text\"}"}"#
            .to_string()
    }

    fn book() -> Book {
        Book {
            id: 1,
            origin: tome_core::BookOrigin::Network,
            origin_id: "https://net.test|https://net.test/book/1".to_string(),
            name: "书".to_string(),
            author: None,
            cover_url: None,
            intro: None,
            kind: None,
            book_group: None,
            read_config: None,
            toc_url: Some("https://net.test/book/1/toc".to_string()),
            latest_chapter_title: None,
            latest_chapter_time: None,
            library_root_id: None,
            relative_path: None,
            charset: None,
            format: None,
            total_chapters: None,
            dur_chapter_index: None,
            added_at: 0,
        }
    }

    fn provider(host: Arc<FakeHost>) -> NetworkContentProvider {
        NetworkContentProvider::bare(Arc::new(MapLookup::with(&source_text())), host)
    }

    // --- 书源 id 解析 ---

    #[test]
    fn 从身份解出书源_id() {
        assert_eq!(
            NetworkContentProvider::source_id_of(&book()).expect("应成功"),
            "https://net.test"
        );
    }

    #[test]
    fn 身份格式不对时报规则错误() {
        let mut broken = book();
        broken.origin_id = "没有竖线".to_string();
        assert_eq!(
            NetworkContentProvider::source_id_of(&broken).expect_err("应报错"),
            ErrorKind::Rule
        );
    }

    // --- 目录 ---

    #[test]
    fn 目录经_seam_可用() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        let chapters = provider.toc(&book()).expect("应成功");
        assert_eq!(chapters.len(), 4);
        assert_eq!(chapters[0].title, "第0章 起始");
    }

    #[test]
    fn 找不到书源时报规则错误() {
        let host = Arc::new(FakeHost::new());
        let provider = NetworkContentProvider::bare(
            Arc::new(MapLookup::default()), // 空查找表
            host,
        );
        assert_eq!(provider.toc(&book()).expect_err("应报错"), ErrorKind::Rule);
    }

    // --- 正文 ---

    #[test]
    fn 正文经_seam_可用() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        let content = provider.chapter_content(&book(), 1).expect("应成功");
        assert_eq!(content.chapter_index, 1);
        assert!(content.content.contains("这里是第1章的中段内容"));
        assert!(!content.from_cache, "没有缓存层时不应报命中");
    }

    #[test]
    fn 净化在_seam_之下生效() {
        // FR-014：净化对上层不可见，但确实生效了
        let host = Arc::new(FakeHost::new());
        let purifier = Arc::new(Purifier::new(
            vec![tome_core::ReplaceRule {
                id: 1,
                name: "去广告".to_string(),
                pattern: "正文开始".to_string(),
                replacement: "[净化过]".to_string(),
                scope: tome_core::RuleScope::Content,
                is_regex: false,
                enabled: true,
                sort_order: 0,
            }],
            vec![],
            vec![],
        ));
        let provider = NetworkContentProvider::new(
            Arc::new(MapLookup::with(&source_text())),
            host,
            None,
            purifier,
        );
        let content = provider.chapter_content(&book(), 1).expect("应成功");
        assert!(
            content.content.contains("[净化过]"),
            "净化应已生效：{}",
            content.content
        );
        assert!(!content.content.contains("正文开始"));
    }

    #[test]
    fn 无规则时不改正文() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        let content = provider.chapter_content(&book(), 2).expect("应成功");
        assert!(content.content.contains("第2章 的正文开始。"));
    }

    #[test]
    fn 章节不存在时报内容错误() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        assert_eq!(
            provider.chapter_content(&book(), 99).expect_err("应报错"),
            ErrorKind::Content
        );
    }

    #[test]
    fn 同一章重复读走暂存不发新请求() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host.clone());
        provider.chapter_content(&book(), 1).expect("首次应成功");
        let calls_after_first = host.calls().len();
        provider.chapter_content(&book(), 1).expect("二次应成功");
        assert_eq!(
            host.calls().len(),
            calls_after_first,
            "同一章重复读不该再发请求"
        );
    }

    #[test]
    fn 暂存命中时报来自缓存() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        provider.chapter_content(&book(), 1).expect("首次应成功");
        let second = provider.chapter_content(&book(), 1).expect("二次应成功");
        assert!(second.from_cache, "二次应报命中");
    }

    // --- 锚点解析（SC-008）---

    #[test]
    fn 无锚点文本时原样返回() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        let resolved = provider
            .resolve_anchor(
                &book(),
                &AnchorQuery {
                    chapter_index: 2,
                    char_offset: 42,
                    anchor_text: None,
                },
            )
            .expect("应成功");
        assert_eq!(resolved, (2, 42));
    }

    #[test]
    fn 空锚点文本时原样返回() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        let resolved = provider
            .resolve_anchor(
                &book(),
                &AnchorQuery {
                    chapter_index: 1,
                    char_offset: 7,
                    anchor_text: Some("   ".to_string()),
                },
            )
            .expect("应成功");
        assert_eq!(resolved, (1, 7));
    }

    #[test]
    fn 锚点文本命中时返回其位置() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        let resolved = provider
            .resolve_anchor(
                &book(),
                &AnchorQuery {
                    chapter_index: 1,
                    // 旧偏移是错的（换源/重排后），应被锚点文本纠正
                    char_offset: 999,
                    anchor_text: Some("这里是第1章的中段内容".to_string()),
                },
            )
            .expect("应成功");
        assert_eq!(resolved.0, 1, "章节序号不变");
        assert_ne!(resolved.1, 999, "偏移应被锚点文本纠正");
        assert!(resolved.1 > 0);
    }

    #[test]
    fn 锚点文本未命中时回落到原偏移() {
        // 正文可能因源改版而变了。报错会让用户完全打不开书——
        // 回落至少把他送到那一章。
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        let resolved = provider
            .resolve_anchor(
                &book(),
                &AnchorQuery {
                    chapter_index: 1,
                    char_offset: 55,
                    anchor_text: Some("这段文字在正文里根本不存在".to_string()),
                },
            )
            .expect("应成功（回落而非报错）");
        assert_eq!(resolved, (1, 55));
    }

    #[test]
    fn 取不到正文时也回落() {
        let host = Arc::new(FakeHost::failing());
        let provider = provider(host);
        let resolved = provider
            .resolve_anchor(
                &book(),
                &AnchorQuery {
                    chapter_index: 1,
                    char_offset: 33,
                    anchor_text: Some("任意锚点文本".to_string()),
                },
            )
            .expect("网络失败时应回落而不是报错");
        assert_eq!(resolved, (1, 33));
    }

    // --- 文本定位 ---

    #[test]
    fn 定位返回字符偏移而非字节偏移() {
        // 与 ReadProgress::char_offset 的单位一致。中文下字节是字符的
        // 3 倍，混淆会让高亮整体偏移。
        let text = "中文内容在这里";
        assert_eq!(locate_text(text, "内容"), Some(2), "字符偏移，不是字节偏移");
    }

    #[test]
    fn 定位失败返回_none() {
        assert_eq!(locate_text("文本", "不存在"), None);
        assert_eq!(locate_text("文本", ""), None);
    }

    #[test]
    fn 忽略空白差异的定位() {
        // 渲染层可能已把换行/缩进归一过
        let text = "他站在山巅，\n    风吹起衣角。";
        let needle = "他站在山巅， 风吹起衣角。";
        let located = locate_text(text, needle).expect("应能忽略空白差异后定位");
        assert_eq!(located, 0, "起点在 0");
    }

    #[test]
    fn 空白折叠后的偏移仍指向原文位置() {
        // 折叠会改变位置，故必须建索引映射——否则找到的位置会偏
        let text = "前缀\n\n\n\n后面有一 段文字";
        let needle = "后面有一 段文字";
        let located = locate_text(text, needle).expect("应能定位");
        let expected = text.chars().position(|_| false).unwrap_or(0) + "前缀".chars().count() + 3; // 折叠后的 3 个空白
        let _ = expected;
        // 直接从原文验证：第 located 个字符起应是"后"
        let actual: String = text.chars().skip(located as usize).take(1).collect();
        assert_eq!(actual, "后", "定位必须指向原文里的真实位置");
    }

    // --- 预取 ---

    #[test]
    fn 预取加载邻域章节() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host.clone());
        provider.prefetch(&book(), 1, 2);
        // 应该抓过第 1、2、3 章
        let calls = host.calls();
        assert!(
            calls.iter().any(|url| url.contains("/1.html")),
            "应预取第 1 章：{calls:?}"
        );
        assert!(calls.iter().any(|url| url.contains("/2.html")));
    }

    #[test]
    fn 预取失败不panic也不影响后续() {
        // 预取是优化，失败无副作用
        let host = Arc::new(FakeHost::failing());
        let provider = provider(host);
        provider.prefetch(&book(), 0, 3);
        // 走到这里就说明没 panic
    }

    #[test]
    fn 预取不重复抓已暂存的章() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host.clone());
        provider.chapter_content(&book(), 1).expect("应成功");
        let calls_before = host.calls().len();
        provider.prefetch(&book(), 1, 0);
        assert_eq!(host.calls().len(), calls_before, "已暂存的章不该被重复抓取");
    }

    #[test]
    fn 预取负序号不越界() {
        let host = Arc::new(FakeHost::new());
        let provider = provider(host);
        // 从第 0 章往前不会 panic
        provider.prefetch(&book(), 0, 2);
        provider.prefetch(&book(), -5, 2);
    }

    // --- 便利函数 ---

    #[test]
    fn 锚点查询从进度构造() {
        let progress = ReadProgress {
            book_id: 1,
            chapter_index: 5,
            char_offset: 100,
            anchor_text: Some("锚点".to_string()),
            updated_at: 0,
        };
        let query = anchor_of(&progress);
        assert_eq!(query.chapter_index, 5);
        assert_eq!(query.char_offset, 100);
        assert_eq!(query.anchor_text.as_deref(), Some("锚点"));
    }

    #[test]
    fn 进度从元组构造() {
        let progress = progress_of(7, 3, 42, Some("锚".to_string()));
        assert_eq!(progress.book_id, 7);
        assert_eq!(progress.chapter_index, 3);
        assert_eq!(progress.char_offset, 42);
    }
}
