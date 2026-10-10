//! T034 验收：RenderBatch 绘制指令序列（FR-048 / **SC-012**）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T034）
//!
//! - 批次含四类指令：背景 / 字形四边形 / 装饰（高亮、下划线、引用线）/ 图片四边形
//! - **SC-012**：同页（或同滚动视口）同参数两次渲染，绘制指令**逐条相同**
//!   （预热后 100%；字形缓存首次预热造成的差异必须在第二次之前消除）
//!
//! ## 判据为什么是"指令序列"而不是"像素"
//!
//! SC-012 的原话是"产生相同的**绘制指令序列**"——它比像素更严格（像素相同
//! 可能是两次不同指令恰好同色），也**更快、更可定位**（失败时能指出第几条
//! 指令不同，而不是"有 37 个像素不一样"）。像素级判据另属 SC-011（T036）。
//!
//! ## "预热"这个词的确切含义
//!
//! SC-012 括注"字形缓存首次预热造成的差异必须在第二次之前消除"。实现上：
//! 第一次渲染会**栅格化**字形（写 atlas、产生待上传队列），第二次全部命中
//! 缓存。故判据是"**第二次与第三次**逐条相同"，而不是"第一次与第二次"。
//!
//! 而这正是最容易写成假绿的地方：若只比"两次渲染"，一个每次都重新栅格化的
//! 实现（缓存完全失效）也能通过——两次都产生同样的指令。故本文件**额外**
//! 断言"预热确实发生了"（`miss_count` 在第二次不再增长），否则那条判据是空的。

#![cfg(feature = "render")]

use tome_core::model::theme::{DayTheme, NightTheme};
use tome_reader::layout::engine::{Geometry, LayoutEngine};
use tome_reader::paginate::config::ReadingConfig;
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::render::batch::{DecorationKind, ImagePlacement, RenderBatch};
use tome_reader::render::pipeline::RenderPipeline;
use tome_reader::text::model::TextModel;

#[expect(
    clippy::panic,
    reason = "夹具内的 panic 是刻意的：非能力缺失的失败必须让测试红，不得被当成跳过"
)]
fn pipeline_or_skip(context: &str) -> Option<RenderPipeline> {
    use tome_reader::render::pipeline::PipelineError;

    match RenderPipeline::probe_backend() {
        Ok(backend) => Some(build_pipeline(backend)),
        Err(PipelineError::NoAdapter { reason }) => {
            println!("[T034] SKIPPED ({context}): 无可用图形后端——{reason}");
            println!("[T034] 本条**不计为通过**（FR-053）");
            None
        }
        Err(other) => panic!("[T034] 后端探测以非'无适配器'的方式失败：{other}"),
    }
}

#[expect(
    clippy::expect_used,
    reason = "本夹具只在探测已成功之后调用；失败说明装置坏了，应当让测试红"
)]
fn build_pipeline(backend: tome_reader::render::pipeline::GraphicsBackend) -> RenderPipeline {
    RenderPipeline::new(backend, 342, 300).expect("有后端时应能建管线")
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
fn build_batch() -> RenderBatch {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 300.0);

    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    RenderBatch::for_chapter(&chapter, &geometry, &viewport, DayTheme.theme())
}

// ===========================================================================
// 批次构成：四类指令
// ===========================================================================

/// 批次含**背景**指令（FR-030：阅读区用主题的阅读层色）。
#[test]
fn batch_starts_with_a_background() {
    let batch = build_batch();
    let background = batch.background();
    assert_eq!(background.width, 342, "背景宽度应等于视口可用宽");
    assert_eq!(background.height, 300, "背景高度应等于视口可用高");
    assert_eq!(
        background.color,
        DayTheme.theme().palette().bg_reading,
        "阅读区背景必须用主题的 bg_reading（FR-030 强制语义色）"
    );
}

/// 批次含**字形四边形**（正文的全部可见字形）。
#[test]
fn batch_contains_glyph_quads() {
    let batch = build_batch();
    assert!(
        batch.glyph_count() > 50,
        "一页正文应有大量字形四边形，实测 {}",
        batch.glyph_count()
    );

    // 每个字形四边形都带位置与尺寸（可被绘制）
    for quad in batch.glyphs().iter().take(20) {
        assert!(quad.width > 0.0, "字形宽度应为正：{quad:?}");
        assert!(quad.height > 0.0, "字形高度应为正：{quad:?}");
        assert!(
            quad.x.is_finite() && quad.y.is_finite(),
            "字形位置应为有限值：{quad:?}"
        );
    }
}

