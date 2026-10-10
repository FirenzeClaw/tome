//! \[DONE] JSONPath 引擎（tasks.md T045 / research.md R3）
//!
//! # 为什么是 serde_json_path
//!
//! research R3 已论证：引擎选 **`serde_json_path`**（RFC 9535 最字面实现），
//! 前置一个**词法感知的 Jayway→RFC9535 重写 shim**（[`crate::jsonpath_shim`]），
//! 结果形状由 [`crate::jsonpath_fn`] 适配。
//!
//! 三条理由：
//!
//! 1. **过滤器实现最完整**——`&&`/`||`/`!`/括号表达式/函数注册 API 都有，
//!    而 `jsonpath_lib`（解析行为最贴近 Jayway）2021-06 起停更。
//! 2. **有官方函数注册宏**（`#[function]`），`min/max/avg/stddev/sum`
//!    能成为一等函数而非字符串替换——后者会在嵌套表达式里出错。
//! 3. **RFC 9535 是最字面的规范**，"与 Jayway 的差异"因此可以被**枚举**出来
//!    并逐条翻译，而不是"两套方言各自演化"。
//!
//! # 分隔：本模块只做"求值"，不做"方言翻译"
//!
//! 三层职责分明（对应 tasks.md 的三个任务编号）：
//!
//! | 层 | 模块 | 职责 |
//! |---|---|---|
//! | 翻译 | [`crate::jsonpath_shim`] | Jayway 专有**语法** → RFC 9535 |
//! | 求值 | 本模块 | 解析 + 查询 + 结果形状适配 |
//! | 函数 | [`crate::jsonpath_fn`] | `min/max/avg/stddev/sum` 注册 |
//!
//! 拆开是为了让每一层都能**独立测试**：shim 的正确性靠"改写前后文本"断言，
//! 与能否真的查出一致结果无关（那由本模块的端到端测试覆盖）。
//!
//! # 与上游 Jayway 的语义对齐点
//!
//! 公开资料（Jayway JsonPath 的 README 与 Goessner 的 JSONPath 原文）里，
//! 书源最常用的三处与 RFC 9535 的差异是：
//!
//! | 特性 | Jayway | RFC 9535 | 处理 |
//! |---|---|---|---|
//! | 结果形状 | definite 路径给**标量**，indefinite 给**列表** | 一律给列表 | [`JsonPathResult`] 复现 Jayway 形状 |
//! | `$.a.length()` | 数组/字符串长度 | 无此函数 | shim 改写为 `length()` 函数调用 |
//! | `min()/max()/avg()/stddev()/sum()` | 终端函数 | 无 | [`crate::jsonpath_fn`] 注册 |
//!
//! ## 参考来源（干净室纪律：只依据公开文档）
//!
//! - Goessner 的 JSONPath 原文（goessner.net/articles/JsonPath）——定义 `$..`、
//!   `[*]`、`[?(...)]` 的基本形态
//! - Jayway JsonPath 的 README（github.com/json-path/JsonPath）——列举其扩展：
//!   脚本索引 `[(@.length-1)]`、终端函数、`in/nin/subsetof/anyof/noneof`、
//!   `=~ /re/i`、`.length()`
//! - RFC 9535（JSONPath: Query Expressions for JSON）——本引擎的**目标语法**，
//!   段（segment）/选择器（selector）/过滤器表达式（filter-expression）的定义

use serde_json::Value;

use crate::error::RuleError;

