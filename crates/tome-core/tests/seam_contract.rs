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
    AnchorQuery, BackupJson, BookRepository, CacheKind, CancelToken, ChangeSourceOutcome,
    ChapterContent, CheckReport, CheckStream, CheckVerdict, ContentProvider, ExplorePage,
    HighlightRuleRow, ImportFailure, ImportReport, ProgressMapping, SearchHit, SearchResultItem,
    SearchStream, SourceCheckFilter, SourceEngine, SourceImportChannel,
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
            // 这一章无图 —— 空列表是"无图"的正常表达（见 `ImageRef` 的说明）
            images: Vec::new(),
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

    fn check_sources(&self, _filter: &SourceCheckFilter) -> Box<dyn CheckStream> {
        Box::new(FakeCheckStream {
            token: CancelToken::new(),
            pending: std::collections::VecDeque::new(),
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

/// FR-008：`RequestSpec` 必须含**全部**请求定制维度。
///
/// 这条测试的价值在于：任一字段被删掉，此处**编译失败**——
/// 比断言某个 getter 更早、更硬。
///
/// （`request_interval_ms` 是 2026-10-10 PM 仲裁 R2 补入的——FR-008 的
/// "请求间隔"原先只有 `rate_limit_key` 表达"按源限速"，没有表达
/// "该源要求多大间隔"的字段。）
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
        request_interval_ms: Some(2_000),
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
    assert_eq!(spec.request_interval_ms, Some(2_000));
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
        search_url: None,
        explore_url: None,
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

// ---------------------------------------------------------------------------
// 契约 §1/§3 的其余方法（批量源检查、备份、规则 CRUD）
// ---------------------------------------------------------------------------

/// 极简 `CheckStream` 实现——证明批量源检查的流同样可被外部 crate 实现。
///
/// 用 `VecDeque` 从队首取（FIFO）：真实流是"按源顺序产出"的，
/// 用 `Vec::pop`（LIFO）会让夹具的语义与断言顺序相反，制造无谓的困惑。
struct FakeCheckStream {
    token: CancelToken,
    pending: std::collections::VecDeque<CheckReport>,
}

impl CheckStream for FakeCheckStream {
    fn next_report(&mut self) -> Option<CheckReport> {
        if self.token.is_cancelled() {
            return None;
        }
        self.pending.pop_front()
    }

    fn cancel_token(&self) -> &CancelToken {
        &self.token
    }
}

/// FR-011：批量源检查是**流式**的，且可取消（契约 §1 的 `check_sources`）。
///
/// 为什么必须是流：批量检查可能涉及上百个源、每个都要发请求。用 `Vec`
/// 会把检查进度完全藏起来，界面只能长时间无反馈。
#[test]
fn 批量源检查返回可取消的流() {
    let token = CancelToken::new();
    let mut stream: Box<dyn CheckStream> = Box::new(FakeCheckStream {
        token: token.clone(),
        pending: std::collections::VecDeque::from(vec![
            CheckReport {
                source: sample_source(),
                verdict: CheckVerdict::Available,
            },
            CheckReport {
                source: sample_source(),
                verdict: CheckVerdict::Unavailable {
                    reason: ErrorKind::Network,
                },
            },
        ]),
    });

    // 逐源产出（不是攒齐再给）
    let first = stream.next_report().expect("应有检查结果");
    assert_eq!(first.verdict, CheckVerdict::Available);

    // 单源不可用**不终止**流：第二个结果仍可达
    let second = stream.next_report().expect("失败源不应终止流");
    assert!(matches!(
        second.verdict,
        CheckVerdict::Unavailable {
            reason: ErrorKind::Network
        }
    ));

    // 取完后结束
    assert!(stream.next_report().is_none());

    // 取消令牌可用
    stream.cancel_token().cancel();
    assert!(token.is_cancelled());
}

/// `SourceCheckFilter` 的默认值是"检查全部启用的源"（空过滤即全量）。
#[test]
fn 源检查过滤条件默认全量() {
    let filter = SourceCheckFilter::default();
    assert!(filter.source_ids.is_empty());
    assert_eq!(filter.group_id, None);

    let scoped = SourceCheckFilter {
        source_ids: vec!["source-a".to_owned()],
        group_id: Some(2),
    };
    assert_ne!(scoped, filter);
}

/// 极简 `BookRepository` 实现——证明该 trait 的方法集是可实现的，
/// 且覆盖契约 §3 声明的全部方法组。
struct FakeRepository;

impl BookRepository for FakeRepository {
    fn bookshelf(&self) -> Result<Vec<Book>, ErrorKind> {
        Ok(vec![sample_book()])
    }
    fn add_book(&self, _book: &Book) -> Result<i64, ErrorKind> {
        Ok(1)
    }
    fn remove_book(&self, _book_id: i64) -> Result<(), ErrorKind> {
        Ok(())
    }
    fn update_book(&self, _book: &Book) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn chapters(&self, _book_id: i64) -> Result<Vec<Chapter>, ErrorKind> {
        Ok(vec![sample_chapter()])
    }
    fn save_chapters(&self, _book_id: i64, _chapters: &[Chapter]) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn get_progress(
        &self,
        _book_id: i64,
    ) -> Result<Option<tome_core::model::ReadProgress>, ErrorKind> {
        Ok(None)
    }
    fn save_progress(&self, _progress: &tome_core::model::ReadProgress) -> Result<(), ErrorKind> {
        Ok(())
    }
    fn add_read_record(&self, _record: &tome_core::model::ReadRecord) -> Result<(), ErrorKind> {
        Ok(())
    }
    fn read_records(
        &self,
        _b: i64,
        _from: &str,
        _to: &str,
    ) -> Result<Vec<tome_core::model::ReadRecord>, ErrorKind> {
        Ok(Vec::new())
    }

    fn bookmarks(&self, _book_id: i64) -> Result<Vec<tome_core::model::Bookmark>, ErrorKind> {
        Ok(Vec::new())
    }
    fn add_bookmark(&self, _bookmark: &tome_core::model::Bookmark) -> Result<i64, ErrorKind> {
        Ok(1)
    }
    fn remove_bookmark(&self, _id: i64) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn highlights(&self, _book_id: i64) -> Result<Vec<tome_core::model::Highlight>, ErrorKind> {
        Ok(Vec::new())
    }
    fn add_highlight(&self, _highlight: &tome_core::model::Highlight) -> Result<i64, ErrorKind> {
        Ok(1)
    }
    fn remove_highlight(&self, _id: i64) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn memos(&self, _book_id: i64) -> Result<Vec<tome_core::model::Memo>, ErrorKind> {
        Ok(Vec::new())
    }
    fn add_memo(&self, _memo: &tome_core::model::Memo) -> Result<i64, ErrorKind> {
        Ok(1)
    }
    fn remove_memo(&self, _id: i64) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn sources(&self) -> Result<Vec<tome_core::model::Source>, ErrorKind> {
        Ok(vec![sample_source()])
    }
    fn source_by_id(&self, _id: &str) -> Result<Option<tome_core::model::Source>, ErrorKind> {
        Ok(None)
    }
    fn import_sources(&self, _channel: &SourceImportChannel) -> Result<ImportReport, ErrorKind> {
        Ok(ImportReport::default())
    }
    fn import_parsed_sources(
        &self,
        _sources: &[tome_core::model::Source],
    ) -> Result<ImportReport, ErrorKind> {
        Ok(ImportReport::default())
    }
    fn source_groups(&self) -> Result<Vec<tome_core::model::SourceGroup>, ErrorKind> {
        Ok(Vec::new())
    }

    fn replace_rules(&self) -> Result<Vec<tome_core::model::ReplaceRule>, ErrorKind> {
        Ok(Vec::new())
    }
    fn save_replace_rule(&self, _rule: &tome_core::model::ReplaceRule) -> Result<i64, ErrorKind> {
        Ok(1)
    }
    fn remove_replace_rule(&self, _id: i64) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn highlight_rules(&self) -> Result<Vec<HighlightRuleRow>, ErrorKind> {
        Ok(Vec::new())
    }
    fn save_highlight_rule(&self, _rule: &HighlightRuleRow) -> Result<i64, ErrorKind> {
        Ok(1)
    }
    fn remove_highlight_rule(&self, _id: i64) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn dict_rules(&self) -> Result<Vec<tome_core::model::DictRule>, ErrorKind> {
        Ok(Vec::new())
    }
    fn save_dict_rule(&self, _rule: &tome_core::model::DictRule) -> Result<i64, ErrorKind> {
        Ok(1)
    }
    fn remove_dict_rule(&self, _id: i64) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn txt_toc_rules(&self) -> Result<Vec<tome_core::model::TxtTocRule>, ErrorKind> {
        Ok(Vec::new())
    }
    fn save_txt_toc_rule(&self, _rule: &tome_core::model::TxtTocRule) -> Result<i64, ErrorKind> {
        Ok(1)
    }
    fn remove_txt_toc_rule(&self, _id: i64) -> Result<(), ErrorKind> {
        Ok(())
    }

    fn library_roots(&self) -> Result<Vec<tome_core::model::LibraryRoot>, ErrorKind> {
        Ok(Vec::new())
    }
    fn add_library_root(&self, _path: &str) -> Result<i64, ErrorKind> {
        Ok(1)
    }

    fn search_in_book(&self, _book_id: i64, _query: &str) -> Result<Vec<SearchHit>, ErrorKind> {
        Ok(Vec::new())
    }

    fn export_all(&self) -> Result<BackupJson, ErrorKind> {
        Ok(BackupJson {
            json: "{}".to_owned(),
        })
    }
    fn import_backup(&self, _json: &BackupJson) -> Result<ImportReport, ErrorKind> {
        Ok(ImportReport::default())
    }

    fn clear_cache(&self, _kind: CacheKind) -> Result<(), ErrorKind> {
        Ok(())
    }
}

/// 契约 §3 的 `export_all` / `import_backup`（FR-033）。
///
/// 两者共用 `BackupJson`，故"导出与导入对称"由**类型**保证——
/// 而不是靠两处各自维护的字段列表（那是漂移的温床）。
#[test]
fn 备份导出与导入共用类型保证对称() {
    let repository: Box<dyn BookRepository> = Box::new(FakeRepository);

    let backup = repository.export_all().expect("导出应可用");
    assert!(!backup.json.is_empty());

    // 导出的结果可直接喂回导入——类型相同，无需字段映射
    let report = repository.import_backup(&backup).expect("导入应可用");
    assert_eq!(report, ImportReport::default());
}

/// 契约 §3 要求四类规则**都要 CRUD**，而不只是读。
///
/// FR-014/015/016 要求用户能配置净化/高亮/词典规则；FR-020 要求 TXT 目录
/// 识别支持"多条规则回退"（自然也需增删改）。只有只读 getter 是不够的。
#[test]
fn 四类规则均可增删改() {
    let repository = FakeRepository;

    // 替换规则
    assert!(repository.replace_rules().is_ok());
    assert!(repository.save_replace_rule(&sample_replace_rule()).is_ok());
    assert!(repository.remove_replace_rule(1).is_ok());

    // 高亮规则
    assert!(repository.highlight_rules().is_ok());
    assert!(repository
        .save_highlight_rule(&sample_highlight_rule())
        .is_ok());
    assert!(repository.remove_highlight_rule(1).is_ok());

    // 词典规则
    assert!(repository.dict_rules().is_ok());
    assert!(repository.save_dict_rule(&sample_dict_rule()).is_ok());
    assert!(repository.remove_dict_rule(1).is_ok());

    // TXT 目录识别规则（FR-020）
    assert!(repository.txt_toc_rules().is_ok());
    assert!(repository.save_txt_toc_rule(&sample_txt_toc_rule()).is_ok());
    assert!(repository.remove_txt_toc_rule(1).is_ok());
}

/// 书内全文搜索限定已缓存内容（FR-019b）——签名里没有任何"抓取"参数，
/// 实现方无从发起额外请求。
#[test]
fn 书内搜索限定缓存内容() {
    let repository = FakeRepository;
    let hits = repository.search_in_book(1, "关键词").expect("搜索应可用");
    assert!(hits.is_empty());
}

fn sample_replace_rule() -> tome_core::model::ReplaceRule {
    tome_core::model::ReplaceRule {
        id: 1,
        name: "去广告".to_owned(),
        pattern: "广告".to_owned(),
        replacement: String::new(),
        scope: tome_core::model::RuleScope::Content,
        is_regex: false,
        enabled: true,
        sort_order: 0,
    }
}

fn sample_highlight_rule() -> HighlightRuleRow {
    HighlightRuleRow {
        id: 1,
        name: "高亮".to_owned(),
        pattern: "关键词".to_owned(),
        style: "red".to_owned(),
        enabled: true,
        sort_order: 0,
    }
}

fn sample_dict_rule() -> tome_core::model::DictRule {
    tome_core::model::DictRule {
        id: 1,
        name: "词典".to_owned(),
        url_rule: "https://dict.example.com/{}".to_owned(),
        content_rule: "@css:.content".to_owned(),
        enabled: true,
    }
}

fn sample_txt_toc_rule() -> tome_core::model::TxtTocRule {
    tome_core::model::TxtTocRule {
        id: 1,
        name: "章节正则".to_owned(),
        rule: "^第.+章".to_owned(),
        example: Some("第一章".to_owned()),
        enabled: true,
        sort_order: 0,
    }
}

// ---------------------------------------------------------------------------
// FR-001：书源导入三渠道（契约 §3 的 `import_sources(file | url | text)`）
// ---------------------------------------------------------------------------

/// FR-001 要求导入渠道**至少覆盖三种**：本地文件、URL、粘贴文本。
///
/// 用枚举而非"已解析的 Source 列表"，是为了让三种渠道在**类型层面**可穷举——
/// 实现方 match 时必须处理全部三种。若只收 `&[Source]`，"URL 拉取"这一渠道
/// 在接缝上无法表达，会退化成上层各自实现（于是网络策略被复制多处，
/// 绕过 A2 的"唯一发请求处"）。
#[test]
fn 书源导入三渠道均可表达() {
    let text = r#"[{"name":"源","url":"https://example.com"}]"#;

    let file_channel = SourceImportChannel::FileText(text.to_owned());
    let url_channel = SourceImportChannel::Url("https://example.com/sources.json".to_owned());
    let pasted_channel = SourceImportChannel::PastedText(text.to_owned());

    // 三者互异——渠道是不同类型的导入，不是同一个字符串
    assert_ne!(url_channel, pasted_channel);
    assert_ne!(file_channel, pasted_channel);

    // URL 渠道**只承载 URL 本身**：拉取由实现方走 NetHost 进行，
    // 从而遵守限速/代理/重试与"唯一发请求处"（A2）。
    match &url_channel {
        SourceImportChannel::Url(url) => assert!(url.starts_with("https://")),
        other => panic!("应为 URL 渠道，实际 {other:?}"),
    }

    // 三渠道都可交给仓库
    let repository = FakeRepository;
    for channel in [file_channel, url_channel, pasted_channel] {
        assert!(repository.import_sources(&channel).is_ok());
    }
}

/// `ImportReport` 的失败明细类型必须**可从外部命名**。
///
/// 否则外部调用方能拿到 report、却无法写出它的失败元素类型
/// （`report.failures` 的静态类型），导致该公开字段半不可用。
#[test]
fn 导入报告的失败明细类型可命名() {
    let failure = ImportFailure {
        subject: "source-a".to_owned(),
        error: ErrorKind::Rule,
    };

    let report = ImportReport {
        added: 0,
        updated: 0,
        skipped: 1,
        failures: vec![failure],
    };

    // 关键：能写出 failures 元素的类型（这正是 ImportFailure 必须导出的理由）
    let first: &ImportFailure = report.failures.first().expect("应有一条失败明细");
    assert_eq!(first.subject, "source-a");
    assert_eq!(first.error.message_id(), "error.rule");
}
