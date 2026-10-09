//! tome-rule 自身的错误类型与「不支持能力」的结构化表达（FR-006）
//!
//! # 为什么需要它，而不是直接用 `ErrorKind`
//!
//! AGENTS.md 的约定是"每个 crate 拥有自己的错误类型（thiserror）；在接缝处映射为
//! tome-core 的小型 `ErrorKind` 分类枚举"。本 crate 的错误需要携带**解析位置**
//! （第几字节处选择器语法错了）与环境细节，这些都不该进 core（core 的变更不应
//! 波及全员，宪法原则 IV）。
//!
//! # 为什么「不支持能力」单独有类型
//!
//! FR-006 要求不支持的能力抛**可被书源自身错误处理捕获的结构化错误**，
//! 携带能力标识。这与"出错了"有本质区别：它不是失败信号，而是"此路不通，请改道"
//! 的可编程信号——书源的 `try/catch` 降级逻辑靠它判断**是哪一项**能力不可用。
//! 因此 [`UnsupportedCapability`] 是一个独立类型（携带能力 id 与受影响动作），
//! 而不是一个字符串字段。
//!
//! S1-1 阶段 `@json:` / `@XPath:` / `@js:` 三类规则都走这条路径：分派器能识别它们，
//! 求值器尚未就绪，故返回结构化「未就绪」错误。这样书源在 S1-1 就已能正确降级。

use std::fmt;

use tome_core::{ErrorCapability, ErrorKind};

/// 引擎明确不支持的能力，携带能力标识与受影响动作（FR-006）。
///
/// ## 能力标识取值
///
/// 用书源生态里的**通用叫法**（如 `jsonPath`、`xpath`、`js`、`webView`），
/// 因为这些标识会被书源自己的 `try/catch` 逻辑判断——用本项目自造的命名会让
/// 既有书源认不出来，违背兼容性优先（宪法原则 I）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedCapability {
    /// 能力标识（书源生态通用叫法）
    capability: String,
    /// 受该能力影响的具体动作
    action: String,
}

impl UnsupportedCapability {
    /// 构造一个"能力不可用"信号。
    ///
    /// `capability` 用生态通用叫法；`action` 说明是哪个动作受影响
    /// （如 `rule`、`search`、`content`），便于上层告知用户影响范围。
    pub fn new(capability: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            capability: capability.into(),
            action: action.into(),
        }
    }

    /// 能力标识。
    pub fn capability(&self) -> &str {
        &self.capability
    }

    /// 受影响的动作。
    pub fn action(&self) -> &str {
        &self.action
    }

    /// 转成 tome-core 的错误分类，供跨 crate 边界传播。
    pub fn to_error_kind(&self) -> ErrorKind {
        ErrorKind::unsupported_capability(ErrorCapability::new(self.capability.clone()))
    }
}

impl fmt::Display for UnsupportedCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}", self.capability, self.action)
    }
}

/// tome-rule 的错误。
///
/// 变体刻意保持少而语义明确：分类是接缝处的关切（[`RuleError::to_error_kind`]），
/// 细节是本 crate 的关切。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleError {
    /// 规则语法错误（选择器解析失败、组合符不成对等）。
    #[error("规则语法错误：{detail}")]
    Syntax {
        /// 诊断细节（**不含**面向用户的自然语言文案，文案映射留给上层，FR-044）
        detail: String,
    },

    /// 引擎明确不支持的能力（FR-006）。
    #[error("不支持的能力：{0}")]
    Unsupported(UnsupportedCapability),

    /// 正则表达式编译或执行失败。
    #[error("正则错误：{detail}")]
    Regex {
        /// 诊断细节
        detail: String,
    },

    /// 输入超出引擎的处理上界（正则回溯保护、DOM 深度上界等）。
    #[error("超出处理上界：{detail}")]
    Limit {
        /// 诊断细节
        detail: String,
    },
}

impl RuleError {
    /// 便捷构造语法错误。
    pub fn syntax(detail: impl Into<String>) -> Self {
        Self::Syntax {
            detail: detail.into(),
        }
    }

    /// 便捷构造正则错误。
    pub fn regex(detail: impl Into<String>) -> Self {
        Self::Regex {
            detail: detail.into(),
        }
    }

    /// 便捷构造上界错误。
    pub fn limit(detail: impl Into<String>) -> Self {
        Self::Limit {
            detail: detail.into(),
        }
    }

    /// 便捷构造"能力不可用"。
    pub fn unsupported(capability: impl Into<String>, action: impl Into<String>) -> Self {
        Self::Unsupported(UnsupportedCapability::new(capability, action))
    }

    /// 映射到 tome-core 的错误分类（接缝处唯一出口，AGENTS.md 的 crate 错误约定）。
    pub fn to_error_kind(&self) -> ErrorKind {
        match self {
            Self::Syntax { .. } => ErrorKind::Rule,
            Self::Regex { .. } => ErrorKind::Rule,
            Self::Unsupported(capability) => capability.to_error_kind(),
            Self::Limit { .. } => ErrorKind::Rule,
        }
    }
}

impl From<RuleError> for ErrorKind {
    fn from(error: RuleError) -> Self {
        error.to_error_kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 不支持能力映射到结构化错误分类() {
        let error = RuleError::unsupported("jsonPath", "rule");
        let kind = error.to_error_kind();
        match &kind {
            ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "jsonPath");
            }
            other => panic!("应映射为 UnsupportedCapability，实际 {other:?}"),
        }
        // 可本地化标识由 core 给出，引擎内无自然语言文案（FR-044）
        assert_eq!(kind.message_id(), "error.unsupported_capability");
    }

    #[test]
    fn 语法与正则错误映射到规则类() {
        assert_eq!(RuleError::syntax("x").to_error_kind(), ErrorKind::Rule);
        assert_eq!(RuleError::regex("x").to_error_kind(), ErrorKind::Rule);
        assert_eq!(RuleError::limit("x").to_error_kind(), ErrorKind::Rule);
    }

    #[test]
    fn 错误可经from转换为错误分类() {
        let kind: ErrorKind = RuleError::syntax("bad selector").into();
        assert_eq!(kind, ErrorKind::Rule);
    }
}
