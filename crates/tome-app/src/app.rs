//! `[WIP]` 应用骨架：视图状态机 + eframe 接线（S3 T009）
//!
//! ## 本模块的分层
//!
//! ```text
//!   ViewState（纯 Rust 值）      ← 六条不变量都在这层，headless 可测
//!        ↑ apply_event / 各种意图方法
//!   TomeApp（eframe::App）       ← 只做「消费状态 → 画」与「把输入翻成意图」
//! ```
//!
//! 分开的理由是判据的可达性：SC-010（往返状态保持）、SC-006（锚点保持）这类
//! 要求若只能在真窗口里验，判据就退化成人工点击。状态机是纯值后，往返就是
//! 两次方法调用，断言是普通的相等比较——**每一条不变量都能被 CI 覆盖**。
//!
//! ## 六条不变量（`data-model.md` §2）
//!
//! | # | 内容 | 断言位置 |
//! |---|---|---|
//! | 1 | `Shelf ⇄ Reading` 互斥，往返状态保持 | `tests/app_state.rs` |
//! | 2 | `content_version` 单调递增 | 同上 |
//! | 3 | 参数调整只改 `params`，`anchor` 不变 | 同上 |
//! | 4 | `SearchSession.generation` 全局单调，只呈现当前代际 | 同上 |
//! | 5 | 抽屉开合不改 `params` 与版心几何 | 同上 |
//! | 6 | `ErrorToast.actions` 非空 | 同上 |

use std::sync::Arc;

use tome_core::model::theme::Theme;

use crate::engine::messages::{
    ClassifiedErrorView, ContentState, EngineEvent, EngineRequest, PageLayout, ReadingAnchor,
    ReadingParams, SearchSession, ShelfEntry, ThemeChoice, UserSettings,
};
use crate::state::shelf::{ShelfSort, ShelfState};
use crate::theme_bridge::{BridgeTheme, FollowSystem, ThemeSource};

/// 视图（互斥切换的两个顶层视图）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppView {
    /// 书架
    Shelf,
    /// 阅读
    Reading,
}

/// 正交叠加层（不属于视图切换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    /// 设置叠加层
    Settings,
    /// 本地书导入对话框
    ImportLocal,
    /// 书源导入对话框
    ImportSource,
}

/// 抽屉页签（FR-008 的双页签）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerTab {
    /// 目录
    Toc,
    /// 书签
    Bookmarks,
}

/// 错误呈现的可行动作（DM §2 不变量 6：非空）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAction {
    /// 重试
    Retry,
    /// 换源（FR-016）
    SwitchSource,
    /// 返回书架
    Back,
}

/// 结构化错误呈现（含动作集）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorToast {
    error: ClassifiedErrorView,
    actions: Vec<ErrorAction>,
}

impl ErrorToast {
    /// 以默认动作集构造（`Back` 恒有：任何错误都能退回安全状态）。
    #[must_use]
    pub fn new(error: ClassifiedErrorView) -> Self {
        Self {
            error,
            actions: vec![ErrorAction::Back],
        }
    }

    /// 附加一个动作（去重）。
    #[must_use]
    pub fn with_action(mut self, action: ErrorAction) -> Self {
        if !self.actions.contains(&action) {
            self.actions.push(action);
        }
        self
    }

    /// 结构化错误。
    #[must_use]
    pub fn error(&self) -> &ClassifiedErrorView {
        &self.error
    }

    /// 可行动作集（**保证非空**）。
    #[must_use]
    pub fn actions(&self) -> &[ErrorAction] {
        &self.actions
    }
}

/// 阅读区的版心几何（**只含与分页/布局相关的量**，不含颜色）。
///
/// ## 为什么单独成类型而不是把字段摊在 `ViewState` 上
///
/// 抽屉开合"不得改变版心几何"（不变量 5）需要一个**可整体比较**的值。摊成
/// 若干字段后，那条断言就要逐个字段写——新增一个几何量时必然漏掉一处。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingGeometry {
    /// 窗口宽（px）
    window_width_px: f32,
    /// 阅读字号（px）
    font_size_px: f32,
    /// 自动判定允许的排布（单/双页）
    allowed: PageLayout,
}

impl ReadingGeometry {
    /// 自动判定允许的排布。
    #[must_use]
    pub fn allowed_layout(&self) -> PageLayout {
        self.allowed
    }

    /// 窗口宽。
    #[must_use]
    pub fn window_width_px(&self) -> f32 {
        self.window_width_px
    }

    /// 阅读字号。
    #[must_use]
    pub fn font_size_px(&self) -> f32 {
        self.font_size_px
    }
}

/// 视图状态机（**纯值**，headless 可测）。
#[derive(Debug)]
pub struct ViewState {
    view: AppView,
    overlay: Option<Overlay>,

    // --- 书架 ---
    //
    // 书架的**全部决策**（条目、过滤、排序、滚动、选中）在 `state::shelf`
    // 里，这里只持有一份实例。分开的理由见那个模块的文档：那些决策都有
    // 明确判据（SC-005/010），而判据不该依赖一个真实窗口。
    shelf: ShelfState,

