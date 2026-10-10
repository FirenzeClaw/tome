//! \[DONE] 并行流式搜索（tasks.md T073 / A6 / FR-012，SC-006）
//!
//! # 本模块**替代** `search.rs` 里的串行实现
//!
//! S1-1 阶段的 `SourceSearchStream` 是串行的（逐源依次跑），当时的理由是
//! "确定性与限速纪律优先于并行度"——`--record` 的录制顺序不确定会让
//! 重放逐字节一致（T029）失效。
//!
//! T073 明确要求"多源并行"。本模块解决当时那个顾虑的办法是：
//! **并行执行、按源 id 排序产出**。
//!
//! ```text
//!    worker 1 ──┐
//!    worker 2 ──┤ 并行发请求          ┌─ 按 source_id 排序缓冲 ─► next_item()
//!    worker 3 ──┤（各自的限速仍生效）──┘
//!    worker N ──┘
//! ```
//!
//! 于是两边都成立：首结果时间不再等于"最慢的源"（并行），而**产出顺序
//! 与源 id 排序一致**（确定性）。录制顺序因此固定，重放仍逐字节一致。
//!
//! # 为什么要缓冲而不是"谁先完成谁先出"
//!
//! "谁先完成谁先出"会让顺序取决于网络时序——同一份输入两次运行产出不同
//! 顺序，`--record` 语料的字节就不同。那正是 T029 要防的。
//!
//! 代价是**首结果必须等**：产出第 N 个源的结果前，必须等前 N-1 个源完成
//! （或失败）。对"第一个源很快、第二个源很慢"的场景，用户要多等一会儿。
//!
//! ## 为什么这个代价是可接受的
//!
//! 替代方案（乱序产出）牺牲的是**可重放性**——而那是宪章 III 的一致性
//! 验证基础，也是差分测试的前提（没有确定的输入输出对，就没有可比的基线）。
//! 相比之下多等几百毫秒是可以接受的。
//!
//! 何况并行已经把总时长压到"最慢的那个源"，而不是"所有源之和"。
//!
//! # 取消语义（FR-012 / A6）
//!
//! 取消令牌在**每个源开始前**与**产出每个条目时**检查。已被 worker 取走
//! 的源仍会跑完（无法中断一个已经在飞的 HTTP 请求），但：
//!
//! 1. 不会再取新的源
//! 2. 已取消后 `next_item` 立即返回 `None`
//!
//! 故取消是"尽快收尾"而不是"立即停止"——这与 A6 的"实现方须尽快响应
//! 取消"一致。测试里用"取消后不再有新请求"来验证（那才是可观测的）。

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tome_core::seam::{CancelToken, SearchResultItem, SearchStream};
use tome_core::{Book, NetHost};

use crate::search::{search_one_source_with_js, JsEngineFactory};
use crate::source::ParsedSource;

/// 并行搜索的工作线程数上限。
///
/// # 为什么不等于源数
///
/// 100 个源开 100 个线程会：
///
/// 1. **对第三方造成压力**——100 个请求同时打向各自的站点，那是别人
///    风控系统眼里的异常流量（而"遵守书源声明的请求间隔"是我们的红线，
///    并发度过高会让单源的间隔约束在**全局**层面失去意义）
/// 2. **耗尽本机资源**——每个 worker 可能要建 QuickJS 沙箱（含 JS 的源）
///
/// 取 8：足以让"一个慢源"不再拖住全部（8 个槽位里还有 7 个在跑），
/// 又不至于让单机产生异常并发。
///
/// \[WORKAROUND] 硬编码常量：若实测发现慢源占比很高（8 个槽位仍不够），
/// 应改为可配置项随 `config.toml` 注入。
pub const MAX_SEARCH_WORKERS: usize = 8;

