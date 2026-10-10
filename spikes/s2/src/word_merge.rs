//! R5-1：ShapeWord 合并行为验证（T005，**证伪门**）
//!
//! ## 要验证的假设
//!
//! research.md R1 的方案 A：`Buffer::line_shape()` 取出 `ShapeLine` **克隆体**
//! → 在克隆体上**合并相邻 `ShapeWord`**（拼接 `glyphs`）→ 调 `layout()`。
//! 期望：**断点只在 word 边界**，故合并即禁止断行，从而在纯中文文本上实现
//! 行首/行尾禁则。
//!
//! 这个假设来自源码阅读推断（`ShapeLine`/`ShapeWord` 字段全 `pub` 且 `Clone`），
//! **未经实测**。S2-1 的整个 `layout/engine.rs` 都建在它上面，故它是
//! "die 早 die 便宜"的首要对象。
//!
//! ## 证伪的后果
//!
//! 若合并**不能**约束断点（例如 `layout()` 在 word 内部还会按 glyph 再切），
//! 则方案 A 不成立，必须走备选方案 C（自写 layout 层：harfrust + unicode-linebreak）。
//! tasks.md 明文：**证伪则停止并上报 PM，备选执行前须用户批准**。
//!
//! ## 本模块的组织
//!
//! | 测试 | 验证什么 |
//! |---|---|
//! | [`t005_word_merge_restricts_line_breaks`] | 合并后断点遵守合并边界（**主判据**） |
//! | [`t005_merge_does_not_lose_glyphs`] | 合并不丢字形、不改原文区间 |
//! | [`t005_merged_word_width_is_sum_of_parts`] | 合并后宽度是各部分之和（无隐形间隙） |
//! | [`t005_decorations_survive_merge`] | decorations 不被合并破坏 |
//! | [`t005_baseline_unmerged_splits_mid_word`] | **对照**：不合并时断点落在 word 内部 |
//!
//! 最后那条是**关键对照**：只有先证明"不合并会断在 word 内部"，"合并后不断"
//! 才有信息量。否则可能是"这个宽度下本来就不会断在那"，结论毫无价值。

use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, ShapeSpan, ShapeWord, Shaping, Wrap,
};

/// 验证用的正文字体名（系统字体；spike 不依赖内嵌子集，T008 才有产物）。
///
/// 中文需要一个含 CJK 字形的系统字体。Windows 上是 SimSun/微软雅黑，
/// macOS/Linux 走各自的回退——spike 只关心**断行行为**而非字形观感，
/// 故由 `fontdb` 自动回退即可，不硬编码某一字体族。
pub const TEST_FONT_SIZE: f32 = 19.0;
/// 行高（字号倍数）；与 `design-system/MASTER.md` §5.1 一致
pub const TEST_LINE_HEIGHT_SCALE: f32 = 1.85;

/// 取指定行的 `ShapeLine` **克隆体**。
///
/// ## 为什么需要这个包装（0.19 的 API 形态）
///
/// `Buffer::line_shape` 收 `&mut FontSystem` 且返回 `&ShapeLine`——对
/// `buffer` 是可变借用。而我们要在**克隆体**上改动、随后调 `layout()`
/// （它也只要 `&ShapeLine`），故借用必须在此结束。本函数把"借用 → 取克隆
/// → 归还"收在一处，避免每个调用点都写一遍借用作用域。
///
/// 注意同期的 `set_text`/`set_size` 等**不收** `font_system`——0.19 里只有
/// 需要 shape 的方法才借它（shape 是惰性的）。
pub fn shape_line_of(
    font_system: &mut FontSystem,
    buffer: &mut Buffer,
    line_index: usize,
) -> cosmic_text::ShapeLine {
    buffer
        .line_shape(font_system, line_index)
        .expect("指定行应有 shape 结果")
        .clone()
}

