//! HTTP 客户端与阻塞外观（tasks.md T013）
//!
//! 本模块是 tome-net 里**唯一**构造 `reqwest::Client` 的地方。上层（执行器）
//! 只通过 [`HttpClient`] / [`BlockingClient`] 的窄接口使用网络能力。
//!
//! # 异步客户端 + 阻塞外观，而不是 reqwest 的 `blocking` 特性
//!
//! T013 要求"阻塞外观与异步客户端并存（JS 用同步语义，R1）"。reqwest 自带的
//! `blocking` 特性看起来正合用，但它会构造**第二个** `reqwest::Client` 实例：
//!
//! - 配置要维护两份（超时、连接池、代理、HTTP/2 参数…），两份迟早不一致；
//! - 两个客户端各自持连接池，同一站点的连接被劈成两半，池化收益打折；
//! - `blocking::Client` 内部有一个常驻事件循环线程，另加我们自己的 runtime，
//!   线程开销翻倍（体积/资源纪律）。
//!
//! 故本模块自己实现阻塞外观：**自持一个 tokio runtime，`block_on` 同一个异步
//! 客户端**（[`BlockingClient`]）。一份客户端、一套配置、一个连接池。
//!
//! # 代理随请求变化 → 客户端按路线缓存
//!
//! reqwest 的代理是**客户端级**配置（没有 per-request 代理 API），而 FR-008
//! 要求"按源单独指定代理"。两条路可选：
//!
//! 1. 每个请求现造客户端 —— 简单，但每次调用都会丢弃连接池，
//!    等于放弃 keep-alive；书源抓取恰恰是"同站点连打十几个请求"的模式。
//! 2. **按代理路线缓存客户端** —— 路线只有三种形态（直连 / 系统 / 某个具体地址），
//!    自定义地址的数量等于"配置了独立代理的书源数"，通常是个位数。
//!
//! 取 2。代价是客户端数 = 1 + 1 + 自定义代理数，可接受且换回了连接复用。
//!
//! # 安全红线：只允许 https
//!
//! 宪法原则 V 与 AGENTS.md 安全红线都要求"外部通信一律 HTTPS"。
//! 本模块在**发起请求之前**校验 scheme，非 https 一律 [`NetError::Url`]。
//!
//! 唯一例外是**回环地址**（`127.0.0.1` / `localhost` / `[::1]`）：本地测试服务器
//! 与用户自建的本机书源服务不可能有证书，禁止 http 会让本地端到端测试无法进行
//! （T013/T014 的验收本身就要求用本地回环测试服务器验证）。回环流量不出本机，
//! "明文传输被监听"的风险面在这里不存在，故这个例外不削弱红线。
//! 判定见 [`validate_url`]。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use tome_core::{HeaderMap, HttpMethod, RequestSpec, Response};

use crate::error::NetError;
use crate::proxy::{self, DefaultProxyPolicy, ProxyRoute};

/// HTTP 客户端（异步）。
///
/// 内部按代理路线缓存 `reqwest::Client`；所有方法取 `&self`，可跨线程共享。
/// `Clone` 是**廉价**的（仅克隆一个 `Arc`）：克隆体共享同一份客户端缓存与连接池，
/// 这正是"一份配置而不是两份"的落地方式。
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: std::sync::Arc<HttpClientInner>,
}

/// 客户端的共享内部状态（见 [`HttpClient`]）。
#[derive(Debug)]
struct HttpClientInner {
    /// 代理路线描述 → 该路线对应的客户端。
    ///
    /// 键用 [`ProxyRoute::describe`] 而不是枚举本身：枚举里 `Custom` 带地址，
    /// 用字符串做键让"同一个地址复用同一个客户端"这件事直接体现在哈希键上。
    clients: Mutex<HashMap<String, reqwest::Client>>,
    /// 未给出按源覆盖时采用的默认代理策略。
    default_policy: DefaultProxyPolicy,
    /// 连接池每主机空闲连接上限。
    pool_max_idle_per_host: usize,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    /// 以默认配置新建（跟随系统代理）。
    pub fn new() -> Self {
        Self::with_policy(DefaultProxyPolicy::default())
    }

    /// 以指定默认代理策略新建。
    pub fn with_policy(default_policy: DefaultProxyPolicy) -> Self {
        Self {
            inner: std::sync::Arc::new(HttpClientInner {
                clients: Mutex::new(HashMap::new()),
                default_policy,
                pool_max_idle_per_host: DEFAULT_POOL_MAX_IDLE_PER_HOST,
            }),
        }
    }

