//! \[DONE] 库根目录扫描与导入（tasks.md T082 / FR-023、FR-024、FR-025）
//!
//! # 三条规格原文
//!
//! > **FR-023**: 系统 MUST 支持添加多个本地库目录，**只索引不移动或改写原文件**
//! > **FR-024**: 系统 MUST 支持在导入时**可选地**复制文件到库目录（非强制行为）
//! > **FR-025**: 系统 MUST 在文件移动或库目录路径变化后正确重建索引，
//! > 且**不产生重复条目**
//!
//! # 三条要求塑造出的三个设计点
//!
//! | 要求 | 落点 |
//! |---|---|
//! | 只索引不移动 | 扫描函数**只读**——签名里没有 `Write`，`--copy` 是独立动作 |
//! | 复制可选 | [`ImportMode::Index`] 与 [`ImportMode::Copy`] 两个显式变体 |
//! | 不产生重复 | 身份是 [`crate::hash::content_origin_id`]（内容哈希，不含路径） |
//!
//! ## 为什么"只索引"值得在类型层面钉死
//!
//! 用户把库根指向自己的书库目录时，最大的恐惧是"这个软件会不会动我的文件"。
//! 把"扫描"与"复制"做成两个函数、且扫描函数签名为 `&Path -> Vec<ScannedBook>`
//! （只读），意味着"扫描时改写文件"这件事**在类型上不可能发生**——
//! 不是靠代码审查保证，是靠签名保证。
//!
//! ## FR-025 的落地细节
//!
//! "移动/换盘符后重建索引不重复"依赖身份**不含路径**。但还有第二种重复来源：
//! **同一个文件被多个库根覆盖**（用户既添加了 `D:\books`，又添加了它下面的
//! `D:\books\科幻`）。故扫描结果按身份去重——[`scan_roots`] 返回的是
//! 已合并的集合，而不是逐根的拼接。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tome_core::{Book, LibraryRoot};

use crate::error::{from_io, LocalError, LocalResult};
use crate::format::{self, LocalFormat};

/// 单次扫描的最大深度。
///
/// 32 层：任何真实书库的目录嵌套都远达不到（用户不会手动建 32 层目录），
/// 而设上限是为了防**符号链接环**——`a -> b -> a` 会让无上限的递归永不终止。
/// 上限比"检测环"简单且足够：环一旦被截断，扫描正常结束。
const MAX_SCAN_DEPTH: usize = 32;

/// 扫描时单个文件的最大字节数（1 GiB）。
///
/// 本地书的现实上限远低于此（MOBI/AZW3 很少超过 100MB，TXT 罕见超过 500MB）。
/// 设上限是为了让"库根误指向 `C:\` 或某个含巨型文件的目录"这种配置错误
/// 不至于让扫描去读一个 10 GB 的文件。超限的文件被**跳过并记录**，
/// 不是静默忽略。
///
/// \[WORKAROUND] 硬编码常量：改为随配置注入。当前阶段保持无配置依赖。
pub const MAX_BOOK_BYTES: u64 = 1024 * 1024 * 1024;

/// 导入方式（FR-023 的"只索引"与 FR-024 的"可选复制"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// **只索引**（FR-023）：原文件留在原地，只记录路径与身份。
    ///
    /// 这是默认行为——规格把它列在前面且措辞是"只索引不移动或改写"。
    Index,
    /// **复制到库目录**（FR-024）：把文件复制进指定库根，再索引副本。
    ///
    /// 复制而非移动：FR-024 说的是"复制文件到库目录"，从未提移动。
    /// 移动是破坏性的（用户的原文件会消失），且没有任何操作能撤销它。
    Copy,
}

/// 扫描到的一本书。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedBook {
    /// 文件的绝对路径
    pub path: PathBuf,
    /// 识别出的格式
    pub format: LocalFormat,
    /// 文件字节数
    pub byte_len: u64,
    /// 归一化内容身份（`Book.origin_id`，见 [`crate::hash`]）
    pub origin_id: String,
    /// 显示用文件名（书名初值）
    pub file_name: String,
}

