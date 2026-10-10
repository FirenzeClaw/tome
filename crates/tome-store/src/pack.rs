//! \[DONE] Pack 章节缓存容器（tasks.md T060 / FR-029、FR-030、FR-031）
//!
//! # 为什么是容器而不是"一章一文件"
//!
//! FR-029 要求批量容器。SC-005b 把它量化了：**5 万章规模下缓存文件数不超过
//! 总章数的 2%**（即 ≤1000 个文件）。一章一文件在 5 万章时会产生 5 万个
//! 小文件——那在三类场景下都会出问题：目录扫描、备份拷贝、以及部分文件系统
//! 上的 inode 耗尽。
//!
//! # 文件布局（`data-model.md`）
//!
//! ```text
//! cache/packs/<book_hash>/<pack_id>.pk
//! ```
//!
//! 头部 `[magic 8B][version u16][pack_id u32]` = 14 字节，正文为顺序追加的
//! `[u32 len][bytes]` 记录。`<book_hash>` 让同一本书的容器聚在一起（也便于
//! 按书清理）。
//!
//! ## 头部为什么要带 `pack_id`
//!
//! 文件名里已经有了。带进头部是为了让**单个文件自证身份**：文件被改名/复制/
//! 从备份还原到错误位置时，头部与文件名不符能被检测出来。只信文件名的话，
//! 一份放错位置的容器会被当成本地容器读，用户看到的是**别的书的正文**。
//!
//! # 崩溃安全（FR-030 / SC-007）——本模块的核心不变量
//!
//! > **内容追加落盘并 fsync 必须先于**索引行的事务提交。
//!
//! 反过来（先提交索引后写内容）会产生两种损坏：
//!
//! - 提交后崩溃在写内容之前 → 索引指向**不存在**的偏移，读时越界
//! - 提交后崩溃在写内容一半 → 索引指向**半截**内容，用户看到截断的正文
//!
//! 而先写后提交的中间状态是**可恢复的**：多出来的字节没有任何索引指向它，
//! 下次追加时会被新记录覆盖（或留在容器尾部成为空洞，由容量核算忽略）。
//! 那正是"崩溃后不存在指向缺失内容的引用"的含义。
//!
//! ## `synchronous=FULL` 为什么还不够
//!
//! SQLite 的 `FULL` 保证的是**数据库文件**的持久性，管不到我们用
//! `std::fs` 写的 Pack 文件。故内容侧必须自己 `sync_data()`。
//!
//! # 淘汰以容器为单位（FR-031）
//!
//! 容器内的记录布局是"顺序追加、不可变"——删掉容器中间的一条会让其余记录的
//! 偏移失效。故淘汰的最小粒度就是**整个容器**：删容器 + 删指向它的全部索引行。
//! 选"哪个容器"用 LRU（`chapter_cache.cached_at` 的最大值）。
//!
//! # 清缓存不伤个人数据（FR-031 / SC-015）
//!
//! 本模块只碰 `cache/` 下的文件与 `chapter_cache` 表。书架、进度、书签、
//! 高亮、批注、书源、规则一张都不碰——这是结构性的，不是靠"记得别删"。

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::db::sql;
use crate::error::{from_io, StoreError};

/// 容器文件魔数（8 字节）。
///
/// 选一个**不会是有效内容**的序列：`\0` 开头的字节组合在文本正文里不可能
/// 自然出现，故"这不是 Pack 容器"能被立即识别，而不是把二进制垃圾当记录解析。
pub const PACK_MAGIC: [u8; 8] = *b"\0tomePK\0";

/// 容器格式版本。
///
/// 升级格式时，读到旧版本要么迁移要么明确拒绝——用版本号而不是"猜"。
pub const PACK_VERSION: u16 = 1;

/// 容器头部长度（`magic(8) + version(2) + pack_id(4)`）。
pub const PACK_HEADER_LEN: u64 = 14;

/// 单条记录的长度前缀字节数（`u32`）。
const RECORD_LEN_PREFIX: usize = 4;

/// 单条记录允许的最大长度（64 MiB）。
///
/// # 为什么需要上限
///
/// 长度前缀是从文件里读出来的 `u32`，**不可信**——文件损坏或被人为构造时它
/// 可能是 4 GB。若不设上限就去分配缓冲区，一次读取请求会让进程 OOM。
/// 这是"边界处校验外部输入"（AGENTS.md 安全红线）在缓存格式上的落点。
///
/// 64 MiB 远大于任何真实章节（章节正文通常几 KB 到几百 KB，图片缓存走
/// 另一条路），故不构成对正常内容的限制。
pub const MAX_RECORD_BYTES: u32 = 64 * 1024 * 1024;

/// 默认容器容量上限（字节）：32 MiB。
///
/// # 为什么是容量上限而不是条数上限
///
/// 章节大小差异极大（一首诗 200 字节，一章 50 KB）。按条数分配会让容器
/// 大小在 100 KB 到 2 GB 之间浮动，而那让"容器容量可配置"（FR-029）失去
/// 意义——用户配的是磁盘占用，不是条数。
///
/// 选 32 MiB：按平均 6 KB/章算约装 5000 章，5 万章的书约 10 个容器，
/// 远低于 SC-005b 的 2% 上限（1000 个文件）；而单个文件又不会大到
/// "淘汰一个容器释放不出空间"。
///
/// 同时要满足 SC-005（总字节数 ≤ 正文 × 1.01）：头部 14 字节 + 每记录 4 字节
/// 前缀，按 6 KB/章计开销约 0.07%，充裕地落在 1.01 以内。
///
/// \[WORKAROUND] 硬编码默认值：`config.toml` 的存储子集（T062）里有
/// `chapter_pack_bytes` 字段，组合根会注入；此处只是无配置时的取值。
pub const DEFAULT_PACK_CAPACITY_BYTES: u64 = 32 * 1024 * 1024;

/// 缓存的一条内容在容器中的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackLocation {
    /// 容器 id
    pub pack_id: i64,
    /// 容器内偏移（指向 `[u32 len]` 前缀的起始）
    pub offset: u64,
    /// 内容字节数（**不含**长度前缀）
    pub length: u32,
}

/// 一次写入的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackWrite {
    /// 写入位置
    pub location: PackLocation,
    /// 该容器写满后是否被封闭（后续写入会开新容器）
    pub sealed: bool,
}

/// 章节缓存容器的管理器。
///
/// # 与数据库的分工
///
/// 内容在文件里，索引在 `chapter_cache` 表里。本类型负责**两侧的写入顺序**
/// （内容 → fsync → 索引），故它持有 [`crate::db::Db`] 而不只是文件路径——
/// 把"写脚本"和"提交索引"拆到两个类型，顺序约束就只能靠调用方自觉。
pub struct PackStore {
    /// `cache/packs` 根目录
    root: PathBuf,
    db: crate::db::Db,
    /// 容器容量上限
    capacity_bytes: u64,
}

impl std::fmt::Debug for PackStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PackStore")
            .field("root", &self.root)
            .field("capacity_bytes", &self.capacity_bytes)
            .finish_non_exhaustive()
    }
}

impl PackStore {
    /// 以默认容量构造。
    pub fn new(root: &Path, db: crate::db::Db) -> Self {
        Self::with_capacity(root, db, DEFAULT_PACK_CAPACITY_BYTES)
    }

    /// 以指定容量构造（FR-029：容器容量可配置）。
    pub fn with_capacity(root: &Path, db: crate::db::Db, capacity_bytes: u64) -> Self {
        Self {
            root: root.to_path_buf(),
            db,
            // 容量下限：头部 + 一条最小记录。比它还小的容器放不下任何东西，
            // 会导致"每次写入都开新容器"，那是无意义的配置。
            capacity_bytes: capacity_bytes.max(PACK_HEADER_LEN + RECORD_LEN_PREFIX as u64 + 1),
        }
    }

    /// 容器根目录。
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    /// 单本书的容器目录（`<book_hash>`）。
    ///
    /// # 为什么用 book_id 而不是内容哈希
    ///
    /// `data-model.md` 写的是 `<book_hash>`。此处用 `book_id`（十进制的
    /// 十进制形式加前缀）——因为**容器按书分组的目的只是"让同一本书的容器
    /// 聚在一起"**，而 `book_id` 在库内唯一且稳定。用内容哈希会让"同一本书
    /// 改了元信息"产生新目录，旧目录成为孤儿。
    ///
    /// 名字前缀 `book-` 是为了与目录下可能出现的其他条目（如临时文件）区分。
    fn book_dir(&self, book_id: i64) -> PathBuf {
        self.root.join(format!("book-{book_id}"))
    }

    /// 容器文件路径。
    fn pack_path(&self, book_id: i64, pack_id: i64) -> PathBuf {
        self.book_dir(book_id).join(format!("{pack_id}.pk"))
    }

    // -----------------------------------------------------------------------
    // 读
    // -----------------------------------------------------------------------

