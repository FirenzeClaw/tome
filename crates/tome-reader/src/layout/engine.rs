//! [DONE] 排版引擎：ShapeLine 克隆后处理（T013 / research R1 方案 A）
//!
//! ## 引擎在做什么（一句话）
//!
//! cosmic-text 的断行只认 **word 边界**（T005 实测证实）。故"禁止在 A 与 B
//! 之间断行"的**实现手段就是**把 A 与 B 合并成同一个 `ShapeWord`——
//! 断行自然不会再落在它们之间。
//!
//! ```text
//! 原始 shape 结果            合并后                  layout()
//! ┌───┬───┬───┐             ┌─────┬───┐            行1: 甲乙
//! │甲 │乙 │丙 │   ──────→   │甲乙 │丙 │   ──────→   行2: 丙
//! └───┴───┴───┘             └─────┴───┘
//!   （丙前可断）              （丙前仍可断，甲乙间不可）
//! ```
//!
//! ## 三个来自 T005 spike 硬约束（照做，不重新论证）
//!
//! 1. **必须在 `Wrap::Word` 下做**。实测 `Wrap::WordOrGlyph` 在遇到超宽
//!    西文单词时会**切换到 glyph 路径**，而那条路径无视 word 边界——
//!    合并 word 的禁则手段对它失效。故 FR-009 的"超长单词退化为按字符
//!    断行"**不能**靠 `WordOrGlyph` 获得，须在 T014 自行实现。
//! 2. **hinting 关闭**。实测量化发生在 layout 层（px），偏差达半字身
//!    挤压单位的 4.19%——开 hinting 会让挤压语义不可表达。
//!    注意 shape 层的 `x_advance`（em 单位）改写**不受** hinting 干扰，
//!    故挤压改写点无需额外处理。
//! 3. **合并不得丢字形、不得改原文区间**。T005 已实测（`merge_words`
//!    的 `glyphs` 移动保持 `start`/`end` 序列不变），本层复用该实现。

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Hinting, Metrics, ShapeLine, ShapeWord, Shaping, Wrap,
};

use crate::layout::linebox::{GlyphBox, HardOrderStep, LineBox, RuleExceptionRecord};
use crate::rules::cjk::{break_decision, BreakDecision};
use crate::text::model::TextModel;

/// 排版几何参数（T013 阶段的最小集；完整的 `ReadingConfig` 属 T017）。
///
/// ## 为什么只放这三项
///
/// T013 只负责"文本 → 行盒"。几何参数里真正影响**断行**的只有版心宽度；
/// 字号与行高影响**度量**（字形位置与行盒高度）。其余参数（段缩进、
/// 中缝、单双页）属分页层（T019/T020），此处不引入以免过早耦合。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geometry {
    /// 版心宽度（px）
    pub measure_px: f32,
    /// 阅读字号（px）
    pub font_size_px: f32,
    /// 行高（字号倍数）
    pub line_height: f32,
    /// 对齐方式（FR-011：正文默认两端对齐）
    pub alignment: crate::rules::justify::Alignment,
}

impl Geometry {
    /// 默认几何（MASTER §5.1 的默认值）。
    #[must_use]
    pub const fn default_ma() -> Self {
        Self {
            measure_px: 36.0 * 19.0, // 36em @ 19px = 684px
            font_size_px: 19.0,
            line_height: 1.85,
            alignment: crate::rules::justify::Alignment::Justified,
        }
    }

    /// 行高（px）。
    #[must_use]
    pub fn line_height_px(&self) -> f32 {
        self.font_size_px * self.line_height
    }
}

/// 排版一段文本的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutResult {
    /// 行盒序列
    pub lines: Vec<LineBox>,
    /// 本段产生的全部禁则例外（供 SC-002 计数）
    pub exceptions: Vec<RuleExceptionRecord>,
    /// 因超长而退化为按字符断行的西文词数（FR-009 的诊断口径）
    ///
    /// 非零表示"该段的断行结果里有词被拆开"——那是 FR-009 允许的两种
    /// 情形之一（单词长于整行）。SC-003 的判据之一是"西文断裂只发生在
    /// FR-009 允许的两种情形"，本计数是那个判据的输入。
    pub degraded_words: usize,
}

/// 排版引擎（持有 `FontSystem`——它是昂贵的字体库句柄）。
///
/// ## 为什么持有而非每次新建
///
/// `FontSystem::new()` 会加载系统字体库（实测耗时可观）。分页预计算
/// （T024）要对多章反复排版，每次新建会成为瓶颈。故引擎是**长生命周期**
/// 对象，排版是它的方法。
pub struct LayoutEngine {
    font_system: FontSystem,
}

