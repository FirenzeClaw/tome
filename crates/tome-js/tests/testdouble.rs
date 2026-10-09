//! T033 验收测试：内存态宿主替身（tasks.md Phase 4）
//!
//! ## 这些替身为什么是"产品的一部分"，不是"测试脚手架"
//!
//! tasks.md 明确要求"供 JS 沙箱单测与 **S1-2 阶段使用**；真实 store 实现在
//! S1-4 T054"。这有两个直接后果：
//!
//! 1. 它们必须是**库的公开 API**（`tome_js::testdouble`），而不是
//!    `#[cfg(test)]` 里的私有类型——S1-2 的 CLI 闭环（`tome run`）要用它们
//!    跑通"含 JS 的书源"，那时 `tome-store` 还没落地。
//! 2. 它们必须**忠实实现 trait 的约束**，尤其是文件访问的越界拒绝。
//!    一个"什么都接受"的替身会让 T041 的安全红线测试假绿——真实
//!    `tome-store` 装上后才发现越界没挡住。
//!
//! ## 测试覆盖的两条线
//!
//! - **行为正确性**：缓存读写往返、文件读写往返。
//! - **安全约束**：路径越界必须被拒绝（`../../etc/passwd` 这类），
//!   且拒绝方式是**结构化错误**而非 panic——书源的 try/catch 依赖它。

use std::sync::Arc;

use tome_core::{CryptoHost, ErrorKind, StoreHost};
use tome_js::testdouble::{MemoryCryptoHost, MemoryStoreHost};

// ---------------------------------------------------------------------------
// MemoryStoreHost：缓存
// ---------------------------------------------------------------------------

#[test]
fn 缓存写入后可读回() {
    let store = MemoryStoreHost::new();
    store.cache_put("token", b"abc").expect("写缓存应成功");
    let value = store.cache_get("token").expect("读缓存应成功");
    assert_eq!(value.as_deref(), Some(&b"abc"[..]));
}

#[test]
fn 未写过的缓存键返回_none() {
    // `None` 与 `Some(空 vec)` 语义不同：前者是"没有这个键"，
    // 后者是"这个键的值是空"。书源脚本靠它区分"缓存没命中"与"命中了空值"。
    let store = MemoryStoreHost::new();
    assert_eq!(store.cache_get("missing").expect("读应成功"), None);
}

#[test]
fn 缓存可覆盖且可存二进制() {
    let store = MemoryStoreHost::new();
    store.cache_put("k", b"first").expect("应成功");
    store.cache_put("k", b"second").expect("应成功");
    assert_eq!(
        store.cache_get("k").expect("应成功").as_deref(),
        Some(&b"second"[..]),
        "同键应覆盖"
    );

    // 二进制（含 NUL 与非 UTF-8 字节）：缓存是字节通道，不是文本通道。
    // 用 `Vec<u8>` 而非 `String` 正是为了让图片/压缩数据也能过。
    let binary: Vec<u8> = vec![0x00, 0xFF, 0x80, 0x01];
    store.cache_put("bin", &binary).expect("应成功");
    assert_eq!(
        store.cache_get("bin").expect("应成功").as_deref(),
        Some(binary.as_slice())
    );
}

#[test]
fn 键空间是隔离的() {
    let store = MemoryStoreHost::new();
    store.cache_put("a", b"1").expect("应成功");
    store.cache_put("b", b"2").expect("应成功");
    assert_eq!(
        store.cache_get("a").expect("应成功").as_deref(),
        Some(&b"1"[..])
    );
    assert_eq!(
        store.cache_get("b").expect("应成功").as_deref(),
        Some(&b"2"[..])
    );
}

// ---------------------------------------------------------------------------
// MemoryStoreHost：文件（限数据目录内）
// ---------------------------------------------------------------------------

#[test]
fn 文件写入后可读回() {
    let store = MemoryStoreHost::new();
    store
        .write_file("book/1.txt", "内容".as_bytes())
        .expect("写文件应成功");
    assert_eq!(
        store.read_file("book/1.txt").expect("读文件应成功"),
        "内容".as_bytes().to_vec()
    );
}

#[test]
fn 未写过的文件报错而非返回空() {
    // 与缓存不同：文件缺失是**错误**（书源脚本会据此判断"书不存在"），
    // 而缓存缺失是正常状态（`None`）。
    let store = MemoryStoreHost::new();
    let error = store.read_file("missing.txt").expect_err("应报错");
    assert_eq!(error, ErrorKind::Content);
}

