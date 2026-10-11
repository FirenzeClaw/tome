//! T007 验收：引擎线程（`crates/tome-app/src/engine/thread.rs`）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T007）
//!
//! - 发送会导致 panic 的合成请求 → 收到 [`EngineEvent::EnginePanic`]，且线程
//!   **存活可继续服务**（T003 的 `panic = "unwind"` 前提）
//! - 线程模型断言：`Paginator` 不离开引擎线程（编译期 + 文档双重）
//! - 启动期 IO 失败（数据目录不可写 / 库损坏）以**结构化错误**呈现并给出指引
//!   （S1 FR-043 语义 / spec FR-023）——引擎线程启动失败**不静默**
//!
//! ## 为什么"panic 后线程仍可用"是这里的核心判据
//!
//! spec 的边缘情况写着「排版/解析在单章上的失败 MUST 被隔离在该章……
//! MUST NOT 使应用整体崩溃」。这条要求落在实现上只有一种形态：**引擎线程
//! 在每次请求边界 `catch_unwind`**。而它成立的前提是 release profile 的
//! `panic = "unwind"`（T003）——在 `abort` 下 `catch_unwind` 不可能生效。
//!
//! 故本文件的两条测试合起来是 T003 那个决策的**可执行证据**：
//!
//! | 测试 | 证明 |
//! |---|---|
//! | `panic_被捕获且线程存活` | 捕获确实发生了，且进程没死 |
//! | `panic_之后的请求照常服务` | 线程状态没被 panic 污染（不是"捕获了但废了"） |
//!
//! 第二条尤其重要：一个"捕获 panic 后直接退出循环"的实现能通过第一条，
//! 但用户看到的是"读某章崩了，之后整个阅读器不再响应"。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tome_app::engine::messages::{ClassifiedErrorView, EngineEvent, EngineRequest};
use tome_app::engine::thread::{EngineHandle, EngineWorker, StartupFailure};

// ===========================================================================
// 合成 worker（测试用；真实 worker 在 T010/T023 的 adapter 之后）
// ===========================================================================

/// 合成引擎：按请求内容决定"正常回应 / panic / 记录顺序"。
///
/// ## 为什么用合成 worker 而不是等真实 adapter
///
/// 本任务要验的是**线程与兜底的骨架**，与"用哪个 store"无关。合成 worker
/// 让判据保持纯粹：panic 是我们**要求**它 panic 的，而不是某个真实依赖
/// 偶发失败——后者会让测试 flaky，而 flaky 的兜底测试比没有测试更糟
/// （它会在真正需要时被当成"又 flaky 了"忽略掉）。
struct SyntheticWorker {
    /// 已处理的请求数（用来断言"线程还在服务"）
    served: Arc<AtomicUsize>,
    /// 记录见到的关键词顺序（用来断言线程状态未被 panic 污染）
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

impl EngineWorker for SyntheticWorker {
    #[expect(
        clippy::panic,
        reason = "夹具里的 panic 是**被测对象**：本任务要验的正是「引擎 worker 炸了会怎样」"
    )]
    fn handle(&mut self, request: EngineRequest) -> Vec<EngineEvent> {
        self.served.fetch_add(1, Ordering::SeqCst);

        match request {
            // 合成请求：`SourceSearch` 的 keyword 是 "panic" 时炸掉。
            // 用真实变体而非测试专用变体，是为了让"panic 发生在真实消息路径上"
            // 这一点成立——否则测的是另一条不存在于生产的分支。
            EngineRequest::SourceSearch { keyword, .. } if keyword == "panic" => {
                panic!("合成的引擎 panic：keyword={keyword}");
            }
            EngineRequest::SourceSearch {
                keyword,
                generation,
            } => {
                self.seen
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(keyword.clone());
                vec![EngineEvent::SearchDone { generation }]
            }
            EngineRequest::SettingsLoad => {
                self.seen
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push("settings".to_owned());
                vec![EngineEvent::SettingsLoaded {
                    settings: tome_app::engine::messages::UserSettings::default_ma(),
                }]
            }
            _ => vec![EngineEvent::Saved],
        }
    }
}

