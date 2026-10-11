//! T010 验收：`BookRepository` 接缝的**生产实现**（`SqliteBookRepository`）
//!
//! # 这个文件守什么
//!
//! `crates/tome-core/tests/seam_contract.rs` 用假体（`FakeRepository`）证明了
//! trait **可被实现**；本文件用真实现 + 真 SQLite + 真文件系统验证同一组判据
//! 在生产路径上成立。两者缺一不可：假体全绿而生产实现没接线，是接缝类改动
//! 最典型的失败形态——trait 编译通过，功能却一处都没接上。
//!
//! 判据形态（书架增删查、进度往返、章节往返）刻意与假体契约一致：
//!
//! | 判据 | 来源 |
//! |---|---|
//! | 书架增删查 | `seam_contract.rs` 的 `bookshelf`/`add_book`/`remove_book` |
//! | 进度往返 | 同一接缝的 `get_progress`/`save_progress`（FR-027 锚点三要素） |
//! | 书源导入三渠道 | 同一接缝的 `import_sources`（FR-001） |
//! | 缓存可独立清理 | 同一接缝的 `clear_cache`（SC-017） |
//!
//! # 为什么对象类型写成 `Box<dyn BookRepository>`
//!
//! 接缝的消费方（S3 界面、CLI）拿到的是 trait 对象，不是具体类型
//! （AGENTS.md 架构铁律："只依赖接缝 trait，永不直接构造实现 crate 的内部
//! 类型"）。用 trait 对象持有生产实现，顺带钉住"该方法集可对象化"。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::sync::Arc;

use tome_core::seam::{BookRepository, CacheKind, SourceImportChannel};
use tome_core::{Book, BookOrigin, Chapter, ErrorKind, ReadProgress, Source, SourceKind};
use tome_store::book_repository::SourceImporter;
use tome_store::fts::CachedTextIndex;
use tome_store::{DataDir, Db, PackStore, SqliteBookRepository};

/// 一次测试的临时数据目录 + 库 + 接缝对象。
struct Fixture {
    /// 必须持有到测试结束：`TempDir` 一 drop，库文件与缓存就没了。
    /// 名字带下划线前缀是因为它只作生存期守卫，测试里不读它。
    _dir: tempfile::TempDir,
    data_dir: DataDir,
    db: Db,
    repository: Box<dyn BookRepository>,
}

fn fixture() -> Fixture {
    fixture_with_importer(None)
}

/// 搭夹具：临时数据目录 → `DataDir` → `Db` → 接缝对象。
///
/// 数据目录走 `DataDir::resolve`（**真的写一次探测文件**，见其文档），
/// 故这里不会因为"看着可写其实不可写"而在后续写入时炸掉。
fn fixture_with_importer(importer: Option<Arc<dyn SourceImporter>>) -> Fixture {
    let dir = tempfile::tempdir().expect("应能建临时目录");
    let resolved = DataDir::resolve(Some(dir.path()), None, None);
    let data_dir = DataDir::from_resolved(&resolved).expect("临时目录应可用作数据目录");
    let db = Db::open(&data_dir.database_path()).expect("应能开库");

    let mut repository = SqliteBookRepository::new(db.clone(), &data_dir);
    if let Some(importer) = importer {
        repository = repository.with_source_importer(importer);
    }

    Fixture {
        _dir: dir,
        data_dir,
        db,
        repository: Box::new(repository),
    }
}

fn sample_book(origin_id: &str) -> Book {
    Book {
        id: 0,
        origin: BookOrigin::Network,
        origin_id: origin_id.to_string(),
        name: "斗破苍穹".to_string(),
        author: Some("天蚕土豆".to_string()),
        cover_url: Some("https://a.com/c.jpg".to_string()),
        intro: Some("简介".to_string()),
        kind: Some("玄幻".to_string()),
        book_group: None,
        read_config: None,
        toc_url: Some("https://a.com/toc".to_string()),
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
        url: Some(format!("https://a.com/c/{index}")),
        byte_offset: None,
        epub_entry: None,
        is_cached: false,
        is_read: false,
        word_count: Some(3000),
    }
}

