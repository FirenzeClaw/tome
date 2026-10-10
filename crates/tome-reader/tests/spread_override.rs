//! T020 验收：手动覆盖三态 + 语义回落（FR-015，裁决 OQ#14→C）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T020）
//!
//! - 回落路径与保留路径各有测试（宽度变化**两类**）
//!
//! ## FR-015 的两个半句（缺一不可）
//!
//! > 手动选择在会话内 MUST 优先于自动判定，并 MUST 按**语义回落**失效：
//! > 即当手动选择与当前阅读区可用宽度相矛盾时（如强制双页但宽度已容不下
//! > 两栏），系统 MUST 回落到自动判定；宽度仅发生**不改变该矛盾**的变化时
//! > MUST NOT 清除用户的手动选择。
//!
//! 前一个半句容易实现（加个 if），后一个半句才是这套语义的价值所在——它
//! 要求判定"矛盾是否改变"，而不是"宽度是否改变"。MASTER 原条款的"变化幅度
//! 阈值"正因如此被判为**凭空数字且会误伤微调**（裁决 OQ#14→C）。
//!
//! 故本文件的"两类宽度变化"是：
//!
//! | 类 | 例子（ForceSpread 下） | 期望 |
//! |---|---|---|
//! | 矛盾**未变** | 1500px → 2400px（都容得下两栏） | 保留选择 |
//! | 矛盾**出现** | 2400px → 800px（容不下了） | 回落并清除 |
//!
//! 本文件还覆盖第三类（**矛盾已存在**时继续变窄）——它必须**保持**回落状态
//! 而不产生"反复清除"的额外副作用。

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig, SpreadMode};
use tome_reader::paginate::page::Paginator;
use tome_reader::paginate::spread::{
    auto_columns, column_measure_px, override_decision_after_resize, required_width_px,
    resolve_columns, resolve_override, Columns, OverrideDecision, Viewport,
};

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

/// ForceSpread 的几何（默认字号 19px → 阈值 1432px）。
fn force_spread() -> GeometryConfig {
    let mut config = geometry();
    config.spread_mode = SpreadMode::ForceSpread;
    config
}

fn force_single() -> GeometryConfig {
    let mut config = geometry();
    config.spread_mode = SpreadMode::ForceSingle;
    config
}

// ===========================================================================
// 三态的基本语义
// ===========================================================================

/// **Auto**：完全由判据决定，不存在"手动选择"。
#[test]
fn auto_mode_follows_criterion_only() {
    let g = geometry();
    let threshold = required_width_px(&g);

    for (width, expected) in [
        (threshold - 1.0, Columns::Single),
        (threshold, Columns::Double),
        (threshold + 1000.0, Columns::Double),
    ] {
        let viewport = Viewport::new(width, 800.0);
        assert_eq!(
            resolve_columns(&viewport, &g),
            expected,
            "Auto 模式下宽度 {width}px 应判 {expected:?}"
        );
        assert_eq!(
            auto_columns(&viewport, &g),
            expected,
            "Auto 模式下 resolve_columns 必须等于纯判据的结果"
        );
    }
}

/// **ForceSingle**：任何宽度下都是单页，且选择**恒成立**。
#[test]
fn force_single_always_wins_and_always_active() {
    let g = force_single();
    for width in [0.0f32, 1.0, 640.0, 1432.0, 4000.0] {
        let viewport = Viewport::new(width, 800.0);
        let resolution = resolve_override(&viewport, &g);
        assert_eq!(
            resolution.columns,
            Columns::Single,
            "宽度 {width}px 下 ForceSingle 应压过判据"
        );
        assert!(
            resolution.override_active,
            "宽度 {width}px 下单页无宽度下限，选择不得被判为失效"
        );
    }
}

/// **ForceSpread**：宽窗下压过判据（即使判据说单页）。
#[test]
fn force_spread_wins_when_width_allows() {
    let g = force_spread();
    // 2000px：判据说双页，手动也选双页 → 结果双页
    let wide = resolve_override(&Viewport::new(2000.0, 800.0), &g);
    assert_eq!(wide.columns, Columns::Double);
    assert!(wide.override_active);

    // 关键情形：把版心调大到判据说单页，而手动仍强制双页——**成立**吗？
    // 这里的答案是**不成立**：FR-015 的语义回落判据就是"宽度是否容得下两栏"，
    // 而"容得下"由判据计算，故判据说单页时手动双页必然回落。
    let narrow = Viewport::new(1000.0, 800.0);
    assert_eq!(
        auto_columns(&narrow, &g),
        Columns::Single,
        "前置：判据说单页"
    );
    let resolution = resolve_override(&narrow, &g);
    assert_eq!(
        resolution.columns,
        Columns::Single,
        "判据说容不下两栏时，强制双页必须回落"
    );
    assert!(!resolution.override_active);
}

// ===========================================================================
// 两类宽度变化（本任务的核心）
// ===========================================================================

