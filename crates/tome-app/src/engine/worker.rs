//! `[WIP]` 生产引擎 worker（S3 T011/T013/T014/T015 的引擎侧）
//!
//! ## 这一层是什么
//!
//! [`crate::engine::thread`] 提供的是**线程与兜底的骨架**（T007），它只知道
//! "有个 `EngineWorker` 在跑"。本模块是那个 worker 的**生产实现**：它独占
//! `Paginator`（排版）、`BookRepository`（存储接缝）与本地书的内容提供者，
//! 把 [`EngineRequest`] 翻成这些能力上的一次次调用，再把结果翻成
//! [`EngineEvent`]。
//!
//! ```text
//!   UI 线程                    引擎线程（本模块）              下游
//!   ────────                   ──────────────────             ────
//!   ShelfLoad        ────►      load_shelf()        ────►     BookRepository
//!   ShelfImportLocal ────►      import_local()      ────►     tome-local 解析 → 落库
//!   ReadingOpen      ────►      open_book()         ────►     目录 + 进度 + 本地源
//!   ReadingViewport  ────►      relayout()          ────►     Paginator
//!   ReadingTurnPage  ────►      turn_page()         ────►     锚点 → 页 → 重排视图
//!   ProgressFlush    ────►      flush_progress()    ────►     BookRepository
//! ```
//!
//! ## `laid_out` 到底承载什么（一个必须写清楚的解释）
//!
//! `data-model.md` §3 把 [`EngineEvent::ChapterReady::laid_out`] 定义为
//! `Arc<ChapterLayout>`。协议刻意**不携带渲染类型**（`RenderBatch` 在
//! `render` feature 下，而 headless 构建也要能跑通整条消息链），所以 UI 侧
//! 拿到的必须是"从行盒出发就能构造批次"的东西。
//!
//! 本模块放进 `laid_out` 的是**当前这一页（或这一屏）的排版视图**：`lines`
//! 是本页的行盒，其余字段（`char_len` / `measure_px` / `columns` /
//! `rule_exceptions` / `split_diagnostic` / `degraded_words`）取整章的真实值。
//!
//! ### 为什么不是"整章行盒 + 页码"
//!
//! 那会迫使 UI 自己把整章行盒切成页——而"按可用高切页"正是 S2 的分页语义
//! （FR-020/021 把两种形态钉在同一份行盒序列上）。在 UI 侧重算一遍等于把
//! 分页规则复制到外壳，两份实现一旦漂移，症状是"页脚说第 3 页、内容却是
//! 第 4 页"——极难归因。**切页只发生在引擎线程**，UI 只画它拿到的行。
//!
//! ## 线程纪律（本模块是它的落点）
//!
//! `Paginator` / store / provider **只出现在本模块的字段里**，而本模块的
//! 实例被 `move` 进引擎线程后就不再离开（`EngineHandle` 拿不到它）。
//! 这条不是约定：`AppEngine` 不出现在任何 pub 签名里。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tome_core::model::{Book, BookOrigin, Chapter, ReadProgress, TxtTocRule};
use tome_core::seam::{BookRepository, ChapterContent, ContentProvider};
use tome_core::{ErrorCapability, ErrorKind};
use tome_local::format::LocalFormat;
use tome_local::provider::{Cleaner, LocalContentProvider, LocalSource};
use tome_reader::paginate::config::{GeometryConfig, ReadingMode, SpreadMode};
use tome_reader::paginate::page::{layout_chapter, ChapterLayout, PaginationResult, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_rule::pipeline::{
    ContentPipeline, DictRule as PipelineDictRule, HighlightRule as PipelineHighlightRule,
    ReplaceRule as PipelineReplaceRule,
};

use super::messages::{
    ClassifiedErrorView, EngineEvent, EngineRequest, ReadingAnchor, ReadingParams, ShelfEntry,
};
use super::thread::EngineWorker;
use crate::state::progress::{flush_window, MonotonicClock, ProgressTracker};

/// 本地书的文件大小上限（与 `tome_local::library::MAX_BOOK_BYTES` 同口径）。
///
/// ## 为什么要在导入前先看大小
///
/// 解析一个 2GB 的文件会把整份内容读进内存（TXT 路径尤其如此，`LocalSource`
/// 会整本缓存）。在**读之前**拒绝，比读完再发现内存不够要好——后者的表现是
/// 进程被系统杀掉，用户看到的是"点了导入，应用没了"。
const MAX_LOCAL_BOOK_BYTES: u64 = 1024 * 1024 * 1024;

/// 锚点语义校验文本的长度（`ReadProgress.anchor_text`）。
///
/// 与 001 的锚点契约同口径：存锚点处的一小段文本，供"内容变了之后锚点还能
/// 不能对上"的判断使用（书签锚点失效时按 S2 钳制语义定位，spec 边缘情况）。
const ANCHOR_TEXT_CHARS: usize = 32;

/// worker 内部的失败载体。
///
/// ## 为什么不直接用 `ErrorKind`
///
/// 分类（`ErrorKind`）是**契约**，而 worker 的每个失败点都带上下文（哪个文件、
/// 哪一章、哪个字段）。细节不携带的话，用户看到的"内容无法解析"就无从排查。
///
/// ## 为什么不直接用 `ClassifiedError`
///
/// `tome-core` 的那个类型**没有"无细节"的公开构造**（`ErrorKind::with_detail`
/// 必带 detail），而 worker 里有大量"原样透传下游分类错误"的路径——那些错误
/// 自己可能没有细节。用一个 `Option` 承载让两种情况都自然表达，而不是给它们
/// 编一个空串（空串会在界面上渲染成一行空白）。
struct Failure {
    /// 分类（上层按它分支）
    kind: ErrorKind,
    /// 诊断细节（无则 `None`）
    detail: Option<String>,
}

impl Failure {
    /// 带诊断细节的失败。
    fn new(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: Some(detail.into()),
        }
    }

    /// 投影成消息协议的错误视图。
    fn view(&self) -> ClassifiedErrorView {
        let mut view = ClassifiedErrorView::from_kind(&self.kind);
        view.detail = self.detail.clone();
        view
    }
}

impl From<ErrorKind> for Failure {
    fn from(kind: ErrorKind) -> Self {
        Self { kind, detail: None }
    }
}

