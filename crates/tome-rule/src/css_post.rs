//! CSS 后缀与索引切片（tasks.md T022）
//!
//! # 依据来源
//!
//! 本节所有语法取自**书源规则的公开语法约定**（社区流通的《书源规则说明》文档，
//! 属公开资料，非某实现的源码）。原文要点抄录如下，作为实现的直接依据：
//!
//! ## 后缀（原文：「@的最后一段为获取内容」）
//!
//! > 如text,textNodes,ownText,href,src,html,all等
//!
//! 故 `href` / `src` 是**一等后缀**而非 `@attr(href)` 的语法糖——两者等价，
//! 但前者在书源里更常见，必须直接支持。
//!
//! ## 索引（原文：「第三段是位置」）
//!
//! > 位置正着数从0开始,0是第一个,如为负数则是取倒着数的值,-1为倒数第一个,-2为倒数第二个
//! > !是排除,有些位置不符合需要排除用!,后面的序号用:隔开0是第1个,负数为倒数序号,-1最后一个
//! > 获取列表的最前面加上负号- 可以使列表倒置,有些网站目录列表是倒的,前面加个负号可变为正的
//!
//! ## 索引的出现形态有两种
//!
//! 原文的 Default（jsoup）语法把索引写成第三段（用 `.` 分隔，如
//! `class.odd.0@tag.a.0@text`）；而 CSS 规则把索引写成方括号附在选择器后
//! （如 `@css:.name>a@href` 取全部、`@css:p:eq(2)>a@text` 用伪类定位）。
//! 本模块**两种形态都接受**：[`IndexSlice::parse_bracket`] 处理 `[0]` / `[0:3]`
//! / `[!0:3]`，[`IndexSlice::parse_dot`] 处理 `.0` / `.-1`。语义完全一致。
//!
//! # 与 Jsoup 的关系
//!
//! 后缀里的 `text` / `ownText` / `html` / `all` 都直接落在 [`crate::dom`] 已实现的
//! Jsoup 语义上（`text()` 空白归一化、`ownText()` 只看直接子级、`html()` 是内部
//! HTML 序列化）。故这里只做"取哪个"的分派，语义不在本模块重复实现。

use crate::dom::{Document, NodeRef};

/// 规则后缀：取节点的哪一部分。
///
/// 依据见模块文档引用的公开规则说明（「如text,textNodes,ownText,href,src,html,all等」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleSuffix {
    /// 元素及后代的文本，空白归一化（Jsoup `text()`）
    Text,
    /// **直接子级文本节点**的列表（Jsoup `textNodes()`）。与 `text` 的区别是
    /// 它返回**多个**值而不是拼接成一个。
    TextNodes,
    /// 直接子级的文本，空白归一化（Jsoup `ownText()`）
    OwnText,
    /// 内部 HTML（Jsoup `html()`）
    Html,
    /// 元素的外部 HTML（Jsoup `outerHtml()`）
    OuterHtml,
    /// 全部：对元素是外部 HTML，对文本节点是文本值。
    ///
    /// 公开文档把它列在"获取内容"清单里，且书源用它的场合多为"拿到这段原始内容
    /// 再交给后续替换链处理"。
    All,
    /// 属性值（`@attr(href)` / `@attr(src)` 等任意属性名）
    Attr(String),
    /// 节点的值（Jsoup `Node#nodeValue()`）。文本/注释/Data 节点用。
    NodeValue,
    /// 取原始 HTML 串（等价于 `All` 对元素的行为）；`html` 与 `all` 在文本节点上
    /// 行为不同，故保留独立变体以便分别处理。
    Href,
    /// `src` 后缀——等价于 `@attr(src)`，但作为一等后缀存在（见模块文档）
    Src,
    /// 未指定后缀：默认行为。
    ///
    /// 默认行为的定义在公开文档里没有单独一句话，但从示例可推断：
    /// `"name": "@css:.name@text"` 显式写了 `text`，而 `"chapterName": "text"` 是
    /// Default 语法的简写。故"未指定"时按 `Text` 处理是安全的选择——这也是
    /// 唯一能让"只给选择器不给后缀"的规则产出有意义结果的解释。
    Unspecified,
}

impl RuleSuffix {
    /// 按后缀名解析。名字大小写敏感（书源里一律小写）。
    ///
    /// 返回 `None` 表示不是已知后缀——调用方应把整段当属性名还是报错，
    /// 由更上层决定（本模块不做"猜"）。
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "text" => Some(Self::Text),
            "textNodes" => Some(Self::TextNodes),
            "ownText" => Some(Self::OwnText),
            "html" => Some(Self::Html),
            "outerHtml" => Some(Self::OuterHtml),
            "all" => Some(Self::All),
            "nodeValue" => Some(Self::NodeValue),
            "href" => Some(Self::Href),
            "src" => Some(Self::Src),
            "" => Some(Self::Unspecified),
            _ => None,
        }
    }

    /// 规范名（诊断用）。
    pub fn canonical_name(&self) -> &str {
        match self {
            Self::Text => "text",
            Self::TextNodes => "textNodes",
            Self::OwnText => "ownText",
            Self::Html => "html",
            Self::OuterHtml => "outerHtml",
            Self::All => "all",
            Self::Attr(name) => name,
            Self::NodeValue => "nodeValue",
            Self::Href => "href",
            Self::Src => "src",
            Self::Unspecified => "",
        }
    }

    /// 该后缀是否产出**多个**值（调用方据此决定是拼接还是保留列表）。
    ///
    /// 只有 `textNodes` 是多值：公开文档把它与 `text` 并列列出，二者的实质区别
    /// 正是"接成一段"与"逐节点给"。
    pub fn is_multi_valued(&self) -> bool {
        matches!(self, Self::TextNodes)
    }

    /// 对单个节点求值。
    ///
    /// [DONE] 后缀求值的唯一入口。`document` 参数用于 `Html`/`OuterHtml` 的序列化。
    pub fn evaluate(&self, node: &NodeRef) -> Vec<String> {
        match self {
            Self::Text => vec![node.text()],
            Self::TextNodes => collect_text_nodes(node),
            Self::OwnText => vec![node.own_text()],
            Self::Html => vec![inner_html(node)],
            Self::OuterHtml | Self::All => vec![outer_html(node)],
            Self::Attr(name) => node.attr(name).into_iter().collect(),
            Self::Href => node.attr("href").into_iter().collect(),
            Self::Src => node.attr("src").into_iter().collect(),
            Self::NodeValue => vec![node.node_value()],
            // 未指定后缀按 `Text` 处理（理由见变体文档）
            Self::Unspecified => vec![node.text()],
        }
    }
}

/// 取元素的直接子**文本节点**的内容（Jsoup `textNodes()`）。
///
/// 依据 Jsoup 公开文档 `Element#textNodes` 条目给出的确切例子：
///
/// > with the input HTML `<p>One <span>Two</span> Three <br> Four</p>`, and the `p`
/// > element selected: `p.textNodes()` = `List<TextNode>["One ", " Three ", " Four"]`
fn collect_text_nodes(node: &NodeRef) -> Vec<String> {
    node.children()
        .filter(|child| child.kind() == crate::dom::NodeKind::Text)
        .map(|child| child.node_value())
        .collect()
}

/// 元素的**内部** HTML（Jsoup `html()`）。
///
/// 依据 Jsoup 公开文档：*"on a `<div>` with one empty `<p>`, this returns `<p></p>`,
/// whereas `Node.outerHtml()` returns `<div><p></p></div>`"*。
fn inner_html(node: &NodeRef) -> String {
    let mut buffer = String::new();
    for child in node.children() {
        serialize_node(&child, &mut buffer);
    }
    buffer
}

/// 元素及其子树的 HTML（Jsoup `outerHtml()`）。
fn outer_html(node: &NodeRef) -> String {
    let mut buffer = String::new();
    serialize_node(node, &mut buffer);
    buffer
}

