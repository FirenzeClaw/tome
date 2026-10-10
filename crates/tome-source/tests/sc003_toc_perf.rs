//! SC-003 实测：5 万章目录的**解析** < 1s（tasks.md T066 验收项）
//!
//! # 口径：这条验收项约束的是解析，不是下载
//!
//! 一个 5 万章的目录页在慢站点上光是传输就不止 1 秒。若把下载计入，
//! 这条验收项就变成了"源站有多快"的度量，而不是"我们的解析有多快"。
//!
//! 故本测试分两段计时并分别报告：
//!
//! | 段 | 计入 SC-003 | 说明 |
//! |---|---|---|
//! | HTML 解析（DOM 构建） | ✅ | 是解析路径的一部分 |
//! | 规则求值（5 万章逐个字段） | ✅ | 解析路径的主体 |
//! | 网络传输 | ❌ | 用 `FakeHost`（内存返回） |
//!
//! # 为什么用 `--release` 跑
//!
//! debug 构建的 CSS 选择器求值比 release 慢 10-30 倍，用 debug 判定
//! 会让"通过的实现"看起来不达标、或逼着实现去做无意义的微优化。
//! CI 的命令是 `cargo test --workspace --release`（见 AGENTS.md）。
//!
//! 若在 debug 下跑，本测试**自动放宽**阈值并明确说明——一个在 debug 下
//! 静默失败的性能测试只会被忽略。

use std::time::{Duration, Instant};

use tome_core::{ErrorKind, HeaderMap, NetHost, RequestSpec, Response};
use tome_source::source::parse_sources;
use tome_source::toc;

/// 目标章数与阈值（SC-003：5 万章 <1s）。
const CHAPTER_COUNT: usize = 50_000;
const THRESHOLD: Duration = Duration::from_secs(1);