/// JSONPath 求值结果。
///
/// # 为什么要区分"明确"与"不明确"
///
/// 这是 Jayway 与 RFC 9535 最实质的**语义**差异（R3 点名的"结果形状"）：
///
/// - **definite**（确定路径，如 `$.a.b`、`$[0]`）在 Jayway 里返回**标量**
///   ——`$.store.book[0].title` 直接给出那个字符串。
/// - **indefinite**（`..`、`[*]`、`[?()]`、切片）在 Jayway 里返回**列表**。
///
/// RFC 9535 一律返回节点列表。若不复现这个差异，书源里
/// `@json:$.data.name`（期望一个字符串）会拿到 `["名字"]`，
/// 后续的替换链 `##^##x` 就会作用在 JSON 数组字面量上——静默出错。
///
/// 复现方式见 [`JsonPathResult::flatten_for_rule_engine`]。
#[derive(Debug, Clone, PartialEq)]
pub struct JsonPathResult {
    /// 匹配到的节点（按文档序）
    pub nodes: Vec<Value>,
    /// 查询路径是否为 definite（确定路径）
    pub definite: bool,
    /// **终端函数**的结果（`$.list.min()` 形态）。
    ///
    /// # 为什么单独一个字段而不是塞进 `nodes`
    ///
    /// 终端函数在 Jayway 里返回**标量**，而 RFC 9535 的查询语法**不允许**
    /// 函数调用直接作为整个查询（查询必须以 `$` 起头；函数只能出现在过滤器
    /// 的表达式里）。故这类表达式无法交给 RFC 引擎求值，由
    /// [`crate::jsonpath_fn`] 在 Rust 侧直接算。
    ///
    /// `None` 表示"不是终端函数形态"，此时结果在 [`Self::nodes`] 里。
    pub scalar: Option<JsonPathScalar>,
}

/// 终端函数的结果。
///
/// 三个变体对应 Jayway 的三类返回：
///
/// | 变体 | 来源 | Jayway 返回 |
/// |---|---|---|
/// | `Value` | `min/max/avg/stddev/sum` | 数值 |
/// | `Count` | `length()` | 整数 |
/// | `Nothing` | 空集合上的聚合 | 无值 |
#[derive(Debug, Clone, PartialEq)]
pub enum JsonPathScalar {
    /// 一个数值结果
    Value(Value),
    /// 长度结果（`length()`）
    Count(usize),
    /// 无结果（空集合上的聚合）
    Nothing,
}

impl JsonPathResult {
    /// 按 Jayway 的结果形状扁平化成规则引擎消费的字符串列表。
    ///
    /// # 语义（逐条对应上表）
    ///
    /// | 情形 | 输出 |
    /// |---|---|
    /// | 终端函数 | 该标量的字面量（0 或 1 个元素） |
    /// | definite + 命中 1 个 | 该值的字面量（**标量形状**） |
    /// | definite + 命中 0 个 | 空列表 |
    /// | indefinite + 命中 N 个 | N 个字面量 |
    /// | 命中值是数组/对象 | JSON 序列化文本（书源里用它再交给 `@js:` 或替换链） |
    ///
    /// ## 为什么标量形状是"一个字面量"而不是"额外包一层"
    ///
    /// 规则引擎的产出类型是 `Vec<String>`（见 [`crate::css_post::evaluate_rule`]）。
    /// Jayway 的标量返回值落到这里就是"只有一个元素的列表"——
    /// 与 CSS 规则命中一个节点时的形状一致。调用方（搜索管线取首个）无需分支。
    #[must_use]
    pub fn flatten_for_rule_engine(&self) -> Vec<String> {
        if let Some(scalar) = &self.scalar {
            return match scalar {
                JsonPathScalar::Value(value) => vec![render_json_value(value)],
                JsonPathScalar::Count(count) => vec![count.to_string()],
                // 空集合上的聚合无结果——空列表（而不是 "null"）
                JsonPathScalar::Nothing => Vec::new(),
            };
        }
        self.nodes.iter().map(render_json_value).collect()
    }
}

/// 把一个 JSON 值渲染成书源规则里期待的文本。
///
/// # 三类值的处理
///
/// - **字符串**：直接给内容（不带引号）。书源里 `@json:$.name` 期望的是
///   `斗破苍穹` 而不是 `"斗破苍穹"`——带引号会让后续的替换链与拼接出错。
/// - **数字/布尔/null**：用 JSON 的标准字面量（`1`、`1.5`、`true`、`null`）。
/// - **数组/对象**：JSON 序列化。书源拿到它通常再交给 `@js:` 处理，
///   而给的必须是合法 JSON。
///
/// # 为什么不用 `Value::to_string()`
///
/// 那条路对字符串会给带引号的形式（`"斗破"`）。三类值的目标形状不同，
/// 故必须分支。
#[must_use]
pub fn render_json_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        // 数字/布尔/null 的 `to_string` 就是 JSON 字面量，无需特殊处理
        other => other.to_string(),
    }
}

