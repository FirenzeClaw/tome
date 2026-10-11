//! T009 验收：应用骨架（`crates/tome-app/src/app.rs`）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T009）
//!
//! - `AppView` 状态机的六条不变量（`data-model.md` §2）单元断言全绿
//! - 视图状态机为**纯 Rust 值**（headless 可测，不依赖 egui 上下文）
//!
//! ## 为什么状态机与绘制分开
//!
//! "书架 ⇄ 阅读往返后状态不变"（SC-010）若只能在真窗口里验，判据就退化成
//! 人工点击。把状态机做成纯值类型后，往返就是两次方法调用，断言是普通的
//! 相等比较——**每一条不变量都能被 CI 覆盖**，而不是留在走查清单上。
//!
//! 绘制（`TomeApp::ui`）只消费状态机的输出，不含决策逻辑。

use std::sync::Arc;

use tome_app::app::{AppView, DrawerTab, ErrorAction, ErrorToast, Overlay, ViewState};
use tome_app::engine::messages::{
    ClassifiedErrorView, ContentState, EngineEvent, ReadingAnchor, ReadingParams, SearchHit,
    SearchSession, SourceRef, ThemeChoice,
};
use tome_reader::paginate::config::ReadingMode;

/// 造一个初始状态（空书架）。
fn fresh() -> ViewState {
    ViewState::new()
}

fn anchor(chapter: i32, offset: i64) -> ReadingAnchor {
    ReadingAnchor {
        chapter_index: chapter,
        char_offset: offset,
    }
}

// ===========================================================================
// 不变量 1：Shelf ⇄ Reading 互斥，往返状态保持（SC-010）
// ===========================================================================

/// 书架 ⇄ 阅读**互斥**：任一时刻只处于其一。
#[test]
fn shelf_and_reading_are_mutually_exclusive() {
    let mut state = fresh();
    assert_eq!(state.view(), AppView::Shelf);

    state.open_book("local:三体");
    assert_eq!(state.view(), AppView::Reading);

    state.back_to_shelf();
    assert_eq!(state.view(), AppView::Shelf);
}

/// 往返后书架的**过滤词与滚动位置**不变（SC-010 前半）。
#[test]
fn shelf_keeps_filter_and_scroll_across_a_round_trip() {
    let mut state = fresh();
    state.set_shelf_filter("三体".to_owned());
    state.set_shelf_scroll(432.5);

    state.open_book("local:三体");
    state.back_to_shelf();

    assert_eq!(
        state.shelf_filter(),
        "三体",
        "往返后过滤词必须保持（SC-010）"
    );
    assert!(
        (state.shelf_scroll() - 432.5).abs() < f32::EPSILON,
        "往返后滚动位置必须保持（SC-010），实测 {}",
        state.shelf_scroll()
    );
}

/// 往返后阅读的**锚点与参数**不变（SC-010 后半）。
#[test]
fn reading_keeps_anchor_and_params_across_a_round_trip() {
    let mut state = fresh();
    state.open_book("local:三体");

    let mut params = ReadingParams::default_ma();
    params.font_size_px = 24.0;
    params.mode = ReadingMode::Scroll;
    params.theme = ThemeChoice::Night;
    state.apply_params(params);
    state.seek_anchor(anchor(7, 1234));

    state.back_to_shelf();
    state.open_book("local:三体");

    assert_eq!(
        state.anchor(),
        anchor(7, 1234),
        "往返后阅读锚点必须保持（SC-010）"
    );
    assert_eq!(state.params(), params, "往返后阅读参数必须保持（SC-010）");
}

/// 打开**另一本**书时，上一本的锚点不串（每本书各有其位置）。
#[test]
fn opening_another_book_does_not_leak_the_previous_anchor() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.seek_anchor(anchor(7, 1234));

    state.back_to_shelf();
    state.open_book("local:活着");

    assert_eq!(
        state.anchor(),
        anchor(0, 0),
        "打开另一本书时锚点应重置为进度（此处尚无进度事件 → 章首）"
    );
}

