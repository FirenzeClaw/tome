//! `[DONE]` 依赖图断言（tasks.md T096 / FR-039、宪章 IV）
//!
//! # 需求原文与它要守的东西
//!
//! > **FR-039**: 引擎 MUST NOT 依赖阅读器或界面层的任何类型；上层实现被
//! > 替换时引擎无需改动
//!
//! 宪章 IV（简单性与单向依赖）把它具体化为：
//!
//! ```text
//!                          ┌──────────────┐
//!                          │  tome-core   │  ← 无依赖，被所有人依赖
//!                          └──────────────┘
//!                             ▲  ▲  ▲  ▲
//!             ┌───────────────┘  │  │  └───────────────┐
//!             │                  │  │                  │
//!      ┌────────────┐   ┌────────────┐   ┌────────────┐
//!      │  tome-js   │   │  tome-net  │   │ tome-store │
//!      └────────────┘   └────────────┘   └────────────┘
//! ```
//!
//! **星型无环**：`tome-js` / `tome-net` / `tome-store` 三者互不依赖，
//! 各自只依赖 `tome-core`。这条结构的价值在于"改一个实现 crate 不会
//! 牵动另一个"——它是"crate 边界即架构"（AGENTS.md）的具体内容。
//!
//! # 为什么用**测试**断言而不是只靠 CI 脚本
//!
//! CI 脚本（`ci.yml` 里的步骤）只在 CI 上跑；本地开发者改 Cargo.toml 后
//! 要到推送后才知道越界。测试则 `cargo test` 就能发现——**反馈越快，
//! 架构越不容易在"随手加一行"里破掉**。
//!
//! CI 里另有一份等价断言（T096 要求），两者不重复劳动：CI 那份是给
//! 分支保护用的（单个可勾选的 check），本地这份是给开发者的即时反馈。
//!
//! # 断言的三类
//!
//! | 类 | 内容 |
//! |---|---|
//! | ① `tome-core` 独立 | 它的依赖表里**没有**任何 `tome-*`——core 是地基 |
//! | ② 星型无环 | `tome-js`/`tome-net`/`tome-store` 互不依赖 |
//! | ③ 引擎不依赖上层 | 引擎 crate 不依赖 `tome-reader`/`tome-app` |
//!
//! ## 关于 `tome-store → tome-net` 这个例外
//!
//! `tome-store` 的依赖表里有 `tome-net`（T064：`http_log` 的落点要实现
//! `tome_net::HttpLogSink`）。**这不违反星型无环**——方向仍是单向
//! （`tome-store → tome-net → tome-core`），没有环。
//!
//! 宪章 IV 要的是"无环"，不是"任意两个实现 crate 都不相连"。故本文件
//! 对 `tome-store → tome-net` **显式放行并记录理由**，而不是假装它不存在：
//! 一个把所有例外都写明的断言表，比一个"看起来干净但需要猜测哪些是允许的"
//! 更有用。

// `.clippy.toml` 的豁免只覆盖 `#[test]` 函数体，不覆盖辅助函数
// （如 `workspace_deps`）。那些辅助函数失败说明测试环境本身有问题
// （Cargo.toml 读不到），此时让调用它的测试当场失败是正确行为。
// 与 `tome-js/tests/dependency_boundary.rs` 同一处理。
#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "依赖表读不到时说明测试环境有问题，让所属测试当场失败是正确行为"
)]

use std::path::{Path, PathBuf};

/// 仓库根（`crates/tome-cli/tests/` 上溯三级）。
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate 目录应有两级父目录")
        .to_path_buf()
}

/// 读一个 crate 的 `Cargo.toml`，返回它在 `[dependencies]` 里声明的
/// **workspace 内**依赖（`tome-*`）。
///
/// # 为什么只取 `[dependencies]`
///
/// `[dev-dependencies]` 不参与生产构建，故"测试代码依赖了别的 crate"
/// 不构成架构破口。例如 `tome-source` 的 dev-deps 里有 `zip`（构造测试样本），
/// 那不影响它的生产依赖面。
///
/// # 为什么手工解析而不是引 toml crate
///
/// 目标格式极简（`name = ...` 每行一条）。引一个 TOML 解析器只为读几行
/// 会让这个测试文件的依赖变重——而它恰恰是**守卫依赖边界**的测试，
/// 自己引入新依赖显得讽刺。
fn workspace_deps(crate_name: &str) -> Vec<String> {
    let manifest = repo_root()
        .join("crates")
        .join(crate_name)
        .join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("应能读 {}：{error}", manifest.display()));

    let mut deps = Vec::new();
    let mut in_dependencies = false;

    for line in text.lines() {
        let trimmed = line.trim();

        // 段落切换：进入/离开 `[dependencies]`
        if trimmed.starts_with('[') {
            in_dependencies = trimmed == "[dependencies]";
            continue;
        }
        if !in_dependencies {
            continue;
        }
        // 注释行跳过（注释里提到 crate 名不该算依赖）
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        // 形如 `tome-core = { workspace = true }` 或 `tome-net.workspace = true`
        let name = trimmed.split(['=', '.']).next().unwrap_or_default().trim();
        if name.starts_with("tome-") {
            deps.push(name.to_string());
        }
    }

    deps.sort();
    deps.dedup();
    deps
}

