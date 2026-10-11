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

    // 行：**段首行带 2em 缩进**，故可用宽 = 228 − 38 = 190px
    //
    // ## 这条断言为什么在 S2-4 前导 2 之后改了
    //
    // 旧断言是"12 字身版心容纳 11 字 → 单行"。那个前提在 FR-018 的**施加**
    // 落地后不再成立：默认缩进模式下段首行要留 2em（38px），实际可用宽
    // 只剩 190px——11 字必然折行。
    //
    // 这不是缺陷，正是"缩进模式真的生效了"的可观测证据：同一输入在
    // `--paragraph-mode gap` 下仍是单行（见
    // `paragraph_mode_changes_line_breaking`）。
    //
    // ## 为什么首行是 9 字而不是 10 字（手算会算错的地方）
    //
    // 190px 确实容得下 10 个字身，但**末两字不是两个独立单元**：
    // 「癸。」是「闭标点与前一字」的合并单元（BASELINE §11.1 实测：
    // `unicode-linebreak` 已把中文标点与其前一字绑成多字 word），
    // 宽 38px。故首行只能取 9 字（171px）——再放这个单元会到 209px > 190px。
    let lines = paragraphs[0]["lines"].as_array().expect("lines 是数组");
    assert_eq!(
        lines.len(),
        2,
        "228px 版心 − 2em 缩进 = 190px 可用宽，11 字应折两行"
    );
    assert_eq!(lines[0]["char_start"], 0);
    assert_eq!(
        lines[0]["char_len"], 9,
        "首行容 9 字（「癸。」是 2 字一体的禁则单元，放不下）"
    );
    assert_eq!(lines[1]["char_start"], 9);
    assert_eq!(lines[1]["char_len"], 2, "次行收「癸。」这个不可拆单元");
    assert_eq!(lines[0]["paragraph_index"], 0);

    // 分页与滚动属后续切片——**空数组而非伪造**
    //
    // `[NOTE]` 本断言在 T019 之后**已过时并被替换**：`pages` 现在产出真实页。
    // 见下方 `layout_pages_tile_chapter_exactly` 与
    // `layout_roll_produces_real_pages`。此处只保留 `scroll` 仍是空的断言
    // ——滚动流属 Phase 5（T025/T026）。
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

// ===========================================================================
// T019/T021：页序列经 CLI 的真实产出（SC-006 的端到端形态）
// ===========================================================================

/// **页区间拼接与正文逐字符相等**（SC-006 经 CLI 的端到端验证）。
///
/// ## 为什么 CLI 层也要测这条
///
/// 库内测试（`pagination_lossless.rs`）已证过同一性质。CLI 侧的差别在于
/// **输出的区间是序列化过的**：JSON 里的 `char_start`/`char_len` 若在序列化
/// 时算错（如把行内区间当页区间、或漏掉 `is_padding` 的过滤），库层测试
/// 看不见。故这里按**契约 §2.1 的 JSON 字段**重建正文并比对。
#[test]
fn layout_pages_tile_chapter_exactly() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("tile.txt");
    let text = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                后面跟着一个逗号，逗号之后是半句话。\n\
                书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮。\n\
                她说：「真的吗？」我说：「当然，」然后他笑了，笑声在屋子里荡开。";
    std::fs::write(&path, text).expect("写临时正文");

    let characters: Vec<char> = text.chars().collect();
    let mut checked = 0usize;

    for (width, height, font_size) in [
        ("228", "200", "19"),
        ("342", "400", "19"),
        ("1432", "200", "19"),
        ("228", "300", "30"),
    ] {
        let (code, stdout, stderr) = run_layout(&[
            "layout",
            "--text",
            path.to_str().unwrap(),
            "--width",
            width,
            "--height",
            height,
            "--font-size",
            font_size,
        ]);
        assert_eq!(code, 0, "{width}x{height}/{font_size}px stderr: {stderr}");

        let value: serde_json::Value = serde_json::from_str(&stdout).expect("应输出 JSON");
        let pages = value["pages"].as_array().expect("pages 是数组");
        assert!(!pages.is_empty(), "pages 不得为空（T019 后应产出真实页）");

        let mut reconstructed = String::new();
        for page in pages {
            let is_padding = page["is_padding"].as_bool().expect("is_padding 是 bool");
            let start = page["char_start"].as_u64().expect("char_start") as usize;
            let len = page["char_len"].as_u64().expect("char_len") as usize;

            if is_padding {
                assert_eq!(len, 0, "留白页不得认领字符（SC-006 的口径）");
                continue;
            }
            reconstructed.extend(characters[start..start + len].iter());
        }

        assert_eq!(
            reconstructed, text,
            "{width}x{height}/{font_size}px：页区间拼接与正文不符（丢字或重字）"
        );
        checked += 1;
    }
    assert_eq!(checked, 4);
}

