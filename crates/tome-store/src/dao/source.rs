//! [DONE] DAO·书源与分组（tasks.md T056 / FR-001、FR-026、FR-035）
//!
//! # 一张表装四类源
//!
//! `data-model.md` 定的是"统一表，`kind` 区分书源/订阅源/词典源"。四类源的
//! 元信息（名称/分组/启用/请求定制/脚本）高度重合，差异只在规则字段的有无；
//! 拆表会让"列出全部源"变成三次 join，而那是界面最常见的操作。
//!
//! # `upsert` 而非 `insert`
//!
//! spec 边缘情况要求"导入重复书源（同 id）按更新处理"。用
//! `ON CONFLICT(id) DO UPDATE` 而不是先查后写：后者在并发导入时有 TOCTOU
//! 窗口，且要多一次往返。SQLite 的 upsert 是原子的，语义与要求逐字对应。
//!
//! # 为什么不用 `INSERT OR REPLACE`
//!
//! `OR REPLACE` 是**先删后插**：`source` 行被删除时，外键 `ON DELETE CASCADE`
//! 会连带清掉挂在它下面的数据。本表暂时没有下游，但 `source.id` 已被
//! `cookie` 与 `script_cache` 引用（按源持久化的登录态）——`OR REPLACE`
//! 会把用户的登录态一起抹掉，而那正是 FR-026 要求保留的东西。

use rusqlite::{Connection, Row};

use tome_core::seam::{ImportFailure, ImportReport};
use tome_core::{Source, SourceGroup, SourceKind};

use crate::db::sql;
use crate::error::StoreError;

/// 一次导入的结果（新增/更新/跳过 + 逐条失败说明）。
///
/// 直接用 core 的 [`ImportReport`]：接缝的 `import_sources`/`import_parsed_sources`
/// 返回的就是它，DAO 再包一层只会制造两个需要同步维护的形状。
pub type SourceImportReport = ImportReport;

/// 从一行读出书源。
///
/// # 为什么把转换写成函数而不是 `impl From<Row>`
///
/// `Row` 带生命周期且每次查询都是新的，逐列取值的错误（列名写错、类型不符）
/// 只能在这一处集中处理。写成函数让它可被 `query_map` 直接调用，且
/// 各 DAO 方法共用同一份映射——列顺序变化时只有一个地方要改。
fn read_source(row: &Row<'_>) -> rusqlite::Result<Source> {
    // 未知的 kind 编码：报错而不是猜默认值。猜成 Book 会让"词典源被当书源跑"，
    // 表现为"规则里没有 searchUrl"这类误导性的下游错误。
    let kind_code: i32 = row.get("kind")?;
    let kind = SourceKind::from_storage_code(kind_code).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("source.kind 编码 {kind_code} 未知（应为 0..=3）"),
            )),
        )
    })?;

    Ok(Source {
        id: row.get("id")?,
        name: row.get("name")?,
        kind,
        group_id: row.get("group_id")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        book_source_type: row.get("book_source_type")?,
        book_url_pattern: row.get("book_url_pattern")?,
        search_url: row.get("search_url")?,
        explore_url: row.get("explore_url")?,
        rule_search: row.get("rule_search")?,
        rule_explore: row.get("rule_explore")?,
        rule_book_info: row.get("rule_book_info")?,
        rule_toc: row.get("rule_toc")?,
        rule_content: row.get("rule_content")?,
        header: row.get("header")?,
        login_url: row.get("login_url")?,
        login_ui: row.get("login_ui")?,
        login_check_js: row.get("login_check_js")?,
        js: row.get("js")?,
        charset: row.get("charset")?,
        rate_limit_ms: row.get("rate_limit_ms")?,
        weight: row.get("weight")?,
        explore_screen: row.get("explore_screen")?,
        comment: row.get("comment")?,
        last_update_time: row.get("last_update_time")?,
        created_at: row.get("created_at")?,
    })
}

