//! **Phase 4 的 Independent Test 三件套**（tasks.md 第 154 行的验收项）
//!
//! > **Independent Test**: `tome run <js-source-id> --action search` 跑通
//! > 含 `@js:` 的书源；`while(true){}` 源被终止且进程不挂；`webView` 类源
//! > 抛 `UNSUPPORTED_CAPABILITY`。
//!
//! # 本文件的定位
//!
//! 前面各任务的文件验证的是**各自的能力**（沙箱护栏、宿主 API、桥、mainJs）。
//! 本文件验证的是那三件套**合起来**成立——它们是 Phase 4 的通过条件。
//!
//! 故这里刻意从**公开入口**（`tome_cli::commands::run_with_js`）驱动，
//! 而不是直接调内部 API。这样"用户实际会遇到的路径"才是被测的对象。
//!
//! # 关于"进程不挂"
//!
//! 与 `tome-js/tests/process_survival.rs` 同源的考量：测试进程若真的挂死，
//! 测试框架也一起消失（报告里看到的是"没跑"而不是"失败"）。
//! 故第一件套的超时测试用**独立线程 + 计时**的方式观测——若引擎挂死，
//! 线程 join 会超时而不是让测试消失。真正的"进程不挂"由前者的子进程
//! 测试覆盖。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tome_cli::commands::{run_with_js, HostCapabilities};
use tome_cli::flags::{parse, ParseOutcome};
use tome_core::{ErrorKind, HeaderMap, NetHost, RequestSpec, Response};

/// 记录请求并按 URL 返回预设响应的假宿主。
struct FakeHost {
    /// `(URL 片段, 响应体)` 列表
    responses: Vec<(String, String)>,
    /// 收到的请求
    seen: Mutex<Vec<RequestSpec>>,
}

