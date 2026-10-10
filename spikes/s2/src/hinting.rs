//! R5-2：hinting 对 `x_advance` 挤压的量化影响（T006）
//! 与 R5-7：贪心换行 + 挤压的排版质量评测
//!
//! ## R5-2 要验证什么
//!
//! research.md R1/R4 共同决策「hinting 关闭」，理由是：hinting 会把
//! `x_advance` **取整到整像素**，而标点挤压靠的是**亚像素**级别的字身改写
//! （半字身 = 9.5px，在 19px 字号下）。若挤压量被量化吃掉，挤压语义就失效。
//!
//! ### 源码依据（已核实 cosmic-text 0.19.0 shape.rs）
//!
//! ```ignore
//! // shape.rs:2910-2912
//! if hinting == Hinting::Enabled {
//!     x_advance = x_advance.round();
//! }
//! ```
//!
//! 即：开启 hinting 时，**每个字形的水平步进被四舍五入到整像素**。
//! 这不是"稍微失真"，而是"亚像素量归零"。
//!
//! ## R5-7 要验证什么
//!
//! cosmic-text 是 **first-fit（贪心）**换行，不是 Knuth-Plass 全局最优。
//! 贪心 + 挤压的组合可能产出"次优断点"——即行末留白明显偏大、本可通过
//! 挤压把下一个字放进来的位置。本模块给出**次优断点出现率**的口径与实测值。
//!
//! ## 复跑
//!
//! ```bash
//! cd spikes/s2 && cargo test hinting -- --nocapture
//! ```

use cosmic_text::{Attrs, Family, Hinting, Metrics, Shaping, Wrap};

use crate::word_merge::shape_line_of;

// ===========================================================================
// R5-2：hinting 对挤压量的量化影响
// ===========================================================================
/// 一次测量的结果（**两层**：shape 与 layout）。
///
/// ## 为什么必须分两层测（T006 实测的关键修正）
///
/// 初版只测了 `ShapeGlyph::x_advance`，结论是"hinting 无影响（偏差 0.0000px）"
/// ——**那是错的**。实测发现（见 `probe_advance_layers`）：
///
/// | 层 | 单位 | hinting 开 | hinting 关 |
/// |---|---|---|---|
/// | `ShapeGlyph::x_advance` | **em** | 同左 | 同左（**两层完全相同**） |
/// | `LayoutGlyph::w` | px | **全为整像素**（0/17 非整） | 7/17 非整（如 9.936） |
///
/// 原因在源码：`ShapeGlyph::x_advance` 是 **em 单位的字形步进**（与字号无关），
/// 量化发生在 `layout()` 里 `x_advance = font_size * glyph.x_advance` **之后**
/// 的 `x_advance.round()`（shape.rs:2910-2912）。故：
///
/// - **shape 层（挤压改写点）不受 hinting 影响** —— 这对方案 A 是好消息
/// - **layout 层的 px 宽度被量化** —— 这才是挤压语义被吃掉的地方
#[derive(Debug, Clone)]
pub struct HintingMeasurement {
    /// 版心宽度（px）
    pub width: f32,
    /// 开启 hinting 时 shape 层的 `x_advance`（**em 单位**）
    pub shape_advances_hinted: Vec<f32>,
    /// 关闭 hinting 时 shape 层的 `x_advance`（**em 单位**）
    pub shape_advances_unhinted: Vec<f32>,
    /// 开启 hinting 时 layout 层的字形宽度（**px**）
    pub layout_widths_hinted: Vec<f32>,
    /// 关闭 hinting 时 layout 层的字形宽度（**px**）
    pub layout_widths_unhinted: Vec<f32>,
}

impl HintingMeasurement {
    /// shape 层开/关 hinting 是否**完全一致**。
    ///
    /// 若一致，说明挤压改写在 shape 层做是 hinting 无关的（好消息）；
    /// 若不一致，则挤压量在改写点就被 hinting 动了手脚。
    #[must_use]
    pub fn shape_layer_is_hinting_independent(&self) -> bool {
        self.shape_advances_hinted.len() == self.shape_advances_unhinted.len()
            && self
                .shape_advances_hinted
                .iter()
                .zip(&self.shape_advances_unhinted)
                .all(|(hinted, unhinted)| (hinted - unhinted).abs() < 1e-6)
    }

