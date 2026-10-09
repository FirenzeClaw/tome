//! S1-1 最小闭环的端到端测试（tasks.md T028/T029 与 Phase 3 检查点）
//!
//! # 这是真正的端到端：真 HTTP 服务器 + 真录制文件 + 真 CLI 路径
//!
//! 与各 crate 的单元/集成测试不同，这里跑的是**完整链路**：
//!
//! ```text
//!  fixture 书源文件
//!        │  source import
//!        ▼
//!   导入报告（JSON）
//!        │  search --source-file … --record <dir>
//!        ▼  真实 HTTP（本地回环服务器）
//!   搜索结果（JSON）      ← ①
//!        │  search --source-file … --replay <dir> --offline
//!        ▼  零网络
//!   搜索结果（JSON）      ← ②
//!   断言 ① == ② 逐字节一致
//! ```
//!
//! # 为什么用本地回环服务器而不是 mock 宿主
//!
//! T029 的验收项是"录制→离线重放输出**逐字节一致**"，而录制的写入发生在
//! **执行器**里（`NetError::Status` 之外的响应后处理）。若用 mock 宿主替掉
//! 执行器，录制路径根本不会被跑——测的就不是这条链路了。故这里起一个
//! 真实的 TCP 服务器（手写 HTTP/1.1 响应，不引入新依赖、不触公网）。
//!
//! # 与 Phase 3 检查点的对应
//!
//! | 检查点（tasks.md 第 142-146 行） | 本文件的测试 |
//! |---|---|
//! | 导入 CSS+正则型简化书源样本 → 搜索出结果 | [`完整闭环_导入后搜索出结果`] |
//! | `--replay` 离线重放一致 | [`录制后离线重放输出逐字节一致`] |
//! | 无全局 CONFIG；RequestSpec 纯化成立 | [`规则求值不发请求_纯化成立`] |
//! | CLI 输出结构化 JSON | [`cli_输出是结构化json`] |

// `.clippy.toml` 的 `allow-expect-in-tests` 覆盖 `#[test]` 函数体，
// 但**不覆盖**本文件里的辅助函数（如 `MiniServer::start`、`localized_source`）。
// 这些辅助函数若失败，说明测试环境本身有问题（端口绑不上、临时目录建不了、
// fixture 读不到），此时让调用它的那个测试当场失败是正确行为。
// 故显式豁免并说明理由——与 `tome-net/tests/net_integration.rs` 同一处理。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tome_cli::commands::run;
use tome_cli::flags::{parse, GlobalOptions, OutputFormat, ParseOutcome};

// ---------------------------------------------------------------------------
// 极简回环 HTTP 服务器
// ---------------------------------------------------------------------------

/// 一次回环 HTTP 交互的服务器。
///
/// 只实现测试需要的最小子集：读请求头（含 `Content-Length` 的体），
/// 回一段固定的 HTML。不引入 HTTP 库——测试依赖越少，失败时越容易定位。
struct MiniServer {
    address: String,
    /// 已服务的请求数（用于断言"离线重放不触网"）
    served: Arc<AtomicUsize>,
    shutdown: Arc<AtomicUsize>,
}

impl MiniServer {
    /// 启动服务器，用 `body` 回应每个请求。
    fn start(body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("应能绑定回环端口");
        let address = listener.local_addr().expect("应能取地址").to_string();
        let served = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicUsize::new(0));

        let body = body.to_string();
        let served_for_thread = Arc::clone(&served);
        let shutdown_for_thread = Arc::clone(&shutdown);

        std::thread::spawn(move || {
            for incoming in listener.incoming() {
                if shutdown_for_thread.load(Ordering::SeqCst) != 0 {
                    break;
                }
                let Ok(stream) = incoming else { break };
                // 单连接失败不该终止整个服务器（客户端可能提前断开）
                let _ = serve_one(stream, &body, &served_for_thread);
            }
        });

