//! T030 的**进程存活**验收：沙箱内的失控脚本不得带走宿主进程
//!
//! ## 为什么必须用子进程来证明
//!
//! "进程不挂"这条断言无法在测试进程内自证：若沙箱真的挂死了测试进程，
//! 测试框架本身也一起消失——测试报告里看到的会是"测试没跑"而不是"失败"，
//! 而那恰恰是最容易被忽略的失败形态（CI 上表现为超时或干脆没有结果）。
//!
//! 故这里用**子进程**验证：父进程（测试）启动子进程跑一段失控脚本，
//! 观察子进程的收场方式：
//!
//! | 观察结果 | 含义 |
//! |---|---|
//! | 退出码 0 | 沙箱按预期产出错误并正常收尾 |
//! | 退出码 1 | 测试断言失败（子进程自己报的） |
//! | 退出码 2 | 脚本被中断、但沙箱之外出了问题（见子进程的输出） |
//! | 信号/异常退出码（如 0xC0000005、-1073741819） | **栈溢出/段错误**——护栏失效 |
//! | 超时 | **挂死**——中断未生效 |
//!
//! ## 子进程怎么跑
//!
//! 复用测试二进制自身：用环境变量选择一个"以子进程身份运行"的模式
//! （`TOME_JS_CHILD_CASE`），该模式下 `main` 不做别的，只跑指定用例然后退出。
//! 这比另建一个 bin target 更省——测试二进制已经链接了 tome-js。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数
// （`run_in_child` / `run_child_case`）。这些辅助函数失败说明测试环境本身
// 有问题（取不到当前可执行文件路径、起不了子进程），此时让调用它的测试
// 当场失败是正确行为。与 `tome-cli/tests/minimal_loop.rs` 同一处理。
#![expect(
    clippy::expect_used,
    reason = "子进程机制无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tome_js::runtime::{Limits, Sandbox, SandboxError};

/// 子进程的环境变量名（父进程设置它来指定要跑哪个用例）。
const CHILD_CASE_ENV: &str = "TOME_JS_CHILD_CASE";

/// 子进程整体超时。真挂死时父进程不能无限等——那会让 CI 卡满时长。
const CHILD_TIMEOUT: Duration = Duration::from_secs(30);

/// 失控脚本的用例表：父进程逐个用子进程跑。
const CASES: &[&str] = &[
    "dead-loop",
    "infinite-recursion",
    "oom",
    "syntax-error",
    "thrown-object",
    "uncatchable-in-try",
];

/// 子进程入口：由 `#[test]` 在设置了环境变量时调用。
///
/// 返回子进程应使用的退出码。父进程从不调用它。
pub fn run_child_case(case: &str) -> i32 {
    // 刻意压低资源上限：让"内存/栈护栏"在 CI 上稳定触发，而不依赖机器规格。
    //
    // ## GC 阈值必须跟着内存上限一起压
    //
    // `Limits` 要求 `gc_threshold_bytes <= max_memory_bytes`（否则阈值永远
    // 达不到，GC 只在内存耗尽前跑一次巨型回收，与"短的不可中断窗口"相反）。
    // 只改内存上限会得到 `GcThresholdAboveMemoryLimit`，那会让子进程在
    // **建沙箱**这一步就失败——而那样测出来的"进程没崩"是假绿：
    // 它证明的只是"没跑脚本"，不是"沙箱收住了脚本"。
    let limits = Limits {
        max_stack_bytes: 128 * 1024,
        // 墙钟上限压到 1 秒：子进程的父进程只等 30 秒（`CHILD_TIMEOUT`），
        // 用默认的 30 秒会让"中断生效"与"父进程超时"同时发生——
        // 那样观察到的 `Timeout` 无法区分"沙箱中断成功"与"沙箱挂死"。
        wall_clock_timeout: Some(std::time::Duration::from_secs(1)),
        ..Limits {
            max_memory_bytes: 4 * 1024 * 1024,
            gc_threshold_bytes: 1024 * 1024,
            ..Limits::default()
        }
    };

    let sandbox = match Sandbox::new(limits) {
        Ok(sandbox) => sandbox,
        Err(error) => {
            eprintln!("建沙箱失败：{error}");
            return 2;
        }
    };

    let script = match case {
        "dead-loop" => "while(true){}",
        // 撞栈上限：护栏失效时这里会是段错误（进程被信号带走）
        "infinite-recursion" => "function f(){ return f(); } f();",
        "oom" => "var a = []; for (;;) { a.push(new Array(1000).fill(0)); }",
        "syntax-error" => "function ( { [ }",
        "thrown-object" => "throw { x: 1 };",
        // 取消信号穿透 try/catch（书源自带错误处理拦不住）
        "uncatchable-in-try" => "try { while(true){} } catch (e) {} ",
        other => {
            eprintln!("未知用例：{other}");
            return 2;
        }
    };

    let outcome = sandbox.eval(script);
    match outcome {
        Err(SandboxError::Interrupted { .. })
        | Err(SandboxError::Script { .. })
        | Err(SandboxError::MemoryLimit { .. })
        | Err(SandboxError::Poisoned { .. }) => {
            // 期望路径：沙箱把失控脚本收成了一个**错误值**，进程完好。
            //
            // 再跑一次以证明沙箱对象状态仍然自洽（未污染时可用、
            // 已污染时给结构化错误）——"没崩"不等于"状态还对"。
            if sandbox.is_poisoned() {
                if !matches!(sandbox.eval("1"), Err(SandboxError::Poisoned { .. })) {
                    eprintln!("已废弃的沙箱应拒绝执行");
                    return 1;
                }
            } else if sandbox.eval("1+1").map(|v| v.as_int()) != Ok(Some(2)) {
                eprintln!("未污染的沙箱应仍可执行");
                return 1;
            }
            0
        }
        // 配额/构造错误：说明子进程的前置条件就不成立，属"机制坏了"
        // 而非"沙箱收住了脚本"——按非 0 报出，避免这类问题被当成成功。
        Err(SandboxError::Limits(_)) | Err(SandboxError::Setup { .. }) => {
            eprintln!("子进程建沙箱失败：{outcome:?}");
            2
        }
        Ok(value) => {
            // 用例设计上这些都该失败。`Ok` 说明沙箱根本没起作用。
            eprintln!("用例 {case} 本应失败，实际成功并返回 {value:?}");
            1
        }
    }
}

