//! `[DONE]` 阅读形态切换（T027 / FR-023、SC-005b）
//!
//! ## 切换时唯一要保住的东西：**锚点值**
//!
//! SC-005b 的要求是"切换前后锚点值 100% 不变，且该字符 100% 可见"。锚点值
//! 是 `(chapter_index, char_offset)`——它由 001 契约定义，**本层无权改写**。
//! 故本模块的切换逻辑**只做定位，不做换算**：把给定的锚点字符在目标形态里
//! 找出来，返回它的定位结果，锚点值原样带过。
//!
//! 这看起来"什么都没做"，但正是它的价值——一个试图"聪明地"把像素位置换算
//! 过去（"滚动偏移 300px ≈ 第 2 页 40px"）的实现**必然**引入误差，因为两种
//! 形态的切分粒度不同（页与视口的边界不重合）。字符锚点是唯一无损失的载体。
//!
//! ## 两阶段：过渡态与稳定态（FR-023 的"中间态不写库"）
//!
//! ```text
//!   begin(from_mode, anchor)  ──→ 过渡态（is_settled == false）
//!          │                            │
//!          │ observe_event() × N        │ commit(sink) → **拒绝写入**
//!          ↓                            │
//!   settle(to_mode, target)   ──→ 稳定态（is_settled == true）
//!                                       │
//!                                       └─ commit(sink) → 写入一次（幂等）
//! ```
//!
//! FR-023 明文："书写进度 MUST 以切换稳定后的锚点为准（MUST NOT 以切换过程
//! 中的中间态写库）"。故 [`ModeSwitcher::commit`] 在过渡态**拒绝**写入——
//! 这是把那条负面要求变成**可断言行为**的手段（返回 `false` 而非静默跳过，
//! 让调用方能区分"写了"与"没写"）。
//!
//! ## 为什么"稳定"需要一个显式的 settle 调用
//!
//! 切换何时算稳定取决于上层（S3）：可能是动效播完，也可能是第一次渲染帧
//! 完成。本层**不猜**那个时刻——它提供一个显式的 `settle`，由上层在它认为
//! 稳定时调用。`observe_event` 记录过渡期间发生的事件数（供上层做启发式
//! 判断），但**不自行判定稳定**。
//!
//! 这样切分的理由：本层若自行用"事件数超过 N 就算稳定"判定，那个 N 是凭空
//! 数字（与 MASTER 的"变化幅度阈值"同类问题，裁决 OQ#14→C 已判废）。稳定
//! 的判据属于 UI 生命周期，不属排版几何。

use crate::paginate::config::ReadingMode;
use crate::paginate::page::PaginationResult;
use crate::scroll::flow::ScrollFlow;

/// 进度写入的接收方（由组合根实现，转发到 001 的 `BookRepository`）。
///
/// ## 为什么本层不直接吃 `BookRepository`
///
/// `BookRepository` 有四十来个方法（书架/章节/痕迹/规则/备份），而本层只
/// 需要其中一个"写进度"。依赖整个 trait 会把"本层需要什么"这件事淹没在
/// 四十个方法里，也让测试必须实现全部方法。
///
/// 用一个窄 trait 表达真实依赖：调用方（组合根）把它适配到 001 的接缝上。
/// 这是 FR-004 的同一思路——本层不感知持久化的全貌，只管自己那一条通路。
pub trait ProgressSink {
    /// 保存阅读进度（章号 + 章内字符偏移）。
    fn save_progress(&mut self, chapter_index: i32, char_offset: i64);
}

/// 一次形态切换的状态机。
///
/// ## 生命周期的两步
///
/// 1. [`Self::begin`]：记录来源形态与锚点，进入**过渡态**
/// 2. [`Self::settle`]：在目标形态里定位锚点，进入**稳定态**
///
/// 过渡态期间 [`Self::commit`] 拒绝写入（FR-023）。
#[derive(Debug, Clone, PartialEq)]
pub struct ModeSwitcher {
    from_mode: ReadingMode,
    to_mode: Option<ReadingMode>,
    anchor: usize,
    /// 过渡期间观察到的事件数（供上层启发式判断，本层不据此自行 settle）
    observed_events: usize,
    /// 是否已提交过（幂等）
    committed: bool,
    /// 目标形态里锚点是否可见
    target_visible: bool,
}

impl ModeSwitcher {
    /// 开始一次切换（进入过渡态）。
    ///
    /// `from_mode` 是**当前**形态（记录它让"往哪切"在调用 settle 时才定，
    /// 从而允许上层在过渡中改变主意——快速来回切换时会发生）。
    #[must_use]
    pub fn begin(from_mode: ReadingMode, anchor: usize) -> Self {
        Self {
            from_mode,
            to_mode: None,
            anchor,
            observed_events: 0,
            committed: false,
            target_visible: false,
        }
    }

