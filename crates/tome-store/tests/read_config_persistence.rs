//! T018 验收：`Book.read_config` 的读写接线（FR-013 的 schema 载体 / 裁决 OQ#12→A）
//!
//! ## 本文件覆盖什么
//!
//! 1. **单书覆盖的原子写**：`set_read_config` 只改 `read_config` 一列，
//!    不触碰书籍元信息（唯一性靠 `BookDao` 的既有身份约束）
//! 2. **清空覆盖**：`set_read_config(None)` 让该书回到"跟随全局"
//! 3. **不存在的书报结构化错误**（而非静默成功）
//! 4. **往返**：写进去的 JSON 原样读回来（S2 的 schema 与存储层解耦——
//!    存储层不认识 `ReadingConfig`，只存字符串）
//!
//! ## 为什么这一层不解析 `read_config`
//!
//! 001 契约把 `Book.read_config` 定义为 `Option<String>` 且明文"内容由 S2
//! 排版层解释，此处不透明"。存储层若解析它，就产生了**第二个** schema 定义点
//! ——S2 改字段时存储层要跟着改，而它本不该知道这些字段。
//!
//! 故本文件的断言刻意只涉及"字符串进、字符串出"。schema 的校验在
//! `tome-reader`（`tests/reading_config.rs`），作用域语义（全局默认 vs 单书
//! 覆盖）也在 `tome-reader`（`config::resolve_for_book`）。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_core::{Book, BookOrigin};
use tome_store::dao::book::BookDao;
use tome_store::db::Db;
use tome_store::error::StoreError;

/// 一次测试的临时库。
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
        "tome-read-config-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("应能建目录");
    let db = Db::open(&dir.join("tome.db")).expect("应能开库");
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
    }
}

/// S2 的 schema 示例（与 `tome-reader` 的 `ReadingConfig::to_json` 同形态）。
fn sample_config_json() -> String {
    r#"{"geometry":{"font_size_px":24,"line_height":2.2,"paragraph_mode":{"Gap":{"em":0.4}},"measure_em":30.0,"measure_max_em":42.0,"gutter_px":48,"spread_mode":"ForceSpread","reading_mode":"Paged","chapter_new_page":false},"presentation":{"theme_id":"Obsidian","font_family":"SystemFallback","reduce_motion":true,"paper_noise":false}}"#
        .to_string()
}

// ===========================================================================
// 写 + 读往返
// ===========================================================================

/// 为这本书设置配置后，`by_id` 读回来逐字节相同。
#[test]
fn set_read_config_roundtrips_through_storage() {
    let fixture = fixture("roundtrip");
    let book_id = fixture
        .dao
        .add_book(&sample_book("id-1"))
        .expect("应能加入");

    let json = sample_config_json();
    fixture
        .dao
        .set_read_config(book_id, Some(&json))
        .expect("应能写入 read_config");

    let loaded = fixture
        .dao
        .by_id(book_id)
        .expect("应能查")
        .expect("应有这本书");
    assert_eq!(
        loaded.read_config.as_deref(),
        Some(json.as_str()),
        "read_config 应逐字节往返（存储层不解析、不改写）"
    );
}

/// 未设置时读出 `None`（"跟随全局"的表达）。
#[test]
fn unset_read_config_reads_as_none() {
    let fixture = fixture("unset");
    let book_id = fixture
        .dao
        .add_book(&sample_book("id-1"))
        .expect("应能加入");

    let loaded = fixture.dao.by_id(book_id).expect("应能查").expect("应有");
    assert_eq!(
        loaded.read_config, None,
        "未设置时必须是 None（不得写入一个'默认配置'的副本——那会让'为这本书设置过'变成不可区分）"
    );
}

/// **清空覆盖**：写入后再清空 → 回到 `None`。
///
/// ## 这条为什么必须有
///
/// 用户"取消为这本书单独设置"的唯一表达方式就是清空。若清空不被支持，
/// 用户一旦设置过就再也回不到"跟随全局"——那是一个单向操作，与
/// 裁决 OQ#12→A 的"未设置时回退全局默认"语义矛盾。
#[test]
fn clearing_read_config_restores_follow_global() {
    let fixture = fixture("clear");
    let book_id = fixture
        .dao
        .add_book(&sample_book("id-1"))
        .expect("应能加入");

    fixture
        .dao
        .set_read_config(book_id, Some(&sample_config_json()))
        .expect("应能写入");
    fixture
        .dao
        .set_read_config(book_id, None)
        .expect("应能清空");

    let loaded = fixture.dao.by_id(book_id).expect("应能查").expect("应有");
    assert_eq!(loaded.read_config, None, "清空后应回到 None");
}

