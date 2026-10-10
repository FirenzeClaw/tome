//! [DONE] 阅读记录两量产出与去重（T031 / FR-039、SC-013b）
//!
//! ## FR-039 原文
//!
//! > 系统 MUST 支持在无界面环境下产生阅读记录所需的两个量（阅读秒数、阅读
//! > 字符数）并交给 S1 既有接口落库；上报 MUST 节流且 MUST NOT 依赖任何界面
//! > （对齐 001 §FR-026b）。
//!
//! ## 阅读字数口径（裁决 OQ#15→A）
//!
//! > MUST 为**已展示字符区间去重累计**——翻页模式按翻过的页区间、滚动模式按
//! > 进入视口的区间计入，**回翻/回滚 MUST NOT 重复计入**。去重口径 MUST 跨
//! > 阅读形态统一（模式切换不得使同一字符被重复计入）。
//!
//! ## 本模块的核心：**字符区间集合**是去重的唯一依据
//!
//! ```text
//!   翻页：page.char_range × N ──┐
//!                                ├─→ 区间并集 ─→ 字数
//!   滚动：viewport.char_range × N ┘
//! ```
//!
//! 为什么不能按"页/屏"去重：**页边界与屏边界不重合**。页按可用高切、屏也按
//! 可用高切，但单双页模式下版心不同 → 断行不同 → 两个边界不同。故"第 3 页"
//! 与"第 3 屏"覆盖的字符集合通常不同——按页/屏去重会让模式切换时整段重计。
//!
//! 按**字符区间**去重则天然跨形态成立：它只关心"哪些字符被展示过"，不关心
//! 展示它们的是页还是屏。这正是 FR-021（位置口径唯一）的自然延伸。
//!
//! ## 实现：有序不相交区间集
//!
//! 维护一个按起点排序、互不相交、不相邻的区间列表。每次 `observe_range`：
//!
//! 1. 二分定位插入点
//! 2. 与相邻区间**合并**（重叠或相接）
//! 3. 累计字数按 `合并前长度 − 合并后长度` 的**减量**调整
//!
//! 用减量而非重算全和，是为了让"每次观察"的代价与**受影响区间数**成正比
//! （而非与总区间数成正比）——快速翻页时前者是常数量级。
//!
//! ## 节流（FR-039 的"MUST 节流"）
//!
//! 观察（`observe_range`）只改内存计数；落库只发生在 [`ReadingRecordAccumulator::commit`]。
//! 故"快速翻 20 页"只产生**一次**写库。调用方（S3）按自己的节奏 commit
//! （如每 30 秒或每次关闭阅读页）——那个节奏属 UI 生命周期，不属本层。

/// 阅读记录落库的接收方（由组合根适配到 001 的 `BookRepository::add_read_record`）。
///
/// ## 为什么是窄 trait
///
/// 与 [`crate::scroll::switch::ProgressSink`] 同一理由：本层只需要
/// "追加一条记录"这一个能力，而 `BookRepository` 有四十来个方法。依赖整个
/// trait 会把真实依赖淹没，也让测试必须实现全部方法。
pub trait RecordSink {
    /// 追加一条阅读记录（两个量，**累加语义**，对齐 001 §FR-026b）。
    fn add_read_record(&mut self, read_seconds: i64, read_chars: i64);
}

/// 阅读记录的累加器（FR-039 的两个量）。
///
/// ## 为什么把两个量放一起
///
/// 它们在同一次 `commit` 里一起写库（001 的 `ReadRecord` 就是两个字段）。
/// 分成两个对象会让调用方必须记住"两个都要提交"——一处漏了就丢一个量。
#[derive(Debug, Clone, Default)]
pub struct ReadingRecordAccumulator {
    /// 已展示字符区间的**有序不相交并集**（按起点升序，互不相邻）
    spans: Vec<std::ops::Range<usize>>,
    /// 并集的总字符数（维护为增量，避免每次重算）
    read_chars: i64,
    /// 自上次 commit 以来的阅读秒数
    pending_seconds: i64,
    /// 自上次 commit 以来**新增**的字符数
    pending_chars: i64,
}

