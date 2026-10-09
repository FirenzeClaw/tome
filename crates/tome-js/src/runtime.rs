//! [WIP] QuickJS runtime 包装：三道护栏（tasks.md T030 / FR-007 / SC-013）
//!
//! # 这个模块为什么存在
//!
//! 直接把 `rquickjs::Runtime` 暴露出去是不够的——**配额必须一次性配齐**，
//! 而"配齐"这件事有三个独立的技术点，每一个都对应一类真实事故：
//!
//! | 护栏 | 不设的后果 | 落点 |
//! |---|---|---|
//! | 内存上限 | 失控脚本耗尽进程内存 → 被系统杀掉 | `Runtime::set_memory_limit` |
//! | 栈上限 | 深层递归撞穿 C 栈 → **进程 abort，Rust 层无从捕获** | `Runtime::set_max_stack_size` |
//! | 中断 handler | `while(true){}` 挂死引擎，用户只能杀进程 | `Runtime::set_interrupt_handler` |
//!
//! 三者之中**栈上限**最容易被漏掉：QuickJS 自己的默认值是 256 KiB，
//! 而"宿主线程栈够大所以默认值够用"是错觉——一旦真的溢出就是段错误/abort，
//! Rust 侧完全没有机会把它转成错误值。故本模块**显式**设置它，且把它做成
//! [`Limits`] 的必填字段（而非 `Option`），让"忘记设"在类型层面不可能。
//!
//! # 关于 `rust-alloc` / `allocator` feature
//!
//! 一旦启用这两个 feature 之一，`set_memory_limit` 会**静默变成 noop**，
//! 内存防线彻底失效且没有任何报错。故它们在 Cargo.toml 层面被禁止，
//! 并由 `scripts/check-rquickjs-features.sh` 在 CI 里把守。
//!
//! # 中断为什么必须用 `set_interrupt_handler`
//!
//! FR-007 与 AGENTS.md 都明令：**绝不可用线程终止兜住死循环脚本**
//! （Rust 中杀线程是未定义行为）。唯一正确的做法是让引擎自己在检查点
//! 上报"该停了"。
//!
//! ## 检查点覆盖面的**实测结论**（对 vendored QuickJS 源码核验，非推测）
//!
//! 中断回调由 `quickjs.c` 的 `js_poll_interrupts` 触发，检查点是
//! **每 10000 条字节码**（`JS_INTERRUPT_COUNTER_INIT`）。其调用点：
//!
//! - **每一条 `OP_goto` / `OP_goto8` / `OP_goto16`**（解释器主循环）——
//!   故 `while(true){}` / `for(;;){}` / `do{}while(true)` 都能中断。
//! - `JS_CallInternal` 入口、`for...in` 迭代器、`Array.every/reduce/find/...`、
//!   `Object.isPrototypeOf`、`JS_OrdinaryIsInstanceOf` 等**可能长循环**的
//!   内建路径（源码里它们的注释都写着 `must check for timeout to avoid infinite loop`）。
//! - 正则引擎的 `lre_check_timeout`。
//!
//! ## `[UNCERTAIN]` GC 期间的中断
//!
//! **`JS_RunGC` 内部没有任何 `js_poll_interrupts` 检查点**——这是源码事实
//! （实测：`quickjs.c` 的 `gc_decref` / `gc_scan` / `gc_free_cycles` 三个函数
//! 里 `js_poll_interrupts` 出现次数为 0）。修复方式是继续在**程序计数器的
//! 层面**做，而不是指望 GC 能被中断：
//!
//! - GC 由 `js_trigger_gc` 在**分配 JSObject 时**触发，而分配只发生在解释器
//!   正在执行的语义步骤里。
//! - 故只要我们**把每次分配的总预算收紧**——即内存上限 + 每次 GC 的阈值
//!   （见 [`Limits::gc_threshold_bytes`]）——单次 GC 的工作量与存活对象数
//!   成正比而**有上界**，回到解释器后下一个 `OP_goto` 就会检查中断。
//! - 换句话说：GC 的"不可中断窗口"长度由内存上限间接界定，而不是无界。
//!
//! 这条推理链是本模块对 research.md R1「中断 GC 修复（PR #664）」的**落地方式**：
//! 修复的实质是"分配路径上的 GC 不再无限期阻塞中断响应"，而保证它成立
//! 需要调用方**确实设了内存上限**（设 0 = 不设限会让 GC 无界）。
//!
//! ## 中断异常不可被书源 `try/catch` 捕获（FR-007 的硬要求）
//!
//! 同样是源码事实（`quickjs.c`）：
//!
//! 1. `__js_poll_interrupts` 在回调返回 true 时走 `JS_ThrowInterrupted`
//!    → `JS_ThrowInternalError(ctx, "interrupted")` + **`JS_SetUncatchableError`**。
//! 2. 解释器的 `exception:` 段先判 `JS_IsUncatchableError(rt->current_exception)`，
//!    为真则**直接跳过**扫 catch offset 的循环。
//! 3. 显式 `throw` 走 `JS_Throw`，它**不**设置 uncatchable 标志。
//!
//! 故：书源自带的 `try/catch` 拦不住取消（因为引擎拒绝让 catch 生效），
//! 而书源自己抛的异常照常可捕获。两个方向都有测试钉住。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rquickjs::{Context, Ctx, Runtime};

