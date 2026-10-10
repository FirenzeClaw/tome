//! [DONE] 正文管线（tasks.md T067 / FR-002 第五类动作，FR-030）
//!
//! # 顺序就是全部意义
//!
//! ```text
//!   ① 查 Pack 缓存 ──命中──► 净化 ──► 返回（from_cache = true）
//!        │未命中
//!        ▼
//!   ② 请求正文页 → 解码
//!   ③ 规则求值 → 纯文本
//!   ④ 写 Pack 缓存（内容落盘 fsync **先于**索引提交，FR-030）
//!   ⑤ 净化 ──► 返回（from_cache = false）
//! ```
//!
//! ## 为什么"净化在缓存之后"而不是"净化后缓存"
//!
//! 缓存的应当是**原始正文**，不是净化结果。理由是净化规则会被用户修改
//! （换一条净化规则、启用/停用某条）——若缓存的是净化结果，用户改完规则
//! 后看到的仍是旧文本，且看不出原因。
//!
//! 代价是每次读缓存都要重跑净化。而净化是纯 CPU（无 I/O），在本地是
//! 微秒级的；换来的是"改规则立即生效"这条用户能感知的正确性。
//!
//! ## 缓存写入失败为什么不让整个动作失败
//!
//! 正文已经拿到了，用户要的是"能读这一章"。写缓存失败（磁盘满）报错
//! 会让用户读不了书——而那是**可用性**问题，比"下次还得重抓"严重得多。
//! 故写缓存失败只记录诊断（[`ChapterOutcome::cache_warning`]），正常返回。

use std::sync::Arc;

use tome_core::{Book, Chapter, NetHost};

use crate::action;
use crate::error::SourceError;
use crate::source::ParsedSource;

/// `ruleContent` 的字段名（公开书源格式的约定）。
pub mod content_keys {
    /// 正文内容规则
    pub const CONTENT: &str = "content";
    /// 下一章地址（部分源提供，用于连续阅读）
    pub const NEXT_CONTENT_URL: &str = "nextContentUrl";
}

/// 章节缓存接缝（由组合根注入 `tome-store` 的 Pack 实现）。
///
/// # 为什么这里又是 trait 而不是直接依赖 `tome-store::PackStore`
///
/// `tome-source` 确实依赖 `tome-store`（plan.md 的依赖图如此）。但正文
/// 管线只需"读/写一章内容"这一件事，而 `PackStore` 的完整面（淘汰、
/// 核算、清缓存）都在别的场景用。用最小 trait 表达需求让：
///
/// 1. **测试不必搭库**——一个内存实现即可跑全部正文管线测试
/// 2. **需求可见**——"正文管线要缓存做什么"就在这个 trait 的签名里
///
/// `PackStore` 侧有一行 `impl`（见 `tome-store` 的 `pack_reader` 适配）。
pub trait ChapterCache: Send + Sync {
    /// 读一章的缓存内容；未缓存返回 `Ok(None)`。
    fn get(&self, chapter_id: i64) -> Result<Option<Vec<u8>>, SourceError>;

    /// 写一章的缓存内容。
    ///
    /// # 契约：**崩溃安全由实现方保证**
    ///
    /// 实现方必须"内容落盘并 fsync 先于索引提交"（FR-030）。调用方
    /// 无从检查这一点——它是实现的不变量，不是可选的优化。
    fn put(&self, chapter_id: i64, content: &[u8]) -> Result<(), SourceError>;
}

/// 不缓存（`None` 时的替代，让调用点不必到处写 `if let`）。
pub struct NoChapterCache;

impl ChapterCache for NoChapterCache {
    fn get(&self, _chapter_id: i64) -> Result<Option<Vec<u8>>, SourceError> {
        Ok(None)
    }
    fn put(&self, _chapter_id: i64, _content: &[u8]) -> Result<(), SourceError> {
        Ok(())
    }
}