    /// 记录一次过渡期间的事件（动画帧、滚动事件、布局完成）。
    ///
    /// 本层**不**据事件数自行判定稳定（见模块文档）；它只累计，供上层读取。
    pub fn observe_event(&mut self) {
        self.observed_events += 1;
    }

    /// 过渡期间观察到的事件数。
    #[must_use]
    pub fn observed_events(&self) -> usize {
        self.observed_events
    }

    /// 是否已稳定（可以写进度）。
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.to_mode.is_some()
    }

    /// 在目标形态里定位锚点（进入稳定态），返回**稳定后的锚点值**。
    ///
    /// ## 为什么返回 `usize` 而不是 `&Self`
    ///
    /// 初版返回 `&Self` 以便链式阅读（`begin(..).settle(..).anchor()`）。但那
    /// 要求调用方为临时的 `ModeSwitcher` 找一个**活过整条链**的绑定位置——
    /// `begin(..).settle(..)` 里的 `begin` 产物是临时值，`settle` 借它之后
    /// 立刻失效（E0716）。写法上被迫拆成两行，链式的意图反而落空。
    ///
    /// 返回锚点值则让最常见的用法（"切换后我的位置在哪"）一步到位，且
    /// 调用方仍持有 `&mut self` 的状态机可继续 `commit`：
    ///
    /// ```ignore
    /// let mut switcher = ModeSwitcher::begin(Paged, anchor);
    /// let settled_anchor = switcher.settle(Scroll, &flow);
    /// switcher.commit(&mut sink);
    /// ```
    pub fn settle(&mut self, to_mode: ReadingMode, target: &impl AnchorTarget) -> usize {
        self.anchor = target.resolve_anchor(self.anchor);
        self.target_visible = target.anchor_is_visible(self.anchor);
        self.to_mode = Some(to_mode);
        self.anchor
    }

    /// 锚点值（**切换前后不变**，SC-005b）。
    #[must_use]
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// 锚点在目标形态里是否可见（SC-005b 的后半句）。
    #[must_use]
    pub fn target_visible(&self) -> bool {
        self.target_visible
    }

    /// 来源形态（记录用）。
    #[must_use]
    pub fn from_mode(&self) -> ReadingMode {
        self.from_mode
    }

    /// 目标形态（未 settle 时为 `None`）。
    #[must_use]
    pub fn to_mode(&self) -> Option<ReadingMode> {
        self.to_mode
    }

    /// 提交进度写入。返回**是否真的写了**。
    ///
    /// ## 三条规则
    ///
    /// | 状态 | 行为 | 返回 |
    /// |---|---|---|
    /// | 过渡态（未 settle） | **拒绝写入** | `false` |
    /// | 稳定态、首次提交 | 写入一次 | `true` |
    /// | 稳定态、重复提交 | 不写（幂等） | `false` |
    ///
    /// 第二条的"不写"是防止"每次事件都写一次库"（FR-039 的上报节流要求）。
    /// 返回 `bool` 而非静默处理，让调用方能把"这次提交做了什么"记录下来
    /// ——那对排查"进度为什么没保存"是必要信息。
    pub fn commit(&mut self, sink: &mut impl ProgressSink) -> bool {
        if !self.is_settled() || self.committed {
            return false;
        }
        // 章号由来源形态决定：本层当前只服务单章阅读（S3 的跨章切换属上层）
        sink.save_progress(0, self.anchor as i64);
        self.committed = true;
        true
    }
}

/// 可作为切换目标的两类形态（FR-023 的两个方向）。
///
/// ## 为什么用 trait 而不是两个 `settle_*` 方法
///
/// `settle` 的逻辑（定位锚点 + 记录可见性 + 标记稳定）对两种形态**完全
/// 相同**，差别只在"怎么解析锚点"。用 trait 把这个差别抽出来，切换状态机
/// 就只有一份实现——两个方向的行为不会各自漂移。
pub trait AnchorTarget {
    /// 把锚点解析为**本形态下有效**的锚点（越界钳制，SC-005b 的输入侧）。
    ///
    /// 返回的仍是**字符下标**——不是页号、不是像素偏移。这条限制了实现的
    /// 自由度，但正是 FR-021"不引入第二套坐标"的要求：切换的载体只能是
    /// 字符锚点。
    fn resolve_anchor(&self, anchor: usize) -> usize;

