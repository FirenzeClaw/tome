//! T038 验收测试：摘要与加密类宿主 API 在 JS 侧可用（FR-005）
//!
//! ## 覆盖矩阵（FR-005 的本类清单，逐项可对照）
//!
//! | 能力 | 测试 |
//! |---|---|
//! | MD5 | [`md5_在_js_侧可用`] |
//! | SHA 系列 | [`sha_系列在_js_侧可用`] |
//! | AES | [`aes_在_js_侧可用`] |
//! | DES | [`des_在_js_侧可用`] |
//! | RSA | [`rsa_在_js_侧可用`] |
//!
//! ## 最要紧的一条断言：摘要输出是**可读的十六进制**
//!
//! 书源把 `java.md5Encode(x)` 的返回值**直接拼进签名串**。若返回的是
//! 字节数组，JS 里 `"" + bytes` 会得到 `"49,50,51"` 这样的东西，签名
//! 全错——而这种错误只在服务端拒绝时才暴露，排查方向会被完全带偏。
//! 故 [`md5_在_js_侧可用`] 用 RFC 1321 的标准向量断言**精确的字符串**。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_js::api::{crypto, text};
use tome_js::runtime::{Limits, Sandbox};

/// 建一个已注册摘要与加密类 API 的沙箱。
fn sandbox_with_crypto_api() -> Sandbox {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let count = sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            // 摘要类的十六进制输出依赖 text 模块的编码函数？不——它们是
            // 内部调用，不经 JS。但书源常把两者串起来用，故一并注册。
            let registered = crypto::register(&ctx, &java)
                .map_err(|error| format!("注册 crypto 失败：{error:?}"))?;
            let text_registered = text::register(&ctx, &java)
                .map_err(|error| format!("注册 text 失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(registered + text_registered)
        })
        .expect("应能装配");
    let _ = count;
    sandbox
}

/// 求值并取回文本。
fn eval_text(sandbox: &Sandbox, script: &str) -> String {
    sandbox
        .eval(script)
        .unwrap_or_else(|error| panic!("脚本应失败但报错：{error:?}"))
        .text
}

// ---------------------------------------------------------------------------
// 摘要
// ---------------------------------------------------------------------------