    /// 开启 hinting 时 layout 层宽度是否全为整像素。
    #[must_use]
    pub fn layout_widths_hinted_are_integral(&self) -> bool {
        self.layout_widths_hinted
            .iter()
            .all(|width| (width - width.round()).abs() < 1e-3)
    }

    /// 关闭 hinting 时 layout 层是否存在非整像素宽度。
    #[must_use]
    pub fn layout_widths_unhinted_have_subpixel(&self) -> bool {
        self.layout_widths_unhinted
            .iter()
            .any(|width| (width - width.round()).abs() > 1e-3)
    }

    /// layout 层因量化产生的最大单字形偏差（px）。
    ///
    /// 这**才是**"hinting 吃掉多少挤压量"的正确度量。
    #[must_use]
    pub fn max_layout_delta_px(&self) -> f32 {
        self.layout_widths_hinted
            .iter()
            .zip(&self.layout_widths_unhinted)
            .map(|(hinted, unhinted)| (hinted - unhinted).abs())
            .fold(0.0f32, f32::max)
    }

    /// 非整像素布局宽度的个数（关闭 hinting 时）。
    #[must_use]
    pub fn subpixel_width_count_unhinted(&self) -> usize {
        self.layout_widths_unhinted
            .iter()
            .filter(|width| (**width - width.round()).abs() > 1e-3)
            .count()
    }
}

/// 用给定 hinting 设置 shape + layout 一段文本，两层都取。
#[must_use]
pub fn measure_both(text: &str, width: f32) -> HintingMeasurement {
    let enabled = measure_one_layer(text, width, Hinting::Enabled);
    let disabled = measure_one_layer(text, width, Hinting::Disabled);
    HintingMeasurement {
        width,
        shape_advances_hinted: enabled.0,
        shape_advances_unhinted: disabled.0,
        layout_widths_hinted: enabled.1,
        layout_widths_unhinted: disabled.1,
    }
}

/// 单次测量：返回 (shape 层 x_advance[em], layout 层 w[px])。
fn measure_one_layer(text: &str, width: f32, hinting: Hinting) -> (Vec<f32>, Vec<f32>) {
    let (mut font_system, mut buffer) = shaped_buffer_with_hinting(text, width, hinting);
    let shape_line = shape_line_of(&mut font_system, &mut buffer, 0);

    let shape_advances = shape_line
        .spans
        .iter()
        .flat_map(|span| span.words.iter())
        .flat_map(|word| word.glyphs.iter())
        .map(|glyph| glyph.x_advance)
        .collect();

    let layout_widths = shape_line
        .layout(
            crate::word_merge::TEST_FONT_SIZE,
            Some(width),
            Wrap::Word,
            None,
            None,
            hinting,
        )
        .iter()
        .flat_map(|line| line.glyphs.iter())
        .map(|glyph| glyph.w)
        .collect();

    (shape_advances, layout_widths)
}

/// 仅取 shape 层的 `x_advance`（em 单位）——供需要单独看该层的探针用。
#[must_use]
pub fn measure_shape_advances(text: &str, width: f32, hinting: Hinting) -> Vec<f32> {
    measure_one_layer(text, width, hinting).0
}

/// 同 [`crate::word_merge::shaped_buffer`]，但可指定 hinting。
fn shaped_buffer_with_hinting(
    text: &str,
    width: f32,
    hinting: Hinting,
) -> (cosmic_text::FontSystem, cosmic_text::Buffer) {
    let mut font_system = cosmic_text::FontSystem::new();
    let metrics = Metrics::relative(
        crate::word_merge::TEST_FONT_SIZE,
        crate::word_merge::TEST_LINE_HEIGHT_SCALE,
    );
    let mut buffer = cosmic_text::Buffer::new(&mut font_system, metrics);
    buffer.set_size(Some(width), None);
    buffer.set_wrap(Wrap::Word);
    buffer.set_hinting(hinting);
    buffer.set_text(
        text,
        &Attrs::new().family(Family::Serif),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);
    (font_system, buffer)
}

