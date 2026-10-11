//! T048 验收：排版语料的**求值器**（`verify-layout` 的逻辑内核）
//!
//! ## 为什么求值器在 `tome-reader` 而不在 CLI
//!
//! 契约 §1 的"薄壳"要求：CLI 只做参数解析、调用库 API、序列化结果，
//! **逻辑 MUST NOT 在 CLI 内重复实现**（宪法 IV DRY）。语料求值（把一条用例
//! 的输入排版、再与期望比对）是逻辑，故它属于本 crate；`tome-cli` 只负责
//! 装载目录、调用本模块、把结果序列化成契约 §2.5 的形态。
//!
//! ## 求值器的三件事
//!
//! 1. **跑**：按用例的 `geometry` 排版 `input`（走既有 `LayoutEngine`，
//!    不另写一条排版路径——那会与生产路径分叉）
//! 2. **比**：把产出与 `expect` 逐项比对，**定位到具体哪一项不符**
//! 3. **报**：产出结构化的差异（供 CLI 序列化、供 CI 展示"错在哪一位"）
//!
//! ## 差异为什么必须是结构化的
//!
//! 语料 README 写明："一个 300 字的段落，行数对不上时报'整体不等'远不如报
//! '第 3 行应为 18 字、实为 17 字'有用"。故 [`CaseDiff`] 携带**逐项**的
//! 期望值与实际值，而不是一个 `bool` 或一句消息。

use std::collections::BTreeMap;

use crate::corpus::{CaseExpectation, TypesettingCase};
use crate::layout::engine::{Geometry as LayoutGeometry, LayoutEngine, LayoutResult};
use crate::layout::linebox::RuleExceptionRecord;

/// 单条用例的求值结果。
#[derive(Debug, Clone, PartialEq)]
pub struct CaseOutcome {
    /// 用例名（携带类别前缀）
    pub name: String,
    /// 类别（取自用例名前缀，与 `count_by_category` 同口径）
    pub category: String,
    /// 是否通过
    pub passed: bool,
    /// 差异明细（`passed == false` 时非空）
    pub diffs: Vec<CaseDiff>,
}

/// 一处差异（**定位到具体项**，不是一句概况）。
#[derive(Debug, Clone, PartialEq)]
pub struct CaseDiff {
    /// 出问题的项（如 `line_breaks.chars_per_line[3]`）
    pub field: String,
    /// 期望值（字符串化，便于 JSON 输出与打印）
    pub expected: String,
    /// 实际值
    pub actual: String,
}

/// 一批用例的求值汇总（契约 §2.5 的"通过率与逐条差异"）。
#[derive(Debug, Clone, PartialEq)]
pub struct CorpusReport {
    /// 逐条结果（保序，与输入同序）
    pub outcomes: Vec<CaseOutcome>,
    /// 总条数
    pub total: usize,
    /// 通过条数
    pub passed: usize,
}

impl CorpusReport {
    /// 通过率（`0.0`–`1.0`；`total == 0` 时为 `0.0`）。
    ///
    /// ## 为什么空语料算 0.0 而不是 1.0
    ///
    /// 空目录让"通过率 100%"成立的话，CI 里语料路径写错会**静默通过**——
    /// 那正是最需要被挡住的情况。0.0 让空语料必然不达标。
    #[must_use]
    pub fn pass_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.passed as f64 / self.total as f64
    }

    /// 逐类别通过情况（供报告分类展示）。
    #[must_use]
    pub fn by_category(&self) -> BTreeMap<String, (usize, usize)> {
        let mut map: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for outcome in &self.outcomes {
            let entry = map.entry(outcome.category.clone()).or_insert((0, 0));
            entry.1 += 1;
            if outcome.passed {
                entry.0 += 1;
            }
        }
        map
    }
}

/// 求值一批用例（**纯计算**，不触网、不开窗口）。
///
/// ## 引擎为何在这里新建一次
///
/// `LayoutEngine::new()` 会加载系统字体库（耗时可观）。批量求值共用**一个**
/// 引擎实例——每条用例各建一次会让 `verify-layout` 在几十条语料上慢一个量级。
#[must_use]
pub fn evaluate_corpus(cases: &[TypesettingCase]) -> CorpusReport {
    let mut engine = LayoutEngine::new();
    let outcomes: Vec<CaseOutcome> = cases
        .iter()
        .map(|case| evaluate_case(&mut engine, case))
        .collect();

    let passed = outcomes.iter().filter(|outcome| outcome.passed).count();
    CorpusReport {
        total: outcomes.len(),
        passed,
        outcomes,
    }
}

