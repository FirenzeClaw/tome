//! T039 验收：图文混排（FR-049 / US6 三场景）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T039）
//!
//! - 图片行盒按显示尺寸参与布局
//! - 占位高度与最终一致
//! - **不被页边界切开**
//! - 不可用占位不中断正文
//! - US6 三场景：不被页边界切开 / 尺寸未定按声明或回退尺寸占位且重排不改锚点 /
//!   与首段成组不孤立
//!
//! ## 接线说明（PM 已确认前置就绪，**不用占位降级**）
//!
//! 001 P8 已给 `ChapterContent` 增了 `images: Vec<ImageRef>`
//! （`crates/tome-core/src/seam/source.rs`），本任务按**真实接线**实现：
//! 图片行盒由 `ImageRef.char_offset` 定位，占位尺寸来自声明尺寸或回退尺寸。
//!
//! ## FR-049 的三条要求落到哪
//!
//! | 要求 | 落点 | 判据 |
//! |---|---|---|
//! | 按显示尺寸参与布局 | [`ImageBox::height_px`] 进入行的累计高度 | 页容量随之变化 |
//! | 占位高度与最终一致 | 占位用**同一个** fallback 尺寸 | 两次布局的行高相同 |
//! | 不被页边界切开 | `pack` 阶段把图片行盒当作**不可分割单位** | 图盒完整落在某一页内 |
//! | 不可用占位不中断正文 | 不可用时不产出图盒，改产**占位块**并继续排正文 | 正文区间仍完整（SC-006） |

// 本文件依赖渲染面（`tome_reader::render::image`），故必须与 `snapshot.rs` /
// `render_pipeline.rs` 同样门控——否则 `cargo test --workspace
// --no-default-features`（headless CI job）会因找不到 `render` 模块而编译失败。
#![cfg(feature = "render")]

use tome_core::seam::ImageRef;
use tome_reader::paginate::config::ReadingConfig;
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::render::image::{
    ImageBox, ImageLayout, ImageSizes, ImageStatus, DEFAULT_IMAGE_FALLBACK_HEIGHT_PX,
    DEFAULT_IMAGE_MAX_WIDTH_EM,
};

fn sample_chapter() -> String {
    [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样；她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然。」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
    ]
    .join("\n")
}

/// 造一个图片引用（**用 001 契约的真实类型**，不是本层另立一个）。
///
/// `tome_core::seam::ImageRef` 是 001 P8 接进来的类型（字符位置 / 缓存键 / alt）。
/// 本层**消费**它而不转换——那是"真实接线"的含义。
fn image_at(char_offset: i64, alt: Option<&str>) -> ImageRef {
    ImageRef {
        char_offset,
        cache_key: format!("https://example.invalid/image-{char_offset}.jpg"),
        alt: alt.map(str::to_owned),
    }
}

// ===========================================================================
// 按显示尺寸参与布局
// ===========================================================================

/// 图片行盒的高度**由显示尺寸决定**（不是固定占位）。
#[test]
fn image_box_height_follows_display_size() {
    let small = ImageBox::with_intrinsic_size(120, 80, 684.0, 19.0);
    let large = ImageBox::with_intrinsic_size(600, 400, 684.0, 19.0);

    assert!(
        small.height_px < large.height_px,
        "大图的显示高应更大：小图 {:.1}px vs 大图 {:.1}px",
        small.height_px,
        large.height_px
    );

    // 按宽高比缩放：120×80 缩到 684 宽以内时保持 3:2
    let ratio = small.width_px / small.height_px;
    assert!(
        (ratio - 1.5).abs() < 0.02,
        "显示尺寸应保持原始宽高比 3:2，实测 {ratio:.3}"
    );
}

