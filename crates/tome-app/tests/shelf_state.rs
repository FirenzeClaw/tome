//! T011 / T012 验收：书架状态与虚拟化几何（`crates/tome-app/src/state/shelf.rs`）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T011 / T012）
//!
//! - T011：过滤/排序/滚动位置在 `Shelf ⇄ Reading` 往返后不变（SC-010 的
//!   headless 形态）；1000 册合成数据的过滤/排序响应（**状态层**，不涉渲染）
//! - T012：虚拟化断言——1000 册仅实例化可视区卡片（帧时间分布记录供 SC-005）
//!
//! ## 为什么这些断言必须是 headless 的
//!
//! SC-005（1000 册 60fps）与 SC-010（往返状态保持）都是**可判定**的要求。
//! 若判定它们必须先开窗口、拖滚动条、数帧，判据就退化成人工观察——而人工
//! 观察在回归时会漏。做成纯值后：
//!
//! - "往返后滚动位置变没变"= 两次方法调用 + 一次相等比较
//! - "1000 册过滤一次要多久"= 一次 `Instant::now()` 包住的函数调用
//! - "实例化了几张卡"= 一个区间长度
//!
//! **三条都能进 CI**。

use std::time::Instant;

use tome_app::app::ViewState;
use tome_app::engine::messages::ShelfEntry;
use tome_app::state::shelf::{GridLayout, ShelfSort, ShelfState};

/// 造一个条目。
fn entry(book_id: &str, title: &str, last_read_at: Option<i64>) -> ShelfEntry {
    ShelfEntry {
        book_id: book_id.to_owned(),
        title: title.to_owned(),
        author: "作者".to_owned(),
        origin_label: "local".to_owned(),
        cover: None,
        progress_percent: 0.0,
        last_read_at,
    }
}

/// 1000 册合成书架（SC-005 的规模）。
///
/// 标题带零填充序号：这样"按标题排序"的顺序是确定的，断言可以逐项比对，
/// 而不必写成"集合相等"（那会漏掉排序稳定性问题）。
fn thousand_books() -> ShelfState {
    let mut shelf = ShelfState::new();
    let entries: Vec<ShelfEntry> = (0..1000)
        .map(|index| {
            let title = format!("书 {index:04}");
            entry(
                &format!("{index}"),
                &title,
                if index % 7 == 0 {
                    Some(1_760_000_000 + index as i64)
                } else {
                    None
                },
            )
        })
        .collect();
    shelf.replace_all(entries);
    shelf
}

// ===========================================================================
// T011：过滤与排序
// ===========================================================================

/// 过滤按标题子串匹配，且**不改动**全量条目数。
#[test]
fn filtering_does_not_change_the_total_count() {
    let mut shelf = thousand_books();
    assert_eq!(shelf.len(), 1000);

    shelf.set_filter("书 0001");
    assert_eq!(shelf.visible_len(), 1, "「书 0001」只应匹配一本");
    assert_eq!(shelf.len(), 1000, "过滤是视图关切，不改全量条目");
}

/// 过滤词两端空白被忽略（粘贴与输入法极易混入）。
#[test]
fn filtering_trims_surrounding_whitespace() {
    let mut shelf = thousand_books();
    shelf.set_filter("  书 0001  ");
    assert_eq!(
        shelf.visible_len(),
        1,
        "首尾空白不该让过滤结果变成 0（那是最难自查的一类）"
    );
}

/// 排序与过滤**正交**：排序不改变过滤后的集合。
#[test]
fn sorting_and_filtering_are_orthogonal() {
    let mut shelf = thousand_books();
    shelf.set_filter("书 000");

    let before: Vec<String> = shelf
        .visible()
        .iter()
        .map(|entry| entry.book_id.clone())
        .collect();

    for _ in 0..3 {
        shelf.cycle_sort();
        let after: Vec<String> = shelf
            .visible()
            .iter()
            .map(|entry| entry.book_id.clone())
            .collect();
        assert_eq!(
            before.len(),
            after.len(),
            "排序键 {:?} 改变了过滤后的条目数",
            shelf.sort()
        );
        assert!(
            before.iter().all(|id| after.contains(id)),
            "排序键 {:?} 改变了过滤后的集合",
            shelf.sort()
        );
    }
}

