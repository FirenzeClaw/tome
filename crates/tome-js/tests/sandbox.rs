//! T030 验收测试：QuickJS runtime 包装的三道护栏（tasks.md Phase 4）
//!
//! ## 这些测试在守什么
//!
//! 沙箱是**安全边界**（AGENTS.md 生产级护栏）。三条约束各自对应一类真实事故：
//!
//! | 约束 | 不满足时的后果 |
//! |---|---|
//! | 内存上限 | 恶意/失控脚本耗尽进程内存，整个应用被系统杀掉 |
//! | 栈上限 | 深层递归触发 C 栈溢出 → **进程直接 abort**，无法捕获 |
//! | 中断 handler | `while(true){}` 挂死引擎，用户只能杀进程 |
//!
//! 三者里最容易被忽略的是**栈上限**：QuickJS 默认栈上限 256 KiB，在宿主
//! 线程栈较大的平台上"默认值够用"是错觉——真出栈溢出时是 SIGSEGV/abort，
//! Rust 层完全没有机会把它转成可捕获的错误。故 T030 的验收项明确要求
//! **显式** `set_max_stack_size`。
//!
//! ## 为什么这些测试必须放在 tests/（集成测试）
//!
//! 它们断言的是**公开 API 的契约**（`Sandbox::eval` 的错误分类、runtime 的
//! 可复用性），而不是内部实现细节。集成测试看到的是外部消费方（tome-rule /
//! tome-source / 组合根）看到的那个接口。
//!
//! ## 关于"进程不挂"这类断言的写法
//!
//! 真正的"进程不挂"只能由**父进程观察子进程**来证明——测试进程自己挂掉时
//! 测试框架也一起消失，`assert` 根本不会执行。故此处：
//!
//! - 本文件的测试验证**行为正确性**（能中断、能捕获、能继续用）；
//! - "不带走进程"由 `tests/process_survival.rs` 用**子进程**验证（那里跑
//!   同样的脚本，断言子进程正常退出且退出码不是崩溃码）。

use std::time::{Duration, Instant};

use tome_js::runtime::{Limits, Sandbox, SandboxError};

/// 等待中断生效的时间上界。
///
/// 取 10 秒而不是 1 秒：中断检查点是**每 10000 条字节码**一次
/// （QuickJS 的 `JS_INTERRUPT_COUNTER_INIT`），在 CI 的慢机上留足余量。
/// 这个上界同时也是一个断言——"挂死"与"用了几秒才停"是两回事，
/// 我们只接受前者失败。
const INTERRUPT_DEADLINE: Duration = Duration::from_secs(10);

/// 测试用的墙钟上限。
///
/// ## 为什么测试不能直接用 `Limits::default()`
///
/// 默认上限是 30 秒（对真实书源是合理的——见该值的文档），但测试跑一遍
/// 要等满它。**更糟的是**：若测试用默认值却断言"10 秒内中断"，
/// 那个断言在护栏完全失效时仍会通过（因为 30 秒后终于超时了），
/// 于是测试变成了假绿。故测试统一用短上限，让"没中断"立刻表现为超时。
const TEST_TIMEOUT: Duration = Duration::from_millis(500);

/// 测试用配额：在默认值上换一个短的在墙钟上限。
fn default_limits() -> Limits {
    Limits {
        wall_clock_timeout: Some(TEST_TIMEOUT),
        ..Limits::default()
    }
}

/// 构造配额，并**按需要在比例上缩小的同时保持自洽**。
///
/// `Limits` 要求 `gc_threshold_bytes <= max_memory_bytes`（见 `Limits::validate`），
/// 故测试里把内存上限压低时必须一并压 GC 阈值——否则会得到
/// `GcThresholdAboveMemoryLimit`，而那不是这些测试要验证的东西。
fn limits_with_memory(max_memory_bytes: usize) -> Limits {
    Limits {
        max_memory_bytes,
        gc_threshold_bytes: (max_memory_bytes / 4).max(1),
        wall_clock_timeout: Some(TEST_TIMEOUT),
        ..Limits::default()
    }
}

// ---------------------------------------------------------------------------
// 死循环可中断（FR-007 / SC-013）
// ---------------------------------------------------------------------------

