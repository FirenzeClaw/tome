//! \[DONE] DAO·书籍 / 章节 / 库根（tasks.md T057 / FR-023、FR-025）
//!
//! # 身份规则（`data-model.md`）
//!
//! 网络书以 `(书源id, 书籍详情URL)` 唯一；本地书以**归一化文件内容哈希**唯一
//! ——**移动文件不产生重复书目**。两者共同落在 `UNIQUE(origin, origin_id)` 上。
//!
//! ## 为什么身份靠数据库约束而不是靠调用方自觉
//!
//! "同一本书只出现一次"是**跨会话**的不变量：用户今天导入一次、下周从别的
//! 目录再导入一次，两次之间没有任何共享状态可用于判重。唯一能在两次之间
//! 保持的判据就是库里的 `UNIQUE` 约束。放到应用层判重，任何一条新代码路径
//! 漏判就会产生重复书目，而那是用户要手工清理的脏数据。
//!
//! # 批量写章节
//!
//! 目录抓取一次可能落 5 万章（SC-003 的量级）。逐条 INSERT 在这个数量级下
//! 的往返开销不可接受，故 [`BookDao::save_chapters`] 在**一个事务**里用
//! 预编译语句循环——事务把 WAL 提交次数从 5 万降到 1。
//!
//! 仍未用 `sqlx` 风格的批量插入语法：SQLite 的 `VALUES (...),(...)` 有参数
//! 上限（默认 999，编译期常量），5 万章要分 200 批，反而比预编译循环更绕。

use rusqlite::Row;

use tome_core::{Book, BookOrigin, Chapter, LibraryRoot};

use crate::db::sql;
use crate::error::StoreError;

/// 从一行读出书籍。
fn read_book(row: &Row<'_>) -> rusqlite::Result<Book> {
    // 未知 origin 编码报错而非猜默认值：猜成 Network 会让本地书去走网络
    // 管线的路径，错误要到很远的下游才暴露。
    let origin_code: i32 = row.get("origin")?;
    let origin = BookOrigin::from_storage_code(origin_code).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("book.origin 编码 {origin_code} 未知（应为 0 或 1）"),
            )),
        )
    })?;

    Ok(Book {
        id: row.get("id")?,
        origin,
        origin_id: row.get("origin_id")?,
        name: row.get("name")?,
        author: row.get("author")?,
        cover_url: row.get("cover_url")?,
        intro: row.get("intro")?,
        kind: row.get("kind")?,
        book_group: row.get("book_group")?,
        read_config: row.get("read_config")?,
        toc_url: row.get("toc_url")?,
        latest_chapter_title: row.get("latest_chapter_title")?,
        latest_chapter_time: row.get("latest_chapter_time")?,
        library_root_id: row.get("library_root_id")?,
        relative_path: row.get("relative_path")?,
        charset: row.get("charset")?,
        format: row.get("format")?,
        total_chapters: row.get("total_chapters")?,
        dur_chapter_index: row.get("dur_chapter_index")?,
        added_at: row.get("added_at")?,
    })
}

/// 从一行读出章节。
fn read_chapter(row: &Row<'_>) -> rusqlite::Result<Chapter> {
    Ok(Chapter {
        id: row.get("id")?,
        book_id: row.get("book_id")?,
        index_in_book: row.get("index_in_book")?,
        title: row.get("title")?,
        url: row.get("url")?,
        byte_offset: row.get("byte_offset")?,
        epub_entry: row.get("epub_entry")?,
        is_cached: row.get::<_, i64>("is_cached")? != 0,
        is_read: row.get::<_, i64>("is_read")? != 0,
        word_count: row.get("word_count")?,
    })
}

/// 书籍与章节 DAO。
#[derive(Debug, Clone)]
pub struct BookDao {
    db: crate::db::Db,
    now: i64,
}

impl BookDao {
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

    // --- 书架 ---

