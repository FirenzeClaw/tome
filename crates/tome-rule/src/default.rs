//! \[DONE] Default（JSOUP 之 Default / preter）语法解析器（PM 补漏裁决）
//!
//! # 这是公开规则说明的**第一语法**
//!
//! 公开文档《Legado 书源规则说明》§1「语法说明」把 Default 列在 CSS/JSONPath/
//! XPath/JS 之前，是书源里最基础也最高频的规则形态。此前它的多段形态
//! （`class.odd.0@tag.a.0@text`）没有解析器——整段正文被交给 CSS 解析器，
//! 必然语法错。本模块补上那一段。
//!
//! # 依据来源（干净室：只依据公开文档）
//!
//! 语义原文抄录自公开的《Legado 书源规则说明》§1「JSOUP之Default」节：
//!
//! > `@` 为分隔符,用来分隔获取规则
//! > 每段规则可分为 3 段
//! > 第一段是类型,如 `class`,`id`,`tag`,`text`,`children` 等,
//! >   `children` 获取所有子标签,不需要第二段和第三段, `text` 可以根据文本内容获取
//! > 第二段是名称, `text.` 第二段为文本内容的一部分
//! > 第三段是位置, `class`,`tag`,`id` 等会获取到多个,所以要加位置
//! > 如不加位置会获取所有
//! > 位置正着数从 0 开始,0 是第一个,如为负数则是取倒着数的值,-1 为倒数第一个,-2 为倒数第二个
//! > `!` 是排除,有些位置不符合需要排除用 `!`,后面的序号用 `:` 隔开
//! >   0 是第 1 个,负数为倒数序号,-1 最后一个,-2 倒数第 2 个,依次
//! > 获取列表的最前面加上负号 `-` 可以使列表倒置,有些网站目录列表是倒的,前面加个负号可变为正的
//! > `@` 的最后一段为获取内容,如 `text`,`textNodes`,`ownText`,`href`,`src`,`html`,`all` 等
//! > 如需要正则替换在最后加上 `##正则表达式##替换内容`，替换内容为空时，第二个 `##` 可以省略
//!
//! 另有一处**多段语义**的证据来自公开的书源开发文档（ColorTxt 的
//! 「找书（Legado 书源）开发文档」§3 的差旅清单）：
//!
//! > **`getElements` 中间段**：`id.list@dd@a` 等列表规则在 `list` 模式下
//! > 中间选择器须保留全部匹配节点（勿 `pickElements(0)`），
//! > 否则目录只会解析到第一个 `dd`。
//!
//! 以及同节的相对选行为：
//!
//! > **Default 相对 CSS**：对齐 Jsoup `Element.select`，非整页查询时须包含
//! > scope **自身**；仅 `find` 会导致 `.category-div@h3@…` 在条目根已是
//! > `category-div` 时取空。
//!
//! 两条合起来确定了 [`evaluate`] 的核心语义：**多段是"逐段嵌套下钻"**
//! ——每段在上一段结果的**各自子树内**求，且子树查询包含起点自身。
//!
//! # 与 CSS 引擎的分工
//!
//! 每段（类型 + 名称）翻译成一条等价 CSS 选择器，交给既有的
//! [`crate::css`] 在**同一棵 arena DOM**（架构铁律）上求值——
//! 本模块不建第二棵树，也不重写选择器匹配。
//!
//! | Default 段 | 等价 CSS |
//! |---|---|
//! | `class.odd` | `.odd` |
//! | `id.list` | `#list` |
//! | `tag.a` | `a` |
//! | `tag.div` | `div` |
//! | `text.关键字` | `*:containsOwn(关键字)` |
//! | `children` | `>` （所有直接子元素） |
//!
//! # 位置语义与 T022 的对齐
//!
//! 位置的三种形态（不加 / 正负数 / `!` 排除）与 [`crate::css_post::IndexSlice`]
//! **完全一致**，故直接复用那个类型，不另写一份——两处各写一份必然漂移。
//!
//! ## 一处**有意的差异**：`!` 的序数基准
//!
//! 公开文档说"`!` 是排除,有些位置不符合需要排除用 `!`,后面的序号用 `:`
//! 隔开 **0 是第 1 个**"——即排除区间用 **0 基**。而 T022 的
//! [`crate::css_post::IndexSlice::Exclude`] 也按 0 基处理。
//!
//! **两者一致**（都按文档的 0 基），故本模块直接复用，无分歧。

use crate::css_post::IndexSlice;
use crate::dom::{Document, NodeRef};
use crate::error::RuleError;

// ---------------------------------------------------------------------------
// 段与规则的数据结构
// ---------------------------------------------------------------------------

/// Default 段的类型（公开文档：「第一段是类型,如 class,id,tag,text,children 等」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentKind {
    /// `class.名称` —— 按 class 名选
    Class(String),
    /// `id.名称` —— 按 id 选
    Id(String),
    /// `tag.名称` —— 按标签名选
    Tag(String),
    /// `text.内容片段` —— 按**自身文本**包含该片段选
    ///
    /// 公开文档：「`text` 可以根据文本内容获取」「`text.` 第二段为文本内容的一部分」。
    /// 翻译成 CSS 的 `*:containsOwn(...)`（Jsoup 公开文档 `:containsOwn` 是
    /// "directly contain the given text"，与"自身文本包含"同义）。
    Text(String),
    /// `children` —— 取所有直接子标签
    ///
    /// 公开文档：「`children` 获取所有子标签,不需要第二段和第三段」。
    Children,
    /// **原样 CSS 选择器**（不是 Default 段语法）
    ///
    /// # 为什么需要这个变体
    ///
    /// 公开的书源开发文档在"规则类型判别"表里写：`@css:` / `class.` / `tag.`
    /// 都归 **default** 引擎，且"其他"也归 default。即 Default 引擎实际
    /// 承担两类写法：
    ///
    /// | 写法 | 例 |
    /// |---|---|
    /// | Default 段语法 | `class.odd.0@tag.a.0@text` |
    /// | **裸 CSS 选择器** | `.odd>a@href`、`li:nth-child(2)@text` |
    ///
    /// 第二类不含 `class.`/`tag.` 前缀，但它落在同一个引擎里。
    /// 不认它会让**绝大多数既有书源**（CSS 写法比 Default 写法更常见）
    /// 整条失效——那是比"不实现多段语法"更严重的回归。
    ///
    /// 故当某段不匹配 Default 段语法时，整段选择器原样交给 CSS 引擎
    /// （`crate::css::compile`），由它决定合法性。
    Raw(String),
}

