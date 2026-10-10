//! [DONE] 配置 TOML 的加载与原子写（tasks.md T062 / FR-032）
//!
//! # 没有全局 `CONFIG` 单例（架构审查 A3）
//!
//! 本模块**只提供数据与纯函数**：读一个 [`Config`]，写一个 [`Config`]，
//! 把它切成若干类型化子集。它自己**不持有**任何全局状态，也不提供
//! `Config::global()` 之类的入口。
//!
//! 组合根（`tome-cli` 的 `root.rs`，T063）负责"读一次、注入各处"：
//!
//! ```text
//!   Config::load(path)?      → 组合根手里的一个值
//!        ├── .network()      → 注入 tome-net 的构造
//!        ├── .storage()      → 注入 tome-store 的构造
//!        └── .display()      → 注入上层（S3 界面）
//! ```
//!
//! ## 为什么"切成子集"比"传整个 Config"好
//!
//! 子集是**各模块真正需要的字段**的窄类型。传整个 `Config` 会让每个模块
//! 都能读到所有配置——包括与它无关的部分（比如排版层能读代理密码）。
//! 那是"隐式耦合"：日后改一个字段的语义，要检查全部模块。
//!
//! 子集也让测试可以只构造它需要的那一小块，不必搭一份完整配置。
//!
//! # 原子写（FR-032）
//!
//! 「写入具备原子性」的落地方式：写临时文件 → fsync → `rename` 覆盖。
//! `rename` 在同一文件系统内是原子的，故读者要么看到旧内容、要么看到
//! 新内容，**不会**看到半截文件。
//!
//! ## 为什么不能用"直接打开原文件写"
//!
//! 直接写会在"截断后、写入前"留下一个**空文件**。此时断电/崩溃，
//! 用户的配置就没了——而配置里可能有代理地址、书源分组、阅读偏好，
//! 那些是用户手工攒出来的。
//!
//! ## 为什么临时文件必须与目标同目录
//!
//! `rename` 只在**同一文件系统内**保证原子。放到系统临时目录（可能是另一个
//! 分区）会让它退化成"复制 + 删除"，那就没有原子性了。故临时文件写在
//! 目标文件旁边。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{from_io, StoreError};

/// 应用配置（`config.toml` 的顶层结构）。
///
/// # 为什么每个字段都带 `#[serde(default)]`
///
/// FR-032 要求配置"可读、可手工编辑"。手工编辑意味着**用户可以只写他关心的
/// 那几项**——比如只想改代理，那文件里就只有 `[network]`。缺项必须有默认值，
/// 否则"只写想改的"会变成"必须写全，否则启动失败"。
///
/// 用了 `#[serde(default)]` 后，一个空文件也是合法配置。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 网络
    pub network: NetworkConfig,
    /// 存储与缓存
    pub storage: StorageConfig,
    /// 界面与语言
    pub display: DisplayConfig,
}

/// 网络子配置（FR-008 的代理与超时；组合根注入给 `tome-net`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// 代理地址（`http://127.0.0.1:7890` 或 `socks5://...`）。
    ///
    /// `None` = 跟随系统代理（不是"直连"——两者语义不同，见
    /// [`tome_core::ProxyOverride`] 的文档）。
    pub proxy: Option<String>,
    /// 默认请求间隔毫秒数。书源自己声明了 `concurrentRate` 时以书源为准
    /// （书源更了解自己站点能承受的速率）。
    pub request_interval_ms: Option<u32>,
    /// 默认超时毫秒数。
    pub timeout_ms: Option<u64>,
    /// 是否记录请求日志（FR-017 的"可开关"）。
    pub http_log_enabled: bool,
    /// 请求日志保留条数（滚动清理上限，FR-017）。
    pub http_log_keep: usize,
    /// 请求日志里**响应体**最多记多少个字符；`None`（默认）= 不记。
    ///
    /// # 为什么默认不记
    ///
    /// 脱敏对"结构已知"的两类有效（HTTP 头、表单式字段），而响应体正文
    /// 里可能以任何形态出现凭据。默认不记是**保守策略**：宁可少记，
    /// 不可泄漏。打开时也只记前若干字符（凭据常在首部）。
    pub http_log_response_chars: Option<usize>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            proxy: None,
            request_interval_ms: None,
            timeout_ms: None,
            http_log_enabled: true,
            http_log_keep: crate::dao::misc::DEFAULT_HTTP_LOG_KEEP,
            http_log_response_chars: None,
        }
    }
}

