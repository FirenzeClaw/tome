//! T041 验收测试：宿主状态类 API 在 JS 侧可用（FR-005）
//!
//! ## 覆盖矩阵
//!
//! | 能力 | 测试 |
//! |---|---|
//! | 缓存读写 | [`缓存读写可用`] |
//! | 文件读写（限数据目录） | [`文件读写可用`]、[`文件越界被拒绝且可捕获`] |
//! | Cookie 读写 | [`cookie_读写可用`] |
//! | 引入外部脚本 | [`引入外部脚本可用`] |
//! | 变量池读写 | [`变量池可用`] |
//! | 上下文访问 | [`上下文对象可用`] |
//!
//! ## 本文件最重要的一组：`文件越界被拒绝且可捕获`
//!
//! AGENTS.md 的安全红线与 spec 的边缘情况都要求"脚本的文件访问限制在
//! 数据目录内，越界须拒绝并报告"。拒绝在 `StoreHost` 实现侧（本替身实现了
//! 完整的包含判定），而本测试验证的是**整条链路**：
//!
//! ```text
//! 脚本 java.writeFile("../../x") → StoreHost 拒绝（UnsupportedCapability）
//!   → 转成 JS 可捕获异常 → 书源 catch 拿到 capability
//! ```
//!
//! 只测 StoreHost 会让"JS 桥把错误吞掉了"这类缺陷隐形。

// `.clippy.toml` 的测试豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::sync::Arc;

use tome_core::{CryptoHost, StoreHost};
use tome_js::api::state;
use tome_js::bridge::JsContext;
use tome_js::runtime::{Limits, Sandbox};
use tome_js::{HostBridge, MemoryCryptoHost, MemoryStoreHost};

/// 建一个已注册宿主状态类 API 的沙箱。
///
/// 返回 `(沙箱, 存储替身)`。
fn sandbox_with_state_api() -> (Sandbox, Arc<MemoryStoreHost>) {
    let store = Arc::new(MemoryStoreHost::new());

    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox.install_prelude().expect("应能注入前导脚本");

    // 构造一个"完整的"上下文：书源 + 书籍 + 章节都有值，
    // 以便验证 `source.bookSourceName` 之类的访问真的拿到了值。
    let mut context = JsContext::for_source("https://source.test");
    context.source_name = Some("测试书源".to_string());
    context.source_group = Some("自用".to_string());
    context.book_name = Some("斗破苍穹".to_string());
    context.book_author = Some("天蚕土豆".to_string());
    context.book_url = Some("https://source.test/book/1".to_string());
    context.chapter_title = Some("第一章".to_string());
    context.chapter_url = Some("https://source.test/book/1/c1".to_string());
    context.chapter_index = Some(0);
    context.base_url = Some("https://source.test".to_string());

    let store_for_bridge: Arc<dyn StoreHost> = store.clone();
    let crypto: Arc<dyn CryptoHost> = Arc::new(MemoryCryptoHost::new());
    let bridge = HostBridge::bare(context)
        .with_store(store_for_bridge)
        .with_crypto(crypto);
    sandbox.install_host_bridge(bridge).expect("应能注入宿主桥");

    sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            state::register(&ctx, &java).map_err(|error| format!("注册 state 失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            // 上下文对象经桥装配（读的是同一个 `JsContext`）
            let bridged = ctx
                .userdata::<HostBridge>()
                .ok_or_else(|| "宿主桥应已注入".to_string())?;
            state::install_context_objects(&ctx, &bridged)
                .map_err(|error| format!("装配上下文失败：{error:?}"))?;
            Ok(())
        })
        .expect("应能装配");

    (sandbox, store)
}