    // --- 阅读 ---
    book_id: String,
    anchor: ReadingAnchor,
    params: ReadingParams,
    content: ContentState,
    content_version: Option<u64>,
    current_chapter: i32,
    drawer: Option<DrawerTab>,
    geometry: ReadingGeometry,
    /// 当前章的目录（T014 的页脚要显示章标题）
    toc: Vec<String>,
    /// 当前章的字符总数（页脚进度百分比的分母；来自排版结果）
    chapter_char_len: usize,
    /// 导入是否在进行中（T013 的骨架态判据）
    importing: bool,
    /// 首屏计时（SC-002：从"打开书"到 `ChapterReady` 抵达）
    ///
    /// 存成 `Option<Span>` 而不是在 `apply_event` 里现起一次：起点是**用户的
    /// 动作**（点书卡），不是"事件开始处理"。用错起点会让这个数字只反映引擎
    /// 的排版耗时，而 SC-002 说的是"打开书到首屏可阅读"。
    first_frame: Option<crate::timing::Span>,
    /// 待发往引擎的请求（由事件处理与视图意图共同产生）
    ///
    /// ## 为什么要有队列而不是"各处直接发"
    ///
    /// 状态机是**纯值**（headless 可测），它拿不到 `EngineHandle`。若把句柄
    /// 传进来，"每一条不变量都能被 CI 覆盖"这条性质就没了——测试就得起一条
    /// 真线程。
    ///
    /// 故状态机只把请求**排进队列**，由 `TomeApp` 在帧末统一发出。这条缝
    /// 让"状态变化产生了哪些请求"变成一个可断言的量（`tests/shelf_state.rs`
    /// 用它验证"打开书会请求 ReadingOpen"）。
    requests: Vec<EngineRequest>,

    // --- 搜索 ---
    search: Option<SearchSession>,
    next_generation: u64,
    search_hits: Vec<(String, Vec<crate::engine::messages::SearchHit>)>,

    // --- 错误与设置 ---
    error: Option<ErrorToast>,
    settings: UserSettings,
    theme: Theme,
}

impl Default for ViewState {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewState {
    /// 初始状态（空书架 + MASTER 默认参数 + 跟随系统主题）。
    #[must_use]
    pub fn new() -> Self {
        let params = ReadingParams::default_ma();
        Self {
            view: AppView::Shelf,
            overlay: None,
            shelf: ShelfState::new(),
            book_id: String::new(),
            anchor: ReadingAnchor {
                chapter_index: 0,
                char_offset: 0,
            },
            params,
            content: ContentState::Loading,
            content_version: None,
            current_chapter: 0,
            drawer: None,
            geometry: ReadingGeometry {
                window_width_px: 1280.0,
                font_size_px: params.font_size_px,
                allowed: PageLayout::Single,
            },
            toc: Vec::new(),
            chapter_char_len: 0,
            importing: false,
            first_frame: None,
            requests: Vec::new(),
            search: None,
            next_generation: 0,
            search_hits: Vec::new(),
            error: None,
            settings: UserSettings::default_ma(),
            theme: BridgeTheme::resolve(ThemeSource::FollowSystem, FollowSystem::system()).theme(),
        }
    }

    // --- 视图切换（不变量 1）---

    /// 当前视图。
    #[must_use]
    pub fn view(&self) -> AppView {
        self.view
    }

    /// 打开一本书（进入阅读视图）。
    ///
    /// ## 为什么要区分"同一本"与"另一本"
    ///
    /// SC-010 要求「书架 ⇄ 阅读往返后，阅读位置与参数 **100%** 保持」。若每次
    /// 打开都重置，往返就成了状态丢失——而那正是该判据要防的。
    ///
    /// 但打开**另一本**书时上一本的锚点必须清掉（否则会显示"第 7 章第 1234 字"
    /// 而其实在读另一本）。故：同一本保持，不同本重置。
    ///
    /// 真正的持久化进度恢复由 [`EngineEvent::ProgressRestored`] 抵达后覆盖——
    /// 它带的是库里存的值，与内存里的这份互为校验。
    ///
    /// ## 为什么同时发出 `ReadingOpen`
    ///
    /// 进入阅读视图与"引擎开始为这本书准备内容"是**同一件事的两面**：界面
    /// 若只切视图不发请求，用户会停在一个永远不会到达内容的加载态上。
    /// 两者一起做，`ReadingOpen` 的失败也走同一条错误呈现路径（R3）。
    pub fn open_book(&mut self, book_id: impl Into<String>) {
        let book_id = book_id.into();
        let is_same_book = self.book_id == book_id;

        self.book_id = book_id;
        self.view = AppView::Reading;
        self.drawer = None;

        if !is_same_book {
            self.anchor = ReadingAnchor {
                chapter_index: 0,
                char_offset: 0,
            };
            self.current_chapter = 0;
            self.content = ContentState::Loading;
            self.content_version = None;
            self.toc.clear();
            self.chapter_char_len = 0;
        }
    }

    /// 打开一本书的完整意图（视图切换 + 需要发出的引擎请求）。
    ///
    /// 返回请求而不是自己发：状态机是纯值，拿不到 `EngineHandle`（见
    /// `requests` 字段的说明）。视图层收下它、与自己的其它请求一并返回，
    /// 由 `TomeApp` 在帧末统一发出。
    #[must_use]
    pub fn open_book_request(&mut self, book_id: impl Into<String>) -> EngineRequest {
        let book_id = book_id.into();
        self.open_book(book_id.clone());
        // SC-002 的计时起点（"打开书"这个动作本身）
        self.first_frame = Some(crate::timing::Span::start());
        EngineRequest::ReadingOpen { book_id }
    }

    /// 返回书架（并冲刷进度，FR-009）。
    ///
    /// ## 为什么返回书架必须冲刷
    ///
    /// 用户"读完就退回书架"是最常见的收尾动作，而此刻内存里的位置可能还在
    /// 节流窗口内（尚未落盘）。若不冲刷，用户以为读完就走、结果下次打开回到
    /// 三秒前的位置——那看起来像"进度没记住"。
    #[must_use]
    pub fn back_to_shelf_request(&mut self) -> Option<EngineRequest> {
        let flush = self.flush_request();
        self.back_to_shelf();
        flush
    }

    /// 返回书架。
    pub fn back_to_shelf(&mut self) {
        self.view = AppView::Shelf;
        self.drawer = None;
    }

    /// 当前书的标识。
    #[must_use]
    pub fn book_id(&self) -> &str {
        &self.book_id
    }

