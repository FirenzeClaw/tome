//! T043 验收测试：`@js:` 内联脚本 + `{{}}` 插值 + `@webjs`（FR-005）
//!
//! ## 这条链路的验收点
//!
//! ```text
//! 规则文本 "q={{key}}&t={{1+1}}"
//!   → parse_interpolation（T025，切占位）
//!   → JsPlaceholderBridge（本模块）
//!   → JobRuntime（T030 的沙箱，含 T035 的 prelude）
//!   → 拼回字符串
//! ```
//!
//! ## 为什么两类片段都要测
//!
//! `@js:` 的片段有两种形态，**两类在真实书源里都大量存在**：
//!
//! | 形态 | 例子 | 若只支持另一类会怎样 |
//! |---|---|---|
//! | 表达式 | `@js:page - 1` | —— |
//! | 语句块 | `@js:var x = 1; x + 1` | 语法错（`var` 不是表达式） |
//!
//! 反过来若只支持语句块（包成函数体），表达式形态会**静默返回
//! `undefined`**（没有 `return` 语句）——那是最危险的一类失效：
//! 不报错，但 URL 里少了一个参数。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::time::Duration;

use tome_js::{JsContext, Limits};
use tome_rule::js_bridge::{is_webjs_prefix, wrap_inline_script, JobRuntime, JsPlaceholderBridge};
use tome_rule::replace::{parse_interpolation, PlaceholderBridge};

/// 建一个可用的规则运行时（带前导脚本与最小宿主桥）。
fn runtime() -> JobRuntime {
    let mut runtime = JobRuntime::new(Limits {
        wall_clock_timeout: Some(Duration::from_secs(5)),
        ..Limits::default()
    })
    .expect("应能建运行时");
    runtime
        .install_bridge(tome_js::HostBridge::bare(JsContext::for_source(
            "https://s.test",
        )))
        .expect("应能注入宿主桥");
    runtime
}

/// 在一段空文档上求值一个 `{{}}` 表达式。
fn interpolate(runtime: &JobRuntime, template: &str) -> String {
    let document = tome_rule::dom::Document::parse_html("<p>x</p>");
    let interpolation = parse_interpolation(template);
    let bridge = JsPlaceholderBridge::new(runtime, &document);
    interpolation
        .evaluate(&bridge)
        .unwrap_or_else(|error| panic!("插值应成功，实际 {error}"))
}

// ---------------------------------------------------------------------------
// `@js:` 的两类片段形态
// ---------------------------------------------------------------------------

#[test]
fn 表达式形态的片段求值正确() {
    // 书源里最常见的写法：`@js:key`、`@js:page-1`
    let runtime = runtime();
    for (script, expected) in [
        ("1+1", "2"),
        ("page - 1", "undefined"), // 未定义变量 → 非报错路径单独测（见下）
        ("'abc'.toUpperCase()", "ABC"),
        ("[1,2,3].join('-')", "1-2-3"),
    ] {
        let wrapped = wrap_inline_script(script);
        let outcome = runtime.eval(&wrapped);
        if script == "page - 1" {
            // 未定义变量会抛 ReferenceError → 兜底路径也失败 →
            // 整体报错。这是**正确**行为（而不是静默 undefined）。
            assert!(outcome.is_err(), "未定义变量应报错，实际 {outcome:?}");
            continue;
        }
        assert_eq!(
            outcome
                .unwrap_or_else(|error| panic!("`{script}` 应成功：{error}"))
                .text,
            expected,
            "片段 `{script}`"
        );
    }
}

#[test]
fn 语句块形态的片段求值正确() {
    // 第二类写法：`@js:var x=1; x+1`。若只按表达式求值会语法错。
    let runtime = runtime();
    for (script, expected) in [
        ("var x = 1; x + 1", "2"),
        ("var a = 'x'; a + 'y';", "xy"),
        ("if (true) { 'yes' } else { 'no' }", "yes"),
    ] {
        let wrapped = wrap_inline_script(script);
        let outcome = runtime
            .eval(&wrapped)
            .unwrap_or_else(|error| panic!("`{script}` 应成功：{error}"));
        assert_eq!(outcome.text, expected, "片段 `{script}`");
    }
}

