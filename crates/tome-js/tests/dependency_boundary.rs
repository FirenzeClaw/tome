//! T031 验收测试：`tome-js` 的依赖边界与沙箱隔离（tasks.md Phase 4）
//!
//! ## 这些测试在守什么
//!
//! 架构铁律是"crate 边界即架构，用编译期强制，不靠约定"（AGENTS.md）。
//! `tome-js` 的边界有两条：
//!
//! 1. **只依赖 `tome-core`** —— 宿主能力 trait（`NetHost`/`StoreHost`/`CryptoHost`）
//!    定义在 core，由**组合根**注入实现。若 `tome-js` 去依赖 `tome-net`，
//!    就会与"tome-net 实现 NetHost、NetHost 引用 RequestSpec"构成循环锁定的
//!    隐患，也让沙箱无法在测试里用替身（T033 的存在意义）。
//! 2. **每个书源脚本一个独立 runtime** —— 宿主函数在 JS 侧**同步阻塞**
//!    （书源脚本是同步语义，R1），阻塞期间该 runtime 的锁被持有。共享
//!    runtime 意味着"一个源发请求，所有源停摆"。
//!
//! ## 为什么第 1 条要用"读 Cargo.toml"来测
//!
//! 编译期强制已经由 Cargo 保证（没写在依赖表里的 crate 用不了）。但
//! **"没写"这件事本身会随重构被人顺手改掉**——有人为了图方便加一行
//! `tome-net = { workspace = true }`，编译器不会有任何意见，架构却已经破了。
//! 故这里把"依赖表里只能有谁"写成断言，让越界在 CI 上当场失败。

// `.clippy.toml` 的 `allow-panic-in-tests` 覆盖 `#[test]` 函数体，
// 但**不覆盖**本文件里的辅助函数（如 `manifest_text`）。
// 这些辅助函数失败说明测试环境本身有问题（Cargo.toml 读不到），
// 此时让调用它的测试当场失败是正确行为。与 `tome-cli/tests/minimal_loop.rs`
// 同一处理。
#![expect(
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::path::PathBuf;

use tome_js::runtime::{Limits, Sandbox};

/// 读取 `tome-js` 自己的 `Cargo.toml` 文本。
fn manifest_text() -> String {
    // `CARGO_MANIFEST_DIR` 由 cargo 在编译测试时注入，指向本 crate 的根目录。
    // 用相对路径拼到仓库根再从 `crates/tome-js/Cargo.toml` 读，避免把
    // 绝对路径写进测试（那会让测试在别的机器上失败）。
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = crate_dir.join("Cargo.toml");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("应能读 {}：{error}", path.display()))
}

/// 从 `Cargo.toml` 的 `[dependencies]` 段里取出依赖名。
///
/// 只需处理"段首到下一个 `[` 之间"的行，够本测试用；不引 TOML 解析器是
/// 为了不给 tome-js 增加 dev-dependency（体积纪律）。
fn dependency_names(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_dependencies = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_dependencies = trimmed == "[dependencies]";
            continue;
        }
        if !in_dependencies || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // 形态：`name = ...` 或 `name.workspace = true`
        if let Some((key, _)) = trimmed.split_once('=') {
            let name = key.trim().split('.').next().unwrap_or("").trim();
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }
    names
}

