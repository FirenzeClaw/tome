//! `[WIP]` 引擎线程与消息通道（S3 T006/T007）
//!
//! 模块职责见各子模块的文档：[`messages`] 是线程间唯一的语言，`thread` 是
//! 引擎线程本体（独占 `Paginator`/store/source 的阻塞工作）。

pub mod messages;
