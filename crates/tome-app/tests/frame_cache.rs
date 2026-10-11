//! T014 验收：版心帧缓存与静态帧零重绘（`crates/tome-app/src/render/texture.rs`）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T014）
//!
//! - 静态帧零重绘断言：`content_version` 未变时 S2 重渲染不被触发
//!   （`contracts/ui.md` §4.2）
//!
//! ## "零重绘"的准确口径（不说满）
//!
//! 见 `render/texture.rs` 的模块文档：路径 A（PaintCallback 同 device 直绘）
//! 下没有可读回的纹理，缓存的是**从排版结果构造绘制批次**这一步（遍历行盒
//! × 逐字形算四边形），而它正是"S2 重渲染"在 CPU 侧的全部代价。
//!
//! GPU 侧的 render pass 仍会执行——那是 egui 的重绘节奏决定的（它重画整个
//! 窗口），把它算进"重绘"会让判据变成"窗口永远不能重绘"，那既不可能也无意义。
//!
//! ## 为什么这条判据能在 CI 里跑（不需要窗口）
//!
//! `RenderBatch::for_line_slice` 是**纯计算**（值进、值出），不碰 GPU。故本
//! 文件用真实的排版结果构造真实的批次，断言的是真实的重算次数——不是模拟。

#![cfg(feature = "reader-render")]

use std::time::Instant;

use tome_app::render::texture::PageFrameCache;
use tome_core::model::theme::{DayTheme, Theme};
use tome_reader::paginate::config::GeometryConfig;
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;

/// 造一份真实的排版结果（跑真的排版引擎，不是假数据）。
#[expect(clippy::expect_used, reason = "夹具：排版失败则全部断言失去意义")]
fn laid_out() -> tome_reader::paginate::page::ChapterLayout {
    let mut text = String::new();
    for index in 0..40 {
        text.push_str(&format!(
            "这是第 {index} 段正文，用来让排版引擎产出足够多的行盒以便断言缓存行为。\n"
        ));
    }
    let geometry = GeometryConfig::default();
    let viewport = Viewport::new(800.0, 1000.0);
    let mut paginator = Paginator::new();
    layout_chapter(&mut paginator, &text, &geometry, &viewport).expect("排版应成功")
}

fn theme() -> Theme {
    DayTheme.theme()
}

/// **静态帧零重绘**：同一 `content_version` 反复取帧，只构造一次批次。
#[test]
fn an_unchanged_version_rebuilds_the_batch_once() {
    let layout = laid_out();
    let geometry = GeometryConfig::default();
    let viewport = Viewport::new(800.0, 1000.0);
    let mut cache = PageFrameCache::new();

    for _ in 0..60 {
        // 60 帧 ≈ 一秒的 60fps
        let _ = cache.frame(1, &layout, &geometry, &viewport, theme());
    }

    assert_eq!(
        cache.rebuild_count(),
        1,
        "版本未变时只应构造一次批次（S2 重渲染不得被触发），实测 {} 次",
        cache.rebuild_count()
    );
    assert_eq!(
        cache.hit_count(),
        59,
        "除首次外其余 59 帧都应命中缓存，实测 {}",
        cache.hit_count()
    );
}

/// 版本**变化**时重构造一次（这正是"内容变了要重绘"）。
#[test]
fn a_new_version_rebuilds_the_batch() {
    let layout = laid_out();
    let geometry = GeometryConfig::default();
    let viewport = Viewport::new(800.0, 1000.0);
    let mut cache = PageFrameCache::new();

    let _ = cache.frame(1, &layout, &geometry, &viewport, theme());
    let _ = cache.frame(1, &layout, &geometry, &viewport, theme());
    assert_eq!(cache.rebuild_count(), 1);

    let _ = cache.frame(2, &layout, &geometry, &viewport, theme());
    assert_eq!(cache.rebuild_count(), 2, "版本变化必须重构造");
    assert_eq!(cache.version(), Some(2));
}