/// 生成 5 万章的目录页 HTML。
///
/// # 为什么用"接近真实"的结构而不是最短可能的形式
///
/// 最短形式（`<a>` 列表）会让 CSS 选择器和 DOM 构建都走在最省的路上，
/// 测出的数字比真实站点乐观。这里掺入真实的目录页噪音：外层容器、
/// 分卷标题、章节条目里的多个子元素（序号 + 标题 + 可能的标记）。
fn toc_html(chapters: usize) -> String {
    let mut html = String::with_capacity(chapters * 120);
    html.push_str("<html><head><title>目录</title></head><body>");
    html.push_str(r#"<div class="book-info"><h1>书名</h1><p>作者</p></div>"#);
    html.push_str(r#"<div id="list">"#);

    let mut volume = 0usize;
    for index in 1..=chapters {
        // 每 1000 章一个分卷标题（真实目录页的常见结构）
        if index % 1000 == 1 {
            volume += 1;
            html.push_str(&format!(r#"<dl class="volume"><dt>第{volume}卷</dt>"#));
        }
        html.push_str(&format!(
            r#"<dd class="chapter" data-i="{index}"><span class="idx">{index}</span><a href="/book/1/{index}.html">第{index}章 标题文字{index}</a><i class="vip"></i></dd>"#
        ));
        if index % 1000 == 0 {
            html.push_str("</dl>");
        }
    }

    html.push_str("</div></body></html>");
    html
}

/// 分段计时（诊断用）：把解析路径拆成 DOM / 选节点 / 逐章求值三段。
///
/// # 为什么保留这条测试
///
/// SC-003 只有一个"<1s"的结论，而结论达标时看不出余量在哪段。这条
/// 测试把三段分别计时并打印——当某天它变慢时，能立刻定位是哪一段，
/// 而不必重新做一次性能分析。
#[test]
fn sc003_分段计时诊断() {
    // 本测试不走 `toc()`，故不需要 ParsedSource——只验证分段耗时
    let _source_unused = parse_sources(
        r#"{"bookSourceName":"prof","bookSourceUrl":"https://prof.test",
            "ruleToc":"{\"chapterList\":\"@css:#list dd.chapter\",\"chapterName\":\"@css:a@text\",\"chapterUrl\":\"@css:a@href\"}"}"#,
    )
    .expect("应能解析")
    .sources
    .into_iter()
    .next()
    .expect("应有一条源");

    let html = toc_html(CHAPTER_COUNT);

    let t0 = Instant::now();
    let document = tome_rule::dom::Document::parse_html(&html);
    let dom = t0.elapsed();

    let t1 = Instant::now();
    let nodes = tome_rule::css_post::select_nodes(&document, "@css:#list dd.chapter")
        .expect("选节点应成功");
    let select = t1.elapsed();

    // 逐章求值三段（标题/地址/序号），与 `parse_toc` 的字段集一致——
    // 只测标题会低估真实开销
    let t2 = Instant::now();
    let engine = tome_rule::css_post::RuleEngine::new(&document);
    let mut titles = 0usize;
    for node in &nodes {
        let title = engine.evaluate_in(node, "@css:a@text").expect("应成功");
        let href = engine.evaluate_in(node, "@css:a@href").expect("应成功");
        let _idx = engine
            .evaluate_in(node, "@css:dd@attr(data-i)")
            .expect("应成功");
        if title.into_iter().next().is_some() && href.into_iter().next().is_some() {
            titles += 1;
        }
    }
    let per_chapter = t2.elapsed();

    eprintln!(
        "[SC-003 分段] DOM构建={dom:?} 选节点={select:?}（{} 个） 逐章求值={per_chapter:?}（{titles} 个标题）",
        nodes.len()
    );
    assert_eq!(nodes.len(), CHAPTER_COUNT);
    assert_eq!(titles, CHAPTER_COUNT);
}

/// 内存宿主：不触网，直接返回预设正文。
struct FakeHost {
    body: Vec<u8>,
}

impl NetHost for FakeHost {
    fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
        Ok(Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: self.body.clone(),
            detected_charset: Some("utf-8".to_string()),
        })
    }
}

#[test]
fn sc003_五万章目录解析在一秒内() {
    let source = parse_sources(
        r#"{"bookSourceName":"SC003","bookSourceUrl":"https://sc003.test",
            "ruleToc":"{\"chapterList\":\"@css:#list dd.chapter\",\"chapterName\":\"@css:a@text\",\"chapterUrl\":\"@css:a@href\",\"chapterIndex\":\"@css:dd@attr(data-i)\"}"}"#,
    )
    .expect("书源应能解析")
    .sources
    .into_iter()
    .next()
    .expect("应有一条源");

    let html = toc_html(CHAPTER_COUNT);
    eprintln!(
        "[SC-003] 目录页 HTML 大小 = {:.2} MiB（{CHAPTER_COUNT} 章）",
        html.len() as f64 / 1024.0 / 1024.0
    );

    // ① DOM 构建耗时（它是解析路径的一部分，单独报告以便定位瓶颈）
    let document_start = Instant::now();
    let document = tome_rule::dom::Document::parse_html(&html);
    let document_elapsed = document_start.elapsed();
    drop(document);

    // ② 端到端解析（含 DOM 构建 + 5 万章的规则求值）
    let host = FakeHost {
        body: html.as_bytes().to_vec(),
    };
    let book = tome_core::Book {
        id: 1,
        origin: tome_core::BookOrigin::Network,
        origin_id: "https://sc003.test|https://sc003.test/book/1".to_string(),
        name: "书".to_string(),
        author: None,
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: Some("https://sc003.test/book/1/toc".to_string()),
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: None,
        format: None,
        total_chapters: None,
        dur_chapter_index: None,
        added_at: 0,
    };

    let start = Instant::now();
    let chapters = toc::toc(&source, &book, &host).expect("目录应能解析");
    let elapsed = start.elapsed();

    // ③ 正确性：不能为了快而少解析
    assert_eq!(
        chapters.len(),
        CHAPTER_COUNT,
        "必须真的解析出 5 万章（不是提前退出）"
    );
    // 本测试的 HTML 里 `data-i` 从 1 起（真实目录页的编号习惯），
    // 而 `chapterIndex` 规则被显式配置为读它，故序号跟着 1 起——
    // 这正好顺带验证了"书源显式给出的序号优先于列表位置"。
    assert_eq!(chapters[0].index_in_book, 1);
    assert_eq!(
        chapters[CHAPTER_COUNT - 1].index_in_book,
        CHAPTER_COUNT as i32
    );
    assert_eq!(chapters[0].title, "第1章 标题文字1");
    assert_eq!(
        chapters[0].url.as_deref(),
        Some("https://sc003.test/book/1/1.html"),
        "相对地址应被补全"
    );
    assert_eq!(
        chapters[4].index_in_book, 5,
        "书源显式给出的序号应被采用（data-i 编号）"
    );

    // ④ 判定
    //
    // debug 构建下 CSS 求值慢 10-30 倍，此时**放宽并明确说明**——
    // 一个在 debug 下静默失败的性能测试只会被忽略。
    let debug_build = cfg!(debug_assertions);
    let effective_threshold = if debug_build {
        THRESHOLD * 30
    } else {
        THRESHOLD
    };

    eprintln!(
        "[SC-003 实测] 章数={CHAPTER_COUNT} 端到端解析={:?} DOM构建={:?} 阈值={:?}{}",
        elapsed,
        document_elapsed,
        effective_threshold,
        if debug_build {
            "（debug 构建，阈值已放宽 30×）"
        } else {
            ""
        }
    );
    eprintln!(
        "[SC-003 实测] 平均每章 = {:.2} 微秒",
        elapsed.as_micros() as f64 / CHAPTER_COUNT as f64
    );

    assert!(
        elapsed < effective_threshold,
        "SC-003：5 万章目录解析应在 {effective_threshold:?} 内完成，实际 {elapsed:?}\n\
         DOM 构建 {document_elapsed:?}；若瓶颈在 DOM 构建，应优化 html5ever 的用法而非规则求值"
    );
}

