//! `[WIP]` 引擎线程（S3 T007）
//!
//! ## 为什么必须有一条独立线程
//!
//! S1/S2 的能力全是**同步阻塞**的：SQLite 查询、HTTP 抓取、排版（`Paginator`
//! 持一台昂贵的 `FontSystem`，全方法 `&mut self`）。把它们放进 egui 的 UI 线程
//! 会让界面在抓取/排版期间冻结——阅读器不可接受。
//!
//! 故：**UI 线程持 egui 与渲染状态，引擎线程独占 `Paginator`/store/source**，
//! 两者经 [`EngineRequest`]/[`EngineEvent`] 通信（`research.md` R5）。
//!
//! ## 线程纪律（本模块的核心约束）
//!
//! | 约束 | 落实方式 |
//! |---|---|
//! | 引擎对象不离开引擎线程 | [`EngineHandle`] **不是泛型的**，worker 类型被擦除在线程内，无任何访问器 |
//! | 单次请求的 panic 被隔离 | 每个请求边界 `catch_unwind` → [`EngineEvent::EnginePanic`]，循环继续 |
//! | 启动失败不静默 | [`EngineHandle::spawn`] 的启动握手把失败变成 `Err`，拿不到句柄 |
//!
//! ## 为什么 `catch_unwind` 是最后一道网，而不是错误处理手段
//!
//! 引擎层（S1/S2）刻意**不**用 panic 传播做错误处理（见 `tome-reader` 的
//! `error.rs` 模块文档）。本模块的 `catch_unwind` 是**兜底**：它接住的是
//! 缺陷（不该发生的 panic），而不是流程控制。接住之后如实报
//! [`EngineEvent::EnginePanic`]，让用户看到结构化错误并**继续用**——
//! 而不是让整个阅读器进程消失。
//!
//! ## 启动握手的取舍
//!
//! [`EngineHandle::spawn`] **阻塞**到引擎线程的启动阶段结束（建数据目录、
//! 开库、迁移）。这与"别阻塞 UI 线程"看似矛盾，取舍如下：
//!
//! - 这段 IO 正是**冷启动**的固有成本（CLI 在启动时同步做同一件事），
//!   落在 SC-001 的 1.5s 预算内；
//! - 换来的是启动失败**只能是 `Err`**——若改成"失败时发一个事件"，上层就
//!   可能拿着一个永不服务事件的句柄继续跑，而那正是 FR-023 要防的静默失败。
//!
//! 调用点在 `eframe` 的创建闭包里（窗口已存在，失败时渲染结构化错误页）。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::messages::{ClassifiedErrorView, EngineEvent, EngineRequest};

/// 引擎线程能做的事（一次请求 → 零到多条事件）。
///
/// ## 为什么是 trait 而不是具体类型
///
/// 真实实现要持 `BookRepository`/`SourceEngine` 的 adapter（T010/T023 落地），
/// 而本任务（T007）要先把**线程与兜底的骨架**钉死并测通。trait 让两者解耦：
/// 骨架的测试用合成 worker（panic 是刻意要求的，不依赖真实依赖偶发失败），
/// 真实 worker 随后接上，判据不变。
///
/// ## 为什么 `&mut self`
///
/// worker 独占 `Paginator`/store/source，而这些 API 都是 `&mut self`。把它
/// 写成 `&self` 会迫使实现内部用 `RefCell`/`Mutex`——那只是把"独占"从类型
/// 层面挪到运行期，还多一次借用检查。独占本来就是这个线程存在的理由。
pub trait EngineWorker: Send + 'static {
    /// 处理一个请求，返回零到多条事件。
    fn handle(&mut self, request: EngineRequest) -> Vec<EngineEvent>;

    /// 距下次需要"定时唤醒"还有多久（`None` = 不需要定时唤醒）。
    ///
    /// ## 为什么这个能力在 worker 而不在服务循环
    ///
    /// "什么时候该做一次后台工作"（如进度落盘窗口到点）取决于 worker 自己的
    /// 状态。服务循环若自己去猜一个轮询间隔，就必然出现"该落盘的时刻晚了一个
    /// 间隔"——而 SC-009 的丢失量口径正是那个窗口本身。
    ///
    /// 默认实现返回 `None`：不关心定时工作的 worker 一行都不用改。
    fn next_wakeup(&self) -> Option<std::time::Duration> {
        None
    }

    /// 定时唤醒时的工作（返回零到多条事件）。
    ///
    /// 只有在 [`Self::next_wakeup`] 返回 `Some` 之后才可能被调用。
    fn on_wakeup(&mut self) -> Vec<EngineEvent> {
        Vec::new()
    }
}

