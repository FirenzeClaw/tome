//! 替换链与 `{{}}` 插值占位（tasks.md T025 / FR-004）
//!
//! # 依据来源
//!
//! 语法取自**书源规则的公开语法约定**（社区流通的《书源规则说明》文档）。
//! 原文要点：
//!
//! ## 替换链
//!
//! > 如需要正则替换在最后加上 `##正则表达式##替换内容`，替换内容为空时，
//! > 第二个 `##` 可以省略
//! > 例：`class.odd.0@tag.a.0@text||tag.dd.0@tag.h1@text##全文阅读`
//! >
//! > 正则之净化 —— 形式 `##正则表达式##替换内容`
//! > 只能跟在其他规则后面，独立使用相当于 `all##正则表达式##替换内容`
//! > 注意点：该规则为**循环匹配替换**
//!
//! ## `{{}}` 插值
//!
//! > 在 `{{}}` 里只能使用 js
//! > 在搜索URL与发现URL以外的 `{{}}`：可在 `{{}}` 中使用**任意规则**（正则除外），
//! > 默认为 js，使用其他规则需要有明显的标志头，如：Default规则需要以 `@@` 开头，
//! > XPath 需要以 `@xpath:` 或 `//` 开头，JSONPath 需要以 `@json:` 或 `$.` 开头，
//! > CSS 需要以 `@css:` 开头
//!
//! # 本任务的边界（tasks.md T025 的验收口径）
//!
//! > JS 桥在 S1-2 才存在——`{{}}` **只做到解析出占位表达式并定义桥接 trait**，不求值。
//!
//! 故 [`Interpolation::evaluate`] 对 js 类占位产出结构化「未就绪」错误，
//! 而 CSS/Default 类占位本阶段即可求值（它们在 `{{}}` 里同样合法）。

use crate::dispatch::{dispatch_prefix, RuleKind};
use crate::error::RuleError;
use crate::regex::{self, RegexLimits};

/// 一次替换（`##正则##替换内容`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// 正则原文
    pub pattern: String,
    /// 替换内容（原文，尚未经 `$N` → `${N}` 翻译）
    pub replacement: String,
}

/// 解析出的替换链。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplaceChain {
    /// 替换链之前的**主体规则**（可能为空——公开文档说"独立使用相当于
    /// `all##正则##替换内容`"，即主体缺省时是对整段内容做替换）
    pub subject: String,
    /// 按出现顺序排列的替换（先出现的先应用）
    pub replacements: Vec<Replacement>,
}

/// 解析替换链：`主体##正则##替换##正则2##替换2`。
///
/// # `##` 边界的引号与括号感知
///
/// 原文只给了形式，没说 `##` 不能出现在正则或替换内容里——而**它可以**：
///
/// ```text
/// @css:div@html##<span class="n">(.*?)</span>##$1##
/// ```
///
/// 上面这条里，正则 `(.*?)` 不含 `##`，但替换串里出现 `##` 完全可能（例如把
/// 分隔符本身作为字面量替换）。故切分时跟踪引号、转义与括号深度：只有
/// **顶层**的 `##` 才是分隔符。
///
/// ## 段数规则（公开文档：「替换内容为空时，第二个 ## 可以省略」）
///
/// - `主体##正则##替换` → 主体 + 一条替换
/// - `主体##正则` → 主体 + 一条替换（替换内容为空）
/// - `主体##正则##替换##正则2##替换2` → 主体 + 两条替换
///
/// 即：`##` 之后的内容按"正则, 替换"成对消费，最后一对允许缺替换。
/// 奇数个 `##` 分隔出的段数如果是偶数，最后一段被当作正则、替换为空。
pub fn parse_replace_chain(rule: &str) -> ReplaceChain {
    let parts = split_by_hash_hash(rule);
    if parts.len() < 2 {
        return ReplaceChain {
            subject: rule.trim().to_string(),
            replacements: Vec::new(),
        };
    }

    let subject = parts[0].trim().to_string();
    let mut replacements = Vec::new();
    let mut index = 1usize;
    while index < parts.len() {
        // ## 正则与替换内容都**不 trim**
        //
        // 理由：`##` 已经精确界定了两端的边界，任何空白都是作者有意写的。
        // 最典型的反例是"把连续空白折叠为单个空格"：
        //
        //     ##\s+## ##       ← 替换内容是一个空格，trim 掉就变成了"删除空白"
        //
        // 这是净化规则里极常见的一类，trim 会让它静默产出错误结果。
        // 忠实于原文是兼容性优先（宪法原则 I）在该处的落点。
        let pattern = unquote_pattern(&parts[index]);
        let replacement = parts
            .get(index + 1)
            .map(|text| text.to_string())
            .unwrap_or_default();
        if !pattern.is_empty() {
            replacements.push(Replacement {
                pattern,
                replacement,
            });
        }
        index += 2;
    }

    ReplaceChain {
        subject,
        replacements,
    }
}