#[test]
fn 越界路径被拒绝并报结构化错误() {
    // ## 这是本模块最重要的测试（AGENTS.md 安全红线）
    //
    // 脚本的文件访问限制在数据目录内，越界须拒绝并报告。
    // 覆盖四类越界写法：
    //
    // 1. `../` 向上逃逸
    // 2. 绝对路径（绕过相对目录的语义）
    // 3. 嵌套多层的 `../`
    // 4. Windows 的反斜杠形态（`..\\` 与 `..\`）
    //
    // 拒绝方式必须是**结构化错误**且可被书源 try/catch 捕获
    // （`UnsupportedCapability`），而不是 panic 或静默成功。
    let store = MemoryStoreHost::new();
    for path in [
        "../secret.txt",
        "../../etc/passwd",
        "sub/../../outside.txt",
        "/etc/passwd",
        "C:/Windows/system32/config",
        "..\\windows\\file.txt",
        "sub\\..\\..\\escape.txt",
        // 仅 `..` 本身（不指向任何文件但仍是逃逸）
        "..",
        "../",
    ] {
        let error = store
            .read_file(path)
            .expect_err(&format!("越界读 `{path}` 必须被拒绝"));
        assert!(
            matches!(error, ErrorKind::UnsupportedCapability { .. }),
            "`{path}` 应产出 UnsupportedCapability（可被书源捕获），实际 {error:?}"
        );

        let error = store
            .write_file(path, b"x")
            .expect_err(&format!("越界写 `{path}` 必须被拒绝"));
        assert!(
            matches!(error, ErrorKind::UnsupportedCapability { .. }),
            "越界写 `{path}` 应产出 UnsupportedCapability，实际 {error:?}"
        );
    }
}

#[test]
fn 越界拒绝后不留下任何副作用() {
    // 拒绝必须发生在写入**之前**——否则文件已经写出去，报错只是事后通知。
    let store = MemoryStoreHost::new();
    let _ = store.write_file("../escape.txt", "不该被写出".as_bytes());
    assert!(
        store.read_file("../escape.txt").is_err(),
        "越界写入不得真的落盘（拒绝必须前置）"
    );
}

#[test]
fn 数据目录内的正常路径不受限() {
    // 反向验收：安全约束不该误伤正常用法。
    // 尤其要覆盖"名字里带点但不在越界语义里"的形态。
    let store = MemoryStoreHost::new();
    for path in [
        "a.txt",
        "sub/a.txt",
        "deep/nested/path/file.bin",
        // 带点的文件名（不是 `..`）
        "...txt",
        "..hidden",
        "a..b.txt",
        // 单点（当前目录语义，不是逃逸）
        "./a.txt",
    ] {
        store
            .write_file(path, b"ok")
            .unwrap_or_else(|error| panic!("数据目录内的 `{path}` 不应被拒绝：{error:?}"));
    }
}

#[test]
fn 存储替身可跨线程共享() {
    // trait 要求 `Send + Sync`（宿主能力会被多个沙箱共享）。
    // 这条测试同时证明了"替身真的能进 Arc<dyn StoreHost>"——
    // 组合根正是这么注入它的。
    let store: Arc<dyn StoreHost> = Arc::new(MemoryStoreHost::new());
    store.cache_put("k", b"v").expect("应成功");

    let cloned = Arc::clone(&store);
    let handle = std::thread::spawn(move || cloned.cache_get("k").expect("应成功"));
    assert_eq!(
        handle.join().expect("线程应正常结束").as_deref(),
        Some(&b"v"[..])
    );
}

#[test]
fn 存储替身记录调用历史() {
    // 供测试断言"脚本确实按预期访问了存储"，而不只是看到最终状态。
    // 例如 T041 要验证"文件写入限数据目录"时，需要知道脚本尝试访问了什么。
    let store = MemoryStoreHost::new();
    store.cache_put("a", b"1").expect("应成功");
    let _ = store.cache_get("a");
    let _ = store.read_file("../bad");

    let accesses = store.access_log();
    assert_eq!(accesses.len(), 3, "应记录三次访问：{accesses:?}");
    assert!(accesses[0].contains("cache_put"), "{accesses:?}");
    assert!(accesses[1].contains("cache_get"), "{accesses:?}");
    assert!(
        accesses[2].contains("read_file") && accesses[2].contains("bad"),
        "记录应含被拒绝的路径：{accesses:?}"
    );
}

// ---------------------------------------------------------------------------
// MemoryCryptoHost
// ---------------------------------------------------------------------------

