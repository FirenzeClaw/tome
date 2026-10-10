//! [DONE] 组合根装配（tasks.md T063 / 架构决策 A1、A3）
//!
//! # 本模块做的事，以及为什么只能在这里做
//!
//! 架构铁律（`AGENTS.md` / `plan.md` 依赖图）：实现 crate 之间**只依赖
//! `tome-core`**，星型无环。故"把 `tome-net` 的实现交给 `tome-js`"这件事
//! 无法发生在任何一个实现 crate 里——`tome-js` 依赖 `tome-net` 会成环。
//!
//! 唯一允许依赖全部实现 crate 的位置就是组合根（CLI / S3 应用外壳）。
//! 本模块就是那个位置，一切"接线"集中在此：
//!
//! ```text
//!   config.toml ──读一次──► Config
//!                             │
//!         ┌───────────────────┼───────────────────┐
//!         ▼                   ▼                   ▼
//!    NetworkSettings    StorageSettings     DisplaySettings
//!         │                   │                   │
//!         ▼                   ▼                   ▼
//!   RequestExecutor     SqliteStoreHost       （上层使用）
//!    (NetHost)           (StoreHost)
//!         │                   │
//!         └─────────┬─────────┘
//!                   ▼
//!          tome-js 沙箱（HostBridge）
//! ```
//!
//! # A3：无全局 `CONFIG` 单例
//!
//! 本模块**不持有**任何 `static`/`OnceLock`。装配的产物是 [`AppRoot`]，
//! 一个**值**——由 `main` 持有并显式传给命令实现。
//!
//! ## 为什么这条不能只是"约定"
//!
//! 全局单例的代价不是"不优雅"，而是三件具体的事：
//!
//! 1. **测试无法并行**：两个测试要不同的配置就得互斥跑。
//! 2. **"哪份配置生效"变成隐式事实**：读代码时看不到配置从哪来。
//! 3. **依赖关系消失**：模块不再声明它需要什么，改配置项要全局搜。
//!
//! 故本模块的公开面里**没有任何返回引用自全局状态的东西**——
//! [`AppRoot::assemble`] 收配置、返回所有权明确的值。
//!
//! # 无界面下可完整装配（FR-036 / SC-011）
//!
//! 本模块不引任何图形依赖（`tome-cli` 的 `Cargo.toml` 里也没有）。
//! CLI 的存在本身就是这条约束的验收手段。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tome_core::{ErrorKind, NetHost, StoreHost};
use tome_store::config::{Config, HttpLogSettings, NetworkSettings, StorageSettings};
use tome_store::{DataDir, Db, SourceStore};

use crate::flags::{GlobalOptions, NetworkMode};

/// 装配好的应用根：全部实现的所有权汇聚处。
///
/// ## 为什么字段是 `Arc<dyn Trait>` 而不是具体类型
///
/// 上层（命令实现）只应看见接缝。存具体类型会让"换个实现"要改调用方，
/// 而接缝的全部意义就是让调用方不知道实现是谁。
///
/// ## 为什么 `net` 同时以 `Arc` 与 `&dyn` 两种形态被使用
///
/// `tome-js` 的 `HostBridge` 存的是 `Arc<dyn NetHost>`（沙箱要跨线程持有
/// 能力），而管线只收 `&dyn NetHost`（它不该拥有网络层）。`Arc` 可以借出
/// `&dyn`，反之不行——故根上存 `Arc`，需要借用时 `.as_ref()`。
pub struct AppRoot {
    /// 配置（装配时读入的那一份，供诊断与需要完整配置的调用方使用）
    config: Config,
    /// 数据目录
    data_dir: DataDir,
    /// 数据库句柄
    db: Db,
    /// 网络能力（`NetHost` 的真实实现）
    net: Arc<dyn NetHost>,
    /// 存储能力（`StoreHost` 的真实实现，按书源建实例）
    ///
    /// ## 为什么这里存的是"构造器"而不是一个实例
    ///
    /// `SqliteStoreHost` 绑定**单个书源 id**（缓存与 Cookie 按源分区，
    /// 见 `tome_store::host` 的说明）。而一个 CLI 进程会跑多个源，
    /// 故存的应是"给定源 id 造一个宿主"的能力。
    store_factory: Arc<StoreHostFactory>,
    /// 书源存储（SQLite + `sources.json` 投影）
    source_store: SourceStore,
    /// 启动期告警（磁盘空间偏低等）。上层负责展示——引擎不产自然语言（FR-044）。
    warnings: Vec<String>,
}

