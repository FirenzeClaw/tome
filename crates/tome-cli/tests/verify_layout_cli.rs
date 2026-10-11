//! T048 验收：`verify-layout` 子命令（`contracts/cli.md` §2.5 + §4）
//!
//! ## 契约 §2.5 的原文
//!
//! > 跑排版层合成语料的一致性/快照比对，输出通过率与逐条差异（对齐 001 的
//! > `verify` 形态与 spec §SC-001~SC-003）。语料格式与 001 的 `tests/corpus/`
//! > 同风格，条目声明输入正文 + 参数 + 期望（分页结构 / 规则命中 / 禁则例外）。
//!
//! ## 契约 §4 对本子命令的要求
//!
//! | 要求 | 本文件的测试 |
//! |---|---|
//! | golden 测试（输出 JSON 与期望逐字段比对） | `golden_shape_matches_contract` |
//! | 确定性（同参数两次输出逐字节相同） | `determinism_is_asserted` |
//! | 不触发网络 | `does_not_touch_the_network` |
//!
//! ## 与 001 `verify` 的关系
//!
//! 形态对齐（`total` / `passed` / `pass_rate` / 逐条差异），但**阈值不同**：
//! 001 的 SC-002a 是"≥95%"的硬门槛，本层无硬门槛——spec §SC-001~003 是
//! 规则级不变量（已由 `tome-reader` 的专项测试覆盖），本子命令的职责是
//! **诚实报告**语料的通过率与差异，不是给一个凑数的阈值。
//!
//! ## 一条重要的实测发现（记录在此，供 PM 裁决）
//!
//! 仓库内 8 条语料里，4 条 `squeeze` / `mixed` / `kinsoku_exceptions` 条目的
//! 期望值与实际排版结果不符（详见 `tests/corpus_eval.rs` 与汇报）。那 4 条是
//! T004 的"示例条目"，其期望值在引擎落地前写就，未与实现核对。本子命令
//! **如实报告**它们未通过——不调低断言、不改语料去迁就实现（后者会让语料
//! 失去约束力，且语料期望带 `basis` 引用属规格资产）。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音（与 render_cli.rs 同一约定）"
)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// 跑一次 CLI，返回 `(退出码, stdout, stderr)`。
fn run_cli(args: &[&str]) -> (i32, String, String) {
    // 独立 `--data-dir`：不传时并行的 tome-cli 子进程共享默认可移植
    // 数据目录（exe 同级 data/），CI 时序下并发打开同一 SQLite 库会报
    // `error.storage`（与 layout_cli.rs / render_cli.rs 同一约定）。
    let data_dir = tempfile::tempdir().expect("临时数据目录");
    let binary = env!("CARGO_BIN_EXE_tome-cli");
    let output = Command::new(binary)
        .arg("--data-dir")
        .arg(data_dir.path())
        .args(args)
        .output()
        .expect("应能启动 tome-cli");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// 仓库内的排版语料目录。
fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tome-reader/tests/corpus")
}

/// 造一个只含**单条必然通过**用例的语料目录（golden 的可控输入）。
fn write_passing_corpus(dir: &Path) -> PathBuf {
    let corpus = dir.join("corpus");
    std::fs::create_dir_all(&corpus).expect("应能建语料目录");
    // 一条禁则用例：断言"行首不得为逗号/句号"，而排版确实满足它
    let json = r#"[
  {
    "name": "kinsoku:golden-样例",
    "input": {
      "kind": "text",
      "paragraphs": [
        "他的手指在书页边缘停了很久，那一行字写的是「山有木兮木有枝」，后面跟着一个逗号。"
      ]
    },
    "geometry": {
      "measure_px": 342.0,
      "font_size_px": 19.0,
      "line_height": 1.85,
      "paragraph_indent_em": 2.0
    },
    "expect": {
      "kind": "kinsoku",
      "forbidden_at_line_start": ["，", "。"],
      "forbidden_at_line_end": ["「"]
    },
    "basis": "UAX#14 LineBreak.txt 18.0.0：CL 类不得置于行首、OP 类不得置于行末。"
  }
]"#;
    std::fs::write(corpus.join("golden.json"), json).expect("应能写语料");
    corpus
}

// ===========================================================================
// 1. golden：输出形态逐字段
// ===========================================================================