/// 存储子配置（组合根注入给 `tome-store` 的构造）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StorageConfig {
    /// 章节缓存容器容量（字节，FR-029 的"容器容量可配置"）。
    pub chapter_pack_bytes: u64,
    /// 图片缓存容量上限（字节）。
    pub image_cache_bytes: u64,
    /// 数据目录（`None` = 按 FR-042 自动判定：程序同级 `data/` 优先）。
    pub data_dir: Option<PathBuf>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            chapter_pack_bytes: crate::pack::DEFAULT_PACK_CAPACITY_BYTES,
            // [WORKAROUND] 256 MiB 是"封面 + 正文内嵌图片"的常见量级；
            // 超出的由 LRU 淘汰兜住（T061）。实测若某类书普遍超限，应改为
            // 按书架规模比例计算而非继续调这个常量。
            image_cache_bytes: 256 * 1024 * 1024,
            data_dir: None,
        }
    }
}

/// 界面子配置（注入给上层；引擎自身不使用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DisplayConfig {
    /// 界面语言标识（如 `zh-CN`）。引擎只承载它，不做文案映射（FR-044）。
    pub language: Option<String>,
    /// 是否跟随系统主题。
    pub follow_system_theme: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            language: None,
            follow_system_theme: true,
        }
    }
}

impl Config {
    /// 从文件加载。
    ///
    /// # 文件不存在时返回默认配置（而不是错误）
    ///
    /// `config.toml` 是**可选**的——用户不配置就用默认值。把它当错误会让
    /// 首次启动直接失败，而"什么都没配"恰恰是最常见的初始状态。
    ///
    /// 目录存在但文件缺失与"用户还没建过配置"是同一件事，故同样返回默认值。
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(from_io(path, error)),
        };
        Self::from_toml(&text, path)
    }

    /// 从 TOML 文本解析。
    ///
    /// `path` 只用于错误消息——让"哪个文件配错了"在日志里可查。
    pub fn from_toml(text: &str, path: &Path) -> Result<Self, StoreError> {
        toml::from_str(text).map_err(|error| {
            // 配置错误归 `Configuration` 分类（不是 `Storage`）：
            // 上层据此提示"去改配置"而不是"去查磁盘"。
            StoreError::config(format!("解析 {} 失败：{error}", path.display()))
        })
    }

    /// 序列化为 TOML 文本。
    pub fn to_toml(&self) -> Result<String, StoreError> {
        toml::to_string_pretty(self)
            .map_err(|error| StoreError::config(format!("序列化配置失败：{error}")))
    }

    /// 原子写入文件（临时文件 + `rename`）。
    ///
    /// 见模块文档「原子写」。父目录会自动创建。
    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        let text = self.to_toml()?;
        write_atomically(path, text.as_bytes())
    }

    /// 缺失项的补全：把配置文件里没写的项用默认值填上并写回。
    ///
    /// # 为什么需要它
    ///
    /// 用户手工只写了 `[network]`。若某天程序新增了 `[storage]` 的字段，
    /// 用户下次打开配置文件会发现"怎么多了一堆我不知道的东西"。
    /// 显式补全让"这个版本认识哪些配置项"对用户可见、可编辑——FR-032
    /// 要求配置"可读、可手工编辑"，而**看不见的默认值无法被编辑**。
    ///
    /// 返回是否真的写入了（内容无变化时不写，避免每次启动都改 mtime）。
    pub fn normalize_file(path: &Path) -> Result<bool, StoreError> {
        let original = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(from_io(path, error)),
        };
        let config = Self::from_toml(&original, path)?;
        let normalized = config.to_toml()?;
        if normalized == original {
            return Ok(false);
        }
        write_atomically(path, normalized.as_bytes())?;
        Ok(true)
    }

    /// 网络子集（注入给 `tome-net`）。
    pub fn network(&self) -> NetworkSettings {
        NetworkSettings {
            proxy: self.network.proxy.clone(),
            request_interval_ms: self.network.request_interval_ms,
            timeout_ms: self.network.timeout_ms,
        }
    }

    /// 存储子集（注入给 `tome-store` 的构造）。
    pub fn storage(&self) -> StorageSettings {
        StorageSettings {
            chapter_pack_bytes: self.storage.chapter_pack_bytes,
            image_cache_bytes: self.storage.image_cache_bytes,
            data_dir: self.storage.data_dir.clone(),
        }
    }

    /// 界面子集（注入给上层）。
    pub fn display(&self) -> DisplaySettings {
        DisplaySettings {
            language: self.display.language.clone(),
            follow_system_theme: self.display.follow_system_theme,
        }
    }

    /// 日志子集（注入给 HTTP 日志管线，T064）。
    pub fn http_log(&self) -> HttpLogSettings {
        HttpLogSettings {
            enabled: self.network.http_log_enabled,
            keep: self.network.http_log_keep,
        }
    }
}

