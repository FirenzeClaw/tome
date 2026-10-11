//! `[WIP]` S2 渲染管线嵌入 egui 的 PaintCallback（S3 T005 / 路径 A）
//!
//! ## 三阶段的对应关系
//!
//! `egui_wgpu::CallbackTrait` 的三阶段与 S2 的绘制流程正好对上：
//!
//! | egui 阶段 | 本模块做什么 | 为什么必须在这一阶段 |
//! |---|---|---|
//! | `prepare` | `GlyphRenderer::prepare`：栅格化 + 图集 `write_texture` + 建管线 | `write_texture` 必须在 render pass **之外**，且早于引用它的绘制命令 |
//! | `finish_prepare` | 不做（无跨回调共享的上传） | — |
//! | `paint` | `GlyphRenderer::draw`：在 egui 的 render pass 上提交绘制 | 只有这一阶段拿得到 `RenderPass` |
//!
//! ## GPU 资源放哪里
//!
//! `prepare` 的签名是 `&self`（回调值本身是不可变的、每帧新建的），故跨帧
//! 持有的资源（管线、字形缓存、当帧备好的 `GlyphRenderer`）必须放进
//! `CallbackResources`——那是 egui `Renderer` 上的 `TypeMap`，生命周期跟随
//! egui 渲染器本身。
//!
//! ## 静态帧的零重绘
//!
//! 回调只在 egui 决定重绘时被调用；`content_version` 未变时上层不提交新回调
//! （T014 的纹理缓存）。本模块不做缓存决策——它只保证"被调用时画对"。

use std::sync::Arc;

use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
// `eframe::wgpu` 是 `egui_wgpu::wgpu` 的再导出——**与 tome-reader 用的是同一份**
// wgpu（semver 30.x 合并为单实例，见 Cargo.toml 的说明）。故两侧的类型互通，
// 无需任何转换；这也正是路径 A 成立的前提。
use eframe::wgpu;
use tome_reader::render::batch::RenderBatch;
use tome_reader::render::glyph_atlas::GlyphRenderer;
use tome_reader::render::glyph_cache::GlyphCache;
use tome_reader::render::pipeline::RenderPipeline;

/// egui 渲染器上常驻的 GPU 侧资源（跨帧持有）。
///
/// ## 为什么是"一个包"而不是三个独立条目
///
/// 三者必须来自**同一台 device**且生命周期一致（管线决定目标格式，缓存与
/// 渲染器都建在它上面）。分成三个 `TypeMap` 条目会让"谁先建"变成隐式约定，
/// 而那是能被静默破坏的——合成一个类型后，顺序由 [`Self::new`] 一处定死。
pub struct StageResources {
    /// 建在 egui device 上的 S2 管线（路径 A 的核心）。
    pipeline: RenderPipeline,
    /// 字形栅格化缓存（跨帧复用，避免每帧重新栅格化）。
    cache: GlyphCache,
    /// 本帧备好的字形绘制资源（`prepare` 写、`paint` 读）。
    renderer: Option<GlyphRenderer>,
}

impl StageResources {
    /// 在 egui 的 device/queue 上建常驻资源。
    #[must_use]
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            pipeline: RenderPipeline::from_shared_device(device, queue, target_format),
            cache: GlyphCache::new(),
            renderer: None,
        }
    }

    /// 共享 device 上的 S2 管线（供上层查询后端/格式）。
    #[must_use]
    pub fn pipeline(&self) -> &RenderPipeline {
        &self.pipeline
    }

    /// 目标格式（字形管线的 color target 必须与它一致）。
    #[must_use]
    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.pipeline.target_format()
    }
}

/// 一帧的排版快照，交给 egui 在它的 render pass 上绘制。
///
/// `Arc` 而非持有所有权：`PaintCallback` 自身要求 `Clone`（`epaint` 的
/// 注释写明"Shape has to be clone, therefore this has to be an Arc"），
/// 而 `RenderBatch` 含 `Vec`，克隆它意味着每帧复制整页绘制指令。
pub struct GlyphCallback {
    batch: Arc<RenderBatch>,
}

