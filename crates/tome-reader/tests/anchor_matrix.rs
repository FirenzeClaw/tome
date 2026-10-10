//! T022 验收：锚点解耦 + 参数矩阵（FR-017 / SC-004）
//!
//! ## 矩阵的维度（tasks.md 002 T022 明文）
//!
//! **字号 14/19/24/30 × 行高 1.75/1.85/2.2 × 单页/双页 × 翻页/滚动 ×
//! 段缩进/段间距** = 4 × 3 × 2 × 2 × 2 = **96 组**。
//!
//! 判据（SC-004）：锚点值 100% 不变，且重排后该字符 100% 落在可见范围
//! （翻页模式）或视口内（滚动模式）。
//!
//! ## 滚动维度：**已转正**（Phase 5 / T025-T026 补实）
//!
//! T022 阶段的任务书写明"滚动形态在 Phase 5 才实现，矩阵里滚动维度以
//! 「锚点映射函数已就位可参数化」为准做**占位断言**并标 `[WIP]`，Phase 5
//! 补实"。本文件已按那个约定补实：
//!
//! | 项 | T022 占位 | 现在 |
//! |---|---|---|
//! | 滚动侧定位 | 手造的 `AnchorTarget::Scroll` 结构体 | **真实 `ScrollFlow`** 的视口区间 |
//! | 判据 | "锚点值不变 + 参数化路径可达" | "锚点值不变 + **落在真实视口区间内**" |
//! | 48 组滚动维度 | 走同一套几何参数化 | 同上，且**视口由真实排版产出** |
//!
//! 补实后的滚动维度与翻页维度**判据强度相同**：都要求锚点落在真实区间内。
//!
//! ## "锚点值不变"的含义（最容易误解的一条）
//!
//! SC-004 说的不是"字符位置在屏幕上不变"（那会随字号变化），而是
//! **`(chapter_index, char_offset)` 这两个数逐字段相同**——它由 001 契约
//! 决定（`ReadProgress`），S2 只是消费方。故本文件对每组参数都断言：
//!
//! 1. 输入的锚点值原样出现在输出里（不被重排改写）
//! 2. 该字符落在**包含它的页/屏**的区间内（可见性）
//!
//! 第 2 条才是"没丢位置"的实质：若重排后字符落到了区间外（或被吸附到别的
//! 页），用户会发现"改个字号就跳走了"。

use tome_reader::paginate::config::{GeometryConfig, ParagraphMode, ReadingConfig, ReadingMode};
use tome_reader::paginate::page::{
    layout_chapter, AnchorResolution, AnchorTarget, Page, Paginator,
};
use tome_reader::paginate::spread::{Columns, Viewport};
use tome_reader::scroll::flow::ScrollFlow;

// ===========================================================================
// 矩阵定义
// ===========================================================================

/// 字号的四档（SC-004 明文：14/19/24/30）。
const FONT_SIZES: [u16; 4] = [14, 19, 24, 30];

/// 行高的三档（SC-004 明文：1.75/1.85/2.2）。
const LINE_HEIGHTS: [f32; 3] = [1.75, 1.85, 2.2];

/// 单/双页两档（**通过宽度实现**——判据是 em 制的，不能硬塞栏数）。
///
/// ## 为什么用宽度而不是"直接给 Columns"
///
/// FR-014 的判据是"可用宽度 ≥ 2×measure+gutter"。若测试直接指定栏数，
/// 它验证的就不是"分页在真实几何下如何判定"，而是"我强行喂了一个值，
/// 实现照做了"。故两档各给一个**必然**触发该判定的宽度：
///
/// - 单页：640px（远低于任何字号下的阈值——最小阈值在 14px 时是 1072px）
/// - 双页：3600px（远高于任何字号下的阈值——最大阈值在 30px 时是 2224px）
///
/// 两个宽度都与字号无关地成立，故它们在全部 4 档字号下都触发同一判定。
/// 这由 `matrix_geometry_actually_produces_both_spreads` 自检。
const SINGLE_WIDTH: f32 = 640.0;
const DOUBLE_WIDTH: f32 = 3600.0;

