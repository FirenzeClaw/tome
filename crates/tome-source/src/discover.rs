//! \[DONE] 发现导航与分页（tasks.md T072 / FR-013）
//!
//! # 发现与搜索的区别
//!
//! | 维度 | 搜索 | 发现 |
//! |---|---|---|
//! | 输入 | 关键词 | 分类/榜单标识 + 页码 |
//! | 地址 | 单一 `searchUrl` 模板 | `exploreUrl`，**可含多条**（`&&` 或换行分隔） |
//! | 规则 | `ruleSearch` | `ruleExplore`（字段集合略有不同） |
//! | 产出 | 书籍列表 | 书籍列表 + **下一页页码** |
//!
//! 最后一行是 FR-013"支持导航与分页"的落点：发现是**遍历**语义，
//! 调用方要知道"还有没有下一页"才能继续翻。
//!
//! # `exploreUrl` 的多条形态
//!
//! 公开格式允许它写成多条（用 `&&` 或换行分隔），每条对应一个分类。
//! 故本模块先解析成 [`ExploreEntry`] 列表，再按 [`kind`] 选择。
//!
//! ## 为什么不用下标而是用 `kind` 字符串
//!
//! 下标的语义会随源作者调整分类顺序而变（今天 `0` 是"玄幻"、明天可能
//! 是"都市"），而用户书架上存的是**上一次浏览的分类**。用可变的下标
//! 会让"回到上次的分类"变成"跳到另一个分类"。`kind` 是作者给的稳定标识。
//!
//! [`kind`]: ExploreEntry::kind

use tome_core::seam::ExplorePage;
use tome_core::{Book, BookOrigin, NetHost};

use crate::action;
use crate::error::SourceError;
use crate::source::ParsedSource;

/// `ruleExplore` 的字段名（公开书源格式的约定）。
///
/// 与 `search_rule_keys` 的差异：发现**没有** `kind`（分类是输入而非产出，
/// 有了分类就无需规则去取），也**没有** `bookUrlPattern`。
pub mod explore_keys {
    /// 书籍列表
    pub const BOOK_LIST: &str = "bookList";
    /// 书名
    pub const NAME: &str = "name";
    /// 作者
    pub const AUTHOR: &str = "author";
    /// 封面
    pub const COVER_URL: &str = "coverUrl";
    /// 简介
    pub const INTRO: &str = "intro";
    /// 详情页地址
    pub const BOOK_URL: &str = "bookUrl";
    /// 分类标签
    pub const KIND: &str = "kind";
    /// 最新章节
    pub const LAST_CHAPTER: &str = "lastChapter";
    /// 字数（自述串）
    pub const WORD_COUNT: &str = "wordCount";
}

/// `exploreUrl` 里的一条（一个分类）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExploreEntry {
    /// 分类标识（源作者给出的名字，如"玄幻"）
    pub kind: String,
    /// 地址模板（含 `{{page}}` 占位）
    pub url: String,
}

/// 解析 `exploreUrl` 的多条形态。
///
/// # 两种分隔符都要接受
///
/// 公开格式里多条可以写成：
///
/// ```text
///   玄幻::/list/1/{{page}}.html&&都市::/list/2/{{page}}.html
///   玄幻::/list/1/{{page}}.html
///   都市::/list/2/{{page}}.html
/// ```
///
/// `&&` 与换行都出现过（源作者手写时的习惯不同）。行尾的 `\r`
/// 也要吃掉——Windows 上编辑过的源文件会带它，而它会让分类名变成
/// `"都市\r"`，表现为"按 kind 找不到分类"。
pub fn parse_explore_url(raw: &str) -> Vec<ExploreEntry> {
    raw.split("&&")
        .flat_map(|segment| segment.split('\n'))
        .map(|segment| segment.trim().trim_end_matches('\r').trim())
        .filter(|segment| !segment.is_empty())
        .filter_map(parse_one_entry)
        .collect()
}

