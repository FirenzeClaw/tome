//! `[WIP]` 叠加层（S3 T013 的导入对话框 + T009 的设置骨架）
//!
//! ## 为什么叠加层独立成模块
//!
//! `data-model.md` §2 明确"overlays（正交叠加，不属于视图切换）"。把它做成
//! 视图的一个分支会迫使 `AppView` 从两态变成四态，而"书架 ⇄ 阅读互斥"这条
//! 不变量（DM §2 不变量 1）就没法用类型表达了——叠加层打开时，底下那个视图
//! 仍然是书架或阅读，它没有被替换。
//!
//! ## 导入的两条路径（FR-002 的"拖入与文件选择"）
//!
//! | 路径 | 触发 | 落点 |
//! |---|---|---|
//! | 拖入 | `ctx.input().raw.dropped_files`（`app.rs` 的 `logic`） | 直接发 `ShelfImportLocal` |
//! | 文件选择 | 本模块的"选择文件…"按钮 | 走 `rfd` 原生对话框 |
//!
//! 两条路最终汇到**同一个请求**——这不是巧合，而是"导入"这件事只有一条
//! 语义（解析 → 落库 → 发事件）。若两条路各自实现，重复导入识别、错误呈现、
//! 半成品回滚就会各写一遍。

use egui::Ui;

use crate::app::{Overlay, ViewState};
use crate::engine::messages::EngineRequest;
use crate::i18n;
use crate::theme_bridge::{BridgeTheme, SurfaceEmphasis};
use crate::widgets::skeleton;

/// 画当前叠加层，返回本帧产生的引擎请求。
pub fn draw(ui: &mut Ui, state: &mut ViewState, overlay: Overlay) -> Vec<EngineRequest> {
    let mut requests = Vec::new();
    let theme = BridgeTheme::from_theme(state.theme());
    let mut close = false;

    let title = match overlay {
        Overlay::Settings => i18n::t(i18n::key::SETTINGS_TITLE),
        Overlay::ImportLocal => i18n::t(i18n::key::IMPORT_LOCAL_TITLE),
        Overlay::ImportSource => i18n::t(i18n::key::IMPORT_SOURCE_TITLE),
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            match overlay {
                Overlay::Settings => draw_settings(ui, &theme, state),
                Overlay::ImportLocal => draw_import_local(ui, &theme, state, &mut requests),
                Overlay::ImportSource => {
                    ui.label(
                        egui::RichText::new(i18n::t(i18n::key::IMPORT_SOURCE_HINT))
                            .color(theme.text_color_for(SurfaceEmphasis::Required)),
                    );
                }
            }

            ui.add_space(theme.theme().spacing().sp_4 as f32);
            if ui.button(i18n::t(i18n::key::COMMON_CLOSE)).clicked() {
                close = true;
            }
        });

    if close {
        state.close_overlay();
    }
    requests
}

/// 本地导入对话框。
///
/// ## 为什么"选择文件…"用原生对话框
///
/// spec 的 US1 验收口径是"导入到开始阅读 ≤3 步"（SC-003）。一个自绘的文件
/// 浏览器会把这件事变成"在应用里导航目录"（远超 3 步），而原生对话框是
/// 用户已经会用的东西。代价是引入一个平台依赖（`rfd`），收益是**步数**。
fn draw_import_local(
    ui: &mut Ui,
    theme: &BridgeTheme,
    state: &mut ViewState,
    requests: &mut Vec<EngineRequest>,
) {
    ui.label(
        egui::RichText::new(i18n::t(i18n::key::IMPORT_LOCAL_HINT))
            .color(theme.text_color_for(SurfaceEmphasis::Required)),
    );

    if state.is_importing() {
        // 导入中：骨架（MOTION 规则下的静态形态——导入通常不足一秒，
        // 扫光反而会让人以为"卡住了"）
        skeleton::block(ui, theme, egui::vec2(220.0, 18.0), 0.0, true);
        ui.label(
            egui::RichText::new(i18n::t(i18n::key::IMPORT_LOCAL_BUSY))
                .color(theme.text_color_for(SurfaceEmphasis::Required)),
        );
        return;
    }

    if ui.button(i18n::t(i18n::key::IMPORT_LOCAL_CHOOSE)).clicked() {
        if let Some(path) = pick_local_book_file() {
            state.begin_import();
            requests.push(EngineRequest::ShelfImportLocal {
                path: path.display().to_string(),
            });
        }
    }
}

/// 设置叠加层（四组默认属 US4/T030，本切片只显示当前值）。
fn draw_settings(ui: &mut Ui, theme: &BridgeTheme, state: &ViewState) {
    let settings = state.settings();
    ui.label(
        egui::RichText::new(i18n::t(i18n::key::SETTINGS_HINT))
            .color(theme.text_color_for(SurfaceEmphasis::Required)),
    );
    ui.add_space(theme.theme().spacing().sp_2 as f32);
    ui.label(format!(
        "{}: {:?}",
        i18n::t(i18n::key::SETTINGS_THEME),
        settings.theme
    ));
    ui.label(format!(
        "{}: {}px",
        i18n::t(i18n::key::SETTINGS_DEFAULT_FONT_SIZE),
        settings.default_font_size_px
    ));
}

/// 弹出原生文件选择对话框（阻塞当前线程——它在用户做出选择之前不返回）。
///
/// ## 为什么阻塞在这里是可接受的
///
/// 原生模态对话框期间，用户**不可能**与应用交互（窗口被系统模态挡住），
/// 故冻结 UI 不会造成任何可感知的问题。这与"在 UI 线程跑 SQLite/HTTP"有
/// 本质区别：后者冻结的是一个**用户正在用**的界面。
///
/// 返回 `None` 表示用户取消——那是正常路径，不是错误。
fn pick_local_book_file() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .add_filter("电子书", &["txt", "epub", "mobi"])
        .add_filter("全部文件", &["*"])
        .pick_file()
}
