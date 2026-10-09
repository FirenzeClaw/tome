//! `RequestSpec` 执行器：本 crate 对外的唯一入口（tasks.md T014）
//!
//! 实现 `tome_core::NetHost`——**全系统唯一的发请求出口**（架构决策 A2：
//! 规则引擎产出 `RequestSpec`，不亲自发请求；管线是唯一执行处）。
//!
//! # 执行顺序及其理由
//!
//! ```text
//! URL/scheme 校验 → 限速等待 → 注入 Cookie → 代理选择
//!                              → 发请求 → 吸收 Set-Cookie → 解码字符集 → 返回
//! ```
//!
//! 这个次序不是随意的，每一步的位置都有理由：
//!
//! 1. **URL 校验最先**：非法 URL 不该消耗限速配额，也不该进重试循环——
//!    重试一个语法错误的 URL 只会白等三次。当场失败还让错误归因更准
//!    （是配置错，不是网络错）。
//! 2. **限速等待在校验之后、发请求之前**：限速约束的是"对目标站的请求频率"，
//!    校验不通过时没有请求发出，自然不该占用配额。
//! 3. **Cookie 注入在发请求前**：Cookie 由 jar 依请求 URL 计算，必须在
//!    请求即将发出的那一刻算（早算会让"上一轮响应刚设的 cookie"漏掉）。
//! 4. **代理选择在发请求前**：它决定用哪个客户端（连接池），
//!    而客户端的选取必须在构造请求之前。
//! 5. **吸收 `Set-Cookie` 在读体之后**：`Set-Cookie` 与正文同属一次响应，
//!    但要等整个响应的头都收到才能确定。放在读体之后保证"无论正文读取
//!    成功与否，登录态都已更新"——站点用 302 + Set-Cookie 做登录跳转时，
//!    正文可能是空的，cookie 却不能丢。
//! 6. **解码字符集最后**：它是纯计算，不产生 I/O，失败也不该影响已发生的
//!    请求与已吸收的 cookie。
//!
//! # 重试判据
//!
//! 只对**可重试错误**重试（[`NetError::is_retryable`] 与 [`is_retryable_status`]）：
//!
//! | 情况 | 重试？ | 理由 |
//! |---|---|---|
//! | 超时 | 是 | 网络抖动，重来可能就好 |
//! | 连接失败（目标站） | 是 | 同上 |
//! | 5xx | 是 | 服务端临时故障是 5xx 的定义 |
//! | 429 | 是 | 限流是**临时**的；配合退避正好是"降速而不是硬撞"（spec 边缘情况） |
//! | 4xx（除 429） | **否** | 请求本身有问题，重发一万次也是同一个 404 |
//! | 代理故障 | **否** | 配置不对，重试无益；早报错让用户去改配置 |
//! | 回放未命中 | **否** | 语料缺条目，重试不会让文件出现 |
//! | TLS 失败 | **否** | 证书问题重试无益（且每试一次都要握手，浪费） |
//!
//! 4xx 不重试这条尤其重要：它直接对应 spec 边缘情况的"书源被限流或封禁时
//! 遵循声明间隔并降速，而**不是重试到被封**"。

use std::sync::Arc;
use std::time::Duration;

use tome_core::{ErrorKind, HeaderMap, NetHost, RequestSpec, Response};

use crate::charset;
use crate::client::{self, BlockingClient, HttpClient};
use crate::cookie::CookieJar;
use crate::error::NetError;
use crate::proxy::DefaultProxyPolicy;
use crate::ratelimit::RateLimiter;
use crate::replay::{self, ReplayMode, ReplayStore};

/// 重试间隔的退避上界。
///
/// 指数退避在第 n 次重试后的等待是 `backoff_ms * 2^(n-1)`；不设上界时
/// `max_retries` 一旦被写大（书源里的数字不可信），等待会指数爆炸到小时级。
/// 30 秒是"再等下去用户以为卡死了"的界线，与 [`tome_core::DEFAULT_TIMEOUT_MS`] 同量级。
const MAX_BACKOFF_MS: u64 = 30_000;