#[test]
fn 死循环被中断且不挂死() {
    let sandbox = Sandbox::new(default_limits()).expect("应能建沙箱");
    let started = Instant::now();
    let outcome = sandbox.eval("while(true){}");
    let elapsed = started.elapsed();

    assert!(
        matches!(outcome, Err(SandboxError::Interrupted { .. })),
        "死循环应被中断，实际 {outcome:?}"
    );
    assert!(
        elapsed < INTERRUPT_DEADLINE,
        "中断应在 {INTERRUPT_DEADLINE:?} 内生效，实际用了 {elapsed:?}"
    );
}

#[test]
fn 深度嵌套循环也被中断() {
    // 只覆盖 `while(true){}` 会在"中断检查点只挂在 OP_goto 上"这种实现下
    // 假绿——`for(;;)` 编译出的跳转指令不同。两类都测。
    for script in [
        "while(true){}",
        "for(;;){}",
        "do{}while(true)",
        "let i=0; while(true){ i++; }",
        "function f(){ while(true){} } f();",
    ] {
        let sandbox = Sandbox::new(default_limits()).expect("应能建沙箱");
        let outcome = sandbox.eval(script);
        assert!(
            matches!(outcome, Err(SandboxError::Interrupted { .. })),
            "`{script}` 应被中断，实际 {outcome:?}"
        );
    }
}

#[test]
fn 无限递归被栈上限接住而非弄崩进程() {
    // 这是"为什么必须显式设栈上限"的直接验收：不设或设得比宿主线程栈还大时，
    // 这个脚本会撞穿 C 栈并 abort 掉整个进程——那时下面的 assert 都不会执行。
    let limits = Limits {
        max_stack_bytes: 128 * 1024,
        ..default_limits()
    };
    let sandbox = Sandbox::new(limits).expect("应能建沙箱");

    let outcome = sandbox.eval("function f(){ return f(); } f();");
    assert!(
        outcome.is_err(),
        "无限递归必须以错误收场（而不是耗尽进程栈），实际 {outcome:?}"
    );
    // 具体分类允许随 QuickJS 的内部判定变化（可能是 stack overflow，
    // 也可能是通用 JS 异常），但**必须是可捕获的错误而非崩溃**。
    match outcome.expect_err("应报错") {
        SandboxError::Script { .. }
        | SandboxError::Interrupted { .. }
        | SandboxError::MemoryLimit { .. } => {}
        other => panic!("应是脚本级错误，实际 {other:?}"),
    }
}

