//! 规则分派器与组合符切分（tasks.md T023 / FR-003）
//!
//! # 依据来源
//!
//! 语法取自**书源规则的公开语法约定**（社区流通的《书源规则说明》文档）。
//! 原文要点：
//!
//! ## 规则类型前缀
//!
//! > JSOUP之CSS —— 必须以 `@css:` 开头
//! > JSONPath —— 最好以 `@json:` 或 `$.` 开头，其他形式不可靠
//! > XPath —— 必须以 `@XPath:` 或 `//` 开头
//! > JavaScript —— 可以在 `@js:` 中使用
//! > `{{}}` 与 `{}` 规则 —— Default 规则需要以 `@@` 开头
//!
//! ## 组合符
//!
//! > 自定义三种连接符号：`&&`、`||`、`%%`
//! > 只能在同种规则间使用，不包括 js 和正则
//! >
//! > - `&&` 会合并所有取到的值
//! > - `||` 会以第一个取到值的为准
//! > - `%%` 会依次取数，如三个列表，先取列表 1 的第一个，再取列表 2 的第一个，
//! >   再取列表 3 的第一个，再取列表 1 的第二个，再取列表 2 的第二个…
//!
//! # 本任务的边界（tasks.md T023 的验收口径）
//!
//! `@json:` / `@XPath:` / `@js:` 三类**只须被识别并返回结构化「不支持/未就绪」
//! 错误**（FR-006），求值在 S1-2（JS）与 S1-3（JSONPath/XPath）才实现。
//! 故本模块的分派结果里这三类带 `Ready::NotYet`，由调用方转成
//! [`crate::error::RuleError::unsupported`]。
//!
//! # 为什么组合符切分必须引号与括号感知
//!
//! 公开文档明确说组合符"不包括 js 和正则"，但**没说不能出现在 JSONPath 里**——
//! 而 JSONPath 的过滤器语法里 `&&` / `||` 是**内置运算符**：
//!
//! ```text
//! $.data[?(@.type==1 && @.status=="ok")]
//! ```
//!
//! 朴素的 `split("&&")` 会把这条 JSONPath 切成两半，两半都语法非法。故
//! [`split_combinators`] 跟踪字符串字面量、括号嵌套与转义——这是 tasks.md
//! T023 验收项的原文要求（「组合符不被 JSONPath 内的 `&&` 误切」）。

use crate::error::RuleError;

/// 规则类型前缀。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleKind {
    /// 无前缀的 Jsoup/Default 规则
    Default,
    /// `@css:` 选择器
    Css,
    /// `@json:` 或 `$.` 开头的 JSONPath
    JsonPath,
    /// `@XPath:` 或 `//` 开头的 XPath
    XPath,
    /// `@js:` 脚本
    JavaScript,
    /// `@@` 显式标记的 Default 规则（用于 `{{}}` 内的分派）
    ExplicitDefault,
    /// 以 `:` 开头的正则 AllInOne（公开文档：「正则之AllInOne 必须以 : 开头」）
    RegexAllInOne,
}

impl RuleKind {
    /// 类型名（诊断用）。
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Css => "Css",
            Self::JsonPath => "JsonPath",
            Self::XPath => "XPath",
            Self::JavaScript => "JavaScript",
            Self::ExplicitDefault => "ExplicitDefault",
            Self::RegexAllInOne => "RegexAllInOne",
        }
    }
}

/// 某类规则在本阶段是否可求值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ready {
    /// 本阶段已实现
    Yes,
    /// 尚未实现（S1-2 / S1-3）
    NotYet,
}

/// 一个分派出来的规则段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSegment {
    /// 规则类型
    pub kind: RuleKind,
    /// 去掉前缀后的规则正文
    pub body: String,
    /// 本阶段可求值性
    pub ready: Ready,
    /// 该段是否以反序标记 `-` 开头（原文只对目录列表说明，但语法上属段首标记）
    pub reversed: bool,
}