/// 建一个**未注入**存储能力的沙箱（用于验证能力错误的形状）。
fn sandbox_without_store() -> Sandbox {
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .install_host_bridge(HostBridge::bare(JsContext::for_source("s")))
        .expect("应能注入桥");
    sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            state::register(&ctx, &java).map_err(|error| format!("注册失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(())
        })
        .expect("应能装配");
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
// 缓存
// ---------------------------------------------------------------------------

#[test]
fn 缓存读写可用() {
    let (sandbox, store) = sandbox_with_state_api();
    let script = r#"
      java.cachePut("token", "abc123");
      java.cacheGet("token");
    "#;
    assert_eq!(eval_text(&sandbox, script), "abc123");
    // 应真的落到存储
    assert_eq!(
        store.cache_get("token").expect("应能读").as_deref(),
        Some(&b"abc123"[..])
    );
}

#[test]
fn 缓存未命中返回空串() {
    // 空串让 `if (java.cacheGet("k"))` 与 `=== ""` 两种判空写法都能用
    // （`null` 会让后者失败）。
    let (sandbox, _store) = sandbox_with_state_api();
    assert_eq!(eval_text(&sandbox, r#"java.cacheGet("missing")"#), "");
    assert_eq!(
        eval_text(&sandbox, r#"String(java.cacheGet("missing") === "")"#),
        "true"
    );
}

#[test]
fn 缓存可存非文本内容() {
    let (sandbox, store) = sandbox_with_state_api();
    // 字节数组形态（书源里从网络取回的二进制内容）
    let _ = eval_text(&sandbox, r#"java.cachePut("bin", [0, 255, 128]); "ok";"#);
    assert_eq!(
        store.cache_get("bin").expect("应能读"),
        Some(vec![0, 255, 128])
    );
}

#[test]
fn 未注入存储能力时抛结构化能力错误() {
    let sandbox = sandbox_without_store();
    let script = r#"
      try { java.cachePut("k", "v"); "未抛出"; }
      catch (e) { e.capability; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "store");
}

// ---------------------------------------------------------------------------
// 文件（安全红线）
// ---------------------------------------------------------------------------

#[test]
fn 文件读写可用() {
    let (sandbox, store) = sandbox_with_state_api();
    let script = r#"
      java.writeFile("data/note.txt", "内容");
      java.readFile("data/note.txt");
    "#;
    assert_eq!(eval_text(&sandbox, script), "内容");
    assert_eq!(
        store.read_file("data/note.txt").expect("应能读"),
        "内容".as_bytes().to_vec()
    );
}

#[test]
fn 文件越界被拒绝且可捕获() {
    // ## 本文件最重要的测试（AGENTS.md 安全红线 / spec 边缘情况）
    //
    // 验证**整条链路**：StoreHost 拒绝 → 转成 JS 可捕获异常 →
    // 书源 catch 拿到 capability 标识。
    //
    // 只测 StoreHost 会让"JS 桥把错误吞掉了"这类缺陷隐形——那样书源会
    // 拿到一个空的成功结果，**以为文件写成功了**。
    let (sandbox, store) = sandbox_with_state_api();
    for path in [
        "../escape.txt",
        "../../etc/passwd",
        "/etc/passwd",
        "sub/../../outside.txt",
        "C:/Windows/x",
        "..\\win\\x",
    ] {
        let script = format!(
            r#"
            try {{ java.writeFile({path}, "x"); "未抛出"; }}
            catch (e) {{ e.capability; }}
            "#,
            path = js_string(path)
        );
        assert_eq!(
            eval_text(&sandbox, &script),
            "fileAccessOutOfDataDir",
            "越界路径 `{path}` 应抛带能力标识的可捕获错误"
        );
    }
    // 且不得留下副作用（拒绝必须发生在写入之前）
    assert_eq!(store.file_len(), 0, "越界写入不得真的落盘");

    // 读路径同样受限
    let script = r#"
      try { java.readFile("../../escape.txt"); "未抛出"; }
      catch (e) { e.capability; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "fileAccessOutOfDataDir");
}

#[test]
fn 书源的降级逻辑能处理越界() {
    // FR-006 的目的：书源的 try/catch 能据此降级，
    // 而不是让整段脚本因一个越界尝试而中断。
    let (sandbox, _store) = sandbox_with_state_api();
    let script = r#"
      function saveFile(pathOfFile, content) {
        try {
          java.writeFile(pathOfFile, content);
          return "saved:" + pathOfFile;
        } catch (e) {
          if (e.capability === "fileAccessOutOfDataDir") {
            return "rejected:" + pathOfFile;
          }
          throw e;
        }
      }
      saveFile("ok.txt", "x") + " / " + saveFile("../bad.txt", "x");
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "saved:ok.txt / rejected:../bad.txt"
    );
}

#[test]
fn 读取不存在的文件抛出可捕获错误() {
    let (sandbox, _store) = sandbox_with_state_api();
    let script = r#"
      try { java.readFile("missing.txt"); "未抛出"; }
      catch (e) { "已捕获"; }
    "#;
    assert_eq!(eval_text(&sandbox, script), "已捕获");
}

// ---------------------------------------------------------------------------
// Cookie
// ---------------------------------------------------------------------------

#[test]
fn cookie_读写可用() {
    let (sandbox, store) = sandbox_with_state_api();
    let script = r#"
      java.setCookie("token", "secret-value");
      java.getCookie("token");
    "#;
    assert_eq!(eval_text(&sandbox, script), "secret-value");
    // 应落在带前缀的键上，与普通缓存不冲突
    assert_eq!(
        store
            .cache_get("tome://cookie/token")
            .expect("应能读")
            .as_deref(),
        Some(&b"secret-value"[..]),
        "cookie 应存在带前缀的键上"
    );
}

#[test]
fn cookie_与普通缓存不互相覆盖() {
    // 同名的 cookie 与缓存键必须互不干扰——否则表现是"登录态莫名失效"。
    let (sandbox, _store) = sandbox_with_state_api();
    let script = r#"
      java.setCookie("token", "cookie-value");
      java.cachePut("token", "cache-value");
      java.getCookie("token") + "/" + java.cacheGet("token");
    "#;
    assert_eq!(eval_text(&sandbox, script), "cookie-value/cache-value");
}

#[test]
fn cookie_未设置返回空串() {
    let (sandbox, _store) = sandbox_with_state_api();
    assert_eq!(eval_text(&sandbox, r#"java.getCookie("none")"#), "");
}

// ---------------------------------------------------------------------------
// 变量池
// ---------------------------------------------------------------------------

#[test]
fn 变量池可用() {
    let (sandbox, _store) = sandbox_with_state_api();
    let script = r#"
      java.put("stage", "search");
      java.get("stage");
    "#;
    assert_eq!(eval_text(&sandbox, script), "search");
}

#[test]
fn 变量池别名可用() {
    // 两个名字在书源生态里都出现过；多两个别名比让一批书源失效便宜。
    let (sandbox, _store) = sandbox_with_state_api();
    let script = r#"
      java.setVariable("a", "1");
      var first = java.getVariable("a");
      java.put("b", "2");
      first + java.get("b");
    "#;
    assert_eq!(eval_text(&sandbox, script), "12");
}

#[test]
fn 变量池可删除与清空() {
    let (sandbox, _store) = sandbox_with_state_api();
    let script = r#"
      java.put("a", "1");
      java.put("b", "2");
      var removed = java.removeVariable("a");
      var afterRemove = java.get("a") + "/" + java.get("b");
      java.clearVariables();
      removed + ":" + afterRemove + ":" + java.get("b");
    "#;
    assert_eq!(eval_text(&sandbox, script), "true:/2:");
}

#[test]
fn 变量池在两个阶段之间传值() {
    // 书源的真实用法：搜索页存 token，目录页取用（同一次抓取内）。
    // 这条验证变量池的**持久性**（同一沙箱内跨 eval 调用保留）。
    let (sandbox, _store) = sandbox_with_state_api();
    let _ = eval_text(&sandbox, r#"java.put("token", "xyz"); "ok";"#);
    // 下一次独立的 eval 仍应读得到
    assert_eq!(eval_text(&sandbox, r#"java.get("token")"#), "xyz");
}

#[test]
fn 变量池未命中返回空串() {
    let (sandbox, _store) = sandbox_with_state_api();
    assert_eq!(eval_text(&sandbox, r#"java.get("none")"#), "");
}

// ---------------------------------------------------------------------------
// 上下文
// ---------------------------------------------------------------------------

#[test]
fn 上下文对象可用() {
    let (sandbox, _store) = sandbox_with_state_api();
    assert_eq!(
        eval_text(&sandbox, "source.bookSourceUrl"),
        "https://source.test"
    );
    assert_eq!(eval_text(&sandbox, "source.bookSourceName"), "测试书源");
    assert_eq!(eval_text(&sandbox, "book.name"), "斗破苍穹");
    assert_eq!(eval_text(&sandbox, "book.author"), "天蚕土豆");
    assert_eq!(eval_text(&sandbox, "chapter.title"), "第一章");
    assert_eq!(eval_text(&sandbox, "chapter.index"), "0");
    assert_eq!(eval_text(&sandbox, "baseUrl"), "https://source.test");
}

#[test]
fn 上下文的值可直接参与拼接() {
    // 书源里最常见的写法：`book.name + " - " + chapter.title`
    let (sandbox, _store) = sandbox_with_state_api();
    assert_eq!(
        eval_text(&sandbox, r#"book.name + " - " + chapter.title"#),
        "斗破苍穹 - 第一章"
    );
}

#[test]
fn 缺失的上下文字段是空串而非_undefined() {
    // `undefined` 参与拼接会得到 "undefined:书名" —— 看起来正常但错误的结果。
    // 组合根未提供某字段时给空串，让结果至少是"无害"的形态。
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    sandbox
        .install_host_bridge(HostBridge::bare(JsContext::for_source("s")))
        .expect("应能注入");
    sandbox
        .with_context(|ctx| {
            let bridged = ctx.userdata::<HostBridge>().ok_or("应有桥".to_string())?;
            state::install_context_objects(&ctx, &bridged)
                .map_err(|error| format!("装配失败：{error:?}"))?;
            Ok(())
        })
        .expect("应能装配");

    assert_eq!(eval_text(&sandbox, "book.name"), "");
    assert_eq!(eval_text(&sandbox, "chapter.title"), "");
    assert_eq!(eval_text(&sandbox, r#"book.author + "x""#), "x");
}

#[test]
fn 上下文对象的原型链不干扰访问() {
    // 书源可能写 `source.bookSourceName.length` 之类。对象应是普通对象
    // （不是 Proxy 桩），属性访问不受阻。
    let (sandbox, _store) = sandbox_with_state_api();
    assert_eq!(eval_text(&sandbox, "source.bookSourceName.length"), "4");
    assert_eq!(eval_text(&sandbox, "typeof source"), "object");
}

// ---------------------------------------------------------------------------
// 引入外部脚本
// ---------------------------------------------------------------------------

#[test]
fn 引入外部脚本可用() {
    let (sandbox, _store) = sandbox_with_state_api();
    // 未配置 nested_runtime 工厂时应报**结构化能力错误**
    // （而不是 panic 或静默返回空）
    let script = r#"
      try { java.importScript("1+1"); "未抛出"; }
      catch (e) { e.capability; }
    "#;
    assert_eq!(
        eval_text(&sandbox, script),
        "importScript",
        "未配置嵌套 runtime 时应报结构化能力错误"
    );
}

#[test]
fn 配置了嵌套工厂后引入外部脚本可用() {
    // 用独立 runtime 执行外部脚本（避免重入当前引擎）。
    let store = Arc::new(MemoryStoreHost::new());
    let sandbox = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let store_for_bridge: Arc<dyn StoreHost> = store;
    let factory: Arc<dyn Fn() -> Result<Sandbox, tome_js::SandboxError> + Send + Sync> =
        Arc::new(|| Sandbox::new(Limits::default()));
    sandbox
        .install_host_bridge(
            HostBridge::bare(JsContext::for_source("s"))
                .with_store(store_for_bridge)
                .with_nested_runtime(factory),
        )
        .expect("应能注入");
    sandbox
        .with_context(|ctx| {
            let java = rquickjs::Object::new(ctx.clone())
                .map_err(|error| format!("建 java 失败：{error}"))?;
            state::register(&ctx, &java).map_err(|error| format!("注册失败：{error:?}"))?;
            ctx.globals()
                .set("java", java)
                .map_err(|error| format!("设置全局失败：{error}"))?;
            Ok(())
        })
        .expect("应能装配");

    assert_eq!(
        eval_text(&sandbox, r#"java.importScript("40 + 2")"#),
        "42",
        "外部脚本应在独立沙箱里执行并返回结果"
    );
}

// ---------------------------------------------------------------------------
// 注册完整性
// ---------------------------------------------------------------------------

#[test]
fn 全部宿主状态类函数都已注册() {
    let (sandbox, _store) = sandbox_with_state_api();
    for name in [
        "cachePut",
        "cacheGet",
        "readFile",
        "writeFile",
        "getCookie",
        "setCookie",
        "put",
        "get",
        "setVariable",
        "getVariable",
        "removeVariable",
        "clearVariables",
        "importScript",
    ] {
        assert_eq!(
            eval_text(&sandbox, &format!("typeof java.{name}")),
            "function",
            "`java.{name}` 应是已注册的函数（FR-005 宿主状态类清单）"
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