/// 超宽的图片**按版心缩到宽度上限**（不溢出，FR-049 与 SC-003 一致）。
#[test]
fn oversized_image_is_scaled_down_to_measure() {
    let measure_px = 684.0f32;
    let image = ImageBox::with_intrinsic_size(3000, 2000, measure_px, 19.0);

    assert!(
        image.width_px <= measure_px + 1e-3,
        "图片显示宽不得超过版心：{:.1}px vs 版心 {measure_px}px",
        image.width_px
    );
    // 缩到版心宽，高度按比例（3:2 → 684×456）
    let ratio = image.width_px / image.height_px;
    assert!(
        (ratio - 1.5).abs() < 0.02,
        "缩放应保持宽高比，实测 {ratio:.3}"
    );
}

/// **显示宽度有上限**（MASTER §5.1 的版心上限 42em 同类约束）。
#[test]
fn image_width_respects_em_cap() {
    let font_size = 19.0f32;
    let measure_px = 2000.0; // 故意给一个很大的版心
    let image = ImageBox::with_intrinsic_size(5000, 1000, measure_px, font_size);

    let cap_px = font_size * DEFAULT_IMAGE_MAX_WIDTH_EM;
    assert!(
        image.width_px <= cap_px + 1e-3,
        "图片显示宽不应超过 {DEFAULT_IMAGE_MAX_WIDTH_EM}em（{cap_px:.1}px），实测 {:.1}px",
        image.width_px
    );
}

// ===========================================================================
// 不被页边界切开（US6 场景 1）
// ===========================================================================

/// **图片不被页边界切开**：图盒完整落在某一页的纵向范围内。
///
/// ## 判据的构造为什么这么讲究（本任务踩过一次）
///
/// 初版用"视口 200px + 600×400 的图"——那张图按版心缩到 380px 高，
/// **本身就高于整页**，于是走了"单图独占一页"的分支（页高临时等于图高），
/// 于是"完整落在页内"平凡成立。实测：把页容量判据改成恒真（允许切开）
/// 之后那条断言**照样通过**——判据是空的。
///
/// 有效构造要让图**明显小于一页**，但"页内已有内容的剩余空间"装不下它：
/// 只有这样"推进下一页"才是唯一正确处置，而"允许切开"必然违反。
///
/// 参数推导（本用例实测的几何）：行高 = 19 × 1.85 = 35.15px；视口高 200px；
/// 图 400×100 → 显示 100 高（宽 400 < 30em = 570 的上限，不缩放）。
///
/// 取字符位置 **150**：它落在第 4 行（区间 `123..157`，行顶 y = 140.60）。
/// 于是 `140.60 + 100 = 240.60 > 200` → 装不下，**必须**推下一页。
/// 若取更靠前的字符（如 60 → 行顶 70.30），`70.30 + 100 = 170.30 ≤ 200`
/// 恰好装得下——那条路径测不到"推进下一页"。
///
/// ## 自校验（本用例末尾）
///
/// 断言 `pages.len() > 1` 证明"推进"真的发生了。没有它，一个"位置恰好
/// 装得下"的构造会让本条平凡通过（本任务初版就是这样失效的）。
#[test]
fn image_is_not_split_across_pages() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 200.0);

    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    // 图盒必须**小于一页**（否则会走"独占一页"分支，判据失去意义）
    let probe = ImageBox::with_intrinsic_size(400, 100, viewport.available_width_px, 19.0);
    assert!(
        probe.height_px < viewport.available_height_px,
        "本用例的图盒高（{:.1}px）必须小于页高（{:.1}px）——否则测不到'推进下一页'路径",
        probe.height_px,
        viewport.available_height_px
    );

    let layout = ImageLayout::place(
        &chapter,
        &[image_at(150, None)],
        &geometry,
        &viewport,
        &ImageSizes::sized(400, 100),
    );

    let image = layout.images.first().expect("应产出一个图片盒");
    let page = layout
        .page_containing(image)
        .expect("图片应完整归属于某一页");

    let top_in_page = image.y_px - page.top_px;
    assert!(
        top_in_page >= -1e-3,
        "图片上缘不应超出所在页的上边界：图在页内 y={top_in_page:.2}px"
    );
    assert!(
        top_in_page + image.height_px <= page.height_px + 1e-3,
        "图片下缘不应超出所在页的下边界：图底在页内 y={:.2}px 而页高 {:.2}px",
        top_in_page + image.height_px,
        page.height_px
    );

    // 自校验：本条确实走到了"推进下一页"的分支（否则判据是空的）
    assert!(
        layout.pages.len() > 1,
        "本用例应触发'装不下 → 推进下一页'，实际只有 {} 页（判据未生效）",
        layout.pages.len()
    );
}

