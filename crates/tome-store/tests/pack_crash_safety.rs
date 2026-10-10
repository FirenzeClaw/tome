//! [DONE] Pack 崩溃安全的**故障注入**测试（tasks.md T060 / FR-030 / SC-007）
//!
//! # 为什么必须是"真的杀进程"
//!
//! 单元测试里能构造的损坏只有"我自己写坏的"（如往索引里插一行指向不存在
//! 的容器）。而 FR-030 要防的是**真实的异常中断**：进程在"内容已写、
//! 索引未提交"或"内容写了一半"的窗口里被杀死。
//!
//! 那个窗口用 in-process 的手段测不到——`std::process::abort` 会带走整个
//! 测试进程，`catch_unwind` 不覆盖 SIGKILL。故本测试**启动子进程**
//! （同一个测试二进制，用环境变量切换行为），在指定时机从外部 kill 它，
//! 然后在新进程里检查数据。
//!
//! # 场景矩阵
//!
//! | 场景 | 杀在什么时候 | 期望 |
//! |---|---|---|
//! | A | 写完内容、**未提交索引** | 索引无该章（=未缓存），无悬空引用 |
//! | B | 写完内容与索引、继续写下一章时 | 前一章可读，后一章未缓存 |
//! | C | 循环写若干次，随机时刻杀 | 全部索引行都能读出内容 |
//!
//! 共同断言：**重启后不存在指向缺失内容的引用**（SC-007 的原文）。
//!
//! # 为什么子进程场景用"写完立刻自杀"而不是"睡一会等外部 kill"
//!
//! `std::process::abort()` 在写入之后**立即**终止，窗口由代码位置精确决定，
//! 不依赖调度时序。用外部 kill 则需要 sleep 竞态——在慢机器上会变成
//! flaky 测试，而 flaky 的崩溃安全测试等于没有（它绿的时候你不知道是真绿）。

//! # 为什么本文件带 `#![expect(...)]`
//!
//! `.clippy.toml` 的 `allow-unwrap/expect/panic-in-tests` 只覆盖 `#[test]`
//! **函数体**，不覆盖它们调用的辅助函数。本文件的绝大部分代码在辅助函数里
//! （夹具 `setup`、子进程驱动 `run_child`、断言 `assert_no_dangling_references`），
//! 故需要文件级豁免。
//!
//! 这些 `expect` 的表达力在这里是**必要**的：夹具失败必须让测试立刻红，
//! 而"夹具写不出数据"与"被测代码坏了"是两件事，用 `expect("应能…")`
//! 的消息把前者标出来，读失败输出时能直接判断是哪一类。

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "集成测试的夹具与断言函数——失败即 panic 是测试框架报告失败的唯一机制（AGENTS.md 禁的是生产路径上的 panic）"
)]

use std::path::Path;
use std::process::Command;

use tome_store::db::{sql, Db};
use tome_store::pack::PackStore;

/// 子进程场景的环境变量名。
const SCENARIO_ENV: &str = "TOME_CRASH_TEST_SCENARIO";
/// 子进程数据目录的环境变量名。
const DATA_DIR_ENV: &str = "TOME_CRASH_TEST_DIR";

/// 建立测试用的书与章节，返回 `(book_id, chapter_ids)`。
fn setup(path: &Path, chapters: usize) -> (Db, i64, Vec<i64>) {
    let db = Db::open(path).expect("应能开库");
    let book_id: i64 = db
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO book(origin, origin_id, name, added_at) VALUES (0, 'x', '书', 0)",
                    [],
                )
                .map_err(sql)?;
            Ok(connection.last_insert_rowid())
        })
        .expect("应能建书");

    let chapter_ids = db
        .write(move |connection| {
            let transaction = connection.transaction().map_err(sql)?;
            let mut ids = Vec::new();
            {
                let mut statement = transaction
                    .prepare_cached(
                        "INSERT INTO chapter(book_id, index_in_book, title) VALUES (?1, ?2, ?3)",
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
                    ids.push(transaction.last_insert_rowid());
                }
            }
            transaction.commit().map_err(sql)?;
            Ok(ids)
        })
        .expect("应能建章节");

    (db, book_id, chapter_ids)
}

