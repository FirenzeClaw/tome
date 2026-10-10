//! \[DONE] 合成差分语料的自检引擎（tasks.md T052 / SC-002a）
//!
//! 本模块是本阶段的**兼容性门**的判据来源：读一组合成用例，逐条跑规则，
//! 与期望值比对，产出通过率与逐条 diff。
//!
//! # 为什么放在 tome-rule 而不是 tome-cli
//!
//! 它要用**全部五个引擎**求值，而 CLI 是组合根（只做装配与 I/O）。
//! 把比对逻辑放这里有两个收益：
//!
//! 1. `tome-rule` 的测试能直接跑语料（不依赖 CLI 二进制）
//! 2. 引擎与它的验收标准在同一个 crate——改引擎时最容易看到语料的变化
//!
//! CLI 的 `verify` 子命令只负责**读文件、调本模块、格式化输出**。
//!
//! # 语料格式
//!
//! 见 `tests/corpus/README.md`。要点：每个用例有 `name` / `kind` / `input` /
//! `rule` / `expected` / `basis` 六个字段，且 **`basis` 必填**——tasks.md
//! T052 的原文要求"期望值依据成文，不靠实现者直觉填期望值"。
//! 缺 `basis` 的用例在此**被拒绝**，而不是静默通过。
//!
//! # 通过率的定义
//!
//! 一条用例通过当且仅当引擎输出与 `expected` **逐元素相等**（顺序敏感）。
//! 顺序敏感是有意的：书源规则里结果顺序决定"取第几个"。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::RuleError;

/// 一条合成用例（`tests/corpus/*.json` 的元素）。
#[derive(Debug, Clone, Deserialize)]
pub struct CorpusCase {
    /// 唯一标识（冒号前缀标语法类别，便于按类别统计）
    pub name: String,
    /// 输入类型：`html` 或 `json`
    pub kind: InputKind,
    /// 输入原文
    pub input: String,
    /// 规则原文
    pub rule: String,
    /// 期望的字符串列表
    pub expected: Vec<String>,
    /// **期望值的依据**（必填，见模块文档）
    pub basis: String,
    /// 是否反序（等价于规则前加 `-`）
    #[serde(default)]
    pub reversed: bool,
}

/// 输入类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    /// HTML 片段（走 HTML 解析，供 CSS/XPath 规则）
    Html,
    /// JSON 文本（供 `@json:` 规则）
    Json,
}

/// 一条用例的运行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseOutcome {
    /// 用例名
    pub name: String,
    /// 语法类别（从 `name` 的冒号前缀取；无前缀时是 `other`）
    pub category: String,
    /// 期望值
    pub expected: Vec<String>,
    /// 实际值（求值失败时为空，原因在 `error`）
    pub actual: Vec<String>,
    /// 求值失败的原因（`None` 表示求值本身成功）
    pub error: Option<String>,
}

impl CaseOutcome {
    /// 本用例是否通过。
    ///
    /// 通过 = 求值成功 **且** 输出与期望逐元素相等（顺序敏感）。
    #[must_use]
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.actual == self.expected
    }
}

/// 整个语料文件的运行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorpusReport {
    /// 语料文件的来源路径（报告里指明"跑的是哪一份"）
    pub source: String,
    /// 逐条结果（保持用例在文件里的顺序）
    pub outcomes: Vec<CaseOutcome>,
}

impl CorpusReport {
    /// 总条数。
    pub fn total(&self) -> usize {
        self.outcomes.len()
    }

    /// 通过条数。
    pub fn passed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|outcome| outcome.passed())
            .count()
    }

    /// 通过率（0.0–1.0）。空语料算 1.0（没有失败可言）。
    #[must_use]
    pub fn pass_rate(&self) -> f64 {
        if self.outcomes.is_empty() {
            return 1.0;
        }
        self.passed() as f64 / self.total() as f64
    }

    /// 通过率是否达标（SC-002a：≥95%）。
    #[must_use]
    pub fn meets_threshold(&self) -> bool {
        self.pass_rate() >= CORPUS_PASS_THRESHOLD
    }

    /// 未通过的用例（供逐条 diff 输出）。
    pub fn failures(&self) -> impl Iterator<Item = &CaseOutcome> {
        self.outcomes.iter().filter(|outcome| !outcome.passed())
    }

    /// 按语法类别统计（`类别 → (通过, 总数)`）。
    ///
    /// # 为什么需要分类统计
    ///
    /// 一个 95% 的总通过率可能掩盖"某一类语法全错"——比如 CSS 全对而
    /// XPath 全错，总数仍可能过线。分类统计让这种情形无处躲藏。
    #[must_use]
    pub fn by_category(&self) -> Vec<(String, usize, usize)> {
        let mut categories: Vec<(String, usize, usize)> = Vec::new();
        for outcome in &self.outcomes {
            let passed = usize::from(outcome.passed());
            match categories
                .iter_mut()
                .find(|(name, _, _)| *name == outcome.category)
            {
                Some((_, passed_count, total_count)) => {
                    *passed_count += passed;
                    *total_count += 1;
                }
                None => categories.push((outcome.category.clone(), passed, 1)),
            }
        }
        categories
    }
}

