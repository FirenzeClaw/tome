//! `[DONE]` headless 形态下 `render` / `fonts` 的能力错误契约（T056 / FR-053、FR-057、FR-058）
//!
//! # 这条测试防的是什么
//!
//! `--no-default-features` 构建里渲染面在**编译期**不存在，于是 `render` /
//! `fonts` 这两个命令根本不存在。对不存在的命令，`contracts/cli.md` §2.3 要求
//! 回**退出码 3 + `offscreenRendering` 能力标识**（FR-058：结构化、可捕获、
//! 不硬编码自然语言）。
//!
//! 本文件锁住的那条裁决：**能力缺失压过用法错误**。CI 的 headless job 曾实测
//! `render`（无参）退出码为 2——参数校验先于能力检查，于是"命令不存在"被
//! 报成了"参数写错了"。而 `fonts`（无参）恰好没有必需参数，直接走到分发点退 3。
//! 两条对称的命令给出不同退出码，取决于各自**恰好有没有必需参数**——那不是
//! 契约，是参数校验顺序的副产物。
//!
//! # 为什么用例要覆盖"参数不合法"的形态
//!
//! 只测无参形态会漏掉真正的一般性：`--out` 缺失在子命令分支里报错、`--theme`
//! 白名单在选项循环里报错、未知选项在循环兜底里报错——**三个不同的失败点**。
//! 只要任一处先触发，退出码就变 2。故本文件用参数形态的**矩阵**断言，让
//! "无论参数是否合法"这句裁决真的被逐点锁住（这正是本缺陷当初逃过验收的原因：
//! 当时只断言了无参形态，而它恰好在三种形态里只覆盖了一种）。
//!
//! # 为什么跑真实二进制而不是调 `parse`
//!
//! 与 `render_cli.rs` 同一口径：本文件断言的是**对外的进程行为**（退出码 +
//! stderr 上的能力标识）。那是 CI headless job 直接断言的东西，也是脚本
//! 消费的东西——只测库内 `parse()` 会漏掉 `main` 的退出码映射那一层。
//!
//! # 门控：整个文件在 `reader-render` **开**时为空
//!
//! feature 开启时这两条命令**存在**，正确行为是用法错误退 2（由
//! `render_cli.rs::render_requires_out_and_target` 断言）。本文件的断言
//! 在那种构建下恒假，故整份文件门控在 `not(feature = "reader-render")` 上
//! ——与 `render_cli.rs` 的门控方向恰好互为补集。

#![cfg(not(feature = "reader-render"))]
#![expect(
    clippy::expect_used,
    reason = "CLI 契约测试的夹具失败应让测试立即失败，错误传播只会加噪音（与 render_cli.rs 同一约定）"
)]

use std::process::Command;

/// 能力标识（`contracts/cli.md` §2.3）。
///
/// 刻意写字面量而非引用 crate 常量：本文件断言的是**对外契约的那个字符串**
/// （CI 脚本也 grep 同一个字面量）。若哪天常量改了值，这里必须跟着改——
/// 那正是我们想被提醒的事。
const CAPABILITY_ID: &str = "offscreenRendering";

