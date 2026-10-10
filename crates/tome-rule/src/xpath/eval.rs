//! \[DONE] XPath 1.0 子集求值器（tasks.md T049）
//!
//! # 求值的三条主线
//!
//! 1. **位置路径**（§2、§3.3）——从上下文节点集出发，逐步推进；
//!    每一步的产出先按**轴方向**编号（谓词位置语义，§2.4），
//!    谓词过滤后再按文档序归一化（节点集是无序集合，§3.3）。
//! 2. **函数调用**（§4）——内建函数表 + 书源生态扩展函数。
//! 3. **运算符**（§3.4 / §3.5）——按 §3.4 的转换规则在四种类型间转换。
//!
//! # 关于"应用顺序"
//!
//! 规范对 `//para[1]` 与 `/descendant::para[1]` 的差别有明确说明（§3.3）：
//!
//! > `//para[1]` does not mean the same as `/descendant::para[1]`. The latter
//! > selects the first descendant `para` element; the former selects all
//! > descendant `para` elements that are the first `para` children of their
//! > parents.
//!
//! 本实现因为把 `//x` 解析成 `descendant-or-self::node()` + `child::x`
//! 两相步（见 `parse.rs`），谓词作用在 `child::x` 这一相上，
//! **自动**得到正确语义——不需要任何特判。这正是取规范的展开式而不是
//! "把 `//` 当成 descendant 轴"的理由。

use super::ast::{Axis, BinaryOperator, CompiledExpression, Expr, LocationPath, NodeTest, Value};
use super::error::{XPathError, XPathErrorCode};
use super::value::{compare_document_order, same_node, string_value, XPathValue};
use crate::dom::{Document, NodeRef};

/// 谓词求值的最大步数。
///
/// 每条谓词对每个候选节点求一次；候选节点数由输入文档决定（不可信），
/// 而谓词里若含 `count(//x)` 这类全文档扫描，单次求值又会下探整棵树。
/// 这个上界把"表达式 × 文档"的乘积封住，使最坏情况有确定天花板
/// （AGENTS.md 的生产级护栏：引擎不得因输入崩溃或无限耗时）。
const MAX_PREDICATE_STEPS: u64 = 5_000_000;

/// 求值上下文（W3C XPath 1.0 §1.2 "Context"）。
///
/// 规范列的上下文成分有五项：context node、context position、context size、
/// variable bindings、function library、namespace declarations。
/// 本项目只支持 XPath 的子集，故：
///
/// - **变量绑定不支持**（书源规则没有给 XPath 传变量的机制，见 `parse.rs`）
/// - **命名空间声明不支持**（HTML DOM 无前缀→URI 的声明表；
///   \[UNCERTAIN] 带前缀的名字测试按本地名匹配，见 [`matches_name_test`]）
pub(super) struct Context<'a> {
    /// 上下文节点
    pub(super) node: NodeRef,
    /// 上下文位置（**1 起**，§2.4）
    pub(super) position: usize,
    /// 上下文大小（§2.4）
    pub(super) size: usize,
    /// 求值所在的文档（等于 `node.document()`，缓存一份省去重复取引用）
    document: &'a Document,
    /// 当前值（只在"值序列上的谓词"里被置位，供 `[. > 3]` 形态使用）。
    current_value: Option<XPathValue>,
    /// 当前正在求值的属性名（只在属性轴的谓词里被置位）。
    ///
    /// 属性不是独立节点（见 [`axis_nodes`] 的文档），故"当前属性"没有
    /// 节点上下文可承载，只能挂在求值上下文上。
    current_attribute: Option<String>,
    /// 已消耗的谓词步数（对照 [`MAX_PREDICATE_STEPS`]）
    steps: u64,
}

impl<'a> Context<'a> {
    pub(super) fn new(document: &'a Document, node: NodeRef) -> Self {
        Self {
            node,
            position: 1,
            size: 1,
            document,
            current_value: None,
            current_attribute: None,
            steps: 0,
        }
    }
}

/// 在文档上求值一条**已编译**的表达式。
///
/// \[DONE] 求值层的唯一入口。上下文节点是文档根（与 XPath 的"初始上下文"
/// 一致：绝对路径 `/` 在任何上下文里都指向根，而相对路径从根起走）。
///
/// \[UNCERTAIN] 规范没有规定 XPath 求值的**初始上下文节点**是什么——
/// 那是宿主语言的职责。本项目的选择是"根节点"，因为书源规则的 XPath
/// 一律写成绝对路径（`//div[@class='x']`），从根起走是唯一有意义的解释。
/// 若将来出现以相对路径书写的规则，这个选择需要复核。
pub fn evaluate_compiled(
    document: &Document,
    compiled: &CompiledExpression,
) -> Result<XPathValue, XPathError> {
    let root = document.root();
    let mut context = Context::new(document, root);
    evaluate_expression(&compiled.expression, &mut context)
}

/// 在**子树**内求值一条已编译的表达式。
///
/// \[DONE] 与 [`crate::css_post::evaluate_rule_in`] 同款语义：求值起点从
/// `root` 起，供搜索管线按 `bookList` 切出每本书的子树后逐字段求值。
///
/// 绝对路径（`//x`）**仍从文档根**起走——这是 §2.5 的定义（`/` 指向根），
/// 与 CSS 的"子树内查"不同。需要"仅在本子树内找"的规则应写相对路径
/// （`descendant::x` 或 `.//x`）。
pub fn evaluate_compiled_in(
    root: &NodeRef,
    compiled: &CompiledExpression,
) -> Result<XPathValue, XPathError> {
    let document = root.document().clone();
    let mut context = Context::new(&document, root.clone());
    evaluate_expression(&compiled.expression, &mut context)
}

/// 求值任意表达式（§3 的 Expr 递归）。
fn evaluate_expression(
    expression: &Expr,
    context: &mut Context<'_>,
) -> Result<XPathValue, XPathError> {
    match expression {
        Expr::Literal(Value::Number(value)) => Ok(XPathValue::Number(*value)),
        Expr::Literal(Value::String(value)) => Ok(XPathValue::String(value.clone())),
        Expr::FunctionCall { name, arguments } => {
            super::func::call(name, arguments, context, evaluate_expression)
        }
        Expr::Path { filter, path } => evaluate_path(filter.as_deref(), path, context),
        Expr::Negate(operand) => {
            // §3.5 的一元负号语义是 `0 - x`（见 `parse.rs` 的说明）。
            let value = evaluate_expression(operand, context)?;
            Ok(XPathValue::Number(-to_number(&value)))
        }
        Expr::Unary {
            operator,
            left,
            right,
        } => evaluate_binary(*operator, left, right, context),
        Expr::Union(left, right) => {
            let left_value = evaluate_expression(left, context)?;
            let right_value = evaluate_expression(right, context)?;
            let mut nodes = require_node_set(left_value, "`|` 的左操作数")?;
            nodes.extend(require_node_set(right_value, "`|` 的右操作数")?);
            // §3.5：并集的两个操作数必须是节点集；结果去重并按文档序
            // （`XPathValue::node_set` 承担这两件事）。
            Ok(XPathValue::node_set(nodes))
        }
    }
}

/// 求值一条路径表达式（§3.3）。
fn evaluate_path(
    filter: Option<&Expr>,
    path: &LocationPath,
    context: &mut Context<'_>,
) -> Result<XPathValue, XPathError> {
    // 初值节点集 + 属性轴标记。
    let mut current: NodeSet = match filter {
        // 过滤表达式形态：初值是它求出的节点集
        Some(filter) => {
            let value = evaluate_expression(filter, context)?;
            NodeSet::nodes(require_node_set(value, "路径的过滤表达式")?)
        }
        // 绝对路径：初值只含根节点（§2.4：路径以 `/` 起头时是"从根出发"）
        None if path.absolute => NodeSet::nodes(vec![context.document.root()]),
        // 相对路径：初值是上下文节点
        None => NodeSet::nodes(vec![context.node.clone()]),
    };

    for step in &path.steps {
        current = evaluate_step(step, current, context)?;
    }

    // 末相步是扩展节点测试时，结果是计算值序列（`allText()` 等）。
    // 用 `is_extension` 标记而不是"values 非空"——空节点集上求值
    // （`//nonexistent/allText()`）也应产出空序列而非退化成节点集。
    if current.extension {
        return Ok(XPathValue::Sequence(current.values));
    }

    // 末相步是属性轴时，结果是"属性值"而非节点——把值直接算出来
    // （依据见 `NodeSet::attribute` 的文档）。
    if let Some(attribute) = current.attribute.as_deref() {
        let values = current
            .nodes
            .iter()
            .filter_map(|node| node.attr(attribute))
            .map(XPathValue::String)
            .collect();
        return Ok(XPathValue::Sequence(values));
    }
    Ok(XPathValue::node_set(current.nodes))
}

