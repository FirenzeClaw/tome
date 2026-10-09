//! 代理策略：跟随系统代理 + 按源覆盖（tasks.md T017 / FR-008）
//!
//! FR-008 要求"跟随系统代理，以及**按源单独指定**代理"。这两件事在实现上分成两层：
//!
//! 1. **本模块（纯决策层）**：[`resolve`] 把「按源覆盖 + 客户端默认策略」归约成
//!    三态 [`ProxyRoute`]。它是纯函数，不碰网络、不读环境，因此可被无限次断言。
//! 2. **client 层（副作用层）**：把 [`ProxyRoute`] 翻译成 `reqwest::Client`——
//!    直连用 `ClientBuilder::no_proxy()`，指定代理用 `Proxy::all(addr)`，
//!    跟随系统则什么都不设（reqwest 的 `system-proxy` 特性自带探测）。
//!
//! # 为什么决策与翻译要分开
//!
//! "本次请求走不走代理"是**可测的语义**，"怎么让 reqwest 走代理"是**库的细节**。
//! 混在一起后，代理逻辑就只能靠"真的发一次请求看它连到哪儿"来验证——那既慢又脆弱。
//! 拆开后，三态归约的全部组合都是普通单元测试。
//!
//! # 系统代理探测的归属
//!
//! 探测由 reqwest 完成（`system-proxy` 特性 → hyper-util 的
//! `hyper_util::client::proxy::matcher`）。它按 curl 的规则检查
//! `ALL_PROXY` / `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` 环境变量，
//! 在 Windows/macOS 上还会回退到系统网络设置。本 crate **不重复实现探测**：
//! 两份探测逻辑迟早会不一致，而探测结果直接影响能否联网，不一致的代价是
//! "测试环境能通、真实环境不能通"这类最难查的问题。

use tome_core::ProxyOverride;

use crate::error::NetError;

/// 客户端的**默认**代理策略（未给出按源覆盖时生效）。
///
/// 为什么需要这个显式开关：真实世界里"某些源在代理下反而不可达"很常见，
/// 用户需要一个"整体关掉代理"的开关，而 FR-008 只规定了"跟随系统"与
/// "按源指定"。把默认策略做成配置项，等于让"整体直连"也能表达，
/// 且不必给每个源都写一条覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DefaultProxyPolicy {
    /// 跟随系统代理（FR-008 的默认行为）
    #[default]
    FollowSystem,
    /// 默认直连，忽略系统代理设置
    Direct,
}

/// 一次请求实际采用的代理路线（决策结果，可断言）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyRoute {
    /// 明确直连：忽略系统代理设置
    Direct,
    /// 使用指定代理，地址已规范化为可交给 reqwest 的 URL
    Custom {
        /// 规范化后的代理地址（如 `http://127.0.0.1:7897`）
        endpoint: String,
    },
    /// 跟随系统代理
    System,
}

impl ProxyRoute {
    /// 本路线对应的 `reqwest::Proxy` 构造依据（用于日志与错误归因）。
    pub fn describe(&self) -> String {
        match self {
            Self::Direct => "direct".to_string(),
            Self::Custom { endpoint } => format!("custom {endpoint}"),
            Self::System => "system".to_string(),
        }
    }

    /// 本路线**是否确定**经由代理。
    ///
    /// - [`Self::Direct`] → `Some(false)`：明确直连，不存在代理段。
    /// - [`Self::Custom`] → `Some(true)`：我们亲手配置的代理，必然经过它。
    /// - [`Self::System`] → [`system_proxy_endpoint_hint`] 的结果：环境变量里能看到
    ///   代理地址时是 `Some(true)`；看不到时为 `None`（**未知**，不是"没有"）。
    ///
    /// 返回 `None` 的语义很重要：它让调用方知道"这里我给不出结论"，
    /// 从而选择保守的处理而不是假装知道。
    ///
    /// # 已知局限：系统代理走注册表时归因不到
    ///
    /// [WORKAROUND] Windows 用户多数只配系统设置（注册表 `Internet Settings`）
    /// 而不设环境变量。此时本函数返回 `None`，于是"[`Self::System`] 路线下代理
    /// 不可达"会被归成 [`crate::NetError::Connect`]（目标站故障）而非
    /// [`crate::NetError::Proxy`]——FR-008 的"两者可区分"在这种配置下**只在
    /// 显式覆盖（[`Self::Custom`]）时成立**。
    ///
    /// 要补齐这一点，需要读取注册表（引入 Windows 专属依赖）或直接依赖
    /// hyper-util 的 matcher 去查询探测结果。两条路都要为一个**错误归因**
    /// 引入新依赖，而错误分类本身已在 `ErrorKind` 层面受限（缺 Proxy 类目）。
    /// 故此处选择如实返回 `None`、把局限写清楚，而不是猜一个值——
    /// 猜错会把"目标站不通"误导成"改你的代理设置"，比不知道更糟。
    pub fn proxy_certainty(&self) -> Option<bool> {
        match self {
            Self::Direct => Some(false),
            Self::Custom { .. } => Some(true),
            Self::System => system_proxy_endpoint_hint().map(|_| true),
        }
    }

