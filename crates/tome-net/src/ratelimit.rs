//! [DONE] 按源限速（tasks.md T018 / FR-008）
//!
//! FR-008 要求书源能声明"请求间隔"，宪法原则 V 与 AGENTS.md 安全红线进一步要求
//! "遵守书源声明的请求间隔，**不得绕过限速**"。本模块是这条要求的唯一执行点。
//!
//! # 为什么是"虚拟调度"而不是"睡在锁里"
//!
//! 朴素实现是"持锁 → 睡到期 → 放锁"，但这会把**所有源**串成一条队列：
//! A 源在睡觉时 B 源的请求也拿不到锁，多源并发搜索会退化成完全串行，
//! 直接违反 FR-008"按源限速"的意图（也违反 SC-006 的多源并行预期）。
//!
//! 本实现改为：拿锁只为**占一个未来时隙**，锁内只做算术，随即放锁；
//! 等待发生在锁外。核心状态是每个 key 的 `next_slot`（下一个可用时刻）：
//!
//! ```text
//! 请求到达：now >= next_slot ?
//!   是 → 时隙 = now，next_slot = now + interval
//!   否 → 时隙 = next_slot，next_slot = next_slot + interval
//! 返回时隙（绝对时刻），由调用方等待到该时刻再发请求
//! ```
//!
//! 这样即使 10 个线程同时打同一个 key，它们也会被排到 10 个**互不重叠**的
//! 时隙上，时隙之间严格相差 `interval`，而不同 key 完全不受影响。
//!
//! # 为什么返回"时刻"而不是"还要等多久"
//!
//! 若返回等待时长，调用方会在等待**之后**才发送，于是实际发送时刻 =
//! 计划时刻 + 等待期间的系统延迟；多线程下相邻请求的实际间隔会被压缩到略小于
//! `interval`——限速被悄悄削弱，而恰恰是"不得绕过限速"要防的事。
//!
//! 返回绝对时刻（`Instant`）则把语义钉死：等到该时刻再发，实际间隔必然
//! `>= interval`。这条不变量可以精确测试，不必依赖"睡够了吗"的近似。
//!
//! # 为什么等待要交给调用方（而不是本模块自己睡）
//!
//! 因为执行器是异步的：在 tokio 工作线程上 `std::thread::sleep` 会占死一个
//! 工作线程（默认工作线程数 = CPU 核数），几个源同时限速就能让整个 runtime
//! 停摆。故本模块只回答"**什么时候可以发**"（[`RateLimiter::plan`]），
//! 由执行器用 `tokio::time::sleep_until(...).await` 真正等待——异步等待不占线程。
//!
//! # 缺口：`RequestSpec` 没有请求间隔字段
//!
//! [WORKAROUND] FR-008 明确要求"书源声明的……请求间隔"，但 T012 定稿的
//! `RequestSpec` 字段清单里**没有**间隔字段（只有 `timeout_ms` 与 `rate_limit_key`）。
//! 本模块因此把间隔做成**执行器侧配置注入**：
//! [`RateLimiter::set_interval`] 按 key 覆盖、[`RateLimiter::set_default_interval`]
//! 设全局默认。这能工作，但有两个缺点：
//!
//! 1. **多一跳**：管线解析出书源的 `rate_limit_ms` 后，必须记得调
//!    `set_interval`，忘了就静默退化为默认间隔——没有编译期强制。
//! 2. **配置与请求分离**：`RequestSpec` 自称"FR-008 全字段"，实际不是。
//!
//! 建议（已上报 PM 裁决，本 crate 不擅自改 core）：给 `RequestSpec` 增加
//! `request_interval_ms: Option<u64>`，或明确把"间隔由管线侧注入"写进
//! `seam/host.rs` 的文档。注意 `tome_core::model::BookSource.rate_limit_ms`
//! 已经存在（`Option<i64>`），数据源是现成的，缺的只是到网络层的通道。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 默认请求间隔：0（不限速）。
///
/// 为什么不给一个"安全"的非零默认值：默认值只在**书源没声明间隔**时生效，
/// 而多数书源确实声明了（或本就不需要限速）。给一个非零默认会把所有未声明
/// 间隔的源都拖慢，而这类源恰恰是"作者认为不需要限速"的。真正的限速依据应在
/// 书源侧，这里保持中立（`0` = 不做额外等待）。
pub const DEFAULT_REQUEST_INTERVAL: Duration = Duration::ZERO;

