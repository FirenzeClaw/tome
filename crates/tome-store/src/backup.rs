//! \[DONE] 全部阅读数据的导出与导入（tasks.md T087 / FR-033）
//!
//! # FR-033 的原文与它带来的三个设计约束
//!
//! > 系统 MUST 支持导出与导入**全部阅读数据**：书源集合、书架清单、阅读
//! > 进度、书签、高亮、批注、规则配置（净化/高亮/词典），格式为可读的
//! > JSON；导出与导入须**对称**（能导出即可完整导回）
//!
//! | 约束 | 落地 |
//! |---|---|
//! | 「可读、可手工编辑」（FR-032 同款要求） | 人可读 JSON 文本，字段名用业务语义 |
//! | 「对称」 | **单一类型** [`Backup`] 同时承载导出与导入 |
//! | core DTO 不派生 serde（`tome-core/src/lib.rs` 的"零外部依赖"约束） | 本模块定义**镜像类型**（`*Row`）承载序列化 |
//!
//! ## 为什么"对称"要用单一类型而不是两个结构体
//!
//! 两个结构体（`ExportData` / `ImportData`）会让对称性变成**两处字段列表
//! 的约定**——加字段时忘了改另一边，导出就静默丢数据，而丢的那份**只在
//! 用户恢复时才发现**。用一个 `#[derive(Serialize, Deserialize)]` 的类型，
//! 序列化与反序列化天然同构，编译器替我们守这件事。
//!
//! ## 为什么需要镜像类型（而不是给 core DTO 加 serde）
//!
//! `tome-core` 明确"DTO 刻意不派生 serde 等序列化能力（那是存储/网络实现
//! crate 的关切）"（宪法 IV 单向依赖）。给 core 加 serde 会让所有 crate
//! 都跟着引入这一依赖，且把"备份格式"这一**存储关切**塞进公共语言。
//!
//! 镜像类型的额外收益：**"哪些字段进了备份"变成可读的清单**。若直接序列化
//! DTO，加一个字段就会静默进备份；有镜像层则必须显式写一行转换，
//! 于是每次扩字段都会面对"这个字段该不该备份"这个问题。
//!
//! # 导出**不含**什么（有意的边界）
//!
//! 不含章节正文缓存与图片缓存（FR-029/SC-017 的缓存是**可重建**的派生数据，
//! 体积巨大）。不含 HTTP 日志与 Cookie（前者是诊断信息，后者是**凭据**——
//! 导出一份带登录态的 JSON 再随手分享出去是个真实的风险）。
//!
//! 这不违背"全部阅读数据"：FR-033 逐项列举的七类都在，缓存与凭据不在其列。

use serde::{Deserialize, Serialize};

use tome_core::model::{
    Book, Bookmark, Chapter, DictRule, Highlight, HighlightRule, Memo, ReadProgress, ReadRecord,
    ReplaceRule, RuleScope, Source, SourceKind, TxtTocRule,
};
use tome_core::BookOrigin;

use crate::dao::{BookDao, MiscDao, SourceDao, TraceDao};
use crate::db::Db;
use crate::error::StoreError;

/// 备份格式的版本号。
///
/// # 为什么现在就带上（还没有第二个版本）
///
/// 导入方必须能判断"这份文件我读不读得懂"。没有版本号时，遇到未来格式
/// 只能猜——猜错就是静默数据损坏。带上它，导入时能明确报错而不是误解。
pub const BACKUP_FORMAT_VERSION: i32 = 1;

// ---------------------------------------------------------------------------
// 镜像类型（可序列化）
// ---------------------------------------------------------------------------

/// 书源（镜像 [`Source`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRow {
    /// 书源 id（书源 URL）
    pub id: String,
    /// 书源名
    pub name: String,
    /// 类型（`book` / `js` / `subscribe` / `dictionary`）
    pub kind: String,
    /// 所属分组 id
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<i64>,
    /// 是否启用
    pub enabled: bool,
    /// 书籍类型
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_source_type: Option<i32>,
    /// 详情页 URL 匹配模式
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_url_pattern: Option<String>,
    /// 搜索地址模板
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search_url: Option<String>,
    /// 发现地址模板
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explore_url: Option<String>,
    /// 搜索规则
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_search: Option<String>,
    /// 发现规则
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_explore: Option<String>,
    /// 详情规则
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_book_info: Option<String>,
    /// 目录规则
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_toc: Option<String>,
    /// 正文规则
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_content: Option<String>,
    /// 请求头
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// 登录地址
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_url: Option<String>,
    /// 登录界面
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_ui: Option<String>,
    /// 登录校验脚本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login_check_js: Option<String>,
    /// 纯脚本源主脚本
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub js: Option<String>,
    /// 字符集
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    /// 请求间隔（毫秒）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_ms: Option<i64>,
    /// 排序权重
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<i32>,
    /// 发现页配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explore_screen: Option<String>,
    /// 备注
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// 最后更新时间戳
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update_time: Option<i64>,
    /// 创建时间戳
    pub created_at: i64,
}

impl SourceRow {
    /// core DTO → 镜像。
    #[must_use]
    pub fn from_source(source: &Source) -> Self {
        Self {
            id: source.id.clone(),
            name: source.name.clone(),
            kind: kind_code(source.kind).to_string(),
            group_id: source.group_id,
            enabled: source.enabled,
            book_source_type: source.book_source_type,
            book_url_pattern: source.book_url_pattern.clone(),
            search_url: source.search_url.clone(),
            explore_url: source.explore_url.clone(),
            rule_search: source.rule_search.clone(),
            rule_explore: source.rule_explore.clone(),
            rule_book_info: source.rule_book_info.clone(),
            rule_toc: source.rule_toc.clone(),
            rule_content: source.rule_content.clone(),
            header: source.header.clone(),
            login_url: source.login_url.clone(),
            login_ui: source.login_ui.clone(),
            login_check_js: source.login_check_js.clone(),
            js: source.js.clone(),
            charset: source.charset.clone(),
            rate_limit_ms: source.rate_limit_ms,
            weight: source.weight,
            explore_screen: source.explore_screen.clone(),
            comment: source.comment.clone(),
            last_update_time: source.last_update_time,
            created_at: source.created_at,
        }
    }

