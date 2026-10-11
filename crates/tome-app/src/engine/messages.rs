//! `[WIP]` 引擎消息协议（S3 T006；`data-model.md` §3）
//!
//! ## 这一层解决什么
//!
//! 外壳是**单窗口双视图**（书架 ⇄ 阅读），而 S1/S2 的能力全是同步阻塞调用
//! （SQLite、HTTP、排版，见 `research-api-surface.md` §B.1）。把它们放进
//! egui 的 UI 线程会让界面在抓取/排版时冻结——阅读器不可接受。
//!
//! 故分成两条线程，本模块是它们之间**唯一**的语言：
//!
//! ```text
//!   UI 线程（egui）               引擎线程（独占 Paginator/store/source）
//!        │  EngineRequest ────────────────►│
//!        │◄──────────────── EngineEvent    │
//! ```
//!
//! ## 为什么是"值"而不是共享状态
//!
//! 全部消息是 `Clone` 的纯值（`data-model.md` §1 的要求），线程间**零共享
//! 可变状态**。这条约束的收益具体而直接：状态机的断言可以在 headless 下
//! 构造消息值来测（不经 GUI、不经线程），而"两个线程同时改同一块内存"这类
//! 缺陷在类型层面就不存在。
//!
//! ## 为什么可序列化
//!
//! 见 `tests/messages.rs` 的模块文档（崩溃现场可重放 / 契约可机检 /
//! headless 断言）。**唯一例外**是 [`EngineEvent::ChapterReady`] 的
//! `laid_out`：排版结果是进程内渲染产物，跨进程无意义，故显式不参与序列化
//! 并有专门断言防止它被当成遗漏。

use serde::{Deserialize, Serialize};

// ===========================================================================
// §1 实体（`data-model.md` §1 的落地形态）
// ===========================================================================

/// 阅读锚点 `(chapter_index, char_offset)`（S1/S2 契约原样，不在本层重定义）。
///
/// ## 为什么不做成类型别名
///
/// `(usize, usize)` 的别名无法阻止"把 `(offset, chapter)` 写反"——而两者都是
/// `usize`，写反了编译器一声不吭。具名字段让那个错误变成类型错误。
///
/// 字段用 `i32`/`i64` 而非 `usize`：存储层（`ReadProgress`）就是这两个宽度，
/// 在接缝处对齐省掉一层无意义的转换与溢出讨论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReadingAnchor {
    /// 章序号（书内，0 起）
    pub chapter_index: i32,
    /// 章内字符偏移（0 起）
    pub char_offset: i64,
}

/// 阅读形态（`Paginated` / `Scroll`）。
///
/// 直接复用 `tome-reader` 的 [`ReadingMode`]：它是 S2 已验收的几何子集成员，
/// 本层另立一个同义枚举只会引入一处需要手工同步的映射。
pub use tome_reader::paginate::config::ReadingMode;

/// 单页 / 双页的**意图**（三态，与 S2 的 `SpreadMode` 同构）。
///
/// ## 为什么必须有 `Auto`（T009 修正 T006 的两态决策）
///
/// T006 曾把它做成两态（`Single` / `Spread`），理由是"`Auto` 由窗口宽度阈值
/// 自动触发而非用户选择"。那个理由**与设计系统矛盾**：MASTER §5.2 的关键行为
/// 第 3 条明写
///
/// > 用户可在工具栏**手动覆盖**为「自动 / 强制单页 / 强制双页」，手动选择在
/// > 本次会话内优先于自动判定，并按**语义回落**失效
///
/// 即 `Auto` 是一个**用户可见的选择项**，不是内部状态。两态模型的后果在
/// T009 的状态机测试里当场暴露：默认值只能是 `Single`，于是"默认配置在
/// 1512px 窗口下应允许双页"与"用户选了单页就不切双页"这两条断言**互相矛盾**
/// ——同一个值不可能既表示"未表态"又表示"表态为单页"。
///
/// 三态把这个歧义消灭在类型层：`Auto` = 未表态（宽度说了算），`Single` /
/// `Spread` = 表态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PageLayout {
    /// 自动（按 em 制宽度判据决定；MASTER §5.2 的默认行为）
    #[default]
    Auto,
    /// 强制单页
    Single,
    /// 强制双页（宽度容不下两栏时按语义回落，MASTER §5.2 第 3 条）
    Spread,
}

