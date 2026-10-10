//! 合成差分语料的**通过率验收**（tasks.md T052 / SC-002a 的落地证据）
//!
//! # 这个测试为什么必须在仓库里
//!
//! SC-002a 是**本阶段的兼容性门**：合成语料差分通过率 ≥95%。它是"书源
//! 兼容性"这一最高优先级（宪法原则 I）的唯一**可复现度量**。
//!
//! 把度量做成测试（而不是只在汇报里写一个数字）有两个收益：
//!
//! 1. **回归防线**：任何一次改动导致通过率跌破阈值，CI 立刻红
//! 2. **可核对**：数字来自代码而不是人的记忆
//!
//! # 为什么还要按类别断言
//!
//! 一个 95% 的总通过率可能掩盖"某一类语法全错"——比如 CSS 全对而
//! XPath 全错，总数仍可能过线。故本测试对**每个语法类别**单独设阈值，
//! 让这种情形无处躲藏。
//!
//! # 语料位置
//!
//! `tests/corpus/`（相对仓库根）。用 `CARGO_MANIFEST_DIR` 上溯定位，
//! 而不是依赖当前工作目录——`cargo test` 的工作目录是 crate 根，
//! 而语料在 workspace 根。

// `.clippy.toml` 的 `allow-expect-in-tests` 覆盖 `#[test]` 函数体，
// 但**不覆盖**本文件里的辅助函数（如 `corpus_directory`、`run_full_corpus`）。
// 这些辅助函数若失败，说明测试环境本身有问题（语料目录被删、文件格式坏了），
// 此时让调用它的那个测试当场失败是正确行为。
// 故显式豁免并说明理由——与 `tome-cli/tests/minimal_loop.rs` 同一处理。
#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::path::PathBuf;

use tome_rule::corpus::{load_corpus_directory, run_corpus, CorpusReport};
use tome_rule::js_bridge::JobRuntime;

/// 定位仓库根的 `tests/corpus/`。
fn corpus_directory() -> PathBuf {
    // `crates/tome-rule` → 上溯两级到 workspace 根
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|path| path.parent())
        .expect("crate 目录应有两级父目录");
    root.join("tests").join("corpus")
}

/// 跑整个语料目录，产出报告。
///
/// JS 运行时是**必须**给的：语料里有 `js:` 类用例，不给引擎它们会全部
/// 失败（且失败原因是环境缺失而非引擎缺陷，那样测出来的通过率没有意义）。
fn run_full_corpus() -> Vec<CorpusReport> {
    let directory = corpus_directory();
    assert!(
        directory.is_dir(),
        "语料目录应存在：{}（它是 SC-002a 的载体）",
        directory.display()
    );

    let runtime = JobRuntime::new(tome_js::Limits::default()).expect("应能建 JS 沙箱");
    let files = load_corpus_directory(&directory).expect("语料应能加载");

    assert!(!files.is_empty(), "语料目录里应至少有一个 .json 文件");

    files
        .into_iter()
        .map(|(path, cases)| CorpusReport {
            source: path.display().to_string(),
            outcomes: run_corpus(&cases, Some(&runtime)),
        })
        .collect()
}

/// 把失败用例渲染成可读的逐条 diff。
fn render_failures(report: &CorpusReport) -> String {
    let mut rendered = String::new();
    for outcome in report.failures() {
        rendered.push_str(&format!("  ✗ {}\n", outcome.name));
        rendered.push_str(&format!("      期望: {:?}\n", outcome.expected));
        match &outcome.error {
            Some(error) => rendered.push_str(&format!("      求值失败: {error}\n")),
            None => rendered.push_str(&format!("      实际: {:?}\n", outcome.actual)),
        }
    }
    rendered
}

// ---------------------------------------------------------------------------
// SC-002a：总通过率 ≥95%
// ---------------------------------------------------------------------------

#[test]
fn 合成语料差分通过率达标() {
    let reports = run_full_corpus();

    let total: usize = reports.iter().map(CorpusReport::total).sum();
    let passed: usize = reports.iter().map(CorpusReport::passed).sum();
    let rate = passed as f64 / total as f64;

    // 阈值取自 `tome_rule::corpus::CORPUS_PASS_THRESHOLD`（SC-002a 的单一来源），
    // 不在这里另写一个 95——那样两处会各自演化。
    if rate < tome_rule::corpus::CORPUS_PASS_THRESHOLD {
        let mut detail = String::new();
        for report in &reports {
            if report.passed() < report.total() {
                detail.push_str(&format!(
                    "\n{}（{}/{}）：\n{}",
                    report.source,
                    report.passed(),
                    report.total(),
                    render_failures(report)
                ));
            }
        }
        panic!(
            "SC-002a 未达标：{passed}/{total} = {:.2}%（要求 ≥{:.0}%）{}",
            rate * 100.0,
            tome_rule::corpus::CORPUS_PASS_THRESHOLD * 100.0,
            detail
        );
    }
}