    // --- 书架 ---
    //
    // 下面这些方法**全部**是 `ShelfState` 的转发。保留它们的理由是调用点的
    // 稳定性：T009 的状态机断言与 T012 的视图都按这些名字调用，而把决策搬进
    // `state::shelf` 是一次内部重构，不该迫使所有调用点跟着改。

    /// 书架状态（视图层的读入口）。
    #[must_use]
    pub fn shelf(&self) -> &ShelfState {
        &self.shelf
    }

    /// 书架状态（可变；供视图写回滚动位置与选中）。
    pub fn shelf_mut(&mut self) -> &mut ShelfState {
        &mut self.shelf
    }

    /// 设置过滤词。
    pub fn set_shelf_filter(&mut self, filter: String) {
        self.shelf.set_filter(filter);
    }

    /// 当前过滤词。
    #[must_use]
    pub fn shelf_filter(&self) -> &str {
        self.shelf.filter()
    }

    /// 记录书架滚动位置。
    pub fn set_shelf_scroll(&mut self, offset: f32) {
        self.shelf.set_scroll(offset);
    }

    /// 书架滚动位置。
    #[must_use]
    pub fn shelf_scroll(&self) -> f32 {
        self.shelf.scroll()
    }

    /// 书架条目数（全量）。
    #[must_use]
    pub fn shelf_len(&self) -> usize {
        self.shelf.len()
    }

    /// 按标识取条目。
    #[must_use]
    pub fn shelf_entry(&self, book_id: &str) -> Option<&ShelfEntry> {
        self.shelf.get(book_id)
    }

    /// 过滤后的可见条目（FR-005 的标题子串匹配）。
    #[must_use]
    pub fn visible_shelf_entries(&self) -> Vec<&ShelfEntry> {
        self.shelf.visible()
    }

    /// 切换排序键（视图的"排序"按钮；三态循环）。
    pub fn toggle_shelf_sort(&mut self) {
        self.shelf.cycle_sort();
    }

    /// 当前排序键。
    #[must_use]
    pub fn shelf_sort(&self) -> ShelfSort {
        self.shelf.sort()
    }

    /// 设置排序键。
    pub fn set_shelf_sort(&mut self, sort: ShelfSort) {
        self.shelf.set_sort(sort);
    }

    /// 选中的书卡。
    #[must_use]
    pub fn shelf_selected(&self) -> Option<&str> {
        self.shelf.selected()
    }

    /// 选中书卡。
    pub fn select_shelf_entry(&mut self, book_id: Option<String>) {
        self.shelf.select(book_id);
    }

    /// 导入是否进行中（T013 的骨架态判据）。
    #[must_use]
    pub fn is_importing(&self) -> bool {
        self.importing
    }

    /// 标记"导入已开始"（拖入或文件选择之后，事件回来之前）。
    pub fn begin_import(&mut self) {
        self.importing = true;
    }

    // --- 阅读 ---

    /// 当前锚点。
    #[must_use]
    pub fn anchor(&self) -> ReadingAnchor {
        self.anchor
    }

    /// 当前阅读参数。
    #[must_use]
    pub fn params(&self) -> ReadingParams {
        self.params
    }

    /// 当前内容态。
    #[must_use]
    pub fn content(&self) -> &ContentState {
        &self.content
    }

    /// 当前内容版本号。
    #[must_use]
    pub fn content_version(&self) -> Option<u64> {
        self.content_version
    }

    /// 当前章序号。
    #[must_use]
    pub fn current_chapter(&self) -> i32 {
        self.current_chapter
    }

    /// 当前章的目录（章标题列表）。
    #[must_use]
    pub fn toc(&self) -> &[String] {
        &self.toc
    }

    /// 当前章的字符总数（页脚进度百分比的分母；来自排版结果）。
    #[must_use]
    pub fn chapter_char_len(&self) -> usize {
        self.chapter_char_len
    }

    /// 版心几何。
    #[must_use]
    pub fn reading_geometry(&self) -> ReadingGeometry {
        self.geometry
    }

    /// 跳到锚点（目录跳转 / 书签跳转走同一路径，FR-008d）。
    pub fn seek_anchor(&mut self, anchor: ReadingAnchor) {
        self.anchor = anchor;
        self.current_chapter = anchor.chapter_index;
    }

    /// 跳到锚点的完整意图（状态 + 引擎请求入队）。
    pub fn seek_anchor_request(&mut self, anchor: ReadingAnchor) {
        self.seek_anchor(anchor);
        self.push_request(EngineRequest::ReadingSeekAnchor { anchor });
    }

    /// 翻页的完整意图（FR-006b 的章间接续由引擎处理）。
    ///
    /// ## 为什么锚点先动、请求后发
    ///
    /// 界面上"翻了一页"应当**立刻**有反应（进度条、页脚数字），而不是等引擎
    /// 的事件回来才更新。锚点的**权威值**随后由 `ChapterReady` 校正（引擎按
    /// 页边界给出精确的章内偏移），故这里的乐观更新不会留下错误。
    pub fn turn_page_request(&mut self, delta: i32) {
        self.push_request(EngineRequest::ReadingTurnPage { delta });
    }

    /// 告知引擎阅读区尺寸（每帧按需；引擎侧对未变化的尺寸去重）。
    pub fn report_viewport(&mut self, width_px: f32, height_px: f32) {
        self.push_request(EngineRequest::ReadingViewport {
            width_px,
            height_px,
        });
    }