// ===========================================================================
// 不变量 2：content_version 单调递增
// ===========================================================================

/// `ContentState::Ready` 的 `content_version` **单调递增**。
#[test]
fn content_version_is_monotonic() {
    let mut state = fresh();
    state.open_book("local:三体");

    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 1,
    });
    assert_eq!(state.content_version(), Some(1));

    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 2,
    });
    assert_eq!(state.content_version(), Some(2));

    // 迟到的**旧**版本不得回退（否则纹理缓存会拿旧内容当新内容）
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 1,
    });
    assert_eq!(
        state.content_version(),
        Some(2),
        "content_version 必须单调递增——回退会让纹理缓存把旧内容当新内容"
    );
}

/// 内容未变时 `content_version` **不变**（静态帧零重绘的前提，contracts/ui.md §4.2）。
#[test]
fn unchanged_content_does_not_bump_the_version() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 5,
    });

    // 同一版本再来一次（如窗口重绘触发的事件重放）——版本不得变
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 5,
    });
    assert_eq!(state.content_version(), Some(5));
}

/// 换章时版本继续递增（不是按章重置）。
#[test]
fn content_version_keeps_growing_across_chapters() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 1,
    });
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 1,
        laid_out: None,
        content_version: 2,
    });
    assert_eq!(state.content_version(), Some(2));
    assert_eq!(state.current_chapter(), 1);
}

// ===========================================================================
// 不变量 3：调整只改 params，anchor 不变（SC-006 的状态机表达）
// ===========================================================================

/// 五组参数逐一改变，`anchor` 逐字符不变（SC-006）。
#[test]
fn every_param_change_preserves_the_anchor() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.seek_anchor(anchor(4, 999));

    let base = ReadingParams::default_ma();

    // ① 字号
    let mut changed = base;
    changed.font_size_px = 28.0;
    state.apply_params(changed);
    assert_eq!(state.anchor(), anchor(4, 999), "改字号不得移动锚点");

    // ② 行高
    changed.line_height = 2.1;
    state.apply_params(changed);
    assert_eq!(state.anchor(), anchor(4, 999), "改行高不得移动锚点");

    // ③ 形态
    changed.mode = ReadingMode::Scroll;
    state.apply_params(changed);
    assert_eq!(state.anchor(), anchor(4, 999), "改形态不得移动锚点");

    // ④ 单双页
    changed.page_layout = tome_app::engine::messages::PageLayout::Spread;
    state.apply_params(changed);
    assert_eq!(state.anchor(), anchor(4, 999), "改单双页不得移动锚点");

    // ⑤ 主题
    changed.theme = ThemeChoice::Night;
    state.apply_params(changed);
    assert_eq!(state.anchor(), anchor(4, 999), "改主题不得移动锚点");

    assert_eq!(state.params(), changed, "五组改动都应落到 params");
}

/// 调整后**重排完成**（引擎回事件）时 `content_version +1`。
#[test]
fn relayout_completion_bumps_the_version_by_one() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 1,
    });

    let mut params = ReadingParams::default_ma();
    params.font_size_px = 26.0;
    state.apply_params(params);

    // 引擎完成重排 → 事件回来
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 2,
    });
    assert_eq!(
        state.content_version(),
        Some(2),
        "重排完成应使版本 +1（缓存失效的触发点）"
    );
}

// ===========================================================================
// 不变量 4：generation 全局单调，只呈现当前代际
// ===========================================================================

/// `SearchSession.generation` **全局单调**（取消后新搜索的代际更大）。
#[test]
fn search_generation_is_globally_monotonic() {
    let mut state = fresh();

    let first = state.begin_search("三体".to_owned());
    assert_eq!(first, 1);

    state.cancel_search();
    let second = state.begin_search("活着".to_owned());
    assert_eq!(
        second, 2,
        "取消后新搜索的代际必须更大（否则过期结果会与新的混淆）"
    );

    state.cancel_search();
    let third = state.begin_search("围城".to_owned());
    assert_eq!(third, 3);
}