/// `StoreHost` 的构造器（按书源建实例）。
///
/// 单独成类型而不是一个闭包字段：它需要**同时**持有 `Db` 与数据目录根，
/// 而闭包捕获多个值后类型不可名状（`dyn Fn` 可以，但那样就没法在
/// 测试里构造一个"假工厂"了——本类型可以被直接实现）。
pub struct StoreHostFactory {
    db: Db,
    data_root: PathBuf,
}

impl std::fmt::Debug for StoreHostFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoreHostFactory")
            .field("data_root", &self.data_root)
            .finish_non_exhaustive()
    }
}

impl StoreHostFactory {
    /// 构造。
    pub fn new(db: Db, data_root: &Path) -> Self {
        Self {
            db,
            data_root: data_root.to_path_buf(),
        }
    }

    /// 为某个书源建一个 `StoreHost`。
    ///
    /// # 为什么每个源一个实例（而不是共享一个）
    ///
    /// 缓存与 Cookie 按 `source_id` 分区（跨源共享会让 A 源的登录 token
    /// 漏给 B 源）。把 `source_id` 绑进实例，让"写错分区"这件事在类型上
    /// 不可能发生——调用方拿到的宿主已经知道自己是谁。
    pub fn for_source(&self, source_id: &str) -> SqliteStoreHostHandle {
        SqliteStoreHostHandle {
            inner: Arc::new(tome_store::SqliteStoreHost::new(
                self.db.clone(),
                &self.data_root,
                source_id,
            )),
        }
    }
}

/// `StoreHost` 的共享句柄。
///
/// 包一层是为了同时满足两个要求：`tome_store::SqliteStoreHost` 本身
/// 不派生 `Arc`（它是具体类型），而 `HostBridge` 要 `Arc<dyn StoreHost>`。
/// 本类型把 `Arc<dyn StoreHost>` 的构造收在一处，调用方不必重复写
/// `Arc::new(...) as Arc<dyn StoreHost>`（那样的转换在泛型位置容易写错）。
#[derive(Debug, Clone)]
pub struct SqliteStoreHostHandle {
    inner: Arc<tome_store::SqliteStoreHost>,
}

impl SqliteStoreHostHandle {
    /// 作为接缝 trait 对象使用。
    pub fn as_store_host(&self) -> Arc<dyn StoreHost> {
        self.inner.clone()
    }

    /// 作为具体类型使用（需要访问 `cookie_get` 等 trait 外的方法时）。
    pub fn concrete(&self) -> &tome_store::SqliteStoreHost {
        self.inner.as_ref()
    }
}

impl std::fmt::Debug for AppRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppRoot")
            .field("data_dir", &self.data_dir.root())
            .field("warnings", &self.warnings)
            .finish_non_exhaustive()
    }
}

impl AppRoot {
    /// 装配：读配置 → 解析数据目录 → 建库 → 建网络 → 接线。
    ///
    /// # 参数里的 `config_override`
    ///
    /// 来自 `--config <path>`。`None` 时用数据目录下的 `config.toml`。
    /// 数据目录本身又可能被 `--data-dir` 覆盖——故顺序是：
    ///
    /// 1. 先按 `--data-dir`（或自动判定）定数据目录
    /// 2. 再按 `--config`（或数据目录下的 `config.toml`）读配置
    /// 3. 配置里若**也**写了 `data_dir`，以命令行优先
    ///
    /// ## 为什么命令行的数据目录优先于配置里的
    ///
    /// `--data-dir` 是用户在**这一次调用**里明确说的，而配置里的是长期设置。
    /// 反过来会让"临时指个别处的库跑一下"变成不可能——它会去配置里指定的
    /// 位置建库。
    pub fn assemble(
        options: &GlobalOptions,
        config_override: Option<&Path>,
    ) -> Result<Self, ErrorKind> {
        Self::assemble_with(options, config_override, |executor| {
            Ok(Arc::new(executor) as Arc<dyn NetHost>)
        })
    }