        Self {
            address,
            served,
            shutdown,
        }
    }

    /// 服务器的基地址（`http://127.0.0.1:<port>`）。
    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// 已服务的请求数。
    fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

impl Drop for MiniServer {
    fn drop(&mut self) {
        self.shutdown.store(1, Ordering::SeqCst);
        // 用一个连接唤醒 accept 循环，让它看到 shutdown 标记
        let _ = TcpStream::connect(&self.address);
    }
}

/// 处理一个连接：读请求 → 回响应。
fn serve_one(mut stream: TcpStream, body: &str, served: &AtomicUsize) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);

    // 请求行
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }

    // 请求头（读到空行）
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    // 请求体（若声明了长度）
    if content_length > 0 {
        let mut body_buffer = vec![0u8; content_length];
        reader.read_exact(&mut body_buffer)?;
    }

    served.fetch_add(1, Ordering::SeqCst);

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

// ---------------------------------------------------------------------------
// 测试夹具
// ---------------------------------------------------------------------------

/// 搜索页 HTML（与 fixture 书源的规则对应）。
///
/// 含两本书、每本五字段，且带广告串以覆盖正文净化规则。
const SEARCH_HTML: &str = r#"<!DOCTYPE html>
<html><body>
<ul class="list">
  <li class="item">
    <h3 class="name"><a href="/book/1001">斗破苍穹</a></h3>
    <p class="author">天蚕土豆</p>
    <p class="kind">玄幻</p>
    <p class="intro">这里是简介一</p>
    <img src="/cover/1001.jpg">
  </li>
  <li class="item">
    <h3 class="name"><a href="/book/1002">完美世界</a></h3>
    <p class="author">辰东</p>
    <p class="kind">仙侠</p>
    <p class="intro">这里是简介二</p>
    <img src="/cover/1002.jpg">
  </li>
</ul>
</body></html>"#;

/// fixture 书源文件的路径（仓库内，随测试分发）。
fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("simplified-source.json")
}

/// 把 fixture 书源里的 `bookSourceUrl` 改成本地服务器地址，
/// 写到临时目录（fixture 本身要保持指向 `example.com` 的语义）。
fn localized_source(server: &MiniServer) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("应能建临时目录");
    let path = dir.path().join("source.json");

    let text = std::fs::read_to_string(fixture_path()).expect("fixture 应可读");
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("fixture 应是合法 JSON");
    value["bookSourceUrl"] = serde_json::Value::String(server.base_url());
    // 搜索地址也要指向本地（fixture 里是相对路径，但 base 变了即可）
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&value).expect("应能序列化"),
    )
    .expect("应能写书源文件");

    (dir, path)
}

/// 用给定的全局选项与参数跑一次 CLI，返回结果。
///
/// ## 为什么自动补 `data_dir`
///
/// 未指定时注册表会落到**工作目录**的 `.tome/`（那是产品设计的"可移植模式"），
/// 但测试用它会**污染工作树**——跑一遍测试后仓库里会冒出个 `.tome/`。
/// 故这里在调用方没显式指定时重定向到临时目录。临时目录的 guard 活到
/// 内层调用返回为止（用作用域而非 `into_path()`，后者会泄漏目录）。
fn run_cli(
    options: &GlobalOptions,
    args: &[&str],
    host: &dyn tome_core::NetHost,
) -> tome_cli::CommandOutcome {
    if options.data_dir.is_none() {
        let dir = tempfile::tempdir().expect("应能建临时数据目录");
        let mut effective = options.clone();
        effective.data_dir = Some(dir.path().to_path_buf());
        return run_cli_inner(&effective, args, host);
    }
    run_cli_inner(options, args, host)
}