/// 一相步的产出：节点序列 + 可选的"这些是属性"标记。
///
/// ## 为什么需要这个标记
///
/// 本项目的 DOM 里属性不是独立节点（见 [`axis_nodes`] 的文档），
/// 属性结果用"宿主元素"表示。而 `//a/@href` 必须产出**属性值**
/// `/b/1` 而不是元素文本，故"这组结果代表的是元素的哪个属性"必须
/// 跟着结果一起流动。用 `Option<String>` 而不是 `bool`：
/// 它同时给出了取值时要读的属性名（`@*` 时为 `None`，表示"全部属性"）。
#[derive(Debug, Clone)]
struct NodeSet {
    /// 节点序列（已按文档序归一化）
    nodes: Vec<NodeRef>,
    /// 属性名。`Some(name)` 表示这组结果代表元素的 `name` 属性；
    /// `Some("*")` 表示代表元素的全部属性；`None` 表示就是元素本身。
    attribute: Option<String>,
    /// 计算值序列（扩展节点测试 `allText()` 等的产出）。
    values: Vec<XPathValue>,
    /// 本结果是否来自扩展节点测试。
    ///
    /// 单列一个布尔而不靠 `values.is_empty()` 判断：空输入下
    /// （`//nonexistent/allText()`）值序列也是空的，但结果仍是"值序列"
    /// 而非"节点集"——两者在顶层会被转成同一种输出，但语义不同。
    extension: bool,
}

impl NodeSet {
    /// 由普通节点序列构造（非属性结果）。
    fn nodes(nodes: Vec<NodeRef>) -> Self {
        Self {
            nodes,
            attribute: None,
            values: Vec::new(),
            extension: false,
        }
    }

    /// 由计算值序列构造（扩展节点测试的产出）。
    fn values(values: Vec<XPathValue>) -> Self {
        Self {
            nodes: Vec::new(),
            attribute: None,
            values,
            extension: true,
        }
    }
}

/// 求值一相步：轴遍历 → 节点测试 → 谓词。
///
/// ## 顺序为什么不能颠倒
///
/// §3.3 的语义是"先按轴取候选节点，再按节点测试筛，再按谓词筛"：
///
/// > A step ... is evaluated with respect to a context node as follows:
/// > the axis is applied ..., then the node test is applied ..., then
/// > the predicates are applied.
///
/// 谓词里的 `position()` 数的是**已经过硬筛之后的**列表位置
/// （`div[1]` 是"第一个 div"而不是"第一个子节点且它是 div"）。
/// 把节点测试与谓词换个顺序，`div[1]` 会变成"若第一个子节点是 div 则取它"——
/// 这是本函数最容易写错的地方。
///
/// ## 谓词位置与文档序
///
/// 谓词必须按**轴方向**编号（§2.4 的 proximity position），而每相步的产出
/// 要按**文档序**归一化（节点集是无序集合）。故本函数按三段走：
/// 轴遍历（已是轴方向序）→ 逐谓词过滤（用轴方向位置）→ 最后按文档序排序。
///
/// 反向轴的顺序差异有规范原文（§2.2）：
///
/// > ... the proximity position of a node in a node-set is defined to be the
/// > position of the node in the node-set **in the axis direction**.
fn evaluate_step(
    step: &super::ast::Step,
    input: NodeSet,
    context: &mut Context<'_>,
) -> Result<NodeSet, XPathError> {
    // 属性轴单独走一条路径：它的候选**不是**树上的节点，而是元素上的属性。
    // 见 [`axis_nodes`] 与 [`NodeSet::attribute`] 的文档。
    if step.axis == Axis::Attribute {
        return evaluate_attribute_step(step, &input, context);
    }

    // 扩展节点测试（`allText()` 等）同样单独走一条路径：它们产出的不是
    // 文档中的节点，而是"对**上下文节点**求出的值"（依据见 `NodeTest`
    // 各变体的文档与 [`extension_value`]）。
    //
    // 这一步**不再走轴**：`/div/allText()` 的语义是"取每个 div 的全文"，
    // 即值作用于匹配到的 div 本身，而不是 div 的子节点。若照轴走，
    // `child::allText()` 会把三个子元素各转成一个值，得到
    // `["一","二","三"]` 而非 `["一二三"]`。
    if step.node_test.step_result() == super::ast::StepResult::AllText {
        return evaluate_extension_step(step, &input, context);
    }

    let mut result = Vec::new();
    for node in &input.nodes {
        let mut candidates = axis_nodes(step.axis, node, context)?;
        // §2.4：谓词里的 `position()` 数的是"轴方向上的第几个"，
        // 而反向轴的遍历本身就是反的，故这里**不**排序。
        candidates.retain(|candidate| matches_node_test(&step.node_test, candidate));

        for predicate in &step.predicates {
            candidates = apply_predicate(&predicate.expression, candidates, context)?;
        }

        result.extend(candidates);
    }

    // 每相步结束后按文档序归一化（并在 `node_set` 里统一去重）。
    // 依据 §3.3：节点集是无序集合，但"求值产出"要有确定顺序——
    // 见 `value::sort_document_order` 的说明。
    result.sort_by(compare_document_order);
    result.dedup_by(|left, right| same_node(left, right));
    Ok(NodeSet::nodes(result))
}

/// 求值一相**扩展节点测试**步（`allText()` / `html()` / `outerHtml()` / `num()`）。
///
/// ## 为什么这些不走轴
///
/// XPath 1.0 §2.3 的四种节点类型测试与名字测试筛的都是"文档中的节点"，
/// 故它们天然与轴配合（`child::text()` 取子文本节点）。而书源生态的扩展
/// 节点测试产出的是**计算值**（一段文本、一段 HTML、一个数字），
/// 它在文档里没有对应节点，也**没有子节点可继续遍历**。
///
/// 依据来源：书源规则的公开语法约定把 `allText` / `html` 等列在"获取内容"
/// 清单里，书写形态是 `/div/allText()`——语义是"对匹配到的 div 取全文"，
/// 即值作用于**匹配到的节点自身**。故这里对输入节点集逐个取值，不再走轴。
///
/// 谓词仍然可用（`/div[1]/allText()` 的谓词在前一相解析，不在这里），
/// 而形如 `/div/allText()[1]` 的谓词作用在**值序列**上——按位置筛选值。
/// \[UNCERTAIN] 值序列上的谓词是生态语法里没有明确定义的形态，
/// 这里按"与节点序列同一套位置语义"处理，是最一致的选择。
fn evaluate_extension_step(
    step: &super::ast::Step,
    input: &NodeSet,
    context: &mut Context<'_>,
) -> Result<NodeSet, XPathError> {
    let mut values: Vec<XPathValue> = Vec::new();
    for node in &input.nodes {
        if let Some(value) = extension_value(&step.node_test, node) {
            values.push(value);
        }
    }

    // 值序列上的谓词：位置语义与节点序列一致（1 起，按收集顺序）
    for predicate in &step.predicates {
        values = apply_predicate_to_values(&predicate.expression, values, context)?;
    }
    Ok(NodeSet::values(values))
}

/// 按扩展节点测试从节点取值。
///
/// 取值语义来自 Jsoup 公开文档（`Element#text()` / `Element#html()` /
/// `Node#outerHtml()`），与 [`crate::css_post::RuleSuffix`] 的同名后缀
/// **共用同一批语义**，故两个引擎的输出不会分叉。
fn extension_value(node_test: &NodeTest, node: &NodeRef) -> Option<XPathValue> {
    Some(match node_test {
        NodeTest::AllText => XPathValue::String(node.text()),
        NodeTest::Html => XPathValue::String(inner_html(node)),
        NodeTest::OuterHtml => XPathValue::String(outer_html(node)),
        NodeTest::Number => XPathValue::Number(extract_first_number(&node.text())),
        _ => return None,
    })
}

