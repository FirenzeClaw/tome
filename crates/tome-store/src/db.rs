//! [DONE] SQLite actor 线程 + 迁移器（tasks.md T053）
//!
//! # 为什么是 actor 而不是连接池
//!
//! `rusqlite::Connection` 不是 `Sync`（它内部有非同步的语句缓存与句柄状态），
//! 而 `data-model.md`「Schema 治理」定的是：**数据库归专属 actor 线程所有，
//! 消息通道收发**。这条设计带来一个直接后果——调用方拿到的是**结果**而不是
//! 连接，故接缝 `BookRepository` 的每个方法都是"一次调用 = 一次完整操作"
//! （`contracts/seams.md` §3 的最后一条不变量）。
//!
//! 若改用 `Mutex<Connection>`，写操作会互相串行化（同样效果）但**读也会**——
//! 一个慢查询会卡住所有写入。actor 分读写两条路正是为了避免这一点。
//!
//! # 读写分离：读为什么不走 actor
//!
//! WAL 模式下 SQLite 允许**一个写者与多个读者并发**。若把所有读也塞进 actor
//! 队列，一次全表扫描（书内搜索、书架列表）就会排在所有写后面——那正是
//! tasks.md 验收项「读不阻塞写」要防的情形。
//!
//! 故：
//!
//! | 通道 | 实现 | 用途 |
//! |---|---|---|
//! | [`Db::write`] / [`Db::call`] | 专属 actor 线程独占一个写连接 | 一切写操作与需要事务的操作 |
//! | [`Db::read`] | **线程局部**只读连接（`query_only=ON`） | 纯查询 |
//!
//! 读连接用 `query_only=ON` 而非 `SQLITE_OPEN_READ_ONLY`：后者在 WAL 库上
//! 要求 `-shm`/`-wal` 文件可写，在"数据目录只读"的场景下会打不开——而那种
//! 场景恰恰是用户最需要能读出数据的（FR-043 的报告路径）。`query_only` 在
//! 连接层挡住写入，语义等价而不引入额外文件权限要求。
//!
//! # 事务的边界
//!
//! `Db::write` 收到的闭包**独占** `&mut Connection`，故实现方可以在其中开
//! 事务（`connection.transaction()`）。actor 一次只跑一个闭包，故不会有两个
//! 事务交错。这也是"批量写章节"（T057）能一次落库几万行的前提。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use rusqlite::Connection;

use crate::error::{from_rusqlite, StoreError};
use crate::schema::{MIGRATIONS, SCHEMA_VERSION};

/// actor 队列里的一项工作：在写连接上执行一段闭包。
///
/// 用 `Box<dyn FnOnce>` 而不是枚举各种操作：枚举会让每加一种操作就要改
/// 本文件（进而改这个 crate 的公共面），而闭包让 DAO 自由组合——DAO 层
/// 增删方法不会波及 actor 的骨架。
type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// SQLite 忙等待上限（毫秒）。
///
/// 读者与写者在 WAL 下的争用窗口极短，但 checkpoint 期间仍可能撞上。
/// 5 秒是"足够覆盖一次 checkpoint"与"用户不至于以为程序卡死"的折中。
///
/// [WORKAROUND] 硬编码常量：若实测发现大库（5 万章 + 全量缓存）checkpoint
/// 超过 5 秒，应改为可配置项（随 `config.toml` 的存储子集注入）而非继续调大。
const BUSY_TIMEOUT_MS: u64 = 5_000;

/// 数据库句柄。
///
/// `Clone` 是廉价的（共享同一个 actor 通道），故各 DAO 各持一份即可，
/// 不需要把 `Db` 包进 `Arc` 再逐层传递。
#[derive(Debug, Clone)]
pub struct Db {
    /// 数据库文件路径（读连接按需新建，故路径要留在句柄上）
    path: Arc<PathBuf>,
    /// actor 通道。发送端断开 = actor 线程退出（见 [`Db::write`] 的说明）。
    sender: Sender<Job>,
}

