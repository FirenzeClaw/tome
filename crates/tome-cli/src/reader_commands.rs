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

// ===========================================================================
// `render` —— 离屏渲染一页为 PNG（contracts/cli.md §2.3）
// ===========================================================================
//
// ## 门控与诚实报告
//
// `tome-reader` 的 `render` feature 关闭时，本函数编译成"永远报能力错误"
// 的形态（见下方 cfg 分支）——那正是 headless 构建的预期行为
// （tasks.md 002 T032 的门控约定、T048 的第三条验收）。
//
// ## 后端不可用 → 退出码 3
//
// contracts/cli.md §2.3："MUST 输出结构化错误并退出码 3
// （`UnsupportedCapability { capability: "offscreenRendering" }`），
// MUST NOT 产出空文件或伪造成功"。
//
// 两条不可用路径（编译期无渲染面 / 运行期无后端）**共用同一个能力标识**，
// 因为对调用方而言处置相同：本机拿不到图像，应改走 `tome layout` 做结构验证。

/// 执行 `render` 子命令。
#[cfg(feature = "reader-render")]
pub fn run_render(options: &GlobalOptions, command: &Command) -> CommandOutcome {
    use tome_reader::render::batch::RenderBatch;
    use tome_reader::render::glyph_cache::GlyphCache;
    use tome_reader::render::offscreen::OffscreenRenderer;
    use tome_reader::render::pipeline::{PipelineError, RenderPipeline};

    let Command::Render(args) = command else {
        return CommandOutcome::failure(
            &ErrorKind::Configuration,
            "run_render 收到的不是 Render 命令",
        );
    };

    let (_chapter_index, text) =
        match resolve_chapter_text(&args.layout.chapter, &args.layout.text_file) {
            Ok(resolved) => resolved,
            Err(outcome) => return outcome,
        };

    let (geometry, viewport) = match resolve_geometry(
        args.layout.width,
        args.layout.height,
        args.layout.font_size,
        args.layout.line_height,
        args.layout.paragraph_mode.as_deref(),
        args.layout.measure_em,
        args.layout.gutter,
        args.layout.spread.as_deref(),
        args.layout.mode.as_deref(),
    ) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };

    // 主题（默认 day，FR-030）
    let theme = match args.theme.as_deref() {
        Some("night") => tome_core::model::theme::NightTheme.theme(),
        _ => tome_core::model::theme::DayTheme.theme(),
    };

    let width = viewport.available_width_px.round().max(1.0) as u32;
    let height = viewport.available_height_px.round().max(1.0) as u32;

    let backend = match RenderPipeline::probe_backend() {
        Ok(backend) => backend,
        Err(PipelineError::NoAdapter { reason }) => {
            return capability_failure(&format!("离屏渲染不可用：无图形后端（{reason}）"));
        }
        Err(other) => {
            return CommandOutcome::failure(
                &ErrorKind::Content,
                &format!("图形后端探测以意外方式失败：{other}"),
            );
        }
    };

    let pipeline = match RenderPipeline::new(backend.clone(), width, height) {
        Ok(pipeline) => pipeline,
        Err(error) => {
            return CommandOutcome::failure(
                &ErrorKind::Content,
                &format!("离屏管线初始化失败：{error}"),
            );
        }
    };

    // --- 分页 + 取目标页 ---
    //
    // ## 为什么必须走完整分页（而不是"按视口截取 N 段行"）
    //
    // 页边界是**分页层的结论**：含留白页（FR-016 的书页语义）、单行超高
    // 独占一页、以及手动单双页覆盖的影响。渲染层自己按视口重算会与之分叉
    // ——表现为"渲染的第 N 页与 `tome layout` 报告的第 N 页内容不同"。
    //
    // 故：先 `paginate` 拿到权威页序列，再取第 `--page` 页。
    let mut paginator = Paginator::new();
    let pagination = match paginator.paginate(&text, &geometry, &viewport) {
        Ok(result) => result,
        Err(error) => {
            return CommandOutcome::failure(&error.kind(), &format!("分页失败：{error}"));
        }
    };

    // --- 目标选取：`--page <n>`（翻页）或 `--viewport <n>`（滚动）---
    //
    // ## 两条路径各自都「先算再取」，不互相借用
    //
    // `--page` 取 `paginate` 的**权威页序列**（含留白页、单行超高独占一页）；
    // `--viewport` 取 `ScrollFlow` 切好的**视口序列**（视口高 = 画布高）。
    // 二者是两套切分，不能用一个近似另一个：滚动视口高通常远小于页高，
    // 用页序列去截首屏会与原视口分叉（表现为"渲染的视口 N 与 `tome layout`
    // 报告的视口 N 内容不同"，与 `--page` 那条修正同一条纪律）。
    let (batch, target_report) = if let Some(index) = args.viewport_index {
        let target_index = index as usize;
        // 先分页取行盒（`ScrollFlow::new` 的输入是 `ChapterLayout`），
        // 再交给滚动流按视口高切分——两次转换都是既有 API，CLI 不复制逻辑。
        let mut scroll_paginator = Paginator::new();
        let chapter = match layout_chapter(&mut scroll_paginator, &text, &geometry, &viewport) {
            Ok(chapter) => chapter,
            Err(error) => {
                return CommandOutcome::failure(&error.kind(), &format!("排版失败：{error}"));
            }
        };
        let flow = match ScrollFlow::new(chapter, geometry, viewport) {
            Ok(flow) => flow,
            Err(error) => {
                return CommandOutcome::failure(&error.kind(), &format!("滚动流构造失败：{error}"));
            }
        };
        let Some(scroll_viewport) = flow.viewports().get(target_index) else {
            // 越界视口号 = **结构化错误**，不是一张只有底色的图（与越界页码同理）。
            return CommandOutcome::failure(
                &ErrorKind::Configuration,
                &format!(
                    "--viewport {target_index} 越界：本章共 {} 个视口（视口号 0 起）",
                    flow.viewports().len()
                ),
            );
        };
        let lines = flow.viewport_lines(scroll_viewport);
        let batch = RenderBatch::for_line_slice(lines, &geometry, &viewport, theme);
        (
            batch,
            serde_json::json!({ "viewport_index": target_index, "page_index": serde_json::Value::Null }),
        )
    } else {
        // 页号是**章内页码**（0 起），与 `tome layout` 输出的 `page_index` 同口径。
        let target_index = args.page.map(|value| value as usize).unwrap_or(0);
        let Some(page) = pagination.pages.get(target_index) else {
            // 越界页码 = **结构化错误**，不是一张只有底色的图。
            // 静默产出空图会让调用方无法分辨"这页真的是空的"与"页码写错了"。
            return CommandOutcome::failure(
                &ErrorKind::Configuration,
                &format!(
                    "--page {target_index} 越界：本章共 {} 页（章内页码 0 起）",
                    pagination.pages.len()
                ),
            );
        };
        (
            RenderBatch::for_page(page, &geometry, &viewport, theme),
            serde_json::json!({ "page_index": target_index, "viewport_index": serde_json::Value::Null }),
        )
    };

    let mut cache = GlyphCache::new();

    let image = match OffscreenRenderer::new(width, height)
        .and_then(|renderer| renderer.render(&pipeline, &batch, &mut cache))
    {
        Ok(image) => image,
        Err(error) => {
            return CommandOutcome::failure(&ErrorKind::Content, &format!("离屏渲染失败：{error}"));
        }
    };

    let Some(out) = args.out.as_ref() else {
        return CommandOutcome::failure(&ErrorKind::Configuration, "render 缺少 --out");
    };

    let png = crate::png::encode_png(image.width, image.height, &image.pixels);
    if let Err(error) = std::fs::write(out, &png) {
        return CommandOutcome::failure(
            &ErrorKind::Storage,
            &format!("写入 {} 失败：{error}", out.display()),
        );
    }

    let palette = theme.palette();
    let foreground = image.foreground_pixels([
        palette.bg_reading.red,
        palette.bg_reading.green,
        palette.bg_reading.blue,
    ]);

    let report = serde_json::json!({
        "out": out.display().to_string(),
        "width": image.width,
        "height": image.height,
        "bytes": png.len(),
        "foreground_pixels": foreground,
        "total_pixels": image.total_pixels(),
        "backend": backend.name(),
        "adapter": backend.adapter_name(),
        "software": backend.is_software,
        "theme": args.theme.clone().unwrap_or_else(|| "day".to_owned()),
        "reduced_motion": args.reduced_motion,
        // 目标回显：`--page` 与 `--viewport` 二选一，未被选中的那项为 `null`——
        // 调用方据此确认"我给的视口号真的被消费了"，而不是渲染了首屏。
        "page_index": target_report.get("page_index").cloned().unwrap_or(serde_json::Value::Null),
        "viewport_index": target_report.get("viewport_index").cloned().unwrap_or(serde_json::Value::Null),
    });

    CommandOutcome::with_stdout(format_report(options, &report))
}

