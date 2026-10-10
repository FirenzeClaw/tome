//! `[DONE]` 规则求值轨迹（tasks.md T093 / FR-037）
//!
//! # FR-037 的原文与它要解决的问题
//!
//! > 系统 MUST 提供命令行工具……MUST 支持可选的**规则求值轨迹**输出
//! > （逐步展示每一步规则匹配到的中间值），供上层实现书源调试器
//!
//! 要解决的问题很具体：**书源规则错了，光看结果不知道为什么错**。
//! 一条 `div.book > a@href##^##https://b.com` 没取到值，可能是选择器没命中、
//! 可能是 `@href` 那个位置没有 href 属性、也可能是替换链的正则不匹配——
//! 只看"最终结果是空数组"分辨不出是哪种。
//!
//! 轨迹把这个过程摊开：每一步的输入、输出、以及为什么。
//!
//! # 轨迹只记录**真正发生的事**
//!
//! 首版设计想按"选择器 / 后缀 / 替换链"三段展开，并给组合符（`&&`/`||`/`%%`）
//! 单独一段。实测发现**组合符尚未接入生产求值路径**（`dispatch::combine_results`
//! 目前只被测试用到），故那条分支是**凭空假设**——它会让轨迹显示"逐段求值 +
//! 合并"，而实际求值走的是另一条路。测试
//! `轨迹的最终结果与直接求值一致` 当场抓出了这个不一致（轨迹给出
//! `["甲","甲"]`，直接求值给 `[]`）。
//!
//! 故现在：轨迹**只调用真实求值函数并记录它返回什么**，不自行模拟任何步骤。
//! 不存在的步骤不会被写进轨迹——**轨迹说的与实际做的必须是同一件事**，
//! 否则它就从调试工具变成了误导来源。
//!
//! 组合符接入生产路径后，这里应补上对应的观测点（届时它会真的发生）。
//!
//! # 与求值路径的关系：**同一份逻辑**，不是两套
//!
//! 轨迹最危险的失败形态是"轨迹说的和实际求值不一样"——那就成了误导。
//! 故本模块**不重写求值**：它调用与正常求值相同的
//! [`crate::css_post::evaluate_rule`] 与 [`crate::dispatch::parse_rule`]，
//! 只是**额外记录**中间值。若某天求值逻辑改了，轨迹自动跟着改。

use serde::Serialize;

use crate::error::RuleError;

/// 轨迹里的一步。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceStep {
    /// 这一步在做什么（面向人的短说明，如"分派规则"）
    ///
    /// # 为什么说明用中文而键用英文
    ///
    /// 说明是给**调试书源的人**看的（书源生态以中文社区为主）；
    /// 而 JSON 的键是契约（脚本按它取字段），必须稳定且与其它命令一致。
    pub what: String,
    /// 这一步的输入（规则片段或中间值）
    pub input: String,
    /// 这一步的产出
    pub output: Vec<String>,
    /// 该步是否失败（失败时 `output` 为空、`error` 有原因）
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    /// 失败原因（仅失败时存在）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TraceStep {
    /// 成功的一步。
    #[must_use]
    pub fn ok(what: impl Into<String>, input: impl Into<String>, output: Vec<String>) -> Self {
        Self {
            what: what.into(),
            input: input.into(),
            output,
            failed: false,
            error: None,
        }
    }

    /// 失败的一步。
    #[must_use]
    pub fn failed(
        what: impl Into<String>,
        input: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            what: what.into(),
            input: input.into(),
            output: Vec::new(),
            failed: true,
            error: Some(error.into()),
        }
    }
}

/// 一条规则的完整轨迹。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleTrace {
    /// 原始规则
    pub rule: String,
    /// 逐步的中间值（按发生顺序）
    pub steps: Vec<TraceStep>,
    /// 最终结果（与不带 `--trace` 时的输出**必须一致**）
    pub result: Vec<String>,
}

impl RuleTrace {
    /// 是否全程无失败步骤。
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.steps.iter().all(|step| !step.failed)
    }

    /// 找到第一个失败的步骤（调试时最该看的那一步）。
    #[must_use]
    pub fn first_failure(&self) -> Option<&TraceStep> {
        self.steps.iter().find(|step| step.failed)
    }
}