/// **`--spread single` 在宽窗下仍产出单页**（FR-015 的手动覆盖经 CLI）。
#[test]
fn layout_force_single_overrides_wide_window() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, _) = run_layout(&[
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--width",
        "1432",
        "--spread",
        "single",
    ]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["geometry"]["columns"], 1,
        "--spread single 在宽窗下必须压过判据"
    );
    for page in value["pages"].as_array().unwrap() {
        assert!(
            page["side"].is_null(),
            "单页模式下不得有左右归属（裁决 OQ#5→A）"
        );
    }
}

/// **`--spread auto` 在宽窗下产出双页且带左右归属**（FR-016 的书页语义）。
#[test]
fn layout_auto_spread_marks_sides_in_wide_window() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, _) = run_layout(&[
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--width",
        "1432",
    ]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(value["geometry"]["columns"], 2, "1432px 应判双页");
    let pages = value["pages"].as_array().unwrap();
    assert!(!pages.is_empty());
    for (index, page) in pages.iter().enumerate() {
        let expected = if index % 2 == 0 { "left" } else { "right" };
        assert_eq!(
            page["side"], expected,
            "双页模式下第 {index} 页应为 {expected}（左偶右奇）"
        );
    }
}

/// **`--anchor` 报告页内定位**（FR-017 经 CLI 的形态）。
#[test]
fn layout_anchor_reports_page_location() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, stderr) = run_layout(&[
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--width",
        "228",
        "--height",
        "200",
        "--anchor",
        "30",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let hit = &value["anchor_hit"];
    assert_eq!(hit["char_offset"], 30, "锚点值必须原样报告（FR-017）");
    assert_eq!(hit["visible"], true, "锚点字符应可见");
    let page_index = hit["page_index"].as_u64().expect("应有页序号") as usize;

    // 该页的区间确实包含锚点
    let pages = value["pages"].as_array().unwrap();
    let start = pages[page_index]["char_start"].as_u64().unwrap() as usize;
    let len = pages[page_index]["char_len"].as_u64().unwrap() as usize;
    assert!(
        (start..start + len).contains(&30),
        "anchor_hit 的 page_index={page_index} 区间 {start}..{} 不含锚点 30",
        start + len
    );
}

/// 越界锚点被**钳制**且不 panic（spec §Edge Cases）。
#[test]
fn layout_out_of_range_anchor_is_clamped() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, stderr) = run_layout(&[
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--width",
        "228",
        "--height",
        "200",
        "--anchor",
        "999999",
    ]);
    assert_eq!(code, 0, "越界锚点不得导致失败，stderr: {stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let hit = &value["anchor_hit"];
    assert_eq!(hit["visible"], false, "钳制到章末的位置不算可见");
    assert!(
        hit["page_index"].as_u64().is_some(),
        "仍应给出所在的（末）页"
    );
}

/// **页序列的确定性经 CLI**（同参数两次运行逐字节相同，含 pages）。
#[test]
fn layout_pages_are_deterministic() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let path = text.to_str().unwrap();

    for width in ["228", "1432"] {
        let args = [
            "layout", "--text", path, "--width", width, "--height", "200",
        ];
        let (_, first, _) = run_layout(&args);
        let (_, second, _) = run_layout(&args);
        assert_eq!(
            first, second,
            "版心 {width}px 下含 pages 的完整输出必须逐字节相同"
        );
    }
}

// ===========================================================================
// T025-T027：滚动形态与位置换算经 CLI 的真实产出
// ===========================================================================

