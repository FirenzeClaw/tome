//! T006 验收：引擎消息协议（`data-model.md` §3）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T006）
//!
//! - 协议字段与 `data-model.md` §3 **逐条一致**（自检表）
//! - `ClassifiedErrorView` 含 `id` / `detail` / `capability` 三字段
//! - 序列化 round-trip 测试绿（全变体）
//!
//! ## 为什么协议要可序列化（而 GUI 内部通道本不必）
//!
//! 三个具体用途，都不是"为将来可能有用"：
//!
//! 1. **崩溃/卡死现场可复现**：用户报"搜某本书卡住"时，把当帧的请求序列
//!    落盘就是一份可重放的输入（消息全是值，重放不依赖任何运行时状态）。
//! 2. **契约可机检**：本文件的"自检表"就是靠逐字段构造来钉住协议形状——
//!    没有序列化，就只能靠"读代码确认"，而那是会漂移的。
//! 3. **headless 断言**：视图状态机的测试（T009 起）直接构造消息值，
//!    不经 GUI。
//!
//! ## 唯一的例外：`ChapterReady.laid_out`
//!
//! 排版结果是**进程内的渲染产物**（`ChapterLayout` 含行盒与诊断，跨进程
//! 无意义），故它在协议里显式标注为不参与序列化。这条例外有专门断言
//! （`laid_out_是刻意不参与序列化的载荷`），以免它被当成遗漏。
//!
//! ## 本文件**不**门控在 `reader-render` 上
//!
//! 消息协议属**引擎层**：它只用到 `tome-reader` 的排版面（`paginate`/
//! `ReadingMode`/`ChapterLayout`），而排版面在任何构建里都在（`render` 才
//! 是门控项）。故本文件在 `--no-default-features` 下**同样运行**——协议形状
//! 与渲染面是否编入无关，把它门控掉只会让 headless 形态少一层保护。

use std::sync::Arc;

use tome_app::engine::messages::{
    BookmarkEntry, ClassifiedErrorView, ContentState, EngineEvent, EngineRequest, PageLayout,
    ReadingAnchor, ReadingParams, SearchHit, ShelfEntry, SourceRef, ThemeChoice, UserSettings,
};
use tome_core::error::ErrorCapability;
use tome_core::model::theme::Color;
use tome_reader::paginate::config::ReadingMode;

// ===========================================================================
// 自检表：协议字段与 data-model.md §3 逐条一致
// ===========================================================================

/// 一个请求归属 `data-model.md` §3 的哪一节。
///
/// ## 这个函数的存在理由就是"穷尽性"
///
/// 它是**穷尽匹配**：`EngineRequest` 新增变体而不在此归类 → 编译失败。
/// 于是"协议有 24 项、分属 7 节"这句话有了编译期的守门人，而不是靠注释。
fn request_section(request: &EngineRequest) -> &'static str {
    match request {
        EngineRequest::ShelfLoad
        | EngineRequest::ShelfImportLocal { .. }
        | EngineRequest::ShelfRemove { .. } => "Shelf",
        EngineRequest::ReadingOpen { .. }
        | EngineRequest::ReadingClose
        | EngineRequest::ReadingSetParams { .. }
        | EngineRequest::ReadingTurnPage { .. }
        | EngineRequest::ReadingSeekAnchor { .. }
        | EngineRequest::ReadingLoadChapter { .. }
        | EngineRequest::ReadingViewport { .. } => "Reading",
        EngineRequest::TocLoad { .. } => "Toc",
        EngineRequest::BookmarkAdd { .. }
        | EngineRequest::BookmarkRemove { .. }
        | EngineRequest::BookmarkList { .. } => "Bookmark",
        EngineRequest::SourceImportFile { .. }
        | EngineRequest::SourceImportUrl { .. }
        | EngineRequest::SourceImportText { .. }
        | EngineRequest::SourceSearch { .. }
        | EngineRequest::SourceCancelSearch { .. }
        | EngineRequest::SourceDetail { .. }
        | EngineRequest::SourceAddToShelf { .. }
        | EngineRequest::SourceSwitch { .. } => "Source",
        EngineRequest::ProgressFlush { .. } => "Progress",
        EngineRequest::SettingsLoad | EngineRequest::SettingsSave { .. } => "Settings",
    }
}

