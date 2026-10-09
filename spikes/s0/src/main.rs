//! # S0 可行性 spike —— 一次性技术栈验证骨架
//!
//! 对应 `tasks.md` T006-T008。目的：用**最小**代码验证三平台技术栈可行
//! （die 早 die 便宜），并产出体积/启动耗时/DOM 差异率三项基线数据。
//!
//! ## 验证项
//!
//! | 项 | 落点 | 对应需求 |
//! |---|---|---|
//! | eframe 开窗口 | [`window`] | FR-040 三平台构建运行 |
//! | rusqlite 一次写读 | [`storage`] | FR-026 持久化 |
//! | reqwest 一次 HTTP（**用户触发**） | [`network`] | FR-041/SC-012 无隐式外发 |
//! | rquickjs 一次 eval | [`scripting`] | FR-007 脚本沙箱 |
//! | Jsoup↔html5ever DOM 差异 | `dom_diff` 子命令 | research.md R2 遗留待办 |
//!
//! ## 用法（无界面可跑，便于 CI）
//!
//! ```text
//! tome-s0-spike --headless          # 跑存储/脚本/无网络检查，不开窗口
//! tome-s0-spike --http              # 显式发起一次 HTTP（用户触发才算数）
//! tome-s0-spike                     # 开窗口（需图形环境）
//! tome-s0-spike dom-diff <目录>      # DOM 差异量化（T008）
//! ```
//!
//! ## 本 crate 的代码标准
//!
//! 验证性一次性代码，但**编译警告必须为零**，且不写 `unwrap()`——spike 也要
//! 能区分"验证失败"与"程序崩溃"，否则数据不可信。

mod dom_diff;
mod network;
mod scripting;
mod storage;
mod window;

use std::process::ExitCode;

/// spike 的临时数据目录。**刻意不写用户数据目录**：spike 是一次性的，
/// 不该在用户真实数据位置留下任何东西。落在系统临时目录，由调用方清理。
const SPIKE_TEMP_DIR_NAME: &str = "tome-s0-spike";

/// 主入口。子命令形态而非 clap：spike 不引入多余依赖，手写解析足够。
fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let command: &str = arguments.first().map(String::as_str).unwrap_or("");

    let outcome = match command {
        "--headless" => run_headless(),
        "--http" => run_explicit_http(),
        "dom-diff" => match arguments.get(1) {
            Some(directory) => dom_diff::run(directory),
            None => Err(anyhow_lite("dom-diff 需要一个语料目录参数")),
        },
        // 调试入口：直接打印 html5ever 侧的序列化，供人工与 Jsoup 产物并排核对。
        // 必要性：两侧输出的"约定不同"与"解析不同"必须能区分开，否则差异率
        // 会变成一个无法解释的数字（本次 spike 正是靠它定位到深度错位与空文本节点）。
        "dom-dump" => match arguments.get(1) {
            Some(file) => dom_diff::dump_file(file),
            None => Err(anyhow_lite("dom-dump 需要一个 HTML 文件参数")),
        },
        "--help" | "-h" => {
            print_usage();
            Ok(())
        }
        "" => window::open(),
        unknown => Err(anyhow_lite(&format!(
            "未知子命令：{unknown}（用 --help 查看用法）"
        ))),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("[spike] 失败：{message}");
            ExitCode::FAILURE
        }
    }
}

/// 无界面路径：不起窗口，依次验证存储与脚本。
///
/// 这条路径是 CI 用的主路径——Linux runner 无 X11/Wayland，开窗口会直接失败，
/// 而"三平台构建通过"的检查不该因此被卡住。窗口验证留给有图形环境的机器。
fn run_headless() -> Result<(), String> {
    println!("[spike] headless 模式：不开窗口");

    let store = storage::verify_once()?;
    println!("[spike] rusqlite OK：{store}");

    // 脚本验证仅在启用 spike-js feature 时执行——理由见 Cargo.toml 的
    // [features] 注释（默认臂要能在没有 rquickjs 的情况下独立验证）。
    if cfg!(feature = "spike-js") {
        let evaluated = scripting::verify_once()?;
        println!("[spike] rquickjs OK：eval 结果 = {evaluated}");
    } else {
        println!("[spike] rquickjs 未编译（未启用 spike-js feature）");
    }

    println!("[spike] headless 全部通过；本次运行未发起任何网络请求");
    Ok(())
}

/// 显式 HTTP 验证。**必须由用户显式触发**（FR-041/SC-012）——
/// 故它是独立子命令，绝不挂在 headless 路径上。
fn run_explicit_http() -> Result<(), String> {
    let summary = network::fetch_once("https://example.com/")?;
    println!("[spike] reqwest OK：{summary}");
    Ok(())
}

fn print_usage() {
    println!("tome-s0-spike —— S0 技术栈可行性验证骨架");
    println!();
    println!("用法：");
    println!("  --headless           无界面验证存储/脚本（CI 路径，无网络外发）");
    println!("  --http               显式发起一次 HTTP 请求（用户触发）");
    println!("  dom-diff <目录>      对比 Jsoup 与 html5ever 的 DOM 序列化");
    println!("  (无参数)             开窗口（图形环境验证）");
}

/// 极简错误构造。
///
/// 刻意**不引入 anyhow/thiserror**：spike 的错误只需打印，引入依赖会污染
/// 体积对照的基线（T007 的差值必须是"rquickjs 的代价"，不能掺入别的）。
fn anyhow_lite(message: &str) -> String {
    message.to_owned()
}

/// 返回 spike 的临时目录，不存在则创建。
///
/// 用 `temp_dir()` 而非数据目录：spike 不该在用户真实位置留痕。
pub(crate) fn spike_temp_dir() -> Result<std::path::PathBuf, String> {
    let directory = std::env::temp_dir().join(SPIKE_TEMP_DIR_NAME);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("创建临时目录 {directory:?} 失败：{error}"))?;
    Ok(directory)
}