/// 沙箱配额。
///
/// ## 为什么三个字段都不是 `Option`
///
/// "可选的护栏"在实践中等价于"没有护栏"——调用方会忘记设。做成必填字段后，
/// 想放宽也必须**显式写出来**，那是一个有意识的决定。默认值见
/// [`Limits::default`]，它们是有界的合理值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// JS 堆内存上限（字节）。
    ///
    /// **不得为 0**：`set_memory_limit(0)` 在 QuickJS 里等于"不设限"。
    /// 构造函数会拒绝 0（见 [`Limits::validate`]）。
    pub max_memory_bytes: usize,
    /// C 栈上限（字节）——显式设置，理由见模块文档。
    pub max_stack_bytes: usize,
    /// GC 触发阈值（字节）。
    ///
    /// 它同时决定了**单次 GC 的工作量上界**：GC 只在分配累计到该阈值时触发，
    /// 阈值越小则每次 GC 处理的存活对象越少、不可中断窗口越短（见模块文档的
    /// `[UNCERTAIN] GC 期间的中断`）。
    pub gc_threshold_bytes: usize,
    /// 单次脚本执行的墙钟上限。`None` 表示不限时——
    /// 但上层应当只在"已知脚本必然很快"的场合这么做。
    pub wall_clock_timeout: Option<Duration>,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // 32 MiB：够跑典型的书源脚本（字符串处理 + 少量 JSON），
            // 同时远小于"会把桌面应用拖垮"的量级。
            max_memory_bytes: 32 * 1024 * 1024,
            // 2 MiB：远大于 QuickJS 自身的 256 KiB 默认值，也远小于
            // Windows 主线程默认的 1 MiB 之外的常见线程栈配置——
            // 关键是它**有明确上界**，溢出时 QuickJS 会抛 JS 错误而不是撞穿 C 栈。
            //
            // [WORKAROUND] 这个值没有按平台区分。若实测发现某平台上
            // 正常书源脚本（如 deep-equal 之类的递归工具函数）会被它误伤，
            // 应改为按平台取 `std::thread` 的栈大小而不是继续调大常数。
            max_stack_bytes: 2 * 1024 * 1024,
            // 8 MiB：内存上限的四分之一。GC 频率与单次工作量之间的折中，
            // 理由见字段文档。
            gc_threshold_bytes: 8 * 1024 * 1024,
            // 30 秒：与 `tome_core::DEFAULT_TIMEOUT_MS` 同量级（网络请求的超时），
            // 一个纯计算脚本跑到 30 秒必然已经失控。
            wall_clock_timeout: Some(Duration::from_secs(30)),
        }
    }
}

impl Limits {
    /// 校验配额是否自洽。返回 `Err` 说明调用方给了一组不可能生效的值。
    ///
    /// ## 校验项及其理由
    ///
    /// - `max_memory_bytes == 0`：QuickJS 语义是"不设限"，与"必填护栏"矛盾。
    /// - `max_stack_bytes == 0`：同上（0 会让引擎立刻在第一次调用时报栈溢出）。
    /// - `gc_threshold_bytes > max_memory_bytes`：阈值永远不会达到，
    ///   等于 GC 只在内存耗尽时才跑——那正是"OOM 前的一次巨型 GC"，
    ///   与本文档要求的"短的不可中断窗口"相反。
    pub fn validate(&self) -> Result<(), LimitsError> {
        if self.max_memory_bytes == 0 {
            return Err(LimitsError::ZeroMemoryLimit);
        }
        if self.max_stack_bytes == 0 {
            return Err(LimitsError::ZeroStackLimit);
        }
        if self.gc_threshold_bytes == 0 {
            return Err(LimitsError::ZeroGcThreshold);
        }
        if self.gc_threshold_bytes > self.max_memory_bytes {
            return Err(LimitsError::GcThresholdAboveMemoryLimit {
                gc_threshold: self.gc_threshold_bytes,
                memory_limit: self.max_memory_bytes,
            });
        }
        Ok(())
    }
}

/// 配额本身不合法（配置错误，不是脚本错误）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LimitsError {
    /// 内存上限为 0（QuickJS 语义是"不设限"）
    #[error("内存上限不得为 0（QuickJS 把 0 解释为不设限）")]
    ZeroMemoryLimit,
    /// 栈上限为 0
    #[error("栈上限不得为 0")]
    ZeroStackLimit,
    /// GC 阈值为 0
    #[error("GC 阈值不得为 0")]
    ZeroGcThreshold,
    /// GC 阈值超过内存上限（阈值永远不会达到）
    #[error("GC 阈值（{gc_threshold} 字节）超过内存上限（{memory_limit} 字节），阈值永远不会达到")]
    GcThresholdAboveMemoryLimit {
        /// 配置的 GC 阈值
        gc_threshold: usize,
        /// 配置的内存上限
        memory_limit: usize,
    },
}

