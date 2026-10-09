//! `SourceEngine` / `ContentProvider` 接缝（`contracts/seams.md` §1 / §2）

use crate::error::ErrorKind;
use crate::model::{Book, Chapter, Source};
use crate::seam::stream::SearchStream;

/// 书源引擎：书源五动作 + 换源 + 源检查（`contracts/seams.md` §1）。
///
/// 实现方：`tome-source`。
///
/// ## 契约要点
///
/// - `search` 返回**流 + 取消令牌**（A6），不阻塞至全部源完成；
///   单源失败是流内的一个条目，不终止整个流
/// - `toc` 只管目录；正文在 [`ContentProvider`]（职责分离，避免上层拿到
///   "半个引擎"）
/// - `change_source` 必须标明采用了哪一级进度映射（FR-010b）
///
/// ## 错误模式
///
/// 源不存在 → [`ErrorKind::Rule`]；网络失败 → [`ErrorKind::Network`]；
/// WebView 类能力 → [`ErrorKind::UnsupportedCapability`]（结构化、可被上层
/// 识别，FR-006）。
#[allow(async_fn_in_trait)]
pub trait SourceEngine {
    /// 关键词搜索（可限定源范围）。返回增量流。
    ///
    /// `source_ids` 为空表示搜索全部启用的源。返回的流可被其 [`SearchStream::cancel_token`]
    /// 取消（FR-012「结果须增量呈现且整体可取消」）。
    fn search(&self, keyword: &str, source_ids: &[String]) -> Box<dyn SearchStream>;

    /// 发现（分类遍历），FR-013 支持导航与分页。
    fn discover(&self, source_id: &str, kind: &str, page: u32) -> Result<ExplorePage, ErrorKind>;

    /// 书籍详情（FR-002 第三类动作）。
    ///
    /// **与契约的字面差异**：`contracts/seams.md` §1 写作 `→ Result<BookInfo>`，
    /// 此处返回 [`Book`]。理由是数据模型里**没有**独立的 BookInfo 实体
    /// （`data-model.md` 的 `book` 表即承载书名/作者/封面/简介/分类等全部详情字段），
    /// 故 `BookInfo` 与 `Book` 会是同一份数据的两个名字——多一个类型只会制造
    /// 无意义的转换。此处取"实体即契约"的解读，并在测试中覆盖字段可用性。
    fn book_info(&self, source_id: &str, book_url: &str) -> Result<Book, ErrorKind>;

    /// 目录（FR-002 第四类动作）。性能目标：5 万章 <1s（SC-003）。
    fn toc(&self, source_id: &str, book: &Book) -> Result<Vec<Chapter>, ErrorKind>;

    /// 换源：把一本书迁移到候选源，并尽量保留阅读进度（FR-010 / FR-010b）。
    fn change_source(
        &self,
        book: &Book,
        candidate_source_id: &str,
    ) -> Result<ChangeSourceOutcome, ErrorKind>;

    /// 单源可用性检查（FR-011），给出可判定结论。
    fn check_source(&self, source_id: &str) -> Result<CheckReport, ErrorKind>;

    /// 批量源检查（FR-011），**流式**返回。
    ///
    /// `filter` 为空表示检查全部启用的源。
    ///
    /// ## 为什么是流而不是 `Vec<CheckReport>`
    ///
    /// 与 [`SourceEngine::search`] 同理（A6）：批量检查可能涉及上百个源，
    /// 每个源都要发请求。用 `Vec` 会把"检查进度"完全藏起来，用户面对一个
    /// 长时间无反馈的界面。流式返回让界面能逐源展示进度与结果。
    ///
    /// 单源检查失败是**流内的一个条目**（`CheckReport` 的 verdict 为
    /// [`CheckVerdict::Unavailable`]），不终止整个流。
    fn check_sources(&self, filter: &SourceCheckFilter) -> Box<dyn CheckStream>;
}

/// 批量源检查的过滤条件（[`SourceEngine::check_sources`] 的输入）。
///
/// 用结构体而非 `Option<&[String]>`：日后要加"按分组检查""只查未验证过的源"
/// 等条件时，不必改动 trait 签名（那会波及所有实现与调用方）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceCheckFilter {
    /// 限定书源 id；为空表示全部启用的源
    pub source_ids: Vec<String>,
    /// 限定分组；为空表示不限分组
    pub group_id: Option<i64>,
}

/// 批量源检查的结果流（FR-011）。
///
/// 契约与 [`crate::seam::SearchStream`] 一致：逐源产出、可取消、
/// 单源失败不终止流。故同样必须对象安全（界面持 `Box<dyn CheckStream>`）。
pub trait CheckStream: Send {
    /// 取下一个源的检查结果；`None` 表示全部完成。
    fn next_report(&mut self) -> Option<CheckReport>;

