//! `\[DONE]` 数据面命令：备份、缓存清理、完整删除（tasks.md T093 / FR-033、FR-035）
//!
//! # 本模块覆盖 `contracts/cli.md` 的哪些命令
//!
//! | 命令 | 契约 | 实现 |
//! |---|---|---|
//! | `data export [--output <file>]` | FR-033 | 走 T087 的 `BackupStore::export_json` |
//! | `data import <file>` | FR-033 | 走 `BackupStore::import_json` |
//! | `data clear-cache <chapters\|images>` | SC-017 | 走 PackStore / ImageCache |
//! | `data delete [--yes]` | FR-035 | 走 T091 的 `UserDataDeleter` |
//!
//! # 为什么这些命令集中在一个模块
//!
//! 它们都作用于**整个数据目录**（而非某个书源或某本书），共享同一个前提：
//! 要先解析数据目录、再开库。把它们放一起让"一次调用 = 一次完整的数据面
//! 操作"这件事在结构上可见。
//!
//! # `--output` 没给时写 stdout
//!
//! 与 `verify --report` 的取向一致：报告类产物默认写 stdout（可管道可 diff），
//! 显式给路径才落文件。

use std::path::Path;

use tome_core::ErrorKind;

use crate::commands::CommandOutcome;
use crate::flags::GlobalOptions;

/// `data export [--output <file>]`（FR-033）。
///
/// # 为什么不打印到 stdout 时加个提示
///
/// 备份 JSON 是**要能直接被重定向到文件**的（`tome data export > b.json`），
/// 故 stdout 必须**只有** JSON——任何"已导出 N 条"的提示都会污染它。
/// 统计信息因此只在写文件时才输出（那时 stdout 空着，提示不会干扰管道）。
pub fn run_export(options: &GlobalOptions, output: Option<&Path>) -> CommandOutcome {
    let store = match open_store(options) {
        Ok(store) => store,
        Err(outcome) => return outcome,
    };

    let json = match store.export_json() {
        Ok(json) => json,
        Err(error) => {
            return CommandOutcome::failure(
                &error.to_error_kind(),
                &format!("error.storage.backup reason={error}"),
            );
        }
    };

    // 统计（供写文件时的提示；也便于脚本核对）
    let stats = match store.export() {
        Ok(backup) => tome_store::backup::stats_of(&backup),
        Err(error) => {
            return CommandOutcome::failure(
                &error.to_error_kind(),
                &format!("error.storage.backup reason={error}"),
            );
        }
    };

    let Some(path) = output else {
        // 写 stdout：只有 JSON，不加任何提示（见函数文档）
        return CommandOutcome::with_stdout(json);
    };

    if let Err(error) = std::fs::write(path, &json) {
        return CommandOutcome::failure(
            &ErrorKind::Storage,
            &format!("error.storage.io path={} reason={error}", path.display()),
        );
    }

    let report = serde_json::json!({
        "output": path.display().to_string(),
        "bytes": json.len(),
        "sources": stats.sources,
        "books": stats.books,
        "chapters": stats.chapters,
        "progress": stats.progress,
        "bookmarks": stats.bookmarks,
        "highlights": stats.highlights,
        "memos": stats.memos,
        "rules": stats.rules,
    });
    CommandOutcome::with_stdout(format!("{report}\n"))
}

/// `data import <file>`（FR-033）。
pub fn run_import(options: &GlobalOptions, file: &Path) -> CommandOutcome {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) => {
            return CommandOutcome::failure(
                &ErrorKind::Storage,
                &format!("error.storage.io path={} reason={error}", file.display()),
            );
        }
    };

    let store = match open_store(options) {
        Ok(store) => store,
        Err(outcome) => return outcome,
    };

    let stats = match store.import_json(&text) {
        Ok(stats) => stats,
        Err(error) => {
            return CommandOutcome::failure(
                &error.to_error_kind(),
                &format!("error.storage.backup reason={error}"),
            );
        }
    };

    let report = serde_json::json!({
        "file": file.display().to_string(),
        "sources": stats.sources,
        "books": stats.books,
        "chapters": stats.chapters,
        "progress": stats.progress,
        "read_records": stats.read_records,
        "bookmarks": stats.bookmarks,
        "highlights": stats.highlights,
        "memos": stats.memos,
        "rules": stats.rules,
        "library_roots": stats.library_roots,
    });
    CommandOutcome::with_stdout(format!("{report}\n"))
}

