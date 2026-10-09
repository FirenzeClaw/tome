//! [DONE] 宿主 API · 结构化数据类（tasks.md T039 / FR-005）
//!
//! # FR-005 的本类清单
//!
//! > **结构化数据**：JSON/XML 的解析与路径取值、网页标记的解析与选择
//!
//! 三项的落地方式**并不相同**，这个差异是有意的：
//!
//! | 能力 | 实现位置 | 为什么 |
//! |---|---|---|
//! | JSON 解析与路径取值 | **本模块**（`serde_json`） | JSON 是通用格式，与规则 DSL 无关 |
//! | XML 解析与路径取值 | **本模块**（最小自研） | 书源里 XML 极罕见（RSS 才用），不值得引一个大依赖 |
//! | 网页标记解析与选择 | **经 [`MarkupHost`] 注入**（由 tome-rule 实现） | see below |
//!
//! # 为什么"网页标记解析"必须经 trait 注入
//!
//! tasks.md T039 的原文要求"网页标记解析与选择——**接 tome-rule DOM**"。
//! 但架构铁律 A1 规定一切实现 crate 只依赖 `tome-core`（星型无环），
//! 故 `tome-js` **不能**依赖 `tome-rule`。
//!
//! 解法与 T043 的 `@js:` 桥完全同构：**trait 定义在可以被依赖的一侧，
//! 由组合根接线**。
//!
//! ```text
//!   tome-rule ──实现──▶ MarkupHost（trait 定义在 tome-js）
//!       │                     ▲
//!       │                     │ 组合根注入
//!       └───依赖──▶ tome-js ───┘
//! ```
//!
//! 这样 `tome-rule` 依然只依赖 `tome-js` 与 `tome-core`（不产生环），
//! 而 JS 侧的 `java.html(x).select('@css:...')` 能落到真实的规则引擎上。
//!
//! ## `[UNCERTAIN]` 这一层包装的代价
//!
//! 每次 `select` 调用都要跨越 trait 边界（字符串进出），比"tome-js 直接
//! 用 tome-rule 的 DOM 类型"多一次转换。对书源脚本的调用频率而言这个
//! 代价可忽略（一次抓取最多几十次选择），而换来的是架构约束不被破坏。
//! 若某天实测成为瓶颈，应先测量再决定——而不是现在就为性能破坏分层。

use rquickjs::{Ctx, Object};
use serde_json::Value;

use crate::bridge::throw_host_error;
use crate::sandbox_error::JsError;

/// 网页标记（HTML/XML）的解析与选择能力（由 `tome-rule` 实现，组合根注入）。
///
/// # 为什么接口是"字符串进、字符串出"
///
/// 这是**接缝的必然形状**：trait 定义在 `tome-js`，而实现方 `tome-rule`
/// 持有自己的 DOM 类型（arena DOM / `NodeRef`）。若接口里出现那些类型，
/// `tome-js` 就得依赖 `tome-rule`（破坏 A1）。
///
/// 返回 `Vec<String>` 而不是单个字符串：CSS 规则天然可以命中多个节点
/// （`div.a` 匹配全部同类元素），书源脚本靠 `.length` 判断命中数。
///
/// # 实现方须遵守的语义
///
/// - `rule` 是**书源规则原文**（可含 `@css:` 前缀、后缀、索引切片、
///   替换链）。实现方应当直接把它交给规则引擎，而不是自己解析——
///   解析逻辑重复一份必然与规则引擎漂移。
/// - 求值失败须返回 `Err`，由本模块转成 JS 异常。**不得**返回空列表
///   来掩盖失败："没有匹配"与"规则写错了"对书源作者是完全不同的信号。
pub trait MarkupHost: Send + Sync {
    /// 在给定标记文本上按书源规则求值，返回全部匹配结果。
    fn select(&self, markup: &str, rule: &str) -> Result<Vec<String>, JsError>;
}

