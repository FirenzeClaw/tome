//! T011 / T012 验收测试：接缝 trait 的形状与共享值对象
//!
//! ## 这些测试在守什么
//!
//! 1. `search` 是 **Stream + CancelToken** 形状（A6），不是 `Vec`
//! 2. `change_source` 返回 `ChangeSourceOutcome` 且**标明映射级别**（FR-010b）
//! 3. `ContentProvider` 含 `chapter_content` / `resolve_anchor` / `prefetch`
//! 4. `RequestSpec` 含 FR-008 的**全部**字段（method/url/headers/body/charset/
//!    proxy_override/rate_limit_key/timeout/retry）
//! 5. trait 只引用 core 类型——由"本测试只 import tome_core"隐式保证：
//!    若签名里出现实现 crate 的类型，本文件无法构造 mock 实现

use tome_core::error::ErrorKind;
use tome_core::model::{Book, BookOrigin, Chapter};
use tome_core::seam::host::{
    HeaderMap, HttpMethod, ProxyOverride, RequestSpec, Response, RetryPolicy,
};
use tome_core::seam::{
    AnchorQuery, CancelToken, ChangeSourceOutcome, ChapterContent, CheckReport, CheckVerdict,
    ContentProvider, ExplorePage, ProgressMapping, SearchResultItem, SearchStream, SourceEngine,
};

/// 极简 `SearchStream` 实现——只为证明该 trait **可被外部 crate 实现**
/// （对象安全、无隐藏 crate 私有约束）。
struct FakeStream {
    token: CancelToken,
    remaining: Vec<SearchResultItem>,
}

impl SearchStream for FakeStream {
    fn next_item(&mut self) -> Option<SearchResultItem> {
        if self.token.is_cancelled() {
            return None;
        }
        self.remaining.pop()
    }

    fn cancel_token(&self) -> &CancelToken {
        &self.token
    }
}

/// `search` 返回 `Box<dyn SearchStream>`，且流可被取消（A6 / FR-012）。
#[test]
fn 搜索返回可取消的流() {
    let token = CancelToken::new();
    assert!(!token.is_cancelled());

    let mut stream: Box<dyn SearchStream> = Box::new(FakeStream {
        token: token.clone(),
        remaining: vec![SearchResultItem::SourceResults {
            source_id: "source-a".to_owned(),
            books: Vec::new(),
        }],
    });

    // 未取消时能取到条目
    assert!(stream.next_item().is_some());

    // 取消后立即收敛
    stream.cancel_token().cancel();
    assert!(token.is_cancelled(), "取消应传递到共享令牌");
    assert!(stream.next_item().is_none(), "取消后不应再产出条目");
}

/// 单源失败是**流内条目**，不终止整个流（A6 / FR-012）。
#[test]
fn 单源失败不终止流() {
    let token = CancelToken::new();
    let mut stream = FakeStream {
        token,
        // 栈序取用：先取到"失败"，再取到"成功"
        remaining: vec![
            SearchResultItem::SourceResults {
                source_id: "source-ok".to_owned(),
                books: Vec::new(),
            },
            SearchResultItem::SourceFailed {
                source_id: "source-bad".to_owned(),
                error: ErrorKind::Network,
            },
        ],
    };

    let first = stream.next_item().expect("应有条目");
    match first {
        SearchResultItem::SourceFailed { source_id, error } => {
            assert_eq!(source_id, "source-bad");
            assert_eq!(error, ErrorKind::Network);
        }
        other => panic!("应先是失败条目，实际 {other:?}"),
    }

    // 关键：失败之后流**仍然可用**
    let second = stream.next_item().expect("失败不应终止流");
    match second {
        SearchResultItem::SourceResults { source_id, .. } => assert_eq!(source_id, "source-ok"),
        other => panic!("应取到成功条目，实际 {other:?}"),
    }
}

/// `ChangeSourceOutcome` 必须能表达 FR-010b 的**三级降级**，且三级互异。
#[test]
fn 换源结果标注进度映射级别() {
    let book = sample_book();
    let chapters = vec![sample_chapter()];

    // 第一级：特征段匹配（带相似度）
    let anchor = ChangeSourceOutcome {
        book: book.clone(),
        chapters: chapters.clone(),
        progress_mapping: ProgressMapping::AnchorSegment {
            chapter_index: 3,
            char_offset: 120,
            similarity_percent: 92,
        },
    };
    match &anchor.progress_mapping {
        ProgressMapping::AnchorSegment {
            similarity_percent, ..
        } => assert_eq!(*similarity_percent, 92),
        other => panic!("应为特征段匹配，实际 {other:?}"),
    }

    // 第二级：标题匹配
    let title = ChangeSourceOutcome {
        book: book.clone(),
        chapters: chapters.clone(),
        progress_mapping: ProgressMapping::ChapterTitle { chapter_index: 3 },
    };
    assert_ne!(title.progress_mapping, anchor.progress_mapping);

    // 第三级：兜底（显式变体 → 上层据此提示用户，不静默跳转）
    let fallback = ChangeSourceOutcome {
        book,
        chapters,
        progress_mapping: ProgressMapping::FallbackToStart,
    };
    assert_ne!(fallback.progress_mapping, title.progress_mapping);
    assert_ne!(fallback.progress_mapping, anchor.progress_mapping);
}

