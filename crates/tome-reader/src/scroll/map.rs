//! [DONE] 滚动偏移 ⇄ 字符位置双向映射（T026 / FR-022、FR-040）
//!
//! ## 两条轴的分辨率不同（本模块全部设计的出发点）
//!
//! | 轴 | 一个单位 | 能分辨到 | 往返阈值 |
//! |---|---|---|---|
//! | **垂直** | 行盒高（19px 字号下 35.15px） | **行** | 一字身（行盒高） |
//! | **水平**（行内） | 字形步进（CJK 全角 = 19px） | **字符** | 半个字形 |
//!
//! FR-040 说"往返误差不大于半个字形"——那是**水平**方向的约束，那里字形步进
//! 给出字符级分辨率。垂直方向**不可能**满足该字面阈值：同一行的所有字符共享
//! 同一个垂直位置，故任意垂直偏移最多只能确定到**行**。
//!
//! 要求垂直误差 ≤ 9.5px 而一行高 35.15px，等价于要求从"行号"恢复"行内第几列"
//! ——信息在映射中已丢失，**没有任何实现**能满足。本模块不假装能满足它，而是：
//!
//! 1. 垂直按**行分辨率**给出可断言的性质（误差 ≤ 一行高，且**幂等**）
//! 2. 水平单独提供字符级映射（`point_in_line_to_char` 一族），**满足半个字形**
//!
//! ## 幂等性为什么是垂直方向的关键性质
//!
//! "误差 ≤ 一行"允许每次往返漂一点。滚动是高频重复动作，累积漂移的表现是
//! "慢慢滚着滚着位置就不对了"。故 `offset → char → offset` 必须**规范化**：
//! 第一次往返把偏移吸附到行首（或行内某个确定位置），第二次就不动了。
//!
//! ## 与 001 锚点口径的关系（FR-022）
//!
//! 映射的**键**是章内字符下标（`char_offset`），与 `ReadProgress.char_offset`
//! 同口径。像素偏移只是**表达**——位置的主口径永远是字符区间（FR-021）。

use crate::paginate::page::{Page, PageLine};
use crate::scroll::flow::ScrollFlow;

/// 滚动流内的位置映射（data-model.md §10 的 `PositionMap`，两形态共用）。
///
/// ## 为什么不是"给每个字符预存一个 Rect"
///
/// 一行几十个字符共享同一 `y` 与同一 `height`，逐字符存会让内存随字符数线性
/// 增长（20 万字 → 20 万条记录）。而 SC-010 要求"单章排版结果 ≤ 章节字节 ×8"
/// ——逐字符 Rect 会显著吃掉那个预算。
///
/// 故本类型的表示是**行索引 + 行内字形偏移**：`O(行数)` 的存储，查询时按行
/// 二分。位置信息的来源是既有的 `PageLine`（T025 已持有），**不新增数据结构**。
#[derive(Debug, Clone)]
pub struct PositionMap<'a> {
    /// 行盒序列（按阅读顺序）——**借自**来源，不持有
    lines: &'a [PageLine],
    /// 每行的顶部偏移（px，相对来源原点）
    line_tops: Vec<f32>,
    /// 来源总高（px）
    total_height_px: f32,
    /// 章内字符总数
    char_len: usize,
}

impl<'a> PositionMap<'a> {
    /// 构造滚动形态的映射（原点 = 滚动流首）。
    #[must_use]
    pub fn new(flow: &'a ScrollFlow) -> Self {
        Self::from_lines(flow.lines(), flow.content_height_px(), flow.char_len())
    }

    /// 构造翻页形态的映射（原点 = **页首**，FR-040 要求两形态都可用）。
    ///
    /// ## 为什么页内映射的 `char_len` 取页的区间上界
    ///
    /// 页内映射的"章内字符"范围是本页的 `char_range`。取上界让越界钳制落在
    /// 本页末尾，而不是整章末尾——否则点页面下方会跳到全书末尾。
    #[must_use]
    pub fn for_page(page: &'a Page) -> Self {
        // 页内各行的像素位置从 0 重新起算（页首为原点）
        Self::from_lines(&page.lines, f32::MAX, page.char_range.end)
    }

    /// 由行盒序列构造（两条入口的公共实现）。
    fn from_lines(lines: &'a [PageLine], total_height_px: f32, char_len: usize) -> Self {
        let mut line_tops = Vec::with_capacity(lines.len());
        let mut accumulated = 0.0f32;
        for line in lines {
            line_tops.push(accumulated);
            accumulated += line.line.height_px.max(0.0);
        }
        // `for_page` 传 `f32::MAX` 表示"用算出来的高度"（页高不是页内容的和）
        let resolved_total = if total_height_px.is_finite() {
            total_height_px
        } else {
            accumulated
        };
        Self {
            lines,
            line_tops,
            total_height_px: resolved_total.max(0.0),
            char_len,
        }
    }