    /// 应用阅读参数（**只改 params**，不碰 anchor —— 不变量 3）。
    ///
    /// ## 为什么这里不重算几何
    ///
    /// 几何里含"允许的单双页"，而它随字号变化（em 制判据）。但那条重算由
    /// [`Self::apply_params`] 一并做——**不是**因为调整会移动锚点，而是因为
    /// 字号变了阈值就变了。锚点是独立的量，不受影响。
    pub fn apply_params(&mut self, params: ReadingParams) {
        self.params = params;
        self.geometry.font_size_px = params.font_size_px;
        self.geometry = self.recompute_geometry(params.font_size_px);
    }

    /// 窗口宽度变化（FR-011 响应式）。
    pub fn set_window_width(&mut self, width_px: f32) {
        self.geometry.window_width_px = width_px;
        self.geometry = self.recompute_geometry(self.params.font_size_px);
    }

    /// 重算版心几何（**只回答"宽度容不容得下两栏"**）。
    ///
    /// ## 单双页判据（contracts/ui.md §4.3 / R8 的口径修正）
    ///
    /// MASTER §5.2 的公式给的是**阅读区可用宽度**的阈值（19px 时 1432px）。
    /// S3 的窗口宽阈值 = 1432 + 2×sp-10（40px）= **1512px**——两侧各 40px 的
    /// 阅读区内边距。这是 R8 明确的口径修正，不是随手加的余量。
    ///
    /// 判据**随字号缩放**（em 制，FR-014 明令不得用固定 px 断点）。
    ///
    /// ## 为什么这里不看 `params.page_layout`
    ///
    /// 那是**意图**，这是**能力**。两者的关系由 MASTER §5.2 第 3 条定死：
    /// 手动覆盖"在本次会话内优先于自动判定"，但**按语义回落失效**——只有
    /// 当手动选择与当前宽度相矛盾时（强制双页但容不下两栏）才回落到自动判定。
    ///
    /// 混在一起的写法（`if chosen == Single { Single } else { threshold }`）会
    /// 让"用户选单页"变成**永久**单页：窗口拉大到 2000px 也不切双页。那不是
    /// MASTER 的语义（它说的是本次会话内优先，不是否决自动判定）。故拆开：
    /// 本函数给"容不容得下"，[`Self::effective_layout`] 才是最终裁决。
    fn recompute_geometry(&self, font_size_px: f32) -> ReadingGeometry {
        const READING_PADDING_EACH_SIDE_PX: f32 = 40.0;

        let metrics = tome_core::model::theme::READING.with_font_size(font_size_px);
        let threshold = metrics.two_page_threshold_px() + 2.0 * READING_PADDING_EACH_SIDE_PX;

        let allowed = if self.geometry.window_width_px >= threshold {
            PageLayout::Spread
        } else {
            PageLayout::Single
        };

        ReadingGeometry {
            window_width_px: self.geometry.window_width_px,
            font_size_px,
            allowed,
        }
    }

    /// 最终生效的排布（**意图 + 能力**的裁决，MASTER §5.2 第 3 条）。
    ///
    /// - `Auto`（未表态）→ 宽度说了算
    /// - 强制双页但宽度容不下 → 回落单页（"强制双页但容不下两栏"）
    /// - 强制单页 → 恒单页（本次会话内优先）
    /// - 宽度变了但不改变"容不容得下" → 手动选择**不清除**（OQ#14→C）
    #[must_use]
    pub fn effective_layout(&self) -> PageLayout {
        match self.params.page_layout {
            PageLayout::Auto => self.geometry.allowed,
            PageLayout::Single => PageLayout::Single,
            PageLayout::Spread => self.geometry.allowed,
        }
    }

    // --- 抽屉（不变量 5）---

    /// 开合抽屉（同一页签再点一次即关闭）。
    pub fn toggle_drawer(&mut self, tab: DrawerTab) {
        if self.drawer == Some(tab) {
            self.drawer = None;
        } else {
            self.drawer = Some(tab);
        }
    }

    /// 切换抽屉页签（**不**关闭抽屉）。
    pub fn set_drawer_tab(&mut self, tab: DrawerTab) {
        if self.drawer.is_some() {
            self.drawer = Some(tab);
        }
    }

    /// 抽屉是否打开。
    #[must_use]
    pub fn drawer_is_open(&self) -> bool {
        self.drawer.is_some()
    }

    /// 当前抽屉页签。
    #[must_use]
    pub fn drawer_tab(&self) -> Option<DrawerTab> {
        self.drawer
    }

    // --- 搜索（不变量 4）---

    /// 开始一次搜索，返回本次的**代际号**（全局单调）。
    pub fn begin_search(&mut self, keyword: String) -> u64 {
        self.next_generation += 1;
        let generation = self.next_generation;
        self.search = Some(SearchSession {
            generation,
            keyword,
            done: false,
        });
        self.search_hits.clear();
        generation
    }

    /// 取消当前搜索。
    ///
    /// ## 为什么取消后 `generation` **不**清零
    ///
    /// 清零会让"取消前发起的请求"在结果回来时代际**恰好相等**，于是过期结果
    /// 被当成当前结果收下。保持单调是这条不变量成立的前提。
    pub fn cancel_search(&mut self) {
        if let Some(session) = &mut self.search {
            session.done = true;
        }
        // 会话本身留着（代际要继续单调），但结果清空
        self.search_hits.clear();
    }

    /// 当前搜索代际（无会话为 `None`）。
    #[must_use]
    pub fn search_generation(&self) -> Option<u64> {
        self.search.as_ref().map(|session| session.generation)
    }

    /// 搜索是否已结束。
    #[must_use]
    pub fn search_is_done(&self) -> bool {
        self.search.as_ref().is_some_and(|session| session.done)
    }

    /// 当前呈现的命中总数（按源分组后的合计）。
    #[must_use]
    pub fn search_hit_count(&self) -> usize {
        self.search_hits.iter().map(|(_, hits)| hits.len()).sum()
    }

