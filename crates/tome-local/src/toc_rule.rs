//! \[DONE] TXT 目录规则求值与回退（tasks.md T076 / FR-020）
//!
//! # 本模块的职责边界
//!
//! 只做**一个**判断：给定一段文本与一组规则，**哪一条规则产出了可用的
//! 章节切点**。它不负责切分兜底（那是 [`crate::split`]），也不负责
//! 编码（那是 [`crate::encoding`]）。
//!
//! # 规则的形态
//!
//! `TxtTocRule.rule` 在数据模型里写作「正则或脚本，由内容形态判定」。
//! 本阶段实现**正则**形态（覆盖社区规则里的绝大多数），脚本形态需要 JS
//! 沙箱（`tome-js`），而 `tome-local` 按架构铁律不依赖它——故脚本形态的
//! 规则在此被**识别并跳过**，跳过原因进入 [`TocDetection::attempts`]，
//! 让 `tome local toc` 能如实回答"为什么这条规则没生效"。
//!
//! > \[WIP] 脚本形态（rule 以 `<js>` 包裹或经 `@js:` 前缀标记）目前只跳过并
//! > 记录。接线到 `tome-js` 需要组合根注入 JS 运行时，属后续切片。
//!
//! # 为什么"命中即停"而不是"取产出最多者"
//!
//! FR-020 的"多条规则回退"是**回退**语义：用户按顺序配置规则，
//! 前面的是更精确的尝试。若改成"取产出最多切点的规则胜出"，一条贪婪的
//! 宽规则（如 `^.{1,20}$`）永远会赢，用户精心写的第一条规则变成摆设。
//! 故语义定死为：**按 `sort_order` 升序，第一条产出足够切点的规则胜出**。
//!
//! ## 什么算"足够切点"
//!
//! 至少 [`MIN_TOC_POINTS`] 个。取 2 而不是 1：只切出一个"章节"等于没切
//! （书名本身就会命中一次），用 1 作为门槛会让绝大多数误匹配通过。
//! 而 2 是"至少区分了前后两段"的最小有意义结果。

use regex::Regex;
use tome_core::TxtTocRule;

use crate::txt::TocPoint;

/// 一条规则被采纳所需的最少切点数。
pub const MIN_TOC_POINTS: usize = 2;

/// 目录识别规则的形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleForm {
    /// 正则表达式
    Regex,
    /// 脚本片段（需要 JS 沙箱，本 crate 不执行）
    Script,
}

/// 单条规则的尝试结果（回退过程的可观测记录）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleAttempt {
    /// 规则名（用户配置的名字，便于对照）
    pub rule_name: String,
    /// 规则的 `sort_order`
    pub sort_order: i32,
    /// 判定出的规则形式
    pub form: RuleForm,
    /// 产出的切点数（脚本形式恒为 0）
    pub points_found: usize,
    /// 未采纳的原因；`None` 表示这条被采纳了
    pub rejected_because: Option<String>,
}

/// 目录识别的完整结论。
///
/// # 为什么记录"采用的那一条"而不是全部结果
///
/// `contracts/cli.md` 要求 `tome local toc` 输出「**含用到的目录规则**」。
/// 一个字符串字段即可满足，而 attempts 列表回答的是另一个问题——
/// "为什么别的规则没被用上"，那是用户调试自己的规则时最需要的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocDetection {
    /// 被采纳的规则名；`None` 表示没有规则产出足够切点
    pub rule_name: Option<String>,
    /// 是否走了"按大小切分"的兜底
    pub split_by_size: bool,
    /// 全部尝试记录（按 `sort_order` 升序）
    pub attempts: Vec<RuleAttempt>,
    /// 采纳规则产出的切点（未被采纳时为空）
    pub points: Vec<TocPoint>,
}

impl TocDetection {
    /// 没有任何可用结果（供上层判断是否需要兜底）。
    fn empty() -> Self {
        Self {
            rule_name: None,
            split_by_size: false,
            attempts: Vec::new(),
            points: Vec::new(),
        }
    }
}

