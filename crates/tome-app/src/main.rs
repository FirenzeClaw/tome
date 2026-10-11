//! `[WIP]` tome 应用外壳入口（S3 T001：crate 骨架，空窗口可启动）
//!
//! ## 本任务的范围
//!
//! T001 只交付「crate 落地 + 空 egui 窗口可开关 + 双形态构建通过」。
//! 单实例、`ViewportBuilder` 尺寸、视图状态机在 T009 落地——届时本文件
//! 收缩为薄壳（单实例判定 → `run_native`），视图与状态迁到 `app.rs`。
//!
//! ## 为什么 `#![deny(unsafe_code)]`
//!
//! 与 `tome-store` 同形态：默认禁止 unsafe，确有需要时（如后续接 Win32
//! 窗口激活）在**单点**写 `#[expect(unsafe_code)]` + SAFETY 注释，
//! 而不是全局放开。

#![deny(unsafe_code)]

/// 空窗口应用（T001 的占位；T009 起由 `app.rs` 的 `TomeApp` 取代）。
struct EmptyShell;

impl eframe::App for EmptyShell {
    /// eframe 0.34 起 `App` 拆为 `ui` + `logic` 两方法，**旧的单 `update`
    /// 形态不再编译**（research-eframe.md Q1）。此处只实现必需的 `ui`。
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                ui.label("tome");
            });
        });
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "tome",
        eframe::NativeOptions::default(),
        Box::new(|_creation_context| Ok(Box::new(EmptyShell))),
    )
}
