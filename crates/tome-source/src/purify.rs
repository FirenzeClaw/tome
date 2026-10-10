//! [DONE] 净化管线（tasks.md T069 / FR-014、FR-015、FR-016）
//!
//! # 本模块只做一件事：把落库的规则 DTO 翻译成加工链的输入
//!
//! 加工链本身（替换 → 高亮 → 词典的**定序**求值）在
//! [`tome_rule::pipeline::ContentPipeline`] 里早已实现并通过验收。
//! 本模块**不重新实现**它——重复实现是"两处顺序约定迟早写错一处"的来源。
//!
//! 它要做的是把那套能力接到存储与管线上：
//!
//! ```text
//!   read replace_rule ─┐
//!   read highlight_rule├─► 翻译（DTO → 加工链类型）─► ContentPipeline
//!   read dict_rule ────┘                                      │
//!                                                             ▼
//!                                          PipelineOutcome（文本 + 高亮区间 + 词典查询）
//! ```
//!
//! # 为什么"翻译"值得单成一个模块
//!
//! 两侧的类型形状**刻意不同**，这是设计而非疏漏：
//!
//! | 维度 | 存储 DTO（`tome-core`） | 加工链类型（`tome-rule`） |
//! |---|---|---|
//! | `id` | `i64`（数据库主键） | `String`（诊断用的人可读标识） |
//! | `scope` | `RuleScope` 枚举（FR-014 的生效范围） | 无（调用方先按范围过滤） |
//! | `is_regex` | 有 | 有 |
//!
//! `id` 的类型差异是有意的：存储层的主键是数据库概念，而加工链要的是
//! "这条高亮哪来的"这类**可读**标识。强行统一会让其中一侧承担不属于
//! 它的关切。
//!
//! # 生效范围（`scope`）的过滤发生在**这里**，不在加工链
//!
//! FR-014 的 `scope` 决定一条替换规则作用在哪类文本上（正文/书名/章节
//! 标题/全局）。加工链只知道"给一段文本做替换"——**范围是调用方的知识**。
//! 故 [`Purifier`] 按 [`PurifyTarget`] 在翻译阶段就过滤，让加工链收到的
//! 永远是"该作用于这段文本的规则"。

use std::sync::Arc;

use tome_core::{DictRule, Highlight, HighlightRule, ReplaceRule, RuleScope};
use tome_rule::pipeline::{ContentPipeline, PipelineOutcome};

use crate::error::SourceError;

/// 被净化的文本类型（决定哪些 `scope` 的规则生效）。
///
/// # 为什么用枚举而不是 `RuleScope`
///
/// `RuleScope` 有一项 `Global`，而"全局"是**规则侧**的性质而非"文本侧"
/// 的类型——一个文本不会"是全局的"。用独立的枚举让 `PurifyTarget` 的
/// 取值就是"实际存在的文本类别"，而 `Global` 的匹配逻辑落在
/// [`applies_to`] 里一处。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PurifyTarget {
    /// 正文内容
    Content,
    /// 书名
    BookName,
    /// 章节标题
    ChapterTitle,
}

impl PurifyTarget {
    /// 该文本类别对应的 `RuleScope`（`Global` 不在其中，见 [`applies_to`]）。
    pub fn scope(self) -> RuleScope {
        match self {
            Self::Content => RuleScope::Content,
            Self::BookName => RuleScope::BookName,
            Self::ChapterTitle => RuleScope::ChapterTitle,
        }
    }
}

/// 一条规则是否作用于某类文本。
///
/// # 判据
///
/// 范围相符，**或**规则声明为 `Global`（FR-014 的"全局"即作用于所有文本）。
pub fn applies_to(scope: RuleScope, target: PurifyTarget) -> bool {
    scope == RuleScope::Global || scope == target.scope()
}

/// 加工链的产物（净化后的正文 + 高亮区间 + 词典查询 + 诊断）。
pub type PurifyOutcome = PipelineOutcome;

/// 净化器：持有一条书源/全局的规则集合（**已排序**），按目标文本求值。
///
/// # 为什么在构造时排序并过滤
///
/// FR-014 的顺序要求是"同一层内按 `sort_order`"。若在每次求值时排序，
/// 5 万章的阅读路径上就是 5 万次排序。构造时排一次让求值路径是纯遍历。
///
/// 同样地，禁用规则在构造时就被丢弃——它们是**配置状态**而非逐次变化
/// 的输入。
#[derive(Debug, Clone, Default)]
pub struct Purifier {
    replace_rules: Vec<ReplaceRule>,
    highlight_rules: Vec<HighlightRule>,
    dict_rules: Vec<DictRule>,
}