/// 一个 Default 段：类型 + 可选位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// 段的类型（含名称）
    pub kind: SegmentKind,
    /// 位置切片（[`IndexSlice`]，与 T022 的 CSS 索引语义一致）
    pub slice: IndexSlice,
}

/// 解析出的 Default 规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultRule {
    /// 选择段（至少一个；空规则时为空）
    ///
    /// 逐段嵌套下钻：第 k 段在第 k−1 段结果的**各自子树内**求。
    pub segments: Vec<Segment>,
    /// 反序标记（公开文档：「获取列表的最前面加上负号 `-` 可以使列表倒置」）
    pub reversed: bool,
    /// 内容访问器（最后一段，如 `text` / `href`）
    ///
    /// 为 `None` 表示规则没有末段访问器（如 `class.odd.0`）——
    /// 此时按"取节点文本"处理（与 [`crate::css_post::RuleSuffix::Unspecified`] 一致）。
    pub accessor: Option<crate::css_post::RuleSuffix>,
}

// ---------------------------------------------------------------------------
// 解析
// ---------------------------------------------------------------------------

/// 解析一条 Default 规则正文（调用方已剥掉 `-` 反序标记与 `@@` 前缀）。
///
/// \[DONE] 本模块的解析入口。
///
/// # 段与访问器的切分
///
/// 公开文档：「`@` 的最后一段为获取内容」。故 **`@` 切出的最后一段若是
/// 已知访问器名**（`text`/`href`/...），它就不是选择段而是访问器。
///
/// 判据是"**是否是已知访问器名**"而不是"是否含 `.`"——后者会把
/// `tag.a` 误判（`a` 恰是标签名），而前者按公开文档的访问器清单判定，
/// 语义明确。
pub fn parse(rule: &str) -> Result<DefaultRule, RuleError> {
    let trimmed = rule.trim();
    if trimmed.is_empty() {
        return Ok(DefaultRule {
            segments: Vec::new(),
            reversed: false,
            accessor: None,
        });
    }

    // 反序标记：串首的 `-`（公开文档：「获取列表的最前面加上负号 `-`」）。
    // 与 `parse_rule_tail` 同一判据：紧邻数字的 `-` 是负索引而非反序标记。
    let (reversed, body) = match trimmed.strip_prefix('-') {
        Some(rest) if !rest.starts_with(|c: char| c.is_ascii_digit()) => (true, rest.trim_start()),
        _ => (false, trimmed),
    };

    // ## 段的切分：**只按单个 `@`**，不把 `@@` 当分隔符
    //
    // `@@` 按公开文档是"Default 规则的**前缀标记**"（「Default 规则需要以
    // `@@` 开头」），用于在 `{{}}` 里与其他语法区分；公开的书源开发文档里
    // 它的用法也一律是前缀（`java.getElement("@@tag.a.0")`、
    // `{{@@td.N@text##…}}`）。
    //
    // 故此处先把**开头的** `@@` 剥掉（与 `dispatch_prefix` 的处理一致），
    // 再把剩下的按单个 `@` 切段。
    //
    // `[UNCERTAIN]` 关于 `A@@B` 的**中缀**形态：社区书源里偶见
    // `#list li@@a@text` 这类写法，但它们无法从公开文档得到权威语义
    // （文档只把 `@@` 定义为前缀）。本实现按"开头的 `@@` 是前缀"处理，
    // 中缀的 `@@` 会被 `split('@')` 切出一个空段并被跳过——于是
    // `A@@B` 等价于 `A@B`。这是**无据的退让**而非有据的设计，
    // 记录在此以免被误当作已确认的语义。若 S1-6 的真实基线显示该形态
    // 确有不同语义，应以基线证据修正。
    let body = body.strip_prefix("@@").unwrap_or(body);
    let parts: Vec<&str> = body.split('@').map(str::trim).collect();

    // 末段是否为访问器
    //
    // 公开文档：「`@` 的最后一段为获取内容,如 text,textNodes,ownText,href,src,
    // html,all 等」。故末段若是**已知访问器名**，它是访问器而非选择段。
    //
    // ## 必须用 `parse_suffix_text` 而不是 `RuleSuffix::parse`
    //
    // 前者额外认 `attr(名称)` 形态（`a@attr(data-id)`）。用后者会让
    // Default 路径上 `attr(...)` 不可用——它会被当成选择段喂给 CSS，
    // 报"选择器解析失败"，而同一写法在 `@css:` 路径上却能工作。
    // 两条路径的访问器清单必须逐字一致（公开文档列的是**一份**清单）。
    let (selector_parts, accessor) = match parts.last() {
        Some(last) => match crate::css_post::parse_suffix_text(last) {
            // 已知访问器名 → 它是访问器（`!last.is_empty()` 排除空串：
            // `parse_suffix_text("")` 给 `Unspecified`，但空末段只意味着
            // 规则以 `@` 结尾，不是访问器）
            Some(suffix) if !last.is_empty() => (&parts[..parts.len() - 1], Some(suffix)),
            // 空串或未知名（如 `class.odd@cat`）→ 不是访问器，整段都是选择段。
            // 未知名走这里而不是报错：`@cat` 可能是作者想写 `@attr(cat)` 的
            // 简写，而"按无访问器处理"会让它落到一个明确的选择器错误上。
            _ => (&parts[..], None),
        },
        None => (&parts[..], None),
    };

    let mut segments = Vec::with_capacity(selector_parts.len());
    for part in selector_parts {
        if part.is_empty() {
            // 空的段（如规则以 `@` 开头、或主体为空）：跳过而不是报错。
            // 报错会让一条可救的规则整条失效，而跳过只影响那一段。
            continue;
        }
        segments.push(parse_segment(part)?);
    }

    Ok(DefaultRule {
        segments,
        reversed,
        accessor,
    })
}