/// 一次正文取用的结果。
///
/// # 为什么带 `from_cache` 与 `cache_warning`
///
/// - `from_cache`：上层据此展示"离线可读"之类的状态，测试据此断言
///   缓存真的被用上了（只看内容无法区分"命中缓存"与"重抓后恰好相同"）
/// - `cache_warning`：写缓存失败不中断动作（见模块文档），但**不能静默**
///   ——否则"缓存一直没写进去"会表现成"每次都要重抓"，而用户无从知道原因
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterOutcome {
    /// 章节序号
    pub chapter_index: i32,
    /// 章节标题
    pub title: String,
    /// **净化前**的原始正文
    pub raw: String,
    /// 正文页的原始 HTML（`None` = 来自缓存，HTML 未保留）
    ///
    /// # 为什么保留它
    ///
    /// 图片位置（`ImageRef::char_offset`，PM 裁决 OQ#13）要从 HTML 里算
    /// ——`<img>` 在正文元素序列中的位置决定了它插进文本流的偏移。
    /// 从缓存读回时只有纯文本，没有 HTML，故那时算不出位置。
    ///
    /// ## 为什么缓存里不存 HTML
    ///
    /// 缓存的是**正文**（FR-029），HTML 比纯文本大数倍。为图片位置把
    /// HTML 也缓存进去会让缓存体积显著膨胀，与 SC-005 的开销约束冲突。
    /// 折中：缓存路径不产出图片位置（由 provider 的暂存兜住常见情况——
    /// 同一章被连读时命中暂存，不经过缓存路径）。
    pub raw_html: Option<String>,
    /// 内容是否来自缓存
    pub from_cache: bool,
    /// 缓存写入失败时的诊断（`None` = 没出问题或未尝试写入）
    pub cache_warning: Option<String>,
}

impl ChapterOutcome {
    /// 净化前的正文（供净化管线（T069）作为输入）。
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// 正文页 HTML；没有时给空串。
    ///
    /// # 为什么不返回 `Option`
    ///
    /// 调用方（图片锚点提取）拿到空 HTML 时的行为就是"解析出空文档、
    /// 得空锚点列表"——那正是 `Option::None` 该有的下游行为。让它返回
    /// `&str` 省得每个调用点都写一次 `unwrap_or_default()`。
    pub fn raw_html(&self) -> &str {
        self.raw_html.as_deref().unwrap_or("")
    }
}

/// 从正文页 HTML 提取纯文本（**不发请求**，A2 纯化）。
pub fn parse_chapter_content(parsed: &ParsedSource, html: &str) -> Result<String, SourceError> {
    parse_chapter_content_with_js(parsed, html, None)
}