    /// 按源分组的命中（供 V3 呈现）。
    #[must_use]
    pub fn search_groups(&self) -> &[(String, Vec<crate::engine::messages::SearchHit>)] {
        &self.search_hits
    }

    // --- 叠加层 ---

    /// 当前叠加层。
    #[must_use]
    pub fn overlay(&self) -> Option<Overlay> {
        self.overlay
    }

    /// 打开叠加层。
    ///
    /// ## 为什么打开设置时要顺带要一次设置
    ///
    /// 启动序列**不**请求设置（见 `start` 的说明：那会让用户一打开应用就看到
    /// 一个与他当下无关的能力提示）。设置在用户真的去看它时才要——此时提示
    /// 出现在上下文中，而不是凭空冒出来。
    pub fn open_overlay(&mut self, overlay: Overlay) {
        if overlay == Overlay::Settings {
            self.push_request(EngineRequest::SettingsLoad);
        }
        self.overlay = Some(overlay);
    }

    /// 关闭叠加层。
    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    // --- 错误（不变量 6）---

    /// 当前错误呈现。
    #[must_use]
    pub fn error(&self) -> Option<&ErrorToast> {
        self.error.as_ref()
    }

    /// 清除错误呈现。
    pub fn dismiss_error(&mut self) {
        self.error = None;
    }

    /// 当前主题 token。
    #[must_use]
    pub fn theme(&self) -> Theme {
        self.theme
    }

    /// 用户设置。
    #[must_use]
    pub fn settings(&self) -> UserSettings {
        self.settings
    }

    /// 应用主题选择（昼/夜/跟随系统）。
    pub fn apply_theme_choice(&mut self, choice: ThemeChoice) {
        self.params.theme = choice;
        self.theme = resolve_theme(choice);
    }

    // --- 事件分发（引擎 → 状态机）---

    /// 把一条引擎事件落到状态上。
    ///
    /// ## 为什么"别的书的事件"要被丢弃
    ///
    /// 引擎线程可能同时服务多本书的请求（如换源时新旧书都在动），而 UI 只呈现
    /// 一本。若不按 `book_id` 过滤，一本后台书的失败会在当前书上弹错误——那种
    /// 串扰极难复现与归因。过滤收在这一处，视图不必各自判断。
    pub fn apply_event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::ShelfLoaded { entries } => {
                self.shelf.replace_all(entries);
                // SC-001 的终点口径："书架视图可响应输入"。书架数据到位即为
                // 那一刻——视图此后每帧都能响应点击与键盘。
                crate::timing::milestone("shelf_loaded");
            }
            EngineEvent::ShelfEntryChanged { entry } => {
                self.shelf.upsert(entry);
            }
            EngineEvent::ImportFailed { error } => {
                self.importing = false;
                self.error = Some(
                    ErrorToast::new(error)
                        .with_action(ErrorAction::Retry)
                        .with_action(ErrorAction::Back),
                );
            }

            EngineEvent::ChapterReady {
                book_id,
                chapter_index,
                laid_out,
                content_version,
            } => {
                if book_id != self.book_id {
                    return;
                }
                // 不变量 2：版本**单调递增**——迟到的旧版本不得回退
                if self
                    .content_version
                    .is_none_or(|current| content_version >= current)
                {
                    self.content_version = Some(content_version);
                    self.content = ContentState::Ready { content_version };
                }
                self.current_chapter = chapter_index;
                // 章字符总数供页脚进度用（`laid_out` 是排版结果，它知道）
                if let Some(layout) = &laid_out {
                    self.chapter_char_len = layout.char_len;
                }
                // SC-002：从"打开书"到首屏可阅读
                if let Some(span) = self.first_frame.take() {
                    span.finish("first_frame");
                }
            }
            EngineEvent::ChapterFailed {
                book_id,
                chapter_index,
                error,
            } => {
                if book_id != self.book_id {
                    return;
                }
                self.current_chapter = chapter_index;
                self.content = ContentState::Failed {
                    error: error.clone(),
                };
                self.error = Some(
                    ErrorToast::new(error)
                        .with_action(ErrorAction::Retry)
                        .with_action(ErrorAction::SwitchSource)
                        .with_action(ErrorAction::Back),
                );
            }

            EngineEvent::ProgressRestored {
                book_id,
                progress,
                anchor,
            } => {
                if book_id != self.book_id {
                    return;
                }
                if let Some(params) = progress {
                    self.params = params;
                    self.geometry = self.recompute_geometry(params.font_size_px);
                }
                self.seek_anchor(anchor);
            }
            EngineEvent::TocLoaded { book_id, toc } => {
                if book_id != self.book_id {
                    return;
                }
                self.toc = toc;
            }
            EngineEvent::Bookmarks { .. } => {
                // 书签列表在 T020 落地；本任务只需状态机骨架。
            }

            EngineEvent::SearchResult {
                generation,
                source,
                hits,
            } => {
                // 不变量 4：只呈现**当前**代际的结果
                if !self.accepts_generation(generation) {
                    return;
                }
                match self
                    .search_hits
                    .iter_mut()
                    .find(|(source_id, _)| *source_id == source.source_id)
                {
                    Some((_, existing)) => existing.extend(hits),
                    None => self.search_hits.push((source.source_id, hits)),
                }
            }
            EngineEvent::SearchDone { generation } => {
                if !self.accepts_generation(generation) {
                    return;
                }
                if let Some(session) = &mut self.search {
                    session.done = true;
                }
            }
            EngineEvent::DetailLoaded { .. } | EngineEvent::AddedToShelf { .. } => {
                // 详情/加架在 T026 落地。
            }
            EngineEvent::SourceSwitched { book_id, anchor } => {
                if book_id != self.book_id {
                    return;
                }
                self.seek_anchor(anchor);
            }

