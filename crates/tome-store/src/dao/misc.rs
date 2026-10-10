//! [DONE] DAO·规则四类 / 关键词 / 请求日志 / Cookie（tasks.md T059）
//!
//! 对应 FR-014（净化）、FR-015（自动高亮）、FR-016（词典）、FR-020（目录识别）、
//! FR-018（关键词管理）、FR-017（请求日志滚动清理）、FR-026（Cookie 持久化）。
//!
//! # 四类规则的共同形状
//!
//! `replace` / `highlight` / `dict` / `txt_toc` 都是"用户可配置的规则行"，
//! 共同点是 `id`（0 表示新增）+ `enabled` + `sort_order`。差异在各自动作字段。
//! 本模块为每类给出"读全部 / 存一条 / 删一条"三个入口——不是四份复制粘贴，
//! 而是共用 [`MiscDao::save_rule_row`] 的 upsert 骨架，各传自己的列清单。
//!
//! ## 为什么"`id` 为 0 表示新增"
//!
//! 界面上编辑规则时，新建的行没有 id。若签名分成 `insert` / `update` 两个，
//! 调用方必须自己记住"这一行是新的还是已有的"——而那个状态在界面重绘、
//! 撤销重做等场景下很容易和实际不一致。`id == 0` 让这件事由**值本身**表达。
//!
//! # 请求日志的滚动清理（FR-017）
//!
//! 「可清理且不无限增长」。本模块在每次写入后按条数裁剪：超过 `keep_rows`
//! 就删掉最旧的若干行。在**写入路径**上做而不是靠定时任务——定时任务在没有
//! 界面的场景（CLI、后台）不会跑，而日志会照常增长。
//!
//! # Cookie 与 FR-026
//!
//! 「按源落盘，重启后保留登录态」。本模块给出**按源**的读写与清理；
//! JS 脚本用的那条路径（`java.getCookie`/`setCookie`）在 [`crate::host`]，
//! 两者共用同一张表，故脚本写的 Cookie 与用户导入的 Cookie 不会分裂成两份。

use rusqlite::Row;

use tome_core::{
    Cookie, DictRule, HighlightRule, HttpLogEntry, ReplaceRule, RuleScope, SearchKeyword,
    TxtTocRule,
};

use crate::db::sql;
use crate::error::StoreError;

/// 请求日志的默认保留条数（FR-017 的滚动清理上限）。
///
/// 取 5000：一条日志约 1–2 KB（含截断后的响应体），5000 条约 5–10 MB——
/// 那是用户可接受的空间占用，也足够覆盖"最近几次抓取出了什么问题"的排查需要。
///
/// [WORKAROUND] 硬编码常量：`config.toml` 的存储子集（T062）里已有 `http_log_keep`
/// 字段，组合根会把它传进来；此处只提供无配置时的默认值。
pub const DEFAULT_HTTP_LOG_KEEP: usize = 5_000;

/// 请求日志中响应体的默认截断长度（**字符数**）。
///
/// 截断的理由是空间，不是为了脱敏——脱敏由调用方在写入前完成
/// （见 [`HttpLogEntry::request_headers_redacted`] 的字段文档）。
///
/// [WORKAROUND] 与 [`DEFAULT_HTTP_LOG_KEEP`] 同属可配置项，当前取默认值。
pub const DEFAULT_RESPONSE_BODY_LIMIT: usize = 8_192;

/// 规则 / 关键词 / 日志 / Cookie DAO。
#[derive(Debug, Clone)]
pub struct MiscDao {
    db: crate::db::Db,
    now: i64,
}

impl MiscDao {
    /// 以当前时间构造。
    pub fn new(db: crate::db::Db) -> Self {
        Self {
            db,
            now: crate::now_unix(),
        }
    }

    /// 以指定时间戳构造（测试用）。
    pub fn with_now(db: crate::db::Db, now: i64) -> Self {
        Self { db, now }
    }

    // -----------------------------------------------------------------------
    // 替换/净化规则（FR-014）
    // -----------------------------------------------------------------------

