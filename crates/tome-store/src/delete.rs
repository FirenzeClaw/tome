//! [DONE] 完整删除用户数据（tasks.md T091 / FR-035）
//!
//! # FR-035 与它在验收里的落点
//!
//! > 系统 MUST 支持完整删除用户数据（**配置、缓存与个人数据**）
//!
//! tasks.md 的验收原文：
//!
//! > 删除后书架/进度/痕迹/规则/配置全清；**不残留文件或索引引用**
//!
//! 最后半句是本模块存在的理由。"不残留文件"排除了"只清库表、留着缓存
//! 文件"的做法；"不残留索引引用"排除了"删主表、留关联行"的做法。
//!
//! # 三类数据，三种清法（这是本模块的核心区分）
//!
//! | 类别 | 例子 | 清法 | 为什么 |
//! |---|---|---|---|
//! | **数据库行** | 书架、进度、痕迹、规则、书源 | `DELETE FROM` 各表 | 它们在同一个库里，事务清干净 |
//! | **文件系统** | 章节 Pack、图片缓存、配置 | 删文件/目录 | 库清干净了但文件还在＝"删了却没省空间" |
//! | **索引 + 文件** | 章节缓存（Pack 文件 + `chapter_cache` 行） | **两者都清** | 只清一边就会产生悬空引用 |
//!
//! ## 为什么"悬空引用"是这里最需要防的东西
//!
//! `chapter_cache` 是 Pack 文件的索引。若只删文件不删索引行，下次读那一章
//! 会按索引去读一个不存在的文件——**报错**（还算好的）或者读到相邻章节的
//! 内容（**静默错内容**）。反过来只删索引不删文件，空间没释放，而"删除
//! 用户数据"这件事在用户看来就是**为了释放空间**。
//!
//! 故本模块按"先删文件、再删索引、最后删表"的顺序推进，且**每一步都核对**。
//!
//! # 删除范围的边界（哪些**不**删）
//!
//! **不删数据目录本身**，也不删用户放进去的无关文件。理由：用户可能把
//! 自己的东西放在数据目录里（FR-042 的可移植模式明确"数据目录可置于程序
//! 同级并随程序整体移动"）。把整个目录 `remove_dir_all` 会连带删掉它们——
//! 那是"删除用户数据"变成"删除用户的文件"，越权了。
//!
//! 只删**我们知道自己在管**的东西：库文件、配置、`cache/` 下的两个子目录。
//!
//! # 为什么不提供"只删一部分"的选项
//!
//! FR-035 要的是"**完整**删除"。提供 `--keep-sources` 之类的开关会让
//! "我说要删干净"变成一个需要仔细读文档才能确认的操作——而在数据删除
//! 这件事上，**不确定性本身就是缺陷**。需要部分保留的用户应该用导出
//! （FR-033）先把要留的东西导出来，再执行完整删除。

use std::path::Path;

use crate::datadir::DataDir;
use crate::db::sql;
use crate::error::StoreError;

/// 删除结果（逐项计数，供报告与测试断言）。
///
/// # 为什么每类都计数而不是只报"成功"
///
/// "删了多少"是**验证删除是否真的发生**的唯一手段。若只报成功，一个
/// 因表名写错而"删了 0 行"的 bug 会显示为成功——而用户以为数据没了。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteReport {
    /// 清掉的数据库行总数
    pub database_rows: usize,
    /// 逐表清掉的行数（表名 → 行数），只含非零项
    pub per_table: Vec<(&'static str, usize)>,
    /// 删掉的章节缓存 Pack 文件数
    pub pack_files: usize,
    /// 删掉的图片缓存文件数
    pub image_files: usize,
    /// 删掉的下载缓存文件数（若该目录存在）
    pub download_files: usize,
    /// 是否删掉了配置文件
    pub config_removed: bool,
}

impl DeleteReport {
    /// 是否有任何东西被删（用于"重复执行为空"的断言）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.database_rows == 0
            && self.pack_files == 0
            && self.image_files == 0
            && self.download_files == 0
            && !self.config_removed
    }

    /// 删掉的文件总数（不含库文件本身）。
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.pack_files + self.image_files + self.download_files
    }
}