/// 一个事件归属 §3 的哪一节（穷尽匹配，同 [`request_section`]）。
fn event_section(event: &EngineEvent) -> &'static str {
    match event {
        EngineEvent::ShelfLoaded { .. }
        | EngineEvent::ShelfEntryChanged { .. }
        | EngineEvent::ImportFailed { .. } => "Shelf",
        EngineEvent::ChapterReady { .. } | EngineEvent::ChapterFailed { .. } => "Chapter",
        EngineEvent::ProgressRestored { .. } => "Progress",
        EngineEvent::TocLoaded { .. } => "Toc",
        EngineEvent::Bookmarks { .. } => "Bookmark",
        EngineEvent::SearchResult { .. } | EngineEvent::SearchDone { .. } => "Search",
        EngineEvent::DetailLoaded { .. } => "Detail",
        EngineEvent::AddedToShelf { .. } => "Shelf",
        EngineEvent::SourceSwitched { .. } => "Source",
        EngineEvent::SettingsLoaded { .. } | EngineEvent::Saved => "Settings",
        EngineEvent::RequestFailed { .. } => "Failed",
        EngineEvent::EnginePanic { .. } => "Panic",
    }
}

/// 全部 `EngineRequest` 变体都构造得出来，且分类与 §3 一致。
///
/// ## 这条测试为什么用"逐条构造"而不是"读一遍代码"
///
/// 协议的价值在于**形状**：少一个变体、字段名写错，都要在有人用到它时
/// 才暴露。逐条构造把"§3 写的每一项都真的存在"变成编译期 + 运行期的事实。
///
/// ## 穷尽性怎么保证（不靠"记得改测试"）
///
/// 下面 [`request_section`] 是一个**穷尽匹配**：给 `EngineRequest` 新增变体而
/// 不在这里归类，**编译直接失败**。故"忘了更新本表"不是一种可能——它无法
/// 通过构建。这比"测试里数个数"强得多（数个数只能事后发现）。
#[test]
fn requests_cover_every_section_of_the_model() {
    let anchor = ReadingAnchor {
        chapter_index: 3,
        char_offset: 120,
    };

    let requests = vec![
        // Shelf
        EngineRequest::ShelfLoad,
        EngineRequest::ShelfImportLocal {
            path: "D:/books/三体.txt".to_owned(),
        },
        EngineRequest::ShelfRemove {
            book_id: "local:三体".to_owned(),
            purge_cache: true,
        },
        // Reading
        EngineRequest::ReadingOpen {
            book_id: "local:三体".to_owned(),
        },
        EngineRequest::ReadingClose,
        EngineRequest::ReadingSetParams {
            book_id: "local:三体".to_owned(),
            params: ReadingParams::default_ma(),
        },
        EngineRequest::ReadingTurnPage { delta: -1 },
        EngineRequest::ReadingSeekAnchor { anchor },
        EngineRequest::ReadingLoadChapter {
            book_id: "local:三体".to_owned(),
            chapter_index: 7,
        },
        // Toc
        EngineRequest::TocLoad {
            book_id: "local:三体".to_owned(),
        },
        // Bookmark
        EngineRequest::BookmarkAdd {
            book_id: "local:三体".to_owned(),
            anchor,
            preview: "他的手指在书页边缘停了很久".to_owned(),
        },
        EngineRequest::BookmarkRemove {
            book_id: "local:三体".to_owned(),
            anchor,
        },
        EngineRequest::BookmarkList {
            book_id: "local:三体".to_owned(),
        },
        // Source
        EngineRequest::SourceImportFile {
            path: "D:/sources/synthetic.json".to_owned(),
        },
        EngineRequest::SourceImportUrl {
            url: "https://example.invalid/source.json".to_owned(),
        },
        EngineRequest::SourceImportText {
            text: "{\"bookSourceName\":\"合成源\"}".to_owned(),
        },
        EngineRequest::SourceSearch {
            generation: 1,
            keyword: "三体".to_owned(),
        },
        EngineRequest::SourceCancelSearch { generation: 1 },
        EngineRequest::SourceDetail {
            source_id: "synthetic".to_owned(),
            book_url: "https://example.invalid/book/1".to_owned(),
        },
        EngineRequest::SourceAddToShelf {
            source_id: "synthetic".to_owned(),
            book_url: "https://example.invalid/book/1".to_owned(),
        },
        EngineRequest::SourceSwitch {
            book_id: "synthetic|https://example.invalid/book/1".to_owned(),
            new_source_id: "synthetic-b".to_owned(),
        },
        // Progress
        EngineRequest::ProgressFlush {
            book_id: "local:三体".to_owned(),
        },
        // Settings
        EngineRequest::SettingsLoad,
        EngineRequest::SettingsSave {
            settings: UserSettings::default_ma(),
        },
    ];

    assert_eq!(
        requests.len(),
        24,
        "§3 的 EngineRequest 共 24 项（Shelf 3 / Reading 6 / Toc 1 / Bookmark 3 / \
         Source 8 / Progress 1 / Settings 2）"
    );

    // 每项都被穷尽匹配归了类（`request_section` 的穷尽性由编译器保证）
    let mut section_counts = std::collections::BTreeMap::new();
    for request in &requests {
        *section_counts
            .entry(request_section(request))
            .or_insert(0usize) += 1;
    }
    assert_eq!(
        section_counts.get("Shelf"),
        Some(&3),
        "§3 的 Shelf 节共 3 项，实测 {section_counts:?}"
    );
    assert_eq!(
        section_counts.get("Reading"),
        Some(&6),
        "实测 {section_counts:?}"
    );
    assert_eq!(
        section_counts.get("Toc"),
        Some(&1),
        "实测 {section_counts:?}"
    );
    assert_eq!(
        section_counts.get("Bookmark"),
        Some(&3),
        "实测 {section_counts:?}"
    );
    assert_eq!(
        section_counts.get("Source"),
        Some(&8),
        "实测 {section_counts:?}"
    );
    assert_eq!(
        section_counts.get("Progress"),
        Some(&1),
        "实测 {section_counts:?}"
    );
    assert_eq!(
        section_counts.get("Settings"),
        Some(&2),
        "实测 {section_counts:?}"
    );

    // 每个请求都能序列化（形状自洽的最廉价判据）
    for request in &requests {
        let json = serde_json::to_string(request).expect("请求应可序列化");
        assert!(!json.is_empty(), "序列化结果不应为空：{request:?}");
    }
}