#[test]
fn 表达式形态不因包装而丢失返回值() {
    // ## 这是包装逻辑最要紧的一条
    //
    // 若把片段统一包成 `(function(){ <片段> })()`，表达式形态会因为没有
    // `return` 而得到 `undefined`——**不报错但结果错**。
    // 那是最危险的一类失效：URL 里少一个参数，而书源作者看不出原因。
    let runtime = runtime();
    assert_eq!(
        runtime
            .eval(&wrap_inline_script("42"))
            .expect("应成功")
            .text,
        "42",
        "纯数字表达式必须返回它自己"
    );
    assert_eq!(
        runtime
            .eval(&wrap_inline_script("'key'"))
            .expect("应成功")
            .text,
        "key"
    );
}

#[test]
fn 片段的类型被保留() {
    // 值形态由 `JsValue::kind` 保真（上层需要区分字符串与数字）。
    let runtime = runtime();
    let number = runtime.eval(&wrap_inline_script("1+1")).expect("应成功");
    assert_eq!(number.kind, "number");

    let text = runtime.eval(&wrap_inline_script("'a'")).expect("应成功");
    assert_eq!(text.kind, "string");

    let boolean = runtime.eval(&wrap_inline_script("1 < 2")).expect("应成功");
    assert_eq!(boolean.kind, "boolean");
}

#[test]
fn 片段里的语法错报出可读错误() {
    let runtime = runtime();
    let outcome = runtime.eval(&wrap_inline_script("function ( { [ }"));
    let error = match outcome {
        Ok(value) => panic!("语法错应报错，实际成功并返回 {value:?}"),
        Err(error) => error,
    };
    assert!(
        matches!(
            error,
            tome_js::SandboxError::Script { .. } | tome_js::SandboxError::Interrupted { .. }
        ),
        "应是脚本级错误，实际 {error:?}"
    );
}

// ---------------------------------------------------------------------------
// `{{}}` 插值
// ---------------------------------------------------------------------------

#[test]
fn 简单占位被求值() {
    let runtime = runtime();
    // 公开文档：「在 {{}} 里只能使用 js」——无前缀的按 JS 处理
    assert_eq!(interpolate(&runtime, "{{1+1}}"), "2");
    assert_eq!(interpolate(&runtime, "a{{'b'}}c"), "abc");
}

#[test]
fn 搜索_url_的典型形态() {
    // 公开文档给的示例形态：`https://x/dramaapi/search?s={{key}}&page=1`
    let runtime = runtime();
    // `key` 未定义会报错（正确行为）——故用字面量模拟真实可用场景
    assert_eq!(
        interpolate(&runtime, "https://x/s?q={{'斗破'}}&t={{1+1}}"),
        "https://x/s?q=斗破&t=2"
    );
}

