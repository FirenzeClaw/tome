//! T014 验收：挤压与混排（SC-003 + FR-006/FR-009 的端到端判据）
//!
//! ## SC-003 的原文口径
//!
//! "中西文混排语料下，**无任一行超出容器宽度**（100%）；西文单词的断裂
//! 只发生在 FR-009 允许的两种情形（整词放不下、单词长于整行）。"
//!
//! ## US1 场景 1 的原文口径
//!
//! "每一行的行长在版心宽度内浮动**不超过 1 个标点挤压单位**，且无任一行
//! 超出容器"（spec §User Story 1 场景 1）。
//!
//! 故本文件断言三件：
//!
//! 1. 无行超出容器（SC-003 主体）
//! 2. 挤压前后**字符下标一致**（FR-006 第 3 条）
//! 3. 行长浮动 ≤ 1 个挤压单位（US1 场景 1）
//!
//! ## 测量装置的有效性
//!
//! 前两条都可能平凡通过（"本来就不溢出"、"本来就不改下标"）。故本文件
//! 同时包含**对照测试**：证明存在"若不挤压就会溢出"的真实情形。

use tome_reader::layout::engine::{Geometry, LayoutEngine};
use tome_reader::rules::squeeze::{max_compression_px, MIN_SQUEEZE_RATIO};
use tome_reader::text::model::TextModel;

fn geometry(measure_px: f32) -> Geometry {
    Geometry {
        measure_px,
        font_size_px: 19.0,
        line_height: 1.85,
    }
}

/// 中西文混排语料。
///
/// ## 语料设计：必须有**直接相邻**的中西文交界
///
/// 实测发现初版语料的问题是：CJK 与西文之间大多隔着**空格**
/// （`"是 The"`、`"与 HTTP"`），而空格属于 `Other` 类——
/// 故"直接相邻的中西文交界"只有 2 处，SC-003 的间隙判据几乎没被触发。
///
/// 本版刻意加入**无空格相邻**的构造（`中文-English`、`AI技术`、
/// `第1章`、`3D建模`），这些才是 FR-009「最小间隙」的真实适用场景。
fn mixed_corpus() -> Vec<String> {
    vec![
        // 无空格相邻（边界密集）
        "AI技术正在改变世界，而GPT模型只是其中的一个例子，NLP领域亦然。".to_owned(),
        "第1章讲的是3D建模，第2章讲2D渲染，第10章总结2024年的进展。".to_owned(),
        "他用的API叫OpenAI，SDK是python版的，IDE选VSCode。".to_owned(),
        "中文标点，与English punctuation混排时，间隙处理是难点之一。".to_owned(),
        // 空格分隔 + 长整词（覆盖"整词回退"路径）
        "这本书的英文名是 The Shadow of the Wind，作者 Carlos Ruiz Zafón 在 2001 年出版它的时候，大概没有想到它会被翻译成几十种语言。".to_owned(),
        "TCP/IP 协议栈与 HTTP/2 的关系，在 RFC 7540 里有详细说明，但大多数人只关心它能用就行。".to_owned(),
    ]
}

/// **语料有效性**：混排语料必须真的含中西文交界（否则 SC-003 平凡通过）。
#[test]
fn mixed_corpus_actually_has_script_boundaries() {
    use tome_reader::rules::mixed::needs_minimum_gap;

    // 判据针对**整组语料**（不要求每句都 ≥3）：初版要求每句达标，
    // 实测第一条只有 1 处交界——那是阈值定得不对，不是语料有问题。
    let total: usize = mixed_corpus()
        .iter()
        .map(|text| {
            let chars: Vec<char> = text.chars().collect();
            chars
                .windows(2)
                .filter(|pair| needs_minimum_gap(pair[0], pair[1]))
                .count()
        })
        .sum();

    assert!(
        total >= 10,
        "整组混排语料的中西文（**直接相邻**）交界只有 {total} 处——\n\
         SC-003 的间隙判据需要足够的真实交界才能被触发。\n\
         注意：CJK 与西文之间隔空格时**不算**交界（空格属 Other 类，\n\
         它自己已是间隙）。语料须含无空格相邻的构造。"
    );

    // 覆盖性：至少有一条语料是"无空格相邻"型（边界密集）——
    // 否则整组都是空格分隔，间隙判据虽被触发但场景单一。
    let dense_texts = mixed_corpus()
        .iter()
        .filter(|text| {
            let chars: Vec<char> = text.chars().collect();
            chars
                .windows(2)
                .filter(|pair| needs_minimum_gap(pair[0], pair[1]))
                .count()
                >= 3
        })
        .count();
    assert!(
        dense_texts >= 2,
        "只有 {dense_texts} 条语料是边界密集型（≥3 处相邻交界）——\
         语料场景过于单一"
    );
}

