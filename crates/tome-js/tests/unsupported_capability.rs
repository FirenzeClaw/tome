//! T042 验收测试：不支持能力的结构化可捕获错误（FR-006）
//!
//! ## FR-006 的三个要件，逐条验收
//!
//! > 当书源使用系统不支持的能力时，系统 MUST 抛出**可被书源自身错误处理
//! > 捕获**的**结构化**错误，错误须携带**能力标识**与**人类可读说明**
//!
//! | 要件 | 本文件怎么证 |
//! |---|---|
//! | 可被捕获 | 用 `try/catch` 真的捕获它并读属性 |
//! | 结构化 | 断言 `e.capability` 是**稳定标识**（书源据此分支） |
//! | 人类可读说明 | 断言 `e.message` 含能力名与"为什么不支持" |
//!
//! ## 为什么必须用真实沙箱而不是只测 Rust 侧的类型
//!
//! 能力标识从 Rust 一路传到 JS 的 `catch` 要经过三道关：
//! `JsError` → `throw_host_error` 设置的异常属性 → JS 侧读取。
//! 只测第一道会让"抛出时忘了设属性"这类缺陷隐形——而那正是 FR-006
//! 最核心的失效形态（错误能捕获但书源读不到 capability，降级依然失效）。
//!
//! ## 关于"书源的降级逻辑"
//!
//! 每个测试都模拟一段真实的书源降级写法，而不是只断言"抛了个错"。
//! 因为 FR-006 的**目的**是让既有书源能降级运行，不是"抛错好看"。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_js::api::{register_all_unsupported, UnsupportedCapability};
use tome_js::runtime::{Limits, Sandbox};

/// 建一个已注册全部能力桩的沙箱，并把 `java` 暴露出来。
///
/// 返回的沙箱里 `java` 是一个**真实存在的对象**，其成员是能力桩——
/// 这正是组合根应当装配出的形态。
fn sandbox_with_capability_stubs() -> Sandbox {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox.install_prelude().expect("应能注入前导脚本");
    let registered = sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 对象失败：{error}"))?;
            let count = register_all_unsupported(&ctx, &java)
                .map_err(|error| format!("注册能力桩失败：{error:?}"))?;
            // 覆盖前导脚本放的桩：现在 `java` 是真实对象（含全部能力桩），
            // 而前导脚本的 `java` 是一个"任意属性都抛 hostRuntime"的 Proxy。
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局 java 失败：{error}"))?;
            Ok(count)
        })
        .expect("应能注册能力桩");
    // 注册数量 = 能力清单 + `webView` 家族成员（见 `register_all_unsupported`）。
    // 家族成员共用 `webView` 这个标识，故不在 `ALL` 里。
    assert!(
        registered >= UnsupportedCapability::ALL.len(),
        "至少应注册清单里的全部能力，实际 {registered}"
    );
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
// 每个能力：可被捕获 + 结构化 + 有说明
// ---------------------------------------------------------------------------

#[test]
fn 每个能力都可被_try_catch_捕获且带能力标识() {
    let sandbox = sandbox_with_capability_stubs();

    for capability in UnsupportedCapability::ALL {
        let id = capability.capability_id();
        // 模拟真实的书源降级写法
        let script = format!(
            r#"
            var outcome;
            try {{
                java.{id}("some-url");
                outcome = "NOT_THROWN";
            }} catch (e) {{
                outcome = e.capability;
            }}
            outcome;
            "#
        );
        assert_eq!(
            eval_text(&sandbox, &script),
            id,
            "`java.{id}()` 应在 JS 侧抛出带 capability=`{id}` 的可捕获错误"
        );
    }
}

#[test]
fn 异常携带人类可读说明() {
    let sandbox = sandbox_with_capability_stubs();
    let script = r#"
      try { java.webView("https://x/"); ""; }
      catch (e) { e.message; }
    "#;
    let message = eval_text(&sandbox, script);
    assert!(
        message.contains("webView"),
        "说明应含能力名，实际：{message}"
    );
    assert!(
        message.contains("浏览器"),
        "说明应解释为什么不支持，实际：{message}"
    );
}

#[test]
fn 异常是结构化的错误对象() {
    // 书源里常见的写法是 `catch (e) { e.name }` 或 `e instanceof Error`。
    // 若抛的是字符串，这些写法会得到 undefined。
    let sandbox = sandbox_with_capability_stubs();
    let script = r#"
      try { java.webView("x"); ""; }
      catch (e) {
        (e instanceof Error) + "|" + e.name + "|" + e.isUnsupportedCapability;
      }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "true|UnsupportedCapabilityError|true",
        "应是 Error 实例且带辨识属性"
    );
}

// ---------------------------------------------------------------------------
// 书源的降级逻辑真的能工作
// ---------------------------------------------------------------------------

