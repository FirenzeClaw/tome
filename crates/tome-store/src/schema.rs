//! [DONE] v1 schema 与迁移脚本（tasks.md T053 / `data-model.md`）
//!
//! # 迁移机制
//!
//! `PRAGMA user_version` 记版本号，[`MIGRATIONS`] 是内嵌 SQL 脚本数组——
//! 数组下标 + 1 即该脚本产出的版本。升级时从当前版本顺序执行到最新，
//! 全程在一个事务里（中途失败整体回滚，不留半迁移状态）。
//!
//! ## 为什么不引第三方迁移框架
//!
//! `data-model.md`「Schema 治理」明确要求"迁移器 = `PRAGMA user_version`
//! + 内嵌 SQL 脚本数组（~100 行，不引第三方框架）"。本模块正是这一条的实现。
//!
//! 迁移的全部逻辑就是"取版本 → 顺序跑脚本 → 写回版本"，没有需要框架解决的
//! 复杂度：无 down 迁移、无并发迁移、无多数据库方言。
//!
//! # v1 表清单（`data-model.md` 的 18 实体）
//!
//! | # | 表 | 实体 |
//! |---|---|---|
//! | 1 | `source` | 书源（统一表，`kind` 区分四类源） |
//! | 2 | `source_group` | 书源分组 |
//! | 3 | `book` | 书籍 |
//! | 4 | `chapter` | 章节 |
//! | 5 | `chapter_cache` | 章节缓存容器索引 |
//! | 6 | `read_progress` | 阅读进度（锚点） |
//! | 7 | `read_record` | 阅读记录 |
//! | 8 | `bookmark` | 书签 |
//! | 9 | `highlight` | 高亮 |
//! | 10 | `memo` | 批注 |
//! | 11 | `replace_rule` | 替换/净化规则 |
//! | 12 | `highlight_rule` | 高亮规则 |
//! | 13 | `dict_rule` | 词典规则 |
//! | 14 | `txt_toc_rule` | 目录识别规则 |
//! | 15 | `search_keyword` | 搜索关键词 |
//! | 16 | `http_log` | 请求日志 |
//! | 17 | `cookie` | 按源持久化的登录态 |
//! | 18 | `library_root` | 库根目录 |
//!
//! ## 两张 `data-model.md` 未列入实体表但 v1 必需的辅助表
//!
//! `data-model.md` 的「图片缓存」一节描述了 `image_cache(url PK, source_id,
//! path, fetched_at)` 索引表（属文件系统 + 索引的设计，未计入 18 实体），
//! 以及 JS 宿主 `cache_get`/`cache_put` 需要一个通用键值表
//! （`StoreHost` 契约在 `seams.md` §4，但没规定落哪张表）。二者都是 v1 必需：
//!
//! - `image_cache`：T061 的索引落点（SC-017 要求它与章节缓存各自可清）
//! - `script_cache`：JS 脚本缓存（`java.cachePut`/`cacheGet`）的落点，
//!   按 `source_id` 隔离——**跨源共享缓存会把 A 源的登录 token 漏给 B 源**
//!
//! 加表不改任何既有实体的形状，故不构成对 `data-model.md` 的偏离。

/// 本程序支持的最高 schema 版本。
///
/// 与 [`MIGRATIONS`] 的长度绑定（`debug_assert` 守着）。数据目录里的版本高于
/// 它时，迁移器**拒绝打开**而不是尽力而为——旧程序用旧 schema 读新表结构会
/// 产生静默数据损坏（见 [`crate::error::StoreError::Migration`] 的说明）。
pub const SCHEMA_VERSION: i64 = 1;

/// v1 schema。
///
/// # 若干字段的类型选择
///
/// - **时间戳一律 `INTEGER`（Unix 秒）**：SQLite 无原生日期类型，用整数让
///   比较与排序直接可用；`read_record.date` 是唯一例外，它用 `YYYY-MM-DD`
///   字符串（聚合粒度就是"天"，字符串可直接作主键且有可读性）。
/// - **布尔一律 `INTEGER` 0/1**：SQLite 无 `BOOLEAN` 类型，`CHECK` 约束把取值
///   钉死在 0/1，避免出现"2"这种既非真也非假的值。
/// - **规则字段一律 `TEXT`（JSON）**：`data-model.md` 明确"规则结构会演进，
///   不值得拆表"。
///
/// # 外键为什么开 `ON DELETE CASCADE`
///
/// `book` 删除时其章节、进度、痕迹、缓存索引都必须一起消失（接缝的
/// `remove_book` 契约："移除书籍（含其章节、痕迹、进度）"）。用数据库层的
/// 级联而不是应用层逐表删除：后者漏一张表就是永久残留，而那是静默的。
///
/// 注意 SQLite 的外键默认**关闭**，须每条连接 `PRAGMA foreign_keys=ON`
/// （见 [`crate::db`]）。
const V1_SCHEMA: &str = r#"
-- ---------------------------------------------------------------------------
-- 书源与分组
-- ---------------------------------------------------------------------------

