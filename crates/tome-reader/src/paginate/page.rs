//! [DONE] 页序列（T019 / FR-012、FR-014、FR-004）
//!
//! ## 一页是什么
//!
//! data-model.md §5：页 = 字符区间 + 行盒列表 + 左右归属 + 留白标记。
//!
//! **位置一律字符区间**（FR-012："MUST NOT 以渲染坐标表达分页结果"）。
//! 本模块产出的每个位置都能与 001 的 `ReadProgress.char_offset` 直接比较。
//!
//! ## 分页算法：为什么是"贪心装行"而不是"按高度切"
//!
//! 一页能容下哪些行，取决于**行的累计高度**。故流程是：
//!
//! ```text
//! 正文 → 段落切分 → 逐段排版成行盒 → 按累计高度贪心装页
//!                     ↑ 每行带段内与章内两套区间
//! ```
//!
//! 贪心的正确性由 SC-006 的后验断言保证（区间拼接逐字符等于原文）——
//! 不靠"算法看起来对"。
//!
//! ## 两套字符区间（本模块最容易出错的地方）
//!
//! 行盒（[`crate::layout::linebox::LineBox`]）的 `char_range` 是**段内**偏移，
//! 而页区间是**章内**偏移。两者必须显式换算，否则多段正文会整体错位。
//!
//! | 结构 | 区间口径 | 为什么 |
//! |---|---|---|
//! | [`LineBox::char_range`] | 段内 | 排版引擎每次只吃一段（T013 的设计） |
//! | [`PageLine::chapter_char_range`] | **章内** | 页是章的一段，与 001 锚点同口径 |
//! | [`Page::char_range`] | **章内** | FR-012 的落点 |
//!
//! 换算在装页时发生**一次**，之后全链路都是章内口径。
//!
//! ## 段间分隔符的归属（SC-006 的隐性陷阱）
//!
//! 段落切分产出的段文本**不含**换行符。故若页区间取"本页行盒的并集"，
//! 段间的换行符会落在**所有页之外**——SC-006 的逐字符比对直接失败，
//! 而现象只是"拼接结果少了一个换行"。
//!
//! 本模块的口径：**页 i 覆盖到下一页首个字符为止**。段间的一切
//! （换行符、被段落切分丢弃的空白段）由**前一页**认领。

use crate::layout::engine::{Geometry as LayoutGeometry, LayoutEngine};
use crate::layout::linebox::LineBox;
use crate::paginate::config::GeometryConfig;
use crate::paginate::spread::{column_measure_px, resolve_override, Columns, SpreadSide, Viewport};
use crate::text::model::TextModel;
use crate::text::paragraph;

/// 页上的一个行盒（携带**章内**区间）。
///
/// ## 为什么包一层而不直接放 [`LineBox`]
///
/// `LineBox::char_range` 是段内偏移。若把它直接放进页里，页的使用者
/// （痕迹渲染 FR-037、命中测试 FR-040）每次都要自己去查"这行属于哪一段、
/// 段的章内起点在哪"——那是一次极易写错的换算，且写错的表现是"跨段
/// 位置偏移"，很难从结果反推。
///
/// 包一层后，章内区间在**装页时算好一次**，使用者直接取用。
#[derive(Debug, Clone, PartialEq)]
pub struct PageLine {
    /// 行盒（其 `char_range` 是**段内**偏移）
    pub line: LineBox,
    /// 该行在**章内**的字符区间（= 段内区间 + 段的章内起点）
    pub chapter_char_range: std::ops::Range<usize>,
}

/// 页（data-model.md §5）。
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// 章节序号（0 起，与 001 的 `chapter_index` 同口径）
    pub chapter_index: usize,
    /// 章内页码（0 起）
    pub page_in_chapter: u32,
    /// 页内容的**章内**字符区间（留白页为空区间）
    pub char_range: std::ops::Range<usize>,
    /// 本页的行盒序列（章内区间已换算）
    pub lines: Vec<PageLine>,
    /// 双页模式下的左右归属；单页为 `None`
    pub side: Option<SpreadSide>,
    /// 是否为书页语义的**留白页**（FR-016；SC-006 不计入内容区间但须显式标记）
    pub is_padding: bool,
}

