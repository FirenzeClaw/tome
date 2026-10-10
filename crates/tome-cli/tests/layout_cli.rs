//! `tome layout` 的契约形态与 golden 测试（T048 的先行部分 / `contracts/cli.md` §2.1）
//!
//! ## 本文件覆盖什么
//!
//! 1. **契约形态**：输出的每个顶层字段与 §2.1 的 JSON 示例对应
//! 2. **确定性**：同参数两次运行**逐字节相同**（§2.1 的"关键不变量"）
//! 3. **字段级 golden**：对固定输入逐字段断言（而非整串比对——
//!    整串比对在字段增减时会全红，看不出是哪一处变了）
//! 4. **退出码**：用法错误 2 / 不支持能力 3
//!
//! ## 为什么用字段断言而不是快照文件
//!
//! 快照（整串）比对的缺点是**失败信息无信息量**：输出一行 diff，
//! 读者要自己找哪一处变了。字段级断言直接指出"`geometry.measure_px`
//! 期望 342 实为 684"。像素快照（T036）用整图比对是因为图像没有字段；
//! JSON 有字段，就该用字段。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音（与 verify_gate.rs 同一约定）"
)]

use std::process::Command;

/// 跑一次 `tome layout`，返回 (退出码, stdout, stderr)。
fn run_layout(args: &[&str]) -> (i32, String, String) {
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

/// 写一份临时正文文件，返回其路径（`TempDir` 由调用方持有以保活）。
fn write_sample(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("sample.txt");
    std::fs::write(
        &path,
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号。\n\
         书页翻开的时候，光线正好落在那一行字上。",
    )
    .expect("应能写入临时正文");
    path
}

// ===========================================================================
// 契约形态
// ===========================================================================

/// `contracts/cli.md` §2.1 的顶层字段**逐一存在**。
#[test]
fn layout_output_has_contract_top_level_fields() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, stderr) = run_layout(&["layout", "--text", text.to_str().unwrap()]);
    assert_eq!(code, 0, "layout 应成功。stderr: {stderr}");

    let value: serde_json::Value = serde_json::from_str(&stdout).expect("输出应是合法 JSON");

    // §2.1 的字段（本阶段能给出的全部；pages/scroll 为空但必须存在）
    for field in [
        "chapter_index",
        "mode",
        "geometry",
        "paragraphs",
        "pages",
        "scroll",
        "rule_exceptions",
    ] {
        assert!(
            value.get(field).is_some(),
            "契约字段 `{field}` 缺失。实际顶层字段：{:?}",
            value.as_object().map(|obj| obj.keys().collect::<Vec<_>>())
        );
    }

    // geometry 的子字段（§2.1 的示例逐一对应）
    let geometry = &value["geometry"];
    for field in [
        "font_size",
        "line_height",
        "measure_em",
        "gutter",
        "width",
        "height",
        "paragraph_mode",
        "spread",
    ] {
        assert!(
            geometry.get(field).is_some(),
            "geometry 字段 `{field}` 缺失"
        );
    }
}

/// 行的子字段与 §2.1 的行形态对应。
#[test]
fn layout_line_entries_have_contract_fields() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, _) = run_layout(&["layout", "--text", text.to_str().unwrap()]);
    assert_eq!(code, 0);

    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let first_para = &value["paragraphs"][0];
    let first_line = &first_para["lines"][0];

    for field in ["char_start", "char_len", "paragraph_index"] {
        assert!(first_line.get(field).is_some(), "行字段 `{field}` 缺失");
    }
}

// ===========================================================================
// 字段级 golden
// ===========================================================================

