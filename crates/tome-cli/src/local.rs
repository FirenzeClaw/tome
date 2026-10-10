//! [DONE] `tome local` 子命令（tasks.md T076-T083 / `contracts/cli.md`）
//!
//! # 契约原文
//!
//! ```text
//! tome local import <path> [--copy]         # 导入 TXT/EPUB/MOBI，输出解析出的目录与元数据
//! tome local toc <path>                      # 只输出目录识别结果（含用到的目录规则）
//! tome local scan <dir>                      # 库根扫描，列出可识别的书
//! ```
//!
//! # 为什么单开一个文件
//!
//! 本工作树是**并行 worktree**（`feature/p7-local-books`），另一条线正在
//! 主树的 `commands.rs`/`flags.rs` 上推进网络侧的任务。把 local 面的全部
//! 实现收在这一个文件里，让主树侧的改动只需要**几行**接线
//! （见 `commands.rs` 里带 `[P7-MERGE]` 标记的分派与 `flags.rs` 的解析），
//! 从而把合并冲突面压到最小。
//!
//! 本模块只依赖 `tome_local` 与 `tome_core`，不碰网络与存储。
//!
//! # 输出形态
//!
//! 与 CLI 其余部分一致：默认 JSON（可 diff、可被脚本消费），`--format human`
//! 给人看。JSON 的字段顺序由 `serde` 的结构体定义固定——确定性输出是
//! 差分测试的前提（`commands.rs` 的"输出确定性"一节）。

use std::path::{Path, PathBuf};

use tome_core::{ErrorKind, TxtTocRule};
use tome_local::epub::ParsedEpub;
use tome_local::mobi::ParsedMobi;
use tome_local::txt::ParsedTxt;
use tome_local::{LocalFormat, LocalResult};

use crate::commands::CommandOutcome;
use crate::flags::{GlobalOptions, OutputFormat};

/// 库根扫描时要跳过忽略的目录名（版本控制、依赖、缓存）。
const IGNORED_DIRECTORIES: [&str; 6] = [
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    "__pycache__",
    "target",
];

/// `tome local import <path> [--copy]`
pub fn run_import(
    options: &GlobalOptions,
    path: &Path,
    copy: bool,
    library_root: Option<&Path>,
) -> CommandOutcome {
    // `--copy` 必须与库根同用：契约里 `--copy` 是"复制到库目录"，
    // 没有目标库根时这个开关没有意义，故明确报用法错而不是静默忽略。
    if copy && library_root.is_none() {
        return usage_error(
            "--copy 需要库根目录（用 `--data-dir <dir>` 或 `--library <dir>` 指定）",
        );
    }

    if !path.exists() {
        return failure(
            &ErrorKind::Configuration,
            &format!("路径不存在：{}", path.display()),
        );
    }

    let parsed = match parse_any(path) {
        Ok(parsed) => parsed,
        Err(error) => return local_failure(&error),
    };

    // FR-024：可选的复制到库目录。
    let mut copy_report = None;
    if let (true, Some(root)) = (copy, library_root) {
        match tome_local::library::copy_into_library(path, root) {
            Ok(target) => {
                copy_report = Some(target.display().to_string());
            }
            Err(error) => return local_failure(&error),
        }
    }

    let document = ImportOutput {
        path: path.display().to_string(),
        format: parsed.format.as_str().to_string(),
        name: parsed.book.name.clone(),
        author: parsed.book.author.clone(),
        charset: parsed.book.charset.clone(),
        total_chapters: parsed.toc.len(),
        origin_id: parsed.book.origin_id.clone(),
        toc_rule: parsed.toc_rule.clone(),
        split_by_size: parsed.split_by_size,
        warnings: parsed.warnings.clone(),
        chapters: parsed
            .toc
            .iter()
            .enumerate()
            .map(|(index, chapter)| ChapterOutput {
                index: index as i32,
                title: chapter.title.clone(),
                locator: chapter.locator.clone(),
                char_offset: chapter.char_offset,
            })
            .collect(),
        copied_to: copy_report,
    };

    match options.format {
        OutputFormat::Json => match serde_json::to_string_pretty(&document) {
            Ok(json) => ok(format!("{json}\n")),
            Err(error) => failure(&ErrorKind::Content, &format!("输出序列化失败：{error}")),
        },
        OutputFormat::Human => ok(render_import_human(&document)),
    }
}

