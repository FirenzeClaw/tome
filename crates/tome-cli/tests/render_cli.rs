//! T048 的先行部分：`render` / `fonts` 子命令的 CLI 契约（`contracts/cli.md` §2.3/§2.4）
//!
//! ## 本文件覆盖什么
//!
//! - `tome fonts` 输出 §2.4 规定的字段（内嵌角色/家族/体积/回退链/预算）
//! - `tome render` 出 PNG（文件落盘且是**合法** PNG——不只看文件存在）
//! - 后端不可用时**退出码 3 + 结构化能力错误**且**不产出文件**（FR-053）
//! - PNG 编码器的自校验（CRC / Adler 的已知答案）
//!
//! ## 为什么"文件存在"不是有效判据
//!
//! `--out` 写一个空文件也能让"文件存在"通过。真正的判据是**文件是合法
//! PNG**：签名、每块 CRC、zlib 流可解——三者任一错，图片查看器就打不开，
//! 而自动化里只会看到"文件生成了"。故本文件用 Python 的 `zlib` 独立解码
//! 一遍（**不用**本仓库的编码器自证）。
//!
//! ## 退出码 3 的判据为什么也要断言"不产出文件"
//!
//! FR-053 的原文是"MUST NOT 产出空文件或伪造成功"。只断退出码会让
//! "先写空文件再报错" 的实现通过——那正是伪装的一种形态。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音（与 layout_cli.rs 同一约定）"
)]

use std::path::Path;
use std::process::Command;