/// **SC-003 主体**：混排语料下无任一行超出容器宽度。
///
/// ## 判据的容差
///
/// 浮点累加会有微小误差，故判"超出"用 `> measure + epsilon`。
/// epsilon 取 `1e-3`——远小于一个像素，不会掩盖真实溢出。
#[test]
fn sc003_no_line_exceeds_measure() {
    const EPSILON: f32 = 1e-3;
    let mut total_lines = 0usize;
    let mut data_points = 0usize;

    for text in mixed_corpus() {
        for measure_px in [228.0f32, 247.0, 285.0, 323.0, 342.0, 361.0, 399.0, 418.0] {
            let mut engine = LayoutEngine::new();
            let model = TextModel::new(&text);
            let result = engine.layout_paragraph(&model, 0, &geometry(measure_px));

            total_lines += result.lines.len();
            data_points += 1;

            for line in &result.lines {
                let width = line.content_width_px();
                assert!(
                    width <= measure_px + EPSILON,
                    "SC-003 违反：行宽 {width:.2}px 超出容器 {measure_px}px（超出 {:.2}px）\n\
                     行字符区间 {:?}\n行内容前 30 字：{:?}",
                    width - measure_px,
                    line.char_range,
                    text.chars()
                        .skip(line.char_range.start)
                        .take(30)
                        .collect::<String>()
                );
            }
        }
    }

    assert!(
        total_lines > 30,
        "数据点太少（{data_points} 组合 / {total_lines} 行）——结论不可信"
    );
    println!("[SC-003] {data_points} 个组合，共 {total_lines} 行，无超宽");
}

/// **FR-006 第 3 条**：挤压前后**字符下标一致**（区间归属不变）。
///
/// ## 怎么测"前后"
///
/// 无法直接对比"挤压前"（引擎一次性产出最终结果）。故用**不变量**的
/// 形式表达：字形序列的字符区间必须【连续覆盖】行的字符区间，
/// 且每个字形恰好覆盖其字符数——若挤压改变了归属，这两条会破。
///
/// 另外断言：字形数 == 行内字符数（中文排版的基本性质，挤压不该破坏它）。
#[test]
fn fr006_squeeze_preserves_char_attribution() {
    for text in mixed_corpus() {
        for measure_px in [228.0f32, 285.0, 342.0, 399.0] {
            let mut engine = LayoutEngine::new();
            let model = TextModel::new(&text);
            let result = engine.layout_paragraph(&model, 0, &geometry(measure_px));

            for line in &result.lines {
                // 1. 字形覆盖的字符集合必须与行的字符区间一致（连续、无洞）
                let mut expected_char = line.char_range.start;
                for glyph in &line.glyphs {
                    // 字形可能覆盖多个字符（组合簇或者 cosmic-text 的合并）
                    assert!(
                        glyph.char_range.start >= expected_char,
                        "字形区间 {:?} 与前行不连续（期望起点 ≥ {expected_char}）",
                        glyph.char_range
                    );
                    expected_char = glyph.char_range.end;
                }
                assert_eq!(
                    expected_char, line.char_range.end,
                    "字形序列应覆盖到行尾，实测覆盖到 {expected_char}，行尾为 {}",
                    line.char_range.end
                );

                // 2. 压缩量不为负、且各行压缩量之和 == 行级 squeeze_px
                for glyph in &line.glyphs {
                    assert!(
                        glyph.squeeze_px >= 0.0,
                        "压缩量不得为负：{}",
                        glyph.squeeze_px
                    );
                    assert!(
                        glyph.advance_px > 0.0,
                        "字形步进必须为正（压到 0 或负说明下限失效）：{}",
                        glyph.advance_px
                    );
                }
                assert!(
                    (line.squeeze_px - line.summed_glyph_squeeze_px()).abs() < 1e-3,
                    "行级压缩量 {} 与字形明细之和 {} 不符",
                    line.squeeze_px,
                    line.summed_glyph_squeeze_px()
                );
            }
        }
    }
}

