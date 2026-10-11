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
//! # 两条裁决，各自都有"当初怎么逃掉的"记录
//!
//! 1. **能力缺失压过用法错误**：参数校验散在选项循环、子命令分支、循环兜底
//!    三处，任一处先触发就把退出码变成 2。故用参数形态**矩阵**锁住
//!    （当初只断言了无参形态，而它在三种形态里只覆盖了一种）。
//! 2. **能力错误不经装配**：首版短路伪造 `Run` 并丢弃用户的全局选项，
//!    装配照常发生。当初的验证用 `--data-dir` 构造"坏数据目录"——那个选项
//!    被丢弃，装配实际打的是默认目录（本机可用），于是看起来全对。
//!    **用一个被丢弃的选项做断言，测了个寂寞。** 故现在的断言改成阻断
//!    **默认**数据目录（`APPDATA`/`HOME`/`XDG_DATA_HOME`），并在注释里
//!    写明这次验的是机理而非结果。
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
    let data_dir = tempfile::tempdir().expect("临时数据目录");
    run_cli_with_data_dir(args, data_dir.path())
}

/// 同上，但显式指定数据目录（用于构造**必然装配失败**的场景）。
fn run_cli_with_data_dir(args: &[&str], data_dir: &std::path::Path) -> (i32, String, String) {
    let binary = env!("CARGO_BIN_EXE_tome-cli");
    let output = Command::new(binary)
        .arg("--data-dir")
        .arg(data_dir)
        .args(args)
        .output()
        .expect("应能启动 tome-cli");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// 在**系统数据目录被阻断**的环境下跑一次，返回 `(退出码, stdout, stderr)`。
///
/// # 为什么阻断"系统数据目录"而不是"`--data-dir`"
///
/// 首版短路的破绽正在这里：它伪造 `Run { options: GlobalOptions::default() }`，
/// **把用户给的 `--data-dir` 丢掉了**，于是装配打的是**默认（系统）数据目录**。
/// 本机上那个目录可用，测试就通过了——用一个被丢弃的选项做断言，测了个寂寞。
///
/// 故本函数不用 `--data-dir`，而是把"默认数据目录"这个落点本身堵死：把
/// 平台对应的环境变量指向一个**已存在的普通文件**，`create_dir_all` 必失败。
/// 于是只要进程真的去装配，就必然报 `error.storage`——把"不经装配"从注释
/// 承诺变成可证伪的断言。
///
/// 三个变量分别对应三平台（见 `tome_store::datadir::system_data_dir`）：
///
/// | 变量 | 平台 | 默认落点 |
/// |---|---|---|
/// | `APPDATA` | Windows | `%APPDATA%\tome` |
/// | `HOME` | macOS | `$HOME/Library/Application Support/tome` |
/// | `XDG_DATA_HOME`（优先）/ `HOME` | Linux | `$XDG_DATA_HOME/tome` 或 `~/.local/share/tome` |
///
/// 三个都设成阻断路径，故三平台都成立。
fn run_cli_with_blocked_system_data_dir(args: &[&str]) -> (i32, String, String) {
    let binary = env!("CARGO_BIN_EXE_tome-cli");
    let dir = tempfile::tempdir().expect("临时目录");
    // 一个**普通文件**——任何 `create_dir_all` 到它下面的尝试都必失败。
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("应能写");

    let output = Command::new(binary)
        .args(args)
        .env("APPDATA", &blocker)
        .env("HOME", &blocker)
        .env("XDG_DATA_HOME", &blocker)
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

/// 渲染面命令的能力错误**不依赖装配成功**（T056 C1 的核心判据）。
///
/// # 这条防的是"退出码取决于本机 IO 运气"
///
/// 首版修复只把 `render` 纳入 parse 短路，`fonts` 仍走"装配组合根 → 分发
/// → 能力错误"的旧路。那让 `fonts` 的退出码**取决于装配成不成**：
///
/// - 装配成功（常态）→ 退 3，看起来正确
/// - 装配失败（数据目录写探测撞上杀软瞬时占用）→ 退 1 + 裸 `error.storage`
///
/// 同一个命令、同一次构建，两种退出码，差别只在"这次 IO 顺不顺"——那不是
/// 契约，是运气。PM 机上 8 轮中 2 轮复现的失败正是这个形态。
///
/// # 怎么构造"必然装配失败"
///
/// `--data-dir` 指向一个**已存在的普通文件**：`create_dir_all` 必失败，
/// 装配必然报 `error.storage`。于是本测试把"装配失败"这一分支**确定性地**
/// 摆到台面上——不需要碰运气等杀软来干扰。
///
/// 判据：`render` / `fonts` 仍退 3 + 能力标识（短路早于任何 IO）；
/// 而 `layout`（真的要走装配）如实退 1 并报 `error.storage`——后者同时
/// 证明"坏数据目录"这个前提真的生效了，测试没有空跑。
#[test]
fn 渲染面命令的能力错误不依赖装配成功() {
    // 前提自检：**系统数据目录被阻断**时 `layout`（真的要走装配）确实失败。
    // 这一步让"阻断生效"这件事本身可证伪——否则下面可能只是测了个没生效的环境。
    let (layout_code, _stdout, layout_stderr) = run_cli_with_blocked_system_data_dir(&["layout"]);
    assert_eq!(
        layout_code, 1,
        "系统数据目录被阻断时 layout 应装配失败退 1（本测试的前提）；stderr：{layout_stderr}"
    );
    assert!(
        layout_stderr.contains("error.storage"),
        "前提自检：应报 error.storage，实际：{layout_stderr}"
    );

    // 正题：渲染面两命令在**装配必然失败**的前提下仍退 3 + 能力标识，
    // 即它们的错误路径根本不经过装配。
    for subcommand in ["render", "fonts"] {
        let (code, _stdout, stderr) = run_cli_with_blocked_system_data_dir(&[subcommand]);
        assert_eq!(
            code, 3,
            "{subcommand} 的能力错误不该依赖装配是否成功（应退 3）；stderr：{stderr}"
        );
        assert!(
            stderr.contains(CAPABILITY_ID),
            "{subcommand} 应携带能力标识 `{CAPABILITY_ID}`，实际：{stderr}"
        );
        assert!(
            !stderr.contains("error.storage"),
            "{subcommand} 不该报存储错误——那说明它走了装配路径；stderr：{stderr}"
        );
    }
}

/// 防再造：短路的判据必须是**默认数据目录**，而不是用户给的 `--data-dir`。
///
/// # 这条防的是首版短路的原始破绽（PM 二次打回）
///
/// 首版短路返回 `Run { options: GlobalOptions::default() }`，**丢掉用户的全局
/// 选项**，装配于是打默认数据目录。当时的"决定性验证"用 `--data-dir` 指向
/// 坏路径——那个选项被丢掉了，装配仍打默认目录（本机可用），于是退 3，
/// 看起来全对。**用一个被丢弃的选项做断言，测了个寂寞。**
///
/// 本测试反过来：**不传** `--data-dir`，改把默认数据目录的落点堵死。
/// 若实现回退成"伪造 Run + 丢弃选项"，装配必然撞上阻断路径而报
/// `error.storage` → 本测试变红。
///
/// # 为什么带一个会被丢弃的 `--data-dir` 一起跑
///
/// 同时传 `--data-dir <可用临时目录>`：即便实现把选项捡回来用了，阻断的
/// 是**默认**目录——但只要它真的不经装配，两者都不该被碰。这条同时覆盖
/// "选项被丢弃"与"选项被使用"两种实现的错误形态。
#[test]
fn 短路判据不依赖被丢弃的_data_dir_选项() {
    // 对照：先确认这个临时目录本身是**可用**的——否则下面的断言可能只是
    // 因为目录坏，而不是因为"没装配"。
    let usable = tempfile::tempdir().expect("临时数据目录");
    let (ok_code, _stdout, _stderr) = run_cli_with_data_dir(&["layout", "--text"], usable.path());
    assert_ne!(
        ok_code, 3,
        "对照：可用数据目录下 layout 不该退 3（否则本测试的前提不成立）"
    );

    // 正题：默认数据目录被阻断 + 同时给一个可用的 `--data-dir`。
    let binary = env!("CARGO_BIN_EXE_tome-cli");
    let dir = tempfile::tempdir().expect("临时目录");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("应能写");

    for subcommand in ["render", "fonts"] {
        let output = Command::new(binary)
            .arg("--data-dir")
            .arg(usable.path())
            .arg(subcommand)
            .env("APPDATA", &blocker)
            .env("HOME", &blocker)
            .env("XDG_DATA_HOME", &blocker)
            .output()
            .expect("应能启动 tome-cli");
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(
            code, 3,
            "{subcommand} 应退 3（不经装配，两个数据目录都不该被碰）；stderr：{stderr}"
        );
        assert!(
            !stderr.contains("error.storage"),
            "{subcommand} 报了存储错误 → 说明它装配了（默认目录被阻断或 --data-dir 被用上）；\
             stderr：{stderr}"
        );
    }
}

/// 防再造（结构断言）：处理能力不可用的分支**自己**不装配、不调 `execute`。
///
/// # 为什么还要一条源码结构断言
///
/// 上面两条是行为断言，已足以覆盖三平台的真实形态。本条是**廉价的哨兵**：
/// 行为测试要跑起子进程才能发现"又走了装配"，本断言在源码层面直接盯住
/// 那个分支体——不需要任何平台条件，跑得也快。
///
/// # 为什么断言的是"分支体内不含装配"而不是"分支排在前面"
///
/// `Run` 与 `CapabilityUnavailable` 是**互斥的枚举变体**，match 的分支顺序
/// 不可能造成误分派——断言顺序是在断言一件不存在的事（初版这么写，实测
/// 直接失败）。真正要守的性质是：**处理能力不可用的那段代码里没有装配**。
/// 故这里切出该变体名到下一个分支之间的源码，扫它有没有 `assemble`/`execute`。
///
/// 取舍：源码断言脆（改写法就可能失效），故它**不是**主要防线——主要防线
/// 是上面的行为断言（阻断默认数据目录的那两条）。这里只当"更快、更直白的
/// 第二道"。
#[test]
fn dispatch_的能力不可用分支自己不装配() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    )
    .expect("应能读 main.rs");

    let arm_start = source
        .find("ParseOutcome::CapabilityUnavailable")
        .expect("main.rs 应显式处理 CapabilityUnavailable 分支");

    // 切到**本分支的结尾**（`}` 收尾后紧跟 match 的闭合）。
    // 不能切到"下一个 `ParseOutcome::`"——本变体是最后一个分支，那样会一路
    // 吞到文件末尾，把后面的 `execute` 函数也算进来（初版踩到，实测误报）。
    //
    // 判据：从分支起点起，找第一个"顶格（缩进 0）的 `}`"——那是 match 语句
    // 本身的闭合，即 dispatch 函数体的结束边界。
    let arm_end = source[arm_start..]
        .find("\n}")
        .map(|offset| arm_start + offset + 1)
        .unwrap_or(source.len());
    let arm_body = &source[arm_start..arm_end];

    // 先剥注释再扫：本分支的**注释里**本来就在讲"为什么不走 execute"，
    // 直接扫原文会把说明文字当成代码（初版踩到，实测立刻误报）。
    // 只扫代码行，注释行整行丢弃。
    let code_only: String = arm_body
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !code_only.contains("execute("),
        "能力不可用分支不该调 execute（那会装配组合根）；代码：\n{code_only}"
    );
    assert!(
        !code_only.contains("assemble"),
        "能力不可用分支不该装配组合根；代码：\n{code_only}"
    );
    assert!(
        code_only.contains('3'),
        "该分支应以退出码 3 结束（不支持能力）；代码：\n{code_only}"
    );
}

