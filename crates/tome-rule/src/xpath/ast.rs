//! [DONE] XPath 1.0 子集的抽象语法树（tasks.md T048）
//!
//! # 结构依据
//!
//! 节点形状直接对应 W3C XPath 1.0 §3 的语法产生式，不做"更聪明的等价改写"：
//!
//! - §3.3 `LocationPath` —— [`LocationPath`]、[`Step`]
//! - §3.3 `AxisSpecifier` / `NodeTest` / `Predicate` —— [`Axis`]、[`NodeTest`]、[`Predicate`]
//! - §3.4 `FilterExpr` / `PathExpr` —— [`Expr::Path`]（谓词可作用在任意表达式上）
//! - §3.5 `UnionExpr` —— [`Expr::Union`]
//! - §3.2 `PathExpr` 的每一步产出 —— [`StepResult`] 的 `All` / `NodeSet`
//!
//! # 为什么用 `Vec` 而不是链表保存位置路径
//!
//! `Vec<Step>` 的中间产出要反复按**文档序去重排序**（节点集是集合，§3.3），
//! 而排序需要随机访问。`Vec` 让"按步推进"变成一次朴素循环，
//! 且 `Step::predicates` 的 `Vec` 顺序天然就是"谓词从左到右依次收窄"（§2.4）。

use super::lexer::SpannedToken;

/// 轴（W3C XPath 1.0 §2.2 Axes）。
///
/// 13 条轴全部列出，**不省略**任何一条：即便 `namespace` 在本项目的 HTML
/// DOM 上恒为空（[`Axis::Namespace`]），保留它使"语法齐全"与"语义可用"
/// 两件事分开——不支持的语义应由求值期给出，而不是让解析器把表达式
/// 当成非法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// `child::`（默认轴）
    Child,
    /// `descendant::`
    Descendant,
    /// `parent::`（缩写 `..`）
    Parent,
    /// `ancestor::`
    Ancestor,
    /// `following-sibling::`
    FollowingSibling,
    /// `preceding-sibling::`
    PrecedingSibling,
    /// `following::`
    Following,
    /// `preceding::`
    Preceding,
    /// `attribute::`（缩写 `@`）
    Attribute,
    /// `self::`（缩写 `.`）
    SelfAxis,
    /// `descendant-or-self::`（缩写 `//`）
    DescendantOrSelf,
    /// `ancestor-or-self::`
    AncestorOrSelf,
    /// `namespace::`
    Namespace,
}

impl Axis {
    /// 按轴名解析。名字大小写敏感（§2.2 的轴名是全小写）。
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "child" => Self::Child,
            "descendant" => Self::Descendant,
            "parent" => Self::Parent,
            "ancestor" => Self::Ancestor,
            "following-sibling" => Self::FollowingSibling,
            "preceding-sibling" => Self::PrecedingSibling,
            "following" => Self::Following,
            "preceding" => Self::Preceding,
            "attribute" => Self::Attribute,
            "self" => Self::SelfAxis,
            "descendant-or-self" => Self::DescendantOrSelf,
            "ancestor-or-self" => Self::AncestorOrSelf,
            "namespace" => Self::Namespace,
            _ => return None,
        })
    }

    /// 轴名（诊断用）。
    pub fn name(&self) -> &'static str {
        match self {
            Self::Child => "child",
            Self::Descendant => "descendant",
            Self::Parent => "parent",
            Self::Ancestor => "ancestor",
            Self::FollowingSibling => "following-sibling",
            Self::PrecedingSibling => "preceding-sibling",
            Self::Following => "following",
            Self::Preceding => "preceding",
            Self::Attribute => "attribute",
            Self::SelfAxis => "self",
            Self::DescendantOrSelf => "descendant-or-self",
            Self::AncestorOrSelf => "ancestor-or-self",
            Self::Namespace => "namespace",
        }
    }

    /// 是否为**反向轴**（W3C XPath 1.0 §2.2 的 "reverse axes"）。
    ///
    /// 依据 §2.2 正文：
    ///
    /// > The ancestor, ancestor-or-self, preceding, and preceding-sibling axes
    /// > are reverse axes; all other axes are forward axes. The ancestor,
    /// > descendant, following, preceding and self axes partition a document
    /// > (ignoring attribute and namespace nodes): they do not overlap and
    /// > together contain all the nodes in the document.
    ///
    /// 这一位决定**谓词位置**怎么数（§2.4：*"the proximity position ...
    /// in the axis direction"*），是本模块最容易被写错的一点，故单列成函数
    /// 并配专门的测试。
    pub fn is_reverse(&self) -> bool {
        matches!(
            self,
            Self::Ancestor | Self::AncestorOrSelf | Self::Preceding | Self::PrecedingSibling
        )
    }
}

