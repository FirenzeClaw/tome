//! CSS 选择器引擎（tasks.md T021 / research.md R2）
//!
//! # 架构
//!
//! 用 servo `selectors`（Firefox 同款引擎）做标准选择器的解析与匹配，把它接到
//! [`crate::dom`] 的自建 arena DOM 上（实现 `selectors::Element`）。
//! "一棵 DOM，两个查询引擎"（research R2）——CSS 与将来的 XPath 跑同一棵树，
//! 结果不会系统性分叉。
//!
//! # 兼容目标：Jsoup 1.2x 的 Selector 行为
//!
//! research R2 明确把对齐目标钉在 Jsoup 1.2x。Jsoup 的公开文档
//! （jsoup.org apidocs 的 `Selector` 一节）是唯一合法依据，本模块的每条非常规
//! 行为都标注了对应条目。
//!
//! ## 三处在 CSS 标准里不成立的 Jsoup 方言（必须预处理）
//!
//! | 方言 | 标准 CSS 里的含义 | Jsoup 的含义 |
//! |---|---|---|
//! | `[attr~=X]` | X 是**空白分词**之一 | X 是**正则表达式** |
//! | `[^prefix]` | 非法 | 属性名以 prefix 开头 |
//! | `[*]` | 非法 | 有任意属性 |
//!
//! `~=` 的重载是 R2 点名的"同一条规则两边行为不同，必须特判"。前两项要靠
//! [`rewrite_jsoup_dialect`] 在交给 `selectors` 之前改写（它们连语法都过不了
//! CSS 解析器），`~=` 则在 [`Element`] 的 `attr_matches` 里换掉匹配算法。
//!
//! ## 大小写
//!
//! Jsoup 公开文档 `Selector` 一节开篇：
//!
//! > Selectors are **case-insensitive** (including against elements, attributes,
//! > and attribute values).
//!
//! 故 `has_class` 恒做 ASCII 大小写不敏感匹配；标签名由 DOM 层保证已小写
//! （HTML5 树构造的既有行为）；属性名在查询时做小写归一。

use std::borrow::Borrow;
use std::fmt;

use cssparser::{CowRcStr, ParseError, Parser as CssParser, ToCss};
use html5ever::tree_builder::QuirksMode;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::bloom::BloomFilter;
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags,
    QuirksMode as SelectorQuirksMode, SelectorCaches,
};
use selectors::matching::{matches_selector, ElementSelectorFlags};
use selectors::parser::{
    NonTSPseudoClass, ParseRelative, Parser, PseudoElement, SelectorImpl, SelectorList,
};
use selectors::{Element, OpaqueElement};

use crate::dom::{Document, NodeKind, NodeRef};
use crate::error::RuleError;
use crate::regex::{self, RegexLimits};

/// HTML 命名空间 URL。
const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";

// ---------------------------------------------------------------------------
// 标识符类型
// ---------------------------------------------------------------------------

/// CSS 里出现的字符串（元素名/命名空间/类名/id/属性名/属性值）。
///
/// 用一个统一 newtype 而非 `String`：`SelectorImpl` 要求这些类型实现
/// `PrecomputedHash`（供 Bloom 过滤器快速排除），而 `String` 没有该实现。
/// 用 newtype 也比 `string_cache::Atom` 更合适——本项目不做跨线程共享，
/// 原子化的开销白付。
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct CssName(String);

impl CssName {
    /// 取内部字符串。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CssName {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl From<String> for CssName {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl Borrow<str> for CssName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for CssName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl ToCss for CssName {
    fn to_css<W>(&self, dest: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        // 只用于错误信息与往返调试，故按"原样输出"处理而非按 CSS 标识符转义规则
        // 重新编码——转义规则会让 `div` 变成 `div`，但也会让带特殊字符的名字
        // 输出成 `\3x ` 之类难以阅读的形式。
        dest.write_str(&self.0)
    }
}

impl precomputed_hash::PrecomputedHash for CssName {
    fn precomputed_hash(&self) -> u32 {
        // 与 `std::hash::Hash` 的默认实现无关的独立散列：Bloom 过滤器只需要
        // "同值同散列、分布均匀"。用 FNV-1a（32 位）——短字符串上比 SipHash 快，
        // 且不需要引入额外依赖。
        let mut hash: u32 = 0x811c_9dc5;
        for byte in self.0.as_bytes() {
            hash ^= u32::from(*byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
        hash
    }
}

// ---------------------------------------------------------------------------
// 伪类与伪元素
// ---------------------------------------------------------------------------

/// Jsoup 专有伪类（research R2：标准伪类由 servo 覆盖，专有的一律自研）。
///
/// ## 支持的清单与依据
///
/// 全部取自 Jsoup 公开文档 `Selector` 一节的伪类表，逐条对应：
/// `:contains` / `:containsOwn` / `:containsData` / `:matches` /
/// `:lt` / `:gt` / `:eq` / `:lang`。
///
/// ## 为什么用"预编译正则"存文本参数
///
/// `:contains` 是**大小写不敏感的子串匹配**、`:containsWholeText` 是精确匹配、
/// `:matches` 是正则匹配。把它们统一表达为"一个已编译的正则"能少一条分支，
/// 但也丢掉了非正则路径的线性优势。故这里按语义分三种存储：子串（大小写不敏感）、
/// 精确（大小写敏感）、正则。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JsoupPseudoClass {
    /// `:contains(text)` —— 后代文本包含 text（大小写不敏感，空白归一化）
    Contains(String),
    /// `:containsOwn(text)` —— **自身**文本包含 text（大小写不敏感）
    ContainsOwn(String),
    /// `:containsData(data)` —— data node 内容包含 data（大小写不敏感）
    ContainsData(String),
    /// `:containsWholeText(text)` —— 后代文本精确包含 text（大小写敏感，不归一化）
    ContainsWholeText(String),
    /// `:containsWholeOwnText(text)` —— 自身文本精确包含（大小写敏感，不归一化）
    ContainsWholeOwnText(String),
    /// `:matches(regex)` —— 后代文本匹配正则（空白归一化）
    Matches(String),
    /// `:matchesOwn(regex)` —— **自身**文本匹配正则（不含后代）
    ///
    /// 依据 Jsoup 公开文档 `Selector` 一节的伪类表（`:matches` 的自身版）。
    /// 与 [`Self::MatchesWholeOwnText`] 的区别在**空白归一化**：本变体归一化，
    /// 那个不归一化。
    MatchesOwn(String),
    /// `:matchesWholeText(regex)` —— 后代文本匹配正则（不归一化）
    MatchesWholeText(String),
    /// `:matchesWholeOwnText(regex)` —— 自身文本匹配正则（不归一化）
    MatchesWholeOwnText(String),
    /// `:eq(n)` —— **兄弟索引**等于 n（0 基）
    Eq(i32),
    /// `:lt(n)` —— 兄弟索引小于 n
    Lt(i32),
    /// `:gt(n)` —— 兄弟索引大于 n
    Gt(i32),
    /// `:lang(l)`
    Lang(String),
    /// `[attr~=regex]` 方言改写而来：属性值匹配正则。
    ///
    /// 携带 (属性名小写, 正则原文)。正则的**已编译**形态在
    /// [`AttrRegexCache`]（经 `extra_data` 注入），此处只带键。
    AttrMatches {
        attribute: String,
        pattern: String,
    },
    /// `[^prefix]` 方言改写而来：属性名以 prefix 开头
    AttrNamePrefix(String),
    /// `[*]` 方言改写而来：有任意属性
    AnyAttribute,
}

impl ToCss for JsoupPseudoClass {
    fn to_css<W>(&self, dest: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        // 只用于诊断输出
        match self {
            Self::Contains(text) => write!(dest, ":contains({text})"),
            Self::ContainsOwn(text) => write!(dest, ":containsOwn({text})"),
            Self::ContainsData(text) => write!(dest, ":containsData({text})"),
            Self::ContainsWholeText(text) => write!(dest, ":containsWholeText({text})"),
            Self::ContainsWholeOwnText(text) => write!(dest, ":containsWholeOwnText({text})"),
            Self::Matches(text) => write!(dest, ":matches({text})"),
            Self::MatchesOwn(text) => write!(dest, ":matchesOwn({text})"),
            Self::MatchesWholeText(text) => write!(dest, ":matchesWholeText({text})"),
            Self::MatchesWholeOwnText(text) => write!(dest, ":matchesWholeOwnText({text})"),
            Self::Eq(index) => write!(dest, ":eq({index})"),
            Self::Lt(index) => write!(dest, ":lt({index})"),
            Self::Gt(index) => write!(dest, ":gt({index})"),
            Self::Lang(language) => write!(dest, ":lang({language})"),
            Self::AttrMatches { attribute, pattern } => {
                write!(dest, "[{attribute}~={pattern}]")
            }
            Self::AttrNamePrefix(prefix) => write!(dest, "[^{prefix}]"),
            Self::AnyAttribute => dest.write_str("[*]"),
        }
    }
}

impl NonTSPseudoClass for JsoupPseudoClass {
    fn is_active_or_hover(&self) -> bool {
        false
    }