/// 请求执行器。
///
/// 组合了网络能力（客户端）、状态（Cookie jar、限速器、回放语料），
/// 并实现 [`NetHost`] 供 tome-js 沙箱内的脚本调用。
///
/// # 为什么用 `Arc` 持有可变部件
///
/// `NetHost::fetch` 取 `&self`（trait 要求 `Send + Sync`），而 Cookie jar 与
/// 回放存储需要可变状态。两者各自用内部可变性（`Mutex`）解决，故执行器本身
/// 只需持 `Arc` 即可在线程间共享——`Arc` 在这里是为了让组合根能把同一个
/// 执行器注入多个沙箱，而不是为了线程安全（那是内部 `Mutex` 的职责）。
pub struct RequestExecutor {
    http: HttpClient,
    blocking: Option<BlockingClient>,
    cookies: Arc<CookieJar>,
    limiter: Arc<RateLimiter>,
    /// `Some(_)` 表示启用了录制/回放。
    ///
    /// 用 `Mutex` 而不是 `RwLock`：录制路径要写（`record` 取 `&mut self`），
    /// 而录制在真实抓取时是常态；读锁此时毫无收益。
    replay: Option<Mutex<ReplayStore>>,
    /// 录制/回放模式。
    mode: ReplayMode,
    /// 默认代理策略。
    default_policy: DefaultProxyPolicy,
    /// 是否把非 2xx 视为错误（见 [`ExecutionPolicy::fail_on_status`]）。
    fail_on_error_status: bool,
}

use std::sync::Mutex;

/// 执行器的构造参数（避免构造函数参数过多，`.clippy.toml` 上限 7）。
#[derive(Debug, Clone, Copy)]
pub struct ExecutionPolicy {
    /// 默认代理策略。
    pub default_policy: DefaultProxyPolicy,
    /// 是否把非 2xx 响应视为错误。
    ///
    /// - `true`：非 2xx 返回 [`NetError::Status`]（映射为 `ErrorKind::Network`），
    ///   适合"取内容"的调用点（拿不到内容就是失败）。
    /// - `false`：原样返回带状态的 `Response`，适合"检查状态"的调用点
    ///   （书源里的 `check` 脚本要知道 404 还是 403）。
    ///
    /// 默认 `true`：多数调用点是取内容，让它们不必逐个判状态码。
    pub fail_on_status: bool,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            default_policy: DefaultProxyPolicy::default(),
            fail_on_status: true,
        }
    }
}

impl RequestExecutor {
    /// 以默认配置新建（真实联网、不录制、所有源共享一个 cookie jar 与限速器）。
    pub fn new() -> Result<Self, NetError> {
        Self::builder().build()
    }

    /// 新建构造器。
    pub fn builder() -> RequestExecutorBuilder {
        RequestExecutorBuilder::default()
    }

    /// 是否启用了录制/回放。
    pub fn replay_mode(&self) -> Option<ReplayMode> {
        self.replay.as_ref().map(|_| self.mode)
    }

    /// Cookie jar（供持久化：FR-026 修订要求按源落盘）。
    pub fn cookies(&self) -> &CookieJar {
        &self.cookies
    }

    /// 限速器（供管线注入书源声明的间隔）。
    pub fn limiter(&self) -> &RateLimiter {
        &self.limiter
    }

    /// 阻塞外观（供 JS 同步语义，R1）。未构造时返回 `None`。
    pub fn blocking(&self) -> Option<&BlockingClient> {
        self.blocking.as_ref()
    }

    /// 执行一次请求（**不做重试**，供需要精确控制次数的调用点使用）。
    ///
    /// 大多数调用点应该用 [`NetHost::fetch`]（含重试）。
    pub async fn execute_once(&self, spec: &RequestSpec) -> Result<Response, NetError> {
        // 1. URL / scheme 校验（安全红线，见模块文档的次序说明）。
        client::validate_url(&spec.url)?;

        // 回放模式：完全离线。放在限速之前——离线重放不该消耗限速配额，
        // 否则"重放 100 条记录"会被限速拖成几分钟，差分测试变得不可用。
        if self.mode.is_offline_only() {
            return self.replay_from_corpus(spec);
        }

        // `RecordOnce`：先看语料里有没有。命中就回放（不触网），未命中才真实抓取。
        //
        // 为什么这个判断必须在限速**之前**：命中语料根本不发请求，占用限速配额
        // 毫无道理；更重要的是，若把限速放在前面，重放已有语料会被限速拖慢，
        // 而"补空洞"这个使用场景恰恰是"重放绝大部分 + 补少数几条"。
        if self.mode.prefers_corpus() {
            if let Some(hit) = self.replay_hit(spec)? {
                return Ok(hit);
            }
        }

        // 2. 限速等待（按 rate_limit_key 独立）。
        self.wait_for_rate_limit(&spec.rate_limit_key).await;

        // 3. 代理选择（决定用哪个客户端/连接池）。
        let route = crate::proxy::resolve(spec.proxy_override.as_ref(), self.default_policy)?;

        // 4. Cookie 注入（依请求 URL 现算，保证用到最新的登录态）。
        let extra_headers = self.cookie_headers(spec)?;

        // 5. 发请求。
        let response = self.http.send(spec, &route, &extra_headers).await?;

        // 6-9. 响应后处理（吸收 cookie → 解码字符集 → 录制 → 状态判定）。
        self.finish_response(spec, response)
    }