/// `ContentProvider` 的四个方法齐备（含 `resolve_anchor` 与 `prefetch`）。
///
/// 能写出本 mock 即是"trait 形状正确且对象安全"的证明。
struct FakeContentProvider;

impl ContentProvider for FakeContentProvider {
    fn toc(&self, _book: &Book) -> Result<Vec<Chapter>, ErrorKind> {
        Ok(vec![sample_chapter()])
    }

    fn chapter_content(
        &self,
        _book: &Book,
        chapter_index: i32,
    ) -> Result<ChapterContent, ErrorKind> {
        Ok(ChapterContent {
            chapter_index,
            title: "第一章".to_owned(),
            content: "正文".to_owned(),
            from_cache: false,
        })
    }

    fn resolve_anchor(&self, _book: &Book, anchor: &AnchorQuery) -> Result<(i32, i64), ErrorKind> {
        // 契约要求：结果与排版参数无关，故此处直接回显输入锚点
        Ok((anchor.chapter_index, anchor.char_offset))
    }

    fn prefetch(&self, _book: &Book, _around_index: i32, _depth: u32) {
        // 契约：预取失败无副作用。此处空实现即合法。
    }
}

/// `ContentProvider` 可用作 trait 对象，且 `prefetch` 无返回值（失败无副作用）。
#[test]
fn 内容提供者可对象化且预取无副作用() {
    let provider: Box<dyn ContentProvider> = Box::new(FakeContentProvider);
    let book = sample_book();

    let toc = provider.toc(&book).expect("目录应可取");
    assert_eq!(toc.len(), 1);

    let content = provider.chapter_content(&book, 0).expect("正文应可取");
    assert_eq!(content.chapter_index, 0);
    assert!(!content.from_cache);

    // 锚点解析：任意排版参数下解析为同一锚点（SC-008）
    let query = AnchorQuery {
        chapter_index: 5,
        char_offset: 42,
        anchor_text: Some("锚点".to_owned()),
    };
    assert_eq!(
        provider.resolve_anchor(&book, &query).expect("应解析"),
        (5, 42)
    );

    // prefetch 返回 ()，失败不会打断调用方流程
    provider.prefetch(&book, 5, 3);
}

/// `SourceEngine` 的六个方法齐备（五动作 + 换源 + 检查）。
struct FakeSourceEngine;

impl SourceEngine for FakeSourceEngine {
    fn search(&self, _keyword: &str, _source_ids: &[String]) -> Box<dyn SearchStream> {
        Box::new(FakeStream {
            token: CancelToken::new(),
            remaining: Vec::new(),
        })
    }

    fn discover(&self, _source_id: &str, _kind: &str, page: u32) -> Result<ExplorePage, ErrorKind> {
        Ok(ExplorePage {
            books: Vec::new(),
            next_page: page.checked_add(1),
        })
    }

    fn book_info(&self, _source_id: &str, _book_url: &str) -> Result<Book, ErrorKind> {
        Ok(sample_book())
    }

    fn toc(&self, _source_id: &str, _book: &Book) -> Result<Vec<Chapter>, ErrorKind> {
        Ok(vec![sample_chapter()])
    }

    fn change_source(
        &self,
        book: &Book,
        _candidate_source_id: &str,
    ) -> Result<ChangeSourceOutcome, ErrorKind> {
        Ok(ChangeSourceOutcome {
            book: book.clone(),
            chapters: vec![sample_chapter()],
            progress_mapping: ProgressMapping::FallbackToStart,
        })
    }

    fn check_source(&self, _source_id: &str) -> Result<CheckReport, ErrorKind> {
        Ok(CheckReport {
            source: sample_source(),
            verdict: CheckVerdict::Available,
        })
    }
}