    fn is_user_action_state(&self) -> bool {
        false
    }
}

/// 伪元素。本项目不支持任何伪元素——Jsoup 的 `::text` / `::data` / `::comment`
/// 是**节点选择器**而非伪元素（它们选的是节点本身，不是元素的生成内容），
/// 语义上属 [`crate::css_post`] 的节点过滤，不在这里。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsoupPseudoElement;

impl ToCss for JsoupPseudoElement {
    fn to_css<W>(&self, dest: &mut W) -> fmt::Result
    where
        W: fmt::Write,
    {
        dest.write_str("::jsoup-pseudo-element")
    }
}

impl PseudoElement for JsoupPseudoElement {
    fn valid_after_slotted(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// SelectorImpl
// ---------------------------------------------------------------------------

/// servo `selectors` 的实现参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsoupSelectorImpl;

impl SelectorImpl for JsoupSelectorImpl {
    /// 匹配期的附加数据：`[attr~=regex]` 的已编译正则表。
    ///
    /// `selectors` 为"实现方需要向匹配过程传递额外状态"提供了这个注入口，
    /// 这里用它把正则**编译一次**而不是每个元素编译一次（见 [`AttrRegexCache`]）。
    type ExtraMatchingData<'a> = Option<&'a AttrRegexCache>;
    type AttrValue = CssName;
    type Identifier = CssName;
    type LocalName = CssName;
    type NamespaceUrl = CssName;
    type NamespacePrefix = CssName;
    type BorrowedNamespaceUrl = str;
    type BorrowedLocalName = str;
    type NonTSPseudoClass = JsoupPseudoClass;
    type PseudoElement = JsoupPseudoElement;
}

// ---------------------------------------------------------------------------
// 解析器
// ---------------------------------------------------------------------------

/// 选择器解析器。开哪几个语法开关都有明确依据。
struct JsoupSelectorParser {
    /// 解析期收集的 `[attr~=regex]` 正则原文，按出现顺序。
    ///
    /// 为什么在这里收集：`~=` 的值被 [`rewrite_jsoup_dialect`] 改写成带引号的
    /// CSS 字符串后才能通过 CSS 语法解析，改写时无法在 `attr_matches` 里回溯
    /// 出"它原本是正则"。故把原文按 (属性名 → 正则) 记在这里，匹配时查表。
    /// 用 `Vec<(String, String)>` 而非 map：同一属性可以出现多次且顺序稳定，
    /// 便于确定性。
    regex_attrs: Vec<(String, String)>,
}

/// 解析选择器时开的语法开关（逐条给依据）。
impl<'i> Parser<'i> for JsoupSelectorParser {
    type Impl = JsoupSelectorImpl;
    type Error = selectors::parser::SelectorParseErrorKind;

    fn parse_slotted(&self) -> bool {
        // 书源不涉及 Shadow DOM，关掉可让 `::slotted` 明确报错而非静默接受。
        false
    }

    fn parse_part(&self) -> bool {
        false
    }

    fn parse_nth_child_of(&self) -> bool {
        // `:nth-child(An+B of S)` 是 Selectors 4 语法。Jsoup 1.2x 的公开文档
        // 只列了 `:nth-child(an+b)`，故不开——多支持会让本该报错的规则静默通过，
        // 反而不利于暴露书源问题。
        false
    }

    fn parse_is_and_where(&self) -> bool {
        // Jsoup 公开文档明确列出 `:is(selector list)` 与 `:where`（后者虽未在
        // 表格里单列，但同属 Selectors 4 的别名机制）。故开启。
        true
    }

    fn parse_has(&self) -> bool {
        // 公开文档明确列出 `:has(selector)`，并给出 `div:has(> a)` 与
        // `section:has(h1, h2)` 两种写法（相对选择器）。故开启。
        true
    }

    fn parse_parent_selector(&self) -> bool {
        // `&` 是 CSS Nesting 语法，书源里不出现。
        false
    }

    fn is_is_alias(&self, name: &str) -> bool {
        // `:-jsoup-is()` 之类的别名不存在；显式返回 false。
        let _ = name;
        false
    }

    fn parse_host(&self) -> bool {
        false
    }

    fn parse_non_ts_pseudo_class(
        &self,
        name: CowRcStr<'i>,
    ) -> Result<JsoupPseudoClass, ParseError<Self::Error>> {
        match name.as_ref() {
            // `[*]` 方言改写目标（见 rewrite_jsoup_dialect）。它无实参，
            // 故走非函数形式的钩子。
            "jsoupAnyAttribute" => Ok(JsoupPseudoClass::AnyAttribute),
            // Jsoup 的 `:matchText` 已废弃（公开文档标 "Deprecated"），且它要改写
            // DOM（把文本节点包装成伪元素），与本项目的只读查询模型不符，明确不支持。
            _ => Err(ParseError::custom(
                selectors::parser::SelectorParseErrorKind::UnsupportedPseudoClassOrElement,
            )),
        }
    }

    fn parse_non_ts_functional_pseudo_class(
        &self,
        name: CowRcStr<'i>,
        parser: &mut CssParser<'i>,
        _after_part: bool,
    ) -> Result<JsoupPseudoClass, ParseError<Self::Error>> {
        match name.as_ref() {
            "contains" => Ok(JsoupPseudoClass::Contains(read_text_argument(parser)?)),
            "containsOwn" => Ok(JsoupPseudoClass::ContainsOwn(read_text_argument(parser)?)),
            "containsData" => Ok(JsoupPseudoClass::ContainsData(read_text_argument(parser)?)),
            "containsWholeText" => Ok(JsoupPseudoClass::ContainsWholeText(read_text_argument(
                parser,
            )?)),
            "containsWholeOwnText" => Ok(JsoupPseudoClass::ContainsWholeOwnText(
                read_text_argument(parser)?,
            )),
            "matches" => Ok(JsoupPseudoClass::Matches(read_text_argument(parser)?)),
            // 公开文档伪类表列了 `:matchesOwn(regex)`（`:matches` 的自身版）
            "matchesOwn" => Ok(JsoupPseudoClass::MatchesOwn(read_text_argument(parser)?)),
            "matchesWholeText" => Ok(JsoupPseudoClass::MatchesWholeText(read_text_argument(
                parser,
            )?)),
            "matchesWholeOwnText" => Ok(JsoupPseudoClass::MatchesWholeOwnText(read_text_argument(
                parser,
            )?)),
            // 索引类伪类按 Jsoup 文档接受负值（"index number" 为 int）。
            // 负号在 CSS 语法里是单独的分隔符 token，故用带符号解析。
            "eq" => Ok(JsoupPseudoClass::Eq(read_signed_integer(parser)?)),
            "lt" => Ok(JsoupPseudoClass::Lt(read_signed_integer(parser)?)),
            "gt" => Ok(JsoupPseudoClass::Gt(read_signed_integer(parser)?)),
            "lang" => Ok(JsoupPseudoClass::Lang(read_text_argument(parser)?)),
            // `[attr~=X]` 的改写目标（见 rewrite_jsoup_dialect）
            "jsoupAttrMatches" => {
                let attribute = parser.expect_string()?.as_ref().to_ascii_lowercase();
                parser.expect_comma()?;
                let pattern = parser.expect_string()?.as_ref().to_string();
                Ok(JsoupPseudoClass::AttrMatches { attribute, pattern })
            }
            // `[^prefix]` 的改写目标
            "jsoupAttrNamePrefix" => Ok(JsoupPseudoClass::AttrNamePrefix(read_text_argument(
                parser,
            )?)),
            _ => Err(ParseError::custom(
                selectors::parser::SelectorParseErrorKind::UnsupportedPseudoClassOrElement,
            )),
        }
    }

    fn parse_pseudo_element(
        &self,
        _name: CowRcStr<'i>,
    ) -> Result<JsoupPseudoElement, ParseError<Self::Error>> {
        Err(ParseError::custom(
            selectors::parser::SelectorParseErrorKind::UnsupportedPseudoClassOrElement,
        ))
    }

    fn parse_functional_pseudo_element(
        &self,
        _name: CowRcStr<'i>,
        _arguments: &mut CssParser<'i>,
    ) -> Result<JsoupPseudoElement, ParseError<Self::Error>> {
        Err(ParseError::custom(
            selectors::parser::SelectorParseErrorKind::UnsupportedPseudoClassOrElement,
        ))
    }
}

/// 读一个伪类的文本参数（原样文本，不走 CSS 的 ident 规则）。
///
/// Jsoup 的文本参数允许不加引号（`p:contains(jsoup)`），且允许用 `\(` / `\)`
/// 转义括号（公开文档：*"To find content that includes parentheses, escape those
/// with a `\`"*）。故这里**不吃 CSS 的 token 语法**——那会把 `jsoup` 当成 ident、
/// 把 `\(` 当成非法转义、把 `a b` 当成两个 ident。
///
/// ## 为什么用「消费到块尾再取原文」
///
/// `selectors` 调用本钩子时**已经**用 `parse_nested_block` 打开了函数的括号块
/// （见 `selectors::parser::parse_functional_pseudo_class` 的调用点），故这里
/// **不能**再嵌套一次 `parse_nested_block`——那会 panic（cssparser 的
/// "A nested parser can only be created when a Function ... token was just consumed"）。
///
/// 块的停止点已被设为右括号，故把 token 消费干净再取原文即可拿到完整实参。
fn read_text_argument<'i>(
    parser: &mut CssParser<'i>,
) -> Result<String, ParseError<selectors::parser::SelectorParseErrorKind>> {
    let start = parser.position();
    // `next_including_whitespace_and_comments` 会返回 Err 于块结束点（右括号），
    // 故这个循环的终止条件就是"实参读完了"。
    while parser.next_including_whitespace_and_comments().is_ok() {}
    let raw = parser.slice_from(start).trim().to_string();

    // 去掉最外层成对的引号（`p:contains("x")` 与 `p:contains(x)` 都应成立）
    let unquoted = strip_outer_quotes(&raw);
    Ok(unescape_parens(unquoted))
}

/// 去掉最外层成对的引号（仅当首尾是同一个引号字符）。
fn strip_outer_quotes(text: &str) -> &str {
    let trimmed = text.trim();
    let mut chars = trimmed.chars();
    if let (Some(first), Some(last)) = (chars.next(), trimmed.chars().next_back()) {
        if (first == '"' || first == '\'') && first == last && trimmed.len() >= 2 {
            return &trimmed[first.len_utf8()..trimmed.len() - last.len_utf8()];
        }
    }
    trimmed
}

/// 把 `\(` / `\)` 还原成字面括号（Jsoup 公开文档的转义约定）。
fn unescape_parens(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\\' {
            match chars.peek().copied() {
                Some('(') | Some(')') => {
                    if let Some(escaped) = chars.next() {
                        output.push(escaped);
                    }
                }
                // 其它反斜杠原样保留——正则里的 `\d` 之类不能被吃掉
                _ => output.push('\\'),
            }
            continue;
        }
        output.push(character);
    }
    output
}