CREATE TABLE source_group (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL UNIQUE,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
);

CREATE TABLE source (
    -- 主键是书源 URL 或 JS 源标识（`data-model.md`：id 唯一）
    id                TEXT    PRIMARY KEY,
    name              TEXT    NOT NULL,
    kind              INTEGER NOT NULL DEFAULT 0,
    group_id          INTEGER REFERENCES source_group(id) ON DELETE SET NULL,
    enabled           INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    book_source_type  INTEGER,
    book_url_pattern  TEXT,
    search_url        TEXT,
    explore_url       TEXT,
    rule_search       TEXT,
    rule_explore      TEXT,
    rule_book_info    TEXT,
    rule_toc          TEXT,
    rule_content      TEXT,
    header            TEXT,
    login_url         TEXT,
    login_ui          TEXT,
    login_check_js    TEXT,
    js                TEXT,
    charset           TEXT,
    rate_limit_ms     INTEGER,
    weight            INTEGER,
    explore_screen    TEXT,
    comment           TEXT,
    last_update_time  INTEGER,
    created_at        INTEGER NOT NULL
);

CREATE INDEX idx_source_group   ON source(group_id);
CREATE INDEX idx_source_enabled ON source(enabled);

-- ---------------------------------------------------------------------------
-- 书籍 / 章节 / 缓存索引 / 库根
-- ---------------------------------------------------------------------------

CREATE TABLE library_root (
    id       INTEGER PRIMARY KEY,
    -- 绝对路径唯一（`data-model.md`）：同一个库根添加两次是用户误操作，
    -- 不该产生两条记录（否则扫描会做两遍、书目会重复）
    path     TEXT    NOT NULL UNIQUE,
    added_at INTEGER NOT NULL
);

CREATE TABLE book (
    id                    INTEGER PRIMARY KEY,
    origin                INTEGER NOT NULL CHECK (origin IN (0, 1)),
    -- 网络书为「书源id|书籍详情URL」，本地书为归一化内容哈希
    origin_id             TEXT    NOT NULL,
    name                  TEXT    NOT NULL,
    author                TEXT,
    cover_url             TEXT,
    intro                 TEXT,
    kind                  TEXT,
    book_group            TEXT,
    read_config           TEXT,
    toc_url               TEXT,
    latest_chapter_title  TEXT,
    latest_chapter_time   INTEGER,
    library_root_id       INTEGER REFERENCES library_root(id) ON DELETE SET NULL,
    relative_path         TEXT,
    charset               TEXT,
    format                TEXT,
    total_chapters        INTEGER,
    dur_chapter_index     INTEGER,
    added_at              INTEGER NOT NULL,
    -- 身份规则（`data-model.md`）：网络书以 (书源id, 详情URL) 唯一，
    -- 本地书以内容哈希唯一。移动文件不产生重复书目。
    UNIQUE (origin, origin_id)
);

CREATE INDEX idx_book_library_root ON book(library_root_id);
CREATE INDEX idx_book_added_at     ON book(added_at);

CREATE TABLE chapter (
    id            INTEGER PRIMARY KEY,
    book_id       INTEGER NOT NULL REFERENCES book(id) ON DELETE CASCADE,
    index_in_book INTEGER NOT NULL,
    title         TEXT    NOT NULL,
    url           TEXT,
    byte_offset   INTEGER,
    epub_entry    TEXT,
    is_cached     INTEGER NOT NULL DEFAULT 0 CHECK (is_cached IN (0, 1)),
    is_read       INTEGER NOT NULL DEFAULT 0 CHECK (is_read IN (0, 1)),
    word_count    INTEGER,
    UNIQUE (book_id, index_in_book)
);

CREATE INDEX idx_chapter_book ON chapter(book_id, index_in_book);

CREATE TABLE chapter_cache (
    chapter_id INTEGER PRIMARY KEY REFERENCES chapter(id) ON DELETE CASCADE,
    book_id    INTEGER NOT NULL REFERENCES book(id) ON DELETE CASCADE,
    pack_id    INTEGER NOT NULL,
    offset     INTEGER NOT NULL,
    length     INTEGER NOT NULL,
    cached_at  INTEGER NOT NULL
);