    /// 全部替换规则（按 `sort_order` 再按 id 排序——**顺序即应用顺序**）。
    pub fn replace_rules(&self) -> Result<Vec<ReplaceRule>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM replace_rule ORDER BY sort_order, id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(ReplaceRule {
                        id: row.get("id")?,
                        name: row.get("name")?,
                        pattern: row.get("pattern")?,
                        replacement: row.get("replacement")?,
                        // 未知 scope 编码报错而非猜默认值：猜成 Content 会让
                        // 一条本该只作用于书名的规则去改正文
                        scope: RuleScope::from_storage_code(row.get("scope")?).ok_or_else(
                            || {
                                rusqlite::Error::FromSqlConversionFailure(
                                    0,
                                    rusqlite::types::Type::Integer,
                                    Box::new(std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        "replace_rule.scope 编码未知（应为 0..=3）",
                                    )),
                                )
                            },
                        )?,
                        is_regex: row.get::<_, i64>("is_regex")? != 0,
                        enabled: row.get::<_, i64>("enabled")? != 0,
                        sort_order: row.get("sort_order")?,
                    })
                })
                .map_err(sql)?;
            let mut rules = Vec::new();
            for row in rows {
                rules.push(row.map_err(sql)?);
            }
            Ok(rules)
        })
    }

    /// 存一条替换规则（`id` 为 0 表示新增），返回主键。
    pub fn save_replace_rule(&self, rule: &ReplaceRule) -> Result<i64, StoreError> {
        let rule = rule.clone();
        self.db.write(move |connection| {
            if rule.id == 0 {
                connection
                    .execute(
                        "INSERT INTO replace_rule(
                            name, pattern, replacement, scope, is_regex, enabled, sort_order
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            rule.name,
                            rule.pattern,
                            rule.replacement,
                            rule.scope.as_storage_code(),
                            i64::from(rule.is_regex),
                            i64::from(rule.enabled),
                            rule.sort_order,
                        ],
                    )
                    .map_err(sql)?;
                return Ok(connection.last_insert_rowid());
            }
            let affected = connection
                .execute(
                    "UPDATE replace_rule SET
                        name=?2, pattern=?3, replacement=?4, scope=?5,
                        is_regex=?6, enabled=?7, sort_order=?8
                     WHERE id=?1",
                    rusqlite::params![
                        rule.id,
                        rule.name,
                        rule.pattern,
                        rule.replacement,
                        rule.scope.as_storage_code(),
                        i64::from(rule.is_regex),
                        i64::from(rule.enabled),
                        rule.sort_order,
                    ],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("replace_rule#{}", rule.id)));
            }
            Ok(rule.id)
        })
    }

    /// 删一条替换规则。
    pub fn remove_replace_rule(&self, rule_id: i64) -> Result<(), StoreError> {
        self.remove_by_id("replace_rule", rule_id)
    }

    // -----------------------------------------------------------------------
    // 高亮规则（FR-015）
    // -----------------------------------------------------------------------

    /// 全部高亮规则。
    pub fn highlight_rules(&self) -> Result<Vec<HighlightRule>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM highlight_rule ORDER BY sort_order, id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(HighlightRule {
                        id: row.get("id")?,
                        name: row.get("name")?,
                        pattern: row.get("pattern")?,
                        style: row.get("style")?,
                        enabled: row.get::<_, i64>("enabled")? != 0,
                        sort_order: row.get("sort_order")?,
                    })
                })
                .map_err(sql)?;
            let mut rules = Vec::new();
            for row in rows {
                rules.push(row.map_err(sql)?);
            }
            Ok(rules)
        })
    }

    /// 存一条高亮规则（`id` 为 0 表示新增），返回主键。
    pub fn save_highlight_rule(&self, rule: &HighlightRule) -> Result<i64, StoreError> {
        let rule = rule.clone();
        self.db.write(move |connection| {
            if rule.id == 0 {
                connection
                    .execute(
                        "INSERT INTO highlight_rule(name, pattern, style, enabled, sort_order)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![
                            rule.name,
                            rule.pattern,
                            rule.style,
                            i64::from(rule.enabled),
                            rule.sort_order,
                        ],
                    )
                    .map_err(sql)?;
                return Ok(connection.last_insert_rowid());
            }
            let affected = connection
                .execute(
                    "UPDATE highlight_rule SET
                        name=?2, pattern=?3, style=?4, enabled=?5, sort_order=?6
                     WHERE id=?1",
                    rusqlite::params![
                        rule.id,
                        rule.name,
                        rule.pattern,
                        rule.style,
                        i64::from(rule.enabled),
                        rule.sort_order,
                    ],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("highlight_rule#{}", rule.id)));
            }
            Ok(rule.id)
        })
    }

    /// 删一条高亮规则。
    pub fn remove_highlight_rule(&self, rule_id: i64) -> Result<(), StoreError> {
        self.remove_by_id("highlight_rule", rule_id)
    }

    // -----------------------------------------------------------------------
    // 词典规则（FR-016）
    // -----------------------------------------------------------------------

    /// 全部词典规则（无 `sort_order` 列，按 id 排序）。
    pub fn dict_rules(&self) -> Result<Vec<DictRule>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM dict_rule ORDER BY id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(DictRule {
                        id: row.get("id")?,
                        name: row.get("name")?,
                        url_rule: row.get("url_rule")?,
                        content_rule: row.get("content_rule")?,
                        enabled: row.get::<_, i64>("enabled")? != 0,
                    })
                })
                .map_err(sql)?;
            let mut rules = Vec::new();
            for row in rows {
                rules.push(row.map_err(sql)?);
            }
            Ok(rules)
        })
    }

    /// 存一条词典规则（`id` 为 0 表示新增），返回主键。
    pub fn save_dict_rule(&self, rule: &DictRule) -> Result<i64, StoreError> {
        let rule = rule.clone();
        self.db.write(move |connection| {
            if rule.id == 0 {
                connection
                    .execute(
                        "INSERT INTO dict_rule(name, url_rule, content_rule, enabled)
                         VALUES (?1, ?2, ?3, ?4)",
                        rusqlite::params![
                            rule.name,
                            rule.url_rule,
                            rule.content_rule,
                            i64::from(rule.enabled),
                        ],
                    )
                    .map_err(sql)?;
                return Ok(connection.last_insert_rowid());
            }
            let affected = connection
                .execute(
                    "UPDATE dict_rule SET name=?2, url_rule=?3, content_rule=?4, enabled=?5
                     WHERE id=?1",
                    rusqlite::params![
                        rule.id,
                        rule.name,
                        rule.url_rule,
                        rule.content_rule,
                        i64::from(rule.enabled),
                    ],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("dict_rule#{}", rule.id)));
            }
            Ok(rule.id)
        })
    }

    /// 删一条词典规则。
    pub fn remove_dict_rule(&self, rule_id: i64) -> Result<(), StoreError> {
        self.remove_by_id("dict_rule", rule_id)
    }

    // -----------------------------------------------------------------------
    // TXT 目录识别规则（FR-020，多条规则回退）
    // -----------------------------------------------------------------------

    /// 全部目录识别规则。**顺序即回退尝试顺序**（FR-020）。
    pub fn txt_toc_rules(&self) -> Result<Vec<TxtTocRule>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM txt_toc_rule ORDER BY sort_order, id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(TxtTocRule {
                        id: row.get("id")?,
                        name: row.get("name")?,
                        rule: row.get("rule")?,
                        example: row.get("example")?,
                        enabled: row.get::<_, i64>("enabled")? != 0,
                        sort_order: row.get("sort_order")?,
                    })
                })
                .map_err(sql)?;
            let mut rules = Vec::new();
            for row in rows {
                rules.push(row.map_err(sql)?);
            }
            Ok(rules)
        })
    }

    /// 存一条目录识别规则（`id` 为 0 表示新增），返回主键。
    pub fn save_txt_toc_rule(&self, rule: &TxtTocRule) -> Result<i64, StoreError> {
        let rule = rule.clone();
        self.db.write(move |connection| {
            if rule.id == 0 {
                connection
                    .execute(
                        "INSERT INTO txt_toc_rule(name, rule, example, enabled, sort_order)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        rusqlite::params![
                            rule.name,
                            rule.rule,
                            rule.example,
                            i64::from(rule.enabled),
                            rule.sort_order,
                        ],
                    )
                    .map_err(sql)?;
                return Ok(connection.last_insert_rowid());
            }
            let affected = connection
                .execute(
                    "UPDATE txt_toc_rule SET
                        name=?2, rule=?3, example=?4, enabled=?5, sort_order=?6
                     WHERE id=?1",
                    rusqlite::params![
                        rule.id,
                        rule.name,
                        rule.rule,
                        rule.example,
                        i64::from(rule.enabled),
                        rule.sort_order,
                    ],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("txt_toc_rule#{}", rule.id)));
            }
            Ok(rule.id)
        })
    }

    /// 删一条目录识别规则。
    pub fn remove_txt_toc_rule(&self, rule_id: i64) -> Result<(), StoreError> {
        self.remove_by_id("txt_toc_rule", rule_id)
    }

    /// 四类规则共用的按 id 删除。
    fn remove_by_id(&self, table: &'static str, rule_id: i64) -> Result<(), StoreError> {
        self.db.write(move |connection| {
            let affected = connection
                .execute(&format!("DELETE FROM {table} WHERE id=?1"), [rule_id])
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("{table}#{rule_id}")));
            }
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // 搜索关键词（FR-018）
    // -----------------------------------------------------------------------

    /// 全部关键词。默认按**使用次数降序**（常用词置顶），同次数按收藏时间倒序。
    pub fn search_keywords(&self) -> Result<Vec<SearchKeyword>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT * FROM search_keyword ORDER BY use_count DESC, created_at DESC, id",
                )
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(SearchKeyword {
                        id: row.get("id")?,
                        keyword: row.get("keyword")?,
                        created_at: row.get("created_at")?,
                        use_count: row.get("use_count")?,
                    })
                })
                .map_err(sql)?;
            let mut keywords = Vec::new();
            for row in rows {
                keywords.push(row.map_err(sql)?);
            }
            Ok(keywords)
        })
    }

    /// 收藏一个关键词。
    ///
    /// # 已收藏时**不报错**，返回已有主键
    ///
    /// "再收藏一次"是用户的常见动作（可能是忘了已经收藏过）。报唯一约束
    /// 冲突会把这件无害的事变成一个错误提示。返回已有 id 让调用方可以做
    /// "收藏后就跳到结果"这类动作。
    pub fn add_search_keyword(&self, keyword: &str) -> Result<i64, StoreError> {
        let keyword = keyword.to_string();
        let now = self.now;
        self.db.write(move |connection| {
            connection
                .execute(
                    "INSERT INTO search_keyword(keyword, created_at, use_count) VALUES (?1, ?2, 0)
                     ON CONFLICT(keyword) DO NOTHING",
                    rusqlite::params![keyword, now],
                )
                .map_err(sql)?;
            connection
                .query_row(
                    "SELECT id FROM search_keyword WHERE keyword=?1",
                    [&keyword],
                    |row| row.get(0),
                )
                .map_err(sql)
        })
    }

    /// 记录一次关键词复用（`use_count` 加一）。
    ///
    /// 用 SQL 里的 `+1` 而不是读改写：连续快速搜索时读改写会丢更新。
    pub fn touch_search_keyword(&self, keyword: &str) -> Result<(), StoreError> {
        let keyword = keyword.to_string();
        self.db.write(move |connection| {
            let affected = connection
                .execute(
                    "UPDATE search_keyword SET use_count = use_count + 1 WHERE keyword=?1",
                    [&keyword],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("search_keyword:{keyword}")));
            }
            Ok(())
        })
    }

    /// 删除一个关键词。
    pub fn remove_search_keyword(&self, keyword: &str) -> Result<(), StoreError> {
        let keyword = keyword.to_string();
        self.db.write(move |connection| {
            let affected = connection
                .execute("DELETE FROM search_keyword WHERE keyword=?1", [&keyword])
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("search_keyword:{keyword}")));
            }
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // 请求日志（FR-017）
    // -----------------------------------------------------------------------

    /// 记一条请求日志并做滚动清理。
    ///
    /// # 截断与脱敏的分工
    ///
    /// - **脱敏**由调用方在写入前完成（`*_redacted` 字段名就是提醒）。
    ///   DAO 无权判断"哪个头算敏感"——那是网络层的知识（Cookie/Authorization/
    ///   Proxy-Authorization 等），放这里会让两处各维护一份敏感头清单。
    /// - **截断**在这里做：它是纯空间考量，与语义无关。
    ///
    /// 两者都不做的话，日志会同时成为泄漏渠道与磁盘杀手。
    pub fn log_request(&self, entry: &HttpLogEntry) -> Result<i64, StoreError> {
        self.log_request_keeping(entry, DEFAULT_HTTP_LOG_KEEP)
    }

    /// 同 [`Self::log_request`]，但显式给出保留条数（配置注入的入口）。
    ///
    /// `keep_rows` 为 0 表示不清理（调试时想看全量日志）。
    pub fn log_request_keeping(
        &self,
        entry: &HttpLogEntry,
        keep_rows: usize,
    ) -> Result<i64, StoreError> {
        let entry = entry.clone();
        let now = self.now;
        self.db.write(move |connection| {
            let created_at = if entry.created_at > 0 {
                entry.created_at
            } else {
                now
            };
            let body = entry
                .response_body_redacted
                .as_deref()
                .map(|text| truncate_chars(text, DEFAULT_RESPONSE_BODY_LIMIT));
            connection
                .execute(
                    "INSERT INTO http_log(
                        source_id, url, method, request_headers_redacted,
                        response_code, response_body_redacted, duration_ms, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    rusqlite::params![
                        entry.source_id,
                        entry.url,
                        entry.method,
                        entry.request_headers_redacted,
                        entry.response_code,
                        body,
                        entry.duration_ms,
                        created_at,
                    ],
                )
                .map_err(sql)?;
            let id = connection.last_insert_rowid();

            if keep_rows > 0 {
                // 一次 DELETE 而不是"数一遍再删"：后者在并发写入下算出的
                // 数量可能已经过期。`OFFSET` 配合子查询让裁剪与计数在同一条
                // 语句里完成。
                connection
                    .execute(
                        "DELETE FROM http_log WHERE id IN (
                            SELECT id FROM http_log ORDER BY created_at DESC, id DESC
                            LIMIT -1 OFFSET ?1
                         )",
                        [keep_rows as i64],
                    )
                    .map_err(sql)?;
            }
            Ok(id)
        })
    }

    /// 查最近若干条日志（按时间倒序，新的在前）。
    pub fn recent_logs(&self, limit: usize) -> Result<Vec<HttpLogEntry>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM http_log ORDER BY created_at DESC, id DESC LIMIT ?1")
                .map_err(sql)?;
            let rows = statement.query_map([limit as i64], read_log).map_err(sql)?;
            let mut logs = Vec::new();
            for row in rows {
                logs.push(row.map_err(sql)?);
            }
            Ok(logs)
        })
    }

    /// 按源查日志。
    pub fn logs_of_source(
        &self,
        source_id: &str,
        limit: usize,
    ) -> Result<Vec<HttpLogEntry>, StoreError> {
        let source_id = source_id.to_string();
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT * FROM http_log WHERE source_id=?1
                     ORDER BY created_at DESC, id DESC LIMIT ?2",
                )
                .map_err(sql)?;
            let rows = statement
                .query_map(rusqlite::params![source_id, limit as i64], read_log)
                .map_err(sql)?;
            let mut logs = Vec::new();
            for row in rows {
                logs.push(row.map_err(sql)?);
            }
            Ok(logs)
        })
    }

    /// 日志条数（滚动清理的断言载体）。
    pub fn log_count(&self) -> Result<i64, StoreError> {
        self.db.read(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM http_log", [], |row| row.get(0))
                .map_err(sql)
        })
    }

    /// 清空全部日志（FR-017 的"可清理"）。
    pub fn clear_logs(&self) -> Result<usize, StoreError> {
        self.db
            .write(|connection| connection.execute("DELETE FROM http_log", []).map_err(sql))
    }

    // -----------------------------------------------------------------------
    // Cookie（FR-026）
    // -----------------------------------------------------------------------

    /// 读一个源的某个 Cookie。
    ///
    /// `domain` 为 `None` 时读"脚本写入"的那一类（见 [`crate::host`] 的说明）；
    /// 给定时按 HTTP domain 精确匹配。
    pub fn cookie(
        &self,
        source_id: &str,
        name: &str,
        domain: Option<&str>,
    ) -> Result<Option<Cookie>, StoreError> {
        let source_id = source_id.to_string();
        let name = name.to_string();
        let domain = domain
            .unwrap_or(crate::host::SCRIPT_COOKIE_DOMAIN)
            .to_string();
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM cookie WHERE source_id=?1 AND name=?2 AND domain=?3")
                .map_err(sql)?;
            let mut rows = statement
                .query(rusqlite::params![source_id, name, domain])
                .map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(read_cookie(row).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// 某个源的全部 Cookie。
    pub fn cookies_of_source(&self, source_id: &str) -> Result<Vec<Cookie>, StoreError> {
        let source_id = source_id.to_string();
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM cookie WHERE source_id=?1 ORDER BY name, domain")
                .map_err(sql)?;
            let rows = statement
                .query_map([&source_id], read_cookie)
                .map_err(sql)?;
            let mut cookies = Vec::new();
            for row in rows {
                cookies.push(row.map_err(sql)?);
            }
            Ok(cookies)
        })
    }

    /// 写 Cookie（同 `(source_id, name, domain)` 覆盖）。
    pub fn put_cookie(&self, cookie: &Cookie) -> Result<(), StoreError> {
        let cookie = cookie.clone();
        self.db.write(move |connection| {
            let domain = cookie
                .domain
                .clone()
                .unwrap_or_else(|| crate::host::SCRIPT_COOKIE_DOMAIN.to_string());
            connection
                .execute(
                    "INSERT INTO cookie(source_id, name, value, domain, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(source_id, name, domain) DO UPDATE SET
                        value=excluded.value,
                        expires_at=excluded.expires_at",
                    rusqlite::params![
                        cookie.source_id,
                        cookie.name,
                        cookie.value,
                        domain,
                        cookie.expires_at,
                    ],
                )
                .map_err(sql)?;
            Ok(())
        })
    }

    /// 删除一个源的某个 Cookie。
    pub fn remove_cookie(&self, source_id: &str, name: &str) -> Result<(), StoreError> {
        let source_id = source_id.to_string();
        let name = name.to_string();
        self.db.write(move |connection| {
            let affected = connection
                .execute(
                    "DELETE FROM cookie WHERE source_id=?1 AND name=?2",
                    rusqlite::params![source_id, name],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("cookie:{source_id}/{name}")));
            }
            Ok(())
        })
    }

    /// 清空某个源的全部 Cookie（登出）。
    pub fn clear_cookies_of_source(&self, source_id: &str) -> Result<usize, StoreError> {
        let source_id = source_id.to_string();
        self.db.write(move |connection| {
            connection
                .execute("DELETE FROM cookie WHERE source_id=?1", [&source_id])
                .map_err(sql)
        })
    }

    /// 过期 Cookie 的清理（`expires_at` 非空且早于 now）。
    ///
    /// 返回清理条数。调用时机由上层决定（启动时或定期）——DAO 不做定时。
    pub fn purge_expired_cookies(&self) -> Result<usize, StoreError> {
        let now = self.now;
        self.db.write(move |connection| {
            connection
                .execute(
                    "DELETE FROM cookie WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                    [now],
                )
                .map_err(sql)
        })
    }
}