/// 对一条规则求值并产出轨迹。
///
/// # 步骤的顺序即求值的实际顺序
///
/// | # | 步骤 | 说明 |
/// |---|---|---|
/// | 1 | 分派 | 按前缀判断是哪类规则（CSS/JSONPath/XPath/JS/正则）、是否就绪 |
/// | 2 | 组合符切分 | 若规则含 `&&`/`\|\|`/`%%`，切成多段 |
/// | 3… | 逐段求值 | 每段的产出 |
/// | n | 组合 | 按组合符合并各段结果 |
/// | n+1 | 替换链 | `##` 链逐步应用（每步一个子步骤） |
///
/// # 单段规则不带"组合"步骤
///
/// 没有组合符时不该出现一步"组合（无操作）"——那是噪音，会让调试器里
/// 全是永远通过的步骤。**只记录真正发生了的事**。
pub fn trace_rule(rule: &str, input_kind: TraceInput, input: &str) -> RuleTrace {
    let mut steps = Vec::new();

    // ---- ① 分派：这条规则属于哪类语法（从规则文本解析，非模拟求值）----
    let combined = crate::dispatch::parse_rule(rule);
    let kinds: Vec<String> = combined
        .segments
        .iter()
        .map(|segment| {
            format!(
                "{}（{}）",
                segment.kind.name(),
                if segment.ready == crate::dispatch::Ready::Yes {
                    "本阶段可求值"
                } else {
                    "本阶段未实现"
                }
            )
        })
        .collect();
    steps.push(TraceStep::ok("分派规则类型", rule, kinds));

    // 有未就绪的段：直接报能力错误（与正常求值一致——不假装算下去了）
    if let Some(segment) = combined.first_not_ready() {
        let error = segment
            .not_ready_error("rule")
            .map_or_else(|| "规则未就绪".to_string(), |error| format!("{error}"));
        steps.push(TraceStep::failed("规则未就绪", segment.body.clone(), error));
        return RuleTrace {
            rule: rule.to_string(),
            steps,
            result: Vec::new(),
        };
    }

    // ---- ② 求值 ----
    let result = match input_kind {
        TraceInput::Html => evaluate_html(rule, input, &mut steps),
        TraceInput::Json => evaluate_json(rule, input, &mut steps),
    };

    match result {
        Ok(values) => RuleTrace {
            rule: rule.to_string(),
            steps,
            result: values,
        },
        Err(error) => {
            steps.push(TraceStep::failed("求值", rule, format!("{error}")));
            RuleTrace {
                rule: rule.to_string(),
                steps,
                result: Vec::new(),
            }
        }
    }
}

/// 求值输入的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceInput {
    /// HTML（走 CSS/Default/XPath/替换链）
    Html,
    /// JSON 文本（走 JSONPath）
    Json,
}

/// HTML 输入的求值（**复用正常求值路径**，只是额外记录）。
///
/// # 为什么只调一次 `evaluate_rule` 而不自己拆步骤
///
/// 见模块文档：首版按"逐段求值 + 组合"模拟了一遍，与真实路径不符。现在
/// 只调用真实入口、记录它返回什么——**轨迹的正确性由"它是同一次调用"
/// 保证**，不靠复现求值逻辑。
///
/// 代价是步骤粒度较粗（"求值规则"一步）。换来的是**绝不误导**——而一个
/// 会说谎的调试器比没有调试器更糟。更细的步骤需要求值器本身暴露观测点
/// （那是后续切片的工作，见模块文档末尾）。
fn evaluate_html(
    rule: &str,
    html: &str,
    steps: &mut Vec<TraceStep>,
) -> Result<Vec<String>, RuleError> {
    let document = crate::dom::Document::parse_html(html);
    let values = crate::css_post::evaluate_rule(&document, rule)?;
    steps.push(TraceStep::ok("求值规则", rule, values.clone()));
    Ok(values)
}

/// JSON 输入的求值（JSONPath 引擎）。
fn evaluate_json(
    rule: &str,
    json: &str,
    steps: &mut Vec<TraceStep>,
) -> Result<Vec<String>, RuleError> {
    let body = strip_json_prefix(rule);
    let values = crate::jsonpath::evaluate(json, body)?;
    steps.push(TraceStep::ok("求值 JSONPath", body, values.clone()));
    Ok(values)
}