/// **`--mode scroll` 的视口区间并集与正文逐字符相等**（SC-006b 经 CLI）。
#[test]
fn scroll_viewports_tile_chapter_exactly() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("scroll.txt");
    let text = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                后面跟着一个逗号，逗号之后是半句话。\n\
                书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮。\n\
                她说：「真的吗？」我说：「当然，」然后他笑了。";
    std::fs::write(&path, text).expect("写临时正文");

    let characters: Vec<char> = text.chars().collect();
    let mut checked = 0usize;

    for (width, height, font_size) in [
        ("342", "150", "19"),
        ("342", "400", "19"),
        ("228", "200", "14"),
        ("684", "600", "24"),
    ] {
        let (code, stdout, stderr) = run_layout(&[
            "layout",
            "--text",
            path.to_str().unwrap(),
            "--mode",
            "scroll",
            "--width",
            width,
            "--height",
            height,
            "--font-size",
            font_size,
        ]);
        assert_eq!(code, 0, "{width}x{height}/{font_size}px stderr: {stderr}");

        let value: serde_json::Value = serde_json::from_str(&stdout).expect("应输出 JSON");
        let viewports = value["scroll"]["viewports"]
            .as_array()
            .expect("viewports 是数组");
        assert!(
            !viewports.is_empty(),
            "scroll 的 viewports 不得为空（T025 后应产出真实视口）"
        );

        // 区间并集 == 正文
        let mut reconstructed = String::new();
        for viewport in viewports {
            let start = viewport["char_start"].as_u64().expect("char_start") as usize;
            let len = viewport["char_len"].as_u64().expect("char_len") as usize;
            reconstructed.extend(characters[start..start + len].iter());
        }
        assert_eq!(
            reconstructed, text,
            "{width}x{height}/{font_size}px：视口区间并集与正文不符"
        );

        // 相邻视口连续无缺口无重叠
        let mut expected_start = 0usize;
        for (index, viewport) in viewports.iter().enumerate() {
            let start = viewport["char_start"].as_u64().unwrap() as usize;
            assert_eq!(
                start, expected_start,
                "第 {index} 屏起点应为上一屏终点（无缺口无重叠）"
            );
            expected_start = start + viewport["char_len"].as_u64().unwrap() as usize;
        }
        assert_eq!(expected_start, text.chars().count(), "末屏终点应为正文末尾");

        // `top` 单调递增
        let tops: Vec<f64> = viewports
            .iter()
            .map(|viewport| viewport["top"].as_f64().unwrap())
            .collect();
        for pair in tops.windows(2) {
            assert!(pair[1] > pair[0], "top 必须递增");
        }

        checked += 1;
    }
    assert_eq!(checked, 4);
}

/// **`scroll.content_height` 真实产出**（FR-020 经 CLI）。
#[test]
fn scroll_content_height_is_reported() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, _) = run_layout(&[
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--mode",
        "scroll",
        "--width",
        "342",
        "--height",
        "150",
    ]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let height = value["scroll"]["content_height"]
        .as_f64()
        .expect("应有总高");
    assert!(height > 0.0, "滚动流总高应为正，实测 {height}");

    // 分页模式下 `content_height` 是 null（本次没算滚动流——不是"未实现"）
    let (_, paginated_stdout, _) = run_layout(&[
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--width",
        "342",
        "--height",
        "150",
    ]);
    let paginated: serde_json::Value = serde_json::from_str(&paginated_stdout).unwrap();
    assert!(
        paginated["scroll"]["content_height"].is_null(),
        "翻页模式下 content_height 应为 null"
    );
    assert_eq!(
        paginated["scroll"]["viewport_count"], 0,
        "翻页模式下视口数应为 0"
    );
}

/// **`--anchor` 在滚动形态下报视口位置**（FR-021 的同构，经 CLI）。
#[test]
fn layout_anchor_reports_viewport_in_scroll_mode() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, stderr) = run_layout(&[
        "layout",
        "--text",
        text.to_str().unwrap(),
        "--mode",
        "scroll",
        "--width",
        "342",
        "--height",
        "150",
        "--anchor",
        "20",
    ]);
    assert_eq!(code, 0, "stderr: {stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    let hit = &value["anchor_hit"];
    assert_eq!(hit["char_offset"], 20);
    assert_eq!(hit["visible"], true);
    assert!(
        hit["viewport_index"].as_u64().is_some(),
        "滚动形态下应报视口序号，实测 {hit}"
    );

    // 该视口的区间确实包含锚点
    let index = hit["viewport_index"].as_u64().unwrap() as usize;
    let viewports = value["scroll"]["viewports"].as_array().unwrap();
    let start = viewports[index]["char_start"].as_u64().unwrap() as usize;
    let len = viewports[index]["char_len"].as_u64().unwrap() as usize;
    assert!(
        (start..start + len).contains(&20),
        "anchor_hit 的 viewport_index={index} 区间 {start}..{} 不含锚点 20",
        start + len
    );
}

/// **`layout-anchor` 两方向都报锚点值不变**（SC-005b 经 CLI）。
#[test]
fn layout_anchor_command_preserves_anchor_both_directions() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let path = text.to_str().unwrap();

    for (from_mode, to_mode) in [
        ("paginated", "scroll"),
        ("scroll", "paginated"),
        ("paginated", "paginated"),
    ] {
        let (code, stdout, stderr) = run_layout(&[
            "layout-anchor",
            "--text",
            path,
            "--char",
            "40",
            "--width",
            "342",
            "--height",
            "150",
            "--mode",
            from_mode,
            "--target-mode",
            to_mode,
        ]);
        assert_eq!(code, 0, "{from_mode}→{to_mode} stderr: {stderr}");

        let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(
            value["anchor_value_unchanged"], true,
            "{from_mode}→{to_mode}：锚点值必须不变（SC-005b）"
        );
        assert_eq!(value["char_offset"], 40, "锚点值应原样回显");
        assert_eq!(
            value["from"]["char_offset"], 40,
            "来源侧的 char_offset 应等于锚点"
        );
        assert_eq!(
            value["to"]["char_offset"], 40,
            "目标侧的 char_offset 应等于锚点"
        );
        assert_eq!(
            value["to_visible"], true,
            "{from_mode}→{to_mode}：目标形态下锚点必须可见"
        );
    }
}

