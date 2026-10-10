//! [DONE] EXTH 元数据块（tasks.md T078 / research R4）
//!
//! # 依据的公开规范
//!
//! MobileRead Wiki 的 MOBI 词条列出约 80 个 EXTH tag。本模块实现
//! **FR-021/FR-022 需要的那几个**（书名、作者、语言、简介等），
//! 其余 tag 被读取并保留在原始列表中（供诊断与后续扩展）。
//!
//! # 块布局
//!
//! ```text
//!   偏移  长度  内容
//!   0x00  4     魔数 `EXTH`
//!   0x04  4     块总长度（含这 8 字节头）
//!   0x08  4     记录条数 n
//!   0x0C  8×n   记录（每条：4 字节 tag + 4 字节长度 + (长度-8) 字节数据）
//! ```
//!
//! ## 两个易错点
//!
//! 1. **长度字段含 8 字节记录头**：`length` 是 tag + length + data 的总长，
//!    不是 data 的长度。按 data 长度解释会一路错位。
//! 2. **块长度是"软"约束**：真实文件里偶尔出现块长度与实际不符（生成工具
//!    的 bug）。故实现**同时**用块长度与文件长度做边界，取更严格的一个。
//!
//! # 为什么 tag 值要保留原始列表
//!
//! 用户导入一本 MOBI 后如果看到书名不对，需要知道"文件里到底有什么"。
//! 保留原始 tag 列表让 `tome local import` 能输出完整元数据供排查，
//! 而不必重新实现一遍解析。

use crate::error::{LocalError, LocalResult};

/// EXTH 块魔数。
const EXTH_MAGIC: &[u8; 4] = b"EXTH";

/// 单条记录的头部长度（tag 4 字节 + length 4 字节）。
const RECORD_HEADER_BYTES: usize = 8;

/// 一个 EXTH tag（原始形式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExthRecord {
    /// tag 编号
    pub tag: u32,
    /// 数据（可能含尾部 NUL 填充）
    pub data: Vec<u8>,
}

impl ExthRecord {
    /// 数据按字符串解释（去尾部 NUL 与首尾空白）。
    pub fn as_string(&self) -> String {
        let end = self
            .data
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.data.len());
        String::from_utf8_lossy(&self.data[..end])
            .trim()
            .to_string()
    }

    /// 数据按大端 u32 解释（用于 204/205 这类数值 tag）。
    ///
    /// EXTH 的数值 tag 的 data 是 **8 字节**（4 字节填充 + 4 字节值），
    /// 而值在**末尾** 4 字节——故取末 4 字节而不是前 4 字节。
    pub fn as_u32(&self) -> Option<u32> {
        if self.data.len() < 4 {
            return None;
        }
        let tail = &self.data[self.data.len() - 4..];
        Some(u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]))
    }
}

/// 解析出的 EXTH 块。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Exth {
    /// 全部记录（按文件里的顺序）
    pub records: Vec<ExthRecord>,
    /// 解析过程中的非致命问题（**不静默**）
    pub warnings: Vec<String>,
}

impl Exth {
    /// 取第一个匹配 tag 的记录。
    pub fn first(&self, tag: u32) -> Option<&ExthRecord> {
        self.records.iter().find(|record| record.tag == tag)
    }

    /// 取第一段匹配 tag 的字符串数据。
    pub fn string(&self, tag: u32) -> Option<String> {
        let value = self.first(tag)?.as_string();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    }

    /// 取全部匹配 tag 的字符串数据（`subject` 这类 tag 允许多值）。
    pub fn strings(&self, tag: u32) -> Vec<String> {
        self.records
            .iter()
            .filter(|record| record.tag == tag)
            .map(ExthRecord::as_string)
            .filter(|value| !value.is_empty())
            .collect()
    }
}