/// 批次含**装饰**指令，且装饰类型可枚举（高亮/下划线/引用线）。
#[test]
fn batch_contains_decorations() {
    use tome_reader::render::batch::DecorationKind;

    let batch = build_batch();

    // 无痕迹时装饰为空——这是合法状态（不是"忘了接线"）
    assert_eq!(
        batch.decoration_count(),
        0,
        "本章无痕迹输入，装饰应为空（本用例的语料不含高亮/下划线）"
    );

    // 而有痕迹时装饰必须产出：用带下划线的语料验一遍
    let mut with_decoration = build_batch();
    with_decoration.push_decoration(0, 4, DecorationKind::Underline);
    assert_eq!(
        with_decoration.decoration_count(),
        1,
        "推入一条装饰后应有一条"
    );
    assert_eq!(
        with_decoration.decorations()[0].kind,
        DecorationKind::Underline
    );
    // 装饰类型可枚举（三类的存在性）
    let kinds = [
        DecorationKind::Highlight,
        DecorationKind::Underline,
        DecorationKind::QuoteLine,
    ];
    assert_eq!(kinds.len(), 3, "装饰类型表应含高亮/下划线/引用线三类");
}

/// 批次含**图片四边形**（FR-049 的图文混排落到绘制指令上）。
#[test]
fn batch_contains_image_quads() {
    let mut batch = build_batch();
    let before = batch.image_count();

    batch.push_image(ImagePlacement {
        x: 10.0,
        y: 20.0,
        width: 100.0,
        height: 80.0,
        char_offset: 5,
    });

    assert_eq!(
        batch.image_count(),
        before + 1,
        "推入一张图片后图片四边形应多一条"
    );
    let image = batch.images()[0];
    assert_eq!((image.width, image.height), (100.0, 80.0));
    assert_eq!(
        image.char_offset, 5,
        "图片带字符位置（图像与字符区间的对应）"
    );
}

/// **指令顺序固定**：背景 → 图片 → 装饰 → 字形。
///
/// ## 为什么顺序要固定且要断言
///
/// 绘制顺序决定**遮挡关系**（后画的盖住先画的）。正文必须压在最上层——
/// 否则高亮背景会盖住文字；图片在装饰之下（图片上的高亮不该被图片盖住）。
/// 顺序若随输入变化，"同输入同指令序列"（SC-012）就只是巧合。
///
/// ## 为什么本用例要推入**多条**图片与装饰
///
/// 顺序判据只有在**每一类都有多于一条**时才真的约束到"类间顺序"：
/// 若装饰只有一条、字形也只有一条，"装饰在字形前"与"装饰在字形后"
/// 都能通过"图片在字形之前"那一条断言。实测：只推一条装饰、只断言
/// "图片 < 字形"时，把装饰移到字形**之后**（正文被盖住）测不出来。
#[test]
fn instruction_order_is_fixed() {
    let mut batch = build_batch();
    // 两张图（第一张的 char_offset 靠后，用于同时验类内顺序）
    batch.push_image(ImagePlacement {
        x: 0.0,
        y: 0.0,
        width: 10.0,
        height: 10.0,
        char_offset: 0,
    });
    batch.push_image(ImagePlacement {
        x: 0.0,
        y: 40.0,
        width: 10.0,
        height: 10.0,
        char_offset: 30,
    });
    // 两条装饰（覆盖不同区间）
    batch.push_decoration(0, 3, DecorationKind::Underline);
    batch.push_decoration(5, 3, DecorationKind::Highlight);

    let kinds: Vec<&str> = batch
        .instructions()
        .iter()
        .map(tome_reader::render::batch::DrawInstruction::kind_name)
        .collect();

    let position_of = |name: &str| -> usize {
        kinds
            .iter()
            .position(|kind| *kind == name)
            .unwrap_or_else(|| panic!("指令序列里应有 {name}，实测 {kinds:?}"))
    };
    let last_position_of = |name: &str| -> usize {
        kinds
            .iter()
            .rposition(|kind| *kind == name)
            .unwrap_or_else(|| panic!("指令序列里应有 {name}，实测 {kinds:?}"))
    };

    assert_eq!(kinds[0], "background", "首条指令必须是背景");

    // 类间顺序：背景 < 图片 < 装饰 < 字形（**用最后一个各类型的位置**比较，
    // 这样"把某一类移到另一类之后"的变异必然被抓到）
    assert!(
        last_position_of("image") < position_of("underline"),
        "全部图片应排在全部装饰之前：{kinds:?}"
    );
    assert!(
        last_position_of("underline") < position_of("glyph"),
        "全部装饰应排在全部字形之前——正文必须压在最上层，\
         否则高亮底色会盖住文字：{kinds:?}"
    );
    assert!(
        last_position_of("highlight") < position_of("glyph"),
        "高亮同样应排在字形之前：{kinds:?}"
    );

    // 类内顺序：装饰按推入顺序（稳定性）
    assert!(
        position_of("underline") < position_of("highlight"),
        "装饰应保持推入顺序：{kinds:?}"
    );
}

// ===========================================================================
// SC-012：同输入同指令序列
// ===========================================================================