/// 主题选择（三态）。
///
/// ## 为什么 `FollowSystem` 必须与 `Day`/`Night` 并列
///
/// 设计系统没有"跟随系统"机制（R7：`tokens.css` 无 `prefers-color-scheme`），
/// 它是外壳的 **OS 探测 + 映射**。若把它解析成 `Day` 或 `Night` 再存，语义就
/// 丢了——用户下次启动时无法知道"当初选的是跟随"还是"当初选的就是昼"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// 昼：Parchment
    Day,
    /// 夜：Obsidian
    Night,
    /// 跟随系统（OS 探测后映射到昼/夜 token）
    #[default]
    FollowSystem,
}

/// 该书最近的阅读参数快照（FR-007b 的载体）。
///
/// ## 为什么与 `UserSettings` 分成两个类型
///
/// 两者字段高度重叠（主题/字号/行高/形态/单双页），但**语义不同**：
/// `UserSettings` 是"新书首次打开用什么"，本类型是"这本书上次用什么"。
/// FR-007b 的规则正是两者的关系（书级快照覆盖全局默认，全局改动不回写已读
/// 过的书）。合成一个类型会让那条规则无处安放。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReadingParams {
    /// 字号（px）
    pub font_size_px: f32,
    /// 行高（字号倍数）
    pub line_height: f32,
    /// 翻页 / 滚动
    pub mode: ReadingMode,
    /// 单页 / 双页
    pub page_layout: PageLayout,
    /// 昼 / 夜 / 跟随系统
    pub theme: ThemeChoice,
}

impl ReadingParams {
    /// MASTER §5.1 的设计默认值。
    ///
    /// 命名带 `_ma`（与 S2 的 `ReadingConfig::default_ma` 同形）：让"这些数字
    /// 有出处"在调用点可见。数值直接取自 `tome_core::model::theme::READING`，
    /// **不在本层重声明**（MASTER §10：禁止各控件散读常量）。
    #[must_use]
    pub fn default_ma() -> Self {
        let reading = tome_core::model::theme::READING;
        Self {
            font_size_px: reading.font_size_px,
            line_height: reading.line_height,
            mode: ReadingMode::Paged,
            page_layout: PageLayout::Auto,
            theme: ThemeChoice::FollowSystem,
        }
    }
}

/// 四组全局默认（FR-018）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UserSettings {
    /// 主题（昼 / 夜 / 跟随系统）
    pub theme: ThemeChoice,
    /// 默认字号（px）
    pub default_font_size_px: f32,
    /// 默认行高（字号倍数）
    pub default_line_height: f32,
    /// 默认阅读形态
    pub default_mode: ReadingMode,
    /// 默认单双页
    pub default_page_layout: PageLayout,
}

impl UserSettings {
    /// MASTER §5.1 的默认值（与 [`ReadingParams::default_ma`] 同源）。
    #[must_use]
    pub fn default_ma() -> Self {
        let reading = tome_core::model::theme::READING;
        Self {
            theme: ThemeChoice::FollowSystem,
            default_font_size_px: reading.font_size_px,
            default_line_height: reading.line_height,
            default_mode: ReadingMode::Paged,
            default_page_layout: PageLayout::Auto,
        }
    }

    /// 展开为某本书首次打开时使用的阅读参数（FR-007b 的"取全局默认"分支）。
    #[must_use]
    pub fn to_reading_params(self) -> ReadingParams {
        ReadingParams {
            font_size_px: self.default_font_size_px,
            line_height: self.default_line_height,
            mode: self.default_mode,
            page_layout: self.default_page_layout,
            theme: self.theme,
        }
    }
}