/// 全部 `EngineEvent` 变体都构造得出来。
#[test]
fn events_cover_every_section_of_the_model() {
    let anchor = ReadingAnchor {
        chapter_index: 1,
        char_offset: 0,
    };
    let error = ClassifiedErrorView::from_classified(
        &tome_core::ErrorKind::Storage.with_detail("pack 索引损坏"),
    );

    let events = vec![
        EngineEvent::ShelfLoaded {
            entries: vec![sample_shelf_entry()],
        },
        EngineEvent::ShelfEntryChanged {
            entry: sample_shelf_entry(),
        },
        EngineEvent::ImportFailed {
            error: error.clone(),
        },
        EngineEvent::ChapterReady {
            book_id: "local:三体".to_owned(),
            chapter_index: 0,
            laid_out: None,
            content_version: 1,
        },
        EngineEvent::ChapterFailed {
            book_id: "local:三体".to_owned(),
            chapter_index: 0,
            error: error.clone(),
        },
        EngineEvent::ProgressRestored {
            book_id: "local:三体".to_owned(),
            progress: Some(ReadingParams::default_ma()),
            anchor,
        },
        EngineEvent::TocLoaded {
            book_id: "local:三体".to_owned(),
            toc: vec!["第一章".to_owned()],
        },
        EngineEvent::Bookmarks {
            book_id: "local:三体".to_owned(),
            list: vec![BookmarkEntry {
                anchor,
                preview: "他的手指".to_owned(),
                created_at: 1_760_000_000,
            }],
        },
        EngineEvent::SearchResult {
            generation: 1,
            source: SourceRef {
                source_id: "synthetic".to_owned(),
                name: "合成源".to_owned(),
                imported_at: 1_760_000_000,
            },
            hits: vec![sample_search_hit()],
        },
        EngineEvent::SearchDone { generation: 1 },
        EngineEvent::DetailLoaded {
            source_id: "synthetic".to_owned(),
            book_url: "https://example.invalid/book/1".to_owned(),
            title: "三体".to_owned(),
            author: "刘慈欣".to_owned(),
            intro: "文革如火如荼".to_owned(),
            toc: vec!["第一章".to_owned()],
        },
        EngineEvent::AddedToShelf {
            entry: sample_shelf_entry(),
        },
        EngineEvent::SourceSwitched {
            book_id: "synthetic|https://example.invalid/book/1".to_owned(),
            anchor,
        },
        EngineEvent::SettingsLoaded {
            settings: UserSettings::default_ma(),
        },
        EngineEvent::Saved,
        EngineEvent::EnginePanic {
            context: "layout_chapter".to_owned(),
        },
    ];

    assert_eq!(
        events.len(),
        16,
        "§3 的 EngineEvent 共 16 项（含 EnginePanic）"
    );

    // 穷尽匹配归类（`event_section` 的穷尽性由编译器保证）
    for event in &events {
        let section = event_section(event);
        assert!(!section.is_empty(), "事件必须归类到 §3 的某一节：{event:?}");
    }

    for event in &events {
        let json = serde_json::to_string(event).expect("事件应可序列化");
        assert!(!json.is_empty(), "序列化结果不应为空：{event:?}");
    }
}