    /// 镜像 → core DTO。
    #[must_use]
    pub fn into_source(self) -> Source {
        Source {
            kind: parse_kind(&self.kind),
            id: self.id,
            name: self.name,
            group_id: self.group_id,
            enabled: self.enabled,
            book_source_type: self.book_source_type,
            book_url_pattern: self.book_url_pattern,
            search_url: self.search_url,
            explore_url: self.explore_url,
            rule_search: self.rule_search,
            rule_explore: self.rule_explore,
            rule_book_info: self.rule_book_info,
            rule_toc: self.rule_toc,
            rule_content: self.rule_content,
            header: self.header,
            login_url: self.login_url,
            login_ui: self.login_ui,
            login_check_js: self.login_check_js,
            js: self.js,
            charset: self.charset,
            rate_limit_ms: self.rate_limit_ms,
            weight: self.weight,
            explore_screen: self.explore_screen,
            comment: self.comment,
            last_update_time: self.last_update_time,
            created_at: self.created_at,
        }
    }
}

/// 书源类型 → 稳定字符串标识。
///
/// 用字符串而非整数编码：备份是**给人看的、可手工编辑的**（FR-033），
/// 而 `2` 这种编码对人不友好且与 schema 内部约定耦合。
#[must_use]
fn kind_code(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Book => "book",
        SourceKind::Js => "js",
        SourceKind::Subscribe => "subscribe",
        SourceKind::Dictionary => "dictionary",
    }
}

/// 字符串标识 → 书源类型。未知值回落为 `Book`（最宽容的选择：宁可当成普通源）。
#[must_use]
fn parse_kind(text: &str) -> SourceKind {
    match text {
        "js" => SourceKind::Js,
        "subscribe" => SourceKind::Subscribe,
        "dictionary" => SourceKind::Dictionary,
        _ => SourceKind::Book,
    }
}

/// 书源分组（镜像 [`SourceGroup`](tome_core::SourceGroup)）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceGroupRow {
    /// 分组名（**不含 id**：id 是本机分配的，导入时重建）
    pub name: String,
    /// 排序权重
    pub sort_order: i32,
    /// 组内书源是否启用
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// `serde` 的默认值：`true`。
fn default_true() -> bool {
    true
}

/// 规则作用域 → 稳定字符串。
#[must_use]
fn scope_code(scope: RuleScope) -> &'static str {
    match scope {
        RuleScope::Content => "content",
        RuleScope::BookName => "bookName",
        RuleScope::ChapterTitle => "chapterTitle",
        RuleScope::Global => "global",
    }
}

/// 字符串 → 规则作用域（未知值回落 `Content`）。
#[must_use]
fn parse_scope(text: &str) -> RuleScope {
    match text {
        "bookName" => RuleScope::BookName,
        "chapterTitle" => RuleScope::ChapterTitle,
        "global" => RuleScope::Global,
        _ => RuleScope::Content,
    }
}

/// 替换/净化规则（镜像 [`ReplaceRule`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceRuleRow {
    /// 规则名
    pub name: String,
    /// 匹配模式
    pub pattern: String,
    /// 替换为
    pub replacement: String,
    /// 生效范围
    pub scope: String,
    /// 是否正则
    pub is_regex: bool,
    /// 是否启用
    pub enabled: bool,
    /// 应用顺序
    pub sort_order: i32,
}

/// 高亮规则（镜像 [`HighlightRule`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightRuleRow {
    /// 规则名
    pub name: String,
    /// 匹配模式
    pub pattern: String,
    /// 样式标识
    pub style: String,
    /// 是否启用
    pub enabled: bool,
    /// 应用顺序
    pub sort_order: i32,
}

/// 词典规则（镜像 [`DictRule`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictRuleRow {
    /// 规则名
    pub name: String,
    /// 查词地址规则（含占位符，由 tome-rule 求值）
    pub url_rule: String,
    /// 释义提取规则
    pub content_rule: String,
    /// 是否启用
    pub enabled: bool,
}

/// TXT 目录识别规则（镜像 [`TxtTocRule`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxtTocRuleRow {
    /// 规则名
    pub name: String,
    /// 识别规则（正则）
    pub rule: String,
    /// 示例（供界面预览）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
    /// 是否启用
    pub enabled: bool,
    /// 应用顺序（多条规则的回退顺序）
    pub sort_order: i32,
}

/// 书籍元信息（镜像 [`Book`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookRow {
    /// 来源类型（`network` / `local`）
    pub origin: String,
    /// 来源标识
    pub origin_id: String,
    /// 书名
    pub name: String,
    /// 作者
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// 封面
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    /// 简介
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    /// 分类
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 书架分组
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_group: Option<String>,
    /// 排版配置
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_config: Option<String>,
    /// 目录页地址
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toc_url: Option<String>,
    /// 最新章节标题
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_chapter_title: Option<String>,
    /// 最新章节时间戳
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_chapter_time: Option<i64>,
    /// 相对库根路径（本地书）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    /// 字符集（本地书）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charset: Option<String>,
    /// 文件格式（本地书）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// 总章数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_chapters: Option<i32>,
    /// 当前读到第几章
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur_chapter_index: Option<i32>,
    /// 加入时间戳
    pub added_at: i64,
}

impl BookRow {
    /// core DTO → 镜像。
    ///
    /// # 为什么不带 `library_root_id` 与 `id`
    ///
    /// 两者都是**本机分配的主键**，跨机器无意义。`library_roots` 按路径
    /// 重建，导入时重新关联；书籍 id 由导入时重新分配（见 [`BookBackup`]）。
    #[must_use]
    pub fn from_book(book: &Book) -> Self {
        Self {
            origin: match book.origin {
                BookOrigin::Network => "network".to_string(),
                BookOrigin::Local => "local".to_string(),
            },
            origin_id: book.origin_id.clone(),
            name: book.name.clone(),
            author: book.author.clone(),
            cover_url: book.cover_url.clone(),
            intro: book.intro.clone(),
            kind: book.kind.clone(),
            book_group: book.book_group.clone(),
            read_config: book.read_config.clone(),
            toc_url: book.toc_url.clone(),
            latest_chapter_title: book.latest_chapter_title.clone(),
            latest_chapter_time: book.latest_chapter_time,
            relative_path: book.relative_path.clone(),
            charset: book.charset.clone(),
            format: book.format.clone(),
            total_chapters: book.total_chapters,
            dur_chapter_index: book.dur_chapter_index,
            added_at: book.added_at,
        }
    }

