//! \[DONE] 目录管线（tasks.md T066 / FR-002 第四类动作，SC-003）
//!
//! # 动作的形状
//!
//! ```text
//!   书源 + 书籍（用其 toc_url）
//!     └─ 抓取目录页 → 解码
//!     └─ ruleToc 规则 → Vec<Chapter>
//! ```
//!
//! # SC-003：5 万章 <1s
//!
//! 这条要求约束的是**解析**（拿 HTML 与规则产出章节列表），不是下载——
//! 一个 5 万章的目录页在慢站点上光是传输就不止 1 秒。故本模块把"解析"
//! 做成可独立计时的纯函数 [`parse_toc`]，并由
//! [`tests/sc003_toc_perf.rs`](../../tests/sc003_toc_perf.rs) 端到端实测。
//!
//! ## 解析路径上刻意避免的三件事
//!
//! | 避免 | 原因 | 本实现的做法 |
//! |---|---|---|
//! | 每章重新构造 `RuleEngine` | 构造含 DOM 索引，5 万次会变成 O(n²) | 全局一个 engine，逐节点相对求值 |
//! | 每章重新分配规则表 | `BTreeMap` 克隆是实打实的开销 | 规则表借用，不克隆 |
//! | 逐章 `String` 二次解析 | URL 补全用格式化，不做 `Url::parse` | [`action::absolute_url`] |
//!
//! # 章节序号为什么从 0 起
//!
//! `data-model.md` 的 `chapter.index_in_book` 从 0 起，而目录页上的
//! "第几章"通常从 1 起（且常含卷/分页重排）。故本模块用**列表位置**做
//! 序号——那是唯一无论源站怎么编号都自洽的口径。

use tome_core::{Book, Chapter, NetHost};

use crate::action;
use crate::error::SourceError;
use crate::source::ParsedSource;

/// `ruleToc` 的字段名（公开书源格式的约定）。
pub mod toc_keys {
    /// 章节列表（每个元素是一个章节节点）
    pub const CHAPTER_LIST: &str = "chapterList";
    /// 章节标题
    pub const CHAPTER_NAME: &str = "chapterName";
    /// 章节地址
    pub const CHAPTER_URL: &str = "chapterUrl";
    /// 章节是否 VIP/付费（供上层展示，不参与可读性判断）
    pub const IS_VIP: &str = "isVip";
    /// 章节序号（书源显式给出时优先于列表位置）
    pub const CHAPTER_INDEX: &str = "chapterIndex";
    /// 卷名（分卷目录用；本期不透传到 `Chapter`，仅记录以支持诊断）
    pub const VOLUME_NAME: &str = "volumeName";
}

/// 从目录页 HTML 解析章节列表（**不发请求**，A2 纯化）。
///
/// 这是 SC-003 计时的那个函数。
pub fn parse_toc(
    parsed: &ParsedSource,
    book_url: &str,
    html: &str,
) -> Result<Vec<Chapter>, SourceError> {
    parse_toc_with_js(parsed, book_url, html, None)
}

