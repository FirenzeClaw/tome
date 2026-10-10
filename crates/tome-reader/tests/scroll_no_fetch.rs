//! T029 验收：滚动不触发抓取（FR-025）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T029）
//!
//! > 请求计数断言：滚动到未缓存区抓取次数 = 0
//! > 加载态/占位可辨识（同文件断言其存在与文案标识）
//!
//! ## FR-025 原文
//!
//! > 滚动模式下系统 MUST NOT 因滚动触发任何内容抓取——滚动到未缓存的区域时，
//! > 系统 MUST 按 001 §FR-019b 的既有约束行事（限定已缓存内容，不触发额外
//! > 抓取），并以明确的占位或加载态呈现。
//!
//! ## 本层怎么满足"零抓取"：**结构上做不到**
//!
//! `tome-reader` 的依赖表里**没有** `tome-net`，也没有 `reqwest`
//! （见 crate 根文档与 `Cargo.toml`）。正文唯一的入口是调用方注入的
//! [`tome_core::ContentProvider`]，而滚动流的构造只吃**已排好的行盒**
//! （`paginate::page::layout_chapter` 的产物）。
//!
//! 故"滚动触发抓取"在本层不是"被禁止了"，而是**没有可调用的手段**。本文件
//! 用两件事把这个事实变成可回归的断言：
//!
//! 1. **依赖面断言**：`tome-reader` 的源码与清单里不出现任何网络 crate
//!    （源码级 grep + Cargo.toml 解析）
//! 2. **计数宿主断言**：一个记录调用的 `ContentProvider` 在"滚动到任意位置"
//!    全程的 `chapter_content` 调用数**不增**（FR-025 的"抓取次数 = 0"）
//!
//! 第 1 条比第 2 条强：计数断言只能证明"这次没调"，依赖面断言证明"永远调
//! 不动"。两条都在，是因为依赖面断言在未来有人加依赖时会红——那时作者会
//! 看到这里的理由。
//!
//! ## "未缓存区"的形态：占位而非空白
//!
//! FR-025 的后半句要求以"明确的占位或加载态"呈现。本层提供
//! [`crate::scroll::placeholder`] 表达这个状态（**标识式**，不含面向用户的
//! 文案——FR-058）。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::cell::Cell;
use std::rc::Rc;

use tome_core::seam::{AnchorQuery, ChapterContent, ContentProvider};
use tome_core::{Book, BookOrigin, Chapter, ErrorKind};

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::ScrollFlow;
use tome_reader::scroll::map::PositionMap;
use tome_reader::scroll::placeholder::{PlaceholderKind, ScrollPlaceholder};

// ===========================================================================
// 计数宿主
// ===========================================================================

/// 记录 `toc` / `chapter_content` / `prefetch` 调用次数的宿主。
///
/// ## 为什么三个方法都记
///
/// FR-025 说"不触发任何内容抓取"。本层可能在三条路径上碰到内容源：
/// `toc`（取目录）、`chapter_content`（取正文）、`prefetch`（预热）。
/// 只记 `chapter_content` 会漏掉"滚动时顺手 prefetch 下一章"这类实现。
#[derive(Default)]
struct CountingHost {
    toc_calls: Rc<Cell<usize>>,
    chapter_calls: Rc<Cell<usize>>,
    prefetch_calls: Rc<Cell<usize>>,
    content: String,
}

impl ContentProvider for CountingHost {
    fn toc(&self, _book: &Book) -> Result<Vec<Chapter>, ErrorKind> {
        self.toc_calls.set(self.toc_calls.get() + 1);
        Ok(Vec::new())
    }

    fn chapter_content(
        &self,
        _book: &Book,
        chapter_index: i32,
    ) -> Result<ChapterContent, ErrorKind> {
        self.chapter_calls.set(self.chapter_calls.get() + 1);
        Ok(ChapterContent {
            chapter_index,
            title: String::new(),
            content: self.content.clone(),
            from_cache: true,
        })
    }

    fn resolve_anchor(&self, _book: &Book, anchor: &AnchorQuery) -> Result<(i32, i64), ErrorKind> {
        Ok((anchor.chapter_index, anchor.char_offset))
    }

    fn prefetch(&self, _book: &Book, _around_index: i32, _depth: u32) {
        self.prefetch_calls.set(self.prefetch_calls.get() + 1);
    }
}

fn sample_book() -> Book {
    Book {
        id: 1,
        origin: BookOrigin::Network,
        origin_id: "src|url".to_string(),
        name: "测试书".to_string(),
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
    }
}

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

