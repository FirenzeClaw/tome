//! [DONE] `NetHost` 的实现接线（tasks.md T032 / FR-005 网络类）
//!
//! # 本模块的职责边界
//!
//! `NetHost` 的**实现在 [`crate::executor`]**（[`RequestExecutor`] 的
//! `impl NetHost for` 块）——那是"全系统唯一的发请求出口"（架构决策 A2）。
//! 本模块不重复实现它，而是回答另一个问题：
//!
//! > **JS 沙箱的 `ajax`/`get`/`post`/`head` 怎么走到那条出口上？**
//!
//! 答案是"经这个模块"：它把 `NetHost` 包成**面向书源脚本的调用形态**，
//! 处理脚本侧与接缝侧之间的三处语义落差（见下）。tome-js 的
//! `api::net`（T036）拿到的就是这个形态。
//!
//! # 三处语义落差（逐条对应一个真实的书源用法）
//!
//! ## 1. 脚本给的是"要什么"，不是 `RequestSpec`
//!
//! 书源脚本写 `java.get(url)` / `java.post(url, body)` / `java.head(url)`，
//! 它们没有 `rate_limit_key`、`timeout`、`retry` 这些概念。故这里要把
//! "裸调用"补全成完整的 [`RequestSpec`]，且补全方式必须与**管线**一致
//! （限速键用书源 id，否则同一个源的脚本请求与规则请求各限一份，
//! 反而更容易触发对方风控——FR-008 的"按源"限速会失效）。
//!
//! ## 2. 状态码算不算失败，脚本侧与接缝侧判断不同
//!
//! 接缝按 R3 的约定："事务完成但状态非 2xx"返回 `Ok(Response)`（错误分类权
//! 在调用方）。而脚本侧的 `java.get` **期望拿到正文**——大多数书源脚本
//! 直接 `JSON.parse(java.get(url))`，拿到一个 404 的 HTML 页会立刻抛异常。
//!
//! 故本模块提供两种形态，让调用方显式选择：
//!
//! | 形态 | 非 2xx 时 | 用于 |
//! |---|---|---|
//! | [`HostRequest::send`] | 原样返回 `Response` | `java.ajax`（脚本自己判状态） |
//! | [`HostRequest::send_text`] | 报 `ErrorKind::Network` | `java.get`/`post`（要正文） |
//!
//! 两者共用同一个执行器与同一套限速/Cookie/字符集逻辑——差异只在**状态判定**
//! 这一处，不重复实现任何传输逻辑。
//!
//! ## 3. 编码：脚本要的是字符串，不是字节
//!
//! `Response::body_bytes` 是字节（解码是 charset 层的关切，FR-009）。
//! 脚本侧的 `java.get` 返回**字符串**，且必须走容错解码（书源声明的
//! charset 与实际不符是常态）。故 [`HostRequest::send_text`] 内部委托给
//! tome-net 的 charset 层，不在本模块重写解码。

use std::sync::Arc;

use tome_core::{ErrorKind, HttpMethod, NetHost, RequestSpec, Response};

use crate::charset;

/// 搜索/抓取的单请求超时（毫秒）。
///
/// 与 `tome_source::search::SEARCH_TIMEOUT_MS` 取同一个值（15 秒）：
/// 两者都是"书源动作"级别的超时，取不同值只会让"为什么这个慢、那个快"
/// 变成一个没有依据的谜。此处**不**依赖 tome-source（那是反向依赖），
/// 故这个数字在两处各写一次——它是**契约值**而非实现细节。
///
/// [WORKAROUND] 若将来它需要可配置，应下沉到 `RequestSpec` 的构造方
/// （管线与脚本桥共用一处），而不是继续在两边各调一次。
pub const SCRIPT_REQUEST_TIMEOUT_MS: u64 = 15_000;