impl LayoutEngine {
    /// 新建引擎（加载系统字体库）。
    #[must_use]
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
        }
    }

    /// 排版一段文本。
    ///
    /// ## 流程（与 research R1 方案 A 逐条对应）
    ///
    /// 1. 建 `Buffer`（`Wrap::Word` + hinting 关闭 + 按版心宽设尺寸）
    /// 2. `line_shape` 取 `ShapeLine` **克隆体**（不改 Buffer 原文——
    ///    这一点重要：Buffer 可被复用，克隆体上的改动不应污染它）
    /// 3. 按禁则表**合并需要禁断的相邻 word**
    /// 4. 在合并后的克隆体上调 `layout()` 拿断行结果
    /// 5. 把 `LayoutLine` 的字形区间从**字节**换算为**字符**，组装 `LineBox`
    ///
    /// ## 第 5 步为什么必须做换算
    ///
    /// cosmic-text 的 `LayoutGlyph.start/end` 是**字节**区间，而本层的
    /// 输出契约是**字符**区间（FR-003）。换算在此发生一次，
    /// 之后全链路都是字符口径。
    pub fn layout_paragraph(
        &mut self,
        model: &TextModel,
        paragraph_index: usize,
        geometry: &Geometry,
    ) -> LayoutResult {
        let text = model.text();
        if text.is_empty() {
            return LayoutResult {
                lines: Vec::new(),
                exceptions: Vec::new(),
                degraded_words: 0,
            };
        }

        let metrics = Metrics::relative(geometry.font_size_px, geometry.line_height);

        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(Some(geometry.measure_px), None);
        // 约束 1：必须 Wrap::Word（见模块文档）
        buffer.set_wrap(Wrap::Word);
        // 约束 2：hinting 关闭
        buffer.set_hinting(Hinting::Disabled);
        buffer.set_text(
            text,
            &Attrs::new().family(Family::Serif),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        // `.cloned()` 而非 `.map(ShapeLine::clone)`——clippy 的
        // `map_clone` 认为后者是显式克隆。语义上等价，取惯用写法。
        let Some(shape_line) = buffer.line_shape(&mut self.font_system, 0).cloned() else {
            return LayoutResult {
                lines: Vec::new(),
                exceptions: Vec::new(),
                degraded_words: 0,
            };
        };

        // 第 3 步：按禁则合并（在克隆体上）
        let (mut merged, merge_plan) = merge_for_kinsoku(&shape_line, text);

        // 第 3.2 步：**超长西文单词的退化**（FR-009 / T015 硬序第 ③ 步）
        //
        // ## 为什么在 shape 层拆（与间隙同一理由）
        //
        // T005 spike 实测 `Wrap::Word` 对超宽词是**溢出**而非切分
        // （268px 的词在 60px 版心下产出 268px 的行）。而 `WordOrGlyph`
        // 的 glyph 路径无视 word 边界，会让禁则合并失效。
        //
        // 故唯一的办法是**自己把这些词的 word 拆开**——让 `Wrap::Word`
        // 自然把它们分到不同行。这必须在 layout 之前做，理由与间隙相同：
        // layout 之后改字身只能让行变宽，无法参与断行。
        let degraded_words = apply_overlong_word_degradation(&mut merged, text, geometry);

        // 第 3.5 步：**中西文最小间隙**（FR-009）
        //
        // ## 为什么必须在 layout 之前做（实测踩到过）
        //
        // 初版把间隙加在**layout 之后**的字形步进上，结果它只能让行变宽、
        // 永远无法让断行把间隙考虑进去。实测：228px 版心下的一行
        // `"Shadow of the Wind，作者"` layout 给出 228.00px（刚好填满），
        // 加上 4.75px 的 Latin→CJK 间隙后变成 232.75px → **SC-003 必破**。
        //
        // 改用"先预留一格"也是错的：那会无谓地惩罚纯中文行（少放一个字，
        // 实测留白 19px = 整字身，US1 场景 1 必破）。
        //
        // 正确做法是利用 T013 已验证的机制——**在 shape 层把间隙写进
        // `ShapeGlyph::x_advance`**（em 单位）。这样 layout 计算宽度时
        // 天然包含间隙，断行与间隙不再是两件事。
        // 与标点挤压共用同一个"字身改写"注入点（research R1 方案 A 的第 3 步）。
        apply_mixed_script_gaps_to_shape(&mut merged, text);

        // 第 4 步：layout
        let laid_out = merged.layout(
            geometry.font_size_px,
            Some(geometry.measure_px),
            Wrap::Word,
            None,
            None,
            Hinting::Disabled,
        );

        // 第 5 步：字节 → 字符换算 + 组装行盒
        let mut lines = Vec::with_capacity(laid_out.len());
        let mut exceptions = Vec::new();

        for (line_index, line) in laid_out.iter().enumerate() {
            let mut glyphs = Vec::with_capacity(line.glyphs.len());
            let mut char_start: Option<usize> = None;
            let mut char_end = 0usize;

            for glyph in &line.glyphs {
                let byte_range = glyph.start..glyph.end;
                // 字节区间 → 字符区间。两端的字节位置都是字符边界
                // （cosmic-text 的字形区间按字形切，必落在字符边界上）。
                let Some(glyph_char_start) = model.to_char_index(glyph.start) else {
                    continue;
                };
                let Some(glyph_char_end) = model.to_char_index(glyph.end) else {
                    continue;
                };

                char_start.get_or_insert(glyph_char_start);
                char_end = glyph_char_end.max(char_end);

                glyphs.push(GlyphBox {
                    char_range: glyph_char_start..glyph_char_end,
                    byte_range,
                    // `LayoutGlyph::x` 与 `w` **已经是像素单位**（layout.rs
                    // 的字段文档："X offset of hitbox" / "Width of hitbox"）。
                    // 初版误写成 `glyph.x * font_size_px`，把 px 又乘了一次字号
                    // ——实测 x 值达到 1900+ 而版心只有 228。
                    x_px: glyph.x,
                    advance_px: glyph.w,
                    squeeze_px: 0.0,
                });
            }

            let Some(char_start) = char_start else {
                continue;
            };

            // 第 5.5 步：**标点挤压**（FR-006 / T014）
            //
            // 溢出量为正时，把超出版心的部分压给行内可压缩标点。
            // 这一步必须在**行已确定**之后做——挤压不改变断点（那会把
            // 断行与字身的依赖变成循环），它只让已经排定的一行"收进版心"。
            let content_width: f32 = glyphs.iter().map(|glyph| glyph.advance_px).sum();
            let overflow_px = content_width - geometry.measure_px;
            let squeeze_px = apply_squeeze(&mut glyphs, text, overflow_px, geometry.font_size_px);

            // 第 5.6 步：**两端对齐**（FR-011 / T015）
            //
            // ## 为什么在挤压之后
            //
            // FR-008 的硬序是"① 挤压 → ……"；对齐不是硬序的一步，它是在
            // 硬序处理完"放不下"之后，处理"**放不满**"的手段。
            // 顺序反过来（先对齐再加挤压）会让两者互相抵消：
            // 挤压收回溢出的同时又撑开余白，结果既没省下空间也没对齐。
            //
            // ## 末行不伸展
            //
            // 段落的最后一行天然不满（段落结束了），把它伸展成满行
            // 反而显得刻意。故按"**本行是否段落的末行**"决定。
            //
            // ## 为什么按"行下标"而不是按"末字符位置"
            //
            // 初版写成 `laid_out.last()...glyph.end == text.len()`——
            // 那个条件在**每一轮迭代**里算出的都是同一个值（段落的末行
            // 是否到文末），于是所有行都被判为末行、**对齐永不生效**
            // （实测：22 行里 0 行被伸展）。
            //
            // 正确判据是"当前行是第几行"：末行即 `laid_out` 的最后一条。
            // 故必须用**迭代下标**而非字符位置。
            let is_last_line = line_index + 1 == laid_out.len();
            let justify_extra_px = apply_justify(
                &mut glyphs,
                text,
                geometry,
                is_last_line,
                geometry.alignment,
            );

            lines.push(LineBox {
                paragraph_index,
                char_range: char_start..char_end,
                glyphs,
                height_px: geometry.line_height_px(),
                baseline_px: line.max_ascent,
                squeeze_px,
                justify_extra_px,
                rule_exception: None,
            });
        }

        // 第 6 步：**事后核验**——layout 是否真的遵守了合并边界？
        //
        // 这是"判据必须能实际失败"的落点：若 layout 无视了合并（T005 的
        // 证伪条件），此处会产出 RuleExceptionRecord。它同时是 FR-008
        // 第 ④ 步的记账机制——"几何条件不允许时放弃禁则"与"引擎无视禁则"
        // 在这个口径下是同一件事，都必须留痕。
        exceptions.extend(verify_kinsoku_respected(&lines, text, &merge_plan));

        for line in &mut lines {
            if let Some(record) = exceptions
                .iter()
                .find(|record| line.char_range.contains(&record.char_index))
                .cloned()
            {
                line.rule_exception = Some(record);
            }
        }

        LayoutResult {
            lines,
            exceptions,
            degraded_words,
        }
    }
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// 一次合并操作的记录（供事后核验使用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePlanEntry {
    /// 合并后该 word 覆盖的字节区间
    pub byte_range: std::ops::Range<usize>,
    /// 被强制绑在一起的位置（字节）
    pub merged_at_byte: usize,
}