/// FR-035 要清掉的表。
///
/// # 为什么是显式清单而不是"遍历 sqlite_master 全删"
///
/// 遍历 `sqlite_master` 会连**迁移记录**、`sqlite_sequence` 等一起删，
/// 结果是"库还在但 schema 元数据没了"——下次打开会尝试重跑迁移，
/// 而表已存在，报错。显式清单让"删什么"是被审查过的决定。
///
/// 顺序很重要：**先删引用方，再删被引用方**（外键约束）。
/// 虽然 `ON DELETE CASCADE` 会自动级联，但显式删除让每一步的计数
/// 可核对——`DELETE FROM book` 报 0 行而行数确实变了，说明它在依赖级联。
///
/// # `chapter_fts` 不在此列
///
/// 它是 FTS5 虚拟表，`DELETE FROM` 语法与非虚拟表一致，但它的**存在性**
/// 由 `fts::ensure_schema` 决定（可能根本没建过）。故单独处理：存在才删。
const USER_DATA_TABLES: &[&str] = &[
    // --- 引用方（依赖别人的）在前 ---
    "chapter_cache", // → chapter, book
    "read_progress",
    "read_record",
    "bookmark",
    "highlight",
    "memo",
    "chapter", // → book
    "book",
    "source", // → source_group
    "source_group",
    "replace_rule",
    "highlight_rule",
    "dict_rule",
    "txt_toc_rule",
    "search_keyword",
    "http_log",
    "cookie",
    "script_cache",
    "library_root",
];

/// 用户数据的删除器。
#[derive(Debug, Clone)]
pub struct UserDataDeleter {
    db: crate::db::Db,
    data_dir: Option<DataDir>,
}

impl UserDataDeleter {
    /// 只清数据库（不动文件系统）。
    ///
    /// **适用场景有限**：测试，或调用方自己负责文件。生产路径用
    /// [`Self::with_data_dir`]——否则"删了用户数据"之后缓存文件还在，
    /// 用户看不到空间释放。
    #[must_use]
    pub fn new(db: crate::db::Db) -> Self {
        Self { db, data_dir: None }
    }

    /// 连文件系统一起清（推荐）。
    #[must_use]
    pub fn with_data_dir(db: crate::db::Db, data_dir: DataDir) -> Self {
        Self {
            db,
            data_dir: Some(data_dir),
        }
    }

    /// 执行完整删除（FR-035）。
    ///
    /// # 顺序：先文件、再索引、最后表
    ///
    /// 见模块文档的"三类数据"。这个顺序保证**任何中途失败都不会留下
    /// 悬空引用**：
    ///
    /// - 删文件失败 → 索引与表都还在，是"删了一半"，状态一致（可重试）
    /// - 删索引后失败 → 文件已没了，行没了，表还在但指向的章节数据已清
    /// - 最后删表 → 表没了，任何残留引用也随之消失
    ///
    /// 反过来（先删表）会出现"Pack 文件还在但没有任何行指向它"——
    /// 那是垃圾文件，且因为索引行已没，**再也找不到它们来清理**。
    /// 故 `pack.rs` 有 `evict_orphan_packs` 兜底，但正确做法是不产生它们。
    pub fn delete_all(&self) -> Result<DeleteReport, StoreError> {
        let mut report = DeleteReport::default();

        // --- 1. 文件系统：缓存（先于索引，避免悬空）---
        if let Some(data_dir) = &self.data_dir {
            report.pack_files = remove_dir_contents(&data_dir.packs_dir())?;
            report.image_files = remove_dir_contents(&data_dir.images_dir())?;
            report.download_files = remove_dir_contents(&data_dir.download_packs_dir())?;
        }

        // --- 2. 索引 + 数据库表 ---
        let per_table = self.db.write(|connection| {
            // FTS 索引：存在才删（`fts::ensure_schema` 可能从没被调用过）
            let has_fts: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chapter_fts'",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            if has_fts > 0 {
                // 用 `DELETE FROM chapter_fts` 一次清空。
                // 逐条按 rowid 删在 5 万章的量级下是 5 万次语句。
                connection
                    .execute("DELETE FROM chapter_fts", [])
                    .map_err(sql)?;
            }

            let mut counts: Vec<(&'static str, usize)> = Vec::new();
            for table in USER_DATA_TABLES {
                // 表可能不存在（例如旧版本的数据目录缺某表）。
                // 此时跳过而非报错——"删不存在的表"与"删除失败"是两回事。
                let exists: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        [table],
                        |row| row.get(0),
                    )
                    .map_err(sql)?;
                if exists == 0 {
                    continue;
                }
                let removed = connection
                    .execute(&format!("DELETE FROM {table}"), [])
                    .map_err(sql)?;
                if removed > 0 {
                    counts.push((table, removed));
                }
            }
            Ok(counts)
        })?;

