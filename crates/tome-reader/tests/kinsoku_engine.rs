//! T013 验收：禁则端到端（SC-002 的判据载体）
//!
//! ## SC-002 的原文口径
//!
//! "在标点密集的合成语料（每百字含不少于 15 个全角标点）下，行首禁则与
//! 行尾禁则的**违反次数为 0**（允许的例外必须落进 FR-008 第 ④ 步的例外
//! 记录并可计数）。"
//!
//! 故本文件断言**两件**事：
//!
//! 1. 没有"未记录的"违规——即引擎自己核验出的例外数 == 我们独立重算的违规数
//! 2. 记录下来的例外可枚举可计数（这是 SC-002 括号里那句的落点）
//!
//! ## 为什么"零违规"要拆成这两条
//!
//! 只断言"例外数 == 0"是有风险的：如果核验逻辑本身失效（永远返回空），
//! 断言会平凡通过——这正是 T-005/T-007/T-009 三次踩到的坑。故本文件用
//! **独立的**违规重算（不复用引擎的核验代码），两条对得上才算有效。

use tome_reader::layout::engine::{Geometry, LayoutEngine};
use tome_reader::layout::linebox::HardOrderStep;

use tome_reader::text::model::TextModel;
use tome_reader::text::paragraph;

/// 标点密集的合成语料（SC-002 要求每百字 ≥15 个全角标点）。
///
/// ## 语料设计
///
/// 用真实感的中文散文，但把标点频率提高——引号、逗号、句号、问号密集出现。
/// 关键是要覆盖**行首禁则字符出现在断点附近**的情形：若语料里标点都远离
/// 断点位置，测试即使实现有 bug 也会通过（那正是"语料不够密集"的失效模式）。
fn punctuation_dense_corpus() -> Vec<String> {
    vec![
        // 引号与逗号密集
        "他说：「真的吗？」我说：「当然，」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。"
            .to_owned(),
        // 问号叹号密集
        "什么？怎么会这样？不可能！她喊出来，声音里带着难以置信，也带着一丝说不清的期待，然后她转过身，看着窗外。"
            .to_owned(),
        // 括号与书名号
        "《红楼梦》（又名《石头记》）是清代曹雪芹的作品，书中有（据不完全统计）四百多个人物，其中（当然）以宝黛钗三人最为人所知。"
            .to_owned(),
        // 破折号与省略号（补丁条目）
        "他说——那是很久以前的事了——人这一生总要留下点什么。可是留下的东西……究竟算不算数呢？风从窗缝里挤进来。"
            .to_owned(),
        // 顿号与分号
        "桌上有笔、墨、纸、砚；窗边有兰、竹、菊、梅；墙上挂着山水、花鸟、人物；架上摆着经、史、子、集。"
            .to_owned(),
    ]
}

/// 统计文本中的全角标点数量（用于验证语料确实"密集"）。
fn count_full_width_punctuation(text: &str) -> usize {
    const PUNCT: &str = "，。、；：？！（）［］｛｝〈〉《》「」『』【】〖〗〘〙〚〛“”‘’…—·％‰℃°";
    text.chars().filter(|ch| PUNCT.contains(*ch)).count()
}

/// **语料有效性自检**：标点密度必须达到 SC-002 的门槛（≥15 个/百字）。
///
/// 这条防的是"语料不够密集导致测试平凡通过"——SC-002 的门槛是对**语料**的
/// 要求，若不检查语料，测试的结论就没有前提。
#[test]
fn corpus_meets_sc002_punctuation_density() {
    for text in punctuation_dense_corpus() {
        let chars = text.chars().count();
        let punct = count_full_width_punctuation(&text);
        let density = punct as f64 * 100.0 / chars as f64;
        assert!(
            density >= 15.0,
            "语料标点密度 {density:.1}/百字 低于 SC-002 的 15 门槛\n\
             语料：{text}（{chars} 字，{punct} 个标点）"
        );
    }
}