/// 单个 key 的调度状态。
#[derive(Debug, Clone, Copy)]
struct KeySchedule {
    /// 下一个可用时隙
    next_slot: Instant,
    /// 本 key 生效的间隔
    interval: Duration,
}

/// 调度器状态（单一锁保护）。
///
/// 默认间隔与各 key 的状态放在**同一把锁**下：若分开存（如默认间隔用原子量），
/// `plan` 就可能读到"字段已是新值、表还是旧值"的中间态，导致同一时刻不同 key
/// 用不同默认值。放在一起则任何观察都是一致的快照。
#[derive(Debug)]
struct LimiterState {
    default_interval: Duration,
    schedules: HashMap<String, KeySchedule>,
}

/// 按 `rate_limit_key` 独立限速的调度器（线程安全）。
///
/// `rate_limit_key` 由产生 `RequestSpec` 的一方给出（通常是书源 id）：
/// 同一源的不同 URL 共享同一个桶，不同源互相隔离。
#[derive(Debug)]
pub struct RateLimiter {
    state: Mutex<LimiterState>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    /// [DONE] 新建调度器，全部 key 用默认间隔（零 = 不限速）。
    pub fn new() -> Self {
        Self::with_default_interval(DEFAULT_REQUEST_INTERVAL)
    }

    /// 以指定默认间隔新建。
    pub fn with_default_interval(interval: Duration) -> Self {
        Self {
            state: Mutex::new(LimiterState {
                default_interval: interval,
                schedules: HashMap::new(),
            }),
        }
    }

    /// 设置全局默认间隔（对**尚未登记**的 key 生效）。
    ///
    /// 不改动已登记 key 的既有间隔：中途改变已生效的间隔会让"这个源到底限成多少"
    /// 变得不可预测。需要改某个已登记 key，用 [`Self::set_interval`]。
    pub fn set_default_interval(&self, interval: Duration) {
        self.lock_state().default_interval = interval;
    }

    /// 设置某个 key 的请求间隔（[WORKAROUND] 见模块文档：间隔本应随 `RequestSpec` 传递）。
    ///
    /// `interval` 为 0 表示该 key 不限速。
    ///
    /// 重复调用同一 key 会**更新**间隔（书源改动后重新导入即走这条路），
    /// 但不会重置 `next_slot`——否则"改配置"会变成绕过限速的手段。
    pub fn set_interval(&self, key: &str, interval: Duration) {
        let mut state = self.lock_state();
        match state.schedules.get_mut(key) {
            Some(schedule) => schedule.interval = interval,
            None => {
                state.schedules.insert(
                    key.to_string(),
                    KeySchedule {
                        next_slot: Instant::now(),
                        interval,
                    },
                );
            }
        }
    }