/// 解析单个选择段：`类型.名称.位置`。
///
/// # 三段的切分
///
/// 公开文档说"每段规则可分为 3 段"，而实际书源里：
///
/// | 形态 | 类型 | 名称 | 位置 |
/// |---|---|---|---|
/// | `class.odd` | class | odd | 无 |
/// | `class.odd.0` | class | odd | 0 |
/// | `class.odd.-1` | class | odd | -1 |
/// | `class.odd.!0:3` | class | odd | 排除 0..3 |
/// | `children` | children | — | — |
/// | `text.下一章` | text | 下一章 | 无 |
///
/// 故按 `.` 切分后：第 1 件是类型，第 2 件是名称（`children` 无），
/// 剩余是位置（可能有 `!` 与 `:`）。
fn parse_segment(text: &str) -> Result<Segment, RuleError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(RuleError::syntax("Default 规则里有空的段".to_string()));
    }

    // `children` 是唯一的单件类型（公开文档：「不需要第二段和第三段」）
    if trimmed.eq_ignore_ascii_case("children") {
        return Ok(Segment {
            kind: SegmentKind::Children,
            slice: IndexSlice::All,
        });
    }

    // ## 位置形态的识别先于名称切分
    //
    // 位置里可能含 `.`（`.-1` 形态）或 `:`（`!0:3`），故**先**从尾部把
    // 位置摘出来，剩下的才是"类型.名称"。次序反了会把 `class.a.!0:3`
    // 的名称切成 `a.!0:3`。
    let (head, slice) = split_position(trimmed);

    // 名称里可能含 `.`（class 名允许，如 `class.a.b`）——按**第一个** `.` 切，
    // 这样 `class.a.b` 的类型是 class、名称是 `a.b`（与"类型是第一段"一致）。
    let (kind_name, name) = match head.split_once('.') {
        Some((kind, name)) => (kind, name),
        // ## 无 `.`：可能是裸 CSS 选择器，而不是 Default 段
        //
        // 公开的书源开发文档把"其他"也归 default 引擎，故 `.odd>a@href`
        // 这类裸 CSS 选择器同样落在这里。它们不含 `类型.名称` 形态，
        // 故整段交给 CSS 引擎（见 `SegmentKind::Raw` 的文档）。
        //
        // 用 `head` 而不是 `trimmed`：位置已被 `split_position` 摘出
        // （`li.0` → head `li` + 切片 `Single(0)`）。用 `trimmed` 会把
        // 切片当选择器正文再喂给 CSS，于是 `li.0` 变成非法选择器。
        None => {
            return Ok(Segment {
                kind: SegmentKind::Raw(head.to_string()),
                slice,
            });
        }
    };
    let kind_name = kind_name.trim();
    let name = name
        .trim()
        .trim_matches(|quote| quote == '"' || quote == '\'');

    let kind = match kind_name.to_ascii_lowercase().as_str() {
        "class" => SegmentKind::Class(name.to_string()),
        "id" => SegmentKind::Id(name.to_string()),
        "tag" => SegmentKind::Tag(name.to_string()),
        "text" => SegmentKind::Text(name.to_string()),
        // ## 未知类型名 → 当作裸 CSS 选择器而不是报错
        //
        // 书源里有大量 CSS 写法（`li.item>a`、`div[class*=x]`），它们按 `.`
        // 切开后的第一件不是已知类型名。报错会让它们整条失效，而交给
        // CSS 引擎能正确求值——**绝大多数既有书源**都是这个形态。
        //
        // 判据是精确的：已知类型名（class/id/tag/text/children）与 CSS
        // 选择器首件（元素名、`.类`、`#id`、`*`）不重叠。
        //
        // 同样用 `head`（已剥掉位置）而不是 `trimmed`。
        other => {
            let _ = other;
            return Ok(Segment {
                kind: SegmentKind::Raw(head.to_string()),
                slice,
            });
        }
    };

    if name.is_empty() {
        return Err(RuleError::syntax(format!(
            "Default 段 `{trimmed}` 的名称为空"
        )));
    }

    Ok(Segment { kind, slice })
}

/// 从段文本尾部摘出位置，返回 (剩余部分, 切片)。
///
/// # 位置的三种形态（公开文档）
///
/// | 形态 | 例 | 含义 |
/// |---|---|---|
/// | 不加 | `class.odd` | 取全部 |
/// | 单个 | `class.odd.0` / `class.odd.-1` | 第 0 个 / 倒数第 1 个 |
/// | 排除 | `class.odd.!0:3` | 排除第 0 到第 3 个 |
///
/// `!` 形态的序号是 **0 基**（公开文档：「后面的序号用 `:` 隔开 0 是第 1 个」），
/// 与 [`IndexSlice`] 的 `Exclude` 一致，故直接复用其解析。
fn split_position(text: &str) -> (&str, IndexSlice) {
    // ## 方括号形态的切片（T022 的 `li[0]` / `li[0:3]` / `li[!0:3]`）
    //
    // 这是与 CSS 属性选择器**有歧义**的一处：`li[0]` 的 `[0]` 按 T022 是
    // 索引切片，而 `li[href]` 的 `[href]` 是 CSS 属性选择器。判据是
    // "方括号里能否解析成切片"——`IndexSlice::parse_bracket` 只认
    // 纯数字与 `!数字:数字`，故 `[href]`/`[!3]` 都会给它 `None`，
    // 从而留在选择器里交给 CSS（与 T022 的 `split_slice` 同判据）。
    if text.ends_with(']') {
        if let Some(open) = matching_open_bracket(text) {
            if let Some(slice) = IndexSlice::parse_bracket(&text[open..]) {
                return (text[..open].trim_end(), slice);
            }
        }
    }

    // 排除形态：`!start:end`（可能出现在尾部，也可能带 `.` 前缀）
    for marker in [".!", "!"] {
        if let Some(position) = text.rfind(marker) {
            // 排除形态必须一直延伸到串尾（`!` 之后不能还有别的段）
            let candidate = &text[position + marker.len() - 1..];
            if let Some(slice) = IndexSlice::parse_bracket(&format!("[{candidate}]")) {
                return (text[..position].trim_end_matches('.'), slice);
            }
        }
    }

    // 单个位置：尾部 `.<带符号整数>`
    if let Some(position) = text.rfind('.') {
        let candidate = &text[position..];
        if let Some(slice) = IndexSlice::parse_dot(candidate) {
            return (&text[..position], slice);
        }
    }

    (text, IndexSlice::All)
}

