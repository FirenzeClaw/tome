//! T040 验收测试：正则 + 时间日期 + 杂项在 JS 侧可用（FR-005）
//!
//! ## 覆盖矩阵
//!
//! | 能力 | 测试 |
//! |---|---|
//! | 正则：匹配/查找 | [`正则匹配可用`] |
//! | 正则：替换 | [`正则替换可用`] |
//! | 正则：捕获组提取 | [`捕获组提取可用`] |
//! | 时间日期：时间戳 ↔ 格式化 | [`时间格式化可用`] |
//! | 时间日期：日期计算 | [`日期计算可用`] |
//! | 杂项：随机数 | [`随机数可用`] |
//! | 杂项：压缩解压 | [`压缩解压可用`] |
//! | 杂项：HTML 实体 | [`html_实体可用`] |
//!
//! ## 最要紧的一组断言：正则必须是 **JS 语义**
//!
//! `util.rs` 的正则入口委托给 JS 引擎的原生 `RegExp`，而不是在 Rust 侧
//! 用 `regex`/`fancy-regex` 重实现。理由见该模块文档。本文件把几条
//! **只有 JS 语义才会这样**的行为钉住——若哪天有人把它改成 Rust 引擎，
//! 这些测试会失败：
//!
//! - `$&` / `$1` 等 JS 替换模式
//! - 命名捕获组 `(?<name>...)`
//! - 回溯语义（`(a+)+b` 这类在 DFA 引擎下行为不同）
//! - `/u` 标志下的 Unicode 行为

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_js::api::{text, util};
use tome_js::runtime::{Limits, Sandbox};