/// 只呈现**当前代际**的结果（DM §2 不变量 4）。
#[test]
fn stale_generation_results_are_discarded() {
    let mut state = fresh();
    let generation = state.begin_search("三体".to_owned());

    // 当前代际的结果：收下
    state.apply_event(EngineEvent::SearchResult {
        generation,
        source: source_ref("synthetic"),
        hits: vec![hit("三体")],
    });
    assert_eq!(state.search_hit_count(), 1);

    // 过期的代际（上一次搜索的迟到结果）：丢弃
    state.apply_event(EngineEvent::SearchResult {
        generation: generation - 1,
        source: source_ref("stale-source"),
        hits: vec![hit("不该出现")],
    });
    assert_eq!(
        state.search_hit_count(),
        1,
        "过期代际的结果不得进入呈现列表（DM §2 不变量 4）"
    );

    // 更大的代际（未来的搜索）：同样丢弃
    state.apply_event(EngineEvent::SearchResult {
        generation: generation + 1,
        source: source_ref("future-source"),
        hits: vec![hit("也不该出现")],
    });
    assert_eq!(state.search_hit_count(), 1);
}

/// 取消搜索后，当前代际的结果**也不再收**（代际已作废）。
#[test]
fn results_after_cancel_are_discarded() {
    let mut state = fresh();
    let generation = state.begin_search("三体".to_owned());
    state.cancel_search();

    state.apply_event(EngineEvent::SearchResult {
        generation,
        source: source_ref("synthetic"),
        hits: vec![hit("迟到的结果")],
    });
    assert_eq!(
        state.search_hit_count(),
        0,
        "取消之后该代际的结果不得进入呈现列表（FR-013 可取消）"
    );
}

/// `SearchDone` 只对当前代际生效（过期代际的完成不该结束新搜索）。
#[test]
fn search_done_only_applies_to_the_current_generation() {
    let mut state = fresh();
    let generation = state.begin_search("三体".to_owned());

    state.apply_event(EngineEvent::SearchDone {
        generation: generation - 1,
    });
    assert!(
        !state.search_is_done(),
        "过期代际的 SearchDone 不得结束当前搜索"
    );

    state.apply_event(EngineEvent::SearchDone { generation });
    assert!(state.search_is_done(), "当前代际的 SearchDone 应结束搜索");
}

// ===========================================================================
// 不变量 5：抽屉开合不改 params 与版心几何（FR-008 覆盖式）
// ===========================================================================

/// 抽屉开合前后 `params` 与版心几何**逐字段不变**。
#[test]
fn drawer_open_close_does_not_change_params_or_geometry() {
    let mut state = fresh();
    state.open_book("local:三体");
    let mut params = ReadingParams::default_ma();
    params.font_size_px = 21.0;
    state.apply_params(params);

    let geometry_before = state.reading_geometry();
    let params_before = state.params();

    state.toggle_drawer(DrawerTab::Toc);
    assert!(state.drawer_is_open());
    assert_eq!(
        state.reading_geometry(),
        geometry_before,
        "抽屉是**覆盖式**的，不得改变版心几何（FR-008）"
    );
    assert_eq!(state.params(), params_before, "抽屉不得改变阅读参数");

    state.toggle_drawer(DrawerTab::Toc);
    assert!(!state.drawer_is_open());
    assert_eq!(state.reading_geometry(), geometry_before);
    assert_eq!(state.params(), params_before);
}

/// 抽屉的两个页签可切换，且切换不关抽屉。
#[test]
fn drawer_tabs_switch_without_closing() {
    let mut state = fresh();
    state.open_book("local:三体");

    state.toggle_drawer(DrawerTab::Toc);
    assert_eq!(state.drawer_tab(), Some(DrawerTab::Toc));

    state.set_drawer_tab(DrawerTab::Bookmarks);
    assert!(state.drawer_is_open(), "切页签不得关抽屉");
    assert_eq!(state.drawer_tab(), Some(DrawerTab::Bookmarks));

    // 关闭后页签记忆清空（下次打开回到默认页签）
    state.toggle_drawer(DrawerTab::Bookmarks);
    assert_eq!(state.drawer_tab(), None);
}