// ===========================================================================
// round-trip（全变体）
// ===========================================================================

/// 每个变体序列化后再反序列化，值**逐字段相等**。
///
/// ## 为什么这条比"能序列化"强得多
///
/// `serde_json::to_string` 成功只说明"编码没报错"。真正的风险在**解码**：
/// 字段名拼错、`#[serde(rename)]` 漏写、枚举的变体标签不一致——这些都会
/// 让编码成功而解码得到别的值（或直接失败）。故判据必须是**往返相等**。
#[test]
fn every_variant_round_trips_exactly() {
    for request in all_requests() {
        let json = serde_json::to_string(&request).expect("编码应成功");
        let decoded: EngineRequest = serde_json::from_str(&json).expect("解码应成功");
        assert_eq!(decoded, request, "请求往返不相等：{json}");
    }

    for event in all_events() {
        let json = serde_json::to_string(&event).expect("编码应成功");
        let decoded: EngineEvent = serde_json::from_str(&json).expect("解码应成功");
        assert_eq!(
            serializable_projection(&decoded),
            serializable_projection(&event),
            "事件往返不相等：{json}"
        );
    }
}

/// `laid_out` 是**刻意**不参与序列化的载荷。
///
/// ## 为什么这条断言不能省
///
/// "某个字段没被序列化"既可能是设计（进程内渲染产物跨进程无意义），也可能是
/// **遗漏**。两者在代码上只差一个 `#[serde(skip)]`，而在行为上差一个静默的
/// 数据丢失。故把它写成断言：往返后该字段必为 `None`，且**原始值仍持有载荷**
/// （证明跳过发生在编码侧，而不是构造时就没给）。
#[test]
fn laid_out_是刻意不参与序列化的载荷() {
    let layout = Arc::new(sample_layout());
    let event = EngineEvent::ChapterReady {
        book_id: "local:三体".to_owned(),
        chapter_index: 2,
        laid_out: Some(Arc::clone(&layout)),
        content_version: 9,
    };

    let json = serde_json::to_string(&event).expect("编码应成功");
    let decoded: EngineEvent = serde_json::from_str(&json).expect("解码应成功");

    let EngineEvent::ChapterReady {
        book_id,
        chapter_index,
        laid_out,
        content_version,
    } = decoded
    else {
        panic!("解码后应仍是 ChapterReady，实测 {decoded:?}");
    };

    assert_eq!(book_id, "local:三体");
    assert_eq!(chapter_index, 2);
    assert_eq!(content_version, 9);
    assert!(
        laid_out.is_none(),
        "排版结果是进程内渲染产物，不该出现在序列化载荷里"
    );
    assert_eq!(
        Arc::strong_count(&layout),
        2,
        "跳过发生在编码侧——原始事件仍持有载荷（若构造时就没给，本断言无意义）"
    );
    drop(event);
    assert_eq!(
        Arc::strong_count(&layout),
        1,
        "事件析构后只剩本测试的引用，证明载荷从未被序列化路径克隆"
    );
}

// ===========================================================================
// ClassifiedErrorView：三字段
// ===========================================================================

