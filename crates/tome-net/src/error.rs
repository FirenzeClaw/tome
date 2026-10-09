//! [DONE] tome-net 的错误类型与接缝映射（tasks.md T013 基础件）
//!
//! # 为什么本 crate 自带错误类型
//!
//! AGENTS.md 要求"每个 crate 拥有自己的错误类型（thiserror），在接缝处映射为
//! tome-core 的小型 [`ErrorKind`] 分类枚举"。本模块的 [`NetError`] 就是网络层
//! 自己的错误类型，[`NetError::to_error_kind`] 是它与接缝之间**唯一**的转换点。
//!
//! # 为什么 `Display` 输出"标识 + 字段"而不是中文句子
//!
//! FR-044 / SC-016 要求引擎产出可本地化的**标识**、文案映射留给上层
//! （AGENTS.md 生产级护栏："引擎内禁止硬编码面向用户的自然语言"）。
//! 故本类型的 `Display` 是给日志与开发者看的诊断串，形如
//! `error.network.timeout url=... timeout_ms=...`：既可 grep，也不承担本地化责任。
//! `reason` 字段承载底层库（reqwest/rustls/io）的原始英文说明，属诊断信息。

use std::time::Duration;

use tome_core::error::ErrorKind;

/// `Retry-After` 的上限：超过它一律忽略。
///
/// `Retry-After` 是不可信输入（第三方响应），一个荒唐的值（如 `86400`）
/// 能让调用方停摆一天。上限取 60 秒——足够容纳正常的限流退避，
/// 又不至于让用户以为程序卡死。
///
/// 定义在 `error` 模块是因为它约束的是 [`NetError::Status::retry_after`]
/// 字段的取值范围（[`crate::executor`] 的解析函数据此钳制）。
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// 网络层错误。
///
/// 变体划分的依据是"**上层能据此做什么**"，而不是"底层库报了什么"——
/// 这正是 [`Self::to_error_kind`] 能把多种变体压回七类 [`ErrorKind`] 的前提。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetError {
    /// URL 非法或 scheme 不被允许（安全红线：外部通信一律 https，仅回环允许 http）
    #[error("error.configuration.url url={url} reason={reason}")]
    Url {
        /// 出问题的 URL 原文
        url: String,
        /// 诊断说明
        reason: String,
    },

    /// 请求描述无法构造（请求头名/头值非法等）。
    ///
    /// 归为规则类而非配置类：`RequestSpec` 由规则引擎产出，构造失败即规则给出的值非法。
    #[error("error.rule.request reason={reason}")]
    Request {
        /// 诊断说明
        reason: String,
    },

    /// 单请求超时（含连接、传输、读体任一阶段）
    #[error("error.network.timeout url={url} timeout_ms={timeout_ms}")]
    Timeout {
        /// 请求 URL
        url: String,
        /// `RequestSpec.timeout_ms` 给定的预算
        timeout_ms: u64,
    },

    /// 目标站不可达：TCP 连接失败、DNS 解析失败、连接被重置
    #[error("error.network.connect url={url} reason={reason}")]
    Connect {
        /// 请求 URL
        url: String,
        /// 诊断说明
        reason: String,
    },

    /// TLS 握手或证书校验失败。
    ///
    /// [UNCERTAIN] reqwest/rustls 未暴露结构化的 TLS 错误分类，本变体靠遍历
    /// `source()` 链后匹配证书/握手关键字判定（见 [`Self::from_reqwest`]）。
    /// 误判的代价有限：`Tls` 与 `Connect` 都映射到 [`ErrorKind::Network`]，
    /// 上层分支不变，只是日志细节不同。
    #[error("error.network.tls url={url} reason={reason}")]
    Tls {
        /// 请求 URL
        url: String,
        /// 诊断说明
        reason: String,
    },

    /// 代理不可达或代理握手失败。
    #[error("error.configuration.proxy reason={reason} route={route} url={url}")]
    Proxy {
        /// 请求 URL
        url: String,
        /// 本次采用的代理路线描述
        route: String,
        /// 诊断说明
        reason: String,
    },

    /// 重定向次数超限或进入重定向环
    #[error("error.network.redirect url={url} reason={reason}")]
    Redirect {
        /// 请求 URL
        url: String,
        /// 诊断说明
        reason: String,
    },

    /// 非 2xx 状态码（4xx/5xx）。
    ///
    /// ## 触发范围（R3 后收窄）
    ///
    /// **接缝（[`NetHost::fetch`]）不再触发本变体**——它把"事务完成但状态非
    /// 2xx"作为 `Ok(Response)` 返回，因为接缝的调用方（书源脚本）需要读错误页
    /// 正文，且"状态码算不算失败"是调用方的判断（R3，PM 仲裁 2026-10-10）。
    ///
    /// 本变体保留给**异步 API 的严格模式**：[`ExecutionPolicy::fail_on_status`]
    /// 为 `true`（默认）时，[`RequestExecutor::execute`] 这条取内容路径仍按
    /// "拿不到内容就是失败"处理，并借本变体携带 `retry_after` 供重试循环降速。
    ///
    /// `retry_after` 承载服务端在 `Retry-After` 头里声明的等待时长——
    /// 它是站点把"该等多久"明说出来的唯一渠道，重试循环据此降速而不是按
    /// 自己的节奏硬撞（spec 边缘情况）。
    #[error("error.network.status status={status} url={url} retry_after_ms={retry_after:?}")]
    Status {
        /// HTTP 状态码
        status: u16,
        /// 请求 URL
        url: String,
        /// 服务端要求的重试等待时长（`Retry-After`，已被 [`MAX_RETRY_AFTER`] 钳过）
        retry_after: Option<Duration>,
    },

    /// 传输层 I/O 失败：读体中断、响应体解码失败、本地文件读写失败
    #[error("error.network.io reason={reason}")]
    Io {
        /// 诊断说明
        reason: String,
    },

    /// 网络栈初始化失败：TLS 后端/证书库加载、作业运行时创建。
    ///
    /// 与本机环境相关（缺证书库、线程资源耗尽），与目标站无关，
    /// 故归配置类：换站点、加重试都不会好，要改的是本机环境。
    #[error("error.configuration.setup what={what} reason={reason}")]
    Setup {
        /// 失败的部件
        what: String,
        /// 诊断说明
        reason: String,
    },

    /// 阻塞外观被在异步执行器上下文内调用。
    ///
    /// `Runtime::block_on` 在同线程已有 runtime 时会 panic，生产路径禁 panic
    /// （AGENTS.md），故本层提前检测并返回结构化错误。调用方的正确做法是把
    /// 阻塞调用移到专用线程（`spawn_blocking`）。
    #[error("error.network.blocking_context reason={reason}")]
    BlockingContext {
        /// 诊断说明
        reason: String,
    },

    /// 回放未命中：录制语料里没有该请求指纹对应的响应。
    #[error("error.configuration.replay_miss url={url} fingerprint={fingerprint}")]
    ReplayMiss {
        /// 请求 URL
        url: String,
        /// 规范化后的请求指纹
        fingerprint: String,
    },

    /// 本地资产损坏：回放记录或 Cookie 持久化文件无法解析。
    ///
    /// 与 [`Self::ReplayMiss`] 一样归配置类——两者都是"本次运行所依赖的
    /// 本地数据资产不完整/不可用"，重试与换站点都无济于事。
    #[error("error.configuration.corrupt asset={asset} reason={reason}")]
    Corrupt {
        /// 损坏资产的名字（如 `cookie-jar`、`replay:<指纹>`）
        asset: String,
        /// 诊断说明
        reason: String,
    },
}

