//! [DONE] S2 排版层的 CLI 薄壳（T048 的先行部分 / `contracts/cli.md` §2.1）
//!
//! ## 薄壳的含义
//!
//! 本模块**不重复实现任何排版逻辑**——它只做四件事：读入正文、把 CLI
//! 选项映射成 `tome-reader` 的几何参数、调用排版引擎、把结果序列化为
//! `contracts/cli.md` §2.1 定义的 JSON 形态。
//!
//! ## 为什么放在 tome-cli 而不是 tome-reader
//!
//! CLI 是**组合根**（plan.md 的依赖图）：只有它允许依赖全部实现 crate。
//! `tome-reader` 是库，它不该知道"命令行"这种交互形态——那会让库的
//! 依赖面被 CLI 关切污染（宪法 IV 的单向依赖）。
//!
//! ## 正文来源（本阶段的诚实标注）
//!
//! `contracts/cli.md` 定义的入口是 `--chapter <book>#<index>`，要求该书
//! 在书架里。那需要 001 的 `BookRepository` 与本地书扫描**接线**，
//! 而 S2-1 阶段的排版层尚未接线（属 S3 与 001 的工作）。
//!
//! 故本阶段提供 **`--text <file>`** 直接给一份正文文件——它是
//! **Indendent Test 的载体**（任务书要求 `tome layout` 出契约形态 JSON），
//! 但不改变 `--chapter` 的契约地位。`--chapter` 的接线标 `[WIP]`。

use std::path::Path;

use tome_core::ErrorKind;
use tome_reader::layout::linebox::LineBox;
use tome_reader::paginate::config::{
    GeometryConfig, ParagraphMode, ReadingConfig, ReadingMode, SpreadMode,
};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::{ScrollFlow, ScrollViewport};
use tome_reader::text::paragraph;

use crate::commands::CommandOutcome;
use crate::flags::{Command, GlobalOptions, OutputFormat};

/// CLI 的默认阅读区可用宽高（`contracts/cli.md` §2.1 的示例 1180×800）。
///
/// 它们**不是** `GeometryConfig` 的字段——几何子集描述版心与字号，可用尺寸
/// 来自外壳布局（FR-014 的口径：扣侧栏与内边距之后）。
const DEFAULT_AVAILABLE_WIDTH_PX: f32 = 1180.0;
const DEFAULT_AVAILABLE_HEIGHT_PX: f32 = 800.0;

/// `layout` 与 `layout-anchor` 共用的**正文来源解析**。
///
/// ## 两个来源
///
/// - `--text <file>`：直接给一份正文（本阶段的验收入口）
/// - `--chapter <book>#<n>`：书架形式（契约定义的形式，待 001 仓库接线）
///
/// 返回 `(chapter_index, 正文)`；失败时给出**已构造好的 `CommandOutcome`**，
/// 调用方直接返回它（避免每个调用点重复写错误映射）。
fn resolve_chapter_text(
    chapter: &Option<String>,
    text_file: &Option<std::path::PathBuf>,
) -> Result<(usize, String), CommandOutcome> {
    match (text_file, chapter) {
        (Some(path), _) => read_text(path).map(|text| (0usize, text)),
        (None, Some(reference)) => Err(CommandOutcome::failure(
            &ErrorKind::unsupported_capability(tome_core::ErrorCapability::new(
                "chapter-by-bookshelf",
            )),
            &format!(
                "--chapter `{reference}` 需要书架接线（属 S3 与 001 的仓库职责）；\
                 本阶段请用 `--text <file>` 直接给正文"
            ),
        )),
        (None, None) => Err(CommandOutcome::failure(
            &ErrorKind::Configuration,
            "需要一个正文来源：`--text <file>`（本阶段）或 `--chapter <book>#<n>`（待接线）",
        )),
    }
}