/// **字段级 golden**：固定输入 + 固定几何 → 逐字段断言。
#[test]
fn layout_golden_fields_for_fixed_input() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("fixed.txt");
    // 固定输入：10 个汉字 + 句号（便于手算）
    std::fs::write(&path, "甲乙丙丁戊己庚辛壬癸。").expect("写临时正文");

    let (code, stdout, stderr) = run_layout(&[
        "layout",
        "--text",
        path.to_str().unwrap(),
        "--width",
        "228",
        "--font-size",
        "19",
        "--line-height",
        "1.85",
        "--measure-em",
        "12",
        "--gutter",
        "64",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");

    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    // 顶层
    assert_eq!(value["chapter_index"], 0);
    assert_eq!(value["mode"], "paginated");

    // 几何：CLI 值应如实回显
    //
    // ## 为什么用容差比较而不是 `assert_eq!`
    //
    // JSON 里的 f32 是**精确的二进制值**：`1.85f32` 序列化为
    // `1.850000023841858`（不是 `1.85`）。用 `assert_eq!` 会因为
    // 这个表示误差而失败，而那是**序列化的正常行为**，不是缺陷。
    // 故逐个用 1e-5 容差比较。
    let near = |field: &str, expected: f64| {
        let actual = value["geometry"][field]
            .as_f64()
            .unwrap_or_else(|| panic!("geometry.{field} 应是数值"));
        assert!(
            (actual - expected).abs() < 1e-5,
            "geometry.{field} 期望 {expected}，实测 {actual}"
        );
    };
    near("font_size", 19.0);
    near("line_height", 1.85);
    near("measure_em", 12.0);
    near("gutter", 64.0);
    near("width", 228.0);
    assert_eq!(value["geometry"]["paragraph_mode"], "indent");
    assert_eq!(value["geometry"]["spread"], "auto");

    // 版心：min(12em × 19px = 228px, width 228px, 42em 上限) = 228px
    near("measure_px", 228.0);

    // 段落：恰好一段
    let paragraphs = value["paragraphs"].as_array().expect("paragraphs 是数组");
    assert_eq!(paragraphs.len(), 1, "输入无换行，应是一段");
    assert_eq!(paragraphs[0]["paragraph_index"], 0);
    assert_eq!(paragraphs[0]["char_start"], 0);
    assert_eq!(paragraphs[0]["char_len"], 11, "10 个汉字 + 1 个句号");

    // 行：11 字 / 每行 12 字 → 单行（版心够宽）
    let lines = paragraphs[0]["lines"].as_array().expect("lines 是数组");
    assert_eq!(lines.len(), 1, "228px 版心容纳 12 字，11 字应单行");
    assert_eq!(lines[0]["char_start"], 0);
    assert_eq!(lines[0]["char_len"], 11);
    assert_eq!(lines[0]["paragraph_index"], 0);

    // 分页与滚动属后续切片——**空数组而非伪造**
    assert!(value["pages"]
        .as_array()
        .is_some_and(|pages| pages.is_empty()));
    assert!(value["scroll"]["viewports"]
        .as_array()
        .is_some_and(|viewports| viewports.is_empty()));
}

/// 版心**封顶 42em**（MASTER §5.3 的裁决：单页超出 42em 则留白）。
#[test]
fn layout_measure_is_capped_at_42em() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, _) = run_layout(&[
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--width",
        "3000",
        "--font-size",
        "19",
        "--measure-em",
        "36",
    ]);
    assert_eq!(code, 0);

    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let cap = 42.0 * 19.0; // 798px
    let measure = value["geometry"]["measure_px"].as_f64().unwrap();
    assert!(
        measure <= cap + 1e-3,
        "版心 {measure}px 超过 42em 上限 {cap}px——MASTER §5.3 要求封顶留白"
    );
}

// ===========================================================================
// 确定性（§2.1 的关键不变量）
// ===========================================================================

/// **同参数两次运行输出逐字节相同**（§2.1 明文要求）。
#[test]
fn layout_output_is_byte_identical_across_runs() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let args = [
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--width",
        "342",
        "--font-size",
        "19",
    ];

    let (code_a, stdout_a, _) = run_layout(&args);
    let (code_b, stdout_b, _) = run_layout(&args);

    assert_eq!(code_a, 0);
    assert_eq!(code_b, 0);
    assert_eq!(
        stdout_a, stdout_b,
        "同参数两次运行的输出必须逐字节相同（SPEC §FR-048 的确定性要求）"
    );
}

/// 不同几何参数下也各自确定（不是"恰好这次一样"）。
#[test]
fn layout_determinism_holds_across_geometry() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let path = text.to_str().unwrap();

    for width in ["228", "342", "418"] {
        let args = ["layout", "--text", path, "--width", width];
        let (_, first, _) = run_layout(&args);
        let (_, second, _) = run_layout(&args);
        assert_eq!(first, second, "版心 {width}px 下两次运行不一致");
    }
}

// ===========================================================================
// 退出码
// ===========================================================================

/// 用法错误 → 退出码 2（字号越界由 `flags` 拒绝）。
#[test]
fn invalid_font_size_is_usage_error() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    // FR-013 的范围是 14-30
    for bad in ["13", "31", "0", "-5"] {
        let (code, _, stderr) = run_layout(&[
            "layout",
            "--text",
            text.to_str().unwrap(),
            "--font-size",
            bad,
        ]);
        assert_eq!(
            code, 2,
            "字号 {bad} 应判用法错误（退出码 2），stderr: {stderr}"
        );
    }
}