// ===========================================================================
// 不变量 6：ErrorToast.actions 非空（每个错误必有可行动作）
// ===========================================================================

/// 每个错误呈现都带**至少一个**可行动作（DM §2 不变量 6 / FR-010）。
#[test]
fn every_error_carries_at_least_one_action() {
    let toast = ErrorToast::new(ClassifiedErrorView::from_kind(
        &tome_core::ErrorKind::Network,
    ));
    assert!(
        !toast.actions().is_empty(),
        "错误呈现必须带可行动作（DM §2 不变量 6）"
    );

    // 默认动作集含"返回"（任何错误都能退出到安全状态）
    assert!(
        toast.actions().contains(&ErrorAction::Back),
        "默认动作集应含返回，实测 {:?}",
        toast.actions()
    );
}

/// 章节失败时错误呈现含「重试」（R3 契约：错误必有动作）。
#[test]
fn chapter_failure_offers_retry() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.apply_event(EngineEvent::ChapterFailed {
        book_id: "local:三体".to_owned(),
        chapter_index: 3,
        error: ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Network),
    });

    let toast = state.error().expect("章失败应产生错误呈现");
    assert!(
        toast.actions().contains(&ErrorAction::Retry),
        "章节加载失败应提供重试（contracts/ui.md R3），实测 {:?}",
        toast.actions()
    );
    assert!(
        toast.actions().contains(&ErrorAction::Back),
        "也应能返回书架"
    );
}

/// 能力错误呈现 `[capability: xxx]` 标识（FR-022 / 002 §2.3）。
#[test]
fn capability_errors_expose_the_identifier() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.apply_event(EngineEvent::ChapterFailed {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        error: ClassifiedErrorView::from_kind(&tome_core::ErrorKind::unsupported_capability(
            tome_core::ErrorCapability::new("offscreenRendering"),
        )),
    });

    let toast = state.error().expect("应产生错误呈现");
    assert_eq!(
        toast.error().capability.as_deref(),
        Some("offscreenRendering"),
        "能力标识应原样透出（不翻译）"
    );
    assert!(
        toast.actions().contains(&ErrorAction::SwitchSource),
        "能力缺失时换源是有意义的退路，实测 {:?}",
        toast.actions()
    );
}

/// 错误标识**永不翻译**（FR-019 / 002 FR-058）。
#[test]
fn error_toast_keeps_the_raw_identifier() {
    let toast = ErrorToast::new(
        ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Storage).with_detail("pack 索引损坏"),
    );
    assert_eq!(toast.error().id, "error.storage");
    assert_eq!(toast.error().detail.as_deref(), Some("pack 索引损坏"));
}

/// 错误被处理后可清除（不粘在界面上）。
#[test]
fn error_can_be_dismissed() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.apply_event(EngineEvent::ChapterFailed {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        error: ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Content),
    });
    assert!(state.error().is_some());

    state.dismiss_error();
    assert!(state.error().is_none(), "错误应可被清除");
}

// ===========================================================================
// 引擎 panic 的呈现（spec 边缘情况）
// ===========================================================================

/// `EnginePanic` 事件产生结构化错误呈现，且**不退出**当前视图。
#[test]
fn engine_panic_is_presented_without_leaving_the_view() {
    let mut state = fresh();
    state.open_book("local:三体");

    state.apply_event(EngineEvent::EnginePanic {
        context: "ReadingLoadChapter".to_owned(),
    });

    assert_eq!(
        state.view(),
        AppView::Reading,
        "引擎 panic 不得把视图弹回书架（用户应留在原地重试）"
    );
    let toast = state.error().expect("引擎 panic 应产生错误呈现");
    assert!(
        toast.actions().contains(&ErrorAction::Retry),
        "应可重试，实测 {:?}",
        toast.actions()
    );
    assert_eq!(
        toast.error().id,
        "error.content",
        "引擎 panic 归入内容类错误（单章隔离语义）"
    );
    assert!(
        toast
            .error()
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("ReadingLoadChapter")),
        "细节应带上发生位置，实测 {:?}",
        toast.error().detail
    );
}