/// CLI 选项 → **几何参数 + 视口**（两子命令共用的映射）。
///
/// ## 版心与分栏的计算归属
///
/// 全部由 `tome_reader::paginate` 负责（`spread::column_measure_px` 与
/// `resolve_columns`）。CLI **不重复实现**它们——只把选项映射成
/// `GeometryConfig` 与 `Viewport`，然后回显结果（薄壳，宪法 IV 的 DRY）。
///
/// ## 为什么从 `ReadingConfig` 出发而不是手搓
///
/// 默认值与白名单校验都是 schema 的一部分，CLI 不该有第二份；选项只做覆盖。
#[allow(clippy::too_many_arguments)]
fn resolve_geometry(
    width: Option<f32>,
    height: Option<f32>,
    font_size: Option<f32>,
    line_height: Option<f32>,
    paragraph_mode: Option<&str>,
    measure_em: Option<f32>,
    gutter: Option<f32>,
    spread: Option<&str>,
    mode: Option<&str>,
) -> Result<(GeometryConfig, Viewport), CommandOutcome> {
    let mut available_width_px = DEFAULT_AVAILABLE_WIDTH_PX;
    let mut available_height_px = DEFAULT_AVAILABLE_HEIGHT_PX;
    let mut geometry = ReadingConfig::default_ma().geometry;

    if let Some(value) = width {
        available_width_px = value;
    }
    if let Some(value) = height {
        available_height_px = value;
    }
    if let Some(value) = font_size {
        // `f32` → `u16`：`flags` 已在校验阶段拒绝非整数与越界值（退出码 2），
        // 故此处的转换不会丢信息；仍走一次 `round` 以免浮点表示误差。
        geometry.font_size_px = value.round().max(0.0) as u16;
    }
    if let Some(value) = line_height {
        geometry.line_height = value;
    }
    if let Some(value) = measure_em {
        geometry.measure_em = value;
    }
    if let Some(value) = gutter {
        geometry.gutter_px = value.round().max(0.0) as u16;
    }
    if let Some(mode) = paragraph_mode {
        geometry.paragraph_mode = match mode {
            "gap" => ParagraphMode::Gap { em: 0.4 },
            _ => ParagraphMode::Indent { em: 2.0 },
        };
    }
    if let Some(mode) = spread {
        geometry.spread_mode = match mode {
            "double" => SpreadMode::ForceSpread,
            "single" => SpreadMode::ForceSingle,
            _ => SpreadMode::Auto,
        };
    }
    geometry.reading_mode = match mode {
        Some("scroll") => ReadingMode::Scroll,
        _ => ReadingMode::Paged,
    };

    // 白名单校验：非法几何报结构化错误（与 `flags` 的用法错误区分开——
    // 那是"参数写错"，这是"参数组合非法"）
    if let Err(field) = geometry.validate() {
        return Err(CommandOutcome::failure(
            &ErrorKind::Configuration,
            &format!("几何参数 `{field}` 非法（见 ReadingConfig 的白名单范围）"),
        ));
    }

    Ok((
        geometry,
        Viewport::new(available_width_px, available_height_px),
    ))
}

/// 一个滚动视口的 JSON 形态（`contracts/cli.md` §2.1 的 `scroll.viewports[]`）。
fn scroll_viewport_to_json(flow: &ScrollFlow, viewport: &ScrollViewport) -> serde_json::Value {
    serde_json::json!({
        "viewport_index": viewport.viewport_index,
        "top": viewport.top_px,
        "height": viewport.height_px,
        "char_start": viewport.char_range.start,
        "char_len": viewport.char_len(),
        "lines": flow.viewport_lines(viewport).iter().map(|page_line| serde_json::json!({
            "char_start": page_line.chapter_char_range.start,
            "char_len": page_line.chapter_char_range.end - page_line.chapter_char_range.start,
            "paragraph_index": page_line.line.paragraph_index,
        })).collect::<Vec<_>>(),
    })
}

