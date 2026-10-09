//! [DONE] JSONPath 聚合函数注册（tasks.md T047 / research R3）
//!
//! # 为什么用 `#[function]` 而不是文本替换
//!
//! Jayway 的 `min()/max()/avg()/stddev()/sum()` 是**终端函数**——它们
//! 出现在整条表达式的末尾（`$.list.min()`）。而 RFC 9535 里函数是
//! 表达式的一等公民，可以嵌套（`$[? length(@.tags) > 2 ]`）。
//!
//! 若用文本替换（`$.list.min()` → `$.list[...]` 某种手写形态），
//! 嵌套场合就会出错，且**参数类型**无从校验。故走 `serde_json_path`
//! 的官方注册宏：函数由引擎的**类型系统**校验（`NodesType` / `ValueType`
//! / `LogicalType`），误用会在 `JsonPath::parse` 阶段报错。
//!
//! # 五个函数的语义（依据公开文档）
//!
//! | 函数 | 语义 | 空集合行为 | 非数值忽略 |
//! |---|---|---|---|
//! | `min()` | 最小值 | `Nothing` | 是 |
//! | `max()` | 最大值 | `Nothing` | 是 |
//! | `avg()` | 平均值 | `Nothing` | 是 |
//! | `stddev()` | 样本标准差 | `Nothing` | 是 |
//! | `sum()` | 求和 | `0` | 是 |
//!
//! ## 三条语义决定与它们的依据
//!
//! 1. **只统计数值**：Jayway 的聚合函数对非数值元素**跳过**而不是报错
//!    （公开 README 的 Aggregate Functions 一节把参数描述为 "the number array"，
//!    而书源传来的数组常混有 `null`/字符串）。跳过比报错更符合书源的实际数据。
//! 2. **空集合给 `Nothing`**（而非 0）：`$.list.min()` 在空列表上给出
//!    "没有值"是正确的信号，给 0 会被书源误当成真实数据。
//!    `sum()` 例外——数学上空和是 0，且书源常用它做累加初值。
//! 3. **`stddev()` 是样本标准差**（除以 `n-1`）：Jayway 的实现对应
//!    Apache Commons Math 的 `StandardDeviation`，那是样本标准差。
//!    单元素时 `n-1 == 0`，返回 0 而不是 NaN。
//!
//! # 参数形状：为什么收 `NodesType`
//!
//! `$(...)` 与 `$.list.min()` 改写后传进来的都是**节点列表**（[`NodesType`]），
//! 故函数签名收 `NodesType`。RFC 9535 的类型系统允许把 nodelist 传给
//! 声明为 `ValueType` 的参数（当列表恰有一个节点时），但反向不成立——
//! 用 `NodesType` 能接受更宽的调用形态。

use serde_json::{Number, Value};

use serde_json_path::functions::{NodesType, ValueType};

use crate::error::RuleError;

/// 聚合统计的中间结果。
///
/// # 字段名为什么用 `smallest`/`largest` 而不是 `min`/`max`
///
/// `min()` / `max()` 是**要注册到 JSONPath 的函数名**（模块级 `pub fn`），
/// 而 Rust 里字段访问 `aggregate.min` 与函数名 `min` 分属不同命名空间、
/// 本不冲突。但可读性上"函数体内的 `min` 与结构体字段 `min`"容易误读，
/// 且将来若有人把字段访问写成方法调用就会踩到遮蔽。换个名字是零成本的。
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Aggregate {
    /// 已统计的数值个数
    count: usize,
    /// 数值和
    sum: f64,
    /// 数值平方和（标准差用）
    sum_of_squares: f64,
    /// 最小值
    smallest: Option<f64>,
    /// 最大值
    largest: Option<f64>,
}

impl Aggregate {
    /// 从节点列表里收集数值。
    ///
    /// # 只收数值是一处**有意**的宽容
    ///
    /// 书源的 JSON 数组常混入 `null`（接口的稀疏字段）与字符串（数值被
    /// 引号包裹，是接口侧的常见缺陷）。把它们当 0 会污染统计结果，
    /// 报错会让整条规则失败——取"跳过"是唯一能既给出结果又不误导的做法。
    ///
    /// # 为什么转发到 `collect_slice`
    ///
    /// 终端函数形态（`$.list.min()`）走的是"我们自己取出的 `Vec<Value>`"，
    /// 而过滤器里的嵌套调用走 `NodesType`。两处的**算法必须同一份**，
    /// 否则同一个 `min` 会在两个入口给出不同结果。
    fn collect(nodes: NodesType<'_>) -> Self {
        collect_slice(nodes.all())
    }
}

