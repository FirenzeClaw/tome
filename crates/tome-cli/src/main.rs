//! `tome` 二进制入口（tasks.md T028 / `contracts/cli.md`）。
//!
//! # 薄壳：一切逻辑在库侧
//!
//! 本文件只做四件事：收集参数 → 解析 → 执行 → 写输出并退出。
//! 逻辑放在 `lib.rs` 的模块里，**唯一的理由是它可被测试断言**——
//! T029 的验收项"录制→离线重放输出逐字节一致"要求输出可被断言，
//! 而二进制进程的输出断言要启动子进程（慢且脆）。
//!
//! # 退出码
//!
//! 由 [`tome_cli::exit_code_for`] 统一映射（`contracts/cli.md`：0/1/2/3）。
//! 本文件不自行决定任何码。

use std::io::Write;

use tome_cli::commands::run;
use tome_cli::flags::{parse, ParseOutcome};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let exit_code = dispatch(&args);
    // `process::exit` 不做栈展开，故先显式 flush——
    // 否则管道/重定向下可能丢掉缓冲里的输出。
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(exit_code);
}

/// 解析并执行，返回退出码。
fn dispatch(args: &[String]) -> i32 {
    match parse(args) {
        ParseOutcome::Help(text) => {
            println!("{text}");
            0
        }
        ParseOutcome::Usage(message) => {
            // 用法错误：说明 + 指向 `--help`（契约：退出码 2）
            eprintln!("用法错误：{message}");
            eprintln!("用 `tome --help` 查看可用命令。");
            2
        }
        ParseOutcome::Run { options, command } => {
            // 组合根：按网络模式装配执行器，作为 `NetHost` 注入管线
            // （A1：宿主能力由组合根注入，管线只认接缝 trait）。
            let executor = match tome_cli::build_executor(&options.network, None) {
                Ok(executor) => executor,
                Err(kind) => {
                    eprintln!("{kind}");
                    return tome_cli::exit_code_for(&kind);
                }
            };

            let mut stdin = std::io::stdin().lock();
            let outcome = run(&options, &command, &executor, &mut stdin);

            if !outcome.stdout.is_empty() {
                print!("{}", outcome.stdout);
            }
            if !outcome.stderr.is_empty() {
                eprint!("{}", outcome.stderr);
            }
            outcome.exit_code
        }
    }
}