/// 构造一个已 shape 的 Buffer。
///
/// ## 为什么用 `set_text` 而非 `set_rich_text`
///
/// 本 spike 验证的是**断行机制**，与富文本无关。单属性文本能产出
/// 最少的 span，让 ShapeWord 的分布更可读（出现意外时容易定位）。
pub fn shaped_buffer(text: &str, width: f32) -> (FontSystem, Buffer) {
    let mut font_system = FontSystem::new();
    let metrics = Metrics::relative(TEST_FONT_SIZE, TEST_LINE_HEIGHT_SCALE);
    let mut buffer = Buffer::new(&mut font_system, metrics);

    buffer.set_size(Some(width), None);
    buffer.set_wrap(Wrap::Word);
    // hinting 关闭：research R1/R4 的共同决策（亚像素挤压量与三平台一致性）
    buffer.set_hinting(cosmic_text::Hinting::Disabled);
    buffer.set_text(
        text,
        &Attrs::new().family(Family::Serif),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);

    (font_system, buffer)
}

/// 从 clone 出来的 `ShapeLine` 里取全部 (span 索引, word 索引) → 文本区间。
///
/// 合并行为的判据需要知道"哪些 word 是哪些字"。
pub fn word_ranges(shape_line: &cosmic_text::ShapeLine) -> Vec<(usize, usize, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    for (span_index, span) in shape_line.spans.iter().enumerate() {
        for (word_index, word) in span.words.iter().enumerate() {
            let range = match (word.glyphs.first(), word.glyphs.last()) {
                (Some(first), Some(last)) => first.start..last.end,
                _ => 0..0,
            };
            out.push((span_index, word_index, range));
        }
    }
    out
}

/// 合并一个 span 内**相邻的** word（`[from, to)` 闭区间内的 word 合成一个）。
///
/// ## 这是方案 A 的注入点
///
/// `layout()` 只在 word 边界断行，故把两个 word 合并 = 禁止它俩之间断行。
/// S2-1 的禁则实现就是"把'行尾禁则字符之前的字'与'该字符'合并"这样一组操作。
///
/// 实现要点：合并 = 把后一个 word 的 `glyphs` **移动**到前一个的 `glyphs` 尾部，
/// 然后删掉后一个 word。`glyphs` 是 `Vec<ShapeGlyph>` 且 `ShapeGlyph` 有
/// `start`/`end` 原文区间——移动后区间天然连续，无需重算。
pub fn merge_words(span: &mut ShapeSpan, from: usize, to: usize) {
    assert!(from < to, "合并区间必须至少含两个 word");
    assert!(to <= span.words.len(), "合并区间越界");

    let mut merged_glyphs = Vec::new();
    for word in &span.words[from..to] {
        merged_glyphs.extend(word.glyphs.iter().cloned());
    }
    // 合并后的 blank 语义：整段是否全是空白。字面量上取"第一个 word 的 blank"
    // 会错——若第一个是空白而后面有实质字形，合并后就变成了"空白 word 含字形"。
    let merged_blank = span.words[from..to].iter().all(|word| word.blank);

    span.words[from] = ShapeWord {
        blank: merged_blank,
        glyphs: merged_glyphs,
    };
    // 从后往前删，避免索引位移
    for index in (from + 1..to).rev() {
        span.words.remove(index);
    }
}

/// 取 `layout()` 出的每一行覆盖的原文区间（由该行字形的 `start`/`end` 推出）。
pub fn line_ranges(lines: &[cosmic_text::LayoutLine]) -> Vec<std::ops::Range<usize>> {
    lines
        .iter()
        .filter_map(|line| {
            let first = line.glyphs.first()?;
            let last = line.glyphs.last()?;
            Some(first.start..last.end)
        })
        .collect()
}

// ===========================================================================
// T005 主判据
// ===========================================================================

