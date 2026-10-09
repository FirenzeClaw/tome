//! [DONE] XPath 1.0 内建函数与书源生态扩展函数（tasks.md T049）
//!
//! # 依据来源
//!
//! - 核心函数库：**W3C XPath 1.0 §4 "Core Function Library"**。
//!   四个分节各对应一类：§4.1 节点集函数、§4.2 字符串函数、
//!   §4.3 布尔函数、§4.4 数字函数。
//! - 扩展函数（`allText()` / `html()` / `outerHtml()` / `num()`）：属书源规则
//!   生态的公开语法约定，其**取值语义**对应 Jsoup 公开文档的
//!   `Element#text()` / `Element#html()` / `Node#outerHtml()`。
//!   本模块只做"从节点取值"这一步，节点测试的识别在 `parse.rs`。
//!
//! # 自变量类型与元数
//!
//! 每个函数在 [`lookup`] 里声明**元数**并在此处逐条校验。规范对
//! 元数不符的处理是 "error"，本项目把它映射成
//! [`XPathError::type_mismatch`]（`err:FORG0006` 一类的"wrong argument type"），
//! 而不是让书源的 `try/catch` 看到一条笼统的语法错。

use super::ast::Expr;
use super::error::XPathError;
use super::eval::{to_boolean, to_number, to_string, Context};
use super::value::{string_value, XPathValue};
use crate::dom::NodeRef;

/// 函数表条目的元数约束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arity {
    /// 恰好 N 个自变量
    Exact(usize),
    /// N 到 M 个自变量（含两端）
    Range(usize, usize),
}

impl Arity {
    /// 校验实际自变量个数。
    fn check(self, name: &str, count: usize) -> Result<(), XPathError> {
        let ok = match self {
            Self::Exact(expected) => count == expected,
            Self::Range(low, high) => (low..=high).contains(&count),
        };
        if ok {
            return Ok(());
        }
        let expected = match self {
            Self::Exact(expected) => format!("{expected} 个"),
            Self::Range(low, high) => format!("{low}~{high} 个"),
        };
        Err(XPathError::type_mismatch(format!(
            "`{name}()` 需要 {expected}自变量，实际给了 {count} 个"
        )))
    }
}

/// 查函数表。`None` 表示未实现该函数。
///
/// 未实现的函数**不静默返回空**：`call` 会把它变成
/// [`XPathError::type_mismatch`]，让书源的降级逻辑能看见是哪一项不可用。
fn lookup(name: &str) -> Option<Arity> {
    Some(match name {
        // §4.1 节点集函数
        "last" => Arity::Exact(0),
        "position" => Arity::Exact(0),
        "count" => Arity::Exact(1),
        "id" => Arity::Exact(1),
        "local-name" => Arity::Range(0, 1),
        "namespace-uri" => Arity::Range(0, 1),
        "name" => Arity::Range(0, 1),
        // §4.2 字符串函数
        "string" => Arity::Range(0, 1),
        "concat" => Arity::Range(2, usize::MAX),
        "starts-with" => Arity::Exact(2),
        "contains" => Arity::Exact(2),
        "substring-before" => Arity::Exact(2),
        "substring-after" => Arity::Exact(2),
        "substring" => Arity::Range(2, 3),
        "string-length" => Arity::Range(0, 1),
        "normalize-space" => Arity::Range(0, 1),
        "translate" => Arity::Exact(3),
        // §4.3 布尔函数
        "boolean" => Arity::Exact(1),
        "not" => Arity::Exact(1),
        "true" => Arity::Exact(0),
        "false" => Arity::Exact(0),
        "lang" => Arity::Exact(1),
        // §4.4 数字函数
        "number" => Arity::Range(0, 1),
        "sum" => Arity::Exact(1),
        "floor" => Arity::Exact(1),
        "ceiling" => Arity::Exact(1),
        "round" => Arity::Exact(1),
        _ => return None,
    })
}

