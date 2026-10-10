//! SC-014 与 SC-018 的端到端实测（tasks.md T068 验收项）
//!
//! # 这两条度量的是什么
//!
//! | 验收项 | 原文 | 本测试怎么证 |
//! |---|---|---|
//! | **SC-014** | 图片失败 MUST NOT 中断正文获取 | 用一个"正文能取、所有图都挂"的宿主跑完整链路，断言正文照常返回 |
//! | **SC-018** | 鉴权图片获取成功率与正文一致 | 同一个宿主要求鉴权头，分别跑正文与图片，断言两者**都成功**或**都失败** |
//!
//! # 为什么 SC-018 要"两侧都跑"而不是只跑图片
//!
//! "与正文一致"是一个**相对**断言。只测图片一侧（"带头的图片请求成功了"）
//! 证明不了"它与正文一致"——那可能只是因为宿主恰好不看头。
//! 故两侧都跑，并对比结果：宿主认头则两者都成功，宿主不认头则两者都失败。
//! 差异出现即意味着图片路径少带了什么。

// 集成测试的夹具与断言函数——失败即 panic 是测试框架报告失败的唯一机制
// （AGENTS.md 禁的是生产路径上的 panic）。`.clippy.toml` 的豁免只覆盖
// `#[test]` 函数体，不覆盖它们调用的辅助函数，故需要文件级豁免。
#![expect(
    clippy::expect_used,
    reason = "集成测试的夹具与断言——失败即 panic 是测试框架报告失败的唯一机制"
)]

use std::sync::Mutex;

use tome_core::{ErrorKind, HeaderMap, NetHost, RequestSpec, Response};
use tome_source::content;
use tome_source::image::{self, ImagePolicy};
use tome_source::source::{parse_sources, ParsedSource};

/// 宿主：**要求**请求带指定头，否则返回 403。
///
/// # 为什么需要"要求鉴权"的宿主
///
/// 一个不检查头的宿主无法区分"图片请求带了头"与"没带头"——两者都返回
/// 200，测试就永远绿。本宿主把"缺头"变成可观测的失败，从而让 SC-018
/// 的断言有内容。
struct AuthHost {
    /// 要求的头名与值
    required: (String, String),
    /// 正文页内容
    content_html: String,
    seen: Mutex<Vec<RequestSpec>>,
}

impl AuthHost {
    fn new(required: (&str, &str), content_html: &str) -> Self {
        Self {
            required: (required.0.to_string(), required.1.to_string()),
            content_html: content_html.to_string(),
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

impl NetHost for AuthHost {
    fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(spec.clone());

        // 鉴权检查：缺头或缺对值即 403（模拟真实图床/需登录的站点）
        if spec.headers.get(&self.required.0) != Some(self.required.1.as_str()) {
            return Ok(Response {
                status: 403,
                headers: HeaderMap::new(),
                body_bytes: b"forbidden".to_vec(),
                detected_charset: None,
            });
        }

        // 带对了头：图片请求返回图片字节，其余返回正文页
        let is_image = spec.url.ends_with(".png") || spec.url.ends_with(".jpg");
        let body = if is_image {
            b"\x89PNG\r\n\x1a\n".to_vec()
        } else {
            self.content_html.as_bytes().to_vec()
        };
        Ok(Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: body,
            detected_charset: Some("utf-8".to_string()),
        })
    }
}

/// 源：正文规则 + 需要鉴权的站点。
fn source_with_auth(auth_value: &str) -> ParsedSource {
    let text = format!(
        r#"{{
            "bookSourceName":"鉴权源",
            "bookSourceUrl":"https://auth.test",
            "header":"{{\"Authorization\":\"{auth_value}\"}}",
            "ruleContent":"{{\"content\":\"@css:#body@text\"}}"
        }}"#
    );
    parse_sources(&text)
        .expect("应能解析")
        .sources
        .into_iter()
        .next()
        .expect("应有一条源")
}