/// 等待下一条结果的轮询间隔。
///
/// # 为什么是轮询而不是 `recv()`
///
/// `recv()` 会一直阻塞到有结果或通道关闭——期间**无法响应取消**。
/// 用户点了"取消"却要等到某个源的 15 秒超时才有反应，那不算"尽快收尾"。
///
/// 轮询间隔取 50ms：人对 50ms 无感，而它让取消的响应延迟不超过 50ms。
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 一个源的结果（worker 产出的原始形态）。
enum WorkerOutcome {
    Success {
        source_id: String,
        books: Vec<Book>,
    },
    Failed {
        source_id: String,
        error: tome_core::ErrorKind,
    },
}

/// 并行流式搜索。
///
/// 由 [`parallel_search`] 构造——不直接 `new`，因为构造时要启动 worker。
pub struct ParallelSearchStream {
    /// 已完成的结果，按源 id 排序缓冲。
    ///
    /// 用 `BTreeMap` 让"输出顺序"由数据结构保证，而不是靠调用方排序。
    ready: BTreeMap<String, WorkerOutcome>,
    /// 尚未产出的源 id（有序），决定"下一个该产出谁"。
    order: Vec<String>,
    /// 已产出的游标。
    cursor: usize,
    /// worker 的结果通道。
    receiver: Receiver<WorkerOutcome>,
    /// 活着的 worker 数（用来判断"再也不会有新结果了"）。
    pending_workers: Arc<Mutex<usize>>,
    token: CancelToken,
}