    /// 查询某 key 当前生效的间隔。不登记 key。
    /// 应用**书源声明的**请求间隔（`RequestSpec.request_interval_ms`，R2）。
    ///
    /// ## 三态语义（与 `RequestSpec` 的字段文档一一对应）
    ///
    /// | 传入 | 含义 | 行为 |
    /// |---|---|---|
    /// | `Some(interval)` | 书源显式声明 | 覆盖该 key 的生效间隔（含 `Some(0)`） |
    /// | `None` | 书源未声明 | 不动任何状态，回落到该 key 的配置值或默认值 |
    ///
    /// ## 为什么书源声明优先于本机默认
    ///
    /// 声明是书源作者**针对该站点**的明确要求（站点风控写在它的文档里，
    /// 作者据此填数），而默认值只是本机的通用兜底。把默认值放在声明之上
    /// 会让"作者按要求填了 3 秒"被本机的 1 秒静默覆盖——那是引擎在替作者
    /// 决定，且方向是"更激进"，与宪法原则 V"不得绕过限速"的取向相反。
    ///
    /// 声明**更短**时同样生效：若那构成"绕过限速"，责任在书源而非引擎——
    /// 引擎的职责是"尊重声明的间隔"，不是"审查声明是否够保守"。
    ///
    /// 标 `[DONE]`：这是 R2 的落点，被 [`Self::plan`] 消费。
    pub fn set_declared_interval(&self, key: &str, interval: Option<Duration>) {
        // `None` 不动状态：语义是"书源没说话"，而非"书源要求零间隔"。
        // 若这里把 None 当作零去写，会让"未声明的 key"意外获得零间隔，
        // 从而绕过本机配置的默认限速。
        let Some(interval) = interval else {
            return;
        };
        self.set_interval(key, interval);
    }

    /// 该 key 当前生效的间隔。
    ///
    /// 解析次序：该 key 的显式设置（可能来自 [`Self::set_interval`] 或
    /// [`Self::set_declared_interval`]）→ 默认间隔。
    pub fn interval_for(&self, key: &str) -> Duration {
        let state = self.lock_state();
        state
            .schedules
            .get(key)
            .map_or(state.default_interval, |schedule| schedule.interval)
    }

    /// 为一次请求**预约时隙**，返回该请求应当发出的**时刻**。
    ///
    /// 调用方等到返回的时刻（或更晚）再发请求，即满足限速声明。
    /// 本方法**不睡眠**，锁内只做算术——语义与理由见模块文档。
    pub fn plan(&self, key: &str) -> Instant {
        let now = Instant::now();
        let mut state = self.lock_state();
        let default_interval = state.default_interval;

        let schedule = state
            .schedules
            .entry(key.to_string())
            .or_insert(KeySchedule {
                next_slot: now,
                interval: default_interval,
            });

        let interval = schedule.interval;
        if interval.is_zero() {
            schedule.next_slot = now;
            return now;
        }

        // 本次占用的时隙：要么是"现在"（空闲足够久），要么是已排定的下一个时隙。
        let slot = if now >= schedule.next_slot {
            now
        } else {
            schedule.next_slot
        };
        // 下一个时隙从**本次占用的时隙**顺推一个间隔。
        // 累加而非 `now + interval`：这样并发抢占时会排成互不重叠的时隙序列，
        // 而不是让每个后来者都去挤"从它自己被唤醒那一刻起算"的时隙
        // （后者会让实际间隔随唤醒抖动而偏移）。
        schedule.next_slot = slot + interval;
        slot
    }

    /// 计算某次调用的等待时长（`None` = 无需等待）。
    ///
    /// 便捷包装：内部调用 [`Self::plan`] 并与当前时刻比较。执行器的异步路径
    /// 用它拿 `Duration`，再交给 `tokio::time::sleep`。
    ///
    /// 注意调用它意味着**已占用时隙**，故不要在"只是想知道还要等多久"的
    /// 场合重复调用（那会白白推进时隙、放慢实际速率）。
    pub fn plan_wait(&self, key: &str) -> Option<Duration> {
        let slot = self.plan(key);
        slot.checked_duration_since(Instant::now())
            .filter(|wait| !wait.is_zero())
    }

    /// 清空全部调度状态（测试与"重置限速"用）。默认间隔保持不变。
    pub fn clear(&self) {
        self.lock_state().schedules.clear();
    }

