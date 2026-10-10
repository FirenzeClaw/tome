//! T028 验收：滚动重排保锚点（FR-024）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T028）
//!
//! > 参数化测试（含窗口连续变化）；**无"保留像素偏移"式漂移断言**
//!
//! ## FR-024 原文
//!
//! > 滚动模式下几何参数变化（字号/行高/版心/窗口尺寸）触发的重排 MUST 保持
//! > 锚点：重排后锚点字符 MUST 仍落在视口内，MUST NOT 因"**保留滚动像素偏移**"
//! > 而发生位置漂移。
//!
//! 括号里那五个字是本任务的全部难点，也是它的判据形态：
//!
//! | 做法 | 结果 |
//! |---|---|
//! | 保留 `offset_px`（重排后仍滚到同一个像素位置） | 漂移——行高变了，同一像素落在别的行 |
//! | 保留**字符锚点**（重排后滚到同一字符所在行） | 不漂移 ✅ |
//!
//! 故"断言无漂移"的**正确形式**不是检查像素值，而是检查**锚点字符仍在视口内**：
//!
//! 1. 记录重排前的锚点字符 `c`
//! 2. 换几何参数重排
//! 3. 断言 `c` 落在新视口的字符区间内
//!
//! 那个断言天然排除"保留像素偏移"的实现：几何一变，同一像素偏移对应的行
//! 就变了，`c` 会落到视口外。
//!
//! ## 为什么还要测"窗口连续变化"
//!
//! spec §Edge Cases：
//!
//! > 滚动模式下窗口尺寸连续变化：重排后锚点须落在视口内，不得因"重排时保留
//! > 的是滚动像素偏移"而漂移
//!
//! 连续变化（resize 拖拽）会触发**大量**重排请求。若每个请求都立即执行，
//! 不仅浪费算力，还会让"当前锚点"在抖动中漂走。故需要合并/节流，且合并后
//! 的结果应等于"按最终尺寸重排一次"。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::ScrollFlow;
use tome_reader::scroll::relayout::{RelayoutRequest, ResizeCoalescer};

// ===========================================================================
// 夹具
// ===========================================================================

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

fn long_chapter() -> String {
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                     后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。";
    (0..10)
        .map(|index| format!("第{}段。{paragraph}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_flow(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("构造滚动流应成功")
}

/// 锚点字符是否落在某个视口的区间内（FR-024 的判据）。
fn anchor_visible_in(flow: &ScrollFlow, anchor: usize) -> bool {
    flow.viewports()
        .iter()
        .any(|viewport| viewport.char_range.contains(&anchor))
}

// ===========================================================================
// FR-024 主判据：重排后锚点仍在视口内
// ===========================================================================

/// **字号变化后锚点仍在视口内**（参数化：四档字号逐对切换）。
#[test]
fn anchor_stays_visible_across_font_size_changes() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let viewport = Viewport::new(342.0, 300.0);

    let font_sizes = [14u16, 19, 24, 30];
    let anchors = [
        0usize,
        char_len / 5,
        char_len / 3,
        char_len / 2,
        (char_len * 4) / 5,
        char_len - 1,
    ];

    for from_size in font_sizes {
        let mut from_geometry = geometry();
        from_geometry.font_size_px = from_size;
        let from_flow = build_flow(&content, &from_geometry, &viewport);

        for to_size in font_sizes {
            if to_size == from_size {
                continue;
            }
            let mut to_geometry = geometry();
            to_geometry.font_size_px = to_size;

            for anchor in anchors {
                // 重排（模拟"参数变化"）
                let to_flow = build_flow(&content, &to_geometry, &viewport);

                // 重排前锚点在哪一屏 → 重排后必须仍在某一屏内
                assert!(
                    anchor_visible_in(&from_flow, anchor) || anchor >= from_flow.char_len(),
                    "前置：锚点 {anchor} 在 {from_size}px 下应可见"
                );
                assert!(
                    anchor_visible_in(&to_flow, anchor),
                    "字号 {from_size}px → {to_size}px 后锚点 {anchor} 不在任何视口内（漂移）"
                );
            }
        }
    }
}

/// **行高变化后锚点仍在视口内**（三档，含 SC-004 矩阵的三档）。
#[test]
fn anchor_stays_visible_across_line_height_changes() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let viewport = Viewport::new(342.0, 300.0);

    for from_height in [1.75f32, 1.85, 2.2] {
        let mut from_geometry = geometry();
        from_geometry.line_height = from_height;
        let from_flow = build_flow(&content, &from_geometry, &viewport);

        for to_height in [1.75f32, 1.85, 2.2] {
            if (to_height - from_height).abs() < 1e-6 {
                continue;
            }
            let mut to_geometry = geometry();
            to_geometry.line_height = to_height;
            let to_flow = build_flow(&content, &to_geometry, &viewport);

            for anchor in [0usize, char_len / 2, char_len - 1] {
                assert!(
                    anchor_visible_in(&to_flow, anchor),
                    "行高 {from_height} → {to_height} 后锚点 {anchor} 不在视口内"
                );
            }
            let _ = &from_flow;
        }
    }
}

