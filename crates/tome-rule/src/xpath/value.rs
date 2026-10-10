//! \[DONE] XPath 值模型与 XPath 1.0 §3.6 的节点字符串展开（tasks.md T049）

use std::fmt;

use crate::dom::{NodeKind, NodeRef};

/// XPath 1.0 的四种值（§1.1 "Types"）。
///
/// > The primary syntactic constructs of the expression language are:
/// > ... a node-set (an unordered collection of nodes without duplicates)
/// > ... a boolean, a number, a string
///
/// 节点集在本实现里用 `Vec<NodeRef>` 表示：**去重**由 [`Self::node_set`]
/// 保证，**文档序**由 [`sort_document_order`] 保证（节点集是无序集合，
/// 但求值产出要按文档序，理由见该函数文档）。
///
/// 不派生 `PartialEq`：[`NodeRef`] 没有值相等语义（它是 arena 句柄），
/// 而"两个值相等"只在测试里用得上——测试用 [`Self::node_set`] 与
/// [`same_node`] 显式比较，不靠派生。
#[derive(Debug, Clone)]
pub enum XPathValue {
    /// 节点集
    NodeSet(Vec<NodeRef>),
    /// 数字（XPath 1.0 的数字统一是 double，§3.7）
    Number(f64),
    /// 布尔
    Boolean(bool),
    /// 字符串
    String(String),
    /// 字符串序列：**本项目的内部表示**，不属于 XPath 1.0 的四种类型。
    ///
    /// 存在的理由：属性轴（`//a/@href`）在书源规则里的语义是"每个 a 的
    /// href 值拼成列表"，而 XPath 1.0 的节点集经 `string()` 转换只会给出
    /// **第一个**节点的值。若把属性结果也折成一个字符串，`//a/@href`
    /// 就只会产出第一个 href——那是错的。
    ///
    /// 故属性轴的结果用序列承载，由 [`super`] 的入口展开成字符串
    /// 列表。除该入口外，序列**不参与**任何 XPath 运算（对它做算术会被
    /// [`super::eval::to_number`] 按惯例取首项）。
    Sequence(Vec<XPathValue>),
}

impl XPathValue {
    /// 构造一个节点集：去重后按文档序排列。
    ///
    /// \[DONE] 节点集的**唯一**构造入口。去重是 §3.6 明文要求的：
    ///
    /// > A node-set is a set of nodes with no duplicates.
    ///
    /// 排序则是本项目对"节点集是无序集合"的**有意选择**：书源规则把它当
    /// 列表用（`//a[1]`、`//a[5]` 的位置语义必须有确定答案），而列表必须有
    /// 稳定顺序；文档序是 §5 定义的唯一内在顺序，故取它。
    pub fn node_set(nodes: Vec<NodeRef>) -> Self {
        let mut nodes = nodes;
        sort_document_order(&mut nodes);
        nodes.dedup_by(|left, right| same_node(left, right));
        Self::NodeSet(nodes)
    }
}

/// 两个句柄是否指向同一个节点（同文档 + 同 arena id）。
pub fn same_node(left: &NodeRef, right: &NodeRef) -> bool {
    left.document().same_document(right.document()) && left.id() == right.id()
}

/// 按文档序排列。
///
/// 依据 W3C XPath 1.0 §5 "Data Model" 对文档序的定义：
///
/// > The ordering of nodes in a document is defined ... document order is
/// > defined to be the order in which the first character of the XML
/// > representation of the node occurs in the XML representation of the document.
///
/// 即先序（根 → 祖先 → 自身 → 子孙 → 后续兄弟）。本项目的 DOM 把
/// `script`/`style` 内容建成 DataNode、把元素属性存在元素内，故节点自身的
/// 先序位置就是它的文档序——**唯一的例外是属性节点与命名空间节点**：
///
/// > There is an ordering, document order, defined on all the nodes in the
/// > document ... The root node will be the first node. ... the namespace
/// > nodes and attribute nodes of an element are ordered before the element's
/// > children.
///
/// 故属性节点的文档序有歧义（属性之间本身无序）。本实现的取舍见
/// [`compare_document_order`]。
pub fn sort_document_order(nodes: &mut [NodeRef]) {
    nodes.sort_by(compare_document_order);
}