/// 按禁则表合并相邻 `ShapeWord`。
///
/// ## 算法
///
/// 逐 word 扫描：若「当前 word 的末字符」与「下一 word 的首字符」之间的
/// 断点被禁则禁止（[`break_decision`]），则把两者合并。
///
/// 合并可能**连锁**：`甲乙），` 里「乙」与「）」禁断、「）」与「，」也禁断，
/// 故三者要合成一个 word。实现用"累积待合并窗口"而不是两两合并——
/// 后者只能处理相邻一对（T005 的 `merge_words(from, to)` 天然支持区间，
/// 故这里传累积后的区间）。
///
/// ## 返回值
///
/// `(合并后的 ShapeLine, 合并记录)`。记录用于事后核验（见调用处）。
fn merge_for_kinsoku(shape_line: &ShapeLine, text: &str) -> (ShapeLine, Vec<MergePlanEntry>) {
    let mut merged = shape_line.clone();
    let mut plan = Vec::new();

    for span in &mut merged.spans {
        if span.words.len() < 2 {
            continue;
        }

        // 先算出"哪些相邻 word 之间需要禁断"，再一次性合并区间。
        // 分两步是为了让 `merge_words` 的区间参数能被正确累积。
        let mut merge_ranges: Vec<(usize, usize)> = Vec::new();
        let mut window_start: Option<usize> = None;

        for index in 0..span.words.len() - 1 {
            let left_char = last_char_of_word(&span.words[index], text);
            let right_char = first_char_of_word(&span.words[index + 1], text);

            let must_merge = match (left_char, right_char) {
                (Some(left), Some(right)) => !break_decision(left, right).is_allowed(),
                _ => false,
            };

            if must_merge {
                if window_start.is_none() {
                    window_start = Some(index);
                }
            } else if let Some(start) = window_start.take() {
                // 窗口是 [start, index]（闭区间，因为 index-1 与 index 需合并）
                merge_ranges.push((start, index + 1));
            }
        }
        if let Some(start) = window_start {
            merge_ranges.push((start, span.words.len()));
        }

        // 从后往前合并，避免索引位移
        for (from, to) in merge_ranges.into_iter().rev() {
            if to - from < 2 {
                continue;
            }
            let merged_at_byte = span.words[to - 1]
                .glyphs
                .first()
                .map_or(0, |glyph| glyph.start);
            merge_words(span, from, to);
            plan.push(MergePlanEntry {
                byte_range: span.words[from]
                    .glyphs
                    .first()
                    .zip(span.words[from].glyphs.last())
                    .map_or(0..0, |(first, last)| first.start..last.end),
                merged_at_byte,
            });
        }
    }

    (merged, plan)
}