/// **矛盾未变**：宽度变化但不改变"能否容下两栏" → **保留**选择。
///
/// 这是 FR-015 第二半句的落点。一个只实现"宽度变了就重判"的版本会在这里
/// 失败——它会认为"宽度变了"就该重新裁决，于是用户每调一次窗口就丢掉手动
/// 选择。而正确语义是：只要还容得下两栏，用户的双页选择就一直有效。
#[test]
fn width_change_without_new_contradiction_keeps_override() {
    let g = force_spread();
    let threshold = required_width_px(&g);

    // 一串都容得下两栏的宽度（含阈值本身）
    let widths = [threshold, threshold + 1.0, 1600.0, 2400.0, 5000.0];
    for window in widths.windows(2) {
        let (from, to) = (window[0], window[1]);

        let before = resolve_override(&Viewport::new(from, 800.0), &g);
        let after = resolve_override(&Viewport::new(to, 800.0), &g);

        assert!(before.override_active, "{from}px 下选择应成立");
        assert!(
            after.override_active,
            "{from}px → {to}px 的矛盾未变（都容得下两栏），选择必须保留"
        );
        assert_eq!(
            override_decision_after_resize(&Viewport::new(to, 800.0), &g),
            OverrideDecision::Keep,
            "{from}px → {to}px 后应判 Keep"
        );
    }
}

/// **矛盾出现**：宽度缩到容不下两栏 → **回落**并报告选择已失效。
#[test]
fn new_contradiction_clears_override() {
    let g = force_spread();
    let threshold = required_width_px(&g);

    let wide = resolve_override(&Viewport::new(threshold + 500.0, 800.0), &g);
    assert!(wide.override_active, "前置：宽窗下选择成立");

    // 恰好差 0.1px 就容不下——边界的另一侧
    let narrow_viewport = Viewport::new(threshold - 0.1, 800.0);
    let narrow = resolve_override(&narrow_viewport, &g);
    assert_eq!(narrow.columns, Columns::Single, "应回落到单页");
    assert!(
        !narrow.override_active,
        "矛盾出现时选择必须被判为失效（供调用方清除）"
    );
    assert_eq!(
        override_decision_after_resize(&narrow_viewport, &g),
        OverrideDecision::Clear
    );
}

/// **矛盾已存在**时继续收窄 → 仍是 Clear（幂等，不产生额外状态）。
///
/// ## 为什么这条要单独测
///
/// "反复清除"在实现上可能表现为"每次 resize 都触发一次回落通知"——对
/// 调用方而言是状态抖动（界面可能反复闪烁"已回落"的提示）。本测试钉住
/// 判定是**无状态**的纯函数：同一个宽度反复问，答案恒相同。
#[test]
fn already_contradictory_stays_clear_and_is_idempotent() {
    let g = force_spread();

    for width in [1400.0f32, 1000.0, 500.0, 100.0] {
        let viewport = Viewport::new(width, 800.0);
        assert_eq!(
            override_decision_after_resize(&viewport, &g),
            OverrideDecision::Clear,
            "宽度 {width}px 下已矛盾，应保持 Clear"
        );
        // 幂等：反复问答案相同
        for _ in 0..3 {
            assert_eq!(
                override_decision_after_resize(&viewport, &g),
                OverrideDecision::Clear
            );
        }
    }
}

/// 回落方向可以逆转：窄 → 宽时用户**重新**选择双页仍然有效。
///
/// ## 为什么这条重要
///
/// "回落"清除的是**会话内的手动选择**（调用方的状态），不是某个持久标记。
/// 用户收窄窗口→回落→再拉宽→**重新点选双页** → 必须有效。若实现把"回落"
/// 记成了一个粘性状态（如 `spread_override_disabled = true`），用户在宽窗下
/// 就再也选不了双页。
#[test]
fn user_can_choose_spread_again_after_fallback() {
    let g = force_spread();
    let threshold = required_width_px(&g);

    // 窄窗：回落
    let narrow = Viewport::new(threshold - 100.0, 800.0);
    assert!(!resolve_override(&narrow, &g).override_active);

    // 拉宽：手动选择**重新成立**（无粘性状态）
    let wide = Viewport::new(threshold + 100.0, 800.0);
    let resolution = resolve_override(&wide, &g);
    assert_eq!(resolution.columns, Columns::Double);
    assert!(
        resolution.override_active,
        "拉宽后用户的双页选择必须重新成立（回落不是粘性状态）"
    );
}

// ===========================================================================
// 字号变化也触发重判（FR-014 的"字号变化时 MUST 重新计算"）
// ===========================================================================