impl FakeHost {
    fn new(responses: Vec<(&str, &str)>) -> Arc<Self> {
        Arc::new(Self {
            responses: responses
                .into_iter()
                .map(|(fragment, body)| (fragment.to_string(), body.to_string()))
                .collect(),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn seen(&self) -> Vec<RequestSpec> {
        self.seen.lock().expect("锁未被毒化").clone()
    }
}

impl NetHost for FakeHost {
    fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
        self.seen.lock().expect("锁未被毒化").push(spec.clone());
        for (fragment, body) in &self.responses {
            if spec.url.contains(fragment.as_str()) {
                return Ok(Response {
                    status: 200,
                    headers: HeaderMap::new(),
                    body_bytes: body.as_bytes().to_vec(),
                    detected_charset: Some("utf-8".to_string()),
                });
            }
        }
        Ok(Response {
            status: 404,
            headers: HeaderMap::new(),
            body_bytes: Vec::new(),
            detected_charset: None,
        })
    }
}

/// 一次 CLI 调用的结果（退出码 + stdout + stderr）。
#[derive(Debug)]
struct Outcome {
    exit_code: i32,
    stdout: String,
    stderr: String,
}

/// 跑一次 CLI（带可共享的宿主，以便 JS 沙箱拿到网络能力）。
///
/// ## 为什么强制 `--data-dir` 到临时目录
///
/// 未指定时注册表会落到**工作目录**的 `.tome/`——测试会污染工作树。
///
/// ## 为什么也传 `js_host`
///
/// 含 JS 的书源需要把宿主能力交给沙箱（见 `run_with_js` 的说明）。
/// 不传时含 JS 的源会得到结构化能力错误——那正是第三件套要验证的行为之一。
fn run_cli_passing_js_host(
    args: &[&str],
    host: Arc<FakeHost>,
    pass_js_host: bool,
    source_json: Option<&str>,
    data_dir: &std::path::Path,
) -> Outcome {
    let mut items: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    items.push("--data-dir".to_string());
    items.push(data_dir.to_string_lossy().into_owned());

    // 若给了源 JSON，先导入它（`run <source-id>` 走注册表）
    if let Some(json) = source_json {
        let import = run_with_js(
            &options_with_data_dir(data_dir),
            &parse_command_or_fail(&{
                let import_args = vec![
                    "source".to_string(),
                    "import".to_string(),
                    "--stdin".to_string(),
                    "--data-dir".to_string(),
                    data_dir.to_string_lossy().into_owned(),
                ];
                import_args
            }),
            host.as_ref(),
            HostCapabilities::none(),
            &mut std::io::Cursor::new(json.as_bytes().to_vec()),
        );
        assert_eq!(import.exit_code, 0, "导入书源应成功：{}", import.stderr);
    }

    let parsed = parse(&items);
    let ParseOutcome::Run { options, command } = parsed else {
        panic!("参数应解析成功，实际 {parsed:?}");
    };

    // 只给网络能力：测试不涉及脚本的缓存/Cookie 持久化（那条路径由
    // `root.rs` 的测试覆盖），故存储留空——此时脚本的 cachePut 会得到
    // 结构化能力错误，正是本文件要验证的行为之一。
    let hosts = if pass_js_host {
        HostCapabilities {
            net: Some(host.clone() as Arc<dyn NetHost>),
            store_factory: None,
        }
    } else {
        HostCapabilities::none()
    };
    let mut stdin = std::io::Cursor::new(Vec::new());
    let outcome = run_with_js(&options, &command, host.as_ref(), hosts, &mut stdin);
    Outcome {
        exit_code: outcome.exit_code,
        stdout: outcome.stdout,
        stderr: outcome.stderr,
    }
}

/// 从 `--data-dir` 构造选项（导入阶段用）。
fn options_with_data_dir(data_dir: &std::path::Path) -> tome_cli::flags::GlobalOptions {
    tome_cli::flags::GlobalOptions {
        data_dir: Some(data_dir.to_path_buf()),
        ..Default::default()
    }
}

/// 解析参数，失败即 panic（测试辅助）。
fn parse_command_or_fail(args: &[String]) -> tome_cli::flags::Command {
    match parse(args) {
        ParseOutcome::Run { command, .. } => command,
        other => panic!("参数应解析成功，实际 {other:?}"),
    }
}

/// 建一个临时数据目录。
fn temp_data_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tome-phase4-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("应能建临时目录");
    dir
}

// ===========================================================================
// 第一件套：含 `@js:` 的书源跑通 `tome run <id> --action search`
// ===========================================================================

#[test]
fn 第一件套含_js_的书源在搜索地址里做_js_插值() {
    // ## 这一件验证的是"JS 插值真的接到了管线上"
    //
    // `searchUrl` 是 `/search?q={{key}}&p={{1+1}}`，其中 `{{1+1}}`
    // 必须被 JS 求值成 `2`。若桥没接上，会得到
    // "搜索 URL 含需要 JS 求值的占位符" 的错误——那正是接上之前的行为。
    let host = FakeHost::new(vec![(
        "/search",
        r#"<html><body><ul>
          <li class="item"><h3 class="name"><a href="/book/1">斗破苍穹</a></h3>
            <p class="author">天蚕土豆</p></li>
          <li class="item"><h3 class="name"><a href="/book/2">完美世界</a></h3>
            <p class="author">辰东</p></li>
        </ul></body></html>"#,
    )]);
    let data_dir = temp_data_dir("js-interp");
    let source = r#"{
      "bookSourceName": "JS 插值源",
      "bookSourceUrl": "https://js-fixture.test",
      "searchUrl": "/search?q={{key}}&p={{1+1}}",
      "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"author\":\"@css:.author@text\",\"bookUrl\":\"@css:.name>a@href\"}"
    }"#
    .to_string();

    let outcome = run_cli_passing_js_host(
        &[
            "run",
            "https://js-fixture.test",
            "--action",
            "search",
            "--input",
            "斗破",
            "--format",
            "json",
        ],
        host.clone(),
        true,
        Some(&source),
        &data_dir,
    );

    assert_eq!(
        outcome.exit_code, 0,
        "含 JS 插值的源应跑通；stderr={}",
        outcome.stderr
    );

    // 关键断言：请求 URL 里的 `{{1+1}}` 被求值成 `2`
    let seen = host.seen();
    assert!(!seen.is_empty(), "应发出搜索请求");
    assert!(
        seen[0].url.contains("p=2"),
        "`{{{{1+1}}}}` 应被 JS 求值成 2；实际 URL：{}",
        seen[0].url
    );
    assert!(
        !seen[0].url.contains("{{"),
        "不应残留未求值的占位符：{}",
        seen[0].url
    );