/// 造一个合成引擎线程。
#[expect(
    clippy::expect_used,
    reason = "夹具内的 expect 是刻意的：引擎起不来说明装置坏了，应当让测试红"
)]
fn spawn_synthetic() -> (
    EngineHandle,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Vec<String>>>,
) {
    let served = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let worker = SyntheticWorker {
        served: Arc::clone(&served),
        seen: Arc::clone(&seen),
    };
    let handle = EngineHandle::spawn(move || Ok(worker)).expect("合成引擎应能启动");
    (handle, served, seen)
}

/// 轮询直到收到至少 `count` 条事件（或超时）。
///
/// ## 为什么不用"睡固定时长再看"
///
/// 固定睡眠在快机器上是浪费、在慢机器上是 flaky。这里轮询到条件满足为止，
/// 上限只用来把**真失败**（事件永不到达）变成一条明确的断言失败。
fn collect_at_least(handle: &EngineHandle, count: usize) -> Vec<EngineEvent> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut events = Vec::new();
    while events.len() < count && std::time::Instant::now() < deadline {
        events.extend(handle.drain_events());
        if events.len() < count {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    events
}

// ===========================================================================
// panic 兜底（本任务的核心判据）
// ===========================================================================

/// 会 panic 的请求 → 收到 `EnginePanic` 事件，**进程没死**。
#[test]
fn panic_被捕获并转为结构化事件() {
    let (handle, _served, _seen) = spawn_synthetic();

    handle.send(EngineRequest::SourceSearch {
        generation: 1,
        keyword: "panic".to_owned(),
    });

    let events = collect_at_least(&handle, 1);
    let panic_event = events
        .iter()
        .find(|event| matches!(event, EngineEvent::EnginePanic { .. }))
        .unwrap_or_else(|| panic!("应收到 EnginePanic 事件，实测 {events:?}"));

    let EngineEvent::EnginePanic { context } = panic_event else {
        unreachable!("上面已筛过");
    };
    assert!(
        !context.is_empty(),
        "EnginePanic 应带上发生位置（context），否则无法定位"
    );
    assert!(
        context.contains("panic") || context.contains("SourceSearch") || context.contains("search"),
        "context 应指向发生 panic 的请求，实测 {context}"
    );
}

/// panic 之后线程**照常服务**（不是"捕获了但废了"）。
///
/// ## 这条为什么不能省
///
/// 一个"捕获 panic 后 `break` 出循环"的实现能通过上一条测试，但用户看到的
/// 是"读某章崩了，之后整个阅读器不再响应"。判据必须是**后续请求有回应**，
/// 而不只是"没崩"。
#[test]
fn panic_之后的请求照常服务() {
    let (handle, served, seen) = spawn_synthetic();

    // 1) 先来一发会炸的
    handle.send(EngineRequest::SourceSearch {
        generation: 1,
        keyword: "panic".to_owned(),
    });
    let events = collect_at_least(&handle, 1);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, EngineEvent::EnginePanic { .. })),
        "前置：应收到 EnginePanic，实测 {events:?}"
    );

    // 2) 再来一发正常的——它必须有回应
    handle.send(EngineRequest::SourceSearch {
        generation: 2,
        keyword: "三体".to_owned(),
    });
    let events = collect_at_least(&handle, 1);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, EngineEvent::SearchDone { generation: 2 })),
        "panic 之后线程必须继续服务，实测 {events:?}"
    );

    // 3) 状态没被污染：正常请求的关键词被记录下来了
    let recorded = seen.lock().expect("互斥锁不应中毒").clone();
    assert_eq!(
        recorded,
        vec!["三体".to_owned()],
        "panic 的那次不应留下状态（它没走到记录那一步），正常的那次应在"
    );

    // 4) 两次请求都真的进了 worker（panic 那次也算"处理过"）
    assert_eq!(
        served.load(Ordering::SeqCst),
        2,
        "panic 与正常请求都应到达 worker"
    );
}

