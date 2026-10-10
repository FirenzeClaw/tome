//! T036 验收：像素快照 harness（SC-011 / FR-053；research R4）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T036）
//!
//! - pixelmatch-YIQ 双闸门：per-pixel 0.6 + 失败比例 ≤ 0.1%（初期 0.05% 从严）
//! - **自校验**：造差异图断言**检出**、造相同图断言通过（两侧）
//! - 后端不可用时输出「SKIPPED: <原因>」显式报告，**不计为通过**
//!
//! ## 自校验为什么是本文件的重点（PM 点名的装置纪律）
//!
//! `spikes/s2/BASELINE.md` §10.1 记了三次"全绿但没测到"，于是把"测量装置
//! 自验"固化为纪律。快照 harness 是最容易假绿的装置形态：若比对函数恒返回
//! "无差异"，所有快照测试都会通过，而它们其实什么都没验。
//!
//! 故 [`harness_detects_injected_differences`] 是**本文件的第一个用例**：
//! 它人为往图像里注入已知差异，断言 harness **必须报红**。这条不过，
//! 后面所有"快照通过"的结论都不成立。
//!
//! ## CI artifacts 约定（写在注释里，供 T049 的 CI 任务照做）
//!
//! 快照失败时 CI 应上传三类文件供人工判断：
//!
//! ```text
//!   <name>-expected.png   基线（仓库内 tests/snapshots/）
//!   <name>-actual.png     本次实际渲染
//!   <name>-difference.png 差异可视化（超出阈值的像素标红）
//! ```
//!
//! 前两个让 reviewer 肉眼看"差在哪"，第三个让"差多少"有客观依据。
//! CI 上传步骤见 `specs/002-typesetting-renderer/tasks.md` 的 T049。

#![cfg(feature = "render")]

use tome_core::model::theme::{DayTheme, NightTheme};
use tome_reader::paginate::config::ReadingConfig;
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::render::batch::RenderBatch;
use tome_reader::render::glyph_cache::GlyphCache;
use tome_reader::render::offscreen::{OffscreenRenderer, RenderedImage};
use tome_reader::render::pipeline::{PipelineError, RenderPipeline};
use tome_reader::render::pixel_diff::{
    compare_rgba, DEFAULT_MAX_DIFF_RATIO, DEFAULT_YIQ_THRESHOLD, SPEC_MAX_DIFF_RATIO,
};

const WIDTH: u32 = 342;
const HEIGHT: u32 = 300;

// ===========================================================================
// 自校验：harness 必须能**检出**差异（装置纪律，本文件的第一个用例）
// ===========================================================================