    /// 镜像 → core DTO（`id` 与 `library_root_id` 为占位，由导入方填写）。
    #[must_use]
    pub fn into_book(&self, id: i64, library_root_id: Option<i64>) -> Book {
        Book {
            id,
            origin: if self.origin == "local" {
                BookOrigin::Local
            } else {
                BookOrigin::Network
            },
            origin_id: self.origin_id.clone(),
            name: self.name.clone(),
            author: self.author.clone(),
            cover_url: self.cover_url.clone(),
            intro: self.intro.clone(),
            kind: self.kind.clone(),
            book_group: self.book_group.clone(),
            read_config: self.read_config.clone(),
            toc_url: self.toc_url.clone(),
            latest_chapter_title: self.latest_chapter_title.clone(),
            latest_chapter_time: self.latest_chapter_time,
            library_root_id,
            relative_path: self.relative_path.clone(),
            charset: self.charset.clone(),
            format: self.format.clone(),
            total_chapters: self.total_chapters,
            dur_chapter_index: self.dur_chapter_index,
            added_at: self.added_at,
        }
    }
}

/// 章节（镜像 [`Chapter`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChapterRow {
    /// 书内序号
    pub index_in_book: i32,
    /// 标题
    pub title: String,
    /// 内容地址（网络书）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 字节偏移（本地 TXT）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<i64>,
    /// zip 目录项名（本地 EPUB）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epub_entry: Option<String>,
    /// 是否已读
    #[serde(default)]
    pub is_read: bool,
    /// 字数
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_count: Option<i32>,
}

impl ChapterRow {
    /// core DTO → 镜像。
    ///
    /// # 为什么不带 `is_cached`
    ///
    /// `is_cached` 描述的是"章正文是否在本地缓存容器里"，而缓存**不在
    /// 备份范围内**（可重建的派生数据）。带上它会产生一份声称"已缓存"
    /// 但缓存目录为空的备份——导入方据它跳过抓取就会读到空正文。
    #[must_use]
    pub fn from_chapter(chapter: &Chapter) -> Self {
        Self {
            index_in_book: chapter.index_in_book,
            title: chapter.title.clone(),
            url: chapter.url.clone(),
            byte_offset: chapter.byte_offset,
            epub_entry: chapter.epub_entry.clone(),
            is_read: chapter.is_read,
            word_count: chapter.word_count,
        }
    }

    /// 镜像 → core DTO。
    #[must_use]
    pub fn into_chapter(&self, book_id: i64) -> Chapter {
        Chapter {
            id: 0,
            book_id,
            index_in_book: self.index_in_book,
            title: self.title.clone(),
            url: self.url.clone(),
            byte_offset: self.byte_offset,
            epub_entry: self.epub_entry.clone(),
            // 缓存不在备份内，故一律 false（见 `from_chapter` 的说明）
            is_cached: false,
            is_read: self.is_read,
            word_count: self.word_count,
        }
    }
}

/// 阅读进度（镜像 [`ReadProgress`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressRow {
    /// 章内序号
    pub chapter_index: i32,
    /// 章内字符偏移
    pub char_offset: i64,
    /// 锚点文本（FR-010b 的特征段匹配输入）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_text: Option<String>,
    /// 更新时间戳
    pub updated_at: i64,
}

/// 阅读记录（镜像 [`ReadRecord`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadRecordRow {
    /// 日期 `YYYY-MM-DD`
    pub date: String,
    /// 当日阅读秒数
    pub read_seconds: i64,
    /// 当日阅读字数
    pub read_chars: i64,
}

/// 书签（镜像 [`Bookmark`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkRow {
    /// 章内序号
    pub chapter_index: i32,
    /// 章内字符偏移
    pub char_offset: i64,
    /// 备注
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 创建时间戳
    pub created_at: i64,
}

/// 高亮（镜像 [`Highlight`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightRow {
    /// 章内序号
    pub chapter_index: i32,
    /// 区间起点字符偏移
    pub char_offset: i64,
    /// 区间长度（字符数）
    pub length: i64,
    /// 样式标识
    pub style: String,
    /// 备注
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// 创建时间戳
    pub created_at: i64,
}

/// 批注（镜像 [`Memo`]）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoRow {
    /// 章内序号
    pub chapter_index: i32,
    /// 区间起点字符偏移
    pub char_offset: i64,
    /// 区间长度（字符数）
    pub length: i64,
    /// 笔记内容
    pub content: String,
    /// 创建时间戳
    pub created_at: i64,
}

/// 库根目录（镜像 [`LibraryRoot`](tome_core::LibraryRoot)）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryRootRow {
    /// 绝对路径
    pub path: String,
    /// 添加时间戳
    pub added_at: i64,
}

/// 一本书及其全部个人数据。
///
/// # 为什么嵌套而不是"所有进度一个平铺数组"
///
/// 因为书籍主键 `id` 是**本机分配的**，换台机器导入时会被重新分配——
/// 平铺的 `progress` 数组里那些 `book_id` 就全指向了错误的对象。
/// 嵌套让"这册书的这些数据"成为结构上不可分割的一组，导入时一起
/// 重新映射到新的主键。
///
/// 这是"能导出即可完整导回"在实际数据上的关键一步：**必须先把
/// 主键依赖关系变成结构，再谈映射**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookBackup {
    /// 书籍元信息
    pub book: BookRow,
    /// 章节列表（目录）
    #[serde(default)]
    pub chapters: Vec<ChapterRow>,
    /// 阅读进度
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ProgressRow>,
    /// 阅读记录（FR-026b）
    #[serde(default)]
    pub read_records: Vec<ReadRecordRow>,
    /// 书签
    #[serde(default)]
    pub bookmarks: Vec<BookmarkRow>,
    /// 高亮
    #[serde(default)]
    pub highlights: Vec<HighlightRow>,
    /// 批注
    #[serde(default)]
    pub memos: Vec<MemoRow>,
}