impl RuleSegment {
    /// 若本段尚不可求值，产出结构化「未就绪」错误（FR-006）。
    ///
    /// 能力标识用书源生态通用叫法（`jsonPath` / `xpath` / `js`），
    /// 供书源自身的 `try/catch` 降级逻辑判断——自造命名会让既有书源认不出来。
    pub fn not_ready_error(&self, action: &str) -> Option<RuleError> {
        match self.ready {
            Ready::Yes => None,
            Ready::NotYet => Some(RuleError::unsupported(self.capability_id(), action)),
        }
    }

    /// 本段对应的能力标识。
    pub fn capability_id(&self) -> &'static str {
        match self.kind {
            RuleKind::JsonPath => "jsonPath",
            RuleKind::XPath => "xpath",
            RuleKind::JavaScript => "js",
            // 已实现的三类不会走到这里，给一个明确的兜底标识
            RuleKind::Default | RuleKind::Css | RuleKind::ExplicitDefault => "css",
            RuleKind::RegexAllInOne => "regexAllInOne",
        }
    }
}

/// 组合符（公开文档：「自定义三种连接符号」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// `&&` —— 合并所有取到的值
    Merge,
    /// `||` —— 以第一个取到值的为准
    FirstNonEmpty,
    /// `%%` —— 依次轮回取数（列表 1 第 1 个 → 列表 2 第 1 个 → …）
    RoundRobin,
}

impl Combinator {
    /// 符号字面量。
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Merge => "&&",
            Self::FirstNonEmpty => "||",
            Self::RoundRobin => "%%",
        }
    }
}

/// 组合规则：一串段 + 段之间的组合符。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombinedRule {
    /// 各段（至少一段）
    pub segments: Vec<RuleSegment>,
    /// 段之间的组合符，长度为 `segments.len() - 1`
    pub combinators: Vec<Combinator>,
}

impl CombinedRule {
    /// 是否存在尚未实现的段。
    pub fn first_not_ready(&self) -> Option<&RuleSegment> {
        self.segments
            .iter()
            .find(|segment| segment.ready == Ready::NotYet)
    }
}

// ---------------------------------------------------------------------------
// 前缀分派
// ---------------------------------------------------------------------------

/// 识别一条规则的前缀类型并剥掉前缀。
///
/// 识别顺序（不能换）：
///
/// 1. `@@` —— 显式 Default。必须在 `@css:` 之类之前判，因为 `@@` 是
///    `@` 后跟 `@`，而其余前缀都是 `@` 后跟字母。
/// 2. `@css:` / `@json:` / `@XPath:` / `@js:` —— 显式类型前缀。`@XPath:` 的
///    大小写按公开文档的写法（原文写作 `@XPath:`）。
/// 3. `$.` —— 隐式 JSONPath（原文：「最好以 @json: 或 $. 开头」）。
/// 4. `//` —— 隐式 XPath（原文：「必须以 @XPath: 或 // 开头」）。
/// 5. `:` —— 正则 AllInOne。**放在最后**：其它前缀的正文里也可能出现 `:`，
///    只在该规则**整体以 `:` 开头**时才算。
/// 6. 其余 → Default。
///
/// ## 为什么 `//` 的判定要晚于 `@@`
///
/// 书源里 `//` 也用作 JS 的行注释（公开文档的示例里大量出现），但那是**段内**
/// 内容；段首的 `//` 按原文规定就是 XPath。两者不冲突。
pub fn dispatch_prefix(rule: &str) -> RuleSegment {
    let trimmed = rule.trim();
    let mut reversed = false;
    let mut body = trimmed;

    // 段首反序标记 `-`。排除紧邻数字的情形（那是负索引的一部分，
    // 虽然段首不会出现，但显式排除比默默吞掉安全）。
    if let Some(rest) = body.strip_prefix('-') {
        if !rest.starts_with(|character: char| character.is_ascii_digit()) {
            reversed = true;
            body = rest.trim_start();
        }
    }

    let (kind, payload) = if let Some(rest) = body.strip_prefix("@@") {
        (RuleKind::ExplicitDefault, rest)
    } else if let Some(rest) = strip_prefix_ignore_ascii_case(body, "@css:") {
        (RuleKind::Css, rest)
    } else if let Some(rest) = strip_prefix_ignore_ascii_case(body, "@json:") {
        (RuleKind::JsonPath, rest)
    } else if let Some(rest) = strip_prefix_ignore_ascii_case(body, "@xpath:") {
        (RuleKind::XPath, rest)
    } else if let Some(rest) = strip_prefix_ignore_ascii_case(body, "@js:") {
        (RuleKind::JavaScript, rest)
    } else if body.starts_with("$.") || body.starts_with("$[") {
        (RuleKind::JsonPath, body)
    } else if body.starts_with("//") {
        (RuleKind::XPath, body)
    } else if let Some(rest) = body.strip_prefix(':') {
        // 前缀是 `:`，正文从 `:` 之后开始（`:` 本身不算正文的一部分）
        (RuleKind::RegexAllInOne, rest)
    } else {
        (RuleKind::Default, body)
    };

    RuleSegment {
        kind,
        body: payload.trim().to_string(),
        ready: readiness(kind),
        reversed,
    }
}