/// 真正执行的那一层（`data_dir` 已确保不为 `None`）。
fn run_cli_inner(
    options: &GlobalOptions,
    args: &[&str],
    host: &dyn tome_core::NetHost,
) -> tome_cli::CommandOutcome {
    let parsed = parse(
        &args
            .iter()
            .map(|item| (*item).to_string())
            .collect::<Vec<String>>(),
    );
    match parsed {
        ParseOutcome::Run { command, .. } => {
            let mut stdin = std::io::Cursor::new(Vec::new());
            run(options, &command, host, &mut stdin)
        }
        other => panic!("参数应解析成功，实际 {other:?}"),
    }
}

/// 构造执行器（按网络模式）。
fn executor_for(options: &GlobalOptions) -> tome_net::RequestExecutor {
    tome_cli::build_executor(&options.network, None).expect("应能构造执行器")
}

// ---------------------------------------------------------------------------
// 检查点：导入 → 搜索出结果
// ---------------------------------------------------------------------------

#[test]
fn 完整闭环_导入后搜索出结果() {
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);

    // ① 导入：报告应显示 1 条新增
    // `--data-dir` 指向临时目录：不指定时注册表会落到工作目录的 `.tome/`，
    // 那是**测试污染工作树**——必须避免（本测试最初就踩了这个坑）。
    let data_dir = tempfile::tempdir().expect("应能建数据目录");
    let options = GlobalOptions {
        format: OutputFormat::Json,
        data_dir: Some(data_dir.path().to_path_buf()),
        ..GlobalOptions::default()
    };
    let executor = executor_for(&options);
    let imported = run_cli(
        &options,
        &[
            "source",
            "import",
            source_path.to_str().expect("路径应为 UTF-8"),
        ],
        &executor,
    );
    assert_eq!(imported.exit_code, 0, "导入应成功：{}", imported.stderr);
    let report: serde_json::Value =
        serde_json::from_str(&imported.stdout).expect("导入报告应是 JSON");
    assert_eq!(report["added"], 1, "应新增 1 条书源");
    assert_eq!(report["skipped"], 0);

    // ② 搜索：应产出 2 条结构化结果
    let searched = run_cli(
        &options,
        &[
            "search",
            "--source-file",
            source_path.to_str().expect("路径应为 UTF-8"),
            "斗破",
        ],
        &executor,
    );
    assert_eq!(searched.exit_code, 0, "搜索应成功：{}", searched.stderr);

    let books: serde_json::Value =
        serde_json::from_str(&searched.stdout).expect("搜索结果应是 JSON 数组");
    let books = books.as_array().expect("应是数组");
    assert_eq!(books.len(), 2, "应搜到 2 本书");

    // 契约 cli.md 定义的字段
    assert_eq!(books[0]["name"], "斗破苍穹");
    assert_eq!(books[0]["author"], "天蚕土豆");
    assert_eq!(books[0]["intro"], "这里是简介一");
    assert_eq!(
        books[0]["book_url"],
        format!("{}/book/1001", server.base_url()),
        "bookUrl 规则里的 ##^## 前缀补全应生效"
    );
    assert_eq!(books[0]["cover"], "/cover/1001.jpg");

    // 服务器确实被访问过（真实 HTTP，不是 mock）
    assert!(server.served() >= 1, "应真实发过请求");
}

#[test]
fn cli_输出是结构化json() {
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let options = GlobalOptions::default();
    let executor = executor_for(&options);

    let outcome = run_cli(
        &options,
        &[
            "search",
            "--source-file",
            source_path.to_str().unwrap(),
            "k",
        ],
        &executor,
    );
    assert_eq!(outcome.exit_code, 0);

    // 严格解析（拒绝尾随内容）→ 输出里没有日志噪音混入
    let mut deserializer = serde_json::Deserializer::from_str(&outcome.stdout);
    let _: serde_json::Value =
        serde::Deserialize::deserialize(&mut deserializer).expect("应是合法 JSON");
    deserializer.end().expect("不应有尾随内容");
}