/// 沙箱错误（本 crate 的错误类型；在接缝处映射为 `tome_core::ErrorKind`）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxError {
    /// 脚本执行被中止（死循环、超时、或外部取消）。
    ///
    /// ## 为什么单独一个变体，而不是并入 `Script`
    ///
    /// 两者的**处置完全不同**：`Script` 是"这个书源的这条规则有问题"（用户该
    /// 去改规则），`Interrupted` 是"我们主动喊停"（用户该知道是超时/取消）。
    /// 混在一起会让"书源规则错"的错误提示指向错误的方向。故它是结构化的一类，
    /// 对应 `tome_core::ErrorKind::Rule` 但携带 `interrupted` 细节。
    #[error("脚本执行被中止：{reason}")]
    Interrupted {
        /// 中止原因（诊断用，非面向用户的文案）
        reason: String,
    },
    /// 脚本自身出错（语法错、抛出的异常、类型错等）。
    #[error("脚本错误：{message}")]
    Script {
        /// JS 侧的异常描述
        message: String,
    },
    /// 触及内存上限。
    #[error("触及内存上限（{limit} 字节）")]
    MemoryLimit {
        /// 生效的上限
        limit: usize,
    },
    /// 沙箱已废弃（曾触发 OOM，见 [`Sandbox::is_poisoned`]）。
    #[error("沙箱已废弃，不得复用（曾触发内存耗尽）")]
    Poisoned {
        /// 原始原因
        reason: String,
    },
    /// 配额配置不合法。
    #[error("沙箱配额不合法：{0}")]
    Limits(#[from] LimitsError),
    /// 创建运行时/上下文失败（资源不足等）。
    #[error("无法创建 JS 运行时：{reason}")]
    Setup {
        /// 诊断信息
        reason: String,
    },
}

impl SandboxError {
    /// 映射到 `tome_core::ErrorKind`（接缝处唯一出口）。
    ///
    /// 四类落到 core 的分类上：中止与脚本错都属 `Rule`（都源自书源的脚本），
    /// 内存上限属 `Rule`（脚本行为导致），配额不合法属 `Configuration`
    /// （是组合根的配置问题，不是书源的问题）。
    pub fn to_error_kind(&self) -> tome_core::ErrorKind {
        match self {
            Self::Interrupted { .. }
            | Self::Script { .. }
            | Self::MemoryLimit { .. }
            | Self::Poisoned { .. } => tome_core::ErrorKind::Rule,
            Self::Limits(_) | Self::Setup { .. } => tome_core::ErrorKind::Configuration,
        }
    }
}

/// 取消令牌（SC-013："用户可中止"）。
///
/// 只暴露 `cancel()` 与查询，**不暴露任何"复位"能力**：取消是单向的，
/// 允许复位会让"已经取消的脚本又跑起来"成为可能，而责任方无从追溯。
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// 新建未取消的令牌。
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消。幂等。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// 是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// 一次脚本求值的产物。
///
/// ## 为什么是"值 + 文本"而不是裸 `String`
///
/// 书源脚本的返回值形态多样：字符串（最常见）、数字（分页计算）、数组
/// （`chapterList` 那样的多值）。若在沙箱里就 `.to_string()`，数字
/// `2` 与字符串 `"2"` 会被压成同一种东西，而**调用方需要区分它们**
/// （`mainJs` 的入口约定靠返回类型判断结果形态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsValue {
    /// 按 JS 语义转成的文本
    ///
    /// - 字符串 → 原样
    /// - 数字/布尔 → 字面量（`"2"` / `"true"`）
    /// - `null` / `undefined` → **空串**（书源里大量出现"没取到就返回 undefined"，
    ///   把它变成 `"undefined"` 会让后续规则收到一个非空但无意义的值）
    /// - 数组/对象 → JSON 文本
    pub text: String,
    /// 值在 JS 里的类型名（`string` / `number` / `boolean` / `object` / `undefined` / `null`）
    pub kind: &'static str,
}

impl JsValue {
    /// 是否为未定义/空。
    ///
    /// 书源脚本里"取不到"与"取到空串"在**多数**场合同义，故上层用它做
    /// 空值判断。需要区分两者时读 [`Self::kind`]。
    pub fn is_nullish(&self) -> bool {
        self.text.is_empty() || self.kind == "undefined" || self.kind == "null"
    }

    /// 当作字符串用。
    pub fn as_str(&self) -> Option<&str> {
        (self.kind == "string").then_some(self.text.as_str())
    }

    /// 当作整数用（数字型值）。
    pub fn as_int(&self) -> Option<i64> {
        (self.kind == "number")
            .then(|| self.text.parse::<f64>().ok())
            .flatten()
            .map(|value| value as i64)
    }
}

/// QuickJS 沙箱：**一个书源脚本一个实例**（A1 / R1）。
///
/// # 为什么不做成可复用的全局实例
///
/// 宿主函数在 JS 侧**同步阻塞**（书源脚本是同步语义），阻塞期间该 runtime
/// 的锁被持有。共享一个 runtime 就意味着"一个源发请求，所有源停摆"。
/// 故隔离粒度是"每书源脚本一个 runtime"，由组合根保证。
pub struct Sandbox {
    runtime: Runtime,
    context: Context,
    limits: Limits,
    /// 取消令牌（上层可主动中止）
    token: CancelToken,
    /// 已废弃标记（触发过 OOM）
    poisoned: Arc<AtomicBool>,
    /// 废弃原因（诊断用）
    poison_reason: std::sync::Mutex<Option<String>>,
    /// 本次求值的墙钟起点（供中断 handler 判超时）
    ///
    /// 与 handler 共享同一个 `Arc`——两处各建一个会让 handler 永远读到
    /// `None`（见 `Sandbox::new` 里的说明）。
    deadline: Arc<std::sync::Mutex<Option<Instant>>>,
    /// 是否已开始装配（`install_prelude` / `with_context` 会置位）
    ///
    /// 与 `source_evaluated` 的区别见 `install_host_bridge` 的文档。
    evaluated: AtomicBool,
    /// 是否已跑过**书源脚本**（`eval` 会置位）
    ///
    /// 只有它守"宿主桥的注入时机"——prelude 是装配步骤，不算书源求值。
    source_evaluated: AtomicBool,
}