            EngineEvent::SettingsLoaded { settings } => {
                self.settings = settings;
                self.params = settings.to_reading_params();
                self.theme = resolve_theme(settings.theme);
            }
            EngineEvent::Saved => {}

            EngineEvent::RequestFailed { request, error } => {
                // 非导入、非章节类失败的统一出口（见 `messages.rs` 的说明）。
                // 动作集按请求类型给：书架类可重试或返回，阅读类给换源。
                let toast = ErrorToast::new(error).with_action(ErrorAction::Retry);
                let toast = if request.starts_with("Reading") {
                    toast.with_action(ErrorAction::SwitchSource)
                } else {
                    toast
                };
                self.error = Some(toast.with_action(ErrorAction::Back));
            }

            EngineEvent::EnginePanic { context } => {
                // spec 边缘情况：引擎 panic 经外壳兜底捕获 → 结构化呈现。
                // **不**退出当前视图：用户应留在原地重试（判据见测试）。
                self.content = ContentState::Failed {
                    error: ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Content)
                        .with_detail(format!("引擎内部错误（{context}）")),
                };
                self.error = Some(
                    ErrorToast::new(
                        ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Content)
                            .with_detail(format!("引擎内部错误（{context}）")),
                    )
                    .with_action(ErrorAction::Retry)
                    .with_action(ErrorAction::Back),
                );
            }
        }
    }

    /// 某个代际的结果是否应被接受（不变量 4 的唯一判定点）。
    fn accepts_generation(&self, generation: u64) -> bool {
        self.search
            .as_ref()
            .is_some_and(|session| session.generation == generation && !session.done)
    }

    /// 把当前状态翻译成引擎请求（**退出前冲刷进度**等）。
    #[must_use]
    pub fn flush_request(&self) -> Option<EngineRequest> {
        if self.book_id.is_empty() {
            return None;
        }
        Some(EngineRequest::ProgressFlush {
            book_id: self.book_id.clone(),
        })
    }

    // --- 请求队列 ---

    /// 排入一个请求（视图与事件处理共用）。
    pub fn push_request(&mut self, request: EngineRequest) {
        self.requests.push(request);
    }

    /// 取走当前排队的全部请求。
    ///
    /// ## 为什么是"取走"而不是"读"
    ///
    /// 请求是**一次性**的：发出去之后队列就该空，否则下一帧会重发（表现为
    /// "翻一次页引擎收到两次请求"——重复的重排会被用户看成卡顿）。取走让
    /// "已发送"与"待发送"在类型层面分开。
    #[must_use]
    pub fn take_requests(&mut self) -> Vec<EngineRequest> {
        std::mem::take(&mut self.requests)
    }

    /// 当前排队的请求数（供断言）。
    #[must_use]
    pub fn pending_request_count(&self) -> usize {
        self.requests.len()
    }
}

/// 主题选择 → 具体 token。
fn resolve_theme(choice: ThemeChoice) -> Theme {
    let source = match choice {
        ThemeChoice::Day => ThemeSource::Day,
        ThemeChoice::Night => ThemeSource::Night,
        ThemeChoice::FollowSystem => ThemeSource::FollowSystem,
    };
    BridgeTheme::resolve(source, FollowSystem::system()).theme()
}

/// eframe 应用：**只做「消费状态 → 画」与「把输入翻成意图」**。
///
/// ## 为什么单实例句柄在这里
///
/// 第二实例被唤醒时，主实例要"把窗口带到前台"。`app-single-instance` 的回调
/// 在**另一条线程**上跑（它只负责 `request_repaint`），真正的 `ViewportCommand`
/// 必须在 UI 帧里发——故句柄存在应用上，每帧 `check_show()` 一次。
pub struct TomeApp {
    state: ViewState,
    /// 单实例句柄（`None` = 未启用，如测试构造）
    single_instance: Option<app_single_instance::PrimaryHandle>,
    /// 本帧是否需要把窗口带到前台（由单实例回调置位）
    bring_to_front: bool,
    /// 引擎句柄（T010 起接入真实 worker）
    engine: Option<crate::engine::thread::EngineHandle>,
    /// 版心帧缓存（T014；`content_version` 未变时复用已构造的绘制批次）
    #[cfg(feature = "reader-render")]
    frames: crate::render::texture::PageFrameCache,
    /// 当前版心（引擎事件带来的排版结果）
    ///
    /// ## 为什么 UI 侧要留一份 `ChapterLayout`
    ///
    /// 路径 A（PaintCallback 同 device 直绘）下，绘制发生在 egui 的 render
    /// pass 里，而那一阶段只拿得到回调值。构造绘制批次（`RenderBatch`）必须
    /// 在提交回调**之前**完成，故排版结果要在这里留一份。
    ///
    /// 它不参与序列化（`ChapterReady.laid_out` 的 `#[serde(skip)]` 即此意）
    /// ——排版结果是进程内渲染产物，跨进程无意义。
    #[cfg(feature = "reader-render")]
    laid_out: Option<Arc<tome_reader::paginate::page::ChapterLayout>>,
}

impl TomeApp {
    /// 以既有状态构造（测试与"从引擎句柄起步"的路径用）。
    #[must_use]
    pub fn with_state(state: ViewState) -> Self {
        Self {
            state,
            single_instance: None,
            bring_to_front: false,
            engine: None,
            #[cfg(feature = "reader-render")]
            frames: crate::render::texture::PageFrameCache::new(),
            #[cfg(feature = "reader-render")]
            laid_out: None,
        }
    }

    /// 状态机的只读访问（测试与集成用）。
    #[must_use]
    pub fn state(&self) -> &ViewState {
        &self.state
    }

    /// 状态机的可变访问。
    pub fn state_mut(&mut self) -> &mut ViewState {
        &mut self.state
    }

    /// 挂上单实例句柄。
    pub fn set_single_instance(&mut self, handle: app_single_instance::PrimaryHandle) {
        self.single_instance = Some(handle);
    }