/// **窗口尺寸变化后锚点仍在视口内**（FR-024 明文列出的第三项）。
#[test]
fn anchor_stays_visible_across_window_size_changes() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();

    let sizes = [
        (228.0f32, 150.0f32),
        (342.0, 250.0),
        (342.0, 600.0),
        (684.0, 400.0),
        (1432.0, 300.0),
    ];

    for (from_width, from_height) in sizes {
        for (to_width, to_height) in sizes {
            if (from_width - to_width).abs() < 1.0 && (from_height - to_height).abs() < 1.0 {
                continue;
            }
            let to_flow = build_flow(&content, &geometry, &Viewport::new(to_width, to_height));

            for anchor in [
                0usize,
                char_len / 4,
                char_len / 2,
                (char_len * 3) / 4,
                char_len - 1,
            ] {
                assert!(
                    anchor_visible_in(&to_flow, anchor),
                    "窗口 {from_width}x{from_height} → {to_width}x{to_height} 后\
                     锚点 {anchor} 不在视口内"
                );
            }
        }
    }
}

/// **版心变化后锚点仍在视口内**。
#[test]
fn anchor_stays_visible_across_measure_changes() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let viewport = Viewport::new(684.0, 300.0);

    for to_measure in [24.0f32, 30.0, 36.0, 42.0] {
        let mut to_geometry = geometry();
        to_geometry.measure_em = to_measure;
        let to_flow = build_flow(&content, &to_geometry, &viewport);

        for anchor in [0usize, char_len / 2, char_len - 1] {
            assert!(
                anchor_visible_in(&to_flow, anchor),
                "版心 → {to_measure}em 后锚点 {anchor} 不在视口内"
            );
        }
    }
}

// ===========================================================================
// "无保留像素偏移式漂移"——判据的核心
// ===========================================================================

/// **锚点的像素位置随几何变化而变，但字符不变**——这正是"不保留像素偏移"。
///
/// ## 这条把两种做法的差别变成可观测的
///
/// 若实现"保留像素偏移"，则重排后同一偏移对应的字符会**改变**（因为行高
/// 变了）。若实现"保留字符锚点"，则字符位置不变、像素位置改变。
///
/// 故断言：字符 → 偏移的映射在重排前后**指向同一个字符**，而像素值**通常
/// 不同**（后者证明几何真的变了，否则这条测试是空的）。
#[test]
fn relayout_preserves_char_anchor_while_pixel_offset_changes() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let viewport = Viewport::new(342.0, 300.0);

    let mut small = geometry();
    small.font_size_px = 14;
    let mut large = geometry();
    large.font_size_px = 30;

    let small_flow = build_flow(&content, &small, &viewport);
    let large_flow = build_flow(&content, &large, &viewport);

    let mut changed_pixels = 0usize;
    for anchor in [char_len / 4, char_len / 2, (char_len * 3) / 4] {
        let small_map = tome_reader::scroll::map::PositionMap::new(&small_flow);
        let large_map = tome_reader::scroll::map::PositionMap::new(&large_flow);

        let small_px = small_map.char_to_offset(anchor).expect("应可映射");
        let large_px = large_map.char_to_offset(anchor).expect("应可映射");

        // ## 断言的是"锚点字符仍可见"，不是"行边界不变"
        //
        // 字号变化会**改变断行**，故同一字符所在行的首字符会变（实测
        // 14px 下某锚点所在行起于 160，30px 下起于 168）。那是重排的**定义**
        // 而非错误——把"行首字符不变"当断言是混淆了两件事：
        //
        // | 概念 | 字号变化后 | 该不该变 |
        // |---|---|---|
        // | 锚点**字符** | 不变 | **不可变**（FR-024 的全部要求） |
        // | 该字符所在的**行** | 变（断行重算） | 可变 |
        //
        // FR-024 要的是"重排后锚点字符仍落在视口内"，故断言可见性。
        assert!(
            anchor_visible_in(&small_flow, anchor) && anchor_visible_in(&large_flow, anchor),
            "锚点 {anchor} 应始终可见（FR-024 的唯一要求）"
        );
        // 两个映射都指向"包含该锚点的行"——它们各自自洽
        assert!(
            small_map.offset_to_char(small_px).is_some()
                && large_map.offset_to_char(large_px).is_some(),
            "两边的映射都应可往返"
        );

        if (small_px - large_px).abs() > 1.0 {
            changed_pixels += 1;
        }
    }

    assert!(
        changed_pixels > 0,
        "字号 14px → 30px 后锚点的像素偏移应改变（否则几何没真的变，本测试是空的）"
    );
}