impl Sandbox {
    /// 以给定配额新建沙箱。
    ///
    /// 配额会**立即**落到 runtime 上（三道护栏一并设好），而不是等到第一次
    /// 求值——"先建后用"的窗口里若有别的代码路径能拿到 runtime，就绕过了护栏。
    pub fn new(limits: Limits) -> Result<Self, SandboxError> {
        limits.validate()?;

        let runtime = Runtime::new().map_err(|error| SandboxError::Setup {
            reason: format!("Runtime::new 失败：{error}"),
        })?;

        // 护栏之一：内存上限。禁止 rust-alloc/allocator feature 是它能生效的前提。
        runtime.set_memory_limit(limits.max_memory_bytes);
        // 护栏之二：栈上限。**显式**设置（理由见模块文档）。
        runtime.set_max_stack_size(limits.max_stack_bytes);
        // GC 阈值：决定单次 GC 的不可中断窗口长度（理由见模块文档）。
        runtime.set_gc_threshold(limits.gc_threshold_bytes);

        let poisoned = Arc::new(AtomicBool::new(false));
        // 护栏之三：中断 handler。
        //
        // 闭包每 10000 条字节码被调用一次，返回 true 即中止。
        // 判据有两类：外部取消（令牌）与超时（墙钟）。
        //
        // ## 为什么闭包不碰 `Sandbox` 自身的字段
        //
        // `set_interrupt_handler` 的闭包会在**沙箱正在执行**时被调用，此刻
        // 若它去锁 `Sandbox` 的 `Mutex` 字段，就会与调用栈上层的锁构成
        // 潜在死锁。故这里只捕获两个 `Arc`（无锁共享）。
        //
        // ## `deadline` 必须是**同一个** `Arc`
        //
        // 这个字段被两处读写：handler（读）与 `eval_inner`（写）。
        // 若构造时各建一个 `Arc<Mutex<..>>`，handler 会永远读到 `None`——
        // 具体表现是**死循环挂死且没有任何报错**，正是这条护栏要防的事故。
        // 故此处先建 `Arc`，再把它同时交给 handler 与 `Sandbox`。
        let deadline = Arc::new(std::sync::Mutex::new(None::<Instant>));
        let handler_token = CancelToken::new();
        let handler_state = Arc::new(InterruptState {
            external: handler_token.clone(),
            deadline: Arc::clone(&deadline),
        });
        {
            let state = Arc::clone(&handler_state);
            runtime.set_interrupt_handler(Some(Box::new(move || state.should_stop())));
        }

        let context = Context::full(&runtime).map_err(|error| SandboxError::Setup {
            reason: format!("Context::full 失败：{error}"),
        })?;

        Ok(Self {
            runtime,
            context,
            limits,
            token: handler_token,
            poisoned,
            poison_reason: std::sync::Mutex::new(None),
            deadline,
            evaluated: AtomicBool::new(false),
            source_evaluated: AtomicBool::new(false),
        })
    }

    /// 本沙箱生效的配额。
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// 取消令牌：交给上层（用户中止 / 源级超时）使用。
    pub fn cancel_token(&self) -> CancelToken {
        self.token.clone()
    }