#[test]
fn 摘要算法产出已知向量() {
    // ## 为什么用已知测试向量而不是"两次调用结果相同"
    //
    // "两次调用结果相同"只能证明它是确定性的，不能证明它算的是**MD5**。
    // 一个永远返回空串的实现也能通过那种测试——而这正是摘要最危险的
    // 失效形态（书源的签名校验会静默失败）。
    let crypto = MemoryCryptoHost::new();

    // RFC 1321 的 MD5 标准测试向量
    assert_eq!(
        hex(&crypto.digest("md5", b"").expect("应成功")),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
    assert_eq!(
        hex(&crypto.digest("md5", b"abc").expect("应成功")),
        "900150983cd24fb0d6963f7d28e17f72"
    );

    // FIPS 180-4 的 SHA-1 / SHA-256 / SHA-512 标准测试向量（`abc`）
    assert_eq!(
        hex(&crypto.digest("sha1", b"abc").expect("应成功")),
        "a9993e364706816aba3e25717850c26c9cd0d89d"
    );
    assert_eq!(
        hex(&crypto.digest("sha256", b"abc").expect("应成功")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&crypto.digest("sha512", b"abc").expect("应成功")),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
}

#[test]
fn 摘要算法名大小写与别名都接受() {
    // 书源里 `java.md5Encode` 之类的调用形态多样，算法名的大小写
    // 与连字符写法都不统一。宽松接受、严格计算。
    let crypto = MemoryCryptoHost::new();
    let expected = hex(&crypto.digest("sha256", b"abc").expect("应成功"));
    for alias in ["sha256", "SHA256", "SHA-256", "Sha256", "sha-256"] {
        assert_eq!(
            hex(&crypto.digest(alias, b"abc").expect("应成功")),
            expected,
            "别名 `{alias}` 应等价"
        );
    }
}

#[test]
fn 未知摘要算法报结构化错误() {
    let crypto = MemoryCryptoHost::new();
    let error = crypto.digest("sha3-512", b"x").expect_err("应报错");
    assert_eq!(
        error,
        ErrorKind::Rule,
        "算法不支持属规则错（书源写了引擎不认的名字）"
    );
}

#[test]
fn base64_往返() {
    let crypto = MemoryCryptoHost::new();
    // RFC 4648 的标准向量
    assert_eq!(crypto.base64_encode(b""), "");
    assert_eq!(crypto.base64_encode(b"f"), "Zg==");
    assert_eq!(crypto.base64_encode(b"fo"), "Zm8=");
    assert_eq!(crypto.base64_encode(b"foo"), "Zm9v");
    assert_eq!(crypto.base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(crypto.base64_encode(b"fooba"), "Zm9vYmE=");
    assert_eq!(crypto.base64_encode(b"foobar"), "Zm9vYmFy");

    for raw in [
        b"".as_slice(),
        b"f",
        b"fo",
        b"foo",
        b"foob",
        b"fooba",
        b"foobar",
    ] {
        let encoded = crypto.base64_encode(raw);
        assert_eq!(
            crypto.base64_decode(&encoded).expect("解码应成功"),
            raw.to_vec(),
            "往返应无损：{encoded}"
        );
    }

    // 二进制（含高位字节）
    let binary: Vec<u8> = vec![0x00, 0xFF, 0x80, 0x7F];
    assert_eq!(
        crypto
            .base64_decode(&crypto.base64_encode(&binary))
            .expect("应成功"),
        binary
    );
}

#[test]
fn 非法_base64_报内容错() {
    // 非法输入是第三方的常态（书源里的字符串常量可能被写坏）。
    // 必须是结构化错误，不能 panic、也不能静默产出垃圾字节。
    let crypto = MemoryCryptoHost::new();
    for invalid in ["!!!!", "a", "ab", "abc!", "=====", "Zg==Zg=="] {
        let outcome = crypto.base64_decode(invalid);
        assert!(
            outcome.is_err(),
            "非法 base64 `{invalid}` 应报错，实际 {outcome:?}"
        );
        assert_eq!(
            outcome.expect_err("应报错"),
            ErrorKind::Content,
            "非法编码属内容错"
        );
    }
}

#[test]
fn base64_解码容忍空白() {
    // 真实的 base64 数据常带换行（PEM 风格、HTTP 头折行）。
    // 拒绝它会让一批能用的源不可用，而语义没有歧义。
    let crypto = MemoryCryptoHost::new();
    assert_eq!(
        crypto.base64_decode("Zm9v\nYmFy").expect("应成功"),
        b"foobar".to_vec()
    );
    assert_eq!(
        crypto.base64_decode("  Zm9vYmFy  ").expect("应成功"),
        b"foobar".to_vec()
    );
}

#[test]
fn 加密替身可跨线程共享() {
    let crypto: Arc<dyn CryptoHost> = Arc::new(MemoryCryptoHost::new());
    let cloned = Arc::clone(&crypto);
    let handle = std::thread::spawn(move || cloned.base64_encode(b"x"));
    assert_eq!(handle.join().expect("线程应正常结束"), "eA==");
}

/// 字节转小写十六进制（测试断言用）。
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        // 去掉 `assert_eq!` 里为了方便阅读而写的续行空白
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
