//! T032 验收测试：JS 面向的网络调用经 `NetHost` → `RequestSpec` → 真实 HTTP
//!
//! ## 这条测试在证什么（tasks.md T032 的验收项原文）
//!
//! > JS 的 `ajax`/`get`/`post` 经 NetHost → RequestSpec → 真实网络
//!
//! 上述链条有四段，缺任何一段都不算通过：
//!
//! 1. 脚本侧的调用形态（[`HostRequest`] 的 `get`/`post`/`head`）
//! 2. 被补全成完整的 `RequestSpec`（带限速键/超时/字符集）
//! 3. 经 `NetHost` 接缝交给真实执行器（[`RequestExecutor`]）
//! 4. **真的发出 HTTP**（打到本地回环测试服务器并拿回响应）
//!
//! 前三段在 `host.rs` 的单元测试里用替身覆盖了（快、不触网）。这一段专门
//! 补上第 4 段：**用真实的 `RequestExecutor` + 真实的 HTTP 服务器**。
//! 只用替身测会让"接线正确但执行器根本没被调用"这种缺陷完全不可见。
//!
//! ## 为什么用本地回环服务器而不是公网
//!
//! 回环是 `tome_net::client::validate_url` 允许明文 http 的唯一例外
//! （见该函数的文档）。用公网会让 CI 依赖外部站点的可用性，而本测试要验的
//! "请求真的发出去了"在本地能完整覆盖。
//!
//! # 关于本文件里的 `expect`
#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

mod support;

use std::sync::Arc;

use support::{Reply, TestServer};
use tome_core::{ErrorKind, NetHost, RequestSpec};
use tome_net::executor::RequestExecutor;
use tome_net::host::HostRequest;

/// 构造"接在真实执行器上"的脚本请求入口。
///
/// 返回执行器的 `Arc` 以便调用方共享所有权（`HostRequest` 取 `Arc<dyn NetHost>`，
/// 而 `Arc<RequestExecutor>` 可以强转成它）。
fn script_request(source_id: &str) -> (HostRequest, Arc<RequestExecutor>) {
    let executor = Arc::new(RequestExecutor::new().expect("应能构造执行器"));
    let host: Arc<dyn NetHost> = executor.clone();
    (HostRequest::for_source(host, source_id), executor)
}

// ---------------------------------------------------------------------------
// 真实 HTTP：get / post / head
// ---------------------------------------------------------------------------

#[test]
fn get_真的发出请求并拿回正文() {
    let server = TestServer::start(vec![Reply::text("服务端内容")]);
    let (request, _executor) = script_request("https://source.test");

    let text = request
        .send_text(request.get(server.url("/api")))
        .expect("GET 应成功");
    assert_eq!(text, "服务端内容", "应拿回真实服务器返回的正文");
}

#[test]
fn post_真的把请求体发出去() {
    // 用服务器侧记录到的请求反查：断言**请求体真的到了对端**，
    // 而不只是"响应能拿回来"（后者在把 body 丢掉的实现下也会通过）。
    let server = TestServer::start(vec![Reply::text("已收到")]);
    let (request, _executor) = script_request("https://source.test");

    let text = request
        .send_text(request.post(server.url("/submit"), b"q=1".to_vec()))
        .expect("POST 应成功");
    assert_eq!(text, "已收到");

    assert!(
        server.wait_for_requests(1, std::time::Duration::from_secs(2)),
        "服务器应收到一个请求"
    );
    let recorded = server.requests();
    assert_eq!(recorded.len(), 1, "应恰好一个请求");
    assert_eq!(recorded[0].method, "POST", "方法必须是 POST");
    assert_eq!(
        recorded[0].body.as_slice(),
        b"q=1".as_slice(),
        "请求体必须真的发到了对端"
    );
    assert_eq!(recorded[0].target, "/submit");
}

#[test]
fn get_的请求行与头对端可见() {
    let server = TestServer::start(vec![Reply::text("ok")]);
    let (request, _executor) = script_request("https://source.test");

    let response = request
        .send(request.get(server.url("/api/list?page=2")))
        .expect("GET 应成功");
    assert_eq!(response.status, 200);

    assert!(server.wait_for_requests(1, std::time::Duration::from_secs(2)));
    let recorded = server.requests();
    assert_eq!(recorded[0].method, "GET");
    assert_eq!(
        recorded[0].target, "/api/list?page=2",
        "查询串必须原样发出去"
    );
    // reqwest 默认会带 Host（HTTP/1.1 必需），验证头确实到达对端。
    assert!(
        recorded[0].header("host").is_some(),
        "Host 头应到达对端，实际头：{:?}",
        recorded[0].headers
    );
}

#[test]
fn head_请求可发出() {
    // HEAD 的响应没有正文，但请求本身必须能走完（不少书源用 HEAD 探活）。
    let server = TestServer::start(vec![Reply::text("")]);
    let (request, _executor) = script_request("https://source.test");

    let response = request
        .send(request.head(server.url("/probe")))
        .expect("HEAD 应成功");
    assert_eq!(response.status, 200);

    // 方法与路径都要对端可见——HEAD 被悄悄降级成 GET 是很容易发生的缺陷，
    // 它会让"探活"变成一次真实的内容下载（多耗一倍流量）。
    assert!(server.wait_for_requests(1, std::time::Duration::from_secs(2)));
    let recorded = server.requests();
    assert_eq!(recorded[0].method, "HEAD", "方法必须原样是 HEAD");
    assert_eq!(recorded[0].target, "/probe");
    // `base_url` 与 `url` 应指向同一个服务器（拼接方式不同而已）
    assert!(
        server.base_url().starts_with("http://127.0.0.1:"),
        "base_url 应是回环地址：{}",
        server.base_url()
    );
    assert!(
        server.url("/x").starts_with(&server.base_url()),
        "url() 应以 base_url 为前缀"
    );
}

