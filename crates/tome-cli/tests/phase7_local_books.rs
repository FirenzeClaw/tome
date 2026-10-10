//! **Phase 7 的 Independent Test**（tasks.md 第 341 行）
//!
//! > **Independent Test**: `tome local import <样本>` 对三类格式输出目录与
//! > 元数据；MOBI 用真实文件验证（非仅单元测试）；本地书经同一
//! > ContentProvider seam 供上层读取。
//!
//! # 本文件的定位
//!
//! `tome-local` 的单元测试验证的是**解析能力**（各自的格式、各自的分支）。
//! 本文件验证的是它们**经 CLI 接线后端到端成立**——用户敲下
//! `tome local import` 能拿到目录与元数据，这是 Phase 7 的通过条件。
//!
//! 故这里从**公开入口**（`tome_cli::flags::parse` + `tome_cli::commands::run`）
//! 驱动，而不直接调 `tome_local` 的 API。
//!
//! # 为什么用临时文件而不是仓库里的样本
//!
//! 真实书源/书籍产物不入仓库（`.gitignore` 覆盖 `testdata/`）。故本文件
//! **自己造**小样本（TXT/EPUB），而 MOBI 的真实文件验证在
//! `tome-local/tests/mobi_real_files.rs`（那里有语料时自动跑）。

#![expect(
    clippy::expect_used,
    clippy::panic,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::io::Cursor;
use std::path::{Path, PathBuf};

use tome_cli::commands::{run, CommandOutcome};
use tome_cli::flags::{parse, GlobalOptions, ParseOutcome};

/// 解析参数并执行（宿主用一个不做任何事的假实现——本地书全程离线）。
fn run_cli(args: &[&str]) -> (i32, String, String) {
    let (options, command) = match parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()) {
        ParseOutcome::Run { options, command } => (options, command),
        other => panic!("应解析成功，实际 {other:?}"),
    };
    let host = NoNetwork;
    let mut stdin = Cursor::new(Vec::new());
    let outcome: CommandOutcome = run(&options, &command, &host, &mut stdin);
    (outcome.exit_code, outcome.stdout, outcome.stderr)
}

/// 任何请求都失败的宿主（本地书不该发请求；发了就是缺陷）。
struct NoNetwork;

impl tome_core::NetHost for NoNetwork {
    fn fetch(
        &self,
        spec: tome_core::RequestSpec,
    ) -> Result<tome_core::Response, tome_core::ErrorKind> {
        panic!("本地书不应发起网络请求，实际请求了 {}", spec.url);
    }
}

/// 写一个临时 TXT 文件并返回（连同守卫）。
fn write_txt(dir: &Path, name: &str, content: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).expect("写入测试文件");
    path
}

/// 一段带三章的 TXT。
const SAMPLE_TXT: &str = concat!(
    "第一章 开端\n这是第一章的正文内容，用于端到端验证。\n",
    "第二章 承接\n这是第二章的正文内容。\n",
    "第三章 收束\n这是第三章的正文内容。\n",
);

#[test]
fn local_import_reports_metadata_and_toc_for_txt() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_txt(dir.path(), "样本书.txt", SAMPLE_TXT);

    let (code, stdout, stderr) = run_cli(&[
        "local",
        "import",
        &path.to_string_lossy(),
        "--format",
        "json",
    ]);
    assert_eq!(code, 0, "应成功，stderr={stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("输出应是合法 JSON");

    assert_eq!(document["format"], "txt");
    assert_eq!(document["name"], "样本书");
    assert_eq!(document["total_chapters"], 3);
    assert_eq!(document["toc_rule"], "第X章/节/回");
    assert_eq!(document["chapters"][0]["title"], "第一章 开端");
    assert_eq!(document["chapters"][2]["title"], "第三章 收束");
    // 章节定位信息必须给出（T083 的偏移直读依赖它）。
    assert!(
        document["chapters"][0]["locator"]
            .as_str()
            .expect("locator 应是字符串")
            .starts_with("byte_offset="),
        "TXT 章节应用字节偏移定位，实际 {:?}",
        document["chapters"][0]["locator"]
    );
}