/// 面向书源脚本的请求入口（T032 的接线层）。
///
/// # 为什么持有 `Arc<dyn NetHost>` 而不是具体类型
///
/// 这样同一个接线层既能接真实执行器（[`crate::executor::RequestExecutor`]），
/// 也能接测试替身或回放适配器——三者都只实现 `NetHost`。沙箱侧的测试
/// （T036）正是靠这一点在不联网的前提下验证 `ajax`/`get`/`post` 的行为。
#[derive(Clone)]
pub struct HostRequest {
    host: Arc<dyn NetHost>,
    /// 限速键：通常是发起请求的书源 id。
    ///
    /// FR-008 要求"按源"限速。脚本请求必须与管线请求共享同一个键，
    /// 否则同一个源会被限成两份（见模块文档的落差 1）。
    rate_limit_key: String,
    /// 书源声明的字符集（FR-009）
    declared_charset: Option<String>,
    /// 请求间隔（毫秒，来自书源的 `concurrentRate`）
    request_interval_ms: Option<u32>,
}

impl HostRequest {
    /// 为某个书源构造接线层。
    ///
    /// `rate_limit_key` 传**书源 id**（不是 URL）：见模块文档落差 1。
    pub fn for_source(host: Arc<dyn NetHost>, source_id: impl Into<String>) -> Self {
        Self {
            host,
            rate_limit_key: source_id.into(),
            declared_charset: None,
            request_interval_ms: None,
        }
    }

    /// 带上书源声明的字符集（FR-009）。
    pub fn with_charset(mut self, charset: Option<String>) -> Self {
        self.declared_charset = charset;
        self
    }

    /// 带上书源声明的请求间隔（R5 的 `concurrentRate`）。
    pub fn with_request_interval(mut self, interval_ms: Option<u32>) -> Self {
        self.request_interval_ms = interval_ms;
        self
    }

    /// 当前限速键（诊断用）。
    pub fn rate_limit_key(&self) -> &str {
        &self.rate_limit_key
    }

    /// 构造一次 GET 请求（**不发**）。
    pub fn get(&self, url: impl Into<String>) -> RequestSpec {
        self.spec(HttpMethod::Get, url.into(), None)
    }

    /// 构造一次 POST 请求（**不发**）。
    pub fn post(&self, url: impl Into<String>, body: impl Into<Vec<u8>>) -> RequestSpec {
        self.spec(HttpMethod::Post, url.into(), Some(body.into()))
    }

    /// 构造一次 HEAD 请求（**不发**）。
    pub fn head(&self, url: impl Into<String>) -> RequestSpec {
        self.spec(HttpMethod::Head, url.into(), None)
    }

    /// 构造一次指定方法的请求（**不发**）。
    ///
    /// 补全的三处默认值与管线的取向一致：限速键 = 书源 id、超时 = 15 秒、
    /// 不重试（脚本请求失败应当尽快报给脚本，由脚本的 try/catch 决定降级，
    /// 而不是在这里静默重试三次拖慢整个源）。
    pub fn spec(&self, method: HttpMethod, url: String, body: Option<Vec<u8>>) -> RequestSpec {
        let mut spec = RequestSpec::get(url, self.rate_limit_key.clone());
        spec.method = method;
        spec.body = body;
        spec.charset = self.declared_charset.clone();
        spec.request_interval_ms = self.request_interval_ms;
        spec.timeout_ms = SCRIPT_REQUEST_TIMEOUT_MS;
        spec.retry = tome_core::RetryPolicy::NONE;
        spec
    }

    /// 执行请求，原样返回 `Response`（**非 2xx 也返回 `Ok`**，R3 语义）。
    ///
    /// 对应脚本侧的 `java.ajax`：它把状态码与正文都交给脚本自己判断
    /// （书源的可用性检查脚本正需要读 403 验证页的正文）。
    pub fn send(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
        self.host.fetch(spec)
    }

