//! [DONE] PalmDOC 头与 MOBI 头（tasks.md T078 / research R4）
//!
//! # 依据的公开规范
//!
//! MobileRead Wiki 的 **MOBI** 词条给出 PalmDOC 头（0x00–0x10B 全表）与
//! MOBI 头各字段的偏移语义。本模块按该表逐字段实现，**不读任何上游
//! Kotlin 实现**（干净室纪律）。
//!
//! # 两层头的分工
//!
//! ```text
//!   记录 0 的布局：
//!   ┌──────────────────────────────┐
//!   │ PalmDOC 头（16 字节，0x00-0x0F）│  ← 压缩算法、文本长度、记录数
//!   ├──────────────────────────────┤
//!   │ 填充 2 字节（0x10-0x11）        │
//!   ├──────────────────────────────┤
//!   │ MOBI 魔数 `MOBI`（0x10-0x13）   │  ← 注意与上一行重叠的常见误解
//!   ├──────────────────────────────┤
//!   │ MOBI 头（0x14 起，长度可变）     │  ← 编码、firstNonBook、EXTH 标志
//!   ├──────────────────────────────┤
//!   │ EXTH 块（可选）                 │  ← 书名、作者等元数据
//!   └──────────────────────────────┘
//! ```
//!
//! ## 关于 `MOBI` 魔数的偏移
//!
//! MobileRead 的表把 PalmDOC 头列为 16 字节、接着 2 字节填充、`MOBI` 魔数
//! 从 **0x10** 开始。实测（本仓库 `.tome-mobi/` 下的真实 Gutenberg 文件）
//! 也是 `MOBI` 在记录 0 的偏移 16 处。故实现按 **0x10** 读取魔数，
//! MOBI 头本体从 **0x14**（魔数之后）开始，其长度由 0x14 处的 4 字节给出。
//!
//! # 什么必须严格校验
//!
//! 记录 0 来自不可信文件，其长度可能只有 10 字节。故每读一个字段前都要
//! 确认它在记录 0 的范围内——越界就报 [`LocalError::Damaged`]，而不是
//! 让切片 panic。

use crate::error::{LocalError, LocalResult};

/// 记录 0 里 `MOBI` 魔数的偏移。
pub const MOBI_MAGIC_OFFSET: usize = 16;

/// MOBI 头本体（魔数之后）的起始偏移。
pub const MOBI_HEADER_OFFSET: usize = 20;

/// MOBI 头的最小长度（读出 0x00-0x10B 所需的字节数）。
///
/// 字段表延伸到 0x10B，而 MOBI 头本体从 0x14 开始，故需要至少
/// `0x10B - 0x14 + 4 = 0xFF` 字节。
const MIN_MOBI_HEADER_BYTES: usize = 0x110 - MOBI_HEADER_OFFSET;

/// 文本压缩算法（PalmDOC 头的 0x00 字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// 1：不压缩
    None,
    /// 2：PalmDOC 压缩
    PalmDoc,
    /// 17480（`0x4448` = `DH`）：HUFF/CDIC 压缩
    ///
    /// # 为什么这个值值得一个显式变体
    ///
    /// research R4 点名的陷阱：`17480` 常被误判为 DRM。它**不是** DRM，
    /// 只是另一种压缩算法（Kindle 的 `kindlegen -c2` 产出）。
    /// 把它与 DRM 混淆会让用户看到"文件受 DRM 保护"这条完全错误的提示。
    HuffCdic,
    /// 其它未知值
    Unknown(u16),
}

impl Compression {
    /// 从原始码位还原。
    pub fn from_code(code: u16) -> Self {
        match code {
            1 => Self::None,
            2 => Self::PalmDoc,
            17480 => Self::HuffCdic,
            other => Self::Unknown(other),
        }
    }

    /// 原始码位。
    pub fn code(self) -> u16 {
        match self {
            Self::None => 1,
            Self::PalmDoc => 2,
            Self::HuffCdic => 17480,
            Self::Unknown(code) => code,
        }
    }
}