/// `render` 的 headless 形态（`tome-reader` 的 `render` feature 关闭）。
///
/// ## 这条路径就是 T048 第三条验收的被测对象
///
/// "渲染面在编译期就不存在" 是 §2.3 要求的最强形态——比 "运行期拿不到
/// 后端" 更彻底：连试图渲染的代码都不在二进制里。
#[cfg(not(feature = "reader-render"))]
pub fn run_render(_options: &GlobalOptions, _command: &Command) -> CommandOutcome {
    capability_failure(concat!(
        "本构建未启用渲染面（tome-reader 的 render feature 关闭）：离屏渲染不可用。",
        "处置：用默认 feature 构建（开发/发布构建默认带渲染面），",
        "或改走 `tome layout` 做排版结构验证（无需图形环境）。"
    ))
}

/// 构造 `offscreenRendering` 能力错误的 `CommandOutcome`（退出码 3）。
///
/// ## 为什么抽成函数
///
/// 两条路径（编译期无渲染面 / 运行期无后端）**必须给出同一形态的错误**——
/// 否则调用方要写两条分支来处理"同样一件事"。抽成函数让"同一形态"是
/// 结构性的，而不是两处各写一遍碰巧相同。
fn capability_failure(message: &str) -> CommandOutcome {
    CommandOutcome::failure(
        &ErrorKind::unsupported_capability(tome_core::ErrorCapability::new(
            tome_reader::capability::OFFSCREEN_RENDERING,
        )),
        message,
    )
}