    /// 挂上引擎句柄。
    pub fn set_engine(&mut self, engine: crate::engine::thread::EngineHandle) {
        self.engine = Some(engine);
    }

    /// 启动序列：向引擎要书架。
    ///
    /// ## 为什么不由引擎线程在启动时主动推
    ///
    /// 协议是**拉取式**的（UI 发请求、引擎回事件，见 `engine/thread.rs` 的
    /// 说明）。让引擎在启动时自己推一个事件会引入"UI 还没准备好收"的时序
    /// 问题——而那是可以被拉取模型完全避开的复杂度。
    ///
    /// ## 为什么**不**在这里要设置
    ///
    /// 设置的持久化属 US4（T030），引擎当前对 `SettingsLoad` 如实返回能力
    /// 错误。在启动时请求它，用户一打开应用就会看到一个错误提示——而那件
    /// 事（"四组默认还没落地"）与他此刻要做的事（读书）毫无关系，提示只会
    /// 制造困惑。
    ///
    /// 设置叠加层（T030）打开时才请求，那时提示出现在上下文中。
    /// `ViewState.settings` 的初值是 `UserSettings::default_ma()`——即
    /// MASTER §5.1 的设计默认值，故在此之前界面用的是正确的默认参数。
    ///
    /// 调用点在 `main.rs` 的创建闭包里（引擎已挂上、窗口已存在）。
    pub fn start(&mut self) {
        self.send(EngineRequest::ShelfLoad);
    }

    /// 从创建上下文构造（注册主题、UI 字体、渲染资源）。
    ///
    /// ## 为什么这些注册必须在创建期一次做完
    ///
    /// egui 的 `FontDefinitions` / `Style` 是**每帧读**的；每帧重建它们既慢
    /// 又会打断 IME 与字体图集缓存。创建期设一次，之后只在主题切换时重设。
    #[must_use]
    pub fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        let app = Self::with_state(ViewState::new());

        // 平台 UI 字体链（R7 的裁决：不内嵌，走系统栈）
        let mut fonts = egui::FontDefinitions::default();
        let report = crate::theme_bridge::register_ui_fonts(
            &mut fonts,
            crate::theme_bridge::Platform::current(),
        );
        if !report.loaded_any() {
            // 缺失不阻断（egui 自带拉丁字面），但**必须可见**：否则界面里的
            // 中文会显示成豆腐块，而症状看起来像"文本渲染坏了"。
            log::warn!(
                "tome-app: 未找到任何平台 UI 字体（已尝试 {:?}）——中文将显示为占位方框",
                report.missing
            );
        }
        creation_context.egui_ctx.set_fonts(fonts);

        let bridge = BridgeTheme::resolve(ThemeSource::FollowSystem, FollowSystem::system());
        bridge.apply(&creation_context.egui_ctx);

        app
    }
}

impl eframe::App for TomeApp {
    /// 逻辑段：**不可见时也跑**（`eframe` 的 0.36 形态）。
    ///
    /// ## 为什么单实例的**整条**处理都在这里（含 ViewportCommand）
    ///
    /// 初版把"轮询信号"放这里、把"发 `ViewportCommand`"放 `ui()`。那是个
    /// **只在最小化时才现形**的错误：
    ///
    /// - 窗口最小化时 `eframe` 走 `update_logic_only` 路径——`logic` 照跑，
    ///   但**不跑 UI pass**（0.36.0 起"没有东西要显示时不跑 egui pass"）。
    /// - 于是 `bring_to_front` 被置位却永远没有消费点，命令从不发出。
    /// - 而"用户在应用最小化时再点一次图标"**恰恰是单实例最常见的场景**
    ///   （他以为应用没开）——最需要它的时候它不工作。
    ///
    /// 本机实测抓到：第一次最小化后二次启动窗口被恢复，第二次同样操作却保持
    /// 最小化。**不一致**正是"依赖时序"的典型征兆，而不是"偶发失败"。
    ///
    /// 修法：命令直接从 `logic` 发——那里有 `ctx`，且 `run_logic` 会处理
    /// `logic_output.viewport_commands`（`epi_integration.rs` 的
    /// `update_logic_only` 明确读它）。于是可见/不可见两条路径都成立。
    ///
    /// ## 拖入文件为什么在这里处理（而不是在 `ui()`）
    ///
    /// `egui` 的 `dropped_files` 是**事件**：它只在发生的那一帧存在于
    /// `ctx.input()` 里。在 `ui()` 里读它意味着"那帧必须恰好是可见帧"，
    /// 而窗口最小化时拖入是不可能的（用户看不到窗口）——这条其实安全。
    /// 放在 `logic()` 的理由是它更早：UI pass 可能因为别的原因跳过（如
    /// 窗口刚被最小化），而事件不该因为"这帧没画"而丢失。
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 单实例：第二实例发来唤醒信号 → 立即把窗口带到前台
        if let Some(handle) = &self.single_instance {
            if handle.check_show() {
                self.bring_to_front = true;
                ctx.request_repaint();
            }
        }
        if std::mem::take(&mut self.bring_to_front) {
            bring_to_front(ctx);
        }

        // 拖入的本地书（FR-002 的"拖入"路径）
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        for file in dropped {
            // `DroppedFile` 是 trait（egui 把平台句柄的持有权留给集成层），
            // 故路径要经方法取——原生平台上 `path()` 返回绝对路径。
            let path = file.path();
            if path.as_os_str().is_empty() {
                continue;
            }
            self.state.begin_import();
            self.send(EngineRequest::ShelfImportLocal {
                path: path.display().to_string(),
            });
        }