#[test]
fn 公开文档的表达式形态可用() {
    // 公开文档示例：`{{(page-1)*20}}` 与 `{{page - 1 == 0 ? "": page}}`
    let runtime = runtime();
    // 用 `var` 声明模拟页面上下文的变量（真实场景由组合根注入）
    assert_eq!(
        interpolate(&runtime, "{{(2-1)*20}}"),
        "20",
        "算术表达式应求值"
    );
    assert_eq!(
        interpolate(&runtime, r#"{{2 - 1 == 0 ? "": "第2页"}}"#),
        "第2页"
    );
}

#[test]
fn 多个占位按序求值并拼接() {
    let runtime = runtime();
    assert_eq!(interpolate(&runtime, "{{1}}{{2}}{{3}}"), "123");
    assert_eq!(interpolate(&runtime, "a{{1}}b{{2}}c"), "a1b2c");
}

#[test]
fn 规则类占位仍走规则引擎() {
    // `{{}}` 里也可以显式写规则（公开文档：「使用其他规则需要有明显的标志头」）。
    // 这条验证桥同时服务两类——js 类走沙箱、规则类走 CSS 引擎。
    let runtime = runtime();
    let document = tome_rule::dom::Document::parse_html(r#"<div class="t">标题</div>"#);
    let interpolation = parse_interpolation("[{{@css:.t@text}}]");
    let bridge = JsPlaceholderBridge::new(&runtime, &document);
    assert_eq!(interpolation.evaluate(&bridge).expect("应成功"), "[标题]");
}

#[test]
fn 无占位时原样返回() {
    let runtime = runtime();
    assert_eq!(
        interpolate(&runtime, "https://x/s?q=fixed"),
        "https://x/s?q=fixed"
    );
}

#[test]
fn 占位的未定义变量报错而不是静默空串() {
    // 「没被替换」比「静默产出空串」容易定位——错误消息会指向
    // 那个未定义的变量名。
    let runtime = runtime();
    let document = tome_rule::dom::Document::parse_html("<p>x</p>");
    let interpolation = parse_interpolation("{{undefinedVariable}}");
    let bridge = JsPlaceholderBridge::new(&runtime, &document);
    assert!(
        interpolation.evaluate(&bridge).is_err(),
        "未定义变量应报错，而不是静默产出空串"
    );
}

#[test]
fn 插值结果可含特殊字符而不破坏拼接() {
    // 值里含引号/反斜杠时不得破坏结果（包装时的转义必须正确）。
    let runtime = runtime();
    assert_eq!(
        interpolate(&runtime, r#"{{'a"b'}}|{{'c\\d'}}"#),
        r#"a"b|c\d"#
    );
}

// ---------------------------------------------------------------------------
// `@webjs`（明确不支持）
// ---------------------------------------------------------------------------

#[test]
fn webjs_前缀被识别并报能力错误() {
    // 见 js_bridge 的模块文档第 3 条：`@webjs` 依赖内嵌浏览器，
    // 而本产品不含浏览器内核。必须抛结构化能力错误让书源降级。
    assert!(is_webjs_prefix("@webjs:java.webView('x')"));

    let error = tome_rule::js_bridge::webjs_unsupported_error("rule");
    match error.to_error_kind() {
        tome_core::ErrorKind::UnsupportedCapability { capability } => {
            assert_eq!(
                capability.capability_id(),
                "webView",
                "能力标识必须是书源生态的通用叫法（兼容性关键）"
            );
        }
        other => panic!("应报能力错误，实际 {other:?}"),
    }
}

#[test]
fn 书源可对_webjs_降级() {
    // FR-006 的目的：书源的 try/catch 能据能力标识走备选路径。
    let error = tome_rule::js_bridge::webjs_unsupported_error("search");
    let kind = error.to_error_kind();
    let capability = match kind {
        tome_core::ErrorKind::UnsupportedCapability { capability } => capability,
        other => panic!("应是能力错误，实际 {other:?}"),
    };
    // 书源里的判断形态：`if (e.capability === "webView") { 走纯 HTTP }`
    assert_eq!(capability.capability_id(), "webView");
}

#[test]
fn 普通_js_前缀不被误判为_webjs() {
    assert!(!is_webjs_prefix("@js:java.webView('x')"));
    // 脚本体里 *调用* webView 是运行时能力问题（由 tome-js 的桩处理），
    // 不是前缀形态问题——本层不应误判。
}

// ---------------------------------------------------------------------------
// 沙箱护栏在规则求值路径上同样生效
// ---------------------------------------------------------------------------

#[test]
fn 死循环片段被中断() {
    // 规则里的 `@js:while(true){}` 同样必须能停——FR-007 不区分
    // 片段是来自规则还是来自 mainJs。
    let mut runtime = JobRuntime::new(Limits {
        wall_clock_timeout: Some(Duration::from_millis(300)),
        ..Limits::default()
    })
    .expect("应能建运行时");
    runtime
        .install_bridge(tome_js::HostBridge::bare(JsContext::for_source("s")))
        .expect("应能注入");

    let outcome = runtime.eval(&wrap_inline_script("while(true){}"));
    assert!(
        matches!(outcome, Err(tome_js::SandboxError::Interrupted { .. })),
        "死循环片段应被中断，实际 {outcome:?}"
    );
}

#[test]
fn 前导脚本在规则运行时里已注入() {
    // 书源的 `@js:` 片段会引用 `java` 之类的宿主全局；缺了 prelude
    // 会得到 ReferenceError（而那个信息让作者以为是自己的写法问题）。
    let runtime = runtime();
    // `Packages` 是 prelude 放的桩——它存在说明 prelude 已注入。
    assert_eq!(
        runtime
            .eval(&wrap_inline_script("typeof Packages"))
            .expect("应成功")
            .text,
        "function",
        "前导脚本应已注入（否则书源的 @js: 片段会 ReferenceError）"
    );
}

#[test]
fn 编排的_runtime_配置可诊断() {
    let runtime = runtime();
    assert!(
        runtime.describe().contains("HostBridge"),
        "配置摘要应可读：{}",
        runtime.describe()
    );
    assert!(!runtime.is_poisoned());
}
