//! \[DONE] 阅读痕迹渲染层（T042 / FR-037、FR-033）
//!
//! # 本模块做什么
//!
//! 把 S1 的**字符区间**痕迹（书签 / 高亮 / 批注）翻译成**可绘制片段**：
//! 给定一批痕迹区间与一批行盒（一页或几屏），产出若干 [`TraceSegment`]，
//! 每个段落携带它在章内的字符区间与应画的装饰类型。
//!
//! ## FR-037 的两条硬约束与本模块的对应
//!
//! 1. **覆盖恰好相等**："渲染出的像素范围覆盖的字符集合恰好等于该区间"。
//!    本模块的做法是**按字符区间夹具**：段落的 `char_range` 由"痕迹区间与
//!    行盒的章内区间求交"得到，故它既不会多（不越出痕迹）也不会少
//!    （每个被覆盖的字符都落在某个行内）。**不做像素级裁剪**——那正是
//!    FR-028「不以渲染坐标存储痕迹」的反面。
//! 2. **跨页/跨视口分段且拼接无缝隙无重叠**：本模块**逐行**产出段落，
//!    相邻行共用端点（前段的 `end` == 后段的 `start`），故拼接天然连续。
//!    用"逐行"而非"逐页"是因为：逐页分段时，一页内的多行之间仍需要接着
//!    画，那又是一次分段——逐行是更小的原子，让两种形态走同一条路径。
//!
//! ## 与渲染层的关系
//!
//! 本模块**不依赖** `render` feature（不给 GPU 类型）。产出的 [`TraceSegment`]
//! 是纯数据，由 `render::batch` 在绘制时消费成 `DecorationQuad`。故
//! `--no-default-features` 的构建里，痕迹的**语义计算**（哪段该画什么）
//! 仍然完整可用——只有"变成像素"那一步不在。
//!
//! ## 样式词表（FR-033）
//!
//! [`SEMANTIC_STYLE_VOCABULARY`] 是可枚举的语义词表；[`resolve_style`] 按
//! 三级解析序把 `HighlightRule.style` 字符串解析成确定的渲染效应：
//! ① 语义词表 → ② 颜色名 → ③ 默认下划线 + 诊断。**MUST NOT 静默不渲染**。

use std::collections::BTreeSet;
use std::ops::Range;

use tome_core::model::theme::{Color, Theme};

use crate::paginate::config::GeometryConfig;
use crate::paginate::page::{Page, PageLine};
use crate::paginate::spread::Viewport;
use crate::scroll::flow::ScrollFlow;

// ===========================================================================
// 痕迹区间
// ===========================================================================

/// 一条痕迹的**章内字符区间**（FR-028：位置一律字符下标）。
///
/// ## 为什么自带类型而不是直接用 `Range<usize>`
///
/// `Range<usize>` 无法表达"长度可能为 0 的合法痕迹"与"非法痕迹"的区别，
/// 也无法约束 `start <= end`。本类型在构造时规范化（`length` 为负则截为 0），
/// 让下游不必到处写防御性检查。它与 001 的 `Highlight { char_offset, length }`
/// 同口径——`length` 是**字符数**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceRange {
    /// 起始章内字符下标（含）
    pub start: usize,
    /// 覆盖的字符数
    pub length: usize,
}

impl TraceRange {
    /// 由起点与长度构造（`length` 任意，内部按"不越界"语义使用）。
    #[must_use]
    pub const fn new(start: usize, length: usize) -> Self {
        Self { start, length }
    }

    /// 由半开区间构造。
    #[must_use]
    pub const fn from_range(range: Range<usize>) -> Self {
        Self {
            start: range.start,
            length: range.end.saturating_sub(range.start),
        }
    }

    /// 终点（不含）。
    #[must_use]
    pub const fn end(&self) -> usize {
        self.start.saturating_add(self.length)
    }

    /// 章内半开区间。
    #[must_use]
    pub const fn as_range(&self) -> Range<usize> {
        self.start..self.end()
    }

    /// 是否为空（不覆盖任何字符）。
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// 与一个行盒的章内区间求交——**FR-037 精确性的实现**。
    ///
    /// 返回 `None` 表示不相交（该行不该被这条痕迹装饰）。
    #[must_use]
    pub fn intersect(&self, line_range: &Range<usize>) -> Option<Range<usize>> {
        let start = self.start.max(line_range.start);
        let end = self.end().min(line_range.end);
        if start < end {
            Some(start..end)
        } else {
            None
        }
    }
}

