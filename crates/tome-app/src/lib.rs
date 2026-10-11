//! `[WIP]` tome 应用外壳库入口（S3）
//!
//! ## 为什么外壳要有一个库目标
//!
//! 它同时是「示例与集成测试能复用外壳代码」的前提（`examples/spike_render.rs`
//! 要 `use tome_app::render::...`），也是把**可测的逻辑**与**进程入口**
//! （单实例判定、`eframe::run_native`）分开的落点：`main.rs` 是薄壳，
//! 逻辑在库里。
//!
//! ## feature 门控
//!
//! [`render`] 在 `reader-render` 下才存在——该 feature 转发到
//! `tome-reader/render`，关闭时 S2 的渲染面在**编译期**消失，外壳的渲染路径
//! 随之不存在（而不是"运行时用不了"）。这是 workspace 级
//! `cargo test --workspace --no-default-features` 门不红的前提。

#![deny(unsafe_code)]

pub mod app;
pub mod composition;
pub mod engine;
pub mod i18n;
pub mod state;
pub mod theme_bridge;
pub mod timing;
pub mod views;
pub mod widgets;

#[cfg(feature = "reader-render")]
pub mod render;