/// [`parse_toc`] 的**带 JS 版本**（字段规则里的 `@js:`）。
pub fn parse_toc_with_js(
    parsed: &ParsedSource,
    book_url: &str,
    html: &str,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<Vec<Chapter>, SourceError> {
    let source = &parsed.source;
    let rules = action::parse_rule_object(parsed, "ruleToc", source.rule_toc.as_deref())?;

    let document = tome_rule::dom::Document::parse_html(html);
    // ## 引擎只构造一次（SC-003 的关键）
    //
    // 5 万章循环里每章构造一次 `RuleEngine` 会让总开销变成 O(n²)
    // （`RuleEngine::new` 要建立 DOM 的索引结构）。故在循环外建一次。
    let engine = match js {
        Some(engine) => tome_rule::css_post::RuleEngine::new(&document).with_js(engine.runtime()),
        None => tome_rule::css_post::RuleEngine::new(&document),
    };

    let nodes = action::select_nodes(&document, &source.id, &rules, toc_keys::CHAPTER_LIST)?;

    // 容量预分配：省掉 5 万次增长的重新分配
    let mut chapters = Vec::with_capacity(nodes.len());
    // ## 相对 URL 的基准是**书源地址**而不是详情页地址
    //
    // 与 `parse_book_info` 同口径：规则里的相对地址是相对源站根路径书写的
    // （`/book/1/2.html`）。用源站地址做基准让 `absolute_url` 走"base + url"
    // 的快路径——而标题里的 `format!` 在 5 万次下是实打实的开销，故
    // 只在其参数真正需要时才构造（见下）。
    let _ = book_url;

    for (position, node) in nodes.iter().enumerate() {
        // 标题：规则未写时给一个占位而不是跳过这一章。
        //
        // ## 为什么不跳过
        //
        // 跳过会让**后续章节的序号整体前移**，而阅读进度是按 `index_in_book`
        // 记的——那会让用户"读到第 100 章"变成另一章。宁可有标题为空的
        // 条目（可见且可诊断），不可让序号错位（不可见且难查）。
        //
        // ## 为什么占位文案用 `format!` 而正文路径不用
        //
        // `unwrap_or_else` 只在**缺失时**才构造——有标题的章节（绝大多数）
        // 不为它付出分配。故这里不需要为 5 万次分配担心。
        let title = action::evaluate_field(&engine, node, &rules, toc_keys::CHAPTER_NAME)?
            .unwrap_or_else(|| format!("（无标题·第 {} 项）", position + 1));

        // ## 为什么先判"已是绝对地址"再补全
        //
        // 文本目录的 URL 绝大多数有两种形态：绝对地址（`https://...`）与
        // 站内相对地址（`/book/1/2.html`）。前者的补全应**原样返回**——
        // 而 `absolute_url` 内部要先 `contains("://")` 再决定，这个判断
        // 在 5 万次下就是 5 万次子串扫描。这里把它提前，让绝对地址走零
        // 分配路径（直接移动原字符串）。
        let url =
            action::evaluate_field(&engine, node, &rules, toc_keys::CHAPTER_URL)?.map(|raw| {
                if raw.contains("://") {
                    raw
                } else {
                    action::absolute_url(&source.id, &raw)
                }
            });

        // 序号：书源显式给出时**优先**（有些源的目录页分页后列表位置
        // 不等于书内序号），否则用列表位置。
        let index_in_book =
            match action::evaluate_field(&engine, node, &rules, toc_keys::CHAPTER_INDEX)? {
                Some(text) => text.trim().parse::<i32>().unwrap_or(position as i32),
                None => position as i32,
            };

        chapters.push(Chapter {
            id: 0,
            book_id: 0,
            index_in_book,
            title,
            url,
            // 网络书用 URL 定位，不使用本地书的两种定位方式
            byte_offset: None,
            epub_entry: None,
            // 缓存状态由内容管线（T067）维护，目录动作不臆造
            is_cached: false,
            is_read: false,
            word_count: None,
        });
    }

    Ok(chapters)
}

/// 目录动作的完整执行（求值 → 抓取 → 解析）。
///
/// \[DONE] T066 的对外入口。
///
/// `book.toc_url` 为 `None` 时回落到 `book.origin_id` 里记录的详情页地址
/// ——`origin_id` 的格式是 `书源id|详情页地址`（`data-model.md`）。
pub fn toc(
    parsed: &ParsedSource,
    book: &Book,
    host: &dyn NetHost,
) -> Result<Vec<Chapter>, SourceError> {
    toc_with_js(parsed, book, host, None)
}

/// [`toc`] 的带 JS 版本。
pub fn toc_with_js(
    parsed: &ParsedSource,
    book: &Book,
    host: &dyn NetHost,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<Vec<Chapter>, SourceError> {
    let toc_url = directory_url(book).ok_or_else(|| {
        SourceError::incomplete(
            parsed.source.id.clone(),
            "书籍既无 toc_url 也无可解析的 origin_id（无法定位目录页）",
        )
    })?;

    let spec = action::base_spec(parsed, toc_url.clone())?;
    let (html, _response) = action::fetch_text(host, spec, "目录")?;
    parse_toc_with_js(parsed, &toc_url, &html, js)
}

/// 定位目录页地址：优先 `toc_url`，否则从 `origin_id` 里解出详情页地址。
///
/// # 为什么要有这条回落
///
/// `origin_id` 按 `data-model.md` 是 `书源id|书籍详情URL`。而从搜索/发现
/// 入库的书**一定**有它；`toc_url` 则只有跑了详情动作才会有。
/// 没有这条回落，"搜索到一半直接看目录"（CLI 的常见用法）就必然失败。
///
/// ## 书源 id 里可能含 `|` 吗
///
/// 不会——书源 id 是 URL（`bookSourceUrl`），而 URL 里的 `|` 必须被
/// 百分号编码（`%7C`）。故按**最后一个** `|` 切分是安全的：它后面
/// 一定是详情页地址。
pub fn directory_url(book: &Book) -> Option<String> {
    if let Some(url) = book.toc_url.as_deref() {
        if !url.trim().is_empty() {
            return Some(url.to_string());
        }
    }
    let (_, detail_url) = book.origin_id.rsplit_once('|')?;
    if detail_url.trim().is_empty() {
        return None;
    }
    Some(detail_url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::parse_sources;
    use std::sync::Mutex;
    use tome_core::{BookOrigin, ErrorKind, HeaderMap, RequestSpec, Response};

    fn toc_html(chapters: usize) -> String {
        let mut html = String::from(r#"<html><body><div id="list">"#);
        for index in 1..=chapters {
            html.push_str(&format!(
                r#"<dd><a href="/book/1/{index}.html">第{index}章 标题{index}</a></dd>"#
            ));
        }
        html.push_str("</div></body></html>");
        html
    }

    fn parsed(rule: &str) -> ParsedSource {
        let text = format!(
            r#"{{"bookSourceName":"S","bookSourceUrl":"https://a.com","ruleToc":{}}}"#,
            serde_json::to_string(rule).expect("应能序列化")
        );
        parse_sources(&text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    fn sample_book(toc_url: Option<&str>, origin_id: &str) -> Book {
        Book {
            id: 1,
            origin: BookOrigin::Network,
            origin_id: origin_id.to_string(),
            name: "书".to_string(),
            author: None,
            cover_url: None,
            intro: None,
            kind: None,
            book_group: None,
            read_config: None,
            toc_url: toc_url.map(str::to_string),
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

    struct FakeHost {
        body: String,
        status: u16,
        seen: Mutex<Vec<RequestSpec>>,
    }

    impl FakeHost {
        fn ok(body: String) -> Self {
            Self {
                body,
                status: 200,
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(spec);
            Ok(Response {
                status: self.status,
                headers: HeaderMap::new(),
                body_bytes: self.body.as_bytes().to_vec(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    #[test]
    fn 解析章节列表() {
        let source = parsed(
            r#"{"chapterList":"@css:dd","chapterName":"@css:a@text","chapterUrl":"@css:a@href"}"#,
        );
        let chapters = parse_toc(&source, "https://a.com/book/1", &toc_html(3)).expect("应成功");
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[0].title, "第1章 标题1");
        assert_eq!(chapters[0].index_in_book, 0, "序号从 0 起");
        assert_eq!(chapters[2].index_in_book, 2);
        assert_eq!(
            chapters[0].url.as_deref(),
            Some("https://a.com/book/1/1.html"),
            "相对地址应被补全"
        );
    }

    #[test]
    fn 序号是列表位置而非标题里的数字() {
        // 目录页可能分页/分卷重排，标题里的"第几章"不可靠。
        // 列表位置是唯一无论源站怎么编号都自洽的口径。
        let html = r#"<div>
            <dd><a href="/c/1">第 99 章 乱序标题</a></dd>
            <dd><a href="/c/2">第 1 章 另一个乱序</a></dd>
        </div>"#;
        let source = parsed(r#"{"chapterList":"@css:dd","chapterName":"@css:a@text"}"#);
        let chapters = parse_toc(&source, "https://a.com/b", html).expect("应成功");
        assert_eq!(chapters[0].index_in_book, 0);
        assert_eq!(chapters[1].index_in_book, 1);
    }

    #[test]
    fn 书源显式给出序号时优先() {
        // 有些源的目录页分页后列表位置不等于书内序号
        let html = r#"<div>
            <dd data-idx="10"><a href="/c/1">A</a></dd>
            <dd data-idx="11"><a href="/c/2">B</a></dd>
        </div>"#;
        let source = parsed(
            r#"{"chapterList":"@css:dd","chapterName":"@css:a@text",
                "chapterIndex":"@css:dd@attr(data-idx)"}"#,
        );
        let chapters = parse_toc(&source, "https://a.com/b", html).expect("应成功");
        assert_eq!(chapters[0].index_in_book, 10);
        assert_eq!(chapters[1].index_in_book, 11);
    }

    #[test]
    fn 标题缺失时用占位而不跳过() {
        // 跳过会让后续章节序号整体前移，而进度是按 index_in_book 记的
        // ——那会让"读到第 100 章"变成另一章
        let html = r#"<div><dd><a href="/c/1">有标题</a></dd><dd><a href="/c/2"></a></dd></div>"#;
        let source = parsed(
            r#"{"chapterList":"@css:dd","chapterName":"@css:.nope@text","chapterUrl":"@css:a@href"}"#,
        );
        let chapters = parse_toc(&source, "https://a.com/b", html).expect("应成功");
        assert_eq!(chapters.len(), 2, "缺标题不该让章节数减少");
        assert!(
            chapters[0].title.contains("无标题"),
            "{}",
            chapters[0].title
        );
        assert_eq!(chapters[1].index_in_book, 1, "序号不得错位");
    }

    #[test]
    fn 空目录页返回空列表() {
        let source = parsed(r#"{"chapterList":"@css:dd","chapterName":"@css:a@text"}"#);
        let chapters =
            parse_toc(&source, "https://a.com/b", "<html><body></body></html>").expect("应成功");
        assert!(chapters.is_empty(), "没有章节是合法的（不是错误）");
    }

    #[test]
    fn 缺_ruletoc_报配置不完整() {
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#;
        let source = parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有源");
        let error = parse_toc(&source, "https://a.com/b", "<html/>").expect_err("应报错");
        assert!(error.to_string().contains("ruleToc"), "{error}");
    }

    #[test]
    fn 缺章节列表规则报错() {
        let source = parsed(r#"{"chapterName":"@css:a@text"}"#);
        let error = parse_toc(&source, "https://a.com/b", &toc_html(1)).expect_err("应报错");
        assert!(error.to_string().contains("chapterList"), "{error}");
    }

    #[test]
    fn 目录动作用_toc_url_抓取() {
        let source = parsed(
            r#"{"chapterList":"@css:dd","chapterName":"@css:a@text","chapterUrl":"@css:a@href"}"#,
        );
        let book = sample_book(
            Some("https://a.com/book/1/toc"),
            "https://a.com|https://a.com/book/1",
        );
        let host = FakeHost::ok(toc_html(5));
        let chapters = toc(&source, &book, &host).expect("应成功");
        assert_eq!(chapters.len(), 5);

        let seen = host
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(seen[0].url, "https://a.com/book/1/toc");
    }

    #[test]
    fn 无_toc_url_时回落到_origin_id_里的详情页() {
        // 从搜索/发现入库的书一定有 origin_id，而 toc_url 只有跑过详情才有。
        // 没有这条回落，"搜索到一半直接看目录"必然失败。
        let source = parsed(r#"{"chapterList":"@css:dd","chapterName":"@css:a@text"}"#);
        let book = sample_book(None, "https://a.com|https://a.com/book/1");
        let host = FakeHost::ok(toc_html(2));
        toc(&source, &book, &host).expect("应成功");

        let seen = host
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(seen[0].url, "https://a.com/book/1");
    }

    #[test]
    fn 目录页地址解析_书源_id_含冒号斜杠也正确() {
        // 书源 id 是 URL，含 `://` 但不含 `|`（URL 里的 | 必须编码）
        let book = sample_book(None, "https://a.com/x?y=1|https://b.com/book/9");
        assert_eq!(
            directory_url(&book).as_deref(),
            Some("https://b.com/book/9")
        );
    }

    #[test]
    fn 两者都缺时报配置不完整() {
        let source = parsed(r#"{"chapterList":"@css:dd"}"#);
        let book = sample_book(None, "");
        let host = FakeHost::ok(toc_html(1));
        let error = toc(&source, &book, &host).expect_err("应报错");
        assert!(
            error.to_string().contains("origin_id") || error.to_string().contains("toc"),
            "{error}"
        );
    }

    #[test]
    fn 非_2xx_报网络错误() {
        let source = parsed(r#"{"chapterList":"@css:dd"}"#);
        let book = sample_book(Some("https://a.com/toc"), "https://a.com|x");
        let mut host = FakeHost::ok(String::new());
        host.status = 404;
        let error = toc(&source, &book, &host).expect_err("应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Network);
    }

    #[test]
    fn 章节的缓存与已读标记不在目录动作里臆造() {
        let source = parsed(r#"{"chapterList":"@css:dd","chapterName":"@css:a@text"}"#);
        let chapters = parse_toc(&source, "https://a.com/b", &toc_html(2)).expect("应成功");
        for chapter in &chapters {
            assert!(!chapter.is_cached, "缓存状态由内容管线维护");
            assert!(!chapter.is_read, "已读状态由进度维护");
            assert_eq!(chapter.word_count, None, "字数不在目录里臆造");
            assert_eq!(chapter.byte_offset, None, "网络书不用字节偏移");
            assert_eq!(chapter.epub_entry, None);
        }
    }

    #[test]
    fn 目录地址可以用替换链加工() {
        // 公开格式允许在规则里用替换链（`##正则##替换内容`，
        // 见 tests/corpus/css.json 的同款用例及其依据说明）
        let html = r#"<div><dd><a href="/prefix/c/1.html">A</a></dd></div>"#;
        let source = parsed(
            r#"{"chapterList":"@css:dd","chapterName":"@css:a@text",
                "chapterUrl":"@css:a@href##/prefix##"}"#,
        );
        let chapters = parse_toc(&source, "https://a.com/b", html).expect("应成功");
        assert_eq!(
            chapters[0].url.as_deref(),
            Some("https://a.com/c/1.html"),
            "替换链应生效"
        );
    }

    #[test]
    fn 目录动作带书源请求头() {
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                       "header":"{\"Cookie\":\"a=b\"}",
                       "ruleToc":"{\"chapterList\":\"@css:dd\",\"chapterName\":\"@css:a@text\"}"}"#;
        let source = parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有源");
        let book = sample_book(Some("https://a.com/toc"), "https://a.com|x");
        let host = FakeHost::ok(toc_html(1));
        toc(&source, &book, &host).expect("应成功");
        let seen = host
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(seen[0].headers.get("Cookie"), Some("a=b"));
    }
}
