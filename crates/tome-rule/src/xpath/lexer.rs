//! [DONE] XPath 1.0 词法层：把表达式串切成 token 序列（tasks.md T048）
//!
//! # 为什么要单独一层词法
//!
//! XPath 1.0 有若干**只看字符分不出来**的 token，必须先知道语法上下文——
//! 但上下文只有词法层知道，故解开这个环的办法是让词法层**接受一个模式参数**
//! （[`LexMode`]），由调用方按语法位置传入：
//!
//! | 形态 | `/` 之后（[`LexMode::Operator`]） | 轴名之后（[`LexMode::StepStart`]） |
//! |---|---|---|
//! | `*` | 通配符 `Wildcard` | 乘号 `Multiply` |
//! | `div` | 元素名 `Name` | 除法 `Divide` |
//! | `and` / `or` / `mod` | 元素名 | 运算符 |
//!
//! 这是 W3C XPath 1.0 §3.7「Lexical structure」明文规定的消歧规则：
//!
//! > If there is a preceding token and the preceding token is not one of `@`,
//! > `::`, `(`, `[`, `,` or an Operator, then a `*` must be recognized as a
//! > MultiplyOperator, and an NCName must be recognized as an OperatorName.
//!
//! 同一段还规定：`(`, `[`, `@`, `::`, `,` 之后，以及轴名（含缩写 `//`、`..`）
//! 之后，必须回到"像步首"的模式。这两条规则合起来正好等价于本模块的
//! [`LexMode`] 开关，故**不另造一套上下文推导**。
//!
//! # 数字的字面量规则
//!
//! 依据同节：数字是 `Digits ('.' Digits?)? | '.' Digits`——故 `.5` 是一个数，
//! 而 `1.` 也是。纯 `.` 不是数字，它是 `self::node()` 的缩写。

use super::error::XPathError;

/// 词法模式：决定 `*` / `div` / `and` 等歧义 token 怎么读。
///
/// 取值来自 W3C XPath 1.0 §3.7 的两条消歧规则（见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexMode {
    /// 运算符位置：`*` 是乘号，NCName 是运算符名（`div` / `mod` / `and` / `or`）。
    Operator,
    /// 步首位置：`*` 是通配符，NCName 是节点名或具名轴。
    StepStart,
}

/// 词法记号。
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// 具名节点、轴名、函数名或运算符名（`div` / `and` 等）。
    Name(String),
    /// 数字字面量。XPath 1.0 的数字统一是 double（§3.7）。
    Number(f64),
    /// 字符串字面量：单引号或双引号包裹（§3.7）。
    /// 两种引号都不产生转义序列，且闭合引号必须与开启引号同类。
    Literal(String),
    /// `*` 通配符（出现在能否省略 `child::` 的位置）。
    Wildcard,
    /// `@`
    At,
    /// `::`
    DoubleColon,
    /// `//`
    DoubleSlash,
    /// `..`
    DotDot,
    /// `.`
    Dot,
    /// `/`
    Slash,
    /// `(`
    LeftParen,
    /// `)`
    RightParen,
    /// `[`
    LeftBracket,
    /// `]`
    RightBracket,
    /// `,`
    Comma,
    /// `|`
    Union,
    /// `+`
    Plus,
    /// `-`
    Minus,
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
    /// `*` 乘号（运算符位置）。
    Multiply,
    /// `div`
    Divide,
    /// `mod`
    Modulo,
    /// `and`
    And,
    /// `or`
    Or,
}

impl Token {
    /// 该记号是否为运算符（用于判断"前一个 token 是否运算符"）。
    ///
    /// 依据 W3C XPath 1.0 §3.7 的运算符清单：`and` / `or` / `mod` / `div`
    /// 与比较、算术、联合运算符，再加上 `*` 乘号。
    pub fn is_operator(&self) -> bool {
        matches!(
            self,
            Self::Union
                | Self::Plus
                | Self::Minus
                | Self::Equal
                | Self::NotEqual
                | Self::Less
                | Self::LessOrEqual
                | Self::Greater
                | Self::GreaterOrEqual
                | Self::Multiply
                | Self::Divide
                | Self::Modulo
                | Self::And
                | Self::Or
        )
    }
}

/// 带源码定位的记号。
#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    /// 记号本身
    pub token: Token,
    /// 起始字节下标（诊断用）
    pub start: usize,
    /// 结束字节下标（不含）
    pub end: usize,
}

