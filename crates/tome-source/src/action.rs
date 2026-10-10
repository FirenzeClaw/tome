//! \[DONE] 五类动作的公共骨架（tasks.md T065-T067 的基础设施）
//!
//! # 为什么需要这一层
//!
//! 搜索（T027）、详情（T065）、目录（T066）、正文（T067）、发现（T072）
//! 走的是同一条路：
//!
//! ```text
//!   ① 规则求值 → RequestSpec（不发请求，A2 纯化）
//!   ② 执行请求（唯一发请求处）
//!   ③ 判状态码
//!   ④ 解码（容错，FR-009）
//!   ⑤ 规则求值 → 结构化结果
//! ```
//!
//! 五份各自实现一遍，第 ③ 步（状态码判定）与第 ④ 步（解码）必然写出
//! 五份略有差异的版本——而它们的差异不会有人注意到，直到某个源的
//! GBK 页面在"详情"能读、在"正文"是乱码。
//!
//! 故把 ②③④ 抽到这里，各动作只实现自己的 ① 与 ⑤。
//!
//! # 状态码判定的统一口径
//!
//! 接缝按 R3 对"事务完成但状态非 2xx"返回 `Ok(Response)`（错误分类权
//! 在调用方）。本模块统一判定为 [`ErrorKind::Network`]，理由：
//!
//! - 书源动作**期望拿到内容**。404 的 HTML 页拿去跑 CSS 规则会产出
//!   空结果，表现为"这本书没目录"——而真相是源改版了。两者对用户的
//!   含义完全不同。
//! - 需要读非 2xx 正文的场景（`check` 脚本验 403 验证页）走的是
//!   [`tome_net::HostRequest::send`]，不经过本模块。

use tome_core::{ErrorKind, NetHost, Response};

use crate::error::SourceError;
use crate::source::ParsedSource;

/// 抓取超时（毫秒）。
///
/// 与 `search::SEARCH_TIMEOUT_MS`、`tome_net::SCRIPT_REQUEST_TIMEOUT_MS`
/// 取同一个值（15 秒）：三者都是"书源动作"级别的超时，取不同值只会让
/// "为什么这个慢、那个快"变成一个没有依据的谜。
///
/// \[UNCERTAIN] 大目录页（5 万章）在慢站点上可能超过 15 秒。SC-003 的
/// 口径是"解析"而非"下载"，故此处不影响该验收项；但若实测发现真实大站
/// 普遍超时，应改为可配置项而非继续调大这个常量。
pub const ACTION_TIMEOUT_MS: u64 = 15_000;

/// 执行一次请求并返回**已解码**的正文。
///
/// # 三步都在这里：执行 → 判状态 → 解码
///
/// 见模块文档。任何一步单独散落出去，都会让"某个动作的解码方式与别的
/// 不同"成为可能。
pub fn fetch_text(
    host: &dyn NetHost,
    spec: tome_core::RequestSpec,
    context: &str,
) -> Result<(String, Response), SourceError> {
    let response = host
        .fetch(spec)
        .map_err(|kind| SourceError::Network { kind })?;

    // R3 之后接缝对非 2xx 返回 `Ok`，故这里自己判：
    // 拿一个 404 页面去跑规则会产出空结果，而"空结果"与"源改版了"
    // 对用户的含义完全不同，必须分开报。
    if !response.is_success() {
        return Err(SourceError::Network {
            kind: ErrorKind::Network,
        });
    }

    let text = decode(&response);
    let _ = context;
    Ok((text, response))
}

/// 容错解码响应体（FR-009：声明与实际不符时须容错）。
///
/// 委托给 `tome-net` 的 charset 层——本模块不重写解码。这条委托是必要的：
/// charset 层处理的是"GBK 页面被声明成 UTF-8"这类真实场景，重写一遍
/// 只会得到一份更差的实现。
pub fn decode(response: &Response) -> String {
    // 优先用检测出的字符集（`detected_charset`），回落到声明头。
    // 顺序不能反：检测是对**本次响应的实际字节**做的，而声明头可能是
    // 源站写错的——那正是 FR-009 要容错的场景。
    let declared = response
        .detected_charset
        .as_deref()
        .map(str::to_string)
        .or_else(|| response.headers.get("content-type").map(str::to_string));
    tome_net::charset::decode_body(&response.body_bytes, declared.as_deref()).text
}

