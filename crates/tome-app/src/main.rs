//! `[WIP]` tome 应用外壳入口（S3 T009：单实例 + 窗口配置 + eframe 启动）
//!
//! ## 本文件刻意很薄
//!
//! 全部逻辑在 [`tome_app::app`] 的 `TomeApp` / `ViewState` 里（那层是纯 Rust
//! 值，headless 可测）。这里只做四件必须在**进程入口**做的事：
//!
//! 1. **单实例判定**（第二实例唤醒已有窗口后立即退出）
//! 2. **窗口配置**（`ViewportBuilder`：标题、最小尺寸、app id）
//! 3. **组合根装配**（[`tome_app::composition`]：数据目录 → 库 → 引擎线程）
//! 4. **渲染资源注册**（`StageResources` 放进 egui 的 `callback_resources`）
//!
//! ## 单实例的时序（为什么是这个顺序）
//!
//! ```text
//!   notify_if_running(APP_ID)  ← 第二实例在这里发唤醒信号并退出
//!        ↓ false（本进程是主实例）
//!   run_native(...)
//!        ↓ 创建闭包内
//!   start_primary(APP_ID, cb)  ← 注册唤醒回调（必须在窗口存在之后）
//! ```
//!
//! `start_primary` **必须**在 `run_native` 的创建闭包里调用：回调要
//! `ctx.request_repaint()`，而 `egui::Context` 那时才存在。提前注册就得把
//! ctx 存起来再跨线程取——那条路会引入"回调在 ctx 就绪前触发"的竞态。
//!
//! ## 装配失败的呈现
//!
//! 组合根在**窗口创建之前**跑（它要建库、起引擎线程）。失败时不 panic，
//! 而是渲染一个结构化错误窗：标识 + 细节 + **指引**（FR-023 / S1 FR-043 的
//! "明确报告并给出指引，而非静默失败"）。一个只会 panic 的启动失败，用户
//! 看到的是"程序闪了一下就没了"——那是最难排查的一类。
//!
//! ## 为什么 `#![deny(unsafe_code)]`
//!
//! 与 `tome-store` 同形态：默认禁止 unsafe，确有需要时在**单点**写
//! `#[expect(unsafe_code)]` + SAFETY 注释，而不是全局放开。

#![deny(unsafe_code)]

use tome_app::app::TomeApp;
use tome_app::composition;

/// 单实例标识（named pipe / UDS 的名字，也是 Wayland 下关联 `.desktop` 的键）。
///
/// 用固定的反向域名串而非 `CARGO_PKG_NAME`：包名可能随重构变化，而单实例
/// 标识一旦变了，**旧版本实例与新版本实例会同时启动**——用户看到两个窗口，
/// 且两个进程写同一个数据目录（spec 边缘情况要防的正是这个）。
const APP_ID: &str = "com.firenzeclaw.tome";

fn main() -> eframe::Result<()> {
    // --- ① 单实例：已有实例则唤醒它并退出 ---
    if app_single_instance::notify_if_running(APP_ID) {
        return Ok(());
    }

    // --- ② 组合根：数据目录 → 库 → 接缝实现 → 引擎线程 ---
    let composition = match composition::assemble() {
        Ok(composition) => composition,
        Err(failure) => {
            // 启动失败：开一个**只有错误信息**的窗口（不静默、不 panic）
            return run_startup_failure(&failure);
        }
    };
    for warning in &composition.warnings {
        log::warn!("tome-app: 启动期诊断——{warning}");
    }
    let engine = composition.engine;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("tome")
            // 最小尺寸：低于它侧栏与版心都放不下（MASTER §6 的 768px 档）
            .with_min_inner_size([768.0, 520.0])
            .with_inner_size([1280.0, 840.0])
            // app_id 的双重作用：eframe 的持久化目录 key + Wayland 的图标分组
            .with_app_id(APP_ID),
        ..Default::default()
    };

    eframe::run_native(
        "tome",
        options,
        Box::new(move |creation_context| {
            // --- ③ 注册主实例（回调里只 request_repaint，真正的激活在 UI 帧里做）---
            let ctx = creation_context.egui_ctx.clone();
            let handle = app_single_instance::start_primary(APP_ID, move || {
                // 只唤醒：`ViewportCommand` 必须在 UI 帧里发（见 app.rs 的说明）
                ctx.request_repaint();
            });

            // --- ④ 渲染资源（路径 A 的核心：S2 的管线建在 egui 的 device 上）---
            register_stage_resources(creation_context);

            let mut app = TomeApp::new(creation_context);
            app.set_single_instance(handle);
            app.set_engine(engine);
            // 拉取式协议：启动时主动要书架与设置（见 `TomeApp::start` 的说明）
            app.start();
            Ok(Box::new(app))
        }),
    )
}

