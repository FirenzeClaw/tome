//! [DONE] 图片缓存（tasks.md T061 / FR-019、SC-017）
//!
//! 文件系统 + 索引表：文件在 `cache/images/<xx>/<hash>`，索引在 `image_cache`
//! 表（`url` 主键）。
//!
//! # 为什么 `<xx>` 两级分片
//!
//! 一张封面一张文件，一个书架几百本书 → 几千张图；正文内嵌图片更多。
//! 平铺在一个目录下，`read_dir` 在部分文件系统上会明显变慢，且部分工具
//! （备份、杀软扫描）会卡住。按哈希前 2 位分 256 个子目录把单目录条目数
//! 降到 1/256。
//!
//! # 为什么文件名是哈希而不是 URL
//!
//! URL 里有 `/`、`?`、`&`、`=` 以及任意百分号编码，还有长度限制（部分
//! 文件系统 255 字节）与大小写不敏感（Windows/macOS）。全部转义既容易出错
//! 又可能仍然超长。哈希没有这些问题，而"URL → 哈希"的映射存在索引表里
//! ——**文件名不需要可反解**。
//!
//! ## 用 SHA-256 而不是 MD5
//!
//! 这里不需要密码学强度（没有攻击者），但需要**碰撞极低**：MD5 对
//! 刻意构造的输入有已知碰撞，而 URL 来自书源（第三方可控）。一次碰撞
//! 会让两张不同的图互相覆盖，表现为"封面串了"——那是个很难排查的现象。
//!
//! # 与章节缓存是两个可独立清理的资源（SC-017）
//!
//! 两者在**文件系统上**（`cache/packs` vs `cache/images`）与**索引表上**
//! （`chapter_cache` vs `image_cache`）都完全分离，且各自的清理入口只碰
//! 自己那半边。清理其一不影响其二的可用性——这是结构性的，不是靠约定。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::db::sql;
use crate::error::{from_io, StoreError};

/// URL 对应的缓存文件名（SHA-256 十六进制）。
///
/// 暴露为公开函数：测试要断言"同一 URL 总是同一个文件名"，
/// 而调用方（tome-source 的图片管线）可能需要先探是否存在再决定是否抓取。
pub fn cache_file_name(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    // 手工转十六进制而不是 `format!("{digest:x}")`：`sha2` 0.11 的
    // `digest` 返回泛型 `Array`，它不实现 `LowerHex`（只有固定长度数组
    // 的某些形态实现了）。手写一行避免依赖该 crate 的格式化 trait 面。
    let mut name = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(name, "{byte:02x}");
    }
    name
}

/// 图片缓存。
#[derive(Debug, Clone)]
pub struct ImageCache {
    root: PathBuf,
    db: crate::db::Db,
    now: i64,
}

impl ImageCache {
    /// 以当前时间构造。
    pub fn new(root: &Path, db: crate::db::Db) -> Self {
        Self {
            root: root.to_path_buf(),
            db,
            now: crate::now_unix(),
        }
    }

    /// 以指定时间戳构造（测试用）。
    pub fn with_now(root: &Path, db: crate::db::Db, now: i64) -> Self {
        Self {
            root: root.to_path_buf(),
            db,
            now,
        }
    }

    /// 缓存根目录。
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    /// 某 URL 的缓存文件路径（`<root>/<xx>/<hash>`）。
    pub fn path_for(&self, url: &str) -> PathBuf {
        let name = cache_file_name(url);
        // 前 2 位做分片。`get(..2)` 而不是切片索引：哈希永远是 64 个十六进制
        // 字符，但用 get 让"万一变了"退化为不分子目录而不是 panic。
        let shard = name.get(..2).unwrap_or("00");
        self.root.join(shard).join(name)
    }