/// \[DONE] FR-020 的规则回退求值：按 `sort_order` 依次尝试，第一条足够的胜出。
pub fn detect(text: &str, rules: &[TxtTocRule]) -> TocDetection {
    let mut ordered: Vec<&TxtTocRule> = rules.iter().filter(|rule| rule.enabled).collect();
    // 主序 `sort_order`、次序 `name`（稳定）：同一份配置的多次运行必须
    // 得出同一结论，否则差分测试会随机失败。
    ordered.sort_by(|left, right| left.name.cmp(&right.name));
    ordered.sort_by_key(|rule| rule.sort_order);

    let mut detection = TocDetection::empty();
    if ordered.is_empty() {
        return detection;
    }

    for rule in ordered {
        let (form, points_result) = evaluate_rule(text, &rule.rule);
        let mut attempt = RuleAttempt {
            rule_name: rule.name.clone(),
            sort_order: rule.sort_order,
            form,
            points_found: 0,
            rejected_because: None,
        };

        match points_result {
            Ok(points) => {
                attempt.points_found = points.len();
                if points.len() < MIN_TOC_POINTS {
                    attempt.rejected_because =
                        Some(format!("切点数不足（{} < {MIN_TOC_POINTS}）", points.len()));
                    detection.attempts.push(attempt);
                    continue;
                }
                detection.rule_name = Some(rule.name.clone());
                detection.points = points;
                detection.attempts.push(attempt);
                return detection;
            }
            Err(reason) => {
                attempt.rejected_because = Some(reason);
                detection.attempts.push(attempt);
            }
        }
    }

    detection
}

/// 求值一条规则，产出切点（按位置升序、已去重）。
fn evaluate_rule(text: &str, rule: &str) -> (RuleForm, Result<Vec<TocPoint>, String>) {
    if let Some(form) = classify(rule) {
        if form == RuleForm::Script {
            return (
                RuleForm::Script,
                Err("脚本规则需要 JS 沙箱，本 crate 不执行（见模块文档）".to_string()),
            );
        }
    }

    let Some(pattern) = strip_regex_markers(rule) else {
        return (
            RuleForm::Script,
            Err("规则为空或不含可求值的正则".to_string()),
        );
    };

    // 多行锚点：目录规则里的 `^`/`$` 指的是**行**首尾（用户的直观预期，
    // 也是社区规则的通行写法）。若不启用多行模式，`^第.章` 只会在整个
    // 文本的开头匹配一次——那会让所有规则都"切点不足"。
    let anchored = format!("(?m){pattern}");
    let compiled = match Regex::new(&anchored) {
        Ok(compiled) => compiled,
        Err(error) => return (RuleForm::Regex, Err(format!("正则编译失败：{error}"))),
    };

    // 每次匹配都记录它在文本上的字节位置；切点是那一行的起点。
    //
    // ## 为什么先收集字节偏移、再统一换算字符偏移
    //
    // 朴素的写法是在循环里对每个匹配做 `text[..start].chars().count()`。
    // 那是**O(n) 每次**——4.8 万个匹配 × 一亿字符的扫描 = O(n²)，
    // 实测让 SC-004 的 100MB 用例跑出 210 秒（预算 8 秒），慢了 26 倍。
    //
    // 正确做法是把两件事分开：
    //
    // 1. 匹配循环只记字节偏移（O(1) 每次）
    // 2. **一次** `char_indices` 扫描把字节偏移映射为字符偏移（O(n) 总计）
    //
    // 第 2 步用双指针推进：两串都单调递增，故整体是线性的。
    let mut byte_offsets: Vec<(usize, String)> = Vec::new();
    for matched in compiled.find_iter(text) {
        let start = matched.start();
        // 标题：取整行（从切点到该行结尾），而不是只取匹配到的片段——
        // 规则常写成 `^第.章`，而用户期望标题是完整的 `第一章 起点`。
        let line_end = text[start..]
            .find('\n')
            .map(|offset| start + offset)
            .unwrap_or(text.len());
        byte_offsets.push((start, text[start..line_end].trim_end().to_string()));
    }

    let points = to_points_with_char_offsets(text, byte_offsets);

    let mut points = points;
    points.sort_by_key(|point| point.byte_offset);
    points.dedup_by_key(|point| point.byte_offset);
    (RuleForm::Regex, Ok(points))
}

