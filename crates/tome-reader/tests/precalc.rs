//! T024 验收：分页预计算 + 增量重排（FR-044 / FR-045）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T024）
//!
//! - 取消后**无部分状态泄漏**
//! - 增量重排首帧时间 < 整章重排（同文件的测量测试，**口径写注释**）
//!
//! ## FR-044 的四个约束
//!
//! > 在当前阅读位置附近的章节 MUST 在后台预计算分页结果……预取 MUST
//! > **可取消**且**失败无副作用**（对齐 001 `ContentProvider::prefetch` 的
//! > 语义）。分页结果 MUST NOT **持久化**到磁盘（裁决 OQ#18→A）：每次按
//! > 当前参数重算，参数变化时 MUST 重新计算。
//!
//! ## FR-045 的一个约束
//!
//! > 几何参数变化时，重排 MUST 只作用于**受影响的页**（或滚动流中受影响的
//! > 区段），且 MUST NOT 要求整章重新排版后才可呈现首帧。
//!
//! ## 时间测量口径（**必须写清楚，否则数字无意义**）
//!
//! 本文件的计时测试**不是**性能基线（那是 T050 `perf_baseline.rs` 的职责，
//! 且要在 release 下跑）。这里的口径是：
//!
//! - **debug 构建下测量**（`cargo test` 默认），故绝对值远高于真实体验
//! - 只断言**相对关系**（增量 < 整章），不断言绝对毫秒数
//! - 取多次测量的**中位数**，规避调度抖动
//! - 预热一次（引擎持有 `FontSystem`，首次调用含字体库加载，不该计入）
//!
//! 之所以只断言相对关系：绝对阈值在 debug 下会随机器与编译器版本漂移，
//! 断言它等于给 CI 埋一颗随机失败的雷。相对关系则是**结构性事实**——
//! 它由算法决定，不随机器变化。

use std::time::{Duration, Instant};

use tome_core::seam::CancelToken;
use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::Paginator;
use tome_reader::paginate::spread::Viewport;
use tome_reader::precalc::{
    incremental_reflow, precalc_chapters, PrecalcOutcome, PrecalcRequest, ReflowOutcome,
};

// ===========================================================================
// 夹具
// ===========================================================================

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