/// `data clear-cache <chapters|images>`（SC-017）。
///
/// # SC-017 的关键约束：两种缓存**各自可清**，且都不动个人数据
///
/// `chapters` 清 Pack 容器，`images` 清图片缓存。两者互不影响，也都不碰
/// 书架/进度/痕迹（那是 FR-031 / SC-015）。本函数按类别分派，不提供
/// "全清"——契约只定义了两种独立清理，加一个"全清"会让"我到底清了什么"
/// 变得含糊。
pub fn run_clear_cache(options: &GlobalOptions, kind: &str) -> CommandOutcome {
    let data_dir = match resolve_data_dir(options) {
        Ok(dir) => dir,
        Err(outcome) => return outcome,
    };

    let db = match open_db(&data_dir) {
        Ok(db) => db,
        Err(outcome) => return outcome,
    };

    let (what, released, detail) = match kind {
        "chapters" => {
            let store = std::sync::Arc::new(tome_store::PackStore::new(
                &data_dir.packs_dir(),
                db.clone(),
            ));
            match store.clear_all() {
                Ok(report) => (
                    "chapters",
                    report.freed_bytes,
                    serde_json::json!({
                        "packs_removed": report.evicted_packs,
                        "remaining_bytes": report.remaining_bytes,
                    }),
                ),
                Err(error) => {
                    return CommandOutcome::failure(
                        &error.to_error_kind(),
                        &format!("error.storage.cache reason={error}"),
                    );
                }
            }
        }
        "images" => {
            let cache = tome_store::ImageCache::new(&data_dir.images_dir(), db.clone());
            match cache.clear() {
                Ok(report) => (
                    "images",
                    report.freed_bytes,
                    serde_json::json!({ "entries_removed": report.removed_entries }),
                ),
                Err(error) => {
                    return CommandOutcome::failure(
                        &error.to_error_kind(),
                        &format!("error.storage.cache reason={error}"),
                    );
                }
            }
        }
        other => {
            return CommandOutcome::failure(
                &ErrorKind::Configuration,
                &format!("error.configuration reason=未知的缓存类别 `{other}`（可用：chapters / images）"),
            );
        }
    };

    // ## 清理后核对：个人数据**必须**还在（FR-031 / SC-015）
    //
    // 光"清成功"不足以证明不变量成立——那条不变量说的是"清缓存不影响
    // 个人数据"。故这里主动查一次书架条数并放进报告：它既是对用户的
    // 交代，也让"清缓存把数据一起清了"这种回归在报告层面可见。
    let book_dao = tome_store::dao::BookDao::new(db.clone());
    let books_remaining = book_dao.bookshelf().map(|list| list.len()).unwrap_or(0);

    let report = serde_json::json!({
        "cleared": what,
        "released_bytes": released,
        "detail": detail,
        "bookshelf_intact": books_remaining,
    });
    CommandOutcome::with_stdout(format!("{report}\n"))
}