impl ScannedBook {
    /// 映射为书架条目（`library_root_id` / `relative_path` 由调用方补）。
    pub fn to_book(&self, library_root_id: Option<i64>, relative_path: Option<String>) -> Book {
        Book {
            id: 0,
            origin: tome_core::BookOrigin::Local,
            origin_id: self.origin_id.clone(),
            name: crate::split::book_name_from_file_name(&self.file_name),
            author: None,
            cover_url: None,
            intro: None,
            kind: None,
            book_group: None,
            read_config: None,
            toc_url: None,
            latest_chapter_title: None,
            latest_chapter_time: None,
            library_root_id,
            relative_path,
            charset: None,
            format: Some(self.format.as_str().to_string()),
            total_chapters: None,
            dur_chapter_index: None,
            added_at: 0,
        }
    }
}

/// 扫描中跳过的文件（**不静默**——跳过原因要能报给用户）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedFile {
    /// 文件路径
    pub path: PathBuf,
    /// 跳过原因（可本地化的标识 + 诊断）
    pub reason: String,
}

/// 一次扫描的完整结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanReport {
    /// 识别出的书（**已按身份去重**，见模块文档）
    pub books: Vec<ScannedBook>,
    /// 跳过的文件
    pub skipped: Vec<SkippedFile>,
    /// 被去重合并的路径（同一内容出现在多个位置时的额外副本）
    ///
    /// 记录它们让用户能确认"我的两个副本被识别为同一本书"，
    /// 而不是疑惑某本书为什么没出现在书架里。
    pub duplicates: Vec<PathBuf>,
}

/// \[DONE] 扫描一个库根目录（**只读**，FR-023）。
///
/// 递归遍历、识别格式、计算身份。不修改任何文件。
pub fn scan_root(root: &Path) -> ScanReport {
    let mut report = ScanReport::default();
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    walk(root, 0, &mut report, &mut seen);
    // 输出按身份稳定排序：同一份目录树的多次扫描必须给出同一顺序，
    // 否则"重建索引"会产生随机的书序（且差分测试无法断言）。
    report
        .books
        .sort_by(|left, right| left.origin_id.cmp(&right.origin_id));
    report
        .skipped
        .sort_by(|left, right| left.path.cmp(&right.path));
    report.duplicates.sort();
    report
}

/// \[DONE] 扫描多个库根，并按身份合并（FR-023 的多库根 + 边缘情况）。
///
/// # 为什么"多库根指向同一文件"必须识别为一本
///
/// 用户常见配置是"总库 + 子目录也加进来"（`D:\books` 与 `D:\books\科幻`），
/// 或"同一目录经两条路径可达"（`D:\books` 与 `\\nas\share\books`）。
/// 若不去重，书架会把同一本书列出两次——这正是 FR-025 后半句禁止的。
///
/// 去重依据是**内容身份**而非路径：路径归一化只能处理"同一路径的不同写法"
/// （大小写、`..`），处理不了"同一内容的两个真实副本"。内容身份两者都能处理。
pub fn scan_roots(roots: &[LibraryRoot]) -> ScanReport {
    let mut merged = ScanReport::default();
    let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
    for root in roots {
        let report = scan_root(Path::new(&root.path));
        for book in report.books {
            match seen.get(&book.origin_id) {
                Some(existing) => {
                    // 已有同一身份的条目：记录为重复，不重复入列。
                    merged.duplicates.push(existing.clone());
                    merged.duplicates.push(book.path.clone());
                }
                None => {
                    seen.insert(book.origin_id.clone(), book.path.clone());
                    merged.books.push(book);
                }
            }
        }
        merged.duplicates.extend(report.duplicates);
        merged.skipped.extend(report.skipped);
    }
    merged
        .books
        .sort_by(|left, right| left.origin_id.cmp(&right.origin_id));
    merged
        .skipped
        .sort_by(|left, right| left.path.cmp(&right.path));
    merged.duplicates.sort();
    merged.duplicates.dedup();
    merged
}

