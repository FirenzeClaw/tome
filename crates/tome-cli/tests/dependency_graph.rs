//! `\[DONE]` 依赖图断言（tasks.md T096 / FR-039、宪章 IV）
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
//! | ③ 引擎不依赖上层 | 实现 crate（不含组合根 `tome-cli`）不依赖 `tome-reader`/`tome-app` |
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
///
/// ## 为什么没有 `tome-cli`
///
/// `tome-cli` 是**组合根**，不是引擎：它按定义要装配全部实现 crate
/// （001 的 `contracts/cli.md` §3 已声明"它是组合根，依赖全部 crate"）。
/// 把它算进"不得依赖上层"的集合里，结论就恒定为"不可依赖任何东西"，
/// 与它唯一的职责直接冲突。
///
/// S2 的 `tome layout` 把这条冲突摆到了明面上（`tome-cli → tome-reader`）：
/// 排版层的可验证入口按 002 的 `contracts/cli.md` §2 就是 CLI 薄壳，
/// 而 002 规格 §3 关系表同时写明这**不违反**架构铁律。
///
/// 故此处把边界画在真正该在的地方——**被判定的"引擎"是除组合根以外的
/// 实现 crate**。组合根自身的约束仍被 `tome_reader_只依赖_core` 与
/// `实现_crate_呈星型无环` 两条从侧面钉住。
///
/// ## 组合根不止一个（S3 起）
///
/// S3 的 `tome-app` 是**第二个组合根**：它按 plan.md 的依赖图装配
/// `tome-reader` 的排版渲染面（阅读视图是它的全部存在理由），
/// 与 `tome-cli` 同一授权、同一条理由。故它同样不在此表里——
/// 「哪些 crate 算组合根」由 [`COMPOSITION_ROOTS`] 显式登记，
/// 并由 `依赖_tome_reader_的_crate_必须登记为组合根` 那条守住：
/// 新出现的 `→ tome-reader` 依赖**不能**靠"忘记加进本表"蒙混过关。
const ENGINE_CRATES: &[&str] = &[
    "tome-core",
    "tome-rule",
    "tome-js",
    "tome-net",
    "tome-store",
    "tome-source",
    "tome-local",
];

/// 组合根登记表：**合法**依赖 `tome-reader` 的 crate。
///
/// ## 为什么要有这张显式的表
///
/// 「引擎不得依赖上层」这条断言（`引擎不依赖阅读器与界面层`）的成立，
/// 一半靠引擎的自觉，另一半靠**组合根没有被误算进引擎名单**。后者此前
/// 是隐式的：`ENGINE_CRATES` 里没有 `tome-cli`，于是它不被检查——
/// 但"没写进去"既可能是**有意的授权**（组合根），也可能是**忘记加**。
/// 两者在代码上长得一模一样，而后果相反：前者正确，后者是守卫的盲区。
///
/// 这张表把它变成可判定的：**凡依赖 `tome-reader` 的 crate，必须在此登记**；
/// 未登记的会由 `依赖_tome_reader_的_crate_必须登记为组合根` 抓出来，
/// 而不是悄悄从引擎名单里溜过去。
///
/// ## 为什么登记表本身也要被反向检查
///
/// 一张可以随意填写而无人核对的表会退化成空转：把某个引擎 crate 写进来，
/// 它立刻就不再受「不得依赖上层」的约束。故那条测试同时断言：
/// 表里每一项**真的存在**、**真的依赖 `tome-reader`**、且**不在**
/// [`ENGINE_CRATES`] 里——三个条件一起，才让"登记"这件事有代价。
const COMPOSITION_ROOTS: &[&str] = &["tome-cli", "tome-app"];

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
///
/// **不含 `tome-cli`**（组合根按定义依赖全部 crate，理由见 [`ENGINE_CRATES`]）。
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

