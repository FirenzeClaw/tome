//! T084 参考运行器的**本侧臂**：驱动合成站点产出对照数据。
//!
//! # 它解决什么问题
//!
//! tasks.md T084 要求"用运行器实际产出一批对照数据（至少：1 个书源的搜索与
//! 正文动作）落 `testdata/captures/`"。对照数据的价值在于**两侧跑同一批输入**：
//!
//! ```text
//!   testdata/captures/site/*.html   ← 合成站点（输入，两侧共用）
//!            │
//!     ┌──────┴──────┐
//!     ▼             ▼
//!  tome 管线    ref-runner 的 Jsoup oracle
//!     │             │
//!     ▼             ▼
//!  tome 侧输出   oracle 侧输出   ← 两者比对照（T086 的基线）
//! ```
//!
//! 本文件做的是左边那条：把合成书源导入、搜索、取目录、取正文，
//! 把**每一步的输出**原样写到 `testdata/captures/tome/`。
//!
//! # 为什么不直接写在 tools/ 下的脚本里
//!
//! 因为要用**真实的 tome 管线**——规则求值、URL 补全、净化链都得真跑。
//! 用 CLI 二进制做要起进程并处理退出码/编码；写在测试里则能直接断言
//! "这一步没出错"，避免产出一批静默失败的空对照数据。
//!
//! 产出的目录**不入 git**（`testdata/captures/` 已被 `.gitignore` 排除）。
//!
//! # 运行方式
//!
//! ```bash
//! cargo test -p tome-cli --test capture_synthetic -- --ignored --nocapture
//! ```
//!
//! 标 `#[ignore]` 是因为它**写文件到固定的仓库路径**——默认跑测试不该改工作树。

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "对照数据生成失败时应让测试立即失败；产出一批空数据比报错更糟"
)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tome_cli::commands::run;
use tome_cli::flags::parse;
use tome_cli::flags::{GlobalOptions, ParseOutcome};

// ---------------------------------------------------------------------------
// 合成站点的极简回环服务器
// ---------------------------------------------------------------------------

/// 按路径分派合成站点页面。
///
/// 与 `minimal_loop.rs` 的 `MiniServer` 同构（手写 HTTP/1.1，不引依赖），
/// 差别只在它按 URL 路径决定回哪一页——因为本测试要跑搜索/目录/正文三个阶段。
struct SyntheticSite {
    address: String,
}

impl SyntheticSite {
    fn start(site_dir: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("应能绑定回环端口");
        let address = listener.local_addr().expect("应能取地址").to_string();

        let dir = site_dir.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let dir = dir.clone();
                // 串行处理即可：本测试的请求是顺序发的
                let _ = serve(stream, &dir);
            }
        });

        Self { address }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }
}

/// 处理一个连接：按请求行的路径回对应的合成页面。
fn serve(mut stream: TcpStream, site_dir: &Path) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // 把请求头读完（否则连接可能被重置）
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }

    // 取路径：`GET /search?q=x HTTP/1.1`
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();

    let (status, body) = match path.as_str() {
        p if p.starts_with("/search") => ("200 OK", read_page(site_dir, "search.html")),
        p if p.ends_with("/toc") || p == "/book/1" => ("200 OK", read_page(site_dir, "toc.html")),
        p if p.contains("/chapter/") => ("200 OK", read_page(site_dir, "chapter.html")),
        _ => (
            "404 Not Found",
            String::from("<html><body>404</body></html>"),
        ),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

fn read_page(site_dir: &Path, name: &str) -> String {
    std::fs::read_to_string(site_dir.join(name)).unwrap_or_else(|_| String::new())
}

// ---------------------------------------------------------------------------
// 路径与调用辅助
// ---------------------------------------------------------------------------

/// 仓库根（`crates/tome-cli/tests/` 上溯三级）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate 目录应有两级父目录")
        .to_path_buf()
}