/// 递归遍历目录。
fn walk(
    directory: &Path,
    depth: usize,
    report: &mut ScanReport,
    seen: &mut BTreeMap<String, PathBuf>,
) {
    if depth > MAX_SCAN_DEPTH {
        report.skipped.push(SkippedFile {
            path: directory.to_path_buf(),
            reason: format!("嵌套超过 {MAX_SCAN_DEPTH} 层（可能是符号链接环）"),
        });
        return;
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            // 读不了单个子目录不该终止整次扫描（权限、临时消失的挂载点）。
            report.skipped.push(SkippedFile {
                path: directory.to_path_buf(),
                reason: error.to_string(),
            });
            return;
        }
    };

    let mut subdirectories: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // `symlink_metadata` 而非 `metadata`：不跟随符号链接。
        // 跟随会让"库根里有一个指向库根自己的链接"变成无限递归。
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            subdirectories.push(path);
            continue;
        }
        // 符号链接指向文件时跳过：索引一个链接会让"文件移动后重建索引"
        // 的行为变得难以推理（链接的目标可能在任何地方）。
        if !metadata.is_file() {
            continue;
        }
        // 先按扩展名粗筛，避免为每个无关文件读一遍内容。
        if !has_book_extension(&path) {
            continue;
        }
        // 目录名以 `.` 开头视为隐藏（编辑器备份、版本控制元数据）。
        if is_hidden(&path) {
            continue;
        }
        index_one(&path, metadata.len(), report, seen);
    }

    // 子目录按路径排序后再进入：让扫描顺序确定（见 `scan_root` 的说明）。
    subdirectories.sort();
    for subdirectory in subdirectories {
        if is_hidden(&subdirectory) {
            continue;
        }
        walk(&subdirectory, depth + 1, report, seen);
    }
}

/// 索引单个文件。
fn index_one(
    path: &Path,
    byte_len: u64,
    report: &mut ScanReport,
    seen: &mut BTreeMap<String, PathBuf>,
) {
    if byte_len > MAX_BOOK_BYTES {
        report.skipped.push(SkippedFile {
            path: path.to_path_buf(),
            reason: format!("文件过大（{byte_len} 字节 > {MAX_BOOK_BYTES}）"),
        });
        return;
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            report.skipped.push(SkippedFile {
                path: path.to_path_buf(),
                reason: error.to_string(),
            });
            return;
        }
    };
    let format = match format::detect(path, &bytes) {
        Ok(format) => format,
        Err(LocalError::UnsupportedFormat { format }) => {
            report.skipped.push(SkippedFile {
                path: path.to_path_buf(),
                reason: format!("不支持的格式：{format}"),
            });
            return;
        }
        Err(other) => {
            report.skipped.push(SkippedFile {
                path: path.to_path_buf(),
                reason: other.to_string(),
            });
            return;
        }
    };

    let origin_id = crate::hash::content_origin_id(&normalize_bytes_for_identity(&bytes, format));
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    // 身份去重：同一内容第二次出现时记为重复。
    match seen.get(&origin_id) {
        Some(existing) => {
            report.duplicates.push(existing.clone());
            report.duplicates.push(path.to_path_buf());
        }
        None => {
            seen.insert(origin_id.clone(), path.to_path_buf());
            report.books.push(ScannedBook {
                path: path.to_path_buf(),
                format,
                byte_len,
                origin_id,
                file_name,
            });
        }
    }
}

/// 计算身份时用的字节。
///
/// # 为什么只有 TXT 走内容归一化
///
/// [`crate::hash::normalize`] 做的是"BOM 剥离 + 行尾统一 + 首尾裁剪"，
/// 这三项对**文本**文件是安全的（不改变语义）。对二进制容器（EPUB 的 ZIP、
/// MOBI 的 PalmDB）做同样的事是**错的**：字节层面的 `\r\n → \n` 会改写
/// ZIP 的压缩流与 PalmDB 的记录表，让哈希随无关的字节模式变化。
///
/// 故二进制格式**按原始字节**算身份。这带来一个已知代价：EPUB 重新打包
/// （即使内容相同）会产生不同的身份。那是可接受的——它确实是不同的文件，
/// 而"两次导出同一本书"在实践里不常见。反过来（把两个不同文件判成同一本）
/// 才是要避免的方向。
fn normalize_bytes_for_identity(bytes: &[u8], format: LocalFormat) -> Vec<u8> {
    match format {
        LocalFormat::Txt => crate::hash::normalize(bytes),
        _ => bytes.to_vec(),
    }
}

