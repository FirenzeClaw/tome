//! T046 验收：阅读工具栏状态机与几何（FR-034）
//!
//! ## FR-034 的三组判据
//!
//! | # | 判据 | 本文件的测试 |
//! |---|---|---|
//! | 1 | 默认隐形 | `starts_hidden_by_default` |
//! | 2 | 静止 2.5s / 移出后淡出（`--dur-base`） | `fades_out_after_idle_and_on_exit` |
//! | 3 | 顶部 8px 热区，**整窗顶部全域**（不随单双页变） | `hot_zone_spans_the_full_window_width` |
//! | 4 | 入口清单十项 | `entry_set_matches_master` |
//! | 5 | 对 S3 暴露状态与边界（**不要求反查内部**） | `exposes_state_and_bounds_for_shell` |
//!
//! ## 为什么"整窗顶部全域"要单独断言
//!
//! MASTER §5.2 的版心在单页与双页下宽度不同（双页是两栏 + 中缝）。若热区
//! 跟着版心走，用户在双页模式下把鼠标移到窗口顶部靠边的位置就唤不出工具栏
//! ——那是"工具栏失灵"的表现，而原因很难被联想到版心。故热区必须锚在
//! **窗口**上沿，与版心无关。
//!
//! ## 状态机的时间语义
//!
//! 本模块不持有真实时钟（那是外壳的职责），而是**吃时间增量**
//! （`advance(milliseconds)`）。这让状态迁移可被确定性地测试——注入真实
//! 时钟的版本只能靠 `sleep` 测，那既慢又 flaky。

use tome_core::model::theme::{Theme, MOTION, READING_TOOLBAR};
use tome_reader::toolbar::{
    ReadingToolbar as ToolbarStateMachine, ToolbarBounds, ToolbarInput, ToolbarVisibility,
    TOOLBAR_ENTRIES,
};

// ===========================================================================
// 1. 默认隐形
// ===========================================================================

/// **默认状态是隐形**（MASTER §5.3：阅读区默认隐形）。
#[test]
fn starts_hidden_by_default() {
    let machine = ToolbarStateMachine::new();
    assert_eq!(
        machine.visibility(),
        ToolbarVisibility::Hidden,
        "工具栏默认必须隐形（MASTER §5.3）"
    );
    assert!(
        !machine.is_visible(),
        "默认不处于可见态——否则阅读区一开始就带着一条栏，与「最安静的地方」冲突"
    );
}

// ===========================================================================
// 2. 唤出与淡出
// ===========================================================================

/// **进入热区即唤出**（FR-034 的"保留顶部 8px 热区"的意义所在）。
#[test]
fn entering_hot_zone_reveals_the_toolbar() {
    let mut machine = ToolbarStateMachine::new();
    machine.apply(ToolbarInput::PointerInHotZone);
    assert_eq!(
        machine.visibility(),
        ToolbarVisibility::Visible,
        "指针进入热区应唤出工具栏"
    );
}

/// **显式唤出也可见**（键盘唤出，FR-035 的`工具栏唤出`键）。
#[test]
fn explicit_reveal_makes_it_visible() {
    let mut machine = ToolbarStateMachine::new();
    machine.apply(ToolbarInput::RevealRequested);
    assert!(machine.is_visible(), "显式唤出应可见");
}

/// **静止 2.5 秒后淡出**（FR-034 明文的时间）。
#[test]
fn fades_out_after_idle() {
    let mut machine = ToolbarStateMachine::new();
    machine.apply(ToolbarInput::RevealRequested);
    assert!(machine.is_visible());

    // 未满 2.5s：仍可见
    machine.advance(READING_TOOLBAR.idle_fade_after_ms - 1);
    assert!(
        machine.is_visible(),
        "静止未满 {}ms 时不该淡出",
        READING_TOOLBAR.idle_fade_after_ms
    );

    // 越过 2.5s：进入淡出
    machine.advance(2);
    assert_eq!(
        machine.visibility(),
        ToolbarVisibility::FadingOut,
        "越过 {}ms 静止应开始淡出",
        READING_TOOLBAR.idle_fade_after_ms
    );
}