/// 合并 `[from, to)` 区间的 `ShapeWord` 为一个。
///
/// ## 实现与 T005 spike 的 `merge_words` 一致
///
/// 把后一个 word 的 `glyphs` 移到前一个的尾部，再删掉后一个。
/// `ShapeGlyph` 的 `start`/`end` 是原文区间，移动后天然连续，无需重算。
/// 合并后的 `blank` 取"整段是否全空白"——取第一个的 `blank` 会错
/// （若第一个是空白而后面有实质字形，"空白 word 含字形"是非法状态）。
fn merge_words(span: &mut cosmic_text::ShapeSpan, from: usize, to: usize) {
    let mut merged_glyphs = Vec::new();
    for word in &span.words[from..to] {
        merged_glyphs.extend(word.glyphs.iter().cloned());
    }
    let merged_blank = span.words[from..to].iter().all(|word| word.blank);

    span.words[from] = ShapeWord {
        blank: merged_blank,
        glyphs: merged_glyphs,
    };
    for index in (from + 1..to).rev() {
        span.words.remove(index);
    }
}

/// 取一个 word 的**末字符**。
fn last_char_of_word(word: &ShapeWord, text: &str) -> Option<char> {
    let glyph = word.glyphs.last()?;
    text.get(glyph.start..glyph.end)?.chars().next_back()
}

