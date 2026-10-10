//! # S2-0 可行性 spike（tasks.md 002 T005-T009）
//!
//! 一次性验证骨架，**刻意不进主 workspace**（见 `Cargo.toml` 文件头）。
//! 目标是"die 早 die 便宜"：把 research.md R5 的七项遗留风险在写生产代码
//! **之前**变成实测数据。
//!
//! ## 七项与落点
//!
//! | R5 | 验证项 | 落点 |
//! |---|---|---|
//! | 1 | ShapeWord 合并后 `layout()` 的行为 | [`word_merge`]（**证伪门**） |
//! | 2 | hinting 对 `x_advance` 挤压的量化影响 | [`hinting`] |
//! | 3 | cosmic-text 全栈体积净增量 | [`size`] + `scripts/measure-s2-spike-size.sh` |
//! | 4 | Windows-gnu + wgpu/lavapipe 构建与离屏初始化 | [`offscreen_probe`] |
//! | 5 | 字体子集实际体积与改名后 name 表 | `tools/subset-font.py`（不在本 crate） |
//! | 6 | 单基线快照 YIQ 容差校准 | [`offscreen_probe`] |
//! | 7 | 贪心换行 + 挤压的排版质量 | [`hinting`] |
//!
//! 结论汇总写入 `spikes/s2/BASELINE.md`。
//!
//! ## 判据纪律（对齐 S0 spike 的既有做法）
//!
//! - 所有数字均为**本机实测**，命令与原始输出记录在 BASELINE.md
//! - 未实测的项明确标注"遗留"，不以推断冒充实测
//! - 证伪路径**不硬闯**：T005 若证伪即停止并上报 PM（备选方案 C 须用户批准）

pub mod hinting;
pub mod offscreen_probe;
pub mod size;
pub mod word_merge;
