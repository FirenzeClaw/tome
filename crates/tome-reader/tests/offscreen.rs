//! T035 验收：离屏渲染 + 读回（FR-053、FR-052；research R4）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T035）
//!
//! - 离屏产出 PNG（读回像素**非全零**断言）
//! - 无后端环境（mock 注入）下结构化错误测试
//!
//! ## "非全零"为什么是这条链路唯一有效的判据
//!
//! 未真正渲染的纹理读回的是**全零**——那是合法的缓冲区内容，长度断言会
//! 通过。spike T009 的首轮就是被这个骗过（像素只是过了一遍搬运，GPU 一次
//! 算术都没做）。故本文件除了断言非零，还断言**覆盖率在合理区间**：
//! 全零（没渲染）与 100%（整块涂满，说明画错了）都要能被区分。
//!
//! ## 诚实跳过（FR-053）
//!
//! 无后端时打印 SKIPPED 且**不计为通过**；而"无后端"这条路径本身必须
//! 可测——否则它只是一句没被执行过的代码。用
//! `RenderPipeline::probe_backend_with(Backends::empty())` 确定性地触发它。

#![cfg(feature = "render")]

use tome_core::model::theme::DayTheme;
use tome_reader::paginate::config::ReadingConfig;
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::render::batch::RenderBatch;
use tome_reader::render::glyph_cache::GlyphCache;
use tome_reader::render::offscreen::{aligned_bytes_per_row, OffscreenRenderer};
use tome_reader::render::pipeline::{PipelineError, RenderPipeline};

const WIDTH: u32 = 342;
const HEIGHT: u32 = 300;

#[expect(
    clippy::panic,
    reason = "夹具内的 panic 是刻意的：非能力缺失的失败必须让测试红，不得被当成跳过"
)]
fn pipeline_or_skip(context: &str) -> Option<RenderPipeline> {
    match RenderPipeline::probe_backend() {
        Ok(backend) => Some(build_pipeline(backend)),
        Err(PipelineError::NoAdapter { reason }) => {
            println!("[T035] SKIPPED ({context}): 无可用图形后端——{reason}");
            println!("[T035] 本条**不计为通过**（FR-053）");
            None
        }
        Err(other) => panic!("[T035] 后端探测以非'无适配器'的方式失败：{other}"),
    }
}

#[expect(
    clippy::expect_used,
    reason = "本夹具只在探测已成功之后调用；失败说明装置坏了，应当让测试红"
)]
fn build_pipeline(backend: tome_reader::render::pipeline::GraphicsBackend) -> RenderPipeline {
    RenderPipeline::new(backend, WIDTH, HEIGHT).expect("有后端时应能建管线")
}

fn sample_chapter() -> String {
    [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样；她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然。」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
        "《红楼梦》（又名《石头记》）是清代曹雪芹的作品，书中有四百多个人物，其中以宝黛钗三人最为人所知。",
    ]
    .join("\n")
}

#[expect(
    clippy::expect_used,
    reason = "夹具内的 expect 是刻意的：样本排版失败说明装置坏了，应当让测试红"
)]
fn build_batch() -> RenderBatch {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(f32::from(WIDTH as u16), f32::from(HEIGHT as u16));
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");
    RenderBatch::for_chapter(&chapter, &geometry, &viewport, DayTheme.theme())
}

// ===========================================================================
// 离屏链路（读回像素非全零）
// ===========================================================================