impl ReadingRecordAccumulator {
    /// 新建（无任何已读区间）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 记录一段**被展示过**的字符区间（FR-039 的"已展示区间"）。
    ///
    /// ## 语义
    ///
    /// - 该区间与已有的并集**取并**；重叠与相接部分不重复计入
    /// - 反向区间（`start > end`）视为空，不 panic
    /// - 空区间不产生任何效果
    ///
    /// ## 调用时机
    ///
    /// - 翻页模式：每呈现一页时传该页的 `char_range`
    /// - 滚动模式：每进入一个新视口时传该视口的 `char_range`
    ///
    /// 两者传的都是**章内字符区间**（FR-021 的口径），故去重天然跨形态统一。
    pub fn observe_range(&mut self, range: std::ops::Range<usize>) {
        if range.start >= range.end {
            return; // 空区间或反向区间
        }

        // 二分定位：第一个"**没能完全落在本区间左边**"的区间。
        //
        // 谓词是 `span.end < range.start`（**严格小于**），故它恰好留下
        // "终点等于起点"的那一个——那是**相接**的区间，必须被吸收。
        //
        // ## 这里踩过的一个坑（记录以免复发）
        //
        // 初版写的是 `span.end < range.start + 1`，意图是"把相接也纳入"。
        // 但那与 `< range.start` 在整数上**等价**（`end < start+1` ⟺
        // `end <= start`），而它把"end == start"的相接区间判为"已在左边、
        // 可以跳过"——**恰好排除了要吸收的那一个**。
        //
        // 症状很隐蔽：`read_chars` 仍然正确（新增量按"合并后长度 − 被吸收
        // 长度"算，跳过一个相接区间时它的长度没被减掉……但因为没被吸收，
        // 合并区间也没包含它，两者抵消），只有 `span_count` 会暴露（区间
        // 数不减）。故本模块的测试同时断言**计数与区间数**。
        let first = self.spans.partition_point(|span| span.end < range.start);

        // 合并所有与本区间**相交或相接**的区间
        let mut merged_start = range.start;
        let mut merged_end = range.end;
        let mut last = first;

        while last < self.spans.len() {
            let span = &self.spans[last];
            // 严格在合并区间右侧（不相交且不相接）→ 停止
            if span.start > merged_end {
                break;
            }
            merged_start = merged_start.min(span.start);
            merged_end = merged_end.max(span.end);
            last += 1;
        }

        // 新增量 = 合并后长度 − 被吸收的那些区间的总长度
        let absorbed: usize = self.spans[first..last]
            .iter()
            .map(|span| span.end - span.start)
            .sum();
        let merged_len = merged_end - merged_start;
        let added = merged_len.saturating_sub(absorbed);

        if added > 0 {
            self.read_chars += added as i64;
            self.pending_chars += added as i64;
        }

        // 替换被吸收的区间为合并后的区间
        self.spans
            .splice(first..last, std::iter::once(merged_start..merged_end));
    }

    /// 累加阅读秒数（不参与去重）。
    pub fn add_seconds(&mut self, seconds: i64) {
        if seconds > 0 {
            self.pending_seconds += seconds;
        }
    }

    /// 去重累计的阅读字符数（SC-013b 的判据载体）。
    #[must_use]
    pub fn read_chars(&self) -> i64 {
        self.read_chars
    }

    /// 本阶段累计的阅读秒数（尚未提交的部分**不含**）。
    ///
    /// ## 为什么只报未提交的部分
    ///
    /// 已提交的秒数已经进了库（001 是累加语义），本对象不该重复持有它——
    /// 否则"提交两次"会把同一段时间算两遍。故这里的语义是"待上报的秒数"。
    #[must_use]
    pub fn read_seconds(&self) -> i64 {
        self.pending_seconds
    }

    /// 自上次提交以来**新增**的字符数。
    #[must_use]
    pub fn pending_chars(&self) -> i64 {
        self.pending_chars
    }

    /// 提交到落库接口（**节流后的唯一写库点**）。
    ///
    /// ## 三条规则
    ///
    /// | 情形 | 行为 | 返回 |
    /// |---|---|---|
    /// | 无新增字符**且**无新增秒数 | 不写 | `false` |
    /// | 有新增（任一项） | 写一条记录，然后清零待报量 | `true` |
    ///
    /// 第一条避免了"翻回上一页也写一次库"——那是最常见的写库放大源。
    /// 第二条把"只有时间推进"（用户停留在一页上读）也计入，否则"阅读时长"
    /// 会丢。
    ///
    /// ## 写入的是**增量**（不是累计总量）
    ///
    /// 001 的 `add_read_record` 是累加语义（FR-026b）。若写累计总量，库里会
    /// 变成 `total + total' + ...`。故本方法写完即清零待报量。
    pub fn commit(&mut self, sink: &mut impl RecordSink) -> bool {
        if self.pending_chars <= 0 && self.pending_seconds <= 0 {
            return false;
        }

        sink.add_read_record(self.pending_seconds, self.pending_chars);
        self.pending_seconds = 0;
        self.pending_chars = 0;
        true
    }