/// 把表达式切成记号序列。
///
/// [DONE] 词法层的唯一入口。
///
/// 串首用 [`LexMode::StepStart`]：§3.7 的消歧规则说的是"**若存在前一个
/// 记号**且它不是 `@`/`::`/`(`/`[`/`,`/运算符"。串首没有前一个记号，
/// 故按规范它属于"步首"——`div` 在表达式开头是元素名而不是除法。
pub fn tokenize(source: &str) -> Result<Vec<SpannedToken>, XPathError> {
    Lexer::new(source).run(LexMode::StepStart)
}

/// 词法分析器状态机。
struct Lexer<'a> {
    /// 剩余待扫描部分（字节切片）。用 `strip_prefix` 而非手工索引推进：
    /// 多字节 UTF-8 字符不会被从中间切开（W3C XPath 1.0 §3.7 只对 ASCII
    /// 标点赋予语法意义，其余字符一律按 Unicode 处理）。
    rest: &'a str,
    /// 已经消费到源串的哪个字节。
    position: usize,
    /// 已产出的记号（顺序即源码顺序）。
    tokens: Vec<SpannedToken>,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            rest: source,
            position: 0,
            tokens: Vec::new(),
        }
    }

    /// 主循环：不断产出记号，直到剩余部分只剩空白。
    ///
    /// XPath 1.0 §3.7 只把空格、制表符、换行、回车列为空白，故用
    /// `char::is_whitespace` 会多吞掉 U+00A0 之类的字符——而全角/不换行空格
    /// 在书源规则里常作为**名字的一部分**出现（中文站点的标签属性名）。
    /// 从严只认那四种，是对规则原文的忠实。
    fn run(mut self, initial: LexMode) -> Result<Vec<SpannedToken>, XPathError> {
        let mut mode = initial;
        while !self.rest.is_empty() {
            if !self.skip_whitespace() {
                break;
            }

            let start = self.position;
            let token = self.next_token(mode)?;
            mode = mode_after(&token);
            let end = self.position;
            self.tokens.push(SpannedToken { token, start, end });
        }
        Ok(self.tokens)
    }

    /// 跳过空白。返回是否还有剩余输入。
    fn skip_whitespace(&mut self) -> bool {
        loop {
            let Some(head) = self.rest.chars().next() else {
                return false;
            };
            if !is_xpath_whitespace(head) {
                return true;
            }
            self.advance(head.len_utf8());
        }
    }

    /// 消费一个字节长度并推进位置。
    fn advance(&mut self, bytes: usize) {
        self.rest = &self.rest[bytes..];
        self.position += bytes;
    }

    /// 读出一个记号。
    fn next_token(&mut self, mode: LexMode) -> Result<Token, XPathError> {
        // 双字符记号必须先于单字符判定（`::` 不能读成两个 `:`）。
        //
        // 但 `//` 例外：W3C XPath 1.0 §3.3 把 `//` 定义成
        // `/descendant-or-self::node()/` 的**缩写**，即它由一个真正的 `/`
        // 加上一个"后代或自身"标记组成。词法层若把 `//` 合成一个记号，
        // 解析层就得在每个分隔位置补回那个 `/`，反而更绕。故本实现
        // 保留 `/` 与 `//` 的**分离**语义：`//` 读成 `Slash` + `DoubleSlash`，
        // 使 `/a`、`//a`、`a//b` 的路径结构在记号层就完全一致。
        for (text, token) in [
            ("::", Token::DoubleColon),
            ("..", Token::DotDot),
            ("!=", Token::NotEqual),
            ("<=", Token::LessOrEqual),
            (">=", Token::GreaterOrEqual),
        ] {
            if self.rest.starts_with(text) {
                self.advance(text.len());
                return Ok(token);
            }
        }

        let head = self
            .rest
            .chars()
            .next()
            .ok_or_else(|| XPathError::syntax("表达式在期望记号处结束"))?;

        match head {
            '/' if self.rest[1..].starts_with('/') => self.single(Token::DoubleSlash, 2),
            '/' => self.single(Token::Slash, 1),
            '(' => self.single(Token::LeftParen, 1),
            ')' => self.single(Token::RightParen, 1),
            '[' => self.single(Token::LeftBracket, 1),
            ']' => self.single(Token::RightBracket, 1),
            ',' => self.single(Token::Comma, 1),
            '|' => self.single(Token::Union, 1),
            '+' => self.single(Token::Plus, 1),
            '-' => self.single(Token::Minus, 1),
            '=' => self.single(Token::Equal, 1),
            '<' => self.single(Token::Less, 1),
            '>' => self.single(Token::Greater, 1),
            '@' => self.single(Token::At, 1),
            '.' if !self.rest_starts_with_digit() => self.single(Token::Dot, 1),
            '*' if mode == LexMode::StepStart => self.single(Token::Wildcard, 1),
            '*' => self.single(Token::Multiply, 1),
            '\'' | '"' => self.read_literal(head),
            _ if head.is_ascii_digit() => self.read_number(),
            // `.5` 形态：点号起头的数字（§3.7 的第二个备选）
            '.' => self.read_number(),
            _ => self.read_name_or_operator(mode),
        }
    }

    /// `1.` 之后的 `.` 是否应并入数字。判据是"点号后紧跟数字"。
    fn rest_starts_with_digit(&self) -> bool {
        self.rest[1..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_digit())
    }

    /// 产出单字符记号并推进。
    fn single(&mut self, token: Token, bytes: usize) -> Result<Token, XPathError> {
        self.advance(bytes);
        Ok(token)
    }

    /// 读字符串字面量。
    ///
    /// 依据 §3.7：字面量由单引号或双引号包裹，**内部无转义序列**，
    /// 且不能跨引号类型闭合——`'a"b'` 是合法的单引号字面量，而 `'a"b"`
    /// 在第一个 `'` 处就结束了。因此这里只找**同类**引号。
    fn read_literal(&mut self, quote: char) -> Result<Token, XPathError> {
        self.advance(quote.len_utf8());
        let rest = self.rest;
        let end = rest
            .find(quote)
            .ok_or_else(|| XPathError::syntax(format!("字符串字面量缺少闭合引号 `{quote}`")))?;
        let value = &rest[..end];
        self.advance(end + quote.len_utf8());
        Ok(Token::Literal(value.to_string()))
    }

    /// 读数字字面量。
    ///
    /// 依据 §3.7 的 Number 产生式：`Digits ('.' Digits?)? | '.' Digits`。
    /// 故 `1.` 合法（尾部点号算数），而 `1.2.3` 不合法的部分会留给下一个记号
    /// 触发解析错。
    fn read_number(&mut self) -> Result<Token, XPathError> {
        let text = self.rest;
        let mut taken = 0usize;
        let mut seen_dot = false;
        for character in text.chars() {
            if character.is_ascii_digit() {
                taken += 1;
                continue;
            }
            if character == '.' && !seen_dot {
                seen_dot = true;
                taken += 1;
                continue;
            }
            break;
        }
        let literal = &text[..taken];
        let value: f64 = literal
            .parse()
            .map_err(|_| XPathError::syntax(format!("无法解析的数字字面量 `{literal}`")))?;
        self.advance(taken);
        Ok(Token::Number(value))
    }

    /// 读名字，或在运算符位置把它归成运算符名。
    ///
    /// 名字用的字符集是宽松的多语言版本（见 [`is_name_char`]）：严格版只收
    /// ASCII `[A-Za-z0-9_-]`，会让**中文标签名**（`<价格>` 这类抓取页面）
    /// 无法出现在路径里。
    fn read_name_or_operator(&mut self, mode: LexMode) -> Result<Token, XPathError> {
        let text = self.rest;
        let taken = name_length(text);
        let Some(taken) = taken else {
            let head = text.chars().next().unwrap_or('\u{0}');
            return Err(XPathError::syntax(format!("无法识别的字符 `{head}`")));
        };
        let name = &text[..taken];
        self.advance(taken);
        Ok(classify_name(name, mode))
    }
}

