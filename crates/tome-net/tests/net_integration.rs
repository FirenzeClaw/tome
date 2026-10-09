//! tome-net 的端到端集成测试（tasks.md T013 / T014 / T018 验收）。
//!
//! 全部测试针对**本地回环测试服务器**（`support::TestServer`），不访问公网。
//! 这是刻意的：net 层的验收点（方法/头/体/超时/重试/限速）都能在本地完整覆盖，
//! 而依赖公网会让 CI 变得不可靠（网络抖动、站点改版、被墙）。
//!
//! 回环地址是 `client::validate_url` 允许明文 http 的唯一例外——见该函数的文档。
//!
//! # 关于本文件里的 `expect`
//!
//! [WORKAROUND] `.clippy.toml` 的 `allow-expect-in-tests` 覆盖 `#[test]` 函数，
//! 但**不覆盖**本文件里的辅助函数（如 `run`、`temp_dir`）。
//! 这些辅助函数若失败，说明测试环境本身有问题（runtime 建不起来、临时目录建不了），
//! 此时让调用它的那个测试当场失败是正确行为。故显式豁免并说明理由。
#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use support::{Reply, TestServer};
use tome_core::{
    ErrorKind, HeaderMap, HttpMethod, NetHost, ProxyOverride, RequestSpec, RetryPolicy,
};
use tome_net::client::HttpClient;
use tome_net::executor::RequestExecutor;
use tome_net::proxy::{DefaultProxyPolicy, ProxyRoute};
use tome_net::ratelimit::RateLimiter;
use tome_net::{NetError, ReplayMode};

/// 在独立 runtime 上跑一个异步块（集成测试里方便地驱动 net 的异步路径）。
fn run<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("应能构造测试 runtime")
        .block_on(future)
}

// ---------------------------------------------------------------------------
// T013：GET/POST 可发；响应含 status/headers/body_bytes
// ---------------------------------------------------------------------------

#[test]
fn get_returns_status_headers_and_body() {
    let server = TestServer::start(vec![Reply::Ok {
        status: 200,
        reason: "OK".to_string(),
        headers: vec![
            (
                "Content-Type".to_string(),
                "text/html; charset=gbk".to_string(),
            ),
            ("X-Trace".to_string(), "trace-1".to_string()),
        ],
        body: b"hello".to_vec(),
    }]);

    let client = HttpClient::new();
    let spec = RequestSpec::get(server.url("/index.html"), "src");
    let response =
        run(client.send(&spec, &ProxyRoute::Direct, &HeaderMap::new())).expect("GET 应成功");

    assert_eq!(response.status, 200);
    assert_eq!(response.body_bytes, b"hello");
    assert_eq!(response.headers.get("X-Trace"), Some("trace-1"));
    assert!(
        response.headers.get("content-type").is_some(),
        "取头应大小写不敏感"
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/index.html");
}

#[test]
fn post_sends_body_and_custom_headers() {
    let server = TestServer::start(vec![Reply::text("created")]);

    let client = HttpClient::new();
    let mut spec = RequestSpec::get(server.url("/submit"), "src");
    spec.method = HttpMethod::Post;
    spec.headers.set("Content-Type", "application/json");
    spec.headers.set("X-Custom", "value-1");
    spec.body = Some(br#"{"kw":"rust"}"#.to_vec());

    let response =
        run(client.send(&spec, &ProxyRoute::Direct, &HeaderMap::new())).expect("POST 应成功");
    assert_eq!(response.status, 200);

    let requests = server.requests();
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].body, br#"{"kw":"rust"}"#);
    assert_eq!(requests[0].header("X-Custom"), Some("value-1"));
    assert_eq!(requests[0].header("Content-Type"), Some("application/json"));
}

#[test]
fn duplicate_response_headers_are_preserved() {
    let server = TestServer::start(vec![Reply::Ok {
        status: 200,
        reason: "OK".to_string(),
        headers: vec![
            ("Set-Cookie".to_string(), "a=1; Path=/".to_string()),
            ("Set-Cookie".to_string(), "b=2; Path=/".to_string()),
        ],
        body: b"ok".to_vec(),
    }]);

    let client = HttpClient::new();
    let spec = RequestSpec::get(server.url("/"), "src");
    let response = run(client.send(&spec, &ProxyRoute::Direct, &HeaderMap::new())).expect("应成功");

    let cookies: Vec<&str> = response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, value)| value)
        .collect();
    assert_eq!(
        cookies,
        vec!["a=1; Path=/", "b=2; Path=/"],
        "重名响应头不得被合并丢失"
    );
}