impl From<tome_core::error::ClassifiedError> for Failure {
    fn from(error: tome_core::error::ClassifiedError) -> Self {
        Self {
            kind: error.kind().clone(),
            detail: error.detail().map(str::to_owned),
        }
    }
}

impl From<tome_local::LocalError> for Failure {
    fn from(error: tome_local::LocalError) -> Self {
        Self {
            kind: ErrorKind::Content,
            detail: Some(error.to_string()),
        }
    }
}

impl From<tome_reader::error::ReaderError> for Failure {
    fn from(error: tome_reader::error::ReaderError) -> Self {
        // 排版层已经做了错误分类（`kind()`），故**不**在这里重新分类——
        // 重分类会让"能力缺失"这类可捕获错误退化成"内容错误"
        Self {
            kind: error.kind(),
            detail: error.detail().map(str::to_owned),
        }
    }
}

/// 生产引擎 worker。
///
/// ## 为什么字段全是"独占"的
///
/// `Paginator` 的所有方法要 `&mut self`（它持 `FontSystem`），`Paginator`
/// 因此不能被两个线程共享。把它放进一个**只在本线程存在**的结构体，比
/// `Arc<Mutex<Paginator>>` 更省：后者每次调用都要过一次锁，而锁的争用方
/// 只有 UI 线程——那条路径本来就不该存在（UI 不排版）。
pub struct AppEngine {
    /// 存储接缝（书架/章节/进度/书签/规则）
    repository: Arc<dyn BookRepository + Send + Sync>,
    /// 净化规则来源（每次取——用户改了规则不必重启，见 `Cleaner` 的文档）
    cleaner: Arc<dyn Cleaner>,
    /// 排版器（跨请求复用同一个字体栈）
    paginator: Paginator,
    /// 单调时钟（节流窗口的判据）
    clock: MonotonicClock,
    /// 当前阅读会话
    session: Option<Session>,
    /// 阅读区可用尺寸（由 UI 每帧按需告知；`None` = 还不知道，暂不排版）
    viewport: Option<Viewport>,
    /// 内容版本号（单调递增；纹理缓存以它失效）
    content_version: u64,
    /// 进度记忆（节流落盘）
    progress: Option<ProgressTracker>,
    /// 本地书的内容提供者缓存（`book_id` → 已打开的源）
    ///
    /// ## 为什么要缓存
    ///
    /// `LocalSource` 对 TXT 会整本缓存文件内容（见其模块文档）。每次翻章都
    /// 重开一次文件、重读一遍 100MB，是纯粹的浪费。缓存按 `book_id` 键控，
    /// 换书时不清（同一会话里来回换书很常见），进程退出时随 worker 一起释放。
    local_sources: HashMap<String, Arc<LocalSource>>,
}

/// 一次阅读会话的全部状态。
struct Session {
    /// 当前书
    book: Book,
    /// 目录（章标题 + 定位信息）
    chapters: Vec<Chapter>,
    /// 正文来源（本地书走 `LocalContentProvider`）
    provider: Arc<dyn ContentProvider + Send + Sync>,
    /// 当前章序号
    chapter_index: i32,
    /// 当前章正文（**已净化**）
    content: String,
    /// 当前章的分页结果（切页只在引擎线程发生，见模块文档）
    ///
    /// `None` = 尚未排版（视口尺寸未知）。用 `Option` 而不是空结果表达这件事：
    /// "没有页"与"排版出来是零页"是两回事，前者该等，后者该报（S2 保证空正文
    /// 也产出一页，故零页只可能是缺陷）。
    pagination: Option<PaginationResult>,
    /// 阅读锚点（**唯一的定位真相**，FR-017/SC-008）
    anchor: ReadingAnchor,
}

