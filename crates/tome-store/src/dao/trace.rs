//! \[DONE] DAO·进度 / 阅读记录 / 痕迹（tasks.md T058 / FR-027、FR-028、FR-026b）
//!
//! # 本模块是 FR-028 的执行面
//!
//! **MUST NOT 以渲染坐标存储任何阅读痕迹**。故本模块的每一个写入签名都只
//! 接受 `chapter_index` + `char_offset` + `length`——**结构上没有可以传像素
//! 的地方**。这不是靠注释约束，是靠类型约束：想写坐标进不去。
//!
//! 表结构上还有一道：schema 里三张痕迹表都没有 `x_`/`y_`/`pixel` 类列，
//! 且 `schema.rs` 的测试断言这件事。
//!
//! # 锚点的三要素
//!
//! `read_progress` = `(chapter_index, char_offset)` + `anchor_text`。
//! 前两者定位，后者做语义校验——换源时用它在目标源的邻域缓存里做特征段
//! 匹配（FR-010b）。故 `anchor_text` 不是可选装饰，而是降级链第一级的输入，
//! 写入时不该被省略。
//!
//! # 阅读记录为什么按天聚合
//!
//! `read_record` 的主键是 `(book_id, date)`（`data-model.md`）。FR-026b 要的是
//! "可聚合为日/总量统计"，日粒度正是那个聚合单位。故累加写入用
//! `ON CONFLICT DO UPDATE SET read_seconds = read_seconds + excluded...`
//! ——在数据库里做加法而不是"读出来加完写回去"，后者在多次快速写入时会丢更新。

use rusqlite::Row;

use tome_core::{Bookmark, Highlight, Memo, ReadProgress, ReadRecord};

use crate::db::sql;
use crate::error::StoreError;

/// 一条痕迹的定位（三张痕迹表共用的形状）。
///
/// # 为什么单独抽出这个类型
///
/// 书签是单点（无 `length`），高亮与批注是区间。三者的共同部分是
/// "书 + 章 + 起点偏移"，抽出来让"位置一律是字符偏移"这件事在类型层面
/// 显式可见——任何往这里加像素字段的改动都会同时触及三个实体，而不得不
/// 面对 FR-028。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharLocation {
    /// 章内序号（0 起）
    pub chapter_index: i32,
    /// 章内 **Unicode 字符**偏移（不是字节偏移）
    pub char_offset: i64,
}

impl CharLocation {
    /// 构造。
    pub fn new(chapter_index: i32, char_offset: i64) -> Self {
        Self {
            chapter_index,
            char_offset,
        }
    }
}

/// 进度 / 阅读记录 / 痕迹 DAO。
#[derive(Debug, Clone)]
pub struct TraceDao {
    db: crate::db::Db,
    now: i64,
}

impl TraceDao {
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

    // --- 阅读进度（FR-027）---

