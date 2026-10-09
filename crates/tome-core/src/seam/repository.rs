//! `BookRepository` 接缝（`contracts/seams.md` §3）

use crate::error::ErrorKind;
use crate::model::{
    Book, Bookmark, Chapter, DictRule, Highlight, LibraryRoot, Memo, ReadProgress, ReadRecord,
    ReplaceRule, Source, SourceGroup,
};

/// 数据读写接缝。实现方：`tome-store`（SQLite actor 线程 + Pack 文件）。
///
/// ## 关键不变量（`contracts/seams.md` §3）
///
/// - **阅读痕迹一律字符区间**，不存渲染坐标（FR-028）
/// - 缓存写入**崩溃安全**（FR-030）；缓存清除**不影响个人数据**（FR-031 / SC-015）
/// - 导入重复书源（同 id）按"更新"处理
/// - 所有写操作走 DB actor 线程，调用方拿到的是**结果**而非连接
///
/// ## 为什么方法返回 `Result` 而不是连接/句柄
///
/// 最后一条不变量决定：`rusqlite::Connection` 不是 `Sync`，数据库归专属
/// actor 线程所有。调用方发消息、收结果——故本 trait 的每个方法都是
/// "一次调用 = 一次完整操作"，不暴露任何会话状态。
pub trait BookRepository {
    // --- 书架 ---

    /// 书架全量。
    fn bookshelf(&self) -> Result<Vec<Book>, ErrorKind>;
    /// 加入书架。返回分配好的主键。
    fn add_book(&self, book: &Book) -> Result<i64, ErrorKind>;
    /// 移除书籍（含其章节、痕迹、进度）。
    fn remove_book(&self, book_id: i64) -> Result<(), ErrorKind>;
    /// 更新书籍元信息。
    fn update_book(&self, book: &Book) -> Result<(), ErrorKind>;

    // --- 章节 ---

    /// 某书的章节列表。
    fn chapters(&self, book_id: i64) -> Result<Vec<Chapter>, ErrorKind>;
    /// 批量写入/更新章节（目录抓取后一次性落库）。
    fn save_chapters(&self, book_id: i64, chapters: &[Chapter]) -> Result<(), ErrorKind>;

    // --- 进度与阅读记录 ---

    /// 读进度。
    fn get_progress(&self, book_id: i64) -> Result<Option<ReadProgress>, ErrorKind>;
    /// 写进度（锚点：章节 + 字符偏移 + 锚点文本）。
    fn save_progress(&self, progress: &ReadProgress) -> Result<(), ErrorKind>;
    /// 累加一条阅读记录（FR-026b：**无界面下同样落库**）。
    fn add_read_record(&self, record: &ReadRecord) -> Result<(), ErrorKind>;
    /// 查询阅读记录（按日期范围）。
    fn read_records(
        &self,
        book_id: i64,
        from_date: &str,
        to_date: &str,
    ) -> Result<Vec<ReadRecord>, ErrorKind>;

    // --- 阅读痕迹（一律字符区间，FR-028）---

    /// 某书的书签。
    fn bookmarks(&self, book_id: i64) -> Result<Vec<Bookmark>, ErrorKind>;
    /// 新增书签，返回主键。
    fn add_bookmark(&self, bookmark: &Bookmark) -> Result<i64, ErrorKind>;
    /// 删除书签。
    fn remove_bookmark(&self, bookmark_id: i64) -> Result<(), ErrorKind>;

    /// 某书的高亮。
    fn highlights(&self, book_id: i64) -> Result<Vec<Highlight>, ErrorKind>;
    /// 新增高亮，返回主键。
    fn add_highlight(&self, highlight: &Highlight) -> Result<i64, ErrorKind>;
    /// 删除高亮。
    fn remove_highlight(&self, highlight_id: i64) -> Result<(), ErrorKind>;

    /// 某书的批注。
    fn memos(&self, book_id: i64) -> Result<Vec<Memo>, ErrorKind>;
    /// 新增批注，返回主键。
    fn add_memo(&self, memo: &Memo) -> Result<i64, ErrorKind>;
    /// 删除批注。
    fn remove_memo(&self, memo_id: i64) -> Result<(), ErrorKind>;

    // --- 书源 ---

    /// 全部书源。
    fn sources(&self) -> Result<Vec<Source>, ErrorKind>;
    /// 单个书源。
    fn source_by_id(&self, source_id: &str) -> Result<Option<Source>, ErrorKind>;
    /// 导入书源。**同 id 按"更新"处理**（spec 边缘情况）。
    fn import_sources(&self, sources: &[Source]) -> Result<ImportReport, ErrorKind>;
    /// 书源分组。
    fn source_groups(&self) -> Result<Vec<SourceGroup>, ErrorKind>;

    // --- 规则 ---

    /// 替换/净化规则。
    fn replace_rules(&self) -> Result<Vec<ReplaceRule>, ErrorKind>;
    /// 高亮规则。
    fn highlight_rules(&self) -> Result<Vec<HighlightRuleRow>, ErrorKind>;
    /// 词典规则。
    fn dict_rules(&self) -> Result<Vec<DictRule>, ErrorKind>;

    // --- 库根目录 ---

    /// 本地库根目录列表（FR-023 支持多个）。
    fn library_roots(&self) -> Result<Vec<LibraryRoot>, ErrorKind>;
    /// 添加库根目录。
    fn add_library_root(&self, path: &str) -> Result<i64, ErrorKind>;

    // --- 书内搜索（FR-019b）---

    /// 书内全文搜索，**限定已缓存内容，不触发额外抓取**（FR-019b）。
    fn search_in_book(&self, book_id: i64, query: &str) -> Result<Vec<SearchHit>, ErrorKind>;

    // --- 缓存清理（FR-031 / SC-017）---

    /// 清理缓存。**MUST NOT 影响个人阅读数据**（FR-031 / SC-015）。
    fn clear_cache(&self, kind: CacheKind) -> Result<(), ErrorKind>;
}

/// 规则行的别名。
///
/// `HighlightRule`（规则定义）与 `Highlight`（痕迹）在语义上分属"规则"与
/// "痕迹"，但名字在中文语境里都叫"高亮"，容易误用。此处给出显式别名，
/// 让接缝签名自解释。
pub use crate::model::HighlightRule as HighlightRuleRow;

/// 可独立清理的缓存类别（SC-017 要求章节缓存与图片缓存**各自可清**）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheKind {
    /// 章节内容缓存（Pack 容器，FR-029）
    Chapter,
    /// 图片缓存（FR-019）
    Image,
}

/// 书源导入结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// 新增的源数
    pub added: usize,
    /// 更新的源数（同 id 已存在）
    pub updated: usize,
    /// 跳过的源数（如格式非法）
    pub skipped: usize,
    /// 逐条失败的说明（源的 id 或索引 + 分类原因）
    pub failures: Vec<ImportFailure>,
}

/// 单条书源导入失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFailure {
    /// 失败条目的标识（书源 id，或其在输入中的序号）
    pub subject: String,
    /// 失败分类
    pub error: ErrorKind,
}

/// 书内全文搜索的命中（FR-019b）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// 命中章节序号
    pub chapter_index: i32,
    /// 命中处在章内的**字符偏移**
    pub char_offset: i64,
    /// 命中处的上下文片段（供上层展示）
    pub snippet: String,
}