/// 文档序比较。
///
/// ## 属性节点的次序
///
/// §5 说属性节点排在元素的子节点**之前**，但没有定义属性之间的次序，
/// 而 DOM 的属性本身保持源码顺序（[`crate::dom`] 的 `attrs` 是 `Vec`）。
/// 故本实现按"元素本身的位置 + 属性在元素内的下标"排序——这是让
/// `@*[1]`、`@*[last()]` 有确定答案的唯一办法。属性节点在
/// [`crate::dom`] 里由 `(元素, 属性名)` 二元组表示，见 `AttributeNode`。
pub fn compare_document_order(left: &NodeRef, right: &NodeRef) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    if same_node(left, right) {
        return Ordering::Equal;
    }
    // 不同文档之间没有文档序可言；用 id 作稳定兜底，避免比较函数不满足
    // 全序——那会让 `sort_by` panic（"comparison function does not implement
    // a total order"），而本项目禁止生产路径 panic。
    if !left.document().same_document(right.document()) {
        return left.id().cmp(&right.id());
    }
    // 不同文档之间没有文档序可言；用 id 作稳定兜底，避免比较函数不满足
    // 全序（排序会因此 panic："comparison function does not implement a total order"）。
    let left_chain = ancestor_chain_ids(left);
    let right_chain = ancestor_chain_ids(right);
    let shared = left_chain
        .iter()
        .zip(right_chain.iter())
        .take_while(|(left_id, right_id)| left_id == right_id)
        .count();

    match (left_chain.get(shared), right_chain.get(shared)) {
        // 一个是另一个的祖先：祖先在前
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left_id), Some(right_id)) => left_id.cmp(right_id),
        (None, None) => Ordering::Equal,
    }
}

/// 从根到自身的 id 链（根在首）。
///
/// 用显式循环而非递归：DOM 深度由外部 HTML 决定（见 [`crate::dom`] 的
/// 深嵌套测试），递归会爆栈。
fn ancestor_chain_ids(node: &NodeRef) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut cursor = Some(node.clone());
    while let Some(current) = cursor {
        chain.push(current.id());
        cursor = current.parent();
    }
    chain.reverse();
    chain
}

/// XPath 1.0 §3.6 的**字符串函数** `string()` 对节点的展开规则。
///
/// 依据 §3.6 的 `string` 函数词条：
///
/// > A node-set is converted to a string by returning the string-value of the
/// > node in the node-set that is first in document order.
///
/// 而各节点的 string-value 由 §5 定义：
///
/// > The string-value of an element node is the concatenation of the
/// > string-values of all text node descendants of the element node in
/// > document order. The string-value of a text node is the character data.
/// > The string-value of an attribute node is the normalized value.
/// > The string-value of a comment node is the content of the comment.
/// > The string-value of a processing instruction node is the part of the
/// > content following the target. The string-value of a namespace node is
/// > the namespace URI.
///
/// ## 与 Jsoup 序列的差别
///
/// 纯规范版的元素 string-value **不归一化空白**（"concatenation of the
/// string-values of all text node descendants"）。而书源规则生态里
/// "取元素文本"的通行语义是 Jsoup 公开文档 `Element#text()`：
///
/// > Gets the normalized, combined text ... Whitespace is normalized and trimmed.
///
/// 即折叠空白并 trim。这里取 Jsoup 语义：书源规则产出的值会直接进入
/// 书名/作者等字段，带着原文缩进与换行是没用的。这也是
/// [`crate::css_post::RuleSuffix`] 对 `text` 的既有处理，两个引擎的输出
/// 因此**不会系统性分叉**（research R2 的一致性要求）。
pub fn string_value(node: &NodeRef) -> String {
    match node.kind() {
        NodeKind::Element => node.text(),
        // 属性节点在 DOM 里没有独立表示（见 AttributeNode），由调用方先行处理；
        // 走到这里说明是"元素以外的节点"，取它的字符数据。
        NodeKind::Text | NodeKind::CData | NodeKind::Data | NodeKind::Comment => node.node_value(),
        // §5：处理指令的 string-value 是 target 之后的内容
        NodeKind::ProcessingInstruction => node.node_value(),
        NodeKind::Doctype => node.node_value(),
        // 文档节点的 string-value 是所有文本后代的拼接（不给它加空白归一化，
        // 与 CData 无关；这里仍走 text() 以保持与元素一致）
        NodeKind::Document => node.text(),
    }
}