/// 书架条目（`data-model.md` §1.1）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShelfEntry {
    /// 书籍标识（S1 存储主键的规范化形式）
    pub book_id: String,
    /// 标题（非空；空标题由 S1 语义回退为文件名/书名）
    pub title: String,
    /// 作者
    pub author: String,
    /// 来源类别的**展示标签**（如「本地书」「合成源」）
    ///
    /// 存标签而非枚举：来源类别在 S1 侧是判别联合（本地路径 / 源 id + URL），
    /// 而书卡上只显示一个词。把它做成字符串让"标签怎么来"（查源名？查库根名？）
    /// 留给产生事件的引擎线程，UI 不必再查一次。
    pub origin_label: String,
    /// 封面引用（本地路径或图片缓存键；`None` 走 MASTER 占位形态）
    pub cover: Option<String>,
    /// 进度百分比（纯展示；0.0–100.0）
    pub progress_percent: f32,
    /// 最近阅读时刻（Unix 秒；从未打开为 `None`）
    pub last_read_at: Option<i64>,
}

/// 已导入书源的引用（`data-model.md` §1.4）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// 源标识
    pub source_id: String,
    /// 展示名
    pub name: String,
    /// 导入时刻（Unix 秒）
    pub imported_at: i64,
}

/// 一条搜索命中（`data-model.md` §1.5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// 书籍 URL（详情 / 加架的键）
    pub book_url: String,
    /// 书名
    pub title: String,
    /// 作者
    pub author: String,
    /// 封面 URL（有则显示）
    pub cover_url: Option<String>,
}

/// 一条书签（`data-model.md` §1.6）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookmarkEntry {
    /// 定位（与阅读进度同一锚点类型，FR-008d 同一路径）
    pub anchor: ReadingAnchor,
    /// 位置预览文本（锚点处截取，定长）
    pub preview: String,
    /// 创建时刻（Unix 秒）
    pub created_at: i64,
}

/// 错误视图（`data-model.md` §4）——引擎错误经消息协议抵达 UI 的形态。
///
/// ## 三个字段各有分工
///
/// | 字段 | 来源 | 用途 |
/// |---|---|---|
/// | `id` | `ErrorKind::message_id()` | 文案表查表键；**永不翻译**，随文案一并呈现 |
/// | `detail` | `ClassifiedError::detail()` | 开发者向诊断（日志、折叠区） |
/// | `capability` | `ErrorKind::UnsupportedCapability` | 能力标识，呈现为 `[capability: xxx]` |
///
/// ## 为什么不直接传 `ClassifiedError`
///
/// `ClassifiedError` 属 `tome-core`，本身可以传——但它**不可序列化**（core 的
/// DTO 刻意不派生 serde，见 core 的模块文档）。消息协议要求可序列化，故这里
/// 做一个显式的投影。收益是顺带的：投影让"外壳依赖 core 的哪些字段"变得可见。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedErrorView {
    /// 可本地化标识（`error.*` 形态）
    pub id: String,
    /// 诊断细节（开发者向；非面向用户文案）
    pub detail: Option<String>,
    /// 能力标识（仅能力错误有）
    pub capability: Option<String>,
}

impl ClassifiedErrorView {
    /// 从 `tome-core` 的分类错误投影。
    #[must_use]
    pub fn from_kind(kind: &tome_core::ErrorKind) -> Self {
        Self {
            id: kind.message_id().to_owned(),
            detail: None,
            capability: capability_of(kind),
        }
    }

    /// 从 `tome-core` 的**带细节**错误投影。
    #[must_use]
    pub fn from_classified(error: &tome_core::error::ClassifiedError) -> Self {
        Self {
            id: error.message_id().to_owned(),
            detail: error.detail().map(str::to_owned),
            capability: capability_of(error.kind()),
        }
    }