#[test]
fn 中断错误不可被书源自带的_try_catch_捕获() {
    // FR-007 的硬要求（tasks.md T030 第四条验收项）：取消信号必须穿透
    // 书源自己的错误处理。
    //
    // ## 依据（不是猜测，已在 vendored QuickJS 源码里核实）
    //
    // 1. 中断走 `JS_ThrowInterrupted` → `JS_SetUncatchableError`
    //    （`quickjs.c` 的 `__js_poll_interrupts`）。
    // 2. 解释器的 `exception:` 段先判 `JS_IsUncatchableError`，为真就**不**
    //    去扫 catch offset（`quickjs.c` 的 `JS_CallInternal`）。
    // 3. 显式 `throw` 是 `JS_Throw`，**不**设 uncatchable 标志。
    //
    // 故"书源 try/catch 拦不住取消"成立，而"书源自己 throw 能被自己的
    // catch 接住"也同时成立——两个方向都测，避免把测试写成"任何错误都
    // 穿透"的过强断言。
    let sandbox = Sandbox::new(default_limits()).expect("应能建沙箱");

    let caught = sandbox.eval(
        r#"
        var swallowed = false;
        try { while(true){} } catch (e) { swallowed = true; }
        swallowed;
        "#,
    );
    assert!(
        matches!(caught, Err(SandboxError::Interrupted { .. })),
        "取消信号必须穿透 try/catch，实际 {caught:?}"
    );

    // 反向对照：书源自己抛的异常**必须**能被自己捕获，否则降级逻辑失效。
    let own_error = sandbox
        .eval(r#"try { throw new Error("x"); } catch (e) { "caught:" + e.message; }"#)
        .expect("自己的异常应可捕获");
    assert_eq!(own_error.as_str(), Some("caught:x"));
}

// ---------------------------------------------------------------------------
// 内存上限（OOM 可捕获，runtime 整体丢弃）
// ---------------------------------------------------------------------------

#[test]
fn 内存上限被触发且错误可捕获() {
    // 上限设小：2 MiB 足够跑完 prelude（如果有），远不够分配一个大数组。
    let sandbox = Sandbox::new(limits_with_memory(2 * 1024 * 1024)).expect("应能建沙箱");

    let outcome = sandbox.eval("var a = []; for (;;) { a.push(new Array(1000).fill(0)); }");
    assert!(
        matches!(outcome, Err(SandboxError::MemoryLimit { .. })),
        "超内存上限应产出可捕获的 MemoryLimit，实际 {outcome:?}"
    );
}

#[test]
fn 触发过_oom_的_runtime_被整体丢弃() {
    // tasks.md T030 第二条验收项。QuickJS 在 OOM 之后内部状态可能不一致
    // （分配失败的清理路径不保证恢复全部不变量），故治理方式是**丢弃整个
    // runtime**，而不是指望它还能继续跑下一个脚本。
    let sandbox = Sandbox::new(limits_with_memory(2 * 1024 * 1024)).expect("应能建沙箱");

    let _ = sandbox.eval("var a = []; for (;;) { a.push(new Array(1000).fill(0)); }");
    assert!(
        sandbox.is_poisoned(),
        "触发 OOM 后沙箱必须标记为已废弃（不得复用）"
    );

    // 废弃后的调用必须是**结构化错误**，而不是"再试一次可能失败的脚本"。
    let reused = sandbox.eval("1+1");
    assert!(
        matches!(reused, Err(SandboxError::Poisoned { .. })),
        "已废弃的沙箱不得再执行脚本，实际 {reused:?}"
    );

    // 换一个新沙箱照常工作——证明"丢弃"是可行路径。
    let fresh = Sandbox::new(default_limits()).expect("应能建新沙箱");
    assert_eq!(fresh.eval("1+1").expect("应能执行").as_int(), Some(2));
}

#[test]
fn 非_oom_的错误不污染沙箱() {
    // 反向对照：普通脚本错误（语法错、自己抛的异常）**不该**让 runtime 报废。
    // 若把两者混为一谈，书源里任何一个被 try/catch 接住的异常都会让该源
    // 后续规则无法求值——那是可用性的灾难。
    let sandbox = Sandbox::new(default_limits()).expect("应能建沙箱");

    let _ = sandbox.eval("this is not valid javascript");
    assert!(!sandbox.is_poisoned(), "语法错不应废弃沙箱");
    assert_eq!(sandbox.eval("1+1").expect("应能执行").as_int(), Some(2));

    let _ = sandbox.eval("throw new Error('boom')");
    assert!(!sandbox.is_poisoned(), "脚本异常不应废弃沙箱");
    assert_eq!(sandbox.eval("2+2").expect("应能执行").as_int(), Some(4));
}

// ---------------------------------------------------------------------------
// 低内存限额 + 畸形脚本：沙箱崩溃不带走进程（T030 第三条验收项）
// ---------------------------------------------------------------------------

#[test]
fn 低内存限额下的畸形脚本不会带走沙箱() {
    // 这一条把三种压力叠在一起：**低内存限额** + **畸形脚本**。
    // 单独测任一项都可能碰巧过关，叠起来才是真实书源会遇到的最坏情况
    // （一份写得乱七八糟的源 + 一台内存紧张的机器）。
    //
    // 这里覆盖四类畸形脚本，每一类在真实书源里都存在：
    // 非法语法、深层递归、抛掷异常、超长字符串。
    let limits = Limits {
        max_stack_bytes: 128 * 1024,
        ..limits_with_memory(4 * 1024 * 1024)
    };

    for script in [
        // 1. 非法语法
        "function ( { [ }",
        "var x = ;",
        "\"unterminated string",
        // 2. 深层递归（会撞栈上限）
        "function f(n){ return f(n+1); } f(0);",
        // 3. 异常抛掷
        "throw { weird: 'object thrown, not an Error' };",
        "null.foo.bar;",
        // 4. 畸形但合法的构造
        "JSON.parse('{ invalid json')",
        "new Array(-1)",
    ] {
        let sandbox = Sandbox::new(limits).expect("应能建沙箱");
        let outcome = sandbox.eval(script);
        assert!(
            outcome.is_err(),
            "畸形脚本 `{script}` 应产出错误，实际 {outcome:?}"
        );

        // 关键断言：沙箱在这次冲击之后仍是一个**结构完整的对象**。
        // 未污染时它必须还能跑简单脚本（证明 runtime 没处于半死状态）；
        // 已污染时必须给出结构化错误（证明废弃标记生效）。
        if sandbox.is_poisoned() {
            assert!(matches!(
                sandbox.eval("1"),
                Err(SandboxError::Poisoned { .. })
            ));
        } else {
            assert_eq!(
                sandbox
                    .eval("1+1")
                    .expect("未污染的沙箱应继续可用")
                    .as_int(),
                Some(2),
                "`{script}` 之后沙箱应仍可执行"
            );
        }
    }
}

#[test]
fn 畸形脚本不会影响后续新建的沙箱() {
    // 单元制隔离的验收：每个书源脚本一个 runtime（T031）。
    // 一个源把沙箱搞坏之后，下一个源必须从干净状态开始。
    let limits = Limits {
        max_stack_bytes: 64 * 1024,
        ..limits_with_memory(2 * 1024 * 1024)
    };

    let poisoned = Sandbox::new(limits).expect("应能建沙箱");
    let _ = poisoned.eval("var a=[]; for(;;){ a.push(new Array(1000).fill(0)); }");
    assert!(poisoned.is_poisoned());

    // 新沙箱完全不受影响
    let clean = Sandbox::new(default_limits()).expect("应能建沙箱");
    assert_eq!(
        clean.eval("'干净' + '沙箱'").expect("应能执行").as_str(),
        Some("干净沙箱")
    );
}

// ---------------------------------------------------------------------------
// 三道护栏本身：限额确实被设上了
// ---------------------------------------------------------------------------

#[test]
fn 中止信号可由外部触发() {
    // 除"死循环自动超时"之外，还要有**外部主动取消**的路径——
    // SC-013 要求"用户可中止"，而上层拿到的是取消令牌（A6 的 CancelToken）。
    let sandbox = Sandbox::new(default_limits()).expect("应能建沙箱");
    let token = sandbox.cancel_token();
    // 用一个慢脚本（大量循环）触发，在另一个线程里取消。
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        token.cancel();
    });

    let outcome = sandbox.eval("var i = 0; while (true) { i++; }");
    handle.join().expect("取消线程应正常结束");
    assert!(
        matches!(outcome, Err(SandboxError::Interrupted { .. })),
        "外部取消应中断脚本，实际 {outcome:?}"
    );
}