/// 解析单条 `分类名::地址`。
///
/// 没有 `::` 时把整段当地址、分类名取一个稳定的自动名（`分类N`，
/// 由调用方按位置补）——见 [`parse_explore_url_with_default`]。
fn parse_one_entry(segment: &str) -> Option<ExploreEntry> {
    let (kind, url) = segment.split_once("::")?;
    let kind = kind.trim();
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    Some(ExploreEntry {
        kind: if kind.is_empty() {
            // 空分类名：用地址本身做标识（比空串好——空串无法被选择）
            url.to_string()
        } else {
            kind.to_string()
        },
        url: url.to_string(),
    })
}

/// 解析并给没写分类名的条目补上按位置的默认名。
///
/// # 为什么需要"补默认名"
///
/// 部分书源的 `exploreUrl` 只写地址不写分类名（单分类源，或作者觉得
/// 名字显而易见）。丢弃它们会让这些源的发现功能完全不可用——而它们
/// 的地址是好的。
pub fn parse_explore_url_with_default(raw: &str) -> Vec<ExploreEntry> {
    let mut entries = Vec::new();
    for (position, segment) in raw
        .split("&&")
        .flat_map(|part| part.split('\n'))
        .map(|part| part.trim().trim_end_matches('\r').trim())
        .filter(|part| !part.is_empty())
        .enumerate()
    {
        match parse_one_entry(segment) {
            Some(entry) => entries.push(entry),
            None => entries.push(ExploreEntry {
                kind: format!("分类{}", position + 1),
                url: segment.to_string(),
            }),
        }
    }
    entries
}

/// 从 `exploreUrl` 里按分类标识取一条。
pub fn select_entry<'a>(entries: &'a [ExploreEntry], kind: &str) -> Option<&'a ExploreEntry> {
    entries.iter().find(|entry| entry.kind == kind)
}

/// 替换地址模板里的 `{{page}}`。
///
/// # 与搜索的 `{{key}}` 的差异
///
/// 发现只有 `{{page}}`，没有 `{{key}}`（没有关键词）。故本函数比
/// `search::substitute_url` 简单——它不做 URL 编码（页码是数字）。
///
/// 未含 `{{page}}` 的地址原样返回：单页分类（如"完本榜"）是合法的，
/// 强行要求占位符会让这些分类不可用。
pub fn substitute_page(template: &str, page: u32) -> String {
    template.replace("{{page}}", &page.to_string())
}

/// 从发现页 HTML 解析书籍列表（**不发请求**，A2 纯化）。
pub fn parse_explore_page(
    parsed: &ParsedSource,
    html: &str,
    next_page: Option<u32>,
) -> Result<ExplorePage, SourceError> {
    parse_explore_page_with_js(parsed, html, next_page, None)
}

/// [`parse_explore_page`] 的**带 JS 版本**。
pub fn parse_explore_page_with_js(
    parsed: &ParsedSource,
    html: &str,
    next_page: Option<u32>,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<ExplorePage, SourceError> {
    let source = &parsed.source;
    let rules = action::parse_rule_object(parsed, "ruleExplore", source.rule_explore.as_deref())?;

    let document = tome_rule::dom::Document::parse_html(html);
    let engine = match js {
        Some(engine) => tome_rule::css_post::RuleEngine::new(&document).with_js(engine.runtime()),
        None => tome_rule::css_post::RuleEngine::new(&document),
    };

    let nodes = action::select_nodes(&document, &source.id, &rules, explore_keys::BOOK_LIST)?;

    let mut books = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let value = |key: &str| -> Result<Option<String>, SourceError> {
            action::evaluate_field(&engine, node, &rules, key)
        };

        let Some(name) = value(explore_keys::NAME)? else {
            // ## 没有书名的条目跳过（与目录管线的口径**不同**）
            //
            // 目录里缺标题的章节必须保留（跳过会让后续章节序号整体前移，
            // 而进度按序号记）。但发现页的条目是**列表项**，没有序号语义
            // ——跳过它只是少显示一本书，不会让别的东西错位。
            //
            // 反过来（给个占位书名）会在列表里出现"（无标题）"这种条目，
            // 用户点进去发现是空的。少显示比假显示好。
            continue;
        };

        let Some(book_url) = value(explore_keys::BOOK_URL)? else {
            // 没有详情页地址的书**无法打开**，同样跳过
            continue;
        };
        if book_url.trim().is_empty() {
            continue;
        }

        let book_url = action::absolute_url(&source.id, &book_url);
        books.push(Book {
            id: 0,
            origin: BookOrigin::Network,
            origin_id: format!("{}|{}", source.id, book_url),
            name,
            author: value(explore_keys::AUTHOR)?,
            cover_url: value(explore_keys::COVER_URL)?
                .map(|url| action::absolute_url(&source.id, &url)),
            intro: value(explore_keys::INTRO)?,
            kind: value(explore_keys::KIND)?,
            book_group: None,
            read_config: None,
            toc_url: Some(book_url),
            latest_chapter_title: value(explore_keys::LAST_CHAPTER)?,
            latest_chapter_time: None,
            library_root_id: None,
            relative_path: None,
            charset: source.charset.clone(),
            format: None,
            total_chapters: None,
            dur_chapter_index: None,
            added_at: crate::source::now_unix_seconds(),
        });
    }

    Ok(ExplorePage { books, next_page })
}

