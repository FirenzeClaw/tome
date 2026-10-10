//! [DONE] `StoreHost` 的真实实现（tasks.md T054 / `seams.md` §4）
//!
//! JS 沙箱的四个宿主状态类 API（`java.cachePut`/`cacheGet`/`readFile`/`writeFile`）
//! 与 Cookie 读写（`java.getCookie`/`setCookie`）经本模块落到真实存储。
//!
//! ## 为什么 `cache_get`/`cache_put` 复用一个类型（与内存替身不同）
//!
//! | 通道 | 真实落点 |
//! |---|---|
//! | `cache_get`/`cache_put` | `script_cache` 表（按 `source_id` 隔离） |
//! | `read_file`/`write_file` | 数据目录下的真实文件 |
//!
//! ## 为什么 Cookie 必须**按源隔离**
//!
//! `java.setCookie("token", ...)` 里的名字是书源自己起的，不同源完全可能
//! 用同名键存各自的登录态。若共用一个命名空间，A 源写的 token 会被 B 源
//! 读到——那是**跨源凭据泄漏**。故本实现把 Cookie 单独落 `cookie` 表并按
//! `source_id` 分区（与 `script_cache` 的分区理由相同）。
//!
//! ## 为什么文件路径要**双重**校验
//!
//! `tome-js` 的 `MemoryStoreHost` 用纯词法判定（`is_inside_data_dir`）。
//! 真实实现面对的是真实文件系统，多出两类威胁：
//!
//! 1. **符号链接**：`link -> /etc` 之后 `<data>/link/passwd` 在词法上完全合法
//! 2. **Windows 的 8.3 短名**与大小写不敏感
//!
//! 故这里先做词法判定（挡住绝大多数），再对**父目录**做 `canonicalize` 后的
//! 前缀包含判定（挡住符号链接）。父目录不存在时（首次写入）沿路径上溯到
//! 最近的已存在祖先再判——否则"第一次写就跳过检查"会成为绕过口。

use std::path::{Component, Path, PathBuf};

use tome_core::{ErrorKind, StoreHost};

use crate::db::sql;
use crate::error::{from_io, StoreError};

/// Cookie 表的 `domain` 列在本实现中的固定取值。
///
/// ## 为什么不用空串
///
/// `cookie` 表的主键是 `(source_id, name, domain)`。空串在 SQLite 里是合法
/// 的主键分量，但它让"没有 domain"与"domain 为空串"无法区分，也让后续真要
/// 落多 domain 时无法分辨哪些是占位。用显式标记 `script`（表示"由脚本
/// `setCookie` 写入、无 HTTP domain 语义"）留出区分空间。
pub const SCRIPT_COOKIE_DOMAIN: &str = "script";

/// JS 宿主通道里 Cookie 键的前缀。
///
/// **必须与 `tome_js::api::state` 的 `COOKIE_KEY_PREFIX` 同值**——它是
/// 跨 crate 的隐式协议，`tome-store` 尚无反查 `tome-js` 常量的途径
/// （依赖方向不允许：`tome-js` 不依赖 `tome-store`，反之亦然）。
///
/// 故本模块有 `cookie_键前缀与_js_侧一致` 测试，把 "tome://cookie/" 这个
/// 字面量钉住；改动任一侧都会让该测试红，从而迫使两处同步。
pub const COOKIE_KEY_PREFIX: &str = "tome://cookie/";

/// 真实存储支撑的 [`StoreHost`]。
///
/// 每个书源脚本一个实例（`source_id` 不同即实例不同），故脚本的缓存与 Cookie
/// 天然按源分区。
#[derive(Debug, Clone)]
pub struct SqliteStoreHost {
    db: crate::db::Db,
    /// 数据目录根（文件读写的边界）
    data_root: PathBuf,
    /// 当前书源 id（缓存与 Cookie 的分区键）
    source_id: String,
}