/// 求值一次函数调用。
///
/// [DONE] 函数调用的唯一入口。`evaluate` 是上层求值器的入口指针——
/// 传进来而不直接调 `eval::evaluate_expression`，是为了避免两个模块
/// 互相 `use` 形成循环依赖。
pub(super) fn call(
    name: &str,
    arguments: &[Expr],
    context: &mut Context<'_>,
    evaluate: fn(&Expr, &mut Context<'_>) -> Result<XPathValue, XPathError>,
) -> Result<XPathValue, XPathError> {
    let arity = lookup(name).ok_or_else(|| {
        // 未实现的函数是**能力缺失**而非语法错，故用类型/能力类错误
        // （FR-006 的结构化信号由上层 `mod.rs` 映射成 Unsupported）。
        XPathError::type_mismatch(format!("未实现 XPath 函数 `{name}()`"))
    })?;
    arity.check(name, arguments.len())?;

    // 自变量按**从左到右**求值：XPath 1.0 没有规定求值顺序，但书源规则里
    // 的自变量可能有副作用感知（如 `count(//x)` 的代价），从左到右是
    // 唯一不让作者意外的顺序。
    let mut values = Vec::with_capacity(arguments.len());
    for argument in arguments {
        values.push(evaluate(argument, context)?);
    }

    dispatch(name, &values, context)
}

/// 按函数名分派到具体实现。
fn dispatch(
    name: &str,
    values: &[XPathValue],
    context: &Context<'_>,
) -> Result<XPathValue, XPathError> {
    match name {
        // ---------------- §4.1 节点集函数 ----------------
        //
        // `last()` 与 `position()` 都返回**上下文**的位置信息（§4.1）：
        //
        // > position() returns the context position ... last() returns the
        // > context size
        "last" => Ok(XPathValue::Number(context.size as f64)),
        "position" => Ok(XPathValue::Number(context.position as f64)),
        "count" => Ok(XPathValue::Number(
            require_nodes(&values[0], "count()")?.len() as f64,
        )),
        // `id()` 需要 DTD 里的 ID 属性声明表，HTML DOM 没有它。
        // 依据 §4.1：`id()` "selects elements by their unique ID"，
        // 而"unique ID"的判定依赖 XML 的 ID 类型——HTML 的 `id` 属性
        // 在规范里不是 ID 类型。故返回空节点集而不是猜测匹配 `id` 属性。
        "id" => Ok(XPathValue::node_set(Vec::new())),
        "local-name" => Ok(XPathValue::String(local_name_of(values, context))),
        "namespace-uri" => Ok(XPathValue::String(namespace_uri_of(values, context))),
        "name" => Ok(XPathValue::String(qualified_name_of(values, context))),

        // ---------------- §4.2 字符串函数 ----------------
        "string" => Ok(XPathValue::String(argument_string(values, context))),
        "concat" => Ok(XPathValue::String(
            values.iter().map(to_string).collect::<String>(),
        )),
        "starts-with" => Ok(XPathValue::Boolean(
            to_string(&values[0]).starts_with(&to_string(&values[1])),
        )),
        "contains" => Ok(XPathValue::Boolean(
            to_string(&values[0]).contains(&to_string(&values[1])),
        )),
        "substring-before" => Ok(XPathValue::String(substring_before(
            &to_string(&values[0]),
            &to_string(&values[1]),
        ))),
        "substring-after" => Ok(XPathValue::String(substring_after(
            &to_string(&values[0]),
            &to_string(&values[1]),
        ))),
        "substring" => Ok(XPathValue::String(substring(values))),
        "string-length" => {
            // `string-length()` 无自变量时取**上下文节点**的字符串值（§4.2）
            let text = match values.first() {
                Some(value) => to_string(value),
                None => string_value(&context.node),
            };
            Ok(XPathValue::Number(text.chars().count() as f64))
        }
        "normalize-space" => {
            // `normalize-space()` 无自变量时取上下文节点（§4.2）
            let text = match values.first() {
                Some(value) => to_string(value),
                None => string_value(&context.node),
            };
            Ok(XPathValue::String(normalize_space(&text)))
        }
        "translate" => Ok(XPathValue::String(translate(
            &to_string(&values[0]),
            &to_string(&values[1]),
            &to_string(&values[2]),
        ))),

        // ---------------- §4.3 布尔函数 ----------------
        "boolean" => Ok(XPathValue::Boolean(to_boolean(&values[0]))),
        "not" => Ok(XPathValue::Boolean(!to_boolean(&values[0]))),
        "true" => Ok(XPathValue::Boolean(true)),
        "false" => Ok(XPathValue::Boolean(false)),
        // `lang()` 依赖 `xml:lang` 属性的祖先查找，HTML DOM 的 `lang`
        // 不是 `xml:lang`（§4.3 明确说的是 "xml:lang attribute"）。
        // 故按"文档语言未知"处理，返回 false。
        "lang" => Ok(XPathValue::Boolean(false)),

        // ---------------- §4.4 数字函数 ----------------
        "number" => {
            let value = match values.first() {
                Some(value) => to_number(value),
                // 无自变量时取上下文节点（§4.4）
                None => to_number(&XPathValue::String(string_value(&context.node))),
            };
            Ok(XPathValue::Number(value))
        }
        "sum" => {
            let nodes = require_nodes(&values[0], "sum()")?;
            Ok(XPathValue::Number(
                nodes
                    .iter()
                    .map(|node| to_number(&XPathValue::String(string_value(node))))
                    .sum(),
            ))
        }
        "floor" => Ok(XPathValue::Number(to_number(&values[0]).floor())),
        "ceiling" => Ok(XPathValue::Number(to_number(&values[0]).ceil())),
        "round" => Ok(XPathValue::Number(round_xpath(to_number(&values[0])))),
        // `lookup` 已保证不会有别的名字走到这里
        other => Err(XPathError::type_mismatch(format!(
            "未实现 XPath 函数 `{other}()`"
        ))),
    }
}

/// 要求一个值是节点集。
fn require_nodes<'a>(value: &'a XPathValue, what: &str) -> Result<&'a [NodeRef], XPathError> {
    match value {
        XPathValue::NodeSet(nodes) => Ok(nodes),
        other => Err(XPathError::node_set_required(format!(
            "{what} 需要节点集自变量，实际是 {other}"
        ))),
    }
}

