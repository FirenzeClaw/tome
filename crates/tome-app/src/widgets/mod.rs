//! `[WIP]` 自绘组件（S3 T012/T013；MASTER 组件清单的落点）
//!
//! ## 纪律：本目录**零中文字面串**
//!
//! `contracts/ui.md` §3.1：界面文案一律出自 [`crate::i18n`] 的文案表。这条在
//! `views/` 与 `widgets/` 两个目录上由 T031 的 grep 守卫强制。
//!
//! ## 纪律：本目录**零色值字面量**
//!
//! MASTER §10：配色只取 `tome_core::theme` token，且换算只在
//! [`crate::theme_bridge::BridgeTheme::color`] 一处发生。`tests/theme_bridge.rs`
//! 的守卫扫描 `src/**/*.rs`，本目录在其中。
//!
//! 两条纪律的共同点：违反它们的代码**能跑、观感也对、没有任何测试会红**
//! ——直到设计系统改一次主色，或需要加一种语言。守卫是它们唯一的执行机制。

pub mod book_card;
pub mod empty;
pub mod skeleton;