/// **判据一**：hinting **不影响** shape 层的 `x_advance`（挤压改写点安全）。
///
/// 这是方案 A 的好消息：挤压改写 `ShapeGlyph::x_advance` 的操作，
/// 在开/关 hinting 下**写入同一个值**（该字段是 em 单位，量化发生在
/// layout 时才乘字号）。故"改写是否被 hinting 干扰"这个担心不成立。
#[test]
fn r5_2_shape_layer_is_hinting_independent() {
    let text = "中文标点，测试english混排。";
    let measurement = measure_both(text, 400.0);

    assert!(
        !measurement.shape_advances_hinted.is_empty(),
        "测量未取到任何字形（夹具失效）"
    );
    assert!(
        measurement.shape_layer_is_hinting_independent(),
        "【判据一失败】shape 层 x_advance 随 hinting 变化——\
         这意味着挤压改写点会被 hinting 干扰，方案 A 的风险面比预期大。\n\
         hinting 开：{:?}\nhinting 关：{:?}",
        measurement.shape_advances_hinted,
        measurement.shape_advances_unhinted
    );
}

/// **判据二**：hinting 把 layout 层宽度量化为整像素。
#[test]
fn r5_2_layout_layer_is_quantized_by_hinting() {
    let text = "中文标点，测试english混排。";
    let measurement = measure_both(text, 400.0);

    assert!(
        measurement.layout_widths_hinted_are_integral(),
        "开启 hinting 后 layout 层宽度未全为整像素：{:?}",
        measurement.layout_widths_hinted
    );
}

/// **判据三**：关闭 hinting 保留亚像素宽度（与判据二构成完整论证）。
#[test]
fn r5_2_subpixel_is_preserved_when_hinting_disabled() {
    let text = "中文标点，测试english混排。";
    let measurement = measure_both(text, 400.0);

    assert!(
        measurement.layout_widths_unhinted_have_subpixel(),
        "关闭 hinting 时未见任何非整像素宽度——判据不成立。实测：{:?}",
        measurement.layout_widths_unhinted
    );
}

/// **判据四（量化结论）**：量化偏差相对半字身挤压量的占比。
///
/// 这是"默认关 hinting"决策的**数据支撑**：若偏差能达到半字身挤压量的
/// 可观比例，则挤压在开 hinting 时不可用。
#[test]
fn r5_2_quantization_delta_is_quantified() {
    let text = "中文标点，测试english混排。一二三四五六七八九十";
    let measurement = measure_both(text, 400.0);

    let delta = measurement.max_layout_delta_px();
    let half_em = crate::word_merge::TEST_FONT_SIZE / 2.0;
    let subpixel_count = measurement.subpixel_width_count_unhinted();
    let total = measurement.layout_widths_unhinted.len();

    println!(
        "[R5-2] layout 层最大量化偏差 = {delta:.4}px；半字身 = {half_em:.2}px；\
         偏差/半字身 = {:.2}%",
        delta / half_em * 100.0
    );
    println!("[R5-2] 关闭 hinting 时非整像素宽度 {subpixel_count} / {total} 个字形");

    // 结论性断言：偏差必须显著存在（否则 hinting 无所谓，关它是无谓牺牲）。
    // 阈值取 "≥ 0.1px"——这是"亚像素确实存在"的下界，不是审美判断。
    assert!(
        delta >= 0.1,
        "layout 层量化偏差仅 {delta:.4}px，不足以支撑「默认关 hinting」的决策——\
         需重新评估（research R1/R4 的该条假设不成立）"
    );
}