#[test]
fn 脚本超时被中断() {
    // 与"上限"并列的另一种护栏：执行时长上限。死循环只是时长无界的一种形态，
    // `for (var i=0;i<1e12;i++)` 这种"有界但远超预算"的脚本同样要能停下。
    let limits = Limits {
        wall_clock_timeout: Some(Duration::from_millis(200)),
        ..default_limits()
    };
    let sandbox = Sandbox::new(limits).expect("应能建沙箱");

    let started = Instant::now();
    let outcome = sandbox.eval("var s = 0; for (var i = 0; i < 1e12; i++) { s += i; } s;");
    assert!(
        matches!(outcome, Err(SandboxError::Interrupted { .. })),
        "超时脚本应被中断，实际 {outcome:?}"
    );
    assert!(
        started.elapsed() < INTERRUPT_DEADLINE,
        "超时应在时限附近生效"
    );
}

#[test]
fn 正常脚本不受护栏影响() {
    // 护栏的**反面验收**：它不该把正常脚本也拦下。一个"什么都能中断"的
    // 沙箱等于一个不能用的沙箱。
    let sandbox = Sandbox::new(default_limits()).expect("应能建沙箱");
    assert_eq!(sandbox.eval("1+1").expect("应能执行").as_int(), Some(2));
    assert_eq!(
        sandbox
            .eval("var a = []; for (var i = 0; i < 10000; i++) { a.push(i); } a.length;")
            .expect("正常循环应能跑完")
            .as_int(),
        Some(10000)
    );
    assert_eq!(
        sandbox
            .eval("function fib(n){ return n<2 ? n : fib(n-1)+fib(n-2); } fib(15);")
            .expect("正常递归应能跑完")
            .as_int(),
        Some(610)
    );
}

#[test]
fn 默认限额是有限值() {
    // "默认无限"是最危险的默认值：它意味着忘记配限额的调用点没有任何护栏。
    let limits = Limits::default();
    assert!(
        limits.max_memory_bytes > 0,
        "默认内存上限必须是有限值，否则 set_memory_limit(0) 等于不设限"
    );
    assert!(
        limits.max_stack_bytes > 0,
        "默认栈上限必须显式设置（QuickJS 自身默认仅 256KiB，且依赖平台线程栈大小）"
    );
    assert!(
        limits.wall_clock_timeout.is_some(),
        "默认应有执行时长上限——死循环之外还有'有界但无望'的脚本"
    );
}