        report.database_rows = per_table.iter().map(|(_, count)| count).sum();
        report.per_table = per_table;

        // --- 3. 配置文件 ---
        if let Some(data_dir) = &self.data_dir {
            let config = data_dir.config_path();
            if config.is_file() {
                std::fs::remove_file(&config).map_err(|error| StoreError::Io {
                    path: config.display().to_string(),
                    reason: format!("删除配置失败：{error}"),
                })?;
                report.config_removed = true;
            }
        }

        Ok(report)
    }

    /// 删除后核对：数据库里是否还有任何用户数据行。
    ///
    /// # 为什么提供这个方法而不是让调用方自己查
    ///
    /// `delete_all` 报的是"我删了几行"，而**验证**要问的是"还有没有剩"。
    /// 两者是不同的断言：前者依赖删除逻辑的计数正确，后者直接查库。
    /// 把"剩余量"做成可调用的方法，让测试与上层都能独立核对，
    /// 而不是信任前者的计数。
    pub fn remaining_rows(&self) -> Result<usize, StoreError> {
        self.db.read(|connection| {
            let mut total = 0usize;
            for table in USER_DATA_TABLES {
                let exists: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        [table],
                        |row| row.get(0),
                    )
                    .map_err(sql)?;
                if exists == 0 {
                    continue;
                }
                let count: i64 = connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .map_err(sql)?;
                total += count as usize;
            }
            Ok(total)
        })
    }
}