/// 取一个 word 的**首字符**。
fn first_char_of_word(word: &ShapeWord, text: &str) -> Option<char> {
    let glyph = word.glyphs.first()?;
    text.get(glyph.start..glyph.end)?.chars().next()
}

/// 事后核验：每行的**首字符**与**尾字符**是否违反禁则？
///
/// ## 判据直接取自 SC-002 的原文
///
/// SC-002 说的是"行首禁则与行尾禁则的**违反次数为 0**"——故判据就是
/// **逐行查行首字符与行尾字符**，而不是我初版写的"查相邻两行的边界"。
///
/// 两种写法的差别在**段首/段尾**：
///
/// - 边界写法会漏掉"某行以闭标点开头，而它前面没有上一行"的情形
///   （段落本身以标点开头）——那**是**违反 SC-002 的，且是正当的
///   第 ④ 步情形（输入把标点放在章首，任何排版手段都无法把它移到别处）。
/// - 逐行写法覆盖它，因而让"例外记录"这条路径**真的可达**。
///
/// ## 为什么例外必须可达（而不是"永远记 0 条"）
///
/// 一个永远返回空例外表的核验逻辑与"禁则完美生效"在结果上无法区分——
/// 那正是 T-005/T-007/T-009 三次踩到的"全绿但没测到"。
/// 逐行写法让机制在真实输入下可见地触发（见 `exceptions_are_reachable`）。
///
/// ## 与 T013 的实现手段的关系
///
/// T013 的禁则靠**合并 word**实现，故断点几乎不会落在禁则位置——
/// 违规只可能来自"输入的边界条件"（段首就是标点、或单个合并单元本身
/// 超出版心而被迫独行）。这是 merge 路线的固有性质：它的失效模式是
/// **溢出**（SC-003 关切）而非**违规**（SC-002 关切）。
/// T015 的硬序前三步会进一步减少例外数量。
fn verify_kinsoku_respected(
    lines: &[LineBox],
    text: &str,
    _plan: &[MergePlanEntry],
) -> Vec<RuleExceptionRecord> {
    let mut exceptions = Vec::new();

    for line in lines {
        // 行首：首字符是否属行首禁则
        if let Some(first) = char_at_start_of_line(line, text) {
            if let BreakDecision::Forbidden { rule_id, .. } = break_decision('\0', first) {
                // 用 '\0' 作左邻（非任何禁则字符），只让**行首禁则**参与判定
                if rule_affects_line_start(rule_id) {
                    exceptions.push(RuleExceptionRecord {
                        char_index: line.char_range.start,
                        rule_id,
                        reached_step: HardOrderStep::GiveUpKinsoku,
                    });
                }
            }
        }

        // 行尾：末字符是否属行尾禁则
        if let Some(last) = char_at_end_of_line(line, text) {
            if let BreakDecision::Forbidden { rule_id, .. } = break_decision(last, '\0') {
                if rule_affects_line_end(rule_id) {
                    exceptions.push(RuleExceptionRecord {
                        char_index: line.char_range.end.saturating_sub(1),
                        rule_id,
                        reached_step: HardOrderStep::GiveUpKinsoku,
                    });
                }
            }
        }
    }

    exceptions
}

/// 该规则是否约束**行首**（用于区分"行首禁则"与"行尾禁则"的命中）。
fn rule_affects_line_start(rule_id: &str) -> bool {
    rule_id.starts_with("kinsoku.line_start.")
}

/// 该规则是否约束**行尾**。
fn rule_affects_line_end(rule_id: &str) -> bool {
    rule_id.starts_with("kinsoku.line_end.")
}