/// 引擎线程启动失败的**结构化**报告（FR-023 / S1 FR-043）。
///
/// ## 为什么三个字段都要
///
/// S1 的 FR-043 原话是「明确报告并给出指引，**而非静默失败**」。只报"启动
/// 失败"等于把排查责任推给用户；只报细节不给指引等于告诉用户"坏了"却不告诉
/// 他怎么办。故：标识（供 i18n 查表 + 检索）、细节（发生了什么）、指引
/// （下一步做什么）三者缺一不可。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupFailure {
    error: ClassifiedErrorView,
    detail: String,
    guidance: String,
}

impl StartupFailure {
    /// 构造一份启动失败报告。
    #[must_use]
    pub fn new(
        error: ClassifiedErrorView,
        detail: impl Into<String>,
        guidance: impl Into<String>,
    ) -> Self {
        Self {
            error,
            detail: detail.into(),
            guidance: guidance.into(),
        }
    }

    /// 结构化错误（标识 + 能力，供 i18n 与呈现层）。
    #[must_use]
    pub fn error(&self) -> &ClassifiedErrorView {
        &self.error
    }

    /// 具体原因（开发者向诊断）。
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// 可行动的指引（面向用户）。
    #[must_use]
    pub fn guidance(&self) -> &str {
        &self.guidance
    }
}

impl std::fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}: {}（指引：{}）",
            self.error.id, self.detail, self.guidance
        )
    }
}

impl std::error::Error for StartupFailure {}