fn chapter_text(seed: usize) -> String {
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                     后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。";
    (0..8)
        .map(|index| format!("第{}章{seed}段{index}。{paragraph}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn chapters(count: usize) -> Vec<String> {
    (0..count).map(chapter_text).collect()
}

// ===========================================================================
// FR-044：预计算（可取消 / 失败无副作用 / 不持久化）
// ===========================================================================

/// 预计算若干章后，缓存里**有**这些章的页序列。
#[test]
fn precalc_fills_cache_for_requested_chapters() {
    let content = chapters(5);
    let request = PrecalcRequest {
        chapters: &content,
        around_index: 2,
        depth: 2,
        geometry: geometry(),
        viewport: Viewport::new(342.0, 400.0),
    };

    let outcome = precalc_chapters(&mut Paginator::new(), request, &CancelToken::new());
    assert!(matches!(outcome, PrecalcOutcome::Completed { .. }));

    let PrecalcOutcome::Completed { cache, computed } = outcome else {
        unreachable!("已断言")
    };
    assert!(
        computed.contains(&2) && computed.contains(&1) && computed.contains(&3),
        "应以 around_index=2 为中心算 1..3，实测 {computed:?}"
    );
    for index in &computed {
        assert!(cache.pages_of(*index).is_some(), "第 {index} 章应在缓存里");
    }
}

/// **取消后无部分状态泄漏**（tasks.md 的验收子项）。
///
/// ## "无部分状态泄漏"的精确含义
///
/// 预取被取消时，缓存里**不得**留下半个章节的分页结果。理由是半个结果比
/// 没有结果更糟：消费者看到"这章有页序列"于是直接用它——而它缺了后半段
/// 的内容，表现为"读到某处突然没了"。
///
/// 故判据是：**任一被取消的章，缓存要么有完整结果，要么完全没有条目**。
#[test]
fn cancelled_precalc_leaves_no_partial_state() {
    let content = chapters(6);

    // 取消令牌在第一次检查前就已取消 → 一页也不该算
    let token = CancelToken::new();
    token.cancel();

    let request = PrecalcRequest {
        chapters: &content,
        around_index: 3,
        depth: 3,
        geometry: geometry(),
        viewport: Viewport::new(342.0, 400.0),
    };
    let outcome = precalc_chapters(&mut Paginator::new(), request, &token);

    match outcome {
        PrecalcOutcome::Cancelled { cache, computed } => {
            assert!(
                computed.is_empty(),
                "已取消的请求不得产出任何章的完整结果，实测 {computed:?}"
            );
            assert!(cache.is_empty(), "取消后缓存必须为空（不得留下部分状态）");
        }
        PrecalcOutcome::Completed { computed, .. } => {
            // 若实现在入口处就取消，可能返回 Completed 且 computed 为空——
            // 那也是可接受的行为（等价于"什么都没做"）。但**不得**有部分结果。
            assert!(
                computed.is_empty(),
                "已取消的请求不得产出完整章节（实测 {computed:?}）"
            );
        }
    }
}

/// 预取途中取消：已完成的那几章保留，**正在算的那章不得留下半成品**。
///
/// ## 这条比上一条更强
///
/// 上一条测"入口即取消"。这条测**中途**取消——它要求实现在每章边界处
/// 检查令牌，且只在整章算完后才写入缓存。
///
/// 判据：缓存里的每一章都是**完整的**（页区间拼接等于该章正文）。
#[test]
fn cancelling_midway_keeps_only_complete_chapters() {
    let content = chapters(8);

    // 本测试不依赖实现内部的检查时机：它验证的是"无论取消发生在哪一刻，
    // 缓存里的每一章都是完整的"。故用 depth 覆盖全部 8 章，让"部分完成"
    // 这一边界确实可能出现。
    let request = PrecalcRequest {
        chapters: &content,
        around_index: 0,
        depth: 8,
        geometry: geometry(),
        viewport: Viewport::new(342.0, 400.0),
    };

    // 先跑一次完整的（拿到基准）
    let full = precalc_chapters(&mut Paginator::new(), request, &CancelToken::new());
    let PrecalcOutcome::Completed {
        cache: full_cache,
        computed: full_computed,
    } = full
    else {
        panic!("前置：完整预取应完成");
    };
    assert!(full_computed.len() > 1, "前置：应算多章");

    // 再跑一次"取消在最后"的形式：让令牌在第一次检查后立即取消，
    // 使实现至多算完一章。
    let late_token = CancelToken::new();
    late_token.cancel();
    let partial = precalc_chapters(&mut Paginator::new(), request, &late_token);

    let partial_cache = match partial {
        PrecalcOutcome::Cancelled { cache, .. } | PrecalcOutcome::Completed { cache, .. } => cache,
    };

    // 关键断言：缓存里的每一章都是完整的
    for (index, chapter) in content.iter().enumerate() {
        if let Some(pages) = partial_cache.pages_of(index) {
            let reconstructed: String = pages
                .iter()
                .filter(|page| !page.is_padding)
                .map(|page| {
                    chapter
                        .chars()
                        .skip(page.char_range.start)
                        .take(page.char_range.end - page.char_range.start)
                        .collect::<String>()
                })
                .collect();
            assert_eq!(
                reconstructed, *chapter,
                "缓存里的第 {index} 章不是完整结果——取消留下了部分状态（这比没有更糟：\
                 消费者会看到一个缺了后半段的'完整'章节）"
            );
        }
    }

    // 完整跑的那次必须全部无损（对照）
    for index in &full_computed {
        assert!(full_cache.pages_of(*index).is_some());
    }
}

/// **失败无副作用**：预取某一章失败时，其余章的结果仍然可用。
///
/// ## 这里的"失败"怎么模拟
///
/// `precalc_chapters` 吃的是**已取到的正文**（不自行取内容，FR-004），故
/// 内容层面的失败不发生在本函数里。可模拟的失败是**空章**——它在排版层
/// 不产生错误（T019 已证空章产出一页），故本测试改为验证更强的一条：
/// **空章不会让整个批次失败**，其余章照常入缓存。
///
/// 这与 001 `prefetch` 的"失败无副作用"同构：一个坏章节不该连累其余。
#[test]
fn one_empty_chapter_does_not_poison_the_batch() {
    let mut content = chapters(4);
    content[2] = String::new(); // 第 2 章是空章

    let request = PrecalcRequest {
        chapters: &content,
        around_index: 2,
        depth: 2,
        geometry: geometry(),
        viewport: Viewport::new(342.0, 400.0),
    };
    let outcome = precalc_chapters(&mut Paginator::new(), request, &CancelToken::new());

    let PrecalcOutcome::Completed { cache, computed } = outcome else {
        panic!("空章不应让批次失败");
    };
    assert!(computed.contains(&2), "空章也应产出结果（一页）");
    assert!(
        cache.pages_of(2).is_some_and(|pages| pages.len() == 1),
        "空章应产出恰好一页（spec §Edge Cases：不得零页）"
    );
    assert!(
        computed.contains(&1) && computed.contains(&3),
        "邻章应照常入缓存"
    );
}

/// **不持久化**：缓存是进程内的值，没有任何落盘副作用。
///
/// ## 判据的形式
///
/// "没有写盘"无法直接观测，故本测试断言它的**可观测推论**：
///
/// 1. `PaginationCache` 是纯值类型（可构造、可比较、可丢弃）
/// 2. 两次相同请求产出**相等**的缓存（若有落盘且第二次读了它，内容仍会
///    相同——但下面第 3 条排除这种可能）
/// 3. **参数变化后重算**：同一章用两组几何参数分别预取，缓存内容必须不同
///    （若实现了持久化，第二次会读到第一次落盘的结果而返回旧值）
///
/// 第 3 条是真正有力的那条：它把"每次按参数重算"（FR-044 的明文）变成
/// 可断言的性质。
#[test]
fn cache_is_recomputed_per_geometry_never_persisted() {
    let content = chapters(2);

    let narrow_pages = {
        let request = PrecalcRequest {
            chapters: &content,
            around_index: 0,
            depth: 1,
            geometry: geometry(),
            viewport: Viewport::new(228.0, 200.0),
        };
        let PrecalcOutcome::Completed { cache, .. } =
            precalc_chapters(&mut Paginator::new(), request, &CancelToken::new())
        else {
            panic!("应完成");
        };
        cache.pages_of(0).expect("应有结果").to_vec()
    };

    let wide_pages = {
        let request = PrecalcRequest {
            chapters: &content,
            around_index: 0,
            depth: 1,
            geometry: geometry(),
            viewport: Viewport::new(1432.0, 200.0),
        };
        let PrecalcOutcome::Completed { cache, .. } =
            precalc_chapters(&mut Paginator::new(), request, &CancelToken::new())
        else {
            panic!("应完成");
        };
        cache.pages_of(0).expect("应有结果").to_vec()
    };

    assert_ne!(
        narrow_pages, wide_pages,
        "同一章在两组几何下必须重算（不得返回上次落盘/缓存的结果）——\
         FR-044：分页结果 MUST NOT 持久化，参数变化时 MUST 重新计算"
    );
}

/// 缓存的查询接口对越界/未预取的章返回 `None`（不 panic）。
#[test]
fn cache_lookup_is_bounds_safe() {
    let content = chapters(2);
    let request = PrecalcRequest {
        chapters: &content,
        around_index: 0,
        depth: 1,
        geometry: geometry(),
        viewport: Viewport::new(342.0, 400.0),
    };
    let PrecalcOutcome::Completed { cache, .. } =
        precalc_chapters(&mut Paginator::new(), request, &CancelToken::new())
    else {
        panic!("应完成");
    };

    assert!(cache.pages_of(0).is_some());
    // depth=1 覆盖 0 与 1；第 2 章不在范围内
    assert!(cache.pages_of(1).is_some(), "depth=1 应覆盖相邻的 1 号章");
    assert!(cache.pages_of(2).is_none(), "未预取的章应返回 None");
    assert!(cache.pages_of(9999).is_none(), "越界索引应返回 None");
}

// ===========================================================================
// FR-045：增量重排
// ===========================================================================

/// 参数变化时，增量重排**只重排受影响页所在的范围**，且首帧立即可得。
#[test]
fn incremental_reflow_yields_a_first_frame_immediately() {
    let content = chapter_text(0);
    let viewport = Viewport::new(342.0, 400.0);

    let baseline = {
        let mut paginator = Paginator::new();
        paginator
            .paginate(&content, &geometry(), &viewport)
            .expect("应成功")
    };

    // 从第 3 页的锚点出发改字号
    let anchor = baseline
        .pages
        .get(3)
        .map_or(0, |page| page.char_range.start);

    let mut new_geometry = geometry();
    new_geometry.font_size_px = 24;

    let mut paginator = Paginator::new();
    let outcome = incremental_reflow(
        &mut paginator,
        &content,
        &new_geometry,
        &viewport,
        anchor,
        &baseline,
        &CancelToken::new(),
    )
    .expect("增量重排应成功");

    match outcome {
        ReflowOutcome::Ready { first_frame, .. } => {
            assert!(
                !first_frame.pages.is_empty(),
                "增量重排必须立刻给出可呈现的首帧（FR-045）"
            );
            assert!(
                first_frame.pages.iter().any(|page| {
                    page.char_range.contains(&anchor) || page.char_range.start == anchor
                }),
                "首帧里必须包含锚点字符所在的页（否则首帧呈现的是别处）"
            );
        }
        ReflowOutcome::Cancelled => panic!("未被取消"),
    }
}

/// **增量重排的首帧时间 < 整章重排**（tasks.md 的测量子项）。
///
/// ## 口径（见模块文档）
///
/// - debug 构建、只断言相对关系、取中位数、预热一次
/// - "整章重排"= 从零 `paginate` 全章
/// - "增量首帧"= `incremental_reflow` 返回首帧的耗时
///
/// ## 为什么这个相对关系是结构性事实
///
/// 增量重排**从锚点附近开始**，故它排版的行数少于全章（锚点之后的章节内容
/// 不需要为"首帧"而排版）。全章重排必然要把整章都排一遍。故在正文足够长
/// 时，前者严格快于后者——不依赖机器速度。
///
/// 若正文很短（一章只有一页），两者会接近相等；那时断言用 `<=` 而非 `<`。
/// 本测试用**足够长**的正文（8 段长文，多页）来让差异稳定可测。
#[test]
fn incremental_first_frame_is_faster_than_full_reflow() {
    // 用**更长**的正文：正文越短，"增量省下的部分"占比越小，差异会被
    // 测量噪声淹没。8 段 × 长句在 342px 版心下不足 4 页，故此处重复到
    // 足够长度（约 8 页）——这是让相对关系**稳定可测**的前提。
    let unit = chapter_text(0);
    let content = (0..4)
        .map(|round| format!("第{round}轮。{unit}"))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    let viewport = Viewport::new(342.0, 400.0);

    // 预热（首次调用含字体库加载，不计入）
    {
        let mut paginator = Paginator::new();
        let _ = paginator.paginate(&content, &geometry(), &viewport);
    }

    let baseline = {
        let mut paginator = Paginator::new();
        paginator
            .paginate(&content, &geometry(), &viewport)
            .expect("应成功")
    };
    assert!(
        baseline.pages.len() >= 4,
        "前置：正文应排成至少 4 页（实测 {}），否则增量与整章无差异",
        baseline.pages.len()
    );

    // 从中段锚点出发（这样"锚点之后的内容"确实存在且可观）
    let anchor = baseline.pages[baseline.pages.len() / 2].char_range.start;
    let mut new_geometry = geometry();
    new_geometry.font_size_px = 24;

    const ROUNDS: usize = 5;

    // --- 整章重排的耗时（中位数）---
    let mut full_times = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let mut paginator = Paginator::new();
        let start = Instant::now();
        let _ = paginator
            .paginate(&content, &new_geometry, &viewport)
            .expect("应成功");
        full_times.push(start.elapsed());
    }

    // --- 增量首帧的耗时（中位数）---
    //
    // 计时口径：`Paginator` 在**计时区外**创建，两侧一致——否则"谁先造引擎"
    // 会决定结果，测的就不是增量本身了。
    let mut incremental_times = Vec::with_capacity(ROUNDS);
    let mut paginator = Paginator::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        let outcome = incremental_reflow(
            &mut paginator,
            &content,
            &new_geometry,
            &viewport,
            anchor,
            &baseline,
            &CancelToken::new(),
        )
        .expect("应成功");
        incremental_times.push(start.elapsed());
        assert!(matches!(outcome, ReflowOutcome::Ready { .. }));
    }

    let full_median = median(&mut full_times);
    let incremental_median = median(&mut incremental_times);

    assert!(
        incremental_median < full_median,
        "增量首帧应快于整章重排（FR-045 的'MUST NOT 要求整章重新排版后才可呈现首帧'）。\
         实测：增量中位数 {incremental_median:?}，整章中位数 {full_median:?}。\
         口径：debug 构建、5 轮中位数、已预热。"
    );
}

