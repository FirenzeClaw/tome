//! T027 验收：翻页 ⇄ 滚动切换保锚点（FR-023 / SC-005b）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T027）
//!
//! > SC-005b：两方向切换锚点值 100% 不变且字符 100% 可见
//! > （同文件断言）切换过程中无进度写库
//!
//! ## SC-005b 原文
//!
//! > **阅读形态切换的位置不变性**：在翻页 ⇄ 滚动两个方向各切换时，切换前后
//! > 锚点值 100% 不变，且该字符在切换后 100% 可见（翻页为可见字符区间内、
//! > 滚动为视口内）。
//!
//! 两个方向是独立判据：`paged → scroll` 与 `scroll → paged` 各有自己的
//! 可见性定义（前者要求落在某一页区间内，后者要求落在某一屏区间内）。
//!
//! ## 「中间态不写库」为什么必须单独测
//!
//! FR-023 末句：
//!
//! > 书写进度 MUST 以切换稳定后的锚点为准（MUST NOT 以切换过程中的中间态写库）
//!
//! 这是一条**负面**要求——它说"不要做某事"。负面要求最容易在实现里被
//! 违反（因为不做它不需要写代码），也最难在事后发现：日志里只看到"进度被
//! 写了一次"，看不出那次写的是稳定值还是中间态。
//!
//! 故本文件用**写入计数 + 写入值**双重断言：
//!
//! - 过渡中（未 settle）提交 → **零次**写入
//! - settle 后提交 → **恰好一次**，且写入的锚点 == 切换前的锚点
//!
//! 第二条同样重要：若实现只在 settle 时写但写错了值（如写成了别的字符），
//! 计数断言照样通过。两个断言合起来才覆盖 FR-023 的完整语义。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig, ReadingMode};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::{ScrollFlow, TRANSCRIPTION_SETTLE_EVENTS};
use tome_reader::scroll::switch::{AnchorTarget, ModeSwitcher, ProgressSink};

// ===========================================================================
// 夹具
// ===========================================================================

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

fn long_chapter() -> String {
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                     后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。";
    (0..8)
        .map(|index| format!("第{}段。{paragraph}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 记录每次进度写入的**存根**（spy）。
///
/// ## 为什么用 `Vec` 记全部写入而非只记次数
///
/// "写了几次"只能抓"过渡中不该写"这一类违规。抓不到"settle 后写错值"。
/// 记下每次写入的完整内容让两类断言共用一份数据。
#[derive(Debug, Default)]
struct RecordingSink {
    writes: Vec<(i32, i64)>,
}

impl ProgressSink for RecordingSink {
    fn save_progress(&mut self, chapter_index: i32, char_offset: i64) {
        self.writes.push((chapter_index, char_offset));
    }
}

fn build_flow(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("构造滚动流应成功")
}

// ===========================================================================
// SC-005b 主判据：两个方向各自
// ===========================================================================

/// **`paged → scroll`**：锚点值不变，且字符落在视口内。
#[test]
fn paged_to_scroll_preserves_anchor_and_visibility() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    // 每个内容页的首字符都试一次（覆盖"页边界"这一最容易出错的输入）
    for page in paginated.pages.iter().filter(|page| !page.is_padding) {
        for anchor in [page.char_range.start, page.char_range.end.saturating_sub(1)] {
            if anchor >= char_len {
                continue;
            }

            let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, anchor);
            let settled_anchor = switcher.settle(ReadingMode::Scroll, &flow);

            assert_eq!(
                settled_anchor, anchor,
                "paged → scroll 后锚点值必须不变（输入 {anchor}）"
            );
            assert!(
                switcher.target_visible(),
                "锚点 {anchor} 在滚动形态下必须可见"
            );

            // 可见性的实质：落在某一屏的字符区间内
            let owner = flow
                .viewports()
                .iter()
                .find(|vw| vw.char_range.contains(&anchor));
            assert!(
                owner.is_some(),
                "锚点 {anchor} 不属于任何视口（SC-005b 的可见性判据失败）"
            );
        }
    }
}

/// **`scroll → paged`**：锚点值不变，且字符落在某一页内。
#[test]
fn scroll_to_paged_preserves_anchor_and_visibility() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    // 每屏的首字符与其区间中段
    for viewer in flow.viewports() {
        for anchor in [
            viewer.char_range.start,
            (viewer.char_range.start + viewer.char_range.end) / 2,
            viewer.char_range.end.saturating_sub(1),
        ] {
            let mut switcher = ModeSwitcher::begin(ReadingMode::Scroll, anchor);
            let settled_anchor = switcher.settle(ReadingMode::Paged, &paginated);

            assert_eq!(
                settled_anchor, anchor,
                "scroll → paged 后锚点值必须不变（输入 {anchor}）"
            );
            assert!(
                switcher.target_visible(),
                "锚点 {anchor} 在翻页形态下必须可见"
            );

            let owner = paginated
                .pages
                .iter()
                .find(|page| page.char_range.contains(&anchor));
            assert!(
                owner.is_some(),
                "锚点 {anchor} 不属于任何页（SC-005b 的可见性判据失败）"
            );
        }
    }
}

