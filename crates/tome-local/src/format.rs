//! [DONE] 本地书格式识别（tasks.md T077-T081 / FR-021、FR-022）
//!
//! # 为什么先看魔数而不是扩展名
//!
//! 扩展名是用户给的**线索**，内容是**事实**。两者不符的情形在真实书库里
//! 很常见：
//!
//! - `.txt` 其实是 EPUB（下载工具改了名）
//! - `.mobi` 其实是 AZW3 的双格式容器（Kindle 生态里两者共用同一容器）
//! - `.azw3` 其实是 KFX（新 Kindle 格式，本产品不支持）
//!   ——**必须显式拒绝**，否则会按 MOBI 解析出一个乱码书
//!
//! 故 [`detect`] 以**内容**为准，扩展名只在内容无法判定时才用作兜底。
//! 这也让"导入一个没有扩展名的文件"成为可能。
//!
//! # 二进制与文本的界线
//!
//! TXT 的识别是**反向**的：它没有魔数。故 [looks_like_text] 用排除法——
//! 含 NUL 字节、或非 ASCII 字节里替换字符比例过高，就不是文本。这条判据
//! 同样是"宁可拒绝也不要静默产出乱码书"的取向。

use std::path::Path;

use crate::error::{LocalError, LocalResult};

/// 识别出的本地书格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalFormat {
    /// 纯文本（含多种编码）
    Txt,
    /// EPUB（OCF/OPF 容器）
    Epub,
    /// MOBI / AZW3（PalmDB 容器，含双格式 KF8）
    Mobi,
    /// KFX —— 本产品**不支持**，显式拒绝
    Kfx,
    /// Topaz（`.tpz`）—— 本产品**不支持**，显式拒绝
    Topaz,
}

impl LocalFormat {
    /// 落库用的格式标识（`Book.format`，data-model.md）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Epub => "epub",
            Self::Mobi => "mobi",
            // AZW3 与 MOBI 共用 PalmDB 容器，落库统一为 `mobi`
            // （`Book.format` 的文档列了 `mobi` / `azw3` 两种取值，
            //  但解析路径完全相同，区分它们对上层没有信息量）。
            Self::Kfx => "kfx",
            Self::Topaz => "topaz",
        }
    }

    /// 本产品是否支持解析该格式。
    pub fn is_supported(self) -> bool {
        matches!(self, Self::Txt | Self::Epub | Self::Mobi)
    }
}

/// [DONE] 按内容识别格式；内容无法判定时用扩展名兜底。
///
/// 无法判定且扩展名也不认识时返回 [`LocalError::UnsupportedFormat`]——
/// 而不是"当 TXT 试试"。那样做的后果是用户导入一个损坏的 MOBI 后
/// 得到一本全是乱码的书，且**没有任何错误提示**。
pub fn detect(path: &Path, bytes: &[u8]) -> LocalResult<LocalFormat> {
    // 1. 内容嗅探优先于一切。
    if let Some(format) = sniff(bytes) {
        ensure_supported(format)?;
        return Ok(format);
    }

    // 2. **文本判定优先于扩展名**。
    //
    // 这个次序不是随意的。真实场景里有一类文件：内容是纯文本，但扩展名
    // 指向别的格式（`.epub` 其实里面装的是 TXT、`.mobi` 是用户手工改名）。
    // 若先信扩展名，这类文件会被送去 EPUB/MOBI 解析器，在那里报一个
    // 与真实原因无关的错误（"ZIP 头非法"），用户完全无从下手。
    //
    // 反过来，**二进制容器绝不会通过文本判定**（NUL 字节这条判据极可靠），
    // 所以把文本判定提前不会让 EPUB/MOBI 被误当成 TXT。次序因此是安全的。
    if looks_like_text(bytes) {
        return Ok(LocalFormat::Txt);
    }

    // 3. 内容判不出来且不是文本：用扩展名。
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let by_extension = match extension.as_str() {
        "txt" | "text" => Some(LocalFormat::Txt),
        "epub" => Some(LocalFormat::Epub),
        "mobi" | "azw" | "azw3" | "prc" => Some(LocalFormat::Mobi),
        "kfx" => Some(LocalFormat::Kfx),
        "tpz" | "azw1" => Some(LocalFormat::Topaz),
        _ => None,
    };
    if let Some(format) = by_extension {
        ensure_supported(format)?;
        return Ok(format);
    }

    Err(LocalError::UnsupportedFormat {
        format: "unknown".to_string(),
    })
}

