//! T051 验收：快照资产与发布物的边界（SC-017 的"dev-only"要求）
//!
//! ## 判据（T051 验收子项）
//!
//! > `cargo package -p tome-reader --list` 断言不含 `tests/snapshots/`，
//! > 且 `assets/fonts/` 仅含 `.otf`/`OFL.txt`/`FONTS.sha256`
//!
//! ## 为什么这两条要各有一个测试
//!
//! "快照基线资产确认 dev-only 不进发布包"是 SC-017 的体积纪律之一：快照基线
//! 是**测试资产**，进了发布包就是"与功能无关的膨胀"。断言的形式取决于资产的
//! 实际形态——**本仓库的快照比对在内存中进行，不落盘 PNG 基线**，故
//! `tests/snapshots/` 目录本就不存在。把这条断言写出来（而不是靠"反正没有"）
//! 的价值在于：**将来若有人引入落盘基线**，本测试会红，提示"它现在会进包吗"。
//!
//! `assets/fonts/` 的断言同理：字体是**唯一**的发布资源，它必须只含三类文件
//! （字体重命名所需的 OFL 许可文本、子集、校验清单）。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音（与 corpus_loading.rs 同一约定）"
)]

use std::path::PathBuf;
use std::process::Command;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    crate_dir().join("../..")
}

/// 跑 `cargo package -p tome-reader --list`，返回文件清单。
///
/// ## 为什么带 `--allow-dirty`
///
/// 本测试在开发期随时可能跑（工作树有未提交改动是常态）。`--list` 本身不做
/// 构建验证，`--allow-dirty` 只影响"工作树脏时是否拒绝"，不影响清单内容。
fn package_list() -> Vec<String> {
    let output = Command::new(env!("CARGO"))
        .args(["package", "-p", "tome-reader", "--list", "--allow-dirty"])
        .current_dir(repo_root())
        .output()
        .expect("应能运行 cargo package");
    assert!(
        output.status.success(),
        "cargo package --list 应成功（T051 的验收命令）：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// **发布包不含快照基线资产**（T051 验收子项）。
#[test]
fn release_package_excludes_snapshot_assets() {
    let files = package_list();

    let snapshots: Vec<&String> = files
        .iter()
        .filter(|path| path.contains("snapshots") || path.ends_with(".png"))
        .collect();
    assert!(
        snapshots.is_empty(),
        "发布包不得含快照基线资产（SC-017 的 dev-only 要求），实测：{snapshots:?}"
    );

    // 前提校验：确认 `--list` 真的列出了测试文件（否则上面的断言是空话——
    // 一个只列 src/ 的清单同样不含 snapshots）
    assert!(
        files.iter().any(|path| path.starts_with("tests/")),
        "清单里应有 tests/ 条目（否则「不含快照资产」这条断言没有分辨力）"
    );
}

/// **`assets/fonts/` 仅含三类文件**（T051 验收子项）。
///
/// ## 三类各自的角色
///
/// | 文件 | 角色 |
/// |---|---|
/// | `*.otf` | 内嵌正文衬线子集（`include_bytes!` 的输入） |
/// | `OFL.txt` | SIL OFL 1.1 许可文本——R3 的**改名**是 OFL RFN 条款的要求，许可须随资产分发 |
/// | `FONTS.sha256` | 子集的校验清单（构建与 CI 用它确认资产未被替换） |
///
/// ## 为什么断言"恰好三类"而非"至少三类"
///
/// 多出来的文件（设计稿、构建中间产物、字体源文件）会进发布包——那正是
/// SC-017 说的"为打包而放的冗余资源"。用白名单断言让新增文件必须显式改本测试。
#[test]
fn font_assets_contain_only_the_three_expected_kinds() {
    let fonts_dir = repo_root().join("assets/fonts");
    assert!(
        fonts_dir.is_dir(),
        "字体资产目录应存在：{}",
        fonts_dir.display()
    );

    let mut unexpected = Vec::new();
    let mut kinds = (0usize, 0usize, 0usize); // (otf, OFL.txt, FONTS.sha256)
    let entries = std::fs::read_dir(&fonts_dir).expect("应能读字体目录");
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            unexpected.push(format!("{}（目录）", path.display()));
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.ends_with(".otf") {
            kinds.0 += 1;
        } else if name == "OFL.txt" {
            kinds.1 += 1;
        } else if name == "FONTS.sha256" {
            kinds.2 += 1;
        } else {
            unexpected.push(name.to_owned());
        }
    }

    assert!(
        unexpected.is_empty(),
        "assets/fonts/ 只应含 .otf / OFL.txt / FONTS.sha256，多出的会进发布包（SC-017）：{unexpected:?}"
    );
    assert_eq!(kinds.0, 1, "应有恰好一个字体子集 .otf");
    assert_eq!(kinds.1, 1, "应有 OFL 许可文本（R3 改名须随资产分发许可）");
    assert_eq!(kinds.2, 1, "应有子集校验清单");
}

/// **字体子集体积在 SC-017 预算内**（不超上限）。
///
/// ## 口径
///
/// SC-017："内嵌正文衬线字体子集 MUST 控制在 **3–5 MB** 预算内……超出预算须
/// 记录原因并提请用户裁决"。故判据取**上限**（5 MiB）——超了须解释；低于下限
/// 是好事（体积目标是 `<20 MB`），不判失败。
///
/// 实测值与其偏低的原因见 `specs/002-typesetting-renderer/size-report.md` §3.1。
#[test]
fn embedded_font_subset_is_within_budget() {
    let fonts_dir = repo_root().join("assets/fonts");
    let subset = std::fs::read_dir(&fonts_dir)
        .expect("应能读字体目录")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "otf"))
        .expect("应有 .otf 子集");

    let bytes = std::fs::metadata(&subset).expect("应能读文件元信息").len();

    /// SC-017 的预算上限（5 MB，按 1024 进制 = 5 MiB）。
    const BUDGET_BYTES: u64 = 5 * 1024 * 1024;

    assert!(
        bytes <= BUDGET_BYTES,
        "字体子集 {bytes} B 超过 SC-017 的 5 MiB 预算上限——超出须记录原因并提请用户裁决"
    );
    println!(
        "[SC-017] 字体子集 {} = {} B = {:.2} MiB（预算上限 {:.0} MiB）",
        subset.display(),
        bytes,
        bytes as f64 / (1024.0 * 1024.0),
        BUDGET_BYTES as f64 / (1024.0 * 1024.0)
    );
}

