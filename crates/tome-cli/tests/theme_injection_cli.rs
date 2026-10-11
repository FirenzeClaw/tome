//! T040 验收（CLI 侧）：Theme **组合根注入**的端到端路径（FR-029）
//!
//! ## 与 `tome-reader/tests/theme_injection.rs` 的分工
//!
//! 那个文件验证的是**注入面存在且可用**（下游能经 `tome-core` 取到昼夜两套
//! 实例）与**分页不读 Theme**（架构断言）。本文件验证的是**组合根真的在
//! 注入**：`tome-cli` 的 `AppRoot` 构造 `ThemeSet`，经 `HostCapabilities`
//! 传给 `render`，`render` 用它绘制——而不是自己在绘制点造主题。
//!
//! ## 判据为什么是"昼夜像素不同" + "报告回显"
//!
//! 单看"渲染成功"无法区分"用了注入的主题"与"用了硬编码的昼主题"。
//! 两个方向的判据合起来才有意义：
//!
//! | 断言 | 挡住的写法 |
//! |---|---|
//! | `--theme night` 与 `--theme day` 像素不同 | 主题参数未被消费（恒定昼） |
//! | 报告 `theme` 字段回显实参 | 参数被消费但来源不是注入值（碰巧同色） |

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音（与 render_cli.rs 同一约定）"
)]

use std::path::Path;
use std::process::Command;

/// 跑一次 CLI，返回 `(退出码, stdout, stderr)`。
fn run_cli(args: &[&str]) -> (i32, String, String) {
    // 独立 `--data-dir`：不传时并行的 tome-cli 子进程共享默认可移植
    // 数据目录（exe 同级 data/），CI 时序下并发打开同一 SQLite 库会报
    // `error.storage`（与 layout_cli.rs / render_cli.rs 同一约定）。
    let data_dir = tempfile::tempdir().expect("临时数据目录");
    let binary = env!("CARGO_BIN_EXE_tome-cli");
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

/// **组合根注入的昼/夜两实例产出不同的像素**（FR-029 的注入路径）。
#[test]
fn injected_day_and_night_themes_paint_differently() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let day_out = dir.path().join("day.png");
    let night_out = dir.path().join("night.png");

    let render = |theme: &str, out: &Path| -> (i32, String, String) {
        run_cli(&[
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
            "--theme",
            theme,
        ])
    };

    let (day_code, day_stdout, day_stderr) = render("day", &day_out);
    if day_code == 3 {
        println!("[render] SKIPPED: 本机无图形后端（FR-053 诚实跳过口径）");
        return;
    }
    assert_eq!(day_code, 0, "昼主题渲染应成功，stderr: {day_stderr}");
    let (night_code, night_stdout, night_stderr) = render("night", &night_out);
    assert_eq!(night_code, 0, "夜主题渲染应成功，stderr: {night_stderr}");

    // 报告必须回显实参——否则无法确认"我给的标识真的被消费了"
    let day_report: serde_json::Value = serde_json::from_str(&day_stdout).expect("应是 JSON");
    let night_report: serde_json::Value = serde_json::from_str(&night_stdout).expect("应是 JSON");
    assert_eq!(day_report["theme"], "day", "报告应回显昼主题标识");
    assert_eq!(night_report["theme"], "night", "报告应回显夜主题标识");

    // 两套主题的阅读区背景不同（#F5EEDC vs #000000）——像素内容必须不同
    let day_bytes = std::fs::read(&day_out).expect("应能读昼图");
    let night_bytes = std::fs::read(&night_out).expect("应能读夜图");
    assert_ne!(
        day_bytes, night_bytes,
        "昼/夜主题的渲染结果必须不同——相同说明组合根注入的主题未被消费"
    );

    // 两者都应有前景（不是"夜主题画了一片纯黑"这种平凡差异）
    for (label, report) in [("昼", &day_report), ("夜", &night_report)] {
        assert!(
            report["foreground_pixels"].as_u64().unwrap_or(0) > 0,
            "{label}主题应有前景像素：{report}"
        );
    }
}

/// **未注入主题时回落到组合根的默认构造**（不 panic、不静默失败）。
///
/// ## 这条覆盖什么
///
/// `HostCapabilities::themes` 为 `None` 的路径（测试与纯计算场景）。此时
/// `run_render` 用 `ThemeSet::default()`（即组合根的同一构造点），
/// 而不是"没有主题就不画"——后者会让调用方在缺注入时拿到一张空图。
#[test]
fn missing_injection_falls_back_to_default_theme_set() {
    // 走 main 的"无需宿主能力"分支（`--text` 直接给正文，不经 AppRoot 装配）
    // 时 `themes` 为 `None`——渲染仍应成功，且默认是昼主题。
    let dir = tempfile::tempdir().expect("临时目录");
    let text = write_sample(dir.path());
    let out = dir.path().join("fallback.png");

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

    if code == 3 {
        println!("[render] SKIPPED: 本机无图形后端（FR-053 诚实跳过口径）");
        return;
    }
    assert_eq!(
        code, 0,
        "未显式给主题时应回落到默认（昼），stderr: {stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(
        report["theme"], "day",
        "未注入时默认应为昼主题（FR-030 的默认）"
    );
}