    /// 来源总高（px）。
    #[must_use]
    pub fn total_height_px(&self) -> f32 {
        self.total_height_px
    }

    /// 行数（供内存断言与调试）。
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    // -----------------------------------------------------------------------
    // 垂直：偏移 ⇄ 字符（行分辨率）
    // -----------------------------------------------------------------------

    /// 字符 → 垂直偏移（px）。
    ///
    /// ## 规范化到**行首**
    ///
    /// 同一行的字符共享一个垂直位置，故本函数返回**该行的顶部偏移**——这是
    /// 唯一自洽的选择：返回行内某个插值会让 `offset → char → offset` 不可能
    /// 幂等（插值依赖字符在行内的位置，而那个信息在反向映射里没有）。
    ///
    /// 越界字符钳制到 `[0, char_len]`；`char_len` 对应来源总高（"读完了"）。
    #[must_use]
    pub fn char_to_offset(&self, char_index: usize) -> Option<f32> {
        if self.lines.is_empty() {
            return Some(0.0);
        }
        let clamped = char_index.min(self.char_len);

        // 末尾（含越界）：返回总高
        if clamped >= self.char_len {
            return Some(self.total_height_px);
        }

        // 找到包含该字符的行；落在段间分隔符（不属于任何行）时取其后一行
        match self.lines.iter().position(|line| {
            line.chapter_char_range.start <= clamped && clamped < line.chapter_char_range.end
        }) {
            Some(index) => Some(self.line_tops[index]),
            None => {
                // 段间分隔符：归**前一行的末尾**（与分页的区间归属一致）
                let previous = self
                    .lines
                    .iter()
                    .rposition(|line| line.chapter_char_range.end <= clamped);
                Some(previous.map_or(0.0, |index| self.line_tops[index]))
            }
        }
    }

    /// 垂直偏移 → 字符（取该偏移所在行的**首字符**）。
    ///
    /// ## 为什么取行首而不是"行内最近的字符"
    ///
    /// 垂直偏移没有行内信息，无法知道"这一行的第几个字"。取行首是**确定**的
    /// 选择（任何实现都能复现），且让 `offset → char → offset` 幂等：行首字符
    /// 映射回行首偏移，闭环。
    ///
    /// 若取"行内某个估算位置"（如按 x 中值取中间的字符），反向映射回来的偏移
    /// 会随估算变化——那正是累积漂移的来源。
    ///
    /// 越界偏移钳制：负值 → 首字符；超出总高 → `char_len`。
    #[must_use]
    pub fn offset_to_char(&self, offset_px: f32) -> Option<usize> {
        if self.lines.is_empty() {
            return Some(0);
        }
        if !offset_px.is_finite() {
            return Some(0);
        }
        if offset_px < 0.0 {
            return Some(self.lines[0].chapter_char_range.start);
        }
        if offset_px >= self.total_height_px {
            return Some(self.char_len);
        }

        // 找到包含该偏移的行（二分：`line_tops` 已按序）
        //
        // `partition_point` 给出"第一条 top > offset 的行"的下标，其前一条即
        // 包含该偏移的行。
        let index = self
            .line_tops
            .partition_point(|top| *top <= offset_px)
            .saturating_sub(1);

        Some(
            self.lines
                .get(index)
                .map_or(self.char_len, |line| line.chapter_char_range.start),
        )
    }

    // -----------------------------------------------------------------------
    // 水平：行内 x ⇄ 字符（字符分辨率，FR-040 的"半个字形"在这条轴上）
    // -----------------------------------------------------------------------

    /// 行内 x（相对行左边缘）→ 字符（章内下标）。
    ///
    /// ## 判据：x 落在哪个字形的步进区间内
    ///
    /// 逐字形累加 `advance_px`，x 落在哪一段就返回该字形的首字符。x 超出行的
    /// 内容宽度时钳到**行末**（返回行区间上界）——那是用户点到了行尾空白，
    /// 光标应停在行末。
    #[must_use]
    pub fn point_in_line_to_char(&self, line: &PageLine, x_px: f32) -> Option<usize> {
        if !x_px.is_finite() {
            return Some(line.chapter_char_range.start);
        }
        if x_px <= 0.0 {
            return Some(line.chapter_char_range.start);
        }

        // ## 判据用字形的**实际位置**（`x_px`）而不是"累加步进"
        //
        // 两者在**无缩进**时等价，但段首缩进（FR-018）让首行从 `indent_px`
        // 开始——那时"从 0 累加步进"会整体少算一个缩进量，把行内所有点都
        // 映射到偏左一个缩进的字符上（实测偏差 49.88px ≈ 2 字身）。
        //
        // 用 `x_px`（布局给出的真实左缘，含缩进）则天然正确，且与
        // `char_to_point_in_line` 的取法（同样读 `x_px`）口径一致——
        // 两者不一致正是往返不闭合的来源。
        let paragraph_offset = line
            .chapter_char_range
            .start
            .saturating_sub(line.line.char_range.start);

        for glyph in &line.line.glyphs {
            let right_edge = glyph.x_px.max(0.0) + glyph.advance_px.max(0.0);
            if x_px < right_edge {
                // 字形区间是**段内**的，加段起始换算为章内
                return Some(glyph.char_range.start + paragraph_offset);
            }
        }

        // 超出内容宽度 → 行末
        Some(line.chapter_char_range.end)
    }