    /// 同 [`Self::assemble`]，但允许调用方在网络执行器构造后**再定制**它。
    ///
    /// # 为什么需要这个扩展点
    ///
    /// 测试要在不起真实网络的前提下跑完整装配（假宿主），而"用哪个宿主"
    /// 是装配的一部分。用闭包而不是给 `assemble` 加一个 `Arc<dyn NetHost>`
    /// 参数：后者会让"真实网络执行器"的构造逻辑跑到调用方去，而那是
    /// 组合根的职责（限速、代理、重放模式都在那里配）。
    pub fn assemble_with<F>(
        options: &GlobalOptions,
        config_override: Option<&Path>,
        customize_executor: F,
    ) -> Result<Self, ErrorKind>
    where
        F: FnOnce(tome_net::RequestExecutor) -> Result<Arc<dyn NetHost>, ErrorKind>,
    {
        // ① 数据目录：命令行 > 自动判定（可移植优先）
        let mut resolved = DataDir::resolve_for_current_exe(options.data_dir.as_deref());

        // 数据目录不可用：先尝试用配置里的 `data_dir` 兜一次（配置可能
        // 指向一个真正可用的位置），仍不可用才报错——FR-043 要求明确报告
        // 并给出指引，但"能自愈"比"报了错让用户自己改"好。
        if matches!(resolved, tome_store::datadir::Resolved::Unusable { .. }) {
            if let Some(from_config) = peek_config_data_dir(config_override)? {
                resolved = DataDir::resolve_for_current_exe(Some(&from_config));
            }
        }

        let data_dir = match &resolved {
            tome_store::datadir::Resolved::Usable { .. } => DataDir::from_resolved(&resolved)
                .ok_or_else(|| {
                    ErrorKind::Storage
                        .with_detail("数据目录解析结果内部不一致")
                        .kind()
                        .clone()
                })?,
            tome_store::datadir::Resolved::Unusable {
                reason, guidance, ..
            } => {
                // FR-043：明确报告 + 给出指引（而不是静默失败或 panic）
                return Err(ErrorKind::Storage
                    .with_detail(format!("{reason}；{guidance}"))
                    .kind()
                    .clone());
            }
        };

        data_dir.prepare().map_err(|error| error.to_error_kind())?;

        // ② 配置：命令行 `--config` > 数据目录下的 config.toml
        let config_path = config_override
            .map(Path::to_path_buf)
            .unwrap_or_else(|| data_dir.config_path());
        let config = Config::load(&config_path).map_err(|error| error.to_error_kind())?;

        // 收集启动期告警（上层展示；引擎不产自然语言，FR-044）
        let mut warnings = Vec::new();
        if let Some(warning) = data_dir.space_warning() {
            warnings.push(warning.describe());
        }

        // ③ 建库
        let db = Db::open(&data_dir.database_path()).map_err(|error| error.to_error_kind())?;

        // ④ 网络：按配置的类型化子集构造执行器
        let network_settings = config.network();
        let executor = build_executor_from(options, &network_settings)?;
        let net = customize_executor(executor)?;

        // ⑤ 书源存储（SQLite + sources.json 投影）
        let source_store = SourceStore::new(
            tome_store::dao::SourceDao::new(db.clone()),
            data_dir
                .root()
                .join(tome_store::source_store::SOURCE_EXPORT_FILE_NAME),
        );

        Ok(Self {
            config,
            store_factory: Arc::new(StoreHostFactory::new(db.clone(), data_dir.root())),
            data_dir,
            db,
            net,
            source_store,
            warnings,
        })
    }

    /// 完整配置（只读）。
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 数据目录。
    pub fn data_dir(&self) -> &DataDir {
        &self.data_dir
    }

    /// 数据库句柄。
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// 网络能力（接缝形态）。
    pub fn net(&self) -> Arc<dyn NetHost> {
        self.net.clone()
    }

    /// 网络能力（借用形态，管线用）。
    pub fn net_ref(&self) -> &dyn NetHost {
        self.net.as_ref()
    }

    /// 为某个书源取 `StoreHost`（A1：注入给 JS 沙箱的真实存储）。
    ///
    /// # 这是 T063 的核心验收项
    ///
    /// 此前 `tome-cli` 的 `js_host.rs` 用的是 `tome_js::MemoryStoreHost`
    /// ——一个进程内内存替身，进程退出即丢。换成本方法之后，脚本的
    /// `java.cachePut`/`setCookie` 才真的落到磁盘上（FR-026 的"重启后
    /// 保留登录态"在脚本路径上成立）。
    pub fn store_host_for(&self, source_id: &str) -> Arc<dyn StoreHost> {
        self.store_factory.for_source(source_id).as_store_host()
    }