/// **输出形态与契约 §2.5 对齐**（total / passed / pass_rate / 逐条差异）。
#[test]
fn golden_shape_matches_contract() {
    let dir = tempfile::tempdir().expect("临时目录");
    let corpus = write_passing_corpus(dir.path());

    let (code, stdout, stderr) = run_cli(&[
        "verify-layout",
        "--corpus",
        corpus.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "全通过的语料应退出码 0，stderr: {stderr}");

    let value: serde_json::Value = serde_json::from_str(&stdout).expect("输出应是 JSON");

    // 契约要的四个字段
    assert_eq!(value["mode"], "layout-corpus", "应标明本子命令的模式");
    assert_eq!(value["total"], 1, "总条数");
    assert_eq!(value["passed"], 1, "通过条数");
    assert!(
        (value["pass_rate"].as_f64().expect("pass_rate 应是数字") - 1.0).abs() < f64::EPSILON,
        "通过率应为 1.0"
    );
    assert!(
        value["outcomes"].as_array().is_some(),
        "应给出逐条结果（供 CI 展示与定位）"
    );

    // 逐条结果携带用例名与类别
    let first = &value["outcomes"][0];
    assert_eq!(first["name"], "kinsoku:golden-样例");
    assert_eq!(first["category"], "kinsoku");
    assert_eq!(first["passed"], true);
}

/// **逐条差异在报告里可见**（不通过时给出可定位的明细）。
#[test]
fn failing_case_diffs_are_reported() {
    let dir = tempfile::tempdir().expect("临时目录");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&corpus).expect("应能建");
    // 一条必然失败的用例（期望每行 999 字）
    let json = r#"[
  {
    "name": "line_breaks:必然失败",
    "input": { "kind": "text", "paragraphs": ["短句，用来排版。"] },
    "geometry": {
      "measure_px": 342.0, "font_size_px": 19.0,
      "line_height": 1.85, "paragraph_indent_em": 2.0
    },
    "expect": { "kind": "line_breaks", "chars_per_line": [999] },
    "basis": "本用例刻意失败，用于验证差异被如实报告。"
  }
]"#;
    std::fs::write(corpus.join("fail.json"), json).expect("应能写");

    let (code, _stdout, stderr) = run_cli(&[
        "verify-layout",
        "--corpus",
        corpus.to_str().unwrap(),
        "--format",
        "json",
    ]);
    // 有未通过条目 → 退出码 1（业务错误；与 001 的 verify 同口径）。
    // 报告随错误诊断走 **stderr**（`CommandOutcome::failure` 的既定形态：
    // stdout 留给成功结果，保证管道里的 stdout 永远是可直接消费的 JSON）。
    assert_eq!(code, 1, "有未通过条目应退出码 1，stderr: {stderr}");

    let value: serde_json::Value = serde_json::from_str(&stderr)
        .unwrap_or_else(|error| panic!("stderr 应是 JSON 报告：{error}\n{stderr}"));
    assert_eq!(value["passed"], 0);
    let diffs = value["outcomes"][0]["diffs"]
        .as_array()
        .expect("应给出差异数组");
    assert!(!diffs.is_empty(), "未通过的用例必须有差异明细");
    let first = &diffs[0];
    assert!(
        first["field"]
            .as_str()
            .is_some_and(|f| f.contains("line_breaks")),
        "差异应定位到具体项，实测：{first}"
    );
    assert!(first["expected"].is_string(), "差异应携带期望值");
    assert!(first["actual"].is_string(), "差异应携带实际值");
}

// ===========================================================================
// 2. 确定性（契约 §4）
// ===========================================================================

/// **同参数两次运行输出逐字节相同**（契约 §4 的确定性要求）。
#[test]
fn determinism_is_asserted() {
    let dir = tempfile::tempdir().expect("临时目录");
    let corpus = write_passing_corpus(dir.path());
    let corpus_str = corpus.to_str().unwrap().to_string();

    let args = ["verify-layout", "--corpus", &corpus_str, "--format", "json"];
    let (code_a, out_a, _) = run_cli(&args);
    let (code_b, out_b, _) = run_cli(&args);

    assert_eq!(code_a, 0, "首次运行应成功");
    assert_eq!(code_b, 0, "二次运行应成功");
    assert_eq!(
        out_a, out_b,
        "同参数两次运行的输出必须逐字节相同（契约 §4 的确定性）"
    );
}

