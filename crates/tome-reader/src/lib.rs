//! # tome-reader —— 排版渲染层（S2）
//!
//! `[DONE]` crate 骨架（T001）：门面、feature 门控、模块占位。
//!
//! 本 crate 把一章正文变成**可读的书页**：段落切分 → 中文排版规则（禁则/挤压/
//! 混排/两端对齐）→ 行盒序列 → 页序列或滚动流 → wgpu 绘制指令。
//!
//! ## 两条架构铁律（编译期强制，非文档约定）
//!
//! 1. **排版不依赖渲染**：`text`/`rules`/`layout`/`paginate`/`scroll` 各模块
//!    一律不引用 [`render`]；渲染只在 `render` feature 下存在。故
//!    `--no-default-features` 的构建**依赖树里根本没有 wgpu**——排版计算在
//!    无图形环境下不是"跑得起来"，而是"编译产物里没有图形栈"
//!    （宪法 IV / FR-052 / FR-036）。
//! 2. **位置一律字符下标**：对外的位置表达只有 `char_offset`（Unicode 标量值
//!    序列下标），与 001 的 `ReadProgress.char_offset` 同口径。字节偏移与
//!    UTF-16 码元偏移**不得**出现在公开 API 上（FR-003 / 001 §FR-028）。
//!
//! ## 依赖方向
//!
//! `tome-reader → tome-core`（接缝与 DTO）。本 crate **不依赖** tome-source /
//! tome-store / tome-net：正文唯一来源是组合根注入的
//! [`tome_core::ContentProvider`]，本层不感知缓存、抓取与净化顺序（FR-004）。
//!
//! ## feature 门控
//!
//! | feature | 默认 | 依赖 | 关闭时的行为 |
//! |---|---|---|---|
//! | `render` | **开** | wgpu + swash + etagere | 依赖树零图形包；`render` 子命令返回 `UnsupportedCapability{capability:"offscreenRendering"}` 且退出码 3 |
//!
//! headless CI job 用 `cargo build --no-default-features` 构建并断言上表第三列。
//!
//! ## 模块一览
//!
//! | 模块 | 职责 | 状态 |
//! |---|---|---|
//! | [`text`] | 文本模型（字符下标口径）+ 段落切分 | `[DONE]` T010/T011 |
//! | [`rules`] | 中文禁则清单与断点判定 | `[DONE]` T012 |
//! | [`layout`] | 排版引擎（禁则合并 + 行盒） | `[DONE]` T013 |
//! | [`paginate`] | 参数集 / 页序列 / 单双页判据 / 书页语义 | `[DONE]` T017-T022 |
//! | [`scroll`] | 滚动流 / 双向映射 / 形态切换 / 重排 / 占位 / 节流 | `[DONE]` T025-T030 |
//! | [`precalc`] | 分页预计算 + 增量重排 | `[DONE]` T024 |
//! | [`record`] | 阅读记录两量 + 跨形态去重 | `[DONE]` T031 |
//! | [`corpus`] | 排版合成语料的装载与校验 | `[DONE]` T004 |
//! | [`error`] | 本层错误类型 + `→ErrorKind` 映射（FR-057/058） | `[DONE]` T003 |
//! | [`font_setup`] | 内嵌正文衬线的**唯一**装载点（feature 无关；排版与渲染共用） | `[DONE]` C1 缺陷修复 |
//! | [`render`] | wgpu 自绘管线（仅 `render` feature） | `[WIP]` S2-4 |
//!
//! 后续切片的模块（`trace` / `toolbar` / `footer` / `keys`）随 S2-5 落地，
//! 见 `specs/002-typesetting-renderer/tasks.md`。

#![forbid(unsafe_code)]

pub mod corpus;
pub mod corpus_eval;
pub mod error;
pub mod font_setup;
pub mod footer;
pub mod keys;
pub mod layout;
pub mod paginate;
pub mod precalc;
pub mod record;
pub mod rules;
pub mod scroll;
pub mod text;
pub mod theme;
pub mod toolbar;
pub mod trace;

#[cfg(feature = "render")]
pub mod render;

pub use error::{ReaderError, ReaderErrorKind};

/// 本层"不支持的能力"的能力标识（FR-057/058）。
///
/// 与 001 的 `ErrorCapability` 同一机制：书源/上层据此做 `try/catch` 降级。
/// 标识用**书源生态的通用叫法**（驼峰，如 `webOffscreenRendering`），不是英文句子，
/// 也不是面向用户的文案。
pub mod capability {
    /// 离屏渲染不可用（`render` feature 关闭，或图形后端初始化失败）。
    ///
    /// 两种触发场景共用同一标识，因为对调用方而言处置相同：**本机拿不到图像**，
    /// 应改走排版结构验证而非像素比对（FR-053 的诚实跳过口径）。
    pub const OFFSCREEN_RENDERING: &str = "offscreenRendering";

    /// 字体不可用（内嵌子集缺失且系统回退链也取不到可用字体）。
    pub const FONT_UNAVAILABLE: &str = "fontUnavailable";
}