/// 元素的**内部** HTML（Jsoup `Element#html()`）。
///
/// 复用 [`crate::css_post`] 的序列化（那里已实现且经深嵌套回归测试），
/// 避免同一件事写两份。
fn inner_html(node: &NodeRef) -> String {
    let mut buffer = String::new();
    for child in node.children() {
        crate::css_post::serialize_node(&child, &mut buffer);
    }
    buffer
}

/// 元素及子树的 HTML（Jsoup `Node#outerHtml()`）。
fn outer_html(node: &NodeRef) -> String {
    let mut buffer = String::new();
    crate::css_post::serialize_node(node, &mut buffer);
    buffer
}

/// 从文本里抽出**第一个数字**（扩展节点测试 `num()`）。
///
/// \[UNCERTAIN] 依据来源与取舍：书源规则的公开语法约定把 `num` 列在"获取内容"
/// 清单里，其用途是"从形如 `评分 8.9 分` 的文本里取 `8.9`"。规范原文没有
/// 给出精确的抽取规则，故本实现取最窄的可行解释：扫描文本，取第一个
/// 由"可选符号 + 数字/小数点"构成的最长连续片段。抽不到数字时返回 `NaN`
/// （与 XPath 的 `number()` 转换一致）。若有语料显示别的抽取范围更常见，
/// 此处需复核。
fn extract_first_number(text: &str) -> f64 {
    let mut digits = String::new();
    let mut started = false;
    for character in text.chars() {
        let is_numeric = character.is_ascii_digit() || character == '.';
        if is_numeric {
            started = true;
            digits.push(character);
            continue;
        }
        // 允许负号仅在数字片段**尚未开始**时
        if !started && character == '-' {
            started = true;
            digits.push(character);
            continue;
        }
        if started && !digits.is_empty() {
            break;
        }
    }
    digits.parse::<f64>().unwrap_or(f64::NAN)
}

/// 对**值序列**施加谓词（位置语义与节点序列一致）。
fn apply_predicate_to_values(
    expression: &Expr,
    values: Vec<XPathValue>,
    context: &mut Context<'_>,
) -> Result<Vec<XPathValue>, XPathError> {
    let size = values.len();
    let mut kept = Vec::new();
    for (index, value) in values.into_iter().enumerate() {
        context.count_steps(1)?;

        // 值序列没有节点上下文，故以当前上下文节点作为谓词求值的节点上下文
        let saved = (context.position, context.size);
        context.position = index + 1;
        context.size = size;

        // 谓词里可以直接引用当前值（如 `[. > 3]`），故暂时把它放进上下文
        let saved_value = context.current_value.clone();
        context.current_value = Some(value.clone());

        let result = evaluate_expression(expression, context);

        context.position = saved.0;
        context.size = saved.1;
        context.current_value = saved_value;

        if predicate_keeps(&result?, index) {
            kept.push(value);
        }
    }
    Ok(kept)
}

/// 求值一相**属性轴**步。
///
/// 属性轴的结果不是树上的节点，而是"元素 + 属性名"。故这里：
///
/// 1. 把宿主元素的属性展开成候选（每个候选 = 一个 `(元素, 属性名)` 对）
/// 2. 按节点测试筛（名字测试比对属性名，通配符全收）
/// 3. 谓词按**属性顺序**定位（DOM 保序，见 [`crate::dom`] 的 `attributes`）
/// 4. 产出"宿主元素序列 + 属性名"
///
/// ## 谓词里的 `position()` 与 `last()`
///
/// 谓词求值需要一个上下文**节点**，而属性不是节点。取它的宿主元素作上下文
/// 节点——这是 XPath 1.0 下最接近的解释（属性节点的"父"就是宿主元素）。
/// 故 `@*[last()]` 能正确得到最后一个属性。
///
/// \[UNCERTAIN] 谓词里若写 `self::node()`，得到的是宿主元素而非属性节点。
/// 这是"属性不是独立节点"这一取舍无法避免的后果，已在
/// `mod.rs` 的「已知边界」里记录。
fn evaluate_attribute_step(
    step: &super::ast::Step,
    input: &NodeSet,
    context: &mut Context<'_>,
) -> Result<NodeSet, XPathError> {
    let mut hosts = Vec::new();
    let mut selected_names: Vec<String> = Vec::new();
    // 逐宿主元素收集它匹配的属性名。用 `Vec<Vec<String>>` 而非
    // `HashMap`：属性本身保序（DOM 的 `attrs` 是 `Vec`），
    // 而 `@*[1]` 的"第几个"必须有确定答案。
    for node in &input.nodes {
        if !node.is_element() {
            continue;
        }
        let mut names: Vec<String> = node
            .attributes()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| matches_attribute_test(&step.node_test, name))
            .collect();

        // 谓词：逐个属性求值，位置按属性顺序（1 起）
        if !step.predicates.is_empty() {
            names = filter_attributes_by_predicates(&names, &step.predicates, node, context)?;
        }

        if !names.is_empty() {
            hosts.push(node.clone());
            selected_names.extend(names);
        }
    }

    // 产出"宿主元素 + 属性名"。属性名的选取规则：
    // - `@name`：名字测试已把列表收窄到该名字，取值时读它
    // - `@*`：全部属性，取值时把每个元素的全部属性值依次给出
    //
    // 表达方式取"宿主序列 + 单个属性名"，而 `@*` 的情形由
    // `attribute = Some("*")` 表示——取值层据此展开。
    let attribute = match &step.node_test {
        NodeTest::Name { local, .. } => Some(local.clone()),
        NodeTest::Wildcard => Some("*".to_string()),
        // 非名字/通配符的测试用在属性轴上没有意义（属性没有 `text()`），
        // 按"无属性"处理
        _ => None,
    };

    Ok(NodeSet {
        nodes: hosts,
        attribute,
        values: Vec::new(),
        extension: false,
    })
}

/// 属性名是否匹配节点测试。
fn matches_attribute_test(node_test: &NodeTest, name: &str) -> bool {
    match node_test {
        NodeTest::Wildcard => true,
        NodeTest::Name { local, .. } => name == local,
        // §2.3 的节点类型测试在属性轴上不成立（属性不是 text/comment）
        _ => false,
    }
}

/// 对属性名列表施加谓词。
///
/// 上下文节点取宿主元素（属性不是节点，见 [`evaluate_attribute_step`] 的文档）。
fn filter_attributes_by_predicates(
    names: &[String],
    predicates: &[super::ast::Predicate],
    host: &NodeRef,
    context: &mut Context<'_>,
) -> Result<Vec<String>, XPathError> {
    let mut kept: Vec<String> = names.to_vec();
    for predicate in predicates {
        let size = kept.len();
        let mut next = Vec::new();
        for (index, name) in kept.iter().enumerate() {
            context.count_steps(1)?;
            // 属性谓词里要能取到属性值，故把上下文节点设成宿主元素，
            // 并把当前属性名暂存进上下文——`@*[@x=...]` 的写法因此可用。
            let saved = (
                context.node.clone(),
                context.position,
                context.size,
                context.current_attribute.clone(),
            );
            context.node = host.clone();
            context.position = index + 1;
            context.size = size;
            context.current_attribute = Some(name.clone());

            let value = evaluate_expression(&predicate.expression, context);

            context.node = saved.0;
            context.position = saved.1;
            context.size = saved.2;
            context.current_attribute = saved.3;

            if predicate_keeps(&value?, index) {
                next.push(name.clone());
            }
        }
        kept = next;
    }
    Ok(kept)
}