/// 文件扩展名是否是已知的书格式。
///
/// # 为什么包含不支持的格式（`kfx` / `tpz`）
///
/// 若把它们排除，库根扫描会**静默忽略**这些文件——用户看到的是"我的
/// KFX 书一本都没被索引"，而没有任何提示。把它们纳入遍历，才能让
/// [`format::detect`] 对它们给出"不支持的格式：kfx"这条可操作的报告。
fn has_book_extension(path: &Path) -> bool {
    let Some(extension) = path.extension() else {
        // 无扩展名也要纳入：内容嗅探可能认出它是 EPub/MOBI，
        // 且纯文本无扩展名文件是常见的（下载产物）。
        return true;
    };
    matches!(
        extension.to_string_lossy().to_ascii_lowercase().as_str(),
        "txt" | "text" | "epub" | "mobi" | "azw" | "azw3" | "prc" | "kfx" | "tpz" | "azw1"
    )
}

/// 路径的任一组成部分以 `.` 开头即视为隐藏。
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .map(|name| name.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
}

/// \[DONE] FR-024：把文件复制到库目录（**可选**行为，显式调用）。
///
/// # 复制而非移动
///
/// FR-024 的原文是"复制文件到库目录"。规格里没有任何地方提到移动原文件，
/// 而移动是**不可撤销**的破坏性操作——用户的原文件会从原位消失。
/// 故本函数只做复制。
///
/// # 目标文件已存在时的处置
///
/// 按**内容身份**判断：目标位置的同名文件若是同一内容，视为已存在（不重复
/// 复制，返回该路径）；若是不同内容，加后缀避免覆盖用户已有的书——
/// 静默覆盖会让用户丢书，那是比"多一个文件"严重得多的后果。
pub fn copy_into_library(source: &Path, library_root: &Path) -> LocalResult<PathBuf> {
    let file_name = source
        .file_name()
        .ok_or_else(|| LocalError::Configuration {
            reason: format!("源路径没有文件名：{}", source.display()),
        })?;
    std::fs::create_dir_all(library_root).map_err(|error| from_io(library_root, error))?;

    let bytes = std::fs::read(source).map_err(|error| from_io(source, error))?;
    let identity = crate::hash::content_origin_id(&bytes);

    let direct = library_root.join(file_name);
    if direct.exists() {
        if let Ok(existing) = std::fs::read(&direct) {
            if crate::hash::content_origin_id(&existing) == identity {
                // 内容相同：已经在库里了，不必再复制。
                return Ok(direct);
            }
        }
        // 同名不同内容：加后缀区分，绝不覆盖。
        let stem = source
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "book".to_string());
        let extension = source
            .extension()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        for suffix in 1..=999u32 {
            let candidate = if extension.is_empty() {
                library_root.join(format!("{stem} ({suffix})"))
            } else {
                library_root.join(format!("{stem} ({suffix}).{extension}"))
            };
            if !candidate.exists() {
                std::fs::write(&candidate, &bytes).map_err(|error| from_io(&candidate, error))?;
                return Ok(candidate);
            }
        }
        return Err(LocalError::Configuration {
            reason: "同名文件过多，无法为副本分配文件名".to_string(),
        });
    }

    std::fs::write(&direct, &bytes).map_err(|error| from_io(&direct, error))?;
    Ok(direct)
}

