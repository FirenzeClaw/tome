//! [DONE] 内存态宿主替身：`StoreHost` 与 `CryptoHost`（tasks.md T033）
//!
//! # 为什么替身是产品的一部分，而不仅是测试脚手架
//!
//! tasks.md 明确要求它们"供 JS 沙箱单测与 **S1-2 阶段使用**；真实 store 实现
//! 在 S1-4 T054"。故它们是**公开 API**：
//!
//! - S1-2 的 CLI 闭环（`tome run <js-source>`）需要它们才能跑通"含 JS 的书源"，
//!   那时 `tome-store` 还没落地。没有替身，这个阶段的独立测试无法演示。
//! - 测试替身让"宿主 API 八类"（T036-T041）可以在**不依赖真实存储**的前提下
//!   逐个验证，把"脚本↔宿主"这条接缝与"宿主的实现质量"分开来看。
//!
//! # 替身也必须忠实实现 trait 的约束
//!
//! 尤其是**文件访问限数据目录内**（AGENTS.md 安全红线 / spec 边缘情况）。
//! 一个"什么都接受"的替身会让 T041 的安全红线测试**假绿**——等 S1-4 真实
//! `tome-store` 装上后才发现越界没挡住，而那时问题已经被埋进两层实现里。
//!
//! 故本模块的 [`MemoryStoreHost::read_file`] / [`write_file`] 实现了完整的
//! 路径包含判定，与真实实现的要求一致：
//!
//! | 输入形态 | 判定 | 理由 |
//! |---|---|---|
//! | `a.txt` / `sub/a.txt` | 允许 | 数据目录内的相对路径 |
//! | `../x` / `sub/../../x` | 拒绝 | 向上逃逸 |
//! | `/etc/passwd` / `C:/...` | 拒绝 | 绝对路径绕过相对目录语义 |
//! | `..\x` / `sub\..\..\x` | 拒绝 | Windows 反斜杠形态 |
//! | `./a` / `..hidden` / `...txt` | 允许 | 名字带点但不是逃逸语义 |
//!
//! 拒绝一律返回 [`ErrorKind::UnsupportedCapability`]——**可被书源 try/catch
//! 捕获**（FR-006）。若返回 panic 或静默失败，书源的降级逻辑就无从判断。
//!
//! # 为什么缓存与文件对"缺失"的处理不同
//!
//! | 通道 | 缺失时 | 理由 |
//! |---|---|---|
//! | `cache_get` | `Ok(None)` | 缓存未命中是**正常状态**，不是错误 |
//! | `read_file` | `Err(Content)` | 文件不存在是**事实错误**，脚本据此判断"书没了" |
//!
//! 把二者统一会让"缓存没命中"被当成错误（每次首次访问都报错），
//! 或让"文件不存在"被当成空内容（脚本拿到空串继续跑，产出无意义结果）。

use std::collections::HashMap;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::prelude::Engine as _;
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use tome_core::{CryptoHost, ErrorKind, StoreHost};

/// 内存态的 [`StoreHost`]。
///
/// 缓存与文件共用一个实例，但分属两个独立键空间（缓存键与文件路径互不干扰）。
///
/// # 为什么内部用 `Mutex<HashMap>`
///
/// trait 要求 `Send + Sync`（宿主会被多个沙箱共享），而 `HashMap` 需要可变
/// 访问。用内部可变性让方法取 `&self`——这与真实实现的形态一致
/// （`tome-store` 是 actor 线程 + 消息传递，对外同样是 `&self`）。
///
/// 锁毒化时用 `PoisonError::into_inner` 继续：数据是"内存态替身"，
/// 一个测试线程 panic 不该让其他线程全部失效（生产路径禁 panic，
/// 但测试替身面对的正是会 panic 的测试环境）。
#[derive(Debug, Default)]
pub struct MemoryStoreHost {
    cache: Mutex<HashMap<String, Vec<u8>>>,
    files: Mutex<HashMap<String, Vec<u8>>>,
    /// 访问历史（诊断与测试断言用）
    log: Mutex<Vec<String>>,
}