/// 装配失败时，stderr 必须带上**诊断细节**（T056 C2 的可观测面）。
///
/// # 为什么这条与能力错误同处一个文件
///
/// 两者是同一场景的两面：坏数据目录下，装配失败的命令要能**说清楚**
/// 为什么失败。此前 `root.rs` 在装配的每个失败点 `.kind().clone()`，把
/// `with_detail` 刚写进去的原因丢掉，用户只看到一行裸 `error.storage`——
/// 那是"有错误信息但无法定位"的形态，排查成本全落在用户身上。
///
/// 判据：`error.storage` 标识仍在（可本地化标识前缀不变，契约断言语义
/// 不受影响），且后面跟有具体原因（不只是 id 本身）。
#[test]
fn 装配失败时错误带诊断细节() {
    let dir = tempfile::tempdir().expect("临时目录");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("应能写");

    let (code, _stdout, stderr) = run_cli_with_data_dir(&["layout"], &blocker);
    assert_eq!(code, 1, "应退 1（业务错误）；stderr：{stderr}");

    // ① 标识前缀不变（`ClassifiedError::message_id` 仍是 error.storage）
    assert!(
        stderr.contains("error.storage"),
        "应保留 error.storage 标识便于日志检索；stderr：{stderr}"
    );

    // ② 细节确实带出来了：不能只有那一行裸标识。
    //    判据用"含冒号分隔的后续内容"——`error.storage: <原因>` 是
    //    `ClassifiedError` 的 Display 契约。
    let detailed_line = stderr
        .lines()
        .find(|line| line.contains("error.storage"))
        .unwrap_or_default();
    assert!(
        detailed_line.len() > "error.storage".len() + 2,
        "error.storage 之后应跟有诊断细节（此前被 .kind() 丢掉）；实际整行：{detailed_line}"
    );
}
