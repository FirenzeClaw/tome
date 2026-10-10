//! `tome verify` 的端到端验收（tasks.md T052 / SC-002a 的 Independent Test）
//!
//! # 这是本阶段 Independent Test 的载体
//!
//! tasks.md 的 Phase 5 原文：
//!
//! > **Independent Test**: `tome verify --corpus tests/corpus/` 输出一致率
//! > ≥95%；逐条 diff 可查。
//!
//! 故本文件走**真实的 CLI 路径**（`flags::parse` → `commands::run`），
//! 不直接调 `tome_rule::corpus` 的函数——那样测的是库而不是命令，
//! 而契约（`contracts/cli.md`）约束的是命令。
//!
//! # 覆盖的三件事
//!
//! | 测试 | 对应契约 |
//! |---|---|
//! | 仓库语料通过率达标 | 一致率 ≥95% |
//! | 报告结构含逐条 diff | "逐条 diff 可查" |
//! | 未达标时退出码为 1 | 全局退出码约定（1 = 业务错误） |

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::path::PathBuf;

use tome_cli::flags::{parse, Command, GlobalOptions, OutputFormat, ParseOutcome};

/// 仓库根的 `tests/corpus/`。
///
/// 用 `CARGO_MANIFEST_DIR` 上溯定位，而不是依赖当前工作目录——
/// `cargo test` 的工作目录是 crate 根，而语料在 workspace 根。
fn corpus_directory() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .parent()
        .and_then(|path| path.parent())
        .expect("crate 目录应有两级父目录");
    root.join("tests").join("corpus")
}

/// 跑一次 `verify --corpus <dir>`，返回 (退出码, stdout, stderr)。
fn run_verify(corpus: &str) -> (i32, String, String) {
    let args: Vec<String> = vec![
        "verify".to_string(),
        "--corpus".to_string(),
        corpus.to_string(),
    ];
    let (options, command) = match parse(&args) {
        ParseOutcome::Run { options, command } => (options, command),
        other => panic!("应解析成功，实际 {other:?}"),
    };

    // verify 不发请求，但仍要一个宿主——用默认（不联网的）执行器。
    // 这里显式构造而不复用 `--replay`：`verify` 与网络模式无关。
    let executor = tome_cli::build_executor(&options.network, None).expect("应能构造执行器");
    let mut stdin = std::io::empty();
    let outcome = tome_cli::commands::run(&options, &command, &executor, &mut stdin);
    (outcome.exit_code, outcome.stdout, outcome.stderr)
}

// ---------------------------------------------------------------------------
// 参数解析
// ---------------------------------------------------------------------------

#[test]
fn verify_参数被正确解析() {
    let args: Vec<String> = vec![
        "verify".to_string(),
        "--corpus".to_string(),
        "tests/corpus".to_string(),
    ];
    match parse(&args) {
        ParseOutcome::Run { command, .. } => match command {
            Command::Verify { corpus, report } => {
                assert_eq!(corpus, PathBuf::from("tests/corpus"));
                assert!(report.is_none());
            }
            other => panic!("应为 verify 命令，实际 {other:?}"),
        },
        other => panic!("应解析成功，实际 {other:?}"),
    }
}

