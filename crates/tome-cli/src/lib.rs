//! # [WIP] tome-cli —— CLI 验证器
//!
//! 引擎的无界面调用入口，也是**组合根**：所有实现 crate 在此被装配起来。
//! 子命令规划见 `specs/001-pc-engine-core/contracts/cli.md`：
//! `search` / `info` / `toc` / `content` / `run` / `verify` / `local` / `data`。
//!
//! ## 在本项目中的位置
//!
//! 唯一允许依赖全部实现 crate 的位置（见 plan.md 依赖图）。它承载两项关键职责：
//!
//! 1. **组合根**：读 `config.toml` 一次，把类型化子集注入各模块；
//!    禁止全局 `CONFIG` 单例（架构决策 A3）。
//! 2. **差分测试入口**：合成语料入仓库、真实书源对照数据不入仓库。
//!
//! ## 架构约束
//!
//! 引擎必须能在**无图形环境**下完整运行——CLI 的存在本身就是这条约束的验收手段
//! （FR-036 / SC-011）。
//!
//! ## 关键入口
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`flags`] | 参数解析与退出码映射（T028/T029） |
//! | [`commands`] | 命令实现：`source import` / `search` + 执行器装配（T028/T029） |
//!
//! 二进制入口是 `src/main.rs`（薄壳：解析 → 执行 → 写 stdout/stderr → 退出）。
//!
//! ## 完成度
//!
//! [WIP] `source import` 与 `search` 已实现（S1-1 切片）；
//! `info` / `toc` / `content` / `run` / `verify` / `local` / `data`
//! 属后续切片（`contracts/cli.md` 定义了它们的契约）。

#![forbid(unsafe_code)]

pub mod commands;
pub mod flags;
pub mod js_host;
// `local` 是 T076-T083 的 CLI 面，全部实现收在这一个文件里（P7 合并）。
pub mod local;
pub mod pipeline_commands;
pub mod root;

pub use commands::{build_executor, run, CommandOutcome};
pub use flags::{
    exit_code_for, parse, Command, GlobalOptions, NetworkMode, OutputFormat, ParseOutcome,
};
pub use local::{
    run_import as run_local_import, run_scan as run_local_scan, run_toc as run_local_toc,
};
pub use root::{AppRoot, ConfigProxyOverride, StoreHostFactory};