/// 校验"不存在指向缺失内容的引用"（SC-007 的核心断言）。
///
/// 对每一条索引行，都必须能读出内容。任何一条读不出来就是悬空引用。
fn assert_no_dangling_references(db: &Db, root: &Path) {
    let store = PackStore::new(root, db.clone());
    let indexed = db
        .read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT chapter_id FROM chapter_cache ORDER BY chapter_id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| row.get::<_, i64>(0))
                .map_err(sql)?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(sql)?);
            }
            Ok(ids)
        })
        .expect("应能读索引");

    for chapter_id in &indexed {
        let content = store.read_chapter(*chapter_id).unwrap_or_else(|error| {
            panic!("崩溃后出现悬空引用：chapter#{chapter_id} 在索引里但读不出内容（{error}）")
        });
        assert!(
            content.is_some(),
            "索引里的 chapter#{chapter_id} 应能读出内容"
        );
    }

    // 反向也要成立：`chapter.is_cached` 标记为 1 的章节，索引里必须真的有
    let flagged: Vec<i64> = db
        .read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT id FROM chapter WHERE is_cached=1 ORDER BY id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| row.get::<_, i64>(0))
                .map_err(sql)?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(sql)?);
            }
            Ok(ids)
        })
        .expect("应能查标记");
    for chapter_id in flagged {
        assert!(
            indexed.contains(&chapter_id),
            "chapter#{chapter_id} 标了 is_cached 但索引里没有——目录页会显示「已缓存」却读不出内容"
        );
    }
}

// ---------------------------------------------------------------------------
// 子进程侧：由环境变量决定行为
// ---------------------------------------------------------------------------

/// 子进程的入口。被主测试经 `cargo test` 再次调用本二进制时触发。
///
/// 用 `#[test]` 而不是 `main`：集成测试二进制没有 `main` 可挂号，而
/// `cargo test` 会跑全部测试。故用一个"只在环境变量存在时才做事"的测试，
/// 它在正常情况下是空操作。
#[test]
fn 崩溃注入子进程() {
    let Ok(scenario) = std::env::var(SCENARIO_ENV) else {
        // 正常运行时（主测试进程）什么都不做
        return;
    };
    let data_dir = std::env::var(DATA_DIR_ENV).expect("子进程需要数据目录");
    let root = Path::new(&data_dir).join("cache").join("packs");
    let db = Db::open(&Path::new(&data_dir).join("tome.db")).expect("应能开库");
    let store = PackStore::new(&root, db.clone());

    let (_db, _book_id, chapter_ids) = setup_lookup(&db);
    let content = "正文内容".repeat(200).into_bytes();

    match scenario.as_str() {
        // 场景 A：目标是在"写完内容、提交索引之前"死
        //
        // 用真实的包内 API 无法精确卡在那里（write_chapter 是一条链），故这里
        // 手工重现那两个步骤，并在中间 abort——**这正是崩溃窗口的定义**，
        // 而不是对内部实现的模拟。
        "content-without-index" => {
            let pack_dir = root.join(format!("book-{_book_id}"));
            std::fs::create_dir_all(&pack_dir).expect("应能建目录");
            let pack_path = pack_dir.join("1.pk");
            write_pack_file_with_header(&pack_path, 1, &content);
            // 内容已在磁盘上（含 fsync），**索引未提交** → 立即终止
            std::process::abort();
        }
        // 场景 B：写完第一章（含索引）后，在写第二章的过程中死
        "after-first-chapter" => {
            store
                .write_chapter(chapter_ids[0], &content)
                .expect("第一章应能写");
            let pack_dir = root.join(format!("book-{_book_id}"));
            let pack_path = pack_dir.join("1.pk");
            // 第二章：内容追加进容器，但索引不提交
            append_record_without_index(&pack_path, &content);
            std::process::abort();
        }
        // 场景 C：连续写若干章，每写几章就 abort（模拟"写到一半被杀"）
        "loop" => {
            for (index, chapter_id) in chapter_ids.iter().enumerate() {
                store.write_chapter(*chapter_id, &content).expect("应能写");
                if index == 7 {
                    // 第 8 章已提交索引，第 9 章的内容写了一半
                    let pack_dir = root.join(format!("book-{_book_id}"));
                    let pack_path = pack_dir.join("1.pk");
                    append_record_without_index(&pack_path, &content);
                    std::process::abort();
                }
            }
        }
        other => panic!("未知场景 {other}"),
    }
}

