//! T065-T072 的 CLI 接线端到端验证（从 `run_with_js` 公开入口驱动）
//!
//! # 这一层验的是什么
//!
//! `pipeline_commands.rs` 里的渲染函数有各自的单元测试，各管线也有
//! `tome-source` 内的测试。本文件验的是**接线本身**：参数解析 → 书源
//! 从库里取出 → 管线跑通 → 输出格式符合 `contracts/cli.md`。
//!
//! 之所以必要：接线错误（命令名对不上、`HostCapabilities` 没传、
//! 输出字段名与契约不符）不会在任何单元测试里露头——它们只在这一层。
//!
//! # 数据目录一律临时
//!
//! 不隔离的话书源会落到工作目录，测试之间互相污染，也会弄脏工作树。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::sync::{Arc, Mutex};

use tome_cli::commands::{run_with_js, HostCapabilities};
use tome_cli::flags::{parse, ParseOutcome};
use tome_core::{ErrorKind, HeaderMap, NetHost, RequestSpec, Response};

/// 详情页/目录页/正文页三段式站点。
const BOOK_PAGE: &str = r#"<html><body>
  <h1 class="title">斗破苍穹</h1>
  <span class="author">天蚕土豆</span>
  <span class="kind">玄幻</span>
  <div class="intro">少年萧炎的故事</div>
  <img class="cover" src="/img/cover.jpg"/>
  <a class="toclink" href="/book/1">目录</a>
  <ul id="toc">
    <li><a href="/c/1">第一章 陨落的天才</a></li>
    <li><a href="/c/2">第二章 斗之气</a></li>
  </ul>
</body></html>"#;

const CHAPTER_1: &str = r#"<html><body>
  <div class="content">
    &nbsp;&nbsp;&nbsp;&nbsp;“斗之力，三段！”
    <br/>望着测验魔石碑上面闪亮得甚至有些刺眼的五个大字，少年面无表情。
  </div>
</body></html>"#;

/// 记录请求并按 URL 片段返回预设响应的假宿主。
struct FakeHost {
    responses: Vec<(String, String)>,
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

/// 断言某个 URL 片段确实被请求过（证明命令真的走通了管线）。
#[allow(dead_code)]
fn assert_requested(host: &FakeHost, fragment: &str) {
    let urls: Vec<String> = host.seen().into_iter().map(|spec| spec.url).collect();
    assert!(
        urls.iter().any(|url| url.contains(fragment)),
        "应请求过含 `{fragment}` 的地址，实际请求：{urls:?}"
    );
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

/// 一段可用的合成书源（覆盖搜索/详情/目录/正文四类规则）。
///
/// 搜索规则也在：`source check` 判"可用"的口径是**能跑通一次搜索**
/// （见 `tome-source/src/check.rs`），缺它会被判不可用。
const SOURCE_JSON: &str = r#"{
  "bookSourceName": "合成源",
  "bookSourceUrl": "https://fixture.test",
  "searchUrl": "/search?q={{key}}",
  "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"author\":\"@css:.author@text\",\"bookUrl\":\"@css:.name>a@href\"}",
  "ruleBookInfo": "{\"name\":\"@css:h1.title@text\",\"author\":\"@css:.author@text\",\"kind\":\"@css:.kind@text\",\"intro\":\"@css:.intro@text\",\"coverUrl\":\"@css:.cover@src\",\"tocUrl\":\"@css:.toclink@href\"}",
  "ruleToc": "{\"chapterList\":\"@css:#toc li\",\"chapterName\":\"@css:a@text\",\"chapterUrl\":\"@css:a@href\"}",
  "ruleContent": "{\"content\":\"@css:.content@text\"}"
}"#;

/// 搜索结果页（`source check` 会打这个地址）。
const SEARCH_PAGE: &str = r#"<html><body><ul>
  <li class="item"><h3 class="name"><a href="/book/1">斗破苍穹</a></h3>
    <p class="author">天蚕土豆</p></li>
</ul></body></html>"#;

/// 跑一次 CLI（含把合成书源导入临时库）。
///
/// 第二个返回值是假宿主，供调用方断言"命令确实打了哪些请求"。
fn run_cli(args: &[&str], tag: &str) -> (i32, String, String, std::path::PathBuf, Arc<FakeHost>) {
    let host = FakeHost::new(vec![
        ("/search", SEARCH_PAGE),
        ("/book/1", BOOK_PAGE),
        ("/c/1", CHAPTER_1),
        ("/c/2", CHAPTER_1),
    ]);
    let data_dir = temp_data_dir(tag);

    let import_args = vec![
        "source".to_string(),
        "import".to_string(),
        "--stdin".to_string(),
        "--data-dir".to_string(),
        data_dir.to_string_lossy().into_owned(),
    ];
    let import = run_with_js(
        &options(&data_dir),
        &parse_with_options(&import_args).1,
        host.as_ref(),
        HostCapabilities::none(),
        &mut std::io::Cursor::new(SOURCE_JSON.as_bytes().to_vec()),
    );
    assert_eq!(import.exit_code, 0, "导入书源应成功：{}", import.stderr);

    let mut items: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
    items.push("--data-dir".to_string());
    items.push(data_dir.to_string_lossy().into_owned());

    let hosts = HostCapabilities {
        net: Some(host.clone() as Arc<dyn NetHost>),
        store_factory: None,
    };
    // `--format` 等全局选项由解析器带出，必须以**解析结果**为准驱动
    // ——手搓一个 `GlobalOptions` 会让"选项到底有没有被认出来"这件事
    // 在测试里被绕过（真实调用路径是解析器给的）。
    let (parsed_options, command) = parse_with_options(&items);
    let outcome = run_with_js(
        &parsed_options,
        &command,
        host.as_ref(),
        hosts,
        &mut std::io::Cursor::new(Vec::new()),
    );
    (
        outcome.exit_code,
        outcome.stdout,
        outcome.stderr,
        data_dir,
        host,
    )
}

fn options(data_dir: &std::path::Path) -> tome_cli::flags::GlobalOptions {
    tome_cli::flags::GlobalOptions {
        data_dir: Some(data_dir.to_path_buf()),
        ..Default::default()
    }
}

/// 解析并同时取出选项（`--format` 等全局选项在 `Run` 分支里）。
fn parse_with_options(
    args: &[String],
) -> (tome_cli::flags::GlobalOptions, tome_cli::flags::Command) {
    match parse(args) {
        ParseOutcome::Run { options, command } => (options, command),
        other => panic!("参数应解析成功，实际 {other:?}"),
    }
}

fn temp_data_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tome-pipeline-{tag}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("应能建临时目录");
    dir
}

