//! 资产类 DTO（data-model.md 的 `cookie` / `image_cache` 表）

/// 按源落盘的登录态（data-model.md 的 `cookie` 表，FR-026 / Q6）。
///
/// ## 安全约定
///
/// `value` 是敏感数据：AGENTS.md「安全红线」要求"禁止在日志记录敏感数据
/// （Cookie、认证头须脱敏）"。本 DTO **不实现 `Display`**，正是为了让
/// `println!("{cookie}")` 这类写法无法编译——脱敏是各实现 crate 在日志
/// 边界的责任，而 DTO 层面先堵死"顺手打印"的路径。
///
/// `Debug` 仍然派生（调试需要），但实现方在日志中记录本类型时必须显式脱敏。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    /// 所属书源（与 name/domain 共同构成主键）
    pub source_id: String,
    /// Cookie 名
    pub name: String,
    /// Cookie 值（**敏感**，日志中须脱敏）
    pub value: String,
    /// 作用域域名
    pub domain: Option<String>,
    /// 过期时间戳
    pub expires_at: Option<i64>,
}

/// 图片缓存的索引条目（data-model.md 的 `image_cache` 表）。
///
/// 文件落在 `cache/images/<xx>/<hash>`，索引记录 URL → 落盘路径的映射。
/// SC-017：图片缓存与章节缓存是**两个可独立清理**的资源，故此处不与
/// 章节缓存共用类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCacheRef {
    /// 图片 URL（主键）
    pub url: String,
    /// 来源书源（可空：本地书的封面等无来源书源）
    pub source_id: Option<String>,
    /// 落盘相对路径
    pub path: String,
    /// 抓取时间戳（供 LRU 淘汰）
    pub fetched_at: i64,
}
