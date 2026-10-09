//! T035 验收测试：兼容前导脚本（Rhino 方言补齐）
//!
//! ## 这些测试在守什么
//!
//! tasks.md T035 的验收项是"prelude 在每个 runtime eval 前注入"，
//! 而它的**实质目标**是：既有书源脚本无需改动即可运行（宪法原则 I）。
//! 故测试不止于"注入成功"，而要逐类验证补齐项真的生效：
//!
//! | 补齐项 | 不补时的表现 |
//! |---|---|
//! | `trimLeft`/`trimRight` 别名 | `TypeError: not a function` |
//! | `Java.type` 桩 | `TypeError: Cannot read property 'type' of undefined` |
//! | `Packages.*` 桩 | 同上，且路径越深越难归因 |
//! | `source`/`book`/`chapter` 上下文 | `TypeError: Cannot read property ... of undefined` |
//! | `console` | `ReferenceError: console is not defined` |
//!
//! ## 两类测试
//!
//! - **补齐生效**：别名可调用、桩抛出正确的能力错误。
//! - **不越界**：前导脚本不得覆盖宿主已注入的真实实现，也不得污染全局
//!   命名空间（书源会遍历全局）。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数
// （`sandbox_with_prelude` / `eval_text` / `js_string`）。这些辅助函数失败
// 说明测试环境本身有问题，此时让调用它的测试当场失败是正确行为。
// 与 `tome-cli/tests/minimal_loop.rs` 同一处理。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::time::Duration;

use tome_js::prelude::PRELUDE_JS;
use tome_js::runtime::{Limits, Sandbox, SandboxError};

/// 建一个已注入前导脚本的沙箱。
fn sandbox_with_prelude() -> Sandbox {
    let sandbox = Sandbox::new(Limits {
        wall_clock_timeout: Some(Duration::from_secs(5)),
        ..Limits::default()
    })
    .expect("应能建沙箱");
    sandbox.install_prelude().expect("前导脚本应能注入");
    sandbox
}

/// 求值并取回文本结果。
fn eval_text(sandbox: &Sandbox, script: &str) -> String {
    sandbox
        .eval(script)
        .unwrap_or_else(|error| panic!("脚本 `{script}` 应成功，实际 {error:?}"))
        .text
}

// ---------------------------------------------------------------------------
// 注入本身
// ---------------------------------------------------------------------------

#[test]
fn 前导脚本可注入且语法合法() {
    // 前导脚本是一大段 JS，写错一个符号就会让**所有**依赖它的书源失败。
    // 这条测试是最基本的护栏。
    let sandbox = sandbox_with_prelude();
    assert_eq!(eval_text(&sandbox, "1+1"), "2");
}

#[test]
fn 前导脚本可重复注入而不报错() {
    // 同一个 runtime 上重复注入应当幂等（桩只在缺失时才补，
    // 已在的不会被换掉）。若重复注入报错，会让"误调两次"变成致命错误。
    let sandbox = sandbox_with_prelude();
    sandbox.install_prelude().expect("重复注入也应成功");
    assert_eq!(eval_text(&sandbox, "1+1"), "2");
}

#[test]
fn 未注入前导脚本时依然可用() {
    // 反面：前导脚本是补丁层，不是沙箱的必需品。不注入时沙箱必须照常工作
    // （否则"要不要注入"就不是一个真正的选择）。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    assert_eq!(eval_text(&sandbox, "1+1"), "2");
}

// ---------------------------------------------------------------------------
// String 别名
// ---------------------------------------------------------------------------

