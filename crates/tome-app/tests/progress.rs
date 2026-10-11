//! T015 验收：进度记忆与节流落盘（`crates/tome-app/src/state/progress.rs`）
//!
//! ## 本文件覆盖的验收子项（`specs/003-app-shell/tasks.md` T015）
//!
//! - 正常退出 → 重开位置**逐字符相等**（SC-009 前半）
//! - 节流窗口内杀进程（模拟）→ 回退 ≤ **一个窗口**（SC-009 后半）
//! - 节流窗口值定为 **3 秒**（spec 假设节的量化落定）
//!
//! ## 为什么时钟要注入
//!
//! "3 秒到了没有"若直接读 `Instant::now()`，本文件就只能靠 `sleep(3s)` 来验
//! ——那是**慢且不稳**的（CI 上尤其；并行跑时更糟）。把"现在几毫秒"作为参数
//! 传进来后，窗口边界、跨越、回退、失败重试等情形都是普通函数调用：**瞬时
//! 且确定**。
//!
//! ## "杀进程"是怎么模拟的
//!
//! 真实的杀进程无法在单元测试里做（那会杀掉测试进程）。这里模拟的是它**在
//! 我们的模型里的等价物**：丢弃 `ProgressTracker`（内存状态消失），只保留
//! "上次落盘的值"。于是"丢失量 = 内存里比已落盘多出来的那部分"可以被精确
//! 断言——而那正是 SC-009 后半句要的东西。

use std::time::Duration;

use tome_app::engine::messages::ReadingAnchor;
use tome_app::state::progress::{
    flush_window, MonotonicClock, ProgressTracker, PROGRESS_FLUSH_WINDOW_MS,
};

fn anchor(chapter: i32, offset: i64) -> ReadingAnchor {
    ReadingAnchor {
        chapter_index: chapter,
        char_offset: offset,
    }
}

/// 节流窗口是 **3 秒**（T015 的口径落定）。
#[test]
fn the_flush_window_is_three_seconds() {
    assert_eq!(PROGRESS_FLUSH_WINDOW_MS, 3_000);
    assert_eq!(flush_window(), Duration::from_millis(3_000));
}

/// 刚打开时**不脏**（打开本身不该产生一次落盘）。
///
/// ## 这条为什么重要
///
/// 若打开就置脏，则"打开书 → 立刻退出"会写一次与库里相同的数据；更糟的是
/// 它把"上次落盘时刻"归零，于是**下次翻页会立刻触发落盘**——看起来像节流
/// 失效（每翻一页写一次）。
#[test]
fn opening_a_book_is_not_dirty() {
    let tracker = ProgressTracker::open("1", anchor(3, 500), 1_000);
    assert!(!tracker.is_dirty());
    assert_eq!(tracker.take_pending(), None);
    assert!(!tracker.due(99_999), "不脏时永远不到点");
    assert_eq!(tracker.next_deadline_in(1_000), None);
}

/// 位置变化置脏；**位置未变不置脏**（按住方向键到章末不该反复写同一个值）。
#[test]
fn only_actual_changes_make_it_dirty() {
    let mut tracker = ProgressTracker::open("1", anchor(3, 500), 0);

    tracker.observe(anchor(3, 500));
    assert!(!tracker.is_dirty(), "观察到同一个位置不该置脏");

    tracker.observe(anchor(3, 501));
    assert!(tracker.is_dirty(), "位置变了就该置脏");
}

/// 窗口**边界**：恰好 3000ms 到点，2999ms 不到点。
///
/// ## 为什么边界要单独断言
///
/// 判据写成 `>` 还是 `>=` 只差一个刻度，而它决定的是"SC-009 的丢失量口径
/// 是窗口还是窗口减一毫秒"。这类差一错误在代码里看不出来，只有边界断言能钉。
#[test]
fn the_window_boundary_is_inclusive() {
    let mut tracker = ProgressTracker::open("1", anchor(0, 0), 0);
    tracker.observe(anchor(0, 100));

    assert!(!tracker.due(2_999), "差 1ms 不该到点");
    assert!(tracker.due(3_000), "恰好一个窗口应到点（闭区间）");
    assert!(tracker.due(3_001));
}