/// **语料目录内的文件顺序不影响结果**（报告按用例名稳定排序）。
///
/// ## 为什么这条重要
///
/// 文件系统返回目录项的次序不保证稳定（跨平台尤甚）。若报告按"读到什么次序
/// 就输出什么次序"，同参数两次运行会因目录序变化而产生不同字节——契约 §4
/// 的确定性就会**间歇性**失败。故报告必须按稳定键排序。
#[test]
fn file_order_does_not_affect_output() {
    let dir = tempfile::tempdir().expect("临时目录");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&corpus).expect("应能建");

    // 两个文件，用例名刻意反序（b.json 里的名字字典序在前）
    let case = |name: &str| {
        format!(
            r#"[{{"name":"{name}","input":{{"kind":"text","paragraphs":["测试文本，用于排序验证。"]}},
"geometry":{{"measure_px":342.0,"font_size_px":19.0,"line_height":1.85,"paragraph_indent_em":2.0}},
"expect":{{"kind":"kinsoku","forbidden_at_line_start":["，"],"forbidden_at_line_end":["「"]}},
"basis":"UAX#14：CL 类不得行首。"}}]"#
        )
    };
    std::fs::write(corpus.join("b.json"), case("kinsoku:zzz")).expect("写 b");
    std::fs::write(corpus.join("a.json"), case("kinsoku:aaa")).expect("写 a");

    let (_, out1, _) = run_cli(&[
        "verify-layout",
        "--corpus",
        corpus.to_str().unwrap(),
        "--format",
        "json",
    ]);
    let (_, out2, _) = run_cli(&[
        "verify-layout",
        "--corpus",
        corpus.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(out1, out2, "两次运行应逐字节相同");

    let value: serde_json::Value = serde_json::from_str(&out1).expect("JSON");
    let names: Vec<&str> = value["outcomes"]
        .as_array()
        .expect("outcomes")
        .iter()
        .map(|o| o["name"].as_str().expect("name"))
        .collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "逐条结果应按稳定键排序（不随目录序变化）");
}

// ===========================================================================
// 3. 用法与错误
// ===========================================================================

/// **缺 `--corpus` → 用法错误（退出码 2）**。
#[test]
fn missing_corpus_is_a_usage_error() {
    let (code, _, stderr) = run_cli(&["verify-layout", "--format", "json"]);
    assert_eq!(code, 2, "缺必需选项应是用法错误，实测 {code}");
    assert!(
        stderr.contains("--corpus"),
        "错误应指出缺哪个选项：{stderr}"
    );
}

/// **语料目录不存在 → 业务错误（退出码 1）**，不静默成功。
#[test]
fn missing_corpus_directory_is_an_error() {
    let (code, _, stderr) = run_cli(&[
        "verify-layout",
        "--corpus",
        "does-not-exist-anywhere",
        "--format",
        "json",
    ]);
    assert_eq!(code, 1, "目录不存在应是业务错误，实测 {code}");
    assert!(!stderr.is_empty(), "应给出可读的错误说明（不静默）");
}