/// 按顶层 `##` 切分。返回各段（含 `##` 之间的空段）。
fn split_by_hash_hash(rule: &str) -> Vec<String> {
    let chars: Vec<char> = rule.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut index = 0usize;

    let mut quote: Option<char> = None;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;

    while index < chars.len() {
        let character = chars[index];

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
            _ => {}
        }

        let at_top_level = paren_depth <= 0 && bracket_depth <= 0;
        if at_top_level && character == '#' && chars.get(index + 1).copied() == Some('#') {
            parts.push(current.clone());
            current.clear();
            index += 2;
            continue;
        }

        current.push(character);
        index += 1;
    }

    parts.push(current);
    parts
}

/// 去掉正则两端成对的分隔引号。
///
/// 公开文档给的示例里正则都不加引号（`:href="(/read[^"]*html)">([^<]*)`），
/// 但实践中有人写成 `"正则"` 以避开 `##` 的歧义。剥掉成对引号是宽容处理。
///
/// **不 trim 内部内容**：`" a "` 剥引号后是 ` a `（含空格）而不是 `a`——
/// 引号已经明确了边界，内部的空白是作者写的（理由同 `parse_replace_chain`）。
fn unquote_pattern(text: &str) -> String {
    if text.len() >= 2 {
        let mut chars = text.chars();
        let first = chars.next();
        let last = text.chars().next_back();
        if let (Some(first), Some(last)) = (first, last) {
            if (first == '"' || first == '\'') && first == last {
                return text[first.len_utf8()..text.len() - last.len_utf8()].to_string();
            }
        }
    }
    text.to_string()
}

impl ReplaceChain {
    /// 按序应用全部替换（FR-004 的"可叠加"）。
    ///
    /// \[DONE] 替换链求值的唯一入口。
    ///
    /// ## 三条实现取舍
    ///
    /// 1. **循环匹配替换**（公开文档：「该规则为循环匹配替换」）。等价于
    ///    `replace_all`，故这里用 [`regex::CompiledRegex::replace_all_java`]。
    /// 2. **替换串过翻译层**（T024 的陷阱二）：Java 的 `$1a` 若不翻译会被
    ///    Rust 读成组名 `1a` 而静默丢字。`replace_all_java` 已内建该翻译。
    /// 3. **单条替换失败不中断整条链**：正则编译失败或回溯超限时跳过该条，
    ///    保留前一条的结果。理由：净化规则链是"尽力清理正文"的场合，
    ///    中断会让用户拿到未净化的全文（更糟），跳过只是少清一处。
    ///    `\[UNCERTAIN]` 这个取向未与上游行为对照，S1-6 的差分基线会给出结论。
    pub fn apply(&self, text: &str, limits: RegexLimits) -> String {
        let mut result = text.to_string();
        for replacement in &self.replacements {
            let compiled = match regex::compile(&replacement.pattern, limits) {
                Ok(compiled) => compiled,
                Err(_) => continue,
            };
            match compiled.replace_all_java(&result, &replacement.replacement, limits) {
                Ok(replaced) => result = replaced,
                Err(_) => continue,
            }
        }
        result
    }

    /// 是否存在替换（无替换时调用方可跳过整条链）。
    pub fn is_empty(&self) -> bool {
        self.replacements.is_empty()
    }
}

// ---------------------------------------------------------------------------
// {{}} 插值
// ---------------------------------------------------------------------------

/// `{{}}` 里的一个占位表达式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    /// 表达式原文（已去空白）
    pub expression: String,
    /// 分派出的规则类型（依公开文档：`@@`/`@css:`/`@json:`/`@xpath:`/`//`/`$.`/默认 js）
    pub kind: RuleKind,
}

impl Placeholder {
    /// 该占位在当前阶段是否可求值。
    ///
    /// `{{}}` 里**默认是 js**（公开文档原文），故无前缀的表达式一律按 js
    /// 处理。S1-2 起 JS 已就绪（沙箱 + 桥），故 js 类占位可求值；
    /// `jsonPath` / `xpath` 仍待 S1-3。
    pub fn is_ready(&self) -> bool {
        matches!(
            self.kind,
            RuleKind::Css | RuleKind::Default | RuleKind::ExplicitDefault | RuleKind::JavaScript
        )
    }

