//! \[DONE] 内容加工链定序（tasks.md T051 / FR-014、FR-015、FR-016）
//!
//! # 规格原文与它定死的事
//!
//! spec.md 的 FR-014：
//!
//! > 系统 MUST 支持用户配置的正文净化/替换规则，按定义顺序应用；内容加工链的
//! > 顺序 MUST 确定为：**替换规则 → 高亮规则求值 → 词典规则求值**，
//! > 且该顺序**对所有消费者一致**。
//!
//! 三件事因此**不是**本模块的设计选择，而是必须承接的约束：
//!
//! | 约束 | 落点 |
//! |---|---|
//! | 顺序固定为 替换 → 高亮 → 词典 | [`ContentPipeline::run`] 的流水线结构 |
//! | 替换规则内按 `sort_order` 应用 | `ReplaceStage` 的排序 |
//! | 对所有消费者一致 | 唯一入口 [`ContentPipeline::run`]；无第二份实现 |
//!
//! # 为什么"顺序固定"值得一个模块
//!
//! 顺序错了**不会报错**，只会给出不同的结果：
//!
//! - 替换**先于**高亮：替换可能改变文本长度，故高亮的字符区间必须
//!   落在替换后的文本上（这正是它排在高亮之前的原因）。
//! - 高亮**先于**词典：高亮产出区间、词典产出释义，两者都不改文本；
//!   此顺序保证"高亮区间与词典查询针对的是**同一份文本**"。
//! - 若把高亮排到替换之前，替换一改长度，高亮区间就会整体偏移到错误的
//!   位置——而**没有任何错误消息**会提示这一点。
//!
//! 故本模块用类型把顺序固定下来：[`ContentPipeline::run`] 里三步是
//! 顺序语句，不存在"调用方自行拼装"的可能。
//!
//! # 单条规则出错**跳过并记录**，不中断其余（US5 场景 4）
//!
//! 一条坏规则（正则编译失败、模式为空）不该让整章正文加工失败——
//! 那会让用户失去整章内容，而代价仅是"这条规则没生效"。故三步都收集
//! [`PipelineDiagnostic`] 而不是向上抛错。
//!
//! ## 为什么本模块**不**报错而 [`crate::css_post`] 报错
//!
//! 两者面对的错误性质不同：
//!
//! | 场合 | 错误性质 | 取向 |
//! |---|---|---|
//! | 规则求值（`@css:` 选择器写错） | **书源**写错了，求不出结果 | 报错（书源要能发现） |
//! | 加工链（净化规则坏了） | **用户配置**坏了，但正文已取到 | 跳过（不能被一条规则毁掉整章） |
//!
//! # 字符区间而非渲染坐标（FR-028）
//!
//! 高亮产出的是 [`HighlightRange`]，字段是**字符区间**（`start`/`end`
//! 都是字符下标，非字节、非像素）。这是 AGENTS.md 的生产级护栏之一：
//! 改字号后不错位的前提。

use crate::error::RuleError;
use crate::regex::RegexLimits;

// ---------------------------------------------------------------------------
// 输入：三类规则的形态
// ---------------------------------------------------------------------------

/// 一条替换（净化）规则。
///
/// 字段与 `data-model.md` 的 `replace_rule` 表对齐：
/// `(id, name, pattern, replacement, scope, is_regex, enabled, sort_order)`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceRule {
    /// 规则 id（诊断用；用户看得懂的名字）
    pub id: String,
    /// 匹配模式。`is_regex` 为真时是正则，否则是字面串。
    pub pattern: String,
    /// 替换内容（支持 `$1` 之类的组引用，见 `crate::regex` 的替换串翻译层）
    pub replacement: String,
    /// 是否按正则解释 `pattern`
    pub is_regex: bool,
    /// 是否启用。**禁用的规则不参与**，单独一个字段而不是"调用方先过滤"：
    /// 过滤逻辑放这里才能保证所有消费者一致。
    pub enabled: bool,
    /// 应用顺序（**小者先**）。相同 `sort_order` 时按 `id` 稳定排序，
    /// 保证同一份配置的多次运行结果一致。
    pub sort_order: i64,
}