/// `tome local toc <path>`——只输出目录识别结果（含用到的目录规则）。
pub fn run_toc(options: &GlobalOptions, path: &Path) -> CommandOutcome {
    if !path.exists() {
        return failure(
            &ErrorKind::Configuration,
            &format!("路径不存在：{}", path.display()),
        );
    }
    let parsed = match parse_any(path) {
        Ok(parsed) => parsed,
        Err(error) => return local_failure(&error),
    };

    let document = TocOutput {
        path: path.display().to_string(),
        format: parsed.format.as_str().to_string(),
        // 契约要求"含用到的目录规则"——用到的与未用到的原因都给。
        toc_rule: parsed.toc_rule.clone(),
        split_by_size: parsed.split_by_size,
        rule_attempts: parsed.rule_attempts.clone(),
        chapter_count: parsed.toc.len(),
        chapters: parsed
            .toc
            .iter()
            .enumerate()
            .map(|(index, chapter)| ChapterOutput {
                index: index as i32,
                title: chapter.title.clone(),
                locator: chapter.locator.clone(),
                char_offset: chapter.char_offset,
            })
            .collect(),
    };

    match options.format {
        OutputFormat::Json => match serde_json::to_string_pretty(&document) {
            Ok(json) => ok(format!("{json}\n")),
            Err(error) => failure(&ErrorKind::Content, &format!("输出序列化失败：{error}")),
        },
        OutputFormat::Human => ok(render_toc_human(&document)),
    }
}

/// `tome local scan <dir>`——库根扫描，列出可识别的书。
pub fn run_scan(options: &GlobalOptions, directory: &Path, roots: &[PathBuf]) -> CommandOutcome {
    if !directory.is_dir() {
        return failure(
            &ErrorKind::Configuration,
            &format!("不是目录：{}", directory.display()),
        );
    }

    // 扫描目录本身；给了额外库根时一并扫描并按身份合并（FR-023 的多库根）。
    let mut all_roots: Vec<tome_core::LibraryRoot> = vec![tome_core::LibraryRoot {
        id: 0,
        path: directory.display().to_string(),
        added_at: 0,
    }];
    for (index, root) in roots.iter().enumerate() {
        all_roots.push(tome_core::LibraryRoot {
            id: index as i64 + 1,
            path: root.display().to_string(),
            added_at: 0,
        });
    }

    let report = tome_local::library::scan_roots(&all_roots);

    let document = ScanOutput {
        directory: directory.display().to_string(),
        roots: all_roots.iter().map(|root| root.path.clone()).collect(),
        book_count: report.books.len(),
        books: report
            .books
            .iter()
            .map(|book| ScannedBookOutput {
                path: book.path.display().to_string(),
                format: book.format.as_str().to_string(),
                byte_len: book.byte_len,
                origin_id: book.origin_id.clone(),
                file_name: book.file_name.clone(),
            })
            .collect(),
        duplicates: report
            .duplicates
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        skipped: report
            .skipped
            .iter()
            .map(|entry| SkippedOutput {
                path: entry.path.display().to_string(),
                reason: entry.reason.clone(),
            })
            .collect(),
    };

    match options.format {
        OutputFormat::Json => match serde_json::to_string_pretty(&document) {
            Ok(json) => ok(format!("{json}\n")),
            Err(error) => failure(&ErrorKind::Content, &format!("输出序列化失败：{error}")),
        },
        OutputFormat::Human => ok(render_scan_human(&document)),
    }
}

// ---------------------------------------------------------------------------
// 统一解析：把三种格式归一到同一个形状
// ---------------------------------------------------------------------------

/// 三种格式解析结果的公共形状。
struct UnifiedDocument {
    format: LocalFormat,
    book: tome_core::Book,
    toc: Vec<UnifiedChapter>,
    toc_rule: Option<String>,
    split_by_size: bool,
    rule_attempts: Vec<RuleAttemptOutput>,
    warnings: Vec<String>,
}

/// 统一的章节表示（三种格式的定位方式不同，但展示形状一致）。
struct UnifiedChapter {
    title: String,
    /// 定位信息的可读形式（`byte_offset=123` / `entry=OEBPS/a.xhtml`）
    locator: String,
    char_offset: Option<i64>,
}