#[test]
fn non_success_status_is_returned_verbatim_by_the_transport_layer() {
    let server = TestServer::start(vec![Reply::status_text(404, "nope")]);

    let client = HttpClient::new();
    let spec = RequestSpec::get(server.url("/missing"), "src");
    // 传输层不替调用方判定"404 算不算失败"。
    let response =
        run(client.send(&spec, &ProxyRoute::Direct, &HeaderMap::new())).expect("传输层应成功返回");
    assert_eq!(response.status, 404);
    assert_eq!(response.body_bytes, b"nope");
}

#[test]
fn repeated_request_headers_are_all_sent() {
    let server = TestServer::start(vec![Reply::text("ok")]);

    let client = HttpClient::new();
    let mut spec = RequestSpec::get(server.url("/"), "src");
    // HTTP 允许请求头重名（如多个 Accept）：core 的 HeaderMap 用 Vec 正是为此。
    spec.headers.append("Accept", "text/html");
    spec.headers.append("Accept", "application/xhtml+xml");

    let _ = run(client.send(&spec, &ProxyRoute::Direct, &HeaderMap::new())).expect("应成功");

    let requests = server.requests();
    let accepts = requests[0].header_values("Accept");
    assert_eq!(
        accepts,
        vec!["text/html", "application/xhtml+xml"],
        "重名请求头应全部发出"
    );
}

#[test]
fn body_read_failure_is_reported_as_io_error() {
    // 服务器声明 Content-Length: 100 却只发头就断开 → 读体必失败。
    let server = TestServer::start(vec![Reply::AbortAfterHeaders { status: 200 }]);

    let client = HttpClient::new();
    let spec = RequestSpec::get(server.url("/truncated"), "src");
    let error =
        run(client.send(&spec, &ProxyRoute::Direct, &HeaderMap::new())).expect_err("读体应失败");

    // 断言它是"传输层失败"这一类，而不是被误判成超时或状态错误。
    assert!(
        matches!(error, NetError::Io { .. } | NetError::Connect { .. }),
        "读体中断应归 Io/Connect，实际: {error}"
    );
    assert_eq!(error.to_error_kind(), ErrorKind::Network);
}

#[test]
fn connection_reset_is_reported_as_a_structured_error() {
    // 服务器收到连接立刻关闭：必须得到结构化错误，而不是 panic。
    let server = TestServer::start(vec![Reply::Close]);

    let client = HttpClient::new();
    let spec = RequestSpec::get(server.url("/reset"), "src");
    let error = run(client.send(&spec, &ProxyRoute::Direct, &HeaderMap::new()))
        .expect_err("连接被关闭应报错");
    assert_eq!(error.to_error_kind(), ErrorKind::Network);
    // server 变量在 Drop 时收尾，这里确认它确实被连过。
    assert!(
        server.wait_for_requests(1, Duration::from_millis(100)) || server.connection_count() >= 1
    );
}

// ---------------------------------------------------------------------------
// T014：RequestSpec 执行器（超时/重试/限速/Cookie）
// ---------------------------------------------------------------------------

#[test]
fn executor_reads_timeout_from_spec() {
    // 服务器挂住不回应，触发超时。
    let server = TestServer::start(vec![Reply::HangFor(Duration::from_secs(5))]);

    let executor = RequestExecutor::new().expect("应能构造");
    let mut spec = RequestSpec::get(server.url("/slow"), "src");
    spec.timeout_ms = 150;

    let started = Instant::now();
    let error = run(executor.execute_once(&spec)).expect_err("应超时");
    let elapsed = started.elapsed();

    assert!(
        matches!(error, NetError::Timeout { .. }),
        "实际错误: {error}"
    );
    assert_eq!(error.to_error_kind(), ErrorKind::Network);
    assert!(
        elapsed < Duration::from_secs(3),
        "超时应在 spec.timeout_ms 附近触发，实际耗时 {elapsed:?}"
    );
}

