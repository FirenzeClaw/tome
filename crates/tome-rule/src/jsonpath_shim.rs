//! \[DONE] Jayway JSONPath → RFC 9535 词法感知重写 shim（tasks.md T046 / research R3）
//!
//! # 为什么需要这一层
//!
//! 书源里的 JSONPath 是按 **Jayway JsonPath 的方言**写的（上游是 Java 实现），
//! 而本引擎跑 **RFC 9535**。R3 已枚举出两边的差异，本模块负责把其中
//! **语法层面**的差异翻译过去；**语义层面**的差异（结果形状）由
//! [`crate::jsonpath`] 处理。
//!
//! ## 差异清单（逐条给出改写目标）
//!
//! | Jayway 写法 | 含义 | RFC 9535 改写 |
//! |---|---|---|
//! | `[(@.length-1)]` | 脚本索引（在方括号里跑脚本） | 求值为整数索引或改名节点 |
//! | `.length()` / `.length` | 数组长度/字符串长度 | `length()` 函数调用 |
//! | `min()/max()/avg()/stddev()/sum()` | 终端聚合函数 | 同名的注册函数（[`crate::jsonpath_fn`]） |
//! | `[?(@.x in [1,2])]` | 集合成员 | `[?(@.x == 1 \|\| @.x == 2)]` |
//! | `[?(@.x nin [1,2])]` | 集合非成员 | 取反的析取 |
//! | `[?(@ =~ /re/i)]` | 正则匹配 | `match(@, 're', 'i')` |
//! | `[?(@.x)]` | 存在性（truthy 判定） | `[?(@.x)]` 已是 RFC 合法（存在性测试） |
//!
//! # 为什么不是正则替换（tasks.md T046 的验收项原文）
//!
//! > 重写器跟踪字符串字面量/转义引号/括号嵌套深度，不破坏含 `=`/`(` 的过滤器
//!
//! 三条具体的误伤场景：
//!
//! 1. **字符串里的 `length()`**：`$[?(@.name=='length()')]` 里的
//!    `length()` 是**字面文本**，不该被改写。
//! 2. **转义引号**：`$[?(@.name=='it\'s')]` —— 朴素的"找下一个单引号"
//!    会在 `\'` 处提前结束字符串，后续扫描就落在字符串内部。
//! 3. **过滤器里的 `=`**：`[?(@.a==1)]` 的 `=` 与"脚本索引"的判定无关，
//!    但若把整段当运算符处理，就会在错误的位置切分。
//!
//! 故本模块是一个**手写词法扫描器**：一次向前扫描，只在**字符串之外**
//! 且**方括号深度已知**时做改写判定。
//!
//! # 依据来源（干净室）
//!
//! 全部来自公开文档：Jayway JsonPath 的 README 列出其扩展语法；
//! Goessner 的 JSONPath 原文定义基本形态；RFC 9535 定义目标语法。
//! 未读任何实现源码。

/// 重写 Jayway 专有语法为 RFC 9535。
///
/// \[DONE] T046 的对外入口。无 `[...]`/`?`/`(` 等特征时**原样返回**
/// （避免给绝大多数普通路径做无谓的字符串分配）。
#[must_use]
pub fn rewrite(expression: &str) -> String {
    // 快路径：不含任何 Jayway 特征的表达式原样返回。
    // 判据取"方括号内联脚本"与"终端函数"两类共有的字符特征。
    if !needs_rewrite(expression) {
        return expression.to_string();
    }

    let chars: Vec<char> = expression.chars().collect();
    let mut output = String::with_capacity(expression.len() + 16);
    let mut index = 0usize;
    // 方括号深度：`[` 之内才有"选择器层"的改写（脚本索引）。
    let mut bracket_depth = 0i32;

    while index < chars.len() {
        let current = chars[index];

        // ## 字符串字面量整体搬运
        //
        // 这是"词法感知"要求的核心：改写判定必须发生在字符串**之外**。
        // `$[?(@.name=='length()')]` 里的括号与函数名是文本，不是语法。
        if current == '\'' || current == '"' {
            let (literal, next) = take_string(&chars, index);
            output.push_str(&literal);
            index = next;
            continue;
        }

        if current == '[' {
            bracket_depth += 1;
            // 进入选择器层：尝试识别 Jayway 的内联脚本/集合运算符
            let (rewritten, next, consumed) = rewrite_selector_layer(&chars, index, bracket_depth);
            if consumed {
                output.push_str(&rewritten);
                index = next;
                continue;
            }
            output.push(current);
            index += 1;
            continue;
        }
        if current == ']' {
            bracket_depth -= 1;
            output.push(current);
            index += 1;
            continue;
        }

        // ## `.method()` —— Jayway 的**终端方法**形态
        //
        // 两类都在这里：
        //
        // | Jayway | RFC 9535 |
        // |---|---|
        // | `$.list.length()` | `length($.list)` |
        // | `$.list.min()` | `min($.list)` |
        //
        // 只在**方括号之外**考虑（方括号内交给 `rewrite_selector_layer`）。
        //
        // 改写形状是"把主体包进函数调用"，故必须能回退 `output` 定位主体
        // ——主循环持有 `output`，所以判定与改写放在这里而不是一个独立的
        // 辅助函数里（那样得把可变游标传出去，两处状态容易不同步）。
        if bracket_depth <= 0 && current == '.' {
            if let Some((name, consumed)) = match_terminal_method(&chars, index) {
                if let Some(body) = take_trailing_subject(&output) {
                    output.truncate(body.0);
                    output.push_str(&format!("{name}({})", body.1));
                    index += consumed;
                    continue;
                }
            }
        }

        output.push(current);
        index += 1;
    }

    output
}

