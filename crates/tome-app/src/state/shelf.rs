//! `[WIP]` 书架状态（S3 T011）
//!
//! ## 这一层解决什么
//!
//! 书架的**全部决策**收在这里：有哪些条目、哪些可见、按什么顺序、滚动到
//! 哪里、选中了谁。视图（`views/shelf.rs`）只做"把这里算出来的东西画出来"。
//!
//! 分开的理由与 `app.rs` 的说明同源：SC-005（1000 册 60fps）与 SC-010
//! （往返状态保持）都是**可判定**的要求，若判定它们必须先开窗口、拖滚动条、
//! 数帧，判据就退化成人工观察。做成纯值后，"1000 册过滤一次要多久"是一次
//! `Instant::now()` 包住的函数调用，"往返后滚动位置变没变"是两次方法调用
//! 加一次相等比较——**都能进 CI**。
//!
//! ## 过滤与排序为什么必须是两个正交的量
//!
//! FR-005 要求"按标题过滤"与"按最近阅读/标题排序"**同时**可用。实现上最
//! 容易写错的形态是"过滤时顺手按当前排序重排一次、排序时顺手再过滤一次"
//! ——两处各自维护同一套逻辑，于是"改了一处忘了另一处"必然发生。
//!
//! 这里的形态是：**过滤与排序各自是纯函数**，`visible()` 把它们串起来。
//! `tests/shelf_state.rs` 有一条正交性断言（排序不改变过滤后的集合）。
//!
//! ## 为什么排序键是三态而不是 `bool`
//!
//! 初版把排序做成 `sort_descending: bool`（只按标题，正序/倒序）。FR-005
//! 要的是**两个键**（最近阅读 / 标题），而"最近阅读"没有"倒序"这个说法
//! ——它按定义就是从新到旧。用 `bool` 表达两个键会立刻出现无意义状态
//! （"最近阅读的倒序"= 从旧到新，没人想要）。三态枚举让无意义状态不存在。

use crate::engine::messages::ShelfEntry;

/// 排序键（FR-005 的两种排序 × 标题的两个方向）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShelfSort {
    /// 最近阅读（新 → 旧；从未打开的书排最后）
    #[default]
    Recent,
    /// 标题升序
    TitleAsc,
    /// 标题降序
    TitleDesc,
}

impl ShelfSort {
    /// 循环切换（视图上的"排序"按钮点一下走一格）。
    ///
    /// 三态循环的顺序按**使用频率**排：默认的最近阅读在最前，其次升序，
    /// 最后降序。点击是线性遍历，故顺序即"点几下能得到什么"。
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Recent => Self::TitleAsc,
            Self::TitleAsc => Self::TitleDesc,
            Self::TitleDesc => Self::Recent,
        }
    }
}

/// 书架状态（**纯值**，headless 可测）。
#[derive(Debug, Clone, Default)]
pub struct ShelfState {
    /// 全部在架条目（引擎事件的落地处；顺序无意义，呈现顺序由 [`Self::visible`] 决定）
    entries: Vec<ShelfEntry>,
    /// 过滤词（按标题子串匹配）
    filter: String,
    /// 排序键
    sort: ShelfSort,
    /// 滚动位置（逻辑点；**只由视图写回**，状态层不解释它的含义）
    scroll: f32,
    /// 选中项（键盘导航用）
    selected: Option<String>,
    /// 是否已完成首次加载（`false` 时视图应显示加载态而不是空态）
    loaded: bool,
}

impl ShelfState {
    /// 空书架（未加载）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // --- 条目 ---

    /// 全量替换（`ShelfLoaded` 事件的落地）。
    ///
    /// ## 为什么选中项要在这里校验
    ///
    /// 移除一本书之后，`selected` 可能指向一个已不存在的条目——那会让
    /// 键盘导航从"不存在的选中项"开始，症状是"按 Enter 没反应"。
    /// 校验收在唯一的写入点上，视图不必各自防御。
    pub fn replace_all(&mut self, entries: Vec<ShelfEntry>) {
        self.entries = entries;
        self.loaded = true;
        self.retain_valid_selection();
    }