#[test]
fn 未知关键词产出空数组而非报错() {
    // 页面在，但关键词匹配不到任何东西（此处服务器不分关键词，
    // 故用一份没有 li.item 的页面模拟"无结果"）
    let server = MiniServer::start("<html><body><p>没有结果</p></body></html>");
    let (_guard, source_path) = localized_source(&server);
    let options = GlobalOptions::default();
    let executor = executor_for(&options);

    let outcome = run_cli(
        &options,
        &[
            "search",
            "--source-file",
            source_path.to_str().unwrap(),
            "不存在的书",
        ],
        &executor,
    );
    assert_eq!(outcome.exit_code, 0, "源在线但无匹配不是错误");
    let books: serde_json::Value = serde_json::from_str(&outcome.stdout).expect("应是 JSON");
    assert_eq!(books.as_array().expect("应是数组").len(), 0);
}

// ---------------------------------------------------------------------------
// 检查点：--replay 离线重放逐字节一致（T029 的核心验收项）
// ---------------------------------------------------------------------------

#[test]
fn 录制后离线重放输出逐字节一致() {
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let capture_dir = tempfile::tempdir().expect("应能建录制目录");

    let source_arg = source_path.to_str().expect("路径应为 UTF-8").to_string();
    let capture_arg = capture_dir
        .path()
        .to_str()
        .expect("路径应为 UTF-8")
        .to_string();

    // ① 录制：真实联网，把响应写进语料目录
    let record_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Record(capture_dir.path().to_path_buf()),
        data_dir: None,
    };
    let record_executor = executor_for(&record_options);
    let recorded = run_cli(
        &record_options,
        &["search", "--source-file", &source_arg, "斗破"],
        &record_executor,
    );
    assert_eq!(recorded.exit_code, 0, "录制搜索应成功：{}", recorded.stderr);
    assert!(!recorded.stdout.is_empty(), "录制时应产出结果");
    let served_during_record = server.served();
    assert!(served_during_record >= 1, "录制时应真实发过请求");

    // 语料目录应真有内容
    let captured_files: Vec<PathBuf> = std::fs::read_dir(capture_dir.path())
        .expect("录制目录应可读")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();
    assert!(
        !captured_files.is_empty(),
        "录制后语料目录不该为空（目录：{}）",
        capture_dir.path().display()
    );

    // ② 离线重放：绝不触网
    let replay_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Replay {
            root: capture_dir.path().to_path_buf(),
            offline: true,
        },
        data_dir: None,
    };
    let replay_executor = executor_for(&replay_options);
    let replayed = run_cli(
        &replay_options,
        &["search", "--source-file", &source_arg, "斗破"],
        &replay_executor,
    );
    assert_eq!(replayed.exit_code, 0, "离线重放应成功：{}", replayed.stderr);

    // ③ T029 的核心断言：输出逐字节一致
    assert_eq!(
        recorded.stdout, replayed.stdout,
        "录制与离线重放的输出必须逐字节一致\n--- 录制 ---\n{}\n--- 重放 ---\n{}",
        recorded.stdout, replayed.stdout
    );

    // ④ 重放期间服务器**没有**被访问（证明"完全离线"）
    assert_eq!(
        server.served(),
        served_during_record,
        "离线重放不得发起任何真实请求"
    );

    let _ = capture_arg;
}