/// 把一个节点序列化回 HTML 文本。
///
/// 供 [`crate::xpath::eval`] 的扩展节点测试（`html()` / `outerHtml()`）复用
/// ——同一件事不该在两个引擎里各写一份，否则 `@html` 后缀与 `html()`
/// 节点测试的输出会分叉。
///
/// ## 这是本项目自带的序列化，不是"对齐 Jsoup 的输出格式"
///
/// 书源规则里的 `@html` 多数只用来"拿到这段内容再交给替换链/正则处理"，
/// 故本实现的取舍是：**结构保真、字符转义与自闭合形式按 HTML5 规范**，
/// 不追求与 Jsoup 逐字节相同（那需要对照数据，属 S1-6 真实书源基线的范畴）。
///
/// [UNCERTAIN] 属性引号风格、自闭合标签写法（`<br>` vs `<br/>`）、实体转义
/// 范围这三处与 Jsoup 的差异未逐项对照。S1-6 的差分基线会给出实测差异率。
///
/// ## 为什么用显式栈而不是递归
///
/// DOM 深度由外部 HTML 决定，而书源抓到的畸形页面深度可达数千。递归版本
/// 在 3000 层嵌套上**实测栈溢出**（`STATUS_STACK_OVERFLOW`，进程直接死），
/// 而解析层与遍历层都刻意用了显式栈（`dom::PreOrderIterator`）——序列化层
/// 漏了这一条。AGENTS.md 的生产级护栏要求引擎不得因输入崩溃。
///
/// 显式栈的元素是"待处理项"：要么是一个待输出的节点，要么是一段待输出的
/// 字面文本（元素开始/结束标签、doctype 等）。用后进先出保证文档序输出：
/// 压栈时**逆序**压入子节点。
pub(crate) fn serialize_node(node: &NodeRef, buffer: &mut String) {
    let mut stack: Vec<SerializeTask> = vec![SerializeTask::Node(node.clone())];

    while let Some(task) = stack.pop() {
        match task {
            SerializeTask::Literal(text) => buffer.push_str(&text),
            SerializeTask::Node(current) => {
                serialize_one(&current, buffer, &mut stack);
            }
        }
    }
}

/// 输出单个节点的"自身部分"，并把其子节点以**逆序**压栈（保证文档序弹出）。
fn serialize_one(node: &NodeRef, buffer: &mut String, stack: &mut Vec<SerializeTask>) {
    use crate::dom::NodeKind;

    match node.kind() {
        // 文档/片段节点自身无输出，只需把子节点逆序压栈
        NodeKind::Document => push_children_reversed(node, stack),
        NodeKind::Doctype => {
            let name = node.node_value();
            buffer.push_str("<!DOCTYPE ");
            buffer.push_str(&name);
            if let Some((public_id, system_id)) = node.doctype_ids() {
                if !public_id.is_empty() {
                    buffer.push_str(" PUBLIC \"");
                    buffer.push_str(&public_id);
                    buffer.push('"');
                }
                if !system_id.is_empty() {
                    buffer.push_str(" \"");
                    buffer.push_str(&system_id);
                    buffer.push('"');
                }
            }
            buffer.push('>');
        }
        NodeKind::Comment => {
            buffer.push_str("<!--");
            buffer.push_str(&node.node_value());
            buffer.push_str("-->");
        }
        NodeKind::ProcessingInstruction => {
            buffer.push_str("<?");
            buffer.push_str(&node.processing_instruction_target().unwrap_or_default());
            buffer.push(' ');
            buffer.push_str(&node.node_value());
            buffer.push('>');
        }
        // Data 与 CData 的内容按原样输出（rawtext 元素的正文不该被转义）
        NodeKind::Data | NodeKind::CData => buffer.push_str(&node.node_value()),
        NodeKind::Text => escape_text(&node.node_value(), buffer),
        NodeKind::Element => serialize_element(node, buffer, stack),
    }
}

/// 序列化任务的显式栈元素。
enum SerializeTask {
    /// 待输出的节点
    Node(NodeRef),
    /// 待输出的字面文本
    Literal(String),
}

/// 把子节点以逆序压栈（配合 `Vec::pop` 得到文档序）。
fn push_children_reversed(node: &NodeRef, stack: &mut Vec<SerializeTask>) {
    let children: Vec<NodeRef> = node.children().collect();
    for child in children.into_iter().rev() {
        stack.push(SerializeTask::Node(child));
    }
}

fn serialize_element(node: &NodeRef, buffer: &mut String, stack: &mut Vec<SerializeTask>) {
    buffer.push('<');
    buffer.push_str(&node.qualified_name());
    for (name, value) in node.attributes() {
        buffer.push(' ');
        buffer.push_str(&name);
        buffer.push_str("=\"");
        escape_attribute(&value, buffer);
        buffer.push('"');
    }
    buffer.push('>');

    // 空元素（void element）不写结束标签。清单取自 WHATWG 规范 "void elements"。
    if is_void_element(&node.node_name()) {
        push_children_reversed(node, stack);
        return;
    }

    // 结束标签必须在全部子节点**之后**输出，故先压它（后进先出 → 最后弹出）
    let mut closing = String::from("</");
    closing.push_str(&node.qualified_name());
    closing.push('>');
    stack.push(SerializeTask::Literal(closing));
    push_children_reversed(node, stack);
}