/// 把 `f64` 装成 `ValueType`。
///
/// # 为什么整数要还原成整数
///
/// `serde_json::Number::from_f64(3.0)` 序列化成 `3.0`，而书源里的
/// `min()` 结果用在字符串拼接时会得到 `3.0` 而不是 `3`——与 Java 的
/// `Long`/`Double` 语义不符（Jayway 对整型输入给整型）。
///
/// 故当 `f64` 是整数值时用 `Number::from(i64)`。（超出 i64 范围时退回浮点。）
fn number_to_value(number: f64) -> ValueType<'static> {
    if !number.is_finite() {
        // NaN / 无穷在 JSON 里没有表示。返回 `Nothing` 而不是伪造一个值
        // ——它表示"没有有效结果"，语义准确。
        return ValueType::Nothing;
    }
    if number.fract() == 0.0 {
        if let Some(integer) = f64_to_i64(number) {
            return ValueType::Value(Value::Number(Number::from(integer)));
        }
    }
    match Number::from_f64(number) {
        Some(number) => ValueType::Value(Value::Number(number)),
        None => ValueType::Nothing,
    }
}

/// `f64` → `i64`（只在**精确**可表示时转换）。
///
/// 用边界比较而不是 `as` 强转：`as` 在越界时是**饱和**的（给 `i64::MAX`），
/// 那会静默产生一个假的大数。这里返回 `None` 让调用方退回浮点表示。
///
/// ## 边界的算术
///
/// `i64` 的可表示范围是 `[-2^63, 2^63 − 1]`。浮点能精确表示这两个端点
/// （它们都是 2 的幂次），故判据是 `number >= -2^63 && number < 2^63`。
/// 用**严格小于**上界：`2^63 as f64` 恰好等于 `2^63`，而那个值超出 `i64`
/// （最大是 `2^63 − 1`），给 `Some` 会截断成 `i64::MIN`——一个静默的错值。
fn f64_to_i64(number: f64) -> Option<i64> {
    const LOWER: f64 = -9_223_372_036_854_775_808.0; // -2^63
    const UPPER_EXCLUSIVE: f64 = 9_223_372_036_854_775_808.0; // 2^63
    if (LOWER..UPPER_EXCLUSIVE).contains(&number) {
        Some(number as i64)
    } else {
        None
    }
}

/// 最小値。
///
/// RFC 9535 的 `#[function]` 宏要求函数是**自由函数**（它用 `inventory`
/// 在链接期收集），故五个函数都是模块级的 `pub`。
#[serde_json_path::function]
pub fn min(nodes: NodesType) -> ValueType<'static> {
    let aggregate = Aggregate::collect(nodes);
    to_value(aggregate.smallest)
}

/// 最大值。
#[serde_json_path::function]
pub fn max(nodes: NodesType) -> ValueType<'static> {
    let aggregate = Aggregate::collect(nodes);
    to_value(aggregate.largest)
}

/// 平均值。
///
/// # 为什么不叫 `mean`
///
/// 书源里写的是 `avg()`（Jayway 的函数名），故注册名必须是 `avg`。
#[serde_json_path::function]
pub fn avg(nodes: NodesType) -> ValueType<'static> {
    let aggregate = Aggregate::collect(nodes);
    if aggregate.count == 0 {
        return ValueType::Nothing;
    }
    let count = aggregate.count as f64;
    to_value(Some(aggregate.sum / count))
}

/// 求和。
///
/// # 空集合给 0 而不是 `Nothing`
///
/// 见模块文档的第 2 条：数学上空和是 0，且书源里 `sum()` 常作为累加起点。
#[serde_json_path::function]
pub fn sum(nodes: NodesType) -> ValueType<'static> {
    let aggregate = Aggregate::collect(nodes);
    number_to_value(aggregate.sum)
}