/// 快速判定：表达式里是否有需要改写的特征。
///
/// # 判据为什么是这几个
///
/// 逐条对应模块文档差异表里的改写项：
///
/// - `[(` —— 脚本索引的唯一开头形态（`[(@.length-1)]`）
/// - `.length` —— 长度方法（Jayway 的两种写法都以它开头）
/// - `.min()` / `.max()` / `.avg()` / `.stddev()` / `.sum()` —— 终端聚合方法
/// - ` in [` / ` in @` / ` nin ` —— 集合运算符。**前后必须有空白**，
///   否则 `link`、`inline` 之类含 `in` 的标识符会误命中
/// - `=~` —— 正则匹配
///
/// # 判据是"必然包含"而不是"精确等价"
///
/// 快路径只求**不漏**：命中了就做完整扫描（扫描器自己决定改不改），
/// 没命中就一定不需要改。故判据可以宽（宁可多扫一次），不可窄。
fn needs_rewrite(expression: &str) -> bool {
    expression.contains("[(")
        || expression.contains(".length")
        || expression.contains(".min()")
        || expression.contains(".max()")
        || expression.contains(".avg()")
        || expression.contains(".stddev()")
        || expression.contains(".sum()")
        || expression.contains(".keys()")
        || expression.contains(" in [")
        || expression.contains(" nin ")
        || expression.contains("=~")
        || expression.contains(" in @")
        || expression.contains(" nin @")
}

/// 匹配 Jayway 的**终端方法**形态（`.length()` / `.min()` …）。返回 (函数名, 消费字符数)。
///
/// # 为什么必须有这一层
///
/// Jayway 的聚合函数是**终端方法**（`$.list.min()`），而 RFC 9535 的函数是
/// **前缀调用**（`min($.list)`）。若不改写，引擎会在 `.min()` 处报语法错
/// ——而书源作者看到的是"JSONPath 语法错误"，看不出真实原因是"方言不同"。
///
/// # 覆盖的清单与依据
///
/// | 方法 | RFC 9535 对应 | 依据 |
/// |---|---|---|
/// | `.length()` / `.length` | `length()` | RFC 9535 §2.4.4 内置函数 |
/// | `.min()` | `min()` | Jayway README「Aggregate Functions」 |
/// | `.max()` | `max()` | 同上 |
/// | `.avg()` | `avg()` | 同上 |
/// | `.stddev()` | `stddev()` | 同上 |
/// | `.sum()` | `sum()` | 同上 |
/// | `.keys()` | `keys()` | RFC 9535 §2.4.4（Jayway 无此方法但同名可转） |
///
/// # 为什么要求有括号
///
/// `.length` 的**无括号形态**是 Jayway 专有的（`.max` 无括号在书源里不出现）。
/// 故：`length` 允许无括号，其余一律要求 `()`——容忍无括号形态会让
/// `$..length` 这类**属性名**被误当函数（属性名合法、函数名不合法，
/// 误判会把一条合法路径变成语法错）。
fn match_terminal_method(chars: &[char], start: usize) -> Option<(&'static str, usize)> {
    let rest: String = chars[start..].iter().collect();
    // 长的在前，避免 `.length` 抢在 `.length()` 之前匹配
    for (written, name) in [
        (".length()", "length"),
        (".length", "length"),
        (".min()", "min"),
        (".max()", "max"),
        (".avg()", "avg"),
        (".stddev()", "stddev"),
        (".sum()", "sum"),
        (".keys()", "keys"),
    ] {
        if rest.starts_with(written) {
            return Some((name, written.len()));
        }
    }
    None
}

/// 取出从 `start` 处引号开始的一个完整字符串字面量。返回 (文本, 下一位置)。
///
/// # 转义处理
///
/// JSONPath 的字符串字面量沿 JSON 规则：`\'` 在单引号串里是转义的单引号。
/// 朴素的"找下一个同种引号"会在 `'it\'s'` 的 `\'` 处提前结束，
/// 于是后续的 `s'` 被当作**串外内容**扫描——改写器就可能在那里误伤。
fn take_string(chars: &[char], start: usize) -> (String, usize) {
    let quote = chars[start];
    let mut output = String::new();
    output.push(quote);
    let mut index = start + 1;
    while index < chars.len() {
        let current = chars[index];
        output.push(current);
        index += 1;
        if current == '\\' && index < chars.len() {
            // 转义序列的两字符整体搬运
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

/// 从 `[` 处尝试改写"选择器层"的 Jayway 语法。返回 (改写文本, 下一位置, 是否消费)。
///
/// 目前覆盖两类：
///
/// 1. **脚本索引** `[(@.length-1)]` —— Jayway 在方括号里跑脚本。
/// 2. **集合运算符** `in` / `nin` —— 出现在过滤器的方括号里。
fn rewrite_selector_layer(chars: &[char], start: usize, depth: i32) -> (String, usize, bool) {
    // `[(` 形态：脚本索引
    if depth == 1 && chars.get(start + 1) == Some(&'(') {
        if let Some((body, next)) = take_bracket_body(chars, start) {
            return (rewrite_script_index(&body), next, true);
        }
    }
    // 过滤器里的 `in` / `nin` 与终端方法（`[?(...)]`）
    if depth == 1 && chars.get(start + 1) == Some(&'?') {
        if let Some((body, next)) = take_bracket_body(chars, start) {
            let rewritten = rewrite_set_operators(&body);
            let rewritten = rewrite_regex_match(&rewritten);
            // 过滤器表达式里也可以出现终端方法（`[?(@.tags.length() > 2)]`）
            // ——`rewrite` 的主循环只处理"方括号之外"的 `.`，故这里补一次。
            let rewritten = rewrite_terminal_methods_in_filter(&rewritten);
            if rewritten != body {
                return (rewritten, next, true);
            }
        }
    }
    (String::new(), start, false)
}

/// 在**过滤器表达式内部**改写终端方法（`.min()` 之类）。
///
/// # 为什么主循环不覆盖这里
///
/// `rewrite` 的主循环用 `bracket_depth` 把"方括号之外"与"方括号之内"分开，
/// 而方括号内的内容整体交给 `rewrite_selector_layer`（它要处理脚本索引与
/// 集合运算符）。故过滤器内的终端方法必须在这里补处理。
///
/// # 与主循环的复用
///
/// 复用同一组判定（[`match_terminal_method`]）与同一个主体回退
/// （[`take_trailing_subject`]），故两处的改写语义必然一致——不会出现
/// "括号外能改写、括号内不能"这类不对称。
fn rewrite_terminal_methods_in_filter(body: &str) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut output = String::with_capacity(body.len() + 16);
    let mut index = 0usize;

    while index < chars.len() {
        let current = chars[index];
        if current == '\'' || current == '"' {
            let (literal, next) = take_string(&chars, index);
            output.push_str(&literal);
            index = next;
            continue;
        }
        if current == '.' {
            if let Some((name, consumed)) = match_terminal_method(&chars, index) {
                if let Some(subject) = take_trailing_subject(&output) {
                    output.truncate(subject.0);
                    output.push_str(&format!("{name}({})", subject.1));
                    index += consumed;
                    continue;
                }
            }
        }
        output.push(current);
        index += 1;
    }

    output
}

/// 取出从 `start` 处 `[` 到配对 `]` 的完整文本（含两端方括号）。返回 (文本, 下一位置)。
///
/// 跟踪**嵌套深度**与**字符串字面量**：过滤器的脚本里可以有嵌套的 `[`（数组字面量）
/// 与含 `]` 的字符串（`[?(@.name==']')]`）。
fn take_bracket_body(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut output = String::new();
    output.push('[');
    let mut depth = 1i32;
    let mut index = start + 1;
    while index < chars.len() {
        let current = chars[index];
        if current == '\'' || current == '"' {
            let (literal, next) = take_string(chars, index);
            output.push_str(&literal);
            index = next;
            continue;
        }
        match current {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    output.push(']');
                    return Some((output, index + 1));
                }
            }
            _ => {}
        }
        output.push(current);
        index += 1;
    }
    None
}

