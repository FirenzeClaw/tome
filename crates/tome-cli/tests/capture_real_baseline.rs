//! 生成 `verify --real-sources` 的首轮基线（T086 的落盘动作）。
//!
//! # 它为什么存在
//!
//! T086 要求「首轮基线落盘」。基线是**要入 git** 的东西（只含特征指纹，
//! 不含第三方内容），故它必须由一个**可复跑**的步骤产出，而不是手工敲出来。
//! 本测试就是那个步骤：
//!
//! ```bash
//! cargo test -p tome-cli --test capture_real_baseline -- --ignored --nocapture
//! ```
//!
//! 它用 `testdata/captures/site/` 的合成站点起一个回环服务器，把
//! `testdata/captures/sources/ref-runner-sample.json` 指向它，然后经
//! **真实的 CLI 路径**跑一遍 `verify --real-sources --write-baseline`，
//! 把指纹写到 `testdata/baselines/fingerprint.json`。
//!
//! # 为什么基线入 git 而来样本不入
//!
//! | 内容 | 位置 | 入 git | 理由 |
//! |---|---|---|---|
//! | 指纹（条数/长度/散列） | `testdata/baselines/` | **是** | 不含第三方内容；且必须随代码走才能做回归 |
//! | 抓取产物（正文/书名） | `testdata/captures/` | 否 | 第三方资产 |
//!
//! 标 `#[ignore]`：它写仓库路径，默认测试运行不该改工作树。

#![expect(
    clippy::expect_used,
    reason = "基线生成失败应让测试立即失败；静默产出空基线会让回归门永远绿"
)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;

use tome_cli::flags::{parse, GlobalOptions, ParseOutcome};

/// 回环合成站点（复用 `capture_synthetic.rs` 的页面文件）。
/// 合成站点的固定地址（见 [`Site::start`] 对"为什么固定"的说明）。
const SITE_ADDRESS: &str = "127.0.0.1:38921";

struct Site {
    address: String,
}

impl Site {
    /// 启动站点。
    ///
    /// # 为什么要绑**固定端口**而不是 `:0`（让 OS 随便挑一个）
    ///
    /// 基线要入 git，而它的键是 `bookSourceUrl`——用 `:0` 时每次运行拿到
    /// 不同端口，键每次都变，**基线永远配不上**（回归门形同虚设）。
    /// 固定端口让 `http://127.0.0.1:38921` 成为稳定标识。
    ///
    /// 选一个不常用的高端口，降低"被别的进程占住"的概率（占了会直接
    /// 报错而不是静默用别的端口——那正是我们要的：宁可见红不可静默失效）。
    fn start(site_dir: PathBuf) -> Self {
        let listener = TcpListener::bind(SITE_ADDRESS).expect("应能绑定回环端口");
        let address = listener.local_addr().expect("应能取地址").to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let _ = serve(stream, &site_dir);
            }
        });
        Self { address }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }
}

fn serve(mut stream: TcpStream, site_dir: &std::path::Path) -> std::io::Result<()> {
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
    let body = std::fs::read_to_string(site_dir.join("search.html")).unwrap_or_default();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crate 目录应有两级父目录")
        .to_path_buf()
}

/// 产出首轮基线并报告条目。
#[test]
#[ignore = "写基线到 testdata/baselines/，按需手动跑"]
fn 产出首轮基线() {
    let root = repo_root();
    let captures = root.join("testdata").join("captures");
    let site_dir = captures.join("site");
    let baseline_dir = root.join("testdata").join("baselines");

    assert!(
        site_dir.join("search.html").is_file(),
        "合成站点应存在：{}（先跑 capture_synthetic 的产出动作）",
        site_dir.display()
    );

    // 启动站点 + 把样本源指向它，写到一个临时"真实书源目录"
    let site = Site::start(site_dir.clone());
    let base = site.base_url();

    let source_dir = tempfile::tempdir().expect("应能建临时目录");
    let sample = std::fs::read_to_string(captures.join("sources").join("ref-runner-sample.json"))
        .expect("样本书源应可读");
    let mut value: serde_json::Value = serde_json::from_str(&sample).expect("应是合法 JSON");
    value["bookSourceUrl"] = serde_json::Value::String(base);
    let source_path = source_dir.path().join("ref-runner-sample.json");
    std::fs::write(
        &source_path,
        serde_json::to_string_pretty(&value).expect("应能序列化"),
    )
    .expect("应能写书源");

    let data_dir = tempfile::tempdir().expect("应能建临时数据目录");
    let executor =
        tome_cli::build_executor(&tome_cli::NetworkMode::Live, None).expect("应能构造执行器");
    let host: Arc<dyn tome_core::NetHost> = Arc::new(executor);

    // 导入书源（verify --real-sources 按 id 载源）
    let import_argv: Vec<String> = vec![
        "source".into(),
        "import".into(),
        source_path.to_string_lossy().to_string(),
        "--data-dir".into(),
        data_dir.path().to_string_lossy().to_string(),
    ];
    let options = GlobalOptions {
        data_dir: Some(data_dir.path().to_path_buf()),
        ..GlobalOptions::default()
    };
    match parse(&import_argv) {
        ParseOutcome::Run { command, .. } => {
            let mut stdin = std::io::empty();
            let outcome = tome_cli::commands::run(&options, &command, host.as_ref(), &mut stdin);
            assert_eq!(outcome.exit_code, 0, "导入应成功：{}", outcome.stderr);
        }
        other => panic!("导入参数应解析成功，实际 {other:?}"),
    }

    // 跑 verify --real-sources --write-baseline
    let verify_argv: Vec<String> = vec![
        "verify".into(),
        "--real-sources".into(),
        source_dir.path().to_string_lossy().to_string(),
        "--golden".into(),
        baseline_dir.to_string_lossy().to_string(),
        "--write-baseline".into(),
    ];
    let command = match parse(&verify_argv) {
        ParseOutcome::Run { command, .. } => command,
        other => panic!("verify 参数应解析成功，实际 {other:?}"),
    };
    let mut stdin = std::io::empty();
    let outcome = tome_cli::commands::run(&options, &command, host.as_ref(), &mut stdin);
    assert_eq!(
        outcome.exit_code, 0,
        "建基线应成功\nstdout:\n{}\nstderr:\n{}",
        outcome.stdout, outcome.stderr
    );

    let baseline_file = baseline_dir.join("fingerprint.json");
    assert!(
        baseline_file.is_file(),
        "基线应落盘：{}",
        baseline_file.display()
    );

    let text = std::fs::read_to_string(&baseline_file).expect("应能读基线");
    let entries: Vec<tome_cli::verify_real::Fingerprint> =
        serde_json::from_str(&text).expect("基线应是合法指纹数组");
    println!("基线已写出 → {}", baseline_file.display());
    for entry in &entries {
        println!(
            "  {} ok={} entries={} chars={} digest={}",
            entry.case, entry.ok, entry.entries, entry.total_chars, entry.digest
        );
    }
    assert!(!entries.is_empty(), "基线不该为空");
}