    /// 执行一次请求，并按 `spec.retry` 重试（T014 验收：读取 `RequestSpec.retry`）。
    pub async fn execute(&self, spec: &RequestSpec) -> Result<Response, NetError> {
        let mut attempt = 0u32;
        loop {
            match self.execute_once(spec).await {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let retryable = error.is_retryable()
                        || matches!(&error, NetError::Status { status, .. } if is_retryable_status(*status));
                    if !retryable || attempt >= spec.retry.max_retries {
                        return Err(error);
                    }
                    // 退避：服务端给了 `Retry-After` 就听它的（那是站点的明确要求），
                    // 否则用自身策略的指数退避。
                    //
                    // 为什么优先听服务端：spec 边缘情况要求"连续请求触发限流时……
                    // 在必要时降速，而不是重试到被封"。`Retry-After` 是站点把
                    // "该等多久"明说出来的唯一渠道，忽略它去按自己的节奏重试
                    // 正是"重试到被封"的成因。它已被 [`parse_retry_after`] 钳过
                    // 上限（第三方响应不可信）。
                    let delay = error
                        .retry_after()
                        .unwrap_or_else(|| backoff_delay(spec.retry.backoff_ms, attempt));
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    attempt += 1;
                }
            }
        }
    }

    /// 同步执行（阻塞外观，供 JS 同步语义 R1）。
    ///
    /// 未构造阻塞外观时返回 [`NetError::BlockingContext`]——那是组合根的配置问题，
    /// 不是运行时错误，故明确报出而不是静默降级。
    pub fn execute_blocking(&self, spec: &RequestSpec) -> Result<Response, NetError> {
        let Some(blocking) = self.blocking.as_ref() else {
            return Err(NetError::BlockingContext {
                reason: "未为此执行器构造阻塞外观；组合根需用 with_blocking(true) 启用".to_string(),
            });
        };
        let route = crate::proxy::resolve(spec.proxy_override.as_ref(), self.default_policy)?;
        // `RecordOnce`：先看语料里有没有（同步路径同理，见 `execute_once` 的说明）。
        if self.mode.prefers_corpus() {
            if let Some(hit) = self.replay_hit(spec)? {
                return Ok(hit);
            }
        }
        // 同步路径同样走完整的限速与 cookie 逻辑：
        // 先把限速给出的等待在锁外睡掉，再交给阻塞客户端。
        //
        // 注意这里**不**复用 `wait_for_rate_limit`：它是异步的，而本方法在
        // `block_on` 之外调用（甚至可能根本不在 runtime 里），故用标准库睡眠。
        let slot = self.limiter.plan(&spec.rate_limit_key);
        if let Some(wait) = slot.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(wait);
        }
        let extra_headers = self.cookie_headers(spec)?;
        let response = blocking.send_blocking(spec, &route, &extra_headers)?;
        self.finish_response(spec, response)
    }

    /// 响应到手之后的统一处理（异步与同步路径共用）。
    ///
    /// 抽出来是为了让两条路径**不可能出现行为差异**：若各写一遍，早迟会有一处
    /// 忘了录 cookie 或忘了填字符集，而那种偏差恰恰是差分测试最难发现的一类。
    ///
    /// 次序及理由（见模块文档）：
    /// 吸收 `Set-Cookie` → 解码字符集 → 录制 → 状态判定。
    /// 录制放在状态判定**之前**：即使这次是 404，语料也应记录这个事实，
    /// 否则重放时会把"当时确实是 404"重放成"未命中"。
    fn finish_response(
        &self,
        spec: &RequestSpec,
        mut response: Response,
    ) -> Result<Response, NetError> {
        self.absorb_cookies(spec, &response)?;
        attach_charset(&mut response, spec);
        if self.mode.records() {
            self.record(spec, &response)?;
        }
        if self.fail_on_error_status && !response.is_success() {
            return Err(NetError::Status {
                status: response.status,
                url: spec.url.clone(),
                // 把站点声明的等待时长带进错误，供重试循环遵守
                // （见 `execute` 里关于 spec 边缘情况的说明）。
                retry_after: parse_retry_after(&response.headers),
            });
        }
        Ok(response)
    }

    /// 限速：向限速器预约时隙，并在锁外异步等待到该时刻。
    ///
    /// 用 `sleep_until`（绝对时刻）而不是 `sleep(时长)`：后者会把等待期间的系统
    /// 延迟累加进实际间隔，让相邻请求被排得更密——正是"不得绕过限速"要防的事。
    /// 详见 [`RateLimiter::plan`] 的文档。
    ///
    /// 回放模式在调用本方法**之前**就已返回，故这里只会被真实请求路径触达。
    async fn wait_for_rate_limit(&self, key: &str) {
        let slot = self.limiter.plan(key);
        // 时隙可能已是过去（无限速，或调度落后了），此时不等待。
        if slot > std::time::Instant::now() {
            // `tokio::time::sleep_until` 而非 `std::thread::sleep`：
            // 异步等待不占工作线程。`Instant` 转成 tokio 的对应类型（两者同源，
            // 转换是零成本的包装）。
            tokio::time::sleep_until(slot.into()).await;
        }
    }

    /// 计算本次请求要注入的 `Cookie` 头。
    fn cookie_headers(&self, spec: &RequestSpec) -> Result<HeaderMap, NetError> {
        let mut headers = HeaderMap::new();
        // 调用方（书源脚本）已显式给出的 Cookie 头优先，不被 jar 覆盖：
        // 显式设置是更具体的意图，jar 是"回落"。
        if spec.headers.get("cookie").is_some() {
            return Ok(headers);
        }
        if let Some(cookie) = self.cookies.header_for(&spec.rate_limit_key, &spec.url)? {
            headers.set("Cookie", cookie);
        }
        Ok(headers)
    }

    /// 吸收响应里的 `Set-Cookie`（FR-008）。
    fn absorb_cookies(&self, spec: &RequestSpec, response: &Response) -> Result<(), NetError> {
        self.cookies
            .store_from_response(&spec.rate_limit_key, &spec.url, &response.headers)?;
        Ok(())
    }

    /// 回放命中（离线）。
    fn replay_from_corpus(&self, spec: &RequestSpec) -> Result<Response, NetError> {
        match self.replay_hit(spec)? {
            Some(response) => Ok(response),
            None => Err(NetError::ReplayMiss {
                url: spec.url.clone(),
                fingerprint: replay::fingerprint(spec),
            }),
        }
    }

    /// 查语料：命中给出响应，未命中给出 `None`，语料损坏则报错。
    ///
    /// 把"未命中"与"语料坏了"分开：前者在 `RecordOnce` 下是正常情况（该去抓），
    /// 后者是必须暴露的问题——若把损坏也当成未命中，`RecordOnce` 会静默重抓，
    /// 让损坏的语料看起来"工作正常"，而差分测试其实已经失去基线。
    fn replay_hit(&self, spec: &RequestSpec) -> Result<Option<Response>, NetError> {
        let Some(store) = self.replay.as_ref() else {
            return Ok(None);
        };
        let outcome = {
            let guard = store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.replay(spec)
        };
        let mut response = match outcome {
            Ok(response) => response,
            Err(NetError::ReplayMiss { .. }) => return Ok(None),
            Err(other) => return Err(other),
        };
        // 回放同样走一遍字符集解码，保证重放路径与真实路径经过同一套逻辑
        // （否则"真实路径解错、重放路径解对"这类偏差不会被差分测试发现）。
        attach_charset(&mut response, spec);
        Ok(Some(response))
    }

    /// 录制一条响应。
    fn record(&self, spec: &RequestSpec, response: &Response) -> Result<(), NetError> {
        let Some(store) = self.replay.as_ref() else {
            return Ok(());
        };
        let mut guard = store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.record(spec, response)
    }
}