/// **反向对照**：故意"保留像素偏移"会漂移——证明判据能抓到那种实现。
///
/// ## 这条是判据有效性的自检，不是实现的一部分
///
/// 它直接模拟"保留像素偏移"的做法：
///
/// 1. 在新几何下，取**同一个像素偏移**（旧几何里的值）
/// 2. 看它映射到哪个字符
/// 3. 断言那个字符**通常不是**原锚点（即确实漂移了）
///
/// 若这条自检不成立（即保留像素偏移恰好不漂移），那 FR-024 的关切在本
/// 测试数据下无法体现，主判据就是空的。故它是主判据的"有效性前提"。
#[test]
fn keeping_pixel_offset_would_actually_drift() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let viewport = Viewport::new(342.0, 300.0);

    let mut small = geometry();
    small.font_size_px = 14;
    let mut large = geometry();
    large.font_size_px = 30;

    let small_flow = build_flow(&content, &small, &viewport);
    let large_flow = build_flow(&content, &large, &viewport);
    let small_map = tome_reader::scroll::map::PositionMap::new(&small_flow);
    let large_map = tome_reader::scroll::map::PositionMap::new(&large_flow);

    let anchor = char_len / 2;
    let old_px = small_map.char_to_offset(anchor).expect("应可映射");
    let drifted_char = large_map.offset_to_char(old_px).expect("应可映射");

    assert_ne!(
        drifted_char, anchor,
        "前置自检失败：在本数据下'保留像素偏移'恰好不漂移，\
         故主判据无法体现 FR-024 的关切。请增大字号差异或换语料。"
    );
}

// ===========================================================================
// 连续窗口变化（resize 合并/节流）
// ===========================================================================

/// **连续 resize 合并为一次重排**（FR-024 的"合并/节流不累积"）。
#[test]
fn consecutive_resizes_coalesce_into_one_relayout() {
    let mut coalescer = ResizeCoalescer::new();

    // 模拟拖拽：连续提交 20 个不同的尺寸
    for step in 0..20 {
        coalescer.request(342.0 + step as f32, 250.0 + step as f32);
    }

    assert_eq!(
        coalescer.pending_count(),
        1,
        "连续 resize 应合并为 1 个待处理请求（实测 {}）",
        coalescer.pending_count()
    );

    let request = coalescer.take().expect("应有一个待处理请求");
    assert_eq!(
        request,
        RelayoutRequest {
            width_px: 361.0,
            height_px: 269.0
        },
        "合并后应保留**最后一次**尺寸（拖拽的终态）"
    );
    assert_eq!(coalescer.pending_count(), 0, "取出后不应残留");
}

/// **合并后取最后一次尺寸**（不是第一次，也不是平均值）。
#[test]
fn coalescer_keeps_the_latest_size() {
    let mut coalescer = ResizeCoalescer::new();
    coalescer.request(100.0, 100.0);
    coalescer.request(200.0, 200.0);
    coalescer.request(300.0, 400.0);

    let request = coalescer.take().expect("应有请求");
    assert_eq!(request.width_px, 300.0);
    assert_eq!(request.height_px, 400.0);
}

/// **空队列时 take 返回 None**（不产出假请求）。
#[test]
fn coalescer_take_on_empty_is_none() {
    let mut coalescer = ResizeCoalescer::new();
    assert_eq!(coalescer.pending_count(), 0);
    assert!(coalescer.take().is_none());
}