fn book() -> tome_core::Book {
    tome_core::Book {
        id: 1,
        origin: tome_core::BookOrigin::Network,
        origin_id: "https://auth.test|https://auth.test/book/1".to_string(),
        name: "书".to_string(),
        author: None,
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: None,
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

fn chapter(index: i32) -> tome_core::Chapter {
    tome_core::Chapter {
        id: 0,
        book_id: 1,
        index_in_book: index,
        title: format!("第{index}章"),
        url: Some(format!("https://auth.test/c/{index}.html")),
        byte_offset: None,
        epub_entry: None,
        is_cached: false,
        is_read: false,
        word_count: None,
    }
}

fn fast_policy() -> ImagePolicy {
    ImagePolicy {
        max_retries: 1,
        backoff_ms: 0,
        max_bytes: 1024 * 1024,
    }
}

// ---------------------------------------------------------------------------
// SC-018：鉴权图片成功率与正文一致
// ---------------------------------------------------------------------------

#[test]
fn sc018_鉴权头正确时正文与图片都成功() {
    let source = source_with_auth("Bearer CORRECT-TOKEN");
    let html = r#"<div id="body">正文内容</div><img src="/pic.png"/>"#;
    // 宿主**要求**这个头存在且值正确
    let host = AuthHost::new(("Authorization", "Bearer CORRECT-TOKEN"), html);

    // 正文侧
    let text = content::chapter_content(&source, &book(), &chapter(0), &host, None)
        .expect("正文应成功")
        .raw;
    assert_eq!(text, "正文内容");

    // 图片侧
    let urls =
        image::extract_image_urls(&source, html, "https://auth.test/c/0.html").expect("应能提取");
    assert_eq!(urls.len(), 1);
    let set = image::fetch_images(&source, &urls, &host, None, &fast_policy()).expect("应成功");

    // SC-018：两侧都成功
    assert_eq!(set.fetched_count(), 1, "正文能取到时图片也必须能取到");
    assert_eq!(
        set.success_rate(),
        1.0,
        "鉴权头正确时图片成功率应为 100%（与正文一致）"
    );
}

#[test]
fn sc018_鉴权头缺失时正文与图片都失败() {
    // 反向验证：宿主要求的头**不在**书源声明里。
    // 若图片路径偷偷补了头而正文路径没有（或反之），两侧结果就会不一致
    // ——那正是 SC-018 要抓的偏差。
    let source = source_with_auth("Bearer WRONG-TOKEN");
    let html = r#"<div id="body">正文内容</div><img src="/pic.png"/>"#;
    let host = AuthHost::new(("Authorization", "Bearer CORRECT-TOKEN"), html);

    // 正文侧：403 → 网络错误
    let content_result = content::chapter_content(&source, &book(), &chapter(0), &host, None);
    assert!(
        content_result.is_err(),
        "正文应因鉴权失败而报错：{content_result:?}"
    );

    // 图片侧：同样 403 → 占位
    let urls =
        image::extract_image_urls(&source, html, "https://auth.test/c/0.html").expect("应能提取");
    let set = image::fetch_images(&source, &urls, &host, None, &fast_policy()).expect("应成功");
    assert_eq!(set.placeholder_count(), 1);
    assert_eq!(set.success_rate(), 0.0);

    // **两侧结果一致**：都失败。这是 SC-018 的实质。
}

#[test]
fn sc018_图片请求确实携带_authorization_头() {
    // 直接断言"带没带"，比"成功率一致"更靠近根因——
    // 后者失败时还要再查是哪一侧的问题
    let source = source_with_auth("Bearer TOKEN");
    let host = AuthHost::new(("Authorization", "Bearer TOKEN"), "<p>x</p>");
    let _ = image::fetch_image(
        &source,
        "https://auth.test/pic.png",
        &host,
        None,
        &fast_policy(),
    )
    .expect("应成功");

    let requests = host.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers.get("Authorization"),
        Some("Bearer TOKEN"),
        "图片请求必须携带书源声明的 Authorization 头"
    );
}

// ---------------------------------------------------------------------------
// SC-014：图片失败不中断正文
// ---------------------------------------------------------------------------

/// 宿主：正文页正常，但**所有**图片请求失败。
struct BrokenImagesHost {
    content_html: String,
}

impl NetHost for BrokenImagesHost {
    fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
        // 图片一律传输层失败（模拟图床宕机 / CDN 被墙）
        let is_image = spec.url.contains(".png")
            || spec.url.contains(".jpg")
            || spec.url.contains("img")
            || spec.url.contains("pic");
        if is_image {
            return Err(ErrorKind::Network);
        }
        Ok(Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: self.content_html.as_bytes().to_vec(),
            detected_charset: Some("utf-8".to_string()),
        })
    }
}

