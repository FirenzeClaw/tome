//! [DONE] XPath 1.0 子集解析器：记号序列 → 抽象语法树（tasks.md T048）
//!
//! # 覆盖范围（与 `specs/001-pc-engine-core/tasks.md` T048 对齐）
//!
//! 位置路径（绝对/相对、13 条轴的缩写与全称）、节点测试（名字/`*`/`@*`/
//! `text()`/`node()`/`comment()`/`processing-instruction()` 无自变量形态）、
//! 谓词、函数调用、全部比较与算术运算符、`|` 联合、`(` `)` 分组，
//! 以及书源生态的四个扩展节点测试（`allText()`/`html()`/`outerHtml()`/`num()`）。
//!
//! # 递归下降的层次
//!
//! 按 W3C XPath 1.0 §3 的产生式层层下降，每一层对应一个函数：
//!
//! ```text
//! OrExpr      → AndExpr    ('or' AndExpr)*
//! AndExpr     → Equality   ('and' Equality)*
//! Equality    → Relational (('=' | '!=') Relational)*
//! Relational  → Additive   (('<' | '>' | '<=' | '>=') Additive)*
//! Additive    → Multiplicative (('+' | '-') Multiplicative)*
//! Multiplicative → Unary   (('*' | 'div' | 'mod') Unary)*
//! Unary       → '-' Unary | Union
//! Union       → PathExpr   ('|' PathExpr)*
//! PathExpr    → LocationPath | FilterExpr ('/' RelativeLocationPath)?
//! FilterExpr  → PrimaryExpr Predicate*
//! ```
//!
//! # 与规范的一处刻意偏离：乘号的结合性
//!
//! §3 的产生式把 `*`/`div`/`mod` 写成左结合（`(('*' | 'div' | 'mod') Unary)*`），
//! 但**同层混用**时 `a div b * c` 的两种结合给出不同结果。规范的 XPath 1.0
//! 语法注解注明实际按优先级分层（即 `*` 先于 `div`/`mod`；同优先级左结合）。
//! 本实现按左结合解析（`((a div b) * c)` 形态），这是规范原文产生式的直接读法；
//! [UNCERTAIN] 若有语料显示别的读法更常见，此处需复核。

use super::ast::{
    BinaryOperator, CompiledExpression, Expr, LocationPath, NodeTest, ParseContext, Predicate,
    ResultMode, Step, Value,
};
use super::error::{XPathError, XPathErrorCode};
use super::lexer::{tokenize, LexMode, SpannedToken, Token};

/// 谓词求值的最大嵌套层数（[`Expr`] 的递归深度上界）。
///
/// 取 64：书源规则里的谓词嵌套极少超过三层，而 64 层已足够触发"畸形输入
/// 导致深递归"的防护。用规范 §2.4 的 `err:DEPTH` 语义命名（见 `XPathErrorCode`）。
const MAX_EXPRESSION_DEPTH: usize = 64;

/// 解析一条 XPath 表达式。
///
/// [DONE] 解析层的唯一入口。产出 [`CompiledExpression`]，其 `mode` 按
/// §3.3 的 Disambiguation 规则给出（决定"选择节点"与"取值"两个入口的行为）。
pub fn compile(source: &str) -> Result<CompiledExpression, XPathError> {
    let tokens = tokenize(source)?;
    if tokens.is_empty() {
        return Err(XPathError::syntax("表达式为空"));
    }

    let context = ParseContext::new(source, &tokens);
    let mut parser = Parser {
        context: &context,
        position: 0,
        depth: 0,
    };

    let expression = parser.parse_or_expression()?;
    if parser.position < context.len() {
        let token = context
            .get(parser.position)
            .map(|spanned| format!("{:?}", spanned.token))
            .unwrap_or_default();
        return Err(XPathError::syntax(format!(
            "表达式在第 {} 个记号处有未消费的剩余部分：{token}",
            parser.position + 1
        )));
    }

    // §3.3 Disambiguation：唯一路径与"过滤表达式本身是位置路径"的形态按节点集求值。
    let mode = if is_location_path_form(&expression) {
        ResultMode::NodeSet
    } else {
        ResultMode::Scalar
    };

    Ok(CompiledExpression {
        expression,
        mode,
        source_len: source.len(),
    })
}

/// 判断表达式是否按 §3.3 的 Disambiguation 规则"作为节点集求值"。
///
/// 依据 §3.3：
///
/// > If the expression is a LocationPath, or if it is a FilterExpr that is
/// > itself a LocationPath, then it is evaluated as a node-set.
///
/// 即唯一路径与"路径 + 谓词"两种形态。带显式过滤表达式的（`(a|b)/c`）
/// 不在此列——但它们的**结果**仍是节点集，`mode` 只影响入口怎么解释结果，
/// 故这里判 `Path { filter: None, .. }` 即可。
fn is_location_path_form(expression: &Expr) -> bool {
    matches!(expression, Expr::Path { filter: None, .. })
}

/// 递归下降解析器。
struct Parser<'a> {
    context: &'a ParseContext<'a>,
    /// 下一个待消费的记号下标
    position: usize,
    /// 当前递归深度（上界见 [`MAX_EXPRESSION_DEPTH`]）
    depth: usize,
}

impl<'a> Parser<'a> {
    // -----------------------------------------------------------------------
    // 记号访问
    // -----------------------------------------------------------------------