/// 按轴取候选节点。
///
/// 返回顺序**就是轴方向序**（正向轴是文档序，反向轴是逆文档序），
/// 供 [`evaluate_step`] 的谓词定位使用。
///
/// ## 属性轴的结果是**宿主元素**
///
/// 本项目的 DOM 把属性存在元素内（[`crate::dom`] 的 `ElementData::attrs`），
/// 属性不是 arena 里的独立节点。而 XPath 的 `@x` 在语义上是"一组属性节点"，
/// 其字符串值是属性值。实现取"宿主元素 + 属性名"表示：
///
/// - **筛选**：`@href` 筛出"**有** `href` 属性的元素"，`@*` 筛出所有元素
/// - **取值**：由 [`value::string_value`] 的 `attribute_name_of` 上下文给出
///
/// 因为这个原因，一相步的产出必须能携带"这是哪个属性"的信息，
/// 见 [`NodeSet::attribute`]。
///
/// \[UNCERTAIN] 代价是无法表达"属性节点本身的节点同一性"，
/// 即 `//a/@href | //a` 无法区分两类结果——见 `mod.rs` 的「已知边界」。
fn axis_nodes(
    axis: Axis,
    node: &NodeRef,
    context: &mut Context<'_>,
) -> Result<Vec<NodeRef>, XPathError> {
    Ok(match axis {
        Axis::Child => node.children().collect(),
        Axis::Descendant => descendants_of(node),
        Axis::DescendantOrSelf => {
            let mut nodes = vec![node.clone()];
            nodes.extend(descendants_of(node));
            nodes
        }
        Axis::Parent => node.parent().into_iter().collect(),
        Axis::Ancestor => ancestors_of(node, false),
        Axis::AncestorOrSelf => ancestors_of(node, true),
        Axis::FollowingSibling => following_siblings_of(node),
        Axis::PrecedingSibling => preceding_siblings_of(node),
        Axis::Following => following_of(node),
        Axis::Preceding => preceding_of(node),
        Axis::SelfAxis => vec![node.clone()],
        // 属性轴不会走到这里——[`evaluate_step`] 在进入本函数前已把它
        // 分派给 [`evaluate_attribute_step`]（属性不是树上的节点）。
        // 保留空结果而非 `unreachable!()`：本项目禁止生产路径 panic。
        Axis::Attribute => Vec::new(),
        // 命名空间轴：HTML DOM 没有命名空间节点（前缀声明表），
        // 故恒为空。**这不是"没实现"**——依据 §5.4，命名空间节点只在
        // 有命名空间声明的 XML 文档里存在，而本项目的输入是 HTML。
        // 返回空集是规范下的正确结果，不是错误。
        Axis::Namespace => {
            context.count_steps(1)?;
            Vec::new()
        }
    })
}

/// 后代节点（不含自身），**文档序**。
///
/// 用迭代器的下降序即可，但 `descendants_and_self` 含自身，故跳过首项。
fn descendants_of(node: &NodeRef) -> Vec<NodeRef> {
    node.descendants_and_self().skip(1).collect()
}

/// 祖先（不含自身，除非 `include_self`），按 §2.2 的**反向轴**语义
/// 由近及远返回。
///
/// 顺序即轴方向序：`ancestor::*[1]` 必须是**最近**的祖先（§2.2 把
/// ancestor / ancestor-or-self / preceding / preceding-sibling 列为反向轴），
/// 故这里从父节点起往根走，不反转。
fn ancestors_of(node: &NodeRef, include_self: bool) -> Vec<NodeRef> {
    let mut nodes = Vec::new();
    let mut cursor = if include_self {
        Some(node.clone())
    } else {
        node.parent()
    };
    while let Some(current) = cursor {
        cursor = current.parent();
        nodes.push(current);
    }
    nodes
}

/// 后续兄弟，**文档序**（正向轴）。
fn following_siblings_of(node: &NodeRef) -> Vec<NodeRef> {
    let mut nodes = Vec::new();
    let mut cursor = node.next_sibling();
    while let Some(current) = cursor {
        cursor = current.next_sibling();
        nodes.push(current);
    }
    nodes
}

/// 前序兄弟，由近及远（§2.2 的**反向轴**）。
fn preceding_siblings_of(node: &NodeRef) -> Vec<NodeRef> {
    let mut nodes = Vec::new();
    let mut cursor = node.prev_sibling();
    while let Some(current) = cursor {
        cursor = current.prev_sibling();
        nodes.push(current);
    }
    nodes
}

/// `following` 轴：文档序上在本节点**之后**、且不是其后代的所有节点。
///
/// 依据 §2.2：
///
/// > the following axis contains all nodes in the same document as the context
/// > node that are after the context node in document order, excluding any
/// > descendants and excluding attribute nodes and namespace nodes
///
/// 实现取"从根做一次先序遍历，取排在上下文节点之后的部分"——先序里
/// 上下文节点的**后代紧跟其后**，故要显式排除。
fn following_of(node: &NodeRef) -> Vec<NodeRef> {
    let descendants: Vec<usize> = descendants_of(node)
        .into_iter()
        .map(|child| child.id())
        .collect();

    let mut nodes = Vec::new();
    let mut seen = false;
    for candidate in node.document().descendants() {
        if !seen {
            seen = same_node(&candidate, node);
            continue;
        }
        if descendants.contains(&candidate.id()) {
            continue;
        }
        nodes.push(candidate);
    }
    nodes
}

/// `preceding` 轴：文档序上在本节点**之前**、且不是其祖先的所有节点。
///
/// 依据 §2.2：
///
/// > the preceding axis contains all nodes in the same document as the context
/// > node that are before the context node in document order, excluding any
/// > ancestors and excluding attribute nodes and namespace nodes
///
/// 顺序是**逆文档序**（§2.2 把 preceding 列为反向轴），故结果在收集后反转。
fn preceding_of(node: &NodeRef) -> Vec<NodeRef> {
    let ancestors: Vec<usize> = ancestors_of(node, true)
        .into_iter()
        .map(|ancestor| ancestor.id())
        .collect();

    let mut nodes = Vec::new();
    for candidate in node.document().descendants() {
        if same_node(&candidate, node) {
            break;
        }
        if ancestors.contains(&candidate.id()) {
            continue;
        }
        nodes.push(candidate);
    }
    // 反向轴 → 由近及远
    nodes.reverse();
    nodes
}

/// 节点测试筛选（§2.3）。
fn matches_node_test(node_test: &NodeTest, node: &NodeRef) -> bool {
    use crate::dom::NodeKind;

    match node_test {
        NodeTest::AnyNode => true,
        NodeTest::Text => matches!(node.kind(), NodeKind::Text | NodeKind::CData),
        NodeTest::Comment => node.kind() == NodeKind::Comment,
        NodeTest::ProcessingInstruction => node.kind() == NodeKind::ProcessingInstruction,
        NodeTest::Wildcard => node.is_element(),
        NodeTest::Name { prefix, local } => {
            if !node.is_element() {
                return false;
            }
            if node.node_name() != *local {
                return false;
            }
            // [UNCERTAIN] 带前缀的名字测试按§2.3 的 QName 语义应比对
            // "前缀在当前命名空间声明下展开的 URI"。HTML DOM 没有前缀声明表，
            // 故退化为"本地名匹配 + 前缀只用于诊断"。对书源抓取的 HTML
            // 页面这不影响可达性（页面里没有前缀声明），但**会**让
            // `<svg:rect>` 与 `<rect>` 无法区分。
            let _ = prefix;
            true
        }
        // 扩展节点测试的筛选语义见 [`extension_value`]：它们对**任意元素**
        // 都成立（值可能为空串），故筛选一律通过。
        NodeTest::AllText | NodeTest::Html | NodeTest::OuterHtml | NodeTest::Number => {
            node.is_element() || node.parent().is_some()
        }
    }
}

/// 施加一条谓词（§2.4）。
///
/// > A predicate filters a node-set with respect to an axis to produce a new
/// > node-set. For each node in the node-set to be filtered, the PredicateExpr
/// > is evaluated with that node as the context node, with the number of nodes
/// > in the node-set as the context size, and with the proximity position of
/// > the node in the node-set with respect to the axis as the context position
///
/// 即：逐个候选节点求值，结果按 §2.4 的规则决定去留——
/// **数字结果按位置比较，其余类型按布尔转换**。
fn apply_predicate(
    expression: &Expr,
    candidates: Vec<NodeRef>,
    context: &mut Context<'_>,
) -> Result<Vec<NodeRef>, XPathError> {
    let size = candidates.len();
    let mut kept = Vec::new();

    for (index, candidate) in candidates.into_iter().enumerate() {
        context.count_steps(1)?;

        // 保存并替换上下文（求值完必须还原——否则外层谓词的位置语义被破坏）。
        let saved = (context.node.clone(), context.position, context.size);
        context.node = candidate.clone();
        context.position = index + 1;
        context.size = size;

        let value = evaluate_expression(expression, context);

        context.node = saved.0;
        context.position = saved.1;
        context.size = saved.2;

        if predicate_keeps(&value?, index) {
            kept.push(candidate);
        }
    }
    Ok(kept)
}