fn sample_source(id: &str, name: &str) -> Source {
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
        rule_search: Some(r#"{"bookList":"@css:li"}"#.to_string()),
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

/// 内存假体：把构造时给的源原样返回，不解析渠道内容。
///
/// 它同时钉住"注入点真的被用上了"——渠道文本是随便写的，能落库就说明
/// `import_sources` 走的是注入进来的这条路，而不是自己硬解析。
struct FakeImporter {
    sources: Vec<Source>,
}

impl SourceImporter for FakeImporter {
    fn load(&self, _channel: &SourceImportChannel) -> Result<Vec<Source>, ErrorKind> {
        Ok(self.sources.clone())
    }
}

/// 把一章落进 Pack 缓存并建好搜索索引（模拟"这一章已被读过"）。
///
/// 生产路径由管线做这三件事（内容 → 缓存索引 → FTS），此处手工复现，
/// 为的是让 `search_in_book` / `remove_book` 有真实的可搜内容。
fn seed_cached_chapter(fixture: &Fixture, book_id: i64, title: &str, content: &str) -> i64 {
    fixture
        .repository
        .save_chapters(book_id, &[sample_chapter(0, title)])
        .expect("应能写章节");
    let chapter_id = fixture.repository.chapters(book_id).expect("应能查")[0].id;

    let pack = Arc::new(PackStore::new(
        &fixture.data_dir.packs_dir(),
        fixture.db.clone(),
    ));
    pack.write_chapter(chapter_id, content.as_bytes())
        .expect("应能写缓存");
    let index = CachedTextIndex::with_pack(fixture.db.clone(), Arc::clone(&pack));
    index.ensure_schema().expect("应能建索引表");
    index
        .index_chapter(chapter_id, title, content)
        .expect("应能建索引");

    chapter_id
}

// ===========================================================================
// 书架
// ===========================================================================

#[test]
fn 空库的书架为空() {
    let fixture = fixture();
    assert!(
        fixture.repository.bookshelf().expect("应能查").is_empty(),
        "空库应返回空书架（不是错误）"
    );
}

#[test]
fn 加入书架返回主键且字段往返一致() {
    let fixture = fixture();
    let book = sample_book("https://a.com|https://a.com/book/1");

    let book_id = fixture.repository.add_book(&book).expect("应能加入");
    assert!(book_id > 0, "应返回分配好的主键，实际 {book_id}");

    let shelf = fixture.repository.bookshelf().expect("应能查");
    assert_eq!(shelf.len(), 1);
    assert_eq!(shelf[0].id, book_id);
    assert_eq!(shelf[0].name, book.name);
    assert_eq!(shelf[0].author, book.author);
    assert_eq!(shelf[0].origin, BookOrigin::Network);
    assert_eq!(shelf[0].origin_id, book.origin_id);
    assert_eq!(shelf[0].total_chapters, book.total_chapters);
}

#[test]
fn 移除后书架不再含它() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");

    fixture.repository.remove_book(book_id).expect("应能移除");

    assert!(fixture.repository.bookshelf().expect("应能查").is_empty());
}

/// FR-004：移除后其**缓存内容**一并清理（不只是库里的行）。
#[test]
fn 移除书籍连带清理该书缓存与搜索索引() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");
    seed_cached_chapter(&fixture, book_id, "第一章", "前面五个字目标在后面");

    let pack = Arc::new(PackStore::new(
        &fixture.data_dir.packs_dir(),
        fixture.db.clone(),
    ));
    let index = CachedTextIndex::with_pack(fixture.db.clone(), Arc::clone(&pack));
    assert!(
        pack.usage_bytes().expect("应能算") > 0,
        "前提：缓存里确有内容"
    );
    assert_eq!(index.indexed_count().expect("应能计数"), 1);

    fixture.repository.remove_book(book_id).expect("应能移除");

    assert!(fixture.repository.bookshelf().expect("应能查").is_empty());
    assert_eq!(
        pack.usage_bytes().expect("应能算"),
        0,
        "缓存索引行应随移除清掉（残留即是「删了却没省空间」）"
    );
    assert_eq!(
        index.indexed_count().expect("应能计数"),
        0,
        "搜索索引条目应一并清掉（虚拟表没有外键，级联管不着它）"
    );
}