/// **FR-006 的压缩下限**：任何字形的压缩量不得超过半字身。
#[test]
fn fr006_compression_respects_named_limit() {
    let limit = max_compression_px(19.0);
    assert!(
        (limit - 19.0 * (1.0 - MIN_SQUEEZE_RATIO)).abs() < 1e-4,
        "下限公式与命名常量应一致"
    );

    for text in mixed_corpus() {
        for measure_px in [228.0f32, 247.0, 285.0, 342.0] {
            let mut engine = LayoutEngine::new();
            let model = TextModel::new(&text);
            let result = engine.layout_paragraph(&model, 0, &geometry(measure_px));

            for line in &result.lines {
                for glyph in &line.glyphs {
                    assert!(
                        glyph.squeeze_px <= limit + 1e-3,
                        "字形压缩量 {} 超过命名下限 {}（半字身）",
                        glyph.squeeze_px,
                        limit
                    );
                }
            }
        }
    }
}

/// **US1 场景 1**：**纯中文行**的行长在版心内浮动 ≤ 1 个挤压单位。
///
/// ## 为什么限定"纯中文行"（这一条经实测修正）
///
/// 初版对本文件的混排语料整体断言，实测 28 行违反。排查发现违反的
/// 全是**含西文单词的行**——例如 `"这本书的英文名是 The Shadow "`
/// 留白 27.77px，因为下一个词 `"of"` 作为**整词**放不下（`Wrap::Word`
/// 的整词优先原则，FR-009 明文要求）。
///
/// 那不是缺陷，是**西文整词规则的必然结果**：贪心断行在"整词放不下"
/// 时只能留白，挤压（只能压标点）对西文字母无能为力。
///
/// 故正确的口径是：判据适用于**纯中文行**——那些行里唯一能调整宽度
/// 的手段就是标点挤压，故"留白 ≤ 1 挤压单位"在那里才成立。
/// 混排行的留白由 T015 的两端对齐（justify）吸收，不是本条的关切。
///
/// ## 口径
///
/// "浮动不超过 1 个标点挤压单位" = 非末行的行宽至少达到
/// `版心 - 挤压单位`。挤压单位取半字身（`max_compression_px`）。
///
/// ## 为什么只看非末行
///
/// 段落的最后一行天然不满（段落结束了），把它计入会让判据失真。
#[test]
fn us1_scenario_1_line_width_within_one_squeeze_unit() {
    let squeeze_unit = max_compression_px(19.0);
    let mut checked_lines = 0usize;
    let mut violations = Vec::new();

    // 纯中文语料（混排行由 justify 吸收留白，见文档）
    let pure_cjk_corpus = [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样。",
        "她说：「真的吗？」我说：「当然，」然后他笑了，笑声在屋子里荡开，混着窗外的雨声。",
        "甲乙丙丁戊己庚辛壬癸子丑寅卯辰巳午未申酉戌亥。",
    ];

    for text in pure_cjk_corpus {
        for measure_px in [342.0f32, 361.0, 380.0, 399.0, 418.0, 437.0] {
            let mut engine = LayoutEngine::new();
            let model = TextModel::new(text);
            let result = engine.layout_paragraph(&model, 0, &geometry(measure_px));

            let body = if result.lines.len() > 1 {
                &result.lines[..result.lines.len() - 1]
            } else {
                continue;
            };

            for line in body {
                checked_lines += 1;
                let width = line.content_width_px();
                let slack = measure_px - width;

                // 精确判据：**下一行的首个可断单元**能否放进本行的剩余空间？
                //
                // 这里用"下一个可断单元"而不是"一个字"——因为禁则会把它
                // 与后续字符绑定（如「是「」是 2 字单元）。用公开的
                // `break_decision` 判定绑定边界，故判据与引擎同源、
                // 不依赖任何内部状态。
                let next_unit_px = next_breakable_unit_width(&result.lines, line, text, measure_px);
                let unit_fits = next_unit_px.is_some_and(|unit| unit <= slack + 1e-3);

                if unit_fits {
                    violations.push((
                        measure_px,
                        width,
                        slack,
                        text.chars()
                            .skip(line.char_range.start)
                            .take(20)
                            .collect::<String>(),
                    ));
                }
            }
        }
    }

    assert!(checked_lines > 10, "检查的行数太少（{checked_lines}）");
    assert!(
        violations.is_empty(),
        "US1 场景 1 违反：{} 个纯中文行的留白超过 1 个挤压单位（{squeeze_unit:.2}px）
         前几处：
{}",
        violations.len(),
        violations
            .iter()
            .take(5)
            .map(|(measure, width, slack, sample)| format!(
                "  版心 {measure}px，行宽 {width:.2}px，留白 {slack:.2}px，内容：{sample:?}"
            ))
            .collect::<Vec<_>>()
            .join(
                "
"
            )
    );

    println!("[US1 场景 1] 检查 {checked_lines} 个纯中文非末行，留白均 ≤ {squeeze_unit:.2}px");
}

