//! T004 验收：语料目录的装载与完整性。
//!
//! ## 为什么这组断言要与 loader 的单测分开
//!
//! `src/corpus.rs` 的单测用**合成 JSON** 验证解析与校验逻辑；本文件用
//! **仓库里真实存在的语料**验证三件事：
//!
//! 1. 目录能被装载（路径接线正确，不是"测试通过但 CI 根本读不到语料"）
//! 2. 条目数达到 T004 的下限要求（禁则/挤压/混排各一）
//! 3. 每条 `basis` 都指向**可核对的来源**（而非"随意"这类占位词）
//!
//! 第 3 条是这两个层级的真正分工：格式合法性在 loader 里管，**内容质量**
//! 只有拿到真实语料才谈得上。

use std::collections::BTreeSet;
use std::path::PathBuf;

use tome_reader::corpus::{
    category_of, count_by_category, load_corpus_directory, parse_corpus, CaseExpectation,
};

/// 语料目录（相对 crate 根）。
fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

#[test]
fn real_corpus_directory_loads() {
    let cases = load_corpus_directory(&corpus_dir()).expect("仓库内的语料目录必须可装载");

    assert!(
        !cases.is_empty(),
        "语料目录存在但装载结果为空——loader 的过滤条件可能过严"
    );
}

/// T004 验收子项：**≥3 条示例条目（禁则/挤压/混排各一）**。
#[test]
fn corpus_covers_the_three_required_rule_families() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");

    let categories = count_by_category(&cases);
    for required in ["kinsoku", "squeeze", "mixed"] {
        let count = categories.get(required).copied().unwrap_or(0);
        assert!(
            count >= 1,
            "语料缺少 `{required}` 类别（T004 要求禁则/挤压/混排各一）；现有类别：{categories:?}"
        );
    }

    // 禁则类还要求覆盖"例外记录"这一支——FR-008 的第 ④ 步是硬序里唯一会
    // 留下痕迹的分支，没有语料覆盖它，SC-002 的例外计数就没有判据来源。
    assert!(
        categories.contains_key("kinsoku_exceptions"),
        "语料缺少硬序第 ④ 步的例外记录覆盖（FR-008 / SC-002 的计数判据）"
    );
}

/// `basis` 必须指向**可核对的来源**，而不是占位词。
///
/// ## 为什么要查这个（而不是只查非空）
///
/// loader 已拦"空串"与"缺字段"，但拦不住 `"basis": "随便写的"` ——
/// 那在格式上完全合法。真实语料的 `basis` 应命中已知的依据词表，
/// 本测试是对**内容质量**的最小把关。
#[test]
fn every_basis_cites_a_checkable_source() {
    const KNOWN_SOURCES: &[&str] = &[
        "UAX#14",
        "LineBreak.txt",
        "CLreq",
        "W3C",
        "Unicode",
        "MASTER.md",
        "FR-",
        "SC-",
        "OPEN-QUESTIONS",
        "research",
    ];

    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");

    for case in &cases {
        let basis = case.basis.trim();
        assert!(
            KNOWN_SOURCES.iter().any(|source| basis.contains(source)),
            "用例 `{}` 的 basis 未引用任何已知依据来源（{KNOWN_SOURCES:?}）。\n\
             排版规则的依据只能是公开规范（UAX#14/CLreq）、设计规范（MASTER.md）\
             或项目裁决（FR/SC/OQ 编号）——不得写「照抄上游」，那违反干净室纪律。\n\
             实际 basis：{basis}",
            case.name
        );

        // 占位词反向检查：这些词出现即说明 basis 是敷衍填的。
        for placeholder in ["照抄", "上游源码", "TODO", "待补", "无所谓"] {
            assert!(
                !basis.contains(placeholder),
                "用例 `{}` 的 basis 含占位词 `{placeholder}`：{basis}",
                case.name
            );
        }
    }
}

/// 用例名唯一且都带类别前缀（loader 已保证；此处对**真实语料**复述一遍，
/// 因为 loader 的校验只在其单测里被验证过，真实语料可能触到新路径）。
#[test]
fn real_corpus_names_are_unique_and_prefixed() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");

    let mut names = BTreeSet::new();
    for case in &cases {
        assert!(
            names.insert(case.name.clone()),
            "真实语料里有重复用例名：{}",
            case.name
        );
        assert!(case.name.contains(':'), "用例 `{}` 缺类别前缀", case.name);
        // 类别前缀必须与 expect 的形态对得上（否则分类统计会指错方向）
        let prefix = category_of(case);
        let expected_category = case.expect.category();
        assert!(
            prefix == expected_category
                || (prefix == "kinsoku" && expected_category == "kinsoku")
                || (prefix == "mixed" && expected_category == "mixed_script"),
            "用例 `{}` 的名字前缀 `{prefix}` 与期望形态 `{expected_category}` 不一致——\
             分类统计按前缀分组，错配会让某一类的覆盖率被误算",
            case.name
        );
    }
}