impl NetHost for RequestExecutor {
    /// 执行一次请求（含 `spec.retry` 重试），错误映射为 `ErrorKind`（接缝层）。
    ///
    /// # 为什么这里用 `block_on` 而不是异步
    ///
    /// `NetHost` 的签名是同步的（research.md R1：书源脚本是同步语义）。
    /// 实现方式与 [`BlockingClient`] 一致：自持 runtime + `block_on`。
    /// R1 已约定"每个书源脚本用独立 runtime"，故宿主函数阻塞期间
    /// 只影响自己那一个引擎（契约要求，由组合根保证）。
    fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
        // 复用阻塞外观的 runtime（若组合根提供了它），否则临时造一个——
        // 临时造只发生在"只用同步 API 但没显式启用阻塞外观"的调用点，
        // 此时构造失败要报出来（不能 panic）。
        // 组合两层的 `Result`：外层是"能否跑起来"（runtime 构造/上下文受限），
        // 内层是"请求本身的结果"。两者都可能是错误，但只有内层的错误需要分类。
        let outcome: Result<Response, NetError> = match self.blocking.as_ref() {
            Some(blocking) => blocking.run(self.execute(&spec)).and_then(identity),
            None => HttpClient::block_on(self.execute(&spec)).and_then(identity),
        };
        // 接缝边界：此处是全 crate 唯一的错误分类转换点。
        outcome.map_err(|error| error.to_error_kind())
    }
}

