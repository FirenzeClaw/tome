//! # tome-js —— QuickJS 脚本沙箱
//!
//! 承载书源中的 JS 规则片段：QuickJS 运行时封装、宿主能力（网络/存储/加密）的注入接线，
//! 以及让既有书源脚本无需改动即可运行的**兼容前导脚本**（compat prelude）。
//!
//! ## 在本项目中的位置
//!
//! 本 crate **不依赖** `tome-net` / `tome-store`——宿主能力 trait 定义在 `tome-core`，
//! 由组合根注入实现（架构决策 A1）。这避免了 net ↔ js 的循环依赖。
//!
//! ## 安全护栏（本 crate 是安全边界）
//!
//! - 脚本执行必须设**内存上限**与**执行中断**；Rust 中杀线程是未定义行为，
//!   **绝不可**用线程终止兜住死循环脚本（FR-007 / SC-013）。
//!   > 关键前置：**绝对禁止**开启 rquickjs 的 `rust-alloc` / `allocator` feature
//!   > ——一旦启用，`set_memory_limit` 会静默变为 noop，内存防线失效。
//!   > 该约束由 `.github/workflows/ci.yml` 的 feature 守卫与
//!   > `scripts/check-rquickjs-features.sh` 强制。
//! - 脚本的文件访问限制在数据目录内，越界须拒绝并报告。
//! - **不支持的能力要抛可捕获的结构化错误**，而非让 API 未定义——书源的 try/catch
//!   降级逻辑依赖这一点（FR-006）。
//!
//! ## 关键入口
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`runtime`] | QuickJS runtime 包装：内存/栈/中断三道护栏（T030） |
//! | [`testdouble`] | 内存态 `StoreHost`/`CryptoHost` 替身（T033） |
//! | [`sandbox_error`] | 宿主函数侧的 JS 异常类型与能力错误（FR-006） |
//! | [`bridge`] | Rust ↔ JS 调用约定：宿主状态、同步阻塞、错误展开（T034） |
//!
//! [WIP] S1-2 阶段落地进行中；本 crate 此前为空骨架。

// ## 关于 `unsafe_code`
//
// 本 crate 需要**恰好一处** `unsafe`：`bridge.rs` 里为 `HostBridge` 实现
// rquickjs 的 `JsLifetime`。那个 trait 是 `unsafe`，而它是把宿主状态放进
// `Ctx::userdata` 的**唯一**途径（rquickjs 0.14 没有安全替代）。
//
// 该实现的安全性由类型本身的形状保证：`HostBridge` 的字段全是 `'static`
// （`Arc<dyn Trait>` / `String` / `Arc<Mutex<HashMap>>`），不含任何 `'js` 值，
// 故 rquickjs 文档所述的"类型没有关联生命周期时总是安全"直接适用。
// 详见该 impl 处的 SAFETY 注释。
//
// 故此处用 `deny` 而非 `forbid`：`forbid` 无法被局部 `#[allow]` 放宽，
// 会把上述必要实现也一并禁掉。改用 `deny` 后，除该处之外任何新引入的
// `unsafe` 仍会被 CI 的 `-D warnings` 拦下，护栏强度不降。
#![deny(unsafe_code)]

pub mod bridge;
pub mod prelude;
pub mod runtime;
pub mod sandbox_error;
pub mod testdouble;

pub use bridge::{host_bridge, require_bridge, throw_host_error, HostBridge, JsContext};
pub use prelude::PRELUDE_JS;
pub use runtime::{CancelToken, JsValue, Limits, LimitsError, Sandbox, SandboxError};
pub use sandbox_error::JsError;
pub use testdouble::{MemoryCryptoHost, MemoryStoreHost};