impl SqliteStoreHost {
    /// 为某个书源构造。
    ///
    /// `data_root` 会被 `canonicalize`：真实路径是后续前缀包含判定的基准。
    /// 取不到时（目录尚不存在）退化为绝对化的词法路径——此时前缀判定仍有效，
    /// 只是对"数据目录自身是符号链接"的场景判得保守一些（宁可拒绝）。
    pub fn new(db: crate::db::Db, data_root: &Path, source_id: impl Into<String>) -> Self {
        let data_root =
            std::fs::canonicalize(data_root).unwrap_or_else(|_| absolute_lexical(data_root));
        Self {
            db,
            data_root,
            source_id: source_id.into(),
        }
    }

    /// 数据目录根（诊断用）。
    pub fn data_root(&self) -> &Path {
        self.data_root.as_path()
    }

    /// 当前书源 id。
    pub fn source_id(&self) -> &str {
        self.source_id.as_str()
    }

    /// 把相对路径解析为数据目录内的绝对路径，越界时拒绝。
    ///
    /// # 判定分两级（见模块文档「为什么文件路径要双重校验」）
    ///
    /// 1. **词法级**：拒绝绝对路径与向上逃逸的 `..`
    /// 2. **文件系统级**：解析符号链接后确认仍在数据目录内
    fn resolve_inside(&self, relative_path: &str) -> Result<PathBuf, StoreError> {
        if !is_lexically_inside(relative_path) {
            return Err(StoreError::PathOutOfDataDir {
                path: relative_path.to_string(),
            });
        }

        let candidate = self.data_root.join(relative_path);

        // 第二级：沿路径上溯到最近的**已存在**祖先，解析它再看是否越界。
        //
        // 为什么是父目录而不是目标本身：目标在写入前通常不存在，
        // `canonicalize` 会直接失败——若因此跳过检查，第一次写入就成了绕过口。
        let mut probe: &Path = candidate.as_path();
        while !probe.exists() {
            match probe.parent() {
                Some(parent) if parent.starts_with(&self.data_root) || parent != probe => {
                    probe = parent;
                }
                _ => break,
            }
        }
        let resolved_ancestor =
            std::fs::canonicalize(probe).unwrap_or_else(|_| probe.to_path_buf());
        if !resolved_ancestor.starts_with(&self.data_root) {
            return Err(StoreError::PathOutOfDataDir {
                path: relative_path.to_string(),
            });
        }

        Ok(candidate)
    }
}

impl StoreHost for SqliteStoreHost {
    fn cache_get(&self, key: &str) -> Result<Option<Vec<u8>>, ErrorKind> {
        self.cache_get_inner(key)
            .map_err(|error| error.to_error_kind())
    }

    fn cache_put(&self, key: &str, value: &[u8]) -> Result<(), ErrorKind> {
        self.cache_put_inner(key, value)
            .map_err(|error| error.to_error_kind())
    }

    fn read_file(&self, relative_path: &str) -> Result<Vec<u8>, ErrorKind> {
        self.read_file_inner(relative_path)
            .map_err(|error| error.to_error_kind())
    }

    fn write_file(&self, relative_path: &str, contents: &[u8]) -> Result<(), ErrorKind> {
        self.write_file_inner(relative_path, contents)
            .map_err(|error| error.to_error_kind())
    }
}