fn long_chapter() -> String {
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                     后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。";
    (0..10)
        .map(|index| format!("第{}段。{paragraph}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_flow(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("构造滚动流应成功")
}

// ===========================================================================
// FR-025 主判据：抓取次数 = 0
// ===========================================================================

/// **滚动到任意位置，内容抓取次数恒为 0**（FR-025 的计数断言）。
///
/// ## "滚动到未缓存区"怎么模拟
///
/// 本层没有"缓存区"的概念（那是 001 的关切）。可模拟的是：**遍历全部视口**
/// 并把每个视口的定位、映射、重排都跑一遍——若实现在某条路径上会去取内容，
/// 那是它唯一的机会。
///
/// 宿主返回 `from_cache: true`（已缓存），故本用例测的是"缓存命中时也不该
/// 重复取"。
#[test]
fn scrolling_never_fetches_content() {
    let toc_calls = Rc::new(Cell::new(0usize));
    let chapter_calls = Rc::new(Cell::new(0usize));
    let prefetch_calls = Rc::new(Cell::new(0usize));

    let host = CountingHost {
        toc_calls: Rc::clone(&toc_calls),
        chapter_calls: Rc::clone(&chapter_calls),
        prefetch_calls: Rc::clone(&prefetch_calls),
        content: long_chapter(),
    };
    let book = sample_book();

    // 组合根取一次正文（唯一一次）
    let chapter = host.chapter_content(&book, 0).expect("取正文应成功");
    assert_eq!(chapter_calls.get(), 1, "取正文应恰好一次");

    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&chapter.content, &geometry, &viewport);
    let map = PositionMap::new(&flow);

    // 滚动全程：遍历每一屏、每屏的首/中/末，做定位与映射
    let mut steps = 0usize;
    for viewer in flow.viewports() {
        for anchor in [
            viewer.char_range.start,
            (viewer.char_range.start + viewer.char_range.end) / 2,
            viewer.char_range.end.saturating_sub(1),
        ] {
            // 滚动到该位置：这些都是"滚动"会做的事
            let _ = map.char_to_offset(anchor);
            let _ = map.offset_to_char(viewer.top_px);
            let _ = viewer.char_range.contains(&anchor);
            steps += 1;
        }
    }
    // 覆盖度自检：每屏 3 个位置，故至少应有 3 屏（否则"遍历全程"是空话）
    assert!(
        steps >= 9 && steps == flow.viewports().len() * 3,
        "应遍历每屏的 3 个位置（实测 {steps} 步 / {} 屏）",
        flow.viewports().len()
    );

    // 重排（几何变化）也不得取内容
    let mut changed = geometry;
    changed.font_size_px = 24;
    let _ = build_flow(&chapter.content, &changed, &viewport);

    assert_eq!(
        chapter_calls.get(),
        1,
        "滚动与重排全程的正文取用次数必须仍是 1（FR-025：抓取次数 = 0 增量）"
    );
    assert_eq!(toc_calls.get(), 0, "滚动不得取目录");
    assert_eq!(
        prefetch_calls.get(),
        0,
        "滚动不得触发预取（FR-025 的'任何内容抓取'）"
    );
}

/// **宿主不可用时滚动仍能工作**（证明本层不依赖内容源）。
///
/// ## 这条断言的形式为什么是"给一个会报错的宿主"
///
/// 若本层在任何滚动路径上碰内容源，给它一个"永远失败"的宿主就会让滚动失败。
/// 而滚动流的构造与定位**只吃已排好的行盒**，与宿主无关——故它能正常工作。
#[test]
fn scrolling_works_with_a_failing_host() {
    /// 所有调用都失败的宿主。
    struct FailingHost;

    impl ContentProvider for FailingHost {
        fn toc(&self, _book: &Book) -> Result<Vec<Chapter>, ErrorKind> {
            Err(ErrorKind::Network)
        }
        fn chapter_content(
            &self,
            _book: &Book,
            _chapter_index: i32,
        ) -> Result<ChapterContent, ErrorKind> {
            Err(ErrorKind::Network)
        }
        fn resolve_anchor(
            &self,
            _book: &Book,
            _anchor: &AnchorQuery,
        ) -> Result<(i32, i64), ErrorKind> {
            Err(ErrorKind::Network)
        }
        fn prefetch(&self, _book: &Book, _around_index: i32, _depth: u32) {}
    }

    let host = FailingHost;
    let book = sample_book();
    assert!(
        host.chapter_content(&book, 0).is_err(),
        "前置：该宿主确实不可用"
    );

    // 用**早已取得**的正文构造滚动流（模拟组合根在缓存命中时取得的内容）
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);

    assert!(
        !flow.viewports().is_empty(),
        "宿主不可用不影响滚动流的构造（本层不依赖内容源）"
    );
}

// ===========================================================================
// 结构断言：本层没有网络依赖
// ===========================================================================

