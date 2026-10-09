//! 宿主能力 trait 与共享值对象（tasks.md T012，架构审查 A1 / A2）
//!
//! # 为什么这些定义在 core，而不在 tome-js
//!
//! `plan.md`/`seams.md` 初稿曾写"tome-js 定义宿主 trait"。那会构成**循环依赖**：
//! `tome-net` 要实现 `NetHost` → 必须依赖 `tome-js`；而 `NetHost::fetch` 的参数
//! 是 `RequestSpec` → `tome-js` 又得被 `tome-net` 依赖。双向锁定。
//!
//! 放在 core（接缝中枢）则一切实现 crate 只依赖 core，**星型无环**：
//!
//! ```text
//!        tome-core  (RequestSpec / Response / NetHost / StoreHost / CryptoHost)
//!         ▲    ▲    ▲
//!         │    │    └──────── tome-js     （被组合根注入 NetHost/StoreHost 实现）
//!         │    └───────────── tome-store  （实现 StoreHost）
//!         └────────────────── tome-net    （实现 NetHost）
//! ```
//!
//! # A2 规则引擎纯化
//!
//! [`RequestSpec`] 是**规则求值的产物**：规则引擎求出"要请求什么"，
//! 但**不亲自发请求**；管线（tome-source）是唯一发请求处，它把 `RequestSpec`
//! 交给 tome-net 执行。这带来两个直接收益：
//!
//! 1. **离线可重放**：录制 `RequestSpec` → `Response` 的映射即可完全离线重放，
//!    差分测试（宪法 III 的一致性验证）由此确定。
//! 2. **网络策略集中**：限速、代理、重试、Cookie 只在 tome-net 一处生效，
//!    规则引擎无从绕过。

use crate::error::ErrorKind;

/// HTTP 方法。
///
/// 用枚举而非 `String`：书源规则里的方法取值只有这几种，枚举让非法值
/// 在解析边界当场暴露，而不是发出去才失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    /// GET
    Get,
    /// POST
    Post,
    /// PUT
    Put,
    /// DELETE
    Delete,
    /// HEAD
    Head,
}

impl HttpMethod {
    /// 转为 HTTP 请求行里的大写方法名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
        }
    }

    /// 从字符串解析（大小写不敏感）。未知方法返回 `None`。
    pub fn from_str_ignore_case(value: &str) -> Option<Self> {
        match value.to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "POST" => Some(Self::Post),
            "PUT" => Some(Self::Put),
            "DELETE" => Some(Self::Delete),
            "HEAD" => Some(Self::Head),
            _ => None,
        }
    }
}

/// 请求头集合。
///
/// ## 为什么不用 `HashMap`
///
/// HTTP 头**允许重名**（如多个 `Set-Cookie`、`Accept`），且顺序在某些场景
/// 有意义。`HashMap` 会丢重复项，也能被外部顺序扰动。
///
/// ## 为什么不在 core 里存大小写规范化
///
/// HTTP 头名大小写不敏感，但各实现 crate 的处理策略（是否保留原始大小写用于
/// 日志）不同。此处保留原始名，由实现方决定规范化时机——core 不做这种取舍。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderMap {
    entries: Vec<(String, String)>,
}

impl HeaderMap {
    /// 空集合。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一个头。**不覆盖**同名项（HTTP 允许多值）。
    pub fn append(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.entries.push((name.into(), value.into()));
    }

    /// 设置一个头：移除既有同名项后追加。
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        self.entries
            .retain(|(existing, _)| !existing.eq_ignore_ascii_case(&name));
        self.entries.push((name, value.into()));
    }

    /// 取第一个同名头的值（大小写不敏感）。
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// 全部头（保序，含重名）。
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 头数量（含重名）。
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// 代理配置覆盖（FR-008：跟随系统代理，以及**按源单独指定**代理）。
///
/// 三态是必须的：`None`（跟随系统）与 `Some(Direct)`（明确不走代理）语义不同
/// ——后者用于"某源在代理下反而不可达"的场景。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyOverride {
    /// 显式直连，**忽略**系统代理设置
    Direct,
    /// 使用指定代理地址
    Custom(String),
}

/// 重试策略（FR-008：书源可声明重试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// 最大重试次数（**不含**首次尝试）
    pub max_retries: u32,
    /// 首次退避毫秒数（实现方按指数退避放大）
    pub backoff_ms: u64,
}

impl RetryPolicy {
    /// 不重试。
    pub const NONE: Self = Self {
        max_retries: 0,
        backoff_ms: 0,
    };

    /// 构造一个策略。
    pub const fn new(max_retries: u32, backoff_ms: u64) -> Self {
        Self {
            max_retries,
            backoff_ms,
        }
    }
}