impl SearchStream for ParallelSearchStream {
    fn next_item(&mut self) -> Option<SearchResultItem> {
        // ① 取消后立即收尾（A6：尽快响应，不应把已取消的流跑完）
        if self.token.is_cancelled() {
            return None;
        }

        // ② 反复尝试产出"顺序上的下一个"
        loop {
            if self.cursor >= self.order.len() {
                return None;
            }
            let next_id = self.order[self.cursor].clone();

            // 已经拿到这个源的结果 → 产出它
            if let Some(outcome) = self.ready.remove(&next_id) {
                self.cursor += 1;
                return Some(match outcome {
                    WorkerOutcome::Success { source_id, books } => {
                        SearchResultItem::SourceResults { source_id, books }
                    }
                    WorkerOutcome::Failed { source_id, error } => {
                        SearchResultItem::SourceFailed { source_id, error }
                    }
                });
            }

            // ③ 还没拿到 → 收一个结果（带超时，以便响应取消）
            match self.receiver.recv_timeout(POLL_INTERVAL) {
                Ok(outcome) => {
                    // `pending_workers` 的递减在 worker 线程侧完成（见
                    // `spawn_workers`），此处只负责把结果放进缓冲
                    let id = outcome_id(&outcome);
                    self.ready.insert(id, outcome);
                }
                Err(RecvTimeoutError::Timeout) => {
                    // 超时不是错误——检查取消后继续等
                    if self.token.is_cancelled() {
                        return None;
                    }
                    // 所有 worker 都退出了却还等不到结果：说明有 worker
                    // 异常退出（panic）。此时不能无限等下去。
                    if self.workers_finished() && self.ready.is_empty() {
                        // 剩下的源永远不会产出——为它们补上"失败"条目，
                        // 让流正常结束而不是卡死。
                        //
                        // ## 为什么不直接返回 None
                        //
                        // 直接结束会让"缺少某个源的结果"变成不可观测的事
                        // ——调用方无法区分"源还没跑"与"源的结果丢了"。
                        self.cursor += 1;
                        return Some(SearchResultItem::SourceFailed {
                            source_id: next_id,
                            error: tome_core::ErrorKind::Storage,
                        });
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // 通道断开 = 全部 worker 退出。若还有未产出的源，
                    // 它们的结果再也到不了——同样补成失败条目。
                    if self.cursor >= self.order.len() {
                        return None;
                    }
                    self.cursor += 1;
                    return Some(SearchResultItem::SourceFailed {
                        source_id: next_id,
                        error: tome_core::ErrorKind::Storage,
                    });
                }
            }
        }
    }

    fn cancel_token(&self) -> &CancelToken {
        &self.token
    }
}

impl ParallelSearchStream {
    /// 是否所有 worker 都已退出。
    fn workers_finished(&self) -> bool {
        *self
            .pending_workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            == 0
    }
}

/// 取一个 worker 结果的源 id。
fn outcome_id(outcome: &WorkerOutcome) -> String {
    match outcome {
        WorkerOutcome::Success { source_id, .. } | WorkerOutcome::Failed { source_id, .. } => {
            source_id.clone()
        }
    }
}

/// 并行搜索（T073 的入口）。
///
/// `sources` 是**要搜索的源**（调用方已按 id 过滤）。返回的流按源 id
/// 排序产出，**确定性**由此保证。
pub fn parallel_search(
    sources: Vec<ParsedSource>,
    keyword: String,
    host: Arc<dyn NetHost>,
    js_factory: Option<JsEngineFactory<'static>>,
    max_workers: usize,
) -> ParallelSearchStream {
    // 顺序表：先排序再决定产出次序。
    //
    // ## 为什么排序在这里而不是留给调用方
    //
    // "输出顺序确定"是这个类型对调用方的承诺——把它留给调用方就等于
    // 没有承诺（任何新调用点都可能忘了排）。
    let mut ordered: Vec<ParsedSource> = sources;
    ordered.sort_by(|left, right| left.source.id.cmp(&right.source.id));
    let order: Vec<String> = ordered
        .iter()
        .map(|parsed| parsed.source.id.clone())
        .collect();

    let (sender, receiver) = mpsc::channel::<WorkerOutcome>();
    let token = CancelToken::new();
    let pending_workers = Arc::new(Mutex::new(0usize));
    let worker_count = max_workers.max(1).min(ordered.len().max(1));

    // ## 为什么用"共享队列 + N 个 worker"而不是"每源一线程"
    //
    // 每源一线程在 100 个源时开 100 个线程——既浪费又让并发度失控
    // （见 `MAX_SEARCH_WORKERS` 的说明）。共享队列让并发的**上限**
    // 由 worker 数决定，而源数只影响队列长度。
    //
    // `VecDeque` + `pop_front`（而不是 `Vec::pop`）：源已按 id 排序，
    // 从**头部**取才能让 worker 的处理顺序与产出顺序一致（虽然产出顺序
    // 由 `order` 表保证，但从头部取让日志里的先后也更直观）。
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(ordered)));

    for _ in 0..worker_count {
        let queue = queue.clone();
        let sender = sender.clone();
        let token = token.clone();
        let keyword = keyword.clone();
        let host = host.clone();
        let js_factory = js_factory.clone();
        // 每个 worker 拿一个自己的计数器句柄；主线程也留一份用于启动失败回退
        let worker_pending = pending_workers.clone();
        let main_pending = pending_workers.clone();

        *worker_pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;

        // 线程启动失败（资源不足）不该让整个搜索失败——跳过这个 worker，
        // 剩下的仍在跑（并发度降低但仍可用）。
        let spawned = std::thread::Builder::new()
            .name("tome-search-worker".to_string())
            .spawn(move || {
                loop {
                    // 取一个源；取不到说明队列空了
                    let job = {
                        let mut guard = queue
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        guard.pop_front()
                    };
                    let Some(parsed) = job else {
                        break;
                    };

                    // 取消后不再取新源（"尽快收尾"的可观测形态）
                    if token.is_cancelled() {
                        break;
                    }

                    let source_id = parsed.source.id.clone();
                    let outcome = run_one(&parsed, &keyword, host.as_ref(), js_factory.as_ref());
                    // 通道可能已被丢弃（调用方提前放弃）。那不是错误。
                    let _ = sender.send(outcome);
                    let _ = source_id;
                }

                let mut guard = worker_pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *guard = guard.saturating_sub(1);
            });

        if spawned.is_err() {
            // 启动失败：把计数退回去（否则 `workers_finished` 永远不会为真，
            // 流会一直等到超时）
            let mut guard = main_pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = guard.saturating_sub(1);
        }
    }

    // 主线程持的 `sender` 必须丢弃：否则通道永远不会断开
    // （`Receiver` 只有在**所有** `Sender` 都 drop 后才报 Disconnected）
    drop(sender);

    ParallelSearchStream {
        ready: BTreeMap::new(),
        order,
        cursor: 0,
        receiver,
        pending_workers,
        token,
    }
}

