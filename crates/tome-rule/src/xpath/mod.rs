//! [DONE] 自研 XPath 1.0 子集引擎（tasks.md T048 解析 / T049 求值）
//!
//! # 为什么自研而不是引第三方 XPath 库
//!
//! 三条理由（research.md R2 已记录方向）：
//!
//! 1. **同一棵树**：CSS 引擎跑在 [`crate::dom`] 的自建 arena DOM 上。
//!    XPath 若用别的 DOM（或把 HTML 再解析一遍），两个引擎的结果会**系统性
//!    分叉**——同一个书源里 CSS 规则与 XPath 规则选出不同的节点，作者无从
//!    判断是规则写错了还是引擎不一致。
//! 2. **节点类型模型**：本项目的 DOM 把 `script`/`style` 内容建成 DataNode
//!    （见 [`crate::dom`] 的模块文档），而通用 XPath 库看到的是一棵普通
//!    文本树。`text()` 节点测试的语义因此不同。
//! 3. **书源生态的扩展节点测试**（`allText()` / `html()` / `outerHtml()` /
//!    `num()`）没有任何通用库支持，无论选哪个都要写一层适配。
//!
//! # 覆盖范围
//!
//! | 分类 | 覆盖 |
//! |---|---|
//! | 位置路径 | 绝对/相对，13 条轴（缩写与全称） |
//! | 节点测试 | 名字、`*`、`@*`、`text()`、`node()`、`comment()`、`processing-instruction()` |
//! | 谓词 | 数字位置、`last()`、属性、`and`/`or`、串联 |
//! | 函数 | §4 核心库的常用项 + 扩展节点测试 |
//! | 运算符 | 比较、算术、`and`/`or`、`\|` 联合、`()` 分组 |
//!
//! # 已知边界（实现里显式记录，不静默给错结果）
//!
//! - **属性节点不是独立节点**：本项目的 DOM 把属性存在元素内
//!   （[`crate::dom`] 的 `ElementData::attrs`），`@href` 的结果是**宿主元素**
//!   加属性名。故 `count(//li/@class)` 数的是"带 `class` 的 `li` 个数"，
//!   而不是"属性节点个数"（两者在这里恒等）；`//a/@href | //a` 这类
//!   "属性与元素混合的联合"无法区分二者。
//! - **命名空间轴恒为空**，`namespace-uri()` 返回 HTML 的命名空间 URL。
//!   见 `eval.rs` 的 `Axis::Namespace` 与 `func.rs` 的 `namespace-uri` 说明。
//! - **变量引用（`$x`）不支持**——书源规则没有给 XPath 传变量的机制。
//! - **`processing-instruction('target')` 带自变量形态不支持**；
//!   `id()` / `lang()` 恒返回空/false（HTML DOM 没有 DTD 的 ID 声明与
//!   `xml:lang`，见 `func.rs` 的说明）。
//!
//! # 关键入口
//!
//! - [`evaluate`] —— 在文档上求值，返回字符串列表（与 [`crate::css_post::evaluate_rule`] 对齐）
//! - [`evaluate_in`] —— 在子树内求值（与 [`crate::css_post::evaluate_rule_in`] 对齐）
//! - [`select_nodes`] —— 取节点（与 [`crate::css_post::select_nodes`] 对齐）

pub mod ast;
pub mod error;
pub mod eval;
pub mod func;
pub mod lexer;
pub mod parse;
pub mod value;

use crate::dom::{Document, NodeRef};
use crate::error::RuleError;
use error::XPathError;
use value::XPathValue;

/// 在文档上求值一条 XPath 表达式，返回规则引擎消费的字符串列表。
///
/// [DONE] 规则层的值入口。语义与 [`crate::css_post::evaluate_rule`] 对齐：
/// 节点集逐个转字符串，标量转成一个元素。
///
/// ## 结果怎么转字符串
///
/// 依据 [`crate::css_post`] 对后缀的处理：
///
/// - 元素节点走 `text()`（空白的 Jsoup 语义归一化）
/// - 属性轴产生的值走属性值本身
/// - 标量（`count(...)` 等函数结果）按 XPath 1.0 §3.4 的 `string()` 转换
///
/// 属性轴与元素节点共用同一个 [`NodeRef`]，故 `evaluate_expression` 的
/// 调用方需要知道"这一相步是不是属性轴"。这个信息在 AST 里，
/// 故本函数先解析再按 AST 上的属性轴标记取值——见 [`evaluate_compiled`]。
pub fn evaluate(document: &Document, expression: &str) -> Result<Vec<String>, RuleError> {
    let compiled = parse::compile(expression).map_err(map_parse_error)?;
    let value = eval::evaluate_compiled(document, &compiled).map_err(map_eval_error)?;
    Ok(value_to_strings(&value))
}