/// **harness 检出注入的差异**（否则后面所有"通过"都不成立）。
///
/// ## 为什么这条必须先跑
///
/// 一个恒返回"无差异"的比对函数会让全部快照测试通过。本用例注入三种
/// 已知差异，断言它们**都被检出**：
///
/// | 注入 | 应被哪个闸门抓到 |
/// |---|---|
/// | 整幅偏移一个通道 +80 | 比例闸门（很多像素各差一点） |
/// | 一块 20×20 涂成纯黑 | per-pixel 闸门（少数像素差得离谱） |
/// | 尺寸不同 | 直接判 100% |
#[test]
fn harness_detects_injected_differences() {
    let width = 64usize;
    let height = 64usize;
    let background = [0xF5u8, 0xEE, 0xDC];

    let baseline: Vec<u8> = (0..width * height)
        .flat_map(|_| [background[0], background[1], background[2], 0xFF])
        .collect();

    // --- 注入 1：整幅改为**深色**（比例闸门该抓）---
    //
    // ## 为什么不用"饱和加 80"
    //
    // 初版注入 `#F5EEDC + 80`——它在每通道各自饱和，结果变成近白。
    // 而米黄与近白的 YIQ 距离只有 0.071（远低于 0.6 阈值），于是
    // **整幅改了颜色却没被检出**。这本身是个有价值的观察：YIQ 的
    // 亮度加权意味着"两个都接近白的颜色"差异很小——阈值 0.6 的语义是
    // "容忍抗锯齿级别的细微差异"，不是"任何颜色变化"。
    //
    // 真实的失效形态是"内容变了"（字形错位、底色变了），那会产生**显著的
    // 亮度差**。注入用它才有代表性。
    let shifted: Vec<u8> = baseline
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|_| [0x2Eu8, 0x24, 0x18, 0xFF])
        .collect();
    let report = compare_rgba(&baseline, &shifted, DEFAULT_YIQ_THRESHOLD);
    assert!(
        !report.passes(DEFAULT_MAX_DIFF_RATIO),
        "整幅换成深色必须被检出（否则比例闸门失效）：{}",
        report.summary()
    );
    assert!(
        report.ratio() > 0.5,
        "整幅改色的差异比例应接近 100%，实测 {:.4}",
        report.ratio()
    );

    // --- 注入 2：局部块变化（per-pixel 闸门该抓）---
    let mut patched = baseline.clone();
    for row in 10..30 {
        for column in 10..30 {
            let index = (row * width + column) * 4;
            patched[index] = 0;
            patched[index + 1] = 0;
            patched[index + 2] = 0;
        }
    }
    let report = compare_rgba(&baseline, &patched, DEFAULT_YIQ_THRESHOLD);
    assert!(
        report.different_pixels > 0,
        "20×20 的黑块必须被检出（否则 per-pixel 闸门失效）：{}",
        report.summary()
    );
    assert!(
        report.ratio() < 0.5,
        "局部块的差异比例应远小于整幅，实测 {:.4}（说明两个闸门测的是不同的量）",
        report.ratio()
    );

    // --- 注入 3：尺寸不同 ---
    let wrong_size = vec![0u8; 4 * 10 * 10];
    let report = compare_rgba(&baseline, &wrong_size, DEFAULT_YIQ_THRESHOLD);
    assert_eq!(
        report.ratio(),
        1.0,
        "尺寸不同必须判为 100% 差异（结构变了比内容变了更严重）"
    );

    println!("[T036] 自校验通过：三种注入差异都被检出");
}

/// **相同图像必须通过**（自校验的另一侧）。
///
/// ## 为什么两侧都要断言
///
/// 只测"能检出差异"会放过一个"永远报红"的实现——那是另一种失效形态
/// （测试全红 → 被忽略 → 等于没有）。快照 harness 必须**两侧都对**：
/// 相同图通过、不同图报红。
#[test]
fn harness_passes_identical_images() {
    let baseline: Vec<u8> = (0..64 * 64)
        .flat_map(|_| [0xF5u8, 0xEE, 0xDC, 0xFF])
        .collect();

    let report = compare_rgba(&baseline, &baseline, DEFAULT_YIQ_THRESHOLD);
    assert_eq!(report.different_pixels, 0, "相同图像必须零差异");
    assert!(
        report.passes(DEFAULT_MAX_DIFF_RATIO),
        "相同图像必须通过比例闸门：{}",
        report.summary()
    );
    assert_eq!(report.max_delta, 0.0);
}

/// 双闸门**不可互相替代**（R4 的设计前提）。
#[test]
fn two_gates_are_not_substitutable() {
    let count = 100usize;

    // 情形 A：1 个像素差到极限（per-pixel 抓，比例闸门会放行）
    let mut extreme = vec![0u8; 4 * count];
    let baseline = vec![0u8; 4 * count];
    // 涂白一个像素
    extreme[0] = 255;
    extreme[1] = 255;
    extreme[2] = 255;
    let report = compare_rgba(&baseline, &extreme, DEFAULT_YIQ_THRESHOLD);
    assert_eq!(
        report.different_pixels, 1,
        "极限差异的单个像素应被 per-pixel 抓"
    );
    assert!(
        report.passes(0.05),
        "1/100 的比例在 5% 闸门下应放行——证明两个闸门测的是不同的量"
    );

    // 情形 B：很多像素各差一点（比例闸门抓）
    let small: Vec<u8> = (0..count).flat_map(|_| [200u8, 200, 200, 0xFF]).collect();
    let report = compare_rgba(&baseline, &small, DEFAULT_YIQ_THRESHOLD);
    assert_eq!(
        report.different_pixels, count,
        "每个像素都差一点时，全部像素都应被计入"
    );
    assert!(
        !report.passes(DEFAULT_MAX_DIFF_RATIO),
        "全员小差异必须被比例闸门挡下：{}",
        report.summary()
    );
}