/// 读一个带符号整数参数（`:eq(-1)`）。
///
/// 负号在 CSS token 流里是独立分隔符，故要分别判断正负号是否出现。
fn read_signed_integer<'i>(
    parser: &mut CssParser<'i>,
) -> Result<i32, ParseError<selectors::parser::SelectorParseErrorKind>> {
    let negative = parser.try_parse(|input| input.expect_delim('-')).is_ok();
    let number = parser.expect_integer()?;
    Ok(if negative { -number } else { number })
}

// ---------------------------------------------------------------------------
// Jsoup 方言预处理
// ---------------------------------------------------------------------------

/// 把 Jsoup 专有方言改写成 `selectors` 能解析的等价形式。
///
/// 三项改写（依据见模块文档的表格）：
///
/// 1. `[attr~=X]` → `[attr~="X"]`（**加引号**，让含 `|`/`(`/`\` 的正则能过语法；
///    真正的正则匹配在 [`Element::attr_matches`] 里，正则原文由
///    [`ParsedSelector::regex_attrs`] 保存）
/// 2. `[^prefix]` → `:jsoupAttrNamePrefix(prefix)`
/// 3. `[*]` → `:jsoupAnyAttribute`
///
/// ## 为什么必须做词法扫描而不是字符串替换
///
/// 选择器里可以出现引号（`[title="a]b"]`），`]` 在引号内不结束属性选择器。
/// 朴素的 `find(']')` 会在这种输入上截断错误位置，把后续语法搅乱。
pub fn rewrite_jsoup_dialect(selector: &str) -> String {
    let chars: Vec<char> = selector.chars().collect();
    let mut output = String::with_capacity(selector.len() + 16);
    let mut index = 0usize;

    while index < chars.len() {
        let current = chars[index];

        // 引号内的整段原样搬运（含转义字符）
        if current == '"' || current == '\'' {
            let (literal, next) = take_quoted(&chars, index);
            output.push_str(&literal);
            index = next;
            continue;
        }

        if current == '[' {
            match take_attribute_selector(&chars, index) {
                Some((inner, next)) => {
                    output.push_str(&rewrite_attribute_selector(&inner));
                    index = next;
                    continue;
                }
                // 没有配对的 `]`：原样输出，让 CSS 解析器报出准确错误
                None => {
                    output.push(current);
                    index += 1;
                    continue;
                }
            }
        }

        output.push(current);
        index += 1;
    }

    output
}

/// 从 `start` 处的引号开始，取出完整的带引号字面量。返回 (文本, 下一个下标)。
fn take_quoted(chars: &[char], start: usize) -> (String, usize) {
    let quote = chars[start];
    let mut output = String::new();
    output.push(quote);
    let mut index = start + 1;
    while index < chars.len() {
        let current = chars[index];
        output.push(current);
        index += 1;
        if current == '\\' && index < chars.len() {
            output.push(chars[index]);
            index += 1;
            continue;
        }
        if current == quote {
            break;
        }
    }
    (output, index)
}

/// 从 `start` 处的 `[` 取出属性选择器的内层文本。返回 (内层, `]` 之后的下标)。
///
/// 跟踪嵌套深度：`[attr~=a[bc]]` 这类正则里会再出现 `[`，只认配对的 `]` 才结束
/// 属性选择器。否则正则在第一个 `]` 处被截断——`a[bc]+d` 是可复现的例子。
fn take_attribute_selector(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut inner = String::new();
    let mut depth = 0usize;
    let mut index = start + 1;
    while index < chars.len() {
        let current = chars[index];
        if current == '"' || current == '\'' {
            let (literal, next) = take_quoted(chars, index);
            inner.push_str(&literal);
            index = next;
            continue;
        }
        if current == '[' {
            depth += 1;
        } else if current == ']' {
            if depth == 0 {
                return Some((inner, index + 1));
            }
            depth -= 1;
        }
        inner.push(current);
        index += 1;
    }
    None
}

/// 属性选择器的运算符清单（长的在前，避免 `=` 抢在 `~=` 之前匹配）。
const ATTRIBUTE_OPERATORS: [&str; 6] = ["~=", "|=", "^=", "$=", "*=", "="];

/// 改写单个属性选择器的内层文本。
///
/// ## 为什么连**标准**运算符的值也要改写
///
/// Jsoup 的属性值解析是"读到 `]` 为止"，不吃 CSS 的 token 语法。公开文档给出的
/// 示例直接证明了这一点：
///
/// | 示例 | 为什么不是合法 CSS 标识符 |
/// |---|---|
/// | `a[href^=http:]` | `:` 不能出现在 ident 里 |
/// | `img[src$=.png]` | `.` 不能出现在 ident 里 |
/// | `a[href*=/search/]` | `/` 不能出现在 ident 里 |
///
/// 故本函数对**所有**裸值（未被引号包裹的）一律加引号。这是 Jsoup 宽容语法的
/// 忠实翻译，不是"顺手多做事"。
fn rewrite_attribute_selector(inner: &str) -> String {
    let trimmed = inner.trim();

    // 方言 2：`[^prefix]`
    if let Some(prefix) = trimmed.strip_prefix('^') {
        let prefix = strip_outer_quotes(prefix.trim());
        return format!(":jsoupAttrNamePrefix({})", quote_for_pseudo(prefix));
    }

    // 方言 3：`[*]`
    if trimmed == "*" {
        return ":jsoupAnyAttribute".to_string();
    }

    // 无运算符：`[attr]`（存在性检查），原样
    let found = ATTRIBUTE_OPERATORS.iter().find_map(|operator| {
        find_unescaped_operator(trimmed, operator).map(|position| (position, *operator))
    });
    let (position, operator) = match found {
        Some(found) => found,
        None => return format!("[{inner}]"),
    };

    // 属性名本身不能含引号，故原样；但属性名允许 `*`（`[*|attr]`）等，
    // 交给 CSS 解析器判断合法性。
    let name = trimmed[..position].trim();
    let raw_value = trimmed[position + operator.len()..].trim();
    let value = strip_outer_quotes(raw_value);

    // 值可能带大小写标志（`[attr=val i]`）。Jsoup 不做这件事，但 CSS 的标准写法
    // 出现在书源里时应被容忍——把末尾单独的 `i`/`s` 摘出来还原。
    let (value, flag) = split_trailing_case_flag(value);

    // 方言 1 的落点：`~=` 改写成功能伪类，把匹配交给带上下文的钩子，
    // 这样已编译的正则能从 `MatchingContext::extra_data` 取到（见
    // [`ParsedSelector::regex_cache`] 的文档）。属性名与模式都写成 CSS 字符串，
    // 故任意字符（`[`、`\`、`|`、`(`）都能安全通过语法解析。
    if operator == "~=" {
        return format!(
            ":jsoupAttrMatches({},{})",
            quote_css_string(&name.to_ascii_lowercase()),
            quote_css_string(value)
        );
    }

    let mut rewritten = format!("[{name}{operator}{}]", quote_css_string(value));
    if let Some(flag) = flag {
        rewritten.push(' ');
        rewritten.push(flag);
    }
    rewritten
}

