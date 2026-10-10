//! \[DONE] 数据目录解析与启动期检测（tasks.md T055 / FR-042、FR-043）
//!
//! # 两条运行模式
//!
//! | 模式 | 触发条件 | 数据位置 |
//! |---|---|---|
//! | **可移植** | 程序同级存在 `data/` 目录 | `<程序目录>/data/` |
//! | **安装** | 其余情况 | 系统数据目录（`%APPDATA%` / `~/Library/Application Support` / `$XDG_DATA_HOME`） |
//!
//! FR-042 要求"数据目录可置于程序同级并随程序整体移动"。这条要求的落地方式
//! 是**目录存在即生效**——用户把 `data/` 放在程序旁边（U 盘、便携包），
//! 整个目录搬走再搬回来，数据跟着走。不需要配置文件里的开关，也不需要
//! 命令行参数：目录本身的存在就是开关，这是"随程序搬移"最直接的语义。
//!
//! ## 为什么"安装模式"不覆盖已有数据
//!
//! 若 `data/` 里已经有一个库（用户此前用过可移植模式），而本次因为某种原因
//! 判定为安装模式，程序会去系统目录建一个**空库**——用户看到的是"书架空了"。
//! 故 [`DataDir::resolve`] 在把 `data/` 判为不可用时，只回落到系统目录而
//! **不**迁移或删除 `data/` 里的任何东西；同时把判定理由放进
//! `DataDir::resolution_note` 供上层展示，让"我的数据去哪了"有答案。
//!
//! # 启动期检测（FR-043）
//!
//! 「不可写或磁盘空间不足时明确报告并给出指引，**而非静默失败**」。
//! 故 [`DataDir::resolve`] 返回 [`Resolved::Unusable`] 时带**两条**信息：
//!
//! - `reason`：为什么不可用（写测试失败的具体错误、剩余空间的具体数值）
//! - `guidance`：用户该做什么（改用 `--data-dir`、清理空间、检查只读属性）
//!
//! 只报"数据目录不可用"是没有指引的报错；只报"磁盘剩余 50MB"是没有结论的
//! 数据。两条一起给，用户才知道下一步动作是什么。

use std::path::{Path, PathBuf};

use crate::error::{from_io, StoreError};

/// 可移植模式的数据目录名（程序同级的子目录）。
pub const PORTABLE_DIR_NAME: &str = "data";

/// 安装模式下的应用数据目录名。
pub const APP_DIR_NAME: &str = "tome";

/// 启动期磁盘空间检查的**建议**下限（字节）。
///
/// ## 为什么是"建议"而不是硬门槛
///
/// 硬门槛会在"磁盘快满但还能用"时拒绝启动——那剥夺了用户继续读书的能力，
/// 而读书本身几乎不占空间（缓存可以随时清）。故空间不足**不**阻止启动，
/// 只在剩余量低于本值时给出警告（`Layout::space_warning`）。
///
/// 取 100 MiB：足以容纳一本书的全部章节缓存（5 万章 × 6 KB ≈ 300 MB 是上限，
/// 但按需缓存的实际占用远低于此），而低于它的磁盘本身就该引起用户注意。
///
/// \[WORKAROUND] 硬编码常量：改为随 `config.toml` 的存储子集注入（T063 的组合根
/// 已有注入位），当前阶段保持无配置依赖。
pub const RECOMMENDED_FREE_BYTES: u64 = 100 * 1024 * 1024;

/// 数据目录的解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// 可用。
    Usable {
        /// 数据目录的绝对路径
        path: PathBuf,
        /// 采用哪种模式
        mode: Layout,
        /// 解析过程中的说明（如"程序同级 data/ 不可写，已回落到系统目录"）
        note: Option<String>,
    },
    /// 不可用。上层**必须**把 `reason` 与 `guidance` 报告给用户（FR-043）。
    Unusable {
        /// 尝试使用的路径
        path: PathBuf,
        /// 不可用的原因（含底层错误文本）
        reason: String,
        /// 用户的下一步动作
        guidance: String,
    },
}