/// 会经 `default` 拉进图形栈的 workspace 表条目必须 `default-features = false`。
///
/// # 这条守的是什么（S3 T005 实测抓到的静默失效）
///
/// `tome-reader` 的 `default = ["render"]` 与 `tome-cli` 的
/// `default = ["reader-render"]` 都会（直接或转发）打开 `tome-reader/render`
/// ——连 wgpu 一起。而 **workspace 表里的引用方**若用默认 feature 引用它们，
/// 图形栈就会被拉进任何依赖它的构建。
///
/// 2026-10-11 S3 T005 踩到的具体形态：`tome-app` 为复用 CLI 的 PNG 编码器
/// 加了一条 dev-dependency（`tome-cli = { workspace = true }`），于是
/// `cargo test --workspace --no-default-features`——本应是"渲染面在编译期
/// 不存在"的那道门——**悄悄把 render 面装了回去**。
///
/// 它的可怕之处是**全绿**：测试总数从 3049 涨回 3197，看起来只是"多跑了些
/// 测试"，没有任何东西变红。若不是逐项对数，这道门会一直以为自己还在守。
///
/// 故此处直接断言**机制**（表里必须写着 `default-features = false`），
/// 而不是断言某个可观测后果——后果可能被别处的 feature 并集掩盖。
#[test]
fn graphics_pulling_workspace_entries_disable_default_features() {
    let manifest = repo_root().join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("应能读 {}：{error}", manifest.display()));

    // 会经 default 打开 tome-reader/render 的 crate
    let graphics_pulling = ["tome-reader", "tome-cli"];

    let mut in_workspace_deps = false;
    let mut checked: Vec<String> = Vec::new();
    let mut violations: Vec<String> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_workspace_deps = trimmed == "[workspace.dependencies]";
            continue;
        }
        if !in_workspace_deps || trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let name = key.trim();
        if !graphics_pulling.contains(&name) {
            continue;
        }
        checked.push(name.to_owned());
        if !value.contains("default-features = false") {
            violations.push(format!("{name} = {value}"));
        }
    }

    assert_eq!(
        checked.len(),
        graphics_pulling.len(),
        "应在 workspace 表里找到 {graphics_pulling:?} 两条（实测 {checked:?}）——         若某条被移走，本断言会红，提示这里该同步更新"
    );
    assert!(
        violations.is_empty(),
        "下列 workspace 表条目会经 default 把图形栈拉进依赖树（S3 T005 实测的         静默失效形态：headless 门照样全绿但 render 面已装回）：{violations:?}"
    );
}