/// 对一行施加两端对齐（FR-011 / T015）。
///
/// ## 返回
///
/// 实际施加的伸展总量（px）。调用方把它写进 `LineBox::justify_extra_px`。
///
/// ## 与挤压的协同（FR-011 的括号要求）
///
/// 挤压（收）与对齐（放）都会改写字形的 `advance_px`，但**作用方向相反**：
/// 挤压只在行溢出时动作、对齐只在行不满时动作，故同一行不会两者都大动。
/// 这一点由"先挤压、后对齐"的顺序保证——挤压后若已填满，对齐的余白为 0
/// 自然不动作；若挤压后仍不满，对齐才补足。
fn apply_justify(
    glyphs: &mut [GlyphBox],
    text: &str,
    geometry: &Geometry,
    is_last_line: bool,
    alignment: crate::rules::justify::Alignment,
) -> f32 {
    use crate::rules::justify::{centering_offset_px, plan_justify, Alignment};

    match alignment {
        Alignment::Left => 0.0,
        Alignment::Justified => {
            // 两端对齐不作用于末行（段落结束了，撑满会显得刻意）
            let plan = plan_justify(
                glyphs,
                text,
                geometry.measure_px,
                geometry.font_size_px,
                is_last_line,
            );
            if !plan.stretched {
                return 0.0;
            }

            // ## 施加时的**硬上界**（实测踩到过）
            //
            // `plan_justify` 的 slack 是按调用时的 `advance_px` 求和算的，
            // 而本函数在 `apply_squeeze` **之后**执行——挤压已经改小了
            // 部分 advance。若两者失配（plan 用旧值、施加用新值），
            // 伸展量会超出实际余白，把行撑过版心。
            //
            // 实测：某行伸展后达 437px（版心 418px），超出正好一个字身。
            //
            // 故此处按**当前实际值**重算可伸展总量，并把每处伸展按比例
            // 钳到"不超过余白"：
            let content_now: f32 = glyphs.iter().map(|glyph| glyph.advance_px).sum();
            let slack_now = (geometry.measure_px - content_now).max(0.0);
            let planned_total: f32 = plan.extra_px.iter().sum();
            let scale = if planned_total > 1e-3 {
                (slack_now / planned_total).min(1.0)
            } else {
                0.0
            };

            let mut total = 0.0f32;
            let mut shift = 0.0f32;
            for (index, glyph) in glyphs.iter_mut().enumerate() {
                glyph.x_px += shift;
                let extra = plan.extra_px.get(index).copied().unwrap_or(0.0) * scale;
                if extra > 1e-4 {
                    glyph.advance_px += extra;
                    total += extra;
                    shift += extra;
                }
            }
            total
        }
        Alignment::Centered => {
            // 居中不改变字间距离（故不会"拉散"），只整体左移余白的一半
            let content_width: f32 = glyphs.iter().map(|glyph| glyph.advance_px).sum();
            let offset = centering_offset_px(content_width, geometry.measure_px);
            for glyph in glyphs.iter_mut() {
                glyph.x_px += offset;
            }
            // 居中不是"伸展"，故返回 0（余白在两侧，不在字间）
            0.0
        }
    }
}

/// 把超长西文单词**拆成多个 word**（FR-009 的退化实现 / 硬序第 ③ 步）。
///
/// ## 原理
///
/// `Wrap::Word` 只在 **word 边界**断行。故"让长词能被断"的手段就是
/// **把它的 word 切小**——切完后 `Wrap::Word` 会自然把各片段分到不同行。
/// 这与 T013 的禁则合并是**同一个机制的两个方向**：
///
/// | 目的 | 手段 |
/// |---|---|
/// | 禁止某处断行 | 把相邻 word **合并** |
/// | 允许某处断行（退化） | 把 word **切分** |
///
/// ## 必须切在字形的**标记**上而非任意位置
///
/// `ShapeWord` 是 `{ blank, glyphs }`，切分即在指定下标处把一个 word
/// 拆成两个（`glyphs` 的 `split_off`）。**不得**改动任何 `ShapeGlyph`
/// 本身——那样会破坏字符区间归属（001 §FR-028 的前提）。
///
/// ## 返回
///
/// 被拆分的词数（供诊断与测试断言）。
pub fn apply_overlong_word_degradation(
    shape_line: &mut ShapeLine,
    text: &str,
    geometry: &Geometry,
) -> usize {
    use crate::rules::mixed::{chars_per_line_for_measure, find_overlong_words};

    let overlong = find_overlong_words(text, geometry.measure_px, geometry.font_size_px);
    if overlong.is_empty() {
        return 0;
    }

    // **单位换算**：`find_overlong_words` 给的是**字符**区间（对外口径），
    // 而 `ShapeGlyph::start/end` 是**字节**偏移（cosmic-text 的口径）。
    // 比较两者前必须把字符起点换成字节——初版直接比较，匹配恒失败
    // （实测 `degraded_words = 0`，词被整词溢出）。
    let char_to_byte: Vec<usize> = {
        let mut offsets: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
        offsets.push(text.len());
        offsets
    };

    let chunk = chars_per_line_for_measure(geometry.measure_px, geometry.font_size_px);
    let mut degraded = 0usize;

    for span in &mut shape_line.spans {
        // 逐个处理超长词：找到覆盖它的 word，按 chunk 切成若干段。
        // 从后往前处理避免下标位移（与 merge_words 同样的理由）。
        for word in overlong.iter().rev() {
            let Some(expected_byte_start) = char_to_byte.get(word.char_range.start).copied() else {
                continue;
            };
            let Some(word_index) = span.words.iter().position(|candidate| {
                candidate
                    .glyphs
                    .first()
                    .is_some_and(|glyph| glyph.start == expected_byte_start)
            }) else {
                continue;
            };

            // 该 word 的字符数（不假定等于 glyphs.len()——组合簇会不等）
            let glyph_count = span.words[word_index].glyphs.len();
            if glyph_count <= chunk {
                continue;
            }

            // 从后往前按 chunk 切分：每次把尾部 chunk 个字形 split_off 出去
            let mut remaining = glyph_count;
            let mut inserted = 0usize;
            while remaining > chunk {
                let split_at = remaining - chunk;
                let tail = span.words[word_index].glyphs.split_off(split_at);
                span.words.insert(
                    word_index + 1,
                    ShapeWord {
                        // 片段不是空白词（它由字母组成）
                        blank: false,
                        glyphs: tail,
                    },
                );
                remaining = split_at;
                inserted += 1;
            }

            if inserted > 0 {
                degraded += 1;
            }
        }
    }

    degraded
}