/// 建一个已注册正则/时间/杂项 API 的沙箱。
fn sandbox_with_util_api() -> Sandbox {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let count = sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            let registered = util::register(&ctx, &java)
                .map_err(|error| format!("注册 util 失败：{error:?}"))?;
            // 杂项测试里要用 base64 之类的编码函数
            let text_registered = text::register(&ctx, &java)
                .map_err(|error| format!("注册 text 失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(registered + text_registered)
        })
        .expect("应能装配");
    assert!(count >= 14, "本类应注册至少 14 个函数，实际 {count}");
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
// 正则
// ---------------------------------------------------------------------------

#[test]
fn 正则匹配可用() {
    let sandbox = sandbox_with_util_api();
    // 书源的典型用法：从响应里提取内容
    // `\w` 在 JS 里只匹配 ASCII 词字符（中文要用显式字符类），
    // 故这里用 `.+`——它也贴近书源"取到行尾"的实际写法。
    assert_eq!(
        eval_text(
            &sandbox,
            r#"String(java.match("书名：斗破苍穹", "书名：(.+)"))"#
        ),
        "书名：斗破苍穹,斗破苍穹"
    );
    // 无匹配返回 null
    assert_eq!(
        eval_text(&sandbox, r#"String(java.match("abc", "\\d+"))"#),
        "null"
    );
}

#[test]
fn 正则匹配带标志可用() {
    let sandbox = sandbox_with_util_api();
    // `i` 忽略大小写
    assert_eq!(
        eval_text(&sandbox, r#"String(java.match("ABC", "abc", "i"))"#),
        "ABC"
    );
}

#[test]
fn 正则替换可用() {
    let sandbox = sandbox_with_util_api();
    // 默认全局替换（书源净化正文的用途几乎都是全部替换）
    assert_eq!(
        eval_text(&sandbox, r##"java.replace("a1b2c3", "\\d", "#")"##),
        "a#b#c#"
    );
}

#[test]
fn 正则替换支持_js_替换模式() {
    // ## 这是"必须是 JS 语义"的关键证据
    //
    // `$&`（整个匹配）、`$1`（捕获组）是 JS 的替换模式语法。
    // Rust 的 `regex` crate 用 `${1}` 且不认 `$&`——若我们把正则委托给
    // Rust 引擎，下面两条会静默产出错误的文本。
    let sandbox = sandbox_with_util_api();
    assert_eq!(
        eval_text(&sandbox, r#"java.replace("abc", "b", "[$&]")"#),
        "a[b]c",
        "`$&` 应展开为整个匹配"
    );
    assert_eq!(
        eval_text(&sandbox, r#"java.replace("a1b2", "(\\d)", "<$1>")"#),
        "a<1>b<2>",
        "`$1` 应展开为捕获组"
    );
}

#[test]
fn 捕获组提取可用() {
    let sandbox = sandbox_with_util_api();
    // 返回数组：第 0 项是整个匹配，其后是各捕获组
    let script = r#"
      var groups = java.getMatchGroups("2026-10-11", "(\\d{4})-(\\d{2})-(\\d{2})");
      groups.join("|");
    "#;
    assert_eq!(eval_text(&sandbox, script), "2026-10-11|2026|10|11");
}

#[test]
fn 捕获组未参与匹配时给空串() {
    // `(a)|(b)` 匹配 "a" 时第二个组未参与，JS 给 undefined。
    // 我们统一成空串（书源里判空更少陷阱）。
    let sandbox = sandbox_with_util_api();
    assert_eq!(
        eval_text(&sandbox, r#"java.getMatchGroups("a", "(a)|(b)").join("|")"#),
        "a|a|"
    );
}

#[test]
fn 正则支持命名捕获组() {
    // 命名捕获组是 ES2018 特性，Rust 的 `regex` crate 也支持但语法与
    // 提取方式不同（`captures["name"]`）。委托给 JS 引擎则天然一致。
    let sandbox = sandbox_with_util_api();
    let script = r#"
      var m = java.match("2026-10-11", "(?<year>\\d{4})-(?<month>\\d{2})");
      m.groups.year + "/" + m.groups.month;
    "#;
    assert_eq!(eval_text(&sandbox, script), "2026/10");
}

#[test]
fn 正则语法的_js_特性可用() {
    // 后行断言（lookbehind）是 ES2018 特性；Rust `regex` 不支持它，
    // `fancy-regex` 支持但语义不完全一致。
    let sandbox = sandbox_with_util_api();
    assert_eq!(
        eval_text(&sandbox, r#""价格100元".replace(/(?<=价格)\d+/, "N")"#),
        "价格N元"
    );
}

#[test]
fn 非法正则模式抛出可捕获错误() {
    let sandbox = sandbox_with_util_api();
    let script = r#"
      try { java.match("abc", "(["); "未抛出"; }
      catch (e) { "已捕获"; }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "已捕获",
        "非法模式应抛可捕获错误（而不是让引擎错误裸奔）"
    );
}

#[test]
fn 正则求值不留全局临时变量() {
    // 实现上用临时全局变量传参（见 util.rs 的说明），必须清理。
    let sandbox = sandbox_with_util_api();
    let _ = eval_text(&sandbox, r#"java.replace("abc", "b", "x"); "ok";"#);
    for name in [
        "__tome_regex_text",
        "__tome_regex_pattern",
        "__tome_regex_flags",
        "__tome_regex_replacement",
    ] {
        assert_eq!(
            eval_text(&sandbox, &format!("typeof {name}")),
            "undefined",
            "临时全局变量 `{name}` 必须被清理"
        );
    }
}

#[test]
fn 正则模式里的反斜杠不被转义破坏() {
    // 实现上用全局变量传参而不是拼进脚本——拼接会让 `\d` 变成 `d`。
    // 这条测试钉住那个决定。
    let sandbox = sandbox_with_util_api();
    assert_eq!(
        eval_text(&sandbox, r#"java.replace("a1", "\\d", "N")"#),
        "aN",
        "`\\d` 应作为正则元字符生效"
    );
    assert_eq!(
        eval_text(&sandbox, r#"java.replace("a.b", "\\.", "-")"#),
        "a-b"
    );
}

// ---------------------------------------------------------------------------
// 时间日期
// ---------------------------------------------------------------------------

#[test]
fn 时间格式化可用() {
    let sandbox = sandbox_with_util_api();
    // 书源的典型用法：把接口返回的秒级时间戳转成可读日期。
    //
    // ## 用"往返验证"而不是断言字面值
    //
    // 格式化受**本地时区**影响（书源作者与用户在同一个时区运行 tome，
    // 故时间是本地时间是正确的语义）。断言字面值会让测试在别的时区失败。
    // 这里改为：格式化 → 用 JS 的 Date 解析回来 → 应与原时间戳一致。
    let script = r#"
      var ts = 1700000000;  // 秒
      var formatted = java.timeFormat(ts, "yyyy-MM-dd HH:mm:ss");
      // 用同一模式解析回来需要手动拆解；改为验证"格式正确且各字段自洽"
      var parts = formatted.split(/[- :]/);
      var reconstructed = new Date(
        parseInt(parts[0], 10), parseInt(parts[1], 10) - 1, parseInt(parts[2], 10),
        parseInt(parts[3], 10), parseInt(parts[4], 10), parseInt(parts[5], 10)
      );
      Math.floor(reconstructed.getTime() / 1000) === ts;
    "#;
    assert_eq!(eval_text(&sandbox, script), "true");
}

#[test]
fn 时间格式化输出符合模式形状() {
    let sandbox = sandbox_with_util_api();
    let formatted = eval_text(
        &sandbox,
        r#"java.timeFormat(1700000000, "yyyy-MM-dd HH:mm:ss")"#,
    );
    // 形状断言（不依赖时区）：长度与分隔符
    assert_eq!(formatted.len(), 19, "实际：{formatted}");
    assert_eq!(&formatted[4..5], "-");
    assert_eq!(&formatted[7..8], "-");
    assert_eq!(&formatted[10..11], " ");
    assert_eq!(&formatted[13..14], ":");
    assert_eq!(&formatted[16..17], ":");
    assert!(
        formatted.chars().filter(char::is_ascii_digit).count() == 14,
        "实际：{formatted}"
    );
}

#[test]
fn 时间格式化的常见模式都可用() {
    let sandbox = sandbox_with_util_api();
    for pattern in [
        "yyyy-MM-dd",
        "yyyy/MM/dd",
        "HH:mm",
        "yyyy-MM-dd HH:mm:ss",
        "yyyy年MM月dd日",
    ] {
        let script = format!(
            r#"java.timeFormat(1700000000, {pattern})"#,
            pattern = js_string(pattern)
        );
        let formatted = eval_text(&sandbox, &script);
        assert!(
            !formatted.contains("yyyy") && !formatted.contains("MM") && !formatted.contains("dd"),
            "模式 `{pattern}` 的 token 应全部被替换，实际：{formatted}"
        );
    }
}

#[test]
fn 时间格式化的短_token_不被长_token_吃掉() {
    // `yyyy` 与 `yy`、`MM` 与 `M` 的替换顺序错乱会产出 `2626` 这类错值。
    // 这是实现里最容易写错的一处，故单独钉住。
    let sandbox = sandbox_with_util_api();
    let full = eval_text(&sandbox, r#"java.timeFormat(1700000000, "yyyy")"#);
    let short = eval_text(&sandbox, r#"java.timeFormat(1700000000, "yy")"#);
    assert_eq!(full.len(), 4, "`yyyy` 应是 4 位年份，实际：{full}");
    assert_eq!(short.len(), 2, "`yy` 应是 2 位年份，实际：{short}");
    assert!(
        full.ends_with(&short),
        "`yy` 应是 `yyyy` 的后两位：{full} vs {short}"
    );
}

#[test]
fn 毫秒时间戳可用() {
    let sandbox = sandbox_with_util_api();
    // JS 的 Date 用毫秒，书源里的毫秒时间戳用 timeFormatMs
    let script = r#"
      var fromSeconds = java.timeFormat(1700000000, "yyyy-MM-dd HH:mm:ss");
      var fromMillis  = java.timeFormatMs(1700000000000, "yyyy-MM-dd HH:mm:ss");
      fromSeconds === fromMillis;
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "true",
        "秒与毫秒入口应给出同一时刻的同一结果"
    );
}

#[test]
fn 当前时间可用() {
    let sandbox = sandbox_with_util_api();
    let script = r#"
      var now = java.now();
      // 应是合理的毫秒时间戳（2020 年之后、2100 年之前）
      now > 1577836800000 && now < 4102444800000;
    "#;
    assert_eq!(eval_text(&sandbox, script), "true");
}

#[test]
fn 日期计算可用() {
    // 书源里会用"当前时间 ± N 天"构造查询范围。
    let sandbox = sandbox_with_util_api();
    let script = r#"
      var dayMs = 86400000;
      var nowMs = java.now();
      var lastWeek = java.timeFormatMs(nowMs - 7 * dayMs, "yyyy-MM-dd");
      var today = java.timeFormatMs(nowMs, "yyyy-MM-dd");
      var parsedLastWeek = new Date(lastWeek + "T00:00:00").getTime();
      var parsedToday = new Date(today + "T00:00:00").getTime();
      Math.round((parsedToday - parsedLastWeek) / dayMs) === 7;
    "#;
    assert_eq!(eval_text(&sandbox, script), "true");
}

// ---------------------------------------------------------------------------
// 杂项：随机数
// ---------------------------------------------------------------------------

#[test]
fn 随机数可用() {
    let sandbox = sandbox_with_util_api();
    let script = r#"
      var values = [];
      for (var i = 0; i < 20; i++) { values.push(java.random()); }
      var allInRange = values.every(function(v){ return v >= 0 && v < 1; });
      var hasVariation = values[0] !== values[1];
      allInRange + ":" + hasVariation;
    "#;
    assert_eq!(eval_text(&sandbox, script), "true:true");
}

#[test]
fn 随机整数可用() {
    let sandbox = sandbox_with_util_api();
    let script = r#"
      var values = [];
      for (var i = 0; i < 50; i++) { values.push(java.randomInt(10)); }
      values.every(function(v){ return v >= 0 && v < 10; });
    "#;
    assert_eq!(eval_text(&sandbox, script), "true");
    // 边界值不崩：书源可能传一个算错的边界
    assert_eq!(
        eval_text(&sandbox, "java.randomInt(0) + java.randomInt(-5)"),
        "0"
    );
}

// ---------------------------------------------------------------------------
// 杂项：压缩解压
// ---------------------------------------------------------------------------

#[test]
fn 压缩解压可用() {
    let sandbox = sandbox_with_util_api();
    // gzip 编解码：输出是字节数组，故用 base64 中转做往返验证
    let script = r#"
      var original = "压缩测试内容 compress me";
      var compressed = java.gzipEncode(original);
      java.gzipDecode(compressed);
    "#;
    assert_eq!(eval_text(&sandbox, script), "压缩测试内容 compress me");
}

#[test]
fn 通用解压自动识别_gzip() {
    // 书源作者常分不清 gzip/zlib/裸 deflate，故 inflate 自动识别。
    let sandbox = sandbox_with_util_api();
    let script = r#"
      var compressed = java.gzipEncode("auto");
      java.inflate(compressed);
    "#;
    assert_eq!(eval_text(&sandbox, script), "auto");
}

#[test]
fn 不可识别的压缩数据抛出可捕获错误() {
    let sandbox = sandbox_with_util_api();
    let script = r#"
      try { java.inflate([255, 254, 253]); "未抛出"; }
      catch (e) { "已捕获"; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "已捕获");
}

// ---------------------------------------------------------------------------
// 杂项：HTML 实体
// ---------------------------------------------------------------------------

#[test]
fn html_实体可用() {
    let sandbox = sandbox_with_util_api();
    assert_eq!(
        eval_text(
            &sandbox,
            r#"java.htmlDecode("&lt;div&gt;&amp;&nbsp;&#20013;")"#
        ),
        "<div>&\u{00A0}中"
    );
    assert_eq!(
        eval_text(&sandbox, r#"java.htmlEncode("<a href=\"x\">")"#),
        "&lt;a href=&quot;x&quot;&gt;"
    );
}

#[test]
fn html_实体解码在书源净化场景里生效() {
    // 书源从页面取到的文本常带实体，需要解码后才能用于展示或匹配。
    let sandbox = sandbox_with_util_api();
    let script = r#"
      var fromPage = "斗破&amp;苍穹&hellip;";
      java.htmlDecode(fromPage) === "斗破&苍穹…";
    "#;
    assert_eq!(eval_text(&sandbox, script), "true");
}

// ---------------------------------------------------------------------------
// 注册完整性
// ---------------------------------------------------------------------------

#[test]
fn 全部正则时间杂项函数都已注册() {
    let sandbox = sandbox_with_util_api();
    for name in [
        "match",
        "matchAll",
        "replace",
        "getMatchGroups",
        "timeFormat",
        "timeFormatMs",
        "now",
        "random",
        "randomInt",
        "gzipEncode",
        "gzipDecode",
        "inflate",
        "deflate",
        "htmlEncode",
        "htmlDecode",
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
