//! T031 验收：阅读记录两量与跨模式去重（FR-039 / SC-013b）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T031）
//!
//! > SC-013b：同段字符两模式阅读字数相同；模式间来回切换只计一次
//! > （同文件）快速翻页不产生高频写库（节流断言）
//!
//! ## SC-013b 原文
//!
//! > **阅读记录跨模式一致**：同一段字符在翻页与滚动两种形态下阅读，去重累计
//! > 的阅读字数**相同**（100%）；在两种模式间来回切换同一段文字，其字符只被
//! > 计入一次。
//!
//! ## 口径（裁决 OQ#15→A，FR-039 的括号条款）
//!
//! > 阅读字数口径 MUST 为**已展示字符区间去重累计**——翻页模式按翻过的页区间、
//! > 滚动模式按进入视口的区间计入，**回翻/回滚 MUST NOT 重复计入**。去重口径
//! > MUST 跨阅读形态统一（模式切换不得使同一字符被重复计入）。
//!
//! 三个要点都在本文件里有对应断言：
//!
//! | 要点 | 断言 |
//! |---|---|
//! | "已展示字符区间去重累计" | 同一段字符反复进入视口只计一次 |
//! | "回翻/回滚不重复计入" | 翻回上一页 / 滚回上一屏不增计数 |
//! | "跨形态统一" | 同一段字符两模式的字数**相同** |
//!
//! ## 为什么"去重"必须是**字符区间**级而不是"页/屏"级
//!
//! 页边界与屏边界**不重合**（页按可用高切、屏也按可用高切，但单双页模式下
//! 版心不同 → 断行不同 → 边界不同）。故"第 3 页"与"第 3 屏"覆盖的字符集合
//! 通常不同。若按"页/屏"去重，切一次模式就会把整段算两次。
//!
//! 本模块据此把**字符区间集合**作为去重的唯一依据——与 FR-021 的位置口径
//! 一致（那也正是"跨形态统一"能成立的原因）。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig, ReadingMode};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::record::{ReadingRecordAccumulator, RecordSink};
use tome_reader::scroll::flow::ScrollFlow;

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

fn build_flow(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("构造滚动流应成功")
}

/// 记录每次写库的存根。
#[derive(Debug, Default)]
struct RecordingSink {
    writes: Vec<(i64, i64)>,
}

impl RecordSink for RecordingSink {
    fn add_read_record(&mut self, read_seconds: i64, read_chars: i64) {
        self.writes.push((read_seconds, read_chars));
    }
}

// ===========================================================================
// SC-013b 主判据（1）：两模式读同一段，字数相同
// ===========================================================================

/// **同一段字符在翻页与滚动两形态下阅读，去重字数相同**（SC-013b 前半句）。
///
/// ## 判据的构造
///
/// 两形态各自的"已展示区间"不同（页边界 ≠ 屏边界），但**目标字符段相同**。
/// 故：把同一段字符分别经两条路径送入累加器，最终计数必须相等。
#[test]
fn same_span_yields_same_count_in_both_modes() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    // 目标：正文中段连续 800 个字符（足够跨多页与多屏）
    let target_start = char_len / 3;
    let target_end = (target_start + 800).min(char_len);

    // --- 翻页路径：把覆盖目标段的页逐一计入 ---
    let mut paged_accumulator = ReadingRecordAccumulator::new();
    for page in &paginated.pages {
        if page.is_padding {
            continue;
        }
        // 只看与目标段相交的页（模拟"读者只读了那一段"）
        if page.char_range.end <= target_start || page.char_range.start >= target_end {
            continue;
        }
        paged_accumulator.observe_range(
            page.char_range.start.max(target_start)..page.char_range.end.min(target_end),
        );
    }

    // --- 滚动路径：把覆盖目标段的屏逐一计入 ---
    let mut scroll_accumulator = ReadingRecordAccumulator::new();
    for viewer in flow.viewports() {
        if viewer.char_range.end <= target_start || viewer.char_range.start >= target_end {
            continue;
        }
        scroll_accumulator.observe_range(
            viewer.char_range.start.max(target_start)..viewer.char_range.end.min(target_end),
        );
    }

    assert_eq!(
        paged_accumulator.read_chars(),
        scroll_accumulator.read_chars(),
        "同一段字符（{target_start}..{target_end}）在两形态下的去重字数必须相同\
         （翻页 {}，滚动 {}）",
        paged_accumulator.read_chars(),
        scroll_accumulator.read_chars()
    );
    assert_eq!(
        paged_accumulator.read_chars() as usize,
        target_end - target_start,
        "该段恰好被完整覆盖，故字数应等于段长"
    );
}