/// `SourceEngine` 可用作 trait 对象，六法齐备且可调用。
#[test]
fn 书源引擎六法齐备且可对象化() {
    let engine: Box<dyn SourceEngine> = Box::new(FakeSourceEngine);
    let book = sample_book();

    // 五动作
    let mut stream = engine.search("关键词", &[]);
    assert!(stream.next_item().is_none(), "空结果的流应直接结束");

    let page = engine.discover("source-a", "分类", 0).expect("发现应可用");
    assert_eq!(page.next_page, Some(1));

    assert!(engine
        .book_info("source-a", "https://example.com/b")
        .is_ok());
    assert!(engine.toc("source-a", &book).is_ok());

    // 换源 + 检查
    let outcome = engine.change_source(&book, "source-b").expect("换源应可用");
    assert_eq!(outcome.progress_mapping, ProgressMapping::FallbackToStart);

    let report = engine.check_source("source-a").expect("检查应可用");
    assert_eq!(report.verdict, CheckVerdict::Available);
}

/// FR-006：能力错误是**结构化且可被识别**的，书源据此降级。
#[test]
fn 能力错误可被识别并用于降级() {
    let engine = FakeSourceEngine;
    // 真实实现会在遇到 WebView 类能力时返回此错误
    let capability_error =
        ErrorKind::unsupported_capability(tome_core::error::ErrorCapability::new("webView"));

    // 书源的降级逻辑形如：match 能力标识决定改走哪条路
    let recovered = match &capability_error {
        ErrorKind::UnsupportedCapability { capability }
            if capability.capability_id() == "webView" =>
        {
            "改走纯 HTTP 规则"
        }
        _ => "其他处理",
    };
    assert_eq!(recovered, "改走纯 HTTP 规则");

    // 检查报告也能承载能力错误
    let report = CheckReport {
        source: sample_source(),
        verdict: CheckVerdict::Unavailable {
            reason: capability_error,
        },
    };
    assert!(matches!(
        report.verdict,
        CheckVerdict::Unavailable {
            reason: ErrorKind::UnsupportedCapability { .. }
        }
    ));

    // 保持 engine 被使用，避免未使用变量警告（spike 里它只是形状证明）
    let _ = engine;
}
// ---------------------------------------------------------------------------
// T012：RequestSpec / Response 与宿主 trait
// ---------------------------------------------------------------------------

/// FR-008：`RequestSpec` 必须含**全部**九个维度。
///
/// 这条测试的价值在于：任一字段被删掉，此处**编译失败**——
/// 比断言某个 getter 更早、更硬。
#[test]
fn 请求规格含_fr008_全字段() {
    let spec = RequestSpec {
        method: HttpMethod::Post,
        url: "https://example.com/search".to_owned(),
        headers: {
            let mut headers = HeaderMap::new();
            headers.append("User-Agent", "tome/0.1");
            headers
        },
        body: Some(b"keyword=x".to_vec()),
        charset: Some("GBK".to_owned()),
        proxy_override: Some(ProxyOverride::Direct),
        rate_limit_key: "source-a".to_owned(),
        timeout_ms: 15_000,
        retry: RetryPolicy::new(2, 500),
    };

    // 逐项验证语义（不只是"字段存在"）
    assert_eq!(spec.method, HttpMethod::Post);
    assert_eq!(spec.method.as_str(), "POST");
    assert_eq!(spec.url, "https://example.com/search");
    assert_eq!(spec.headers.get("user-agent"), Some("tome/0.1"));
    assert_eq!(spec.body.as_deref(), Some(b"keyword=x".as_slice()));
    assert_eq!(spec.charset.as_deref(), Some("GBK"));
    assert_eq!(spec.proxy_override, Some(ProxyOverride::Direct));
    assert_eq!(spec.rate_limit_key, "source-a");
    assert_eq!(spec.timeout_ms, 15_000);
    assert_eq!(spec.retry.max_retries, 2);
    assert_eq!(spec.retry.backoff_ms, 500);
}

/// 未指定 URL/限速键以外的任何字段，仍能构造出合法请求。
///
/// 目的：`RequestSpec::get` 是"默认值只有一处定义"的落点——书源规则里大量
/// 请求只给出 URL，这些默认值决定了它们的行为。
#[test]
fn 请求规格默认值可复现() {
    let spec = RequestSpec::get("https://example.com", "source-a");
    assert_eq!(spec.method, HttpMethod::Get);
    assert!(spec.body.is_none());
    assert!(spec.headers.is_empty());
    assert_eq!(spec.timeout_ms, tome_core::seam::host::DEFAULT_TIMEOUT_MS);
    assert_eq!(spec.retry, RetryPolicy::NONE);
}