/// 从一行读出一条 Cookie。
fn read_cookie(row: &Row<'_>) -> rusqlite::Result<Cookie> {
    Ok(Cookie {
        source_id: row.get("source_id")?,
        name: row.get("name")?,
        value: row.get("value")?,
        domain: Some(row.get("domain")?),
        expires_at: row.get("expires_at")?,
    })
}

/// 日志行的类型别名（`tome-store` 侧引用 `tome-core` 的 DTO）。
pub type HttpLogRow = HttpLogEntry;

/// `http_log` 表的落点实现（T064 的 tome-store 侧）。
///
/// # 为什么实现放在 DAO 这边而不在 tome-store 根部
///
/// 它需要 `MiscDao` 的 `log_request_keeping`（含滚动清理），而那是本模块
/// 的职责。放在根部会让根部还得再转发一次。
///
/// # 为什么错误被吞掉
///
/// 日志是**诊断设施**：它写不进去不该让用户的抓取失败。故 `record` 返回
/// `()`，失败只静默丢弃（上层不会因为日志表满了而读不了书）。
///
/// `[UNCERTAIN]` 完全静默会掩盖"日志一直没写进去"这种配置问题。
/// 后续若需要可观测性，应加一个内部计数器而不是改签名（改签名会让
/// 每个调用点都要处理一个它无法处置的错误）。
pub struct HttpLogSinkAdapter {
    dao: MiscDao,
    /// 保留条数（0 = 不清理）
    keep: usize,
}