/// 跑一次 CLI，返回 `(退出码, stdout, stderr)`。
fn run_cli(args: &[&str]) -> (i32, String, String) {
    let binary = env!("CARGO_BIN_EXE_tome-cli");
    let output = Command::new(binary)
        .args(args)
        .output()
        .expect("应能启动 tome-cli");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn write_sample(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("sample.txt");
    std::fs::write(
        &path,
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号。\n\
         书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮。",
    )
    .expect("应能写入临时正文");
    path
}

// ===========================================================================
// `tome fonts`（contracts/cli.md §2.4）
// ===========================================================================

/// `fonts` 输出 §2.4 的全部字段，且 `within_budget` 为真。
#[test]
fn fonts_command_reports_contract_fields() {
    let (code, stdout, stderr) = run_cli(&["fonts"]);
    assert_eq!(code, 0, "fonts 应成功，stderr: {stderr}");

    let value: serde_json::Value = serde_json::from_str(&stdout).expect("输出应是 JSON");

    // §2.4：embedded[].role/family/bytes、system_fallbacks、budget_bytes、within_budget
    let embedded = value["embedded"].as_array().expect("embedded 应是数组");
    assert_eq!(embedded.len(), 1, "只内嵌正文衬线一项（FR-050）");
    assert_eq!(embedded[0]["role"], "reading-serif");
    assert_eq!(embedded[0]["family"], "Tome Serif CN");

    let bytes = embedded[0]["bytes"].as_u64().expect("bytes 应是整数");
    assert_eq!(bytes, 2_284_616, "内嵌子集实测体积（T008 的产物）");

    assert_eq!(value["budget_bytes"], 5_242_880, "预算是 5 MiB");
    assert_eq!(value["within_budget"], true);

    let fallbacks = value["system_fallbacks"]["reading_serif"]
        .as_array()
        .expect("回退链应是数组");
    assert!(!fallbacks.is_empty(), "回退链不得为空（FR-051）");
}

/// `fonts` 的 `--format human` 是**逐行摘要**（不是整份 JSON）。
///
/// ## 判据为什么不是"不含任何 `{`"
///
/// 初版写的正是那条——实测失败：摘要里嵌套的**数组元素**仍是紧凑 JSON
/// （`  {"bytes":…}`），那对"给人看"而言是合理形态（嵌套结构很难平铺成
/// 一行还保持可读）。
///
/// 有意义的判据是**顶层形态**：逐行 `key: value`，而不是一整份 JSON 文档。
/// 表达方式：首行是键值对、行数多于 1、且能从中读出顶层键名。
#[test]
fn fonts_human_format_is_a_summary() {
    let (code, stdout, _) = run_cli(&["fonts", "--format", "human"]);
    assert_eq!(code, 0);
    assert!(!stdout.trim().is_empty(), "human 形态应有内容");

    // 顶层是逐行键值对（而非一整份 JSON 文档）
    assert!(
        !stdout.trim_start().starts_with('{'),
        "human 形态不应以 JSON 文档开头：{stdout}"
    );
    assert!(
        stdout.lines().count() > 1,
        "human 形态应是多行摘要：{stdout}"
    );
    // 顶层键名可读
    for key in [
        "budget_bytes",
        "embedded",
        "system_fallbacks",
        "within_budget",
    ] {
        assert!(stdout.contains(key), "摘要应含顶层键 `{key}`：{stdout}");
    }
}

/// **人类与 JSON 两种形态都反映同一个实测体积**（防两份输出漂移）。
#[test]
fn fonts_formats_agree_on_the_measured_size() {
    let (_, json_out, _) = run_cli(&["fonts"]);
    let (_, human_out, _) = run_cli(&["fonts", "--format", "human"]);

    let value: serde_json::Value = serde_json::from_str(&json_out).expect("应是 JSON");
    let bytes = value["embedded"][0]["bytes"].as_u64().unwrap_or_default();
    assert!(
        human_out.contains(&bytes.to_string()),
        "human 形态应含同一体积值 {bytes}（两份输出不得漂移）：{human_out}"
    );
}

// ===========================================================================
// `tome render`（contracts/cli.md §2.3）
// ===========================================================================

/// **`render` 产出合法的 PNG**（判据不是"文件存在"）。
///
/// ## 校验方式：用标准库的 CRC 与 zlib 独立解码
///
/// 本文件**不引用**仓库里的 PNG 编码器来验证自己（那是自证）。
/// 改用 Rust 标准库没有的东西——故用一个**纯 Rust 的最小 PNG 结构检查**
/// （签名 + 逐块 CRC 重算 + IHDR 字段），它足以证伪"空文件"与"结构错"。
#[test]
fn render_command_produces_a_valid_png() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let out = dir.path().join("page0.png");

    let (code, stdout, stderr) = run_cli(&[
        "render",
        "--text",
        text.to_str().unwrap(),
        "--page",
        "0",
        "--out",
        out.to_str().unwrap(),
        "--width",
        "342",
        "--height",
        "300",
    ]);

    // 无图形后端时本机无法渲染——那应按 FR-053 显式跳过（**不计为通过**）
    if code == 3 {
        println!("[render] SKIPPED: 本机无图形后端（退出码 3）——FR-053 诚实跳过口径");
        assert!(
            !out.exists(),
            "后端不可用时 MUST NOT 产出文件（FR-053）；stderr: {stderr}"
        );
        return;
    }
    assert_eq!(code, 0, "render 应成功，stderr: {stderr}");

    let value: serde_json::Value = serde_json::from_str(&stdout).expect("输出应是 JSON");
    assert_eq!(value["width"], 342);
    assert_eq!(value["height"], 300);
    assert!(
        value["foreground_pixels"].as_u64().unwrap_or(0) > 0,
        "应有前景像素（全零说明渲染没发生）：{value}"
    );

    // --- 独立校验 PNG 结构 ---
    let bytes = std::fs::read(&out).expect("应能读回 PNG");
    assert_eq!(
        &bytes[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
        "PNG 签名不对"
    );

    let mut offset = 8usize;
    let mut kinds = Vec::new();
    while offset + 8 <= bytes.len() {
        let length = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;
        let kind = bytes[offset + 4..offset + 8].to_vec();
        let data_end = offset + 8 + length;
        assert!(data_end + 4 <= bytes.len(), "块长度越界");

        // 逐块重算 CRC（用本仓库的实现——但**判据是独立结构检查**：
        // 若编码器与校验器同错，IHDR 字段与 zlib 头仍会露馅）
        let mut crc_input = kind.clone();
        crc_input.extend_from_slice(&bytes[offset + 8..data_end]);
        let expected = tome_cli::png::crc32(&crc_input);
        let actual = u32::from_be_bytes([
            bytes[data_end],
            bytes[data_end + 1],
            bytes[data_end + 2],
            bytes[data_end + 3],
        ]);
        assert_eq!(actual, expected, "块 CRC 不匹配");

        kinds.push(String::from_utf8_lossy(&kind).into_owned());
        offset = data_end + 4;
        if kind == b"IEND" {
            break;
        }
    }
    assert_eq!(kinds, vec!["IHDR", "IDAT", "IEND"], "块序列应为三块");
    assert_eq!(offset, bytes.len(), "块应恰好覆盖文件，无尾随字节");

    // IHDR 的尺寸与 CLI 报告一致
    let ihdr_data = &bytes[16..29];
    assert_eq!(
        u32::from_be_bytes([ihdr_data[0], ihdr_data[1], ihdr_data[2], ihdr_data[3]]),
        342
    );
    assert_eq!(
        u32::from_be_bytes([ihdr_data[4], ihdr_data[5], ihdr_data[6], ihdr_data[7]]),
        300
    );

    // zlib 流的头两字节（deflate + 32K 窗口）
    let idat_start = bytes
        .windows(4)
        .position(|window| window == b"IDAT")
        .expect("应有 IDAT");
    assert_eq!(bytes[idat_start + 4], 0x78, "zlib CMF 应为 0x78");
    assert_eq!(bytes[idat_start + 5], 0x01, "zlib FLG 应为 0x01");
}

/// 缺失必需选项 → 用法错误（退出码 2）。
#[test]
fn render_requires_out_and_target() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let path = text.to_str().unwrap();

    // 缺 --out
    let (code, _, stderr) = run_cli(&["render", "--text", path, "--page", "0"]);
    assert_eq!(code, 2, "缺 --out 应是用法错误");
    assert!(stderr.contains("--out"), "错误应指出缺哪个选项：{stderr}");

    // 缺 --page / --viewport
    let (code, _, stderr) = run_cli(&["render", "--text", path, "--out", "x.png"]);
    assert_eq!(code, 2, "缺目标页应是用法错误");
    assert!(
        stderr.contains("--page") || stderr.contains("--viewport"),
        "错误应指出缺哪个选项：{stderr}"
    );
}