/// 一份完整备份（FR-033）。
///
/// ## 字段顺序即 JSON 里的顺序
///
/// 保持稳定可让备份文件可 diff——用户手工编辑或版本对比时，字段乱序
/// 会产生无意义的 diff 噪音。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    /// 格式版本（见 [`BACKUP_FORMAT_VERSION`]）
    pub version: i32,
    /// 导出时刻（Unix 秒；纯信息性，导入不依赖它）
    pub exported_at: i64,
    /// 书源集合
    #[serde(default)]
    pub sources: Vec<SourceRow>,
    /// 书源分组
    #[serde(default)]
    pub source_groups: Vec<SourceGroupRow>,
    /// 书架清单
    #[serde(default)]
    pub books: Vec<BookBackup>,
    /// 净化/替换规则
    #[serde(default)]
    pub replace_rules: Vec<ReplaceRuleRow>,
    /// 高亮规则
    #[serde(default)]
    pub highlight_rules: Vec<HighlightRuleRow>,
    /// 词典规则
    #[serde(default)]
    pub dict_rules: Vec<DictRuleRow>,
    /// TXT 目录识别规则
    #[serde(default)]
    pub txt_toc_rules: Vec<TxtTocRuleRow>,
    /// 库根目录（本地书，FR-023）
    #[serde(default)]
    pub library_roots: Vec<LibraryRootRow>,
}

/// 导出/导入结果统计（供报告与测试断言）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackupStats {
    /// 书源数
    pub sources: usize,
    /// 书籍数
    pub books: usize,
    /// 章节总数
    pub chapters: usize,
    /// 进度条数
    pub progress: usize,
    /// 阅读记录数
    pub read_records: usize,
    /// 书签数
    pub bookmarks: usize,
    /// 高亮数
    pub highlights: usize,
    /// 批注数
    pub memos: usize,
    /// 规则总数（四类之和）
    pub rules: usize,
    /// 库根数
    pub library_roots: usize,
}

impl BackupStats {
    /// 逐项求和（用于"导出/导入后统计一致"的断言）。
    #[must_use]
    pub fn total(&self) -> usize {
        self.sources
            + self.books
            + self.chapters
            + self.progress
            + self.read_records
            + self.bookmarks
            + self.highlights
            + self.memos
            + self.rules
            + self.library_roots
    }
}

/// 统计一份备份的条目数（不落库）。
#[must_use]
pub fn stats_of(backup: &Backup) -> BackupStats {
    BackupStats {
        sources: backup.sources.len(),
        books: backup.books.len(),
        chapters: backup.books.iter().map(|entry| entry.chapters.len()).sum(),
        progress: backup
            .books
            .iter()
            .filter(|entry| entry.progress.is_some())
            .count(),
        read_records: backup
            .books
            .iter()
            .map(|entry| entry.read_records.len())
            .sum(),
        bookmarks: backup.books.iter().map(|entry| entry.bookmarks.len()).sum(),
        highlights: backup
            .books
            .iter()
            .map(|entry| entry.highlights.len())
            .sum(),
        memos: backup.books.iter().map(|entry| entry.memos.len()).sum(),
        rules: backup.replace_rules.len()
            + backup.highlight_rules.len()
            + backup.dict_rules.len()
            + backup.txt_toc_rules.len(),
        library_roots: backup.library_roots.len(),
    }
}

// ---------------------------------------------------------------------------
// 读写器
// ---------------------------------------------------------------------------

/// 备份读写器（组合 [`Db`] 之上的各 DAO）。
///
/// # 为什么持四个 DAO 而不是裸 `Db`
///
/// 备份要跨十余张表读写，靠自己拼 SQL 会让"哪些表参与了备份"变成实现细节
/// 而非可读的清单。用既有 DAO 则每张表的读写都走各自已验证的路径
/// （含约束、编码转换、排序），不会出现"备份用的 SQL 与日常用的不一致"。
#[derive(Debug, Clone)]
pub struct BackupStore {
    db: Db,
    now: i64,
}

impl BackupStore {
    /// 以当前时间构造。
    pub fn new(db: Db) -> Self {
        Self {
            db,
            now: crate::now_unix(),
        }
    }

    /// 以指定时间戳构造（测试用，让 `exported_at` 可断言）。
    pub fn with_now(db: Db, now: i64) -> Self {
        Self { db, now }
    }

    /// 底层数据库句柄（供 `delete` 等协作者复用）。
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// 导出**全部**阅读数据（FR-033）。
    pub fn export(&self) -> Result<Backup, StoreError> {
        let source_dao = SourceDao::new(self.db.clone());
        let book_dao = BookDao::new(self.db.clone());
        let trace_dao = TraceDao::with_now(self.db.clone(), self.now);
        let misc_dao = MiscDao::with_now(self.db.clone(), self.now);

        let books = book_dao.bookshelf()?;
        let mut book_backups = Vec::with_capacity(books.len());
        for book in books {
            book_backups.push(self.export_book(&book_dao, &trace_dao, book)?);
        }

        Ok(Backup {
            version: BACKUP_FORMAT_VERSION,
            exported_at: self.now,
            sources: source_dao
                .all()?
                .iter()
                .map(SourceRow::from_source)
                .collect(),
            source_groups: source_dao
                .groups()?
                .iter()
                .map(|group| SourceGroupRow {
                    name: group.name.clone(),
                    sort_order: group.sort_order,
                    enabled: group.enabled,
                })
                .collect(),
            books: book_backups,
            replace_rules: misc_dao
                .replace_rules()?
                .iter()
                .map(|rule| ReplaceRuleRow {
                    name: rule.name.clone(),
                    pattern: rule.pattern.clone(),
                    replacement: rule.replacement.clone(),
                    scope: scope_code(rule.scope).to_string(),
                    is_regex: rule.is_regex,
                    enabled: rule.enabled,
                    sort_order: rule.sort_order,
                })
                .collect(),
            highlight_rules: misc_dao
                .highlight_rules()?
                .iter()
                .map(|rule| HighlightRuleRow {
                    name: rule.name.clone(),
                    pattern: rule.pattern.clone(),
                    style: rule.style.clone(),
                    enabled: rule.enabled,
                    sort_order: rule.sort_order,
                })
                .collect(),
            dict_rules: misc_dao
                .dict_rules()?
                .iter()
                .map(|rule| DictRuleRow {
                    name: rule.name.clone(),
                    url_rule: rule.url_rule.clone(),
                    content_rule: rule.content_rule.clone(),
                    enabled: rule.enabled,
                })
                .collect(),
            txt_toc_rules: misc_dao
                .txt_toc_rules()?
                .iter()
                .map(|rule| TxtTocRuleRow {
                    name: rule.name.clone(),
                    rule: rule.rule.clone(),
                    example: rule.example.clone(),
                    enabled: rule.enabled,
                    sort_order: rule.sort_order,
                })
                .collect(),
            library_roots: book_dao
                .library_roots()?
                .iter()
                .map(|root| LibraryRootRow {
                    path: root.path.clone(),
                    added_at: root.added_at,
                })
                .collect(),
        })
    }