/// 独立重算：**逐行**查行首/行尾禁则违规（**不复用引擎的核验代码**）。
///
/// ## 判据与 SC-002 原文一致
///
/// SC-002 说的是"行首禁则与行尾禁则的违反次数为 0"——故逐行查首字符
/// 与末字符。初版写成"查相邻两行的边界"，那会漏掉"某行以闭标点开头
/// 而它前面没有上一行"（段落本身以标点开头）的情形。
///
/// ## 为什么必须独立实现
///
/// 若与引擎的核验共用代码，两者的 bug 会一起犯——测试变成同义反复。
/// 此处刻意从 `lines` 出发重新取字符、重新判禁则，作为**交叉验证**。
fn independently_count_violations(
    lines: &[tome_reader::layout::linebox::LineBox],
    model: &TextModel,
) -> Vec<(usize, &'static str)> {
    use tome_reader::rules::cjk::{line_end_rule_for, line_start_rule_for};

    let text = model.text();
    let mut violations = Vec::new();

    for line in lines {
        if let Some(first) = line
            .glyphs
            .first()
            .and_then(|glyph| text.get(glyph.byte_range.clone()))
            .and_then(|slice| slice.chars().next())
        {
            if let Some(rule_id) = line_start_rule_for(first) {
                violations.push((line.char_range.start, rule_id));
            }
        }

        if let Some(last) = line
            .glyphs
            .last()
            .and_then(|glyph| text.get(glyph.byte_range.clone()))
            .and_then(|slice| slice.chars().next_back())
        {
            if let Some(rule_id) = line_end_rule_for(last) {
                violations.push((line.char_range.end.saturating_sub(1), rule_id));
            }
        }
    }

    violations
}

/// 排版一段文本，返回 (行盒, 例外)。
#[expect(
    clippy::expect_used,
    reason = "测试夹具无法就绪时应让所属测试立即失败，错误传播只会加噪音（与 .clippy.toml 的 allow-expect-in-tests 同一意图；该配置不覆盖 #[test] 之外的辅助函数）"
)]
fn layout(
    text: &str,
    measure_px: f32,
) -> (
    Vec<tome_reader::layout::linebox::LineBox>,
    Vec<tome_reader::layout::linebox::RuleExceptionRecord>,
) {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let model = TextModel::new(text);
    let geometry = Geometry {
        measure_px,
        font_size_px: 19.0,
        line_height: 1.85,
        alignment: tome_reader::rules::justify::Alignment::Justified,
        paragraph_mode: tome_reader::paginate::config::ParagraphMode::default(),
    };
    let result = engine.layout_paragraph(&model, 0, &geometry);
    (result.lines, result.exceptions)
}

/// **SC-002 主体**：标点密集语料下，禁则违反次数为 0。
///
/// ## 数据点覆盖
///
/// 扫过多种版心宽度——**断点位置随宽度变化**，故只有多宽度才能让
/// 行首禁则字符真的落到断点附近。单一宽度可能恰好避开所有禁则位置。
#[test]
fn sc002_no_kinsoku_violations_on_dense_corpus() {
    let mut total_violations = 0usize;
    let mut total_lines = 0usize;
    let mut data_points = Vec::new();

    for text in punctuation_dense_corpus() {
        for measure_px in [228.0f32, 247.0, 285.0, 323.0, 342.0, 361.0, 399.0, 418.0] {
            let (lines, exceptions) = layout(&text, measure_px);
            let model = TextModel::new(&text);
            let violations = independently_count_violations(&lines, &model);

            total_lines += lines.len();
            total_violations += violations.len();

            data_points.push((
                text.chars().count(),
                measure_px,
                lines.len(),
                violations.len(),
                exceptions.len(),
            ));

            assert!(
                violations.is_empty(),
                "SC-002 违反：版心 {measure_px}px 下出现 {} 处禁则违规\n\
                 语料（前 40 字）：{}\n\
                 违规明细（行首字符位置, 规则）：{violations:?}\n\
                 行盒数：{}",
                violations.len(),
                text.chars().take(40).collect::<String>(),
                lines.len()
            );
        }
    }

    // 数据规模自检：若行数很少，上面的断言可能是平凡的
    assert!(
        total_lines > 40,
        "数据点太少（共 {total_lines} 行）——语料或宽度覆盖不足，结论不可信"
    );
    println!(
        "[SC-002] {} 个数据点，共 {total_lines} 行，违规 {total_violations} 处",
        data_points.len()
    );
}