/// **模式间来回切换同一段文字，其字符只被计入一次**（SC-013b 后半句）。
#[test]
fn switching_modes_does_not_double_count() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    // 目标段必须**落在正文之内**（否则覆盖不到，字数会小于段长）
    let target_start = char_len / 4;
    let target_end = (target_start + 600).min(char_len);

    // 一个**共用**的累加器（模拟同一本书的阅读记录）
    let mut accumulator = ReadingRecordAccumulator::new();

    // 来回切换 4 轮：paged → scroll → paged → scroll
    for round in 0..4 {
        let use_scroll = round % 2 == 1;
        if use_scroll {
            for viewer in flow.viewports() {
                if viewer.char_range.end <= target_start || viewer.char_range.start >= target_end {
                    continue;
                }
                accumulator.observe_range(
                    viewer.char_range.start.max(target_start)
                        ..viewer.char_range.end.min(target_end),
                );
            }
        } else {
            for page in &paginated.pages {
                if page.is_padding {
                    continue;
                }
                if page.char_range.end <= target_start || page.char_range.start >= target_end {
                    continue;
                }
                accumulator.observe_range(
                    page.char_range.start.max(target_start)..page.char_range.end.min(target_end),
                );
            }
        }
    }

    assert_eq!(
        accumulator.read_chars() as usize,
        target_end - target_start,
        "4 轮模式切换后字数应仍是段长（{target_start}..{target_end}），实测 {}——\
         说明同一字符被重复计入了",
        accumulator.read_chars()
    );
}

// ===========================================================================
// 去重口径（裁决 OQ#15→A）
// ===========================================================================

/// **回翻/回滚不重复计入**（FR-039 的括号条款）。
#[test]
fn scrolling_back_does_not_double_count() {
    let mut accumulator = ReadingRecordAccumulator::new();

    // 向前读
    accumulator.observe_range(0..100);
    accumulator.observe_range(100..200);
    assert_eq!(accumulator.read_chars(), 200);

    // 滚回上一屏（全重叠）
    accumulator.observe_range(100..200);
    accumulator.observe_range(0..100);
    assert_eq!(
        accumulator.read_chars(),
        200,
        "回滚不得重复计入（实测 {}）",
        accumulator.read_chars()
    );
}

/// **部分重叠只计新增部分**（不是"整段重计"，也不是"一点不计"）。
#[test]
fn partial_overlap_counts_only_new_characters() {
    let mut accumulator = ReadingRecordAccumulator::new();

    accumulator.observe_range(0..100);
    assert_eq!(accumulator.read_chars(), 100);

    // 重叠 50（50..150 → 新增 100..150）
    accumulator.observe_range(50..150);
    assert_eq!(
        accumulator.read_chars(),
        150,
        "部分重叠应只计新增的 50 个字符（实测 {}）",
        accumulator.read_chars()
    );

    // 再重叠
    accumulator.observe_range(120..200);
    assert_eq!(
        accumulator.read_chars(),
        200,
        "继续向右滚应累计到 200（实测 {}）",
        accumulator.read_chars()
    );
}