/// 一条高亮规则。
///
/// 对齐 `data-model.md` 的 `highlight_rule(id, name, pattern, style, enabled, sort_order)`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightRule {
    /// 规则 id
    pub id: String,
    /// 匹配模式（**正则**——高亮规则的语义就是"匹配到的地方高亮"）
    pub pattern: String,
    /// 样式标识（如 `"red"` / `"#ff0000"`）。本层不解释它，只透传。
    pub style: String,
    /// 是否启用
    pub enabled: bool,
    /// 应用顺序
    pub sort_order: i64,
}

/// 一条词典规则。
///
/// 对齐 `data-model.md` 的 `dict_rule(id, name, url_rule, content_rule, enabled)`。
///
/// # 为什么词典**不产出区间也不改文本**
///
/// FR-016 的原文是"给定词语产出释义结果"。词典是**按需查询**的：
/// 用户选中一个词，才去查释义。故它在加工链里的角色是"给定分词结果
/// 逐个查"，而不是"扫全文替换"——那样会把正文改成释义，显然不是意图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictRule {
    /// 规则 id
    pub id: String,
    /// 查词地址规则（含 `{{word}}` 占位的那种字符串，本层只透传不解释）
    pub url_rule: String,
    /// 释义内容的规则（本层只透传）
    pub content_rule: String,
    /// 是否启用
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// 输出：三类产物
// ---------------------------------------------------------------------------

/// 一处高亮（**字符区间**，FR-028 / FR-015）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightRange {
    /// 起始**字符**下标（含）
    pub start: usize,
    /// 结束**字符**下标（不含）
    pub end: usize,
    /// 样式（透传自规则）
    pub style: String,
    /// 产出的规则 id（诊断与"这条高亮哪来的"用）
    pub rule_id: String,
}

impl HighlightRange {
    /// 长度（字符数）。
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// 是否为空区间。
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// 一次词典查询请求。
///
/// # 为什么是"请求"而不是"结果"
///
/// 释义要发网络请求（`url_rule` 指向词典源），而**加工链不发请求**
/// （架构决策 A2：唯一发请求处是 `tome-source` 管线）。故本层产出
/// "要查哪些词、按哪条规则查"，由上层执行并回填释义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DictQuery {
    /// 词
    pub word: String,
    /// 用哪条词典规则
    pub rule_id: String,
    /// 查词地址规则（透传）
    pub url_rule: String,
    /// 释义内容规则（透传）
    pub content_rule: String,
}

/// 加工链的诊断记录（单条规则出错时的记录，不中断其余）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineDiagnostic {
    /// 出错阶段（`replace` / `highlight` / `dict`）
    pub stage: &'static str,
    /// 出错的规则 id
    pub rule_id: String,
    /// 可读原因
    pub message: String,
}

/// 加工链的完整产出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineOutcome {
    /// 加工后的正文（只有替换阶段会改它）
    pub text: String,
    /// 高亮区间（在 **`text` 上**的字符区间）
    pub highlights: Vec<HighlightRange>,
    /// 词典查询请求
    pub dict_queries: Vec<DictQuery>,
    /// 出错规则的记录（跳过但不静默）
    pub diagnostics: Vec<PipelineDiagnostic>,
}

// ---------------------------------------------------------------------------
// 流水线
// ---------------------------------------------------------------------------

/// 内容加工链。
///
/// # 用法
///
/// ```text
/// let pipeline = ContentPipeline::new()
///     .with_replace_rules(...)
///     .with_highlight_rules(...)
///     .with_dict_rules(...);
/// let outcome = pipeline.run(chapter_text, &["词1", "词2"]);
/// ```
///
/// # 为什么链式而不收一个配置结构
///
/// 三类规则相互独立（各自的表、各自的语义），用一个 `PipelineConfig`
/// 结构体只是把三个 `Vec` 装在一起，却让"只配替换规则"这种常见用法
/// 要写三个空字段。链式 setter 让调用方只写它有的那些。
#[derive(Debug, Clone, Default)]
pub struct ContentPipeline {
    replace_rules: Vec<ReplaceRule>,
    highlight_rules: Vec<HighlightRule>,
    dict_rules: Vec<DictRule>,
}