/// 数据目录的布局模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// 可移植：数据在程序同级的 `data/`
    Portable,
    /// 安装：数据在系统数据目录
    Installed,
}

impl Layout {
    /// 稳定的标识字符串（供 CLI 输出与日志；引擎内不产自然语言，FR-044）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Portable => "portable",
            Self::Installed => "installed",
        }
    }
}

/// 已解析的数据目录：路径 + 其下的标准子路径。
///
/// # 为什么把子路径集中在这里
///
/// Pack 缓存、图片缓存、数据库、配置各有自己的相对位置（`data-model.md`
/// 规定了 `cache/packs/`、`cache/images/` 等）。若各处自行 `join("cache")`，
/// 布局变更就会漏改其中几处——而漏改的表现是"新的写到新位置、旧的还在老位置"，
/// 用户看到的是"缓存时有时无"。集中一处让布局只有**一个**定义点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    root: PathBuf,
    mode: Layout,
}

impl DataDir {
    /// 解析数据目录：显式路径 > 可移植优先 > 系统目录。
    ///
    /// `explicit` 来自 `--data-dir`。显式指定时**只**看它——用户明确说了
    /// 用哪就要用哪，此时可移植目录的存在不该改变结果（否则"我明明指了路径
    /// 它却用了别处"会成为一类难查的怪事）。
    ///
    /// `program_dir` 是程序可执行文件所在目录（组合根从 `std::env::current_exe`
    /// 取得后传入，本 crate 不自取——见 [`DataDir::resolve_for_current_exe`] 的说明）。
    pub fn resolve(
        explicit: Option<&Path>,
        program_dir: Option<&Path>,
        system_data_dir: Option<&Path>,
    ) -> Resolved {
        if let Some(path) = explicit {
            return match probe(path) {
                Ok(()) => Resolved::Usable {
                    path: path.to_path_buf(),
                    mode: Layout::Installed,
                    note: Some("路径由 --data-dir 显式指定".to_string()),
                },
                Err(error) => Resolved::Unusable {
                    path: path.to_path_buf(),
                    reason: format!("--data-dir 指定的数据目录不可用：{error}"),
                    guidance: "--data-dir 指向一个可写目录，或去掉该参数改用默认位置".to_string(),
                },
            };
        }

        // 可移植优先：程序同级有 `data/` 就用它（FR-042）
        if let Some(program_dir) = program_dir {
            let portable = program_dir.join(PORTABLE_DIR_NAME);
            if portable.is_dir() {
                return match probe(&portable) {
                    Ok(()) => Resolved::Usable {
                        path: portable,
                        mode: Layout::Portable,
                        note: None,
                    },
                    Err(error) => {
                        // 可移植目录存在但不可用：**不静默回落到系统目录**的
                        // 前提是把理由带出去。用户把数据放在 U 盘上、U 盘拔了，
                        // 直接回落会让程序看起来"书架空了"。
                        let mut fallback = resolve_system(
                            system_data_dir,
                            Some(&format!(
                                "程序同级的数据目录 {} 不可用（{error}），已改用系统数据目录",
                                portable.display()
                            )),
                        );
                        if let Resolved::Unusable {
                            path,
                            reason,
                            guidance,
                        } = &mut fallback
                        {
                            // 系统目录也不可用时，把两条理由都给出——
                            // 只说系统目录的消息会让用户去查错的地方
                            *reason = format!(
                                "程序同级的数据目录 {} 不可用（{error}）；系统数据目录 {reason}",
                                portable.display()
                            );
                            *guidance = "--data-dir 指向一个可写目录".to_string();
                            *path = portable;
                        }
                        fallback
                    }
                };
            }
        }

        resolve_system(system_data_dir, None)
    }