/// **例外记录的有效性**：核验发现的违规数与引擎记录的例外数一致。
///
/// ## 这条防什么
///
/// 若引擎的核验逻辑失效（永远返回空），上面的 SC-002 测试会平凡通过。
/// 本测试把"引擎记的"与"独立重算的"对上——两者必须相等。
/// 在当前实现下两者通常都是 0（禁则生效），但**相等**这个断言本身
/// 才是有效性的证明。
#[test]
fn engine_exceptions_match_independent_recount() {
    for text in punctuation_dense_corpus() {
        for measure_px in [228.0f32, 323.0, 361.0, 418.0] {
            let (lines, exceptions) = layout(&text, measure_px);
            let model = TextModel::new(&text);
            let violations = independently_count_violations(&lines, &model);

            assert_eq!(
                exceptions.len(),
                violations.len(),
                "引擎记录的例外数（{}）与独立重算的违规数（{}）不符——\n\
                 两者必须一致，否则 SC-002 的计数判据不可信",
                exceptions.len(),
                violations.len()
            );

            // 逐条比对规则 id 与位置
            for (record, (expected_char, expected_rule)) in exceptions.iter().zip(&violations) {
                assert_eq!(record.char_index, *expected_char);
                assert_eq!(record.rule_id, *expected_rule);
            }
        }
    }
}

/// 例外记录**可枚举可计数**（SC-002 括号内的明文要求）。
///
/// ## 什么输入能让例外**真的可达**
///
/// 合并 word 的失效模式是**溢出**而非违规（超宽单元被独自放一行），
/// 故"窄版心"构造**不能**逼出例外——实测 9px 版心下例外为 0。
///
/// 真正可达的路径是**输入本身的边界条件**：段落以行首禁则字符**开头**时，
/// 没有任何排版手段能把该字符移到别处（它前面没有内容可合并）——
/// 这是 FR-008 第 ④ 步的正当情形，也是 SC-002 括号里"允许的例外"。
///
/// 若这条测试失败（例外为空），说明核验机制没接线，而不是"禁则完美生效"
/// ——两者的区别正是 SC-002 要区分的东西。
#[test]
fn exceptions_are_enumerable_and_countable() {
    // 段落**以闭标点开头**：无论怎么排，该字符都在首行行首
    let text = "。这是一段以句号开头的异常文本，用于逼出第 ④ 步的例外记录。";
    let (lines, exceptions) = layout(text, 342.0);

    assert!(!lines.is_empty(), "应产出行盒");

    let model = TextModel::new(text);
    let violations = independently_count_violations(&lines, &model);

    assert!(
        !exceptions.is_empty(),
        "段落以「。」开头时，首行行首必然是该闭标点——无任何排版手段可避免。\n\
         实测：{} 行，独立重算违规 {} 处，引擎记录例外 {} 条。\n\
         若例外为空，说明核验机制未接线（而不是「禁则完美生效」）。",
        lines.len(),
        violations.len(),
        exceptions.len()
    );

    // 可计数：每条记录都有明确的 rule_id 与位置
    for record in &exceptions {
        assert!(
            record.rule_id.starts_with("kinsoku."),
            "例外记录的规则 id 须来自禁则清单，实际 {}",
            record.rule_id
        );
        assert!(
            record.char_index <= text.chars().count(),
            "例外的字符位置 {} 越界（文本 {} 字）",
            record.char_index,
            text.chars().count()
        );
        assert_eq!(
            record.reached_step,
            HardOrderStep::GiveUpKinsoku,
            "禁则被违反意味着走到了第 ④ 步"
        );
    }

    // 记录数 == 独立重算数（一致性）
    assert_eq!(
        exceptions.len(),
        violations.len(),
        "引擎记录 {} 条 vs 独立重算 {} 处",
        exceptions.len(),
        violations.len()
    );

    println!(
        "[SC-002] 可达例外构造：{} 行、{} 条例外（规则：{:?}）",
        lines.len(),
        exceptions.len(),
        exceptions
            .iter()
            .map(|record| record.rule_id)
            .collect::<Vec<_>>()
    );
}

