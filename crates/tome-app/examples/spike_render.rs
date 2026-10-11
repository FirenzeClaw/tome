//! S3-0 spike：eframe 窗口内 PaintCallback 跑通 S2 渲染管线（S3 T005，**高风险门**）
//!
//! ## 这个 spike 要回答的唯一问题
//!
//! > S2 的排版渲染管线能不能在 **eframe/egui 的 device** 上跑起来，
//! > 并把同一页画成**与离屏路径逐像素相同**的结果？
//!
//! 答"能"则 S3 的阅读视图走路径 A（PaintCallback 同 device 零拷贝）；
//! 答"不能"则须停下来由用户裁决是否启用 fallback 路径 B（readback 纹理）。
//! **本 spike 不得自行降级**——它只负责把答案与证据摆出来。
//!
//! ## 为什么"逐像素相同"是这里的判据（而不是"窗口里有东西"）
//!
//! "窗口里出现了一页字"是个**几乎无法证伪**的判据：离屏路径画错了也会出现
//! 一页字。真正要证的是**两条路径是同一份绘制代码、同一台 device**——那才
//! 是路径 A 的实质。故：
//!
//! 1. **headless 形态**：用 `egui_wgpu::RenderState::create` 建**与 eframe
//!    完全同一条**的 device 路径（同一个函数），走 PaintCallback 的
//!    `prepare`/`paint` 两阶段，再把结果读回与离屏路径逐像素比对。
//!    这一条能在**三平台 CI 上跑**，是本 spike 的自动化证据。
//! 2. **窗口形态**：真的开一个 eframe 窗口，把同一份绘制贴到窗口里，
//!    截图落盘供人工核对（本机实测；CI 上无 X11/Wayland 时不跑）。
//!
//! 两者用**同一份** `tome-app` 代码（`render/callback.rs`），故 headless 的
//! 绿灯对窗口形态有实际意义，而不是"另一个环境下另一段代码通过了"。
//!
//! ## 退出码（沿用 T049 的探针形态）
//!
//! | 码 | 含义 |
//! |---|---|
//! | 0 | 通过（打印 `SPIKE OK ...`） |
//! | 1 | **失败**（逐像素不一致 / 装置坏了）——job 直接红 |
//! | 3 | 无可用图形后端（打印 `SKIPPED: ...`）——CI 上同样**直接红** |
//!
//! 第 3 条是关键：CI runner 没配好后端时，"所有测试跳过"会让 job 看起来是绿的。
//! 与 T049 的 probe 一样，无后端必须是**失败**而非跳过。

use std::sync::Arc;

use eframe::egui_wgpu::{self, RendererOptions};
// 与 `tome-app` 库同一份 wgpu（`eframe::wgpu` = `egui_wgpu::wgpu`）
use eframe::wgpu;
use tome_app::render::StageResources;
use tome_core::model::theme::DayTheme;
use tome_reader::paginate::config::ReadingConfig;
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::render::batch::RenderBatch;
use tome_reader::render::glyph_cache::GlyphCache;
use tome_reader::render::offscreen::{read_texture_rgba, OffscreenRenderer};
use tome_reader::render::pipeline::{PipelineError, RenderPipeline};

/// 画布尺寸（与 `tests/shared_device.rs` 同口径）。
const CANVAS_W: u32 = 342;
const CANVAS_H: u32 = 300;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let headless = args.iter().any(|arg| arg == "--headless");
    let out_dir = arg_value(&args, "--out").map(std::path::PathBuf::from);
    let no_backend = args.iter().any(|arg| arg == "--no-backend");

    if headless {
        run_headless(out_dir.as_deref(), no_backend)
    } else {
        run_window()
    }
}

/// 取 `--flag value` 形式的值。
fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.get(index + 1).cloned()
}

// ===========================================================================
// headless：三平台 CI 的自动化证据
// ===========================================================================

