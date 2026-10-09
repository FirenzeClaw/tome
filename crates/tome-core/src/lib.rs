//! # tome-core —— 接缝中枢
//!
//! 本 crate 是整个 tome 引擎的**依赖地基与接缝中枢**：所有实现 crate 只依赖它，
//! 它自己不依赖任何实现 crate（星型无环，见 `specs/001-pc-engine-core/plan.md` 依赖图）。
//!
//! ## 职责边界
//!
//! - **宿主能力 trait**（`NetHost` / `StoreHost` / `CryptoHost`）：定义能力的**契约**，
//!   由 `tome-net` / `tome-store` 实现，经组合根注入给 `tome-js` 的沙箱使用。
//! - **共享值对象**（`RequestSpec` / `Response`）：规则引擎求值产出 `RequestSpec`，
//!   管线是唯一发请求处（架构决策 A2，规则引擎因此可离线重放）。
//! - **`ErrorKind` 错误分类**：各 crate 自带 thiserror 错误类型，在接缝处映射为本 crate
//!   的小型分类枚举，携带**可本地化的错误标识**而非文案（FR-044 / SC-016）。
//!
//! ## 设计约束
//!
//! - **零图形依赖**：必须能在无窗口系统上完整运行、可被无界面调用（FR-036 / SC-011）。
//! - **窄接口、无工具函数**：这里只放接缝与 DTO，不放"顺手能用"的辅助函数——
//!   任何加进来的东西都会扩散到全部 crate，且随 core 一起进入所有人的编译链路。
//! - **不做全局上帝错误枚举**：core 的变更不应波及全员，故错误分类保持粗粒度。
//!
//! ## 关键入口
//!
//! 骨架阶段尚无公开 API；Phase 2（T009 起）按 `contracts/seams.md` 落地
//! 宿主能力 trait、共享值对象与 `ErrorKind`。
//!
//! [WIP] 本 crate 当前为空骨架，未含任何实现（Phase 2 起填充）。

#![forbid(unsafe_code)]
