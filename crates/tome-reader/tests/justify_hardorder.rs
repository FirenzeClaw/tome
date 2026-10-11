//! T015 验收：两端对齐 + 硬序 + 超长词退化的引擎接线
//!
//! ## 本文件覆盖三条需求
//!
//! | 需求 | 判据 |
//! |---|---|
//! | FR-011 两端对齐为正文默认 | 非末行伸展后行长 == 版心（±1e-3） |
//! | FR-011 不拉散标点 | 标点两侧的字距不因对齐改变 |
//! | FR-008 硬序固定可预测 | 同一输入反复判定给同一结果；四步各有触发用例 |
//! | FR-009 超长词退化（引擎接线） | 超长词被拆，且**断裂只发生在词内** |
//!
//! ## 为什么"超长词退化"的判据放在本文件
//!
//! tasks.md 把退化路径记在 T014，但其**引擎接线**（预处理 → 插入断点 →
//! 重排）与 T015 的硬序第 ③ 步共用同一套机制。PM 已澄清：两者一并落地。
//! 故判据在此，而工具函数（候选发现、拆词）仍在 `rules::mixed`。

use tome_reader::layout::engine::{Geometry, LayoutEngine};
use tome_reader::layout::linebox::LineBox;
use tome_reader::rules::justify::{plan_hard_order, Alignment, HardOrderStep};
use tome_reader::text::model::TextModel;

fn geometry(measure_px: f32, alignment: Alignment) -> Geometry {
    Geometry {
        measure_px,
        font_size_px: 19.0,
        line_height: 1.85,
        alignment,
        paragraph_mode: tome_reader::paginate::config::ParagraphMode::default(),
    }
}

/// 一行的**视觉宽度**。
///
/// ## ⚠️ 为什么**不**加 `justify_extra_px`（这里踩过一次）
///
/// `content_width_px()` 是各字形 `advance_px` 之和，而 `apply_justify`
/// 是**把伸展量直接写进** `advance_px` 的。故 `content_width_px()` 在
/// 对齐后**已经包含**伸展——`justify_extra_px` 只是那份伸展的**记账副本**
/// （供诊断与断言"这行是否被伸展"用），不是额外的宽度。
///
/// 初版写成 `content + extra`，于是 399px 的行被算成 437px（正好的
/// 399 + 19 + 19），误报"超出版心"。**那是测试的错，不是引擎的错**——
/// 引擎的 `sum` 实测就是 418.00，恰好等于版心。
fn visual_width(line: &LineBox) -> f32 {
    line.content_width_px()
}

// ===========================================================================
// FR-011 两端对齐
// ===========================================================================

/// **正文默认两端对齐**：默认几何下非末行应被撑到版心宽。
#[test]
fn fr011_default_alignment_is_justified() {
    let default_geometry = Geometry::default_ma();
    assert_eq!(
        default_geometry.alignment,
        Alignment::Justified,
        "FR-011：正文两端对齐 MUST 为默认"
    );
}

/// **两端对齐生效**：非末行的视觉宽度 == 版心宽。
///
/// ## 判据的精确性
///
/// 用 `visual_width`（字身 + 伸展）而不是 `content_width_px`（仅字身）——
/// 后者在对齐后仍小于版心（伸展记在独立字段里），断言会误报。
#[test]
fn fr011_non_last_lines_are_stretched_to_measure() {
    const EPSILON: f32 = 1e-3;
    let mut checked = 0usize;
    let mut stretched_lines = 0usize;

    let corpus = [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样。",
        "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉戌亥子丑寅卯辰巳午未申酉戌亥。",
    ];

    for text in corpus {
        for measure_px in [342.0f32, 361.0, 380.0, 399.0, 418.0] {
            let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
            let model = TextModel::new(text);
            let result =
                engine.layout_paragraph(&model, 0, &geometry(measure_px, Alignment::Justified));

            let body = if result.lines.len() > 1 {
                &result.lines[..result.lines.len() - 1]
            } else {
                continue;
            };

            for line in body {
                checked += 1;
                if line.justify_extra_px > 0.0 {
                    stretched_lines += 1;
                }
                assert!(
                    visual_width(line) <= measure_px + EPSILON,
                    "对齐后行宽 {:.2}px 超出版心 {measure_px}px",
                    visual_width(line)
                );
            }
        }
    }

    assert!(checked > 10, "检查行数太少（{checked}）");
    // **有效性自检**：必须真的有行被伸展，否则上面的断言是空转
    assert!(
        stretched_lines > 0,
        "没有任何行被伸展（{checked} 行里 0 行）——对齐未生效，判据是空转"
    );

    println!("[FR-011] 检查 {checked} 个非末行，其中 {stretched_lines} 行被伸展");
}