    /// 窥视第 `offset` 个待消费记号的 token 类型。
    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.context
            .get(self.position + offset)
            .map(|spanned| &spanned.token)
    }

    /// 当前 token 是否等于给定形态。
    fn check(&self, expected: &Token) -> bool {
        self.peek_at(0) == Some(expected)
    }

    /// 消费当前 token（调用方已确认形态）。
    fn advance(&mut self) -> Option<&'a SpannedToken> {
        let spanned = self.context.get(self.position)?;
        self.position += 1;
        Some(spanned)
    }

    /// 若当前 token 形态匹配则消费并返回 `true`。
    fn accept(&mut self, expected: &Token) -> bool {
        if self.check(expected) {
            self.position += 1;
            return true;
        }
        false
    }

    /// 要求当前 token 形态匹配，否则报语法错。
    fn expect(&mut self, expected: &Token, what: &str) -> Result<(), XPathError> {
        if self.accept(expected) {
            return Ok(());
        }
        Err(self.unexpected(what))
    }

    /// 当前 token 对应的源码片段（诊断用）。
    fn current_text(&self) -> &'a str {
        self.context
            .get(self.position)
            .map(|spanned| self.context.slice((spanned.start, spanned.end)))
            .unwrap_or("<表达式结束>")
    }

    /// 构造一个"期望 X、实际是 Y"的语法错误。
    fn unexpected(&self, expected: &str) -> XPathError {
        XPathError::syntax(format!("期望{expected}，实际是 `{}`", self.current_text()))
    }

    /// 进入一层递归。超出上界即报 `err:DEPTH` 语义的错误。
    ///
    /// 深度上界存在的理由与本项目其他上界一致（AGENTS.md 的生产级护栏）：
    /// 输入不可信，而递归深度由输入决定。这里给出**结构化错误**而非栈溢出。
    fn enter_depth(&mut self) -> Result<(), XPathError> {
        self.depth += 1;
        if self.depth > MAX_EXPRESSION_DEPTH {
            return Err(XPathError::limit(
                XPathErrorCode::DepthLimit,
                format!("表达式嵌套超过 {MAX_EXPRESSION_DEPTH} 层"),
            ));
        }
        Ok(())
    }

    /// 退出一层递归。
    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    // -----------------------------------------------------------------------
    // 表达式层次（§3.3 / §3.4 / §3.5）
    // -----------------------------------------------------------------------

    /// `OrExpr ::= AndExpr | OrExpr 'or' AndExpr`
    fn parse_or_expression(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_and_expression()?;
        while self.accept(&Token::Or) {
            let right = self.parse_and_expression()?;
            left = combined(BinaryOperator::Or, left, right);
        }
        Ok(left)
    }

    /// `AndExpr ::= EqualityExpr | AndExpr 'and' EqualityExpr`
    fn parse_and_expression(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_equality_expression()?;
        while self.accept(&Token::And) {
            let right = self.parse_equality_expression()?;
            left = combined(BinaryOperator::And, left, right);
        }
        Ok(left)
    }

    /// `EqualityExpr ::= RelationalExpr (('=' | '!=') RelationalExpr)*`
    fn parse_equality_expression(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_relational_expression()?;
        loop {
            let operator = match self.peek_at(0) {
                Some(Token::Equal) => BinaryOperator::Equal,
                Some(Token::NotEqual) => BinaryOperator::NotEqual,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.parse_relational_expression()?;
            left = combined(operator, left, right);
        }
    }

    /// `RelationalExpr ::= AdditiveExpr (('<' | '>' | '<=' | '>=') AdditiveExpr)*`
    fn parse_relational_expression(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_additive_expression()?;
        loop {
            let operator = match self.peek_at(0) {
                Some(Token::Less) => BinaryOperator::Less,
                Some(Token::LessOrEqual) => BinaryOperator::LessOrEqual,
                Some(Token::Greater) => BinaryOperator::Greater,
                Some(Token::GreaterOrEqual) => BinaryOperator::GreaterOrEqual,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.parse_additive_expression()?;
            left = combined(operator, left, right);
        }
    }

    /// `AdditiveExpr ::= MultiplicativeExpr (('+' | '-') MultiplicativeExpr)*`
    fn parse_additive_expression(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_multiplicative_expression()?;
        loop {
            let operator = match self.peek_at(0) {
                Some(Token::Plus) => BinaryOperator::Add,
                Some(Token::Minus) => BinaryOperator::Subtract,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.parse_multiplicative_expression()?;
            left = combined(operator, left, right);
        }
    }

    /// `MultiplicativeExpr ::= UnaryExpr (('*' | 'div' | 'mod') UnaryExpr)*`
    fn parse_multiplicative_expression(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_unary_expression()?;
        loop {
            let operator = match self.peek_at(0) {
                Some(Token::Multiply) => BinaryOperator::Multiply,
                Some(Token::Divide) => BinaryOperator::Divide,
                Some(Token::Modulo) => BinaryOperator::Modulo,
                _ => return Ok(left),
            };
            self.advance();
            let right = self.parse_unary_expression()?;
            left = combined(operator, left, right);
        }
    }

    /// `UnaryExpr ::= UnionExpr | '-' UnaryExpr`
    ///
    /// 一元的 `-` 在 XPath 1.0 §3.5 里**没有**独立产生式，它由
    /// `0 - x` 的语义给出（规范正文的 "The `-` operator ... is defined as
    /// `0 - x`"）。故这里保留 [`Expr::Negate`] 变体但语义按减法实现。
    fn parse_unary_expression(&mut self) -> Result<Expr, XPathError> {
        if self.accept(&Token::Minus) {
            self.enter_depth()?;
            let operand = self.parse_unary_expression();
            self.leave_depth();
            return Ok(Expr::Negate(Box::new(operand?)));
        }
        self.parse_union_expression()
    }

    /// `UnionExpr ::= PathExpr ('|' PathExpr)*`（§3.5）
    fn parse_union_expression(&mut self) -> Result<Expr, XPathError> {
        let mut left = self.parse_path_expression()?;
        while self.accept(&Token::Union) {
            let right = self.parse_path_expression()?;
            left = Expr::Union(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    /// `PathExpr ::= LocationPath | FilterExpr (('/' | '//') RelativeLocationPath)?`
    ///
    /// 两条分支的区分是**语法层面**的（§3.3 的 Disambiguation）：
    /// 以 `/`、`//` 起头，或以（可选轴前缀的）节点测试起头，就是位置路径；
    /// 否则是过滤表达式。
    fn parse_path_expression(&mut self) -> Result<Expr, XPathError> {
        self.enter_depth()?;
        let result = self.parse_path_expression_inner();
        self.leave_depth();
        result
    }

    fn parse_path_expression_inner(&mut self) -> Result<Expr, XPathError> {
        // 绝对路径：`/` 或 `//` 起头
        if self.check(&Token::Slash) || self.check(&Token::DoubleSlash) {
            let path = self.parse_location_path(true)?;
            return Ok(Expr::Path { filter: None, path });
        }

        // 相对位置路径：`.`, `..`, `@`, `*`, 名字后跟 `::`
        if self.starts_relative_location_path() {
            let path = self.parse_location_path(false)?;
            return Ok(Expr::Path { filter: None, path });
        }

        // 过滤表达式
        let filter = self.parse_filter_expression()?;
        if self.accept(&Token::DoubleSlash) {
            // §3.3：`FilterExpr '//' RelativeLocationPath`。`//` 仍是
            // `/descendant-or-self::node()/` 的缩写，故展开步照样要插。
            let mut steps = Vec::new();
            self.push_expansion_step(&mut steps)?;
            steps.push(self.parse_step()?);
            return Ok(Expr::Path {
                filter: Some(Box::new(filter)),
                path: LocationPath {
                    absolute: false,
                    steps,
                },
            });
        }
        if !self.accept(&Token::Slash) {
            return Ok(filter);
        }
        // §3.3：过滤表达式之后的 `/` 是"相对位置路径"，其初值是过滤结果，
        // 故这里**不补** `self::node()` 步——路径的起点由求值期用 filter 的结果给出。
        let path = self.parse_location_path(false)?;
        Ok(Expr::Path {
            filter: Some(Box::new(filter)),
            path,
        })
    }

    /// 当前记号是否开启一条**相对位置路径**。
    ///
    /// 依据 §3.3：
    ///
    /// > If the character following an `@` ... is an NCName or `*`, it is a
    /// > relative location path; ... `..` is an AbbreviatedStep;
    /// > an NCName followed by `::` is an axis specifier.
    ///
    /// 反之，NCName 后跟 `(` 是函数调用（走过滤表达式分支），
    /// 后跟别的（运算符等）则不是路径。
    fn starts_relative_location_path(&self) -> bool {
        match self.peek_at(0) {
            Some(Token::Dot | Token::DotDot | Token::At | Token::Wildcard) => true,
            // 名字后跟 `::` 是具名轴；名字后跟 `(` 是扩展节点测试或函数调用，
            // 二者都要交给 `parse_location_path` 去分辨（如 `allText()` 是节点测试、
            // `contains()` 是函数）。
            Some(Token::Name(_)) => match self.peek_at(1) {
                Some(Token::DoubleColon) => true,
                Some(Token::LeftParen) => is_node_test_function(self.name_at(0)),
                _ => true,
            },
            _ => false,
        }
    }

    /// 取第 `offset` 个待消费记号若为名字则返回其文本。
    fn name_at(&self, offset: usize) -> &str {
        match self.peek_at(offset) {
            Some(Token::Name(name)) => name.as_str(),
            _ => "",
        }
    }

    /// `FilterExpr ::= PrimaryExpr Predicate*`（§3.4）
    fn parse_filter_expression(&mut self) -> Result<Expr, XPathError> {
        self.enter_depth()?;
        let result = self.parse_filter_expression_inner();
        self.leave_depth();
        result
    }

    fn parse_filter_expression_inner(&mut self) -> Result<Expr, XPathError> {
        let primary = self.parse_primary_expression()?;
        let predicates = self.parse_predicates()?;
        if predicates.is_empty() {
            return Ok(primary);
        }
        // 谓词作用在**任意**表达式上（§3.4 的 FilterExpr）。本实现把它
        // 表示成"零步的路径"：初值是 primary 的结果，谓词按位置过滤。
        Ok(Expr::Path {
            filter: Some(Box::new(primary)),
            path: LocationPath {
                absolute: false,
                steps: vec![Step {
                    axis: super::ast::Axis::SelfAxis,
                    node_test: NodeTest::AnyNode,
                    predicates,
                }],
            },
        })
    }

    /// `PrimaryExpr ::= VariableReference | '(' Expr ')' | Literal | Number | FunctionCall`
    ///
    /// 变量引用（`$x`）**不支持**：书源规则没有给 XPath 传变量的机制，
    /// 支持它只会造出一条永远取不到值的分支。词法层不产出 `$` 记号，
    /// 故这里会自然落到"期望主表达式"的语法错误上。
    fn parse_primary_expression(&mut self) -> Result<Expr, XPathError> {
        match self.peek_at(0) {
            Some(Token::Number(value)) => {
                let value = *value;
                self.advance();
                Ok(Expr::Literal(Value::Number(value)))
            }
            Some(Token::Literal(text)) => {
                let text = text.clone();
                self.advance();
                Ok(Expr::Literal(Value::String(text)))
            }
            Some(Token::LeftParen) => {
                self.advance();
                self.enter_depth()?;
                let inner = self.parse_or_expression();
                self.leave_depth();
                let inner = inner?;
                self.expect(&Token::RightParen, "`)`")?;
                Ok(inner)
            }
            Some(Token::Name(_)) => self.parse_function_call(),
            _ => Err(self.unexpected("表达式")),
        }
    }

    /// `FunctionCall ::= FunctionName '(' ( Argument ( ',' Argument )* )? ')'`
    fn parse_function_call(&mut self) -> Result<Expr, XPathError> {
        let name = self.name_at(0).to_string();
        self.advance();
        self.expect(&Token::LeftParen, "`(`")?;

        let mut arguments = Vec::new();
        if !self.check(&Token::RightParen) {
            loop {
                self.enter_depth()?;
                let argument = self.parse_or_expression();
                self.leave_depth();
                arguments.push(argument?);
                if !self.accept(&Token::Comma) {
                    break;
                }
            }
        }
        self.expect(&Token::RightParen, "`)`")?;
        Ok(Expr::FunctionCall { name, arguments })
    }

    // -----------------------------------------------------------------------
    // 位置路径与步
    // -----------------------------------------------------------------------

    /// `LocationPath ::= RelativeLocationPath | AbsoluteLocationPath`
    ///
    /// 依据 W3C XPath 1.0 §3.3 的产生式：
    ///
    /// ```text
    /// LocationPath ::= RelativeLocationPath | AbsoluteLocationPath
    /// AbsoluteLocationPath ::= '/' RelativeLocationPath? | AbbreviatedAbsoluteLocationPath
    /// AbbreviatedAbsoluteLocationPath ::= '//' RelativeLocationPath
    /// RelativeLocationPath ::= Step | RelativeLocationPath '/' Step
    ///                        | AbbreviatedRelativeLocationPath
    /// AbbreviatedRelativeLocationPath ::= RelativeLocationPath '//' Step
    /// ```
    ///
    /// 且 §2.5 给出 `//` 的规范等价：
    ///
    /// > `//` is short for `/descendant-or-self::node()/`
    ///
    /// 故 `//a` 解析出的步序列是 `[descendant-or-self::node(), child::a]`，
    /// 与手写展开式**逐字相同**——两者共用同一段求值代码，语义不可能分叉。
    ///
    /// ## `//` 的展开步在何时插入
    ///
    /// `descendant-or-self::node()` 是**分隔符的一部分**，不是可选的补充。
    /// 但 `//` 后面**必须有**一相步（`AbbreviatedRelativeLocationPath` 的
    /// 产生式里 `Step` 不可省），故实现顺序是"先确认后面有步，再插入展开步"——
    /// 反过来会在 `a//` 上先插一步再报错，产出的错误位置指向表达式末尾而
    /// 不是那个多余的 `//`。
    fn parse_location_path(&mut self, absolute: bool) -> Result<LocationPath, XPathError> {
        let mut steps = Vec::new();

        // 起头：`/` 或 `//` 之一。
        //
        // 依据 §3.3：`AbsoluteLocationPath ::= '/' RelativeLocationPath?`
        // 而 `AbbreviatedAbsoluteLocationPath ::= '//' RelativeLocationPath`
        // ——后者的 `RelativeLocationPath` **不可省**，故 `//` 之后没有步
        // 必须报错，而单个 `/` 之后没有步是合法的（路径只含根节点）。
        if absolute {
            if self.accept(&Token::DoubleSlash) {
                self.push_expansion_step(&mut steps)?;
                steps.push(self.parse_step()?);
            } else if self.accept(&Token::Slash) {
                if self.starts_step() {
                    steps.push(self.parse_step()?);
                } else {
                    return Ok(LocationPath { absolute, steps });
                }
            } else {
                // 调用方已确认以 `/` 或 `//` 起头，走到这里说明是内部不一致
                return Err(self.unexpected("路径的分隔符"));
            }
        } else {
            if !self.starts_step() {
                return Err(self.unexpected("位置路径的步"));
            }
            steps.push(self.parse_step()?);
        }

        // 后续的 `/` 与 `//` 分隔。
        loop {
            if self.accept(&Token::DoubleSlash) {
                self.push_expansion_step(&mut steps)?;
                steps.push(self.parse_step()?);
                continue;
            }
            if self.accept(&Token::Slash) {
                if !self.starts_step() {
                    return Err(self.unexpected("`/` 之后的步"));
                }
                steps.push(self.parse_step()?);
                continue;
            }
            break;
        }

        Ok(LocationPath { absolute, steps })
    }

    /// 插入 `//` 贡献的展开步（§2.5），并确认它后面确实有一相步。
    fn push_expansion_step(&mut self, steps: &mut Vec<Step>) -> Result<(), XPathError> {
        if !self.starts_step() {
            return Err(self.unexpected("`//` 之后的步"));
        }
        steps.push(abbreviated_descendant_or_self());
        Ok(())
    }

    /// 当前记号是否开启一相步。
    ///
    /// `Name` + `(` 的形态**算**步首——否则 `/contains(a,b)` 会在"步"这一层
    /// 就报"`/` 之后的步"这种指错方向的错，而真实原因是"这个函数名不能
    /// 作节点测试"。把它放进步里，由 [`Self::parse_node_test`] 给出**指名**
    /// 的诊断（"`contains()` 不能作为节点测试（可用：…）"）。
    ///
    /// 函数调用**不会**因此被误解析成路径：`parse_path_expression` 先用
    /// [`Self::starts_relative_location_path`] 分辨"整条表达式是路径还是
    /// 过滤表达式"，只有真正以 `/`、`//`、轴缩写或步起头的才进路径分支；
    /// 而本函数只在**已经确定身处路径之中**时使用（分隔符之后）。
    fn starts_step(&self) -> bool {
        matches!(
            self.peek_at(0),
            Some(
                Token::Dot
                    | Token::DotDot
                    | Token::At
                    | Token::Wildcard
                    | Token::Number(_)
                    | Token::Literal(_)
                    | Token::LeftParen
                    | Token::Name(_)
            )
        )
    }

    /// `Step ::= AxisSpecifier NodeTest Predicate* | AbbreviatedStep`
    fn parse_step(&mut self) -> Result<Step, XPathError> {
        let (axis, node_test) = self.parse_axis_and_node_test()?;
        let predicates = self.parse_predicates()?;
        Ok(Step {
            axis,
            node_test,
            predicates,
        })
    }

    /// 解析"轴 + 节点测试"。
    fn parse_axis_and_node_test(&mut self) -> Result<(super::ast::Axis, NodeTest), XPathError> {
        use super::ast::Axis;

        // 缩写步 `.`（self::node()）与 `..`（parent::node()）
        if self.accept(&Token::Dot) {
            return Ok((Axis::SelfAxis, NodeTest::AnyNode));
        }
        if self.accept(&Token::DotDot) {
            return Ok((Axis::Parent, NodeTest::AnyNode));
        }

        // 属性轴缩写 `@`
        if self.accept(&Token::At) {
            let node_test = self.parse_node_test()?;
            return Ok((Axis::Attribute, node_test));
        }

        // 具名轴 `name::`
        if let Some(Token::Name(_)) = self.peek_at(0) {
            if self.peek_at(1) == Some(&Token::DoubleColon) {
                let axis_name = self.name_at(0).to_string();
                let axis = Axis::parse(&axis_name)
                    .ok_or_else(|| XPathError::syntax(format!("未知的轴名 `{axis_name}::`")))?;
                self.advance();
                self.advance();
                let node_test = self.parse_node_test()?;
                return Ok((axis, node_test));
            }
        }

        // 默认轴是 `child`
        let node_test = self.parse_node_test()?;
        Ok((Axis::Child, node_test))
    }

    /// `NodeTest ::= NameTest | NodeType '(' ')' | ProcessingInstructionTest | 扩展节点测试`
    fn parse_node_test(&mut self) -> Result<NodeTest, XPathError> {
        if self.accept(&Token::Wildcard) {
            return Ok(NodeTest::Wildcard);
        }

        // 名字：可能是普通名字测试，也可能是 `text()` 这类节点类型测试，
        // 或书源生态的扩展节点测试 `allText()` / `html()` / `outerHtml()` / `num()`。
        let Some(Token::Name(_)) = self.peek_at(0) else {
            return Err(self.unexpected("节点测试"));
        };
        let name = self.name_at(0).to_string();
        let has_parentheses = self.peek_at(1) == Some(&Token::LeftParen);

        if !has_parentheses {
            self.advance();
            return Ok(name_test_from(&name));
        }

        if let Some(node_test) = node_type_test(&name) {
            self.advance();
            self.advance();
            self.expect(&Token::RightParen, "`)`")?;
            return Ok(node_test);
        }

        if let Some(node_test) = extension_node_test(&name) {
            self.advance();
            self.advance();
            self.expect(&Token::RightParen, "`)`")?;
            return Ok(node_test);
        }

        Err(XPathError::syntax(format!(
            "`{name}()` 不能作为节点测试（可用：text()/node()/comment()/\
             processing-instruction()/allText()/html()/outerHtml()/num()）"
        )))
    }

    /// `Predicate ::= '[' PredicateExpr ']'`
    fn parse_predicates(&mut self) -> Result<Vec<Predicate>, XPathError> {
        let mut predicates = Vec::new();
        while self.accept(&Token::LeftBracket) {
            let start = self
                .context
                .get(self.position)
                .map(|spanned| spanned.start)
                .unwrap_or_default();
            self.enter_depth()?;
            let expression = self.parse_or_expression();
            self.leave_depth();
            let expression = expression?;
            let end = self
                .context
                .get(self.position)
                .map(|spanned| spanned.start)
                .unwrap_or_default();
            self.expect(&Token::RightBracket, "`]`")?;
            predicates.push(Predicate {
                expression,
                span: (start, end),
            });
        }
        Ok(predicates)
    }
}

/// 构造一个二元表达式。
fn combined(operator: BinaryOperator, left: Expr, right: Expr) -> Expr {
    Expr::Unary {
        operator,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// 把名字拆成 [`NodeTest::Name`]（`ns:local` 的前缀是第 2 个 `:` 之前的部分，§3.7）。
fn name_test_from(name: &str) -> NodeTest {
    match name.split_once(':') {
        Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() => NodeTest::Name {
            prefix: Some(prefix.to_string()),
            local: local.to_string(),
        },
        _ => NodeTest::Name {
            prefix: None,
            local: name.to_string(),
        },
    }
}

/// 节点类型测试（§2.3 的四种，无自变量形态）。
fn node_type_test(name: &str) -> Option<NodeTest> {
    Some(match name {
        "text" => NodeTest::Text,
        "comment" => NodeTest::Comment,
        "processing-instruction" => NodeTest::ProcessingInstruction,
        "node" => NodeTest::AnyNode,
        _ => return None,
    })
}

/// 书源生态的扩展节点测试（依据见 [`NodeTest`] 各变体的文档）。
fn extension_node_test(name: &str) -> Option<NodeTest> {
    Some(match name {
        "allText" => NodeTest::AllText,
        "html" => NodeTest::Html,
        "outerHtml" => NodeTest::OuterHtml,
        "num" => NodeTest::Number,
        _ => return None,
    })
}

/// 名字 + `(` 的形态里，哪些是**节点测试**而非函数调用。
///
/// 这是 `parse_path_expression` 区分"位置路径"与"过滤表达式"的判据：
/// `/div/allText()` 的末段是节点测试，而 `count(...)` 是函数调用。
/// [UNCERTAIN] `text()` / `node()` 同时是"节点类型测试"与"扩展函数名"——
/// 本实现**一律**按节点测试解析（在 XPath 1.0 里 `text()` 本就只有
/// 节点测试一种含义，见 §2.3）。
fn is_node_test_function(name: &str) -> bool {
    node_type_test(name).is_some() || extension_node_test(name).is_some()
}

/// 展开 `//` 产生的那一相步：`descendant-or-self::node()`（§2.5）。
fn abbreviated_descendant_or_self() -> Step {
    Step::new(super::ast::Axis::DescendantOrSelf, NodeTest::AnyNode)
}

/// 词法模式在解析层的复用点：步首允许 `*` 作通配符。
///
/// 单独引一次 [`LexMode`] 使"词法模式由解析器按语法位置传入"这一契约
/// 在本模块可见（真正的传入发生在 `lexer::tokenize` 内部的
/// `mode_after`，它等价于 §3.7 的两条规则）。
const _LEX_MODE_MARKER: LexMode = LexMode::StepStart;

#[cfg(test)]
mod tests {
    use super::super::ast::Axis;
    use super::*;

    /// 便捷：解析并取出根表达式。
    fn parse(source: &str) -> Expr {
        compile(source).expect("应解析成功").expression
    }

    /// 便捷：断言解析失败且细节里含某段文字。
    fn parse_error(source: &str, contains: &str) {
        let error = compile(source).expect_err(&format!("`{source}` 应解析失败"));
        assert!(
            error.detail().contains(contains),
            "错误细节应含 `{contains}`，实际为 `{}`",
            error.detail()
        );
    }

    /// 便捷：把 `CompiledExpression` 的 mode 转成可读串。
    fn mode_of(source: &str) -> ResultMode {
        compile(source).expect("应解析成功").mode
    }

    // ---------- 位置路径 ----------

    #[test]
    fn 绝对路径与相对路径被区分() {
        match parse("/a/b") {
            Expr::Path { filter, path } => {
                assert!(filter.is_none());
                assert!(path.absolute, "以 `/` 起头是绝对路径");
                assert_eq!(path.steps.len(), 2);
                assert_eq!(path.steps[0].axis, Axis::Child);
            }
            other => panic!("应为路径表达式，实际 {other:?}"),
        }

        match parse("a//b") {
            Expr::Path { path, .. } => {
                assert!(!path.absolute);
                assert_eq!(path.steps.len(), 3, "`//` 展开成一相步");
                assert_eq!(path.steps[0].axis, Axis::Child);
                assert_eq!(path.steps[1].axis, Axis::DescendantOrSelf);
                assert_eq!(path.steps[1].node_test, NodeTest::AnyNode);
                assert_eq!(path.steps[2].axis, Axis::Child);
            }
            other => panic!("应为路径表达式，实际 {other:?}"),
        }
    }

    #[test]
    fn 双斜杠展开成descendant_or_self步() {
        // §2.5：`//a` 等价于 `/descendant-or-self::node()/a`。
        // 两者**解析出同一棵 AST**，故不可能出现语义分叉。
        assert_eq!(parse("//a"), parse("/descendant-or-self::node()/a"));
    }

    #[test]
    fn 十三种轴都能解析且缩写形态齐全() {
        let cases: &[(&str, Axis)] = &[
            ("child::a", Axis::Child),
            ("descendant::a", Axis::Descendant),
            ("parent::a", Axis::Parent),
            ("ancestor::a", Axis::Ancestor),
            ("following-sibling::a", Axis::FollowingSibling),
            ("preceding-sibling::a", Axis::PrecedingSibling),
            ("following::a", Axis::Following),
            ("preceding::a", Axis::Preceding),
            ("attribute::a", Axis::Attribute),
            ("self::a", Axis::SelfAxis),
            ("descendant-or-self::a", Axis::DescendantOrSelf),
            ("ancestor-or-self::a", Axis::AncestorOrSelf),
            ("namespace::a", Axis::Namespace),
        ];
        for (source, expected) in cases {
            match parse(source) {
                Expr::Path { path, .. } => {
                    assert_eq!(path.steps[0].axis, *expected, "`{source}` 的轴");
                }
                other => panic!("`{source}` 应为路径，实际 {other:?}"),
            }
        }

        // 缩写形态
        for (source, axis, node_test) in [
            (".", Axis::SelfAxis, NodeTest::AnyNode),
            ("..", Axis::Parent, NodeTest::AnyNode),
            ("@href", Axis::Attribute, NodeTest::AnyNode),
        ] {
            match parse(source) {
                Expr::Path { path, .. } => {
                    assert_eq!(path.steps[0].axis, axis, "`{source}` 的轴");
                    if source != "@href" {
                        assert_eq!(path.steps[0].node_test, node_test, "`{source}` 的测试");
                    }
                }
                other => panic!("`{source}` 应为路径，实际 {other:?}"),
            }
        }

        // `@href` 的节点测试是名字测试而非 `node()`
        match parse("@href") {
            Expr::Path { path, .. } => assert_eq!(
                path.steps[0].node_test,
                NodeTest::Name {
                    prefix: None,
                    local: "href".into()
                }
            ),
            other => panic!("应为路径，实际 {other:?}"),
        }
    }

    #[test]
    fn 反向轴被正确标记() {
        // §2.2：ancestor / ancestor-or-self / preceding / preceding-sibling
        // 是反向轴。这一位决定谓词位置怎么数，故单独断言。
        for reverse in [
            Axis::Ancestor,
            Axis::AncestorOrSelf,
            Axis::Preceding,
            Axis::PrecedingSibling,
        ] {
            assert!(reverse.is_reverse(), "{} 应为反向轴", reverse.name());
        }
        for forward in [
            Axis::Child,
            Axis::Descendant,
            Axis::DescendantOrSelf,
            Axis::Parent,
            Axis::Following,
            Axis::FollowingSibling,
            Axis::SelfAxis,
            Axis::Attribute,
            Axis::Namespace,
        ] {
            assert!(!forward.is_reverse(), "{} 应为正向轴", forward.name());
        }
    }

    #[test]
    fn 未知轴名报错且指出轴名() {
        parse_error("bogus::a", "bogus");
    }

    #[test]
    fn 节点测试的全部形态都能解析() {
        let cases: &[(&str, NodeTest)] = &[
            (
                "div",
                NodeTest::Name {
                    prefix: None,
                    local: "div".into(),
                },
            ),
            ("*", NodeTest::Wildcard),
            ("text()", NodeTest::Text),
            ("node()", NodeTest::AnyNode),
            ("comment()", NodeTest::Comment),
            ("processing-instruction()", NodeTest::ProcessingInstruction),
        ];
        for (source, expected) in cases {
            match parse(source) {
                Expr::Path { path, .. } => {
                    assert_eq!(path.steps[0].node_test, *expected, "`{source}` 的测试");
                }
                other => panic!("`{source}` 应为路径，实际 {other:?}"),
            }
        }

        // 带前缀的名字测试
        match parse("svg:rect") {
            Expr::Path { path, .. } => assert_eq!(
                path.steps[0].node_test,
                NodeTest::Name {
                    prefix: Some("svg".into()),
                    local: "rect".into()
                }
            ),
            other => panic!("应为路径，实际 {other:?}"),
        }

        // 属性通配 `@*`
        match parse("@*") {
            Expr::Path { path, .. } => {
                assert_eq!(path.steps[0].axis, Axis::Attribute);
                assert_eq!(path.steps[0].node_test, NodeTest::Wildcard);
            }
            other => panic!("应为路径，实际 {other:?}"),
        }
    }

    #[test]
    fn 书源扩展节点测试可作为路径末段() {
        // 兼容性关键点：`/div/allText()` 里 `allText()` 是**节点测试**，
        // 不是函数调用（依据见 NodeTest 各变体的文档）。
        for (source, expected) in [
            ("/div/allText()", NodeTest::AllText),
            ("/div/html()", NodeTest::Html),
            ("/div/outerHtml()", NodeTest::OuterHtml),
            ("/div/num()", NodeTest::Number),
        ] {
            match parse(source) {
                Expr::Path { path, .. } => {
                    assert_eq!(path.steps.len(), 2, "`{source}` 应有两相步");
                    assert_eq!(path.steps[1].node_test, expected, "`{source}` 的测试");
                }
                other => panic!("`{source}` 应为路径，实际 {other:?}"),
            }
        }

        // 扩展节点测试的产出形态是"计算值"而非节点集
        assert_eq!(
            NodeTest::AllText.step_result(),
            super::super::ast::StepResult::AllText
        );
        assert_eq!(
            NodeTest::Name {
                prefix: None,
                local: "a".into()
            }
            .step_result(),
            super::super::ast::StepResult::NodeSet
        );
    }

    #[test]
    fn 非节点测试的函数名不作节点测试() {
        // `contains()` 是函数而非节点测试——若把它当节点测试，
        // `contains(a,b)` 会被误解析成路径。
        parse_error("a/contains()", "不能作为节点测试");
    }

    // ---------- 谓词 ----------

    #[test]
    fn 谓词可串联且顺序保留() {
        match parse("div[1][@class='x']") {
            Expr::Path { path, .. } => {
                assert_eq!(path.steps[0].predicates.len(), 2, "两个谓词按序保留");
                assert_eq!(
                    path.steps[0].predicates[0].expression,
                    Expr::Literal(Value::Number(1.0))
                );
            }
            other => panic!("应为路径，实际 {other:?}"),
        }
    }

    #[test]
    fn 谓词里的逻辑与比较表达式能解析() {
        match parse("div[@class='x' and @id!='y']") {
            Expr::Path { path, .. } => match &path.steps[0].predicates[0].expression {
                Expr::Unary { operator, .. } => {
                    assert_eq!(*operator, BinaryOperator::And);
                }
                other => panic!("谓词应是与运算，实际 {other:?}"),
            },
            other => panic!("应为路径，实际 {other:?}"),
        }
    }

    #[test]
    fn 谓词缺闭合方括号时报错() {
        parse_error("div[1", "`]`");
    }

    // ---------- 表达式层次 ----------

    #[test]
    fn 算术与比较运算符的优先级正确() {
        // `1 + 2 * 3` 应是 `1 + (2 * 3)`：AdditiveExpr 在 MultiplicativeExpr 之上
        match parse("1 + 2 * 3") {
            Expr::Unary {
                operator,
                left,
                right,
            } => {
                assert_eq!(operator, BinaryOperator::Add);
                assert_eq!(*left, Expr::Literal(Value::Number(1.0)));
                match *right {
                    Expr::Unary { operator, .. } => {
                        assert_eq!(operator, BinaryOperator::Multiply);
                    }
                    other => panic!("右侧应是乘法，实际 {other:?}"),
                }
            }
            other => panic!("应为加法，实际 {other:?}"),
        }

        // `or` 低于 `and`：`a or b and c` 应是 `a or (b and c)`
        match parse("1 or 2 and 3") {
            Expr::Unary {
                operator, right, ..
            } => {
                assert_eq!(operator, BinaryOperator::Or);
                match *right {
                    Expr::Unary { operator, .. } => assert_eq!(operator, BinaryOperator::And),
                    other => panic!("右侧应是与运算，实际 {other:?}"),
                }
            }
            other => panic!("应为或运算，实际 {other:?}"),
        }
    }

    #[test]
    fn 运算符名按位置消歧为除法或元素名() {
        // §3.7：前一个记号不是运算符时，`div` 是运算符名
        match parse("6 div 2") {
            Expr::Unary { operator, .. } => assert_eq!(operator, BinaryOperator::Divide),
            other => panic!("应为除法，实际 {other:?}"),
        }
        // 轴缩写之后 `div` 是元素名
        match parse("//div") {
            Expr::Path { path, .. } => assert_eq!(
                path.steps[1].node_test,
                NodeTest::Name {
                    prefix: None,
                    local: "div".into()
                },
                "`//div` 的 `div` 是元素名（`//` 是起头记号，展开步在下标 0）"
            ),
            other => panic!("应为路径，实际 {other:?}"),
        }
    }

    #[test]
    fn 一元负号被解析且语义是零减() {
        match parse("-1") {
            Expr::Negate(inner) => assert_eq!(*inner, Expr::Literal(Value::Number(1.0))),
            other => panic!("应为一元负号，实际 {other:?}"),
        }
        // `- -1` 可嵌套
        match parse("- -1") {
            Expr::Negate(inner) => match *inner {
                Expr::Negate(_) => {}
                other => panic!("内层应为负号，实际 {other:?}"),
            },
            other => panic!("应为一元负号，实际 {other:?}"),
        }
    }

    #[test]
    fn 联合与分组括号() {
        match parse("a | b") {
            Expr::Union(left, right) => {
                assert!(matches!(*left, Expr::Path { .. }));
                assert!(matches!(*right, Expr::Path { .. }));
            }
            other => panic!("应为联合，实际 {other:?}"),
        }

        // `(a|b)[1]`：谓词作用在分组结果上
        match parse("(a|b)[1]") {
            Expr::Path { filter, path } => {
                assert!(matches!(filter.as_deref(), Some(Expr::Union(_, _))));
                assert_eq!(path.steps[0].predicates.len(), 1);
            }
            other => panic!("应为带过滤的路径，实际 {other:?}"),
        }
    }

    #[test]
    fn 函数调用与自变量() {
        match parse("contains(text(), 'x')") {
            Expr::FunctionCall { name, arguments } => {
                assert_eq!(name, "contains");
                assert_eq!(arguments.len(), 2);
                assert!(matches!(&arguments[0], Expr::Path { .. }));
                assert_eq!(arguments[1], Expr::Literal(Value::String("x".into())));
            }
            other => panic!("应为函数调用，实际 {other:?}"),
        }

        // 无自变量
        match parse("last()") {
            Expr::FunctionCall { name, arguments } => {
                assert_eq!(name, "last");
                assert!(arguments.is_empty());
            }
            other => panic!("应为函数调用，实际 {other:?}"),
        }
    }

    #[test]
    fn 过滤表达式后可接相对路径() {
        // §3.3 PathExpr 的第二分支：`FilterExpr '/' RelativeLocationPath`
        match parse("//a[1]/b") {
            Expr::Path { filter: None, path } => {
                assert_eq!(path.steps.len(), 3, "`//` 展开步 + a + b");
                assert_eq!(path.steps[0].axis, Axis::DescendantOrSelf);
            }
            other => panic!("应为纯位置路径，实际 {other:?}"),
        }
        match parse("(//a)[1]/b") {
            Expr::Path {
                filter: Some(_),
                path,
            } => {
                assert_eq!(path.steps.len(), 1);
                assert_eq!(
                    path.steps[0].node_test,
                    NodeTest::Name {
                        prefix: None,
                        local: "b".into()
                    }
                );
            }
            other => panic!("应为带过滤的路径，实际 {other:?}"),
        }
    }

    // ---------- 结果模式 ----------

    #[test]
    fn 唯一路径按节点集模式求值() {
        // §3.3 Disambiguation
        assert_eq!(mode_of("//a"), ResultMode::NodeSet);
        assert_eq!(mode_of("a/b"), ResultMode::NodeSet);
        assert_eq!(mode_of("//a[1]"), ResultMode::NodeSet);
        // 函数调用是标量（或由函数自身决定），走 Scalar 分支
        assert_eq!(mode_of("count(//a)"), ResultMode::Scalar);
        assert_eq!(mode_of("'abc'"), ResultMode::Scalar);
    }

    // ---------- 错误 ----------

    #[test]
    fn 空表达式报语法错() {
        parse_error("", "表达式为空");
        parse_error("   ", "表达式为空");
    }

    #[test]
    fn 未消费的剩余记号报错() {
        parse_error("a b)", "未消费");
    }

    #[test]
    fn 尾随斜杠缺步时报错() {
        parse_error("a/", "`/` 之后的步");
        parse_error("//", "`//` 之后的步");
        parse_error("a//", "`//` 之后的步");
        // 单个 `/` **合法**：§3.3 的 `AbsoluteLocationPath ::= '/' RelativeLocationPath?`
        // 允许省略相对路径，此时路径只取根节点。
        assert!(compile("/").is_ok(), "`/` 是合法表达式（只取根节点）");
    }

    #[test]
    fn 深层嵌套超过上界时报深度错误() {
        // 上界存在的理由见 enter_depth 的文档：输入不可信，而递归深度由输入决定。
        let deep = format!("{}1{}", "(".repeat(200), ")".repeat(200));
        let error = compile(&deep).expect_err("超过深度上界应报错");
        assert_eq!(error.code(), XPathErrorCode::DepthLimit);
    }

    #[test]
    fn 变量引用不被支持且报语法错() {
        // 书源规则没有给 XPath 传变量的机制。词法层不产出 `$` 记号，
        // 故这里落到"无法识别的字符"。
        parse_error("$x", "无法识别的字符");
    }

    #[test]
    fn 括号不配对时报错() {
        parse_error("(a", "`)`");
        parse_error("count(a", "`)`");
    }
}
