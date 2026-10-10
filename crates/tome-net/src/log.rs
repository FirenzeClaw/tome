//! \[DONE] HTTP 日志管线（tasks.md T064 / FR-017）
//!
//! # 本模块的位置：在 `NetHost` 之外包一层
//!
//! 日志记录**不能**塞进 [`crate::executor::RequestExecutor`] 内部，理由有两条：
//!
//! 1. **依赖方向**：持久化要落 `tome-store` 的 `http_log` 表，而
//!    `tome-net` 依赖 `tome-store` 会让"网络层依赖存储层"——接缝
//!    让 `tome-store` 只依赖 `tome-core`（星型无环），这条依赖会破坏它。
//! 2. **可关闭**：FR-017 要求日志可开关。开关是运行期决定，而执行器是
//!    一次构造的。
//!
//! 故做成 [`LoggingNetHost`]：一个**包装宿主**（装饰器）。它与
//! `AppRoot` 的 `ConfigProxyOverride` 同属一类——都是"在接缝上做一层
//! 横切的事"，而接缝之所以能做这件事，正是因为它是**唯一**的发请求出口
//! （架构决策 A2）。
//!
//! ```text
//!   管线 ──► LoggingNetHost ──► ConfigProxyOverride ──► RequestExecutor
//!              │  记录 + 脱敏                            （真实网络）
//!              ▼
//!         HttpLogSink（tome-store 的 http_log 表）
//! ```
//!
//! # 脱敏是**结构化**的，不是字符串替换
//!
//! 本模块按**头名**判定敏感（`Cookie` / `Authorization` / `Proxy-Authorization`
//! / `Set-Cookie` / `X-Auth-Token` …），把匹配到的头的值整体替换为
//! `[REDACTED]`。**不做**"在整段文本里搜关键词"那种做法——那会漏掉
//! 大小写变体、缩写变体，也会误伤正文里恰好含 "token" 的内容。
//!
//! 同样的规则作用于请求体与响应体里的**表单式**敏感字段
//! （`password=`、`token=`、`access_token=`）。这两类是该脱敏的最常见形态
//! （登录表单、OAuth 回调），而它们的结构是已知的。
//!
//! ## 声明能力边界
//!
//! 本模块**做不到**的事：识别"正文里长得像凭据但不叫 token 的东西"。
//! 那需要模式识别，误报率不可接受。故采取的是**保守策略**——
//! 见 [`RedactionPolicy::response_body_limit`]：默认**不记响应体**，
//! 只记长度。想看正文的用户显式打开，并自行承担。

use std::sync::Arc;

use tome_core::{ErrorKind, HttpMethod, NetHost, RequestSpec, Response};

/// 敏感头的名字（小写比较）。
///
/// # 这份清单的依据
///
/// 都是"值的泄漏直接等于凭据泄漏"的头，来自 RFC 7235（`Authorization`）、
/// RFC 6265（`Cookie` / `Set-Cookie`）、通用实践（`X-Auth-Token` 等）。
///
/// 采用**精确名**而不是"包含 auth 就脱敏"的子串匹配：后者会误伤
/// `Authentication-Info`（本身不含凭据）这类头，而误伤的代价是日志失去
/// 诊断价值——用户看不出"为什么这个头是 [REDACTED]"。
pub const SENSITIVE_HEADERS: &[&str] = &[
    "cookie",
    "set-cookie",
    "authorization",
    "proxy-authorization",
    "www-authenticate",
    "x-auth-token",
    "x-api-key",
    "api-key",
    "apikey",
    "access-token",
    "x-csrf-token",
    "x-xsrf-token",
];

/// 敏感字段的关键词（用于请求体/响应体里的表单式凭据）。
///
/// # 为什么这里用子串匹配而头名用精确匹配
///
/// 表单字段名没有标准：`password` / `passwd` / `pwd` / `userPassword` /
/// `loginPassword` 全都存在。故用**子串**匹配这几个词根。误伤（如
/// `password_hint=请输入密码` 被脱敏）的代价可接受——那是提示文本，
/// 丢失它不影响诊断。
pub const SENSITIVE_FIELD_KEYWORDS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "access_key",
    "private_key",
];

