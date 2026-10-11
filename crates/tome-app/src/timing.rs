//! `[WIP]` 启动与首帧的计时探针（S3 T016 的测量设施）
//!
//! ## 为什么需要它（而不是"看着表估一个数"）
//!
//! SC-001（冷启动 ≤1.5s）与 SC-002（首屏 ≤400/800ms）是**可复现**的指标，
//! 而"肉眼看着窗口出现"不是测量——它受桌面合成器、窗口动画、人的反应时间
//! 三重污染，且无法在回归时重跑同一口径。
//!
//! 本模块把两个口径钉成**代码里的两个时刻**：
//!
//! | 指标 | 起点 | 终点 | 口径依据 |
//! |---|---|---|---|
//! | SC-001 冷启动 | `main()` 第一行 | 书架事件抵达（可响应输入） | spec §SC-001「进程启动到书架视图可响应输入」 |
//! | SC-002 首屏 | 发出 `ReadingOpen` | 收到 `ChapterReady` | 002 §SC-008 沿用（内容已在本机；含排版+分页，不含抓取） |
//!
//! ## 为什么用 stderr 而不是日志文件
//!
//! `tome-app` 没设 `#![windows_subsystem = "windows"]`，故从终端启动时
//! stderr 直接可见——`TOME_TIMING=1 cargo run -p tome-app` 的输出就是原始
//! 测量记录，可以直接贴进回执。写日志文件反而引入"文件在哪、什么时候轮转"
//! 这类与测量无关的问题。
//!
//! ## 为什么默认关闭
//!
//! 探针本身的开销可忽略（几次 `Instant::now()`），但它的**输出**会污染正常
//! 使用的终端。用环境变量门控：默认零输出，需要测量时显式打开。这也让
//! "这个构建是否带探针"不再是一个需要分支的问题。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// 探针是否启用（`TOME_TIMING` 环境变量）。
static ENABLED: AtomicBool = AtomicBool::new(false);

/// 进程启动时刻（`main()` 第一行调用一次）。
static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// 记录进程启动（`main()` 的**第一行**）。
///
/// ## 为什么必须由 `main` 显式调用，而不是在首次 `elapsed` 时惰性初始化
///
/// 惰性初始化会把"第一次调用探针的时刻"当成起点，而那是**窗口创建之后**
/// ——冷启动的绝大部分耗时（数据目录判定、建库、迁移、起引擎线程）恰好都
/// 在它之前。那样测出来的数字会好看得多，但它测的不是 SC-001 说的那件事。
pub fn mark_process_start() {
    let enabled = std::env::var("TOME_TIMING").is_ok_and(|value| value == "1");
    ENABLED.store(enabled, Ordering::Relaxed);
    let _ = ORIGIN.set(Instant::now());
}

/// 探针是否启用。
#[must_use]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 距进程启动的毫秒数。
#[must_use]
pub fn since_start_ms() -> f64 {
    ORIGIN
        .get()
        .map_or(0.0, |origin| origin.elapsed().as_secs_f64() * 1000.0)
}

/// 打印一条测量记录（未启用时什么都不做）。
///
/// 输出格式刻意是**机器可读的一行**（`[timing] <标签> <毫秒>`）：这样
/// `grep '^\[timing\]'` 就能把测量记录从其它输出里摘出来，而人也能直接读。
pub fn report(label: &str, elapsed_ms: f64) {
    if is_enabled() {
        eprintln!("[timing] {label} {elapsed_ms:.1}ms");
    }
}

/// 记录"距进程启动"的一个里程碑。
pub fn milestone(label: &str) {
    if is_enabled() {
        eprintln!("[timing] {label} {:.1}ms", since_start_ms());
    }
}

/// 一次计时的起点（用于 SC-002 那种"从某个动作到某个结果"的口径）。
#[derive(Debug, Clone, Copy)]
pub struct Span {
    /// 起点
    started: Instant,
}

impl Span {
    /// 起一次计时。
    #[must_use]
    pub fn start() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    /// 结束并报告。
    pub fn finish(self, label: &str) {
        report(label, self.started.elapsed().as_secs_f64() * 1000.0);
    }

    /// 已过去的毫秒数（不报告）。
    #[must_use]
    pub fn elapsed_ms(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }
}