    /// 读进度。没读过时 `Ok(None)`（不是错误——没开始读是正常状态）。
    pub fn progress(&self, book_id: i64) -> Result<Option<ReadProgress>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM read_progress WHERE book_id=?1")
                .map_err(sql)?;
            let mut rows = statement.query([book_id]).map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(read_progress(row).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// 写进度（一册一条，覆盖写）。
    ///
    /// `updated_at` 为 0 时用 DAO 的时钟。
    pub fn save_progress(&self, progress: &ReadProgress) -> Result<(), StoreError> {
        let progress = progress.clone();
        let now = self.now;
        self.db.write(move |connection| {
            let updated_at = if progress.updated_at > 0 {
                progress.updated_at
            } else {
                now
            };
            connection
                .execute(
                    "INSERT INTO read_progress(
                        book_id, chapter_index, char_offset, anchor_text, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(book_id) DO UPDATE SET
                        chapter_index=excluded.chapter_index,
                        char_offset=excluded.char_offset,
                        anchor_text=excluded.anchor_text,
                        updated_at=excluded.updated_at",
                    rusqlite::params![
                        progress.book_id,
                        progress.chapter_index,
                        progress.char_offset,
                        progress.anchor_text,
                        updated_at,
                    ],
                )
                .map_err(sql)?;
            Ok(())
        })
    }

    // --- 阅读记录（FR-026b）---

    /// 累加一条阅读记录（同日同书累加，不覆盖）。
    ///
    /// # 为什么在 SQL 里做加法
    ///
    /// "读出来 + 加 + 写回去"在连续快速写入（如每秒上报一次阅读时长）时会丢
    /// 更新：两次写入都读到旧值，第二次覆盖第一次的结果。`SET read_seconds =
    /// read_seconds + excluded.read_seconds` 由数据库原子完成，没有窗口。
    pub fn add_read_record(&self, record: &ReadRecord) -> Result<(), StoreError> {
        let record = record.clone();
        self.db.write(move |connection| {
            connection
                .execute(
                    "INSERT INTO read_record(book_id, date, read_seconds, read_chars)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(book_id, date) DO UPDATE SET
                        read_seconds = read_seconds + excluded.read_seconds,
                        read_chars   = read_chars   + excluded.read_chars",
                    rusqlite::params![
                        record.book_id,
                        record.date,
                        record.read_seconds,
                        record.read_chars,
                    ],
                )
                .map_err(sql)?;
            Ok(())
        })
    }

    /// 查询阅读记录（按日期范围，闭区间）。
    ///
    /// 日期是 `YYYY-MM-DD` 字符串。这保证了按字典序比较即等价于按日期比较。
    pub fn read_records(
        &self,
        book_id: i64,
        from_date: &str,
        to_date: &str,
    ) -> Result<Vec<ReadRecord>, StoreError> {
        let from_date = from_date.to_string();
        let to_date = to_date.to_string();
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT * FROM read_record
                     WHERE book_id=?1 AND date>=?2 AND date<=?3
                     ORDER BY date",
                )
                .map_err(sql)?;
            let rows = statement
                .query_map(rusqlite::params![book_id, from_date, to_date], read_record)
                .map_err(sql)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(sql)?);
            }
            Ok(records)
        })
    }

    /// 某书的**全部**阅读记录（不限日期，按日期排序）。
    ///
    /// # 为什么单独一个方法而不是用 `read_records(book, "", "9999")`
    ///
    /// 那个写法依赖"空串比任何日期都小、`9999` 比任何日期都大"这一
    /// **字典序技巧**。它能工作，但把"不限范围"的意图编码进了两个魔法
    /// 字符串里——读代码的人得先想明白为什么是这两个值。备份导出
    /// （T087）要的正是"这本书的全部记录"，给它一个语义直白的方法。
    pub fn read_records_of(&self, book_id: i64) -> Result<Vec<ReadRecord>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM read_record WHERE book_id=?1 ORDER BY date")
                .map_err(sql)?;
            let rows = statement
                .query_map(rusqlite::params![book_id], read_record)
                .map_err(sql)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(sql)?);
            }
            Ok(records)
        })
    }

    /// 全部书的阅读记录（按日期范围，跨书聚合统计用）。
    pub fn all_read_records(
        &self,
        from_date: &str,
        to_date: &str,
    ) -> Result<Vec<ReadRecord>, StoreError> {
        let from_date = from_date.to_string();
        let to_date = to_date.to_string();
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT * FROM read_record WHERE date>=?1 AND date<=?2
                     ORDER BY date, book_id",
                )
                .map_err(sql)?;
            let rows = statement
                .query_map(rusqlite::params![from_date, to_date], read_record)
                .map_err(sql)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(sql)?);
            }
            Ok(records)
        })
    }

    // --- 书签 ---

    /// 某书的书签（按章 + 偏移排序，顺序确定）。
    pub fn bookmarks(&self, book_id: i64) -> Result<Vec<Bookmark>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT * FROM bookmark WHERE book_id=?1
                     ORDER BY chapter_index, char_offset, id",
                )
                .map_err(sql)?;
            let rows = statement
                .query_map([book_id], |row| {
                    Ok(Bookmark {
                        id: row.get("id")?,
                        book_id: row.get("book_id")?,
                        chapter_index: row.get("chapter_index")?,
                        char_offset: row.get("char_offset")?,
                        note: row.get("note")?,
                        created_at: row.get("created_at")?,
                    })
                })
                .map_err(sql)?;
            let mut bookmarks = Vec::new();
            for row in rows {
                bookmarks.push(row.map_err(sql)?);
            }
            Ok(bookmarks)
        })
    }

    /// 新增书签，返回主键。
    pub fn add_bookmark(&self, bookmark: &Bookmark) -> Result<i64, StoreError> {
        let bookmark = bookmark.clone();
        let now = self.now;
        self.db.write(move |connection| {
            let created_at = if bookmark.created_at > 0 {
                bookmark.created_at
            } else {
                now
            };
            connection
                .execute(
                    "INSERT INTO bookmark(book_id, chapter_index, char_offset, note, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        bookmark.book_id,
                        bookmark.chapter_index,
                        bookmark.char_offset,
                        bookmark.note,
                        created_at,
                    ],
                )
                .map_err(sql)?;
            Ok(connection.last_insert_rowid())
        })
    }

    /// 删除书签。
    pub fn remove_bookmark(&self, bookmark_id: i64) -> Result<(), StoreError> {
        self.remove_trace("bookmark", bookmark_id)
    }

    // --- 高亮 ---

    /// 某书的高亮。
    pub fn highlights(&self, book_id: i64) -> Result<Vec<Highlight>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT * FROM highlight WHERE book_id=?1
                     ORDER BY chapter_index, char_offset, id",
                )
                .map_err(sql)?;
            let rows = statement
                .query_map([book_id], |row| {
                    Ok(Highlight {
                        id: row.get("id")?,
                        book_id: row.get("book_id")?,
                        chapter_index: row.get("chapter_index")?,
                        char_offset: row.get("char_offset")?,
                        length: row.get("length")?,
                        style: row.get("style")?,
                        note: row.get("note")?,
                        created_at: row.get("created_at")?,
                    })
                })
                .map_err(sql)?;
            let mut highlights = Vec::new();
            for row in rows {
                highlights.push(row.map_err(sql)?);
            }
            Ok(highlights)
        })
    }

    /// 新增高亮，返回主键。
    ///
    /// 负长度在 DB 层被 `CHECK (length >= 0)` 拒绝——区间长度为负在语义上
    /// 不可解释，而"起点 + 负长度"会被下游渲染当作 0 处理，表现为高亮消失。
    pub fn add_highlight(&self, highlight: &Highlight) -> Result<i64, StoreError> {
        let highlight = highlight.clone();
        let now = self.now;
        self.db.write(move |connection| {
            let created_at = if highlight.created_at > 0 {
                highlight.created_at
            } else {
                now
            };
            connection
                .execute(
                    "INSERT INTO highlight(
                        book_id, chapter_index, char_offset, length, style, note, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        highlight.book_id,
                        highlight.chapter_index,
                        highlight.char_offset,
                        highlight.length,
                        highlight.style,
                        highlight.note,
                        created_at,
                    ],
                )
                .map_err(sql)?;
            Ok(connection.last_insert_rowid())
        })
    }

    /// 删除高亮。
    pub fn remove_highlight(&self, highlight_id: i64) -> Result<(), StoreError> {
        self.remove_trace("highlight", highlight_id)
    }

    // --- 批注 ---

    /// 某书的批注。
    pub fn memos(&self, book_id: i64) -> Result<Vec<Memo>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT * FROM memo WHERE book_id=?1
                     ORDER BY chapter_index, char_offset, id",
                )
                .map_err(sql)?;
            let rows = statement
                .query_map([book_id], |row| {
                    Ok(Memo {
                        id: row.get("id")?,
                        book_id: row.get("book_id")?,
                        chapter_index: row.get("chapter_index")?,
                        char_offset: row.get("char_offset")?,
                        length: row.get("length")?,
                        content: row.get("content")?,
                        created_at: row.get("created_at")?,
                    })
                })
                .map_err(sql)?;
            let mut memos = Vec::new();
            for row in rows {
                memos.push(row.map_err(sql)?);
            }
            Ok(memos)
        })
    }

    /// 新增批注，返回主键。
    pub fn add_memo(&self, memo: &Memo) -> Result<i64, StoreError> {
        let memo = memo.clone();
        let now = self.now;
        self.db.write(move |connection| {
            let created_at = if memo.created_at > 0 {
                memo.created_at
            } else {
                now
            };
            connection
                .execute(
                    "INSERT INTO memo(
                        book_id, chapter_index, char_offset, length, content, created_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        memo.book_id,
                        memo.chapter_index,
                        memo.char_offset,
                        memo.length,
                        memo.content,
                        created_at,
                    ],
                )
                .map_err(sql)?;
            Ok(connection.last_insert_rowid())
        })
    }

    /// 删除批注。
    pub fn remove_memo(&self, memo_id: i64) -> Result<(), StoreError> {
        self.remove_trace("memo", memo_id)
    }

    /// 三张痕迹表共用的删除。
    ///
    /// 表名来自本模块的**常量调用点**（不是外部输入），故拼接无注入面；
    /// 用拼接而非三份几乎相同的代码，是因为"删除 + 未命中即报错"的逻辑
    /// 只该有一份——分三份时漏改一处就是某类痕迹删不掉却静默成功。
    fn remove_trace(&self, table: &'static str, trace_id: i64) -> Result<(), StoreError> {
        self.db.write(move |connection| {
            let affected = connection
                .execute(&format!("DELETE FROM {table} WHERE id=?1"), [trace_id])
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("{table}#{trace_id}")));
            }
            Ok(())
        })
    }

    /// 某章的全部痕迹数（诊断用；也是"改字号后痕迹仍在"的断言载体）。
    pub fn trace_count(&self, book_id: i64, chapter_index: i32) -> Result<usize, StoreError> {
        self.db.read(move |connection| {
            let mut total = 0usize;
            for table in ["bookmark", "highlight", "memo"] {
                let count: i64 = connection
                    .query_row(
                        &format!(
                            "SELECT COUNT(*) FROM {table} WHERE book_id=?1 AND chapter_index=?2"
                        ),
                        rusqlite::params![book_id, chapter_index],
                        |row| row.get(0),
                    )
                    .map_err(sql)?;
                total += count as usize;
            }
            Ok(total)
        })
    }
}