#[test]
fn 书源的降级分支能走到() {
    // FR-006 的**目的**：既有书源在内嵌浏览器不可用时改走纯 HTTP 规则。
    // 这段脚本就是那种降级逻辑的缩影。
    let sandbox = sandbox_with_capability_stubs();
    let script = r#"
      function fetchWithFallback(url) {
        try {
          return java.webView(url);        // 本产品不支持
        } catch (e) {
          if (e.capability === "webView") {
            return "已降级到纯 HTTP 规则";   // 书源的备选路径
          }
          throw e;                          // 其它错误照常上抛
        }
      }
      fetchWithFallback("https://x/");
    "#;
    assert_eq!(eval_text(&sandbox, script), "已降级到纯 HTTP 规则");
}

#[test]
fn 书源可据能力标识区分不同能力() {
    // 有的书源对"浏览器不可用"与"验证码不可用"走不同的降级路径。
    let sandbox = sandbox_with_capability_stubs();
    let script = r#"
      function pick(name) {
        try {
          if (name === "browser") { java.startBrowserAwait("u"); }
          else { java.getVerificationCode(); }
        } catch (e) {
          if (e.isUnsupportedCapability) { return "unsupported:" + e.capability; }
          return "other:" + e.message;
        }
      }
      pick("browser") + " / " + pick("captcha");
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "unsupported:startBrowserAwait / unsupported:getVerificationCode"
    );
}

#[test]
fn 非能力错误不会被误判为能力不可用() {
    // 反向验收：`isUnsupportedCapability` 必须能区分"能力不可用"与
    // "这次失败了"。若书源把所有异常都当成能力问题去降级，它会掩盖
    // 真实的网络/规则错误。
    let sandbox = sandbox_with_capability_stubs();
    let script = r#"
      try { throw new Error("普通错误"); }
      catch (e) { String(e.isUnsupportedCapability); }
    "#;
    assert_eq!(eval_text(&sandbox, script), "undefined");
}

// ---------------------------------------------------------------------------
// 与前导脚本的桩保持一致（两处都必须报同一个能力名）
// ---------------------------------------------------------------------------

#[test]
fn 注册的桩覆盖前导脚本的兜底桩() {
    // 前导脚本放的是一个"任何属性都抛 hostRuntime"的 Proxy 兜底桩
    // （因为那时不知道组合根会装什么）。组合根注册真实能力桩之后，
    // 应当拿到**具体的能力标识**而不是笼统的 hostRuntime——
    // 否则书源的 `if (e.capability === "webView")` 永远不成立。
    let sandbox = sandbox_with_capability_stubs();
    let script = r#"
      try { java.webView("x"); ""; }
      catch (e) { e.capability; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "webView");
    assert_ne!(
        eval_text(&sandbox, script),
        "hostRuntime",
        "不应落到前导脚本的笼统兜底标识"
    );
}

#[test]
fn 未注册的能力落到前导脚本的兜底桩() {
    // 反面：某个能力没被注册时，前导脚本的 Proxy 兜底仍应给出**可捕获**的
    // 错误（哪怕标识笼统）。这保证"组合根漏注册"不会退化成 TypeError。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox.install_prelude().expect("应能注入");
    let script = r#"
      try { java.someUnregisteredCapability("x"); ""; }
      catch (e) { String(e.isUnsupportedCapability); }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "true",
        "未注册的能力也应抛可捕获的结构化错误"
    );
}

// ---------------------------------------------------------------------------
// 能力清单是完整且可枚举的（FR-005b 的取向）
// ---------------------------------------------------------------------------

#[test]
fn 能力清单覆盖_fr006_点名的四项() {
    // spec 与 tasks.md 都点名了这四个。清单里缺任何一个，
    // 对应的书源就会撞上 TypeError 而不是能力错误。
    let ids: Vec<&str> = UnsupportedCapability::ALL
        .iter()
        .map(|capability| capability.capability_id())
        .collect();
    for expected in [
        "webView",
        "startBrowserAwait",
        "getVerificationCode",
        "openVideoPlayer",
    ] {
        assert!(
            ids.contains(&expected),
            "能力清单应含 `{expected}`，实际：{ids:?}"
        );
    }
}

#[test]
fn 注册数量等于清单长度() {
    // 组合根可据此断言"装齐了"。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let count = sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 对象失败：{error}"))?;
            register_all_unsupported(&ctx, &java).map_err(|error| format!("{error:?}"))
        })
        .expect("应能执行");
    // 含 `webView` 家族成员，故 >= 清单长度
    assert!(count >= UnsupportedCapability::ALL.len(), "实际 {count}");
    assert!(count >= 4, "至少覆盖 FR-006 点名的四项");
}