    /// 导出一本书及其全部关联数据。
    fn export_book(
        &self,
        book_dao: &BookDao,
        trace_dao: &TraceDao,
        book: Book,
    ) -> Result<BookBackup, StoreError> {
        let book_id = book.id;
        Ok(BookBackup {
            chapters: book_dao
                .chapters(book_id)?
                .iter()
                .map(ChapterRow::from_chapter)
                .collect(),
            progress: trace_dao.progress(book_id)?.map(|progress| ProgressRow {
                chapter_index: progress.chapter_index,
                char_offset: progress.char_offset,
                anchor_text: progress.anchor_text.clone(),
                updated_at: progress.updated_at,
            }),
            read_records: trace_dao
                .read_records_of(book_id)?
                .iter()
                .map(|record| ReadRecordRow {
                    date: record.date.clone(),
                    read_seconds: record.read_seconds,
                    read_chars: record.read_chars,
                })
                .collect(),
            bookmarks: trace_dao
                .bookmarks(book_id)?
                .iter()
                .map(|bookmark| BookmarkRow {
                    chapter_index: bookmark.chapter_index,
                    char_offset: bookmark.char_offset,
                    note: bookmark.note.clone(),
                    created_at: bookmark.created_at,
                })
                .collect(),
            highlights: trace_dao
                .highlights(book_id)?
                .iter()
                .map(|highlight| HighlightRow {
                    chapter_index: highlight.chapter_index,
                    char_offset: highlight.char_offset,
                    length: highlight.length,
                    style: highlight.style.clone(),
                    note: highlight.note.clone(),
                    created_at: highlight.created_at,
                })
                .collect(),
            memos: trace_dao
                .memos(book_id)?
                .iter()
                .map(|memo| MemoRow {
                    chapter_index: memo.chapter_index,
                    char_offset: memo.char_offset,
                    length: memo.length,
                    content: memo.content.clone(),
                    created_at: memo.created_at,
                })
                .collect(),
            book: BookRow::from_book(&book),
        })
    }

    /// 导出为可读 JSON 文本（FR-033 的"可读、可手工编辑"）。
    pub fn export_json(&self) -> Result<String, StoreError> {
        let backup = self.export()?;
        serde_json::to_string_pretty(&backup).map_err(|error| StoreError::Corrupt {
            subject: "backup".to_string(),
            reason: format!("备份序列化失败：{error}"),
        })
    }

    /// 导入备份（FR-033）。与 [`Self::export`] 对称。
    ///
    /// # 语义：**合并**而非覆盖
    ///
    /// 导入按 id upsert，不先清库。理由：用户可能把两份不同时期的备份
    /// 导入同一台机器（比如"我的书源"+"我的书签"），覆盖会让后导入的
    /// 那份把先前的一切抹掉——那是数据丢失，不是导入。
    ///
    /// 需要"先清空再导入"的场景由调用方先跑 [`crate::delete`]（T091）。
    ///
    /// # 主键重映射
    ///
    /// 书籍主键是本机分配的，导入时重新分配，并把该书的全部关联数据
    /// 一起指到新主键（见 [`BookBackup`] 的说明）。
    pub fn import(&self, backup: &Backup) -> Result<BackupStats, StoreError> {
        if backup.version > BACKUP_FORMAT_VERSION {
            return Err(StoreError::Corrupt {
                subject: "backup".to_string(),
                reason: format!(
                    "备份格式版本 {} 高于本程序支持的 {BACKUP_FORMAT_VERSION}——\
                     请升级本程序后再导入（继续导入可能静默丢字段）",
                    backup.version
                ),
            });
        }

        let source_dao = SourceDao::new(self.db.clone());
        let book_dao = BookDao::new(self.db.clone());
        let trace_dao = TraceDao::with_now(self.db.clone(), self.now);
        let misc_dao = MiscDao::with_now(self.db.clone(), self.now);

        let mut stats = BackupStats {
            ..Default::default()
        };

        // --- 书源分组（先落：书源的 group_id 指向它们）---
        for group in &backup.source_groups {
            source_dao.ensure_group(&group.name, group.sort_order)?;
        }

        // --- 库根（先落：本地书的 library_root_id 指向它们）---
        let mut root_ids: Vec<i64> = Vec::new();
        for root in &backup.library_roots {
            root_ids.push(book_dao.add_library_root(&root.path)?);
            stats.library_roots += 1;
        }
        let library_root_id = root_ids.first().copied();

        // --- 书源 ---
        if !backup.sources.is_empty() {
            let sources: Vec<Source> = backup
                .sources
                .iter()
                .map(|row| row.clone().into_source())
                .collect();
            let report = source_dao.upsert_all(&sources)?;
            stats.sources = report.added + report.updated;
        }

        // --- 规则（主键由存储层重新分配）---
        for rule in &backup.replace_rules {
            misc_dao.save_replace_rule(&ReplaceRule {
                id: 0,
                name: rule.name.clone(),
                pattern: rule.pattern.clone(),
                replacement: rule.replacement.clone(),
                scope: parse_scope(&rule.scope),
                is_regex: rule.is_regex,
                enabled: rule.enabled,
                sort_order: rule.sort_order,
            })?;
        }
        for rule in &backup.highlight_rules {
            misc_dao.save_highlight_rule(&HighlightRule {
                id: 0,
                name: rule.name.clone(),
                pattern: rule.pattern.clone(),
                style: rule.style.clone(),
                enabled: rule.enabled,
                sort_order: rule.sort_order,
            })?;
        }
        for rule in &backup.dict_rules {
            misc_dao.save_dict_rule(&DictRule {
                id: 0,
                name: rule.name.clone(),
                url_rule: rule.url_rule.clone(),
                content_rule: rule.content_rule.clone(),
                enabled: rule.enabled,
            })?;
        }
        for rule in &backup.txt_toc_rules {
            misc_dao.save_txt_toc_rule(&TxtTocRule {
                id: 0,
                name: rule.name.clone(),
                rule: rule.rule.clone(),
                example: rule.example.clone(),
                enabled: rule.enabled,
                sort_order: rule.sort_order,
            })?;
        }
        stats.rules = backup.replace_rules.len()
            + backup.highlight_rules.len()
            + backup.dict_rules.len()
            + backup.txt_toc_rules.len();

        // --- 书籍及其关联数据 ---
        for entry in &backup.books {
            self.import_book(&book_dao, &trace_dao, entry, library_root_id)?;
            stats.books += 1;
            stats.chapters += entry.chapters.len();
            stats.progress += usize::from(entry.progress.is_some());
            stats.read_records += entry.read_records.len();
            stats.bookmarks += entry.bookmarks.len();
            stats.highlights += entry.highlights.len();
            stats.memos += entry.memos.len();
        }

        Ok(stats)
    }