/// 从一行读出一条阅读进度。
fn read_progress(row: &Row<'_>) -> rusqlite::Result<ReadProgress> {
    Ok(ReadProgress {
        book_id: row.get("book_id")?,
        chapter_index: row.get("chapter_index")?,
        char_offset: row.get("char_offset")?,
        anchor_text: row.get("anchor_text")?,
        updated_at: row.get("updated_at")?,
    })
}

/// 从一行读出一条阅读记录。
fn read_record(row: &Row<'_>) -> rusqlite::Result<ReadRecord> {
    Ok(ReadRecord {
        book_id: row.get("book_id")?,
        date: row.get("date")?,
        read_seconds: row.get("read_seconds")?,
        read_chars: row.get("read_chars")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: std::path::PathBuf,
        dao: TraceDao,
        book_id: i64,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-trace-dao-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");

        let book_id: i64 = db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at)
                         VALUES (0, 'x', '书', 0)",
                        [],
                    )
                    .map_err(sql)?;
                Ok(connection.last_insert_rowid())
            })
            .expect("应能建书");

        Fixture {
            dao: TraceDao::with_now(db, 1_700_000_000),
            dir,
            book_id,
        }
    }

    // --- 进度 ---

    #[test]
    fn 进度往返且锚点三要素齐全() {
        let fixture = fixture("progress");
        let progress = ReadProgress {
            book_id: fixture.book_id,
            chapter_index: 12,
            char_offset: 3456,
            anchor_text: Some("他站在山巅，风吹起衣角".to_string()),
            updated_at: 0,
        };
        fixture.dao.save_progress(&progress).expect("应能写");

        let loaded = fixture
            .dao
            .progress(fixture.book_id)
            .expect("应能读")
            .expect("应有");
        assert_eq!(loaded.chapter_index, 12);
        assert_eq!(loaded.char_offset, 3456);
        assert_eq!(
            loaded.anchor_text.as_deref(),
            Some("他站在山巅，风吹起衣角"),
            "anchor_text 是换源降级链第一级的输入（FR-010b），不得丢失"
        );
        assert_eq!(loaded.updated_at, 1_700_000_000);
    }

    #[test]
    fn 一册一条_覆盖写() {
        let fixture = fixture("progress-overwrite");
        let mut progress = ReadProgress {
            book_id: fixture.book_id,
            chapter_index: 1,
            char_offset: 100,
            anchor_text: None,
            updated_at: 0,
        };
        fixture.dao.save_progress(&progress).expect("应能写");
        progress.chapter_index = 2;
        progress.char_offset = 200;
        fixture.dao.save_progress(&progress).expect("应能覆盖");

        let loaded = fixture
            .dao
            .progress(fixture.book_id)
            .expect("应能读")
            .expect("应有");
        assert_eq!(loaded.chapter_index, 2);
        assert_eq!(loaded.char_offset, 200);
    }

    #[test]
    fn 未读过的书返回_none() {
        let fixture = fixture("progress-none");
        assert_eq!(
            fixture.dao.progress(fixture.book_id).expect("应成功"),
            None,
            "没开始读是正常状态，不是错误"
        );
    }

    #[test]
    fn 锚点与排版参数无关_同值反复读取一致() {
        // SC-008 的数据层落点：字号/窗口变化后仍解析到同一处。
        // 数据层要保证的是"写进去什么、读出来还是什么"——排版参数在回读
        // 路径上根本不存在，故它无法影响结果。
        let fixture = fixture("anchor-invariance");
        fixture
            .dao
            .save_progress(&ReadProgress {
                book_id: fixture.book_id,
                chapter_index: 3,
                char_offset: 500,
                anchor_text: Some("特征段".to_string()),
                updated_at: 0,
            })
            .expect("应能写");

        for _ in 0..5 {
            let loaded = fixture
                .dao
                .progress(fixture.book_id)
                .expect("应能读")
                .expect("应有");
            assert_eq!((loaded.chapter_index, loaded.char_offset), (3, 500));
        }
    }

    // --- 阅读记录 ---

    #[test]
    fn 同日阅读记录累加而非覆盖() {
        let fixture = fixture("record-accumulate");
        fixture
            .dao
            .add_read_record(&ReadRecord {
                book_id: fixture.book_id,
                date: "2026-10-10".to_string(),
                read_seconds: 60,
                read_chars: 1000,
            })
            .expect("应能写");
        fixture
            .dao
            .add_read_record(&ReadRecord {
                book_id: fixture.book_id,
                date: "2026-10-10".to_string(),
                read_seconds: 30,
                read_chars: 500,
            })
            .expect("应能写");

        let records = fixture
            .dao
            .read_records(fixture.book_id, "2026-10-01", "2026-10-31")
            .expect("应能查");
        assert_eq!(records.len(), 1, "同一天应只有一条");
        assert_eq!(records[0].read_seconds, 90, "秒数应累加");
        assert_eq!(records[0].read_chars, 1500, "字数应累加");
    }

    #[test]
    fn 阅读记录按日期范围查询且有序() {
        let fixture = fixture("record-range");
        for (date, seconds) in [
            ("2026-10-05", 10),
            ("2026-10-01", 20),
            ("2026-10-03", 30),
            ("2026-11-01", 40),
        ] {
            fixture
                .dao
                .add_read_record(&ReadRecord {
                    book_id: fixture.book_id,
                    date: date.to_string(),
                    read_seconds: seconds,
                    read_chars: seconds * 10,
                })
                .expect("应能写");
        }

        let records = fixture
            .dao
            .read_records(fixture.book_id, "2026-10-01", "2026-10-31")
            .expect("应能查");
        let dates: Vec<&str> = records.iter().map(|record| record.date.as_str()).collect();
        assert_eq!(
            dates,
            vec!["2026-10-01", "2026-10-03", "2026-10-05"],
            "应只要范围内的，且按日期排序（11 月那条在外）"
        );
    }

    #[test]
    fn 跨书聚合查询() {
        let fixture = fixture("record-all");
        let second: i64 = fixture
            .dao
            .db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at)
                         VALUES (0, 'y', '书二', 0)",
                        [],
                    )
                    .map_err(sql)?;
                Ok(connection.last_insert_rowid())
            })
            .expect("应能建书二");
        for book_id in [fixture.book_id, second] {
            fixture
                .dao
                .add_read_record(&ReadRecord {
                    book_id,
                    date: "2026-10-10".to_string(),
                    read_seconds: 60,
                    read_chars: 100,
                })
                .expect("应能写");
        }
        let all = fixture
            .dao
            .all_read_records("2026-10-10", "2026-10-10")
            .expect("应能查");
        assert_eq!(all.len(), 2, "两本书各一条");
        assert_eq!(
            all.iter().map(|record| record.read_seconds).sum::<i64>(),
            120
        );
    }

    // --- 痕迹（FR-028）---

    #[test]
    fn 书签往返_单点加备注() {
        let fixture = fixture("bookmark");
        let id = fixture
            .dao
            .add_bookmark(&Bookmark {
                id: 0,
                book_id: fixture.book_id,
                chapter_index: 7,
                char_offset: 123,
                note: Some("这里写得好".to_string()),
                created_at: 0,
            })
            .expect("应能加");

        let bookmarks = fixture.dao.bookmarks(fixture.book_id).expect("应能查");
        assert_eq!(bookmarks.len(), 1);
        assert_eq!(bookmarks[0].id, id);
        assert_eq!(bookmarks[0].chapter_index, 7);
        assert_eq!(bookmarks[0].char_offset, 123);
        assert_eq!(bookmarks[0].note.as_deref(), Some("这里写得好"));
    }

    #[test]
    fn 高亮往返_字符区间加样式() {
        let fixture = fixture("highlight");
        fixture
            .dao
            .add_highlight(&Highlight {
                id: 0,
                book_id: fixture.book_id,
                chapter_index: 2,
                char_offset: 50,
                length: 20,
                style: "yellow".to_string(),
                note: Some("重点".to_string()),
                created_at: 0,
            })
            .expect("应能加");

        let highlights = fixture.dao.highlights(fixture.book_id).expect("应能查");
        assert_eq!(highlights.len(), 1);
        assert_eq!(highlights[0].char_offset, 50);
        assert_eq!(highlights[0].length, 20, "长度单位是字符数");
        assert_eq!(highlights[0].style, "yellow");
    }

    #[test]
    fn 批注往返_区间加内容() {
        let fixture = fixture("memo");
        fixture
            .dao
            .add_memo(&Memo {
                id: 0,
                book_id: fixture.book_id,
                chapter_index: 4,
                char_offset: 10,
                length: 5,
                content: "我的想法".to_string(),
                created_at: 0,
            })
            .expect("应能加");

        let memos = fixture.dao.memos(fixture.book_id).expect("应能查");
        assert_eq!(memos.len(), 1);
        assert_eq!(memos[0].content, "我的想法");
        assert_eq!(memos[0].length, 5);
    }

    #[test]
    fn 负长度被约束拒绝() {
        // "起点 + 负长度"在渲染侧会被当作 0 处理，表现为高亮凭空消失——
        // 与其静默产出不可见的高亮，不如在写入时挡住
        let fixture = fixture("negative-length");
        let error = fixture
            .dao
            .add_highlight(&Highlight {
                id: 0,
                book_id: fixture.book_id,
                chapter_index: 0,
                char_offset: 0,
                length: -1,
                style: "s".to_string(),
                note: None,
                created_at: 0,
            })
            .expect_err("负长度应被拒绝");
        assert!(error.to_string().contains("CHECK constraint"), "{error}");
    }

    #[test]
    fn 删除痕迹() {
        let fixture = fixture("remove");
        let bookmark_id = fixture
            .dao
            .add_bookmark(&Bookmark {
                id: 0,
                book_id: fixture.book_id,
                chapter_index: 0,
                char_offset: 0,
                note: None,
                created_at: 0,
            })
            .expect("应能加");
        fixture.dao.remove_bookmark(bookmark_id).expect("应能删");
        assert!(fixture
            .dao
            .bookmarks(fixture.book_id)
            .expect("应能查")
            .is_empty());

        let error = fixture
            .dao
            .remove_bookmark(bookmark_id)
            .expect_err("重复删除应报错");
        assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn 三类痕迹并存且按章排序() {
        let fixture = fixture("all-traces");
        for (chapter_index, char_offset) in [(5, 100), (1, 200), (3, 50)] {
            fixture
                .dao
                .add_bookmark(&Bookmark {
                    id: 0,
                    book_id: fixture.book_id,
                    chapter_index,
                    char_offset,
                    note: None,
                    created_at: 0,
                })
                .expect("应能加");
        }
        let bookmarks = fixture.dao.bookmarks(fixture.book_id).expect("应能查");
        let order: Vec<i32> = bookmarks
            .iter()
            .map(|bookmark| bookmark.chapter_index)
            .collect();
        assert_eq!(order, vec![1, 3, 5], "应按章排序（顺序确定）");

        fixture
            .dao
            .add_highlight(&Highlight {
                id: 0,
                book_id: fixture.book_id,
                chapter_index: 1,
                char_offset: 0,
                length: 1,
                style: "s".to_string(),
                note: None,
                created_at: 0,
            })
            .expect("应能加");
        fixture
            .dao
            .add_memo(&Memo {
                id: 0,
                book_id: fixture.book_id,
                chapter_index: 1,
                char_offset: 0,
                length: 1,
                content: "c".to_string(),
                created_at: 0,
            })
            .expect("应能加");
        assert_eq!(
            fixture
                .dao
                .trace_count(fixture.book_id, 1)
                .expect("应能计数"),
            3,
            "同一章应有三类痕迹各一条"
        );
    }

    #[test]
    fn 痕迹的位置字段只有字符区间() {
        // FR-028 的结构性证据：写入路径上**没有**任何可以传像素/行号的地方。
        // 这条断言依靠编译期——`Bookmark`/`Highlight`/`Memo` 三个 DTO 的
        // 位置字段就是 chapter_index + char_offset (+ length)，没有别的。
        let fixture = fixture("no-pixels");
        let bookmark = Bookmark {
            id: 0,
            book_id: fixture.book_id,
            chapter_index: 1,
            char_offset: 2,
            note: None,
            created_at: 0,
        };
        // 若日后有人给 Bookmark 加 `x: f32`，本行会因结构体字面量缺字段而
        // 编译失败——那正是我们想要的：改动必须面对 FR-028。
        let _location = CharLocation::new(bookmark.chapter_index, bookmark.char_offset);
        fixture.dao.add_bookmark(&bookmark).expect("应能加");
    }

    #[test]
    fn 痕迹顺序确定() {
        let fixture = fixture("order");
        // 同一章内按偏移排序
        for char_offset in [300, 100, 200] {
            fixture
                .dao
                .add_bookmark(&Bookmark {
                    id: 0,
                    book_id: fixture.book_id,
                    chapter_index: 0,
                    char_offset,
                    note: None,
                    created_at: 0,
                })
                .expect("应能加");
        }
        let offsets: Vec<i64> = fixture
            .dao
            .bookmarks(fixture.book_id)
            .expect("应能查")
            .into_iter()
            .map(|bookmark| bookmark.char_offset)
            .collect();
        assert_eq!(offsets, vec![100, 200, 300]);
    }
}