/// 求值单条用例。
///
/// ## 为什么要走段落切分（而不是把整份正文当一段）
///
/// 语料的 `input.paragraphs` 是**段落列表**（README：段内换行由 S2-1 的段落
/// 切分处理），且 `expect.line_breaks` 的逐行字符数是**跨段**的。故本函数
/// 复用生产路径的段落切分（[`crate::text::paragraph`]）与逐段排版，把各段的
/// 行盒按序汇总——与 `Paginator` 的排版路径同源，不另开一条。
fn evaluate_case(engine: &mut LayoutEngine, case: &TypesettingCase) -> CaseOutcome {
    let text = case.input.joined_text();
    let geometry = to_layout_geometry(&case.geometry);

    // 段落切分（与分页路径同源）
    let split = crate::text::paragraph::split(&text);

    let mut lines: Vec<crate::layout::linebox::LineBox> = Vec::new();
    let mut exceptions: Vec<RuleExceptionRecord> = Vec::new();
    let mut degraded_words = 0usize;

    for para in &split.paragraphs {
        let model = crate::text::model::TextModel::new(para.text.clone());
        let result = engine.layout_paragraph(&model, para.index, &geometry);
        degraded_words += result.degraded_words;
        exceptions.extend(result.exceptions.iter().cloned());
        // 行盒的段内区间换算为**全文**区间：+ 段的起点
        for mut line in result.lines {
            let offset = para.char_start;
            line.char_range = (line.char_range.start + offset)..(line.char_range.end + offset);
            for glyph in &mut line.glyphs {
                glyph.char_range =
                    (glyph.char_range.start + offset)..(glyph.char_range.end + offset);
            }
            lines.push(line);
        }
    }

    let result = LayoutResult {
        lines,
        exceptions,
        degraded_words,
    };

    let diffs = compare(&case.expect, &result, &geometry, &text);
    CaseOutcome {
        name: case.name.clone(),
        category: category_of_name(&case.name),
        passed: diffs.is_empty(),
        diffs,
    }
}

/// 语料几何 → 排版引擎几何。
///
/// ## `paragraph_indent_em == 0` 表示**段间距模式**
///
/// 语料的 `geometry` 只有四件（版心/字号/行高/段缩进），没有独立的模式字段。
/// README 的字段说明写明"段首缩进（em，0 = 段间距模式）"，故此处做这个映射：
/// 缩进量 0 对应 `ParagraphMode::Gap`。这是语料格式的既定约定，不是本模块
/// 的发明。
fn to_layout_geometry(corpus: &crate::corpus::Geometry) -> LayoutGeometry {
    use crate::layout::paragraph_mode::ParagraphMode;

    let paragraph_mode = if corpus.paragraph_indent_em > 0.0 {
        ParagraphMode::Indent {
            em: corpus.paragraph_indent_em,
        }
    } else {
        ParagraphMode::Gap { em: 0.4 }
    };

    LayoutGeometry {
        measure_px: corpus.measure_px,
        font_size_px: corpus.font_size_px,
        line_height: corpus.line_height,
        alignment: crate::rules::justify::Alignment::Justified,
        paragraph_mode,
    }
}

/// 取自用例名的类别前缀（与 `corpus::category_of` 同口径）。
fn category_of_name(name: &str) -> String {
    name.split(':').next().unwrap_or("other").to_string()
}

/// 逐项比对期望与实际。
fn compare(
    expect: &CaseExpectation,
    result: &LayoutResult,
    geometry: &LayoutGeometry,
    text: &str,
) -> Vec<CaseDiff> {
    match expect {
        CaseExpectation::LineBreaks { chars_per_line } => {
            compare_line_breaks(chars_per_line, result)
        }
        CaseExpectation::Kinsoku {
            forbidden_at_line_start,
            forbidden_at_line_end,
        } => compare_kinsoku(forbidden_at_line_start, forbidden_at_line_end, result, text),
        CaseExpectation::KinsokuExceptions { count, .. } => {
            let actual = result.exceptions.len();
            if actual == *count {
                Vec::new()
            } else {
                vec![CaseDiff {
                    field: "kinsoku_exceptions.count".to_string(),
                    expected: count.to_string(),
                    actual: actual.to_string(),
                }]
            }
        }
        CaseExpectation::Squeeze { compression_px } => compare_squeeze(compression_px, result),
        CaseExpectation::MixedScript { overflow } => compare_mixed(overflow, result, geometry),
    }
}