/// 在子树内求值（与 [`crate::css_post::evaluate_rule_in`] 同款语义）。
///
/// 注意：**绝对路径仍从文档根起走**（XPath 1.0 §2.5 的 `/` 指向根），
/// 这是与 CSS 的"子树内查"的实质差异。需要限定在本子树内的规则应写
/// 相对路径（`descendant::x` 或 `.//x`）。
pub fn evaluate_in(root: &NodeRef, expression: &str) -> Result<Vec<String>, RuleError> {
    let compiled = parse::compile(expression).map_err(map_parse_error)?;
    let value = eval::evaluate_compiled_in(root, &compiled).map_err(map_eval_error)?;
    Ok(value_to_strings(&value))
}

/// 取节点（`bookList` 那类用途），与 [`crate::css_post::select_nodes`] 同款。
///
/// 表达式的结果必须是节点集——标量结果（如 `count(//a)`）在此报语法错，
/// 因为调用方要的是"一组节点"而不是一个值。
pub fn select_nodes(document: &Document, expression: &str) -> Result<Vec<NodeRef>, RuleError> {
    let compiled = parse::compile(expression).map_err(map_parse_error)?;
    let value = eval::evaluate_compiled(document, &compiled).map_err(map_eval_error)?;
    match value {
        XPathValue::NodeSet(nodes) => Ok(nodes),
        other => Err(RuleError::syntax(format!(
            "取节点的 XPath 必须求值为节点集，`{expression}` 求值为 {other}"
        ))),
    }
}

/// 在**子树**内取节点（与 [`crate::css_post::select_nodes`] 的子树形态对应）。
///
/// # 与 [`select_nodes`] 的区别
///
/// 求值起点是 `root` 而不是文档根。故**绝对路径**（`/a/b`）在此仍从文档根
/// 起走（XPath 的规定如此），只有**相对路径**（`descendant::x`、`.//x`）
/// 才限定在子树内。这与 CSS 的"子树内查"是语义差异，书源作者需据此书写。
pub fn select_nodes_in(root: &NodeRef, expression: &str) -> Result<Vec<NodeRef>, RuleError> {
    let compiled = parse::compile(expression).map_err(map_parse_error)?;
    let value = eval::evaluate_compiled_in(root, &compiled).map_err(map_eval_error)?;
    match value {
        XPathValue::NodeSet(nodes) => Ok(nodes),
        other => Err(RuleError::syntax(format!(
            "取节点的 XPath 必须求值为节点集，`{expression}` 求值为 {other}"
        ))),
    }
}

/// 节点集/序列/标量 → 字符串列表。
///
/// ## 为什么属性结果（序列）要展开而不是折成一个
///
/// 书源规则取属性时（`//a/@href`）期望的是**每个 a 各给一个 href**，
/// 而 XPath 1.0 的 `string()` 转换只取文档序首个节点的值。故属性轴的结果
/// 用 [`XPathValue::Sequence`] 承载并由这里展开——见该变体的文档。
fn value_to_strings(value: &XPathValue) -> Vec<String> {
    match value {
        XPathValue::NodeSet(nodes) => nodes.iter().map(node_output_string).collect(),
        XPathValue::Sequence(values) => values.iter().map(eval::to_string).collect(),
        scalar => vec![eval::to_string(scalar)],
    }
}

/// 单个节点作为规则结果的字符串表示。
///
/// 元素走 `text()`（Jsoup 公开文档的 `Element#text()`：*"Gets the normalized,
/// combined text"*），与 [`crate::css_post::RuleSuffix::Text`] 完全一致——
/// 两个引擎的默认输出因此不会分叉。
///
/// ## 元素之外的节点
///
/// 文本/注释/Data 取 `nodeValue()`（即其字符数据）；文档节点取整棵树的
/// 归一化文本。这与 [`crate::css_post::RuleSuffix::NodeValue`] 对非元素
/// 节点的处理一致。
fn node_output_string(node: &NodeRef) -> String {
    value::string_value(node)
}

/// 解析错误 → 规则错误。
///
/// 映射规则：语法类映射到 [`RuleError::syntax`]，上界类映射到
/// [`RuleError::limit`]，其余（类型/节点集要求）也是规则类错误——
/// 它们都由表达式本身决定，与"能力缺失"不同。
fn map_parse_error(error: XPathError) -> RuleError {
    match error.code() {
        error::XPathErrorCode::DepthLimit | error::XPathErrorCode::StepLimit => {
            RuleError::limit(format!("XPath 表达式：{error}"))
        }
        _ => RuleError::syntax(format!("XPath 表达式：{error}")),
    }
}