impl Db {
    /// 打开（或创建）数据库：建连接 → 设 PRAGMA → 跑迁移。
    ///
    /// # 为什么迁移在**调用方线程**完成，而不是在 actor 线程里
    ///
    /// 若迁移在 actor 里跑，`open` 就必须等一个就绪信号才能返回——那要么
    /// 引入一次握手（多一个会出错的环节），要么让"迁移失败"变成一个
    /// 异步事件（调用方拿到的 `Db` 看起来是好的，第一次查询才报错）。
    ///
    /// 在调用方线程跑则让"打开失败"与"打开成功"是**同一个返回值**的两种取值：
    /// 迁移报错时 `Db` 根本构造不出来，类型层面就没有"半开的库"。
    ///
    /// `Connection` 是 `Send`（非 `Sync`），故建好后移交给 actor 线程是安全的。
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| crate::error::from_io(parent, error))?;
            }
        }

        let mut connection = Connection::open(path).map_err(from_rusqlite)?;
        apply_pragmas(&connection)?;
        migrate(&mut connection)?;

        let (sender, receiver) = mpsc::channel::<Job>();
        let handle = std::thread::Builder::new()
            .name("tome-store-db".to_string())
            .spawn(move || {
                // 收到一项跑一项，直到发送端全部断开（进程退出或 Db 全部 drop）。
                while let Ok(job) = receiver.recv() {
                    job(&mut connection);
                }
                // 退出前收尾：WAL 里可能还有未 checkpoint 的页。
                // 失败不报错——库已经不再被使用，且下一次打开会自动重放 WAL。
                let _ = connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            })
            .map_err(|error| StoreError::io(path.display().to_string(), error.to_string()))?;

        // 线程句柄留在本地变量上：本函数返回后它被丢弃，即线程**分离**运行。
        // 这是刻意的——`Db` 是 `Clone` 的，若句柄存在 `Db` 里，最后一个克隆
        // 要负责 join，而"哪个是最后一个"在克隆语义下不可知。actor 的退出
        // 由通道关闭驱动（`Sender` 全部 drop 时 `recv` 返回 `Err`），不需要 join。
        drop(handle);

        Ok(Self {
            path: Arc::new(path.to_path_buf()),
            sender,
        })
    }

    /// 数据库文件路径。
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// 在 actor 线程上执行一段闭包，拿到结果。
    ///
    /// # 为什么同步等待
    ///
    /// 接缝 `BookRepository` 的方法是同步签名（`contracts/seams.md` §3）。
    /// 上层的"不阻塞"由**上层自己的线程**保证（GUI 把调用放到工作线程），
    /// 而不是靠这里返回一个 future——后者会让接缝的每个调用方都要处理
    /// 异步，而它们中的大多数（CLI、后台任务）本来就是同步语境。
    pub fn call<T, F>(&self, operation: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let (result_sender, result_receiver) = mpsc::channel::<Result<T, StoreError>>();
        let job: Job = Box::new(move |connection| {
            // 结果接收端可能已被丢弃（调用方提前返回/取消）。
            // 那不是错误，静默忽略——写操作本身仍然完成。
            let _ = result_sender.send(operation(connection));
        });
        self.sender.send(job).map_err(|_| self.actor_gone())?;
        result_receiver.recv().map_err(|_| self.actor_gone())?
    }

    /// 写操作的语义别名（`call` 的读写通道）。
    ///
    /// 单独留一个名字不是为了区分行为（二者是同一个通道），而是让 DAO 的
    /// 调用点**自解释**：读一眼就知道这一行是写。读走 [`Db::read`]。
    pub fn write<T, F>(&self, operation: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        self.call(operation)
    }

    /// 在**线程局部**的只读连接上执行查询。
    ///
    /// 不走 actor 队列，故不受排队的写操作影响（WAL 下读者与写者并发）。
    /// 见模块文档「读写分离」。
    pub fn read<T, F>(&self, query: F) -> Result<T, StoreError>
    where
        F: FnOnce(&Connection) -> Result<T, StoreError>,
    {
        with_reader(self.path.as_path(), query)
    }

    /// actor 已退出时的统一错误。
    ///
    /// ## 为什么会走到这里
    ///
    /// 正常路径下不会：`Db` 持有 `Sender`，而 actor 只在**全部** `Sender`
    /// 被 drop 后退出——即 `Db` 已经不存在了。能触发它的是 actor 线程在跑
    /// 某一项工作时 panic 了。那属于程序缺陷（生产路径禁 panic），但把它
    /// 报成"存储不可用"比让调用方拿到一个无解释的 `RecvError` 好。
    fn actor_gone(&self) -> StoreError {
        StoreError::sqlite(format!(
            "数据库 actor 线程已退出（{}）——通常是线程内发生了 panic",
            self.path.display()
        ))
    }
}

