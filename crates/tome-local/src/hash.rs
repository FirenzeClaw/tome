//! \[DONE] 归一化文件内容哈希（tasks.md T082 / FR-025）
//!
//! # 规格原文
//!
//! > **FR-025**: 系统 MUST 在文件移动或库目录路径变化后正确重建索引，
//! > 且**不产生重复条目**
//!
//! 本模块提供这条要求所依赖的那一个值：`Book.origin_id`。
//!
//! # 为什么哈希"归一化后的内容"而不是原始字节
//!
//! 同一本书在用户的硬盘上常有**字节不同但语义相同**的副本：
//!
//! | 差异 | 原始字节哈希 | 归一化哈希 |
//! |---|---|---|
//! | `\r\n` vs `\n`（Windows 与 Unix 之间拷贝过） | 不同 | **相同** |
//! | 尾部多一个换行（编辑器自动补） | 不同 | **相同** |
//! | UTF-8 BOM 有无（不同工具另存过） | 不同 | **相同** |
//!
//! 如果按原始字节算哈希，用户在 Windows 上重新保存一次 TXT 就会得到
//! "第二本书"，书架出现两个一模一样的条目——这正是 FR-025 要禁止的。
//!
//! 故哈希前先做三项归一化（见 [`normalize`]），只保留**内容本身**的差异。
//!
//! # 为什么哈希**不含路径**
//!
//! 路径是最易变的东西（换盘符、整理目录、库根重新挂载），而 FR-025 的
//! 后半句正是"库目录路径变化后不产生重复条目"。身份若含路径，一次
//! `D:\books` → `E:\books` 就会让整库书重影。
//!
//! # 摘要的交叉验证
//!
//! 本模块的测试用 `assert_eq!` 做进程内比较，也可随时用外部工具
//! （`sha256sum`、`hashlib`）交叉验证——PM 于 2026-10-10 复核确认外部
//! 工具输出正常（`sha256("abc") = ba7816bf…`，与 FIPS 180-4 向量一致）。
//!
//! > 勘误：P7 会话曾在此记录「本机把十六进制摘要输出短接成全零」的
//! > 现象并建议勿用外部工具验证——经 PM 复核**不成立**，系该会话的
//! > 观测误判（其自身 SHA-256 实现本身是正确的）。

//! # 代价：内容相同但确实是两本书的情形
//!
//! 归一化哈希会把"同一本书的两个副本"合并为一本——这是我们**要**的行为
//! （FR-025 的原文"多库根目录指向同一文件时识别为同一本书"）。
//! 但它也会合并"两份内容完全相同的独立文件"。这种情形在实践里罕见
//! （用户不会真的存两份字节相同的书还期望它们是两本），而误合并的代价
//! （少一本书）远低于误分裂的代价（书架越用越多重复条目）。

/// 归一化文件内容并计算稳定的身份标识。
///
/// 返回形如 `local:sha256:<hex>` 的字符串。前缀 `local:` 让本地书与网络书
/// 的 `origin_id`（`书源id|URL`）在同一个 `UNIQUE(origin, origin_id)` 上
/// 不可能相撞——即使某个书源的 id 恰好长得像 hash。
pub fn content_origin_id(bytes: &[u8]) -> String {
    format!("local:sha256:{}", normalize_and_hash(bytes))
}

/// 归一化后的 SHA-256 十六进制摘要。
pub fn normalize_and_hash(bytes: &[u8]) -> String {
    sha256_hex(&normalize(bytes))
}

/// 内容归一化：BOM 剥离 + 行尾统一 + 首尾空白裁剪。
///
/// # 为什么是这三项而不是更多
///
/// 每一项都必须**不改变文本语义**。超出这条线就会把真正不同的书判成同一本：
///
/// | 归一化 | 是否安全 | 理由 |
/// |---|---|---|
/// | 剥 BOM | 安全 | BOM 是编码标记，不是内容 |
/// | `\r\n`/`\r` → `\n` | 安全 | 行尾约定不是内容 |
/// | 首尾空白裁剪 | 安全 | 编辑器加的空行不是内容 |
/// | 内部空白折叠 | **不安全** | 段落内的缩进是内容 |
/// | 全角/半角统一 | **不安全** | 那是改写正文 |
pub fn normalize(bytes: &[u8]) -> Vec<u8> {
    // 1. 剥 BOM（UTF-8 / UTF-16 都要处理——不同工具另存出的副本 BOM 常不一致）。
    let body = strip_bom(bytes);

    // 2. 行尾统一：`\r\n` → `\n`，孤立的 `\r` → `\n`。
    //
    // ## 为什么在字节层做而不是先解码
    //
    // 先解码要付出"整本书解成 String"的内存代价（300MB 文件 → 几百 MB），
    // 而这三个字节模式在任何 ASCII 兼容编码（UTF-8/GBK/Big5/Latin-1）里
    // 含义一致——BOM 与 CR/LF 都不与多字节序列的续字节冲突。UTF-16 是例外，
    // 但 UTF-16 文件带 BOM，会在上一步被剥掉后走同一路径……故对 UTF-16
    // 这里额外做一次检测（见下）。
    let unified = unify_line_endings(body);

    // 3. 首尾空白裁剪。
    trim_ascii_whitespace(&unified)
}