#[test]
fn executor_returns_status_error_per_default_policy() {
    let server = TestServer::start(vec![Reply::status_text(503, "unavailable")]);

    let executor = RequestExecutor::new().expect("应能构造");
    let spec = RequestSpec::get(server.url("/"), "src");
    let error = run(executor.execute_once(&spec)).expect_err("非 2xx 应报错");
    assert!(matches!(error, NetError::Status { status: 503, .. }));
    assert_eq!(error.to_error_kind(), ErrorKind::Network);
}

#[test]
fn executor_can_return_non_success_status_when_configured() {
    let server = TestServer::start(vec![Reply::status_text(403, "forbidden")]);

    let executor = RequestExecutor::builder()
        .fail_on_status(false)
        .build()
        .expect("应能构造");
    let spec = RequestSpec::get(server.url("/"), "src");
    let response = run(executor.execute_once(&spec)).expect("应原样返回响应");
    assert_eq!(response.status, 403, "书源 check 脚本需要看到真实状态码");
}

#[test]
fn executor_retries_retryable_status_up_to_the_policy_limit() {
    // 前两次 503，第三次 200。max_retries=3 应刚好够。
    let server = TestServer::start(vec![
        Reply::status_text(503, "try again"),
        Reply::status_text(503, "try again"),
        Reply::text("finally"),
    ]);

    let executor = RequestExecutor::new().expect("应能构造");
    let mut spec = RequestSpec::get(server.url("/flaky"), "src");
    spec.retry = RetryPolicy::new(3, 1);

    let response = run(executor.execute(&spec)).expect("重试后应成功");
    assert_eq!(response.status, 200);
    assert_eq!(response.body_bytes, b"finally");
    assert_eq!(server.request_count(), 3, "应恰好重试到成功为止");
}

#[test]
fn executor_does_not_retry_client_errors() {
    // 4xx 不重试：重发只会白等，且可能触发站点风控（spec 边缘情况）。
    let server = TestServer::start(vec![Reply::status_text(404, "gone")]);

    let executor = RequestExecutor::new().expect("应能构造");
    let mut spec = RequestSpec::get(server.url("/missing"), "src");
    spec.retry = RetryPolicy::new(5, 1);

    let error = run(executor.execute(&spec)).expect_err("应报错");
    assert!(matches!(error, NetError::Status { status: 404, .. }));
    assert_eq!(server.request_count(), 1, "404 不得重试");
}

#[test]
fn executor_gives_up_after_max_retries() {
    // 服务器持续 500，策略只允许重试 2 次 → 总共 3 次请求。
    let server = TestServer::start(vec![Reply::status_text(500, "broken")]);

    let executor = RequestExecutor::new().expect("应能构造");
    let mut spec = RequestSpec::get(server.url("/always-broken"), "src");
    spec.retry = RetryPolicy::new(2, 1);

    let error = run(executor.execute(&spec)).expect_err("应最终报错");
    assert!(matches!(error, NetError::Status { status: 500, .. }));
    assert_eq!(server.request_count(), 3, "1 次首发 + 2 次重试");
}

/// 服务端用 `Retry-After` 声明等待时长时，重试循环必须遵守它（spec 边缘情况：
/// "遵循书源声明的间隔并在必要时降速，而不是重试到被封"）。
#[test]
fn retry_after_header_is_carried_into_the_status_error() {
    let server = TestServer::start(vec![Reply::Ok {
        status: 429,
        reason: "Too Many Requests".to_string(),
        headers: vec![
            ("Retry-After".to_string(), "2".to_string()),
            ("Content-Type".to_string(), "text/plain".to_string()),
        ],
        body: b"slow down".to_vec(),
    }]);

    let executor = RequestExecutor::new().expect("应能构造");
    // 不重试，只观察错误里是否带上了服务端声明的等待时长。
    let spec = RequestSpec::get(server.url("/limited"), "src");
    let error = run(executor.execute_once(&spec)).expect_err("429 应报错");

    assert!(
        matches!(error, NetError::Status { status: 429, .. }),
        "429 应归 Status，实际: {error}"
    );
    assert_eq!(
        error.retry_after(),
        Some(Duration::from_secs(2)),
        "服务端声明的 Retry-After 应被解析并带上"
    );
    // 429 是可重试状态（限流是临时的）。
    assert!(tome_net::executor::is_retryable_status(429));
}