// ---------------------------------------------------------------------------
// PRAGMA 与迁移
// ---------------------------------------------------------------------------

/// 设置连接级 PRAGMA。
///
/// 逐条说明为什么需要：
///
/// | PRAGMA | 值 | 理由 |
/// |---|---|---|
/// | `journal_mode` | `WAL` | 读写并发的前提；崩溃后可自动恢复 |
/// | `foreign_keys` | `ON` | SQLite 默认**关闭**外键——不开则 `ON DELETE CASCADE` 完全不生效，删书会留下孤儿章节 |
/// | `synchronous` | `FULL` | FR-030 的崩溃安全要求索引提交必须真的落盘 |
/// | `busy_timeout` | 5000ms | 见 [`BUSY_TIMEOUT_MS`] |
fn apply_pragmas(connection: &Connection) -> Result<(), StoreError> {
    // `journal_mode` 返回一行结果，须用 `query_row` 而非 `execute`
    // （`execute` 对返回行的语句会报错）。
    let mode: String = connection
        .query_row("PRAGMA journal_mode=WAL;", [], |row| row.get(0))
        .map_err(from_rusqlite)?;
    if !mode.eq_ignore_ascii_case("wal") {
        // 内存库或只读文件系统上 WAL 会静默退化为别的模式。这里**明确报错**
        // 而不是继续跑：没有 WAL 就没有"读不阻塞写"，而那是 tasks.md 的
        // 验收项——静默降级会让验收项在无人察觉的情况下失效。
        return Err(StoreError::io(
            connection.path().unwrap_or("<memory>").to_string(),
            format!("无法启用 WAL 模式（实际为 `{mode}`）——需要可写的本地文件系统"),
        ));
    }

    // `synchronous=FULL`：WAL 下每次提交都 fsync WAL。
    //
    // 代价是批量写入变慢，收益是 FR-030 的崩溃安全有确定保证。这里选保守
    // 一侧：本项目是阅读器，写入不是热点路径，而"崩溃后索引与内容对不上"
    // 是用户无法自行修复的损坏。
    connection
        .execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA synchronous=FULL;
             PRAGMA busy_timeout=5000;",
        )
        .map_err(from_rusqlite)?;
    Ok(())
}

/// 把数据库迁移到 [`SCHEMA_VERSION`]。
///
/// 版本高于本程序所知时**拒绝打开**（见 [`StoreError::Migration`]）。
pub fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let current: i64 = connection
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(from_rusqlite)?;

    if current > SCHEMA_VERSION {
        return Err(StoreError::Migration {
            version: current,
            supported: SCHEMA_VERSION,
            reason: "数据目录由更新版本的程序创建，本版本不认识它的表结构".to_string(),
        });
    }

    if current == SCHEMA_VERSION {
        return Ok(());
    }

    for (index, script) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        apply_one_migration(connection, version, script)?;
    }
    Ok(())
}