// ===========================================================================
// 进度与章节
// ===========================================================================

#[test]
fn 未读过的书进度为空() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");

    assert_eq!(
        fixture.repository.get_progress(book_id).expect("应能读"),
        None,
        "没开始读是正常状态，不是错误"
    );
}

/// FR-027：锚点三要素（章 + 字符偏移 + 锚点文本）逐字段往返。
#[test]
fn 进度往返逐字段相等() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");

    let progress = ReadProgress {
        book_id,
        chapter_index: 12,
        char_offset: 3456,
        anchor_text: Some("他站在山巅，风吹起衣角".to_string()),
        updated_at: 1_700_000_123,
    };
    fixture
        .repository
        .save_progress(&progress)
        .expect("应能写进度");

    let loaded = fixture
        .repository
        .get_progress(book_id)
        .expect("应能读")
        .expect("应有进度");
    assert_eq!(
        loaded, progress,
        "chapter_index / char_offset / anchor_text / updated_at 应逐字段相等"
    );
}

#[test]
fn 章节往返且次序正确() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");

    let chapters = vec![sample_chapter(0, "第1章"), sample_chapter(1, "第2章")];
    fixture
        .repository
        .save_chapters(book_id, &chapters)
        .expect("应能批量写章节");

    let loaded = fixture.repository.chapters(book_id).expect("应能查");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].index_in_book, 0);
    assert_eq!(loaded[0].title, "第1章");
    assert_eq!(loaded[1].index_in_book, 1);
    assert_eq!(loaded[1].title, "第2章");
    assert_eq!(loaded[1].url.as_deref(), Some("https://a.com/c/1"));
    assert_eq!(loaded[0].word_count, Some(3000));
}

// ===========================================================================
// 书源导入（FR-001 三渠道）
// ===========================================================================

/// 未注入导入器时**如实**返回结构化能力错误，而不是"导入 0 条"的假成功。
#[test]
fn 未注入导入器时书源导入返回结构化能力错误() {
    let fixture = fixture();
    let channel = SourceImportChannel::PastedText(r#"[{"bookSourceUrl":"x"}]"#.to_string());

    let error = fixture
        .repository
        .import_sources(&channel)
        .expect_err("未注入导入器应返回能力错误");

    match &error {
        ErrorKind::UnsupportedCapability { capability } => assert_eq!(
            capability.capability_id(),
            "sourceImport",
            "能力错误必须带上可编程判别的标识（FR-006）"
        ),
        other => panic!("应是 UnsupportedCapability，实际 {other:?}"),
    }
    assert_eq!(error.message_id(), "error.unsupported_capability");
    assert!(
        fixture.repository.sources().expect("应能查").is_empty(),
        "失败时不得留下任何书源"
    );
}

/// 注入导入器后三渠道都走通，且源真的落库。
#[test]
fn 注入导入器后书源渠道导入落库() {
    let importer: Arc<dyn SourceImporter> = Arc::new(FakeImporter {
        sources: vec![sample_source("https://a.com", "源A")],
    });
    let fixture = fixture_with_importer(Some(importer));

    let report = fixture
        .repository
        .import_sources(&SourceImportChannel::FileText(
            "随便写的渠道文本".to_string(),
        ))
        .expect("注入后应能导入");
    assert_eq!(report.added, 1);
    assert_eq!(report.updated, 0);
    assert_eq!(report.skipped, 0);
    assert!(report.failures.is_empty());

    let sources = fixture.repository.sources().expect("应能查");
    assert_eq!(sources.len(), 1, "注入点确实被用上了");
    assert_eq!(sources[0].id, "https://a.com");
    assert_eq!(sources[0].name, "源A");
    assert_eq!(
        fixture
            .repository
            .source_by_id("https://a.com")
            .expect("应能查")
            .expect("应有")
            .name,
        "源A"
    );

    // 三种渠道都交给同一个接缝——渠道不同不该改变落库语义
    for channel in [
        SourceImportChannel::Url("https://a.com/sources.json".to_string()),
        SourceImportChannel::PastedText("[]".to_string()),
    ] {
        let again = fixture
            .repository
            .import_sources(&channel)
            .expect("三渠道都应能导入");
        assert_eq!(again.added, 0, "同 id 不该重复新增");
        assert_eq!(again.updated, 1, "同 id 按更新处理");
    }
    assert_eq!(fixture.repository.sources().expect("应能查").len(), 1);
}

/// `import_parsed_sources` 是"手里已有 `Source`"的快路径，**不需要**注入点。
#[test]
fn 已解析书源不依赖导入器即可落库() {
    let fixture = fixture();

    let report = fixture
        .repository
        .import_parsed_sources(&[sample_source("https://b.com", "源B")])
        .expect("应能落库");
    assert_eq!(report.added, 1);
    assert_eq!(report.updated, 0);
    assert!(report.failures.is_empty());

    let sources = fixture.repository.sources().expect("应能查");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].id, "https://b.com");
}