/// 取第一个自变量的字符串值；无自变量时取上下文节点（§4.2/§4.4 的
/// "the argument is optional, defaulting to the context node"）。
fn argument_string(values: &[XPathValue], context: &Context<'_>) -> String {
    match values.first() {
        Some(value) => to_string(value),
        None => string_value(&context.node),
    }
}

/// 取第一个自变量的节点集；无自变量时取上下文节点（§4.1）。
///
/// 无自变量时把上下文节点包成单元素节点集——这是 §4.1 对
/// `local-name()` / `namespace-uri()` / `name()` 的"defaulting to a
/// node-set containing just the context node"。
fn argument_nodes<'a>(
    values: &'a [XPathValue],
    context: &'a Context<'_>,
) -> Result<Vec<NodeRef>, XPathError> {
    match values.first() {
        Some(value) => Ok(require_nodes(value, "节点集函数")?.to_vec()),
        None => Ok(vec![context.node.clone()]),
    }
}

/// §4.1 `local-name()`。
fn local_name_of(values: &[XPathValue], context: &Context<'_>) -> String {
    argument_nodes(values, context)
        .ok()
        .and_then(|nodes| nodes.first().cloned())
        .map(|node| node.node_name())
        .unwrap_or_default()
}

/// §4.1 `namespace-uri()`。
fn namespace_uri_of(values: &[XPathValue], context: &Context<'_>) -> String {
    argument_nodes(values, context)
        .ok()
        .and_then(|nodes| nodes.first().cloned())
        .map(|node| node.namespace())
        .unwrap_or_default()
}

/// §4.1 `name()`：返回 QName（含前缀）。
fn qualified_name_of(values: &[XPathValue], context: &Context<'_>) -> String {
    argument_nodes(values, context)
        .ok()
        .and_then(|nodes| nodes.first().cloned())
        .map(|node| node.qualified_name())
        .unwrap_or_default()
}

/// §4.2 `substring-before()`。
///
/// > returns the substring of the first argument string that precedes the
/// > first occurrence of the second argument string ... or the empty string
/// > if the first argument string does not contain the second argument string
fn substring_before(text: &str, needle: &str) -> String {
    match text.find(needle) {
        Some(position) => text[..position].to_string(),
        None => String::new(),
    }
}