impl NetError {
    /// 把 reqwest 的错误归类为 [`NetError`]。
    ///
    /// # 判定次序（顺序即优先级，不可调换）
    ///
    /// 1. `is_timeout()` —— **必须先判**。reqwest 把超时错误构造成
    ///    `Kind::Request`（见其 `error::request(TimedOut)`），与"请求发送失败"
    ///    共用同一个 kind。若先判 `is_request()`，所有超时都会被吞掉——
    ///    一个纯粹的网络超时会被报成"书源规则有问题"，排查方向完全跑偏。
    ///    `is_timeout()` 会遍历 `source()` 链去找 `TimedOut` 标记，因此能在
    ///    这个共用 kind 上把它摘出来。
    /// 2. `is_builder()` —— **只有它**代表"请求描述本身构造不出来"
    ///    （非法的头名/头值、不支持的 scheme 等）。归 [`NetError::Request`]
    ///    → `ErrorKind::Rule`，因为 `RequestSpec` 由规则引擎产出。
    /// 3. `is_connect()` —— 连接类错误，按代理判据分流（见下）。
    /// 4. `is_redirect()`。
    /// 5. 其余（含 `is_request()` 代表的"请求已构造但发送/接收失败"）→
    ///    [`NetError::Io`]。
    ///
    /// `is_request()` 为什么**不**归 `Request`：reqwest 在
    /// `hyper_service.call(req).await` 失败时用它包装错误，此时请求已经构造好了，
    /// 失败发生在传输阶段（连接被对端重置、写请求时断链等）。把它归成
    /// "规则有问题"会让调用方去读书源规则，而真正的原因是网络。
    ///
    /// # 代理故障判据（FR-008 边缘情况：代理故障与目标站故障可区分）
    ///
    /// reqwest **没有**任何 API 能回答"这次错误是代理造成的吗"——它只暴露
    /// `is_connect()`/`is_timeout()`/`is_dns()` 这类"连接类错误"的粗判。
    /// 故本层采用的判据是：
    ///
    /// > **本次请求确实配置了代理**（由调用点按 [`crate::proxy::ProxyRoute`] 判定，
    /// > 见 [`crate::proxy::ProxyRoute::proxy_certainty`]）**且** reqwest 报的是连接类错误
    /// > （`is_connect()`），则归为代理故障。
    ///
    /// 依据：配置了代理时，HTTP 客户端到目标站之间**不存在**直连路径——
    /// 连接阶段的失败必然发生在"本机 → 代理"这一段，归因代理在逻辑上是确定的，
    /// 不是猜测。反之未配置代理时不存在代理段，`is_connect()` 必然是目标站的问题。
    ///
    /// 刻意**不**把 `is_timeout()` 归给代理：reqwest 的超时是覆盖"连接 + 传输 +
    /// 读体"的总预算，超时可能发生在拿到连接之后的读体阶段，把它记成"代理坏了"
    /// 会误导排查方向。这与上面连接类错误的确定性形成对比。
    ///
    /// `proxy_route` 由调用点传入而非在此重算：只有调用点知道本次用的是
    /// 直连客户端、系统代理客户端还是按源覆盖的代理客户端；传
    /// `Some(路线描述)` 表示"本次确定经由代理"。
    pub fn from_reqwest(
        error: reqwest::Error,
        requested_url: &str,
        timeout_ms: u64,
        proxy_route: Option<&str>,
    ) -> Self {
        let url = requested_url.to_string();
        let reason = describe_error_chain(&error);

        if error.is_timeout() {
            return Self::Timeout { url, timeout_ms };
        }
        if error.is_builder() {
            return Self::Request { reason };
        }
        if error.is_connect() {
            if let Some(route) = proxy_route {
                return Self::Proxy {
                    url,
                    route: route.to_string(),
                    reason,
                };
            }
            if looks_like_tls_failure(&error) {
                return Self::Tls { url, reason };
            }
            return Self::Connect { url, reason };
        }
        if error.is_redirect() {
            return Self::Redirect { url, reason };
        }
        Self::Io { reason }
    }