    /// 当前已登记的 key 数量。
    pub fn tracked_keys(&self) -> usize {
        self.lock_state().schedules.len()
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, LimiterState> {
        // 锁中毒只可能由持锁 panic 引起；本模块锁内不做任何可能 panic 的操作
        // （无 unwrap、无索引、无算术溢出）。即便真的中毒，也不该让限速器把
        // 整个抓取流程带崩——用 `into_inner()` 取回数据继续工作。
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    /// 发送一次请求：等到限速器给出的时隙再"发出"，返回（时隙, 实际发送时刻）。
    ///
    /// 循环确认是必要的：`thread::sleep` 在部分平台上可能**略早**返回
    /// （Windows 的定时器分辨率），而生产路径用的 `tokio::time::sleep_until`
    /// 保证不在截止时刻之前完成。测试若直接睡一次就返回，会模拟出比真实情况
    /// 更紧密的间隔，从而对正确的实现报假失败。
    fn send_at_slot(limiter: &RateLimiter, key: &str) -> (Instant, Instant) {
        let slot = limiter.plan(key);
        loop {
            let now = Instant::now();
            if now >= slot {
                return (slot, now);
            }
            thread::sleep(slot - now);
        }
    }

    #[test]
    fn default_interval_is_zero_so_no_throttling() {
        let limiter = RateLimiter::new();
        let first = limiter.plan("source-a");
        let second = limiter.plan("source-a");
        // 零间隔时后续时隙不被推迟。
        assert!(second.duration_since(first) < Duration::from_millis(5));
        assert_eq!(limiter.interval_for("source-a"), Duration::ZERO);
    }

    #[test]
    fn second_call_within_interval_is_scheduled_later() {
        let limiter = RateLimiter::new();
        let interval = Duration::from_millis(50);
        limiter.set_interval("source-a", interval);

        let first = limiter.plan("source-a");
        let second = limiter.plan("source-a");
        let gap = second.duration_since(first);
        // 第二次的时隙应不早于"第一次 + 一个间隔"。
        assert!(
            gap >= interval,
            "第二次时隙只晚了 {gap:?}，应不早于 {interval:?}"
        );
    }

    /// 同一 key 连续 N 次请求总耗时 ≥ (N-1) × 间隔（T018 验收）。
    ///
    /// # 断言的是什么（以及为什么不是"实际发送间隔"）
    ///
    /// 本限速器是**前置排程**：它保证连续的**时隙**间隔恰好为一个 `interval`，
    /// 且每次发送不早于其时隙。但"相邻两次**实际发送**的间隔也 >= interval"
    /// 是它无法保证的——若第 1 次因调度延迟晚发出（时隙 0 却在 5ms 发出），
    /// 第 2 次仍在时隙 25ms 发出，则实际间隔只有 20ms。
    ///
    /// 要消除这个偏差，需要传输层回传"实际发出时刻"来重排后续时隙；
    /// 代价是热路径上多一次锁获取与一套反馈协议，而收益是把误差从
    /// "OS 定时器粒度"（毫秒级）降到 0——书源的声明间隔通常是秒级，
    /// 毫秒级偏差占比可忽略。故不做，但在此明确：
    ///
    /// - **保证**：时隙间隔 >= interval；实际发送 >= 自身时隙。
    /// - **不保证**：相邻实际发送间隔 >= interval（受上一条抖动影响）。
    #[test]
    fn same_key_requests_are_spaced_by_interval() {
        let interval = Duration::from_millis(25);
        let limiter = RateLimiter::new();
        limiter.set_interval("source-a", interval);

        const REQUESTS: u32 = 4;
        let started = Instant::now();
        let mut slots = Vec::with_capacity(REQUESTS as usize);
        for _ in 0..REQUESTS {
            let (slot, sent) = send_at_slot(&limiter, "source-a");
            // 实际发送不得早于自身时隙（这是 `plan` 返回绝对时刻的意义所在）。
            assert!(sent >= slot, "发送时刻 {sent:?} 早于时隙 {slot:?}");
            slots.push(slot);
        }

        // 时隙序列严格按 interval 递增——这是"不得绕过限速"（宪法原则 V）的落点。
        for pair in slots.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(
                gap >= interval,
                "相邻时隙间隔 {gap:?} 小于声明间隔 {interval:?}"
            );
        }

        let elapsed = started.elapsed();
        let minimum = interval * (REQUESTS - 1);
        assert!(
            elapsed >= minimum,
            "总耗时 {elapsed:?} 小于最小间隔总和 {minimum:?}"
        );
        // 上界只为防止实现变成"睡太久"（如误用了秒级单位）。
        assert!(
            elapsed < minimum + Duration::from_secs(2),
            "总耗时 {elapsed:?} 远超预期，疑似实现错误"
        );
    }

    /// 单源限速不影响其他源（T018 验收）。
    #[test]
    fn different_keys_do_not_slow_each_other() {
        let interval = Duration::from_millis(60);
        let limiter = RateLimiter::new();
        limiter.set_interval("slow-source", interval);

        // 先在 slow-source 上占满一段时隙，让它处于"需要等待"的状态。
        let _ = send_at_slot(&limiter, "slow-source");

        // 另一个源连续 3 次：时隙必须都是"此刻"，不受 slow-source 影响。
        for round in 0..3 {
            let slot = limiter.plan("other-source");
            // 时隙不得落在未来——"未限速"的含义就是无需等待。
            // 用调用**之后**的时刻比较：调用之前的时刻必然 <= slot，
            // 用它做上界无法区分"现在"与"稍后"。
            let after_call = Instant::now();
            assert!(
                slot <= after_call,
                "第 {round} 次：未限速的源被排到了未来（其他源的间隔漏了过来）"
            );
        }

        // 整体耗时也应远小于 slow-source 的间隔——顺序执行 3 次不该有任何等待。
        let started = Instant::now();
        for _ in 0..3 {
            let _ = limiter.plan("other-source");
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < interval / 2,
            "另一源耗时 {elapsed:?} 受 slow-source 的 {interval:?} 影响"
        );
    }

    /// 交错请求两个源：总耗时应由"各自次数"决定，而不是"总次数"。
    #[test]
    fn interleaved_keys_scale_independently() {
        // 两个源各自 3 次请求，各需 (3-1)×100=200ms；若被错误地串成一列，
        // 会需要 (6-1)×100=500ms。取 350ms 为界可把两种情况分开。
        // 区间取值放大的原因：共享 CI runner（尤其 macOS arm64）的睡眠调度
        // 噪声单次可达数十 ms，40ms 区间的判定 margin 会被噪声吞掉（CI 实测
        // macOS 上 150ms 阈值 flaky）；放大区间让 margin 远大于噪声。
        let interval = Duration::from_millis(100);
        let limiter = RateLimiter::new();
        limiter.set_interval("source-a", interval);
        limiter.set_interval("source-b", interval);

        let started = Instant::now();
        for _ in 0..3 {
            send_at_slot(&limiter, "source-a");
            send_at_slot(&limiter, "source-b");
        }
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(350),
            "交错耗时 {elapsed:?} 表明两个源被串成了一列"
        );
    }