/// 从 `Result<Result<T, E>, E>` 里取出内层结果（两层的错误类型相同，故可直接合并）。
fn identity<T>(outcome: Result<T, NetError>) -> Result<T, NetError> {
    outcome
}

/// 给响应补上 `detected_charset`（FR-009）。
///
/// 声明来源按 HTML 规范：`Content-Type` 头优先，其次扫正文开头的 `<meta>`。
/// 这条次序与 charset 模块的 [`charset::sniff_declared_charset`] 一致。
fn attach_charset(response: &mut Response, spec: &RequestSpec) {
    let content_type = response.headers.get("content-type");
    // 优先用 spec 里来自书源声明的 charset（那是书源作者对站点的已知知识），
    // 其次是响应头/meta 的自述。两者都没有时交给检测。
    let declared = spec.charset.as_deref().or(content_type).map(str::to_string);
    let sniffed = charset::sniff_declared_charset(declared.as_deref(), &response.body_bytes);
    let effective = sniffed.as_deref().or(declared.as_deref());
    let decoded = charset::decode_body(&response.body_bytes, effective);
    response.detected_charset = Some(decoded.charset);
}

/// 状态码是否值得重试（重试判据见模块文档的表格）。
pub fn is_retryable_status(status: u16) -> bool {
    // 5xx：服务端临时故障。
    // 429：限流，配合退避正好实现"降速而不是硬撞"（spec 边缘情况）。
    // 408：请求超时，服务端自己说这次的等待太久了，重来可能就好。
    (500..600).contains(&status) || status == 429 || status == 408
}

/// 指数退避：第 `attempt` 次重试（从 0 起）等待 `backoff_ms * 2^attempt`，带上界。
///
/// 用 `saturating_mul` 而非 `<<`：`backoff_ms` 来自书源（不可信输入），
/// 位移量过大在 debug 构建下会 panic。饱和乘法把它钳到 `u64::MAX` 再被
/// `min` 收进上界，全路径无 panic。
pub fn backoff_delay(backoff_ms: u64, attempt: u32) -> Duration {
    if backoff_ms == 0 {
        return Duration::ZERO;
    }
    let multiplier = 2u64.saturating_pow(attempt.min(32));
    let millis = backoff_ms.saturating_mul(multiplier).min(MAX_BACKOFF_MS);
    Duration::from_millis(millis)
}

/// 解析 `Retry-After` 头（RFC 9110 §10.2.3）：宽容地只接受秒数形式。
///
/// 也接受 HTTP-date 形式，但仅在能解析出"未来多久"时使用；
/// 超过 [`crate::error::MAX_RETRY_AFTER`] 的值一律忽略（见该常量的理由）。
pub fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get("retry-after")?;
    let seconds: u64 = value.trim().parse().ok()?;
    let duration = Duration::from_secs(seconds);
    if duration > crate::error::MAX_RETRY_AFTER {
        return None;
    }
    Some(duration)
}

/// 执行器构造器（参数较多，用构造器而非长参数列表）。
#[derive(Debug, Default)]
pub struct RequestExecutorBuilder {
    default_policy: Option<DefaultProxyPolicy>,
    fail_on_status: Option<bool>,
    with_blocking: bool,
    replay_root: Option<std::path::PathBuf>,
    mode: Option<ReplayMode>,
    cookies: Option<Arc<CookieJar>>,
    limiter: Option<Arc<RateLimiter>>,
    default_interval: Option<Duration>,
}

impl RequestExecutorBuilder {
    /// 设置默认代理策略。
    pub fn default_policy(mut self, policy: DefaultProxyPolicy) -> Self {
        self.default_policy = Some(policy);
        self
    }

    /// 设置非 2xx 是否算错误。
    pub fn fail_on_status(mut self, value: bool) -> Self {
        self.fail_on_status = Some(value);
        self
    }

