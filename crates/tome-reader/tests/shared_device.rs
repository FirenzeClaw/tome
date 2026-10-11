//! T004 验收：**共享 device** 的渲染管线（S3 路径 A 的地基）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T004）
//!
//! - `RenderPipeline::from_shared_device(device, queue, target_format)` 接受外部
//!   device/queue 建管线，**不自建 Instance**
//! - 共享 device 构造的管线渲染一帧与离屏路径**逐像素一致**（同字体同内容）
//! - 离屏路径（`Rgba8Unorm` + readback）行为不变
//!
//! ## 为什么"逐像素一致"是这条的正确判据
//!
//! S3 的阅读视图走 `egui_wgpu` 的 PaintCallback，把 S2 的绘制**直接提交到
//! egui 的 render pass**（零拷贝）。这条路只有在"同一份绘制代码、同一份
//! device"时才对得上——若共享路径另写一套绘制，两者"看起来都对"但像素可能
//! 不同，而那种差异要到三平台 CI 快照阶段才会暴露，代价高得多。
//!
//! 故本文件把"同一份代码 + 同一个 device"变成可断言的两件事：
//!
//! 1. **同一个 device**：目标纹理在**自有管线**的 device 上创建，绘制却由
//!    共享管线执行。若 `from_shared_device` 偷偷自建了 device，wgpu 会拒绝
//!    跨 device 使用资源（校验错误）——这条因此不是形式断言。
//! 2. **逐像素一致**：同内容分别经两条路径渲染，读回的字节数组完全相等。
//!
//! ## 诚实跳过（FR-053）
//!
//! 无图形后端时显式打印 `SKIPPED` 且不计为通过——与 `render_pipeline.rs`
//! 同一口径。

#![cfg(feature = "render")]

use tome_core::model::theme::DayTheme;
use tome_reader::paginate::config::ReadingConfig;
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::render::batch::RenderBatch;
use tome_reader::render::glyph_cache::GlyphCache;
use tome_reader::render::offscreen::{read_texture_rgba, OffscreenRenderer, OFFSCREEN_FORMAT};
use tome_reader::render::pipeline::{GraphicsBackend, PipelineError, RenderPipeline};

/// 画布尺寸（与 `render_deterministic.rs` 同口径，便于两条路径比对）。
const CANVAS_W: u32 = 342;
const CANVAS_H: u32 = 300;

/// 取一次后端（无可用后端时按 FR-053 显式跳过）。
#[expect(
    clippy::panic,
    reason = "夹具内的 panic 是刻意的：非 NoAdapter 的失败必须让测试红，不得被当成跳过"
)]
fn backend_or_skip(context: &str) -> Option<GraphicsBackend> {
    match RenderPipeline::probe_backend() {
        Ok(backend) => Some(backend),
        Err(PipelineError::NoAdapter { reason }) => {
            println!("[T004] SKIPPED ({context}): 无可用图形后端——{reason}");
            println!("[T004] 本条**不计为通过**（FR-053 诚实跳过口径）");
            None
        }
        Err(other) => panic!("[T004] 后端探测以非'无适配器'的方式失败：{other}"),
    }
}

#[expect(
    clippy::expect_used,
    reason = "夹具只在探测已成功之后调用；失败说明装置坏了，应当让测试红"
)]
fn build_owned_pipeline(backend: GraphicsBackend) -> RenderPipeline {
    RenderPipeline::new(backend, CANVAS_W, CANVAS_H).expect("有后端时应能建离屏管线")
}

fn sample_chapter() -> String {
    [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样；她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然。」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
    ]
    .join("\n")
}

#[expect(
    clippy::expect_used,
    reason = "夹具内的 expect 是刻意的：样本排版失败说明装置坏了，应当让测试红"
)]
fn build_batch() -> RenderBatch {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(CANVAS_W as f32, CANVAS_H as f32);

    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    RenderBatch::for_chapter(&chapter, &geometry, &viewport, DayTheme.theme())
}

// ===========================================================================
// 构造形态
// ===========================================================================

/// 共享 device 构造**不产生尺寸**——尺寸由每帧的渲染目标决定。
///
/// ## 为什么这条值得单独断言
///
/// 离屏管线的尺寸是"建管线时就定死"的（它自建纹理）；共享 device 的渲染目标
/// 是 egui 的视口，**每帧可变**。若共享管线也携带一个尺寸，迟早会有人拿它去
/// 尺寸化纹理——那是个只在窗口缩放时才现形的错误。
#[test]
fn shared_device_pipeline_has_no_fixed_extent() {
    let Some(backend) = backend_or_skip("shared_device_pipeline_has_no_fixed_extent") else {
        return;
    };
    let owned = build_owned_pipeline(backend);

    // 离屏形态：尺寸固定且可读
    assert_eq!(owned.extent(), Some((CANVAS_W, CANVAS_H)));
    assert_eq!(owned.width(), Some(CANVAS_W));
    assert_eq!(owned.height(), Some(CANVAS_H));
    assert_eq!(owned.target_format(), OFFSCREEN_FORMAT);

    let shared = RenderPipeline::from_shared_device(
        owned.device().clone(),
        owned.queue().clone(),
        OFFSCREEN_FORMAT,
    );

    assert_eq!(
        shared.extent(),
        None,
        "共享 device 的管线不该携带固定尺寸（目标由每帧的渲染目标给出）"
    );
    assert_eq!(shared.width(), None);
    assert_eq!(shared.height(), None);
    assert_eq!(
        shared.target_format(),
        OFFSCREEN_FORMAT,
        "目标格式必须原样保留——它决定字形管线的 color target"
    );
}