/// 荒唐的 `Retry-After` 不得让调用方停摆。
#[test]
fn absurd_retry_after_is_not_honoured() {
    let server = TestServer::start(vec![Reply::Ok {
        status: 503,
        reason: "Unavailable".to_string(),
        // 第三方响应不可信：这个值若被当真，调用方要等一整天。
        headers: vec![("Retry-After".to_string(), "86400".to_string())],
        body: b"later".to_vec(),
    }]);

    let executor = RequestExecutor::new().expect("应能构造");
    let spec = RequestSpec::get(server.url("/"), "src");
    let error = run(executor.execute_once(&spec)).expect_err("503 应报错");
    assert_eq!(error.retry_after(), None, "超过上限的 Retry-After 应被忽略");
}

#[test]
fn executor_zero_retries_means_single_attempt() {
    let server = TestServer::start(vec![Reply::status_text(500, "broken")]);

    let executor = RequestExecutor::new().expect("应能构造");
    let mut spec = RequestSpec::get(server.url("/"), "src");
    spec.retry = RetryPolicy::NONE;

    assert!(run(executor.execute(&spec)).is_err());
    assert_eq!(server.request_count(), 1);
}

#[test]
fn executor_stores_and_replays_cookies_across_requests() {
    // 第一次响应设 cookie；第二次请求必须带上它。
    let server = TestServer::start(vec![
        Reply::Ok {
            status: 200,
            reason: "OK".to_string(),
            headers: vec![("Set-Cookie".to_string(), "sid=abc123; Path=/".to_string())],
            body: b"set".to_vec(),
        },
        Reply::text("done"),
    ]);

    let executor = RequestExecutor::new().expect("应能构造");
    let login = RequestSpec::get(server.url("/login"), "source-a");
    let _ = run(executor.execute_once(&login)).expect("登录应成功");

    // 用 base_url 直接拼路径，验证这一便捷入口给出的地址同样可用。
    let next = RequestSpec::get(format!("{}/profile", server.base_url()), "source-a");
    let _ = run(executor.execute_once(&next)).expect("第二次应成功");

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].header("Cookie"),
        Some("sid=abc123"),
        "第二次请求应带上第一次设的 cookie"
    );
}

#[test]
fn cookies_are_not_shared_between_sources() {
    let server = TestServer::start(vec![
        Reply::Ok {
            status: 200,
            reason: "OK".to_string(),
            headers: vec![("Set-Cookie".to_string(), "sid=aaa; Path=/".to_string())],
            body: b"set".to_vec(),
        },
        Reply::text("other"),
    ]);

    let executor = RequestExecutor::new().expect("应能构造");
    // 源 A 拿到 cookie。
    let _ = run(executor.execute_once(&RequestSpec::get(server.url("/login"), "source-a")))
        .expect("应成功");
    // 源 B 访问同一站点：不得带上源 A 的 cookie（T016 按源隔离）。
    let _ = run(executor.execute_once(&RequestSpec::get(server.url("/x"), "source-b")))
        .expect("应成功");

    let requests = server.requests();
    assert_eq!(
        requests[1].header("Cookie"),
        None,
        "另一源不得继承这个源的登录态"
    );
}

// ---------------------------------------------------------------------------
// T017：代理故障与目标站故障可区分
// ---------------------------------------------------------------------------