/// 语料里的几何参数必须自洽（正数、行高在无障碍下限之上）。
///
/// 这条防的是"语料本身违反设计规范"：若某条用例写了 `line_height: 1.4`，
/// 它会成为一条**错误的基准**——实现按它排出来的结果反而"通过"了，
/// 而那个结果是违反 MASTER §2.3 强制条款的。
#[test]
fn corpus_geometry_respects_design_constraints() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");

    for case in &cases {
        let geometry = case.geometry;
        assert!(
            geometry.measure_px > 0.0,
            "用例 `{}` 的版心宽必须为正",
            case.name
        );
        assert!(
            geometry.font_size_px > 0.0,
            "用例 `{}` 的字号必须为正",
            case.name
        );
        assert!(
            geometry.line_height >= 1.75,
            "用例 `{}` 的行高 {} 低于 MASTER §2.3 的强制下限 1.75——\
             它会把违反无障碍条款的结果变成基准",
            case.name,
            geometry.line_height
        );
        assert!(
            geometry.paragraph_indent_em >= 0.0,
            "用例 `{}` 的段首缩进不得为负",
            case.name
        );
        // 缩进模式与间距模式的互斥（FR-018）：缩进为 0 即间距模式，
        // 缩进非 0 即缩进模式——不存在"两者都有"的合法取值。
        assert!(
            geometry.paragraph_indent_em == 0.0 || geometry.paragraph_indent_em >= 1.0,
            "用例 `{}` 的段首缩进 {} 既非 0（间距模式）也非 ≥1em（缩进模式），\
             违反 FR-018 的互斥约定",
            case.name,
            geometry.paragraph_indent_em
        );
    }
}

/// loader 与语料格式的**端到端**：每条用例都能取到非空段落文本。
#[test]
fn every_case_has_non_empty_input() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");

    for case in &cases {
        let paragraphs = case.input.paragraphs();
        assert!(
            !paragraphs.is_empty(),
            "用例 `{}` 没有任何段落——空输入会让任何断言都平凡成立",
            case.name
        );
        assert!(
            paragraphs.iter().any(|text| !text.trim().is_empty()),
            "用例 `{}` 的段落全为空白",
            case.name
        );
    }
}

/// 期望形态的**覆盖矩阵**：五类形态中至少四类有真实语料。
///
/// 不要求五类全有（`line_breaks` 的逐行字数断言在 S2-1 实现稳定前难以手工
/// 给出可靠值，它的覆盖可由生成的基准补），但四类是底线——否则语料对实现
/// 的约束力太薄。
#[test]
fn corpus_expectation_forms_are_broadly_covered() {
    let cases = load_corpus_directory(&corpus_dir()).expect("语料必须可装载");

    let mut forms = BTreeSet::new();
    for case in &cases {
        forms.insert(match &case.expect {
            CaseExpectation::LineBreaks { .. } => "line_breaks",
            CaseExpectation::Kinsoku { .. } => "kinsoku",
            CaseExpectation::KinsokuExceptions { .. } => "kinsoku_exceptions",
            CaseExpectation::Squeeze { .. } => "squeeze",
            CaseExpectation::MixedScript { .. } => "mixed_script",
        });
    }

    assert!(
        forms.len() >= 4,
        "语料的期望形态只有 {forms:?}，覆盖不足（至少四类）"
    );
}

/// 解析器对真实文件内容也成立（不经目录装载路径）——两条入口必须一致。
#[test]
fn direct_parse_matches_directory_load() {
    let dir = corpus_dir();
    let text = std::fs::read_to_string(dir.join("kinsoku.json")).expect("语料文件必须可读");
    let direct = parse_corpus(&text).expect("直接解析必须成功");

    let loaded = load_corpus_directory(&dir).expect("目录装载必须成功");
    assert!(
        loaded.len() >= direct.len(),
        "目录装载的结果不应少于单文件解析（{} vs {}）",
        loaded.len(),
        direct.len()
    );

    for case in &direct {
        assert!(
            loaded.iter().any(|other| other.name == case.name),
            "直接解析出的用例 `{}` 未出现在目录装载结果里——两条入口不一致",
            case.name
        );
    }
}