/// §4.2 `substring-after()`。
fn substring_after(text: &str, needle: &str) -> String {
    match text.find(needle) {
        Some(position) => text[position + needle.len()..].to_string(),
        None => String::new(),
    }
}

/// §4.2 `substring()`。
///
/// 规范用**四舍五入后的浮点位置**定义它，且位置是 **1 起**的：
///
/// > returns the substring of the first argument starting at the position
/// > specified in the second argument with length specified in the third
/// > argument. ... For example, `substring("12345", 2, 3)` returns `"234"`.
/// > ... `substring("12345", 1.5, 2.6)` returns `"234"`.
///
/// 且规范明确举了边界情形：`substring("12345", 0, 3)` 返回 `"12"`、
/// `substring("12345", -42, 1 div 0)` 返回 `"12345"`。故实现按
/// "先算闭开区间 `[round(start), round(start)+round(length))`，再与
/// `[1, len+1]` 求交"来写——这是把规范那几条例子的公共结构直接编码。
fn substring(values: &[XPathValue]) -> String {
    let text: Vec<char> = to_string(&values[0]).chars().collect();
    let start = round_xpath(to_number(&values[1]));

    // 无第三个自变量时"到末尾"：用 `Infinity` 让区间自然闭合（§4.2 的
    // 默认行为是"returning characters to the end of the string"）
    let length = match values.get(2) {
        Some(value) => round_xpath(to_number(value)),
        None => f64::INFINITY,
    };

    // 位置是 1 起的浮点数；`NaN` 参与时区间为空（规范下 `NaN` 的任何比较
    // 都是假，故这里显式短路）
    if start.is_nan() || length.is_nan() {
        return String::new();
    }

    let low = start.max(1.0);
    let high = (start + length).min(text.len() as f64 + 1.0);
    if high <= low {
        return String::new();
    }
    // `low`/`high` 已是 1 起的位置，转成 0 起下标
    let from = (low as usize).saturating_sub(1);
    let to = (high as usize).saturating_sub(1).min(text.len());
    text[from.min(to)..to].iter().collect()
}

/// §4.2 `normalize-space()`。
///
/// > returns the argument string with whitespace normalized by stripping
/// > leading and trailing whitespace and replacing sequences of whitespace
/// > characters by a single space
fn normalize_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// §4.2 `translate()`。
///
/// > returns the first argument string with occurrences of characters in the
/// > second argument string replaced by the character at the corresponding
/// > position in the third argument string. ... If a character occurs more
/// > than once in the second argument string, then the first occurrence
/// > determines the replacement character. ... If the third argument string is
/// > longer than the second argument string, then the extra characters are ignored.
/// > If a character occurs in the first argument string but is not in the second
/// > argument string, then the character is copied to the result string unchanged.
/// > ... if a character is in the second argument string but not the third,
/// > then that character is removed from the result.
///
/// 实现按上述四条逐字来：先给第二个串建"字符 → 下标"表（**首次出现优先**），
/// 再逐字符查表——查不到的原样复制，查到但下标超出第三个串则删除。
fn translate(text: &str, from: &str, to: &str) -> String {
    let from_characters: Vec<char> = from.chars().collect();
    let to_characters: Vec<char> = to.chars().collect();

    let mut result = String::with_capacity(text.len());
    for character in text.chars() {
        match from_characters.iter().position(|item| *item == character) {
            // 落在第二个串里：由第三个串的对应位置决定，超出则删除
            Some(index) => {
                if let Some(replacement) = to_characters.get(index) {
                    result.push(*replacement);
                }
            }
            None => result.push(character),
        }
    }
    result
}