/// 节点测试（W3C XPath 1.0 §2.3 Node Tests）+ 书源生态的扩展节点测试。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeTest {
    /// 名字测试：`div` / `ns:div`。
    ///
    /// `prefix` 是明写的命名空间前缀；HTML 文档上无前缀声明可查，
    /// 故求值期对带前缀的名字测试做**本地名匹配**并把前缀记入诊断
    /// （[UNCERTAIN] 见 `eval.rs` 的同名说明）。
    Name {
        /// 明写的前缀（`ns:div` 的 `ns`）。无前缀为 `None`。
        prefix: Option<String>,
        /// 本地名
        local: String,
    },
    /// 通配符 `*`：匹配任意元素名。
    Wildcard,
    /// 节点类型测试 `text()`。
    ///
    /// 依据 §2.3 的 `TextTest`：*"matches any text node"*，且规范明确
    /// "character data nodes" 可能被合并——本项目的 HTML DOM 把
    /// `script`/`style` 内容建成 DataNode（[`crate::dom::NodeKind::Data`]），
    /// 它**不是** text node，故不匹配（与 Jsoup 公开文档对
    /// "contents of data nodes (such as `<script>` tags) are not considered
    /// text" 的表述一致）。CData 经 §5.1 的 "text node" 归一化后是文本，
    /// 故匹配。
    Text,
    /// 节点类型测试 `comment()`。
    Comment,
    /// 节点类型测试 `processing-instruction()`（§2.3 允许带 target 自变量；
    /// 本子集只支持无自变量形态，带 target 的形态见 `parse.rs` 的说明）。
    ProcessingInstruction,
    /// 节点类型测试 `node()`：匹配任意节点（§2.3 *"matches any node"*）。
    AnyNode,
    /// 扩展：`allText()` —— 元素及其全部后代的**文本拼接**，产出一个值节点。
    ///
    /// 依据来源：`allText()` 属书源规则生态的公开扩展函数（社区流通的
    /// 《书源规则说明》把 `allText` 与 `text` 并列在"获取内容"清单里），
    /// 在 XPath 规则中**写作路径末段的节点测试**（如 `/div/allText()`），
    /// 故它在本实现里是 [`NodeTest`] 的一个变体而不是普通函数。
    AllText,
    /// 扩展：`html()` —— 元素的**内部** HTML，产出一个值节点。
    ///
    /// 依据来源同 [`NodeTest::AllText`]（公开规则说明把 `html` 列为可取的
    /// "内容"之一，其语义对应 Jsoup 公开文档 `Element#html()`：
    /// *"on a `<div>` with one empty `<p>`, this returns `<p></p>`"*）。
    Html,
    /// 扩展：`outerHtml()` —— 元素及子树的 HTML，产出一个值节点。
    ///
    /// 依据来源同 [`NodeTest::AllText`]；语义对应 Jsoup 公开文档
    /// `Node#outerHtml()`：*"returns `<div><p></p></div>`"*。
    OuterHtml,
    /// 扩展：`num()` —— 从节点内容中抽出**第一个数字**，产出一个值节点。
    ///
    /// 依据来源同 [`NodeTest::AllText`]。抽取判据见 `eval.rs`
    /// （[UNCERTAIN] 精确抽取范围）。
    Number,
}

/// 谓词（§3.3 `Predicate ::= '[' PredicateExpr ']'`）。
///
/// 保留原始表达式与源码区间：源码区间用于"谓词位置反了"这类诊断
/// （报出行内第几段谓词，比只报整条表达式有用）。
#[derive(Debug, Clone, PartialEq)]
pub struct Predicate {
    /// 谓词表达式
    pub expression: Expr,
    /// 方括号内表达式的源码字节区间（诊断用）
    pub span: (usize, usize),
}

/// 一步（§3.3 `Step ::= AxisSpecifier NodeTest Predicate* | AbbreviatedStep`）。
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// 轴
    pub axis: Axis,
    /// 节点测试
    pub node_test: NodeTest,
    /// 谓词，按书写顺序
    pub predicates: Vec<Predicate>,
}

impl Step {
    /// 便捷构造无谓词的步。
    pub fn new(axis: Axis, node_test: NodeTest) -> Self {
        Self {
            axis,
            node_test,
            predicates: Vec::new(),
        }
    }
}

/// 位置路径（§3.3 `LocationPath`）。
#[derive(Debug, Clone, PartialEq)]
pub struct LocationPath {
    /// 是否绝对路径（以 `/` 或 `//` 起头）。绝对路径的**初值节点集**
    /// 只含根节点（§2.4："the context node is the root node"）。
    pub absolute: bool,
    /// 步序列。空序列表示"整条路径就是 `.`"（如 `//` 之后的空步被
    /// 规范化成 `self::node()`，见 `parse.rs`）。
    pub steps: Vec<Step>,
}

/// 表达式的**求值产出形态**。
///
/// 依据 W3C XPath 1.0 §3.2 `PathExpr`：
///
/// > A `/` is transformed into `[self::node()]`, a `//` into
/// > `[self::node(), descendant-or-self::node()]`.
///
/// 即"路径的每一步产出**节点序列**"。而书源生态的扩展节点测试
/// （`allText()` 等）产出的是**值**而非文档中的节点——它们没有可继续
/// 遍历的子节点。故这一步的产出形态被显式区分，而不是把"计算出的字符串
/// 又造一个假节点"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    /// 产出文档中的节点序列（节点集）
    NodeSet,
    /// 产出"全部后代文本拼接"这类计算值
    AllText,
}