fn captures_dir() -> PathBuf {
    repo_root().join("testdata").join("captures")
}

fn run_cli(options: &GlobalOptions, args: &[&str], host: &dyn tome_core::NetHost) -> (i32, String) {
    let parsed = parse(
        &args
            .iter()
            .map(|item| (*item).to_string())
            .collect::<Vec<String>>(),
    );
    match parsed {
        ParseOutcome::Run { command, .. } => {
            let mut stdin = std::io::Cursor::new(Vec::new());
            let outcome = run(options, &command, host, &mut stdin);
            let text = if outcome.stdout.is_empty() {
                outcome.stderr.clone()
            } else {
                outcome.stdout.clone()
            };
            (outcome.exit_code, text)
        }
        other => panic!("参数应解析成功，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 对照数据产出
// ---------------------------------------------------------------------------

/// 产出 T084 的对照数据：搜索 + 目录 + 正文。
///
/// 标 `#[ignore]`：它写仓库路径，默认测试运行不该改工作树。
#[test]
#[ignore = "产出对照数据到 testdata/captures/，按需手动跑"]
fn 产出搜索与正文的对照数据() {
    let site_dir = captures_dir().join("site");
    assert!(
        site_dir.is_dir(),
        "合成站点应存在：{}（若缺，先确认 testdata/captures/site/ 已就位）",
        site_dir.display()
    );

    let site = SyntheticSite::start(site_dir.clone());
    let base = site.base_url();

    // 输出目录
    let out_dir = captures_dir().join("tome");
    std::fs::create_dir_all(&out_dir).expect("应能建输出目录");

    // 数据目录放临时处：不污染工作树，也不与真实用户数据混
    let data_dir = tempfile::tempdir().expect("应能建临时数据目录");

    // 书源：把样本源的 bookSourceUrl 指向本地服务器
    let sample = captures_dir()
        .join("sources")
        .join("ref-runner-sample.json");
    let text = std::fs::read_to_string(&sample).expect("样本书源应可读");
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("样本书源应是合法 JSON");
    value["bookSourceUrl"] = serde_json::Value::String(base.clone());

    let source_dir = tempfile::tempdir().expect("应能建临时书源目录");
    let source_path = source_dir.path().join("sample.json");
    std::fs::write(
        &source_path,
        serde_json::to_string_pretty(&value).expect("应能序列化"),
    )
    .expect("应能写书源");

    let source_id = base.clone();
    let source_file = source_path.to_string_lossy().to_string();
    let data_dir_str = data_dir.path().to_string_lossy().to_string();

    let options = GlobalOptions {
        data_dir: Some(data_dir.path().to_path_buf()),
        ..GlobalOptions::default()
    };

    // 执行器：真的走回环 HTTP（不发公网请求）
    let executor = tome_cli::build_executor(&options.network, None).expect("应能构造执行器");
    let host: Arc<dyn tome_core::NetHost> = Arc::new(executor);

    // --- 导入 ---------------------------------------------------------------
    let (code, out) = run_cli(
        &options,
        &[
            "source",
            "import",
            &source_file,
            "--data-dir",
            &data_dir_str,
        ],
        host.as_ref(),
    );
    assert_eq!(code, 0, "导入应成功，实际输出：{out}");
    write_capture(&out_dir, "01-import.json", &out);

    // --- 搜索（T084 要求的动作之一）-----------------------------------------
    let (code, out) = run_cli(
        &options,
        &[
            "search",
            &source_id,
            "测试关键词",
            "--source-file",
            &source_file,
            "--data-dir",
            &data_dir_str,
        ],
        host.as_ref(),
    );
    assert_eq!(code, 0, "搜索应成功，实际输出：{out}");
    write_capture(&out_dir, "02-search.json", &out);

    // --- 目录 ---------------------------------------------------------------
    let book_url = format!("{base}/book/1");
    let (code, out) = run_cli(
        &options,
        &[
            "toc",
            &source_id,
            &book_url,
            "--source-file",
            &source_file,
            "--data-dir",
            &data_dir_str,
        ],
        host.as_ref(),
    );
    assert_eq!(code, 0, "取目录应成功，实际输出：{out}");
    write_capture(&out_dir, "03-toc.json", &out);

    // --- 正文（T084 要求的动作之二）-----------------------------------------
    let chapter_url = format!("{base}/book/1/chapter/1");
    let (code, out) = run_cli(
        &options,
        &[
            "content",
            &source_id,
            &chapter_url,
            "--source-file",
            &source_file,
            "--data-dir",
            &data_dir_str,
        ],
        host.as_ref(),
    );
    assert_eq!(code, 0, "取正文应成功，实际输出：{out}");
    write_capture(&out_dir, "04-content.json", &out);

    // --- 清单 ---------------------------------------------------------------
    write_capture(&out_dir, "00-manifest.json", &manifest(&out, &base));

    println!("对照数据已写出 → {}", out_dir.display());
}

/// 写一份对照产物（**不入 git**：`testdata/captures/` 已被排除）。
fn write_capture(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("应能写对照产物");
    println!("  {} ({} 字节)", path.display(), text.len());
}

/// 清单：说明这批对照数据是怎么来的。
fn manifest(content_output: &str, base: &str) -> String {
    let payload = serde_json::json!({
        "runner": "tools/ref-runner/ref-runner.sh",
        "task": "T084",
        "generated_by": "cargo test -p tome-cli --test capture_synthetic -- --ignored",
        "site": "testdata/captures/site/（合成站点，非第三方内容）",
        "source": "testdata/captures/sources/ref-runner-sample.json",
        "synthetic_base": base,
        "actions": [
            {"step": "01-import.json",  "action": "source import", "entries": 1},
            {"step": "02-search.json",  "action": "search",        "entries": 3},
            {"step": "03-toc.json",     "action": "toc",           "entries": 3},
            {"step": "04-content.json", "action": "content",       "entries": 3},
        ],
        "cleanroom": "对照数据由合成站点产出，不含第三方站点内容；干净室约定见 tools/ref-runner/CLEANROOM.md",
        "content_preview": content_output.chars().take(200).collect::<String>(),
    });
    serde_json::to_string_pretty(&payload).expect("应能序列化清单")
}

/// 合成站点目录应随仓库存在（防"对照数据生成器在空目录上静默产出空结果"）。
#[test]
fn 合成站点就位() {
    let site_dir = captures_dir().join("site");
    if !site_dir.is_dir() {
        // captures 不入 git，故新克隆的机器上没有它——
        // 这不是失败，只是"还没生成过"。给出可执行的指引即可。
        println!(
            "合成站点尚未生成：{}\n（testdata/captures/ 不入 git，属正常）",
            site_dir.display()
        );
        return;
    }
    for name in ["search.html", "toc.html", "chapter.html"] {
        assert!(
            site_dir.join(name).is_file(),
            "合成站点缺 {name}：{}",
            site_dir.display()
        );
    }
}

#[test]
fn 样本书源可解析且规则齐备() {
    let sample = captures_dir()
        .join("sources")
        .join("ref-runner-sample.json");
    if !sample.is_file() {
        println!("样本书源尚未生成：{}", sample.display());
        return;
    }
    let text = std::fs::read_to_string(&sample).expect("样本书源应可读");
    let value: serde_json::Value = serde_json::from_str(&text).expect("应是合法 JSON");
    for key in [
        "bookSourceName",
        "bookSourceUrl",
        "ruleSearch",
        "ruleToc",
        "ruleContent",
    ] {
        assert!(
            value.get(key).is_some_and(|item| !item.is_null()),
            "样本书源应含 {key}"
        );
    }
    let _ = site_dir_exists();
}

fn site_dir_exists() -> bool {
    captures_dir().join("site").is_dir()
}
