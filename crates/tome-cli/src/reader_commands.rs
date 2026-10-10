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
use tome_reader::layout::engine::{Geometry, LayoutEngine};
use tome_reader::layout::linebox::LineBox;
use tome_reader::rules::justify::Alignment;
use tome_reader::text::model::TextModel;
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

    // --- 几何参数（CLI 覆盖默认值）---
    let mut layout_options = LayoutOptions::default();
    if let Some(value) = width {
        layout_options.width_px = *value;
    }
    if let Some(value) = height {
        layout_options.height_px = *value;
    }
    if let Some(value) = font_size {
        layout_options.font_size_px = *value;
    }
    if let Some(value) = line_height {
        layout_options.line_height = *value;
    }
    if let Some(value) = measure_em {
        layout_options.measure_em = *value;
    }
    if let Some(value) = gutter {
        layout_options.gutter_px = *value;
    }

    // --- 单双页判据（MASTER §5.2 的 em 制公式）---
    //
    // 本阶段只用于决定版心宽度；完整的单双页语义（手动覆盖三态、
    // 书页语义）属 T020/T021。
    let threshold =
        2.0 * layout_options.measure_em * layout_options.font_size_px + layout_options.gutter_px;
    let auto_double = layout_options.width_px >= threshold;
    let columns = match spread.as_deref() {
        Some("double") => 2,
        Some("single") => 1,
        _ => usize::from(auto_double) + 1,
    };

    // --- 排版 ---
    let geometry = Geometry {
        measure_px: layout_options.measure_width_px(columns),
        font_size_px: layout_options.font_size_px,
        line_height: layout_options.line_height,
        alignment: Alignment::Justified,
    };

    let split = paragraph::split(&text);
    let mut engine = LayoutEngine::new();

    let mut paragraph_reports = Vec::new();
    let mut all_exceptions = Vec::new();
    let mut degraded_words = 0usize;

    for para in &split.paragraphs {
        let model = TextModel::new(para.text.clone());
        let result = engine.layout_paragraph(&model, para.index, &geometry);
        degraded_words += result.degraded_words;
        all_exceptions.extend(result.exceptions.iter().cloned());

        paragraph_reports.push(serde_json::json!({
            "paragraph_index": para.index,
            "char_start": para.char_start,
            "char_len": para.char_len(),
            "from_heuristic": para.from_heuristic,
            "lines": result.lines.iter().map(line_to_json).collect::<Vec<_>>(),
        }));
    }

    // --- 锚点命中（`--anchor` 时）---
    let anchor_hit = anchor.map(|offset| {
        // 全文偏移 → 段落 + 段内偏移
        let mut remaining = offset;
        for para in &split.paragraphs {
            if remaining <= para.char_len() {
                return serde_json::json!({
                    "char_offset": offset,
                    "paragraph_index": para.index,
                    "offset_in_paragraph": remaining,
                    "visible": true,
                });
            }
            remaining -= para.char_len() + 1; // +1 为换行符
        }
        serde_json::json!({
            "char_offset": offset,
            "paragraph_index": serde_json::Value::Null,
            "offset_in_paragraph": serde_json::Value::Null,
            "visible": false,
        })
    });

    let payload = serde_json::json!({
        "chapter_index": chapter_index,
        "mode": mode.as_deref().unwrap_or("paginated"),
        "geometry": {
            "font_size": layout_options.font_size_px,
            "line_height": layout_options.line_height,
            "measure_em": layout_options.measure_em,
            "gutter": layout_options.gutter_px,
            "width": layout_options.width_px,
            "height": layout_options.height_px,
            "paragraph_mode": paragraph_mode.as_deref().unwrap_or("indent"),
            "spread": spread.as_deref().unwrap_or("auto"),
            "columns": columns,
            "measure_px": geometry.measure_px,
        },
        "paragraphs": paragraph_reports,
        // `[WIP]` 分页与滚动流属 T019/T025，本阶段输出空数组（不伪造）
        "pages": [],
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
                geometry.measure_px,
                split.paragraphs.len()
            ));
            let total_lines: usize = paragraph_reports
                .iter()
                .map(|para| para["lines"].as_array().map_or(0, std::vec::Vec::len))
                .sum();
            out.push_str(&format!("共 {total_lines} 行\n"));
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