/// **可见性判据能报告 false**——否则它是个恒真的空断言。
///
/// ## 为什么要专门测"不可见"
///
/// 本文件其它测试都断言 `target_visible() == true`（合法输入下确实可见）。
/// 若实现把可见性写成 `true`（或"有任意页就返回 true"），那些断言**全部
/// 照过**——一个恒真的判据与"正确的判据"在那些用例上无法区分。
///
/// 判据的可失败性必须由**反例**确定：一个确实不在任何区间内的位置必须报告
/// 不可见。故用"钳制前的越界锚点"构造成反例：
///
/// - `char_len`（正文末尾）本身**不属于**任何页区间（页区间是半开的，
///   上界最大到 `char_len - 1` 的所属页）
/// - 同样不属于任何视口区间
///
/// 故两形态下它都应是"不可见"。这正是"读到末尾"这个位置的**真实语义**：
/// 它是合法的进度值，但它不是一个"屏幕上可见的字符"。
///
/// 变异验证揭示了这一点：把 `ScrollFlow::anchor_is_visible` 改成恒 `true`
/// 后，原有用例全部通过。
#[test]
fn visibility_judgement_can_report_false() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    // 正文末尾：合法的锚点值，但不落在任何区间内（半开区间的上界之外）
    assert!(
        !flow.anchor_is_visible(char_len),
        "正文末尾不应报告为'可见'（它不属于任何视口区间）"
    );
    assert!(
        !paginated.anchor_is_visible(char_len),
        "正文末尾不应报告为'可见'（它不属于任何页区间）"
    );

    // 对照：末尾**前一个**字符确实可见（证明判据不是恒 false）
    if char_len > 0 {
        assert!(
            flow.anchor_is_visible(char_len - 1),
            "正文末尾的前一个字符应可见（判据不得恒为 false）"
        );
        assert!(
            paginated.anchor_is_visible(char_len - 1),
            "正文末尾的前一个字符应可见"
        );
    }

    // 首字符也不可见性判据的边界：它应可见
    assert!(flow.anchor_is_visible(0), "首字符应可见");
    assert!(paginated.anchor_is_visible(0), "首字符应可见");
}

/// **两方向各切一轮回到原形态**：锚点值全程不变（往返恒等）。
#[test]
fn round_trip_between_modes_keeps_anchor() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    for anchor in (0..char_len).step_by(37) {
        // paged → scroll
        let mut first = ModeSwitcher::begin(ReadingMode::Paged, anchor);
        let hop = first.settle(ReadingMode::Scroll, &flow);
        assert_eq!(hop, anchor, "第一跳改变了锚点");
        assert!(first.target_visible());

        // scroll → paged
        let mut second = ModeSwitcher::begin(ReadingMode::Scroll, hop);
        let back = second.settle(ReadingMode::Paged, &paginated);
        assert_eq!(back, anchor, "往返后锚点值变了（{anchor} → {back}）");
        assert!(second.target_visible());
    }
}

/// **同一组几何参数下两形态可切换**（FR-020 的前半句）。
#[test]
fn both_modes_share_the_same_geometry() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    // 版心是两形态共用的几何结果（FR-020："同一组几何参数下可切换"）
    assert!(
        (paginated.spread.measure_px - flow.measure_px()).abs() < 1e-3,
        "两形态的版心应相同（分页 {:.2}，滚动 {:.2}）",
        paginated.spread.measure_px,
        flow.measure_px()
    );
    // 章内字符总数相同
    assert_eq!(paginated.char_len, flow.char_len());
}

// ===========================================================================
// FR-023：切换过程中无进度写库
// ===========================================================================

/// **过渡中提交 → 零次写入**（FR-023 的"MUST NOT 以中间态写库"）。
#[test]
fn no_progress_write_during_transition() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);

    let mut sink = RecordingSink::default();
    let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, 300);

    // 过渡刚开始：不允许写
    assert!(!switcher.is_settled(), "刚 begin 的切换不该是稳定态");
    assert!(!switcher.commit(&mut sink), "过渡中 commit 应报告'未写入'");
    assert!(
        sink.writes.is_empty(),
        "过渡中不得写进度（实测写了 {:?}）",
        sink.writes
    );

    // 模拟若干次中间态（动画帧 / 中间布局）
    for _ in 0..TRANSCRIPTION_SETTLE_EVENTS {
        switcher.observe_event();
        if !switcher.is_settled() {
            assert!(!switcher.commit(&mut sink), "中间态 commit 应未写入");
            assert!(
                sink.writes.is_empty(),
                "中间态不得写进度（实测 {:?}）",
                sink.writes
            );
        }
    }

    // 稳定后提交一次
    switcher.settle(ReadingMode::Scroll, &flow);
    assert!(switcher.is_settled(), "settle 后应是稳定态");
    assert!(switcher.commit(&mut sink), "稳定后 commit 应报告'已写入'");

    assert_eq!(
        sink.writes.len(),
        1,
        "稳定后应恰好写一次（实测 {:?}）",
        sink.writes
    );
    assert_eq!(
        sink.writes[0].1, 300,
        "写入的锚点必须是稳定后的值（切换前的锚点）"
    );
}