/// **对照实验**：不合并时，断点确实落在 word 边界之间吗？
///
/// ## 这条为什么必须排在最前
///
/// 若"不合并也不断在 word 之间"，那么"合并后不断"就没有信息量——结论
/// 可能是"这个宽度下本来就不该断在那"。故先建立基准：找一个宽度，
/// 使**未合并**的排版在某个 word 边界处断行。有了它，"合并后不断在那"
/// 才是证据。
///
/// ## 探针实测的结构（决定本测试怎么写）
///
/// `probe_word_structure` 的输出（见 BASELINE.md §R5-1）显示：
///
/// - **中文**：`unicode-linebreak` 把每个汉字切成**独立 word**（"甲" "乙" "丙"…，
///   各 19px）。断行便发生在 word 之间——这正是中文排版需要的粒度。
/// - **长西文单词**：整词是**一个 word**（268.2px）。`Wrap::Word` 下它
///   **溢出容器而不在词内断开**（实测：60px 版心产出 268.2px 的行）。
///   故"长西文单词"**不能**用作"不合并会在 word 内部断裂"的基准。
///
/// 初版本测试正是用了长西文单词，结果失败——那不是实现有问题，是**测试的
/// 前提错了**（cosmic-text 在 `Wrap::Word` 下根本不切词）。此处据实测改正。
#[test]
fn t005_baseline_unmerged_breaks_between_words() {
    // 10 个汉字 = 10 个独立 word，各 19px；版心 57px = 3 字身
    let text = "甲乙丙丁戊己庚辛壬癸";
    let width = 57.0;
    let (mut font_system, mut buffer) = shaped_buffer(text, width);

    let shape_line = shape_line_of(&mut font_system, &mut buffer, 0);
    let words = word_ranges(&shape_line);

    // 前提自检：中文必须逐字成 word，否则后续以"每 3 字一行"为前提的断言无意义
    assert!(
        words.len() >= 10,
        "中文应逐字切成 word（实测 {} 个），否则本测试的前提不成立",
        words.len()
    );

    let lines = shape_line.layout(
        TEST_FONT_SIZE,
        Some(width),
        Wrap::Word,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    );
    let line_ranges_out = line_ranges(&lines);

    // 基准判据（实测值）：每行 3 字 → 断点落在字与字之间
    assert_eq!(
        line_ranges_out,
        vec![0..9, 9..18, 18..27, 27..30],
        "未合并的中文应以每 3 字断行（版心 57px / 字身 19px）——         若此断言失败，说明 word 切分粒度变了，后续合并判据需重新推导"
    );
}

/// **主判据**：合并相邻 word 后，断点遵守合并边界（该处不再断行）。
///
/// ## 判据形态的两处修正（据 T005 实测）
///
/// 1. **不用"word 内部断点计数"作为判据**。初版用长西文单词构造对照，但实测
///    表明 `Wrap::Word` 对超宽 word 是**溢出而非切分**——故"word 内部断点"
///    这个量在 `Wrap::Word` 下恒为 0，断言会**平凡通过**（假绿）。
/// 2. 改为**直接比较断点位置**：合并某两个相邻 word 后，它们之间的断点
///    必须消失，且合并后的组合出现在同一行里。这是可证伪的强判据。
#[test]
fn t005_word_merge_restricts_line_breaks() {
    let text = "甲乙丙丁戊己庚辛壬癸";
    let width = 57.0; // 3 字身
    let (mut font_system, mut buffer) = shaped_buffer(text, width);

    let original = shape_line_of(&mut font_system, &mut buffer, 0);
    assert!(
        word_ranges(&original).len() >= 4,
        "需要至少 4 个 word 才能做合并测试"
    );

    // --- 基准：未合并，每 3 字一行 ---
    let baseline = line_ranges(&original.layout(
        TEST_FONT_SIZE,
        Some(width),
        Wrap::Word,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    ));
    assert_eq!(
        baseline,
        vec![0..9, 9..18, 18..27, 27..30],
        "基准断行应为每 3 字（版心 57px / 字身 19px）"
    );

    // --- 合并 word[2..4)（第 3、4 个字「丙」「丁」）---
    //
    // 第 1 行本来是「甲乙丙」（word 0,1,2），断点在 word 2|3 之间。
    // 合并 word 2 与 3 后 {丙,丁} 成为不可分整体：第 1 行若仍放「甲乙丙」，
    // 则「丁」被留到下一行——但 {丙,丁} 不可分，故第 1 行只能放「甲乙」
    // （38px），{丙,丁}（38px）整体下移。**断点必须移动**，这是可证伪的判据。
    let mut merged = original.clone();
    for span in &mut merged.spans {
        if span.words.len() >= 4 {
            merge_words(span, 2, 4);
        }
    }

    let merged_words = word_ranges(&merged);
    let merged_lines = line_ranges(&merged.layout(
        TEST_FONT_SIZE,
        Some(width),
        Wrap::Word,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    ));

    // 判据一：合并后的 {丙丁} 必须落在**同一行**内
    let merged_pair_range = merged_words
        .iter()
        .find(|(_, _, range)| range.start == 6 && range.end == 12)
        .map(|(_, _, range)| range.clone())
        .expect("合并后应存在覆盖原文 6..12 的 word（丙丁）");

    let pair_in_single_line = merged_lines
        .iter()
        .any(|line| line.start <= merged_pair_range.start && line.end >= merged_pair_range.end);
    assert!(
        pair_in_single_line,
        "【证伪】合并后的 word「丙丁」（原文 {merged_pair_range:?}）跨越了行边界——\
         layout 未把 word 当硬约束。\n\
         合并后的行区间={merged_lines:?}\n\
         合并后的 word 区间={:?}\n\
         按 tasks.md T005：停止并上报 PM，不得自行开工备选方案 C。",
        merged_words
            .iter()
            .map(|(_, _, range)| range.clone())
            .collect::<Vec<_>>()
    );

    // 判据二：断点位置确实**变了**——若与基准相同，说明合并被 layout 无视
    assert_ne!(
        merged_lines, baseline,
        "合并相邻 word 后断点位置完全未变——合并未生效（方案 A 的注入点无效）"
    );

    // 判据三：合并后不得再有"行首落在「丁」上"（即基准里 9|18 那个断点处
    // 的相对位置）之外的分割——具体说，{丙丁} 的中间（byte 9）不得成为行首
    let breaks_inside_pair = merged_lines
        .iter()
        .skip(1)
        .any(|line| line.start > merged_pair_range.start && line.start < merged_pair_range.end);
    assert!(
        !breaks_inside_pair,
        "合并后仍有行首落在 {merged_pair_range:?} 内部（行区间={merged_lines:?}）"
    );
}