/// **体积报告在案且含必需的几项**（SC-017 的"记录在案"）。
///
/// ## 为什么断言报告的内容而不只是存在
///
/// "记录在案"若只断言文件存在，一个空文件也算在案。故逐项检查报告必须含
/// 三样：净增量数字、与目标的对比、字体子集体积。缺任一项，报告就不足以
/// 作为"移交 S3 的输入"（SC-017 的措辞）。
#[test]
fn size_report_is_on_record_with_required_sections() {
    let report_path = repo_root().join("specs/002-typesetting-renderer/size-report.md");
    assert!(
        report_path.is_file(),
        "体积报告应落盘：{}",
        report_path.display()
    );
    let report = std::fs::read_to_string(&report_path).expect("应能读报告");

    for required in ["SC-017", "净增量", "20 MB", "字体子集", "MiB"] {
        assert!(
            report.contains(required),
            "体积报告应含 `{required}`（作为移交 S3 的输入，缺一不可）"
        );
    }
    // 报告里出现的实测字节数应与真实资产一致（防报告与资产漂移）
    let subset_bytes = std::fs::metadata(fonts_dir_otf()).expect("应有字体").len();
    let with_grouping = format_with_thousands(subset_bytes);
    assert!(
        report.contains(&with_grouping) || report.contains(&subset_bytes.to_string()),
        "报告里的字体子集字节数应与实际资产一致（实测 {subset_bytes}）——报告与资产不得漂移"
    );
}

/// 千分位格式化（报告里写成 `2,284,616`；Rust 的 format 不支持 `,` 分组）。
fn format_with_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped
}

fn fonts_dir_otf() -> PathBuf {
    repo_root()
        .join("assets/fonts")
        .join("TomeSerifCN-subset.otf")
}