impl MemoryStoreHost {
    /// 新建空的替身。
    pub fn new() -> Self {
        Self::default()
    }

    /// 已记录的访问历史（按发生顺序）。
    ///
    /// 供测试断言"脚本确实按预期访问了存储"——只看最终状态无法区分
    /// "脚本没读它"与"读了但没改"。T041 验证越界拒绝时也靠它确认
    /// **拒绝发生在写入之前**（历史里有记录，但存储里没有脏数据）。
    pub fn access_log(&self) -> Vec<String> {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 当前缓存中的键数（诊断用）。
    pub fn cache_len(&self) -> usize {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 当前文件数（诊断用）。
    pub fn file_len(&self) -> usize {
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 记一条访问历史。
    fn record(&self, entry: String) {
        self.log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(entry);
    }
}

impl StoreHost for MemoryStoreHost {
    fn cache_get(&self, key: &str) -> Result<Option<Vec<u8>>, ErrorKind> {
        self.record(format!("cache_get({key})"));
        Ok(self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned())
    }

    fn cache_put(&self, key: &str, value: &[u8]) -> Result<(), ErrorKind> {
        self.record(format!("cache_put({key}, {} 字节)", value.len()));
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.to_string(), value.to_vec());
        Ok(())
    }

    fn read_file(&self, relative_path: &str) -> Result<Vec<u8>, ErrorKind> {
        self.record(format!("read_file({relative_path})"));
        // 越界检查**先于**读取：顺序反了会先读到数据再报错，
        // 而那意味着越界已经发生过。
        if !is_inside_data_dir(relative_path) {
            return Err(ErrorKind::unsupported_capability(
                tome_core::ErrorCapability::new("fileAccessOutOfDataDir"),
            ));
        }
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(relative_path)
            .cloned()
            .ok_or(ErrorKind::Content)
    }

    fn write_file(&self, relative_path: &str, contents: &[u8]) -> Result<(), ErrorKind> {
        self.record(format!(
            "write_file({relative_path}, {} 字节)",
            contents.len()
        ));
        // 越界检查必须在**写入之前**：事后再报错，文件已经写出去了。
        if !is_inside_data_dir(relative_path) {
            return Err(ErrorKind::unsupported_capability(
                tome_core::ErrorCapability::new("fileAccessOutOfDataDir"),
            ));
        }
        self.files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(relative_path.to_string(), contents.to_vec());
        Ok(())
    }
}

/// 判断相对路径是否**留在数据目录内**。
///
/// # 判据
///
/// 按 `/` 与 `\` 两种分隔符切分（书源脚本可能来自任何平台，两种都要挡），
/// 逐段折叠目录深度：`..` 减一，普通段加一，`.` 与空段（`//`）忽略。
/// 任何时刻深度为负即逃逸。
///
/// ## 为什么不用 `std::path::Path::canonicalize`
///
/// 那要求路径**真的存在**于文件系统上——而这是内存态替身，文件本来就不存在。
/// 且 canonicalize 会跟随符号链接，语义上比我们要判的"词法包含"更宽。
/// 真实实现（S1-4）需要额外处理符号链接与大小写（Windows），
/// 故那边应当在 canonicalize 之后**再**做一次前缀包含判定；
/// 本替身只需词法判定，但判据必须同样严格。
///
/// ## 为什么先判绝对路径
///
/// 绝对路径（`/...`、`C:/...`、`\\server\share`）在语义上**不在**相对目录内，
/// 无论它指向哪里。若只按 `..` 折叠判深度，`/etc/passwd` 会被判为"深度 1"而
/// 放行——那是明显的越界。
fn is_inside_data_dir(relative_path: &str) -> bool {
    let trimmed = relative_path.trim();
    if trimmed.is_empty() {
        // 空路径不指向任何文件。放行会让"写空名文件"变得可能，
        // 而那是无意义的输入——拒绝它更安全。
        return false;
    }

    // 绝对路径一律拒绝（逐条对应一种写法）
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
            // 当前目录与重复分隔符：不影响深度
            "" | "." => continue,
            ".." => {
                depth -= 1;
                // 深度为负即已逃出数据目录——**不必等到遍历结束**：
                // `a/../../..` 在第二个 `..` 处就已经越界了。
                if depth < 0 {
                    return false;
                }
            }
            _ => depth += 1,
        }
    }
    // 遍历结束仍在界内（`..` 只在把深度打到负数时才算逃逸——
    // 上面已即时拦截，故此处无需再判）
    true
}