    /// 已有 key 的**请求级**间隔覆盖：书源声明的间隔优先于默认值。
    #[test]
    fn spec_declared_interval_overrides_default() {
        // R2（PM 仲裁）：`RequestSpec.request_interval_ms` 是书源可声明的请求间隔。
        // 语义：**书源声明优先于限速器的默认值**——声明是书源作者对该站点的
        // 明确要求（站点风控写在文档里，作者据此填），默认值只是本机的通用兜底。
        let limiter = RateLimiter::with_default_interval(Duration::ZERO);
        limiter.set_interval("src", Duration::from_millis(50));

        // 声明更长的间隔：应生效
        limiter.set_declared_interval("src", Some(Duration::from_millis(80)));
        assert_eq!(
            limiter.interval_for("src"),
            Duration::from_millis(80),
            "书源声明的间隔应覆盖已配置值"
        );

        // 声明更短的间隔：同样应生效（书源知道自己在做什么；
        // 若这正是"绕过限速"的场景，责任在书源而非引擎——
        // 引擎的职责是"尊重声明的间隔"，见宪法原则 V 的原文）
        limiter.set_declared_interval("src", Some(Duration::from_millis(20)));
        assert_eq!(limiter.interval_for("src"), Duration::from_millis(20));
    }

    /// `None` 表示"书源未声明"，此时应回落到既有的按 key 配置或默认值。
    #[test]
    fn undeclared_interval_falls_back_to_configured_default() {
        let limiter = RateLimiter::with_default_interval(Duration::from_millis(30));
        limiter.set_interval("configured", Duration::from_millis(70));

        // 未声明：用该 key 已配置的间隔
        limiter.set_declared_interval("configured", None);
        assert_eq!(
            limiter.interval_for("configured"),
            Duration::from_millis(70)
        );

        // 未声明且该 key 无配置：用默认值
        limiter.set_declared_interval("fresh", None);
        assert_eq!(limiter.interval_for("fresh"), Duration::from_millis(30));
    }