// ===========================================================================
// 样式词表与三级解析（FR-033）
// ===========================================================================

/// 渲染效应：一条痕迹最终**画成什么**。
///
/// ## 为什么是"效应"而不是"颜色"
///
/// FR-033 的语义词表里既有"背景色"这类纯颜色项，也有"加粗"这类纯字形项，
/// 还有"下划线 + 背景"这类组合。用效应枚举表达让一个样式可以同时带
/// 多个面（背景 + 下划线），而不必为每种组合开一个变体。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleEffect {
    /// 是否画背景底
    pub background: bool,
    /// 是否画下划线
    pub underline: bool,
    /// 是否画删除线
    pub strikethrough: bool,
    /// 是否加粗
    pub bold: bool,
}

impl StyleEffect {
    /// 只要下划线（FR-033 的默认回退效果）。
    pub const UNDERLINE_ONLY: Self = Self {
        background: false,
        underline: true,
        strikethrough: false,
        bold: false,
    };

    /// 只要背景底（颜色名的落点）。
    pub const BACKGROUND_ONLY: Self = Self {
        background: true,
        underline: false,
        strikethrough: false,
        bold: false,
    };
}

/// 语义词表的一个条目（FR-033 的"可枚举"要求）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticStyle {
    /// `HighlightRule.style` 里的稳定标识（**不是**面向用户的文案）
    pub token: &'static str,
    /// 该标识对应的渲染效应
    pub effect: StyleEffect,
}

/// **语义词表**（FR-033：MUST 可枚举）。
///
/// ## 词条怎么定的
///
/// 取自书源生态里实际出现的样式标识。`HighlightRule.style` 是 001 契约里
/// 的**自由字符串**（书源作者可写任意值），故本表的角色是"覆盖常见写法"，
/// 未覆盖的走第二 / 三级——而不是"白名单之外即错误"。
///
/// 标识一律小写且用连字符：解析时先做规范化（小写 + 下划线转连字符），
/// 故 `UnderLine` / `underline` / `under_line` 都命中同一条。
pub const SEMANTIC_STYLE_VOCABULARY: &[SemanticStyle] = &[
    SemanticStyle {
        token: "underline",
        effect: StyleEffect::UNDERLINE_ONLY,
    },
    SemanticStyle {
        token: "background",
        effect: StyleEffect::BACKGROUND_ONLY,
    },
    SemanticStyle {
        token: "highlight",
        effect: StyleEffect::BACKGROUND_ONLY,
    },
    SemanticStyle {
        token: "bold",
        effect: StyleEffect {
            background: false,
            underline: false,
            strikethrough: false,
            bold: true,
        },
    },
    SemanticStyle {
        token: "strikethrough",
        effect: StyleEffect {
            background: false,
            underline: false,
            strikethrough: true,
            bold: false,
        },
    },
    SemanticStyle {
        token: "underline-background",
        effect: StyleEffect {
            background: true,
            underline: true,
            strikethrough: false,
            bold: false,
        },
    },
    SemanticStyle {
        token: "bold-background",
        effect: StyleEffect {
            background: true,
            underline: false,
            strikethrough: false,
            bold: true,
        },
    },
];

/// 样式解析命中的**级别**（FR-033 的三级）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleLevel {
    /// ① 语义词表
    Semantic,
    /// ② 颜色名
    ColorName,
    /// ③ 默认回退（下划线 + 诊断）
    Fallback,
}

/// 一个样式标识的**解析结果**（FR-033）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleResolution {
    /// 命中的级别
    pub level: StyleLevel,
    /// 渲染效应
    pub effect: StyleEffect,
    /// 颜色名的解析结果（仅 [`StyleLevel::ColorName`] 时非 `None`）
    pub color: Option<Color>,
    /// 回退诊断（仅 [`StyleLevel::Fallback`] 时非 `None`，携带**原样式串**）
    pub diagnostic: Option<String>,
}

