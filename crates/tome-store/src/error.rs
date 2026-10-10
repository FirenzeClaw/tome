//! \[DONE] tome-store 的错误类型（AGENTS.md：每个 crate 拥有自己的错误类型）
//!
//! 与 `tome-net`/`tome-rule`/`tome-source` 同构：**细节留在本 crate，分类留给
//! 上层**。接缝处经 [`StoreError::to_error_kind`] 映射为 [`tome_core::ErrorKind`]，
//! 上层（界面/CLI）据分类给提示，不依赖本 crate 的错误变体（宪法 IV）。
//!
//! # 为什么底层错误存成 `String` 而不是 `#[from] rusqlite::Error`
//!
//! `rusqlite::Error` 不实现 `Clone`，而本 crate 的错误要在 DB actor 的**跨线程
//! 通道**上往返（actor 线程产出、调用方线程消费）。用 `#[from]` 会把
//! `rusqlite::Error` 直接焊进类型，届时通道两端只能传 `Box<dyn Error>`——
//! 那又丢了分类信息。故在**错误产生的那一行**把它转成分类 + 诊断文本，
//! 让错误类型保持 `Send + Sync + Clone`。
//!
//! 代价是丢失了"用 `matches!` 匹配具体 SQLite 错误码"的能力。此处可接受：
//! 需要按码分支的地方（如磁盘满 → [`StoreError::DiskFull`]）在转换点显式判别，
//! 而不是把判断散到各调用方。

use tome_core::{ErrorCapability, ErrorKind};

/// 存储层错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// 文件系统 I/O 失败（数据目录、Pack 文件、图片缓存文件）。
    #[error("error.storage.io path={path} reason={reason}")]
    Io {
        /// 出错的路径
        path: String,
        /// 诊断说明
        reason: String,
    },

    /// SQLite 操作失败（建表、查询、事务）。
    #[error("error.storage.sqlite reason={reason}")]
    Sqlite {
        /// 诊断说明（含 SQLite 的原始报错文本）
        reason: String,
    },

    /// 迁移失败：schema 版本高于本程序所知，或迁移脚本执行出错。
    ///
    /// ## 为什么"版本过高"必须是独立变体
    ///
    /// 用户把新版本的数据目录拿给旧版本程序打开时，**唯一安全的动作是拒绝**。
    /// 若退化成"尽力而为"，旧程序会用旧 schema 去读新表结构，产生的是**静默
    /// 数据损坏**——比直接报错严重得多。故这条路径必须有明确的错误出口。
    #[error("error.storage.migration version={version} supported={supported} reason={reason}")]
    Migration {
        /// 数据目录里的 schema 版本
        version: i64,
        /// 本程序支持的版本
        supported: i64,
        /// 诊断说明
        reason: String,
    },

    /// 数据目录不可写（FR-043：明确报告，不静默失败）。
    #[error("error.storage.data_dir_not_writable path={path} reason={reason}")]
    DataDirNotWritable {
        /// 数据目录路径
        path: String,
        /// 诊断说明
        reason: String,
    },

    /// 磁盘空间不足（FR-043）。
    #[error("error.storage.disk_full path={path} required={required} available={available}")]
    DiskFull {
        /// 涉及的路径
        path: String,
        /// 需要的字节数
        required: u64,
        /// 可用的字节数
        available: u64,
    },

    /// 数据不存在（如按 id 取书源未命中）。
    ///
    /// ## 为什么"不存在"是错误而不是 `Ok(None)`
    ///
    /// 由**调用方**决定：接缝签名已经用 `Option` 表达"可能没有"的地方
    /// （`source_by_id` 等），实现方直接返回 `Ok(None)`。本变体服务的是
    /// "按主键更新/删除但目标不在"这类**语义上失败**的操作——静默成功会让
    /// 调用方以为写进去了。
    #[error("error.storage.not_found subject={subject}")]
    NotFound {
        /// 缺失对象的标识
        subject: String,
    },

    /// 数据损坏（Pack 容器头非法、索引指向越界、编码无法还原）。
    #[error("error.storage.corrupt subject={subject} reason={reason}")]
    Corrupt {
        /// 损坏对象的标识
        subject: String,
        /// 诊断说明
        reason: String,
    },

    /// 路径越界：JS 宿主要求的文件读写落在了数据目录之外（安全红线）。
    ///
    /// 映射为 [`ErrorKind::UnsupportedCapability`]——**可被书源 `try/catch`
    /// 捕获**（FR-006）。若返回 panic 或静默失败，书源的降级逻辑无从判断。
    #[error("error.unsupported_capability capability=fileAccessOutOfDataDir path={path}")]
    PathOutOfDataDir {
        /// 被拒绝的路径
        path: String,
    },

    /// 配置非法或缺失（config.toml 解析失败、类型不符）。
    #[error("error.configuration reason={reason}")]
    Config {
        /// 诊断说明
        reason: String,
    },
}

impl StoreError {
    /// 便捷构造 I/O 错误。
    pub fn io(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Io {
            path: path.into(),
            reason: reason.into(),
        }
    }

    /// 便捷构造 SQLite 错误。
    pub fn sqlite(reason: impl Into<String>) -> Self {
        Self::Sqlite {
            reason: reason.into(),
        }
    }

    /// 便捷构造"数据损坏"。
    pub fn corrupt(subject: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Corrupt {
            subject: subject.into(),
            reason: reason.into(),
        }
    }