impl Purifier {
    /// 从规则集合构造（自动过滤禁用项并按 `sort_order` 排序）。
    ///
    /// 排序规则：`sort_order` 升序，同值时按 `id` 升序——保证**同一份配置
    /// 的多次运行结果一致**（否则同样两段文本会因排序不稳定而产生不同的
    /// 净化结果，那是无法复现的 bug）。
    pub fn new(
        replace_rules: Vec<ReplaceRule>,
        highlight_rules: Vec<HighlightRule>,
        dict_rules: Vec<DictRule>,
    ) -> Self {
        let mut replace_rules: Vec<ReplaceRule> = replace_rules
            .into_iter()
            .filter(|rule| rule.enabled)
            .collect();
        replace_rules.sort_by_key(|rule| (rule.sort_order, rule.id));

        let mut highlight_rules: Vec<HighlightRule> = highlight_rules
            .into_iter()
            .filter(|rule| rule.enabled)
            .collect();
        highlight_rules.sort_by_key(|rule| (rule.sort_order, rule.id));

        // 词典规则没有 sort_order（`data_model` 的 dict_rule 无该列），
        // 故只过滤 + 按 id 排序保证确定性
        let mut dict_rules: Vec<DictRule> =
            dict_rules.into_iter().filter(|rule| rule.enabled).collect();
        dict_rules.sort_by_key(|rule| rule.id);

        Self {
            replace_rules,
            highlight_rules,
            dict_rules,
        }
    }

    /// 空净化器（没有任何规则时用，让调用点不必写 `Option`）。
    pub fn empty() -> Self {
        Self::default()
    }

    /// 规则数量（诊断用）。
    pub fn rule_counts(&self) -> (usize, usize, usize) {
        (
            self.replace_rules.len(),
            self.highlight_rules.len(),
            self.dict_rules.len(),
        )
    }

    /// 对一段文本求值加工链。
    ///
    /// `target` 决定哪些 `scope` 的替换规则生效；`dictionary_words` 是
    /// 要对哪些词发起词典查询（**不产释义**——那要发网络请求，见
    /// [`tome_rule::pipeline::DictQuery`] 的说明）。
    pub fn purify(
        &self,
        text: &str,
        target: PurifyTarget,
        dictionary_words: &[String],
    ) -> Result<PurifyOutcome, SourceError> {
        let pipeline = self.build_pipeline(target)?;
        Ok(pipeline.run(text, dictionary_words))
    }

    /// 只做替换（高亮与词典不参与）。
    ///
    /// # 为什么需要它
    ///
    /// 书名与章节标题只需要净化（去掉广告词），不需要高亮区间与词典查询
    /// ——后两者对"目录里的一行标题"没有意义。为它们跑完整的加工链会
    /// 产出无人使用的区间数据。
    pub fn purify_text_only(
        &self,
        text: &str,
        target: PurifyTarget,
    ) -> Result<String, SourceError> {
        let pipeline = self.build_pipeline(target)?;
        Ok(pipeline.run(text, &[]).text)
    }

    /// 构造加工链（按目标文本过滤替换规则的范围）。
    fn build_pipeline(&self, target: PurifyTarget) -> Result<ContentPipeline, SourceError> {
        let replace: Vec<tome_rule::pipeline::ReplaceRule> = self
            .replace_rules
            .iter()
            .filter(|rule| applies_to(rule.scope, target))
            .map(translate_replace)
            .collect();

        let highlight: Vec<tome_rule::pipeline::HighlightRule> = self
            .highlight_rules
            .iter()
            .map(translate_highlight)
            .collect();

        let dict: Vec<tome_rule::pipeline::DictRule> =
            self.dict_rules.iter().map(translate_dict).collect();

        Ok(ContentPipeline::new()
            .with_replace_rules(replace)
            .with_highlight_rules(highlight)
            .with_dict_rules(dict))
    }
}

/// `tome-core` 的替换规则 → 加工链的替换规则。
fn translate_replace(rule: &ReplaceRule) -> tome_rule::pipeline::ReplaceRule {
    tome_rule::pipeline::ReplaceRule {
        // 存储主键是数字，而加工链的诊断标识要人可读——拼上规则名，
        // 让"这条替换是哪来的"在日志里直接可读
        id: format!("#{} {}", rule.id, rule.name),
        pattern: rule.pattern.clone(),
        replacement: rule.replacement.clone(),
        is_regex: rule.is_regex,
        // 已在 `Purifier::new` 过滤
        enabled: true,
        sort_order: rule.sort_order as i64,
    }
}