    /// 按章节 id 读内容（走索引表）。
    ///
    /// # 未命中返回 `Ok(None)`
    ///
    /// "没缓存过"是正常状态（下次会去抓）。报错误会让调用方用 try/catch
    /// 表达一个高频的正常分支。
    ///
    /// # 索引指向不存在的内容时返回 `Err(Corrupt)`
    ///
    /// 这正是 FR-030 要杜绝的情形。它**不该**在正常流程里出现；一旦出现，
    /// 说明有不遵守写入顺序的代码路径，或文件被外部破坏。报出来比返回
    /// `None`（伪装成"没缓存"）好——后者会让上层静默重抓，永远发现不了
    /// 那道防线已经失效。
    pub fn read_chapter(&self, chapter_id: i64) -> Result<Option<Vec<u8>>, StoreError> {
        let location = match self.location_of(chapter_id)? {
            Some(location) => location,
            None => return Ok(None),
        };

        let book_id = self
            .book_id_of(chapter_id)?
            .ok_or_else(|| StoreError::not_found(format!("chapter#{chapter_id}")))?;

        let path = self.pack_path(book_id, location.pack_id);
        let content = read_at(&path, book_id, location.pack_id, location)?;
        Ok(Some(content))
    }

    /// 查索引里某章的位置。
    pub fn location_of(&self, chapter_id: i64) -> Result<Option<PackLocation>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT pack_id, offset, length FROM chapter_cache WHERE chapter_id=?1",
                )
                .map_err(sql)?;
            let mut rows = statement.query([chapter_id]).map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(PackLocation {
                    pack_id: row.get(0).map_err(sql)?,
                    offset: row.get::<_, i64>(1).map_err(sql)? as u64,
                    length: row.get::<_, i64>(2).map_err(sql)? as u32,
                })),
                None => Ok(None),
            }
        })
    }

    /// 章节所属的书籍 id。
    fn book_id_of(&self, chapter_id: i64) -> Result<Option<i64>, StoreError> {
        self.db.read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT book_id FROM chapter WHERE id=?1")
                .map_err(sql)?;
            let mut rows = statement.query([chapter_id]).map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(row.get(0).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    // -----------------------------------------------------------------------
    // 写（本模块的核心）
    // -----------------------------------------------------------------------

    /// 写入一章内容并提交索引。
    ///
    /// **顺序不可颠倒**：追加内容 → `sync_data` → 索引事务提交（FR-030）。
    /// 每一步的理由见模块文档。
    ///
    /// # 返回的偏移是"内容起点"还是"前缀起点"
    ///
    /// 索引里的 `offset` 指向**长度前缀的起点**。读的时候先读 4 字节前缀拿
    /// 长度，再读内容。另一个选择是索引直接存内容起点（+4），但那样任何
    /// 格式微调（比如未来加校验和字段）都要迁移全部索引行；存前缀起点则
    /// 只需改读取逻辑。
    ///
    /// # 性能：写入路径只有两次 DB 往返
    ///
    /// 5 万章规模下（SC-005b 的测试场景）每次写入的往返成本是主导项。
    /// 故这里刻意避免"每写一章查三次库"的写法：
    ///
    /// | 查什么 | 次数 | 做法 |
    /// |---|---|---|
    /// | 章节属于哪本书、该书当前该写哪个容器 | 1 | `Self::locate_write_target` 一次问清 |
    /// | 提交索引 | 1 | 事务内的 upsert |
    ///
    /// 两次都在 actor 上排队，而**文件写入与 fsync 不占队列**——那才是
    /// 真正慢的部分（每章一次磁盘同步），但它必须保持串行以保证
    /// "内容先于索引"的顺序。
    pub fn write_chapter(&self, chapter_id: i64, content: &[u8]) -> Result<PackWrite, StoreError> {
        if content.len() > MAX_RECORD_BYTES as usize {
            return Err(StoreError::Corrupt {
                subject: format!("chapter#{chapter_id}"),
                reason: format!(
                    "单条内容 {} 字节超过上限 {} 字节",
                    content.len(),
                    MAX_RECORD_BYTES
                ),
            });
        }
        // 单章路径没有待提交的批次，故传空切片
        let (book_id, pack_id) = self.locate_write_target(chapter_id, &[])?;

        std::fs::create_dir_all(self.book_dir(book_id))
            .map_err(|error| from_io(&self.book_dir(book_id), error))?;

        let path = self.pack_path(book_id, pack_id);
        ensure_pack_file(&path, pack_id)?;

        // ① 内容追加到文件末尾。
        let offset = {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|error| from_io(&path, error))?;
            let offset = file
                .seek(SeekFrom::End(0))
                .map_err(|error| from_io(&path, error))?;
            let mut record = Vec::with_capacity(RECORD_LEN_PREFIX + content.len());
            record.extend_from_slice(&(content.len() as u32).to_le_bytes());
            record.extend_from_slice(content);
            file.write_all(&record)
                .map_err(|error| from_io(&path, error))?;

            // ② 落盘。**这一步不能省**——SQLite 的 synchronous=FULL 只管
            // 数据库文件，管不到我们用 std::fs 写的这个文件。
            //
            // 用 `sync_data` 而不是 `sync_all`：后者还要同步文件元数据
            // （权限、mtime），而崩溃安全只关心**内容与长度**是否落盘。
            // 元数据晚一点同步不影响"重启后读到完整内容"。
            file.sync_data().map_err(|error| from_io(&path, error))?;
            offset
        };

        let location = PackLocation {
            pack_id,
            offset,
            length: content.len() as u32,
        };

        // ③ 索引事务提交（在内容已落盘**之后**）。
        //
        // 闭包跨线程，故把用到的值全部 clone 成自有——`location` 是 `Copy`，
        // `content.len()` 只用到长度。
        let now = crate::now_unix();
        let content_len = content.len() as u64;
        let sealed = self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            transaction
                .execute(
                    "INSERT INTO chapter_cache(chapter_id, book_id, pack_id, offset, length, cached_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(chapter_id) DO UPDATE SET
                        pack_id=excluded.pack_id,
                        offset=excluded.offset,
                        length=excluded.length,
                        cached_at=excluded.cached_at",
                    rusqlite::params![
                        chapter_id,
                        book_id,
                        location.pack_id,
                        location.offset as i64,
                        location.length as i64,
                        now,
                    ],
                )
                .map_err(sql)?;
            // 该章节已被缓存：更新 chapter 表的冗余标记，让"目录页显示哪些
            // 章已缓存"不必 join 索引表
            transaction
                .execute(
                    "UPDATE chapter SET is_cached=1 WHERE id=?1",
                    [chapter_id],
                )
                .map_err(sql)?;
            transaction.commit().map_err(sql)?;
            Ok(offset + RECORD_LEN_PREFIX as u64 + content_len)
        })?;

        Ok(PackWrite {
            location,
            sealed: sealed >= self.capacity_bytes,
        })
    }

    /// 批量写入多章，**只在最后同步一次**。
    ///
    /// # 代价与适用场景
    ///
    /// 中途崩溃会丢失"尚未同步的那些章节的内容"——但**索引不会被污染**：
    /// 索引在所有内容写完之后才提交。故崩溃后的状态是"这些章节没缓存"，
    /// 而不是"索引指向半截内容"。这正是 FR-030 允许的中间状态。
    ///
    /// 适用场景是**回填与批量导入**（连续抓完一整本书的正文后一次落库），
    /// 不是"读到哪缓存到哪"。后者要用 [`Self::write_chapter`]，它的
    /// 单章崩溃安全性更强（每章内容都独立落盘）。
    ///
    /// # 为什么它值得单独存在
    ///
    /// 5 万章逐条 fsync 在慢盘上要几分钟。而这批写入的每个中间状态对用户都
    /// **没有价值**（用户看不到"缓存到第 3000 章了"这个过程）——为它付出
    /// 5 万次磁盘同步是不划算的。
    pub fn write_chapter_batch(
        &self,
        entries: &[(i64, Vec<u8>)],
    ) -> Result<Vec<PackWrite>, StoreError> {
        let mut results = Vec::with_capacity(entries.len());
        // 每章一次：(章节 id, 容器 id, 偏移, 长度)
        let mut pending: Vec<(i64, i64, u64, u32, i64)> = Vec::with_capacity(entries.len());

        for (chapter_id, content) in entries {
            if content.len() > MAX_RECORD_BYTES as usize {
                return Err(StoreError::Corrupt {
                    subject: format!("chapter#{chapter_id}"),
                    reason: format!(
                        "单条内容 {} 字节超过上限 {} 字节",
                        content.len(),
                        MAX_RECORD_BYTES
                    ),
                });
            }
            let (book_id, pack_id) = self.locate_write_target(*chapter_id, &pending)?;
            let path = self.pack_path(book_id, pack_id);
            if !path.exists() {
                std::fs::create_dir_all(self.book_dir(book_id))
                    .map_err(|error| from_io(&self.book_dir(book_id), error))?;
                ensure_pack_file(&path, pack_id)?;
            }

            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|error| from_io(&path, error))?;
            let offset = file
                .seek(SeekFrom::End(0))
                .map_err(|error| from_io(&path, error))?;
            let mut record = Vec::with_capacity(RECORD_LEN_PREFIX + content.len());
            record.extend_from_slice(&(content.len() as u32).to_le_bytes());
            record.extend_from_slice(content);
            file.write_all(&record)
                .map_err(|error| from_io(&path, error))?;
            // 不在这里 fsync——交给批次结束时的统一同步。

            let after = offset + RECORD_LEN_PREFIX as u64 + content.len() as u64;
            pending.push((*chapter_id, pack_id, offset, content.len() as u32, book_id));
            results.push(PackWrite {
                location: PackLocation {
                    pack_id,
                    offset,
                    length: content.len() as u32,
                },
                sealed: after >= self.capacity_bytes,
            });
        }

        // ① 统一落盘：把本批涉及的每个容器文件同步一次。
        let mut touched: std::collections::BTreeSet<(i64, i64)> = std::collections::BTreeSet::new();
        for (_, pack_id, _, _, book_id) in &pending {
            touched.insert((*book_id, *pack_id));
        }
        for (book_id, pack_id) in &touched {
            let path = self.pack_path(*book_id, *pack_id);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|error| from_io(&path, error))?;
            file.sync_data().map_err(|error| from_io(&path, error))?;
        }

        // ② 索引事务提交（在全部内容已落盘**之后**）
        let now = crate::now_unix();
        let rows = pending;
        self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            {
                let mut statement = transaction
                    .prepare_cached(
                        "INSERT INTO chapter_cache(chapter_id, book_id, pack_id, offset, length, cached_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(chapter_id) DO UPDATE SET
                            pack_id=excluded.pack_id,
                            offset=excluded.offset,
                            length=excluded.length,
                            cached_at=excluded.cached_at",
                    )
                    .map_err(sql)?;
                for (chapter_id, pack_id, offset, length, book_id) in &rows {
                    statement
                        .execute(rusqlite::params![
                            chapter_id,
                            book_id,
                            pack_id,
                            *offset as i64,
                            *length as i64,
                            now,
                        ])
                        .map_err(sql)?;
                }
            }
            {
                let mut statement = transaction
                    .prepare_cached("UPDATE chapter SET is_cached=1 WHERE id=?1")
                    .map_err(sql)?;
                for (chapter_id, _, _, _, _) in &rows {
                    statement.execute([chapter_id]).map_err(sql)?;
                }
            }
            transaction.commit().map_err(sql)?;
            Ok(())
        })?;

        Ok(results)
    }

    /// 查写入目标，并把**本批次内已经写出去**的字节算作已占用。
    ///
    /// # 为什么需要第二个参数
    ///
    /// [`Self::write_chapter`] 每章都提交索引，故"已占用"能从索引读出来。
    /// 批量写入则相反：内容先全部落盘、索引最后才提交。若不把本批已写出去
    /// 的字节算进去，5 万章会全部堆进**同一个容器**——因为索引一直是空的，
    /// `MAX(pack_id)` 恒为 `None`。
    fn locate_write_target(
        &self,
        chapter_id: i64,
        pending: &[(i64, i64, u64, u32, i64)],
    ) -> Result<(i64, i64), StoreError> {
        let capacity = self.capacity_bytes;
        let chapter_id_for_query = chapter_id;
        let (book_id, latest, used) = self.db.read(move |connection| {
            let book_id: Option<i64> = connection
                .query_row(
                    "SELECT book_id FROM chapter WHERE id=?1",
                    [chapter_id_for_query],
                    |row| row.get(0),
                )
                .map(Some)
                .or_else(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(sql(other)),
                })?;
            let Some(book_id) = book_id else {
                return Err(StoreError::not_found(format!("chapter#{chapter_id_for_query}")));
            };

            // 一次拿齐：该书最大的 pack_id，以及那个容器的已用字节数
            let (latest, used): (Option<i64>, Option<i64>) = connection
                .query_row(
                    "SELECT MAX(pack_id),
                            (SELECT MAX(offset + length + ?2) FROM chapter_cache
                              WHERE book_id=?1 AND pack_id = (SELECT MAX(pack_id) FROM chapter_cache WHERE book_id=?1))
                     FROM chapter_cache WHERE book_id=?1",
                    rusqlite::params![book_id, RECORD_LEN_PREFIX as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(sql)?;
            Ok((book_id, latest, used))
        })?;

        // 把本批已写出去的字节并进来：取索引侧与该批次侧的较大者。
        let mut current_pack = latest.unwrap_or(1);
        let mut used_bytes = used.unwrap_or(PACK_HEADER_LEN as i64) as u64;
        for (_, pack_id, offset, length, _) in pending {
            if *pack_id == current_pack {
                used_bytes = used_bytes.max(*offset + RECORD_LEN_PREFIX as u64 + *length as u64);
            } else if *pack_id > current_pack {
                // 本批已经开过更新的容器：索引还没提交，以本批为准
                current_pack = *pack_id;
                used_bytes = *offset + RECORD_LEN_PREFIX as u64 + *length as u64;
            }
        }

        if used_bytes >= capacity {
            return Ok((book_id, current_pack + 1));
        }
        Ok((book_id, current_pack))
    }

    // -----------------------------------------------------------------------
    // 淘汰（FR-031）
    // -----------------------------------------------------------------------

    /// 按 LRU 淘汰容器，直到总占用降到 `target_bytes` 以下。
    ///
    /// 返回被淘汰的容器数与被释放的字节数。
    ///
    /// # 淘汰的单位是容器
    ///
    /// 容器内的记录是顺序追加、不可变的，删中间一条会让其余记录偏移失效。
    /// 故一次淘汰整个容器（文件 + 它的全部索引行）。
    ///
    /// # 为什么返回两个数值
    ///
    /// "淘汰了 3 个容器"与"释放了 12 MB"是两件事：前者说明粒度，后者说明
    /// 效果。只报一个的话，用户看不出"清了很多但没释放空间"（容器本来就很小时）
    /// 与"清了一个但释放了很多"。
    pub fn evict_to(&self, target_bytes: u64) -> Result<EvictionReport, StoreError> {
        let mut report = EvictionReport::default();
        // 淘汰循环的上限：容器数 + 1（防御性）。正常路径下每轮必然删掉
        // 至少一个容器，故不会真的跑满；而万一淘汰没能减小占用（如目标
        // 值比单容器还小），上限保证不会死循环。
        let max_rounds = self.list_packs()?.len() + 1;

        for _ in 0..max_rounds {
            let usage = self.usage_bytes()?;
            if usage <= target_bytes {
                break;
            }
            let Some((pack, _last_used)) = self.oldest_pack()? else {
                // 没有可淘汰的容器了：剩下的占用不来自容器（如目录项）
                break;
            };
            let freed = self.evict_pack(pack.0, pack.1)?;
            report.evicted_packs += 1;
            report.freed_bytes += freed;
        }
        report.remaining_bytes = self.usage_bytes()?;
        Ok(report)
    }

    /// 淘汰所有**没有任何索引行指向**的容器文件（崩溃遗留的孤儿）。
    ///
    /// 返回删除的文件数。它们不占索引，但占磁盘——不清的话，反复崩溃会让
    /// 空间一直被吃掉，而 `usage_bytes`（按索引算）看不出异常。
    pub fn evict_orphan_packs(&self) -> Result<usize, StoreError> {
        let tracked = self.tracked_pack_files()?;
        let mut removed = 0usize;
        for (book_id, pack_id, path) in self.list_packs()? {
            if tracked.contains(&(book_id, pack_id)) {
                continue;
            }
            // 只删**空**的或**没有索引指向**的容器。但可移植目录下可能有
            // 别的进程正在写——故删除失败不报错，只是没删成。
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// 删掉某一章及其内容索引（不回收文件空间）。
    pub fn remove_chapter(&self, chapter_id: i64) -> Result<(), StoreError> {
        self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            let affected = transaction
                .execute(
                    "DELETE FROM chapter_cache WHERE chapter_id=?1",
                    [chapter_id],
                )
                .map_err(sql)?;
            transaction
                .execute("UPDATE chapter SET is_cached=0 WHERE id=?1", [chapter_id])
                .map_err(sql)?;
            transaction.commit().map_err(sql)?;
            if affected == 0 {
                return Err(StoreError::not_found(format!("chapter_cache#{chapter_id}")));
            }
            Ok(())
        })
    }

    /// 清空**某一本书**的章节缓存（FR-031 / SC-015）。
    ///
    /// # 只碰缓存
    ///
    /// 本方法只删 `chapter_cache` 行（与 `chapter.is_cached` 标记）与
    /// `cache/packs/book-<id>/` 目录。书架、进度、书签、高亮、批注、
    /// 书源、规则一张表都不碰——SC-015 要求"用户执行缓存清理后，个人阅读
    /// 数据 100% 保留"。
    pub fn clear_book_cache(&self, book_id: i64) -> Result<EvictionReport, StoreError> {
        let freed = self.book_cache_bytes(book_id)?;
        let packs = self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            let packs: i64 = transaction
                .query_row(
                    "SELECT COUNT(DISTINCT pack_id) FROM chapter_cache WHERE book_id=?1",
                    [book_id],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            transaction
                .execute("DELETE FROM chapter_cache WHERE book_id=?1", [book_id])
                .map_err(sql)?;
            transaction
                .execute("UPDATE chapter SET is_cached=0 WHERE book_id=?1", [book_id])
                .map_err(sql)?;
            transaction.commit().map_err(sql)?;
            Ok(packs)
        })? as usize;

        let dir = self.book_dir(book_id);
        if dir.is_dir() {
            std::fs::remove_dir_all(&dir).map_err(|error| from_io(&dir, error))?;
        }
        Ok(EvictionReport {
            evicted_packs: packs,
            freed_bytes: freed,
            remaining_bytes: self.usage_bytes()?,
        })
    }

    /// 清空全部章节缓存（FR-031 / SC-015）。
    pub fn clear_all(&self) -> Result<EvictionReport, StoreError> {
        let freed = self.usage_bytes()?;
        let packs = self.db.write(|connection| {
            let transaction = connection.transaction().map_err(sql)?;
            let packs: i64 = transaction
                .query_row(
                    "SELECT COUNT(DISTINCT pack_id) FROM chapter_cache",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            transaction
                .execute("DELETE FROM chapter_cache", [])
                .map_err(sql)?;
            transaction
                .execute("UPDATE chapter SET is_cached=0", [])
                .map_err(sql)?;
            transaction.commit().map_err(sql)?;
            Ok(packs)
        })? as usize;

        if self.root.is_dir() {
            std::fs::remove_dir_all(&self.root).map_err(|error| from_io(&self.root, error))?;
        }
        Ok(EvictionReport {
            evicted_packs: packs,
            freed_bytes: freed,
            remaining_bytes: 0,
        })
    }

    // -----------------------------------------------------------------------
    // 核算（SC-005 / SC-005b）
    // -----------------------------------------------------------------------

    /// 容器占用的总字节数（**按索引算**，不含孤儿文件）。
    ///
    /// # 为什么"按索引算"而不是"扫目录求和"
    ///
    /// SC-005 的判据是"缓存目录总字节数 ≤ 所缓存正文内容字节数 × 1.01"——
    /// 分子是实际占用，分母是内容。若分子把崩溃遗留的孤儿字节算进去，
    /// 一次崩溃就能让这个比值爆掉，而那是**可回收的垃圾**而不是缓存的
    /// 正常开销。分开报（本方法 + [`Self::orphan_bytes`]）让两者都可观测。
    pub fn usage_bytes(&self) -> Result<u64, StoreError> {
        self.db.read(|connection| {
            // 头部（每容器 14 字节）+ 每条记录的长度前缀（4 字节）+ 内容
            let pack_headers: i64 = connection
                .query_row(
                    "SELECT COUNT(DISTINCT book_id || '-' || pack_id) FROM chapter_cache",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            let records: i64 = connection
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(length), 0) FROM chapter_cache",
                    [],
                    |row| {
                        Ok(row.get::<_, i64>(0)? * RECORD_LEN_PREFIX as i64
                            + row.get::<_, i64>(1)?)
                    },
                )
                .map_err(sql)?;
            Ok((pack_headers * PACK_HEADER_LEN as i64 + records) as u64)
        })
    }

    /// 内容本身的总字节数（SC-005 的分母）。
    pub fn content_bytes(&self) -> Result<u64, StoreError> {
        self.db.read(|connection| {
            let total: i64 = connection
                .query_row(
                    "SELECT COALESCE(SUM(length), 0) FROM chapter_cache",
                    [],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            Ok(total as u64)
        })
    }

    /// 单本书的缓存字节数（按索引算）。
    pub fn book_cache_bytes(&self, book_id: i64) -> Result<u64, StoreError> {
        self.db.read(move |connection| {
            let pack_count: i64 = connection
                .query_row(
                    "SELECT COUNT(DISTINCT pack_id) FROM chapter_cache WHERE book_id=?1",
                    [book_id],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            let records: i64 = connection
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(length), 0) FROM chapter_cache WHERE book_id=?1",
                    [book_id],
                    |row| {
                        Ok(row.get::<_, i64>(0)? * RECORD_LEN_PREFIX as i64
                            + row.get::<_, i64>(1)?)
                    },
                )
                .map_err(sql)?;
            Ok((pack_count * PACK_HEADER_LEN as i64 + records) as u64)
        })
    }

    /// 磁盘上**实际**的容器文件字节数（含孤儿）。
    ///
    /// 与 [`Self::usage_bytes`] 的差额即孤儿占用。测试用它验证"内容侧真的
    /// 落盘了"，而崩溃恢复用它诊断"有多少垃圾可以回收"。
    pub fn disk_bytes(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;
        for (_, _, path) in self.list_packs()? {
            if let Ok(metadata) = std::fs::metadata(&path) {
                total += metadata.len();
            }
        }
        Ok(total)
    }

    /// 孤儿字节数（磁盘实际占用 − 索引覆盖的占用）。
    pub fn orphan_bytes(&self) -> Result<u64, StoreError> {
        Ok(self.disk_bytes()?.saturating_sub(self.usage_bytes()?))
    }

    /// 容器文件总数（SC-005b 的文件数断言）。
    pub fn pack_file_count(&self) -> Result<usize, StoreError> {
        Ok(self.list_packs()?.len())
    }

    /// 缓存条目数（索引行数）。
    pub fn entry_count(&self) -> Result<i64, StoreError> {
        self.db.read(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM chapter_cache", [], |row| row.get(0))
                .map_err(sql)
        })
    }

    /// 扫描磁盘上的全部容器文件，返回 `(book_id, pack_id, path)`。
    fn list_packs(&self) -> Result<Vec<(i64, i64, PathBuf)>, StoreError> {
        let mut packs = Vec::new();
        let Ok(books) = std::fs::read_dir(&self.root) else {
            // 目录不存在 = 还没缓存过任何东西，是正常状态
            return Ok(packs);
        };
        for book_entry in books {
            let book_entry = match book_entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let book_name = book_entry.file_name();
            let Some(book_id) = book_name
                .to_str()
                .and_then(|name| name.strip_prefix("book-"))
                .and_then(|digits| digits.parse::<i64>().ok())
            else {
                continue;
            };
            let Ok(files) = std::fs::read_dir(book_entry.path()) else {
                continue;
            };
            for file_entry in files {
                let file_entry = match file_entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                let file_name = file_entry.file_name();
                let Some(pack_id) = file_name
                    .to_str()
                    .and_then(|name| name.strip_suffix(".pk"))
                    .and_then(|digits| digits.parse::<i64>().ok())
                else {
                    continue;
                };
                packs.push((book_id, pack_id, file_entry.path()));
            }
        }
        packs.sort();
        Ok(packs)
    }

    /// 索引里出现过的 `(book_id, pack_id)`。
    fn tracked_pack_files(&self) -> Result<std::collections::HashSet<(i64, i64)>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT DISTINCT book_id, pack_id FROM chapter_cache")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(sql)?;
            let mut tracked = std::collections::HashSet::new();
            for row in rows {
                tracked.insert(row.map_err(sql)?);
            }
            Ok(tracked)
        })
    }

    /// 最久未使用的容器 `((book_id, pack_id), 最大的 cached_at)`。
    fn oldest_pack(&self) -> Result<Option<((i64, i64), i64)>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT book_id, pack_id, MAX(cached_at) AS last_used
                     FROM chapter_cache GROUP BY book_id, pack_id
                     ORDER BY last_used, book_id, pack_id LIMIT 1",
                )
                .map_err(sql)?;
            let mut rows = statement.query([]).map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some((
                    (row.get(0).map_err(sql)?, row.get(1).map_err(sql)?),
                    row.get(2).map_err(sql)?,
                ))),
                None => Ok(None),
            }
        })
    }

    /// 淘汰一个容器：删文件 + 删它的全部索引行。返回释放的字节数。
    fn evict_pack(&self, book_id: i64, pack_id: i64) -> Result<u64, StoreError> {
        let path = self.pack_path(book_id, pack_id);
        let file_bytes = std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);

        let indexed = self.db.write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            let bytes: i64 = transaction
                .query_row(
                    "SELECT COALESCE(SUM(length), 0) FROM chapter_cache
                     WHERE book_id=?1 AND pack_id=?2",
                    rusqlite::params![book_id, pack_id],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            // 按容器淘汰：先把该容器的章节标为未缓存，再删索引行。
            // 顺序反了会先失去"哪些章节该重置标记"的信息。
            transaction
                .execute(
                    "UPDATE chapter SET is_cached=0 WHERE id IN (
                        SELECT chapter_id FROM chapter_cache WHERE book_id=?1 AND pack_id=?2
                     )",
                    rusqlite::params![book_id, pack_id],
                )
                .map_err(sql)?;
            let deleted = transaction
                .execute(
                    "DELETE FROM chapter_cache WHERE book_id=?1 AND pack_id=?2",
                    rusqlite::params![book_id, pack_id],
                )
                .map_err(sql)?;
            transaction.commit().map_err(sql)?;
            Ok((bytes, deleted))
        })?;

        // 删文件在索引清空**之后**：反过来会留下指向不存在文件的索引行
        // （正是 FR-030 要杜绝的悬空引用，只不过这里是自己造的）。
        if path.exists() {
            if let Err(error) = std::fs::remove_file(&path) {
                // 文件删不掉（被占用/权限）不算致命：索引已经清了，那些字节
                // 成为孤儿，下次启动的 evict_orphan_packs 会再试一次。
                let _ = error;
            }
        }
        // 目录空了就一并删掉，避免留下一堆空目录
        let book_dir = self.book_dir(book_id);
        if book_dir.is_dir()
            && std::fs::read_dir(&book_dir)
                .map(|mut dir| dir.next().is_none())
                .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&book_dir);
        }
        Ok(file_bytes.max(indexed.0 as u64))
    }

    /// 校验一个容器文件的头部，返回其中的 `pack_id`。
    ///
    /// 供恢复路径与自检使用：把"这个文件是不是合法容器、属于谁"变成可查询的事实。
    pub fn inspect_pack(path: &Path) -> Result<u32, StoreError> {
        let mut file = File::open(path).map_err(|error| from_io(path, error))?;
        let mut header = [0u8; PACK_HEADER_LEN as usize];
        // 读不满头部 = 文件被截断（写头部途中崩溃）。归类为 Corrupt 而不是 Io：
        // 调用方据此知道"文件本身不完整"，而不是"磁盘出了错"——两者的处置
        // 完全不同（前者删了重下，后者去查硬件）。
        file.read_exact(&mut header).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                StoreError::Corrupt {
                    subject: path.display().to_string(),
                    reason: format!(
                        "容器文件不完整：头部应 {PACK_HEADER_LEN} 字节，实际不足（写入途中被中断）"
                    ),
                }
            } else {
                from_io(path, error)
            }
        })?;
        decode_header(&header, path)
    }
}