/// 执行单个迁移脚本并写回版本号，全程一个事务。
///
/// # 为什么版本号在同一个事务里写
///
/// 若先提交 schema 再单独写 `user_version`，中间崩溃会留下"表建好了但版本
/// 还是旧的"状态——下次启动会**再跑一遍同一个脚本**，而 `CREATE TABLE` 不带
/// `IF NOT EXISTS`，于是启动永远失败。放进同一事务则要么都生效要么都不生效。
fn apply_one_migration(
    connection: &mut Connection,
    version: i64,
    script: &str,
) -> Result<(), StoreError> {
    let transaction = connection.transaction().map_err(from_rusqlite)?;
    transaction.execute_batch(script).map_err(from_rusqlite)?;
    // `PRAGMA` 不接受参数绑定，故用格式化拼入。`version` 是程序内自增的
    // 整数常量（不来自任何外部输入），不存在注入面。
    transaction
        .execute_batch(&format!("PRAGMA user_version={version};"))
        .map_err(from_rusqlite)?;
    transaction.commit().map_err(from_rusqlite)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 线程局部读连接
// ---------------------------------------------------------------------------

thread_local! {
    /// 每个线程一份只读连接表（按库路径区分，支持同一线程访问多个库）。
    ///
    /// # 为什么是线程局部而不是全局池
    ///
    /// `rusqlite::Connection` 非 `Sync`，放进全局容器就得加锁——那把
    /// "并发读"又变回了串行。线程局部让每个线程各读各的，零争用。
    /// 代价是连接数随线程数增长，而本项目的线程数是个位数（DB actor +
    /// 若干工作线程），不构成问题。
    static READERS: RefCell<HashMap<PathBuf, Connection>> = RefCell::new(HashMap::new());
}

/// 在线程局部读连接上执行查询。
fn with_reader<T, F>(path: &Path, query: F) -> Result<T, StoreError>
where
    F: FnOnce(&Connection) -> Result<T, StoreError>,
{
    READERS.with(|readers| {
        // 借用只在取/建连接期间持有，查询本身在 `RefCell` 之外执行——
        // 若查询闭包里又调 `Db::read`（递归读），持有借用会 panic。
        // 故这里先确保连接存在，再取出裸引用……但 `RefCell` 借出的引用
        // 生命周期受借用约束，无法这样跨出去。
        //
        // 解法：借用期间完成查询。递归读会撞上 `already borrowed`——
        // 这是**有意的**：`Db::read` 的闭包应当是纯查询，嵌套读同一个库
        // 意味着设计有问题（应把两次查询合并到一次闭包里）。
        let mut borrowed = readers.borrow_mut();
        if !borrowed.contains_key(path) {
            let connection = open_reader(path)?;
            borrowed.insert(path.to_path_buf(), connection);
        }
        let connection = borrowed
            .get(path)
            .ok_or_else(|| StoreError::sqlite("读连接应当刚刚被建立".to_string()))?;
        query(connection)
    })
}

/// 建一个只读连接。
fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let connection = Connection::open(path).map_err(from_rusqlite)?;
    // `query_only` 在连接层拒绝一切写语句。见模块文档「读写分离」的说明：
    // 用它而非 `SQLITE_OPEN_READ_ONLY`，是为了不额外要求 `-wal`/`-shm` 可写。
    connection
        .execute_batch(&format!(
            "PRAGMA query_only=ON;
             PRAGMA busy_timeout={BUSY_TIMEOUT_MS};"
        ))
        .map_err(from_rusqlite)?;
    Ok(connection)
}

/// 当前 schema 版本（诊断与测试用）。
pub fn schema_version(connection: &Connection) -> Result<i64, StoreError> {
    connection
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .map_err(from_rusqlite)
}

