//! [DONE] 排版层（T013 起）
//!
//! | 模块 | 职责 | 需求 |
//! |---|---|---|
//! | [`engine`] | ShapeLine 克隆后处理引擎（禁则合并 + layout） | research R1 方案 A |
//! | [`linebox`] | 行盒结构（data-model §4 七字段） | FR-008 / FR-012 |
//! | [`style`] | 行内样式最小集合（标题/强调/引用/居中） | FR-010 |
//!
//! ## 依赖方向
//!
//! ```text
//! text（文本模型） ──┐
//!                    ├──→ layout（本模块） ──→ paginate / scroll（T019 起）
//! rules（禁则表） ───┘
//! ```
//!
//! layout **不依赖** render——排版计算必须能在无图形环境下完整运行
//! （宪法 IV / FR-052）。这条由 crate 边界强制：`render` 挂 feature，
//! 而本模块无条件编译。

pub mod engine;
pub mod linebox;
pub mod style;