/// 书源 DAO。
///
/// 持 [`crate::db::Db`]（廉价克隆，共享同一个 actor 通道），故各方法都取 `&self`。
#[derive(Debug, Clone)]
pub struct SourceDao {
    db: crate::db::Db,
    /// 导入时的默认 `created_at`（新增行用它；更新行不动原值）。
    ///
    /// 放在构造参数而不是每次调用传入：它是"程序认为现在几点"这类环境事实，
    /// 而不是逐次变化的操作参数。测试可以注入固定值让结果可断言。
    now: i64,
}

impl SourceDao {
    /// 以当前时间构造。
    pub fn new(db: crate::db::Db) -> Self {
        Self {
            db,
            now: crate::now_unix(),
        }
    }

    /// 以指定时间戳构造（测试用，让"创建时间"可断言）。
    pub fn with_now(db: crate::db::Db, now: i64) -> Self {
        Self { db, now }
    }

    /// 全部书源（按 id 排序）。
    ///
    /// # 为什么排序在这里而不是留给上层
    ///
    /// CLI 的 T029 验收项要求"输出逐字节一致"。若顺序取决于 SQLite 的返回
    /// 顺序（无 `ORDER BY` 时是未定义的），同一份数据两次运行就可能不同——
    /// 那会让"录制→重放逐字节一致"变成时绿时红的测试。故顺序在 DAO 层固定。
    pub fn all(&self) -> Result<Vec<Source>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM source ORDER BY id")
                .map_err(sql)?;
            let rows = statement.query_map([], read_source).map_err(sql)?;
            let mut sources = Vec::new();
            for row in rows {
                sources.push(row.map_err(sql)?);
            }
            Ok(sources)
        })
    }

    /// 按 id 取单个书源；不存在时 `Ok(None)`。
    pub fn by_id(&self, source_id: &str) -> Result<Option<Source>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM source WHERE id=?1")
                .map_err(sql)?;
            let mut rows = statement.query([source_id]).map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(read_source(row).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// 按启用状态过滤（FR-001：书源可批量启停）。
    pub fn enabled_only(&self) -> Result<Vec<Source>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM source WHERE enabled=1 ORDER BY id")
                .map_err(sql)?;
            let rows = statement.query_map([], read_source).map_err(sql)?;
            let mut sources = Vec::new();
            for row in rows {
                sources.push(row.map_err(sql)?);
            }
            Ok(sources)
        })
    }

    /// 按分组过滤（`group_id` 为 `None` 时取未分组的源）。
    pub fn by_group(&self, group_id: Option<i64>) -> Result<Vec<Source>, StoreError> {
        self.db.read(|connection| {
            // 两条 SQL 而不是一条 `group_id IS ?1`：SQLite 的 `IS` 对参数绑定的
            // NULL 语义与直觉不同（`group_id IS NULL` 是判定，`group_id IS ?1`
            // 在 ?1 非空时退化成等值比较但优化器不走索引）。分支写清楚更稳。
            let mut sources = Vec::new();
            match group_id {
                Some(id) => {
                    let mut statement = connection
                        .prepare_cached("SELECT * FROM source WHERE group_id=?1 ORDER BY id")
                        .map_err(sql)?;
                    let rows = statement.query_map([id], read_source).map_err(sql)?;
                    for row in rows {
                        sources.push(row.map_err(sql)?);
                    }
                }
                None => {
                    let mut statement = connection
                        .prepare_cached("SELECT * FROM source WHERE group_id IS NULL ORDER BY id")
                        .map_err(sql)?;
                    let rows = statement.query_map([], read_source).map_err(sql)?;
                    for row in rows {
                        sources.push(row.map_err(sql)?);
                    }
                }
            }
            Ok(sources)
        })
    }

    /// 落库一批书源：**同 id 即更新**（spec 边缘情况）。
    ///
    /// 返回的 [`ImportReport`] 区分"新增"与"更新"——逐条在事务内判定，故
    /// 计数与实际写入一致（若先批量查存在性再写，中间会被并发写插入干扰）。
    pub fn upsert_all(&self, sources: &[Source]) -> Result<SourceImportReport, StoreError> {
        if sources.is_empty() {
            return Ok(SourceImportReport::default());
        }
        let now = self.now;
        let sources = sources.to_vec();
        self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            let mut report = SourceImportReport::default();
            for source in &sources {
                match upsert_one(&transaction, source, now) {
                    Ok(was_update) => {
                        if was_update {
                            report.updated += 1;
                        } else {
                            report.added += 1;
                        }
                    }
                    Err(error) => {
                        // 单条失败不中断整批：一本坏源不该让另外 500 本导不进来。
                        // 失败逐条记进报告（含 id 与分类），上层据此提示用户。
                        report.skipped += 1;
                        report.failures.push(ImportFailure {
                            subject: source.id.clone(),
                            error: error.to_error_kind(),
                        });
                    }
                }
            }
            transaction.commit().map_err(sql)?;
            Ok(report)
        })
    }

    /// 删除书源（其 Cookie 与脚本缓存一并删除）。
    ///
    /// # 为什么显式删 Cookie 与脚本缓存
    ///
    /// `cookie` 与 `script_cache` 的 `source_id` **没有**外键约束——它们是
    /// 按源分区的辅助数据，加外键会让"先写缓存后导入源"的顺序变得不可能
    /// （脚本可能在源正式落库前就跑过）。故这里必须显式清理，否则删掉的源
    /// 会把自己的登录 token 永久留在库里。
    pub fn remove(&self, source_id: &str) -> Result<(), StoreError> {
        let source_id = source_id.to_string();
        self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            transaction
                .execute("DELETE FROM cookie WHERE source_id=?1", [&source_id])
                .map_err(sql)?;
            transaction
                .execute("DELETE FROM script_cache WHERE source_id=?1", [&source_id])
                .map_err(sql)?;
            let affected = transaction
                .execute("DELETE FROM source WHERE id=?1", [&source_id])
                .map_err(sql)?;
            transaction.commit().map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(source_id));
            }
            Ok(())
        })
    }

    /// 批量设置启用状态（FR-001 的批量启停）。
    ///
    /// 返回受影响的行数。
    pub fn set_enabled(&self, source_ids: &[String], enabled: bool) -> Result<usize, StoreError> {
        if source_ids.is_empty() {
            return Ok(0);
        }
        let ids = source_ids.to_vec();
        self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            let mut affected = 0usize;
            {
                let mut statement = transaction
                    .prepare_cached("UPDATE source SET enabled=?1 WHERE id=?2")
                    .map_err(sql)?;
                for id in &ids {
                    affected += statement
                        .execute(rusqlite::params![i64::from(enabled), id])
                        .map_err(sql)?;
                }
            }
            transaction.commit().map_err(sql)?;
            Ok(affected)
        })
    }

    /// 把一批源归入某分组（`None` 表示移出分组）。
    pub fn assign_group(
        &self,
        source_ids: &[String],
        group_id: Option<i64>,
    ) -> Result<usize, StoreError> {
        if source_ids.is_empty() {
            return Ok(0);
        }
        let ids = source_ids.to_vec();
        self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            let mut affected = 0usize;
            {
                let mut statement = transaction
                    .prepare_cached("UPDATE source SET group_id=?1 WHERE id=?2")
                    .map_err(sql)?;
                for id in &ids {
                    affected += statement
                        .execute(rusqlite::params![group_id, id])
                        .map_err(sql)?;
                }
            }
            transaction.commit().map_err(sql)?;
            Ok(affected)
        })
    }

    // --- 分组 ---

    /// 全部分组（按 `sort_order` 再按 id 排序，顺序确定）。
    pub fn groups(&self) -> Result<Vec<SourceGroup>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM source_group ORDER BY sort_order, id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(SourceGroup {
                        id: row.get("id")?,
                        name: row.get("name")?,
                        sort_order: row.get("sort_order")?,
                        enabled: row.get::<_, i64>("enabled")? != 0,
                    })
                })
                .map_err(sql)?;
            let mut groups = Vec::new();
            for row in rows {
                groups.push(row.map_err(sql)?);
            }
            Ok(groups)
        })
    }

    /// 建分组（同名分组**复用**已有行而不是报错）。
    ///
    /// # 为什么同名是复用而非错误
    ///
    /// 导入书源时源里带的 `bookSourceGroup` 是自由文本，用户填重复的名字是
    /// 常态。若报唯一约束冲突，整批导入会因"分组名重复"失败——而那不是用户
    /// 关心的错误。`ON CONFLICT(name) DO NOTHING` + 回查让语义变成"确保存在"。
    pub fn ensure_group(&self, name: &str, sort_order: i32) -> Result<i64, StoreError> {
        let name = name.to_string();
        self.db.write(move |connection| {
            connection
                .execute(
                    "INSERT INTO source_group(name, sort_order, enabled) VALUES (?1, ?2, 1)
                     ON CONFLICT(name) DO NOTHING",
                    rusqlite::params![name, sort_order],
                )
                .map_err(sql)?;
            connection
                .query_row(
                    "SELECT id FROM source_group WHERE name=?1",
                    [&name],
                    |row| row.get(0),
                )
                .map_err(sql)
        })
    }

    /// 删除分组。组内书源**不删除**，只解除归属（schema 的 `ON DELETE SET NULL`）。
    pub fn remove_group(&self, group_id: i64) -> Result<(), StoreError> {
        self.db.write(move |connection| {
            let affected = connection
                .execute("DELETE FROM source_group WHERE id=?1", [group_id])
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("source_group#{group_id}")));
            }
            Ok(())
        })
    }

    /// 书源总数（诊断与报告用）。
    pub fn count(&self) -> Result<i64, StoreError> {
        self.db.read(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM source", [], |row| row.get(0))
                .map_err(sql)
        })
    }
}

