//! # S2-0 可行性 spike（tasks.md 002 T005-T009）
//!
//! 一次性验证骨架，**刻意不进主 workspace**（见 `Cargo.toml` 文件头）。
//! 目标是"die 早 die 便宜"：把 research.md R5 的七项遗留风险在写生产代码
//! **之前**变成实测数据。
//!
//! ## 七项与落点
//!
//! | R5 | 验证项 | 落点 | feature |
//! |---|---|---|---|
//! | 1 | ShapeWord 合并后 `layout()` 的行为 | [`word_merge`]（**证伪门**） | `cosmic-text-stack` |
//! | 2 | hinting 对 `x_advance` 挤压的量化影响 | [`hinting`] | `cosmic-text-stack` |
//! | 3 | cosmic-text 全栈体积净增量 | [`size`] + `scripts/measure-s2-stacks.sh` | 见脚本 |
//! | 4 | Windows-gnu + wgpu 构建与离屏初始化 | [`offscreen_probe`] | `render-stack` |
//! | 5 | 字体子集实际体积与改名后 name 表 | `tools/subset-font.py`（不在本 crate） | — |
//! | 6 | 单基线快照 YIQ 容差校准 | [`offscreen_probe`] | `render-stack` |
//! | 7 | 贪心换行 + 挤压的排版质量 | [`hinting`] | `cosmic-text-stack` |
//!
//! 结论汇总写入 `spikes/s2/BASELINE.md`。
//!
//! ## 为什么依赖全部挂 feature（T007 的方法论要求）
//!
//! T007 要测「cosmic-text 全栈（与 wgpu 栈）接入带来的体积**净增量**」。
//! 有意义的测法是**对照**：空壳二进制 vs 接入后的二进制，两者差分才是
//! "接入的代价"。故依赖必须可关——默认全关 = 空壳臂。
//!
//! 这与 S0 spike 的 rquickjs 测法同构（其 `spike-js` feature 的注释里
//! 写明了同样的理由）。
//!
//! ```bash
//! cargo build --release                          # 空壳臂
//! cargo build --release --features cosmic-text-stack
//! cargo build --release --features render-stack
//! cargo build --release --features full-stack
//! ```

/// R5-1 / R5-4 / R5-7 的验证对象需要 cosmic-text。
#[cfg(feature = "cosmic-text-stack")]
pub mod hinting;

/// R5-3：体积测量的**方法论与口径**（不依赖任何可选栈）。
pub mod size;

/// R5-6：pixelmatch-YIQ 双闸门比对（零依赖，故不挂 feature）。
pub mod pixel_diff;

/// R5-4 / R5-6：离屏渲染探针需要渲染栈。
#[cfg(feature = "render-stack")]
pub mod offscreen_probe;

/// R5-1：ShapeWord 合并行为（**证伪门**）。
#[cfg(feature = "cosmic-text-stack")]
pub mod word_merge;