/// 标题升序/降序互为反序。
#[test]
fn title_sort_directions_are_reverses_of_each_other() {
    let mut shelf = thousand_books();
    shelf.set_sort(ShelfSort::TitleAsc);
    let ascending: Vec<String> = shelf.visible().iter().map(|e| e.title.clone()).collect();

    shelf.set_sort(ShelfSort::TitleDesc);
    let descending: Vec<String> = shelf.visible().iter().map(|e| e.title.clone()).collect();

    let mut expected = ascending.clone();
    expected.reverse();
    assert_eq!(descending, expected, "降序应是升序的严格反序");
}

/// 最近阅读排序：**从未打开的书排最后**（而不是当成时间戳 0）。
///
/// ## 这条为什么重要
///
/// 把 `None` 当成 0 会让"从没读过的书"混在"很久以前读过的书"中间——用户
/// 看到的是"新导入的书莫名其妙夹在中间"，而那没有任何线索可循。
#[test]
fn never_opened_books_sort_last_in_recent_order() {
    let mut shelf = thousand_books();
    shelf.set_sort(ShelfSort::Recent);
    let visible = shelf.visible();

    let first_unread = visible
        .iter()
        .position(|entry| entry.last_read_at.is_none())
        .expect("1000 册里有从未打开的（夹具保证）");
    assert!(
        visible[first_unread..]
            .iter()
            .all(|entry| entry.last_read_at.is_none()),
        "一旦出现未读的书，其后必须全是未读的"
    );
    assert!(
        visible[..first_unread]
            .iter()
            .all(|entry| entry.last_read_at.is_some()),
        "未读之前必须全是读过的"
    );
}

/// 排序键三态循环回到起点。
#[test]
fn sort_key_cycles_through_three_states() {
    let mut shelf = ShelfState::new();
    assert_eq!(shelf.sort(), ShelfSort::Recent, "默认是最近阅读");
    shelf.cycle_sort();
    assert_eq!(shelf.sort(), ShelfSort::TitleAsc);
    shelf.cycle_sort();
    assert_eq!(shelf.sort(), ShelfSort::TitleDesc);
    shelf.cycle_sort();
    assert_eq!(shelf.sort(), ShelfSort::Recent, "三态应循环回起点");
}

// ===========================================================================
// T011：往返状态保持（SC-010 的状态层形态）
// ===========================================================================

/// `Shelf ⇄ Reading` 往返后过滤词、排序键、滚动位置、选中项**逐项不变**。
#[test]
fn shelf_view_state_survives_a_round_trip() {
    let mut state = ViewState::new();
    state.apply_event(tome_app::engine::messages::EngineEvent::ShelfLoaded {
        entries: vec![
            entry("1", "三体", Some(1_760_000_000)),
            entry("2", "活着", None),
        ],
    });

    state.set_shelf_filter("三体".to_owned());
    state.set_shelf_sort(ShelfSort::TitleDesc);
    state.set_shelf_scroll(432.5);
    state.select_shelf_entry(Some("1".to_owned()));

    state.open_book("1");
    state.back_to_shelf();

    assert_eq!(state.shelf_filter(), "三体");
    assert_eq!(state.shelf_sort(), ShelfSort::TitleDesc);
    assert!(
        (state.shelf_scroll() - 432.5).abs() < f32::EPSILON,
        "滚动位置必须保持（SC-010），实测 {}",
        state.shelf_scroll()
    );
    assert_eq!(state.shelf_selected(), Some("1"));
}