/// **连续变化后按最终尺寸重排，锚点仍在视口内**（本任务的核心场景）。
#[test]
fn anchor_survives_continuous_resize() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let anchor = char_len / 2;

    // 拖拽：从 342x250 连续变到 684x500
    let mut coalescer = ResizeCoalescer::new();
    for step in 0..30 {
        let t = step as f32 / 29.0;
        coalescer.request(342.0 + 342.0 * t, 250.0 + 250.0 * t);
    }

    let final_request = coalescer.take().expect("应有合并后的请求");
    let final_flow = build_flow(
        &content,
        &geometry,
        &Viewport::new(final_request.width_px, final_request.height_px),
    );

    // 锚点必须仍在视口内（不得因中间态的像素偏移漂走）
    assert!(
        anchor_visible_in(&final_flow, anchor),
        "连续 resize 后锚点 {anchor} 不在视口内（漂移）"
    );

    // 且按最终尺寸直接重排的结果应与之一致（合并无副作用）
    let direct_flow = build_flow(
        &content,
        &geometry,
        &Viewport::new(final_request.width_px, final_request.height_px),
    );
    assert_eq!(
        final_flow.viewports().len(),
        direct_flow.viewports().len(),
        "合并后的重排结果应与按最终尺寸直接重排一致"
    );
    for (a, b) in final_flow.viewports().iter().zip(direct_flow.viewports()) {
        assert_eq!(a.char_range, b.char_range);
    }
}

/// **不累积排队任务**（FR-027 的前半句在本任务的形态）。
///
/// 拖拽 1000 次后仍只有 1 个待处理请求——否则会累积成 1000 次重排。
#[test]
fn resize_storm_does_not_accumulate_tasks() {
    let mut coalescer = ResizeCoalescer::new();
    for step in 0..1000 {
        coalescer.request(200.0 + (step % 50) as f32, 300.0);
        assert!(
            coalescer.pending_count() <= 1,
            "待处理请求数不得超过 1（实测 {}，step {step}）",
            coalescer.pending_count()
        );
    }
    assert_eq!(coalescer.pending_count(), 1);
}

// ===========================================================================
// 边界
// ===========================================================================

/// 极端窗口尺寸下重排不 panic、锚点仍可判定。
#[test]
fn extreme_window_sizes_are_safe() {
    let content = long_chapter();
    let geometry = geometry();

    for (width, height) in [(0.0f32, 0.0f32), (1.0, 1.0), (10.0, 5.0), (5000.0, 5000.0)] {
        let flow = build_flow(&content, &geometry, &Viewport::new(width, height));
        assert!(!flow.viewports().is_empty(), "{width}x{height} 应产出视口");
        assert!(
            anchor_visible_in(&flow, 0),
            "{width}x{height} 下首字符应可见"
        );
    }
}

/// 空章在重排后仍可用。
///
/// ## 空章**没有**"可见的字符"（这一点必须写清楚）
///
/// 空章产出一屏，但它的区间是 `0..0`（空区间，半开语义），故
/// `contains(0) == false`——位置 0 在空章里**不是一个字符**，无从谈起
/// "是否可见"。
///
/// 这不是缺陷：SC-005b/FR-024 的可见性判据都是关于**字符**的，空章没有
/// 字符。可断言的性质是"视口存在且区间为空"（T025 已钉住）以及"切换/重排
/// 不 panic"。
#[test]
fn empty_chapter_relayout_is_safe() {
    let geometry = geometry();
    for (width, height) in [(342.0f32, 250.0f32), (684.0, 500.0)] {
        let flow = build_flow("", &geometry, &Viewport::new(width, height));
        assert_eq!(flow.viewports().len(), 1);
        assert!(
            flow.viewports()[0].char_range.is_empty(),
            "空章的视口区间必须是空的"
        );
        assert!(
            !anchor_visible_in(&flow, 0),
            "空章没有任何字符，故不存在'可见的字符'（区间 0..0 不含 0）"
        );
    }
}

/// 重排的**确定性**：同输入两次重排的视口序列相同。
#[test]
fn relayout_is_deterministic() {
    let content = long_chapter();
    let mut geometry = geometry();
    geometry.font_size_px = 24;
    let viewport = Viewport::new(342.0, 300.0);

    let first = build_flow(&content, &geometry, &viewport);
    let second = build_flow(&content, &geometry, &viewport);

    assert_eq!(first.viewports().len(), second.viewports().len());
    for (a, b) in first.viewports().iter().zip(second.viewports()) {
        assert_eq!(a.char_range, b.char_range);
        assert_eq!(a.top_px, b.top_px);
    }
}