/// `Wrap::Glyph` 会切进 word 内部——**S2-1 必须避开的陷阱**。
///
/// ## 这条为什么属于 R5-1 的结论范围
///
/// 实测（见 BASELINE.md §R5-1）：
///
/// | `Wrap` | 长西文单词（268px）在 60px 版心下 |
/// |---|---|
/// | `Word` | 溢出成 268.2px 的行（**不切**） |
/// | `WordOrGlyph` | 退化为按字形断行 |
/// | `Glyph` | 无条件按字形断行 |
///
/// 这意味着：**禁则的后处理必须在 `Wrap::Word` 下做**。若 S2-1 用了
/// `WordOrGlyph`（看似"更宽容"），长西文单词会走 glyph 路径，而那条路径
/// **无视 word 边界**——合并 word 的禁则手段对它失效。
///
/// FR-009 要求的"西文整词优先、超长单词退化为按字符断行"，因此**不能**靠
/// `Wrap::WordOrGlyph` 直接获得；须由 S2-1 自己判定"该词是否长于整行"
/// 再决定用哪个 wrap（或自己按字形切）。本测试把这条约束钉成实测证据。
#[test]
fn t005_glyph_wrap_ignores_word_boundaries() {
    let long_word = "supercalifragilisticexpialidocious";
    let text = format!("前{long_word}后");
    let width = 60.0;
    let (mut font_system, mut buffer) = shaped_buffer(&text, width);

    let shape_line = shape_line_of(&mut font_system, &mut buffer, 0);
    let words = word_ranges(&shape_line);
    let long_word_range = words
        .iter()
        .find(|(_, _, range)| range.end - range.start > 20)
        .map(|(_, _, range)| range.clone())
        .expect("应存在覆盖长西文单词的 word");

    // Word 臂：该 word 必须整体落在一行（不切）
    let word_lines = line_ranges(&shape_line.layout(
        TEST_FONT_SIZE,
        Some(width),
        Wrap::Word,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    ));
    let intact_under_word = word_lines
        .iter()
        .any(|line| line.start <= long_word_range.start && line.end >= long_word_range.end);
    assert!(
        intact_under_word,
        "Wrap::Word 下长西文单词应整体成行（区间 {long_word_range:?}，行={word_lines:?}）"
    );

    // Glyph 臂：该 word 必须被切开（这是本测试要钉住的对照事实）
    let glyph_lines = line_ranges(&shape_line.layout(
        TEST_FONT_SIZE,
        Some(width),
        Wrap::Glyph,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    ));
    let split_under_glyph = glyph_lines
        .windows(2)
        .any(|pair| pair[1].start > long_word_range.start && pair[1].start < long_word_range.end);
    assert!(
        split_under_glyph,
        "Wrap::Glyph 应把长西文单词切开（否则 WordOrGlyph 的退化路径不存在，\
         FR-009 的「超长单词退化为按字符断行」需另找实现路径）；行={glyph_lines:?}"
    );
}