    /// 是否已废弃（曾触发内存耗尽，不得复用）。
    ///
    /// ## 为什么 OOM 之后整个 runtime 都要丢
    ///
    /// QuickJS 在分配失败之后的清理路径不保证恢复全部内部不变量（它的
    /// `in_out_of_memory` 标志正是为了避免在 OOM 处理中递归），故"还能不能
    /// 继续跑下一个脚本"没有可靠的答案。治理方式是**不复用**：
    /// 上层丢掉这个沙箱、新建一个。代价是一次 runtime 构造，收益是
    /// 不用赌引擎的内部状态。
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst)
    }

    /// 注入宿主桥（A1：宿主能力由组合根注入）。
    ///
    /// # 为什么必须在书源脚本求值之前
    ///
    /// 宿主桥经 `Ctx::userdata` 存放，宿主函数在运行期取它。之后注入会让
    /// "之前那次求值里注册的函数"拿不到桥——而那种失败表现为运行期的
    /// "宿主桥未注入"，归因困难。故本方法在已有**书源脚本**求值发生过时
    /// 拒绝注入，把问题挡在配置阶段。
    ///
    /// ## `evaluated` 记的是"是否已开始装配"，不是"是否跑过书源脚本"
    ///
    /// 一个容易踩的坑：前导脚本（[`Self::install_prelude`]）也走 `eval`。
    /// 若把"任何 eval"都算作"已求值"，那么组合根的正常顺序
    /// （注 prelude → 注桥 → 跑脚本）会被自己的守卫挡下——
    /// 而那个顺序正是**推荐**的顺序（prelude 要在书源脚本之前注入）。
    ///
    /// 故 [`Self::eval`] 设 [`Self::source_evaluated`]，而 prelude 与
    /// [`Self::with_context`] 只设 `evaluated`（"装配已开始"）。
    /// 两者用途不同：前者守"桥的注入时机"，后者守"装配期方法的调用时机"。
    ///
    /// # 为什么用 `Ctx::userdata` 而不是闭包捕获
    ///
    /// 见 `crate::bridge` 的模块文档：闭包捕获会掉进 rquickjs 的嵌套
    /// 可变借用（宿主函数内再碰 `ctx` 就 panic）。
    pub fn install_host_bridge(
        &self,
        bridge: crate::bridge::HostBridge,
    ) -> Result<(), SandboxError> {
        if self.source_evaluated.load(Ordering::SeqCst) {
            return Err(SandboxError::Setup {
                reason: "宿主桥必须在首次书源脚本求值之前注入（之后的注入对已注册的宿主函数无效）"
                    .to_string(),
            });
        }
        let outcome = self.context.with(|ctx: Ctx<'_>| -> Result<(), String> {
            // `store_userdata` 返回**被替换掉的旧值**（若有）。重复注入时
            // 静默替换会让"两次注入不同配置"的问题不可见，故第二次注入
            // 直接拒绝——配置应当只有一次。
            match ctx.store_userdata(bridge) {
                Ok(None) => Ok(()),
                Ok(Some(_previous)) => Err(
                    "宿主桥已注入过；重复注入会让部分宿主函数持有旧桥（配置应只做一次）"
                        .to_string(),
                ),
                Err(error) => Err(format!("store_userdata 失败：{error}")),
            }
        });
        outcome.map_err(|reason| SandboxError::Setup { reason })
    }

    /// 注入兼容前导脚本（T035）。
    ///
    /// 由 [`Self::new`] 之外单独调用而不是内置在 `new` 里，是为了让
    /// "要不要注入"成为组合根的显式决定——某些场景（如跑一段纯计算脚本）
    /// 不需要补丁层，少注入一次就少一份可能与脚本自身定义冲突的表面。
    ///
    /// `[UNCERTAIN]`：目前没有发现"prelude 与书源脚本冲突"的真实案例，
    /// 若始终不出现，这个开关可以收敛掉（直接内置进 `new`）。
    pub fn install_prelude(&self) -> Result<(), SandboxError> {
        // ## 为什么不用 `self.eval(...)`
        //
        // `eval` 会置 `source_evaluated`（"书源脚本已求值"），而守卫
        // `install_host_bridge` 看的正是这个标志。组合根的**推荐**顺序是
        // "注 prelude → 注桥 → 跑书源脚本"，若 prelude 也算书源求值，
        // 那个顺序会被自己的守卫挡下。
        //
        // 故这里走一个不置位求值标志的内部路径。前导脚本是**装配步骤**，
        // 不是书源执行。
        self.evaluated.store(true, Ordering::SeqCst);
        self.context
            .with(|ctx: Ctx<'_>| -> Result<(), String> {
                match ctx.eval::<rquickjs::Value<'_>, _>(crate::prelude::PRELUDE_JS.as_bytes()) {
                    Ok(_) => Ok(()),
                    Err(rquickjs::Error::Exception) => {
                        let exception = ctx.catch();
                        let message = exception
                            .as_object()
                            .and_then(|object| object.get::<_, String>("message").ok())
                            .or_else(|| exception.as_string().and_then(|s| s.to_string().ok()))
                            .unwrap_or_else(|| "前导脚本执行失败".to_string());
                        Err(format!("前导脚本执行失败：{message}"))
                    }
                    Err(other) => Err(format!("前导脚本执行失败：{other}")),
                }
            })
            .map_err(|reason| SandboxError::Setup { reason })
    }

    /// 在沙箱的 JS 上下文里跑一段 Rust 代码（用于注册宿主函数、装配全局）。
    ///
    /// # 为什么必须经它而不是直接暴露 `Context`
    ///
    /// 暴露 `Context` 会让调用方绕开本模块的护栏与错误映射（`Ctx` 上的
    /// 任何操作都可能产生 rquickjs 的原始错误）。经这个入口，调用方拿到
    /// `Ctx` 但错误类型由自己定义，且明确处在"沙箱正在被配置"的语义下。
    ///
    /// # 借用语义（调用方必读）
    ///
    /// 回调执行期间 runtime 的 `RefCell` 被借出，故回调内**不得**调用
    /// 本沙箱的其它方法（`eval` / `memory_usage_bytes` 等）——那会
    /// `RefCell already borrowed` panic。回调只应操作 `Ctx` 与它自己的数据。
    ///
    /// # 为什么返回 `Result<T, String>`
    ///
    /// 装配期的失败原因五花八门（建对象、设属性、注册函数），且都是
    /// **组合根的配置问题**而非运行时错误。用 `String` 承载诊断信息，
    /// 由调用方决定如何呈现——本模块不替它选错误分类。
    pub fn with_context<T>(
        &self,
        callback: impl FnOnce(Ctx<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        if self.is_poisoned() {
            return Err("沙箱已废弃，不得再配置".to_string());
        }
        // 标记"已使用过"：让"求值之后才注入宿主桥"被挡在配置阶段
        // （见 `install_host_bridge` 的说明）。
        self.evaluated.store(true, Ordering::SeqCst);
        self.context.with(callback)
    }

    /// 求值一段脚本，返回其值。
    ///
    /// # 中断语义
    ///
    /// 每次求值开始时会设定截止时刻（若配了墙钟上限），求值结束时清空。
    /// 中断有两种来源：外部 [`CancelToken::cancel`] 与超时，**两者都产出
    /// [`SandboxError::Interrupted`]**——因为它们对调用方的含义相同（"停下来了，
    /// 但不是脚本自己的错"），区别只在 [`SandboxError::Interrupted::reason`] 里。
    ///
    /// # 为什么"已废弃"的沙箱还要能调用本方法
    ///
    /// 它返回 [`SandboxError::Poisoned`] 而不是 panic——调用方（书源管线）
    /// 需要的是一个**可捕获的错误**，好把它当成"这个源这次失败了"处理。
    pub fn eval(&self, source: &str) -> Result<JsValue, SandboxError> {
        self.eval_inner(source).map_err(|error| {
            // 内存耗尽是唯一需要"废弃整个 runtime"的错误（理由见 `is_poisoned`）。
            if matches!(error, SandboxError::MemoryLimit { .. }) {
                self.poison("脚本触发内存耗尽；按设计该 runtime 整体丢弃不复用");
            }
            error
        })
    }

    /// 求值的实现主体（错误处理见 [`Self::eval`]）。
    fn eval_inner(&self, source: &str) -> Result<JsValue, SandboxError> {
        if self.is_poisoned() {
            let reason = self
                .poison_reason
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .unwrap_or_else(|| "未知原因".to_string());
            return Err(SandboxError::Poisoned { reason });
        }
        if self.token.is_cancelled() {
            return Err(SandboxError::Interrupted {
                reason: "求值开始前沙箱已被取消".to_string(),
            });
        }

        // 设定本次求值的截止时刻（中断 handler 读它判超时）。
        self.set_deadline();
        // 标记"书源脚本已求值"：让"之后才注入宿主桥"被挡在配置阶段
        // （见 `install_host_bridge` 的说明）。同时置 `evaluated`——
        // 跑过书源当然说明装配已开始。
        self.source_evaluated.store(true, Ordering::SeqCst);
        self.evaluated.store(true, Ordering::SeqCst);

        // ## `with` 作用域内的三条硬约束
        //
        // `Context::with` 在**整个闭包期间**持有 `runtime.inner` 的 `RefCell`
        // 可变借用，而 `rquickjs::Runtime` 的任何方法都要再借一次同一个
        // `RefCell`。故闭包内**不得**调用本结构上与 runtime 相关的方法
        // （`memory_usage_bytes` 之类）——那会 `RefCell already borrowed` panic。
        //
        // 这直接决定了异常分类的实现形态：闭包里只**采集**（异常消息、类型名、
        // 值形态），分类与判定放在闭包**之后**（见下方 `classify_exception`）。
        //
        // 另一条约束：`Ctx`/`Value` 的借用不能活到闭包之外，故采集结果必须是
        // 与借期无关的所有权数据（这里用 `Collected` 结构承载）。
        let outcome = self
            .context
            .with(|ctx: Ctx<'_>| -> Result<JsValue, Collected> {
                match ctx.eval::<rquickjs::Value<'_>, _>(source.as_bytes()) {
                    Ok(value) => Ok(describe_value(&ctx, &value)),
                    Err(rquickjs::Error::Exception) => Err(collect_exception(&ctx)),
                    // 非 JS 异常的引擎错误（如参数类型不符）没有可分类的 JS 侧信息，
                    // 直接带上引擎的消息。
                    Err(other) => Err(Collected::Engine(other.to_string())),
                }
            });

        // 离开 `with` 之后 runtime 借用已释放，此刻才能安全地读内存用量。
        let outcome = outcome.map_err(|collected| self.classify_exception(collected));

        self.clear_deadline();
        outcome
    }

    /// 把（已采集的）异常分类成结构化错误。
    ///
    /// ## 为什么参数是 `Collected` 而不是 `Ctx`
    ///
    /// 本方法必须在 `Context::with` **之外**调用：分类要用到 runtime 的内存用量，
    /// 而 `with` 期间 runtime 的 `RefCell` 已被借用（见 `eval_inner` 的说明）。
    /// 故闭包内只采集所有权数据，分类在这里做。
    ///
    /// ## 分类判据的来历（源码核实，非猜测）
    ///
    /// - **中断**：`JS_ThrowInterrupted` 抛的是 `InternalError`，消息是写死的
    ///   `"interrupted"`，且带 `JS_SetUncatchableError` 标志。
    /// - **内存耗尽**：QuickJS 在分配失败时抛 `InternalError`，消息形如
    ///   `"out of memory"`。
    /// - 其余一律 `Script`。
    ///
    /// ## 为什么除消息之外还要看内存用量
    ///
    /// 有些 OOM 路径抛出的是普通 `Error` 而非 `InternalError`（取决于在哪个
    /// 分配点失败）。故分类时**同时**看"内存用量是否已贴近上限"，两条证据任一
    /// 成立即判为 [`SandboxError::MemoryLimit`]。这比只信消息文本稳健——
    /// 文本会随 QuickJS 版本变化，而内存用量不会。
    fn classify_exception(&self, collected: Collected) -> SandboxError {
        let Collected::JsException { name, message } = collected else {
            // 引擎级错误（非 JS 异常）：没有可分类的 JS 侧信息。
            let Collected::Engine(message) = collected else {
                unreachable!("`Collected` 只有两个变体，上面已排除其一")
            };
            return SandboxError::Script { message };
        };

        // 判据一：QuickJS 的中断字面量（见 `JS_ThrowInterrupted`）
        if name == "InternalError" && message.contains("interrupted") {
            return SandboxError::Interrupted {
                reason: self.interrupt_reason(),
            };
        }

        // 判据二：OOM 的两种证据（消息 + 内存用量贴近上限）
        let usage = self.memory_usage_bytes();
        // 用 u128 做中间类型：`malloc_size` 与上限各自乘 10 都可能在
        // `usize` 上溢出（书源给的荒谬上限 + 极大的实际用量），
        // 而这是一个"比较"而非"计算"，不应用饱和语义掩盖溢出。
        let near_limit = (usage as u128) * 10 >= (self.limits.max_memory_bytes as u128) * 9;
        if (name == "InternalError" && message.contains("out of memory")) || near_limit {
            return SandboxError::MemoryLimit {
                limit: self.limits.max_memory_bytes,
            };
        }

        SandboxError::Script { message }
    }

    /// 给中断附上可读原因（超时 / 外部取消）。
    fn interrupt_reason(&self) -> String {
        let timed_out = self
            .limits
            .wall_clock_timeout
            .is_some_and(|_| self.deadline_expired());
        match (timed_out, self.token.is_cancelled()) {
            (true, true) => "超时且已被外部取消".to_string(),
            (true, false) => match self.limits.wall_clock_timeout {
                Some(timeout) => format!("执行超过墙钟上限 {timeout:?}"),
                None => "执行超时".to_string(),
            },
            (false, true) => "被外部取消".to_string(),
            // 两者都不成立：中断信号来自别处（例如 QuickJS 内部的保护）。
            (false, false) => "引擎报告中断（非超时、非外部取消）".to_string(),
        }
    }

    /// 是否已过截止时刻。
    fn deadline_expired(&self) -> bool {
        self.deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// 设定本次求值的截止时刻。
    fn set_deadline(&self) {
        let deadline = self
            .limits
            .wall_clock_timeout
            .map(|timeout| Instant::now() + timeout);
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = deadline;
    }

    /// 清空截止时刻（求值结束）。
    fn clear_deadline(&self) {
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// 标记沙箱为已废弃。幂等，首次调用的原因被保留。
    fn poison(&self, reason: &str) {
        let mut slot = self
            .poison_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(reason.to_string());
        }
        self.poisoned.store(true, Ordering::SeqCst);
    }

    /// 运行时的内存用量（诊断与测试用）。
    ///
    /// `malloc_size` 是 `i64`（QuickJS 的 `JS_MemoryUsage` 用它表达字节数）。
    /// 负数在现实中不会出现，但**生产路径禁止 panic**，故用 `max(0)`
    /// 把它钳进 `usize` 而不是 `try_into().unwrap()`。
    pub fn memory_usage_bytes(&self) -> usize {
        usize::try_from(self.runtime.memory_usage().malloc_size.max(0)).unwrap_or(usize::MAX)
    }
}

