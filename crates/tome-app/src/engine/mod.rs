//! `[WIP]` 引擎线程与消息通道（S3 T006/T007/T011/T013/T014/T015）
//!
//! 模块职责见各子模块的文档：[`messages`] 是线程间唯一的语言，
//! [`thread`] 是引擎线程本体（独占 `Paginator`/store/source 的阻塞工作），
//! [`worker`] 是生产 worker（把请求翻成 S1/S2 能力上的一次次调用）。

pub mod messages;
pub mod thread;
pub mod worker;