/// 用 **eframe 同一条** device 创建路径跑 PaintCallback，与离屏路径逐像素比对。
///
/// ## 为什么用 `egui_wgpu::RenderState::create` 而不是自己建 device
///
/// `eframe` 内部就是调它（`eframe/src/native/wgpu_integration.rs`），参数也
/// 就是 `WgpuConfiguration::default()`。直接调它意味着本 spike 验证的是
/// **eframe 真正用的那条路径**，而不是"另一条看起来相似的路径"。
///
/// 副作用：这条路径需要 async（`RenderState::create` 是 `async fn`），故引
/// `pollster::block_on`——它同时是 eframe 自己的选择（eframe 的 `wgpu` feature
/// 依赖 `pollster`），故不引入新的运行时形态。
///
/// ## `--no-backend` 是**刻意的负例**
///
/// "无后端时退 3 而不是伪装通过"这条判据，若只能用"本机没有 GPU"触发，就
/// **无法被验证**——开发机有 GPU、CI 有 lavapipe，任何一处都走不到那条分支。
/// 传 `Backends::empty()` 即可确定性地走到它（与 S2 的
/// `probe_backend_with(Backends::empty())` 同一手法）。
fn run_headless(out_dir: Option<&std::path::Path>, no_backend: bool) -> std::process::ExitCode {
    println!("[spike] S3-0 headless：eframe 同源 device + PaintCallback");

    let instance = if no_backend {
        // 负例：实例的后端集清空 → `request_adapter` 必定失败。
        //
        // **注意**：只改 `WgpuConfiguration` 里的 `instance_descriptor.backends`
        // 是**无效**的——`RenderState::create` 把外部传入的 `instance` 直接用于
        // 适配器选择，配置里的 backends 只影响它打日志用的 `available_adapters`
        // 列表（实测：清了配置里的后端集，spike 照样在 Vulkan 上跑通）。
        // 故负例必须在**建 instance 时**就清空。
        println!("[spike] --no-backend：实例后端集已清空（负例，期望退 3）");
        wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::empty(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        })
    } else {
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle())
    };
    let config = egui_wgpu::WgpuConfiguration::default();

    let render_state = match pollster::block_on(egui_wgpu::RenderState::create(
        &config,
        &instance,
        None, // 无 surface：headless 形态
        RendererOptions::default(),
    )) {
        Ok(state) => state,
        Err(error) => {
            println!("SKIPPED: 无法建立 egui 渲染状态——{error}");
            return std::process::ExitCode::from(3);
        }
    };

    let device = render_state.device.clone();
    let queue = render_state.queue.clone();
    let target_format = render_state.target_format;

    let info = device.adapter_info();
    println!(
        "SPIKE backend={:?} adapter={} software={} format={:?}",
        info.backend,
        info.name,
        matches!(info.device_type, wgpu::DeviceType::Cpu),
        target_format
    );

    // --- 离屏路径（S2 现状）：自建 device + 自建纹理 + 读回 ---
    let batch = Arc::new(build_batch());
    let owned_backend = match RenderPipeline::probe_backend() {
        Ok(backend) => backend,
        Err(PipelineError::NoAdapter { reason }) => {
            println!("SKIPPED: 离屏路径无可用后端——{reason}");
            return std::process::ExitCode::from(3);
        }
        Err(other) => {
            println!("SPIKE FAIL 离屏后端探测异常：{other}");
            return std::process::ExitCode::from(1);
        }
    };

    // 两条路径必须落在**同一块适配器**上，逐像素比对才有意义。
    //
    // ## 为什么这条守卫是必要的而不是多余的
    //
    // egui 建 device 用 `PowerPreference` 默认值，S2 的 `RenderPipeline::new`
    // 用 `LowPower`——**双显卡机器**上两者可能选到不同适配器（如 iGPU 与
    // dGPU），此时像素差异来自硬件而不是"路径 A 不成立"，报"路径 A 失败"
    // 就是误诊。CI runner 只有一块适配器，故本守卫在 CI 上恒真；它的价值
    // 在开发机（本机实测：单适配器，恒真）。
    if owned_backend.adapter_name() != info.name {
        println!(
            "SPIKE FAIL 两条路径落在不同适配器上（egui={} / S2={}）——\
             本比对在双显卡机器上无意义，请用 WGPU_ADAPTER_NAME 或单卡环境重跑",
            info.name,
            owned_backend.adapter_name()
        );
        return std::process::ExitCode::from(1);
    }

    let owned = match RenderPipeline::new(owned_backend, CANVAS_W, CANVAS_H) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            println!("SPIKE FAIL 建离屏管线失败：{error}");
            return std::process::ExitCode::from(1);
        }
    };
    let mut offscreen_cache = GlyphCache::new();
    let offscreen_image = match OffscreenRenderer::new(CANVAS_W, CANVAS_H)
        .and_then(|renderer| renderer.render(&owned, &batch, &mut offscreen_cache))
    {
        Ok(image) => image,
        Err(error) => {
            println!("SPIKE FAIL 离屏渲染失败：{error}");
            return std::process::ExitCode::from(1);
        }
    };

    // --- 路径 A：egui device 上的 PaintCallback ---
    let mut callback_resources = egui_wgpu::CallbackResources::default();
    callback_resources.insert(StageResources::new(
        device.clone(),
        queue.clone(),
        target_format,
    ));

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tome-spike-target"),
        size: wgpu::Extent3d {
            width: CANVAS_W,
            height: CANVAS_H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // **与 egui 的 target_format 一致**：字形管线的 color target 必须匹配
        format: target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());

    let callback = tome_app::render::GlyphCallback::new(Arc::clone(&batch));
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("tome-spike-encoder"),
    });
    {
        let descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [CANVAS_W, CANVAS_H],
            pixels_per_point: 1.0,
        };
        use egui_wgpu::CallbackTrait as _;
        let _command_buffers = callback.prepare(
            &device,
            &queue,
            &descriptor,
            &mut encoder,
            &mut callback_resources,
        );
    }
    queue.submit(Some(encoder.finish()));

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("tome-spike-paint-encoder"),
    });
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tome-spike-paint-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(background_clear(&batch)),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        use egui_wgpu::CallbackTrait as _;
        // `forget_lifetime()` 是 `egui_wgpu` 自己也在用的那一步（`winit.rs`：
        // `renderer.render(&mut render_pass.forget_lifetime(), ...)`）——
        // `CallbackTrait::paint` 的签名要求 `RenderPass<'static>`。
        // 代价是丢掉"pass 存活时不得再用 encoder"的编译期保护，故必须让
        // pass 的作用域严格早于 `encoder.finish()`（下面这个块就是为此）。
        callback.paint(
            egui::PaintCallbackInfo {
                viewport: egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(CANVAS_W as f32, CANVAS_H as f32),
                ),
                clip_rect: egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(CANVAS_W as f32, CANVAS_H as f32),
                ),
                pixels_per_point: 1.0,
                screen_size_px: [CANVAS_W, CANVAS_H],
            },
            &mut pass.forget_lifetime(),
            &callback_resources,
        );
    }
    queue.submit(Some(encoder.finish()));

    let painted = match read_texture_rgba(&device, &queue, &target, CANVAS_W, CANVAS_H) {
        Ok(image) => image,
        Err(error) => {
            println!("SPIKE FAIL 路径 A 读回失败：{error}");
            return std::process::ExitCode::from(1);
        }
    };

    // --- 比对 ---
    //
    // 判据分两步，顺序刻意：先证"两条路径都真的画了东西"，再比逐像素。
    // 只比字节的话，一个"两边都空白"的实现也会通过。
    let background = [
        batch.background().color.red,
        batch.background().color.green,
        batch.background().color.blue,
    ];
    let offscreen_foreground = offscreen_image.foreground_pixels(background);
    let painted_foreground = painted.foreground_pixels(background);

    println!(
        "SPIKE foreground offscreen={offscreen_foreground} paintcallback={painted_foreground}"
    );

    if offscreen_foreground == 0 {
        println!("SPIKE FAIL 离屏路径前景像素为 0——装置坏了（不是路径 A 的问题）");
        return std::process::ExitCode::from(1);
    }
    if painted_foreground == 0 {
        println!("SPIKE FAIL 路径 A 前景像素为 0——PaintCallback 没有画出任何字形");
        return std::process::ExitCode::from(1);
    }

    let mismatches = count_mismatches(&offscreen_image.pixels, &painted.pixels);
    println!(
        "SPIKE mismatched_pixels={mismatches} of {}",
        offscreen_image.pixels.len()
    );

    if let Some(dir) = out_dir {
        write_artifacts(dir, &offscreen_image, &painted);
    }

    if mismatches != 0 {
        println!(
            "SPIKE FAIL 路径 A 与离屏路径像素不一致（{mismatches} 字节不同）——\
             路径 A 的「同一份绘制代码」前提不成立"
        );
        return std::process::ExitCode::from(1);
    }

    println!("SPIKE OK paintcallback matches offscreen pixel-for-pixel");
    std::process::ExitCode::SUCCESS
}