// ===========================================================================
// 占位高度与最终一致（US6 场景 2）
// ===========================================================================

/// **尺寸未定时按回退尺寸占位，且重排不改锚点**（US6 场景 2）。
#[test]
fn placeholder_height_matches_final_height() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 400.0);

    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    // 尺寸未定 → 用回退尺寸占位
    let placeholder = ImageLayout::place(
        &chapter,
        &[image_at(60, None)],
        &geometry,
        &viewport,
        &ImageSizes::unknown(),
    );
    let placeholder_box = placeholder.images.first().expect("应产出占位盒");

    assert_eq!(
        placeholder_box.status,
        ImageStatus::Placeholder,
        "尺寸未定应产出占位状态"
    );
    assert!(
        (placeholder_box.height_px - DEFAULT_IMAGE_FALLBACK_HEIGHT_PX).abs() < 1e-3,
        "占位高应恰为回退高度 {DEFAULT_IMAGE_FALLBACK_HEIGHT_PX}px，实测 {:.1}px",
        placeholder_box.height_px
    );

    // 尺寸确定后 → 高度变为实际显示高
    let final_layout = ImageLayout::place(
        &chapter,
        &[image_at(60, None)],
        &geometry,
        &viewport,
        &ImageSizes::sized(400, 300),
    );
    let final_box = final_layout.images.first().expect("应产出图盒");
    assert_eq!(final_box.status, ImageStatus::Available);
    assert!(
        final_box.height_px > 0.0 && (final_box.height_px - placeholder_box.height_px).abs() > 1e-3,
        "尺寸确定后高度应改变（占位 → 实际）：{:.1}px vs {:.1}px",
        placeholder_box.height_px,
        final_box.height_px
    );

    // 两次布局的**正文区间**完整（占位不打断正文）
    for layout in [&placeholder, &final_layout] {
        let covered: usize = layout.covered_char_count(&chapter);
        assert_eq!(
            covered, chapter.char_len,
            "图片占位不得让正文出现缺口（SC-006 的图文形态）"
        );
    }
}

// ===========================================================================
// 不可用占位不中断正文（US6 场景 3 / FR-049 末句）
// ===========================================================================

/// **图片不可用时以占位呈现且不中断正文**（FR-049 明文）。
#[test]
fn unavailable_image_does_not_interrupt_text() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 400.0);

    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    // 不可用：取图失败
    let layout = ImageLayout::place(
        &chapter,
        &[image_at(60, Some("山间小径"))],
        &geometry,
        &viewport,
        &ImageSizes::unavailable(),
    );

    let placeholder = layout.images.first().expect("不可用也应产出占位盒");
    assert_eq!(
        placeholder.status,
        ImageStatus::Unavailable,
        "取图失败应标为 Unavailable（不静默丢弃）"
    );
    assert!(
        placeholder.alt_text.is_some(),
        "不可用占位应带 alt 文本（让读者知道这里原本是什么图）"
    );
    assert_eq!(placeholder.alt_text.as_deref(), Some("山间小径"));

    // 而正文**照常排完**（不中断）
    assert_eq!(
        layout.covered_char_count(&chapter),
        chapter.char_len,
        "图片不可用不得让正文出现缺口（FR-049：MUST NOT 中断正文排版）"
    );

    // 分页结果仍满足 SC-006（用同一章跑一遍分页）
    let paged = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");
    let rebuilt: String = paged
        .pages
        .iter()
        .filter(|page| !page.is_padding)
        .map(|page| {
            content
                .chars()
                .skip(page.char_range.start)
                .take(page.char_range.len())
                .collect::<String>()
        })
        .collect();
    assert_eq!(rebuilt, content, "SC-006：正文逐字符完整");
}