#[test]
fn md5_在_js_侧可用() {
    let sandbox = sandbox_with_crypto_api();
    // RFC 1321 标准向量。断言**精确字符串**（不是"长度 32"）——
    // 书源把它拼进签名串，一个字符错就整条请求失败。
    assert_eq!(
        eval_text(&sandbox, "java.md5Encode('abc')"),
        "900150983cd24fb0d6963f7d28e17f72"
    );
    assert_eq!(
        eval_text(&sandbox, "java.md5Encode('')"),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
    // 中文（UTF-8 字节）
    assert_eq!(
        eval_text(&sandbox, "java.md5Encode('中文')"),
        "a7bac2239fcdcb3a067903d8077c4a07"
    );
}

#[test]
fn sha_系列在_js_侧可用() {
    let sandbox = sandbox_with_crypto_api();
    assert_eq!(
        eval_text(&sandbox, "java.sha1Encode('abc')"),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(
        eval_text(&sandbox, "java.sha256Encode('abc')"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        eval_text(&sandbox, "java.sha512Encode('abc')"),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
}

#[test]
fn md5_别名可用() {
    // 老书源里有 `java.md5(...)` 的写法（Rhino 时代）。
    let sandbox = sandbox_with_crypto_api();
    assert_eq!(
        eval_text(&sandbox, "java.md5('abc')"),
        "900150983cd24fb0d6963f7d28e17f72"
    );
}

#[test]
fn 摘要结果可参与字符串拼接() {
    // 书源的真实用法：把摘要拼进请求参数。
    let sandbox = sandbox_with_crypto_api();
    let script = r#"
      var sign = java.md5Encode("key=" + "value" + "&salt=abc");
      "https://a.com/api?sign=" + sign;
    "#;
    let url = eval_text(&sandbox, script);
    assert!(url.starts_with("https://a.com/api?sign="), "{url}");
    assert_eq!(
        url.trim_start_matches("https://a.com/api?sign=").len(),
        32,
        "摘要应是 32 位十六进制，实际：{url}"
    );
}

// ---------------------------------------------------------------------------
// AES
// ---------------------------------------------------------------------------

#[test]
fn aes_在_js_侧可用() {
    let sandbox = sandbox_with_crypto_api();
    let script = r#"
      var key = "1234567890abcdef";
      var iv  = "abcdef1234567890";
      var cipher = java.aesEncode("secret data", key, iv);
      java.aesDecode(cipher, key, iv);
    "#;
    assert_eq!(eval_text(&sandbox, script), "secret data");
}

#[test]
fn aes_密文是_base64() {
    // 书源把密文塞进 URL/JSON，故必须是 Base64 文本而不是二进制。
    let sandbox = sandbox_with_crypto_api();
    let cipher = eval_text(
        &sandbox,
        r#"java.aesEncode("x", "1234567890abcdef", "abcdef1234567890")"#,
    );
    assert!(
        cipher
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "+/=".contains(character)),
        "密文应是 Base64 文本，实际：{cipher}"
    );
    // 1 字节明文 + PKCS#7 填充 = 一个 16 字节块 → Base64 24 字符（含一个 `=`）。
    // 断言长度是为了钉住"输出的是**编码后的文本**"这一事实：
    // 若输出的是原始密文字节（16 字节），JS 侧会得到一个长度 16 的字符串，
    // 而那个字符串塞进 URL 会因含不可打印字节而坏掉。
    assert_eq!(cipher.len(), 24, "实际：{cipher}");

    // 明文更长时应按块对齐增长（再次证明是真加密而非截断）
    let longer = eval_text(
        &sandbox,
        r#"java.aesEncode("0123456789abcdef0123", "1234567890abcdef", "abcdef1234567890")"#,
    );
    // 20 字节 → 两个块（32 字节）→ Base64 44 字符
    assert_eq!(longer.len(), 44, "实际：{longer}");
}

#[test]
fn aes_三个密钥长度在_js_侧可用() {
    let sandbox = sandbox_with_crypto_api();
    for (name, key) in [
        ("aesEncode128", "a".repeat(16)),
        ("aesEncode192", "b".repeat(24)),
        ("aesEncode256", "c".repeat(32)),
    ] {
        let script = format!(
            r#"
            var key = "{key}";
            var iv = "0123456789abcdef";
            var cipher = java.{name}("data", key, iv);
            var decoded = java.{decode} (cipher, key, iv);
            decoded;
            "#,
            decode = name.replace("Encode", "Decode")
        );
        assert_eq!(eval_text(&sandbox, &script), "data", "`{name}` 应可用");
    }
}

#[test]
fn aes_十六进制变体可用() {
    let sandbox = sandbox_with_crypto_api();
    let script = r#"
      var key = "1234567890abcdef";
      var iv  = "abcdef1234567890";
      var hex = java.aesEncodeHex("data", key, iv);
      java.aesDecodeHex(hex, key, iv);
    "#;
    assert_eq!(eval_text(&sandbox, script), "data");
}

#[test]
fn aes_错误密钥长度抛出可捕获错误() {
    // 密钥写短是书源作者的常见笔误。必须报**可读**的错，而不是底层
    // crate 的晦涩信息——后者会让作者不知道是密钥长度问题。
    let sandbox = sandbox_with_crypto_api();
    let script = r#"
      try { java.aesEncode("x", "tooshort", "abcdef1234567890"); "未抛出"; }
      catch (e) { e.message.indexOf("16") >= 0 ? "已报长度" : "报错但不含长度：" + e.message; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "已报长度");
}

#[test]
fn aes_错误密钥时解密失败可捕获() {
    let sandbox = sandbox_with_crypto_api();
    let script = r#"
      var cipher = java.aesEncode("data", "1234567890abcdef", "abcdef1234567890");
      try { java.aesDecode(cipher, "1234567890abcdef", "0000000000000000"); "未抛出"; }
      catch (e) { "已捕获"; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "已捕获");
}

// ---------------------------------------------------------------------------
// DES
// ---------------------------------------------------------------------------

#[test]
fn des_在_js_侧可用() {
    let sandbox = sandbox_with_crypto_api();
    let script = r#"
      var key = "12345678";
      var iv  = "abcdefgh";
      var cipher = java.desEncode("secret", key, iv);
      java.desDecode(cipher, key, iv);
    "#;
    assert_eq!(eval_text(&sandbox, script), "secret");
}

// ---------------------------------------------------------------------------
// RSA
// ---------------------------------------------------------------------------

#[test]
fn rsa_在_js_侧可用() {
    let sandbox = sandbox_with_crypto_api();

    // 生成一对密钥（1024 位够验证语义且生成快）
    let mut rng = rand::thread_rng();
    let private = rsa::RsaPrivateKey::new(&mut rng, 1024).expect("应能生成");
    let public = rsa::RsaPublicKey::from(&private);
    let public_pem = {
        use rsa::pkcs8::EncodePublicKey;
        public
            .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
            .expect("应能导出")
    };
    let private_pem = {
        use rsa::pkcs8::EncodePrivateKey;
        private
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .expect("应能导出")
    };

    // PEM 里含换行，用 js_string 编成合法字面量
    let script = format!(
        r#"
        var cipher = java.rsaEncode("hello rsa", {public});
        java.rsaDecode(cipher, {private});
        "#,
        public = js_string(&public_pem),
        private = js_string(&private_pem)
    );
    assert_eq!(eval_text(&sandbox, &script), "hello rsa");
}

#[test]
fn rsa_无效密钥抛出可捕获错误() {
    let sandbox = sandbox_with_crypto_api();
    let script = r#"
      try { java.rsaEncode("x", "not a valid key"); "未抛出"; }
      catch (e) { e.message.indexOf("解析") >= 0 || e.message.indexOf("RSA") >= 0 ? "已报" : "报错但信息不清：" + e.message; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "已报");
}

// ---------------------------------------------------------------------------
// 与文本类协作（书源的常见组合）
// ---------------------------------------------------------------------------

#[test]
fn 摘要与_base64_可串联使用() {
    // 书源里的典型组合：先摘要再对摘要做 Base64。
    let sandbox = sandbox_with_crypto_api();
    let script = r#"
      java.base64Encode(java.md5Encode("abc"));
    "#;
    // "900150983cd24fb0d6963f7d28e17f72" 的 base64
    assert_eq!(
        eval_text(&sandbox, script),
        "OTAwMTUwOTgzY2QyNGZiMGQ2OTYzZjdkMjhlMTdmNzI="
    );
}

/// 把一个 Rust 字符串编成 JS 字符串字面量（含必要转义）。
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