    /// 本错误是否值得重试（传输层判据，不含状态码判定）。
    ///
    /// `Status` 不在列表里：4xx 与 5xx 的可重试性相反，须由执行器按状态码
    /// 分别判定（见 [`crate::executor::is_retryable_status`]），不能在这里一刀切。
    ///
    /// `Proxy` 也不在列表里：代理不可达属"配置不对"，重试一百次也是同一个结局，
    /// 应当把失败尽快交回上层去改配置。
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Timeout { .. } | Self::Connect { .. } | Self::Io { .. }
        )
    }

    /// 服务端要求的重试等待时长（仅 [`Self::Status`] 可能携带）。
    ///
    /// 重试循环用它优先于自身退避策略——站点把"该等多久"明说了，照做就是
    /// 遵守它声明的请求间隔（宪法原则 V），也是避免"重试到被封"的唯一办法。
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Status { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// 映射为接缝层的错误分类（唯一转换点）。
    ///
    /// | 本层 | 接缝 | 理由 |
    /// |---|---|---|
    /// | `Url` / `Setup` | `Configuration` | 本机配置问题（接入点地址、数据目录等） |
    /// | `Proxy` | `Proxy` | 代理链路问题——与目标站故障**必须**分类可分（FR-008） |
    /// | `ReplayMiss` / `Corrupt` | `Configuration` | 本地资产没准备好（录制语料缺失/损坏） |
    /// | `Request` | `Rule` | 请求描述来自规则引擎，构造失败即规则给出的值非法 |
    /// | `Timeout` / `Connect` / `Tls` / `Redirect` / `Status` / `Io` / `BlockingContext` | `Network` | 取数没成功 |
    pub fn to_error_kind(&self) -> ErrorKind {
        match self {
            Self::Request { .. } => ErrorKind::Rule,

            // 代理故障走专用的 `ErrorKind::Proxy`（2026-10-10 PM 仲裁新增）。
            // FR-008 的边缘情况要求它与目标站故障可区分，而"可区分"必须在
            // **分类层**成立——借 `Configuration` 承载会让它与"非法 URL"
            // "回放未命中"同类，上层要细分只能下探诊断细节。
            Self::Proxy { .. } => ErrorKind::Proxy,

            // 回放语料是"本次运行所依赖的本地配置资产"，缺失即配置不完整，
            // 故归 `Configuration`。与代理故障不是一回事：一个是"资产没准备"，
            // 一个是"链路走不通"。
            Self::ReplayMiss { .. } => ErrorKind::Configuration,
            Self::Corrupt { .. } => ErrorKind::Configuration,

            Self::Url { .. } => ErrorKind::Configuration,
            Self::Setup { .. } => ErrorKind::Configuration,

            Self::Timeout { .. }
            | Self::Connect { .. }
            | Self::Tls { .. }
            | Self::Redirect { .. }
            | Self::Status { .. }
            | Self::Io { .. }
            | Self::BlockingContext { .. } => ErrorKind::Network,
        }
    }

    /// 本错误涉及的 URL（有则给出），供上层做按源统计或日志归并。
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Url { url, .. }
            | Self::Timeout { url, .. }
            | Self::Connect { url, .. }
            | Self::Tls { url, .. }
            | Self::Proxy { url, .. }
            | Self::Redirect { url, .. }
            | Self::Status { url, .. }
            | Self::ReplayMiss { url, .. } => Some(url.as_str()),
            Self::Request { .. }
            | Self::Io { .. }
            | Self::Setup { .. }
            | Self::BlockingContext { .. }
            | Self::Corrupt { .. } => None,
        }
    }
}