// ===========================================================================
// 叠加层（正交于视图切换）
// ===========================================================================

/// 设置叠加层是**正交叠加**，不改变当前视图。
#[test]
fn settings_overlay_does_not_change_the_view() {
    let mut state = fresh();
    state.open_book("local:三体");

    state.open_overlay(Overlay::Settings);
    assert_eq!(state.view(), AppView::Reading, "叠加层不改视图");
    assert_eq!(state.overlay(), Some(Overlay::Settings));

    state.close_overlay();
    assert_eq!(state.view(), AppView::Reading);
    assert_eq!(state.overlay(), None);
}

/// 导入对话框同理正交。
#[test]
fn import_dialog_is_orthogonal_to_the_view() {
    let mut state = fresh();
    state.open_overlay(Overlay::ImportLocal);
    assert_eq!(state.view(), AppView::Shelf);
    assert_eq!(state.overlay(), Some(Overlay::ImportLocal));
}

// ===========================================================================
// 内容三态（FR-010：不白屏 / 不伪装成功）
// ===========================================================================

/// 打开书 → `Loading`；章就绪 → `Ready`；章失败 → `Failed`。
#[test]
fn content_state_follows_the_engine_events() {
    let mut state = fresh();
    state.open_book("local:三体");
    assert_eq!(
        state.content(),
        &ContentState::Loading,
        "打开书即进入加载态（FR-010：不白屏）"
    );

    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 1,
    });
    assert!(matches!(state.content(), ContentState::Ready { .. }));

    state.apply_event(EngineEvent::ChapterFailed {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        error: ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Content),
    });
    assert!(
        matches!(state.content(), ContentState::Failed { .. }),
        "章失败应进入错误态，实测 {:?}",
        state.content()
    );
}

/// 章节失败**被隔离在该章**：视图不退出，锚点不丢（spec 边缘情况）。
#[test]
fn chapter_failure_is_isolated_to_that_chapter() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.seek_anchor(anchor(5, 200));
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 5,
        laid_out: None,
        content_version: 1,
    });

    state.apply_event(EngineEvent::ChapterFailed {
        book_id: "local:三体".to_owned(),
        chapter_index: 6,
        error: ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Network),
    });

    assert_eq!(state.view(), AppView::Reading, "单章失败不得退出阅读视图");
    assert_eq!(state.anchor(), anchor(5, 200), "单章失败不得移动当前锚点");
}

/// 其他书的失败事件**不影响**当前书（多书事件串扰的防护）。
#[test]
fn events_for_other_books_are_ignored() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.apply_event(EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 0,
        laid_out: None,
        content_version: 1,
    });

    state.apply_event(EngineEvent::ChapterFailed {
        book_id: "local:活着".to_owned(),
        chapter_index: 9,
        error: ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Storage),
    });

    assert!(state.error().is_none(), "别的书的失败不该在当前书里弹错误");
    assert_eq!(state.content_version(), Some(1));
}

// ===========================================================================
// 版心几何（单双页阈值，contracts/ui.md §4.3）
// ===========================================================================

/// 窗口宽 < 1512px（19px 字号）→ 单页；≥ 1512px → 允许双页。
#[test]
fn two_page_threshold_follows_the_window_width() {
    let mut state = fresh();
    state.open_book("local:三体");

    state.set_window_width(1511.0);
    assert_eq!(
        state.reading_geometry().allowed_layout(),
        tome_app::engine::messages::PageLayout::Single,
        "低于阈值应自动单页"
    );

    state.set_window_width(1512.0);
    assert_eq!(
        state.reading_geometry().allowed_layout(),
        tome_app::engine::messages::PageLayout::Spread,
        "达到阈值（含）应允许双页"
    );
}