/// `layout` 子命令的实现（`contracts/cli.md` §2.1）。
///
/// ## 输出形态
///
/// 见 `contracts/cli.md` §2.1 的 JSON 示例：`chapter_index` / `mode` /
/// `geometry` / `paragraphs` / `pages`（`--mode paginated`）/
/// `scroll.viewports`（`--mode scroll`）/ `rule_exceptions` / `anchor_hit`。
///
/// ## 两种形态共用一条管线
///
/// 分页与滚动都经 `paginate::page::layout_chapter` 取行盒（FR-020/021 的
/// "同一排版结果"），只是之后的切法不同。故排版只做一次，按 `--mode` 选择
/// 切法。
pub fn run_layout(options: &GlobalOptions, command: &Command) -> CommandOutcome {
    let Command::Layout(args) = command else {
        return CommandOutcome::failure(
            &ErrorKind::Configuration,
            "run_layout 收到的不是 Layout 命令",
        );
    };

    // 解构参数（借用，不移动——`command` 是 `&Command`）
    let crate::flags::LayoutArgs {
        chapter,
        text_file,
        mode,
        width,
        height,
        font_size,
        line_height,
        paragraph_mode,
        measure_em,
        gutter,
        spread,
        anchor,
        anchor_char: _,
        target_mode: _,
    } = args;

    let (chapter_index, text) = match resolve_chapter_text(chapter, text_file) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };

    let (geometry, viewport) = match resolve_geometry(
        *width,
        *height,
        *font_size,
        *line_height,
        paragraph_mode.as_deref(),
        *measure_em,
        *gutter,
        spread.as_deref(),
        mode.as_deref(),
    ) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };

    // --- 排版与分页（一条管线：`paginate` 内部含逐段排版）---
    let mut paginator = Paginator::new();
    let paginated = match paginator.paginate(&text, &geometry, &viewport) {
        Ok(result) => result,
        Err(error) => {
            return CommandOutcome::failure(&error.kind(), &format!("分页失败：{error}"));
        }
    };

    // 段落报告：从页序列里还原行盒（`Paginator` 已把段内区间换算成章内区间）
    let split = paragraph::split(&text);
    let mut paragraph_reports = Vec::new();
    for para in &split.paragraphs {
        let lines: Vec<serde_json::Value> = paginated
            .pages
            .iter()
            .flat_map(|page| page.lines.iter())
            .filter(|page_line| page_line.line.paragraph_index == para.index)
            .map(|page_line| line_to_json(&page_line.line))
            .collect();
        paragraph_reports.push(serde_json::json!({
            "paragraph_index": para.index,
            "char_start": para.char_start,
            "char_len": para.char_len(),
            "from_heuristic": para.from_heuristic,
            "lines": lines,
        }));
    }

    let all_exceptions = &paginated.rule_exceptions;
    let degraded_words = paginated.degraded_words;

    // --- 页序列（T019/T021：真实页，不再是空数组）---
    let pages_json: Vec<serde_json::Value> = paginated
        .pages
        .iter()
        .map(|page| {
            serde_json::json!({
                "page_index": page.page_in_chapter,
                "char_start": page.char_range.start,
                "char_len": page.char_len(),
                "is_padding": page.is_padding,
                "side": page.side.map(|side| side.as_str()),
                "lines": page.lines.iter().map(|page_line| serde_json::json!({
                    // 契约 §2.1 的 `char_start` 在页内是**章内**口径
                    "char_start": page_line.chapter_char_range.start,
                    "char_len": page_line.chapter_char_range.end - page_line.chapter_char_range.start,
                    "paragraph_index": page_line.line.paragraph_index,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    // --- 滚动流（`--mode scroll`；与分页共用同一份行盒，FR-020/021）---
    //
    // 用 `layout_chapter` 单独取一份排版结果而非从 `paginated.pages` 还原：
    // 前者是**唯一排版入口**（FR-021 的"不引入第二套坐标"），且流需要完整的
    // 行盒序列（页里只有被切出来的那些）。
    let scroll_json = if mode.as_deref() == Some("scroll") {
        let mut scroll_paginator = Paginator::new();
        match layout_chapter(&mut scroll_paginator, &text, &geometry, &viewport)
            .and_then(|chapter| ScrollFlow::new(chapter, geometry, viewport))
        {
            Ok(flow) => serde_json::json!({
                "content_height": flow.content_height_px(),
                "measure_px": flow.measure_px(),
                "viewport_count": flow.viewports().len(),
                "viewports": flow
                    .viewports()
                    .iter()
                    .map(|viewport| scroll_viewport_to_json(&flow, viewport))
                    .collect::<Vec<_>>(),
            }),
            Err(error) => {
                return CommandOutcome::failure(&error.kind(), &format!("滚动流构造失败：{error}"));
            }
        }
    } else {
        // 非滚动形态：`viewports` 为空是**准确的**（这次没算滚动流），
        // 不是"未实现"的占位。
        serde_json::json!({
            "content_height": serde_json::Value::Null,
            "viewport_count": 0,
            "viewports": [],
        })
    };

    // --- 锚点命中（`--anchor` 时）---
    //
    // 定位依据随形态而变：滚动形态报视口位置（`AnchorTarget::Scroll` 的口径），
    // 翻页形态报页位置。两者共用同一个 `char_offset`（FR-021 的同构）。
    let anchor_hit = anchor.map(|offset| match mode.as_deref() {
        Some("scroll") => {
            // 滚动形态下在流里定位
            let mut scroll_paginator = Paginator::new();
            match layout_chapter(&mut scroll_paginator, &text, &geometry, &viewport)
                .and_then(|chapter| ScrollFlow::new(chapter, geometry, viewport))
            {
                Ok(flow) => {
                    let clamped = offset.min(flow.char_len());
                    let owner = flow
                        .viewports()
                        .iter()
                        .find(|viewport| viewport.char_range.contains(&clamped));
                    match owner {
                        Some(viewport) => serde_json::json!({
                            "char_offset": clamped,
                            "viewport_index": viewport.viewport_index,
                            "offset_in_viewport": clamped - viewport.char_range.start,
                            "visible": true,
                        }),
                        None => serde_json::json!({
                            "char_offset": clamped,
                            "viewport_index": serde_json::Value::Null,
                            "offset_in_viewport": serde_json::Value::Null,
                            "visible": false,
                        }),
                    }
                }
                Err(error) => serde_json::json!({
                    "char_offset": offset,
                    "viewport_index": serde_json::Value::Null,
                    "offset_in_viewport": serde_json::Value::Null,
                    "visible": false,
                    "error": error.message_id(),
                }),
            }
        }
        _ => match paginated.resolve_anchor(offset) {
            Some(resolution) => serde_json::json!({
                "char_offset": resolution.char_offset,
                "page_index": resolution.page_index(),
                "offset_in_page": resolution.offset_in_page(),
                "visible": resolution.is_visible(),
            }),
            None => serde_json::json!({
                "char_offset": offset,
                "page_index": serde_json::Value::Null,
                "offset_in_page": serde_json::Value::Null,
                "visible": false,
            }),
        },
    });

    let payload = serde_json::json!({
        "chapter_index": chapter_index,
        "mode": mode.as_deref().unwrap_or("paginated"),
        "geometry": {
            "font_size": geometry.font_size_px,
            "line_height": geometry.line_height,
            "measure_em": geometry.measure_em,
            "gutter": geometry.gutter_px,
            "width": viewport.available_width_px,
            "height": viewport.available_height_px,
            "paragraph_mode": paragraph_mode.as_deref().unwrap_or("indent"),
            "spread": spread.as_deref().unwrap_or("auto"),
            "columns": paginated.spread.columns.count(),
            "measure_px": paginated.spread.measure_px,
            "override_active": paginated.spread.override_active,
        },
        "paragraphs": paragraph_reports,
        "pages": pages_json,
        "scroll": scroll_json,
        "rule_exceptions": all_exceptions.iter().map(|record| serde_json::json!({
            "char_offset": record.char_index,
            "rule": record.rule_id,
            "reached_step": format!("{:?}", record.reached_step),
        })).collect::<Vec<_>>(),
        "degraded_words": degraded_words,
        "text_split": {
            "split_mode": format!("{:?}", split.diagnostic.split_mode),
            "newline_count": split.diagnostic.newline_count,
            "leading_space_hits": split.diagnostic.leading_space_hits,
            "heuristic_applied": split.diagnostic.heuristic_applied,
        },
        "anchor_hit": anchor_hit,
    });

    match options.format {
        OutputFormat::Json => CommandOutcome::with_stdout(format!("{payload:#}")),
        OutputFormat::Human => {
            let mut out = String::new();
            out.push_str(&format!(
                "章节 {} · {} · 版心 {:.0}px · {} 段\n",
                chapter_index,
                payload["mode"].as_str().unwrap_or("paginated"),
                paginated.spread.measure_px,
                split.paragraphs.len()
            ));
            let total_lines: usize = paragraph_reports
                .iter()
                .map(|para| para["lines"].as_array().map_or(0, std::vec::Vec::len))
                .sum();
            out.push_str(&format!("共 {total_lines} 行\n"));
            out.push_str(&format!(
                "共 {} 页（{} 栏）\n",
                paginated.pages.len(),
                paginated.spread.columns.count()
            ));
            if !all_exceptions.is_empty() {
                out.push_str(&format!("禁则例外 {} 条\n", all_exceptions.len()));
            }
            if degraded_words > 0 {
                out.push_str(&format!("超长词退化 {degraded_words} 处\n"));
            }
            CommandOutcome::with_stdout(out)
        }
    }
}

/// 把一个行盒序列化为 `contracts/cli.md` §2.1 的行形态。
fn line_to_json(line: &LineBox) -> serde_json::Value {
    serde_json::json!({
        "char_start": line.char_range.start,
        "char_len": line.char_len(),
        "paragraph_index": line.paragraph_index,
        "width_px": line.content_width_px(),
        "height_px": line.height_px,
        "baseline_px": line.baseline_px,
        "squeeze_px": line.squeeze_px,
        "justify_extra_px": line.justify_extra_px,
        "glyph_count": line.glyphs.len(),
    })
}

/// 读正文文件（错误映射为**结构化**的 CLI 结果，不 panic）。
fn read_text(path: &Path) -> Result<String, CommandOutcome> {
    std::fs::read_to_string(path).map_err(|error| {
        CommandOutcome::failure(
            &ErrorKind::Content,
            &format!("读取正文文件失败 {}：{error}", path.display()),
        )
    })
}

/// `layout-anchor` 子命令的实现（`contracts/cli.md` §2.2）。
///
/// ## 输出形态
///
/// 契约 §2.2 要求给出指定字符在**两种形态**下的定位，以及锚点值是否不变：
///
/// ```jsonc
/// {
///   "char_offset": 300,
///   "from": { "mode": "scroll", "viewport_index": 0, "offset_in_viewport": 300, "visible": true },
///   "to":   { "mode": "paginated", "page_index": 0, "offset_in_page": 300, "visible": true },
///   "anchor_value_unchanged": true
/// }
/// ```
///
/// ## 判据（SC-005b）
///
/// `anchor_value_unchanged` MUST 为 `true`，且 `to.visible` MUST 为 `true`。
///
/// ## `from` 与 `to` 的来处
///
/// `--mode` 指定**来源**形态（默认 paginated），`--target-mode` 指定**目标**
/// 形态。两者都算一遍，故本子命令是"切换保锚点"的**可验证入口**——它不
/// 复用 `scroll::switch` 的状态机，而是独立地各算一次定位；若两侧的锚点值
/// 不同，`anchor_value_unchanged` 会如实报 `false`（不伪造 `true`）。
pub fn run_layout_anchor(options: &GlobalOptions, command: &Command) -> CommandOutcome {
    let Command::LayoutAnchor(args) = command else {
        return CommandOutcome::failure(
            &ErrorKind::Configuration,
            "run_layout_anchor 收到的不是 LayoutAnchor 命令",
        );
    };

    let crate::flags::LayoutArgs {
        chapter,
        text_file,
        mode,
        width,
        height,
        font_size,
        line_height,
        paragraph_mode,
        measure_em,
        gutter,
        spread,
        anchor,
        anchor_char,
        // `--target-mode` 只影响**报告哪个方向**（from/to 的先后），
        // 不影响计算——两种形态都算一遍（见函数文档）
        target_mode: _,
    } = args;

    let Some(char_offset) = anchor_char.or(*anchor) else {
        return CommandOutcome::failure(
            &ErrorKind::Configuration,
            "layout-anchor 需要 --char <offset>",
        );
    };

    let (chapter_index, text) = match resolve_chapter_text(chapter, text_file) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };

    // ## 两个形态必须在**同一组几何**下取值（FR-020 的"同一组几何参数"）
    //
    // 故本函数**忽略** `--mode`/`--target-mode` 对 `geometry.reading_mode` 的
    // 影响：那个字段是"当前形态"的标签，而本子命令要同时算两种形态的位置。
    // 若让它参与，两种形态就会在不同几何下比较——那时锚点"不变"是巧合。
    let (geometry, viewport) = match resolve_geometry(
        *width,
        *height,
        *font_size,
        *line_height,
        paragraph_mode.as_deref(),
        *measure_em,
        *gutter,
        spread.as_deref(),
        None, // 见上：形态标签不参与几何，几何本身与形态无关
    ) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };

    // --- 翻页形态的定位 ---
    let mut paginator = Paginator::new();
    let paginated = match paginator.paginate(&text, &geometry, &viewport) {
        Ok(result) => result,
        Err(error) => {
            return CommandOutcome::failure(&error.kind(), &format!("分页失败：{error}"));
        }
    };

    // --- 滚动形态的定位 ---
    //
    // 用**同一个** `paginator`（它持有昂贵的字体库句柄）——两形态共用一次
    // 排版引擎的持有权。
    let flow = match layout_chapter(&mut paginator, &text, &geometry, &viewport)
        .and_then(|chapter| ScrollFlow::new(chapter, geometry, viewport))
    {
        Ok(flow) => flow,
        Err(error) => {
            return CommandOutcome::failure(&error.kind(), &format!("滚动流构造失败：{error}"));
        }
    };

    let clamped = char_offset.min(paginated.char_len).min(flow.char_len());

    // 翻页侧定位
    let paged_target = paginated.resolve_anchor(clamped);
    let paged_json = match &paged_target {
        Some(resolution) => serde_json::json!({
            "mode": "paginated",
            "char_offset": resolution.char_offset,
            "page_index": resolution.page_index(),
            "offset_in_page": resolution.offset_in_page(),
            "visible": resolution.is_visible(),
        }),
        None => serde_json::json!({
            "mode": "paginated",
            "char_offset": clamped,
            "page_index": serde_json::Value::Null,
            "offset_in_page": serde_json::Value::Null,
            "visible": false,
        }),
    };

    // 滚动侧定位
    let scroll_owner = flow
        .viewports()
        .iter()
        .find(|entry| entry.char_range.contains(&clamped));
    let scroll_json = match scroll_owner {
        Some(entry) => serde_json::json!({
            "mode": "scroll",
            "char_offset": clamped,
            "viewport_index": entry.viewport_index,
            "offset_in_viewport": clamped - entry.char_range.start,
            "visible": true,
        }),
        None => serde_json::json!({
            "mode": "scroll",
            "char_offset": clamped,
            "viewport_index": serde_json::Value::Null,
            "offset_in_viewport": serde_json::Value::Null,
            "visible": false,
        }),
    };

    // --- 方向：`--mode` 是来源，`--target-mode` 是目标 ---
    let from_is_scroll = mode.as_deref() == Some("scroll");
    let (from_json, to_json) = if from_is_scroll {
        (scroll_json, paged_json)
    } else {
        (paged_json, scroll_json)
    };

    // ## `anchor_value_unchanged` 的判据
    //
    // 两侧报的 `char_offset` 必须相同。它们都源自同一个 `clamped`，故除非
    // 某个定位函数改写了锚点值（那正是 SC-005b 要抓的），本项恒真。此处
    // **如实比较**两侧的输出而不是硬编码 `true`——硬编码会让判据永远成立。
    let from_offset = from_json["char_offset"].as_u64();
    let to_offset = to_json["char_offset"].as_u64();
    let anchor_value_unchanged = from_offset.is_some() && from_offset == to_offset;
    let to_visible = to_json["visible"].as_bool().unwrap_or(false);

    let payload = serde_json::json!({
        "chapter_index": chapter_index,
        "char_offset": clamped,
        "from": from_json,
        "to": to_json,
        "anchor_value_unchanged": anchor_value_unchanged,
        "to_visible": to_visible,
        // 便于人工核对的可读摘要（标识式，不含面向用户的文案，FR-058）
        "geometry": {
            "font_size": geometry.font_size_px,
            "line_height": geometry.line_height,
            "measure_em": geometry.measure_em,
            "gutter": geometry.gutter_px,
            "width": viewport.available_width_px,
            "height": viewport.available_height_px,
            "columns": paginated.spread.columns.count(),
            "measure_px": paginated.spread.measure_px,
        },
        "page_count": paginated.pages.len(),
        "viewport_count": flow.viewports().len(),
    });

    match options.format {
        OutputFormat::Json => CommandOutcome::with_stdout(format!("{payload:#}")),
        OutputFormat::Human => {
            let mut out = String::new();
            out.push_str(&format!(
                "锚点 {} · {} → {} · 值不变 {} · 目标可见 {}
",
                clamped,
                from_json["mode"].as_str().unwrap_or("?"),
                to_json["mode"].as_str().unwrap_or("?"),
                if anchor_value_unchanged { "是" } else { "否" },
                if to_visible { "是" } else { "否" },
            ));
            out.push_str(&format!(
                "共 {} 页 / {} 屏
",
                paginated.pages.len(),
                flow.viewports().len()
            ));
            CommandOutcome::with_stdout(out)
        }
    }
}