/// 宽度不变、字号变大 → 阈值右移 → 手动双页由"成立"变"矛盾"。
///
/// ## 为什么这条与"宽度变化"分开测
///
/// FR-014 明文："字号变化时 MUST 重新计算该判据。" 一个只监听窗口 resize
/// 的实现（把判据缓存成"上次算的结果"）会在改字号时漏掉重判——用户把字号
/// 调到 30px 后，双页模式会把每栏压到远小于版心目标，而界面仍是双页。
#[test]
fn font_size_change_recomputes_override_validity() {
    let geometry_at_19 = force_spread();
    let mut geometry_at_24 = force_spread();
    geometry_at_24.font_size_px = 24;

    // 1500px：19px 下容得下（阈值 1432），24px 下容不下（阈值 1792）
    let viewport = Viewport::new(1500.0, 800.0);

    let at_19 = resolve_override(&viewport, &geometry_at_19);
    assert_eq!(at_19.columns, Columns::Double);
    assert!(at_19.override_active, "19px 下双页选择成立");

    let at_24 = resolve_override(&viewport, &geometry_at_24);
    assert_eq!(
        at_24.columns,
        Columns::Single,
        "24px 下同一宽度容不下两栏（阈值 {:.0}px），必须回落",
        required_width_px(&geometry_at_24)
    );
    assert!(!at_24.override_active, "字号变化必须触发重判");
}

/// 版心目标变化同样触发重判（不是只看字号）。
#[test]
fn measure_change_recomputes_override_validity() {
    let base = force_spread();
    let mut generous = force_spread();
    generous.measure_em = 42.0; // 阈值 = 2×798+64 = 1660

    let viewport = Viewport::new(1500.0, 800.0);
    assert!(
        resolve_override(&viewport, &base).override_active,
        "36em 下成立"
    );
    assert!(
        !resolve_override(&viewport, &generous).override_active,
        "42em 下同一宽度容不下两栏（阈值 1660px），必须回落"
    );
}

// ===========================================================================
// 回落后的版心与页序列（回落不是"什么都不做"）
// ===========================================================================

/// 回落发生时，**版心按单页计算**（不是仍按双页的半栏算）。
#[test]
fn fallen_back_measure_uses_single_column() {
    let g = force_spread();
    let viewport = Viewport::new(800.0, 800.0);

    let columns = resolve_columns(&viewport, &g);
    assert_eq!(columns, Columns::Single);

    let measure = column_measure_px(columns, &viewport, &g);
    // 单页版心 = min(36em=684, 可用宽 800, 42em=798) = 684
    assert!(
        (measure - 684.0).abs() < 1e-3,
        "回落后的版心应按单页算（684px），实测 {measure}px"
    );

    // 对照：若仍按双页算会是 (800−64)/2 = 368
    let wrong = column_measure_px(Columns::Double, &viewport, &g);
    assert!(
        (wrong - 368.0).abs() < 1e-3,
        "前置对照：双页版心应为 368px，实测 {wrong}px"
    );
}

/// 手动覆盖的**结果**贯穿到分页：同一正文在 ForceSingle / ForceSpread 下
/// 产出不同的栏数（并与 `spread` 字段一致）。
#[test]
fn pagination_reports_resolved_columns_consistently() {
    let content = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」。\n\
                   书页翻开的时候，光线正好落在那一行字上。";

    let mut paginator = Paginator::new();

    // ForceSingle：宽窗下也是单页
    let single = paginator
        .paginate(content, &force_single(), &Viewport::new(2000.0, 400.0))
        .expect("应成功");
    assert_eq!(single.spread.columns, Columns::Single);
    for page in &single.pages {
        assert_eq!(page.side, None, "单页模式下不得有左右归属");
    }

    // ForceSpread：宽窗下是双页，页有左右归属
    let spread = paginator
        .paginate(content, &force_spread(), &Viewport::new(2000.0, 400.0))
        .expect("应成功");
    assert_eq!(spread.spread.columns, Columns::Double);
    assert!(spread.pages.iter().any(|page| page.side.is_some()));
}

/// 回落状态下分页的 `spread` 如实报告**单页**（不撒谎成双页）。
#[test]
fn pagination_reports_fallback_honestly() {
    let content = "内容够长，会在窄版心下排成多行，以便观察分页结果。";

    let mut paginator = Paginator::new();
    // ForceSpread 但宽度只有 800px（容不下两栏）→ 回落
    let result = paginator
        .paginate(content, &force_spread(), &Viewport::new(800.0, 300.0))
        .expect("应成功");

    assert_eq!(
        result.spread.columns,
        Columns::Single,
        "回落时 spread 字段必须如实报告单页"
    );
    assert!(
        !result.spread.override_active,
        "回落时 override_active 必须为 false（供上层清除会话内的手动选择）"
    );
    for page in &result.pages {
        assert_eq!(page.side, None, "回落成单页后不得保留左右归属");
    }
}

/// 强制双页**成立**时，分页的 `override_active` 为 true。
#[test]
fn pagination_reports_active_override() {
    let content = "内容。";
    let mut paginator = Paginator::new();
    let result = paginator
        .paginate(content, &force_spread(), &Viewport::new(2000.0, 400.0))
        .expect("应成功");
    assert!(result.spread.override_active, "成立时须如实报告");
}