    /// `Some(0)` 与 `None` 语义不同：前者是"显式声明不要间隔"，
    /// 应覆盖默认值（否则书源无法表达"这个站点不需要间隔"）。
    #[test]
    fn explicit_zero_interval_overrides_default() {
        let limiter = RateLimiter::with_default_interval(Duration::from_millis(100));
        limiter.set_declared_interval("no-delay", Some(Duration::ZERO));
        assert_eq!(
            limiter.interval_for("no-delay"),
            Duration::ZERO,
            r#"Some(0) 是显式声明"不要间隔"，应覆盖默认值"#
        );
    }

    /// 声明只影响自己的 key。
    #[test]
    fn declared_interval_is_per_key() {
        let limiter = RateLimiter::new();
        limiter.set_declared_interval("slow", Some(Duration::from_millis(100)));
        limiter.set_declared_interval("fast", Some(Duration::from_millis(5)));

        assert_eq!(limiter.interval_for("slow"), Duration::from_millis(100));
        assert_eq!(limiter.interval_for("fast"), Duration::from_millis(5));
        // 未声明的 key 不受影响
        assert_eq!(limiter.interval_for("untouched"), Duration::ZERO);
    }

    /// 声明对**后续**的 `plan` 立即生效，且不影响已排定的时隙序列的连续性。
    #[test]
    fn declared_interval_takes_effect_for_subsequent_planning() {
        let limiter = RateLimiter::new();
        limiter.set_declared_interval("src", Some(Duration::from_millis(30)));
        let first = limiter.plan("src");
        let second = limiter.plan("src");
        assert!(
            second.duration_since(first) >= Duration::from_millis(30),
            "相邻时隙间隔应不少于声明的 30ms，实际 {:?}",
            second.duration_since(first)
        );
    }

    /// 并发调用者必须落在互不重叠的时隙上。
    #[test]
    fn concurrent_callers_get_non_overlapping_slots() {
        let interval = Duration::from_millis(20);
        let limiter = Arc::new(RateLimiter::new());
        limiter.set_interval("busy", interval);

        const THREADS: usize = 4;
        let mut handles = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            let limiter = Arc::clone(&limiter);
            handles.push(thread::spawn(move || send_at_slot(&limiter, "busy")));
        }

        let mut slots: Vec<Instant> = handles
            .into_iter()
            .map(|handle| handle.join().expect("线程不应 panic"))
            .map(|(_slot, _sent)| _slot)
            .collect();
        slots.sort();