// ===========================================================================
// info（T065）
// ===========================================================================

#[test]
fn info_输出契约字段() {
    let (exit_code, stdout, stderr, data_dir, host) = run_cli(
        &[
            "info",
            "https://fixture.test",
            "https://fixture.test/book/1",
        ],
        "info",
    );
    assert_eq!(exit_code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    // contracts/cli.md：`{name, author, intro, cover_url, toc_url, ...}`
    assert_eq!(value["name"], "斗破苍穹");
    assert_eq!(value["author"], "天蚕土豆");
    assert_eq!(value["intro"], "少年萧炎的故事");
    assert_eq!(value["toc_url"], "https://fixture.test/book/1");
    // 封面是相对地址，应已绝对化（否则上层无法直接取图）
    assert_eq!(value["cover_url"], "https://fixture.test/img/cover.jpg");
    assert_requested(&host, "/book/1");
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn info_源不存在时报业务错误() {
    let (exit_code, _, stderr, data_dir, _host) = run_cli(
        &["info", "https://nothing.test", "https://nothing.test/b/1"],
        "info-missing",
    );
    assert_eq!(exit_code, 1, "业务错误退出码应为 1：{stderr}");
    assert!(stderr.contains("source.missing"), "{stderr}");
    std::fs::remove_dir_all(&data_dir).ok();
}

// ===========================================================================
// toc（T066）
// ===========================================================================

#[test]
fn toc_输出契约字段() {
    let (exit_code, stdout, stderr, data_dir, host) = run_cli(
        &["toc", "https://fixture.test", "https://fixture.test/book/1"],
        "toc",
    );
    assert_eq!(exit_code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    let items = value.as_array().expect("应是数组");
    assert_eq!(items.len(), 2, "{stdout}");
    // contracts/cli.md：`[{index, title, url}...]`
    assert_eq!(items[0]["index"], 0);
    assert_eq!(items[0]["title"], "第一章 陨落的天才");
    assert_eq!(items[0]["url"], "https://fixture.test/c/1");
    assert_eq!(items[1]["index"], 1);
    assert_requested(&host, "/book/1");
    std::fs::remove_dir_all(&data_dir).ok();
}

// ===========================================================================
// content（T067）
// ===========================================================================

#[test]
fn content_默认已净化且输出契约字段() {
    let (exit_code, stdout, stderr, data_dir, host) = run_cli(
        &[
            "content",
            "https://fixture.test",
            "https://fixture.test/c/1",
        ],
        "content",
    );
    assert_eq!(exit_code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    let text = value["content"].as_str().expect("应是字符串");
    assert!(text.contains("斗之力，三段！"), "{text}");
    // 上游行为：正文首尾空白与 HTML 实体应被规范化
    assert!(!text.contains("&nbsp;"), "实体应已还原：{text}");
    assert_eq!(
        value["length"].as_u64().expect("应是数字") as usize,
        text.chars().count()
    );
    assert_requested(&host, "/c/1");
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn content_人读形态是纯文本() {
    let (exit_code, stdout, stderr, data_dir, _host) = run_cli(
        &[
            "--format",
            "human",
            "content",
            "https://fixture.test",
            "https://fixture.test/c/1",
        ],
        "content-human",
    );
    assert_eq!(exit_code, 0, "stderr={stderr}");
    // 允许净化告警走 stderr；stdout 必须是纯正文（可直接阅读）
    assert!(stdout.contains("斗之力，三段！"), "{stdout}");
    assert!(!stdout.contains('{'), "人读形态不应是 JSON：{stdout}");
    assert!(!stdout.contains("\"content\""), "{stdout}");
    std::fs::remove_dir_all(&data_dir).ok();
}

// ===========================================================================
// source list / source check（T056 / T071）
// ===========================================================================

#[test]
fn source_list_默认列出全部且不带分组时不过滤() {
    let (exit_code, stdout, stderr, data_dir, _host) = run_cli(&["source", "list"], "source-list");
    assert_eq!(exit_code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    let items = value.as_array().expect("应是数组");
    assert_eq!(items.len(), 1, "{stdout}");
    assert_eq!(items[0]["id"], "https://fixture.test");
    assert_eq!(items[0]["name"], "合成源");
    assert_eq!(items[0]["enabled"], true);
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn source_list_按不存在的分组报配置错误() {
    let (exit_code, _, stderr, data_dir, _host) = run_cli(
        &["source", "list", "--group", "并无此组"],
        "source-list-group",
    );
    assert_eq!(exit_code, 1, "stderr={stderr}");
    assert!(stderr.contains("group.missing"), "{stderr}");
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn source_check_单源给出可判定结论() {
    let (exit_code, stdout, stderr, data_dir, host) =
        run_cli(&["source", "check", "https://fixture.test"], "source-check");
    // 合成源指向的地址在本测试的假宿主里存在，应判为可用
    assert_eq!(exit_code, 0, "stderr={stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(value["total"], 1, "{stdout}");
    assert_eq!(value["available"], 1, "{stdout}");
    assert_eq!(value["results"][0]["verdict"], "available", "{stdout}");
    // 判定口径是"能跑通一次搜索"——故必然打过搜索地址
    assert_requested(&host, "/search");
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn source_check_全部不可用时退出码为非零() {
    // 书源指向一个假宿主没有任何响应的地址 → 检查必失败
    let host = FakeHost::new(vec![]);
    let data_dir = temp_data_dir("source-check-fail");
    let dead_source = r#"{
      "bookSourceName": "死源",
      "bookSourceUrl": "https://dead.test",
      "searchUrl": "/search?q={{key}}",
      "ruleSearch": "{\"bookList\":\"@css:li\"}"
    }"#;
    let import_args = vec![
        "source".to_string(),
        "import".to_string(),
        "--stdin".to_string(),
        "--data-dir".to_string(),
        data_dir.to_string_lossy().into_owned(),
    ];
    run_with_js(
        &options(&data_dir),
        &parse_with_options(&import_args).1,
        host.as_ref(),
        HostCapabilities::none(),
        &mut std::io::Cursor::new(dead_source.as_bytes().to_vec()),
    );

    let mut args: Vec<String> = ["source", "check", "--all", "--data-dir"]
        .iter()
        .map(|item| (*item).to_string())
        .collect();
    args.push(data_dir.to_string_lossy().into_owned());
    let outcome = run_with_js(
        &options(&data_dir),
        &parse_with_options(&args).1,
        host.as_ref(),
        HostCapabilities::none(),
        &mut std::io::Cursor::new(Vec::new()),
    );

    assert_ne!(
        outcome.exit_code, 0,
        "全不可用应给非零退出码：stdout={} stderr={}",
        outcome.stdout, outcome.stderr
    );
    std::fs::remove_dir_all(&data_dir).ok();
}

// ===========================================================================
// 录制/回放与确定性（T029 对**所有**命令的要求）
// ===========================================================================

#[test]
fn 同一输入的两次运行输出逐字节一致() {
    let first = run_cli(
        &["toc", "https://fixture.test", "https://fixture.test/book/1"],
        "repeat-a",
    );
    let second = run_cli(
        &["toc", "https://fixture.test", "https://fixture.test/book/1"],
        "repeat-b",
    );
    assert_eq!(first.1, second.1, "同一输入应逐字节一致");
    std::fs::remove_dir_all(&first.3).ok();
    std::fs::remove_dir_all(&second.3).ok();
}

#[test]
fn 参数解析把管线命令接到正确的分支() {
    // 这层防的是"命令名解析对了但派发到别处"
    let parsed = parse(
        &["info", "https://s.test", "https://s.test/u"]
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>(),
    );
    let ParseOutcome::Run { command, .. } = parsed else {
        panic!("应解析成功");
    };
    assert!(
        matches!(command, tome_cli::flags::Command::Info { .. }),
        "`info` 应解析为 Info 命令：{command:?}"
    );

    let parsed = parse(
        &["toc", "https://s.test", "https://s.test/u"]
            .iter()
            .map(|item| item.to_string())
            .collect::<Vec<_>>(),
    );
    let ParseOutcome::Run { command, .. } = parsed else {
        panic!("应解析成功");
    };
    assert!(
        matches!(command, tome_cli::flags::Command::Toc { .. }),
        "`toc` 应解析为 Toc 命令：{command:?}"
    );
}