    /// 导入一本书及其全部关联数据，返回新分配的主键。
    fn import_book(
        &self,
        book_dao: &BookDao,
        trace_dao: &TraceDao,
        entry: &BookBackup,
        library_root_id: Option<i64>,
    ) -> Result<i64, StoreError> {
        let origin = if entry.book.origin == "local" {
            BookOrigin::Local
        } else {
            BookOrigin::Network
        };

        // 判重：同一本书导入两次不该出现两条。
        // 判据是 (origin, origin_id)——与 `book` 表的 UNIQUE 约束一致。
        let existing = book_dao.by_origin(origin, &entry.book.origin_id)?;
        let new_id = match existing.as_ref() {
            Some(book) => {
                // 已存在：更新元信息并沿用原主键。
                // 沿用主键是关键——否则关联数据会挂到新 id 上，
                // 原书变成空壳、新书重复一份痕迹。
                let merged = entry.book.into_book(book.id, book.library_root_id);
                book_dao.update_book(&merged)?;
                book.id
            }
            None => {
                let fresh = entry.book.into_book(0, library_root_id);
                book_dao.add_book(&fresh)?
            }
        };

        // 章节：整体重写（目录是"这份备份的目录"，不是增量）
        if !entry.chapters.is_empty() {
            let chapters: Vec<Chapter> = entry
                .chapters
                .iter()
                .map(|chapter| chapter.into_chapter(new_id))
                .collect();
            book_dao.save_chapters(new_id, &chapters)?;
        }

        // 进度
        if let Some(progress) = &entry.progress {
            let _ = trace_dao.save_progress(&ReadProgress {
                book_id: new_id,
                chapter_index: progress.chapter_index,
                char_offset: progress.char_offset,
                anchor_text: progress.anchor_text.clone(),
                updated_at: progress.updated_at,
            });
        }

        // 阅读记录：主键是 (book_id, date)，按日累加写入
        for record in &entry.read_records {
            let _ = trace_dao.add_read_record(&ReadRecord {
                book_id: new_id,
                date: record.date.clone(),
                read_seconds: record.read_seconds,
                read_chars: record.read_chars,
            });
        }

        // 痕迹：主键都是自增，置 0 让存储层重新分配
        for bookmark in &entry.bookmarks {
            let _ = trace_dao.add_bookmark(&Bookmark {
                id: 0,
                book_id: new_id,
                chapter_index: bookmark.chapter_index,
                char_offset: bookmark.char_offset,
                note: bookmark.note.clone(),
                created_at: bookmark.created_at,
            });
        }
        for highlight in &entry.highlights {
            let _ = trace_dao.add_highlight(&Highlight {
                id: 0,
                book_id: new_id,
                chapter_index: highlight.chapter_index,
                char_offset: highlight.char_offset,
                length: highlight.length,
                style: highlight.style.clone(),
                note: highlight.note.clone(),
                created_at: highlight.created_at,
            });
        }
        for memo in &entry.memos {
            let _ = trace_dao.add_memo(&Memo {
                id: 0,
                book_id: new_id,
                chapter_index: memo.chapter_index,
                char_offset: memo.char_offset,
                length: memo.length,
                content: memo.content.clone(),
                created_at: memo.created_at,
            });
        }

        Ok(new_id)
    }