-- 淘汰按容器粒度做（FR-031），故 (pack_id) 必须有索引；
-- 按书清理缓存（清缓存不伤个人数据，SC-015）走 (book_id)。
CREATE INDEX idx_chapter_cache_pack ON chapter_cache(pack_id);
CREATE INDEX idx_chapter_cache_book ON chapter_cache(book_id);

-- 图片缓存索引（`data-model.md`「图片缓存」）：文件在 cache/images/<xx>/<hash>
CREATE TABLE image_cache (
    url        TEXT    PRIMARY KEY,
    source_id  TEXT,
    path       TEXT    NOT NULL,
    fetched_at INTEGER NOT NULL
);

CREATE INDEX idx_image_cache_source  ON image_cache(source_id);
CREATE INDEX idx_image_cache_fetched ON image_cache(fetched_at);

-- ---------------------------------------------------------------------------
-- 阅读进度 / 记录 / 痕迹（一律字符区间，FR-028）
-- ---------------------------------------------------------------------------

CREATE TABLE read_progress (
    -- 一册一条，故 book_id 即主键（`data-model.md`）
    book_id       INTEGER PRIMARY KEY REFERENCES book(id) ON DELETE CASCADE,
    chapter_index INTEGER NOT NULL,
    char_offset   INTEGER NOT NULL,
    anchor_text   TEXT,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE read_record (
    book_id      INTEGER NOT NULL REFERENCES book(id) ON DELETE CASCADE,
    date         TEXT    NOT NULL,
    read_seconds INTEGER NOT NULL DEFAULT 0,
    read_chars   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (book_id, date)
);

CREATE INDEX idx_read_record_date ON read_record(date);

CREATE TABLE bookmark (
    id            INTEGER PRIMARY KEY,
    book_id       INTEGER NOT NULL REFERENCES book(id) ON DELETE CASCADE,
    chapter_index INTEGER NOT NULL,
    char_offset   INTEGER NOT NULL,
    note          TEXT,
    created_at    INTEGER NOT NULL
);

CREATE INDEX idx_bookmark_book ON bookmark(book_id, chapter_index);

CREATE TABLE highlight (
    id            INTEGER PRIMARY KEY,
    book_id       INTEGER NOT NULL REFERENCES book(id) ON DELETE CASCADE,
    chapter_index INTEGER NOT NULL,
    char_offset   INTEGER NOT NULL,
    -- 长度单位是**Unicode 字符数**（`model/trace.rs`）：中文正文下与字节数
    -- 差 3 倍，混淆会导致高亮整体偏移
    length        INTEGER NOT NULL CHECK (length >= 0),
    style         TEXT    NOT NULL,
    note          TEXT,
    created_at    INTEGER NOT NULL
);

CREATE INDEX idx_highlight_book ON highlight(book_id, chapter_index);

CREATE TABLE memo (
    id            INTEGER PRIMARY KEY,
    book_id       INTEGER NOT NULL REFERENCES book(id) ON DELETE CASCADE,
    chapter_index INTEGER NOT NULL,
    char_offset   INTEGER NOT NULL,
    length        INTEGER NOT NULL CHECK (length >= 0),
    content       TEXT    NOT NULL,
    created_at    INTEGER NOT NULL
);

CREATE INDEX idx_memo_book ON memo(book_id, chapter_index);

-- ---------------------------------------------------------------------------
-- 规则四类
-- ---------------------------------------------------------------------------

CREATE TABLE replace_rule (
    id          INTEGER PRIMARY KEY,
    name        TEXT    NOT NULL,
    pattern     TEXT    NOT NULL,
    replacement TEXT    NOT NULL,
    scope       INTEGER NOT NULL DEFAULT 0,
    is_regex    INTEGER NOT NULL DEFAULT 0 CHECK (is_regex IN (0, 1)),
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_replace_rule_order ON replace_rule(enabled, sort_order);

CREATE TABLE highlight_rule (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    pattern    TEXT    NOT NULL,
    style      TEXT    NOT NULL,
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_highlight_rule_order ON highlight_rule(enabled, sort_order);

CREATE TABLE dict_rule (
    id           INTEGER PRIMARY KEY,
    name         TEXT    NOT NULL,
    url_rule     TEXT    NOT NULL,
    content_rule TEXT    NOT NULL,
    enabled      INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
);

CREATE TABLE txt_toc_rule (
    id         INTEGER PRIMARY KEY,
    name       TEXT    NOT NULL,
    rule       TEXT    NOT NULL,
    example    TEXT,
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    -- FR-020：多条规则回退，sort_order 即尝试顺序
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_txt_toc_rule_order ON txt_toc_rule(enabled, sort_order);

-- ---------------------------------------------------------------------------
-- 关键词 / 日志 / Cookie / 脚本缓存
-- ---------------------------------------------------------------------------

CREATE TABLE search_keyword (
    id         INTEGER PRIMARY KEY,
    keyword    TEXT    NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    use_count  INTEGER NOT NULL DEFAULT 0
);

-- 使用次数降序是"常用词置顶"（FR-018 的复用语义）的排序依据
CREATE INDEX idx_search_keyword_use ON search_keyword(use_count DESC);

CREATE TABLE http_log (
    id                       INTEGER PRIMARY KEY,
    source_id                TEXT,
    url                      TEXT    NOT NULL,
    method                   TEXT    NOT NULL,
    -- 敏感头须先脱敏再写入（AGENTS.md 安全红线 / FR-017）
    request_headers_redacted TEXT,
    response_code            INTEGER,
    response_body_redacted   TEXT,
    duration_ms              INTEGER,
    created_at               INTEGER NOT NULL
);

-- 滚动清理（FR-017）按 created_at 删最旧的，故必须有索引
CREATE INDEX idx_http_log_created ON http_log(created_at);
CREATE INDEX idx_http_log_source  ON http_log(source_id, created_at);

CREATE TABLE cookie (
    source_id  TEXT    NOT NULL,
    name       TEXT    NOT NULL,
    -- 敏感：日志中一律脱敏
    value      TEXT    NOT NULL,
    domain     TEXT    NOT NULL DEFAULT '',
    expires_at INTEGER,
    PRIMARY KEY (source_id, name, domain)
);

CREATE TABLE script_cache (
    source_id  TEXT    NOT NULL,
    key        TEXT    NOT NULL,
    value      BLOB    NOT NULL,
    updated_at INTEGER NOT NULL,
    -- 按 source_id 隔离：跨源共享缓存会把 A 源的登录 token 漏给 B 源
    PRIMARY KEY (source_id, key)
);
"#;

/// 内嵌迁移脚本数组：下标 + 1 = 该脚本产出的 schema 版本。
///
/// 追加新版本时**只允许在末尾新增**，已发布的脚本一个字都不能改——
/// 用户的库里已经跑过它，改动只会让新旧库产生无声的结构差异。
pub const MIGRATIONS: &[&str] = &[V1_SCHEMA];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 迁移脚本数量与支持版本一致() {
        assert_eq!(
            MIGRATIONS.len() as i64,
            SCHEMA_VERSION,
            "SCHEMA_VERSION 必须等于迁移脚本数（每个脚本产出一个版本）"
        );
    }

    #[test]
    fn v1_覆盖_data_model_的_18_张实体表() {
        // 这条断言防的是"漏建一张表"——漏掉的表要到第一次用到它时才报错，
        // 而那时错误信息是"no such table: xxx"，与"忘了建"的因果关系不明显。
        let expected = [
            "source",
            "source_group",
            "book",
            "chapter",
            "chapter_cache",
            "read_progress",
            "read_record",
            "bookmark",
            "highlight",
            "memo",
            "replace_rule",
            "highlight_rule",
            "dict_rule",
            "txt_toc_rule",
            "search_keyword",
            "http_log",
            "cookie",
            "library_root",
        ];
        assert_eq!(expected.len(), 18, "data-model.md 的实体数");
        for table in expected {
            assert!(
                V1_SCHEMA.contains(&format!("CREATE TABLE {table} ")),
                "v1 schema 缺少表 `{table}`"
            );
        }
    }

    #[test]
    fn 痕迹表全部使用字符区间而非渲染坐标() {
        // FR-028 的 schema 层落点：三张痕迹表必须有 chapter_index + char_offset，
        // 且**不得**出现任何像素/行号类字段。字号变化后不错位的前提就在这里。
        for table in ["bookmark", "highlight", "memo"] {
            let start = V1_SCHEMA
                .find(&format!("CREATE TABLE {table} ("))
                .unwrap_or_else(|| panic!("应有表 {table}"));
            let body = &V1_SCHEMA[start..];
            let end = body.find(");").expect("建表语句应以 ); 结束");
            let body = &body[..end];
            assert!(body.contains("chapter_index"), "{table} 应有 chapter_index");
            assert!(body.contains("char_offset"), "{table} 应有 char_offset");
            for forbidden in ["x_", "y_", "pixel", "line_number", "screen"] {
                assert!(
                    !body.contains(forbidden),
                    "{table} 不得含渲染坐标字段 `{forbidden}`（FR-028）"
                );
            }
        }
    }
}