/// 摘出属性值末尾的大小写标志（`i` / `s`）。返回 (值, 标志)。
///
/// 只在"值与标志之间有空白"时生效——否则 `[title=si]` 里的 `i` 会被误当标志。
fn split_trailing_case_flag(value: &str) -> (&str, Option<char>) {
    let trimmed = value.trim_end();
    if trimmed.len() < 2 {
        return (value, None);
    }
    let mut chars = trimmed.chars().rev();
    let mut last = chars.next();
    let before_last = chars.next();
    if let (Some(mut flag), Some(separator)) = (last, before_last) {
        if (flag == 'i' || flag == 'I' || flag == 's' || flag == 'S') && separator.is_whitespace() {
            if flag == 'I' {
                flag = 'i';
            }
            if flag == 'S' {
                flag = 's';
            }
            last = Some(flag);
            return (trimmed[..trimmed.len() - 1].trim_end(), last);
        }
    }
    (value, None)
}

/// 找 `needle` 在 `haystack` 里未被反斜杠转义的出现位置。
fn find_unescaped_operator(haystack: &str, needle: &str) -> Option<usize> {
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    if needle_bytes.is_empty() || bytes.len() < needle_bytes.len() {
        return None;
    }
    let mut index = 0usize;
    while index + needle_bytes.len() <= bytes.len() {
        if &bytes[index..index + needle_bytes.len()] == needle_bytes {
            // 数它前面连续的 `\`
            let mut backslashes = 0usize;
            let mut probe = index;
            while probe > 0 && bytes[probe - 1] == b'\\' {
                backslashes += 1;
                probe -= 1;
            }
            if backslashes.is_multiple_of(2) {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

/// 把文本拼成合法的 CSS 双引号字符串。
fn quote_css_string(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\a "),
            '\r' => output.push_str("\\d "),
            _ => output.push(character),
        }
    }
    output.push('"');
    output
}

/// 把文本拼成伪类实参（同样用双引号，保证任意文本都能通过语法解析）。
fn quote_for_pseudo(text: &str) -> String {
    quote_css_string(text)
}

/// 把文本编成功能伪类的实参（公开给 [`crate::default`] 用）。
///
/// # 为什么需要这个公开入口
///
/// Default 语法的 `text.片段` 段被翻译成 `*:containsOwn(片段)`，而片段是
/// 任意文本（可能含 `)`、引号、换行）。若不按 CSS 字符串规则编码，
/// 拼接出的选择器会解析失败或**改变语义**。
///
/// 故本函数是 [`quote_for_pseudo`] 的公开形态——**同一份编码逻辑**，
/// 不另写一份（两处各写一份必然在转义细节上漂移）。
pub fn quote_for_pseudo_argument(text: &str) -> String {
    quote_for_pseudo(text)
}

// ---------------------------------------------------------------------------
// 编译好的选择器
// ---------------------------------------------------------------------------

/// 编译好的选择器（可反复用于不同文档，符合"同一查询多次执行"的用法）。
#[derive(Debug)]
pub struct ParsedSelector {
    /// 规范化（方言改写后）的选择器文本，诊断用。
    normalized: String,
    /// 属性名 → 正则原文（`[attr~=regex]` 方言，见 [`rewrite_jsoup_dialect`]）。
    regex_attrs: Vec<(String, String)>,
    /// 已编译的 `[attr~=regex]` 正则，键为 `(属性名, 正则原文)`。
    ///
    /// ## 为什么预编译而不是匹配时现编
    ///
    /// `selectors::Element::attr_matches` **没有**上下文参数，拿不到本结构体。
    /// 若在那里现编正则，每次匹配都会重新编译——对 6 万个节点的文档就是 6 万次编译，
    /// 而正确做法是编译一次用到底。故 `[attr~=X]` 被改写成**功能伪类**
    /// `:jsoupAttrMatches("attr","X")`：伪类的钩子带 `MatchingContext`，
    /// 而 `MatchingContext.extra_data` 正是 `selectors` 为此提供的注入点。
    regex_cache: AttrRegexCache,
    /// servo 的选择器列表。
    list: SelectorList<JsoupSelectorImpl>,
}

/// `[attr~=regex]` 的已编译正则表。作为 `MatchingContext::extra_data` 注入匹配过程。
#[derive(Debug, Default)]
pub struct AttrRegexCache {
    /// `(属性名小写, 正则原文) → 已编译正则`
    ///
    /// 用 `Vec` 而非 `HashMap`：选择器里的 `[attr~=X]` 通常只有 0–2 个，
    /// 线性查找比哈希更快，且顺序稳定便于诊断输出。
    entries: Vec<AttrRegexEntry>,
}

#[derive(Debug)]
struct AttrRegexEntry {
    attribute: String,
    pattern: String,
    /// 编译结果。`Err` 时保留原因（诊断用），匹配时退化为字面包含。
    compiled: Result<regex::CompiledRegex, String>,
}

impl AttrRegexCache {
    fn insert(&mut self, attribute: String, pattern: String) {
        if self
            .entries
            .iter()
            .any(|entry| entry.attribute == attribute && entry.pattern == pattern)
        {
            return;
        }
        let compiled =
            regex::compile(&pattern, RegexLimits::DEFAULT).map_err(|error| error.to_string());
        self.entries.push(AttrRegexEntry {
            attribute,
            pattern,
            compiled,
        });
    }

    /// 按 (属性名, 正则原文) 取已编译正则。
    fn find(&self, attribute: &str, pattern: &str) -> Option<&AttrRegexEntry> {
        self.entries
            .iter()
            .find(|entry| entry.attribute == attribute && entry.pattern == pattern)
    }
}

/// 编译一个选择器。
///
/// 流程固定：**Jsoup 方言改写 → CSS 语法解析 → 正则属性编译**。
pub fn compile(selector: &str) -> Result<ParsedSelector, RuleError> {
    let normalized = rewrite_jsoup_dialect(selector);

    // parser 借用了 normalized 的字符串切片，故解析必须在一个内层作用域里完成，
    // 免得它的析构把 normalized 的移动挡在后面。
    let (list, regex_attrs) = {
        let mut parser = CssParser::new(&normalized);
        let selector_parser = JsoupSelectorParser {
            regex_attrs: collect_regex_attrs(selector),
        };
        let list = SelectorList::parse(&selector_parser, &mut parser, ParseRelative::No).map_err(
            |error| {
                RuleError::syntax(format!(
                    "选择器 `{selector}` 解析失败（规范化后为 `{normalized}`）：{error:?}"
                ))
            },
        )?;
        (list, selector_parser.regex_attrs)
    };

    let mut regex_cache = AttrRegexCache::default();
    for (attribute, pattern) in &regex_attrs {
        regex_cache.insert(attribute.to_ascii_lowercase(), pattern.clone());
    }

    Ok(ParsedSelector {
        normalized,
        regex_attrs,
        regex_cache,
        list,
    })
}

/// 取出所有 `[attr~=regex]` 的 (属性名, 正则原文)。
///
/// 与改写共用同一套词法扫描逻辑，保证两者看到的是同一份数据。
fn collect_regex_attrs(selector: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = selector.chars().collect();
    let mut found = Vec::new();
    let mut index = 0usize;

    while index < chars.len() {
        let current = chars[index];
        if current == '"' || current == '\'' {
            index = take_quoted(&chars, index).1;
            continue;
        }
        if current == '[' {
            match take_attribute_selector(&chars, index) {
                Some((inner, next)) => {
                    let trimmed = inner.trim();
                    if let Some(position) = find_unescaped_operator(trimmed, "~=") {
                        let name = trimmed[..position].trim();
                        let value = strip_outer_quotes(trimmed[position + 2..].trim());
                        found.push((name.to_string(), value.to_string()));
                    }
                    index = next;
                    continue;
                }
                None => {
                    index += 1;
                    continue;
                }
            }
        }
        index += 1;
    }

    found
}

impl ParsedSelector {
    /// 规范化后的选择器文本。
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// 其中 `[attr~=regex]` 方言的 (属性名, 正则原文) 清单。
    pub fn regex_attrs(&self) -> &[(String, String)] {
        &self.regex_attrs
    }
}

// ---------------------------------------------------------------------------
// Element 实现
// ---------------------------------------------------------------------------

/// 承载匹配上下文的文档所有权。
///
/// `selectors::Element` 的 `opaque()` 需要**稳定的节点身份指针**。arena 的
/// `Rc<Node>` 分配地址在节点生命周期内不变（arena 只增不删），故直接取
/// `&*node` 作为身份。`NodeRef` 单独存在时持有 `Document`（即 `Rc<DocumentInner>`），
/// 保证节点的 `Rc` 计数在其使用期间非零。
///
/// 这里用一个裸的 `NodeRef` 而非包装：`Element` 需要 `Clone`，
/// `NodeRef` 的克隆成本是两次 `Rc` 自增。
impl Element for NodeRef {
    type Impl = JsoupSelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // 身份指针取自 arena 中 `Node` 的地址。arena 只增不删，
        // 故同一节点的地址在整个文档生命周期内稳定且唯一。
        let node = self.document().node_at(self.id());
        OpaqueElement::new(&*node)
    }

    fn parent_element(&self) -> Option<Self> {
        NodeRef::parent_element(self)
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        NodeRef::prev_sibling_element(self)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        NodeRef::next_sibling_element(self)
    }

    fn first_element_child(&self) -> Option<Self> {
        NodeRef::first_element_child(self)
    }

    fn is_html_element_in_html_document(&self) -> bool {
        // 本引擎只解析 HTML（XML 路径在 S1-3），故非 HTML 命名空间的元素
        // 只可能是 SVG/MathML 外部内容。
        self.namespace() == HTML_NAMESPACE
    }

    fn has_local_name(&self, local_name: &str) -> bool {
        // HTML 元素名由 HTML5 树构造保证已小写；属性选择器与类型选择器都
        // 传小写形式（`select_name` 在 HTML 文档里取 `lower_name`）。
        self.node_name() == local_name
    }

    fn has_namespace(&self, namespace: &str) -> bool {
        self.namespace() == namespace
    }

    fn is_same_type(&self, other: &Self) -> bool {
        self.node_name() == other.node_name() && self.namespace() == other.namespace()
    }

    fn attr_matches(
        &self,
        _namespace: &NamespaceConstraint<&CssName>,
        local_name: &CssName,
        operation: &AttrSelectorOperation<&CssName>,
    ) -> bool {
        let name = local_name.as_str();

        // 属性名匹配：Jsoup 的 `Selector` 文档说"selectors are case-insensitive
        // (including against elements, attributes, and attribute values)"，
        // 故属性名按小写归一后比较。HTML5 树构造已把属性名小写化，
        // 但 XML/SVG 路径可能保留原样。
        let actual = match self.attr_case_insensitive(name) {
            Some(value) => value,
            None => return false,
        };

        match operation {
            AttrSelectorOperation::Exists => true,
            AttrSelectorOperation::WithValue {
                operator,
                case_sensitivity,
                value,
            } => {
                // 方言 1（research R2 点名的重载）：`~=` 在 CSS 标准里是空白分词，
                // 在 Jsoup 里是**正则**。本实现已在 [`rewrite_jsoup_dialect`] 把
                // `[attr~=X]` 改写成 `:jsoupAttrMatches("attr","X")`，故匹配走的是
                // [`Element::match_non_ts_pseudo_class`] 而非这里。
                //
                // 万一有 `~=` 漏到这里（不该发生），按 CSS 标准的分词语义处理并在
                // debug 构建里显式暴露：静默按标准语义匹配会让"正则方言失效"变成
                // 难以发现的兼容性缺口。
                debug_assert!(
                    *operator != selectors::attr::AttrSelectorOperator::Includes,
                    "`~=` 应已被改写为 :jsoupAttrMatches，不该到达此处"
                );
                let _ = case_sensitivity;
                // Jsoup 的属性值匹配是大小写不敏感的（见上引文档），
                // 故忽略 CSS 的大小写敏感度标志，一律 ASCII 不敏感。
                operator.eval_str(
                    &actual,
                    value.as_str(),
                    CaseSensitivity::AsciiCaseInsensitive,
                )
            }
        }
    }

    fn match_non_ts_pseudo_class(
        &self,
        pseudo: &JsoupPseudoClass,
        context: &mut MatchingContext<JsoupSelectorImpl>,
    ) -> bool {
        match pseudo {
            // 公开文档：`:contains(text)` —— "The search is case insensitive.
            // The text may appear in the found element, or any of its descendants.
            // The text is whitespace normalized."
            JsoupPseudoClass::Contains(text) => contains_ascii_case_insensitive(&self.text(), text),
            // 公开文档：`:containsOwn(text)` —— "The text must appear in the found
            // element, not any of its descendants."
            JsoupPseudoClass::ContainsOwn(text) => {
                contains_ascii_case_insensitive(&self.own_text(), text)
            }
            // 公开文档：`:containsData(data)` —— data node 内容，后代包含
            JsoupPseudoClass::ContainsData(data) => {
                contains_ascii_case_insensitive(&self.data(), data)
            }
            // 公开文档：`:containsWholeText(text)` —— "The search is case sensitive,
            // and will match exactly against spaces and newlines found in the original input."
            JsoupPseudoClass::ContainsWholeText(text) => self.whole_text().contains(text.as_str()),
            JsoupPseudoClass::ContainsWholeOwnText(text) => {
                self.whole_own_text().contains(text.as_str())
            }
            // 公开文档：`:matches(regex)` —— "elements containing whitespace normalized
            // text that matches the specified regular expression."
            JsoupPseudoClass::Matches(source) => matches_regex_source(&self.text(), source),
            // `:matchesOwn` 只看**自身**文本（同 `:matches` 的归一化口径）
            JsoupPseudoClass::MatchesOwn(source) => matches_regex_source(&self.own_text(), source),
            JsoupPseudoClass::MatchesWholeText(source) => {
                matches_regex_source(&self.whole_text(), source)
            }
            JsoupPseudoClass::MatchesWholeOwnText(source) => {
                matches_regex_source(&self.whole_own_text(), source)
            }
            // 公开文档：`:eq(n)`/`:lt(n)`/`:gt(n)` —— "elements whose **sibling index**
            // is equal to / less than / greater than n"。
            // 注意是兄弟索引而不是结果集下标（见 dom::element_sibling_index 的说明）。
            JsoupPseudoClass::Eq(index) => sibling_index_matches(self, *index, |a, b| a == b),
            JsoupPseudoClass::Lt(index) => sibling_index_matches(self, *index, |a, b| a < b),
            JsoupPseudoClass::Gt(index) => sibling_index_matches(self, *index, |a, b| a > b),
            JsoupPseudoClass::Lang(language) => {
                // `:lang()` 按 CSS 规则向上找最近的 lang 属性。
                //
                // ## 匹配规则
                //
                // CSS Selectors 规范对 `:lang(l)` 的定义是"元素的 lang 等于 l，
                // **或以 l 加 `-` 起头**"（`en` 匹配 `en`、`en-US`、`en-GB`）。
                // 大小写不敏感（BCP 47 的 language tag 大小写不敏感）。
                //
                // 顺带去掉一处原来的 `format!` 分配：旧写法在每次匹配时构造
                // `"en-"` 字符串，而 `:lang` 会在遍历中反复求值。
                let _ = context;
                element_language(self).is_some_and(|actual| {
                    let actual_lower = actual.to_ascii_lowercase();
                    let wanted = language.to_ascii_lowercase();
                    actual_lower == wanted
                        || actual_lower
                            .strip_prefix(&wanted)
                            .is_some_and(|rest| rest.starts_with('-'))
                })
            }
            // 方言 1 的匹配落点：从 extra_data 取**已编译**的正则（见 AttrRegexCache）。
            JsoupPseudoClass::AttrMatches { attribute, pattern } => {
                let value = match self.attr_case_insensitive(attribute) {
                    Some(value) => value,
                    None => return false,
                };
                match context
                    .extra_data
                    .and_then(|cache| cache.find(attribute, pattern))
                {
                    Some(entry) => match &entry.compiled {
                        Ok(compiled) => {
                            // 回溯/长度上界触发时按"不匹配"处理，而不是让整条
                            // 选择器失败——理由同 matches_regex_source。
                            compiled
                                .is_match(&value, RegexLimits::DEFAULT)
                                .unwrap_or(false)
                        }
                        // 正则编译失败：退化为字面包含而不是整条选择器报错。
                        // 选择器编译阶段已能检出非法正则（`ParsedSelector::regex_cache`
                        // 里记着 Err），走到这里说明是匹配期的补救路径。
                        Err(_) => value.contains(pattern.as_str()),
                    },
                    // 缓存缺失同样是"不该发生"，兜底字面包含。
                    None => value.contains(pattern.as_str()),
                }
            }
            JsoupPseudoClass::AttrNamePrefix(prefix) => {
                self.attributes().iter().any(|(name, _)| {
                    name.to_ascii_lowercase()
                        .starts_with(&prefix.to_ascii_lowercase())
                })
            }
            JsoupPseudoClass::AnyAttribute => !self.attributes().is_empty(),
        }
    }

    fn match_pseudo_element(
        &self,
        _pseudo: &JsoupPseudoElement,
        _context: &mut MatchingContext<JsoupSelectorImpl>,
    ) -> bool {
        false
    }

    fn apply_selector_flags(&self, _flags: ElementSelectorFlags) {
        // 静态文档（解析后不再变更），无需失效标记。
    }

    fn is_link(&self) -> bool {
        // CSS 标准的 `:link` 定义：有 href 的 a/area/link 元素。
        matches!(self.node_name().as_str(), "a" | "area" | "link") && self.attr("href").is_some()
    }

    fn is_html_slot_element(&self) -> bool {
        false
    }

    fn has_id(&self, id: &CssName, case_sensitivity: CaseSensitivity) -> bool {
        self.attr_case_insensitive("id")
            .is_some_and(|actual| match case_sensitivity {
                CaseSensitivity::CaseSensitive => actual == id.as_str(),
                CaseSensitivity::AsciiCaseInsensitive => actual.eq_ignore_ascii_case(id.as_str()),
            })
    }

    fn has_class(&self, name: &CssName, _case_sensitivity: CaseSensitivity) -> bool {
        // Jsoup 公开文档 `Element#hasClass`："Tests if this element has a class.
        // Case-insensitive." 故**忽略** CSS 传入的大小写敏感度，恒不敏感。
        self.attr_case_insensitive("class").is_some_and(|classes| {
            classes
                .split_whitespace()
                .any(|class| class.eq_ignore_ascii_case(name.as_str()))
        })
    }

    fn has_custom_state(&self, _name: &CssName) -> bool {
        false
    }

    fn imported_part(&self, _name: &CssName) -> Option<CssName> {
        None
    }

    fn is_part(&self, _name: &CssName) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        // 委托给 DOM 层的 Jsoup 语义实现（与 CSS 标准的 `:empty` 不同，
        // 见 dom::NodeRef::is_empty_element 的文档）。
        NodeRef::is_empty_element(self)
    }

    fn is_root(&self) -> bool {
        // CSS 的 `:root` 是"文档的根元素"。Jsoup 公开文档：":root — The element
        // that is the root of the document. In HTML, this is the `html` element"。
        // 判据用"父节点是文档节点"，而不是"没有父元素"——后者会把
        // `<template>` 的片段根也算进来。
        self.parent()
            .is_some_and(|parent| parent.kind() == NodeKind::Document)
    }

    fn add_element_unique_hashes(&self, _filter: &mut BloomFilter) -> bool {
        // 不用 Bloom 过滤器快速排除（书源的 DOM 规模远小于样式引擎面对的文档），
        // 故不贡献散列。
        false
    }
}

