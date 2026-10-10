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
use tome_reader::paginate::config::{ParagraphMode, ReadingConfig, ReadingMode, SpreadMode};
use tome_reader::paginate::page::Paginator;
use tome_reader::paginate::spread::Viewport;
use tome_reader::text::paragraph;

use crate::commands::CommandOutcome;
use crate::flags::{Command, GlobalOptions, OutputFormat};

/// 排版参数（`contracts/cli.md` §2.1 的默认值来自 MASTER §5.1）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutOptions {
    /// 阅读区可用宽度（px）——**不是窗口宽度**（spec §FR-014 的口径）
    pub width_px: f32,
    /// 阅读区可用高度（px）
    pub height_px: f32,
    /// 阅读字号（px）
    pub font_size_px: f32,
    /// 行高（字号倍数）
    pub line_height: f32,
    /// 版心目标（em）
    pub measure_em: f32,
    /// 双页中缝（px）
    pub gutter_px: f32,
}

impl Default for LayoutOptions {
    /// 默认值取自 `design-system/MASTER.md` §5.1 与 `contracts/cli.md` §2.1。
    fn default() -> Self {
        Self {
            // 契约示例里的默认阅读区（1180×800）——它是"阅读区可用宽度"
            // 而非窗口宽度（§5.2 的口径声明）
            width_px: 1180.0,
            height_px: 800.0,
            font_size_px: 19.0,
            line_height: 1.85,
            measure_em: 36.0,
            gutter_px: 64.0,
        }
    }
}

impl LayoutOptions {
    /// 实际用于排版的**版心宽度**（px）。
    ///
    /// ## 口径（spec §FR-014 / 裁决 OQ#22→A）
    ///
    /// 单页时版心 = `min(measure_em × font_size, width)`，
    /// 且不超过 `measure_max_em`（42em）——超过则留白而非拉伸
    /// （MASTER §5.3 的"单页版心封顶 42em"裁决）。
    ///
    /// 双页判据属 T019（分页），本阶段按 `--spread` 的显式值或自动判据
    /// 决定是否减半。
    #[must_use]
    pub fn measure_width_px(&self, columns: usize) -> f32 {
        let per_column_target = self.measure_em * self.font_size_px;
        let usable = if columns <= 1 {
            self.width_px
        } else {
            // 双页：扣除中缝后对半
            (self.width_px - self.gutter_px) / columns as f32
        };
        // MASTER §5.3：单页版心封顶 42em（超出留白，不拉伸）
        let cap = 42.0 * self.font_size_px;
        per_column_target.min(usable).min(cap).max(1.0)
    }
}