impl NodeTest {
    /// 该节点测试在"作为一步的产出"时的形态。
    ///
    /// 规范里的四种节点类型测试与名字测试、通配符都产出节点集；
    /// 三个扩展节点测试产出计算值。
    pub fn step_result(&self) -> StepResult {
        match self {
            Self::Name { .. }
            | Self::Wildcard
            | Self::Text
            | Self::Comment
            | Self::ProcessingInstruction
            | Self::AnyNode => StepResult::NodeSet,
            Self::AllText | Self::Html | Self::OuterHtml | Self::Number => StepResult::AllText,
        }
    }
}

/// 表达式（§3 `Expr`）。
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// 字面量。XPath 1.0 只有数字与字符串两种字面量（§3.7），
    /// 布尔由 `true()` / `false()` 函数给出，故无 `Bool` 变体。
    Literal(Value),
    /// 函数调用（§3.2 `FunctionCall`）。自变量可为空（如 `true()`）。
    FunctionCall {
        /// 函数名（大小写敏感，§3.2 的内建函数清单全是小写）
        name: String,
        /// 自变量
        arguments: Vec<Expr>,
    },
    /// 路径表达式（§3.2 `PathExpr`）
    Path {
        /// 过滤表达式——路径的**初值**。唯一路径（`/a`、`a/b`）为 `None`，
        /// 此时初值按 §2.4 取上下文节点（相对）或根节点（绝对）。
        filter: Option<Box<Expr>>,
        /// 位置路径。空序列表示"取过滤结果本身"（`(//a)[1]` 的 `//a` 部分
        /// 由 `filter` 承担，`steps` 为空）。
        path: LocationPath,
    },
    /// 一元的负号。`-x` 等价于 `0 - x`（§3.5 没有一元产生式，
    /// 这里保留独立变体以便维持**解析出的形状**，求值时按 `0 - x` 算）。
    Negate(Box<Expr>),
    /// 算术与比较。
    Unary {
        /// 运算符
        operator: BinaryOperator,
        /// 左操作数
        left: Box<Expr>,
        /// 右操作数
        right: Box<Expr>,
    },
    /// 节点集联合 `|`（§3.5 `UnionExpr`）
    Union(Box<Expr>, Box<Expr>),
}

/// 字面量值。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// 数字（§3.7：XPath 1.0 的数字统一是 double）
    Number(f64),
    /// 字符串
    String(String),
}

/// 二元运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    /// 逻辑或
    Or,
    /// 逻辑与
    And,
    /// `=`
    Equal,
    /// `!=`
    NotEqual,
    /// `<`
    Less,
    /// `<=`
    LessOrEqual,
    /// `>`
    Greater,
    /// `>=`
    GreaterOrEqual,
    /// `+`
    Add,
    /// `-`
    Subtract,
    /// `*`
    Multiply,
    /// `div`
    Divide,
    /// `mod`
    Modulo,
}

impl BinaryOperator {
    /// 运算符的书写形式（诊断用）。
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Or => "or",
            Self::And => "and",
            Self::Equal => "=",
            Self::NotEqual => "!=",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "div",
            Self::Modulo => "mod",
        }
    }
}

/// 一个**已编译**的表达式：AST 加上它被编译时的模式。
///
/// `mode` 记的是"当作节点集路径求值"还是"当作标量表达式求值"，
/// 由 [`super::parse::compile`] 按 W3C XPath 1.0 §3.3 的
/// **Disambiguation** 规则判定：
///
/// > If the expression is a LocationPath, or if it is a FilterExpr that is
/// > itself a LocationPath, then it is evaluated as a node-set.
#[derive(Debug)]
pub struct CompiledExpression {
    /// 根表达式
    pub expression: Expr,
    /// 结果模式
    pub mode: ResultMode,
    /// 源码字节总长度（诊断用）
    pub source_len: usize,
}

/// 表达式的结果模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultMode {
    /// 结果是节点集（用于"选择节点"的入口）
    NodeSet,
    /// 结果是标量或节点集（用于"取值"的入口，节点集会被转成字符串列表）
    Scalar,
}

/// 解析过程中跨 token 传递的信息。
///
/// 把源码与 token 序列包在一起，避免每个 `parse_*` 函数都传三个参数。
pub(crate) struct ParseContext<'a> {
    /// 源码
    pub source: &'a str,
    /// 记号序列
    pub tokens: &'a [SpannedToken],
}

impl<'a> ParseContext<'a> {
    pub(crate) fn new(source: &'a str, tokens: &'a [SpannedToken]) -> Self {
        Self { source, tokens }
    }

    /// 取第 `index` 个记号。
    pub(crate) fn get(&self, index: usize) -> Option<&'a SpannedToken> {
        self.tokens.get(index)
    }

    /// 记号总数。
    pub(crate) fn len(&self) -> usize {
        self.tokens.len()
    }

    /// 源码里某个字节区间对应的原文（诊断用）。
    pub(crate) fn slice(&self, span: (usize, usize)) -> &'a str {
        self.source.get(span.0..span.1).unwrap_or_default()
    }
}