/// `tome-core` 的高亮规则 → 加工链的高亮规则。
fn translate_highlight(rule: &HighlightRule) -> tome_rule::pipeline::HighlightRule {
    tome_rule::pipeline::HighlightRule {
        id: format!("#{} {}", rule.id, rule.name),
        pattern: rule.pattern.clone(),
        style: rule.style.clone(),
        enabled: true,
        sort_order: rule.sort_order as i64,
    }
}

/// `tome-core` 的词典规则 → 加工链的词典规则。
fn translate_dict(rule: &DictRule) -> tome_rule::pipeline::DictRule {
    tome_rule::pipeline::DictRule {
        id: format!("#{} {}", rule.id, rule.name),
        url_rule: rule.url_rule.clone(),
        content_rule: rule.content_rule.clone(),
        enabled: true,
    }
}

/// 把加工链产出的高亮区间转成可入库的痕迹（FR-015 → FR-028）。
///
/// # 只转"区间"，不产 DTO 的其余字段
///
/// `Highlight` 还有 `book_id`/`note`/`created_at` 等字段，那些是调用方
/// 的上下文（哪本书、什么时间），不是加工链的产物。本函数只把
/// `（区间 + 样式）` 填进去，其余留给调用方——那样"加工链知道什么"
/// 与"它产出了什么"保持一一对应。
pub fn highlights_to_traces(
    outcome: &PurifyOutcome,
    book_id: i64,
    chapter_index: i32,
) -> Vec<Highlight> {
    outcome
        .highlights
        .iter()
        .map(|range| Highlight {
            id: 0,
            book_id,
            chapter_index,
            // `HighlightRange` 的 start/end 是 `usize`（字符下标），
            // 而痕迹 DTO 用 `i64`。转换失败只可能是区间超过 i64——那在
            // 真实文本里不可能（i64 的上限是 9e18 个字符）。
            char_offset: range.start as i64,
            length: range.len() as i64,
            style: range.style.clone(),
            note: None,
            created_at: 0,
        })
        .collect()
}

/// 净化器的共享句柄（阅读路径上多个章节共用同一套规则）。
pub type SharedPurifier = Arc<Purifier>;

#[cfg(test)]
mod tests {
    use super::*;

    fn replace_rule(id: i64, name: &str, pattern: &str, replacement: &str) -> ReplaceRule {
        ReplaceRule {
            id,
            name: name.to_string(),
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            scope: RuleScope::Content,
            is_regex: false,
            enabled: true,
            sort_order: id as i32,
        }
    }

    fn highlight_rule(id: i64, pattern: &str, style: &str) -> HighlightRule {
        HighlightRule {
            id,
            name: format!("高亮{id}"),
            pattern: pattern.to_string(),
            style: style.to_string(),
            enabled: true,
            sort_order: id as i32,
        }
    }

    fn dict_rule(id: i64, name: &str) -> DictRule {
        DictRule {
            id,
            name: name.to_string(),
            url_rule: format!("https://dict.test/{{{{word}}}}?r={id}"),
            content_rule: "@css:.def@text".to_string(),
            enabled: true,
        }
    }

    // --- 替换 ---

    #[test]
    fn 空净化器原样返回() {
        let purifier = Purifier::empty();
        let outcome = purifier
            .purify("正文内容", PurifyTarget::Content, &[])
            .expect("应成功");
        assert_eq!(outcome.text, "正文内容");
        assert!(outcome.highlights.is_empty());
    }

    #[test]
    fn 替换规则生效() {
        let purifier = Purifier::new(
            vec![replace_rule(1, "去广告", "本章由XXX提供", "")],
            vec![],
            vec![],
        );
        let outcome = purifier
            .purify("正文本章由XXX提供结束", PurifyTarget::Content, &[])
            .expect("应成功");
        assert_eq!(outcome.text, "正文结束");
    }

    #[test]
    fn 多条替换按_sort_order_依次应用() {
        // FR-014 的"顺序固定且可测"：同层内按 sort_order
        let mut first = replace_rule(1, "先", "A", "B");
        first.sort_order = 1;
        let mut second = replace_rule(2, "后", "B", "C");
        second.sort_order = 2;
        let purifier = Purifier::new(vec![second, first], vec![], vec![]);

        let outcome = purifier
            .purify("A", PurifyTarget::Content, &[])
            .expect("应成功");
        assert_eq!(outcome.text, "C", "A→B→C 说明按 sort_order 依次应用");
    }