/// 引擎线程句柄（UI 侧唯一能持有的东西）。
///
/// ## 为什么**不是**泛型的（这条是刻意的）
///
/// worker 持 `Paginator`（含 `FontSystem`）与 store/source 句柄。若句柄带上
/// worker 类型参数，上层就能 `handle.worker()` 把它拿到 UI 线程里用——
/// "独占"随即从**结构性事实**退化成**约定**，而并发问题要到线上才现形。
///
/// 类型擦除（worker 被 `move` 进线程闭包，句柄只留通道）让"拿不到"成为
/// 编译期事实。`tests/engine_thread.rs` 有断言钉住这一点。
pub struct EngineHandle {
    requests: Sender<EngineRequest>,
    events: Arc<Mutex<Receiver<EngineEvent>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EngineHandle {
    /// 起一条引擎线程。
    ///
    /// `build` 在**引擎线程上**执行（数据目录/库/迁移都在那儿），其失败经启动
    /// 握手回传为本函数的 `Err`——故启动失败**不可能**被静默忽略：拿不到句柄
    /// 就没有后续。
    ///
    /// # Errors
    ///
    /// `build` 返回 `Err` 时原样透出；引擎线程创建失败时也返回 `Err`。
    pub fn spawn<W, B>(build: B) -> Result<Self, StartupFailure>
    where
        W: EngineWorker,
        B: FnOnce() -> Result<W, StartupFailure> + Send + 'static,
    {
        let (requests, request_rx) = channel::<EngineRequest>();
        let (events, event_rx) = channel::<EngineEvent>();
        // 启动握手：容量 1 的同步通道——线程发一次，主线程收一次。
        let (startup_tx, startup_rx) =
            std::sync::mpsc::sync_channel::<Result<(), StartupFailure>>(1);

        let thread = std::thread::Builder::new()
            .name("tome-engine".to_owned())
            .spawn(move || {
                // --- 启动阶段：建 worker（IO 都在这里）---
                let worker = match build() {
                    Ok(worker) => {
                        // 通知主线程"启动成功"，然后进入服务循环
                        let _ = startup_tx.send(Ok(()));
                        worker
                    }
                    Err(failure) => {
                        // 通知主线程失败并**结束本线程**（不留空转线程）
                        let _ = startup_tx.send(Err(failure));
                        return;
                    }
                };

                serve(worker, request_rx, events);
            })
            .map_err(|error| {
                StartupFailure::new(
                    ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Configuration),
                    format!("无法创建引擎线程：{error}"),
                    "请检查系统资源限制（线程数上限），或重启应用",
                )
            })?;

        // 等启动阶段的结果。这里**必须**等：见模块文档的取舍说明。
        match startup_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                requests,
                events: Arc::new(Mutex::new(event_rx)),
                thread: Some(thread),
            }),
            Ok(Err(failure)) => {
                // 线程已在上面 return，join 只是回收资源
                let _ = thread.join();
                Err(failure)
            }
            Err(_) => {
                // 通道断了 = 线程在发结果前就没了（如 build 里 abort 级崩溃）
                let _ = thread.join();
                Err(StartupFailure::new(
                    ClassifiedErrorView::from_kind(&tome_core::ErrorKind::Storage),
                    "引擎线程在启动阶段意外终止",
                    "请重试；若反复出现，请检查数据目录是否可用",
                ))
            }
        }
    }

    /// 发一个请求（非阻塞）。
    ///
    /// 返回 `false` 表示引擎线程已结束（句柄被 shutdown 或线程已退出）。
    /// **不 panic**：GUI 里"引擎没了"应呈现为错误态，而不是让 UI 也崩。
    pub fn send(&self, request: EngineRequest) -> bool {
        self.requests.send(request).is_ok()
    }

    /// 取走当前累积的全部事件（非阻塞）。
    ///
    /// 每帧调一次即可——这是"UI 拉取"而非"引擎推送"，因为 egui 的重绘节奏
    /// 由 UI 决定（`request_repaint`），推送只会造成无谓的唤醒。
    #[must_use]
    pub fn drain_events(&self) -> Vec<EngineEvent> {
        let Ok(events) = self.events.lock() else {
            // 中毒：某个持锁者 panic 了。此处**不**继续 panic（那会把中毒扩散
            // 到 UI 线程），返回空并让上层按"没有新事件"处理。
            return Vec::new();
        };
        events.try_iter().collect()
    }

    /// 关停引擎线程并等它结束。
    ///
    /// ## 为什么是 `&mut self` 且消费掉 `JoinHandle`
    ///
    /// 关停是**一次性**的：重复调用要么是逻辑错误（那该编译期挡住），要么
    /// 是并发关停（那需要更复杂的语义）。`&mut self` + `Option::take` 让
    /// "关停两次"在类型上就不可能——第二次拿到 `None`，无事可做。
    pub fn shutdown(mut self) {
        self.shutdown_in_place();
    }

    /// 关停的内部形态（供 `Drop` 复用）。
    fn shutdown_in_place(&mut self) {
        // 关掉请求发送端：引擎线程的 `recv` 会得到 `Disconnected`，循环退出。
        //
        // 用一个"空请求"来表达关停是不可靠的（worker 可能恰好忽略它），
        // 而断开通道是**结构性**的：`recv()` 的返回值本身就带这个信息。
        let (dead, _) = channel::<EngineRequest>();
        let _ = std::mem::replace(&mut self.requests, dead);

        if let Some(thread) = self.thread.take() {
            // join 失败 = 引擎线程 panic 了。那是缺陷，但**不该在 Drop 里再 panic**
            // （析构中 panic 会直接终止进程，且往往发生在真正的错误路径上）。
            // 兜底：忽略，因为 catch_unwind 已经把每次请求的 panic 转成了事件，
            // 走到这里说明是循环之外的 panic——那是另一类问题，不该连带杀死 UI。
            let _ = thread.join();
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.shutdown_in_place();
    }
}