/// **乱序区间**（跳读）也正确去重——区间不必按顺序到达。
#[test]
fn out_of_order_ranges_are_deduped_correctly() {
    let mut accumulator = ReadingRecordAccumulator::new();

    accumulator.observe_range(500..600);
    accumulator.observe_range(0..100);
    accumulator.observe_range(300..400);
    assert_eq!(accumulator.read_chars(), 300);

    // 补上中间的缺口
    accumulator.observe_range(100..300);
    accumulator.observe_range(400..500);
    assert_eq!(
        accumulator.read_chars(),
        600,
        "补齐缺口后应恰好是并集大小 0..600"
    );
}

/// 区间被**包含**时不增计数（边界情形）。
#[test]
fn contained_range_adds_nothing() {
    let mut accumulator = ReadingRecordAccumulator::new();
    accumulator.observe_range(100..300);
    assert_eq!(accumulator.read_chars(), 200);

    accumulator.observe_range(150..250);
    assert_eq!(accumulator.read_chars(), 200, "被包含的区间不增计数");

    // 空区间也不增
    accumulator.observe_range(200..200);
    assert_eq!(accumulator.read_chars(), 200);
}

/// 相邻区间**无缝拼接**（100..200 后接 200..300 计 300 而非 199+101）。
#[test]
fn adjacent_ranges_are_seamless() {
    let mut accumulator = ReadingRecordAccumulator::new();
    accumulator.observe_range(0..50);
    accumulator.observe_range(50..100);
    accumulator.observe_range(100..150);
    assert_eq!(accumulator.read_chars(), 150);
}

/// **反向区间**（start > end）被安全处置（不 panic、不产生负数）。
#[test]
fn reversed_range_is_safe() {
    let mut accumulator = ReadingRecordAccumulator::new();
    // 用变量构造反向区间（字面量 `200..100` 会被 clippy 判为"空区间"并报错）
    let (start, end) = (200usize, 100usize);
    accumulator.observe_range(start..end);
    assert_eq!(accumulator.read_chars(), 0, "反向区间应被视为空");
}

/// 计数**不依赖区间的到达顺序与切分方式**（同一并集的多种切法结果相同）。
///
/// ## 这条把"去重"与"怎么切"解耦
///
/// 两模式的切分不同（页 vs 屏），若去重结果依赖切法，SC-013b 就不成立。
/// 本测试用三种切法切同一个并集 `0..300`，断言结果都是 300。
#[test]
fn count_depends_only_on_the_union() {
    let unions: Vec<(usize, Vec<std::ops::Range<usize>>)> = vec![
        // 一次整段 → 并集 0..300 = 300
        (300, std::iter::once(0..300).collect()),
        // 均分三段 → 并集 0..300 = 300
        (300, vec![0..100, 100..200, 200..300].into_iter().collect()),
        // 不均匀 + 乱序 + 重叠：并集是 0..160 ∪ 200..320 = 160 + 120 = **280**
        // （注意不是 0..300：200..300 与 0..150 之间有 160..200 的缺口）
        (
            280,
            vec![200..300, 0..150, 120..160, 250..320]
                .into_iter()
                .collect(),
        ),
    ];

    for (index, (expected, ranges)) in unions.iter().enumerate() {
        let mut accumulator = ReadingRecordAccumulator::new();
        for range in ranges {
            accumulator.observe_range(range.clone());
        }
        assert_eq!(
            accumulator.read_chars() as usize,
            *expected,
            "第 {index} 种切法的结果应为并集大小 {expected}（实测 {}）——去重不得依赖切法",
            accumulator.read_chars()
        );
    }
}

// ===========================================================================
// 节流（FR-039 的"MUST 节流"）
// ===========================================================================