    /// 未就绪时产出结构化错误（FR-006）。
    pub fn not_ready_error(&self) -> Option<RuleError> {
        if self.is_ready() {
            return None;
        }
        let capability = match self.kind {
            RuleKind::JsonPath => "jsonPath",
            RuleKind::XPath => "xpath",
            RuleKind::JavaScript => "js",
            RuleKind::RegexAllInOne => "regexAllInOne",
            _ => return None,
        };
        Some(RuleError::unsupported(capability, "interpolation"))
    }
}

/// `{{}}` 里可用规则的求值桥。
///
/// # 为什么现在就要定义这个 trait（tasks.md T025 的验收口径）
///
/// JS 桥在 S1-2 才存在，但**接口形状**是本期就能确定的：占位要么由 JS 求值、
/// 要么由规则引擎求值，两者都产出字符串。先钉住这个形状有两个收益：
///
/// 1. S1-2 接 JS 时只需提供一个实现，不必改动调用方与解析层。
/// 2. 测试可用一个桩实现验证"占位被正确识别并按序求值"，而不依赖 JS 是否就绪。
pub trait PlaceholderBridge {
    /// 求值一个 js 类占位表达式。
    ///
    /// **当前无实现**（S1-2 才提供）。S1-1 阶段的实现应返回
    /// [`RuleError::unsupported`]，能力标识 `js`（FR-006）。
    fn evaluate_js(&self, expression: &str) -> Result<String, RuleError>;

    /// 求值一个规则类占位（CSS / Default）。
    ///
    /// 本阶段即可实现（跑 [`crate::css_post::evaluate_rule`]），故不是"未就绪"。
    fn evaluate_rule(&self, expression: &str) -> Result<String, RuleError>;
}

/// 解析出的插值模板：`{{}}` 之外的原文 + 占位序列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interpolation {
    /// 静态片段（长度 = 占位数 + 1）
    pub literals: Vec<String>,
    /// 占位（按出现顺序）
    pub placeholders: Vec<Placeholder>,
}

impl Interpolation {
    /// 是否存在占位（无占位时是纯静态文本）。
    pub fn has_placeholders(&self) -> bool {
        !self.placeholders.is_empty()
    }

    /// 未就绪的占位（第一个），供调用方产出 FR-006 的结构化错误。
    pub fn first_not_ready(&self) -> Option<&Placeholder> {
        self.placeholders
            .iter()
            .find(|placeholder| !placeholder.is_ready())
    }

    /// 是否含 js 类占位（S1-1 阶段唯一必然未就绪的一类）。
    pub fn has_javascript(&self) -> bool {
        self.placeholders
            .iter()
            .any(|placeholder| placeholder.kind == RuleKind::JavaScript)
    }

    /// 用桥求值并拼接出最终字符串。
    ///
    /// 求值顺序：按占位出现顺序，逐个交给 [`PlaceholderBridge`]。
    ///
    /// ## 路由规则
    ///
    /// - 可求值的规则类（CSS / Default）→ [`PlaceholderBridge::evaluate_rule`]
    /// - **其余一律走 [`PlaceholderBridge::evaluate_js`]**，但先经
    ///   [`Placeholder::not_ready_error`] 判定：JSONPath/XPath 这类占位即便
    ///   交给 js 桥也只会得到 `js` 的能力标识，那会**掩盖真实原因**
    ///   （书源作者看到"js 不可用"而实际是"JSONPath 未就绪"，无从改起）。
    pub fn evaluate(&self, bridge: &dyn PlaceholderBridge) -> Result<String, RuleError> {
        let mut output = String::new();
        for (position, literal) in self.literals.iter().enumerate() {
            output.push_str(literal);
            if let Some(placeholder) = self.placeholders.get(position) {
                let value = match placeholder.kind {
                    RuleKind::Css | RuleKind::Default | RuleKind::ExplicitDefault => {
                        bridge.evaluate_rule(&placeholder.expression)?
                    }
                    // 未就绪的类型直接报出**它自己**的能力标识（FR-006）
                    _ => {
                        if let Some(error) = placeholder.not_ready_error() {
                            return Err(error);
                        }
                        bridge.evaluate_js(&placeholder.expression)?
                    }
                };
                output.push_str(&value);
            }
        }
        Ok(output)
    }
}