    /// 单条新增或更新（`ShelfEntryChanged` 事件的落地）。
    ///
    /// ## 为什么按 `book_id` 就地更新而不是 push
    ///
    /// 重复导入必须识别为同一条目（spec 边缘情况："更新而非重复卡片"）。
    /// 判定收在这里，与 `ShelfEntryChanged` 的语义一致：那个事件既表示
    /// "新书导入"也表示"进度变了"，两者的区别就是 `book_id` 是否已存在。
    pub fn upsert(&mut self, entry: ShelfEntry) {
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.book_id == entry.book_id)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }

    /// 移除一条（确认之后）。
    pub fn remove(&mut self, book_id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.book_id != book_id);
        self.retain_valid_selection();
        self.entries.len() != before
    }

    /// 是否已完成首次加载。
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// 全量条目数（**过滤前**——契约 V2 的"书架有 N 册"说的是这个数）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否一本书都没有（与"过滤后无结果"是**两件不同的事**）。
    ///
    /// 区分它们有直接的界面后果：空书架显示引导（V1），过滤无结果只显示
    /// "没有匹配的书"（V2 的过滤态）。若把两者合并，用户在过滤框里打错
    /// 一个字就会看到"书架是空的，请导入一本书"——那是在撒谎。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 按标识取条目（全量，不受过滤影响）。
    #[must_use]
    pub fn get(&self, book_id: &str) -> Option<&ShelfEntry> {
        self.entries.iter().find(|entry| entry.book_id == book_id)
    }

    /// 全量条目（只读）。
    #[must_use]
    pub fn entries(&self) -> &[ShelfEntry] {
        &self.entries
    }

    // --- 过滤 ---

    /// 设置过滤词。
    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
    }

    /// 当前过滤词。
    #[must_use]
    pub fn filter(&self) -> &str {
        &self.filter
    }

    // --- 排序 ---

    /// 当前排序键。
    #[must_use]
    pub fn sort(&self) -> ShelfSort {
        self.sort
    }

    /// 设置排序键。
    pub fn set_sort(&mut self, sort: ShelfSort) {
        self.sort = sort;
    }

    /// 切换到下一个排序键（视图的"排序"按钮）。
    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
    }

    // --- 滚动与选中 ---

    /// 记录滚动位置（视图每帧写回）。
    pub fn set_scroll(&mut self, offset: f32) {
        self.scroll = offset;
    }

    /// 滚动位置。
    #[must_use]
    pub fn scroll(&self) -> f32 {
        self.scroll
    }

    /// 选中项。
    #[must_use]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// 设置选中项。
    pub fn select(&mut self, book_id: Option<String>) {
        self.selected = book_id;
    }

    // --- 呈现（过滤 + 排序的合成）---

    /// 可见条目（**唯一**的过滤 + 排序实现）。
    ///
    /// ## 为什么返回引用切片而不是克隆
    ///
    /// 1000 册的书架上，每次重绘都克隆一遍条目是纯粹的浪费（而书卡只需要
    /// 读）。返回 `Vec<&ShelfEntry>` 让排序的代价只落在指针上。
    ///
    /// ## 过滤为什么是"标题子串 + trim"
    ///
    /// FR-005 只说"按标题过滤"。取子串匹配是**最不容易让人困惑**的形态：
    /// 用户输入"三体"期望看到《三体》与《三体 II》，而正则/前缀匹配都会
    /// 给出意外结果。`trim` 是因为过滤框里极易混入尾随空格（粘贴、输入法），
    /// 而"我明明打了字却什么都没匹配到"是极难自查的。
    #[must_use]
    pub fn visible(&self) -> Vec<&ShelfEntry> {
        let needle = self.filter.trim();
        let mut visible: Vec<&ShelfEntry> = self
            .entries
            .iter()
            .filter(|entry| needle.is_empty() || entry.title.contains(needle))
            .collect();

        match self.sort {
            ShelfSort::Recent => {
                // `last_read_at` 为 `None` = 从未打开，排最后（而不是当成 0
                // ——那会让"从没读过的书"混在很久以前读过的书中间）。
                visible.sort_by(
                    |left, right| match (left.last_read_at, right.last_read_at) {
                        (Some(a), Some(b)) => b.cmp(&a),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        // 都没读过时按标题，避免顺序随插入顺序漂移（那会让"列表
                        // 每次打开都不一样"，看起来像随机）
                        (None, None) => left.title.cmp(&right.title),
                    },
                );
            }
            ShelfSort::TitleAsc => visible.sort_by(|left, right| left.title.cmp(&right.title)),
            ShelfSort::TitleDesc => visible.sort_by(|left, right| right.title.cmp(&left.title)),
        }

        visible
    }

    /// 可见条目数。
    #[must_use]
    pub fn visible_len(&self) -> usize {
        self.visible().len()
    }

    /// 选中项是否已被移除（移除后清空选中）。
    fn retain_valid_selection(&mut self) {
        if let Some(selected) = &self.selected {
            if !self.entries.iter().any(|entry| entry.book_id == *selected) {
                self.selected = None;
            }
        }
    }
}

/// 书架网格的**几何**（虚拟化的全部数学）。
///
/// ## 为什么把虚拟化算在这里而不是视图里
///
/// T012 的验收子项是"1000 册仅实例化可视区卡片"。若那段计算写在绘制函数里，
/// 判定它就只能靠开窗口、数帧——判据退化成人工观察。做成纯值后，
/// "1000 册在 800px 高的视口里实例化了几张卡"是一次函数调用加一个区间比较，
/// **能进 CI**。
///
/// ## 为什么是"固定卡片宽 + 由可用宽推导列数"
///
/// 虚拟化的前提是"第 N 张卡画在哪"可**直接算出**。自适应宽高（如等分可用宽）
/// 会让位置依赖一次布局测量，而那正是虚拟化最不该有的东西——它会在滚动时
/// 产生跳动。固定宽让 [`Self::card_rect`] 是一个纯算术表达式。
///
/// ## 为什么用"行"而不是"索引"做可视判据
///
/// 网格按行排列：同一行的卡片高度相同、y 相同。按行判定可视性只需一次除法，
/// 而按索引判定要做两次（行 + 列）。先算行区间、再摊成索引区间（
/// [`Self::visible_index_range`]），两步各自可单独断言。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLayout {
    /// 列数（至少 1）
    columns: usize,
    /// 卡片宽（逻辑点）
    card_width_px: f32,
    /// 卡片高（逻辑点）
    card_height_px: f32,
    /// 行距与列距（网格槽距）
    gap_px: f32,
}