/// `proxy_override` 的三态语义必须可区分（FR-008 跟随系统 vs 显式直连）。
#[test]
fn 代理覆盖区分跟随系统与显式直连() {
    let follow_system = RequestSpec::get("https://example.com", "source-a");
    assert_eq!(
        follow_system.proxy_override, None,
        "None 表示跟随系统代理，与 Direct 语义不同"
    );

    let direct = RequestSpec {
        proxy_override: Some(ProxyOverride::Direct),
        ..follow_system.clone()
    };
    let custom = RequestSpec {
        proxy_override: Some(ProxyOverride::Custom("http://127.0.0.1:7890".to_owned())),
        ..follow_system
    };

    assert_ne!(direct.proxy_override, custom.proxy_override);
    assert_ne!(direct.proxy_override, None);
}

/// `HeaderMap` 必须保留重名头（HTTP 允许，且 Set-Cookie 等多值场景依赖它）。
#[test]
fn 头集合保留重名且支持大小写不敏感查找() {
    let mut headers = HeaderMap::new();
    headers.append("Set-Cookie", "a=1");
    headers.append("Set-Cookie", "b=2");
    assert_eq!(headers.len(), 2, "重名头不得被合并");

    let values: Vec<&str> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value)
        .collect();
    assert_eq!(values, vec!["a=1", "b=2"]);

    // 大小写不敏感查找
    assert_eq!(headers.get("SET-COOKIE"), Some("a=1"));

    // set 覆盖同名项
    headers.set("set-cookie", "c=3");
    assert_eq!(headers.len(), 1);
    assert_eq!(headers.get("Set-Cookie"), Some("c=3"));
}

/// `Response` 的成功判定与体积读取。
#[test]
fn 响应可判定成功与体积() {
    let ok = Response {
        status: 200,
        headers: HeaderMap::new(),
        body_bytes: vec![1, 2, 3],
        detected_charset: Some("UTF-8".to_owned()),
    };
    assert!(ok.is_success());
    assert_eq!(ok.body_len(), 3);

    let redirect = Response {
        status: 302,
        ..ok.clone()
    };
    assert!(!redirect.is_success(), "3xx 不算成功");

    let failed = Response { status: 404, ..ok };
    assert!(!failed.is_success());
}

/// `HttpMethod` 的往返转换与非法值拒绝。
#[test]
fn http_方法可解析且拒绝非法值() {
    assert_eq!(
        HttpMethod::from_str_ignore_case("get"),
        Some(HttpMethod::Get)
    );
    assert_eq!(
        HttpMethod::from_str_ignore_case("POST"),
        Some(HttpMethod::Post)
    );
    assert_eq!(
        HttpMethod::from_str_ignore_case("DELETE"),
        Some(HttpMethod::Delete)
    );
    assert_eq!(HttpMethod::from_str_ignore_case("TRACE"), None);

    // as_str 是 HTTP 请求行使用的大写形态
    assert_eq!(HttpMethod::Head.as_str(), "HEAD");
}

// ---------------------------------------------------------------------------
// 测试夹具
// ---------------------------------------------------------------------------

fn sample_book() -> Book {
    Book {
        id: 1,
        origin: BookOrigin::Network,
        origin_id: "source-a|https://example.com/book/1".to_owned(),
        name: "测试书".to_owned(),
        author: Some("作者".to_owned()),
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: Some("https://example.com/book/1/toc".to_owned()),
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: None,
        format: None,
        total_chapters: Some(10),
        dur_chapter_index: Some(0),
        added_at: 1_700_000_000,
    }
}

fn sample_chapter() -> Chapter {
    Chapter {
        id: 1,
        book_id: 1,
        index_in_book: 0,
        title: "第一章".to_owned(),
        url: Some("https://example.com/book/1/c1".to_owned()),
        byte_offset: None,
        epub_entry: None,
        is_cached: false,
        is_read: false,
        word_count: Some(3000),
    }
}

fn sample_source() -> tome_core::model::Source {
    tome_core::model::Source {
        id: "source-a".to_owned(),
        name: "示例源".to_owned(),
        kind: tome_core::model::SourceKind::Book,
        group_id: None,
        enabled: true,
        book_source_type: None,
        book_url_pattern: None,
        rule_search: None,
        rule_explore: None,
        rule_book_info: None,
        rule_toc: None,
        rule_content: None,
        header: None,
        login_url: None,
        login_ui: None,
        login_check_js: None,
        js: None,
        charset: None,
        rate_limit_ms: None,
        weight: None,
        explore_screen: None,
        comment: None,
        last_update_time: None,
        created_at: 1_700_000_000,
    }
}