/// 解析 `{{}}` 插值。
///
/// # 嵌套与转义（逐条对应一个具体输入）
///
/// ## 定界符归谁
///
/// `{{` 归**前一段**字面，`}}` **不属任何字面段**（它与 `{{` 一起是定界符）。
/// 故 `a{{x}}b` 的 `literals` 是 `["a", "b"]` 而不是 `["a{{", "}}b"]`。
/// 这个约定让"字面拼接 + 占位值拼接"能重建原文（见 `evaluate`），
/// 也让 `has_placeholders` 为假时 `literals[0]` 就是完整原文。
///
/// ## 输入对照表
///
/// | 输入 | 占位数 | `literals` |
/// |---|---|---|
/// | `a{{x}}b` | 1 | `["a", "b"]` |
/// | `{{x}}{{y}}` | 2 | `["", "", ""]` |
/// | `a{{b{{c}}d}}e` | 1（表达式 `c`） | `["a{{b", "d}}e"]` |
/// | `a{{x` | 0（未闭合） | `["a{{x"]` |
/// | `a}}b` | 0 | `["a}}b"]` |
/// | `a\{\{x\}\}b` | 0（转义） | `["a{{x}}b"]` |
///
/// ## 嵌套怎么处理
///
/// 公开文档没提嵌套，但 JS 里的对象字面量会让它自然出现
/// （`{{ {a:1}.a }}`）。这里的取向是"**按最内层配平、外层保留为字面**"：
/// `a{{b{{c}}d}}e` 解析为字面 `a{{b` + 占位 `c` + 字面 `d}}e`。
/// 这与"把 `{{`/`}}` 当括号整体匹配"不同，但更符合"取最内层表达式去求值"的直觉，
/// 且实现不需要栈（畸形输入不会爆栈，也不会因深度过大而退化）。
pub fn parse_interpolation(text: &str) -> Interpolation {
    let chars: Vec<char> = text.chars().collect();
    let mut literals = Vec::new();
    let mut placeholders = Vec::new();
    let mut literal = String::new();
    // 当前**最内层** `{{` 在 `literal` 里的起始下标。`None` 表示不在占位内。
    //
    // 用下标而不是一个"是否在占位内"的布尔：嵌套时后者需要额外维护表达式缓冲区，
    // 而下标方案下表达式就是 `literal` 的尾部切片——少一份状态，也就少一处不一致。
    let mut pending_open: Option<usize> = None;
    let mut index = 0usize;

    while index < chars.len() {
        let character = chars[index];

        // 转义：`\{` 与 `\}` 都按字面处理
        if character == '\\' {
            match chars.get(index + 1).copied() {
                Some(next @ ('{' | '}')) => {
                    literal.push(next);
                    index += 2;
                    continue;
                }
                _ => {
                    literal.push(character);
                    index += 1;
                    continue;
                }
            }
        }

        // `{{`：开一个占位；若已在占位内则**重定位**到新的最内层（"按最内层配平"）
        if character == '{' && chars.get(index + 1).copied() == Some('{') {
            literal.push('{');
            literal.push('{');
            pending_open = Some(literal.len() - 2);
            index += 2;
            continue;
        }

        // `}}`：闭合当前占位
        if character == '}' && chars.get(index + 1).copied() == Some('}') {
            if let Some(open) = pending_open.take() {
                let expression = literal[open + 2..].to_string();
                literal.truncate(open);
                literals.push(std::mem::take(&mut literal));
                placeholders.push(build_placeholder(&expression));
                // `}}` **不**进任何字面段：它与 `{{` 一起构成占位的定界符，
                // 不属静态文本。下一条测试会断言这一点。
            } else {
                // 无配对的 `}}`：当字面保留（畸形输入不报错）
                literal.push_str("}}");
            }
            index += 2;
            continue;
        }

        literal.push(character);
        index += 1;
    }

    literals.push(literal);

    // 不变量：字面数 = 占位数 + 1
    debug_assert_eq!(literals.len(), placeholders.len() + 1);

    Interpolation {
        literals,
        placeholders,
    }
}