impl ContentPipeline {
    /// 空加工链（三步都跳过）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 装配替换（净化）规则。
    #[must_use]
    pub fn with_replace_rules(mut self, rules: Vec<ReplaceRule>) -> Self {
        self.replace_rules = rules;
        self
    }

    /// 装配高亮规则。
    #[must_use]
    pub fn with_highlight_rules(mut self, rules: Vec<HighlightRule>) -> Self {
        self.highlight_rules = rules;
        self
    }

    /// 装配词典规则。
    #[must_use]
    pub fn with_dict_rules(mut self, rules: Vec<DictRule>) -> Self {
        self.dict_rules = rules;
        self
    }

    /// 跑完整加工链。
    ///
    /// \[DONE] FR-014 的唯一落点：顺序固定为 **替换 → 高亮 → 词典**。
    ///
    /// # 参数
    ///
    /// - `text`：原始章节正文
    /// - `dictionary_words`：要查词典的词（通常来自上层的高亮/选中）。
    ///   加工链**不自己做分词**——分词的策略（按高亮区间？按用户选择？
    ///   按既有词典规则匹配？）属上层决策，本层只接收结果。
    ///
    /// # 返回值
    ///
    /// 三步的产物与诊断。任何一步的单条规则出错都只记录不中断。
    pub fn run(&self, text: &str, dictionary_words: &[String]) -> PipelineOutcome {
        let mut diagnostics = Vec::new();

        // ---- 第 1 步：替换（净化）----
        // 它排在最先，因为它**会改变文本长度**——高亮的字符区间必须落在
        // 替换之后的文本上，否则区间整体偏移（且不会有任何错误提示）。
        let processed = self.run_replace_stage(text, &mut diagnostics);

        // ---- 第 2 步：高亮求值 ----
        // 在**替换后的文本**上求区间（FR-015：产出与排版无关的字符区间）。
        let highlights = self.run_highlight_stage(&processed, &mut diagnostics);

        // ---- 第 3 步：词典求值 ----
        // 与前两步不共享状态（它按词查，不扫全文），但仍排在最后——
        // FR-014 定死了顺序，且"同一份文本"这一点要求它看到替换后的样子。
        let dict_queries = self.run_dict_stage(&processed, dictionary_words, &mut diagnostics);

        PipelineOutcome {
            text: processed,
            highlights,
            dict_queries,
            diagnostics,
        }
    }

    /// 第一步：按 `sort_order` 逐条应用替换规则。
    fn run_replace_stage(&self, text: &str, diagnostics: &mut Vec<PipelineDiagnostic>) -> String {
        let mut current = text.to_string();
        for rule in active_replace_rules(&self.replace_rules) {
            match apply_replace_rule(&current, rule) {
                Ok(updated) => current = updated,
                Err(error) => diagnostics.push(PipelineDiagnostic {
                    stage: "replace",
                    rule_id: rule.id.clone(),
                    message: error.to_string(),
                }),
            }
        }
        current
    }

    /// 第二步：在文本上求高亮区间。
    fn run_highlight_stage(
        &self,
        text: &str,
        diagnostics: &mut Vec<PipelineDiagnostic>,
    ) -> Vec<HighlightRange> {
        let mut highlights = Vec::new();
        for rule in active_highlight_rules(&self.highlight_rules) {
            match collect_highlights(text, rule) {
                Ok(mut found) => highlights.append(&mut found),
                Err(error) => diagnostics.push(PipelineDiagnostic {
                    stage: "highlight",
                    rule_id: rule.id.clone(),
                    message: error.to_string(),
                }),
            }
        }
        highlights
    }