    /// 便捷入口：从 `std::env::current_exe` 取程序目录后调用 [`Self::resolve`]。
    ///
    /// # 为什么取程序目录的动作放在这里而不是 `resolve`
    ///
    /// `resolve` 保持**纯函数**（输入决定输出，不读进程全局状态），故可被
    /// 测试穷举各种盘符与权限组合。本函数是它的"接上真实环境"的薄壳，
    /// 也是唯一依赖 `current_exe` 的地方——取不到时可执行文件路径（某些
    /// 受限环境）不是错误，退化为安装模式即可。
    pub fn resolve_for_current_exe(explicit: Option<&Path>) -> Resolved {
        let program_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let system_data_dir = system_data_dir();
        Self::resolve(explicit, program_dir.as_deref(), system_data_dir.as_deref())
    }

    /// 从一次成功的解析结果构造。
    ///
    /// 只接受 [`Resolved::Usable`]——不可用的结果无法构造出 [`DataDir`]，
    /// 故"用着一个不可用的目录"在类型层面就不成立。
    pub fn from_resolved(resolved: &Resolved) -> Option<Self> {
        match resolved {
            Resolved::Usable { path, mode, .. } => Some(Self {
                root: path.clone(),
                mode: *mode,
            }),
            Resolved::Unusable { .. } => None,
        }
    }

    /// 数据目录根。
    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    /// 布局模式。
    pub fn mode(&self) -> Layout {
        self.mode
    }

    /// 数据库文件路径。
    pub fn database_path(&self) -> PathBuf {
        self.root.join("tome.db")
    }

    /// 配置文件路径（FR-032：可读、可手工编辑、可随程序搬移）。
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// 章节缓存容器目录（`data-model.md`：`cache/packs/<book_hash>/<pack_id>.pk`）。
    pub fn packs_dir(&self) -> PathBuf {
        self.root.join("cache").join("packs")
    }

    /// 图片缓存目录（`cache/images/<xx>/<hash>`）。
    pub fn images_dir(&self) -> PathBuf {
        self.root.join("cache").join("images")
    }

    /// 下载区（pinned 缓存，不参与淘汰）。
    pub fn download_packs_dir(&self) -> PathBuf {
        self.root.join("download").join("packs")
    }

    /// 建立全部子目录（幂等）。
    pub fn ensure_layout(&self) -> Result<(), StoreError> {
        for dir in [
            self.root.clone(),
            self.packs_dir(),
            self.images_dir(),
            self.download_packs_dir(),
        ] {
            std::fs::create_dir_all(&dir).map_err(|error| from_io(&dir, error))?;
        }
        Ok(())
    }

    /// 当前可用空间（字节）。取不到时返回 `None`——**不猜一个数字**。
    pub fn free_bytes(&self) -> Option<u64> {
        free_bytes(&self.root)
    }

    /// 空间不足时的警告文案（足够时返回 `None`）。
    ///
    /// 返回的是**结构化数据**而不是直接打印：引擎不产面向用户的自然语言
    /// （FR-044），上层据此决定怎么展示。
    pub fn space_warning(&self) -> Option<SpaceWarning> {
        let free = self.free_bytes()?;
        if free >= RECOMMENDED_FREE_BYTES {
            return None;
        }
        Some(SpaceWarning {
            path: self.root.clone(),
            free_bytes: free,
            recommended_bytes: RECOMMENDED_FREE_BYTES,
        })
    }

    /// 启动期完整检测：建目录 + 写测试 + 空间检查。
    ///
    /// 这是组合根该调的那一个（`--data-dir` 解析完 → 本函数 → 报告问题）。
    pub fn prepare(&self) -> Result<Option<SpaceWarning>, StoreError> {
        self.ensure_layout()?;
        probe(&self.root)?;
        Ok(self.space_warning())
    }
}

/// 磁盘空间不足的警告（FR-043）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceWarning {
    /// 涉及的路径
    pub path: PathBuf,
    /// 当前可用字节数
    pub free_bytes: u64,
    /// 建议的下限
    pub recommended_bytes: u64,
}