#[test]
fn 离线重放未命中时报错而非静默空结果() {
    // 录了 A 请求，重放时问 B——语料里没有 B
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let capture_dir = tempfile::tempdir().expect("应能建录制目录");
    let source_arg = source_path.to_str().unwrap().to_string();

    // 用关键词 "A" 录制
    let record_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Record(capture_dir.path().to_path_buf()),
        data_dir: None,
    };
    let record_executor = executor_for(&record_options);
    let _ = run_cli(
        &record_options,
        &["search", "--source-file", &source_arg, "A"],
        &record_executor,
    );

    // 换关键词 "B" 离线重放：指纹不同 → 未命中
    let replay_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Replay {
            root: capture_dir.path().to_path_buf(),
            offline: true,
        },
        data_dir: None,
    };
    let replay_executor = executor_for(&replay_options);
    let outcome = run_cli(
        &replay_options,
        &["search", "--source-file", &source_arg, "B"],
        &replay_executor,
    );

    assert_ne!(
        outcome.exit_code, 0,
        r#"未命中应报错——静默返回空结果会让「语料缺失」表现为「没有书」，两者排查方向完全不同"#
    );
    assert!(
        outcome.stderr.contains("error."),
        "应给出分类标识：{}",
        outcome.stderr
    );
}

#[test]
fn record_once_模式只补空洞() {
    // `--replay`（不带 `--offline`）的语义：命中回放、未命中补抓。
    // 这是"补空洞"的用法——已录好的部分不该因站点改版而失效。
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let capture_dir = tempfile::tempdir().expect("应能建录制目录");
    let source_arg = source_path.to_str().unwrap().to_string();

    // 先用关键词 A 录一条
    let record_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Record(capture_dir.path().to_path_buf()),
        data_dir: None,
    };
    let record_executor = executor_for(&record_options);
    let first = run_cli(
        &record_options,
        &["search", "--source-file", &source_arg, "A"],
        &record_executor,
    );
    assert_eq!(first.exit_code, 0);
    let served_after_first = server.served();

    // 再用 `RecordOnce` 模式跑同一个关键词 A：应命中语料、不触网
    let once_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Replay {
            root: capture_dir.path().to_path_buf(),
            offline: false,
        },
        data_dir: None,
    };
    let once_executor = executor_for(&once_options);
    let second = run_cli(
        &once_options,
        &["search", "--source-file", &source_arg, "A"],
        &once_executor,
    );
    assert_eq!(second.exit_code, 0);
    assert_eq!(first.stdout, second.stdout, "命中语料时输出应与录制时一致");
    assert_eq!(
        server.served(),
        served_after_first,
        r#"命中语料时不该触网（RecordOnce 的「优先回放」语义）"#
    );

    // 换成关键词 B（语料里没有）：应补抓并成功
    let third = run_cli(
        &once_options,
        &["search", "--source-file", &source_arg, "B"],
        &once_executor,
    );
    assert_eq!(third.exit_code, 0, "未命中应补抓：{}", third.stderr);
    assert!(
        server.served() > served_after_first,
        "未命中时应真实抓取一次"
    );
}

// ---------------------------------------------------------------------------
// 检查点：RequestSpec 纯化成立
// ---------------------------------------------------------------------------

#[test]
fn 规则求值不发请求_纯化成立() {
    // A2 纯化的端到端形态：用**绝不触网**的离线模式搜索，
    // 若规则求值真的只产出 RequestSpec 而不发请求，那它必须能跑通
    // （无请求也无可发——纯路径不该依赖网络设施）。
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let capture_dir = tempfile::tempdir().expect("应能建录制目录");
    let source_arg = source_path.to_str().unwrap().to_string();

    // 先录一条，让离线模式有语料可用
    let record_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Record(capture_dir.path().to_path_buf()),
        data_dir: None,
    };
    let record_executor = executor_for(&record_options);
    let _ = run_cli(
        &record_options,
        &["search", "--source-file", &source_arg, "k"],
        &record_executor,
    );
    let served = server.served();

    // 离线重放：全过程零真实请求
    let offline_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Replay {
            root: capture_dir.path().to_path_buf(),
            offline: true,
        },
        data_dir: None,
    };
    let offline_executor = executor_for(&offline_options);
    let outcome = run_cli(
        &offline_options,
        &["search", "--source-file", &source_arg, "k"],
        &offline_executor,
    );

    assert_eq!(outcome.exit_code, 0, "{}", outcome.stderr);
    assert_eq!(server.served(), served, "离线全程不得触网");

    // 更直接的纯化证据：导入本身（纯解析路径）在离线模式下也能跑
    let imported = run_cli(
        &offline_options,
        &["source", "import", source_path.to_str().unwrap()],
        &offline_executor,
    );
    assert_eq!(
        imported.exit_code, 0,
        "纯解析路径应无需网络：{}",
        imported.stderr
    );
    assert_eq!(server.served(), served);
}