#[test]
fn verify_缺语料目录是用法错误() {
    let args: Vec<String> = vec!["verify".to_string()];
    match parse(&args) {
        ParseOutcome::Usage(text) => assert!(text.contains("--corpus"), "{text}"),
        other => panic!("应为用法错误，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 一致率（SC-002a）
// ---------------------------------------------------------------------------

#[test]
fn 仓库语料的一致率达标() {
    let directory = corpus_directory();
    assert!(
        directory.is_dir(),
        "语料目录应存在：{}",
        directory.display()
    );

    let (exit_code, stdout, stderr) = run_verify(&directory.to_string_lossy());
    assert_eq!(
        exit_code, 0,
        "SC-002a 应达标（退出码 0）\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let report: serde_json::Value = serde_json::from_str(&stdout).expect("报告应是合法 JSON");
    let total = report["total"].as_u64().expect("应含 total");
    let passed = report["passed"].as_u64().expect("应含 passed");
    let rate = report["pass_rate"].as_f64().expect("应含 pass_rate");

    assert!(total >= 100, "语料应有足够规模，实际 {total} 条");
    assert_eq!(
        report["meets_threshold"].as_bool(),
        Some(true),
        "报告应自述达标"
    );
    assert!(
        rate >= 0.95,
        "SC-002a 要求 ≥95%，实际 {passed}/{total} = {:.2}%",
        rate * 100.0
    );
    println!("SC-002a 实测：{passed}/{total} = {:.2}%", rate * 100.0);
}

#[test]
fn 报告含逐条分类统计() {
    let directory = corpus_directory();
    let (_, stdout, _) = run_verify(&directory.to_string_lossy());
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("报告应是合法 JSON");

    let categories = report["by_category"]
        .as_array()
        .expect("应含 by_category 数组");
    // tasks.md 的检查点要求"CSS/JSONPath/XPath/正则/JS 各语法点均有用例"
    let names: Vec<&str> = categories
        .iter()
        .filter_map(|entry| entry["category"].as_str())
        .collect();
    for required in ["css", "jsonpath", "xpath", "regex", "js"] {
        assert!(
            names.contains(&required),
            "报告应含 `{required}` 类统计，实际 {names:?}"
        );
    }
    // 每类都要有通过率字段（fixture 里它被用来防"某类全错但总数过线"）
    for entry in categories {
        assert!(entry["rate"].is_f64(), "每条分类统计应含 rate：{entry}");
        assert!(entry["total"].is_u64(), "每条分类统计应含 total：{entry}");
    }
}

#[test]
fn 达标时无失败条目() {
    let directory = corpus_directory();
    let (_, stdout, _) = run_verify(&directory.to_string_lossy());
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("报告应是合法 JSON");
    let failures = report["failures"].as_array().expect("应含 failures 数组");
    assert!(
        failures.is_empty(),
        "达标时不该有失败条目，实际：{failures:?}"
    );
}

// ---------------------------------------------------------------------------
// 未达标路径（门必须真的会拦）
// ---------------------------------------------------------------------------

#[test]
fn 未达标时退出码为业务错误() {
    // 造一个必然失败的语料：期望值与实际不符。
    //
    // 这条测试是"门真的会拦"的证据——只有通过的路径被测过，
    // 就不知道门是开的还是根本没装。
    let temp = tempfile::tempdir().expect("应能建临时目录");
    let corpus = temp.path().join("broken.json");
    let content = r#"[
        {
            "name": "css:必然失败",
            "kind": "html",
            "input": "<p>实际</p>",
            "rule": "p@text",
            "expected": ["期望"],
            "basis": "本测试内置（故意与实现不符）"
        }
    ]"#;
    std::fs::write(&corpus, content).expect("应能写语料");

    let (exit_code, stdout, stderr) = run_verify(&temp.path().to_string_lossy());
    assert_eq!(
        exit_code, 1,
        "未达标应是业务错误（1）而不是成功\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("css:必然失败"),
        "报告应指认失败的用例名：{stderr}"
    );
    assert!(
        stderr.contains("期望") && stderr.contains("实际"),
        "报告应给出期望与实际的 diff：{stderr}"
    );
}

#[test]
fn 语料目录不存在时报错而非静默成功() {
    let (exit_code, _, stderr) = run_verify("definitely/not/a/directory");
    assert_ne!(exit_code, 0, "目录不存在不该报成功");
    assert!(
        stderr.contains("不存在") || stderr.contains("不是目录"),
        "错误应说明原因：{stderr}"
    );
}

#[test]
fn 格式错误的语料被拒绝() {
    let temp = tempfile::tempdir().expect("应能建临时目录");
    // 缺 `basis`——T052 的"期望值依据成文"由类型系统强制
    let corpus = temp.path().join("bad.json");
    std::fs::write(
        &corpus,
        r#"[{"name":"x","kind":"html","input":"a","rule":"b","expected":[]}]"#,
    )
    .expect("应能写语料");

    let (exit_code, _, stderr) = run_verify(&temp.path().to_string_lossy());
    assert_ne!(exit_code, 0);
    assert!(
        stderr.contains("语料加载失败") || stderr.contains("语料格式错误"),
        "错误应指明语料格式问题：{stderr}"
    );
}

// ---------------------------------------------------------------------------
// 报告文件
// ---------------------------------------------------------------------------

#[test]
fn report_参数把报告写到文件() {
    let temp = tempfile::tempdir().expect("应能建临时目录");
    let report_path = temp.path().join("report.json");

    let args: Vec<String> = vec![
        "verify".to_string(),
        "--corpus".to_string(),
        corpus_directory().to_string_lossy().to_string(),
        "--report".to_string(),
        report_path.to_string_lossy().to_string(),
    ];
    let (options, command) = match parse(&args) {
        ParseOutcome::Run { options, command } => (options, command),
        other => panic!("应解析成功，实际 {other:?}"),
    };

    let executor = tome_cli::build_executor(&options.network, None).expect("应能构造执行器");
    let mut stdin = std::io::empty();
    let outcome = tome_cli::commands::run(&options, &command, &executor, &mut stdin);
    assert_eq!(outcome.exit_code, 0, "stderr: {}", outcome.stderr);

    let written = std::fs::read_to_string(&report_path).expect("报告文件应存在");
    let report: serde_json::Value = serde_json::from_str(&written).expect("报告应是合法 JSON");
    assert_eq!(report["meets_threshold"].as_bool(), Some(true));
}

// ---------------------------------------------------------------------------
// 与 options 的交互
// ---------------------------------------------------------------------------

#[test]
fn verify_不受输出格式选项影响() {
    // 报告的 JSON 是**契约**（`contracts/cli.md` 的全局约定），
    // 不该因为 `--format human` 而变成别的形状——脚本要靠它做判定。
    let args: Vec<String> = vec![
        "--format".to_string(),
        "human".to_string(),
        "verify".to_string(),
        "--corpus".to_string(),
        corpus_directory().to_string_lossy().to_string(),
    ];
    let (options, command) = match parse(&args) {
        ParseOutcome::Run { options, command } => (options, command),
        other => panic!("应解析成功，实际 {other:?}"),
    };
    assert_eq!(options.format, OutputFormat::Human);

    let executor = tome_cli::build_executor(&options.network, None).expect("应能构造执行器");
    let mut stdin = std::io::empty();
    let outcome = tome_cli::commands::run(&options, &command, &executor, &mut stdin);
    assert_eq!(outcome.exit_code, 0);
    serde_json::from_str::<serde_json::Value>(&outcome.stdout)
        .expect("报告应始终是合法 JSON（与 --format 无关）");

    let _: &GlobalOptions = &options;
}