impl SpaceWarning {
    /// 面向人的说明（上层负责把它翻译成具体语言）。
    ///
    /// 含**结论**与**指引**两部分：只给数字用户不知道要不要管，只给结论
    /// 用户不知道严重程度。
    pub fn describe(&self) -> String {
        format!(
            "磁盘空间偏低：{} 所在卷剩余 {}（建议下限 {}）。缓存可以清理（清缓存不影响书架与进度），\
             或把数据目录换到空间更充裕的位置（--data-dir）。",
            self.path.display(),
            human_bytes(self.free_bytes),
            human_bytes(self.recommended_bytes)
        )
    }
}

/// 把字节数写成人类可读形式（展示用）。
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 系统数据目录（平台差异在此收口）。
///
/// 不引 `dirs`/`directories` crate：三平台的规则各是三行环境变量读取
/// （见下），而多加一个依赖要付出的是传递依赖面（`dirs-sys` → `windows-sys`）
/// 与一份我们并不使用的 API 面。
fn system_data_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        // Windows 的规范位置是 %APPDATA%（Roaming），与 macOS/Linux 的
        // "每用户应用数据"语义对应。
        std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join(APP_DIR_NAME))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_DIR_NAME)
        })
    } else {
        // XDG Base Directory：未设 XDG_DATA_HOME 时回落到 ~/.local/share
        if let Some(base) = std::env::var_os("XDG_DATA_HOME") {
            return Some(PathBuf::from(base).join(APP_DIR_NAME));
        }
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join(APP_DIR_NAME)
        })
    }
}

/// 系统目录路径下的解析（含"环境变量缺失"的处理）。
fn resolve_system(system_data_dir: Option<&Path>, note: Option<&str>) -> Resolved {
    let Some(path) = system_data_dir else {
        return Resolved::Unusable {
            path: PathBuf::from("<系统数据目录>"),
            reason: "取不到系统数据目录（平台环境变量缺失）".to_string(),
            guidance: "--data-dir 指定一个可写目录".to_string(),
        };
    };
    match probe(path) {
        Ok(()) => Resolved::Usable {
            path: path.to_path_buf(),
            mode: Layout::Installed,
            note: note.map(str::to_string),
        },
        Err(error) => Resolved::Unusable {
            path: path.to_path_buf(),
            reason: format!("系统数据目录不可用：{error}"),
            guidance: "检查该目录的写入权限，或用 --data-dir 指向别处".to_string(),
        },
    }
}