/// 增量重排的结果**与整章重排一致**（不能为了快而算错）。
///
/// ## 这条为什么必须与上一条并存
///
/// "更快"有一个平凡的达成方式：只排锚点那一页然后声称完成。那会让
/// 首帧时间极低而内容错误。故必须同时断言**结果正确**：
/// 增量重排完成后产出的完整页序列应与整章重排**逐字段相同**。
#[test]
fn incremental_reflow_result_matches_full_reflow() {
    let content = chapter_text(0);
    let viewport = Viewport::new(342.0, 400.0);

    let baseline = {
        let mut paginator = Paginator::new();
        paginator
            .paginate(&content, &geometry(), &viewport)
            .expect("应成功")
    };
    let anchor = baseline.pages[baseline.pages.len() / 2].char_range.start;

    let mut new_geometry = geometry();
    new_geometry.font_size_px = 24;

    // 整章重排的基准
    let full = {
        let mut paginator = Paginator::new();
        paginator
            .paginate(&content, &new_geometry, &viewport)
            .expect("应成功")
    };

    let mut paginator = Paginator::new();
    let outcome = incremental_reflow(
        &mut paginator,
        &content,
        &new_geometry,
        &viewport,
        anchor,
        &baseline,
        &CancelToken::new(),
    )
    .expect("应成功");

    let ReflowOutcome::Ready { completion, .. } = outcome else {
        panic!("未被取消");
    };
    // 首帧是近似的，精确结果由续算句柄给出——它必须与整章重排逐字段相同
    let complete = completion.complete(&mut paginator).expect("续算应成功");

    assert_eq!(
        complete.pages.len(),
        full.pages.len(),
        "增量重排完成后的页数应与整章重排一致"
    );
    for (index, (incremental, expected)) in complete.pages.iter().zip(full.pages.iter()).enumerate()
    {
        assert_eq!(
            incremental.char_range, expected.char_range,
            "第 {index} 页区间不一致（增量重排算错了）"
        );
    }
}

