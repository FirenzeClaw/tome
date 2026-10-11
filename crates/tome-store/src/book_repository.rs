//! \[DONE] `BookRepository` 接缝的生产实现（tasks.md 003-T010 / `contracts/seams.md` §3）
//!
//! # 这个模块是什么
//!
//! 把 `tome-core` 定义的 [`BookRepository`] 接到本 crate 的既有能力上：各表
//! DAO、Pack 章节缓存、图片缓存、备份读写器、FTS 书内搜索索引、书源适配器。
//! 它**不含业务逻辑**——每个方法都是一次转发，唯一三处"多做一点"各有自己的
//! 方法文档说明（[`SqliteBookRepository::remove_book`] 的缓存/索引清理、
//! `search_in_book` 的幂等建表、`import_sources` 的解析委托）。
//!
//! # 为什么接缝的实现落在 tome-store
//!
//! 接缝 trait 定义在 `tome-core`（依赖方向：实现 crate → core），而"数据在
//! 哪"这个知识只有存储层有：SQLite 归 actor 线程独占、章节正文在 Pack 文件里、
//! 图片在 `cache/images/`。把这些知识搬到上层，等于让每个调用方自己拼 SQL 与
//! 路径——那正是接缝要消除的东西。
//!
//! # 为什么是"转发"而不是"重新实现一遍"
//!
//! DAO 的方法各自带着已固化的约束（身份唯一性、`is_read` 不被目录重抓覆盖、
//! 缓存写入顺序、错误分类）。在此处另写一套查询就产生了**第二个定义点**：
//! 某天 DAO 改了约束，接缝这边不会跟着变，而两者的差异只在特定数据下才显形。
//! 故本模块只做转发与错误分类映射（[`seam_kind`]），不碰 SQL。
//!
//! # 书源渠道导入为什么需要一个注入点
//!
//! `import_sources` 的三种渠道（文件文本 / URL / 粘贴文本）都要先**解析**成
//! `Source`，而解析与"URL 渠道要发请求"都在 `tome-source`——本 crate **不得**
//! 依赖它（`tome-store → {tome-core, tome-net}`，加一条边即成环）。故这一步
//! 由组合根实现 [`SourceImporter`] 并注入；未注入时接缝**如实返回能力错误**
//! （FR-006），而不是把"做不到"报成"导入 0 条"。

use std::sync::Arc;

use tome_core::seam::{
    BackupJson, BookRepository, CacheKind, HighlightRuleRow, ImportReport, SearchHit,
    SourceImportChannel,
};
use tome_core::{
    Book, Bookmark, Chapter, DictRule, ErrorCapability, ErrorKind, Highlight, LibraryRoot, Memo,
    ReadProgress, ReadRecord, ReplaceRule, Source, SourceGroup, TxtTocRule,
};

use crate::backup::BackupStore;
use crate::dao::{BookDao, MiscDao, SourceDao, TraceDao};
use crate::datadir::DataDir;
use crate::db::Db;
use crate::error::StoreError;
use crate::fts::CachedTextIndex;
use crate::image_cache::ImageCache;
use crate::pack::PackStore;
use crate::source_store::{SourceStore, SOURCE_EXPORT_FILE_NAME};

/// 未注入 [`SourceImporter`] 时报告的能力标识。
///
/// 用书源生态里通用的叫法，让上层的提示文案与书源的 `try/catch` 降级逻辑
/// 都能按同一个字符串判别（FR-006）。
const SOURCE_IMPORT_CAPABILITY: &str = "sourceImport";

/// 书源渠道导入的委托（组合根注入）。
///
/// `tome-store` 不得依赖 `tome-source`（依赖方向），故"渠道 → 已解析书源"
/// 这一步由组合根实现并注入；未注入时接缝如实返回能力错误，不伪造成功。
pub trait SourceImporter: Send + Sync {
    /// 渠道（文件文本 / URL / 粘贴文本）→ 已解析书源。
    fn load(&self, channel: &SourceImportChannel) -> Result<Vec<Source>, ErrorKind>;
}

/// 接缝处的统一错误映射：细节留在本 crate，分类进接缝（FR-044）。
///
/// 单独一个函数而不是每处写 `|error| error.to_error_kind()`：映射只该有
/// **一个**定义点，否则日后加一类错误就要在几十个调用点里找齐。
fn seam_kind(error: StoreError) -> ErrorKind {
    error.to_error_kind()
}