/// **settle 后重复提交 → 不重复写**（幂等，防"每次事件都写一次库"）。
#[test]
fn settled_commit_is_idempotent() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);

    let mut sink = RecordingSink::default();
    let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, 120);
    switcher.settle(ReadingMode::Scroll, &flow);

    for _ in 0..5 {
        switcher.commit(&mut sink);
    }

    assert_eq!(
        sink.writes.len(),
        1,
        "settle 后多次 commit 只应写一次（实测 {} 次）",
        sink.writes.len()
    );
}

/// **写入的章号来自来源形态**（不得写 0 号章以外的值而掩盖错误）。
#[test]
fn committed_progress_carries_the_mode_anchor() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    // 逐屏 settle 并核对写入值
    for viewer in flow.viewports().iter().take(5) {
        let anchor = viewer.char_range.start;
        let mut sink = RecordingSink::default();
        let mut switcher = ModeSwitcher::begin(ReadingMode::Scroll, anchor);
        switcher.settle(ReadingMode::Paged, &paginated);
        switcher.commit(&mut sink);

        assert_eq!(sink.writes.len(), 1, "应恰好写一次");
        assert_eq!(
            sink.writes[0].1 as usize, anchor,
            "写入的锚点应与切换时的锚点一致"
        );
    }
}

/// **过渡中间态不得把位置写坏**（spec §Edge Cases）。
///
/// > 滚动模式与翻页模式之间快速来回切换：必须保持同一锚点，不得因切换中的
/// > 中间态把位置写坏（进度写入须以稳定后的锚点为准）
#[test]
fn rapid_mode_flapping_does_not_corrupt_position() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    let anchor = char_len / 3;
    let mut sink = RecordingSink::default();
    let mut current = anchor;

    // 快速来回切换 6 次（每次都走完整的 begin → 若干中间态 → settle）
    for round in 0..6 {
        let to_scroll = round % 2 == 0;
        let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, current);
        // 中间态：观察若干事件但不 settle（模拟快速切换发生的时机）
        for _ in 0..2 {
            switcher.observe_event();
            switcher.commit(&mut sink);
        }
        // 稳定
        current = if to_scroll {
            switcher.settle(ReadingMode::Scroll, &flow)
        } else {
            switcher.settle(ReadingMode::Paged, &paginated)
        };
        switcher.commit(&mut sink);
    }

    assert_eq!(current, anchor, "6 轮来回切换后锚点值被写坏");
    assert_eq!(
        sink.writes.len(),
        6,
        "每轮恰好一次写入（共 6 轮），实测 {}",
        sink.writes.len()
    );
    for (chapter_index, char_offset) in &sink.writes {
        assert_eq!(*chapter_index, 0);
        assert_eq!(
            *char_offset as usize, anchor,
            "每次写入都必须是稳定锚点（不得是中间态）"
        );
    }
}

// ===========================================================================
// 边界（spec §Edge Cases）
// ===========================================================================

/// 空章的切换可用（不得 panic）。
#[test]
fn empty_chapter_switch_is_safe() {
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate("", &geometry, &viewport)
        .expect("应成功");
    let flow = build_flow("", &geometry, &viewport);

    let mut a = ModeSwitcher::begin(ReadingMode::Paged, 0);
    assert_eq!(a.settle(ReadingMode::Scroll, &flow), 0);
    let mut b = ModeSwitcher::begin(ReadingMode::Scroll, 0);
    assert_eq!(b.settle(ReadingMode::Paged, &paginated), 0);
}

/// 越界锚点在切换中被**钳制**且两个方向钳制结果一致。
#[test]
fn out_of_range_anchor_is_clamped_consistently() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    let mut to_scroll = ModeSwitcher::begin(ReadingMode::Paged, char_len + 5000);
    let scroll_anchor = to_scroll.settle(ReadingMode::Scroll, &flow);
    let mut to_paged = ModeSwitcher::begin(ReadingMode::Scroll, char_len + 5000);
    let paged_anchor = to_paged.settle(ReadingMode::Paged, &paginated);

    assert_eq!(
        scroll_anchor, paged_anchor,
        "两方向的越界钳制结果必须一致（否则切换会改变锚点）"
    );
    assert_eq!(scroll_anchor, char_len, "应钳制到正文末尾");
}

/// 切换的**确定性**：同输入两次切换结果相同。
#[test]
fn switching_is_deterministic() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);

    for anchor in [0usize, 100, 500, 1000] {
        let mut a = ModeSwitcher::begin(ReadingMode::Paged, anchor);
        let anchor_a = a.settle(ReadingMode::Scroll, &flow);
        let mut b = ModeSwitcher::begin(ReadingMode::Paged, anchor);
        let anchor_b = b.settle(ReadingMode::Scroll, &flow);
        assert_eq!(anchor_a, anchor_b);
        assert_eq!(a.target_visible(), b.target_visible());
    }
}