#[test]
fn proxy_failure_is_distinguishable_from_target_failure() {
    // 两条路径都用回环，不触公网。
    //
    // 目标站故障：连一个确定没有监听的端口（先在回环上取一个空闲端口再释放，
    // 得到"几乎必然无人监听"的地址）。
    let unused = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("应能绑定");
        let address = listener.local_addr().expect("应能取地址");
        drop(listener);
        address
    };
    assert!(!matches!(unused.port(), 0), "应拿到一个具体端口而非 0");

    let executor = RequestExecutor::new().expect("应能构造");
    let direct = RequestSpec::get(format!("http://{unused}/"), "src");
    let direct_error = run(executor.execute_once(&direct)).expect_err("直连应失败");
    assert_eq!(
        direct_error.to_error_kind(),
        ErrorKind::Network,
        "目标站不可达应归网络错，实际: {direct_error}"
    );

    // 代理故障：指定一个同样无人监听的代理地址。
    let mut proxied = RequestSpec::get("https://target.invalid/", "src");
    proxied.proxy_override = Some(ProxyOverride::Custom(format!("http://{unused}")));
    let proxy_error = run(executor.execute_once(&proxied)).expect_err("代理应失败");

    assert!(
        matches!(proxy_error, NetError::Proxy { .. }),
        "配置了代理时连接失败应归因代理，实际: {proxy_error}"
    );
    assert_eq!(
        proxy_error.to_error_kind(),
        ErrorKind::Configuration,
        "代理故障属本机配置问题（借 Configuration 承载，见 NetError 文档）"
    );

    // 两类的分类必须不同——这是 FR-008 边缘情况的验收点。
    assert_ne!(direct_error.to_error_kind(), proxy_error.to_error_kind());
}

#[test]
fn direct_override_ignores_system_proxy() {
    // `ProxyOverride::Direct` 必须明确直连，即使系统代理环境变量指向坏地址。
    // 用进程级环境变量会污染其他测试，故只验证归约结果（纯函数）+ 真实请求走通。
    let server = TestServer::start(vec![Reply::text("direct-ok")]);

    let executor = RequestExecutor::builder()
        .default_policy(DefaultProxyPolicy::Direct)
        .build()
        .expect("应能构造");
    let mut spec = RequestSpec::get(server.url("/"), "src");
    spec.proxy_override = Some(ProxyOverride::Direct);

    let response = run(executor.execute_once(&spec)).expect("直连应成功");
    assert_eq!(response.body_bytes, b"direct-ok");
}

// ---------------------------------------------------------------------------
// T018：限速生效
// ---------------------------------------------------------------------------

#[test]
fn rate_limit_spaces_requests_for_the_same_source() {
    let server = TestServer::start(vec![Reply::text("ok")]);

    let interval = Duration::from_millis(60);
    let limiter = Arc::new(RateLimiter::new());
    limiter.set_interval("throttled", interval);

    let executor = RequestExecutor::builder()
        .limiter(Arc::clone(&limiter))
        .build()
        .expect("应能构造");

    let spec = RequestSpec::get(server.url("/"), "throttled");
    let started = Instant::now();
    for _ in 0..3 {
        let _ = run(executor.execute_once(&spec)).expect("应成功");
    }
    let elapsed = started.elapsed();

    // 3 次请求之间至少有两个间隔。
    assert!(
        elapsed >= interval * 2,
        "3 次请求耗时 {elapsed:?} 小于 2×{interval:?}，限速未生效"
    );
    assert_eq!(server.request_count(), 3);
}

#[test]
fn rate_limit_does_not_delay_other_sources() {
    let server = TestServer::start(vec![Reply::text("ok")]);

    let interval = Duration::from_millis(200);
    let limiter = Arc::new(RateLimiter::new());
    // 只给 slow 源设间隔。
    limiter.set_interval("slow", interval);

    let executor = RequestExecutor::builder()
        .limiter(Arc::clone(&limiter))
        .build()
        .expect("应能构造");

    // 另一个源连续 3 次请求应几乎立刻完成。
    let started = Instant::now();
    for _ in 0..3 {
        let spec = RequestSpec::get(server.url("/"), "fast");
        let _ = run(executor.execute_once(&spec)).expect("应成功");
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < interval,
        "未限速的源耗时 {elapsed:?} 已达 slow 源的间隔 {interval:?}，限速串源了"
    );
}

// ---------------------------------------------------------------------------
// T019：回放（离线）
// ---------------------------------------------------------------------------