/// 谓词结果是否保留该节点。
///
/// 依据 §2.4：
///
/// > If the result is a number, the result will be converted to true if the
/// > number is equal to the context position and will be converted to false
/// > otherwise. If the result is not a number, the result will be converted as
/// > if by a call to the boolean function.
///
/// `index` 是 **0 起**的循环下标，故与位置（1 起）比较时加一。
fn predicate_keeps(value: &XPathValue, index: usize) -> bool {
    match value {
        XPathValue::Number(number) => *number == (index + 1) as f64,
        other => to_boolean(other),
    }
}

/// 析取节点集，非节点集时报类型错。
fn require_node_set(value: XPathValue, what: &str) -> Result<Vec<NodeRef>, XPathError> {
    match value {
        XPathValue::NodeSet(nodes) => Ok(nodes),
        other => Err(XPathError::node_set_required(format!(
            "{what}应为节点集，实际是 {other}"
        ))),
    }
}

/// 二元运算（§3.4 Comparison / §3.5 Arithmetic）。
fn evaluate_binary(
    operator: BinaryOperator,
    left: &Expr,
    right: &Expr,
    context: &mut Context<'_>,
) -> Result<XPathValue, XPathError> {
    use BinaryOperator as Op;

    // `and` / `or` 有**短路**语义，故先单独处理再求右操作数。
    match operator {
        Op::And => {
            let left_value = evaluate_expression(left, context)?;
            if !to_boolean(&left_value) {
                return Ok(XPathValue::Boolean(false));
            }
            let right_value = evaluate_expression(right, context)?;
            return Ok(XPathValue::Boolean(to_boolean(&right_value)));
        }
        Op::Or => {
            let left_value = evaluate_expression(left, context)?;
            if to_boolean(&left_value) {
                return Ok(XPathValue::Boolean(true));
            }
            let right_value = evaluate_expression(right, context)?;
            return Ok(XPathValue::Boolean(to_boolean(&right_value)));
        }
        _ => {}
    }

    let left_value = evaluate_expression(left, context)?;
    let right_value = evaluate_expression(right, context)?;

    match operator {
        Op::Equal => Ok(XPathValue::Boolean(compare_values(
            &left_value,
            &right_value,
            |ordering| ordering.is_eq(),
        ))),
        Op::NotEqual => Ok(XPathValue::Boolean(compare_values(
            &left_value,
            &right_value,
            |ordering| ordering.is_ne(),
        ))),
        Op::Less => Ok(XPathValue::Boolean(compare_values(
            &left_value,
            &right_value,
            |ordering| ordering.is_lt(),
        ))),
        Op::LessOrEqual => Ok(XPathValue::Boolean(compare_values(
            &left_value,
            &right_value,
            |ordering| ordering.is_le(),
        ))),
        Op::Greater => Ok(XPathValue::Boolean(compare_values(
            &left_value,
            &right_value,
            |ordering| ordering.is_gt(),
        ))),
        Op::GreaterOrEqual => Ok(XPathValue::Boolean(compare_values(
            &left_value,
            &right_value,
            |ordering| ordering.is_ge(),
        ))),
        Op::Add => Ok(XPathValue::Number(
            to_number(&left_value) + to_number(&right_value),
        )),
        Op::Subtract => Ok(XPathValue::Number(
            to_number(&left_value) - to_number(&right_value),
        )),
        Op::Multiply => Ok(XPathValue::Number(
            to_number(&left_value) * to_number(&right_value),
        )),
        Op::Divide => Ok(XPathValue::Number(
            to_number(&left_value) / to_number(&right_value),
        )),
        Op::Modulo => Ok(XPathValue::Number(
            to_number(&left_value) % to_number(&right_value),
        )),
        // and/or 已在上面短路处理
        Op::And | Op::Or => Ok(XPathValue::Boolean(false)),
    }
}

/// XPath 1.0 的比较结果。
///
/// \[UNCERTAIN] XPath 1.0 的比较是**三态**的（真/假/不可比，见 §3.4 的
/// "if both objects to be compared are ..."整段），而这里的 `bool` 把
/// "不可比"与"假"合并了。对书源规则的实际影响：`@x > 3` 在 `x` 非数字时
/// 得到 `false`（规范也是 `false`——规范说不可比时"the result is false"，
/// 故这里没有语义偏差，只是内部表示少了一态）。保留 `bool` 是因为
/// 没有调用方需要区分二者。
fn compare_values(
    left: &XPathValue,
    right: &XPathValue,
    predicate: impl Fn(std::cmp::Ordering) -> bool,
) -> bool {
    use XPathValue as V;

    match (left, right) {
        // 节点集与节点集的比较（§3.4）：存在一对节点使比较成立即为真
        (V::NodeSet(left_nodes), V::NodeSet(right_nodes)) => left_nodes.iter().any(|left_node| {
            right_nodes
                .iter()
                .any(|right_node| predicate(string_value(left_node).cmp(&string_value(right_node))))
        }),
        // 节点集与标量：节点集里**任一**节点的字符串值使比较成立即为真
        (V::NodeSet(nodes), scalar) => nodes
            .iter()
            .any(|node| predicate(string_value(node).cmp(&to_string(scalar)))),
        (scalar, V::NodeSet(nodes)) => nodes
            .iter()
            .any(|node| predicate(to_string(scalar).cmp(&string_value(node)))),
        // 两个布尔：转成布尔比较（§3.4："if at least one object to be compared
        // is a boolean, then each object to be compared is converted to a
        // boolean as if by applying the boolean function"）
        (V::Boolean(_), _) | (_, V::Boolean(_)) => {
            predication_to_ordering(to_boolean(left), to_boolean(right), predicate)
        }
        // 两个数字（或"数字与字符串"）：按数字比较
        (V::Number(_), _) | (_, V::Number(_)) => {
            predication_to_ordering_f64(to_number(left), to_number(right), predicate)
        }
        // 其余（字符串与字符串）：按字符串比较
        (left_value, right_value) => predicate(to_string(left_value).cmp(&to_string(right_value))),
    }
}

/// 把布尔的"比较谓词"转成 `Ordering` 上的谓词。
fn predication_to_ordering(
    left: bool,
    right: bool,
    predicate: impl Fn(std::cmp::Ordering) -> bool,
) -> bool {
    let ordering = match (left, right) {
        (true, true) | (false, false) => std::cmp::Ordering::Equal,
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
    };
    predicate(ordering)
}

/// 把数字比较转成 `Ordering` 上的谓词。
///
/// `partial_cmp` 对 `NaN` 返回 `None`——依据 §3.4，与 NaN 的比较
/// "will be false"（规范原文："if either object ... is NaN the result is false"），
/// 故 `None` 一律判假，**不**当作 `Equal`。用 `then(false)` 显式表达这一点。
fn predication_to_ordering_f64(
    left: f64,
    right: f64,
    predicate: impl Fn(std::cmp::Ordering) -> bool,
) -> bool {
    left.partial_cmp(&right).is_some_and(predicate)
}

// ---------------------------------------------------------------------------
// XPath 1.0 §3.4 的类型转换
// ---------------------------------------------------------------------------

/// `boolean()` 转换（§3.4 "Boolean Functions" / §4.3）。
///
/// 规范原文：
///
/// > a number is true if and only if it is neither positive or negative zero
/// > nor NaN; ... a node-set is true if and only if it is non-empty;
/// > a string is true if and only if its length is non-zero
pub fn to_boolean(value: &XPathValue) -> bool {
    match value {
        XPathValue::Boolean(value) => *value,
        XPathValue::Number(value) => !(value.is_nan() || *value == 0.0),
        XPathValue::String(value) => !value.is_empty(),
        XPathValue::NodeSet(nodes) => !nodes.is_empty(),
        // 序列按"非空即真"处理（与节点集同一取向）
        XPathValue::Sequence(values) => !values.is_empty(),
    }
}