/// 注册结构化数据类的全部宿主函数到 `java` 对象。
///
/// `markup_host` 为 `None` 时，网页选择类的入口仍会注册，但调用时抛出
/// **结构化能力错误**（`capability = "markup"`）——这比"函数不存在"
/// 好：书源的 `try/catch` 能据此降级（FR-006）。
pub fn register<'js>(
    ctx: &Ctx<'js>,
    java: &Object<'js>,
    markup_host: Option<std::sync::Arc<dyn MarkupHost>>,
) -> Result<usize, JsError> {
    let mut count = 0usize;

    // ---------- JSON ----------
    java.set(
        "jsonParse",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String| -> Result<rquickjs::Value<'js>, rquickjs::Error> {
                // 解析成 `serde_json::Value` 再转成 JS 值。
                //
                // ## 为什么不直接把文本交给 `JSON.parse`
                //
                // 那看起来更简单（引擎自带），但会引入两处不一致：
                // 1. 书源拿到的文本常带 BOM 或首尾空白（从响应里截出来的），
                //    原生 `JSON.parse` 会拒绝，而书源作者看不出原因。
                // 2. 我们**必须先解析一次**才能做 `jsonPath` 取值
                //    （见下），两次解析的容错行为应当一致。
                match parse_json(&text) {
                    Ok(value) => {
                        json_to_js(&ctx, &value).map_err(|error| throw_host_error(&ctx, &error))
                    }
                    Err(error) => Err(throw_host_error(&ctx, &error)),
                }
            },
        )
        .map_err(|error| registration_error("jsonParse", &error))?,
    )
    .map_err(|error| registration_error("jsonParse", &error))?;
    count += 1;

    // JSON 序列化（书源里 `java.toJson` / `JSON.stringify` 两种写法都有）
    java.set(
        "toJson",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, value: rquickjs::Value<'js>| -> Result<String, rquickjs::Error> {
                // 用引擎自己的 JSON 序列化：JS 值的形态（函数、undefined、
                // 循环引用）只有引擎知道该怎么处理，我们不该重写一套。
                ctx.json_stringify(value.clone())
                    .ok()
                    .flatten()
                    .and_then(|text| text.to_string().ok())
                    .ok_or_else(|| {
                        throw_host_error(
                            &ctx,
                            &JsError::new("值无法序列化为 JSON（可能含循环引用）"),
                        )
                    })
            },
        )
        .map_err(|error| registration_error("toJson", &error))?,
    )
    .map_err(|error| registration_error("toJson", &error))?;
    count += 1;

    // ---------- JSON 路径取值 ----------
    //
    // ## 为什么不用规则引擎的 JSONPath
    //
    // 书源脚本里的路径取值语法是 **JavaScript 原生**（`data.list[0].name`），
    // 不是规则 DSL 的 `$.data.list`。前者由 JS 引擎自己求值即可——我们只需
    // 把解析出的对象放进引擎，然后求值那段表达式。
    java.set(
        "jsonPath",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             text: String,
             path: String|
             -> Result<rquickjs::Value<'js>, rquickjs::Error> {
                let value = parse_json(&text).map_err(|error| throw_host_error(&ctx, &error))?;
                eval_json_path(&ctx, &value, &path).map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("jsonPath", &error))?,
    )
    .map_err(|error| registration_error("jsonPath", &error))?;
    count += 1;

    // ---------- 网页标记 ----------
    //
    // `java.getElement` / `java.getElements` 是书源里对 HTML 做选择的两种
    // 写法（前者取首个，后者取全部）。两者都经 [`MarkupHost`]。
    java.set(
        "getElements",
        rquickjs::Function::new(ctx.clone(), {
            let host = markup_host.clone();
            move |ctx: Ctx<'js>,
                  markup: String,
                  rule: String|
                  -> Result<Vec<String>, rquickjs::Error> {
                let host = host
                    .as_ref()
                    .ok_or_else(|| throw_host_error(&ctx, &markup_unavailable_error()))?;
                host.select(&markup, &rule)
                    .map_err(|error| throw_host_error(&ctx, &error))
            }
        })
        .map_err(|error| registration_error("getElements", &error))?,
    )
    .map_err(|error| registration_error("getElements", &error))?;
    count += 1;

    java.set(
        "getElement",
        rquickjs::Function::new(ctx.clone(), {
            let host = markup_host;
            move |ctx: Ctx<'js>, markup: String, rule: String| -> Result<String, rquickjs::Error> {
                let host = host
                    .as_ref()
                    .ok_or_else(|| throw_host_error(&ctx, &markup_unavailable_error()))?;
                let values = host
                    .select(&markup, &rule)
                    .map_err(|error| throw_host_error(&ctx, &error))?;
                // 取首个；无匹配返回空串（书源里 `java.getElement` 的
                // 返回值为空时通常直接判空，空串比 `undefined` 更少陷阱）。
                Ok(values.into_iter().next().unwrap_or_default())
            }
        })
        .map_err(|error| registration_error("getElement", &error))?,
    )
    .map_err(|error| registration_error("getElement", &error))?;
    count += 1;

    // ---------- XML ----------
    java.set(
        "xmlParse",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String| -> Result<String, rquickjs::Error> {
                parse_xml_to_json(&text).map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("xmlParse", &error))?,
    )
    .map_err(|error| registration_error("xmlParse", &error))?;
    count += 1;

    Ok(count)
}

