//! `[WIP]` 进度记忆与节流落盘（S3 T015）
//!
//! ## 为什么不能"每次翻页都落盘"
//!
//! 翻页是阅读器里最高频的动作。每次翻页写一次 SQLite，代价有两层：
//!
//! 1. **直接代价**——一次事务（含 fsync 语义）是毫秒级，而翻页的手感预算
//!    只有几十毫秒；
//! 2. **间接代价**——写操作全部走 `Db` 的**单写者 actor 线程**（001 的
//!    既定形态），于是翻页产生的写请求会和书架加载、章节缓存落盘排在同一条
//!    队列上。翻页越频繁，其它操作被推得越后。
//!
//! 故按**节流窗口**落盘（FR-009），而窗口内的位置只留在内存里。
//!
//! ## 为什么不能"只在退出时落盘"
//!
//! 那会让 SC-009 的后半句（"异常退出后位置回退 ≤ 一个节流窗口"）变成
//! "回退到本书开头"——用户杀一次进程就丢掉整本书的阅读位置。窗口是这两端
//! 的折中：**最多丢一个窗口的量**。
//!
//! ## 窗口值为什么是 3 秒
//!
//! spec 的假设节把窗口值留到任务阶段定（"预期秒级"），T015 定为 **3 秒**。
//! 定这个数的依据是两端的实际量级：
//!
//! - **上界**——用户"杀进程"的动机通常是应用卡住了。3 秒的丢失量在阅读
//!   语境里是"一两屏"，重开之后很容易找回；10 秒就可能是"半章"。
//! - **下界**——连续翻页时（每秒 2–3 次）3 秒窗口把落盘次数压到 1/6 以下，
//!   已经吃掉了绝大部分收益；再放大对写队列的改善是边际的。
//!
//! 调整路径：改 [`PROGRESS_FLUSH_WINDOW_MS`] 一处即可——本模块的判定与
//! 调度都以它为唯一输入（`tests/progress.rs` 有窗口边界断言）。
//!
//! ## 为什么时钟要注入
//!
//! "3 秒到了没有"若直接读 `Instant::now()`，测试就只能靠 `sleep(3s)` 来验，
//! 而那是**慢且不稳**的（CI 上尤其）。这里把"现在几毫秒"作为参数传进来，
//! 于是窗口边界、跨越、回退等情形都是一次函数调用——测试瞬时且确定。

use std::time::Duration;

use crate::engine::messages::ReadingAnchor;

/// 进度落盘的节流窗口（毫秒）。
///
/// 见模块文档"窗口值为什么是 3 秒"。**这是唯一的口径来源**——SC-009 的
/// "回退 ≤ 一个节流窗口"以它为准。
pub const PROGRESS_FLUSH_WINDOW_MS: u64 = 3_000;

/// 节流窗口（`Duration` 形态，供调度用）。
#[must_use]
pub fn flush_window() -> Duration {
    Duration::from_millis(PROGRESS_FLUSH_WINDOW_MS)
}

/// 一本书的进度记忆状态（**纯值**，headless 可测）。
///
/// ## 生命周期
///
/// ```text
///   open(book, anchor, now)          打开书：以上次恢复的位置为基线（干净）
///        ↓ observe(new_anchor)       翻页/滚动/跳转：置脏，记下待写的值
///        ↓ due(now)?                 到窗口了吗
///        ↓ mark_flushed(now)         落盘完成
///   take_pending()                   退出前冲刷：取出待写值（无则 None）
/// ```
#[derive(Debug, Clone)]
pub struct ProgressTracker {
    /// 当前书
    book_id: String,
    /// 待落盘的位置（`dirty` 为真时有效）
    anchor: ReadingAnchor,
    /// 是否有未落盘的位置变化
    dirty: bool,
    /// 上次落盘的时刻（毫秒；由调用方注入的时钟给出）
    last_flush_ms: u64,
}

impl ProgressTracker {
    /// 打开一本书：以上次恢复的位置为基线（**干净**状态）。
    ///
    /// ## 为什么基线取恢复到的位置而不是 0
    ///
    /// 打开书本身不该产生一次落盘——刚读到的位置就是库里已有的那个值。
    /// 若基线取 0，则"打开书 → 立刻退出"会写一次与库里相同的数据；更糟的是
    /// 它把 `last_flush_ms` 归零，于是**下次翻页会立刻触发落盘**（看起来像
    /// 节流失效）。基线取恢复值让这两件事都自然成立。
    #[must_use]
    pub fn open(book_id: impl Into<String>, anchor: ReadingAnchor, now_ms: u64) -> Self {
        Self {
            book_id: book_id.into(),
            anchor,
            dirty: false,
            last_flush_ms: now_ms,
        }
    }