/// **淡出时长是 `--dur-base`**（MASTER §5.3 / §4）。
#[test]
fn fade_duration_is_dur_base() {
    let mut machine = ToolbarStateMachine::new();
    machine.apply(ToolbarInput::RevealRequested);
    machine.advance(READING_TOOLBAR.idle_fade_after_ms + 1);
    assert_eq!(machine.visibility(), ToolbarVisibility::FadingOut);

    // 淡出未完成
    machine.advance(MOTION.dur_base_ms - 1);
    assert_eq!(
        machine.visibility(),
        ToolbarVisibility::FadingOut,
        "淡出未走完 {}ms 时仍应在淡出中",
        MOTION.dur_base_ms
    );

    // 淡出走完 → 隐形
    machine.advance(2);
    assert_eq!(
        machine.visibility(),
        ToolbarVisibility::Hidden,
        "淡出走完 {}ms 后应完全隐形",
        MOTION.dur_base_ms
    );
}

/// **鼠标移出阅读区立即淡出**（FR-034："或移出阅读区后淡出"）。
///
/// ## 与"静止淡出"的区别
///
/// 移出是**明确离开**，不必等 2.5 秒——等下去只会让工具栏在用户已经不看
/// 它的时候还挂着。故 `PointerLeftReadingArea` 直接进入淡出。
#[test]
fn leaving_reading_area_starts_fade_immediately() {
    let mut machine = ToolbarStateMachine::new();
    machine.apply(ToolbarInput::RevealRequested);
    machine.apply(ToolbarInput::PointerLeftReadingArea);
    assert_eq!(
        machine.visibility(),
        ToolbarVisibility::FadingOut,
        "移出阅读区应**立即**开始淡出（不等 2.5s 静止）"
    );
}

/// **淡出过程中活动会取消淡出并回到可见**（工具栏不该"闪一下"）。
#[test]
fn activity_during_fade_cancels_it() {
    let mut machine = ToolbarStateMachine::new();
    machine.apply(ToolbarInput::RevealRequested);
    machine.advance(READING_TOOLBAR.idle_fade_after_ms + 1);
    assert_eq!(machine.visibility(), ToolbarVisibility::FadingOut);

    // 用户又动了鼠标（回到热区）
    machine.apply(ToolbarInput::PointerInHotZone);
    assert_eq!(
        machine.visibility(),
        ToolbarVisibility::Visible,
        "淡出中回到热区应恢复可见（而不是继续淡到隐形）"
    );
    // 且静止计时被重置
    machine.advance(READING_TOOLBAR.idle_fade_after_ms - 1);
    assert!(
        machine.is_visible(),
        "唤出应重置静止计时（不该沿用上次的计时继续淡出）"
    );
}

/// **隐藏态下推进时间不会有副作用**（快进一整分钟仍隐形）。
#[test]
fn advancing_time_while_hidden_is_a_no_op() {
    let mut machine = ToolbarStateMachine::new();
    machine.advance(60_000);
    assert_eq!(
        machine.visibility(),
        ToolbarVisibility::Hidden,
        "隐藏态推进时间应保持隐形"
    );
}

// ===========================================================================
// 3. 热区几何：整窗顶部全域
// ===========================================================================

/// **热区是整窗顶部全域**（FR-034 明文；不随单/双页版心变化）。
///
/// ## 判据：热区宽度 == 窗口宽度，且与版心宽度无关
///
/// 本测试给两种版心宽度（单页 vs 双页）但**同一窗口宽度**，断言热区一致。
/// 若实现把热区算成版心宽度，双页那次的断言会失败——那正是"双页下
/// 窗口顶部靠边处唤不出工具栏"这个缺陷的形态。
#[test]
fn hot_zone_spans_the_full_window_width() {
    let single_page_bounds =
        ToolbarBounds::for_window(1500.0, 900.0, 620.0, READING_TOOLBAR.hot_zone_height_px);
    let two_page_bounds =
        ToolbarBounds::for_window(1500.0, 900.0, 1290.0, READING_TOOLBAR.hot_zone_height_px);

    // 热区宽度 == 窗口宽度（整窗全域）
    assert!(
        (single_page_bounds.hot_zone_width_px - 1500.0).abs() < f32::EPSILON,
        "热区宽度应等于窗口宽度（整窗顶部全域），实测 {}",
        single_page_bounds.hot_zone_width_px
    );
    assert_eq!(
        single_page_bounds.hot_zone_width_px, two_page_bounds.hot_zone_width_px,
        "热区宽度不得随单/双页版心变化（FR-034 明文）"
    );

    // 热区高度是 8px，且锚在窗口上沿
    assert!(
        (single_page_bounds.hot_zone_height_px - 8.0).abs() < f32::EPSILON,
        "热区高度应为 8px（MASTER §5.3 / FR-034）"
    );
    assert!(
        single_page_bounds.hot_zone_top_px.abs() < f32::EPSILON,
        "热区应锚在窗口上沿（top = 0）"
    );
}

