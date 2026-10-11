//! `[WIP]` 书架视图（S3 T012）
//!
//! ## 契约（`contracts/ui.md` §1.1）
//!
//! | # | 状态 | 本模块的落点 |
//! |---|---|---|
//! | V1 | 空书架 | [`widgets::empty`] + 两条**真的动作**（打开导入叠加层） |
//! | V2 | 有书 | 虚拟化网格 + 过滤框 + 排序切换；封面缺失走占位形态 |
//! | V3 | 搜索中 | US3（T025），本模块只留入口 |
//! | V4 | 详情 | US3（T026） |
//!
//! ## 三种"没内容"必须区分开（本模块最容易被写错的地方）
//!
//! | 状态 | 判据 | 呈现 |
//! |---|---|---|
//! | 加载中 | `!is_loaded()` | 骨架屏（还不知道有没有书） |
//! | 空书架 | `is_loaded() && is_empty()` | 空态引导（V1） |
//! | 过滤无结果 | 有书但 `visible()` 为空 | "没有匹配的书"（**不是**空态） |
//!
//! 把它们合并的后果很具体：用户在过滤框里打错一个字，界面会说"书架是空的，
//! 请导入一本书"——那是在撒谎，而且他会去找"书去哪了"。
//!
//! ## 虚拟化（SC-005 的落点）
//!
//! 只实例化可视区的卡片。几何计算在 [`crate::state::shelf::GridLayout`]（纯值，
//! headless 可测），本模块只做"按它给的矩形画"。分开的理由见那个类型的文档。

use egui::Ui;

use crate::app::{Overlay, ViewState};
use crate::engine::messages::EngineRequest;
use crate::i18n;
use crate::state::shelf::GridLayout;
use crate::theme_bridge::{BridgeTheme, SurfaceEmphasis};
use crate::widgets::{book_card, empty, skeleton};

/// 画书架视图，返回本帧产生的引擎请求。
///
/// ## 为什么返回请求而不是自己发
///
/// 视图拿不到 `EngineHandle`（状态机是纯值，见 `app.rs` 的说明）。把请求
/// 作为返回值让"这一帧的界面动作产生了哪些引擎请求"变成可断言的量——而
/// 这正是 T012/T013 的验收子项要测的东西。
pub fn draw(ui: &mut Ui, state: &mut ViewState) -> Vec<EngineRequest> {
    let mut requests = Vec::new();
    let theme = BridgeTheme::from_theme(state.theme());

    if !state.shelf().is_loaded() {
        // 首次加载：骨架屏（此时"有没有书"还不知道，故不能显示空态）
        ui.vertical_centered(|ui| {
            ui.add_space(theme.theme().spacing().sp_10 as f32);
            skeleton::block(
                ui,
                &theme,
                egui::vec2(220.0, 28.0),
                0.0,
                state.params().font_size_px <= 0.0,
            );
        });
        return requests;
    }

    if state.shelf().is_empty() {
        // V1：空态必有引导动作（契约断言）
        let response = empty::draw(
            ui,
            &theme,
            i18n::t(i18n::key::SHELF_EMPTY_TITLE),
            i18n::t(i18n::key::SHELF_EMPTY_HINT),
            i18n::t(i18n::key::SHELF_EMPTY_IMPORT_LOCAL),
            Some(i18n::t(i18n::key::SHELF_EMPTY_IMPORT_SOURCE)),
        );
        if response.primary_clicked {
            state.open_overlay(Overlay::ImportLocal);
        }
        if response.secondary_clicked {
            state.open_overlay(Overlay::ImportSource);
        }
        return requests;
    }

    // --- 顶部：过滤框 + 排序 + 计数 ---
    ui.horizontal(|ui| {
        let mut filter = state.shelf().filter().to_owned();
        let response = ui.add(
            egui::TextEdit::singleline(&mut filter)
                .hint_text(i18n::t(i18n::key::SHELF_FILTER_HINT))
                .desired_width(240.0),
        );
        if response.changed() {
            state.shelf_mut().set_filter(filter);
        }

        let sort_label = match state.shelf_sort() {
            crate::state::shelf::ShelfSort::Recent => i18n::t(i18n::key::SHELF_SORT_RECENT),
            crate::state::shelf::ShelfSort::TitleAsc => i18n::t(i18n::key::SHELF_SORT_TITLE_ASC),
            crate::state::shelf::ShelfSort::TitleDesc => i18n::t(i18n::key::SHELF_SORT_TITLE_DESC),
        };
        if ui
            .button(format!(
                "{}: {}",
                i18n::t(i18n::key::SHELF_SORT_LABEL),
                sort_label
            ))
            .clicked()
        {
            state.toggle_shelf_sort();
        }

        ui.label(
            egui::RichText::new(i18n::book_count(state.shelf_len()))
                .color(theme.text_color_for(SurfaceEmphasis::Required)),
        );
    });

    ui.separator();

    // --- 过滤无结果（**与空态不同**）---
    let visible_ids: Vec<String> = state
        .shelf()
        .visible()
        .iter()
        .map(|entry| entry.book_id.clone())
        .collect();
    if visible_ids.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(theme.theme().spacing().sp_8 as f32);
            ui.label(
                egui::RichText::new(i18n::t(i18n::key::SHELF_NO_MATCH))
                    .color(theme.text_color_for(SurfaceEmphasis::Required)),
            );
        });
        return requests;
    }

    // --- 虚拟化网格 ---
    // 视口高度由 `ScrollArea` 在闭包里给出（它才是"可视区"的权威）——
    // 这里只用可用宽推导列数
    let viewport_width = ui.available_width();
    let card = book_card::card_size();
    let layout = GridLayout::new(
        viewport_width,
        card.x,
        card.y,
        theme.theme().spacing().sp_2 as f32,
    );

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            // 先占位撑开滚动条，再把可视区的卡片画到**绝对坐标**上
            ui.allocate_space(egui::vec2(
                viewport_width,
                layout.content_height_px(visible_ids.len()),
            ));

            let scroll_offset = viewport.min.y;
            let indices =
                layout.visible_index_range(visible_ids.len(), scroll_offset, viewport.height());

            for index in indices {
                let Some(book_id) = visible_ids.get(index) else {
                    continue;
                };
                let Some(entry) = state.shelf().get(book_id).cloned() else {
                    continue;
                };
                // 网格坐标相对内容区原点，而 `viewport.min` 就是内容区原点
                // 在屏幕上的位置（egui 已把滚动偏移折进去了），故直接相加。
                let (x, y) = layout.card_position(index);
                let rect = egui::Rect::from_min_size(
                    egui::pos2(viewport.min.x + x, viewport.min.y + y),
                    card,
                );
                let selected = state.shelf_selected() == Some(book_id.as_str());
                let response = book_card::draw(ui, &theme, &entry, rect, selected);
                if response.clicked() {
                    state.select_shelf_entry(Some(book_id.clone()));
                    requests.push(state.open_book_request(book_id.clone()));
                }
            }
        });

    requests
}