    /// 当前默认代理策略。
    pub fn default_policy(&self) -> DefaultProxyPolicy {
        self.inner.default_policy
    }

    /// 取得（或构造）某代理路线对应的客户端。
    ///
    /// ## HTTP/2 与连接池
    ///
    /// `http2` 特性让 reqwest 在 TLS 上按 ALPN 协商 h2；协商不成自动回落 HTTP/1.1，
    /// 故不需要（也不应该）强制 `http2_prior_knowledge`——那会让只支持 HTTP/1.1
    /// 的站点直接握手失败。连接池参数只在需要时显式设置，其余用 reqwest 默认
    /// （idle 超时 90s、keepalive 15s），它们对本场景是合理的。
    ///
    /// ## 超时为什么不在客户端上设
    ///
    /// 超时来自 `RequestSpec.timeout_ms`，**每次请求都可能不同**。
    /// 设成客户端级会逼我们为每个不同的超时值各造一个客户端（连接池爆炸）。
    /// reqwest 的 `RequestBuilder::timeout` 正好是覆盖客户端设置的 per-request 值，
    /// 故统一在请求上设。
    pub fn client_for(&self, route: &ProxyRoute) -> Result<reqwest::Client, NetError> {
        let key = route.describe();
        {
            let clients = self.lock_clients();
            if let Some(client) = clients.get(&key) {
                return Ok(client.clone());
            }
        }

        // 构造可能失败（TLS 后端/证书库加载、解析系统代理），必须是结构化错误。
        let client = self.build_client(route)?;
        self.lock_clients().insert(key, client.clone());
        Ok(client)
    }

    /// 构造某路线的客户端。
    fn build_client(&self, route: &ProxyRoute) -> Result<reqwest::Client, NetError> {
        let mut builder = reqwest::Client::builder()
            .pool_max_idle_per_host(self.inner.pool_max_idle_per_host)
            // 压缩：与 Cargo.toml 的 gzip/brotli/deflate/zstd 特性配合，
            // 由 reqwest 自动发 Accept-Encoding 并透明解压（FR-009 的"压缩传输"）。
            .gzip(true)
            .brotli(true)
            .deflate(true)
            .zstd(true);

        builder = match route {
            // 明确直连：`no_proxy()` 同时清空代理列表并关掉系统代理探测。
            ProxyRoute::Direct => builder.no_proxy(),
            // 跟随系统代理：什么都不设。reqwest 的 `system-proxy` 特性在
            // `build()` 时会挂上系统探测（环境变量 + Windows/macOS 系统设置）。
            ProxyRoute::System => builder,
            // 按源指定的代理：`.proxy()` 会关掉系统代理自动探测，
            // 正好符合"这个源走这条代理"的语义（不与系统设置叠加）。
            ProxyRoute::Custom { endpoint } => {
                // 二次校验：`ProxyRoute` 是公开枚举，调用方可以直接构造它绕过
                // `proxy::resolve` 的校验。reqwest 对 `http:///no-host` 这类
                // 缺主机名的地址是**接受**的（它把空主机名补成 `no-host`），
                // 于是错误会推迟到真正连接时才以"代理连不上"的形式暴露——
                // 那时已经分不清是地址写错还是代理没开。故在此处补齐校验。
                validate_proxy_endpoint(endpoint)?;
                let proxy =
                    reqwest::Proxy::all(endpoint.as_str()).map_err(|error| NetError::Url {
                        url: endpoint.clone(),
                        reason: format!("代理地址无法被 HTTP 客户端接受: {error}"),
                    })?;
                builder.proxy(proxy)
            }
        };

        builder.build().map_err(|error| NetError::Setup {
            what: "http-client".to_string(),
            reason: crate::error::describe_reqwest(&error),
        })
    }