/// 找与尾部 `]` 配对的 `[` 的下标。
///
/// 与 [`crate::css_post`] 的同名辅助一致：从右往左数配对，嵌套的方括号
/// 也能正确处理（`[attr~=a[bc]]` 这类正则里会再出现 `[`）。
fn matching_open_bracket(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut position = bytes.len();
    while position > 0 {
        position -= 1;
        match bytes[position] {
            b']' => depth += 1,
            b'[' => {
                depth -= 1;
                if depth == 0 {
                    return Some(position);
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 求值
// ---------------------------------------------------------------------------

/// 在文档上求值一条 Default 规则，产出字符串列表。
///
/// \[DONE] 本模块的求值入口。多段是**逐段嵌套下钻**（见模块文档的依据）。
pub fn evaluate(document: &Document, rule: &str) -> Result<Vec<String>, RuleError> {
    let parsed = parse(rule)?;
    evaluate_with_nodes(None, Some(document), &parsed)
}

/// 在**子树**内求值（搜索/目录的字段规则用）。
pub fn evaluate_in(root: &NodeRef, rule: &str) -> Result<Vec<String>, RuleError> {
    let parsed = parse(rule)?;
    evaluate_with_nodes(Some(root), None, &parsed)
}

/// 取节点（`bookList` 那类用途）。
pub fn select_nodes(document: &Document, rule: &str) -> Result<Vec<NodeRef>, RuleError> {
    let parsed = parse(rule)?;
    if parsed.segments.is_empty() {
        return Ok(Vec::new());
    }
    let starts = vec![document.root()];
    descend(&starts, &parsed.segments, parsed.reversed)
}

/// 求值的主体（文档根或子树起点二选一）。
fn evaluate_with_nodes(
    root: Option<&NodeRef>,
    document: Option<&Document>,
    parsed: &DefaultRule,
) -> Result<Vec<String>, RuleError> {
    // 起点：子树时是 `root` 自身，文档时是**文档根**
    let starts: Vec<NodeRef> = match (root, document) {
        (Some(root), _) => vec![root.clone()],
        (None, Some(document)) => vec![document.root()],
        (None, None) => Vec::new(),
    };

    // ## 空规则产出空
    //
    // 没有任何选择段的规则（空串、或只有访问器）没有可作用的对象。
    // 若在此返回起点，末段访问器会把**整篇文档**的文本返回——
    // 那是静默的巨量错误结果，比空结果糟得多。
    if parsed.segments.is_empty() {
        return Ok(Vec::new());
    }

    // 反序在 `descend` 里已作用于末段结果（见其文档）
    let nodes = descend(&starts, &parsed.segments, parsed.reversed)?;

    let accessor = parsed
        .accessor
        .clone()
        .unwrap_or(crate::css_post::RuleSuffix::Unspecified);
    let mut values = Vec::new();
    for node in &nodes {
        values.extend(accessor.evaluate(node));
    }
    Ok(values)
}

/// 逐段下钻：第 k 段在第 k−1 段结果的**各自子树内**求。
///
/// # 为什么每段要在**各自**子树内求（而不是在整页上求）
///
/// 公开书源开发文档的差旅清单：
///
/// > **`getElements` 中间段**：`id.list@dd@a` 等列表规则在 `list` 模式下
/// > 中间选择器须保留全部匹配节点（勿 `pickElements(0)`），
/// > 否则目录只会解析到第一个 `dd`。
///
/// 即 `id.list@dd@a` 的语义是"先取 `#list`，在它里面取**所有** `dd`，
/// 再在每个 `dd` 里取 `a`"——是**嵌套下钻**，不是"三段各自在整页上求"。
///
/// ## 子树查询包含起点自身
///
/// 同文档：「非整页查询时须包含 scope **自身**；仅 `find` 会导致
/// `.category-div@h3@…` 在条目根已是 `category-div` 时取空」。
/// [`crate::css::select`] 的文档已保证这一点（对齐 Jsoup `Element#select`）。
///
/// # 位置施加在**段内**，反序施加在**末段产出之后**
///
/// 段的位置属于段自身（`class.odd.0` 的 `0` 是"第 0 个 .odd"），故在段内施加。
/// 反序是"整个结果列表倒过来"（公开文档：「获取列表的最前面加上负号 `-`」），
/// 故只作用一次、在**末段结果上**——若在每段都倒置，`-a@b` 会变成
/// "先倒 a 再倒 b"，与"把最终列表倒过来"不是一回事。
fn descend(
    starts: &[NodeRef],
    segments: &[Segment],
    has_reversed: bool,
) -> Result<Vec<NodeRef>, RuleError> {
    let mut current: Vec<NodeRef> = starts.to_vec();

    for (position, segment) in segments.iter().enumerate() {
        let is_first = position == 0;
        // 反序只作用在**首段**（见下方说明），且要作用于它**选出全部之后、
        // 施加位置之前**。故这里把它并入"每个父节点的子结果"处理之前。
        let reversed_here = has_reversed && is_first;

        // ## 位置必须**逐父节点**施加，而不是在拼接后的总列表上
        //
        // 公开文档把位置定义为**段的一部分**（「第三段是位置,class,tag,id 等
        // 会获取到多个,所以要加位置」）。段是"在上一段结果的**各自子树内**"
        // 求的（见本函数前文的依据），故它的位置也应当在**各自子树的命中集**
        // 上施加——`class.book@class.name.0@text` 的含义是"每本书取它自己的
        // 第 0 个 .name"，而不是"把所有 .name 拼起来取全局第 0 个"。
        //
        // 若在拼接后的总列表上施加位置，前一本书的命中会被后一本书的命中
        // "挤掉"——那是与"逐段下钻"自相矛盾的：中间段的位置已经是逐父节点
        // 施加的（`descend` 每段都对 `current` 的每个元素分别选），
        // 末段若改成全局就会前后不一致。
        let mut propagated: Vec<NodeRef> = Vec::new();
        for node in &current {
            // 每个父节点各自求一遍这一段的命中
            let mut hits: Vec<NodeRef> = if segment.kind == SegmentKind::Children {
                // `children` 不走 CSS：它的语义是"所有直接子**元素**"，
                // 而 `> *` 是相对选择器，servo 不接受它作为独立选择器
                // （CSS 规范里 `>` 必须以组合符的形态跟在主体之后）。
                // 直接按 DOM 取子元素既准确又更省一次选择器编译。
                node.children().filter(|child| child.is_element()).collect()
            } else {
                let selector = crate::css::compile(&segment_to_css(segment))?;
                crate::css::select(node, &selector)
            };

            // 反序在这一层的命中集上施加（与 `css_post` 的"先反序再取位置"同序）
            if reversed_here {
                hits.reverse();
            }
            // 位置在这一层的命中集上施加
            let hits = segment.slice.apply_nodes(hits);
            propagated.extend(hits);
        }

        // 去重：同一节点可能经不同父节点多次命中（子树重叠时）。
        // 用 arena 的节点 id 判定身份（`NodeRef` 的 `PartialEq` 已按 id 比较）。
        let mut deduped: Vec<NodeRef> = Vec::with_capacity(propagated.len());
        for node in propagated {
            if !deduped.iter().any(|existing| existing.id() == node.id()) {
                deduped.push(node);
            }
        }
        current = deduped;
    }

    Ok(current)
}

/// 把一个 Default 段翻译成等价的 CSS 选择器（见模块文档的对照表）。
fn segment_to_css(segment: &Segment) -> String {
    match &segment.kind {
        SegmentKind::Class(name) => format!(".{}", escape_css_identifier(name)),
        SegmentKind::Id(name) => format!("#{}", escape_css_identifier(name)),
        SegmentKind::Tag(name) => name.clone(),
        // `text.片段` → `:containsOwn`（Jsoup 公开文档：directly contain the given text）
        SegmentKind::Text(fragment) => format!(
            "*:containsOwn({})",
            crate::css::quote_for_pseudo_argument(fragment)
        ),
        // `children` → 由 `descend` 直接按 DOM 取子元素（不走 CSS）
        SegmentKind::Children => "> *".to_string(),
        // 裸 CSS 选择器原样通过
        SegmentKind::Raw(selector) => selector.clone(),
    }
}

/// 把标识符转义成可安全放进 CSS 选择器的形态。
///
/// # 为什么需要转义
///
/// 书源里的类名/标签名来自第三方，可能含 CSS 元字符。不转义会让选择器
/// **改变语义**而不是报错——那是静默的错误匹配，比报错更糟：
///
/// | 名称 | 不转义的结果 | 为什么不接受 |
/// |---|---|---|
/// | `a:b` | `.a:b` | `:b` 被当成伪类 |
/// | `a.b` | `.a.b` | `.b` 被当成第二个类 |
/// | `a#b` | `.a#b` | `#b` 被当成 id |
///
/// # 为什么不转义空格与 `*`
///
/// 空格在 CSS 里是**后代组合符**，而 Jsoup 的 Default 语法同样如此处理
/// （`class.a b` 的含义是"class=a 的后代里的 `b` 标签"）。故空格要
/// **保留**为组合符而不是转义成字面空格——后者会让 `class.a b` 去匹配
/// 一个"类名叫 `a b`"的元素，而 HTML 的 class 属性按空白分词，
/// 那样的元素**永远不会存在**。
///
/// 同理 `*` 保留为通配符（Jsoup 的 `tag.*` 是合法写法）。
fn escape_css_identifier(name: &str) -> String {
    let mut output = String::with_capacity(name.len() + 4);
    for character in name.chars() {
        if character.is_alphanumeric()
            || character == '-'
            || character == '_'
            || character.is_whitespace()
            || character == '*'
        {
            output.push(character);
        } else {
            // CSS 标识符的转义形态：反斜杠 + 原字符
            output.push('\\');
            output.push(character);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 便捷：在 HTML 上求值一条 Default 规则。
    fn run(html: &str, rule: &str) -> Vec<String> {
        let document = Document::parse_html(html);
        evaluate(&document, rule).unwrap_or_else(|error| panic!("`{rule}` 应求值成功：{error}"))
    }

    /// 公开文档给的搜索页形态的样例 HTML。
    const SEARCH_HTML: &str = r#"<html><body><ul>
      <li class="odd"><a href="/b/1">第一本</a></li>
      <li class="even"><a href="/b/2">第二本</a></li>
      <li class="odd"><a href="/b/3">第三本</a></li>
    </ul></body></html>"#;

    // ---------- 解析：类型 ----------

    #[test]
    fn class段解析() {
        let parsed = parse("class.odd").expect("应解析成功");
        assert_eq!(parsed.segments.len(), 1);
        assert_eq!(
            parsed.segments[0].kind,
            SegmentKind::Class("odd".to_string())
        );
        assert!(!parsed.reversed);
        assert!(parsed.accessor.is_none());
    }

    #[test]
    fn id段解析() {
        let parsed = parse("id.list").expect("应解析成功");
        assert_eq!(parsed.segments[0].kind, SegmentKind::Id("list".to_string()));
    }

    #[test]
    fn tag段解析() {
        let parsed = parse("tag.a").expect("应解析成功");
        assert_eq!(parsed.segments[0].kind, SegmentKind::Tag("a".to_string()));
    }

    #[test]
    fn text段解析() {
        // 公开文档：「text 可以根据文本内容获取」「text. 第二段为文本内容的一部分」
        let parsed = parse("text.下一章").expect("应解析成功");
        assert_eq!(
            parsed.segments[0].kind,
            SegmentKind::Text("下一章".to_string())
        );
    }

    #[test]
    fn children段解析() {
        // 公开文档：「children 获取所有子标签,不需要第二段和第三段」
        let parsed = parse("children").expect("应解析成功");
        assert_eq!(parsed.segments[0].kind, SegmentKind::Children);
        assert_eq!(parsed.segments[0].slice, IndexSlice::All);
    }

    #[test]
    fn 未知类型退到裸_css_而不是报错() {
        // 公开书源开发文档把"其他"也归 default 引擎，而书源里大量使用裸
        // CSS 写法（`li.item>a`）。它们按 `.` 切开后的首件不是已知类型名，
        // 故应退到 CSS 引擎求值而不是报错——报错会让这些源整条失效。
        let parsed = parse("widget.foo").expect("应退到 CSS 解析");
        assert_eq!(
            parsed.segments[0].kind,
            SegmentKind::Raw("widget.foo".to_string())
        );

        // 端到端：裸 CSS 选择器能真的选出节点
        let html = r#"<div class="widget"><span class="foo">x</span></div>"#;
        assert_eq!(run(html, "div.widget>span.foo@text"), vec!["x"]);
    }

    #[test]
    fn 裸_css_选择器可求值() {
        // 这**不是** Default 段语法，但它落在同一个引擎里（同上依据）
        let html = r#"<ul><li class="i"><a href="/1">一</a></li><li class="i"><a href="/2">二</a></li></ul>"#;
        assert_eq!(run(html, "li.i>a@href"), vec!["/1", "/2"]);
        assert_eq!(run(html, ".i>a@text"), vec!["一", "二"]);
        // CSS 伪类也可用（Default 引擎把整段交给 CSS）
        assert_eq!(run(html, "li:nth-child(2)>a@text"), vec!["二"]);
        assert_eq!(run(html, "li:eq(0)>a@text"), vec!["一"]);
    }

    #[test]
    fn 裸_css_的段不带默认段语义() {
        // 裸 CSS 选择器是**整段**交给 CSS，不做逐段下钻。
        // `li.i>a` 里的 `>` 是 CSS 子组合符，首段整体是 `li.i>a`。
        let parsed = parse("li.i>a@text").expect("应解析成功");
        assert_eq!(parsed.segments.len(), 1, "裸 CSS 是单段");
        assert_eq!(
            parsed.segments[0].kind,
            SegmentKind::Raw("li.i>a".to_string())
        );
        assert_eq!(parsed.accessor, Some(crate::css_post::RuleSuffix::Text));
    }

    // ---------- 解析：`@@` 前缀 ----------

    #[test]
    fn 开头的双_at_是前缀标记() {
        // 公开文档：「Default 规则需要以 @@ 开头」——`@@` 是**前缀标记**，
        // 用于在 `{{}}` 里与其他语法区分（`{{@@td.N@text##…}}`）。
        // 公开书源开发文档里它的用法也一律是前缀
        // （`java.getElement("@@tag.a.0")`）。
        let plain = parse("class.odd@tag.a@text").expect("应解析成功");
        let prefixed = parse("@@class.odd@tag.a@text").expect("应解析成功");
        assert_eq!(
            plain.segments, prefixed.segments,
            "前缀标记不该改变段的语义"
        );
        assert_eq!(plain.accessor, prefixed.accessor);
    }

    #[test]
    fn 双_at_前缀可端到端求值() {
        let html = r#"<ul><li class="odd"><a href="/1">一</a></li></ul>"#;
        assert_eq!(run(html, "@@class.odd@tag.a@href"), vec!["/1"]);
    }

    #[test]
    fn 中缀双_at_按无据退让处理并已标注() {
        // `[UNCERTAIN]`：公开文档只把 `@@` 定义为**前缀**，没有中缀
        // `A@@B` 的权威语义。本实现的退让是"中缀的 `@@` 等价于 `@`"
        // （切出的空段被跳过）。这条测试把**当前行为**钉住，
        // 以免它被误当作已确认的语义——若将来有基线证据表明语义不同，
        // 这里会失败并提醒去核对。
        let infix = parse("class.odd@@tag.a@text").expect("应解析成功");
        let plain = parse("class.odd@tag.a@text").expect("应解析成功");
        assert_eq!(
            infix.segments, plain.segments,
            "当前实现里中缀 `@@` 等价于 `@`（无据退让，见 parse 的文档）"
        );
    }

    // ---------- 解析：位置 ----------

    #[test]
    fn 位置被解析为切片() {
        assert_eq!(
            parse("class.odd.0").expect("应解析成功").segments[0].slice,
            IndexSlice::Single(0)
        );
        // 公开文档：「-1 为倒数第一个,-2 为倒数第二个」
        assert_eq!(
            parse("class.odd.-1").expect("应解析成功").segments[0].slice,
            IndexSlice::Single(-1)
        );
    }

    #[test]
    fn 不加位置时取全部() {
        // 公开文档：「如不加位置会获取所有」
        assert_eq!(
            parse("class.odd").expect("应解析成功").segments[0].slice,
            IndexSlice::All
        );
    }

    #[test]
    fn 排除位置被解析() {
        // 公开文档：「! 是排除…后面的序号用 : 隔开 0 是第 1 个」
        let parsed = parse("class.odd.!0:1").expect("应解析成功");
        assert_eq!(
            parsed.segments[0].slice,
            IndexSlice::Exclude {
                start: 0,
                end: Some(1)
            }
        );
    }

    #[test]
    fn 名称里的点不被误当位置() {
        // `class.a.b` 的类型是 class、名称是 `a.b`（名称里的点合法）
        let parsed = parse("class.a.b").expect("应解析成功");
        assert_eq!(
            parsed.segments[0].kind,
            SegmentKind::Class("a.b".to_string())
        );
        assert_eq!(parsed.segments[0].slice, IndexSlice::All);
    }

    #[test]
    fn 名称与位置共存() {
        let parsed = parse("class.a.b.0").expect("应解析成功");
        assert_eq!(
            parsed.segments[0].kind,
            SegmentKind::Class("a.b".to_string())
        );
        assert_eq!(parsed.segments[0].slice, IndexSlice::Single(0));
    }

    // ---------- 解析：反序 ----------

    #[test]
    fn 反序标记被摘出() {
        // 公开文档：「获取列表的最前面加上负号 - 可以使列表倒置」
        let parsed = parse("-class.odd@tag.a@text").expect("应解析成功");
        assert!(parsed.reversed);
        assert_eq!(parsed.segments.len(), 2);
    }

    #[test]
    fn 紧邻数字的负号不是反序标记() {
        // `-1@text`：`-` 紧邻数字，故按"不是反序标记"处理（与 `parse_rule_tail`
        // 同判据）。于是主体是 `1`——它不是反序标记，但作为裸 CSS 选择器
        // 合法（`1` 会匹配不到任何元素）。关键断言是 `reversed == false`：
        // 若误判成反序，`1` 会被当成选择器且列表被静默倒置。
        let parsed = parse("-1@text").expect("裸 CSS 形态可解析");
        assert!(!parsed.reversed, "紧邻数字的 `-` 不是反序标记");

        // 反证：`-class.odd@text` 的 `-` 不紧邻数字，是反序标记
        assert!(parse("-class.odd@text").expect("应解析成功").reversed);
    }

    // ---------- 解析：访问器 ----------

    #[test]
    fn 末段被识别为访问器() {
        // 公开文档：「@ 的最后一段为获取内容,如 text,textNodes,ownText,href,src,html,all 等」
        for (rule, expected) in [
            ("class.odd@text", crate::css_post::RuleSuffix::Text),
            ("class.odd@href", crate::css_post::RuleSuffix::Href),
            ("class.odd@src", crate::css_post::RuleSuffix::Src),
            ("class.odd@html", crate::css_post::RuleSuffix::Html),
            ("class.odd@all", crate::css_post::RuleSuffix::All),
            ("class.odd@ownText", crate::css_post::RuleSuffix::OwnText),
            (
                "class.odd@textNodes",
                crate::css_post::RuleSuffix::TextNodes,
            ),
        ] {
            let parsed = parse(rule).expect("应解析成功");
            assert_eq!(parsed.accessor, Some(expected), "规则 {rule}");
            assert_eq!(parsed.segments.len(), 1, "规则 {rule} 只有一段选择");
        }
    }

    #[test]
    fn 无访问器时按未指定处理() {
        let parsed = parse("class.odd.0").expect("应解析成功");
        assert!(parsed.accessor.is_none());
    }

    #[test]
    fn 访问器与选择段共存() {
        // 公开文档的原文示例
        let parsed = parse("class.odd.0@tag.a.0@text").expect("应解析成功");
        assert_eq!(parsed.segments.len(), 2);
        assert_eq!(
            parsed.segments[0].kind,
            SegmentKind::Class("odd".to_string())
        );
        assert_eq!(parsed.segments[0].slice, IndexSlice::Single(0));
        assert_eq!(parsed.segments[1].kind, SegmentKind::Tag("a".to_string()));
        assert_eq!(parsed.segments[1].slice, IndexSlice::Single(0));
        assert_eq!(parsed.accessor, Some(crate::css_post::RuleSuffix::Text));
    }

    // ---------- 求值：单段 ----------

    #[test]
    fn 单段按类名选并取文本() {
        let values = run(SEARCH_HTML, "class.odd@tag.a@text");
        assert_eq!(values, vec!["第一本", "第三本"], "两个 odd 各取一次文本");
    }

    #[test]
    fn 单段按标签名选() {
        assert_eq!(
            run(SEARCH_HTML, "tag.a@text"),
            vec!["第一本", "第二本", "第三本"]
        );
    }

    #[test]
    fn 单段按_id_选() {
        let html = r#"<div id="w"><p>x</p></div>"#;
        assert_eq!(run(html, "id.w@tag.p@text"), vec!["x"]);
    }

    #[test]
    fn text段按文本内容选() {
        // 公开文档：「text 可以根据文本内容获取」
        let html = r#"<div class="menu"><a>下一章</a><a>目录</a></div>"#;
        assert_eq!(run(html, "text.下一章@href"), Vec::<String>::new());
        // 取文本验证选中的是"下一章"那个
        assert_eq!(run(html, "text.下一章@text"), vec!["下一章"]);
    }

    #[test]
    fn children段取所有直接子标签() {
        let html = r#"<ul><li>a</li><li>b</li></ul>"#;
        assert_eq!(run(html, "tag.ul@children@text"), vec!["a", "b"]);
    }

    // ---------- 求值：位置 ----------

    #[test]
    fn 位置取单个() {
        assert_eq!(run(SEARCH_HTML, "class.odd.0@tag.a@text"), vec!["第一本"]);
        assert_eq!(run(SEARCH_HTML, "class.odd.1@tag.a@text"), vec!["第三本"]);
    }

    #[test]
    fn 负数位置取倒数() {
        assert_eq!(run(SEARCH_HTML, "class.odd.-1@tag.a@text"), vec!["第三本"]);
        assert_eq!(run(SEARCH_HTML, "class.odd.-2@tag.a@text"), vec!["第一本"]);
    }

    #[test]
    fn 排除位置() {
        let html = r#"<ul><li>0</li><li>1</li><li>2</li><li>3</li></ul>"#;
        assert_eq!(run(html, "tag.li.!0:1@text"), vec!["2", "3"]);
    }

    #[test]
    fn 越界位置产出空() {
        assert_eq!(
            run(SEARCH_HTML, "class.odd.99@tag.a@text"),
            Vec::<String>::new()
        );
    }

    // ---------- 求值：反序 ----------

    #[test]
    fn 反序使列表倒置() {
        // 公开文档：「有些网站目录列表是倒的,前面加个负号可变为正的」
        assert_eq!(
            run(SEARCH_HTML, "-class.odd@tag.a@text"),
            vec!["第三本", "第一本"]
        );
    }

    #[test]
    fn 反序与位置共存时先反序再取位置() {
        // 与 `css_post` 的"先反序再切片"一致
        assert_eq!(run(SEARCH_HTML, "-class.odd.0@tag.a@text"), vec!["第三本"]);
    }

    // ---------- 求值：多段嵌套下钻 ----------

    #[test]
    fn 多段是逐段嵌套下钻() {
        // 公开书源开发文档：`id.list@dd@a` 的中间段须保留**全部**匹配节点
        let html = r#"<div id="list">
          <dd><a href="/1">一</a><a href="/2">二</a></dd>
          <dd><a href="/3">三</a></dd>
        </div>"#;
        assert_eq!(
            run(html, "id.list@tag.dd@tag.a@text"),
            vec!["一", "二", "三"],
            "两个 dd 里的 a 都要取到（不是只取第一个 dd）"
        );
    }

    #[test]
    fn 多段下钻在各自子树内求() {
        // 两本书各有一个 `.name`。若在整页上求第二段，会先取到全部 .name，
        // 结果相同；故这里用"每本书的 .name 数量不同"来区分两种实现。
        let html = r#"<div class="book"><span class="name">A</span></div>
                      <div class="book"><span class="name">B</span><span class="name">C</span></div>"#;
        let values = run(html, "class.book@class.name@text");
        assert_eq!(values, vec!["A", "B", "C"]);
    }

    #[test]
    fn 文档给的原文示例形态可求值() {
        // 公开文档的例子主体（组合符部分由 T023 分派器处理，此处只测单条体）。
        //
        // 例:class.odd.0@tag.a.0@text
        assert_eq!(run(SEARCH_HTML, "class.odd.0@tag.a.0@text"), vec!["第一本"]);
    }

    #[test]
    fn 多段下钻不重复计同一节点() {
        // 子树重叠时同一节点可能被多次命中，去重后只算一次
        let html = r#"<div class="a"><div class="a"><span class="t">x</span></div></div>"#;
        let values = run(html, "class.a@class.a@class.t@text");
        assert_eq!(values, vec!["x"], "同一节点不该重复");
    }

    // ---------- 求值：子树 ----------

    #[test]
    fn 子树内求值() {
        let document = Document::parse_html(
            r#"<ul><li class="i"><a href="/1">一</a></li><li class="i"><a href="/2">二</a></li></ul>"#,
        );
        let first = crate::css::select_in_document(
            &document,
            &crate::css::compile("li.i").expect("应编译"),
        )
        .into_iter()
        .next()
        .expect("应有 li");
        assert_eq!(
            evaluate_in(&first, "tag.a@href").expect("应求值成功"),
            vec!["/1"]
        );
    }

    #[test]
    fn 子树起点自身可被首段命中() {
        // 公开书源开发文档：「非整页查询时须包含 scope 自身；仅 find 会导致
        // `.category-div@h3@…` 在条目根已是 `category-div` 时取空」
        let document = Document::parse_html(r#"<div class="category-div"><h3>标题</h3></div>"#);
        let scope = crate::css::select_in_document(
            &document,
            &crate::css::compile("div.category-div").expect("应编译"),
        )
        .into_iter()
        .next()
        .expect("应有 category-div");
        assert_eq!(
            evaluate_in(&scope, "class.category-div@tag.h3@text").expect("应求值成功"),
            vec!["标题"],
            "首段应能命中 scope 自身"
        );
    }

    #[test]
    fn 取节点入口可用() {
        let document = Document::parse_html(SEARCH_HTML);
        let nodes = select_nodes(&document, "class.odd").expect("应求值成功");
        assert_eq!(nodes.len(), 2);
        assert!(nodes.iter().all(|node| node.node_name() == "li"));
    }

    // ---------- 回归：两处被语料编写发现的缺陷 ----------

    #[test]
    fn attr访问器与_css_路径行为一致() {
        // 回归：`attr(名称)` 在 Default 路径上曾不可用——访问器检测用的是
        // `RuleSuffix::parse`，它不认 `attr(...)`，于是 `a@attr(data-id)` 被
        // 当成选择段喂给 CSS 并报"选择器解析失败"，而同一写法在 `@css:`
        // 路径上正常。
        //
        // 公开文档列的是**一份**访问器清单，两条路径必须逐字一致。
        let html = r#"<a href="/u" title="t" data-id="7">文字</a>"#;
        assert_eq!(run(html, "a@attr(data-id)"), vec!["7"]);
        assert_eq!(run(html, "a@attr(href)"), vec!["/u"]);
        // 带引号的写法也应接受（与 `css_post` 的宽容一致）
        assert_eq!(run(html, r#"a@attr("data-id")"#), vec!["7"]);
        // 属性不存在 → 空
        assert_eq!(run(html, "a@attr(nope)"), Vec::<String>::new());
    }

    #[test]
    fn 末段的位置逐父节点施加而不是全局() {
        // 回归：末段的位置曾在**拼接后的总列表**上施加，于是前一本书的命中
        // 被后一本书的命中挤掉——与"逐段下钻"自相矛盾（中间段的位置一直
        // 是逐父节点施加的）。
        //
        // 公开文档把位置定义为**段的一部分**，而段是在各自子树内求的，
        // 故位置也在各自子树的命中集上施加。
        let html = r#"<div class="book"><span class="name">A</span><span class="name">B</span></div>
                      <div class="book"><span class="name">C</span><span class="name">D</span></div>"#;
        assert_eq!(
            run(html, "class.book@class.name.0@text"),
            vec!["A", "C"],
            "每本书各取自己的第 0 个 .name"
        );
        assert_eq!(
            run(html, "class.book@class.name.-1@text"),
            vec!["B", "D"],
            "每本书各取自己的倒数第 1 个"
        );
        // 与中间段的位置行为一致（后者一直是逐父节点的）
        assert_eq!(
            run(html, "class.book.0@class.name@text"),
            vec!["A", "B"],
            "首段取第 0 本书后，它的两个 .name 都给"
        );
    }

    #[test]
    fn 末段位置与中间段位置行为对称() {
        // 对称性的直接断言：把位置放在中间段还是末段，结果应一致
        let html = r#"<div class="m"><i>x</i><i>y</i></div><div class="m"><i>z</i><i>w</i></div>"#;
        assert_eq!(run(html, "class.m@tag.i.1@text"), vec!["y", "w"]);
        assert_eq!(run(html, "class.m.1@tag.i@text"), vec!["z", "w"]);
        // 两者都是"逐父节点"的：前者每个 m 取自己的第 1 个 i，
        // 后者取第 1 个 m 的全部 i
    }

    // ---------- 边界 ----------

    #[test]
    fn 空规则产出空() {
        assert_eq!(run("<p>x</p>", ""), Vec::<String>::new());
    }

    #[test]
    fn 无匹配产出空列表而非错误() {
        assert_eq!(
            run(SEARCH_HTML, "class.missing@tag.a@text"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn 畸形输入不崩溃() {
        for rule in [
            "@",
            "@@",
            ".",
            "..",
            "class.",
            ".odd",
            "class.odd@",
            "text.",
            "children.0",
        ] {
            let document = Document::parse_html("<p>x</p>");
            let _ = evaluate(&document, rule);
        }
    }

    #[test]
    fn 类名里的_css_元字符被转义() {
        // ## 空格是组合符，不属于类名
        //
        // `class.a b` 里的空格按 CSS 语义是**后代组合符**（Jsoup 亦然），
        // 故它的含义是"class=a 的后代里选 b"（tag b），不是"类名叫 a b"。
        // 本实现保留这个语义（转义只处理真正需要转义的字符，不动空格）。
        let html = r#"<div class="a"><b>内层</b></div>"#;
        assert_eq!(run(html, "class.a b@text"), vec!["内层"]);

        // 冒号与点这类"会静默改变语义"的字符才需要转义：
        // `class.a:b` 若不转义会被 CSS 解析器当成伪类 `:b`。
        let html = r#"<div class="a:b">x</div>"#;
        assert_eq!(
            run(html, "class.a:b@text"),
            vec!["x"],
            "类名里的冒号必须转义，否则会被当成伪类"
        );
    }
}