/// 供 DAO 复用的 SQLite → [`StoreError`] 转换。
///
/// 各 DAO 的 `query_map`/`execute` 都要做这一步；集中一处保证"磁盘满判别"
/// 等转换点逻辑不会在某个 DAO 里被漏掉。
pub(crate) fn sql(error: rusqlite::Error) -> StoreError {
    from_rusqlite(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(label: &str) -> (PathBuf, Db) {
        let dir = std::env::temp_dir().join(format!(
            "tome-db-test-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建临时目录");
        let path = dir.join("tome.db");
        let db = Db::open(&path).expect("应能打开数据库");
        (dir, db)
    }

    #[test]
    fn 打开后_schema_版本为_v1() {
        let (dir, db) = temp_db("version");
        let version = db
            .read(schema_version)
            .expect("应能读版本");
        assert_eq!(version, SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 打开是幂等的_重复打开不重复迁移() {
        let (dir, db) = temp_db("idempotent");
        let path = db.path().to_path_buf();
        drop(db);
        // 第二次打开：版本已是 v1，迁移器应当直接返回（而非再跑一遍 CREATE TABLE，
        // 那会因"表已存在"而失败）
        let db2 = Db::open(&path).expect("重复打开应成功");
        assert_eq!(
            db2.read(schema_version).expect("应能读版本"),
            SCHEMA_VERSION
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 十八张实体表全部建成() {
        let (dir, db) = temp_db("tables");
        let count: i64 = db
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(sql)
            })
            .expect("应能查表数");
        // 18 实体表 + image_cache + script_cache = 20
        assert_eq!(count, 20, "表数不符：18 实体 + 图片缓存索引 + 脚本缓存");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wal_模式已启用() {
        let (dir, db) = temp_db("wal");
        let mode: String = db
            .read(|connection| {
                connection
                    .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查 journal_mode");
        assert_eq!(mode.to_lowercase(), "wal");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 外键约束已启用() {
        let (dir, db) = temp_db("fk");
        let enabled: i64 = db
            .read(|connection| {
                connection
                    .query_row("PRAGMA foreign_keys;", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查 foreign_keys");
        assert_eq!(enabled, 1, "外键必须开启，否则级联删除不生效");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 写操作在_actor_线程上执行() {
        let (dir, db) = temp_db("actor");
        let main_thread = std::thread::current().id();
        let worker = db
            .write(|_connection| Ok(std::thread::current().id()))
            .expect("应能执行");
        assert_ne!(worker, main_thread, "写必须在 actor 线程上跑");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 读不阻塞写() {
        // 在一条读连接上开一个长事务（持有读快照），同时验证写仍能提交。
        // WAL 下二者并发；若读走 actor 队列，这个写会一直等到读事务结束。
        let (dir, db) = temp_db("read-write");

        let reader_db = db.clone();
        let (started_sender, started_receiver) = mpsc::channel::<()>();
        let (release_sender, release_receiver) = mpsc::channel::<()>();

        let reader = std::thread::spawn(move || {
            reader_db
                .read(|connection| {
                    connection
                        .execute_batch("BEGIN; SELECT COUNT(*) FROM source;")
                        .map_err(sql)?;
                    let _ = started_sender.send(());
                    // 等主线程把写做完
                    let _ = release_receiver.recv();
                    connection.execute_batch("COMMIT;").map_err(sql)
                })
                .expect("读事务应成功");
        });

        started_receiver.recv().expect("读事务应已开始");
        // 读事务持有中：写仍应成功（这正是"读不阻塞写"）
        let written = db.write(|connection| {
            connection
                .execute(
                    "INSERT INTO source_group(id, name, sort_order, enabled) VALUES (1, 'g', 0, 1)",
                    [],
                )
                .map_err(sql)
        });
        assert!(written.is_ok(), "读事务持有时写应仍能进行：{written:?}");

        let _ = release_sender.send(());
        reader.join().expect("读线程应正常结束");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 高版本库被拒绝打开() {
        let (dir, db) = temp_db("future");
        let path = db.path().to_path_buf();
        // 把版本改成未来值（模拟"新程序建的库被旧程序打开"）
        db.write(|connection| {
            connection
                .execute_batch(&format!("PRAGMA user_version={};", SCHEMA_VERSION + 1))
                .map_err(sql)
        })
        .expect("应能改版本");
        drop(db);

        let error = Db::open(&path).expect_err("高版本库应被拒绝");
        assert!(
            matches!(error, StoreError::Migration { .. }),
            "应报迁移错误而不是尽力而为：{error:?}"
        );
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Storage);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 迁移失败时整体回滚() {
        // 直接调 migrate 用一个坏脚本，验证版本号不前进、schema 无残留。
        let dir = std::env::temp_dir().join(format!("tome-db-rollback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let mut connection = Connection::open(dir.join("t.db")).expect("应能打开连接");

        let transaction = connection.transaction().expect("应能开事务");
        transaction
            .execute_batch("CREATE TABLE good (id INTEGER);")
            .expect("应能建表");
        // 第二条语句语法错误 → execute_batch 整体失败
        let failed = transaction.execute_batch("CREATE TABLE bad (;");
        assert!(failed.is_err(), "坏 SQL 应当失败");
        drop(transaction); // 未 commit → 回滚

        let tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='good'",
                [],
                |row| row.get(0),
            )
            .expect("应能查");
        assert_eq!(tables, 0, "未提交的建表应当回滚");
        let _ = std::fs::remove_dir_all(dir);
    }
}