/// [`parse_chapter_content`] 的**带 JS 版本**。
pub fn parse_chapter_content_with_js(
    parsed: &ParsedSource,
    html: &str,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<String, SourceError> {
    let source = &parsed.source;
    let rules = action::parse_rule_object(parsed, "ruleContent", source.rule_content.as_deref())?;

    let Some(rule) = rules.get(content_keys::CONTENT) else {
        return Err(SourceError::incomplete(
            source.id.clone(),
            "ruleContent 缺少 `content` 规则",
        ));
    };
    let _ = rule;

    let document = tome_rule::dom::Document::parse_html(html);
    let engine = match js {
        Some(engine) => tome_rule::css_post::RuleEngine::new(&document).with_js(engine.runtime()),
        None => tome_rule::css_post::RuleEngine::new(&document),
    };

    // 正文规则在**文档根**上求值（正文页只有一章的内容，没有需要区分的
    // 重复结构——与详情同口径，与搜索不同）。
    let values = engine
        .evaluate(rule)
        .map_err(|error| SourceError::rule_eval(&error, "content"))?;

    // ## 多值为什么拼接而不是取第一个
    //
    // 正文规则常写成能匹配多个段落的形态（`@css:.content p`），此时
    // **每段都是正文的一部分**。取第一个会丢掉后面所有段落——表现为
    // "只读到第一段"。搜索/详情那些"单值"语义的字段才该取第一个。
    //
    // 用换行拼接而非直接相连：规则可能匹配到多个 `<p>`，相连会让两个
    // 段落的首尾字粘在一起。
    let text = values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<String>>()
        .join("\n");

    if text.trim().is_empty() {
        return Err(SourceError::rule(format!(
            "正文规则 `content` 未命中任何内容（选择器 `{rule}`）"
        )));
    }
    Ok(text)
}

/// 正文动作的完整执行。
///
/// [DONE] T067 的对外入口。
///
/// `cache` 为 `None` 时不做缓存（纯在线取用）。
pub fn chapter_content(
    parsed: &ParsedSource,
    book: &Book,
    chapter: &Chapter,
    host: &dyn NetHost,
    cache: Option<&dyn ChapterCache>,
) -> Result<ChapterOutcome, SourceError> {
    chapter_content_with_js(parsed, book, chapter, host, cache, None)
}

/// [`chapter_content`] 的带 JS 版本。
pub fn chapter_content_with_js(
    parsed: &ParsedSource,
    book: &Book,
    chapter: &Chapter,
    host: &dyn NetHost,
    cache: Option<&dyn ChapterCache>,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<ChapterOutcome, SourceError> {
    // ① 先查缓存（有 chapter.id 才有意义：未入库的章节没有缓存身份）
    if let Some(cache) = cache {
        if chapter.id != 0 {
            if let Some(bytes) = cache.get(chapter.id)? {
                let raw = String::from_utf8(bytes).map_err(|_| {
                    SourceError::rule(format!(
                        "缓存里的 chapter#{} 不是合法 UTF-8（缓存已损坏）",
                        chapter.id
                    ))
                })?;
                return Ok(ChapterOutcome {
                    chapter_index: chapter.index_in_book,
                    title: chapter.title.clone(),
                    raw,
                    // 缓存里只有正文，没有 HTML（见字段说明）
                    raw_html: None,
                    from_cache: true,
                    cache_warning: None,
                });
            }
        }
    }

    // ②③ 抓取 + 解析
    let url = chapter.url.as_deref().ok_or_else(|| {
        SourceError::incomplete(
            parsed.source.id.clone(),
            format!(
                "章节 `{}` 没有地址（网络书必须有 chapterUrl）",
                chapter.title
            ),
        )
    })?;
    let _ = book;
    let spec = action::base_spec(parsed, url)?;
    let (html, _response) = action::fetch_text(host, spec, "正文")?;
    let raw = parse_chapter_content_with_js(parsed, &html, js)?;

    // ④ 写缓存（失败不中断，见模块文档）
    let mut cache_warning = None;
    if let Some(cache) = cache {
        if chapter.id != 0 {
            if let Err(error) = cache.put(chapter.id, raw.as_bytes()) {
                cache_warning = Some(format!("缓存写入失败（{error}），下次仍需重新抓取"));
            }
        }
    }

    Ok(ChapterOutcome {
        chapter_index: chapter.index_in_book,
        title: chapter.title.clone(),
        raw,
        raw_html: Some(html),
        from_cache: false,
        cache_warning,
    })
}

/// 为空章节 id 时给缓存层的诊断（未入库的章节没有缓存身份）。
pub fn cache_key_hint(chapter: &Chapter) -> Option<String> {
    if chapter.id == 0 {
        Some(format!(
            "章节「{}」尚未入库（id=0），本次不落缓存",
            chapter.title
        ))
    } else {
        None
    }
}

/// 组合根便利：把 `Arc<dyn ChapterCache>` 借用成 `&dyn ChapterCache`。
pub fn cache_ref(cache: &Option<Arc<dyn ChapterCache>>) -> Option<&dyn ChapterCache> {
    cache.as_ref().map(|cache| cache.as_ref())
}

/// `tome_store::PackStore` 的 `ChapterCache` 适配（T067 的接线点）。
///
/// # 为什么 impl 在这里而不在 `tome-store`
///
/// 依赖方向是 `tome-source → tome-store`（plan.md 的依赖图），单向。
/// 若把 impl 放在 `tome-store`，那个 crate 就得认识 `ChapterCache`
/// ——于是它要依赖 `tome-source`，依赖成环。
///
/// 故适配层在**依赖方**这一侧：它能同时看见 `PackStore` 与
/// `ChapterCache`，而两侧各自的形状都不必为对方改变。
///
/// ## 崩溃安全的归属
///
/// `ChapterCache::put` 的契约要求"内容落盘 fsync 先于索引提交"（FR-030）。
/// 本适配**不重新实现**它，而是直接委托给
/// [`tome_store::PackStore::write_chapter`]——那里有完整的不变量与
/// 崩溃安全测试（`crates/tome-store/tests/pack_crash_safety.rs`）。
/// 在适配层重做一遍只会得到第二份可能写错的实现。
pub struct PackChapterCache {
    store: std::sync::Arc<tome_store::PackStore>,
}

impl PackChapterCache {
    /// 包装一个 Pack 容器。
    pub fn new(store: std::sync::Arc<tome_store::PackStore>) -> Self {
        Self { store }
    }

    /// 底层容器（需要淘汰/核算等完整能力时用）。
    pub fn store(&self) -> &std::sync::Arc<tome_store::PackStore> {
        &self.store
    }
}

impl ChapterCache for PackChapterCache {
    fn get(&self, chapter_id: i64) -> Result<Option<Vec<u8>>, SourceError> {
        self.store
            .read_chapter(chapter_id)
            .map_err(|error| SourceError::Network {
                // 缓存层的错误分类原样传递（`Storage` 对上层意味着"去看磁盘"）
                kind: error.to_error_kind(),
            })
    }

    fn put(&self, chapter_id: i64, content: &[u8]) -> Result<(), SourceError> {
        self.store
            .write_chapter(chapter_id, content)
            .map(|_| ())
            .map_err(|error| SourceError::Network {
                kind: error.to_error_kind(),
            })
    }
}

/// 把一个 `PackStore` 包成 `Arc<dyn ChapterCache>`（组合根用）。
pub fn pack_cache(store: std::sync::Arc<tome_store::PackStore>) -> Arc<dyn ChapterCache> {
    Arc::new(PackChapterCache::new(store))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::parse_sources;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tome_core::{BookOrigin, ErrorKind, HeaderMap, RequestSpec, Response};

    const CONTENT_HTML: &str = r#"<html><body>
      <div id="content"><p>第一段正文。</p><p>第二段正文。</p><p>第三段正文。</p></div>
    </body></html>"#;

    fn parsed(rule: &str) -> ParsedSource {
        let text = format!(
            r#"{{"bookSourceName":"S","bookSourceUrl":"https://a.com","ruleContent":{}}}"#,
            serde_json::to_string(rule).expect("应能序列化")
        );
        parse_sources(&text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    fn sample_book() -> Book {
        Book {
            id: 1,
            origin: BookOrigin::Network,
            origin_id: "https://a.com|https://a.com/book/1".to_string(),
            name: "书".to_string(),
            author: None,
            cover_url: None,
            intro: None,
            kind: None,
            book_group: None,
            read_config: None,
            toc_url: Some("https://a.com/toc".to_string()),
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

    fn sample_chapter(id: i64, index: i32) -> Chapter {
        Chapter {
            id,
            book_id: 1,
            index_in_book: index,
            title: format!("第{index}章"),
            url: Some(format!("https://a.com/c/{index}.html")),
            byte_offset: None,
            epub_entry: None,
            is_cached: false,
            is_read: false,
            word_count: None,
        }
    }

    /// 假宿主：记录请求，返回预设正文。
    struct FakeHost {
        body: String,
        status: u16,
        calls: Mutex<usize>,
    }

    impl FakeHost {
        fn ok(body: &str) -> Self {
            Self {
                body: body.to_string(),
                status: 200,
                calls: Mutex::new(0),
            }
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
            *self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            if self.status != 200 {
                return Err(ErrorKind::Network);
            }
            Ok(Response {
                status: self.status,
                headers: HeaderMap::new(),
                body_bytes: self.body.as_bytes().to_vec(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    impl FakeHost {
        fn call_count(&self) -> usize {
            *self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    /// 内存缓存（记录写失败注入）。
    #[derive(Default)]
    struct MemoryCache {
        entries: Mutex<HashMap<i64, Vec<u8>>>,
        fail_puts: bool,
        put_count: Mutex<usize>,
    }

    impl MemoryCache {
        fn failing() -> Self {
            Self {
                fail_puts: true,
                ..Self::default()
            }
        }
        fn len(&self) -> usize {
            self.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
        }
        fn puts(&self) -> usize {
            *self
                .put_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl ChapterCache for MemoryCache {
        fn get(&self, chapter_id: i64) -> Result<Option<Vec<u8>>, SourceError> {
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&chapter_id)
                .cloned())
        }
        fn put(&self, chapter_id: i64, content: &[u8]) -> Result<(), SourceError> {
            *self
                .put_count
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            if self.fail_puts {
                return Err(SourceError::rule("磁盘满"));
            }
            self.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(chapter_id, content.to_vec());
            Ok(())
        }
    }

    // --- 解析 ---

    #[test]
    fn 正文按段落拼接不粘连() {
        // 规则常写成能匹配多个段落的形态，每段都是正文的一部分。
        // 取第一个会丢掉后面段落（表现为"只读到第一段"）；
        // 直接相连会让两段首尾字粘在一起。
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let text = parse_chapter_content(&source, CONTENT_HTML).expect("应成功");
        assert_eq!(text, "第一段正文。\n第二段正文。\n第三段正文。");
    }

    #[test]
    fn 单节点规则也成立() {
        let source = parsed(r#"{"content":"@css:#content@text"}"#);
        let text = parse_chapter_content(&source, CONTENT_HTML).expect("应成功");
        assert!(text.contains("第一段正文。"));
        assert!(text.contains("第三段正文。"));
    }

    #[test]
    fn 空白段被过滤() {
        let html = r#"<div><p>正文</p><p>   </p><p></p><p>尾部</p></div>"#;
        let source = parsed(r#"{"content":"@css:div p@text"}"#);
        let text = parse_chapter_content(&source, html).expect("应成功");
        assert_eq!(text, "正文\n尾部");
    }

    #[test]
    fn 正文未命中时报错() {
        // 空正文入库会让读者打开一章看到空白，且不知道是源坏了还是真没内容
        let source = parsed(r#"{"content":"@css:.not-exist@text"}"#);
        let error = parse_chapter_content(&source, CONTENT_HTML).expect_err("应报错");
        assert!(error.to_string().contains("未命中"), "{error}");
    }

    #[test]
    fn 缺_content_规则报配置不完整() {
        let source = parsed(r#"{"nextContentUrl":"@css:a@href"}"#);
        let error = parse_chapter_content(&source, CONTENT_HTML).expect_err("应报错");
        assert!(error.to_string().contains("content"), "{error}");
    }

    #[test]
    fn 缺_rulecontent_报配置不完整() {
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#;
        let source = parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有源");
        let error = parse_chapter_content(&source, CONTENT_HTML).expect_err("应报错");
        assert!(error.to_string().contains("ruleContent"), "{error}");
    }

    // --- 缓存 ---

    #[test]
    fn 未命中时抓取并写缓存() {
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let cache = MemoryCache::default();

        let outcome = chapter_content(
            &source,
            &sample_book(),
            &sample_chapter(7, 0),
            &host,
            Some(&cache),
        )
        .expect("应成功");

        assert!(!outcome.from_cache, "首次应是抓取");
        assert_eq!(host.call_count(), 1);
        assert_eq!(cache.len(), 1, "应写进缓存");
        assert_eq!(cache.puts(), 1);
        assert_eq!(outcome.cache_warning, None);
    }

    #[test]
    fn 命中缓存时不发请求() {
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let cache = MemoryCache::default();
        let book = sample_book();
        let chapter = sample_chapter(7, 0);

        let first =
            chapter_content(&source, &book, &chapter, &host, Some(&cache)).expect("首次应成功");
        let second =
            chapter_content(&source, &book, &chapter, &host, Some(&cache)).expect("二次应成功");

        assert!(!first.from_cache);
        assert!(second.from_cache, "二次应命中缓存");
        assert_eq!(second.raw, first.raw, "内容应一致");
        assert_eq!(
            host.call_count(),
            1,
            "命中缓存时不得再发请求（这是缓存的意义）"
        );
    }

    #[test]
    fn 缓存存的是净化前的原始正文() {
        // 净化规则会被用户修改。缓存净化结果会让用户改完规则后看到旧文本，
        // 且看不出原因。
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let cache = MemoryCache::default();
        let chapter = sample_chapter(7, 0);
        let outcome = chapter_content(&source, &sample_book(), &chapter, &host, Some(&cache))
            .expect("应成功");

        let cached = cache
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&7)
            .cloned()
            .expect("应有缓存条目");
        assert_eq!(
            String::from_utf8(cached).expect("应是 UTF-8"),
            outcome.raw,
            "缓存内容应与返回的 raw（净化前）逐字一致"
        );
    }

    #[test]
    fn 写缓存失败不中断动作() {
        // 正文已经拿到了，用户要的是"能读这一章"。写缓存失败报错会让用户
        // 读不了书——那是可用性问题，比"下次还得重抓"严重得多。
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let cache = MemoryCache::failing();

        let outcome = chapter_content(
            &source,
            &sample_book(),
            &sample_chapter(7, 0),
            &host,
            Some(&cache),
        )
        .expect("写缓存失败不该让动作失败");

        assert!(!outcome.from_cache);
        assert!(outcome.raw.contains("第一段正文。"), "正文应正常返回");
        assert!(
            outcome.cache_warning.is_some(),
            "但要留下诊断——否则「缓存一直没写进去」会表现成「每次都要重抓」，用户无从知道原因"
        );
        assert!(outcome
            .cache_warning
            .expect("已断言非空")
            .contains("缓存写入失败"));
    }

    #[test]
    fn 未入库的章节不落缓存() {
        // id=0 表示尚未入库，没有缓存身份
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let cache = MemoryCache::default();

        let outcome = chapter_content(
            &source,
            &sample_book(),
            &sample_chapter(0, 5),
            &host,
            Some(&cache),
        )
        .expect("应成功");

        assert_eq!(cache.puts(), 0, "id=0 时不该尝试写缓存");
        assert_eq!(cache.len(), 0);
        assert!(outcome.cache_warning.is_none(), "不写也不该报警告");
    }

    #[test]
    fn 无缓存时不报错() {
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let outcome = chapter_content(&source, &sample_book(), &sample_chapter(7, 0), &host, None)
            .expect("应成功");
        assert!(!outcome.from_cache);
        assert_eq!(host.call_count(), 1);
    }

    #[test]
    fn 缓存损坏时报错而非静默() {
        // 缓存里存了非 UTF-8 字节（不该发生，但外部工具可能改坏文件）
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let cache = MemoryCache::default();
        cache
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(7, vec![0xFF, 0xFE, 0x00]);

        let error = chapter_content(
            &source,
            &sample_book(),
            &sample_chapter(7, 0),
            &host,
            Some(&cache),
        )
        .expect_err("应报错");
        assert!(error.to_string().contains("损坏"), "{error}");
    }

    // --- 其他 ---

    #[test]
    fn 章节无地址时报配置不完整() {
        let source = parsed(r#"{"content":"@css:#content@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let mut chapter = sample_chapter(1, 0);
        chapter.url = None;
        let error =
            chapter_content(&source, &sample_book(), &chapter, &host, None).expect_err("应报错");
        assert!(
            error.to_string().contains("地址") || error.to_string().contains("chapterUrl"),
            "{error}"
        );
    }

    #[test]
    fn 请求失败时不写缓存() {
        let source = parsed(r#"{"content":"@css:#content@text"}"#);
        let host = FakeHost {
            body: String::new(),
            status: 500,
            calls: Mutex::new(0),
        };
        let cache = MemoryCache::default();
        chapter_content(
            &source,
            &sample_book(),
            &sample_chapter(7, 0),
            &host,
            Some(&cache),
        )
        .expect_err("应报错");
        assert_eq!(cache.puts(), 0, "抓取失败时不该写缓存");
    }

    #[test]
    fn 结果携带章节序号与标题() {
        let source = parsed(r#"{"content":"@css:#content p@text"}"#);
        let host = FakeHost::ok(CONTENT_HTML);
        let outcome = chapter_content(&source, &sample_book(), &sample_chapter(7, 42), &host, None)
            .expect("应成功");
        assert_eq!(outcome.chapter_index, 42);
        assert_eq!(outcome.title, "第42章");
        assert_eq!(outcome.raw(), outcome.raw.as_str());
    }

    #[test]
    fn 缓存键提示可用() {
        assert!(cache_key_hint(&sample_chapter(0, 0)).is_some());
        assert!(cache_key_hint(&sample_chapter(5, 0)).is_none());
    }

    #[test]
    fn 空实现是安全空操作() {
        assert!(NoChapterCache.get(1).expect("应成功").is_none());
        NoChapterCache.put(1, b"x").expect("应成功");
    }

    #[test]
    fn 真实_pack_容器经接缝可用() {
        // 接缝的端到端验证：`PackChapterCache` 委托给 `PackStore`，
        // 而后者的崩溃安全不变量由 tome-store 自己的故障注入测试守着。
        // 本用例只需证明"接缝是通的、数据真的落到了容器里"。
        let dir = std::env::temp_dir().join(format!(
            "tome-content-pack-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = tome_store::Db::open(&dir.join("tome.db")).expect("应能开库");

        // Pack 要求章节先入库（`write_chapter` 会查 chapter 表确定属于哪本书）
        let (book_id, chapter_id): (i64, i64) = db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at) VALUES (0,'x','书',0)",
                        [],
                    )
                    .map_err(tome_store::db::sql)?;
                let book_id = connection.last_insert_rowid();
                connection
                    .execute(
                        "INSERT INTO chapter(book_id, index_in_book, title) VALUES (?1,0,'章')",
                        [book_id],
                    )
                    .map_err(tome_store::db::sql)?;
                Ok((book_id, connection.last_insert_rowid()))
            })
            .expect("应能建书与章");

        let store = std::sync::Arc::new(tome_store::PackStore::new(
            &dir.join("cache").join("packs"),
            db.clone(),
        ));
        let cache = PackChapterCache::new(store);
        let _ = book_id;

        // 未命中 → 写入 → 命中
        assert!(ChapterCache::get(&cache, chapter_id)
            .expect("应成功")
            .is_none());
        ChapterCache::put(&cache, chapter_id, "正文内容".as_bytes()).expect("应能写");
        let read = ChapterCache::get(&cache, chapter_id)
            .expect("应成功")
            .expect("应命中");
        assert_eq!(String::from_utf8(read).expect("应是 UTF-8"), "正文内容");

        // 经 trait 对象也能用（组合根的形态）
        let boxed: Arc<dyn ChapterCache> = pack_cache(std::sync::Arc::new(
            tome_store::PackStore::new(&dir.join("cache").join("packs"), db.clone()),
        ));
        assert_eq!(
            String::from_utf8(
                ChapterCache::get(boxed.as_ref(), chapter_id)
                    .expect("应成功")
                    .expect("应命中")
            )
            .expect("应是 UTF-8"),
            "正文内容",
            "trait 对象路径应看到同一份缓存"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 缓存借用辅助可用() {
        let cache: Option<Arc<dyn ChapterCache>> = Some(Arc::new(MemoryCache::default()));
        assert!(cache_ref(&cache).is_some());
        let none: Option<Arc<dyn ChapterCache>> = None;
        assert!(cache_ref(&none).is_none());
    }
}