/// SC-002a 的阈值：合成语料差分通过率 ≥95%。
pub const CORPUS_PASS_THRESHOLD: f64 = 0.95;

/// 从 JSON 文本解析一组用例。
///
/// # 缺 `basis` 的用例被拒绝
///
/// 这是 tasks.md T052 "期望值依据成文"约束的**执行点**：`serde` 的反序列化
/// 会因缺字段而报错，故"没有依据的期望值"进不了语料。把约束交给类型系统
/// 而不是代码审查。
pub fn parse_corpus(json: &str) -> Result<Vec<CorpusCase>, RuleError> {
    let cases: Vec<CorpusCase> = serde_json::from_str(json)
        .map_err(|error| RuleError::syntax(format!("语料格式错误：{error}")))?;

    // 重名检查：重名会让报告里"哪条失败了"无从定位。
    for (position, case) in cases.iter().enumerate() {
        if cases[..position]
            .iter()
            .any(|other| other.name == case.name)
        {
            return Err(RuleError::syntax(format!(
                "语料里有重复的用例名 `{}`——重名会让失败报告无从定位",
                case.name
            )));
        }
        if case.basis.trim().is_empty() {
            return Err(RuleError::syntax(format!(
                "用例 `{}` 的 basis 为空——T052 要求期望值依据成文",
                case.name
            )));
        }
    }
    Ok(cases)
}

/// 跑一组用例。`js` 为 `None` 时 `@js:` 类用例会失败（原因会被记录）。
pub fn run_corpus(
    cases: &[CorpusCase],
    js: Option<&crate::js_bridge::JobRuntime>,
) -> Vec<CaseOutcome> {
    cases.iter().map(|case| run_case(case, js)).collect()
}