/// 拼接整条错误链，得到尽量完整的诊断串。
///
/// reqwest 的 `Display` 只有最外层一句话（如 `error sending request`），
/// 真正有用的信息（"connection refused"、"invalid peer certificate"）在
/// `source()` 链的更深处。日志只有拿到整条链才可诊断。
fn describe_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut cursor: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(item) = cursor {
        parts.push(item.to_string());
        cursor = item.source();
    }
    parts.join(" <- ")
}

/// 公开的错误链描述（供 `client` 等在自行构造 `NetError` 变体时复用）。
///
/// 存在的意义是把"如何从 reqwest 错误里榨出可诊断信息"这条知识集中在一处——
/// 分散写会让不同调用点的诊断质量参差不齐。
pub fn describe_reqwest(error: &reqwest::Error) -> String {
    describe_error_chain(error)
}

/// TLS 失败关键字。用于在连接类错误里把 TLS 握手失败摘出来。
///
/// 为什么用关键字而不用类型 downcast：rustls 是**传递依赖**（经 reqwest），
/// 本 crate 不能直接 `use` 它的类型做 downcast，而为一个错误分类引入直接依赖
/// 不划算（体积纪律）。关键字匹配在这里是可接受的降级——判错的后果只是
/// `Tls` 与 `Connect` 互换，两者同属 [`ErrorKind::Network`]，上层行为一致。
const TLS_FAILURE_MARKERS: [&str; 4] = ["certificate", "tls", "handshake", "ssl"];