    /// 第三步：为每个词产出查询请求。
    fn run_dict_stage(
        &self,
        _text: &str,
        words: &[String],
        diagnostics: &mut Vec<PipelineDiagnostic>,
    ) -> Vec<DictQuery> {
        let active: Vec<&DictRule> = self.dict_rules.iter().filter(|rule| rule.enabled).collect();
        let mut queries = Vec::new();
        for word in words {
            let trimmed = word.trim();
            if trimmed.is_empty() {
                // 空词不查：它没有意义，且会让词典源收到空查询。
                // 不算诊断（不是配置错，是调用方传了个空词）。
                continue;
            }
            if active.is_empty() {
                // 没有启用的词典规则：记一次诊断并停。
                // 只记一次（而不是每个词记一次）——那是同一个配置问题的重复。
                if queries.is_empty() {
                    diagnostics.push(PipelineDiagnostic {
                        stage: "dict",
                        rule_id: String::new(),
                        message: "没有启用的词典规则，跳过查词".to_string(),
                    });
                }
                break;
            }
            // 所有启用的词典规则都查一遍（多源交叉查是既有用法）
            for rule in &active {
                queries.push(DictQuery {
                    word: trimmed.to_string(),
                    rule_id: rule.id.clone(),
                    url_rule: rule.url_rule.clone(),
                    content_rule: rule.content_rule.clone(),
                });
            }
        }
        queries
    }
}

/// 取启用的替换规则，按 `sort_order` 升序（同序时按 id 稳定）。
///
/// # 为什么排两份（`sort_by_key` 而非 `sort_by`）
///
/// 需要"主序 sort_order、次序 id"的**稳定**排序。`sort_by` 的闭包要写
/// 一个比较函数，而这里用先按 id 排、再按 sort_order 稳定排的组合更直白
/// ——`sort_by_key` 是稳定排序，故第二次排不会打乱第一次排好的次序。
fn active_replace_rules(rules: &[ReplaceRule]) -> Vec<&ReplaceRule> {
    let mut active: Vec<&ReplaceRule> = rules.iter().filter(|rule| rule.enabled).collect();
    active.sort_by(|left, right| left.id.cmp(&right.id));
    active.sort_by_key(|rule| rule.sort_order);
    active
}

/// 取启用的高亮规则并按 `sort_order` 升序。
fn active_highlight_rules(rules: &[HighlightRule]) -> Vec<&HighlightRule> {
    let mut active: Vec<&HighlightRule> = rules.iter().filter(|rule| rule.enabled).collect();
    active.sort_by(|left, right| left.id.cmp(&right.id));
    active.sort_by_key(|rule| rule.sort_order);
    active
}

/// 应用一条替换规则。
fn apply_replace_rule(text: &str, rule: &ReplaceRule) -> Result<String, RuleError> {
    if rule.pattern.is_empty() {
        return Err(RuleError::syntax("替换规则的模式为空".to_string()));
    }
    if rule.is_regex {
        let compiled = crate::regex::compile(&rule.pattern, RegexLimits::DEFAULT)?;
        // 用 `replace_all_java` 而不是 `replace_all`：替换串是**书源/用户**
        // 写的 Java 风格（`$1`），必须先过翻译层。R5 点名的陷阱是
        // Rust 按最长名解析 `$1a` 会当成组名 `1a` → 静默丢字不报错。
        compiled.replace_all_java(text, &rule.replacement, RegexLimits::DEFAULT)
    } else {
        // 字面替换：语义是"全部替换"（用户的直觉如此，且正则路径也是全部）。
        Ok(text.replace(&rule.pattern, &rule.replacement))
    }
}