impl Page {
    /// 页内容的字符长度。
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.char_range.end - self.char_range.start
    }

    /// 是否为空页（无行、无内容区间）。
    ///
    /// ## 与 `is_padding` 的区别
    ///
    /// "空"是几何事实（这页没排下任何东西），"留白"是**语义标记**
    /// （这页是为了书页语义而刻意留的，见 FR-016）。一页可以是留白
    /// 但非空（罕见），也可以是非留白的空页（空章的首页）。两者不可互相
    /// 代替——SC-006 的口径只看 `is_padding`。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.char_range.is_empty()
    }
}

/// 实际生效的分栏与版心（分页的几何输入，供 CLI 回显与测试断言）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpreadResolution {
    /// 实际生效的栏数（含手动覆盖的结果，FR-015）
    pub columns: Columns,
    /// 单栏版心宽度（px）
    pub measure_px: f32,
    /// 手动覆盖是否仍在生效（FR-015 第二半句）
    pub override_active: bool,
}

/// 锚点的定位结果（FR-017 / 001 §SC-008）。
///
/// ## 为什么两形态共用同一个类型（FR-021）
///
/// FR-021 明文："系统 MUST NOT 为滚动模式引入第二套坐标或第二套字符下标
/// 口径。" 故翻页与滚动的定位差异落在 [`AnchorTarget`] 的**变体**里，而
/// 锚点值本身（`chapter_index` / `char_offset`）是同一个字段——
/// 切换形态时它逐字段不变（FR-023 的基础）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorResolution {
    /// 章节序号（与 001 的 `ReadProgress.chapter_index` 同口径）
    pub chapter_index: usize,
    /// 章内字符偏移（**字符下标**，FR-003）
    pub char_offset: usize,
    /// 该字符在当前阅读形态下的定位
    pub target: AnchorTarget,
}

impl AnchorResolution {
    /// 锚点是否落在可见范围内（SC-004 的可见性判据）。
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.target.visible()
    }

    /// 所在页的序号（滚动形态或不可见时为 `None`）。
    #[must_use]
    pub fn page_index(&self) -> Option<usize> {
        self.target.page_index()
    }

    /// 页内/视口内偏移。
    #[must_use]
    pub fn offset_in_page(&self) -> Option<usize> {
        self.target.offset_in_target()
    }
}

/// 锚点在某一阅读形态下的定位（FR-021 的"同构"落点）。
///
/// ## 两变体都以**字符区间**为定位依据
///
/// 翻页侧用 `page_index` + 页区间（由 `Page::char_range` 给出），
/// 滚动侧用 `viewport_index` + `viewport_char_range`。两者都是"一个字符
/// 区间 + 区间内偏移"的形状——这正是 FR-021 说的"视口与页同构"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorTarget {
    /// 翻页模式：落在第 `page_index` 页。
    Paged {
        /// 页序号（0 起，含留白页）
        page_index: usize,
        /// 章内页码（与 `Page::page_in_chapter` 一致，供页脚 FR-042）
        page_in_chapter: u32,
        /// 页内字符偏移
        offset_in_page: usize,
        /// 该页的左右归属（单页模式为 `None`）
        side: Option<SpreadSide>,
        /// 是否可见（落在页的字符区间内）
        visible: bool,
    },
    /// 滚动模式：落在第 `viewport_index` 个视口。
    ///
    /// `[WIP]` 真实视口区间由 Phase 5（T025/T026）提供；本变体自 T022 起
    /// 就位，使几何参数矩阵能跨形态参数化。
    Scroll {
        /// 视口序号
        viewport_index: usize,
        /// 视口内字符偏移
        offset_in_viewport: usize,
        /// 该视口覆盖的**字符区间**（FR-021：与 Page 区间同构）
        viewport_char_range: std::ops::Range<usize>,
        /// 是否可见（落在视口区间内）
        visible: bool,
    },
}

impl AnchorTarget {
    /// 定位是否可见。
    #[must_use]
    pub fn visible(&self) -> bool {
        match self {
            Self::Paged { visible, .. } | Self::Scroll { visible, .. } => *visible,
        }
    }

    /// 页序号（滚动形态返回 `None`）。
    #[must_use]
    pub fn page_index(&self) -> Option<usize> {
        match self {
            Self::Paged { page_index, .. } => Some(*page_index),
            Self::Scroll { .. } => None,
        }
    }

    /// 视口序号（翻页形态返回 `None`）。
    #[must_use]
    pub fn viewport_index(&self) -> Option<usize> {
        match self {
            Self::Paged { .. } => None,
            Self::Scroll { viewport_index, .. } => Some(*viewport_index),
        }
    }

