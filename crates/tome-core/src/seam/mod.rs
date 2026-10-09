//! 顶层接缝 trait —— 上层与引擎的唯一交互面
//!
//! 对应 tasks.md T011，契约见 `specs/001-pc-engine-core/contracts/seams.md` §1/§2/§3。
//!
//! # 为什么接缝定义在 core
//!
//! 上层（S3 界面）、CLI 验证器、以及换源逻辑都通过这些 trait 交互，
//! 而**永不直接构造**实现 crate 的内部类型（宪法 IV / AGENTS.md 架构铁律）。
//! 定义在 core 使依赖方向单向成立：实现 crate 依赖 core 并实现这些 trait，
//! core 不认识任何实现。
//!
//! # 三个 trait 的分工
//!
//! | trait | 职责 | 实现方 |
//! |---|---|---|
//! | [`SourceEngine`] | 书源五动作 + 换源 + 源检查 | tome-source |
//! | [`ContentProvider`] | 统一内容提供者（网络书/本地书两个 adapter） | tome-source / tome-local |
//! | [`BookRepository`] | 书架/进度/痕迹/规则的读写 | tome-store |
//!
//! # 本模块的 trait 只引用 core 类型
//!
//! 任何 trait 签名中出现实现 crate 的类型，都会把该 crate 拖进所有调用方的
//! 依赖图——这正是接缝要防止的。故签名里的每个类型都来自 [`crate::model`] 或
//! [`crate::error`]。

mod repository;
mod source;
mod stream;

pub use repository::{
    BackupJson, BookRepository, CacheKind, HighlightRuleRow, ImportReport, SearchHit,
};
pub use source::{
    AnchorQuery, ChangeSourceOutcome, ChapterContent, CheckReport, CheckStream, CheckVerdict,
    ContentProvider, ExplorePage, ProgressMapping, SourceCheckFilter, SourceEngine,
};
pub use stream::{CancelToken, SearchResultItem, SearchStream};

pub mod host;