    /// 本路线确实经过代理时才给出路线描述（供错误归因）。
    pub fn proxy_route_if_in_use(&self) -> Option<String> {
        match self.proxy_certainty() {
            Some(true) => Some(self.describe()),
            _ => None,
        }
    }
}

/// 归约代理路线（纯函数，FR-008）。
///
/// 归约规则：
///
/// | 按源覆盖 | 客户端默认 | 结果 |
/// |---|---|---|
/// | `Some(Direct)` | 任意 | [`ProxyRoute::Direct`] |
/// | `Some(Custom(a))` | 任意 | [`ProxyRoute::Custom`]（`a` 规范化后） |
/// | `None` | `FollowSystem` | [`ProxyRoute::System`] |
/// | `None` | `Direct` | [`ProxyRoute::Direct`] |
///
/// **按源覆盖优先于默认策略**，包括"默认直连但某源指定了代理"——"按源指定"
/// 是更具体的一层意图，具体应压过笼统。
///
/// # 为什么 `Custom` 的地址要在这里规范化
///
/// 早失败：地址写错（少了端口、带了非法 scheme）应当在**决策层**当场报错，
/// 而不是等到构造客户端或发出请求时才炸。这样校验点只有一处，
/// 且能在没有网络的环境下被测试覆盖。
pub fn resolve(
    proxy_override: Option<&ProxyOverride>,
    default_policy: DefaultProxyPolicy,
) -> Result<ProxyRoute, NetError> {
    match proxy_override {
        Some(ProxyOverride::Direct) => Ok(ProxyRoute::Direct),
        Some(ProxyOverride::Custom(address)) => Ok(ProxyRoute::Custom {
            endpoint: normalize_proxy_endpoint(address)?,
        }),
        None => match default_policy {
            DefaultProxyPolicy::FollowSystem => Ok(ProxyRoute::System),
            DefaultProxyPolicy::Direct => Ok(ProxyRoute::Direct),
        },
    }
}

/// 规范化用户/书源给出的代理地址。
///
/// 接受两种写法：
///
/// - 带 scheme：`http://127.0.0.1:7897`、`socks5://127.0.0.1:1080`
/// - 不带 scheme：`127.0.0.1:7897` → 补 `http://`
///
/// 补 `http://` 而非 `https://`：代理地址的 scheme 描述的是"如何连接代理"，
/// 绝大多数本地/内网代理是明文 HTTP CONNECT（对目标站仍是 HTTPS 加密）。
/// 补成 `https://` 会让所有这类代理握手失败——这是 reqwest 自身
/// （`IntoProxy` 的 fallback）与 curl 的共同选择，此处置对齐。
///
/// 校验：必须有主机名与端口。没有端口的代理地址是无意义的（代理不像目标站
/// 有默认端口语义），早报错优于让 reqwest 在连接阶段给出含糊的失败。
fn normalize_proxy_endpoint(address: &str) -> Result<String, NetError> {
    let trimmed = address.trim();
    if trimmed.is_empty() {
        return Err(NetError::Url {
            url: address.to_string(),
            reason: "代理地址为空".to_string(),
        });
    }

    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };

    let parsed = reqwest::Url::parse(&candidate).map_err(|error| NetError::Url {
        url: address.to_string(),
        reason: format!("代理地址无法解析: {error}"),
    })?;

    let scheme = parsed.scheme();
    if !matches!(
        scheme,
        "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
    ) {
        return Err(NetError::Url {
            url: address.to_string(),
            reason: format!("不支持的代理协议 {scheme}"),
        });
    }

    if parsed.host_str().is_none() {
        return Err(NetError::Url {
            url: address.to_string(),
            reason: "代理地址缺少主机名".to_string(),
        });
    }

    if parsed.port().is_none() {
        return Err(NetError::Url {
            url: address.to_string(),
            reason: "代理地址缺少端口".to_string(),
        });
    }

    Ok(parsed.to_string())
}