    /// 本流的取消令牌。
    fn cancel_token(&self) -> &crate::seam::CancelToken;
}

/// 发现页的一页结果（FR-013 分页）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorePage {
    /// 该页的书籍条目
    pub books: Vec<Book>,
    /// 下一页页码；`None` 表示没有下一页
    pub next_page: Option<u32>,
}

/// 换源结果：**必须**标明采用了哪一级进度映射（FR-010b）。
///
/// ## 为什么进度映射是返回值的核心而非附带信息
///
/// FR-010b 明确要求"换源过程 MUST 可观测（告知用户采用了哪一级匹配及其结果）"，
/// 且第三级要求"明确提示用户进度无法自动映射并落到目录开头，**不静默跳转**"。
/// 把级别放进返回值，使"静默降级"在类型层面就不成立——调用方必须处理它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSourceOutcome {
    /// 迁移后的书籍（新源的 origin_id / toc_url / 章节）
    pub book: Book,
    /// 新源的目录
    pub chapters: Vec<Chapter>,
    /// 采用的进度映射级别及其结果
    pub progress_mapping: ProgressMapping,
}

/// 进度映射的级别与结果（FR-010b 三级降级）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressMapping {
    /// 第一级：特征段匹配（在新源邻域章节缓存内按约 90% 相似度搜索）
    AnchorSegment {
        /// 映射到的章节序号
        chapter_index: i32,
        /// 映射到的章内字符偏移
        char_offset: i64,
        /// 实际相似度（供上层展示"以 x% 相似度定位"）
        similarity_percent: u8,
    },
    /// 第二级：章节标题近似匹配
    ChapterTitle {
        /// 映射到的章节序号
        chapter_index: i32,
    },
    /// 第三级：均未命中，落到目录开头。
    ///
    /// **不是静默行为**——FR-010b 要求明确提示用户，故它是一个显式变体，
    /// 调用方据此展示提示。
    FallbackToStart,
}

/// 源检查报告（FR-011）。结论须可判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    /// 被检查的源
    pub source: Source,
    /// 可用性结论
    pub verdict: CheckVerdict,
}

/// 源检查的判定结论（FR-011「给出可判定的结论」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckVerdict {
    /// 可用
    Available,
    /// 不可用，附分类原因
    Unavailable {
        /// 失败分类
        reason: ErrorKind,
    },
}

/// 统一内容提供者（`contracts/seams.md` §2，架构审查 A4）。
///
/// ## 两个 adapter，故这是真 seam
///
/// - **网络书**：先查 Pack 缓存，未命中则抓取并写入缓存（崩溃安全 FR-030）
/// - **本地书**：按偏移直读
///
/// 上层（S2 阅读器）只认这个 trait，不关心书从哪来。
///
/// ## 关键不变量：净化管线在 seam 下游
///
/// FR-014 的处理顺序（替换 → 高亮 → 词典）由**本 seam 之下的统一管线**施加，
/// 网络书与本地书一视同仁。故本 trait 的方法签名里看不到净化参数——
/// 那是实现方的内部关切，不该泄漏到接缝。
pub trait ContentProvider {
    /// 目录。对网络书可能来自缓存或抓取，对本地书来自解析结果。
    fn toc(&self, book: &Book) -> Result<Vec<Chapter>, ErrorKind>;

    /// 取某章正文（净化管线已应用）。
    fn chapter_content(&self, book: &Book, chapter_index: i32)
        -> Result<ChapterContent, ErrorKind>;

    /// 锚点解析（SC-008）：把持久化的锚点解析为当前书中的真实位置。
    ///
    /// **与排版参数无关**——任意排版参数下解析为同一锚点，这是 SC-008 的核心。
    fn resolve_anchor(&self, book: &Book, anchor: &AnchorQuery) -> Result<(i32, i64), ErrorKind>;

    /// 预取（顺读预热）。**失败无副作用**：预取是优化，失败不应影响阅读。
    fn prefetch(&self, book: &Book, around_index: i32, depth: u32);
}

/// 章正文（净化管线已应用的结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterContent {
    /// 所属章节序号
    pub chapter_index: i32,
    /// 章节标题
    pub title: String,
    /// 正文（字符区间以**本字段的字符下标**为基准）
    pub content: String,
    /// 命中缓存的来源标记（供上层展示或调试）
    pub from_cache: bool,
}

/// 锚点查询（`resolve_anchor` 的输入）。
///
/// 三要素与 [`crate::model::ReadProgress`] 的锚点一致：定位 + 语义校验文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorQuery {
    /// 期望的章节序号（作为搜索起点）
    pub chapter_index: i32,
    /// 期望的章内字符偏移
    pub char_offset: i64,
    /// 锚点处的文本片段（供语义校验/特征段匹配）
    pub anchor_text: Option<String>,
}