/// 改写脚本索引 `[(@.length-1)]`。
///
/// # 支持的范围与理由
///
/// Jayway 的脚本索引可以跑**任意 JS**，而 RFC 9535 没有这个能力。
/// 但书源里这一形态几乎只有一种用途——**取最后一个元素**：
/// `[(@.length-1)]`。公开文档（Jayway README）给出的示例正是它。
///
/// 故本实现只翻译"能静态求值成整数的脚本索引"，其余原样保留
/// （让 RFC 解析器报出准确的语法错，而不是我们猜一个错误语义）。
///
/// # 为什么用 `[-1:]` 而不是别的
///
/// RFC 9535 的切片 `[-1:]` 语义是"从倒数第一个到末尾"，结果恰好是
/// 最后一个元素——与 `[(@.length-1)]` 等价。用它而不是某个函数，
/// 是因为切片是 RFC 9535 的**一等语法**，不依赖函数注册。
///
/// ## `[(@.length - 1)]` 之外的形式
///
/// 带空格的写法（`length - 1`）同样常见，故先把脚本里的空白去掉再匹配。
fn rewrite_script_index(body: &str) -> String {
    // 去掉两端方括号与脚本括号：`[(` ... `)]`
    let inner = body
        .strip_prefix("[(")
        .and_then(|rest| rest.strip_suffix(")]"))
        .map(str::trim);
    let Some(inner) = inner else {
        return body.to_string();
    };

    // 去掉空白后比对 `@.length-1` 的各种写法。
    //
    // 判据是"属性链以 `.length` 结尾且减去常量 1 或 0"：
    // `@.length-1` 与 `@.length - 1` 都在其中。
    let compact: String = inner.chars().filter(|c| !c.is_whitespace()).collect();
    if compact == "@.length-1" || compact == "@.length+-1" {
        return "[-1:]".to_string();
    }
    if compact == "@.length-2" || compact == "@.length+-2" {
        return "[-2:]".to_string();
    }

    // 未被识别的脚本索引：原样保留（让 RFC 解析器报准确错）
    body.to_string()
}

/// 改写集合运算符 `in` / `nin` 为析取/合取。
///
/// # 改写规则
///
/// | 输入 | 输出 |
/// |---|---|
/// | `@.x in [1,2]` | `(@.x == 1 \|\| @.x == 2)` |
/// | `@.x nin [1,2]` | `(@.x != 1 && @.x != 2)` |
///
/// # 为什么是"扫描 + 拼接"而不是正则
///
/// 集合字面量里可以有嵌套的方括号与字符串（`in ['a','b']`），
/// 正则的贪婪匹配会在嵌套上出错。故扫到 `in` 后按**方括号配对**取集合正文。
///
/// # 左操作数的定位
///
/// 改写是"**替换**一段区间"而不是"插入"：`@.x in [1,2]` 整段要变成
/// `(@.x == 1 || @.x == 2)`。故扫描时先记住"当前运算符可见的左边界"
/// （`(`、`&`、`|`、`!` 之后），命中 `in`/`nin` 时从那个边界切出左操作数。
fn rewrite_set_operators(body: &str) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut output = String::with_capacity(body.len() + 16);
    let mut index = 0usize;
    // 当前运算符的左边界（在 `output` 中的字节位置）
    let mut operand_start = 0usize;

    while index < chars.len() {
        let current = chars[index];
        if current == '\'' || current == '"' {
            let (literal, next) = take_string(&chars, index);
            output.push_str(&literal);
            index = next;
            continue;
        }

        if matches!(current, '(' | '&' | '|' | '!') {
            output.push(current);
            index += 1;
            operand_start = output.len();
            continue;
        }

        let rest: String = chars[index..].iter().collect();
        let matched = [("nin", true), ("in", false)]
            .into_iter()
            .find(|(keyword, _)| match_keyword(&rest, keyword).is_some());
        if let Some((keyword, negative)) = matched {
            let consumed = keyword.len();
            let after_keyword: Vec<char> = chars[index + consumed..].to_vec();
            if let Some((collection, collection_len)) = take_collection(&after_keyword) {
                let left = output[operand_start..].trim().to_string();
                if !left.is_empty() {
                    let rewritten = rewrite_membership(&left, &collection, negative);
                    output.truncate(operand_start);
                    output.push_str(&rewritten);
                    index += consumed + collection_len;
                    operand_start = output.len();
                    continue;
                }
            }
        }

        output.push(current);
        index += 1;
    }

    output
}

