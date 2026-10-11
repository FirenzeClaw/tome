//! `[WIP]` tome 应用外壳入口（S3 T009：单实例 + 窗口配置 + eframe 启动）
//!
//! ## 本文件刻意很薄
//!
//! 全部逻辑在 [`tome_app::app`] 的 `TomeApp` / `ViewState` 里（那层是纯 Rust
//! 值，headless 可测）。这里只做三件必须在**进程入口**做的事：
//!
//! 1. **单实例判定**（第二实例唤醒已有窗口后立即退出）
//! 2. **窗口配置**（`ViewportBuilder`：标题、最小尺寸、app id）
//! 3. **引擎句柄的构造与移交**（启动失败 → 渲染结构化错误页，不静默）
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
//! ## 为什么 `#![deny(unsafe_code)]`
//!
//! 与 `tome-store` 同形态：默认禁止 unsafe，确有需要时（如后续接 Win32
//! 窗口激活）在**单点**写 `#[expect(unsafe_code)]` + SAFETY 注释，
//! 而不是全局放开。

#![deny(unsafe_code)]

use tome_app::app::TomeApp;

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
        Box::new(|creation_context| {
            // --- ② 注册主实例（回调里只 request_repaint，真正的激活在 UI 帧里做）---
            let ctx = creation_context.egui_ctx.clone();
            let handle = app_single_instance::start_primary(APP_ID, move || {
                // 只唤醒：`ViewportCommand` 必须在 UI 帧里发（见 app.rs 的说明）
                ctx.request_repaint();
            });

            let mut app = TomeApp::new(creation_context);
            app.set_single_instance(handle);
            Ok(Box::new(app))
        }),
    )
}