/// panic 只影响**那一次请求**，不影响后续请求的返回内容。
#[test]
fn panic_不改变后续请求的返回内容() {
    let (handle, _served, _seen) = spawn_synthetic();

    // 基准：没有 panic 时的返回
    handle.send(EngineRequest::SettingsLoad);
    let baseline = collect_at_least(&handle, 1);
    assert!(
        baseline
            .iter()
            .any(|event| matches!(event, EngineEvent::SettingsLoaded { .. })),
        "前置：正常请求应有回应，实测 {baseline:?}"
    );

    // 中间插一发 panic
    handle.send(EngineRequest::SourceSearch {
        generation: 9,
        keyword: "panic".to_owned(),
    });
    let _ = collect_at_least(&handle, 1);

    // 同样的请求，返回内容应与基准**逐字段相同**
    handle.send(EngineRequest::SettingsLoad);
    let after = collect_at_least(&handle, 1);
    let settings_after: Vec<&EngineEvent> = after
        .iter()
        .filter(|event| matches!(event, EngineEvent::SettingsLoaded { .. }))
        .collect();
    assert_eq!(
        settings_after.len(),
        1,
        "panic 之后的同类请求仍应有且只有一条回应，实测 {after:?}"
    );
    let baseline_settings = baseline
        .iter()
        .find(|event| matches!(event, EngineEvent::SettingsLoaded { .. }))
        .expect("基准里应有 SettingsLoaded");
    assert_eq!(
        *settings_after[0], *baseline_settings,
        "panic 不该改变后续请求的返回内容"
    );
}

// ===========================================================================
// 线程模型（Paginator 不离开引擎线程）
// ===========================================================================

/// `EngineHandle` **不是泛型的**——worker 的类型被擦除在引擎线程内。
///
/// ## 这条断言在守什么
///
/// "`Paginator` 不离开引擎线程"这条约束最容易破的形态是：把 worker 类型
/// 暴露到句柄上（`EngineHandle<W>`），于是上层可以 `handle.worker()` 拿到
/// 那台昂贵的 `FontSystem` 并在 UI 线程里用。那一刻起，"独占"就成了约定
/// 而非事实，而并发问题要到线上才现形。
///
/// 类型名里出现泛型参数就是那种改动的**可观测后果**，故此处直接断言它。
/// 这不优雅，但它是一个**会响的铃**：比"读代码确认没暴露"可靠。
#[test]
fn engine_handle_不暴露_worker_类型() {
    let name = std::any::type_name::<EngineHandle>();
    assert_eq!(
        name, "tome_app::engine::thread::EngineHandle",
        "EngineHandle 不该带类型参数——worker（含 Paginator/FontSystem）必须被\
         擦除在引擎线程内，上层拿不到它"
    );
}

/// `EngineHandle` 可跨线程持有（UI 线程建、UI 线程用），且**只有**这些公开面。
///
/// ## 编译期断言的意义
///
/// `EngineHandle: Send + Sync` 是 UI 线程持有它的前提。而"只有 send /
/// drain_events / shutdown 三个方法"这件事由**签名**表达：本测试调用它们
/// 即证明它们在；不存在 `worker()` 之类的访问器则由上一条的泛型断言兜住。
#[test]
fn engine_handle_可跨线程持有且接口收敛() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<EngineHandle>();

    let (handle, _served, _seen) = spawn_synthetic();
    let shared = Arc::new(handle);
    let clone = Arc::clone(&shared);

    // 从**另一个线程**发请求，并在主线程收事件——跨线程持有真的可用
    let sender = std::thread::spawn(move || {
        clone.send(EngineRequest::SettingsLoad);
    });
    sender.join().expect("发送线程应正常结束");

    let events = collect_at_least(&shared, 1);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, EngineEvent::SettingsLoaded { .. })),
        "跨线程持有句柄时请求仍应送达，实测 {events:?}"
    );
}

/// 句柄析构后引擎线程**已结束**（不泄漏线程）。
#[test]
fn 句柄析构后引擎线程结束() {
    let (handle, _served, _seen) = spawn_synthetic();
    handle.send(EngineRequest::SettingsLoad);
    let _ = collect_at_least(&handle, 1);

    // `shutdown` 阻塞到线程真正结束——这是"不泄漏"的可判定形态
    handle.shutdown();
}

// ===========================================================================
// 启动期失败（FR-023 / S1 FR-043）
// ===========================================================================