/// 未注入 `MarkupHost` 时的能力错误。
///
/// 能力标识用 `markup`（书源里"网页标记解析"的通用叫法），
/// 说明里指明这是组合根的配置问题而非书源的错。
fn markup_unavailable_error() -> JsError {
    JsError::unsupported(
        "markup",
        "本沙箱未启用网页标记解析（组合根未注入 MarkupHost，通常应接 tome-rule 的 DOM）",
    )
}

/// 构造注册失败的诊断错误。
fn registration_error(name: &str, error: &rquickjs::Error) -> JsError {
    JsError::new(format!("注册 `{name}` 失败：{error}"))
}

/// 解析 JSON 文本，容错 BOM 与首尾空白。
///
/// # 为什么容错 BOM
///
/// 书源脚本拿到的 JSON 文本常是从响应体里截出来的（`response.substring(n)`），
/// 而响应可能带 UTF-8 BOM。原生 `JSON.parse` 会因 BOM 报 "Unexpected token"，
/// 而那个字符在编辑器里看不见——书源作者无从定位。
pub(crate) fn parse_json(text: &str) -> Result<Value, JsError> {
    let cleaned = text.trim_start_matches('\u{feff}').trim();
    serde_json::from_str(cleaned).map_err(|error| JsError::new(format!("JSON 解析失败：{error}")))
}

/// 把 `serde_json::Value` 转成 JS 值。
///
/// # 为什么用 JSON 文本中转而不是逐类型构造
///
/// `rquickjs` 为每种 JS 类型都提供了构造 API（`Object::new` /
/// `Array::new` / `set` …），逐类型写一遍要处理"对象键 → 属性名"、
/// "数字 → int 还是 float"、"null 与 undefined 的区别"三类细节。
/// 而"`serde_json::Value` → JSON 文本 → `ctx.json_parse`"只需三步且
/// 语义由引擎自己保证（它是 JS 的原生解析器）。
///
/// 代价是一次序列化 + 一次解析。对书源脚本的数据量（几 KB 到几百 KB）
/// 可接受，换来的是不会有"我们自己转换时漏了某个类型"的缺陷。
pub(crate) fn json_to_js<'js>(
    ctx: &Ctx<'js>,
    value: &Value,
) -> Result<rquickjs::Value<'js>, JsError> {
    let text = serde_json::to_string(value)
        .map_err(|error| JsError::new(format!("JSON 序列化失败：{error}")))?;
    ctx.json_parse(text)
        .map_err(|error| JsError::new(format!("转换为 JS 值失败：{error}")))
}