/// **快速翻页不产生高频写库**（tasks.md 的节流断言）。
#[test]
fn rapid_paging_does_not_flood_the_sink() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("应成功");
    assert!(paginated.pages.len() > 3, "前置：应有多页");

    let mut accumulator = ReadingRecordAccumulator::new();
    let mut sink = RecordingSink::default();

    // 快速翻过所有页（每页都"读过"），但只在最后提交一次
    for page in &paginated.pages {
        accumulator.observe_range(page.char_range.clone());
        // 注意：**不**逐页提交——那正是"高频写库"的反模式
    }
    accumulator.commit(&mut sink);

    assert_eq!(
        sink.writes.len(),
        1,
        "一批观察应只写一次库（实测 {} 次）",
        sink.writes.len()
    );
    assert_eq!(
        sink.writes[0].1,
        content.chars().count() as i64,
        "写入的字数应是全章（所有页的并集）"
    );
}

/// **无新增时提交不写库**（避免"翻回上一页也写一次"）。
#[test]
fn commit_without_new_progress_does_not_write() {
    let mut accumulator = ReadingRecordAccumulator::new();
    let mut sink = RecordingSink::default();

    accumulator.observe_range(0..100);
    accumulator.commit(&mut sink);
    assert_eq!(sink.writes.len(), 1, "首次提交应写");

    // 回翻（无新增）再提交
    accumulator.observe_range(0..100);
    accumulator.commit(&mut sink);
    assert_eq!(
        sink.writes.len(),
        1,
        "无新增的提交不得写库（实测 {} 次）",
        sink.writes.len()
    );

    accumulator.commit(&mut sink);
    assert_eq!(sink.writes.len(), 1, "连续空提交也不得写");
}

/// 提交写入的是**增量**（不是累计总量）。
///
/// ## 为什么要增量而不是总量
///
/// 001 的 `add_read_record` 语义是"累加一条记录"（FR-026b）。若每次写的是
/// 累计总量，数据库里的数会变成 `total + total' + ...`——一个快速跑偏的量。
#[test]
fn commit_writes_the_increment() {
    let mut accumulator = ReadingRecordAccumulator::new();
    let mut sink = RecordingSink::default();

    accumulator.observe_range(0..100);
    accumulator.commit(&mut sink);

    accumulator.observe_range(100..250);
    accumulator.commit(&mut sink);

    assert_eq!(sink.writes.len(), 2);
    assert_eq!(sink.writes[0].1, 100, "第一次写 100");
    assert_eq!(
        sink.writes[1].1, 150,
        "第二次应写增量 150（不是累计的 250），实测 {}",
        sink.writes[1].1
    );
    assert_eq!(accumulator.read_chars(), 250);
}

/// 阅读秒数按**累计**写入（它是另一个量，不参与去重）。
#[test]
fn seconds_are_reported_as_accumulated() {
    let mut accumulator = ReadingRecordAccumulator::new();
    let mut sink = RecordingSink::default();

    accumulator.add_seconds(30);
    accumulator.observe_range(0..100);
    accumulator.commit(&mut sink);
    assert_eq!(sink.writes[0], (30, 100));

    accumulator.add_seconds(45);
    accumulator.observe_range(100..200);
    accumulator.commit(&mut sink);
    assert_eq!(
        sink.writes[1].0, 45,
        "秒数应写**本批**的增量（001 接口是累加语义）"
    );
}

/// 秒数有而字符无新增时仍需写（用户"在看书但没翻页"）。
#[test]
fn time_only_progress_still_writes() {
    let mut accumulator = ReadingRecordAccumulator::new();
    let mut sink = RecordingSink::default();

    accumulator.observe_range(0..100);
    accumulator.commit(&mut sink);
    assert_eq!(sink.writes.len(), 1);

    // 停留 60 秒，位置没变
    accumulator.add_seconds(60);
    accumulator.commit(&mut sink);

    assert_eq!(
        sink.writes.len(),
        2,
        "只有时间推进时也应上报（否则'读书时长'会丢失）"
    );
    assert_eq!(sink.writes[1], (60, 0), "本次字符增量为 0");
}

// ===========================================================================
// 与 001 接缝的形态
// ===========================================================================

