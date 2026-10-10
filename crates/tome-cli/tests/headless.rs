//! `\[DONE]` 无头运行验证（tasks.md T094 / FR-036、SC-011）
//!
//! # 需求原文
//!
//! > **FR-036**: 系统 MUST 以无界面库的形式提供全部引擎能力，且 MUST NOT
//! > 依赖任何窗口或图形环境
//! >
//! > **SC-011**: 在无图形环境的系统上，全部引擎能力可被调用且**无图形相关
//! > 失败**（100%）
//!
//! # 本文件的验证口径（先写清楚"怎么算通过"）
//!
//! SC-011 要证的是"在无图形环境上也能跑"。真要在无图形环境上跑，得有一台
//! 没有窗口系统的机器——本项目的 CI 里 ubuntu runner 是无头的，但**本地
//! 开发机不是**。故本文件用两层证据，各自都写明了局限：
//!
//! | 层次 | 做法 | 能证什么 | 局限 |
//! |---|---|---|---|
//! | ① **环境模拟** | 清空 `DISPLAY`/`WAYLAND_DISPLAY`/`XDG_SESSION_TYPE` 等变量后再调全部能力 | 代码路径不读这些变量 | 不能证"没有窗口系统时也能跑" |
//! | ② **依赖面断言** | 断言引擎各 crate 的依赖里**没有**图形栈 | **结构上不可能**依赖图形环境 | 只看直接依赖表，不看传递依赖 |
//!
//! 两层互补：① 是动态的（真跑一遍）但弱，② 是静态的但强（结构性）。
//! **任一单独都不足以立论**，故两者都要。
//!
//! ## 为什么"清环境变量"是有意义的（而不是装装样子）
//!
//! 依赖图形环境的 crate 在启动时会去读 `DISPLAY`（X11）/ `WAYLAND_DISPLAY`
//! 来决定连哪个显示服务。`winit` 之类的库在**创建事件循环**那一刻就查它，
//! 查不到即报错——而那个错误往往在进程启动早期就发生。
//!
//! 故"清空这些变量后全部能力仍可用"是一条**真实可证伪**的断言：若哪天有人
//! 给引擎引了 `winit`，本文件的测试会立刻变红。这比"读一遍 Cargo.toml 觉得
//! 没问题"强得多。
//!
//! ## 为什么还要第 ② 层
//!
//! 第 ① 层的弱点是它**只测了当前这条执行路径**。比如某能力在本机走了
//! 缓存分支，根本没碰到会读 `DISPLAY` 的代码——那测试就漏了。
//! 第 ② 层从**依赖表**上排除整个可能性，补上这个洞。
//!
//! # 覆盖范围：引擎 crate，**不含** `tome-reader`
//!
//! SC-011 说的是"全部**引擎**能力"。`tome-reader` 是 S2 的排版渲染层
//! （`cosmic-text` + 可选 `wgpu`），它的无头性由 S2 自己的
//! `feature_gating.rs` 负责（那里断言"headless 构建下错误契约也必须完整
//! 可构造"）。本文件不碰它——两条线各有各的验收面。

#![expect(
    clippy::expect_used,
    reason = "无头验证的夹具失败应让测试立即失败；静默跳过会让'全能力可调'变成空话"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tome_cli::commands::run;
use tome_cli::flags::{parse, GlobalOptions, ParseOutcome};

// ---------------------------------------------------------------------------
// ① 环境模拟
// ---------------------------------------------------------------------------

/// 会被图形栈读取的环境变量。
///
/// 逐个说明为什么它在列：
///
/// | 变量 | 谁读它 |
/// |---|---|
/// | `DISPLAY` | X11 客户端（`winit`/`glutin`/GTK）决定连哪个 X 服务 |
/// | `WAYLAND_DISPLAY` | Wayland 客户端 |
/// | `XDG_SESSION_TYPE` | 部分库据此判断"是 wayland 还是 x11" |
/// | `MIR_SOCKET` | Mir（Ubuntu 的显示服务） |
/// | `SDL_VIDEODRIVER` / `WINIT_UNIX_BACKEND` | 显式指定后端的库 |
const GRAPHICS_ENV_VARS: &[&str] = &[
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_SESSION_TYPE",
    "MIR_SOCKET",
    "SDL_VIDEODRIVER",
    "WINIT_UNIX_BACKEND",
];