/// 发现动作的完整执行（求值 → 抓取 → 解析）。
///
/// \[DONE] T072 的对外入口。
///
/// `next_page` 由调用方给出（`page + 1`）或 `None` 表示"这是最后一页"。
/// 本模块**不从 HTML 推断"有没有下一页"**——那需要每个源写一条规则，
/// 而公开格式里没有这个约定。
pub fn discover(
    parsed: &ParsedSource,
    kind: &str,
    page: u32,
    host: &dyn NetHost,
) -> Result<ExplorePage, SourceError> {
    discover_with_js(parsed, kind, page, host, None)
}

/// [`discover`] 的带 JS 版本。
pub fn discover_with_js(
    parsed: &ParsedSource,
    kind: &str,
    page: u32,
    host: &dyn NetHost,
    js: Option<&crate::js::SourceJsEngine>,
) -> Result<ExplorePage, SourceError> {
    let raw = parsed.source.explore_url.as_deref().ok_or_else(|| {
        SourceError::incomplete(parsed.source.id.clone(), "缺少 exploreUrl（发现地址）")
    })?;
    let entries = parse_explore_url_with_default(raw);
    if entries.is_empty() {
        return Err(SourceError::incomplete(
            parsed.source.id.clone(),
            "exploreUrl 里没有可用的分类",
        ));
    }

    let entry = select_entry(&entries, kind).ok_or_else(|| {
        SourceError::rule(format!(
            "exploreUrl 里没有分类 `{kind}`（可用：{}）",
            entries
                .iter()
                .map(|entry| entry.kind.as_str())
                .collect::<Vec<_>>()
                .join("、")
        ))
    })?;

    // ## 相对地址要补全为绝对地址
    //
    // `exploreUrl` 常写成 `/list/1/{{page}}.html`（相对源站）。不补全会
    // 得到一个以 `/` 开头的地址，而 `RequestSpec` 要的是完整 URL——
    // 不补的后果是请求发不出去（或发到一个错误的主机）。
    let url = action::absolute_url(&parsed.source.id, &substitute_page(&entry.url, page));
    let spec = action::base_spec(parsed, &url)?;
    let (html, _response) = action::fetch_text(host, spec, "发现")?;

    // 下一页：页码递增即"有下一页"。真正的终止由"这一页返回 0 本书"
    // 或调用方的上限决定——见 `has_next_page` 的说明。
    let next_page = if page < u32::MAX {
        Some(page + 1)
    } else {
        None
    };
    parse_explore_page_with_js(parsed, &html, next_page, js)
}

/// 判断某页之后是否还值得继续翻。
///
/// # 为什么用"这一页有没有书"而不是"有没有下一页按钮"
///
/// 公开格式里**没有**"下一页"的规则约定（每个源的翻页标记写法千差万别：
/// `下一页`、`>`、`next`、`›`……）。为它发明一条规则会让大量源需要改。
///
/// 而"这一页返回了 0 本书"是一个**跨所有源都成立**的终止信号：
/// 目录走到末尾时，列表规则匹配不到任何条目。
///
/// ## 代价
///
/// 最后一页的下一页会被请求一次（拿到 0 本书）。多一次请求换取
/// "不需要任何额外的规则约定"，是划算的——尤其对书源兼容性（本项目的
/// 最高优先级）。
pub fn has_next_page(page: &ExplorePage) -> bool {
    !page.books.is_empty() && page.next_page.is_some()
}