impl HttpLogSinkAdapter {
    /// 构造。
    pub fn new(dao: MiscDao, keep: usize) -> Self {
        Self { dao, keep }
    }
}

impl tome_net::HttpLogSink for HttpLogSinkAdapter {
    fn record(&self, entry: &crate::dao::misc::HttpLogRow) {
        // 失败静默：日志写不进去不该让用户的抓取失败（见类型文档）
        let _ = self.dao.log_request_keeping(entry, self.keep);
    }
}

/// 从一行读出一条日志。
fn read_log(row: &Row<'_>) -> rusqlite::Result<HttpLogEntry> {
    Ok(HttpLogEntry {
        id: row.get("id")?,
        source_id: row.get("source_id")?,
        url: row.get("url")?,
        method: row.get("method")?,
        request_headers_redacted: row.get("request_headers_redacted")?,
        response_code: row
            .get::<_, Option<i64>>("response_code")?
            .map(|code| code as u16),
        response_body_redacted: row.get("response_body_redacted")?,
        duration_ms: row.get("duration_ms")?,
        created_at: row.get("created_at")?,
    })
}

/// 按**字符数**截断（不是字节数）。
///
/// # 为什么按字符
///
/// 按字节截断会把一个多字节字符切成两半，产出非法的 UTF-8——存进 SQLite 的
/// TEXT 列时被替换成 U+FFFD，用户看到日志末尾是"�"。中文正文下每个字 3 字节，
/// 8 KiB 的字节上限实际只有约 2700 字，与"截到 8000 字"的意图相差 3 倍。
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(limit).collect();
    // 明确的截断标记：用户看到它才知道后面还有内容，而不是以为日志就这么短
    truncated.push_str("…[已截断]");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: std::path::PathBuf,
        dao: MiscDao,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-misc-dao-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");
        Fixture {
            dao: MiscDao::with_now(db, 1_700_000_000),
            dir,
        }
    }

    // --- 四类规则 ---

    #[test]
    fn 替换规则新增与更新() {
        let fixture = fixture("replace");
        let mut rule = ReplaceRule {
            id: 0,
            name: "去掉广告".to_string(),
            pattern: "广告位".to_string(),
            replacement: String::new(),
            scope: RuleScope::Content,
            is_regex: false,
            enabled: true,
            sort_order: 0,
        };
        let id = fixture.dao.save_replace_rule(&rule).expect("应能新增");
        assert!(id > 0);

        rule.id = id;
        rule.name = "改名".to_string();
        rule.is_regex = true;
        rule.scope = RuleScope::ChapterTitle;
        assert_eq!(fixture.dao.save_replace_rule(&rule).expect("应能更新"), id);

        let rules = fixture.dao.replace_rules().expect("应能查");
        assert_eq!(rules.len(), 1, "更新不该产生新行");
        assert_eq!(rules[0].name, "改名");
        assert!(rules[0].is_regex);
        assert_eq!(rules[0].scope, RuleScope::ChapterTitle);
    }

    #[test]
    fn 规则按_sort_order_排序_顺序即应用顺序() {
        // FR-014 的净化链顺序固定，同层内的顺序由 sort_order 决定。
        // 顺序不确定会让净化结果随存储抖动而变。
        let fixture = fixture("order");
        for (name, order) in [("第三", 30), ("第一", 10), ("第二", 20)] {
            fixture
                .dao
                .save_replace_rule(&ReplaceRule {
                    id: 0,
                    name: name.to_string(),
                    pattern: name.to_string(),
                    replacement: String::new(),
                    scope: RuleScope::Content,
                    is_regex: false,
                    enabled: true,
                    sort_order: order,
                })
                .expect("应能存");
        }
        let names: Vec<String> = fixture
            .dao
            .replace_rules()
            .expect("应能查")
            .into_iter()
            .map(|rule| rule.name)
            .collect();
        assert_eq!(names, vec!["第一", "第二", "第三"]);
    }

    #[test]
    fn 未知规则范围编码报错() {
        let fixture = fixture("bad-scope");
        fixture
            .dao
            .db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO replace_rule(name, pattern, replacement, scope, sort_order)
                         VALUES ('坏', 'p', 'r', 99, 0)",
                        [],
                    )
                    .map_err(sql)
            })
            .expect("应能写");
        let error = fixture.dao.replace_rules().expect_err("应报错");
        assert!(error.to_string().contains("scope"), "{error}");
    }

    #[test]
    fn 高亮规则增删查() {
        let fixture = fixture("highlight-rule");
        let id = fixture
            .dao
            .save_highlight_rule(&HighlightRule {
                id: 0,
                name: "关键词".to_string(),
                pattern: "关键词".to_string(),
                style: "red".to_string(),
                enabled: true,
                sort_order: 0,
            })
            .expect("应能存");
        assert_eq!(fixture.dao.highlight_rules().expect("应能查").len(), 1);
        fixture.dao.remove_highlight_rule(id).expect("应能删");
        assert!(fixture.dao.highlight_rules().expect("应能查").is_empty());
    }

    #[test]
    fn 词典规则增删查() {
        let fixture = fixture("dict-rule");
        let id = fixture
            .dao
            .save_dict_rule(&DictRule {
                id: 0,
                name: "百度汉语".to_string(),
                url_rule: "https://x/{{word}}".to_string(),
                content_rule: "@css:.def".to_string(),
                enabled: true,
            })
            .expect("应能存");
        let rules = fixture.dao.dict_rules().expect("应能查");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].url_rule, "https://x/{{word}}");
        fixture.dao.remove_dict_rule(id).expect("应能删");
    }

    #[test]
    fn 目录识别规则按回退顺序排列() {
        // FR-020：多条规则回退，sort_order 即尝试顺序
        let fixture = fixture("toc-rule");
        for (name, order) in [("备用", 5), ("首选", 1)] {
            fixture
                .dao
                .save_txt_toc_rule(&TxtTocRule {
                    id: 0,
                    name: name.to_string(),
                    rule: "^第.+章".to_string(),
                    example: Some("第一章 开始".to_string()),
                    enabled: true,
                    sort_order: order,
                })
                .expect("应能存");
        }
        let names: Vec<String> = fixture
            .dao
            .txt_toc_rules()
            .expect("应能查")
            .into_iter()
            .map(|rule| rule.name)
            .collect();
        assert_eq!(names, vec!["首选", "备用"], "顺序即回退尝试顺序");
    }

    #[test]
    fn 删不存在的规则报结构化错误() {
        let fixture = fixture("rule-missing");
        for removed in [
            fixture.dao.remove_replace_rule(999),
            fixture.dao.remove_highlight_rule(999),
            fixture.dao.remove_dict_rule(999),
            fixture.dao.remove_txt_toc_rule(999),
        ] {
            let error = removed.expect_err("应报错");
            assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
        }
    }

    // --- 关键词 ---

    #[test]
    fn 关键词收藏与复用计数() {
        let fixture = fixture("keyword");
        let id = fixture
            .dao
            .add_search_keyword("斗破苍穹")
            .expect("应能收藏");
        assert_eq!(
            fixture
                .dao
                .add_search_keyword("斗破苍穹")
                .expect("重复收藏应成功"),
            id,
            "重复收藏不该报错，返回已有 id"
        );
        assert_eq!(fixture.dao.search_keywords().expect("应能查").len(), 1);

        fixture
            .dao
            .touch_search_keyword("斗破苍穹")
            .expect("应能计数");
        fixture
            .dao
            .touch_search_keyword("斗破苍穹")
            .expect("应能计数");
        assert_eq!(
            fixture.dao.search_keywords().expect("应能查")[0].use_count,
            2
        );
    }

    #[test]
    fn 关键词按使用次数降序() {
        let fixture = fixture("keyword-order");
        fixture.dao.add_search_keyword("少用").expect("应能收藏");
        fixture.dao.add_search_keyword("常用").expect("应能收藏");
        fixture.dao.touch_search_keyword("常用").expect("应能计数");
        let keywords: Vec<String> = fixture
            .dao
            .search_keywords()
            .expect("应能查")
            .into_iter()
            .map(|entry| entry.keyword)
            .collect();
        assert_eq!(keywords, vec!["常用", "少用"], "常用词应置顶");
    }

    #[test]
    fn 未收藏的关键词计数报错() {
        let fixture = fixture("keyword-missing");
        let error = fixture
            .dao
            .touch_search_keyword("没有")
            .expect_err("应报错");
        assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
    }

    // --- 请求日志 ---

    fn log_entry(index: usize) -> HttpLogEntry {
        HttpLogEntry {
            id: 0,
            source_id: Some("https://a.com".to_string()),
            url: format!("https://a.com/page/{index}"),
            method: "GET".to_string(),
            request_headers_redacted: Some("{\"User-Agent\":\"x\"}".to_string()),
            response_code: Some(200),
            response_body_redacted: Some(format!("body {index}")),
            duration_ms: Some(120),
            // 递增时间戳，让"最旧的"有确定含义
            created_at: 1_700_000_000 + index as i64,
        }
    }

    #[test]
    fn 日志往返() {
        let fixture = fixture("log");
        fixture.dao.log_request(&log_entry(1)).expect("应能记");
        let logs = fixture.dao.recent_logs(10).expect("应能查");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].url, "https://a.com/page/1");
        assert_eq!(logs[0].response_code, Some(200));
        assert_eq!(logs[0].method, "GET");
    }

    #[test]
    fn 日志滚动清理只留最新的() {
        // FR-017：日志须"可清理且不无限增长"
        let fixture = fixture("log-roll");
        for index in 0..50 {
            fixture
                .dao
                .log_request_keeping(&log_entry(index), 10)
                .expect("应能记");
        }
        assert_eq!(
            fixture.dao.log_count().expect("应能计数"),
            10,
            "应只保留 10 条"
        );
        let logs = fixture.dao.recent_logs(10).expect("应能查");
        assert_eq!(logs[0].url, "https://a.com/page/49", "最新的应留下");
        assert_eq!(logs[9].url, "https://a.com/page/40", "最旧的 40 条应被裁掉");
    }

    #[test]
    fn 保留条数为零时不做清理() {
        let fixture = fixture("log-keep-all");
        for index in 0..5 {
            fixture
                .dao
                .log_request_keeping(&log_entry(index), 0)
                .expect("应能记");
        }
        assert_eq!(fixture.dao.log_count().expect("应能计数"), 5);
    }

    #[test]
    fn 超长响应体按字符截断() {
        let fixture = fixture("log-truncate");
        let long_body = "中".repeat(DEFAULT_RESPONSE_BODY_LIMIT + 500);
        let mut entry = log_entry(0);
        entry.response_body_redacted = Some(long_body);
        fixture.dao.log_request(&entry).expect("应能记");

        let logged = &fixture.dao.recent_logs(1).expect("应能查")[0];
        let body = logged.response_body_redacted.as_ref().expect("应有响应体");
        assert!(body.ends_with("…[已截断]"), "应有截断标记");
        assert_eq!(
            body.chars().filter(|character| *character == '中').count(),
            DEFAULT_RESPONSE_BODY_LIMIT,
            "应按字符数截断（不是字节数——中文下两者差 3 倍）"
        );
    }

    #[test]
    fn 截断不产生非法_utf8() {
        // "中文内容" 逐字重复；取 7 个字符恰好是「中文内容中文内」——2 个「中」。
        // 断言具体的字符构成而不是"长度差不多"，才真的验证了没有把多字节
        // 字符切成半个（切开会产生 U+FFFD 或丢失字符，计数就不对了）。
        let text = "中文内容".repeat(100);
        let truncated = truncate_chars(&text, 7);
        let body = truncated.strip_suffix("…[已截断]").expect("应有截断标记");
        assert_eq!(body, "中文内容中文内");
        assert_eq!(body.chars().count(), 7);
        assert!(!truncated.contains('\u{FFFD}'), "不得产生替换字符");
    }

    #[test]
    fn 按源查日志() {
        let fixture = fixture("log-by-source");
        fixture.dao.log_request(&log_entry(0)).expect("应能记");
        let mut other = log_entry(1);
        other.source_id = Some("https://b.com".to_string());
        fixture.dao.log_request(&other).expect("应能记");

        let logs = fixture
            .dao
            .logs_of_source("https://a.com", 10)
            .expect("应能查");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].url, "https://a.com/page/0");
    }

    #[test]
    fn 清空日志() {
        let fixture = fixture("log-clear");
        for index in 0..3 {
            fixture.dao.log_request(&log_entry(index)).expect("应能记");
        }
        assert_eq!(fixture.dao.clear_logs().expect("应能清"), 3);
        assert_eq!(fixture.dao.log_count().expect("应能计数"), 0);
    }

    // --- Cookie ---

    #[test]
    fn cookie_往返与落盘保留() {
        // FR-026：按源落盘，重启后保留登录态
        let fixture = fixture("cookie");
        fixture
            .dao
            .put_cookie(&Cookie {
                source_id: "https://a.com".to_string(),
                name: "token".to_string(),
                value: "abc".to_string(),
                domain: None,
                expires_at: None,
            })
            .expect("应能写");

        let loaded = fixture
            .dao
            .cookie("https://a.com", "token", None)
            .expect("应能读")
            .expect("应有");
        assert_eq!(loaded.value, "abc");

        // 重开库
        let path = fixture.dir.join("tome.db");
        let reopened = MiscDao::new(crate::db::Db::open(&path).expect("应能重开"));
        assert_eq!(
            reopened
                .cookie("https://a.com", "token", None)
                .expect("应能读")
                .expect("重启后应还在")
                .value,
            "abc",
            "登录态必须在重启后保留（FR-026）"
        );
    }

    #[test]
    fn cookie_按源隔离() {
        let fixture = fixture("cookie-isolation");
        for (source, value) in [("https://a.com", "A"), ("https://b.com", "B")] {
            fixture
                .dao
                .put_cookie(&Cookie {
                    source_id: source.to_string(),
                    name: "token".to_string(),
                    value: value.to_string(),
                    domain: None,
                    expires_at: None,
                })
                .expect("应能写");
        }
        assert_eq!(
            fixture
                .dao
                .cookie("https://a.com", "token", None)
                .expect("应能读")
                .expect("应有")
                .value,
            "A"
        );
        assert_eq!(
            fixture
                .dao
                .cookie("https://b.com", "token", None)
                .expect("应能读")
                .expect("应有")
                .value,
            "B",
            "同名 Cookie 不得跨源串味"
        );
    }

    #[test]
    fn 清空某源的全部_cookie() {
        let fixture = fixture("cookie-clear");
        for name in ["a", "b"] {
            fixture
                .dao
                .put_cookie(&Cookie {
                    source_id: "https://a.com".to_string(),
                    name: name.to_string(),
                    value: "v".to_string(),
                    domain: None,
                    expires_at: None,
                })
                .expect("应能写");
        }
        assert_eq!(
            fixture
                .dao
                .cookies_of_source("https://a.com")
                .expect("应能查")
                .len(),
            2
        );
        assert_eq!(
            fixture
                .dao
                .clear_cookies_of_source("https://a.com")
                .expect("应能清"),
            2
        );
        assert!(fixture
            .dao
            .cookies_of_source("https://a.com")
            .expect("应能查")
            .is_empty());
    }

    #[test]
    fn 过期_cookie_被清理() {
        let fixture = fixture("cookie-expire");
        fixture
            .dao
            .put_cookie(&Cookie {
                source_id: "https://a.com".to_string(),
                name: "过期".to_string(),
                value: "v".to_string(),
                domain: None,
                // 早于 DAO 的 now（1_700_000_000）
                expires_at: Some(1_600_000_000),
            })
            .expect("应能写");
        fixture
            .dao
            .put_cookie(&Cookie {
                source_id: "https://a.com".to_string(),
                name: "还有效".to_string(),
                value: "v".to_string(),
                domain: None,
                expires_at: Some(1_800_000_000),
            })
            .expect("应能写");
        fixture
            .dao
            .put_cookie(&Cookie {
                source_id: "https://a.com".to_string(),
                name: "永久".to_string(),
                value: "v".to_string(),
                domain: None,
                expires_at: None,
            })
            .expect("应能写");

        assert_eq!(fixture.dao.purge_expired_cookies().expect("应能清"), 1);
        let remaining: Vec<String> = fixture
            .dao
            .cookies_of_source("https://a.com")
            .expect("应能查")
            .into_iter()
            .map(|cookie| cookie.name)
            .collect();
        assert_eq!(remaining, vec!["永久", "还有效"], "未过期的与永久的应留下");
    }

    #[test]
    fn 删不存在的_cookie_报错() {
        let fixture = fixture("cookie-missing");
        let error = fixture
            .dao
            .remove_cookie("https://a.com", "nope")
            .expect_err("应报错");
        assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
    }
}