/// **离屏产出像素且非全零**（T035 的主判据之一）。
#[test]
fn offscreen_renders_non_black_pixels() {
    let Some(pipeline) = pipeline_or_skip("offscreen_renders_non_black_pixels") else {
        return;
    };

    let renderer = OffscreenRenderer::new(WIDTH, HEIGHT).expect("尺寸合法");
    let mut cache = GlyphCache::new();
    let batch = build_batch();

    let image = renderer
        .render(&pipeline, &batch, &mut cache)
        .expect("离屏渲染应成功");

    assert!(image.is_consistent(), "像素数与尺寸应自洽");
    assert_eq!(image.width, WIDTH);
    assert_eq!(image.height, HEIGHT);
    assert_eq!(
        image.pixels.len(),
        (WIDTH * HEIGHT * 4) as usize,
        "读回的紧凑像素长度应为 宽×高×4"
    );

    // 非全零（"渲染真的发生了"的判据）
    assert!(
        image.non_black_pixels() > 0,
        "读回像素全为零——渲染没有发生（spike T009 的首轮教训）"
    );

    // ## 前景覆盖率（与**背景色**比较，不是"非黑"）
    //
    // 昼主题底色是米黄，本身就不是黑——用"非黑"口径时背景清色成功就会让
    // 覆盖率恒为 100%，于是"只清底、没画字形"的实现也能通过（实测踩到）。
    // 故判据必须相对于背景色。
    let background = DayTheme.theme().palette().bg_reading;
    let coverage = image.foreground_coverage([background.red, background.green, background.blue]);
    assert!(
        coverage > 0.001,
        "前景覆盖率过低（{coverage:.4}）——几乎没有字形被画上去（只剩底色的可能性）"
    );
    assert!(
        coverage < 0.99,
        "前景覆盖率过高（{coverage:.4}）——整块涂满说明几何算错了（正文应是稀疏前景）"
    );
    println!(
        "[T035] 离屏读回 {}×{}，前景像素 {}/{}（{:.2}%）",
        image.width,
        image.height,
        image.foreground_pixels([background.red, background.green, background.blue]),
        image.total_pixels(),
        coverage * 100.0
    );
}

/// **背景色真的被清上**（不是只有字形）。
///
/// ## 为什么单独一条
///
/// "非全零"可能只由字形块带来，而背景清色若失效（比如 `LoadOp::Clear`
/// 参数错），页面的底色就丢了——那是 FR-030 的违规（阅读区必须用主题的
/// 阅读层色），但上面那条非零断言看不出来。
#[test]
fn background_is_cleared_to_theme_color() {
    let Some(pipeline) = pipeline_or_skip("background_is_cleared_to_theme_color") else {
        return;
    };

    let renderer = OffscreenRenderer::new(WIDTH, HEIGHT).expect("尺寸合法");
    let mut cache = GlyphCache::new();
    // 空批次：只有背景，没有字形
    let batch = RenderBatch::background_only(WIDTH, HEIGHT, DayTheme.theme());

    let image = renderer
        .render(&pipeline, &batch, &mut cache)
        .expect("离屏渲染应成功");

    // 空批次的整张图都应是背景色
    let expected = DayTheme.theme().palette().bg_reading;
    let first = &image.pixels[..4];
    assert_eq!(
        [first[0], first[1], first[2]],
        [expected.red, expected.green, expected.blue],
        "阅读区底色应为主题的 bg_reading（FR-030）"
    );

    // 全图都是该色（空批次没有别的内容）
    let all_background = image.pixels.as_chunks::<4>().0.iter().all(|pixel| {
        pixel[0] == expected.red && pixel[1] == expected.green && pixel[2] == expected.blue
    });
    assert!(all_background, "空批次应整张都是背景色");
}

/// 昼夜两主题产出**不同底色**（FR-030 的语义色映射在两个方向都成立）。
#[test]
fn day_and_night_produce_different_backgrounds() {
    use tome_core::model::theme::NightTheme;

    let Some(pipeline) = pipeline_or_skip("day_and_night_produce_different_backgrounds") else {
        return;
    };

    let renderer = OffscreenRenderer::new(64, 64).expect("尺寸合法");
    let mut cache = GlyphCache::new();

    let day = renderer
        .render(
            &pipeline,
            &RenderBatch::background_only(64, 64, DayTheme.theme()),
            &mut cache,
        )
        .expect("昼主题渲染应成功");
    let night = renderer
        .render(
            &pipeline,
            &RenderBatch::background_only(64, 64, NightTheme.theme()),
            &mut cache,
        )
        .expect("夜主题渲染应成功");

    assert_ne!(
        &day.pixels[..3],
        &night.pixels[..3],
        "昼夜主题的底色必须不同（FR-030）"
    );
    // 夜主题是**真黑**（FR-030：不得用深灰替代）
    assert_eq!(
        &night.pixels[..3],
        &[0u8, 0u8, 0u8],
        "夜主题阅读区底色应为纯黑（FR-030：MUST NOT 用深灰替代真黑）"
    );
}