/// 目录解析的**规模不变量**：耗时随章数大致线性，不出现超线性。
///
/// # 为什么单独一条
///
/// "5 万章 <1s"可能被"线性但常数极小"或"超线性但当前规模还小"两种实现
/// 满足。前者在 10 万章时仍然可用，后者会突然崩掉。这条测试用两个规模
/// 对比，把"是不是超线性"变成可判定的事实。
///
/// 用 1 万 : 5 万 = 1 : 5 对比，允许 3 倍宽裕（缓存、分配器抖动都会影响
/// 单次测量，而测试的价值在于抓 O(n²) 那种量级的异常）。
#[test]
fn sc003_解析耗时随章数线性而非超线性() {
    let source = parse_sources(
        r#"{"bookSourceName":"SC003b","bookSourceUrl":"https://sc003b.test",
            "ruleToc":"{\"chapterList\":\"@css:#list dd.chapter\",\"chapterName\":\"@css:a@text\",\"chapterUrl\":\"@css:a@href\"}"}"#,
    )
    .expect("书源应能解析")
    .sources
    .into_iter()
    .next()
    .expect("应有一条源");

    let book = |_url: &str| tome_core::Book {
        id: 1,
        origin: tome_core::BookOrigin::Network,
        origin_id: "https://sc003b.test|https://sc003b.test/book/1".to_string(),
        name: "书".to_string(),
        author: None,
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: Some("https://sc003b.test/toc".to_string()),
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: None,
        format: None,
        total_chapters: None,
        dur_chapter_index: None,
        added_at: 0,
    };

    let measure = |count: usize| -> Duration {
        let html = toc_html(count);
        let host = FakeHost {
            body: html.into_bytes(),
        };
        // 先跑一次热身（分配器预热、页表建立），计时只取第二次
        let _ = toc::toc(&source, &book(""), &host).expect("应成功");
        let start = Instant::now();
        let chapters = toc::toc(&source, &book(""), &host).expect("应成功");
        let elapsed = start.elapsed();
        assert_eq!(chapters.len(), count);
        elapsed
    };

    let small = measure(10_000);
    let large = measure(50_000);

    // 章数比 5×。线性实现下耗时比应接近 5；给到 15 倍宽裕
    // （1 万章的测量基数太小，抖动占比大）。超线性（O(n²)）会是 25×。
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    eprintln!(
        "[SC-003 线性度] 1万章={small:?} 5万章={large:?} 耗时比={ratio:.2}（线性应约 5.0，O(n²) 会到 25）"
    );
    assert!(
        ratio < 15.0,
        "解析耗时比 {ratio:.2} 过高，疑似超线性。1万章 {small:?}，5万章 {large:?}"
    );
}