    /// 行内容的**左缘**（行内坐标，px）。
    ///
    /// ## 为什么命中测试需要它
    ///
    /// 段首缩进行的内容不从 0 开始，而 `[0, indent)` 那段是"缩进留白"。
    /// 落在留白里的点击应判给**本行首字符**（用户意图是"点在这行上"），
    /// 但往返误差的判据（FR-040 的半个字形）只在**内容区**内有意义——
    /// 留白宽度（2em）远大于半个字形，拿它当误差就是判据错位。
    #[must_use]
    pub fn line_content_origin_px(&self, line: &PageLine) -> f32 {
        line.line.indent_px.max(0.0)
    }

    /// 字符（章内下标）→ 行内 x（相对行左边缘）。
    ///
    /// ## 返回字形的**中心**而非左边缘（FR-040 的"半个字形"能达标的唯一取法）
    ///
    /// `point_in_line_to_char` 把 x 映射到"x 落在哪个字形"→ 该字形的首字符。
    /// 反向若取该字形的**左边缘**，则误差可达**一整个字身**：x=10 落在 10..20
    /// 的字形内（首个字形 advance=10 时 x=10 属于第二个字形，边界另说），
    /// 最近的左边缘是 10 → 误差 10px；而半个字形是 9.5px，**超标**。
    ///
    /// 取中心则误差上界是一半个字身：x 落在字形 `[L, L+A)` 内，中心是 `L+A/2`，
    /// 故 `|x - center| ≤ A/2`。那正好是 FR-040 的阈值。
    ///
    /// 这也是"半个字形"这个数字的**来源**：它是"用字形中心代表字形"时的
    /// 必然误差上界。取左边缘对应的是"一个字身"，不是半个。
    ///
    /// 字符不在本行时返回 `None`。
    #[must_use]
    pub fn char_to_point_in_line(&self, line: &PageLine, char_index: usize) -> Option<f32> {
        if !line.chapter_char_range.contains(&char_index) {
            return None;
        }
        let paragraph_offset = line
            .chapter_char_range
            .start
            .saturating_sub(line.line.char_range.start);
        let paragraph_index = char_index.checked_sub(paragraph_offset)?;

        for glyph in &line.line.glyphs {
            if glyph.char_range.start <= paragraph_index && paragraph_index < glyph.char_range.end {
                return Some(glyph.x_px + glyph.advance_px.max(0.0) / 2.0);
            }
        }

        // 字符在行区间内但没有对应字形（段间分隔符等）→ 行首
        Some(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::linebox::{GlyphBox, LineBox};

    /// 造一行：`len` 个字符，每字身 `advance` px，高度 `height` px。
    fn page_line(start: usize, len: usize, advance: f32, height: f32) -> PageLine {
        let glyphs: Vec<GlyphBox> = (0..len)
            .map(|index| GlyphBox {
                char_range: index..index + 1,
                byte_range: index * 3..index * 3 + 3,
                x_px: index as f32 * advance,
                advance_px: advance,
                squeeze_px: 0.0,
                font_id: cosmic_text::fontdb::ID::dummy(),
                glyph_id: 0,
                font_weight: cosmic_text::fontdb::Weight::NORMAL,
            })
            .collect();
        PageLine {
            line: LineBox {
                paragraph_index: 0,
                char_range: 0..len,
                glyphs,
                height_px: height,
                baseline_px: height * 0.8,
                squeeze_px: 0.0,
                justify_extra_px: 0.0,
                indent_px: 0.0,
                rule_exception: None,
            },
            chapter_char_range: start..(start + len),
        }
    }

    fn map_of(lines: &[PageLine], char_len: usize) -> PositionMap<'_> {
        let total: f32 = lines.iter().map(|line| line.line.height_px).sum();
        PositionMap::from_lines(lines, total, char_len)
    }

    #[test]
    fn char_maps_to_its_line_top() {
        let lines = vec![page_line(0, 5, 10.0, 20.0), page_line(5, 5, 10.0, 20.0)];
        let map = map_of(&lines, 10);

        assert_eq!(map.char_to_offset(0), Some(0.0));
        assert_eq!(map.char_to_offset(4), Some(0.0), "同一行共享行首偏移");
        assert_eq!(map.char_to_offset(5), Some(20.0), "第二行起点");
        assert_eq!(map.char_to_offset(9), Some(20.0));
        assert_eq!(map.char_to_offset(10), Some(40.0), "末尾 = 总高");
    }

    #[test]
    fn offset_maps_to_line_first_char() {
        let lines = vec![page_line(0, 5, 10.0, 20.0), page_line(5, 5, 10.0, 20.0)];
        let map = map_of(&lines, 10);

        assert_eq!(map.offset_to_char(0.0), Some(0));
        assert_eq!(map.offset_to_char(19.9), Some(0), "仍在第一行");
        assert_eq!(map.offset_to_char(20.0), Some(5), "第二行首字符");
        assert_eq!(map.offset_to_char(39.9), Some(5));
        assert_eq!(map.offset_to_char(40.0), Some(10), "末尾");
    }

    /// 幂等性是本模块的核心性质。
    #[test]
    fn vertical_roundtrip_is_idempotent() {
        let lines = vec![
            page_line(0, 5, 10.0, 20.0),
            page_line(5, 5, 10.0, 33.0),
            page_line(10, 5, 10.0, 17.0),
        ];
        let map = map_of(&lines, 15);

        let mut offset = 0.0f32;
        while offset < 80.0 {
            let first_char = map.offset_to_char(offset).expect("应可映射");
            let first_offset = map.char_to_offset(first_char).expect("应可映射");
            let second_char = map.offset_to_char(first_offset).expect("应可映射");
            let second_offset = map.char_to_offset(second_char).expect("应可映射");
            assert_eq!(second_char, first_char);
            assert!((second_offset - first_offset).abs() < 1e-3, "非幂等");
            offset += 3.0;
        }
    }

    #[test]
    fn out_of_range_is_clamped() {
        let lines = vec![page_line(0, 5, 10.0, 20.0)];
        let map = map_of(&lines, 5);

        assert_eq!(map.offset_to_char(-1e9), Some(0));
        assert_eq!(map.offset_to_char(f32::NAN), Some(0));
        assert_eq!(map.offset_to_char(1e9), Some(5));
        assert_eq!(map.char_to_offset(9999), Some(20.0));
    }

    #[test]
    fn empty_lines_are_safe() {
        let map = map_of(&[], 0);
        assert_eq!(map.char_to_offset(0), Some(0.0));
        assert_eq!(map.offset_to_char(0.0), Some(0));
        assert_eq!(map.offset_to_char(500.0), Some(0));
        assert_eq!(map.total_height_px(), 0.0);
    }

    #[test]
    fn horizontal_mapping_uses_glyph_advances() {
        let line = page_line(0, 3, 10.0, 20.0);
        let map = map_of(std::slice::from_ref(&line), 3);

        assert_eq!(map.point_in_line_to_char(&line, 0.0), Some(0));
        assert_eq!(map.point_in_line_to_char(&line, 5.0), Some(0), "第一字内");
        assert_eq!(map.point_in_line_to_char(&line, 15.0), Some(1), "第二字内");
        assert_eq!(map.point_in_line_to_char(&line, 25.0), Some(2), "第三字内");
        assert_eq!(map.point_in_line_to_char(&line, 999.0), Some(3), "行末");

        // 返回字形**中心**（半个字身的误差上界来自这里，见方法文档）
        assert_eq!(map.char_to_point_in_line(&line, 0), Some(5.0));
        assert_eq!(map.char_to_point_in_line(&line, 1), Some(15.0));
        assert_eq!(map.char_to_point_in_line(&line, 2), Some(25.0));
        assert_eq!(map.char_to_point_in_line(&line, 3), None, "行外字符");
    }

    /// 章内偏移与段内偏移的换算（多段时不得错位）。
    #[test]
    fn horizontal_mapping_translates_chapter_offsets() {
        // 第二段：章内从 100 起，段内从 0 起
        let line = page_line(100, 3, 10.0, 20.0);
        let map = map_of(std::slice::from_ref(&line), 200);

        assert_eq!(
            map.point_in_line_to_char(&line, 15.0),
            Some(101),
            "第二字应换算为章内偏移 101"
        );
        assert_eq!(map.char_to_point_in_line(&line, 101), Some(15.0));
    }
}