/// 脱敏后的占位文本。
pub const REDACTED: &str = "[REDACTED]";

/// 脱敏策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactionPolicy {
    /// 响应体最多记录多少**字符**；`None` 表示不记响应体。
    ///
    /// # 为什么默认不记
    ///
    /// 响应体是唯一"内容不可预知"的部分——本模块的脱敏对**结构已知**的
    /// 两类（头、表单字段）有效，而正文里可能以任何形态出现凭据。
    /// 默认不记是**保守策略**：宁可少记，不可泄漏。
    ///
    /// 打开它时只记**前 N 个字符**：截断本身也是一道防线（凭据常在
    /// 请求/响应首部）。
    pub response_body_limit: Option<usize>,
    /// 是否记请求体（同样只记前若干字符，且做表单式脱敏）。
    pub request_body_limit: Option<usize>,
}

impl Default for RedactionPolicy {
    fn default() -> Self {
        Self {
            // 默认不记响应体（见字段文档）
            response_body_limit: None,
            // 请求体默认记前 512 字符：它通常是脚本生成的参数
            // （如 `{"page":1}`），对诊断"源为什么失效"价值很高，
            // 而它的结构比响应体可预期（表单式为主）。
            request_body_limit: Some(512),
        }
    }
}

impl RedactionPolicy {
    /// 记响应体（前 `limit` 个字符），供调试"源改版了"这类问题。
    pub fn with_response_body(limit: usize) -> Self {
        Self {
            response_body_limit: Some(limit),
            ..Self::default()
        }
    }
}

/// 日志落点（由组合根注入 `tome-store` 的实现）。
///
/// # 为什么是 trait 而不是直接依赖 `tome-store`
///
/// `tome-net` 依赖 `tome-store` 会破坏"实现 crate 只依赖 `tome-core`"
/// 的星型结构（那会让网络层把存储层拖进它的依赖树）。用一个最小 trait
/// 表达"记一条日志"，让实现侧（组合根）决定落哪张表。
///
/// ## 为什么签名是 `&HttpLogEntry` 而不是散字段
///
/// 条目形状已由 `tome-core` 的 DTO 定义（`data-model.md` 的 18 实体之一），
/// 传散字段会让"加一个字段"要改 trait 的全部实现与调用点。
pub trait HttpLogSink: Send + Sync {
    /// 记一条日志。
    ///
    /// **失败不向上传播**：日志是诊断设施，它写不进去不该让用户的抓取失败。
    /// 故返回 `()`——实现方内部决定怎么处理（通常是忽略并计数）。
    fn record(&self, entry: &tome_core::HttpLogEntry);
}

/// 什么都不做的落点（日志关闭时用）。
///
/// # 为什么不用 `Option<Box<dyn HttpLogSink>>`
///
/// `Option` 会让包装宿主的每次 `fetch` 都要多一个 `if let`，而
/// "不记日志"这个分支本身没有逻辑——用一个空实现把它变成一次虚调用，
/// 路径更直（也更容易在 profile 里看到它的开销确实是零）。
pub struct NullLogSink;

impl HttpLogSink for NullLogSink {
    fn record(&self, _entry: &tome_core::HttpLogEntry) {}
}

/// 带日志的 [`NetHost`] 包装（FR-017）。
///
/// # 记录时机：**请求完成后**才写
///
/// 记在 `fetch` 返回之后而不是之前：日志条目里有响应状态码与耗时，
/// 而那是请求完成后才知道的。代价是"请求过程中崩溃"不会留下日志——
/// 那正是 crash-safe 设计的取向（半条日志没有诊断价值）。
pub struct LoggingNetHost {
    inner: Arc<dyn NetHost>,
    sink: Arc<dyn HttpLogSink>,
    policy: RedactionPolicy,
    /// 是否启用（FR-017 的"可开关"）。
    enabled: bool,
}

