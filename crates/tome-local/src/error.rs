//! \[DONE] 本 crate 的错误类型与 `ErrorKind` 映射
//!
//! AGENTS.md 规定「每个 crate 拥有自己的错误类型（thiserror）；在接缝处映射为
//! tome-core 的小型 `ErrorKind` 分类枚举」。本模块即 tome-local 的这一层。
//!
//! # 为什么要区分 `UnsupportedFormat` 与 `Damaged`
//!
//! 两者在上层得到**完全不同的处置**：
//!
//! | 情形 | 变体 | 上层动作 |
//! |---|---|---|
//! | KFX/Topaz（本产品不支持的格式） | [`LocalError::UnsupportedFormat`] | 提示"格式不受支持"，不重试 |
//! | 文件截断/记录表越界（损坏） | [`LocalError::Damaged`] | 提示"文件已损坏"，可建议重新下载 |
//! | DRM 加密（有密钥才行） | [`LocalError::DrmProtected`] | 提示需要去 DRM，**不是**"格式不支持" |
//!
//! 把这三者都塞进一个 "无法解析" 变体，用户就拿不到可操作的指引——
//! 这正是 FR-006「不支持的能力要抛可捕获的结构化错误，而非让 API 未定义」
//! 的取向：错误的**类别**本身是给上层的可编程信号。

use std::fmt;

use tome_core::ErrorKind;

/// tome-local 的错误类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalError {
    /// 文件系统 I/O 失败。
    Io {
        /// 出错的路径
        path: String,
        /// 诊断说明
        reason: String,
    },
    /// 文件格式可识别，但内容损坏（截断、记录表越界、压缩流非法）。
    Damaged {
        /// 诊断说明（含出错位置）
        reason: String,
    },
    /// 格式不受本产品支持（KFX / Topaz 等），或格式与扩展名不符。
    UnsupportedFormat {
        /// 格式标识（`kfx` / `topaz` / 具体魔数说明）
        format: String,
    },
    /// 文件受 DRM 保护，无法在无密钥的情况下解析（FR-022 的边缘情形）。
    DrmProtected {
        /// 加密类型码（PalmDOC 头 0x0C 的值）
        encryption_type: u16,
    },
    /// 编码无法解码。**当前实现不会返回它**——编码检测永不失败
    /// （[`crate::encoding`] 的契约是"永远给出可读文本"），保留变体是为了
    /// 让"路径不存在"这类配置错误有归处。
    Configuration {
        /// 诊断说明
        reason: String,
    },
}

impl LocalError {
    /// 构造 I/O 错误。
    pub fn io(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Io {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// 构造损坏错误。
    pub fn damaged(reason: impl Into<String>) -> Self {
        Self::Damaged {
            reason: reason.into(),
        }
    }

    /// 映射到 tome-core 的错误分类（接缝处唯一的映射点）。
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Io { .. } => ErrorKind::Storage,
            Self::Damaged { .. } => ErrorKind::Content,
            // FR-006：能力缺失是**结构化信号**而非普通失败，故带上能力标识。
            Self::UnsupportedFormat { format } => ErrorKind::unsupported_capability(
                tome_core::ErrorCapability::new(format!("localFormat:{format}")),
            ),
            // DRM 同样是"此路不通"而非"文件坏了"：本产品**不可能**在无密钥时
            // 解开它，故归入能力类（上层据 capability_id 区分是 DRM 还是别的）。
            Self::DrmProtected { .. } => ErrorKind::unsupported_capability(
                tome_core::ErrorCapability::new("localFormat:drm"),
            ),
            Self::Configuration { .. } => ErrorKind::Configuration,
        }
    }
}

impl fmt::Display for LocalError {
    /// 输出形如 `error.content.local_damaged: 记录表越界`。
    ///
    /// 标识在前、细节在后：日志按标识检索，细节供人阅读（与 core 的
    /// [`ClassifiedError`](tome_core::error::ClassifiedError) 同构）。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, reason } => {
                write!(formatter, "error.local.io path={path} reason={reason}")
            }
            Self::Damaged { reason } => {
                write!(formatter, "error.local.damaged reason={reason}")
            }
            Self::UnsupportedFormat { format } => {
                write!(formatter, "error.local.unsupported_format format={format}")
            }
            Self::DrmProtected { encryption_type } => write!(
                formatter,
                "error.local.drm encryption_type={encryption_type}"
            ),
            Self::Configuration { reason } => {
                write!(formatter, "error.local.configuration reason={reason}")
            }
        }
    }
}

impl std::error::Error for LocalError {}

/// 把 `std::io::Error` 连同路径一起包装。
pub fn from_io(path: &std::path::Path, error: std::io::Error) -> LocalError {
    LocalError::io(path.to_string_lossy().into_owned(), error.to_string())
}

/// 本 crate 的便捷 `Result`。
pub type LocalResult<T> = Result<T, LocalError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_maps_to_storage() {
        assert_eq!(LocalError::io("/x", "denied").kind(), ErrorKind::Storage);
    }

    #[test]
    fn damaged_maps_to_content() {
        assert_eq!(LocalError::damaged("truncated").kind(), ErrorKind::Content);
    }

    #[test]
    fn unsupported_format_carries_capability_id() {
        // FR-006：能力类错误必须携带标识，上层据此分支（而非读文案）。
        let kind = LocalError::UnsupportedFormat {
            format: "kfx".to_string(),
        }
        .kind();
        match kind {
            ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "localFormat:kfx");
            }
            other => panic!("应映射为能力类错误，实际 {other:?}"),
        }
    }

    #[test]
    fn drm_is_a_capability_error_not_a_content_error() {
        // DRM 与"文件损坏"必须可区分：前者用户能自己做点什么（换文件/去 DRM），
        // 后者只能重新获取。
        let kind = LocalError::DrmProtected { encryption_type: 1 }.kind();
        assert!(matches!(kind, ErrorKind::UnsupportedCapability { .. }));
    }

    #[test]
    fn display_uses_stable_identifiers() {
        let text = LocalError::DrmProtected { encryption_type: 2 }.to_string();
        assert!(text.starts_with("error.local.drm"), "实际 {text}");
    }
}