    /// 已读区间的个数（诊断与测试用，不对外承诺稳定语义）。
    #[must_use]
    pub fn span_count(&self) -> usize {
        self.spans.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Counter {
        calls: usize,
    }

    impl RecordSink for Counter {
        fn add_read_record(&mut self, _read_seconds: i64, _read_chars: i64) {
            self.calls += 1;
        }
    }

    #[test]
    fn overlapping_ranges_count_once() {
        let mut accumulator = ReadingRecordAccumulator::new();
        accumulator.observe_range(0..100);
        accumulator.observe_range(50..150);
        accumulator.observe_range(0..150);
        assert_eq!(accumulator.read_chars(), 150);
    }

    #[test]
    fn adjacent_ranges_merge() {
        let mut accumulator = ReadingRecordAccumulator::new();
        accumulator.observe_range(0..50);
        accumulator.observe_range(50..100);
        assert_eq!(accumulator.read_chars(), 100);
        assert_eq!(accumulator.span_count(), 1, "相接的区间应合并为一个");
    }

    #[test]
    fn disjoint_ranges_stay_separate() {
        let mut accumulator = ReadingRecordAccumulator::new();
        accumulator.observe_range(0..50);
        accumulator.observe_range(100..150);
        assert_eq!(accumulator.read_chars(), 100);
        assert_eq!(accumulator.span_count(), 2);
    }

    #[test]
    fn reversed_and_empty_ranges_are_ignored() {
        let mut accumulator = ReadingRecordAccumulator::new();
        // 用变量构造（字面量反向区间会被 clippy 判为"空区间"并报错）
        let (start, end) = (100usize, 50usize);
        accumulator.observe_range(start..end);
        accumulator.observe_range(10..10);
        assert_eq!(accumulator.read_chars(), 0);
        assert_eq!(accumulator.span_count(), 0);
    }

    #[test]
    fn out_of_order_insertion_merges_correctly() {
        let mut accumulator = ReadingRecordAccumulator::new();
        accumulator.observe_range(100..200);
        accumulator.observe_range(0..50);
        accumulator.observe_range(50..150); // 把两段连起来
        assert_eq!(accumulator.read_chars(), 200);
        assert_eq!(accumulator.span_count(), 1);
    }

    #[test]
    fn commit_requires_new_progress() {
        let mut accumulator = ReadingRecordAccumulator::new();
        let mut sink = Counter::default();

        assert!(!accumulator.commit(&mut sink), "无新增不得写库");
        assert_eq!(sink.calls, 0);

        accumulator.observe_range(0..10);
        assert!(accumulator.commit(&mut sink));
        assert_eq!(sink.calls, 1);

        // 重复同一区间 → 无新增 → 不写
        accumulator.observe_range(0..10);
        assert!(!accumulator.commit(&mut sink));
        assert_eq!(sink.calls, 1);
    }

    #[test]
    fn commit_reports_increment_not_total() {
        let mut accumulator = ReadingRecordAccumulator::new();
        let mut sink = Counter::default();

        accumulator.observe_range(0..100);
        assert!(accumulator.commit(&mut sink));
        assert_eq!(accumulator.pending_chars(), 0, "提交后应清零");

        accumulator.observe_range(100..180);
        assert_eq!(accumulator.pending_chars(), 80, "待报量应是增量");
        assert_eq!(accumulator.read_chars(), 180, "累计量不受影响");
    }

    #[test]
    fn time_only_commit_still_writes() {
        let mut accumulator = ReadingRecordAccumulator::new();
        let mut sink = Counter::default();

        accumulator.add_seconds(30);
        assert!(accumulator.commit(&mut sink), "只有时间推进也应写");
        assert_eq!(accumulator.read_seconds(), 0, "提交后秒数清零");
    }

    #[test]
    fn negative_seconds_are_ignored() {
        let mut accumulator = ReadingRecordAccumulator::new();
        accumulator.add_seconds(-10);
        assert_eq!(accumulator.read_seconds(), 0);
    }
}