/// 某类规则在本阶段（S1-1）的可求值性。
///
/// [WIP] `JsonPath` 在 S1-2 就绪，`XPath` 在 S1-3 就绪——届时应改这里的返回值，
/// 分派逻辑本身不动。
fn readiness(kind: RuleKind) -> Ready {
    match kind {
        RuleKind::Default | RuleKind::ExplicitDefault => Ready::Yes,
        RuleKind::Css => Ready::Yes,
        RuleKind::RegexAllInOne => Ready::Yes,
        RuleKind::JsonPath | RuleKind::XPath | RuleKind::JavaScript => Ready::NotYet,
    }
}

/// 大小写不敏感的前缀剥离（`@XPath:` 与 `@xpath:` 都接受）。
fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if text.len() >= prefix.len() && text[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return Some(&text[prefix.len()..]);
    }
    None
}

// ---------------------------------------------------------------------------
// 组合符切分
// ---------------------------------------------------------------------------

/// 引号/括号感知的组合符切分（FR-003，tasks.md T023 的验收项）。
///
/// 返回 `(段文本, 组合符)` 序列，`combinators` 的长度是段数减一。
///
/// ## 跳过规则（逐条对应一个具体误切场景）
///
/// | 状态 | 进入条件 | 为什么必须跳过 |
/// |---|---|---|
/// | 单/双引号字符串 | `'` 或 `"` | JSONPath 的过滤器值里可以含 `&&` |
/// | 反斜杠转义 | `\` | `\&\&` 是被转义的组合符，不构成运算符 |
/// | 圆括号 | `(` / `)` | 函数调用实参里可以含组合符 |
/// | 方括号 | `[` / `]` | JSONPath 的过滤器 `[?(...)]` 与 CSS 属性选择器 |
/// | 花括号 | `{` / `}` | 正则量词 `{2,3}` 里的逗号不是切分点 |
pub fn split_combinators(rule: &str) -> (Vec<String>, Vec<Combinator>) {
    let chars: Vec<char> = rule.chars().collect();
    let mut segments = Vec::new();
    let mut combinators = Vec::new();
    let mut current = String::new();
    let mut index = 0usize;

    let mut quote: Option<char> = None;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut brace_depth = 0i32;

    while index < chars.len() {
        let character = chars[index];

        // 转义序列整体搬运（只在引号内生效；引号外 JS 的 `\` 也是转义）
        if character == '\\' {
            current.push(character);
            if let Some(next) = chars.get(index + 1) {
                current.push(*next);
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if let Some(active_quote) = quote {
            current.push(character);
            if character == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }

        match character {
            '\'' | '"' => {
                quote = Some(character);
                current.push(character);
                index += 1;
                continue;
            }
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            '[' => bracket_depth += 1,
            ']' => bracket_depth -= 1,
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            _ => {}
        }

        // 只有全部嵌套深度为 0 时，双字符序列才可能是组合符
        let at_top_level = paren_depth <= 0 && bracket_depth <= 0 && brace_depth <= 0;
        if at_top_level && index + 1 < chars.len() {
            let pair = (character, chars[index + 1]);
            let combinator = match pair {
                ('&', '&') => Some(Combinator::Merge),
                ('|', '|') => Some(Combinator::FirstNonEmpty),
                ('%', '%') => Some(Combinator::RoundRobin),
                _ => None,
            };
            if let Some(combinator) = combinator {
                segments.push(current.clone());
                current.clear();
                combinators.push(combinator);
                index += 2;
                continue;
            }
        }

        current.push(character);
        index += 1;
    }

    segments.push(current);

    // 段数与组合符数必须满足 `segments.len() == combinators.len() + 1`。
    // 上面的循环保证了这一点，此处只做不变量的显式记录。
    debug_assert_eq!(segments.len(), combinators.len() + 1);

    (segments, combinators)
}

/// 解析一条完整规则：先按组合符切分，再对每段做前缀分派。
///
/// [DONE] 规则分派的唯一入口。
pub fn parse_rule(rule: &str) -> CombinedRule {
    let (segments, combinators) = split_combinators(rule);
    CombinedRule {
        segments: segments
            .iter()
            .map(|segment| dispatch_prefix(segment))
            .collect(),
        combinators,
    }
}

/// 把一组求值结果按组合符合并（公开文档定义的三种语义）。
///
/// ## 三种语义的实现依据（原文引文见模块文档）
///
/// - `&&`「会合并所有取到的值的」→ 全部拼接（**保持段的顺序**，而不是交错）
/// - `||`「会以第一个取到值的为准」→ 取第一个非空结果，其余丢弃
/// - `%%`「会依次取数…先取列表 1 的第一个，再取列表 2 的第一个…再取列表 1 的
///   第二个」→ **按位置轮转**，而不是按段拼接
///
/// ## 为什么 `||` 判的是"空"而不是"有值"
///
/// 原文说"第一个取到值的为准"，即空结果不算"取到值"。若把"返回空列表"当作
/// 有效结果，`||` 就退化成了"永远取第一段"，与它作为**降级链**的用途不符
/// （书源里 `||` 的典型用法是"主选择器没匹配到就试备选选择器"）。
pub fn combine_results(results: Vec<Vec<String>>, combinators: &[Combinator]) -> Vec<String> {
    if results.is_empty() {
        return Vec::new();
    }
    if results.len() == 1 {
        return results.into_iter().next().unwrap_or_default();
    }

    // ## `%%` 的 N 元语义迫使我们先分组再折叠
    //
    // 公开文档对 `%%` 的定义跨**全部**参与列表的逐轮取数：
    //
    // > 先取列表1的第一个，再取列表2的第一个，再取列表3的第一个，
    // > 再取列表1的第二个，再取列表2的第二个…
    //
    // 若把它实现成左折叠 `round_robin(round_robin(a,b), c)`，第二轮会得到
    // a2,c2,b2 这样的**错序**（中间结果已经把 a、b 交错了）。故先把连续的
    // `%%` 归成一组做 N 元轮转，组之间再按 `&&`/`||` 折叠。
    //
    // 分组做法：遍历组合符，遇到 `%%` 就把下一段并入当前组；遇到 `&&`/`||`
    // 就**封口**当前组（此时该组合符成为组间连接符），并以该段开一个新组。
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut joiners: Vec<Combinator> = Vec::new();
    let mut group: Vec<Vec<String>> = vec![results[0].clone()];

    for (position, combinator) in combinators.iter().enumerate() {
        let next = match results.get(position + 1) {
            Some(next) => next.clone(),
            None => break,
        };
        if *combinator == Combinator::RoundRobin {
            group.push(next);
            continue;
        }
        groups.push(round_robin_n(std::mem::take(&mut group)));
        joiners.push(*combinator);
        group.push(next);
    }
    groups.push(round_robin_n(group));

    // 组间折叠（`&&` 与 `||` 都满足结合律，左折叠即可）
    let mut accumulator = groups.remove(0);
    for (position, joiner) in joiners.iter().enumerate() {
        let next = match groups.get(position) {
            Some(next) => next.clone(),
            None => break,
        };
        accumulator = match joiner {
            Combinator::Merge => merge_segments(accumulator, next),
            Combinator::FirstNonEmpty => {
                if accumulator.is_empty() {
                    next
                } else {
                    accumulator
                }
            }
            // 组内已处理完所有 `%%`，组间不会再有
            Combinator::RoundRobin => merge_segments(accumulator, next),
        };
    }
    accumulator
}

/// `&&` 的合并：按段顺序全部拼接。
fn merge_segments(left: Vec<String>, right: Vec<String>) -> Vec<String> {
    let mut merged = left;
    merged.extend(right);
    merged
}

/// `%%` 的 N 元轮转：第 k 轮依次取每个列表的第 k 个元素，任一列表耗尽即停。
fn round_robin_n(lists: Vec<Vec<String>>) -> Vec<String> {
    let total: usize = lists.iter().map(|list| list.len()).sum();
    let mut combined = Vec::with_capacity(total);
    let mut buffers: Vec<std::vec::IntoIter<String>> =
        lists.into_iter().map(|values| values.into_iter()).collect();

    loop {
        let mut produced_any = false;
        for buffer in &mut buffers {
            if let Some(value) = buffer.next() {
                combined.push(value);
                produced_any = true;
            }
        }
        if !produced_any {
            break;
        }
    }
    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 前缀分派 ----------

    #[test]
    fn css前缀被识别并剥离() {
        let segment = dispatch_prefix("@css:.name>a@text");
        assert_eq!(segment.kind, RuleKind::Css);
        assert_eq!(segment.body, ".name>a@text");
        assert_eq!(segment.ready, Ready::Yes);
    }

    #[test]
    fn 大小写不敏感的类型前缀() {
        // 公开文档写作 `@XPath:`，但书源里也见 `@xpath:` 与 `@js:` 的变体大小写
        for (input, expected) in [
            ("@XPath://div/a", RuleKind::XPath),
            ("@xpath://div/a", RuleKind::XPath),
            ("@XPATH://div/a", RuleKind::XPath),
            ("@JSON:$.a", RuleKind::JsonPath),
            ("@Js:1+1", RuleKind::JavaScript),
            ("@CSS:.a", RuleKind::Css),
        ] {
            assert_eq!(dispatch_prefix(input).kind, expected, "输入 {input}");
        }
    }

    #[test]
    fn 隐式jsonpath与xpath前缀() {
        // 公开文档：「必须以 @XPath: 或 // 开头」「最好以 @json: 或 $. 开头」
        let segment = dispatch_prefix("$.data.list");
        assert_eq!(segment.kind, RuleKind::JsonPath);
        assert_eq!(segment.body, "$.data.list");

        let segment = dispatch_prefix("//div[@class='x']/a");
        assert_eq!(segment.kind, RuleKind::XPath);
        assert_eq!(segment.body, "//div[@class='x']/a");

        // `$[` 形态（数组索引起头）
        assert_eq!(dispatch_prefix("$[0].name").kind, RuleKind::JsonPath);
    }

    #[test]
    fn 显式default前缀() {
        // 公开文档：「Default规则需要以 @@ 开头」
        let segment = dispatch_prefix("@@div.a@text");
        assert_eq!(segment.kind, RuleKind::ExplicitDefault);
        assert_eq!(segment.body, "div.a@text");
        assert_eq!(segment.ready, Ready::Yes);
    }

    #[test]
    fn 正则allinone前缀() {
        // 公开文档：「正则之AllInOne 必须以 : 开头」
        let segment = dispatch_prefix(":href=\"(/read[^\"]*html)\">([^<]*)");
        assert_eq!(segment.kind, RuleKind::RegexAllInOne);
        assert!(segment.body.starts_with("href="));
        assert_eq!(segment.ready, Ready::Yes);
    }

    #[test]
    fn 无前缀落到default() {
        let segment = dispatch_prefix("div.a@text");
        assert_eq!(segment.kind, RuleKind::Default);
        assert_eq!(segment.body, "div.a@text");
        assert_eq!(segment.ready, Ready::Yes);
    }

    #[test]
    fn 未实现的类型标为notyet() {
        for (input, expected_capability) in [
            ("@json:$.a", "jsonPath"),
            ("$.a", "jsonPath"),
            ("@XPath://a", "xpath"),
            ("//a", "xpath"),
            ("@js:1+1", "js"),
        ] {
            let segment = dispatch_prefix(input);
            assert_eq!(segment.ready, Ready::NotYet, "输入 {input}");
            assert_eq!(segment.capability_id(), expected_capability, "输入 {input}");

            // FR-006：产出结构化「不支持能力」错误
            let error = segment
                .not_ready_error("rule")
                .expect("未实现的类型应产出错误");
            let kind = error.to_error_kind();
            match kind {
                tome_core::ErrorKind::UnsupportedCapability { capability } => {
                    assert_eq!(
                        capability.capability_id(),
                        expected_capability,
                        "输入 {input}"
                    );
                }
                other => panic!("应产出 UnsupportedCapability，实际 {other:?}（输入 {input}）"),
            }
        }
    }

    #[test]
    fn 已实现的类型不产出未就绪错误() {
        for input in ["@css:.a", "div.a@text", "@@div", ":regex"] {
            assert!(
                dispatch_prefix(input).not_ready_error("rule").is_none(),
                "输入 {input} 本阶段应可求值"
            );
        }
    }

    #[test]
    fn 段首反序标记被摘出() {
        let segment = dispatch_prefix("-.list@text");
        assert!(segment.reversed);
        assert_eq!(segment.body, ".list@text");

        let segment = dispatch_prefix("-@css:.list");
        assert!(segment.reversed);
        assert_eq!(segment.kind, RuleKind::Css);
        assert_eq!(segment.body, ".list");

        // 紧邻数字的 `-` 不算反序标记
        let segment = dispatch_prefix("-1");
        assert!(!segment.reversed);
    }

    // ---------- 组合符切分 ----------

    #[test]
    fn 三种组合符被切出() {
        let (segments, combinators) = split_combinators("a@text&&b@text");
        assert_eq!(segments, vec!["a@text", "b@text"]);
        assert_eq!(combinators, vec![Combinator::Merge]);

        let (segments, combinators) = split_combinators("a@text||b@text");
        assert_eq!(segments, vec!["a@text", "b@text"]);
        assert_eq!(combinators, vec![Combinator::FirstNonEmpty]);

        let (segments, combinators) = split_combinators("a@text%%b@text");
        assert_eq!(segments, vec!["a@text", "b@text"]);
        assert_eq!(combinators, vec![Combinator::RoundRobin]);
    }

    #[test]
    fn 多段与混合组合符() {
        let (segments, combinators) = split_combinators("a&&b||c%%d");
        assert_eq!(segments, vec!["a", "b", "c", "d"]);
        assert_eq!(
            combinators,
            vec![
                Combinator::Merge,
                Combinator::FirstNonEmpty,
                Combinator::RoundRobin
            ]
        );
        assert_eq!(segments.len(), combinators.len() + 1);
    }

    #[test]
    fn 单段无组合符() {
        let (segments, combinators) = split_combinators("a@text");
        assert_eq!(segments, vec!["a@text"]);
        assert!(combinators.is_empty());
    }

    #[test]
    fn jsonpath过滤器里的组合符不被误切() {
        // T023 验收项的原文要求：「组合符不被 JSONPath 内的 && 误切」。
        // JSONPath 的过滤器语法里 `&&`/`||` 是内置运算符。
        let rule = r#"$.data[?(@.type==1 && @.status=="ok")]"#;
        let (segments, combinators) = split_combinators(rule);
        assert_eq!(segments, vec![rule], "整条 JSONPath 必须保持完整");
        assert!(combinators.is_empty(), "不该切出任何组合符");

        let rule = r#"$.data[?(@.type==1 || @.type==2)]"#;
        let (segments, combinators) = split_combinators(rule);
        assert_eq!(segments, vec![rule]);
        assert!(combinators.is_empty());

        // 混合：过滤器外真正需要切分
        let rule = r#"$.data[?(@.a==1 && @.b==2)]&&$.other"#;
        let (segments, combinators) = split_combinators(rule);
        assert_eq!(segments, vec![r#"$.data[?(@.a==1 && @.b==2)]"#, "$.other"]);
        assert_eq!(combinators, vec![Combinator::Merge]);
    }

    #[test]
    fn 引号内的组合符不被误切() {
        let (segments, combinators) = split_combinators(r#"a@attr(x)&&b[title="p&&q"]"#);
        assert_eq!(segments, vec!["a@attr(x)", r#"b[title="p&&q"]"#]);
        assert_eq!(combinators, vec![Combinator::Merge]);

        // 单引号同理（XPath 里常见）
        let (segments, combinators) = split_combinators(r#"//a[@t='x&&y']||//b"#);
        assert_eq!(segments, vec![r#"//a[@t='x&&y']"#, "//b"]);
        assert_eq!(combinators, vec![Combinator::FirstNonEmpty]);
    }

    #[test]
    fn 转义的组合符不被误切() {
        let (segments, combinators) = split_combinators(r"a\&\&b");
        assert_eq!(segments, vec![r"a\&\&b"]);
        assert!(combinators.is_empty());
    }

    #[test]
    fn 括号内的组合符不被误切() {
        // JS 里的函数实参
        let rule = r#"@js:if (a && b) { return "x"; }"#;
        let (segments, combinators) = split_combinators(rule);
        assert_eq!(segments, vec![rule]);
        assert!(combinators.is_empty());

        // 括号内 + 括号外
        let rule = r#"f(a && b)&&g(c)"#;
        let (segments, combinators) = split_combinators(rule);
        assert_eq!(segments, vec!["f(a && b)", "g(c)"]);
        assert_eq!(combinators, vec![Combinator::Merge]);
    }

    #[test]
    fn 正则量词里的花括号不影响切分() {
        // `{2,3}` 里的花括号会进 brace_depth，但它内部的字符本就不含组合符；
        // 关键是不能因为花括号未闭合而把后续的真实组合符吞掉。
        let (segments, combinators) = split_combinators(r"a{2,3}@text&&b");
        assert_eq!(segments, vec!["a{2,3}@text", "b"]);
        assert_eq!(combinators, vec![Combinator::Merge]);
    }

    #[test]
    fn css属性选择器里的方括号不影响切分() {
        let (segments, combinators) = split_combinators(r#"a[href^=http]@text&&b[src$=.png]@src"#);
        assert_eq!(segments, vec!["a[href^=http]@text", "b[src$=.png]@src"]);
        assert_eq!(combinators, vec![Combinator::Merge]);
    }

    #[test]
    fn 未闭合引号不崩溃且不误切() {
        // 畸形输入是书源常态。未闭合引号下的取向：**引号内的内容照常跳过**，
        // 故引号之后若真有组合符，它仍会被认出来——这比"整段吞掉"更接近
        // "作者手滑少写一个引号但组合符确实存在"的真实意图。
        //
        // 本测试把这个边界行为钉住：它变了就说明引号跟踪的逻辑被改动了。
        let (segments, combinators) = split_combinators(r#"a@text&&b[title="未闭合"#);
        assert_eq!(segments, vec!["a@text", r#"b[title="未闭合"#]);
        assert_eq!(combinators, vec![Combinator::Merge]);

        // 引号闭合时同样切分——两种输入产出同一种结构（这正是"不因未闭合而变行为"）
        let (segments, combinators) = split_combinators(r#"a@text&&b[title="闭合"]"#);
        assert_eq!(segments, vec!["a@text", r#"b[title="闭合"]"#]);
        assert_eq!(combinators, vec![Combinator::Merge]);
    }

    // ---------- 完整解析 ----------

    #[test]
    fn 完整解析产出段落与组合符() {
        let combined = parse_rule("@css:.name@text&&@css:.author@text");
        assert_eq!(combined.segments.len(), 2);
        assert_eq!(combined.combinators, vec![Combinator::Merge]);
        assert_eq!(combined.segments[0].kind, RuleKind::Css);
        assert_eq!(combined.segments[0].body, ".name@text");
        assert_eq!(combined.segments[1].body, ".author@text");
        assert!(combined.first_not_ready().is_none());
    }

    #[test]
    fn 含未实现类型的组合规则能被定位() {
        // 公开文档说组合符"只能在同种规则间使用"，但书源实际会混用。
        // 无论是否合规，分派器都要能指出**是哪一段**未就绪（FR-006 的可观测性）。
        let combined = parse_rule("@css:.a@text||@js:java.ajax('x')");
        assert_eq!(combined.segments.len(), 2);
        let not_ready = combined.first_not_ready().expect("应有未就绪的段");
        assert_eq!(not_ready.kind, RuleKind::JavaScript);
        assert_eq!(not_ready.capability_id(), "js");
    }

    // ---------- 结果合并 ----------

    #[test]
    fn merge合并全部值() {
        let merged = combine_results(
            vec![vec!["a".into(), "b".into()], vec!["c".into()]],
            &[Combinator::Merge],
        );
        assert_eq!(merged, vec!["a", "b", "c"]);
    }

    #[test]
    fn firstnonempty取第一个非空() {
        let picked = combine_results(
            vec![Vec::new(), vec!["b".into()], vec!["c".into()]],
            &[Combinator::FirstNonEmpty, Combinator::FirstNonEmpty],
        );
        assert_eq!(picked, vec!["b"], "空结果不算取到值，应继续找下一个");

        let picked = combine_results(
            vec![vec!["a".into()], vec!["b".into()]],
            &[Combinator::FirstNonEmpty],
        );
        assert_eq!(picked, vec!["a"]);
    }

    #[test]
    fn roundrobin按位置轮转而非拼接() {
        // 公开文档：「先取列表1的第一个，再取列表2的第一个，再取列表3的第一个，
        // 再取列表1的第二个…」
        let merged = combine_results(
            vec![
                vec!["a1".into(), "a2".into()],
                vec!["b1".into(), "b2".into()],
            ],
            &[Combinator::RoundRobin],
        );
        assert_eq!(merged, vec!["a1", "b1", "a2", "b2"]);

        // 三个列表：严格按文档的"逐轮取"——第一轮取三者的第 1 个，
        // 第二轮取三者的第 2 个
        let merged = combine_results(
            vec![
                vec!["a1".into(), "a2".into()],
                vec!["b1".into(), "b2".into()],
                vec!["c1".into(), "c2".into()],
            ],
            &[Combinator::RoundRobin, Combinator::RoundRobin],
        );
        assert_eq!(merged, vec!["a1", "b1", "c1", "a2", "b2", "c2"]);
    }

    #[test]
    fn roundrobin在长度不齐时不丢元素() {
        let merged = combine_results(
            vec![
                vec!["a1".into(), "a2".into(), "a3".into()],
                vec!["b1".into()],
            ],
            &[Combinator::RoundRobin],
        );
        assert_eq!(merged, vec!["a1", "b1", "a2", "a3"]);
    }

    #[test]
    fn 空输入产出空() {
        assert_eq!(combine_results(Vec::new(), &[]), Vec::<String>::new());
        assert_eq!(combine_results(vec![Vec::new()], &[]), Vec::<String>::new());
    }
}
