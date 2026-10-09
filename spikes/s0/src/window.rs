//! eframe 开窗口验证（T006 验证项）
//!
//! 验证"三平台能构建并启动图形窗口"（FR-040）。注意这是 **spike 专用**：
//! 真正的界面外壳是 S3（tome-app），且引擎侧（tome-core 及以下）**绝不能**
//! 依赖图形环境（FR-036）——这正是 spike 必须脱离主 workspace 的原因之一。
//!
//! Linux 上需要 X11 或 Wayland；CI 的无头 runner 跑不了本路径，
//! 故 `--headless` 是 CI 的路径，本模块留给有图形环境的机器。

/// 开一个最小窗口，进入事件循环。
pub fn open() -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([SPIKE_WINDOW_WIDTH, SPIKE_WINDOW_HEIGHT])
            .with_title("tome S0 spike"),
        ..Default::default()
    };

    eframe::run_native(
        "tome S0 spike",
        options,
        Box::new(|_creation_context| Ok(Box::new(SpikeApp::default()))),
    )
    .map_err(|error| format!("eframe 启动失败（无图形环境时属预期）：{error}"))
}

/// 最小应用状态。窗口显示技术栈自检结论即可——spike 的窗口只需证明
/// "能开起来"，不需要任何交互。
#[derive(Default)]
struct SpikeApp {
    window_opened_at: Option<std::time::Instant>,
}

impl eframe::App for SpikeApp {
    // eframe 0.36 的 App trait 形态是 `logic`（每帧逻辑）+ `ui`（绘制），
    // 0.31 及更早的单一 `update(&mut self, ctx, frame)` 已不再存在。
    // 选 `ui`：spike 只需证明能画出内容。
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        // 首帧记录时间戳：启动耗时 = 进程开始 → 首帧绘制完成，
        // 这个差值稳定后即写入 BASELINE.md。
        let opened_at = *self
            .window_opened_at
            .get_or_insert_with(std::time::Instant::now);

        ui.heading("tome S0 spike");
        ui.label("eframe 窗口可用");
        ui.label(format!("首帧已绘制，窗口存活 {:?}", opened_at.elapsed()));
    }
}

/// spike 窗口初始宽度（逻辑像素）
const SPIKE_WINDOW_WIDTH: f32 = 480.0;
/// spike 窗口初始高度（逻辑像素）
const SPIKE_WINDOW_HEIGHT: f32 = 240.0;
