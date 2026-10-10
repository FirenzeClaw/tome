//! \[DONE] `tome` 二进制入口（tasks.md T028 / T063 / `contracts/cli.md`）。
//!
//! # 薄壳：一切逻辑在库侧
//!
//! 本文件只做四件事：收集参数 → 解析 → **装配组合根** → 执行 → 写输出并退出。
//! 逻辑放在 `lib.rs` 的模块里，**唯一的理由是它可被测试断言**——
//! T029 的验收项"录制→离线重放输出逐字节一致"要求输出可被断言，
//! 而二进制进程的输出断言要启动子进程（慢且脆）。
//!
//! # 组合根的位置（T063）
//!
//! 装配发生在 [`tome_cli::root::AppRoot::assemble`]，**不在本文件**。
//! 本文件只是"在正确的时候调用它"：
//!
//! ```text
//!   解析参数  →  AppRoot::assemble（读配置、建库、建网络、接线）
//!             →  run_with_js（把真实 NetHost/StoreHost 交给沙箱）
//!             →  写 stdout/stderr  →  退出
//! ```
//!
//! 之所以装配不在 `main.rs`：`main.rs` 是**不可测试**的（要跑它就得启动
//! 进程）。把装配放进库侧让"无界面下可完整装配"（SC-011）成为一条
//! 普通的单元测试，而不是一条要起子进程的集成测试。
//!
//! # 退出码
//!
//! 由 [`tome_cli::exit_code_for`] 统一映射（`contracts/cli.md`：0/1/2/3）。
//! 本文件不自行决定任何码。

use std::io::Write;

use tome_cli::commands::{run_with_js, HostCapabilities};
use tome_cli::flags::{parse, GlobalOptions, ParseOutcome};

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
        ParseOutcome::Run { options, command } => execute(&options, &command),
    }
}

/// 装配组合根并执行命令。
///
/// # `verify` 为什么要先于装配
///
/// 差分语料校验（`verify --corpus`）是**纯计算**：它读语料文件、跑规则引擎、
/// 比对照期望值。它既不发请求也不读数据库，故**不需要**组合根装配。
///
/// 若先装配再分派，一个只想跑语料校验的用户会因为"数据目录不可写"而失败
/// ——那与他要做的事毫无关系。把不需要装配的命令提前分派，让"跑校验"
/// 这件事在任何环境下都成立（这也符合 FR-036 的取向：引擎能力应可被
/// 单独调用）。
fn execute(options: &GlobalOptions, command: &tome_cli::Command) -> i32 {
    if matches!(command, tome_cli::Command::Verify { .. }) {
        // 无需宿主能力的命令：用一个"什么都没有"的宿主走同一条执行路径
        // （不另开分支，避免两条路径的行为漂移）。
        let host = tome_net::RequestExecutor::builder()
            .build()
            .map(|executor| {
                std::sync::Arc::new(executor) as std::sync::Arc<dyn tome_core::NetHost>
            });
        let Ok(host) = host else {
            eprintln!("error.configuration 无法构造网络执行器");
            return 1;
        };
        let mut stdin = std::io::stdin().lock();
        let outcome = run_with_js(
            options,
            command,
            host.as_ref(),
            HostCapabilities::none(),
            &mut stdin,
        );
        return report(outcome);
    }

    // 组合根装配：读 config.toml、解析数据目录、建库、建网络、接线（T063）
    // `--config <path>` 走这里（此前恒传 None，等于该 flag 没有效果——
    // T093 的契约对照发现并修掉）
    let root = match tome_cli::root::AppRoot::assemble(options, options.config.as_deref()) {
        Ok(root) => root,
        Err(kind) => {
            // FR-043：数据目录不可用等要在启动期明确报告（`AppRoot::assemble`
            // 的 `ErrorKind` 已带诊断细节）而不是静默失败
            eprintln!("{kind}");
            return tome_cli::exit_code_for(&kind);
        }
    };

    // 启动期告警（磁盘空间偏低等）：报到 stderr，不干扰 stdout 的结构化输出
    for warning in root.warnings() {
        eprintln!("warning: {warning}");
    }

    // 注入宿主能力：网络 + 存储（A1）。
    //
    // 存储以**构造器**形式注入而不是一个实例——`SqliteStoreHost` 绑定
    // 单个书源，而一次运行要跑多个源（见 `HostCapabilities` 的说明）。
    let hosts = HostCapabilities {
        net: Some(root.net()),
        store_factory: Some(root.store_factory()),
    };

    let net = root.net();
    let mut stdin = std::io::stdin().lock();
    let outcome = run_with_js(options, command, net.as_ref(), hosts, &mut stdin);
    report(outcome)
}

/// 写输出并返回退出码（stdout 与 stderr 分开，保持结构化输出可被管道消费）。
fn report(outcome: tome_cli::CommandOutcome) -> i32 {
    if !outcome.stdout.is_empty() {
        print!("{}", outcome.stdout);
    }
    if !outcome.stderr.is_empty() {
        eprint!("{}", outcome.stderr);
    }
    outcome.exit_code
}
