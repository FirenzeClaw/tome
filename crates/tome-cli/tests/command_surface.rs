//! T093 的命令面端到端验收（contracts/cli.md 对照）
//!
//! # 本文件验证什么
//!
//! `flags.rs` 的单元测试只验"参数解析成了正确的 `Command`"。本文件走
//! **真实的 CLI 路径**（parse → run），验证命令**真的做了那件事**：
//! 数据落库了、删除真删了、退出码符合契约。
//!
//! # 为什么用临时数据目录而不是默认
//!
//! 默认数据目录是"可移植模式"（工作目录下的 `.tome/`），跑测试会在工作树
//! 里留下痕迹。全部测试显式传 `--data-dir` 指向临时目录——这也是契约里
//! 该 flag 存在的意义之一（让"换个地方跑"成为一次参数的事）。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败；静默跳过会让命令面缺口被掩盖"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tome_cli::commands::run;
use tome_cli::flags::{parse, Command, ParseOutcome};

/// 用临时数据目录跑一次 CLI，返回 (退出码, stdout, stderr)。
fn run_cli(args: &[&str], data_dir: &Path) -> (i32, String, String) {
    let executor =
        tome_cli::build_executor(&tome_cli::NetworkMode::Live, None).expect("应能构造执行器");
    let host: Arc<dyn tome_core::NetHost> = Arc::new(executor);
    let mut full: Vec<String> = vec!["--data-dir".to_string(), data_dir.display().to_string()];
    full.extend(args.iter().map(|arg| (*arg).to_string()));

    match parse(&full) {
        ParseOutcome::Run { options, command } => {
            let mut stdin = std::io::empty();
            let outcome = run(&options, &command, host.as_ref(), &mut stdin);
            (outcome.exit_code, outcome.stdout, outcome.stderr)
        }
        ParseOutcome::Usage(text) => (2, String::new(), text),
        ParseOutcome::Help(text) => (0, text, String::new()),
        // 与 `main::dispatch` 同形态：命令在本次构建中不存在 → 退 3，**不经装配**。
        ParseOutcome::CapabilityUnavailable {
            capability,
            command,
        } => (
            3,
            String::new(),
            format!("本构建不支持命令 `{command}`：能力 `{capability}` 在编译期缺席。"),
        ),
    }
}

// ---------------------------------------------------------------------------
// 规则求值轨迹（FR-037）
// ---------------------------------------------------------------------------

#[test]
fn run_trace_输出逐步中间值() {
    let dir = tempfile::tempdir().expect("临时目录");
    let (code, stdout, stderr) = run_cli(
        &[
            "run",
            "--trace",
            "--rule",
            "div.book a@text",
            "--input",
            r#"<div class="book"><a>书名甲</a></div>"#,
        ],
        dir.path(),
    );
    assert_eq!(code, 0, "应成功\nstdout:{stdout}\nstderr:{stderr}");

    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(report["rule"], "div.book a@text");
    assert_eq!(report["result"][0], "书名甲", "结果应与不带 --trace 一致");

    let steps = report["steps"].as_array().expect("应含 steps");
    assert!(steps.len() >= 2, "应有多步（分派 + 求值）：{steps:?}");
    // 每步都有说明、输入、产出——这是"逐步展示中间值"的字面要求
    for step in steps {
        assert!(step["what"].is_string(), "每步应有说明：{step}");
        assert!(step["input"].is_string(), "每步应有输入：{step}");
        assert!(step["output"].is_array(), "每步应有产出：{step}");
    }
}

#[test]
fn run_trace_空结果仍成功且可看出未命中() {
    // 选择器没命中是**正常情形**（调试最常遇到的），不是错误
    let dir = tempfile::tempdir().expect("临时目录");
    let (code, stdout, stderr) = run_cli(
        &[
            "run",
            "--trace",
            "--rule",
            "div.nope@text",
            "--input",
            "<p>x</p>",
        ],
        dir.path(),
    );
    assert_eq!(code, 0, "没命中不该是错误\nstderr:{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert!(report["result"]
        .as_array()
        .is_some_and(|list| list.is_empty()));
}