/// **热区高度可与窗口宽度无关地独立变化**（几何是两项独立参数）。
#[test]
fn hot_zone_geometry_is_independent_of_window_size() {
    let narrow = ToolbarBounds::for_window(800.0, 600.0, 620.0, 8);
    let wide = ToolbarBounds::for_window(2560.0, 1440.0, 620.0, 8);
    assert!(
        (narrow.hot_zone_height_px - wide.hot_zone_height_px).abs() < f32::EPSILON,
        "热区高度不应因窗口大小而变"
    );
    assert!(
        (narrow.hot_zone_width_px - 800.0).abs() < f32::EPSILON
            && (wide.hot_zone_width_px - 2560.0).abs() < f32::EPSILON,
        "热区宽度应各自等于其窗口宽度"
    );
}

/// **命中判定：热区内为真，热区外为假**（几何可被外壳直接使用）。
#[test]
fn hot_zone_hit_testing_is_exact() {
    let bounds = ToolbarBounds::for_window(1000.0, 800.0, 620.0, 8);
    assert!(bounds.contains_in_hot_zone(0.0, 0.0), "左上角应在热区");
    assert!(
        bounds.contains_in_hot_zone(999.0, 7.9),
        "热区右缘上方应在热区"
    );
    assert!(
        !bounds.contains_in_hot_zone(500.0, 8.0),
        "热区下沿（y == 8）应是**区外**（半开区间，避免与首行文字争输入）"
    );
    assert!(
        !bounds.contains_in_hot_zone(1000.0, 0.0),
        "超出右缘应在区外"
    );
}

// ===========================================================================
// 4. 入口清单
// ===========================================================================

/// **入口清单与 MASTER §5.3 逐项一致**（十项）。
#[test]
fn entry_set_matches_master() {
    let expected = [
        "back",
        "toc",
        "progress",
        "font_size",
        "line_height",
        "theme",
        "spread",
        "bookmark",
        "change_source",
        "settings",
    ];
    assert_eq!(
        TOOLBAR_ENTRIES, expected,
        "工具栏入口清单应与 MASTER §5.3 逐项一致"
    );
    // 与 tome-core 的 Theme 常量也一致（两处定义不漂移）
    assert_eq!(
        TOOLBAR_ENTRIES,
        READING_TOOLBAR.entries.as_slice(),
        "本层的入口清单应与 tome-core 的 READING_TOOLBAR.entries 一致"
    );
}

/// **入口是稳定标识而非面向用户的文案**（FR-058）。
#[test]
fn entries_are_stable_identifiers_not_user_text() {
    for entry in TOOLBAR_ENTRIES {
        assert!(
            !entry
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "入口标识不得是中文文案（本地化留给 S3），实测：{entry}"
        );
        assert!(
            entry.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
            "入口标识应是 snake_case 稳定标识，实测：{entry}"
        );
    }
}

// ===========================================================================
// 5. 对 S3 的接口形态
// ===========================================================================