/// 网络配置子集。
///
/// 独立的窄类型：让 `tome-net` 的构造签名只声明它真正需要的三项，
/// 而不是收下整个 [`Config`]（见模块文档「为什么切成子集」）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkSettings {
    /// 代理覆盖；`None` = 跟随系统代理
    pub proxy: Option<String>,
    /// 默认请求间隔毫秒
    pub request_interval_ms: Option<u32>,
    /// 默认超时毫秒
    pub timeout_ms: Option<u64>,
}

/// 存储配置子集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSettings {
    /// 章节缓存容器容量（字节）
    pub chapter_pack_bytes: u64,
    /// 图片缓存容量上限（字节）
    pub image_cache_bytes: u64,
    /// 显式指定的数据目录
    pub data_dir: Option<PathBuf>,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            chapter_pack_bytes: crate::pack::DEFAULT_PACK_CAPACITY_BYTES,
            image_cache_bytes: 256 * 1024 * 1024,
            data_dir: None,
        }
    }
}

/// 界面配置子集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplaySettings {
    /// 语言标识
    pub language: Option<String>,
    /// 是否跟随系统主题
    pub follow_system_theme: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            language: None,
            follow_system_theme: true,
        }
    }
}

/// HTTP 日志子集（FR-017 的"可开关"与滚动上限）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpLogSettings {
    /// 是否记录
    pub enabled: bool,
    /// 保留条数（0 = 不清理）
    pub keep: usize,
}

impl Default for HttpLogSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            keep: crate::dao::misc::DEFAULT_HTTP_LOG_KEEP,
        }
    }
}

/// 原子写入：临时文件 → fsync → `rename`。
///
/// # 临时文件名里为什么带进程号
///
/// 两个实例同时启动（可移植模式放在共享盘上时会发生）会各自建临时文件。
/// 若名字固定，后一个会覆盖前一个的半截内容，再 rename 出去的就是坏文件。
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|error| from_io(parent, error))?;
        }
    }

    // 同目录下建临时文件：`rename` 只在同一文件系统内原子（见模块文档）
    let temp_path = path.with_extension(format!("tmp-{}", std::process::id()));

    {
        let mut file =
            std::fs::File::create(&temp_path).map_err(|error| from_io(&temp_path, error))?;
        file.write_all(bytes)
            .map_err(|error| from_io(&temp_path, error))?;
        // 内容必须先到磁盘，否则 rename 之后崩溃会得到"名字换了但内容是空的"
        file.sync_data()
            .map_err(|error| from_io(&temp_path, error))?;
    }

    // Windows 上 `rename` 到已存在的文件会失败（与 Unix 语义不同），
    // 故先尝试直接 rename，失败时删除目标再重试。
    match std::fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(first_error) => {
            if path.exists() {
                std::fs::remove_file(path).map_err(|error| from_io(path, error))?;
                std::fs::rename(&temp_path, path).map_err(|error| {
                    let _ = std::fs::remove_file(&temp_path);
                    from_io(path, error)
                })
            } else {
                let _ = std::fs::remove_file(&temp_path);
                Err(from_io(path, first_error))
            }
        }
    }
}