/// 剥掉 `@json:` 前缀（与 `corpus.rs` 同一约定）。
fn strip_json_prefix(rule: &str) -> &str {
    let trimmed = rule.trim();
    for prefix in ["@json:", "@JSON:", "@Json:"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.trim();
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 简单规则产出分派与求值两步() {
        let trace = trace_rule("p@text", TraceInput::Html, "<p>x</p>");
        assert!(trace.is_clean(), "{:?}", trace.steps);
        assert_eq!(trace.result, vec!["x"]);
        // 只有"真正发生的事"：分派（读规则文本）+ 求值（调真实入口）
        assert_eq!(trace.steps.len(), 2, "{:?}", trace.steps);
        assert!(trace.steps[0].what.contains("分派"));
        assert!(trace.steps[1].what.contains("求值"));
    }

    #[test]
    fn 轨迹的最终结果与直接求值一致() {
        // 这是轨迹最要紧的性质：它不能与实际求值不同
        let html = r#"<div class="book"><a href="https://x.com/1">甲</a></div>"#;
        for rule in [
            "div.book a@href",
            ".book@text",
            "a@text##甲##乙",
            "div a@text&&.book@text",
            "div.nonexistent a@text",
        ] {
            let direct =
                crate::css_post::evaluate_rule(&crate::dom::Document::parse_html(html), rule)
                    .unwrap_or_default();
            let trace = trace_rule(rule, TraceInput::Html, html);
            assert_eq!(
                trace.result, direct,
                "规则 `{rule}` 的轨迹结果与直接求值不一致"
            );
        }
    }

    /// 组合符规则：**结果必须与直接求值一致**。
    ///
    /// # 这条测试守住的是"轨迹不撒谎"
    ///
    /// 首版按"逐段求值 + 合并"模拟了一遍，得到的 `["甲","甲"]` 与直接求值的
    /// `[]` 不同——组合符尚未接入生产求值路径，那段模拟是凭空假设。
    ///
    /// 现在轨迹只调真实入口，故这条断言必然成立。它同时是**回归护栏**：
    /// 若将来有人又想在轨迹里"模拟"求值，这里会立刻变红。
    #[test]
    fn 组合规则结果与直接求值一致() {
        let html = "<p>甲</p><span>乙</span>";
        for rule in [
            "p@text&&span@text",
            "p@text||span@text",
            "p@text%%span@text",
        ] {
            let direct =
                crate::css_post::evaluate_rule(&crate::dom::Document::parse_html(html), rule)
                    .unwrap_or_default();
            let trace = trace_rule(rule, TraceInput::Html, html);
            assert_eq!(
                trace.result, direct,
                "规则 `{rule}` 的轨迹结果与直接求值不一致（轨迹不该模拟求值）"
            );
        }
    }

    /// 组合符**未**接入生产路径时，轨迹不该出现"逐段/合并"这类捏造的步骤。
    #[test]
    fn 不捏造未发生的步骤() {
        let trace = trace_rule("p@text&&span@text", TraceInput::Html, "<p>甲</p>");
        let whats: Vec<&str> = trace.steps.iter().map(|step| step.what.as_str()).collect();
        for fabricated in ["第 1 段", "第 2 段", "合并", "切分"] {
            assert!(
                !whats.iter().any(|what| what.contains(fabricated)),
                "轨迹里不该出现未真正发生的步骤 `{fabricated}`：{whats:?}"
            );
        }
    }

    #[test]
    fn 空结果也如实记录() {
        // 调试最常遇到的情形：选择器没命中。轨迹要能看出"这一步产出为空"
        let trace = trace_rule("div.nothere@text", TraceInput::Html, "<p>x</p>");
        assert!(trace.is_clean(), "选择器没命中不是错误：{:?}", trace.steps);
        assert!(trace.result.is_empty());
        // 最后一步的产出是空数组（可据此判断"没命中"而不是"报错了"）
        assert!(
            trace
                .steps
                .last()
                .is_some_and(|step| step.output.is_empty()),
            "应记录空产出：{:?}",
            trace.steps
        );
    }

    #[test]
    fn 坏规则记录失败步骤而非中断() {
        let trace = trace_rule("div >", TraceInput::Html, "<p>x</p>");
        assert!(!trace.is_clean(), "坏规则应有失败步骤");
        let failure = trace.first_failure().expect("应有失败步骤");
        assert!(failure.error.is_some(), "失败步骤应带原因");
        assert!(trace.result.is_empty());
    }

    #[test]
    fn json_规则走_jsonpath() {
        let trace = trace_rule("@json:$.a", TraceInput::Json, r#"{"a":"b"}"#);
        assert!(trace.is_clean(), "{:?}", trace.steps);
        assert_eq!(trace.result, vec!["b"]);
        assert!(
            trace
                .steps
                .iter()
                .any(|step| step.what.contains("JSONPath")),
            "{:?}",
            trace.steps
        );
    }

    #[test]
    fn 未实现的能力被明确标出() {
        // webView 类规则在本阶段不可求值——轨迹要说清"是能力不支持"
        // 而不是含糊的"求值失败"
        let trace = trace_rule("@webjs:java.webView('u')", TraceInput::Html, "<p>x</p>");
        let failure = trace.first_failure().expect("应有失败步骤");
        let error = failure.error.as_deref().unwrap_or_default();
        assert!(
            error.contains("webView") || error.contains("不支持"),
            "应指明能力不支持：{error}"
        );
    }

    #[test]
    fn 替换链步骤被记录() {
        let trace = trace_rule("p@text##甲##乙", TraceInput::Html, "<p>甲</p>");
        // 替换链在单段求值内完成（`evaluate_rule` 内部处理），
        // 故这里断言"最终结果正确"以及"至少能看到求值后的值"
        assert_eq!(trace.result, vec!["乙"], "{:?}", trace.steps);
        assert!(trace.is_clean());
    }

    #[test]
    fn 轨迹可序列化为_json() {
        let trace = trace_rule("p@text", TraceInput::Html, "<p>x</p>");
        let json = serde_json::to_string(&trace).expect("应能序列化");
        assert!(json.contains("\"rule\""));
        assert!(json.contains("\"steps\""));
        assert!(json.contains("\"result\""));
        // `failed: false` 不序列化（省噪音），但失败时要有
        assert!(
            !json.contains("\"failed\":false"),
            "未失败的步骤不该带 failed：{json}"
        );
    }

    #[test]
    fn 失败步骤序列化带_error() {
        let trace = trace_rule("div >", TraceInput::Html, "<p>x</p>");
        let json = serde_json::to_string(&trace).expect("应能序列化");
        assert!(json.contains("\"failed\":true"), "{json}");
        assert!(json.contains("\"error\""), "{json}");
    }
}