impl NodeRef {
    /// 按**大小写不敏感**的属性名取属性值（Jsoup 的选择器大小写约定）。
    fn attr_case_insensitive(&self, local_name: &str) -> Option<String> {
        self.attributes()
            .into_iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(local_name))
            .map(|(_, value)| value)
    }

    /// 该元素及其全部后代的文本，**保留原始空白**（Jsoup `wholeText()`）。
    fn whole_own_text(&self) -> String {
        let mut buffer = String::new();
        for child in self.children() {
            if matches!(
                child.kind(),
                NodeKind::Text | NodeKind::Data | NodeKind::CData
            ) {
                buffer.push_str(&child.node_value());
            }
        }
        buffer
    }
}

/// 兄弟索引判定（`:eq` / `:lt` / `:gt` 共用）。
fn sibling_index_matches(
    element: &NodeRef,
    target: i32,
    compare: impl Fn(i64, i64) -> bool,
) -> bool {
    let index = match element.element_sibling_index() {
        Some(index) => index as i64,
        None => return false,
    };
    compare(index, i64::from(target))
}

/// 大小写不敏感的包含判断（只对 ASCII 折叠；中文等不受影响）。
fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let haystack_lower = haystack.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    haystack_lower.contains(&needle_lower)
}

/// 用本 crate 的正则引擎做匹配（含 fancy 路由与护栏），失败视为不匹配。
fn matches_regex_source(text: &str, source: &str) -> bool {
    match regex::compile(source, RegexLimits::DEFAULT) {
        Ok(compiled) => {
            // 长文本 + fancy 特性会触发回溯上界，此时按"不匹配"处理而非
            // 让整条选择器失败——理由同 attr_matches_regex。
            compiled
                .is_match(text, RegexLimits::DEFAULT)
                .unwrap_or(false)
        }
        Err(_) => text.contains(source),
    }
}