    /// 读一张缓存的图片。
    ///
    /// 未缓存返回 `Ok(None)`（未命中是正常状态）。索引行存在但文件不在时
    /// 报 [`StoreError::Corrupt`]——那是"悬空引用"，静默返回 `None` 会让
    /// 上层反复重抓却永远发现不了索引与文件已经不一致。
    pub fn get(&self, url: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let path = self.path_for(url);
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if self.is_indexed(url)? {
                    return Err(StoreError::Corrupt {
                        subject: url.to_string(),
                        reason: format!("索引存在但文件 {} 缺失（悬空引用）", path.display()),
                    });
                }
                Ok(None)
            }
            Err(error) => Err(from_io(&path, error)),
        }
    }

    /// 只查索引（不读文件）——用于快速判断"要不要抓"。
    pub fn is_indexed(&self, url: &str) -> Result<bool, StoreError> {
        let url = url.to_string();
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT 1 FROM image_cache WHERE url=?1")
                .map_err(sql)?;
            let mut rows = statement.query([&url]).map_err(sql)?;
            Ok(rows.next().map_err(sql)?.is_some())
        })
    }

    /// 写入一张图片：先落盘（fsync）→ 再提交索引。
    ///
    /// # 顺序与章节缓存同源
    ///
    /// 反过来的话，索引指向不存在的文件 → [`Self::get`] 报悬空引用。
    /// 虽然图片缓存损坏的后果比章节缓存轻（重抓即可），但**同一条不变量
    /// 就该在所有缓存上一致**——两套不同的顺序约定迟早会被写错一处。
    pub fn put(&self, url: &str, bytes: &[u8], source_id: Option<&str>) -> Result<(), StoreError> {
        let path = self.path_for(url);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| from_io(parent, error))?;
        }

        // ① 落盘 + fsync
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&path).map_err(|error| from_io(&path, error))?;
            file.write_all(bytes)
                .map_err(|error| from_io(&path, error))?;
            file.sync_data().map_err(|error| from_io(&path, error))?;
        }

        // ② 索引
        let relative = self
            .relative_path(&path)
            .unwrap_or_else(|| path.display().to_string());
        let url = url.to_string();
        let source_id = source_id.map(str::to_string);
        let now = self.now;
        self.db.write(move |connection| {
            connection
                .execute(
                    "INSERT INTO image_cache(url, source_id, path, fetched_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(url) DO UPDATE SET
                        source_id=excluded.source_id,
                        path=excluded.path,
                        fetched_at=excluded.fetched_at",
                    rusqlite::params![url, source_id, relative, now],
                )
                .map_err(sql)?;
            Ok(())
        })
    }

    /// 删一张图片（文件 + 索引）。
    pub fn remove(&self, url: &str) -> Result<(), StoreError> {
        let url_owned = url.to_string();
        let affected = self.db.write(move |connection| {
            connection
                .execute("DELETE FROM image_cache WHERE url=?1", [&url_owned])
                .map_err(sql)
        })?;
        let path = self.path_for(url);
        let _ = std::fs::remove_file(&path);
        if affected == 0 {
            return Err(StoreError::not_found(url.to_string()));
        }
        Ok(())
    }

    /// 清空图片缓存（SC-017：与章节缓存各自可清）。
    ///
    /// # 只碰图片
    ///
    /// 本方法只删 `image_cache` 行与 `cache/images/` 目录。章节缓存
    /// （`cache/packs` + `chapter_cache`）与全部个人数据一张表都不碰。
    pub fn clear(&self) -> Result<ClearReport, StoreError> {
        let freed = self.disk_bytes()?;
        let removed = self.db.write(|connection| {
            connection
                .execute("DELETE FROM image_cache", [])
                .map_err(sql)
        })?;

        if self.root.is_dir() {
            std::fs::remove_dir_all(&self.root).map_err(|error| from_io(&self.root, error))?;
        }
        Ok(ClearReport {
            removed_entries: removed,
            freed_bytes: freed,
        })
    }

    /// 按容量上限淘汰（最旧的先走，按 `fetched_at`）。
    ///
    /// 返回淘汰条数与释放字节数。
    pub fn evict_to(&self, target_bytes: u64) -> Result<ClearReport, StoreError> {
        let mut report = ClearReport::default();
        let max_rounds = self.entry_count()? as usize + 1;
        for _ in 0..max_rounds {
            if self.disk_bytes()? <= target_bytes {
                break;
            }
            let Some(oldest) = self.oldest_url()? else {
                break;
            };
            let path = self.path_for(&oldest);
            let file_bytes = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let url = oldest.clone();
            let removed = self.db.write(move |connection| {
                connection
                    .execute("DELETE FROM image_cache WHERE url=?1", [&url])
                    .map_err(sql)
            })?;
            if removed == 0 {
                break;
            }
            let _ = std::fs::remove_file(&path);
            report.removed_entries += removed;
            report.freed_bytes += file_bytes;
        }
        Ok(report)
    }

    /// 图片缓存条目数（索引行数）。
    pub fn entry_count(&self) -> Result<i64, StoreError> {
        self.db.read(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM image_cache", [], |row| row.get(0))
                .map_err(sql)
        })
    }

    /// 清掉**索引里没有**的图片文件（孤儿回收）。
    ///
    /// 返回删除的文件数。与 [`crate::pack::PackStore::evict_orphan_packs`]
    /// 同理：崩溃会在磁盘上留下没有索引指向的文件。
    pub fn evict_orphan_files(&self) -> Result<usize, StoreError> {
        let indexed: std::collections::HashSet<String> = self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT url FROM image_cache")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(sql)?;
            let mut urls = std::collections::HashSet::new();
            for row in rows {
                urls.insert(cache_file_name(&row.map_err(sql)?));
            }
            Ok(urls)
        })?;

        let mut removed = 0usize;
        let Ok(shards) = std::fs::read_dir(&self.root) else {
            return Ok(0);
        };
        for shard in shards.flatten() {
            let Ok(files) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                if indexed.contains(&name) {
                    continue;
                }
                if std::fs::remove_file(file.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        Ok(removed)
    }

    /// 磁盘上图片文件的实际总字节数。
    pub fn disk_bytes(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;
        let Ok(shards) = std::fs::read_dir(&self.root) else {
            return Ok(0);
        };
        for shard in shards.flatten() {
            let Ok(files) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                if let Ok(metadata) = file.metadata() {
                    if metadata.is_file() {
                        total += metadata.len();
                    }
                }
            }
        }
        Ok(total)
    }

    /// 最久未取的 URL。
    fn oldest_url(&self) -> Result<Option<String>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT url FROM image_cache ORDER BY fetched_at, url LIMIT 1")
                .map_err(sql)?;
            let mut rows = statement.query([]).map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(row.get(0).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// 文件相对缓存根的路径（索引里存相对路径，便于数据目录整体搬移）。
    ///
    /// FR-042 要求数据目录可随程序整体移动——索引里若存**绝对**路径，
    /// 搬移后全部失效。存相对路径则搬移天然有效。
    fn relative_path(&self, path: &Path) -> Option<String> {
        path.strip_prefix(&self.root)
            .ok()
            .map(|relative| relative.to_string_lossy().replace('\\', "/"))
    }
}