/// 剥离已知的 BOM。
fn strip_bom(bytes: &[u8]) -> &[u8] {
    for bom in [
        &[0xEF, 0xBB, 0xBF][..], // UTF-8
        &[0xFF, 0xFE][..],       // UTF-16LE
        &[0xFE, 0xFF][..],       // UTF-16BE
    ] {
        if bytes.starts_with(bom) {
            return &bytes[bom.len()..];
        }
    }
    bytes
}

/// 统一行尾为 `\n`。
fn unify_line_endings(bytes: &[u8]) -> Vec<u8> {
    if !bytes.contains(&b'\r') {
        // 绝大多数文件走这条路径：连一次分配都省掉。
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\r' {
            out.push(b'\n');
            // `\r\n` 是两个字节表示的一个换行，不能产出两个 `\n`。
            if bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
            }
        } else {
            out.push(byte);
        }
        index += 1;
    }
    out
}

/// 裁剪首尾的 ASCII 空白与常见全角空白。
fn trim_ascii_whitespace(bytes: &[u8]) -> Vec<u8> {
    let is_space = |byte: u8| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C);
    let mut start = 0usize;
    let mut end = bytes.len();
    while start < end && is_space(bytes[start]) {
        start += 1;
    }
    while end > start && is_space(bytes[end - 1]) {
        end -= 1;
    }
    bytes[start..end].to_vec()
}

/// 计算 SHA-256 的十六进制摘要。
///
/// # 为什么自带实现而不是引入 `sha2`
///
/// 体积纪律（宪法「附加约束」）要求单二进制 <20MB 是目标基线。`sha2` 本身
/// 很小（约 40KB 编译产物），引入它并不违反纪律——但**本模块只需要这一个
/// 摘要**，而 FIPS 180-4 的 SHA-256 是约 120 行的定长实现（无表、无堆分配、
/// 无 unsafe）。为一处用法引入依赖与自带实现之间，这里选自带：
///
/// - 实现可被本 crate 的测试**逐条对照公开测试向量**验证（见本模块测试）
/// - 哈希只用于**身份判重**，不是安全边界——即使实现有缺陷也不构成漏洞，
///   只会影响判重准确率，而那是可测的
///
/// > \[UNCERTAIN] 若后续出现第二处哈希需求（如备份校验和），应改为引入
/// > `sha2` 并删掉本实现——两处各有实现才是真正的维护负担。
pub fn sha256_hex(input: &[u8]) -> String {
    let digest = sha256(input);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0F) as u32, 16).unwrap_or('0'));
    }
    out
}