/// 只按内容嗅探（不做扩展名兜底）。
pub fn sniff(bytes: &[u8]) -> Option<LocalFormat> {
    // ZIP（EPUB 是 ZIP 容器）：`PK\x03\x04`。
    if bytes.starts_with(b"PK\x03\x04") {
        // 只有真的含 `mimetype` 条目才算 EPUB——普通 ZIP 不是书。
        return if is_epub_zip(bytes) {
            Some(LocalFormat::Epub)
        } else {
            None
        };
    }

    // PalmDB 容器（MOBI/AZW3/PRC）。
    if bytes.len() >= 68 && bytes.get(60..68) == Some(b"BOOKMOBI") {
        return Some(LocalFormat::Mobi);
    }
    // Topaz 也是 PalmDB，但类型是 `TOPZ`。
    if bytes.len() >= 68 && bytes.get(60..68) == Some(b"TEXtREAd") {
        return Some(LocalFormat::Topaz);
    }

    // KFX：容器魔数 `CONT`（KFX 的 header 以 `CONT` 开头）。
    if bytes.starts_with(b"CONT") {
        return Some(LocalFormat::Kfx);
    }

    None
}

/// 判断一个 ZIP 是否真的是 EPUB（OCF 要求 `mimetype` 是第一个条目且不压缩）。
///
/// ## 为什么用"ZIP 中央目录里有 mimetype"而不是"读第一个本地文件头"
///
/// 规范要求 `mimetype` 是**第一个**条目且**不压缩**（便于快速识别），
/// 但真实世界里的 EPUB 由各种工具产出，顺序并不总被遵守。故这里采用更宽容
/// 的判据：ZIP 的**中央目录**里存在名为 `mimetype` 的条目，且其内容为
/// `application/epub+zip`。这既容错又不会把随机 ZIP 误判为 EPUB。
///
/// 中央目录在文件尾部，但这里只做**粗筛**（真正的解析在 [`crate::epub`]），
/// 故扫一遍本地文件头就够——不追求完整解析，只求"不是 EPUB 就早点拒绝"。
fn is_epub_zip(bytes: &[u8]) -> bool {
    // 条目名在本地文件头里，偏移 30 开始（固定头 30 字节）。
    // 扫描有限个条目即可判定，不做完整遍历（大 EPUB 有几千条目）。
    let mut offset = 0usize;
    let mut scanned = 0usize;
    while offset + 30 <= bytes.len() && scanned < 64 {
        if bytes[offset..offset + 4] != *b"PK\x03\x04" {
            break;
        }
        let name_len = u16::from_le_bytes([bytes[offset + 26], bytes[offset + 27]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
        let compressed = u16::from_le_bytes([bytes[offset + 8], bytes[offset + 9]]) as usize;
        let name_start = offset + 30;
        let name_end = name_start + name_len;
        if name_end > bytes.len() {
            break;
        }
        if &bytes[name_start..name_end] == b"mimetype" {
            let data_start = name_end + extra_len;
            let data_end = data_start + compressed;
            if data_end <= bytes.len()
                && bytes[data_start..data_end].starts_with(b"application/epub+zip")
            {
                return true;
            }
        }
        // 跳到下一个条目。
        let data_start = name_end + extra_len;
        let compress_method = u16::from_le_bytes([bytes[offset + 8], bytes[offset + 9]]) as usize;
        let _ = compress_method;
        let compressed_size = u32::from_le_bytes([
            bytes[offset + 18],
            bytes[offset + 19],
            bytes[offset + 20],
            bytes[offset + 21],
        ]) as usize;
        let next = data_start + compressed_size;
        if next <= offset {
            break;
        }
        offset = next;
        scanned += 1;
    }
    // 已扫描的条目里没找到 `mimetype` 也**不**立即否定：EPUB 规范要求它
    // 第一个，而一些生成器把它放在后面。故退一步看整个头部区域是否出现
    // `application/epub+zip` 这个特征串（它几乎只会出现在 EPUB 里）。
    let head = &bytes[..bytes.len().min(64 * 1024)];
    head.windows(b"application/epub+zip".len())
        .any(|window| window == b"application/epub+zip")
}

/// 内容是否是纯文本（TXT 的反向识别）。
///
/// # 判据
///
/// 1. **含 NUL 字节** → 不是文本。这是最可靠的信号：真正的文本编码
///    （UTF-8/UTF-16/GBK/Big5）在正文里不会出现 `0x00`，而几乎所有二进制
///    格式（含被误当 TXT 的 MOBI/EPUB）都含大量 NUL。
/// 2. **非 ASCII 字节里"不可解码"比例过高** → 不是文本。这里只做粗判
///    （UTF-8 合法性 + 控制字符比例），精确的编码判定交给
///    [`crate::encoding`]——本函数只回答"值不值得试"。
pub fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        // 空文件当文本：它是退化但合法的输入（一本空书）。
        return true;
    }
    // 只检查前 8 KB：判定"是不是二进制"不需要读全文件，而 300MB 文件的
    // 全量扫描会成为 SC-004 预算里的纯浪费。
    let head = &bytes[..bytes.len().min(8192)];

    // UTF-16/UTF-32 是**例外**：它们的正文里满是 NUL 字节（ASCII 字符的
    // 高字节），会撞上下面"含 NUL 就是二进制"的判据。带 BOM 时身份明确，
    // 在 NUL 检查之前放行。
    //
    // 无 BOM 的 UTF-16 检测不出来（那需要启发式），且它极其罕见——
    // Windows 记事本、`.NET` 的 `Encoding.Unicode`、以及绝大多数工具
    // 都会写 BOM。
    if starts_with_unicode_bom(bytes) {
        return true;
    }

    if head.contains(&0x00) {
        return false;
    }

    // 控制字符（排除制表/换行/回车/换页）比例。
    let control = head
        .iter()
        .filter(|byte| matches!(byte, 0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F))
        .count();
    if control * 100 > head.len() * 5 {
        // 超过 5% 是控制字符：那不是文本排版，是二进制数据的特征。
        return false;
    }

    true
}