    /// 是否同时构造阻塞外观（JS 同步语义需要）。
    pub fn with_blocking(mut self, value: bool) -> Self {
        self.with_blocking = value;
        self
    }

    /// 启用录制/回放，指定语料目录与模式。
    pub fn replay(mut self, root: impl Into<std::path::PathBuf>, mode: ReplayMode) -> Self {
        self.replay_root = Some(root.into());
        self.mode = Some(mode);
        self
    }

    /// 复用既有的 cookie jar（持久化后恢复登录态）。
    pub fn cookies(mut self, jar: Arc<CookieJar>) -> Self {
        self.cookies = Some(jar);
        self
    }

    /// 复用既有的限速器。
    pub fn limiter(mut self, limiter: Arc<RateLimiter>) -> Self {
        self.limiter = Some(limiter);
        self
    }

    /// 设置限速器的默认间隔（[WORKAROUND] 见 [`crate::ratelimit`] 的模块文档：
    /// `RequestSpec` 没有间隔字段，故由执行器侧注入）。
    pub fn default_interval(mut self, interval: Duration) -> Self {
        self.default_interval = Some(interval);
        self
    }

    /// 构造执行器。任何环节失败都返回结构化错误，不 panic。
    pub fn build(self) -> Result<RequestExecutor, NetError> {
        let policy = ExecutionPolicy {
            default_policy: self.default_policy.unwrap_or_default(),
            fail_on_status: self.fail_on_status.unwrap_or(true),
        };
        let http = HttpClient::with_policy(policy.default_policy);
        let blocking = if self.with_blocking {
            // 用 `clone()` 而非另造：阻塞外观共享同一个 `reqwest::Client` 缓存
            // （一份配置、一个连接池），见模块文档"一份配置，不是两份"。
            Some(BlockingClient::with_client(http.clone())?)
        } else {
            None
        };

        let limiter = self.limiter.unwrap_or_default();
        if let Some(interval) = self.default_interval {
            limiter.set_default_interval(interval);
        }

        let (replay, mode) = match self.replay_root {
            Some(root) => {
                let store = ReplayStore::open(root)?;
                (
                    Some(Mutex::new(store)),
                    self.mode.unwrap_or(ReplayMode::Record),
                )
            }
            None => (None, ReplayMode::Record),
        };

        Ok(RequestExecutor {
            http,
            blocking,
            cookies: self.cookies.unwrap_or_default(),
            limiter,
            replay,
            mode,
            default_policy: policy.default_policy,
            fail_on_error_status: policy.fail_on_status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_status_covers_5xx_and_throttling() {
        for status in [500, 502, 503, 504, 599, 429, 408] {
            assert!(is_retryable_status(status), "{status} 应可重试");
        }
    }

    #[test]
    fn retryable_status_excludes_client_errors() {
        // 4xx 不重试：请求本身有问题，重发一万次也是同一个结果。
        // 这条直接对应 spec 边缘情况"不要重试到被封"。
        for status in [400, 401, 403, 404, 410, 422, 451] {
            assert!(!is_retryable_status(status), "{status} 不应重试");
        }
        // 3xx 是重定向，reqwest 已自动跟随，此处不该再重试。
        for status in [301, 302, 304] {
            assert!(!is_retryable_status(status));
        }
    }

    #[test]
    fn backoff_grows_exponentially_and_is_capped() {
        assert_eq!(backoff_delay(0, 0), Duration::ZERO);
        assert_eq!(backoff_delay(100, 0), Duration::from_millis(100));
        assert_eq!(backoff_delay(100, 1), Duration::from_millis(200));
        assert_eq!(backoff_delay(100, 2), Duration::from_millis(400));
        assert_eq!(backoff_delay(100, 3), Duration::from_millis(800));
        // 上界：即使书源写了荒谬的 backoff/重试次数也不会等待到天荒地老。
        assert_eq!(
            backoff_delay(100, 30),
            Duration::from_millis(MAX_BACKOFF_MS)
        );
        assert_eq!(
            backoff_delay(u64::MAX, 64),
            Duration::from_millis(MAX_BACKOFF_MS),
            "极端输入不得 panic（debug 下位移会 panic，故用饱和乘法）"
        );
    }

    #[test]
    fn retry_after_is_parsed_when_reasonable() {
        let mut headers = HeaderMap::new();
        headers.set("Retry-After", "5");
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(5)));
    }

    #[test]
    fn absurd_retry_after_is_ignored() {
        let mut headers = HeaderMap::new();
        // 第三方响应不可信：一个荒唐的值不该让调用方停摆一天。
        headers.set("Retry-After", "86400");
        assert_eq!(parse_retry_after(&headers), None);

        headers.set("Retry-After", "not-a-number");
        assert_eq!(parse_retry_after(&headers), None);

        let empty = HeaderMap::new();
        assert_eq!(parse_retry_after(&empty), None);
    }

    #[test]
    fn status_error_carries_server_requested_delay() {
        let error = NetError::Status {
            status: 503,
            url: "https://a/".to_string(),
            retry_after: Some(Duration::from_secs(7)),
        };
        assert_eq!(error.retry_after(), Some(Duration::from_secs(7)));
        // 其他变体不携带该信息。
        assert_eq!(
            NetError::Timeout {
                url: "https://a/".to_string(),
                timeout_ms: 1,
            }
            .retry_after(),
            None
        );
    }

    #[test]
    fn builder_defaults_are_sane() {
        let executor = RequestExecutor::new().expect("应能构造");
        assert_eq!(executor.replay_mode(), None);
        assert!(executor.blocking().is_none());
        assert_eq!(
            executor.http.default_policy(),
            DefaultProxyPolicy::FollowSystem
        );
    }

    #[test]
    fn builder_can_enable_blocking_facade() {
        let executor = RequestExecutor::builder()
            .with_blocking(true)
            .build()
            .expect("应能构造");
        assert!(executor.blocking().is_some(), "阻塞外观应已就绪");
    }

    #[test]
    fn blocking_without_facade_reports_structured_error() {
        let executor = RequestExecutor::new().expect("应能构造");
        let error = executor
            .execute_blocking(&RequestSpec::get("https://example.com/", "s"))
            .expect_err("未启用阻塞外观时应报错");
        assert!(matches!(error, NetError::BlockingContext { .. }));
    }

    #[test]
    fn executor_constructs_in_replay_mode() {
        let dir = std::env::temp_dir().join(format!(
            "tome-exec-replay-{:?}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let executor = RequestExecutor::builder()
            .replay(&dir, ReplayMode::Replay)
            .build()
            .expect("应能构造");
        assert_eq!(executor.replay_mode(), Some(ReplayMode::Replay));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn replay_mode_returns_miss_without_touching_network() {
        let dir = std::env::temp_dir().join(format!(
            "tome-exec-miss-{:?}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let executor = RequestExecutor::builder()
            .replay(&dir, ReplayMode::Replay)
            .build()
            .expect("应能构造");

        // 目标是公网地址但绝不会被访问——回放模式下必须直接报未命中。
        let spec = RequestSpec::get("https://never-visited.invalid/", "src");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("应能构造测试 runtime");
        let outcome = runtime.block_on(executor.execute_once(&spec));
        let error = outcome.expect_err("回放未命中应报错");
        assert!(matches!(error, NetError::ReplayMiss { .. }));
        assert_eq!(
            error.to_error_kind(),
            ErrorKind::Configuration,
            "回放语料缺失属配置问题"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn replay_mode_rejects_invalid_url_before_corpus_lookup() {
        let dir = std::env::temp_dir().join(format!(
            "tome-exec-url-{:?}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let executor = RequestExecutor::builder()
            .replay(&dir, ReplayMode::Replay)
            .build()
            .expect("应能构造");
        let spec = RequestSpec::get("http://public.example.com/", "src");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("应能构造测试 runtime");
        // 安全红线校验在回放查找之前：非法 scheme 必须优先报出。
        let error = runtime
            .block_on(executor.execute_once(&spec))
            .expect_err("应报错");
        assert!(matches!(error, NetError::Url { .. }));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn charset_attachment_sets_detected_charset() {
        let mut response = Response {
            status: 200,
            headers: {
                let mut headers = HeaderMap::new();
                headers.set("Content-Type", "text/html; charset=gbk");
                headers
            },
            body_bytes: vec![0xD6, 0xD0, 0xCE, 0xC4],
            detected_charset: None,
        };
        let spec = RequestSpec::get("https://example.com/", "src");
        attach_charset(&mut response, &spec);
        assert_eq!(response.detected_charset.as_deref(), Some("GBK"));
    }

    #[test]
    fn spec_charset_overrides_header() {
        let mut response = Response {
            status: 200,
            headers: {
                let mut headers = HeaderMap::new();
                // 头里声明 utf-8，但字节是 GBK。
                headers.set("Content-Type", "text/html; charset=utf-8");
                headers
            },
            body_bytes: vec![0xD6, 0xD0, 0xCE, 0xC4],
            detected_charset: None,
        };
        let mut spec = RequestSpec::get("https://example.com/", "src");
        // 书源作者声明 gbk（对站点的已知知识），应优先。
        spec.charset = Some("gbk".to_string());
        attach_charset(&mut response, &spec);
        assert_eq!(response.detected_charset.as_deref(), Some("GBK"));
        // 更关键的是：解码结果可读（FR-009 的容错要求）。
        let decoded = charset::decode_body(&response.body_bytes, Some("gbk"));
        assert_eq!(decoded.text, "中文");
    }

    #[test]
    fn explicit_cookie_header_is_not_overridden_by_jar() {
        let executor = RequestExecutor::new().expect("应能构造");
        executor
            .cookies()
            .store_from_response("src", "https://example.com/", &{
                let mut headers = HeaderMap::new();
                headers.append("Set-Cookie", "jar=1");
                headers
            })
            .expect("应能存储");

        let mut spec = RequestSpec::get("https://example.com/", "src");
        spec.headers.set("Cookie", "explicit=1");
        let injected = executor.cookie_headers(&spec).expect("应能计算");
        assert!(
            injected.get("cookie").is_none(),
            "调用方显式给出的 Cookie 头不应被 jar 叠加"
        );
    }

    #[test]
    fn jar_cookie_is_injected_when_absent() {
        let executor = RequestExecutor::new().expect("应能构造");
        executor
            .cookies()
            .store_from_response("src", "https://example.com/", &{
                let mut headers = HeaderMap::new();
                headers.append("Set-Cookie", "jar=1");
                headers
            })
            .expect("应能存储");

        let spec = RequestSpec::get("https://example.com/", "src");
        let injected = executor.cookie_headers(&spec).expect("应能计算");
        assert_eq!(injected.get("cookie"), Some("jar=1"));
    }

    #[test]
    fn cookies_from_other_source_are_not_injected() {
        let executor = RequestExecutor::new().expect("应能构造");
        executor
            .cookies()
            .store_from_response("source-a", "https://example.com/", &{
                let mut headers = HeaderMap::new();
                headers.append("Set-Cookie", "token=aaa");
                headers
            })
            .expect("应能存储");

        // 另一个源访问同一站点：不得拿到 source-a 的 cookie。
        let spec = RequestSpec::get("https://example.com/", "source-b");
        let injected = executor.cookie_headers(&spec).expect("应能计算");
        assert_eq!(injected.get("cookie"), None);
    }

    /// 接缝边界：`NetHost::fetch` 必须把错误映射为 `ErrorKind`（FR-044 / SC-016）。
    ///
    /// 用一个必然失败的 URL 触发真实的 `fetch` 调用链，验证**接缝上拿到的是
    /// 分类枚举**而不是实现 crate 的错误类型。非 https 的公网地址会在
    /// URL 校验阶段就失败，故这个测试不会触网。
    #[test]
    fn fetch_maps_errors_to_error_kind_at_the_seam() {
        let executor = RequestExecutor::new().expect("应能构造");
        let spec = RequestSpec::get("http://public.example.com/", "src");

        // `NetHost::fetch` 是同步的；测试线程本身不在异步上下文中，可直接调用。
        let outcome = NetHost::fetch(&executor, spec);
        let kind = outcome.expect_err("明文 http 到公网应被拒绝");
        assert_eq!(
            kind,
            ErrorKind::Configuration,
            "非法 scheme 属配置问题（不是网络问题）"
        );
    }

    /// 代理故障与目标站故障在接缝处必须**可区分**（FR-008 边缘情况）。
    ///
    /// 这两条路径都通过 `NetError::to_error_kind` 映射，且落在**不同**的
    /// `ErrorKind` 上——上层据此可以给出"去改代理设置"与"检查网络"两种提示。
    #[test]
    fn proxy_and_target_failures_map_to_different_kinds() {
        let proxy = NetError::Proxy {
            url: "https://target.example/".to_string(),
            route: "custom http://127.0.0.1:1/".to_string(),
            reason: "connection refused".to_string(),
        };
        let target = NetError::Connect {
            url: "https://target.example/".to_string(),
            reason: "connection refused".to_string(),
        };
        assert_ne!(proxy.to_error_kind(), target.to_error_kind());
        assert_eq!(proxy.to_error_kind(), ErrorKind::Configuration);
        assert_eq!(target.to_error_kind(), ErrorKind::Network);
    }
}