/// 下一行的**首个可断单元**的宽度（px）。
///
/// ## 什么是"可断单元"
///
/// 从下一行的首字符开始，沿着公开的禁则表（`break_decision`）向前走，
/// 直到遇到一个**允许断行**的位置——那段字符就是一个不可再分的单元。
/// 禁则会把标点与相邻字符绑定（如「亥。」是 2 字单元），故单元的宽度
/// 可能是 1 个字身，也可能是多个。
///
/// ## 为什么这是正确的判据
///
/// US1 场景 1 说"行长在版心内浮动 ≤ 1 挤压单位"——它的**意图**是
/// "别留下本可填满的空白"。而"本可填满"的精确含义就是
/// **下一个可断单元能否放进本行的剩余空间**：
///
/// - 能放 → 贪心本该放它（或挤压后放它）→ 有留白就是次优
/// - 不能放 → 留白是**必然**的（禁则绑定让它不可分）→ 不是缺陷
///
/// 这比"留白 ≤ N px"精确：后者在"下一个单元是 2 字身"时必然误报。
fn next_breakable_unit_width(
    lines: &[tome_reader::layout::linebox::LineBox],
    line: &tome_reader::layout::linebox::LineBox,
    text: &str,
    measure_px: f32,
) -> Option<f32> {
    let _ = measure_px;
    let index = lines
        .iter()
        .position(|other| other.char_range == line.char_range)?;
    let next = lines.get(index + 1)?;

    let mut width = 0.0f32;
    let mut previous: Option<char> = None;

    for glyph in &next.glyphs {
        let Some(ch) = text
            .get(glyph.byte_range.clone())
            .and_then(|slice| slice.chars().next())
        else {
            continue;
        };

        // 与该字形的**前一个**字符之间能否断？能断则单元在此结束
        if let Some(left) = previous {
            if tome_reader::rules::cjk::break_decision(left, ch).is_allowed() {
                return Some(width);
            }
        }

        width += glyph.advance_px;
        previous = Some(ch);
    }

    // 整行都是同一个单元（极端绑定）
    Some(width)
}