impl SqliteStoreHost {
    /// `cache_get` 的 crate 内版本（保留 [`StoreError`] 的细节）。
    ///
    /// # 为什么"未命中"返回 `Ok(None)` 而不是错误
    ///
    /// 缓存未命中是**正常状态**，不是错误。把它报成错误会让每次首次访问都
    /// 产生一次异常——书源的 `try/catch` 降级逻辑会被无关的"缓存没有"触发。
    /// （与 `read_file` 的"文件不存在是事实错误"相对，见 `MemoryStoreHost` 的
    /// 对照表。）
    ///
    /// # Cookie 键走 `cookie` 表
    ///
    /// `tome-js` 的 `java.getCookie`/`setCookie` 经 `cache_get`/`cache_put`
    /// 走宿主通道，键带 `tome://cookie/` 前缀（见 `tome_js::api::state` 的
    /// `COOKIE_KEY_PREFIX`）。这里**识别该前缀并转存到 `cookie` 表**——
    /// 若不转，Cookie 会以普通脚本缓存的形式躺在 `script_cache` 里，
    /// 与 `MiscDao` 的 Cookie 读写（界面登出、导入导出）看到的是两处数据，
    /// 表现为"脚本登录了但界面上显示未登录"。
    ///
    /// ## 用 `cache_get`/`cache_put` 而不是给 trait 加方法
    ///
    /// `StoreHost` 是 `tome-core` 定的接缝，加方法会波及 `tome-js` 的
    /// 全部实现与替身。而"Cookie 就是一类带前缀的缓存键"这个约定已经
    /// 在 JS 侧成立，在存储侧承认它即可——两边共用同一个前缀常量
    /// （[`crate::host::COOKIE_KEY_PREFIX`]，与 `tome-js` 的值一致并有测试守着）。
    pub(crate) fn cache_get_inner(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        if let Some(name) = key.strip_prefix(COOKIE_KEY_PREFIX) {
            return Ok(self.cookie_get(name)?.map(String::into_bytes));
        }
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached("SELECT value FROM script_cache WHERE source_id=?1 AND key=?2")
                .map_err(sql)?;
            let mut rows = statement
                .query(rusqlite::params![self.source_id, key])
                .map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(row.get::<_, Vec<u8>>(0).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// `cache_put` 的 crate 内版本。
    pub(crate) fn cache_put_inner(&self, key: &str, value: &[u8]) -> Result<(), StoreError> {
        if let Some(name) = key.strip_prefix(COOKIE_KEY_PREFIX) {
            // Cookie 值按定义是文本（`setCookie(name, value)` 的 value 是字符串）。
            // 非法 UTF-8 说明调用方用错了通道——报内容错误而不是 lossy 转换，
            // 那会把"脚本写错了"变成"存了一个带替换字符的凭据"。
            let text = std::str::from_utf8(value).map_err(|_| StoreError::Corrupt {
                subject: format!("cookie:{name}"),
                reason: "Cookie 值不是合法 UTF-8".to_string(),
            })?;
            return self.cookie_put(name, text);
        }
        let now = crate::now_unix();
        // 闭包跨线程（actor 通道），故必须持有数据的所有权而非借用。
        // 直接把 `&str`/`&[u8]` move 进去会让闭包带上 `self` 的生命周期，
        // 与 `F: 'static` 冲突——这就是上面两个 `to_owned` 的存在理由。
        let source_id = self.source_id.clone();
        let key = key.to_string();
        let value = value.to_vec();
        self.db.write(move |connection| {
            connection
                .execute(
                    "INSERT INTO script_cache(source_id, key, value, updated_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(source_id, key) DO UPDATE SET value=excluded.value,
                                                                 updated_at=excluded.updated_at",
                    rusqlite::params![source_id, key, value, now],
                )
                .map_err(sql)?;
            Ok(())
        })
    }

    /// `read_file` 的 crate 内版本。
    pub(crate) fn read_file_inner(&self, relative_path: &str) -> Result<Vec<u8>, StoreError> {
        // 越界检查**先于**读取：顺序反了会先读到数据再报错，
        // 而那意味着越界已经发生过。
        let path = self.resolve_inside(relative_path)?;
        std::fs::read(&path).map_err(|error| {
            // 文件不存在是**事实错误**（`Content` 分类），不是"存储坏了"。
            // 脚本据此判断"书没了"并走降级路径。
            if error.kind() == std::io::ErrorKind::NotFound {
                StoreError::not_found(relative_path)
            } else {
                from_io(&path, error)
            }
        })
    }

    /// `write_file` 的 crate 内版本。
    pub(crate) fn write_file_inner(
        &self,
        relative_path: &str,
        contents: &[u8],
    ) -> Result<(), StoreError> {
        // 越界检查必须在**写入之前**：事后再报错，文件已经写出去了。
        let path = self.resolve_inside(relative_path)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| from_io(parent, error))?;
        }
        std::fs::write(&path, contents).map_err(|error| from_io(&path, error))
    }