/// 行高越界 → 退出码 2（FR-013 的 1.75–2.2，下界是无障碍强制）。
#[test]
fn invalid_line_height_is_usage_error() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    for bad in ["1.4", "1.74", "2.21", "3"] {
        let (code, _, stderr) = run_layout(&[
            "layout",
            "--text",
            text.to_str().unwrap(),
            "--line-height",
            bad,
        ]);
        assert_eq!(
            code, 2,
            "行高 {bad} 应判用法错误（FR-013 的 1.75–2.2），stderr: {stderr}"
        );
    }
}

/// 正文文件不存在 → 业务错误（退出码 1），且**不 panic**。
#[test]
fn missing_text_file_is_business_error() {
    let (code, _, stderr) = run_layout(&["layout", "--text", "definitely-not-a-file.txt"]);
    assert_eq!(code, 1, "文件不存在应判业务错误，stderr: {stderr}");
    assert!(
        !stderr.is_empty(),
        "必须给出可操作的错误说明（不得静默失败）"
    );
}

/// `--chapter`（书架形式）本阶段未接线 → **结构化能力错误**（退出码 3）。
///
/// ## 为什么这条重要
///
/// 契约定义的入口是 `--chapter`，而本阶段只有 `--text`。若 `--chapter`
/// 静默降级或报一个笼统错误，调用方无从区分"参数写错"与"能力未就绪"。
/// 退出码 3（不支持能力）正是为这种情形准备的（对齐 FR-006/FR-058）。
#[test]
fn chapter_form_reports_structured_capability_error() {
    let (code, stdout, stderr) = run_layout(&["layout", "--chapter", "some-book#0"]);

    assert_eq!(
        code, 3,
        "--chapter 未接线应报不支持能力（退出码 3）。stdout: {stdout} stderr: {stderr}"
    );
    assert!(
        stderr.contains("chapter-by-bookshelf") || stderr.contains("--text"),
        "错误须给出能力标识或可操作指引，实际：{stderr}"
    );
}

/// 缺正文来源 → 业务错误（不是崩溃）。
#[test]
fn missing_text_source_is_handled() {
    let (code, _, stderr) = run_layout(&["layout", "--width", "342"]);
    assert_eq!(code, 1, "缺正文源应判业务错误，stderr: {stderr}");
    assert!(stderr.contains("--text") || stderr.contains("--chapter"));
}

// ===========================================================================
// 排版结果的自洽性（跨层交叉验证）
// ===========================================================================

/// 输出的行区间**拼接后覆盖整段**（不丢字——SC-006 的排版层形态）。
#[test]
fn layout_lines_cover_paragraph_completely() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, _) =
        run_layout(&["layout", "--text", text.to_str().unwrap(), "--width", "228"]);
    assert_eq!(code, 0);

    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    for paragraph in value["paragraphs"].as_array().unwrap() {
        let expected_len = paragraph["char_len"].as_u64().unwrap() as usize;
        let lines = paragraph["lines"].as_array().unwrap();

        let mut expected_start = 0usize;
        for line in lines {
            assert_eq!(
                line["char_start"].as_u64().unwrap() as usize,
                expected_start,
                "行区间须连续（段 {}）",
                paragraph["paragraph_index"]
            );
            expected_start += line["char_len"].as_u64().unwrap() as usize;
        }
        assert_eq!(
            expected_start, expected_len,
            "行区间拼接应覆盖整段（段 {}：覆盖 {expected_start} / 段长 {expected_len}）",
            paragraph["paragraph_index"]
        );
    }
}

/// 超长西文词在输出里体现为 `degraded_words > 0`（FR-009 的 CLI 可见性）。
#[test]
fn layout_reports_degraded_words() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("long.txt");
    std::fs::write(
        &path,
        "这个术语拼作 pneumonoultramicroscopicsilicovolcanoconiosis，是一个很长的词。",
    )
    .expect("写临时正文");

    let (code, stdout, _) =
        run_layout(&["layout", "--text", path.to_str().unwrap(), "--width", "228"]);
    assert_eq!(code, 0);

    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert!(
        value["degraded_words"].as_u64().unwrap_or(0) > 0,
        "超长词应触发退化并在输出中可见（实测 degraded_words = {}）",
        value["degraded_words"]
    );
}

/// 段落切分的诊断随输出可见（FR-002 的"可读的诊断记录"）。
#[test]
fn layout_exposes_split_diagnostic() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, _) = run_layout(&["layout", "--text", text.to_str().unwrap()]);
    assert_eq!(code, 0);

    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let split = &value["text_split"];
    for field in [
        "split_mode",
        "newline_count",
        "leading_space_hits",
        "heuristic_applied",
    ] {
        assert!(split.get(field).is_some(), "诊断字段 `{field}` 缺失");
    }
    assert_eq!(split["split_mode"], "NewlineOnly", "本例用换行分段");
    assert_eq!(split["heuristic_applied"], false);
}