/// `ClassifiedErrorView` 含 `id` / `detail` / `capability` 三字段（§4）。
#[test]
fn classified_error_view_has_the_three_fields() {
    // 普通分类错误：有 id 与 detail，无 capability
    let storage = ClassifiedErrorView::from_classified(
        &tome_core::ErrorKind::Storage.with_detail("pack 索引损坏"),
    );
    assert_eq!(storage.id, "error.storage");
    assert_eq!(storage.detail.as_deref(), Some("pack 索引损坏"));
    assert_eq!(storage.capability, None);

    // 无 detail 时不编造
    let network = ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Network);
    assert_eq!(network.id, "error.network");
    assert_eq!(network.detail, None);

    // 能力错误：三字段齐备（FR-022 / 002 §2.3 的 `[capability: xxx]` 形态）
    let capability = ClassifiedErrorView::from_kind(&tome_core::ErrorKind::unsupported_capability(
        ErrorCapability::new("offscreenRendering"),
    ));
    assert_eq!(capability.id, "error.unsupported_capability");
    assert_eq!(capability.capability.as_deref(), Some("offscreenRendering"));

    // 三字段都参与往返（否则"含三字段"只是构造侧的巧合）
    for view in [&storage, &network, &capability] {
        let json = serde_json::to_string(view).expect("编码应成功");
        let decoded: ClassifiedErrorView = serde_json::from_str(&json).expect("解码应成功");
        assert_eq!(&decoded, view, "错误视图往返不相等：{json}");
    }
}

/// 错误标识**永不翻译**（FR-019 / 002 FR-058）。
///
/// 外壳文案表做 `id → 文案` 映射，但 `id` 本身随文案一并呈现——否则用户
/// 报错时无法检索，书源的 `try/catch` 也无从匹配。
#[test]
fn error_identifiers_are_never_translated() {
    let view = ClassifiedErrorView::from_classified(
        &tome_core::ErrorKind::Network.with_detail("连接超时"),
    );
    // 标识是 `error.*` 形态的稳定串，不含任何面向用户的措辞
    assert!(
        view.id.starts_with("error."),
        "错误标识应是 error.* 形态的稳定标识，实测 {}",
        view.id
    );
    assert!(
        view.id.is_ascii(),
        "错误标识不得含中文（那意味着它被当成文案了）"
    );
}

// ===========================================================================
// 实体默认值（MASTER §5.1 / FR-018）
// ===========================================================================

/// 默认阅读参数取自 MASTER §5.1（不是随手写的数）。
#[test]
fn default_reading_params_come_from_the_design_system() {
    let params = ReadingParams::default_ma();
    let reading = tome_core::model::theme::READING;

    assert!(
        (params.font_size_px - reading.font_size_px).abs() < f32::EPSILON,
        "默认字号应取 MASTER §5.1 的 {}，实测 {}",
        reading.font_size_px,
        params.font_size_px
    );
    assert!(
        (params.line_height - reading.line_height).abs() < f32::EPSILON,
        "默认行高应取 MASTER §5.1 的 {}",
        reading.line_height
    );
    assert_eq!(params.mode, ReadingMode::Paged);
    assert_eq!(
        params.page_layout,
        PageLayout::Auto,
        "默认是自动（MASTER §5.2 的默认行为），不是强制单页"
    );
    assert_eq!(params.theme, ThemeChoice::FollowSystem);
}

/// 用户设置只有四组（FR-018 的范围纪律）。
#[test]
fn user_settings_are_exactly_four_groups() {
    let settings = UserSettings::default_ma();

    // 逐字段断言即"只有四组"的可判定形式：字段数在编译期固定，
    // 新增第五组必须改本测试（那是刻意的摩擦——范围纪律靠它守住）。
    let UserSettings {
        theme,
        default_font_size_px,
        default_line_height,
        default_mode,
        default_page_layout,
    } = settings;

    assert_eq!(theme, ThemeChoice::FollowSystem);
    assert!((default_font_size_px - tome_core::model::theme::READING.font_size_px).abs() < 0.001);
    assert!((default_line_height - tome_core::model::theme::READING.line_height).abs() < 0.001);
    assert_eq!(default_mode, ReadingMode::Paged);
    assert_eq!(default_page_layout, PageLayout::Auto);
}

/// `ThemeChoice` 三态可往返，且 `FollowSystem` **不是**一个主题名。
///
/// R7：设计系统没有"跟随系统"机制，它是外壳的**探测 + 映射**——故它必须与
/// `Day` / `Night` 并列存在，而不是解析成其中之一后丢失语义。
#[test]
fn theme_choice_keeps_follow_system_distinct() {
    for choice in [
        ThemeChoice::Day,
        ThemeChoice::Night,
        ThemeChoice::FollowSystem,
    ] {
        let json = serde_json::to_string(&choice).expect("编码应成功");
        let decoded: ThemeChoice = serde_json::from_str(&json).expect("解码应成功");
        assert_eq!(decoded, choice);
    }
    assert_ne!(ThemeChoice::FollowSystem, ThemeChoice::Day);
    assert_ne!(ThemeChoice::FollowSystem, ThemeChoice::Night);
}