#[test]
fn run_trace_坏规则给业务错误码() {
    let dir = tempfile::tempdir().expect("临时目录");
    let (code, stdout, stderr) = run_cli(
        &["run", "--trace", "--rule", "div >", "--input", "<p>x</p>"],
        dir.path(),
    );
    assert_eq!(code, 1, "规则跑不通应是业务错误");

    // ## 失败时轨迹走 stderr（`contracts/cli.md`：错误输出到 stderr）
    //
    // 这不影响可用性——轨迹本来就是给调试者看的，它会与其它诊断一起
    // 出现在 stderr。成功时才走 stdout（那时 stdout 只有结构化结果）。
    let report_text = if stderr.trim_start().starts_with('{') {
        stderr
    } else {
        stdout
    };
    let report: serde_json::Value = serde_json::from_str(&report_text).expect("轨迹应是 JSON");
    let steps = report["steps"].as_array().expect("应含 steps");
    assert!(
        steps.iter().any(|step| step["failed"] == true),
        "应有失败步骤：{steps:?}"
    );
    // 失败步骤要带原因（否则调试者不知道该改什么）
    let failed = steps
        .iter()
        .find(|step| step["failed"] == true)
        .expect("应找到失败步骤");
    assert!(failed["error"].is_string(), "失败步骤应带 error：{failed}");
}

// ---------------------------------------------------------------------------
// 书源分组与启停（FR-001）
// ---------------------------------------------------------------------------

/// 造一个可导入的书源文件。
fn write_source(dir: &Path, name: &str, url: &str) -> PathBuf {
    let path = dir.join(format!("{name}.json"));
    let source = serde_json::json!({
        "bookSourceName": name,
        "bookSourceUrl": url,
        "enabled": true,
    });
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&source).expect("应能序列化"),
    )
    .expect("应能写书源");
    path
}

#[test]
fn 书源分组与启停可经_cli_操作() {
    let dir = tempfile::tempdir().expect("临时目录");
    let sources = tempfile::tempdir().expect("书源目录");
    let source = write_source(sources.path(), "分组测试源", "https://grp.example.com");

    // 导入
    let (code, stdout, stderr) = run_cli(
        &["source", "import", source.to_str().expect("路径")],
        dir.path(),
    );
    assert_eq!(code, 0, "导入应成功\nstdout:{stdout}\nstderr:{stderr}");

    // 分组
    let (code, stdout, stderr) = run_cli(
        &[
            "source",
            "group",
            "https://grp.example.com",
            "--name",
            "玄幻",
        ],
        dir.path(),
    );
    assert_eq!(code, 0, "分组应成功\nstdout:{stdout}\nstderr:{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(report["updated"], 1);
    assert_eq!(report["group"], "玄幻");

    // 分组被建出来了
    let (code, stdout, _) = run_cli(&["source", "groups"], dir.path());
    assert_eq!(code, 0);
    let groups: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert!(
        groups
            .as_array()
            .expect("应是数组")
            .iter()
            .any(|group| group["name"] == "玄幻"),
        "分组应出现在列表里：{groups}"
    );

    // 停用
    let (code, stdout, _) = run_cli(
        &["source", "disable", "https://grp.example.com"],
        dir.path(),
    );
    assert_eq!(code, 0);
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(report["enabled"], false);
    assert_eq!(report["changed"], 1);

    // `--enabled` 过滤应看不到它了
    let (_, stdout, _) = run_cli(&["source", "list", "--enabled"], dir.path());
    let listed: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert!(
        listed.as_array().is_none_or(|list| list.is_empty()),
        "停用后不该出现在 --enabled 列表：{listed}"
    );
}

#[test]
fn 分组移出用连字符() {
    let dir = tempfile::tempdir().expect("临时目录");
    let sources = tempfile::tempdir().expect("书源目录");
    let source = write_source(sources.path(), "移出测试", "https://ungroup.example.com");
    run_cli(
        &["source", "import", source.to_str().expect("路径")],
        dir.path(),
    );

    run_cli(
        &[
            "source",
            "group",
            "https://ungroup.example.com",
            "--name",
            "临时组",
        ],
        dir.path(),
    );
    let (code, stdout, _) = run_cli(
        &[
            "source",
            "group",
            "https://ungroup.example.com",
            "--name",
            "-",
        ],
        dir.path(),
    );
    assert_eq!(code, 0, "移出分组应成功");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert!(report["group"].is_null(), "移出后分组应为 null：{report}");
}

#[test]
fn 分组时源不存在报错而非静默成功() {
    let dir = tempfile::tempdir().expect("临时目录");
    let (code, _, stderr) = run_cli(
        &[
            "source",
            "group",
            "https://never.example.com",
            "--name",
            "组",
        ],
        dir.path(),
    );
    assert_ne!(code, 0, "源不存在不该报成功");
    assert!(
        stderr.contains("missing") || stderr.contains("不存在"),
        "应指明源不存在：{stderr}"
    );
}

#[test]
fn all_与显式_id_同时给是用法错误() {
    let dir = tempfile::tempdir().expect("临时目录");
    let (code, _, stderr) = run_cli(&["source", "enable", "https://a.com", "--all"], dir.path());
    assert_eq!(code, 2, "应是用例错误而非业务错误");
    assert!(stderr.contains("--all"), "{stderr}");
}

// ---------------------------------------------------------------------------
// 搜索关键词（FR-018）
// ---------------------------------------------------------------------------

#[test]
fn 关键词可收藏与复用() {
    let dir = tempfile::tempdir().expect("临时目录");

    // 收藏
    let (code, stdout, stderr) = run_cli(&["keyword", "add", "斗破苍穹"], dir.path());
    assert_eq!(code, 0, "收藏应成功\nstdout:{stdout}\nstderr:{stderr}");

    // 列出
    let (code, stdout, _) = run_cli(&["keyword", "list"], dir.path());
    assert_eq!(code, 0);
    let keywords: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    let list = keywords.as_array().expect("应是数组");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["keyword"], "斗破苍穹");

    // 重复收藏不报错（是"再用一次"而非错误）
    let (code, _, stderr) = run_cli(&["keyword", "add", "斗破苍穹"], dir.path());
    assert_eq!(code, 0, "重复收藏不该报错\nstderr:{stderr}");

    // 删除
    let (code, _, _) = run_cli(&["keyword", "remove", "斗破苍穹"], dir.path());
    assert_eq!(code, 0);
    let (_, stdout, _) = run_cli(&["keyword", "list"], dir.path());
    let keywords: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert!(keywords.as_array().is_none_or(|list| list.is_empty()));
}