// ===========================================================================
// 书内搜索（FR-019b）
// ===========================================================================

#[test]
fn 书内搜索返回命中章节与字符偏移() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");
    seed_cached_chapter(&fixture, book_id, "第一章", "前面五个字目标在后面");

    let hits = fixture
        .repository
        .search_in_book(book_id, "目标")
        .expect("应能搜索");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].chapter_index, 0);
    assert_eq!(
        hits[0].char_offset, 5,
        "应是章内**字符**偏移（字节口径会给出 15）"
    );
    assert!(hits[0].snippet.contains("目标"));
}

/// 空库上搜索不该报错——索引表由实现侧按需建（迁移器不建它）。
#[test]
fn 空库书内搜索返回空() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");

    assert!(fixture
        .repository
        .search_in_book(book_id, "主角")
        .expect("应能搜索")
        .is_empty());
}

// ===========================================================================
// 备份（FR-033）与缓存清理（SC-017）
// ===========================================================================

#[test]
fn 备份导出与导入对称() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");
    fixture
        .repository
        .save_progress(&ReadProgress {
            book_id,
            chapter_index: 3,
            char_offset: 500,
            anchor_text: Some("特征段".to_string()),
            updated_at: 1_700_000_000,
        })
        .expect("应能写进度");

    let backup = fixture.repository.export_all().expect("应能导出");
    assert!(!backup.json.is_empty(), "导出应是可读的 JSON 文本");

    let report = fixture
        .repository
        .import_backup(&backup)
        .expect("导出的结果应能直接导回（同一类型，无需字段映射）");
    assert!(report.failures.is_empty());

    // 导入是**合并**语义：原有的书与进度都还在
    assert_eq!(fixture.repository.bookshelf().expect("应能查").len(), 1);
    assert_eq!(
        fixture
            .repository
            .get_progress(book_id)
            .expect("应能读")
            .expect("应有")
            .anchor_text
            .as_deref(),
        Some("特征段"),
        "锚点文本是换源降级链的输入，往返不得丢"
    );
}

/// SC-017：两类缓存**各自可清**，且清理本身不碰个人数据（FR-031 / SC-015）。
#[test]
fn 清空两类缓存不失败且不影响个人数据() {
    let fixture = fixture();
    let book_id = fixture
        .repository
        .add_book(&sample_book("id-1"))
        .expect("应能加入");
    fixture
        .repository
        .save_progress(&ReadProgress {
            book_id,
            chapter_index: 1,
            char_offset: 20,
            anchor_text: None,
            updated_at: 1_700_000_000,
        })
        .expect("应能写进度");

    fixture
        .repository
        .clear_cache(CacheKind::Chapter)
        .expect("空库上清章节缓存应成功");
    fixture
        .repository
        .clear_cache(CacheKind::Image)
        .expect("空库上清图片缓存应成功");

    assert_eq!(fixture.repository.bookshelf().expect("应能查").len(), 1);
    assert!(
        fixture
            .repository
            .get_progress(book_id)
            .expect("应能读")
            .is_some(),
        "清缓存 MUST NOT 影响个人阅读数据（SC-015）"
    );
}