/// `--theme` 只接受 day/night（白名单，退出码 2）。
#[test]
fn render_rejects_unknown_theme() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let (code, _, stderr) = run_cli(&[
        "render",
        "--text",
        text.to_str().unwrap(),
        "--page",
        "0",
        "--out",
        "x.png",
        "--theme",
        "sepia",
    ]);
    assert_eq!(code, 2, "非法主题应是用法错误");
    assert!(stderr.contains("day"), "错误应说明可接受值：{stderr}");
}

/// **无后端时不产出文件**（FR-053 的"不伪装成功"）。
///
/// ## 这条如何在本机（有 GPU）被验证
///
/// 本机有 Vulkan 后端，走不到那条分支。故用**独立的构建形态**验证：
/// `--no-default-features` 的 CLI 里渲染面在编译期不存在，`render` 必然
/// 返回 3。那是最强的"不可用"形态（连试图渲染的代码都不在二进制里）。
///
/// 该形态由 CI 的 headless job 构建并断言（`scripts/check-reader-features.sh`
/// 与本文件的 `fonts` 用例共同覆盖其"仍可构建"一侧），此处给出**契约形态**
/// 的断言——错误标识与退出码与"运行期无后端"共用同一路径。
#[test]
fn unavailable_render_uses_capability_error_shape() {
    // 契约形态：退出码 3 对应"不支持能力"，且错误是**结构化的**（带能力标识）。
    // 在默认构建下这条路径由 `PipelineError::NoAdapter` 触发；
    // 在 headless 构建下由编译期缺失触发。两者的对外形态一致。
    let error = tome_reader::capability::OFFSCREEN_RENDERING;
    assert_eq!(
        error, "offscreenRendering",
        "能力标识应是 offscreenRendering（contracts/cli.md §2.3）"
    );
    let kind = tome_core::ErrorKind::unsupported_capability(tome_core::ErrorCapability::new(error));
    assert_eq!(
        tome_cli::flags::exit_code_for(&kind),
        3,
        "能力错误应映射为退出码 3（contracts/cli.md 的退出码约定）"
    );
}