/// 按内容识别格式并解析。
fn parse_any(path: &Path) -> LocalResult<UnifiedDocument> {
    let bytes = std::fs::read(path).map_err(|error| {
        tome_local::LocalError::io(path.display().to_string(), error.to_string())
    })?;
    let format = tome_local::format::detect(path, &bytes)?;

    match format {
        LocalFormat::Txt => {
            let parsed = tome_local::txt::import_bytes(
                &bytes,
                &path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                &default_txt_rules(),
            );
            Ok(unify_txt(parsed, path))
        }
        LocalFormat::Epub => {
            let parsed = tome_local::epub::import_bytes(&bytes)?;
            Ok(unify_epub(parsed, path))
        }
        LocalFormat::Mobi => {
            let parsed = tome_local::mobi::parse(&bytes)?;
            Ok(unify_mobi(parsed, path))
        }
        // `detect` 已经拒绝它们；此处是穷举的完备性要求。
        LocalFormat::Kfx | LocalFormat::Topaz => Err(tome_local::LocalError::UnsupportedFormat {
            format: format.as_str().to_string(),
        }),
    }
}

/// 默认的 TXT 目录识别规则（用户未配置时的回退集合）。
///
/// # 为什么要有一组默认规则
///
/// FR-020 要求"支持多条规则回退"，而规则的**来源**是用户的配置。但用户
/// 还没配置任何规则时，导入一本普通的中文 TXT 应当也能识别出目录——
/// 否则"导入"这个动作对绝大多数文件都只产出机械分页。
///
/// ## 规则集的选择依据
///
/// 取自中文网文生态里最常见的几种章节写法（公开的社区惯例，非任何具体
/// 实现）：`第X章`/`第X节`/`第X回`、`【X】`、纯数字编号。它们按
/// `sort_order` 回退，先精确后宽泛。
fn default_txt_rules() -> Vec<TxtTocRule> {
    vec![
        TxtTocRule {
            id: 0,
            name: "第X章/节/回".to_string(),
            rule: r"^\s*第\s*[0-9一二三四五六七八九十百千零两]+\s*[章节回卷篇].*$".to_string(),
            example: Some("第一章 开端".to_string()),
            enabled: true,
            sort_order: 0,
        },
        TxtTocRule {
            id: 0,
            name: "括注编号".to_string(),
            rule: r"^\s*【[^】]{1,30}】\s*$".to_string(),
            example: Some("【壹】开端".to_string()),
            enabled: true,
            sort_order: 1,
        },
        TxtTocRule {
            id: 0,
            name: "卷/序/番外".to_string(),
            rule: r"^\s*(卷[0-9一二三四五六七八九十百千零两]+|序章|序言|楔子|尾声|后记|番外).*$"
                .to_string(),
            example: Some("楔子".to_string()),
            enabled: true,
            sort_order: 2,
        },
        TxtTocRule {
            id: 0,
            name: "数字编号".to_string(),
            rule: r"^\s*[0-9]{1,4}[.、,，]\s*\S.*$".to_string(),
            example: Some("1. 起点".to_string()),
            enabled: true,
            sort_order: 3,
        },
    ]
}

/// TXT 解析结果归一化。
fn unify_txt(parsed: ParsedTxt, path: &Path) -> UnifiedDocument {
    let chapters = parsed
        .toc
        .iter()
        .map(|point| UnifiedChapter {
            title: point.title.clone(),
            locator: format!("byte_offset={}", point.byte_offset),
            char_offset: Some(point.char_offset),
        })
        .collect();
    let attempts = parsed
        .detection
        .attempts
        .iter()
        .map(|attempt| RuleAttemptOutput {
            rule_name: attempt.rule_name.clone(),
            sort_order: attempt.sort_order,
            form: format!("{:?}", attempt.form).to_lowercase(),
            points_found: attempt.points_found,
            rejected_because: attempt.rejected_because.clone(),
        })
        .collect();

    let mut book = parsed.book;
    book.relative_path = Some(tome_local::library::normalize_for_storage(path));

    UnifiedDocument {
        format: LocalFormat::Txt,
        book,
        toc: chapters,
        toc_rule: parsed.detection.rule_name,
        split_by_size: parsed.detection.split_by_size,
        rule_attempts: attempts,
        warnings: Vec::new(),
    }
}