impl StyleResolution {
    /// 是否命中第一级（语义词表）。
    #[must_use]
    pub const fn is_semantic(&self) -> bool {
        matches!(self.level, StyleLevel::Semantic)
    }

    /// 是否命中第二级（颜色名）。
    #[must_use]
    pub const fn is_color_name(&self) -> bool {
        matches!(self.level, StyleLevel::ColorName)
    }

    /// 是否走第三级（回退）。
    #[must_use]
    pub const fn is_fallback(&self) -> bool {
        matches!(self.level, StyleLevel::Fallback)
    }

    /// 渲染效应。
    #[must_use]
    pub const fn effect(&self) -> StyleEffect {
        self.effect
    }

    /// 是否画背景。
    #[must_use]
    pub const fn has_background(&self) -> bool {
        self.effect.background
    }

    /// 是否画下划线。
    #[must_use]
    pub const fn has_underline(&self) -> bool {
        self.effect.underline
    }

    /// 诊断串（回退时非 `None`）。
    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }
}

/// 颜色名 → 色值的对照表（FR-033 的第二级）。
///
/// ## 取值原则
///
/// 这些是**书源作者写下的颜色名**，不是设计 token——故不能取 MASTER 的
/// 语义色（那是"主色/成功色"，与"作者说这个高亮是红的"是两回事）。
/// 但取值必须落在**暖色系**内（MASTER §8 禁蓝是全局约束，高亮底色同样
/// 适用）——故 `blue` 被映射到一个偏冷的**暖灰蓝**而非纯蓝，并在文档里
/// 说明这一处刻意的偏移。
const COLOR_NAMES: &[(&str, Color)] = &[
    ("red", Color::rgb(0xC0, 0x4A, 0x3C)),
    ("orange", Color::rgb(0xC9, 0x7B, 0x3A)),
    ("yellow", Color::rgb(0xC7, 0xA1, 0x3F)),
    ("green", Color::rgb(0x4F, 0x77, 0x55)),
    ("purple", Color::rgb(0x8A, 0x5A, 0x7A)),
    ("gray", Color::rgb(0x6E, 0x64, 0x58)),
    ("grey", Color::rgb(0x6E, 0x64, 0x58)),
    ("brown", Color::rgb(0x8B, 0x66, 0x3F)),
    ("pink", Color::rgb(0xB8, 0x6A, 0x7A)),
    // 刻意偏移：MASTER §8 禁蓝，故 `blue` 映射为"暖灰蓝"（蓝分量与红
    // 分量接近，不构成蓝色主导）。书源作者写 `blue` 时得到的是一个偏冷的
    // 中性色，而不是通用 admin 蓝——这是在"尊重作者意图"与"守住色系
    // 身份"之间的取舍，取后者（MASTER §8 是已拍板的硬约束）。
    ("blue", Color::rgb(0x5A, 0x6B, 0x7E)),
    ("cyan", Color::rgb(0x4E, 0x74, 0x74)),
];

/// 规范化样式标识：转小写、下划线转连字符、去首尾空白。
///
/// ## 为什么需要规范化
///
/// `HighlightRule.style` 是书源作者手写的自由字符串，实测里 `Underline` /
/// `under_line` / `UNDERLINE` 都出现过。逐字面匹配会让它们全部掉进回退分支，
/// 而回退产物（诊断噪声 + 默认下划线）对作者毫无用处——他要的就是下划线。
fn normalize(style: &str) -> String {
    style.trim().to_lowercase().replace('_', "-")
}