/// **只改 `read_config` 一列**：书的其他字段（含身份）不受影响。
///
/// ## 为什么不用 `update_book`
///
/// `update_book` 会写全部 17 列。用一个"读出来→改一个字段→写回去"的
/// 序列来设置配置，在多处并发读取时会**用陈旧值覆盖别人刚写的字段**
/// ——典型的表现是"改了个字号，最新章节标题退回去了"。
///
/// 单列 UPDATE 让这次写入的影响面就是它自己。本测试用"先由另一条路径
/// 写入元信息，再设配置，再核对元信息还在"来钉住这一点。
#[test]
fn set_read_config_does_not_touch_other_columns() {
    let fixture = fixture("isolation");
    let mut book = sample_book("id-1");
    book.name = "原名".to_string();
    book.total_chapters = Some(100);
    // 由另一条路径（加入时的元信息）写入的字段
    book.latest_chapter_title = Some("第99章".to_string());
    let book_id = fixture.dao.add_book(&book).expect("应能加入");

    fixture
        .dao
        .set_read_config(book_id, Some(&sample_config_json()))
        .expect("应能写入 read_config");

    let loaded = fixture.dao.by_id(book_id).expect("应能查").expect("应有");
    assert_eq!(loaded.name, "原名", "书名不得被改动");
    assert_eq!(loaded.total_chapters, Some(100), "总章数不得被改动");
    assert_eq!(
        loaded.latest_chapter_title.as_deref(),
        Some("第99章"),
        "最新章节标题不得被改动（单列更新的核心证据）"
    );
    assert_eq!(loaded.origin_id, "id-1", "身份不得被改动");
    assert!(loaded.read_config.is_some(), "read_config 应已写入");
}

/// 更新不存在的书 → 结构化 `NotFound`（不静默成功）。
#[test]
fn set_read_config_on_missing_book_reports_not_found() {
    let fixture = fixture("missing");
    let error = fixture
        .dao
        .set_read_config(999, Some(&sample_config_json()))
        .expect_err("不存在的书应报错");
    assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
}

/// 清空不存在的书同样报错（"操作成功但什么也没发生"是更糟的契约）。
#[test]
fn clearing_read_config_on_missing_book_reports_not_found() {
    let fixture = fixture("missing-clear");
    let error = fixture
        .dao
        .set_read_config(999, None)
        .expect_err("不存在的书应报错");
    assert!(matches!(error, StoreError::NotFound { .. }), "{error:?}");
}

/// 两本书的配置**互不影响**（单书覆盖的作用域边界）。
#[test]
fn read_config_is_per_book() {
    let fixture = fixture("per-book");
    let first = fixture
        .dao
        .add_book(&sample_book("id-1"))
        .expect("应能加入");
    let second = fixture
        .dao
        .add_book(&sample_book("id-2"))
        .expect("应能加入");

    fixture
        .dao
        .set_read_config(first, Some(&sample_config_json()))
        .expect("应能写入");

    let loaded_first = fixture.dao.by_id(first).expect("查").expect("有");
    let loaded_second = fixture.dao.by_id(second).expect("查").expect("有");
    assert!(loaded_first.read_config.is_some());
    assert_eq!(
        loaded_second.read_config, None,
        "未设置的第二本书不得被连带写入（否则'为这本书设置'会变成'为所有书设置'）"
    );
}

/// **与 `update_book` 的互操作**：`update_book` 写回的 `read_config` 也是通的。
///
/// 两条路径必须一致——若 `set_read_config` 与 `update_book` 对同一列的处理
/// 不同（如一个用 NULL 一个用空串），"未设置"的判定会出现两个含义。
#[test]
fn update_book_and_set_read_config_agree_on_encoding() {
    let fixture = fixture("interop");
    let mut book = sample_book("id-1");
    book.read_config = Some(sample_config_json());
    let book_id = fixture.dao.add_book(&book).expect("应能加入");

    // 经 update_book 写入空（None）
    let mut cleared = fixture.dao.by_id(book_id).expect("查").expect("有");
    cleared.read_config = None;
    fixture.dao.update_book(&cleared).expect("应能更新");

    let after_update = fixture.dao.by_id(book_id).expect("查").expect("有");
    assert_eq!(
        after_update.read_config, None,
        "update_book 写 None 后应读出 None"
    );

    // 经 set_read_config 写入，再经 update_book 读；两条路径的编码须一致
    fixture
        .dao
        .set_read_config(book_id, Some(&sample_config_json()))
        .expect("应能写入");
    let after_set = fixture.dao.by_id(book_id).expect("查").expect("有");
    assert_eq!(after_set.read_config, Some(sample_config_json()));

    // update_book 原样写回（模拟"改了别的字段"）不得丢 read_config
    let mut roundtrip = after_set.clone();
    roundtrip.name = "改名".to_string();
    fixture.dao.update_book(&roundtrip).expect("应能更新");
    let final_state = fixture.dao.by_id(book_id).expect("查").expect("有");
    assert_eq!(
        final_state.read_config,
        Some(sample_config_json()),
        "update_book 不得丢掉已有的 read_config"
    );
}