// ---------------------------------------------------------------------------
// 数据面：导出 / 导入 / 清缓存 / 删除（FR-033 / SC-017 / FR-035）
// ---------------------------------------------------------------------------

#[test]
fn 数据导出导入往返() {
    let dir = tempfile::tempdir().expect("临时目录");
    let sources = tempfile::tempdir().expect("书源目录");
    let source = write_source(sources.path(), "往返源", "https://rt.example.com");
    run_cli(
        &["source", "import", source.to_str().expect("路径")],
        dir.path(),
    );
    run_cli(&["keyword", "add", "关键词甲"], dir.path());

    // 导出到 stdout：**必须只有 JSON**（要能被重定向到文件）
    let (code, stdout, stderr) = run_cli(&["data", "export"], dir.path());
    assert_eq!(code, 0, "导出应成功\nstderr:{stderr}");
    let backup: serde_json::Value = serde_json::from_str(&stdout)
        .expect("导出到 stdout 时必须是纯 JSON（不加提示，否则污染管道）");
    assert_eq!(backup["version"], 1);

    // 导出到文件：此时可以有统计提示
    let out = dir.path().join("backup.json");
    let (code, stdout, _) = run_cli(
        &["data", "export", "--output", out.to_str().expect("路径")],
        dir.path(),
    );
    assert_eq!(code, 0);
    assert!(out.is_file(), "应写出文件");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(report["sources"], 1, "统计应含源数：{report}");

    // 清空后导入
    let (code, _, _) = run_cli(&["data", "delete", "--yes"], dir.path());
    assert_eq!(code, 0, "删除应成功");

    let (code, stdout, stderr) =
        run_cli(&["data", "import", out.to_str().expect("路径")], dir.path());
    assert_eq!(code, 0, "导入应成功\nstdout:{stdout}\nstderr:{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(report["sources"], 1);

    // 导入后源真的在库里
    let (_, stdout, _) = run_cli(&["source", "list"], dir.path());
    let listed: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert!(
        listed
            .as_array()
            .expect("应是数组")
            .iter()
            .any(|source| source["id"] == "https://rt.example.com"),
        "导入的源应可列出：{listed}"
    );
}

#[test]
fn 清缓存不影响个人数据() {
    // SC-017 / FR-031 的核心不变量
    let dir = tempfile::tempdir().expect("临时目录");
    let sources = tempfile::tempdir().expect("书源目录");
    let source = write_source(sources.path(), "保留源", "https://keep.example.com");
    run_cli(
        &["source", "import", source.to_str().expect("路径")],
        dir.path(),
    );

    for kind in ["chapters", "images"] {
        let (code, stdout, stderr) = run_cli(&["data", "clear-cache", kind], dir.path());
        assert_eq!(
            code, 0,
            "清 {kind} 缓存应成功\nstdout:{stdout}\nstderr:{stderr}"
        );
        let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
        assert_eq!(report["cleared"], kind);
    }

    // 个人数据（书源）必须还在
    let (_, stdout, _) = run_cli(&["source", "list"], dir.path());
    let listed: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(
        listed.as_array().expect("应是数组").len(),
        1,
        "清缓存不该动书源（FR-031）"
    );
}

#[test]
fn 数据删除无_yes_时拒绝执行() {
    let dir = tempfile::tempdir().expect("临时目录");
    let sources = tempfile::tempdir().expect("书源目录");
    let source = write_source(sources.path(), "待删源", "https://del.example.com");
    run_cli(
        &["source", "import", source.to_str().expect("路径")],
        dir.path(),
    );

    // 不给 --yes：应拒绝，且**不执行**
    let (code, _, stderr) = run_cli(&["data", "delete"], dir.path());
    assert_eq!(code, 2, "应是用法错误（区分'没执行'与'执行失败'）");
    assert!(stderr.contains("--yes"), "应提示确认方式：{stderr}");

    // 数据还在（证明"没执行"）
    let (_, stdout, _) = run_cli(&["source", "list"], dir.path());
    let listed: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(
        listed.as_array().expect("应是数组").len(),
        1,
        "拒绝执行时数据不该被动过"
    );

    // 给 --yes：执行，数据全清
    let (code, stdout, stderr) = run_cli(&["data", "delete", "--yes"], dir.path());
    assert_eq!(code, 0, "确认后应执行\nstdout:{stdout}\nstderr:{stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(report["remaining_rows"], 0, "删完不该有残留行：{report}");

    let (_, stdout, _) = run_cli(&["source", "list"], dir.path());
    let listed: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert!(
        listed.as_array().is_none_or(|list| list.is_empty()),
        "删除后不该还有书源"
    );
}

// ---------------------------------------------------------------------------
// 全局 flag（contracts/cli.md 全局约定）
// ---------------------------------------------------------------------------

#[test]
fn config_进_options_且被装配读取() {
    // `--config` 不能是个"传了但没效果"的 flag
    let dir = tempfile::tempdir().expect("临时目录");
    let config = dir.path().join("c.toml");
    std::fs::write(&config, "portable = true\n").expect("应能写");

    let args: Vec<String> = vec![
        "--data-dir".into(),
        dir.path().display().to_string(),
        "--config".into(),
        config.display().to_string(),
        "keyword".into(),
        "list".into(),
    ];
    let (options, command) = match parse(&args) {
        ParseOutcome::Run { options, command } => (options, command),
        other => panic!("应解析成功，实际 {other:?}"),
    };
    assert_eq!(
        options.config,
        Some(config.clone()),
        "--config 应进 options"
    );
    assert!(matches!(command, Command::KeywordList));

    // 非法 TOML 应被**装配路径**报出配置错（证明它真被读）
    let bad = dir.path().join("bad.toml");
    std::fs::write(&bad, "这不是 TOML @@@").expect("应能写");
    let root = tome_cli::root::AppRoot::assemble(&options, Some(&bad));
    assert!(
        root.is_err(),
        "非法配置应让装配失败（否则 --config 是死参数）"
    );
}

#[test]
fn lang_切换进_options() {
    let dir = tempfile::tempdir().expect("临时目录");
    let args: Vec<String> = vec![
        "--data-dir".into(),
        dir.path().display().to_string(),
        "--lang".into(),
        "en".into(),
        "keyword".into(),
        "list".into(),
    ];
    let options = match parse(&args) {
        ParseOutcome::Run { options, .. } => options,
        other => panic!("应解析成功，实际 {other:?}"),
    };
    assert_eq!(options.language, tome_cli::i18n::Language::En);
}

#[test]
fn 帮助文本列出全部命令面() {
    // 帮助是用户发现命令的入口——漏列等于该命令不可发现
    let help = tome_cli::flags::help_text();
    for expected in [
        "source import",
        "source list",
        "source check",
        "source group",
        "source enable",
        "source groups",
        "search",
        "info",
        "toc",
        "content",
        "run",
        "keyword",
        "data export",
        "data import",
        "data clear-cache",
        "data delete",
        "verify",
        "local",
        "--config",
        "--lang",
        "--trace",
        "--data-dir",
    ] {
        assert!(help.contains(expected), "帮助应列出 `{expected}`");
    }
}

#[test]
fn 未给数据目录时解析成功() {
    // 契约：`--data-dir` 覆盖（默认可移植模式）。故不给时不该报错。
    let args: Vec<String> = vec!["keyword".into(), "list".into()];
    let (options, command) = match parse(&args) {
        ParseOutcome::Run { options, command } => (options, command),
        other => panic!("应解析成功，实际 {other:?}"),
    };
    assert!(options.data_dir.is_none(), "未给时不该有值");
    assert!(matches!(command, Command::KeywordList));

    // 真跑一次（显式给临时目录，避免在工作树留痕）
    let dir = tempfile::tempdir().expect("临时目录");
    let (code, _, _) = run_cli(&["keyword", "list"], dir.path());
    assert_eq!(code, 0);
}