#[test]
fn tome_js_只依赖_tome_core() {
    let manifest = manifest_text();
    let names = dependency_names(&manifest);

    // 架构允许的三类：
    //
    // 1. `tome-core` —— 接缝（宿主能力 trait 与共享值对象）
    // 2. 引擎本身（`rquickjs`）与错误类型（`thiserror`，AGENTS.md 要求
    //    每个 crate 拥有自己的错误类型）
    // 3. **纯计算**算法 crate（摘要/编码）—— 它们不引入"实现 crate 依赖"，
    //    与 `tome-net`/`tome-store` 这类承载 I/O 实现与宿主能力的 crate 不同：
    //    摘要与 base64 是 `CryptoHost` 的本体实现（trait 文档写明
    //    "纯计算，tome-js 内建，不需外部实现"），放进 `tome-core` 会污染
    //    接缝中枢的零依赖约束。
    //
    // 判据不是"库名白名单"本身，而是**"这个依赖会不会让 tome-js 拿到
    // 宿主能力的实现"**——下面显式点名的四个 crate 才是真正要防的。
    let allowed = [
        "tome-core",
        "rquickjs",
        "thiserror",
        // 摘要与 Base64（纯计算）
        "md-5",
        "sha1",
        "sha2",
        "base64",
        // 对称/非对称加密（纯计算）
        "aes",
        "cbc",
        "des",
        "rsa",
        // RSA 的 PKCS#1 v1.5 填充需要随机数（纯计算，无 I/O）
        "rand",
        // 结构化数据：JSON 解析/序列化（纯计算）
        // 与 tome-store 的 SQLite 无关——不引入任何 I/O 能力
        "serde_json",
    ];
    let unexpected: Vec<&String> = names
        .iter()
        .filter(|name| !allowed.contains(&name.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "tome-js 的依赖表出现了架构不允许的 crate：{unexpected:?}\n\
         实际依赖：{names:?}\n\
         新增依赖前先想清楚：它会不会让 tome-js 拿到宿主能力的实现？\n\
         （宿主能力 trait 定义在 tome-core，由组合根注入实现）"
    );

    // 显式点名四个最容易被顺手加上的。
    for forbidden in ["tome-net", "tome-store", "tome-source", "tome-rule"] {
        assert!(
            !names.iter().any(|name| name == forbidden),
            "tome-js 不得依赖 {forbidden}（架构铁律 A1：实现 crate 只依赖 tome-core）"
        );
    }
}

#[test]
fn 依赖表中确实有_tome_core() {
    // 反向断言：上面的测试在"依赖表为空"时也会通过。接缝 trait 必须真的在。
    let names = dependency_names(&manifest_text());
    assert!(
        names.iter().any(|name| name == "tome-core"),
        "tome-js 必须依赖 tome-core（宿主能力 trait 在那里），实际：{names:?}"
    );
}

// ---------------------------------------------------------------------------
// 每个书源脚本独立 runtime（A1 / R1）
// ---------------------------------------------------------------------------

#[test]
fn 两个沙箱的状态完全隔离() {
    // 这是"每书源脚本独立 runtime"的可观测形态：一个沙箱里的全局变量
    // 不会泄漏到另一个。
    let first = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let second = Sandbox::new(Limits::default()).expect("应能建沙箱");

    first
        .eval("var leaked = '第一个沙箱的值';")
        .expect("应能执行");

    let observed = second
        .eval("typeof leaked === 'undefined' ? '未泄漏' : leaked")
        .expect("应能执行");
    assert_eq!(
        observed.as_str(),
        Some("未泄漏"),
        "第二个沙箱不得看到第一个沙箱的全局变量（runtime 必须隔离）"
    );
}

#[test]
fn 一个沙箱被废弃不影响另一个() {
    // R1 的实质：隔离的收益是"一个源出事不拖垮别的源"。
    // 若共享 runtime，OOM 会同时废掉所有源。
    let poisoned = Sandbox::new(Limits {
        max_memory_bytes: 2 * 1024 * 1024,
        gc_threshold_bytes: 512 * 1024,
        ..Limits::default()
    })
    .expect("应能建沙箱");

    let _ = poisoned.eval("var a = []; for (;;) { a.push(new Array(1000).fill(0)); }");
    assert!(poisoned.is_poisoned(), "应已废弃");

    let healthy = Sandbox::new(Limits::default()).expect("应能建沙箱");
    assert_eq!(
        healthy.eval("'仍然可用'").expect("应能执行").as_str(),
        Some("仍然可用"),
        "另一个沙箱不得受影响"
    );
}

#[test]
fn 取消令牌不跨沙箱传播() {
    // 若要停的是"某一个书源的脚本"，取消就不能是全局的。
    // 这里钉住"取消令牌属于单个沙箱"这一事实。
    let first = Sandbox::new(Limits::default()).expect("应能建沙箱");
    let second = Sandbox::new(Limits::default()).expect("应能建沙箱");

    assert!(!first.cancel_token().is_cancelled());
    assert!(!second.cancel_token().is_cancelled());

    first.cancel_token().cancel();
    assert!(first.cancel_token().is_cancelled(), "第一个应已取消");
    assert!(
        !second.cancel_token().is_cancelled(),
        "第二个不该被连带取消（取消令牌不跨沙箱）"
    );

    // 已取消的沙箱明确拒绝执行，而不是"静默跑完"
    assert!(second.eval("1+1").is_ok(), "未取消的沙箱应照常工作");
    assert!(first.eval("1+1").is_err(), "已取消的沙箱应拒绝执行新脚本");
}