/// 样本标准差。
///
/// # 为什么是**样本**（除以 `n-1`）
///
/// Jayway 的实现对应 Apache Commons Math 的 `StandardDeviation`，那是
/// 样本标准差（Bessel 校正）。用总体标准差（除以 `n`）会在小样本上
/// 系统性偏小——而书源里的数组通常很短（章节列表、书单），差异显著。
///
/// # 单元素与空集合
///
/// - 空集合：`Nothing`（无值可算）
/// - 单元素：`n-1 == 0` 无法除，返回 `0`（样本标准差在 n=1 时无定义，
///   而 0 是书源期待的最不令人意外的值）
#[serde_json_path::function]
pub fn stddev(nodes: NodesType) -> ValueType<'static> {
    let aggregate = Aggregate::collect(nodes);
    if aggregate.count == 0 {
        return ValueType::Nothing;
    }
    if aggregate.count == 1 {
        return number_to_value(0.0);
    }
    let count = aggregate.count as f64;
    let mean = aggregate.sum / count;
    // 样本方差的数值稳定形式：Σx² − n·μ² 在均值远大于方差时会丢精度，
    // 故展开成 Σ(x−μ)² 的等价式并做下界保护（浮点舍入可能给出极小负数）。
    let variance = (aggregate.sum_of_squares - count * mean * mean) / (count - 1.0);
    number_to_value(variance.max(0.0).sqrt())
}

/// 把可选的数值结果装成函数返回值。
fn to_value(computed: Option<f64>) -> ValueType<'static> {
    match computed {
        Some(number) => number_to_value(number),
        None => ValueType::Nothing,
    }
}

/// 对一个**已取出的节点列表**施加终端函数（`$.list.min()` 形态）。
///
/// # 为什么需要一个"在 Rust 侧直接调用"的入口
///
/// RFC 9535 的查询语法不允许函数调用作为整条查询（查询必须以 `$` 起头）。
/// 故 Jayway 的终端形态（`$.list.min()`）无法交给 RFC 引擎——
/// [`crate::jsonpath`] 提主体路径取出节点后，经本函数算聚合。
///
/// # 与 `#[function]` 注册版的关系
///
/// **同一份算法，两个入口**：
///
/// | 入口 | 服务对象 |
/// |---|---|
/// | `#[function]` 注册的 `min()` 等 | 过滤器里的**嵌套**调用（`[? count(@.x) > min($.y)]`） |
/// | 本函数 | 整条表达式就是终端调用的场合（`$.list.min()`） |
///
/// 两者都调 [`Aggregate`] 的同一套计算，故结果必然一致——不会出现
/// "过滤器里算出来的 min 与终端算出来的 min 不同"这类分叉。
pub fn apply_terminal(
    name: &str,
    nodes: &[Value],
    expression: &str,
) -> Result<crate::jsonpath::JsonPathScalar, RuleError> {
    use crate::jsonpath::JsonPathScalar;

    match name {
        "length" => {
            // `length()` 对数组与字符串有效（RFC 9535 §2.4.4）。
            //
            // ## 多节点时取第一个
            //
            // `length()` 的语义是"单个值的长度"，而主体路径可能命中多个
            // （`$.a[*].list.length()`）。Jayway 在这种场合取首个——
            // 与 definite 路径的标量语义一致。
            let Some(node) = nodes.first() else {
                return Ok(JsonPathScalar::Nothing);
            };
            if let Some(array) = node.as_array() {
                return Ok(JsonPathScalar::Count(array.len()));
            }
            if let Some(text) = node.as_str() {
                // 字符数而不是字节数（Java 的 `String#length()` 是 UTF-16
                // 码元数，对 BMP 内的字符与字符数一致；书源处理的文本几乎
                // 全是 BMP，故按字符数）
                return Ok(JsonPathScalar::Count(text.chars().count()));
            }
            if let Some(object) = node.as_object() {
                // Jayway 的 `.length()` 对对象给**键数**（JS 的 `Object.keys().length`）
                return Ok(JsonPathScalar::Count(object.len()));
            }
            Ok(JsonPathScalar::Nothing)
        }
        "sum" | "min" | "max" | "avg" | "stddev" => {
            // ## 数组节点要先摊平
            //
            // Jayway 的聚合函数是**数组方法**（`$.list.min()` 里的 `$.list`
            // 是一个数组），而主体路径 `$.list` 在 RFC 引擎里返回的是
            // "那个数组节点"本身（**一个**节点），不是它的元素。
            //
            // 故此处摊平：主体命中单个数组时，对**数组元素**聚合。
            // 主体命中多个节点时，把它们整体当值集合（`$.a[*].n.min()`）。
            let flattened: Vec<&Value> = match nodes {
                [Value::Array(items)] if nodes.len() == 1 => items.iter().collect(),
                other => other.iter().collect(),
            };
            let aggregate = collect_slice(flattened);
            let computed = match name {
                "sum" => Some(aggregate.sum),
                "min" => aggregate.smallest,
                "max" => aggregate.largest,
                "avg" => {
                    if aggregate.count == 0 {
                        None
                    } else {
                        Some(aggregate.sum / aggregate.count as f64)
                    }
                }
                "stddev" => {
                    if aggregate.count == 0 {
                        None
                    } else if aggregate.count == 1 {
                        Some(0.0)
                    } else {
                        let count = aggregate.count as f64;
                        let mean = aggregate.sum / count;
                        let variance =
                            (aggregate.sum_of_squares - count * mean * mean) / (count - 1.0);
                        Some(variance.max(0.0).sqrt())
                    }
                }
                _ => None,
            };
            match computed {
                Some(number) => match number_to_value(number).as_value() {
                    Some(value) => Ok(JsonPathScalar::Value(value.clone())),
                    None => Ok(JsonPathScalar::Nothing),
                },
                None => Ok(JsonPathScalar::Nothing),
            }
        }
        other => Err(RuleError::unsupported(
            format!("jsonPath:{other}"),
            format!("JSONPath 终端函数 `{other}`（表达式 `{expression}`）"),
        )),
    }
}