/// `BookRepository` 的 SQLite 实现。
///
/// # 为什么持一堆 DAO 而不是一个 `Db`
///
/// 每个 DAO 只覆盖自己那几张表（书/章节、痕迹、规则、书源），方法名即语义。
/// 直接持 `Db` 则本模块要自己拼每一条 SQL——于是"哪些表参与"变成散落在
/// 几十个方法里的字符串，而约束（唯一性、级联、编码转换）要靠人记住。
///
/// 各 DAO 都是 `Clone` 且内部共享同一个 `Db`（actor 通道），故各存一份不
/// 增加连接数，也不引入并发问题。
pub struct SqliteBookRepository {
    book_dao: BookDao,
    trace_dao: TraceDao,
    misc_dao: MiscDao,
    source_store: SourceStore,
    backup_store: BackupStore,
    pack_store: Arc<PackStore>,
    image_cache: ImageCache,
    text_index: CachedTextIndex,
    source_importer: Option<Arc<dyn SourceImporter>>,
}

impl SqliteBookRepository {
    /// [DONE] 以已打开的库与数据目录构造。
    ///
    /// # 为什么收 `&DataDir` 而不是几个路径
    ///
    /// Pack 缓存、图片缓存、书源导出文件的落点都由数据目录布局决定
    /// （`data-model.md`：`cache/packs/`、`cache/images/`）。收 `DataDir` 让
    /// "布局只有一处定义"成立——若这里各自 `join("cache")`，布局一变就会
    /// 出现"新的写到新位置、旧的还在老位置"，表现为缓存时有时无。
    pub fn new(db: Db, data_dir: &DataDir) -> Self {
        let pack_store = Arc::new(PackStore::new(&data_dir.packs_dir(), db.clone()));
        let images_dir = data_dir.images_dir();

        Self {
            book_dao: BookDao::new(db.clone()),
            trace_dao: TraceDao::new(db.clone()),
            misc_dao: MiscDao::new(db.clone()),
            source_store: SourceStore::new(
                SourceDao::new(db.clone()),
                data_dir.root().join(SOURCE_EXPORT_FILE_NAME),
            ),
            backup_store: BackupStore::new(db.clone()),
            // 带 pack 的索引：检索时能校验"内容还在"，从而把缓存淘汰留下的
            // 悬空条目顺手清掉（见 `CachedTextIndex::search_in_book` 的说明）。
            text_index: CachedTextIndex::with_pack(db.clone(), Arc::clone(&pack_store)),
            pack_store,
            image_cache: ImageCache::new(&images_dir, db),
            source_importer: None,
        }
    }

    /// [DONE] 注入书源渠道导入的委托（组合根在装配期调用）。
    ///
    /// builder 形态而非构造参数：不注入时接缝仍可用（只是 `import_sources`
    /// 返回能力错误），故它不该成为构造的必填项。
    pub fn with_source_importer(mut self, importer: Arc<dyn SourceImporter>) -> Self {
        self.source_importer = Some(importer);
        self
    }

    /// 落库一批书源，并把存储层的计数映射成接缝的报告。
    ///
    /// # `skipped` 为什么由差额算出
    ///
    /// `SourceStore::merge` 的 [`PersistCounts`](crate::source_store::PersistCounts)
    /// 只带"新增/更新"两个计数——逐条失败明细在解析阶段已由导入器报过。
    /// 但"写进去的条数少于拿到的条数"这件事仍须如实反映：差额正是没落库的
    /// 那几条。若一律报 `skipped: 0`，一次部分失败的导入会显示为全部成功。
    fn persist_sources(&self, sources: &[Source]) -> Result<ImportReport, ErrorKind> {
        let counts = self.source_store.merge(sources).map_err(seam_kind)?;
        let persisted = counts.added + counts.updated;
        Ok(ImportReport {
            added: counts.added,
            updated: counts.updated,
            skipped: sources.len().saturating_sub(persisted),
            failures: Vec::new(),
        })
    }
}

impl BookRepository for SqliteBookRepository {
    // -----------------------------------------------------------------------
    // 书架
    // -----------------------------------------------------------------------

    /// [DONE] 书架全量（最近加入的在前）。
    fn bookshelf(&self) -> Result<Vec<Book>, ErrorKind> {
        self.book_dao.bookshelf().map_err(seam_kind)
    }

    fn add_book(&self, book: &Book) -> Result<i64, ErrorKind> {
        self.book_dao.add_book(book).map_err(seam_kind)
    }