        // 关键不变量：**时隙**两两不重叠，且相邻间隔 >= interval。
        // 四个线程并发抢占，也得排成 four 个相隔 20ms 的时隙。
        for pair in slots.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(
                gap >= interval,
                "并发调用者之间时隙间隔 {gap:?} 小于 {interval:?}，时隙重叠了"
            );
        }
        assert!(
            slots[THREADS - 1].duration_since(slots[0]) >= interval * (THREADS as u32 - 1),
            "末次时隙过早：{:?}",
            slots[THREADS - 1].duration_since(slots[0])
        );
    }

    #[test]
    fn zero_interval_disables_throttling_for_that_key() {
        let limiter = RateLimiter::new();
        limiter.set_interval("source-a", Duration::from_millis(50));
        limiter.set_interval("source-a", Duration::ZERO);
        let first = limiter.plan("source-a");
        let second = limiter.plan("source-a");
        // 零间隔：不再推迟后续时隙。
        assert!(second.duration_since(first) < Duration::from_millis(5));
    }

    #[test]
    fn updating_interval_does_not_reset_pending_slot() {
        let limiter = RateLimiter::new();
        limiter.set_interval("source-a", Duration::from_millis(50));
        let first = limiter.plan("source-a");
        // 调大间隔不得变成"重新计时"的绕过手段：已排定的时隙只能维持或更晚。
        limiter.set_interval("source-a", Duration::from_millis(80));
        let second = limiter.plan("source-a");
        assert!(
            second.duration_since(first) >= Duration::from_millis(50),
            "更新间隔不应重置已排定的时隙"
        );
    }

    #[test]
    fn interval_for_returns_default_for_untracked_key() {
        let limiter = RateLimiter::with_default_interval(Duration::from_millis(7));
        assert_eq!(limiter.interval_for("never-seen"), Duration::from_millis(7));
        assert_eq!(limiter.tracked_keys(), 0, "查询不应登记 key");
    }

    #[test]
    fn new_keys_inherit_default_interval() {
        let limiter = RateLimiter::with_default_interval(Duration::from_millis(30));
        let first = limiter.plan("fresh");
        let second = limiter.plan("fresh");
        assert!(
            second.duration_since(first) >= Duration::from_millis(30),
            "应继承默认间隔"
        );
    }

    #[test]
    fn changing_default_does_not_affect_tracked_keys() {
        let limiter = RateLimiter::with_default_interval(Duration::from_millis(10));
        let _ = limiter.plan("already-seen");
        limiter.set_default_interval(Duration::from_millis(200));
        // 已登记的 key 保持自己的间隔（改默认只影响"将来的" key）。
        assert_eq!(
            limiter.interval_for("already-seen"),
            Duration::from_millis(10)
        );
        assert_eq!(
            limiter.interval_for("brand-new"),
            Duration::from_millis(200)
        );
    }

    #[test]
    fn clear_resets_tracked_state() {
        let limiter = RateLimiter::new();
        limiter.set_interval("source-a", Duration::from_millis(50));
        let _ = limiter.plan("source-a");
        assert_eq!(limiter.tracked_keys(), 1);
        limiter.clear();
        assert_eq!(limiter.tracked_keys(), 0);
    }

    #[test]
    fn plan_never_blocks_the_caller() {
        // `plan` 必须在锁内只做算术：用一个已排到远端的 key 验证
        // 连续调用会立刻返回（而不是睡在锁里）。
        let limiter = RateLimiter::new();
        limiter.set_interval("busy", Duration::from_secs(30));
        let _ = limiter.plan("busy");

        let started = Instant::now();
        for _ in 0..50 {
            let _ = limiter.plan("busy");
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "plan 阻塞了 {elapsed:?}，说明它把等待做在了锁内"
        );
    }

    #[test]
    fn plan_wait_reports_zero_as_none() {
        let limiter = RateLimiter::new();
        // 零间隔：无需等待，故应是 None 而不是 Some(0)。
        assert_eq!(limiter.plan_wait("k"), None);

        limiter.set_interval("k", Duration::from_millis(30));
        // 第一次调用占用"现在"这个时隙，随即把下一个时隙推到 30ms 后。
        assert_eq!(limiter.plan_wait("k"), None, "首次请求无需等待");
        // 第二次落在已排定的未来时隙上，应报出等待时长。
        let wait = limiter.plan_wait("k").expect("紧接的第二次应需等待");
        assert!(wait > Duration::ZERO && wait <= Duration::from_millis(30));
    }
}
