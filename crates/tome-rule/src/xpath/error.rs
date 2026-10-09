//! [DONE] XPath 子集的错误类型
//!
//! # 为什么在 [`crate::error::RuleError`] 之外再造一层
//!
//! `RuleError` 是**对外契约**：书源管线与 CLI 按它的分类决定行为。
//! 而 XPath 的解析与求值内部有大量"这里必须是一个数字"式的局部失败，
//! 它们全部映射到同一个 `RuleError::Syntax`，区分它们对上层没有价值。
//!
//! 独立一层的好处是：解析器与求值器都不必携带 `capability`/`action`
//! 上下文（那是模块边界的关切），且内部错误天然携带**源码字节位置**。
//! 与 `RuleError` 的转换只有 [ `XPathError` → `RuleError` ] 一个方向，
//! 由 `xpath/mod.rs` 的三个公开入口完成。

use std::fmt;

/// XPath 求值中"表达式本身合法、但运行期失败"的类别（W3C XPath 1.0 §2.4）。
///
/// 规范把这些定义为**动态错误**并给它们取了名字；保留名字与编号是为了让
/// 诊断信息可被检索（例如栈深上界对应 `XPath 1.0 §2.4 err:DEPTH`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XPathErrorCode {
    /// 语法错误（规范里对应静态错误，无 `err:` 代号）。
    Syntax,
    /// 自变量类型不匹配（§2.4.2 `err:FORG0006` 一类的"wrong argument type"）。
    TypeMismatch,
    /// 路径或谓词对节点集类型有要求而得到别的类型（§3.2 上下文要求）。
    NodeSetRequired,
    /// 表达式递归深度超限（§2.4 的 `err:DEPTH` 语义）。
    DepthLimit,
    /// 谓词求值的步数超限（本项目自设的**计算上界**，非规范概念）。
    StepLimit,
}

impl fmt::Display for XPathErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Syntax => "语法错误",
            Self::TypeMismatch => "参数类型不匹配",
            Self::NodeSetRequired => "此处要求节点集",
            Self::DepthLimit => "表达式深度超限",
            Self::StepLimit => "谓词求值步数超限",
        };
        formatter.write_str(text)
    }
}

/// XPath 子集的错误。
///
/// 不实现 `thiserror::Error` 的 derive：本类型是内部表示，展示格式由
/// [`Self::detail`] 交给 `RuleError::syntax` / `RuleError::limit` 承担，
/// 避免两处各写一份文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XPathError {
    /// 错误类别
    code: XPathErrorCode,
    /// 诊断细节（**不含**面向用户的自然语言文案，FR-044）
    detail: String,
}

impl XPathError {
    /// 语法错误。
    pub fn syntax(detail: impl Into<String>) -> Self {
        Self {
            code: XPathErrorCode::Syntax,
            detail: detail.into(),
        }
    }

    /// 自变量类型不匹配。
    pub fn type_mismatch(detail: impl Into<String>) -> Self {
        Self {
            code: XPathErrorCode::TypeMismatch,
            detail: detail.into(),
        }
    }

    /// 节点集要求未被满足。
    pub fn node_set_required(detail: impl Into<String>) -> Self {
        Self {
            code: XPathErrorCode::NodeSetRequired,
            detail: detail.into(),
        }
    }

    /// 深度或步数上界被突破。
    pub fn limit(code: XPathErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    /// 错误类别。
    pub fn code(&self) -> XPathErrorCode {
        self.code
    }

    /// 诊断细节。
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for XPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}：{}", self.code, self.detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 构造的错误携带类别与细节() {
        let error = XPathError::syntax("缺少 `]`");
        assert_eq!(error.code(), XPathErrorCode::Syntax);
        assert_eq!(error.detail(), "缺少 `]`");

        let limit = XPathError::limit(XPathErrorCode::StepLimit, "超过步数上限");
        assert_eq!(limit.code(), XPathErrorCode::StepLimit);
    }

    #[test]
    fn 展示文案含类别与细节() {
        let text = XPathError::type_mismatch("number() 收到节点集").to_string();
        assert!(text.contains("参数类型不匹配"));
        assert!(text.contains("number() 收到节点集"));
    }
}