/// 断行位置比对：逐行字符数。
///
/// ## 差异为什么逐行报
///
/// 行数不同时报"行数应为 N、实为 M"只说了一半——真正有用的是"第一条不同的
/// 行是第几条、各自多少字"。故本函数在行数不等时也逐行比对到**较短的那一侧
/// 长度**，并额外报一条长度差异。
fn compare_line_breaks(expected: &[usize], result: &LayoutResult) -> Vec<CaseDiff> {
    let mut diffs = Vec::new();
    let actual: Vec<usize> = result
        .lines
        .iter()
        .map(|line| line.char_range.end - line.char_range.start)
        .collect();

    for (index, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
        if want != got {
            diffs.push(CaseDiff {
                field: format!("line_breaks.chars_per_line[{index}]"),
                expected: want.to_string(),
                actual: got.to_string(),
            });
        }
    }
    if expected.len() != actual.len() {
        diffs.push(CaseDiff {
            field: "line_breaks.chars_per_line.len".to_string(),
            expected: expected.len().to_string(),
            actual: actual.len().to_string(),
        });
    }
    diffs
}

/// 禁则比对：行首/行尾不得出现的字符。
///
/// ## 判据是"实际的首/尾字符"而非"是否出现在行内"
///
/// 禁则约束的是**位置**（行首或行尾）。一个字出现在行中间完全合法——故比对
/// 取每行的第一个与最后一个字符，与禁用集合求交。
///
/// ## 为什么要传源文本
///
/// `LineBox.char_range` 是**段内**字符区间，本身不含字符内容。取"这一行的
/// 首/尾字符"需要回源文本按区间取——故 `text` 是必需输入。段落切分产出的
/// 段文本与该区间同口径（都是段内字符下标）。
fn compare_kinsoku(
    forbidden_at_start: &[String],
    forbidden_at_end: &[String],
    result: &LayoutResult,
    text: &str,
) -> Vec<CaseDiff> {
    let mut diffs = Vec::new();
    let chars: Vec<char> = text.chars().collect();

    for (index, line) in result.lines.iter().enumerate() {
        // 行首字符：区间起点处的字符
        if let Some(first) = chars.get(line.char_range.start) {
            let first = first.to_string();
            if forbidden_at_start.contains(&first) {
                diffs.push(CaseDiff {
                    field: format!("kinsoku.forbidden_at_line_start[line {index}]"),
                    expected: format!("行首不得为 `{first}`"),
                    actual: format!("第 {index} 行行首为 `{first}`"),
                });
            }
        }
        // 行尾字符：区间末位处的字符（`char_range.end` 是**不含**端，故减一）
        if let Some(last) = line
            .char_range
            .end
            .checked_sub(1)
            .and_then(|end| chars.get(end))
        {
            let last = last.to_string();
            if forbidden_at_end.contains(&last) {
                diffs.push(CaseDiff {
                    field: format!("kinsoku.forbidden_at_line_end[line {index}]"),
                    expected: format!("行尾不得为 `{last}`"),
                    actual: format!("第 {index} 行行尾为 `{last}`"),
                });
            }
        }
    }
    diffs
}

/// 挤压比对：逐行压缩量（行盒的 `squeeze_px`）。
fn compare_squeeze(expected: &[f32], result: &LayoutResult) -> Vec<CaseDiff> {
    let mut diffs = Vec::new();
    let actual: Vec<f32> = result.lines.iter().map(|line| line.squeeze_px).collect();
    for (index, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
        // 浮点比较用容差（0.5px——小于半字身，足够区分"挤了"与"没挤"）
        if (want - got).abs() > 0.5 {
            diffs.push(CaseDiff {
                field: format!("squeeze.compression_px[{index}]"),
                expected: format!("{want:.2}"),
                actual: format!("{got:.2}"),
            });
        }
    }
    if expected.len() != actual.len() {
        diffs.push(CaseDiff {
            field: "squeeze.compression_px.len".to_string(),
            expected: expected.len().to_string(),
            actual: actual.len().to_string(),
        });
    }
    diffs
}

/// 混排比对：逐行是否超宽。
///
/// ## "超宽"用 `visual_width_px`（含两端对齐的伸展）
///
/// SC-003 的判据是"无任一行超出容器"——那是**视觉**行长。用 `content_width_px`
/// （字身之和，不含伸展）会漏掉"被对齐撑出容器"的情形。
fn compare_mixed(
    expected: &[bool],
    result: &LayoutResult,
    geometry: &LayoutGeometry,
) -> Vec<CaseDiff> {
    let mut diffs = Vec::new();
    let actual: Vec<bool> = result
        .lines
        .iter()
        .map(|line| line.visual_width_px() > geometry.measure_px + 0.5)
        .collect();
    for (index, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
        if want != got {
            diffs.push(CaseDiff {
                field: format!("mixed_script.overflow[{index}]"),
                expected: want.to_string(),
                actual: got.to_string(),
            });
        }
    }
    if expected.len() != actual.len() {
        diffs.push(CaseDiff {
            field: "mixed_script.overflow.len".to_string(),
            expected: expected.len().to_string(),
            actual: actual.len().to_string(),
        });
    }
    diffs
}
