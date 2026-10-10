//! `verify --real-sources` 的端到端验收（tasks.md T086 / SC-002b）
//!
//! # 为什么用合成站点而不是真站点
//!
//! 这条命令的**契约**（基线比对、回归判定、退出码）与"站点是不是真的"无关，
//! 而合成站点带来两件真站点给不了的东西：
//!
//! 1. **可判定**——真站点的内容每天都在变，无法断言"应当回退"；
//!    合成站点可以精确构造"结果变少"这一回退场景。
//! 2. **可复现**——CI 里不发外网请求（也符合 FR-041 的取向）。
//!
//! 真站点的对照由 `tools/ref-runner/` 在受控环境跑，产物入 `testdata/captures/`
//! （不入仓库）。本文件管的是**门本身对不对**。
//!
//! # 覆盖的三件事
//!
//! | 测试 | 对应验收 |
//! |---|---|
//! | 首轮建基线 | 「首轮基线落盘」 |
//! | 有回退时退出码 1 | 「回归不得回退」（门真的会拦） |
//! | 无回退时退出码 0 | 门不误报 |
//! | 缺基线时告知而非静默通过 | 基线缺失必须可见 |

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败；静默产出空对照数据更糟"
)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tome_cli::commands::run;
use tome_cli::flags::{parse, Command, GlobalOptions, ParseOutcome};
use tome_cli::verify_real::Fingerprint;

// ---------------------------------------------------------------------------
// 可变结果的合成站点（用于构造"回退"场景）
// ---------------------------------------------------------------------------

/// 合成站点：书目条数由计数器控制，便于构造"本次结果变少"。
struct ShrinkingSite {
    address: String,
    /// 每次搜索返回的书目条数（原子，可在测试中改小）
    book_count: Arc<AtomicUsize>,
}

impl ShrinkingSite {
    fn start(initial_count: usize) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("应能绑定回环端口");
        let address = listener.local_addr().expect("应能取地址").to_string();
        let book_count = Arc::new(AtomicUsize::new(initial_count));

        let counter = Arc::clone(&book_count);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let count = counter.load(Ordering::SeqCst);
                let _ = serve_search(stream, count);
            }
        });

        Self {
            address,
            book_count,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// 让站点"内容变少"——模拟抓取质量回退。
    fn shrink_to(&self, count: usize) {
        self.book_count.store(count, Ordering::SeqCst);
    }
}

fn serve_search(mut stream: TcpStream, count: usize) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }

    let mut items = String::new();
    for index in 0..count {
        items.push_str(&format!(
            r#"<li class="book"><span class="title"><a href="/book/{index}">合成书{index}</a></span><span class="author">作者{index}</span></li>"#
        ));
    }
    let body = format!("<html><body><ul>{items}</ul></body></html>");

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 造一份真实书源目录（内含指向合成站点的书源）。
fn make_sources_dir(base: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("应能建临时目录");
    let source = serde_json::json!({
        "bookSourceName": "T086 合成源",
        "bookSourceUrl": base,
        "searchUrl": "/search?q={{key}}",
        "ruleSearch": "{\"bookList\":\"@css:li.book\",\"name\":\"@css:.title@text\",\"author\":\"@css:.author@text\",\"bookUrl\":\"@css:.title>a@href\"}",
        "enabled": true
    });
    std::fs::write(
        dir.path().join("synthetic.json"),
        serde_json::to_string_pretty(&source).expect("应能序列化"),
    )
    .expect("应能写书源");
    let path = dir.path().to_path_buf();
    (dir, path)
}