/// 文本编码（MOBI 头的 0x1C 字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    /// 1252：Windows-1252（`cp1252`）
    Cp1252,
    /// 65001：UTF-8
    Utf8,
    /// 其它码页
    CodePage(u32),
}

impl TextEncoding {
    /// 从 MOBI 头的编码字段还原。
    pub fn from_code(code: u32) -> Self {
        match code {
            1252 => Self::Cp1252,
            65001 => Self::Utf8,
            other => Self::CodePage(other),
        }
    }

    /// 对应的字符集名（供 `Book.charset`）。
    pub fn charset_name(self) -> String {
        match self {
            Self::Cp1252 => "windows-1252".to_string(),
            Self::Utf8 => "UTF-8".to_string(),
            Self::CodePage(code) => format!("cp{code}"),
        }
    }
}

/// PalmDOC 头 + MOBI 头（记录 0 里读出的全部结构信息）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobiHeader {
    /// 压缩算法（PalmDOC 头 0x00）
    pub compression: Compression,
    /// 未压缩文本的长度（PalmDOC 头 0x04）
    ///
    /// **这是剥离 trailing data 的依据**——见 [`crate::mobi::text`]。
    pub text_length: u32,
    /// 文本记录数（PalmDOC 头 0x08）
    pub record_count: u16,
    /// 每条文本记录的**目标**解压大小（PalmDOC 头 0x0A）
    pub record_size: u16,
    /// 加密类型（PalmDOC 头 0x0C）。0 = 未加密，非 0 = DRM。
    pub encryption_type: u16,
    /// MOBI 头长度（0x14 处的 4 字节）
    pub header_length: u32,
    /// MOBI 类型（0x18）
    pub mobi_type: u32,
    /// 文本编码（0x1C）
    pub encoding: TextEncoding,
    /// 唯一 id（0x20）
    pub unique_id: u32,
    /// 文件版本（0x24）。**8 表示含 KF8 部分**（见 [`Self::is_kf8_capable`]）
    pub file_version: u32,
    /// 正向索引记录号（0x40，`firstNonBookIndex`）
    pub first_non_book_index: u32,
    /// 正式文本的第一个记录号（0x50）。**KF8 用 0xFFFFFFFF 表示"本部分无文本"**
    pub first_content_record: u32,
    /// EXTH 标志（0x70）。`0x40` 位表示存在 EXTH 块
    pub exth_flags: u32,
    /// DRM 偏移（0x98）
    pub drm_offset: u32,
    /// DRM 数量（0x9C）
    pub drm_count: u32,
    /// 记录的起始偏移（0xA0）——用于 trailing data 剥离
    pub record_start_offset: u32,
    /// 原始文件总长度（0xB0）
    pub original_file_length: u32,
    /// KF8 边界记录号（0xC0，`FDST`/双格式的边界）
    pub kf8_boundary: u32,
    /// FDST 记录号（0xC4）
    pub fdst_index: u32,
    /// FDST 数量（0xC8）
    pub fdst_count: u32,
}