impl AppEngine {
    /// 以存储接缝构造。
    ///
    /// `Paginator::new()` **不失败**（内嵌字体装载失败时降级到系统字体栈，
    /// 见其文档），故本函数不返回 `Result`。
    #[must_use]
    pub fn new(repository: Arc<dyn BookRepository + Send + Sync>) -> Self {
        let cleaner: Arc<dyn Cleaner> = Arc::new(DaoCleaner {
            repository: Arc::clone(&repository),
        });
        Self {
            repository,
            cleaner,
            paginator: Paginator::new(),
            clock: MonotonicClock::new(),
            session: None,
            viewport: None,
            content_version: 0,
            progress: None,
            local_sources: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // 书架
    // -----------------------------------------------------------------------

    /// 书架全量（`ShelfLoaded`）。
    fn load_shelf(&self) -> Result<Vec<ShelfEntry>, Failure> {
        let books = self.repository.bookshelf()?;
        let mut entries = Vec::with_capacity(books.len());
        for book in books {
            entries.push(self.shelf_entry_of(&book)?);
        }
        Ok(entries)
    }

    /// 一本书 → 书架条目。
    ///
    /// ## 进度百分比的口径
    ///
    /// `(已完成章数 + 章内位置) / 总章数`。章内位置**取不到**（要读该章正文
    /// 才知道字符总数），故这里用"当前章序号 / 总章数"——它的含义是"读到第
    /// 几章"，与书卡上那个百分比的说服力一致，且**不需要读正文**（1000 册
    /// 的书架上，为算百分比而读 1000 次正文是不可接受的）。
    fn shelf_entry_of(&self, book: &Book) -> Result<ShelfEntry, Failure> {
        let progress = self.repository.get_progress(book.id)?;
        let total_chapters = book.total_chapters.unwrap_or(0).max(0);
        let percent = match (&progress, total_chapters) {
            (Some(read), total) if total > 0 => {
                (f32::from(read.chapter_index.clamp(0, total) as i16) / f32::from(total as i16))
                    * 100.0
            }
            _ => 0.0,
        };

        Ok(ShelfEntry {
            book_id: book.id.to_string(),
            title: book.name.clone(),
            author: book.author.clone().unwrap_or_default(),
            // 来源标签存**稳定标识**而非中文（与 `error.xxx` 同一条纪律：
            // 引擎侧不产自然语言，文案映射在 i18n）
            origin_label: match book.origin {
                BookOrigin::Local => ORIGIN_LOCAL.to_owned(),
                BookOrigin::Network => ORIGIN_NETWORK.to_owned(),
            },
            cover: book.cover_url.clone(),
            progress_percent: percent,
            last_read_at: progress.map(|read| read.updated_at),
        })
    }

    /// 导入本地书（FR-002）。
    ///
    /// ## 不留半成品（spec 边缘情况）
    ///
    /// 顺序是**先解析、后落库**：解析失败时库里什么都没发生（不会出现"有书
    /// 无章"的条目）。落库分两步（`add_book` → `save_chapters`），第二步失败
    /// 时把第一步的书删掉——否则会留下一个"打开没有内容"的卡片。
    fn import_local(&mut self, path: &str) -> Result<ShelfEntry, Failure> {
        let path = Path::new(path);

        // --- ① 预检：存在 + 大小 ---
        let metadata = std::fs::metadata(path).map_err(|error| {
            Failure::new(
                ErrorKind::Configuration,
                format!("无法读取 {}：{error}", path.display()),
            )
        })?;
        if metadata.len() > MAX_LOCAL_BOOK_BYTES {
            return Err(Failure::new(
                ErrorKind::Content,
                format!(
                    "文件 {:.1} GB，超过单本上限 {} GB",
                    metadata.len() as f64 / (1024.0 * 1024.0 * 1024.0),
                    MAX_LOCAL_BOOK_BYTES / (1024 * 1024 * 1024)
                ),
            ));
        }

        // --- ② 解析（失败即返回，库中无痕）---
        let bytes = std::fs::read(path)
            .map_err(|error| Failure::new(ErrorKind::Content, format!("读取失败：{error}")))?;
        let parsed = self.parse_local(path, &bytes)?;

        // --- ③ 重复导入识别为同一条目（spec 边缘情况）---
        let existing = self
            .repository
            .bookshelf()?
            .into_iter()
            .find(|book| book.origin_id == parsed.book.origin_id);

        let book_id = match existing {
            Some(existing) => {
                let mut merged = parsed.book;
                merged.id = existing.id;
                merged.added_at = existing.added_at;
                self.repository.update_book(&merged)?;
                merged.id
            }
            None => {
                let mut fresh = parsed.book;
                fresh.added_at = now_seconds();
                self.repository.add_book(&fresh)?
            }
        };

        // --- ④ 落章（失败则回滚书条目，不留半成品）---
        let chapters = parsed.chapters;
        if let Err(error) = self.repository.save_chapters(book_id, &chapters) {
            if let Err(rollback) = self.repository.remove_book(book_id) {
                // 回滚也失败：**不能吞**。如实报出两件事，用户至少知道
                // 库里有残留，而不是看到"导入失败"却发现书架上多了一本。
                return Err(Failure::new(
                    ErrorKind::Storage,
                    format!("章节写入失败（{error:?}），且回滚书条目也失败（{rollback:?}）"),
                ));
            }
            return Err(Failure::from(error));
        }

        // --- ⑤ 返回新条目 ---
        let book = self
            .repository
            .bookshelf()?
            .into_iter()
            .find(|candidate| candidate.id == book_id)
            .ok_or_else(|| {
                Failure::new(ErrorKind::Storage, "导入后未能在书架上找到该书".to_owned())
            })?;
        self.shelf_entry_of(&book)
    }

    /// 按格式解析本地文件（三种格式的公共形状）。
    fn parse_local(&self, path: &Path, bytes: &[u8]) -> Result<ParsedLocal, Failure> {
        let format = tome_local::format::detect(path, bytes)
            .map_err(|error| Failure::new(ErrorKind::Content, error.to_string()))?;

        match format {
            LocalFormat::Txt => {
                let rules = self.txt_toc_rules()?;
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let parsed = tome_local::txt::import_bytes(bytes, &file_name, &rules);
                Ok(self.finish_parsed(
                    path,
                    parsed.book,
                    tome_local::txt::to_chapters(&parsed.toc, 0),
                ))
            }
            LocalFormat::Epub => {
                let parsed = tome_local::epub::import_bytes(bytes)
                    .map_err(|error| Failure::new(ErrorKind::Content, error.to_string()))?;
                Ok(self.finish_parsed(
                    path,
                    parsed.book,
                    tome_local::epub::to_chapters(&parsed.toc, 0),
                ))
            }
            LocalFormat::Mobi => {
                let parsed = tome_local::mobi::parse(bytes)
                    .map_err(|error| Failure::new(ErrorKind::Content, error.to_string()))?;
                Ok(self.finish_parsed(
                    path,
                    parsed.book,
                    tome_local::mobi::to_chapters(&parsed.toc, 0),
                ))
            }
            // KFX / Topaz 是**显式拒绝**的格式（S1 语义），如实报能力缺失
            LocalFormat::Kfx | LocalFormat::Topaz => Err(Failure::new(
                ErrorKind::unsupported_capability(ErrorCapability::new("localFormat")),
                format!("暂不支持 {} 格式", format.as_str()),
            )),
        }
    }

    /// 补齐落库前的公共字段。
    ///
    /// ## 路径为什么用 `normalize_for_storage`
    ///
    /// Windows 的反斜杠若原样写进库，数据目录搬到 Linux 后路径全部失效
    /// （001 FR-042 的"可随程序搬移"）。归一化是**写库前的强制步骤**，
    /// 不是可选的整洁动作。
    fn finish_parsed(&self, path: &Path, mut book: Book, chapters: Vec<Chapter>) -> ParsedLocal {
        book.relative_path = Some(tome_local::library::normalize_for_storage(path));
        // 拖入单个文件时不在任何库根之下（FR-023 的库根语义），故为 `None`；
        // 此时 `relative_path` 承载绝对路径
        book.library_root_id = None;
        book.total_chapters = Some(chapters.len() as i32);
        ParsedLocal { book, chapters }
    }

    /// TXT 目录识别规则：用户配置优先，空则回退到默认集。
    ///
    /// ## 为什么默认集要在外壳里
    ///
    /// 规则是**用户的配置**（存在库里），但用户还没配置任何规则时，导入一本
    /// 普通的中文 TXT 也应当能识别出目录——否则"导入"对绝大多数文件只产出
    /// 机械分页（每 3000 字一章），那不是一个能用的书架。
    ///
    /// 这组默认值与 `tome-cli` 的 `local::default_txt_rules` 同源（同一批
    /// 社区惯例写法）。放在外壳而不是 `tome-local` 的理由是：它是**策略**
    /// （试哪几种写法）而非解析能力，而策略属于组合根。
    fn txt_toc_rules(&self) -> Result<Vec<TxtTocRule>, Failure> {
        let configured = self.repository.txt_toc_rules()?;
        if configured.iter().any(|rule| rule.enabled) {
            return Ok(configured);
        }
        Ok(default_txt_rules())
    }

    /// 从书架移除（FR-004）。
    fn remove_from_shelf(&self, book_id: &str, purge_cache: bool) -> Result<(), Failure> {
        let id = parse_book_id(book_id)?;
        self.repository.remove_book(id)?;
        if purge_cache {
            // 缓存清理由接缝的 `clear_cache` 承担；单本粒度的清理在 adapter 里
            // 随 `remove_book` 一并做（FR-004 的"其缓存内容按 S1 语义清理"）。
            // 这里再显式清一次是**幂等**的，代价可忽略，换来的是"调用方说清
            // 就清"这条语义不依赖 adapter 的实现细节。
            self.repository
                .clear_cache(tome_core::seam::CacheKind::Chapter)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 阅读
    // -----------------------------------------------------------------------

    /// 打开一本书（FR-003：恢复上次位置）。
    fn open_book(&mut self, book_id: &str) -> Result<Vec<EngineEvent>, Failure> {
        let id = parse_book_id(book_id)?;
        let book = self
            .repository
            .bookshelf()?
            .into_iter()
            .find(|candidate| candidate.id == id)
            .ok_or_else(|| Failure::new(ErrorKind::Storage, format!("书架上没有 id={id} 的书")))?;

        let chapters = self.repository.chapters(id)?;
        if chapters.is_empty() {
            return Err(Failure::new(
                ErrorKind::Content,
                "这本书没有可读的章节".to_owned(),
            ));
        }

        let provider = self.provider_for(&book, &chapters)?;
        let progress = self.repository.get_progress(id)?;
        let anchor = progress.as_ref().map_or(
            ReadingAnchor {
                chapter_index: 0,
                char_offset: 0,
            },
            |read| ReadingAnchor {
                chapter_index: read.chapter_index,
                char_offset: read.char_offset,
            },
        );

        // 进度基线取**恢复到的位置**（见 `ProgressTracker::open` 的说明）
        self.progress = Some(ProgressTracker::open(book_id, anchor, self.clock.now_ms()));

        let toc: Vec<String> = chapters
            .iter()
            .map(|chapter| chapter.title.clone())
            .collect();

        let start_chapter = anchor.chapter_index.clamp(0, chapters.len() as i32 - 1);
        self.session = Some(Session {
            book,
            chapters,
            provider,
            chapter_index: start_chapter,
            content: String::new(),
            // 等 `ReadingViewport` 到达后才排版（见 `layout_current`）
            pagination: None,
            anchor: ReadingAnchor {
                chapter_index: start_chapter,
                char_offset: anchor.char_offset,
            },
        });

        let mut events = vec![
            EngineEvent::ProgressRestored {
                book_id: book_id.to_owned(),
                // 本书的参数快照（FR-007b）属 US2（T017）：本切片一律按全局
                // 默认打开，故为 `None`
                progress: None,
                anchor: self
                    .session
                    .as_ref()
                    .map_or(anchor, |session| session.anchor),
            },
            EngineEvent::TocLoaded {
                book_id: book_id.to_owned(),
                toc,
            },
        ];

        events.extend(self.layout_current(book_id)?);
        Ok(events)
    }

    /// 按 `Book` 造内容提供者。
    ///
    /// ## 为什么网络书在这里是能力错误而不是"以后再补"
    ///
    /// `ContentProvider` 的**网络实现**要注入 `SourceLookup` + `NetHost` +
    /// 章节缓存（组合根职责），那是 US3（T023-T027）的范围。本切片如实返回
    /// 结构化能力错误——书源能力缺席不影响本地书闭环（US1 的独立测试口径）。
    fn provider_for(
        &mut self,
        book: &Book,
        chapters: &[Chapter],
    ) -> Result<Arc<dyn ContentProvider + Send + Sync>, Failure> {
        match book.origin {
            BookOrigin::Local => {
                let path = self.resolve_local_path(book)?;
                let key = book.id.to_string();
                if !self.local_sources.contains_key(&key) {
                    let source = LocalSource::open(&path, chapters.to_vec())
                        .map_err(|error| Failure::new(ErrorKind::Content, error.to_string()))?;
                    self.local_sources.insert(key.clone(), Arc::new(source));
                }
                let source = self
                    .local_sources
                    .get(&key)
                    .ok_or_else(|| Failure::new(ErrorKind::Storage, "本地源缓存缺失".to_owned()))?;
                Ok(Arc::new(LocalContentProvider::new(
                    Arc::clone(source),
                    Arc::clone(&self.cleaner),
                )))
            }
            BookOrigin::Network => Err(Failure::new(
                ErrorKind::unsupported_capability(ErrorCapability::new("networkContent")),
                "网络书的正文加载属 US3（书源获取闭环）",
            )),
        }
    }

    /// 从 `Book` 恢复本地文件路径。
    ///
    /// 两种形态（001 的库根语义）：
    /// - 在库根之下 → `library_root_id` + `relative_path`
    /// - 库根之外的单个文件（拖入）→ `library_root_id` 为 `None`，
    ///   `relative_path` 就是绝对路径
    fn resolve_local_path(&self, book: &Book) -> Result<PathBuf, Failure> {
        let relative = book.relative_path.as_deref().ok_or_else(|| {
            Failure::new(ErrorKind::Storage, format!("《{}》缺少文件路径", book.name))
        })?;

        match book.library_root_id {
            None => Ok(PathBuf::from(relative)),
            Some(root_id) => {
                let root = self
                    .repository
                    .library_roots()?
                    .into_iter()
                    .find(|candidate| candidate.id == root_id)
                    .ok_or_else(|| {
                        Failure::new(ErrorKind::Storage, format!("库根 {root_id} 不存在"))
                    })?;
                Ok(Path::new(&root.path).join(relative))
            }
        }
    }

    /// 排版当前章并产出 `ChapterReady`（**切页的唯一入口**）。
    ///
    /// ## 为什么"不知道视口就不排版"
    ///
    /// 版心高度决定一页装几行。在不知道阅读区尺寸时排版，得到的页边界是错的
    /// ——而那会直接表现为"页脚说 3 页，翻两下就到头了"。故 `viewport` 未知
    /// 时本函数**不产出事件**，等 UI 发来尺寸（`ReadingViewport`）再排。
    fn layout_current(&mut self, book_id: &str) -> Result<Vec<EngineEvent>, Failure> {
        let Some(viewport) = self.viewport else {
            return Ok(Vec::new());
        };
        let geometry = geometry_for(self.params());

        let Some(session) = self.session.as_mut() else {
            return Ok(Vec::new());
        };

        // 正文按需取（首次打开、或跳到别的章时）
        if session.content.is_empty() {
            let content = session
                .provider
                .chapter_content(&session.book, session.chapter_index)?;
            session.content = content.content;
        }

        let pagination = self
            .paginator
            .paginate(&session.content, &geometry, &viewport)?;
        session.pagination = Some(pagination);

        self.content_version += 1;
        let version = self.content_version;
        let (laid_out, chapter_index) = page_view(session, version)?;

        Ok(vec![EngineEvent::ChapterReady {
            book_id: book_id.to_owned(),
            chapter_index,
            laid_out: Some(Arc::new(laid_out)),
            content_version: version,
        }])
    }

    /// 当前阅读参数（本切片恒为 MASTER 默认；书级快照属 US2/T017）。
    fn params(&self) -> ReadingParams {
        ReadingParams::default_ma()
    }

    /// 翻页（`delta` 为 ±1；越界则接续相邻章，FR-006b）。
    fn turn_page(&mut self, delta: i32) -> Result<Vec<EngineEvent>, Failure> {
        let Some(session) = self.session.as_ref() else {
            return Ok(Vec::new());
        };
        let book_id = session.book.id.to_string();

        let Some(pagination) = session.pagination.as_ref() else {
            // 还没排版（视口未知）：翻页无事可做。**不报错**——用户可能只是
            // 在首帧到达之前按了方向键，那不是错误。
            return Ok(Vec::new());
        };
        let current = pagination.resolve_anchor(session.anchor.char_offset.max(0) as usize);
        let page_count = pagination.pages.len();

        let Some(resolution) = current else {
            return Ok(Vec::new());
        };
        let current_index = resolution.target.page_index().unwrap_or(0);
        let target = current_index as i64 + i64::from(delta);

        // --- 章内翻页 ---
        if target >= 0 && (target as usize) < page_count {
            let page = &pagination.pages[target as usize];
            let offset = page.char_range.start as i64;
            self.seek(ReadingAnchor {
                chapter_index: session.chapter_index,
                char_offset: offset,
            })?;
            return self.layout_current(&book_id);
        }

        // --- 章间接续（FR-006b）---
        let next_chapter = session.chapter_index + delta.signum();
        if next_chapter < 0 || next_chapter as usize >= session.chapters.len() {
            // 已经是全书首/末页：钳制在当前页，不报错（用户按到边界不是错误）
            return Ok(Vec::new());
        }
        let offset = if delta > 0 {
            0
        } else {
            // 往前翻到上一章的**末页首页字符**：先加载该章再定位
            i64::MAX
        };
        self.load_chapter(&book_id, next_chapter, offset)
    }

    /// 跳到锚点（目录跳转 / 书签跳转走同一路径，FR-008d）。
    fn seek(&mut self, anchor: ReadingAnchor) -> Result<Vec<EngineEvent>, Failure> {
        let Some(session) = self.session.as_mut() else {
            return Ok(Vec::new());
        };
        session.anchor = anchor;
        if let Some(tracker) = &mut self.progress {
            tracker.observe(anchor);
        }
        Ok(Vec::new())
    }

    /// 加载指定章（`offset` 为章内目标偏移；`i64::MAX` 表示"章末"）。
    fn load_chapter(
        &mut self,
        book_id: &str,
        chapter_index: i32,
        offset: i64,
    ) -> Result<Vec<EngineEvent>, Failure> {
        let Some(session) = self.session.as_mut() else {
            return Ok(Vec::new());
        };
        if chapter_index < 0 || chapter_index as usize >= session.chapters.len() {
            return Ok(Vec::new());
        }

        session.chapter_index = chapter_index;
        session.content.clear();
        session.anchor = ReadingAnchor {
            chapter_index,
            char_offset: 0,
        };
        if let Some(tracker) = &mut self.progress {
            tracker.observe(session.anchor);
        }

        // 先排版拿到章长，再按 offset 落点（章末用 `i64::MAX` 表示，这里
        // 依赖 `resolve_anchor` 的钳制语义）
        let events = self.layout_current(book_id)?;
        if offset == i64::MAX {
            let last_start = self
                .session
                .as_ref()
                .and_then(|session| session.pagination.as_ref())
                .and_then(|pagination| pagination.pages.last())
                .map_or(0, |page| page.char_range.start);
            self.seek(ReadingAnchor {
                chapter_index,
                char_offset: last_start as i64,
            })?;
            return self.layout_current(book_id);
        }
        if offset > 0 {
            self.seek(ReadingAnchor {
                chapter_index,
                char_offset: offset,
            })?;
            return self.layout_current(book_id);
        }
        Ok(events)
    }

    /// 关闭阅读视图（退出前冲刷进度，FR-009）。
    fn close_reading(&mut self) -> Vec<EngineEvent> {
        let mut events = self.flush_progress();
        self.session = None;
        self.progress = None;
        events.push(EngineEvent::Saved);
        events
    }

    /// 冲刷进度（节流窗口到点 / 退出前）。
    fn flush_progress(&mut self) -> Vec<EngineEvent> {
        let Some(tracker) = &self.progress else {
            return Vec::new();
        };
        let Some((book_id, anchor)) = tracker.take_pending() else {
            return Vec::new();
        };
        let Ok(id) = parse_book_id(&book_id) else {
            return Vec::new();
        };

        let anchor_text = self
            .session
            .as_ref()
            .filter(|session| session.anchor == anchor)
            .and_then(|session| anchor_text_at(&session.content, anchor.char_offset));

        let progress = ReadProgress {
            book_id: id,
            chapter_index: anchor.chapter_index,
            char_offset: anchor.char_offset,
            anchor_text,
            updated_at: now_seconds(),
        };

        match self.repository.save_progress(&progress) {
            Ok(()) => {
                // 只有**写成功**才清脏（写失败要保持脏，下次到点重试）
                if let Some(tracker) = &mut self.progress {
                    tracker.mark_flushed(self.clock.now_ms());
                }
                Vec::new()
            }
            Err(error) => {
                // 写失败：保持"脏"（下次重试）但**推后一整个窗口**——否则
                // `due()` 立刻又为真，定时唤醒退化成忙循环（见 `defer` 的说明）
                if let Some(tracker) = &mut self.progress {
                    tracker.defer(self.clock.now_ms());
                }
                vec![EngineEvent::RequestFailed {
                    request: "ProgressFlush".to_owned(),
                    error: Failure::from(error).view(),
                }]
            }
        }
    }

    /// 按请求构造错误事件（能力缺失 / 未落地的动作）。
    fn unsupported(&self, request: &str, capability: &str) -> Vec<EngineEvent> {
        let error = Failure::from(ErrorKind::unsupported_capability(ErrorCapability::new(
            capability,
        )));
        vec![EngineEvent::RequestFailed {
            request: request.to_owned(),
            error: error.view(),
        }]
    }
}

/// 本地解析结果（三种格式归一）。
struct ParsedLocal {
    /// 书（`id` 尚未分配）
    book: Book,
    /// 章（`book_id` 为 0，落库时由存储层改写）
    chapters: Vec<Chapter>,
}

/// 把一页的行盒装成 `ChapterLayout`（见模块文档"`laid_out` 到底承载什么"）。
///
/// ## 为什么先 `seek` 再取
///
/// 锚点是**唯一的定位真相**：翻页、跳章、目录跳转都只改锚点，页序由锚点
/// 反查（`resolve_anchor` 的钳制语义顺带处理了"锚点越界"这一 spec 边缘
/// 情况）。若反过来（先定页再回写锚点），两处状态就有不一致的窗口。
fn page_view(session: &Session, version: u64) -> Result<(ChapterLayout, i32), Failure> {
    let pagination = session.pagination.as_ref().ok_or_else(|| {
        Failure::new(ErrorKind::Content, "排版结果缺失（视口未知时不应走到这里）")
    })?;
    let char_offset = session.anchor.char_offset.max(0) as usize;
    let resolution = pagination
        .resolve_anchor(char_offset)
        .ok_or_else(|| Failure::new(ErrorKind::Content, "排版结果里找不到锚点所在的页"))?;
    let page_index = resolution.target.page_index().unwrap_or(0);
    let page = pagination
        .pages
        .get(page_index)
        .ok_or_else(|| Failure::new(ErrorKind::Content, format!("页序号 {page_index} 越界")))?;

    let _ = version;
    Ok((
        ChapterLayout {
            lines: page.lines.clone(),
            char_len: pagination.char_len,
            measure_px: pagination.spread.measure_px,
            columns: pagination.spread.columns,
            override_active: pagination.spread.override_active,
            rule_exceptions: pagination.rule_exceptions.clone(),
            split_diagnostic: pagination.split_diagnostic.clone(),
            degraded_words: pagination.degraded_words,
        },
        session.chapter_index,
    ))
}

/// 按参数与视口构造排版几何。
///
/// ## 为什么这里**强制** `reading_mode: Paged`
///
/// 滚动形态的呈现（滚动事件节流、视口重排、连续滚过章末）属 US2 的
/// T018/T021。本切片只走翻页路径，故几何里的形态恒为 `Paged`——把
/// `params.mode` 原样透进去会让 `paginate` 收到一个与调用路径不符的形态，
/// 而那会产出无法解释的页边界。
fn geometry_for(params: ReadingParams) -> GeometryConfig {
    GeometryConfig {
        font_size_px: params.font_size_px.round().clamp(1.0, f32::from(u16::MAX)) as u16,
        line_height: params.line_height,
        spread_mode: match params.page_layout {
            crate::engine::messages::PageLayout::Auto => SpreadMode::Auto,
            crate::engine::messages::PageLayout::Single => SpreadMode::ForceSingle,
            crate::engine::messages::PageLayout::Spread => SpreadMode::ForceSpread,
        },
        reading_mode: ReadingMode::Paged,
        ..GeometryConfig::default()
    }
}

/// 锚点处的语义校验文本（定长）。
fn anchor_text_at(content: &str, char_offset: i64) -> Option<String> {
    if char_offset < 0 {
        return None;
    }
    let start = usize::try_from(char_offset).ok()?;
    let slice: String = content
        .chars()
        .skip(start)
        .take(ANCHOR_TEXT_CHARS)
        .collect();
    if slice.trim().is_empty() {
        None
    } else {
        Some(slice)
    }
}

/// 书架条目里的本地书来源标识（文案映射在 `i18n`）。
pub const ORIGIN_LOCAL: &str = "local";
/// 书架条目里的网络书来源标识。
pub const ORIGIN_NETWORK: &str = "network";

/// `book_id` 是**十进制字符串**（`ShelfEntry.book_id` 的类型是 `String`）。
fn parse_book_id(book_id: &str) -> Result<i64, Failure> {
    book_id.parse::<i64>().map_err(|_| {
        Failure::new(
            ErrorKind::Storage,
            format!("书籍标识不是合法的主键：{book_id}"),
        )
    })
}

/// 当前 Unix 秒（失败退化为 0）。
///
/// 系统时钟早于 1970 时返回 0——那是一个明显不合理的值，比 panic 好，也比
/// "用 `unwrap` 让整个导入失败"好（时间戳不影响书能否读）。
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64)
}

/// TXT 目录识别的默认规则集（见 [`AppEngine::txt_toc_rules`]）。
fn default_txt_rules() -> Vec<TxtTocRule> {
    vec![
        TxtTocRule {
            id: 0,
            name: "第X章/节/回".to_owned(),
            rule: r"^\s*第\s*[0-9一二三四五六七八九十百千零两]+\s*[章节回卷篇].*$".to_owned(),
            example: Some("第一章 开端".to_owned()),
            enabled: true,
            sort_order: 0,
        },
        TxtTocRule {
            id: 0,
            name: "括注编号".to_owned(),
            rule: r"^\s*【[^】]{1,30}】\s*$".to_owned(),
            example: Some("【壹】开端".to_owned()),
            enabled: true,
            sort_order: 1,
        },
        TxtTocRule {
            id: 0,
            name: "卷/序/番外".to_owned(),
            rule: r"^\s*(卷[0-9一二三四五六七八九十百千零两]+|序章|序言|楔子|尾声|后记|番外).*$"
                .to_owned(),
            example: Some("楔子".to_owned()),
            enabled: true,
            sort_order: 2,
        },
        TxtTocRule {
            id: 0,
            name: "数字编号".to_owned(),
            rule: r"^\s*[0-9]{1,4}[.、,，]\s*\S.*$".to_owned(),
            example: Some("1. 起点".to_owned()),
            enabled: true,
            sort_order: 3,
        },
    ]
}

/// 净化规则来源：每次取，故用户改了规则不必重启（见 `Cleaner` 的文档）。
struct DaoCleaner {
    /// 存储接缝（四类规则的读入口）
    repository: Arc<dyn BookRepository + Send + Sync>,
}

impl Cleaner for DaoCleaner {
    fn pipeline(&self) -> ContentPipeline {
        // 取规则失败时退化为**空链**而不是报错：净化是"锦上添花"的一步，
        // 它失败不该让整章读不出来。空链的语义明确（三步都跳过），
        // 比"取一半规则"更可预期。
        let replace = self
            .repository
            .replace_rules()
            .unwrap_or_default()
            .iter()
            .map(translate_replace_rule)
            .collect();
        let highlight = self
            .repository
            .highlight_rules()
            .unwrap_or_default()
            .iter()
            .map(translate_highlight_rule)
            .collect();
        let dict = self
            .repository
            .dict_rules()
            .unwrap_or_default()
            .iter()
            .map(translate_dict_rule)
            .collect();
        ContentPipeline::new()
            .with_replace_rules(replace)
            .with_highlight_rules(highlight)
            .with_dict_rules(dict)
    }
}

/// `tome-core` 的替换规则行 → 加工链的替换规则。
///
/// ## 为什么必须有这三层翻译（而不能直接传）
///
/// 存储层的规则行与加工链的规则是**两个不同的类型**，字段并不一一对应：
///
/// | 存储行（`tome-core`） | 加工链（`tome-rule`） | 差异 |
/// |---|---|---|
/// | `id: i64` | `id: String` | 加工链的诊断标识要**人可读** |
/// | `name` / `scope` | — | 加工链不关心名字与作用域 |
/// | `sort_order: i32` | `sort_order: i64` | 宽度不同 |
///
/// 它们不共享类型是有道理的：存储行是**用户配置的持久化形态**（含 UI 用的
/// 名字与作用域），加工链规则是**执行形态**（只留执行需要的东西）。合成一个
/// 类型会让存储 schema 的每次调整都波及规则引擎。
///
/// ## 为什么 `id` 要拼上名字
///
/// 加工链的 `id` 只出现在**诊断**里（"这条替换是哪来的"）。纯数字主键在日志
/// 里不可读，拼上规则名之后一眼能对上用户配置里那条。这是 `tome-source` 的
/// 净化器已定的形态（同一条纪律：诊断标识人可读）。
///
/// ## 为什么 `scope` 不在这里过滤
///
/// 作用域过滤（"这条替换只作用于正文/只作用于标题"）需要**目标类型**，而
/// `Cleaner::pipeline()` 没有它——它只产出"当前配置对应的加工链"。本地书的
/// provider 拿到链后按自己的目标跑（`LocalContentProvider` 的行为）。
fn translate_replace_rule(rule: &tome_core::model::ReplaceRule) -> PipelineReplaceRule {
    PipelineReplaceRule {
        id: format!("#{} {}", rule.id, rule.name),
        pattern: rule.pattern.clone(),
        replacement: rule.replacement.clone(),
        is_regex: rule.is_regex,
        enabled: rule.enabled,
        sort_order: i64::from(rule.sort_order),
    }
}

/// `tome-core` 的高亮规则行 → 加工链的高亮规则。
fn translate_highlight_rule(rule: &tome_core::model::HighlightRule) -> PipelineHighlightRule {
    PipelineHighlightRule {
        id: format!("#{} {}", rule.id, rule.name),
        pattern: rule.pattern.clone(),
        style: rule.style.clone(),
        enabled: rule.enabled,
        sort_order: i64::from(rule.sort_order),
    }
}

/// `tome-core` 的词典规则行 → 加工链的词典规则。
fn translate_dict_rule(rule: &tome_core::model::DictRule) -> PipelineDictRule {
    PipelineDictRule {
        id: format!("#{} {}", rule.id, rule.name),
        url_rule: rule.url_rule.clone(),
        content_rule: rule.content_rule.clone(),
        enabled: rule.enabled,
    }
}

impl EngineWorker for AppEngine {
    fn handle(&mut self, request: EngineRequest) -> Vec<EngineEvent> {
        match request {
            EngineRequest::ShelfLoad => match self.load_shelf() {
                Ok(entries) => vec![EngineEvent::ShelfLoaded { entries }],
                Err(error) => vec![EngineEvent::RequestFailed {
                    request: "ShelfLoad".to_owned(),
                    error: error.view(),
                }],
            },

            EngineRequest::ShelfImportLocal { path } => match self.import_local(&path) {
                Ok(entry) => vec![EngineEvent::ShelfEntryChanged { entry }],
                Err(error) => vec![EngineEvent::ImportFailed {
                    error: error.view(),
                }],
            },

            EngineRequest::ShelfRemove {
                book_id,
                purge_cache,
            } => match self.remove_from_shelf(&book_id, purge_cache) {
                Ok(()) => match self.load_shelf() {
                    Ok(entries) => vec![EngineEvent::ShelfLoaded { entries }],
                    Err(error) => vec![EngineEvent::RequestFailed {
                        request: "ShelfRemove".to_owned(),
                        error: error.view(),
                    }],
                },
                Err(error) => vec![EngineEvent::RequestFailed {
                    request: "ShelfRemove".to_owned(),
                    error: error.view(),
                }],
            },

            EngineRequest::ReadingOpen { book_id } => match self.open_book(&book_id) {
                Ok(events) => events,
                Err(error) => vec![EngineEvent::ChapterFailed {
                    book_id,
                    chapter_index: 0,
                    error: error.view(),
                }],
            },

            EngineRequest::ReadingClose => self.close_reading(),

            EngineRequest::ReadingViewport {
                width_px,
                height_px,
            } => {
                let next = Viewport::new(width_px, height_px);
                // 尺寸没变就不重排：egui 每帧都会报告尺寸，不去重的话每帧
                // 都会触发一次整章重排（而重排是几十毫秒级的）
                if self.viewport == Some(next) || width_px <= 0.0 || height_px <= 0.0 {
                    return Vec::new();
                }
                self.viewport = Some(next);
                let Some(book_id) = self.session.as_ref().map(|s| s.book.id.to_string()) else {
                    return Vec::new();
                };
                match self.layout_current(&book_id) {
                    Ok(events) => events,
                    Err(error) => vec![EngineEvent::ChapterFailed {
                        book_id,
                        chapter_index: 0,
                        error: error.view(),
                    }],
                }
            }

            EngineRequest::ReadingSetParams { book_id, params } => {
                let _ = params;
                // 参数快照与锚点保持的完整语义属 US2（T017）；本切片只保证
                // 收到调整后重排一次（否则界面会停在不匹配的版心上）
                match self.layout_current(&book_id) {
                    Ok(events) => events,
                    Err(error) => vec![EngineEvent::ChapterFailed {
                        book_id,
                        chapter_index: 0,
                        error: error.view(),
                    }],
                }
            }

            EngineRequest::ReadingTurnPage { delta } => match self.turn_page(delta) {
                Ok(events) => events,
                Err(error) => self.chapter_failure("ReadingTurnPage", error),
            },

            EngineRequest::ReadingSeekAnchor { anchor } => {
                let Some(book_id) = self.session.as_ref().map(|s| s.book.id.to_string()) else {
                    return Vec::new();
                };
                let current = self.session.as_ref().map(|s| s.chapter_index);
                if current == Some(anchor.chapter_index) {
                    if let Err(error) = self.seek(anchor) {
                        return self.chapter_failure("ReadingSeekAnchor", error);
                    }
                    match self.layout_current(&book_id) {
                        Ok(events) => events,
                        Err(error) => self.chapter_failure("ReadingSeekAnchor", error),
                    }
                } else {
                    match self.load_chapter(&book_id, anchor.chapter_index, anchor.char_offset) {
                        Ok(events) => events,
                        Err(error) => self.chapter_failure("ReadingSeekAnchor", error),
                    }
                }
            }

            EngineRequest::ReadingLoadChapter {
                book_id,
                chapter_index,
            } => match self.load_chapter(&book_id, chapter_index, 0) {
                Ok(events) => events,
                Err(error) => self.chapter_failure("ReadingLoadChapter", error),
            },

            EngineRequest::TocLoad { book_id } => match parse_book_id(&book_id)
                .and_then(|id| self.repository.chapters(id).map_err(Failure::from))
            {
                Ok(chapters) => vec![EngineEvent::TocLoaded {
                    book_id,
                    toc: chapters.into_iter().map(|chapter| chapter.title).collect(),
                }],
                Err(error) => vec![EngineEvent::RequestFailed {
                    request: "TocLoad".to_owned(),
                    error: error.view(),
                }],
            },

            EngineRequest::ProgressFlush { .. } => self.flush_progress(),

            EngineRequest::BookmarkAdd { .. }
            | EngineRequest::BookmarkRemove { .. }
            | EngineRequest::BookmarkList { .. } => {
                // 书签属 US2（T020）
                self.unsupported("Bookmark", "bookmarks")
            }

            EngineRequest::SourceImportFile { .. }
            | EngineRequest::SourceImportUrl { .. }
            | EngineRequest::SourceImportText { .. }
            | EngineRequest::SourceSearch { .. }
            | EngineRequest::SourceCancelSearch { .. }
            | EngineRequest::SourceDetail { .. }
            | EngineRequest::SourceAddToShelf { .. }
            | EngineRequest::SourceSwitch { .. } => {
                // 书源薄层属 US3（T023-T027）
                self.unsupported("Source", "sourceEngine")
            }

            EngineRequest::SettingsLoad | EngineRequest::SettingsSave { .. } => {
                // 四组全局默认属 US4（T030）
                self.unsupported("Settings", "settings")
            }
        }
    }

    /// 定时唤醒：进度节流窗口到点时落盘（FR-009 / SC-009）。
    ///
    /// ## 为什么定时工作必须在引擎线程上
    ///
    /// "窗口到点"这件事的状态（脏标记 + 上次落盘时刻）只存在于引擎线程。
    /// 让 UI 线程定期发一个"该落盘了吗"的请求，等于把状态复制一份到 UI
    /// ——两份状态就有不一致的窗口。返回剩余时间让线程**恰好**在窗口到点
    /// 时醒来，丢失量就是窗口本身（而不是窗口 + 轮询间隔）。
    fn next_wakeup(&self) -> Option<std::time::Duration> {
        self.progress
            .as_ref()
            .and_then(|tracker| tracker.next_deadline_in(self.clock.now_ms()))
    }

    fn on_wakeup(&mut self) -> Vec<EngineEvent> {
        if self
            .progress
            .as_ref()
            .is_some_and(|tracker| tracker.due(self.clock.now_ms()))
        {
            return self.flush_progress();
        }
        Vec::new()
    }
}

impl AppEngine {
    /// 章节类失败事件（带当前书与章号）。
    fn chapter_failure(&self, request: &str, error: Failure) -> Vec<EngineEvent> {
        let Some(session) = self.session.as_ref() else {
            return vec![EngineEvent::RequestFailed {
                request: request.to_owned(),
                error: error.view(),
            }];
        };
        vec![EngineEvent::ChapterFailed {
            book_id: session.book.id.to_string(),
            chapter_index: session.chapter_index,
            error: error.view(),
        }]
    }
}

/// 节流窗口的对外口径（供 UI 侧显示"多久落一次盘"与测试引用）。
#[must_use]
pub fn progress_flush_window() -> std::time::Duration {
    flush_window()
}

/// 让 `ContentProvider` 的方法名在此处可见（`chapter_content` 的类型推导）。
const _: fn(&dyn ContentProvider, &Book, i32) -> Result<ChapterContent, ErrorKind> =
    |provider, book, index| provider.chapter_content(book, index);

/// `layout_chapter` 的可用性断言（滚动形态落地时会用到它）。
#[allow(dead_code)]
fn layout_chapter_is_available(
    paginator: &mut Paginator,
    text: &str,
    geometry: &GeometryConfig,
) -> bool {
    let viewport = Viewport::new(600.0, 800.0);
    layout_chapter(paginator, text, geometry, &viewport).is_ok()
}