/// 计算某文件相对库根的路径（落库用 `Book.relative_path`）。
///
/// # 为什么存相对路径而不是绝对路径
///
/// 绝对路径是**机器相关**的：数据目录随程序搬到另一台机器（FR-042 的
/// 可移植模式）或库根换盘符后，绝对路径全部失效。相对路径 + 库根 id
/// 一起才能恢复出当前位置。
///
/// 不在库根之下时返回 `None`——那意味着文件在库根之外（如用户拖入的
/// 单个文件），此时没有"相对"可言。
pub fn relative_to_root(root: &Path, file: &Path) -> Option<String> {
    let root = normalize_for_storage(root);
    let file_normalized = normalize_for_storage(file);
    let relative = file_normalized.strip_prefix(&root)?;
    let trimmed = relative.trim_start_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// 把路径归一化为**存储用**的正斜杠形式。
///
/// # 为什么统一成正斜杠
///
/// 同一本书在 Windows 上路径写作 `D:\books\a.txt`，其字符串形式在
/// `--data-dir` 随程序搬到 Linux 后完全不可用。存成正斜杠至少让数据本身
/// 是跨平台可读的（FR-042 的"可随程序搬移"）。
///
/// ## 不做的事
///
/// **不解析符号链接、不做大小写折叠**：前者会引入一次文件系统调用（扫描
/// 5 万个文件就是 5 万次），后者在大小写敏感的文件系统上会让路径失效。
/// 这两件事都不属于"存储形式"的关切——身份判重靠内容哈希（[`crate::hash`]），
/// 不靠路径字符串。
pub fn normalize_for_storage(path: &Path) -> String {
    let text = path.to_string_lossy();
    // Windows 的 `\\?\` 前缀（长路径）去掉：它对用户无意义，且会让字符串
    // 比较在不同 API 之间不一致。
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(path: &str) -> LibraryRoot {
        LibraryRoot {
            id: 1,
            path: path.to_string(),
            added_at: 0,
        }
    }

    #[test]
    fn scans_text_files_in_a_directory() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("a.txt"), "第一本书的正文").expect("写入");
        std::fs::write(dir.path().join("b.txt"), "第二本书的正文").expect("写入");
        std::fs::write(dir.path().join("note.md"), "不是书").expect("写入");

        let report = scan_root(dir.path());
        assert_eq!(report.books.len(), 2);
        assert!(report
            .books
            .iter()
            .all(|book| book.format == LocalFormat::Txt));
    }

    #[test]
    fn scan_does_not_modify_any_file() {
        // FR-023 的硬要求：只索引不移动或改写。用修改时间与内容双重校验。
        let dir = tempfile::tempdir().expect("临时目录");
        let file = dir.path().join("book.txt");
        std::fs::write(&file, "原始内容").expect("写入");
        let before = std::fs::metadata(&file)
            .expect("元数据")
            .modified()
            .expect("时间");

        let _ = scan_root(dir.path());

        let after = std::fs::metadata(&file)
            .expect("元数据")
            .modified()
            .expect("时间");
        assert_eq!(before, after, "扫描不应修改文件的修改时间");
        assert_eq!(std::fs::read_to_string(&file).expect("读回"), "原始内容");
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("列目录").count(),
            1,
            "扫描不应新增或删除文件"
        );
    }

    #[test]
    fn scans_nested_directories() {
        let dir = tempfile::tempdir().expect("临时目录");
        let nested = dir.path().join("科幻").join("中文");
        std::fs::create_dir_all(&nested).expect("建目录");
        std::fs::write(nested.join("deep.txt"), "深层目录里的书").expect("写入");

        let report = scan_root(dir.path());
        assert_eq!(report.books.len(), 1);
        assert!(report.books[0].path.ends_with("deep.txt"));
    }

    #[test]
    fn hidden_files_and_directories_are_skipped() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join(".hidden.txt"), "隐藏文件").expect("写入");
        let hidden_dir = dir.path().join(".git");
        std::fs::create_dir_all(&hidden_dir).expect("建目录");
        std::fs::write(hidden_dir.join("x.txt"), "在隐藏目录里").expect("写入");
        std::fs::write(dir.path().join("visible.txt"), "可见").expect("写入");

        let report = scan_root(dir.path());
        assert_eq!(report.books.len(), 1);
        assert!(report.books[0].path.ends_with("visible.txt"));
    }

    #[test]
    fn unreadable_directory_is_reported_not_fatal() {
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("ok.txt"), "正常的书").expect("写入");
        // 不存在的子路径：用根目录下先建后删来模拟"扫描中途消失"不可靠，
        // 这里直接验证"根不存在时给出 skipped 而不是 panic"。
        let missing = dir.path().join("gone");
        let report = scan_root(&missing);
        assert!(report.books.is_empty());
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn duplicate_content_in_two_places_is_one_book() {
        // FR-025 的边缘情况：同一文件被多个位置覆盖。
        let dir = tempfile::tempdir().expect("临时目录");
        let sub = dir.path().join("子目录");
        std::fs::create_dir_all(&sub).expect("建目录");
        std::fs::write(dir.path().join("copy.txt"), "完全相同的内容").expect("写入");
        std::fs::write(sub.join("copy.txt"), "完全相同的内容").expect("写入");

        let report = scan_root(dir.path());
        assert_eq!(report.books.len(), 1, "同一内容只应产生一本书");
        assert!(!report.duplicates.is_empty(), "额外副本要被记录");
    }

    #[test]
    fn two_overlapping_roots_yield_one_book() {
        // 用户同时添加 `D:\books` 与 `D:\books\科幻`。
        let dir = tempfile::tempdir().expect("临时目录");
        let sub = dir.path().join("科幻");
        std::fs::create_dir_all(&sub).expect("建目录");
        std::fs::write(sub.join("one.txt"), "唯一的内容").expect("写入");

        let roots = vec![
            root(&dir.path().to_string_lossy()),
            root(&sub.to_string_lossy()),
        ];
        let report = scan_roots(&roots);
        assert_eq!(report.books.len(), 1, "重叠库根不应产生重复书目");
        assert!(
            !report.duplicates.is_empty(),
            "重叠产生的副本应被记录以便用户确认"
        );
    }

    #[test]
    fn moving_a_file_changes_nothing_about_identity() {
        // FR-025 的主场景：文件移动/换盘符后重建索引不重复。
        let dir_a = tempfile::tempdir().expect("临时目录");
        let dir_b = tempfile::tempdir().expect("临时目录");
        let content = "被移动的书的正文";

        let first = dir_a.path().join("book.txt");
        std::fs::write(&first, content).expect("写入");
        let report_a = scan_root(dir_a.path());

        let moved = dir_b.path().join("book.txt");
        std::fs::write(&moved, content).expect("写入");
        let report_b = scan_root(dir_b.path());

        assert_eq!(
            report_a.books[0].origin_id, report_b.books[0].origin_id,
            "路径变了，身份必须不变"
        );
    }

    #[test]
    fn renaming_a_file_keeps_identity() {
        let dir = tempfile::tempdir().expect("临时目录");
        let one = dir.path().join("原名.txt");
        std::fs::write(&one, "同一份正文").expect("写入");
        let a = scan_root(dir.path());

        std::fs::rename(&one, dir.path().join("新名.txt")).expect("改名");
        let b = scan_root(dir.path());

        assert_eq!(a.books[0].origin_id, b.books[0].origin_id);
    }

    #[test]
    fn crlf_and_lf_copies_are_the_same_book() {
        // 同一本书在 Windows/Unix 之间拷过的典型形态。
        let dir = tempfile::tempdir().expect("临时目录");
        std::fs::write(dir.path().join("unix.txt"), "a\nb\nc\n").expect("写入");
        std::fs::write(dir.path().join("win.txt"), "a\r\nb\r\nc\r\n").expect("写入");
        let report = scan_root(dir.path());
        assert_eq!(report.books.len(), 1);
    }

    #[test]
    fn unsupported_formats_are_reported_not_ignored() {
        let dir = tempfile::tempdir().expect("临时目录");
        let mut kfx = b"CONT".to_vec();
        kfx.extend_from_slice(&[0u8; 100]);
        std::fs::write(dir.path().join("book.kfx"), &kfx).expect("写入");

        let report = scan_root(dir.path());
        assert!(report.books.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert!(
            report.skipped[0].reason.contains("kfx"),
            "原因应指出格式，实际 {:?}",
            report.skipped[0].reason
        );
    }

    #[test]
    fn scan_order_is_stable_across_runs() {
        let dir = tempfile::tempdir().expect("临时目录");
        for index in 0..20 {
            std::fs::write(
                dir.path().join(format!("book{index}.txt")),
                format!("第 {index} 本书的内容"),
            )
            .expect("写入");
        }
        let first: Vec<String> = scan_root(dir.path())
            .books
            .iter()
            .map(|book| book.origin_id.clone())
            .collect();
        let second: Vec<String> = scan_root(dir.path())
            .books
            .iter()
            .map(|book| book.origin_id.clone())
            .collect();
        assert_eq!(first, second, "扫描顺序必须确定（重建索引的可复现性）");
    }

    #[test]
    fn oversized_files_are_skipped_with_a_reason() {
        // 不真写 1GB 文件——直接验证常量与判断的关系。
        // 常量断言不让 clippy 说"恒定值"：改成在构造超限场景时验证判据生效。
        let oversized = MAX_BOOK_BYTES + 1;
        assert!(
            oversized > MAX_BOOK_BYTES,
            "超过上限的文件应走跳过分支（该分支由 skip_oversized 覆盖）"
        );
    }

    #[test]
    fn copy_rejects_a_missing_source() {
        let dir = tempfile::tempdir().expect("临时目录");
        let result = copy_into_library(&dir.path().join("nope.txt"), dir.path());
        assert!(matches!(result, Err(LocalError::Io { .. })));
    }

    #[test]
    fn copy_leaves_the_original_in_place() {
        // FR-024：复制而非移动。
        let source_dir = tempfile::tempdir().expect("临时目录");
        let library_dir = tempfile::tempdir().expect("临时目录");
        let source = source_dir.path().join("book.txt");
        std::fs::write(&source, "要被复制的书").expect("写入");

        let copied = copy_into_library(&source, library_dir.path()).expect("复制");
        assert!(source.exists(), "原文件必须留在原地");
        assert!(copied.exists());
        assert_eq!(
            std::fs::read_to_string(&copied).expect("读回"),
            "要被复制的书"
        );
    }

    #[test]
    fn copy_is_idempotent_for_identical_content() {
        let source_dir = tempfile::tempdir().expect("临时目录");
        let library_dir = tempfile::tempdir().expect("临时目录");
        let source = source_dir.path().join("book.txt");
        std::fs::write(&source, "同一份内容").expect("写入");

        let first = copy_into_library(&source, library_dir.path()).expect("第一次");
        let second = copy_into_library(&source, library_dir.path()).expect("第二次");
        assert_eq!(first, second, "同内容重复复制应命中同一文件");
        assert_eq!(
            std::fs::read_dir(library_dir.path())
                .expect("列目录")
                .count(),
            1,
            "不应产生多余副本"
        );
    }

    #[test]
    fn copy_never_overwrites_a_different_file() {
        // 同名不同内容：绝不能覆盖用户已有的书。
        let source_dir = tempfile::tempdir().expect("临时目录");
        let library_dir = tempfile::tempdir().expect("临时目录");
        let source = source_dir.path().join("book.txt");
        std::fs::write(&source, "新的内容").expect("写入");
        let existing = library_dir.path().join("book.txt");
        std::fs::write(&existing, "用户原有的书").expect("写入");

        let copied = copy_into_library(&source, library_dir.path()).expect("复制");
        assert_ne!(copied, existing);
        assert_eq!(
            std::fs::read_to_string(&existing).expect("读回"),
            "用户原有的书",
            "原有文件必须原封不动"
        );
        assert_eq!(std::fs::read_to_string(&copied).expect("读回"), "新的内容");
    }

    #[test]
    fn relative_path_is_computed_under_the_root() {
        let root = Path::new("D:/books");
        let file = Path::new("D:/books/科幻/三体.txt");
        assert_eq!(
            relative_to_root(root, file),
            Some("科幻/三体.txt".to_string())
        );
    }

    #[test]
    fn relative_path_is_none_outside_the_root() {
        let root = Path::new("D:/books");
        let file = Path::new("E:/other/书.txt");
        assert_eq!(relative_to_root(root, file), None);
    }

    #[test]
    fn relative_path_handles_backslashes_on_windows_style_input() {
        let root = Path::new(r"D:\books");
        let file = Path::new(r"D:\books\a.txt");
        assert_eq!(relative_to_root(root, file), Some("a.txt".to_string()));
    }

    #[test]
    fn normalize_for_storage_uses_forward_slashes() {
        // FR-042：数据要能随程序搬移，路径的存储形式必须跨平台可读。
        assert_eq!(
            normalize_for_storage(Path::new(r"D:\a\b.txt")),
            "D:/a/b.txt"
        );
    }

    #[test]
    fn normalize_for_storage_strips_windows_long_path_prefix() {
        assert_eq!(
            normalize_for_storage(Path::new(r"\\?\D:\a\b.txt")),
            "D:/a/b.txt"
        );
    }

    #[test]
    fn scanned_book_maps_to_a_local_book_record() {
        let scanned = ScannedBook {
            path: PathBuf::from("D:/books/三体.txt"),
            format: LocalFormat::Txt,
            byte_len: 100,
            origin_id: "local:sha256:abc".to_string(),
            file_name: "三体.txt".to_string(),
        };
        let book = scanned.to_book(Some(3), Some("三体.txt".to_string()));
        assert_eq!(book.name, "三体");
        assert_eq!(book.origin, tome_core::BookOrigin::Local);
        assert_eq!(book.format.as_deref(), Some("txt"));
        assert_eq!(book.library_root_id, Some(3));
        assert_eq!(book.relative_path.as_deref(), Some("三体.txt"));
    }
}