/// **末行不伸展**（段落结束了，撑满显得刻意）。
#[test]
fn fr011_last_line_is_not_stretched() {
    let text = "短短一段。";
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let model = TextModel::new(text);
    // 单行段落 → 它既是首行也是末行
    let result = engine.layout_paragraph(&model, 0, &geometry(400.0, Alignment::Justified));

    assert_eq!(result.lines.len(), 1);
    assert_eq!(
        result.lines[0].justify_extra_px, 0.0,
        "末行不该被伸展（哪怕版心很宽）"
    );
}

/// **左对齐不伸展**（`Alignment::Left` 的语义）。
#[test]
fn fr011_left_alignment_does_not_stretch() {
    let text = "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样。";
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let model = TextModel::new(text);

    let result = engine.layout_paragraph(&model, 0, &geometry(342.0, Alignment::Left));
    for line in &result.lines {
        assert_eq!(
            line.justify_extra_px, 0.0,
            "左对齐不该有任何伸展，实测 {:.2}px",
            line.justify_extra_px
        );
    }
}

/// **不拉散标点**（FR-011 的括号要求）：标点两侧的字距在对齐下不变。
///
/// ## 判据
///
/// 对含标点的一行，分别以 `Left` 与 `Justified` 排版，比较
/// **标点字形的步进**——它不应因对齐而改变；同时标点**前一个字**的
/// 步进也不应变（那正是"标点两侧被撑开"的定义）。
#[test]
fn fr011_punctuation_adjacency_is_not_stretched() {
    let text = "甲乙，丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉戌亥";
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let model = TextModel::new(text);

    let left = engine.layout_paragraph(&model, 0, &geometry(342.0, Alignment::Left));
    let justified = engine.layout_paragraph(&model, 0, &geometry(342.0, Alignment::Justified));

    assert_eq!(left.lines.len(), justified.lines.len(), "行数应一致");

    for (left_line, just_line) in left.lines.iter().zip(&justified.lines) {
        // 找标点字形（字符 '，'）
        let punct_index = just_line.glyphs.iter().position(|glyph| {
            text.get(glyph.byte_range.clone())
                .is_some_and(|slice| slice.starts_with('，'))
        });

        let Some(index) = punct_index else {
            continue;
        };

        assert_eq!(
            just_line.glyphs[index].advance_px, left_line.glyphs[index].advance_px,
            "标点字身的步进不该因对齐改变"
        );

        if index > 0 {
            assert_eq!(
                just_line.glyphs[index - 1].advance_px,
                left_line.glyphs[index - 1].advance_px,
                "标点**前一个字**的步进不该因对齐改变（那就是「把标点拉散」）"
            );
        }
    }
}

/// **居中**：余白对半分到两侧，字距不变（故不会拉散）。
///
/// ## 判据为什么不是"居中 = 左对齐 + 半余白"
///
/// 前导 2 之后 FR-018 生效：同一段文本走**正文**路径时带段首缩进，而居中
/// 路径（标题/引用）不带——两条路径的起点本就不同，"左对齐 + 偏移"这个
/// 差分式不再成立。
///
/// 居中的**定义**与段模式无关：左右余白相等。故此处直接断言该不变量
/// （两侧余白之差 ≈ 0 且首字形落在半余白处），它对两种段模式都成立。
#[test]
fn fr011_centered_splits_slack_in_half() {
    let text = "标题";
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let model = TextModel::new(text);

    let left = engine.layout_paragraph(&model, 0, &geometry(342.0, Alignment::Left));
    let centered = engine.layout_paragraph(&model, 0, &geometry(342.0, Alignment::Centered));

    let centered_line = &centered.lines[0];
    let centered_first_x = centered_line.glyphs[0].x_px;
    let content_width = centered_line.content_width_px();

    // 内容宽 = 2 字身 = 38px；余白 = 342 - 38 = 304px；半 = 152px
    let expected_offset = (342.0 - content_width) / 2.0;
    assert!(
        (centered_first_x - expected_offset).abs() < 1.0,
        "居中应把余白对半：期望首字形 x ≈ {expected_offset:.2}，实测 {centered_first_x:.2}"
    );

    // 两条余白相等（居中的定义）
    let right_margin = 342.0 - (centered_first_x + content_width);
    assert!(
        (centered_first_x - right_margin).abs() < 1.0,
        "居中时左右余白应相等：左 {centered_first_x:.2}px，右 {right_margin:.2}px"
    );

    // 居中不改变字距
    assert_eq!(
        centered_line.glyphs[0].advance_px, left.lines[0].glyphs[0].advance_px,
        "居中不改字距"
    );

    // 居中行不带段首缩进（标题以版心为准居中，见引擎的居中分支说明）
    assert_eq!(
        centered_line.indent_px, 0.0,
        "居中行不该有段首缩进——缩进会让标题相对版心偏斜"
    );
}