/// 取一行**末尾**的字符。
fn char_at_end_of_line(line: &LineBox, text: &str) -> Option<char> {
    let glyph = line.glyphs.last()?;
    text.get(glyph.byte_range.clone())?.chars().next_back()
}

/// 取一行**开头**的字符。
fn char_at_start_of_line(line: &LineBox, text: &str) -> Option<char> {
    let glyph = line.glyphs.first()?;
    text.get(glyph.byte_range.clone())?.chars().next()
}

/// 对一行施加标点挤压（FR-006 / T014）。
///
/// ## 为什么在引擎里做而不在规则模块里
///
/// 挤压要改写 `GlyphBox::advance_px` 与 `x_px`——那是**行盒的内部状态**。
/// 规则模块（`rules::squeeze`）只**计算方案**（压哪个、压多少），
/// 施加由持有行盒的引擎负责。这个分工让规则模块可独立单测
/// （纯函数、无副作用），而引擎负责状态一致性。
///
/// ## 返回
///
/// 本行**实际**施加的压缩总量（px）。调用方把它写进
/// `LineBox::squeeze_px`——它与各字形的 `squeeze_px` 之和必须相等
/// （有测试断言，那是"明细与汇总一致"的判据）。
fn apply_squeeze(glyphs: &mut [GlyphBox], text: &str, overflow_px: f32, font_size_px: f32) -> f32 {
    if overflow_px <= 0.0 || glyphs.is_empty() {
        return 0.0;
    }

    let plan = crate::rules::squeeze::plan_squeeze(glyphs, text, overflow_px, font_size_px, true);

    let mut applied_total = 0.0f32;
    let mut shift = 0.0f32;

    for (glyph, decision) in glyphs.iter_mut().zip(plan.iter()) {
        if let Some(decision) = decision {
            let applied = decision.effective_px();
            glyph.advance_px -= applied;
            glyph.squeeze_px = applied;
            applied_total += applied;
            // 本字形起点也左移（前序累积的压缩量）
            glyph.x_px -= shift;
            shift += applied;
        } else {
            // 未压缩的字形仍要左移（受前面压缩的影响）
            glyph.x_px -= shift;
        }
    }

    applied_total
}

/// 把中西文最小间隙写入 shape 层的 `x_advance`（FR-009 / T014）。
///
/// ## 为什么在 shape 层（而不是 layout 之后）
///
/// 见调用处的说明：layout 之后加间隙只能让行变宽，无法参与断行。
/// 在 shape 层写入则让 layout 的宽度计算天然包含它——断行与间隙一致。
///
/// ## 判据：跨 word 边界检测
///
/// 间隙要加在"汉字与拉丁字母**直接相邻**"处。而 cosmic-text 会把它们
/// 切成**不同的 word**（中文逐字成 word、西文整词成 word），
/// 故判据是"相邻两个 word 的边界处，左末字与右首字属不同书写系统"。
///
/// ## 间隙记在**左侧字形**上
///
/// 不插入零宽字形：那会破坏"中文排版里字形数 == 字符数"这条性质，
/// 进而让字形区间与字符区间的对应变复杂（痕迹渲染依赖后者）。
fn apply_mixed_script_gaps_to_shape(shape_line: &mut ShapeLine, text: &str) {
    use crate::rules::mixed::{needs_minimum_gap, MIXED_SCRIPT_MIN_GAP_EM};

    for span in &mut shape_line.spans {
        if span.words.len() < 2 {
            continue;
        }

        // 先收集需要加间隙的 word 下标（避免边读边改的借用冲突）
        let gap_positions: Vec<usize> = (0..span.words.len() - 1)
            .filter(|index| {
                let left = span.words[*index]
                    .glyphs
                    .last()
                    .and_then(|glyph| text.get(glyph.start..glyph.end))
                    .and_then(|slice| slice.chars().next_back());
                let right = span.words[index + 1]
                    .glyphs
                    .first()
                    .and_then(|glyph| text.get(glyph.start..glyph.end))
                    .and_then(|slice| slice.chars().next());
                matches!((left, right), (Some(left), Some(right)) if needs_minimum_gap(left, right))
            })
            .collect();

        for index in gap_positions {
            // 加到该 word 的**末字形**上（间隙位于它与下一个 word 之间）
            if let Some(glyph) = span.words[index].glyphs.last_mut() {
                glyph.x_advance += MIXED_SCRIPT_MIN_GAP_EM;
            }
        }
    }
}