    /// 附上诊断细节（引擎侧在已知上下文时补）。
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// 取错误的能力标识（非能力错误返回 `None`）。
fn capability_of(kind: &tome_core::ErrorKind) -> Option<String> {
    match kind {
        tome_core::ErrorKind::UnsupportedCapability { capability } => {
            Some(capability.capability_id().to_owned())
        }
        _ => None,
    }
}

/// 内容三态（`data-model.md` §2 的 `ContentState`）。
///
/// ## 为什么 `Ready` 只带版本号、不带排版结果
///
/// 排版结果（`ChapterLayout`）是**进程内渲染产物**，不进可序列化的协议面
/// （见模块文档的例外说明）；它随 [`EngineEvent::ChapterReady`] 的
/// `laid_out` 字段单独抵达。`Ready` 本身只是"当前处于可阅读状态"的标记 +
/// 缓存失效键。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ContentState {
    /// 加载中（FR-010：不白屏）
    Loading,
    /// 就绪（`content_version` 单调递增，纹理缓存以它失效）
    Ready {
        /// 内容版本号
        content_version: u64,
    },
    /// 失败（必有可行动作，由视图层据 `error` 决定）
    Failed {
        /// 结构化错误
        error: ClassifiedErrorView,
    },
}

// ===========================================================================
// §3 引擎消息协议
// ===========================================================================

/// 一次搜索会话（`data-model.md` §2 的 `SearchSession`）。
///
/// ## 为什么 `generation` 在这里而不只在请求里
///
/// 请求里的 `generation` 是"这次搜索的编号"，本结构是"UI 侧当前的搜索状态"。
/// 两者必须同源：UI 收到结果时要比对"事件带的代际 == 我当前会话的代际"，
/// 才能丢弃过期结果（DM §2 不变量 4）。把当前代际存在会话里，比对才有基准。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSession {
    /// 本次搜索的代际号（全局单调）
    pub generation: u64,
    /// 关键词
    pub keyword: String,
    /// 是否已结束（全部源到达或已取消）
    pub done: bool,
}