/// 挤压语义的**直接量化**：亚像素字身能否在开 hinting 下表达。
///
/// ## 用什么量作为"挤压的载体"（两次修正后才对）
///
/// - 初版测**孤立的** `，`：得到 19px 整像素——因为独立标点被当成整字身。
/// - 二版测 `测试标点，压缩。`：8 个字形**全是 19px**——因为**全角标点的
///   天然步进就是 1 个 em**（19px）。挤压是 S2-1 要**施加**的改写，不是
///   cosmic-text 的既有行为。
///
/// 故正确的载体是**天然就带亚像素步进的字形**：半角西文（`g` ≈ 0.52em，
/// 在 19px 字号下约 9.94px）。这与挤压要表达的量级相同（半字身 ≈ 9.5px），
/// 正好用来验证"亚像素在这个渲染栈里是否可表达"。
///
/// 若 hinting 开时该字形被压成 10px（整像素），则说明**任何**亚像素字身
/// 都会被量化——包括挤压想写进去的半字身。
#[test]
fn r5_2_half_em_squeeze_is_quantized_when_hinting_on() {
    let em = crate::word_merge::TEST_FONT_SIZE;
    let half_em = em / 2.0;
    // 纯 ASCII 单字：字形步进天然是半字身量级（< 1em）
    let text = "g";

    let measurement = measure_both(text, 400.0);

    let hinted = measurement
        .layout_widths_hinted
        .first()
        .copied()
        .expect("应有至少一个字形");
    let unhinted = measurement
        .layout_widths_unhinted
        .first()
        .copied()
        .expect("应有至少一个字形");

    println!("[R5-2] 半角字形 `g` 步进：hinting 关 = {unhinted:.5}px");
    println!("[R5-2]                      hinting 开 = {hinted:.5}px");
    println!(
        "[R5-2] 半字身目标 = {half_em}px；关时偏差 = {:.5}px，开时偏差 = {:.5}px",
        (unhinted - half_em).abs(),
        (hinted - half_em).abs()
    );

    // 前提自检：该字形天然就是亚像素（否则本判据的载体选错了）
    assert!(
        (unhinted - unhinted.round()).abs() > 1e-3,
        "hinting 关时该字形应为非整像素（亚像素可表达的前提），实测 {unhinted}"
    );
    assert!(
        unhinted < em,
        "该字形步进应小于整字身（属半字身量级），实测 {unhinted}"
    );

    // 核心判据：开启 hinting 后，这个亚像素量被抹平成整数
    assert!(
        (hinted - hinted.round()).abs() < 1e-3,
        "hinting 开时亚像素字身应被量化为整像素，实测 {hinted}——\
         若未被量化，则「hinting 吃掉挤压量」的假设不成立"
    );

    // 量化造成的绝对偏差（这才是挤压会损失的量）
    println!(
        "[R5-2] ⇒ 量化偏差 = {:.5}px（相对半字身 {:.1}%）",
        (hinted - unhinted).abs(),
        (hinted - unhinted).abs() / half_em * 100.0
    );
}

/// R5-2 与 FR-055（三平台一致）的关系：hinting 是否改变断点位置。
///
/// S2-1 的挤压依赖"改写 x_advance 后 layout 按新宽度重算断点"。
/// 若 hinting 开时断点也被量化影响，则同一输入在开/关 hinting 下会排版不同
/// ——那会与"同字体同参数分页结构一致"（SC-015）的目标相悖。
#[test]
fn r5_2_hinting_may_change_break_positions() {
    let text = "阅读 reader 与排版 typesetting 的关系在于字形度量的优劣。";
    let width = 361.0;

    let (mut font_system_on, mut buffer_on) =
        shaped_buffer_with_hinting(text, width, Hinting::Enabled);
    let lines_on: Vec<usize> = shape_line_of(&mut font_system_on, &mut buffer_on, 0)
        .layout(
            crate::word_merge::TEST_FONT_SIZE,
            Some(width),
            Wrap::Word,
            None,
            None,
            Hinting::Enabled,
        )
        .iter()
        .map(|line| line.glyphs.len())
        .collect();

    let (mut font_system_off, mut buffer_off) =
        shaped_buffer_with_hinting(text, width, Hinting::Disabled);
    let lines_off: Vec<usize> = shape_line_of(&mut font_system_off, &mut buffer_off, 0)
        .layout(
            crate::word_merge::TEST_FONT_SIZE,
            Some(width),
            Wrap::Word,
            None,
            None,
            Hinting::Disabled,
        )
        .iter()
        .map(|line| line.glyphs.len())
        .collect();

    println!("[R5-2] hinting 开：每行字形数 = {lines_on:?}");
    println!("[R5-2] hinting 关：每行字形数 = {lines_off:?}");
    if lines_on != lines_off {
        println!("[R5-2] ⇒ hinting 确实改变了断点位置（开/关结果不同）");
    } else {
        println!("[R5-2] ⇒ 本用例下 hinting 未改变断点位置（但步进已被量化）");
    }

    // 不断言"必不相同"（可能恰好相同）；事实进 BASELINE.md 作为佐证。
}

// ===========================================================================
// R5-7：贪心换行 + 挤压的排版质量
// ===========================================================================

/// 一行与版心之间宽度的测量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineWidth {
    /// 实际宽度（px）
    pub actual_px: f32,
    /// 版心宽度（px）
    pub measure_px: f32,
}