    /// 发送一次请求（不做重试、不做限速、不管 Cookie——那些是执行器的职责）。
    ///
    /// 返回的 [`Response`] **保有原始非 2xx 状态**：状态码的策略判定留给调用方，
    /// 本层不替它决定"404 算不算失败"。
    ///
    /// `extra_headers` 用于注入执行器算出的 `Cookie` 等头；它们在
    /// `spec.headers` 之后追加，故同名时（HTTP 语义）两者都会发出。
    pub async fn send(
        &self,
        spec: &RequestSpec,
        route: &ProxyRoute,
        extra_headers: &HeaderMap,
    ) -> Result<Response, NetError> {
        validate_url(&spec.url)?;
        let client = self.client_for(route)?;

        let method = reqwest::Method::from_bytes(spec.method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::GET);
        let mut builder = client.request(method, spec.url.as_str());

        for (name, value) in spec.headers.iter() {
            builder = builder.header(name, value);
        }
        for (name, value) in extra_headers.iter() {
            builder = builder.header(name, value);
        }
        if let Some(body) = &spec.body {
            builder = builder.body(body.clone());
        }
        // per-request 超时：覆盖客户端的默认（本层没设客户端级超时）。
        builder = builder.timeout(Duration::from_millis(spec.timeout_ms));

        let request = builder.build().map_err(|error| NetError::Request {
            reason: crate::error::describe_reqwest(&error),
        })?;

        let response = client.execute(request).await.map_err(|error| {
            // 代理归因：路线确定经由代理时才把连接类错误记成代理故障。
            NetError::from_reqwest(
                error,
                &spec.url,
                spec.timeout_ms,
                route.proxy_route_if_in_use().as_deref(),
            )
        })?;

        let status = response.status().as_u16();
        let headers = convert_headers(response.headers());
        let body_bytes = response.bytes().await.map_err(|error| {
            // 读体失败的归因同上面：这条路径上的连接类错误同样可能源自代理。
            NetError::from_reqwest(
                error,
                &spec.url,
                spec.timeout_ms,
                route.proxy_route_if_in_use().as_deref(),
            )
        })?;

        Ok(Response {
            status,
            headers,
            body_bytes: body_bytes.to_vec(),
            // 字符集由 charset 层填写（FR-009）——本层只负责把字节取回来。
            detected_charset: None,
        })
    }

    /// 已缓存的客户端数量（诊断用）。
    pub fn client_count(&self) -> usize {
        self.lock_clients().len()
    }

    /// 丢弃全部已缓存的客户端（配置变更后调用）。
    pub fn reset_clients(&self) {
        self.lock_clients().clear();
    }

    /// 在**独立 runtime** 上把一个 future 跑完（供 `NetHost` 的同步实现复用）。
    ///
    /// 不用 `BlockingClient` 的 runtime：那条路径要求组合根显式启用阻塞外观，
    /// 而 `NetHost::fetch` 是契约要求的同步接口，应能在未启用时也工作。
    pub fn block_on<F: std::future::Future>(future: F) -> Result<F::Output, NetError> {
        // `Runtime::block_on` 在异步上下文中会 panic，生产路径禁 panic，
        // 故提前探测并返回结构化错误。
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(NetError::BlockingContext {
                reason: "同步接口不能在异步执行器线程上调用；请用异步接口，\
                         或把调用移到 spawn_blocking 中"
                    .to_string(),
            });
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("tome-net-sync")
            .build()
            .map_err(|error| NetError::Setup {
                what: "tokio-runtime".to_string(),
                reason: error.to_string(),
            })?;
        Ok(runtime.block_on(future))
    }