    /// 区间内偏移（两形态同义）。
    #[must_use]
    pub fn offset_in_target(&self) -> Option<usize> {
        match self {
            Self::Paged { offset_in_page, .. } => Some(*offset_in_page),
            Self::Scroll {
                offset_in_viewport, ..
            } => Some(*offset_in_viewport),
        }
    }

    /// 左右归属（单页与滚动形态返回 `None`）。
    #[must_use]
    pub fn side(&self) -> Option<SpreadSide> {
        match self {
            Self::Paged { side, .. } => *side,
            Self::Scroll { .. } => None,
        }
    }

    /// 定位所依据的**字符区间**（FR-021 的同构表达）。
    #[must_use]
    pub fn char_range(&self) -> Option<std::ops::Range<usize>> {
        match self {
            Self::Paged { .. } => None, // 页区间在 `Page` 上，此处不重复持有
            Self::Scroll {
                viewport_char_range,
                ..
            } => Some(viewport_char_range.clone()),
        }
    }
}

/// 一次分页的完整结果。
#[derive(Debug, Clone, PartialEq)]
pub struct PaginationResult {
    /// 页序列（含留白页）
    pub pages: Vec<Page>,
    /// 几何解析结果（栏数/版心）
    pub spread: SpreadResolution,
    /// 全部禁则例外（聚合自各段，供 SC-002 计数）
    pub rule_exceptions: Vec<crate::layout::linebox::RuleExceptionRecord>,
    /// 段落切分的诊断（FR-002 的"可读诊断记录"）
    pub split_diagnostic: paragraph::Diagnostic,
    /// 超长西文词退化的次数（FR-009 的诊断口径）
    pub degraded_words: usize,
    /// 章内字符总数（锚点钳制的上界，也是"读完"位置）
    pub char_len: usize,
}

impl PaginationResult {
    /// 解析一个**章内字符锚点**（FR-017 / 001 §SC-008 / FR-040）。
    ///
    /// ## 契约
    ///
    /// - 锚点值**原样返回**（`char_offset` 不被重排改写）——它是 001 的
    ///   `ReadProgress` 字段，本层只是消费方
    /// - 越界锚点**钳制**到 `[0, char_len]`（spec §Edge Cases："必须夹取到
    ///   有效范围……不得 panic"）
    /// - 返回 `Option`：空章且锚点无法定位时给 `None`（本实现里空章也会
    ///   产出一页，故实际上恒为 `Some`；保留 `Option` 是为了不把"恒可定位"
    ///   写死进契约——滚动形态的某些中间态可能需要它）
    ///
    /// ## 为什么按"页区间包含"查找而不是按页序号推算
    ///
    /// 页区间的构成含"页 i 覆盖到下一页首个字符为止"的延伸（段间换行符的
    /// 归属，见模块文档）。故"锚点在字符位置 x"与"锚点在第 k 页"之间没有
    /// 简单除法关系——必须按区间实际查找。这也让留白页（空区间）自然被跳过：
    /// 空区间不包含任何字符。
    ///
    /// ## 一个字符属于两页时的取舍（区间相邻处的边界）
    ///
    /// 页区间是首尾相接的（`[a, b)` 与 `[b, c)`），故除端点外不存在归属歧义。
    /// 端点 `b` 属于后一页（半开区间语义）。若前一页把 `b` 也算进去（闭区间），
    /// 会出现"一个字符判在两页"的情形——本实现按半开区间取**后一页**，
    /// 与 Rust 的 `Range::contains` 语义一致。
    #[must_use]
    pub fn resolve_anchor(&self, char_offset: usize) -> Option<AnchorResolution> {
        // 钳制：越界值夹到 [0, char_len]。char_len 本身是合法位置（读完了）。
        let clamped = char_offset.min(self.char_len);

        // 末位锚点的特殊处置：`char_len` 不属于任何页区间（页区间上界最大
        // 到 char_len - 1 的所属页），故取最后一个**内容页**。
        let (page_index, page) = if clamped == self.char_len {
            self.pages
                .iter()
                .enumerate()
                .rev()
                .find(|(_, page)| !page.is_padding)
                .or_else(|| self.pages.iter().enumerate().next())?
        } else {
            self.pages
                .iter()
                .enumerate()
                .find(|(_, page)| page.char_range.contains(&clamped))
                .or_else(|| {
                    // 兜底：字符落在所有页区间之外（理论上不该发生——页区间
                    // 覆盖 [0, char_len)）。取最后一个内容页并把偏移钳到页长。
                    self.pages
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, page)| !page.is_padding)
                })?
        };

        let offset_in_page = clamped.saturating_sub(page.char_range.start);
        let visible = page.char_range.contains(&clamped);

        Some(AnchorResolution {
            chapter_index: page.chapter_index,
            char_offset: clamped,
            target: AnchorTarget::Paged {
                page_index,
                page_in_chapter: page.page_in_chapter,
                offset_in_page,
                side: page.side,
                visible,
            },
        })
    }
}

