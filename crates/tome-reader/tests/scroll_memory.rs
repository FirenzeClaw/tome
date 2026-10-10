//! T030 验收：按需释放 + 事件节流（FR-026 / FR-027、SC-010）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T030）
//!
//! > - 20 万字章滚动：内存 ≤ 章节字节 ×8（`scroll_memory.rs`，SC-010 实测）
//! > - 事件风暴（惯性滚动模拟）后无排队任务断言（同文件）
//!
//! ## SC-010 的原文口径（必须逐字遵守）
//!
//! > **SC-010**: 内存：排版结果常驻内存与章节字符数的比值受控——单章排版结果
//! > （行盒 + 位置映射）的内存占用 **≤ 章节字节数 × 8**（口径：**进程内实测的
//! > 堆分配差值**，不含字形缓存与纹理）。超长章节（≥ 20 万字）下不得因单章
//! > 过大而失败；滚动模式同样受此约束（FR-026 的按需释放必须生效）。
//!
//! 三点解读（都影响本文件的实现）：
//!
//! 1. **分母是"章节字节数"**（UTF-8 字节），不是字符数。20 万汉字 × 3 字节
//!    = 600,000 字节 → 预算 **4.8 MB**
//! 2. **"堆分配差值"**：必须真的测堆，不能用"结构体大小 × 个数"估算。故本
//!    文件装一个**计数分配器**（`#[global_allocator]`）实测
//! 3. **"不含字形缓存与纹理"**：那是渲染层的（S2-4），本层只测排版结果
//!
//! ## 测量装置为什么必须"能失败"
//!
//! BASELINE §10.1 记了四次装置缺陷（三次漏检 + 一次误报）。故本文件的测量
//! 带两个**有效性自检**：
//!
//! - 计数分配器自身有效：一次已知大小的分配必须被数到（否则它在数零）
//! - 被测对象真的常驻：`ScrollFlow` 在测量期间必须活着（否则测的是零）

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]
// ## 为什么本文件需要 unsafe（唯一的例外）
//
// SC-010 的口径是"**进程内实测的堆分配差值**"——那必须真的拦到分配器调用，
// 没有安全的 std 等价物（std 不提供"当前堆占用"的查询）。故本文件装一个
// `#[global_allocator]` 包装 `System`，只加计数、不改分配语义。
//
// 逐处 SAFETY 理由见各方法上的注释。这是**测量装置**的固有代价，不是实现
// 走了捷径（生产代码里 `tome-reader` 仍是 `#![forbid(unsafe_code)]`）。
#![expect(
    unsafe_code,
    reason = "SC-010 要求进程内实测堆分配差值，须装全局分配器包装（仅测试文件）"
)]
#![expect(
    clippy::undocumented_unsafe_blocks,
    reason = "每个 unsafe 块都带 SAFETY 注释，但 clippy 不认中文注释里的标记"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::ScrollFlow;
use tome_reader::scroll::throttle::{ScrollEventThrottle, ThrottleDecision};

// ===========================================================================
// 计数分配器（SC-010 的"进程内实测的堆分配差值"）
// ===========================================================================

/// 累计已分配字节（不减去释放——故它是"分配总量"，配合差值使用）。
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
/// 当前存活字节（分配 − 释放）。
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// 记录堆分配量的分配器包装。
///
/// ## 为什么用两个计数器
///
/// - `ALLOCATED`（累计分配）用于"某段代码一共申请了多少"
/// - `LIVE`（当前存活）用于"某段代码的产物占了多少"——**这是 SC-010 要的量**
///
/// 只用累计值会把测量期间的临时分配（如排序缓冲）也算进去，高估结果；
/// 只用存活值则在"分配→释放→再分配"时看不出发生过什么。两者都记，让
/// 断言可以选择合适的口径，并让诊断信息更完整。
struct CountingAllocator;