impl LineWidth {
    /// 行末留白（px）。
    #[must_use]
    pub fn slack_px(self) -> f32 {
        (self.measure_px - self.actual_px).max(0.0)
    }

    /// 留白占比（0.0–1.0）。
    #[must_use]
    pub fn slack_ratio(self) -> f32 {
        if self.measure_px <= 0.0 {
            return 0.0;
        }
        self.slack_px() / self.measure_px
    }
}

/// 测一次排版的逐行宽度（hinting 关闭——本层的一致前提）。
#[must_use]
pub fn measure_line_widths(text: &str, width: f32) -> Vec<LineWidth> {
    let (mut font_system, mut buffer) = crate::word_merge::shaped_buffer(text, width);
    let shape_line = shape_line_of(&mut font_system, &mut buffer, 0);
    shape_line
        .layout(
            crate::word_merge::TEST_FONT_SIZE,
            Some(width),
            Wrap::Word,
            None,
            None,
            Hinting::Disabled,
        )
        .iter()
        .map(|line| LineWidth {
            actual_px: line.w,
            measure_px: width,
        })
        .collect()
}

/// 次优断点的**主口径**：一行排完后，**下一个 word 本可放进本行**。
///
/// ## 为什么不用"留白 ≥ 1 字身"（初版口径，已废弃）
///
/// 初版用"非末行留白 ≥ 1 整字身"判次优。实测发现它**系统性高估**：
/// 混排语料里行末的下一个 word 往往是一个长西文单词（如 `position` 59px），
/// 留白 71.8px 虽 ≥ 1 字身（19px），但……**这一个能放下**（248.2+59=307.2 ≤ 320）
/// ——那是真次优；而留白 20px 时下一个西文词 59px 放不下，那就**不是**次优，
/// 却被旧口径误判为次优。
///
/// 故主干口径改为对**下一个 word 的实际宽度**判定——它直接回答
/// "贪心是不是放弃了本可以收益的机会"，不依赖任何阈值猜测。
///
/// ## 已知低估
///
/// 本口径不计"靠挤压 0.5 字身就能多放一个字"的情形（S2-1 的挤压尚未实现）。
/// 故它给出的是次优率的**下界**——宁可低估不可高估。
#[must_use]
pub fn greedy_missed_fit_count(rows: &[RowMeasurement]) -> usize {
    rows.iter().filter(|row| row.next_fits).count()
}

/// 一行及其"下一个 word 是否本可放进本行"。
#[derive(Debug, Clone, PartialEq)]
pub struct RowMeasurement {
    /// 本行实际宽度（px）
    pub actual_px: f32,
    /// 版心宽度（px）
    pub measure_px: f32,
    /// 下一个 word 的宽度（px）；末行为 `None`
    pub next_word_px: Option<f32>,
    /// 下一个 word 本可放进本行（**真次优**）
    pub next_fits: bool,
}

impl RowMeasurement {
    /// 行末留白（px）。
    #[must_use]
    pub fn slack_px(&self) -> f32 {
        (self.measure_px - self.actual_px).max(0.0)
    }
}

/// 逐行测量"下一个 word 是否本可放进本行"。
///
/// ## 实现依据（T005 实测的结构）
///
/// 中文逐字成 word、西文整词成一个 word。故对一个"行边界"而言，
/// **下一个 word** 就是紧邻该行末尾的那个 word——把它的宽度与该行剩余
/// 空间比较即可。这里用字节区间求交来定位。
#[must_use]
pub fn measure_rows(text: &str, width: f32) -> Vec<RowMeasurement> {
    let (mut font_system, mut buffer) = crate::word_merge::shaped_buffer(text, width);
    let shape_line = shape_line_of(&mut font_system, &mut buffer, 0);

    // word 宽度表（按原文起点索引，便于按位置查）
    let word_widths: Vec<(usize, usize, f32)> = shape_line
        .spans
        .iter()
        .flat_map(|span| span.words.iter())
        .map(|word| {
            let range = match (word.glyphs.first(), word.glyphs.last()) {
                (Some(first), Some(last)) => first.start..last.end,
                _ => 0..0,
            };
            (
                range.start,
                range.end,
                word.width(crate::word_merge::TEST_FONT_SIZE),
            )
        })
        .collect();

    let laid = shape_line.layout(
        crate::word_merge::TEST_FONT_SIZE,
        Some(width),
        Wrap::Word,
        None,
        None,
        Hinting::Disabled,
    );

    laid.iter()
        .enumerate()
        .map(|(index, line)| {
            let end = line.glyphs.last().map_or(0, |glyph| glyph.end);
            let is_last = index + 1 == laid.len();

            // 找"起点 ≥ 本行末"的第一个 word，即下一个 word
            let next = word_widths.iter().find(|(start, _, _)| *start >= end);
            let next_word_px = next.map(|(_, _, w)| *w);
            let next_fits = match next_word_px {
                Some(next_width) if !is_last => line.w + next_width <= width,
                _ => false,
            };

            RowMeasurement {
                actual_px: line.w,
                measure_px: width,
                next_word_px,
                next_fits,
            }
        })
        .collect()
}