    /// 执行请求并解出一段文本（**非 2xx 报错**）。
    ///
    /// 对应脚本侧的 `java.get`/`java.post`：它们返回的是正文，
    /// 把 404 的 HTML 页当成"成功的结果"交给脚本会让 `JSON.parse` 抛一个
    /// 与真实原因无关的异常（"Unexpected token <"），排查方向完全被带偏。
    ///
    /// 解码走 charset 层的容错路径（FR-009）——书源声明的 charset 与实际
    /// 不符是常态，硬编 UTF-8 会让一批能用的源产出乱码。
    pub fn send_text(&self, spec: RequestSpec) -> Result<String, ErrorKind> {
        let response = self.host.fetch(spec)?;
        if !response.is_success() {
            // ## 为什么这里拿不到状态码（一个已知的契约局限）
            //
            // `NetHost::fetch` 的签名是 `Result<Response, ErrorKind>`，而
            // `ErrorKind` 是**无载荷的分类枚举**（core 的设计意图："分类是
            // 契约，细节是诊断"）。故状态码无法随错误一起传出接缝——
            // `ErrorKind::Network.with_detail("...").kind()` 这类写法会把
            // detail 直接丢掉（`with_detail` 返回 `ClassifiedError`，
            // `.kind()` 只取出分类）。
            //
            // 于是调用方（脚本桥）只知道"网络类失败"，不知道是 404 还是 403。
            // 这不是本模块能修的：要修得让 `NetHost` 的错误类型能带上下文，
            // 那是 core 接缝的改动。
            //
            // 影响面：脚本的 try/catch 仍能捕获它并降级（FR-006 的目的达成），
            // 只是无法据此区分"源改版"与"需要验证"。排查时仍可经
            // [`Self::send`]（原样返回 `Response`）自己看状态码。
            return Err(ErrorKind::Network);
        }
        Ok(self.decode(&response))
    }

    /// 把响应体解成文本（复用 charset 层的容错解码）。
    pub fn decode(&self, response: &Response) -> String {
        // 声明来源按 HTML 规范的次序：响应头优先，其次书源声明，最后自动检测。
        // 与 executor 的 `attach_charset` 同源——那里已经把结果填进
        // `detected_charset`，故这里优先用它（它是"实际生效的那个"）。
        let declared = response
            .detected_charset
            .clone()
            .or_else(|| self.declared_charset.clone())
            .or_else(|| response.headers.get("content-type").map(str::to_string));
        charset::decode_body(&response.body_bytes, declared.as_deref()).text
    }
}

impl std::fmt::Debug for HostRequest {
    /// 手写 `Debug`（`dyn NetHost` 没有 `Debug`）。
    ///
    /// 只打印诊断需要的字段，不打印宿主——宿主的 `Debug` 可能包含
    /// Cookie 等敏感状态（AGENTS.md 安全红线：禁止在日志记录敏感数据）。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostRequest")
            .field("rate_limit_key", &self.rate_limit_key)
            .field("declared_charset", &self.declared_charset)
            .field("request_interval_ms", &self.request_interval_ms)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tome_core::HeaderMap;

    /// 记录收到的 spec，并按需返回预设响应。
    struct RecordingHost {
        response: Mutex<Option<Response>>,
        seen: Mutex<Vec<RequestSpec>>,
    }

    impl RecordingHost {
        fn new(response: Response) -> Self {
            Self {
                response: Mutex::new(Some(response)),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn seen(&self) -> Vec<RequestSpec> {
            self.seen.lock().expect("锁未被毒化").clone()
        }
    }

    impl NetHost for RecordingHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            self.seen.lock().expect("锁未被毒化").push(spec);
            self.response
                .lock()
                .expect("锁未被毒化")
                .clone()
                .ok_or(ErrorKind::Network)
        }
    }