/// 一次 HTTP 请求的完整描述（FR-008 全字段）。
///
/// # 这是规则引擎与网络层之间的唯一契约
///
/// 规则引擎产出它（不亲自发请求），tome-net 执行它，tome-js 的宿主函数转发它。
/// 三方都只引用本类型，故它**必须**包含全部请求定制维度（FR-008）：
/// 方法、URL、头、体、编码、Cookie、超时、重试、请求间隔、代理。
///
/// # 为什么 `rate_limit_key` 是独立的键而非用 URL
///
/// FR-008 要求"按源"限速。同一源的不同 URL 必须共享同一个令牌桶，而不同源的
/// URL 必须互相隔离。用 URL 做键会把限速粒度搞错（同源不同路径各限一份，
/// 反而更容易触发对方风控）。故由产生 spec 的一方显式给出键（通常是书源 id）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestSpec {
    /// 请求方法
    pub method: HttpMethod,
    /// 完整 URL（实现方须校验 scheme 为 https，安全红线）
    pub url: String,
    /// 请求头
    pub headers: HeaderMap,
    /// 请求体（GET 通常为 `None`）
    pub body: Option<Vec<u8>>,
    /// 声明的字符集（FR-009：声明与实际不符时须容错解码）
    pub charset: Option<String>,
    /// 代理覆盖（`None` = 跟随系统代理）
    pub proxy_override: Option<ProxyOverride>,
    /// 限速键：同键共享一个令牌桶，不同键互不影响（FR-008）
    pub rate_limit_key: String,
    /// 超时毫秒数
    pub timeout_ms: u64,
    /// 重试策略
    pub retry: RetryPolicy,
}

impl RequestSpec {
    /// 以最小必要字段构造（GET + 无头 + 默认超时不重试）。
    ///
    /// 提供它不是为了便利，而是为了让"默认值"只有一处定义——
    /// 书源规则里大量请求只给出 URL，这些默认值决定了它们的行为。
    pub fn get(url: impl Into<String>, rate_limit_key: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            url: url.into(),
            headers: HeaderMap::new(),
            body: None,
            charset: None,
            proxy_override: None,
            rate_limit_key: rate_limit_key.into(),
            timeout_ms: DEFAULT_TIMEOUT_MS,
            retry: RetryPolicy::NONE,
        }
    }
}

/// 默认超时：30 秒。
///
/// [WORKAROUND] 上游移动端默认更短，但桌面场景下慢速站点更常见，30s 是
/// "不误判超时"与"用户不至于等太久"的折中。若实测发现某类站点普遍超时，
/// 应改为可配置项而非继续调整此常量。
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// 一次 HTTP 响应的完整描述。
///
/// `body_bytes` 保持**字节**形态：解码是 charset 层的关切（FR-009），
/// 且正文可能是二进制（图片/压缩）。解码后的文本由实现方按需产出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// 状态码
    pub status: u16,
    /// 响应头
    pub headers: HeaderMap,
    /// 响应体原始字节
    pub body_bytes: Vec<u8>,
    /// 实际检测到的字符集（FR-009：可能与声明不同）
    pub detected_charset: Option<String>,
}

impl Response {
    /// 状态码是否表示成功（2xx）。
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// 响应体大小（字节）。
    pub fn body_len(&self) -> usize {
        self.body_bytes.len()
    }
}

/// 网络能力（由 `tome-net` 实现，经组合根注入给 JS 沙箱，A1）。
///
/// ## 为什么是同步方法
///
/// 书源脚本是同步语义（research.md R1）。宿主函数在 JS 侧同步阻塞期间，
/// 该 runtime 的锁被持有——故契约要求"每个书源脚本用独立 runtime"，
/// 由组合根保证，本 trait 无需感知。
pub trait NetHost: Send + Sync {
    /// 执行一次请求。**这是全系统唯一的发请求出口**（A2 纯化）。
    fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind>;
}

/// 存储能力（由 `tome-store` 实现，经组合根注入给 JS 沙箱，A1）。
///
/// 书源脚本的缓存读写、文件读写（**限数据目录内**）走这里。
/// 路径越界须拒绝并报告（spec 边缘情况 / AGENTS.md 安全红线）。
pub trait StoreHost: Send + Sync {
    /// 读缓存（脚本的 `cache.get`）。
    fn cache_get(&self, key: &str) -> Result<Option<Vec<u8>>, ErrorKind>;
    /// 写缓存（脚本的 `cache.put`）。
    fn cache_put(&self, key: &str, value: &[u8]) -> Result<(), ErrorKind>;

    /// 读文件。`relative_path` **必须**在数据目录内，越界须返回
    /// [`ErrorKind::UnsupportedCapability`]（可被书源捕获）。
    fn read_file(&self, relative_path: &str) -> Result<Vec<u8>, ErrorKind>;
    /// 写文件。约束同 [`Self::read_file`]。
    fn write_file(&self, relative_path: &str, contents: &[u8]) -> Result<(), ErrorKind>;
}

/// 加密与摘要能力（**纯计算**，tome-js 内建，不需外部实现）。
///
/// ## 为什么单独成 trait 而不并入 StoreHost
///
/// 它是**纯函数**（无 I/O、无状态），故不需要组合根注入——tome-js 自己实现。
/// 单独成 trait 让"这个能力不依赖宿主"这件事在类型层面显式可见，
/// 也便于测试时直接调用。
///
/// FR-005 的能力清单里"摘要与加密"涵盖 MD5/SHA 系列、AES/DES/RSA。
pub trait CryptoHost: Send + Sync {
    /// 计算摘要。`algorithm` 取 `md5` / `sha1` / `sha256` / `sha512` 等。
    fn digest(&self, algorithm: &str, data: &[u8]) -> Result<Vec<u8>, ErrorKind>;

    /// Base64 编码。
    fn base64_encode(&self, data: &[u8]) -> String;

    /// Base64 解码。非法输入返回 [`ErrorKind::Content`]。
    fn base64_decode(&self, encoded: &str) -> Result<Vec<u8>, ErrorKind>;
}
