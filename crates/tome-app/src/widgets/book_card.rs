//! `[WIP]` 书卡（S3 T012；MASTER `BookGrid` 的单元）
//!
//! ## 卡片的构成与依据
//!
//! MASTER §7 的 `BookGrid` 行给出三条硬要求：**封面 2:3 比例**、
//! **悬停浮起 + 阅读进度细线**、**连载状态徽记**。本组件实现前两条；
//! 徽记依赖书源的连载状态（网络书才有），属 US3，此处在结构上留位。
//!
//! ## 为什么封面缺失要走"占位形态"而不是一个纯色块
//!
//! spec 边缘情况与契约 V2 都要求"封面缺失走 MASTER 占位形态"。`bg_inset`
//! 的纯色块在视觉上与"封面加载失败"无法区分，而用户需要知道**是这本书没有
//! 封面，而不是应用坏了**。故占位块自带说明文字（文案取自文案表，本文件
//! 不写中文字面串）。
//!
//! ## 为什么是"给定矩形"而不是"顺序摆放"
//!
//! 书卡由虚拟化网格驱动（只实例化可视区）。顺序摆放的组件（`ui.add`）会让
//! egui 自己决定位置，而虚拟化需要**精确知道第 N 张卡画在哪**——否则滚动时
//! 卡片会跳。故本组件的签名收一个 `rect`。

use egui::{Response, Ui, Vec2};

use crate::engine::messages::ShelfEntry;
use crate::i18n;
use crate::theme_bridge::{BridgeTheme, SurfaceEmphasis};

/// 封面宽高比（2:3，MASTER `BookGrid`）。
pub const COVER_ASPECT: f32 = 2.0 / 3.0;

/// 卡片宽度（逻辑点）。
///
/// ## 为什么是固定宽而不是自适应
///
/// 虚拟化要能"算出第 N 张卡的位置"。自适应宽度会让位置计算依赖一轮布局
/// （"先量再排"），而那正是虚拟化最不该有的东西——它会在滚动时产生跳动。
/// 固定宽 + 由可用宽推导的列数，位置计算就是纯算术。
pub const CARD_WIDTH_PX: f32 = 132.0;

/// 卡片高度 = 封面高 + 标题/作者两行 + 行距。
pub const CARD_HEIGHT_PX: f32 = CARD_WIDTH_PX / COVER_ASPECT + 52.0;

/// 卡片尺寸（供虚拟化网格算位置）。
#[must_use]
pub fn card_size() -> Vec2 {
    Vec2::new(CARD_WIDTH_PX, CARD_HEIGHT_PX)
}

/// 画一张书卡（**在给定矩形内**）。
///
/// 返回该卡的响应（点击 = 打开这本书）。
///
/// ## 为什么"移除"不在这张卡上
///
/// FR-004 要求移除**需确认**。把移除做成卡角上的一个按钮，会让"手滑点到
/// 删除"变成常态（书卡的点击区本来就大）。故移除走选中 + `Del`（键盘路径，
/// FR-017），确认框由视图层弹。
pub fn draw(
    ui: &mut Ui,
    theme: &BridgeTheme,
    entry: &ShelfEntry,
    rect: egui::Rect,
    selected: bool,
) -> Response {
    let id = ui.id().with(("book-card", &entry.book_id));
    let response = ui.interact(rect, id, egui::Sense::click());

    let palette = theme.theme().palette();
    let radii = theme.theme().radii();

    // hover 浮起（MASTER：悬停浮起）
    let lift = if response.hovered() { -2.0 } else { 0.0 };
    let cover_rect = egui::Rect::from_min_size(
        rect.min + Vec2::new(0.0, lift),
        Vec2::new(CARD_WIDTH_PX, CARD_WIDTH_PX / COVER_ASPECT),
    );

    let painter = ui.painter();

    // --- 封面（或占位）---
    painter.rect_filled(
        cover_rect,
        egui::CornerRadius::same(radii.md_px as u8),
        theme.color(palette.bg_inset),
    );
    if entry.cover.is_none() {
        painter.text(
            cover_rect.center(),
            egui::Align2::CENTER_CENTER,
            i18n::t(i18n::key::SHELF_COVER_PLACEHOLDER),
            egui::FontId::proportional(theme.theme().type_scale().caption.size_px),
            theme.text_color_for(SurfaceEmphasis::Decorative),
        );
    }

    // --- 进度细线（MASTER：阅读进度细线，压在封面底边）---
    if entry.progress_percent > 0.0 {
        let fraction = (entry.progress_percent / 100.0).clamp(0.0, 1.0);
        let bar_height = 3.0;
        let bar_rect = egui::Rect::from_min_size(
            egui::pos2(cover_rect.left(), cover_rect.bottom() - bar_height),
            Vec2::new(cover_rect.width() * fraction, bar_height),
        );
        // 撞色高光（MASTER `ProgressBar`：细线式、`--accent-clash` 高光）
        painter.rect_filled(
            bar_rect,
            egui::CornerRadius::ZERO,
            theme.color(palette.accent_clash),
        );
    }

    // --- 选中环 / 焦点环 ---
    //
    // MASTER §2.3：焦点环 2px `--accent-primary` + 2px offset；**仅键盘来源
    // 显示**。`response.has_focus()` 在 egui 里恰恰只对键盘导航为真（鼠标点击
    // 后仍为 false），这正是那条规则的实现手段——不需要自己判断来源。
    if selected || response.has_focus() {
        painter.rect_stroke(
            rect.expand(theme.focus_ring_offset_px()),
            egui::CornerRadius::same(radii.md_px as u8),
            egui::Stroke::new(theme.focus_ring_width_px(), theme.focus_ring_color()),
            egui::StrokeKind::Outside,
        );
    }

    // --- 标题与作者 ---
    let title_top = cover_rect.bottom() + 6.0;
    painter.text(
        egui::pos2(rect.left(), title_top),
        egui::Align2::LEFT_TOP,
        &entry.title,
        egui::FontId::proportional(theme.theme().type_scale().sm.size_px),
        theme.text_color_for(SurfaceEmphasis::Normal),
    );

    let author = if entry.author.trim().is_empty() {
        i18n::t(i18n::key::SHELF_UNKNOWN_AUTHOR).to_owned()
    } else {
        entry.author.clone()
    };
    painter.text(
        egui::pos2(rect.left(), title_top + 20.0),
        egui::Align2::LEFT_TOP,
        i18n::card_meta(&author, entry.progress_percent),
        egui::FontId::proportional(theme.theme().type_scale().caption.size_px),
        theme.text_color_for(SurfaceEmphasis::Required),
    );

    response
}