impl CountingAllocator {
    fn live_bytes() -> usize {
        LIVE.load(Ordering::Relaxed)
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        LIVE.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: 直接转发给系统分配器，layout 由调用方保证有效（GlobalAlloc
        // 的契约）。本包装只加计数，不改变分配语义。
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: pointer 与 layout 由配对的 alloc 传入（GlobalAlloc 契约）。
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if new_size >= layout.size() {
            let growth = new_size - layout.size();
            ALLOCATED.fetch_add(growth, Ordering::Relaxed);
            LIVE.fetch_add(growth, Ordering::Relaxed);
        } else {
            LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        // SAFETY: 同 alloc/dealloc，参数由配对的调用传入。
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// 分配敏感测试的**串行锁**。
///
/// ## 为什么必须有它
///
/// `LIVE` 是**进程级**计数：并行的其它测试线程一分配，它就会涨。而 cargo
/// 默认并行跑同一个文件里的测试——于是"测量某对象的堆占用"会被邻居污染，
/// 表现为随机的、难以复现的超标。
///
/// 故所有读 `LIVE` 的测试先取这把锁。这是测量纪律，不是实现约束。
static MEMORY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 取测量锁，**容忍中毒**。
///
/// ## 为什么不用 `.expect()`
///
/// 标准 `Mutex` 在持锁线程 panic 时"中毒"，后续 `lock()` 返回 `Err`。本锁
/// 保护的**不是数据**（没有不变量会被破坏）而是"同一时刻只有一个测试在读
/// 进程级计数器"。故一个测试失败不该让其余三个测量测试连锁失败——那会把
/// 一个真实的失败放大成四个，掩盖真正的信号。
fn lock_memory() -> std::sync::MutexGuard<'static, ()> {
    MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ===========================================================================
// 夹具
// ===========================================================================

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

/// 造一份约 `target_chars` 字的正文（含段落与标点，尽量真实）。
fn long_chapter(target_chars: usize) -> String {
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                     后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。";
    let mut text = String::new();
    let mut index = 0usize;
    while text.chars().count() < target_chars {
        text.push_str(&format!("第{index}段。{paragraph}"));
        text.push('\n');
        index += 1;
    }
    text
}

fn build_flow(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("构造滚动流应成功")
}

// ===========================================================================
// 测量装置自检（BASELINE §10.1 的纪律）
// ===========================================================================

/// **计数分配器自身有效**：一次已知大小的分配必须被数到。
///
/// ## 为什么这条必须先跑
///
/// 一个"恒返回 0"的计数器会让所有内存断言**平凡通过**——那正是 BASELINE
/// §10.1 记的三次"全绿但没测到"。故先证它能数到东西。
#[test]
fn counting_allocator_actually_counts() {
    let _guard = lock_memory();

    // ## 用**累计分配量**而不是存活量做自检
    //
    // 本文件的其它测试都依赖 `LIVE`（存活量），而它是进程级的：并行的邻居
    // 线程一**释放**内存，`LIVE` 就会掉——自检里那句"释放后应回落 1MB"会
    // 被邻居的释放量淹没（实测：分配 1MB 后存活只涨 1,097,541，但释放时
    // 同时有邻居在释放，回落被记成 1,097,541 → 80,082，看着"回落了 1MB"
    // 却是因为基准被邻居拉高了）。
    //
    // `ALLOCATED`（累计分配）只增不减，不受邻居释放干扰。故自检改用它：
    // 分配 1MB 必须让累计量至少涨 1MB。这足以证明"计数装置真的在数"——
    // 而那是本自检的唯一目的（BASELINE §10.1：装置失效会让所有断言假绿）。
    let before = ALLOCATED.load(Ordering::Relaxed);
    let payload: Vec<u8> = vec![7u8; 1024 * 1024];
    let after = ALLOCATED.load(Ordering::Relaxed);

    assert!(
        after.saturating_sub(before) >= 1024 * 1024,
        "计数分配器失效：分配 1MB 后累计分配量只涨了 {}          （装置在数零，所有内存断言都会假绿）",
        after.saturating_sub(before)
    );

    // 保留对象直至测量完成（防优化器消除分配）
    assert_eq!(payload.len(), 1024 * 1024);
}

// ===========================================================================
// SC-010：20 万字章的内存
// ===========================================================================

/// **20 万字章：排版结果内存 ≤ 章节字节 × 8**（SC-010 实测）。
///
/// ## 口径（与 SC-010 原文逐条对应）
///
/// | 项 | 取值 |
/// |---|---|
/// | 章节规模 | ≥ 200,000 字 |
/// | 分母 | 章节的 **UTF-8 字节数** |
/// | 预算 | 字节数 × 8 |
/// | 测量对象 | `ScrollFlow`（含行盒 + 位置映射）的**存活**堆字节 |
/// | 不含 | 字形缓存与纹理（渲染层，S2-4） |
///
/// ## 为什么先"释放视口外的行"再测
///
/// FR-026 明文："视口外的行 MUST 可释放；整章布局结果 MUST NOT 要求全部
/// 常驻内存。" 故本节测的是**释放后**的常驻量——那才是滚动模式的实际驻留。
///
/// 本测试同时测"未释放"的量作为对照（见 `unreleased_flow_exceeds_budget`
/// 那条），以证明释放确实必要且有效。
#[test]
fn twenty_myriad_char_chapter_fits_budget_after_release() {
    let _guard = lock_memory();

    let content = long_chapter(200_000);
    let char_count = content.chars().count();
    let byte_count = content.len();
    assert!(
        char_count >= 200_000,
        "前置：章节应 ≥ 20 万字（实测 {char_count}）"
    );

    let geometry = geometry();
    let viewport = Viewport::new(342.0, 800.0);

    // 共用同一个排版引擎（避免每次测量都重建字体库污染差值）
    let mut paginator = Paginator::new();

    // --- 基线：先把"引擎自身的常驻"灌满并测一次 ---
    //
    // `Paginator` 持有 `FontSystem`（加载系统字体库，可达数十 MB）。它不是
    // "单章排版结果"，故必须先建好、再测增量——否则测到的是字体库而不是
    // 排版结果（SC-010 明文"不含字形缓存与纹理"）。
    {
        let warmup =
            layout_chapter(&mut paginator, "预热", &geometry, &viewport).expect("预热应成功");
        let _ = warmup;
    }

    // --- 测量：只测 `ScrollFlow` 构造带来的**存活增量** ---
    let baseline_live = CountingAllocator::live_bytes();

    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");
    let mut flow = ScrollFlow::new(chapter, geometry, viewport).expect("应成功");

    // 释放视口外的行（FR-026）——测的是"释放后"的常驻量
    let viewport_index = flow.viewports().len() / 2;
    let released = flow.release_outside(viewport_index, 2);
    assert!(
        released > 0,
        "应真的释放了若干行（实测 {released}）——否则本节测的不是释放后的量"
    );

    let after_live = CountingAllocator::live_bytes();
    // 用 `Box` 钉住 flow 的存活期（防编译器提前释放导致测到零）
    let flow_box = Box::new(flow);

    let resident = after_live.saturating_sub(baseline_live);
    let budget = byte_count * 8;

    assert!(
        resident <= budget,
        "20 万字章（{byte_count} 字节）的排版结果常驻 {resident} 字节超过预算 {budget}          字节（章节字节 ×8）。共 {} 屏，已释放 {released} 行，仍持有字形的行 {}",
        flow_box.viewports().len(),
        flow_box.resident_glyph_lines()
    );

    assert!(!flow_box.viewports().is_empty());
}

#[test]
fn unreleased_flow_exceeds_budget_proving_release_is_necessary() {
    let _guard = lock_memory();

    let content = long_chapter(200_000);
    let byte_count = content.len();

    let geometry = geometry();
    let viewport = Viewport::new(342.0, 800.0);

    let mut paginator = Paginator::new();
    {
        let warmup =
            layout_chapter(&mut paginator, "预热", &geometry, &viewport).expect("预热应成功");
        let _ = warmup;
    }

    let baseline_live = CountingAllocator::live_bytes();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");
    let flow = ScrollFlow::new(chapter, geometry, viewport).expect("应成功");

    // 未释放：全部行都持有字形
    let glyph_lines = flow.resident_glyph_lines();
    let total_lines = flow.lines().len();
    assert_eq!(
        glyph_lines, total_lines,
        "前置：未释放时应所有行都持有字形（实测 {glyph_lines}/{total_lines}）"
    );

    let after_live = CountingAllocator::live_bytes();
    // 用 `Box` 钉住 flow 的存活期至本断言之后（防编译器提前释放）
    let _flow_box = Box::new(flow);

    let resident = after_live.saturating_sub(baseline_live);
    let budget = byte_count * 8;

    assert!(
        resident > budget,
        "前置自检失败：未释放的 20 万字章常驻 {resident} 字节 ≤ 预算 {budget} 字节，\
         说明本数据下 FR-026 的按需释放无法体现（主判据是空的）。\
         请增大章节规模或调整几何参数。"
    );
}

/// **释放只影响字形，不影响位置口径**（FR-021/SC-006b 不得被破坏）。
#[test]
fn release_preserves_position_semantics() {
    let content = long_chapter(30_000);
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);

    let mut flow = build_flow(&content, &geometry, &viewport);
    let before: Vec<(usize, usize, usize)> = flow
        .viewports()
        .iter()
        .map(|viewport_entry| {
            (
                viewport_entry.char_range.start,
                viewport_entry.char_range.end,
                flow.viewport_lines(viewport_entry).len(),
            )
        })
        .collect();
    let char_len_before = flow.char_len();

    let released = flow.release_outside(0, 1);
    assert!(released > 0, "应释放若干行");

    let after: Vec<(usize, usize, usize)> = flow
        .viewports()
        .iter()
        .map(|viewport_entry| {
            (
                viewport_entry.char_range.start,
                viewport_entry.char_range.end,
                flow.viewport_lines(viewport_entry).len(),
            )
        })
        .collect();

    assert_eq!(
        before, after,
        "释放不得改变视口区间与可见行数（位置口径与 SC-006b 必须不受影响）"
    );
    assert_eq!(flow.char_len(), char_len_before);

    // SC-006b 仍成立：区间并集仍等于正文
    let union: String = flow
        .viewports()
        .iter()
        .map(|viewport_entry| {
            content
                .chars()
                .skip(viewport_entry.char_range.start)
                .take(viewport_entry.char_range.end - viewport_entry.char_range.start)
                .collect::<String>()
        })
        .collect();
    assert_eq!(union, content, "释放后 SC-006b 仍须成立");
}

/// **释放窗口外的行确实丢了字形**（可观测的释放证据）。
#[test]
fn released_lines_no_longer_hold_glyphs() {
    let content = long_chapter(30_000);
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let mut flow = build_flow(&content, &geometry, &viewport);

    let total = flow.lines().len();
    assert!(total > 10, "前置：应有多行，实测 {total}");

    let released = flow.release_outside(0, 0);
    assert!(released > 0);
    assert!(
        flow.resident_glyph_lines() < total,
        "释放后持有字形的行数应减少（实测 {} / {total}）",
        flow.resident_glyph_lines()
    );
}

/// 释放**不丢位置信息**：行区间与行高仍在（那是位置换算的依据）。
#[test]
fn release_keeps_char_ranges_and_heights() {
    let content = long_chapter(20_000);
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let mut flow = build_flow(&content, &geometry, &viewport);

    let before: Vec<(usize, usize, f32)> = flow
        .lines()
        .iter()
        .map(|line| {
            (
                line.chapter_char_range.start,
                line.chapter_char_range.end,
                line.line.height_px,
            )
        })
        .collect();
    let height_before = flow.content_height_px();

    flow.release_outside(0, 0);

    let after: Vec<(usize, usize, f32)> = flow
        .lines()
        .iter()
        .map(|line| {
            (
                line.chapter_char_range.start,
                line.chapter_char_range.end,
                line.line.height_px,
            )
        })
        .collect();

    assert_eq!(
        before, after,
        "释放不得改变行的字符区间与行高（位置换算的依据）"
    );
    assert_eq!(
        flow.content_height_px(),
        height_before,
        "释放不得改变流总高"
    );
}

/// 释放越界的窗口不 panic（钳制）。
#[test]
fn release_with_out_of_range_window_is_safe() {
    let content = long_chapter(5_000);
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let mut flow = build_flow(&content, &geometry, &viewport);

    for (index, radius) in [(9999usize, 0usize), (0, 9999), (9999, 9999)] {
        let _ = flow.release_outside(index, radius);
    }
    assert!(!flow.viewports().is_empty());
}

/// 空章的释放安全。
#[test]
fn release_on_empty_chapter_is_safe() {
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let mut flow = build_flow("", &geometry, &viewport);
    assert_eq!(flow.release_outside(0, 5), 0);
    assert!(!flow.viewports().is_empty());
}

// ===========================================================================
// FR-027：事件节流不累积排队
// ===========================================================================

/// **事件风暴后无排队任务**（FR-027 的"不累积排队"）。
#[test]
fn event_storm_does_not_accumulate_pending_work() {
    let mut throttle = ScrollEventThrottle::new();

    // 惯性滚动：1 秒内 240 个事件（远超常见帧率）
    for step in 0..240 {
        throttle.on_scroll_event(step as f32 * 7.0);
        assert!(
            throttle.pending_count() <= 1,
            "待处理任务数不得超过 1（实测 {}，step {step}）",
            throttle.pending_count()
        );
    }

    // 风暴结束后应恰好有一个待处理（最后一次的位置）
    let decision = throttle.take().expect("风暴后应有决策");
    assert!(
        matches!(decision, ThrottleDecision::Relayout { .. }),
        "风暴后应产出一次重排（而不是零次——那会让界面停在旧位置）"
    );
    assert_eq!(throttle.pending_count(), 0, "取走后不得残留");
}

/// **合并后回的是最后一次位置**（不是第一次，也不是中间某个）。
#[test]
fn throttle_keeps_the_latest_scroll_position() {
    let mut throttle = ScrollEventThrottle::new();
    throttle.on_scroll_event(100.0);
    throttle.on_scroll_event(500.0);
    throttle.on_scroll_event(900.0);

    let decision = throttle.take().expect("应有待处理");
    assert_eq!(
        decision.offset_px(),
        Some(900.0),
        "合并后应保留最后一次位置（拖拽/惯性的终态）"
    );
}

/// **微小位移被忽略**（省掉无意义的视口重算）。
#[test]
fn negligible_scroll_delta_is_skipped() {
    let mut throttle = ScrollEventThrottle::new();
    // 首次事件建立基准
    throttle.on_scroll_event(1000.0);
    let first = throttle.take().expect("首次应有决策");
    assert!(matches!(first, ThrottleDecision::Relayout { .. }));

    // 亚像素抖动：不应触发重排
    throttle.on_scroll_event(1000.2);
    let second = throttle.take().expect("应有决策（Skip 或 Relayout）");
    assert!(
        matches!(second, ThrottleDecision::Skip { .. }),
        "亚像素抖动应被判为无需重排，实测 {second:?}"
    );
}

/// **大到跨屏的位移必须重排**（防"节流把真实滚动吃掉了"）。
#[test]
fn large_scroll_delta_triggers_relayout() {
    let mut throttle = ScrollEventThrottle::new();
    throttle.on_scroll_event(0.0);
    let _ = throttle.take();

    throttle.on_scroll_event(500.0);
    let decision = throttle.take().expect("风暴后应有决策");
    assert!(
        matches!(decision, ThrottleDecision::Relayout { .. }),
        "500px 的位移必须重排，实测 {decision:?}"
    );
}

/// 空队列时 `take` 返回 `None`（不臆造重排）。
#[test]
fn throttle_take_on_empty_is_none() {
    let mut throttle = ScrollEventThrottle::new();
    assert_eq!(throttle.pending_count(), 0);
    assert!(throttle.take().is_none());
}

/// **节流不改变排版结果**（FR-027 的后半句语义：节流只影响"何时算"）。
///
/// ## 这条防的是"节流顺手改了位置"
///
/// 一个把 `offset_px` 取整/吸附到屏边界的节流实现会让滚动位置**轻微漂移**
/// ——用户每次滚动都偏移几像素，累积起来就是"滚着滚着对不上了"。故断言
/// 节流产出的偏移与最后一次事件的偏移**逐位相同**。
#[test]
fn throttle_preserves_exact_offset() {
    let mut throttle = ScrollEventThrottle::new();
    let positions = [0.0f32, 33.7, 120.25, 987.125, 1234.5];
    for position in positions {
        throttle.on_scroll_event(position);
    }
    let decision = throttle.take().expect("应有待处理");
    assert_eq!(
        decision.offset_px(),
        Some(1234.5),
        "节流必须原样传递偏移（不得取整或吸附）"
    );
}
