//! `BookRepository` 接缝（`contracts/seams.md` §3）

use crate::error::ErrorKind;
use crate::model::{
    Book, Bookmark, Chapter, DictRule, Highlight, LibraryRoot, Memo, ReadProgress, ReadRecord,
    ReplaceRule, Source, SourceGroup, TxtTocRule,
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
    ///
    /// 覆盖 FR-001 要求的**三种渠道**（本地文件 / URL / 粘贴文本），由
    /// [`SourceImportChannel`] 表达。契约 §3 的 `import_sources(file | url | text)`
    /// 即指此签名。
    ///
    /// ## 为什么渠道必须是参数而不是"由上层解析好再传 Source"
    ///
    /// FR-001 明确要求三种渠道**都可导入**，且 URL 渠道含"粘贴链接后拉取并解析"
    /// ——这一步需要发网络请求。若签名只收 `&[Source]`，则：
    /// 1. "URL 渠道"在接缝上**无法表达**，该能力会退化成上层各自的实现
    ///    （于是网络策略、重试、限速三处各写一遍，绕过 A2 的"唯一发请求处"）；
    /// 2. 导入格式的解析（含对既有社区流通格式的容错）也会散落到各调用方。
    ///
    /// 故渠道进签名，实现方（tome-store 经组合根拿到 NetHost）负责拉取与解析。
    fn import_sources(&self, channel: &SourceImportChannel) -> Result<ImportReport, ErrorKind>;
    /// 导入**已解析好**的书源（供批量导入场景直接落库，省去重复解析）。
    ///
    /// 与 [`Self::import_sources`] 的关系：前者覆盖 FR-001 的三渠道（含拉取与
    /// 解析），后者是"手里已有 `Source` 列表"时的快路径。两者共用同一套
    /// "同 id 即更新"的落库语义。
    fn import_parsed_sources(&self, sources: &[Source]) -> Result<ImportReport, ErrorKind>;
    /// 书源分组。
    fn source_groups(&self) -> Result<Vec<SourceGroup>, ErrorKind>;

    // --- 规则 ---
    //
    // 契约 §3 写作 `rules (replace/highlight/dict/toc CRUD)`——即四类规则
    // **都要 CRUD**，而不只是读。三类只读 getter 是不够的：
    // FR-014/FR-015/FR-016 要求用户能配置净化/高亮/词典规则，
    // FR-020 要求 TXT 目录识别支持"多条规则回退"（自然也需增删改）。
    // 故此处补齐四类规则的读、增/改、删。

    /// 替换/净化规则（FR-014）。
    fn replace_rules(&self) -> Result<Vec<ReplaceRule>, ErrorKind>;
    /// 保存替换规则（`id` 为 0 表示新增，否则更新）。
    fn save_replace_rule(&self, rule: &ReplaceRule) -> Result<i64, ErrorKind>;
    /// 删除替换规则。
    fn remove_replace_rule(&self, rule_id: i64) -> Result<(), ErrorKind>;

    /// 高亮规则（FR-015）。
    fn highlight_rules(&self) -> Result<Vec<HighlightRuleRow>, ErrorKind>;
    /// 保存高亮规则（`id` 为 0 表示新增）。
    fn save_highlight_rule(&self, rule: &HighlightRuleRow) -> Result<i64, ErrorKind>;
    /// 删除高亮规则。
    fn remove_highlight_rule(&self, rule_id: i64) -> Result<(), ErrorKind>;

    /// 词典规则（FR-016）。
    fn dict_rules(&self) -> Result<Vec<DictRule>, ErrorKind>;
    /// 保存词典规则（`id` 为 0 表示新增）。
    fn save_dict_rule(&self, rule: &DictRule) -> Result<i64, ErrorKind>;
    /// 删除词典规则。
    fn remove_dict_rule(&self, rule_id: i64) -> Result<(), ErrorKind>;

    /// TXT 目录识别规则（FR-020，支持多条规则回退）。
    fn txt_toc_rules(&self) -> Result<Vec<TxtTocRule>, ErrorKind>;
    /// 保存 TXT 目录识别规则（`id` 为 0 表示新增）。
    fn save_txt_toc_rule(&self, rule: &TxtTocRule) -> Result<i64, ErrorKind>;
    /// 删除 TXT 目录识别规则。
    fn remove_txt_toc_rule(&self, rule_id: i64) -> Result<(), ErrorKind>;

    // --- 库根目录 ---

    /// 本地库根目录列表（FR-023 支持多个）。
    fn library_roots(&self) -> Result<Vec<LibraryRoot>, ErrorKind>;
    /// 添加库根目录。
    fn add_library_root(&self, path: &str) -> Result<i64, ErrorKind>;

    // --- 书内搜索（FR-019b）---

    /// 书内全文搜索，**限定已缓存内容，不触发额外抓取**（FR-019b）。
    fn search_in_book(&self, book_id: i64, query: &str) -> Result<Vec<SearchHit>, ErrorKind>;

    // --- 备份（FR-033）---

    /// 导出**全部阅读数据**（FR-033）：书源集合、书架清单、阅读进度、书签、
    /// 高亮、批注、规则配置（净化/高亮/词典）。
    ///
    /// ## 导出与导入必须对称
    ///
    /// FR-033 明确"能导出即可完整导回"。故本方法与
    /// [`Self::import_backup`] 共用 [`BackupJson`] 这一类型——
    /// 对称性由类型保证，而不是靠两处各自维护的字段列表。
    fn export_all(&self) -> Result<BackupJson, ErrorKind>;

    /// 导入备份（FR-033）。与 [`Self::export_all`] 对称。
    fn import_backup(&self, json: &BackupJson) -> Result<ImportReport, ErrorKind>;

    // --- 缓存清理（FR-031 / SC-017）---

    /// 清理缓存。**MUST NOT 影响个人阅读数据**（FR-031 / SC-015）。
    fn clear_cache(&self, kind: CacheKind) -> Result<(), ErrorKind>;
}

/// 全部阅读数据的可读 JSON 表示（FR-033）。
///
/// ## 为什么是 JSON 文本而不是结构化字段
///
/// FR-032/FR-033 要求格式**可读、可手工编辑、可随程序搬移**。用文本承载让
/// 这一要求直接成立；同时它让"对称性"只需维护**一个**类型（导出与导入共用），
/// 而不是在两个结构体之间做字段映射——后者正是漂移的温床。
///
/// 字段的具体 schema 由 tome-store 定义并随实现发布（迁移器负责向前兼容），
/// core 只承载"这里有一份完整的备份"这一契约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupJson {
    /// JSON 文本
    pub json: String,
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

/// 书源导入渠道（FR-001 要求至少覆盖三种）。
///
/// 用枚举把三个渠道变成**类型层面可穷举**的集合：实现方 match 时必须处理全部
/// 三种，新增渠道时编译器会指出所有遗漏处。若改用 `Option<String>` 之类的
/// 松散结构，"URL 渠道没实现"这种缺失不会在编译期暴露。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceImportChannel {
    /// 本地文件（选择或拖入）。内容为文件文本。
    FileText(String),
    /// URL（粘贴链接后拉取并解析）。
    ///
    /// 仅承载 URL 本身——**拉取由实现方进行**（走 `NetHost`，从而遵守限速、
    /// 代理、重试与"唯一发请求处"的约束，A2）。
    Url(String),
    /// 直接粘贴的书源 JSON 文本。
    PastedText(String),
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