/// **状态与几何经公开接口输出**（FR-034：MUST NOT 要求 S3 反查内部状态）。
///
/// ## 这条在测什么
///
/// FR-034 的职责分界要求本层"通过明确的接口暴露状态与边界"。若 S3 要自己
/// 去猜"现在该不该画"，那分界就破了。本测试消费**全部公开输出**，证明
/// 一次性就能拿到绘制所需的一切。
#[test]
fn exposes_state_and_bounds_for_shell() {
    let mut machine = ToolbarStateMachine::new();
    machine.set_bounds(ToolbarBounds::for_window(1500.0, 900.0, 620.0, 8));
    machine.apply(ToolbarInput::RevealRequested);

    // ① 状态
    assert!(machine.is_visible());
    // ② 不透明度（淡出动画用，0.0–1.0）——S3 直接拿它当 alpha
    let opacity = machine.opacity();
    assert!(
        (0.0..=1.0).contains(&opacity),
        "不透明度应在 0–1 之间，实测 {opacity}"
    );
    assert!((opacity - 1.0).abs() < f32::EPSILON, "刚唤出时应完全不透明");

    // ③ 几何边界（热区 + 工具栏本体）
    let bounds = machine.bounds();
    assert!(bounds.hot_zone_height_px > 0.0);
    assert!(bounds.toolbar_height_px > 0.0, "工具栏本体高度应可查");

    // ④ 入口列表
    let entries = machine.entries();
    assert_eq!(entries.len(), 10);
    assert_eq!(entries, TOOLBAR_ENTRIES);
}

/// **不透明度随淡出进度线性下降**（S3 用它做动画，故必须连续可预期）。
#[test]
fn opacity_tracks_fade_progress() {
    let mut machine = ToolbarStateMachine::new();
    machine.apply(ToolbarInput::RevealRequested);
    assert!((machine.opacity() - 1.0).abs() < f32::EPSILON);

    machine.advance(READING_TOOLBAR.idle_fade_after_ms + 1);
    // 淡出刚开始，接近 1.0 但已下降
    let start_opacity = machine.opacity();
    assert!(start_opacity <= 1.0);

    // 走完半个淡出时长
    machine.advance(MOTION.dur_base_ms / 2);
    let mid_opacity = machine.opacity();
    assert!(
        mid_opacity < start_opacity,
        "淡出中不透明度应下降（{start_opacity} → {mid_opacity}）"
    );
    assert!(
        (0.0..=1.0).contains(&mid_opacity),
        "半程不透明度仍在 0–1 内，实测 {mid_opacity}"
    );

    // 走完淡出 → 隐形且不透明度为 0
    machine.advance(MOTION.dur_base_ms / 2 + 2);
    assert_eq!(machine.visibility(), ToolbarVisibility::Hidden);
    assert!(
        machine.opacity().abs() < f32::EPSILON,
        "完全隐形时不透明度应为 0，实测 {}",
        machine.opacity()
    );
}

/// **状态机不依赖图形类型**（编译期证据 + 可纯值比较）。
#[test]
fn state_machine_is_pure_value() {
    let mut machine = ToolbarStateMachine::new();
    machine.apply(ToolbarInput::RevealRequested);
    // 可移动、可比较状态（说明它是纯数据，S3 可跨帧持有）
    let visibility = machine.visibility();
    assert_eq!(visibility, ToolbarVisibility::Visible);
}

/// **工具栏几何取自主题**（不硬编码，MASTER 改动时一处生效）。
#[test]
fn geometry_comes_from_theme_constants() {
    // 本层的常量应等于 tome-core 的 Theme 常量（两处不漂移）
    assert_eq!(
        READING_TOOLBAR.hot_zone_height_px, 8,
        "热区高度取自 tome-core 的 READING_TOOLBAR"
    );
    assert_eq!(READING_TOOLBAR.idle_fade_after_ms, 2500, "静止淡出取 2.5s");
    assert_eq!(READING_TOOLBAR.fade_duration_ms, MOTION.dur_base_ms);
}

/// **主题实例可被消费以取工具栏几何**（FR-029 的注入路径）。
#[test]
fn accepts_injected_theme() {
    let theme: Theme = tome_core::model::theme::DayTheme.theme();
    let bounds = ToolbarBounds::for_window_with_theme(1500.0, 900.0, 620.0, &theme);
    assert!(
        (bounds.hot_zone_height_px - f32::from(theme.reading_toolbar().hot_zone_height_px)).abs()
            < f32::EPSILON,
        "几何应可取自注入的主题实例"
    );
}