/// 全部引擎 crate（SC-011/FR-039 的"引擎"范围）。
const ENGINE_CRATES: &[&str] = &[
    "tome-core",
    "tome-rule",
    "tome-js",
    "tome-net",
    "tome-store",
    "tome-source",
    "tome-local",
    "tome-cli",
];

// ---------------------------------------------------------------------------
// ① `tome-core` 独立
// ---------------------------------------------------------------------------

/// `tome-core` 不依赖任何其它 workspace crate——它是地基。
///
/// # 为什么这条最要紧
///
/// core 定义接缝 trait 与共享值对象，**所有人依赖它**。若它反过来依赖
/// 了某个实现 crate（比如为了用 `SourceError` 而依赖 `tome-source`），
/// 就形成环——而 Cargo 会允许那个环（只要不是同一个 crate 自依赖），
/// 于是"core 变更不应波及全员"这条设计约束会在无人察觉中失效。
#[test]
fn tome_core_不依赖其它_workspace_crate() {
    let deps = workspace_deps("tome-core");
    assert!(
        deps.is_empty(),
        "tome-core 是地基，不该依赖任何 tome-* crate，实际：{deps:?}"
    );
}

/// `cargo build -p tome-core` 能在**不构建上层**时独立通过。
///
/// # 本测试与上一条的分工
///
/// 上一条看**依赖表**（静态、快），这一条看**能不能真的单独构建**（动态）。
/// 后者能抓到前者抓不到的问题：比如某个 `#[cfg]` 条件下的路径引用了
/// 上层类型，或 build script 需要上层产物。
///
/// # 为什么用 `cargo build` 而不是直接调 rustc
///
/// 我们要证的是"用户能这样用它"——而用户的用法就是 `cargo build -p`。
/// 直接调 rustc 会绕开 Cargo 的 feature 解析与 workspace 装配，测的
/// 就不是那条真实路径了。
#[test]
fn tome_core_可独立构建() {
    let output = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "tome-core", "--quiet"])
        .current_dir(repo_root())
        .output()
        .expect("应能执行 cargo build");

    assert!(
        output.status.success(),
        "`cargo build -p tome-core` 应独立通过（FR-039：core 不依赖上层）\n\
         stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---------------------------------------------------------------------------
// ② 星型无环
// ---------------------------------------------------------------------------

/// `tome-js` / `tome-net` / `tome-store` 三者互不依赖（星型无环）。
///
/// # 唯一被显式放行的例外
///
/// `tome-store → tome-net`（T064：`http_log` 落点要实现 `HttpLogSink`）。
/// 见模块文档：方向仍单向，无环。
///
/// 放行方式是**逐个写明**而非"忽略这一对"——若哪天 `tome-net` 反过来
/// 依赖 `tome-store`（那才是有环），这条测试照样会红。
#[test]
fn 实现_crate_呈星型无环() {
    // 每个 crate 允许依赖的**实现 crate**（core 之外的）
    let allowed: &[(&str, &[&str])] = &[
        // js 只依赖 core（宿主能力经接缝注入）
        ("tome-js", &[]),
        // net 只依赖 core
        ("tome-net", &[]),
        // store 依赖 net（HttpLogSink 的落点，见模块文档）
        ("tome-store", &["tome-net"]),
    ];

    let mut violations: Vec<String> = Vec::new();

    for (crate_name, permitted) in allowed {
        for dep in workspace_deps(crate_name) {
            // 允许 core（所有实现 crate 都依赖它）
            if dep == "tome-core" {
                continue;
            }
            if !permitted.contains(&dep.as_str()) {
                violations.push(format!("{crate_name} → {dep}"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "实现 crate 应呈星型无环（宪章 IV）：{violations:?}\n\
         允许的实现依赖（core 之外）：{{\"tome-js\": [], \"tome-net\": [], \
         \"tome-store\": [\"tome-net\"]}}"
    );
}

/// `tome-net` 不依赖 `tome-store`——补齐上一条的反向检查。
///
/// # 为什么单独一条
///
/// 上一条只查"某个 crate 依赖了不该依赖的"。而 `tome-store → tome-net`
/// 是**允许**的，故上一条不会阻止有人在 `tome-net` 里也加一行
/// `tome-store` ——那会形成 `net ↔ store` 的**环**。
///
/// 环是宪章 IV 真正禁止的东西（比"多依赖一个"严重：它让两个 crate
/// 无法独立演进与独立编译）。
#[test]
fn net_与_store_不构成环() {
    let net_deps = workspace_deps("tome-net");
    assert!(
        !net_deps.contains(&"tome-store".to_string()),
        "tome-net 不该依赖 tome-store（会与 store → net 构成环）：{net_deps:?}"
    );
}

/// `tome-rule` 只依赖 `tome-core` 与 `tome-js`。
///
/// # 为什么 rule 可以依赖 js 而其它实现 crate 不行
///
/// 规则引擎要执行规则里的 `@js:` 片段与 `{{}}` 插值（FR-004/FR-005），
/// 那是它的**功能需求**而非偶然耦合。方向仍是单向
/// （`rule → js → core`），无环。
#[test]
fn tome_rule_的依赖面受限() {
    let deps = workspace_deps("tome-rule");
    let allowed = ["tome-core", "tome-js"];
    for dep in &deps {
        assert!(
            allowed.contains(&dep.as_str()),
            "tome-rule 只该依赖 {allowed:?}，实际多出 `{dep}`（全部：{deps:?}）"
        );
    }
    // 且确实依赖 js（否则 @js: 规则跑不了）
    assert!(
        deps.contains(&"tome-js".to_string()),
        "tome-rule 应依赖 tome-js（规则里的 @js: 需要它）：{deps:?}"
    );
}

// ---------------------------------------------------------------------------
// ③ 引擎不依赖上层
// ---------------------------------------------------------------------------

/// 引擎 crate 不依赖 `tome-reader` / `tome-app`（FR-039）。
///
/// # FR-039 的实质
///
/// > 上层实现被替换时引擎无需改动
///
/// 若引擎依赖了阅读器，"替换上层"就要改引擎——那条要求直接落空。
/// 故这是结构性禁令，不是风格偏好。
#[test]
fn 引擎不依赖阅读器与界面层() {
    /// 上层 crate（引擎绝不能依赖它们）
    const UPPER_LAYERS: &[&str] = &["tome-reader", "tome-app", "tome-extra"];

    let mut violations: Vec<String> = Vec::new();
    for crate_name in ENGINE_CRATES {
        for dep in workspace_deps(crate_name) {
            if UPPER_LAYERS.contains(&dep.as_str()) {
                violations.push(format!("{crate_name} → {dep}"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "引擎不该依赖阅读器/界面层（FR-039）：{violations:?}"
    );
}

/// `tome-reader` 只依赖 `tome-core`（S3/S2 约定的单向关系）。
///
/// # 为什么这条在 001 的验收里
///
/// 它把"引擎 ↔ 上层"的单向关系**从两侧同时钉住**：引擎不依赖 reader
/// （上一条），reader 只依赖 core（这一条）。两侧都成立时，"替换上层
/// 无需改引擎"才是结构性事实而非当前状态。
///
/// **本测试只读 Cargo.toml，不改 `tome-reader` 任何文件**。
#[test]
fn tome_reader_只依赖_core() {
    let deps = workspace_deps("tome-reader");
    let extra: Vec<&String> = deps
        .iter()
        .filter(|dep| dep.as_str() != "tome-core")
        .collect();
    assert!(
        extra.is_empty(),
        "tome-reader 只该依赖 tome-core（宪法 IV），实际多出：{extra:?}"
    );
}

// ---------------------------------------------------------------------------
// 守卫自身的有效性
// ---------------------------------------------------------------------------

/// 依赖表解析器本身是对的（否则上面所有断言都是空转）。
///
/// # 为什么这条不能省
///
/// 上面每条断言都以 `workspace_deps` 的输出为前提。若解析器坏了
/// （比如段落识别写错，永远返回空表），**所有断言都会静默通过**——
/// 一个永远绿的守卫比没有守卫更糟（它给出虚假的安全感）。
///
/// 故这条用**已知的非空依赖**验证解析器真的能读出东西。
#[test]
fn 依赖表解析器有效() {
    // `tome-source` 的依赖面是已知的、非空的
    let source_deps = workspace_deps("tome-source");
    for expected in [
        "tome-core",
        "tome-net",
        "tome-store",
        "tome-js",
        "tome-rule",
    ] {
        assert!(
            source_deps.contains(&expected.to_string()),
            "解析器应读出 tome-source → {expected}（实际：{source_deps:?}）"
        );
    }

    // 反向验证：core 的依赖面应为空（若解析器"读什么都是空"，
    // 上一条会红——两条一起构成对解析器有效性的双向约束）
    assert!(
        workspace_deps("tome-core").is_empty(),
        "tome-core 不该有 workspace 依赖"
    );
}
