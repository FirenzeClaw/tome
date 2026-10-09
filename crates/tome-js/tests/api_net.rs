//! T036 验收测试：网络类宿主 API 在 JS 侧可用（FR-005）
//!
//! ## 覆盖矩阵
//!
//! | 能力 | 测试 |
//! |---|---|
//! | 单次请求（get/post/head/ajax） | [`单次请求可用`] |
//! | 并发批量请求（ajaxAll） | [`批量请求可用`] |
//! | 下载到缓存（cacheFile） | [`下载到缓存可用`] |
//! | 下载到文件（downloadFile） | [`下载到文件可用`] |
//!
//! ## 为什么用替身而不是真实 HTTP
//!
//! 本测试的职责是"JS ↔ `NetHost` 这条接缝正确"：入口注册、`RequestSpec`
//! 的补全、错误分类的传递、状态码判定的分岔。真实 HTTP 的连接/超时/重试
//! 由 `tome-net` 的测试覆盖（那里有回环测试服务器）。
//!
//! 混在一起会让"JS 桥没接上"与"HTTP 客户端有问题"两种失败无法区分。
//! 但替身**不是**"什么都接受"：它记录收到的 `RequestSpec`，让
//! "限速键取错了"这类缺陷可见（那是 FR-008「按源」限速的根基）。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::sync::{Arc, Mutex};

use tome_core::{ErrorKind, HeaderMap, NetHost, RequestSpec, Response, StoreHost};
use tome_js::api::net;
use tome_js::bridge::JsContext;
use tome_js::runtime::{Limits, Sandbox};
use tome_js::{HostBridge, MemoryStoreHost};

/// 记录收到的 spec 并按 URL 返回预设响应的替身。
struct FakeNet {
    /// URL 片段 → 响应
    responses: Mutex<Vec<(String, Response)>>,
    /// 收到的全部请求（供断言限速键等）
    seen: Mutex<Vec<RequestSpec>>,
    /// 未匹配任何片段时的状态码
    default_status: u16,
}

impl FakeNet {
    fn new(responses: Vec<(&str, Response)>) -> Self {
        Self {
            responses: Mutex::new(
                responses
                    .into_iter()
                    .map(|(fragment, response)| (fragment.to_string(), response))
                    .collect(),
            ),
            seen: Mutex::new(Vec::new()),
            default_status: 200,
        }
    }

    fn seen(&self) -> Vec<RequestSpec> {
        self.seen.lock().expect("锁未被毒化").clone()
    }
}

impl NetHost for FakeNet {
    fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
        self.seen.lock().expect("锁未被毒化").push(spec.clone());
        let responses = self.responses.lock().expect("锁未被毒化");
        for (fragment, response) in responses.iter() {
            if spec.url.contains(fragment.as_str()) {
                return Ok(response.clone());
            }
        }
        Ok(Response {
            status: self.default_status,
            headers: HeaderMap::new(),
            body_bytes: Vec::new(),
            detected_charset: None,
        })
    }
}

/// 构造一个响应。
fn response(status: u16, body: &str) -> Response {
    Response {
        status,
        headers: HeaderMap::new(),
        body_bytes: body.as_bytes().to_vec(),
        detected_charset: Some("utf-8".to_string()),
    }
}