/// 中断判定所需的共享状态（handler 与沙箱两侧都持有 `Arc`）。
///
/// 单独成结构而不是让闭包直接捕获两个 `Arc`：这样"判定逻辑"只有一处，
/// 沙箱侧（构造时）与 handler 侧读的是同一份实现。
struct InterruptState {
    /// 外部取消令牌
    external: CancelToken,
    /// 本次求值的截止时刻（`None` 表示无时限或不在求值中）
    deadline: Arc<std::sync::Mutex<Option<Instant>>>,
}

impl InterruptState {
    /// 是否应当中止。由 QuickJS 每 10000 条字节码调用一次。
    fn should_stop(&self) -> bool {
        if self.external.is_cancelled() {
            return true;
        }
        self.deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

/// `Context::with` 闭包内**采集**到的异常信息（所有权数据，与借期无关）。
///
/// ## 为什么要有这个中间类型
///
/// 见 `Sandbox::eval_inner` 的注释：闭包内不能碰 runtime（`RefCell`
/// 已借用），且 `Ctx`/`Value` 借用不能外泄。故闭包只做"读出来"，分类放在
/// 借用释放之后。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Collected {
    /// JS 侧抛出的异常（已读出 `name` 与 `message`）
    JsException {
        /// 异常的 `name`（`TypeError` / `InternalError` …；非 Error 对象时为空）
        name: String,
        /// 可读消息
        message: String,
    },
    /// 引擎级错误（非 JS 异常）
    Engine(String),
}

/// 从当前 JS 异常值采集分类所需的信息。
///
/// 抛出的可能不是 `Error` 对象（书源里 `throw "字符串"` 很常见），故消息的
/// 取法分三种：`Error.message` → 字符串值本身 → JSON 序列化兜底。
fn collect_exception<'js>(ctx: &Ctx<'js>) -> Collected {
    let exception = ctx.catch();
    let name = exception
        .as_object()
        .and_then(|object| object.get::<_, String>("name").ok())
        .unwrap_or_default();

    let message = exception
        .as_object()
        .and_then(|object| object.get::<_, String>("message").ok())
        .filter(|text| !text.is_empty())
        .or_else(|| exception.as_string().and_then(|s| s.to_string().ok()))
        // 兜底：用引擎自己的 JSON 序列化（比 `Debug` 更接近 JS 侧看到的东西）
        .or_else(|| {
            ctx.json_stringify(exception.clone())
                .ok()
                .flatten()
                .and_then(|s| s.to_string().ok())
        })
        .unwrap_or_else(|| "<无法读取的异常值>".to_string());

    Collected::JsException { name, message }
}