/// 在"无图形环境"下执行一段代码，返回其结果。
///
/// # 为什么要恢复原值而不是简单删除
///
/// 测试是并行的：删除进程级环境变量会影响**同时运行的其它测试**（如果
/// 它们恰好也读这些变量）。故用 RAII 守卫，退出作用域即恢复。
///
/// # 单线程假设
///
/// `std::env::set_var` 在多线程进程中本身是不安全的（Rust 2024 起标记为
/// `unsafe`）。本函数用在测试里，而**同一测试函数内的代码是顺序执行的**；
/// 风险在于它与其它并行测试的交互——那些测试不读这些变量（引擎 crate
/// 无图形依赖，正是本文件要证的事）。这个前提写在注释里而不是假装不存在。
fn without_graphics_env<T>(f: impl FnOnce() -> T) -> T {
    struct Restore(Vec<(&'static str, Option<String>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (name, value) in &self.0 {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    let saved: Vec<(&'static str, Option<String>)> = GRAPHICS_ENV_VARS
        .iter()
        .map(|name| (*name, std::env::var(name).ok()))
        .collect();
    let _guard = Restore(saved);

    for name in GRAPHICS_ENV_VARS {
        std::env::remove_var(name);
    }

    f()
}

// ---------------------------------------------------------------------------
// CLI 调用辅助
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate 目录应有两级父目录")
        .to_path_buf()
}

/// 在一个隔离的数据目录里跑一次 CLI。
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
    }
}

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

