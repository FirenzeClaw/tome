//! rusqlite 一次性写入读取验证（T006 验证项）
//!
//! 只验证一件事：**SQLite 能在这个三平台目标上实际工作**，且 `bundled` 特性
//! 让 C 源码随 crate 一起编译（无系统 libsqlite3 依赖）。不验证 schema 设计——
//! 那是 S1 的 `tome-store` 的事。

use rusqlite::{Connection, Result as SqliteResult};

/// 建表 → 插入 → 读回 → 断言，全程落在临时目录的一个文件上。
///
/// 用**文件**而非 `:memory:`：`bundled` 的编译正确性已由编译本身证明，
/// 但文件路径还额外覆盖了"平台文件系统 + SQLite VFS"这一层。
pub fn verify_once() -> Result<String, String> {
    let database_path = super::spike_temp_dir()?.join("spike.sqlite");

    // 覆盖式重建：spike 每次运行都应是干净状态，否则上一轮残留会让
    // "写入成功"的断言变得不可信（可能读到的就是上次的数据）。
    if database_path.exists() {
        std::fs::remove_file(&database_path)
            .map_err(|error| format!("清理旧库 {database_path:?} 失败：{error}"))?;
    }

    let connection = Connection::open(&database_path)
        .map_err(|error| format!("打开 {database_path:?} 失败：{error}"))?;

    let round_trip =
        write_and_read(&connection).map_err(|error| format!("SQLite 读写失败：{error}"))?;

    let sqlite_version = rusqlite::version().to_owned();
    Ok(format!(
        "SQLite {sqlite_version}，写读往返值 = {round_trip}，库位于 {}",
        database_path.display()
    ))
}

/// 执行一轮真实的写读往返，返回读回的值供调用方断言。
fn write_and_read(connection: &Connection) -> SqliteResult<String> {
    connection.execute_batch(
        "CREATE TABLE spike_probe (id INTEGER PRIMARY KEY, payload TEXT NOT NULL);",
    )?;

    // 用参数绑定而非字符串拼接：即使 spike 也保持与生产一致的写法，
    // 免得这段代码日后被当作范例复制走。
    connection.execute(
        "INSERT INTO spike_probe (id, payload) VALUES (?1, ?2)",
        rusqlite::params![1_i64, "tome-s0-roundtrip"],
    )?;

    connection.query_row(
        "SELECT payload FROM spike_probe WHERE id = ?1",
        rusqlite::params![1_i64],
        |row| row.get::<_, String>(0),
    )
}