// ## 源 id 从哪来
//
// `RequestSpec` 里没有"这是哪个源的请求"的字段，而 `rate_limit_key`
// 按约定就是书源 id（见 `RequestSpec::rate_limit_key` 的字段文档）。
// [`build_entry`] 复用它，让"按源查日志"（`sqlite_log_sink` 的
// `logs_of_source`）有键可用，且不必改接缝。
//
// `[UNCERTAIN]` 若日后有非书源发起的请求（如 RSS 抓取）用了别的键，
// 这条映射会把它记成某个源。那时应给 `RequestSpec` 加显式字段。

impl std::fmt::Debug for LoggingNetHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoggingNetHost")
            .field("enabled", &self.enabled)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl LoggingNetHost {
    /// 包装一个宿主。
    pub fn wrap(inner: Arc<dyn NetHost>, sink: Arc<dyn HttpLogSink>, enabled: bool) -> Self {
        Self {
            inner,
            sink,
            policy: RedactionPolicy::default(),
            enabled,
        }
    }

    /// 指定脱敏策略。
    pub fn with_policy(mut self, policy: RedactionPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// 由配置构造（组合根的接线点）。
    pub fn from_settings(
        inner: Arc<dyn NetHost>,
        sink: Arc<dyn HttpLogSink>,
        enabled: bool,
    ) -> Self {
        Self::wrap(inner, sink, enabled)
    }
}

impl NetHost for LoggingNetHost {
    fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
        // 不记日志时完全不碰 sink，也不读时钟——零开销路径
        if !self.enabled {
            return self.inner.fetch(spec);
        }

        let started = std::time::Instant::now();
        let outcome = self.inner.fetch(spec.clone());
        let duration_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;

        let entry = build_entry(&spec, outcome.as_ref().ok(), duration_ms, &self.policy);
        self.sink.record(&entry);

        outcome
    }
}

/// 组装一条日志条目（**已脱敏**）。
///
/// 独立成函数以便单测：脱敏是本模块的核心关切，它必须能被**单独**
/// 断言——塞在 `fetch` 里就只能端到端测，而那样很难覆盖各类头的组合。
pub fn build_entry(
    spec: &RequestSpec,
    response: Option<&Response>,
    duration_ms: i64,
    policy: &RedactionPolicy,
) -> tome_core::HttpLogEntry {
    tome_core::HttpLogEntry {
        id: 0,
        // `rate_limit_key` 按约定即书源 id（见 `RequestSpec` 的字段文档）
        source_id: if spec.rate_limit_key.is_empty() {
            None
        } else {
            Some(spec.rate_limit_key.clone())
        },
        url: spec.url.clone(),
        method: method_name(spec.method).to_string(),
        request_headers_redacted: redact_headers(spec, policy),
        response_code: response.map(|response| response.status),
        response_body_redacted: response.and_then(|response| redact_body(response, policy)),
        duration_ms: Some(duration_ms),
        created_at: 0,
    }
}

/// HTTP 方法名。
fn method_name(method: HttpMethod) -> &'static str {
    method.as_str()
}

/// 脱敏请求头与请求体（`None` 表示两者都没有可记的）。
///
/// # 为什么头与体在同一个函数里
///
/// 它们的落点是同一个 `request_headers_redacted` 文本列（`data-model.md`
/// 的 `http_log` 只有这一列承载请求侧信息）。分成两个函数会让"两者都空
/// 才返回 `None`"这条判断落在调用方，而调用方还得多一次分配。
///
/// ## 一个曾经的缺陷（本测试捕获）
///
/// 首版在"无头"时直接 `return None`，于是**有请求体但无头**的请求
/// （脚本常见：`java.post(url, body)` 不带自定义头）会丢掉整个请求体。
/// 那不是"少记一点"，而是让一类请求在日志里完全没有内容。
fn redact_headers(spec: &RequestSpec, policy: &RedactionPolicy) -> Option<String> {
    if spec.headers.is_empty() && spec.body.is_none() {
        return None;
    }

    let mut entries: Vec<(String, String)> = Vec::new();
    for (name, value) in spec.headers.iter() {
        let shown = if is_sensitive_header(name) {
            REDACTED.to_string()
        } else {
            value.to_string()
        };
        entries.push((name.to_string(), shown));
    }

    // 用 JSON 而不是 `name: value` 拼接：头值里可能含换行/冒号，
    // 拼出来的文本无法被可靠地解析回结构
    let object: serde_json::Map<String, serde_json::Value> = entries
        .into_iter()
        .map(|(name, value)| (name, serde_json::Value::String(value)))
        .collect();
    let mut text = serde_json::to_string(&object).unwrap_or_else(|_| "{}".to_string());

    // 请求体（同时做表单式脱敏）
    if let (Some(limit), Some(body)) = (policy.request_body_limit, spec.body.as_deref()) {
        let body_text = String::from_utf8_lossy(body);
        let redacted = redact_form_fields(&body_text);
        let truncated = truncate_chars(&redacted, limit);
        text = format!("{text}\n<body>{truncated}</body>");
    }
    Some(text)
}

