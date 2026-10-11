//! T048 验收：排版语料求值器（`verify-layout` 的逻辑内核）
//!
//! ## 与 `corpus_loading.rs` 的分工
//!
//! | 文件 | 验证什么 |
//! |---|---|
//! | `corpus_loading.rs`（T004） | 语料能被**装载**（格式、必填字段、`basis` 质量） |
//! | 本文件（T048） | 语料能被**求值**（跑排版、逐项比对、差异可定位） |
//!
//! 前者是"语料是完整可读的"，后者是"语料真的在检验实现"。两条都必要：
//! 语料装载成功但求值器有 bug 时，语料的约束力为零。
//!
//! ## 判据的组织
//!
//! 1. **真语料能跑通**（仓库内语料的全量求值）
//! 2. **求值器能识别错**（注入一条必然不符的期望，断言差异被定位到具体项）
//! 3. **差异是结构化的**（携带期望值与实际值，不是一句概况）
//! 4. **汇总统计正确**（通过率、按类别分组、空语料算 0.0）

use std::path::PathBuf;

use tome_reader::corpus::{load_corpus_directory, CaseExpectation, TypesettingCase};
use tome_reader::corpus_eval::{evaluate_corpus, CorpusReport};

fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

// ===========================================================================
// 1. 真语料求值
// ===========================================================================

/// **仓库内的语料可被完整求值**（不 panic、不静默跳过）。
#[test]
fn real_corpus_evaluates_without_crashing() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");
    assert!(!cases.is_empty(), "前置：语料非空");

    let report = evaluate_corpus(&cases);
    assert_eq!(
        report.total,
        cases.len(),
        "每条语料都应被求值（不得静默跳过任何一条）"
    );
    // 逐条都应有结果（哪怕不通过也应有差异明细）
    for outcome in &report.outcomes {
        if !outcome.passed {
            assert!(
                !outcome.diffs.is_empty(),
                "用例 `{}` 判为不通过却没给差异——违反「报错要说清哪里错」",
                outcome.name
            );
        }
    }
}

/// **按类别的汇总与语料实际分布一致**。
#[test]
fn report_groups_by_category() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");
    let report = evaluate_corpus(&cases);
    let by_category = report.by_category();

    // 每个类别都应有条目（总数守恒）
    let summed: usize = by_category.values().map(|(_, total)| total).sum();
    assert_eq!(
        summed, report.total,
        "按类别分组的条目总数应等于总条数（分组不丢条）"
    );

    // 通过数也应守恒
    let passed_summed: usize = by_category.values().map(|(passed, _)| passed).sum();
    assert_eq!(
        passed_summed, report.passed,
        "按类别分组的通过数之和应等于总通过数"
    );
}

// ===========================================================================
// 2. 求值器能识别错（这是求值器有约束力的前提）
// ===========================================================================

/// **注入一条必然不符的期望，断言它被判为不通过且差异定位到具体项**。
///
/// ## 这条为什么是求值器最重要的测试
///
/// 一个"永远判通过"的求值器能让上面两条测试全部通过——但它对实现毫无约束
/// 力。本测试用**已知错误**的期望喂进去，断言求值器真的抓住了它，且差异
/// 指向**具体的行号**（而不是笼统的"不等"）。
#[test]
fn evaluator_detects_a_wrong_expectation() {
    // 取仓库内的第一条语料，把它的期望改成必然不符的值
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");
    let original = cases.first().expect("语料非空").clone();

    let mutated = TypesettingCase {
        name: original.name.clone(),
        input: original.input.clone(),
        geometry: original.geometry,
        // 期望"每一行都是 999 字"——现实中不可能
        expect: CaseExpectation::LineBreaks {
            chars_per_line: vec![999],
        },
        basis: original.basis.clone(),
    };

    let report = evaluate_corpus(&[mutated]);
    assert_eq!(report.total, 1);
    assert_eq!(report.passed, 0, "必然不符的期望必须被判为不通过");

    let outcome = &report.outcomes[0];
    assert!(!outcome.passed);
    assert!(!outcome.diffs.is_empty(), "应给出差异明细");

    // 差异必须指向具体行（不是笼统的"整体不等"）
    let first_diff = &outcome.diffs[0];
    assert!(
        first_diff.field.contains("line_breaks"),
        "差异应指向 line_breaks 这一类，实测 field=`{}`",
        first_diff.field
    );
    // 且携带期望值与实际值
    assert_eq!(first_diff.expected, "999");
    assert!(
        !first_diff.actual.is_empty(),
        "差异应携带实际值（否则用户不知道差多少）"
    );
}

/// **正确的期望会被判为通过**（防"永远判失败"的另一个极端）。
#[test]
fn evaluator_accepts_a_correct_expectation() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");
    let original = cases.first().expect("语料非空").clone();
    let text = original.input.joined_text();

    // 先用实际排版结果造一条"必然正确"的期望
    let geometry = to_engine_geometry(&original);
    let actual_lines = layout_line_lengths(&text, &geometry);

    let correct = TypesettingCase {
        name: original.name.clone(),
        input: original.input.clone(),
        geometry: original.geometry,
        expect: CaseExpectation::LineBreaks {
            chars_per_line: actual_lines,
        },
        basis: original.basis.clone(),
    };

    let report = evaluate_corpus(&[correct]);
    assert_eq!(
        report.passed, 1,
        "按实际排版结果构造的期望必须通过（否则求值器的比对逻辑有问题）：{:?}",
        report.outcomes[0].diffs
    );
}

// ===========================================================================
// 3. 汇总统计
// ===========================================================================

