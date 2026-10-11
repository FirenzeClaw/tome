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

// ---------------------------------------------------------------------------
// 帧时间分布（SC-005 的口径）
// ---------------------------------------------------------------------------

/// 帧时间采样器（**滑动窗口**，不在内存里攒无限多的样本）。
///
/// ## 为什么需要它（而不是"看着流畅"）
///
/// SC-005 说的是"1000 册下滚动与标题过滤保持 60 fps（掉帧率 < 1%，
/// 按帧时间分布计）"。那个口径要求的是**分布**而不是平均值——平均值会把
/// "99 帧 8ms + 1 帧 500ms" 报成"平均 13ms，看起来还行"，而用户感受到的
/// 正是那 1 帧的卡顿。
///
/// 故本类型收的是分位数：p50 / p95 / p99 / 最大值 + 超 16.67ms 的帧占比。
/// 掉帧率的判据是"帧时间 > 一帧预算（16.67ms）"的占比。
#[derive(Debug)]
pub struct FrameSampler {
    /// 最近 N 帧的耗时（毫秒；环形缓冲的简化形态——满了就丢掉最旧的一半）
    samples: Vec<f64>,
    /// 窗口容量
    capacity: usize,
}

impl FrameSampler {
    /// 新建采样器（默认窗口 600 帧 ≈ 10 秒的 60fps）。
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    /// 记一帧。
    ///
    /// 窗口满时丢掉**最旧的一半**而不是逐个丢弃：逐个丢弃会让 `samples`
    /// 每次都要搬移全部元素（O(n) 的 memmove 每帧一次）。丢一半是摊还 O(1)
    /// 的，且分布统计对"窗口里是最近 300 帧还是 600 帧"不敏感。
    pub fn record(&mut self, frame_ms: f64) {
        if self.samples.len() >= self.capacity {
            self.samples.drain(..self.capacity / 2);
        }
        self.samples.push(frame_ms);
    }

    /// 样本数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 是否还没采到样本。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 分位数（`q` 取 0.0–1.0），用**最近秩**法（nearest rank）。
    ///
    /// 空样本返回 `None`——**不返回 0.0**：那会让"还没测"与"零耗时"混为一谈。
    ///
    /// ## 为什么是最近秩而不是插值
    ///
    /// 插值（如 `(n-1)*q` 后线性内插）会产出**样本里不存在的值**——对帧时间
    /// 而言那是编造出来的数字（"p50 = 12.37ms" 而实际没有任何一帧是 12.37ms）。
    /// 最近秩只从真实样本里挑一个，代价是分辨率受样本数限制，而对"掉帧率
    /// < 1%"这类判据来说，样本真实比分辨率精细重要得多。
    ///
    /// 秩的取法：`ceil(q * n) - 1`（`q = 0` 时饱和到 0）。这是该方法的
    /// 标准定义，故 `p50`（n=100）取第 50 个样本而不是第 51 个。
    #[must_use]
    pub fn percentile(&self, q: f64) -> Option<f64> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len() as f64;
        let rank = (q.clamp(0.0, 1.0) * n).ceil() as usize;
        let index = rank.saturating_sub(1).min(sorted.len() - 1);
        sorted.get(index).copied()
    }

    /// 最大值。
    #[must_use]
    pub fn max(&self) -> Option<f64> {
        self.samples.iter().copied().max_by(f64::total_cmp)
    }

    /// 超过 `budget_ms` 的帧占比（SC-005 的"掉帧率"口径）。
    #[must_use]
    pub fn over_budget_ratio(&self, budget_ms: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let over = self
            .samples
            .iter()
            .filter(|sample| **sample > budget_ms)
            .count();
        over as f64 / self.samples.len() as f64
    }

    /// 一行机器可读的分布摘要。
    #[must_use]
    pub fn summary(&self, label: &str) -> String {
        let p50 = self.percentile(0.50).unwrap_or(0.0);
        let p95 = self.percentile(0.95).unwrap_or(0.0);
        let p99 = self.percentile(0.99).unwrap_or(0.0);
        let max = self.max().unwrap_or(0.0);
        let dropped = self.over_budget_ratio(FRAME_BUDGET_MS) * 100.0;
        format!(
            "{label} frames={} p50={p50:.2}ms p95={p95:.2}ms p99={p99:.2}ms max={max:.2}ms dropped={dropped:.2}%",
            self.len()
        )
    }
}

/// 60fps 的一帧预算（毫秒）。
pub const FRAME_BUDGET_MS: f64 = 1000.0 / 60.0;

/// 报告一次帧时间分布（未启用探针时什么都不做）。
pub fn report_frames(sampler: &FrameSampler, label: &str) {
    if is_enabled() && !sampler.is_empty() {
        eprintln!("[timing] {}", sampler.summary(label));
    }
}
