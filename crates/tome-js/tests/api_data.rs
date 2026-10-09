//! T039 验收测试：结构化数据类宿主 API 在 JS 侧可用（FR-005）
//!
//! ## 覆盖矩阵
//!
//! | 能力 | 测试 |
//! |---|---|
//! | JSON 解析与取值 | [`json_在_js_侧可用`]、[`json_路径取值可用`] |
//! | XML 解析与取值 | [`xml_在_js_侧可用`] |
//! | 网页标记解析与选择（经 [`MarkupHost`]） | [`网页选择经_markup_host_注入`] |
//!
//! ## 关于 `MarkupHost` 的那条测试
//!
//! tasks.md 要求"网页标记解析与选择——接 tome-rule DOM"，而 tome-js 不能
//! 依赖 tome-rule（架构铁律 A1）。故 trait 定义在 tome-js，由组合根注入
//! 实现。本文件的替身实现**模拟** tome-rule 的行为（按 `@css:tag` 取内容），
//! 以便在 tome-rule 尚未接线时验证这条接缝成立。
//!
//! 这里刻意用替身而不是真实 tome-rule：本测试的职责是"接缝形状正确"，
//! 真实规则引擎的行为由 tome-rule 自己的测试覆盖。混在一起会让
//! "DOM 选择器算错了"与"JS 桥没接上"这两种失败无法区分。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::sync::Arc;

use tome_js::api::data::{self, MarkupHost};
use tome_js::runtime::{Limits, Sandbox};
use tome_js::JsError;

/// 模拟 tome-rule 的网页选择替身。
///
/// ## 它模拟的是什么
///
/// 只实现测试需要的两种规则形态，足以验证接缝：
/// - `@css:TAG` → 取全部该标签的文本内容
/// - `@css:TAG@text` → 同上（后缀在本替身里无区别）
/// - 含 `exists` 字样 → 返回单个固定值（仅用于验证 `getElement`）
///
/// ## 为什么替身也要"按规则"而不是忽略规则参数
///
/// 若忽略 rule 参数直接返回固定值，那"JS 侧把规则传丢了"这类缺陷
/// 就永远不会被发现——而那正是这条接缝最容易出的问题。
struct StubMarkupHost {
    /// 记录收到的 (markup, rule) 对，供测试断言参数确实传到了
    seen: std::sync::Mutex<Vec<(String, String)>>,
}