#[test]
fn trim_别名被补齐() {
    let sandbox = sandbox_with_prelude();
    // Rhino/老引擎的写法在 QuickJS 上会 TypeError，补别名后应正常。
    assert_eq!(eval_text(&sandbox, r#"'  x  '.trimLeft()"#), "x  ");
    assert_eq!(eval_text(&sandbox, r#"'  x  '.trimRight()"#), "  x");
    // 标准名同样可用（两个方向都接受）
    assert_eq!(eval_text(&sandbox, r#"'  x  '.trimStart()"#), "x  ");
    assert_eq!(eval_text(&sandbox, r#"'  x  '.trimEnd()"#), "  x");
}

#[test]
fn trim_别名与原生实现语义一致() {
    // 补齐的实现是正则，而原生 `trim` 还需处理不换行空格等空白字符。
    // 用真实场景的输入验证两者一致。
    //
    // 测试数据从 Rust 侧传入（用 `js_string` 编成 JS 字面量），而不是把
    // 特殊字符直接写进源码——源码里的真实控制字符看不见，改动时极易写错
    // （本测试的第一版就踩了这个坑）。
    let sandbox = sandbox_with_prelude();
    for probe in [
        "   abc   ",
        "\ttab\t",
        "\n换行\n",
        "\u{00A0}nbsp\u{00A0}",
        "",
        "no-trim-needed",
    ] {
        let script = format!(
            "var p = {literal}; p.trim() === p.trimLeft().trimRight() ? 'same' : 'diff';",
            literal = js_string(probe)
        );
        assert_eq!(
            eval_text(&sandbox, &script),
            "same",
            "输入 {probe:?}：别名结果应等于原生 trim"
        );
    }
}

/// 把一个 Rust 字符串编成 JS 字符串字面量（含必要的转义）。
///
/// # 为什么不引 `serde_json`
///
/// 这是一个测试辅助函数，为它加一个 dev-dependency 不值（体积与供应链
/// 纪律）。需要转义的字符集很明确，手写反而更可控：
///
/// | 字符 | 转义 | 不转义的后果 |
/// |---|---|---|
/// | `"` `\` | 加反斜杠 | 字符串提前结束 / 转义错乱 |
/// | `\n` `\r` `\t` | 标准转义 | 源码里出现真实控制字符 |
/// | U+2028 / U+2029 | `\uXXXX` | **JS 里它们是行终止符**，会让字面量断行 |
///
/// 最后一条是 JS 特有的坑：这两个字符在 JSON 里可以裸写，在 JS 字符串
/// 字面量里却是非法的。本函数因此不能简单复用 JSON 的转义规则。
/// 其余控制字符按 `\uXXXX` 处理（本测试的数据里不会出现，但保持正确性）。
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
            // JS 的行终止符：必须转义，否则字面量会断行（见函数文档）
            '\u{2028}' => output.push_str("\\u2028"),
            '\u{2029}' => output.push_str("\\u2029"),
            other if (other as u32) < 0x20 => {
                output.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => output.push(other),
        }
    }
    output.push('"');
    output
}

#[test]
fn 前导脚本不覆盖已有的别名实现() {
    // 设计约束 2：只补齐，不覆盖。引擎已有时保持原实现。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    // 先自己定义一个标记实现，再注入前导脚本
    sandbox
        .eval("String.prototype.trimLeft = function(){ return 'MARK'; };")
        .expect("应能执行");
    sandbox.install_prelude().expect("应能注入");
    assert_eq!(
        eval_text(&sandbox, "'  x  '.trimLeft()"),
        "MARK",
        "已有的实现不应被前导脚本覆盖"
    );
}

// ---------------------------------------------------------------------------
// Java / Packages 桩
// ---------------------------------------------------------------------------

#[test]
fn java_type_抛结构化能力错误() {
    let sandbox = sandbox_with_prelude();
    let script = r#"
      try {
        Java.type("org.jsoup.Jsoup");
        "未抛出";
      } catch (e) {
        e.capability + "|" + e.isUnsupportedCapability + "|" + e.name;
      }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "javaClass|true|UnsupportedCapabilityError",
        "Java.type 必须抛带能力标识的可捕获错误"
    );
}

#[test]
fn packages_任意深度路径可点到并抛能力错误() {
    // 书源会写 `Packages.org.jsoup.Jsoup.parse(...)`、`Packages.java.util.HashMap`
    // 等任意深度。桩必须支持无限点下去，且在**调用**时才抛。
    let sandbox = sandbox_with_prelude();
    for path in [
        "Packages.java.util.HashMap",
        "Packages.org.jsoup.Jsoup",
        "Packages.a.b.c.d.e.f.g",
        "Packages.java.lang.String",
        "Packages.a().b().c",
    ] {
        let script = format!(r#"try {{ {path}(); "未抛出"; }} catch (e) {{ e.capability; }}"#);
        assert_eq!(
            eval_text(&sandbox, &script),
            "javaClass",
            "`{path}()` 应抛能力错误"
        );
    }
}

#[test]
fn packages_的原型方法落回_js_语义而非抛错() {
    // 已知边界（见 prelude.js 里的 `[UNCERTAIN]` 说明）：`valueOf` / `call`
    // 等 Function.prototype 上的方法无法与"书源以为在调 JVM 方法"区分开，
    // 因为语法完全相同。取"不抛"是更安全的一侧——这些调用不产生错误结果。
    //
    // 本测试把这个**有意**的边界钉住：若哪天改成抛错，这里会失败，
    // 提醒改动者重新权衡（而不是让行为悄悄变化）。
    let sandbox = sandbox_with_prelude();
    assert_eq!(
        eval_text(&sandbox, "typeof Packages.java.lang.String.valueOf"),
        "function",
        "valueOf 应落回 JS 语义（不抛）"
    );
    // 而真正代表"JVM 方法"的任意名字仍会抛
    let script = r#"
      try { Packages.java.lang.String.toUpperCase(); "未抛出"; }
      catch (e) { e.capability; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "javaClass");
}

#[test]
fn 取_java_引用而不调用时不抛() {
    // 关键取舍：书源里常见 `var Jsoup = Packages.org.jsoup.Jsoup;` 这类
    // "取引用但这次不调用"的写法。在取值时就抛会让整段脚本失败，
    // 而它本可以在不走进该分支的情况下正常工作。
    let sandbox = sandbox_with_prelude();
    assert_eq!(
        eval_text(
            &sandbox,
            "var Jsoup = Packages.org.jsoup.Jsoup; typeof Jsoup;"
        ),
        "function",
        "只取引用不应抛错"
    );
}

#[test]
fn java_宿主运行时缺失时给出可捕获错误() {
    // 组合根未注入真实 `java` 对象时，桩必须抛能力错误（而不是
    // `ReferenceError: java is not defined`）——书源的 try/catch 据此降级。
    let sandbox = sandbox_with_prelude();
    let script = r#"
      try { java.ajax("https://x/"); "未抛出"; }
      catch (e) { e.capability; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "hostRuntime");
}

#[test]
fn 已注入的真实_java_对象不被桩覆盖() {
    // 设计约束 2 的实际影响：组合根装了真的 `java` 时，前导脚本的桩
    // 绝不能把它换掉——那会把一个可用能力变成"不支持"。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .eval("var java = { ajax: function(url){ return 'REAL:' + url; } };")
        .expect("应能执行");
    sandbox.install_prelude().expect("应能注入");
    assert_eq!(
        eval_text(&sandbox, "java.ajax('u')"),
        "REAL:u",
        "真实的 java 实现必须保留"
    );
}

#[test]
fn 已注入的真实_packages_对象不被桩覆盖() {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .eval("var Packages = { marker: 'REAL' };")
        .expect("应能执行");
    sandbox.install_prelude().expect("应能注入");
    assert_eq!(eval_text(&sandbox, "Packages.marker"), "REAL");
}

// ---------------------------------------------------------------------------
// 上下文对象
// ---------------------------------------------------------------------------

#[test]
fn 上下文对象存在且取值不抛() {
    // 真实值由组合根注入（T036-T041）。此处只保证"引用不缺"：
    // 缺失时给空对象，让 `source.x` 得到 undefined 而不抛 TypeError。
    let sandbox = sandbox_with_prelude();
    for name in ["source", "book", "chapter", "cookie", "cache"] {
        assert_eq!(
            eval_text(&sandbox, &format!("typeof {name}")),
            "object",
            "`{name}` 应是对象（缺失时给空对象而非 undefined）"
        );
        assert_eq!(
            eval_text(&sandbox, &format!("typeof {name}.anything")),
            "undefined",
            "`{name}.anything` 不应抛错"
        );
    }
}

#[test]
fn 真实的上下文注入不被覆盖() {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .eval("var source = { bookSourceName: '真实源名' };")
        .expect("应能执行");
    sandbox.install_prelude().expect("应能注入");
    assert_eq!(eval_text(&sandbox, "source.bookSourceName"), "真实源名");
}

// ---------------------------------------------------------------------------
// 不污染全局命名空间
// ---------------------------------------------------------------------------

#[test]
fn 前导脚本不留下临时全局名() {
    // 它必须是立即执行的函数表达式。若泄漏了 `i`、`names`、`global` 这类
    // 局部变量名，书源脚本的 `var i` 就会撞上——而那种冲突的表现是
    // "某个源莫名奇妙地算出错值"，极难归因。
    let sandbox = sandbox_with_prelude();
    for name in [
        "global", "names", "noop", "i", "proto", "target", "receiver",
    ] {
        assert_eq!(
            eval_text(&sandbox, &format!("typeof {name}")),
            "undefined",
            "前导脚本不应泄漏全局名 `{name}`"
        );
    }
    // 内部工具名是有意保留的（供 Rust 侧复用），故它们**应该**存在
    assert_eq!(
        eval_text(&sandbox, "typeof __tomeThrowUnsupported"),
        "function",
        "Rust 侧需要复用的抛错入口应保留"
    );
}

// ---------------------------------------------------------------------------
// console
// ---------------------------------------------------------------------------

#[test]
fn console_存在且可调用() {
    // 书源用 `console.log` 调试很常见，缺失会让整段脚本 ReferenceError。
    let sandbox = sandbox_with_prelude();
    assert_eq!(eval_text(&sandbox, "typeof console.log"), "function");
    // 调用它不应抛错（默认实现是 no-op）
    assert_eq!(eval_text(&sandbox, "console.log('x'); 'ok'"), "ok");
}

#[test]
fn 真实的_console_对象不被覆盖() {
    // 组合根把日志接到宿主时，前导脚本不该把它换回 no-op。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .eval("var console = { log: function(x){ return 'CAPTURED:' + x; } };")
        .expect("应能执行");
    sandbox.install_prelude().expect("应能注入");
    assert_eq!(eval_text(&sandbox, "console.log('y')"), "CAPTURED:y");
}

// ---------------------------------------------------------------------------
// 与前导脚本共存：护栏仍生效
// ---------------------------------------------------------------------------

#[test]
fn 注入前导脚本后护栏仍生效() {
    // 前导脚本是额外注入的一段代码，不该削弱沙箱的任何护栏。
    let sandbox = Sandbox::new(Limits {
        wall_clock_timeout: Some(Duration::from_millis(300)),
        ..Limits::default()
    })
    .expect("应能建沙箱");
    sandbox.install_prelude().expect("应能注入");

    let outcome = sandbox.eval("while(true){}");
    assert!(
        matches!(outcome, Err(SandboxError::Interrupted { .. })),
        "注入前导脚本后死循环仍应被中断，实际 {outcome:?}"
    );
}

#[test]
fn 前导脚本源码常量与注入效果一致() {
    // `PRELUDE_JS` 是公开常量，组合根可能自己注入它（而非调
    // `install_prelude`）。两者必须等价。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox.eval(PRELUDE_JS).expect("直接用常量注入也应成功");
    //  桩是 Proxy 包着函数（可点可调用），故  是 function
    assert_eq!(eval_text(&sandbox, "typeof Packages"), "function");
    assert_eq!(eval_text(&sandbox, "' x '.trimLeft()"), "x ");
}