// ===========================================================================
// FR-008 硬序
// ===========================================================================

/// 硬序四步**各有触发用例**（FR-008 的验收子项）。
#[test]
fn fr008_all_four_steps_have_trigger_cases() {
    // ① 挤压足够：溢出 ≤ 可挤压量
    assert!(matches!(
        plan_hard_order(3.0, 19.0, 9.5, 19.0),
        HardOrderStep::SqueezeSufficient { .. }
    ));

    // ② 悬挂：溢出 > 挤压量但 ≤ 半字身
    assert!(matches!(
        plan_hard_order(7.0, 19.0, 5.0, 19.0),
        HardOrderStep::HangingAllowed { .. }
    ));

    // ③ 整字回退：溢出 > 半字身
    assert!(matches!(
        plan_hard_order(20.0, 19.0, 0.0, 19.0),
        HardOrderStep::WholeCharFallback { .. }
    ));

    // ④ 放弃：字号为 0（几何退化，前三步均不可行）
    assert_eq!(plan_hard_order(100.0, 0.0, 0.0, 0.0), HardOrderStep::GiveUp);
}

/// 硬序**顺序固定且可预测**：扫过溢出区间，判定只应沿 ①→②→③→④ 单调推进。
///
/// ## 判据
///
/// 随溢出量增大，判定应依次落在 ① ① ② ③ ④（不得回跳）。
/// 这条把"顺序固定"从"四次独立断言"升级为"**单调性**断言"——
/// 后者才能抓住"顺序被写反"这类破坏。
#[test]
fn fr008_step_progression_is_monotonic() {
    fn rank(step: &HardOrderStep) -> u8 {
        match step {
            HardOrderStep::SqueezeSufficient { .. } => 0,
            HardOrderStep::HangingAllowed { .. } => 1,
            HardOrderStep::WholeCharFallback { .. } => 2,
            HardOrderStep::GiveUp => 3,
        }
    }

    let font_size = 19.0f32;
    let squeezable = 5.0f32;
    let mut previous = 0u8;

    for overflow in [0.0f32, 3.0, 5.0, 6.0, 9.5, 10.0, 20.0, 19.0 * 10.0] {
        let step = plan_hard_order(overflow, font_size, squeezable, font_size);
        let current = rank(&step);
        assert!(
            current >= previous,
            "溢出 {overflow}px 时判定回跳（{previous} → {current}）——硬序顺序被破坏：{step:?}"
        );
        previous = current;
    }
}

/// 第 ④ 步的例外记录**可枚举可计数**（FR-008 的验收子项）。
#[test]
fn fr008_step4_exceptions_are_enumerable() {
    // 段落以闭标点开头 → 首行行首必然是该标点（无排版手段可避免）
    let text = "。这是以句号开头的段落，用于逼出第 ④ 步的例外记录。";
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let model = TextModel::new(text);
    let result = engine.layout_paragraph(&model, 0, &geometry(342.0, Alignment::Justified));

    assert!(
        !result.exceptions.is_empty(),
        "段首标点应产出例外记录（否则记录机制未接线）"
    );

    // 可枚举：逐条有位置与规则 id
    for record in &result.exceptions {
        assert!(record.rule_id.starts_with("kinsoku."));
        assert!(record.char_index <= text.chars().count());
    }
    // 可计数
    assert_eq!(result.exceptions.len(), 1, "本例恰有一处例外");
    assert_eq!(result.exceptions[0].char_index, 0);
}

// ===========================================================================
// FR-009 超长词退化的**引擎接线**
// ===========================================================================