/// 累加器把结果交给 [`RecordSink`]，形态与 001 §FR-026b 的两个量一致。
#[test]
fn sink_receives_the_two_quantities() {
    let mut accumulator = ReadingRecordAccumulator::new();
    let mut sink = RecordingSink::default();

    accumulator.add_seconds(120);
    accumulator.observe_range(0..500);
    accumulator.commit(&mut sink);

    assert_eq!(sink.writes.len(), 1);
    assert_eq!(sink.writes[0].0, 120, "第一个量：阅读秒数");
    assert_eq!(sink.writes[0].1, 500, "第二个量：阅读字符数");
}

/// **无界面可用**（FR-039 的"在无界面环境下产生两个量"）。
///
/// 本模块不依赖任何界面类型——它就是几个纯计数器。这条"断言"的形式是
/// **它能在测试（无界面）里被完整使用**，而这正是本文件的每个测试都在做的。
#[test]
fn accumulator_works_without_any_ui() {
    let mut accumulator = ReadingRecordAccumulator::new();
    let mut sink = RecordingSink::default();

    accumulator.add_seconds(10);
    accumulator.observe_range(0..42);

    // 提交前：待报的两个量都在
    assert_eq!(accumulator.read_seconds(), 10, "提交前待报秒数是 10");
    assert_eq!(accumulator.pending_chars(), 42);

    accumulator.commit(&mut sink);

    // 提交后：累计字符数保留；待报量清零（`read_seconds` 的语义是**待报送**
    // 的秒数——已提交的进了库，本对象不该重复持有，否则再提交一次会算两遍）
    assert_eq!(accumulator.read_chars(), 42, "累计字符数不受提交影响");
    assert_eq!(accumulator.read_seconds(), 0, "提交后待报秒数清零");
    assert_eq!(sink.writes.len(), 1);
    assert_eq!(sink.writes[0], (10, 42), "写入的是提交前的两个量");
}

/// 空累加器提交不写库（无内容可报）。
#[test]
fn empty_accumulator_does_not_write() {
    let mut accumulator = ReadingRecordAccumulator::new();
    let mut sink = RecordingSink::default();
    accumulator.commit(&mut sink);
    assert!(sink.writes.is_empty());
}

/// 跨越模式切换的**大区间**也正确去重（混合边界）。
#[test]
fn interleaved_mode_ranges_dedup_across_boundaries() {
    let mut accumulator = ReadingRecordAccumulator::new();

    // 翻页读了两页
    accumulator.observe_range(0..200);
    accumulator.observe_range(200..400);
    // 切滚动，滚到与第二页部分重叠处
    accumulator.observe_range(350..600);
    // 切回翻页，翻回第一页（完全重叠）
    accumulator.observe_range(0..200);
    // 再切滚动，继续向右
    accumulator.observe_range(600..800);

    assert_eq!(
        accumulator.read_chars(),
        800,
        "混合边界的并集应为 0..800（实测 {}）",
        accumulator.read_chars()
    );
}

/// `ReadingMode` 只影响**如何切**，不影响计数——两模式的累加器类型相同。
#[test]
fn both_modes_use_the_same_accumulator_type() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("应成功");
    let flow = build_flow(&content, &geometry, &viewport);

    // 同一个类型被两形态共用（FR-039 的"口径跨形态统一"在类型上成立）
    let mut paged: ReadingRecordAccumulator = ReadingRecordAccumulator::new();
    let mut scrolled: ReadingRecordAccumulator = ReadingRecordAccumulator::new();

    for page in &paginated.pages {
        paged.observe_range(page.char_range.clone());
    }
    for viewer in flow.viewports() {
        scrolled.observe_range(viewer.char_range.clone());
    }

    assert_eq!(
        paged.read_chars(),
        scrolled.read_chars(),
        "两模式读完整章后的字数应相同（都是全章字符数）"
    );
    assert_eq!(
        paged.read_chars() as usize,
        content.chars().count(),
        "应等于全章字符数"
    );

    // `ReadingMode` 的枚举值本身不参与计数逻辑（只是标签）
    assert_ne!(ReadingMode::Paged, ReadingMode::Scroll);
}