    /// `StoreHost` 的构造器（需要按源建多个实例时用）。
    pub fn store_factory(&self) -> Arc<StoreHostFactory> {
        self.store_factory.clone()
    }

    /// 书源存储。
    pub fn sources(&self) -> &SourceStore {
        &self.source_store
    }

    /// HTTP 日志子集（注入给日志管线，T064）。
    pub fn http_log_settings(&self) -> HttpLogSettings {
        self.config.http_log()
    }

    /// 存储子集。
    pub fn storage_settings(&self) -> StorageSettings {
        self.config.storage()
    }

    /// 启动期告警（供上层展示）。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// 按配置与网络模式构造执行器。
///
/// # 三处配置来源的优先级
///
/// | 项 | 来源 |
/// |---|---|
/// | 重放/离线模式 | 命令行 `--record` / `--replay` / `--offline`（**仅本次调用**） |
/// | 默认请求间隔 | `config.toml` 的 `network.request_interval_ms` |
/// | 代理地址 | `config.toml` 的 `network.proxy`（**经 [`ProxyPolicy`] 在请求层施加**） |
///
/// 网络模式**只能**来自命令行：它是"这次跑用什么数据源"的即时决定，
/// 写进配置文件会让"上次开了离线，这次忘了关"变成一类难查的怪事。
///
/// # 代理为什么不在 builder 上配
///
/// `RequestExecutorBuilder` 只有**策略**（跟随系统 / 直连），代理**地址**
/// 是逐请求的（`RequestSpec.proxy_override`，FR-008 要求"按源可单独指定"）。
/// 配置里的 `network.proxy` 是"默认走哪个代理"，故它的落点是
/// [`ConfigProxyOverride`]——一个在请求发出前给 `proxy_override` 兜底的
/// 包装宿主。这样"源显式指定"仍然优先于"配置里的默认"，与 FR-008 的
/// 三级优先（源 > 配置 > 系统）一致。
fn build_executor_from(
    options: &GlobalOptions,
    settings: &NetworkSettings,
) -> Result<tome_net::RequestExecutor, ErrorKind> {
    let mut builder = tome_net::RequestExecutor::builder();

    if let Some(interval_ms) = settings.request_interval_ms {
        if interval_ms > 0 {
            builder =
                builder.default_interval(std::time::Duration::from_millis(interval_ms as u64));
        }
    }
    if let Some((root, mode)) = options.network.to_executor_mode() {
        builder = builder.replay(root, mode);
    }

    builder.build().map_err(|error| error.to_error_kind())
}

/// 从配置文件里**只读** `storage.data_dir` 一项（用于数据目录的兜底）。
///
/// # 为什么单独一个"偷看"函数
///
/// 数据目录决定配置文件在哪，而配置文件里又可能有数据目录——循环。
/// 解开的方式是：这里只读**显式 `--config` 指定的那个文件**（如果给了），
/// 且只取一个字段。不给 `--config` 时返回 `None`（不做隐式搜索）。
fn peek_config_data_dir(config_override: Option<&Path>) -> Result<Option<PathBuf>, ErrorKind> {
    let Some(path) = config_override else {
        return Ok(None);
    };
    let config = Config::load(path).map_err(|error| error.to_error_kind())?;
    Ok(config.storage().data_dir)
}

/// 便捷：解析网络模式为执行器的重放参数（转发给 `flags`）。
///
/// 存在只是为了给 [`build_executor_from`] 一个可读的调用点，
/// 避免它在 `builder` 上堆三行 `if let`。
pub fn executor_network_mode(mode: &NetworkMode) -> Option<(PathBuf, tome_net::ReplayMode)> {
    mode.to_executor_mode()
}

/// 把配置里的默认代理施加到请求上（FR-008 的三级优先：源 > 配置 > 系统）。
///
/// # 为什么是"包装宿主"而不是"改执行器配置"
///
/// 接缝 `NetHost` 只收 `RequestSpec`，而代理地址是 `RequestSpec` 的一部分
/// （`proxy_override`）。要让它成为"默认值"，就得在**请求发出前**补上
/// 缺省的那一份——那正是包装宿主的位置。
///
/// 包装而不是改 `RequestSpec` 的构造点：构造点有很多（搜索/目录/正文/脚本
/// 各自的规则求值），在每个点写一遍"没有就填配置里的"必然漏一处。
pub struct ConfigProxyOverride {
    inner: Arc<dyn NetHost>,
    /// 配置里的默认代理；`None` 表示不干预（跟随系统）
    proxy: Option<String>,
}

impl std::fmt::Debug for ConfigProxyOverride {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigProxyOverride")
            .field("has_proxy", &self.proxy.is_some())
            .finish_non_exhaustive()
    }
}

