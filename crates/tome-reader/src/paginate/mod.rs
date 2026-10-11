//! `[DONE]` 分页与参数集（T017-T022 / S2-2）
//!
//! | 模块 | 职责 | 需求 |
//! |---|---|---|
//! | [`config`] | 排版参数集（几何/呈现子集分离，白名单校验） | FR-013 / FR-028 |
//! | [`page`] | 页序列（页 = 字符区间）+ 锚点解析 | FR-012 / FR-017 |
//! | [`spread`] | 单双页判据 + 手动覆盖三态与语义回落 | FR-014 / FR-015 |
//! | [`booksem`] | 书页语义（左偶右奇 / 章起偶数页留白） | FR-016 |
//!
//! 增量重排在 crate 根的 [`crate::precalc`]（FR-044/045）。
//!
//! ## 依赖方向
//!
//! ```text
//! layout（行盒） ──→ paginate（本模块） ──→ scroll（T025 起）
//! ```
//!
//! paginate **不依赖** render——分页是几何计算，必须能在无图形环境下完整
//! 运行（宪法 IV / FR-052）。它也**不读 Theme**：分页结果的页数只依赖
//! 几何参数（FR-019 / SC-005）。这条不变量由类型系统辅助强制——
//! [`config::ReadingConfig`] 把几何与呈现拆成两个字段，
//! 分页函数只接受 `geometry` 那一个。

pub mod booksem;
pub mod config;
pub mod page;
pub mod spread;