/// 逐字节不同的数量。
fn count_mismatches(left: &[u8], right: &[u8]) -> usize {
    if left.len() != right.len() {
        // 长度不同本身就是"不一致"的最强形态；用长度差表示
        return left.len().abs_diff(right.len()).max(1);
    }
    left.iter().zip(right).filter(|(a, b)| a != b).count()
}

/// 底色清值。
fn background_clear(batch: &RenderBatch) -> wgpu::Color {
    let background = batch.background().color;
    wgpu::Color {
        r: f64::from(background.red) / 255.0,
        g: f64::from(background.green) / 255.0,
        b: f64::from(background.blue) / 255.0,
        a: 1.0,
    }
}

/// 落盘两张 PNG 供人工核对（CI 上作为 artifact 上传）。
fn write_artifacts(
    dir: &std::path::Path,
    offscreen: &tome_reader::render::offscreen::RenderedImage,
    painted: &tome_reader::render::offscreen::RenderedImage,
) {
    if let Err(error) = std::fs::create_dir_all(dir) {
        println!("[spike] 无法建产物目录 {}：{error}", dir.display());
        return;
    }
    for (name, image) in [("offscreen", offscreen), ("paintcallback", painted)] {
        let path = dir.join(format!("s3-spike-{name}.png"));
        let png = tome_cli::png::encode_png(image.width, image.height, &image.pixels);
        match std::fs::write(&path, png) {
            Ok(()) => println!("[spike] 已写出 {}", path.display()),
            Err(error) => println!("[spike] 写 {} 失败：{error}", path.display()),
        }
    }
}