/// 在文本上求一条高亮规则的全部区间。
///
/// # 字符区间而非字节
///
/// `regex` 的 `find_iter` 给的是**字节**偏移，而 FR-028 要求字符区间
/// （改字号不错位的前提，且"字符"才是用户看到的位置）。故此处把字节
/// 偏移换算成字符下标——换算在 [`RegexMatch`] 层做（见其文档）。
fn collect_highlights(text: &str, rule: &HighlightRule) -> Result<Vec<HighlightRange>, RuleError> {
    if rule.pattern.is_empty() {
        return Err(RuleError::syntax("高亮规则的模式为空".to_string()));
    }
    let compiled = crate::regex::compile(&rule.pattern, RegexLimits::DEFAULT)?;
    let matches = compiled.find_iter(text, RegexLimits::DEFAULT)?;
    Ok(matches
        .into_iter()
        .filter(|found| found.end_char > found.start_char)
        .map(|found| HighlightRange {
            start: found.start_char,
            end: found.end_char,
            style: rule.style.clone(),
            rule_id: rule.id.clone(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷：一条正则替换规则。
    fn replace(id: &str, pattern: &str, replacement: &str, order: i64) -> ReplaceRule {
        ReplaceRule {
            id: id.to_string(),
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            is_regex: true,
            enabled: true,
            sort_order: order,
        }
    }

    /// 便捷：一条高亮规则。
    fn highlight(id: &str, pattern: &str, style: &str, order: i64) -> HighlightRule {
        HighlightRule {
            id: id.to_string(),
            pattern: pattern.to_string(),
            style: style.to_string(),
            enabled: true,
            sort_order: order,
        }
    }

    /// 便捷：一条词典规则。
    fn dict(id: &str) -> DictRule {
        DictRule {
            id: id.to_string(),
            url_rule: format!("https://dict.test/{id}?q={{{{word}}}}"),
            content_rule: "@css:.meaning@text".to_string(),
            enabled: true,
        }
    }

    // ---------- 顺序（FR-014 的核心） ----------

    #[test]
    fn 加工链顺序是替换然后高亮然后词典() {
        // 替换把 "abc" 变成 "abcdef"（变长了）；高亮若在替换**之前**求，
        // 区间会落在旧文本上——用一条匹配替换后文本的模式来暴露这一点。
        let pipeline = ContentPipeline::new()
            .with_replace_rules(vec![replace("r1", "abc", "abcXXXXX", 0)])
            .with_highlight_rules(vec![highlight("h1", "XXXXX", "red", 0)]);

        let outcome = pipeline.run("abc", &[]);
        assert_eq!(outcome.text, "abcXXXXX", "替换先生效");
        assert_eq!(
            outcome.highlights.len(),
            1,
            "高亮应在替换后的文本上找到目标"
        );
        assert_eq!(
            outcome.highlights[0],
            HighlightRange {
                start: 3,
                end: 8,
                style: "red".to_string(),
                rule_id: "h1".to_string(),
            },
            "区间落在替换**之后**的文本位置"
        );
    }

    #[test]
    fn 高亮排在替换之前会得到错位区间() {
        // 反证：若把高亮排到替换之前，区间会是 0..5（旧文本的 XXXXX 不存在，
        // 故其实一个也找不到）。本测试用"替换缩短文本"来暴露错位的方向。
        //
        // 输入 "a--b"：替换把 "--" 去掉得到 "ab"。
        // 高亮 `b` 在替换后是 1..2；若在替换前求，`b` 也在 2..3。
        let pipeline = ContentPipeline::new()
            .with_replace_rules(vec![replace("r1", "--", "", 0)])
            .with_highlight_rules(vec![highlight("h1", "b", "red", 0)]);
        let outcome = pipeline.run("a--b", &[]);
        assert_eq!(outcome.text, "ab");
        assert_eq!(
            outcome.highlights[0].start, 1,
            "b 在缩短后的位置是 1 而不是 2"
        );
        assert_eq!(outcome.highlights[0].end, 2);
    }

    #[test]
    fn 词典在最后且看到替换后的词表() {
        let pipeline = ContentPipeline::new()
            .with_replace_rules(vec![replace("r1", "错误词", "正确词", 0)])
            .with_dict_rules(vec![dict("d1")]);
        // 查的是替换后的词——由调用方传（本层不做分词，见 run 的文档）
        let outcome = pipeline.run("错误词", &["正确词".to_string()]);
        assert_eq!(outcome.text, "正确词");
        assert_eq!(outcome.dict_queries.len(), 1);
        assert_eq!(outcome.dict_queries[0].word, "正确词");
    }

    #[test]
    fn 三步都为空时原样返回() {
        let outcome = ContentPipeline::new().run("原文", &[]);
        assert_eq!(outcome.text, "原文");
        assert!(outcome.highlights.is_empty());
        assert!(outcome.dict_queries.is_empty());
        assert!(outcome.diagnostics.is_empty());
    }

    // ---------- 替换规则的应用顺序 ----------

    #[test]
    fn 替换规则按sort_order升序应用() {
        // 先去掉 `--`，再把 `-` 换成 `+`。按 sort_order 从小到大：
        // "a--b-c" → "ab-c" → "ab+c"
        let pipeline = ContentPipeline::new().with_replace_rules(vec![
            replace("second", "-", "+", 1),
            replace("first", "--", "", 0),
        ]);
        let outcome = pipeline.run("a--b-c", &[]);
        assert_eq!(outcome.text, "ab+c", "小 sort_order 先跑");
    }

    #[test]
    fn 同sort_order时按id稳定排序() {
        // 两条规则的 sort_order 相同，顺序由 id 决定（稳定、可复现）
        let pipeline = ContentPipeline::new()
            .with_replace_rules(vec![replace("b", "x", "y", 0), replace("a", "y", "z", 0)]);
        // id "a" 先跑：先把 y 变 z（此时没有 y，无变化），再 x→y
        assert_eq!(pipeline.run("x", &[]).text, "y");
        // 反过来给（顺序不影响结果，因为排序是稳定的）
        let pipeline = ContentPipeline::new()
            .with_replace_rules(vec![replace("a", "y", "z", 0), replace("b", "x", "y", 0)]);
        assert_eq!(pipeline.run("x", &[]).text, "y");
    }

    #[test]
    fn 按定义顺序应用是可测的() {
        // FR-014：「按定义顺序应用」。顺序不同结果不同的经典例子：
        //
        // 正序（A→AB 先、B→C 后）："A" → "AB" → "AC"
        //   —— 第一条规则**新引入的** B 会被第二条规则命中。
        // 逆序（B→C 先、A→AB 后）："A" → "A"（没有 B）→ "AB"
        //   —— 第一条规则无对象可改，故结果里 B 被保留。
        //
        // 两条路给出 "AC" 与 "AB"，不同且都能预期——这就是"顺序可测"。
        let forward = ContentPipeline::new().with_replace_rules(vec![
            replace("r1", "A", "AB", 0),
            replace("r2", "B", "C", 1),
        ]);
        assert_eq!(
            forward.run("A", &[]).text,
            "AC",
            "后一条规则能改前一条刚引入的内容"
        );

        let reversed = ContentPipeline::new().with_replace_rules(vec![
            replace("r2", "B", "C", 0),
            replace("r1", "A", "AB", 1),
        ]);
        assert_eq!(
            reversed.run("A", &[]).text,
            "AB",
            "顺序反了结果不同（B 是最后引入的，不再被改）"
        );
    }

    #[test]
    fn 禁用的替换规则不生效() {
        let mut rule = replace("r1", "a", "b", 0);
        rule.enabled = false;
        let pipeline = ContentPipeline::new().with_replace_rules(vec![rule]);
        assert_eq!(pipeline.run("a", &[]).text, "a", "禁用的规则不参与");
    }

    #[test]
    fn 字面替换模式可用() {
        let rule = ReplaceRule {
            id: "literal".to_string(),
            // 正则里 `.` 是元字符；`is_regex: false` 时它应是字面点
            pattern: "a.c".to_string(),
            replacement: "X".to_string(),
            is_regex: false,
            enabled: true,
            sort_order: 0,
        };
        let pipeline = ContentPipeline::new().with_replace_rules(vec![rule]);
        // 字面匹配：只有 "a.c" 被替换，"abc" 不动
        assert_eq!(pipeline.run("a.c abc", &[]).text, "X abc");
    }

    #[test]
    fn 替换支持组引用() {
        // `crate::regex` 的替换串翻译层负责 `$1` → `${1}`（java 语义差异）
        let rule = ReplaceRule {
            id: "group".to_string(),
            pattern: r"(\d+)\.(\d+)".to_string(),
            replacement: "$2-$1".to_string(),
            is_regex: true,
            enabled: true,
            sort_order: 0,
        };
        let pipeline = ContentPipeline::new().with_replace_rules(vec![rule]);
        assert_eq!(pipeline.run("3.14", &[]).text, "14-3");
    }

    // ---------- 高亮：字符区间 ----------

    #[test]
    fn 高亮产出字符区间而非字节() {
        // FR-028：「阅读痕迹禁止存渲染坐标」——用**字符**区间，改字号不错位。
        // 中文是高字节的，用它可以区分"字符下标"与"字节下标"。
        let pipeline =
            ContentPipeline::new().with_highlight_rules(vec![highlight("h1", "世界", "red", 0)]);
        let text = "你好世界啊";
        let outcome = pipeline.run(text, &[]);
        assert_eq!(outcome.highlights.len(), 1);
        let range = &outcome.highlights[0];
        assert_eq!(
            (range.start, range.end),
            (2, 4),
            "字符区间（不是字节 6..12）"
        );
        // 用区间切出的正是目标子串——这是"区间正确"的直接证据
        let slice: String = text.chars().skip(range.start).take(range.len()).collect();
        assert_eq!(slice, "世界");
    }

    #[test]
    fn 一条规则命中多处() {
        let pipeline =
            ContentPipeline::new().with_highlight_rules(vec![highlight("h1", "a", "red", 0)]);
        let outcome = pipeline.run("aba", &[]);
        assert_eq!(outcome.highlights.len(), 2);
        assert_eq!(
            outcome
                .highlights
                .iter()
                .map(|range| (range.start, range.end))
                .collect::<Vec<_>>(),
            vec![(0, 1), (2, 3)]
        );
    }

    #[test]
    fn 多条高亮规则各自产出区间() {
        let pipeline = ContentPipeline::new().with_highlight_rules(vec![
            highlight("h1", "a", "red", 0),
            highlight("h2", "b", "blue", 1),
        ]);
        let outcome = pipeline.run("ab", &[]);
        assert_eq!(outcome.highlights.len(), 2);
        assert_eq!(outcome.highlights[0].style, "red");
        assert_eq!(outcome.highlights[1].style, "blue");
    }

    #[test]
    fn 禁用的高亮规则不生效() {
        let mut rule = highlight("h1", "a", "red", 0);
        rule.enabled = false;
        let pipeline = ContentPipeline::new().with_highlight_rules(vec![rule]);
        assert!(pipeline.run("a", &[]).highlights.is_empty());
    }

    #[test]
    fn 空区间被过滤掉() {
        // 零宽匹配（如模式 `x*`）不该产出高亮——空区间在排版上无意义，
        // 且在 UI 里会变成看不见但可点的幽灵
        let pipeline =
            ContentPipeline::new().with_highlight_rules(vec![highlight("h1", "x*", "red", 0)]);
        let outcome = pipeline.run("abc", &[]);
        assert!(
            outcome.highlights.iter().all(|range| !range.is_empty()),
            "不该有零宽区间：{:?}",
            outcome.highlights
        );
    }

    // ---------- 词典 ----------

    #[test]
    fn 词典为每个词每条规则产出请求() {
        let pipeline = ContentPipeline::new().with_dict_rules(vec![dict("d1"), dict("d2")]);
        let outcome = pipeline.run("x", &["词".to_string()]);
        assert_eq!(outcome.dict_queries.len(), 2, "两个词典源各查一次");
        assert_eq!(outcome.dict_queries[0].word, "词");
        assert_eq!(outcome.dict_queries[0].rule_id, "d1");
        assert_eq!(outcome.dict_queries[1].rule_id, "d2");
    }

    #[test]
    fn 空词不查且不算诊断() {
        let pipeline = ContentPipeline::new().with_dict_rules(vec![dict("d1")]);
        let outcome = pipeline.run("x", &["".to_string(), "   ".to_string()]);
        assert!(outcome.dict_queries.is_empty());
        assert!(
            outcome.diagnostics.is_empty(),
            "空词是调用方传的，不是配置问题"
        );
    }

    #[test]
    fn 词首尾空白被去掉() {
        let pipeline = ContentPipeline::new().with_dict_rules(vec![dict("d1")]);
        let outcome = pipeline.run("x", &["  词  ".to_string()]);
        assert_eq!(outcome.dict_queries[0].word, "词");
    }

    #[test]
    fn 无词典规则时记一次诊断() {
        let pipeline = ContentPipeline::new();
        let outcome = pipeline.run("x", &["词".to_string()]);
        assert!(outcome.dict_queries.is_empty());
        assert_eq!(outcome.diagnostics.len(), 1, "只记一次而非每词一次");
        assert_eq!(outcome.diagnostics[0].stage, "dict");
    }

    // ---------- 单条出错跳过不中断（US5 场景 4） ----------

    #[test]
    fn 坏替换规则被跳过而其余仍生效() {
        let pipeline = ContentPipeline::new().with_replace_rules(vec![
            replace("bad", "(未闭合", "x", 0), // 非法正则
            replace("good", "a", "b", 1),
        ]);
        let outcome = pipeline.run("a", &[]);
        assert_eq!(outcome.text, "b", "好规则仍生效——单条坏不毁全部");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(outcome.diagnostics[0].stage, "replace");
        assert_eq!(outcome.diagnostics[0].rule_id, "bad");
        assert!(
            !outcome.diagnostics[0].message.is_empty(),
            "诊断要有可读原因"
        );
    }

    #[test]
    fn 坏高亮规则被跳过而其余仍生效() {
        let pipeline = ContentPipeline::new().with_highlight_rules(vec![
            highlight("bad", "(未闭合", "red", 0),
            highlight("good", "a", "blue", 1),
        ]);
        let outcome = pipeline.run("a", &[]);
        assert_eq!(outcome.highlights.len(), 1);
        assert_eq!(outcome.highlights[0].rule_id, "good");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(outcome.diagnostics[0].stage, "highlight");
    }

    #[test]
    fn 空模式被跳过并记录() {
        let pipeline = ContentPipeline::new()
            .with_replace_rules(vec![replace("empty", "", "x", 0)])
            .with_highlight_rules(vec![highlight("empty", "", "red", 0)]);
        let outcome = pipeline.run("a", &[]);
        assert_eq!(outcome.text, "a");
        assert!(outcome.highlights.is_empty());
        assert_eq!(outcome.diagnostics.len(), 2, "替换与高亮各记一次");
    }

    #[test]
    fn 全部规则都坏时文本原样返回() {
        let pipeline = ContentPipeline::new().with_replace_rules(vec![
            replace("bad1", "(", "x", 0),
            replace("bad2", "[", "y", 1),
        ]);
        let outcome = pipeline.run("原文", &[]);
        assert_eq!(outcome.text, "原文", "正文不能被坏规则毁掉");
        assert_eq!(outcome.diagnostics.len(), 2);
    }

    // ---------- 所有消费者一致 ----------

    #[test]
    fn 同一份配置两次运行结果一致() {
        // "对所有消费者一致"的可测形式：确定性。同一份输入与配置，
        // 多次运行必须给出完全相同的输出（含诊断顺序）。
        let build = || {
            ContentPipeline::new()
                .with_replace_rules(vec![
                    replace("r2", "b", "B", 1),
                    replace("r1", "a", "A", 0),
                    replace("bad", "(", "x", 2),
                ])
                .with_highlight_rules(vec![highlight("h1", "[AB]", "red", 0)])
                .with_dict_rules(vec![dict("d1")])
        };
        let first = build().run("ab", &["词".to_string()]);
        let second = build().run("ab", &["词".to_string()]);
        assert_eq!(first, second, "同输入同配置必须完全一致");
    }

    #[test]
    fn 诊断顺序与规则应用顺序一致() {
        let pipeline = ContentPipeline::new().with_replace_rules(vec![
            replace("bad-late", "(", "x", 5),
            replace("bad-early", "[", "y", 1),
        ]);
        let outcome = pipeline.run("a", &[]);
        assert_eq!(outcome.diagnostics.len(), 2);
        assert_eq!(
            outcome.diagnostics[0].rule_id, "bad-early",
            "诊断按应用顺序排列（sort_order 小的在前）"
        );
        assert_eq!(outcome.diagnostics[1].rule_id, "bad-late");
    }
}