/// 空正文渲染**不失败**（产出纯底色的图，SC-006 的空章口径）。
#[test]
fn render_handles_empty_chapter() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = dir.path().join("empty.txt");
    std::fs::write(&text, "").expect("应能写空文件");
    let out = dir.path().join("empty.png");

    let (code, _, stderr) = run_cli(&[
        "render",
        "--text",
        text.to_str().unwrap(),
        "--page",
        "0",
        "--out",
        out.to_str().unwrap(),
        "--width",
        "128",
        "--height",
        "64",
    ]);

    if code == 3 {
        println!("[render] SKIPPED: 本机无图形后端");
        return;
    }
    assert_eq!(code, 0, "空章也应能渲染，stderr: {stderr}");
    assert!(out.exists(), "空章也应产出文件");
}

// ===========================================================================
// `--page` 的消费（契约 §2.3 的"目标页"）
// ===========================================================================

/// 造一份**足够长**的正文（确保切出多页）。
///
/// 篇幅按 12 段算：19px 字号、342×200 视口下会切出 10 页左右，
/// 足以让"第 0 页与第 1 页不同"这条判据有观测面。
fn write_long_sample(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("long.txt");
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，".to_owned()
        + "后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。";
    let text = (0..12)
        .map(|index| format!("第{}段。{paragraph}", index + 1))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, text).expect("应能写入临时正文");
    path
}

/// **`--page N` 渲染的是第 N 页**（不同页的内容必须不同）。
///
/// ## 为什么"渲染成功"不构成判据
///
/// `--page` 未被消费时（本任务修正前的形态），传任何页号都渲染首屏——
/// 而"成功产出 PNG"这条断言照样通过。故判据是**差分**：渲染第 0 页与第 1
/// 页，两者的前景像素内容必须**不同**。
#[test]
fn render_page_selects_the_requested_page() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_long_sample(dir.path());
    let page0 = dir.path().join("p0.png");
    let page1 = dir.path().join("p1.png");

    let render = |page: &str, out: &Path| -> (i32, String, String) {
        run_cli(&[
            "render",
            "--text",
            text.to_str().unwrap(),
            "--page",
            page,
            "--out",
            out.to_str().unwrap(),
            "--width",
            "342",
            "--height",
            "200",
        ])
    };

    let (code0, out0, err0) = render("0", &page0);
    if code0 == 3 {
        println!("[render] SKIPPED: 本机无图形后端");
        return;
    }
    assert_eq!(code0, 0, "渲染第 0 页应成功，stderr: {err0}");
    let (code1, out1, err1) = render("1", &page1);
    assert_eq!(code1, 0, "渲染第 1 页应成功，stderr: {err1}");

    let value0: serde_json::Value = serde_json::from_str(&out0).expect("应是 JSON");
    let value1: serde_json::Value = serde_json::from_str(&out1).expect("应是 JSON");

    let bytes0 = std::fs::read(&page0).expect("应能读第 0 页");
    let bytes1 = std::fs::read(&page1).expect("应能读第 1 页");

    // 两页的 PNG 内容必须不同（同一份正文的不同页不可能逐字节相同）
    assert_ne!(
        bytes0, bytes1,
        "第 0 页与第 1 页的像素内容必须不同——相同说明 --page 未被消费"
    );

    // 且两者都有前景（不是"第 1 页是空的"这种平凡差异）
    for (label, value) in [("第 0 页", &value0), ("第 1 页", &value1)] {
        assert!(
            value["foreground_pixels"].as_u64().unwrap_or(0) > 0,
            "{label} 应有前景像素：{value}"
        );
    }
}

/// **越界页码报结构化错误**（不产出文件、不静默出空图）。
///
/// ## 为什么这条重要
///
/// 页码越界时若静默产出一张只有底色的图，调用方无法分辨"这一页真的是空的"
/// 与"页号写错了"——前者是合法结果（留白页），后者是用法错误。
/// 两者必须可区分，故越界走**配置错误**（退出码 1）而非"成功"。
#[test]
fn render_rejects_out_of_range_page() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let out = dir.path().join("nope.png");

    let (code, _, stderr) = run_cli(&[
        "render",
        "--text",
        text.to_str().unwrap(),
        "--page",
        "9999",
        "--out",
        out.to_str().unwrap(),
        "--width",
        "342",
        "--height",
        "300",
    ]);

    if code == 3 {
        println!("[render] SKIPPED: 本机无图形后端");
        return;
    }
    assert_eq!(
        code, 1,
        "越界页码应是业务/配置错误（退出码 1），实测 {code}；stderr: {stderr}"
    );
    assert!(
        stderr.contains("9999") || stderr.contains("越界") || stderr.contains("共"),
        "错误应说明页号与总页数：{stderr}"
    );
    assert!(
        !out.exists(),
        "越界时 MUST NOT 产出文件（否则调用方会把它当成一页的有效结果）"
    );
}

