//! `[WIP]` 组合根：装配生产引擎 worker（S3 T010/T013/T014）
//!
//! ## 为什么这层必须在 `tome-app` 里
//!
//! 装配需要**同时看见** `tome-store` 的 `Db`/`DataDir` 与 `tome-core` 的接缝
//! trait——那是组合根的定义。`tome-app` 是第二个组合根（第一个是 `tome-cli`），
//! 依赖图守卫对此有豁免（`dependency_graph.rs` 的 `ENGINE_CRATES` 不含组合根）。
//!
//! ## 装配顺序（照 `tome-cli/src/root.rs` 的形态，用自己的入参）
//!
//! ```text
//!   ① 数据目录判定（便携优先）
//!   ② DataDir::prepare（建目录布局 + 空间检测）
//!   ③ 读 config.toml（非法即错，不静默回落）
//!   ④ Db::open（建连接 + 迁移）
//!   ⑤ SqliteBookRepository（接缝的生产实现，T010）
//!   ⑥ AppEngine（引擎线程上的 worker）
//! ```
//!
//! ## 与 CLI 的 `AppRoot` 为什么不能复用
//!
//! 三条硬理由（`research-api-surface.md` §1.2 有完整论证）：它的入参是
//! `tome_cli::flags::GlobalOptions`（依赖方向禁止）、它**返回具体类型**
//! （`&Db` / `&SourceStore`——上层被迫看见实现 crate 的内部类型）、它是
//! CLI 形态的（"一次进程一次命令"的生命周期，没有重载/重启语义）。
//!
//! 可复用**的是它内部那些独立构建块函数**（`DataDir::resolve_for_current_exe`
//! / `Db::open` / `Config::load`）——本模块照抄那个顺序。

use std::path::PathBuf;
use std::sync::Arc;

use tome_core::seam::BookRepository;
use tome_store::book_repository::SqliteBookRepository;
use tome_store::config::Config;
use tome_store::datadir::DataDir;
use tome_store::db::Db;

use crate::engine::thread::{EngineHandle, StartupFailure};
use crate::engine::worker::AppEngine;

/// 装配结果：引擎句柄 + 启动期需要告知用户的诊断。
pub struct Composition {
    /// 引擎线程句柄（`TomeApp` 每帧取事件）
    pub engine: EngineHandle,
    /// 启动期的诊断（空间告警、配置回落等）——**不静默**
    pub warnings: Vec<String>,
}

/// 装配并启动引擎线程。
///
/// ## 为什么启动失败返回 `Err` 而不是发事件
///
/// FR-023 / S1 FR-043 要求"明确报告并给出指引，**而非静默失败**"。若改成
/// "失败时发一个事件"，上层就可能拿着一个永不服务事件的句柄继续跑——用户
/// 看到的是一个空书架，而真相是"数据目录不可写"。拿不到句柄就没有后续，
/// 这是**结构性**的防静默。
///
/// # Errors
///
/// 数据目录不可用、库无法打开、配置非法时返回带指引的
/// [`StartupFailure`]。
pub fn assemble() -> Result<Composition, StartupFailure> {
    let mut warnings = Vec::new();

    // --- ① 数据目录（便携优先：可执行文件旁的 `data/`）---
    let resolved = DataDir::resolve_for_current_exe(None);
    let data_dir = DataDir::from_resolved(&resolved).ok_or_else(|| {
        StartupFailure::new(
            error_view(&tome_core::ErrorKind::Storage),
            format!("无法确定数据目录（{resolved:?}）"),
            "请确认程序目录可写，或用便携模式（在程序旁建一个 data 目录）",
        )
    })?;

    // --- ② 建目录布局 + 空间检测 ---
    match data_dir.prepare() {
        Ok(Some(space_warning)) => {
            // 空间告警**如实呈现**（`describe()` 产的是中性诊断串，非面向用户文案）
            warnings.push(space_warning.describe());
        }
        Ok(None) => {}
        Err(error) => {
            return Err(StartupFailure::new(
                error_view(&error.to_error_kind()),
                format!("数据目录不可用：{}（{error}）", data_dir.root().display()),
                "请检查该目录的写权限与磁盘剩余空间",
            ))
        }
    }

    // --- ③ 配置（非法即错，不静默用默认值）---
    let config_path = data_dir.config_path();
    let config = if config_path.exists() {
        Config::load(&config_path).map_err(|error| {
            StartupFailure::new(
                error_view(&error.to_error_kind()),
                format!("配置文件无法解析：{}（{error}）", config_path.display()),
                "请修正或删除该文件后重启",
            )
        })?
    } else {
        Config::default()
    };
    let _ = config.network();
    let _ = config.storage();
    let _ = config.display();
    let _ = config.http_log();

    // --- ④ 库（含迁移；失败即无库，不留半开的连接）---
    let db = Db::open(&data_dir.database_path()).map_err(|error| {
        StartupFailure::new(
            error_view(&error.to_error_kind()),
            format!("数据库无法打开：{error}"),
            "若反复出现，请检查数据目录是否可写；损坏的库可改名后重启以重建",
        )
    })?;

    // --- ⑤ 接缝的生产实现（T010）---
    let repository: Arc<dyn BookRepository + Send + Sync> =
        Arc::new(SqliteBookRepository::new(db, &data_dir));

    // --- ⑥ 引擎线程 ---
    //
    // 闭包必须 `move`：`EngineHandle::spawn` 要求 `'static`，而 worker 的生命
    // 周期就是线程的生命周期（它 `move` 进去之后不再离开，见 `thread.rs`）。
    let engine = EngineHandle::spawn(move || Ok(AppEngine::new(repository)))
        .map_err(|failure| decorate(failure, &data_dir))?;

    Ok(Composition { engine, warnings })
}

/// 数据目录（供 `main.rs` 打日志）。
#[must_use]
pub fn data_root() -> Option<PathBuf> {
    DataDir::from_resolved(&DataDir::resolve_for_current_exe(None))
        .map(|dir| dir.root().to_path_buf())
}

/// 启动失败的错误视图（`ClassifiedErrorView` 的构造需要经 core 的类型）。
fn error_view(kind: &tome_core::ErrorKind) -> crate::engine::messages::ClassifiedErrorView {
    crate::engine::messages::ClassifiedErrorView::from_kind(kind)
}

/// 给启动失败补上数据目录的上下文。
///
/// ## 为什么不改 `StartupFailure` 的语义
///
/// 它已经带"标识 + 细节 + 指引"三件套（见其文档）。这里只在细节后面追加
/// **哪个目录**——那是用户唯一能自己检查的东西，缺了它指引就无从执行。
fn decorate(failure: StartupFailure, data_dir: &DataDir) -> StartupFailure {
    StartupFailure::new(
        failure.error().clone(),
        format!(
            "{}（数据目录：{}）",
            failure.detail(),
            data_dir.root().display()
        ),
        failure.guidance(),
    )
}