#[test]
fn sc014_全部图片失败时正文照常返回() {
    let source = source_with_auth("Bearer T");
    let html = r#"<div id="body">这段话必须能读到</div>
                  <img src="/1.png"/><img src="/2.png"/><img src="/3.png"/>"#;
    let host = BrokenImagesHost {
        content_html: html.to_string(),
    };

    // 正文侧
    let outcome = content::chapter_content(&source, &book(), &chapter(0), &host, None)
        .expect("图片全挂不该影响正文");
    assert!(
        outcome.raw.contains("这段话必须能读到"),
        "正文必须完整返回：{}",
        outcome.raw
    );

    // 图片侧：全部占位，但**调用本身成功**
    let urls =
        image::extract_image_urls(&source, html, "https://auth.test/c/0.html").expect("应能提取");
    assert_eq!(urls.len(), 3);
    let set = image::fetch_images(&source, &urls, &host, None, &fast_policy())
        .expect("图片全挂时 fetch_images 仍应返回 Ok");
    assert_eq!(set.placeholder_count(), 3);
    assert_eq!(set.success_rate(), 0.0);

    // 每一张都带上了失败原因（供界面展示"为什么是占位"）
    for result in &set.images {
        match result {
            image::ImageResult::Placeholder { reason, .. } => {
                assert!(!reason.is_empty(), "占位必须带原因");
            }
            other => panic!("应是占位：{other:?}"),
        }
    }
}

#[test]
fn sc014_图片挂掉时正文的缓存路径也正常() {
    // 缓存命中的路径上同样不该受图片影响——那是最容易被漏掉的一条，
    // 因为缓存命中时根本不走网络
    use std::collections::HashMap;

    #[derive(Default)]
    struct Cache {
        entries: Mutex<HashMap<i64, Vec<u8>>>,
    }
    impl content::ChapterCache for Cache {
        fn get(&self, id: i64) -> Result<Option<Vec<u8>>, tome_source::SourceError> {
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&id)
                .cloned())
        }
        fn put(&self, id: i64, bytes: &[u8]) -> Result<(), tome_source::SourceError> {
            self.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(id, bytes.to_vec());
            Ok(())
        }
    }

    let source = source_with_auth("Bearer T");
    let html = r#"<div id="body">缓存里的正文</div><img src="/x.png"/>"#;
    let host = BrokenImagesHost {
        content_html: html.to_string(),
    };
    let cache = Cache::default();
    // 预置缓存条目
    cache
        .entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(42, "已缓存的正文".as_bytes().to_vec());

    let mut cached_chapter = chapter(0);
    cached_chapter.id = 42;
    let outcome = content::chapter_content(&source, &book(), &cached_chapter, &host, Some(&cache))
        .expect("缓存命中应成功");
    assert!(outcome.from_cache);
    assert_eq!(outcome.raw, "已缓存的正文");
}

#[test]
fn sc014_部分图片失败时其余图片仍可取到() {
    // "不中断"同理适用于图片之间：一章几十张图，一张挂在失效 CDN 是常态，
    // 整批失败会让读者连能看的都看不到
    struct PartialHost {
        content_html: String,
    }
    impl NetHost for PartialHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            if spec.url.contains("bad") {
                return Err(ErrorKind::Network);
            }
            let is_image = spec.url.ends_with(".png");
            Ok(Response {
                status: 200,
                headers: HeaderMap::new(),
                body_bytes: if is_image {
                    b"\x89PNG".to_vec()
                } else {
                    self.content_html.as_bytes().to_vec()
                },
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    let source = source_with_auth("Bearer T");
    let html = r#"<div id="body">正文</div>
                  <img src="/ok1.png"/><img src="/bad.png"/><img src="/ok2.png"/>"#;
    let host = PartialHost {
        content_html: html.to_string(),
    };

    let urls =
        image::extract_image_urls(&source, html, "https://auth.test/c/0.html").expect("应能提取");
    let set = image::fetch_images(&source, &urls, &host, None, &fast_policy()).expect("应成功");

    assert_eq!(set.fetched_count(), 2, "能取的图必须取到");
    assert_eq!(set.placeholder_count(), 1, "只有坏的那张走占位");
}