impl ConfigProxyOverride {
    /// 包装一个宿主，缺省代理取其配置值。
    pub fn wrap(inner: Arc<dyn NetHost>, proxy: Option<String>) -> Self {
        Self { inner, proxy }
    }
}

impl NetHost for ConfigProxyOverride {
    fn fetch(&self, mut spec: tome_core::RequestSpec) -> Result<tome_core::Response, ErrorKind> {
        // 只在**源没有显式指定**时补默认值（FR-008：源 > 配置 > 系统）。
        // 若源说了 `ProxyOverride::Direct`，那是"这个源在代理下反而不可达"
        // 的明确声明，不能被配置里的默认代理覆盖。
        if spec.proxy_override.is_none() {
            if let Some(proxy) = &self.proxy {
                spec.proxy_override = Some(tome_core::ProxyOverride::Custom(proxy.clone()));
            }
        }
        self.inner.fetch(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tome_core::{HeaderMap, RequestSpec, Response};
    use tome_store::config::Config as StoreConfig;

    /// 假宿主：不触网，按 URL 片段返回预设响应。
    struct FakeHost {
        responses: Mutex<Vec<(String, Response)>>,
        seen: Mutex<Vec<RequestSpec>>,
    }

    impl FakeHost {
        fn new(responses: Vec<(&str, &str)>) -> Self {
            Self {
                responses: Mutex::new(
                    responses
                        .into_iter()
                        .map(|(fragment, body)| {
                            (
                                fragment.to_string(),
                                Response {
                                    status: 200,
                                    headers: HeaderMap::new(),
                                    body_bytes: body.as_bytes().to_vec(),
                                    detected_charset: Some("utf-8".to_string()),
                                },
                            )
                        })
                        .collect(),
                ),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            let seen = self
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let matched = self
                .responses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|(fragment, _)| spec.url.contains(fragment.as_str()))
                .map(|(_, response)| response.clone());
            drop(seen);
            matched.ok_or(ErrorKind::Network)
        }
    }

    /// 造一个临时数据目录，返回其路径（不自动清理——由调用方用 TempGuard）。
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "tome-root-{label}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("应能建目录");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn options_with(data_dir: &Path) -> GlobalOptions {
        GlobalOptions {
            data_dir: Some(data_dir.to_path_buf()),
            ..GlobalOptions::default()
        }
    }

    fn assemble_with_fake(data_dir: &Path, config_path: Option<&Path>) -> AppRoot {
        AppRoot::assemble_with(&options_with(data_dir), config_path, |_executor| {
            Ok(Arc::new(FakeHost::new(vec![("example.com", "<html></html>")])) as Arc<dyn NetHost>)
        })
        .expect("应能装配")
    }

    #[test]
    fn 无界面下可完整装配() {
        // FR-036 / SC-011：引擎必须能在无图形环境上完整运行。
        // 本测试**不打开任何窗口**——它本身就是这条约束的验收手段。
        let dir = TempDir::new("headless");
        let root = assemble_with_fake(&dir.path, None);

        // 装配产出的每一样都要真的可用
        assert!(root.data_dir().root().is_dir(), "数据目录应已建立");
        assert!(root.data_dir().database_path().exists(), "数据库应已建立");
        root.sources().all().expect("书源存储应可用");
        root.net_ref();
    }

    #[test]
    fn 装配后不存在全局单例() {
        // A3 的结构性证据：两次装配得到两个**独立**的根，
        // 各自的数据库互不影响。若存在全局单例，第二次装配会复用第一份
        // 配置与数据库——表现为"改了配置不生效"。
        let first_dir = TempDir::new("nosigleton-a");
        let second_dir = TempDir::new("nosigleton-b");

        let first = assemble_with_fake(&first_dir.path, None);
        let second = assemble_with_fake(&second_dir.path, None);

        assert_ne!(
            first.data_dir().root(),
            second.data_dir().root(),
            "两次装配应指向各自的数据目录"
        );

        // 往第一个根里写一个源，第二个必须看不到
        let source = sample_source("https://a.com");
        first
            .sources()
            .merge(std::slice::from_ref(&source))
            .expect("应能导入");
        assert_eq!(first.sources().len().expect("应能数"), 1);
        assert_eq!(
            second.sources().len().expect("应能数"),
            0,
            "第二个根不该看到第一个根的数据——那说明有全局状态"
        );
    }

    #[test]
    fn 配置从数据目录的_config_toml_读入() {
        let dir = TempDir::new("config-read");
        // 先建目录与配置（配置必须在装配前就在）
        std::fs::create_dir_all(&dir.path).expect("应能建");
        std::fs::write(
            dir.path.join("config.toml"),
            "[network]\ntimeout_ms = 1234\nhttp_log_keep = 77\n",
        )
        .expect("应能写配置");

        let root = assemble_with_fake(&dir.path, None);
        assert_eq!(root.config().network.timeout_ms, Some(1234));
        assert_eq!(root.http_log_settings().keep, 77);
    }

    #[test]
    fn 命令行_config_覆盖数据目录下的配置() {
        let dir = TempDir::new("config-override");
        let override_path = dir.path.join("custom.toml");
        std::fs::write(&override_path, "[network]\ntimeout_ms = 9999\n").expect("应能写");
        // 数据目录下也放一份（应被忽略）
        std::fs::create_dir_all(&dir.path).expect("应能建");
        std::fs::write(dir.path.join("config.toml"), "[network]\ntimeout_ms = 1\n")
            .expect("应能写");

        let root = assemble_with_fake(&dir.path, Some(&override_path));
        assert_eq!(
            root.config().network.timeout_ms,
            Some(9999),
            "--config 指定的文件应优先"
        );
    }

    #[test]
    fn 配置缺失时用默认值装配() {
        let dir = TempDir::new("config-default");
        let root = assemble_with_fake(&dir.path, None);
        assert_eq!(root.config().network.timeout_ms, None);
        assert!(root.config().network.http_log_enabled);
    }

    #[test]
    fn 非法配置让装配失败而不是用默认值硬跑() {
        // 静默用默认值会让"我明明配了代理"变成"代理没生效"，
        // 而用户看不出原因
        let dir = TempDir::new("config-bad");
        std::fs::create_dir_all(&dir.path).expect("应能建");
        std::fs::write(dir.path.join("config.toml"), "[network\nproxy=").expect("应能写");

        let error = AppRoot::assemble_with(&options_with(&dir.path), None, |_| {
            Ok(Arc::new(FakeHost::new(vec![])) as Arc<dyn NetHost>)
        })
        .expect_err("非法配置应报错");
        assert_eq!(error, ErrorKind::Configuration);
    }

    #[test]
    fn js_沙箱拿到的是真实_storage_host_而非内存替身() {
        // T063 的核心验收项。此前 js_host.rs 用 tome_js::MemoryStoreHost，
        // 进程退出即丢——脚本的 setCookie 不会真的落盘。
        let dir = TempDir::new("real-store-host");
        let root = assemble_with_fake(&dir.path, None);

        let host = root.store_host_for("https://a.com");
        host.cache_put("k", b"v").expect("应能写");
        assert_eq!(host.cache_get("k").expect("应能读"), Some(b"v".to_vec()));

        // 关键：换一个**新的 root**（模拟重启）后，数据应还在
        drop(root);
        let reopened = assemble_with_fake(&dir.path, None);
        let reopened_host = reopened.store_host_for("https://a.com");
        assert_eq!(
            reopened_host.cache_get("k").expect("应能读"),
            Some(b"v".to_vec()),
            "重启后脚本缓存应保留——内存替身做不到这一点"
        );
    }

    #[test]
    fn store_host_按源分区() {
        let dir = TempDir::new("store-host-partition");
        let root = assemble_with_fake(&dir.path, None);

        let first = root.store_host_for("https://a.com");
        let second = root.store_host_for("https://b.com");
        first
            .cache_put("token", "A的秘密".as_bytes())
            .expect("应能写");

        assert_eq!(
            second.cache_get("token").expect("应能读"),
            None,
            "B 源不该读到 A 源的缓存（跨源凭据泄漏）"
        );
    }

    #[test]
    fn 装配不产生全局_cookie_串味() {
        // Cookie 走的是 StoreHost 的 cookie 分区（见 tome-store::host）
        let dir = TempDir::new("cookie-isolation");
        let root = assemble_with_fake(&dir.path, None);

        let first = root.store_host_for("https://a.com");
        first
            .cache_put("tome://cookie/token", b"abc")
            .expect("应能写");

        let second = root.store_host_for("https://b.com");
        assert_eq!(
            second.cache_get("tome://cookie/token").expect("应能读"),
            None,
            "同名 Cookie 不得跨源可见"
        );
        // 而同一个源自己能看到
        assert_eq!(
            first.cache_get("tome://cookie/token").expect("应能读"),
            Some(b"abc".to_vec())
        );
    }

    #[test]
    fn 数据目录由命令行指定而不读配置里的() {
        // 命令行是"这一次"的明确指定，配置是长期设置。反过来的话，
        // "临时指个别处的库跑一下"就不可能——它会去配置里的位置建库。
        let dir = TempDir::new("cli-wins");
        let config_path = dir.path.join("c.toml");
        let other = dir.path.join("elsewhere");
        std::fs::write(
            &config_path,
            format!(
                "[storage]\ndata_dir = {:?}\n",
                other.to_string_lossy().replace('\\', "/")
            ),
        )
        .expect("应能写");

        let explicit = dir.path.join("explicit");
        let root = AppRoot::assemble_with(&options_with(&explicit), Some(&config_path), |_| {
            Ok(Arc::new(FakeHost::new(vec![])) as Arc<dyn NetHost>)
        })
        .expect("应能装配");

        assert_eq!(
            root.data_dir().root(),
            explicit,
            "--data-dir 应压过配置里的 storage.data_dir"
        );
    }

    #[test]
    fn 网络模式来自命令行而非配置() {
        // 联网/离线是"这次跑用什么数据源"的即时决定，写进配置会让
        // "上次开了离线，这次忘了关"变成难查的怪事
        let dir = TempDir::new("net-mode");
        std::fs::create_dir_all(&dir.path).expect("应能建");
        // 配置里没有网络模式的字段（结构体里根本没有）——这是结构性的证据：
        // 想写也写不进去（`deny_unknown_fields` 会拒绝）
        let bad = "[network]\noffline = true\n";
        let error = StoreConfig::from_toml(bad, &dir.path.join("config.toml"))
            .expect_err("配置里不该有网络模式字段");
        assert!(error.to_string().contains("offline"), "{error}");
    }

    #[test]
    fn 书源存储经根可用() {
        let dir = TempDir::new("source-store");
        let root = assemble_with_fake(&dir.path, None);
        root.sources()
            .merge(&[sample_source("https://a.com")])
            .expect("应能导入");
        assert_eq!(root.sources().len().expect("应能数"), 1);
        assert!(
            root.sources().export_path().exists(),
            "sources.json 投影应被写出"
        );
    }

    #[test]
    fn 装配幂等_重复装配不破坏数据() {
        let dir = TempDir::new("idempotent");
        {
            let root = assemble_with_fake(&dir.path, None);
            root.sources()
                .merge(&[sample_source("https://a.com")])
                .expect("应能导入");
        }
        // 第二次装配（模拟第二次启动）
        let root = assemble_with_fake(&dir.path, None);
        assert_eq!(root.sources().len().expect("应能数"), 1, "数据应还在");
    }

    #[test]
    fn 存储子集被注入到根上() {
        let dir = TempDir::new("storage-settings");
        std::fs::create_dir_all(&dir.path).expect("应能建");
        std::fs::write(
            dir.path.join("config.toml"),
            "[storage]\nchapter_pack_bytes = 8388608\n",
        )
        .expect("应能写");

        let root = assemble_with_fake(&dir.path, None);
        assert_eq!(root.storage_settings().chapter_pack_bytes, 8_388_608);
    }

    /// 造一个最小书源。
    fn sample_source(id: &str) -> tome_core::Source {
        tome_core::Source {
            id: id.to_string(),
            name: "样例".to_string(),
            kind: tome_core::SourceKind::Book,
            group_id: None,
            enabled: true,
            book_source_type: None,
            book_url_pattern: None,
            search_url: Some("/s?q={{key}}".to_string()),
            explore_url: None,
            rule_search: Some("{\"bookList\":\"@css:li\"}".to_string()),
            rule_explore: None,
            rule_book_info: None,
            rule_toc: None,
            rule_content: None,
            header: None,
            login_url: None,
            login_ui: None,
            login_check_js: None,
            js: None,
            charset: None,
            rate_limit_ms: None,
            weight: None,
            explore_screen: None,
            comment: None,
            last_update_time: None,
            created_at: 1_700_000_000,
        }
    }
}