/// 在 JSON 文本上按 JS 路径表达式取值。
///
/// # 实现方式：临时全局变量
///
/// 要让 JS 表达式能在解析出的对象上求值，必须把那个对象放进引擎的某个
/// 作用域。三种做法的取舍：
///
/// | 做法 | 问题 |
/// |---|---|
/// | 拼进脚本字面量 | 对象可能有几 MB，拼成字面量既慢又可能超栈 |
/// | 临时全局变量 | 需保证用完即删（否则污染书源的全局视图） |
/// | 具名函数参数 | `eval` 无法把值作为参数注入（不是调用） |
///
/// 取第二种。临时变量的名字带 `__tome_` 前缀，且**无论成功失败都删除**
/// （书源会遍历全局对象，残留的键会污染它的视图）。
fn eval_json_path<'js>(
    ctx: &Ctx<'js>,
    value: &Value,
    path: &str,
) -> Result<rquickjs::Value<'js>, JsError> {
    const ROOT: &str = "__tome_json_root";

    let root = json_to_js(ctx, value)?;
    ctx.globals()
        .set(ROOT, root)
        .map_err(|error| JsError::new(format!("注入取值根失败：{error}")))?;

    // 路径为空时直接返回根（书源里 `java.jsonPath(text, "")` 表示取整个对象）。
    let expression = if path.trim().is_empty() {
        ROOT.to_string()
    } else {
        format!("{ROOT}.{path}")
    };

    let outcome = ctx.eval::<rquickjs::Value<'js>, _>(expression.as_bytes());

    // 无论成败都清理。失败也清理是必须的：不然一次失败的取值会让
    // 这个全局键永久残留。
    let _ = ctx
        .globals()
        .set(ROOT, rquickjs::Value::new_undefined(ctx.clone()));

    match outcome {
        Ok(value) => Ok(value),
        Err(rquickjs::Error::Exception) => {
            // 取值失败（路径不存在、抛错）**不报错**，返回 undefined。
            //
            // ## 为什么这个场合不报错
            //
            // 书源里大量出现 `java.jsonPath(text, "data.list[0].name")` 而
            // 某个中间节点可能不存在——那是**数据的常态**（接口返回结构
            // 随业务变化），不是书源写错了。报错会让整段脚本中断，
            // 而 `undefined` 能让书源继续（后续判空逻辑会处理）。
            //
            // 与"JSON 本身非法"区别开：那种情况在 `parse_json` 阶段就报了错
            // （那是书源的输入问题，必须让作者知道）。
            let _ = ctx.catch();
            Ok(rquickjs::Value::new_undefined(ctx.clone()))
        }
        Err(other) => Err(JsError::new(format!("路径求值失败：{other}"))),
    }
}

/// 把 XML 解析成 JSON 文本（供书源脚本读取）。
///
/// # 为什么自研而不是引 `quick-xml`
///
/// 书源里 XML 的用途极窄：RSS 订阅源与极少数老站点的接口。所需的全部
/// 能力是"把标签树变成嵌套对象"，不涉及命名空间、DTD、CDATA 的完整语义。
/// 引一个大依赖为这么窄的用途不值（体积纪律）。
///
/// 标 `[WIP]`：本实现覆盖**格式良好的简单 XML**（无命名空间、无 DTD、
/// 无实体引用）。畸形输入报错而不是猜。若 S1-6 的基线显示 RSS 源的
/// XML 结构比预期复杂，应改用成熟解析器并接受体积代价。
///
/// # 输出形状
///
/// ```xml
/// <rss><channel><title>T</title><item><title>I1</title></item></channel></rss>
/// ```
/// →
/// ```json
/// {"rss":{"channel":{"title":"T","item":{"title":"I1"}}}}
/// ```
///
/// 同名子元素**重复出现时**收成数组（`<item>a</item><item>b</item>` →
/// `"item": ["a","b"]`）——那是 RSS 的典型形态，且书源需要遍历它们。
fn parse_xml_to_json(text: &str) -> Result<String, JsError> {
    let (root_name, children) = parse_xml(text)?;
    // ## 为什么这里显式包一层而不是直接把根节点转 JSON
    //
    // `XmlNode::to_json` 对元素的语义是"把**子节点**收成对象"——它对应的是
    // `<channel><title>T</title></channel>` → `{"title":"T"}`。
    // 那是**元素内部**的形状，元素自身的名字由父层负责写成键。
    // 根节点没有父层，故在此补上：`{"rss": {...}}`。
    //
    // 若忘了这一层，书源拿到的 JSON 会缺掉根标签，而它只能靠猜来补
    // （例如硬编码 `data.channel`），换一个源就失效。
    let wrapped = Value::Object({
        let mut object = serde_json::Map::new();
        object.insert(
            root_name,
            XmlNode::Element {
                name: String::new(),
                children,
            }
            .to_json(),
        );
        object
    });
    serde_json::to_string(&wrapped).map_err(|error| JsError::new(format!("序列化失败：{error}")))
}