    /// [DONE] 移除书籍：库里的行靠级联清，**缓存与搜索索引在此显式清**。
    ///
    /// # 为什么顺带清缓存（FR-004）
    ///
    /// FR-004 要求"移除后其缓存内容按 S1 存储语义清理"。`chapter_cache` 行会
    /// 随 `book` 级联删除，但**Pack 文件不会**——那是文件系统上的东西，数据库
    /// 的级联管不着。留下的容器文件既占空间又永远不会再被引用（`evict_orphan_packs`
    /// 只在淘汰路径上跑）。故这里调 [`PackStore::clear_book_cache`]。
    ///
    /// 接缝签名里 `remove_book` 的契约只说"含其章节、痕迹、进度"，缓存清理
    /// 另有独立入口（`clear_cache`）——但"移除一本书"之后还留着它几十 MB 的
    /// 正文文件，对用户就是"删了却没省空间"。故这里顺带做掉，代价只是多一次
    /// 目录删除。
    ///
    /// # 为什么顺序是"索引 → 缓存 → 删书"
    ///
    /// 三者都必须在书还在的时候定位"这本书的条目"：`chapter_fts` 是虚拟表
    /// （没有外键），它的 rowid 靠 `SELECT id FROM chapter WHERE book_id=?`
    /// 找；`chapter_cache` 的外键指向 `chapter`/`book`，书一删就级联没了。
    /// 先删书则前两步都会退化成"删 0 行"——静默留下悬空引用。
    fn remove_book(&self, book_id: i64) -> Result<(), ErrorKind> {
        self.text_index.ensure_schema().map_err(seam_kind)?;
        self.text_index.remove_book(book_id).map_err(seam_kind)?;
        self.pack_store
            .clear_book_cache(book_id)
            .map_err(seam_kind)?;
        self.book_dao.remove_book(book_id).map_err(seam_kind)
    }

    fn update_book(&self, book: &Book) -> Result<(), ErrorKind> {
        self.book_dao.update_book(book).map_err(seam_kind)
    }

    // -----------------------------------------------------------------------
    // 章节
    // -----------------------------------------------------------------------

    fn chapters(&self, book_id: i64) -> Result<Vec<Chapter>, ErrorKind> {
        self.book_dao.chapters(book_id).map_err(seam_kind)
    }

    fn save_chapters(&self, book_id: i64, chapters: &[Chapter]) -> Result<(), ErrorKind> {
        self.book_dao
            .save_chapters(book_id, chapters)
            .map_err(seam_kind)
    }

    // -----------------------------------------------------------------------
    // 进度与阅读记录
    // -----------------------------------------------------------------------

    fn get_progress(&self, book_id: i64) -> Result<Option<ReadProgress>, ErrorKind> {
        self.trace_dao.progress(book_id).map_err(seam_kind)
    }

    fn save_progress(&self, progress: &ReadProgress) -> Result<(), ErrorKind> {
        self.trace_dao.save_progress(progress).map_err(seam_kind)
    }

    fn add_read_record(&self, record: &ReadRecord) -> Result<(), ErrorKind> {
        self.trace_dao.add_read_record(record).map_err(seam_kind)
    }

    fn read_records(
        &self,
        book_id: i64,
        from_date: &str,
        to_date: &str,
    ) -> Result<Vec<ReadRecord>, ErrorKind> {
        self.trace_dao
            .read_records(book_id, from_date, to_date)
            .map_err(seam_kind)
    }

    // -----------------------------------------------------------------------
    // 阅读痕迹（一律字符区间，FR-028）
    // -----------------------------------------------------------------------

    fn bookmarks(&self, book_id: i64) -> Result<Vec<Bookmark>, ErrorKind> {
        self.trace_dao.bookmarks(book_id).map_err(seam_kind)
    }

    fn add_bookmark(&self, bookmark: &Bookmark) -> Result<i64, ErrorKind> {
        self.trace_dao.add_bookmark(bookmark).map_err(seam_kind)
    }

    fn remove_bookmark(&self, bookmark_id: i64) -> Result<(), ErrorKind> {
        self.trace_dao
            .remove_bookmark(bookmark_id)
            .map_err(seam_kind)
    }

    fn highlights(&self, book_id: i64) -> Result<Vec<Highlight>, ErrorKind> {
        self.trace_dao.highlights(book_id).map_err(seam_kind)
    }

    fn add_highlight(&self, highlight: &Highlight) -> Result<i64, ErrorKind> {
        self.trace_dao.add_highlight(highlight).map_err(seam_kind)
    }