/// 把 `(字节偏移, 标题)` 列表补上字符偏移（**单次**线性扫描）。
///
/// 输入必须按字节偏移**升序**（`find_iter` 天然如此），本函数用一个
/// 单调前进的字符计数器把它映射为字符偏移——不做逐次的从头计数。
fn to_points_with_char_offsets(text: &str, entries: Vec<(usize, String)>) -> Vec<TocPoint> {
    let mut points = Vec::with_capacity(entries.len());
    let mut byte_cursor = 0usize;
    let mut char_cursor = 0i64;
    for (byte_offset, title) in entries {
        if byte_offset < byte_cursor {
            // 乱序（调用方破坏前置条件）：退回精确但较慢的算法。
            char_cursor = text[..byte_offset.min(text.len())].chars().count() as i64;
        } else {
            // 从上一个匹配推进到当前位置，累计途中经过的字符数。
            char_cursor += text[byte_cursor..byte_offset.min(text.len())]
                .chars()
                .count() as i64;
        }
        byte_cursor = byte_offset.min(text.len());
        points.push(TocPoint {
            title,
            byte_offset: byte_offset as i64,
            char_offset: char_cursor,
        });
    }
    points
}

/// 判定规则的形式。
///
/// 判定依据是`data-model.md` 的原文「正则或脚本，由内容形态判定」，取两种
/// 可判定的形态特征：
///
/// - 以 `@js:` 或 `@script:` 开头 → 脚本
/// - 整串被 `<js>...</js>` 包裹 → 脚本
/// - 其余 → 正则
pub fn classify(rule: &str) -> Option<RuleForm> {
    let trimmed = rule.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("@js:") || lower.starts_with("@script:") {
        return Some(RuleForm::Script);
    }
    if lower.starts_with("<js>") && lower.ends_with("</js>") {
        return Some(RuleForm::Script);
    }
    Some(RuleForm::Regex)
}