/// 跑一次真实二进制，返回 `(退出码, stdout, stderr)`。
fn run_cli(args: &[&str]) -> (i32, String, String) {
    let binary = env!("CARGO_BIN_EXE_tome-cli");
    let data_dir = tempfile::tempdir().expect("临时数据目录");
    let output = Command::new(binary)
        .arg("--data-dir")
        .arg(data_dir.path())
        .args(args)
        .output()
        .expect("应能启动 tome-cli");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// `render` 的一切参数形态都应退 3 + 能力标识。
///
/// 矩阵覆盖三个**互不相同**的失败点，以及一个合法形态：
///
/// | 形态 | 若无短路会从哪报错 | 应有退出码 |
/// |---|---|---|
/// | 无参 | 子命令分支的 `--out` 必需校验 | 2 → 须为 3 |
/// | 只给 `--out` | 子命令分支的 `--page`/`--viewport` 校验 | 2 → 须为 3 |
/// | 非法 `--theme` | 选项循环的白名单校验 | 2 → 须为 3 |
/// | 未知选项 | 选项循环的兜底 | 2 → 须为 3 |
/// | 全部合法 | 走到分发点（本就该 3） | 3 |
#[test]
fn render_在_headless_构建下无论参数如何都退_3() {
    let shapes: &[(&str, &[&str])] = &[
        ("无参", &["render"]),
        ("只给 --out（缺目标页）", &["render", "--out", "x.png"]),
        (
            "非法 --theme（选项循环报错）",
            &["render", "--theme", "sepia", "--page", "0"],
        ),
        ("未知选项（循环兜底报错）", &["render", "--bogus-flag"]),
        (
            "全部合法",
            &["render", "--page", "0", "--out", "x.png", "--theme", "day"],
        ),
    ];

    for (description, args) in shapes {
        let (code, _stdout, stderr) = run_cli(args);
        assert_eq!(
            code, 3,
            "render（{description}）应退 3（能力不存在压过用法错误），实际 {code}；stderr：{stderr}"
        );
        assert!(
            stderr.contains(CAPABILITY_ID),
            "render（{description}）的 stderr 应携带能力标识 `{CAPABILITY_ID}`（FR-058），实际：{stderr}"
        );
    }
}

/// `fonts` 的一切参数形态都应退 3 + 能力标识——与 `render` **对称**。
///
/// 对称性是本缺陷的核心：两条命令同属渲染面，能力缺失时不该因"恰好有没有
/// 必需参数"而分岔。`fonts` 本就退 3，但它的 stderr 此前**不带能力标识**
/// （`CommandOutcome::failure` 只写人类可读 message），故同样在断言范围内。
#[test]
fn fonts_在_headless_构建下无论参数如何都退_3() {
    let shapes: &[(&str, &[&str])] = &[
        ("无参", &["fonts"]),
        ("未知选项（循环兜底报错）", &["fonts", "--bogus-flag"]),
        ("带全局选项", &["--format", "json", "fonts"]),
    ];

    for (description, args) in shapes {
        let (code, _stdout, stderr) = run_cli(args);
        assert_eq!(
            code, 3,
            "fonts（{description}）应退 3（与 render 对称），实际 {code}；stderr：{stderr}"
        );
        assert!(
            stderr.contains(CAPABILITY_ID),
            "fonts（{description}）的 stderr 应携带能力标识 `{CAPABILITY_ID}`（FR-058），实际：{stderr}"
        );
    }
}

/// 全局选项可出现在子命令**之前**——短路须跳过它们再认子命令。
///
/// # 为什么单独一条
///
/// `tome --format json render` 里子命令不在 `args[0]`。若短路只认首参，
/// 这个形态会漏回用法错误路径——而它是完全合法的调用方式（全局选项
/// 解析器明确支持前置）。
#[test]
fn 前置全局选项不影响_headless_短路() {
    let (code, _stdout, stderr) = run_cli(&["--format", "json", "render"]);
    assert_eq!(code, 3, "前置全局选项的 render 也应退 3；stderr：{stderr}");
    assert!(
        stderr.contains(CAPABILITY_ID),
        "应携带能力标识 `{CAPABILITY_ID}`，实际：{stderr}"
    );
}

/// 短路**不扩大**：无关命令的用法错误仍是退出码 2。
///
/// # 这条防的是"把短路写成通配"
///
/// 最省事的写法是"headless 下一切非成功都退 3"，那会吃掉全部用法错误，
/// 让 `tome frobnicate`（拼错子命令）得到一个误导性的"能力不支持"。
/// 契约 §1 的 2/3 是**不同**的信号：2 = 你写错了，3 = 本产品没有这项能力。
#[test]
fn 无关命令的用法错误不受短路影响() {
    // 未知子命令
    let (code, _stdout, stderr) = run_cli(&["frobnicate"]);
    assert_eq!(code, 2, "未知子命令应是用法错误；stderr：{stderr}");

    // 纯计算命令缺必需参数：headless 下它**照常可用**（SC-014），
    // 故这里应报用法错误而不是能力错误。
    let (code, _stdout, stderr) = run_cli(&["verify-layout"]);
    assert_eq!(
        code, 2,
        "verify-layout 缺 --corpus 应是用法错误；stderr：{stderr}"
    );
    assert!(
        !stderr.contains(CAPABILITY_ID),
        "纯计算命令不该被误报为渲染能力缺失；stderr：{stderr}"
    );

    // `layout` 是纯计算命令，headless 下全能力——缺正文来源时报的是
    // **业务错误**而非能力错误（`layout` 没有必需选项，无参时它真的去
    // 找正文、找不到才失败）。关键判据是**不退 3**：那才是"短路写成通配"
    // 的痕迹。这里不断言具体是 1 还是 2——那取决于它内部的失败路径，
    // 与本文件要锁的性质无关。
    let (code, _stdout, stderr) = run_cli(&["layout"]);
    assert_ne!(
        code, 3,
        "layout 在 headless 下是全能力（SC-014），不该退 3；stderr：{stderr}"
    );
    assert!(
        !stderr.contains(CAPABILITY_ID),
        "layout 不该被误报为渲染能力缺失；stderr：{stderr}"
    );
}