    /// 读 Cookie（按源隔离）。
    ///
    /// # 为什么这里又有一份（与 [`crate::dao::misc::MiscDao::cookie`] 并列）
    ///
    /// 两者服务不同的调用方，形状要求不同：
    ///
    /// - `MiscDao::cookie` 给**上层**用（界面展示、导入导出），返回完整
    ///   `Cookie` DTO，含 `domain`/`expires_at`。
    /// - 本方法给**JS 宿主**用（`java.getCookie`），只关心值。JS 侧没有
    ///   domain 的概念（`setCookie(name, value)` 只有两个参数）。
    ///
    /// 二者共用同一张表与同一套 `(source_id, name, 'script')` 主键，
    /// 故脚本写的 Cookie 与上层看到的 Cookie 不会分裂成两份。
    pub(crate) fn cookie_get(&self, name: &str) -> Result<Option<String>, StoreError> {
        self.db.read(|connection| {
            let mut statement = connection
                .prepare_cached(
                    "SELECT value FROM cookie WHERE source_id=?1 AND name=?2 AND domain=?3",
                )
                .map_err(sql)?;
            let mut rows = statement
                .query(rusqlite::params![
                    self.source_id,
                    name,
                    SCRIPT_COOKIE_DOMAIN
                ])
                .map_err(sql)?;
            match rows.next().map_err(sql)? {
                Some(row) => Ok(Some(row.get::<_, String>(0).map_err(sql)?)),
                None => Ok(None),
            }
        })
    }

    /// 写 Cookie（按源隔离，FR-026：重启后保留登录态）。
    pub(crate) fn cookie_put(&self, name: &str, value: &str) -> Result<(), StoreError> {
        let source_id = self.source_id.clone();
        let name = name.to_string();
        let value = value.to_string();
        self.db.write(move |connection| {
            connection
                .execute(
                    "INSERT INTO cookie(source_id, name, value, domain, expires_at)
                     VALUES (?1, ?2, ?3, ?4, NULL)
                     ON CONFLICT(source_id, name, domain) DO UPDATE SET value=excluded.value",
                    rusqlite::params![source_id, name, value, SCRIPT_COOKIE_DOMAIN],
                )
                .map_err(sql)?;
            Ok(())
        })
    }
}

/// 词法判定：相对路径是否留在数据目录内。
///
/// 判据与 `tome-js::MemoryStoreHost` 的 `is_inside_data_dir` **刻意相同**：
/// 替身与真实实现判据不一致会让"替身上绿的测试在真实存储上红"，
/// 而那时问题已经被埋进两层实现里。
///
/// 按 `/` 与 `\` 两种分隔符切分（书源脚本可能来自任何平台，两种都要挡），
/// 逐段折叠目录深度：`..` 减一，普通段加一，`.` 与空段忽略；深度为负即逃逸。
pub(crate) fn is_lexically_inside(relative_path: &str) -> bool {
    let trimmed = relative_path.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return false;
    }
    // Windows 盘符形态：`C:` / `c:\`
    if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
        return false;
    }

    let mut depth: i64 = 0;
    for segment in trimmed.split(['/', '\\']) {
        match segment {
            "" | "." => continue,
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => depth += 1,
        }
    }
    true
}