/// 脱敏响应体（`None` 表示策略决定不记）。
fn redact_body(response: &Response, policy: &RedactionPolicy) -> Option<String> {
    let limit = policy.response_body_limit?;
    // 图片等二进制：记长度而不记内容（记二进制既是垃圾也无诊断价值）
    if response.body_bytes.is_empty() {
        return Some(String::new());
    }
    let text = match std::str::from_utf8(&response.body_bytes) {
        Ok(text) => text.to_string(),
        Err(_) => return Some(format!("[二进制 {} 字节]", response.body_bytes.len())),
    };
    Some(truncate_chars(&redact_form_fields(&text), limit))
}

/// 该头名是否敏感（大小写不敏感）。
pub fn is_sensitive_header(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    SENSITIVE_HEADERS.contains(&lowered.as_str())
}

/// 脱敏 `key=value` 形式的敏感字段（表单体、查询串、JSON 里的字符串值）。
///
/// # 覆盖的三种形态
///
/// | 形态 | 例子 |
/// |---|---|---|
/// | URL 编码表单 | `username=a&password=b` |
/// | JSON | `{"password":"b"}` |
/// | 查询串 | `?token=b` |
///
/// 三者在本处**一视同仁**：本函数不解析结构，而是对每个
/// `关键词 + 分隔符 + 值` 的形态做替换。这样任何看起来像凭据赋值的地方
/// 都被覆盖，而不必为每种格式写一个解析器（写不全才是真正的风险）。
pub fn redact_form_fields(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0usize;

    while cursor < text.len() {
        // 从当前位置找一个敏感关键词的起始
        let Some((keyword_start, keyword)) = find_sensitive_keyword(text, cursor) else {
            // 后面没有敏感词了，原样接上
            result.push_str(&text[cursor..]);
            break;
        };
        // 关键词之前的原样保留
        result.push_str(&text[cursor..keyword_start]);

        let after_keyword = keyword_start + keyword.len();
        // 关键词后面允许有引号（JSON 形态：`"password":`）
        let mut value_start = after_keyword;
        let rest = &text[after_keyword..];
        let mut consumed_prefix = "";
        // 跳过 `"`、`=`、`:`、空白
        for (offset, character) in rest.char_indices() {
            if character == '"' || character == '=' || character == ':' || character.is_whitespace()
            {
                continue;
            }
            value_start = after_keyword + offset;
            consumed_prefix = &text[after_keyword..value_start];
            break;
        }

        result.push_str(keyword);
        result.push_str(consumed_prefix);

        // 找值的结束：`&`、`"`、`,`、`}`、空白、行尾
        let value_end = text[value_start..]
            .find(['&', '"', ',', '}', '\n', '\r', ' ', '\t'])
            .map(|offset| value_start + offset)
            .unwrap_or(text.len());

        if value_end > value_start {
            result.push_str(REDACTED);
        }
        cursor = value_end.max(value_start);
        // 防止在同一个位置打转（值长度为 0 且没有推进）
        if cursor <= keyword_start {
            result.push_str(&text[keyword_start..]);
            break;
        }
    }
    result
}

/// 从 `from` 开始找第一个敏感关键词，返回 `(起始字节位置, 关键词原文)`。
fn find_sensitive_keyword(text: &str, from: usize) -> Option<(usize, &str)> {
    let lowered = text[from..].to_ascii_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for keyword in SENSITIVE_FIELD_KEYWORDS {
        if let Some(offset) = lowered.find(keyword) {
            let absolute = from + offset;
            if best
                .map(|(position, _)| absolute < position)
                .unwrap_or(true)
            {
                best = Some((absolute, &text[absolute..absolute + keyword.len()]));
            }
        }
    }
    best
}

