//! 流式结果与取消令牌（架构审查 A6）
//!
//! # 为什么 `search` 的返回形状是"流 + 取消令牌"而不是 `Vec`
//!
//! 契约 `seams.md` §1 关键不变量：`search` 必须**增量返回**（首个结果尽快），
//! 不能阻塞至全部源完成。多源搜索里慢源/死源很常见，`Vec` 形状会强迫调用方
//! 等到最慢的源超时——用户看到"搜索中"却一直没有结果。
//!
//! A6 要求单源失败是**流内的一个条目**（带 [`crate::ErrorKind`]），
//! 不终止整个流。故流的元素类型是 [`SearchResultItem`] 而非裸的书籍列表。
//!
//! # 为什么不直接用 `futures::Stream`
//!
//! core 刻意零外部依赖（见 crate 文档）。引入 `futures-core` 只为一个 trait
//! 边界不划算——而引擎侧需要同步语义（书源脚本是同步的，research.md R1）。
//! 故此处定义一个最小的拉取式接口，由实现 crate 适配到具体并发模型。

use crate::error::ErrorKind;
use crate::model::Book;

/// 取消令牌：请求中止一个正在进行的流式操作。
///
/// 用 `Arc` 包裹以便跨线程共享（多源并发时各 worker 都要能看到取消信号）。
/// 实现方应**尽快**响应取消并停止后续工作，不应把已取消的流跑完。
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CancelToken {
    /// 新建未取消的令牌。
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消。可被多次调用（幂等）。
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 是否已请求取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// 搜索结果流中的一个条目。
///
/// 单源的结果与失败**共用同一形状**：这样"某个源挂了"不会中断整个流
/// （A6 / FR-012「单源失败或超时 MUST NOT 影响其他源」）。
#[derive(Debug, Clone)]
pub enum SearchResultItem {
    /// 某个源返回的一批结果
    SourceResults {
        /// 产生这些结果的源
        source_id: String,
        /// 该源的结果（可能是空列表：源在线但无匹配）
        books: Vec<Book>,
    },
    /// 某个源失败了。**不是**流的终止条件。
    SourceFailed {
        /// 失败的源
        source_id: String,
        /// 失败分类（上层据此决定是否提示用户）
        error: ErrorKind,
    },
}

/// 搜索结果的拉取式流。
///
/// ## 契约
///
/// - `next_item` 返回 `None` 表示**全部源已完成**（含失败的源）
/// - 每次返回一个 [`SearchResultItem`]，实现方应逐源产出而非攒齐再给
/// - 调用方持 [`CancelToken`] 可请求中止；实现方须检查 `is_cancelled`
///
/// ## 为什么是对象安全的方法而非关联类型 + `Iterator`
///
/// 上层把 `SearchStream` 当作 `Box<dyn SearchStream>` 存放在界面状态里，
/// 故 trait 必须对象安全（`Iterator` 的关联类型使其不能直接 `dyn`）。
pub trait SearchStream: Send {
    /// 取下一个条目；`None` 表示流已结束。
    fn next_item(&mut self) -> Option<SearchResultItem>;

    /// 本流的取消令牌。调用方 `cancel()` 后实现方须尽快收尾。
    fn cancel_token(&self) -> &CancelToken;
}