/// 把一个（可能不存在的）路径绝对化而**不解析符号链接**。
///
/// `canonicalize` 要求路径存在；数据目录首次使用时它还不存在，故需要这个
/// 纯词法的绝对化。它保留 `..` 的原始形态，但 [`is_lexically_inside`] 已经
/// 保证进入这里的路径不含逃逸的 `..`。
fn absolute_lexical(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut result = base;
    for component in path.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use tome_core::ErrorCapability;

    struct Fixture {
        dir: PathBuf,
        host: SqliteStoreHost,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str, source_id: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-host-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = Db::open(&dir.join("tome.db")).expect("应能开库");
        Fixture {
            host: SqliteStoreHost::new(db, &dir, source_id),
            dir,
        }
    }

    #[test]
    fn cookie_键前缀与_js_侧一致() {
        // 这是跨 crate 的隐式协议（tome-js 用这个前缀把 Cookie 塞进 cache 通道）。
        // 依赖方向不允许 tome-store 反过来引用 tome-js 的常量，故用字面量钉住：
        // 改动任一侧都会让本测试红，从而迫使另一侧同步。
        assert_eq!(COOKIE_KEY_PREFIX, "tome://cookie/");
    }

    #[test]
    fn 带_cookie_前缀的键落到_cookie_表() {
        // 若不转存，Cookie 会躺在 script_cache 里，而 MiscDao 的 Cookie 读写
        // 看的是 cookie 表——表现为"脚本登录了但界面上显示未登录"
        let fixture = fixture("cookie-routing", "https://a.com");
        fixture
            .host
            .cache_put_inner(&format!("{COOKIE_KEY_PREFIX}token"), b"abc")
            .expect("应能写");
        assert_eq!(
            fixture
                .host
                .cache_get_inner(&format!("{COOKIE_KEY_PREFIX}token"))
                .expect("应能读"),
            Some(b"abc".to_vec())
        );

        // 关键：它必须能被 MiscDao 的 Cookie 接口看到
        let dao = crate::dao::misc::MiscDao::with_now(fixture.host.db.clone(), 0);
        assert_eq!(
            dao.cookie("https://a.com", "token", None)
                .expect("应能读")
                .expect("应有")
                .value,
            "abc",
            "经 JS 通道写的 Cookie 必须与 MiscDao 看到的是同一份"
        );

        // 而不该污染 script_cache
        let cached: i64 = fixture
            .host
            .db
            .read(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM script_cache", [], |row| row.get(0))
                    .map_err(sql)
            })
            .expect("应能查");
        assert_eq!(cached, 0, "Cookie 不该落进 script_cache");
    }

    #[test]
    fn 缓存往返() {
        let fixture = fixture("cache", "https://a.com");
        assert_eq!(
            fixture.host.cache_get_inner("k").expect("读应成功"),
            None,
            "未命中应是 Ok(None) 而不是错误"
        );
        fixture.host.cache_put_inner("k", b"v").expect("写应成功");
        assert_eq!(
            fixture.host.cache_get_inner("k").expect("读应成功"),
            Some(b"v".to_vec())
        );
        // 覆盖写
        fixture.host.cache_put_inner("k", b"v2").expect("写应成功");
        assert_eq!(
            fixture.host.cache_get_inner("k").expect("读应成功"),
            Some(b"v2".to_vec())
        );
    }

    #[test]
    fn 二进制内容原样往返() {
        let fixture = fixture("binary", "s");
        let payload: Vec<u8> = (0u16..=255).map(|byte| byte as u8).collect();
        fixture
            .host
            .cache_put_inner("bin", &payload)
            .expect("应能写");
        assert_eq!(
            fixture.host.cache_get_inner("bin").expect("应能读"),
            Some(payload),
            "缓存必须能原样承载二进制（BLOB 而非 TEXT）"
        );
    }

    #[test]
    fn 缓存按源隔离() {
        // 跨源共享缓存会把 A 源的登录 token 漏给 B 源
        let base = fixture("isolation", "https://a.com");
        base.host
            .cache_put_inner("token", "A的秘密".as_bytes())
            .expect("应能写");

        let other = SqliteStoreHost::new(base.host.db.clone(), &base.dir, "https://b.com");
        assert_eq!(
            other.cache_get_inner("token").expect("读应成功"),
            None,
            "B 源不该读到 A 源的缓存"
        );
        let _ = &base;
    }

    #[test]
    fn cookie_按源隔离且跨重启保留() {
        let fixture = fixture("cookie", "https://a.com");
        fixture.host.cookie_put("token", "abc").expect("应能写");
        assert_eq!(
            fixture.host.cookie_get("token").expect("应能读"),
            Some("abc".to_string())
        );

        // 另一个源用**同名**的 cookie：必须互不可见
        let other = SqliteStoreHost::new(fixture.host.db.clone(), &fixture.dir, "https://b.com");
        assert_eq!(
            other.cookie_get("token").expect("应能读"),
            None,
            "同名 Cookie 不得跨源泄漏"
        );
        other.cookie_put("token", "xyz").expect("应能写");

        // FR-026：重启（重开库）后仍在
        let db_path = fixture.dir.join("tome.db");
        drop(other);
        drop(std::mem::replace(
            &mut fixture.host.clone(),
            SqliteStoreHost::new(
                Db::open(&db_path).expect("应能重开"),
                &fixture.dir,
                "https://a.com",
            ),
        ));
        let reopened = SqliteStoreHost::new(
            Db::open(&db_path).expect("应能重开"),
            &fixture.dir,
            "https://a.com",
        );
        assert_eq!(
            reopened.cookie_get("token").expect("应能读"),
            Some("abc".to_string()),
            "重启后登录态应保留（FR-026）"
        );
    }

    #[test]
    fn 文件读写往返() {
        let fixture = fixture("file", "s");
        fixture
            .host
            .write_file_inner("sub/a.txt", b"hello")
            .expect("应能写");
        assert_eq!(
            fixture.host.read_file_inner("sub/a.txt").expect("应能读"),
            b"hello".to_vec()
        );
        assert!(
            fixture.dir.join("sub").join("a.txt").exists(),
            "应真的落在数据目录下"
        );
    }

    #[test]
    fn 读不存在的文件报内容错误() {
        let fixture = fixture("missing", "s");
        let error = fixture
            .host
            .read_file_inner("nope.txt")
            .expect_err("应报错");
        assert!(
            matches!(error, StoreError::NotFound { .. }),
            "文件不存在应是 NotFound：{error:?}"
        );
    }

    #[test]
    fn 越界路径一律被拒且不落盘() {
        let fixture = fixture("escape", "s");
        for path in [
            "../x",
            "a/../../x",
            "/etc/passwd",
            "C:/x",
            "..\\x",
            "",
            "   ",
        ] {
            let error = fixture
                .host
                .write_file_inner(path, b"payload")
                .expect_err(&format!("`{path}` 应被拒绝"));
            assert_eq!(
                error.to_error_kind(),
                ErrorKind::unsupported_capability(ErrorCapability::new("fileAccessOutOfDataDir")),
                "`{path}` 应映射为可捕获的能力错误（FR-006）"
            );
        }
        // 越界写入不得留下任何文件
        let escaped = fixture.dir.parent().expect("应有父目录").join("x");
        assert!(!escaped.exists(), "越界写入不得落到数据目录之外");
    }

    #[test]
    fn 词法判据与内存替身一致() {
        // 替身与真实实现判据不一致＝"替身上绿的测试在真实存储上红"，
        // 而那时问题已经埋进两层实现。故逐条对照。
        for allowed in [
            "a.txt",
            "sub/a.txt",
            "./a.txt",
            "sub//a.txt",
            "...txt",
            "..hidden",
            "a..b",
            "a/./b",
            "a/b/../c",
            "a/b/../..",
        ] {
            assert!(is_lexically_inside(allowed), "`{allowed}` 应在界内");
        }
        for denied in [
            "..",
            "../a",
            "a/../..",
            "a/../../b",
            "../../etc/passwd",
            "/etc/passwd",
            "/",
            "\\windows",
            "C:/x",
            "c:\\x",
            "..\\a",
            "a\\..\\..\\b",
            "",
            "   ",
        ] {
            assert!(!is_lexically_inside(denied), "`{denied}` 应被判越界");
        }
    }

    #[test]
    fn 符号链接逃逸被文件系统级判定挡住() {
        // 词法上 `link/passwd` 完全合法——它不含任何 `..`。
        // 挡住它的是 canonicalize 后的前缀判定。
        let fixture = fixture("symlink", "s");
        let outside = fixture
            .dir
            .parent()
            .expect("应有父目录")
            .join(format!("tome-host-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).expect("应能建外部目录");
        std::fs::write(outside.join("secret.txt"), "秘密".as_bytes()).expect("应能写");

        let link = fixture.dir.join("link");
        let linked = create_dir_symlink(&outside, &link);
        if !linked {
            // 无权限建符号链接（Windows 非开发者模式）：跳过本用例而不是
            // 假绿——静默通过会让"符号链接防线失效"永远不被发现。
            let _ = std::fs::remove_dir_all(&outside);
            return;
        }

        let result = fixture.host.read_file_inner("link/secret.txt");
        assert!(
            result.is_err(),
            "经符号链接逃出数据目录必须被拒：{result:?}"
        );
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// 建目录符号链接；平台不支持时返回 `false`。
    fn create_dir_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }
}