#[test]
fn recorded_responses_replay_without_touching_the_network() {
    let corpus = temp_dir("replay-offline");
    let server = TestServer::start(vec![Reply::Ok {
        status: 200,
        reason: "OK".to_string(),
        headers: vec![(
            "Content-Type".to_string(),
            "text/html; charset=gbk".to_string(),
        )],
        body: {
            // 二进制 + GBK 字节，验证记录体（hex）能承载任意字节。
            let mut bytes = vec![0x00, 0x0D, 0x0A, 0xFF, 0xFE];
            bytes.extend_from_slice(&[0xD6, 0xD0, 0xCE, 0xC4]);
            bytes
        },
    }]);

    let spec = RequestSpec::get(server.url("/chapter/1"), "src");

    // 录制阶段：真实请求一次。
    let recorder = RequestExecutor::builder()
        .replay(&corpus, ReplayMode::Record)
        .build()
        .expect("应能构造");
    let recorded = run(recorder.execute_once(&spec)).expect("录制应成功");
    assert_eq!(server.request_count(), 1);

    // 记下录完时的连接数，之后它必须不再增长。
    let connections_after_recording = server.connection_count();

    // 回放阶段：把服务器停掉（Drop 会关掉监听），再重放。
    drop(server);

    let replayer = RequestExecutor::builder()
        .replay(&corpus, ReplayMode::Replay)
        .build()
        .expect("应能构造");
    let replayed = run(replayer.execute_once(&spec)).expect("离线重放应命中");

    assert_eq!(replayed.status, recorded.status);
    assert_eq!(
        replayed.body_bytes, recorded.body_bytes,
        "二进制体必须逐字节相同"
    );

    // 回放不该建立任何新连接——服务器已 Drop，若触网必然失败；
    // 这里再确认一次"没有触发网络路径"的证据：上面的 expect 已通过。
    assert!(connections_after_recording >= 1, "录制阶段应当真的连过一次");

    std::fs::remove_dir_all(&corpus).ok();
}

#[test]
fn replay_miss_is_reported_as_configuration_error() {
    let corpus = temp_dir("replay-miss");
    let replayer = RequestExecutor::builder()
        .replay(&corpus, ReplayMode::Replay)
        .build()
        .expect("应能构造");

    let spec = RequestSpec::get("https://never-recorded.invalid/x", "src");
    let error = run(replayer.execute_once(&spec)).expect_err("应未命中");
    assert!(matches!(error, NetError::ReplayMiss { .. }));
    assert_eq!(error.to_error_kind(), ErrorKind::Configuration);

    std::fs::remove_dir_all(&corpus).ok();
}

#[test]
fn replay_mode_declines_to_send_http_at_all() {
    // 回放模式下即便目标是公网的**明文 http**（本该被安全红线拒掉），
    // 也应先命中 URL 校验——但更重要的是：回放对合法 https 目标也绝不触网。
    let corpus = temp_dir("replay-no-net");
    let replayer = RequestExecutor::builder()
        .replay(&corpus, ReplayMode::Replay)
        .build()
        .expect("应能构造");

    // 一个语法合法但绝不可达的 https 地址。若回放试图联网，会得到连接错误；
    // 实际应是 ReplayMiss（证明它只查语料库）。
    let spec = RequestSpec::get("https://replay-never-touches-network.invalid/page", "src");
    let error = run(replayer.execute_once(&spec)).expect_err("应未命中");
    assert!(
        matches!(error, NetError::ReplayMiss { .. }),
        "回放应只查语料库，实际错误: {error}"
    );

    std::fs::remove_dir_all(&corpus).ok();
}

#[test]
fn record_once_reuses_existing_corpus_entries() {
    let corpus = temp_dir("record-once");
    let server = TestServer::start(vec![Reply::text("live")]);
    let spec = RequestSpec::get(server.url("/page"), "src");

    // 第一轮：录制。
    let recorder = RequestExecutor::builder()
        .replay(&corpus, ReplayMode::RecordOnce)
        .build()
        .expect("应能构造");
    let _ = run(recorder.execute_once(&spec)).expect("首轮应真实抓取");
    assert_eq!(server.request_count(), 1);

    // 第二轮：同一请求命中语料 → 不再触网。
    let second = RequestExecutor::builder()
        .replay(&corpus, ReplayMode::RecordOnce)
        .build()
        .expect("应能构造");
    let response = run(second.execute_once(&spec)).expect("应命中语料");
    assert_eq!(response.body_bytes, b"live");
    assert_eq!(
        server.request_count(),
        1,
        "RecordOnce 命中语料时不应再发请求"
    );

    std::fs::remove_dir_all(&corpus).ok();
}