// ===========================================================================
// T005 辅助判据（合并行为不引入副作用）
// ===========================================================================

/// 合并不丢字形、不改原文区间覆盖。
#[test]
fn t005_merge_does_not_lose_glyphs() {
    let text = "中文与english混排的测试句子，用于验证合并行为。";
    let (mut font_system, mut buffer) = shaped_buffer(text, 200.0);

    let original = shape_line_of(&mut font_system, &mut buffer, 0);
    let before_glyphs: Vec<(usize, usize)> = original
        .spans
        .iter()
        .flat_map(|span| span.words.iter())
        .flat_map(|word| word.glyphs.iter())
        .map(|glyph| (glyph.start, glyph.end))
        .collect();

    let mut merged = shape_line_of(&mut font_system, &mut buffer, 0);
    for span in &mut merged.spans {
        if span.words.len() > 1 {
            merge_words(span, 0, span.words.len());
        }
    }
    let after_glyphs: Vec<(usize, usize)> = merged
        .spans
        .iter()
        .flat_map(|span| span.words.iter())
        .flat_map(|word| word.glyphs.iter())
        .map(|glyph| (glyph.start, glyph.end))
        .collect();

    assert_eq!(
        before_glyphs, after_glyphs,
        "合并改变了字形的原文区间序列——合并必须只重排归属、不改字形本身"
    );
}

/// 合并后宽度 = 各部分宽度之和（无隐形间隙）。
///
/// ## 为什么这条重要
///
/// 若合并引入了额外宽度，`layout()` 的断行结果会与"未合并但恰好放得下"不同，
/// 禁则实现就会出现"合并把行挤爆"的副作用。这条断言把该风险钉住。
#[test]
fn t005_merged_word_width_is_sum_of_parts() {
    let text = "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉戌亥";
    let (mut font_system, mut buffer) = shaped_buffer(text, 400.0);

    let original = shape_line_of(&mut font_system, &mut buffer, 0);
    let span_index = 0;

    let parts_width: f32 = original.spans[span_index]
        .words
        .iter()
        .map(|word| word.width(TEST_FONT_SIZE))
        .sum();

    let mut merged = original.clone();
    // 先把 word 数取出来，避免在可变借用的同一表达式里再读 `spans`
    let word_count = merged.spans[span_index].words.len();
    if word_count > 1 {
        merge_words(&mut merged.spans[span_index], 0, word_count);
    }
    let merged_width = merged.spans[span_index].words[0].width(TEST_FONT_SIZE);

    assert!(
        (merged_width - parts_width).abs() < 0.01,
        "合并后宽度 {merged_width} 与各部分之和 {parts_width} 不符（差 {}）",
        (merged_width - parts_width).abs()
    );
}

