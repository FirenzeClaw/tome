//! [DONE] 书籍详情管线（tasks.md T065 / FR-002 第三类动作）
//!
//! # 动作的形状
//!
//! ```text
//!   书源 + 详情页 URL
//!     └─ 规则求值 → RequestSpec（ruleBookInfo 无关，详情页地址即输入）
//!     └─ 抓取 → 解码
//!     └─ ruleBookInfo 规则 → Book（书名/作者/封面/简介/分类/目录页）
//! ```
//!
//! # 为什么返回 `Book` 而不是单独的 `BookInfo` 类型
//!
//! `data-model.md` 的 `book` 表**就是**详情字段的载体（书名/作者/封面/
//! 简介/分类/目录页）。造一个 `BookInfo` 会得到同一份数据的两个名字，
//! 而两者之间的转换是纯粹的形式开销——且每次给 `Book` 加字段都要在
//! `BookInfo` 上再写一遍。接缝 `SourceEngine::book_info` 已按此决策
//! 返回 `Book`（那里有完整的理由说明）。
//!
//! # 字段规则缺失时的行为
//!
//! 逐字段可选。**唯一必需**的是"书名"——没有书名的书籍记录在书架上是
//! 不可辨识的，故它是唯一会在缺失时报错字段。其余（作者/封面/简介）
//! 缺失是常态（书源作者省略），取 `None`。
//!
//! ## 为什么封面/简介缺失不算失败
//!
//! 详情动作的目的地是"把这本书加进书架"。封面与简介是锦上添花——
//! 因为它们没取到而拒绝加入书架，是把可选信息当成了必需信息。

use std::collections::BTreeMap;

use tome_core::{Book, BookOrigin, NetHost};

use crate::action;
use crate::error::SourceError;
use crate::source::ParsedSource;

/// `ruleBookInfo` 的字段名（公开书源格式的约定）。
///
/// 与搜索的 `search_rule_keys` 分开：两者虽然有几个同名键（`name`/
/// `author`），但**字段集合不同**（详情有 `tocUrl`/`kind`，搜索有
/// `bookUrl`/`lastChapter`）。合成一个常量集会让"某个键属于哪类动作"
/// 变得含糊。
pub mod book_info_keys {
    /// 书名（**唯一必需**）
    pub const NAME: &str = "name";
    /// 作者
    pub const AUTHOR: &str = "author";
    /// 封面地址
    pub const COVER_URL: &str = "coverUrl";
    /// 简介
    pub const INTRO: &str = "intro";
    /// 分类
    pub const KIND: &str = "kind";
    /// 目录页地址（书源常从详情页里取出真正的目录页链接）
    pub const TOC_URL: &str = "tocUrl";
    /// 最新章节标题
    pub const LAST_CHAPTER: &str = "lastChapter";
    /// 字数（书源自述的串，如"200万字"——不强解析为数字，那会丢信息）
    pub const WORD_COUNT: &str = "wordCount";
}

/// 从详情页 HTML 解析书籍（**不发请求**，A2 纯化）。
///
/// `book_url` 是详情页地址，用于填 `origin_id` 与 `toc_url` 的缺省值。
pub fn parse_book_info(
    parsed: &ParsedSource,
    book_url: &str,
    html: &str,
) -> Result<Book, SourceError> {
    parse_book_info_with_js(parsed, book_url, html, None)
}