/// WHATWG 规范定义的 void elements（不自闭合、无结束标签）。
fn is_void_element(local_name: &str) -> bool {
    matches!(
        local_name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

/// 文本节点的转义：`&`、`<`、`>`（HTML5 规范 "escaping a string" 的最小子集）。
fn escape_text(text: &str, buffer: &mut String) {
    for character in text.chars() {
        match character {
            '&' => buffer.push_str("&amp;"),
            '<' => buffer.push_str("&lt;"),
            '>' => buffer.push_str("&gt;"),
            _ => buffer.push(character),
        }
    }
}

/// 属性值的转义：另加 `"`（本实现统一用双引号包裹）。
fn escape_attribute(value: &str, buffer: &mut String) {
    for character in value.chars() {
        match character {
            '&' => buffer.push_str("&amp;"),
            '"' => buffer.push_str("&quot;"),
            _ => buffer.push(character),
        }
    }
}

// ---------------------------------------------------------------------------
// 索引切片
// ---------------------------------------------------------------------------

/// 索引切片。
///
/// 依据见模块文档引用的公开规则说明（「第三段是位置」整段）。所有负索引都是
/// **从末尾倒数**（-1 是最后一个），所有区间端点都是**闭区间**（原文的
/// 「0是第1个,负数为倒数序号,-1最后一个」描述的是端点语义，不是长度）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IndexSlice {
    /// 不切片：取全部。
    #[default]
    All,
    /// 单个索引：正数从 0 起，负数从末尾倒数（-1 是最后一个）。
    Single(i64),
    /// 区间 `[start:end]`，两端都是**闭区间**。
    Range {
        /// 起点（含）
        start: i64,
        /// 终点（含）。`None` 表示到末尾。
        end: Option<i64>,
    },
    /// 排除区间 `[!start:end]`（原文：「!是排除」）。
    Exclude {
        /// 起点（含）
        start: i64,
        /// 终点（含）。`None` 表示到末尾。
        end: Option<i64>,
    },
}

impl IndexSlice {
    /// 解析方括号形态：`[0]` / `[0:3]` / `[!0:3]` / `[-1]` / `[2:]`。
    ///
    /// 返回 `None` 表示不是合法的切片（调用方据此把它当别的语法处理）。
    pub fn parse_bracket(text: &str) -> Option<Self> {
        let inner = text.strip_prefix('[')?.strip_suffix(']')?.trim();
        if inner.is_empty() {
            return None;
        }

        // `!` 前缀标记排除
        let (excluded, body) = match inner.strip_prefix('!') {
            Some(rest) => (true, rest.trim()),
            None => (false, inner),
        };

        if let Some((start_text, end_text)) = body.split_once(':') {
            let start = parse_signed(start_text.trim())?;
            let end = match end_text.trim() {
                "" => None,
                other => Some(parse_signed(other)?),
            };
            return Some(if excluded {
                Self::Exclude { start, end }
            } else {
                Self::Range { start, end }
            });
        }

        // 排除形态必须有区间，单独的 `[!3]` 无意义
        if excluded {
            return None;
        }
        parse_signed(body).map(Self::Single)
    }

    /// 解析点号形态：`.0` / `.-1`（Default 语法的第三段）。
    ///
    /// 注意 `.` 后必须紧跟数字（可带负号），否则不是索引——例如
    /// `class.odd.0` 里的 `class` 与 `odd` 是类型与名称，只有 `.0` 是索引。
    pub fn parse_dot(text: &str) -> Option<Self> {
        let body = text.strip_prefix('.')?;
        // `.0` 之外的内容（如 `.-1`）都交给同一个带符号解析
        parse_signed(body).map(Self::Single)
    }

    /// 是否"取全部"（无需切片）。
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// 把切片施加到一个列表上。
    ///
    /// [DONE] 索引语义的唯一落点。
    ///
    /// ## 端点归一化
    ///
    /// 负端点在**施加时**才换算成绝对位置（因为要列表长度）。换算规则按原文
    /// 「-1为倒数第一个,-2为倒数第二个」：`index < 0` → `len + index`。
    ///
    /// ## 越界处理
    ///
    /// 越界端点被**夹到合法范围**而不是报错。理由：书源规则里的索引常按
    /// "推测的列表长度"书写（例如固定写 `[0:9]` 取前十条），实际不足十条时
    /// 报错会让整条搜索失败，而"取到几条算几条"是书源作者的预期。
    pub fn apply(&self, values: Vec<String>) -> Vec<String> {
        match self {
            Self::All => values,
            Self::Single(index) => {
                let position = normalize(*index, values.len());
                match position {
                    Some(position) => vec![values[position].clone()],
                    None => Vec::new(),
                }
            }
            Self::Range { start, end } => {
                let (start, end) = range_bounds(*start, *end, values.len());
                match (start, end) {
                    (Some(start), Some(end)) if start <= end => values[start..=end].to_vec(),
                    _ => Vec::new(),
                }
            }
            Self::Exclude { start, end } => {
                let (start, end) = range_bounds(*start, *end, values.len());
                match (start, end) {
                    (Some(start), Some(end)) => values
                        .into_iter()
                        .enumerate()
                        .filter(|(position, _)| *position < start || *position > end)
                        .map(|(_, value)| value)
                        .collect(),
                    _ => values,
                }
            }
        }
    }

    /// 把切片施加到一个**节点**列表上（选择器结果用）。
    ///
    /// 与 [`Self::apply`] 分开是为了避免"先把节点转成字符串再切"——那会丢掉
    /// 元素身份，使后续的 `@attr(x)` 后缀无从取值。
    pub fn apply_nodes(&self, values: Vec<NodeRef>) -> Vec<NodeRef> {
        match self {
            Self::All => values,
            Self::Single(index) => match normalize(*index, values.len()) {
                Some(position) => vec![values[position].clone()],
                None => Vec::new(),
            },
            Self::Range { start, end } => {
                let (start, end) = range_bounds(*start, *end, values.len());
                match (start, end) {
                    (Some(start), Some(end)) if start <= end => values[start..=end].to_vec(),
                    _ => Vec::new(),
                }
            }
            Self::Exclude { start, end } => {
                let (start, end) = range_bounds(*start, *end, values.len());
                match (start, end) {
                    (Some(start), Some(end)) => values
                        .into_iter()
                        .enumerate()
                        .filter(|(position, _)| *position < start || *position > end)
                        .map(|(_, value)| value)
                        .collect(),
                    _ => values,
                }
            }
        }
    }
}

/// 解析带符号整数（允许前导 `-`）。
fn parse_signed(text: &str) -> Option<i64> {
    if text.is_empty() {
        return None;
    }
    // 只接受纯数字与可选前导负号——`+3` 在书源里不出现，且 CSS 选择器里的
    // `+` 是组合子，误吞会破坏解析。
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if digits.is_empty() || !digits.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let magnitude: i64 = digits.parse().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// 把索引归一化到 `[0, len)`。越界返回 `None`。
fn normalize(index: i64, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let resolved = if index < 0 { len as i64 + index } else { index };
    if resolved < 0 || resolved >= len as i64 {
        return None;
    }
    Some(resolved as usize)
}

/// 把区间端点归一化并夹到合法范围。返回 `(起点, 终点)`，任一为 `None` 表示空结果。
fn range_bounds(start: i64, end: Option<i64>, len: usize) -> (Option<usize>, Option<usize>) {
    if len == 0 {
        return (None, None);
    }
    let last = len as i64 - 1;
    let resolve = |index: i64| -> i64 {
        if index < 0 {
            len as i64 + index
        } else {
            index
        }
    };

    let start_resolved = resolve(start).max(0);
    let end_resolved = match end {
        Some(end) => resolve(end).min(last),
        None => last,
    };

    if start_resolved > last || end_resolved < 0 {
        return (None, None);
    }
    (
        Some(start_resolved.min(last) as usize),
        Some(end_resolved.max(0) as usize),
    )
}

// ---------------------------------------------------------------------------
// 规则尾部解析（后缀 + 切片 + 反序）
// ---------------------------------------------------------------------------

/// 一条 CSS 规则被拆出的尾部成分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleTail {
    /// 选择器（不含尾部）
    pub selector: String,
    /// 后缀
    pub suffix: RuleSuffix,
    /// 索引切片
    pub slice: IndexSlice,
    /// 是否反序（原文：「获取列表的最前面加上负号- 可以使列表倒置」）
    pub reversed: bool,
}

/// 解析 `@css:` 规则的选择器部分，拆出选择器、后缀、索引切片与反序标记。
///
/// # 接受的全部形态
///
/// | 输入 | 选择器 | 后缀 | 切片 | 反序 |
/// |---|---|---|---|---|
/// | `.name>a@href` | `.name>a` | `href` | all | 否 |
/// | `.name>a[0]@text` | `.name>a` | `text` | `[0]` | 否 |
/// | `.name>a[0:3]` | `.name>a` | 未指定 | `[0:3]` | 否 |
/// | `.list.-1@text` | `.list` | `text` | `.-1` | 否 |
/// | `-.list@text` | `.list` | `text` | all | 是 |
/// | `.list@attr(data-id)` | `.list` | 属性 | all | 否 |
///
/// ## 解析顺序为什么是「反序 → 切片 → 后缀」
///
/// 三者的语法标记互不重叠，但**切片在后缀之前**这一点是必须的：后缀名
/// （`text` 等）不含 `[`，而切片总是带方括号；若先剥后缀，遇到
/// `[0:3]@text` 这种写法会先剥掉 `text` 再处理切片，结果一样。真正需要顺序的是
/// 反序标记（最前面的 `-`）——它必须在切片之后判断，否则 `.list.-1` 里的 `-`
/// 会被误当反序标记。
pub fn parse_rule_tail(rule: &str) -> RuleTail {
    let mut remaining = rule.trim();
    let mut reversed = false;

    // 1) 反序标记：列表最前面的 `-`。只在它**不紧邻数字**时才算反序标记，
    //    否则会把负索引（`.list.-1`）的开头误判——不过 `.list.-1` 里的 `-`
    //    前面是 `.` 不是串首，故这里只需排除"串首的 `-` 后紧跟数字"这一种情况
    //    （那种写法不是合法的书源规则，但明确排除比默默吞掉好）。
    if let Some(rest) = remaining.strip_prefix('-') {
        if !rest.starts_with(|character: char| character.is_ascii_digit()) {
            reversed = true;
            remaining = rest.trim();
        }
    }

    // 2) 后缀：最后一个未被方括号包裹的 `@`
    let (body, suffix) = split_suffix(remaining);
    remaining = body;

    // 3) 切片：尾部的方括号，或尾部点号形态的索引
    let (selector, slice) = split_slice(remaining);

    RuleTail {
        selector: selector.trim().to_string(),
        suffix,
        slice,
        reversed,
    }
}

/// 从尾部切出后缀。返回 (剩余部分, 后缀)。
fn split_suffix(text: &str) -> (&str, RuleSuffix) {
    // 从右往左找第一个不在方括号内的 `@`
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut position = bytes.len();
    while position > 0 {
        position -= 1;
        match bytes[position] {
            b']' => depth += 1,
            b'[' => depth -= 1,
            // `@` 也可能出现在 `@attr(...)` 的实参里，但那种情况括号已由 depth 覆盖
            b'@' if depth == 0 => {
                let suffix_text = &text[position + 1..];
                if let Some(suffix) = parse_suffix_text(suffix_text) {
                    return (&text[..position], suffix);
                }
                // `@` 后的内容不是已知后缀（可能是 `@css:` 之类的前缀残留，
                // 但那种情况调用方已剥掉）——按"无后缀"处理，把整段留给选择器，
                // 让 CSS 解析器报出准确错误。
                return (text, RuleSuffix::Unspecified);
            }
            _ => {}
        }
    }
    (text, RuleSuffix::Unspecified)
}

/// 解析后缀文本，支持 `attr(name)` 形态。
fn parse_suffix_text(text: &str) -> Option<RuleSuffix> {
    let trimmed = text.trim();
    if let Some(argument) = trimmed
        .strip_prefix("attr(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let name = argument
            .trim()
            .trim_matches(|quote| quote == '"' || quote == '\'');
        if name.is_empty() {
            return None;
        }
        return Some(RuleSuffix::Attr(name.to_string()));
    }
    RuleSuffix::parse(trimmed)
}

/// 从尾部切出索引切片。返回 (选择器, 切片)。
fn split_slice(text: &str) -> (&str, IndexSlice) {
    let trimmed = text.trim();

    // 方括号形态：尾部成对的 `[...]`，且其中内容能解析成切片。
    // 必须是**尾部**——CSS 里的 `[href]` 是属性选择器，出现在中间，不该被切掉。
    if trimmed.ends_with(']') {
        if let Some(open) = matching_open_bracket(trimmed) {
            let candidate = &trimmed[open..];
            if let Some(slice) = IndexSlice::parse_bracket(candidate) {
                return (trimmed[..open].trim_end(), slice);
            }
        }
    }

    // 点号形态：末段的 `.N` / `.-N`
    if let Some(position) = trimmed.rfind('.') {
        let candidate = &trimmed[position..];
        if let Some(slice) = IndexSlice::parse_dot(candidate) {
            // 排除小数/类名误判：`.5` 之后不能再有字母数字
            return (trimmed[..position].trim_end(), slice);
        }
    }

    (trimmed, IndexSlice::All)
}

/// 找与尾部 `]` 配对的 `[` 的下标。
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

/// 规则求值的统一入口：**五类语法**都能求值（含可选的 JS 引擎）。
///
/// # 为什么需要一个"引擎"对象而不是自由函数
///
/// 五类规则里有两类需要**外部能力**：
///
/// | 类型 | 需要什么 | 为什么不能自带 |
/// |---|---|---|
/// | `@js:` | JS 沙箱（[`crate::js_bridge::JobRuntime`]） | 建 runtime 有实打实的开销，不该每次求值都建 |
/// | `@json:` | 无（纯计算） | —— |
/// | `@XPath:` | 无（跑同一棵 DOM） | —— |
///
/// 把 JS 运行时挂在对象上（而不是每次调用传参），让调用方的签名保持干净；
/// 而"没注入 JS 时怎么办"有明确定义：报 `unsupported("js")` 结构化错误
/// （FR-006），不是静默失败。
///
/// # 这个类型解决了什么（P4 遗留）
///
/// tasks.md 的「Phase 4 遗留」原文：
///
/// > **字段规则里的 `@js:` 尚未接 JS**：`@js:` 在 `searchUrl` 插值里已可用，
/// > 但五类规则字段（`ruleSearch` 等）里的 `@js:` 仍由 `css_post` 求值、
/// > 报 `unsupported("js")`。接线需要给 `css_post` 一个可选的 JS 引擎参数。
///
/// 本类型就是那个参数。做法与 `parse_search_spec_with_js` 同源：
/// 引擎是 `Option`，`None` 时 JS 类规则报能力错误。
pub struct RuleEngine<'a> {
    /// 文档（CSS 与 XPath 跑在它上面）
    document: &'a Document,
    /// 可选的 JS 运行时（`None` 时 `@js:` 报能力错误）
    js: Option<&'a crate::js_bridge::JobRuntime>,
}