/// 分页器（持有排版引擎——它是昂贵的字体库句柄，T013 的设计）。
///
/// ## 为什么是长生命周期对象
///
/// `LayoutEngine` 持 `FontSystem`（加载系统字体库，实测耗时可观）。
/// 分页预计算（T024）要对多章反复排版，每次新建引擎会成为瓶颈。
/// 故分页器跨多次调用复用同一个引擎。
#[derive(Default)]
pub struct Paginator {
    engine: LayoutEngine,
}

/// 一章的**排版结果**（分页与滚动两种形态的共同输入）。
///
/// ## 为什么抽这个类型（FR-020/FR-021 的落点）
///
/// FR-020 要求两种阅读形态"在同一组几何参数下可切换"，FR-021 要求"不引入
/// 第二套坐标"。两者合起来意味着：**两种形态必须消费同一份行盒序列**——
/// 若各自排版，断行结果可能不同（版心、可用高都会参与），那时"锚点不变"
/// 就是巧合而非保证。
///
/// 故把"正文 → 章内行盒序列"这一步单独成函数，分页与滚动都从它出发。
/// 分页再加"按可用高切页"，滚动再加"按可用高切视口"。
#[derive(Debug, Clone, PartialEq)]
pub struct ChapterLayout {
    /// 章内行盒序列（按阅读顺序，`chapter_char_range` 是**章内**区间）
    pub lines: Vec<PageLine>,
    /// 章内字符总数（含段间分隔符）
    pub char_len: usize,
    /// 版心宽度（px）——分页与滚动共用的几何结果
    pub measure_px: f32,
    /// 实际生效的栏数（含手动覆盖，FR-015）
    pub columns: Columns,
    /// 手动覆盖是否仍在生效
    pub override_active: bool,
    /// 全部禁则例外（供 SC-002 计数）
    pub rule_exceptions: Vec<crate::layout::linebox::RuleExceptionRecord>,
    /// 段落切分的诊断（FR-002 的"可读诊断记录"）
    pub split_diagnostic: paragraph::Diagnostic,
    /// 超长西文词退化的次数（FR-009 的诊断口径）
    pub degraded_words: usize,
}