/// **SC-011 的口径被真正使用**：默认阈值的选用与文档一致。
///
/// ## 为什么不用"两个常量谁大"当判据
///
/// 那是编译期常量比较（clippy 的 `assertions_on_constants` 会指出它等价于
/// 断言一个字面量）——它测不出任何实现问题。有意义的判据是"默认的从严值
/// 真的在比对里生效"：用**刚好卡在两者之间**的差异，断言它在从严口径下
/// 报红、在官方口径下放行。这样"初期从严"是可观测的行为而非文档承诺。
#[test]
fn initial_ratio_is_actually_stricter_in_practice() {
    // 造一个差异比例在 (0.05%, 0.1%) 之间的图：10000 个像素里 7 个不同
    let total = 10_000usize;
    let baseline = vec![0u8; 4 * total];
    let mut patched = baseline.clone();
    for index in 0..7 {
        patched[index * 4] = 255;
        patched[index * 4 + 1] = 255;
        patched[index * 4 + 2] = 255;
    }

    let report = compare_rgba(&baseline, &patched, DEFAULT_YIQ_THRESHOLD);
    let ratio = report.ratio();
    assert!(
        ratio > DEFAULT_MAX_DIFF_RATIO && ratio < SPEC_MAX_DIFF_RATIO,
        "构造的差异比例 {ratio:.6} 应落在从严值 {DEFAULT_MAX_DIFF_RATIO:.4} 与\
         官方口径 {SPEC_MAX_DIFF_RATIO:.4} 之间"
    );

    // 从严口径：报红
    assert!(
        !report.passes(DEFAULT_MAX_DIFF_RATIO),
        "0.07% 的差异在初期从严的 0.05% 口径下应报红：{}",
        report.summary()
    );
    // 官方口径：放行
    assert!(
        report.passes(SPEC_MAX_DIFF_RATIO),
        "同一差异在 SC-011 的 0.1% 口径下应放行：{}",
        report.summary()
    );
}

// ===========================================================================
// 真实渲染的快照（有后端时）
// ===========================================================================

#[expect(
    clippy::panic,
    reason = "夹具内的 panic 是刻意的：非能力缺失的失败必须让测试红，不得被当成跳过"
)]
fn pipeline_or_skip(context: &str) -> Option<RenderPipeline> {
    match RenderPipeline::probe_backend() {
        Ok(backend) => Some(build_pipeline(backend)),
        Err(PipelineError::NoAdapter { reason }) => {
            println!("[T036] SKIPPED ({context}): 无可用图形后端——{reason}");
            println!("[T036] 本条**不计为通过**（FR-053 诚实跳过口径）");
            None
        }
        Err(other) => panic!("[T036] 后端探测以非'无适配器'的方式失败：{other}"),
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
    ]
    .join("\n")
}

#[expect(
    clippy::expect_used,
    reason = "夹具内的 expect 是刻意的：样本排版失败说明装置坏了，应当让测试红"
)]
fn render_sample(pipeline: &RenderPipeline, cache: &mut GlyphCache) -> RenderedImage {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(f32::from(WIDTH as u16), f32::from(HEIGHT as u16));
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");
    let batch = RenderBatch::for_chapter(&chapter, &geometry, &viewport, DayTheme.theme());

    OffscreenRenderer::new(WIDTH, HEIGHT)
        .expect("尺寸合法")
        .render(pipeline, &batch, cache)
        .expect("离屏渲染应成功")
}