/// 把一个 JS 值转成与借期无关的 [`JsValue`]。
///
/// ## 为什么签名里 `Ctx` 与 `Value` 共用同一个生命周期参数
///
/// `rquickjs::Ctx<'js>` 与 `Value<'js>` 都对 `'js` **不变**（invariant），
/// 故 `&Ctx<'_>` / `&Value<'_>` 这样的两个匿名生命周期无法被推导成同一个——
/// 必须显式写成 `<'js>` 让编译器知道它们是同一份运行时数据的两个视图。
///
/// 转换规则及其理由见 [`JsValue::text`] 的文档。
pub(crate) fn describe_value<'js>(ctx: &Ctx<'js>, value: &rquickjs::Value<'js>) -> JsValue {
    use rquickjs::Type;

    let kind = match value.type_of() {
        Type::Uninitialized => "uninitialized",
        Type::Undefined => "undefined",
        Type::Null => "null",
        Type::Bool => "boolean",
        Type::Int | Type::Float => "number",
        Type::String => "string",
        Type::Symbol => "symbol",
        Type::Array => "array",
        Type::Constructor | Type::Function => "function",
        Type::Object | Type::Exception | Type::BigInt | Type::Module | Type::Promise => "object",
        // `Type` 是 `#[non_exhaustive]` 之外的类型：给一个中性名，
        // 不 panic（未来 rquickjs 新增类型不该弄崩引擎）。
        _ => "unknown",
    };

    let text = if matches!(kind, "string") {
        value.as_string().and_then(|s| s.to_string().ok())
    } else if matches!(kind, "number" | "boolean") {
        // 数字/布尔：**不加引号**。`ctx.json_stringify` 会产出 `"2"`（带引号）
        // 与 `2`，而书源脚本的返回值多用于拼接与比较，带引号会污染结果。
        value
            .as_string()
            .and_then(|s| s.to_string().ok())
            .or_else(|| Some(value.type_of().to_string_value(value)))
    } else if matches!(kind, "null" | "undefined" | "uninitialized") {
        Some(String::new())
    } else {
        // 对象/数组/函数等：JSON 文本。序列化失败（循环引用）时退回空串——
        // 抛错会让"脚本返回了一个循环结构"变成整条源失败，而那种场合
        // 语义上更接近"没有可用结果"。
        ctx.json_stringify(value.clone())
            .ok()
            .flatten()
            .and_then(|s| s.to_string().ok())
    };

    JsValue {
        text: text.unwrap_or_default(),
        kind,
    }
}