/// 按前缀分派一个占位表达式。
fn build_placeholder(expression: &str) -> Placeholder {
    let trimmed = expression.trim();
    // `{{}}` 里默认是 js（公开文档原文），故无前缀时按 js 处理 —— 而不是像
    // 普通规则那样落到 Default。这个区别是本模块与 dispatch 的唯一分歧点，
    // 通过显式构造 RuleSegment 表达。
    let kind = if trimmed.is_empty() {
        RuleKind::JavaScript
    } else {
        let dispatched = dispatch_prefix(trimmed);
        match dispatched.kind {
            // 无前缀 → `{{}}` 语境下是 js（覆盖 dispatch 的 Default 结论）
            RuleKind::Default => RuleKind::JavaScript,
            // `:` 开头的正则里层不合法（公开文档：「正则除外」）
            RuleKind::RegexAllInOne => RuleKind::RegexAllInOne,
            other => other,
        }
    };

    Placeholder {
        expression: trimmed.to_string(),
        kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 替换链 ----------

    #[test]
    fn 解析公开文档的示例() {
        // 公开文档示例：class.odd.0@tag.a.0@text||tag.dd.0@tag.h1@text##全文阅读
        let chain = parse_replace_chain("class.odd.0@tag.a.0@text||tag.dd.0@tag.h1@text##全文阅读");
        assert_eq!(
            chain.subject,
            "class.odd.0@tag.a.0@text||tag.dd.0@tag.h1@text"
        );
        assert_eq!(chain.replacements.len(), 1);
        assert_eq!(chain.replacements[0].pattern, "全文阅读");
        assert_eq!(chain.replacements[0].replacement, "");
    }

    #[test]
    fn 无替换链时主体原样() {
        let chain = parse_replace_chain(".list@text");
        assert_eq!(chain.subject, ".list@text");
        assert!(chain.is_empty());
    }

    #[test]
    fn 单条替换含替换内容() {
        let chain = parse_replace_chain(r".list@text##\s+##");
        assert_eq!(chain.subject, ".list@text");
        assert_eq!(chain.replacements.len(), 1);
        assert_eq!(chain.replacements[0].pattern, r"\s+");
        assert_eq!(chain.replacements[0].replacement, "");
    }

    #[test]
    fn 多条替换按序叠加() {
        // FR-004：「可叠加」
        let chain = parse_replace_chain("x##a##b##c##d");
        assert_eq!(chain.subject, "x");
        assert_eq!(chain.replacements.len(), 2);
        assert_eq!(chain.replacements[0].pattern, "a");
        assert_eq!(chain.replacements[0].replacement, "b");
        assert_eq!(chain.replacements[1].pattern, "c");
        assert_eq!(chain.replacements[1].replacement, "d");
    }

    #[test]
    fn 主观缺省的替换链() {
        // 公开文档：「独立使用相当于 all##正则表达式##替换内容」
        let chain = parse_replace_chain("##广告##");
        assert_eq!(chain.subject, "");
        assert_eq!(chain.replacements.len(), 1);
        assert_eq!(chain.replacements[0].pattern, "广告");
    }

    #[test]
    fn 替换链分隔符的引号感知() {
        // 引号内的 `##` 不是分隔符。输入为：a@text  ##  x"##"y  ##  z
        let chain = parse_replace_chain("a@text##x\"##\"y##z");
        assert_eq!(chain.subject, "a@text");
        assert_eq!(chain.replacements.len(), 1);
        assert_eq!(chain.replacements[0].pattern, "x\"##\"y");
        assert_eq!(chain.replacements[0].replacement, "z");
    }

    #[test]
    fn 替换链分隔符的括号感知() {
        // 方括号内的 `##`（CSS 属性选择器或字符类）不是分隔符
        let chain = parse_replace_chain(r"a@text##[#]{2}##X");
        assert_eq!(chain.subject, "a@text");
        assert_eq!(chain.replacements.len(), 1);
        assert_eq!(chain.replacements[0].pattern, "[#]{2}");
        assert_eq!(chain.replacements[0].replacement, "X");
    }

    #[test]
    fn 替换链分隔符的转义感知() {
        let chain = parse_replace_chain(r"a##\#\###b");
        assert_eq!(chain.subject, "a");
        assert_eq!(chain.replacements.len(), 1);
        assert_eq!(chain.replacements[0].pattern, r"\#\#");
        assert_eq!(chain.replacements[0].replacement, "b");
    }

    #[test]
    fn 循环匹配替换() {
        // 公开文档：「该规则为循环匹配替换」——等价于 replace_all
        let chain = parse_replace_chain("x##a##b");
        assert_eq!(chain.apply("aaa", RegexLimits::DEFAULT), "bbb");
    }

    #[test]
    fn 多条替换按序生效() {
        // 先去掉空白，再把逗号换成顿号。顺序反了结果不同，故这个测试同时
        // 证明"按序"确实成立。
        let chain = parse_replace_chain(r"x##\s+####,##、");
        assert_eq!(chain.apply("a b,c d", RegexLimits::DEFAULT), "ab、cd");

        // 顺序相反：先去逗号，再删空白 —— 同样输入应得到同样结果（此处恰好一致），
        // 换个能被顺序影响的例子：
        let chain = parse_replace_chain("x##ab##ba##ba##ab");
        // 先 ab→ba（"ab" → "ba"），再 ba→ab（把刚产生的 "ba" 换回 "ab"）
        assert_eq!(chain.apply("ab", RegexLimits::DEFAULT), "ab");
    }

    #[test]
    fn 替换串过翻译层不丢字() {
        // T024 陷阱二：`$1a` 若不翻译会被 Rust 读成组名 `1a` 而静默丢字
        let chain = parse_replace_chain(r"x##(\d+)##$1a");
        assert_eq!(chain.apply("42", RegexLimits::DEFAULT), "42a");
    }

    #[test]
    fn 单条替换失败不中断整条链() {
        // 第一条正则编译失败（`{2,1}` 的重复次数区间非法），第二条仍应生效。
        let chain = parse_replace_chain("x##a{2,1}##never##ok##fine");
        assert_eq!(chain.replacements.len(), 2);
        assert_eq!(chain.replacements[0].pattern, "a{2,1}");
        assert_eq!(chain.replacements[1].pattern, "ok");
        assert_eq!(chain.replacements[1].replacement, "fine");
        // 第一条被跳过（不改变文本），第二条正常生效
        assert_eq!(chain.apply("ok", RegexLimits::DEFAULT), "fine");
    }

    #[test]
    fn 未闭合的括号会吞掉后续分隔符() {
        // 这**是**刻意的行为：`[` 与 `(` 在正则里必须配平（字面量要写成 `\[`），
        // 故未配平即说明正则本身写错了。此时把后续 `##` 一并当作正则内容，
        // 会让该条正则编译失败并被跳过——结果是**原文保留**（可读），
        // 而不是错误地切成两条半截正则（两条都编译失败，结果一样但顺序错乱）。
        let chain = parse_replace_chain("x##[unclosed##y##z");
        assert_eq!(
            chain.replacements.len(),
            1,
            "整个未配平段被当作一条（很长的）正则"
        );
        assert_eq!(chain.replacements[0].pattern, "[unclosed##y##z");
        // 该正则编译失败（未配平的 `[`）→ 被跳过 → 文本原样
        assert_eq!(chain.apply("abc", RegexLimits::DEFAULT), "abc");
    }

    #[test]
    fn 超长输入下替换链不崩溃() {
        let chain = parse_replace_chain("x##a##b");
        let long = "a".repeat(RegexLimits::DEFAULT.max_input_bytes + 10);
        // 超限时跳过该条替换，保留原文（而不是 panic 或报错）
        assert_eq!(chain.apply(&long, RegexLimits::DEFAULT), long);
    }

    #[test]
    fn 替换链的净化用途() {
        // 公开文档给的正文净化示例形态：@css:.articleDiv p@textNodes##搜索.*手机访问|一秒记住.*|.*阅读下载|
        let chain = parse_replace_chain(
            r"@css:.articleDiv p@textNodes##搜索.*手机访问|一秒记住.*|.*阅读下载|",
        );
        assert_eq!(chain.subject, "@css:.articleDiv p@textNodes");
        assert_eq!(chain.replacements.len(), 1);
        assert_eq!(
            chain.apply("正文内容一秒记住本站", RegexLimits::DEFAULT),
            "正文内容"
        );
    }

    // ---------- {{}} 解析 ----------

    #[test]
    fn 解析简单占位() {
        let interpolation = parse_interpolation("a{{x}}b");
        assert_eq!(interpolation.literals, vec!["a", "b"]);
        assert_eq!(interpolation.placeholders.len(), 1);
        assert_eq!(interpolation.placeholders[0].expression, "x");
        // `{{}}` 里默认是 js（公开文档）
        assert_eq!(interpolation.placeholders[0].kind, RuleKind::JavaScript);
        assert!(interpolation.has_javascript());
    }

    #[test]
    fn 解析多个占位() {
        let interpolation = parse_interpolation("{{a}}{{b}}");
        assert_eq!(interpolation.literals, vec!["", "", ""]);
        assert_eq!(interpolation.placeholders.len(), 2);
        assert_eq!(interpolation.placeholders[0].expression, "a");
        assert_eq!(interpolation.placeholders[1].expression, "b");
    }

    #[test]
    fn 无占位时是纯静态文本() {
        let interpolation = parse_interpolation("https://example.com/search?q=x");
        assert_eq!(
            interpolation.literals,
            vec!["https://example.com/search?q=x"]
        );
        assert!(!interpolation.has_placeholders());
        assert!(!interpolation.has_javascript());
    }

    #[test]
    fn 公开文档的搜索url形态() {
        // 公开文档示例：https://www.missevan.com/dramaapi/search?s={{key}}&page=1
        let interpolation = parse_interpolation("https://x.com/search?s={{key}}&page={{page}}");
        assert_eq!(interpolation.placeholders.len(), 2);
        assert_eq!(interpolation.placeholders[0].expression, "key");
        assert_eq!(interpolation.placeholders[1].expression, "page");
        assert_eq!(
            interpolation.literals,
            vec!["https://x.com/search?s=", "&page=", ""]
        );
    }

    #[test]
    fn 公开文档的表达式形态() {
        // 公开文档示例：{{(page-1)*20}} 与 {{page - 1 == 0 ? "": page}}
        let interpolation = parse_interpolation("{{(page-1)*20}}");
        assert_eq!(interpolation.placeholders[0].expression, "(page-1)*20");

        let interpolation = parse_interpolation(r#"{{page - 1 == 0 ? "": page}}"#);
        assert_eq!(
            interpolation.placeholders[0].expression,
            r#"page - 1 == 0 ? "": page"#
        );
    }

    #[test]
    fn 占位的显式类型前缀() {
        // 公开文档：「使用其他规则需要有明显的标志头」
        for (input, expected) in [
            ("{{@@div.a@text}}", RuleKind::ExplicitDefault),
            ("{{@css:.a@text}}", RuleKind::Css),
            ("{{@json:$.a}}", RuleKind::JsonPath),
            ("{{$.a}}", RuleKind::JsonPath),
            ("{{@xpath://a}}", RuleKind::XPath),
            ("{{//a}}", RuleKind::XPath),
            ("{{@js:1+1}}", RuleKind::JavaScript),
        ] {
            let interpolation = parse_interpolation(input);
            assert_eq!(interpolation.placeholders[0].kind, expected, "输入 {input}");
        }
    }

    #[test]
    fn 规则类占位本阶段可求值() {
        let interpolation = parse_interpolation("{{@css:.a@text}}");
        assert!(interpolation.placeholders[0].is_ready());
        assert!(interpolation.first_not_ready().is_none());
    }

    #[test]
    fn js_类占位自_s12_起已就绪() {
        // S1-2 的状态变更：`{{}}` 里默认是 js 的占位从 NotYet 变 Yes
        // （沙箱 + 桥已落地）。这条测试记录那个变更。
        let interpolation = parse_interpolation("{{key}}");
        assert_eq!(interpolation.placeholders[0].kind, RuleKind::JavaScript);
        assert!(interpolation.placeholders[0].is_ready());
        assert!(interpolation.first_not_ready().is_none());
    }

    #[test]
    fn jsonpath_类占位仍未就绪() {
        // JSONPath 与 XPath 确实要到 S1-3——它们的"未就绪"是**真**的
        // 待后续切片，而不是"永远不做"。故仍产出 FR-006 的结构化错误。
        for input in [
            "{{$.data.name}}",
            "{{@json:$.a}}",
            "{{@xpath://a}}",
            "{{//a}}",
        ] {
            let interpolation = parse_interpolation(input);
            let placeholder = &interpolation.placeholders[0];
            assert!(!placeholder.is_ready(), "输入 {input}");
            let error = interpolation
                .first_not_ready()
                .expect("应有未就绪占位")
                .not_ready_error()
                .expect("应产出错误");
            let expected = match placeholder.kind {
                RuleKind::JsonPath => "jsonPath",
                RuleKind::XPath => "xpath",
                other => panic!("输入 {input} 的实际类型是 {other:?}"),
            };
            match error.to_error_kind() {
                tome_core::ErrorKind::UnsupportedCapability { capability } => {
                    assert_eq!(capability.capability_id(), expected, "输入 {input}");
                }
                other => panic!("应产出 UnsupportedCapability，实际 {other:?}（输入 {input}）"),
            }
        }
    }

    #[test]
    fn 嵌套按最内层配平() {
        let interpolation = parse_interpolation("a{{b{{c}}d}}e");
        assert_eq!(interpolation.placeholders.len(), 1);
        assert_eq!(interpolation.placeholders[0].expression, "c");
        // `{{` 归前一段字面，`}}` 不属任何字面段（它是定界符的一部分）
        assert_eq!(interpolation.literals[0], "a{{b");
        assert_eq!(interpolation.literals[1], "d}}e");
    }

    #[test]
    fn 未闭合的花括号当字面保留() {
        // 畸形输入不报错也不崩溃——书源里手写 URL 出错是常态
        let interpolation = parse_interpolation("a{{x");
        assert!(!interpolation.has_placeholders());
        assert_eq!(interpolation.literals, vec!["a{{x"]);

        let interpolation = parse_interpolation("a}}b");
        assert!(!interpolation.has_placeholders());
        assert_eq!(interpolation.literals, vec!["a}}b"]);
    }

    #[test]
    fn 转义的花括号是字面() {
        let interpolation = parse_interpolation(r"a\{\{x\}\}b");
        assert!(!interpolation.has_placeholders());
        assert_eq!(interpolation.literals, vec!["a{{x}}b"]);
    }

    #[test]
    fn 空占位不崩溃() {
        let interpolation = parse_interpolation("a{{}}b");
        assert_eq!(interpolation.placeholders.len(), 1);
        assert_eq!(interpolation.placeholders[0].expression, "");
        // 空表达式按 js 处理（无害：它本阶段就是未就绪）
        assert_eq!(interpolation.placeholders[0].kind, RuleKind::JavaScript);
    }

    // ---------- 桥接 trait ----------

    /// 测试桩：规则类走真实的 CSS 求值，js 类返回结构化未就绪错误。
    struct StubBridge<'a> {
        document: &'a crate::dom::Document,
    }

    impl PlaceholderBridge for StubBridge<'_> {
        fn evaluate_js(&self, _expression: &str) -> Result<String, RuleError> {
            Err(RuleError::unsupported("js", "interpolation"))
        }

        fn evaluate_rule(&self, expression: &str) -> Result<String, RuleError> {
            crate::css_post::evaluate_rule(self.document, expression).map(|values| values.join(""))
        }
    }

    #[test]
    fn 桥接求值拼接出最终字符串() {
        let document = crate::dom::Document::parse_html(
            r#"<div class="t">标题</div><div class="c">正文</div>"#,
        );
        let bridge = StubBridge {
            document: &document,
        };
        let interpolation = parse_interpolation("[{{@css:.t@text}}]{{@css:.c@text}}");
        let result = interpolation.evaluate(&bridge).expect("应求值成功");
        assert_eq!(result, "[标题]正文");
    }

    #[test]
    fn 桥接对js占位分出到_evaluate_js() {
        // S1-2 起 js 占位**可求值**，故 `evaluate` 应把它交给
        // `evaluate_js`（而不再产出"未就绪"错误）。
        // 这里用桩的"永远返回 Unsupported"来证明**路由发生了**。
        let document = crate::dom::Document::parse_html("<p>x</p>");
        let bridge = StubBridge {
            document: &document,
        };
        let interpolation = parse_interpolation("/search?q={{key}}");
        let error = interpolation
            .evaluate(&bridge)
            .expect_err("桩会报错（它模拟的是未实现的环境）");
        match error.to_error_kind() {
            // 桩的 `evaluate_js` 返回 `unsupported("js", ..)`，能力标识
            // 与占位类型一致——这说明路由走对了。
            tome_core::ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "js");
            }
            other => panic!("应产出 UnsupportedCapability，实际 {other:?}"),
        }
    }

    #[test]
    fn 桥接对jsonpath占位产出结构化错误() {
        let document = crate::dom::Document::parse_html("<p>x</p>");
        let bridge = StubBridge {
            document: &document,
        };
        let interpolation = parse_interpolation("{{$.data.name}}");
        let error = interpolation
            .evaluate(&bridge)
            .expect_err("JSONPath 占位本阶段应报错");
        match error.to_error_kind() {
            tome_core::ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "jsonPath");
            }
            other => panic!("应产出 UnsupportedCapability，实际 {other:?}"),
        }
    }

    #[test]
    fn 纯静态模板无需桥即可求值() {
        let document = crate::dom::Document::parse_html("<p>x</p>");
        let bridge = StubBridge {
            document: &document,
        };
        let interpolation = parse_interpolation("https://example.com/a?b=1");
        assert_eq!(
            interpolation.evaluate(&bridge).expect("应求值成功"),
            "https://example.com/a?b=1"
        );
    }
}