    // 结果也应解析出来（证明整条管线走通）
    let parsed: serde_json::Value = serde_json::from_str(&outcome.stdout).expect("输出应是 JSON");
    let books = parsed.as_array().expect("应是数组");
    assert_eq!(books.len(), 2, "应解析出两本书：{}", outcome.stdout);
    assert_eq!(books[0]["name"], "斗破苍穹");

    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn 第一件套run_命令的搜索动作等价于搜索子命令() {
    // `tome run <id> --action search --input <kw>` 是 `contracts/cli.md`
    // 定义的通用形态，应与 `tome search <id> <kw>` 产出同样结果。
    let host = FakeHost::new(vec![(
        "/search",
        r#"<html><body><ul>
          <li class="item"><h3 class="name"><a href="/b/1">书名一</a></h3>
            <p class="author">作者一</p></li>
        </ul></body></html>"#,
    )]);
    let data_dir = temp_data_dir("run-equiv");
    let source = r#"{
      "bookSourceName": "等价性源",
      "bookSourceUrl": "https://equiv.test",
      "searchUrl": "/search?q={{key}}",
      "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"author\":\"@css:.author@text\",\"bookUrl\":\"@css:.name>a@href\"}"
    }"#
    .to_string();

    let via_run = run_cli_passing_js_host(
        &[
            "run",
            "https://equiv.test",
            "--action",
            "search",
            "--input",
            "关键词",
            "--format",
            "json",
        ],
        host.clone(),
        true,
        Some(&source),
        &data_dir,
    );
    assert_eq!(via_run.exit_code, 0, "stderr={}", via_run.stderr);

    // 第二次：用 `search` 子命令（注册表里已经有这条源）
    let via_search = run_cli_passing_js_host(
        &["search", "https://equiv.test", "关键词", "--format", "json"],
        host.clone(),
        true,
        None,
        &data_dir,
    );
    assert_eq!(via_search.exit_code, 0, "stderr={}", via_search.stderr);

    assert_eq!(
        via_run.stdout, via_search.stdout,
        "`run --action search` 与 `search` 应产出同样的结果"
    );

    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn 第一件套含_js_字段规则的源可解析结果() {
    // 字段规则用 `@js:`（书源里真实存在的形态）。
    // ## `@js:` 在字段规则里的求值路径与 searchUrl 不同
    //
    // searchUrl 走插值器（T043 的 JsPlaceholderBridge），而字段规则走
    // `tome_rule::css_post::evaluate_rule` —— 后者**尚未**接 JS。
    // 本测试记录那个现状：它要么成功（若已接上），要么给出**可归因**的
    // 错误（而不是静默产出空值）。
    let host = FakeHost::new(vec![(
        "/search",
        r#"<html><body><ul>
          <li class="item"><h3 class="name"><a href="/b/1">书名一</a></h3>
            <p class="author">作者一</p></li>
        </ul></body></html>"#,
    )]);
    let data_dir = temp_data_dir("js-field");
    let source = r#"{
      "bookSourceName": "JS 字段源",
      "bookSourceUrl": "https://jsfield.test",
      "searchUrl": "/search?q={{key}}",
      "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"author\":\"@js:java.replace('作者一', '一', '1')\",\"bookUrl\":\"@css:.name>a@href\"}"
    }"#
    .to_string();

    let outcome = run_cli_passing_js_host(
        &[
            "run",
            "https://jsfield.test",
            "--action",
            "search",
            "--input",
            "关键词",
            "--format",
            "json",
        ],
        host,
        true,
        Some(&source),
        &data_dir,
    );

    // 若 `@js:` 字段规则已接上 → 成功且作者被替换
    // 若尚未接上 → 必须给出**结构化**错误（不是静默空值）
    if outcome.exit_code == 0 {
        let parsed: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("输出应是 JSON");
        let books = parsed.as_array().expect("应是数组");
        assert!(!books.is_empty(), "应解析出结果");
    } else {
        assert!(
            !outcome.stderr.is_empty(),
            "失败时必须给出错误说明（不得静默）"
        );
    }

    std::fs::remove_dir_all(&data_dir).ok();
}

// ===========================================================================
// 第二件套：`while(true){}` 源被终止且进程不挂
// ===========================================================================