/// EPUB 解析结果归一化。
fn unify_epub(parsed: ParsedEpub, path: &Path) -> UnifiedDocument {
    let chapters = parsed
        .toc
        .iter()
        .map(|point| UnifiedChapter {
            title: point.title.clone(),
            locator: format!(
                "entry={}{}",
                point.entry,
                point
                    .fragment
                    .as_ref()
                    .map(|fragment| format!("#{fragment}"))
                    .unwrap_or_default()
            ),
            char_offset: None,
        })
        .collect();

    let mut book = parsed.book;
    book.relative_path = Some(tome_local::library::normalize_for_storage(path));
    // 内嵌资源的数量是有用的诊断信息（FR-021 要求解析内嵌资源）。
    let mut warnings = parsed.warnings;
    warnings.push(format!("内嵌资源 {} 项", parsed.resources.len()));

    UnifiedDocument {
        format: LocalFormat::Epub,
        book,
        toc: chapters,
        // EPUB 的目录来自 NCX/nav 文档，不是"用户配置的目录规则"。
        toc_rule: Some("ncx/nav".to_string()),
        split_by_size: false,
        rule_attempts: Vec::new(),
        warnings,
    }
}

/// MOBI 解析结果归一化。
fn unify_mobi(parsed: ParsedMobi, path: &Path) -> UnifiedDocument {
    let chapters = parsed
        .toc
        .iter()
        .map(|chapter| UnifiedChapter {
            title: chapter.title.clone(),
            locator: format!("byte_offset={}", chapter.byte_offset),
            char_offset: Some(chapter.char_offset as i64),
        })
        .collect();

    let mut book = parsed.book;
    book.relative_path = Some(tome_local::library::normalize_for_storage(path));
    let mut warnings = parsed.warnings;
    warnings.push(format!("形态：{}", parsed.layout.as_str()));

    UnifiedDocument {
        format: LocalFormat::Mobi,
        book,
        toc: chapters,
        toc_rule: Some("guide/headings".to_string()),
        split_by_size: false,
        rule_attempts: Vec::new(),
        warnings,
    }
}

// ---------------------------------------------------------------------------
// 输出结构（serde 只在本层用——core 的 DTO 刻意不派生）
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct ImportOutput {
    path: String,
    format: String,
    name: String,
    author: Option<String>,
    charset: Option<String>,
    total_chapters: usize,
    origin_id: String,
    toc_rule: Option<String>,
    split_by_size: bool,
    warnings: Vec<String>,
    chapters: Vec<ChapterOutput>,
    copied_to: Option<String>,
}

#[derive(serde::Serialize)]
struct TocOutput {
    path: String,
    format: String,
    toc_rule: Option<String>,
    split_by_size: bool,
    rule_attempts: Vec<RuleAttemptOutput>,
    chapter_count: usize,
    chapters: Vec<ChapterOutput>,
}

#[derive(serde::Serialize, Clone)]
struct ChapterOutput {
    index: i32,
    title: String,
    locator: String,
    char_offset: Option<i64>,
}

#[derive(serde::Serialize, Clone)]
struct RuleAttemptOutput {
    rule_name: String,
    sort_order: i32,
    form: String,
    points_found: usize,
    rejected_because: Option<String>,
}

#[derive(serde::Serialize)]
struct ScanOutput {
    directory: String,
    roots: Vec<String>,
    book_count: usize,
    books: Vec<ScannedBookOutput>,
    duplicates: Vec<String>,
    skipped: Vec<SkippedOutput>,
}

#[derive(serde::Serialize)]
struct ScannedBookOutput {
    path: String,
    format: String,
    byte_len: u64,
    origin_id: String,
    file_name: String,
}

#[derive(serde::Serialize)]
struct SkippedOutput {
    path: String,
    reason: String,
}

// ---------------------------------------------------------------------------
// human 渲染
// ---------------------------------------------------------------------------