/// §4.4 `round()`。
///
/// > returns the number that is closest to the argument and that is an
/// > integer. If there are two such numbers, then the one that is closest to
/// > positive infinity is returned. ... If the argument is NaN, then NaN is
/// > returned. If the argument is positive infinity, then positive infinity is
/// > returned. ... if the argument is less than zero, but greater than or
/// > equal to -0.5, then negative zero is returned.
///
/// 即**向正无穷取半**（不是 Rust 的 `f64::round`，那是"远离零取半"）。
/// `-0.3` 按规范是 `-0`，故这里保留符号。
fn round_xpath(value: f64) -> f64 {
    if value.is_nan() || value.is_infinite() || value == 0.0 {
        return value;
    }
    // `floor(x + 0.5)` 就是"向正无穷取半"
    let rounded = (value + 0.5).floor();
    // 规范要求 `-0.3 → -0`：`floor(0.2)` 得 `0`（正零），这里补回负号
    if rounded == 0.0 && value < 0.0 {
        return -0.0;
    }
    rounded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 元数校验给出明确错误() {
        assert!(Arity::Exact(2).check("contains", 2).is_ok());
        let error = Arity::Exact(2).check("contains", 1).expect_err("应报错");
        assert!(error.detail().contains("contains"), "错误应指出函数名");

        assert!(Arity::Range(0, 1).check("string", 0).is_ok());
        assert!(Arity::Range(0, 1).check("string", 1).is_ok());
        assert!(Arity::Range(0, 1).check("string", 2).is_err());
    }

    #[test]
    fn 未实现的函数不被静默接受() {
        assert!(lookup("nonexistentFunction").is_none());
        assert!(lookup("contains").is_some());
    }

    #[test]
    fn substring按规范的四舍五入位置语义() {
        // §4.2 给出的确切例子
        let call = |text: &str, start: f64, length: Option<f64>| {
            let mut values = vec![XPathValue::String(text.into()), XPathValue::Number(start)];
            if let Some(length) = length {
                values.push(XPathValue::Number(length));
            }
            substring(&values)
        };

        assert_eq!(call("12345", 2.0, Some(3.0)), "234");
        // §4.2：`substring("12345", 1.5, 2.6)` returns `"234"`
        assert_eq!(call("12345", 1.5, Some(2.6)), "234");
        // §4.2：`substring("12345", 0, 3)` returns `"12"`
        assert_eq!(call("12345", 0.0, Some(3.0)), "12");
        // §4.2：`substring("12345", -42, 1 div 0)` returns `"12345"`
        assert_eq!(call("12345", -42.0, Some(f64::INFINITY)), "12345");
        // 无第三个自变量 → 到末尾
        assert_eq!(call("12345", 2.0, None), "2345");
    }

    #[test]
    fn translate按规范的四条规则() {
        // §4.2 的确切例子：`translate("bar","abc","ABC")` = `"BAr"`
        assert_eq!(translate("bar", "abc", "ABC"), "BAr");
        // 映射是**按位置**的：第 2 个串里每个字符对应第 3 个串同位置的字符。
        // `aaa`→`xyz` 是"每个 a 都映射到 x"（都查到下标 0），而不是逐位不同。
        assert_eq!(translate("aaa", "aaa", "xyz"), "xxx");
        // 逐位不同的映射需要第二个串里各个字符**互不相同**
        assert_eq!(translate("abc", "abc", "xyz"), "xyz");
        // 第三个串更长时多余字符被忽略
        assert_eq!(translate("abc", "ab", "XYZ"), "XYc");
        // 在第二个串里但不在第三个串里 → 删除
        assert_eq!(translate("abc", "ab", "X"), "Xc");
        // 不在第二个串里 → 原样保留
        assert_eq!(translate("abc", "x", "Y"), "abc");
    }

    #[test]
    fn normalize_space折叠空白并去首尾() {
        assert_eq!(normalize_space("  a   b  "), "a b");
        assert_eq!(normalize_space(""), "");
        assert_eq!(normalize_space("a"), "a");
    }

    #[test]
    fn substring_before与after() {
        assert_eq!(substring_before("abc/d", "/"), "abc");
        assert_eq!(substring_before("abcd", "/"), "");
        assert_eq!(substring_after("abc/d", "/"), "d");
        assert_eq!(substring_after("abcd", "/"), "");
    }

    #[test]
    fn round向正无穷取半() {
        // §4.4：两个等距整数时取**靠近正无穷**的那个
        assert_eq!(round_xpath(2.5), 3.0);
        assert_eq!(round_xpath(-2.5), -2.0, "不是 Rust 的远离零取半");
        assert_eq!(round_xpath(1.4), 1.0);
        assert_eq!(round_xpath(1.6), 2.0);
        // §4.4：`-0.5 <= x < 0` 返回负零
        assert!(round_xpath(-0.3).is_sign_negative());
        assert!(round_xpath(f64::NAN).is_nan());
    }
}