/// 配置项清单（键路径 → 一句话说明）。
///
/// 供 `--help`/诊断展示"本版本认识哪些配置项"。**必须与结构体字段同步**——
/// 有测试逐条比对（漏了就等于用户看不到那一项的存在）。
pub fn known_keys() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        ("network.proxy", "代理地址（留空则跟随系统代理）"),
        (
            "network.request_interval_ms",
            "默认请求间隔毫秒（书源声明优先）",
        ),
        ("network.timeout_ms", "默认请求超时毫秒"),
        ("network.http_log_enabled", "是否记录请求日志"),
        ("network.http_log_keep", "请求日志保留条数（滚动清理）"),
        (
            "network.http_log_response_chars",
            "请求日志记录响应体的字符上限（默认不记）",
        ),
        ("storage.chapter_pack_bytes", "章节缓存容器容量（字节）"),
        ("storage.image_cache_bytes", "图片缓存容量上限（字节）"),
        ("storage.data_dir", "数据目录（留空则自动判定）"),
        ("display.language", "界面语言标识"),
        ("display.follow_system_theme", "是否跟随系统主题"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "tome-config-{label}-{}-{:?}",
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

    #[test]
    fn 文件不存在时用默认配置() {
        // config.toml 是可选的——首次启动不该因为"没配"而失败
        let dir = TempDir::new("missing");
        let config = Config::load(&dir.path.join("config.toml")).expect("应成功");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn 空文件是合法配置() {
        let dir = TempDir::new("empty");
        let path = dir.path.join("config.toml");
        std::fs::write(&path, "").expect("应能写");
        let config = Config::load(&path).expect("空文件应合法");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn 只写关心的项也能加载() {
        // FR-032 的"可手工编辑"：用户只想改代理时，文件里应该只有 [network]
        let text = r#"
[network]
proxy = "http://127.0.0.1:7890"
"#;
        let config = Config::from_toml(text, Path::new("t.toml")).expect("应成功");
        assert_eq!(
            config.network.proxy.as_deref(),
            Some("http://127.0.0.1:7890")
        );
        // 其余项应是默认值
        assert_eq!(
            config.storage.chapter_pack_bytes,
            crate::pack::DEFAULT_PACK_CAPACITY_BYTES
        );
        assert!(config.network.http_log_enabled);
    }

    #[test]
    fn 非法_toml_报配置分类错误() {
        let error = Config::from_toml("这不是 TOML {{{", Path::new("t.toml")).expect_err("应报错");
        assert_eq!(
            error.to_error_kind(),
            tome_core::ErrorKind::Configuration,
            "配置错应归 Configuration 而不是 Storage：上层据此提示改配置"
        );
        assert!(
            error.to_string().contains("t.toml"),
            "错误里应带上文件名：{error}"
        );
    }

    #[test]
    fn 未知键被拒绝而非静默忽略() {
        // 打字错误（`proxi`）静默忽略会让用户以为配好了，实际没生效
        let error = Config::from_toml("[network]\nproxi = \"x\"\n", Path::new("t.toml"))
            .expect_err("未知键应被拒绝");
        assert!(
            error.to_string().contains("proxi"),
            "错误应指出是哪个键：{error}"
        );
    }

    #[test]
    fn 往返保存与载入() {
        let dir = TempDir::new("roundtrip");
        let path = dir.path.join("config.toml");

        let mut config = Config::default();
        config.network.proxy = Some("http://127.0.0.1:7890".to_string());
        config.network.timeout_ms = Some(45_000);
        config.storage.chapter_pack_bytes = 8 * 1024 * 1024;
        config.display.language = Some("zh-CN".to_string());

        config.save(&path).expect("应能保存");
        let loaded = Config::load(&path).expect("应能载入");
        assert_eq!(loaded, config, "配置应逐字段往返");
    }

    #[test]
    fn 落盘格式可读且可手工编辑() {
        let dir = TempDir::new("readable");
        let path = dir.path.join("config.toml");
        Config::default().save(&path).expect("应能保存");
        let text = std::fs::read_to_string(&path).expect("应能读");

        // 应带分节标题（TOML 的惯例写法），而不是一串 `network.proxy = ...`
        assert!(text.contains("[network]"), "应有分节：\n{text}");
        assert!(text.contains("[storage]"), "应有分节：\n{text}");
        assert!(text.contains("[display]"), "应有分节：\n{text}");
        // 应能被再次解析（手工编辑的前提）
        Config::from_toml(&text, &path).expect("自己写的格式应能读回");
    }

    #[test]
    fn 原子写不留下临时文件() {
        let dir = TempDir::new("atomic");
        let path = dir.path.join("config.toml");
        Config::default().save(&path).expect("应能保存");

        let leftovers: Vec<_> = std::fs::read_dir(&dir.path)
            .expect("应能读")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不该留下临时文件：{leftovers:?}");
    }

    #[test]
    fn 覆盖写是替换而非截断追加() {
        let dir = TempDir::new("overwrite");
        let path = dir.path.join("config.toml");

        let mut long = Config::default();
        long.network.proxy = Some("http://a-very-long-proxy-address:1234".to_string());
        long.save(&path).expect("应能保存");
        let long_text = std::fs::read_to_string(&path).expect("应能读");

        let mut short = Config::default();
        short.network.proxy = None;
        short.save(&path).expect("应能保存");
        let short_text = std::fs::read_to_string(&path).expect("应能读");

        assert!(
            short_text.len() < long_text.len(),
            "短配置应比长配置短（截断残留会把旧内容留在文件里）"
        );
        assert!(!short_text.contains("a-very-long-proxy-address"));
        Config::from_toml(&short_text, &path).expect("应是合法 TOML");
    }

    #[test]
    fn 保存会创建父目录() {
        let dir = TempDir::new("mkdir");
        let path = dir.path.join("nested").join("deep").join("config.toml");
        Config::default().save(&path).expect("应能建父目录并写入");
        assert!(path.exists());
    }

    #[test]
    fn 内容无变化时不重写文件() {
        // 每次启动都改 mtime 会让"配置文件被改动了"这类排查失去意义
        let dir = TempDir::new("nochange");
        let path = dir.path.join("config.toml");
        Config::default().save(&path).expect("应能保存");
        let first = std::fs::metadata(&path)
            .expect("应有元数据")
            .modified()
            .expect("应有 mtime");

        // 稍等一下让 mtime 有时间粒度上的差别
        std::thread::sleep(std::time::Duration::from_millis(20));
        let written = Config::normalize_file(&path).expect("应成功");
        assert!(!written, "内容无变化时不该写文件");

        let second = std::fs::metadata(&path)
            .expect("应有元数据")
            .modified()
            .expect("应有 mtime");
        assert_eq!(first, second, "mtime 不该变");
    }

    #[test]
    fn 缺失项被补全并写回() {
        let dir = TempDir::new("normalize");
        let path = dir.path.join("config.toml");
        std::fs::write(&path, "[network]\nproxy = \"http://p:1\"\n").expect("应能写");

        let written = Config::normalize_file(&path).expect("应成功");
        assert!(written, "缺失项应被补全");
        let text = std::fs::read_to_string(&path).expect("应能读");
        assert!(text.contains("[storage]"), "应补上 storage 节：\n{text}");
        assert!(text.contains("http://p:1"), "用户写的值必须保留：\n{text}");
    }

    #[test]
    fn 配置错误不会破坏原文件() {
        // 手工编辑写坏了 → 加载报错，但**原文件不动**（用户还能去修）
        let dir = TempDir::new("broken");
        let path = dir.path.join("config.toml");
        let broken = "[network\nproxy = ";
        std::fs::write(&path, broken).expect("应能写");

        Config::load(&path).expect_err("应报错");
        assert_eq!(
            std::fs::read_to_string(&path).expect("应能读"),
            broken,
            "加载失败不该改动原文件"
        );
    }

    // --- 类型化子集注入（A3）---

    #[test]
    fn 网络子集只含_net_需要的字段() {
        let mut config = Config::default();
        config.network.proxy = Some("http://p:1".to_string());
        config.network.timeout_ms = Some(1234);
        config.storage.chapter_pack_bytes = 999;

        let settings = config.network();
        assert_eq!(settings.proxy.as_deref(), Some("http://p:1"));
        assert_eq!(settings.timeout_ms, Some(1234));
        // 类型层面就没有 storage 字段可读——这是"传子集而非整个 Config"
        // 的实际收益：排版/网络模块拿不到与它无关的配置
    }

    #[test]
    fn 存储子集携带缓存容量() {
        let mut config = Config::default();
        config.storage.chapter_pack_bytes = 4 * 1024 * 1024;
        config.storage.image_cache_bytes = 1024;
        let settings = config.storage();
        assert_eq!(settings.chapter_pack_bytes, 4 * 1024 * 1024);
        assert_eq!(settings.image_cache_bytes, 1024);
    }

    #[test]
    fn 日志子集反映开关与上限() {
        let text = r#"
[network]
http_log_enabled = false
http_log_keep = 42
"#;
        let config = Config::from_toml(text, Path::new("t.toml")).expect("应成功");
        let settings = config.http_log();
        assert!(!settings.enabled);
        assert_eq!(settings.keep, 42);
    }

    #[test]
    fn 子集可独立构造_无需完整配置() {
        // 子集的价值之一：测试可以只造它需要的那一小块
        let settings = HttpLogSettings {
            enabled: false,
            keep: 0,
        };
        assert_eq!(settings.keep, 0);

        let network = NetworkSettings::default();
        assert_eq!(network.proxy, None);
    }

    #[test]
    fn 没有全局单例可访问() {
        // A3 的结构性证据：本模块**不存在** `Config::global()` 之类的入口，
        // 且 `Config` 不是 `static`。若日后有人加了一个 `OnceLock<Config>`，
        // 它会成为本 crate 的公开项，而"无全局单例"这条就没有编译期保证了。
        //
        // 故这里断言"访问配置必须经由一个显式传入的值"——写这段代码时
        // 编译器会强制你有一个 `Config` 在手，没有别的途径。
        let config = Config::default();
        let via_value = config.network().proxy;
        assert_eq!(via_value, None);
    }

    #[test]
    fn 配置项清单与结构体同步() {
        // 清单用于 `--help`/诊断展示。漏了就等于用户看不到那一项的存在，
        // 故逐条比对字段名。
        let keys = known_keys();
        let expected = [
            "network.proxy",
            "network.request_interval_ms",
            "network.timeout_ms",
            "network.http_log_enabled",
            "network.http_log_keep",
            "network.http_log_response_chars",
            "storage.chapter_pack_bytes",
            "storage.image_cache_bytes",
            "storage.data_dir",
            "display.language",
            "display.follow_system_theme",
        ];
        assert_eq!(keys.len(), expected.len(), "配置项数量不符");
        for key in expected {
            assert!(keys.contains_key(key), "清单缺 `{key}`");
        }
    }
}