/// 解析并求值一条 JSONPath 表达式。
///
/// # 流程
///
/// ```text
///   Jayway 原文
///     → jsonpath_shim::rewrite        （Jayway 专有语法 → RFC 9535）
///     → JsonPath::parse               （RFC 9535 语法解析）
///     → query                         （在 Value 上求值）
///     → JsonPathResult                （形状标记）
/// ```
///
/// # 错误分类（FR-006）
///
/// 语法错报 [`RuleError::syntax`]；超出处理上界报 [`RuleError::limit`]。
/// 书源的 `try/catch` 靠错误分类决定降级路径，故两者不能混为一谈。
pub fn query_expression(json: &Value, expression: &str) -> Result<JsonPathResult, RuleError> {
    let rewritten = crate::jsonpath_shim::rewrite(expression);
    query_rewritten(json, expression, &rewritten)
}

/// [`query_expression`] 的**短路版本**：调用方已自行做过 shim 改写。
///
/// 拆出来是为了让测试能直接对"改写后的 RFC 9535 文本"求值，
/// 从而把 shim 的缺陷与引擎的缺陷分开归因。
pub fn query_rewritten(
    json: &Value,
    original: &str,
    rewritten: &str,
) -> Result<JsonPathResult, RuleError> {
    // ## 终端函数先于 RFC 求值（见 `JsonPathResult::scalar` 的说明）
    //
    // `$.list.min()` 在 Jayway 里返回标量，而 RFC 9535 的查询**不允许**
    // 函数调用作为整条查询。故这类表达式在此分流：提取主体路径，
    // 在 Rust 侧对取出的节点算聚合。
    if let Some(terminal) = crate::jsonpath_shim::parse_terminal_function(rewritten) {
        return evaluate_terminal_function(json, original, &terminal);
    }

    let path = serde_json_path::JsonPath::parse(rewritten).map_err(|error| {
        RuleError::syntax(format!(
            "JSONPath 解析失败：`{original}`（改写后为 `{rewritten}`）：{error}"
        ))
    })?;

    let list = path.query(json);
    let nodes: Vec<Value> = list.iter().map(|node| (*node).clone()).collect();

    Ok(JsonPathResult {
        nodes,
        definite: crate::jsonpath_shim::is_definite(rewritten),
        scalar: None,
    })
}

/// 求值一条**终端函数**形态的表达式（`min($.list)`）。
///
/// # 为什么在 Rust 侧算而不是喂给 RFC 引擎
///
/// 见 [`JsonPathResult::scalar`] 的说明：RFC 9535 的查询语法不允许函数
/// 调用作为整条查询。故此处提主体路径、用 RFC 引擎取节点、再调
/// [`crate::jsonpath_fn`] 的聚合实现。
fn evaluate_terminal_function(
    json: &Value,
    original: &str,
    terminal: &crate::jsonpath_shim::TerminalFunction,
) -> Result<JsonPathResult, RuleError> {
    // 主体路径单独求值（它本身是合法 RFC 9535 查询）
    let subject = serde_json_path::JsonPath::parse(&terminal.subject).map_err(|error| {
        RuleError::syntax(format!(
            "JSONPath 终端函数的主体解析失败：`{original}`（主体 `{}`）：{error}",
            terminal.subject
        ))
    })?;
    let nodes: Vec<Value> = subject
        .query(json)
        .iter()
        .map(|node| (*node).clone())
        .collect();

    let scalar = crate::jsonpath_fn::apply_terminal(&terminal.name, &nodes, original)?;
    Ok(JsonPathResult {
        nodes: Vec::new(),
        definite: true,
        scalar: Some(scalar),
    })
}

/// 便捷入口：把 JSON **文本**按 JSONPath 求值，产出规则引擎消费的字符串列表。
///
/// \[DONE] T045 的对外入口（`css_post` 的 `@json:` 分支与搜索管线的 JSON 源走它）。
///
/// # 文本而非 `Value`
///
/// 书源拿到的 JSON 总是响应体文本（`java.ajax()` 的返回值），故入口收文本。
/// 需要复用已解析 `Value` 的调用方用 [`query_expression`]。
pub fn evaluate(json_text: &str, expression: &str) -> Result<Vec<String>, RuleError> {
    let value = parse_json_text(json_text)?;
    query_expression(&value, expression).map(|result| result.flatten_for_rule_engine())
}

