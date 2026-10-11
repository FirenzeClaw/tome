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
    shelf: Vec<ShelfEntry>,
    shelf_filter: String,
    shelf_scroll: f32,
    shelf_sort_descending: bool,
    shelf_selected: Option<String>,

    // --- 阅读 ---
    book_id: String,
    anchor: ReadingAnchor,
    params: ReadingParams,
    content: ContentState,
    content_version: Option<u64>,
    current_chapter: i32,
    drawer: Option<DrawerTab>,
    geometry: ReadingGeometry,

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
            shelf: Vec::new(),
            shelf_filter: String::new(),
            shelf_scroll: 0.0,
            shelf_sort_descending: false,
            shelf_selected: None,
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
        }
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

    /// 设置过滤词。
    pub fn set_shelf_filter(&mut self, filter: String) {
        self.shelf_filter = filter;
    }

    /// 当前过滤词。
    #[must_use]
    pub fn shelf_filter(&self) -> &str {
        &self.shelf_filter
    }

    /// 记录书架滚动位置。
    pub fn set_shelf_scroll(&mut self, offset: f32) {
        self.shelf_scroll = offset;
    }

    /// 书架滚动位置。
    #[must_use]
    pub fn shelf_scroll(&self) -> f32 {
        self.shelf_scroll
    }

    /// 书架条目数（全量）。
    #[must_use]
    pub fn shelf_len(&self) -> usize {
        self.shelf.len()
    }

    /// 按标识取条目。
    #[must_use]
    pub fn shelf_entry(&self, book_id: &str) -> Option<&ShelfEntry> {
        self.shelf.iter().find(|entry| entry.book_id == book_id)
    }

    /// 过滤后的可见条目（FR-005 的标题子串匹配）。
    ///
    /// ## 排序为什么在这里做而不是让视图排
    ///
    /// 排序与过滤的**正交性**是一条断言（`sorting_and_filtering_are_orthogonal`）。
    /// 若视图各自 `sort` 一次，那条断言就只能测"某个视图的某个实现"。
    /// 收在这里，"过滤 + 排序"只有一处定义。
    #[must_use]
    pub fn visible_shelf_entries(&self) -> Vec<&ShelfEntry> {
        let needle = self.shelf_filter.trim();
        let mut visible: Vec<&ShelfEntry> = self
            .shelf
            .iter()
            .filter(|entry| needle.is_empty() || entry.title.contains(needle))
            .collect();
        if self.shelf_sort_descending {
            visible.sort_by(|left, right| right.title.cmp(&left.title));
        } else {
            visible.sort_by(|left, right| left.title.cmp(&right.title));
        }
        visible
    }

    /// 切换排序方向。
    pub fn toggle_shelf_sort(&mut self) {
        self.shelf_sort_descending = !self.shelf_sort_descending;
    }

    /// 选中的书卡。
    #[must_use]
    pub fn shelf_selected(&self) -> Option<&str> {
        self.shelf_selected.as_deref()
    }

    /// 选中书卡。
    pub fn select_shelf_entry(&mut self, book_id: Option<String>) {
        self.shelf_selected = book_id;
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
    pub fn open_overlay(&mut self, overlay: Overlay) {
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
                self.shelf = entries;
            }
            EngineEvent::ShelfEntryChanged { entry } => {
                match self
                    .shelf
                    .iter_mut()
                    .find(|existing| existing.book_id == entry.book_id)
                {
                    Some(existing) => *existing = entry,
                    None => self.shelf.push(entry),
                }
            }
            EngineEvent::ImportFailed { error } => {
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
                let _ = laid_out;
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
            EngineEvent::TocLoaded { .. } | EngineEvent::Bookmarks { .. } => {
                // 目录/书签列表在 T019/T020 落地；本任务只需状态机骨架。
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
    /// 引擎句柄（T010 起接入真实 worker；本任务只留位）
    engine: Option<crate::engine::thread::EngineHandle>,
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

        // 引擎事件：每帧取一次（UI 拉取模型，见 engine/thread.rs 的说明）
        if let Some(engine) = &self.engine {
            for event in engine.drain_events() {
                self.state.apply_event(event);
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 先取出视图与叠加层（都是 `Copy`），避免下面借用整个 `self.state`
        let view = self.state.view();
        let overlay = self.state.overlay();
        let state = &mut self.state;

        egui::CentralPanel::default().show(ui, |ui| match view {
            AppView::Shelf => draw_shelf(ui, state),
            AppView::Reading => draw_reading(ui, state),
        });

        if let Some(overlay) = overlay {
            draw_overlay(ui, state, overlay);
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

/// 书架视图（**骨架**；T012 落地虚拟化网格与书卡）。
///
/// 本任务只保证：空态有引导动作（contracts/ui.md V1 的契约断言），
/// 有书时列出标题——足够验证"骨架 wired 到主题"。
fn draw_shelf(ui: &mut egui::Ui, state: &mut ViewState) {
    if state.shelf_len() == 0 {
        // MASTER §7 的 EmptyState：插画位 + 标题 + 说明 + 主操作（禁止只有一行灰字）
        //
        // 两个按钮都**真的动作**（打开对应的导入叠加层）——契约 V1 要求空态
        // "必有下一步动作引导"，一个点了没反应的按钮不算引导。
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.heading("书架是空的");
            ui.label("把一本 TXT / EPUB 拖进窗口，或用下面的按钮选择文件。");
            ui.add_space(16.0);
            if ui.button("导入本地书").clicked() {
                state.open_overlay(Overlay::ImportLocal);
            }
            if ui.button("导入书源").clicked() {
                state.open_overlay(Overlay::ImportSource);
            }
        });
        return;
    }

    let visible: Vec<String> = state
        .visible_shelf_entries()
        .iter()
        .map(|entry| {
            format!(
                "{} · {} · {:.0}%",
                entry.title, entry.author, entry.progress_percent
            )
        })
        .collect();

    for line in visible {
        ui.label(line);
    }
}

/// 阅读视图（**骨架**；T014 落地 PaintCallback 嵌入）。
fn draw_reading(ui: &mut egui::Ui, state: &mut ViewState) {
    // 先克隆内容态：下面要按它分支，同时闭包里还要可变借用 `state`
    // （`ContentState` 是纯值，克隆它比"把借用拆开"更直白）。
    let content = state.content().clone();
    match &content {
        ContentState::Loading => {
            ui.centered_and_justified(|ui| {
                ui.label("正在加载…");
            });
        }
        ContentState::Ready { content_version } => {
            ui.centered_and_justified(|ui| {
                // T014 起这里换成 S2 渲染的版心（PaintCallback）
                ui.label(format!(
                    "第 {} 章 · 第 {} 字（内容版本 {content_version}）",
                    state.current_chapter() + 1,
                    state.anchor().char_offset
                ));
            });
        }
        ContentState::Failed { error } => {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading("这一章打不开");
                ui.label(format!("[{}]", error.id));
                if let Some(detail) = &error.detail {
                    ui.label(detail);
                }
                ui.add_space(12.0);
                // 错误态的两个动作都真的动作（contracts/ui.md R3：错误必有可行动作）
                ui.horizontal(|ui| {
                    if ui.button("重试").clicked() {
                        // T014 起：重发 ReadingLoadChapter。骨架阶段先清除错误态，
                        // 让用户能看到"确实响应了"，而不是点了没反应。
                        state.dismiss_error();
                    }
                    if ui.button("返回书架").clicked() {
                        state.back_to_shelf();
                    }
                });
            });
        }
    }
}

/// 叠加层（骨架：设置 / 导入）。
fn draw_overlay(ui: &mut egui::Ui, state: &mut ViewState, overlay: Overlay) {
    let mut close = false;
    egui::Window::new(match overlay {
        Overlay::Settings => "设置",
        Overlay::ImportLocal => "导入本地书",
        Overlay::ImportSource => "导入书源",
    })
    .collapsible(false)
    .resizable(false)
    .show(ui.ctx(), |ui| {
        if overlay == Overlay::Settings {
            ui.label(format!("主题：{:?}", state.settings().theme));
            ui.label(format!(
                "默认字号：{}px",
                state.settings().default_font_size_px
            ));
        } else {
            ui.label("（T013 / T024 落地导入路径）");
        }
        ui.add_space(12.0);
        if ui.button("关闭").clicked() {
            close = true;
        }
    });

    if close {
        state.close_overlay();
    }
}

/// 让 `Arc` 的导入不被判为未使用（`ChapterReady.laid_out` 的载荷类型）。
#[allow(dead_code)]
fn laid_out_type_probe() -> Option<Arc<tome_reader::paginate::page::ChapterLayout>> {
    None
}