/// 把 `StageResources` 注册进 egui 渲染器（**路径 A 的必需步骤**）。
///
/// ## 为什么必须在这里、且必须显式注册
///
/// `PaintCallback` 的 `prepare` 阶段从 `CallbackResources`（egui 渲染器上的
/// `TypeMap`）里取常驻 GPU 资源。没有它，回调**画不出任何东西**——而症状是
/// "阅读区一片空白"，排查方向会从渲染逻辑找起（方向反了）。
/// `render/callback.rs` 的 `log_missing_stage` 就是为了让这件事可见。
///
/// ## `wgpu_render_state` 为 `None` 的情形
///
/// 只有非 wgpu 后端（如 glow）才为 `None`，而本 crate 钉的是 wgpu 渲染器
/// （`eframe` 的默认）。真发生时**不静默**：阅读区会显示能力错误态。
#[cfg(feature = "reader-render")]
fn register_stage_resources(creation_context: &eframe::CreationContext<'_>) {
    use tome_app::render::StageResources;

    let Some(render_state) = creation_context.wgpu_render_state.as_ref() else {
        log::error!("tome-app: eframe 未提供 wgpu 渲染状态——阅读区将无法绘制");
        return;
    };
    let resources = StageResources::new(
        render_state.device.clone(),
        render_state.queue.clone(),
        render_state.target_format,
    );
    render_state
        .renderer
        .write()
        .callback_resources
        .insert(resources);
}

/// headless 构建（`reader-render` 关闭）下没有渲染资源可注册。
#[cfg(not(feature = "reader-render"))]
fn register_stage_resources(_creation_context: &eframe::CreationContext<'_>) {}

/// 启动失败的窗口（**只有错误信息**，不进入正常界面）。
///
/// ## 为什么还要开一个窗口
///
/// 失败必须**可见**。无控制台的 GUI 进程（Windows 的常规形态）里，
/// `eprintln!` 与 `panic!` 都等于"什么都没说"——用户看到的是程序闪退。
/// 一个显示"标识 + 细节 + 指引"的窗口是唯一能让他采取行动的形式。
fn run_startup_failure(failure: &tome_app::engine::thread::StartupFailure) -> eframe::Result<()> {
    let message = failure.to_string();
    let guidance = failure.guidance().to_owned();
    let error_id = failure.error().id.clone();

    eframe::run_native(
        "tome — 启动失败",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("tome — 启动失败")
                .with_inner_size([720.0, 320.0]),
            ..Default::default()
        },
        Box::new(move |_creation_context| {
            Ok(Box::new(StartupFailureApp {
                message,
                guidance,
                error_id,
            }))
        }),
    )
}

/// 启动失败窗口的应用体。
struct StartupFailureApp {
    /// 完整的失败描述（标识 + 细节）
    message: String,
    /// 可行动的指引
    guidance: String,
    /// 错误标识（**永不翻译**，随文案一并呈现）
    error_id: String,
}

impl eframe::App for StartupFailureApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("tome 无法启动");
            ui.add_space(8.0);
            // 标识在前（可检索），细节在后
            ui.label(format!("[{}]", self.error_id));
            ui.label(&self.message);
            ui.add_space(12.0);
            ui.label(&self.guidance);
        });
    }
}