/// 从书源构造一个请求的基本骨架（头、字符集、间隔、超时）。
///
/// # 为什么单独一个函数
///
/// 五类动作的请求**共享**这四项定制（FR-008）。散在五处的话，
/// "新加一个请求头"要改五个地方，而漏掉的那一处表现为"某个动作的请求
/// 被源站拒绝"——一个很难归因的现象。
pub fn base_spec(
    parsed: &ParsedSource,
    url: impl Into<String>,
) -> Result<tome_core::RequestSpec, SourceError> {
    let mut spec = tome_core::RequestSpec::get(url, &parsed.source.id);
    spec.headers = crate::source::parse_headers(parsed.source.header.as_deref())?;
    spec.charset = parsed.source.charset.clone();
    spec.request_interval_ms = crate::search::interval_from_source(&parsed.source);
    spec.timeout_ms = ACTION_TIMEOUT_MS;
    Ok(spec)
}

/// 把相对 URL 补全为绝对地址（公开文档：动作地址支持相对 URL）。
///
/// 转发给 `search` 的实现——两处各写一份会让"相对 URL 的基准是什么"
/// 出现两个答案。
pub fn absolute_url(base: &str, url: &str) -> String {
    crate::search::resolve_relative(base, url)
}

/// 解析一个 JSON 对象形态的规则字段（`ruleBookInfo` / `ruleToc` / …）。
///
/// # 与 `search::search_rules` 的关系
///
/// 那个函数专用于 `ruleSearch`（它有自己的字段名常量与"必须含 bookList"
/// 的额外校验）。本函数是通用的：给定字段名与规则文本，产出 `字段名 → 规则`。
///
/// ## 为什么容忍两种形态
///
/// 与书源加载器同理（见 `source.rs`）：社区流通格式把规则写成**转义后的
/// JSON 文本**，而手工构造时更自然是**对象**。两种都接受，归一到字符串值。
pub fn parse_rule_object(
    parsed: &ParsedSource,
    field_name: &str,
    text: Option<&str>,
) -> Result<std::collections::BTreeMap<String, String>, SourceError> {
    let id = &parsed.source.id;
    let Some(text) = text else {
        return Err(SourceError::incomplete(
            id.clone(),
            format!("缺少 {field_name}"),
        ));
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(SourceError::incomplete(
            id.clone(),
            format!("{field_name} 为空"),
        ));
    }

    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|error| SourceError::rule(format!("{field_name} 不是合法 JSON（{error}）")))?;
    let Some(object) = value.as_object() else {
        return Err(SourceError::rule(format!(
            "{field_name} 应是 JSON 对象（字段名 → 规则），实际是 {}",
            json_kind(&value)
        )));
    };

    let mut rules = std::collections::BTreeMap::new();
    for (key, rule) in object {
        // 规则值可以是字符串或对象（后者少见但存在），统一成字符串
        let text = match rule {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        if !text.trim().is_empty() {
            rules.insert(key.clone(), text);
        }
    }
    if rules.is_empty() {
        return Err(SourceError::incomplete(
            id.clone(),
            format!("{field_name} 里没有任何规则"),
        ));
    }
    Ok(rules)
}

/// JSON 值的种类名（错误消息用）。
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "布尔值",
        serde_json::Value::Number(_) => "数字",
        serde_json::Value::String(_) => "字符串",
        serde_json::Value::Array(_) => "数组",
        serde_json::Value::Object(_) => "对象",
    }
}