/// **尺寸变化**也重构造（版本号之外的第二个失效键）。
///
/// ## 为什么必须有这一条
///
/// 版本号由引擎线程递增，而尺寸变化要经一次往返（UI → 引擎 → 事件）才体现为
/// 新版本号。在那之前，若窗口已被拖大，UI 拿到的批次尺寸仍是旧的——按它分配
/// 矩形会让版心在窗口里"缩在一角"（拉大窗口后内容不跟着变大，直到引擎的事件
/// 回来）。带上尺寸做二次校验，那一帧就按新尺寸重算背景。
#[test]
fn a_changed_viewport_rebuilds_even_at_the_same_version() {
    let layout = laid_out();
    let geometry = GeometryConfig::default();
    let mut cache = PageFrameCache::new();

    let _ = cache.frame(
        1,
        &layout,
        &geometry,
        &Viewport::new(800.0, 1000.0),
        theme(),
    );
    assert_eq!(cache.rebuild_count(), 1);

    let _ = cache.frame(
        1,
        &layout,
        &geometry,
        &Viewport::new(1200.0, 1000.0),
        theme(),
    );
    assert_eq!(
        cache.rebuild_count(),
        2,
        "宽度变了必须按新尺寸重构造（否则窗口拉大后版心缩在一角）"
    );
}

/// 换书清缓存：清空后同一版本号也会重构造。
///
/// ## 这条守的是什么
///
/// `content_version` 是**会话内**单调的，跨书没有可比性（新书的版本号从头
/// 开始）。不清的话，打开一本新书若恰好拿到相同的版本号，界面会显示**上一本
/// 书**的版心——这是最糟糕的一类缺陷（看起来像渲染错乱，实则是一个忘了失效
/// 的缓存）。
#[test]
fn clearing_the_cache_forces_a_rebuild() {
    let layout = laid_out();
    let geometry = GeometryConfig::default();
    let viewport = Viewport::new(800.0, 1000.0);
    let mut cache = PageFrameCache::new();

    let _ = cache.frame(7, &layout, &geometry, &viewport, theme());
    assert_eq!(cache.version(), Some(7));

    cache.clear();
    assert_eq!(cache.version(), None, "清空后不该再有版本号");

    let _ = cache.frame(7, &layout, &geometry, &viewport, theme());
    assert_eq!(
        cache.rebuild_count(),
        2,
        "清空之后即便是同一个版本号也必须重构造（否则会显示上一本书的版心）"
    );
}

/// 缓存命中的取帧耗时**显著低于**首次构造（缓存真的省了事）。
///
/// ## 为什么要有这条性能断言
///
/// "命中缓存"若只是返回一个等价的批次、而构造照样跑一遍，上一条测试（只数
/// `rebuild_count`）仍会绿——计数是我们自己维护的，它可能与实际代价脱节。
/// 本测试用**时间**交叉验证：命中路径必须比构造路径快一个量级。
#[test]
fn a_cache_hit_is_much_cheaper_than_a_rebuild() {
    let layout = laid_out();
    let geometry = GeometryConfig::default();
    let viewport = Viewport::new(800.0, 1000.0);

    // 首次构造的耗时（多跑几次取最小，避免调度抖动）
    let mut rebuild = f64::MAX;
    for _ in 0..5 {
        let mut cache = PageFrameCache::new();
        let started = Instant::now();
        let _ = cache.frame(1, &layout, &geometry, &viewport, theme());
        rebuild = rebuild.min(started.elapsed().as_secs_f64());
    }

    // 命中路径的耗时
    let mut cache = PageFrameCache::new();
    let _ = cache.frame(1, &layout, &geometry, &viewport, theme());
    let mut hit = f64::MAX;
    for _ in 0..200 {
        let started = Instant::now();
        let _ = cache.frame(1, &layout, &geometry, &viewport, theme());
        hit = hit.min(started.elapsed().as_secs_f64());
    }

    assert!(
        hit * 10.0 < rebuild,
        "命中路径应比构造路径快一个量级以上，实测命中 {:.3}ms vs 构造 {:.3}ms",
        hit * 1000.0,
        rebuild * 1000.0
    );
}

/// 取到的批次确实含有字形（缓存没有把内容吃掉）。
#[test]
fn the_cached_batch_still_carries_the_glyphs() {
    let layout = laid_out();
    let geometry = GeometryConfig::default();
    let viewport = Viewport::new(800.0, 1000.0);
    let mut cache = PageFrameCache::new();

    let first = cache.frame(1, &layout, &geometry, &viewport, theme());
    let second = cache.frame(1, &layout, &geometry, &viewport, theme());

    assert!(
        first.glyph_count() > 0,
        "批次应含字形（排版结果有行盒，行盒里有字形）"
    );
    assert_eq!(
        first.glyph_count(),
        second.glyph_count(),
        "命中缓存取到的应是同一份内容"
    );
    // 背景尺寸应等于视口（`RenderBatch` 的画布以视口为准）
    assert_eq!(first.background().width, 800);
    assert_eq!(first.background().height, 1000);
}