#[test]
fn 无全局配置_配置经参数注入() {
    // A3（禁止全局 CONFIG 单例）：同一进程内用**不同配置**跑两次，
    // 结果必须各自独立——若存在全局单例，第二次会被第一次污染。
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let source_arg = source_path.to_str().unwrap().to_string();

    // data_dir 指向临时目录：不指定时注册表会落到工作目录的 `.tome/`（测试污染）
    let data_dir = tempfile::tempdir().expect("应能建数据目录");
    let json_options = GlobalOptions {
        format: OutputFormat::Json,
        data_dir: Some(data_dir.path().to_path_buf()),
        ..GlobalOptions::default()
    };
    let human_options = GlobalOptions {
        format: OutputFormat::Human,
        data_dir: Some(data_dir.path().to_path_buf()),
        ..GlobalOptions::default()
    };

    let json_executor = executor_for(&json_options);
    let human_executor = executor_for(&human_options);

    let json_out = run_cli(
        &json_options,
        &["search", "--source-file", &source_arg, "k"],
        &json_executor,
    );
    let human_out = run_cli(
        &human_options,
        &["search", "--source-file", &source_arg, "k"],
        &human_executor,
    );

    assert_eq!(json_out.exit_code, 0);
    assert_eq!(human_out.exit_code, 0);
    assert!(
        serde_json::from_str::<serde_json::Value>(&json_out.stdout).is_ok(),
        "json 格式应输出 JSON"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&human_out.stdout).is_err(),
        "human 格式不该是 JSON——若两次输出同形，说明格式来自全局单例"
    );
    assert!(human_out.stdout.contains("斗破苍穹"));
}

// ---------------------------------------------------------------------------
// 契约流程：import 之后按 id 搜索（`contracts/cli.md`）
// ---------------------------------------------------------------------------

/// `contracts/cli.md` 定义的规范流程是 `source import` 之后 `search <source-id>`。
/// 本测试跑完整的那条流程（含注册表落盘与按 id 载入），而不是退化成
/// `--source-file`——后者是便利路径，不是契约本身。
#[test]
fn 按_id_搜索的契约流程() {
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let data_dir = tempfile::tempdir().expect("应能建数据目录");

    let options = GlobalOptions {
        format: OutputFormat::Json,
        data_dir: Some(data_dir.path().to_path_buf()),
        ..GlobalOptions::default()
    };
    let executor = executor_for(&options);

    // ① 导入：应写入注册表
    let imported = run_cli(
        &options,
        &["source", "import", source_path.to_str().unwrap()],
        &executor,
    );
    assert_eq!(imported.exit_code, 0, "{}", imported.stderr);
    let report: serde_json::Value = serde_json::from_str(&imported.stdout).expect("应是 JSON");
    assert_eq!(report["registry_added"], 1, "应写进注册表");
    assert_eq!(report["registry_total"], 1);

    // 注册表文件应真的存在且可读
    let registry_file = data_dir.path().join("sources.json");
    assert!(registry_file.exists(), "注册表文件应被写出");

    // ② 按 id 搜索（契约形态：位置参数就是 source-id）
    let source_id = server.base_url();
    let searched = run_cli(&options, &["search", &source_id, "斗破"], &executor);
    assert_eq!(searched.exit_code, 0, "{}", searched.stderr);
    let books: serde_json::Value = serde_json::from_str(&searched.stdout).expect("应是 JSON");
    assert_eq!(books.as_array().expect("应是数组").len(), 2);
    assert_eq!(books[0]["name"], "斗破苍穹");
}

