//! `[DONE]` 命中测试与文字选择（T043 / FR-040、FR-038）
//!
//! # 本模块做什么
//!
//! 给定屏幕坐标得到字符位置（命中测试），以及它的反向（字符 → 坐标），
//! 并把两次命中组成的拖选表达为**字符区间**。
//!
//! ## 与 `scroll::map::PositionMap` 的关系（**本模块不重复实现映射**）
//!
//! `PositionMap` 是 FR-040 的**载体**：它已经持有"行索引 + 行内字形偏移"的
//! 表示，并实现了 `char_to_offset` / `offset_to_char` /
//! `point_in_line_to_char` / `char_to_point_in_line` 四个原语（T025 的产物）。
//! 本模块在这些原语之上做两件事：
//!
//! 1. **把二维坐标分解为"行 + 行内 x"**——`PositionMap` 的垂直轴给到行，
//!    水平轴给到字符，本模块负责先按 y 找行、再用该行调水平原语。
//! 2. **把选择的两端规范化成字符区间**——拖选方向无关、跨行覆盖中间内容。
//!
//! 不重写映射的第二个理由：`PositionMap` 的两形态（`for_page` / `new`）
//! 已经统一了口径，本模块的两种入口只是选择用哪个构造函数。
//!
//! ## FR-038 的"口径一致"
//!
//! 选择结果的两个端点都是**章内字符下标**，与 `ContentProvider::resolve_anchor`
//! 的输入口径**同一种量**——可以直接作为 `ReadProgress.char_offset`。
//! 本模块不产出任何"渲染坐标形式的选择"，那正是 FR-028 禁止的存储形态。

use crate::paginate::page::{Page, PageLine};
use crate::scroll::flow::ScrollFlow;
use crate::scroll::map::PositionMap;

/// 一次命中（或一个拖选端点）在屏幕上的位置。
///
/// ## 为什么带 `line_index`
///
/// 二维坐标本身足以定位（y 决定行、x 决定行内位置），但**拖选跨行**时需要
/// 知道两端各在哪一行才能正确取中间内容。调用方在拿到点击事件时通常也
/// 已经知道光标所在行（外壳的列表控件会给出），带上它可以让跨行选择的
/// 语义明确而不必从 y 反推。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitPoint {
    /// 行内 x（px，相对行左边缘）
    pub x_px: f32,
    /// 垂直偏移（px，相对坐标系原点——页首或流首）
    pub y_px: f32,
    /// 该点所在的**可见行序号**（在本页/本流的行盒序列里，0 起）
    pub line_index: usize,
}

/// 一次命中测试的结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitResult {
    /// 命中的**章内字符下标**（FR-028 的口径：Unicode 标量值下标）
    pub char_index: usize,
    /// 命中的行在本页/本流里的序号
    pub line_index: usize,
}

/// 一次文字选择的结果（FR-038：**字符区间**，不是像素范围）。
///
/// ## 为什么是半开区间
///
/// 与 001 的 `Highlight { char_offset, length }` 同一套约定：`start` 含、
/// `end` 不含。半开区间让"选择从 A 到 B"与"长度为 B - A"同时成立，且
/// 相邻区间相接时不会重叠。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// 选中的章内字符区间（半开）
    pub char_range: std::ops::Range<usize>,
}

/// 在**一页**上做命中测试（FR-040 翻页形态）。
///
/// ## 找不到行时返回 `None`
///
/// y 落在页内但不在任何行上（例如页底部的留白）时返回 `None`——那表示
/// "这一点不对应任何文字"。静默归给某一行会让用户点空白处也产生选择。
#[must_use]
pub fn hit_test_page(page: &Page, x_px: f32, y_px: f32) -> Option<HitResult> {
    let map = PositionMap::for_page(page);
    hit_test_with_map(&map, &page.lines, x_px, y_px)
}

/// 在**滚动流**上做命中测试（FR-040 滚动形态）。
#[must_use]
pub fn hit_test_scroll(flow: &ScrollFlow, x_px: f32, y_px: f32) -> Option<HitResult> {
    let map = PositionMap::new(flow);
    hit_test_with_map(&map, flow.lines(), x_px, y_px)
}