    /// 书架全量（按加入时间倒序，最近加入的在前）。
    pub fn bookshelf(&self) -> Result<Vec<Book>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM book ORDER BY added_at DESC, id DESC")
                .map_err(sql)?;
            let rows = statement.query_map([], read_book).map_err(sql)?;
            let mut books = Vec::new();
            for row in rows {
                books.push(row.map_err(sql)?);
            }
            Ok(books)
        })
    }

    /// 按身份取书：`(origin, origin_id)`。
    ///
    /// 这是"这本书在书架上吗"的唯一入口——导入前判重、换源时定位都走它。
    pub fn by_origin(
        &self,
        origin: BookOrigin,
        origin_id: &str,
    ) -> Result<Option<Book>, StoreError> {
        let origin_id = origin_id.to_string();
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM book WHERE origin=?1 AND origin_id=?2")
                .map_err(sql)?;
            let mut rows = statement
                .query(rusqlite::params![origin.as_storage_code(), origin_id])
                .map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(read_book(row).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// 按主键取书。
    pub fn by_id(&self, book_id: i64) -> Result<Option<Book>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM book WHERE id=?1")
                .map_err(sql)?;
            let mut rows = statement.query([book_id]).map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(read_book(row).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// 加入书架，返回分配好的主键。
    ///
    /// # 同一本书重复加入时返回**已有**的主键
    ///
    /// 身份规则（`UNIQUE(origin, origin_id)`）意味着"加入"这个动作天生是幂等的。
    /// 报唯一约束冲突会让调用方不得不先查后写（多一次往返 + TOCTOU 窗口），
    /// 而返回已有主键直接给出调用方真正想要的东西——那本书的 id。
    pub fn add_book(&self, book: &Book) -> Result<i64, StoreError> {
        let book = book.clone();
        let now = self.now;
        self.db.write(move |connection| {
            let added_at = if book.added_at > 0 {
                book.added_at
            } else {
                now
            };
            connection
                .execute(
                    "INSERT INTO book (
                        origin, origin_id, name, author, cover_url, intro, kind, book_group,
                        read_config, toc_url, latest_chapter_title, latest_chapter_time,
                        library_root_id, relative_path, charset, format, total_chapters,
                        dur_chapter_index, added_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                        ?16, ?17, ?18, ?19
                     )
                     ON CONFLICT(origin, origin_id) DO NOTHING",
                    rusqlite::params![
                        book.origin.as_storage_code(),
                        book.origin_id,
                        book.name,
                        book.author,
                        book.cover_url,
                        book.intro,
                        book.kind,
                        book.book_group,
                        book.read_config,
                        book.toc_url,
                        book.latest_chapter_title,
                        book.latest_chapter_time,
                        book.library_root_id,
                        book.relative_path,
                        book.charset,
                        book.format,
                        book.total_chapters,
                        book.dur_chapter_index,
                        added_at,
                    ],
                )
                .map_err(sql)?;
            connection
                .query_row(
                    "SELECT id FROM book WHERE origin=?1 AND origin_id=?2",
                    rusqlite::params![book.origin.as_storage_code(), book.origin_id],
                    |row| row.get(0),
                )
                .map_err(sql)
        })
    }

    /// 设置该书的排版配置（`Book.read_config`，002 §FR-013 的 schema 载体）。
    ///
    /// # 为什么是单列更新而不是 `update_book`
    ///
    /// `update_book` 写全部 17 列。用它来设一个配置字段意味着：调用方先读出
    /// 整行（此时它快照了一份**别的字段**），改一处，再整行写回。并发场景下
    /// 那份快照可能已经陈旧——写入会顺手把别人刚更新的 `latest_chapter_title`
    /// 之类改回去。用户看到的现象是"改了个字号，最新章节标题退回去了"，
    /// 而两次操作在代码上毫无关联。
    ///
    /// 单列 UPDATE 把这次写入的影响面收窄到它自己，从而让
    /// "设置排版配置"不影响任何其他字段成为**结构性事实**。
    ///
    /// # `None` 的语义（裁决 OQ#12→A）
    ///
    /// `None` = 该书**跟随全局默认**。它与 `Some(默认配置的 JSON)` 有区别：
    /// 前者在 UI 上显示"跟随全局"、后者显示"已为本书设置"。故本方法
    /// MUST 把 `None` 写成 SQL NULL，而不是写一个默认配置的副本。
    ///
    /// # 存储层不解析这个字符串
    ///
    /// 001 契约明文"内容由 S2 排版层解释，此处不透明"。存储层若解析它，
    /// 就产生了第二个 schema 定义点——S2 改字段时这里要跟着改，而它本不该
    /// 知道那些字段。校验在 `tome-reader::paginate::config`。
    pub fn set_read_config(&self, book_id: i64, json: Option<&str>) -> Result<(), StoreError> {
        let json = json.map(str::to_owned);
        self.db.write(move |connection| {
            let affected = connection
                .execute(
                    "UPDATE book SET read_config=?2 WHERE id=?1",
                    rusqlite::params![book_id, json],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("book#{book_id}")));
            }
            Ok(())
        })
    }

    /// 更新书籍元信息（按 id 定位；身份字段 `origin`/`origin_id` 不改）。
    ///
    /// # 为什么身份字段不在更新范围内
    ///
    /// 改 `origin_id` 等于把这本书变成另一本书——那会让它的章节、痕迹、进度
    /// 全部挂在一个错误的身份上。真要换身份应删了重加（`remove_book` 会级联
    /// 清理，语义明确）。
    pub fn update_book(&self, book: &Book) -> Result<(), StoreError> {
        let book = book.clone();
        self.db.write(move |connection| {
            let affected = connection
                .execute(
                    "UPDATE book SET
                        name=?2, author=?3, cover_url=?4, intro=?5, kind=?6, book_group=?7,
                        read_config=?8, toc_url=?9, latest_chapter_title=?10,
                        latest_chapter_time=?11, library_root_id=?12, relative_path=?13,
                        charset=?14, format=?15, total_chapters=?16, dur_chapter_index=?17
                     WHERE id=?1",
                    rusqlite::params![
                        book.id,
                        book.name,
                        book.author,
                        book.cover_url,
                        book.intro,
                        book.kind,
                        book.book_group,
                        book.read_config,
                        book.toc_url,
                        book.latest_chapter_title,
                        book.latest_chapter_time,
                        book.library_root_id,
                        book.relative_path,
                        book.charset,
                        book.format,
                        book.total_chapters,
                        book.dur_chapter_index,
                    ],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("book#{}", book.id)));
            }
            Ok(())
        })
    }

    /// 移除书籍。其章节、进度、痕迹、缓存索引经外键级联一并消失。
    ///
    /// # 为什么调用方不必（也不能）自己逐表删
    ///
    /// schema 的 `ON DELETE CASCADE` 覆盖了全部从属表。应用层逐表删的风险是
    /// **漏一张表就是永久残留**，而残留是静默的——直到某天查询出现指向不存在
    /// 书籍的痕迹。交给数据库则"从属关系"只有一个定义点（建表语句）。
    ///
    /// # 但不含 Pack 文件与图片文件
    ///
    /// 那是文件系统上的东西，数据库的级联管不着。接缝的 `remove_book`
    /// 契约只说"含其章节、痕迹、进度"，缓存清理有独立入口（`clear_cache`）。
    /// 故这里只删索引行，遗留的 Pack 容器成为孤儿——由 T060 的孤儿回收处理。
    pub fn remove_book(&self, book_id: i64) -> Result<(), StoreError> {
        self.db.write(move |connection| {
            let affected = connection
                .execute("DELETE FROM book WHERE id=?1", [book_id])
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("book#{book_id}")));
            }
            Ok(())
        })
    }

    // --- 章节 ---

    /// 某书的章节列表（按书内序号）。
    pub fn chapters(&self, book_id: i64) -> Result<Vec<Chapter>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM chapter WHERE book_id=?1 ORDER BY index_in_book")
                .map_err(sql)?;
            let rows = statement.query_map([book_id], read_chapter).map_err(sql)?;
            let mut chapters = Vec::new();
            for row in rows {
                chapters.push(row.map_err(sql)?);
            }
            Ok(chapters)
        })
    }

    /// 单章（按书 + 书内序号）。
    pub fn chapter_at(
        &self,
        book_id: i64,
        index_in_book: i32,
    ) -> Result<Option<Chapter>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM chapter WHERE book_id=?1 AND index_in_book=?2")
                .map_err(sql)?;
            let mut rows = statement
                .query(rusqlite::params![book_id, index_in_book])
                .map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(read_chapter(row).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// 批量写入/更新章节（目录抓取后一次性落库）。
    ///
    /// 在一个事务里跑预编译语句，5 万章只有一次 WAL 提交。
    ///
    /// # 冲突处理是 UPDATE 而非忽略
    ///
    /// `(book_id, index_in_book)` 冲突意味着"这一章的元信息变了"（标题改写、
    /// URL 变更、目录重排后同一序号对应另一章）。忽略会让**旧标题永久留存**，
    /// 而用户在目录页看到的是旧的。故用 `DO UPDATE` 覆盖除 `is_read` 之外的
    /// 全部字段。
    ///
    /// # 为什么 `is_read` 与 `is_cached` 不在覆盖范围
    ///
    /// 它们是**用户状态**而不是源的元信息：重抓目录不该把"已读"标记清掉，
    /// 也不该把"已缓存"标记清掉（内容还在 Pack 里，索引行也没变）。
    /// 覆盖它们会造成"更新一次目录，所有已读章节全部变回未读"。
    pub fn save_chapters(&self, book_id: i64, chapters: &[Chapter]) -> Result<(), StoreError> {
        if chapters.is_empty() {
            return Ok(());
        }
        let chapters = chapters.to_vec();
        self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            {
                let mut statement = transaction
                    .prepare_cached(
                        "INSERT INTO chapter (
                            book_id, index_in_book, title, url, byte_offset, epub_entry,
                            is_cached, is_read, word_count
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                         ON CONFLICT(book_id, index_in_book) DO UPDATE SET
                            title=excluded.title,
                            url=excluded.url,
                            byte_offset=excluded.byte_offset,
                            epub_entry=excluded.epub_entry,
                            word_count=excluded.word_count",
                    )
                    .map_err(sql)?;
                for chapter in &chapters {
                    statement
                        .execute(rusqlite::params![
                            book_id,
                            chapter.index_in_book,
                            chapter.title,
                            chapter.url,
                            chapter.byte_offset,
                            chapter.epub_entry,
                            i64::from(chapter.is_cached),
                            i64::from(chapter.is_read),
                            chapter.word_count,
                        ])
                        .map_err(sql)?;
                }
            }
            transaction.commit().map_err(sql)?;
            Ok(())
        })
    }

    /// 标记章节已读。
    pub fn mark_read(&self, chapter_id: i64, is_read: bool) -> Result<(), StoreError> {
        self.db.write(move |connection| {
            let affected = connection
                .execute(
                    "UPDATE chapter SET is_read=?1 WHERE id=?2",
                    rusqlite::params![i64::from(is_read), chapter_id],
                )
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("chapter#{chapter_id}")));
            }
            Ok(())
        })
    }

    /// 章节总数（诊断与 SC-005b 的文件数断言用）。
    pub fn chapter_count(&self, book_id: i64) -> Result<i64, StoreError> {
        self.db.read(move |connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM chapter WHERE book_id=?1",
                    [book_id],
                    |row| row.get(0),
                )
                .map_err(sql)
        })
    }

    // --- 库根目录 ---

    /// 库根目录列表（FR-023：支持多个）。
    pub fn library_roots(&self) -> Result<Vec<LibraryRoot>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT * FROM library_root ORDER BY id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(LibraryRoot {
                        id: row.get("id")?,
                        path: row.get("path")?,
                        added_at: row.get("added_at")?,
                    })
                })
                .map_err(sql)?;
            let mut roots = Vec::new();
            for row in rows {
                roots.push(row.map_err(sql)?);
            }
            Ok(roots)
        })
    }

    /// 添加库根目录（FR-023：**只索引不移动或改写原文件**）。
    ///
    /// 路径已存在时返回**已有**的主键——同一目录添加两次是用户误操作，
    /// 不该产生两条记录（否则扫描会做两遍、书目会重复）。
    pub fn add_library_root(&self, path: &str) -> Result<i64, StoreError> {
        self.add_library_root_at(path, self.now)
    }

    /// 备份导入用：保留备份里的 `added_at` 而非重新打时间戳。
    ///
    /// FR-033 的对称往返要求「导出→清空→导入→再导出」逐字段一致——若导入时
    /// 重新生成 added_at，跨秒边界会让两次导出不一致（CI 慢机上实测变红）。
    pub fn add_library_root_at(&self, path: &str, added_at: i64) -> Result<i64, StoreError> {
        let path = path.to_string();
        self.db.write(move |connection| {
            connection
                .execute(
                    "INSERT INTO library_root(path, added_at) VALUES (?1, ?2)
                     ON CONFLICT(path) DO NOTHING",
                    rusqlite::params![path, added_at],
                )
                .map_err(sql)?;
            connection
                .query_row(
                    "SELECT id FROM library_root WHERE path=?1",
                    [&path],
                    |row| row.get(0),
                )
                .map_err(sql)
        })
    }

    /// 移除库根目录。已在书架上的本地书**不删除**（只解除归属）——
    /// 用户可能只是暂时移除了一个目录。
    pub fn remove_library_root(&self, root_id: i64) -> Result<(), StoreError> {
        self.db.write(move |connection| {
            let affected = connection
                .execute("DELETE FROM library_root WHERE id=?1", [root_id])
                .map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("library_root#{root_id}")));
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: std::path::PathBuf,
        dao: BookDao,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-book-dao-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");
        Fixture {
            dao: BookDao::with_now(db, 1_700_000_000),
            dir,
        }
    }

    fn sample_book(origin_id: &str) -> Book {
        Book {
            id: 0,
            origin: BookOrigin::Network,
            origin_id: origin_id.to_string(),
            name: "斗破苍穹".to_string(),
            author: Some("天蚕土豆".to_string()),
            cover_url: Some("https://x/c.jpg".to_string()),
            intro: Some("简介".to_string()),
            kind: Some("玄幻".to_string()),
            book_group: None,
            read_config: None,
            toc_url: Some("https://x/toc".to_string()),
            latest_chapter_title: Some("第1章".to_string()),
            latest_chapter_time: Some(1_700_000_000),
            library_root_id: None,
            relative_path: None,
            charset: None,
            format: None,
            total_chapters: Some(100),
            dur_chapter_index: Some(0),
            added_at: 0,
        }
    }

    fn sample_chapter(index: i32, title: &str) -> Chapter {
        Chapter {
            id: 0,
            book_id: 0,
            index_in_book: index,
            title: title.to_string(),
            url: Some(format!("https://x/c/{index}")),
            byte_offset: None,
            epub_entry: None,
            is_cached: false,
            is_read: false,
            word_count: Some(3000),
        }
    }

    #[test]
    fn 书全字段往返不丢() {
        let fixture = fixture("book-roundtrip");
        let book = sample_book("https://a.com|https://a.com/book/1");
        let book_id = fixture.dao.add_book(&book).expect("应能加入");
        let loaded = fixture.dao.by_id(book_id).expect("应能查").expect("应有");
        assert_eq!(loaded.name, book.name);
        assert_eq!(loaded.author, book.author);
        assert_eq!(loaded.origin, BookOrigin::Network);
        assert_eq!(loaded.origin_id, book.origin_id);
        assert_eq!(loaded.total_chapters, book.total_chapters);
        assert_eq!(
            loaded.added_at, 1_700_000_000,
            "未给 added_at 时用 DAO 时钟"
        );
    }

    #[test]
    fn 同身份重复加入返回同一主键() {
        // 身份规则（移动文件/重复导入不产生重复书目）的落点
        let fixture = fixture("identity");
        let book = sample_book("hash-abc");
        let first = fixture.dao.add_book(&book).expect("应能加入");
        let second = fixture.dao.add_book(&book).expect("应能重复加入");
        assert_eq!(first, second, "同一身份应是同一本书");
        assert_eq!(fixture.dao.bookshelf().expect("应能查").len(), 1);
    }

    #[test]
    fn 不同来源的相同_origin_id_是两本书() {
        let fixture = fixture("origin-differs");
        let mut network = sample_book("same-id");
        network.origin = BookOrigin::Network;
        let mut local = sample_book("same-id");
        local.origin = BookOrigin::Local;

        let network_id = fixture.dao.add_book(&network).expect("应能加入");
        let local_id = fixture.dao.add_book(&local).expect("应能加入");
        assert_ne!(network_id, local_id);
        assert_eq!(fixture.dao.bookshelf().expect("应能查").len(), 2);
    }

    #[test]
    fn 更新书籍不改变身份() {
        let fixture = fixture("update");
        let book = sample_book("id-1");
        let book_id = fixture.dao.add_book(&book).expect("应能加入");

        let mut updated = book.clone();
        updated.id = book_id;
        updated.name = "新书名".to_string();
        updated.origin_id = "换个身份".to_string(); // 应被忽略
        fixture.dao.update_book(&updated).expect("应能更新");

        let loaded = fixture.dao.by_id(book_id).expect("应能查").expect("应有");
        assert_eq!(loaded.name, "新书名");
        assert_eq!(loaded.origin_id, "id-1", "身份字段不得被 update 改动");
    }

    #[test]
    fn 更新不存在的书报结构化错误() {
        let fixture = fixture("update-missing");
        let mut book = sample_book("x");
        book.id = 999;
        let error = fixture.dao.update_book(&book).expect_err("应报错");
        assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn 删书级联清掉章节与痕迹() {
        let fixture = fixture("cascade");
        let book_id = fixture
            .dao
            .add_book(&sample_book("id-1"))
            .expect("应能加入");
        fixture
            .dao
            .save_chapters(book_id, &[sample_chapter(0, "第1章")])
            .expect("应能写章节");
        fixture
            .dao
            .db
            .write(move |connection| {
                connection
                    .execute(
                        "INSERT INTO bookmark(book_id, chapter_index, char_offset, note, created_at)
                         VALUES (?1, 0, 10, 'n', 0)",
                        [book_id],
                    )
                    .map_err(sql)
            })
            .expect("应能写书签");

        fixture.dao.remove_book(book_id).expect("应能删");

        let chapters: i64 = fixture
            .dao
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM chapter", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查");
        let bookmarks: i64 = fixture
            .dao
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM bookmark", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查");
        assert_eq!(chapters, 0, "删书应级联清掉章节");
        assert_eq!(bookmarks, 0, "删书应级联清掉痕迹");
    }

    #[test]
    fn 批量章节往返且次序正确() {
        let fixture = fixture("chapters");
        let book_id = fixture.dao.add_book(&sample_book("x")).expect("应能加入");
        let chapters: Vec<Chapter> = (0..50)
            .map(|index| sample_chapter(index, &format!("第{index}章")))
            .collect();
        fixture
            .dao
            .save_chapters(book_id, &chapters)
            .expect("应能批量写");

        let loaded = fixture.dao.chapters(book_id).expect("应能查");
        assert_eq!(loaded.len(), 50);
        assert_eq!(loaded[0].index_in_book, 0);
        assert_eq!(loaded[49].title, "第49章");
        assert_eq!(fixture.dao.chapter_count(book_id).expect("应能计数"), 50);
    }

    #[test]
    fn 重抓目录不抹掉已读与已缓存标记() {
        // 这两个是用户状态而非源的元信息——覆盖它们会造成
        // "更新一次目录，所有已读章节全部变回未读"
        let fixture = fixture("preserve-state");
        let book_id = fixture.dao.add_book(&sample_book("x")).expect("应能加入");
        fixture
            .dao
            .save_chapters(book_id, &[sample_chapter(0, "旧标题")])
            .expect("应能写");

        let chapter = fixture
            .dao
            .chapter_at(book_id, 0)
            .expect("应能查")
            .expect("应有");
        fixture
            .dao
            .mark_read(chapter.id, true)
            .expect("应能标记已读");
        fixture
            .dao
            .db
            .write(move |connection| {
                connection
                    .execute("UPDATE chapter SET is_cached=1 WHERE id=?1", [chapter.id])
                    .map_err(sql)
            })
            .expect("应能标记已缓存");

        // 重抓目录：标题变了
        fixture
            .dao
            .save_chapters(book_id, &[sample_chapter(0, "新标题")])
            .expect("应能重写");

        let reloaded = fixture
            .dao
            .chapter_at(book_id, 0)
            .expect("应能查")
            .expect("应有");
        assert_eq!(reloaded.title, "新标题", "源的元信息应被更新");
        assert!(reloaded.is_read, "已读标记不得被清");
        assert!(reloaded.is_cached, "已缓存标记不得被清");
    }

    #[test]
    fn 重抓目录不产生重复章节() {
        let fixture = fixture("no-dup");
        let book_id = fixture.dao.add_book(&sample_book("x")).expect("应能加入");
        let chapters: Vec<Chapter> = (0..10)
            .map(|index| sample_chapter(index, &format!("第{index}章")))
            .collect();
        fixture
            .dao
            .save_chapters(book_id, &chapters)
            .expect("应能写");
        fixture
            .dao
            .save_chapters(book_id, &chapters)
            .expect("应能重写");
        assert_eq!(
            fixture.dao.chapter_count(book_id).expect("应能计数"),
            10,
            "重抓不产生重复行"
        );
    }

    #[test]
    fn 目录重排后同序号对应新章() {
        // 目录重排时同一序号对应的是另一章：元信息必须被覆盖，
        // 否则用户点开"第 5 章"看到的是旧标题
        let fixture = fixture("reorder");
        let book_id = fixture.dao.add_book(&sample_book("x")).expect("应能加入");
        fixture
            .dao
            .save_chapters(book_id, &[sample_chapter(5, "旧的第五章")])
            .expect("应能写");
        fixture
            .dao
            .save_chapters(book_id, &[sample_chapter(5, "新的第五章")])
            .expect("应能写");
        let chapter = fixture
            .dao
            .chapter_at(book_id, 5)
            .expect("应能查")
            .expect("应有");
        assert_eq!(chapter.title, "新的第五章");
    }

    #[test]
    fn 空章节列表是空操作() {
        let fixture = fixture("empty-chapters");
        let book_id = fixture.dao.add_book(&sample_book("x")).expect("应能加入");
        fixture.dao.save_chapters(book_id, &[]).expect("应成功");
        assert_eq!(fixture.dao.chapter_count(book_id).expect("应能计数"), 0);
    }

    #[test]
    fn 库根目录去重() {
        let fixture = fixture("roots");
        let first = fixture.dao.add_library_root("/books").expect("应能添加");
        let second = fixture
            .dao
            .add_library_root("/books")
            .expect("应能重复添加");
        assert_eq!(first, second, "同一目录添加两次应是同一条记录");
        assert_eq!(fixture.dao.library_roots().expect("应能查").len(), 1);
    }

    /// FR-033 对称往返的回归测试（CI 红于 2026-10-10，windows-gnu runner）：
    /// 备份导入必须保留备份里的 added_at。fixture 的 now 钉在 1700000000，
    /// 而此处显式传入另一个值——若导入路径改回「重新打时间戳」，本测试立刻红，
    /// 不依赖「恰好跨秒」的运气。
    #[test]
    fn 库根导入保留备份时间戳() {
        let fixture = fixture("roots-at");
        fixture
            .dao
            .add_library_root_at("/books", 1_500_000_000)
            .expect("应能按指定时间戳添加");
        let roots = fixture.dao.library_roots().expect("应能查");
        assert_eq!(roots.len(), 1);
        assert_eq!(
            roots[0].added_at, 1_500_000_000,
            "added_at 应为传入值而非 now（1700000000）"
        );
    }

    #[test]
    fn 删库根不删书() {
        let fixture = fixture("root-delete");
        let root_id = fixture.dao.add_library_root("/books").expect("应能添加");
        let mut book = sample_book("/books/a.txt");
        book.origin = BookOrigin::Local;
        book.library_root_id = Some(root_id);
        let book_id = fixture.dao.add_book(&book).expect("应能加入");

        fixture.dao.remove_library_root(root_id).expect("应能删根");

        let loaded = fixture
            .dao
            .by_id(book_id)
            .expect("应能查")
            .expect("书应还在");
        assert_eq!(loaded.library_root_id, None, "归属应被解除");
    }

    #[test]
    fn 未知_origin_编码在写入时就被约束挡住() {
        // schema 的 `CHECK (origin IN (0,1))` 是第一道防线：非法编码根本写不进去。
        // 第二道在 read_book（`from_storage_code` 返回 None 时报错）——
        // 它防的是"通过别的手段修库/旧版本遗留"的数据。
        let fixture = fixture("bad-origin");
        let error = fixture
            .dao
            .db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at)
                         VALUES (9, 'x', '坏', 0)",
                        [],
                    )
                    .map_err(sql)
            })
            .expect_err("非法 origin 应被 CHECK 约束拒绝");
        assert!(
            error.to_string().contains("CHECK constraint"),
            "应是约束失败而非静默接受：{error}"
        );

        // 绕过约束写入（模拟损坏的库）后，读取路径也必须报错而不是猜默认值
        fixture
            .dao
            .db
            .write(|connection| {
                connection
                    .execute_batch("PRAGMA ignore_check_constraints=ON;")
                    .map_err(sql)?;
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at)
                         VALUES (9, 'x', '坏', 0)",
                        [],
                    )
                    .map_err(sql)?;
                connection
                    .execute_batch("PRAGMA ignore_check_constraints=OFF;")
                    .map_err(sql)
            })
            .expect("应能绕过约束写入");
        let error = fixture.dao.bookshelf().expect_err("读取应报错");
        assert!(
            error.to_string().contains('9'),
            "错误应带上未知的编码值：{error}"
        );
    }
}