impl<'a> RuleEngine<'a> {
    /// 建一个没有 JS 能力的求值器（纯 CSS/XPath/JSONPath/正则/Default）。
    pub fn new(document: &'a Document) -> Self {
        Self { document, js: None }
    }

    /// 挂上 JS 运行时（`@js:` 类规则因此可用）。
    pub fn with_js(mut self, js: &'a crate::js_bridge::JobRuntime) -> Self {
        self.js = Some(js);
        self
    }

    /// 是否具备 JS 求值能力。
    pub fn has_js(&self) -> bool {
        self.js.is_some()
    }

    /// 求值一条规则（不限定求值起点：文档根）。
    ///
    /// [DONE] 五类规则的统一入口。
    pub fn evaluate(&self, rule: &str) -> Result<Vec<String>, crate::error::RuleError> {
        let (subject, chain) = split_chain(rule);
        let values = self.evaluate_subject(None, &subject)?;
        Ok(apply_chain(values, &chain))
    }

    /// 在**子树内**求值（搜索结果的字段规则用）。
    pub fn evaluate_in(
        &self,
        root: &crate::dom::NodeRef,
        rule: &str,
    ) -> Result<Vec<String>, crate::error::RuleError> {
        let (subject, chain) = split_chain(rule);
        let values = self.evaluate_subject(Some(root), &subject)?;
        Ok(apply_chain(values, &chain))
    }

    /// 求值规则的**主体部分**（不看替换链）。
    ///
    /// `root` 为 `None` 表示从文档根起（等价于 `evaluate_rule_subject`）。
    fn evaluate_subject(
        &self,
        root: Option<&crate::dom::NodeRef>,
        rule: &str,
    ) -> Result<Vec<String>, crate::error::RuleError> {
        use crate::dispatch::{dispatch_prefix, RuleKind};

        // `@webjs:` 必须先判（理由见 `evaluate_rule_subject` 的同款说明）
        if crate::js_bridge::is_webjs_prefix(rule) {
            return Err(crate::js_bridge::webjs_unsupported_error("rule"));
        }

        let dispatched = dispatch_prefix(rule);
        match dispatched.kind {
            RuleKind::Css | RuleKind::Default | RuleKind::ExplicitDefault => {
                let body = if dispatched.reversed {
                    format!("-{}", dispatched.body)
                } else {
                    dispatched.body.clone()
                };
                match root {
                    Some(root) => evaluate_css_body_in(root, &body),
                    None => evaluate_css_body(self.document, &body),
                }
            }
            RuleKind::JsonPath => self.evaluate_json_path(&dispatched.body),
            RuleKind::XPath => self.evaluate_xpath(root, &dispatched.body),
            RuleKind::JavaScript => self.evaluate_javascript(&dispatched.body),
            RuleKind::RegexAllInOne => {
                // 正则 AllInOne 的输入是**上一段的结果**，规则本身不含输入。
                // 单独一条 `:regex` 规则没有可作用的文本，故在此要求调用方
                // 用替换链形态（`主体##regex##替换`）或 `@js:` 组合。
                Err(crate::error::RuleError::syntax(
                    "正则 AllInOne（`:` 开头）不能单独求值——它需要一段被匹配的文本。\
                     请用替换链形态（`主体##正则##替换`）或 `@js:` 组合"
                        .to_string(),
                ))
            }
        }
    }

    /// `@json:` —— 在文档的**文本**上跑 JSONPath。
    ///
    /// # 为什么取"文档文本"而不是"文档元素文本"
    ///
    /// JSON 规则作用的对象是**接口返回的 JSON 文本**，而书源的 JSON 源
    /// 常把整个响应体塞进一个隐藏元素或直接交给 `@js:`。本层的入口是
    /// `&Document`（HTML 语义），故此处取文档的整体文本——那正是
    /// "响应体被当作 HTML 解析后剩下的文本"。
    ///
    /// ## 真正的 JSON 源走别的路
    ///
    /// 接口型书源的响应体**不是 HTML**，调用方应当用
    /// [`crate::jsonpath::evaluate`] 直接喂 JSON 文本，而不是经本方法
    /// （经 HTML 解析会破坏 JSON 里的 `<` 之类字符）。本方法只服务
    /// "JSON 片段嵌在 HTML 里"的少见形态。
    fn evaluate_json_path(&self, expression: &str) -> Result<Vec<String>, crate::error::RuleError> {
        let text = self.document.root().text();
        crate::jsonpath::evaluate(&text, expression)
    }

    /// `@XPath:` —— 跑在与 CSS 同一棵 DOM 上（research R2 的核心决策）。
    fn evaluate_xpath(
        &self,
        root: Option<&crate::dom::NodeRef>,
        expression: &str,
    ) -> Result<Vec<String>, crate::error::RuleError> {
        match root {
            Some(root) => crate::xpath::evaluate_in(root, expression),
            None => crate::xpath::evaluate(self.document, expression),
        }
    }

    /// `@js:` —— 交给 JS 运行时求值。
    ///
    /// # 未注入 JS 时的行为（FR-006）
    ///
    /// 报**结构化能力错误**（`capability = "js"`），不是"未定义"或空结果。
    /// 书源的 `try/catch` 降级逻辑依赖这个标识——它据此知道"这条路走不通
    /// 是因为本产品没有 JS 能力，而不是我的规则写错了"。
    fn evaluate_javascript(&self, fragment: &str) -> Result<Vec<String>, crate::error::RuleError> {
        let Some(runtime) = self.js else {
            return Err(crate::error::RuleError::unsupported(
                "js",
                "规则中的 @js:（本次求值未注入 JS 引擎）",
            ));
        };
        let script = crate::js_bridge::wrap_inline_script(fragment);
        match runtime.eval(&script) {
            Ok(value) => Ok(vec![value.text]),
            Err(error) => Err(crate::js_bridge::SandboxErrorExt::into_rule_error(error)),
        }
    }
}

/// 便捷：把一条规则应用到一组节点上，产出字符串列表。
///
/// 接受完整的书源规则形态（都要支持）：
///
/// - 裸选择器：`div.a@text`
/// - 带 `@css:` 前缀：`@css:div.a@text`（书源规则的常见写法）
/// - **带替换链**：`@css:.name>a@href##^##https://example.com`
///
/// 第三项出自公开文档（「如需要正则替换在最后加上 `##正则表达式##替换内容`」），
/// 且**极其常用**——把相对地址补成绝对地址就是这么写的。故本函数负责把
/// T022（后缀/切片）与 T025（替换链）串起来，调用方不必自己先切 `##`。
///
/// **只接受 CSS 与 Default 两类**——其余类型（JSONPath/XPath/JS）在此产出
/// 结构化「不支持能力」错误（FR-006），而不是把 `$.a` 当 CSS 选择器去解析后
/// 报一个无关的语法错。
///
/// 需要五类全支持（含 `@json:` / `@XPath:` / `@js:`）的调用方用
/// [`RuleEngine`]——它带可选的 JS 运行时，且能跑 JSONPath 与 XPath。
pub fn evaluate_rule(
    document: &Document,
    rule: &str,
) -> Result<Vec<String>, crate::error::RuleError> {
    let (subject, chain) = split_chain(rule);
    let values = evaluate_rule_subject(document, &subject)?;
    Ok(apply_chain(values, &chain))
}