/// 跑一次 `verify --real-sources`，返回 (退出码, stdout, stderr)。
fn run_verify_real(
    sources: &Path,
    golden: &Path,
    write_baseline: bool,
    data_dir: &Path,
) -> (i32, String, String) {
    // 先把书源导入注册表（`verify --real-sources` 按 id 载源）
    let mut argv: Vec<String> = vec![
        "source".into(),
        "import".into(),
        sources.join("synthetic.json").to_string_lossy().to_string(),
        "--data-dir".into(),
        data_dir.to_string_lossy().to_string(),
    ];
    let executor =
        tome_cli::build_executor(&tome_cli::NetworkMode::Live, None).expect("应能构造执行器");
    let host: Arc<dyn tome_core::NetHost> = Arc::new(executor);

    match parse(&argv) {
        ParseOutcome::Run { command, options } => {
            let mut stdin = std::io::empty();
            let outcome = run(&options, &command, host.as_ref(), &mut stdin);
            assert_eq!(outcome.exit_code, 0, "导入应成功：{}", outcome.stderr);
        }
        other => panic!("导入参数应解析成功，实际 {other:?}"),
    }

    // 再跑真实源对照
    argv = vec![
        "verify".into(),
        "--real-sources".into(),
        sources.to_string_lossy().to_string(),
        "--golden".into(),
        golden.to_string_lossy().to_string(),
        "--data-dir".into(),
        data_dir.to_string_lossy().to_string(),
    ];
    if write_baseline {
        argv.push("--write-baseline".into());
    }

    let options = GlobalOptions {
        data_dir: Some(data_dir.to_path_buf()),
        ..GlobalOptions::default()
    };
    let command = match parse(&argv) {
        ParseOutcome::Run { command, .. } => command,
        other => panic!("verify 参数应解析成功，实际 {other:?}"),
    };
    assert!(matches!(command, Command::VerifyReal { .. }));

    let mut stdin = std::io::empty();
    let outcome = run(&options, &command, host.as_ref(), &mut stdin);
    (outcome.exit_code, outcome.stdout, outcome.stderr)
}

// ---------------------------------------------------------------------------
// 参数解析
// ---------------------------------------------------------------------------

#[test]
fn real_sources_参数被正确解析() {
    let argv: Vec<String> = vec![
        "verify".into(),
        "--real-sources".into(),
        "testdata/captures/sources".into(),
        "--golden".into(),
        "testdata/baselines".into(),
        "--write-baseline".into(),
    ];
    match parse(&argv) {
        ParseOutcome::Run { command, .. } => match command {
            Command::VerifyReal {
                real_sources,
                golden,
                write_baseline,
                ..
            } => {
                assert_eq!(real_sources, PathBuf::from("testdata/captures/sources"));
                assert_eq!(golden, PathBuf::from("testdata/baselines"));
                assert!(write_baseline, "--write-baseline 应被识别");
            }
            other => panic!("应为 VerifyReal，实际 {other:?}"),
        },
        other => panic!("应解析成功，实际 {other:?}"),
    }
}

#[test]
fn real_sources_缺_golden_是用法错误() {
    let argv: Vec<String> = vec!["verify".into(), "--real-sources".into(), "somewhere".into()];
    match parse(&argv) {
        ParseOutcome::Usage(text) => assert!(text.contains("--golden"), "{text}"),
        other => panic!("应为用法错误，实际 {other:?}"),
    }
}