/// 依赖 `tome-reader` 的 crate **必须**登记为组合根（S3 T002）。
///
/// # 这条补的是什么漏洞
///
/// `引擎不依赖阅读器与界面层` 的判据是「`ENGINE_CRATES` 里的 crate 不得依赖
/// 上层」。它默认 `ENGINE_CRATES` 是**完备**的——可这张表是手写的。
/// 于是一个新 crate 若既依赖 `tome-reader`、又忘了（或刻意不）写进引擎名单，
/// 就同时躲开了这条断言与星型无环断言，架构在无人察觉中破掉。
///
/// 本测试把「依赖 `tome-reader`」与「登记为组合根」绑成充要关系：
///
/// - **正向**：凡依赖 `tome-reader` 的 crate，必须在 [`COMPOSITION_ROOTS`] 里
/// - **反向**：登记表里每一项都必须**真的**依赖 `tome-reader`
///   （防"填进表里就豁免"的滥用），且不得同时出现在 [`ENGINE_CRATES`] 里
///   （防一个 crate 两边都算，把约束稀释成空转）
///
/// 组合根的**成员资格**因此是显式且有代价的，而不是"没被写进引擎名单"。
///
/// # 为什么扫整个 workspace 而不是固定名单
///
/// 固定名单会重蹈本测试要防的那个错：新 crate 不在名单里就查不到。
/// 故从根 `Cargo.toml` 的 `[workspace] members` 取全集——**新成员自动进入
/// 检查范围**，"忘记登记"于是变成一条会红的测试而不是一个静默的盲区。
#[test]
fn 依赖_tome_reader_的_crate_必须登记为组合根() {
    let mut violations: Vec<String> = Vec::new();

    for crate_name in workspace_members() {
        let depends_on_reader = workspace_deps(&crate_name)
            .iter()
            .any(|dep| dep == "tome-reader");
        let registered = COMPOSITION_ROOTS.contains(&crate_name.as_str());

        if depends_on_reader && !registered {
            violations.push(format!(
                "{crate_name} 依赖 tome-reader 却未登记为组合根——\
                 若它确是组合根请加入 COMPOSITION_ROOTS，否则这是架构破口"
            ));
        }
        if registered && !depends_on_reader {
            violations.push(format!(
                "{crate_name} 登记为组合根却并不依赖 tome-reader——\
                 登记表不该收录无此项职责的 crate"
            ));
        }
        if registered && ENGINE_CRATES.contains(&crate_name.as_str()) {
            violations.push(format!(
                "{crate_name} 同时在 COMPOSITION_ROOTS 与 ENGINE_CRATES 里——\
                 一个 crate 只能是其中一种，两边都算等于把约束稀释掉"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "组合根登记表与实际依赖不一致（S3 T002）：{violations:?}\n\
         当前登记：{COMPOSITION_ROOTS:?}"
    );
}

/// 读根 `Cargo.toml` 的 `[workspace] members`，返回 crate 名（末段目录名）。
///
/// # 为什么从根清单取而不是硬编码
///
/// 见 `依赖_tome_reader_的_crate_必须登记为组合根` 的说明：硬编码名单会让
/// 新成员成为守卫盲区，而那正是该测试要消除的东西。
///
/// # 为什么用目录末段而不是 `[package] name`
///
/// `members` 里写的是路径（`crates/tome-app`），而本文件其余部分的
/// [`workspace_deps`] 也按**目录名**拼 `Cargo.toml` 路径。取末段让两者
/// 用同一套命名，不必再解析每个 crate 的 `[package] name`
/// （当前 workspace 里两者一致，且有 `workspace_成员解析器有效` 兜底）。
///
/// # 为什么不能"进了 `[workspace]` 就开始收"
///
/// 该段里 `members` 之前还有 `resolver = "2"`——把它当成成员路径会拼出
/// `crates/resolver = "2/Cargo.toml` 这种路径（首轮实现实测报 os error 123）。
/// 故收集必须**由 `members = [` 显式开启**，而不是由段名隐含。
fn workspace_members() -> Vec<String> {
    let manifest = repo_root().join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|error| panic!("应能读 {}：{error}", manifest.display()));

    let mut members = Vec::new();
    let mut in_workspace = false;
    let mut collecting = false;

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') {
            in_workspace = trimmed == "[workspace]";
            collecting = false;
            continue;
        }
        if !in_workspace {
            continue;
        }

        // `members = [` 开启清单；`]` 收尾。单行形式 `members = ["a", "b"]`
        // 一并支持（同一行的方括号成对出现时直接就地解析）。
        if trimmed.starts_with("members") {
            let (_, rest) = trimmed.split_once('=').unwrap_or(("", ""));
            if rest.contains('[') && rest.contains(']') {
                members.extend(quoted_paths(rest));
                collecting = false;
            } else {
                collecting = rest.contains('[');
            }
            continue;
        }
        if collecting {
            if trimmed.starts_with(']') {
                collecting = false;
                continue;
            }
            members.extend(quoted_paths(trimmed));
        }
    }

    members.sort();
    members.dedup();
    members
}

/// 从一段文本里取出所有被引号包住的路径的**末段**。
fn quoted_paths(text: &str) -> Vec<String> {
    text.split('"')
        .skip(1)
        .step_by(2)
        .filter_map(|quoted| quoted.rsplit('/').next())
        .map(str::to_string)
        .collect()
}

/// workspace 成员解析器有效（否则上一条断言是空转）。
///
/// # 为什么这条不能省
///
/// 与 `依赖表解析器有效` 同一理由：`workspace_members` 若因格式变化永远返回
/// 空表，`依赖_tome_reader_的_crate_必须登记为组合根` 会**静默全绿**——
/// 一个永远绿的守卫比没有守卫更糟。故此处用已知成员做正向断言，
/// 并断言 `tome-app`（S3 新增成员）在列。
#[test]
fn workspace_成员解析器有效() {
    let members = workspace_members();

    for expected in ["tome-core", "tome-reader", "tome-cli", "tome-app"] {
        assert!(
            members.contains(&expected.to_string()),
            "workspace members 应含 {expected}（实际：{members:?}）"
        );
    }
    assert!(
        members.len() >= 10,
        "workspace 至少有 10 个成员（S3 后），实际 {}：{members:?}",
        members.len()
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