/// `data delete [--yes]`（FR-035 / T091）。
///
/// # 没有 `--yes` 时**不执行**，并以用法错误（2）退出
///
/// 见 `flags.rs` 里 `DataDelete` 的说明：这是不可撤销操作，脚本里一个空
/// 变量就可能触发它。退出码用 2 而非 1，让调用方能区分"压根没执行"
/// 与"执行了但失败"——前者改参数重试，后者要排查。
pub fn run_delete(options: &GlobalOptions, yes: bool) -> CommandOutcome {
    let data_dir = match resolve_data_dir(options) {
        Ok(dir) => dir,
        Err(outcome) => return outcome,
    };

    // 先算"将要删除什么"，无论是否执行都要报出来——
    // 用户按下确认前有权知道规模。
    let db = match open_db(&data_dir) {
        Ok(db) => db,
        Err(outcome) => return outcome,
    };
    let preview = {
        let deleter = tome_store::delete::UserDataDeleter::new(db.clone());
        deleter.remaining_rows().unwrap_or(0)
    };

    if !yes {
        return CommandOutcome {
            exit_code: 2,
            stdout: String::new(),
            stderr: format!(
                "error.usage 即将**完整删除**用户数据（数据库 {preview} 行 + 缓存文件 + 配置），\
                 此操作不可撤销。\n确认请加 `--yes`。\n"
            ),
        };
    }

    let deleter = tome_store::delete::UserDataDeleter::with_data_dir(db.clone(), data_dir.clone());
    let report = match deleter.delete_all() {
        Ok(report) => report,
        Err(error) => {
            return CommandOutcome::failure(
                &error.to_error_kind(),
                &format!("error.storage.delete reason={error}"),
            );
        }
    };

    // 删除后核对：库里不该再有用户数据行（独立于"我删了几行"的计数）
    let remaining = deleter.remaining_rows().unwrap_or(0);

    let payload = serde_json::json!({
        "database_rows_removed": report.database_rows,
        "per_table": report.per_table.iter().map(|(table, count)| {
            serde_json::json!({ "table": table, "rows": count })
        }).collect::<Vec<_>>(),
        "pack_files_removed": report.pack_files,
        "image_files_removed": report.image_files,
        "download_files_removed": report.download_files,
        "config_removed": report.config_removed,
        "remaining_rows": remaining,
    });
    CommandOutcome::with_stdout(format!("{payload}\n"))
}

/// 解析数据目录（各数据面命令共用的第一步）。
///
/// # 为什么复用 `DataDir::resolve_for_current_exe`
///
/// 数据目录的解析有**两种模式**（可移植：程序同级；已安装：系统数据目录），
/// 且要处理"不可写/空间不足"（FR-043）。自己再解析一遍会让两种模式
/// 出现第二份实现——而它们的分支逻辑（尤其 FR-043 的报告）不该有副本。
fn resolve_data_dir(options: &GlobalOptions) -> Result<tome_store::DataDir, CommandOutcome> {
    let resolved = tome_store::DataDir::resolve_for_current_exe(options.data_dir.as_deref());
    match &resolved {
        tome_store::datadir::Resolved::Usable { .. } => {
            let dir = tome_store::DataDir::from_resolved(&resolved).ok_or_else(|| {
                CommandOutcome::failure(
                    &ErrorKind::Storage,
                    "error.configuration reason=数据目录解析结果不可用（内部不一致）",
                )
            })?;
            if let Err(error) = dir.ensure_layout() {
                return Err(CommandOutcome::failure(
                    &error.to_error_kind(),
                    &format!("error.storage.reason={error}"),
                ));
            }
            Ok(dir)
        }
        // FR-043：不可写/空间不足时明确报告并给出指引，而非静默失败
        tome_store::datadir::Resolved::Unusable {
            reason, guidance, ..
        } => Err(CommandOutcome::failure(
            &ErrorKind::Storage,
            &format!("error.storage.data_dir_not_writable reason={reason} guidance={guidance}"),
        )),
    }
}

/// 解析数据目录（供 `pipeline_commands` 的关键词命令复用同一条解析路径）。
///
/// # 为什么公开这个内部步骤
///
/// 关键词与书源**同库**，故它们必须用**同一个**数据目录解析逻辑——否则
/// 一次解析偏差会让关键词落在另一个库里（表现为"加了看不到"）。
/// 与其让 `pipeline_commands` 自己再解析一遍（两份逻辑必然漂移），
/// 不如把这一步显式共享出来。
pub(crate) fn resolve_data_dir_for_keywords(
    options: &GlobalOptions,
) -> Result<tome_store::DataDir, CommandOutcome> {
    resolve_data_dir(options)
}

/// 打开数据库。
fn open_db(data_dir: &tome_store::DataDir) -> Result<tome_store::Db, CommandOutcome> {
    tome_store::Db::open(&data_dir.database_path()).map_err(|error| {
        CommandOutcome::failure(
            &error.to_error_kind(),
            &format!("error.storage.sqlite reason={error}"),
        )
    })
}

/// 打开备份读写器。
fn open_store(options: &GlobalOptions) -> Result<tome_store::backup::BackupStore, CommandOutcome> {
    let data_dir = resolve_data_dir(options)?;
    let db = open_db(&data_dir)?;
    Ok(tome_store::backup::BackupStore::new(db))
}