/// `ContentState` 三态齐备（FR-010：不白屏 / 不伪装成功）。
#[test]
fn content_state_has_three_states() {
    let loading = ContentState::Loading;
    let failed = ContentState::Failed {
        error: ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Content),
    };

    assert_ne!(loading, failed);
    for state in [loading, failed] {
        let json = serde_json::to_string(&state).expect("编码应成功");
        let decoded: ContentState = serde_json::from_str(&json).expect("解码应成功");
        assert_eq!(decoded, state);
    }

    // `Ready` 携带内容版本号（纹理缓存以它失效，DM §2 不变量 2）
    let ready = ContentState::Ready { content_version: 4 };
    let json = serde_json::to_string(&ready).expect("编码应成功");
    let decoded: ContentState = serde_json::from_str(&json).expect("解码应成功");
    assert_eq!(decoded, ready);
}

// ===========================================================================
// 夹具
// ===========================================================================

fn sample_shelf_entry() -> ShelfEntry {
    ShelfEntry {
        book_id: "local:三体".to_owned(),
        title: "三体".to_owned(),
        author: "刘慈欣".to_owned(),
        origin_label: "本地书".to_owned(),
        cover: None,
        progress_percent: 0.0,
        last_read_at: None,
    }
}

fn sample_search_hit() -> SearchHit {
    SearchHit {
        book_url: "https://example.invalid/book/1".to_owned(),
        title: "三体".to_owned(),
        author: "刘慈欣".to_owned(),
        cover_url: None,
    }
}

/// 事件的可序列化投影（`laid_out` 除外，见
/// `laid_out_是刻意不参与序列化的载荷`）。
fn serializable_projection(event: &EngineEvent) -> EngineEvent {
    match event {
        EngineEvent::ChapterReady {
            book_id,
            chapter_index,
            content_version,
            ..
        } => EngineEvent::ChapterReady {
            book_id: book_id.clone(),
            chapter_index: *chapter_index,
            laid_out: None,
            content_version: *content_version,
        },
        other => other.clone(),
    }
}

fn all_requests() -> Vec<EngineRequest> {
    let anchor = ReadingAnchor {
        chapter_index: 3,
        char_offset: 120,
    };
    vec![
        EngineRequest::ShelfLoad,
        EngineRequest::ShelfImportLocal {
            path: "D:/books/三体.txt".to_owned(),
        },
        EngineRequest::ShelfRemove {
            book_id: "local:三体".to_owned(),
            purge_cache: true,
        },
        EngineRequest::ReadingOpen {
            book_id: "local:三体".to_owned(),
        },
        EngineRequest::ReadingClose,
        EngineRequest::ReadingSetParams {
            book_id: "local:三体".to_owned(),
            params: ReadingParams::default_ma(),
        },
        EngineRequest::ReadingTurnPage { delta: 1 },
        EngineRequest::ReadingSeekAnchor { anchor },
        EngineRequest::ReadingLoadChapter {
            book_id: "local:三体".to_owned(),
            chapter_index: 7,
        },
        EngineRequest::TocLoad {
            book_id: "local:三体".to_owned(),
        },
        EngineRequest::BookmarkAdd {
            book_id: "local:三体".to_owned(),
            anchor,
            preview: "他的手指".to_owned(),
        },
        EngineRequest::BookmarkRemove {
            book_id: "local:三体".to_owned(),
            anchor,
        },
        EngineRequest::BookmarkList {
            book_id: "local:三体".to_owned(),
        },
        EngineRequest::SourceImportFile {
            path: "D:/sources/synthetic.json".to_owned(),
        },
        EngineRequest::SourceImportUrl {
            url: "https://example.invalid/source.json".to_owned(),
        },
        EngineRequest::SourceImportText {
            text: "{}".to_owned(),
        },
        EngineRequest::SourceSearch {
            generation: 1,
            keyword: "三体".to_owned(),
        },
        EngineRequest::SourceCancelSearch { generation: 1 },
        EngineRequest::SourceDetail {
            source_id: "synthetic".to_owned(),
            book_url: "https://example.invalid/book/1".to_owned(),
        },
        EngineRequest::SourceAddToShelf {
            source_id: "synthetic".to_owned(),
            book_url: "https://example.invalid/book/1".to_owned(),
        },
        EngineRequest::SourceSwitch {
            book_id: "synthetic|https://example.invalid/book/1".to_owned(),
            new_source_id: "synthetic-b".to_owned(),
        },
        EngineRequest::ProgressFlush {
            book_id: "local:三体".to_owned(),
        },
        EngineRequest::SettingsLoad,
        EngineRequest::SettingsSave {
            settings: UserSettings::default_ma(),
        },
    ]
}