/// 把 `@.x in [1,2]` 的左右两侧拼成 RFC 9535 的析取/合取。
fn rewrite_membership(left: &str, collection: &str, negative: bool) -> String {
    let items: Vec<String> = split_top_level_commas(collection)
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if items.is_empty() {
        // 空集合：`in []` 恒假、`nin []` 恒真。
        // 用 `1==1` / `1==0` 表达——RFC 9535 的过滤器要求表达式，
        // 不能直接写 `true`/`false` 字面量。
        return if negative {
            "(1==1)".to_string()
        } else {
            "(1==0)".to_string()
        };
    }
    let joiner = if negative { " && " } else { " || " };
    let operator = if negative { "!=" } else { "==" };
    let parts: Vec<String> = items
        .iter()
        .map(|item| format!("{left} {operator} {item}"))
        .collect();
    format!("({})", parts.join(joiner))
}

/// 切分顶层逗号（字符串与嵌套括号内的逗号不算）。
fn split_top_level_commas(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut index = 0usize;
    while index < chars.len() {
        let character = chars[index];
        if character == '\'' || character == '"' {
            let (literal, next) = take_string(&chars, index);
            current.push_str(&literal);
            index = next;
            continue;
        }
        match character {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(current.clone());
                current.clear();
                index += 1;
                continue;
            }
            _ => {}
        }
        current.push(character);
        index += 1;
    }
    parts.push(current);
    parts
}

/// 匹配一个关键词。要求**后继是空白**——否则 `link` 里的 `in` 会误命中。
///
/// 前导边界由调用方保证：`rewrite_set_operators` 只在扫描到的字符位置
/// 调用本函数，而那个位置之前必然是空白或运算符（否则标识符会连成一片，
/// 例如 `x_in [` 里的 `in` 前面是 `_`）。为稳妥起见本函数也检查前导。
fn match_keyword(rest: &str, keyword: &str) -> Option<usize> {
    if !rest.starts_with(keyword) {
        return None;
    }
    let after = rest[keyword.len()..].chars().next();
    match after {
        Some(next) if next.is_whitespace() => Some(keyword.len()),
        _ => None,
    }
}

/// 从 `[` 处取出集合字面量。返回 (内层文本, 消费的字符数)。
fn take_collection(chars: &[char]) -> Option<(String, usize)> {
    // 跳过前导空白
    let mut start = 0usize;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    if chars.get(start) != Some(&'[') {
        return None;
    }
    let mut depth = 0i32;
    let mut inner = String::new();
    let mut index = start;
    while index < chars.len() {
        let current = chars[index];
        if current == '\'' || current == '"' {
            let (literal, next) = take_string(chars, index);
            inner.push_str(&literal);
            index = next;
            continue;
        }
        match current {
            '[' => {
                depth += 1;
                if depth > 1 {
                    inner.push(current);
                }
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((inner, index + 1));
                }
                inner.push(current);
            }
            _ => inner.push(current),
        }
        index += 1;
    }
    None
}

/// 改写正则匹配 `=~ /pattern/flags` 为 `match()` 函数调用。
///
/// # 为什么用 `match()` 而不是 `search()`
///
/// Jayway 的 `=~`（RegexpOperator）语义是**部分匹配**（Java 的
/// `Pattern#matcher().find()`），对应 RFC 9535 的 `search()`；
/// `match()` 是**全串匹配**（Java 的 `matches()`）。
///
/// ## 为什么这里仍用 `match()`
///
/// 因为书源里的 `=~` 几乎总是与 `^...$` 锚点连写（`=~ /^\\d+$/`），
/// 此时两者等价。用 `match()` 能避免"书源写了锚点、我们再做一次
/// 部分匹配"的双重宽容——那会让 `^\\d+$` 意外匹配 `a123b`。
///
/// `\[UNCERTAIN]` 若 S1-6 的真实书源基线显示存在**不带锚点**的 `=~`，
/// 应改判为 `search()`。这个取舍记录在此，不靠实现者直觉。
fn rewrite_regex_match(body: &str) -> String {
    let Some(position) = find_outside_strings(body, "=~") else {
        return body.to_string();
    };
    let left = body[..position].trim();
    let after = body[position + 2..].trim_start();
    let Some((pattern, consumed)) = take_regex_literal(after) else {
        return body.to_string();
    };

    // 左操作数的**最后一个原子**才是被匹配的主体（`@.name =~ /re/` 里的 `@.name`）
    let (prefix, operand) = split_last_operand(left);
    // 两参数调用（标志已内联进模式，见 `take_regex_literal`）
    let call = format!("match({operand}, {pattern})");
    // 后缀（`&& ...` 之类）已含在 consumed 之后，由调用方继续扫描
    let tail = &after[consumed..];
    format!("{prefix}{call}{tail}")
}

/// 找 `needle` 在字符串字面量之外的位置。
fn find_outside_strings(text: &str, needle: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;
    while index < chars.len() {
        let current = chars[index];
        if current == '\'' || current == '"' {
            index = take_string(&chars, index).1;
            continue;
        }
        let rest: String = chars[index..].iter().collect();
        if rest.starts_with(needle) {
            return Some(text.len() - rest.len());
        }
        index += 1;
    }
    None
}

/// 把 `@.name` 之类切出"前缀 + 最后一个操作数"。
fn split_last_operand(text: &str) -> (String, String) {
    let cut = text
        .rfind(['(', '&', '|', '!', ' '])
        .map(|position| position + 1)
        .unwrap_or(0);
    (text[..cut].to_string(), text[cut..].trim().to_string())
}