#[test]
fn 非_2xx_经真实服务器时行为分岔正确() {
    let server = TestServer::start(vec![
        Reply::status_text(404, "not found"),
        Reply::status_text(403, "需要人工验证"),
    ]);
    let (request, _executor) = script_request("https://source.test");

    // `send`：拿到状态码与正文（书源的可用性检查脚本靠它读 403 页）
    let response = request
        .send(request.get(server.url("/missing")))
        .expect("send 对 404 应返回 Ok");
    assert_eq!(response.status, 404);
    assert_eq!(
        String::from_utf8_lossy(&response.body_bytes),
        "not found",
        "错误页正文必须带回来"
    );

    // `send_text`：报错（`java.get` 要的是正文，不是错误页）
    let error = request
        .send_text(request.get(server.url("/forbidden")))
        .expect_err("send_text 对 403 应报错");
    assert_eq!(error, ErrorKind::Network);
}

#[test]
fn 多个请求打到同一服务器() {
    // 书源脚本一次抓取常常连打十几个请求（列表页 + 详情页 + 目录页）。
    // 这条验证接线层可被反复调用，不是一次性的。
    let server = TestServer::start(vec![
        Reply::text("第一"),
        Reply::text("第二"),
        Reply::text("第三"),
    ]);
    let (request, _executor) = script_request("https://source.test");

    for expected in ["第一", "第二", "第三"] {
        let text = request
            .send_text(request.get(server.url("/page")))
            .unwrap_or_else(|error| panic!("第 {expected} 次请求失败：{error:?}"));
        assert_eq!(text, expected);
    }
}

// ---------------------------------------------------------------------------
// 限速键：脚本请求与管线请求共享同一个令牌桶（FR-008）
// ---------------------------------------------------------------------------

#[test]
fn 脚本请求与管线请求共享限速键() {
    // ## 这是 FR-008「按源」限速能否成立的关键
    //
    // 若脚本请求用 URL 当限速键、管线请求用书源 id，同一个源就会被限成
    // 两份速率，实际频率翻倍——而"翻倍"正是最容易触发对方风控的量。
    //
    // 断言方式：构造出来的 `RequestSpec` 的限速键必须是**书源 id**，
    // 与管线（`tome_source::search::parse_search_spec`）的取值一致。
    let (request, _executor) = script_request("https://source.test");
    let spec: RequestSpec = request.get("https://target.test/whatever");
    assert_eq!(
        spec.rate_limit_key, "https://source.test",
        "限速键必须是书源 id（不是请求 URL），否则同源会被限成两份"
    );
}

#[test]
fn 书源声明的间隔一路传到请求规格() {
    // R5 的链条在脚本路径上同样成立：`concurrentRate` → `rate_limit_ms`
    // → `RequestSpec.request_interval_ms` → 执行器的限速器。
    let executor = Arc::new(RequestExecutor::new().expect("应能构造执行器"));
    let host: Arc<dyn NetHost> = executor;
    let request = HostRequest::for_source(host, "https://source.test")
        .with_charset(Some("gbk".to_string()))
        .with_request_interval(Some(1000));

    let spec = request.get("https://target.test/a");
    assert_eq!(spec.request_interval_ms, Some(1000));
    assert_eq!(spec.charset.as_deref(), Some("gbk"));
}

// ---------------------------------------------------------------------------
// 传输失败：接缝如实报出，脚本侧可捕获
// ---------------------------------------------------------------------------

#[test]
fn 目标不可达时报网络错() {
    // `Reply::Close`：服务器收到连接立刻关闭——目标站不可达的一种形态。
    let server = TestServer::start(vec![Reply::Close]);
    let (request, _executor) = script_request("https://source.test");

    let error = request
        .send(request.get(server.url("/gone")))
        .expect_err("目标不可达应报错");
    // 具体分类可能是 Network（连接失败）或 Proxy（若走了系统代理时的归因），
    // 两者的共同点是**都是传输层失败**且**都是结构化错误**（不是 panic）。
    assert!(
        matches!(error, ErrorKind::Network | ErrorKind::Proxy),
        "应是传输层错误，实际 {error:?}"
    );
    // 不重试（脚本请求的取向）：连接数不该因重试而增长。
    // 给它一点时间让连接真的建立过。
    assert!(
        server.wait_for_requests(1, std::time::Duration::from_secs(2)),
        "至少应有过一次连接尝试"
    );
}

#[test]
fn 非法_url_在执行器侧被拒绝而不是发出() {
    // 安全红线（宪法原则 V / AGENTS.md）：外部通信一律 https。
    // 明文 http 到公网地址必须在**发出去之前**被拒绝。
    let (request, _executor) = script_request("https://source.test");
    let error = request
        .send(request.get("http://public.example.com/"))
        .expect_err("明文 http 到公网应被拒绝");
    assert_eq!(
        error,
        ErrorKind::Configuration,
        "scheme 不合法属配置问题（不是网络问题）"
    );
}