/// 把一章正文排版为章内行盒序列（**两种阅读形态的共同输入**）。
///
/// ## 这是唯一的排版入口
///
/// 分页（[`Paginator::paginate`]）与滚动（`crate::scroll::flow`）都必须经此
/// 函数取得行盒。这样"两种形态共享同一排版结果"是**结构性事实**而非约定
/// ——没有第二条排版路径可供绕过（有测试断言两者行盒逐字段相同）。
///
/// ## 不做的事
///
/// - 不切页、不切视口（那是调用方各自的关切）
/// - 不调 `ContentProvider`（FR-004：正文由调用方一次交给本层）
/// - 不读 Theme / 呈现参数（FR-019/SC-005：分页只依赖几何）
pub fn layout_chapter(
    paginator: &mut Paginator,
    content: &str,
    geometry: &GeometryConfig,
    viewport: &Viewport,
) -> Result<ChapterLayout, crate::error::ReaderError> {
    geometry
        .validate()
        .map_err(|field| crate::error::ReaderError::invalid_geometry(field, "invalid value"))?;

    let override_resolution = resolve_override(viewport, geometry);
    let resolution = override_resolution.columns;
    let measure_px = column_measure_px(resolution, viewport, geometry);

    let split = paragraph::split(content);
    let layout_geometry = LayoutGeometry {
        measure_px,
        font_size_px: f32::from(geometry.font_size_px),
        line_height: geometry.line_height,
        alignment: crate::rules::justify::Alignment::Justified,
        // FR-018：段缩进模式是**几何参数**（FR-019/SC-005 把"段缩进模式"
        // 列在页数依赖的几何子集里），故必须传进排版——留默认值会让
        // 配置里的模式永远不生效（"schema 有、施加无"的半成品形态）。
        paragraph_mode: geometry.paragraph_mode,
    };

    // 逐段排版，把行盒换算成章内区间。
    //
    // 段落的章内起点（`para.char_start`）是换算的关键：行盒的段内区间
    // 加上它即得章内区间。
    let mut lines: Vec<PageLine> = Vec::new();
    let mut rule_exceptions = Vec::new();
    let mut degraded_words = 0usize;

    for para in &split.paragraphs {
        let model = TextModel::new(para.text.clone());
        let result = paginator
            .engine
            .layout_paragraph(&model, para.index, &layout_geometry);

        degraded_words += result.degraded_words;
        rule_exceptions.extend(result.exceptions.iter().cloned());

        for line in result.lines {
            let offset = para.char_start;
            let chapter_char_range =
                (line.char_range.start + offset)..(line.char_range.end + offset);
            lines.push(PageLine {
                line,
                chapter_char_range,
            });
        }
    }

    Ok(ChapterLayout {
        lines,
        char_len: content.chars().count(),
        measure_px,
        columns: resolution,
        override_active: override_resolution.override_active,
        rule_exceptions,
        split_diagnostic: split.diagnostic,
        degraded_words,
    })
}

impl Paginator {
    /// 新建分页器。
    ///
    /// ## 为什么保持无参不失败
    ///
    /// 引擎的字体栈装载失败是**可恢复**的错误，但把它前移到构造函数会让
    /// 全部 117 个调用点都去处理一个"正常运行时不会发生"的分支（内嵌字节
    /// 是编译期 `include_bytes!` 嵌进来的），而 `paginate` 本来就返回
    /// `Result`——错误面已经在正确的位置。
    ///
    /// ## 失败时为什么降级而不是 panic
    ///
    /// 内嵌字节不可解析意味着构建产物被破坏。此时走
    /// [`LayoutEngine::with_system_fonts_only`] 让排版**仍然可运行**（度量
    /// 不再由内嵌字体决定），而不是 panic 终止进程——`panic = "abort"` 下
    /// panic 即进程退出（AGENTS.md 安全红线）。调用方若在意 FR-050 的
    /// 一致性保证，应当用 [`LayoutEngine::new`] 的 `Result` 自行判定；
    /// 本类型是"排版必须给出结果"的那条路径。
    #[must_use]
    pub fn new() -> Self {
        let engine = LayoutEngine::new().unwrap_or_else(|_| LayoutEngine::with_system_fonts_only());
        Self { engine }
    }

    /// 把一章正文分页（FR-012/FR-014）。
    ///
    /// ## 输入
    ///
    /// - `content`：章节正文（由调用方从 [`tome_core::ContentProvider`] 取来
    ///   ——本层不感知缓存与抓取，FR-004）
    /// - `geometry`：几何参数（**只含几何子集**，呈现参数在类型上取不到，
    ///   故主题切换不会触发重排，FR-019/SC-005）
    /// - `viewport`：阅读区**可用**宽高（FR-014 的口径）
    ///
    /// ## 返回
    ///
    /// 页序列 + 几何解析结果 + 诊断聚合。空正文产出一页（spec §Edge Cases：
    /// 不得产出零页导致翻页越界）。
    ///
    /// ## 本方法不做的事
    ///
    /// - 不调 `ContentProvider`（正文由调用方一次交给它）
    /// - 不读 Theme / 呈现参数
    /// - 不持久化（裁决 OQ#18→A：分页结果不落盘）
    pub fn paginate(
        &mut self,
        content: &str,
        geometry: &GeometryConfig,
        viewport: &Viewport,
    ) -> Result<PaginationResult, crate::error::ReaderError> {
        // --- 1. 排版（与滚动形态共用同一入口，FR-020/021）---
        let chapter = layout_chapter(self, content, geometry, viewport)?;

        let line_height_px = geometry.line_height_px();
        let available_height = viewport.available_height_px;
        let resolution = chapter.columns;

        // --- 2. 贪心装页 ---
        let mut pages = pack_pages(&chapter.lines, line_height_px, available_height);

        // --- 3. 内容页区间的收尾：让每页认领到"下一页首个字符"为止 ---
        //
        // 段间换行符的归属在此落定（见模块文档）。做法是：把区间终点重置为
        // **所有行盒章内区间的并集之外**，再统一后延。
        fill_page_ranges(&mut pages, content);

        // --- 4. 左右归属（双页模式） ---
        apply_sides(&mut pages, resolution);

        // 空章：一页都没有 → 补一页空页（spec §Edge Cases：不得零页）
        if pages.is_empty() {
            pages.push(Page {
                chapter_index: 0,
                page_in_chapter: 0,
                char_range: 0..0,
                lines: Vec::new(),
                side: None,
                is_padding: false,
            });
            apply_sides(&mut pages, resolution);
        }

        Ok(PaginationResult {
            pages,
            spread: SpreadResolution {
                columns: resolution,
                measure_px: chapter.measure_px,
                override_active: chapter.override_active,
            },
            rule_exceptions: chapter.rule_exceptions,
            split_diagnostic: chapter.split_diagnostic,
            degraded_words: chapter.degraded_words,
            char_len: chapter.char_len,
        })
    }
}