    fn remove_highlight(&self, highlight_id: i64) -> Result<(), ErrorKind> {
        self.trace_dao
            .remove_highlight(highlight_id)
            .map_err(seam_kind)
    }

    fn memos(&self, book_id: i64) -> Result<Vec<Memo>, ErrorKind> {
        self.trace_dao.memos(book_id).map_err(seam_kind)
    }

    fn add_memo(&self, memo: &Memo) -> Result<i64, ErrorKind> {
        self.trace_dao.add_memo(memo).map_err(seam_kind)
    }

    fn remove_memo(&self, memo_id: i64) -> Result<(), ErrorKind> {
        self.trace_dao.remove_memo(memo_id).map_err(seam_kind)
    }

    // -----------------------------------------------------------------------
    // 书源
    // -----------------------------------------------------------------------

    fn sources(&self) -> Result<Vec<Source>, ErrorKind> {
        self.source_store.all().map_err(seam_kind)
    }

    fn source_by_id(&self, source_id: &str) -> Result<Option<Source>, ErrorKind> {
        self.source_store.get(source_id).map_err(seam_kind)
    }

    /// [DONE] 导入书源（FR-001 的三渠道）。
    ///
    /// # 未注入导入器时为什么返回能力错误而不是"导入 0 条"
    ///
    /// 解析在 `tome-source`，本 crate 拿不到它（见模块文档）。把"没有解析
    /// 能力"报成成功会让用户看到"导入完成，新增 0 条"——那与"这份文件里
    /// 确实没有源"**无法区分**，用户会去反复检查文件。能力错误是结构化、
    /// 可捕获的（FR-006），上层据此提示"此构建不含书源解析"。
    fn import_sources(&self, channel: &SourceImportChannel) -> Result<ImportReport, ErrorKind> {
        let Some(importer) = &self.source_importer else {
            return Err(ErrorKind::unsupported_capability(ErrorCapability::new(
                SOURCE_IMPORT_CAPABILITY,
            )));
        };
        let sources = importer.load(channel)?;
        self.persist_sources(&sources)
    }

    fn import_parsed_sources(&self, sources: &[Source]) -> Result<ImportReport, ErrorKind> {
        self.persist_sources(sources)
    }

    fn source_groups(&self) -> Result<Vec<SourceGroup>, ErrorKind> {
        self.source_store.groups().map_err(seam_kind)
    }

    // -----------------------------------------------------------------------
    // 规则（四类，FR-014 / FR-015 / FR-016 / FR-020）
    // -----------------------------------------------------------------------

    fn replace_rules(&self) -> Result<Vec<ReplaceRule>, ErrorKind> {
        self.misc_dao.replace_rules().map_err(seam_kind)
    }

    fn save_replace_rule(&self, rule: &ReplaceRule) -> Result<i64, ErrorKind> {
        self.misc_dao.save_replace_rule(rule).map_err(seam_kind)
    }

    fn remove_replace_rule(&self, rule_id: i64) -> Result<(), ErrorKind> {
        self.misc_dao
            .remove_replace_rule(rule_id)
            .map_err(seam_kind)
    }

    fn highlight_rules(&self) -> Result<Vec<HighlightRuleRow>, ErrorKind> {
        self.misc_dao.highlight_rules().map_err(seam_kind)
    }

    fn save_highlight_rule(&self, rule: &HighlightRuleRow) -> Result<i64, ErrorKind> {
        self.misc_dao.save_highlight_rule(rule).map_err(seam_kind)
    }

    fn remove_highlight_rule(&self, rule_id: i64) -> Result<(), ErrorKind> {
        self.misc_dao
            .remove_highlight_rule(rule_id)
            .map_err(seam_kind)
    }

    fn dict_rules(&self) -> Result<Vec<DictRule>, ErrorKind> {
        self.misc_dao.dict_rules().map_err(seam_kind)
    }

    fn save_dict_rule(&self, rule: &DictRule) -> Result<i64, ErrorKind> {
        self.misc_dao.save_dict_rule(rule).map_err(seam_kind)
    }

    fn remove_dict_rule(&self, rule_id: i64) -> Result<(), ErrorKind> {
        self.misc_dao.remove_dict_rule(rule_id).map_err(seam_kind)
    }

    fn txt_toc_rules(&self) -> Result<Vec<TxtTocRule>, ErrorKind> {
        self.misc_dao.txt_toc_rules().map_err(seam_kind)
    }

    fn save_txt_toc_rule(&self, rule: &TxtTocRule) -> Result<i64, ErrorKind> {
        self.misc_dao.save_txt_toc_rule(rule).map_err(seam_kind)
    }