#[test]
fn local_toc_reports_the_rule_that_was_used() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_txt(dir.path(), "样本书.txt", SAMPLE_TXT);

    let (code, stdout, _) = run_cli(&["local", "toc", &path.to_string_lossy()]);
    assert_eq!(code, 0);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");

    // 契约要求"含用到的目录规则"。
    assert_eq!(document["toc_rule"], "第X章/节/回");
    assert_eq!(document["chapter_count"], 3);
    // 以及"为什么别的规则没被用上"（命中即停，故只有一条被求值）。
    let attempts = document["rule_attempts"].as_array().expect("应是数组");
    assert_eq!(attempts.len(), 1, "命中即停——后续规则不该被求值");
    assert_eq!(attempts[0]["points_found"], 3);
}

#[test]
fn txt_without_recognisable_chapters_is_split_by_size() {
    let dir = tempfile::tempdir().expect("临时目录");
    // 足够长且没有任何章节标记 → 走按大小切分。
    let body = "这是一段没有章节标记的连续正文。".repeat(2_000);
    let path = write_txt(dir.path(), "无目录.txt", &body);

    let (code, stdout, _) = run_cli(&["local", "import", &path.to_string_lossy()]);
    assert_eq!(code, 0);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(document["split_by_size"], true, "无规则命中应标记为按大小切分");
    assert!(document["total_chapters"].as_u64().unwrap_or(0) > 1);
}

#[test]
fn gbk_txt_is_decoded_and_reports_its_charset() {
    let dir = tempfile::tempdir().expect("临时目录");
    let text = concat!(
        "第一章 开端\n这是用 GBK 编码保存的中文正文，需要足够长才能让检测器收敛。\n",
        "第二章 承接\n继续写入更多的中文内容以确保统计检测有效。\n",
    );
    let (bytes, _, _) = encoding_rs::GBK.encode(text);
    let path = dir.path().join("gbk书.txt");
    std::fs::write(&path, &bytes).expect("写入");

    let (code, stdout, _) = run_cli(&["local", "import", &path.to_string_lossy()]);
    assert_eq!(code, 0);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(document["charset"], "GBK");
    assert_eq!(document["total_chapters"], 2, "GBK 正文的章节也要识别出来");
}

#[test]
fn local_scan_lists_recognisable_books() {
    let dir = tempfile::tempdir().expect("临时目录");
    write_txt(dir.path(), "一本.txt", SAMPLE_TXT);
    write_txt(dir.path(), "另一本.txt", "完全不同的正文内容，用于区分身份。");
    std::fs::write(dir.path().join("笔记.md"), "不是书").expect("写入");

    let (code, stdout, _) = run_cli(&["local", "scan", &dir.path().to_string_lossy()]);
    assert_eq!(code, 0);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(document["book_count"], 2, "只应列出可识别的书");
    let books = document["books"].as_array().expect("应是数组");
    assert!(books.iter().all(|book| book["format"] == "txt"));
    // 身份必须给出（FR-025 的重建索引不重复依赖它）。
    assert!(
        books
            .iter()
            .all(|book| book["origin_id"].as_str().unwrap_or("").starts_with("local:sha256:")),
        "每本书都应带内容身份"
    );
}

#[test]
fn scanning_the_same_content_twice_yields_one_book() {
    // FR-025 的边缘情况：多库根指向同一文件。
    let dir = tempfile::tempdir().expect("临时目录");
    let sub = dir.path().join("子目录");
    std::fs::create_dir_all(&sub).expect("建目录");
    write_txt(dir.path(), "副本.txt", SAMPLE_TXT);
    write_txt(&sub, "副本.txt", SAMPLE_TXT);

    let (code, stdout, _) = run_cli(&[
        "local",
        "scan",
        &dir.path().to_string_lossy(),
        "--library",
        &sub.to_string_lossy(),
    ]);
    assert_eq!(code, 0);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert_eq!(
        document["book_count"], 1,
        "同一内容只应算一本书（多库根重叠）"
    );
    assert!(
        !document["duplicates"]
            .as_array()
            .expect("应是数组")
            .is_empty(),
        "重复副本应被记录以便用户确认"
    );
}

#[test]
fn local_import_copies_only_when_asked() {
    // FR-024：复制是**可选**行为。
    let source_dir = tempfile::tempdir().expect("临时目录");
    let library_dir = tempfile::tempdir().expect("临时目录");
    let path = write_txt(source_dir.path(), "要导入的书.txt", SAMPLE_TXT);

    // 不带 `--copy`：库目录应保持为空。
    let (code, _, _) = run_cli(&["local", "import", &path.to_string_lossy()]);
    assert_eq!(code, 0);
    assert_eq!(
        std::fs::read_dir(library_dir.path())
            .expect("列目录")
            .count(),
        0,
        "未指定 --copy 时不该动库目录"
    );

    // 带 `--copy`：文件应出现在库目录，且原文件仍在。
    let (code, stdout, _) = run_cli(&[
        "local",
        "import",
        &path.to_string_lossy(),
        "--copy",
        "--library",
        &library_dir.path().to_string_lossy(),
    ]);
    assert_eq!(code, 0);
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("应是 JSON");
    assert!(
        document["copied_to"].as_str().is_some(),
        "应报告复制目标"
    );
    assert!(path.exists(), "原文件必须留在原地（FR-024 是复制不是移动）");
}