/// `layout` 子命令的实现（`contracts/cli.md` §2.1）。
///
/// ## 输出形态
///
/// 见 `contracts/cli.md` §2.1 的 JSON 示例。本阶段实现其中**排版层能给出**
/// 的部分：`chapter_index` / `mode` / `geometry` / `paragraphs`（行盒结构）
/// / `rule_exceptions`。`pages` 与 `scroll` 属分页切片（T019/T025），
/// 此阶段输出空数组并在 `[WIP]` 中标注——**不伪造**。
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
    } = args;

    // --- 正文来源 ---
    let (chapter_index, text) = match (text_file, chapter) {
        (Some(path), _) => match read_text(path) {
            Ok(text) => (0usize, text),
            Err(outcome) => return outcome,
        },
        (None, Some(reference)) => {
            // `[WIP]` `--chapter` 的接线待书架就绪（001 的 BookRepository
            // 与本地书扫描）。此处给出**可操作**的错误而非静默降级。
            return CommandOutcome::failure(
                &ErrorKind::unsupported_capability(tome_core::ErrorCapability::new(
                    "chapter-by-bookshelf",
                )),
                &format!(
                    "--chapter `{reference}` 需要书架接线（属 S3 与 001 的仓库职责）；\
                     本阶段请用 `--text <file>` 直接给正文"
                ),
            );
        }
        (None, None) => {
            return CommandOutcome::failure(
                &ErrorKind::Configuration,
                "layout 需要 --text <file>（本阶段）或 --chapter <book>#<n>（待接线）",
            );
        }
    };

    // --- 几何参数（CLI 选项覆盖默认值）---
    //
    // 从 T017 的 `ReadingConfig` 出发而不是手搓：几何参数的**默认值与校验**
    // 都是 schema 的一部分，CLI 不该有第二份。选项只做覆盖。
    //
    // `LayoutOptions` 保留为 CLI 的**视图模型**：宽/高是"阅读区可用尺寸"
    // （FR-014 的口径），它不是 `GeometryConfig` 的字段（几何子集描述的是
    // 版心与字号，可用尺寸来自外壳布局），故由 CLI 自己承载并用于构造
    // `Viewport`。
    let mut layout_options = LayoutOptions::default();
    let mut geometry = ReadingConfig::default_ma().geometry;
    if let Some(value) = width {
        layout_options.width_px = *value;
    }
    if let Some(value) = height {
        layout_options.height_px = *value;
    }
    if let Some(value) = font_size {
        // `f32` → `u16`：`flags` 已在校验阶段拒绝非整数与越界值（退出码 2），
        // 故此处的转换不会丢信息；仍走一次 `round` 以免浮点表示误差。
        geometry.font_size_px = value.round().max(0.0) as u16;
    }
    if let Some(value) = line_height {
        geometry.line_height = *value;
    }
    if let Some(value) = measure_em {
        geometry.measure_em = *value;
    }
    if let Some(value) = gutter {
        geometry.gutter_px = value.round().max(0.0) as u16;
    }
    if let Some(mode) = paragraph_mode.as_deref() {
        geometry.paragraph_mode = match mode {
            "gap" => ParagraphMode::Gap { em: 0.4 },
            _ => ParagraphMode::Indent { em: 2.0 },
        };
    }
    if let Some(mode) = spread.as_deref() {
        geometry.spread_mode = match mode {
            "double" => SpreadMode::ForceSpread,
            "single" => SpreadMode::ForceSingle,
            _ => SpreadMode::Auto,
        };
    }
    geometry.reading_mode = match mode.as_deref() {
        Some("scroll") => ReadingMode::Scroll,
        _ => ReadingMode::Paged,
    };

    // 白名单校验：非法几何报结构化错误（与 `flags` 的用法错误区分开——
    // 那是"参数写错"，这是"参数组合非法"）
    if let Err(field) = geometry.validate() {
        return CommandOutcome::failure(
            &ErrorKind::Configuration,
            &format!("几何参数 `{field}` 非法（见 ReadingConfig 的白名单范围）"),
        );
    }

    let viewport = Viewport::new(layout_options.width_px, layout_options.height_px);

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

    // --- 锚点命中（`--anchor` 时）---
    let anchor_hit = anchor.map(|offset| match paginated.resolve_anchor(offset) {
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
    });

    let payload = serde_json::json!({
        "chapter_index": chapter_index,
        "mode": mode.as_deref().unwrap_or("paginated"),
        "geometry": {
            "font_size": geometry.font_size_px,
            "line_height": geometry.line_height,
            "measure_em": geometry.measure_em,
            "gutter": geometry.gutter_px,
            "width": layout_options.width_px,
            "height": layout_options.height_px,
            "paragraph_mode": paragraph_mode.as_deref().unwrap_or("indent"),
            "spread": spread.as_deref().unwrap_or("auto"),
            "columns": paginated.spread.columns.count(),
            "measure_px": paginated.spread.measure_px,
            "override_active": paginated.spread.override_active,
        },
        "paragraphs": paragraph_reports,
        // 真实页序列（T019/T021）。`scroll` 仍为空——滚动流属 Phase 5
        // （T025/T026），此处**不伪造**。
        "pages": pages_json,
        "scroll": {
            "content_height": serde_json::Value::Null,
            "viewports": [],
        },
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