/// 落盘后清脏，且计时从落盘时刻重新起算。
#[test]
fn flushing_resets_the_window() {
    let mut tracker = ProgressTracker::open("1", anchor(0, 0), 0);
    tracker.observe(anchor(0, 100));
    assert!(tracker.due(3_000));

    tracker.mark_flushed(3_000);
    assert!(!tracker.is_dirty());
    assert!(!tracker.due(3_000), "刚落完盘不该立刻又到点");

    // 又翻了一页：计时从**落盘时刻**起算，而不是从这次观察起算
    tracker.observe(anchor(0, 200));
    assert!(!tracker.due(5_999), "距上次落盘不足一个窗口，不该到点");
    assert!(tracker.due(6_000), "距上次落盘满一个窗口应到点");
}

/// 待冲刷的值是**当前位置**（不是打开时的基线）。
#[test]
fn pending_carries_the_latest_position() {
    let mut tracker = ProgressTracker::open("42", anchor(0, 0), 0);
    tracker.observe(anchor(1, 10));
    tracker.observe(anchor(2, 20));
    tracker.observe(anchor(3, 30));

    let (book_id, pending) = tracker.take_pending().expect("应有待冲刷的值");
    assert_eq!(book_id, "42");
    assert_eq!(pending, anchor(3, 30), "取出的应是最后一次观察到的位置");
}

/// 写失败后**保持脏**，但推后一整个窗口（否则定时唤醒退化成忙循环）。
///
/// ## 这条守的是什么
///
/// `due()` 的判据是 `now - last_flush >= window`。写失败时若不动 `last_flush`，
/// 下一次判据**立刻**为真——于是引擎线程每个 tick 都失败、都立刻再试，把 CPU
/// 烧在一个注定失败的操作上（磁盘满、库只读这类失败不会自愈）。
#[test]
fn a_failed_flush_backs_off_by_a_full_window() {
    let mut tracker = ProgressTracker::open("1", anchor(0, 0), 0);
    tracker.observe(anchor(0, 100));

    assert!(tracker.due(3_000));
    // 写失败：推后一整个窗口
    tracker.defer(3_000);

    assert!(tracker.is_dirty(), "写失败必须保持脏，否则数据静默丢失");
    assert!(!tracker.due(3_000), "推后之后不该立刻再试（忙循环）");
    assert!(!tracker.due(5_999));
    assert!(tracker.due(6_000), "一个窗口之后应重试");
}

/// `next_deadline_in` 给出**恰好**的剩余时间（而不是一个固定轮询间隔）。
///
/// ## 为什么这条重要
///
/// 固定间隔轮询会让"落盘时刻"最多晚一个间隔，于是 SC-009 的丢失量变成
/// "窗口 + 间隔"。给出精确剩余时间让引擎线程恰好在窗口到点醒来——丢失量
/// 就是窗口本身。
#[test]
fn the_next_deadline_is_exact() {
    let mut tracker = ProgressTracker::open("1", anchor(0, 0), 0);
    tracker.observe(anchor(0, 100));

    assert_eq!(
        tracker.next_deadline_in(0),
        Some(Duration::from_millis(3_000))
    );
    assert_eq!(
        tracker.next_deadline_in(1_000),
        Some(Duration::from_millis(2_000))
    );
    assert_eq!(
        tracker.next_deadline_in(2_999),
        Some(Duration::from_millis(1)),
        "剩余 1ms 也应精确给出"
    );
    assert_eq!(
        tracker.next_deadline_in(3_000),
        Some(Duration::from_millis(0)),
        "到点时应给出 0（而不是 None——那会让调用方不再醒来）"
    );
}