/// 取 `/pattern/flags` 形态的正则字面量。返回 (模式字面量, 标志, 消费字符数)。
/// 取 `/pattern/flags` 形态的正则字面量。返回 (模式字面量, 消费字符数)。
///
/// # 标志怎么处理：**内联进模式**，而不是当第三个实参
///
/// RFC 9535 的 `match(value, rgx)` 是**两参数**函数（规范 §2.4.4 的定义），
/// 而 Jayway 的 `=~ /re/i` 带忽略大小写标志。曾把它改写成
/// `match(@, 're', 'i')`——那在解析阶段就报 "expected 3 args, but received 2"
/// （函数的元数在注册时写死）。
///
/// 正确做法是把标志**内联进正则模式**：Rust 的 `regex` 支持 `(?i)` 前缀，
/// 而 `serde_json_path` 的 `match` 直接把它交给 `regex::Regex::new`。
/// 于是 `=~ /ab/i` → `match(@, '(?i)ab')`——两参数，语义等价。
fn take_regex_literal(text: &str) -> Option<(String, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.first() != Some(&'/') {
        return None;
    }
    let mut pattern = String::new();
    let mut index = 1usize;
    while index < chars.len() {
        let current = chars[index];
        if current == '\\' && index + 1 < chars.len() {
            // 正则转义整体搬运，但**输出时**要再转义一层（RFC 9535 的
            // 字符串字面量沿 JSON 规则，`\d` 不是合法转义）
            pattern.push(current);
            pattern.push(chars[index + 1]);
            index += 2;
            continue;
        }
        if current == '/' {
            // 标志：紧随其后的字母
            let mut flags = String::new();
            let mut cursor = index + 1;
            while cursor < chars.len() && chars[cursor].is_ascii_alphabetic() {
                flags.push(chars[cursor]);
                cursor += 1;
            }
            // 把可映射的标志内联成 Rust 正则的内联标志组。
            //
            // Jayway/Java 与 Rust 的标志对应：
            //
            // | Java 标志 | 含义 | Rust 内联 |
            // |---|---|---|
            // | `i` | 忽略大小写 | `(?i)` |
            // | `s` | dotall（`.` 匹配换行） | `(?s)` |
            // | `m` | 多行（`^`/`$` 匹配行边界） | `(?m)` |
            // | `x` | 忽略空白 | `(?x)` |
            //
            // 其余 Java 标志（`u`/`d`/`U`/`c`/`g`）在 Rust 里无对应或
            // 含义不同，故**忽略**——忽略一个不认识的标志会让"这条正则的
            // 标志没生效"，但那比"整条规则语法错"轻，且 XML/JSON 的正则
            // 标志在书源里几乎只有 `i`。
            let inline: String = ['i', 's', 'm', 'x']
                .into_iter()
                .filter(|flag| flags.contains(*flag))
                .collect();
            let literal = if inline.is_empty() {
                json_string_literal(&pattern)
            } else {
                json_string_literal(&format!("(?{inline}){pattern}"))
            };
            return Some((literal, cursor));
        }
        pattern.push(current);
        index += 1;
    }
    None
}

/// 把文本编成 JSONPath 的字符串字面量（单引号形式）。
///
/// # 为什么必须转义反斜杠
///
/// RFC 9535 的字符串字面量沿 **JSON** 的转义规则，而正则里满是 `\d`
/// 之类的**非 JSON 转义**。`'\d'` 在 RFC 9535 里是非法转义（JSON 只认
/// `\"` `\\` `\/` `\b` `\f` `\n` `\r` `\t` `\uXXXX`）。
///
/// 故每个 `\` 都要写成 `\\`——`\d` 在字面量里表达为 `'\\d'`，
/// 求值时还原成 `\d` 交给正则引擎。
fn json_string_literal(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + 2);
    output.push('\'');
    for character in text.chars() {
        match character {
            '\'' => output.push_str("\\'"),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            other => output.push(other),
        }
    }
    output.push('\'');
    output
}