/// 在子进程里查出书籍与章节 id（不新建）。
fn setup_lookup(db: &Db) -> (Db, i64, Vec<i64>) {
    let book_id: i64 = db
        .read(|connection| {
            connection
                .query_row("SELECT id FROM book LIMIT 1", [], |row| row.get(0))
                .map_err(sql)
        })
        .expect("应有书");
    let chapter_ids: Vec<i64> = db
        .read(move |connection| {
            let mut statement = connection
                .prepare_cached("SELECT id FROM chapter WHERE book_id=?1 ORDER BY index_in_book")
                .map_err(sql)?;
            let rows = statement
                .query_map([book_id], |row| row.get::<_, i64>(0))
                .map_err(sql)?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(sql)?);
            }
            Ok(ids)
        })
        .expect("应有章节");
    (db.clone(), book_id, chapter_ids)
}

/// 写一个只含头部与一条内容的容器文件（含 fsync），不碰索引。
fn write_pack_file_with_header(path: &Path, pack_id: u32, content: &[u8]) {
    use std::io::Write;
    let mut header = Vec::new();
    header.extend_from_slice(&tome_store::pack::PACK_MAGIC);
    header.extend_from_slice(&tome_store::pack::PACK_VERSION.to_le_bytes());
    header.extend_from_slice(&pack_id.to_le_bytes());
    header.extend_from_slice(&(content.len() as u32).to_le_bytes());
    header.extend_from_slice(content);

    let mut file = std::fs::File::create(path).expect("应能建文件");
    file.write_all(&header).expect("应能写");
    file.sync_data().expect("应能同步");
}

/// 往已有容器追加一条记录（含 fsync），不碰索引。
fn append_record_without_index(path: &Path, content: &[u8]) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("应能打开容器");
    file.write_all(&(content.len() as u32).to_le_bytes())
        .expect("应能写长度");
    file.write_all(content).expect("应能写内容");
    file.sync_data().expect("应能同步");
}

// ---------------------------------------------------------------------------
// 主进程侧：跑场景、杀进程、检查
// ---------------------------------------------------------------------------

/// 跑一次崩溃场景，返回数据目录。
fn run_child(scenario: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tome-crash-{scenario}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("应能建目录");

    // 先建好书与章节（子进程只做写入，不负责初始化）
    let chapters = 16;
    let (_db, _book_id, _chapter_ids) = setup(&dir.join("tome.db"), chapters);
    drop(_db);

    // 启动子进程：用同一个测试二进制，靠环境变量切换行为。
    //
    // `--exact` 限定只跑那一个测试；`--nocapture` 让 panic 信息可见。
    // 子进程会 abort()，故退出码是非零的——那是**预期**，不是失败。
    let exe = std::env::current_exe().expect("应能取到测试二进制路径");
    let output = Command::new(exe)
        .args([
            "崩溃注入子进程",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SCENARIO_ENV, scenario)
        .env(DATA_DIR_ENV, &dir)
        .output()
        .expect("应能启动子进程");

    // abort 在 Unix 是 SIGABRT（signal 6），在 Windows 是 STATUS_ABORT 映射的
    // 退出码。两者都表现为非零退出，但**必须与"子进程 panic"区分开**——
    // 一个因为代码 bug 而 panic 的子进程同样是非零退出，那会让这些测试
    // 在"崩溃注入根本没跑到"的情况下假绿。
    //
    // 判据：panic 的退出码是 Rust 固定的 101（Unix/Windows 一致），
    // abort 则不是。故这里显式排除 101。
    assert!(
        !output.status.success(),
        "子进程应当被 abort 中断（场景 {scenario}）"
    );
    assert_ne!(
        output.status.code(),
        Some(101),
        "子进程不是 abort 而是 panic 了——崩溃注入没跑到目标位置。输出：{}",
        String::from_utf8_lossy(&output.stdout)
    );

    // 顺带确认它真的跑到了场景分支（否则可能是环境变量没传进去）
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("未知场景"), "子进程应识别场景：{stdout}");

    dir
}