// ===========================================================================
// 窗口：本机人工核对
// ===========================================================================

/// 真开一个 eframe 窗口，把同一页贴进去。
///
/// CI 的 Linux runner 无 X11/Wayland，故这一步**不在 CI 上跑**——它在
/// headless 证据之外补"窗口里真的能看"这一层。
fn run_window() -> std::process::ExitCode {
    println!("[spike] S3-0 窗口形态：eframe 窗口 + PaintCallback");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("tome S3-0 spike")
            .with_inner_size([640.0, 420.0]),
        ..Default::default()
    };

    match eframe::run_native(
        "tome-s3-spike",
        options,
        Box::new(|creation_context| Ok(Box::new(SpikeApp::new(creation_context)))),
    ) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            println!("SPIKE FAIL 窗口形态启动失败：{error}");
            std::process::ExitCode::from(1)
        }
    }
}

/// 窗口形态的应用：注册 StageResources，每帧把同一页画进窗口。
struct SpikeApp {
    batch: Arc<RenderBatch>,
    registered: bool,
}

impl SpikeApp {
    fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        let mut registered = false;
        if let Some(render_state) = creation_context.wgpu_render_state.as_ref() {
            let resources = StageResources::new(
                render_state.device.clone(),
                render_state.queue.clone(),
                render_state.target_format,
            );
            println!(
                "[spike] 窗口 device：{:?} adapter={} format={:?}",
                resources.pipeline().backend().name(),
                resources.pipeline().backend().adapter_name(),
                resources.target_format()
            );
            render_state
                .renderer
                .write()
                .callback_resources
                .insert(resources);
            registered = true;
        } else {
            println!("SPIKE FAIL 本窗口没有 wgpu render state（eframe 未跑 wgpu 后端？）");
        }

        Self {
            batch: Arc::new(build_batch()),
            registered,
        }
    }
}

impl eframe::App for SpikeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("S3-0 spike：PaintCallback 内跑 S2 渲染管线");
            if !self.registered {
                ui.colored_label(egui::Color32::RED, "wgpu render state 缺失——见控制台输出");
                return;
            }
            let response = tome_app::render::draw_reading_area(ui, Arc::clone(&self.batch));
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
        });
    }
}

// ===========================================================================
// 样本
// ===========================================================================

/// 合成一页正文（中英混排 + 标点，覆盖常见整形路径）。
#[expect(
    clippy::expect_used,
    reason = "spike 的样本排版失败说明装置坏了，当场 panic 好过继续跑出假结论"
)]
fn build_batch() -> RenderBatch {
    let content = [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样；她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然。」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
    ]
    .join("\n");

    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(CANVAS_W as f32, CANVAS_H as f32);

    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    RenderBatch::for_chapter(&chapter, &geometry, &viewport, DayTheme.theme())
}