/// 命中测试的公共实现（两形态共用，保证口径一致）。
///
/// ## 坐标分解的两步
///
/// 1. **垂直**：`offset_to_char(y)` 给出该偏移所在行的首字符，再由它反查
///    行序号。复用垂直原语（而不是本模块自己扫 `line_tops`）是"口径一致"
///    的结构保证——两处各写一遍必然分叉。
/// 2. **水平**：用上一步的行调用 `point_in_line_to_char(x)`。
///
/// ## 为什么第 1 步要"字符 → 行"绕一下
///
/// `PositionMap` 不公开行序号的直接查询（它的表示是"行索引 + 顶部偏移"，
/// 行序号是内部概念）。绕经由字符是**唯一的公开路径**，且它顺带保证了
/// "命中结果一定是某个行的首字符或其行内后继"这一不变量。
fn hit_test_with_map(
    map: &PositionMap<'_>,
    lines: &[PageLine],
    x_px: f32,
    y_px: f32,
) -> Option<HitResult> {
    if lines.is_empty() {
        return None;
    }
    // 垂直：先找该 y 属于哪一行
    let line_start_char = map.offset_to_char(y_px)?;
    let line_index = lines.iter().position(|line| {
        line.chapter_char_range.start == line_start_char
            || line.chapter_char_range.contains(&line_start_char)
    })?;
    let line = lines.get(line_index)?;

    // 水平：在该行内定位字符
    let char_index = map.point_in_line_to_char(line, x_px)?;
    Some(HitResult {
        char_index,
        line_index,
    })
}

/// 由两个拖选端点构成**字符区间选择**（FR-038）。
///
/// ## 规范化规则
///
/// | 情形 | 结果 |
/// |---|---|
/// | 同点（单击） | 零长区间（光标位置），**不是** `None` |
/// | 同向跨行 | 从起点字符到终点字符的**后继**（覆盖终点字符） |
/// | 反向跨行 | 与同向相同（方向无关） |
///
/// ## 为什么终点取"后继"而不是终点本身
///
/// 选择的语义是"从按下到松开之间**被扫过**的字符"。命中的是终点字符本身，
/// 而它是被扫过的——故区间上界应为其后继（半开区间的性质）。若取终点本身，
/// 用户永远选不到最后一个字。
///
/// ## `None` 只在端点无法归行时返回
///
/// 两个端点都必须能落在某一行上（`line_index` 越界即 `None`）。这与
/// "单击产生零长选择"不同：后者是合法操作，前者是调用方给了坏数据。
#[must_use]
pub fn select_range(
    start: HitPoint,
    end: HitPoint,
    map: &PositionMap<'_>,
    lines: &[PageLine],
) -> Option<Selection> {
    let start_line = lines.get(start.line_index)?;
    let end_line = lines.get(end.line_index)?;

    let start_char = map.point_in_line_to_char(start_line, start.x_px)?;
    let end_char = map.point_in_line_to_char(end_line, end.x_px)?;

    // 单击（两端落在同一行的同一字符）：零长区间，即光标位置。
    //
    // 必须在"含终点"规则之前判：若不判，`next_char_boundary` 会把区间撑成
    // 一个字符长，于是"单击"与"选中一个字"无法区分——而两者的下游行为不同
    // （单击是定位光标，选中是产生可复制内容）。
    if start.line_index == end.line_index && start_char == end_char {
        return Some(Selection {
            char_range: start_char..start_char,
        });
    }

    // 规范化方向：左端点取小者
    let (low_line, low_char, high_line, high_char) =
        if (start.line_index, start_char) <= (end.line_index, end_char) {
            (start_line, start_char, end_line, end_char)
        } else {
            (end_line, end_char, start_line, start_char)
        };

    // 跨行时，中间的行整段纳入（"扫过"的语义）
    let (range_start, range_end) = if low_line.chapter_char_range == high_line.chapter_char_range {
        // 同一行内：低端取字符本身，高端取其后继（含终点字符）
        let (from, to) = if low_char <= high_char {
            (low_char, high_char)
        } else {
            (high_char, low_char)
        };
        (from, next_char_boundary(to, low_line))
    } else {
        // 跨行：从低端行的字符起，到高端行的字符之后
        (
            low_char.min(low_line.chapter_char_range.end),
            next_char_boundary(high_char, high_line).min(high_line.chapter_char_range.end),
        )
    };

    // 保证 start <= end（跨行时若低端字符意外大于高端，钳一下）
    let (range_start, range_end) = if range_start <= range_end {
        (range_start, range_end)
    } else {
        (range_end, range_start)
    };

    Some(Selection {
        char_range: range_start..range_end,
    })
}

/// 字符的后继位置（用于让选择**包含**终点字符）。
///
/// 上限是该行的章内区间上界——这样跨行选择的终点不会越过行尾。
fn next_char_boundary(char_index: usize, line: &PageLine) -> usize {
    char_index
        .saturating_add(1)
        .min(line.chapter_char_range.end)
}