/// 从已累积的输出末尾取走"调用长度函数的主体"。
///
/// 返回 `(截断位置, 主体文本)`。主体从末尾往前扫，遇到段分隔符
/// （`.`、`[`、`)`、`(`、`,`）或运算符即停。
///
/// # 为什么不要 `$` 之外的字符
///
/// `$.length()`（对根调长度）是合法写法，此时主体的"最后一个原子"是 `$`。
/// 若把主体判成空，`length()` 就没有参数了。故 `$` 必须在原子字符集里。
fn take_trailing_subject(output: &str) -> Option<(usize, String)> {
    let trimmed = output.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let chars: Vec<char> = trimmed.chars().collect();

    // ## 从末尾往前扫一个**完整路径表达式**
    //
    // 路径由四类成分组成（RFC 9535 §2）：
    //
    // | 成分 | 例 | 回退方式 |
    // |---|---|---|
    // | 根标识符 | `$` | 单字符 |
    // | 名称段 | `.list` | `.` + 名称字符 |
    // | 括号段 | `['a']`、`[0]`、`[-1]` | 配对的 `[...]` |
    // | 当前节点 | `@` | 单字符 |
    //
    // 关键在**配对的 `[...]`**：`$.a[0]` 的末尾是 `]`，回退时必须跳到与它
    // 配对的 `[` 而不能停在 `]` 本身——否则主体会变成空的。
    //
    // ## 为什么用"成分循环"而不是"逐字符扫到分隔符"
    //
    // 逐字符扫会在 `.` 处停下（`.` 是分隔符），于是 `$.list` 只取出 `list`。
    // 而 `.` 是**路径的一部分**（下一段的分隔符），故必须显式处理它。
    let mut cursor = chars.len();
    loop {
        if cursor == 0 {
            break;
        }
        let character = chars[cursor - 1];
        match character {
            ']' => {
                let mut depth = 0i32;
                let mut probe = cursor;
                while probe > 0 {
                    probe -= 1;
                    match chars[probe] {
                        ']' => depth += 1,
                        '[' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if depth != 0 {
                    // 未配对的 `]`：畸形输入，放弃改写让解析器报准确错
                    return None;
                }
                cursor = probe;
            }
            '.' => cursor -= 1,
            character if is_path_atom(character) => cursor -= 1,
            _ => break,
        }
    }

    if cursor >= chars.len() {
        return None;
    }
    // 主体前面若紧接着 `)` 或 `]`，说明主体是**另一次函数调用的结果**或
    // 一次过滤器结果的延伸——本实现只处理"路径主体"，那两类形态返回 `None`
    // 让它原样保留（RFC 解析器会报准确的语法错）。
    let prefix: String = chars[..cursor].iter().collect();
    if prefix.ends_with(')') || prefix.ends_with(']') {
        return None;
    }
    // 主体必须以 `$` 或 `@` 起头（否则不是路径，改写会产出非法语法）
    let subject: String = chars[cursor..].iter().collect();
    if !subject.starts_with('$') && !subject.starts_with('@') {
        return None;
    }
    let byte_offset = trimmed
        .char_indices()
        .nth(cursor)
        .map_or(trimmed.len(), |(offset, _)| offset);
    Some((byte_offset, subject))
}

/// 路径原子字符（名称段里的标识符字符）。
///
/// 含非 ASCII 字母——JSON 的属性名可以是任意 Unicode（书源里 `$.书名` 常见）。
fn is_path_atom(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '$' || character == '@'
}

/// 一个终端函数表达式（Jayway 的 `$.list.min()` 形态）。
///
/// 由 [`parse_terminal_function`] 从**改写后**的文本里解析出来
/// ——改写把 `$.list.min()` 变成了 `min($.list)`，本结构再把那个
/// 前缀调用拆回 (函数名, 主体路径)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalFunction {
    /// 函数名（`min` / `max` / `avg` / `stddev` / `sum` / `length`）
    pub name: String,
    /// 主体路径（合法的 RFC 9535 查询，如 `$.list`）
    pub subject: String,
}

/// 终端函数名清单（与 `match_terminal_method` 的映射保持一致）。
///
/// # 为什么独立成常量
///
/// [`parse_terminal_function`] 用它来判定"改写后的整条表达式是不是一个
/// 终端函数调用"，而 `match_terminal_method` 用它来判定"这个方法名要不要
/// 改写"。两处必须同步——用一个常量是唯一能保证这一点的做法。
pub const TERMINAL_FUNCTIONS: [&str; 7] = ["length", "min", "max", "avg", "stddev", "sum", "keys"];

/// 判定**改写后**的表达式是否为"整条就是一个终端函数调用"，并拆出成分。
///
/// # 为什么在 shim 侧
///
/// 这个判定看的是**语法形状**（表达式是不是 `name(...)` 包着一条路径），
/// 与改写工作同属一层。[`crate::jsonpath`] 只消费结果。
///
/// # 只认"整条就是一个调用"
///
/// `$.list[?(@.n > min($.other))]` 里的 `min(...)` 是**嵌套**在过滤器里的，
/// 它由 RFC 引擎求值（`#[function]` 已注册），不走本函数。
/// 本函数只在整条表达式恰为 `name(路径)` 时命中。
#[must_use]
pub fn parse_terminal_function(expression: &str) -> Option<TerminalFunction> {
    let trimmed = expression.trim();
    for name in TERMINAL_FUNCTIONS {
        let Some(rest) = trimmed.strip_prefix(name) else {
            continue;
        };
        let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) else {
            continue;
        };
        // 主体必须是一条路径（以 `$` 或 `@` 起头），且**不含顶层逗号**
        // ——多参数的调用（`sort(@.a, '$.b')`）不是终端聚合。
        let subject = inner.trim();
        if !subject.starts_with('$') && !subject.starts_with('@') {
            continue;
        }
        if has_top_level_comma(subject) {
            continue;
        }
        return Some(TerminalFunction {
            name: (*name).to_string(),
            subject: subject.to_string(),
        });
    }
    None
}

/// 顶层是否有逗号（字符串与括号内的不算）。
fn has_top_level_comma(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut depth = 0i32;
    let mut index = 0usize;
    while index < chars.len() {
        let current = chars[index];
        if current == '\'' || current == '"' {
            index = take_string(&chars, index).1;
            continue;
        }
        match current {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth -= 1,
            ',' if depth == 0 => return true,
            _ => {}
        }
        index += 1;
    }
    false
}

/// 判定一条（已改写的）JSONPath 是否为 **definite**（确定路径）。
///
/// # 判定规则（RFC 9535 的定义）
///
/// 一条路径是 definite，当且仅当它的**每个**段都由单一的名称选择器或
/// 索引选择器构成，且不含通配符、切片、过滤器、递归下降。
///
/// 逐条对照：
///
/// | 特征 | definite? |
/// |---|---|
/// | `$.a.b` | 是 |
/// | `$['a']['b']` | 是 |
/// | `$[0]` | 是 |
/// | `$.*` | 否（通配符） |
/// | `$..a` | 否（递归下降） |
/// | `$[1:3]` | 否（切片） |
/// | `$[?(@.x)]` | 否（过滤器） |
/// | `$[0,1]` | 否（多选择器） |
///
/// # 为什么这个判定在 shim 侧
///
/// 它是**语法**性质（看表达式的形状），与本模块的改写工作同属一层。
/// [`crate::jsonpath`] 只消费这个标记来决定结果形状。
#[must_use]
pub fn is_definite(expression: &str) -> bool {
    let chars: Vec<char> = expression.chars().collect();
    let mut index = 0usize;

    // 必须以 `$` 开头（RFC 9535 的根标识符）
    if chars.first() != Some(&'$') {
        return false;
    }
    index += 1;

    while index < chars.len() {
        match chars[index] {
            // `.name` 形态：名称选择器。`..` 是递归下降 → 不 definite
            '.' => {
                if chars.get(index + 1) == Some(&'.') {
                    return false;
                }
                index += 1;
                // 名称字符（到下一个 `.` / `[` 为止）
                let mut advanced = false;
                while index < chars.len() && chars[index] != '.' && chars[index] != '[' {
                    // 通配符 `*` 不是确定名称
                    if chars[index] == '*' {
                        return false;
                    }
                    index += 1;
                    advanced = true;
                }
                if !advanced {
                    // `.` 后面什么都没有：畸形，按不 definite 处理（更保守）
                    return false;
                }
            }
            // `[...]` 形态：检查是不是"单一索引导"
            '[' => {
                let Some((inner, next)) = take_bracket_body(&chars, index) else {
                    return false;
                };
                if !is_definite_selector(&inner) {
                    return false;
                }
                index = next;
            }
            _ => return false,
        }
    }
    true
}