/// [`parse_book_info`] 的**带 JS 版本**（字段规则里的 `@js:`）。
///
/// 与搜索同理：`ruleBookInfo` 的字段规则可以直接写 `@js:`，故需要沙箱。
/// `js = None` 时遇到 `@js:` 会报结构化能力错误（FR-006），而不是静默给空。
pub fn parse_book_info_with_js(
    parsed: &ParsedSource,
    book_url: &str,
    html: &str,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<Book, SourceError> {
    let source = &parsed.source;
    let rules =
        action::parse_rule_object(parsed, "ruleBookInfo", source.rule_book_info.as_deref())?;

    let document = tome_rule::dom::Document::parse_html(html);
    let engine = match js {
        Some(engine) => tome_rule::css_post::RuleEngine::new(&document).with_js(engine.runtime()),
        None => tome_rule::css_post::RuleEngine::new(&document),
    };

    // ## 详情规则为什么在**文档根**上求值（不像搜索那样先切子树）
    //
    // 搜索的字段规则以"每本书"为单位书写，故必须先按 `bookList` 切出子树。
    // 详情页只有**一本书**，规则直接写在整页上（`@css:#info h1@text`）——
    // 没有需要区分的重复结构。故这里不做 `select_nodes`。
    let root = document.root();
    let value = |key: &str| -> Result<Option<String>, SourceError> {
        action::evaluate_field(&engine, &root, &rules, key)
    };

    // 书名是唯一必需字段
    let name = value(book_info_keys::NAME)?.ok_or_else(|| {
        SourceError::rule(format!(
            "详情规则 `name` 未命中（详情页 {}）——没有书名无法在书架上辨识这本书",
            book_url
        ))
    })?;

    // 目录页地址：书源给了就用，否则回落到详情页地址。
    //
    // ## 为什么缺省是详情页地址而不是报错
    //
    // 大量书源的详情页**就是**目录页（单页应用）。规则里不写 `tocUrl`
    // 意味着"就在这一页"，而不是"书源写错了"。报错会让这些源无法入库。
    let toc_url = match value(book_info_keys::TOC_URL)? {
        Some(url) => action::absolute_url(&source.id, &url),
        None => book_url.to_string(),
    };

    Ok(Book {
        id: 0,
        origin: BookOrigin::Network,
        // 身份规则（`data-model.md`）：网络书以 `书源id|书籍详情URL` 唯一
        origin_id: format!("{}|{}", source.id, book_url),
        name,
        author: value(book_info_keys::AUTHOR)?,
        cover_url: value(book_info_keys::COVER_URL)?
            .map(|url| action::absolute_url(&source.id, &url)),
        intro: value(book_info_keys::INTRO)?,
        kind: value(book_info_keys::KIND)?,
        book_group: None,
        read_config: None,
        toc_url: Some(toc_url),
        // 最新章节与字数在这里取到的话是"详情页上的展示值"，
        // 但**权威值**来自目录动作（T066）。故此处不填 `total_chapters`
        // （那是目录长度），只填详情页自述的标题。
        latest_chapter_title: value(book_info_keys::LAST_CHAPTER)?,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: source.charset.clone(),
        format: None,
        total_chapters: None,
        dur_chapter_index: None,
        added_at: crate::source::now_unix_seconds(),
    })
}

/// 详情动作的完整执行（求值 → 抓取 → 解析）。
///
/// [DONE] T065 的对外入口。
pub fn book_info(
    parsed: &ParsedSource,
    book_url: &str,
    host: &dyn NetHost,
) -> Result<Book, SourceError> {
    book_info_with_js(parsed, book_url, host, None)
}

/// [`book_info`] 的带 JS 版本。
pub fn book_info_with_js(
    parsed: &ParsedSource,
    book_url: &str,
    host: &dyn NetHost,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<Book, SourceError> {
    // 详情动作直接请求详情页地址（书源格式里没有"详情地址模板"这一项——
    // 详情页地址是在搜索/发现结果里拿到的）。
    let spec = action::base_spec(parsed, book_url)?;
    let (html, _response) = action::fetch_text(host, spec, "详情")?;
    parse_book_info_with_js(parsed, book_url, &html, js)
}

/// 从规则集合里读一个字段（供调用方做额外校验时复用）。
pub fn field_of<'a>(rules: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    rules.get(key).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::parse_sources;
    use std::sync::Mutex;
    use tome_core::{ErrorKind, HeaderMap, RequestSpec, Response};

    const INFO_HTML: &str = r#"<html><body>
      <div id="info">
        <h1 class="book-name">斗破苍穹</h1>
        <p class="author">作者：天蚕土豆</p>
        <img class="cover" src="/img/cover.jpg"/>
        <div class="intro">三十年河东，三十年河西。</div>
        <span class="kind">玄幻</span>
        <a class="toc" href="/book/1/toc">目录</a>
      </div>
    </body></html>"#;

    fn parsed(rule: &str) -> ParsedSource {
        let text = format!(
            r#"{{"bookSourceName":"S","bookSourceUrl":"https://a.com","ruleBookInfo":{}}}"#,
            serde_json::to_string(rule).expect("应能序列化")
        );
        parse_sources(&text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    /// 假宿主：按 URL 片段返回响应。
    struct FakeHost {
        body: String,
        status: u16,
        seen: Mutex<Vec<RequestSpec>>,
    }

    impl FakeHost {
        fn ok(body: &str) -> Self {
            Self {
                body: body.to_string(),
                status: 200,
                seen: Mutex::new(Vec::new()),
            }
        }
        fn failing(status: u16) -> Self {
            Self {
                body: String::new(),
                status,
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
    fn 全字段解析() {
        let source = parsed(
            r#"{"name":"@css:.book-name@text","author":"@css:.author@text",
                "coverUrl":"@css:.cover@src","intro":"@css:.intro@text",
                "kind":"@css:.kind@text","tocUrl":"@css:.toc@href"}"#,
        );
        let book = parse_book_info(&source, "https://a.com/book/1", INFO_HTML).expect("应成功");

        assert_eq!(book.name, "斗破苍穹");
        assert_eq!(book.author.as_deref(), Some("作者：天蚕土豆"));
        assert_eq!(
            book.cover_url.as_deref(),
            Some("https://a.com/img/cover.jpg"),
            "封面相对地址应被补全"
        );
        assert_eq!(book.intro.as_deref(), Some("三十年河东，三十年河西。"));
        assert_eq!(book.kind.as_deref(), Some("玄幻"));
        assert_eq!(
            book.toc_url.as_deref(),
            Some("https://a.com/book/1/toc"),
            "目录页相对地址应被补全"
        );
    }

    #[test]
    fn 身份规则是书源加详情页地址() {
        // data-model.md：网络书以 (书源id, 书籍详情URL) 唯一
        let source = parsed(r#"{"name":"@css:h1@text"}"#);
        let book = parse_book_info(&source, "https://a.com/book/1", INFO_HTML).expect("应成功");
        assert_eq!(book.origin, BookOrigin::Network);
        assert_eq!(book.origin_id, "https://a.com|https://a.com/book/1");
    }

    #[test]
    fn 无书名时报错() {
        // 书名是唯一必需字段：没有它，书架上的记录无法辨识
        let source = parsed(r#"{"author":"@css:.author@text"}"#);
        let error =
            parse_book_info(&source, "https://a.com/book/1", INFO_HTML).expect_err("应报错");
        assert!(
            error.to_string().contains('名') || error.to_string().contains("name"),
            "错误应指出缺书名：{error}"
        );
    }

    #[test]
    fn 书名规则未命中时报错() {
        // 规则写了但选择器不匹配（源改版）——与"规则没写"同样必须报出来，
        // 否则会入库一条空名字的书
        let source = parsed(r#"{"name":"@css:.not-exist@text"}"#);
        let error =
            parse_book_info(&source, "https://a.com/book/1", INFO_HTML).expect_err("应报错");
        assert!(error.to_string().contains("未命中"), "{error}");
    }

    #[test]
    fn 可选的封面简介缺失不算失败() {
        // 详情动作的目的地是"加进书架"，封面/简介是锦上添花
        let source = parsed(r#"{"name":"@css:.book-name@text"}"#);
        let book = parse_book_info(&source, "https://a.com/book/1", INFO_HTML).expect("应成功");
        assert_eq!(book.name, "斗破苍穹");
        assert_eq!(book.cover_url, None);
        assert_eq!(book.intro, None);
        assert_eq!(book.author, None);
    }

    #[test]
    fn 缺_tocurl_时回落到详情页地址() {
        // 大量书源的详情页就是目录页（单页应用）。报错会让这些源无法入库。
        let source = parsed(r#"{"name":"@css:.book-name@text"}"#);
        let book = parse_book_info(&source, "https://a.com/book/1", INFO_HTML).expect("应成功");
        assert_eq!(book.toc_url.as_deref(), Some("https://a.com/book/1"));
    }

    #[test]
    fn 缺少_rulebookinfo_报配置不完整() {
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#;
        let source = parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有源");
        let error =
            parse_book_info(&source, "https://a.com/book/1", INFO_HTML).expect_err("应报错");
        assert!(error.to_string().contains("ruleBookInfo"), "{error}");
    }

    #[test]
    fn 详情动作发请求并解析() {
        let source = parsed(r#"{"name":"@css:.book-name@text"}"#);
        let host = FakeHost::ok(INFO_HTML);
        let book = book_info(&source, "https://a.com/book/1", &host).expect("应成功");
        assert_eq!(book.name, "斗破苍穹");

        let seen = host
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(seen.len(), 1, "应恰好请求一次");
        assert_eq!(seen[0].url, "https://a.com/book/1");
    }

    #[test]
    fn 非_2xx_报网络错误() {
        let source = parsed(r#"{"name":"@css:h1@text"}"#);
        let host = FakeHost::failing(404);
        let error = book_info(&source, "https://a.com/book/1", &host).expect_err("应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Network);
    }

    #[test]
    fn 书源请求头被带上() {
        // FR-008：详情请求要带书源声明的头（否则需登录的源取不到详情）
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                       "header":"{\"Referer\":\"https://a.com\"}",
                       "ruleBookInfo":"{\"name\":\"@css:.book-name@text\"}"}"#;
        let source = parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有源");
        let host = FakeHost::ok(INFO_HTML);
        book_info(&source, "https://a.com/book/1", &host).expect("应成功");

        let seen = host
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            seen[0].headers.get("Referer"),
            Some("https://a.com"),
            "详情请求应带书源声明的头"
        );
    }

    #[test]
    fn 已入库字段不在详情里臆造() {
        // 详情动作不产"书架分组""排版配置"这类用户设置，也不产章节数
        // （那是目录动作的结果）。凭空虚设会让上层拿到看似有值的空数据。
        let source = parsed(r#"{"name":"@css:.book-name@text"}"#);
        let book = parse_book_info(&source, "https://a.com/book/1", INFO_HTML).expect("应成功");
        assert_eq!(book.book_group, None);
        assert_eq!(book.read_config, None);
        assert_eq!(book.total_chapters, None);
        assert_eq!(book.dur_chapter_index, None);
        assert_eq!(book.library_root_id, None);
    }

    #[test]
    fn 字数与最新章节透传不解析() {
        // "200万字"这种串不强解析为数字——解析会丢信息（"约""余"等）
        let html = r#"<div><h1>书名</h1><span class="wc">200万字</span>
                      <span class="lc">第100章 大结局</span></div>"#;
        let source = parsed(
            r#"{"name":"@css:h1@text","wordCount":"@css:.wc@text",
                "lastChapter":"@css:.lc@text"}"#,
        );
        let book = parse_book_info(&source, "https://a.com/book/1", html).expect("应成功");
        assert_eq!(book.latest_chapter_title.as_deref(), Some("第100章 大结局"));
        // word_count 不落在 Book 上（Book 无该字段）——详情页自述的字数
        // 属于展示信息，权威值由目录统计
        assert_eq!(book.total_chapters, None);
    }

    #[test]
    fn 字段访问器可用() {
        let rules = action::parse_rule_object(
            &parsed(r#"{"name":"@css:h1@text"}"#),
            "ruleBookInfo",
            Some(r#"{"name":"@css:h1@text"}"#),
        )
        .expect("应成功");
        assert_eq!(field_of(&rules, "name"), Some("@css:h1@text"));
        assert_eq!(field_of(&rules, "nope"), None);
    }

    #[test]
    fn 详情页编码按检测值解码() {
        // FR-009：源站声明与实际不符时须容错
        // "斗破苍穹" 的 GBK 编码（8 字节；此处用 `encode('gbk')` 生成后
        // 逐字节写死——测试数据不依赖运行时的编码表，故它在任何平台一致）
        let gbk_bytes: Vec<u8> = vec![0xB6, 0xB7, 0xC6, 0xC6, 0xB2, 0xD4, 0xF1, 0xB7];
        struct GbkHost(Vec<u8>);
        impl NetHost for GbkHost {
            fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
                Ok(Response {
                    status: 200,
                    headers: HeaderMap::new(),
                    body_bytes: self.0.clone(),
                    // 检测层识别为 gbk（声明头为空）
                    detected_charset: Some("gbk".to_string()),
                })
            }
        }
        let source = parsed(r#"{"name":"@css:html@text"}"#);
        let book = book_info(&source, "https://a.com/b", &GbkHost(gbk_bytes)).expect("应成功");
        assert_eq!(book.name, "斗破苍穹", "GBK 页面应被正确解码");
    }
}