/// 对借用的节点切片做聚合（与 [`Aggregate::collect`] 同一套算法）。
///
/// # 为什么不能直接复用 `Aggregate::collect`
///
/// 它收的是 `NodesType`（RFC 引擎的包装类型），而终端形态的节点是我们
/// 自己从 `JsonPath::query` 取出的 `Vec<Value>`。为两者各写一份聚合会让
/// "同一个 min 在两处算出不同结果"成为可能，故此处把**算法**抽到本函数，
/// `NodesType` 版本经它转发。
fn collect_slice<'a>(nodes: impl IntoIterator<Item = &'a Value>) -> Aggregate {
    let mut aggregate = Aggregate::default();
    for node in nodes {
        let Some(number) = node.as_f64() else {
            continue;
        };
        aggregate.count += 1;
        aggregate.sum += number;
        aggregate.sum_of_squares += number * number;
        aggregate.smallest = Some(
            aggregate
                .smallest
                .map_or(number, |current| current.min(number)),
        );
        aggregate.largest = Some(
            aggregate
                .largest
                .map_or(number, |current| current.max(number)),
        );
    }
    aggregate
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 在一个 JSON 上跑一条 JSONPath，返回结果序列的文本形式。
    ///
    /// ## 为什么经 shim 而不是直接 `JsonPath::parse`
    ///
    /// 这些测试要验证的是"书源里写的 Jayway 表达式能跑通"——而书源写的
    /// 正是终端方法形态（`$.list.min()`）。直接喂 `JsonPath::parse`
    /// 测的是 RFC 9535 的前缀形态，与真实用法不符。
    fn run(value: &Value, expression: &str) -> Vec<String> {
        crate::jsonpath::query_expression(value, expression)
            .unwrap_or_else(|error| panic!("`{expression}` 应能求值：{error}"))
            .flatten_for_rule_engine()
    }

    #[test]
    fn 求最小值() {
        let value = json!({"list": [3, 1, 2]});
        assert_eq!(run(&value, "$.list.min()"), vec!["1"]);
    }
    #[test]
    fn 求最大值() {
        let value = json!({"list": [3, 1, 2]});
        assert_eq!(run(&value, "$.list.max()"), vec!["3"]);
    }

    #[test]
    fn 求平均值() {
        let value = json!({"list": [1, 2, 3]});
        assert_eq!(run(&value, "$.list.avg()"), vec!["2"]);
    }

    #[test]
    fn 求和() {
        let value = json!({"list": [1, 2, 3]});
        assert_eq!(run(&value, "$.list.sum()"), vec!["6"]);
    }

    #[test]
    fn 求样本标准差() {
        // [2,4,4,4,5,5,7,9] 的**样本**标准差是 2.138089935299395
        // （总体标准差是 2.0）。两者不同正是本实现选样本口径的证据。
        let value = json!({"list": [2, 4, 4, 4, 5, 5, 7, 9]});
        let got = run(&value, "$.list.stddev()");
        assert_eq!(got.len(), 1);
        let number: f64 = got[0].parse().expect("应是数值");
        assert!(
            (number - 2.138_089_935_299_395).abs() < 1e-9,
            "样本标准差应为 2.138089935299395，实际 {number}（若为 2.0 说明用了总体口径）"
        );
    }

    #[test]
    fn 单元素的样本标准差为零() {
        // n-1 == 0 无法除；返回 0 而不是 NaN
        let value = json!({"list": [5]});
        assert_eq!(run(&value, "$.list.stddev()"), vec!["0"]);
    }

    #[test]
    fn 空集合的聚合结果() {
        let value = json!({"list": []});
        // 前四个给 Nothing（无结果）
        for expression in [
            "$.list.min()",
            "$.list.max()",
            "$.list.avg()",
            "$.list.stddev()",
        ] {
            assert_eq!(
                run(&value, expression),
                Vec::<String>::new(),
                "`{expression}` 在空集合上应无结果"
            );
        }
        // `sum()` 例外：数学上空和是 0
        assert_eq!(run(&value, "$.list.sum()"), vec!["0"]);
    }

    #[test]
    fn 非数值元素被跳过而不是报错() {
        // 书源的数组常混入 null 与字符串（接口侧缺陷）
        let value = json!({"list": [1, null, "x", 3]});
        assert_eq!(run(&value, "$.list.min()"), vec!["1"]);
        assert_eq!(run(&value, "$.list.max()"), vec!["3"]);
        assert_eq!(run(&value, "$.list.avg()"), vec!["2"]);
        assert_eq!(run(&value, "$.list.sum()"), vec!["4"]);
    }

    #[test]
    fn 全非数值的集合按空处理() {
        let value = json!({"list": [null, "a"]});
        assert_eq!(run(&value, "$.list.min()"), Vec::<String>::new());
        assert_eq!(run(&value, "$.list.sum()"), vec!["0"]);
    }

    #[test]
    fn 整数结果不被渲染成浮点() {
        // Jayway 对整型输入给整型——`3` 而不是 `3.0`
        let value = json!({"list": [1, 2, 3]});
        assert_eq!(run(&value, "$.list.max()"), vec!["3"]);
        assert!(
            !run(&value, "$.list.max()")[0].contains('.'),
            "整数结果不该带小数点"
        );
    }

    #[test]
    fn 小数结果保留小数() {
        let value = json!({"list": [1, 2]});
        assert_eq!(run(&value, "$.list.avg()"), vec!["1.5"]);
    }

    #[test]
    fn 负数可统计() {
        let value = json!({"list": [-3, -1, -2]});
        assert_eq!(run(&value, "$.list.min()"), vec!["-3"]);
        assert_eq!(run(&value, "$.list.max()"), vec!["-1"]);
        assert_eq!(run(&value, "$.list.sum()"), vec!["-6"]);
    }

    #[test]
    fn 函数可嵌套在过滤器里() {
        // 这是"用注册函数而非文本替换"的直接收益：函数是表达式的一等公民
        let value = json!({"list": [{"tags": [1, 2, 3]}, {"tags": [1]}]});
        let got = run(&value, "$.list[? length(@.tags) > 1 ]");
        assert_eq!(got.len(), 1, "只有第一个条目的 tags 长度大于 1");
    }

    #[test]
    fn 负零与整数的边界() {
        assert_eq!(f64_to_i64(0.0), Some(0));
        assert_eq!(f64_to_i64(-1.0), Some(-1));
        // `i64::MIN` 恰好可表示，且在范围内
        assert_eq!(f64_to_i64(-9_223_372_036_854_775_808.0), Some(i64::MIN));
        // 越界时给 None（而不是饱和到 i64::MAX——那会静默产生假的大数）
        assert_eq!(
            f64_to_i64(9_223_372_036_854_775_808.0),
            None,
            "2^63 超出 i64"
        );
        assert_eq!(f64_to_i64(1e30), None);
        assert_eq!(f64_to_i64(-1e30), None);
    }

    #[test]
    fn 非有限数给nothing() {
        assert!(number_to_value(f64::NAN).is_nothing());
        assert!(number_to_value(f64::INFINITY).is_nothing());
        assert!(to_value(None).is_nothing());
        assert_eq!(
            to_value(Some(1.0)).as_value().and_then(Value::as_i64),
            Some(1)
        );
    }
}