/// **真实渲染的快照比对**：两次渲染的差异必须在阈值内（SC-011 的本机形态）。
///
/// ## 这条测的是 SC-011 的哪一部分
///
/// SC-011 的完整口径是"与**基线**的差异 ≤ 0.1%"，基线入 git。本用例先把
/// "同一构建内两次渲染"作为基线的代理——它验证的是**比对流程**与
/// **渲染稳定性**，而跨构建/跨平台的基线比对属 CI（T049）。
///
/// 这样拆的好处：本机就能验比对链路（不必先有入 git 的基线 PNG），
/// 而基线的生成与跨平台验证留在 CI 首次跑通时（那时才第一次有第二台机器）。
#[test]
fn snapshot_harness_compares_real_renders() {
    let Some(pipeline) = pipeline_or_skip("snapshot_harness_compares_real_renders") else {
        return;
    };

    let mut cache = GlyphCache::new();
    let first = render_sample(&pipeline, &mut cache);
    let second = render_sample(&pipeline, &mut cache);

    assert!(first.is_consistent() && second.is_consistent());

    let report = compare_rgba(&first.pixels, &second.pixels, DEFAULT_YIQ_THRESHOLD);
    println!("[T036] 真实渲染快照比对：{}", report.summary());

    assert!(
        report.passes(DEFAULT_MAX_DIFF_RATIO),
        "SC-011：同构建两次渲染的差异应 ≤ 初期从严的 0.05%：{}",
        report.summary()
    );
    // 自校验（本用例内的）：两次渲染都应有前景内容，否则"零差异"是平凡的
    let background = DayTheme.theme().palette().bg_reading;
    let background_rgb = [background.red, background.green, background.blue];
    assert!(
        first.foreground_pixels(background_rgb) > 0,
        "两次渲染都应有前景像素，否则'零差异'毫无意义"
    );
}

/// **不同内容必须报红**（真实渲染路径上的自校验）。
///
/// ## 为什么这条在真实路径上还要再验一次
///
/// 上面的自校验用的是**合成像素**（`vec![0xF5; …]`），它证明算法对。
/// 本条证明**接入真实渲染之后**仍然对：若 `render_sample` 因为某个 bug
/// 恒产出同一张图（如批次为空、或缓冲没接上），合成自校验会通过而快照
/// 永远"零差异"——那是典型的假绿。
///
/// 故用**不同的输入**（改主题 → 底色不同）断言 harness 报红。
#[test]
fn harness_detects_real_render_differences() {
    let Some(pipeline) = pipeline_or_skip("harness_detects_real_render_differences") else {
        return;
    };

    let mut cache = GlyphCache::new();
    let day = render_sample(&pipeline, &mut cache);

    // 夜主题：底色不同 → 必然大面积差异
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(f32::from(WIDTH as u16), f32::from(HEIGHT as u16));
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");
    let night_batch = RenderBatch::for_chapter(&chapter, &geometry, &viewport, NightTheme.theme());
    let night = OffscreenRenderer::new(WIDTH, HEIGHT)
        .expect("尺寸合法")
        .render(&pipeline, &night_batch, &mut cache)
        .expect("夜主题渲染应成功");

    let report = compare_rgba(&day.pixels, &night.pixels, DEFAULT_YIQ_THRESHOLD);
    println!("[T036] 昼夜差异（自校验）：{}", report.summary());
    assert!(
        !report.passes(DEFAULT_MAX_DIFF_RATIO),
        "昼夜主题的底色差异必须被检出（否则真实路径上的比对是失效的）：{}",
        report.summary()
    );
}

/// **无后端时显式报告 SKIPPED**（不计为通过）。
///
/// ## 这条断言什么
///
/// 判据的诚实性在**输出形态**上：`probe_backend_with(empty)` 必须给出
/// `NoAdapter`（→ 上层打印 SKIPPED），而不是某个"可用但空白"的结果。
/// 本文件其余用例据此字段显式跳过。
#[test]
fn no_backend_reports_skipped_not_passed() {
    let error = RenderPipeline::probe_backend_with(wgpu::Backends::empty())
        .expect_err("空后端集不该拿到适配器");

    assert!(
        matches!(error, PipelineError::NoAdapter { .. }),
        "无后端必须报 NoAdapter（上层据此打印 SKIPPED 而非通过），实测 {error:?}"
    );
    assert_eq!(
        error.capability_id(),
        Some(tome_reader::capability::OFFSCREEN_RENDERING)
    );
    println!("[T036] 无后端的报告形态：SKIPPED: {error}");
}