/// 内存态的 [`CryptoHost`]（纯计算，无状态）。
///
/// # 为什么它叫 "Memory" 前缀
///
/// 与 [`MemoryStoreHost`] 保持一致：两者都是"不需要外部资源"的实现。
/// 加密本体是**真实实现**（不是桩）——FR-005 的摘要与加密是可完整实现的
/// 纯计算，trait 文档也写明"tome-js 自己实现，不需组合根注入"。
/// 故它可以直接用在生产路径上（S1-2 的 CLI 闭环正是这么用的）。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryCryptoHost {
    _private: (),
}

impl MemoryCryptoHost {
    /// 新建。
    pub fn new() -> Self {
        Self::default()
    }
}

impl CryptoHost for MemoryCryptoHost {
    fn digest(&self, algorithm: &str, data: &[u8]) -> Result<Vec<u8>, ErrorKind> {
        // 算法名归一：去连字符、转小写。书源里 `SHA-256` / `sha256` / `Sha256`
        // 三种写法都常见，语义没有歧义，故宽松接受。
        let normalized: String = algorithm
            .chars()
            .filter(|character| *character != '-' && *character != '_')
            .flat_map(char::to_lowercase)
            .collect();

        match normalized.as_str() {
            "md5" => Ok(Md5::digest(data).to_vec()),
            "sha1" => Ok(Sha1::digest(data).to_vec()),
            "sha256" => Ok(Sha256::digest(data).to_vec()),
            "sha512" => Ok(Sha512::digest(data).to_vec()),
            // 未知算法是**书源写错名字**，属规则错而不是内容错——
            // 前者用户该去改书源，后者该去查数据。
            _ => Err(ErrorKind::Rule),
        }
    }

    fn base64_encode(&self, data: &[u8]) -> String {
        BASE64_STANDARD.encode(data)
    }

    fn base64_decode(&self, encoded: &str) -> Result<Vec<u8>, ErrorKind> {
        // 容错：去掉全部 ASCII 空白。真实的 base64 数据常带换行
        // （PEM 风格、HTTP 头折行），拒绝它只会让一批能用的源不可用，
        // 而语义没有歧义（标准字母表里没有空白字符）。
        let compact: String = encoded
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        BASE64_STANDARD
            .decode(compact.as_bytes())
            .map_err(|_| ErrorKind::Content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 数据目录内的路径被接受() {
        for path in [
            "a.txt",
            "sub/a.txt",
            "a/b/c/d.bin",
            "./a.txt",
            "sub//a.txt",
            "...txt",
            "..hidden",
            "a..b",
            "a/./b",
            "a/b/../c",
        ] {
            assert!(is_inside_data_dir(path), "`{path}` 应在数据目录内");
        }
    }

    #[test]
    fn 逃逸路径被拒绝() {
        for path in [
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
            assert!(!is_inside_data_dir(path), "`{path}` 应被判定为越界");
        }
    }

    #[test]
    fn 深度归类判据是即时的() {
        // `a/../../..` 在第二个 `..` 处就已越界（深度 -1），
        // 不该等到遍历结束才判——否则中间的越界状态会被后续段"抵消"的假象掩盖。
        assert!(!is_inside_data_dir("a/../../.."));
        // 而 `a/b/../..` 恰好回到根，**仍在界内**（深度 0）。
        assert!(is_inside_data_dir("a/b/../.."));
    }

    #[test]
    fn 摘要算法名归一() {
        let crypto = MemoryCryptoHost::new();
        let expected = crypto.digest("sha256", b"x").expect("应成功");
        for alias in ["SHA256", "SHA-256", "sha_256", "Sha256"] {
            assert_eq!(
                crypto.digest(alias, b"x").expect("应成功"),
                expected,
                "别名 `{alias}` 应等价"
            );
        }
    }
}