/// 一次淘汰的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvictionReport {
    /// 被淘汰（或清理）的容器数
    pub evicted_packs: usize,
    /// 释放的字节数
    pub freed_bytes: u64,
    /// 操作后剩余的缓存字节数
    pub remaining_bytes: u64,
}

/// 解码并校验容器头部。
fn decode_header(header: &[u8; PACK_HEADER_LEN as usize], path: &Path) -> Result<u32, StoreError> {
    if header[..8] != PACK_MAGIC {
        return Err(StoreError::Corrupt {
            subject: path.display().to_string(),
            reason: "容器头部魔数不符——这不是 Pack 容器文件".to_string(),
        });
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if version != PACK_VERSION {
        return Err(StoreError::Corrupt {
            subject: path.display().to_string(),
            reason: format!("容器格式版本 {version} 不受支持（本程序支持 {PACK_VERSION}）"),
        });
    }
    let pack_id = u32::from_le_bytes([header[10], header[11], header[12], header[13]]);

    // 文件名里的 id 与头部里的 id 必须一致。不一致说明文件被改名/搬错位置/
    // 从备份还原到了错误的地方——继续读会读到**别的书的正文**。
    if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
        if let Ok(file_pack_id) = stem.parse::<u32>() {
            if file_pack_id != pack_id {
                return Err(StoreError::Corrupt {
                    subject: path.display().to_string(),
                    reason: format!(
                        "文件名里的 pack_id={file_pack_id} 与头部里的 {pack_id} 不一致"
                    ),
                });
            }
        }
    }
    Ok(pack_id)
}