fn render_import_human(document: &ImportOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!("文件: {}\n", document.path));
    out.push_str(&format!("格式: {}\n", document.format));
    out.push_str(&format!("书名: {}\n", document.name));
    if let Some(author) = &document.author {
        out.push_str(&format!("作者: {author}\n"));
    }
    if let Some(charset) = &document.charset {
        out.push_str(&format!("编码: {charset}\n"));
    }
    out.push_str(&format!("章节: {}\n", document.total_chapters));
    match &document.toc_rule {
        Some(rule) => out.push_str(&format!("目录规则: {rule}\n")),
        None => out.push_str("目录规则: （无）\n"),
    }
    if document.split_by_size {
        out.push_str("（目录为空，已按大小切分）\n");
    }
    out.push_str(&format!("身份: {}\n", document.origin_id));
    if let Some(target) = &document.copied_to {
        out.push_str(&format!("已复制到: {target}\n"));
    }
    for warning in &document.warnings {
        out.push_str(&format!("警告: {warning}\n"));
    }
    out.push_str(&render_chapters(&document.chapters));
    out
}

fn render_toc_human(document: &TocOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!("文件: {}\n", document.path));
    out.push_str(&format!("格式: {}\n", document.format));
    match &document.toc_rule {
        Some(rule) => out.push_str(&format!("采用规则: {rule}\n")),
        None => out.push_str("采用规则: （无）\n"),
    }
    if document.split_by_size {
        out.push_str("（无规则命中，已按大小切分）\n");
    }
    if !document.rule_attempts.is_empty() {
        out.push_str("规则尝试:\n");
        for attempt in &document.rule_attempts {
            out.push_str(&format!(
                "  - {}（sort={}，{}）命中 {} 点",
                attempt.rule_name, attempt.sort_order, attempt.form, attempt.points_found
            ));
            if let Some(reason) = &attempt.rejected_because {
                out.push_str(&format!("；未采用：{reason}"));
            }
            out.push('\n');
        }
    }
    out.push_str(&format!("章节数: {}\n", document.chapter_count));
    out.push_str(&render_chapters(&document.chapters));
    out
}

fn render_scan_human(document: &ScanOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!("库根: {}\n", document.directory));
    out.push_str(&format!("识别出 {} 本书\n", document.book_count));
    for book in &document.books {
        out.push_str(&format!(
            "  [{:>5}] {}  ({})\n",
            book.format, book.file_name, book.path
        ));
    }
    if !document.duplicates.is_empty() {
        out.push_str(&format!("重复副本 {} 个:\n", document.duplicates.len()));
        for path in &document.duplicates {
            out.push_str(&format!("  {path}\n"));
        }
    }
    if !document.skipped.is_empty() {
        out.push_str(&format!("跳过 {} 个:\n", document.skipped.len()));
        for entry in &document.skipped {
            out.push_str(&format!("  {} —— {}\n", entry.path, entry.reason));
        }
    }
    out
}

fn render_chapters(chapters: &[ChapterOutput]) -> String {
    let mut out = String::new();
    for chapter in chapters {
        out.push_str(&format!(
            "  {:>4}. {}  [{}]\n",
            chapter.index + 1,
            chapter.title,
            chapter.locator
        ));
    }
    out
}

// ---------------------------------------------------------------------------
// 错误 → 命令结果
// ---------------------------------------------------------------------------

/// 本地错误 → 命令结果（退出码由 `ErrorKind` 决定）。
fn local_failure(error: &tome_local::LocalError) -> CommandOutcome {
    failure(&error.kind(), &error.to_string())
}

/// 成功（退出码 0）。
///
/// # 为什么在本模块直接构造 `CommandOutcome`
///
/// `CommandOutcome` 的字段是 `pub`，而它的构造器（`with_stdout` /
/// `failure`）在 `commands.rs` 里是**私有**的。本工作树是并行 worktree，
/// 把 `commands.rs` 的两个构造器改成 `pub` 会扩大与主线的合并面；
/// 直接构造只多了三行，且序列化面（三个 pub 字段）本就是稳定契约。
fn ok(stdout: String) -> CommandOutcome {
    CommandOutcome {
        exit_code: 0,
        stdout,
        stderr: String::new(),
    }
}

/// 失败：退出码由 `ErrorKind` 决定（`flags::exit_code_for` 是唯一定义处）。
fn failure(kind: &ErrorKind, message: &str) -> CommandOutcome {
    CommandOutcome {
        exit_code: crate::flags::exit_code_for(kind),
        stdout: String::new(),
        stderr: format!(
            "{message}
"
        ),
    }
}