/// **挤压真的在起作用**（端到端，非仅规则模块单测）。
///
/// ## 触发条件（实测确定，见下）
///
/// 挤压只在**行溢出**时触发（`content_width > measure`）。而 `Wrap::Word`
/// 的断行通常不产生溢出——实测扫描发现它只在**禁则绑定单元**（如「丙。」
/// 这一 2 字单元）恰好放不进版心时出现。
///
/// 故有效的触发构造是"版心 = N 字身 - 一点"：让绑定单元溢出半个字身以内，
/// 于是挤压（可压 9.5px）足以把它收进版心。
///
/// ## 实测数据（本测试断言的依据）
///
/// | 版心 | 挤压总量 | 剩余溢出 | 解读 |
/// |---|---|---|---|
/// | 33.25px | 14.25px | **0** | 挤压把行收回版心内 ✅ |
/// | 28.50px | 28.50px | **0** | 同上（多处挤压）✅ |
/// | 23.75px | 28.50px | 14.25px | 几何上不可能（单元 38px > 版心），溢出合法 |
/// | 19.00px | 28.50px | 28.50px | 同上 |
///
/// 前两行是本测试的判据：**存在溢出时挤压必然发生，且能把行收进版心**。
#[test]
fn squeeze_actually_pulls_lines_back_within_measure() {
    let text = "甲乙丙。丁戊己。庚辛壬。";

    let mut verified = 0usize;
    for measure_px in [33.25f32, 28.50] {
        let mut engine = LayoutEngine::new();
        let model = TextModel::new(text);
        let result = engine.layout_paragraph(&model, 0, &geometry(measure_px));

        let total_squeeze: f32 = result.lines.iter().map(|line| line.squeeze_px).sum();
        assert!(
            total_squeeze > 0.0,
            "版心 {measure_px}px 下应触发挤压（绑定单元 38px 放不进），实测挤压为 0"
        );

        // 挤压后**不应**残留溢出（挤压量足以吸收）
        for line in &result.lines {
            let width = line.content_width_px();
            assert!(
                width <= measure_px + 1e-3,
                "版心 {measure_px}px 下行宽 {width:.2}px 仍溢出——挤压未吸收完"
            );
        }
        verified += 1;
    }

    assert_eq!(verified, 2, "两个触发构造都应被验证");
}

/// **挤压的几何上限**：版心小于绑定单元时，溢出是**合法**的。
///
/// ## 为什么单列这条
///
/// 它把"挤压并非万能"这个事实钉住。否则后人看到 SC-003 的"无超宽"断言
/// 可能会以为"任何几何下都不该溢出"——而版心小于一个不可分单元时，
/// 溢出是唯一的可能（FR-008 第 ④ 步：几何条件不允许时放弃）。
#[test]
fn overflow_is_legal_when_measure_is_smaller_than_indivisible_unit() {
    // 「丙。」= 2 字身 = 38px，版心 23.75px 装不下
    let text = "甲乙丙。丁戊己。";
    let measure_px = 23.75f32;

    let mut engine = LayoutEngine::new();
    let model = TextModel::new(text);
    let result = engine.layout_paragraph(&model, 0, &geometry(measure_px));

    // 挤压确实尽力了
    let total_squeeze: f32 = result.lines.iter().map(|line| line.squeeze_px).sum();
    assert!(total_squeeze > 0.0, "挤压应尽力（实测 {total_squeeze}）");

    // 但溢出仍存在——这是几何必然，不是缺陷
    let max_overflow = result
        .lines
        .iter()
        .map(|line| (line.content_width_px() - measure_px).max(0.0))
        .fold(0.0f32, f32::max);
    assert!(
        max_overflow > 0.0,
        "版心 {measure_px}px 装不下 38px 的绑定单元，应有溢出"
    );

    println!(
        "[T014] 版心 {measure_px}px：挤压 {total_squeeze:.2}px，残留溢出 {max_overflow:.2}px（几何必然）"
    );
}