impl MobiHeader {
    /// 从记录 0 解析。
    pub fn parse(record0: &[u8]) -> LocalResult<Self> {
        if record0.len() < MOBI_HEADER_OFFSET + 4 {
            return Err(LocalError::damaged(format!(
                "记录 0 太短（{} 字节），无法容纳 PalmDOC 头与 MOBI 魔数",
                record0.len()
            )));
        }
        if &record0[MOBI_MAGIC_OFFSET..MOBI_MAGIC_OFFSET + 4] != b"MOBI" {
            return Err(LocalError::damaged(format!(
                "记录 0 的偏移 {MOBI_MAGIC_OFFSET} 处不是 `MOBI` 魔数（实际 {:?}）",
                String::from_utf8_lossy(&record0[MOBI_MAGIC_OFFSET..MOBI_MAGIC_OFFSET + 4])
            )));
        }

        // PalmDOC 头（0x00-0x0F）。
        let compression = Compression::from_code(read_u16(record0, 0x00)?);
        let text_length = read_u32(record0, 0x04)?;
        let record_count = read_u16(record0, 0x08)?;
        let record_size = read_u16(record0, 0x0A)?;
        let encryption_type = read_u16(record0, 0x0C)?;

        let header_length = read_u32(record0, MOBI_HEADER_OFFSET)?;
        let declared_end = MOBI_HEADER_OFFSET
            .checked_add(header_length as usize)
            .ok_or_else(|| LocalError::damaged("MOBI 头长度溢出".to_string()))?;
        if declared_end > record0.len() {
            return Err(LocalError::damaged(format!(
                "MOBI 头声明长度 {header_length} 超出记录 0 的实际长度 {}",
                record0.len()
            )));
        }
        if (header_length as usize) < MIN_MOBI_HEADER_BYTES {
            // 短头是合法的（MOBI 头可以只写前面的字段），但不能短到读不出
            // 我们需要的字段。此处不报错——缺失的字段按 0 处理即可，
            // 因为它们的默认值（如 `first_content_record = 0xFFFFFFFF`）
            // 已经表达了"没有"的语义。只是要避免越界读。
        }

        // 一个能读多长就读多长的辅助闭包：字段超出声明长度时给默认值。
        let field_limit = declared_end;
        let read_u32_opt = |offset: usize| -> u32 {
            if offset + 4 <= field_limit {
                read_u32(record0, offset).unwrap_or(0)
            } else {
                0
            }
        };

        Ok(Self {
            compression,
            text_length,
            record_count,
            record_size,
            encryption_type,
            header_length,
            mobi_type: read_u32_opt(0x18),
            encoding: TextEncoding::from_code(read_u32_opt(0x1C)),
            unique_id: read_u32_opt(0x20),
            file_version: read_u32_opt(0x24),
            first_non_book_index: read_u32_opt(0x40),
            first_content_record: read_u32_opt(0x50),
            exth_flags: read_u32_opt(0x80),
            drm_offset: read_u32_opt(0x98),
            drm_count: read_u32_opt(0x9C),
            record_start_offset: read_u32_opt(0xA0),
            original_file_length: read_u32_opt(0xB0),
            kf8_boundary: read_u32_opt(0xC0),
            fdst_index: read_u32_opt(0xC4),
            fdst_count: read_u32_opt(0xC8),
        })
    }

    /// EXTH 块是否存在（标志的 `0x40` 位）。
    pub fn has_exth(&self) -> bool {
        self.exth_flags & 0x40 != 0
    }

    /// 文件是否带 KF8 部分（版本 ≥ 8）。
    ///
    /// # 为什么"版本 8"意味着双格式
    ///
    /// Kindle 从 KF8 起采用**双格式容器**：同一个 `.mobi`/`.azw3` 文件里
    /// 放着旧的 MOBI6 部分（供老设备读）与新的 KF8 部分（供新设备读）。
    /// MOBI 头的 `file_version = 8` 是这一形态的标志。
    ///
    /// 故"版本 8 的文件"解析出**两套**头：记录 0 的 MOBI6 头，以及
    /// KF8 边界记录处（`kf8_boundary`）的第二套头。T081 处理的正是后者。
    pub fn is_kf8_capable(&self) -> bool {
        self.file_version >= 8
    }

    /// 本部分是否**没有正文**（KF8 的 MOBI6 半部分是空壳）。
    ///
    /// 双格式文件里，MOBI6 部分常常是"只有几页引导内容"的空壳，
    /// 而真正的正文在 KF8 部分。判据是 `first_content_record = 0xFFFFFFFF`
    /// （MOBI 的约定：该值表示"本部分不含文本"）。
    pub fn has_no_content(&self) -> bool {
        self.first_content_record == 0xFFFF_FFFF
    }