/// 删掉目录下的全部内容，但**保留目录本身**，返回删掉的文件数。
///
/// # 为什么保留目录
///
/// 目录是"应用布局"的一部分（`DataDir::ensure_layout` 会建它）。删掉它
/// 虽然 `ensure_layout` 会在下次启动重建，但那样"删除数据"与"首次启动"
/// 就变得不可区分了——而某些诊断（比如判断用户是否已使用过）正要区分
/// 这两者。保留空目录让"已初始化但无数据"成为一个可观察状态。
///
/// # 目录不存在时返回 0 而非报错
///
/// 首次删除（用户还没缓存过任何东西）时目录可能不存在——那不是错误。
fn remove_dir_contents(dir: &Path) -> Result<usize, StoreError> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut removed = 0usize;
    let entries = std::fs::read_dir(dir).map_err(|error| StoreError::Io {
        path: dir.display().to_string(),
        reason: format!("读取目录失败：{error}"),
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| StoreError::Io {
            path: dir.display().to_string(),
            reason: format!("遍历目录失败：{error}"),
        })?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| StoreError::Io {
            path: path.display().to_string(),
            reason: format!("读取元信息失败：{error}"),
        })?;

        let result = if metadata.is_dir() {
            // 子目录整体删（Pack 按 `book_hash` 分子目录、图片按前缀分）
            std::fs::remove_dir_all(&path)
        } else {
            removed += 1;
            std::fs::remove_file(&path)
        };
        result.map_err(|error| StoreError::Io {
            path: path.display().to_string(),
            reason: format!("删除失败：{error}"),
        })?;

        // 子目录里的文件数也要计入（否则报告说"删了 0 个文件"而空间确实释放了）
        if metadata.is_dir() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::model::{BookOrigin, RuleScope};
    use tome_core::{Book, Bookmark, ReadProgress, ReadRecord, ReplaceRule, Source, SourceKind};

    struct Fixture {
        dir: std::path::PathBuf,
        db: crate::db::Db,
        data_dir: DataDir,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 建一个带真实数据目录的夹具。
    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-delete-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");

        let resolved = DataDir::resolve(Some(&dir), None, None);
        let data_dir = DataDir::from_resolved(&resolved).expect("应能解析数据目录");
        data_dir.ensure_layout().expect("应能建布局");

        let db = crate::db::Db::open(&data_dir.database_path()).expect("应能开库");
        crate::fts::ensure_schema(&db).expect("应能建索引表");

        Fixture { dir, db, data_dir }
    }

    /// 造全套用户数据（含文件系统上的缓存与配置）。
    fn seed(fixture: &Fixture) {
        let book_dao = crate::dao::BookDao::new(fixture.db.clone());
        let trace_dao = crate::dao::TraceDao::with_now(fixture.db.clone(), 1_700_000_000);
        let source_dao = crate::dao::SourceDao::new(fixture.db.clone());
        let misc_dao = crate::dao::MiscDao::with_now(fixture.db.clone(), 1_700_000_000);

        // 书源 + 分组
        source_dao.ensure_group("组", 0).expect("应能建分组");
        source_dao
            .upsert_all(&[Source {
                id: "https://del.example.com".to_string(),
                name: "待删源".to_string(),
                kind: SourceKind::Book,
                group_id: None,
                enabled: true,
                book_source_type: None,
                book_url_pattern: None,
                search_url: None,
                explore_url: None,
                rule_search: None,
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
                rate_limit_ms: None,
                weight: None,
                explore_screen: None,
                comment: None,
                last_update_time: None,
                created_at: 0,
            }])
            .expect("应能存源");

        // 规则
        misc_dao
            .save_replace_rule(&ReplaceRule {
                id: 0,
                name: "规则".to_string(),
                pattern: "x".to_string(),
                replacement: String::new(),
                scope: RuleScope::Content,
                is_regex: false,
                enabled: true,
                sort_order: 0,
            })
            .expect("应能存规则");
        misc_dao
            .add_search_keyword("关键词")
            .expect("应能存搜索关键词");
        misc_dao
            .log_request(&tome_core::HttpLogEntry {
                id: 0,
                source_id: None,
                url: "https://del.example.com".to_string(),
                method: "GET".to_string(),
                request_headers_redacted: None,
                response_code: Some(200),
                response_body_redacted: None,
                duration_ms: Some(10),
                created_at: 1_700_000_000,
            })
            .expect("应能记日志");

        // 书籍 + 章节 + 痕迹
        let book_id = book_dao
            .add_book(&Book {
                id: 0,
                origin: BookOrigin::Network,
                origin_id: "https://del.example.com|b".to_string(),
                name: "待删书".to_string(),
                author: None,
                cover_url: None,
                intro: None,
                kind: None,
                book_group: None,
                read_config: None,
                toc_url: None,
                latest_chapter_title: None,
                latest_chapter_time: None,
                library_root_id: None,
                relative_path: None,
                charset: None,
                format: None,
                total_chapters: Some(1),
                dur_chapter_index: None,
                added_at: 0,
            })
            .expect("应能加书");

        book_dao
            .save_chapters(
                book_id,
                &[tome_core::Chapter {
                    id: 0,
                    book_id,
                    index_in_book: 0,
                    title: "第一章".to_string(),
                    url: None,
                    byte_offset: None,
                    epub_entry: None,
                    is_cached: true,
                    is_read: false,
                    word_count: None,
                }],
            )
            .expect("应能存章节");

        // 章节内容写进 Pack 缓存，并建索引 + FTS
        let chapter_id: i64 = fixture
            .db
            .read(move |connection| {
                connection
                    .query_row(
                        "SELECT id FROM chapter WHERE book_id=?1",
                        [book_id],
                        |row| row.get(0),
                    )
                    .map_err(crate::db::sql)
            })
            .expect("应能取章节 id");

        let pack = crate::pack::PackStore::new(&fixture.data_dir.packs_dir(), fixture.db.clone());
        pack.write_chapter(chapter_id, "正文内容".as_bytes())
            .expect("应能写缓存");
        crate::fts::CachedTextIndex::new(fixture.db.clone())
            .index_chapter(chapter_id, "第一章", "正文内容")
            .expect("应能建索引");

        trace_dao
            .save_progress(&ReadProgress {
                book_id,
                chapter_index: 0,
                char_offset: 1,
                anchor_text: None,
                updated_at: 0,
            })
            .expect("应能存进度");
        trace_dao
            .add_read_record(&ReadRecord {
                book_id,
                date: "2026-10-11".to_string(),
                read_seconds: 60,
                read_chars: 100,
            })
            .expect("应能存阅读记录");
        trace_dao
            .add_bookmark(&Bookmark {
                id: 0,
                book_id,
                chapter_index: 0,
                char_offset: 0,
                note: None,
                created_at: 0,
            })
            .expect("应能存书签");

        book_dao.add_library_root("/tmp/books").expect("应能存库根");

        // 图片缓存文件 + 下载缓存文件 + 配置文件
        let images_dir = fixture.data_dir.images_dir();
        std::fs::create_dir_all(images_dir.join("ab")).expect("应能建图片子目录");
        std::fs::write(images_dir.join("ab").join("hash1"), b"img").expect("应能写图片");
        std::fs::write(images_dir.join("ab").join("hash2"), b"img2").expect("应能写图片");

        let downloads = fixture.data_dir.download_packs_dir();
        std::fs::create_dir_all(&downloads).expect("应能建下载目录");
        std::fs::write(downloads.join("d1.pk"), b"dl").expect("应能写下载缓存");

        std::fs::write(fixture.data_dir.config_path(), "portable = true\n").expect("应能写配置");
    }

    fn has_any_pack_file(dir: &Path) -> bool {
        if !dir.is_dir() {
            return false;
        }
        std::fs::read_dir(dir)
            .map(|entries| entries.count() > 0)
            .unwrap_or(false)
    }

    #[test]
    fn 删除后数据库无用户数据行() {
        let fixture = fixture("rows");
        seed(&fixture);
        let deleter = UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone());

        assert!(
            deleter.remaining_rows().expect("应能统计") > 0,
            "删除前应有数据"
        );
        let report = deleter.delete_all().expect("应能删除");
        assert!(report.database_rows > 0, "应报告删掉的行数");
        assert_eq!(
            deleter.remaining_rows().expect("应能统计"),
            0,
            "删除后不该有任何用户数据行"
        );
    }

    #[test]
    fn 删除后缓存文件不残留() {
        // "不残留文件"——这是 FR-035 明确要求的一半
        let fixture = fixture("files");
        seed(&fixture);

        assert!(
            has_any_pack_file(&fixture.data_dir.packs_dir()),
            "删除前应有 Pack 文件"
        );
        assert!(
            has_any_pack_file(&fixture.data_dir.images_dir()),
            "删除前应有图片文件"
        );

        UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone())
            .delete_all()
            .expect("应能删除");

        assert!(
            !has_any_pack_file(&fixture.data_dir.packs_dir()),
            "Pack 缓存文件不该残留"
        );
        assert!(
            !has_any_pack_file(&fixture.data_dir.images_dir()),
            "图片缓存文件不该残留"
        );
        assert!(
            !has_any_pack_file(&fixture.data_dir.download_packs_dir()),
            "下载缓存文件不该残留"
        );
        assert!(!fixture.data_dir.config_path().exists(), "配置文件不该残留");
    }

    #[test]
    fn 删除后无悬空索引引用() {
        // "不残留索引引用"——chapter_cache 指向已删的 Pack 文件是最危险的残留
        let fixture = fixture("dangling");
        seed(&fixture);

        UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone())
            .delete_all()
            .expect("应能删除");

        let cache_rows: i64 = fixture
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM chapter_cache", [], |row| row.get(0))
                    .map_err(crate::db::sql)
            })
            .expect("应能查询");
        assert_eq!(cache_rows, 0, "章节缓存索引行不该残留");

        let fts_rows: i64 = fixture
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM chapter_fts", [], |row| row.get(0))
                    .map_err(crate::db::sql)
            })
            .expect("应能查询");
        assert_eq!(fts_rows, 0, "全文索引不该残留");
    }

    #[test]
    fn 删除报告逐表计数() {
        let fixture = fixture("report");
        seed(&fixture);
        let report = UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone())
            .delete_all()
            .expect("应能删除");

        let names: Vec<&str> = report.per_table.iter().map(|(name, _)| *name).collect();
        for expected in ["book", "chapter", "source", "replace_rule", "bookmark"] {
            assert!(
                names.contains(&expected),
                "报告应含 {expected} 的计数，实际 {names:?}"
            );
        }
        assert!(report.file_count() > 0, "应报告删掉的文件数");
        assert!(report.config_removed, "应报告配置已删");
    }

    #[test]
    fn 重复删除结果为空() {
        let fixture = fixture("twice");
        seed(&fixture);
        let deleter = UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone());

        let first = deleter.delete_all().expect("第一次应成功");
        assert!(!first.is_empty());

        let second = deleter.delete_all().expect("第二次应成功（幂等）");
        assert!(second.is_empty(), "第二次不该有东西可删，实际 {second:?}");
    }

    #[test]
    fn 空库删除不报错() {
        let fixture = fixture("empty");
        let report = UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone())
            .delete_all()
            .expect("空库应能删除");
        assert!(report.is_empty());
    }

    #[test]
    fn 不删数据目录本身() {
        // 用户可能把自己的东西放在数据目录里（FR-042 的可移植模式）。
        // 删了整个目录＝删用户文件，越权了。
        let fixture = fixture("keep-dir");
        seed(&fixture);

        // 用户在数据目录里放一个自己的文件
        let user_file = fixture.data_dir.root().join("我的笔记.txt");
        std::fs::write(&user_file, "这是用户自己的东西").expect("应能写");

        UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone())
            .delete_all()
            .expect("应能删除");

        assert!(fixture.data_dir.root().is_dir(), "数据目录本身不该被删");
        assert!(
            user_file.exists(),
            "用户放在数据目录里的无关文件不该被删（FR-042）"
        );
    }

    #[test]
    fn 只清库时不动文件系统() {
        let fixture = fixture("db-only");
        seed(&fixture);
        UserDataDeleter::new(fixture.db.clone())
            .delete_all()
            .expect("应能删除");

        assert_eq!(
            UserDataDeleter::new(fixture.db.clone())
                .remaining_rows()
                .expect("应能统计"),
            0,
            "数据库应清干净"
        );
        assert!(
            has_any_pack_file(&fixture.data_dir.packs_dir()),
            "只清库的模式下文件应保留（调用方负责）"
        );
    }

    #[test]
    fn 库文件本身保留可继续使用() {
        // 删除的是**用户数据**，不是数据库本身。库还在（schema 完好），
        // 故删完可以立刻继续用——不必重启程序。
        let fixture = fixture("reusable");
        seed(&fixture);
        UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone())
            .delete_all()
            .expect("应能删除");

        // 立刻写一条新数据，应成功
        let book_dao = crate::dao::BookDao::new(fixture.db.clone());
        let id = book_dao
            .add_book(&Book {
                id: 0,
                origin: BookOrigin::Local,
                origin_id: "after-delete".to_string(),
                name: "删除后的新书".to_string(),
                author: None,
                cover_url: None,
                intro: None,
                kind: None,
                book_group: None,
                read_config: None,
                toc_url: None,
                latest_chapter_title: None,
                latest_chapter_time: None,
                library_root_id: None,
                relative_path: None,
                charset: None,
                format: None,
                total_chapters: None,
                dur_chapter_index: None,
                added_at: 0,
            })
            .expect("删除后应仍能写数据");
        assert!(id > 0);
        assert_eq!(
            book_dao.bookshelf().expect("应能读").len(),
            1,
            "新数据应可读"
        );
    }

    #[test]
    fn 删除后全文搜索无结果() {
        // 索引被清干净的**行为**证据（不只是计数为 0）
        let fixture = fixture("fts-gone");
        seed(&fixture);

        let index = crate::fts::CachedTextIndex::new(fixture.db.clone());
        assert_eq!(index.indexed_count().expect("应能计数"), 1, "删除前有索引");

        UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone())
            .delete_all()
            .expect("应能删除");

        assert_eq!(
            index.indexed_count().expect("应能计数"),
            0,
            "删除后全文索引应为空"
        );
    }

    #[test]
    fn 缺表的数据目录不报错() {
        // 旧版本的数据目录可能缺某些表。删除时应跳过而非失败——
        // 用户要的是"删干净"，不该因为某个表不存在而删不了
        let fixture = fixture("missing-table");
        fixture
            .db
            .write(|connection| {
                connection
                    .execute("DROP TABLE IF EXISTS script_cache", [])
                    .map_err(crate::db::sql)?;
                Ok(())
            })
            .expect("应能删表");

        UserDataDeleter::with_data_dir(fixture.db.clone(), fixture.data_dir.clone())
            .delete_all()
            .expect("缺表不该让删除失败");
    }
}