/// decorations 在合并后仍然存在且区间语义未破坏。
///
/// ## 为什么单列一条
///
/// research R5-1 明确点出"含 decorations/ellipsize 路径"。装饰（下划线、
/// 删除线）的区间记录在 `ShapeSpan::decoration_spans` 上——**与 words 同级**，
/// 故合并 words 理论上不动它。但要实测：若 `layout()` 依赖
/// "decoration range 与 word 边界的对应关系"，合并就会让装饰错位或消失。
#[test]
fn t005_decorations_survive_merge() {
    use cosmic_text::UnderlineStyle;

    let mut font_system = FontSystem::new();
    let metrics = Metrics::relative(TEST_FONT_SIZE, TEST_LINE_HEIGHT_SCALE);
    let mut buffer = Buffer::new(&mut font_system, metrics);
    buffer.set_size(Some(120.0), None);
    buffer.set_wrap(Wrap::Word);
    buffer.set_hinting(cosmic_text::Hinting::Disabled);

    // 0.19 的 `set_rich_text` 收 `(文本片段, Attrs)` 对——故此处把原文切成三段，
    // 中段带下划线。这恰好制造了"装饰区间与 word 边界不重合"的情形，
    // 正是本测试要观察的。
    let plain = "带下划线的";
    let decorated = "中文段落，用于";
    let tail = "验证装饰与合并的相互作用。";

    let base = Attrs::new().family(Family::Serif);
    let underlined = Attrs::new()
        .family(Family::Serif)
        .underline(UnderlineStyle::Single);
    buffer.set_rich_text(
        [
            (plain, base.clone()),
            (decorated, underlined),
            (tail, base.clone()),
        ],
        &base,
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);

    let original = shape_line_of(&mut font_system, &mut buffer, 0);
    let before_decorations: Vec<usize> = original
        .spans
        .iter()
        .map(|span| span.decoration_spans.len())
        .collect();

    let mut merged = original.clone();
    for span in &mut merged.spans {
        if span.words.len() > 1 {
            merge_words(span, 0, span.words.len());
        }
    }
    let after_decorations: Vec<usize> = merged
        .spans
        .iter()
        .map(|span| span.decoration_spans.len())
        .collect();

    assert_eq!(
        before_decorations, after_decorations,
        "合并 word 改变了 decoration_spans 的数量——装饰与 word 不是同级独立的"
    );

    // 装饰真的产出了（否则上面的断言是空对空）
    let total_original: usize = before_decorations.iter().sum();
    assert!(
        total_original > 0,
        "带 underline 属性的文本应产出 decoration_spans（实测为 0，说明该路径未被触发）"
    );

    // layout 后装饰仍反映到 LayoutLine
    let lines = merged.layout(
        TEST_FONT_SIZE,
        Some(120.0),
        Wrap::Word,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    );
    let layout_decorations: usize = lines.iter().map(|line| line.decorations.len()).sum();
    assert!(
        layout_decorations > 0,
        "合并后 layout 出的行丢失了全部装饰（ShapeLine 有 {total_original} 个装饰 span）"
    );
}

/// 合并后的文本覆盖完整性：逐行区间拼接等于原文（不丢不重）。
///
/// 这是 S2-1 的"分页不丢不重"（SC-006）在 word 层面的先行验证——
/// 若合并本身就会丢字，后面的分页断言无从谈起。
#[test]
fn t005_merged_layout_covers_all_text() {
    let text = "第一句。第二句，第三句；第四句：第五句？第六句！结束。";
    let width = 100.0;
    let (mut font_system, mut buffer) = shaped_buffer(text, width);

    let mut shape_line = shape_line_of(&mut font_system, &mut buffer, 0);
    for span in &mut shape_line.spans {
        if span.words.len() > 1 {
            merge_words(span, 0, span.words.len());
        }
    }

    let lines = shape_line.layout(
        TEST_FONT_SIZE,
        Some(width),
        Wrap::Word,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    );

    // 逐行取字形覆盖的字节区间，按顺序拼接应连续覆盖 0..text.len()
    let mut covered = Vec::new();
    for line in &lines {
        let Some(first) = line.glyphs.first() else {
            continue;
        };
        let Some(last) = line.glyphs.last() else {
            continue;
        };
        covered.push(first.start..last.end);
    }

    assert!(!covered.is_empty(), "layout 未产出任何行");

    // 相邻行必须首尾相接（允许中间有空白的跳过，但不得倒退或重复）
    for window in covered.windows(2) {
        assert!(
            window[1].start >= window[0].end || window[1].start == 0,
            "行区间出现重叠或倒退：{:?} 之后是 {:?}",
            window[0],
            window[1]
        );
    }

    let total_end = covered.last().map(|range| range.end).unwrap_or(0);
    assert_eq!(
        total_end,
        text.len(),
        "合并后 layout 未覆盖到原文末尾（覆盖 {total_covered}/{total_len} 字节）",
        total_len = text.len(),
        total_covered = total_end
    );
}

// ===========================================================================
// 诊断探针（**临时**：用于判定对照实验的真实行为，结论写入 BASELINE.md 后移除）
// ===========================================================================