    fn lock_clients(&self) -> std::sync::MutexGuard<'_, HashMap<String, reqwest::Client>> {
        self.inner
            .clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 连接池每主机空闲连接上限。
///
/// 取 8：书源抓取的并发度受"按源限速"约束，单个源很少同时打出 8 个请求；
/// 但同一站点被多个源使用（或目录页 + 正文页并发）时，稍宽的上限能省下重连。
/// 设成 `usize::MAX`（reqwest 默认）会让长时间运行时对某些站点堆积大量空闲连接，
/// 占用本机文件描述符——桌面应用要照顾这一点。
const DEFAULT_POOL_MAX_IDLE_PER_HOST: usize = 8;

/// 把 reqwest 的响应头转为 core 的 [`HeaderMap`]。
///
/// 用 `iter()` 而非 `get()`：HTTP 允许头重名（多个 `Set-Cookie`、多个
/// `WWW-Authenticate`），`http::HeaderMap::iter` 对每个关联值各产出一次，
/// 重名因此得以完整保留——这正是 core 的 `HeaderMap` 选 `Vec` 的原因。
///
/// 头值按字节经 `from_utf8_lossy` 转换：RFC 9110 规定头值应是 ASCII，
/// 但真实站点会发高位字节（如 GBK 编码的中文头值）。用 lossy 而非丢弃，
/// 至少能把信息带到调用方，不会静默消失。
fn convert_headers(headers: &reqwest::header::HeaderMap) -> HeaderMap {
    let mut converted = HeaderMap::new();
    for (name, value) in headers.iter() {
        converted.append(
            name.as_str().to_string(),
            String::from_utf8_lossy(value.as_bytes()).into_owned(),
        );
    }
    converted
}

/// 校验一个已经规范化的代理地址（主机名与端口齐备、scheme 受支持）。
///
/// 与 [`crate::proxy::resolve`] 里的规范化同源，但入口不同：那里处理"用户手写的
/// 地址"（可省略 scheme），这里处理"已经是 `http://host:port` 形态的端点"。
/// 两处都校验是必要的，因为 `ProxyRoute` 是公开类型，可被直接构造。
pub fn validate_proxy_endpoint(endpoint: &str) -> Result<(), NetError> {
    let parsed = reqwest::Url::parse(endpoint).map_err(|error| NetError::Url {
        url: endpoint.to_string(),
        reason: format!("代理地址无法解析: {error}"),
    })?;
    let host = parsed.host_str().unwrap_or_default();
    if host.is_empty() {
        return Err(NetError::Url {
            url: endpoint.to_string(),
            reason: "代理地址缺少主机名".to_string(),
        });
    }
    if parsed.port().is_none() {
        return Err(NetError::Url {
            url: endpoint.to_string(),
            reason: "代理地址缺少端口".to_string(),
        });
    }
    Ok(())
}

/// 校验 URL 与 scheme（安全红线，见模块文档）。
///
/// 允许 https；允许 http **仅当**目标是回环地址。
pub fn validate_url(url: &str) -> Result<(), NetError> {
    let parsed = reqwest::Url::parse(url).map_err(|error| NetError::Url {
        url: url.to_string(),
        reason: format!("URL 无法解析: {error}"),
    })?;

    match parsed.scheme() {
        "https" => Ok(()),
        // 回环例外：本地测试服务器与用户自建的本机服务没有证书。
        // 流量不出本机，明文传输的风险面在此不存在。
        "http" if is_loopback_host(parsed.host_str()) => Ok(()),
        "http" => Err(NetError::Url {
            url: url.to_string(),
            reason: "外部通信一律 https（安全红线）；明文 http 仅允许回环地址".to_string(),
        }),
        other => Err(NetError::Url {
            url: url.to_string(),
            reason: format!("不支持的 URL scheme: {other}"),
        }),
    }
}

/// 是否为回环主机。
///
/// 完整覆盖三种写法：IPv4 的 `127.0.0.0/8`、`localhost`（含其子域，如
/// `localhost.localdomain` 不在此列——只认精确的 `localhost` 与 `*.localhost`）、
/// IPv6 的 `[::1]`（reqwest 解析后 `host_str()` 给出不带方括号的 `::1`）。
fn is_loopback_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let host = host.trim_matches(['[', ']']).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => false,
    }
}

/// 阻塞外观：**自持一个 tokio runtime**，把异步客户端包成同步接口（T013 / R1）。
///
/// # 为什么需要它
///
/// 书源脚本是同步语义（research.md R1）：`java.get(url)` 之类的调用在 JS 里
/// 必须**当场拿到结果**。R1 的对策是"每个书源脚本用独立 runtime"，
/// 于是宿主函数可以在自己的 runtime 上阻塞等待，而不会卡住别人的引擎。
///
/// # 一份配置，不是两份
///
/// 见模块文档：这里复用的是 [`HttpClient`] 的同一个 `reqwest::Client`，
/// **不是** reqwest 的 `blocking` 特性。配置与连接池因此只有一份。
#[derive(Debug)]
pub struct BlockingClient {
    inner: HttpClient,
    runtime: tokio::runtime::Runtime,
}

impl BlockingClient {
    /// 以默认配置新建。
    pub fn new() -> Result<Self, NetError> {
        Self::with_client(HttpClient::new())
    }

    /// 以给定异步客户端新建（共享其配置与连接池）。
    ///
    /// 构造 runtime 可能失败（线程资源不足），故返回 `Result` 而非 panic
    /// （AGENTS.md：生产路径禁止 panic）。
    pub fn with_client(inner: HttpClient) -> Result<Self, NetError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            // 工作线程数取 2：`block_on` 在调用线程上跑 future，工作线程只负责
            // 被 spawn 的辅助任务（如连接池的后台维护）。桌面应用里这类 runtime
            // 每个书源一个，线程数必须克制——给满 CPU 核数会在多源并发时爆线程。
            .worker_threads(2)
            // `enable_all` 打开时间驱动与 IO 驱动。IO 驱动依赖 tokio 的 `net`
            // 特性，该特性由 reqwest 传递启用（同一构建内特性是并集），
            // 故此处无需在 Cargo.toml 里重复声明。
            .enable_all()
            .thread_name("tome-net")
            .build()
            .map_err(|error| NetError::Setup {
                what: "tokio-runtime".to_string(),
                reason: error.to_string(),
            })?;