/// 按累计行高把行盒贪心装进页。
///
/// ## 一页的容量判据
///
/// 累计行高 + 下一行行高 ≤ 可用高 → 该行放得进本页。
///
/// ## 单行超过整页高的情形
///
/// 该行**独占一页**（`lines.is_empty()` 时无条件收下）。这避免了两个坏结果：
///
/// - 死循环：行永远放不进任何页，装页循环空转
/// - 丢行：该行被跳过，SC-006 的逐字符比对失败
///
/// 溢出留待渲染层处置（那属 S2-4 的视口裁剪），本层只保证没有任何行丢失。
fn pack_pages(chapter_lines: &[PageLine], line_height_px: f32, available_height: f32) -> Vec<Page> {
    let mut pages: Vec<Page> = Vec::new();

    for page_line in chapter_lines {
        // 每行的实际高度（行盒自带；退化行可能是别的值）
        let height = if page_line.line.height_px > 0.0 {
            page_line.line.height_px
        } else {
            line_height_px
        };

        let should_start_new_page = match pages.last() {
            None => true,
            Some(page) => {
                if page.lines.is_empty() {
                    // 上一页还空着（如留白页）：直接放进去
                    false
                } else {
                    let used: f32 = page
                        .lines
                        .iter()
                        .map(|existing| existing.line.height_px)
                        .sum();
                    used + height > available_height + 1e-3
                }
            }
        };

        if should_start_new_page {
            pages.push(Page {
                chapter_index: 0,
                page_in_chapter: pages.len() as u32,
                char_range: 0..0, // 在 fill_page_ranges 里落定
                lines: Vec::new(),
                side: None,
                is_padding: false,
            });
        }

        if let Some(page) = pages.last_mut() {
            page.lines.push(page_line.clone());
        }
    }

    pages
}

/// 落定每页的字符区间。
///
/// ## 口径：页 i 覆盖到"下一页首个字符"为止
///
/// 这解决了段间换行符的归属（见模块文档）。实现分两步：
///
/// 1. 每页的区间 = 其行盒章内区间的**并集**（首行起点 .. 末行终点）
/// 2. 从后往前：页 i 的终点延伸到页 i+1 的起点
///
/// ## 为什么第 2 步从后往前
///
/// 若从前往后延伸，页 i 的终点会立刻变成页 i+1 的起点，而页 i+1 自己的
/// 区间还没算——于是它的起点取自"自己的行盒"，第 2 步再延伸时又变了。
/// 从后往前则每页的起点已定，延伸只改终点，不存在这种相互依赖。
///
/// ## 末页的终点
///
/// 延伸到**正文末尾**（字符数）。这样"被段落切分丢弃的尾部空白段"
/// （如正文末尾的连续换行）也有归属——它们通常产出空段而被丢弃，
/// 但字符仍在原文里。
///
/// ## 不需要第三步（曾经有过，已删）
///
/// 初版还有一步"修正首尾"（空页取点区间、起点钳到前页终点）。它**不可达**：
/// [`pack_pages`] 只在"即将放入一行"时才新建页，故本函数见到的每一页都至少
/// 有一个行盒——第 1 步必然给出非空区间，第 2 步的起点钳制也已覆盖
/// "起点越过终点"的情形。空章的那一页在**本函数之后**才补（见调用处），
/// 故不在此列。删掉后全套测试（332 项）逐项不变。
fn fill_page_ranges(pages: &mut [Page], content: &str) {
    let char_len = content.chars().count();

    // 第 1 步：行盒并集
    for page in pages.iter_mut() {
        page.char_range = match (page.lines.first(), page.lines.last()) {
            (Some(first), Some(last)) => {
                first.chapter_char_range.start..last.chapter_char_range.end
            }
            _ => 0..0,
        };
    }

    // 第 2 步：从后往前延伸终点到下一页起点
    let count = pages.len();
    for index in (0..count).rev() {
        let next_start = pages.get(index + 1).map(|next| next.char_range.start);
        let end = match next_start {
            Some(start) => start,
            None => char_len,
        };
        // 起点不得越过终点（页区间是半开的，相邻页共享边界点）
        if pages[index].char_range.start > end {
            pages[index].char_range.start = end;
        }
        pages[index].char_range.end = end;
    }
}

