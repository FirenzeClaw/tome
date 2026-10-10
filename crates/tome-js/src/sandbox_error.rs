//! \[DONE] 宿主函数侧的 JS 异常类型（FR-006）
//!
//! # 为什么它与 [`crate::runtime::SandboxError`] 是两个类型
//!
//! 两者回答的问题不同：
//!
//! | 类型 | 回答的问题 | 谁看到它 |
//! |---|---|---|
//! | [`crate::runtime::SandboxError`] | "这次求值怎么了" | Rust 侧调用方（管线） |
//! | [`JsError`] | "这个宿主函数怎么了" | JS 侧的 `catch` |
//!
//! 前者是**结果**（求值结束后的状态），后者是**信号**（求值过程中的一个事件）。
//! 合并会让"脚本捕获了这个错误并继续跑"与"脚本因为这错误而终止"变成
//! 同一个值——而它们在调用方看来完全不同。
//!
//! # 能力错误的形状
//!
//! FR-006 要求不支持的能力抛**结构化可捕获错误**，携带能力标识。
//! 书源侧据此判断"是本产品做不到"（该降级）而不是"这次失败了"（该重试）。
//! 故 [`JsError::unsupported`] 构造出的异常对象带三个额外属性：
//!
//! ```js
//! catch (e) {
//!     if (e.capability === "webView") { /* 换纯 HTTP 路径 */ }
//! }
//! ```
//!
//! 三个属性（`capability` / `isUnsupportedCapability` / `name`）都要有：
//! 不同的书源用不同的判据写降级逻辑，只给一个会让一部分源认不出来。
//! 这与 `ErrorKind::UnsupportedCapability` 携带 `capability_id` 是同一条
//! 兼容性考虑在 JS 侧的落点。

use tome_core::{ErrorCapability, ErrorKind};

/// 宿主函数抛给 JS 的错误。
///
/// # 为什么能力标识与消息分开存
///
/// 消息是**给人看的**（会随措辞变化），能力标识是**给代码判断的**
/// （书源的 `catch` 里比对它，必须稳定）。合并成一个字符串会让措辞调整
/// 变成破坏性变更。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct JsError {
    /// 能力标识（`None` 表示这是一次普通失败，不是"能力不可用"）
    capability: Option<String>,
    /// 面向开发者的诊断消息（**不含**面向用户的本自然语言文案，见 FR-044）
    message: String,
}

impl JsError {
    /// 普通失败（脚本可以重试或降级，但性质上不是"能力缺失"）。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            capability: None,
            message: message.into(),
        }
    }

    /// 能力不可用（FR-006）。
    ///
    /// `capability` 用**书源生态的通用叫法**（`webView` / `startBrowserAwait` /
    /// `network`…）——自造命名会让既有书源认不出来（兼容性优先）。
    pub fn unsupported(capability: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            capability: Some(capability.into()),
            message: message.into(),
        }
    }

    /// 从宿主接缝的错误分类构造（把 `ErrorKind` 转成可抛给 JS 的异常）。
    ///
    /// ## 分类 → 异常形状的映射
    ///
    /// | `ErrorKind` | 异常性质 |
    /// |---|---|
    /// | `UnsupportedCapability` | **能力错误**（带 `capability` 属性） |
    /// | 其余 | 普通失败 |
    ///
    /// 映射只在这里发生一处，理由与"接缝处映射为 `ErrorKind`"同源：
    /// 转换点越少，越不容易出现"某个路径忘了带 capability"的偏差。
    pub fn from_error_kind(kind: &ErrorKind) -> Self {
        match kind {
            ErrorKind::UnsupportedCapability { capability } => {
                Self::unsupported(capability.capability_id().to_string(), kind.message_id())
            }
            other => Self::new(other.message_id()),
        }
    }

    /// 能力标识（`None` = 普通失败）。
    pub fn capability(&self) -> Option<&str> {
        self.capability.as_deref()
    }

    /// 诊断消息。
    pub fn message(&self) -> String {
        match &self.capability {
            // 能力错误的文案带上能力名：排查时"哪个能力不可用"是首要信息，
            // 让它出现在消息里省掉一层翻找。
            Some(capability) => format!("{}（能力：{capability}）", self.message),
            None => self.message.clone(),
        }
    }

    /// 原始消息（不带能力后缀，供需要精确比对的场合）。
    pub fn raw_message(&self) -> &str {
        &self.message
    }

    /// 是否为能力错误。
    pub fn is_unsupported(&self) -> bool {
        self.capability.is_some()
    }

    /// 映射回 `tome_core::ErrorKind`（接缝处出口）。
    pub fn to_error_kind(&self) -> ErrorKind {
        match &self.capability {
            Some(capability) => {
                ErrorKind::unsupported_capability(ErrorCapability::new(capability.clone()))
            }
            None => ErrorKind::Rule,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 普通失败不带能力标识() {
        let error = JsError::new("网络超时");
        assert_eq!(error.capability(), None);
        assert!(!error.is_unsupported());
        assert_eq!(error.message(), "网络超时");
        assert_eq!(error.to_error_kind(), ErrorKind::Rule);
    }

    #[test]
    fn 能力错误携带标识且消息含能力名() {
        let error = JsError::unsupported("webView", "本产品不提供内嵌浏览器");
        assert_eq!(error.capability(), Some("webView"));
        assert!(error.is_unsupported());
        assert_eq!(error.raw_message(), "本产品不提供内嵌浏览器");
        assert!(
            error.message().contains("webView"),
            "消息应带上能力名便于排查：{}",
            error.message()
        );
    }

    #[test]
    fn 从_error_kind_转换保能力标识() {
        // FR-006 的关键路径：接缝的 `ErrorKind` 必须一路带能力标识到 JS。
        let kind = ErrorKind::unsupported_capability(ErrorCapability::new("startBrowserAwait"));
        let error = JsError::from_error_kind(&kind);
        assert_eq!(error.capability(), Some("startBrowserAwait"));
        assert!(error.is_unsupported());

        // 普通分类不带能力标识
        let error = JsError::from_error_kind(&ErrorKind::Network);
        assert_eq!(error.capability(), None);
        assert_eq!(error.raw_message(), "error.network");
    }

    #[test]
    fn 能力标识往返不丢() {
        for capability in [
            "webView",
            "startBrowserAwait",
            "openVideoPlayer",
            "getVerificationCode",
        ] {
            let error = JsError::unsupported(capability, "x");
            match error.to_error_kind() {
                ErrorKind::UnsupportedCapability { capability: actual } => {
                    assert_eq!(actual.capability_id(), capability);
                }
                other => panic!("应保持能力分类，实际 {other:?}"),
            }
        }
    }
}