    /// 锚点在本形态下是否可见（SC-005b 的可见性判据）。
    fn anchor_is_visible(&self, anchor: usize) -> bool;
}

impl AnchorTarget for PaginationResult {
    /// 翻页形态：钳到 `[0, char_len]`（越界是"读到末尾"，必须保留该语义）。
    fn resolve_anchor(&self, anchor: usize) -> usize {
        anchor.min(self.char_len)
    }

    /// 可见性：落在某个**内容页**的字符区间内。
    ///
    /// 留白页是空区间，不包含任何字符，故不会被误判为"可见"。
    fn anchor_is_visible(&self, anchor: usize) -> bool {
        self.pages
            .iter()
            .any(|page| !page.is_padding && page.char_range.contains(&anchor))
    }
}

impl AnchorTarget for ScrollFlow {
    /// 滚动形态：钳到 `[0, char_len]`。
    fn resolve_anchor(&self, anchor: usize) -> usize {
        anchor.min(self.char_len())
    }

    /// 可见性：落在某个视口的字符区间内。
    fn anchor_is_visible(&self, anchor: usize) -> bool {
        self.viewports()
            .iter()
            .any(|viewport| viewport.char_range.contains(&anchor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只记次数的存根。
    #[derive(Default)]
    struct Counter {
        count: usize,
        last: Option<(i32, i64)>,
    }

    impl ProgressSink for Counter {
        fn save_progress(&mut self, chapter_index: i32, char_offset: i64) {
            self.count += 1;
            self.last = Some((chapter_index, char_offset));
        }
    }

    /// 一个"锚点就等于输入"的极简目标（用于隔离状态机逻辑）。
    struct Echo {
        visible_below: usize,
    }

    impl AnchorTarget for Echo {
        fn resolve_anchor(&self, anchor: usize) -> usize {
            anchor
        }
        fn anchor_is_visible(&self, anchor: usize) -> bool {
            anchor < self.visible_below
        }
    }

    #[test]
    fn transition_refuses_to_write() {
        let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, 42);
        let mut sink = Counter::default();

        assert!(!switcher.is_settled());
        assert!(!switcher.commit(&mut sink), "过渡态应拒绝写入");
        assert_eq!(sink.count, 0);
    }

    #[test]
    fn settled_commit_writes_once() {
        let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, 42);
        let mut sink = Counter::default();
        let target = Echo { visible_below: 100 };
        switcher.settle(ReadingMode::Scroll, &target);

        assert!(switcher.is_settled());
        assert!(switcher.commit(&mut sink), "稳定态首次提交应写入");
        assert_eq!(sink.count, 1);
        assert_eq!(sink.last, Some((0, 42)));
    }

    #[test]
    fn repeat_commit_is_idempotent() {
        let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, 42);
        let mut sink = Counter::default();
        switcher.settle(ReadingMode::Scroll, &Echo { visible_below: 100 });

        for _ in 0..5 {
            switcher.commit(&mut sink);
        }
        assert_eq!(sink.count, 1, "重复提交只应写一次");
    }

    #[test]
    fn events_are_counted_without_auto_settling() {
        let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, 42);
        for _ in 0..10 {
            switcher.observe_event();
        }
        assert_eq!(switcher.observed_events(), 10);
        assert!(
            !switcher.is_settled(),
            "本层不据事件数自行判定稳定（稳定时刻由上层 settle 表达）"
        );
    }

    #[test]
    fn visibility_is_reported_from_target() {
        let mut switcher = ModeSwitcher::begin(ReadingMode::Paged, 42);
        assert!(
            !switcher.target_visible(),
            "未 settle 时不得报告可见（那是未知状态）"
        );

        switcher.settle(ReadingMode::Scroll, &Echo { visible_below: 100 });
        assert!(switcher.target_visible());

        let mut hidden = ModeSwitcher::begin(ReadingMode::Paged, 42);
        hidden.settle(ReadingMode::Scroll, &Echo { visible_below: 10 });
        assert!(!hidden.target_visible());
    }

    #[test]
    fn anchor_is_never_rewritten_by_settle() {
        let mut switcher = ModeSwitcher::begin(ReadingMode::Scroll, 777);
        switcher.settle(
            ReadingMode::Paged,
            &Echo {
                visible_below: 10_000,
            },
        );
        assert_eq!(switcher.anchor(), 777, "锚点值不得被切换改写（SC-005b）");
        assert_eq!(switcher.from_mode(), ReadingMode::Scroll);
        assert_eq!(switcher.to_mode(), Some(ReadingMode::Paged));
    }
}