/// 同批次两次渲染**逐像素相同**（离屏路径自身确定）。
#[test]
fn offscreen_rendering_is_deterministic() {
    let Some(pipeline) = pipeline_or_skip("offscreen_rendering_is_deterministic") else {
        return;
    };

    let renderer = OffscreenRenderer::new(WIDTH, HEIGHT).expect("尺寸合法");
    let mut cache = GlyphCache::new();
    let batch = build_batch();

    let first = renderer
        .render(&pipeline, &batch, &mut cache)
        .expect("首次渲染应成功");
    let second = renderer
        .render(&pipeline, &batch, &mut cache)
        .expect("二次渲染应成功");

    assert_eq!(
        first.pixels, second.pixels,
        "同批次的离屏渲染应逐像素相同（快照基线的前提）"
    );
}

/// 不同尺寸的离屏渲染各自正确（256 对齐在不同宽度下都对）。
#[test]
fn various_widths_read_back_correctly() {
    let Some(pipeline) = pipeline_or_skip("various_widths_read_back_correctly") else {
        return;
    };

    // 覆盖"刚好对齐"（64×4=256）与"需要补 padding"（65×4=260→512）两类
    for width in [64u32, 65, 100, 127, 128] {
        let renderer = OffscreenRenderer::new(width, 32).expect("尺寸合法");
        let mut cache = GlyphCache::new();
        let batch = RenderBatch::background_only(width, 32, DayTheme.theme());

        let image = renderer
            .render(&pipeline, &batch, &mut cache)
            .unwrap_or_else(|error| panic!("宽度 {width} 的离屏渲染应成功：{error}"));

        assert_eq!(
            image.pixels.len(),
            (width * 32 * 4) as usize,
            "宽度 {width} 的读回长度应为 宽×高×4（去 padding 后）"
        );
        assert!(image.is_consistent(), "宽度 {width} 的像素数与尺寸应自洽");
        // 对齐值必须是 256 的倍数（wgpu 的硬要求）
        assert_eq!(aligned_bytes_per_row(width, 4) % 256, 0);
    }
}

// ===========================================================================
// 无后端：结构化错误（不伪装成功）
// ===========================================================================

/// **无后端时报结构化能力错误**（FR-053 的"不得伪装通过"）。
///
/// ## 这条路径为什么必须可测
///
/// 开发机有 GPU、CI 有 lavapipe——"没有后端"在正常环境里**永远走不到**。
/// 用空后端集确定性地触发，否则这条判据只是一句没被执行过的代码。
#[test]
fn no_backend_reports_structured_capability_error() {
    // 空后端集 → 必然拿不到适配器（不依赖机器环境）
    let error = RenderPipeline::probe_backend_with(wgpu::Backends::empty())
        .expect_err("空后端集不该拿到适配器");

    assert!(
        matches!(error, PipelineError::NoAdapter { .. }),
        "应报 NoAdapter，实测 {error:?}"
    );
    assert_eq!(
        error.capability_id(),
        Some(tome_reader::capability::OFFSCREEN_RENDERING),
        "无后端必须映射为 offscreenRendering 能力标识（FR-053/FR-057）"
    );

    // 映射到统一错误面后仍是**能力错误**（上层据此跳过而非当故障处理）
    let reader_error = error.to_reader_error();
    assert_eq!(
        reader_error.kind().message_id(),
        "error.unsupported_capability",
        "无后端应映射为可捕获的能力错误（FR-058）"
    );
    assert_eq!(
        reader_error.capability_id(),
        Some("offscreenRendering"),
        "能力标识应可被上层读出（书源的 try/catch 降级依赖它）"
    );
}

/// **读回失败不伪装成功**：错误路径给出结构化细节而非空图像。
#[test]
fn readback_errors_are_structured() {
    let error = PipelineError::Readback {
        detail: "map_async 未完成".to_owned(),
    };

    // 读回失败**不是**能力问题（那是运行期故障）——不该让上层静默跳过
    assert_eq!(
        error.capability_id(),
        None,
        "读回故障不该被归为能力不可用（那会掩盖真实缺陷）"
    );
    assert!(
        error.to_string().contains("map_async"),
        "错误应带可定位的细节，实测：{error}"
    );
}

/// 零尺寸的离屏渲染器被拒（不产生无法渲染的状态）。
#[test]
fn zero_sized_offscreen_is_rejected() {
    let error = OffscreenRenderer::new(0, 100).expect_err("零宽应被拒");
    assert!(matches!(error, PipelineError::InvalidExtent { .. }));
}
