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
//! 骨架阶段尚无公开 API；rquickjs 依赖在 Phase 2 引入，届时守卫脚本转为强断言模式。
//!
//! [WIP] 本 crate 当前为空骨架，未含任何实现。

#![forbid(unsafe_code)]