/// 切出替换链：`主体##正则##替换##…` → `(主体, 链)`。
///
/// 复用 [`crate::replace::parse_replace_chain`]——它已实现 `##` 边界的
/// 引号/转义/括号感知（T025），此处不重复一遍切分逻辑。
fn split_chain(rule: &str) -> (String, crate::replace::ReplaceChain) {
    let chain = crate::replace::parse_replace_chain(rule);
    let subject = chain.subject.clone();
    (subject, chain)
}

/// 把替换链作用到规则产出的**每个值**上。
///
/// ## 为什么逐个值而不是拼接后再替换
///
/// 替换的典型用途是"给每个 URL 补前缀"（`##^##https://…`）。拼接后再替换
/// 会让两份结果之间的边界也参与匹配——`^` 锚点会只在整串开头命中，于是只有
/// 第一个 URL 被补上前缀。逐个应用才对。
fn apply_chain(values: Vec<String>, chain: &crate::replace::ReplaceChain) -> Vec<String> {
    if chain.is_empty() {
        return values;
    }
    values
        .into_iter()
        .map(|value| chain.apply(&value, crate::regex::RegexLimits::DEFAULT))
        .collect()
}

/// [`evaluate_rule`] 的**主体部分**（调用方已剥掉替换链）。
pub fn evaluate_rule_subject(
    document: &Document,
    rule: &str,
) -> Result<Vec<String>, crate::error::RuleError> {
    use crate::dispatch::{dispatch_prefix, RuleKind};

    // ## `@webjs:` 必须先判（T043）
    //
    // 它不是 `dispatch_prefix` 认得的类型前缀，故会落到 `Default` 并被当成
    // CSS 选择器去解析——结果是"选择器语法错"，而真实原因是"这段脚本
    // 需要内嵌浏览器"。那个错误信息会让作者去反复检查选择器。
    //
    // 故在此显式拦截并抛**结构化能力错误**（FR-006），让书源的 try/catch
    // 能据此降级到纯 HTTP 路径。
    if crate::js_bridge::is_webjs_prefix(rule) {
        return Err(crate::js_bridge::webjs_unsupported_error("rule"));
    }

    let dispatched = dispatch_prefix(rule);
    match dispatched.kind {
        // 可求值的三类：`dispatched.body` 已剥掉类型前缀（`@css:` / `@@`），
        // 反序标记也已摘出，故正文可以直接交给 CSS 求值。
        RuleKind::Css | RuleKind::Default | RuleKind::ExplicitDefault => {}
        other => {
            return Err(crate::error::RuleError::unsupported(
                other.capability_id(),
                "rule",
            ));
        }
    }

    // 反序标记由 `dispatch_prefix` 摘出，但反序语义在 `parse_rule_tail` 里统一
    // 处理——把 `-` 拼回去而不是在两处各实现一份反序。
    let body = if dispatched.reversed {
        format!("-{}", dispatched.body)
    } else {
        dispatched.body.clone()
    };
    evaluate_css_body(document, &body)
}

fn evaluate_css_body(
    document: &Document,
    rule: &str,
) -> Result<Vec<String>, crate::error::RuleError> {
    let tail = parse_rule_tail(rule);
    let selector = crate::css::compile(&tail.selector)?;
    let mut nodes = crate::css::select_in_document(document, &selector);

    // 反序在**切片之前**：原文说"获取列表的最前面加上负号- 可以使列表倒置"，
    // 而目录列表的典型用法是"整个列表倒过来"，故先整体反序再取位置。
    if tail.reversed {
        nodes.reverse();
    }

    let nodes = tail.slice.apply_nodes(nodes);

    let mut values = Vec::new();
    for node in &nodes {
        values.extend(tail.suffix.evaluate(node));
    }
    Ok(values)
}

// ---------------------------------------------------------------------------
// 子树内的求值（T027 搜索必需）
// ---------------------------------------------------------------------------

/// 在一个**子树**内按选择器取节点（供搜索管线按 `bookList` 切出每本书的子树）。
///
/// ## 与 [`crate::css::select_in_document`] 的区别
///
/// 后者从文档根遍历，前者从 `root` 起遍历**且不越界**（`descendants_and_self`
/// 的上溯边界保证了这一点）。搜索结果的字段选择器（`@css:.name@text`）
/// 是以"每本书"为单位书写的，在整页上求会把所有书的字段混在一起。
///
/// [DONE] T027 的节点级入口。接受带 `@css:` 前缀与裸选择器两种写法
/// （书源里两种都常见，见 `evaluate_rule` 的同款处理）。
pub fn select_nodes(
    document: &Document,
    rule: &str,
) -> Result<Vec<crate::dom::NodeRef>, crate::error::RuleError> {
    use crate::dispatch::{dispatch_prefix, RuleKind};

    // `@webjs:` 必须显式拦截（见 `evaluate_rule_subject` 的同款说明）：
    // 若不拦，它会被当成 CSS 选择器并报"选择器语法错"，
    // 而真实原因是"这段脚本需要内嵌浏览器"。
    if crate::js_bridge::is_webjs_prefix(rule) {
        return Err(crate::js_bridge::webjs_unsupported_error("rule"));
    }

    // 先剥类型前缀——`bookList` 常写作 `@css:li.item`，直接把整串交给
    // CSS 解析器会报 `EmptySelector`（`@css:` 不是合法选择器）。
    let dispatched = dispatch_prefix(rule);
    match dispatched.kind {
        RuleKind::Css | RuleKind::Default | RuleKind::ExplicitDefault => {}
        other => {
            return Err(crate::error::RuleError::unsupported(
                other.capability_id(),
                "rule",
            ));
        }
    }

    let body = dispatched.body.trim();
    if body.is_empty() {
        return Err(crate::error::RuleError::syntax(format!(
            "取节点的规则为空：`{rule}`"
        )));
    }

    let tail = parse_rule_tail(body);
    if tail.reversed || !tail.slice.is_all() || tail.suffix != RuleSuffix::Unspecified {
        // `bookList` 这类"取节点"的规则不应带后缀或切片——带了说明作者写错了，
        // 明确报错比静默忽略后半段好（忽略会让"为什么结果不对"无从排查）。
        return Err(crate::error::RuleError::syntax(format!(
            "取节点的规则不应带后缀或索引切片：`{rule}`"
        )));
    }
    let selector = crate::css::compile(&tail.selector)?;
    Ok(crate::css::select_in_document(document, &selector))
}

/// 在一个**节点子树内**求一条完整规则（选择器 + 切片 + 后缀）。
///
/// [DONE] T027 的字段级入口：搜索结果的每个字段都在"每本书的子树"内求值。
///
/// 与 [`evaluate_rule`] 的区别仅在于求值起点——它从文档根起，本函数从
/// `root` 起。语义上等价于"在这个子树里跑同一条规则"。
pub fn evaluate_rule_in(
    root: &crate::dom::NodeRef,
    rule: &str,
) -> Result<Vec<String>, crate::error::RuleError> {
    let (subject, chain) = split_chain(rule);
    let values = evaluate_rule_subject_in(root, &subject)?;
    Ok(apply_chain(values, &chain))
}

/// [`evaluate_rule_in`] 的**主体部分**（调用方已剥掉替换链）。
pub fn evaluate_rule_subject_in(
    root: &crate::dom::NodeRef,
    rule: &str,
) -> Result<Vec<String>, crate::error::RuleError> {
    use crate::dispatch::{dispatch_prefix, RuleKind};

    // 同 `evaluate_rule_subject`：`@webjs:` 要抛能力错误而非语法错。
    if crate::js_bridge::is_webjs_prefix(rule) {
        return Err(crate::js_bridge::webjs_unsupported_error("rule"));
    }

    let dispatched = dispatch_prefix(rule);
    match dispatched.kind {
        RuleKind::Css | RuleKind::Default | RuleKind::ExplicitDefault => {}
        other => {
            return Err(crate::error::RuleError::unsupported(
                other.capability_id(),
                "rule",
            ));
        }
    }

    let body = if dispatched.reversed {
        format!("-{}", dispatched.body)
    } else {
        dispatched.body.clone()
    };
    evaluate_css_body_in(root, &body)
}