/// 确保容器文件存在且头部正确（不存在则新建并写头部 + fsync）。
fn ensure_pack_file(path: &Path, pack_id: i64) -> Result<(), StoreError> {
    if path.exists() {
        // 已存在：校验头部，避免往一个损坏的文件上继续追加
        PackStore::inspect_pack(path)?;
        return Ok(());
    }

    let mut header = [0u8; PACK_HEADER_LEN as usize];
    header[..8].copy_from_slice(&PACK_MAGIC);
    header[8..10].copy_from_slice(&PACK_VERSION.to_le_bytes());
    header[10..14].copy_from_slice(&(pack_id as u32).to_le_bytes());

    let mut file = File::create(path).map_err(|error| from_io(path, error))?;
    file.write_all(&header)
        .map_err(|error| from_io(path, error))?;
    // 头部也必须落盘：只写内容不写头部的文件读不出来，而"文件存在"
    // 会让后续写入以为头部已经在了。
    file.sync_data().map_err(|error| from_io(path, error))?;
    Ok(())
}

/// 按位置从容器读出内容。
fn read_at(
    path: &Path,
    book_id: i64,
    pack_id: i64,
    location: PackLocation,
) -> Result<Vec<u8>, StoreError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        // 索引指向的容器文件不存在——**这正是 FR-030 要杜绝的悬空引用**。
        // 报出来而不是返回 None：返回 None 会让上层静默重抓，永远发现不了
        // 那道防线已经失效。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::Corrupt {
                subject: format!("book#{book_id}/pack#{pack_id}"),
                reason: format!("索引指向的容器文件 {} 不存在（悬空引用）", path.display()),
            })
        }
        Err(error) => return Err(from_io(path, error)),
    };

    // 头部校验放在读取之前：它一次性挡住"这不是容器""版本不符""放错位置"
    // 三类问题，而三类都会让后续的偏移解析读出垃圾。
    let mut header = [0u8; PACK_HEADER_LEN as usize];
    file.read_exact(&mut header).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            StoreError::Corrupt {
                subject: path.display().to_string(),
                reason: format!("容器文件不完整：头部不足 {PACK_HEADER_LEN} 字节"),
            }
        } else {
            from_io(path, error)
        }
    })?;
    let actual_pack_id = decode_header(&header, path)?;
    if actual_pack_id as i64 != pack_id {
        return Err(StoreError::Corrupt {
            subject: path.display().to_string(),
            reason: format!("容器 id 不符：索引说 {pack_id}，头部说 {actual_pack_id}"),
        });
    }

    if location.offset < PACK_HEADER_LEN {
        return Err(StoreError::Corrupt {
            subject: path.display().to_string(),
            reason: format!(
                "偏移 {} 落在容器头部之内（头部 {} 字节）",
                location.offset, PACK_HEADER_LEN
            ),
        });
    }

    file.seek(SeekFrom::Start(location.offset))
        .map_err(|error| from_io(path, error))?;

    let mut prefix = [0u8; RECORD_LEN_PREFIX];
    file.read_exact(&mut prefix)
        .map_err(|error| from_io(path, error))?;
    let length = u32::from_le_bytes(prefix);
    if length != location.length {
        return Err(StoreError::Corrupt {
            subject: path.display().to_string(),
            reason: format!("记录长度前缀 {length} 与索引里的 {} 不符", location.length),
        });
    }
    if length > MAX_RECORD_BYTES {
        return Err(StoreError::Corrupt {
            subject: path.display().to_string(),
            reason: format!("记录长度 {length} 超过上限 {MAX_RECORD_BYTES}（文件可能已损坏）"),
        });
    }

    let mut content = vec![0u8; length as usize];
    file.read_exact(&mut content)
        .map_err(|error| from_io(path, error))?;
    Ok(content)
}