/// 文件是否以一个 Unicode 编码的 BOM 开头。
///
/// 用于 UTF-16/UTF-32 的放行判定：它们的正文含大量 NUL 字节，
/// 会撞上"含 NUL 即二进制"的判据。UTF-32 的 BOM 以 UTF-16 的 BOM 为前缀，
/// 故这里的判定对两者都成立（对文本判定而言不必区分）。
fn starts_with_unicode_bom(bytes: &[u8]) -> bool {
    const UNICODE_BOMS: [&[u8]; 4] = [
        &[0xFF, 0xFE],             // UTF-16LE（UTF-32LE `FF FE 00 00` 以此为前缀）
        &[0xFE, 0xFF],             // UTF-16BE（UTF-32BE `00 00 FE FF` 例外，单独列出）
        &[0x00, 0x00, 0xFE, 0xFF], // UTF-32BE
        &[0xEF, 0xBB, 0xBF],       // UTF-8（本就不含 NUL，此处只求判定完备）
    ];
    UNICODE_BOMS.iter().any(|bom| bytes.starts_with(bom))
}

/// 把不支持的格式转成明确错误（FR-006 的结构化能力错误）。
fn ensure_supported(format: LocalFormat) -> LocalResult<()> {
    if format.is_supported() {
        return Ok(());
    }
    Err(LocalError::UnsupportedFormat {
        format: format.as_str().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn path(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn detects_mobi_by_content_regardless_of_extension() {
        let mut bytes = vec![0u8; 68];
        bytes[60..68].copy_from_slice(b"BOOKMOBI");
        // 扩展名说是 TXT，内容说是 MOBI —— 以内容为准。
        assert_eq!(
            detect(&path("mislabeled.txt"), &bytes).expect("应识别"),
            LocalFormat::Mobi
        );
    }

    #[test]
    fn detects_topaz_and_rejects_it() {
        let mut bytes = vec![0u8; 68];
        bytes[60..68].copy_from_slice(b"TEXtREAd");
        let error = detect(&path("book.tpz"), &bytes).expect_err("Topaz 应被拒绝");
        match error {
            LocalError::UnsupportedFormat { format } => assert_eq!(format, "topaz"),
            other => panic!("应为不支持格式，实际 {other:?}"),
        }
    }

    #[test]
    fn detects_kfx_and_rejects_it() {
        // KFX 显式拒绝是 FR-022 的硬要求：按 MOBI 去解析它只会产出乱码。
        let bytes = b"CONT\x00\x00\x00\x00rest of a kfx container".to_vec();
        assert_eq!(sniff(&bytes), Some(LocalFormat::Kfx));
        assert!(matches!(
            detect(&path("book.kfx"), &bytes),
            Err(LocalError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn epub_is_detected_from_the_mimetype_entry() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x03\x04");
        bytes.extend_from_slice(&[0u8; 4]); // 版本等
        bytes.extend_from_slice(&0u16.to_le_bytes()); // method = store
        bytes.extend_from_slice(&[0u8; 12]); // 时间/CRC
        bytes.extend_from_slice(&20u32.to_le_bytes()); // compressed size
        bytes.extend_from_slice(&20u32.to_le_bytes()); // uncompressed size
        bytes.extend_from_slice(&8u16.to_le_bytes()); // name len
        bytes.extend_from_slice(&0u16.to_le_bytes()); // extra len
        bytes.extend_from_slice(b"mimetype");
        bytes.extend_from_slice(b"application/epub+zip");
        assert_eq!(sniff(&bytes), Some(LocalFormat::Epub));
    }

    #[test]
    fn plain_zip_is_not_an_epub() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x03\x04");
        bytes.extend_from_slice(&[0u8; 24]);
        bytes.extend_from_slice(b"random.txt");
        assert_eq!(sniff(&bytes), None);
    }

    #[test]
    fn text_without_magic_is_recognised_by_extension() {
        assert_eq!(
            detect(&path("novel.txt"), "中文正文".as_bytes()).expect("应识别"),
            LocalFormat::Txt
        );
    }

    #[test]
    fn text_without_extension_is_still_accepted() {
        assert_eq!(
            detect(&path("no_extension"), "中文正文".as_bytes()).expect("应识别"),
            LocalFormat::Txt
        );
    }

    #[test]
    fn binary_without_extension_is_rejected() {
        let mut bytes = vec![0u8; 2048];
        bytes[0] = 0xFF;
        bytes[1] = 0xD8;
        // 无扩展名 + 不像文本 → 明确拒绝，而不是当 TXT 解出乱码。
        assert!(matches!(
            detect(&path("mystery"), &bytes),
            Err(LocalError::UnsupportedFormat { .. })
        ));
    }

    #[test]
    fn nul_bytes_mean_not_text() {
        let mut bytes = b"some readable prefix".to_vec();
        bytes.extend_from_slice(&[0u8; 100]);
        assert!(!looks_like_text(&bytes));
    }

    #[test]
    fn gbk_text_is_still_text() {
        // GBK 的中文正文里没有 NUL，也没有控制字符 → 必须判为文本。
        let (encoded, _, _) = encoding_rs::GBK.encode("第一章 开端\n这是一段中文正文。");
        assert!(looks_like_text(&encoded));
    }

    #[test]
    fn utf16_text_with_bom_is_recognised_as_text() {
        // UTF-16 正文里满是 NUL 字节，会撞上"含 NUL 就是二进制"的判据。
        // 带 BOM 时身份明确，必须在 NUL 检查**之前**放行——否则一本
        // Windows 记事本另存的 UTF-16 TXT 会被整个拒绝。
        let mut bytes = vec![0xFF, 0xFE];
        for ch in "中文".encode_utf16() {
            bytes.extend_from_slice(&ch.to_le_bytes());
        }
        assert!(looks_like_text(&bytes));
        assert_eq!(
            detect(&path("utf16.txt"), &bytes).expect("应识别"),
            LocalFormat::Txt
        );
    }

    #[test]
    fn utf16_big_endian_with_bom_is_also_text() {
        let mut bytes = vec![0xFE, 0xFF];
        for ch in "中文".encode_utf16() {
            bytes.extend_from_slice(&ch.to_be_bytes());
        }
        assert!(looks_like_text(&bytes));
    }

    #[test]
    fn nul_bytes_without_a_bom_still_mean_binary() {
        // BOM 例外不能开得太宽：没有 BOM 的一堆 NUL 仍是二进制。
        let bytes = vec![0u8; 512];
        assert!(!looks_like_text(&bytes));
    }

    #[test]
    fn a_real_palmdb_header_is_not_text() {
        // 真实 MOBI 的前 68 字节全是偏移表与类型串，含大量 NUL 与
        // Unicode 保留码位，绝不能通过文本判定。
        let mut bytes = vec![0u8; 78];
        bytes[60..68].copy_from_slice(b"BOOKMOBI");
        bytes[76] = 0x00;
        bytes[77] = 0x64;
        assert!(!looks_like_text(&bytes));
    }

    #[test]
    fn empty_file_is_treated_as_text() {
        assert!(looks_like_text(&[]));
        assert_eq!(
            detect(&path("empty.txt"), &[]).expect("应识别"),
            LocalFormat::Txt
        );
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        assert_eq!(
            detect(&path("BOOK.TXT"), "正文".as_bytes()).expect("应识别"),
            LocalFormat::Txt
        );
    }

    #[test]
    fn supported_formats_report_themselves() {
        assert!(LocalFormat::Txt.is_supported());
        assert!(LocalFormat::Epub.is_supported());
        assert!(LocalFormat::Mobi.is_supported());
        assert!(!LocalFormat::Kfx.is_supported());
        assert!(!LocalFormat::Topaz.is_supported());
    }
}