/// 去掉规则里可能的 `##` 替换后缀与首尾斜杠，返回纯正则。
///
/// 社区里 TXT 目录规则有两种常见写法：纯正则，或"正则 + `##` 替换"。
/// 后者是**规则引擎**（`tome-rule` 的替换链）的语法，本模块只做目录识别
/// 的定位，故把 `##` 之后的部分裁掉——标题替换不是切点的关切。
fn strip_regex_markers(rule: &str) -> Option<String> {
    let trimmed = rule.trim();
    let without_chain = trimmed.split("##").next().unwrap_or(trimmed).trim();
    let body = without_chain
        .strip_prefix('/')
        .and_then(|rest| rest.rsplit_once('/').map(|(pattern, _flags)| pattern))
        .unwrap_or(without_chain);
    if body.is_empty() {
        None
    } else {
        Some(body.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(name: &str, pattern: &str, order: i32) -> TxtTocRule {
        TxtTocRule {
            id: 0,
            name: name.to_string(),
            rule: pattern.to_string(),
            example: None,
            enabled: true,
            sort_order: order,
        }
    }

    #[test]
    fn regex_rule_finds_line_starts() {
        let text = "第一章 甲\n内容\n第二章 乙\n内容\n";
        let detection = detect(text, &[rule("r", "^第.章.*", 0)]);
        assert_eq!(detection.points.len(), 2);
        assert_eq!(detection.points[0].title, "第一章 甲");
    }

    #[test]
    fn first_matching_rule_wins_and_later_ones_are_not_tried() {
        let text = "【壹】甲\n内容\n【贰】乙\n内容\n";
        let detection = detect(
            text,
            &[
                rule("宽规则", "^.{1,10}$", 0),
                rule("精确规则", "^【.】.*", 1),
            ],
        );
        assert_eq!(detection.rule_name.as_deref(), Some("宽规则"));
        assert_eq!(
            detection.attempts.len(),
            1,
            "命中即停——后续规则不应再被求值"
        );
    }

    #[test]
    fn fallback_order_is_by_sort_order_not_by_input_order() {
        // 输入顺序刻意与 sort_order 相反，结论必须按 sort_order 走。
        let text = "第一章 甲\n内容\n第二章 乙\n内容\n";
        let detection = detect(
            text,
            &[rule("后", "^第.章.*", 9), rule("先", "^第一章.*", 1)],
        );
        // `先` 只切出 1 个切点 → 不足 → 回退到 `后`。
        assert_eq!(detection.rule_name.as_deref(), Some("后"));
        assert_eq!(detection.attempts[0].rule_name, "先");
    }

    #[test]
    fn insufficient_points_are_rejected_with_a_reason() {
        let text = "第一章 甲\n内容\n";
        let detection = detect(text, &[rule("r", "^第.章.*", 0)]);
        assert!(detection.rule_name.is_none());
        let attempt = &detection.attempts[0];
        assert_eq!(attempt.points_found, 1);
        assert!(attempt.rejected_because.is_some());
    }

    #[test]
    fn invalid_regex_is_recorded_not_fatal() {
        let text = "第一章\n第二章\n";
        let detection = detect(text, &[rule("坏", "^(第[章", 0), rule("好", "^第.章", 1)]);
        assert_eq!(detection.rule_name.as_deref(), Some("好"));
        assert!(detection.attempts[0].rejected_because.is_some());
        assert_eq!(detection.attempts[0].form, RuleForm::Regex);
    }

    #[test]
    fn script_rules_are_classified_and_skipped_with_a_reason() {
        let detection = detect("第一章\n第二章\n", &[rule("脚本", "@js: chapters = []", 0)]);
        assert!(detection.rule_name.is_none());
        assert_eq!(detection.attempts[0].form, RuleForm::Script);
        assert!(detection.attempts[0].rejected_because.is_some());
    }

    #[test]
    fn js_tag_wrapped_rule_is_classified_as_script() {
        assert_eq!(classify("<js>return 1</js>"), Some(RuleForm::Script));
        assert_eq!(classify("@script:x"), Some(RuleForm::Script));
        assert_eq!(classify("^第.章.*"), Some(RuleForm::Regex));
        assert_eq!(classify("   "), None);
    }

    #[test]
    fn anchors_are_per_line() {
        // 没有多行模式的话 `^第.章` 只会在文本最开头匹配一次。
        let text = "第一章 甲\n内容\n第二章 乙\n内容\n第三章 丙\n";
        let detection = detect(text, &[rule("r", "^第.章", 0)]);
        assert_eq!(detection.points.len(), 3);
    }

    #[test]
    fn replace_suffix_is_stripped_from_the_pattern() {
        let text = "[1] 甲\n内容\n[2] 乙\n内容\n";
        let detection = detect(text, &[rule("r", r"^\[\d+\].*##\s*", 0)]);
        assert_eq!(detection.points.len(), 2);
    }

    #[test]
    fn disabled_rules_are_ignored() {
        let mut disabled = rule("d", "^第.章", 0);
        disabled.enabled = false;
        let detection = detect("第一章\n第二章\n", &[disabled]);
        assert!(detection.attempts.is_empty());
    }

    #[test]
    fn empty_rule_list_yields_no_detection() {
        let detection = detect("第一章\n第二章\n", &[]);
        assert!(detection.rule_name.is_none());
        assert!(detection.points.is_empty());
    }

    #[test]
    fn titles_are_whole_lines_not_just_the_match() {
        // 规则常只写 `^第.章`，而标题应是完整的一行。
        let text = "第一章 漫长的起点\n内容\n第二章 同样漫长的承接\n内容\n";
        let detection = detect(text, &[rule("r", "^第.章", 0)]);
        assert_eq!(detection.points[0].title, "第一章 漫长的起点");
    }

    #[test]
    fn char_offsets_track_byte_offsets_through_multibyte_text() {
        let text = "前言：中文\n第一章 甲\n内容\n第二章 乙\n";
        let detection = detect(text, &[rule("r", "^第.章", 0)]);
        let point = &detection.points[0];
        assert!(point.byte_offset > point.char_offset);
        assert_eq!(
            &text.as_bytes()[point.byte_offset as usize..][..point.title.len()],
            point.title.as_bytes()
        );
    }
}