#[test]
fn 按_id_搜索时未导入的_id_报错() {
    let data_dir = tempfile::tempdir().expect("应能建数据目录");
    let options = GlobalOptions {
        format: OutputFormat::Json,
        data_dir: Some(data_dir.path().to_path_buf()),
        ..GlobalOptions::default()
    };
    let executor = executor_for(&options);

    let outcome = run_cli(
        &options,
        &["search", "https://never-imported.com", "k"],
        &executor,
    );
    assert_eq!(outcome.exit_code, 1, "源不存在是业务错误");
    assert!(
        outcome.stderr.contains("error.rule.source.missing"),
        "应给出源缺失的分类标识：{}",
        outcome.stderr
    );
    assert!(
        outcome.stderr.contains("source import"),
        "应提示正确的下一步（先导入）：{}",
        outcome.stderr
    );
}

#[test]
fn 重复导入同一_id_计为更新且不重复条目() {
    // spec 边缘情况：「导入重复书源（同 id）按"更新"处理」
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let data_dir = tempfile::tempdir().expect("应能建数据目录");
    let options = GlobalOptions {
        format: OutputFormat::Json,
        data_dir: Some(data_dir.path().to_path_buf()),
        ..GlobalOptions::default()
    };
    let executor = executor_for(&options);

    let first = run_cli(
        &options,
        &["source", "import", source_path.to_str().unwrap()],
        &executor,
    );
    assert_eq!(first.exit_code, 0);
    let report: serde_json::Value = serde_json::from_str(&first.stdout).expect("应是 JSON");
    assert_eq!(report["registry_added"], 1);

    let second = run_cli(
        &options,
        &["source", "import", source_path.to_str().unwrap()],
        &executor,
    );
    assert_eq!(second.exit_code, 0);
    let report: serde_json::Value = serde_json::from_str(&second.stdout).expect("应是 JSON");
    assert_eq!(report["registry_added"], 0, "同 id 不该重复新增");
    assert_eq!(report["registry_updated"], 1, "应计为更新");
    assert_eq!(report["registry_total"], 1, "总条数不该增长");
}

// ---------------------------------------------------------------------------
// SC-001：从触发到首个来源结果 ≤ 30 秒
// ---------------------------------------------------------------------------