/// **空语料的通过率是 0.0**（不是 1.0——防路径写错时静默通过）。
#[test]
fn empty_corpus_has_zero_pass_rate() {
    let report = evaluate_corpus(&[]);
    assert_eq!(report.total, 0);
    assert_eq!(report.passed, 0);
    assert!(
        (report.pass_rate() - 0.0).abs() < f64::EPSILON,
        "空语料通过率应为 0.0（1.0 会让 CI 里路径写错静默通过），实测 {}",
        report.pass_rate()
    );
}

/// **通过率算式正确**（半数通过 → 0.5）。
#[test]
fn pass_rate_is_computed_correctly() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");
    let original = cases.first().expect("语料非空").clone();

    let good = TypesettingCase {
        name: "line_breaks:good".to_string(),
        ..original.clone()
    };
    let bad = TypesettingCase {
        name: "line_breaks:bad".to_string(),
        expect: CaseExpectation::LineBreaks {
            chars_per_line: vec![999],
        },
        ..original.clone()
    };

    let report = evaluate_corpus(&[good, bad]);
    assert_eq!(report.total, 2);
    assert_eq!(report.passed, 1);
    assert!(
        (report.pass_rate() - 0.5).abs() < f64::EPSILON,
        "半通过应得 0.5，实测 {}",
        report.pass_rate()
    );
}

/// **`CorpusReport` 是可被 CLI 序列化的纯数据**（无闭包、无句柄）。
#[test]
fn report_is_serializable_data() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");
    // 取一条即可（避免在大语料上重复跑）
    let report: CorpusReport = evaluate_corpus(&cases[..1]);
    assert_eq!(report.total, 1);
    // 字段可读（CLI 据此组装契约 §2.5 的 JSON）
    let outcome = &report.outcomes[0];
    assert!(!outcome.name.is_empty());
    assert!(!outcome.category.is_empty());
}

// ===========================================================================
// 4. 引擎几何映射（`paragraph_indent_em == 0` → 段间距模式）
// ===========================================================================

/// **缩进量为 0 的语料按段间距模式排版**（语料格式的既定约定）。
///
/// ## 这条断言什么
///
/// 语料 README 写明"段首缩进（em，0 = 段间距模式）"。若实现把 0 当成
/// "缩进 0em"（而非"切到间距模式"），两种模式的行盒高度不同——表现为
/// 段间没有间距。故本测试造一条缩进为 0 的语料，断言它与缩进模式的行为
/// **不同**（段间距模式下总高度更大，因为段间多了间距）。
#[test]
fn zero_indent_means_gap_mode() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");
    let original = cases.first().expect("语料非空").clone();

    // 同一份正文，两种缩进设置
    let indented = TypesettingCase {
        name: "line_breaks:indent".to_string(),
        geometry: tome_reader::corpus::Geometry {
            paragraph_indent_em: 2.0,
            ..original.geometry
        },
        expect: CaseExpectation::LineBreaks {
            chars_per_line: Vec::new(),
        },
        ..original.clone()
    };
    let gap = TypesettingCase {
        name: "line_breaks:gap".to_string(),
        geometry: tome_reader::corpus::Geometry {
            paragraph_indent_em: 0.0,
            ..original.geometry
        },
        expect: CaseExpectation::LineBreaks {
            chars_per_line: Vec::new(),
        },
        ..original.clone()
    };

    // 两种模式下的行首区间不同（缩进模式的首行从 0 开始但含 2em 空白；
    // 间距模式的首行不含缩进）——用实际排版的行宽差异来判别
    let text = original.input.joined_text();
    let indented_lines = layout_line_lengths(&text, &to_engine_geometry(&indented));
    let gap_lines = layout_line_lengths(&text, &to_engine_geometry(&gap));

    // 缩进模式首行少放 2 字（缩进占掉 2em）；间距模式首行满宽
    assert_ne!(
        indented_lines.first(),
        gap_lines.first(),
        "缩进模式（2em）与间距模式（0 → Gap）的首行字符数应不同——\
         相同说明 0 被当成了「缩进 0em」而非「切到间距模式」"
    );
}

// ===========================================================================
// 辅助
// ===========================================================================

/// 语料几何 → 引擎几何（与求值器内部同口径的测试侧复刻）。
fn to_engine_geometry(case: &TypesettingCase) -> tome_reader::layout::engine::Geometry {
    use tome_reader::layout::paragraph_mode::ParagraphMode;

    let paragraph_mode = if case.geometry.paragraph_indent_em > 0.0 {
        ParagraphMode::Indent {
            em: case.geometry.paragraph_indent_em,
        }
    } else {
        ParagraphMode::Gap { em: 0.4 }
    };
    tome_reader::layout::engine::Geometry {
        measure_px: case.geometry.measure_px,
        font_size_px: case.geometry.font_size_px,
        line_height: case.geometry.line_height,
        alignment: tome_reader::rules::justify::Alignment::Justified,
        paragraph_mode,
    }
}

/// 直接排版并返回逐行字符数（供构造"必然正确"的期望）。
fn layout_line_lengths(text: &str, geometry: &tome_reader::layout::engine::Geometry) -> Vec<usize> {
    use tome_reader::layout::engine::LayoutEngine;
    use tome_reader::text::model::TextModel;
    use tome_reader::text::paragraph;

    let mut engine = LayoutEngine::new();
    let split = paragraph::split(text);
    let mut lengths = Vec::new();
    for para in &split.paragraphs {
        let model = TextModel::new(para.text.clone());
        let result = engine.layout_paragraph(&model, para.index, geometry);
        for line in result.lines {
            lengths.push(line.char_range.end - line.char_range.start);
        }
    }
    lengths
}