/// 从规则集合里取一个字段并求值（在指定节点内）。
///
/// # 为什么返回 `Option` 而不是空串
///
/// 与 `search::evaluate_field_in` 同理：`None` 区分"规则没写"与
/// "写了但没命中"。前者是书源作者的省略（合法），后者是选择器问题
/// （值得报出来）——两者混成一个空串就都看不见了。
pub fn evaluate_field(
    engine: &tome_rule::css_post::RuleEngine<'_>,
    node: &tome_rule::dom::NodeRef,
    rules: &std::collections::BTreeMap<String, String>,
    key: &str,
) -> Result<Option<String>, SourceError> {
    let Some(rule) = rules.get(key) else {
        return Ok(None);
    };
    let values = engine
        .evaluate_in(node, rule)
        .map_err(|error| SourceError::rule_eval(&error, &format!("字段 `{key}`")))?;
    // 多值时取第一个（与搜索同口径）
    Ok(values.into_iter().next().and_then(non_empty))
}

/// 空串（或纯空白）归一为 `None`。
pub fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 按节点规则切出子树列表（每个元素是"一条记录"）。
///
/// # 它是目录/详情/发现三类动作的公共第一步
///
/// 三者的规则都是"先用一个列表规则切出一组节点，再在每个节点内相对求值
/// 各字段"（与搜索的 `bookList` 同构）。差别只在列表规则的字段名与
/// 后续字段的集合。抽出这一步让"相对求值"这条语义只有一处定义。
pub fn select_nodes(
    document: &tome_rule::dom::Document,
    source_id: &str,
    rules: &std::collections::BTreeMap<String, String>,
    list_key: &str,
) -> Result<Vec<tome_rule::dom::NodeRef>, SourceError> {
    let Some(list_rule) = rules.get(list_key) else {
        return Err(SourceError::incomplete(
            source_id.to_string(),
            format!("缺少列表规则 `{list_key}`"),
        ));
    };
    // 委托给 `tome_rule::css_post::select_nodes`——它是与搜索的 `bookList`
    // 同一套实现。本模块不重写它，否则"目录怎么切节点"会长出第二个答案。
    tome_rule::css_post::select_nodes(document, list_rule)
        .map_err(|error| SourceError::rule(format!("列表规则 `{list_key}` 求值失败：{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::parse_sources;

    fn parsed(text: &str) -> ParsedSource {
        parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    #[test]
    fn 基础请求携带全部定制项() {
        let source = parsed(
            r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                "header":"{\"User-Agent\":\"x\"}","charset":"gbk",
                "concurrentRate":"1500"}"#,
        );
        let spec = base_spec(&source, "https://a.com/x").expect("应成功");
        assert_eq!(spec.headers.get("User-Agent"), Some("x"));
        assert_eq!(spec.charset.as_deref(), Some("gbk"));
        assert_eq!(spec.request_interval_ms, Some(1500));
        assert_eq!(spec.timeout_ms, ACTION_TIMEOUT_MS);
        // 限速键必须是书源 id（FR-008 的"按源"限速）
        assert_eq!(spec.rate_limit_key, "https://a.com");
    }

    #[test]
    fn 无头无字符集时也成立() {
        let source = parsed(r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#);
        let spec = base_spec(&source, "https://a.com/x").expect("应成功");
        assert!(spec.headers.is_empty());
        assert_eq!(spec.charset, None);
    }

    #[test]
    fn 相对_url_补全() {
        assert_eq!(
            absolute_url("https://a.com", "/book/1"),
            "https://a.com/book/1"
        );
        assert_eq!(
            absolute_url("https://a.com/", "book/1"),
            "https://a.com/book/1"
        );
        // 已是绝对地址则原样返回
        assert_eq!(
            absolute_url("https://a.com", "https://b.com/x"),
            "https://b.com/x"
        );
    }

    #[test]
    fn 规则对象解析_字符串与对象两形态都接受() {
        let source = parsed(r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#);
        let rules = parse_rule_object(
            &source,
            "ruleToc",
            Some(r#"{"chapterList":"@css:.c","chapterName":"@css:a@text"}"#),
        )
        .expect("应成功");
        assert_eq!(
            rules.get("chapterList").map(String::as_str),
            Some("@css:.c")
        );
        assert_eq!(
            rules.get("chapterName").map(String::as_str),
            Some("@css:a@text")
        );
    }

    #[test]
    fn 规则对象缺失时报配置不完整() {
        let source = parsed(r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#);
        let error = parse_rule_object(&source, "ruleToc", None).expect_err("应报错");
        assert!(
            error.to_string().contains("ruleToc"),
            "错误应指出缺哪个字段：{error}"
        );
    }

    #[test]
    fn 规则对象非_json_报规则错误() {
        let source = parsed(r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#);
        let error = parse_rule_object(&source, "ruleToc", Some("{不是 JSON")).expect_err("应报错");
        assert!(error.to_string().contains("ruleToc"), "{error}");
    }

    #[test]
    fn 规则对象是数组时报错() {
        let source = parsed(r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#);
        let error = parse_rule_object(&source, "ruleToc", Some("[1,2,3]")).expect_err("应报错");
        assert!(
            error.to_string().contains("数组"),
            "应指出实际类型：{error}"
        );
    }

    #[test]
    fn 空规则对象报不完整() {
        let source = parsed(r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com"}"#);
        let error = parse_rule_object(&source, "ruleToc", Some("{}")).expect_err("应报错");
        assert!(error.to_string().contains("没有任何规则"), "{error}");
    }

    #[test]
    fn 非_2xx_报网络错误() {
        struct Host(u16);
        impl NetHost for Host {
            fn fetch(&self, _spec: tome_core::RequestSpec) -> Result<Response, ErrorKind> {
                Ok(Response {
                    status: self.0,
                    headers: tome_core::HeaderMap::new(),
                    body_bytes: b"not found".to_vec(),
                    detected_charset: None,
                })
            }
        }
        for status in [404u16, 403, 500] {
            let error = fetch_text(
                &Host(status),
                tome_core::RequestSpec::get("https://a.com", "s"),
                "详情",
            )
            .expect_err("非 2xx 应报错");
            assert_eq!(
                error.to_error_kind(),
                ErrorKind::Network,
                "状态 {status} 应归网络错误"
            );
        }
        // 2xx 不是错误
        fetch_text(
            &Host(200),
            tome_core::RequestSpec::get("https://a.com", "s"),
            "详情",
        )
        .expect("2xx 应成功");
    }

    #[test]
    fn 传输失败原样传播分类() {
        struct Failing;
        impl NetHost for Failing {
            fn fetch(&self, _spec: tome_core::RequestSpec) -> Result<Response, ErrorKind> {
                Err(ErrorKind::Proxy)
            }
        }
        let error = fetch_text(&Failing, tome_core::RequestSpec::get("u", "s"), "详情")
            .expect_err("应报错");
        assert_eq!(
            error.to_error_kind(),
            ErrorKind::Proxy,
            "代理故障不得被归成网络故障（FR-008 要求两者可区分）"
        );
    }

    #[test]
    fn 解码用检测值而非声明头() {
        // FR-009：声明与实际不符时须容错。检测值（`detected_charset`）
        // 是对本次响应字节做的，声明头可能是源站写错的。
        let body = "中文".as_bytes().to_vec();
        let mut headers = tome_core::HeaderMap::new();
        headers.set("content-type", "text/html; charset=gbk");
        let response = Response {
            status: 200,
            headers,
            body_bytes: body,
            // 检测值是 utf-8（正确），声明头说 gbk（错误）
            detected_charset: Some("utf-8".to_string()),
        };
        assert_eq!(decode(&response), "中文");
    }

    #[test]
    fn 空串归一为_none() {
        assert_eq!(non_empty("  ".to_string()), None);
        assert_eq!(non_empty("".to_string()), None);
        assert_eq!(non_empty(" x ".to_string()), Some("x".to_string()));
    }
}