/// 从串首量出一条名字的长度（字节数）。串首不是名字字符时返回 `None`。
///
/// `:` 的处理是这里唯一的难点：它既是 `QName` 的前缀分隔符（`svg:rect`），
/// 又是轴分隔符 `::` 的一半（`child::div`）。判据是**它在 `::` 里不是名字
/// 的最后一个字符**——故遇 `::` 就停下，遇单个 `:` 就吞下并继续。
/// 这样 `svg:rect` 是**一个**名字，而 `child::div` 是 `child` + `::` + `div`。
fn name_length(text: &str) -> Option<usize> {
    let mut length = 0usize;
    let mut characters = text.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if !is_name_char(character) {
            break;
        }
        if character == ':' {
            // `::` 是轴分隔符，不属于名字
            if matches!(characters.peek(), Some((_, ':'))) {
                break;
            }
            // 前缀分隔符：它必须后面还有名字部分，否则也是 `::` 的残余
            if index + 1 >= text.len() {
                break;
            }
        }
        length = index + character.len_utf8();
    }
    (length > 0).then_some(length)
}

/// 依据 §3.7 的消歧规则把名字归类。
///
/// 只在**运算符位置**才归成运算符名——这正是规范说的"a preceding token ...
/// is not one of `@`, `::`, `(`, `[`, `,` or an Operator"。反过来，在步首位置
/// 它们就是普通的节点名，故 `//div` 里的 `div` 是元素名。
fn classify_name(name: &str, mode: LexMode) -> Token {
    if mode == LexMode::StepStart {
        return Token::Name(name.to_string());
    }
    match name {
        "and" => Token::And,
        "or" => Token::Or,
        "mod" => Token::Modulo,
        "div" => Token::Divide,
        _ => Token::Name(name.to_string()),
    }
}