/// 用法错误（契约：退出码 2）。
fn usage_error(message: &str) -> CommandOutcome {
    CommandOutcome {
        exit_code: 2,
        stdout: String::new(),
        stderr: format!(
            "用法错误：{message}
"
        ),
    }
}

/// 判断目录是否应被库根扫描忽略。
pub fn should_ignore_directory(path: &Path) -> bool {
    path.file_name()
        .map(|name| {
            let name = name.to_string_lossy();
            name.starts_with('.') || IGNORED_DIRECTORIES.contains(&name.as_ref())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_rules_cover_common_chinese_chapter_forms() {
        let rules = default_txt_rules();
        assert!(rules.len() >= 4, "应有多条规则供回退");
        assert!(rules.windows(2).all(|w| w[0].sort_order < w[1].sort_order));

        // 每条规则都要能编译（正则写错会让导入静默退化到机械分页）。
        // 用 `tome_local::toc_rule::detect` 间接验证：把规则喂给一个
        // 空文本，编译失败会被记录进 attempts 的 `rejected_because`。
        let detection = tome_local::toc_rule::detect("", &rules);
        assert_eq!(
            detection.attempts.len(),
            rules.len(),
            "每条启用的规则都应被尝试求值"
        );
        for attempt in &detection.attempts {
            if let Some(reason) = &attempt.rejected_because {
                assert!(
                    !reason.contains("正则编译失败"),
                    "规则 `{}` 的正则无法编译：{reason}",
                    attempt.rule_name
                );
            }
        }
    }

    #[test]
    fn default_rules_match_real_chapter_titles() {
        let rules = default_txt_rules();
        let sample = concat!(
            "第一章 开端\n正文\n",
            "第二百一十三章 归途\n正文\n",
            "【壹】另一本\n正文\n",
            "楔子\n正文\n",
            "1. 数字编号\n正文\n",
        );
        let detection = tome_local::toc_rule::detect(sample, &rules);
        // 第一条规则应命中「第X章」那两处。
        assert_eq!(detection.rule_name.as_deref(), Some("第X章/节/回"));
        assert!(detection.points.len() >= 2);
    }

    #[test]
    fn scan_output_serialises_deterministically() {
        let document = ScanOutput {
            directory: "/books".to_string(),
            roots: vec!["/books".to_string()],
            book_count: 1,
            books: vec![ScannedBookOutput {
                path: "/books/a.txt".to_string(),
                format: "txt".to_string(),
                byte_len: 12,
                origin_id: "local:sha256:deadbeef".to_string(),
                file_name: "a.txt".to_string(),
            }],
            duplicates: Vec::new(),
            skipped: Vec::new(),
        };
        let first = serde_json::to_string(&document).expect("序列化");
        let second = serde_json::to_string(&document).expect("序列化");
        assert_eq!(first, second, "输出必须逐字节确定");
        assert!(first.contains("local:sha256:deadbeef"));
    }

    #[test]
    fn ignored_directories_are_recognised() {
        assert!(should_ignore_directory(Path::new("/books/.git")));
        assert!(should_ignore_directory(Path::new("/books/node_modules")));
        assert!(should_ignore_directory(Path::new("/books/target")));
        assert!(!should_ignore_directory(Path::new("/books/科幻")));
    }

    #[test]
    fn human_output_lists_chapters() {
        let document = TocOutput {
            path: "/books/a.txt".to_string(),
            format: "txt".to_string(),
            toc_rule: Some("第X章".to_string()),
            split_by_size: false,
            rule_attempts: vec![RuleAttemptOutput {
                rule_name: "第X章".to_string(),
                sort_order: 0,
                form: "regex".to_string(),
                points_found: 3,
                rejected_because: None,
            }],
            chapter_count: 1,
            chapters: vec![ChapterOutput {
                index: 0,
                title: "第一章".to_string(),
                locator: "byte_offset=0".to_string(),
                char_offset: Some(0),
            }],
        };
        let rendered = render_toc_human(&document);
        assert!(rendered.contains("第一章"));
        assert!(rendered.contains("第X章"));
        assert!(rendered.contains("byte_offset=0"));
    }
}