#[test]
#[ignore = "诊断探针：输出 word 切分与断点详情"]
fn probe_word_structure() {
    for (label, text, width) in [
        (
            "纯中文-窄",
            "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉戌亥",
            60.0,
        ),
        ("中文+长西文", "前supercalifragilisticexpialidocious", 60.0),
        ("纯长西文", "supercalifragilisticexpialidocious", 60.0),
        ("中文标点", "测试标点，压缩。", 60.0),
        ("中西混排", "中文english混排test", 80.0),
    ] {
        let (mut font_system, mut buffer) = shaped_buffer(text, width);
        let shape_line = shape_line_of(&mut font_system, &mut buffer, 0);
        let words = word_ranges(&shape_line);
        let lines = shape_line.layout(
            TEST_FONT_SIZE,
            Some(width),
            Wrap::Word,
            None,
            None,
            cosmic_text::Hinting::Disabled,
        );
        let line_ranges = line_ranges(&lines);
        println!("--- {label} (width={width}) ---");
        println!("  text bytes = {}", text.len());
        println!("  words = {words:?}");
        for (i, word) in shape_line
            .spans
            .iter()
            .flat_map(|s| s.words.iter())
            .enumerate()
        {
            let seg: String = word
                .glyphs
                .iter()
                .map(|g| &text[g.start.min(text.len())..g.end.min(text.len())])
                .collect();
            println!(
                "    word[{i}] blank={} w={:.1} text={seg:?}",
                word.blank,
                word.width(TEST_FONT_SIZE)
            );
        }
        println!("  lines = {line_ranges:?}");
        for (i, line) in lines.iter().enumerate() {
            let seg: String = line
                .glyphs
                .iter()
                .map(|g| &text[g.start.min(text.len())..g.end.min(text.len())])
                .collect();
            println!("    line[{i}] w={:.1} text={seg:?}", line.w);
        }
    }
}

#[test]
#[ignore = "诊断探针：验证合并真的改变了断点位置"]
fn probe_merge_shifts_break() {
    // 纯中文：每字一个 word，版心 3 字 → 未合并时断点应在每 3 字
    let text = "甲乙丙丁戊己庚辛壬癸";
    let width = 57.0; // 3 × 19
    let (mut font_system, mut buffer) = shaped_buffer(text, width);

    let original = shape_line_of(&mut font_system, &mut buffer, 0);
    let lines = original.layout(
        TEST_FONT_SIZE,
        Some(width),
        Wrap::Word,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    );
    println!("未合并 lines = {:?}", line_ranges(&lines));

    // 合并 word[2..4]（第 3、4 个字）→ 断点不得落在它们之间
    let mut merged = original.clone();
    for span in &mut merged.spans {
        if span.words.len() >= 4 {
            merge_words(span, 2, 4);
        }
    }
    let lines2 = merged.layout(
        TEST_FONT_SIZE,
        Some(width),
        Wrap::Word,
        None,
        None,
        cosmic_text::Hinting::Disabled,
    );
    println!("合并 [2,4) 后 lines = {:?}", line_ranges(&lines2));
    println!(
        "  合并后 word 区间 = {:?}",
        word_ranges(&merged)
            .iter()
            .map(|(_, _, r)| r.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
#[ignore = "诊断探针：WordOrGlyph 是否在超长单词时退化"]
fn probe_word_or_glyph() {
    for wrap in [Wrap::Word, Wrap::WordOrGlyph, Wrap::Glyph] {
        let text = "前supercalifragilisticexpialidocious后";
        let width = 60.0;
        let (mut font_system, mut buffer) = shaped_buffer(text, width);
        let line = shape_line_of(&mut font_system, &mut buffer, 0);
        let lines = line.layout(
            TEST_FONT_SIZE,
            Some(width),
            wrap,
            None,
            None,
            cosmic_text::Hinting::Disabled,
        );
        println!(
            "{wrap:?} → {} 行，区间 {:?}，各行宽 {:?}",
            lines.len(),
            line_ranges(&lines),
            lines
                .iter()
                .map(|l| (l.w * 10.0).round() / 10.0)
                .collect::<Vec<_>>()
        );
    }
}