/// **SC-009 前半**：正常退出 → 重开位置逐字符相等。
#[test]
fn a_normal_exit_restores_the_position_exactly() {
    // --- 第一次会话 ---
    let mut tracker = ProgressTracker::open("1", anchor(0, 0), 0);
    let read_to = anchor(7, 1_234);
    tracker.observe(read_to);

    // 正常退出：冲刷（无论窗口是否到点）
    let (book_id, flushed) = tracker.take_pending().expect("退出前应有待冲刷的值");
    assert_eq!(book_id, "1");

    // --- 第二次会话：以库里的值打开 ---
    let reopened = ProgressTracker::open(book_id, flushed, 10_000);
    assert_eq!(
        reopened.anchor(),
        read_to,
        "重开后位置必须逐字符相等（SC-009）"
    );
    assert!(!reopened.is_dirty(), "恢复到的位置就是库里的值，不该脏");
}

/// **SC-009 后半**：窗口内"杀进程"→ 回退 ≤ 一个窗口。
///
/// ## 这条的判定方式
///
/// 模拟杀进程 = 丢弃内存里的 tracker，只保留"上次落盘的值"。于是丢失量 =
/// 内存里比已落盘多出来的那部分，而它必须**不超过一个窗口能累积的量**。
///
/// 具体做法：让阅读位置随"翻页"推进，每个位置带一个时间戳；在窗口内的某
/// 一刻丢弃 tracker，检查"已落盘的位置"与"内存位置"的差距对应的时间差
/// **≤ 窗口**。
#[test]
fn killing_within_the_window_loses_at_most_one_window() {
    /// 一页推进 100 个字符，模拟"每秒翻一页"。
    const CHARS_PER_PAGE: i64 = 100;
    const MS_PER_PAGE: u64 = 1_000;

    let mut tracker = ProgressTracker::open("1", anchor(0, 0), 0);
    let mut last_flushed = anchor(0, 0);
    let mut flushed_at_ms = 0u64;

    // 跑到 5.5 秒（窗口是 3 秒，故中途至少落过一次盘）
    for page in 1..=5 {
        let now = page * MS_PER_PAGE;
        tracker.observe(anchor(0, page as i64 * CHARS_PER_PAGE));

        if tracker.due(now) {
            let (_, pending) = tracker.take_pending().expect("到点必有待冲刷的值");
            last_flushed = pending;
            flushed_at_ms = now;
            tracker.mark_flushed(now);
        }
    }
    // 第 5.5 页：窗口内的推进（未到点）
    let now = 5 * MS_PER_PAGE + 500;
    tracker.observe(anchor(0, 5 * CHARS_PER_PAGE + 50));

    // --- 杀进程：内存状态消失 ---
    let memory_position = tracker.anchor();
    let lost_chars = memory_position.char_offset - last_flushed.char_offset;
    let lost_ms = now - flushed_at_ms;

    assert!(
        lost_ms <= PROGRESS_FLUSH_WINDOW_MS,
        "丢失的时间跨度 {lost_ms}ms 超过一个窗口（{}ms）——SC-009 后半不成立",
        PROGRESS_FLUSH_WINDOW_MS
    );
    assert_eq!(
        lost_chars,
        (lost_ms / MS_PER_PAGE) as i64 * CHARS_PER_PAGE + 50,
        "丢失的字符数应与时间跨度一致（本夹具的换算关系）"
    );
}

/// 单调时钟不回退（`SystemTime` 会在 NTP 校时时倒退）。
#[test]
fn the_monotonic_clock_never_goes_backwards() {
    let clock = MonotonicClock::new();
    let first = clock.now_ms();
    std::thread::sleep(Duration::from_millis(5));
    let second = clock.now_ms();
    assert!(
        second >= first,
        "单调时钟不得回退（实测 {first} → {second}）"
    );
}