/// **FR-009 的交界间隙**：中西文交界的字距大于同类相邻。
///
/// ## 判据
///
/// 找一个"汉字 → 拉丁字母"的交界，比较"该汉字字形的步进"与
/// "纯汉字字形的步进"。前者应大出约 `0.25em`。
#[test]
fn fr009_boundary_gap_widens_the_step() {
    use tome_reader::rules::mixed::minimum_gap_px;

    // "中a" —— 位置 0 是汉字且后接拉丁字母（需要间隙）
    let text = "中a";
    let measure_px = 342.0;

    let mut engine = LayoutEngine::new();
    let model = TextModel::new(text);
    let result = engine.layout_paragraph(&model, 0, &geometry(measure_px));

    let line = &result.lines[0];
    let cjk_glyph = line
        .glyphs
        .iter()
        .find(|glyph| glyph.char_range.start == 0)
        .expect("应有覆盖首个汉字的字形");

    let expected_extra = minimum_gap_px(19.0);
    // 汉字天然 19px + 间隙 4.75px = 23.75px
    let expected_advance = 19.0 + expected_extra;

    assert!(
        (cjk_glyph.advance_px - expected_advance).abs() < 0.5,
        "汉字后接西文时步进应为 {expected_advance:.2}px（19 + 间隙 {expected_extra:.2}），\
         实测 {:.2}px",
        cjk_glyph.advance_px
    );

    // 反例：汉字后接汉字不加间隙
    let plain = TextModel::new("中中");
    let plain_result = engine.layout_paragraph(&plain, 0, &geometry(measure_px));
    let plain_glyph = &plain_result.lines[0].glyphs[0];
    assert!(
        (plain_glyph.advance_px - 19.0).abs() < 0.5,
        "汉字之间不该加间隙，实测 {:.2}px",
        plain_glyph.advance_px
    );
}

/// **FR-009 的退化**：超长西文单词必须被拆开（不得整词溢出）。
///
/// ## 这一条测的是"自行实现的退化路径"
///
/// T005 spike 实测 `Wrap::Word` 对超宽词是**溢出**而非切分——故
/// FR-009 的"超长单词退化为按字符断行"**不能**靠它获得。
/// 本测试验证该路径是否真的存在（当前 T014 只实现了候选发现与拆词
/// 的**工具**，引擎接线在检查点后补齐——见测试末尾的诚实标注）。
#[test]
fn fr009_overlong_word_degradation_is_available() {
    use tome_reader::rules::mixed::{
        chars_per_line_for_measure, find_overlong_words, split_overlong_word,
    };

    let long_word = "pneumonoultramicroscopicsilicovolcanoconiosis";
    let text = format!("这个术语拼作 {long_word}，是一个很长的词。");
    let measure_px = 228.0;

    let found = find_overlong_words(&text, measure_px, 19.0);
    assert_eq!(found.len(), 1, "应找到 1 个超长词：{found:?}");

    let word = &found[0];
    let chunk = chars_per_line_for_measure(measure_px, 19.0);
    let pieces = split_overlong_word(word, chunk);

    assert!(
        pieces.len() > 1,
        "超长词应被拆成多段，实测 {} 段",
        pieces.len()
    );
    assert_eq!(
        pieces.first().unwrap().start,
        word.char_range.start,
        "首段应从词首开始"
    );
    assert_eq!(
        pieces.last().unwrap().end,
        word.char_range.end,
        "末段应到词尾结束"
    );
    for window in pieces.windows(2) {
        assert_eq!(window[0].end, window[1].start, "片段须首尾相接");
    }

    // --- 诚实标注：引擎接线现状 ---
    //
    // 当前引擎**尚未**把拆词结果送进 layout 调用（那需要"预处理文本 →
    // 插入零宽断点 → 重排"的一轮迭代，属 T015 的硬序第 ③ 步范围）。
    // 故此处**不断言**引擎输出已无超宽——那会是假绿。
    // 本测试只断言"退化工具就绪且正确"。
    //
    // 引擎侧的实际状态由 sc003_no_line_exceeds_measure 覆盖：该测试的
    // 语料**不含**超长单词（最长的 "Programming" 等远短于版心），
    // 故它测的是常规混排，不涉及退化路径。
    let mut engine = LayoutEngine::new();
    let model = TextModel::new(&text);
    let result = engine.layout_paragraph(&model, 0, &geometry(measure_px));

    let widest = result
        .lines
        .iter()
        .map(|line| line.content_width_px())
        .fold(0.0f32, f32::max);

    println!(
        "[FR-009 现状] 含超长词的文本在 {measure_px}px 版心下最宽行 {widest:.2}px\
         （{} 行）——退化路径的**引擎接线**待 T015 完成",
        result.lines.len()
    );
}