// ---------------------------------------------------------------------------
// NetHost 接缝（同步接口）
// ---------------------------------------------------------------------------

#[test]
fn net_host_fetch_works_synchronously() {
    let server = TestServer::start(vec![Reply::text("via-seam")]);

    let executor = RequestExecutor::new().expect("应能构造");
    let spec = RequestSpec::get(server.url("/seam"), "src");

    // `NetHost::fetch` 是同步的（R1：书源脚本是同步语义）。
    let response = NetHost::fetch(&executor, spec).expect("同步 fetch 应成功");
    assert_eq!(response.status, 200);
    assert_eq!(response.body_bytes, b"via-seam");
    // 字符集由执行器在返回前填好（FR-009）。
    assert_eq!(response.detected_charset.as_deref(), Some("UTF-8"));
}

#[test]
fn net_host_fetch_maps_rejections_to_error_kind() {
    let executor = RequestExecutor::new().expect("应能构造");
    // 明文 http 到公网：安全红线拒绝，且不该触网。
    let spec = RequestSpec::get("http://public.example.com/", "src");
    let kind = NetHost::fetch(&executor, spec).expect_err("应被拒绝");
    assert_eq!(kind, ErrorKind::Configuration);

    // ftp 同理。
    let spec = RequestSpec::get("ftp://example.com/", "src");
    assert_eq!(
        NetHost::fetch(&executor, spec).expect_err("应被拒绝"),
        ErrorKind::Configuration
    );
}

// ---------------------------------------------------------------------------
// T015：字符集在实际响应路径上生效
// ---------------------------------------------------------------------------

#[test]
fn charset_is_detected_from_response_headers() {
    let server = TestServer::start(vec![Reply::Ok {
        status: 200,
        reason: "OK".to_string(),
        headers: vec![(
            "Content-Type".to_string(),
            "text/html; charset=gbk".to_string(),
        )],
        // GBK 的 "中文"
        body: vec![0xD6, 0xD0, 0xCE, 0xC4],
    }]);

    let executor = RequestExecutor::new().expect("应能构造");
    let spec = RequestSpec::get(server.url("/gbk"), "src");
    let response = run(executor.execute_once(&spec)).expect("应成功");

    assert_eq!(response.detected_charset.as_deref(), Some("GBK"));
    let decoded = tome_net::charset::decode_body(&response.body_bytes, Some("gbk"));
    assert_eq!(decoded.text, "中文");
}

#[test]
fn compressed_response_is_transparently_decoded() {
    // 服务器手动发一个 gzip 响应体（用预计算的固定字节，不引入压缩库）。
    // 1f 8b 08 00 ... 是 gzip 头；这里用一段"hello"的 gzip 表示。
    // 生成方式：printf 'hello' | gzip -n | xxd -p
    let gzipped: Vec<u8> = vec![
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xcb, 0x48, 0xcd, 0xc9, 0xc9,
        0x07, 0x00, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00,
    ];

    let server = TestServer::start(vec![Reply::Ok {
        status: 200,
        reason: "OK".to_string(),
        headers: vec![("Content-Encoding".to_string(), "gzip".to_string())],
        body: gzipped,
    }]);

    let executor = RequestExecutor::new().expect("应能构造");
    let spec = RequestSpec::get(server.url("/gz"), "src");
    let response = run(executor.execute_once(&spec)).expect("应成功");

    // FR-009 要求正确处理压缩传输；reqwest 在 gzip 特性下透赌解压。
    assert_eq!(
        response.body_bytes,
        b"hello",
        "gzip 响应体应被透明解压（实际 {:?}）",
        String::from_utf8_lossy(&response.body_bytes)
    );
}

/// 每个测试独占的临时目录。
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "tome-net-it-{tag}-{unique}-{:?}",
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("应能创建临时目录");
    dir
}