/// 求值错误 → 规则错误。
fn map_eval_error(error: XPathError) -> RuleError {
    match error.code() {
        error::XPathErrorCode::DepthLimit | error::XPathErrorCode::StepLimit => {
            RuleError::limit(format!("XPath 求值：{error}"))
        }
        _ => RuleError::syntax(format!("XPath 求值：{error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><body>
        <div class="book">
          <h3 class="name"><a href="/book/1">书名一</a></h3>
          <p class="author">作者一</p>
        </div>
        <div class="book">
          <h3 class="name"><a href="/book/2">书名二</a></h3>
          <p class="author">作者二</p>
        </div>
      </body></html>"#;

    #[test]
    fn 求值返回字符串列表() {
        let document = Document::parse_html(PAGE);
        assert_eq!(
            evaluate(&document, "//h3[@class='name']/a").expect("应求值成功"),
            vec!["书名一", "书名二"]
        );
        assert_eq!(
            evaluate(&document, "//p[@class='author']/text()").expect("应求值成功"),
            vec!["作者一", "作者二"]
        );
    }

    #[test]
    fn 标量结果返回单个字符串() {
        let document = Document::parse_html(PAGE);
        assert_eq!(
            evaluate(&document, "count(//div[@class='book'])").expect("应求值成功"),
            vec!["2"]
        );
        assert_eq!(
            evaluate(&document, "true()").expect("应求值成功"),
            vec!["true"]
        );
    }

    #[test]
    fn 取节点返回节点句柄() {
        let document = Document::parse_html(PAGE);
        let nodes = select_nodes(&document, "//div[@class='book']").expect("应取到节点");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].node_name(), "div");
    }

    #[test]
    fn 取节点的表达式求值为标量时报错() {
        let document = Document::parse_html(PAGE);
        let error = select_nodes(&document, "count(//a)").expect_err("应报错");
        assert!(matches!(error, RuleError::Syntax { .. }));
    }

    #[test]
    fn 子树内求值接受相对路径() {
        let document = Document::parse_html(PAGE);
        let books = select_nodes(&document, "//div[@class='book']").expect("应取到节点");
        let first = &books[0];
        assert_eq!(
            evaluate_in(first, "descendant::p").expect("应求值成功"),
            vec!["作者一"]
        );
    }

    #[test]
    fn 语法错误映射为规则语法错误() {
        let document = Document::parse_html(PAGE);
        let error = evaluate(&document, "//div[").expect_err("应报错");
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Rule);
    }

    #[test]
    fn 无匹配产出空列表() {
        let document = Document::parse_html(PAGE);
        assert_eq!(
            evaluate(&document, "//nonexistent").expect("应求值成功"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn 扩展节点测试产出值() {
        let document =
            Document::parse_html(r#"<div class="c"><span>一</span><b>二</b><span>三</span></div>"#);
        // allText()：元素及全部后代的文本拼接
        assert_eq!(
            evaluate(&document, "//div[@class='c']/allText()").expect("应求值成功"),
            vec!["一二三"]
        );
        // html()：内部 HTML
        assert_eq!(
            evaluate(&document, "//div[@class='c']/html()").expect("应求值成功"),
            vec!["<span>一</span><b>二</b><span>三</span>"]
        );
        // outerHtml()：含自身的 HTML
        assert_eq!(
            evaluate(&document, "//div[@class='c']/outerHtml()").expect("应求值成功"),
            vec!["<div class=\"c\"><span>一</span><b>二</b><span>三</span></div>"]
        );
    }

    #[test]
    fn 扩展节点测试num从文本抽数字() {
        let document = Document::parse_html(
            r#"<div><span class="score">评分 8.9 分</span><span class="score">评分 9.5 分</span></div>"#,
        );
        assert_eq!(
            evaluate(&document, "//span[@class='score']/num()").expect("应求值成功"),
            vec!["8.9", "9.5"]
        );
        // 抽不到数字时是 NaN（与 number() 的转换一致）
        let document = Document::parse_html("<div><span>无数字</span></div>");
        assert_eq!(
            evaluate(&document, "//span/num()").expect("应求值成功"),
            vec!["NaN"]
        );
    }

    #[test]
    fn 书源规则的典型形态端到端() {
        // 书源的 XPath 规则长这样（公开语法约定的常见写法）：
        // 取书名、作者、链接、以及「取全部文本」。
        let document = Document::parse_html(
            r#"<html><body><div id="list">
              <div class="item">
                <a class="name" href="/book/1">书名一</a>
                <span class="author">作者一</span>
              </div>
              <div class="item">
                <a class="name" href="/book/2">书名二</a>
                <span class="author">作者二</span>
              </div>
            </div></body></html>"#,
        );

        // 列表项（bookList 用途）
        let items = select_nodes(&document, "//div[@class='item']").expect("应取到节点");
        assert_eq!(items.len(), 2);

        // 每项内的字段（子树内求值）
        assert_eq!(
            evaluate_in(&items[0], "descendant::a/@href").expect("应求值成功"),
            vec!["/book/1"]
        );
        assert_eq!(
            evaluate_in(&items[1], "descendant::span[@class='author']/allText()")
                .expect("应求值成功"),
            vec!["作者二"]
        );

        // 整页取全部书名
        assert_eq!(
            evaluate(&document, "//a[@class='name']/@href").expect("应求值成功"),
            vec!["/book/1", "/book/2"]
        );
        assert_eq!(
            evaluate(&document, "//a[@class='name']/text()").expect("应求值成功"),
            vec!["书名一", "书名二"]
        );
        // 位置谓词
        assert_eq!(
            evaluate(&document, "/descendant::div[@class='item'][2]/a").expect("应求值成功"),
            vec!["书名二"]
        );
    }

    #[test]
    fn 扩展节点测试可与其他规则混用() {
        // 兼容性关键点：书源的 XPath 规则长这样
        let document = Document::parse_html(PAGE);
        assert_eq!(
            evaluate(&document, "//p[@class='author']/allText()").expect("应求值成功"),
            vec!["作者一", "作者二"]
        );
    }
}

#[cfg(test)]
mod independent_checks {
    use super::*;

    /// 独立核验：反向轴的谓词按**轴方向**计数（W3C XPath 1.0 §2.4）。
    ///
    /// 规范原文：谓词的位置"根据轴的方向"——对反向轴（ancestor/preceding 等）
    /// 位置 1 是**离上下文最近**的那个。
    #[test]
    fn 反向轴谓词按轴方向计数() {
        let doc = crate::dom::Document::parse_html("<a><b><c>x</c></b></a>");
        let c = crate::css::select_in_document(&doc, &crate::css::compile("c").expect("应编译"))
            .into_iter()
            .next()
            .expect("c 应存在");
        // 从 c 出发的 ancestor::*[1] 应是**最近**的祖先 b（而非 a）
        let values = select_nodes_in(&c, "ancestor::*[1]").expect("应求值成功");
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].node_name(), "b", "反向轴位置 1 是最远的近亲");
    }

    /// 独立核验：`//x` 与 `/descendant-or-self::node()/child::x` 等价（§2.5）。
    #[test]
    fn 双斜杠的展开语义() {
        let doc = crate::dom::Document::parse_html("<r><x>1</x><m><x>2</x></m></r>");
        let short = evaluate(&doc, "//x").expect("应求值成功");
        let long = evaluate(&doc, "/descendant-or-self::node()/child::x").expect("应求值成功");
        assert_eq!(short, long, "`//x` 必须与展开形态等价");
        assert_eq!(short, vec!["1", "2"]);
    }

    /// 独立核验：联合 `|` 去重且按文档序（§3.3）。
    #[test]
    fn 联合去重且文档序() {
        let doc = crate::dom::Document::parse_html("<r><a>1</a><b>2</b></r>");
        let values = evaluate(&doc, "//b | //a | //a").expect("应求值成功");
        assert_eq!(values, vec!["1", "2"], "去重 + 文档序（a 在 b 前）");
    }

    /// 病态嵌套输入不得让引擎崩溃（AGENTS.md 生产级护栏）。
    ///
    /// # 为什么这个测试在**公开入口**而不在解析器里
    ///
    /// 解析器里已有一条 200 层的测试（`深层嵌套超过上界时报深度错误`），
    /// 但那测的是"解析器会报错"。真正的风险在**生产路径的端到端行为**：
    /// 书源规则是第三方输入，一条深层嵌套的畸形规则若能让引擎栈溢出，
    /// 整个进程直接死（Rust 里栈溢出不可捕获）。
    ///
    /// 故这里从 `evaluate` 走，且深度取到 10 万——远超任何合理规则，
    /// 足以把"递归深度由输入决定"这一风险实际触发出来。
    #[test]
    fn 病态深嵌套不崩溃且报结构化错误() {
        let doc = crate::dom::Document::parse_html("<a>x</a>");
        for depth in [100usize, 1_000, 10_000, 100_000] {
            // `[a[a[a[...]]]]`：每层都进一次谓词递归
            let expression = format!("{}x{}", "/a[".repeat(depth), "]".repeat(depth));
            let error =
                evaluate(&doc, &expression).expect_err(&format!("{depth} 层应超上界而非成功"));
            let rendered = format!("{error}");
            assert!(
                rendered.contains("深度") || rendered.contains("上界"),
                "{depth} 层应给出深度上界错误，实际：{rendered}"
            );
        }
        // 上界之内的正常表达式仍要能跑（护栏不该误伤）
        assert_eq!(evaluate(&doc, "//a").expect("应求值成功"), vec!["x"]);
    }
}