/// 连接类错误是否由 TLS 握手/证书校验引起。
fn looks_like_tls_failure(error: &reqwest::Error) -> bool {
    let text = describe_error_chain(error).to_ascii_lowercase();
    TLS_FAILURE_MARKERS
        .iter()
        .any(|marker| text.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url_error() -> NetError {
        NetError::Url {
            url: "http://example.com/".to_string(),
            reason: "外部通信一律 https".to_string(),
        }
    }

    #[test]
    fn url_and_setup_map_to_configuration() {
        assert_eq!(url_error().to_error_kind(), ErrorKind::Configuration);
        assert_eq!(
            NetError::Setup {
                what: "tls".to_string(),
                reason: "no root certs".to_string(),
            }
            .to_error_kind(),
            ErrorKind::Configuration
        );
    }

    #[test]
    fn request_error_maps_to_rule() {
        assert_eq!(
            NetError::Request {
                reason: "invalid header value".to_string(),
            }
            .to_error_kind(),
            ErrorKind::Rule
        );
    }

    #[test]
    fn transport_errors_map_to_network() {
        let samples = [
            NetError::Timeout {
                url: "https://a/".to_string(),
                timeout_ms: 30_000,
            },
            NetError::Connect {
                url: "https://a/".to_string(),
                reason: "connection refused".to_string(),
            },
            NetError::Tls {
                url: "https://a/".to_string(),
                reason: "invalid peer certificate".to_string(),
            },
            NetError::Redirect {
                url: "https://a/".to_string(),
                reason: "too many redirects".to_string(),
            },
            NetError::Status {
                status: 503,
                url: "https://a/".to_string(),
                retry_after: None,
            },
            NetError::Io {
                reason: "body read interrupted".to_string(),
            },
            NetError::BlockingContext {
                reason: "called from async context".to_string(),
            },
        ];
        for sample in samples {
            assert_eq!(
                sample.to_error_kind(),
                ErrorKind::Network,
                "不应归为网络错: {sample}"
            );
        }
    }

    /// 代理故障与目标站故障在接缝处必须**可区分**（FR-008 边缘情况）。
    #[test]
    fn proxy_failure_is_distinguishable_from_target_failure() {
        let proxy = NetError::Proxy {
            url: "https://target.example/".to_string(),
            route: "custom http://127.0.0.1:1".to_string(),
            reason: "connection refused".to_string(),
        };
        let target = NetError::Connect {
            url: "https://target.example/".to_string(),
            reason: "connection refused".to_string(),
        };
        assert_ne!(proxy.to_error_kind(), target.to_error_kind());
        assert_eq!(target.to_error_kind(), ErrorKind::Network);
    }

    /// 三类在接缝处必须**互不相同**：代理故障、目标站故障、本机配置错。
    ///
    /// 背景：R1 修订前 `ErrorKind` 只有六类，代理故障借 `Configuration` 承载，
    /// 于是它与"非法 URL""回放未命中"在接缝上同类——上层要区分就只能下探诊断细节。
    /// 新增 `ErrorKind::Proxy` 后三者必须在分类层就分开。
    #[test]
    fn proxy_network_and_configuration_are_three_distinct_kinds() {
        let proxy = NetError::Proxy {
            url: "https://target.example/".to_string(),
            route: "custom http://127.0.0.1:1".to_string(),
            reason: "connection refused".to_string(),
        };
        let target = NetError::Connect {
            url: "https://target.example/".to_string(),
            reason: "connection refused".to_string(),
        };
        let configuration = NetError::Url {
            url: "http://insecure.example/".to_string(),
            reason: "only https allowed".to_string(),
        };

        let proxy_kind = proxy.to_error_kind();
        let target_kind = target.to_error_kind();
        let configuration_kind = configuration.to_error_kind();

        assert_ne!(proxy_kind, target_kind, "代理故障 ≠ 目标站故障");
        assert_ne!(proxy_kind, configuration_kind, "代理故障 ≠ 本机配置错");
        assert_ne!(target_kind, configuration_kind, "目标站故障 ≠ 本机配置错");
    }

    #[test]
    fn replay_assets_map_to_configuration() {
        assert_eq!(
            NetError::ReplayMiss {
                url: "https://a/".to_string(),
                fingerprint: "00ff".to_string(),
            }
            .to_error_kind(),
            ErrorKind::Configuration
        );
        assert_eq!(
            NetError::Corrupt {
                asset: "cookie-jar".to_string(),
                reason: "bad line".to_string(),
            }
            .to_error_kind(),
            ErrorKind::Configuration
        );
    }

    #[test]
    fn retryability_excludes_status_and_proxy() {
        assert!(NetError::Timeout {
            url: "https://a/".to_string(),
            timeout_ms: 1
        }
        .is_retryable());
        assert!(NetError::Connect {
            url: "https://a/".to_string(),
            reason: "refused".to_string()
        }
        .is_retryable());
        assert!(!NetError::Proxy {
            url: "https://a/".to_string(),
            route: "custom".to_string(),
            reason: "refused".to_string()
        }
        .is_retryable());
        assert!(!NetError::Status {
            status: 503,
            url: "https://a/".to_string(),
            retry_after: None,
        }
        .is_retryable());
    }

    #[test]
    fn display_carries_identifier_not_prose() {
        let rendered = NetError::Status {
            status: 404,
            url: "https://a/b".to_string(),
            retry_after: None,
        }
        .to_string();
        assert!(rendered.starts_with("error.network.status"));
        assert!(rendered.contains("status=404"));
    }

    #[test]
    fn url_accessor_covers_url_bearing_variants() {
        assert_eq!(url_error().url(), Some("http://example.com/"));
        assert_eq!(
            NetError::Io {
                reason: "x".to_string()
            }
            .url(),
            None
        );
    }
}