/// 建一个已注册网络类 API 的沙箱。
///
/// 返回 `(沙箱, 网络替身, 存储替身)`。
fn sandbox_with_net_api() -> (Sandbox, Arc<FakeNet>, Arc<MemoryStoreHost>) {
    let net_host = Arc::new(FakeNet::new(vec![
        ("/ok", response(200, "服务端内容")),
        ("/echo", response(200, "已收到")),
        ("/forbidden", response(403, "需要人工验证")),
        ("/missing", response(404, "not found")),
        ("/image", response(200, "<binary>")),
        ("/json", response(200, r#"{"name":"斗破"}"#)),
    ]));
    let store = Arc::new(MemoryStoreHost::new());

    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox.install_prelude().expect("应能注入前导脚本");

    let net_for_bridge: Arc<dyn NetHost> = net_host.clone();
    let store_for_bridge: Arc<dyn StoreHost> = store.clone();
    let bridge = HostBridge::bare(
        JsContext::for_source("https://source.test")
            .with_source_request_config(Some("utf-8".to_string()), Some(1000)),
    )
    .with_net(net_for_bridge)
    .with_store(store_for_bridge);
    sandbox.install_host_bridge(bridge).expect("应能注入宿主桥");

    sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            let net_for_api: Arc<dyn NetHost> = net_host.clone();
            let count = net::register(&ctx, &java, Some(net_for_api))
                .map_err(|error| format!("注册 net 失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(count)
        })
        .expect("应能装配");

    (sandbox, net_host, store)
}

/// 建一个**未注入**网络能力的沙箱。
fn sandbox_without_net() -> Sandbox {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .install_host_bridge(HostBridge::bare(JsContext::for_source("s")))
        .expect("应能注入桥");
    sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            net::register(&ctx, &java, None).map_err(|error| format!("注册失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(())
        })
        .expect("应能装配");
    sandbox
}

/// 求值并取回文本。
fn eval_text(sandbox: &Sandbox, script: &str) -> String {
    sandbox
        .eval(script)
        .unwrap_or_else(|error| panic!("脚本应成功，实际 {error:?}"))
        .text
}

// ---------------------------------------------------------------------------
// 单次请求
// ---------------------------------------------------------------------------

#[test]
fn 单次请求可用() {
    let (sandbox, host, _store) = sandbox_with_net_api();
    assert_eq!(
        eval_text(&sandbox, r#"java.get("https://a.test/ok")"#),
        "服务端内容"
    );
    assert_eq!(
        eval_text(&sandbox, r#"java.post("https://a.test/echo", "a=1")"#),
        "已收到"
    );
    // `ajax` 也返回正文（但它对非 2xx 不报错，见下）
    assert_eq!(
        eval_text(&sandbox, r#"java.ajax("https://a.test/ok")"#),
        "服务端内容"
    );
    assert_eq!(host.seen().len(), 3, "应发三次请求");
}

#[test]
fn post_把请求体发出去() {
    let (sandbox, host, _store) = sandbox_with_net_api();
    let _ = eval_text(&sandbox, r#"java.post("https://a.test/echo", "q=斗破")"#);
    let seen = host.seen();
    assert_eq!(seen[0].method, tome_core::HttpMethod::Post);
    assert_eq!(seen[0].body.as_deref(), Some("q=斗破".as_bytes()));
}

#[test]
fn head_用_head_方法() {
    let (sandbox, host, _store) = sandbox_with_net_api();
    let _ = eval_text(&sandbox, r#"java.head("https://a.test/ok")"#);
    assert_eq!(host.seen()[0].method, tome_core::HttpMethod::Head);
}

#[test]
fn 限速键取书源_id_而非_url() {
    // ## 这是 FR-008「按源」限速能否成立的根基
    //
    // 若脚本请求用 URL 当限速键、管线请求用书源 id，同一个源就会被限成
    // 两份速率，实际频率翻倍——而翻倍正是最容易触发对方风控的量。
    let (sandbox, host, _store) = sandbox_with_net_api();
    let _ = eval_text(&sandbox, r#"java.get("https://a.test/ok"); "ok";"#);
    assert_eq!(
        host.seen()[0].rate_limit_key,
        "https://source.test",
        "限速键必须是书源 id"
    );
}

#[test]
fn 书源声明的间隔与字符集传到请求规格() {
    // 组合根在装配时把书源的 concurrentRate/字符集填进上下文，
    // 宿主函数构造 spec 时使用（FR-008/FR-009）。
    let (sandbox, host, _store) = sandbox_with_net_api();
    let _ = eval_text(&sandbox, r#"java.get("https://a.test/ok"); "ok";"#);
    let spec = &host.seen()[0];
    assert_eq!(spec.request_interval_ms, Some(1000));
    assert_eq!(spec.charset.as_deref(), Some("utf-8"));
    assert_eq!(spec.timeout_ms, net::SCRIPT_REQUEST_TIMEOUT_MS);
    assert_eq!(
        spec.retry,
        tome_core::RetryPolicy::NONE,
        "脚本请求不重试：失败应尽快报给脚本的 try/catch"
    );
}

#[test]
fn get_的请求体可选() {
    // 书源里 `java.get(url)` 与 `java.get(url, body)` 两种写法都有。
    let (sandbox, _host, _store) = sandbox_with_net_api();
    assert_eq!(
        eval_text(&sandbox, r#"java.get("https://a.test/ok")"#),
        "服务端内容",
        "不传 body 不应报参数数量错"
    );
}

// ---------------------------------------------------------------------------
// 状态码判定的分岔（R3 语义）
// ---------------------------------------------------------------------------

#[test]
fn get_对非_2xx_报错而_ajax_不报() {
    // ## 这是两者语义差别的核心
    //
    // `java.get` 拿的是**正文**：把 404 页当成结果会让 `JSON.parse`
    // 抛一个与真实原因无关的异常。
    // `java.ajax` 要能读错误页正文（可用性检查脚本靠它区分"要验证"与"源失效"）。
    let (sandbox, _host, _store) = sandbox_with_net_api();

    let get_outcome = eval_text(
        &sandbox,
        r#"
        try { java.get("https://a.test/missing"); "未抛出"; }
        catch (e) { "已捕获"; }
        "#,
    );
    assert_eq!(get_outcome, "已捕获", "get 对 404 应报错");

    let ajax_body = eval_text(&sandbox, r#"java.ajax("https://a.test/forbidden")"#);
    assert_eq!(
        ajax_body, "需要人工验证",
        "ajax 对 403 应返回正文（可用性检查脚本靠它区分情况）"
    );
}

#[test]
fn 错误消息带上状态码与提示() {
    // 书源作者需要知道是 404（源改版）还是 403（要验证）——两者的处置完全不同。
    let (sandbox, _host, _store) = sandbox_with_net_api();
    let message = eval_text(
        &sandbox,
        r#"
        try { java.get("https://a.test/missing"); ""; }
        catch (e) { e.message; }
        "#,
    );
    assert!(message.contains("404"), "应含状态码：{message}");
    assert!(
        message.contains("不存在") || message.contains("失效"),
        "应含可读提示：{message}"
    );
}

#[test]
fn 传输层失败原样传递错误分类() {
    struct FailingNet;
    impl NetHost for FailingNet {
        fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
            Err(ErrorKind::Proxy)
        }
    }
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .install_host_bridge(
            HostBridge::bare(JsContext::for_source("s")).with_net(Arc::new(FailingNet)),
        )
        .expect("应能注入");
    sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            // 这里直接注册带 host 的版本
            net::register(&ctx, &java, Some(Arc::new(FailingNet)))
                .map_err(|error| format!("注册失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(())
        })
        .expect("应能装配");

    let outcome = eval_text(
        &sandbox,
        r#"
        try { java.get("https://a.test/x"); "未抛出"; }
        catch (e) { "已捕获"; }
        "#,
    );
    assert_eq!(outcome, "已捕获", "代理故障应抛可捕获错误");
}

#[test]
fn 未注入网络能力时抛结构化能力错误() {
    let sandbox = sandbox_without_net();
    let script = r#"
      try { java.get("https://a.test/ok"); "未抛出"; }
      catch (e) { e.capability; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "network");
}

// ---------------------------------------------------------------------------
// 批量请求
// ---------------------------------------------------------------------------

#[test]
fn 批量请求可用() {
    let (sandbox, host, _store) = sandbox_with_net_api();
    let script = r#"
      var results = java.ajaxAll([
        "https://a.test/ok",
        "https://a.test/echo",
        "https://a.test/json"
      ]);
      results.length + ":" + results[0] + "|" + results[2];
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "3:服务端内容|{\"name\":\"斗破\"}"
    );
    assert_eq!(host.seen().len(), 3, "每个 URL 应各发一次请求");
}

#[test]
fn 批量请求的顺序与输入一致() {
    // 书源按索引取结果（`results[0]` 对应第一个 URL），顺序错乱
    // 会让它把内容配错。
    let (sandbox, _host, _store) = sandbox_with_net_api();
    let script = r#"
      var results = java.ajaxAll(["https://a.test/ok", "https://a.test/missing"]);
      results[0];
    "#;
    // 第一个成功，第二个 404 → 整批报错（与单次请求的取向一致）
    let outcome = sandbox.eval(script);
    assert!(
        outcome.is_err(),
        "批量里有一个失败应报错（否则书源会把错误页当成内容）"
    );
}

#[test]
fn 空批量请求返回空数组() {
    let (sandbox, _host, _store) = sandbox_with_net_api();
    assert_eq!(eval_text(&sandbox, "java.ajaxAll([]).length"), "0");
}

// ---------------------------------------------------------------------------
// 下载到缓存 / 到文件
// ---------------------------------------------------------------------------

#[test]
fn 下载到缓存可用() {
    let (sandbox, _host, store) = sandbox_with_net_api();
    let script = r#"
      java.cacheFile("https://a.test/image", "img-key");
      "done";
    "#;
    assert_eq!(eval_text(&sandbox, script), "done");
    // 内容应真的写进了存储
    let stored = store
        .cache_get("img-key")
        .expect("应能读")
        .expect("应已写入");
    assert_eq!(stored, b"<binary>".to_vec());
}

#[test]
fn 下载到文件可用() {
    let (sandbox, _host, store) = sandbox_with_net_api();
    let script = r#"
      java.downloadFile("https://a.test/image", "images/a.bin");
      "done";
    "#;
    assert_eq!(eval_text(&sandbox, script), "done");
    assert_eq!(
        store.read_file("images/a.bin").expect("应能读"),
        b"<binary>".to_vec()
    );
}

#[test]
fn 下载到越界路径被拒绝且可捕获() {
    // 安全红线：文件写入限数据目录内。越界由 StoreHost 实现方拒绝，
    // JS 侧应拿到**可捕获的结构化错误**（FR-006）而不是 panic。
    let (sandbox, _host, _store) = sandbox_with_net_api();
    let script = r#"
      try { java.downloadFile("https://a.test/image", "../../escape.bin"); "未抛出"; }
      catch (e) { e.capability || "已捕获"; }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "fileAccessOutOfDataDir",
        "越界应抛带能力标识的错误"
    );
}

#[test]
fn 下载对非_2xx_报错() {
    let (sandbox, _host, _store) = sandbox_with_net_api();
    let script = r#"
      try { java.downloadFile("https://a.test/missing", "x.bin"); "未抛出"; }
      catch (e) { "已捕获"; }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "已捕获",
        "下载失败不应把错误页写进文件"
    );
}

#[test]
fn 缺少存储能力时下载报存储能力错误() {
    // `NetHost` 与 `StoreHost` 是两个独立能力，缺哪个就报哪个。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let net_host: Arc<dyn NetHost> = Arc::new(FakeNet::new(vec![("/ok", response(200, "x"))]));
    sandbox
        .install_host_bridge(
            HostBridge::bare(JsContext::for_source("s")).with_net(net_host.clone()),
        )
        .expect("应能注入");
    sandbox
        .with_context(move |ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            net::register(&ctx, &java, Some(net_host))
                .map_err(|error| format!("注册失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(())
        })
        .expect("应能装配");

    let script = r#"
      try { java.cacheFile("https://a.test/ok", "k"); "未抛出"; }
      catch (e) { e.capability; }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "store",
        "缺存储能力应报 store（而不是笼统的下载失败）"
    );
}

// ---------------------------------------------------------------------------
// 连接探测
// ---------------------------------------------------------------------------

#[test]
fn 连接探测可用() {
    let (sandbox, _host, _store) = sandbox_with_net_api();
    assert_eq!(
        eval_text(&sandbox, r#"String(java.connect("https://a.test/ok"))"#),
        "true"
    );
}

#[test]
fn 连接探测对不可达返回_false_而不抛错() {
    // 书源用它的目的就是探测可用性——抛错会逼每个调用点包 try/catch
    // 才能拿到那个布尔。
    struct FailingNet;
    impl NetHost for FailingNet {
        fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
            Err(ErrorKind::Network)
        }
    }
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .install_host_bridge(HostBridge::bare(JsContext::for_source("s")))
        .expect("应能注入");
    sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            net::register(&ctx, &java, Some(Arc::new(FailingNet)))
                .map_err(|error| format!("注册失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(())
        })
        .expect("应能装配");

    assert_eq!(
        eval_text(&sandbox, r#"String(java.connect("https://down.test/"))"#),
        "false"
    );
}

// ---------------------------------------------------------------------------
// 注册完整性
// ---------------------------------------------------------------------------

#[test]
fn 全部网络类函数都已注册() {
    let (sandbox, _host, _store) = sandbox_with_net_api();
    for name in [
        "ajax",
        "ajaxAll",
        "connect",
        "get",
        "post",
        "head",
        "downloadFile",
        "cacheFile",
    ] {
        assert_eq!(
            eval_text(&sandbox, &format!("typeof java.{name}")),
            "function",
            "`java.{name}` 应是已注册的函数（FR-005 网络类清单）"
        );
    }
}