/// **合并路线的失效模式是溢出，不是违规**（T013 的固有性质，记录在案）。
///
/// ## 为什么这条测试存在
///
/// 它把"窄版心不能逼出例外"这个**实测事实**钉住。否则后人看到
/// `exceptions_are_enumerable_and_countable` 用了"段首标点"这个构造，
/// 可能会误以为"窄版心是失效的"从而去"修"一个不存在的问题。
///
/// 溢出本身是 SC-003（混排无超宽）的关切，由 T014 的挤压与混排处理。
#[test]
fn narrow_measure_overflows_rather_than_violating_kinsoku() {
    let text = "甲，乙。丙、丁；戊：己？庚！辛";
    let (lines, exceptions) = layout(text, 9.0);

    let model = TextModel::new(text);
    let violations = independently_count_violations(&lines, &model);

    assert!(
        violations.is_empty(),
        "窄版心下的合并单元独自成行，不应产生禁则违规：{violations:?}"
    );
    assert!(
        exceptions.is_empty(),
        "同上，例外也应为空（该构造不触发第 ④ 步）：{exceptions:?}"
    );

    // 有效载荷自检：确实产出了多行（否则上面的断言平凡成立）
    assert!(
        lines.len() > 1,
        "9px 版心下应排成多行，实测 {} 行",
        lines.len()
    );
}

/// 行首禁则的实际生效断言：**没有任何一行以闭标点开头**。
///
/// 这是 SC-002 的**直接形态**（不看例外记录，直接查行首字符）——
/// 与上面的"违反计数为 0"互补：前者查的是引擎自报，这条查的是实际结果。
#[test]
fn no_line_starts_with_closing_punctuation() {
    use tome_reader::rules::cjk::line_start_rule_for;

    for text in punctuation_dense_corpus() {
        for measure_px in [228.0f32, 285.0, 342.0, 399.0] {
            let (lines, _) = layout(&text, measure_px);

            for line in &lines {
                let Some(first_glyph) = line.glyphs.first() else {
                    continue;
                };
                let first_char = text
                    .get(first_glyph.byte_range.clone())
                    .and_then(|slice| slice.chars().next());
                let Some(first_char) = first_char else {
                    continue;
                };

                if let Some(rule_id) = line_start_rule_for(first_char) {
                    panic!(
                        "行首禁则违反：某行以 `{first_char}` 开头（规则 {rule_id}）\n\
                         版心 {measure_px}px，该行字符区间 {:?}\n\
                         该行前 20 字：{:?}",
                        line.char_range,
                        text.chars()
                            .skip(line.char_range.start)
                            .take(20)
                            .collect::<String>()
                    );
                }
            }
        }
    }
}

/// 行尾禁则的实际生效断言：**没有任何一行以开标点结尾**。
#[test]
fn no_line_ends_with_opening_punctuation() {
    use tome_reader::rules::cjk::line_end_rule_for;

    for text in punctuation_dense_corpus() {
        for measure_px in [228.0f32, 285.0, 342.0, 399.0] {
            let (lines, _) = layout(&text, measure_px);

            for line in &lines {
                let Some(last_glyph) = line.glyphs.last() else {
                    continue;
                };
                let last_char = text
                    .get(last_glyph.byte_range.clone())
                    .and_then(|slice| slice.chars().next_back());
                let Some(last_char) = last_char else {
                    continue;
                };

                if let Some(rule_id) = line_end_rule_for(last_char) {
                    panic!(
                        "行尾禁则违反：某行以 `{last_char}` 结尾（规则 {rule_id}）\n\
                         版心 {measure_px}px，该行字符区间 {:?}",
                        line.char_range
                    );
                }
            }
        }
    }
}

/// 多段文本：每段的禁则独立成立（段边界不应引入违规）。
#[test]
fn multi_paragraph_kinsoku_holds_per_paragraph() {
    let mut engine = LayoutEngine::new().expect("内嵌字体栈应可装载");
    let split = paragraph::split(
        "他说：「真的吗？」我说：「当然。」\n《红楼梦》里（据说）有四百多个人物。\n什么？怎么会这样！",
    );

    assert_eq!(split.paragraphs.len(), 3, "应有 3 段");

    for (index, para) in split.paragraphs.iter().enumerate() {
        let model = para.model();
        let geometry = Geometry {
            measure_px: 247.0,
            font_size_px: 19.0,
            line_height: 1.85,
            alignment: tome_reader::rules::justify::Alignment::Justified,
            paragraph_mode: tome_reader::paginate::config::ParagraphMode::default(),
        };
        let result = engine.layout_paragraph(&model, index, &geometry);

        for line in &result.lines {
            assert_eq!(line.paragraph_index, index, "行盒的段序号应与其来源段一致");
        }

        let violations = independently_count_violations(&result.lines, &model);
        assert!(
            violations.is_empty(),
            "第 {index} 段（{:?}）出现禁则违规：{violations:?}",
            para.text
        );
    }
}