/// UI → 引擎线程的请求（`data-model.md` §3 全量）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EngineRequest {
    // --- Shelf ---
    /// 加载书架全量
    ShelfLoad,
    /// 导入本地书
    ShelfImportLocal {
        /// 文件路径
        path: String,
    },
    /// 从书架移除（`purge_cache` 决定是否连带清理缓存）
    ShelfRemove {
        /// 书籍标识
        book_id: String,
        /// 是否清理该书的缓存内容（FR-004）
        purge_cache: bool,
    },

    // --- Reading ---
    /// 打开一本书（引擎据 FR-007b 决定用全局默认还是该书快照）
    ReadingOpen {
        /// 书籍标识
        book_id: String,
    },
    /// 关闭阅读视图（触发进度冲刷）
    ReadingClose,
    /// 调整阅读参数（FR-007 的五组）
    ReadingSetParams {
        /// 书籍标识
        book_id: String,
        /// 新参数
        params: ReadingParams,
    },
    /// 翻页（`delta` 为 ±1；章间接续由引擎处理，FR-006b）
    ReadingTurnPage {
        /// 页码增量
        delta: i32,
    },
    /// 跳到锚点（目录跳转 / 书签跳转走同一路径，FR-008d）
    ReadingSeekAnchor {
        /// 目标锚点
        anchor: ReadingAnchor,
    },
    /// 加载指定章
    ReadingLoadChapter {
        /// 书籍标识
        book_id: String,
        /// 章序号
        chapter_index: i32,
    },
    /// 阅读区可用尺寸（物理像素；UI 每帧按需告知）
    ///
    /// ## 为什么视口必须由 UI 告知（S3 T014 补入 `data-model.md` §3）
    ///
    /// 版心高度决定一页装几行，而"阅读区有多高"是**窗口几何**——只有 UI
    /// 知道（它扣掉工具栏、页脚与内边距之后才是净区）。引擎线程据此排版，
    /// 故它必须经协议抵达。
    ///
    /// 不加这个请求的替代方案是"UI 自己把整章行盒切成页"，但那会把 S2 的
    /// 分页语义复制到外壳（见 `engine/worker.rs` 的模块文档）——两份实现
    /// 一旦漂移，症状是"页脚说第 3 页、内容却是第 4 页"。
    ///
    /// 引擎侧对**未变化的尺寸去重**（egui 每帧都会报告），故重复发送是廉价的。
    ReadingViewport {
        /// 阅读区可用宽（物理像素）
        width_px: f32,
        /// 阅读区可用高（物理像素）
        height_px: f32,
    },

    // --- Toc ---
    /// 加载目录
    TocLoad {
        /// 书籍标识
        book_id: String,
    },

    // --- Bookmark ---
    /// 新增书签
    BookmarkAdd {
        /// 书籍标识
        book_id: String,
        /// 锚点
        anchor: ReadingAnchor,
        /// 预览文本
        preview: String,
    },
    /// 删除书签（按锚点定位，FR-008c）
    BookmarkRemove {
        /// 书籍标识
        book_id: String,
        /// 锚点
        anchor: ReadingAnchor,
    },
    /// 列出书签
    BookmarkList {
        /// 书籍标识
        book_id: String,
    },

    // --- Source ---
    /// 从文件导入书源
    SourceImportFile {
        /// 文件路径
        path: String,
    },
    /// 从 URL 导入书源
    SourceImportUrl {
        /// 书源 URL
        url: String,
    },
    /// 从文本导入书源（剪贴板）
    SourceImportText {
        /// 书源 JSON 文本
        text: String,
    },
    /// 发起搜索（`generation` 使过期结果自动作废，FR-013）
    SourceSearch {
        /// 搜索代际号
        generation: u64,
        /// 关键词
        keyword: String,
    },
    /// 取消搜索
    SourceCancelSearch {
        /// 要取消的代际号
        generation: u64,
    },
    /// 取书籍详情
    SourceDetail {
        /// 源标识
        source_id: String,
        /// 书籍 URL
        book_url: String,
    },
    /// 加入书架
    SourceAddToShelf {
        /// 源标识
        source_id: String,
        /// 书籍 URL
        book_url: String,
    },
    /// 换源（FR-016）
    SourceSwitch {
        /// 书籍标识
        book_id: String,
        /// 新源标识
        new_source_id: String,
    },

    // --- Progress ---
    /// 冲刷进度（节流窗口到点 / 退出前，FR-009）
    ProgressFlush {
        /// 书籍标识
        book_id: String,
    },

    // --- Settings ---
    /// 加载设置
    SettingsLoad,
    /// 保存设置（四组，FR-018）
    SettingsSave {
        /// 新设置
        settings: UserSettings,
    },
}