// ---------------------------------------------------------------------------
// 逐类别通过率：防"某类全错但总数过线"
// ---------------------------------------------------------------------------

#[test]
fn 每个语法类别都有用例且通过率达标() {
    let reports = run_full_corpus();

    // 合并全部报告的类别统计
    let mut totals: Vec<(String, usize, usize)> = Vec::new();
    for report in &reports {
        for (category, passed, total) in report.by_category() {
            match totals.iter_mut().find(|(name, _, _)| *name == category) {
                Some((_, passed_sum, total_sum)) => {
                    *passed_sum += passed;
                    *total_sum += total;
                }
                None => totals.push((category, passed, total)),
            }
        }
    }
    totals.sort();

    // tasks.md 的检查点要求"CSS/JSONPath/XPath/正则/JS 各语法点均有用例"
    for required in ["css", "default", "jsonpath", "xpath", "regex", "js"] {
        let found = totals.iter().find(|(name, _, _)| name == required);
        let (_, passed, total) = found
            .unwrap_or_else(|| panic!("语料里缺少 `{required}` 类的用例——检查点的要求是每类都有"));
        assert!(
            *total >= 5,
            "`{required}` 类只有 {total} 条用例，覆盖不足（每类至少 5 条）"
        );
        let rate = *passed as f64 / *total as f64;
        assert!(
            rate >= tome_rule::corpus::CORPUS_PASS_THRESHOLD,
            "`{required}` 类通过率 {passed}/{total} = {:.2}%，低于阈值——\
             某一类语法整体失效被总通过率掩盖了",
            rate * 100.0
        );
    }
}

// ---------------------------------------------------------------------------
// 语料自身的质量约束
// ---------------------------------------------------------------------------

#[test]
fn 每条用例都有依据来源() {
    // tasks.md T052 的原文要求"期望值依据成文，不靠实现者直觉填期望值"。
    // `parse_corpus` 在反序列化层已强制 `basis` 存在且非空；本测试补一条
    // **语义**检查：依据文本要提到一个可核对的来源名。
    let directory = corpus_directory();
    let files = load_corpus_directory(&directory).expect("语料应能加载");

    // 允许的来源标记（与 tests/corpus/README.md 的清单一致）
    const SOURCE_MARKERS: [&str; 10] = [
        "Jsoup",
        "JsoupXpath",
        "W3C XPath",
        "RFC 9535",
        "Jayway",
        "ECMAScript",
        "书源规则",
        "research.md",
        "HTML5",
        "WHATWG",
    ];

    for (path, cases) in &files {
        for case in cases {
            let has_source = SOURCE_MARKERS
                .iter()
                .any(|marker| case.basis.contains(marker));
            assert!(
                has_source,
                "{} 的用例 `{}` 的 basis 未提到可核对来源：`{}`\n\
                 允许的来源见 tests/corpus/README.md",
                path.display(),
                case.name,
                case.basis
            );
        }
    }
}

#[test]
fn 用例名唯一且带类别前缀() {
    let directory = corpus_directory();
    let files = load_corpus_directory(&directory).expect("语料应能加载");

    let mut seen: Vec<String> = Vec::new();
    for (path, cases) in &files {
        for case in cases {
            // 跨文件唯一：报告里"哪条失败了"要能唯一定位
            let qualified = format!("{}::{}", path.display(), case.name);
            assert!(
                !seen.contains(&case.name),
                "用例名 `{}` 重复（{}）——重名会让失败报告无从定位",
                case.name,
                qualified
            );
            seen.push(case.name.clone());

            // 类别前缀：`load_corpus` 按第一个冒号前的内容分类，
            // 没有冒号会落到 `other` 而逃过逐类别断言
            assert!(
                case.name.contains(':'),
                "用例 `{}` 的名字缺类别前缀（形如 `css:xxx`）——\
                 没有前缀会被归到 other 而绕过逐类别通过率检查",
                case.name
            );
        }
    }
}