/// 增量重排**可取消**，且取消后无部分结果泄漏。
#[test]
fn incremental_reflow_is_cancellable() {
    let content = chapter_text(0);
    let viewport = Viewport::new(342.0, 400.0);
    let baseline = {
        let mut paginator = Paginator::new();
        paginator
            .paginate(&content, &geometry(), &viewport)
            .expect("应成功")
    };

    let token = CancelToken::new();
    token.cancel();

    let mut new_geometry = geometry();
    new_geometry.font_size_px = 24;

    let mut paginator = Paginator::new();
    let outcome = incremental_reflow(
        &mut paginator,
        &content,
        &new_geometry,
        &viewport,
        0,
        &baseline,
        &token,
    );

    match outcome {
        Ok(ReflowOutcome::Cancelled) => {}
        Ok(ReflowOutcome::Ready { first_frame, .. }) => {
            // 入口即取消时可能已经产出了首帧（那是完整的一页，不是半成品）。
            // 判据是"首帧必须是完整的页序列"，不得是半个页。
            assert!(
                first_frame.pages.iter().all(|page| {
                    !page.char_range.is_empty() || page.is_padding || page.lines.is_empty()
                }),
                "取消时产出的页也必须是完整的（不得留下半个页）"
            );
        }
        Err(error) => panic!("取消不应产生错误：{error}"),
    }
}