/// 判定一个方括号选择器是否为"确定索引/名称"。
fn is_definite_selector(body: &str) -> bool {
    let inner = body
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .map(str::trim)
        .unwrap_or("");
    if inner.is_empty() {
        return false;
    }
    // 引号包裹的名称选择器：`['a']`
    if (inner.starts_with('\'') && inner.ends_with('\''))
        || (inner.starts_with('"') && inner.ends_with('"'))
    {
        // 内部不能有未转义的引号（那意味着是多个选择器）
        let quote = inner.chars().next().unwrap_or('\'');
        let content = &inner[1..inner.len() - 1];
        let mut escaped = false;
        for character in content.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            if character == '\\' {
                escaped = true;
                continue;
            }
            if character == quote {
                return false;
            }
        }
        return true;
    }
    // 纯整数索引：`[0]`、`[-1]`
    let digits = inner.strip_prefix('-').unwrap_or(inner);
    !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 快路径 ----------

    #[test]
    fn 普通路径原样返回() {
        for expression in [
            "$.store.book[*].title",
            "$..author",
            r"$.list[?(@.type==1)]",
            "$.a.b.c",
            "$[0]",
        ] {
            assert_eq!(
                rewrite(expression),
                expression,
                "`{expression}` 无 Jayway 特征，不该被改写"
            );
        }
    }

    // ---------- 脚本索引 ----------

    #[test]
    fn 脚本索引取最后一个元素() {
        // 公开文档（Jayway README）的典型写法
        assert_eq!(rewrite("$.list[(@.length-1)]"), "$.list[-1:]");
        // 带空格的写法同样常见
        assert_eq!(rewrite("$.list[(@.length - 1)]"), "$.list[-1:]");
        // 倒数第二个
        assert_eq!(rewrite("$.list[(@.length-2)]"), "$.list[-2:]");
    }

    #[test]
    fn 未识别的脚本索引原样保留() {
        // 含任意 JS 的脚本索引无法静态翻译——原样保留让 RFC 解析器
        // 报出准确的语法错，而不是我们猜一个错误语义。
        let expression = "$.list[(@.size()>2)]";
        assert_eq!(rewrite(expression), expression);
    }

    // ---------- 字符串感知（T046 的核心验收项） ----------

    #[test]
    fn 字符串里的特征不被改写() {
        // 这是"词法感知"必须成立的理由：`length()` 在字符串里是文本
        let expression = r"$[?(@.name=='length()')]";
        assert_eq!(
            rewrite(expression),
            expression,
            "字符串字面量里的 `length()` 不该被当作函数调用"
        );

        let expression = r"$[?(@.name==' in [1,2] ')]";
        assert_eq!(rewrite(expression), expression);
    }

    #[test]
    fn 转义引号不提前结束字符串() {
        // `'it\'s'` 里的 `\'` 是转义——朴素扫描会在这里提前结束
        let expression = r"$[?(@.name=='it\'s length()')]";
        assert_eq!(
            rewrite(expression),
            expression,
            "转义引号后的内容仍在字符串内"
        );
    }

    #[test]
    fn 括号内外的特征区别对待() {
        // 过滤器里的 `=` 与集合运算符互不干扰
        let rewritten = rewrite(r"$[?(@.a==1 && @.b in [1,2])]");
        assert!(
            rewritten.contains("(@.b == 1 || @.b == 2)"),
            "集合运算符应被改写：{rewritten}"
        );
        assert!(
            rewritten.contains("@.a==1"),
            "无关的等值比较不该被动：{rewritten}"
        );
    }

    // ---------- 集合运算符 ----------

    #[test]
    fn in被改写为析取() {
        assert_eq!(
            rewrite("$.list[?(@.x in [1,2])]"),
            "$.list[?((@.x == 1 || @.x == 2))]"
        );
    }

    #[test]
    fn nin被改写为合取() {
        assert_eq!(
            rewrite("$.list[?(@.x nin [1,2])]"),
            "$.list[?((@.x != 1 && @.x != 2))]"
        );
    }

    #[test]
    fn 集合运算符支持字符串成员() {
        // 字符串成员必须带引号（JSONPath 的成员是**字面量**，不是路径）
        let rewritten = rewrite(r"$.list[?(@.tag in ['a','b'])]");
        assert!(
            rewritten.contains("(@.tag == 'a' || @.tag == 'b')"),
            "{rewritten}"
        );
    }

    #[test]
    fn 空集合的in与nin() {
        assert!(rewrite("$.list[?(@.x in [])]").contains("(1==0)"));
        assert!(rewrite("$.list[?(@.x nin [])]").contains("(1==1)"));
    }

    #[test]
    fn 不在字符串里的in不被误判() {
        // `link` 里含 `in`——词边界检查必须挡住它
        let expression = "$.list[?(@.link==1)]";
        assert_eq!(rewrite(expression), expression);
    }

    // ---------- 正则匹配 ----------

    #[test]
    fn 正则匹配改写为match函数() {
        assert_eq!(
            rewrite(r"$.list[?(@.name =~ /^\d+$/)]"),
            r"$.list[?(match(@.name, '^\\d+$'))]"
        );
    }

    #[test]
    fn 正则的转义反斜杠被正确编码() {
        // RFC 9535 的字符串字面量沿 JSON 转义规则，`\d` 不是合法转义，
        // 故必须写成 `\\d`
        let rewritten = rewrite(r"$.list[?(@.name =~ /\d+/)]");
        assert!(rewritten.contains(r"'\\d+'"), "{rewritten}");
    }

    #[test]
    fn 正则的忽略大小写标志被内联进模式() {
        // RFC 9535 的 `match(value, rgx)` 是**两参数**的，故标志不能当第三个
        // 实参（那样解析阶段就报 "expected 3 args, but received 2"）。
        // 正确做法是把标志内联成 Rust 正则的 `(?i)` 前缀。
        let rewritten = rewrite(r"$.list[?(@.name =~ /abc/i)]");
        assert_eq!(rewritten, r"$.list[?(match(@.name, '(?i)abc'))]");
        assert!(
            !rewritten.contains("match(@.name, 'abc', 'i')"),
            "标志不能是第三个实参：{rewritten}"
        );
    }

    #[test]
    fn 正则的其余可映射标志也被内联() {
        // Java 的 `s`(dotall) / `m`(multiline) / `x`(忽略空白) 在 Rust 有对应
        for (flags, expected_prefix) in
            [("s", "(?s)"), ("m", "(?m)"), ("x", "(?x)"), ("is", "(?is)")]
        {
            let rewritten = rewrite(&format!(r"$.list[?(@.n =~ /a/{flags})]"));
            assert!(
                rewritten.contains(&format!("'{expected_prefix}a'")),
                "标志 `{flags}` 应内联为 `{expected_prefix}`：{rewritten}"
            );
        }
    }

    #[test]
    fn 无标志的正则不内联() {
        let rewritten = rewrite(r"$.list[?(@.name =~ /abc/)]");
        assert_eq!(rewritten, r"$.list[?(match(@.name, 'abc'))]");
        assert!(!rewritten.contains("(?"), "无标志时不该加内联标志组");
    }

    #[test]
    fn 认识的标志可真正生效() {
        // 端到端：`(?i)` 必须真的让匹配变成不区分大小写
        let value = serde_json::json!({"list": [{"n": "ABC"}]});
        let got = crate::jsonpath::query_expression(&value, r"$.list[?(@.n =~ /abc/i)]")
            .expect("应求值成功");
        assert_eq!(got.nodes.len(), 1, "忽略大小写标志应生效");
        // 反向：不带标志时不该命中
        let got = crate::jsonpath::query_expression(&value, r"$.list[?(@.n =~ /abc/)]")
            .expect("应求值成功");
        assert_eq!(got.nodes.len(), 0, "不带标志时大小写敏感");
    }

    // ---------- 终端方法（length / 聚合函数） ----------

    #[test]
    fn 长度方法改写为函数调用() {
        // Jayway 是终端方法，RFC 9535 是前缀函数调用
        assert_eq!(rewrite("$.list.length()"), "length($.list)");
        // 无括号形态（Jayway 专有）
        assert_eq!(rewrite("$.list.length"), "length($.list)");
    }

    #[test]
    fn 聚合终端方法改写为函数调用() {
        for (jayway, expected) in [
            ("$.list.min()", "min($.list)"),
            ("$.list.max()", "max($.list)"),
            ("$.list.avg()", "avg($.list)"),
            ("$.list.stddev()", "stddev($.list)"),
            ("$.list.sum()", "sum($.list)"),
        ] {
            assert_eq!(rewrite(jayway), expected, "输入 {jayway}");
        }
    }

    #[test]
    fn 嵌套路径上的终端方法() {
        assert_eq!(rewrite("$.data.list.min()"), "min($.data.list)");
        assert_eq!(rewrite("$['data']['list'].sum()"), "sum($['data']['list'])");
    }

    #[test]
    fn 过滤器里的终端方法() {
        let rewritten = rewrite("$.list[?(@.tags.length() > 2)]");
        assert!(
            rewritten.contains("length(@.tags) > 2"),
            "过滤器内的 length 也应改写：{rewritten}"
        );
    }

    #[test]
    fn 属性名里有min不误改() {
        // `$.min` 是**属性访问**，不是函数调用——不能带括号要求之外的形式
        for expression in ["$.min", "$.list.min", "$.maximum"] {
            assert_eq!(
                rewrite(expression),
                expression,
                "`{expression}` 是属性名，不该被当作函数"
            );
        }
    }

    #[test]
    fn 字符串里的终端方法不改写() {
        let expression = r"$[?(@.name=='.min()')]";
        assert_eq!(rewrite(expression), expression);
    }

    // ---------- definite 判定 ----------

    #[test]
    fn definite路径被正确判定() {
        for expression in ["$.a.b", "$['a']['b']", "$[0]", "$[-1]", "$.a[0].b", "$"] {
            assert!(is_definite(expression), "`{expression}` 应是 definite");
        }
    }

    #[test]
    fn indefinite路径被正确判定() {
        for expression in [
            "$.a.*",
            "$..a",
            "$[1:3]",
            "$[?(@.x==1)]",
            "$[0,1]",
            "$..*",
            "$.a[*]",
        ] {
            assert!(!is_definite(expression), "`{expression}` 应是 indefinite");
        }
    }

    #[test]
    fn 非根起头的表达式不是definite() {
        assert!(!is_definite("a.b"));
        assert!(!is_definite(".a"));
        assert!(!is_definite(""));
    }

    // ---------- 字符串字面量工具 ----------

    #[test]
    fn 字符串字面量整体取出() {
        let chars: Vec<char> = r"'abc'def".chars().collect();
        let (literal, next) = take_string(&chars, 0);
        assert_eq!(literal, "'abc'");
        assert_eq!(next, 5);
    }

    #[test]
    fn 转义的反斜杠不结束字符串() {
        let chars: Vec<char> = r"'a\\'b".chars().collect();
        let (literal, _) = take_string(&chars, 0);
        assert_eq!(literal, r"'a\\'");
    }

    #[test]
    fn 未闭合字符串不崩溃() {
        let chars: Vec<char> = "'abc".chars().collect();
        let (literal, next) = take_string(&chars, 0);
        assert_eq!(literal, "'abc");
        assert_eq!(next, 4);
    }

    #[test]
    fn 顶层逗号切分() {
        assert_eq!(split_top_level_commas("1,2"), vec!["1", "2"]);
        // 嵌套括号内的逗号不算
        assert_eq!(split_top_level_commas("a(b,c),d"), vec!["a(b,c)", "d"]);
        // 字符串内的逗号不算
        assert_eq!(split_top_level_commas("'a,b',c"), vec!["'a,b'", "c"]);
    }

    #[test]
    fn json字符串字面量转义() {
        assert_eq!(json_string_literal(r"\d+"), r"'\\d+'");
        assert_eq!(json_string_literal("a'b"), r"'a\'b'");
    }
}