/// 次优断点率（主口径）：本可多放一个 word 却放弃的行数 / 非末行总数。
#[must_use]
pub fn greedy_missed_fit_ratio(rows: &[RowMeasurement]) -> f32 {
    if rows.len() <= 1 {
        return 0.0;
    }
    let body = &rows[..rows.len() - 1];
    let missed = body.iter().filter(|row| row.next_fits).count();
    missed as f32 / body.len() as f32
}

/// **R5-7 判据一**：等宽中文下贪心**无次优**（每行都填满）。
///
/// 纯中文每字一个 word 且字身相等（19px），贪心换行在整除版心下必然最优。
/// 预期次优率 **0%**——若不为 0，说明断行粒度或宽度计算有意外。
#[test]
fn r5_7_uniform_cjk_has_no_missed_fit() {
    let em = crate::word_merge::TEST_FONT_SIZE;
    let text = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉戌亥".repeat(3);
    let rows = measure_rows(&text, 3.0 * em);

    assert!(rows.len() > 2, "语料应排成多行，实测 {} 行", rows.len());

    let ratio = greedy_missed_fit_ratio(&rows);
    println!(
        "[R5-7] 等宽中文（版心 3 字身）：{} 行，次优率 = {:.1}%",
        rows.len(),
        ratio * 100.0
    );

    assert_eq!(
        ratio,
        0.0,
        "等宽中文的贪心换行不应放弃任何可放的 word，实测次优率 {:.1}%。各行：{:?}",
        ratio * 100.0,
        rows.iter()
            .map(|row| (row.actual_px, row.slack_px(), row.next_word_px))
            .collect::<Vec<_>>()
    );
}

/// **R5-7 判据二**：非整除版心下中文仍无次优。
///
/// 版心非字身整数倍时，每行会留不足一字身的余量——放不下下一个字，
/// **不是**次优。真次优只在"下一个 word 更窄"时才可能出现（中文里
/// 下一个 word 也是整字身，故仍不可能）。预期 0%。
#[test]
fn r5_7_non_integral_measure_has_no_missed_fit_for_cjk() {
    let em = crate::word_merge::TEST_FONT_SIZE;
    let text = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉戌亥".repeat(4);

    for width in [300.0f32, 320.0, 342.0, 361.0, 380.0, 400.0, 419.0, 437.0] {
        let rows = measure_rows(&text, width);
        let ratio = greedy_missed_fit_ratio(&rows);
        println!(
            "[R5-7] 版心 {width}px（{:.2} 字身）：{} 行，次优率 {:.1}%",
            width / em,
            rows.len(),
            ratio * 100.0
        );
        assert_eq!(
            ratio, 0.0,
            "非整除版心 {width}px 下中文出现次优断点（{:.1}%）——             中文的 word 宽度恒等于字身，不应存在「本可多放一个」的情形",
            ratio * 100.0
        );
    }
}