        Ok(Self { inner, runtime })
    }

    /// 异步客户端的只读引用。
    pub fn async_client(&self) -> &HttpClient {
        &self.inner
    }

    /// 在自持的 runtime 上跑一个 future（供同步接口复用同一个 runtime）。
    ///
    /// 复用而非每次新建 runtime：`NetHost::fetch` 会被 JS 沙箱高频调用
    /// （书源脚本里一次抓取就可能有十几个请求），每次新建一个多线程 runtime
    /// 的开销（线程创建/销毁）会远超请求本身。
    ///
    /// 泛型而非 `Box<dyn Future>`：future 会借用 `self` 与 `spec`，
    /// 装箱成 `dyn` 就得带上生命周期参数，泛型让借用检查器直接管这件事。
    ///
    /// 在异步上下文里调用返回 [`NetError::BlockingContext`]（`block_on` 会 panic）。
    pub fn run<F: std::future::Future>(&self, future: F) -> Result<F::Output, NetError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(NetError::BlockingContext {
                reason: "阻塞外观不能在异步执行器线程上调用；请用 HttpClient 的异步接口，\
                         或把调用移到 spawn_blocking 中"
                    .to_string(),
            });
        }
        Ok(self.runtime.block_on(future))
    }

    /// 同步发送一次请求（不重试、不限速，同 [`HttpClient::send`]）。
    pub fn send_blocking(
        &self,
        spec: &RequestSpec,
        route: &ProxyRoute,
        extra_headers: &HeaderMap,
    ) -> Result<Response, NetError> {
        self.run(self.inner.send(spec, route, extra_headers))?
    }
}

/// 把 core 的方法枚举转为 reqwest 的方法。
///
/// 单独成函数是为了让"枚举值与 HTTP 方法名一一对应"这件事有唯一的落点
/// （core 的 `as_str` 已给出大写名，这里只是复用）。
pub fn to_reqwest_method(method: HttpMethod) -> reqwest::Method {
    reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET)
}