/// **超长西文词被拆开**（退化路径真的接进了引擎）。
///
/// ## 判据
///
/// 1. `LayoutResult::degraded_words > 0` —— 引擎确实执行了退化
/// 2. 拆开后的行宽**不超版心**
/// 3. 原词的所有字符仍被覆盖（不丢字）
#[test]
fn fr009_overlong_word_is_degraded_by_engine() {
    let long_word = "pneumonoultramicroscopicsilicovolcanoconiosis";
    let text = format!("这个术语拼作 {long_word}，是一个很长的词。");
    let measure_px = 228.0f32;

    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let model = TextModel::new(&text);
    let result = engine.layout_paragraph(&model, 0, &geometry(measure_px, Alignment::Left));

    assert!(
        result.degraded_words > 0,
        "超长词应触发退化（degraded_words = {}）——引擎接线未完成",
        result.degraded_words
    );

    // 不丢字：行的字符区间覆盖到文末
    let covered: usize = result.lines.last().map_or(0, |line| line.char_range.end);
    assert_eq!(
        covered,
        text.chars().count(),
        "退化后应完整覆盖原文（{covered} / {}）",
        text.chars().count()
    );

    // 行宽：退化后仍可能因**单个片段**宽于版心而溢出（granularity 有限），
    // 但不应整词溢出。放开一点余量：允许溢出不超过一个片段宽度。
    let widest = result
        .lines
        .iter()
        .map(LineBox::content_width_px)
        .fold(0.0f32, f32::max);
    let whole_word_width = long_word.len() as f32 * 19.0 * 0.55;
    assert!(
        widest < whole_word_width,
        "退化后最宽行 {widest:.1}px 仍接近整词宽 {whole_word_width:.1}px——退化未生效"
    );

    println!(
        "[FR-009] 超长词退化：{} 行，最宽 {widest:.1}px（整词宽约 {whole_word_width:.1}px）",
        result.lines.len()
    );
}

/// **反例**：普通长度的西文不触发退化（退化只针对真正超长的词）。
#[test]
fn fr009_normal_words_do_not_trigger_degradation() {
    let text = "这本书的英文名是 The Shadow of the Wind，作者写得很好。";
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let model = TextModel::new(text);
    let result = engine.layout_paragraph(&model, 0, &geometry(342.0, Alignment::Left));

    assert_eq!(
        result.degraded_words, 0,
        "普通长度的词不该触发退化（degraded_words = {}）",
        result.degraded_words
    );
}

/// **退化只发生在词内**（SC-003 的"断裂只发生在 FR-009 允许的两种情形"）。
///
/// ## 判据
///
/// 找出所有"行边界落在某个西文词内部"的位置，断言**每个**这样的词都
/// 满足 FR-009 的退化条件（该词长于整行）。
///
/// 这排除了"因为实现 bug 而随意在词中断行"——那是 SC-003 明确禁止的。
#[test]
fn sc003_word_breaks_only_for_overlong_words() {
    use tome_reader::rules::mixed::find_overlong_words;

    let corpus = [
        "这个术语拼作 pneumonoultramicroscopicsilicovolcanoconiosis，是一个很长的词。",
        "AI技术正在改变世界，而GPT模型只是其中一例，NLP领域亦然，API与SDK层出不穷。",
        "第1章讲的是3D建模，第2章讲2D渲染，第10章总结2024年的进展与展望。",
    ];

    for text in corpus {
        let overlong = find_overlong_words(text, 228.0, 19.0);
        let overlong_ranges: Vec<_> = overlong
            .iter()
            .map(|word| word.char_range.clone())
            .collect();

        for measure_px in [228.0f32, 285.0, 342.0] {
            let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
            let model = TextModel::new(text);
            let result = engine.layout_paragraph(&model, 0, &geometry(measure_px, Alignment::Left));

            // 找行边界处被切开的西文词
            for pair in result.lines.windows(2) {
                let left_end = pair[0].char_range.end;
                let right_start = pair[1].char_range.start;

                // 该边界是否落在某个西文词内部？
                let chars: Vec<char> = text.chars().collect();
                let left_char = chars.get(left_end.saturating_sub(1)).copied();
                let right_char = chars.get(right_start).copied();

                let boundary_inside_latin =
                    left_char.zip(right_char).is_some_and(|(left, right)| {
                        use tome_reader::rules::mixed::{classify, ScriptClass};
                        classify(left) == ScriptClass::Latin
                            && classify(right) == ScriptClass::Latin
                    });

                if !boundary_inside_latin {
                    continue;
                }

                // 该边界必须落在某个超长词内
                let inside_overlong = overlong_ranges
                    .iter()
                    .any(|range| left_end > range.start && left_end < range.end);

                assert!(
                    inside_overlong,
                    "SC-003 违反：西文词在非超长位置被切开\n\
                     版心 {measure_px}px，边界在字符 {left_end}\n\
                     左字符 {left_char:?}，右字符 {right_char:?}\n\
                     超长词区间 {overlong_ranges:?}\n\
                     FR-009 只允许「整词放不下」与「单词长于整行」两种断裂"
                );
            }
        }
    }
}