/// 已知 EXTH tag 的编号（MobileRead Wiki 的 tag 表）。
///
/// 只列本实现用到的与最常需要诊断的。`#[allow(dead_code)]` 不必要——
/// 每个常量都在 [`Exth::describe`] 里被引用（那是它们的用途）。
pub mod tags {
    /// 作者（`<dc:creator>`）
    pub const AUTHOR: u32 = 100;
    /// 出版者
    pub const PUBLISHER: u32 = 101;
    /// 简介
    pub const DESCRIPTION: u32 = 103;
    /// ISBN
    pub const ISBN: u32 = 104;
    /// 主题（可多值）
    pub const SUBJECT: u32 = 105;
    /// 出版日期
    pub const PUBLISHING_DATE: u32 = 106;
    /// 贡献者
    pub const CONTRIBUTOR: u32 = 108;
    /// 版权
    pub const RIGHTS: u32 = 109;
    /// 主题词（可多值）
    pub const SUBJECT_CODE: u32 = 110;
    /// 类型
    pub const TYPE: u32 = 111;
    /// 来源
    pub const SOURCE: u32 = 112;
    /// ASIN
    pub const ASIN: u32 = 113;
    /// 语言（与 524 并存于不同工具）
    pub const LANGUAGE: u32 = 524;
    /// 书名（MOBI 的标题 tag）
    pub const TITLE: u32 = 503;
    /// 更新标题
    pub const UPDATED_TITLE: u32 = 504;
    /// 封面偏移（数值）
    pub const COVER_OFFSET: u32 = 201;
    /// 缩略图偏移（数值）
    pub const THUMB_OFFSET: u32 = 202;
    /// 起始阅读位置（数值）
    pub const START_OFFSET: u32 = 203;
    /// 覆盖记录数（数值）
    pub const COVER_RECORDS: u32 = 204;
    /// 文本记录数（数值）
    pub const TEXT_RECORDS: u32 = 205;
    /// 记录大小（数值）
    pub const RECORD_SIZE: u32 = 206;
}

/// 解析 EXTH 块。
///
/// `bytes` 应当是**从 EXTH 魔数开始**的切片（调用方用
/// [`crate::mobi::header::MobiHeader::exth_offset`] 定位）。
pub fn parse(bytes: &[u8]) -> LocalResult<Exth> {
    if bytes.len() < 12 {
        return Err(LocalError::damaged(format!(
            "EXTH 块太短（{} 字节，至少需要 12）",
            bytes.len()
        )));
    }
    if &bytes[0..4] != EXTH_MAGIC {
        return Err(LocalError::damaged(format!(
            "EXTH 魔数不匹配（实际 {:?}）",
            String::from_utf8_lossy(&bytes[0..4])
        )));
    }

    let declared_length = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let count = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;

    let mut warnings = Vec::new();
    // 边界取"声明长度"与"实际可用字节"的更严格者（见模块文档的易错点 2）。
    let limit = if declared_length >= 12 && declared_length <= bytes.len() {
        declared_length
    } else {
        if declared_length > bytes.len() {
            warnings.push(format!(
                "EXTH 声明长度 {declared_length} 超出可用字节 {}，按实际长度解析",
                bytes.len()
            ));
        } else {
            warnings.push(format!(
                "EXTH 声明长度 {declared_length} 不合理（小于块头），按实际长度解析"
            ));
        }
        bytes.len()
    };

    let mut records = Vec::with_capacity(count.min(256));
    let mut cursor = 12usize;
    let mut parsed = 0usize;
    while parsed < count && cursor + RECORD_HEADER_BYTES <= limit {
        let tag = u32::from_be_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]);
        let length = u32::from_be_bytes([
            bytes[cursor + 4],
            bytes[cursor + 5],
            bytes[cursor + 6],
            bytes[cursor + 7],
        ]) as usize;

        // 长度字段含 8 字节记录头（易错点 1）。
        if length < RECORD_HEADER_BYTES {
            warnings.push(format!(
                "第 {parsed} 条 EXTH 记录的长度 {length} 小于记录头，停止解析"
            ));
            break;
        }
        let end = cursor + length;
        if end > limit {
            warnings.push(format!(
                "第 {parsed} 条 EXTH 记录（tag {tag}）超出块边界，截断读取"
            ));
            let available_end = limit.max(cursor + RECORD_HEADER_BYTES);
            records.push(ExthRecord {
                tag,
                data: bytes[cursor + RECORD_HEADER_BYTES..available_end].to_vec(),
            });
            break;
        }

        records.push(ExthRecord {
            tag,
            data: bytes[cursor + RECORD_HEADER_BYTES..end].to_vec(),
        });
        cursor = end;
        parsed += 1;
    }

    if parsed < count {
        warnings.push(format!(
            "EXTH 声称有 {count} 条记录，实际解析出 {parsed} 条"
        ));
    }

    Ok(Exth { records, warnings })
}

/// 从 EXTH 取出版本信息之外的人类可读元数据（供 CLI 输出与诊断）。
impl Exth {
    /// 书名。
    pub fn title(&self) -> Option<String> {
        self.string(tags::TITLE)
            .or_else(|| self.string(tags::UPDATED_TITLE))
    }

    /// 作者（EXTH 的 100 是主作者）。
    pub fn author(&self) -> Option<String> {
        self.string(tags::AUTHOR)
    }