impl StubMarkupHost {
    fn new() -> Self {
        Self {
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn seen(&self) -> Vec<(String, String)> {
        self.seen.lock().expect("锁未被毒化").clone()
    }
}

impl MarkupHost for StubMarkupHost {
    fn select(&self, markup: &str, rule: &str) -> Result<Vec<String>, JsError> {
        self.seen
            .lock()
            .expect("锁未被毒化")
            .push((markup.to_string(), rule.to_string()));

        if rule.contains("boom") {
            return Err(JsError::new("规则求值失败：这是个坏选择器"));
        }
        if rule.contains("none") {
            return Ok(Vec::new());
        }
        // 极简"选择"：把 `@css:tag` 里的 tag 取出来，匹配 `<tag>...</tag>`
        let tag = rule
            .trim_start_matches("@css:")
            .split(['@', '>', ' '])
            .next()
            .unwrap_or("")
            .trim();
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let mut results = Vec::new();
        let mut rest = markup;
        while let Some(start) = rest.find(&open) {
            let after = &rest[start + open.len()..];
            let Some(end) = after.find(&close) else { break };
            results.push(after[..end].to_string());
            rest = &after[end + close.len()..];
        }
        Ok(results)
    }
}

/// 建一个已注册结构化数据类 API 的沙箱（带网页选择能力）。
fn sandbox_with_data_api() -> (Sandbox, Arc<StubMarkupHost>) {
    let host = Arc::new(StubMarkupHost::new());
    let sandbox = build_sandbox(Some(host.clone() as Arc<dyn MarkupHost>));
    (sandbox, host)
}

/// 建一个已注册结构化数据类 API 的沙箱（**不带**网页选择能力）。
fn sandbox_without_markup() -> Sandbox {
    build_sandbox(None)
}

fn build_sandbox(markup: Option<Arc<dyn MarkupHost>>) -> Sandbox {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let count = sandbox
        .with_context(move |ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            let count = data::register(&ctx, &java, markup)
                .map_err(|error| format!("注册 data 失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(count)
        })
        .expect("应能装配");
    assert!(count >= 6, "本类应注册至少 6 个函数，实际 {count}");
    sandbox
}

/// 求值并取回文本。
fn eval_text(sandbox: &Sandbox, script: &str) -> String {
    sandbox
        .eval(script)
        .unwrap_or_else(|error| panic!("脚本应成功，实际 {error:?}"))
        .text
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

#[test]
fn json_在_js_侧可用() {
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      var data = java.jsonParse('{"name":"斗破","list":[1,2,3]}');
      data.name + "/" + data.list.length;
    "#;
    assert_eq!(eval_text(&sandbox, script), "斗破/3");
}

#[test]
fn json_解析容错_bom_与空白() {
    // 书源从响应里截出的 JSON 常带 BOM 或首尾空白，原生 JSON.parse 会拒绝。
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      var data = java.jsonParse('\uFEFF  {"ok":true}  ');
      String(data.ok);
    "#;
    assert_eq!(eval_text(&sandbox, script), "true");
}

#[test]
fn json_解析失败抛出可捕获错误() {
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      try { java.jsonParse("{ 不是 json"); "未抛出"; }
      catch (e) { "已捕获"; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "已捕获");
}

#[test]
fn json_路径取值可用() {
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      var text = '{"data":{"list":[{"name":"A"},{"name":"B"}]}}';
      java.jsonPath(text, "data.list[1].name") + "|" + java.jsonPath(text, "data.list[0].name");
    "#;
    assert_eq!(eval_text(&sandbox, script), "B|A");
}

#[test]
fn json_路径支持任意_js_表达式() {
    // 书源的"路径"里可以出现任意 JS。自己解析路径要重实现半个表达式
    // 求值器，故交给引擎——这条测试钉住那个决定。
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      var text = '{"list":[{"id":1},{"id":5},{"id":3}]}';
      java.jsonPath(text, "list.filter(function(x){ return x.id > 2; })[1].id");
    "#;
    assert_eq!(eval_text(&sandbox, script), "3");
}

#[test]
fn json_路径不存在时返回_undefined_而不报错() {
    // 接口返回结构随业务变化是常态：中间节点不存在**不是书源写错了**。
    // 报错会让整段脚本中断，而 undefined 让书源继续（后续判空会处理）。
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      var value = java.jsonPath('{"a":1}', "b.c.d");
      typeof value;
    "#;
    assert_eq!(eval_text(&sandbox, script), "undefined");
}

#[test]
fn json_路径求值不留全局临时变量() {
    // 实现上要临时放一个全局根（见 data.rs 的说明）。无论成败都必须清理
    // ——书源可能遍历全局对象，残留的键会污染它的视图。
    let (sandbox, _host) = sandbox_with_data_api();
    // 成功的路径求值
    let _ = eval_text(
        &sandbox,
        r#"java.jsonPath('{"a":{"b":1}}', "a.b"); "done";"#,
    );
    // 失败的路径求值
    let _ = eval_text(&sandbox, r#"java.jsonPath('{"a":1}', "x.y.z"); "done";"#);
    assert_eq!(
        eval_text(&sandbox, "typeof __tome_json_root"),
        "undefined",
        "临时全局变量必须被清理"
    );
}

#[test]
fn 序列化可用() {
    let (sandbox, _host) = sandbox_with_data_api();
    assert_eq!(
        eval_text(&sandbox, r#"java.toJson({a:1,b:'x'})"#),
        r#"{"a":1,"b":"x"}"#
    );
}

// ---------------------------------------------------------------------------
// XML
// ---------------------------------------------------------------------------

#[test]
fn xml_在_js_侧可用() {
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      var xml = '<?xml version="1.0"?><rss><channel><title>我的订阅</title></channel></rss>';
      var data = java.jsonParse(java.xmlParse(xml));
      data.rss.channel.title;
    "#;
    assert_eq!(eval_text(&sandbox, script), "我的订阅");
}

#[test]
fn xml_多个同名子元素可遍历() {
    // RSS 的典型形态：多个 `<item>`。书源需要遍历它们。
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      var xml = '<rss><channel>' +
                '<item><title>A</title></item>' +
                '<item><title>B</title></item>' +
                '</channel></rss>';
      var data = java.jsonParse(java.xmlParse(xml));
      var items = data.rss.channel.item;
      items.length + ":" + items[0].title + items[1].title;
    "#;
    assert_eq!(eval_text(&sandbox, script), "2:AB");
}

#[test]
fn xml_畸形输入抛出可捕获错误() {
    let (sandbox, _host) = sandbox_with_data_api();
    let script = r#"
      try { java.xmlParse("<a><b></a>"); "未抛出"; }
      catch (e) { "已捕获"; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "已捕获");
}

// ---------------------------------------------------------------------------
// 网页标记（经 MarkupHost）
// ---------------------------------------------------------------------------

#[test]
fn 网页选择经_markup_host_注入() {
    let (sandbox, host) = sandbox_with_data_api();
    let html = r#"<div><h3>书名一</h3><h3>书名二</h3></div>"#;
    let script = format!(
        r#"java.getElements({html}, "@css:h3@text").join("|")"#,
        html = js_string(html)
    );
    assert_eq!(eval_text(&sandbox, &script), "书名一|书名二");

    // 关键断言：规则原文必须**原样**传到宿主。
    // 若 JS 侧把规则解析过一遍再传（比如剥掉 `@css:` 前缀），
    // 真实的 tome-rule 收到的就不是书源原文，兼容性会静默改变。
    let seen = host.seen();
    assert_eq!(seen.len(), 1, "宿主应收到一次选择调用，实际 {seen:?}");
    assert_eq!(
        seen[0].1, "@css:h3@text",
        "规则原文必须原样传递（不得预解析）"
    );
    assert!(seen[0].0.contains("书名一"), "标记原文也必须原样传递");
}

#[test]
fn 网页选择取首个可用() {
    let (sandbox, _host) = sandbox_with_data_api();
    let html = "<div><h3>第一</h3><h3>第二</h3></div>";
    let script = format!(
        r#"java.getElement({html}, "@css:h3")"#,
        html = js_string(html)
    );
    assert_eq!(eval_text(&sandbox, &script), "第一");
}

#[test]
fn 网页选择无匹配时返回空() {
    let (sandbox, _host) = sandbox_with_data_api();
    let script = format!(
        r#"java.getElement({html}, "@css:none-here").length"#,
        html = js_string("<div></div>")
    );
    // 无匹配返回空串（而非 undefined）——书源里判空更少陷阱
    assert_eq!(eval_text(&sandbox, &script), "0");
}

#[test]
fn 宿主选择失败抛出可捕获错误() {
    let (sandbox, _host) = sandbox_with_data_api();
    let script = format!(
        r#"
        try {{ java.getElements({html}, "@css:boom"); "未抛出"; }}
        catch (e) {{ e.message; }}
        "#,
        html = js_string("<div></div>")
    );
    let message = eval_text(&sandbox, &script);
    assert!(
        message.contains("坏选择器"),
        "宿主求值失败的说明必须原样传到 JS 异常（不得被吞成空列表），实际：{message}"
    );
}

#[test]
fn 未注入_markup_host_时抛结构化能力错误() {
    // 组合根没注入时，入口仍应存在并抛**可捕获的能力错误**（FR-006），
    // 而不是"函数不存在"（ReferenceError 会让书源无法降级）。
    let sandbox = sandbox_without_markup();
    let script = format!(
        r#"
        try {{ java.getElements({html}, "@css:h3"); "未抛出"; }}
        catch (e) {{ e.capability; }}
        "#,
        html = js_string("<h3>x</h3>")
    );
    assert_eq!(
        eval_text(&sandbox, &script),
        "markup",
        "应抛带 capability 的能力错误"
    );
}

// ---------------------------------------------------------------------------
// 注册完整性
// ---------------------------------------------------------------------------

#[test]
fn 全部结构化数据类函数都已注册() {
    let (sandbox, _host) = sandbox_with_data_api();
    for name in [
        "jsonParse",
        "toJson",
        "jsonPath",
        "getElements",
        "getElement",
        "xmlParse",
    ] {
        assert_eq!(
            eval_text(&sandbox, &format!("typeof java.{name}")),
            "function",
            "`java.{name}` 应是已注册的函数"
        );
    }
}

/// 把一个 Rust 字符串编成 JS 字符串字面量。
fn js_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            other => output.push(other),
        }
    }
    output.push('"');
    output
}