#[test]
fn 内容已写但索引未提交时不产生悬空引用() {
    // 场景 A：崩溃窗口 = "内容在磁盘上，索引未提交"
    let dir = run_child("content-without-index");

    let db = Db::open(&dir.join("tome.db")).expect("重启应能开库");
    let root = dir.join("cache").join("packs");

    // ① 核心断言：无悬空引用（SC-007）
    assert_no_dangling_references(&db, &root);

    // ② 该章应表现为**未缓存**（索引里没有它）
    let store = PackStore::new(&root, db.clone());
    assert_eq!(
        store.entry_count().expect("应能数"),
        0,
        "索引未提交时不该有任何缓存条目"
    );

    // ③ 文件在磁盘上成为孤儿，可被回收（而非永久占空间）
    assert!(
        store.orphan_bytes().expect("应能算") > 0,
        "崩溃遗留的内容应被识别为孤儿占用"
    );
    // 实测记录（供汇报引用）
    eprintln!(
        "[崩溃注入实测·场景A] 数据目录={} 索引条目={} 磁盘占用={} 孤儿占用={} 悬空引用=0",
        dir.display(),
        store.entry_count().expect("应能数"),
        store.disk_bytes().expect("应能算"),
        store.orphan_bytes().expect("应能算"),
    );

    let removed = store.evict_orphan_packs().expect("应能回收");
    assert_eq!(removed, 1, "应回收一个孤儿容器");
    assert_eq!(store.orphan_bytes().expect("应能算"), 0);
    eprintln!(
        "[崩溃注入实测·场景A] 回收孤儿后占用={}",
        store.disk_bytes().expect("应能算")
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn 中途崩溃后已提交的章节仍可读() {
    // 场景 B：第一章已完整提交（含索引），第二章的内容写了一半就被杀
    let dir = run_child("after-first-chapter");

    let db = Db::open(&dir.join("tome.db")).expect("重启应能开库");
    let root = dir.join("cache").join("packs");
    let store = PackStore::new(&root, db.clone());

    // ① 核心断言：无悬空引用（SC-007）
    assert_no_dangling_references(&db, &root);

    // ② 第一章应完整可读
    let chapters: Vec<i64> = db
        .read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT id FROM chapter ORDER BY index_in_book")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| row.get::<_, i64>(0))
                .map_err(sql)?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(sql)?);
            }
            Ok(ids)
        })
        .expect("应能查章节");
    let content = store
        .read_chapter(chapters[0])
        .expect("第一章应可读")
        .expect("第一章应有内容");
    assert_eq!(content, "正文内容".repeat(200).into_bytes());

    // ③ 第二章未被索引（它只有没提交的一半内容）
    assert_eq!(
        store.location_of(chapters[1]).expect("应能查"),
        None,
        "未提交索引的章节不该出现在缓存里"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn 连续写入中崩溃已提交的部分全部可读() {
    // 场景 C：写 8 章（全部提交）后，第 9 章的半截内容留下
    let dir = run_child("loop");

    let db = Db::open(&dir.join("tome.db")).expect("重启应能开库");
    let root = dir.join("cache").join("packs");
    let store = PackStore::new(&root, db.clone());

    // ① 核心断言：无悬空引用（SC-007）
    assert_no_dangling_references(&db, &root);

    // ② 已提交的 8 章应全是完整内容
    assert_eq!(
        store.entry_count().expect("应能数"),
        8,
        "应恰有 8 章被完整提交"
    );
    let chapters: Vec<i64> = db
        .read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT chapter_id FROM chapter_cache ORDER BY chapter_id")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| row.get::<_, i64>(0))
                .map_err(sql)?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(sql)?);
            }
            Ok(ids)
        })
        .expect("应能查");
    for chapter_id in chapters {
        let content = store
            .read_chapter(chapter_id)
            .expect("应可读")
            .expect("应有内容");
        assert_eq!(
            content.len(),
            "正文内容".repeat(200).len(),
            "chapter#{chapter_id} 的内容应完整（不是半截）"
        );
    }

    // ③ 崩溃后**仍可继续写**（这是"数据 100% 可读"的实践含义：
    //    恢复后系统能照常使用，而不是进入某种"损坏模式"）
    let store2 = PackStore::new(&root, db.clone());
    let chapters_all: Vec<i64> = db
        .read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT id FROM chapter ORDER BY index_in_book")
                .map_err(sql)?;
            let rows = statement
                .query_map([], |row| row.get::<_, i64>(0))
                .map_err(sql)?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(sql)?);
            }
            Ok(ids)
        })
        .expect("应能查章节");
    let ninth = chapters_all[8];
    store2
        .write_chapter(ninth, "恢复后新写的内容".as_bytes())
        .expect("崩溃恢复后应能继续写");
    assert_eq!(
        store2.read_chapter(ninth).expect("应可读"),
        Some("恢复后新写的内容".as_bytes().to_vec()),
        "恢复后写入的内容应完整可读"
    );
    // 再确认一次没有引入悬空引用
    assert_no_dangling_references(&db, &root);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn 崩溃后清缓存不影响个人数据() {
    // FR-031 / SC-015 在崩溃恢复路径上的形态：崩溃遗留的孤儿 + 清缓存
    // 之后，个人数据必须一点也不少
    let dir = run_child("loop");
    let db = Db::open(&dir.join("tome.db")).expect("重启应能开库");
    let root = dir.join("cache").join("packs");
    let store = PackStore::new(&root, db.clone());

    // 造一点个人数据
    db.write(|connection| {
        connection
            .execute(
                "INSERT INTO read_progress(book_id, chapter_index, char_offset, anchor_text, updated_at)
                 VALUES (1, 3, 500, '锚点', 0)",
                [],
            )
            .map_err(sql)?;
        connection
            .execute(
                "INSERT INTO bookmark(book_id, chapter_index, char_offset, note, created_at)
                 VALUES (1, 3, 500, '备注', 0)",
                [],
            )
            .map_err(sql)?;
        connection
            .execute(
                "INSERT INTO source(id, name, enabled, created_at) VALUES ('s', '源', 1, 0)",
                [],
            )
            .map_err(sql)?;
        Ok(())
    })
    .expect("应能写个人数据");

    store.clear_all().expect("应能清缓存");

    for table in ["book", "chapter", "read_progress", "bookmark", "source"] {
        let count: i64 = db
            .read(|connection| {
                connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .map_err(sql)
            })
            .expect("应能查");
        assert!(count > 0, "清缓存后 `{table}` 必须仍有数据（SC-015）");
    }
    assert_eq!(store.entry_count().expect("应能数"), 0, "缓存应清空");

    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// 外部 kill 变体
// ---------------------------------------------------------------------------
//
// 上面的场景用 `std::process::abort()` 在**代码位置**上精确卡住崩溃窗口，
// 不依赖调度时序（时序竞态会让这类测试变成 flaky，而 flaky 的崩溃安全
// 测试等于没有——它绿的时候你不知道是真绿）。
//
// 但 tasks.md 的原文是"**写入中 kill**"。为覆盖那个字面形态，这里再补一个
// 真正的外部终止：**父进程按固定节奏反复写，父进程自己到点就杀**。
//
// ## 为什么不是"子进程 sleep 等父进程 kill"
//
// 那个形态必然要在"子进程睡多久"与"父进程何时杀"之间赌一个时序。赌宽了
// 测试慢，赌窄了杀在写入之外、测了个寂寞。故这里反过来：**让写入无限循环，
// 父进程固定时刻杀**——无论杀在哪一刻，都必然落在"某一章的写入过程中"。

/// 一个只做写入的循环子进程（由环境变量触发）。
#[test]
fn 崩溃注入循环写入子进程() {
    let Ok(data_dir) = std::env::var("TOME_CRASH_LOOP_DIR") else {
        return;
    };
    let root = Path::new(&data_dir).join("cache").join("packs");
    let db = Db::open(&Path::new(&data_dir).join("tome.db")).expect("应能开库");
    let store = PackStore::new(&root, db.clone());
    let (_db2, _book_id, chapter_ids) = setup_lookup(&db);
    let content = "正文内容".repeat(200).into_bytes();

    // 无限循环地写：父进程会在某一刻杀掉本进程，那时必然在某章的写入中
    let mut round = 0usize;
    loop {
        for chapter_id in &chapter_ids {
            store.write_chapter(*chapter_id, &content).expect("应能写");
        }
        round += 1;
        // 防御性上限：万一父进程没能杀掉，自己退出而不是挂住整个测试
        if round > 1000 {
            return;
        }
    }
}

#[test]
fn 写入中外部终止后重启无悬空引用() {
    // 这一条对应 tasks.md 的字面要求："**写入中 kill**，重启无悬空引用"
    use std::time::Duration;

    let dir = std::env::temp_dir().join(format!(
        "tome-crash-extkill-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("应能建目录");
    // 先建好书与章节
    let (db, _book_id, _chapter_ids) = setup(&dir.join("tome.db"), 32);
    drop(db);

    let exe = std::env::current_exe().expect("应能取到测试二进制路径");
    let mut child = Command::new(exe)
        .args([
            "崩溃注入循环写入子进程",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("TOME_CRASH_LOOP_DIR", &dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("应能启动子进程");

    // 让它写一会儿再杀——固定延时而非"看它写了多少"：后者要读它的输出，
    // 而那会让父进程与子进程耦合在一条管道上（子进程被缓冲阻塞时反而不写了）
    std::thread::sleep(Duration::from_millis(300));
    child.kill().expect("应能杀掉子进程");
    // 等它真正退出，否则接下来打开的库可能还被它持有
    let _ = child.wait();

    // 重启并检查：SC-007 的原文是"数据 100% 可读且不存在指向缺失内容的引用"
    let db = Db::open(&dir.join("tome.db")).expect("重启应能开库");
    let root = dir.join("cache").join("packs");
    assert_no_dangling_references(&db, &root);

    let store = PackStore::new(&root, db.clone());
    eprintln!(
        "[崩溃注入实测·外部 kill] 数据目录={} 已提交缓存条目={} 磁盘占用={} 孤儿占用={} 悬空引用=0",
        dir.display(),
        store.entry_count().expect("应能数"),
        store.disk_bytes().expect("应能算"),
        store.orphan_bytes().expect("应能算"),
    );

    // 崩溃后系统仍可用：能继续写、能读回
    let (_db2, _book_id, chapter_ids) = setup_lookup(&db);
    store
        .write_chapter(chapter_ids[0], "恢复后的内容".as_bytes())
        .expect("恢复后应能继续写");
    assert_eq!(
        store.read_chapter(chapter_ids[0]).expect("应可读"),
        Some("恢复后的内容".as_bytes().to_vec())
    );
    // 再确认一次（含刚写的那条）
    assert_no_dangling_references(&db, &root);

    let _ = std::fs::remove_dir_all(dir);
}