    /// EXTH 块在记录 0 里的起始偏移。
    ///
    /// # 为什么减 4（实测确认）
    ///
    /// `header_length`（0x14 处）**包含**紧随其后的 `EXTH` 魔数的 4 字节。
    /// 实测 `.tome-mobi/368.kf8.mobi`：`header_length = 264`，
    /// `EXTH` 魔数出现在记录 0 的偏移 **280** = `20 + 264 - 4`；若按
    /// `20 + 264 = 284` 去读，拿到的是 EXTH 块的**长度与条数字段**，
    /// 于是魔数比对失败、元数据整块丢失。
    ///
    /// 这与 "MOBI 头长度不含魔数、魔数在 0x10 处" 的布局是同一件事的
    /// 两面：`header_length` 从魔数**之后**开始计量，但它的**结束**位置
    /// 落在 EXTH 魔数之内。
    pub fn exth_offset(&self) -> usize {
        let offset = MOBI_HEADER_OFFSET + self.header_length as usize;
        offset.saturating_sub(4)
    }

    /// 文本记录的起始索引（PalmDOC 头说从 1 开始）。
    pub fn first_text_record(&self) -> usize {
        1
    }

    /// 文本记录的结束索引（不含）。
    ///
    /// # 为什么用 `first_non_book_index` 而不是 `record_count`
    ///
    /// `record_count` 是 PalmDOC 头的字段，语义是"文本记录数"——但它**只是
    /// 个提示**，真实文件里常与实际不符（生成工具写错，或 KF8 双格式里
    /// 只统计了 MOBI6 部分）。而 `first_non_book_index` 是 MOBI 头给出的
    /// "第一条非文本记录"的位置，它才是可靠的边界。
    pub fn text_record_end(&self) -> usize {
        if self.first_non_book_index as usize > 1 {
            self.first_non_book_index as usize
        } else {
            // 没有可用的 `firstNonBook`：退回 PalmDOC 头的记录数。
            1 + self.record_count as usize
        }
    }
}

/// 读大端 u16（越界报错而不是 panic）。
fn read_u16(bytes: &[u8], offset: usize) -> LocalResult<u16> {
    if offset + 2 > bytes.len() {
        return Err(LocalError::damaged(format!(
            "读取 u16 越界：偏移 {offset}，长度 {}",
            bytes.len()
        )));
    }
    Ok(u16::from_be_bytes([bytes[offset], bytes[offset + 1]]))
}