    /// 从 JSON 文本导入（与 [`Self::export_json`] 对称）。
    pub fn import_json(&self, json: &str) -> Result<BackupStats, StoreError> {
        let backup: Backup = serde_json::from_str(json).map_err(|error| StoreError::Corrupt {
            subject: "backup".to_string(),
            reason: format!("备份解析失败：{error}"),
        })?;
        self.import(&backup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::model::{BookOrigin, RuleScope};
    use tome_core::Chapter;

    struct Fixture {
        dir: std::path::PathBuf,
        db: Db,
        store: BackupStore,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-backup-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = Db::open(&dir.join("tome.db")).expect("应能开库");
        Fixture {
            store: BackupStore::with_now(db.clone(), 1_700_000_000),
            db,
            dir,
        }
    }

    /// 造一本有全套个人数据的书。
    fn seed(fixture: &Fixture) {
        let book_dao = BookDao::new(fixture.db.clone());
        let trace_dao = TraceDao::with_now(fixture.db.clone(), 1_700_000_000);
        let source_dao = SourceDao::new(fixture.db.clone());
        let misc_dao = MiscDao::with_now(fixture.db.clone(), 1_700_000_000);

        source_dao.ensure_group("我的源", 0).expect("应能建分组");
        source_dao
            .upsert_all(&[Source {
                id: "https://src.example.com".to_string(),
                name: "示例源".to_string(),
                kind: SourceKind::Book,
                group_id: None,
                enabled: true,
                book_source_type: None,
                book_url_pattern: None,
                search_url: Some("/s?q={{key}}".to_string()),
                explore_url: None,
                rule_search: Some(r#"{"bookList":"@css:li"}"#.to_string()),
                rule_explore: None,
                rule_book_info: None,
                rule_toc: None,
                rule_content: None,
                header: None,
                login_url: None,
                login_ui: None,
                login_check_js: None,
                js: None,
                charset: None,
                rate_limit_ms: Some(1000),
                weight: None,
                explore_screen: None,
                comment: Some("备份往返测试".to_string()),
                last_update_time: None,
                created_at: 1_700_000_000,
            }])
            .expect("应能存源");

        misc_dao
            .save_replace_rule(&ReplaceRule {
                id: 0,
                name: "去广告".to_string(),
                pattern: "广告.*".to_string(),
                replacement: String::new(),
                scope: RuleScope::Content,
                is_regex: true,
                enabled: true,
                sort_order: 1,
            })
            .expect("应能存替换规则");
        misc_dao
            .save_highlight_rule(&HighlightRule {
                id: 0,
                name: "人名高亮".to_string(),
                pattern: "主角".to_string(),
                style: "yellow".to_string(),
                enabled: true,
                sort_order: 0,
            })
            .expect("应能存高亮规则");
        misc_dao
            .save_dict_rule(&DictRule {
                id: 0,
                name: "示例词典".to_string(),
                url_rule: "https://dict.example.com/{{key}}".to_string(),
                content_rule: "@css:.def@text".to_string(),
                enabled: true,
            })
            .expect("应能存词典规则");
        misc_dao
            .save_txt_toc_rule(&TxtTocRule {
                id: 0,
                name: "第N章".to_string(),
                rule: "^第.+章".to_string(),
                example: Some("第一章 起点".to_string()),
                enabled: true,
                sort_order: 0,
            })
            .expect("应能存目录规则");

        let book_id = book_dao
            .add_book(&Book {
                id: 0,
                origin: BookOrigin::Network,
                origin_id: "https://src.example.com|https://src.example.com/book/1".to_string(),
                name: "备份测试书".to_string(),
                author: Some("作者甲".to_string()),
                cover_url: Some("https://src.example.com/c.jpg".to_string()),
                intro: Some("简介".to_string()),
                kind: Some("玄幻".to_string()),
                book_group: Some("默认".to_string()),
                read_config: None,
                toc_url: Some("https://src.example.com/book/1".to_string()),
                latest_chapter_title: Some("第一章".to_string()),
                latest_chapter_time: Some(1_700_000_000),
                library_root_id: None,
                relative_path: None,
                charset: None,
                format: None,
                total_chapters: Some(2),
                dur_chapter_index: Some(0),
                added_at: 1_700_000_000,
            })
            .expect("应能加书");

        book_dao
            .save_chapters(
                book_id,
                &[
                    Chapter {
                        id: 0,
                        book_id,
                        index_in_book: 0,
                        title: "第一章".to_string(),
                        url: Some("https://src.example.com/c/1".to_string()),
                        byte_offset: None,
                        epub_entry: None,
                        is_cached: false,
                        is_read: true,
                        word_count: Some(100),
                    },
                    Chapter {
                        id: 0,
                        book_id,
                        index_in_book: 1,
                        title: "第二章".to_string(),
                        url: Some("https://src.example.com/c/2".to_string()),
                        byte_offset: None,
                        epub_entry: None,
                        is_cached: false,
                        is_read: false,
                        word_count: Some(200),
                    },
                ],
            )
            .expect("应能存章节");

        trace_dao
            .save_progress(&ReadProgress {
                book_id,
                chapter_index: 1,
                char_offset: 42,
                anchor_text: Some("锚点文本".to_string()),
                updated_at: 1_700_000_000,
            })
            .expect("应能存进度");
        trace_dao
            .add_read_record(&ReadRecord {
                book_id,
                date: "2026-10-11".to_string(),
                read_seconds: 600,
                read_chars: 5000,
            })
            .expect("应能存阅读记录");
        trace_dao
            .add_bookmark(&Bookmark {
                id: 0,
                book_id,
                chapter_index: 0,
                char_offset: 10,
                note: Some("这里".to_string()),
                created_at: 1_700_000_000,
            })
            .expect("应能存书签");
        trace_dao
            .add_highlight(&Highlight {
                id: 0,
                book_id,
                chapter_index: 0,
                char_offset: 5,
                length: 3,
                style: "yellow".to_string(),
                note: None,
                created_at: 1_700_000_000,
            })
            .expect("应能存高亮");
        trace_dao
            .add_memo(&Memo {
                id: 0,
                book_id,
                chapter_index: 1,
                char_offset: 20,
                length: 5,
                content: "笔记内容".to_string(),
                created_at: 1_700_000_000,
            })
            .expect("应能存批注");

        book_dao.add_library_root("/tmp/books").expect("应能存库根");
    }

    /// 清空全部用户数据表（模拟"换台机器/清空后恢复"）。
    fn clear_all(db: &Db) {
        db.write(|connection| {
            for table in [
                "book",
                "chapter",
                "read_progress",
                "read_record",
                "bookmark",
                "highlight",
                "memo",
                "source",
                "source_group",
                "replace_rule",
                "highlight_rule",
                "dict_rule",
                "txt_toc_rule",
                "library_root",
            ] {
                connection
                    .execute(&format!("DELETE FROM {table}"), [])
                    .map_err(crate::db::sql)?;
            }
            Ok(())
        })
        .expect("应能清库");
    }

    #[test]
    fn 导出包含全部七类阅读数据() {
        let fixture = fixture("export-all");
        seed(&fixture);

        let backup = fixture.store.export().expect("应能导出");
        assert_eq!(backup.version, BACKUP_FORMAT_VERSION);
        assert_eq!(backup.exported_at, 1_700_000_000);

        assert_eq!(backup.sources.len(), 1, "书源");
        assert_eq!(backup.source_groups.len(), 1, "书源分组");
        assert_eq!(backup.books.len(), 1, "书架");
        assert_eq!(backup.books[0].chapters.len(), 2, "目录");
        assert!(backup.books[0].progress.is_some(), "进度");
        assert_eq!(backup.books[0].read_records.len(), 1, "阅读记录");
        assert_eq!(backup.books[0].bookmarks.len(), 1, "书签");
        assert_eq!(backup.books[0].highlights.len(), 1, "高亮");
        assert_eq!(backup.books[0].memos.len(), 1, "批注");
        assert_eq!(backup.replace_rules.len(), 1, "净化规则");
        assert_eq!(backup.highlight_rules.len(), 1, "高亮规则");
        assert_eq!(backup.dict_rules.len(), 1, "词典规则");
        assert_eq!(backup.txt_toc_rules.len(), 1, "目录识别规则");
        assert_eq!(backup.library_roots.len(), 1, "库根");
    }

    #[test]
    fn 往返导出清空导入后数据一致() {
        // 这是 FR-033「对称」的核心验收：导出→清空→导入→与原数据一致
        let fixture = fixture("roundtrip");
        seed(&fixture);

        let before = fixture.store.export().expect("应能导出");
        let json = fixture.store.export_json().expect("应能出 JSON");

        clear_all(&fixture.db);
        let empty_store = BackupStore::with_now(fixture.db.clone(), 1_700_000_000);
        assert_eq!(
            empty_store.export().expect("应能导出空库").books.len(),
            0,
            "清空后应无书"
        );

        let stats = empty_store.import_json(&json).expect("应能导入");
        assert_eq!(stats.sources, 1);
        assert_eq!(stats.books, 1);
        assert_eq!(stats.chapters, 2);
        assert_eq!(stats.progress, 1);
        assert_eq!(stats.read_records, 1);
        assert_eq!(stats.bookmarks, 1);
        assert_eq!(stats.highlights, 1);
        assert_eq!(stats.memos, 1);

        let after = empty_store.export().expect("应能再导出");
        assert_eq!(
            after, before,
            "往返后应完全一致（FR-033「能导出即可完整导回」）"
        );
    }

    #[test]
    fn 往返统计一致() {
        let fixture = fixture("stats");
        seed(&fixture);
        let json = fixture.store.export_json().expect("应能导出");

        let parsed: Backup = serde_json::from_str(&json).expect("应是合法 JSON");
        let stats = stats_of(&parsed);
        assert_eq!(stats.sources, 1);
        assert_eq!(stats.books, 1);
        assert_eq!(stats.chapters, 2);
        assert!(stats.total() > 0);
    }

    #[test]
    fn 痕迹的字符区间在往返后保持() {
        // FR-028：痕迹以字符区间表达，往返不得改变区间
        let fixture = fixture("char-range");
        seed(&fixture);
        let before = fixture.store.export().expect("应能导出");

        let json = serde_json::to_string(&before).expect("应能序列化");
        let restored: Backup = serde_json::from_str(&json).expect("应能反序列化");

        let original = &before.books[0];
        let roundtrip = &restored.books[0];
        assert_eq!(
            roundtrip.highlights[0].char_offset,
            original.highlights[0].char_offset
        );
        assert_eq!(
            roundtrip.highlights[0].length,
            original.highlights[0].length
        );
        assert_eq!(
            roundtrip.memos[0].char_offset,
            original.memos[0].char_offset
        );
        assert_eq!(roundtrip.memos[0].length, original.memos[0].length);
        assert_eq!(
            roundtrip
                .progress
                .as_ref()
                .map(|p| (p.chapter_index, p.char_offset)),
            original
                .progress
                .as_ref()
                .map(|p| (p.chapter_index, p.char_offset))
        );
        assert_eq!(
            roundtrip
                .progress
                .as_ref()
                .and_then(|p| p.anchor_text.clone()),
            original
                .progress
                .as_ref()
                .and_then(|p| p.anchor_text.clone()),
            "锚点文本是换源降级链的输入，往返不得丢"
        );
    }

    #[test]
    fn 高版本备份被拒绝而非静默误解() {
        let fixture = fixture("version");
        let future = Backup {
            version: BACKUP_FORMAT_VERSION + 1,
            exported_at: 0,
            sources: Vec::new(),
            source_groups: Vec::new(),
            books: Vec::new(),
            replace_rules: Vec::new(),
            highlight_rules: Vec::new(),
            dict_rules: Vec::new(),
            txt_toc_rules: Vec::new(),
            library_roots: Vec::new(),
        };
        let error = fixture.store.import(&future).expect_err("高版本应被拒绝");
        let message = format!("{error}");
        assert!(message.contains("版本"), "错误应说明是版本问题：{message}");
    }

    #[test]
    fn 非法_json_报错而非静默空导入() {
        let fixture = fixture("badjson");
        let error = fixture
            .store
            .import_json("{ 这不是 JSON")
            .expect_err("非法 JSON 应报错");
        assert!(format!("{error}").contains("解析失败"), "{error}");
    }

    #[test]
    fn 导出不含凭据与缓存标记() {
        // 导出里不该出现 cookie / http_log / is_cached——前者是凭据，
        // 后者是可重建的派生数据，都不在 FR-033 的七类里
        let fixture = fixture("no-secrets");
        seed(&fixture);
        let json = fixture.store.export_json().expect("应能导出");

        for forbidden in ["cookie", "http_log", "password", "token", "is_cached"] {
            assert!(!json.contains(forbidden), "导出不该含 `{forbidden}`");
        }
    }

    #[test]
    fn 导入同一备份两次不产生重复书籍() {
        let fixture = fixture("idempotent");
        seed(&fixture);
        let json = fixture.store.export_json().expect("应能导出");

        fixture.store.import_json(&json).expect("第一次导入");
        fixture.store.import_json(&json).expect("第二次导入");

        let backup = fixture.store.export().expect("应能导出");
        assert_eq!(
            backup.books.len(),
            1,
            "同一本书按 (origin, origin_id) 判重，导入两次仍应只有一本"
        );
    }

    #[test]
    fn json_是人可读的() {
        // FR-032/FR-033 要求"可读、可手工编辑"
        let fixture = fixture("readable");
        seed(&fixture);
        let json = fixture.store.export_json().expect("应能导出");

        assert!(json.contains('\n'), "应是格式化的多行 JSON");
        assert!(json.contains("备份测试书"), "应含可读的中文内容");
        assert!(json.contains("\"version\""), "应含版本字段");
        assert!(json.contains("\"sources\""), "字段名应是业务语义而非表名");
    }

    #[test]
    fn 空库导出不报错() {
        let fixture = fixture("empty");
        let backup = fixture.store.export().expect("空库应能导出");
        assert_eq!(backup.books.len(), 0);
        assert_eq!(backup.sources.len(), 0);
        assert_eq!(stats_of(&backup).total(), 0);
    }

    #[test]
    fn 源类型与规则作用域以可读字符串往返() {
        // 用字符串而非整数编码，让备份可手工编辑（FR-033）
        let fixture = fixture("codes");
        seed(&fixture);
        let json = fixture.store.export_json().expect("应能导出");

        assert!(json.contains("\"book\""), "源类型应是可读字符串");
        assert!(json.contains("\"content\""), "规则作用域应是可读字符串");
        // 不该出现存储层的整数编码
        assert!(!json.contains("\"kind\": 0"), "源类型不该用工整数编码");
    }
}