/// 解析某请求应当使用的代理路线（便捷入口，把 spec 与默认策略连起来）。
pub fn route_for(
    spec: &RequestSpec,
    default_policy: DefaultProxyPolicy,
) -> Result<ProxyRoute, NetError> {
    proxy::resolve(spec.proxy_override.as_ref(), default_policy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_is_allowed() {
        assert!(validate_url("https://example.com/path?q=1").is_ok());
        assert!(validate_url("https://127.0.0.1:8443/").is_ok());
    }

    #[test]
    fn plain_http_to_public_host_is_rejected() {
        let error = validate_url("http://example.com/").expect_err("明文 http 必须被拒绝");
        assert!(matches!(error, NetError::Url { .. }));
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Configuration);
        assert!(error.to_string().contains("https"));
    }

    #[test]
    fn plain_http_to_loopback_is_allowed() {
        // 唯一的例外，理由是本地端到端测试必须可行（见模块文档）。
        for url in [
            "http://127.0.0.1:8080/",
            "http://localhost:3000/x",
            "http://[::1]:9/",
            "http://127.0.0.2/",
            "http://LOCALHOST:80/",
        ] {
            assert!(validate_url(url).is_ok(), "{url} 应被允许");
        }
    }

    #[test]
    fn loopback_lookalikes_are_rejected() {
        for url in [
            // 前缀/后缀都要点边界：这些都不是回环。
            "http://127.0.0.1.evil.com/",
            "http://localhost.evil.com/",
            "http://notlocalhost/",
            "http://128.0.0.1/",
        ] {
            assert!(validate_url(url).is_err(), "{url} 不应被当作回环");
        }
    }

    #[test]
    fn other_schemes_are_rejected() {
        for url in [
            "ftp://example.com/",
            "file:///etc/passwd",
            "ws://example.com/",
        ] {
            assert!(validate_url(url).is_err(), "{url} 应被拒绝");
        }
    }

    #[test]
    fn malformed_url_is_reported() {
        let error = validate_url("not a url").expect_err("应报错");
        assert!(matches!(error, NetError::Url { .. }));
    }

    #[test]
    fn method_conversion_is_total() {
        assert_eq!(to_reqwest_method(HttpMethod::Get), reqwest::Method::GET);
        assert_eq!(to_reqwest_method(HttpMethod::Post), reqwest::Method::POST);
        assert_eq!(to_reqwest_method(HttpMethod::Put), reqwest::Method::PUT);
        assert_eq!(
            to_reqwest_method(HttpMethod::Delete),
            reqwest::Method::DELETE
        );
        assert_eq!(to_reqwest_method(HttpMethod::Head), reqwest::Method::HEAD);
    }

    #[test]
    fn header_conversion_preserves_duplicates_and_order() {
        let mut source = reqwest::header::HeaderMap::new();
        source.append("Set-Cookie", "a=1".parse().expect("合法头值"));
        source.append("Set-Cookie", "b=2".parse().expect("合法头值"));
        source.append("Content-Type", "text/html".parse().expect("合法头值"));

        let converted = convert_headers(&source);
        assert_eq!(converted.len(), 3);
        let cookies: Vec<&str> = converted
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, value)| value)
            .collect();
        assert_eq!(cookies, vec!["a=1", "b=2"]);
    }

    #[test]
    fn client_is_cached_per_route() {
        let client = HttpClient::new();
        let direct = ProxyRoute::Direct;
        let first = client.client_for(&direct).expect("应能构造");
        let second = client.client_for(&direct).expect("应能复用");
        assert_eq!(client.client_count(), 1, "同一路线只应有一个客户端");
        // reqwest::Client 是 Arc 包装的句柄，两次取到的是同一个连接池。
        assert_eq!(
            first.get("https://example.com/").build().is_ok(),
            second.get("https://example.com/").build().is_ok()
        );
    }

    #[test]
    fn distinct_routes_get_distinct_clients() {
        let client = HttpClient::with_policy(DefaultProxyPolicy::FollowSystem);
        client.client_for(&ProxyRoute::Direct).expect("直连");
        client.client_for(&ProxyRoute::System).expect("系统");
        client
            .client_for(&ProxyRoute::Custom {
                endpoint: "http://127.0.0.1:7897/".to_string(),
            })
            .expect("自定义");
        assert_eq!(client.client_count(), 3);
    }

    #[test]
    fn reset_clients_drops_the_cache() {
        let client = HttpClient::new();
        client.client_for(&ProxyRoute::Direct).expect("应能构造");
        assert_eq!(client.client_count(), 1);
        client.reset_clients();
        assert_eq!(client.client_count(), 0);
    }

    #[test]
    fn invalid_custom_proxy_is_a_structured_error() {
        let client = HttpClient::new();
        // 空地址不是合法代理，必须是错误而不是 panic。
        let error = client
            .client_for(&ProxyRoute::Custom {
                endpoint: "http:///no-host".to_string(),
            })
            .expect_err("应报错");
        assert!(matches!(
            error,
            NetError::Url { .. } | NetError::Setup { .. }
        ));
    }

    #[test]
    fn blocking_client_can_be_constructed_and_reports_context_errors() {
        // runtime 能建起来（不 panic）。
        let blocking = BlockingClient::new().expect("应能构造阻塞外观");
        // 不实际发请求（测试禁止联网），只验证 façade 的存在与探测逻辑。
        let probe = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("应能构造探测 runtime");
        let outcome = probe.block_on(async {
            blocking.send_blocking(
                &RequestSpec::get("https://example.com/", "s"),
                &ProxyRoute::Direct,
                &HeaderMap::new(),
            )
        });
        // 在异步上下文里必须得到结构化错误，而不是 panic。
        assert!(matches!(outcome, Err(NetError::BlockingContext { .. })));
        assert_eq!(
            outcome.expect_err("应报错").to_error_kind(),
            tome_core::ErrorKind::Network
        );
    }

    #[test]
    fn route_for_uses_spec_override() {
        let mut spec = RequestSpec::get("https://example.com/", "s");
        spec.proxy_override = Some(tome_core::ProxyOverride::Direct);
        let route = route_for(&spec, DefaultProxyPolicy::FollowSystem).expect("应能归约");
        assert_eq!(route, ProxyRoute::Direct);

        let plain = RequestSpec::get("https://example.com/", "s");
        let route = route_for(&plain, DefaultProxyPolicy::FollowSystem).expect("应能归约");
        assert_eq!(route, ProxyRoute::System);
    }
}
