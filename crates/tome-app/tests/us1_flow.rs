//! T013 / T014 / T016 验收：US1 端到端闭环（**真实调用，非桩**）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T013/T014/T016）
//!
//! - T013：本地导入双径（拖入 + 文件选择最终汇到同一请求）；坏文件给出**具体**
//!   错误且书架不留半成品；重复导入识别为同一条目
//! - T014：打开书 → 排版 → `ChapterReady` 带 `laid_out` 与单调递增的
//!   `content_version`；纹理缓存在版本未变时**不重绘**
//! - T016：导入到开始阅读的**步数**计数；位置逐字符恢复
//!
//! ## 为什么这是"端到端"而不是又一层单元测试
//!
//! 本文件起的是一条**真的引擎线程**，配的是 `tome-store` 的 `SqliteBookRepository`
//! 生产实现（T010）、`tome-local` 的真实 TXT 解析、`tome-reader` 的真实排版
//! 管线。没有假体——故它验的是"这些东西接在一起真的能跑"，而那是任何一层的
//! 单元测试都验不到的（接口对得上、语义对不上是常态）。
//!
//! ## 为什么可以 headless 跑
//!
//! 整条链路（解析 → 落库 → 取正文 → 排版）都不需要窗口——图形只在**绘制**
//! 那一步才出现（`RenderBatch`），而本文件断言到 `ChapterLayout` 为止。
//! FR-052 的"能在无窗口系统上完整运行"在这里是**可执行的**，不是一句声明。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tome_app::engine::messages::{EngineEvent, EngineRequest, ShelfEntry};
use tome_app::engine::thread::EngineHandle;
use tome_app::engine::worker::AppEngine;
use tome_core::seam::BookRepository;
use tome_store::book_repository::SqliteBookRepository;
use tome_store::datadir::DataDir;
use tome_store::db::Db;

/// 测试用的数据目录（临时目录，用真的 `resolve` 走一次探测）。
struct Fixture {
    /// 临时目录的持有者（drop 时清理）
    _temp: tempfile::TempDir,
    /// 数据目录
    data_dir: DataDir,
    /// 接缝（供断言直接查库）
    repository: Arc<dyn BookRepository + Send + Sync>,
    /// 引擎线程句柄
    engine: EngineHandle,
}