/// **R5-7 判据三**：混排（中西文）**确有**次优断点——这是本 spike 的实质发现。
///
/// ## 依据（`probe_mixed_break_detail` 的实测明细）
///
/// 版心 320px 下的一行：
///
/// ```
///   line[2] w=248.2 留白=71.8 文本="metric 决定了断行 break 位置"
/// ```
///
/// 下一个 word 是 `position`（宽 59px）。`248.2 + 59 = 307.2 ≤ 320` ——
/// **它本可以放进去**，贪心却把它推到了下一行。随后 `position` 那句又留下
/// 109.3px 空白。这是 first-fit 不回溯的**直接代价**。
///
/// ## 本测试的立场
///
/// 它**不**断言次优率为 0（那与实测不符，是假绿）。它断言的是：
/// **次优率确实 > 0**（即本 spike 真的观测到了该现象），并把它打印出来
/// 作为 S2-1 的输入数据。是否可接受属产品判断，记录在 BASELINE.md。
/// 若某次改动把它变成 0，本测试会失败并提示"结论变了，需重新评估"。
#[test]
fn r5_7_mixed_script_does_have_missed_fits() {
    let em = crate::word_merge::TEST_FONT_SIZE;
    let text = "阅读 reader 与排版 typesetting 的关系 relationship 在于               字形 glyph 的度量 metric 决定了断行 break 位置 position 的优劣 quality。";

    let mut ratios = Vec::new();
    for width in [320.0f32, 361.0, 400.0, 437.0, 480.0] {
        let rows = measure_rows(text, width);
        let ratio = greedy_missed_fit_ratio(&rows);
        let missed = greedy_missed_fit_count(&rows);
        println!(
            "[R5-7] 混排 版心 {width}px（{:.1} 字身）：{} 行，次优 {missed} 处，率 {:.1}%",
            width / em,
            rows.len(),
            ratio * 100.0
        );
        ratios.push(ratio);
    }

    let worst = ratios.iter().copied().fold(0.0f32, f32::max);
    println!("[R5-7] 混排最差次优率 = {:.1}%", worst * 100.0);

    assert!(
        worst > 0.0,
        "混排语料下未观测到任何次优断点——与 probe_mixed_break_detail 的实测不符。         若贪心行为真变了（例如 cosmic-text 改用回溯算法），         则 R5-7 的结论需重新评估并在 BASELINE.md 更新。"
    );
}

/// **R5-7 判据四**：等宽中文 + 整除版心下，非末行填满（留白为 0）。
///
/// 这是"贪心在它擅长的场景里确实最优"的正向证据——与判据三合起来说明
/// 问题**只在混排**，不在中文本身。
#[test]
fn r5_7_greedy_fills_lines_for_uniform_cjk() {
    let em = crate::word_merge::TEST_FONT_SIZE;
    let text = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉戌亥";
    let rows = measure_rows(text, 4.0 * em);

    let body: Vec<f32> = rows[..rows.len() - 1]
        .iter()
        .map(RowMeasurement::slack_px)
        .collect();
    println!("[R5-7] 版心 4 字身，非末行留白 = {body:?}");

    assert!(
        body.iter().all(|slack| *slack < 0.01),
        "等宽中文 + 整除版心时贪心应把每行填满（留白 0），实测留白 {body:?}"
    );
}

/// R5-7 的**对照事实**：贪心是 first-fit，不回溯。
///
/// 用最小可判定的混排构造演示：一个短词跟在行末，若贪心把它放进本行，
/// 随后的大 word 必须整体下移；而"回溯"本可把短词也挪下去换取更均匀的两行。
#[test]
fn r5_7_greedy_does_not_backtrack() {
    let em = crate::word_merge::TEST_FONT_SIZE;
    // 中文铺到接近版心，再跟一个西文长词 —— 制造"整词回退"的机会
    let text = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申西文word在此处";
    let width = 5.0 * em; // 95px

    let rows = measure_rows(text, width);
    println!("[R5-7] 版心 5 字身（{width}px）：{} 行", rows.len());
    for (index, row) in rows.iter().enumerate() {
        println!(
            "  line[{index}] 宽 {:.1}，留白 {:.1}，下一 word {:?}px，本可放下={}",
            row.actual_px,
            row.slack_px(),
            row.next_word_px.map(|width| (width * 10.0).round() / 10.0),
            row.next_fits
        );
    }

    // 定性断言：贪心产出的行里，至少有一行是"填满"的（它不回退去均衡）
    let filled = rows[..rows.len() - 1]
        .iter()
        .filter(|row| row.slack_px() < 0.5)
        .count();
    assert!(
        filled > 0,
        "贪心应至少填满一行（first-fit 的特征），实测无 —— 行为异常，需重新评估"
    );
}