/// 给双页模式下的页标上左右归属（FR-016）。
fn apply_sides(pages: &mut [Page], columns: Columns) {
    if !columns.is_double() {
        for page in pages.iter_mut() {
            page.side = None;
        }
        return;
    }

    for (index, page) in pages.iter_mut().enumerate() {
        page.side = Some(SpreadSide::for_page_index(index as u32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paginate::config::ReadingConfig;

    fn geometry() -> GeometryConfig {
        ReadingConfig::default_ma().geometry
    }

    #[test]
    fn empty_content_yields_single_page() {
        let mut paginator = Paginator::new();
        let result = paginator
            .paginate("", &geometry(), &Viewport::new(684.0, 800.0))
            .expect("应成功");
        assert_eq!(result.pages.len(), 1);
        assert!(result.pages[0].is_empty());
    }

    /// 单双页判定结果被如实回传到 `spread`。
    #[test]
    fn spread_resolution_is_reported() {
        let mut paginator = Paginator::new();
        let narrow = paginator
            .paginate("内容", &geometry(), &Viewport::new(684.0, 800.0))
            .expect("应成功");
        assert_eq!(narrow.spread.columns, Columns::Single);

        let wide = paginator
            .paginate("内容", &geometry(), &Viewport::new(1432.0, 800.0))
            .expect("应成功");
        assert_eq!(wide.spread.columns, Columns::Double);
        assert!((wide.spread.measure_px - 684.0).abs() < 1e-3);
    }

    /// 非法几何参数报结构化错误（`InvalidGeometry`，不是 panic）。
    #[test]
    fn invalid_geometry_reports_structured_error() {
        let mut paginator = Paginator::new();
        let mut bad = geometry();
        bad.font_size_px = 0;
        let error = paginator
            .paginate("内容", &bad, &Viewport::new(684.0, 800.0))
            .expect_err("字号 0 应被拒");
        assert_eq!(error.capability_id(), None);
        assert_eq!(error.message_id(), "error.reader.invalid_geometry");
    }

    /// 章内区间换算正确（多段时不得错位）。
    #[test]
    fn chapter_ranges_account_for_paragraph_offsets() {
        let mut paginator = Paginator::new();
        let content = "第一段。\n第二段。\n第三段。";
        let result = paginator
            .paginate(content, &geometry(), &Viewport::new(684.0, 800.0))
            .expect("应成功");

        let lines: Vec<&PageLine> = result
            .pages
            .iter()
            .flat_map(|page| page.lines.iter())
            .collect();
        assert_eq!(lines.len(), 3, "三段各一行");

        // 三段的章内起点：0 / 5 / 10
        // （「第一段。」是 4 个字符：第、一、段、。 —— 加上段间换行符即 5）
        let starts: Vec<usize> = lines
            .iter()
            .map(|line| line.chapter_char_range.start)
            .collect();
        assert_eq!(starts, vec![0, 5, 10], "章内起点须含段间换行符的位移");
    }

    /// 留白页默认为否（T021 才引入真正的留白页）。
    #[test]
    fn content_pages_are_not_padding() {
        let mut paginator = Paginator::new();
        let result = paginator
            .paginate("一些正文", &geometry(), &Viewport::new(684.0, 800.0))
            .expect("应成功");
        for page in &result.pages {
            assert!(!page.is_padding);
        }
    }
}