/// 按**字符数**截断（不是字节数）。
///
/// 与 `tome_store::dao::misc` 的同名函数同理：按字节截断会把多字节字符
/// 切成两半，产生非法的 UTF-8。两处各有一份是因为依赖方向不允许
/// `tome-net` 依赖 `tome-store`（见模块文档），而这是三行代码。
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(limit).collect();
    truncated.push_str("…[已截断]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tome_core::{HeaderMap, ProxyOverride};

    /// 假宿主：返回预设响应。
    struct FakeHost {
        status: u16,
        body: Vec<u8>,
        seen: Mutex<Vec<RequestSpec>>,
    }

    impl FakeHost {
        fn ok(body: &str) -> Self {
            Self {
                status: 200,
                body: body.as_bytes().to_vec(),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(spec);
            Ok(Response {
                status: self.status,
                headers: HeaderMap::new(),
                body_bytes: self.body.clone(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    /// 收集日志的落点。
    #[derive(Default)]
    struct CollectSink {
        entries: Mutex<Vec<tome_core::HttpLogEntry>>,
    }

    impl CollectSink {
        fn entries(&self) -> Vec<tome_core::HttpLogEntry> {
            self.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl HttpLogSink for CollectSink {
        fn record(&self, entry: &tome_core::HttpLogEntry) {
            self.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(entry.clone());
        }
    }

    fn spec_with_headers(headers: &[(&str, &str)]) -> RequestSpec {
        let mut spec = RequestSpec::get("https://a.com/s", "https://a.com");
        for (name, value) in headers {
            spec.headers.set(*name, *value);
        }
        spec
    }

    // --- 脱敏：头 ---

    #[test]
    fn cookie_头被脱敏() {
        let spec = spec_with_headers(&[
            ("Cookie", "session=abc123; token=xyz"),
            ("User-Agent", "tome/0.1"),
        ]);
        let entry = build_entry(&spec, None, 10, &RedactionPolicy::default());
        let headers = entry.request_headers_redacted.expect("应记头");
        assert!(
            !headers.contains("abc123"),
            "Cookie 的值不得出现：{headers}"
        );
        assert!(!headers.contains("xyz"));
        assert!(headers.contains(REDACTED), "应留下脱敏标记：{headers}");
        // 非敏感头应原样保留（否则日志失去诊断价值）
        assert!(headers.contains("tome/0.1"), "非敏感头应保留：{headers}");
    }

    #[test]
    fn 各类认证头都被脱敏() {
        for name in [
            "Authorization",
            "Proxy-Authorization",
            "Set-Cookie",
            "X-Auth-Token",
            "X-Api-Key",
            "X-CSRF-Token",
        ] {
            let spec = spec_with_headers(&[(name, "super-secret-value")]);
            let entry = build_entry(&spec, None, 0, &RedactionPolicy::default());
            let headers = entry.request_headers_redacted.expect("应记头");
            assert!(
                !headers.contains("super-secret-value"),
                "`{name}` 的值不得出现：{headers}"
            );
        }
    }

    #[test]
    fn 头名大小写不敏感() {
        for name in ["COOKIE", "cookie", "CoOkIe"] {
            assert!(is_sensitive_header(name), "`{name}` 应被判敏感");
        }
        let spec = spec_with_headers(&[("AUTHORIZATION", "secret")]);
        let entry = build_entry(&spec, None, 0, &RedactionPolicy::default());
        assert!(!entry
            .request_headers_redacted
            .expect("应记头")
            .contains("secret"));
    }

    #[test]
    fn 非敏感头不被误伤() {
        for name in ["Content-Type", "Accept", "Authentication-Info", "Referer"] {
            let spec = spec_with_headers(&[(name, "public-value")]);
            let entry = build_entry(&spec, None, 0, &RedactionPolicy::default());
            let headers = entry.request_headers_redacted.expect("应记头");
            assert!(
                headers.contains("public-value"),
                "`{name}` 不该被脱敏（误伤会让日志失去诊断价值）：{headers}"
            );
        }
    }

    #[test]
    fn 无头时记_none() {
        let spec = RequestSpec::get("https://a.com/s", "https://a.com");
        let entry = build_entry(&spec, None, 0, &RedactionPolicy::default());
        assert!(entry.request_headers_redacted.is_none());
    }

    // --- 脱敏：表单/JSON 字段 ---

    #[test]
    fn url_表单式凭据被脱敏() {
        let text = "username=alice&password=hunter2&remember=1";
        let redacted = redact_form_fields(text);
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(
            redacted.contains("username=alice"),
            "非敏感字段应保留：{redacted}"
        );
        assert!(redacted.contains("remember=1"));
    }

    #[test]
    fn json_式凭据被脱敏() {
        let text = r#"{"user":"alice","password":"hunter2","token":"abc"}"#;
        let redacted = redact_form_fields(text);
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(!redacted.contains("abc"), "token 的值应被脱敏：{redacted}");
        assert!(redacted.contains("alice"), "非敏感值应保留：{redacted}");
    }

    #[test]
    fn 查询串里的凭据被脱敏() {
        let text = "https://a.com/cb?code=x&access_token=secret-value&page=2";
        let redacted = redact_form_fields(text);
        assert!(!redacted.contains("secret-value"), "{redacted}");
        assert!(redacted.contains("page=2"));
    }

    #[test]
    fn 多种敏感词根都覆盖() {
        for keyword in [
            "password",
            "passwd",
            "pwd",
            "secret",
            "apikey",
            "access_key",
        ] {
            let text = format!("{keyword}=leaked-value&x=1");
            let redacted = redact_form_fields(&text);
            assert!(
                !redacted.contains("leaked-value"),
                "`{keyword}` 的值应被脱敏，实际 {redacted}"
            );
        }
    }

    #[test]
    fn 无敏感字段时原样返回() {
        let text = "name=alice&age=30";
        assert_eq!(redact_form_fields(text), text);
    }

    #[test]
    fn 脱敏不产生非法_utf8() {
        let text = "密码=秘密&password=中文密码值&x=1";
        let redacted = redact_form_fields(text);
        assert!(!redacted.contains('\u{FFFD}'), "{redacted}");
        assert!(!redacted.contains("中文密码值"), "{redacted}");
    }

    // --- 请求/响应体 ---

    #[test]
    fn 请求体默认记前_512_字符且脱敏() {
        let mut spec = RequestSpec::get("https://a.com/s", "https://a.com");
        spec.body = Some(b"user=alice&password=hunter2".to_vec());
        let entry = build_entry(&spec, None, 0, &RedactionPolicy::default());
        let headers = entry.request_headers_redacted.expect("应记头");
        assert!(headers.contains("alice"), "应记请求体：{headers}");
        assert!(!headers.contains("hunter2"), "请求体也要脱敏：{headers}");
    }

    #[test]
    fn 响应体默认不记() {
        // 保守策略：响应体内容不可预知，默认不记
        let response = Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: b"secret in body".to_vec(),
            detected_charset: None,
        };
        let entry = build_entry(
            &RequestSpec::get("https://a.com", "s"),
            Some(&response),
            0,
            &RedactionPolicy::default(),
        );
        assert!(
            entry.response_body_redacted.is_none(),
            "默认策略下不该记响应体"
        );
    }

    #[test]
    fn 显式打开后响应体被记录并截断() {
        let policy = RedactionPolicy::with_response_body(10);
        let response = Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: "0123456789abcdefghij".as_bytes().to_vec(),
            detected_charset: None,
        };
        let entry = build_entry(
            &RequestSpec::get("https://a.com", "s"),
            Some(&response),
            0,
            &policy,
        );
        let body = entry.response_body_redacted.expect("应记响应体");
        assert_eq!(body, "0123456789…[已截断]", "应按字符截断并标记");
    }

    #[test]
    fn 二进制响应体只记长度() {
        let policy = RedactionPolicy::with_response_body(1000);
        let response = Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00],
            detected_charset: None,
        };
        let entry = build_entry(
            &RequestSpec::get("https://a.com", "s"),
            Some(&response),
            0,
            &policy,
        );
        let body = entry.response_body_redacted.expect("应记");
        assert!(body.contains("二进制"), "{body}");
        assert!(body.contains('5'), "应记字节数：{body}");
    }

    // --- 包装宿主的行为 ---

    #[test]
    fn 包装后请求原样透传() {
        let host = Arc::new(FakeHost::ok("<html/>"));
        let sink = Arc::new(CollectSink::default());
        let logging = LoggingNetHost::wrap(host.clone(), sink.clone(), true);

        let spec = spec_with_headers(&[("Accept", "text/html")]);
        let response = logging.fetch(spec.clone()).expect("应成功");
        assert_eq!(response.status, 200);

        // 内层收到的是同一个 spec（包装不改请求语义）
        let seen = host
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, spec.url);
        assert_eq!(seen[0].headers.get("Accept"), Some("text/html"));
    }

    #[test]
    fn 记录源_id_方法与耗时() {
        let host = Arc::new(FakeHost::ok("ok"));
        let sink = Arc::new(CollectSink::default());
        let logging = LoggingNetHost::wrap(host, sink.clone(), true);

        let mut spec = RequestSpec::get("https://a.com/s", "https://source-x.com");
        spec.method = HttpMethod::Post;
        logging.fetch(spec).expect("应成功");

        let entries = sink.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].source_id.as_deref(),
            Some("https://source-x.com")
        );
        assert_eq!(entries[0].method, "POST");
        assert!(entries[0].duration_ms.is_some(), "应记耗时");
        assert_eq!(entries[0].response_code, Some(200));
    }

    #[test]
    fn 关闭时不记也不碰_sink() {
        let host = Arc::new(FakeHost::ok("ok"));
        let sink = Arc::new(CollectSink::default());
        let logging = LoggingNetHost::wrap(host, sink.clone(), false);

        logging
            .fetch(RequestSpec::get("https://a.com", "s"))
            .expect("应成功");
        assert!(sink.entries().is_empty(), "关闭时不该记日志");
    }

    #[test]
    fn 请求失败也记一条_状态码为_none() {
        struct FailingHost;
        impl NetHost for FailingHost {
            fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
                Err(ErrorKind::Network)
            }
        }
        let sink = Arc::new(CollectSink::default());
        let logging = LoggingNetHost::wrap(Arc::new(FailingHost), sink.clone(), true);

        let outcome = logging.fetch(RequestSpec::get("https://a.com", "s"));
        assert!(outcome.is_err(), "错误应向上传播");

        // 失败也要留下痕迹——否则"源为什么连不上"没有任何可查的线索
        let entries = sink.entries();
        assert_eq!(entries.len(), 1, "失败的请求也该记");
        assert_eq!(entries[0].response_code, None, "没有响应就没有状态码");
    }

    #[test]
    fn 空_rate_limit_key_时不记源_id() {
        let host = Arc::new(FakeHost::ok("ok"));
        let sink = Arc::new(CollectSink::default());
        let logging = LoggingNetHost::wrap(host, sink.clone(), true);
        logging
            .fetch(RequestSpec::get("https://a.com", ""))
            .expect("应成功");
        assert_eq!(sink.entries()[0].source_id, None);
    }

    #[test]
    fn 空实现是安全的空操作() {
        // 日志关闭时用的落点：调用它必须是安全的空操作
        NullLogSink.record(&tome_core::HttpLogEntry {
            id: 0,
            source_id: None,
            url: "u".to_string(),
            method: "GET".to_string(),
            request_headers_redacted: None,
            response_code: None,
            response_body_redacted: None,
            duration_ms: None,
            created_at: 0,
        });
    }

    #[test]
    fn 代理覆盖字段不受影响() {
        // 脱敏只碰头，不改请求语义的其他部分
        let mut spec = RequestSpec::get("https://a.com", "s");
        spec.proxy_override = Some(ProxyOverride::Direct);
        let entry = build_entry(&spec, None, 0, &RedactionPolicy::default());
        assert_eq!(entry.url, "https://a.com");
        assert_eq!(entry.method, "GET");
    }
}