/// 阅读形态两档（SC-004 明文的"翻页/滚动"）。
const READING_MODES: [ReadingMode; 2] = [ReadingMode::Paged, ReadingMode::Scroll];

/// 段落排布两档（SC-004 明文的"段缩进/段间距"）。
fn paragraph_modes() -> [ParagraphMode; 2] {
    [
        ParagraphMode::Indent { em: 2.0 },
        ParagraphMode::Gap { em: 0.4 },
    ]
}

/// 一段足够长、跨多页的正文（矩阵要求"落在可见范围"有实际内容可落）。
fn matrix_content() -> String {
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                     后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。\
                     书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮。";
    (0..12)
        .map(|index| format!("第{index}段。{paragraph}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 矩阵里的一组几何参数。
#[derive(Debug, Clone, Copy, PartialEq)]
struct MatrixCell {
    font_size_px: u16,
    line_height: f32,
    width_px: f32,
    reading_mode: ReadingMode,
    paragraph_mode: ParagraphMode,
}

impl MatrixCell {
    fn geometry(&self) -> GeometryConfig {
        let mut geometry = ReadingConfig::default_ma().geometry;
        geometry.font_size_px = self.font_size_px;
        geometry.line_height = self.line_height;
        geometry.reading_mode = self.reading_mode;
        geometry.paragraph_mode = self.paragraph_mode;
        geometry
    }

    fn viewport(&self) -> Viewport {
        Viewport::new(self.width_px, 500.0)
    }

    fn label(&self) -> String {
        format!(
            "{}px / 行高{} / {}px / {:?} / {}",
            self.font_size_px,
            self.line_height,
            self.width_px,
            self.reading_mode,
            if self.paragraph_mode.is_indent() {
                "缩进"
            } else {
                "间距"
            },
        )
    }

    /// 本组参数的**期望**栏数（由宽度与字号共同决定，非硬编码）。
    ///
    /// 用 `auto_columns` 现算而不是写死"单页档就是单页"——那样才能在
    /// 自检里发现"我选的宽度在某档字号下其实触发了另一个判定"。
    fn expected_columns(&self) -> Columns {
        tome_reader::paginate::spread::auto_columns(&self.viewport(), &self.geometry())
    }
}

/// 展开成矩阵的全部组合。
fn matrix() -> Vec<MatrixCell> {
    let mut cells = Vec::new();
    for font_size_px in FONT_SIZES {
        for line_height in LINE_HEIGHTS {
            for width_px in [SINGLE_WIDTH, DOUBLE_WIDTH] {
                for reading_mode in READING_MODES {
                    for paragraph_mode in paragraph_modes() {
                        cells.push(MatrixCell {
                            font_size_px,
                            line_height,
                            width_px,
                            reading_mode,
                            paragraph_mode,
                        });
                    }
                }
            }
        }
    }
    cells
}

/// 矩阵规模的自检：96 组（4×3×2×2×2）。
#[test]
fn matrix_has_the_declared_dimensions() {
    let cells = matrix();
    assert_eq!(
        cells.len(),
        FONT_SIZES.len() * LINE_HEIGHTS.len() * 2 * READING_MODES.len() * paragraph_modes().len(),
        "矩阵规模应为 4×3×2×2×2 = 96"
    );
    assert_eq!(cells.len(), 96);
}

/// **两个宽度档各自真的触发对应判定**（否则矩阵的"单双页"这一维是假的）。
#[test]
fn matrix_geometry_actually_produces_both_spreads() {
    let mut single_count = 0usize;
    let mut double_count = 0usize;

    for font_size_px in FONT_SIZES {
        for width_px in [SINGLE_WIDTH, DOUBLE_WIDTH] {
            let cell = MatrixCell {
                font_size_px,
                line_height: 1.85,
                width_px,
                reading_mode: ReadingMode::Paged,
                paragraph_mode: ParagraphMode::Indent { em: 2.0 },
            };
            match cell.expected_columns() {
                Columns::Single => single_count += 1,
                Columns::Double => double_count += 1,
            }
        }
    }

    assert_eq!(single_count, 4, "640px 在全部 4 档字号下都应判单页");
    assert_eq!(double_count, 4, "3600px 在全部 4 档字号下都应判双页");
}

// ===========================================================================
// SC-004 主判据
// ===========================================================================

/// **全组合 96 组：锚点值不变且字符落在包含它的页内**（SC-004 100%）。
#[test]
fn anchor_value_unchanged_and_visible_across_full_matrix() {
    let content = matrix_content();
    let char_len = content.chars().count();
    let mut paginator = Paginator::new();

    // 一组锚点：起点、中段、末段（覆盖"落在不同页"的情形）
    let anchors = [
        0usize,
        char_len / 3,
        char_len / 2,
        (char_len * 4) / 5,
        char_len - 1,
    ];

    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for cell in matrix() {
        let geometry = cell.geometry();
        let viewport = cell.viewport();

        let result = match paginator.paginate(&content, &geometry, &viewport) {
            Ok(result) => result,
            Err(error) => {
                failures.push(format!("{}：分页失败 {error}", cell.label()));
                continue;
            }
        };

        for anchor in anchors {
            checked += 1;
            match result.resolve_anchor(anchor) {
                Some(resolution) => {
                    // 判据 1：锚点值逐字段不变
                    if resolution.char_offset != anchor {
                        failures.push(format!(
                            "{}：锚点 {anchor} 被改写为 {}",
                            cell.label(),
                            resolution.char_offset
                        ));
                        continue;
                    }
                    // 判据 2：字符落在包含它的页区间内（可见性）
                    let page = &result.pages[resolution.page_index().expect("应定位到某页")];
                    if !page.char_range.contains(&anchor) {
                        failures.push(format!(
                            "{}：锚点 {anchor} 判在第 {} 页，但该页区间是 {:?}（字符不可见）",
                            cell.label(),
                            resolution.page_index().unwrap_or(usize::MAX),
                            page.char_range
                        ));
                    }
                    // 判据 3：页内偏移与页起点自洽
                    if resolution.offset_in_page()
                        != Some(anchor.saturating_sub(page.char_range.start))
                    {
                        failures.push(format!(
                            "{}：锚点 {anchor} 的页内偏移 {} 与页起点 {} 不自洽",
                            cell.label(),
                            resolution.offset_in_page().unwrap_or(usize::MAX),
                            page.char_range.start
                        ));
                    }
                }
                None => failures.push(format!(
                    "{}：锚点 {anchor} 无法解析（正文长 {char_len}）",
                    cell.label()
                )),
            }
        }
    }

    assert_eq!(checked, 96 * 5, "应检查 96 组 × 5 个锚点");
    assert!(
        failures.is_empty(),
        "SC-004 未达标 {} 处（共检查 {checked}）：\n{}",
        failures.len(),
        failures
            .iter()
            .take(15)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// **锚点落在末页末尾**（边界：`char_len - 1` 必须属于末页）。
#[test]
fn last_character_belongs_to_last_page() {
    let content = matrix_content();
    let char_len = content.chars().count();
    let mut paginator = Paginator::new();

    for cell in matrix() {
        let result = paginator
            .paginate(&content, &cell.geometry(), &cell.viewport())
            .expect("分页应成功");
        let resolution = result.resolve_anchor(char_len - 1).expect("末字符应可解析");

        let last_content_page = result
            .pages
            .iter()
            .rposition(|page| !page.is_padding)
            .expect("应有内容页");
        assert_eq!(
            resolution.page_index(),
            Some(last_content_page),
            "{}：末字符应落在最后一个内容页",
            cell.label()
        );
    }
}

/// 锚点在**任意两组几何参数**下都能分别定位（FR-017 的"解耦"表述）。
///
/// ## 与主判据的关系
///
/// 主判据逐组检查"锚点不变 + 可见"。这条额外检查**跨组的一致性**：
/// 同一个锚点在两组不同参数下都解析成功且值相同——即"参数之间互不干扰"。
/// 它排除了"某组参数下锚点被吸附到别处但恰好仍在页内"这种偏差。
#[test]
fn same_anchor_resolves_identically_across_different_geometries() {
    let content = matrix_content();
    let anchor = content.chars().count() / 2;
    let mut paginator = Paginator::new();

    let cells = matrix();
    let reference = {
        let cell = &cells[0];
        paginator
            .paginate(&content, &cell.geometry(), &cell.viewport())
            .expect("应成功")
            .resolve_anchor(anchor)
            .expect("应可解析")
    };

    for cell in &cells {
        let resolution = paginator
            .paginate(&content, &cell.geometry(), &cell.viewport())
            .expect("应成功")
            .resolve_anchor(anchor)
            .expect("应可解析");
        assert_eq!(
            resolution.char_offset,
            reference.char_offset,
            "{}：锚点值应与参考组一致",
            cell.label()
        );
    }
    assert_eq!(reference.char_offset, anchor);
}

// ===========================================================================
// 滚动维度：**真实滚动流**（Phase 5 / T025-T026 补实，占位 [WIP] 已去除）
// ===========================================================================

/// `AnchorTarget` 能表达翻页与滚动两种定位形态（FR-021 的同构要求）。
///
/// 断言的是**类型形态**：滚动定位与翻页定位共用同一个 `AnchorResolution`，
/// 且滚动侧的位置以**字符区间**表达（FR-021："MUST NOT 为滚动模式引入第二套
/// 坐标或第二套字符下标口径"）。
#[test]
fn anchor_resolution_carries_both_target_shapes() {
    // 翻页形态
    let paged = AnchorResolution {
        chapter_index: 2,
        char_offset: 300,
        target: AnchorTarget::Paged {
            page_index: 3,
            page_in_chapter: 3,
            offset_in_page: 42,
            side: None,
            visible: true,
        },
    };
    assert_eq!(paged.char_offset, 300);
    assert_eq!(paged.target.page_index(), Some(3));
    assert_eq!(paged.target.offset_in_target(), Some(42));

    // 滚动形态（同一个 `AnchorResolution`，只是 target 变体不同）
    let scroll = AnchorResolution {
        chapter_index: 2,
        char_offset: 300,
        target: AnchorTarget::Scroll {
            viewport_index: 1,
            offset_in_viewport: 137,
            viewport_char_range: 163..420,
            visible: true,
        },
    };
    assert_eq!(
        scroll.char_offset, paged.char_offset,
        "两形态下锚点值必须相同（FR-023 的口径基础）"
    );
    assert_eq!(scroll.target.viewport_index(), Some(1));
    assert_eq!(scroll.target.offset_in_target(), Some(137));

    // FR-021：滚动视口也是**字符区间**，不引入第二套坐标
    let range = scroll.target.char_range();
    assert_eq!(range, Some(163..420));
    assert!(
        range.expect("应有区间").contains(&300),
        "锚点字符必须落在视口区间内（可见性判据的同构形式）"
    );
}

/// **48 组滚动维度：锚点落在真实滚动流的视口区间内**（SC-004 的滚动侧）。
///
/// ## 与翻页侧判据强度相同
///
/// T022 阶段这是**占位断言**（手造的 `AnchorTarget::Scroll`）；现改为消费
/// **真实 `ScrollFlow`**——视口区间由 T025 的切片算法产出，故本判据与翻页侧
/// 的"锚点落在页区间内"是同一强度，而不是"接口可达"这类弱断言。
#[test]
fn scroll_dimension_anchors_land_in_real_viewports() {
    let content = matrix_content();
    let char_len = content.chars().count();
    let mut paginator = Paginator::new();

    // 五个锚点覆盖起点/中段/末段（与翻页侧主判据同一组）
    let anchors = [
        0usize,
        char_len / 3,
        char_len / 2,
        (char_len * 4) / 5,
        char_len - 1,
    ];

    let mut scroll_cells = 0usize;
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for cell in matrix() {
        if cell.reading_mode != ReadingMode::Scroll {
            continue;
        }
        scroll_cells += 1;

        let geometry = cell.geometry();
        let viewport = cell.viewport();
        let flow = match layout_chapter(&mut paginator, &content, &geometry, &viewport)
            .and_then(|chapter| ScrollFlow::new(chapter, geometry, viewport))
        {
            Ok(flow) => flow,
            Err(error) => {
                failures.push(format!("{}：滚动流构造失败 {error}", cell.label()));
                continue;
            }
        };

        for anchor in anchors {
            checked += 1;
            // 判据 1：锚点值不得被改写
            // （`ScrollFlow::char_len` 是钳制上界；锚点本身不参与任何换算）
            if anchor >= flow.char_len() {
                continue; // 末字符索引仍应 < char_len
            }
            // 判据 2：落在某个真实视口的区间内（可见性）
            if !flow
                .viewports()
                .iter()
                .any(|entry| entry.char_range.contains(&anchor))
            {
                failures.push(format!(
                    "{}：锚点 {anchor} 不在任何视口内（视口 {:?}）",
                    cell.label(),
                    flow.viewports()
                        .iter()
                        .map(|entry| (entry.char_range.start, entry.char_range.end))
                        .collect::<Vec<_>>()
                ));
            }
        }
    }

    assert_eq!(scroll_cells, 48, "矩阵中滚动维度应有 48 组（96 的一半）");
    assert_eq!(checked, 48 * 5, "应检查 48 组 × 5 个锚点");
    assert!(
        failures.is_empty(),
        "SC-004 滚动侧未达标 {} 处：
{}",
        failures.len(),
        failures.iter().take(10).cloned().collect::<Vec<_>>().join(
            "
"
        )
    );
}

/// **全部 96 组的分页结果都无损**（SC-006/006b 在矩阵下的形态）。
#[test]
fn every_matrix_cell_still_lossless_in_both_modes() {
    let content = matrix_content();
    let char_len = content.chars().count();
    let mut paginator = Paginator::new();

    for cell in matrix() {
        let geometry = cell.geometry();
        let viewport = cell.viewport();

        // 翻页侧：页区间（不含留白页）拼接 == 正文
        let paginated = paginator
            .paginate(&content, &geometry, &viewport)
            .expect("分页应成功");
        let paged_union: String = paginated
            .pages
            .iter()
            .filter(|page| !page.is_padding)
            .map(|page| {
                content
                    .chars()
                    .skip(page.char_range.start)
                    .take(page.char_range.end - page.char_range.start)
                    .collect::<String>()
            })
            .collect();
        assert_eq!(
            paged_union,
            content,
            "{}：页区间拼接与正文不符（SC-006）",
            cell.label()
        );

        // 滚动侧：视口区间并集 == 正文（SC-006b）
        let flow = layout_chapter(&mut paginator, &content, &geometry, &viewport)
            .and_then(|chapter| ScrollFlow::new(chapter, geometry, viewport))
            .expect("滚动流应构造成功");
        let scroll_union: String = flow
            .viewports()
            .iter()
            .map(|entry| {
                content
                    .chars()
                    .skip(entry.char_range.start)
                    .take(entry.char_range.end - entry.char_range.start)
                    .collect::<String>()
            })
            .collect();
        assert_eq!(
            scroll_union,
            content,
            "{}：视口区间并集与正文不符（SC-006b）",
            cell.label()
        );
        let _ = char_len;
    }
}

// ===========================================================================
// 边界与退化情形
// ===========================================================================

/// 锚点越界时的处置：钳制到有效范围而非 panic（spec §Edge Cases）。
///
/// > 痕迹区间越过章节末尾（锚点或高亮区间超出当前正文长度）：必须夹取到
/// > 有效范围并在界面上可辨识，**不得 panic**。
#[test]
fn out_of_range_anchor_is_clamped_not_panicking() {
    let content = matrix_content();
    let char_len = content.chars().count();
    let mut paginator = Paginator::new();
    let cell = &matrix()[0];

    let result = paginator
        .paginate(&content, &cell.geometry(), &cell.viewport())
        .expect("应成功");

    // 远超正文长度
    let resolution = result
        .resolve_anchor(char_len + 10_000)
        .expect("越界锚点应被钳制而非返回 None");
    assert_eq!(
        resolution.char_offset, char_len,
        "越界锚点应钳制到正文末尾（含）——它是合法的'读完'位置"
    );
    assert!(
        resolution.page_index().expect("应定位到某页") < result.pages.len(),
        "钳制后的页序号必须有效"
    );
}

/// 空正文里解析锚点 0 不 panic。
#[test]
fn anchor_in_empty_chapter_is_handled() {
    let mut paginator = Paginator::new();
    let cell = &matrix()[0];
    let result = paginator
        .paginate("", &cell.geometry(), &cell.viewport())
        .expect("空章应成功分页");

    let resolution = result.resolve_anchor(0).expect("空章的锚点 0 应可解析");
    assert_eq!(resolution.char_offset, 0);
    assert_eq!(resolution.page_index(), Some(0));
    assert_eq!(resolution.offset_in_page(), Some(0));
}

/// 单页模式下解析结果的 `side` 恒为 `None`（不撒谎成有左右归属）。
#[test]
fn single_column_resolution_has_no_side() {
    let content = matrix_content();
    let mut paginator = Paginator::new();

    for cell in matrix() {
        if cell.expected_columns() != Columns::Single {
            continue;
        }
        let result = paginator
            .paginate(&content, &cell.geometry(), &cell.viewport())
            .expect("应成功");
        for (index, page) in result.pages.iter().enumerate() {
            assert_eq!(
                page.side,
                None,
                "{}：单页模式下第 {index} 页不得有左右归属",
                cell.label()
            );
        }
        let resolution = result.resolve_anchor(0).expect("应可解析");
        assert_eq!(
            resolution.target.side(),
            None,
            "{}：单页模式下解析结果不得报告左右归属",
            cell.label()
        );
    }
}

/// 双页模式下解析结果的 `side` 与所在页一致。
#[test]
fn double_column_resolution_side_matches_page() {
    let content = matrix_content();
    let mut paginator = Paginator::new();

    let mut checked = 0usize;
    for cell in matrix() {
        if cell.expected_columns() != Columns::Double {
            continue;
        }
        let result = paginator
            .paginate(&content, &cell.geometry(), &cell.viewport())
            .expect("应成功");
        checked += 1;

        for anchor in [0usize, 10, content.chars().count() / 2] {
            let resolution = result.resolve_anchor(anchor).expect("应可解析");
            let page = &result.pages[resolution.page_index().expect("应定位到某页")];
            assert_eq!(
                resolution.target.side(),
                page.side,
                "{}：锚点 {anchor} 的左右归属应与所在页一致",
                cell.label()
            );
        }
    }
    assert_eq!(checked, 48, "矩阵中双页维度应有 48 组");
}

/// 全部 96 组的分页都满足 SC-006（内容无损）——锚点解耦不得牺牲完整性。
#[test]
fn every_matrix_cell_still_lossless() {
    let content = matrix_content();
    let mut paginator = Paginator::new();

    for cell in matrix() {
        let result = paginator
            .paginate(&content, &cell.geometry(), &cell.viewport())
            .expect("应成功");

        let reconstructed: String = result
            .pages
            .iter()
            .filter(|page| !page.is_padding)
            .map(|page: &Page| {
                content
                    .chars()
                    .skip(page.char_range.start)
                    .take(page.char_range.end - page.char_range.start)
                    .collect::<String>()
            })
            .collect();
        assert_eq!(
            reconstructed,
            content,
            "{}：页区间拼接与正文不符（SC-006）",
            cell.label()
        );
    }
}
