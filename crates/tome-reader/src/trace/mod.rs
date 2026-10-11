//! \[DONE] 阅读痕迹层（T042-T044 / FR-037、FR-038、FR-040、FR-033、FR-041）
//!
//! # 本模块的职责
//!
//! S1 把阅读痕迹（书签 / 高亮 / 批注）以**字符区间**存储（FR-028）。本模块
//! 是那些区间在阅读层的**全部消费面**：
//!
//! | 子模块 | 职责 | 需求 | 状态 |
//! |---|---|---|---|
//! | [`render`] | 区间 → 可绘制片段 + 样式词表 | FR-037 / FR-033 | [DONE] T042 |
//! | [`hit`] | 命中测试与反查（互逆）+ 文字选择 | FR-040 / FR-038 | [DONE] T043 |
//! | [`dict`] | 词典查词桥（复用 001 §FR-016） | FR-041 | [WIP] T044 |
//!
//! ## 一条贯穿本模块的口径
//!
//! **位置一律字符下标**（FR-003 / FR-028）。本模块的公开 API 上不出现字节
//! 偏移、UTF-16 码元偏移或"渲染坐标"作为持久表达——屏幕坐标只在
//! [`hit`] 的**查询输入**里出现（命中测试的本质就是从坐标找字符），且
//! 它永远只是输入，不是输出。
//!
//! ## 为什么不依赖 `render` feature
//!
//! 痕迹的**语义**（哪段该画什么、哪个坐标对应哪个字符）与图形栈无关。
//! 故本模块在 `--no-default-features` 的构建里完整存在——只有"变成像素"
//! 那一步在 `render` 里。这让 SC-011（无图形环境全能力可调用）覆盖到痕迹。

pub mod hit;
pub mod render;

pub use hit::{hit_test_page, hit_test_scroll, select_range, HitPoint, HitResult, Selection};
pub use render::{
    covered_chars, render_traces, render_traces_scroll, render_traces_to_lines, resolve_style,
    DecorationKind, SemanticStyle, StyleEffect, StyleLevel, StyleResolution, TraceRange,
    TraceSegment, SEMANTIC_STYLE_VOCABULARY,
};
