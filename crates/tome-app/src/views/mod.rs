//! `[WIP]` 视图层（S3 T012-T014）
//!
//! ## 视图层做什么、不做什么
//!
//! **做**：把状态画出来，把输入翻成意图（引擎请求）。
//! **不做**：任何决策。哪些条目可见、进度何时落盘、页边界在哪——这些都在
//! [`crate::state`] 与引擎线程里，因为它们都有可判定的验收要求，而判定不该
//! 依赖一个真实窗口。
//!
//! ## 纪律（与 [`crate::widgets`] 同）
//!
//! 1. **零中文字面串**——文案一律出自 [`crate::i18n`]（`contracts/ui.md` §3.1）
//! 2. **零色值字面量**——配色只取 token，换算只在
//!    [`crate::theme_bridge::BridgeTheme::color`] 一处
//!
//! 两条由 T031 的守卫与 `tests/theme_bridge.rs` 的 grep 断言强制。

pub mod overlay;
pub mod reading;
pub mod shelf;