/// 把一个节点渲染成便于调试与**结果输出**的字符串。
///
/// 与 [`string_value`] 分开：后者是规范里的 string-value（用于比较与函数
/// 求值），前者是"作为规则结果给用户看的那一份"。两者在当前实现里对
/// 元素节点取值相同，但对**属性结果**不同——书源规则取属性节点时
/// 要的是属性值本身，见 [`crate::css_post`] 对 `@attr(x)` 的处理。
impl fmt::Display for XPathValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeSet(nodes) => write!(formatter, "节点集({} 项)", nodes.len()),
            Self::Number(value) => write!(formatter, "{value}"),
            Self::Boolean(value) => write!(formatter, "{value}"),
            Self::String(value) => formatter.write_str(value),
            Self::Sequence(values) => write!(formatter, "序列({} 项)", values.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Document;

    fn first_element(document: &Document, name: &str) -> NodeRef {
        document
            .descendant_elements()
            .find(|node| node.node_name() == name)
            .expect("元素应存在")
    }

    #[test]
    fn 节点集去重() {
        let document = Document::parse_html("<div><p>x</p></div>");
        let paragraph = first_element(&document, "p");
        let value = XPathValue::node_set(vec![paragraph.clone(), paragraph.clone()]);
        match value {
            XPathValue::NodeSet(nodes) => assert_eq!(nodes.len(), 1, "重复节点应被去掉"),
            other => panic!("应为节点集，实际 {other:?}"),
        }
    }

    #[test]
    fn 节点集按文档序排列() {
        let document = Document::parse_html("<div><a>1</a><b>2</b><c>3</c></div>");
        let a = first_element(&document, "a");
        let b = first_element(&document, "b");
        let c = first_element(&document, "c");
        let value = XPathValue::node_set(vec![c.clone(), a.clone(), b.clone()]);
        match value {
            XPathValue::NodeSet(nodes) => {
                let names: Vec<String> = nodes.iter().map(|node| node.node_name()).collect();
                assert_eq!(names, vec!["a", "b", "c"], "乱序输入应被排成文档序");
            }
            other => panic!("应为节点集，实际 {other:?}"),
        }
    }

    #[test]
    fn 祖先节点排在子孙之前() {
        let document = Document::parse_html("<div><section><p>x</p></section></div>");
        let div = first_element(&document, "div");
        let paragraph = first_element(&document, "p");
        let mut nodes = vec![paragraph.clone(), div.clone()];
        sort_document_order(&mut nodes);
        assert_eq!(nodes[0].node_name(), "div", "祖先在前");
        assert_eq!(nodes[1].node_name(), "p");
    }

    #[test]
    fn 元素字符串值按jsoup语义归一化空白() {
        // 依据见 string_value 的文档：规范版不归一化，但书源生态以 Jsoup
        // 公开文档的 Element#text() 为准（"Whitespace is normalized and trimmed"）。
        let document = Document::parse_html("<p>Hello   <b>there</b>\n  now! </p>");
        let paragraph = first_element(&document, "p");
        assert_eq!(string_value(&paragraph), "Hello there now!");
    }

    #[test]
    fn 注释与datanode的字符串值是其字符数据() {
        let document = Document::parse_html("<div>a<!-- 注释 -->b<script>var x=1</script></div>");
        let comment = document
            .descendants()
            .find(|node| node.kind() == NodeKind::Comment)
            .expect("注释应存在");
        assert_eq!(string_value(&comment), " 注释 ");

        let data = document
            .descendants()
            .find(|node| node.kind() == NodeKind::Data)
            .expect("DataNode 应存在");
        assert_eq!(string_value(&data), "var x=1");
    }
}