    /// 便捷构造"不存在"。
    pub fn not_found(subject: impl Into<String>) -> Self {
        Self::NotFound {
            subject: subject.into(),
        }
    }

    /// 便捷构造"配置非法"。
    pub fn config(reason: impl Into<String>) -> Self {
        Self::Config {
            reason: reason.into(),
        }
    }

    /// 映射为接缝的粗粒度分类（FR-044：分类进接缝，细节留在本 crate）。
    pub fn to_error_kind(&self) -> ErrorKind {
        match self {
            Self::Io { .. } => ErrorKind::Storage,
            Self::Sqlite { .. } => ErrorKind::Storage,
            Self::Migration { .. } => ErrorKind::Storage,
            Self::DataDirNotWritable { .. } => ErrorKind::Storage,
            Self::DiskFull { .. } => ErrorKind::Storage,
            Self::NotFound { .. } => ErrorKind::Storage,
            Self::Corrupt { .. } => ErrorKind::Storage,
            Self::PathOutOfDataDir { .. } => {
                ErrorKind::unsupported_capability(ErrorCapability::new("fileAccessOutOfDataDir"))
            }
            Self::Config { .. } => ErrorKind::Configuration,
        }
    }
}

/// 把 [`rusqlite::Error`] 转成 [`StoreError`]，并在**转换点**判别磁盘满。
///
/// ## 为什么要在这里判别磁盘满
///
/// FR-043 要求"磁盘空间不足时明确报告并给出指引"。SQLite 把它报成
/// `Error::SqliteFailure(ErrorCode::DiskFull, ..)`，而一旦压成文本就再也
/// 分不出来——用户看到的会是"某个 SQL 失败"，而不是"磁盘满了，去清点空间"。
/// 故判别必须发生在**错误还有结构**的那一行。
pub fn from_rusqlite(error: rusqlite::Error) -> StoreError {
    if let rusqlite::Error::SqliteFailure(code, ref message) = error {
        if code.code == rusqlite::ErrorCode::DiskFull {
            return StoreError::sqlite(format!(
                "磁盘空间不足（SQLite DiskFull）：{}",
                message.as_deref().unwrap_or("无附加说明")
            ));
        }
    }
    StoreError::sqlite(error.to_string())
}

/// 把 [`std::io::Error`] 转成 [`StoreError`]，并在**转换点**判别磁盘满。
pub fn from_io(path: &std::path::Path, error: std::io::Error) -> StoreError {
    // `ErrorKind::StorageFull` 至今仍是 unstable，故按平台的原始错误码判别。
    // Windows 的 ERROR_DISK_FULL(112) / ERROR_HANDLE_DISK_FULL(39)、
    // POSIX 的 ENOSPC(28) 都落到这里。
    let is_disk_full = matches!(error.raw_os_error(), Some(28) | Some(112) | Some(39));
    let path_text = path.display().to_string();
    if is_disk_full {
        return StoreError::DiskFull {
            path: path_text,
            required: 0,
            available: 0,
        };
    }
    StoreError::io(path_text, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 分类映射覆盖全部变体() {
        let cases: Vec<(StoreError, ErrorKind)> = vec![
            (StoreError::io("a", "b"), ErrorKind::Storage),
            (StoreError::sqlite("b"), ErrorKind::Storage),
            (
                StoreError::Migration {
                    version: 2,
                    supported: 1,
                    reason: "旧程序打开新库".to_string(),
                },
                ErrorKind::Storage,
            ),
            (
                StoreError::DataDirNotWritable {
                    path: "p".to_string(),
                    reason: "只读".to_string(),
                },
                ErrorKind::Storage,
            ),
            (
                StoreError::DiskFull {
                    path: "p".to_string(),
                    required: 1,
                    available: 0,
                },
                ErrorKind::Storage,
            ),
            (StoreError::not_found("x"), ErrorKind::Storage),
            (StoreError::corrupt("x", "y"), ErrorKind::Storage),
            (
                StoreError::PathOutOfDataDir {
                    path: "../x".to_string(),
                },
                ErrorKind::unsupported_capability(ErrorCapability::new("fileAccessOutOfDataDir")),
            ),
            (StoreError::config("bad"), ErrorKind::Configuration),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_error_kind(), expected, "变体 {error:?} 的分类不符");
        }
    }

    #[test]
    fn 磁盘满在转换点被识别() {
        let error = from_rusqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DiskFull,
                extended_code: 0,
            },
            Some("database or disk is full".to_string()),
        ));
        assert!(
            matches!(error, StoreError::Sqlite { .. }),
            "应保持存储分类：{error:?}"
        );
        assert!(
            error.to_string().contains("磁盘空间不足"),
            "应明确说出磁盘满：{error}"
        );
    }

    #[test]
    fn io_磁盘满按平台错误码识别() {
        let error = from_io(
            std::path::Path::new("/tmp/x"),
            std::io::Error::from_raw_os_error(28),
        );
        assert!(
            matches!(error, StoreError::DiskFull { .. }),
            "ENOSPC 应识别为磁盘满：{error:?}"
        );
    }

    #[test]
    fn 普通_io_错误不被误判为磁盘满() {
        let error = from_io(
            std::path::Path::new("/tmp/x"),
            std::io::Error::from_raw_os_error(13),
        );
        assert!(matches!(error, StoreError::Io { .. }));
    }
}