    fn remove_txt_toc_rule(&self, rule_id: i64) -> Result<(), ErrorKind> {
        self.misc_dao
            .remove_txt_toc_rule(rule_id)
            .map_err(seam_kind)
    }

    // -----------------------------------------------------------------------
    // 库根目录（FR-023）
    // -----------------------------------------------------------------------

    fn library_roots(&self) -> Result<Vec<LibraryRoot>, ErrorKind> {
        self.book_dao.library_roots().map_err(seam_kind)
    }

    fn add_library_root(&self, path: &str) -> Result<i64, ErrorKind> {
        self.book_dao.add_library_root(path).map_err(seam_kind)
    }

    // -----------------------------------------------------------------------
    // 书内搜索（FR-019b）
    // -----------------------------------------------------------------------

    /// [DONE] 书内全文搜索（只查已缓存内容，不触发抓取）。
    ///
    /// # 为什么在这里幂等建索引表
    ///
    /// `chapter_fts` 由 `fts` 模块自己建——迁移器不建它（见该模块的说明）。
    /// 放在这里而不是构造期有两个理由：构造签名不返回 `Result`，在构造期
    /// 建表就只能吞掉失败；而建表是幂等的（`CREATE ... IF NOT EXISTS`），
    /// 放在真正需要它的入口上，失败能如实报出来，而不是变成"搜索永远没结果"。
    fn search_in_book(&self, book_id: i64, query: &str) -> Result<Vec<SearchHit>, ErrorKind> {
        self.text_index.ensure_schema().map_err(seam_kind)?;
        let hits = self
            .text_index
            .search_in_book(book_id, query)
            .map_err(seam_kind)?;
        // 接缝的命中不含章节标题（那是展示层按需再取的字段），故此处丢弃它
        Ok(hits
            .into_iter()
            .map(|hit| SearchHit {
                chapter_index: hit.chapter_index,
                char_offset: hit.char_offset,
                snippet: hit.snippet,
            })
            .collect())
    }

    // -----------------------------------------------------------------------
    // 备份（FR-033）
    // -----------------------------------------------------------------------

    fn export_all(&self) -> Result<BackupJson, ErrorKind> {
        let json = self.backup_store.export_json().map_err(seam_kind)?;
        Ok(BackupJson { json })
    }

    /// 导入备份（FR-033）。与 [`Self::export_all`] 对称。
    ///
    /// # 报告字段的语义（这里没有"新增/更新"的区分）
    ///
    /// 备份导入是**跨类目的合并**（书源、书架、进度、痕迹、规则一起 upsert），
    /// 而 `BackupStats` 只给各类目的条目数，**不区分**某一条是新增还是更新
    /// （存储层在事务内 upsert，判重结果不回流）。故把落库的**条目总数**计入
    /// `added`，`updated` 留 0——报一个"只统计了书源"的数字会让调用方以为
    /// 其余数据没导进来，而凭空估一个更新数则是编造。
    fn import_backup(&self, json: &BackupJson) -> Result<ImportReport, ErrorKind> {
        let stats = self
            .backup_store
            .import_json(&json.json)
            .map_err(seam_kind)?;
        Ok(ImportReport {
            added: stats.total(),
            updated: 0,
            skipped: 0,
            failures: Vec::new(),
        })
    }

    // -----------------------------------------------------------------------
    // 缓存清理（FR-031 / SC-017）
    // -----------------------------------------------------------------------

    /// 清理缓存（MUST NOT 影响个人阅读数据）。
    ///
    /// 两类缓存各自独立：`Chapter` 清 Pack 容器与 `chapter_cache` 索引行，
    /// `Image` 清 `cache/images/` 与 `image_cache` 行。两者都不碰书架/进度/
    /// 痕迹/规则/书源（SC-015 的"个人数据 100% 保留"）。
    ///
    /// 章节缓存清掉后，`chapter_fts` 里可能短暂留下指向已清内容的条目——
    /// 那不是悬空引用：`CachedTextIndex::search_in_book` 会校验内容可取，
    /// 取不到的条目顺手清掉（见该方法的说明），故不必在这里额外扫索引。
    fn clear_cache(&self, kind: CacheKind) -> Result<(), ErrorKind> {
        match kind {
            CacheKind::Chapter => {
                self.pack_store.clear_all().map_err(seam_kind)?;
            }
            CacheKind::Image => {
                self.image_cache.clear().map_err(seam_kind)?;
            }
        }
        Ok(())
    }
}
