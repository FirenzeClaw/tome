//! 辅助实体的 DTO（data-model.md 的 `chapter_cache` / `http_log` / `search_keyword` 表）
//!
//! 这三个实体不直接参与书源兼容语义，但在 data-model.md 的 18 实体清单内，
//! 故同样落为 DTO——存储层据此建表，避免日后各实现 crate 各写一份形状。

/// 章节缓存容器索引（data-model.md 的 `chapter_cache` 表）。
///
/// ## 它是"索引"而非"内容"
///
/// 内容本体按顺序追加在 Pack 文件里，本 DTO 只记录**在哪**（`pack_id`、`offset`、
/// `length` 三者）。这样拆分的理由是 FR-029：批量容器减少文件数，而索引仍可被
/// SQLite 快速查询。
///
/// ## 崩溃安全的顺序约束（FR-030 / AGENTS.md 生产级护栏）
///
/// **内容追加落盘并 fsync 必须先于**本索引行的事务提交。反过来会产生悬空引用
/// （索引指向不存在的内容）。此约束由实现方（tome-store）保证；DTO 层面记录
/// `cached_at` 供 LRU 淘汰（FR-031）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterCacheEntry {
    /// 章节（存储层保证一章节至多一条索引，故此处即主键）
    pub chapter_id: i64,
    /// 所属书籍（冗余，避免按书淘汰时 join chapter 表）
    pub book_id: i64,
    /// 所属容器（写入顺序分配）
    pub pack_id: i64,
    /// 容器内偏移
    pub offset: i64,
    /// 内容长度
    pub length: i64,
    /// 写入时间戳（淘汰按此 LRU，FR-031）
    pub cached_at: i64,
}

/// 请求/响应日志条目（data-model.md 的 `http_log` 表，FR-017）。
///
/// ## 安全约定
///
/// `request_headers` 与 `response_body` 都可能含敏感数据（Cookie、认证头）。
/// AGENTS.md「安全红线」要求日志中**必须脱敏**。故本 DTO 不实现 `Display`，
/// 且字段名带 `_redacted` 后缀以提醒实现方：写入前须完成脱敏。
///
/// ## 滚动清理（FR-017）
///
/// 日志须"可清理且不无限增长"。本 DTO 不含清理策略——那是实现方的关切，
/// 此处只承载一条日志的形状。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpLogEntry {
    /// 主键
    pub id: i64,
    /// 来源书源（可空：非书源发起的请求）
    pub source_id: Option<String>,
    /// 请求地址
    pub url: String,
    /// 请求方法
    pub method: String,
    /// 请求头（**已脱敏**的 JSON 文本；写入前须移除 Cookie/认证头）
    pub request_headers_redacted: Option<String>,
    /// 响应状态码
    pub response_code: Option<u16>,
    /// 响应体（可能已截断；同样须先脱敏）
    pub response_body_redacted: Option<String>,
    /// 往返耗时，毫秒
    pub duration_ms: Option<i64>,
    /// 记录时间戳
    pub created_at: i64,
}

/// 收藏的搜索关键词（data-model.md 的 `search_keyword` 表，FR-018）。
///
/// data-model.md 未给出字段明细（只注明"收藏检索词"），故此处取满足 FR-018
/// 「搜索关键词的管理（收藏与复用）」的最小集合：词本身 + 收藏时间 + 使用次数。
/// 使用次数用于"常用词置顶"，属 FR-018 的"复用"语义的自然延伸。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchKeyword {
    /// 主键
    pub id: i64,
    /// 关键词文本
    pub keyword: String,
    /// 收藏时间戳
    pub created_at: i64,
    /// 被复用次数（供排序）
    pub use_count: i64,
}