    #[test]
    fn 相同_sort_order_按_id_稳定排序() {
        // 顺序不确定会让同一份配置在不同运行下产出不同结果——不可复现的 bug。
        // 两条规则都匹配 `X`：id 小的先应用，它的产出再被 id 大的规则匹配
        // （id=3 把 X→Y，id=5 把 Y→Z，故最终是 Z）。
        let mut a = replace_rule(5, "五", "Y", "Z");
        a.sort_order = 1;
        let mut b = replace_rule(3, "三", "X", "Y");
        b.sort_order = 1;

        // 构造顺序与 id 顺序相反，验证排序确实按 id
        let purifier = Purifier::new(vec![a, b], vec![], vec![]);
        let outcome = purifier
            .purify("X", PurifyTarget::Content, &[])
            .expect("应成功");
        assert_eq!(
            outcome.text, "Z",
            "id=3 先（X→Y），id=5 后（Y→Z）；若是 id=5 先则 X 不匹配，结果是 X"
        );
    }

    #[test]
    fn 禁用规则不参与() {
        let mut rule = replace_rule(1, "禁用的", "A", "B");
        rule.enabled = false;
        let purifier = Purifier::new(vec![rule], vec![], vec![]);
        assert_eq!(purifier.rule_counts().0, 0, "禁用的规则应在构造时被丢弃");
        let outcome = purifier
            .purify("A", PurifyTarget::Content, &[])
            .expect("应成功");
        assert_eq!(outcome.text, "A");
    }

    // --- 生效范围（FR-014 的 scope）---

    #[test]
    fn 范围相符的规则才生效() {
        let mut content_only = replace_rule(1, "去正文广告", "AD", "");
        content_only.scope = RuleScope::Content;
        let purifier = Purifier::new(vec![content_only], vec![], vec![]);

        assert_eq!(
            purifier
                .purify("正文AD", PurifyTarget::Content, &[])
                .expect("应成功")
                .text,
            "正文"
        );
        assert_eq!(
            purifier
                .purify_text_only("书名AD", PurifyTarget::BookName)
                .expect("应成功"),
            "书名AD",
            "Content 范围的规则不该作用于书名"
        );
    }

    #[test]
    fn 全局范围的规则作用于所有文本() {
        let mut global = replace_rule(1, "去尾部", "　完结", "");
        global.scope = RuleScope::Global;
        let purifier = Purifier::new(vec![global], vec![], vec![]);

        for target in [
            PurifyTarget::Content,
            PurifyTarget::BookName,
            PurifyTarget::ChapterTitle,
        ] {
            assert_eq!(
                purifier
                    .purify_text_only("书名　完结", target)
                    .expect("应成功"),
                "书名",
                "{target:?} 应被全局规则作用"
            );
        }
    }

    #[test]
    fn 作用范围判据() {
        assert!(applies_to(RuleScope::Global, PurifyTarget::BookName));
        assert!(applies_to(RuleScope::Content, PurifyTarget::Content));
        assert!(!applies_to(RuleScope::Content, PurifyTarget::ChapterTitle));
        assert!(!applies_to(RuleScope::BookName, PurifyTarget::Content));
    }

    // --- 高亮 ---

    #[test]
    fn 高亮规则产出字符区间() {
        let purifier = Purifier::new(vec![], vec![highlight_rule(1, "关键词", "red")], vec![]);
        let outcome = purifier
            .purify("前关键词后", PurifyTarget::Content, &[])
            .expect("应成功");
        assert_eq!(outcome.highlights.len(), 1);
        let range = &outcome.highlights[0];
        // "前" 是 1 个字符，故区间是 [1, 4)
        assert_eq!(range.start, 1);
        assert_eq!(range.end, 4);
        assert_eq!(range.style, "red");
    }