        // 引擎事件：每帧取一次（UI 拉取模型，见 engine/thread.rs 的说明）
        if let Some(engine) = &self.engine {
            let mut produced = Vec::new();
            for event in engine.drain_events() {
                // 排版结果要留在 UI 侧（构造绘制批次用），但它不在状态机里
                // （状态机是纯值、可序列化，而排版结果是进程内产物）
                #[cfg(feature = "reader-render")]
                if let EngineEvent::ChapterReady {
                    laid_out: Some(layout),
                    ..
                } = &event
                {
                    self.laid_out = Some(Arc::clone(layout));
                }
                self.state.apply_event(event);
                // 状态变化可能产生后续请求（如进入阅读视图后要告知视口尺寸）
                produced.extend(self.state.take_requests());
            }
            for request in produced {
                engine.send(request);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 先取出视图与叠加层（都是 `Copy`），避免下面借用整个 `self.state`
        let view = self.state.view();
        let overlay = self.state.overlay();

        // 本帧的意图（视图里产生的引擎请求）在画完之后统一发出
        let mut requests: Vec<EngineRequest> = Vec::new();

        // --- 键盘：翻页 + Esc 返回书架（**Phase 3 的最小形态**）---
        //
        // ## 为什么现在就有它
        //
        // quickstart §1 的第 3 步是「读到中部 → `Esc` 回书架 → 重开 → 位置
        // 逐字符恢复」。"读到中部"要翻页，回书架要 Esc——两者缺一那条走查就
        // 只能改用鼠标，而走查脚本与实现不一致本身就是缺陷（它意味着"文档
        // 描述的操作做不到"）。
        //
        // ## 与 T029（`keys.rs` 全表）的关系
        //
        // 这是**占位**：T029 会把 `contracts/ui.md` §2 的整张表落地，届时
        // - 翻页键走 `tome_reader::keys` 的平台无关键表（含 `PgUp`/`PgDn` 同义）
        // - Esc 的语义是「关抽屉 → 再按返回书架」（`EscStack` 的分层）
        //
        // 故本段在 T029 落地时应被**整体替换**，不是并存——两处各自处理同一批
        // 键会打架（同一个按键被处理两次，表现为"翻一页跳两页"）。
        if overlay.is_none() && view == AppView::Reading {
            let (page_forward, page_back, escape) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::ArrowRight)
                        || input.key_pressed(egui::Key::PageDown),
                    input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::PageUp),
                    input.key_pressed(egui::Key::Escape),
                )
            });

            // 内容未就绪时不翻页（此时没有"页"可言，翻了也没反应——而
            // "按了没反应"比"明确忽略"更让人困惑，故这里显式只在就绪时处理）
            let ready = matches!(
                self.state.content(),
                crate::engine::messages::ContentState::Ready { .. }
            );
            if ready && page_forward {
                requests.push(EngineRequest::ReadingTurnPage { delta: 1 });
            }
            if ready && page_back {
                requests.push(EngineRequest::ReadingTurnPage { delta: -1 });
            }
            if escape {
                if let Some(flush) = self.state.back_to_shelf_request() {
                    requests.push(flush);
                }
                requests.push(EngineRequest::ReadingClose);
            }
        }

        {
            let state = &mut self.state;

            egui::CentralPanel::default().show(ui, |ui| match view {
                AppView::Shelf => requests.extend(crate::views::shelf::draw(ui, state)),
                AppView::Reading => {
                    // 渲染面（路径 A）只在 `reader-render` 下存在；headless
                    // 构建里 `Stage` 是 `None`，阅读视图退化为加载/错误态呈现
                    #[cfg(feature = "reader-render")]
                    let stage = Some(crate::views::reading::Stage {
                        laid_out: self.laid_out.as_deref(),
                        frames: &mut self.frames,
                        theme: BridgeTheme::from_theme(state.theme()),
                    });
                    #[cfg(not(feature = "reader-render"))]
                    let stage = None;

                    requests.extend(crate::views::reading::draw(ui, state, stage));
                }
            });

            if let Some(overlay) = overlay {
                requests.extend(crate::views::overlay::draw(ui, state, overlay));
            }
        }
        requests.extend(self.state.take_requests());
        for request in requests {
            self.send(request);
        }
    }
}

impl TomeApp {
    /// 发一个请求给引擎（无引擎时静默丢弃——那是"未接引擎的骨架形态"）。
    ///
    /// ## 为什么丢弃是安全的
    ///
    /// 无引擎时界面上的动作本来就无处可去（测试构造的 `TomeApp` 就是这样）。
    /// 这里**不**伪造事件、也**不** panic：一个没有引擎的外壳仍然应该是可
    /// 交互的（能翻视图、能开叠加层），只是数据永远不来。
    fn send(&self, request: EngineRequest) {
        if let Some(engine) = &self.engine {
            if !engine.send(request) {
                // 引擎线程已结束：这是**值得知道**的事（否则表现为"点了没反应"）。
                // 此处**只报"请求被丢弃"**，不打请求内容——请求里含书籍路径
                // 与搜索词，那是用户数据，不该进日志。
                log::error!("tome-app: 引擎线程已结束，请求被丢弃");
            }
        }
    }
}

/// 把窗口带到前台（三件套）。
///
/// ## Wayland 的降级
///
/// Wayland 协议**禁止**应用抢焦点（`app-single-instance` 的 README 亦明示）。
/// 那里只能用 `RequestUserAttention` 让任务栏闪一下——用户会看到提示，而
/// 窗口不会被硬拽到前面（那是协议的意图）。这条降级不是偷懒，是平台约束。
fn bring_to_front(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
        egui::UserAttentionType::Informational,
    ));
}

/// 让 `Arc` 的导入不被判为未使用（`ChapterReady.laid_out` 的载荷类型）。
#[allow(dead_code)]
fn laid_out_type_probe() -> Option<Arc<tome_reader::paginate::page::ChapterLayout>> {
    None
}