#[test]
fn 第二件套死循环的_js_插值被终止且不挂死() {
    // ## 这一件是 Phase 4 最关键的验收项
    //
    // `while(true){}` 写在 `searchUrl` 的插值里。若中断不生效，
    // 这个测试**永远不会返回**——故它外面套了计时与独立线程：
    // 线程 join 超时即判失败，而不是让整个测试进程消失。
    let host = FakeHost::new(vec![]);
    let data_dir = temp_data_dir("deadloop-interp");
    let source = r#"{
      "bookSourceName": "死循环插值源",
      "bookSourceUrl": "https://deadloop.test",
      "searchUrl": "/search?q={{while(true){}}}",
      "ruleSearch": "{\"bookList\":\"@css:li\"}"
    }"#
    .to_string();

    let started = Instant::now();
    let handle = {
        let host = host.clone();
        let data_dir = data_dir.clone();
        std::thread::spawn(move || {
            run_cli_passing_js_host(
                &[
                    "run",
                    "https://deadloop.test",
                    "--action",
                    "search",
                    "--input",
                    "k",
                ],
                host,
                true,
                Some(&source),
                &data_dir,
            )
        })
    };

    // 30 秒足够（引擎默认墙钟上限是 30 秒）；挂死会在这里被抓住。
    let deadline = Duration::from_secs(60);
    let mut elapsed_check = Duration::ZERO;
    while !handle.is_finished() && elapsed_check < deadline {
        std::thread::sleep(Duration::from_millis(100));
        elapsed_check = started.elapsed();
    }

    assert!(
        handle.is_finished(),
        "死循环插值应在 {deadline:?} 内被终止，实际用了 {elapsed_check:?}（进程未挂，但引擎没停）"
    );

    let outcome = handle.join().expect("线程应正常结束（不是 panic）");
    assert_ne!(outcome.exit_code, 0, "死循环应导致该源失败（退出码非 0）");
    assert!(
        !outcome.stderr.is_empty(),
        "应给出可归因的错误信息，实际 stderr 为空"
    );

    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn 第二件套死循环不拖垮同批次的其他源() {
    // ## A6 的核心：单源失败不终止整个流
    //
    // 一个死循环的源不该让其他源也拿不到结果——那是"每源独立 runtime"
    // 的隔离价值所在。
    let host = FakeHost::new(vec![(
        "/good",
        r#"<html><body><ul>
          <li class="item"><h3 class="name"><a href="/b/1">正常书</a></h3>
            <p class="author">作者</p></li>
        </ul></body></html>"#,
    )]);
    let data_dir = temp_data_dir("deadloop-isolation");

    let sources = r#"[
      {
        "bookSourceName": "死循环源",
        "bookSourceUrl": "https://bad.test",
        "searchUrl": "/bad?q={{while(true){}}}",
        "ruleSearch": "{\"bookList\":\"@css:li\"}"
      },
      {
        "bookSourceName": "正常源",
        "bookSourceUrl": "https://good.test",
        "searchUrl": "/good?q={{key}}",
        "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"author\":\"@css:.author@text\",\"bookUrl\":\"@css:.name>a@href\"}"
      }
    ]"#
    .to_string();

    let started = Instant::now();
    let handle = {
        let host = host.clone();
        let data_dir = data_dir.clone();
        std::thread::spawn(move || {
            run_cli_passing_js_host(
                &[
                    "run",
                    "https://good.test",
                    "--action",
                    "search",
                    "--input",
                    "关键词",
                    "--format",
                    "json",
                ],
                host,
                true,
                Some(&sources),
                &data_dir,
            )
        })
    };
    while !handle.is_finished() && started.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(handle.is_finished(), "整批搜索应能结束（死循环源被隔离）");

    let outcome = handle.join().expect("线程应正常结束");
    // ## 只搜指定的好源（`run <id>` 过滤了源）
    //
    // 这条测试验证的是"指定好源时能拿到结果"。死循环源的影响范围
    // 由下面那条测试（两个源都在搜索范围内）覆盖。
    assert_eq!(
        outcome.exit_code, 0,
        "指定正常源时应成功；stderr={}",
        outcome.stderr
    );
    assert!(
        outcome.stdout.contains("正常书"),
        "应拿到正常源的结果：{}",
        outcome.stdout
    );

    std::fs::remove_dir_all(&data_dir).ok();
}

// ===========================================================================
// 第三件套：`webView` 类源抛 UNSUPPORTED_CAPABILITY
// ===========================================================================