    fn ok_response(body: &[u8]) -> Response {
        Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: body.to_vec(),
            detected_charset: None,
        }
    }

    fn request_for(host: RecordingHost) -> (HostRequest, Arc<RecordingHost>) {
        let host = Arc::new(host);
        let request = HostRequest::for_source(host.clone(), "https://source.test");
        (request, host)
    }

    #[test]
    fn 构造的_spec_带齐书源定制() {
        let host = Arc::new(RecordingHost::new(ok_response(b"x")));
        let request = HostRequest::for_source(host, "https://source.test")
            .with_charset(Some("gbk".to_string()))
            .with_request_interval(Some(1000));

        let spec = request.get("https://target.test/a");
        assert_eq!(spec.method, HttpMethod::Get);
        assert_eq!(spec.url, "https://target.test/a");
        // 限速键必须是**书源 id**：脚本请求与管线请求共享令牌桶（FR-008 的"按源"）
        assert_eq!(spec.rate_limit_key, "https://source.test");
        assert_eq!(spec.charset.as_deref(), Some("gbk"));
        assert_eq!(spec.request_interval_ms, Some(1000));
        assert_eq!(spec.timeout_ms, SCRIPT_REQUEST_TIMEOUT_MS);
        assert_eq!(
            spec.retry,
            tome_core::RetryPolicy::NONE,
            "脚本请求不重试：失败应尽快报给脚本的 try/catch"
        );
    }

    #[test]
    fn post_带上请求体() {
        let host = Arc::new(RecordingHost::new(ok_response(b"x")));
        let request = HostRequest::for_source(host, "s");
        let spec = request.post("https://target.test/a", b"a=1".to_vec());
        assert_eq!(spec.method, HttpMethod::Post);
        assert_eq!(spec.body.as_deref(), Some(&b"a=1"[..]));
    }

    #[test]
    fn head_不带请求体() {
        let host = Arc::new(RecordingHost::new(ok_response(b"")));
        let request = HostRequest::for_source(host, "s");
        let spec = request.head("https://target.test/a");
        assert_eq!(spec.method, HttpMethod::Head);
        assert!(spec.body.is_none());
    }

    #[test]
    fn send_对非_2xx_原样返回() {
        // R3 的接缝语义：状态判定权在调用方。`java.ajax` 靠这一点读错误页正文。
        let (request, _host) = request_for(RecordingHost::new(Response {
            status: 403,
            headers: HeaderMap::new(),
            body_bytes: "<html>需要验证</html>".as_bytes().to_vec(),
            detected_charset: None,
        }));
        let response = request
            .send(request.get("https://target.test/a"))
            .expect("非 2xx 也应返回 Ok");
        assert_eq!(response.status, 403);
        assert!(String::from_utf8_lossy(&response.body_bytes).contains("需要验证"));
    }

    #[test]
    fn send_text_对非_2xx_报错() {
        // `java.get` 拿的是正文：把 404 页当成结果交给脚本会让 JSON.parse
        // 抛一个与真实原因无关的异常。
        let (request, _host) = request_for(RecordingHost::new(Response {
            status: 404,
            headers: HeaderMap::new(),
            body_bytes: b"<html>not found</html>".to_vec(),
            detected_charset: None,
        }));
        let error = request
            .send_text(request.get("https://target.test/a"))
            .expect_err("404 应报错");
        assert_eq!(error, ErrorKind::Network);

        // 反向对照：`send` 必须能拿到状态码——这是"已知的接缝局限"
        // （错误类型不带载荷）的唯一补偿路径，钉住它以免被顺手改掉。
        let response = request
            .send(request.get("https://target.test/a"))
            .expect("send 对非 2xx 应返回 Ok");
        assert_eq!(response.status, 404, "send 必须保留状态码供调用方判断");
    }

    #[test]
    fn send_text_返回解码后的正文() {
        let (request, host) = request_for(RecordingHost::new(ok_response(b"hello")));
        let text = request
            .send_text(request.get("https://target.test/a"))
            .expect("应成功");
        assert_eq!(text, "hello");
        assert_eq!(host.seen().len(), 1, "应恰好发一次请求");
    }

    #[test]
    fn 传输失败被透传() {
        struct FailingHost;
        impl NetHost for FailingHost {
            fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
                Err(ErrorKind::Proxy)
            }
        }
        let request = HostRequest::for_source(Arc::new(FailingHost), "s");
        let error = request
            .send(request.get("https://target.test/a"))
            .expect_err("应报错");
        assert_eq!(
            error,
            ErrorKind::Proxy,
            "代理故障应透传，不该被压成 Network（上层要据此给不同建议）"
        );
    }

    #[test]
    fn 解码优先用响应实际检测到的字符集() {
        // GBK 字节 + `detected_charset`：应解出中文。
        // 说明：示=CA BE、例=C0 FD。
        let (request, _host) = request_for(RecordingHost::new(Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: vec![0xCA, 0xBE, 0xC0, 0xFD],
            detected_charset: Some("GBK".to_string()),
        }));
        let text = request
            .send_text(request.get("https://target.test/a"))
            .expect("应成功");
        assert_eq!(text, "示例", "应按 GBK 解码");
    }

    #[test]
    fn debug_不泄漏宿主内部() {
        let (request, _host) = request_for(RecordingHost::new(ok_response(b"x")));
        let rendered = format!("{request:?}");
        assert!(rendered.contains("https://source.test"), "{rendered}");
        assert!(rendered.contains("HostRequest"), "{rendered}");
    }
}
