//! [DONE] tome-source 的错误类型（AGENTS.md：每个 crate 拥有自己的错误类型）
//!
//! 在接缝处映射为 [`ErrorKind`]（见 [`SourceError::to_error_kind`]）。
//! 与 `tome-net`/`tome-rule` 同构：细节留在本 crate，分类留给上层。

use tome_core::ErrorKind;

/// 书源管线的错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// 书源文本不是合法 JSON，或顶层形状不对（渠道级失败）。
    ///
    /// 与"数组里某一条坏掉"不同：那是**条目级**失败，计入
    /// `ImportReport::skipped` 而不中断整份导入。
    #[error("error.rule.source.json reason={reason}")]
    InvalidJson {
        /// 诊断说明
        reason: String,
    },

    /// 拉取书源失败（URL 渠道）。`kind` 已是由 `NetHost` 分类好的错误。
    #[error("error.network.fetch kind={kind:?}")]
    Fetch {
        /// 宿主给出的错误分类
        kind: ErrorKind,
    },

    /// 书源不存在（按 id 查找未命中）。
    #[error("error.rule.source.missing id={id}")]
    SourceNotFound {
        /// 请求的书源 id
        id: String,
    },

    /// 书源的搜索配置不完整（缺 `searchUrl` 或搜索规则）。
    ///
    /// 与"规则语法错误"不同：这是**配置缺失**，重试或改网络都无益，
    /// 需要用户去补书源定义。
    #[error("error.rule.source.incomplete id={id} reason={reason}")]
    IncompleteSource {
        /// 书源 id
        id: String,
        /// 缺什么
        reason: String,
    },

    /// 规则求值失败（选择器语法错、组合符不成对等）。
    #[error("error.rule.eval reason={reason}")]
    Rule {
        /// 诊断说明
        reason: String,
    },

    /// 网络层失败（管线执行 `RequestSpec` 时）。
    #[error("error.network.fetch kind={kind:?}")]
    Network {
        /// 宿主给出的错误分类
        kind: ErrorKind,
    },
}

impl SourceError {
    /// 便捷构造"规则求值失败"。
    pub fn rule(reason: impl Into<String>) -> Self {
        Self::Rule {
            reason: reason.into(),
        }
    }

    /// 便捷构造"书源配置不完整"。
    pub fn incomplete(id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::IncompleteSource {
            id: id.into(),
            reason: reason.into(),
        }
    }

    /// 映射到接缝层的错误分类（唯一转换点）。
    ///
    /// | 本层 | 接缝 | 理由 |
    /// |---|---|---|
    /// | `InvalidJson` / `Rule` / `IncompleteSource` / `SourceNotFound` | `Rule` | 书源定义本身的问题 |
    /// | `Fetch` / `Network` | 透传宿主给出的分类 | 网络层已经分好类，重分类会丢信息 |
    pub fn to_error_kind(&self) -> ErrorKind {
        match self {
            Self::InvalidJson { .. }
            | Self::Rule { .. }
            | Self::IncompleteSource { .. }
            | Self::SourceNotFound { .. } => ErrorKind::Rule,
            // 透传而非重分类：宿主（tome-net）已经区分了 Network/Proxy/超时等，
            // 在这里压成 `Network` 会让"代理故障应改代理设置"这条处置建议丢失。
            Self::Fetch { kind } | Self::Network { kind } => kind.clone(),
        }
    }
}

impl From<SourceError> for ErrorKind {
    fn from(error: SourceError) -> Self {
        error.to_error_kind()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 书源定义类错误归规则() {
        for error in [
            SourceError::InvalidJson {
                reason: "bad".to_string(),
            },
            SourceError::rule("bad selector"),
            SourceError::incomplete("https://a", "缺 searchUrl"),
            SourceError::SourceNotFound {
                id: "https://a".to_string(),
            },
        ] {
            assert_eq!(error.to_error_kind(), ErrorKind::Rule, "{error}");
        }
    }

    #[test]
    fn 网络类错误透传宿主分类() {
        assert_eq!(
            SourceError::Fetch {
                kind: ErrorKind::Proxy
            }
            .to_error_kind(),
            ErrorKind::Proxy,
            r#"代理故障不该被压成 Network——那会丢掉「该改代理设置」这条处置建议"#
        );
        assert_eq!(
            SourceError::Network {
                kind: ErrorKind::Network
            }
            .to_error_kind(),
            ErrorKind::Network
        );
    }

    #[test]
    fn 可从_source_error_转成_error_kind() {
        let kind: ErrorKind = SourceError::rule("x").into();
        assert_eq!(kind, ErrorKind::Rule);
    }
}