/// 元素的 `lang`（向上找最近的 `lang` 或 `xml:lang`）。
fn element_language(element: &NodeRef) -> Option<String> {
    let mut cursor = Some(element.clone());
    while let Some(current) = cursor {
        if let Some(language) = current.attr("lang").or_else(|| current.attr("xml:lang")) {
            return Some(language);
        }
        cursor = current.parent_element();
    }
    None
}

// ---------------------------------------------------------------------------
// 查询入口
// ---------------------------------------------------------------------------

/// 在一棵文档（或其子树）上按 CSS 选择器查询。
///
/// # 起点语义
///
/// 对齐 Jsoup 公开文档 `Element#select`：
///
/// > Find elements that match the CSS query, with this element as the starting
/// > context. **Matched elements may include this element, or any of its descendents.**
///
/// 即结果集**含起点自身**（这与 `querySelectorAll` 一致，且书源规则里
/// `element.select("div")` 期待把 element 本身也算进来的场合真实存在）。
pub fn select(root: &NodeRef, parsed: &ParsedSelector) -> Vec<NodeRef> {
    let mut matched = Vec::new();
    let document = root.document().clone();
    let quirks_mode = selector_quirks_mode(document.quirks_mode());

    // 遍历起点及其后代。`descendants_and_self` 用显式回溯，深文档不爆栈。
    for node in root.descendants_and_self() {
        if !node.is_element() {
            continue;
        }
        if matches_one(&node, parsed, quirks_mode) {
            matched.push(node);
        }
    }

    matched
}

/// 整个文档上查询（等价于 Jsoup 的 `document.select(selector)`）。
pub fn select_in_document(document: &Document, parsed: &ParsedSelector) -> Vec<NodeRef> {
    let quirks_mode = selector_quirks_mode(document.quirks_mode());
    let mut matched = Vec::new();
    for node in document.descendants() {
        if !node.is_element() {
            continue;
        }
        if matches_one(&node, parsed, quirks_mode) {
            matched.push(node);
        }
    }
    matched
}

/// 单个元素是否匹配。
pub fn is_match(element: &NodeRef, parsed: &ParsedSelector) -> bool {
    matches_one(
        element,
        parsed,
        selector_quirks_mode(element.document().quirks_mode()),
    )
}

fn matches_one(element: &NodeRef, parsed: &ParsedSelector, quirks: SelectorQuirksMode) -> bool {
    let mut caches = SelectorCaches::default();
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        None,
        &mut caches,
        quirks,
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    // 注入已编译的正则表（见 AttrRegexCache 的文档）
    context.extra_data = Some(&parsed.regex_cache);
    parsed
        .list
        .slice()
        .iter()
        .any(|selector| matches_selector(selector, 0, None, element, &mut context))
}