/// 可写性检测：**真的写一个文件**再删掉。
///
/// # 为什么不只看权限位
///
/// 权限位（`readonly` 属性、`mode & 0o200`）与"能不能写"在三类场景下不一致：
///
/// 1. **只读挂载**的卷：文件属性看着正常，写入必失败
/// 2. **Windows 上被其他进程独占**的目录
/// 3. **父目录存在但子项名被占**（同名的文件挡在目录位置）
///
/// 唯一可靠的判据是真的写一次。这是 `data-model.md`「启动期检测」的意图——
/// 检测的目的是**提前发现**，而不是预测。
///
/// # 探测文件名的唯一性（并发下的正确性）
///
/// 文件名带**进程号 + 线程号**，而不是只带进程号。
///
/// ## 为什么只带进程号不够（实测踩到的坑）
///
/// 首版只用 `std::process::id()`。那在"一个进程里多个线程各自解析数据目录"
/// 时会出问题：两个线程探测**同一个目录**时用的是**同一个文件名**，
/// 于是 A 的"先删一次"会把 B 刚写下的探测文件删掉，B 随后的
/// `remove_file` 报 not-found → `probe` 失败 → 数据目录被判为不可用。
///
/// 后果不是"测试挂了"这么轻：它会**悄悄退化到默认数据目录**，于是本该
/// 隔离在临时目录里的写入落到了共享位置——表现为互不相关的测试互相污染
/// （实测症状：`source import` 的同 id 更新计数偶发为 0、清缓存偶发
/// "database is locked"）。
///
/// 加上线程号后，每个调用者有自己的文件，互不干扰。
///
/// ## 为什么不干脆用一个随机数
///
/// `thread::current().id()` 是**廉价且必定可用**的（不需要随机源，也不
/// 依赖时钟）。随机数要引 rand 或手搓，而这里只需要"同一时刻不同调用者
/// 不撞名"——线程号恰好就是这个语义，且与"并发"这件事一一对应。
fn probe(dir: &Path) -> Result<(), StoreError> {
    std::fs::create_dir_all(dir).map_err(|error| from_io(dir, error))?;

    let marker = dir.join(format!(
        ".tome-write-probe-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    // 先删一次：上一次异常退出可能留下探测文件（用户看到会疑惑）。
    // 容忍 not-found——那说明本来就没有，不是错误。
    match std::fs::remove_file(&marker) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(from_io(&marker, error)),
    }

    std::fs::write(&marker, b"probe").map_err(|error| from_io(&marker, error))?;
    std::fs::remove_file(&marker).map_err(|error| from_io(&marker, error))?;

    // 目录项清点：目录存在且可写，但**读**不了（如只给了写权限）时，
    // 后续的目录扫描会失败。这里一并挡住。
    std::fs::read_dir(dir).map_err(|error| from_io(dir, error))?;
    Ok(())
}

/// 取路径所在卷的可用空间（字节）。
///
/// # 跨平台实现
///
/// - **Unix**：`libc::statvfs` 的 `f_bavail * f_frsize`。`f_bavail` 是
///   **非特权用户**可用块数——用 `f_bfree`（总空闲块）会高估，因为它把
///   留给 root 的保留块也算进去，而本程序不是 root。
/// - **Windows**：`GetDiskFreeSpaceExW` 的 `lpFreeBytesAvailableToCaller`
///   （同样是非特权用户的视角，与 Unix 侧口径一致）。
///
/// # 为什么直接调系统 API 而不引 crate
///
/// 不需要 `sysinfo` 那样的全量系统信息（CPU/进程/内存），只要一个数值。
/// 而为一个数值引入跨平台抽象层，代价是它自身的依赖面（`sysinfo` 0.31 会
/// 拉入 `windows` 0.57 等一整套 Win32 绑定）。两个平台各一个函数更小、
/// 也更不容易因上层 crate 的 API 变动而失效。
///
/// 失败时返回 `None`——不猜。
#[cfg(unix)]
// SAFETY 豁免：本函数调用 POSIX 的 `statvfs`，理由同 Windows 版本。
#[expect(unsafe_code, reason = "POSIX 卷容量查询无安全等价物，FR-043 需要它")]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    let text = path.to_str()?;
    let c_path = CString::new(text).ok()?;
    // SAFETY: `statvfs` 需要一块可写的 `statvfs` 结构体与一个有效的 C 字符串
    // 路径。两者都在本函数内构造并保证生命周期覆盖调用；返回值已检查。
    unsafe {
        let mut stats: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stats) != 0 {
            return None;
        }
        // `f_frsize` 为 0 的平台（极少）回落到 `f_bsize`
        let block_size = if stats.f_frsize > 0 {
            stats.f_frsize as u64
        } else {
            stats.f_bsize as u64
        };
        Some(block_size.saturating_mul(stats.f_bavail as u64))
    }
}