/// word 粒度的一次自检：确保本模块的测量前提仍然成立。
#[test]
fn r5_7_word_granularity_is_unchanged() {
    let (mut font_system, mut buffer) = crate::word_merge::shaped_buffer("甲乙丙", 200.0);
    let words = crate::word_merge::word_ranges(&shape_line_of(&mut font_system, &mut buffer, 0));
    assert_eq!(
        words.len(),
        3,
        "T005 实测「中文逐字成 word」——若此前提变了，本模块的次优断点口径需重新推导"
    );
}

// ===========================================================================
// 诊断探针：hinting 到底在哪一层量化
// ===========================================================================

#[test]
#[ignore = "诊断探针：对比 shape 层与 layout 层的 x_advance"]
fn probe_advance_layers() {
    let text = "中文标点，测试english混排。";
    let width = 400.0;

    for (label, hinting) in [
        ("Enabled", Hinting::Enabled),
        ("Disabled", Hinting::Disabled),
    ] {
        let (mut font_system, mut buffer) = shaped_buffer_with_hinting(text, width, hinting);
        let line = shape_line_of(&mut font_system, &mut buffer, 0);

        let shape_advances: Vec<f32> = line
            .spans
            .iter()
            .flat_map(|s| s.words.iter())
            .flat_map(|w| w.glyphs.iter())
            .map(|g| g.x_advance)
            .collect();

        let layout_advances: Vec<f32> = line
            .layout(
                crate::word_merge::TEST_FONT_SIZE,
                Some(width),
                Wrap::Word,
                None,
                None,
                hinting,
            )
            .iter()
            .flat_map(|l| l.glyphs.iter())
            .map(|g| g.w)
            .collect();

        println!("--- hinting {label} ---");
        println!(
            "  shape 层 x_advance（em 单位）前 8 个: {:?}",
            &shape_advances[..shape_advances.len().min(8)]
        );
        println!(
            "  layout 层 w（px）前 8 个: {:?}",
            &layout_advances[..layout_advances.len().min(8)]
        );
        let em = crate::word_merge::TEST_FONT_SIZE;
        let non_integral_shape = shape_advances
            .iter()
            .filter(|advance| {
                let px = **advance * em;
                (px - px.round()).abs() > 1e-3
            })
            .count();
        let non_integral_layout = layout_advances
            .iter()
            .filter(|width| (**width - width.round()).abs() > 1e-3)
            .count();
        println!(
            "  shape 层非整像素数: {non_integral_shape} / {}",
            shape_advances.len()
        );
        println!(
            "  layout 层非整像素数: {non_integral_layout} / {}",
            layout_advances.len()
        );
    }
}

#[test]
#[ignore = "诊断探针：混排次优断点的明细（哪一行、留白多少、本可放什么）"]
fn probe_mixed_break_detail() {
    let em = crate::word_merge::TEST_FONT_SIZE;
    let text = "阅读 reader 与排版 typesetting 的关系 relationship 在于\
               字形 glyph 的度量 metric 决定了断行 break 位置 position 的优劣 quality。";
    for width in [320.0f32, 361.0, 400.0] {
        println!("=== 版心 {width}px ({:.2} 字身) ===", width / em);
        let (mut font_system, mut buffer) = crate::word_merge::shaped_buffer(text, width);
        let line = shape_line_of(&mut font_system, &mut buffer, 0);
        let laid = line.layout(
            crate::word_merge::TEST_FONT_SIZE,
            Some(width),
            Wrap::Word,
            None,
            None,
            Hinting::Disabled,
        );
        for (i, l) in laid.iter().enumerate() {
            let seg: String = l
                .glyphs
                .iter()
                .map(|g| &text[g.start.min(text.len())..g.end.min(text.len())])
                .collect();
            println!(
                "  line[{i}] w={:.1} 留白={:.1} ({:.0}%) 文本={:?}",
                l.w,
                (width - l.w).max(0.0),
                (width - l.w).max(0.0) / width * 100.0,
                seg
            );
        }
        // 列出 word 边界（供判断"留白是否真的本可多放"）
        println!(
            "  words: {:?}",
            crate::word_merge::word_ranges(&line)
                .iter()
                .map(|(_, _, range)| {
                    let text_slice: String =
                        text[range.start.min(text.len())..range.end.min(text.len())].into();
                    format!("{text_slice}({})", range.start)
                })
                .collect::<Vec<_>>()
        );
    }
}