/// 跑单条用例。
pub fn run_case(case: &CorpusCase, js: Option<&crate::js_bridge::JobRuntime>) -> CaseOutcome {
    let category = case
        .name
        .split_once(':')
        .map_or_else(|| "other".to_string(), |(prefix, _)| prefix.to_string());

    let outcome = evaluate_case(case, js);
    match outcome {
        Ok(actual) => CaseOutcome {
            name: case.name.clone(),
            category,
            expected: case.expected.clone(),
            actual,
            error: None,
        },
        Err(error) => CaseOutcome {
            name: case.name.clone(),
            category,
            expected: case.expected.clone(),
            actual: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

/// 求值一条用例，产出规则结果。
fn evaluate_case(
    case: &CorpusCase,
    js: Option<&crate::js_bridge::JobRuntime>,
) -> Result<Vec<String>, RuleError> {
    // 反序标记：等价于规则前加 `-`（与 `parse_rule_tail` 的约定一致）
    let rule = if case.reversed {
        format!("-{}", case.rule)
    } else {
        case.rule.clone()
    };

    match case.kind {
        InputKind::Json => crate::jsonpath::evaluate(&case.input, strip_json_prefix(&rule)),
        InputKind::Html => {
            let document = crate::dom::Document::parse_html(&case.input);
            let engine = match js {
                Some(js) => crate::css_post::RuleEngine::new(&document).with_js(js),
                None => crate::css_post::RuleEngine::new(&document),
            };
            engine.evaluate(&rule)
        }
    }
}

/// 剥掉 `@json:` 前缀（JSONPath 引擎自己不需要它）。
fn strip_json_prefix(rule: &str) -> &str {
    let trimmed = rule.trim();
    for prefix in ["@json:", "@JSON:", "@Json:"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim();
        }
    }
    trimmed
}

// ---------------------------------------------------------------------------
// 目录加载
// ---------------------------------------------------------------------------

/// 从目录加载**全部** `.json` 语料文件。
///
/// # 为什么按文件名排序
///
/// 报告里的用例顺序由加载顺序决定，而目录枚举的顺序不保证稳定
/// （文件系统的实现细节）。排序让"同一份语料两次运行产出同一份报告"
/// 成立——那是差分测试的前提。
pub fn load_corpus_directory(root: &Path) -> Result<Vec<(PathBuf, Vec<CorpusCase>)>, RuleError> {
    let mut files: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(root).map_err(|error| {
        RuleError::syntax(format!("语料目录 `{}` 读取失败：{error}", root.display()))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            RuleError::syntax(format!("语料目录 `{}` 遍历失败：{error}", root.display()))
        })?;
        let path = entry.path();
        // 只收 `.json`：README.md 之类同目录的说明文件不该被当语料
        if path.extension().and_then(std::ffi::OsStr::to_str) == Some("json") {
            files.push(path);
        }
    }
    files.sort();

    let mut loaded = Vec::with_capacity(files.len());
    for path in files {
        let text = std::fs::read_to_string(&path).map_err(|error| {
            RuleError::syntax(format!("语料文件 `{}` 读取失败：{error}", path.display()))
        })?;
        let cases = parse_corpus(&text).map_err(|error| {
            RuleError::syntax(format!("语料文件 `{}` 解析失败：{error}", path.display()))
        })?;
        loaded.push((path, cases));
    }
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷：造一条用例。
    fn case(name: &str, kind: InputKind, input: &str, rule: &str, expected: &[&str]) -> CorpusCase {
        CorpusCase {
            name: name.to_string(),
            kind,
            input: input.to_string(),
            rule: rule.to_string(),
            expected: expected.iter().map(|text| (*text).to_string()).collect(),
            basis: "测试内置用例".to_string(),
            reversed: false,
        }
    }

    #[test]
    fn css用例能跑通() {
        let cases = vec![case(
            "css:类选择器与文本后缀",
            InputKind::Html,
            r#"<div class="n">x</div>"#,
            "@css:.n@text",
            &["x"],
        )];
        let outcomes = run_corpus(&cases, None);
        assert!(outcomes[0].passed());
        assert_eq!(outcomes[0].category, "css");
    }

    #[test]
    fn json用例能跑通() {
        let cases = vec![case(
            "jsonpath:取字段",
            InputKind::Json,
            r#"{"a":"b"}"#,
            "@json:$.a",
            &["b"],
        )];
        let outcomes = run_corpus(&cases, None);
        assert!(outcomes[0].passed(), "{:?}", outcomes[0]);
    }

    #[test]
    fn xpath用例能跑通() {
        let cases = vec![case(
            "xpath:后代轴",
            InputKind::Html,
            "<ul><li>a</li><li>b</li></ul>",
            "@XPath://li",
            &["a", "b"],
        )];
        let outcomes = run_corpus(&cases, None);
        assert!(outcomes[0].passed(), "{:?}", outcomes[0]);
    }

    #[test]
    fn 期望值不符时判为失败() {
        let cases = vec![case(
            "css:错期望",
            InputKind::Html,
            "<p>x</p>",
            "p@text",
            &["y"],
        )];
        let outcomes = run_corpus(&cases, None);
        assert!(!outcomes[0].passed());
        assert_eq!(outcomes[0].actual, vec!["x"]);
        assert!(
            outcomes[0].error.is_none(),
            "求值成功但期望不符，不是求值错"
        );
    }

    #[test]
    fn 求值失败被记录而非中断() {
        let cases = vec![
            case("css:坏规则", InputKind::Html, "<p>x</p>", "div >", &["x"]),
            case("css:好规则", InputKind::Html, "<p>x</p>", "p@text", &["x"]),
        ];
        let outcomes = run_corpus(&cases, None);
        assert!(!outcomes[0].passed());
        assert!(outcomes[0].error.is_some(), "求值失败应记录原因");
        assert!(outcomes[1].passed(), "坏用例不该影响后面的用例");
    }

    #[test]
    fn 顺序敏感() {
        let cases = vec![case(
            "css:顺序",
            InputKind::Html,
            "<ul><li>a</li><li>b</li></ul>",
            "li@text",
            &["b", "a"],
        )];
        let outcomes = run_corpus(&cases, None);
        assert!(!outcomes[0].passed(), "集合相同但顺序不同应判失败");
    }

    #[test]
    fn 反序标记生效() {
        let mut one = case(
            "css:反序",
            InputKind::Html,
            "<ul><li>a</li><li>b</li></ul>",
            "li@text",
            &["b", "a"],
        );
        one.reversed = true;
        let outcomes = run_corpus(&[one], None);
        assert!(outcomes[0].passed(), "{:?}", outcomes[0]);
    }

    #[test]
    fn 通过率与阈值() {
        let cases = vec![
            case("css:a", InputKind::Html, "<p>x</p>", "p@text", &["x"]),
            case("css:b", InputKind::Html, "<p>x</p>", "p@text", &["y"]),
        ];
        let report = CorpusReport {
            source: "内置".to_string(),
            outcomes: run_corpus(&cases, None),
        };
        assert_eq!(report.total(), 2);
        assert_eq!(report.passed(), 1);
        assert!((report.pass_rate() - 0.5).abs() < f64::EPSILON);
        assert!(!report.meets_threshold(), "50% 不达标");
    }

    #[test]
    fn 空语料通过率算满() {
        let report = CorpusReport {
            source: "内置".to_string(),
            outcomes: Vec::new(),
        };
        assert_eq!(report.pass_rate(), 1.0);
        assert!(report.meets_threshold());
    }

    #[test]
    fn 分类统计() {
        let cases = vec![
            case("css:a", InputKind::Html, "<p>x</p>", "p@text", &["x"]),
            case("css:b", InputKind::Html, "<p>x</p>", "p@text", &["y"]),
            case("xpath:a", InputKind::Html, "//x", "@XPath://x", &[]),
        ];
        let report = CorpusReport {
            source: "内置".to_string(),
            outcomes: run_corpus(&cases, None),
        };
        let stats = report.by_category();
        assert_eq!(stats[0], ("css".to_string(), 1, 2));
        assert_eq!(stats[1], ("xpath".to_string(), 1, 1));
    }

    #[test]
    fn 语料解析要求_basis() {
        // T052 的"期望值依据成文"由类型系统强制：缺 basis 直接反序列化失败
        let json = r#"[{"name":"a","kind":"html","input":"x","rule":"y","expected":[]}]"#;
        let error = parse_corpus(json).expect_err("缺 basis 应被拒绝");
        assert!(format!("{error}").contains("语料格式错误"));
    }

    #[test]
    fn 语料解析拒绝空_basis() {
        let json =
            r#"[{"name":"a","kind":"html","input":"x","rule":"y","expected":[],"basis":"  "}]"#;
        let error = parse_corpus(json).expect_err("空 basis 应被拒绝");
        assert!(format!("{error}").contains("basis"), "{error}");
    }

    #[test]
    fn 语料解析拒绝重名() {
        let json = r#"[
            {"name":"dup","kind":"html","input":"x","rule":"y","expected":[],"basis":"b"},
            {"name":"dup","kind":"html","input":"x","rule":"y","expected":[],"basis":"b"}
        ]"#;
        let error = parse_corpus(json).expect_err("重名应被拒绝");
        assert!(format!("{error}").contains("重复"), "{error}");
    }

    #[test]
    fn 语料解析接受完整格式() {
        let json = r#"[{
            "name": "css:x",
            "kind": "html",
            "input": "<p>a</p>",
            "rule": "p@text",
            "expected": ["a"],
            "basis": "Jsoup Selector 公开文档",
            "reversed": false
        }]"#;
        let cases = parse_corpus(json).expect("应解析成功");
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].kind, InputKind::Html);
        assert!(!cases[0].reversed);
    }

    #[test]
    fn json_前缀可剥() {
        assert_eq!(strip_json_prefix("@json:$.a"), "$.a");
        assert_eq!(strip_json_prefix("@JSON: $.a"), "$.a");
        assert_eq!(strip_json_prefix("$.a"), "$.a");
    }

    #[test]
    fn 不带_js_时_js_用例报能力错误() {
        let cases = vec![case(
            "js:计算",
            InputKind::Html,
            "<p>x</p>",
            "@js:1+1",
            &["2"],
        )];
        let outcomes = run_corpus(&cases, None);
        assert!(!outcomes[0].passed());
        let error = outcomes[0].error.as_deref().unwrap_or_default();
        assert!(error.contains("js"), "应指明 JS 能力缺失：{error}");
    }

    #[test]
    fn 带_js_时_js_用例通过() {
        let runtime =
            crate::js_bridge::JobRuntime::new(tome_js::Limits::default()).expect("应能建沙箱");
        let cases = vec![case(
            "js:计算",
            InputKind::Html,
            "<p>x</p>",
            "@js:1+1",
            &["2"],
        )];
        let outcomes = run_corpus(&cases, Some(&runtime));
        assert!(outcomes[0].passed(), "{:?}", outcomes[0]);
    }
}