/// 把索引里按 `(book_id)` 分组的容器数统计出来（诊断用）。
///
/// 返回 `book_id → 容器数`。
pub fn packs_per_book(store: &PackStore) -> Result<HashMap<i64, usize>, StoreError> {
    let packs = store.list_packs()?;
    let mut counts = HashMap::new();
    for (book_id, _, _) in packs {
        *counts.entry(book_id).or_insert(0usize) += 1;
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: PathBuf,
        db: crate::db::Db,
        store: PackStore,
        book_id: i64,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str, capacity: u64) -> Fixture {
        fixture_with_chapters(label, capacity, 0)
    }

    fn fixture_with_chapters(label: &str, capacity: u64, chapters: usize) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-pack-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");
        let root = dir.join("cache").join("packs");

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

        if chapters > 0 {
            db.write(move |connection| {
                let transaction = connection.transaction().map_err(sql)?;
                {
                    let mut statement = transaction
                        .prepare_cached(
                            "INSERT INTO chapter(book_id, index_in_book, title)
                             VALUES (?1, ?2, ?3)",
                        )
                        .map_err(sql)?;
                    for index in 0..chapters {
                        statement
                            .execute(rusqlite::params![
                                book_id,
                                index as i32,
                                format!("第{index}章")
                            ])
                            .map_err(sql)?;
                    }
                }
                transaction.commit().map_err(sql)?;
                Ok(())
            })
            .expect("应能建章节");
        }

        let store = PackStore::with_capacity(&root, db.clone(), capacity);
        Fixture {
            dir,
            db,
            store,
            book_id,
        }
    }

    fn chapter_ids(fixture: &Fixture) -> Vec<i64> {
        fixture
            .db
            .read(move |connection| {
                let mut statement = connection
                    .prepare_cached("SELECT id FROM chapter ORDER BY index_in_book")
                    .map_err(sql)?;
                let rows = statement.query_map([], |row| row.get(0)).map_err(sql)?;
                let mut ids = Vec::new();
                for row in rows {
                    ids.push(row.map_err(sql)?);
                }
                Ok(ids)
            })
            .expect("应能查章节")
    }

    fn content_of(index: usize, size: usize) -> Vec<u8> {
        let seed = format!("chapter-{index}-");
        let mut text = seed.clone();
        while text.len() < size {
            text.push_str(&seed);
        }
        text.truncate(size);
        text.into_bytes()
    }

    // --- 读写 ---

    #[test]
    fn 写入后能按章读回() {
        let fixture = fixture_with_chapters("roundtrip", DEFAULT_PACK_CAPACITY_BYTES, 3);
        let ids = chapter_ids(&fixture);
        for (index, id) in ids.iter().enumerate() {
            fixture
                .store
                .write_chapter(*id, &content_of(index, 100))
                .expect("应能写");
        }
        for (index, id) in ids.iter().enumerate() {
            let read = fixture
                .store
                .read_chapter(*id)
                .expect("应能读")
                .expect("应有");
            assert_eq!(read, content_of(index, 100), "第 {index} 章内容应一致");
        }
    }

    #[test]
    fn 未缓存的章节返回_none() {
        let fixture = fixture_with_chapters("miss", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        assert_eq!(
            fixture.store.read_chapter(ids[0]).expect("应成功"),
            None,
            "没缓存是正常状态，不是错误"
        );
    }

    #[test]
    fn 二进制内容原样往返() {
        let fixture = fixture_with_chapters("binary", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        let payload: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        fixture
            .store
            .write_chapter(ids[0], &payload)
            .expect("应能写");
        assert_eq!(
            fixture.store.read_chapter(ids[0]).expect("应能读"),
            Some(payload)
        );
    }

    #[test]
    fn 空内容也能往返() {
        let fixture = fixture_with_chapters("empty-content", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture.store.write_chapter(ids[0], b"").expect("应能写");
        assert_eq!(
            fixture.store.read_chapter(ids[0]).expect("应能读"),
            Some(Vec::new())
        );
    }

    #[test]
    fn 同一章重复写入以最后一次为准() {
        let fixture = fixture_with_chapters("overwrite", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture
            .store
            .write_chapter(ids[0], "旧内容".as_bytes())
            .expect("应能写");
        fixture
            .store
            .write_chapter(ids[0], "新内容".as_bytes())
            .expect("应能写");
        assert_eq!(
            fixture.store.read_chapter(ids[0]).expect("应能读"),
            Some("新内容".as_bytes().to_vec()),
            "索引应指向最后一次写入的位置"
        );
    }

    #[test]
    fn 写入不存在的章节报结构化错误() {
        let fixture = fixture("no-chapter", DEFAULT_PACK_CAPACITY_BYTES);
        let error = fixture
            .store
            .write_chapter(99999, b"x")
            .expect_err("应报错");
        assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn 写入时标记章节已缓存() {
        let fixture = fixture_with_chapters("flag", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture.store.write_chapter(ids[0], b"x").expect("应能写");
        let cached: i64 = fixture
            .db
            .read(move |connection| {
                connection
                    .query_row(
                        "SELECT is_cached FROM chapter WHERE id=?1",
                        [ids[0]],
                        |row| row.get(0),
                    )
                    .map_err(sql)
            })
            .expect("应能查");
        assert_eq!(cached, 1);
    }

    // --- 容器分配 ---

    #[test]
    fn 容器写满后自动开新容器() {
        // 容量 4096：每章 1000 字节（+4 前缀），一个容器约装 4 章
        let fixture = fixture_with_chapters("seal", 4096, 20);
        let ids = chapter_ids(&fixture);
        let mut sealed_count = 0;
        for (index, id) in ids.iter().enumerate() {
            let written = fixture
                .store
                .write_chapter(*id, &content_of(index, 1000))
                .expect("应能写");
            if written.sealed {
                sealed_count += 1;
            }
        }
        assert!(
            sealed_count >= 3,
            "应有多个容器被写满（实际 {sealed_count}）"
        );
        let packs = fixture.store.pack_file_count().expect("应能数");
        assert!(
            packs >= 4,
            "20 章 × 1000 字节 / 4096 应产生至少 4 个容器，实际 {packs}"
        );
        assert_eq!(fixture.store.entry_count().expect("应能数"), 20);
    }

    #[test]
    fn 同一本书的容器在同一目录下() {
        let fixture = fixture_with_chapters("grouping", 4096, 20);
        let ids = chapter_ids(&fixture);
        for (index, id) in ids.iter().enumerate() {
            fixture
                .store
                .write_chapter(*id, &content_of(index, 1000))
                .expect("应能写");
        }
        let book_dir = fixture.store.book_dir(fixture.book_id);
        assert!(book_dir.is_dir(), "同一本书的容器应聚在一个目录");
        let files: Vec<_> = std::fs::read_dir(&book_dir)
            .expect("应能读")
            .filter_map(|entry| entry.ok())
            .collect();
        assert!(files.len() > 1, "应有多个容器文件");
    }

    #[test]
    fn 容器头部自证身份() {
        let fixture = fixture_with_chapters("header", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture.store.write_chapter(ids[0], b"x").expect("应能写");
        let path = fixture.store.pack_path(fixture.book_id, 1);
        assert_eq!(
            PackStore::inspect_pack(&path).expect("头部应合法"),
            1,
            "头部里的 pack_id 应与文件名一致"
        );
    }

    #[test]
    fn 头部损坏的容器被拒绝() {
        let fixture = fixture_with_chapters("bad-header", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture.store.write_chapter(ids[0], b"x").expect("应能写");
        let path = fixture.store.pack_path(fixture.book_id, 1);

        // 破坏魔数
        let mut bytes = std::fs::read(&path).expect("应能读");
        bytes[0] = 0xFF;
        std::fs::write(&path, &bytes).expect("应能写");

        let error = PackStore::inspect_pack(&path).expect_err("应拒绝");
        assert!(
            matches!(error, StoreError::Corrupt { .. }),
            "魔数不符应报损坏：{error:?}"
        );
    }

    #[test]
    fn 文件被改名后读会报损坏而不是读错内容() {
        // 只信文件名的话，一份放错位置的容器会被当成本地容器读，
        // 用户看到的是**别的书的正文**
        let fixture = fixture_with_chapters("renamed", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture
            .store
            .write_chapter(ids[0], "内容".as_bytes())
            .expect("应能写");
        let original = fixture.store.pack_path(fixture.book_id, 1);
        let renamed = fixture.store.pack_path(fixture.book_id, 7);
        std::fs::rename(&original, &renamed).expect("应能改名");

        let error = PackStore::inspect_pack(&renamed).expect_err("应拒绝");
        assert!(
            error.to_string().contains("不一致"),
            "应指出文件名与头部不符：{error}"
        );
    }

    // --- 崩溃安全（FR-030 / SC-007）---

    #[test]
    fn 索引指向不存在的容器时报损坏而非静默() {
        // 这是 FR-030 要杜绝的悬空引用的**可构造形态**：直接往索引里插一行
        // 指向不存在的容器。真实崩溃场景由外部的故障注入测试覆盖
        // （`tests/pack_crash_safety.rs`：写入中 kill 子进程）。
        let fixture = fixture_with_chapters("dangling", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        let chapter_id = ids[0];
        fixture
            .db
            .write(move |connection| {
                connection
                    .execute(
                        "INSERT INTO chapter_cache(chapter_id, book_id, pack_id, offset, length, cached_at)
                         VALUES (?1, ?2, 99, 14, 10, 0)",
                        rusqlite::params![chapter_id, 1i64],
                    )
                    .map_err(sql)
            })
            .expect("应能插索引");

        let error = fixture.store.read_chapter(ids[0]).expect_err("应报损坏");
        assert!(
            matches!(error, StoreError::Corrupt { .. }),
            "悬空引用必须报错而非返回 None（否则永远发现不了防线失效）：{error:?}"
        );
        assert!(
            error.to_string().contains("不存在"),
            "应说明文件不存在：{error}"
        );
    }

    #[test]
    fn 记录长度前缀与索引不符时报损坏() {
        let fixture = fixture_with_chapters("length-mismatch", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture
            .store
            .write_chapter(ids[0], "内容内容内容".as_bytes())
            .expect("应能写");
        // 篡改索引里的长度
        let chapter_id = ids[0];
        fixture
            .db
            .write(move |connection| {
                connection
                    .execute(
                        "UPDATE chapter_cache SET length = length + 5 WHERE chapter_id=?1",
                        [chapter_id],
                    )
                    .map_err(sql)
            })
            .expect("应能改");

        let error = fixture.store.read_chapter(ids[0]).expect_err("应报损坏");
        assert!(error.to_string().contains("不符"), "{error}");
    }

    #[test]
    fn 头部未落盘的半截文件被识别为损坏() {
        // 模拟"只写了一部分头部就被 kill"
        let fixture = fixture_with_chapters("partial-header", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let path = fixture.store.pack_path(fixture.book_id, 1);
        std::fs::create_dir_all(path.parent().expect("应有父目录")).expect("应能建目录");
        std::fs::write(&path, b"\0tome").expect("应能写半截");

        let error = PackStore::inspect_pack(&path).expect_err("应拒绝");
        assert!(matches!(error, StoreError::Corrupt { .. }), "{error:?}");
    }

    #[test]
    fn 写入顺序是内容先于索引() {
        // 顺序颠倒的**可观测后果**：索引提交后若内容还没落盘，
        // 重启（重新打开库 + 重新读文件）会读到不完整内容。
        //
        // 这里用一个直接的观测：写入返回后，**文件里的字节数**必须已经
        // 达到索引所指的位置 + 长度（说明内容真的写出去了，而不是缓冲着）。
        let fixture = fixture_with_chapters("order", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        let content = content_of(0, 4096);
        let written = fixture
            .store
            .write_chapter(ids[0], &content)
            .expect("应能写");

        let path = fixture.store.pack_path(fixture.book_id, 1);
        let disk_len = std::fs::metadata(&path).expect("应有文件").len();
        assert!(
            disk_len >= written.location.offset + RECORD_LEN_PREFIX as u64 + content.len() as u64,
            "写入返回时内容必须已在文件里（{disk_len} 字节），否则索引会先于内容可见"
        );

        // 重新打开库读索引：索引行的存在必须以内容已落盘为前提
        let reopened = PackStore::new(
            fixture.store.root(),
            crate::db::Db::open(&fixture.dir.join("tome.db")).expect("应能重开"),
        );
        assert_eq!(
            reopened.read_chapter(ids[0]).expect("应能读"),
            Some(content),
            "重启路径必须能读到完整内容"
        );
    }

    // --- 容量核算（SC-005 / SC-005b）---

    #[test]
    fn 容量开销在_1_01_倍以内() {
        // SC-005：缓存目录总字节数 ≤ 所缓存正文内容字节数 × 1.01
        let fixture = fixture_with_chapters("overhead", DEFAULT_PACK_CAPACITY_BYTES, 500);
        let ids = chapter_ids(&fixture);
        // 每章约 6 KB（SC-005b 假设的单章平均大小）
        let mut content_total = 0u64;
        for (index, id) in ids.iter().enumerate() {
            let content = content_of(index, 6144);
            content_total += content.len() as u64;
            fixture.store.write_chapter(*id, &content).expect("应能写");
        }

        let usage = fixture.store.usage_bytes().expect("应能算");
        let ratio = usage as f64 / content_total as f64;
        eprintln!("[SC-005 实测/500章] 占用={usage} 内容={content_total} 比值={ratio:.6}");
        assert!(
            ratio <= 1.01,
            "容量开销 {ratio:.6} 超过 1.01（占用 {usage} / 内容 {content_total}）"
        );
        // 同时确认核算是可信的：磁盘上的实际占用与核算值一致（无孤儿）
        let disk = fixture.store.disk_bytes().expect("应能算");
        assert_eq!(disk, usage, "无孤儿时磁盘占用应等于按索引算的占用");
    }

    #[test]
    fn 五万章的文件数不超过总数_2_百分比() {
        // SC-005b：5 万章规模下，单章平均 6 KB 的书，缓存文件数不超过总章数的 2%
        //
        // # 这条用例测的是**容器分配策略**，不是磁盘吞吐
        //
        // 每章一次 `write_chapter` 都带一次 `sync_data`。5 万次 fsync 在
        // 慢盘/CI runner 上要几分钟，而那条断言（文件数 ≤1000）只取决于
        // "容器容量 32 MiB、每章 6 KiB"这个算术，与 fsync 无关。
        //
        // 故用 [`PackStore::write_chapter_batch`] 走同一条分配路径但**只在
        // 最后同步一次**。这不会让断言变弱：分配逻辑、索引提交、每次写入后
        // 的容量检查全部照旧；被省掉的只有中间过程的磁盘同步。
        //
        // 单章级的崩溃安全性由 `写入顺序是内容先于索引` 与
        // `tests/pack_crash_safety.rs` 的故障注入用例覆盖。
        let total_chapters = 50_000;
        let fixture =
            fixture_with_chapters("file-count", DEFAULT_PACK_CAPACITY_BYTES, total_chapters);
        let ids = chapter_ids(&fixture);
        assert_eq!(ids.len(), total_chapters);

        let batch: Vec<(i64, Vec<u8>)> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, content_of(index, 6144)))
            .collect();
        fixture
            .store
            .write_chapter_batch(&batch)
            .expect("应能批量写");

        let files = fixture.store.pack_file_count().expect("应能数");
        let limit = total_chapters * 2 / 100; // 2% = 1000
        assert!(
            files <= limit,
            "容器文件数 {files} 超过总章数的 2%（上限 {limit}）"
        );
        // 确认这批写入真的全部落库了（不是"文件少是因为没写进去"）
        assert_eq!(
            fixture.store.entry_count().expect("应能数"),
            total_chapters as i64
        );
        // 抽查内容真的可读（防止"索引写了、文件没写"的假绿）
        assert_eq!(
            fixture.store.read_chapter(ids[0]).expect("应能读"),
            Some(content_of(0, 6144))
        );
        assert_eq!(
            fixture
                .store
                .read_chapter(ids[total_chapters - 1])
                .expect("应能读"),
            Some(content_of(total_chapters - 1, 6144))
        );

        // 报告实测数据（供汇报引用）
        let usage = fixture.store.usage_bytes().expect("应能算");
        let content = fixture.store.content_bytes().expect("应能算");
        let disk = fixture.store.disk_bytes().expect("应能算");
        // 核算值必须与磁盘实际占用一致——否则 SC-005 的比值算的是幻觉
        assert_eq!(
            disk, usage,
            "按索引算的占用与磁盘实际占用不符（差额 = 孤儿占用）"
        );
        let ratio = usage as f64 / content as f64;
        assert!(
            ratio <= 1.01,
            "容量开销 {ratio:.6} 超过 1.01（占用 {usage} / 内容 {content}）"
        );
        eprintln!(
            "[SC-005b 实测] 章数={total_chapters} 文件数={files} 占比={:.4}% 上限={limit}",
            files as f64 / total_chapters as f64 * 100.0
        );
        eprintln!("[SC-005 实测] 磁盘占用={disk} 核算占用={usage} 内容={content} 比值={ratio:.6}");
    }

    #[test]
    fn 未写满的容器按实际占用计入() {
        let fixture = fixture_with_chapters("partial", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture
            .store
            .write_chapter(ids[0], &content_of(0, 100))
            .expect("应能写");
        let usage = fixture.store.usage_bytes().expect("应能算");
        // 头部 14 + 前缀 4 + 内容 100 = 118
        assert_eq!(usage, 118, "未写满的容器应按实际占用计入，而不是按容量上限");
    }

    // --- 淘汰（FR-031）---

    #[test]
    fn 淘汰以容器为单位且不留残留引用() {
        let fixture = fixture_with_chapters("evict", 4096, 20);
        let ids = chapter_ids(&fixture);
        for (index, id) in ids.iter().enumerate() {
            fixture
                .store
                .write_chapter(*id, &content_of(index, 1000))
                .expect("应能写");
        }
        let before = fixture.store.pack_file_count().expect("应能数");
        assert!(before > 1);

        // 淘汰到只剩一个容器的容量
        let report = fixture.store.evict_to(4096).expect("应能淘汰");
        assert!(report.evicted_packs > 0, "应有容器被淘汰");

        // 无残留引用：每一个索引行都必须能读出内容
        for id in &ids {
            if fixture.store.location_of(*id).expect("应能查").is_some() {
                fixture
                    .store
                    .read_chapter(*id)
                    .unwrap_or_else(|error| panic!("淘汰后不应有悬空引用：{error}"));
            }
        }
        // 被淘汰的容器的文件应真的没了
        assert!(
            fixture.store.pack_file_count().expect("应能数") < before,
            "淘汰后文件数应减少"
        );
    }

    #[test]
    fn 淘汰按_lru_先动最旧的() {
        let fixture = fixture_with_chapters("lru", 4096, 12);
        let ids = chapter_ids(&fixture);
        for (index, id) in ids.iter().enumerate() {
            fixture
                .store
                .write_chapter(*id, &content_of(index, 1000))
                .expect("应能写");
        }
        // 手动把最早的几章的 cached_at 调小，明确"谁是旧的"
        let ids_for_update = ids.clone();
        fixture
            .db
            .write(move |connection| {
                for id in ids_for_update.iter().take(6) {
                    connection
                        .execute(
                            "UPDATE chapter_cache SET cached_at = 1 WHERE chapter_id=?1",
                            [id],
                        )
                        .map_err(sql)?;
                }
                Ok(())
            })
            .expect("应能改");

        let report = fixture.store.evict_to(4096).expect("应能淘汰");
        assert!(report.evicted_packs > 0);

        // 新的那些（后 6 章所在的容器）应还在
        let surviving: usize = ids
            .iter()
            .filter(|id| fixture.store.location_of(**id).expect("应能查").is_some())
            .count();
        assert!(
            surviving > 0,
            "淘汰应保留较新的容器（剩 {surviving} 条索引）"
        );
    }

    #[test]
    fn 孤儿容器可回收() {
        // 崩溃遗留：文件在磁盘上，但没有索引指向它
        let fixture = fixture_with_chapters("orphan", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let orphan_path = fixture.store.pack_path(fixture.book_id, 42);
        std::fs::create_dir_all(orphan_path.parent().expect("应有父目录")).expect("应能建");
        // 写一个合法的容器头 + 一条内容，但**不**提交索引
        {
            let mut header = [0u8; PACK_HEADER_LEN as usize];
            header[..8].copy_from_slice(&PACK_MAGIC);
            header[8..10].copy_from_slice(&PACK_VERSION.to_le_bytes());
            header[10..14].copy_from_slice(&42u32.to_le_bytes());
            let mut bytes = header.to_vec();
            bytes.extend_from_slice(&5u32.to_le_bytes());
            bytes.extend_from_slice("半个内容".as_bytes());
            std::fs::write(&orphan_path, &bytes).expect("应能写");
        }

        assert!(fixture.store.disk_bytes().expect("应能算") > 0);
        assert!(
            fixture.store.orphan_bytes().expect("应能算") > 0,
            "应能识别出孤儿占用"
        );

        let removed = fixture.store.evict_orphan_packs().expect("应能回收");
        assert_eq!(removed, 1, "应回收一个孤儿容器");
        assert_eq!(fixture.store.orphan_bytes().expect("应能算"), 0);
    }

    // --- 清缓存不伤个人数据（FR-031 / SC-015）---

    #[test]
    fn 清缓存保留全部个人数据() {
        let fixture = fixture_with_chapters("preserve", DEFAULT_PACK_CAPACITY_BYTES, 2);
        let ids = chapter_ids(&fixture);
        for (index, id) in ids.iter().enumerate() {
            fixture
                .store
                .write_chapter(*id, &content_of(index, 100))
                .expect("应能写");
        }

        // 造一批个人数据
        let fixture_book = fixture.book_id;
        fixture
            .db
            .write(move |connection| {
                connection
                    .execute(
                        "INSERT INTO read_progress(book_id, chapter_index, char_offset, anchor_text, updated_at)
                         VALUES (?1, 1, 50, '锚点', 0)",
                        [fixture_book],
                    )
                    .map_err(sql)?;
                connection
                    .execute(
                        "INSERT INTO bookmark(book_id, chapter_index, char_offset, note, created_at)
                         VALUES (?1, 1, 50, '备注', 0)",
                        [fixture_book],
                    )
                    .map_err(sql)?;
                connection
                    .execute(
                        "INSERT INTO highlight(book_id, chapter_index, char_offset, length, style, created_at)
                         VALUES (?1, 1, 10, 5, 'yellow', 0)",
                        [fixture_book],
                    )
                    .map_err(sql)?;
                connection
                    .execute(
                        "INSERT INTO memo(book_id, chapter_index, char_offset, length, content, created_at)
                         VALUES (?1, 1, 10, 5, '想法', 0)",
                        [fixture_book],
                    )
                    .map_err(sql)?;
                connection
                    .execute(
                        "INSERT INTO read_record(book_id, date, read_seconds, read_chars)
                         VALUES (?1, '2026-10-10', 60, 1000)",
                        [fixture_book],
                    )
                    .map_err(sql)?;
                connection
                    .execute(
                        "INSERT INTO source(id, name, enabled, created_at) VALUES ('s', '源', 1, 0)",
                        [],
                    )
                    .map_err(sql)?;
                connection
                    .execute(
                        "INSERT INTO replace_rule(name, pattern, replacement, sort_order)
                         VALUES ('规则', 'p', 'r', 0)",
                        [],
                    )
                    .map_err(sql)?;
                Ok(())
            })
            .expect("应能写个人数据");

        fixture.store.clear_all().expect("应能清缓存");

        // 逐表断言：一张都不能少
        let counts: Vec<(&str, i64)> = [
            "book",
            "chapter",
            "read_progress",
            "bookmark",
            "highlight",
            "memo",
            "read_record",
            "source",
            "replace_rule",
        ]
        .iter()
        .map(|table| {
            let count: i64 = fixture
                .db
                .read(|connection| {
                    connection
                        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                            row.get(0)
                        })
                        .map_err(sql)
                })
                .expect("应能查");
            (*table, count)
        })
        .collect();

        for (table, count) in &counts {
            assert!(*count > 0, "清缓存后 `{table}` 的数据必须仍在（SC-015）");
        }
        assert_eq!(
            fixture.store.entry_count().expect("应能数"),
            0,
            "缓存索引应清空"
        );
    }

    #[test]
    fn 按书清缓存不影响别的书() {
        let fixture = fixture_with_chapters("per-book", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        fixture
            .store
            .write_chapter(ids[0], "第一本书".as_bytes())
            .expect("应能写");

        // 第二本书
        let second_book: i64 = fixture
            .db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at)
                         VALUES (0, 'y', '书二', 0)",
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
            .map(|(book_id, _)| book_id)
            .expect("应能建书二");
        let second_chapter: i64 = fixture
            .db
            .read(move |connection| {
                connection
                    .query_row(
                        "SELECT id FROM chapter WHERE book_id=?1",
                        [second_book],
                        |row| row.get(0),
                    )
                    .map_err(sql)
            })
            .expect("应能查");
        fixture
            .store
            .write_chapter(second_chapter, "第二本书".as_bytes())
            .expect("应能写");

        fixture
            .store
            .clear_book_cache(fixture.book_id)
            .expect("应能按书清");

        assert_eq!(
            fixture.store.read_chapter(ids[0]).expect("应成功"),
            None,
            "被清的书应无缓存"
        );
        assert_eq!(
            fixture.store.read_chapter(second_chapter).expect("应成功"),
            Some("第二本书".as_bytes().to_vec()),
            "未被清的书缓存应完好"
        );
    }

    #[test]
    fn 删除单章缓存() {
        let fixture = fixture_with_chapters("remove-chapter", DEFAULT_PACK_CAPACITY_BYTES, 2);
        let ids = chapter_ids(&fixture);
        for id in &ids {
            fixture.store.write_chapter(*id, b"x").expect("应能写");
        }
        fixture.store.remove_chapter(ids[0]).expect("应能删");
        assert_eq!(fixture.store.read_chapter(ids[0]).expect("应成功"), None);
        assert_eq!(
            fixture.store.read_chapter(ids[1]).expect("应成功"),
            Some(b"x".to_vec()),
            "另一章不该受影响"
        );

        let error = fixture
            .store
            .remove_chapter(ids[0])
            .expect_err("重复删应报错");
        assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
    }

    #[test]
    fn 淘汰报告给出粒度与效果两个数值() {
        let fixture = fixture_with_chapters("report", 4096, 20);
        let ids = chapter_ids(&fixture);
        for (index, id) in ids.iter().enumerate() {
            fixture
                .store
                .write_chapter(*id, &content_of(index, 1000))
                .expect("应能写");
        }
        let report = fixture.store.evict_to(0).expect("应能淘汰");
        assert!(report.evicted_packs > 0, "应报淘汰了几个容器");
        assert!(report.freed_bytes > 0, "应报释放了多少字节");
        assert_eq!(report.remaining_bytes, 0, "淘汰到 0 应清空");
    }

    #[test]
    fn 超大内容被拒() {
        let fixture = fixture_with_chapters("too-big", DEFAULT_PACK_CAPACITY_BYTES, 1);
        let ids = chapter_ids(&fixture);
        let huge = vec![0u8; MAX_RECORD_BYTES as usize + 1];
        let error = fixture
            .store
            .write_chapter(ids[0], &huge)
            .expect_err("应拒绝");
        assert!(matches!(error, StoreError::Corrupt { .. }), "{error:?}");
    }

    #[test]
    fn 容器容量过小时被抬到下限() {
        // 比"头部 + 一条最小记录"还小的容量会让每次写入都开新容器，
        // 那是无意义的配置
        let store = PackStore::with_capacity(
            Path::new("/tmp/x"),
            crate::db::Db::open(
                &std::env::temp_dir().join(format!("tome-pack-min-{}.db", std::process::id())),
            )
            .expect("应能开库"),
            1,
        );
        assert!(store.capacity_bytes > PACK_HEADER_LEN + RECORD_LEN_PREFIX as u64);
    }
}