/// SC-001 的计时口径与实测（spec §SC-001）。
///
/// ## 口径（"从输入关键词到看到至少一个来源的搜索结果"对应代码里的哪一段）
///
/// 计时区间 = **`search` 命令入口（`commands::run`）到首个源的结果产出**。
/// 不含进程启动、书源文件读取、参数解析——那些是"程序还没开始搜索"的部分，
/// 而 SC-001 说的是"读者从输入关键词到看到结果"。
///
/// 严格讲还应包含"用户敲完回车"到"命令入口"之间的一切（读写书源文件、
/// 构造执行器），故本测试的区间是**宽松下界**；完整链路的实测数字另记。
///
/// ## 为什么用回放而不是真实网络
///
/// SC-001 的前提是"正常网络条件"，而测试不能依赖外网（CI 无网、站点会挂）。
/// 回放路径与真实路径共用同一套规则求值 + 结果解析（只有取响应那一步不同），
/// 故它测的是"引擎处理一条响应要多久"。真实网络下的总耗时等于本测的耗时
/// 加上网络往返时间；本地回环的往返在毫秒级，故本测给出的数字对真实网络
/// 仍是有效参考。
///
/// ## 阈值
///
/// 断言取 5 秒而非 30 秒：CI runner 的负载波动可能让耗时翻几倍，而 5 秒
/// 与实际耗时（数十毫秒）之间已有两个数量级余量——这个断言要抓的是
/// "数量级退化"（例如某处引入了 O(n²) 或固定 sleep），不是精确计时。
#[test]
fn sc001_首个来源结果远快于30秒() {
    let server = MiniServer::start(SEARCH_HTML);
    let (_guard, source_path) = localized_source(&server);
    let capture_dir = tempfile::tempdir().expect("应能建录制目录");
    let source_arg = source_path.to_str().expect("路径应为 UTF-8").to_string();

    // 先录一条，让计时跑在确定性的回放路径上
    let options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Record(capture_dir.path().to_path_buf()),
        data_dir: None,
    };
    let executor = executor_for(&options);
    let warmup = run_cli(
        &options,
        &["search", "--source-file", &source_arg, "斗破"],
        &executor,
    );
    assert_eq!(warmup.exit_code, 0, "录制应成功：{}", warmup.stderr);

    let replay_options = GlobalOptions {
        format: OutputFormat::Json,
        network: tome_cli::NetworkMode::Replay {
            root: capture_dir.path().to_path_buf(),
            offline: true,
        },
        data_dir: None,
    };
    let replay_executor = executor_for(&replay_options);

    // 计时：命令入口 → 结果产出
    let started = std::time::Instant::now();
    let outcome = run_cli(
        &replay_options,
        &["search", "--source-file", &source_arg, "斗破"],
        &replay_executor,
    );
    let elapsed = started.elapsed();

    assert_eq!(outcome.exit_code, 0);
    assert!(
        !outcome.stdout.is_empty()
            && outcome.stdout
                != "[]
",
        r#"应产出至少一条结果（SC-001 的判据是「至少一个来源的搜索结果」）"#
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "SC-001：首个来源结果耗时 {elapsed:?}，远超 30 秒预算（阈值取 5 秒留两个数量级余量）"
    );
}

// ---------------------------------------------------------------------------
// 检查点：错误路径的退出码
// ---------------------------------------------------------------------------

#[test]
fn 不存在的书源文件给出退出码1() {
    let options = GlobalOptions::default();
    let executor = executor_for(&options);
    let outcome = run_cli(
        &options,
        &["search", "--source-file", "/definitely/missing.json", "k"],
        &executor,
    );
    assert_eq!(outcome.exit_code, 1);
    assert!(outcome.stderr.contains("error.configuration.io"));
}

#[test]
fn 用法错误由解析层报出() {
    // 退出码 2 由 `flags::parse` 的 Usage 分支保证（main.rs 转成 2）；
    // 这里断言解析层确实把它判成用法错误而非正常运行。
    let parsed = parse(&["--record".to_string(), "/a".to_string()]);
    assert!(
        matches!(parsed, ParseOutcome::Usage(_)),
        "缺子命令应是用法错误"
    );
}

#[test]
fn 服务器不可达时给出网络类错误() {
    // 起一个服务器拿端口再关掉，得到一个"几乎必然无人监听"的地址
    let listener = TcpListener::bind("127.0.0.1:0").expect("应能绑定");
    let address = listener.local_addr().expect("应能取地址");
    drop(listener);

    let dir = tempfile::tempdir().expect("应能建目录");
    let path = dir.path().join("source.json");
    let text = std::fs::read_to_string(fixture_path()).expect("fixture 应可读");
    let mut value: serde_json::Value = serde_json::from_str(&text).expect("应是 JSON");
    value["bookSourceUrl"] = serde_json::Value::String(format!("http://{address}"));
    std::fs::write(&path, serde_json::to_string(&value).unwrap()).expect("应能写");

    let options = GlobalOptions::default();
    let executor = executor_for(&options);
    let outcome = run_cli(
        &options,
        &["search", "--source-file", path.to_str().unwrap(), "k"],
        &executor,
    );
    assert_eq!(outcome.exit_code, 1, "目标站不可达应是业务错误（1）");
    assert!(
        outcome.stderr.contains("error.network"),
        "应归网络类：{}",
        outcome.stderr
    );
}