/// 引擎线程的服务循环（**每次请求一个 `catch_unwind` 边界**）。
///
/// ## 为什么边界在"每个请求"而不是"整个循环"
///
/// 若把整个循环包在 `catch_unwind` 里，一次 panic 就结束了服务——用户看到的
/// 是"读某章崩了，之后整个阅读器不再响应"。边界放在请求粒度上，panic 的
/// 影响面就被限制在**那一次请求**内（spec 边缘情况要的正是这个隔离粒度）。
///
/// ## 定时唤醒（T015 的进度节流依赖它）
///
/// 循环在等待请求时，若 worker 声明了"还有多久需要醒来"（
/// [`EngineWorker::next_wakeup`]），就用 `recv_timeout` 而不是无限阻塞的
/// `recv`。到点后调 [`EngineWorker::on_wakeup`]，把"窗口到点该落盘了"这类
/// 后台工作交给 worker 自己处理。
///
/// 这条通道是必要的：进度落盘的状态（脏标记 + 上次落盘时刻）只存在于引擎
/// 线程，UI 线程无从知道"现在该落盘了吗"。若让 UI 定期发一个请求来问，状态
/// 就有了两份。
///
/// ## `AssertUnwindSafe` 的理由
///
/// `catch_unwind` 要求闭包 `UnwindSafe`，而 worker 是 `&mut`（可变引用天然
/// 不是 `UnwindSafe`）。这里的断言**成立**，理由是：panic 之后我们**不**继续
/// 用这个 worker 的旧状态做假设——循环继续，但每次 `handle` 都被当作独立
/// 操作（worker 自己负责"单次请求失败不留半成品"）。若不这么约定，兜底就
/// 只是掩盖了状态污染。
fn serve<W: EngineWorker>(
    mut worker: W,
    requests: Receiver<EngineRequest>,
    events: Sender<EngineEvent>,
) {
    loop {
        // 有定时工作就带超时等，否则无限等（不空转）
        let request = match worker.next_wakeup() {
            Some(timeout) => match requests.recv_timeout(timeout) {
                Ok(request) => Some(request),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match requests.recv() {
                Ok(request) => Some(request),
                Err(_) => return,
            },
        };

        let produced = match request {
            Some(request) => {
                let context = describe(&request);
                match catch_unwind(AssertUnwindSafe(|| worker.handle(request))) {
                    Ok(produced) => produced,
                    // 兜底：如实报结构化事件，**循环继续**（线程存活可继续服务）
                    Err(_) => vec![EngineEvent::EnginePanic { context }],
                }
            }
            None => match catch_unwind(AssertUnwindSafe(|| worker.on_wakeup())) {
                Ok(produced) => produced,
                Err(_) => vec![EngineEvent::EnginePanic {
                    context: "on_wakeup".to_owned(),
                }],
            },
        };

        for event in produced {
            if events.send(event).is_err() {
                // 接收端没了（UI 已退出）——引擎线程该结束了
                return;
            }
        }
    }
}

/// 请求的简短描述（`EnginePanic.context` 的内容）。
///
/// ## 为什么不用 `Debug`
///
/// `format!("{request:?}")` 会把整本书的路径/URL/搜索词拼进 context——那既
/// 冗长又可能含用户数据。这里只取**变体名**，足够定位"哪个动作炸了"。
fn describe(request: &EngineRequest) -> String {
    let variant = match request {
        EngineRequest::ShelfLoad => "ShelfLoad",
        EngineRequest::ShelfImportLocal { .. } => "ShelfImportLocal",
        EngineRequest::ShelfRemove { .. } => "ShelfRemove",
        EngineRequest::ReadingOpen { .. } => "ReadingOpen",
        EngineRequest::ReadingClose => "ReadingClose",
        EngineRequest::ReadingSetParams { .. } => "ReadingSetParams",
        EngineRequest::ReadingTurnPage { .. } => "ReadingTurnPage",
        EngineRequest::ReadingSeekAnchor { .. } => "ReadingSeekAnchor",
        EngineRequest::ReadingLoadChapter { .. } => "ReadingLoadChapter",
        EngineRequest::ReadingViewport { .. } => "ReadingViewport",
        EngineRequest::TocLoad { .. } => "TocLoad",
        EngineRequest::BookmarkAdd { .. } => "BookmarkAdd",
        EngineRequest::BookmarkRemove { .. } => "BookmarkRemove",
        EngineRequest::BookmarkList { .. } => "BookmarkList",
        EngineRequest::SourceImportFile { .. } => "SourceImportFile",
        EngineRequest::SourceImportUrl { .. } => "SourceImportUrl",
        EngineRequest::SourceImportText { .. } => "SourceImportText",
        EngineRequest::SourceSearch { .. } => "SourceSearch",
        EngineRequest::SourceCancelSearch { .. } => "SourceCancelSearch",
        EngineRequest::SourceDetail { .. } => "SourceDetail",
        EngineRequest::SourceAddToShelf { .. } => "SourceAddToShelf",
        EngineRequest::SourceSwitch { .. } => "SourceSwitch",
        EngineRequest::ProgressFlush { .. } => "ProgressFlush",
        EngineRequest::SettingsLoad => "SettingsLoad",
        EngineRequest::SettingsSave { .. } => "SettingsSave",
    };
    variant.to_owned()
}
