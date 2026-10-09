//! 阅读痕迹 DTO（data-model.md 的 `read_progress` / `read_record` /
//! `bookmark` / `highlight` / `memo` 表）
//!
//! # 本模块是 FR-028 的落点
//!
//! **MUST NOT 以渲染坐标存储任何阅读痕迹**（FR-028）。本模块所有位置字段
//! 一律是 `chapter_index` + `char_offset` + `length` 的**字符区间**：
//!
//! - `chapter_index` —— 书内章节序号（0 起）
//! - `char_offset` —— **章内 Unicode 字符偏移**（不是字节偏移；中文正文下两者差 3 倍）
//! - `length` —— **字符数**（仅高亮/批注有长度；书签是单点）
//!
//! 这是"改字号/改窗口后不错位"的前提（SC-008）：排版参数变化只影响
//! 字符→像素的映射，不影响字符区间本身。

/// 阅读进度（data-model.md 的 `read_progress` 表）。
///
/// ## 锚点三要素
///
/// `(chapter_index, char_offset)` 定位 + `anchor_text` 语义校验。FR-027 要求
/// 以**与排版无关的锚点**保存进度；FR-010b 要求换源时用 `anchor_text` 做
/// 特征段匹配（在新源邻域章节缓存内按约 90% 相似度搜索）。
///
/// 故 `anchor_text` 不是可选装饰，而是换源降级链第一级的输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadProgress {
    /// 书籍（存储层保证一册一条，故此处即主键）
    pub book_id: i64,
    /// 章内序号（0 起）
    pub chapter_index: i32,
    /// 章内字符偏移
    pub char_offset: i64,
    /// 锚点处的文本片段（换源特征段匹配用，FR-010b）
    pub anchor_text: Option<String>,
    /// 更新时间戳
    pub updated_at: i64,
}

/// 阅读记录（data-model.md 的 `read_record` 表，FR-026b）。
///
/// 主键是 `(book_id, date)` —— 按天聚合。FR-026b 明确"数据层的记录行为
/// MUST 不依赖任何界面（无界面下同样落库）"，故本 DTO 不含任何界面关切，
/// 也不含"是否已展示统计"之类的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRecord {
    /// 书籍
    pub book_id: i64,
    /// 日期 `YYYY-MM-DD`（用字符串而非时间戳：聚合粒度就是"天"，
    /// 字符串可直接作主键且有可读性）
    pub date: String,
    /// 当日阅读秒数
    pub read_seconds: i64,
    /// 当日阅读字数
    pub read_chars: i64,
}

/// 书签（data-model.md 的 `bookmark` 表）—— 单点 + 备注。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    /// 主键
    pub id: i64,
    /// 书籍
    pub book_id: i64,
    /// 章内序号
    pub chapter_index: i32,
    /// 章内字符偏移
    pub char_offset: i64,
    /// 备注
    pub note: Option<String>,
    /// 创建时间戳
    pub created_at: i64,
}

/// 高亮（data-model.md 的 `highlight` 表）—— 字符区间 + 样式 + 备注。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    /// 主键
    pub id: i64,
    /// 书籍
    pub book_id: i64,
    /// 章内序号
    pub chapter_index: i32,
    /// 区间起点的章内字符偏移
    pub char_offset: i64,
    /// 区间长度（**字符数**）
    pub length: i64,
    /// 样式标识（渲染方式由 S2 排版层解释）
    pub style: String,
    /// 备注
    pub note: Option<String>,
    /// 创建时间戳
    pub created_at: i64,
}

/// 批注（data-model.md 的 `memo` 表）—— 文本区间 + 笔记内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memo {
    /// 主键
    pub id: i64,
    /// 书籍
    pub book_id: i64,
    /// 章内序号
    pub chapter_index: i32,
    /// 区间起点的章内字符偏移
    pub char_offset: i64,
    /// 区间长度（**字符数**）
    pub length: i64,
    /// 笔记内容
    pub content: String,
    /// 创建时间戳
    pub created_at: i64,
}