/// 清空/淘汰的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClearReport {
    /// 移除的索引条目数
    pub removed_entries: usize,
    /// 释放的字节数
    pub freed_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: PathBuf,
        db: crate::db::Db,
        cache: ImageCache,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-image-cache-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");
        Fixture {
            cache: ImageCache::with_now(
                &dir.join("cache").join("images"),
                db.clone(),
                1_700_000_000,
            ),
            dir,
            db,
        }
    }

    /// 一段最小的合法 PNG（1×1 透明像素）。
    fn tiny_png() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG 签名
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR 长度 + 类型
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
            0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, // 位深等 + CRC
            0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, // IDAT
            0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D,
            0xB4, // CRC
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn 写入后能读回() {
        let fixture = fixture("roundtrip");
        fixture
            .cache
            .put("https://x/cover.png", &tiny_png(), Some("https://a.com"))
            .expect("应能写");
        assert_eq!(
            fixture.cache.get("https://x/cover.png").expect("应能读"),
            Some(tiny_png())
        );
    }

    #[test]
    fn 未缓存返回_none() {
        let fixture = fixture("miss");
        assert_eq!(
            fixture.cache.get("https://x/none.png").expect("应成功"),
            None
        );
    }

    #[test]
    fn 索引存在但文件缺失时报悬空引用() {
        let fixture = fixture("dangling");
        fixture
            .cache
            .put("https://x/a.png", &tiny_png(), None)
            .expect("应能写");
        // 删掉文件，保留索引
        let path = fixture.cache.path_for("https://x/a.png");
        std::fs::remove_file(&path).expect("应能删");

        let error = fixture.cache.get("https://x/a.png").expect_err("应报损坏");
        assert!(
            matches!(error, StoreError::Corrupt { .. }),
            "悬空引用应报错而不是伪装成未命中：{error:?}"
        );
    }

    #[test]
    fn 同一_url_总是同一路径() {
        let fixture = fixture("stable-path");
        let first = fixture.cache.path_for("https://x/a.png");
        let second = fixture.cache.path_for("https://x/a.png");
        assert_eq!(first, second);
        assert_ne!(
            first,
            fixture.cache.path_for("https://x/b.png"),
            "不同 URL 应落在不同文件"
        );
    }

    #[test]
    fn 文件名不含_url_里的特殊字符() {
        // URL 里的 / ? & = 与百分号编码在文件名里全是麻烦，且可能超长
        let url = "https://x/a/b/../c.png?token=a&b=%20%2F&filename=很长的中文名字.png";
        let name = cache_file_name(url);
        assert_eq!(name.len(), 64, "应是 SHA-256 的十六进制");
        assert!(
            name.chars().all(|character| character.is_ascii_hexdigit()),
            "文件名应只含十六进制字符：{name}"
        );
        assert!(!name.contains('/') && !name.contains('?') && !name.contains('&'));
    }

    #[test]
    fn 按哈希前两位分片() {
        let fixture = fixture("shard");
        fixture
            .cache
            .put("https://x/a.png", &tiny_png(), None)
            .expect("应能写");
        let path = fixture.cache.path_for("https://x/a.png");
        let shard = path
            .parent()
            .expect("应有父目录")
            .file_name()
            .expect("应有目录名")
            .to_string_lossy()
            .into_owned();
        let name = cache_file_name("https://x/a.png");
        assert_eq!(shard, name[..2], "子目录应是哈希前两位");
    }

    #[test]
    fn 覆盖写() {
        let fixture = fixture("overwrite");
        fixture
            .cache
            .put("https://x/a.png", b"old", None)
            .expect("应能写");
        fixture
            .cache
            .put("https://x/a.png", b"new", None)
            .expect("应能写");
        assert_eq!(
            fixture.cache.get("https://x/a.png").expect("应能读"),
            Some(b"new".to_vec())
        );
        assert_eq!(
            fixture.cache.entry_count().expect("应能数"),
            1,
            "覆盖不该产生新条目"
        );
    }

    #[test]
    fn 索引存相对路径_数据目录可整体搬移() {
        // FR-042：数据目录可置于程序同级并随程序整体移动。
        // 索引里若存绝对路径，搬移后全部失效。
        let fixture = fixture("relative");
        fixture
            .cache
            .put("https://x/a.png", &tiny_png(), None)
            .expect("应能写");
        let stored: String = fixture
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT path FROM image_cache LIMIT 1", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查");
        assert!(
            !Path::new(&stored).is_absolute(),
            "索引里应存相对路径（实际 `{stored}`）"
        );
        assert!(stored.contains('/'), "应用正斜杠，跨平台可读");
    }

    #[test]
    fn 删除单个图片() {
        let fixture = fixture("remove");
        fixture
            .cache
            .put("https://x/a.png", &tiny_png(), None)
            .expect("应能写");
        fixture.cache.remove("https://x/a.png").expect("应能删");
        assert_eq!(fixture.cache.get("https://x/a.png").expect("应成功"), None);
        assert!(
            !fixture.cache.path_for("https://x/a.png").exists(),
            "文件应一并删除"
        );

        let error = fixture
            .cache
            .remove("https://x/a.png")
            .expect_err("重复删应报错");
        assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn 清图片缓存不伤章节缓存与个人数据() {
        // SC-017：章节缓存与图片缓存是两个可独立清理的资源
        let fixture = fixture("independent");
        fixture
            .cache
            .put("https://x/a.png", &tiny_png(), None)
            .expect("应能写");

        // 造一份章节缓存（走真实的 PackStore）
        let pack_root = fixture.dir.join("cache").join("packs");
        let pack_store = crate::pack::PackStore::new(&pack_root, fixture.db.clone());
        let (book_id, chapter_id) = fixture
            .db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at) VALUES (0, 'x', '书', 0)",
                        [],
                    )
                    .map_err(sql)?;
                let book_id = connection.last_insert_rowid();
                connection
                    .execute(
                        "INSERT INTO chapter(book_id, index_in_book, title) VALUES (?1, 0, '章')",
                        [book_id],
                    )
                    .map_err(sql)?;
                Ok((book_id, connection.last_insert_rowid()))
            })
            .expect("应能建书与章");
        pack_store
            .write_chapter(chapter_id, "正文内容".as_bytes())
            .expect("应能写章节缓存");

        // 造个人数据
        fixture
            .db
            .write(move |connection| {
                connection
                    .execute(
                        "INSERT INTO read_progress(book_id, chapter_index, char_offset, updated_at)
                         VALUES (?1, 0, 0, 0)",
                        [book_id],
                    )
                    .map_err(sql)
            })
            .expect("应能写进度");

        // 清图片缓存
        let report = fixture.cache.clear().expect("应能清");
        assert_eq!(report.removed_entries, 1);

        // ① 图片缓存空了
        assert_eq!(fixture.cache.get("https://x/a.png").expect("应成功"), None);
        assert_eq!(fixture.cache.entry_count().expect("应能数"), 0);
        // ② 章节缓存**完好**
        assert_eq!(
            pack_store.read_chapter(chapter_id).expect("应能读"),
            Some("正文内容".as_bytes().to_vec()),
            "清图片缓存不得影响章节缓存（SC-017）"
        );
        assert_eq!(pack_store.entry_count().expect("应能数"), 1);
        // ③ 个人数据**完好**
        let progress: i64 = fixture
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM read_progress", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查");
        assert_eq!(progress, 1, "清图片缓存不得影响个人数据（SC-015）");
    }

    #[test]
    fn 按容量淘汰最旧的() {
        let fixture = fixture("evict");
        for index in 0..10 {
            let mut image = tiny_png();
            image.extend_from_slice(&vec![0x41; 1000]);
            fixture
                .cache
                .put(&format!("https://x/{index}.png"), &image, None)
                .expect("应能写");
            // 让 fetched_at 递增，明确"谁是最旧的"
            fixture
                .db
                .write(move |connection| {
                    connection
                        .execute(
                            "UPDATE image_cache SET fetched_at = ?1 WHERE url = ?2",
                            rusqlite::params![
                                1_700_000_000i64 + index,
                                format!("https://x/{index}.png")
                            ],
                        )
                        .map_err(sql)
                })
                .expect("应能改");
        }
        assert_eq!(fixture.cache.entry_count().expect("应能数"), 10);

        let report = fixture.cache.evict_to(3000).expect("应能淘汰");
        assert!(report.removed_entries > 0, "应有条目被淘汰");
        assert!(
            fixture
                .cache
                .get("https://x/0.png")
                .expect("应成功")
                .is_none(),
            "最旧的应先被淘汰"
        );
        assert!(
            fixture
                .cache
                .get("https://x/9.png")
                .expect("应成功")
                .is_some(),
            "最新的应留下"
        );
    }

    #[test]
    fn 孤儿文件可回收() {
        let fixture = fixture("orphan");
        // 一个有文件没索引的条目
        let path = fixture.cache.root().join("ab").join("deadbeef");
        std::fs::create_dir_all(path.parent().expect("应有父目录")).expect("应能建");
        std::fs::write(&path, b"orphan").expect("应能写");
        assert_eq!(fixture.cache.disk_bytes().expect("应能算"), 6);

        assert_eq!(fixture.cache.evict_orphan_files().expect("应能回收"), 1);
        assert_eq!(fixture.cache.disk_bytes().expect("应能算"), 0);
    }

    #[test]
    fn 孤儿回收不误删有效文件() {
        let fixture = fixture("no-false-positive");
        fixture
            .cache
            .put("https://x/a.png", &tiny_png(), None)
            .expect("应能写");
        assert_eq!(fixture.cache.evict_orphan_files().expect("应能回收"), 0);
        assert_eq!(
            fixture.cache.get("https://x/a.png").expect("应能读"),
            Some(tiny_png()),
            "有效文件不得被误删"
        );
    }

    #[test]
    fn 记录来源书源() {
        let fixture = fixture("source");
        fixture
            .cache
            .put("https://x/a.png", &tiny_png(), Some("https://a.com"))
            .expect("应能写");
        let source: Option<String> = fixture
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT source_id FROM image_cache LIMIT 1", [], |row| {
                        get_opt(row, 0)
                    })
                    .map_err(sql)
            })
            .expect("应能查");
        assert_eq!(source.as_deref(), Some("https://a.com"));
    }

    /// 读一个可空文本列（`query_row` 的闭包不能直接 `.map_err(sql)`，
    /// 故单独抽出来）。
    fn get_opt(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<String>> {
        row.get(index)
    }
}