/// 翻页上限（防御性）。
///
/// # 为什么需要
///
/// 一个"永远返回相同结果"的源（服务端忽略页码参数）会让遍历无限循环。
/// 上限让调用方不必自己防这个——而"忘了防"的表现是**把用户的磁盘写满**
/// （每次翻页都入库一批书）。
///
/// 取 500：按每页 20 本算是 1 万本书，远超任何真实分类的规模。
///
/// \[WORKAROUND] 硬编码：若某源真有超过 500 页的分类，应改成可配置项。
pub const MAX_EXPLORE_PAGES: u32 = 500;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::parse_sources;
    use std::sync::Mutex;
    use tome_core::{ErrorKind, HeaderMap, RequestSpec, Response};

    const LIST_HTML: &str = r#"<html><body><ul>
      <li class="book"><h3 class="n"><a href="/book/1">斗破苍穹</a></h3>
        <span class="a">天蚕土豆</span><img class="c" src="/c/1.jpg"/></li>
      <li class="book"><h3 class="n"><a href="/book/2">完美世界</a></h3>
        <span class="a">辰东</span><img class="c" src="/c/2.jpg"/></li>
    </ul></body></html>"#;

    fn source_with_explore(explore_url: &str, rule: Option<&str>) -> ParsedSource {
        let rule_part = match rule {
            Some(rule) => format!(
                ",\"ruleExplore\":{}",
                serde_json::to_string(rule).expect("应能序列化")
            ),
            None => String::new(),
        };
        let text = format!(
            r#"{{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                 "exploreUrl":{} {rule_part}}}"#,
            serde_json::to_string(explore_url).expect("应能序列化")
        );
        parse_sources(&text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    fn parsed(text: &str) -> ParsedSource {
        parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    const EXPLORE_RULE: &str = r#"{"bookList":"@css:li.book","name":"@css:.n@text",
        "author":"@css:.a@text","bookUrl":"@css:.n>a@href","coverUrl":"@css:.c@src"}"#;

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
        fn requests(&self) -> Vec<RequestSpec> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
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

    // --- exploreUrl 解析 ---

    #[test]
    fn 解析_and_分隔的多条() {
        let entries = parse_explore_url("玄幻::/l/1/{{page}}.html&&都市::/l/2/{{page}}.html");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, "玄幻");
        assert_eq!(entries[0].url, "/l/1/{{page}}.html");
        assert_eq!(entries[1].kind, "都市");
    }

    #[test]
    fn 解析换行分隔的多条() {
        let entries = parse_explore_url("玄幻::/l/1/{{page}}.html\n都市::/l/2/{{page}}.html");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, "玄幻");
        assert_eq!(entries[1].kind, "都市");
    }

    #[test]
    fn 两种分隔符混用() {
        let entries = parse_explore_url("A::/1/{{page}}\nB::/2/{{page}}&&C::/3/{{page}}");
        let kinds: Vec<&str> = entries.iter().map(|entry| entry.kind.as_str()).collect();
        assert_eq!(kinds, vec!["A", "B", "C"]);
    }

    #[test]
    fn 行尾回车被吃掉() {
        // Windows 上编辑过的源文件会带 \r，它会让分类名变成 "都市\r"，
        // 表现为"按 kind 找不到分类"
        let entries = parse_explore_url("玄幻::/l/1/{{page}}\r\n都市::/l/2/{{page}}\r");
        let kinds: Vec<&str> = entries.iter().map(|entry| entry.kind.as_str()).collect();
        assert_eq!(kinds, vec!["玄幻", "都市"], "行尾 \\r 必须被吃掉");
    }

    #[test]
    fn 空段被跳过() {
        let entries = parse_explore_url("A::/1\n\n\n&&\nB::/2");
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn 单条无分类名时补默认名() {
        // 部分源只写地址不写分类名。丢弃它们会让这些源的发现完全不可用
        let entries = parse_explore_url_with_default("/l/1/{{page}}.html");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, "分类1");
        assert_eq!(entries[0].url, "/l/1/{{page}}.html");
    }

    #[test]
    fn 混合有无分类名() {
        let entries = parse_explore_url_with_default("玄幻::/1/{{page}}\n/2/{{page}}");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, "玄幻");
        assert_eq!(entries[1].kind, "分类2", "未命名的按位置补名");
    }

    #[test]
    fn 地址为空的条目被丢弃() {
        let entries = parse_explore_url("玄幻::&&都市::/2/{{page}}");
        assert_eq!(entries.len(), 1, "地址空的条目无法抓取，应丢弃");
        assert_eq!(entries[0].kind, "都市");
    }

    #[test]
    fn 空分类名用地址做标识() {
        // 空串无法被选择，故用地址兜底
        let entries = parse_explore_url("::/1/{{page}}");
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].kind.is_empty());
    }

    #[test]
    fn 选择分类() {
        let entries = parse_explore_url("玄幻::/1&&都市::/2");
        assert_eq!(select_entry(&entries, "都市").expect("应找到").url, "/2");
        assert!(select_entry(&entries, "不存在").is_none());
    }

    // --- 分页占位 ---

    #[test]
    fn 页码替换() {
        assert_eq!(substitute_page("/l/{{page}}.html", 3), "/l/3.html");
        assert_eq!(
            substitute_page("/l/1.html", 3),
            "/l/1.html",
            "无占位符时原样"
        );
    }

    #[test]
    fn 无_page_占位符的分类也能用() {
        // 单页分类（如"完本榜"）是合法的，强行要求占位符会让它们不可用
        let source = source_with_explore("/top.html", Some(EXPLORE_RULE));
        let host = FakeHost::ok(LIST_HTML);
        let page = discover(&source, "分类1", 1, &host).expect("应成功");
        assert_eq!(page.books.len(), 2);
        assert_eq!(host.requests()[0].url, "https://a.com/top.html");
    }

    // --- 解析书籍列表 ---

    #[test]
    fn 解析发现页书籍() {
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let page = parse_explore_page(&source, LIST_HTML, None).expect("应成功");
        assert_eq!(page.books.len(), 2);
        assert_eq!(page.books[0].name, "斗破苍穹");
        assert_eq!(page.books[0].author.as_deref(), Some("天蚕土豆"));
        assert_eq!(
            page.books[0].cover_url.as_deref(),
            Some("https://a.com/c/1.jpg"),
            "封面相对地址应补全"
        );
        assert_eq!(page.books[0].origin, BookOrigin::Network);
        assert_eq!(
            page.books[0].origin_id,
            "https://a.com|https://a.com/book/1"
        );
    }

    #[test]
    fn 缺书名的条目被跳过() {
        // 与目录管线的口径**不同**：目录缺标题必须保留（否则序号错位），
        // 而发现页条目是列表项、没有序号语义。给占位名会让用户点进去
        // 发现是空的——少显示比假显示好。
        let html = r#"<ul>
          <li class="book"><h3 class="n"><a href="/book/1">有名字</a></h3></li>
          <li class="book"><h3 class="n"><a href="/book/2"></a></h3></li>
        </ul>"#;
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let page = parse_explore_page(&source, html, None).expect("应成功");
        assert_eq!(page.books.len(), 1, "缺书名的条目应被跳过");
        assert_eq!(page.books[0].name, "有名字");
    }

    #[test]
    fn 缺详情页地址的条目被跳过() {
        // 没有地址的书无法打开，跳过比入库一条点不开的记录好
        let html = r#"<ul>
          <li class="book"><h3 class="n"><a>没有链接</a></h3></li>
          <li class="book"><h3 class="n"><a href="/book/2">有链接</a></h3></li>
        </ul>"#;
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let page = parse_explore_page(&source, html, None).expect("应成功");
        assert_eq!(page.books.len(), 1);
        assert_eq!(page.books[0].name, "有链接");
    }

    #[test]
    fn 空列表返回空页而非错误() {
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let page = parse_explore_page(&source, "<ul></ul>", None).expect("应成功");
        assert!(page.books.is_empty());
    }

    #[test]
    fn 缺_ruleexplore_报配置不完整() {
        let source = source_with_explore("/l/{{page}}", None);
        let error = parse_explore_page(&source, LIST_HTML, None).expect_err("应报错");
        assert!(error.to_string().contains("ruleExplore"), "{error}");
    }

    // --- 完整发现 ---

    #[test]
    fn 发现走选中的分类() {
        let source = source_with_explore(
            "玄幻::/l/1/{{page}}.html&&都市::/l/2/{{page}}.html",
            Some(EXPLORE_RULE),
        );
        let host = FakeHost::ok(LIST_HTML);
        let page = discover(&source, "都市", 3, &host).expect("应成功");

        assert_eq!(page.books.len(), 2);
        assert_eq!(
            host.requests()[0].url,
            "https://a.com/l/2/3.html",
            "应请求选中分类的第 3 页"
        );
    }

    #[test]
    fn 分类不存在时报错并列出可用分类() {
        let source =
            source_with_explore("玄幻::/1/{{page}}&&都市::/2/{{page}}", Some(EXPLORE_RULE));
        let host = FakeHost::ok(LIST_HTML);
        let error = discover(&source, "不存在", 1, &host).expect_err("应报错");
        let message = error.to_string();
        assert!(message.contains("玄幻"), "应列出可用分类：{message}");
        assert!(message.contains("都市"), "{message}");
    }

    #[test]
    fn 缺_exploreurl_报配置不完整() {
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                       "ruleExplore":"{\"bookList\":\"@css:li\"}"}"#;
        let source = parsed(text);
        let host = FakeHost::ok(LIST_HTML);
        let error = discover(&source, "任意", 1, &host).expect_err("应报错");
        assert!(error.to_string().contains("exploreUrl"), "{error}");
    }

    #[test]
    fn 非_2xx_报网络错误() {
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let host = FakeHost {
            body: String::new(),
            status: 404,
            seen: Mutex::new(Vec::new()),
        };
        let error = discover(&source, "分类1", 1, &host).expect_err("应报错");
        assert_eq!(error.to_error_kind(), ErrorKind::Network);
    }

    #[test]
    fn 发现带书源请求头() {
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                       "header":"{\"Cookie\":\"a=b\"}",
                       "exploreUrl":"/l/{{page}}",
                       "ruleExplore":"{\"bookList\":\"@css:li.book\",\"name\":\"@css:.n@text\",\"bookUrl\":\"@css:.n>a@href\"}"}"#;
        let source = parsed(text);
        let host = FakeHost::ok(LIST_HTML);
        discover(&source, "分类1", 1, &host).expect("应成功");
        assert_eq!(host.requests()[0].headers.get("Cookie"), Some("a=b"));
    }

    // --- 分页终止 ---

    #[test]
    fn 有结果且页码未到头时继续翻() {
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let host = FakeHost::ok(LIST_HTML);
        let page = discover(&source, "分类1", 1, &host).expect("应成功");
        assert!(has_next_page(&page), "有书且有下一页 → 继续翻");
    }

    #[test]
    fn 空页终止翻页() {
        // "这一页返回 0 本书"是跨所有源都成立的终止信号
        // （公开格式里没有"下一页"的规则约定）
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let host = FakeHost::ok("<ul></ul>");
        let page = discover(&source, "分类1", 99, &host).expect("应成功");
        assert!(!has_next_page(&page), "空页应终止翻页");
    }

    #[test]
    fn 页码到上限时终止() {
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let host = FakeHost::ok(LIST_HTML);
        let page = discover(&source, "分类1", u32::MAX, &host).expect("应成功");
        assert!(!has_next_page(&page), "页码到顶时不该再翻（会溢出）");
        assert_eq!(page.next_page, None);
    }

    #[test]
    fn 翻页上限有定义() {
        // 一个"永远返回相同结果"的源会让遍历无限循环，
        // 而"忘了防"的表现是把用户的磁盘写满（每次翻页都入库一批书）。
        //
        // 用 `const` 块做编译期断言：上限是常量，故这个约束在编译期
        // 就能检查——运行时测试反而更晚。
        const {
            assert!(MAX_EXPLORE_PAGES > 0);
            assert!(MAX_EXPLORE_PAGES <= 10_000);
        }
    }

    #[test]
    fn 下一页页码递增() {
        let source = source_with_explore("/l/{{page}}", Some(EXPLORE_RULE));
        let host = FakeHost::ok(LIST_HTML);
        let page = discover(&source, "分类1", 6, &host).expect("应成功");
        assert_eq!(page.next_page, Some(7));
    }
}