/// **空语料目录 → 退出码 1 且明确报错**（不静默通过）。
///
/// ## 实测修正：loader 把空目录当**装载错误**
///
/// 初版测试假设"空语料 → 通过率 0.0 的报告"。实测 loader 直接以
/// `error.reader.corpus.empty_directory` 拒绝——那是**更强**的保证：
/// 空目录连报告都不给，不可能被误读成"跑过了但都不达标"。本测试按其实
/// 形态断言（错误标识可见 + 退出码 1 + 不产出成功输出）。
#[test]
fn empty_corpus_directory_fails() {
    let dir = tempfile::tempdir().expect("临时目录");
    let corpus = dir.path().join("empty");
    std::fs::create_dir_all(&corpus).expect("应能建");

    let (code, stdout, stderr) = run_cli(&[
        "verify-layout",
        "--corpus",
        corpus.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 1, "空语料应是业务错误（退出码 1）");
    assert!(
        stdout.trim().is_empty(),
        "错误路径不该往 stdout 写东西（保证管道消费方拿不到伪结果）"
    );
    assert!(
        stderr.contains("empty_directory") || stderr.contains("语料"),
        "错误应指出空目录这一原因，实测：{stderr}"
    );
}

// ===========================================================================
// 4. 帮助文本与注册
// ===========================================================================

/// **子命令已注册且出现在帮助文本里**（契约 §3）。
#[test]
fn subcommand_is_registered_in_help() {
    let (code, stdout, _) = run_cli(&["--help"]);
    // --help 的退出码约定：成功（0）
    assert!(code == 0 || code == 2, "--help 应正常返回，实测 {code}");
    assert!(
        stdout.contains("verify-layout"),
        "帮助文本应登记 verify-layout 子命令（契约 §3）"
    );
}

/// **human 格式给出摘要**（契约 §1：`--format human` 为可读摘要）。
#[test]
fn human_format_is_a_summary() {
    let dir = tempfile::tempdir().expect("临时目录");
    let corpus = write_passing_corpus(dir.path());
    let (code, stdout, stderr) = run_cli(&[
        "verify-layout",
        "--corpus",
        corpus.to_str().unwrap(),
        "--format",
        "human",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(!stdout.trim().is_empty(), "human 格式应有摘要输出");
}

// ===========================================================================
// 5. 不触网（契约 §4）
// ===========================================================================

/// **不触发网络**（契约 §4：除 `--chapter <本地文件>` 外全部子命令 MUST NOT 触网）。
///
/// ## 判据怎么定
///
/// `--offline` 不是合适的判据：它有前置条件（需与 `--replay` 同用，因为
/// "完全离线"对需要发请求的命令无意义），而 `verify-layout` 压根不该走网络
/// 路径。用更直接的两条：
///
/// 1. **孤立的数据目录**仍能跑通（不依赖任何网络配置/缓存）——`run_cli`
///    已内置每次调用一个独立临时 `--data-dir`，天然满足
/// 2. **源码层面不引用网络**（`reader_commands.rs` 的 verify-layout 路径
///    不出现 `NetHost` / `reqwest` —— 结构性证据，比运行时观测更稳）
#[test]
fn does_not_touch_the_network() {
    let dir = tempfile::tempdir().expect("临时目录");
    let corpus = write_passing_corpus(dir.path());

    let (code, _, stderr) = run_cli(&[
        "verify-layout",
        "--corpus",
        corpus.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(
        code, 0,
        "verify-layout 是纯计算，用孤立数据目录也应照常工作：{stderr}"
    );

    // 结构性证据：verify-layout 的实现路径不引用网络能力
    let source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/reader_commands.rs"),
    )
    .expect("应能读 reader_commands.rs");
    let verify_fn = source
        .split("pub fn run_verify_layout")
        .nth(1)
        .and_then(|rest| rest.split("\npub fn ").next())
        .expect("应能定位 run_verify_layout 的函数体");
    for forbidden in ["NetHost", "reqwest", "net_ref", ".fetch("] {
        assert!(
            !verify_fn.contains(forbidden),
            "verify-layout 的实现里出现了 `{forbidden}`——契约 §4 要求它 MUST NOT 触网"
        );
    }
}

/// **仓库内真实语料的报告形态**（如实反映当前通过率，不作阈值断言）。
///
/// ## 这条断言什么、不断言什么
///
/// **断言**：真实语料能被 load + 求值，报告形态完整（`total` 等于语料条数、
/// 逐条结果齐备、`by_category` 分组守恒）。
///
/// **不断言**：通过率的具体数值。原因是实测发现仓库内 8 条语料里有 4 条
/// （`squeeze` × 2 / `mixed` × 1 / `kinsoku_exceptions` × 1）的期望值与
/// 实际引擎输出不符——它们是 T004 的**示例条目**，期望值在引擎落地前写就。
/// 详见本文件顶部的"一条重要的实测发现"与本任务汇报。
///
/// 若在此处写死"通过率 ≥ X"，要么会因这 4 条而红（掩盖真实情况），要么要
/// 调低到无意义的水平（让断言失去价值）。把数值留给汇报与 PM 裁决，
/// 测试只保证**报告机制本身正确**。
#[test]
fn real_corpus_report_shape_is_intact() {
    let corpus = corpus_dir();
    if !corpus.is_dir() {
        println!("[verify-layout] SKIPPED: 语料目录不在预期位置 {corpus:?}");
        return;
    }
    let corpus_str = corpus.to_str().expect("路径应可转字符串");

    let (code, stdout, stderr) =
        run_cli(&["verify-layout", "--corpus", corpus_str, "--format", "json"]);
    // 有未通过条目时报告走 stderr（退出码 1）——两种情形都要能解析出报告
    let payload = if code == 0 { stdout } else { stderr };
    let value: serde_json::Value = serde_json::from_str(&payload)
        .unwrap_or_else(|error| panic!("报告应是 JSON（退出码 {code}）：{error}\n{payload}"));

    let total = value["total"].as_u64().expect("total") as usize;
    let passed = value["passed"].as_u64().expect("passed") as usize;
    assert!(
        total > 0,
        "仓库语料非空（loader 拒绝空目录，故此处必然 > 0）"
    );
    assert!(passed <= total, "通过数不得超过总数");
    assert_eq!(
        value["outcomes"].as_array().expect("outcomes").len(),
        total,
        "逐条结果数应等于总条数"
    );

    // by_category 守恒（分组不丢条）
    let by_category = value["by_category"].as_object().expect("by_category");
    let grouped_total: u64 = by_category
        .values()
        .map(|entry| entry["total"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(
        grouped_total as usize, total,
        "按类别分组的条数之和应等于总条数"
    );

    println!("[verify-layout] 仓库语料：{passed}/{total} 通过");
}

/// **报告可写入 `--report <file>`**（契约 §2.5 的选项）。
#[test]
fn report_can_be_written_to_a_file() {
    let dir = tempfile::tempdir().expect("临时目录");
    let corpus = write_passing_corpus(dir.path());
    let report = dir.path().join("report.json");

    let (code, _, stderr) = run_cli(&[
        "verify-layout",
        "--corpus",
        corpus.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert!(report.exists(), "报告文件应被写出");
    let content = std::fs::read_to_string(&report).expect("应能读报告");
    let value: serde_json::Value = serde_json::from_str(&content).expect("报告应是 JSON");
    assert_eq!(value["total"], 1);
}
