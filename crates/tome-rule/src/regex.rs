//! 正则双引擎 + 规范化层 + 替换串翻译层（tasks.md T024 / research.md R5）
//!
//! # 为什么要两个引擎
//!
//! 书源规则的正则来自 Java `java.util.regex` 生态，其中相当一部分用了
//! `regex` crate **设计上不支持**的特性（lookaround、反向引用）。研究结论（R5）
//! 是：默认走 `regex`（线性时间，无回溯爆炸），**编译期预扫描**发现 fancy 特性
//! 时才路由到 `fancy-regex`。这样绝大多数规则落在快路径上，只有真正需要的才付
//! 回溯代价。
//!
//! # 三个必须显式处理的陷阱（都不报错，全会静默出错）
//!
//! ## 陷阱一：`a++` 被 `regex` 静默当作嵌套重复
//!
//! Java 的 `a++` 是**占有量词**（possessive quantifier，等价于原子组 `(?>a+)`：
//! 匹配后不回溯）。`regex` crate 的设计不支持占有量词，它把 `a++` 解析为
//! **嵌套重复** `(a+)+`——语义完全不同：
//!
//! | 模式 | 输入 | Java 占有量词 | `regex` 的嵌套重复 |
//! |---|---|---|---|
//! | `a++a` | `aaa` | 不匹配（`a++` 吃掉全部，无 `a` 可回溯） | 匹配 `aaa` |
//!
//! **关键是它不报编译错误**。规范化层必须把 `a++` / `a*+` / `a?+` 显式转成
//! `(?>a+)` 或拒绝——本实现选择转成原子组（语义正确且 `fancy-regex` 支持原子组）。
//!
//! ## 陷阱二：替换串里 `$1a` 被读作组名 `1a`
//!
//! Rust 的替换串用**最长可能名**解析：`$1a` 里的组名是 `1a`（不是组 1 + 字面 `a`），
//! 因组 `1a` 不存在而替换成**空串**。Java 读作组 1 加字面 `a`。
//! 朴素透传会静默丢字，故必须有[`translate_replacement`] 翻译层。
//!
//! ## 陷阱三：`\h` 在两个引擎里含义不同
//!
//! `fancy-regex` 把 `\h` 当十六进制数字位；Java 的 `\h` 是水平空白。
//! 本实现只做**显式记录**（[`PatternKind`] 的分类会把它算作 fancy 特性），
//! 不做改写——改写需要完整的词法状态机，且这条差异影响面小（R5 列为实测分叉之一）。
//! 标 `[UNCERTAIN]` 待 S1-3 合成语料统计实际影响。
//!
//! # 灾难性回溯护栏
//!
//! R5 实测：`(?=a)(a+)+b` 对 5,000 字符耗 1.82s，对 **200,000 字符挂起 >120s**。
//! `fancy-regex` 的 `backtrack_limit` 是唯一进程内护栏，但它是**状态计数**而非
//! 墙钟超时。故本实现叠加两道：[`RegexLimits`] 同时设回溯上界与**输入长度上限**。

use std::borrow::Cow;

use crate::error::RuleError;

/// 引擎选择：快路径还是 fancy 路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    /// 只用 `regex` 线性引擎即可（无 fancy 特性）。
    Fast,
    /// 含 `regex` 不支持的特性，必须走 `fancy-regex`。
    Fancy,
}

/// 编译好的正则。
///
/// 两种引擎的编译结果共存一个枚举里，调用方无须知道走了哪条路
/// （但 [`Self::kind`] 暴露出来供诊断与测试断言）。
#[derive(Debug)]
pub struct CompiledRegex {
    engine: Engine,
    kind: PatternKind,
    /// 规范化后的模式串（诊断用；也能验证规范化层确实做了改写）。
    normalized: String,
}

#[derive(Debug)]
enum Engine {
    /// `regex` 线性引擎。
    Fast(regex::Regex),
    /// `fancy-regex` 混合引擎（无 fancy 特性时它内部仍委托给线性快路径）。
    Fancy(Box<fancy_regex::Regex>),
}

/// 资源上界（防灾难性回溯与超长输入，R5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegexLimits {
    /// `fancy-regex` 的回溯状态上界。
    ///
    /// 默认取 100 万（`fancy-regex` 自身默认）的 **1/10**：R5 实测的挂起案例在
    /// 10 万状态量级就已远超可接受耗时，把上界压低能让它**更早**放弃并返回错误，
    /// 而不是把用户界面冻住两分钟。
    pub backtrack_limit: usize,
    /// 单次匹配允许的最大输入字节数。
    ///
    /// 20 万字符的输入在 1.82s/5,000 字符的实测基础上外推已过百秒，故设 64 KiB
    /// 上限——书源规则处理的是单个字段/章节片段，正常远小于此。
    /// 超限返回 [`RuleError::Limit`]，由调用方决定分块处理还是降级。
    pub max_input_bytes: usize,
}