/// `layout-anchor` 缺 `--char` → 用法错误（退出码 2）。
#[test]
fn layout_anchor_requires_char_option() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, _, stderr) = run_layout(&[
        "layout-anchor",
        "--text",
        text.to_str().unwrap(),
        "--width",
        "342",
    ]);
    assert_eq!(code, 2, "缺 --char 应判用法错误，stderr: {stderr}");
    assert!(
        stderr.contains("--char"),
        "错误须指出缺哪个选项，实测：{stderr}"
    );
}

/// 越界锚点在 `layout-anchor` 里被钳制且两形态一致。
#[test]
fn layout_anchor_clamps_consistently() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());

    let (code, stdout, stderr) = run_layout(&[
        "layout-anchor",
        "--text",
        text.to_str().unwrap(),
        "--char",
        "999999",
        "--width",
        "342",
        "--height",
        "150",
    ]);
    // [DEBUG-CI] windows-2022/ubuntu 上该命令退出码非 0 而本地为 0——
    // 先把 stderr 打进 CI 日志再修（定位后移除此行）
    eprintln!("DEBUG code={code} stderr={stderr}");
    assert_eq!(code, 0, "layout-anchor 退出码应为 0，stderr: {stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(
        value["from"]["char_offset"], value["to"]["char_offset"],
        "两形态的钳制结果必须一致"
    );
    assert_eq!(
        value["anchor_value_unchanged"], true,
        "钳制后两侧仍应一致（钳制是确定性的）"
    );
}

/// **滚动形态的确定性**（FR-048 的形态扩展，经 CLI）。
#[test]
fn scroll_output_is_deterministic() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let path = text.to_str().unwrap();

    for height in ["150", "300"] {
        let args = [
            "layout", "--text", path, "--mode", "scroll", "--width", "342", "--height", height,
        ];
        let (_, first, _) = run_layout(&args);
        let (_, second, _) = run_layout(&args);
        assert_eq!(
            first, second,
            "滚动形态 {height}px 下两次运行必须逐字节相同"
        );
    }
}

/// **FR-018 的两模式经 CLI 可观测**（前导 2 的端到端证据）。
///
/// ## 这条测试要证明什么
///
/// `layout_golden_fields_for_fixed_input` 断言了"缩进模式下 11 字折两行"。
/// 但那个断言之于 FR-018 是**不充分**的——"行数变了"也可能由任何别的几何
/// 差异造成。有效判据是**对照**：同一输入、同一版心，只切段模式，
/// 断行结果必须不同，且差异方向与"缩进占掉 2em"一致。
///
/// 同时钉住另一侧：间距模式不应有首行缩进，故它仍是单行。
#[test]
fn paragraph_mode_changes_line_breaking() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("fixed.txt");
    std::fs::write(&path, "甲乙丙丁戊己庚辛壬癸。").expect("写临时正文");
    let path = path.to_str().unwrap();

    let run = |mode: &str| -> serde_json::Value {
        let (code, stdout, stderr) = run_layout(&[
            "layout",
            "--text",
            path,
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
            "--paragraph-mode",
            mode,
        ]);
        assert_eq!(code, 0, "mode={mode} 应成功，stderr: {stderr}");
        serde_json::from_str(&stdout).expect("输出应是 JSON")
    };

    let indent = run("indent");
    let gap = run("gap");

    // 回显：模式如实出现在几何里
    assert_eq!(indent["geometry"]["paragraph_mode"], "indent");
    assert_eq!(gap["geometry"]["paragraph_mode"], "gap");

    let line_count = |value: &serde_json::Value| -> usize {
        value["paragraphs"][0]["lines"]
            .as_array()
            .map_or(0, Vec::len)
    };

    // 缩进模式：2em 缩进占掉可用宽 → 折行
    assert_eq!(
        line_count(&indent),
        2,
        "缩进模式下 11 字应折两行（228 − 38 = 190px 可用宽）"
    );
    // 间距模式：无首行缩进 → 228px 容 12 字身 → 单行
    assert_eq!(line_count(&gap), 1, "间距模式下无首行缩进，11 字应单行");

    // 两模式的**断行结果必须不同**——这是"模式真的生效了"的核心判据
    assert_ne!(
        line_count(&indent),
        line_count(&gap),
        "两模式的断行必须不同，否则模式未被施加"
    );
}