/// 解析 JSON 文本（容错 BOM 与首尾空白）。
///
/// # 为什么容错 BOM
///
/// 与 `tome_js::api::data::parse_json` 同源的理由：书源拿到的文本是从响应体
/// 里截出来的，可能带 UTF-8 BOM，而那个字符在编辑器里看不见——
/// 直接 `serde_json::from_str` 报的 "expected value at line 1 column 1"
/// 会让作者以为是接口返回了坏数据。
///
/// ## 两处实现为什么没有合并
///
/// `tome-js` 不能依赖 `tome-rule`（架构 A1 的星型依赖），故这边的容错
/// 逻辑只能自成一份。它是 **5 行**的规则（`trim_start_matches(BOM).trim()`），
/// 为它造一个 trait 反而是过度设计（宪法原则 IV：YAGNI）。
pub fn parse_json_text(text: &str) -> Result<Value, RuleError> {
    let cleaned = text.trim_start_matches('\u{feff}').trim();
    serde_json::from_str(cleaned).map_err(|error| {
        RuleError::syntax(format!(
            "JSON 解析失败：{error}（前 80 字节：{:?}）",
            cleaned.chars().take(80).collect::<String>()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 简单路径取值() {
        let value = json!({"store": {"book": [{"title": "A"}, {"title": "B"}]}});
        let result = query_expression(&value, "$.store.book[*].title").expect("应求值成功");
        assert_eq!(
            result.flatten_for_rule_engine(),
            vec!["A", "B"],
            "indefinite 路径给列表"
        );
        assert!(!result.definite);
    }

    #[test]
    fn definite路径给标量形状() {
        // Jayway 语义：确定路径返回标量。落到本项目的 `Vec<String>` 就是
        // "只有一个元素的列表"——与 CSS 命中一个节点的形状一致。
        let value = json!({"store": {"book": [{"title": "A"}, {"title": "B"}]}});
        let result = query_expression(&value, "$.store.book[0].title").expect("应求值成功");
        assert_eq!(result.flatten_for_rule_engine(), vec!["A"]);
        assert!(result.definite, "`$.a.b[0]` 是确定路径");
    }

    #[test]
    fn definite路径未命中给空() {
        let value = json!({"a": 1});
        let result = query_expression(&value, "$.nope.deep").expect("应求值成功");
        assert_eq!(result.flatten_for_rule_engine(), Vec::<String>::new());
    }

    #[test]
    fn 递归下降() {
        let value = json!({"a": {"b": {"c": 1}}, "d": {"c": 2}});
        let result = query_expression(&value, "$..c").expect("应求值成功");
        assert_eq!(result.flatten_for_rule_engine(), vec!["1", "2"]);
    }

    #[test]
    fn 过滤器表达式() {
        let value = json!({"list": [
            {"type": 1, "name": "keep"},
            {"type": 2, "name": "drop"}
        ]});
        let result = query_expression(&value, r"$.list[?(@.type==1)].name").expect("应求值成功");
        assert_eq!(result.flatten_for_rule_engine(), vec!["keep"]);
    }

    #[test]
    fn 过滤器里的逻辑运算符() {
        let value = json!({"list": [
            {"a": 1, "b": 1}, {"a": 1, "b": 2}, {"a": 2, "b": 2}
        ]});
        let result = query_expression(&value, r"$.list[?(@.a==1 && @.b==2)]").expect("应求值成功");
        assert_eq!(result.nodes.len(), 1);
        let result = query_expression(&value, r"$.list[?(@.a==1 || @.a==2)]").expect("应求值成功");
        assert_eq!(result.nodes.len(), 3);
    }

    #[test]
    fn 字符串值不带引号() {
        // 书源里 `@json:$.name` 期望的是内容本身，不是 JSON 字面量
        let value = json!({"name": "斗破苍穹"});
        let result = query_expression(&value, "$.name").expect("应求值成功");
        assert_eq!(result.flatten_for_rule_engine(), vec!["斗破苍穹"]);
    }

    #[test]
    fn 非字符串标量用json字面量() {
        let value = json!({"n": 1, "f": 1.5, "b": true, "z": null});
        for (path, expected) in [
            ("$.n", "1"),
            ("$.f", "1.5"),
            ("$.b", "true"),
            ("$.z", "null"),
        ] {
            let result = query_expression(&value, path).expect("应求值成功");
            assert_eq!(
                result.flatten_for_rule_engine(),
                vec![expected],
                "路径 {path}"
            );
        }
    }

    #[test]
    fn 数组与对象用json序列化() {
        let value = json!({"arr": [1, 2], "obj": {"k": "v"}});
        let result = query_expression(&value, "$.arr").expect("应求值成功");
        // serde_json 的紧凑序列化（无空格）
        assert_eq!(result.flatten_for_rule_engine(), vec!["[1,2]"]);
        let result = query_expression(&value, "$.obj").expect("应求值成功");
        assert_eq!(result.flatten_for_rule_engine(), vec![r#"{"k":"v"}"#]);
    }

    #[test]
    fn 从文本求值容错_bom() {
        let values = evaluate("\u{feff} {\"a\": \"x\"} ", "$.a").expect("应求值成功");
        assert_eq!(values, vec!["x"]);
    }

    #[test]
    fn 非法_json_报语法错() {
        let error = evaluate("{ 不是 json", "$.a").expect_err("应报错");
        assert!(matches!(error, RuleError::Syntax { .. }));
        assert!(
            format!("{error}").contains("JSON 解析失败"),
            "错误应指明是 JSON 本身的问题"
        );
    }

    #[test]
    fn 非法_jsonpath_报语法错() {
        let error = evaluate(r#"{"a":1}"#, "$.[").expect_err("应报错");
        assert!(matches!(error, RuleError::Syntax { .. }));
        let rendered = format!("{error}");
        assert!(rendered.contains("JSONPath 解析失败"), "{rendered}");
    }

    #[test]
    fn 根节点取值() {
        let values = evaluate(r#"{"a":1}"#, "$").expect("应求值成功");
        assert_eq!(values, vec![r#"{"a":1}"#]);
    }

    #[test]
    fn 数组索引起头形态() {
        // 公开文档：「最好以 @json: 或 $. 开头」，但 `$[` 形态也存在
        let values = evaluate(r#"[{"n":"a"},{"n":"b"}]"#, "$[1].n").expect("应求值成功");
        assert_eq!(values, vec!["b"]);
    }

    #[test]
    fn 切片选择器() {
        let value = json!({"list": [0, 1, 2, 3, 4]});
        let result = query_expression(&value, "$.list[1:3]").expect("应求值成功");
        assert_eq!(result.flatten_for_rule_engine(), vec!["1", "2"]);
    }

    #[test]
    fn 通配符选择器() {
        let value = json!({"a": {"x": 1, "y": 2}});
        let result = query_expression(&value, "$.a.*").expect("应求值成功");
        let mut got = result.flatten_for_rule_engine();
        got.sort();
        assert_eq!(got, vec!["1", "2"]);
    }

    /// 病态输入不得让引擎崩溃（AGENTS.md 生产级护栏）。
    ///
    /// # 这个测试防的是什么
    ///
    /// 书源规则与接口响应都是**不可信输入**。两类病态形态：
    ///
    /// | 形态 | 风险 | 本实现的落点 |
    /// |---|---|---|
    /// | 极深嵌套的 JSON | 解析器递归爆栈 | `serde_json` 自带的递归上界 |
    /// | 极长的路径表达式 | 解析器或求值器爆栈/超时 | shim 是**迭代**扫描（无递归） |
    ///
    /// 断言"不 panic 且给出结构化结果"——Rust 里栈溢出不可捕获，所以
    /// 一旦爆栈整个进程直接死，这是必须守住的一条线。
    #[test]
    fn 病态输入不崩溃() {
        // 极深 JSON：应报错而不是爆栈
        for depth in [1_000usize, 10_000] {
            let json = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
            let outcome = evaluate(&json, "$");
            assert!(
                outcome.is_err(),
                "{depth} 层嵌套的 JSON 应报错（serde_json 的递归上界）而不是静默成功"
            );
        }

        // 极长路径：shim 与引擎都应能处理（不递归）
        for length in [1_000usize, 20_000] {
            let expression = format!("${}", ".a".repeat(length));
            // 对 `{"a":1}` 求值应给出空结果（路径太深匹配不到），而不是崩溃
            let outcome = evaluate(r#"{"a":1}"#, &expression);
            assert!(outcome.is_ok(), "{length} 段的路径不该崩：{outcome:?}");
        }

        // 浅层照常工作（护栏不该误伤）
        assert_eq!(
            evaluate(r#"{"a":"b"}"#, "$.a").expect("应求值成功"),
            vec!["b"]
        );
    }
}