/// 取一行**开头**的字符。
#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> LayoutEngine {
        LayoutEngine::new()
    }

    #[test]
    fn empty_text_yields_no_lines() {
        let mut engine = engine();
        let model = TextModel::new("");
        let result = engine.layout_paragraph(&model, 0, &Geometry::default_ma());
        assert!(result.lines.is_empty());
        assert!(result.exceptions.is_empty());
    }

    #[test]
    fn short_text_yields_single_line() {
        let mut engine = engine();
        let model = TextModel::new("很短的一段");
        let result = engine.layout_paragraph(&model, 0, &Geometry::default_ma());

        assert_eq!(result.lines.len(), 1);
        assert_eq!(result.lines[0].char_range.start, 0);
        assert_eq!(result.lines[0].char_range.end, 5);
    }

    /// 字符区间必须是**字符**口径（不是字节）。
    #[test]
    fn char_ranges_are_char_based_not_byte_based() {
        let mut engine = engine();
        // "中文😀" = 3 字符、10 字节
        let model = TextModel::new("中文😀");
        let result = engine.layout_paragraph(&model, 0, &Geometry::default_ma());

        let line = &result.lines[0];
        assert_eq!(
            line.char_range.end, 3,
            "字符区间上界应为 3（字符数）而非 10（字节数）"
        );
    }

    /// 段落序号被如实记录。
    #[test]
    fn paragraph_index_is_recorded() {
        let mut engine = engine();
        let model = TextModel::new("内容");
        let result = engine.layout_paragraph(&model, 7, &Geometry::default_ma());
        assert_eq!(result.lines[0].paragraph_index, 7);
    }

    /// 行盒度量与几何参数一致。
    #[test]
    fn metrics_follow_geometry() {
        let mut engine = engine();
        let model = TextModel::new("测试");
        let geometry = Geometry {
            measure_px: 400.0,
            font_size_px: 19.0,
            line_height: 1.85,
            alignment: crate::rules::justify::Alignment::Justified,
        };
        let result = engine.layout_paragraph(&model, 0, &geometry);

        assert!((result.lines[0].height_px - 19.0 * 1.85).abs() < 1e-4);
        assert!(result.lines[0].baseline_px > 0.0, "基线应为正");
    }

    /// 窄版心产生多行，且行序与字符序一致（不丢不重）。
    #[test]
    fn narrow_measure_wraps_in_order() {
        let mut engine = engine();
        let text = "甲乙丙丁戊己庚辛壬癸";
        let model = TextModel::new(text);
        let geometry = Geometry {
            measure_px: 57.0, // 3 字身
            font_size_px: 19.0,
            line_height: 1.85,
            alignment: crate::rules::justify::Alignment::Justified,
        };
        let result = engine.layout_paragraph(&model, 0, &geometry);

        assert!(result.lines.len() >= 3, "应排成多行");

        let mut expected_start = 0usize;
        for line in &result.lines {
            assert_eq!(line.char_range.start, expected_start, "行序应连续");
            expected_start = line.char_range.end;
        }
        assert_eq!(expected_start, text.chars().count(), "末行应覆盖到文末");
    }

    /// 中文每字一个字形，字形区间与字符区间一一对应。
    #[test]
    fn cjk_glyphs_map_one_to_one_with_chars() {
        let mut engine = engine();
        let model = TextModel::new("甲乙丙");
        let result = engine.layout_paragraph(&model, 0, &Geometry::default_ma());

        let line = &result.lines[0];
        assert_eq!(line.glyphs.len(), 3, "三个汉字应有三个字形");
        for (index, glyph) in line.glyphs.iter().enumerate() {
            assert_eq!(glyph.char_range.start, index);
            assert_eq!(glyph.char_range.end, index + 1);
        }
    }
}