/// 解析出的 XML 节点（自研最小实现，见 [`parse_xml_to_json`] 的说明）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum XmlNode {
    /// 文本内容
    Text(String),
    /// 元素：标签名 + 子节点
    Element {
        /// 标签名（不含属性——属性本实现不保留，理由见下）
        ///
        /// `[WIP]` 属性未保留：书源里从 XML 取值的场合几乎只用标签与文本
        /// （`<link>url</link>`），而属性（`<enclosure url="..."/>`）在
        /// RSS 里确实存在。若 S1-6 的基线显示需要它，应在此处补。
        name: String,
        /// 子节点（有序）
        children: Vec<XmlNode>,
    },
}

impl XmlNode {
    /// 转成 JSON 值。
    fn to_json(&self) -> Value {
        match self {
            XmlNode::Text(text) => Value::String(text.clone()),
            XmlNode::Element { name: _, children } => {
                let mut object = serde_json::Map::new();
                // 分组同名子节点：
                // - 只出现一次 → 直接给值（书源里 `x.title` 是最自然的取值）
                // - 出现多次 → 给数组（RSS 的多个 `<item>` 需要遍历）
                let mut grouped: Vec<(String, Vec<&XmlNode>)> = Vec::new();
                for child in children {
                    let XmlNode::Element { name, .. } = child else {
                        continue;
                    };
                    match grouped.iter_mut().find(|(key, _)| key == name) {
                        Some((_, nodes)) => nodes.push(child),
                        None => grouped.push((name.clone(), vec![child])),
                    }
                }
                for (key, nodes) in grouped {
                    if nodes.len() == 1 {
                        object.insert(key, nodes[0].to_json());
                    } else {
                        object.insert(
                            key,
                            Value::Array(nodes.iter().map(|n| n.to_json()).collect()),
                        );
                    }
                }
                // 纯文本内容的元素直接给出文本（`<title>T</title>` → `"T"`），
                // 而不是 `{"#text": "T"}`——后者在书源里要多写一层，
                // 而普通文本元素是绝大多数。
                if object.is_empty() {
                    let text = collect_text(children);
                    return Value::String(text);
                }
                Value::Object(object)
            }
        }
    }
}

/// 把子节点里的全部文本拼接起来（去首尾空白）。
fn collect_text(children: &[XmlNode]) -> String {
    let mut output = String::new();
    for child in children {
        match child {
            XmlNode::Text(text) => output.push_str(text),
            XmlNode::Element { children, .. } => output.push_str(&collect_text(children)),
        }
    }
    output.trim().to_string()
}

/// 解析格式良好的简单 XML（见 [`parse_xml_to_json`] 的能力边界说明）。
///
/// 返回 `(根标签名, 根的子节点)`——名字与子节点分开给，因为根节点没有
/// 父层来把它的名字写成键（见 [`parse_xml_to_json`] 的说明）。
fn parse_xml(text: &str) -> Result<(String, Vec<XmlNode>), JsError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(JsError::new("XML 内容为空"));
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let mut position = 0usize;

    // 跳过 XML 声明（`<?xml ... ?>`）与注释
    skip_prologue(&chars, &mut position);

    let (name, children) = parse_element(&chars, &mut position, trimmed)?;
    Ok((name, children))
}