fn evaluate_css_body_in(
    root: &crate::dom::NodeRef,
    rule: &str,
) -> Result<Vec<String>, crate::error::RuleError> {
    let tail = parse_rule_tail(rule);
    let selector = crate::css::compile(&tail.selector)?;
    // 从子树根起查：`css::select` 含起点自身，与 `select_in_document` 一致
    let mut nodes = crate::css::select(root, &selector);

    if tail.reversed {
        nodes.reverse();
    }
    let nodes = tail.slice.apply_nodes(nodes);

    let mut values = Vec::new();
    for node in &nodes {
        values.extend(tail.suffix.evaluate(node));
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 后缀 ----------

    #[test]
    fn 已知后缀可解析() {
        assert_eq!(RuleSuffix::parse("text"), Some(RuleSuffix::Text));
        assert_eq!(RuleSuffix::parse("textNodes"), Some(RuleSuffix::TextNodes));
        assert_eq!(RuleSuffix::parse("ownText"), Some(RuleSuffix::OwnText));
        assert_eq!(RuleSuffix::parse("html"), Some(RuleSuffix::Html));
        assert_eq!(RuleSuffix::parse("all"), Some(RuleSuffix::All));
        assert_eq!(RuleSuffix::parse("href"), Some(RuleSuffix::Href));
        assert_eq!(RuleSuffix::parse("src"), Some(RuleSuffix::Src));
        assert_eq!(
            RuleSuffix::parse("unknownSuffix"),
            None,
            "未知后缀不该被猜成某个已存在的"
        );
    }

    #[test]
    fn 后缀求值落在jsoup语义上() {
        let document = Document::parse_html("<div id=\"w\"><p>Hello <b>there</b> now!</p></div>");
        let paragraph = document
            .descendant_elements()
            .find(|node| node.node_name() == "p")
            .expect("p 应存在");

        // text 归一化（Jsoup 公开文档的确切例子）
        assert_eq!(
            RuleSuffix::Text.evaluate(&paragraph),
            vec!["Hello there now!"]
        );
        // ownText 只看直接子级
        assert_eq!(RuleSuffix::OwnText.evaluate(&paragraph), vec!["Hello now!"]);
        // href / src 是一等后缀
        let document = Document::parse_html(r#"<a href="/x" data-k="v">t</a>"#);
        let anchor = document
            .descendant_elements()
            .find(|node| node.node_name() == "a")
            .expect("a 应存在");
        assert_eq!(RuleSuffix::Href.evaluate(&anchor), vec!["/x"]);
        assert_eq!(RuleSuffix::Src.evaluate(&anchor), Vec::<String>::new());
        assert_eq!(
            RuleSuffix::Attr("data-k".to_string()).evaluate(&anchor),
            vec!["v"]
        );
        assert_eq!(RuleSuffix::Text.evaluate(&anchor), vec!["t"]);
    }

    #[test]
    fn textnodes逐个给值而不是拼接() {
        // Jsoup 公开文档给出的例子：textNodes() = ["One ", " Three ", " Four"]
        let document = Document::parse_html("<p>One <span>Two</span> Three <br> Four</p>");
        let paragraph = document
            .descendant_elements()
            .find(|node| node.node_name() == "p")
            .expect("p 应存在");
        assert_eq!(
            RuleSuffix::TextNodes.evaluate(&paragraph),
            vec!["One ", " Three ", " Four"]
        );
        assert!(RuleSuffix::TextNodes.is_multi_valued());
        assert!(!RuleSuffix::Text.is_multi_valued());
    }

    #[test]
    fn html后缀产出可再解析的片段() {
        let document = Document::parse_html(r#"<div><p class="a">x</p></div>"#);
        let div = document
            .descendant_elements()
            .find(|node| node.node_name() == "div")
            .expect("div 应存在");
        assert_eq!(
            RuleSuffix::Html.evaluate(&div),
            vec![r#"<p class="a">x</p>"#]
        );
        assert_eq!(
            RuleSuffix::All.evaluate(&div),
            vec![r#"<div><p class="a">x</p></div>"#]
        );
    }

    #[test]
    fn 深嵌套的html序列化不爆栈() {
        // 回归测试：首版 `serialize_node` 用递归，3000 层嵌套下**实测栈溢出**
        // （STATUS_STACK_OVERFLOW，进程直接死）。而解析层与遍历层都刻意用了
        // 显式栈，序列化层漏了——AGENTS.md 的生产级护栏要求引擎不得因输入崩溃，
        // 而 DOM 深度由外部 HTML 决定，畸形页面深度可达数千。
        //
        // 深度取 3000：与 dom 模块的 `深嵌套不爆栈` 用同一个量级，
        // 该深度已实测能正常解析。
        let depth = 3000;
        let mut html = String::with_capacity(depth * 20);
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str("deep");
        for _ in 0..depth {
            html.push_str("</div>");
        }
        let document = Document::parse_html(&html);

        // `div@html` 对**每一个** div 各产出一份内部 HTML（3000 个 div → 3000 项）。
        // 爆栈发生在序列化**最深子树**时，即最外层 div 的那一项——
        // 它是唯一需要下探 2999 层的调用，故用它做断言。
        let values = evaluate_rule(&document, "div@html").expect("应求值成功");
        assert_eq!(values.len(), depth, "每个 div 各产出一项");

        let outermost = &values[0];
        assert!(
            outermost.contains("deep"),
            "最外层序列化应含最内层文本（长度 {}）",
            outermost.len()
        );
        assert_eq!(
            outermost.matches("<div>").count(),
            depth - 1,
            "最外层序列化应含全部内层 div"
        );
        // 递归版本在这里会 STATUS_STACK_OVERFLOW；能跑到这条断言即证明改动生效
        assert!(outermost.ends_with("</div>"));
    }

    #[test]
    fn html序列化对void元素不写结束标签() {
        let document = Document::parse_html("<div><br><img src=\"/a.png\"></div>");
        let div = document
            .descendant_elements()
            .find(|node| node.node_name() == "div")
            .expect("div 应存在");
        assert_eq!(
            RuleSuffix::Html.evaluate(&div),
            vec![r#"<br><img src="/a.png">"#]
        );
    }

    #[test]
    fn html序列化转义文本与属性() {
        let document = Document::parse_html(r#"<div title="a&quot;b">x &lt; y</div>"#);
        let div = document
            .descendant_elements()
            .find(|node| node.node_name() == "div")
            .expect("div 应存在");
        let html = RuleSuffix::All.evaluate(&div).remove(0);
        assert!(html.contains("x &lt; y"), "文本里的 `<` 应被转义：{html}");
        assert!(
            html.contains(r#"title="a&quot;b""#),
            "属性里的引号应被转义：{html}"
        );
    }

    #[test]
    fn 未指定后缀按文本处理() {
        assert_eq!(RuleSuffix::parse(""), Some(RuleSuffix::Unspecified));
        let document = Document::parse_html("<p>x</p>");
        let paragraph = document
            .descendant_elements()
            .find(|node| node.node_name() == "p")
            .expect("p 应存在");
        assert_eq!(RuleSuffix::Unspecified.evaluate(&paragraph), vec!["x"]);
    }

    // ---------- 索引切片 ----------

    #[test]
    fn 单个索引含负数() {
        let values: Vec<String> = (0..5).map(|index| index.to_string()).collect();
        assert_eq!(IndexSlice::Single(0).apply(values.clone()), vec!["0"]);
        assert_eq!(IndexSlice::Single(2).apply(values.clone()), vec!["2"]);
        // 原文：-1 为倒数第一个，-2 为倒数第二个
        assert_eq!(IndexSlice::Single(-1).apply(values.clone()), vec!["4"]);
        assert_eq!(IndexSlice::Single(-2).apply(values.clone()), vec!["3"]);
        // 越界返回空而不是报错（理由见 apply 的文档）
        assert_eq!(
            IndexSlice::Single(99).apply(values.clone()),
            Vec::<String>::new()
        );
        assert_eq!(IndexSlice::Single(-99).apply(values), Vec::<String>::new());
    }

    #[test]
    fn 区间切片两端都是闭区间() {
        let values: Vec<String> = (0..5).map(|index| index.to_string()).collect();
        assert_eq!(
            IndexSlice::Range {
                start: 0,
                end: Some(2)
            }
            .apply(values.clone()),
            vec!["0", "1", "2"],
            "闭区间：[0:2] 取三个元素"
        );
        assert_eq!(
            IndexSlice::Range {
                start: 2,
                end: None
            }
            .apply(values.clone()),
            vec!["2", "3", "4"]
        );
        assert_eq!(
            IndexSlice::Range {
                start: -2,
                end: None
            }
            .apply(values.clone()),
            vec!["3", "4"]
        );
        // 越界端点被夹到合法范围
        assert_eq!(
            IndexSlice::Range {
                start: 3,
                end: Some(99)
            }
            .apply(values.clone()),
            vec!["3", "4"]
        );
        // 起点晚于终点 → 空
        assert_eq!(
            IndexSlice::Range {
                start: 4,
                end: Some(1)
            }
            .apply(values),
            Vec::<String>::new()
        );
    }

    #[test]
    fn 排除切片() {
        let values: Vec<String> = (0..5).map(|index| index.to_string()).collect();
        assert_eq!(
            IndexSlice::Exclude {
                start: 1,
                end: Some(2)
            }
            .apply(values.clone()),
            vec!["0", "3", "4"]
        );
        assert_eq!(
            IndexSlice::Exclude {
                start: 0,
                end: Some(0)
            }
            .apply(values),
            vec!["1", "2", "3", "4"]
        );
    }

    #[test]
    fn 空列表上任何切片都产出空() {
        for slice in [
            IndexSlice::Single(0),
            IndexSlice::Single(-1),
            IndexSlice::Range {
                start: 0,
                end: None,
            },
            IndexSlice::Exclude {
                start: 0,
                end: None,
            },
            IndexSlice::All,
        ] {
            assert_eq!(slice.apply(Vec::new()), Vec::<String>::new());
        }
    }

    #[test]
    fn 方括号形态可解析() {
        assert_eq!(
            IndexSlice::parse_bracket("[0]"),
            Some(IndexSlice::Single(0))
        );
        assert_eq!(
            IndexSlice::parse_bracket("[-1]"),
            Some(IndexSlice::Single(-1))
        );
        assert_eq!(
            IndexSlice::parse_bracket("[0:3]"),
            Some(IndexSlice::Range {
                start: 0,
                end: Some(3)
            })
        );
        assert_eq!(
            IndexSlice::parse_bracket("[2:]"),
            Some(IndexSlice::Range {
                start: 2,
                end: None
            })
        );
        // 原文：「!是排除」
        assert_eq!(
            IndexSlice::parse_bracket("[!0:3]"),
            Some(IndexSlice::Exclude {
                start: 0,
                end: Some(3)
            })
        );
        assert_eq!(
            IndexSlice::parse_bracket("[!-3:-1]"),
            Some(IndexSlice::Exclude {
                start: -3,
                end: Some(-1)
            })
        );
        // 非切片的方括号（CSS 属性选择器）不该被认下
        assert_eq!(IndexSlice::parse_bracket("[href]"), None);
        assert_eq!(IndexSlice::parse_bracket("[]"), None);
        assert_eq!(IndexSlice::parse_bracket("[!3]"), None, "排除必须带区间");
    }

    #[test]
    fn 点号形态可解析() {
        assert_eq!(IndexSlice::parse_dot(".0"), Some(IndexSlice::Single(0)));
        assert_eq!(IndexSlice::parse_dot(".-1"), Some(IndexSlice::Single(-1)));
        assert_eq!(IndexSlice::parse_dot(".odd"), None, "类名不是索引");
        assert_eq!(IndexSlice::parse_dot("0"), None);
    }

    // ---------- 规则尾部解析 ----------

    #[test]
    fn 尾部解析覆盖公开文档的全部形态() {
        // 公开文档示例：@css:.name>a@href
        let tail = parse_rule_tail(".name>a@href");
        assert_eq!(tail.selector, ".name>a");
        assert_eq!(tail.suffix, RuleSuffix::Href);
        assert_eq!(tail.slice, IndexSlice::All);
        assert!(!tail.reversed);

        // 公开文档示例：@css:p:eq(2)>a@text（伪类定位，切片为全部）
        let tail = parse_rule_tail("p:eq(2)>a@text");
        assert_eq!(tail.selector, "p:eq(2)>a");
        assert_eq!(tail.suffix, RuleSuffix::Text);

        // 方括号切片 + 后缀
        let tail = parse_rule_tail(".list[0]@text");
        assert_eq!(tail.selector, ".list");
        assert_eq!(tail.suffix, RuleSuffix::Text);
        assert_eq!(tail.slice, IndexSlice::Single(0));

        // 方括号切片，无后缀
        let tail = parse_rule_tail(".list[0:3]");
        assert_eq!(tail.selector, ".list");
        assert_eq!(tail.suffix, RuleSuffix::Unspecified);
        assert_eq!(
            tail.slice,
            IndexSlice::Range {
                start: 0,
                end: Some(3)
            }
        );

        // 点号索引 + 后缀（Default 语法形态）
        let tail = parse_rule_tail(".list.-1@text");
        assert_eq!(tail.selector, ".list");
        assert_eq!(tail.suffix, RuleSuffix::Text);
        assert_eq!(tail.slice, IndexSlice::Single(-1));

        // 反序标记
        let tail = parse_rule_tail("-.list@text");
        assert_eq!(tail.selector, ".list");
        assert_eq!(tail.suffix, RuleSuffix::Text);
        assert!(tail.reversed);

        // 属性后缀
        let tail = parse_rule_tail(".list@attr(data-id)");
        assert_eq!(tail.selector, ".list");
        assert_eq!(tail.suffix, RuleSuffix::Attr("data-id".to_string()));
    }

    #[test]
    fn 属性选择器不被误当切片() {
        // `[href]` 在**中间**，不是尾部切片
        let tail = parse_rule_tail("a[href]@text");
        assert_eq!(tail.selector, "a[href]");
        assert_eq!(tail.suffix, RuleSuffix::Text);
        assert_eq!(tail.slice, IndexSlice::All);

        // 尾部是属性选择器但整体无后缀——这是合法 CSS，不该被当切片
        let tail = parse_rule_tail("a[href]");
        assert_eq!(tail.selector, "a[href]");
        assert_eq!(tail.slice, IndexSlice::All);
    }

    #[test]
    fn 未知后缀不被吞掉() {
        // `@unknown` 不是后缀，应留在选择器里让 CSS 解析器报出准确错误
        let tail = parse_rule_tail(".list@unknownThing");
        assert_eq!(tail.selector, ".list@unknownThing");
        assert_eq!(tail.suffix, RuleSuffix::Unspecified);
    }

    // ---------- 端到端 ----------

    #[test]
    fn 端到端规则求值() {
        let document = Document::parse_html(
            r#"<ul><li class="i"><a href="/1">一</a></li>\
               <li class="i"><a href="/2">二</a></li>\
               <li class="i"><a href="/3">三</a></li></ul>"#,
        );
        assert_eq!(
            evaluate_rule(&document, ".i>a@href").expect("应求值成功"),
            vec!["/1", "/2", "/3"]
        );
        assert_eq!(
            evaluate_rule(&document, ".i>a[0]@href").expect("应求值成功"),
            vec!["/1"]
        );
        assert_eq!(
            evaluate_rule(&document, ".i>a[-1]@href").expect("应求值成功"),
            vec!["/3"]
        );
        assert_eq!(
            evaluate_rule(&document, ".i>a[0:1]@href").expect("应求值成功"),
            vec!["/1", "/2"]
        );
        assert_eq!(
            evaluate_rule(&document, ".i>a[!0:1]@href").expect("应求值成功"),
            vec!["/3"]
        );
        // 反序
        assert_eq!(
            evaluate_rule(&document, "-.i>a@href").expect("应求值成功"),
            vec!["/3", "/2", "/1"]
        );
        // 反序 + 切片：先反序再取位置
        assert_eq!(
            evaluate_rule(&document, "-.i>a[0]@href").expect("应求值成功"),
            vec!["/3"]
        );
        // 后缀
        assert_eq!(
            evaluate_rule(&document, ".i>a@text").expect("应求值成功"),
            vec!["一", "二", "三"]
        );
        assert_eq!(
            evaluate_rule(&document, ".i@html")
                .expect("应求值成功")
                .len(),
            3
        );
    }

    #[test]
    fn 规则可带替换链() {
        // 公开文档：「如需要正则替换在最后加上 ##正则表达式##替换内容」
        // 最典型的用法是把相对地址补成绝对地址。
        let document = Document::parse_html(
            r#"<ul><li><a href="/book/1">一</a></li><li><a href="/book/2">二</a></li></ul>"#,
        );
        assert_eq!(
            evaluate_rule(&document, r#"li>a@href##^##https://x.com"#).expect("应求值成功"),
            vec!["https://x.com/book/1", "https://x.com/book/2"],
            "替换应作用在**每个值**上——拼接后再替换会让 `^` 只命中整串开头"
        );
    }

    #[test]
    fn 替换链可叠加() {
        // 两条替换：`--` → `X`，然后 `-` → `Y`。按序生效，故
        // "a--b-c" 先变 "aXb-c" 再变 "aXbYc"。
        // 顺序反了会得到 "a--bYc"（单 `-` 先被换掉，`--` 就匹配不上了）。
        let document = Document::parse_html("<p>a--b-c</p>");
        assert_eq!(
            evaluate_rule(&document, "p@text##-{2}##X##-##Y").expect("应求值成功"),
            vec!["aXbYc"],
            "多条替换按出现顺序生效"
        );

        // 顺序反过来（先 - 后 --）确实得到不同结果——证明"按序"不是巧合
        assert_eq!(
            evaluate_rule(&document, "p@text##-##Y##-{2}##X").expect("应求值成功"),
            vec!["aYYbYc"],
            "顺序不同结果不同"
        );
    }

    #[test]
    fn 无替换链时行为不变() {
        let document = Document::parse_html("<p>x</p>");
        assert_eq!(
            evaluate_rule(&document, "p@text").expect("应求值成功"),
            vec!["x"]
        );
        // 与主体入口等价
        assert_eq!(
            evaluate_rule(&document, "p@text").expect("应求值成功"),
            evaluate_rule_subject(&document, "p@text").expect("应求值成功")
        );
    }

    #[test]
    fn 子树内的规则也支持替换链() {
        let document = Document::parse_html(r#"<div class="book"><a href="/b/1">x</a></div>"#);
        let book = tome_rule_css_first(&document, "div.book");
        assert_eq!(
            evaluate_rule_in(&book, r"a@href##^##https://y.com").expect("应求值成功"),
            vec!["https://y.com/b/1"]
        );
    }

    /// 取文档里第一个匹配的元素（测试辅助）。
    fn tome_rule_css_first(document: &Document, selector: &str) -> crate::dom::NodeRef {
        let compiled = crate::css::compile(selector).expect("选择器应可编译");
        crate::css::select_in_document(document, &compiled)
            .into_iter()
            .next()
            .expect("应至少命中一个元素")
    }

    #[test]
    fn 端到端非法选择器返回结构化错误() {
        let document = Document::parse_html("<p>x</p>");
        let error = evaluate_rule(&document, "div >").expect_err("应报错");
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Rule);
    }

    #[test]
    fn 端到端无匹配产出空列表() {
        let document = Document::parse_html("<p>x</p>");
        assert_eq!(
            evaluate_rule(&document, ".missing@text").expect("应求值成功"),
            Vec::<String>::new()
        );
        // 有匹配但属性不存在 → 空
        assert_eq!(
            evaluate_rule(&document, "p@attr(nope)").expect("应求值成功"),
            Vec::<String>::new()
        );
    }

    // ---------- RuleEngine：五类规则的统一入口（P4 遗留的收口） ----------

    #[test]
    fn 引擎支持css与default() {
        let document = Document::parse_html(r#"<div class="n"><a href="/1">一</a></div>"#);
        let engine = RuleEngine::new(&document);
        assert_eq!(
            engine.evaluate("@css:.n>a@href").expect("应求值成功"),
            vec!["/1"]
        );
        assert_eq!(engine.evaluate("a@text").expect("应求值成功"), vec!["一"]);
    }

    #[test]
    fn 引擎支持xpath() {
        // research R2 的核心决策：XPath 跑在与 CSS 同一棵 DOM 上
        let document = Document::parse_html(r#"<ul><li>一</li><li>二</li></ul>"#);
        let engine = RuleEngine::new(&document);
        assert_eq!(
            engine.evaluate("@XPath://li").expect("应求值成功"),
            vec!["一", "二"]
        );
        // 隐式前缀（`//` 开头）
        assert_eq!(engine.evaluate("//li[2]").expect("应求值成功"), vec!["二"]);
        // CSS 与 XPath 在同一个文档上给出一致的结果
        assert_eq!(
            engine.evaluate("@css:li").expect("应求值成功"),
            engine.evaluate("@XPath://li").expect("应求值成功"),
            "两个引擎不该系统性分叉"
        );
    }

    #[test]
    fn 引擎支持jsonpath() {
        // JSON 片段嵌在 HTML 文本里（本方法的服务对象，见其文档）
        let document = Document::parse_html(r#"{"list":[{"n":"a"},{"n":"b"}]}"#);
        let engine = RuleEngine::new(&document);
        let values = engine.evaluate("@json:$.list[*].n").expect("应求值成功");
        assert_eq!(values, vec!["a", "b"]);
    }

    #[test]
    fn 未注入js时_js规则报能力错误() {
        // FR-006：未注入 JS 引擎时必须是**结构化能力错误**，
        // 而不是空结果——书源的 try/catch 靠这个标识选降级路径。
        let document = Document::parse_html("<p>x</p>");
        let engine = RuleEngine::new(&document);
        assert!(!engine.has_js());

        let error = engine.evaluate("@js:1+1").expect_err("应报能力错误");
        match error.to_error_kind() {
            tome_core::ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "js");
            }
            other => panic!("应产出 UnsupportedCapability，实际 {other:?}"),
        }
    }

    #[test]
    fn 注入js后字段规则里的_js_可用() {
        // 这是 P4 遗留的第一项：**字段规则**里的 `@js:` 要能跑。
        // 此前只有 `searchUrl` 插值里的 `@js:` 可用。
        let document = Document::parse_html(r#"<div class="n"><a href="/b/1">一</a></div>"#);
        let runtime =
            crate::js_bridge::JobRuntime::new(tome_js::Limits::default()).expect("应能建沙箱");
        let engine = RuleEngine::new(&document).with_js(&runtime);
        assert!(engine.has_js());

        // 纯计算
        assert_eq!(engine.evaluate("@js:1+1").expect("应求值成功"), vec!["2"]);
        // 表达式形态
        assert_eq!(
            engine.evaluate("@js:'a'+'b'").expect("应求值成功"),
            vec!["ab"]
        );
        // 语句块形态
        assert_eq!(
            engine.evaluate("@js:var x = 3; x * 2").expect("应求值成功"),
            vec!["6"]
        );
    }

    #[test]
    fn 字段规则里的_js_能参与加工链() {
        // 端到端：字段规则是 `@js:` + 替换链的形态（书源里常见）
        let document = Document::parse_html("<p>x</p>");
        let runtime =
            crate::js_bridge::JobRuntime::new(tome_js::Limits::default()).expect("应能建沙箱");
        let engine = RuleEngine::new(&document).with_js(&runtime);
        assert_eq!(
            engine
                .evaluate(r"@js:'/book/1'##^##https://x.com")
                .expect("应求值成功"),
            vec!["https://x.com/book/1"]
        );
    }

    #[test]
    fn js错误映射为规则错误() {
        let document = Document::parse_html("<p>x</p>");
        let runtime =
            crate::js_bridge::JobRuntime::new(tome_js::Limits::default()).expect("应能建沙箱");
        let engine = RuleEngine::new(&document).with_js(&runtime);
        let error = engine
            .evaluate("@js:throw new Error('boom')")
            .expect_err("应报错");
        assert!(
            format!("{error}").contains("boom"),
            "应保留脚本的原始信息：{error}"
        );
    }

    #[test]
    fn 引擎在子树内求值五类规则() {
        let document = Document::parse_html(
            r#"<ul><li class="i"><a href="/1">一</a></li><li class="i"><a href="/2">二</a></li></ul>"#,
        );
        let engine = RuleEngine::new(&document);
        let first = crate::css::select_in_document(
            &document,
            &crate::css::compile("li.i").expect("应编译"),
        )
        .into_iter()
        .next()
        .expect("应有 li");

        assert_eq!(
            engine.evaluate_in(&first, "a@href").expect("应求值成功"),
            vec!["/1"],
            "子树内的 CSS"
        );
        assert_eq!(
            engine
                .evaluate_in(&first, "@XPath:.//a")
                .expect("应求值成功"),
            vec!["一"],
            "子树内的 XPath（相对路径）"
        );
    }

    #[test]
    fn webjs在引擎里仍报能力错误() {
        let document = Document::parse_html("<p>x</p>");
        let runtime =
            crate::js_bridge::JobRuntime::new(tome_js::Limits::default()).expect("应能建沙箱");
        let engine = RuleEngine::new(&document).with_js(&runtime);
        let error = engine.evaluate("@webjs:1+1").expect_err("应报能力错误");
        match error.to_error_kind() {
            tome_core::ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "webView");
            }
            other => panic!("应产出 webView 能力错误，实际 {other:?}"),
        }
    }

    #[test]
    fn 正则allinone单独求值报语法错() {
        // 它需要"被匹配的文本"，单独一条没有输入——报错比静默返回空好
        let document = Document::parse_html("<p>x</p>");
        let engine = RuleEngine::new(&document);
        let error = engine.evaluate(":href=\"(/x)\"").expect_err("应报错");
        assert!(
            format!("{error}").contains("正则 AllInOne"),
            "错误应说明原因：{error}"
        );
    }
}