/// 共享 device 构造**不建 Instance**，故后端信息来自 device 自身。
///
/// `RenderPipeline::new` 要自建 `Instance` → `request_adapter`（可能报
/// `NoAdapter`）；`from_shared_device` 拿到的是**已经建好的** device，它的
/// 后端信息只能从 `Device::adapter_info()` 取——这既是实现方式，也是
/// "没有第二次探测"的可观测后果。
#[test]
fn shared_device_reports_backend_from_the_device_itself() {
    let Some(backend) = backend_or_skip("shared_device_reports_backend_from_the_device_itself")
    else {
        return;
    };
    let owned = build_owned_pipeline(backend.clone());

    let shared = RenderPipeline::from_shared_device(
        owned.device().clone(),
        owned.queue().clone(),
        OFFSCREEN_FORMAT,
    );

    let reported = shared.backend();
    assert!(
        !reported.name().is_empty(),
        "后端名不得为空（探针与日志靠它判定跑了哪个后端）"
    );
    assert!(
        !reported.adapter_name().is_empty(),
        "适配器名不得为空（区分真 GPU 与 lavapipe/WARP 靠它）"
    );
    assert_eq!(
        reported.adapter_name(),
        backend.adapter_name(),
        "同一个 device 上读出的适配器名应与探测阶段一致"
    );
}

// ===========================================================================
// 逐像素一致（本任务的核心判据）
// ===========================================================================

/// 共享 device 的管线渲染一帧与离屏路径**逐像素一致**。
///
/// ## 目标纹理建在**自有管线**的 device 上
///
/// 这是"真的是同一个 device"的可失败证据：若 `from_shared_device` 自建了
/// device，`shared.draw_batch_into()` 拿到的纹理来自另一台 device，wgpu 会
/// 直接以校验错误拒绝（而不是悄悄画出一张错图）。
#[test]
fn shared_device_pipeline_renders_pixel_identical_frame() {
    let Some(backend) = backend_or_skip("shared_device_pipeline_renders_pixel_identical_frame")
    else {
        return;
    };
    let owned = build_owned_pipeline(backend);
    let shared = RenderPipeline::from_shared_device(
        owned.device().clone(),
        owned.queue().clone(),
        OFFSCREEN_FORMAT,
    );

    let batch = build_batch();

    // --- 路径 1：离屏渲染器（自建纹理 + 读回）---
    let mut offscreen_cache = GlyphCache::new();
    let offscreen_image = OffscreenRenderer::new(CANVAS_W, CANVAS_H)
        .expect("尺寸非零")
        .render(&owned, &batch, &mut offscreen_cache)
        .expect("离屏渲染应成功");

    // --- 路径 2：外部目标纹理（由**自有** device 建）+ 共享管线绘制 ---
    let target = owned.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("tome-shared-device-target"),
        size: wgpu::Extent3d {
            width: CANVAS_W,
            height: CANVAS_H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OFFSCREEN_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let mut shared_cache = GlyphCache::new();
    shared
        .draw_batch_into(&view, &batch, &mut shared_cache)
        .expect("共享 device 上绘制应成功");

    let shared_image =
        read_texture_rgba(owned.device(), owned.queue(), &target, CANVAS_W, CANVAS_H)
            .expect("读回应成功");

    // --- 判据 ---
    assert!(
        offscreen_image.is_consistent() && shared_image.is_consistent(),
        "两张图的像素长度都应与尺寸自洽"
    );
    assert_eq!(
        offscreen_image.foreground_pixels(background_rgb()),
        shared_image.foreground_pixels(background_rgb()),
        "两条路径的前景像素数应相同（先证'都真的画了东西'，再比逐像素）"
    );
    assert!(
        offscreen_image.foreground_pixels(background_rgb()) > 0,
        "前景像素为 0 说明两条路径都只清了底——那是假绿，不是一致"
    );
    assert_eq!(
        offscreen_image.pixels, shared_image.pixels,
        "共享 device 路径与离屏路径必须逐像素一致（同一份绘制代码 + 同一个 device）"
    );
}

/// 同一份绘制代码在**两次**调用间也稳定（本任务不引入新的不确定性）。
///
/// ## 为什么这条与上一条不重复
///
/// 上一条比的是"两条路径"。若两条路径**都**引入了同一种抖动（例如共享管线
/// 每次重建 atlas 而位图顺序不定），上一条仍会绿。这条用同一路径连跑两次，
/// 把那种形态挡在外面。
#[test]
fn shared_device_path_is_deterministic_across_calls() {
    let Some(backend) = backend_or_skip("shared_device_path_is_deterministic_across_calls") else {
        return;
    };
    let owned = build_owned_pipeline(backend);
    let shared = RenderPipeline::from_shared_device(
        owned.device().clone(),
        owned.queue().clone(),
        OFFSCREEN_FORMAT,
    );

    let batch = build_batch();
    let render_once = |cache: &mut GlyphCache| -> Vec<u8> {
        let target = owned.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("tome-shared-device-determinism"),
            size: wgpu::Extent3d {
                width: CANVAS_W,
                height: CANVAS_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        shared
            .draw_batch_into(&view, &batch, cache)
            .expect("绘制应成功");
        read_texture_rgba(owned.device(), owned.queue(), &target, CANVAS_W, CANVAS_H)
            .expect("读回应成功")
            .pixels
    };

    // 第一次：字形缓存是冷的（栅格化发生在这里）
    let mut cache = GlyphCache::new();
    let first = render_once(&mut cache);
    // 第二次：缓存已热
    let second = render_once(&mut cache);

    assert_eq!(
        first, second,
        "同一路径两次渲染必须逐字节一致（冷缓存与热缓存不得改变像素）"
    );
}

/// 阅读区底色（前景判据的比较基准）。
fn background_rgb() -> [u8; 3] {
    let color = DayTheme.theme().palette().bg_reading;
    [color.red, color.green, color.blue]
}