    #[test]
    fn 高亮区间转痕迹() {
        let purifier = Purifier::new(vec![], vec![highlight_rule(1, "关键词", "red")], vec![]);
        let outcome = purifier
            .purify("前关键词后", PurifyTarget::Content, &[])
            .expect("应成功");
        let traces = highlights_to_traces(&outcome, 7, 42);

        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].book_id, 7);
        assert_eq!(traces[0].chapter_index, 42);
        assert_eq!(traces[0].char_offset, 1, "字符区间，非渲染坐标（FR-028）");
        assert_eq!(traces[0].length, 3);
        assert_eq!(traces[0].style, "red");
        assert_eq!(traces[0].note, None, "备注是调用方的上下文，不由加工链产");
    }

    #[test]
    fn 高亮作用于替换后的文本() {
        // FR-014 的顺序：替换 → 高亮。若颠倒，高亮区间会指向被替换掉的
        // 旧文本，表现为"高亮位置偏移"
        let purifier = Purifier::new(
            vec![replace_rule(1, "去前缀", "PREFIX", "")],
            vec![highlight_rule(1, "词", "red")],
            vec![],
        );
        let outcome = purifier
            .purify("PREFIX词", PurifyTarget::Content, &[])
            .expect("应成功");
        assert_eq!(outcome.text, "词");
        assert_eq!(outcome.highlights.len(), 1);
        assert_eq!(
            outcome.highlights[0].start, 0,
            "高亮区间应基于替换**之后**的文本"
        );
    }

    // --- 词典 ---

    #[test]
    fn 词典规则产出查询请求而非释义() {
        // FR-016 是"给定词语产出释义结果"，而释义要发网络请求——
        // 加工链不发请求（A2），故它产出"要查什么"，由上层执行
        let purifier = Purifier::new(vec![], vec![], vec![dict_rule(1, "词典A")]);
        let outcome = purifier
            .purify("正文", PurifyTarget::Content, &["词一".to_string()])
            .expect("应成功");

        assert_eq!(outcome.dict_queries.len(), 1);
        assert_eq!(outcome.dict_queries[0].word, "词一");
        assert!(
            outcome.dict_queries[0].url_rule.contains("dict.test"),
            "应透传地址规则"
        );
        assert!(
            outcome.text.contains("正文"),
            "词典不修改正文（把正文改成释义显然不是意图）"
        );
    }

    #[test]
    fn 无词典规则时不产查询() {
        let purifier = Purifier::empty();
        let outcome = purifier
            .purify("正文", PurifyTarget::Content, &["词".to_string()])
            .expect("应成功");
        assert!(outcome.dict_queries.is_empty());
    }

    // --- 翻译 ---

    #[test]
    fn 翻译时带上人可读的规则标识() {
        // 存储主键是数字，而加工链的诊断标识要人可读
        let rule = replace_rule(7, "去广告", "A", "");
        let translated = translate_replace(&rule);
        assert!(translated.id.contains('7'));
        assert!(translated.id.contains("去广告"), "{}", translated.id);
    }

    #[test]
    fn 替换规则的正则标志被透传() {
        let mut rule = replace_rule(1, "正则", "\\d+", "");
        rule.is_regex = true;
        let purifier = Purifier::new(vec![rule], vec![], vec![]);
        let outcome = purifier
            .purify("abc123def", PurifyTarget::Content, &[])
            .expect("应成功");
        assert_eq!(outcome.text, "abcdef");
    }

    // --- 只做替换 ---

    #[test]
    fn 只做替换时不产高亮与词典() {
        // 书名与章节标题只需要净化，不需要区间与词典查询
        let purifier = Purifier::new(
            vec![replace_rule(1, "去广告", "(广告)", "")],
            vec![highlight_rule(1, "广告", "red")],
            vec![dict_rule(1, "词典")],
        );
        let text = purifier
            .purify_text_only("书名(广告)", PurifyTarget::Content)
            .expect("应成功");
        assert_eq!(text, "书名");
    }

    #[test]
    fn 规则数量诊断() {
        let purifier = Purifier::new(
            vec![replace_rule(1, "a", "a", "b")],
            vec![highlight_rule(1, "b", "red")],
            vec![dict_rule(1, "d")],
        );
        assert_eq!(purifier.rule_counts(), (1, 1, 1));
    }

    #[test]
    fn 构造是可复用的_多次求值结果一致() {
        // 阅读路径上 5 万章共用同一个净化器
        let purifier = Purifier::new(
            vec![replace_rule(1, "去广告", "AD", "")],
            vec![highlight_rule(1, "重点", "red")],
            vec![],
        );
        let first = purifier
            .purify("AD重点内容", PurifyTarget::Content, &[])
            .expect("应成功");
        for _ in 0..10 {
            let again = purifier
                .purify("AD重点内容", PurifyTarget::Content, &[])
                .expect("应成功");
            assert_eq!(again, first, "同一份规则多次求值必须一致");
        }
    }
}