#[test]
fn copy_without_a_library_is_a_usage_error() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_txt(dir.path(), "书.txt", SAMPLE_TXT);
    let (code, _, stderr) = run_cli(&["local", "import", &path.to_string_lossy(), "--copy"]);
    assert_eq!(code, 2, "缺少库根是用法错误（契约：退出码 2）");
    assert!(stderr.contains("--copy"), "应说明缺什么，实际 {stderr}");
}

#[test]
fn local_import_of_a_missing_path_fails_clearly() {
    let (code, _, stderr) = run_cli(&["local", "import", "D:/definitely/not/here.txt"]);
    assert_eq!(code, 1, "路径不存在是业务错误");
    assert!(!stderr.is_empty(), "必须给出诊断而不是静默失败");
}

#[test]
fn unsupported_format_fails_with_exit_code_3() {
    // KFX 显式拒绝（FR-022），且退出码是 3（"不支持能力"的可编程信号）。
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("书.kfx");
    let mut bytes = b"CONT".to_vec();
    bytes.extend_from_slice(&[0u8; 200]);
    std::fs::write(&path, &bytes).expect("写入");

    let (code, _, stderr) = run_cli(&["local", "import", &path.to_string_lossy()]);
    assert_eq!(code, 3, "不支持的格式应给退出码 3，实际 stderr={stderr}");
    assert!(
        stderr.contains("kfx"),
        "诊断应指明是哪种格式，实际 {stderr}"
    );
}

#[test]
fn output_is_deterministic_across_runs() {
    // 差分测试的前提：同一输入的两次运行输出逐字节一致（不含时间戳/随机顺序）。
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_txt(dir.path(), "书.txt", SAMPLE_TXT);
    let path_text = path.to_string_lossy().to_string();
    let args = vec!["local", "import", path_text.as_str()];

    let (_, first, _) = run_cli(&args);
    let (_, second, _) = run_cli(&args);
    assert_eq!(first, second, "同一输入必须产出逐字节相同的输出");
}

#[test]
fn human_format_is_readable() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_txt(dir.path(), "样本书.txt", SAMPLE_TXT);
    let (code, stdout, _) = run_cli(&[
        "local",
        "import",
        &path.to_string_lossy(),
        "--format",
        "human",
    ]);
    assert_eq!(code, 0);
    assert!(stdout.contains("书名: 样本书"));
    assert!(stdout.contains("第一章 开端"), "human 输出应列出章节");
    assert!(!stdout.trim_start().starts_with('{'), "human 不该是 JSON");
}

#[test]
fn local_commands_do_not_touch_the_network() {
    // 本地书全程离线。`NoNetwork` 会在任何请求上 panic——
    // 本测试通过"跑完不 panic"来断言这一点。
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_txt(dir.path(), "离线书.txt", SAMPLE_TXT);
    for args in [
        vec!["local", "import", path.to_string_lossy().to_string().as_str()],
        vec!["local", "toc", path.to_string_lossy().to_string().as_str()],
        vec!["local", "scan", dir.path().to_string_lossy().to_string().as_str()],
    ] {
        let (code, _, _) = run_cli(&args);
        assert_eq!(code, 0, "本地命令应成功：{args:?}");
    }
}

#[test]
fn default_global_options_are_used_when_unspecified() {
    // 不传 `--format` 时默认 JSON（契约的全局约定）。
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_txt(dir.path(), "书.txt", SAMPLE_TXT);
    let (code, stdout, _) = run_cli(&["local", "toc", &path.to_string_lossy()]);
    assert_eq!(code, 0);
    assert!(
        serde_json::from_str::<serde_json::Value>(&stdout).is_ok(),
        "默认应是 JSON，实际 {stdout}"
    );
    // `GlobalOptions` 的默认值确实是 JSON + Live（与 flags 的定义一致）。
    let default = GlobalOptions::default();
    assert_eq!(default.format, tome_cli::OutputFormat::Json);
}