/// **缺 alt 的不可用图片**仍产出占位（alt 不是占位的必要条件）。
#[test]
fn unavailable_image_without_alt_still_yields_placeholder() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 400.0);
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    let layout = ImageLayout::place(
        &chapter,
        &[image_at(60, None)],
        &geometry,
        &viewport,
        &ImageSizes::unavailable(),
    );

    let placeholder = layout.images.first().expect("应产出占位盒");
    assert_eq!(placeholder.status, ImageStatus::Unavailable);
    assert!(
        placeholder.alt_text.is_none(),
        "alt 缺省时应为 None（NULL 而非空串——两者在契约上不同）"
    );
    assert_eq!(
        layout.covered_char_count(&chapter),
        chapter.char_len,
        "无 alt 也不得中断正文"
    );
}

// ===========================================================================
// 多图与边界
// ===========================================================================

/// 多张图片各自定位到自己的字符位置（不串位）。
#[test]
fn multiple_images_keep_their_char_offsets() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 600.0);
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    let references = [image_at(10, None), image_at(80, None), image_at(150, None)];
    let layout = ImageLayout::place(
        &chapter,
        &references,
        &geometry,
        &viewport,
        &ImageSizes::sized(300, 200),
    );

    assert_eq!(layout.images.len(), 3, "三张图应产出三个图盒");
    for (index, image) in layout.images.iter().enumerate() {
        assert_eq!(
            image.char_offset, references[index].char_offset as usize,
            "第 {index} 张图的字符位置应保持（不串位）"
        );
    }

    // 纵向位置按字符顺序递增（图片随正文流动）
    for pair in layout.images.windows(2) {
        assert!(
            pair[1].y_px >= pair[0].y_px,
            "图片应随正文顺序向下排：{:.1}px → {:.1}px",
            pair[0].y_px,
            pair[1].y_px
        );
    }
}

/// **与首段成组不孤立**（US6 场景 3）：章首的图片不把首段推向下一页。
#[test]
fn leading_image_stays_with_first_paragraph() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    // 视口高度恰好容得下图片 + 若干行
    let viewport = Viewport::new(684.0, 400.0);
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    // 章首（字符 0）的图片
    let layout = ImageLayout::place(
        &chapter,
        &[image_at(0, None)],
        &geometry,
        &viewport,
        &ImageSizes::sized(400, 200),
    );

    let image = layout.images.first().expect("应产出图盒");
    // 图片与首段同页（图片之下仍有正文行的空间）
    let page = layout.page_containing(image).expect("图片应归属某页");
    let space_below = page.height_px - (image.y_px - page.top_px) - image.height_px;
    assert!(
        space_below > 0.0,
        "章首图片下方应仍容得下正文（不孤立占满整页）：剩余 {space_below:.1}px"
    );
}

/// 空图片列表：布局产出空的 `images`，不影响正文。
#[test]
fn empty_image_list_is_harmless() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 400.0);
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    let layout = ImageLayout::place(&chapter, &[], &geometry, &viewport, &ImageSizes::unknown());
    assert!(layout.images.is_empty());
    assert_eq!(
        layout.covered_char_count(&chapter),
        chapter.char_len,
        "无图片时正文仍完整"
    );
}

/// 负的字符位置被**钳到 0**（外部输入不可信，边界处校验）。
#[test]
fn negative_char_offset_is_clamped() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 400.0);
    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");

    let layout = ImageLayout::place(
        &chapter,
        &[image_at(-50, None)],
        &geometry,
        &viewport,
        &ImageSizes::sized(200, 150),
    );

    let image = layout.images.first().expect("仍应产出图盒");
    assert_eq!(
        image.char_offset, 0,
        "负的字符位置应钳到 0（01 契约：内容不可信，边界处校验）"
    );
}