/// 记录应当写入的 `created_at`。
///
/// `Source::created_at` 为 0 表示"调用方没给"（上层从书源 JSON 解析出的
/// `Source` 就是这么构造的——书源文件里没有"创建时间"这个字段）。此时用
/// DAO 的时钟，而不是把 0 写进去：0 在界面上会显示成 1970 年，而"导入时间"
/// 才是这个字段的语义。
fn effective_created_at(source: &Source, fallback: i64) -> i64 {
    if source.created_at > 0 {
        source.created_at
    } else {
        fallback
    }
}

/// 写入（或更新）单条书源，返回"是否原本已存在"。
///
/// 独立成函数而不是写在 `upsert_all` 里：单条语义是 upsert 的核心，
/// 让它可以被单独测试与阅读，`upsert_all` 只负责循环与计数。
fn upsert_one(
    connection: &Connection,
    source: &Source,
    created_at: i64,
) -> Result<bool, StoreError> {
    let existed: bool = connection
        .query_row("SELECT 1 FROM source WHERE id=?1", [&source.id], |_| Ok(()))
        .map(|_| true)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            other => Err(sql(other)),
        })?;

    connection
        .execute(
            "INSERT INTO source (
                id, name, kind, group_id, enabled, book_source_type, book_url_pattern,
                search_url, explore_url, rule_search, rule_explore, rule_book_info,
                rule_toc, rule_content, header, login_url, login_ui, login_check_js,
                js, charset, rate_limit_ms, weight, explore_screen, comment,
                last_update_time, created_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
             )
             ON CONFLICT(id) DO UPDATE SET
                name=excluded.name,
                kind=excluded.kind,
                group_id=excluded.group_id,
                enabled=excluded.enabled,
                book_source_type=excluded.book_source_type,
                book_url_pattern=excluded.book_url_pattern,
                search_url=excluded.search_url,
                explore_url=excluded.explore_url,
                rule_search=excluded.rule_search,
                rule_explore=excluded.rule_explore,
                rule_book_info=excluded.rule_book_info,
                rule_toc=excluded.rule_toc,
                rule_content=excluded.rule_content,
                header=excluded.header,
                login_url=excluded.login_url,
                login_ui=excluded.login_ui,
                login_check_js=excluded.login_check_js,
                js=excluded.js,
                charset=excluded.charset,
                rate_limit_ms=excluded.rate_limit_ms,
                weight=excluded.weight,
                explore_screen=excluded.explore_screen,
                comment=excluded.comment,
                last_update_time=excluded.last_update_time",
            rusqlite::params![
                source.id,
                source.name,
                source.kind.as_storage_code(),
                source.group_id,
                i64::from(source.enabled),
                source.book_source_type,
                source.book_url_pattern,
                source.search_url,
                source.explore_url,
                source.rule_search,
                source.rule_explore,
                source.rule_book_info,
                source.rule_toc,
                source.rule_content,
                source.header,
                source.login_url,
                source.login_ui,
                source.login_check_js,
                source.js,
                source.charset,
                source.rate_limit_ms,
                source.weight,
                source.explore_screen,
                source.comment,
                source.last_update_time,
                // `created_at` 只在 INSERT 时生效（`DO UPDATE` 的 SET 列表里
                // 没有它）——更新一条已存在的源不该把它"变成刚创建的"。
                // `created_at` 为 0 表示"调用方没给"（上层构造 DTO 时常见），
                // 此时用 DAO 的时钟而不是把 0 写进去——0 会显示成 1970 年。
                effective_created_at(source, created_at),
            ],
        )
        .map_err(sql)?;
    Ok(existed)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: std::path::PathBuf,
        dao: SourceDao,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-source-dao-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");
        Fixture {
            dao: SourceDao::with_now(db, 1_700_000_000),
            dir,
        }
    }

    fn sample(id: &str, name: &str) -> Source {
        Source {
            id: id.to_string(),
            name: name.to_string(),
            kind: SourceKind::Book,
            group_id: None,
            enabled: true,
            book_source_type: None,
            book_url_pattern: None,
            search_url: Some("/s?q={{key}}".to_string()),
            explore_url: None,
            rule_search: Some("{\"bookList\":\"@css:li\"}".to_string()),
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
            comment: None,
            last_update_time: None,
            created_at: 1_700_000_000,
        }
    }

    #[test]
    fn 全字段往返不丢() {
        let fixture = fixture("roundtrip");
        let mut source = sample("https://a.com", "源A");
        source.kind = SourceKind::Dictionary;
        source.explore_url = Some("/explore/{{page}}".to_string());
        source.rule_content = Some("{\"content\":\"@css:#c\"}".to_string());
        source.header = Some("{\"User-Agent\":\"x\"}".to_string());
        source.login_url = Some("https://a.com/login".to_string());
        source.login_ui = Some("<ui/>".to_string());
        source.login_check_js = Some("java.getCookie('t')".to_string());
        source.js = Some("function search(){}".to_string());
        source.charset = Some("gbk".to_string());
        source.explore_screen = Some("[{\"name\":\"分类\"}]".to_string());
        source.comment = Some("备注".to_string());
        source.book_url_pattern = Some("https://a.com/book/\\d+".to_string());
        source.book_source_type = Some(0);
        source.weight = Some(7);
        source.last_update_time = Some(1_700_000_100);

        fixture
            .dao
            .upsert_all(std::slice::from_ref(&source))
            .expect("应能导入");
        let loaded = fixture
            .dao
            .by_id("https://a.com")
            .expect("应能查")
            .expect("应有");
        assert_eq!(loaded, source, "每一个字段都要原样往返");
    }

    #[test]
    fn 同_id_导入按更新处理() {
        let fixture = fixture("upsert");
        let report = fixture
            .dao
            .upsert_all(&[sample("https://a.com", "旧名")])
            .expect("应能导入");
        assert_eq!(report.added, 1);
        assert_eq!(report.updated, 0);

        let report = fixture
            .dao
            .upsert_all(&[sample("https://a.com", "新名")])
            .expect("应能导入");
        assert_eq!(report.added, 0, "同 id 不该重复新增");
        assert_eq!(report.updated, 1);
        assert_eq!(fixture.dao.count().expect("应能计数"), 1);
        assert_eq!(
            fixture
                .dao
                .by_id("https://a.com")
                .expect("应能查")
                .expect("应有")
                .name,
            "新名"
        );
    }

    #[test]
    fn 更新不抹掉按源数据() {
        // 这是 `INSERT OR REPLACE` 与 upsert 的关键差别：OR REPLACE 先删后插，
        // 外键级联会把用户的登录态一起抹掉（FR-026 要求保留）。
        let fixture = fixture("preserve");
        fixture
            .dao
            .upsert_all(&[sample("https://a.com", "源A")])
            .expect("应能导入");

        // 模拟脚本写下的登录态
        fixture
            .dao
            .db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO cookie(source_id, name, value, domain)
                         VALUES ('https://a.com', 'token', 'abc', 'script')",
                        [],
                    )
                    .map_err(sql)
            })
            .expect("应能写 cookie");

        fixture
            .dao
            .upsert_all(&[sample("https://a.com", "改名了")])
            .expect("应能更新");

        let remaining: i64 = fixture
            .dao
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM cookie", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查");
        assert_eq!(remaining, 1, "更新书源不得抹掉该源的 Cookie（FR-026）");
    }

    #[test]
    fn 一次导入里的重复_id_计为更新() {
        let fixture = fixture("dup-in-batch");
        let report = fixture
            .dao
            .upsert_all(&[
                sample("https://a.com", "第一次"),
                sample("https://a.com", "第二次"),
            ])
            .expect("应能导入");
        assert_eq!(report.added, 1);
        assert_eq!(report.updated, 1);
        assert_eq!(fixture.dao.count().expect("应能计数"), 1);
    }

    #[test]
    fn 空输入是空报告() {
        let fixture = fixture("empty");
        let report = fixture.dao.upsert_all(&[]).expect("应成功");
        assert_eq!(report, SourceImportReport::default());
    }

    #[test]
    fn 启停与分组() {
        let fixture = fixture("enable");
        fixture
            .dao
            .upsert_all(&[sample("https://a.com", "A"), sample("https://b.com", "B")])
            .expect("应能导入");

        let group_id = fixture.dao.ensure_group("玄幻", 0).expect("应能建组");
        let affected = fixture
            .dao
            .assign_group(&["https://a.com".to_string()], Some(group_id))
            .expect("应能归组");
        assert_eq!(affected, 1);

        let in_group = fixture.dao.by_group(Some(group_id)).expect("应能查");
        assert_eq!(in_group.len(), 1);
        assert_eq!(in_group[0].id, "https://a.com");

        let ungrouped = fixture.dao.by_group(None).expect("应能查");
        assert_eq!(ungrouped.len(), 1);
        assert_eq!(ungrouped[0].id, "https://b.com");

        let affected = fixture
            .dao
            .set_enabled(&["https://a.com".to_string()], false)
            .expect("应能停用");
        assert_eq!(affected, 1);
        let enabled = fixture.dao.enabled_only().expect("应能查");
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].id, "https://b.com");
    }

    #[test]
    fn 同名分组复用而非报错() {
        // 导入时源里带的 group 名重复是常态，不该让整批导入失败
        let fixture = fixture("group-dup");
        let first = fixture.dao.ensure_group("玄幻", 0).expect("应能建");
        let second = fixture.dao.ensure_group("玄幻", 5).expect("应能复用");
        assert_eq!(first, second, "同名分组应复用同一行");
        assert_eq!(fixture.dao.groups().expect("应能查").len(), 1);
    }

    #[test]
    fn 删分组不删源() {
        let fixture = fixture("group-delete");
        fixture
            .dao
            .upsert_all(&[sample("https://a.com", "A")])
            .expect("应能导入");
        let group_id = fixture.dao.ensure_group("玄幻", 0).expect("应能建组");
        fixture
            .dao
            .assign_group(&["https://a.com".to_string()], Some(group_id))
            .expect("应能归组");

        fixture.dao.remove_group(group_id).expect("应能删组");
        assert_eq!(fixture.dao.count().expect("应能计数"), 1, "源不该被删");
        assert_eq!(
            fixture
                .dao
                .by_id("https://a.com")
                .expect("应能查")
                .expect("应有")
                .group_id,
            None,
            "归属应被解除"
        );
    }

    #[test]
    fn 删源连带清掉它的_cookie_与脚本缓存() {
        // 不回清会让删掉的源把自己的登录 token 永久留在库里
        let fixture = fixture("cascade");
        fixture
            .dao
            .upsert_all(&[sample("https://a.com", "A")])
            .expect("应能导入");
        let host = crate::host::SqliteStoreHost::new(
            fixture.dao.db.clone(),
            &fixture.dir,
            "https://a.com",
        );
        host.cookie_put("token", "abc").expect("应能写");
        host.cache_put_inner("k", b"v").expect("应能写");

        fixture.dao.remove("https://a.com").expect("应能删");

        let cookies: i64 = fixture
            .dao
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM cookie", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查");
        let cached: i64 = fixture
            .dao
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM script_cache", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查");
        assert_eq!(cookies, 0, "删源应清掉它的 Cookie");
        assert_eq!(cached, 0, "删源应清掉它的脚本缓存");
    }

    #[test]
    fn 删不存在的源报结构化错误() {
        let fixture = fixture("remove-missing");
        let error = fixture.dao.remove("nope").expect_err("应报错");
        assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn 顺序确定() {
        // T029 的"逐字节一致"要求顺序不随存储层抖动
        let fixture = fixture("order");
        fixture
            .dao
            .upsert_all(&[
                sample("https://z.com", "Z"),
                sample("https://a.com", "A"),
                sample("https://m.com", "M"),
            ])
            .expect("应能导入");
        let ids: Vec<String> = fixture
            .dao
            .all()
            .expect("应能查")
            .into_iter()
            .map(|source| source.id)
            .collect();
        assert_eq!(
            ids,
            vec!["https://a.com", "https://m.com", "https://z.com"],
            "应按 id 排序"
        );
    }

    #[test]
    fn 未知_kind_编码报错而非猜默认值() {
        let fixture = fixture("bad-kind");
        fixture
            .dao
            .db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO source(id, name, kind, enabled, created_at)
                         VALUES ('bad', '坏', 99, 1, 0)",
                        [],
                    )
                    .map_err(sql)
            })
            .expect("应能写");
        let error = fixture.dao.all().expect_err("应报错");
        assert!(
            error.to_string().contains("99"),
            "错误里应带上未知的编码值：{error}"
        );
    }

    #[test]
    fn 单条失败不中断整批() {
        let fixture = fixture("partial");
        // 一条 id 正常、一条违反 NOT NULL（用空 name 触发）
        let mut broken = sample("https://b.com", "");
        broken.name = String::new();
        // name 是 NOT NULL 但空串合法，故改用 group_id 指向不存在的分组触发外键失败
        broken.group_id = Some(9999);

        let report = fixture
            .dao
            .upsert_all(&[sample("https://a.com", "好"), broken])
            .expect("整批不该失败");
        assert_eq!(report.added, 1, "好的一条应落库");
        assert_eq!(report.skipped, 1, "坏的一条应被记为跳过");
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].subject, "https://b.com");
        assert_eq!(fixture.dao.count().expect("应能计数"), 1);
    }
}