/// 阈值随**阅读字号**移动（em 制判据，FR-014 / MASTER §5.2）。
#[test]
fn two_page_threshold_scales_with_font_size() {
    let mut state = fresh();
    state.open_book("local:三体");

    // 19px 时阈值 1512px：1512 允许双页
    state.set_window_width(1512.0);
    assert_eq!(
        state.reading_geometry().allowed_layout(),
        tome_app::engine::messages::PageLayout::Spread
    );

    // 调到 24px：阈值右移（24/19 比例），1512 不再够
    let mut params = ReadingParams::default_ma();
    params.font_size_px = 24.0;
    state.apply_params(params);
    assert_eq!(
        state.reading_geometry().allowed_layout(),
        tome_app::engine::messages::PageLayout::Single,
        "字号变大后同样的窗口宽应退回单页（em 制判据，不是固定 px 断点）"
    );
}

/// 用户手动选单页时，即使宽度够也保持单页（FR-007 的显式选择优先）。
///
/// ## `allowed_layout` 与 `effective_layout` 的分工
///
/// 前者是**能力**（宽度容不容得下两栏），后者是**裁决**（意图 + 能力）。
/// 本测试断的是后者——`allowed_layout` 在 2000px 下理应是 `Spread`（宽度确实
/// 够），被否决的是"采用它"这一步。把两者混为一谈会让"用户选单页"变成**永久**
/// 单页：窗口拉到 3000px 也不切双页，而 MASTER §5.2 说的是"本次会话内优先"。
#[test]
fn manual_single_page_overrides_the_automatic_decision() {
    let mut state = fresh();
    state.open_book("local:三体");
    state.set_window_width(2000.0);

    let mut params = ReadingParams::default_ma();
    params.page_layout = tome_app::engine::messages::PageLayout::Single;
    state.apply_params(params);

    assert_eq!(
        state.reading_geometry().allowed_layout(),
        tome_app::engine::messages::PageLayout::Spread,
        "2000px 下宽度确实容得下两栏（能力如此，不该被意图污染）"
    );
    assert_eq!(
        state.effective_layout(),
        tome_app::engine::messages::PageLayout::Single,
        "用户显式选单页时不得采用双页"
    );
}

/// 强制双页但宽度容不下 → **回落单页**（MASTER §5.2 第 3 条的"语义回落"）。
#[test]
fn forced_spread_falls_back_when_the_window_is_too_narrow() {
    let mut state = fresh();
    state.open_book("local:三体");

    let mut params = ReadingParams::default_ma();
    params.page_layout = tome_app::engine::messages::PageLayout::Spread;
    state.apply_params(params);

    state.set_window_width(1000.0);
    assert_eq!(
        state.effective_layout(),
        tome_app::engine::messages::PageLayout::Single,
        "强制双页在容不下两栏时必须回落（否则会挤成不可读的窄栏）"
    );

    // 窗口拉宽到容得下 → 强制双页生效（手动选择**不清除**）
    state.set_window_width(2000.0);
    assert_eq!(
        state.effective_layout(),
        tome_app::engine::messages::PageLayout::Spread,
        "宽度变回容得下时手动选择应重新生效（OQ#14→C：不清除手动选择）"
    );
}

/// 默认（未手动覆盖）时，生效排布 = 宽度判定。
#[test]
fn default_layout_follows_the_width() {
    let mut state = fresh();
    state.open_book("local:三体");

    state.set_window_width(1511.0);
    assert_eq!(
        state.effective_layout(),
        tome_app::engine::messages::PageLayout::Single
    );
    state.set_window_width(1512.0);
    assert_eq!(
        state.effective_layout(),
        tome_app::engine::messages::PageLayout::Spread
    );
}

// ===========================================================================
// 书架事件
// ===========================================================================