/// 读大端 u32（越界报错而不是 panic）。
fn read_u32(bytes: &[u8], offset: usize) -> LocalResult<u32> {
    if offset + 4 > bytes.len() {
        return Err(LocalError::damaged(format!(
            "读取 u32 越界：偏移 {offset}，长度 {}",
            bytes.len()
        )));
    }
    Ok(u32::from_be_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个记录 0（PalmDOC 头 + MOBI 头，无 EXTH）。
    fn build_record0(header_length: u32, patch: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let total = MOBI_HEADER_OFFSET + header_length as usize;
        let mut bytes = vec![0u8; total];
        // PalmDOC 头
        bytes[0x00..0x02].copy_from_slice(&2u16.to_be_bytes()); // PalmDOC 压缩
        bytes[0x04..0x08].copy_from_slice(&1000u32.to_be_bytes()); // 文本长度
        bytes[0x08..0x0A].copy_from_slice(&5u16.to_be_bytes()); // 记录数
        bytes[0x0A..0x0C].copy_from_slice(&4096u16.to_be_bytes()); // 记录大小
        bytes[0x0C..0x0E].copy_from_slice(&0u16.to_be_bytes()); // 未加密
                                                                // MOBI 魔数与头长度
        bytes[MOBI_MAGIC_OFFSET..MOBI_MAGIC_OFFSET + 4].copy_from_slice(b"MOBI");
        bytes[MOBI_HEADER_OFFSET..MOBI_HEADER_OFFSET + 4]
            .copy_from_slice(&header_length.to_be_bytes());
        patch(&mut bytes);
        bytes
    }

    #[test]
    fn parses_the_palmdoc_header() {
        let record0 = build_record0(232, |_| {});
        let header = MobiHeader::parse(&record0).expect("应解析成功");
        assert_eq!(header.compression, Compression::PalmDoc);
        assert_eq!(header.text_length, 1000);
        assert_eq!(header.record_count, 5);
        assert_eq!(header.record_size, 4096);
        assert_eq!(header.encryption_type, 0);
    }

    #[test]
    fn huffcdic_compression_is_not_unknown() {
        // R4 点名的陷阱：17480 必须被识别为 HUFF/CDIC 而非 DRM 或"未知"。
        assert_eq!(Compression::from_code(17480), Compression::HuffCdic);
        assert_eq!(Compression::HuffCdic.code(), 17480);
    }

    #[test]
    fn compression_codes_round_trip() {
        for code in [1u16, 2, 17480, 99] {
            assert_eq!(Compression::from_code(code).code(), code);
        }
    }

    #[test]
    fn missing_mobi_magic_is_rejected() {
        let mut record0 = build_record0(232, |_| {});
        record0[MOBI_MAGIC_OFFSET..MOBI_MAGIC_OFFSET + 4].copy_from_slice(b"XXXX");
        assert!(matches!(
            MobiHeader::parse(&record0),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn short_record0_is_rejected() {
        let bytes = vec![0u8; 10];
        assert!(matches!(
            MobiHeader::parse(&bytes),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn header_length_beyond_record_is_rejected() {
        // 声明的头长度超过记录 0 的实际长度 → 损坏。
        let mut record0 = build_record0(64, |_| {});
        record0[MOBI_HEADER_OFFSET..MOBI_HEADER_OFFSET + 4].copy_from_slice(&4096u32.to_be_bytes());
        assert!(matches!(
            MobiHeader::parse(&record0),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn utf8_encoding_is_recognised() {
        let record0 = build_record0(232, |bytes| {
            bytes[0x1C..0x20].copy_from_slice(&65001u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.encoding, TextEncoding::Utf8);
        assert_eq!(header.encoding.charset_name(), "UTF-8");
    }

    #[test]
    fn cp1252_encoding_is_recognised() {
        let record0 = build_record0(232, |bytes| {
            bytes[0x1C..0x20].copy_from_slice(&1252u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.encoding, TextEncoding::Cp1252);
        assert_eq!(header.encoding.charset_name(), "windows-1252");
    }

    #[test]
    fn other_code_pages_are_preserved() {
        let encoding = TextEncoding::from_code(936);
        assert_eq!(encoding, TextEncoding::CodePage(936));
        assert_eq!(encoding.charset_name(), "cp936");
    }

    #[test]
    fn file_version_8_marks_a_dual_format_container() {
        let record0 = build_record0(232, |bytes| {
            bytes[0x24..0x28].copy_from_slice(&8u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.file_version, 8);
        assert!(header.is_kf8_capable(), "版本 8 意味着含 KF8 部分");
    }

    #[test]
    fn older_versions_are_not_kf8() {
        let record0 = build_record0(232, |bytes| {
            bytes[0x24..0x28].copy_from_slice(&6u32.to_be_bytes());
        });
        assert!(!MobiHeader::parse(&record0)
            .expect("应解析")
            .is_kf8_capable());
    }

    #[test]
    fn first_non_book_index_defines_the_text_record_range() {
        let record0 = build_record0(232, |bytes| {
            bytes[0x08..0x0A].copy_from_slice(&5u16.to_be_bytes());
            bytes[0x40..0x44].copy_from_slice(&83u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.first_text_record(), 1);
        assert_eq!(header.text_record_end(), 83);
    }

    #[test]
    fn text_record_range_falls_back_to_the_palmdoc_count() {
        // `firstNonBook` 缺失（0）时应退回 PalmDOC 头的记录数。
        let record0 = build_record0(232, |bytes| {
            bytes[0x08..0x0A].copy_from_slice(&5u16.to_be_bytes());
            bytes[0x40..0x44].copy_from_slice(&0u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.text_record_end(), 6);
    }

    #[test]
    fn exth_flag_is_read() {
        // EXTH 标志在 0x80（实测确认；0x70 处是 0）。
        let record0 = build_record0(232, |bytes| {
            bytes[0x80..0x84].copy_from_slice(&0x50u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert!(header.has_exth(), "0x50 含 0x40 位");
    }

    #[test]
    fn exth_absent_when_the_flag_bit_is_clear() {
        let record0 = build_record0(232, |bytes| {
            bytes[0x80..0x84].copy_from_slice(&0x10u32.to_be_bytes());
        });
        assert!(!MobiHeader::parse(&record0).expect("应解析").has_exth());
    }

    #[test]
    fn exth_absent_when_the_whole_flag_word_is_zero() {
        // 实测的 368.kf8.mobi 在 0x70 处确实是 0——首版读错偏移，
        // 于是把"有 EXTH"误判为"没有"，元数据整块丢失。
        let record0 = build_record0(232, |bytes| {
            bytes[0x70..0x74].copy_from_slice(&0u32.to_be_bytes());
            bytes[0x80..0x84].copy_from_slice(&0x50u32.to_be_bytes());
        });
        assert!(
            MobiHeader::parse(&record0).expect("应解析").has_exth(),
            "0x70 为 0 不代表没有 EXTH——标志在 0x80"
        );
    }

    #[test]
    fn no_content_marker_is_recognised() {
        let record0 = build_record0(232, |bytes| {
            bytes[0x50..0x54].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        });
        assert!(MobiHeader::parse(&record0)
            .expect("应解析")
            .has_no_content());
    }

    #[test]
    fn kf8_fields_are_read() {
        let record0 = build_record0(264, |bytes| {
            bytes[0xC0..0xC4].copy_from_slice(&97u32.to_be_bytes());
            bytes[0xC4..0xC8].copy_from_slice(&96u32.to_be_bytes());
            bytes[0xC8..0xCC].copy_from_slice(&5u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.kf8_boundary, 97);
        assert_eq!(header.fdst_index, 96);
        assert_eq!(header.fdst_count, 5);
    }

    #[test]
    fn fields_beyond_a_short_header_default_to_zero() {
        // MOBI 头可以很短（只写前面的字段）——超出声明长度的字段读作 0，
        // 而不是越界读或报错。
        let record0 = build_record0(64, |bytes| {
            // 只有 64 字节的头，写不进 0xC0 处的 KF8 字段。
            bytes[0x18..0x1C].copy_from_slice(&2u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.mobi_type, 2);
        assert_eq!(header.kf8_boundary, 0, "头之外的字段应为默认值");
        assert_eq!(header.fdst_count, 0);
    }

    #[test]
    fn exth_offset_sits_four_bytes_before_the_header_length_ends() {
        // `header_length` **包含** EXTH 魔数的 4 字节，故 EXTH 从这个
        // 长度的**倒数第 4 字节**开始（实测 368：264 → 偏移 280）。
        let record0 = build_record0(232, |_| {});
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.exth_offset(), MOBI_HEADER_OFFSET + 232 - 4);
    }

    #[test]
    fn drm_offset_and_count_are_read() {
        let record0 = build_record0(232, |bytes| {
            bytes[0x98..0x9C].copy_from_slice(&1234u32.to_be_bytes());
            bytes[0x9C..0xA0].copy_from_slice(&2u32.to_be_bytes());
        });
        let header = MobiHeader::parse(&record0).expect("应解析");
        assert_eq!(header.drm_offset, 1234);
        assert_eq!(header.drm_count, 2);
    }
}