    /// 当前书。
    #[must_use]
    pub fn book_id(&self) -> &str {
        &self.book_id
    }

    /// 待落盘/已落盘的位置。
    #[must_use]
    pub fn anchor(&self) -> ReadingAnchor {
        self.anchor
    }

    /// 是否有未落盘的变化。
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// 观察到一个新位置。
    ///
    /// 位置**未变**时不置脏：翻页到章末再点一次（或滚动到同一屏）不该产生
    /// 一次落盘。这条不是优化，是正确性——否则"按住方向键"会在每次重复
    /// 按键后写一次同样的值。
    pub fn observe(&mut self, anchor: ReadingAnchor) {
        if anchor != self.anchor {
            self.anchor = anchor;
            self.dirty = true;
        }
    }

    /// 是否到了该落盘的时刻。
    ///
    /// 判据是 `elapsed >= window`（闭区间）：窗口到点即落，不留"恰好 3000ms
    /// 时还差一点"的边界歧义。
    #[must_use]
    pub fn due(&self, now_ms: u64) -> bool {
        self.dirty && now_ms.saturating_sub(self.last_flush_ms) >= PROGRESS_FLUSH_WINDOW_MS
    }

    /// 距下次该落盘还有多久（`None` = 无需定时唤醒）。
    ///
    /// ## 为什么返回剩余时间而不是固定轮询间隔
    ///
    /// 固定间隔轮询（如每 250ms 醒一次）会让"落盘时刻"最多晚一个间隔，
    /// 于是 SC-009 的丢失量变成"窗口 + 间隔"。返回剩余时间让引擎线程**恰好**
    /// 在窗口到点时醒来，丢失量就是窗口本身。
    #[must_use]
    pub fn next_deadline_in(&self, now_ms: u64) -> Option<Duration> {
        if !self.dirty {
            return None;
        }
        let elapsed = now_ms.saturating_sub(self.last_flush_ms);
        let remaining = PROGRESS_FLUSH_WINDOW_MS.saturating_sub(elapsed);
        Some(Duration::from_millis(remaining))
    }

    /// 标记"已落盘"（写成功后调用）。
    ///
    /// 只有**写成功**才该调它。写失败时保持脏，下次到点会重试——若失败也
    /// 标记干净，一次瞬时 IO 错误就会让位置静默停留在旧值。
    pub fn mark_flushed(&mut self, now_ms: u64) {
        self.dirty = false;
        self.last_flush_ms = now_ms;
    }

    /// 推后一次尝试（写失败时调用）。
    ///
    /// ## 为什么写失败不能保持"到点"状态
    ///
    /// `due()` 的判据是 `now - last_flush >= window`。写失败时若不动
    /// `last_flush`，下一次判据**立刻**为真——于是引擎线程的定时唤醒变成
    /// 忙循环：每个 tick 都失败、都立刻再试，把 CPU 烧在一个注定失败的操作上
    /// （磁盘满、库只读这类失败不会自愈）。
    ///
    /// 推后一整个窗口既保住"脏"（数据不丢），又把重试频率限制在窗口粒度。
    pub fn defer(&mut self, now_ms: u64) {
        self.last_flush_ms = now_ms;
    }

    /// 取出待冲刷的位置（退出前冲刷用）。
    ///
    /// 返回 `None` 表示"没有未落盘的变化"，调用方据此跳过写操作。
    /// **不**清脏——清脏由 [`Self::mark_flushed`] 在写成功后做，理由同上。
    #[must_use]
    pub fn take_pending(&self) -> Option<(String, ReadingAnchor)> {
        if self.dirty {
            Some((self.book_id.clone(), self.anchor))
        } else {
            None
        }
    }
}

/// 单调时钟（毫秒），从构造时刻起算。
///
/// ## 为什么不用 `SystemTime`
///
/// 节流判据是"两个时刻的差"。`SystemTime` 会在 NTP 校时或用户改系统时间时
/// **倒退**，而倒退会让 `elapsed` 变小甚至为负——症状是"改完系统时间之后
/// 进度不再落盘"。`Instant` 是单调的，正是这个用途该用的东西。
#[derive(Debug)]
pub struct MonotonicClock {
    origin: std::time::Instant,
}

impl MonotonicClock {
    /// 以当前时刻为原点。
    #[must_use]
    pub fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }

    /// 从原点起算的毫秒数。
    #[must_use]
    pub fn now_ms(&self) -> u64 {
        // 饱和转换：进程运行 5.8 亿年才会溢出 `u64` 毫秒，此处不构成问题。
        u64::try_from(self.origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}