/// `ShelfLoaded` 落地为书架条目；`ShelfEntryChanged` 更新单条。
#[test]
fn shelf_events_update_the_entry_list() {
    let mut state = fresh();
    state.apply_event(EngineEvent::ShelfLoaded {
        entries: vec![entry("local:三体", "三体"), entry("local:活着", "活着")],
    });
    assert_eq!(state.shelf_len(), 2);

    state.apply_event(EngineEvent::ShelfEntryChanged {
        entry: entry("local:三体", "三体（修订版）"),
    });
    assert_eq!(state.shelf_len(), 2, "更新不应新增条目");
    assert_eq!(
        state.shelf_entry("local:三体").map(|e| e.title.as_str()),
        Some("三体（修订版）")
    );
}

/// 过滤按标题子串匹配（FR-005）。
#[test]
fn shelf_filter_matches_titles() {
    let mut state = fresh();
    state.apply_event(EngineEvent::ShelfLoaded {
        entries: vec![
            entry("local:三体", "三体"),
            entry("local:活着", "活着"),
            entry("local:三体2", "三体 II"),
        ],
    });

    state.set_shelf_filter("三体".to_owned());
    let visible = state.visible_shelf_entries();
    assert_eq!(visible.len(), 2, "应匹配到两本，实测 {visible:?}");

    state.set_shelf_filter(String::new());
    assert_eq!(state.visible_shelf_entries().len(), 3);
}

/// 排序切换不改变过滤结果集（两者正交）。
#[test]
fn sorting_and_filtering_are_orthogonal() {
    let mut state = fresh();
    state.apply_event(EngineEvent::ShelfLoaded {
        entries: vec![
            entry("local:三体", "三体"),
            entry("local:活着", "活着"),
            entry("local:三体2", "三体 II"),
        ],
    });
    state.set_shelf_filter("三体".to_owned());

    let before: Vec<String> = state
        .visible_shelf_entries()
        .iter()
        .map(|e| e.book_id.clone())
        .collect();
    state.toggle_shelf_sort();
    let after: Vec<String> = state
        .visible_shelf_entries()
        .iter()
        .map(|e| e.book_id.clone())
        .collect();

    assert_eq!(before.len(), after.len(), "排序不得改变过滤后的条目数");
    assert!(
        before.iter().all(|id| after.contains(id)),
        "排序不得改变过滤后的集合，前 {before:?} 后 {after:?}"
    );
}

// ===========================================================================
// 夹具
// ===========================================================================

fn source_ref(source_id: &str) -> SourceRef {
    SourceRef {
        source_id: source_id.to_owned(),
        name: format!("源 {source_id}"),
        imported_at: 1_760_000_000,
    }
}

fn hit(title: &str) -> SearchHit {
    SearchHit {
        book_url: format!("https://example.invalid/book/{title}"),
        title: title.to_owned(),
        author: "作者".to_owned(),
        cover_url: None,
    }
}

fn entry(book_id: &str, title: &str) -> tome_app::engine::messages::ShelfEntry {
    tome_app::engine::messages::ShelfEntry {
        book_id: book_id.to_owned(),
        title: title.to_owned(),
        author: "作者".to_owned(),
        origin_label: "本地书".to_owned(),
        cover: None,
        progress_percent: 0.0,
        last_read_at: None,
    }
}

/// `Arc` 版本的排版结果在这条测试里不用（`laid_out: None` 已足够表达状态机），
/// 但类型必须可构造——留一个断言防它被误删。
#[test]
fn laid_out_type_is_usable() {
    let _: Option<Arc<tome_reader::paginate::page::ChapterLayout>> = None;
}

/// `SearchSession` 是公开值类型（视图层要能构造与断言）。
#[test]
fn search_session_is_a_public_value_type() {
    let session = SearchSession {
        generation: 7,
        keyword: "三体".to_owned(),
        done: false,
    };
    assert_eq!(session.generation, 7);
    assert!(!session.done);
}