/// 按 FR-033 的**三级解析序**解析样式标识。
///
/// 顺序不可颠倒：先语义词表（`red` 若同时在词表里，应以词表为准），
/// 再颜色名，最后回退。
#[must_use]
pub fn resolve_style(style: &str) -> StyleResolution {
    let normalized = normalize(style);

    // ① 语义词表
    if let Some(entry) = SEMANTIC_STYLE_VOCABULARY
        .iter()
        .find(|entry| entry.token == normalized)
    {
        return StyleResolution {
            level: StyleLevel::Semantic,
            effect: entry.effect,
            color: None,
            diagnostic: None,
        };
    }

    // ② 颜色名 → 背景色
    if let Some((_, color)) = COLOR_NAMES.iter().find(|(name, _)| *name == normalized) {
        return StyleResolution {
            level: StyleLevel::ColorName,
            effect: StyleEffect::BACKGROUND_ONLY,
            color: Some(*color),
            diagnostic: None,
        };
    }

    // ③ 回退：默认下划线 + 诊断（MUST NOT 静默不渲染）
    //
    // 诊断携带**原样式串**（未规范化前的），因为用户看到的是书源里写的那个
    // 值——给他规范化后的形态会让他搜不到。
    StyleResolution {
        level: StyleLevel::Fallback,
        effect: StyleEffect::UNDERLINE_ONLY,
        color: None,
        diagnostic: Some(format!(
            "未知的痕迹样式 `{style}`：已回退为默认下划线。\
             可用语义样式：{}；或使用颜色名（如 red / yellow / green）。",
            SEMANTIC_STYLE_VOCABULARY
                .iter()
                .map(|entry| entry.token)
                .collect::<Vec<_>>()
                .join(" / ")
        )),
    }
}

// ===========================================================================
// 渲染片段
// ===========================================================================

/// 一段可绘制的痕迹（**一行内**的连续区间）。
///
/// ## 为什么以"行"为原子
///
/// FR-037 要求跨页 / 跨视口分段时"拼接无缝隙无重叠"。以行为原子让这条
/// 自动成立：相邻行的段落端点相接（前段 `end` == 后段 `start`），因为
/// 行盒的章内区间本身就是连续切分。若以"页"为原子，页内多行之间还要再分，
/// 那道额外的分段逻辑正是缝隙与重叠的来源。
#[derive(Debug, Clone, PartialEq)]
pub struct TraceSegment {
    /// 该段在**章内**的字符区间（半开）
    pub char_range: Range<usize>,
    /// 该段所属的行索引（在本页/本流的行盒序列里）
    pub line_index: usize,
    /// 应画的装饰类型（由样式解析给出）
    pub decorations: Vec<DecorationKind>,
    /// 背景色（`background` 为真时使用；取自样式解析或主题默认）
    pub background_color: Option<Color>,
    /// 线色（下划线 / 删除线使用）
    pub line_color: Color,
}

/// 装饰类型（与 `render::batch::DecorationKind` 同义，但在**不依赖 render
/// feature** 的层里独立定义——避免痕迹语义层因 feature 关闭而消失）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DecorationKind {
    /// 高亮底（画在文字之下）
    Background,
    /// 下划线
    Underline,
    /// 删除线
    Strikethrough,
}

/// 由样式效应与解析结果推导装饰类型清单。
fn decorations_for(resolution: &StyleResolution) -> Vec<DecorationKind> {
    let effect = resolution.effect();
    let mut kinds = Vec::new();
    if effect.background {
        kinds.push(DecorationKind::Background);
    }
    if effect.underline {
        kinds.push(DecorationKind::Underline);
    }
    if effect.strikethrough {
        kinds.push(DecorationKind::Strikethrough);
    }
    // 回退时效应即 UNDERLINE_ONLY，故 `kinds` 必非空——但为稳妥起见
    // （将来新增效应枚举时忘了填），此处补一次显式检查。
    if kinds.is_empty() {
        kinds.push(DecorationKind::Underline);
    }
    kinds
}

/// 从主题与样式解析结果取背景色。
fn background_color_for(theme: &Theme, resolution: &StyleResolution) -> Option<Color> {
    if !resolution.has_background() {
        return None;
    }
    // 颜色名解析出的色优先（作者明确的意图），否则用主题的高亮底
    Some(
        resolution
            .color
            .unwrap_or_else(|| theme.palette().accent_soft),
    )
}

/// 由主题取线色（下划线 / 删除线）。
fn line_color_for(theme: &Theme, resolution: &StyleResolution) -> Color {
    resolution
        .color
        .unwrap_or_else(|| crate::theme::ReadingPalette::for_theme(theme).highlight_underline)
}

// ===========================================================================
// 渲染入口（两种形态）
// ===========================================================================