/// 父进程视角的收场方式。
#[derive(Debug, PartialEq, Eq)]
enum Exit {
    /// 正常退出（附带码）
    Code(i32),
    /// 被信号/异常终止（护栏失效弄崩进程的形态）
    Abnormal,
    /// 超时（挂死）
    Timeout,
}

fn run_in_child(case: &str) -> Exit {
    let exe = std::env::current_exe().expect("应能取到测试二进制路径");
    let mut child = Command::new(exe)
        // 只跑这一个用例：子进程里的其它测试会被 `#[test]` 的过滤跳过
        .arg("--exact")
        .arg("子进程测试入口")
        .env(CHILD_CASE_ENV, case)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("应能启动子进程");

    let started = Instant::now();
    loop {
        match child.try_wait().expect("应能查询子进程状态") {
            Some(status) => {
                return match status.code() {
                    Some(code) => Exit::Code(code),
                    // `code()` 为 `None` 表示进程被**信号/异常**终止
                    // （Windows 上是访问违例之类的 NTSTATUS，Unix 上是信号）。
                    // 这正是"护栏失效弄崩了进程"的形态，故单独一类。
                    //
                    // 不读具体的信号号/原始状态：跨平台取它的 API 不同
                    // （Windows 的 `ExitStatusExt::into_raw` 返回 `u32`，
                    // Unix 的 `signal()` 返回 `Option<i32>`），而我们的判据
                    // 只需要"是不是异常终止"这一个布尔。
                    None => Exit::Abnormal,
                };
            }
            None => {
                if started.elapsed() > CHILD_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Exit::Timeout;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[test]
fn 子进程测试入口() {
    // 双身份：设了环境变量时本测试就是"子进程主体"，跑完直接返回；
    // 未设置时（父进程里）它是一个空测试。
    if let Ok(case) = std::env::var(CHILD_CASE_ENV) {
        let code = run_child_case(&case);
        // 子进程：正常收尾。`process::exit` 不做栈展开，故先 flush。
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::process::exit(code);
    }
}

/// 逐个用例验证"沙箱失控但宿主进程完好"。
///
/// 这条测试是 T030 "沙箱崩溃不带走进程"验收项的**唯一**有效证据形态。
#[test]
fn 失控脚本不带走宿主进程() {
    let mut failures = Vec::new();
    for case in CASES {
        let exit = run_in_child(case);
        match &exit {
            Exit::Code(0) => {}
            other => failures.push(format!("{case}: {other:?}")),
        }
    }
    assert!(
        failures.is_empty(),
        "下列用例未能正常收场（0 以外的退出方式说明护栏失效）：\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn 子进程机制的健全性自检() {
    // 防"假绿"：若 `run_in_child` 因为路径/参数错误而从来没有真正跑起子进程，
    // 上面的测试会看到非 0 退出码并失败——但若它把"启动失败"也吞掉，
    // 上面那条测试就永远不可能是绿的。这里用一个**已知会失败**的用例
    // 反向确认机制确实在工作：能观察到它失败，才能相信观察到的成功。
    let exit = run_in_child("dead-loop");
    assert_eq!(
        exit,
        Exit::Code(0),
        "中止路径应正常收场；若机制坏了这里会看到 Abnormal/Timeout"
    );

    // 反向：一个会真的把测试写坏的用例（`run_child_case` 返回 2 说明前置失败）。
    // 通过环境变量伪造一个未知用例名来触发该分支。
    let exe = std::env::current_exe().expect("应能取到测试二进制路径");
    let output = Command::new(exe)
        .arg("--exact")
        .arg("子进程测试入口")
        .env(CHILD_CASE_ENV, "no-such-case")
        .output()
        .expect("应能跑子进程");
    assert_eq!(
        output.status.code(),
        Some(2),
        "未知用例应让子进程以 2 退出（证明子进程主体真的在执行我们的代码）"
    );
}