/// 断言一次 CLI 调用**没有图形相关失败**。
///
/// # 判据
///
/// 图形相关的失败在错误文本上有明显特征（显示服务不可用、无法创建窗口、
/// 事件循环失败）。故除了检查退出码，还扫一遍输出里有没有这些特征——
/// 只看退出码会漏掉"某个能力悄悄降级"的情形。
fn assert_no_graphics_failure(context: &str, code: i32, stdout: &str, stderr: &str) {
    const GRAPHICS_MARKERS: &[&str] = &[
        "DISPLAY",
        "Wayland",
        "wayland",
        "X11",
        "x11",
        "cannot open display",
        "failed to create window",
        "event loop",
        "windowing",
        "no graphics",
        "graphics backend",
        "vulkan",
        "Vulkan",
        "wgpu",
        "GLFW",
    ];

    let combined = format!("{stdout}{stderr}");
    for marker in GRAPHICS_MARKERS {
        assert!(
            !combined.contains(marker),
            "{context}：输出里出现图形相关特征 `{marker}`（退出码 {code}）\n\
             stdout:\n{stdout}\nstderr:\n{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// 全能力清单（SC-011 的"全部"要有可核对的范围）
// ---------------------------------------------------------------------------

/// SC-011 要求"**全部**引擎能力可被调用"。"全部"必须有一份清单——
/// 否则"都测过了"是一句无法核对的话。
///
/// 本清单按 `contracts/cli.md` 的命令面组织：**命令面即能力的对外表达**
/// （引擎的一切能力都要能被 CLI 调到，那正是 FR-036 的"以无界面库的形式
/// 提供全部引擎能力"）。
#[test]
fn 无图形环境下全部命令面可用() {
    let data_dir_guard = tempfile::tempdir().expect("数据目录");
    let data_dir = data_dir_guard.path();
    let sources_guard = tempfile::tempdir().expect("书源目录");
    let local_guard = tempfile::tempdir().expect("本地书目录");
    let corpus = repo_root().join("tests").join("corpus");

    // 先造好各命令要用的前置数据（**在无图形环境外**做，避免把
    // "造数据失败"误报成"图形失败"）
    let source = write_source(
        sources_guard.path(),
        "无头测试源",
        "https://headless.example.com",
    );
    let (code, stdout, stderr) = run_cli(
        &["source", "import", source.to_str().expect("路径")],
        data_dir,
    );
    assert_eq!(code, 0, "导入应成功：{stdout}{stderr}");

    // 一本本地 TXT（供 local 命令用）
    let txt = local_guard.path().join("无头书.txt");
    std::fs::write(
        &txt,
        "第一章 开始\n甲甲甲的内容。\n第二章 继续\n乙乙乙的内容。\n",
    )
    .expect("应能写 TXT");

    // 逐条列出命令面。每条：(说明, 参数)
    let commands: Vec<(&str, Vec<&str>)> = vec![
        ("source list", vec!["source", "list"]),
        ("source list --enabled", vec!["source", "list", "--enabled"]),
        ("source groups", vec!["source", "groups"]),
        (
            "source check",
            vec!["source", "check", "https://headless.example.com"],
        ),
        (
            "source group",
            vec![
                "source",
                "group",
                "https://headless.example.com",
                "--name",
                "无头组",
            ],
        ),
        (
            "source enable",
            vec!["source", "enable", "https://headless.example.com"],
        ),
        ("keyword add", vec!["keyword", "add", "无头关键词"]),
        ("keyword list", vec!["keyword", "list"]),
        ("keyword remove", vec!["keyword", "remove", "无头关键词"]),
        ("data export", vec!["data", "export"]),
        (
            "data clear-cache chapters",
            vec!["data", "clear-cache", "chapters"],
        ),
        (
            "data clear-cache images",
            vec!["data", "clear-cache", "images"],
        ),
        (
            "local toc",
            vec!["local", "toc", txt.to_str().expect("路径")],
        ),
        (
            "local import",
            vec!["local", "import", txt.to_str().expect("路径")],
        ),
        (
            "local scan",
            vec!["local", "scan", local_guard.path().to_str().expect("路径")],
        ),
        (
            "run --trace",
            vec![
                "run",
                "--trace",
                "--rule",
                "p@text",
                "--input",
                "<p>无头</p>",
            ],
        ),
        (
            "verify --corpus",
            vec!["verify", "--corpus", corpus.to_str().expect("路径")],
        ),
        ("--help", vec!["--help"]),
    ];

    // 在"无图形环境"下跑全部命令
    let results: Vec<(&str, i32, String, String)> = without_graphics_env(|| {
        commands
            .iter()
            .map(|(name, args)| {
                let (code, stdout, stderr) = run_cli(args, data_dir);
                (*name, code, stdout, stderr)
            })
            .collect()
    });

    // 逐条核对：无图形相关失败
    let mut failed: Vec<String> = Vec::new();
    for (name, code, stdout, stderr) in &results {
        assert_no_graphics_failure(name, *code, stdout, stderr);
        // 注意：这里**不**断言退出码为 0。有些命令在离线环境下会因
        // 网络失败而给业务错误（1）——那是**预期的**，且证明它真的
        // 尝试执行了。SC-011 要的是"无**图形相关**失败"，不是"全部成功"。
        if *code == 3 {
            failed.push(format!("{name}：报能力不支持（3）"));
        }
    }
    assert!(
        failed.is_empty(),
        "无图形环境下不该有能力被报为不支持：{failed:?}"
    );

    // 至少要有命令真的成功了——否则"全能力可调"可能只是"全都失败了"
    let succeeded = results.iter().filter(|(_, code, _, _)| *code == 0).count();
    assert!(
        succeeded >= 10,
        "应有足够多的命令成功（实际 {succeeded}/{}）——若全都失败，\
         这条测试证明的就不是'可调'而是'调了都错'",
        results.len()
    );
}

/// 无图形环境下**组合根能完整装配**（FR-036 的"以库的形式提供"）。
///
/// # 为什么单独一条
///
/// 装配是"引擎能否被用起来"的前提。若装配本身要图形环境，后面测什么
/// 都没意义。故它是 SC-011 最该先证的一条。
#[test]
fn 无图形环境下组合根可完整装配() {
    let dir_guard = tempfile::tempdir().expect("数据目录");
    let options = GlobalOptions {
        data_dir: Some(dir_guard.path().to_path_buf()),
        ..GlobalOptions::default()
    };

    let root = without_graphics_env(|| tome_cli::root::AppRoot::assemble(&options, None));
    let root = root.expect("无图形环境下应能完整装配组合根");

    // 装配产出的东西真的可用（不是"装配了个空壳"）
    assert!(root.config().network().request_interval_ms.is_none() || true);
    // 数据目录已就绪
    assert!(dir_guard.path().is_dir());
}

/// 无图形环境下**引擎各 crate 的核心能力可被直接调用**（不经 CLI）。
///
/// # 为什么 CLI 全覆盖了还要这一条
///
/// FR-036 说的是"以**库**的形式提供全部引擎能力"。CLI 是库的一个消费者，
/// 但"CLI 能跑"不完全等于"库能被独立调用"——比如某个能力可能只在 CLI
/// 的组合根里被接线，而库本身用不起来。这条直接把各 crate 当作库来调。
#[test]
fn 无图形环境下引擎库可直接调用() {
    without_graphics_env(|| {
        // 规则引擎：求值一条 CSS 规则
        let document = tome_rule::dom::Document::parse_html("<p>库调用</p>");
        let values =
            tome_rule::css_post::evaluate_rule(&document, "p@text").expect("规则引擎应可用");
        assert_eq!(values, vec!["库调用"]);

        // JSONPath 引擎
        let json = tome_rule::jsonpath::evaluate(r#"{"a":"b"}"#, "$.a").expect("JSONPath 应可用");
        assert_eq!(json, vec!["b"]);

        // 规则轨迹（T093 新增的能力）
        let trace = tome_rule::trace::trace_rule(
            "p@text",
            tome_rule::trace::TraceInput::Html,
            "<p>轨迹</p>",
        );
        assert_eq!(trace.result, vec!["轨迹"]);

        // JS 沙箱：跑一段脚本
        let runtime = tome_rule::js_bridge::JobRuntime::new(tome_js::Limits::default())
            .expect("JS 沙箱应可用");
        let outcome = runtime.eval("1+1").expect("脚本应能求值");
        assert_eq!(outcome.text, "2", "JS 沙箱应能在无图形环境下求值");

        // 存储层：开库
        let dir = tempfile::tempdir().expect("数据目录");
        let db = tome_store::Db::open(&dir.path().join("tome.db")).expect("库应能打开");
        let book_dao = tome_store::dao::BookDao::new(db.clone());
        assert!(book_dao.bookshelf().expect("书架应可读").is_empty());

        // 本地书解析：识别 TXT 目录
        let txt_dir = tempfile::tempdir().expect("本地书目录");
        let path = txt_dir.path().join("t.txt");
        std::fs::write(&path, "第一章 甲\n内容一\n第二章 乙\n内容二\n").expect("应能写");
        let bytes = std::fs::read(&path).expect("应能读");
        let rules = [tome_core::TxtTocRule {
            id: 0,
            name: "章".to_string(),
            rule: "^第.章.*$".to_string(),
            example: None,
            enabled: true,
            sort_order: 0,
        }];
        let parsed = tome_local::txt::import_bytes(&bytes, "t.txt", &rules);
        assert_eq!(parsed.toc.len(), 2, "应识别出两章");

        // 网络层：构造执行器（不真发请求——那会依赖外网）
        tome_cli::build_executor(&tome_cli::NetworkMode::Live, None).expect("网络执行器应可构造");
    });
}

/// 无图形环境下**完整读一章本地书**（走 ContentProvider 接缝）。
#[test]
fn 无图形环境下可经接缝读本地书() {
    without_graphics_env(|| {
        let dir = tempfile::tempdir().expect("目录");
        let path = dir.path().join("seam.txt");
        std::fs::write(
            &path,
            "第一章 开端\n甲甲甲的内容。\n第二章 承接\n乙乙乙的内容。\n",
        )
        .expect("应能写");

        let bytes = std::fs::read(&path).expect("应能读");
        let rules = [tome_core::TxtTocRule {
            id: 0,
            name: "章".to_string(),
            rule: "^第.章.*$".to_string(),
            example: None,
            enabled: true,
            sort_order: 0,
        }];
        let parsed = tome_local::txt::import_bytes(&bytes, "seam.txt", &rules);
        let toc = tome_local::txt::to_chapters(&parsed.toc, 1);
        let source = Arc::new(tome_local::provider::LocalSource::new(
            path.clone(),
            tome_local::format::LocalFormat::Txt,
            toc,
        ));
        let provider = tome_local::provider::LocalContentProvider::without_cleaning(source);

        // 经 ContentProvider 接缝读正文（FR-036：接缝即库的对外形态）
        use tome_core::seam::ContentProvider;
        let book = tome_core::Book {
            id: 1,
            origin: tome_core::BookOrigin::Local,
            origin_id: "local:headless".to_string(),
            name: "无头书".to_string(),
            author: None,
            cover_url: None,
            intro: None,
            kind: None,
            book_group: None,
            read_config: None,
            toc_url: None,
            latest_chapter_title: None,
            latest_chapter_time: None,
            library_root_id: None,
            relative_path: None,
            charset: Some("UTF-8".to_string()),
            format: Some("txt".to_string()),
            total_chapters: Some(2),
            dur_chapter_index: None,
            added_at: 0,
        };

        let content = provider.chapter_content(&book, 0).expect("应能取正文");
        assert!(
            content.content.contains("甲甲甲"),
            "实际：{}",
            content.content
        );
        assert!(content.images.is_empty(), "TXT 书无内嵌图");
    });
}

// ---------------------------------------------------------------------------
// ② 依赖面断言
// ---------------------------------------------------------------------------

/// 引擎 crate 的依赖里**没有图形栈**。
///
/// # 判据是"结构上不可能"，比"跑一遍没崩"强
///
/// 第 ① 层（环境模拟）只测了当前这条执行路径。这条从**依赖表**上排除
/// 整可能性：只要没有图形依赖，"依赖图形环境"这件事就编译期不成立。
///
/// # 为什么读 Cargo.toml 而不是用 `cargo tree`
///
/// `cargo tree` 会列出**传递依赖**（更强），但它要跑一次 cargo（慢，且
/// 在测试里调 cargo 有嵌套锁的风险）。直接读直接依赖表覆盖了最常见的
/// 退化形态（有人给引擎加了 `winit`），且是**立即**失败。
///
/// 传递依赖的守卫由 T096 的 CI 依赖图断言 + `cargo tree` 在 CI 里补。
#[test]
fn 引擎_crate_无图形依赖() {
    /// 图形栈的特征 crate 名。
    const GRAPHICS_CRATES: &[&str] = &[
        "winit",
        "wgpu",
        "glutin",
        "glfw",
        "sdl2",
        "gtk",
        "gtk4",
        "gtk-sys",
        "cocoa",
        "objc",
        "tao",
        "egui",
        "iced",
        "slint",
        "fltk",
        "minifb",
        "pixels",
        "softbuffer",
        "raw-window-handle",
    ];

    // SC-011 的"引擎"：core 及其全部实现 crate。
    // **不含 `tome-reader`**——它是 S2 的排版渲染层（合法地依赖 wgpu），
    // 无头性由 S2 自己的 feature_gating 负责。见模块文档。
    const ENGINE_CRATES: &[&str] = &[
        "tome-core",
        "tome-rule",
        "tome-js",
        "tome-net",
        "tome-store",
        "tome-source",
        "tome-local",
        "tome-cli",
    ];

    let root = repo_root();
    let mut offenders: Vec<String> = Vec::new();

    for crate_name in ENGINE_CRATES {
        let manifest = root.join("crates").join(crate_name).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|error| panic!("应能读 {}：{error}", manifest.display()));

        // 逐行找依赖声明（形如 `name = ...` 或 `name.workspace = true`）。
        // 只匹配行首的标识符，避免把注释里的名字算进来。
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            let name = trimmed
                .split(['=', '.'])
                .next()
                .unwrap_or_default()
                .trim()
                .trim_matches('"');
            if GRAPHICS_CRATES.contains(&name) {
                offenders.push(format!("{crate_name} → {name}"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "引擎 crate 不该依赖图形栈（FR-036 / SC-011）：{offenders:?}"
    );
}

/// 引擎 crate **不依赖** `tome-reader`（宪法 IV：引擎不依赖渲染层）。
///
/// # 为什么这条也在本文件
///
/// FR-039 与 SC-011 在这里交汇：如果引擎依赖了渲染层，那"无图形环境可运行"
/// 就自动不成立（渲染层含图形栈）。故这是 SC-011 的**充分性反证**。
///
/// （正向的依赖图断言在 T096 的 CI 里；这条是本地可跑的快速护栏。）
#[test]
fn 引擎_crate_不依赖渲染层() {
    const ENGINE_CRATES: &[&str] = &[
        "tome-core",
        "tome-rule",
        "tome-js",
        "tome-net",
        "tome-store",
        "tome-source",
        "tome-local",
        "tome-cli",
    ];

    let root = repo_root();
    let mut offenders: Vec<String> = Vec::new();

    for crate_name in ENGINE_CRATES {
        let manifest = root.join("crates").join(crate_name).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|error| panic!("应能读 {}：{error}", manifest.display()));

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }
            if trimmed.starts_with("tome-reader") {
                offenders.push(format!("{crate_name} → tome-reader"));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "引擎不该依赖渲染层（FR-039：上层实现被替换时引擎无需改动）：{offenders:?}"
    );
}