/// **逐行**把痕迹区间切成段落——两条形态入口的公共实现。
///
/// ## 为什么输入是"行盒切片"而不是"页"或"流"
///
/// 页与流都能给出「章内连续、首尾相接」的行盒序列，这正是本函数需要的
/// 全部信息。让它只依赖行盒，两种形态就共用同一段切分逻辑——"翻页与滚动
/// 的分段结果一致"因此是结构事实而非两处碰巧相同。
fn segments_for_lines(
    traces: &[TraceRange],
    lines: &[(usize, Range<usize>)],
    theme: &Theme,
    resolution: &StyleResolution,
) -> Vec<TraceSegment> {
    let decorations = decorations_for(resolution);
    let background_color = background_color_for(theme, resolution);
    let line_color = line_color_for(theme, resolution);

    let mut segments = Vec::new();
    for trace in traces {
        if trace.is_empty() {
            // 空区间不产出段落：它不是"看不见的高亮"，而是"没有高亮"。
            continue;
        }
        for (line_index, line_range) in lines {
            let Some(intersection) = trace.intersect(line_range) else {
                continue;
            };
            segments.push(TraceSegment {
                char_range: intersection,
                line_index: *line_index,
                decorations: decorations.clone(),
                background_color,
                line_color,
            });
        }
    }

    // 段落按章内起点排序，让"拼接"有确定的先后（也是"无缝隙"可比的前提）
    segments.sort_by_key(|segment| segment.char_range.start);
    segments
}

/// 渲染痕迹到**一页或多页**（翻页形态；FR-037）。
///
/// ## 为什么接收多页而不是一页
///
/// FR-037 要求"区间跨越页边界时分段渲染"——调用方给一批页，本函数逐页
/// 求交，于是跨页区间自然被切成两段。若只收一页，跨页这条就无处表达。
#[must_use]
pub fn render_traces(
    traces: &[TraceRange],
    pages: &[Page],
    _geometry: &GeometryConfig,
    _viewport: &Viewport,
    theme: Theme,
    resolution: &StyleResolution,
) -> Vec<TraceSegment> {
    // 展开为「全局行序号 + 章内区间」——全局序号让跨页的两段不会因
    // "两页里都是行 0"而混淆。
    let mut lines: Vec<(usize, Range<usize>)> = Vec::new();
    let mut index = 0usize;
    for page in pages {
        for line in &page.lines {
            lines.push((index, line.chapter_char_range.clone()));
            index += 1;
        }
    }
    segments_for_lines(traces, &lines, &theme, resolution)
}

/// 渲染痕迹到**滚动流**（FR-037：跨视口分段）。
///
/// ## 为什么用 `flow.lines()` 而不是逐视口取
///
/// 滚动流的行序列覆盖整章（FR-026 的按需释放只清 `glyphs`，不清
/// `chapter_char_range`），故"整章的行盒"就是分段所需的全部信息。逐视口
/// 取行会让同一行被多个视口重复纳入（视口边界处的行），反而制造重叠。
#[must_use]
pub fn render_traces_scroll(
    traces: &[TraceRange],
    flow: &ScrollFlow,
    _geometry: &GeometryConfig,
    _viewport: &Viewport,
    theme: Theme,
    resolution: &StyleResolution,
) -> Vec<TraceSegment> {
    let lines: Vec<(usize, Range<usize>)> = flow
        .lines()
        .iter()
        .enumerate()
        .map(|(index, line)| (index, line.chapter_char_range.clone()))
        .collect();
    segments_for_lines(traces, &lines, &theme, resolution)
}

/// 渲染痕迹到**一组行盒**（调用方已自己取好行的场景）。
#[must_use]
pub fn render_traces_to_lines(
    traces: &[TraceRange],
    lines: &[PageLine],
    theme: Theme,
    resolution: &StyleResolution,
) -> Vec<TraceSegment> {
    let indexed: Vec<(usize, Range<usize>)> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| (index, line.chapter_char_range.clone()))
        .collect();
    segments_for_lines(traces, &indexed, &theme, resolution)
}

/// 一批痕迹渲染后**被覆盖的章内字符集合**（SC-013 的判据载体）。
///
/// 供上层做"装饰覆盖 == 区间"的断言与诊断，也是本模块自测的便利函数。
#[must_use]
pub fn covered_chars(segments: &[TraceSegment]) -> BTreeSet<usize> {
    let mut covered = BTreeSet::new();
    for segment in segments {
        for index in segment.char_range.clone() {
            covered.insert(index);
        }
    }
    covered
}