/// `number()` 转换（§3.4 / §4.4）。
///
/// 字符串转换按规范的 String→Number 规则：去掉首尾空白后按十进制浮点解析，
/// **无法解析时结果是 NaN**（§4.4 的 `number` 函数词条）。
pub fn to_number(value: &XPathValue) -> f64 {
    match value {
        XPathValue::Number(value) => *value,
        XPathValue::Boolean(value) => {
            if *value {
                1.0
            } else {
                0.0
            }
        }
        XPathValue::String(text) => parse_xpath_number(text),
        // §3.4：节点集先转字符串（取文档序首个节点的字符串值），再转数字
        XPathValue::NodeSet(nodes) => match nodes.first() {
            Some(node) => parse_xpath_number(&string_value(node)),
            None => f64::NAN,
        },
        XPathValue::Sequence(values) => match values.first() {
            Some(value) => to_number(value),
            None => f64::NAN,
        },
    }
}

/// `string()` 转换（§3.4 / §4.2）。
pub fn to_string(value: &XPathValue) -> String {
    match value {
        XPathValue::String(text) => text.clone(),
        XPathValue::Boolean(true) => "true".to_string(),
        XPathValue::Boolean(false) => "false".to_string(),
        XPathValue::Number(value) => format_number(*value),
        XPathValue::NodeSet(nodes) => nodes.first().map(string_value).unwrap_or_default(),
        XPathValue::Sequence(values) => values.first().map(to_string).unwrap_or_default(),
    }
}

/// 把字符串解析成 XPath 数字（§4.4 的 String→Number 规则）。
///
/// > a string that consists of optional whitespace followed by an optional
/// > minus sign followed by a Number followed by whitespace is converted to
/// > the IEEE 754 number ... any other string is converted to NaN
fn parse_xpath_number(text: &str) -> f64 {
    let trimmed = text.trim();
    // 空串与 `-` 都不是数字（`"".parse::<f64>()` 会失败，但显式判更清楚）
    if trimmed.is_empty() || trimmed == "-" {
        return f64::NAN;
    }
    trimmed.parse::<f64>().unwrap_or(f64::NAN)
}

/// 数字转字符串（§4.2 的 `string` 对数字的规则）。
///
/// 规范要求：整数不写小数点，NaN 写作 `NaN`，正负零写作 `0`，
/// 无穷写作 `Infinity` / `-Infinity`。
pub fn format_number(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    // `== 0.0` 同时覆盖 +0 与 -0（§4.2 要求 `-0` 输出 `0`）
    if value == 0.0 {
        return "0".to_string();
    }
    // 整数值不写小数点（§4.2："the number is converted to a string ...
    // there is no decimal point if the number is an integer"）
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    format!("{value}")
}