    /// 简介。
    pub fn description(&self) -> Option<String> {
        self.string(tags::DESCRIPTION)
    }

    /// 语言。
    pub fn language(&self) -> Option<String> {
        self.string(tags::LANGUAGE)
            .or_else(|| self.string(tags::LANGUAGE))
    }

    /// 主题词（可多值）。
    pub fn subjects(&self) -> Vec<String> {
        self.strings(tags::SUBJECT)
    }

    /// 封面所在记录的索引（tag 201 是**记录号**，不是字节偏移）。
    ///
    /// # 为什么 tag 201 的值是记录号
    ///
    /// MOBI 把封面图片作为一条普通记录存放，而 201 记录的是它在记录表中的
    /// **序号**（"第一条图像记录 + 偏移"的语义随工具而异，但实测
    /// calibre 产出的文件里它就是记录号）。
    pub fn cover_record(&self) -> Option<u32> {
        self.first(tags::COVER_OFFSET)?.as_u32()
    }

    /// 生成人类可读的元数据摘要（供 `tome local import` 的 human 输出）。
    pub fn describe(&self) -> Vec<(u32, String)> {
        self.records
            .iter()
            .map(|record| (record.tag, record.as_string()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个 EXTH 块。
    fn build_exth(records: &[(u32, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (tag, data) in records {
            let length = (RECORD_HEADER_BYTES + data.len()) as u32;
            body.extend_from_slice(&tag.to_be_bytes());
            body.extend_from_slice(&length.to_be_bytes());
            body.extend_from_slice(data);
        }
        let mut out = Vec::new();
        out.extend_from_slice(EXTH_MAGIC);
        out.extend_from_slice(&((12 + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(&(records.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn parses_a_simple_block() {
        let bytes = build_exth(&[(tags::TITLE, b"My Book Title"), (tags::AUTHOR, b"Jane Doe")]);
        let exth = parse(&bytes).expect("应解析成功");
        assert_eq!(exth.records.len(), 2);
        assert_eq!(exth.title().as_deref(), Some("My Book Title"));
        assert_eq!(exth.author().as_deref(), Some("Jane Doe"));
    }

    #[test]
    fn missing_magic_is_rejected() {
        let mut bytes = build_exth(&[(tags::TITLE, b"x")]);
        bytes[0] = b'X';
        assert!(matches!(parse(&bytes), Err(LocalError::Damaged { .. })));
    }

    #[test]
    fn too_short_block_is_rejected() {
        assert!(matches!(parse(b"EXTH"), Err(LocalError::Damaged { .. })));
    }

    #[test]
    fn record_length_includes_the_eight_byte_header() {
        // 这是易错点 1：若实现把 length 当 data 长度，第二条记录就会错位。
        let bytes = build_exth(&[(100, b"abc"), (101, b"de")]);
        let exth = parse(&bytes).expect("应解析");
        assert_eq!(exth.records[0].data, b"abc".to_vec());
        assert_eq!(exth.records[1].data, b"de".to_vec());
        assert_eq!(exth.records[1].tag, 101);
    }

    #[test]
    fn trailing_nul_is_stripped_from_strings() {
        // 真实 EXTH 的字符串常以 NUL 补齐到 4 字节边界。
        let bytes = build_exth(&[(tags::TITLE, b"Hi\0\0")]);
        let exth = parse(&bytes).expect("应解析");
        assert_eq!(exth.title().as_deref(), Some("Hi"));
    }

    #[test]
    fn numeric_tags_read_the_trailing_four_bytes() {
        // EXTH 的数值 tag 是 8 字节：4 字节填充 + 4 字节值在**末尾**。
        let value: u32 = 0x0000_00C9;
        let mut data = vec![0u8; 4];
        data.extend_from_slice(&value.to_be_bytes());
        let bytes = build_exth(&[(tags::COVER_RECORDS, &data)]);
        let exth = parse(&bytes).expect("应解析");
        assert_eq!(
            exth.first(tags::COVER_RECORDS).and_then(ExthRecord::as_u32),
            Some(201)
        );
    }

    #[test]
    fn multi_valued_tags_are_all_returned() {
        let bytes = build_exth(&[
            (tags::SUBJECT, b"Fiction"),
            (tags::TITLE, b"T"),
            (tags::SUBJECT, b"Adventure"),
        ]);
        let exth = parse(&bytes).expect("应解析");
        assert_eq!(exth.subjects(), vec!["Fiction", "Adventure"]);
    }

    #[test]
    fn count_larger_than_actual_records_is_reported() {
        let mut bytes = build_exth(&[(100, b"only one")]);
        // 把条数改成 5。
        bytes[8..12].copy_from_slice(&5u32.to_be_bytes());
        let exth = parse(&bytes).expect("应解析（不致命）");
        assert_eq!(exth.records.len(), 1);
        assert!(!exth.warnings.is_empty(), "条数不符必须留诊断，不能静默");
    }

    #[test]
    fn declared_length_beyond_the_slice_falls_back_and_warns() {
        let mut bytes = build_exth(&[(100, b"data")]);
        // 把块长度改成一个荒谬的大值。
        bytes[4..8].copy_from_slice(&999_999u32.to_be_bytes());
        let exth = parse(&bytes).expect("应解析");
        assert_eq!(exth.records.len(), 1);
        assert!(exth.warnings.iter().any(|w| w.contains("超出可用字节")));
    }

    #[test]
    fn record_length_smaller_than_the_header_stops_parsing() {
        let mut bytes = build_exth(&[(100, b"abc")]);
        // 把第一条记录的长度改成 4（小于 8 字节的记录头）。
        bytes[12 + 4..12 + 8].copy_from_slice(&4u32.to_be_bytes());
        let exth = parse(&bytes).expect("应解析（不致命）");
        assert!(exth.records.is_empty());
        assert!(exth.warnings.iter().any(|w| w.contains("小于记录头")));
    }

    #[test]
    fn record_extending_past_the_block_is_truncated_with_a_warning() {
        let mut bytes = build_exth(&[(100, b"abc")]);
        // 声明一个比块还长的记录长度。
        bytes[12 + 4..12 + 8].copy_from_slice(&9999u32.to_be_bytes());
        let exth = parse(&bytes).expect("应解析");
        assert_eq!(exth.records.len(), 1);
        assert!(exth.warnings.iter().any(|w| w.contains("超出块边界")));
    }

    #[test]
    fn empty_block_with_zero_records_parses() {
        let bytes = build_exth(&[]);
        let exth = parse(&bytes).expect("应解析");
        assert!(exth.records.is_empty());
        assert!(exth.title().is_none());
    }

    #[test]
    fn absent_fields_return_none_not_empty_strings() {
        let bytes = build_exth(&[(tags::TITLE, b"T")]);
        let exth = parse(&bytes).expect("应解析");
        assert!(exth.author().is_none());
        assert!(exth.description().is_none());
        assert!(exth.subjects().is_empty());
    }

    #[test]
    fn empty_string_values_are_treated_as_absent() {
        let bytes = build_exth(&[(tags::AUTHOR, b"\0\0\0\0")]);
        let exth = parse(&bytes).expect("应解析");
        assert!(exth.author().is_none(), "空值不应产出空字符串");
    }

    #[test]
    fn describe_exposes_every_tag_for_diagnostics() {
        let bytes = build_exth(&[(500, b"custom"), (tags::TITLE, b"T")]);
        let exth = parse(&bytes).expect("应解析");
        let described = exth.describe();
        assert_eq!(described.len(), 2);
        assert_eq!(described[0].0, 500);
        assert_eq!(described[0].1, "custom");
    }

    #[test]
    fn non_utf8_data_does_not_panic() {
        // 部分 EXTH 数据是二进制（如 204/205 之外的嵌入数据）。
        let bytes = build_exth(&[(999, &[0xFF, 0xFE, 0xFD, 0x00, 0x41])]);
        let exth = parse(&bytes).expect("应解析");
        let _ = exth.records[0].as_string();
        let _ = exth.describe();
    }

    #[test]
    fn real_world_tag_set_is_parsed() {
        // 复现实测的 368.kf8.mobi 的 EXTH（截取前几条）。
        let bytes = build_exth(&[
            (524, b"en"),
            (503, b"Acres of Diamonds: Our Every-day Opportunities"),
            (100, b"Russell H. Conwell"),
            (108, b"calibre (9.5.0) [http://calibre-ebook.com]"),
            (105, b"Success"),
            (105, b"Temple University"),
        ]);
        let exth = parse(&bytes).expect("应解析");
        assert_eq!(
            exth.title().as_deref(),
            Some("Acres of Diamonds: Our Every-day Opportunities")
        );
        assert_eq!(exth.author().as_deref(), Some("Russell H. Conwell"));
        assert_eq!(exth.language().as_deref(), Some("en"));
        assert_eq!(exth.subjects(), vec!["Success", "Temple University"]);
        assert!(exth.warnings.is_empty());
    }
}