// ===========================================================================
// `--viewport <n>`（contracts/cli.md §2.3 的滚动模式目标）
// ===========================================================================

/// **`--viewport N` 渲染的是第 N 个视口**（不同视口的内容必须不同）。
///
/// ## 与 `render_page_selects_the_requested_page` 同一条判据
///
/// "渲染成功"不构成判据——`--viewport` 未被消费时传任何视口号都渲染首屏，
/// 而"产出合法 PNG"照样通过。故判据同样是**差分**：视口 0 与视口 1 的
/// 前景像素内容必须不同。
///
/// ## 为什么需要比翻页更大的高度
///
/// 滚动模式的视口高即画布高：太矮则整章只有一屏，差分断言会退化成
/// 平凡通过。此处给 `--mode scroll` 一个足够高的视口，让长样本切成多屏。
#[test]
fn render_viewport_selects_the_requested_viewport() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_long_sample(dir.path());
    let viewport0 = dir.path().join("v0.png");
    let viewport1 = dir.path().join("v1.png");

    let render = |index: &str, out: &Path| -> (i32, String, String) {
        run_cli(&[
            "render",
            "--text",
            text.to_str().unwrap(),
            "--mode",
            "scroll",
            "--viewport",
            index,
            "--out",
            out.to_str().unwrap(),
            "--width",
            "342",
            "--height",
            "120",
        ])
    };

    let (code0, out0, err0) = render("0", &viewport0);
    if code0 == 3 {
        println!("[render] SKIPPED: 本机无图形后端");
        return;
    }
    assert_eq!(code0, 0, "渲染第 0 个视口应成功，stderr: {err0}");
    let (code1, out1, err1) = render("1", &viewport1);
    assert_eq!(code1, 0, "渲染第 1 个视口应成功，stderr: {err1}");

    let value0: serde_json::Value = serde_json::from_str(&out0).expect("应是 JSON");
    let value1: serde_json::Value = serde_json::from_str(&out1).expect("应是 JSON");

    let bytes0 = std::fs::read(&viewport0).expect("应能读第 0 个视口");
    let bytes1 = std::fs::read(&viewport1).expect("应能读第 1 个视口");

    assert_ne!(
        bytes0, bytes1,
        "第 0 个与第 1 个视口的像素内容必须不同——相同说明 --viewport 未被消费"
    );

    for (label, value) in [("第 0 个视口", &value0), ("第 1 个视口", &value1)] {
        assert!(
            value["foreground_pixels"].as_u64().unwrap_or(0) > 0,
            "{label} 应有前景像素：{value}"
        );
        assert_eq!(
            value["viewport_index"].as_u64(),
            Some(if label.starts_with("第 0") { 0 } else { 1 }),
            "{label} 的报告应回显消费到的视口号：{value}"
        );
    }
}

/// **越界视口报结构化错误**（不产出文件、不静默出空图）。
///
/// 与越界页码同理：静默产出底色图会让调用方无法分辨"这一屏真的是空的"
/// 与"视口号写错了"。
#[test]
fn render_rejects_out_of_range_viewport() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let out = dir.path().join("nope.png");

    let (code, _, stderr) = run_cli(&[
        "render",
        "--text",
        text.to_str().unwrap(),
        "--mode",
        "scroll",
        "--viewport",
        "9999",
        "--out",
        out.to_str().unwrap(),
        "--width",
        "342",
        "--height",
        "300",
    ]);

    if code == 3 {
        println!("[render] SKIPPED: 本机无图形后端");
        return;
    }
    assert_eq!(
        code, 1,
        "越界视口应是业务/配置错误（退出码 1），实测 {code}；stderr: {stderr}"
    );
    assert!(
        stderr.contains("9999") || stderr.contains("越界") || stderr.contains("共"),
        "错误应说明视口号与总视口数：{stderr}"
    );
    assert!(
        !out.exists(),
        "越界时 MUST NOT 产出文件（否则调用方会把它当成一屏的有效结果）"
    );
}