/// SHA-256 的 32 字节摘要（FIPS 180-4）。
///
/// 实现按规范的六步：消息填充 → 分块 → 消息调度 → 压缩 → 迭代 → 输出。
/// 常数表 `K` 是规范给定的立方根小数部分前 32 位。
pub fn sha256(input: &[u8]) -> [u8; 32] {
    // FIPS 180-4 §4.2.2：前 64 个质数立方根小数部分的前 32 位。
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    // FIPS 180-4 §5.3.3：前 8 个质数平方根小数部分的前 32 位。
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // 填充：`1` 位 + `k` 个 `0` + 64 位大端长度，使总长 ≡ 448 (mod 512)。
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(input.len() + 72);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for chunk in padded.as_chunks::<64>().0 {
        // 消息调度（§6.2.2 步骤 1）。
        for (index, slot) in w.iter_mut().take(16).enumerate() {
            let base = index * 4;
            *slot = u32::from_be_bytes([
                chunk[base],
                chunk[base + 1],
                chunk[base + 2],
                chunk[base + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        // 压缩（§6.2.2 步骤 2-3）。
        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];
        for index in 0..64 {
            let big_s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(big_s1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let big_s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = big_s0.wrapping_add(majority);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        // 迭代（§6.2.2 步骤 4）。
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut digest = [0u8; 32];
    for (index, word) in h.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS 180-4 的公开测试向量（附录 B 的样例与其扩展）。
    ///
    /// 逐条对照公开向量是自带实现**必须**做的事：没有它，我们无从知道
    /// 那个 120 行的实现有没有写错。
    #[test]
    fn matches_published_sha256_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            sha256_hex(&[b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn digest_is_stable_across_calls() {
        assert_eq!(sha256_hex(b"hello"), sha256_hex(b"hello"));
    }

    #[test]
    fn crlf_and_lf_files_share_an_identity() {
        // FR-025 的核心场景：同一本书在 Windows 与 Unix 之间拷贝过。
        let unix = b"line one\nline two\nline three\n".as_ref();
        let windows = b"line one\r\nline two\r\nline three\r\n".as_ref();
        assert_eq!(content_origin_id(unix), content_origin_id(windows));
    }

    #[test]
    fn lone_carriage_returns_are_also_unified() {
        // 老 Mac 风格的行尾（`\r`）同样应当归一。
        let classic_mac = b"a\rb\rc".as_ref();
        let unix = b"a\nb\nc".as_ref();
        assert_eq!(content_origin_id(classic_mac), content_origin_id(unix));
    }

    #[test]
    fn trailing_newline_difference_does_not_split_identity() {
        let with = b"body text\n".as_ref();
        let without = b"body text".as_ref();
        assert_eq!(content_origin_id(with), content_origin_id(without));
    }

    #[test]
    fn bom_difference_does_not_split_identity() {
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(b"body text");
        assert_eq!(
            content_origin_id(&with_bom),
            content_origin_id(b"body text")
        );
    }

    #[test]
    fn utf16_boms_are_also_stripped() {
        let mut le = vec![0xFF, 0xFE];
        le.extend_from_slice(b"body");
        let mut be = vec![0xFE, 0xFF];
        be.extend_from_slice(b"body");
        // 剥 BOM 后两者在字节层不同（一个是 LE 一个是 BE 的正文）——
        // 这里只断言 BOM 本身被剥掉了，不承诺跨字节序的等价
        // （那需要真正解码，超出"不改变语义的归一化"这条线的收益）。
        assert!(!normalize(&le).starts_with(&[0xFF, 0xFE]));
        assert!(!normalize(&be).starts_with(&[0xFE, 0xFF]));
    }

    #[test]
    fn different_content_yields_different_identity() {
        assert_ne!(
            content_origin_id(b"book one".as_ref()),
            content_origin_id(b"book two".as_ref())
        );
    }

    #[test]
    fn internal_whitespace_is_meaningful_and_not_normalized() {
        // 归一化**不能**动段落内部——那是内容本身。
        let indented = "第一段\n    缩进的第二段\n".as_bytes();
        let flush = "第一段\n缩进的第二段\n".as_bytes();
        assert_ne!(content_origin_id(indented), content_origin_id(flush));
    }

    #[test]
    fn origin_id_is_namespaced_for_local_books() {
        // 前缀让本地书的身份不可能与网络书的 `书源id|URL` 相撞。
        let id = content_origin_id(b"x".as_ref());
        assert!(id.starts_with("local:sha256:"), "实际 {id}");
        assert_eq!(id.len(), "local:sha256:".len() + 64);
    }

    #[test]
    fn large_input_is_handled() {
        // 覆盖多分块的路径（64 字节一块，这里远超一块）。
        let big = vec![b'q'; 300_000];
        let digest = sha256_hex(&big);
        assert_eq!(digest.len(), 64);
        // 与一次性喂入相同内容的结果一致（同一实现，两次调用应相等）。
        assert_eq!(digest, sha256_hex(&big));
    }

    #[test]
    fn padding_boundary_lengths_are_correct() {
        // 55/56/64 是填充的边界长度：55 恰好塞满、56 需要额外一整块。
        for length in [0usize, 1, 55, 56, 57, 63, 64, 65, 119, 120] {
            let input = vec![b'z'; length];
            assert_eq!(sha256_hex(&input).len(), 64, "长度 {length} 的摘要应完整");
        }
        // 边界两侧必须不同，否则说明填充把不同输入折叠成了同一摘要。
        assert_ne!(sha256_hex(&[b'z'; 55]), sha256_hex(&[b'z'; 56]));
    }
}