#[test]
fn corpus_与_real_sources_互斥() {
    let argv: Vec<String> = vec![
        "verify".into(),
        "--corpus".into(),
        "tests/corpus".into(),
        "--real-sources".into(),
        "somewhere".into(),
        "--golden".into(),
        "somewhere-else".into(),
    ];
    match parse(&argv) {
        ParseOutcome::Usage(text) => assert!(text.contains("互斥"), "{text}"),
        other => panic!("同时给两个档应是用例错误，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 端到端：首轮基线 → 无回退 → 有回退
// ---------------------------------------------------------------------------

#[test]
fn 首轮建基线后可检出回退() {
    let site = ShrinkingSite::start(5);
    let (_sources_guard, sources) = make_sources_dir(&site.base_url());
    let golden_guard = tempfile::tempdir().expect("应能建临时基线目录");
    let golden = golden_guard.path().to_path_buf();
    let data_guard = tempfile::tempdir().expect("应能建临时数据目录");
    let data_dir = data_guard.path().to_path_buf();

    // --- 首轮：建基线 -------------------------------------------------------
    let (code, stdout, stderr) = run_verify_real(&sources, &golden, true, &data_dir);
    assert_eq!(
        code, 0,
        "首轮建基线应成功\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let baseline_file = golden.join("fingerprint.json");
    assert!(
        baseline_file.is_file(),
        "基线文件应落盘：{}",
        baseline_file.display()
    );

    let baseline_text = std::fs::read_to_string(&baseline_file).expect("应能读基线");
    let baseline: Vec<Fingerprint> =
        serde_json::from_str(&baseline_text).expect("基线应是合法指纹数组");
    assert_eq!(baseline.len(), 1, "一个源一个动作应有一条指纹");
    let entry = &baseline[0];
    assert!(entry.ok, "首轮抓取应成功");
    // 5 本书 × (书名 + 作者) = 10 条文本
    assert_eq!(
        entry.entries, 10,
        "5 本书的书名+作者共 10 条，实际 {entry:?}"
    );
    assert!(entry.total_chars > 0, "总字符数应大于 0");

    // --- 第二轮：内容不变，不该回退 -----------------------------------------
    let (code, stdout, stderr) = run_verify_real(&sources, &golden, false, &data_dir);
    assert_eq!(
        code, 0,
        "内容未变不该判回退\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // --- 第三轮：站点"内容变少" → 必须检出回退 ------------------------------
    site.shrink_to(2);
    let (code, stdout, stderr) = run_verify_real(&sources, &golden, false, &data_dir);
    assert_eq!(
        code, 1,
        "结果变少应判回退（业务错误）\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report_text = if stderr.contains("\"regressed\"") {
        stderr.clone()
    } else {
        stdout.clone()
    };
    let report: serde_json::Value = serde_json::from_str(&report_text).expect("报告应是合法 JSON");
    assert_eq!(
        report["regressed"].as_u64(),
        Some(1),
        "应报告 1 条回退：{report}"
    );
    let regressions = report["regressions"]
        .as_array()
        .expect("应含 regressions 数组");
    assert!(!regressions.is_empty(), "应逐条列出回退");

    // 回退条目要能看出"从多少变成多少"，否则报告无法据以排查
    let first = &regressions[0];
    assert_eq!(first["baseline"]["entries"].as_u64(), Some(10));
    assert_eq!(
        first["current"]["entries"].as_u64(),
        Some(4),
        "2 本书 = 4 条"
    );
}

#[test]
fn 缺基线时告知而非静默通过() {
    let site = ShrinkingSite::start(3);
    let (_sources_guard, sources) = make_sources_dir(&site.base_url());
    let golden_guard = tempfile::tempdir().expect("应能建临时基线目录");
    let golden = golden_guard.path().to_path_buf();
    let data_guard = tempfile::tempdir().expect("应能建临时数据目录");
    let data_dir = data_guard.path().to_path_buf();

    // 不写基线，故这是首轮
    let (code, stdout, stderr) = run_verify_real(&sources, &golden, false, &data_dir);
    assert_eq!(
        code, 0,
        "首轮不该算失败\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("首轮"),
        "应明确告知这是首轮、尚无基线可比（静默通过会让门形同虚设）：{combined}"
    );
    assert!(
        combined.contains("--write-baseline"),
        "应给出落盘基线的可执行指引：{combined}"
    );
    assert!(
        !golden.join("fingerprint.json").is_file(),
        "未加 --write-baseline 时不该写基线文件"
    );
}

#[test]
fn 目录不存在时报错而非静默成功() {
    let golden_guard = tempfile::tempdir().expect("应能建临时基线目录");
    let data_guard = tempfile::tempdir().expect("应能建临时数据目录");

    let argv: Vec<String> = vec![
        "verify".into(),
        "--real-sources".into(),
        "definitely/not/a/directory".into(),
        "--golden".into(),
        golden_guard.path().to_string_lossy().to_string(),
    ];
    let options = GlobalOptions {
        data_dir: Some(data_guard.path().to_path_buf()),
        ..GlobalOptions::default()
    };
    let command = match parse(&argv) {
        ParseOutcome::Run { command, .. } => command,
        other => panic!("应解析成功，实际 {other:?}"),
    };

    let executor =
        tome_cli::build_executor(&tome_cli::NetworkMode::Live, None).expect("应能构造执行器");
    let host: Arc<dyn tome_core::NetHost> = Arc::new(executor);
    let mut stdin = std::io::empty();
    let outcome = run(&options, &command, host.as_ref(), &mut stdin);

    assert_ne!(outcome.exit_code, 0, "目录不存在不该报成功");
    assert!(
        outcome.stderr.contains("不存在") || outcome.stderr.contains("不是目录"),
        "错误应说明原因：{}",
        outcome.stderr
    );
}

// ---------------------------------------------------------------------------
// 已提交的基线（testdata/baselines/fingerprint.json）
// ---------------------------------------------------------------------------

/// 仓库根（`crates/tome-cli/tests/` 上溯三级）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate 目录应有两级父目录")
        .to_path_buf()
}

/// 基线文件必须存在、可解析、且**只含指纹不含第三方内容**。
///
/// # 这条测试守的是 T086 的数据边界
///
/// 基线入 git 的前提是它不含第三方站点内容。若有人把正文/书名塞进
/// 指纹结构，这条测试会红——否则"第三方资产不入仓库"这条纪律
/// 会因一个字段的添加而静默失效。
#[test]
fn 已提交的基线只含指纹() {
    let baseline_file = repo_root()
        .join("testdata")
        .join("baselines")
        .join("fingerprint.json");
    assert!(
        baseline_file.is_file(),
        "首轮基线应已提交：{}（用 capture_real_baseline 产出）",
        baseline_file.display()
    );

    let text = std::fs::read_to_string(&baseline_file).expect("应能读基线");
    let entries: Vec<Fingerprint> = serde_json::from_str(&text).expect("基线应是合法指纹数组");
    assert!(!entries.is_empty(), "基线不该为空（空基线让回归门恒绿）");

    for entry in &entries {
        assert!(!entry.case.trim().is_empty(), "指纹应有 case 标识");
        assert!(
            !entry.ok || entry.entries > 0,
            "成功的指纹应至少有 1 条结果：{entry:?}"
        );
        assert!(
            entry.digest.len() <= 64,
            "散列应是短指纹而非内容本体（长度 {}）",
            entry.digest.len()
        );
    }

    // 结构性检查：指纹结构里**没有**存正文的字段。
    // 用 JSON 层断言而不是结构体层，因为"将来加了个 text 字段"正是要防的事。
    let raw: serde_json::Value = serde_json::from_str(&text).expect("应是合法 JSON");
    // `label` 是源名（人可读标签）。它**可能**含第三方内容吗？源名是
    // 书源作者起的名字（如"某某小说"），不含站点正文——与 `case` 同类。
    // 仍保留白名单机制：新增字段时必须显式确认"它不承载第三方内容"。
    let allowed = ["case", "label", "ok", "entries", "total_chars", "digest"];
    for item in raw.as_array().expect("应是数组") {
        let object = item.as_object().expect("每项应是对象");
        for key in object.keys() {
            assert!(
                allowed.contains(&key.as_str()),
                "指纹里出现了未预期字段 `{key}`——若它可能承载第三方内容，\
                 基线就不能入仓库（见 verify_real.rs 的模块文档）"
            );
        }
    }
}

/// **回归门必须真的会拦**：把基线人为抬高，现场结果就必须被判为回退。
///
/// # 为什么这条测试重要
///
/// 只测"门会通过"是不知道门装没装的。SC-002b 的核心断言是
/// "后续迭代不得回退"，故必须证明：当结果确实比基线差时，门会红。
///
/// 做法：读已提交的基线，把 `entries` 抬到一个不可达的高值，写进临时
/// 基线目录，再跑同一条命令——它必须退出码 1 并逐条报出回退。
///
/// # 为什么要改**书源名**而不是只改基线
///
/// 基线的键是 `源名/动作`（见 `Fingerprint::case`）。故这条测试必须让
/// 现场源的名字与抬高后的基线键一致，否则两边永远匹配不上、
/// 比对退化成"基线里没有这条"而不是"结果比基线差"——那测的是别的东西。
#[test]
fn 回归门在基线被抬高时变红() {
    let root = repo_root();
    let committed = root
        .join("testdata")
        .join("baselines")
        .join("fingerprint.json");
    if !committed.is_file() {
        println!("基线尚未产出（{}），跳过", committed.display());
        return;
    }

    let site = ShrinkingSite::start(3);
    let (_sources_guard, sources) = make_sources_dir(&site.base_url());
    let data_guard = tempfile::tempdir().expect("应能建临时数据目录");

    // 读已提交的基线，认清它的**真实键**。
    //
    // 键是 `bookSourceUrl`（PM 裁决后的格式），故现场源必须用**同一个
    // bookSourceUrl** —— 这正是"同一条用例"的构造。源名不参与键，
    // 故改名不该影响这条测试（那正是改键的目的）。
    let text = std::fs::read_to_string(&committed).expect("应能读基线");
    let mut entries: Vec<Fingerprint> = serde_json::from_str(&text).expect("应是合法指纹数组");
    let committed_case = entries
        .first()
        .map(|entry| entry.case.clone())
        .expect("基线不该为空");

    // 键形如 `http://host:port/search` —— 含 `://`，故不能用 split('/') 取首段。
    // 从**右侧**切掉 `/动作` 后缀即可（动作名不含 `/`）。
    let committed_url = committed_case
        .rsplit_once('/')
        .map(|(url, _action)| url.to_string())
        .expect("case 形如 bookSourceUrl/动作");
    assert!(
        committed_url.contains("://"),
        "键应是 bookSourceUrl（含协议前缀），实际 {committed_url}"
    );

    let source_file = sources.join("synthetic.json");
    let mut source_value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&source_file).expect("应能读源"))
            .expect("应是合法 JSON");
    source_value["bookSourceUrl"] = serde_json::Value::String(committed_url.clone());
    // 顺带把源名改成另一个名字：验证**改名不影响键匹配**
    source_value["bookSourceName"] = serde_json::Value::String("改名后的源".to_string());
    std::fs::write(
        &source_file,
        serde_json::to_string_pretty(&source_value).expect("应能序列化"),
    )
    .expect("应能写源");

    // 抬高基线：现场任何真实结果都会比它少
    for entry in &mut entries {
        entry.entries = entry.entries.saturating_add(1000);
    }
    let inflated_dir = tempfile::tempdir().expect("应能建临时目录");
    std::fs::write(
        inflated_dir.path().join("fingerprint.json"),
        serde_json::to_string_pretty(&entries).expect("应能序列化"),
    )
    .expect("应能写抬高后的基线");

    let (code, stdout, stderr) =
        run_verify_real(&sources, inflated_dir.path(), false, data_guard.path());
    assert_eq!(
        code, 1,
        "结果比基线少时门必须变红\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let report_text = if stderr.contains("\"regressed\"") {
        stderr
    } else {
        stdout
    };
    let report: serde_json::Value = serde_json::from_str(&report_text).expect("报告应是 JSON");
    assert!(
        report["regressed"].as_u64().unwrap_or(0) > 0,
        "报告应指明回退条数：{report}"
    );

    // 键必须真的对上了——否则上面那条断言可能来自"基线里没这条"的巧合
    let cases = report["cases"].as_array().expect("应含 cases");
    assert!(
        cases.iter().any(|case| case["baseline"].is_object()),
        "现场用例应与抬高后的基线键匹配上（键是 `源名/动作`）：{report}"
    );
}

// ---------------------------------------------------------------------------
// 基线键：bookSourceUrl 而非源名（2026-10-10 PM 裁决）
// ---------------------------------------------------------------------------

/// **源改名不影响回归保护** —— 这正是改键要解决的问题。
///
/// wave1 用源名作键：书源改名后基线配不上，那条源静默失去回归保护
/// （不报错、不报警，只是不再比对）。改键后，改名只是换了 `label`，
/// 比对照常进行，回退照样能被检出。
#[test]
fn 源改名后回归保护仍生效() {
    let site = ShrinkingSite::start(5);
    let (_sources_guard, sources) = make_sources_dir(&site.base_url());
    let golden_guard = tempfile::tempdir().expect("应能建临时基线目录");
    let golden = golden_guard.path().to_path_buf();
    let data_guard = tempfile::tempdir().expect("应能建临时数据目录");
    let data_dir = data_guard.path().to_path_buf();

    // 首轮：建基线
    let (code, stdout, stderr) = run_verify_real(&sources, &golden, true, &data_dir);
    assert_eq!(code, 0, "建基线应成功\nstdout:{stdout}\nstderr:{stderr}");

    let baseline: Vec<Fingerprint> = serde_json::from_str(
        &std::fs::read_to_string(golden.join("fingerprint.json")).expect("应能读基线"),
    )
    .expect("应是合法指纹数组");
    assert!(
        baseline[0].case.contains("://"),
        "键应是 bookSourceUrl，实际 {}",
        baseline[0].case
    );

    // 改源名（不动 bookSourceUrl）
    let source_file = sources.join("synthetic.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&source_file).expect("应能读源"))
            .expect("应是合法 JSON");
    value["bookSourceName"] = serde_json::Value::String("完全换了个名字".to_string());
    std::fs::write(
        &source_file,
        serde_json::to_string_pretty(&value).expect("应能序列化"),
    )
    .expect("应能写源");

    // 内容不变 + 改名 → 仍不该判回退
    let (code, stdout, stderr) = run_verify_real(&sources, &golden, false, &data_dir);
    assert_eq!(
        code, 0,
        "改名不该影响比对结果\nstdout:{stdout}\nstderr:{stderr}"
    );

    // 现在让内容真的变少：必须仍能检出回退（保护没有因改名而失效）
    site.shrink_to(2);
    let (code, stdout, stderr) = run_verify_real(&sources, &golden, false, &data_dir);
    assert_eq!(
        code, 1,
        "改名之后回退仍必须被检出（这正是改键的目的）\nstdout:{stdout}\nstderr:{stderr}"
    );
    let report_text = if stderr.contains("\"regressed\"") {
        stderr
    } else {
        stdout
    };
    let report: serde_json::Value = serde_json::from_str(&report_text).expect("应是 JSON");
    assert!(
        report["regressed"].as_u64().unwrap_or(0) > 0,
        "应报告回退：{report}"
    );
    // 报告里应能看到改后的标签（键是 URL，标签可读）
    let cases = report["cases"].as_array().expect("应含 cases");
    assert!(
        cases[0]["baseline"].is_object(),
        "键应能匹配上（改名不影响）"
    );
}

/// 报告同时给出键（URL）与标签（源名）。
#[test]
fn 报告同时给出键与标签() {
    let baseline_file = repo_root()
        .join("testdata")
        .join("baselines")
        .join("fingerprint.json");
    let entries: Vec<Fingerprint> =
        serde_json::from_str(&std::fs::read_to_string(&baseline_file).expect("应能读基线"))
            .expect("应是合法指纹数组");

    let entry = entries.first().expect("基线不该为空");
    assert!(entry.case.contains("://"), "键应是 bookSourceUrl");
    assert!(!entry.label.is_empty(), "应带可读标签（源名），供人读报告");
}

/// 旧格式（源名作键）的基线被**明示**而非静默失效。
#[test]
fn 旧格式基线被明示而非静默失效() {
    let site = ShrinkingSite::start(3);
    let (_sources_guard, sources) = make_sources_dir(&site.base_url());
    let golden_guard = tempfile::tempdir().expect("应能建临时基线目录");
    let golden = golden_guard.path().to_path_buf();
    let data_guard = tempfile::tempdir().expect("应能建临时数据目录");

    // 伪造一份 wave1 格式的基线（键是源名，不含 `://`）
    let stale = vec![Fingerprint {
        case: "参考运行器合成样本源/search".to_string(),
        label: String::new(),
        ok: true,
        entries: 6,
        total_chars: 21,
        digest: "917bb842292490c7".to_string(),
    }];
    std::fs::write(
        golden.join("fingerprint.json"),
        serde_json::to_string_pretty(&stale).expect("应能序列化"),
    )
    .expect("应能写");

    let (code, stdout, stderr) = run_verify_real(&sources, &golden, false, data_guard.path());
    let report_text = format!("{stdout}{stderr}");

    assert_eq!(code, 0, "旧基线不算失败（它只是配不上）\n{report_text}");
    assert!(
        report_text.contains("旧格式") && report_text.contains("--write-baseline"),
        "应明示旧格式并给出可执行指引（静默失效会让用户以为门还在）\n{report_text}"
    );
}

/// wave1 的基线文件（无 `label` 字段）仍**读得进来**，只是键配不上。
#[test]
fn 旧基线文件无_label_字段仍可读() {
    let json = r#"[
        {
            "case": "旧源名/search",
            "ok": true,
            "entries": 6,
            "total_chars": 21,
            "digest": "917bb842292490c7"
        }
    ]"#;
    let entries: Vec<Fingerprint> =
        serde_json::from_str(json).expect("缺 label 不该让读取失败（serde default）");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].label, "", "缺字段时应为空串");
    assert_eq!(entries[0].entries, 6, "其余字段照常读到");
}