/// 跳过 XML 声明与注释。
fn skip_prologue(chars: &[char], position: &mut usize) {
    loop {
        // 跳过空白
        while *position < chars.len() && chars[*position].is_whitespace() {
            *position += 1;
        }
        if *position + 1 < chars.len() && chars[*position] == '<' {
            let next = chars[*position + 1];
            if next == '?' {
                // `<?xml ... ?>`
                while *position < chars.len() && chars[*position] != '>' {
                    *position += 1;
                }
                *position += 1;
                continue;
            }
            if next == '!' {
                // 注释 `<!-- ... -->` 或 DOCTYPE `<!DOCTYPE ...>`
                if chars.get(*position + 2) == Some(&'-') && chars.get(*position + 3) == Some(&'-')
                {
                    // ## 必须先跳过开头的 `<!--`
                    //
                    // 若从当前位置直接扫 `--`，会立刻命中开头的那两个连字符，
                    // 于是一次都不前进就以为找到了结束——结果是"注释没被跳过，
                    // 反而把扫描位置留在了原地"，下一轮解析就会在 `<!--`
                    // 上按标签处理并报出与真实位置无关的语法错。
                    *position += 4;
                    while *position + 1 < chars.len()
                        && !(chars[*position] == '-' && chars[*position + 1] == '-')
                    {
                        *position += 1;
                    }
                    // 跳过结尾的 `-->`（三个字符）。越界时钳到末尾让
                    // 后续的"未闭合"判断给出更准确的消息。
                    *position = (*position + 3).min(chars.len());
                } else {
                    while *position < chars.len() && chars[*position] != '>' {
                        *position += 1;
                    }
                    *position += 1;
                }
                continue;
            }
        }
        return;
    }
}