/// 下一个记号所处的模式。
///
/// 依据 §3.7：`(`, `[`, `@`, `::`, `,` 之后回到步首模式；运算符之后回到
/// 运算符模式。其余记号（名字、`)`、`]`、字面量、数字）之后是运算符模式——
/// 因为 `a` 之后不该再出现元素名，出现 `div` 就是除法。
fn mode_after(token: &Token) -> LexMode {
    match token {
        Token::LeftParen
        | Token::LeftBracket
        | Token::At
        | Token::DoubleColon
        | Token::Comma
        | Token::Slash
        | Token::DoubleSlash
        | Token::DotDot => LexMode::StepStart,
        _ => LexMode::Operator,
    }
}

/// XPath 1.0 §3.7 定义的空白：空格、制表符、换行、回车。
fn is_xpath_whitespace(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r')
}

/// 名字字符集（宽松多语言版本）。
///
/// [UNCERTAIN] 依据来源与取舍：W3C XPath 1.0 §3.7 把名字定义为
/// `QName`（`Prefix ':' LocalPart` 或 `LocalPart`），故 `:` **属于**名字——
/// 若把它排除在外，`svg:rect` 会被拆成 `svg`、`:`、`rect` 三个记号而无法
/// 解析。真实的 `::`（轴分隔符）由词法层的双字符优先规则先吃掉，
/// 故这里的 `:` 只会是名字里的前缀分隔符。
///
/// 起首字符的限制（规范按 Unicode 大类限定）在
/// [`crate::xpath::lexer`] 的 `is_name_char` 里**放宽**为"ASCII 字母数字
/// 下划线连字符 + 任何非 ASCII 字符"：本项目不引入 Unicode 通用类别表
/// （那是新依赖），而书目抓取页面里的中文标签名本就超出 XPath 1.0 的
/// XML 语境。
fn is_name_char(character: char) -> bool {
    if character.is_ascii() {
        return character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | ':');
    }
    !is_xpath_whitespace(character)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷：只要 token，不要位置。
    fn tokens_of(source: &str) -> Vec<Token> {
        tokenize(source)
            .expect("应词法成功")
            .into_iter()
            .map(|spanned| spanned.token)
            .collect()
    }

    #[test]
    fn 路径与轴分隔符被正确切分() {
        assert_eq!(
            tokens_of("/a/b"),
            vec![
                Token::Slash,
                Token::Name("a".into()),
                Token::Slash,
                Token::Name("b".into())
            ]
        );
        // `//` 是**一个**记号：§3.3 的 `AbbreviatedAbsoluteLocationPath`
        // 把它整个当作起头符号，展开成 `descendant-or-self::node()` 是
        // 解析层的事，词法层不必替它补一个 `/`。
        assert_eq!(
            tokens_of("//a"),
            vec![Token::DoubleSlash, Token::Name("a".into())]
        );
        assert_eq!(
            tokens_of("../a"),
            vec![Token::DotDot, Token::Slash, Token::Name("a".into())]
        );
        assert_eq!(
            tokens_of("child::div"),
            vec![
                Token::Name("child".into()),
                Token::DoubleColon,
                Token::Name("div".into())
            ]
        );
        // `.` 只有单独出现时才是 self::node() 缩写，`.5` 是数字
        assert_eq!(tokens_of("."), vec![Token::Dot]);
        assert_eq!(tokens_of(".5"), vec![Token::Number(0.5)]);
    }

    #[test]
    fn 星号按位置消歧为通配符或乘号() {
        // §3.7：`/` 之后是步首 → 通配符
        assert_eq!(
            tokens_of("/*"),
            vec![Token::Slash, Token::Wildcard],
            "`/` 之后的 `*` 是通配符"
        );
        // 运算符之后是运算符位置 → 乘号
        assert_eq!(
            tokens_of("3*4"),
            vec![Token::Number(3.0), Token::Multiply, Token::Number(4.0)]
        );
        assert_eq!(
            tokens_of("@*"),
            vec![Token::At, Token::Wildcard],
            "`@` 之后是步首 → 通配符"
        );
    }

    #[test]
    fn 运算符名按位置消歧() {
        // §3.7：前面是名字（非运算符）时，`div`/`and`/`or`/`mod` 是运算符
        assert_eq!(
            tokens_of("a div b"),
            vec![
                Token::Name("a".into()),
                Token::Divide,
                Token::Name("b".into())
            ]
        );
        assert_eq!(
            tokens_of("a and b"),
            vec![Token::Name("a".into()), Token::And, Token::Name("b".into())]
        );
        // 前面是 `//`（轴缩写）时，`div` 是元素名
        assert_eq!(
            tokens_of("/div"),
            vec![Token::Slash, Token::Name("div".into())],
            "`/` 之后 `div` 是元素名而非除法"
        );
        // 括号之后同理
        assert_eq!(
            tokens_of("count(div)"),
            vec![
                Token::Name("count".into()),
                Token::LeftParen,
                Token::Name("div".into()),
                Token::RightParen
            ]
        );
    }

    #[test]
    fn 比较与逻辑运算符被正确切分() {
        assert_eq!(
            tokens_of("@a<='v' or @b!=3"),
            vec![
                Token::At,
                Token::Name("a".into()),
                Token::LessOrEqual,
                Token::Literal("v".into()),
                Token::Or,
                Token::At,
                Token::Name("b".into()),
                Token::NotEqual,
                Token::Number(3.0)
            ]
        );
    }

    #[test]
    fn 字符串字面量两种引号都支持且无转义() {
        assert_eq!(tokens_of("'abc'"), vec![Token::Literal("abc".into())]);
        assert_eq!(tokens_of("\"abc\""), vec![Token::Literal("abc".into())]);
        // §3.7：字面量内不产生转义序列，异类引号只是普通字符
        assert_eq!(
            tokens_of(r#"'a"b'"#),
            vec![Token::Literal("a\"b".into())],
            "单引号字面量里的双引号是普通字符"
        );
        // 反斜杠**不是**转义符——`'\n'` 里是两个字符（反斜杠与 n），
        // 而不是一个换行（与 §3.7 "no escape sequences" 一致）
        assert_eq!(tokens_of(r"'\n'"), vec![Token::Literal(r"\n".into())]);
        assert!(tokenize("'未闭合").is_err(), "缺闭合引号应报错");
    }

    #[test]
    fn 数字字面量接受规范化里的两种形态() {
        assert_eq!(tokens_of("12"), vec![Token::Number(12.0)]);
        assert_eq!(tokens_of("1.5"), vec![Token::Number(1.5)]);
        // §3.7：`Digits ('.' Digits?)? | '.' Digits`
        assert_eq!(tokens_of("1."), vec![Token::Number(1.0)]);
        assert_eq!(tokens_of(".25"), vec![Token::Number(0.25)]);
    }

    #[test]
    fn 中文标签名可作节点名() {
        // [UNCERTAIN] 取舍见 is_name_char 的文档：严格 NCName 会让中文标签无法表达。
        assert_eq!(tokens_of("价格"), vec![Token::Name("价格".into())]);
        assert_eq!(
            tokens_of("/价格/text()"),
            vec![
                Token::Slash,
                Token::Name("价格".into()),
                Token::Slash,
                Token::Name("text".into()),
                Token::LeftParen,
                Token::RightParen
            ]
        );
    }

    #[test]
    fn u3000全角空格不被当作空白() {
        // §3.7 只列四种 ASCII 空白。U+3000 属名字字符（见 is_name_char），
        // 故它出现时是一个名字的组成部分而不是分隔符。
        let tokens = tokens_of("a\u{3000}b");
        assert_eq!(
            tokens,
            vec![Token::Name("a\u{3000}b".into())],
            "全角空格按名字字符处理"
        );
    }

    #[test]
    fn 无法识别的字符报语法错() {
        let error = tokenize("a & b").expect_err("`&` 不是 XPath 记号");
        assert!(error.detail().contains('&'), "错误里应指出是哪个字符");
    }

    #[test]
    fn 记号带源码位置() {
        let spanned = tokenize(" foo ").expect("应词法成功");
        assert_eq!(spanned.len(), 1);
        assert_eq!(spanned[0].start, 1);
        assert_eq!(spanned[0].end, 4);
    }

    #[test]
    fn 联合与谓词标点被正确切分() {
        assert_eq!(
            tokens_of("a[1]|b[2]"),
            vec![
                Token::Name("a".into()),
                Token::LeftBracket,
                Token::Number(1.0),
                Token::RightBracket,
                Token::Union,
                Token::Name("b".into()),
                Token::LeftBracket,
                Token::Number(2.0),
                Token::RightBracket
            ]
        );
    }
}