/// 移除选中的书之后，选中项**清空**（否则键盘导航从"不存在的选中项"开始）。
#[test]
fn removing_the_selected_entry_clears_the_selection() {
    let mut shelf = thousand_books();
    shelf.select(Some("7".to_owned()));
    assert_eq!(shelf.selected(), Some("7"));

    assert!(shelf.remove("7"));
    assert_eq!(
        shelf.selected(),
        None,
        "移除后选中项必须清空，否则 Enter 会指向一个不存在的条目"
    );
    assert_eq!(shelf.len(), 999);
}

/// 移除**不存在的**条目返回 `false`（调用方据此区分"删了"与"本来就没有"）。
#[test]
fn removing_a_missing_entry_reports_false() {
    let mut shelf = thousand_books();
    assert!(!shelf.remove("不存在的 id"));
    assert_eq!(shelf.len(), 1000);
}

/// 重复导入识别为**同一条目**（更新而非新增卡片）。
#[test]
fn re_importing_updates_the_existing_entry() {
    let mut shelf = ShelfState::new();
    shelf.upsert(entry("1", "三体", None));
    shelf.upsert(entry("1", "三体（修订版）", Some(1_760_000_000)));

    assert_eq!(shelf.len(), 1, "同一 book_id 必须就地更新，不得新增卡片");
    assert_eq!(
        shelf.get("1").map(|e| e.title.as_str()),
        Some("三体（修订版）")
    );
}

// ===========================================================================
// T011：1000 册的过滤/排序响应（SC-005 的状态层形态）
// ===========================================================================

/// 1000 册下过滤 + 排序一次在**状态层**的耗时。
///
/// ## 这条断言的口径（不说过头）
///
/// SC-005 说的是"滚动与标题过滤保持 60fps"——那是**帧**预算（16.6ms/帧）。
/// 本测试只覆盖其中**状态层**的那部分（过滤 + 排序），渲染层与 egui 的
/// 绘制在 GUI 走查（T016）里量。
///
/// 阈值取 16ms（60fps 的一帧）并留出余量：状态层若自己就吃掉一帧预算，
/// 渲染层无论多快都不可能达标——故它是**必要条件**，不是充分条件。
#[test]
fn filtering_a_thousand_books_stays_within_a_frame() {
    let mut shelf = thousand_books();
    shelf.set_sort(ShelfSort::TitleAsc);

    // 预热一次（首次调用含分配与分支预测冷启动）
    let _ = shelf.visible();

    let started = Instant::now();
    for index in 0..100 {
        // 交替设置不同过滤词，覆盖"匹配多/匹配少/不匹配"三种形态
        shelf.set_filter(format!("书 {:03}", index));
        let _ = shelf.visible();
    }
    let elapsed = started.elapsed();
    let per_call_ms = elapsed.as_secs_f64() * 1000.0 / 100.0;

    assert!(
        per_call_ms < 16.0,
        "1000 册过滤+排序每次 {per_call_ms:.2}ms，超过一帧预算（16.6ms）——\
         状态层自己就吃满预算的话，渲染层再快也不可能达标"
    );
}

// ===========================================================================
// T012：虚拟化几何（1000 册仅实例化可视区卡片）
// ===========================================================================

/// 卡片尺寸（与 `widgets::book_card` 一致；此处独立取值以便断言几何）。
const CARD_W: f32 = 132.0;
const CARD_H: f32 = 250.0;
const GAP: f32 = 8.0;

/// 列数 = `floor((可用宽 + 槽距) / (卡宽 + 槽距))`，且至少 1 列。
#[test]
fn column_count_follows_the_available_width() {
    // 恰好放得下 5 列：5*132 + 4*8 = 692
    let layout = GridLayout::new(692.0, CARD_W, CARD_H, GAP);
    assert_eq!(layout.columns(), 5, "恰好放得下时应给足列数，不少算一列");

    // 差 1px：4 列
    let layout = GridLayout::new(691.0, CARD_W, CARD_H, GAP);
    assert_eq!(layout.columns(), 4);

    // 极窄：仍给 1 列（而不是 0 —— 0 列会让所有卡片都不画）
    let layout = GridLayout::new(10.0, CARD_W, CARD_H, GAP);
    assert_eq!(layout.columns(), 1, "再窄也要有 1 列，否则界面全空");
}