/// 跑单个源（worker 的三行主体，抽出来便于单独测试）。
fn run_one(
    parsed: &ParsedSource,
    keyword: &str,
    host: &dyn NetHost,
    js_factory: Option<&JsEngineFactory<'static>>,
) -> WorkerOutcome {
    let source_id = parsed.source.id.clone();

    // 只为需要 JS 的源建引擎（纯 CSS/正则的源走快路径）
    let engine = match (
        js_factory,
        crate::js::source_needs_javascript(&parsed.source),
    ) {
        (Some(factory), true) => match factory(&parsed.source) {
            Ok(engine) => Some(engine),
            Err(error) => {
                return WorkerOutcome::Failed {
                    source_id,
                    error: error.to_error_kind(),
                }
            }
        },
        _ => None,
    };

    match search_one_source_with_js(parsed, keyword, 1, host, engine.as_ref()) {
        Ok(hits) => WorkerOutcome::Success {
            books: hits.iter().map(|hit| hit.to_book(&source_id)).collect(),
            source_id,
        },
        Err(error) => WorkerOutcome::Failed {
            source_id,
            error: error.to_error_kind(),
        },
    }
}

/// 并行搜索的收集结果：`(成功, 失败)`。
pub type ParallelSearchOutcome = (
    Vec<(String, Vec<Book>)>,
    Vec<(String, tome_core::ErrorKind)>,
);