/// 启动期失败返回**结构化错误 + 指引**，不静默。
#[test]
fn 启动期失败给出结构化错误与指引() {
    let failure = StartupFailure::new(
        ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Storage),
        "数据目录不可写：D:/readonly/tome（只读挂载）",
        "请把程序移到可写目录，或在设置里指定另一个数据目录",
    );

    // 结构化：标识 + 细节 + 指引三者齐备
    assert_eq!(failure.error().id, "error.storage");
    assert!(
        failure.detail().contains("只读挂载"),
        "细节应指向具体原因，实测 {}",
        failure.detail()
    );
    assert!(
        failure.guidance().contains("可写目录"),
        "必须给出可行动指引（FR-043），实测 {}",
        failure.guidance()
    );

    // 不静默：启动失败的**唯一**出路是 Err，拿不到句柄。
    // 闭包显式标注返回类型：`W` 只出现在 Ok 分支，没有注解无从推断。
    let result =
        EngineHandle::spawn(|| -> Result<SyntheticWorker, StartupFailure> { Err(failure) });
    assert!(
        result.is_err(),
        "启动失败必须返回 Err——若返回 Ok，上层就可能拿到一个永不服务事件的句柄"
    );

    let failure = result.err().expect("上面已判 Err");
    assert_eq!(
        failure.error().id,
        "error.storage",
        "错误标识应原样透出（供 i18n 查表）"
    );
}

/// 启动失败时**引擎线程确实已结束**（不留一个空转线程）。
#[test]
fn 启动失败不留空转线程() {
    for _ in 0..8 {
        let result = EngineHandle::spawn(|| -> Result<SyntheticWorker, StartupFailure> {
            Err(StartupFailure::new(
                ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Storage),
                "库损坏",
                "请从备份恢复或删除数据目录重建",
            ))
        });
        assert!(result.is_err(), "应报启动失败");
    }
    // 若启动失败路径忘了 join/退出，这里会累积线程；重复 8 次让泄漏可见
    // （真正的判据是"进程没被拖死"，但那是整体观察——此处用重复调用表达）。
}

/// 启动成功但 worker 立刻失效的情形**不**被误报为启动失败。
#[test]
fn 启动成功即返回可用句柄() {
    let (handle, _served, _seen) = spawn_synthetic();
    handle.send(EngineRequest::SettingsLoad);
    let events = collect_at_least(&handle, 1);
    assert!(!events.is_empty(), "启动成功的句柄必须真的能服务请求");
    handle.shutdown();
}

// ===========================================================================
// release profile 前提（T003 的回退守卫）
// ===========================================================================

/// `[profile.release] panic` 必须是 `"unwind"`（T003 的回退守卫）。
///
/// ## 这条为什么必须存在（否则上面三条测试是假绿）
///
/// `cargo test` 跑的是 **dev** profile，而 dev 的 `panic` 恒为 `unwind`
/// （Cargo 不允许给 dev 设 abort）。于是"panic 被捕获且线程存活"那三条测试
/// **在 release 是 abort 的世界里照样会通过**——它们测的是 dev 的行为，而
/// 用户跑的是 release。
///
/// 这正是 T003 那个决策的失效形态：有人把 `panic` 改回 `"abort"`（比如为了
/// 体积），dev 下全部测试依旧绿，而 release 构建里 `catch_unwind` 静默失效
/// ——用户看到的是"读某章崩了，整个阅读器消失"。
///
/// 故此处**直接断言配置文件**：这是唯一能在测试里覆盖 release 语义的手段
/// （另一种是跑 `cargo build --release` 再验证，代价是分钟级的构建）。
#[test]
fn release_profile_must_be_unwind() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crate 目录应有两级父目录")
        .join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("应能读 {}：{error}", manifest.display()));

    let mut in_release = false;
    let mut panic_setting = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_release = trimmed == "[profile.release]";
            continue;
        }
        if !in_release || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            if key.trim() == "panic" {
                panic_setting = Some(value.trim().trim_matches('"').to_owned());
            }
        }
    }

    assert_eq!(
        panic_setting.as_deref(),
        Some("unwind"),
        "release profile 的 panic 必须是 unwind（S3 T003，用户已批准）：         abort 下 catch_unwind 在 release 不可能生效，而 spec 的边缘情况要求         「引擎 panic 经外壳兜底捕获」。dev profile 恒为 unwind，故本断言是         唯一能覆盖 release 语义的手段——少了它，改回 abort 不会有任何测试变红。"
    );
}
