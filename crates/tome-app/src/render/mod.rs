//! `[WIP]` tome-app 渲染嵌入层（S3 路径 A：PaintCallback 同 device 零拷贝）
//!
//! ## 这一层解决什么
//!
//! S2 的排版渲染层（`tome-reader`）自带 wgpu 管线；S3 的外壳跑在 eframe/egui
//! 上，egui 也有自己的 wgpu device。两者要共用**同一台 device**才能把 S2 的
//! 绘制直接提交到 egui 的 render pass 里（零拷贝）。本模块是那条缝：
//!
//! ```text
//!   eframe 建 device（CreationContext::wgpu_render_state）
//!     ↓ 注入
//!   RenderPipeline::from_shared_device(device, queue, target_format)
//!     ↓ 每帧
//!   GlyphCallback::prepare → GlyphRenderer（栅格化 + 图集上传 + 管线）
//!     ↓
//!   GlyphCallback::paint   → 在 egui 的 render pass 上 draw
//! ```
//!
//! ## feature 门控
//!
//! 整个模块在 `reader-render` feature 下才存在——`--no-default-features`
//! 构建里 `tome-reader` 的渲染面在编译期消失，本模块随之消失
//! （workspace 的 `--no-default-features` 门不得因此变红）。
//!
//! ## 视口与坐标系（实测钉版）
//!
//! `egui_wgpu` 在调用 `CallbackTrait::paint` **之前**会把 render pass 的
//! viewport 设成回调矩形（`renderer.rs` 的 `set_viewport(...)`，注释写明是
//! "a courtesy for the user"）。故 NDC 的 `(-1, 1)` 对应**回调矩形**而不是
//! 整个屏幕——S2 那套"以画布左上角为原点换算裁剪空间"的算法可以直接复用，
//! 不需要任何偏移补偿。

pub mod callback;

pub use callback::{draw_reading_area, GlyphCallback, StageResources};