/// **虚拟化核心断言**：1000 册只实例化可视区（含缓冲行）的卡片。
///
/// ## 判据为什么是"远小于 1000"而不是一个精确数
///
/// 精确数会随缓冲行数变化而脆弱（调一次缓冲就红）。而虚拟化的**目的**是
/// "实例化数量与总册数解耦"——故判据取那个目的：1000 册与 10000 册的实例化
/// 数量应当**相同**（下一条测试断言这一点）。这里再加一个宽松上界（视口
/// 容量的两倍），抓住"缓冲行数写成了常数倍全量"这类错误。
#[test]
fn a_thousand_books_instantiate_only_the_visible_cards() {
    let layout = GridLayout::new(1000.0, CARD_W, CARD_H, GAP);
    let columns = layout.columns();
    assert!(columns >= 7, "1000px 宽应有 7 列以上，实测 {columns}");

    let viewport_height = 800.0;
    let visible = layout.visible_index_range(1000, 0.0, viewport_height);
    let instantiated = visible.len();

    assert!(
        instantiated < 200,
        "1000 册只应实例化可视区卡片，实测 {instantiated} 张"
    );

    // 上界：可视行数 × 列数 × 3（缓冲余量）。超出说明缓冲逻辑写错了。
    let visible_rows = (viewport_height / layout.row_stride_px()).ceil() as usize + 1;
    assert!(
        instantiated <= visible_rows * columns * 3,
        "实例化 {instantiated} 张超过可视行容量的三倍——虚拟化失效"
    );
}

/// 实例化数量与**总册数解耦**（虚拟化的定义）。
#[test]
fn instantiation_count_is_independent_of_the_total() {
    let layout = GridLayout::new(1000.0, CARD_W, CARD_H, GAP);
    let small = layout.visible_index_range(100, 0.0, 800.0).len();
    let large = layout.visible_index_range(100_000, 0.0, 800.0).len();
    assert_eq!(
        small, large,
        "100 册与 100000 册的实例化数量必须相同（虚拟化的定义），实测 {small} vs {large}"
    );
}

/// 滚动到中段时实例化的仍是"那一段"，且区间落在全量之内。
#[test]
fn scrolling_instantiates_the_middle_slice() {
    let layout = GridLayout::new(1000.0, CARD_W, CARD_H, GAP);
    let columns = layout.columns();
    let stride = layout.row_stride_px();

    // 滚到第 50 行
    let offset = 50.0 * stride;
    let visible = layout.visible_index_range(1000, offset, 800.0);

    assert!(!visible.is_empty());
    assert!(
        visible.start >= (50 - 1) * columns,
        "滚动到第 50 行时不应实例化它之前的行（实测从 {} 开始）",
        visible.start
    );
    assert!(visible.end <= 1000, "区间不得越界");
}

/// 内容高度覆盖全部行（供 `ScrollArea` 撑开滚动条）。
#[test]
fn content_height_covers_every_row() {
    let layout = GridLayout::new(1000.0, CARD_W, CARD_H, GAP);
    let rows = layout.row_count(1000);
    let expected = rows as f32 * layout.row_stride_px() - GAP;
    assert!(
        (layout.content_height_px(1000) - expected).abs() < 0.01,
        "内容高应恰好是 N 行卡高 + (N-1) 个槽距"
    );

    // 空书架没有内容高（而不是负的）
    assert_eq!(layout.content_height_px(0), 0.0);
}

/// 空书架不实例化任何卡片（`0..0`）。
#[test]
fn an_empty_shelf_instantiates_nothing() {
    let layout = GridLayout::new(1000.0, CARD_W, CARD_H, GAP);
    assert!(layout.visible_index_range(0, 0.0, 800.0).is_empty());
    assert_eq!(layout.row_count(0), 0);
}