/// `Type` 到数值字面量文本的转换（`describe_value` 的辅助）。
trait TypeText {
    /// 取该值的文本形态（数字/布尔专用）。
    fn to_string_value(self, value: &rquickjs::Value<'_>) -> String;
}

impl TypeText for rquickjs::Type {
    fn to_string_value(self, value: &rquickjs::Value<'_>) -> String {
        match self {
            rquickjs::Type::Bool => value.as_bool().unwrap_or(false).to_string(),
            rquickjs::Type::Int => value
                .as_int()
                .map(|number| number.to_string())
                .unwrap_or_default(),
            rquickjs::Type::Float => value.as_float().map(format_number).unwrap_or_default(),
            _ => String::new(),
        }
    }
}

/// 数字格式化：整数不带小数点。
///
/// `1.0` 在 JS 里就是 `1`，`f64::to_string` 对整数会给出 `"1"`（Rust 的行为），
/// 但浮点运算的产物可能是 `1.0000000000000002` 这类值——那正是 JS 的
/// `Number.prototype.toString` 也会给出的东西，故此处不做额外规范化。
fn format_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认配额是有界的() {
        let limits = Limits::default();
        assert!(limits.validate().is_ok(), "默认配额必须自洽");
    }

    #[test]
    fn 零内存上限被拒绝() {
        // QuickJS 把 0 解释为"不设限"，而这与"必填护栏"的意图相反：
        // 静默变成不设限会让调用方以为有护栏。
        let limits = Limits {
            max_memory_bytes: 0,
            ..Limits::default()
        };
        assert!(matches!(
            limits.validate(),
            Err(LimitsError::ZeroMemoryLimit)
        ));
    }

    #[test]
    fn gc_阈值超过内存上限被拒绝() {
        let limits = Limits {
            max_memory_bytes: 1024,
            gc_threshold_bytes: 2048,
            ..Limits::default()
        };
        assert!(matches!(
            limits.validate(),
            Err(LimitsError::GcThresholdAboveMemoryLimit { .. })
        ));
    }

    #[test]
    fn 取消令牌是单向的() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled(), "重复取消应幂等");
    }

    #[test]
    fn 中断状态响应外部取消() {
        let state = InterruptState {
            external: CancelToken::new(),
            deadline: Arc::new(std::sync::Mutex::new(None)),
        };
        assert!(!state.should_stop());
        state.external.cancel();
        assert!(state.should_stop());
    }

    #[test]
    fn 中断状态响应截止时刻() {
        let state = InterruptState {
            external: CancelToken::new(),
            deadline: Arc::new(std::sync::Mutex::new(Some(
                Instant::now() - Duration::from_millis(1),
            ))),
        };
        assert!(state.should_stop(), "已过截止时刻应中止");

        let state = InterruptState {
            external: CancelToken::new(),
            deadline: Arc::new(std::sync::Mutex::new(Some(
                Instant::now() + Duration::from_secs(60),
            ))),
        };
        assert!(!state.should_stop(), "未到截止时刻不应中止");
    }

    #[test]
    fn 数字格式化去掉整数的小数点() {
        assert_eq!(format_number(2.0), "2");
        assert_eq!(format_number(-3.0), "-3");
        assert_eq!(format_number(2.5), "2.5");
        // 极大值不做整数化（超过 i64 安全范围）
        assert_eq!(format_number(1e20), "100000000000000000000");
    }
}