/// 解析一个元素（`<name ...>children</name>` 或自闭合 `<name/>`）。
///
/// 返回 `(标签名, 子节点)`。
fn parse_element(
    chars: &[char],
    position: &mut usize,
    source: &str,
) -> Result<(String, Vec<XmlNode>), JsError> {
    // 起始标签
    if chars.get(*position) != Some(&'<') {
        return Err(JsError::new(format!(
            "XML 结构错误：位置 {} 处应为标签起始（片段：{:?}）",
            position,
            source.chars().skip(*position).take(20).collect::<String>()
        )));
    }
    *position += 1;

    let mut name = String::new();
    while *position < chars.len()
        && !chars[*position].is_whitespace()
        && chars[*position] != '>'
        && chars[*position] != '/'
    {
        name.push(chars[*position]);
        *position += 1;
    }
    if name.is_empty() {
        return Err(JsError::new("XML 结构错误：空的标签名"));
    }

    // 跳过属性（见 `XmlNode::Element::name` 的说明：本实现不保留属性）
    loop {
        while *position < chars.len() && chars[*position].is_whitespace() {
            *position += 1;
        }
        match chars.get(*position) {
            Some('>') => {
                *position += 1;
                break;
            }
            Some('/') => {
                // 自闭合：`<name/>`
                if chars.get(*position + 1) == Some(&'>') {
                    *position += 2;
                    return Ok((name, Vec::new()));
                }
                return Err(JsError::new("XML 结构错误：`/` 后应为 `>`"));
            }
            Some(_) => {
                // 属性名与值：跳到下一个空白或 `>`
                while *position < chars.len()
                    && !chars[*position].is_whitespace()
                    && chars[*position] != '>'
                    && chars[*position] != '/'
                {
                    *position += 1;
                }
            }
            None => return Err(JsError::new("XML 结构错误：标签未闭合")),
        }
    }

    // 子节点
    let mut children = Vec::new();
    loop {
        // 文本节点（到下一个 `<` 为止）
        let mut text = String::new();
        while *position < chars.len() && chars[*position] != '<' {
            text.push(chars[*position]);
            *position += 1;
        }
        if !text.trim().is_empty() {
            children.push(XmlNode::Text(text));
        }

        if *position >= chars.len() {
            return Err(JsError::new(format!(
                "XML 结构错误：标签 `<{name}>` 未闭合"
            )));
        }

        // 结束标签 `</name>`
        if chars.get(*position + 1) == Some(&'/') {
            let mut closing = String::new();
            *position += 2;
            while *position < chars.len() && chars[*position] != '>' {
                closing.push(chars[*position]);
                *position += 1;
            }
            *position += 1;
            let closing = closing.trim().to_string();
            if closing != name {
                return Err(JsError::new(format!(
                    "XML 结构错误：结束标签 `</{closing}>` 与起始标签 `<{name}>` 不匹配"
                )));
            }
            return Ok((name, children));
        }

        // 注释或声明出现在元素内部
        if chars.get(*position + 1) == Some(&'!') || chars.get(*position + 1) == Some(&'?') {
            skip_prologue(chars, position);
            continue;
        }

        // 嵌套元素
        let (nested_name, nested_children) = parse_element(chars, position, source)?;
        children.push(XmlNode::Element {
            name: nested_name,
            children: nested_children,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_解析容错_bom_与空白() {
        let value = parse_json("\u{feff}  {\"a\":1}  ").expect("应成功");
        assert_eq!(value.get("a").and_then(Value::as_i64), Some(1));
    }

    #[test]
    fn json_解析失败给出结构化错误() {
        let error = parse_json("{ 不是 json").expect_err("应报错");
        assert!(error.message().contains("JSON"));
    }

    #[test]
    fn xml_解析简单文档() {
        let json =
            parse_xml_to_json("<rss><channel><title>T</title></channel></rss>").expect("应成功");
        let value: Value = serde_json::from_str(&json).expect("应是合法 JSON");
        assert_eq!(
            value
                .get("rss")
                .and_then(|rss| rss.get("channel"))
                .and_then(|channel| channel.get("title"))
                .and_then(Value::as_str),
            Some("T")
        );
    }

    #[test]
    fn xml_同名子元素收成数组() {
        // RSS 的典型形态：多个 `<item>`。书源需要遍历它们，
        // 故必须是数组而不是"最后一个覆盖前面的"。
        let json = parse_xml_to_json(
            "<rss><channel><item><title>A</title></item><item><title>B</title></item></channel></rss>",
        )
        .expect("应成功");
        let value: Value = serde_json::from_str(&json).expect("应是合法 JSON");
        let items = value
            .get("rss")
            .and_then(|rss| rss.get("channel"))
            .and_then(|channel| channel.get("item"))
            .expect("应有 item");
        let array = items.as_array().expect("多个同名子元素应是数组");
        assert_eq!(array.len(), 2);
        assert_eq!(array[0].get("title").and_then(Value::as_str), Some("A"));
        assert_eq!(array[1].get("title").and_then(Value::as_str), Some("B"));
    }

    #[test]
    fn xml_单个子元素直接给值而非数组() {
        // 只出现一次时给标量：书源里 `x.title` 是最自然的取值方式，
        // 包成数组会逼它每处都写 `[0]`。
        let json = parse_xml_to_json("<a><b>x</b></a>").expect("应成功");
        let value: Value = serde_json::from_str(&json).expect("应是合法 JSON");
        assert_eq!(
            value
                .get("a")
                .and_then(|a| a.get("b"))
                .and_then(Value::as_str),
            Some("x")
        );
    }

    #[test]
    fn xml_忽略声明与注释() {
        let json = parse_xml_to_json("<?xml version=\"1.0\"?><!-- 注释 --><root><a>1</a></root>")
            .expect("应成功");
        assert!(json.contains("\"a\":\"1\""), "{json}");
    }

    #[test]
    fn xml_忽略属性() {
        // 属性不保留（见 `XmlNode::Element::name` 的说明），但**不能因此失败**。
        let json = parse_xml_to_json("<a href=\"x\" class=\"y\"><b>1</b></a>").expect("应成功");
        assert!(json.contains("\"b\":\"1\""), "{json}");
    }

    #[test]
    fn xml_自闭合标签() {
        let json = parse_xml_to_json("<a><b/><c>1</c></a>").expect("应成功");
        assert!(json.contains("\"c\":\"1\""), "{json}");
    }

    #[test]
    fn xml_结构错误报错而非猜() {
        for malformed in [
            "<a><b></a>", // 标签不匹配
            "<a>",        // 未闭合
            "",           // 空
            "<a><b>",     // 内层未闭合
        ] {
            assert!(
                parse_xml_to_json(malformed).is_err(),
                "畸形 XML {malformed:?} 应报错"
            );
        }
    }
}