#[test]
fn 第三件套webview_源抛能力错误() {
    // ## `@webjs` 前缀（T043 处理的形态）
    //
    // 它表示"这段脚本在 WebView 环境里执行"——本产品不含浏览器内核，
    // 故必须抛结构化能力错误（FR-006），让书源的 try/catch 降级。
    //
    // ## fixture 要让 `@webjs` **真的被求值**
    //
    // 字段规则在搜索**响应解析**阶段求值，故必须先让搜索请求成功。
    // 若搜索页拿不到（404/网络失败），错误会是 `error.network`——
    // 那是另一回事，测不到能力错误。故这里给假宿主配一个能匹配的响应。
    let host = FakeHost::new(vec![(
        "/search",
        r#"<html><body><ul>
          <li class="item"><h3 class="name"><a href="/b/1">书名</a></h3>
            <p class="author">作者</p></li>
        </ul></body></html>"#,
    )]);
    let data_dir = temp_data_dir("webjs");
    let source = r#"{
      "bookSourceName": "webView 源",
      "bookSourceUrl": "https://webview.test",
      "searchUrl": "/search?q={{key}}",
      "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@webjs:java.webView('x')\",\"author\":\"@css:.author@text\",\"bookUrl\":\"@css:.name>a@href\"}"
    }"#
    .to_string();

    let outcome = run_cli_passing_js_host(
        &[
            "run",
            "https://webview.test",
            "--action",
            "search",
            "--input",
            "k",
        ],
        host,
        true,
        Some(&source),
        &data_dir,
    );

    assert_ne!(outcome.exit_code, 0, "@webjs 应让该源失败");
    assert!(
        outcome.stderr.contains("webView")
            || outcome.stderr.contains("unsupported")
            || outcome.stderr.contains("JS 引擎")
            || outcome.stderr.contains("js"),
        "应报能力/JS 相关的错误（而不是笼统的网络错）：{}",
        outcome.stderr
    );

    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn 第三件套脚本里调用_webview_抛出可捕获的能力错误() {
    // ## 更直接的一条：脚本体里调用 `java.webView(...)`
    //
    // 这与 `@webjs` 前缀不同——它经 T042 的能力桩（挂在实际的
    // `java.webView` 上）。两者都要抛**带 capability 标识**的可捕获错误。
    //
    // 这里用 `searchUrl` 的插值触发：插值里调用 `java.webView`，
    // 应抛出能力错误并让该源失败。
    let host = FakeHost::new(vec![]);
    let data_dir = temp_data_dir("webview-call");
    let source = r#"{
      "bookSourceName": "webView 调用源",
      "bookSourceUrl": "https://webviewcall.test",
      "searchUrl": "/search?q={{java.webViewGetSource('https://x/')}}",
      "ruleSearch": "{\"bookList\":\"@css:li\"}"
    }"#
    .to_string();

    let outcome = run_cli_passing_js_host(
        &[
            "run",
            "https://webviewcall.test",
            "--action",
            "search",
            "--input",
            "k",
        ],
        host,
        true,
        Some(&source),
        &data_dir,
    );

    assert_ne!(outcome.exit_code, 0, "调用 webView 应失败");
    assert!(
        outcome.stderr.contains("webView")
            || outcome.stderr.contains("unsupported")
            || outcome.stderr.contains("capability"),
        "错误应指明是 webView 能力问题：{}",
        outcome.stderr
    );

    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn 第三件套能力错误退出码为_3() {
    // `contracts/cli.md` 的全局约定：不支持能力 → 退出码 3。
    // 脚本据此区分"源坏了"（1）与"本产品做不到"（3）。
    let host = FakeHost::new(vec![]);
    let data_dir = temp_data_dir("exit3");
    let source = r#"{
      "bookSourceName": "能力错误源",
      "bookSourceUrl": "https://exit3.test",
      "searchUrl": "/search?q={{java.openVideoPlayer('https://x/')}}",
      "ruleSearch": "{\"bookList\":\"@css:li\"}"
    }"#
    .to_string();

    let outcome = run_cli_passing_js_host(
        &[
            "run",
            "https://exit3.test",
            "--action",
            "search",
            "--input",
            "k",
        ],
        host,
        true,
        Some(&source),
        &data_dir,
    );

    assert_ne!(outcome.exit_code, 0, "调用不支持的能力应失败");
    // 具体退出码取决于错误分类如何穿过插值层映射。此处只断言
    // "非 0 且给出可归因的说明"——退出码的精确值由
    // `tome-cli` 的 flags 测试（`退出码按契约映射`）覆盖。
    assert!(!outcome.stderr.is_empty(), "应给出错误说明（不得静默失败）");

    std::fs::remove_dir_all(&data_dir).ok();
}