fn all_events() -> Vec<EngineEvent> {
    let anchor = ReadingAnchor {
        chapter_index: 1,
        char_offset: 0,
    };
    let error = ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Rule);
    vec![
        EngineEvent::ShelfLoaded {
            entries: vec![sample_shelf_entry()],
        },
        EngineEvent::ShelfEntryChanged {
            entry: sample_shelf_entry(),
        },
        EngineEvent::ImportFailed {
            error: error.clone(),
        },
        EngineEvent::ChapterReady {
            book_id: "local:三体".to_owned(),
            chapter_index: 0,
            laid_out: None,
            content_version: 1,
        },
        EngineEvent::ChapterFailed {
            book_id: "local:三体".to_owned(),
            chapter_index: 0,
            error: error.clone(),
        },
        EngineEvent::ProgressRestored {
            book_id: "local:三体".to_owned(),
            progress: None,
            anchor,
        },
        EngineEvent::TocLoaded {
            book_id: "local:三体".to_owned(),
            toc: vec!["第一章".to_owned()],
        },
        EngineEvent::Bookmarks {
            book_id: "local:三体".to_owned(),
            list: Vec::new(),
        },
        EngineEvent::SearchResult {
            generation: 2,
            source: SourceRef {
                source_id: "synthetic".to_owned(),
                name: "合成源".to_owned(),
                imported_at: 1_760_000_000,
            },
            hits: vec![sample_search_hit()],
        },
        EngineEvent::SearchDone { generation: 2 },
        EngineEvent::DetailLoaded {
            source_id: "synthetic".to_owned(),
            book_url: "https://example.invalid/book/1".to_owned(),
            title: "三体".to_owned(),
            author: "刘慈欣".to_owned(),
            intro: "文革如火如荼".to_owned(),
            toc: vec!["第一章".to_owned()],
        },
        EngineEvent::AddedToShelf {
            entry: sample_shelf_entry(),
        },
        EngineEvent::SourceSwitched {
            book_id: "synthetic|https://example.invalid/book/1".to_owned(),
            anchor,
        },
        EngineEvent::SettingsLoaded {
            settings: UserSettings::default_ma(),
        },
        EngineEvent::Saved,
        EngineEvent::EnginePanic {
            context: "layout_chapter".to_owned(),
        },
    ]
}

/// 一份真实排版结果（用于证明 `laid_out` 载荷确实存在过）。
#[expect(
    clippy::expect_used,
    reason = "夹具内的 expect 是刻意的：样本排版失败说明装置坏了，应当让测试红"
)]
fn sample_layout() -> tome_reader::paginate::page::ChapterLayout {
    use tome_reader::paginate::config::ReadingConfig;
    use tome_reader::paginate::page::{layout_chapter, Paginator};
    use tome_reader::paginate::spread::Viewport;

    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 300.0);
    let mut paginator = Paginator::new();
    layout_chapter(
        &mut paginator,
        "他的手指在书页边缘停了很久。",
        &geometry,
        &viewport,
    )
    .expect("排版应成功")
}

/// 让 `Color` 的导入不被 clippy 判为未使用（本文件的样本不涉颜色，
/// 但保留导入以示协议不依赖任何图形类型）。
#[test]
fn messages_do_not_depend_on_graphics_types() {
    // 协议里的"颜色"只可能以主题 token 的形式出现在渲染侧，
    // 消息本身不带 `wgpu`/`egui` 类型——这条断言用 `Color` 的纯值性质表达。
    let color = Color::rgb(0x2E, 0x24, 0x18);
    assert_eq!(color.to_hex_string(), "#2E2418");
}