impl GlyphCallback {
    /// 以一份排版快照构造回调。
    #[must_use]
    pub fn new(batch: Arc<RenderBatch>) -> Self {
        Self { batch }
    }
}

impl CallbackTrait for GlyphCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(stage) = callback_resources.get_mut::<StageResources>() else {
            // 资源没注册就画不了。**不静默**：让它在日志里可见，
            // 否则表现为"阅读区一片空白"，而排查会从渲染层找起（方向反了）。
            log_missing_stage();
            return Vec::new();
        };

        let format = stage.pipeline.target_format();
        // 三个字段互不重叠，故可同时借用（`pipeline` 只读、`cache` 与
        // `renderer` 各自可变）。
        match GlyphRenderer::prepare(device, queue, format, &self.batch, &mut stage.cache) {
            Ok(renderer) => stage.renderer = renderer,
            Err(error) => {
                // 栅格化/上传失败：本帧不画字形，但**不 panic**——阅读视图
                // 该显示错误态，而不是让整个 GUI 进程崩掉（spec 边缘情况）。
                log_prepare_failure(&error);
                stage.renderer = None;
            }
        }

        // 不返回自有 command buffer：图集上传走 `queue.write_texture`
        // （wgpu 保证它先于其后提交的命令生效），管线与实例缓冲在 `paint`
        // 阶段随 render pass 一起提交。
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let Some(stage) = callback_resources.get::<StageResources>() else {
            return;
        };
        if let Some(renderer) = &stage.renderer {
            renderer.draw(render_pass);
        }
    }
}

/// 把一帧排版结果排进当前 `Ui`，并挂上 PaintCallback。
///
/// ## 尺寸为什么除以 `pixels_per_point`
///
/// `allocate_exact_size` 的入参是**逻辑点**，而 `egui_wgpu` 会把回调矩形
/// 换算成**物理像素**再设 viewport。S2 的裁剪空间换算是以画布**物理像素**
/// 为基准的（`batch.background().width/height`），故这里按 `ppp` 折算，
/// 让"逻辑点 → 物理像素"这一步是恒等的——否则高分屏上字会被拉伸。
///
/// ## 为什么由本函数分配矩形（而不是让调用方传 rect）
///
/// 矩形尺寸**必须**等于批次的画布尺寸（否则 S2 的裁剪空间换算与 viewport
/// 不一致，字会错位）。把它收在这里，"尺寸从哪来"就只有一处答案。
#[must_use]
pub fn draw_reading_area(ui: &mut egui::Ui, batch: Arc<RenderBatch>) -> egui::Response {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let canvas = batch.background();
    let size_in_points = egui::vec2(
        canvas.width as f32 / pixels_per_point,
        canvas.height as f32 / pixels_per_point,
    );

    let (rect, response) = ui.allocate_exact_size(size_in_points, egui::Sense::hover());
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        rect,
        GlyphCallback::new(batch),
    ));
    response
}

/// 资源未注册的可见报告（**不静默**）。
///
/// 用 `log` 而非 `eprintln!`：GUI 进程的 stdout/stderr 在 Windows 上默认
/// 不可见（无控制台），日志才是能落到文件的通道。
fn log_missing_stage() {
    log::error!(
        "tome-app: StageResources 未注册到 egui 渲染器——阅读区将不被绘制。\
         创建时须把 StageResources::new(...) 放进 wgpu_render_state.renderer 的 callback_resources"
    );
}

/// 字形准备失败的可见报告（本帧跳过字形，不 panic）。
fn log_prepare_failure(error: &tome_reader::render::pipeline::PipelineError) {
    log::error!("tome-app: 字形准备失败，本帧不绘制字形——{error}");
}

/// 回调实现的静态性质：`CallbackTrait` 要求 `Send + Sync`。
///
/// ## 为什么值得一条编译期断言
///
/// `GlyphCallback` 只含一个 `Arc<RenderBatch>`，而 `RenderBatch` 全是值类型
/// ——它**应当**是 `Send + Sync` 的。若哪天有人往 `RenderBatch` 里塞了
/// `Rc`/裸指针，这条断言会在编译期炸掉，而不是等到某个平台上偶发崩溃。
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GlyphCallback>();
    assert_send_sync::<StageResources>();
};