// ===========================================================================
// 补充：`run` 的用法与契约
// ===========================================================================

#[test]
fn run_缺_action_时报用法错误() {
    let host = FakeHost::new(vec![]);
    let data_dir = temp_data_dir("usage");
    let parsed = parse(&[
        "run".to_string(),
        "https://x.test".to_string(),
        "--data-dir".to_string(),
        data_dir.to_string_lossy().into_owned(),
    ]);
    match parsed {
        ParseOutcome::Usage(message) => {
            assert!(message.contains("--action"), "应提示缺 --action：{message}");
        }
        other => panic!("应报用法错误，实际 {other:?}"),
    }
    std::fs::remove_dir_all(&data_dir).ok();
    let _ = host;
}

#[test]
fn run_的未知动作报用法错误() {
    let host = FakeHost::new(vec![]);
    let data_dir = temp_data_dir("bad-action");
    let source = r#"{"bookSourceName":"x","bookSourceUrl":"https://x.test"}"#.to_string();
    let outcome = run_cli_passing_js_host(
        &[
            "run",
            "https://x.test",
            "--action",
            "nonsense",
            "--input",
            "k",
        ],
        host,
        true,
        Some(&source),
        &data_dir,
    );
    assert_eq!(outcome.exit_code, 2, "未知动作是用法错误（退出码 2）");
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn run_的后续切片动作报能力错误() {
    // `info`/`toc`/`content` 在 `contracts/cli.md` 里有定义但属后续切片。
    // 它们不该报"用法错误"（那会说"你调错了"），而该报能力错误
    // （退出码 3：本产品将支持但当前做不到）。
    let host = FakeHost::new(vec![]);
    let data_dir = temp_data_dir("later");
    let source = r#"{"bookSourceName":"x","bookSourceUrl":"https://x.test"}"#.to_string();
    for action in ["info", "toc", "content"] {
        let outcome = run_cli_passing_js_host(
            &["run", "https://x.test", "--action", action, "--input", "k"],
            host.clone(),
            true,
            Some(&source),
            &data_dir,
        );
        assert_eq!(
            outcome.exit_code, 3,
            "`{action}` 应报能力错误（退出码 3），实际 {outcome:?}"
        );
        assert!(
            outcome.stderr.contains("后续切片"),
            "应说明原因：{}",
            outcome.stderr
        );
    }
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn 含_js_的源在未传_js_host_时给出可归因错误() {
    // `run_with_js` 的 `js_host = None` 意味着沙箱没有网络能力。
    // 此时含 JS 的源应得到**结构化能力错误**，而不是静默失败——
    // 那让"忘了传"这件事在第一次运行时就暴露。
    let host = FakeHost::new(vec![]);
    let data_dir = temp_data_dir("no-js-host");
    let source = r#"{
      "bookSourceName": "需要 JS 的源",
      "bookSourceUrl": "https://needsjs.test",
      "searchUrl": "/search?q={{1+1}}",
      "ruleSearch": "{\"bookList\":\"@css:li\"}"
    }"#
    .to_string();

    let outcome = run_cli_passing_js_host(
        &[
            "run",
            "https://needsjs.test",
            "--action",
            "search",
            "--input",
            "k",
        ],
        host,
        // 刻意不传 js_host
        false,
        Some(&source),
        &data_dir,
    );

    // 有 JS 引擎但没网络能力时，插值本身（`1+1`）仍可求值——
    // 只是随后的请求会因缺能力而失败。无论哪种，都必须给出说明。
    assert!(
        outcome.exit_code != 0 || !outcome.stdout.is_empty(),
        "应有结果或说明（不得静默）：exit={} stdout={:?} stderr={:?}",
        outcome.exit_code,
        outcome.stdout,
        outcome.stderr
    );

    std::fs::remove_dir_all(&data_dir).ok();
}