/// DOM 层的 quirks 模式 → 选择器层的 quirks 模式。
///
/// 影响：quirks 模式下 `.class` 与 `#id` 的大小写敏感度变为不敏感
/// （`selectors::context::QuirksMode::classes_and_ids_case_sensitivity`）。
/// 本实现对 `has_class`/`has_id` 另有 Jsoup 的大小写约定覆盖，
/// 但把模式如实传递仍然是必要的——它还会影响其它标准行为。
fn selector_quirks_mode(mode: QuirksMode) -> SelectorQuirksMode {
    match mode {
        QuirksMode::Quirks => SelectorQuirksMode::Quirks,
        QuirksMode::LimitedQuirks => SelectorQuirksMode::LimitedQuirks,
        QuirksMode::NoQuirks => SelectorQuirksMode::NoQuirks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷：编译并断言成功。
    fn parsed(selector: &str) -> ParsedSelector {
        compile(selector).unwrap_or_else(|error| panic!("选择器 `{selector}` 应能编译：{error}"))
    }

    /// 便捷：在 HTML 上按选择器查询，返回元素名序列（含重复）。
    fn query(html: &str, selector: &str) -> Vec<String> {
        let document = Document::parse_html(html);
        select_in_document(&document, &parsed(selector))
            .into_iter()
            .map(|node| node.node_name())
            .collect()
    }

    /// 便捷：在 HTML 上查询，返回某个属性的值序列。
    fn query_attr(html: &str, selector: &str, attribute: &str) -> Vec<String> {
        let document = Document::parse_html(html);
        select_in_document(&document, &parsed(selector))
            .into_iter()
            .map(|node| node.attr(attribute).unwrap_or_default())
            .collect()
    }

    // ---------- 方言改写（纯函数，易于逐条验证） ----------

    #[test]
    fn 正则属性被加引号且原文可回收() {
        // `~=` 方言改写成功能伪类（见 AttrRegexCache 的文档）：属性名与模式都变成
        // CSS 字符串，故任意字符都能安全通过语法解析。
        //
        // 注意 CSS 字符串的转义规则：正则里的一个 `\` 在 CSS 字符串里必须写成两个，
        // 故规范化输出里 `\.` 呈现为 `\\.`（两个反斜杠字符 + 点）。
        assert_eq!(
            rewrite_jsoup_dialect(r"img[src~=(?i)\.(png|jpe?g)]"),
            r#"img:jsoupAttrMatches("src","(?i)\\.(png|jpe?g)")"#
        );
        // 回收的正则是**原文**（一个反斜杠），不是 CSS 字符串形态
        let attrs = collect_regex_attrs(r"img[src~=(?i)\.(png|jpe?g)]");
        assert_eq!(
            attrs,
            vec![("src".to_string(), r"(?i)\.(png|jpe?g)".to_string())]
        );
    }

    #[test]
    fn 属性名前缀方言被改写为伪类() {
        assert_eq!(
            rewrite_jsoup_dialect("[^data-]"),
            r#":jsoupAttrNamePrefix("data-")"#
        );
        assert_eq!(
            rewrite_jsoup_dialect("div[^data-]"),
            r#"div:jsoupAttrNamePrefix("data-")"#
        );
    }

    #[test]
    fn 任意属性方言被改写为伪类() {
        assert_eq!(rewrite_jsoup_dialect("p[*]"), "p:jsoupAnyAttribute");
        assert_eq!(
            rewrite_jsoup_dialect("p:not([*])"),
            "p:not(:jsoupAnyAttribute)"
        );
    }

    #[test]
    fn 引号内的方括号不打断属性扫描() {
        // 朴素 `find(']')` 会在这里截断错误位置
        assert_eq!(
            rewrite_jsoup_dialect(r#"[title="a]b"]"#),
            r#"[title="a]b"]"#
        );
        assert_eq!(
            rewrite_jsoup_dialect(r#"[title="a]b"][href~=x]"#),
            r#"[title="a]b"]:jsoupAttrMatches("href","x")"#
        );
    }

    #[test]
    fn 标准选择器不被改写() {
        for selector in [
            "div",
            ".class",
            "#id",
            "div > p",
            "a[href]",
            "li:nth-child(2n+1)",
            ":is(h1, h2)",
            "div:has(> a)",
            "div:not(.logo)",
            "p:contains(jsoup)",
        ] {
            assert_eq!(
                rewrite_jsoup_dialect(selector),
                selector,
                "标准选择器 `{selector}` 不该被改写"
            );
        }
    }

    // ---------- 标准选择器 ----------

    #[test]
    fn 类型与通用选择器() {
        assert_eq!(query("<div><p>x</p></div>", "p"), vec!["p"]);
        // 树构造器会补出 html/head/body，故 5 个元素：html/head/body/div/p
        assert_eq!(query("<div><p>x</p></div>", "*").len(), 5);
    }

    #[test]
    fn 类选择器() {
        let html = r#"<div class="header round first"></div><span class="other"></span>"#;
        assert_eq!(query(html, ".header"), vec!["div"]);
        assert_eq!(query(html, ".round"), vec!["div"]);
        assert_eq!(query(html, ".missing"), Vec::<String>::new());
    }

    #[test]
    fn 类匹配是大小写不敏感的() {
        // Jsoup 公开文档 Element#hasClass："Case-insensitive."
        let html = r#"<div class="Header"></div>"#;
        assert_eq!(query(html, ".header"), vec!["div"]);
        assert_eq!(query(html, ".HEADER"), vec!["div"]);
    }

    #[test]
    fn id选择器() {
        let html = r#"<div id="wrap"><p id="inner"></p></div>"#;
        assert_eq!(query(html, "#wrap"), vec!["div"]);
        assert_eq!(query(html, "#inner"), vec!["p"]);
        assert_eq!(query(html, "div#wrap"), vec!["div"]);
        assert_eq!(query(html, "p#wrap"), Vec::<String>::new());
    }

    #[test]
    fn 属性存在与值选择器() {
        let html = r#"<a href="/x">1</a><a>2</a><a href="/y">3</a>"#;
        assert_eq!(query(html, "a[href]").len(), 2);
        assert_eq!(query(html, r#"a[href="/x"]"#).len(), 1);
        assert_eq!(query(html, "a[href^=/]").len(), 2);
        assert_eq!(query(html, "a[href$=y]").len(), 1);
        assert_eq!(query(html, "a[href*=/]").len(), 2);
    }

    #[test]
    fn 属性值匹配是大小写不敏感的() {
        let html = r#"<a rel="Nofollow">x</a>"#;
        // Jsoup 文档明确属性值也大小写不敏感
        assert_eq!(query(html, "a[rel=nofollow]").len(), 1);
    }

    #[test]
    fn 属性名匹配是大小写不敏感的() {
        let html = r#"<a HREF="/x">x</a>"#;
        assert_eq!(query(html, "a[href]").len(), 1);
    }

    #[test]
    fn 组合子() {
        let html = "<div><p>1</p><section><p>2</p></section></div>\
                    <div><section><p id=\"deep\">3</p><span>s</span><p id=\"last\">4</p></section></div>";
        // 后代
        assert_eq!(query(html, "div p").len(), 4);
        // 直接子
        assert_eq!(query(html, "div > p").len(), 1);
        assert_eq!(query(html, "section > p").len(), 3);
        // 紧随兄弟
        assert_eq!(query_attr(html, "span + p", "id"), vec!["last"]);
        // 后续兄弟
        assert_eq!(query_attr(html, "span ~ p", "id"), vec!["last"]);
    }

    #[test]
    fn 选择器列表() {
        let html = "<h1>a</h1><h2>b</h2><h3>c</h3>";
        assert_eq!(query(html, "h1, h3"), vec!["h1", "h3"]);
    }

    #[test]
    fn nth系列伪类() {
        let html = "<ul><li>1</li><li>2</li><li>3</li><li>4</li><li>5</li></ul>";
        assert_eq!(query(html, "li:nth-child(1)").len(), 1);
        assert_eq!(query(html, "li:nth-child(odd)").len(), 3);
        assert_eq!(query(html, "li:nth-child(even)").len(), 2);
        assert_eq!(query(html, "li:nth-child(2n+1)").len(), 3);
        assert_eq!(query(html, "li:first-child").len(), 1);
        assert_eq!(query(html, "li:last-child").len(), 1);
        assert_eq!(query(html, "li:nth-last-child(1)").len(), 1);
        assert_eq!(query(html, "li:nth-of-type(2)").len(), 1);
        assert_eq!(query(html, "li:only-child").len(), 0);
    }

    #[test]
    fn is与where与not() {
        let html = "<h1>a</h1><h2>b</h2><p>c</p>";
        assert_eq!(query(html, ":is(h1, h2)").len(), 2);
        assert_eq!(query(html, ":where(h1, h2)").len(), 2);
        assert_eq!(query(html, "p:not(.x)"), vec!["p"]);
        let html = r#"<div class="logo"></div><div class="other"></div>"#;
        assert_eq!(query(html, "div:not(.logo)"), vec!["div"]);
    }

    #[test]
    fn has伪类含相对选择器() {
        let html = "<div><p>x</p></div><section><a>y</a></section>";
        assert_eq!(query(html, "div:has(p)"), vec!["div"]);
        assert_eq!(query(html, "div:has(a)"), Vec::<String>::new());
        // 公开文档给的相对选择器写法：`div:has(> a)`
        let html = "<div><a>direct</a></div><div><span><a>nested</a></span></div>";
        assert_eq!(query(html, "div:has(> a)").len(), 1);
        // 公开文档：`section:has(h1, h2)`
        let html = "<section><h1>x</h1></section><section><p>y</p></section>";
        assert_eq!(query(html, "section:has(h1, h2)").len(), 1);
    }

    #[test]
    fn root与empty() {
        let html = "<html><head></head><body>x</body></html>";
        assert_eq!(query(html, ":root"), vec!["html"]);
        // 空元素按 Jsoup 语义（允许空白文本与注释）
        let html = "<div><p></p><p>   </p><p>x</p></div>";
        assert_eq!(query(html, "p").len(), 3);
        assert_eq!(query(html, "p:empty").len(), 2);
    }

    #[test]
    fn 大小写不敏感的类型选择器() {
        let html = "<DIV>x</DIV>";
        assert_eq!(query(html, "div"), vec!["div"]);
        assert_eq!(query(html, "DIV"), vec!["div"]);
    }

    // ---------- Jsoup 专有伪类 ----------

    #[test]
    fn contains是空白归一化且大小写不敏感的() {
        // 公开文档：p:contains(jsoup) finds p elements containing the text "jsoup"
        let html = "<p>Hello   JSOUP\nWorld</p><p>other</p>";
        assert_eq!(query(html, "p:contains(jsoup)").len(), 1);
        // 归一化后的文本是 "Hello JSOUP World"，故含空白的查询要按归一化形态书写
        assert_eq!(query(html, "p:contains(Hello JSOUP World)").len(), 1);
        // 未归一化的原文形态（连续空格）不应命中——这条断言证明归一化确实生效
        assert_eq!(query(html, "p:contains(Hello   JSOUP)").len(), 0);
    }

    #[test]
    fn containsown只看自身文本() {
        let html = "<p>own<span>nested</span></p>";
        assert_eq!(query(html, "p:containsOwn(own)").len(), 1);
        assert_eq!(query(html, "p:containsOwn(nested)").len(), 0);
        assert_eq!(query(html, "span:containsOwn(nested)").len(), 1);
    }

    #[test]
    fn containsdata看datanode内容() {
        // 公开文档：script:contains(jsoup) finds script elements containing the data
        // （文档里 `:contains` 与 `:containsData` 对 script 的示例都用 data 讨论，
        // 但明确区分二者；本实现按"data node 内容"实现 :containsData）
        let html = "<script>var target = 1;</script><p>target</p>";
        assert_eq!(query(html, "script:containsData(target)").len(), 1);
        assert_eq!(query(html, "script:contains(target)").len(), 0);
        assert_eq!(query(html, "p:containsData(target)").len(), 0);
    }

    #[test]
    fn matches使用正则() {
        // 公开文档：td:matches(\d+) finds table cells containing digits
        //
        // 注意用完整 table 包裹：裸 `<td>` 会被 HTML5 树构造器整段丢弃
        // （td 只能在 tr 里），那会让本测试测到"没有元素"而不是"匹配逻辑"。
        let html = "<table><tr><td>abc123</td><td>xyz</td></tr></table>";
        assert_eq!(query(html, r"td:matches(\d+)").len(), 1);
        assert_eq!(query(html, r"td:matches([a-z])").len(), 2);
    }

    #[test]
    fn matches支持内联大小写标志() {
        // 公开文档：div:matches((?i)login)
        let html = "<div>Please LOGIN now</div>";
        assert_eq!(query(html, r"div:matches((?i)login)").len(), 1);
        assert_eq!(query(html, r"div:matches(login)").len(), 0);
    }

    #[test]
    fn containswholetext区分空白与大小写() {
        let html = "<p>Line1\nLine2</p>";
        assert_eq!(
            query(
                html,
                r"p:containsWholeText(Line1
Line2)"
            )
            .len(),
            1
        );
        // 归一化后的 :contains 也命中（空白折叠为单空格）
        assert_eq!(query(html, "p:contains(Line1 Line2)").len(), 1);
        // 大小写敏感
        assert_eq!(query(html, "p:containsWholeText(line1)").len(), 0);
    }

    #[test]
    fn 索引伪类以兄弟索引为准() {
        // 公开文档：`td:eq(0)` finds the first cell of **each row** —— 兄弟索引
        // 而非结果集下标。这是极易搞错的一点。
        let html = "<table><tr><td>a1</td><td>a2</td></tr><tr><td>b1</td><td>b2</td></tr></table>";
        let document = Document::parse_html(html);
        let cells: Vec<String> = select_in_document(&document, &parsed("td:eq(0)"))
            .into_iter()
            .map(|node| node.text())
            .collect();
        assert_eq!(cells, vec!["a1", "b1"], "每一行的第一个单元格都要命中");

        assert_eq!(query(html, "td:lt(1)").len(), 2);
        assert_eq!(query(html, "td:gt(0)").len(), 2);
    }

    #[test]
    fn 索引伪类接受负数() {
        let html = "<ul><li>1</li><li>2</li><li>3</li></ul>";
        // 没有元素的兄弟索引是 -1，故 `:eq(-1)` 应为空——断言它能解析且不崩溃
        assert_eq!(query(html, "li:eq(-1)").len(), 0);
        assert_eq!(query(html, "li:gt(-1)").len(), 3, "所有索引都大于 -1");
        assert_eq!(query(html, "li:lt(99)").len(), 3);
    }

    #[test]
    fn lang伪类() {
        let html = r#"<div lang="en"><p>x</p></div>"#;
        assert_eq!(query(html, "p:lang(en)").len(), 1);
        assert_eq!(query(html, "p:lang(zh)").len(), 0);

        // CSS Selectors 规范：`:lang(en)` 也要匹配 `en-US` / `en-GB`。
        let html =
            r#"<div lang="en-US"><p id="a">x</p></div><div lang="english"><p id="b">y</p></div>"#;
        let document = Document::parse_html(html);
        let matched: Vec<String> = select_in_document(&document, &parsed("p:lang(en)"))
            .into_iter()
            .map(|node| node.attr("id").unwrap_or_default())
            .collect();
        assert_eq!(
            matched,
            vec!["a"],
            "`en` 应匹配 `en-US`，但不该匹配 `english`（必须有 `-` 分隔）"
        );
    }

    #[test]
    fn 属性名前缀与任意属性方言() {
        // 公开文档 `[^attrPrefix]`："Use to find elements with HTML5 datasets"
        let html = r#"<div data-a="1" data-b="2"></div><div class="x"></div><p></p>"#;
        assert_eq!(query(html, "div[^data-]").len(), 1);
        assert_eq!(query(html, "[^data-]").len(), 1);

        // 公开文档 `[*]`："p[*] finds p elements that have at least one attribute;
        // p:not([*]) finds those with no attributes."
        let html = r#"<p id="a"></p><p></p>"#;
        assert_eq!(query(html, "p[*]").len(), 1);
        assert_eq!(query(html, "p:not([*])").len(), 1);
    }

    #[test]
    fn 正则属性方言真正按正则匹配() {
        // 这是 research R2 点名的重载：CSS 标准里 `~=` 是空白分词，
        // Jsoup 里是正则。公开文档的示例：img[src~=(?i)\.(png|jpe?g)]
        let html = r#"<img src="/a/photo.PNG"><img src="/a/photo.gif"><img src="/a/photo.jpeg">"#;
        assert_eq!(query(html, r"img[src~=(?i)\.(png|jpe?g)]").len(), 2);
        // 标准分词语义下达不到这个效果（`\.(png|jpe?g)` 不是一个空白分词）
        assert_eq!(query(html, r"img[src~=photo]").len(), 3);
    }

    // ---------- 错误与边界 ----------

    #[test]
    fn 非法选择器返回结构化语法错误() {
        let error = compile("div >").expect_err("不完整的选择器应报错");
        assert!(matches!(error, RuleError::Syntax { .. }));
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Rule);
    }

    #[test]
    fn 未支持的伪类明确报错() {
        // 伪元素不属于 Jsoup 的规则语法，应报错而不是静默不匹配
        assert!(compile("p::before").is_err());
        assert!(compile("p:unknown-pseudo").is_err());
    }

    #[test]
    fn 空选择器报错() {
        assert!(compile("").is_err());
        assert!(compile("   ").is_err());
    }

    #[test]
    fn select含起点自身() {
        // Jsoup 公开文档 Element#select："Matched elements may include this element"
        let document = Document::parse_html("<div class=\"wrap\"><p>x</p></div>");
        let wrap = select_in_document(&document, &parsed("div.wrap"))
            .into_iter()
            .next()
            .expect("应找到 wrap");
        let inner = select(&wrap, &parsed("p"));
        assert_eq!(inner.len(), 1);
        // 对 div 自身做同类查询应命中自身
        let itself = select(&wrap, &parsed("div"));
        assert_eq!(itself.len(), 1, "结果集应含起点自身");
        assert_eq!(itself[0].attr("class").as_deref(), Some("wrap"));
    }

    #[test]
    fn 选择器可复用在同一文档的多次查询() {
        let document = Document::parse_html("<p>a</p><p>b</p>");
        let selector = parsed("p");
        assert_eq!(select_in_document(&document, &selector).len(), 2);
        assert_eq!(select_in_document(&document, &selector).len(), 2);
        assert_eq!(selector.normalized(), "p");
    }

    #[test]
    fn is_match用于单元素判定() {
        let document = Document::parse_html(r#"<div class="a"><p>x</p></div>"#);
        let div = select_in_document(&document, &parsed("div"))
            .into_iter()
            .next()
            .expect("应找到 div");
        assert!(is_match(&div, &parsed("div.a")));
        assert!(!is_match(&div, &parsed("p")));
    }

    #[test]
    fn 规范化文本可被诊断查看() {
        let selector = parsed(r"img[src~=\.png]");
        // 注意文言：CSS 字符串里写一个反斜杠要写成 `\\`（两个字符），
        // 故规范化结果里出现的是**两个**反斜杠字符。
        assert_eq!(
            selector.normalized(),
            r#"img:jsoupAttrMatches("src","\\.png")"#
        );
        // 回收的原文是正则本身（一个反斜杠）
        assert_eq!(
            selector.regex_attrs(),
            &[("src".to_string(), r"\.png".to_string())]
        );
    }

    #[test]
    fn 正则属性含特殊字符也能解析() {
        // 含 `\`、`|`、`(`、`?`、`[` 的模式若不预处理，CSS 语法解析器会直接拒绝
        for pattern in [
            r"\.(png|jpe?g)",
            r"^\d{3}$",
            r"a[bc]+d",
            r"(?i)hello",
            r"foo\\bar",
        ] {
            let selector = format!(r#"img[src~={pattern}]"#);
            assert!(
                compile(&selector).is_ok(),
                "模式 `{pattern}` 应能被解析（选择器 `{selector}`）"
            );
        }
    }
}