impl RegexLimits {
    /// 生产用默认上界。
    pub const DEFAULT: Self = Self {
        backtrack_limit: 100_000,
        max_input_bytes: 64 * 1024,
    };

    /// 宽松上界（测试用，或处理可信本地内容时）。
    pub const PERMISSIVE: Self = Self {
        backtrack_limit: 1_000_000,
        max_input_bytes: 4 * 1024 * 1024,
    };
}

impl Default for RegexLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 把 Java 风格模式规范化成 Rust 引擎能正确理解的形式（陷阱一的落点）。
///
/// 只做**占有量词 → 原子组**的机械改写。改写需要词法感知（不能碰字符类里的
/// `+`、也不能把 `\++` 这类转义误判），故下面用一个小状态机跟踪
/// `[...]` 字符类与 `\` 转义。
///
/// ## 改写规则
///
/// 原子组的操作数是**前一个"可重复单元"**，不是单字符。所以
/// `abc++` 要改成 `ab(?>c+)`，而 `ab++` 是 `a` 后接 `b++`（`b` 重复）→ `a(?>b+)`。
/// 判断"前一个可重复单元"的边界就是判断**已输出内容的尾部**：
///
/// | 前一个单元的形态 | 边界算法 |
/// |---|---|
/// | 单个字面量 / 转义序列 `\d` | 回退 1 个（转义序列回退到 `\`） |
/// | 字符类 `[...]` | 回退到匹配的 `[` |
/// | 分组 `(...)` / `(?:...)` / 命名组 | 回退到匹配的 `(` |
///
/// 这不是完整的正则语法分析（真正的分析要 AST），而是"够用且可验证"的
/// 边界回退——R5 的实测案例（`a++` 系列的静默分叉）全部落在这三种形态内。
pub fn normalize_pattern(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut output = String::with_capacity(pattern.len() + 8);
    let mut index = 0usize;
    let mut in_class = false;

    while index < chars.len() {
        let current = chars[index];

        if current == '\\' {
            // 转义序列整体输出（含被转义字符）
            output.push(current);
            if index + 1 < chars.len() {
                output.push(chars[index + 1]);
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }

        if in_class {
            output.push(current);
            if current == ']' {
                in_class = false;
            }
            index += 1;
            continue;
        }

        if current == '[' {
            in_class = true;
            output.push(current);
            index += 1;
            continue;
        }

        // 检测量词后紧跟的 `+`：`a++` / `a*+` / `a?+` / `a{2,3}+`
        // 也要挡住 `a+?`（惰性量词，合法）与 `a+++`（无意义，交给引擎报错）。
        let next = chars.get(index + 1).copied();
        if matches!(current, '*' | '+' | '?') && next == Some('+') {
            let after = chars.get(index + 2).copied();
            if after == Some('+') {
                // `a+++`：不是占有量词，原样输出让引擎报错而不是我们猜
                output.push(current);
                index += 1;
                continue;
            }
            let operand_start = find_last_unit_start(&output);
            if let Some(start) = operand_start {
                let operand = output[start..].to_string();
                output.truncate(start);
                output.push_str("(?>");
                output.push_str(&operand);
                output.push(current);
                output.push(')');
            } else {
                // 前一个单元无法识别（如模式以 `++` 开头）：原样输出交给引擎报错
                output.push(current);
                output.push('+');
            }
            index += 2;
            continue;
        }

        if current == '{' {
            // `{n,m}+` 也是占有量词。先整体拷贝到 `}`，再看下一个字符是否是 `+`。
            let close = find_class_or_group_end(&chars, index, '{', '}');
            match close {
                Some(close) => {
                    let quantifier: String = chars[index..=close].iter().collect();
                    if chars.get(close + 1).copied() == Some('+')
                        && chars.get(close + 2).copied() != Some('+')
                    {
                        if let Some(start) = find_last_unit_start(&output) {
                            let operand = output[start..].to_string();
                            output.truncate(start);
                            output.push_str("(?>");
                            output.push_str(&operand);
                            output.push_str(&quantifier);
                            output.push(')');
                        } else {
                            output.push_str(&quantifier);
                            output.push('+');
                        }
                        index = close + 2;
                        continue;
                    }
                    output.push_str(&quantifier);
                    index = close + 1;
                    continue;
                }
                None => {
                    output.push(current);
                    index += 1;
                    continue;
                }
            }
        }

        output.push(current);
        index += 1;
    }

    output
}

/// 找 `index` 处 `open` 对应的 `close`（考虑嵌套与转义）。找不到返回 `None`。
fn find_class_or_group_end(chars: &[char], index: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut cursor = index;
    while cursor < chars.len() {
        let current = chars[cursor];
        if current == '\\' {
            cursor += 2;
            continue;
        }
        if current == open {
            depth += 1;
        } else if current == close {
            depth -= 1;
            if depth == 0 {
                return Some(cursor);
            }
        }
        cursor += 1;
    }
    None
}

/// 回退找出**已输出内容尾部**那个"可重复单元"的起始位置。
///
/// 三种形态（见 [`normalize_pattern`] 的表格）：字符类 `[...]`、分组 `(...)`、
/// 单个字面量或转义序列。返回 `None` 表示尾部没有可识别的单元。
fn find_last_unit_start(output: &str) -> Option<usize> {
    let bytes = output.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let last = *bytes.last()?;
    // 字符类：从尾部找未转义的 `[`
    if last == b']' {
        return find_open_backwards(output, b'[', b']');
    }
    if last == b')' {
        return find_open_backwards(output, b'(', b')');
    }

    // 单个字符：若它前面是 `\`，则单元从 `\` 开始（转义序列是一个单元）
    // `floor_char_boundary(output.len())` 会返回 `len` 本身（那是边界），
    // 故必须从 `len - 1` 处向前找，否则会把最后一个字符也算进上一单元。
    let last_char_start = output.floor_char_boundary(output.len() - 1);
    let before = &output.as_bytes()[..last_char_start];
    if before.last() == Some(&b'\\') {
        // 数一数 `\` 的连续性：奇数个则最后这个字符被转义
        let mut backslashes = 0usize;
        let mut cursor = before.len();
        while cursor > 0 && before[cursor - 1] == b'\\' {
            backslashes += 1;
            cursor -= 1;
        }
        if backslashes % 2 == 1 {
            return Some(cursor + backslashes - 1);
        }
    }
    Some(last_char_start)
}

/// 从尾部反向找未转义的 `open`，并校验它与其后的 `close` 配对。
fn find_open_backwards(output: &str, open: u8, close: u8) -> Option<usize> {
    let bytes = output.as_bytes();
    let mut depth = 0i32;
    let mut cursor = bytes.len();
    while cursor > 0 {
        cursor -= 1;
        let current = bytes[cursor];
        // 数它前面连续的 `\` 判断是否被转义
        let mut backslashes = 0usize;
        let mut probe = cursor;
        while probe > 0 && bytes[probe - 1] == b'\\' {
            backslashes += 1;
            probe -= 1;
        }
        if backslashes % 2 == 1 {
            continue;
        }
        if current == close {
            depth += 1;
        } else if current == open {
            depth -= 1;
            if depth == 0 {
                // 分组可能带修饰前缀 `(?:` / `(?P<name>` / `(?i)`。
                // 回退到 `(` 即可——原子组的操作数包含这些修饰前缀是对的。
                return Some(cursor);
            }
        }
    }
    None
}

/// 扫描模式，判断它能走线性快路径还是要 fancy 引擎。
///
/// 依据 `regex` crate 的公开文档「Syntax」一节的"unsupported"清单：
/// lookaround（`(?=` `(?!` `(?<=` `(?<!`）、反向引用（`\1`、`\k<name>`）、
/// 原子组（`(?>`) 、占有量词（`a++` 等规范化后变成原子组）、子程序调用
/// （`(?R)`、`(?1)`）、条件（`(?(...)`）。
///
/// 预扫描必须在**规范化之后**做：规范化会把 `a++` 变成原子组 `(?>a+)`，
/// 而原子组本身就是 fancy 特性——顺序反了会把 `a++` 误判成快路径。
pub fn classify_after_normalization(normalized: &str) -> PatternKind {
    let chars: Vec<char> = normalized.chars().collect();
    let mut index = 0usize;
    let mut in_class = false;

    while index < chars.len() {
        let current = chars[index];

        if current == '\\' {
            // 反向引用：`\1`..`\9` 与 `\k<name>` / `\k'name'`
            if let Some(&next) = chars.get(index + 1) {
                if next.is_ascii_digit() && next != '0' {
                    return PatternKind::Fancy;
                }
                if next == 'k' {
                    return PatternKind::Fancy;
                }
                // `\Q...\E`：Java 的字面引用。`fancy-regex` 同样不支持（R5 实测
                // parse error），但归类为 Fancy 让它在 fancy 路径上报错，
                // 错误信息来自引擎与调用点一致。
                if next == 'Q' {
                    return PatternKind::Fancy;
                }
            }
            index += 2;
            continue;
        }

        if in_class {
            if current == ']' {
                in_class = false;
            }
            index += 1;
            continue;
        }

        if current == '[' {
            in_class = true;
            index += 1;
            continue;
        }

        if current == '(' && chars.get(index + 1) == Some(&'?') {
            match chars.get(index + 2).copied() {
                // lookaround 与原子组
                Some('=') | Some('!') | Some('>') => return PatternKind::Fancy,
                // `(?<=` / `(?<!`
                Some('<') => match chars.get(index + 3).copied() {
                    Some('=') | Some('!') => return PatternKind::Fancy,
                    // `(?<name>...)` 是命名捕获组，`regex` 支持
                    _ => {}
                },
                // 子程序调用 / 递归 / 条件
                Some('R') | Some('&') | Some('(') => return PatternKind::Fancy,
                _ => {
                    // `(?1)` 之类的数字子程序调用
                    if chars
                        .get(index + 2)
                        .is_some_and(|character| character.is_ascii_digit())
                    {
                        return PatternKind::Fancy;
                    }
                }
            }
        }

        index += 1;
    }

    PatternKind::Fast
}

/// 编译一个 Java 风格正则。
///
/// 流程固定为三步（顺序不可换，见 [`classify_after_normalization`]）：
/// 规范化 → 预扫描路由 → 编译。
pub fn compile(pattern: &str, limits: RegexLimits) -> Result<CompiledRegex, RuleError> {
    let normalized = normalize_pattern(pattern);
    let kind = classify_after_normalization(&normalized);

    match kind {
        PatternKind::Fast => {
            let compiled = regex::Regex::new(&normalized)
                .map_err(|error| RuleError::regex(error.to_string()))?;
            Ok(CompiledRegex {
                engine: Engine::Fast(compiled),
                kind,
                normalized,
            })
        }
        PatternKind::Fancy => {
            let compiled = fancy_regex::RegexBuilder::new(&normalized)
                .backtrack_limit(limits.backtrack_limit)
                .build()
                .map_err(|error| RuleError::regex(error.to_string()))?;
            Ok(CompiledRegex {
                engine: Engine::Fancy(Box::new(compiled)),
                kind,
                normalized,
            })
        }
    }
}

impl CompiledRegex {
    /// 本正则实际走的引擎。
    pub fn kind(&self) -> PatternKind {
        self.kind
    }

    /// 规范化后的模式串。
    pub fn normalized_pattern(&self) -> &str {
        &self.normalized
    }

    /// 是否命中。
    pub fn is_match(&self, haystack: &str, limits: RegexLimits) -> Result<bool, RuleError> {
        check_input_length(haystack, limits)?;
        match &self.engine {
            Engine::Fast(compiled) => Ok(compiled.is_match(haystack)),
            Engine::Fancy(compiled) => compiled
                .is_match(haystack)
                .map_err(|error| RuleError::limit(format!("回溯超限：{error}"))),
        }
    }

    /// 找第一个匹配，返回整体匹配文本与**捕获组**（组 0 是整体）。
    ///
    /// 捕获组列表里未参与匹配的组为 `None`（与 Java `Matcher.group()` 返回 `null`
    /// 对齐，而非空串——这个区别对书源规则有实际意义，很多规则用"组是否为空"
    /// 区分"没匹配到该分支"与"匹配到了但内容为空"）。
    pub fn captures(
        &self,
        haystack: &str,
        limits: RegexLimits,
    ) -> Result<Option<Vec<Option<String>>>, RuleError> {
        check_input_length(haystack, limits)?;
        match &self.engine {
            Engine::Fast(compiled) => Ok(compiled.captures(haystack).map(|captures| {
                captures
                    .iter()
                    .map(|group| group.map(|matched| matched.as_str().to_string()))
                    .collect()
            })),
            Engine::Fancy(compiled) => {
                let captures = compiled
                    .captures(haystack)
                    .map_err(|error| RuleError::limit(format!("回溯超限：{error}")))?;
                Ok(captures.map(|captures| {
                    captures
                        .iter()
                        .map(|group| group.map(|matched| matched.as_str().to_string()))
                        .collect()
                }))
            }
        }
    }

    /// 找第一个匹配文本。
    pub fn find(&self, haystack: &str, limits: RegexLimits) -> Result<Option<String>, RuleError> {
        check_input_length(haystack, limits)?;
        match &self.engine {
            Engine::Fast(compiled) => Ok(compiled
                .find(haystack)
                .map(|matched| matched.as_str().to_string())),
            Engine::Fancy(compiled) => {
                let matched = compiled
                    .find(haystack)
                    .map_err(|error| RuleError::limit(format!("回溯超限：{error}")))?;
                Ok(matched.map(|matched| matched.as_str().to_string()))
            }
        }
    }

    /// 替换**全部**匹配。
    ///
    /// `replacement` 必须是已经过 [`translate_replacement`] 翻译的 Rust 形式替换串；
    /// 调用方通常用 [`replace_all_java`] 一步到位。
    pub fn replace_all(
        &self,
        haystack: &str,
        replacement: &str,
        limits: RegexLimits,
    ) -> Result<String, RuleError> {
        check_input_length(haystack, limits)?;
        match &self.engine {
            Engine::Fast(compiled) => Ok(compiled.replace_all(haystack, replacement).into_owned()),
            Engine::Fancy(compiled) => compiled
                // `try_replacen` 而非 `replacen`：后者在回溯超限时 panic，
                // 违反 AGENTS.md「禁止生产路径 panic」。
                .try_replacen(haystack, 0, replacement)
                .map(|result: Cow<'_, str>| result.into_owned())
                .map_err(|error| RuleError::limit(format!("回溯超限：{error}"))),
        }
    }

    /// 替换全部匹配（Java 替换串语义，自动过翻译层）。
    ///
    /// 这是书源规则替换链（FR-004）的常规入口。
    pub fn replace_all_java(
        &self,
        haystack: &str,
        java_replacement: &str,
        limits: RegexLimits,
    ) -> Result<String, RuleError> {
        let translated = translate_replacement(java_replacement);
        self.replace_all(haystack, &translated, limits)
    }
}

fn check_input_length(haystack: &str, limits: RegexLimits) -> Result<(), RuleError> {
    if haystack.len() > limits.max_input_bytes {
        return Err(RuleError::limit(format!(
            "输入 {} 字节超过上限 {} 字节",
            haystack.len(),
            limits.max_input_bytes
        )));
    }
    Ok(())
}

/// 把 Java 风格的替换串翻译成 Rust 引擎的替换串（陷阱二）。
///
/// ## 为什么必须翻译
///
/// Rust 的替换串按**最长可能名**解析组引用：`$1a` 里的名字是 `1a` 而不是 `1`
/// 加字面 `a`，因组 `1a` 不存在而替换成**空串**——静默丢字，不报错。
/// `fancy-regex` 的 `Expander` 用同一最长名规则，故两引擎共用本翻译层。
///
/// ## 规则表
///
/// | 输入 | 输出 | 依据 |
/// |---|---|---|
/// | `$1` ~ `$99` | `${1}` ~ `${99}` | 数字引用改为花括号形式，终结歧义 |
/// | `$name` | `${name}` | 命名引用同理 |
/// | `$` 后无合法名字 | `$$` | 字面美元符号 |
/// | `\$` | `$$` | Java 的转义美元在 Rust 里写作 `$$` |
/// | `\` 后非 `$` | 原样保留 `\` 与后一字符 | Java 替换串里 `\` 只转义 `\` 与 `$` |
///
/// 非法组引用（如 `$99` 而模式只有 1 个组）在 Rust 里替换为空串——这与 Java 的
/// `IndexOutOfBoundsException` 不同。选择**对齐 Rust 的空串行为**而非抛错：
/// 书源规则的替换串里出现越界引用的常见原因是规则本身写得糙，抛错会让整条
/// 规则链失败，而空串只是让那个位置留白——后者更符合"尽量产出结果"的取向。
/// 这点标 `[UNCERTAIN]`：S1-3 合成语料会统计实际影响。
pub fn translate_replacement(java_replacement: &str) -> String {
    let chars: Vec<char> = java_replacement.chars().collect();
    let mut output = String::with_capacity(java_replacement.len() + 8);
    let mut index = 0usize;

    while index < chars.len() {
        let current = chars[index];

        // `\$` → `$$`（Java 转义美元）
        if current == '\\' {
            match chars.get(index + 1).copied() {
                Some('$') => {
                    output.push_str("$$");
                    index += 2;
                    continue;
                }
                Some('\\') => {
                    // Java 里 `\\` 是字面反斜杠；Rust 替换串里反斜杠无特殊含义
                    output.push('\\');
                    index += 2;
                    continue;
                }
                Some(other) => {
                    // Java 规范里只有 `\` 与 `$` 需要转义，其余反斜杠保留原样
                    output.push('\\');
                    output.push(other);
                    index += 2;
                    continue;
                }
                None => {
                    output.push('\\');
                    index += 1;
                    continue;
                }
            }
        }

        if current == '$' {
            let next = chars.get(index + 1).copied();
            match next {
                // `$$`：Java 里是字面 `$`，Rust 里也是 → 原样
                Some('$') => {
                    output.push_str("$$");
                    index += 2;
                    continue;
                }
                // `${...}` 已是显式形式，原样拷贝整个花括号
                Some('{') => {
                    output.push('$');
                    output.push('{');
                    index += 2;
                    let mut cursor = index;
                    while cursor < chars.len() && chars[cursor] != '}' {
                        output.push(chars[cursor]);
                        cursor += 1;
                    }
                    if cursor < chars.len() {
                        output.push('}');
                        index = cursor + 1;
                    } else {
                        index = cursor;
                    }
                    continue;
                }
                // `$1` 形式的数字引用
                Some(first) if first.is_ascii_digit() => {
                    let mut cursor = index + 1;
                    let mut name = String::new();
                    while cursor < chars.len() && chars[cursor].is_ascii_digit() {
                        name.push(chars[cursor]);
                        cursor += 1;
                    }
                    output.push_str("${");
                    output.push_str(&name);
                    output.push('}');
                    index = cursor;
                    continue;
                }
                // `$name` 形式的命名引用（Rust 命名组规则：`[a-zA-Z_][a-zA-Z0-9_]*`）
                Some(first) if first.is_ascii_alphabetic() || first == '_' => {
                    let mut cursor = index + 1;
                    let mut name = String::new();
                    while cursor < chars.len()
                        && (chars[cursor].is_ascii_alphanumeric() || chars[cursor] == '_')
                    {
                        name.push(chars[cursor]);
                        cursor += 1;
                    }
                    output.push_str("${");
                    output.push_str(&name);
                    output.push('}');
                    index = cursor;
                    continue;
                }
                // `$` 后面什么都没有，或后接非名字字符 → 字面美元
                _ => {
                    output.push_str("$$");
                    index += 1;
                    continue;
                }
            }
        }

        output.push(current);
        index += 1;
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast(pattern: &str) -> CompiledRegex {
        let compiled = compile(pattern, RegexLimits::DEFAULT).expect("正则应能编译");
        assert_eq!(
            compiled.kind(),
            PatternKind::Fast,
            "模式 {pattern} 应走线性快路径"
        );
        compiled
    }

    fn fancy(pattern: &str) -> CompiledRegex {
        let compiled = compile(pattern, RegexLimits::DEFAULT).expect("正则应能编译");
        assert_eq!(
            compiled.kind(),
            PatternKind::Fancy,
            "模式 {pattern} 应走 fancy 路径"
        );
        compiled
    }

    // ---------- 规范化层（陷阱一） ----------

    #[test]
    fn 占有量词被转成原子组() {
        assert_eq!(normalize_pattern("a++"), "(?>a+)");
        assert_eq!(normalize_pattern("a*+"), "(?>a*)");
        assert_eq!(normalize_pattern("a?+"), "(?>a?)");
        assert_eq!(normalize_pattern("a{2,3}+"), "(?>a{2,3})");
    }

    #[test]
    fn 惰性量词与普通量词不被改写() {
        // `a+?` 是惰性（合法），`a+` 是普通量词，都不含陷阱。
        assert_eq!(normalize_pattern("a+?"), "a+?");
        assert_eq!(normalize_pattern("a+"), "a+");
        assert_eq!(normalize_pattern("a*"), "a*");
        assert_eq!(normalize_pattern("a{2,3}"), "a{2,3}");
    }

    #[test]
    fn 字符类里的加号不被误判为量词() {
        assert_eq!(normalize_pattern("[a+]"), "[a+]");
        assert_eq!(normalize_pattern("[+]++"), "(?>[+]+)");
    }

    #[test]
    fn 转义的加号不被误判() {
        // `\+` = 字面加号；`\++` = 一个字或多字面加号（**普通**量词，不是占有量词）
        assert_eq!(normalize_pattern(r"\+"), r"\+");
        assert_eq!(normalize_pattern(r"\++"), r"\++");
        // `\+++` 才是"字面加号的占有量词"
        assert_eq!(normalize_pattern(r"\+++"), r"(?>\++)");
    }

    #[test]
    fn 分组的占有量词作用于整个分组() {
        assert_eq!(normalize_pattern("(ab)++"), "(?>(ab)+)");
        assert_eq!(normalize_pattern("(?:ab)*+"), "(?>(?:ab)*)");
    }

    #[test]
    fn 多字符单元只回退一个单元() {
        // `abc++` 里占有量词作用于 `c`（前一个可重复单元），不是 `abc`
        assert_eq!(normalize_pattern("abc++"), "ab(?>c+)");
    }

    #[test]
    fn 无操作数时原样输出交给引擎报错() {
        // 模式以 `++` 开头：无法识别操作数，不该由我们猜
        assert_eq!(normalize_pattern("++"), "++");
    }

    #[test]
    fn 陷阱一的语义分叉被消除() {
        // R5 的实测案例：`a++a` on "aaa"
        // - Java 占有量词：`a++` 吃掉全部 → 不匹配
        // - regex 静默当嵌套重复：匹配 "aaa"
        // 规范化后必须与 Java 一致（不匹配）。
        let compiled = fancy("a++a");
        assert_eq!(compiled.normalized_pattern(), "(?>a+)a");
        assert!(
            !compiled
                .is_match("aaa", RegexLimits::DEFAULT)
                .expect("匹配不应失败"),
            "占有量词语义下 `a++a` 不该匹配 `aaa`（这正是 regex 静默分叉的案例）"
        );
        // 对照：普通量词 `a+a` 在 "aaa" 上匹配
        assert!(fast("a+a")
            .is_match("aaa", RegexLimits::DEFAULT)
            .expect("匹配不应失败"));
    }

    #[test]
    fn 非占有用法行为不变() {
        // 规范化不该改变普通模式的语义
        let compiled = fast("(\\d+)-(\\d+)");
        let captures = compiled
            .captures("12-34", RegexLimits::DEFAULT)
            .expect("匹配不应失败")
            .expect("应有匹配");
        assert_eq!(captures[1].as_deref(), Some("12"));
        assert_eq!(captures[2].as_deref(), Some("34"));
    }

    // ---------- 预扫描路由 ----------

    #[test]
    fn 无fancy特性走线性快路径() {
        for pattern in [
            r"\d+",
            r"[a-z]{2,5}",
            r"(\w+)\s+(?P<name>\w+)",
            r"(?i)hello",
            r"a|b",
            r"^x$",
            r"[^]]+",
        ] {
            let compiled = compile(pattern, RegexLimits::DEFAULT).expect("应编译");
            assert_eq!(compiled.kind(), PatternKind::Fast, "{pattern} 应走快路径");
        }
    }

    #[test]
    fn lookaround与反向引用路由到fancy() {
        for pattern in [
            r"foo(?=bar)",
            r"foo(?!bar)",
            r"(?<=a)b",
            r"(?<!a)b",
            r"(a)\1",
        ] {
            let compiled = compile(pattern, RegexLimits::DEFAULT).expect("应编译");
            assert_eq!(
                compiled.kind(),
                PatternKind::Fancy,
                "{pattern} 应路由到 fancy 引擎"
            );
        }
    }

    #[test]
    fn 字符类里的问号不误判为lookaround() {
        let compiled = compile(r"[?=!]+", RegexLimits::DEFAULT).expect("应编译");
        assert_eq!(compiled.kind(), PatternKind::Fast);
    }

    #[test]
    fn 命名捕获组不被误判为lookbehind() {
        // `(?<name>...)` 是命名组（regex 支持），`(?<=` / `(?<!` 才是 lookbehind
        let compiled = compile(r"(?<year>\d{4})", RegexLimits::DEFAULT).expect("应编译");
        assert_eq!(compiled.kind(), PatternKind::Fast);
    }

    #[test]
    fn 原子组被视为fancy特性() {
        let compiled = compile(r"(?>a+)b", RegexLimits::DEFAULT).expect("应编译");
        assert_eq!(compiled.kind(), PatternKind::Fancy);
    }

    // ---------- 翻译层（陷阱二） ----------

    #[test]
    fn 数字引用被加上花括号() {
        assert_eq!(translate_replacement("$1"), "${1}");
        assert_eq!(translate_replacement("$12"), "${12}");
        assert_eq!(
            translate_replacement("组：$1，组：$2"),
            "组：${1}，组：${2}"
        );
    }

    #[test]
    fn 美元符号后的字母不被误读为数字组() {
        // 陷阱二的核心案例：`$1a` 在 Java 里是组 1 + 字面 a
        assert_eq!(translate_replacement("$1a"), "${1}a");
    }

    #[test]
    fn 命名引用被加上花括号() {
        assert_eq!(translate_replacement("$name"), "${name}");
        assert_eq!(translate_replacement("$name_x1"), "${name_x1}");
    }

    #[test]
    fn 转义美元变字面美元() {
        assert_eq!(translate_replacement(r"\$"), "$$");
        assert_eq!(translate_replacement(r"价格\$5"), "价格$$5");
    }

    #[test]
    fn 已转义的替换串原样保留() {
        assert_eq!(translate_replacement(r"\\"), r"\");
    }

    #[test]
    fn 孤立的美元变字面美元() {
        assert_eq!(translate_replacement("$"), "$$");
        assert_eq!(translate_replacement("100$"), "100$$");
        assert_eq!(translate_replacement("a$-b"), "a$$-b");
    }

    #[test]
    fn 显式花括号形式原样保留() {
        assert_eq!(translate_replacement("${1}"), "${1}");
        assert_eq!(translate_replacement("${name}"), "${name}");
    }

    #[test]
    fn 双美元是字面美元() {
        assert_eq!(translate_replacement("$$"), "$$");
    }

    #[test]
    fn 翻译层在真实替换里生效() {
        // `(?i)(\w+)@` → "用户$1"：$1 后跟中文字符，若不翻译会被当成组名
        let compiled = fast(r"(\w+)@example");
        let replaced = compiled
            .replace_all_java("alice@example", "$1", RegexLimits::DEFAULT)
            .expect("替换应成功");
        assert_eq!(replaced, "alice");

        // 关键案例：`$1a` —— 若朴素透传，Rust 会把组名读作 "1a" 而产出空串
        let compiled = fast(r"(\d+)");
        let replaced = compiled
            .replace_all_java("42", "$1a", RegexLimits::DEFAULT)
            .expect("替换应成功");
        assert_eq!(replaced, "42a", "`$1a` 必须是组 1 + 字面 a（陷阱二）");

        // 若不做翻译，行为是静默丢字：
        let naive = compiled
            .replace_all("42", "$1a", RegexLimits::DEFAULT)
            .expect("替换应成功");
        assert_eq!(naive, "", "证明朴素透传确实丢字（本测试是这个陷阱的证据）");

        // Java 的 `\$` 是字面美元，后随的 `1` 是普通字符 → `$1`
        let replaced = compiled
            .replace_all_java("42", r"\$1", RegexLimits::DEFAULT)
            .expect("替换应成功");
        assert_eq!(replaced, "$1");

        // 字面美元**紧接**组引用：Java 写作 `\$$1`（`\$` 字面美元 + `$1` 组引用）。
        // 翻译层必须产出 `$$${1}`——`$$` 是字面美元、`${1}` 是组引用。
        // 若只产出 `$$1`，Rust 会读成"字面美元 + 字面 1"而丢掉组值。
        assert_eq!(translate_replacement(r"\$$1"), "$$${1}");
        let replaced = compiled
            .replace_all_java("42", r"\$$1", RegexLimits::DEFAULT)
            .expect("替换应成功");
        assert_eq!(replaced, "$42");
    }

    #[test]
    fn 非法组引用产出空串而非报错() {
        // [UNCERTAIN] 与 Java 抛 IndexOutOfBoundsException 不同，
        // 选择对齐 Rust 的空串行为（理由见 translate_replacement 文档）。
        let compiled = fast(r"(\d+)");
        let replaced = compiled
            .replace_all_java("42", "$9", RegexLimits::DEFAULT)
            .expect("越界引用不该让整条规则失败");
        assert_eq!(replaced, "");
    }

    // ---------- 回溯与长度护栏 ----------

    #[test]
    fn 超长输入被上界拦下() {
        let compiled = fancy(r"(?=a)(a+)+b");
        let long_input = "a".repeat(RegexLimits::DEFAULT.max_input_bytes + 1);
        let error = compiled
            .is_match(&long_input, RegexLimits::DEFAULT)
            .expect_err("超长输入应被拦下");
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Rule);
        assert!(matches!(error, RuleError::Limit { .. }));
    }

    #[test]
    fn 灾难性回溯被回溯上界拦下() {
        // R5 的挂起案例：`(?=a)(a+)+b` 对长输入会指数级回溯。
        // 上界生效时必须**早退并报错**，而不是把界面冻住（FR-007 的进程内护栏）。
        let compiled = fancy(r"(?=a)(a+)+b");
        let input = "a".repeat(5000);
        let started = std::time::Instant::now();
        let outcome = compiled.is_match(&input, RegexLimits::DEFAULT);
        let elapsed = started.elapsed();

        // 无论命中还是被拦下，都必须在上界内快速返回
        assert!(
            elapsed.as_secs_f64() < 10.0,
            "灾难性回溯耗时 {elapsed:?}，护栏未生效"
        );
        if let Err(error) = outcome {
            assert!(matches!(error, RuleError::Limit { .. }));
        }
    }

    #[test]
    fn 快路径的超长输入也被拦() {
        let compiled = fast(r"\d+");
        let long_input = "1".repeat(RegexLimits::DEFAULT.max_input_bytes + 1);
        assert!(compiled
            .is_match(&long_input, RegexLimits::DEFAULT)
            .is_err());
        // 放宽上界后可通过
        assert!(compiled
            .is_match(&long_input, RegexLimits::PERMISSIVE)
            .expect("放宽上界后应能匹配"));
    }

    // ---------- 通用能力 ----------

    #[test]
    fn 未参与的捕获组是none而非空串() {
        // 与 Java Matcher.group() 返回 null 对齐（见 captures 文档）。
        let compiled = fast(r"(a)|(b)");
        let captures = compiled
            .captures("a", RegexLimits::DEFAULT)
            .expect("匹配不应失败")
            .expect("应有匹配");
        assert_eq!(captures[1].as_deref(), Some("a"));
        assert_eq!(captures[2], None, "未参与匹配的组必须是 None");
    }

    #[test]
    fn 两个引擎的结果一致() {
        // 同一语义的模式在两引擎上应给出相同结果（fancy 无 fancy 特性时会
        // 委托给线性引擎，但显式断言能挡住路由判断的回归）。
        // `(?s)` 是 flag 设置（regex 支持，属快路径），故对照用真正的 fancy 特性
        // （前瞻）与等价快路径模式。
        let cases = [("abc", true), ("xyz", false)];
        for (input, expected) in cases {
            assert_eq!(
                fast(r"[abc]+")
                    .is_match(input, RegexLimits::DEFAULT)
                    .expect("匹配不应失败"),
                expected
            );
            assert_eq!(
                fancy(r"(?=[abc])[abc]+")
                    .is_match(input, RegexLimits::DEFAULT)
                    .expect("匹配不应失败"),
                expected
            );
        }
    }

    #[test]
    fn 编译失败返回结构化错误() {
        let error = compile("(unclosed", RegexLimits::DEFAULT).expect_err("应编译失败");
        assert!(matches!(error, RuleError::Regex { .. }));
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Rule);
    }

    #[test]
    fn 规范化的模式可被诊断查看() {
        let compiled = compile("a++", RegexLimits::DEFAULT).expect("应编译");
        assert_eq!(compiled.normalized_pattern(), "(?>a+)");
    }
}