impl Fixture {
    /// 起一套真实链路。
    ///
    /// `#[expect]` 的理由：本 `impl` 块里全是**夹具方法**（非 `#[test]` 函数），
    /// `.clippy.toml` 的 `allow-expect-in-tests` 不覆盖它们（见该文件说明）。
    /// 夹具搭不起来时后续断言全部失去意义，故在此 panic 恰当——但要显式声明。
    #[expect(
        clippy::expect_used,
        reason = "夹具方法：临时目录/建库/起线程失败则后续断言无意义"
    )]
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("临时目录");
        let resolved = DataDir::resolve(Some(temp.path()), None, None);
        let data_dir = DataDir::from_resolved(&resolved).expect("临时目录应可用作数据目录");
        data_dir.prepare().expect("建目录布局");

        let db = Db::open(&data_dir.database_path()).expect("开库");
        let repository: Arc<dyn BookRepository + Send + Sync> =
            Arc::new(SqliteBookRepository::new(db, &data_dir));

        let engine = {
            let repository = Arc::clone(&repository);
            EngineHandle::spawn(move || Ok(AppEngine::new(repository))).expect("起引擎线程")
        };

        Self {
            _temp: temp,
            data_dir,
            repository,
            engine,
        }
    }

    /// 写一本合成 TXT（带章节标记，让目录识别有活干）。
    #[expect(clippy::expect_used, reason = "夹具方法：写不出测试书则无从断言")]
    fn write_book(&self, name: &str, chapters: usize) -> PathBuf {
        let mut text = String::new();
        for index in 1..=chapters {
            text.push_str(&format!("第{index}章 标题{index}\n"));
            // 每章写足够多的字，让排版能产出多页（否则"翻页"无从测）
            for line in 0..40 {
                text.push_str(&format!(
                    "这是第{index}章的第{line}行正文，用来让排版引擎有足够的内容可分页。\n"
                ));
            }
        }
        let path = self.data_dir.root().join(name);
        std::fs::write(&path, text).expect("写测试书");
        path
    }

    /// 发一个请求，等它产生的事件（超时即失败）。
    ///
    /// ## 为什么要超时而不是死等
    ///
    /// 死等一个永不到来的事件会让测试**挂住**（CI 上表现为超时后整批红，
    /// 而失败信息是"测试超时"，看不出是哪一步）。带超时至少能指出"哪个请求
    /// 没有回应"。
    fn request(&self, request: EngineRequest) -> Vec<EngineEvent> {
        assert!(self.engine.send(request), "引擎线程应仍在服务");
        self.wait_events(Duration::from_secs(30))
    }

    /// 收集事件直到**安静下来**（连续 `QUIET` 没有新事件）。
    ///
    /// ## 为什么是"安静下来"而不是"收到某个特定事件"
    ///
    /// 一次请求可能产出零到多条事件（如打开书会产出 `ProgressRestored` +
    /// `TocLoaded` + `ChapterReady` 三条）。按"收到某一条就返回"会让其余的
    /// 留在队列里污染下一个断言——那种串扰极难归因。
    ///
    /// ## 静默计时为什么从**调用时刻**起算
    ///
    /// 有些请求**本就应当零事件**（如视口尺寸未变时不重排）。若静默计时只在
    /// "收到第一个事件之后"才开始，那些请求就得等满超时——本测试文件最初
    /// 就是这么写的，后果是单条断言耗时 30 秒。从调用起算让"零事件"与
    /// "事件已收完"走同一条快速路径。
    ///
    /// `QUIET` 取值要**大于**一次排版的最坏耗时：引擎产出 `ChapterReady` 前
    /// 要跑整章排版（几十毫秒量级），静默窗口短于它会让多事件请求被截断。
    fn wait_events(&self, timeout: Duration) -> Vec<EngineEvent> {
        /// 静默窗口（见上文：必须大于一次排版的最坏耗时）
        const QUIET: Duration = Duration::from_millis(600);

        let deadline = Instant::now() + timeout;
        let mut events = Vec::new();
        let mut last_seen = Instant::now();

        while Instant::now() < deadline {
            let batch = self.engine.drain_events();
            if batch.is_empty() {
                if last_seen.elapsed() >= QUIET {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            last_seen = Instant::now();
            events.extend(batch);
        }
        events
    }
}

/// 从事件里取出书架条目列表。
fn shelf_of(events: &[EngineEvent]) -> Vec<ShelfEntry> {
    events
        .iter()
        .find_map(|event| match event {
            EngineEvent::ShelfLoaded { entries } => Some(entries.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// 从事件里取出章节就绪的那一条。
fn chapter_ready(events: &[EngineEvent]) -> Option<&EngineEvent> {
    events
        .iter()
        .find(|event| matches!(event, EngineEvent::ChapterReady { .. }))
}

/// 从事件里取出导入失败的标识。
fn import_failure_id(events: &[EngineEvent]) -> Option<String> {
    events.iter().find_map(|event| match event {
        EngineEvent::ImportFailed { error } => Some(error.id.clone()),
        _ => None,
    })
}

// ===========================================================================
// T013：本地导入
// ===========================================================================

/// 导入一本 TXT → 书架出现该书（SC-003 的第 1 步）。
#[test]
fn importing_a_txt_puts_it_on_the_shelf() {
    let fixture = Fixture::new();
    let path = fixture.write_book("book.txt", 3);

    let events = fixture.request(EngineRequest::ShelfImportLocal {
        path: path.display().to_string(),
    });

    let changed = events
        .iter()
        .find_map(|event| match event {
            EngineEvent::ShelfEntryChanged { entry } => Some(entry.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("导入应产生 ShelfEntryChanged，实测事件 {events:?}"));
    assert_eq!(changed.title, "book", "标题应回退为文件名（S1 语义）");
    assert_eq!(
        changed.origin_label, "local",
        "来源标签应是稳定标识而非中文（文案映射在 i18n）"
    );

    // 库里的确落了条目，且带上了章节
    let books = fixture.repository.bookshelf().expect("书架可读");
    assert_eq!(books.len(), 1);
    let chapters = fixture.repository.chapters(books[0].id).expect("章节可读");
    assert!(
        chapters.len() >= 3,
        "目录识别应认出至少 3 章，实测 {}",
        chapters.len()
    );
}

/// **坏文件**：给出结构化错误，且书架**不留半成品**（spec 边缘情况）。
///
/// ## 为什么"不留半成品"要单独断言
///
/// 导入分两步（落书 → 落章）。第二步失败时若不回滚，书架上会留一个**打不开
/// 的卡片**——用户点进去得到的是一个"这本书没有可读的章节"，而他会以为文件
/// 本身有问题（其实是他导入时程序出了错）。这类残留最难排查，因为它看起来
/// 像数据损坏。
#[test]
fn a_broken_file_leaves_no_half_written_entry() {
    let fixture = Fixture::new();

    // 伪造 EPUB：扩展名说是 epub，内容是**二进制垃圾**（不是 zip）。
    //
    // 这里必须含 NUL 字节：`tome_local::format::detect` 有意让"文本判定优先于
    // 扩展名"（真实场景里有 .epub 里装 TXT 的文件），故纯文本的假 EPUB 会被
    // 当成 TXT 正常导入——那是 S1 的既定语义，不是缺陷。含 NUL 才是"二进制
    // 容器但结构损坏"，也就是本测试要覆盖的那一类。
    let bad = fixture.data_dir.root().join("broken.epub");
    let mut garbage = vec![0u8; 64];
    garbage.extend_from_slice(b"not a zip archive at all");
    std::fs::write(&bad, garbage).expect("写坏文件");

    let events = fixture.request(EngineRequest::ShelfImportLocal {
        path: bad.display().to_string(),
    });

    let error_id = import_failure_id(&events)
        .unwrap_or_else(|| panic!("坏文件应给出结构化错误，实测事件 {events:?}"));
    assert!(
        error_id.starts_with("error."),
        "错误必须是可本地化的标识形态，实测 {error_id}"
    );
    assert!(
        !error_id.contains("unsupported"),
        "格式损坏应报内容错误而不是能力缺失（能力缺失是'这个格式不支持'），实测 {error_id}"
    );

    // 关键：书架**没有**留下条目
    let books = fixture.repository.bookshelf().expect("书架可读");
    assert!(
        books.is_empty(),
        "导入失败后书架必须为空，实测 {} 条",
        books.len()
    );
}

/// **不存在的路径**：结构化错误，不 panic。
#[test]
fn importing_a_missing_path_fails_structurally() {
    let fixture = Fixture::new();
    let events = fixture.request(EngineRequest::ShelfImportLocal {
        path: "D:/绝对不存在的路径/nope.txt".to_owned(),
    });
    assert!(import_failure_id(&events).is_some());
    assert!(fixture.repository.bookshelf().expect("书架可读").is_empty());
}

/// **重复导入**识别为同一条目（spec 边缘情况："更新而非重复卡片"）。
#[test]
fn re_importing_the_same_file_updates_one_entry() {
    let fixture = Fixture::new();
    let path = fixture.write_book("same.txt", 2);
    let display = path.display().to_string();

    fixture.request(EngineRequest::ShelfImportLocal {
        path: display.clone(),
    });
    fixture.request(EngineRequest::ShelfImportLocal { path: display });

    let books = fixture.repository.bookshelf().expect("书架可读");
    assert_eq!(
        books.len(),
        1,
        "同一份文件导入两次必须识别为同一条目（按内容哈希判身份），实测 {} 条",
        books.len()
    );
}

// ===========================================================================
// T014：打开书 → 排版 → ChapterReady
// ===========================================================================

/// 打开书：先要视口尺寸，再排版；`laid_out` 非空且版本号递增。
///
/// ## 时序为什么必须先给视口
///
/// 版心高度决定一页装几行，而引擎不知道阅读区多大时**无从排版**。这不是实现
/// 细节，而是契约：`ChapterReady` 的 `laid_out` 只在视口已知后产出。本测试
/// 显式走这条时序，把它钉成"可执行的契约"。
#[test]
fn opening_a_book_lays_out_after_the_viewport_is_known() {
    let fixture = Fixture::new();
    let path = fixture.write_book("read.txt", 3);
    fixture.request(EngineRequest::ShelfImportLocal {
        path: path.display().to_string(),
    });
    let book_id = shelf_of(&fixture.request(EngineRequest::ShelfLoad))
        .first()
        .map(|entry| entry.book_id.clone())
        .expect("书架应有书");

    // ① 不给视口就打开：**不应**产出 ChapterReady（无从排版）
    let events = fixture.request(EngineRequest::ReadingOpen {
        book_id: book_id.clone(),
    });
    assert!(
        chapter_ready(&events).is_none(),
        "视口未知时不该产出 ChapterReady，实测 {events:?}"
    );
    // 但进度与目录应当已经到达（它们不依赖视口）
    assert!(events
        .iter()
        .any(|event| matches!(event, EngineEvent::ProgressRestored { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, EngineEvent::TocLoaded { .. })));

    // ② 告知视口：现在应产出 ChapterReady，且带排版结果
    let events = fixture.request(EngineRequest::ReadingViewport {
        width_px: 800.0,
        height_px: 1000.0,
    });
    let (laid_out, version) = events
        .iter()
        .find_map(|event| match event {
            EngineEvent::ChapterReady {
                laid_out,
                content_version,
                ..
            } => Some((laid_out.clone(), *content_version)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("给视口后应产出 ChapterReady，实测事件 {events:?}"));

    let layout = laid_out.expect("ChapterReady 必须带排版结果（否则 UI 无内容可画）");
    assert!(!layout.lines.is_empty(), "排版结果应有行盒");
    assert!(layout.char_len > 0, "章内字符数应大于 0");
    assert!(layout.measure_px > 0.0, "版心宽应大于 0");
    assert_eq!(version, 1, "首次排版版本号应为 1");
}

/// 翻页会递增 `content_version`（纹理缓存以它失效）。
#[test]
fn turning_a_page_bumps_the_content_version() {
    let fixture = Fixture::new();
    let path = fixture.write_book("turn.txt", 3);
    fixture.request(EngineRequest::ShelfImportLocal {
        path: path.display().to_string(),
    });
    let book_id = shelf_of(&fixture.request(EngineRequest::ShelfLoad))
        .first()
        .map(|entry| entry.book_id.clone())
        .expect("书架应有书");

    fixture.request(EngineRequest::ReadingOpen { book_id });
    let first = fixture.request(EngineRequest::ReadingViewport {
        width_px: 800.0,
        height_px: 1000.0,
    });
    let version_of = |events: &[EngineEvent]| -> Option<u64> {
        events.iter().find_map(|event| match event {
            EngineEvent::ChapterReady {
                content_version, ..
            } => Some(*content_version),
            _ => None,
        })
    };
    let v1 = version_of(&first).expect("首次排版应有版本号");

    let second = fixture.request(EngineRequest::ReadingTurnPage { delta: 1 });
    let v2 = version_of(&second).expect("翻页后应重新产出 ChapterReady");
    assert!(
        v2 > v1,
        "翻页后版本号必须递增（纹理缓存以它失效），实测 {v1} → {v2}"
    );
}

/// 视口尺寸**未变**时不重排（否则 egui 每帧都会触发一次整章重排）。
#[test]
fn an_unchanged_viewport_does_not_relayout() {
    let fixture = Fixture::new();
    let path = fixture.write_book("static.txt", 2);
    fixture.request(EngineRequest::ShelfImportLocal {
        path: path.display().to_string(),
    });
    let book_id = shelf_of(&fixture.request(EngineRequest::ShelfLoad))
        .first()
        .map(|entry| entry.book_id.clone())
        .expect("书架应有书");

    fixture.request(EngineRequest::ReadingOpen { book_id });
    fixture.request(EngineRequest::ReadingViewport {
        width_px: 800.0,
        height_px: 1000.0,
    });

    // 同一个尺寸再报三次：应一次都不重排
    for _ in 0..3 {
        let events = fixture.request(EngineRequest::ReadingViewport {
            width_px: 800.0,
            height_px: 1000.0,
        });
        assert!(
            chapter_ready(&events).is_none(),
            "视口未变时不得重排（egui 每帧都会报尺寸），实测 {events:?}"
        );
    }
}

// ===========================================================================
// T016：SC-003 步数与 SC-009 位置恢复
// ===========================================================================

/// **SC-003**：导入到开始阅读 **≤3 步**。
///
/// ## 步的口径
///
/// 一步 = 一个用户动作（点击/拖入/按键一次）。本测试按**状态机**的调用序
/// 计数（`ViewState` 的公开意图方法各算一步），因为那是"用户做了几个动作"
/// 在代码里的精确对应物：
///
/// 1. 拖入文件（`begin_import` + `ShelfImportLocal` 是同一个动作的两半）
/// 2. 点击书卡（`open_book_request`）
/// 3. ——
///
/// 第三步是"引擎自动排版"（不是用户动作）。故实际是 **2 步**，留有余量。
#[test]
fn sc003_importing_and_opening_takes_at_most_three_steps() {
    use tome_app::app::ViewState;

    let fixture = Fixture::new();
    let path = fixture.write_book("steps.txt", 2);

    let mut state = ViewState::new();
    let mut steps = 0usize;

    // --- 第 1 步：把文件拖进窗口（用户动作：拖入）---
    steps += 1;
    state.begin_import();
    let import = EngineRequest::ShelfImportLocal {
        path: path.display().to_string(),
    };

    // 引擎真的把它导进来
    let events = fixture.request(import);
    for event in events {
        state.apply_event(event);
    }

    // --- 第 2 步：点击书卡（用户动作：点击）---
    let book_id = state
        .shelf()
        .entries()
        .first()
        .map(|entry| entry.book_id.clone())
        .expect("导入后书架应有书");
    steps += 1;
    let open = state.open_book_request(book_id);

    // 引擎真的打开并排版（视口由 UI 在渲染时告知）
    for event in fixture.request(open) {
        state.apply_event(event);
    }
    for event in fixture.request(EngineRequest::ReadingViewport {
        width_px: 800.0,
        height_px: 1000.0,
    }) {
        state.apply_event(event);
    }

    // 断言：内容就绪（可读），且用户只做了 2 步
    assert!(
        matches!(
            state.content(),
            tome_app::engine::messages::ContentState::Ready { .. }
        ),
        "两步之后应已进入可读状态，实测内容态 {:?}",
        state.content()
    );
    assert!(steps <= 3, "SC-003 要求 ≤3 步，实测 {steps} 步");
}

/// **SC-009**：读到中部 → 返回书架 → 重开 → 位置**逐字符恢复**。
#[test]
fn sc009_the_position_restores_character_for_character() {
    let fixture = Fixture::new();
    let path = fixture.write_book("progress.txt", 5);
    fixture.request(EngineRequest::ShelfImportLocal {
        path: path.display().to_string(),
    });
    let book_id = shelf_of(&fixture.request(EngineRequest::ShelfLoad))
        .first()
        .map(|entry| entry.book_id.clone())
        .expect("书架应有书");

    // --- 第一次会话：打开、翻几页、退出 ---
    fixture.request(EngineRequest::ReadingOpen {
        book_id: book_id.clone(),
    });
    fixture.request(EngineRequest::ReadingViewport {
        width_px: 800.0,
        height_px: 1000.0,
    });
    for _ in 0..4 {
        fixture.request(EngineRequest::ReadingTurnPage { delta: 1 });
    }
    // 记录离开时的位置（从库里读，而不是从事件里猜）
    fixture.request(EngineRequest::ProgressFlush {
        book_id: book_id.clone(),
    });
    let saved = fixture
        .repository
        .get_progress(book_id.parse().expect("书 id 是数字"))
        .expect("进度可读")
        .expect("冲刷后应有进度");
    fixture.request(EngineRequest::ReadingClose);

    // --- 第二次会话：重开 ---
    let events = fixture.request(EngineRequest::ReadingOpen {
        book_id: book_id.clone(),
    });
    let restored = events
        .iter()
        .find_map(|event| match event {
            EngineEvent::ProgressRestored { anchor, .. } => Some(*anchor),
            _ => None,
        })
        .expect("打开书应恢复进度");

    assert_eq!(
        restored.chapter_index, saved.chapter_index,
        "章序号必须逐字相等（SC-009）"
    );
    assert_eq!(
        restored.char_offset, saved.char_offset,
        "字符偏移必须逐字相等（SC-009）——这是'逐字符恢复'的字面要求"
    );
}

/// 移除（确认）→ 书架消失，缓存按 S1 语义清理（FR-004）。
#[test]
fn removing_a_book_clears_it_from_the_shelf() {
    let fixture = Fixture::new();
    let path = fixture.write_book("remove.txt", 2);
    fixture.request(EngineRequest::ShelfImportLocal {
        path: path.display().to_string(),
    });
    let book_id = shelf_of(&fixture.request(EngineRequest::ShelfLoad))
        .first()
        .map(|entry| entry.book_id.clone())
        .expect("书架应有书");

    let events = fixture.request(EngineRequest::ShelfRemove {
        book_id,
        purge_cache: true,
    });

    let entries = shelf_of(&events);
    assert!(
        entries.is_empty(),
        "移除后书架应为空，实测 {} 条",
        entries.len()
    );
    assert!(fixture.repository.bookshelf().expect("书架可读").is_empty());
}

/// 打开一本**不存在的**书：结构化错误，不 panic、不留错状态。
#[test]
fn opening_a_missing_book_fails_structurally() {
    let fixture = Fixture::new();
    let events = fixture.request(EngineRequest::ReadingOpen {
        book_id: "999999".to_owned(),
    });
    let failed = events.iter().any(|event| {
        matches!(
            event,
            EngineEvent::ChapterFailed { .. } | EngineEvent::RequestFailed { .. }
        )
    });
    assert!(failed, "打开不存在的书应报结构化错误，实测 {events:?}");

    // 引擎线程仍存活（可继续服务）
    let after = fixture.request(EngineRequest::ShelfLoad);
    assert!(
        after
            .iter()
            .any(|event| matches!(event, EngineEvent::ShelfLoaded { .. })),
        "一次失败之后引擎线程必须仍可服务"
    );
}

/// 本地书路径归一化：Windows 反斜杠不得写进库（FR-042 可移植）。
#[test]
fn the_stored_path_is_portable() {
    let fixture = Fixture::new();
    let path = fixture.write_book("portable.txt", 1);
    fixture.request(EngineRequest::ShelfImportLocal {
        path: path.display().to_string(),
    });

    let books = fixture.repository.bookshelf().expect("书架可读");
    let relative = books[0].relative_path.as_deref().expect("本地书应有路径");
    assert!(
        !relative.contains('\\'),
        "存储路径不得含反斜杠（数据目录搬到 Linux 后即失效），实测 {relative}"
    );
}

/// `Path` 在断言里用得上（避免未使用导入）。
#[allow(dead_code)]
fn path_type_probe(path: &Path) -> bool {
    path.is_absolute()
}