/// 从环境变量里读出系统代理地址（尽力而为的**提示**）。
///
/// # 这为什么是"提示"而不是"事实"
///
/// reqwest 探测系统代理走两条路：环境变量，以及 Windows/macOS 的系统网络设置。
/// 后者（如 Windows 注册表 `Internet Settings\ProxyServer`）**无法**在没有额外
/// 依赖的前提下读取——本 crate 不为一个提示信息引入注册表 crate。
///
/// 因此：
///
/// - 环境变量里能看到代理 → 结论可靠（`Some(addr)`）。
/// - 看不到 → **不能**推断"没有代理"（Windows 用户多数只配系统设置、不设环境变量）。
///
/// 这正是返回值用 `Option` 而非 `bool` 的原因：调用方必须显式处理"不知道"，
/// 而不是把一个假的"没有代理"当真。见 [`ProxyRoute::proxy_certainty`]。
///
/// [WORKAROUND] 完整的系统代理探测需要 hyper-util 的 matcher（传递依赖，
/// 不可直接 use）。若日后 ErrorKind 增加 Proxy 类目或本 crate 获准直接依赖
/// hyper-util，此函数应替换为对探测结果的读取。
pub fn system_proxy_endpoint_hint() -> Option<String> {
    [
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ]
    .iter()
    .find_map(|name| std::env::var(name).ok())
    .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_direct_beats_default_follow_system() {
        let route = resolve(
            Some(&ProxyOverride::Direct),
            DefaultProxyPolicy::FollowSystem,
        );
        assert_eq!(route, Ok(ProxyRoute::Direct));
    }

    #[test]
    fn override_direct_beats_default_direct() {
        let route = resolve(Some(&ProxyOverride::Direct), DefaultProxyPolicy::Direct);
        assert_eq!(route, Ok(ProxyRoute::Direct));
    }

    #[test]
    fn absent_override_follows_default() {
        assert_eq!(
            resolve(None, DefaultProxyPolicy::FollowSystem),
            Ok(ProxyRoute::System)
        );
        assert_eq!(
            resolve(None, DefaultProxyPolicy::Direct),
            Ok(ProxyRoute::Direct)
        );
    }

    #[test]
    fn custom_override_beats_default_direct() {
        // 默认直连，但某源明确指定了代理：更具体的一层意图胜出。
        let route = resolve(
            Some(&ProxyOverride::Custom("127.0.0.1:7897".to_string())),
            DefaultProxyPolicy::Direct,
        );
        assert_eq!(
            route,
            Ok(ProxyRoute::Custom {
                endpoint: "http://127.0.0.1:7897/".to_string()
            })
        );
    }

    #[test]
    fn custom_endpoint_without_scheme_gets_http() {
        let route = resolve(
            Some(&ProxyOverride::Custom("127.0.0.1:7897".to_string())),
            DefaultProxyPolicy::FollowSystem,
        )
        .expect("应能规范化");
        let ProxyRoute::Custom { endpoint } = route else {
            panic!("应归约为自定义代理");
        };
        assert!(endpoint.starts_with("http://"), "实际为 {endpoint}");
    }

    #[test]
    fn custom_endpoint_keeps_explicit_scheme() {
        let route = resolve(
            Some(&ProxyOverride::Custom("socks5://10.0.0.1:1080".to_string())),
            DefaultProxyPolicy::FollowSystem,
        )
        .expect("应能规范化");
        let ProxyRoute::Custom { endpoint } = route else {
            panic!("应归约为自定义代理");
        };
        assert!(endpoint.starts_with("socks5://"), "实际为 {endpoint}");
    }

    #[test]
    fn custom_endpoint_without_port_is_rejected() {
        let route = resolve(
            Some(&ProxyOverride::Custom("127.0.0.1".to_string())),
            DefaultProxyPolicy::FollowSystem,
        );
        assert!(matches!(route, Err(NetError::Url { .. })));
        assert_eq!(
            route.expect_err("应报错").to_error_kind(),
            tome_core::ErrorKind::Configuration
        );
    }

    #[test]
    fn custom_endpoint_with_bad_scheme_is_rejected() {
        let route = resolve(
            Some(&ProxyOverride::Custom("ftp://127.0.0.1:21".to_string())),
            DefaultProxyPolicy::FollowSystem,
        );
        assert!(matches!(route, Err(NetError::Url { .. })));
    }

    #[test]
    fn custom_endpoint_empty_is_rejected() {
        let route = resolve(
            Some(&ProxyOverride::Custom("   ".to_string())),
            DefaultProxyPolicy::FollowSystem,
        );
        assert!(matches!(route, Err(NetError::Url { .. })));
    }

    #[test]
    fn certainty_is_definite_for_direct_and_custom() {
        assert_eq!(ProxyRoute::Direct.proxy_certainty(), Some(false));
        assert_eq!(
            ProxyRoute::Custom {
                endpoint: "http://127.0.0.1:7897/".to_string()
            }
            .proxy_certainty(),
            Some(true)
        );
        // Direct 与 Custom 都不依赖环境，故 is_proxy_in_use 有确定答案。
        assert_eq!(ProxyRoute::Direct.proxy_route_if_in_use(), None);
        assert_eq!(
            ProxyRoute::Custom {
                endpoint: "http://127.0.0.1:7897/".to_string()
            }
            .proxy_route_if_in_use(),
            Some("custom http://127.0.0.1:7897/".to_string())
        );
    }

    #[test]
    fn describe_is_stable() {
        assert_eq!(ProxyRoute::Direct.describe(), "direct");
        assert_eq!(ProxyRoute::System.describe(), "system");
        assert_eq!(
            ProxyRoute::Custom {
                endpoint: "http://a:1/".to_string()
            }
            .describe(),
            "custom http://a:1/"
        );
    }
}