impl GridLayout {
    /// 由可用宽、卡片尺寸与槽距推导列数。
    ///
    /// 列数 = `floor((可用宽 + 槽距) / (卡宽 + 槽距))`，至少 1 列。
    ///
    /// ## 为什么加槽距再除
    ///
    /// N 张卡加 N-1 个槽的总宽是 `N*卡宽 + (N-1)*槽距`。写成
    /// `floor((W + g) / (w + g))` 与之等价，但避免了"先乘再减"的边界漂移
    /// ——那会让窗口宽度恰好等于 `N*卡宽 + (N-1)*槽距` 时少算一列（卡片被挤
    /// 到下一行，表现为"明明放得下却只显示 4 列"）。
    #[must_use]
    pub fn new(
        available_width_px: f32,
        card_width_px: f32,
        card_height_px: f32,
        gap_px: f32,
    ) -> Self {
        let usable = available_width_px.max(0.0);
        let stride = (card_width_px + gap_px).max(1.0);
        let columns = (((usable + gap_px) / stride).floor() as usize).max(1);
        Self {
            columns,
            card_width_px,
            card_height_px,
            gap_px,
        }
    }

    /// 列数。
    #[must_use]
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// 卡片宽。
    #[must_use]
    pub fn card_width_px(&self) -> f32 {
        self.card_width_px
    }

    /// 卡片高。
    #[must_use]
    pub fn card_height_px(&self) -> f32 {
        self.card_height_px
    }

    /// 行距。
    #[must_use]
    pub fn gap_px(&self) -> f32 {
        self.gap_px
    }

    /// 一行的高度（卡高 + 行距）。
    #[must_use]
    pub fn row_stride_px(&self) -> f32 {
        self.card_height_px + self.gap_px
    }

    /// 总行数。
    #[must_use]
    pub fn row_count(&self, entry_count: usize) -> usize {
        entry_count.div_ceil(self.columns)
    }

    /// 全部内容的高度（供 `ScrollArea` 的 `content_size`）。
    #[must_use]
    pub fn content_height_px(&self, entry_count: usize) -> f32 {
        let rows = self.row_count(entry_count);
        if rows == 0 {
            return 0.0;
        }
        rows as f32 * self.row_stride_px() - self.gap_px
    }

    /// 第 `index` 张卡的左上角坐标（相对内容区原点）。
    #[must_use]
    pub fn card_position(&self, index: usize) -> (f32, f32) {
        let row = index / self.columns;
        let column = index % self.columns;
        (
            column as f32 * (self.card_width_px + self.gap_px),
            row as f32 * self.row_stride_px(),
        )
    }

    /// 可视的**行**区间（半开）。
    ///
    /// ## 为什么要多留一行（上下各一）
    ///
    /// 滚动是连续的：某一行的底部可能只露出 1px，但用户马上就要看到它。
    /// 只实例化"完全落在视口内"的行，会让卡片在滚入视口的那一帧才被创建
    /// ——表现为"边缘的卡片突然出现"。多留一行是廉价的（一行的卡片数很少），
    /// 换来的是滚动的连续性。
    #[must_use]
    pub fn visible_row_range(
        &self,
        entry_count: usize,
        scroll_offset_px: f32,
        viewport_height_px: f32,
    ) -> std::ops::Range<usize> {
        let rows = self.row_count(entry_count);
        if rows == 0 || viewport_height_px <= 0.0 {
            return 0..0;
        }
        let stride = self.row_stride_px().max(1.0);
        let first = (scroll_offset_px.max(0.0) / stride).floor() as usize;
        let last = ((scroll_offset_px.max(0.0) + viewport_height_px) / stride).ceil() as usize;
        let start = first.saturating_sub(1).min(rows);
        let end = (last + 1).min(rows);
        start..end.max(start)
    }

    /// 可视的**索引**区间（半开；把行区间摊成条目下标）。
    #[must_use]
    pub fn visible_index_range(
        &self,
        entry_count: usize,
        scroll_offset_px: f32,
        viewport_height_px: f32,
    ) -> std::ops::Range<usize> {
        let rows = self.visible_row_range(entry_count, scroll_offset_px, viewport_height_px);
        let start = (rows.start * self.columns).min(entry_count);
        let end = (rows.end * self.columns).min(entry_count);
        start..end.max(start)
    }
}
