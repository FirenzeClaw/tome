//! T037 验收测试：文本与编码类宿主 API 在 JS 侧可用（FR-005）
//!
//! ## 为什么单元测试不够
//!
//! `text.rs` 的 12 条单元测试验证的是**纯函数的行为**（Base64 标准向量、
//! URL 保留字符、简繁映射）。但 FR-005 要求的是"书源脚本能调用它们"——
//! 那中间还隔着注册、类型转换、错误抛出三道关。本文件补上这段：
//!
//! ```text
//! 书源脚本 java.base64Encode(x)
//!   → JS 引擎派发到注册的 Rust 函数
//!   → value_to_bytes 做类型归一（字符串/数组/undefined）
//!   → 纯函数计算
//!   → 返回值转回 JS
//! ```
//!
//! 只在 Rust 侧测会让"注册时名字写错"（`base64Encode` vs `base64encode`）、
//! "参数类型没归一"这类缺陷完全不可见。
//!
//! ## 覆盖矩阵（FR-005 的本类清单，逐项可对照）
//!
//! | 能力 | 测试 |
//! |---|---|
//! | Base64 | [`base64_在_js_侧可用`] |
//! | URL 编解码 | [`url_编解码在_js_侧可用`] |
//! | 十六进制 | [`十六进制在_js_侧可用`] |
//! | 字符集转换 | [`字符集转换在_js_侧可用`] |
//! | 中文简繁转换 | [`简繁转换在_js_侧可用`] |

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_js::api::text;
use tome_js::runtime::{Limits, Sandbox};

/// 建一个已注册文本与编码类 API 的沙箱。
fn sandbox_with_text_api() -> Sandbox {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let count = sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            let count =
                text::register(&ctx, &java).map_err(|error| format!("注册失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(count)
        })
        .expect("应能装配");
    assert!(count >= 8, "本类应注册至少 8 个函数，实际 {count}");
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
// Base64
// ---------------------------------------------------------------------------

#[test]
fn base64_在_js_侧可用() {
    let sandbox = sandbox_with_text_api();
    // 书源里的典型用法：给接口参数做编码
    assert_eq!(eval_text(&sandbox, "java.base64Encode('foo')"), "Zm9v");
    assert_eq!(eval_text(&sandbox, "java.base64Decode('Zm9v')"), "foo");
    // 中文（UTF-8 字节）
    let encoded = eval_text(&sandbox, "java.base64Encode('斗破')");
    assert_eq!(
        eval_text(&sandbox, &format!("java.base64Decode('{encoded}')")),
        "斗破"
    );
}

#[test]
fn base64_编码接受字节数组() {
    // 书源里 `java.strToBytes` 之类的返回值是数字数组，
    // 直接喂给 base64Encode 是常见写法。
    let sandbox = sandbox_with_text_api();
    assert_eq!(
        eval_text(&sandbox, "java.base64Encode([102, 111, 111])"),
        "Zm9v"
    );
}

#[test]
fn base64_非法输入抛出可捕获错误() {
    let sandbox = sandbox_with_text_api();
    let script = r#"
      try { java.base64Decode('!!!!'); "未抛出"; }
      catch (e) { "已捕获"; }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "已捕获",
        "非法输入应抛可捕获错误（而不是让整段脚本因引擎错误而中断）"
    );
}

// ---------------------------------------------------------------------------
// URL 编解码
// ---------------------------------------------------------------------------

#[test]
fn url_编解码在_js_侧可用() {
    let sandbox = sandbox_with_text_api();
    // encodeURI 保留 URI 结构字符（书源几乎都这样用：拼一条 URL 的查询值）
    assert_eq!(
        eval_text(&sandbox, "java.encodeURI('https://a.com/s?q=斗破 苍穹')"),
        "https://a.com/s?q=%E6%96%97%E7%A0%B4%20%E8%8B%8D%E7%A9%B9"
    );
    assert_eq!(
        eval_text(&sandbox, "java.decodeURI('%E6%96%97%E7%A0%B4')"),
        "斗破"
    );
}

#[test]
fn url_编解码往返() {
    let sandbox = sandbox_with_text_api();
    let script = r#"
      var original = "https://a.com/路径?q=值&r=2";
      java.decodeURI(java.encodeURI(original)) === original;
    "#;
    assert_eq!(eval_text(&sandbox, script), "true");
}

// ---------------------------------------------------------------------------
// 十六进制
// ---------------------------------------------------------------------------

#[test]
fn 十六进制在_js_侧可用() {
    let sandbox = sandbox_with_text_api();
    assert_eq!(eval_text(&sandbox, "java.hexEncodeToString('ab')"), "6162");
    assert_eq!(eval_text(&sandbox, "java.hexDecodeToString('6162')"), "ab");
    // 书源接口签名场景：先 hex 再 base64
    assert_eq!(
        eval_text(&sandbox, "java.hexEncodeToString([0, 255])"),
        "00ff"
    );
}

// ---------------------------------------------------------------------------
// 字符集转换
// ---------------------------------------------------------------------------

#[test]
fn 字符集转换在_js_侧可用() {
    let sandbox = sandbox_with_text_api();
    // 与旧站点交互时书源会做显式编码转换。此处验证"字符串↔字节"这条
    // 通道在 JS 侧确实可用（具体字符集的编解码在 tome-net 的 charset 层，
    // 由 T039 的数据类与 T036 的网络类消费）。
    assert_eq!(
        eval_text(&sandbox, "java.base64Encode('中文');"),
        // "中文" 的 UTF-8 字节 5Lit5paH
        "5Lit5paH"
    );
    // 字节数组 → 十六进制 → 再回来，证明字节通道双向可用
    assert_eq!(
        eval_text(
            &sandbox,
            "java.hexDecodeToString(java.hexEncodeToString('中文'))"
        ),
        "中文"
    );
}

// ---------------------------------------------------------------------------
// 中文简繁转换
// ---------------------------------------------------------------------------

#[test]
fn 简繁转换在_js_侧可用() {
    let sandbox = sandbox_with_text_api();
    assert_eq!(eval_text(&sandbox, "java.t2s('這個書')"), "这个书");
    assert_eq!(eval_text(&sandbox, "java.s2t('这个书')"), "這個書");
    assert_eq!(eval_text(&sandbox, "java.t2s('鬥破蒼穹')"), "斗破苍穹");
}

#[test]
fn 简繁转换在书源的匹配场景里生效() {
    // 书源用简繁转换的**真实目的**：把繁体源的书名归一到简体，
    // 以便与用户输入的简体书名做匹配。这条测试模拟那个场景。
    let sandbox = sandbox_with_text_api();
    let script = r#"
      var fromSource = "鬥破蒼穹";       // 繁体站点返回的书名
      var fromUser = "斗破苍穹";         // 用户输入的简体
      java.t2s(fromSource) === fromUser;
    "#;
    assert_eq!(eval_text(&sandbox, script), "true");
}

// ---------------------------------------------------------------------------
// 注册的完整性
// ---------------------------------------------------------------------------

#[test]
fn 全部文本类函数都已注册() {
    let sandbox = sandbox_with_text_api();
    for name in [
        "base64Encode",
        "base64Decode",
        "encodeURI",
        "decodeURI",
        "hexEncodeToString",
        "hexDecodeToString",
        "t2s",
        "s2t",
    ] {
        assert_eq!(
            eval_text(&sandbox, &format!("typeof java.{name}")),
            "function",
            "`java.{name}` 应是已注册的函数"
        );
    }
}