/// 执行 `fonts` 子命令（`contracts/cli.md` §2.4）。
pub fn run_fonts(options: &GlobalOptions) -> CommandOutcome {
    let report = tome_reader::render::fonts::FontsReport::collect();
    let json = serde_json::json!({
        "embedded": [{
            "role": report.embedded_role,
            "family": report.embedded_family,
            "subset_coverage": report.subset_coverage,
            "bytes": report.embedded_bytes,
        }],
        "system_fallbacks": { "reading_serif": report.system_fallbacks },
        "budget_bytes": report.budget_bytes,
        "within_budget": report.within_budget,
    });
    CommandOutcome::with_stdout(format_report(options, &json))
}

/// 按 `--format` 渲染 JSON 或人类可读摘要。
///
/// ## 为什么人类形态不另走一套序列化
///
/// 契约要求默认 `--format json`（供脚本消费），`human` 是给人看的摘要。
/// 若为 human 另写一套结构体，两份输出会随字段增减而漂移——那是
/// "文档与实现不一致" 的经典形态。故 human 直接从同一个 `Value` 平铺出来。
fn format_report(options: &GlobalOptions, value: &serde_json::Value) -> String {
    match options.format {
        OutputFormat::Json => {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_owned())
        }
        OutputFormat::Human => {
            let mut lines = Vec::new();
            if let Some(object) = value.as_object() {
                for (key, entry) in object {
                    match entry {
                        serde_json::Value::Object(nested) => {
                            lines.push(format!("{key}:"));
                            for (nested_key, nested_value) in nested {
                                lines.push(format!("  {nested_key}: {nested_value}"));
                            }
                        }
                        serde_json::Value::Array(items) => {
                            lines.push(format!("{key}:"));
                            for item in items {
                                lines.push(format!("  {item}"));
                            }
                        }
                        other => lines.push(format!("{key}: {other}")),
                    }
                }
            }
            lines.join(
                "
",
            )
        }
    }
}