/// 走一步谓词预算。
impl Context<'_> {
    /// 消耗步数，超上限报 `StepLimit`。
    fn count_steps(&mut self, steps: u64) -> Result<(), XPathError> {
        self.steps = self.steps.saturating_add(steps);
        if self.steps > MAX_PREDICATE_STEPS {
            return Err(XPathError::limit(
                XPathErrorCode::StepLimit,
                format!("谓词求值超过 {MAX_PREDICATE_STEPS} 步"),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xpath::parse::compile;

    /// 便捷：在文档上求值一条表达式。
    fn eval(document: &Document, expression: &str) -> XPathValue {
        let compiled = compile(expression).expect("应解析成功");
        evaluate_compiled(document, &compiled).expect("应求值成功")
    }

    /// 便捷：求值并取节点集的节点名序列。
    fn names(document: &Document, expression: &str) -> Vec<String> {
        match eval(document, expression) {
            XPathValue::NodeSet(nodes) => nodes.into_iter().map(|node| node.node_name()).collect(),
            other => panic!("应为节点集，实际 {other:?}"),
        }
    }

    /// 便捷：求值并取字符串序列（元素按 `text()` 展开）。
    fn strings(document: &Document, expression: &str) -> Vec<String> {
        match eval(document, expression) {
            XPathValue::NodeSet(nodes) => nodes
                .iter()
                .map(crate::xpath::value::string_value)
                .collect(),
            XPathValue::Sequence(values) => values.iter().map(super::to_string).collect(),
            XPathValue::String(text) => vec![text],
            other => panic!("应为节点集或字符串，实际 {other:?}"),
        }
    }

    /// 便捷：求值并取布尔。
    fn boolean(document: &Document, expression: &str) -> bool {
        match eval(document, expression) {
            XPathValue::Boolean(value) => value,
            other => panic!("应为布尔，实际 {other:?}"),
        }
    }

    /// 便捷：求值并取数字。
    fn number(document: &Document, expression: &str) -> f64 {
        match eval(document, expression) {
            XPathValue::Number(value) => value,
            other => panic!("应为数字，实际 {other:?}"),
        }
    }

    const CATALOG: &str = r#"<html><body>
        <div id="wrap">
          <ul class="books">
            <li class="book"><a href="/b/1">一</a><span class="author">甲</span></li>
            <li class="book"><a href="/b/2">二</a><span class="author">乙</span></li>
            <li class="book"><a href="/b/3">三</a><span class="author">丙</span></li>
          </ul>
          <p class="desc">说明</p>
        </div>
      </body></html>"#;

    // ---------- 轴 ----------

    #[test]
    fn 子轴与绝对路径() {
        let document = Document::parse_html(CATALOG);
        assert_eq!(
            names(&document, "/html/body/div/ul/li"),
            vec!["li", "li", "li"]
        );
        assert_eq!(strings(&document, "//li/a"), vec!["一", "二", "三"]);
        assert_eq!(number(&document, "count(//li)"), 3.0);
    }

    #[test]
    fn 后代轴与后代或自身轴() {
        let document = Document::parse_html(CATALOG);
        // `//li` 是 `descendant-or-self::node()` + `child::li` 两相步
        assert_eq!(names(&document, "//li"), vec!["li", "li", "li"]);
        // descendant 轴含全部后代元素
        assert!(names(&document, "//ul/descendant::a").len() == 3);
        // descendant-or-self 含自身
        assert_eq!(names(&document, "//ul/descendant-or-self::ul"), vec!["ul"]);
    }

    #[test]
    fn 父轴与自身轴() {
        let document = Document::parse_html(CATALOG);
        // `//a` 命中 3 个 a，各自取父 → 去重后是 3 个 li（三个不同的 li）
        assert_eq!(names(&document, "//a/.."), vec!["li", "li", "li"]);
        // 三个 a 分属三个不同的 li，故 parent::li 得到三个
        assert_eq!(names(&document, "//a/parent::li"), vec!["li", "li", "li"]);
        assert_eq!(names(&document, "//a/parent::span"), Vec::<String>::new());
        // self 轴
        assert_eq!(names(&document, "//ul/."), vec!["ul"]);
        assert_eq!(names(&document, "//ul/self::ul"), vec!["ul"]);
    }

    #[test]
    fn 祖先轴由近及远且谓词按轴方向数() {
        let document = Document::parse_html(CATALOG);
        // 取文档里**第一个** a：`/descendant::a[1]` 才是整棵树下第一个
        // （`//a[1]` 是"每个父节点下的第一个 a"，三个 a 都满足，见
        // `双斜杠下的谓词作用于每一相而非整条路径`）。
        // 它的祖先是 li → ul → div → body → html（反向轴：由近及远）
        // 结果按**文档序**（节点集是无序集合，产出取文档序；
        // 轴方向序只用于谓词定位，见 evaluate_step 的文档）
        assert_eq!(
            names(&document, "/descendant::a[1]/ancestor::*"),
            vec!["html", "body", "div", "ul", "li"],
            "ancestor 的结果按文档序（反向顺序只影响谓词定位）"
        );
        // §2.2（proximity position "in the axis direction"）：
        // 反向轴上 `[1]` 是**最近**的那个
        assert_eq!(
            names(&document, "/descendant::a[1]/ancestor::*[1]"),
            vec!["li"],
            "反向轴的 `[1]` 是最近祖先而非最远祖先"
        );
        // ancestor-or-self 含自身
        assert_eq!(
            names(&document, "/descendant::a[1]/ancestor-or-self::*[1]"),
            vec!["a"]
        );
    }

    #[test]
    fn 兄弟轴与反向兄弟轴() {
        let document = Document::parse_html(CATALOG);
        let second = "//li[2]";
        assert_eq!(
            names(&document, &format!("{second}/following-sibling::li")),
            vec!["li"]
        );
        // following-sibling 是**正向轴**
        assert_eq!(
            names(&document, &format!("{second}/following-sibling::li[1]")),
            vec!["li"]
        );
        // preceding-sibling 是**反向轴**：`[1]` 是最近的那个
        assert_eq!(
            names(&document, &format!("{second}/preceding-sibling::li")),
            vec!["li"]
        );
        assert_eq!(
            strings(&document, "/descendant::li[3]/preceding-sibling::li[1]/a"),
            vec!["二"],
            "反向兄弟轴的 `[1]` 是紧邻的前一个"
        );
    }

    #[test]
    fn 前序与后续轴() {
        let document = Document::parse_html(CATALOG);
        // `following` 不含后代，也不含祖先
        let following = names(&document, "//ul[1]/following::*");
        assert!(
            following.contains(&"p".to_string()),
            "ul 之后的 p 应在 following 里：{following:?}"
        );
        assert!(
            !following.contains(&"a".to_string()),
            "ul 的后代 a 不在 following 里：{following:?}"
        );

        // `preceding` 不含祖先
        let preceding = names(&document, "//p[1]/preceding::*");
        assert!(
            !preceding.contains(&"html".to_string()) && !preceding.contains(&"body".to_string()),
            "祖先不在 preceding 里：{preceding:?}"
        );
        assert!(
            preceding.contains(&"li".to_string()),
            "p 前的 li 应在 preceding 里：{preceding:?}"
        );
    }

    #[test]
    fn 属性轴() {
        let document = Document::parse_html(CATALOG);
        // `@href` 的结果是"带该属性的元素"，取值由 string_value 走属性分支
        let values = strings(&document, "//a/@href");
        assert_eq!(values, vec!["/b/1", "/b/2", "/b/3"]);
        // 属性存在性判定
        assert_eq!(names(&document, "//a[@href]").len(), 3);
        assert_eq!(names(&document, "//a[@nope]"), Vec::<String>::new());
        // 属性通配
        assert_eq!(number(&document, "count(//li[@class])"), 3.0);
    }

    #[test]
    fn 命名空间轴恒为空() {
        // HTML DOM 没有命名空间节点（§5.4 的命名空间节点只在有前缀声明的
        // XML 文档里存在）。空集是规范下的正确结果。
        let document = Document::parse_html(CATALOG);
        assert_eq!(names(&document, "//li/namespace::*"), Vec::<String>::new());
    }

    // ---------- 节点测试 ----------

    #[test]
    fn 节点类型测试() {
        let document = Document::parse_html("<div>a<!--c--><span>b</span></div>");
        // text() 只匹配文本节点（DataNode 不是文本节点，见 NodeTest::Text）
        let texts = strings(&document, "//div/text()");
        assert_eq!(texts, vec!["a"]);
        // comment()
        assert_eq!(number(&document, "count(//div/comment())"), 1.0);
        // node() 匹配一切
        assert_eq!(number(&document, "count(//div/node())"), 3.0);
    }

    #[test]
    fn script的内容不是文本节点() {
        // 与 Jsoup 公开文档一致："The contents of data nodes (such as
        // `<script>` tags) are not considered text"。
        let document = Document::parse_html("<div><script>var x=1</script>正常</div>");
        assert_eq!(strings(&document, "//div/text()"), vec!["正常"]);
    }

    #[test]
    fn 通配符与名字测试() {
        let document = Document::parse_html(CATALOG);
        assert_eq!(names(&document, "//div/*").len(), 2, "div 的两个子元素");
        assert_eq!(names(&document, "//*[@class='book']").len(), 3);
    }

    // ---------- 谓词 ----------

    #[test]
    fn 谓词位置是一起的且按轴方向() {
        let document = Document::parse_html(CATALOG);
        assert_eq!(strings(&document, "//li[1]/a"), vec!["一"]);
        assert_eq!(strings(&document, "//li[2]/a"), vec!["二"]);
        assert_eq!(strings(&document, "//li[last()]/a"), vec!["三"]);
        assert_eq!(strings(&document, "//li[last()-1]/a"), vec!["二"]);
        assert_eq!(strings(&document, "//li[position()=2]/a"), vec!["二"]);
    }

    #[test]
    fn 谓词串联时第二个谓词数的是收窄后的位置() {
        // §3.3 的例子：`//div[@class='x'][1]` 的位置在**过滤之后**数。
        let document = Document::parse_html(
            r#"<div><p class="x">第一个 x</p><p>不含</p><p class="x">第二个 x</p></div>"#,
        );
        assert_eq!(
            strings(&document, "/descendant::p[@class='x'][1]"),
            vec!["第一个 x"]
        );
        assert_eq!(
            strings(&document, "/descendant::p[@class='x'][2]"),
            vec!["第二个 x"]
        );
        assert_eq!(
            strings(&document, "/descendant::p[@class='x'][1]/text()"),
            vec!["第一个 x"]
        );
    }

    #[test]
    fn 双斜杠下的谓词作用于每一相而非整条路径() {
        // §3.3 明确说明：`//para[1]` 不等于 `/descendant::para[1]`。
        // 前者选"每个父节点下的第一个 para 子节点"。
        let document = Document::parse_html(
            "<div><section><p>s1p1</p><p>s1p2</p></section>\
             <section><p>s2p1</p><p>s2p2</p></section></div>",
        );
        assert_eq!(
            strings(&document, "//section/p[1]"),
            vec!["s1p1", "s2p1"],
            "每个 section 下各取第一个 p"
        );
        // 而 descendant 轴上取第一个
        assert_eq!(
            strings(&document, "/descendant::p[1]"),
            vec!["s1p1"],
            "descendant 轴的 `[1]` 是整棵树下第一个"
        );
    }

    #[test]
    fn 属性谓词() {
        let document = Document::parse_html(CATALOG);
        assert_eq!(
            strings(&document, "/descendant::li[@class='book'][1]/a"),
            vec!["一"],
            "`//li[...][1]` 在每个父节点下各取第一个，故用 descendant 轴取单个"
        );
        assert_eq!(strings(&document, "//li[not(@id)]/a").len(), 3);
        assert_eq!(
            strings(&document, "//li[@class='book' and @id='x']/a"),
            Vec::<String>::new()
        );
        assert_eq!(
            strings(&document, "//li[@class='book' or @id='x']/a").len(),
            3
        );
    }

    #[test]
    fn 谓词里的数字与布尔() {
        let document = Document::parse_html(CATALOG);
        // 数字结果按位置比较
        assert_eq!(strings(&document, "//li[1]/a"), vec!["一"]);
        // `[2]` 之外的节点都被刷掉
        assert_eq!(number(&document, "count(//li[2])"), 1.0);
        // 布尔结果用于存在性
        assert_eq!(number(&document, "count(//li[true()])"), 3.0);
        assert_eq!(number(&document, "count(//li[false()])"), 0.0);
    }

    // ---------- 运算符 ----------

    #[test]
    fn 比较运算符在四种类型间转换() {
        let document = Document::parse_html(CATALOG);
        // 字符串与字符串
        assert!(boolean(&document, "'a' = 'a'"));
        assert!(boolean(&document, "'a' != 'b'"));
        // 数字
        assert!(boolean(&document, "1 < 2"));
        assert!(boolean(&document, "2 <= 2"));
        assert!(boolean(&document, "3 > 2"));
        assert!(!boolean(&document, "3 >= 4"));
        // 节点集与字符串：任一节点匹配即为真
        assert!(boolean(&document, "//a = '二'"));
        assert!(!boolean(&document, "//a = '不存在'"));
        // 布尔
        assert!(boolean(&document, "true() = true()"));
        assert!(boolean(&document, "false() < true()"));
    }

    #[test]
    fn 算术运算符() {
        let document = Document::parse_html("<p>x</p>");
        assert_eq!(number(&document, "1 + 2"), 3.0);
        assert_eq!(number(&document, "5 - 3"), 2.0);
        assert_eq!(number(&document, "3 * 4"), 12.0);
        assert_eq!(number(&document, "7 div 2"), 3.5);
        assert_eq!(number(&document, "7 mod 2"), 1.0);
        // 一元负号等价于 `0 - x`
        assert_eq!(number(&document, "-3"), -3.0);
        assert_eq!(number(&document, "-(1 + 2)"), -3.0);
    }

    #[test]
    fn 联合去重并按文档序() {
        let document = Document::parse_html(CATALOG);
        // `//a | //span` 的结果按文档序：a, span, a, span, a, span
        assert_eq!(
            names(&document, "//a | //span"),
            vec!["a", "span", "a", "span", "a", "span"]
        );
        // 自身与自身联合去重
        assert_eq!(number(&document, "count(//li | //li)"), 3.0);
    }

    #[test]
    fn 逻辑运算符短路() {
        let document = Document::parse_html("<p>x</p>");
        // `and` 左侧为假时右侧不求值——用一个会报错的表达式验证
        assert!(!boolean(&document, "false() and count(//nonexistent) = 0"));
        assert!(boolean(&document, "true() or count(//nonexistent) = 0"));
        // 正常路径
        assert!(boolean(&document, "true() and true()"));
        assert!(boolean(&document, "false() or true()"));
    }

    // ---------- 函数 ----------

    #[test]
    fn 字符串函数() {
        let document = Document::parse_html(CATALOG);
        assert!(boolean(&document, "contains('abc', 'b')"));
        assert!(!boolean(&document, "contains('abc', 'z')"));
        assert!(boolean(&document, "starts-with('abc', 'ab')"));
        assert!(!boolean(&document, "starts-with('abc', 'bc')"));
        match eval(&document, "string-length('abc')") {
            XPathValue::Number(value) => assert_eq!(value, 3.0),
            other => panic!("应为数字，实际 {other:?}"),
        }
    }

    #[test]
    fn 节点集函数() {
        let document = Document::parse_html(CATALOG);
        assert_eq!(number(&document, "count(//li)"), 3.0);
        assert_eq!(number(&document, "count(//nonexistent)"), 0.0);
        assert_eq!(number(&document, "last()"), 1.0, "外层 last() 是上下文大小");
    }

    #[test]
    fn 节点集比较是任一匹配即为真() {
        // §3.4：节点集与字符串比较时"存在一个节点使比较成立"即为真
        let document = Document::parse_html("<ul><li>甲</li><li>乙</li><li>丙</li></ul>");
        assert!(boolean(&document, "//li = '乙'"));
        assert!(boolean(&document, "//li != '甲'"));
        // 全部不等时 `!=` 为真（只要有一对不同）
        assert!(boolean(&document, "//li != '甲'"));
    }

    // ---------- 结果类型 ----------

    #[test]
    fn 顶层函数调用产出标量() {
        let document = Document::parse_html(CATALOG);
        assert!(matches!(
            eval(&document, "count(//li)"),
            XPathValue::Number(_)
        ));
        assert!(matches!(eval(&document, "true()"), XPathValue::Boolean(_)));
        assert!(matches!(eval(&document, "'x'"), XPathValue::String(_)));
    }

    #[test]
    fn 空结果产出空节点集而非错误() {
        let document = Document::parse_html("<p>x</p>");
        assert_eq!(names(&document, "//nonexistent"), Vec::<String>::new());
        assert!(
            matches!(eval(&document, "//nonexistent"), XPathValue::NodeSet(nodes) if nodes.is_empty())
        );
    }

    #[test]
    fn 联合非节点集操作数报类型错() {
        let document = Document::parse_html("<p>x</p>");
        let compiled = compile("//p | 'x'").expect("应解析成功");
        let error = evaluate_compiled(&document, &compiled).expect_err("应报类型错");
        assert_eq!(error.code(), XPathErrorCode::NodeSetRequired);
    }

    #[test]
    fn 相对路径从上下文节点起走() {
        // 子树内求值：`.//` 与 `descendant::` 只在本子树内找
        let document = Document::parse_html(CATALOG);
        let book = crate::css::compile("li.book").expect("选择器应可编译");
        let first_book = crate::css::select_in_document(&document, &book)
            .into_iter()
            .next()
            .expect("应命中一本书");

        let compiled = compile("descendant::a").expect("应解析成功");
        let value = evaluate_compiled_in(&first_book, &compiled).expect("应求值成功");
        match value {
            XPathValue::NodeSet(nodes) => {
                assert_eq!(nodes.len(), 1, "只在子树内找到 1 个 a");
                assert_eq!(nodes[0].attr("href").as_deref(), Some("/b/1"));
            }
            other => panic!("应为节点集，实际 {other:?}"),
        }
    }

    #[test]
    fn 绝对路径在子树内求值仍从文档根起() {
        // §2.5：`/` 指向根。这是与 CSS「子树内查」的关键差异。
        let document = Document::parse_html(CATALOG);
        let book = crate::css::compile("li.book").expect("选择器应可编译");
        let first_book = crate::css::select_in_document(&document, &book)
            .into_iter()
            .next()
            .expect("应命中一本书");

        let compiled = compile("//li").expect("应解析成功");
        let value = evaluate_compiled_in(&first_book, &compiled).expect("应求值成功");
        match value {
            XPathValue::NodeSet(nodes) => assert_eq!(nodes.len(), 3, "绝对路径越过子树边界"),
            other => panic!("应为节点集，实际 {other:?}"),
        }
    }

    // ---------- 转换函数 ----------

    #[test]
    fn 布尔转换遵循规范() {
        assert!(to_boolean(&XPathValue::Number(1.0)));
        assert!(!to_boolean(&XPathValue::Number(0.0)));
        assert!(!to_boolean(&XPathValue::Number(f64::NAN)));
        assert!(!to_boolean(&XPathValue::String(String::new())));
        assert!(to_boolean(&XPathValue::String("x".into())));
        assert!(!to_boolean(&XPathValue::node_set(Vec::new())));
    }

    #[test]
    fn 数字转换遵循规范的字符串规则() {
        assert_eq!(to_number(&XPathValue::String("  12  ".into())), 12.0);
        assert_eq!(to_number(&XPathValue::String("-3.5".into())), -3.5);
        assert!(to_number(&XPathValue::String("abc".into())).is_nan());
        assert!(to_number(&XPathValue::String(String::new())).is_nan());
        assert_eq!(to_number(&XPathValue::Boolean(true)), 1.0);
    }

    #[test]
    fn 数字转字符串的格式() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(3.5), "3.5");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(f64::NAN), "NaN");
        assert_eq!(format_number(f64::INFINITY), "Infinity");
        assert_eq!(format_number(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn 谓词步数上界可触发() {
        // 上界存在的理由见 MAX_PREDICATE_STEPS 的文档。这里直接构造一个
        // 大文档 + 全扫描谓词，验证上界确实生效而不是永远走不到。
        let mut html = String::from("<ul>");
        for index in 0..2000 {
            html.push_str(&format!("<li>项{index}</li>"));
        }
        html.push_str("</ul>");
        let document = Document::parse_html(&html);

        // 正常谓词不该触发上界。注意 `//li[1]` 是"每个父节点下的第一个 li"，
        // 2000 个 li 是同一父节点下的兄弟，故只有 1 个命中——
        // 这正是 §3.3 用 `//para[1]` 举例说明的语义，不是 bug。
        assert_eq!(number(&document, "count(//li[1])"), 1.0);

        // 人为把预算压低到 1，验证错误路径可达
        let compiled = compile("//li[position() > 0]").expect("应解析成功");
        let root = document.root();
        let mut context = Context::new(&document, root);
        context.steps = MAX_PREDICATE_STEPS;
        let error =
            evaluate_expression(&compiled.expression, &mut context).expect_err("应触发步数上界");
        assert_eq!(error.code(), XPathErrorCode::StepLimit);
    }
}