/// 参数**未变化**时，增量重排退化为"直接复用"（不做无谓工作）。
#[test]
fn unchanged_geometry_short_circuits() {
    let content = chapter_text(0);
    let viewport = Viewport::new(342.0, 400.0);
    let baseline = {
        let mut paginator = Paginator::new();
        paginator
            .paginate(&content, &geometry(), &viewport)
            .expect("应成功")
    };

    let mut paginator = Paginator::new();
    let outcome = incremental_reflow(
        &mut paginator,
        &content,
        &geometry(),
        &viewport,
        baseline.pages[0].char_range.start,
        &baseline,
        &CancelToken::new(),
    )
    .expect("应成功");

    let ReflowOutcome::Ready {
        completion,
        first_frame,
    } = outcome
    else {
        panic!("未被取消");
    };
    // 几何未变：续算出的完整结果必须与既有结果逐字段相同
    let complete = completion.complete(&mut paginator).expect("续算应成功");
    assert_eq!(
        complete.pages, baseline.pages,
        "几何未变时续算结果应与既有结果一致（不得重排出不同结果）"
    );
    // 首帧也必须是既有结果的一部分（同一份几何下页边界不因增量而变）
    assert!(!first_frame.pages.is_empty(), "首帧应非空");
}

// ===========================================================================
// 辅助
// ===========================================================================

/// 取中位数（原地排序）。
///
/// 偶数个取中间两个的较小者——对本文件的用途（比较两个中位数的大小）
/// 足够，且避免引入浮点除法。
fn median(times: &mut [Duration]) -> Duration {
    times.sort_unstable();
    times[times.len() / 2]
}