/// 一次性收集并行搜索的结果（供 CLI 与测试用）。
///
/// 返回的顺序**与产出顺序一致**（即按源 id 排序）——这是确定性的体现。
pub fn collect_parallel(mut stream: ParallelSearchStream) -> ParallelSearchOutcome {
    let mut successes = Vec::new();
    let mut failures = Vec::new();
    while let Some(item) = stream.next_item() {
        match item {
            SearchResultItem::SourceResults { source_id, books } => {
                successes.push((source_id, books))
            }
            SearchResultItem::SourceFailed { source_id, error } => {
                failures.push((source_id, error))
            }
        }
    }
    (successes, failures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tome_core::{ErrorKind, HeaderMap, RequestSpec, Response};

    const SEARCH_HTML: &str = r#"<html><body><ul>
      <li class="item"><h3 class="name"><a href="/b/1">书名一</a></h3></li>
    </ul></body></html>"#;

    fn source(id: &str) -> ParsedSource {
        let text = format!(
            r#"{{"bookSourceName":"{id}","bookSourceUrl":"{id}",
                 "searchUrl":"/s?q={{{{key}}}}",
                 "ruleSearch":"{{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"bookUrl\":\"@css:.name>a@href\"}}"}}"#
        );
        crate::source::parse_sources(&text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    /// 假宿主：记录调用，可控延迟与成败。
    struct FakeHost {
        delay_ms: u64,
        /// 哪些源的请求应失败
        fail_containing: Vec<String>,
        calls: Mutex<Vec<String>>,
        /// 并发度观测：当前在跑的请求数 / 峰值
        in_flight: AtomicUsize,
        peak_in_flight: AtomicUsize,
    }

    impl FakeHost {
        fn new(delay_ms: u64) -> Self {
            Self {
                delay_ms,
                fail_containing: Vec::new(),
                calls: Mutex::new(Vec::new()),
                in_flight: AtomicUsize::new(0),
                peak_in_flight: AtomicUsize::new(0),
            }
        }

        fn failing(delay_ms: u64, patterns: &[&str]) -> Self {
            Self {
                fail_containing: patterns.iter().map(|s| s.to_string()).collect(),
                ..Self::new(delay_ms)
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn peak(&self) -> usize {
            self.peak_in_flight.load(Ordering::SeqCst)
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(spec.url.clone());

            // 并发度观测
            let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_in_flight.fetch_max(current, Ordering::SeqCst);

            if self.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(self.delay_ms));
            }

            self.in_flight.fetch_sub(1, Ordering::SeqCst);

            if self
                .fail_containing
                .iter()
                .any(|pattern| spec.url.contains(pattern))
            {
                return Err(ErrorKind::Network);
            }
            Ok(Response {
                status: 200,
                headers: HeaderMap::new(),
                body_bytes: SEARCH_HTML.as_bytes().to_vec(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    fn sources(count: usize) -> Vec<ParsedSource> {
        (0..count)
            .map(|index| source(&format!("https://s{index:02}.com")))
            .collect()
    }

    // --- 基本产出 ---

    #[test]
    fn 并行搜索产出全部源的结果() {
        let host = Arc::new(FakeHost::new(0));
        let stream = parallel_search(
            sources(5),
            "关键词".to_string(),
            host.clone(),
            None,
            MAX_SEARCH_WORKERS,
        );
        let (successes, failures) = collect_parallel(stream);
        assert_eq!(successes.len(), 5);
        assert!(failures.is_empty());
        assert_eq!(host.calls().len(), 5, "每个源一次请求");
    }

    #[test]
    fn 产出顺序按源_id_排序() {
        // 确定性：顺序不取决于网络时序，否则 --record 的语料字节会变，
        // T029 的"重放逐字节一致"失效
        let host = Arc::new(FakeHost::new(0));
        // 故意打乱输入顺序
        let mut list = sources(6);
        list.swap(0, 5);
        list.swap(1, 3);

        let stream = parallel_search(list, "k".to_string(), host, None, MAX_SEARCH_WORKERS);
        let (successes, _) = collect_parallel(stream);
        let ids: Vec<&str> = successes.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "https://s00.com",
                "https://s01.com",
                "https://s02.com",
                "https://s03.com",
                "https://s04.com",
                "https://s05.com",
            ],
            "输出必须按源 id 排序，与输入顺序和完成时序都无关"
        );
    }

    #[test]
    fn 多次运行产出顺序一致() {
        // 顺序确定性的实证：跑 5 次，每次的 id 序列必须相同
        let mut sequences = Vec::new();
        for _ in 0..5 {
            let host = Arc::new(FakeHost::new(1));
            let stream =
                parallel_search(sources(8), "k".to_string(), host, None, MAX_SEARCH_WORKERS);
            let (successes, _) = collect_parallel(stream);
            sequences.push(
                successes
                    .iter()
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>(),
            );
        }
        for sequence in &sequences[1..] {
            assert_eq!(sequence, &sequences[0], "每次运行的产出顺序必须相同");
        }
    }

    // --- 并行度 ---

    #[test]
    fn 真的并行执行() {
        // 每个请求睡 100ms，8 个源、8 个 worker → 总时长应明显短于串行的
        // 800ms。若实现是串行的，这里会接近 800ms。
        let host = Arc::new(FakeHost::new(100));
        let start = std::time::Instant::now();
        let stream = parallel_search(
            sources(8),
            "k".to_string(),
            host.clone(),
            None,
            MAX_SEARCH_WORKERS,
        );
        let (successes, _) = collect_parallel(stream);
        let elapsed = start.elapsed();

        assert_eq!(successes.len(), 8);
        eprintln!(
            "[T073 并行度实测] 8 源 × 100ms：串行需 800ms，实测 {elapsed:?}（{:.0}ms），并发峰值 {}",
            elapsed.as_millis(),
            host.peak()
        );
        assert!(
            elapsed < Duration::from_millis(600),
            "8 个源 × 100ms 串行需 800ms；并行应显著更快，实际 {elapsed:?}"
        );
        assert!(
            host.peak() > 1,
            "应观测到多个请求同时在跑（峰值 {}）——并行度的直接证据",
            host.peak()
        );
    }

    #[test]
    fn worker_数上限限制并发度() {
        // 100 个源开 100 个线程会：① 对第三方造成压力（别人风控眼里的
        // 异常流量）② 耗尽本机资源（每 worker 可能要建 QuickJS 沙箱）
        let host = Arc::new(FakeHost::new(50));
        let stream = parallel_search(
            sources(20),
            "k".to_string(),
            host.clone(),
            None,
            3, // 只给 3 个 worker
        );
        let (successes, _) = collect_parallel(stream);
        assert_eq!(successes.len(), 20, "全部源仍应被处理");
        assert!(
            host.peak() <= 3,
            "并发度不得超过 worker 数（峰值 {}）",
            host.peak()
        );
    }

    #[test]
    fn 单_worker_也能跑完() {
        let host = Arc::new(FakeHost::new(0));
        let stream = parallel_search(sources(5), "k".to_string(), host, None, 1);
        let (successes, _) = collect_parallel(stream);
        assert_eq!(successes.len(), 5);
    }

    // --- 单源失败（A6 / SC-006）---

    #[test]
    fn 单源失败不终止流() {
        // SC-006：任一源失败 MUST NOT 影响其余
        let host = Arc::new(FakeHost::failing(0, &["s01", "s03"]));
        let stream = parallel_search(sources(5), "k".to_string(), host, None, MAX_SEARCH_WORKERS);
        let (successes, failures) = collect_parallel(stream);

        assert_eq!(successes.len(), 3, "其余源应正常产出");
        assert_eq!(failures.len(), 2);
        let failed_ids: Vec<&str> = failures.iter().map(|(id, _)| id.as_str()).collect();
        assert!(failed_ids.contains(&"https://s01.com"));
        assert!(failed_ids.contains(&"https://s03.com"));
    }

    #[test]
    fn 全部失败时流正常结束() {
        let host = Arc::new(FakeHost::failing(0, &["s0", "s1"]));
        let stream = parallel_search(sources(3), "k".to_string(), host, None, MAX_SEARCH_WORKERS);
        let (successes, failures) = collect_parallel(stream);
        assert!(successes.is_empty());
        assert_eq!(failures.len(), 3);
    }

    #[test]
    fn 失败条目也按顺序产出() {
        // 排序是对**所有**条目的承诺，不只是成功的那些
        let host = Arc::new(FakeHost::failing(0, &["s01", "s03"]));
        let mut stream =
            parallel_search(sources(5), "k".to_string(), host, None, MAX_SEARCH_WORKERS);
        let mut ids = Vec::new();
        while let Some(item) = stream.next_item() {
            ids.push(match item {
                SearchResultItem::SourceResults { source_id, .. } => source_id,
                SearchResultItem::SourceFailed { source_id, .. } => source_id,
            });
        }
        assert_eq!(
            ids,
            vec![
                "https://s00.com",
                "https://s01.com",
                "https://s02.com",
                "https://s03.com",
                "https://s04.com",
            ]
        );
    }

    #[test]
    fn 空源列表产出空流() {
        let host = Arc::new(FakeHost::new(0));
        let stream = parallel_search(vec![], "k".to_string(), host.clone(), None, 8);
        let (successes, failures) = collect_parallel(stream);
        assert!(successes.is_empty());
        assert!(failures.is_empty());
        assert_eq!(host.calls().len(), 0);
    }

    // --- 取消（FR-012 / A6）---

    #[test]
    fn 取消后立即收尾() {
        // 给足延迟，让取消发生在请求进行中
        let host = Arc::new(FakeHost::new(200));
        let mut stream = parallel_search(
            sources(50),
            "k".to_string(),
            host.clone(),
            None,
            2, // 少量 worker → 队列里还有大量未开始
        );

        // 等第一个结果出来
        let first = stream.next_item().expect("应有第一个结果");
        assert!(matches!(first, SearchResultItem::SourceResults { .. }));

        stream.cancel_token().cancel();
        assert!(
            stream.next_item().is_none(),
            "取消后应立即返回 None（不应把已取消的流跑完）"
        );
    }

    #[test]
    fn 取消后不再发新请求() {
        // 这是"取消生效"唯一可观测的证据：已飞的请求无法中断，
        // 但**不该再取新源**
        let host = Arc::new(FakeHost::new(150));
        let mut stream = parallel_search(sources(60), "k".to_string(), host.clone(), None, 2);

        let _ = stream.next_item().expect("应有结果");
        stream.cancel_token().cancel();
        let _ = stream.next_item();

        let calls_at_cancel = host.calls().len();
        // 给已飞的请求足够时间完成（但它们不该带来新的调用）
        std::thread::sleep(Duration::from_millis(300));
        let calls_after = host.calls().len();

        eprintln!(
            "[T073 取消实测] 60 源 / 2 worker / 每请求 150ms：取消时已发 {calls_at_cancel} 次，300ms 后 {calls_after} 次（未取消会跑满 60 次）"
        );
        assert!(
            calls_after <= calls_at_cancel + 2,
            "取消后不该再取新源（取消时 {calls_at_cancel} 次，300ms 后 {calls_after} 次；\
             60 个源全跑完会有 60 次）"
        );
    }

    #[test]
    fn 取消是幂等的() {
        let host = Arc::new(FakeHost::new(0));
        let stream = parallel_search(sources(3), "k".to_string(), host, None, 2);
        assert!(!stream.cancel_token().is_cancelled());
        stream.cancel_token().cancel();
        stream.cancel_token().cancel();
        assert!(stream.cancel_token().is_cancelled());
    }

    #[test]
    fn 未取消时正常跑完() {
        let host = Arc::new(FakeHost::new(0));
        let mut stream = parallel_search(sources(10), "k".to_string(), host, None, 4);
        let mut count = 0;
        while stream.next_item().is_some() {
            count += 1;
        }
        assert_eq!(count, 10);
        assert!(!stream.cancel_token().is_cancelled());
    }

    // --- 结果内容 ---

    #[test]
    fn 结果是可入库的书籍() {
        let host = Arc::new(FakeHost::new(0));
        let stream = parallel_search(
            vec![source("https://a.com")],
            "k".to_string(),
            host,
            None,
            1,
        );
        let (successes, _) = collect_parallel(stream);
        let books = &successes[0].1;
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].name, "书名一");
        assert_eq!(books[0].origin_id, "https://a.com|https://a.com/b/1");
    }

    #[test]
    fn 空结果也算成功产出() {
        // "源在线但无匹配"与"源失败"是不同的事实，上层都该看到
        struct EmptyHost;
        impl NetHost for EmptyHost {
            fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
                Ok(Response {
                    status: 200,
                    headers: HeaderMap::new(),
                    body_bytes: b"<ul></ul>".to_vec(),
                    detected_charset: Some("utf-8".to_string()),
                })
            }
        }
        let stream = parallel_search(sources(2), "k".to_string(), Arc::new(EmptyHost), None, 2);
        let (successes, failures) = collect_parallel(stream);
        assert_eq!(successes.len(), 2);
        assert!(successes.iter().all(|(_, books)| books.is_empty()));
        assert!(failures.is_empty());
    }
}