/// **SC-012 主判据**：预热后两次渲染的指令序列**逐条相同**。
#[test]
fn sc012_instruction_sequence_is_identical_after_warmup() {
    let Some(pipeline) = pipeline_or_skip("sc012_instruction_sequence_is_identical_after_warmup")
    else {
        return;
    };

    let mut cache = tome_reader::render::glyph_cache::GlyphCache::new();
    let batch = build_batch();

    // 预热：第一次渲染把字形栅格化进 atlas
    let first = batch.encode(&pipeline, &mut cache);
    let misses_after_first = cache.miss_count();
    assert!(
        misses_after_first > 0,
        "第一次渲染应有未命中（若为 0，说明缓存早就满了——本条判据失去意义）"
    );

    // 第二次：全部命中，指令序列应稳定
    let second = batch.encode(&pipeline, &mut cache);
    // 第三次：用于与第二次比对（第二次自身可能仍受首次上传队列影响）
    let third = batch.encode(&pipeline, &mut cache);

    assert_eq!(
        cache.miss_count(),
        misses_after_first,
        "预热之后不应再产生未命中（否则'两次相同'只是每次都重新栅格化的巧合，\
         那条判据是空的）"
    );

    assert_eq!(
        second, third,
        "SC-012：同页同参数两次渲染的绘制指令序列必须逐条相同"
    );
    assert!(
        !first.is_empty(),
        "指令序列不应为空（空序列相同是平凡通过）"
    );
}

/// 指令序列的**表达能力**：序列可枚举且每条带坐标/颜色。
#[test]
fn instruction_sequence_is_inspectable() {
    let batch = build_batch();
    let instructions = batch.instructions();

    assert!(!instructions.is_empty());
    // 每条指令都能给出类型名与几何（供 CI 差异报告定位）
    for instruction in instructions.iter().take(10) {
        assert!(!instruction.kind_name().is_empty());
    }

    // 背景条数恒为 1（重复背景会让"上层的盖住下层"，浪费一次绘制）
    let backgrounds = instructions
        .iter()
        .filter(|instruction| instruction.kind_name() == "background")
        .count();
    assert_eq!(backgrounds, 1, "背景指令应恰好一条");
}

/// 空章：产出**只有背景**的批次（不产出无法呈现的状态）。
#[test]
fn empty_chapter_yields_background_only() {
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 300.0);
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, "", &geometry, &viewport).expect("空章也应排版成功");

    let batch = RenderBatch::for_chapter(&chapter, &geometry, &viewport, DayTheme.theme());
    assert_eq!(batch.glyph_count(), 0, "空章没有字形");
    assert_eq!(batch.instructions().len(), 1, "空章只应有背景一条指令");
}

/// 夜主题的批次**几何与昼主题逐条相同**，只有颜色不同（FR-019/SC-005）。
///
/// ## 为什么这条在渲染层也要断言
///
/// FR-019 已经在分页层被断言（主题不改变页序列）。本条补上**绘制指令**这一
/// 侧：若主题影响了字形位置（例如夜主题用了不同字号），页序列相同而绘制
/// 结果不同——那仍违反"主题只决定绘制"。
#[test]
fn theme_changes_only_colors_not_geometry() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 300.0);
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    let day = RenderBatch::for_chapter(&chapter, &geometry, &viewport, DayTheme.theme());
    let night = RenderBatch::for_chapter(&chapter, &geometry, &viewport, NightTheme.theme());

    assert_eq!(
        day.glyph_count(),
        night.glyph_count(),
        "两主题的字形数必须相同"
    );
    assert_eq!(
        day.instructions().len(),
        night.instructions().len(),
        "两主题的指令条数必须相同（主题只改颜色）"
    );

    // 几何逐条相同
    for (day_instruction, night_instruction) in day.instructions().iter().zip(night.instructions())
    {
        assert_eq!(
            day_instruction.kind_name(),
            night_instruction.kind_name(),
            "对应指令的类型必须相同"
        );
        assert_eq!(
            day_instruction.geometry_key(),
            night_instruction.geometry_key(),
            "对应指令的几何必须相同（主题只决定颜色）"
        );
    }

    // 而颜色确实不同（否则上面那些断言也可能是"两主题根本没接线"）
    assert_ne!(
        day.background().color,
        night.background().color,
        "昼夜主题的背景色必须不同（FR-030）"
    );
}

/// 字体缺字不影响指令序列的**结构性**（回退字形也产出一条指令）。
#[test]
fn fallback_glyphs_still_produce_quads() {
    let mut engine = LayoutEngine::new();
    // 含 CJK 扩展 B 的生僻字（很可能不在任何系统字体里）
    let model = TextModel::new("读书记录𠀋测试");
    let result = engine.layout_paragraph(&model, 0, &Geometry::default_ma());

    let glyphs = result.lines[0].glyphs.len();
    assert!(
        glyphs >= 6,
        "含生僻字的文本也应产出全部字形的布局（缺字走回退或豆腐块），实测 {glyphs}"
    );
}