/// 引擎线程 → UI 的事件（`data-model.md` §3 全量）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EngineEvent {
    /// 书架加载完成
    ShelfLoaded {
        /// 全部条目
        entries: Vec<ShelfEntry>,
    },
    /// 单个条目变化（导入/进度更新）
    ShelfEntryChanged {
        /// 变化后的条目
        entry: ShelfEntry,
    },
    /// 导入失败（结构化原因，FR-002）
    ImportFailed {
        /// 结构化错误
        error: ClassifiedErrorView,
    },

    /// 章节就绪
    ChapterReady {
        /// 书籍标识
        book_id: String,
        /// 章序号
        chapter_index: i32,
        /// 排版结果（**进程内渲染产物**，不参与序列化；见模块文档）
        #[serde(skip)]
        laid_out: Option<std::sync::Arc<tome_reader::paginate::page::ChapterLayout>>,
        /// 内容版本号（单调递增）
        content_version: u64,
    },
    /// 章节失败（单章失败被隔离，spec 边缘情况）
    ChapterFailed {
        /// 书籍标识
        book_id: String,
        /// 章序号
        chapter_index: i32,
        /// 结构化错误
        error: ClassifiedErrorView,
    },

    /// 进度恢复（打开书时）
    ProgressRestored {
        /// 书籍标识
        book_id: String,
        /// 该书最近的参数快照（`None` = 首次打开，用全局默认）
        progress: Option<ReadingParams>,
        /// 恢复到的锚点
        anchor: ReadingAnchor,
    },
    /// 目录加载完成
    TocLoaded {
        /// 书籍标识
        book_id: String,
        /// 章标题列表（按序）
        toc: Vec<String>,
    },
    /// 书签列表
    Bookmarks {
        /// 书籍标识
        book_id: String,
        /// 书签列表
        list: Vec<BookmarkEntry>,
    },

    /// 一个源的结果到达（流式；按源分组呈现）
    SearchResult {
        /// 搜索代际号（UI 只呈现当前代际，DM §2 不变量 4）
        generation: u64,
        /// 命中的源
        source: SourceRef,
        /// 该源的命中列表
        hits: Vec<SearchHit>,
    },
    /// 搜索结束（全部源都已到达或已取消）
    SearchDone {
        /// 搜索代际号
        generation: u64,
    },
    /// 详情加载完成
    DetailLoaded {
        /// 源标识
        source_id: String,
        /// 书籍 URL
        book_url: String,
        /// 书名
        title: String,
        /// 作者
        author: String,
        /// 简介
        intro: String,
        /// 目录（章标题列表）
        toc: Vec<String>,
    },
    /// 已加入书架
    AddedToShelf {
        /// 新条目
        entry: ShelfEntry,
    },
    /// 换源完成（`anchor` 是映射后的新位置）
    SourceSwitched {
        /// 书籍标识
        book_id: String,
        /// 映射后的锚点
        anchor: ReadingAnchor,
    },

    /// 设置加载完成
    SettingsLoaded {
        /// 设置内容
        settings: UserSettings,
    },
    /// 保存成功
    Saved,

    /// 一次请求失败（**非**导入 / 章节类）
    ///
    /// ## 为什么需要这个事件（S3 T011 补入 `data-model.md` §3）
    ///
    /// 协议的既有事件里，失败只有两个出口：`ImportFailed`（导入）与
    /// `ChapterFailed`（章节）。但"设置保存失败""书签删除失败""书架移除
    /// 失败"既不是导入也不是章节——把它们的失败塞进那两个事件是**语义
    /// 撒谎**（界面会提示"导入失败"而用户在做的是移除）。
    ///
    /// 本变体是那个缺口的补丁：`request` 是请求的**变体名**（稳定标识，
    /// 不翻译、不含用户数据——与 `EnginePanic.context` 同一形态），
    /// `error` 是结构化错误。
    RequestFailed {
        /// 请求变体名（如 `ShelfRemove`）
        request: String,
        /// 结构化错误
        error: ClassifiedErrorView,
    },

    /// 引擎线程捕获到 panic（spec 边缘情况：引擎 panic 经外壳兜底）
    ///
    /// ## 为什么这个事件存在
    ///
    /// `panic = "unwind"`（S3 T003）让引擎线程能 `catch_unwind` 兜住单次请求的
    /// panic 而不终止进程。事件携带发生位置，UI 据此呈现结构化错误并**保持
    /// 可用**（线程存活，后续请求照常服务）。
    EnginePanic {
        /// 发生位置（阶段名，供定位）
        context: String,
    },
}

impl EngineEvent {
    /// 本事件属于哪个搜索代际（非搜索事件返回 `None`）。
    ///
    /// ## 为什么这个判断收在这里
    ///
    /// "只呈现当前 generation 的结果"（DM §2 不变量 4）需要 UI 侧做代际比对。
    /// 若每个视图各自 `match` 一遍，新增搜索类事件时必然漏掉一处。收成一个
    /// 方法后，"哪些事件受代际约束"只有一处答案。
    #[must_use]
    pub fn search_generation(&self) -> Option<u64> {
        match self {
            Self::SearchResult { generation, .. } | Self::SearchDone { generation } => {
                Some(*generation)
            }
            _ => None,
        }
    }
}
