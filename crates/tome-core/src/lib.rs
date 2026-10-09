//! # tome-core —— 接缝中枢
//!
//! 本 crate 是整个 tome 引擎的**依赖地基与接缝中枢**：所有实现 crate 只依赖它，
//! 它自己不依赖任何实现 crate（星型无环，见 `specs/001-pc-engine-core/plan.md` 依赖图）。
//!
//! ## 职责边界
//!
//! - **领域 DTO**（[`model`]）：书源/书籍/章节/规则/阅读痕迹等纯数据。
//!   上层与实现 crate 的公共语言；不含 DB/UI 注解（无 serde/无图形类型）。
//! - **`ErrorKind` 错误分类**：各 crate 自带 thiserror 错误类型，在接缝处映射为
//!   本 crate 的小型分类枚举，携带**可本地化的错误标识**而非文案（FR-044 / SC-016）。
//! - **顶层接缝 trait**：`SourceEngine` / `ContentProvider` / `BookRepository`。
//! - **宿主能力 trait 与共享值对象**：`NetHost` / `StoreHost` / `CryptoHost` 与
//!   `RequestSpec` / `Response`。规则引擎求值产出 `RequestSpec`，管线是唯一发
//!   请求处（架构决策 A2，规则引擎因此可离线重放）。
//!
//! ## 设计约束
//!
//! - **零图形依赖**：必须能在无窗口系统上完整运行、可被无界面调用（FR-036 / SC-011）。
//! - **窄接口、无工具函数**：这里只放接缝与 DTO，不放"顺手能用"的辅助函数——
//!   任何加进来的东西都会扩散到全部 crate，且随 core 一起进入所有人的编译链路。
//! - **不做全局上帝错误枚举**：core 的变更不应波及全员，故错误分类保持粗粒度。
//! - **零外部依赖**：core 被所有人依赖，加依赖即扩散；DTO 刻意不派生 serde 等
//!   序列化能力（那是存储/网络实现 crate 的关切）。
//!
//! ## 关键入口
//!
//! - [`model`] —— 领域 DTO（T009）

#![forbid(unsafe_code)]

pub mod model;

pub use model::*;