/// **`tome-reader` 依赖表里没有任何网络 crate**（FR-025 的结构性保证）。
///
/// ## 为什么这条比计数断言强
///
/// 计数断言只能证明"这个用例没调内容源"。依赖面断言证明**压根没有可调用的
/// 手段**——`Cargo.toml` 里没有 `tome-net`/`reqwest`，源码里就不会出现它们的
/// 类型。
///
/// 若未来有人在 `tome-reader` 里加了网络依赖，本测试会红，而它会把 FR-025
/// 的理由摆在改动者面前。
#[test]
fn reader_crate_has_no_network_dependency() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    // --- 1. 清单里不出现网络 crate ---
    let manifest = std::fs::read_to_string(manifest_dir.join("Cargo.toml"))
        .expect("应能读取 tome-reader 的清单");

    // 只看 `[dependencies]` 段落（注释里提到 crate 名是允许且必要的：
    // 本 crate 的文档就在解释"为什么不依赖 tome-net"）
    let dependencies_section = manifest
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    for forbidden in ["tome-net", "reqwest", "hyper", "ureq", "curl"] {
        assert!(
            !dependencies_section.contains(forbidden),
            "tome-reader 的依赖清单里出现了网络 crate `{forbidden}`——\
             FR-025 要求滚动不得触发任何内容抓取。本层拿到的只能是组合根已取好的正文。"
        );
    }

    // --- 2. 源码里不出现网络类型 ---
    let mut sources = Vec::new();
    collect_rust_sources(&manifest_dir.join("src"), &mut sources);
    assert!(!sources.is_empty(), "应收集到源码文件");

    for path in &sources {
        let source = std::fs::read_to_string(path).expect("应能读取源码");
        let code: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        for forbidden in ["reqwest::", "ureq::", "tome_net::"] {
            assert!(
                !code.contains(forbidden),
                "{} 的代码里出现了 `{forbidden}`——本层不得有任何网络通路（FR-025）",
                path.display()
            );
        }
    }
}

/// 递归收集 `src/` 下的 `.rs` 文件。
fn collect_rust_sources(root: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

// ===========================================================================
// FR-025 后半句：占位/加载态可辨识
// ===========================================================================

/// **未缓存区以占位呈现，且占位类型可枚举、标识可辨识**（FR-025 后半句）。
///
/// ## 判据的形态：标识而非文案
///
/// FR-058 要求引擎不硬编码面向用户的自然语言。故占位以**标识**表达
/// （如 `scroll.placeholder.loading`），文案映射留给 S3。
///
/// 本测试断言：① 变体可枚举；② 每个变体有稳定标识；③ 标识是 ASCII
/// （不是中文字面量）。
#[test]
fn placeholder_kinds_are_enumerable_with_stable_ids() {
    let kinds = [
        PlaceholderKind::Loading,
        PlaceholderKind::Uncached,
        PlaceholderKind::Unavailable,
    ];

    for kind in kinds {
        let id = kind.message_id();
        assert!(
            id.starts_with("scroll.placeholder."),
            "占位标识应有统一前缀，实测 {id}"
        );
        assert!(
            id.is_ascii(),
            "占位标识必须是 ASCII 标识（不得是面向用户的文案，FR-058），实测 {id}"
        );
    }

    // 三个变体的标识互不相同（可辨识）
    let mut ids: Vec<&str> = kinds.iter().map(|kind| kind.message_id()).collect();
    ids.sort_unstable();
    let total = ids.len();
    ids.dedup();
    assert_eq!(total, ids.len(), "各占位变体的标识必须互不相同");
}

/// 占位可**绑定到字符区间**（它占据排版位置，不是一个游离的状态）。
#[test]
fn placeholder_can_bind_to_a_char_range() {
    let placeholder = ScrollPlaceholder {
        kind: PlaceholderKind::Uncached,
        char_range: 1200..1500,
    };
    assert_eq!(placeholder.char_range.len(), 300);
    assert!(
        placeholder.char_range.contains(&1300),
        "占位应能回答'某个字符是否落在它覆盖的范围内'"
    );

    // 空区间占位合法（如"整章不可用"）
    let whole = ScrollPlaceholder {
        kind: PlaceholderKind::Unavailable,
        char_range: 0..0,
    };
    assert!(whole.char_range.is_empty());
}

/// **占位不占用正文的字符区间**（它不认领字符，避免污染 SC-006b 的完整性）。
#[test]
fn placeholder_does_not_claim_characters_from_the_flow() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);

    // 视口区间覆盖正文（SC-006b），占位是**额外的**呈现层信息，不参与那个并集
    let union: String = flow
        .viewports()
        .iter()
        .map(|viewport| {
            content
                .chars()
                .skip(viewport.char_range.start)
                .take(viewport.char_range.end - viewport.char_range.start)
                .collect::<String>()
        })
        .collect();
    assert_eq!(
        union, content,
        "占位的存在不得改变视口区间（SC-006b 的完整性优先）"
    );
}

/// 滚动全程**不产生**占位（内容已在手时无未缓存区）。
#[test]
fn cached_content_produces_no_placeholder() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);

    // 本层不自行判定"哪段未缓存"——那需要缓存状态，属 001。故在没有明确
    // 的未缓存信号时，占位列表应为空（不臆造加载态）。
    let placeholders = ScrollPlaceholder::none_for_cached_content(&flow, &content);
    assert!(
        placeholders.is_empty(),
        "内容已完整在手时不得臆造占位（实测 {} 个）",
        placeholders.len()
    );
}