/// Windows 实现（见 [`free_bytes`] 的 Unix 版本说明）。
#[cfg(windows)]
// SAFETY 豁免：本函数调用 Win32 的 `GetDiskFreeSpaceExW`，这是 FR-043 要求的
// 磁盘空间检测的唯一实现手段（std 不提供卷容量查询）。调用点的前置条件在
// 下方 SAFETY 注释中逐条说明，且无任何裸指针逃出本函数。
#[expect(unsafe_code, reason = "Win32 卷容量查询无安全等价物，FR-043 需要它")]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;

    // 手工声明 Win32 绑定，避免为此引入 windows-sys 的完整依赖。
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available_to_caller: *mut u64,
            total_number_of_bytes: *mut u64,
            total_number_of_free_bytes: *mut u64,
        ) -> i32;
    }

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut available: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    // SAFETY: 路径已转成 NUL 结尾的宽字符串且在本函数内存活；三个出参都是
    // 本函数栈上的有效 `u64`。返回值为 0 表示失败，此时不读它们。
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, &mut total, &mut total_free) };
    if ok == 0 {
        return None;
    }
    let _ = (total, total_free);
    Some(available)
}

/// 其余平台：明确不支持（不猜数字）。
#[cfg(not(any(unix, windows)))]
pub fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tome-datadir-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建临时目录");
        dir
    }

    #[test]
    fn 程序同级有_data_目录时用可移植模式() {
        let base = temp_dir("portable");
        let program = base.join("app");
        std::fs::create_dir_all(program.join(PORTABLE_DIR_NAME)).expect("应能建");
        let system = base.join("system-data");

        let resolved = DataDir::resolve(None, Some(&program), Some(&system));
        match resolved {
            Resolved::Usable { path, mode, .. } => {
                assert_eq!(mode, Layout::Portable, "应选可移植模式");
                assert_eq!(path, program.join(PORTABLE_DIR_NAME));
            }
            other => panic!("应可用：{other:?}"),
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn 没有_data_目录时用系统目录() {
        let base = temp_dir("installed");
        let program = base.join("app");
        std::fs::create_dir_all(&program).expect("应能建");
        let system = base.join("system-data");

        let resolved = DataDir::resolve(None, Some(&program), Some(&system));
        match resolved {
            Resolved::Usable { path, mode, .. } => {
                assert_eq!(mode, Layout::Installed);
                assert_eq!(path, system);
            }
            other => panic!("应可用：{other:?}"),
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn 整体搬移后数据仍可用() {
        // FR-042 的验收形态：把程序目录连同 data/ 一起搬到新位置，
        // 解析结果应当指向新位置下的 data/，且原数据文件还在。
        let base = temp_dir("move");
        let original = base.join("usb-old");
        std::fs::create_dir_all(original.join(PORTABLE_DIR_NAME)).expect("应能建");
        let payload = original.join(PORTABLE_DIR_NAME).join("tome.db");
        std::fs::write(&payload, b"data").expect("应能写");

        // 整体搬移
        let moved = base.join("usb-new");
        std::fs::rename(&original, &moved).expect("应能搬移");

        let resolved = DataDir::resolve(None, Some(&moved), None);
        let data_dir = DataDir::from_resolved(&resolved).expect("搬移后应仍可用");
        assert_eq!(data_dir.mode(), Layout::Portable);
        assert_eq!(data_dir.database_path(), moved.join("data").join("tome.db"));
        assert!(data_dir.database_path().exists(), "搬移后数据文件应还在");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn 显式指定时忽略可移植目录() {
        let base = temp_dir("explicit");
        let program = base.join("app");
        std::fs::create_dir_all(program.join(PORTABLE_DIR_NAME)).expect("应能建");
        let explicit = base.join("custom");

        let resolved = DataDir::resolve(Some(&explicit), Some(&program), None);
        match resolved {
            Resolved::Usable { path, .. } => assert_eq!(path, explicit),
            other => panic!("应可用：{other:?}"),
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn 不可写时给出原因与指引而非静默失败() {
        // FR-043：不可写必须明确报告 + 给指引。
        // 用一个"父路径是个文件"的形态制造必失败（跨平台稳定，不依赖权限位）。
        let base = temp_dir("unwritable");
        let blocking_file = base.join("blocker");
        std::fs::write(&blocking_file, b"x").expect("应能写");
        let target = blocking_file.join("data"); // 文件之下不能建目录

        let resolved = DataDir::resolve(Some(&target), None, None);
        match resolved {
            Resolved::Unusable {
                reason, guidance, ..
            } => {
                assert!(!reason.is_empty(), "必须说明原因");
                assert!(!guidance.is_empty(), "必须给出指引");
                assert!(
                    guidance.contains("--data-dir"),
                    "指引应指出可行的下一步：{guidance}"
                );
            }
            other => panic!("应报不可用：{other:?}"),
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn 可移植目录不可用时回落并说明理由() {
        let base = temp_dir("fallback");
        let program = base.join("app");
        let portable = program.join(PORTABLE_DIR_NAME);
        std::fs::create_dir_all(&program).expect("应能建");
        // `data` 是个**文件**而不是目录：`is_dir()` 为假 → 走系统目录，
        // 且不产生告警（用户没在用可移植模式）
        std::fs::write(&portable, b"not a dir").expect("应能写");
        let system = base.join("system-data");

        let resolved = DataDir::resolve(None, Some(&program), Some(&system));
        match resolved {
            Resolved::Usable { mode, .. } => assert_eq!(mode, Layout::Installed),
            other => panic!("应可用：{other:?}"),
        }

        // 而 `data` 是**目录但不可写**时：回落 + 带出理由
        let blocked_system = base.join("blocked");
        std::fs::write(&blocked_system, b"file").expect("应能写");
        let resolved = DataDir::resolve(None, Some(&program), Some(&blocked_system.join("x")));
        match resolved {
            Resolved::Unusable { reason, .. } => {
                assert!(
                    reason.contains("不可用"),
                    "应说明系统目录也不可用：{reason}"
                );
            }
            Resolved::Usable { note, .. } => {
                assert!(note.is_some(), "回落到系统目录时必须带出理由");
            }
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn 系统目录取不到时是明确错误() {
        let resolved = DataDir::resolve(None, None, None);
        match resolved {
            Resolved::Unusable { guidance, .. } => {
                assert!(guidance.contains("--data-dir"), "{guidance}");
            }
            other => panic!("应报不可用：{other:?}"),
        }
    }

    #[test]
    fn prepare_建立全部子目录() {
        let base = temp_dir("layout");
        let resolved = DataDir::resolve(Some(&base), None, None);
        let data_dir = DataDir::from_resolved(&resolved).expect("应可用");
        data_dir.prepare().expect("应能准备");

        assert!(data_dir.packs_dir().is_dir(), "章节缓存目录应建立");
        assert!(data_dir.images_dir().is_dir(), "图片缓存目录应建立");
        assert!(data_dir.download_packs_dir().is_dir(), "下载区应建立");
        assert!(data_dir
            .database_path()
            .parent()
            .expect("应有父目录")
            .is_dir());
        // 探测文件不该留下
        let leftovers: Vec<_> = std::fs::read_dir(data_dir.root())
            .expect("应能读")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tome-write-probe")
            })
            .collect();
        assert!(leftovers.is_empty(), "探测文件应被清理：{leftovers:?}");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn 空间查询有确定结果() {
        let base = temp_dir("space");
        let resolved = DataDir::resolve(Some(&base), None, None);
        let data_dir = DataDir::from_resolved(&resolved).expect("应可用");
        let free = data_dir.free_bytes();
        assert!(free.is_some(), "临时目录所在卷应能查到可用空间");
        assert!(free.expect("已断言非空") > 0, "可用空间应大于 0");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn 人类可读字节数() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(100 * 1024 * 1024), "100.0 MiB");
    }

    #[test]
    fn 路径不存在的卷查不到空间返回_none() {
        // 明确的"查不到"而不是猜一个数——猜出来的数字会让空间告警要么误报要么漏报
        let bogus = if cfg!(windows) {
            Path::new("Z:\\definitely-not-a-volume\\x")
        } else {
            Path::new("/definitely/not/a/mount/point")
        };
        assert_eq!(free_bytes(bogus), None);
    }
}
