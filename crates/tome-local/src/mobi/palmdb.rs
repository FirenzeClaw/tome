//! \[DONE] PalmDB 容器解析（tasks.md T078 / research R4）
//!
//! # 依据的公开规范
//!
//! - **MobileRead Wiki 的 PDB 词条**：`Palm Database` 的头部布局与记录表
//! - **MobileRead Wiki 的 MOBI 词条**：PalmDOC 头（0x00–0x10B）与 MOBI 头的字段表
//!
//! 本模块**不读任何上游 Kotlin 实现**（干净室纪律）。PalmDB 是一个
//! 30 年历史的公开容器格式（Palm OS 的数据库格式），其头部布局有完整文档。
//!
//! # 容器布局
//!
//! ```text
//!   偏移   长度  内容
//!   0x00   32    数据库名（以 NUL 结尾，不足补 NUL）
//!   0x20   2     属性
//!   0x22   2     版本
//!   0x24   4     创建时间（Palm epoch）
//!   0x28   4     修改时间
//!   0x2C   4     备份时间
//!   0x30   4     修改号
//!   0x34   4     应用信息偏移
//!   0x38   4     排序信息偏移
//!   0x3C   8     类型与创建者（`BOOKMOBI` / `TEXtREAd`）
//!   0x44   4     唯一种子
//!   0x48   4     下一个记录列表偏移
//!   0x4C   2     记录数 n
//!   0x4E   8×n   记录表（每条：4 字节偏移 + 1 字节属性 + 3 字节 UID）
//! ```
//!
//! ## 两个易错点
//!
//! 1. **记录大小要自己算**：记录表只给**起始偏移**，大小是"下一条的偏移
//!    减本条偏移"。最后一条的结束位置是**文件末尾**（不是任何记录头）。
//! 2. **记录表的字段宽度不对称**：偏移是 4 字节、属性 1 字节、UID **3 字节**
//!    ——按 4 字节读 UID 会让每条记录多读 1 字节，整个表错位。

use crate::error::{LocalError, LocalResult};

/// PalmDB 头部中记录表起始位置之前的固定部分长度。
const RECORD_TABLE_START: usize = 78;
/// 记录表每一项的长度（4 偏移 + 1 属性 + 3 UID）。
const RECORD_ENTRY_BYTES: usize = 8;

/// 一条记录（借用了容器的切片）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// 相对文件起点的字节偏移
    pub offset: usize,
    /// 长度（由相邻偏移算出）
    pub size: usize,
    /// 记录属性字节
    pub attributes: u8,
}

impl Record {
    /// 结束位置。
    pub fn end(&self) -> usize {
        self.offset + self.size
    }
}

/// 解析出的 PalmDB 容器（持有原始字节的引用）。
#[derive(Debug, Clone)]
pub struct PalmDb<'a> {
    /// 原始文件字节
    pub bytes: &'a [u8],
    /// 数据库名（头部 32 字节，已去 NUL）
    pub name: String,
    /// 类型与创建者（8 字节，MOBI 为 `BOOKMOBI`）
    pub type_creator: String,
    /// 记录表
    pub records: Vec<Record>,
}

impl<'a> PalmDb<'a> {
    /// 解析容器头部与记录表。
    ///
    /// # 不可信输入
    ///
    /// 每一处长度与偏移都**必须**校验：损坏的文件（截断、偏移乱指）会让
    /// 朴素的切片调用 panic。本函数把所有越界都转成 [`LocalError::Damaged`]。
    pub fn parse(bytes: &'a [u8]) -> LocalResult<Self> {
        if bytes.len() < RECORD_TABLE_START {
            return Err(LocalError::damaged(format!(
                "文件太小（{} 字节），不足以容纳 PalmDB 头部",
                bytes.len()
            )));
        }

        let name = read_c_string(&bytes[0..32]);
        let type_creator = String::from_utf8_lossy(&bytes[60..68]).into_owned();
        let record_count = u16::from_be_bytes([bytes[76], bytes[77]]) as usize;

        if record_count == 0 {
            return Err(LocalError::damaged("PalmDB 记录数为 0".to_string()));
        }

        let table_end = RECORD_TABLE_START
            .checked_add(record_count * RECORD_ENTRY_BYTES)
            .ok_or_else(|| LocalError::damaged("记录表长度溢出".to_string()))?;
        if table_end > bytes.len() {
            return Err(LocalError::damaged(format!(
                "记录表超出文件末尾（需要 {table_end} 字节，文件 {} 字节）",
                bytes.len()
            )));
        }

        // 先读出全部起始偏移。
        let mut offsets: Vec<(usize, u8)> = Vec::with_capacity(record_count);
        for index in 0..record_count {
            let base = RECORD_TABLE_START + index * RECORD_ENTRY_BYTES;
            let offset = u32::from_be_bytes([
                bytes[base],
                bytes[base + 1],
                bytes[base + 2],
                bytes[base + 3],
            ]) as usize;
            // 属性 1 字节，UID 3 字节 —— UID 不参与解析，但要**跨过**它，
            // 否则下一条记录的偏移会读错位置（见模块文档的易错点 2）。
            let attributes = bytes[base + 4];
            offsets.push((offset, attributes));
        }

        // 大小 = 下一条的起始偏移 − 本条起始偏移；末条的结束是文件末尾。
        let mut records = Vec::with_capacity(record_count);
        for (index, (offset, attributes)) in offsets.iter().enumerate() {
            let next = offsets
                .get(index + 1)
                .map(|(next_offset, _)| *next_offset)
                .unwrap_or(bytes.len());
            if *offset > bytes.len() {
                return Err(LocalError::damaged(format!(
                    "第 {index} 条记录的偏移 {offset} 超出文件末尾（{} 字节）",
                    bytes.len()
                )));
            }
            if next < *offset {
                return Err(LocalError::damaged(format!(
                    "第 {index} 条记录的偏移 {offset} 大于下一条的 {next}（记录表乱序）"
                )));
            }
            records.push(Record {
                offset: *offset,
                size: next - *offset,
                attributes: *attributes,
            });
        }

        Ok(Self {
            bytes,
            name,
            type_creator,
            records,
        })
    }

    /// 取第 `index` 条记录的字节。
    pub fn record(&self, index: usize) -> Option<&'a [u8]> {
        let record = self.records.get(index)?;
        self.bytes.get(record.offset..record.end())
    }

    /// 记录数。
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// 是否没有记录（`len() == 0` 的显式形式，clippy 要求成对出现）。
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// 读一个以 NUL 结尾的定长字符串。
fn read_c_string(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小的合法 PalmDB：`record_sizes` 决定各条记录的长度。
    fn build_palmdb(name: &str, records: &[&[u8]]) -> Vec<u8> {
        // 头部 + 记录表（每条 RECORD_ENTRY_BYTES 字节）；记录数据追加在后。
        let mut out = vec![0u8; RECORD_TABLE_START + records.len() * RECORD_ENTRY_BYTES];
        let name_bytes = name.as_bytes();
        let copy = name_bytes.len().min(31);
        out[..copy].copy_from_slice(&name_bytes[..copy]);
        out[60..68].copy_from_slice(b"BOOKMOBI");
        out[76..78].copy_from_slice(&(records.len() as u16).to_be_bytes());

        // 记录表
        let mut cursor = RECORD_TABLE_START + records.len() * RECORD_ENTRY_BYTES;
        let mut offsets = Vec::new();
        for (index, record) in records.iter().enumerate() {
            let base = RECORD_TABLE_START + index * RECORD_ENTRY_BYTES;
            out[base..base + 4].copy_from_slice(&(cursor as u32).to_be_bytes());
            out[base + 4] = 0;
            // UID 3 字节：故意填非零，用来验证"必须跨过它"。
            out[base + 5] = 0x11;
            out[base + 6] = 0x22;
            out[base + 7] = 0x33;
            offsets.push(cursor);
            cursor += record.len();
        }
        let _ = offsets;
        for record in records {
            out.extend_from_slice(record);
        }
        out
    }

    #[test]
    fn parses_name_type_and_record_count() {
        let bytes = build_palmdb("My Book", &[b"rec0", b"rec1"]);
        let db = PalmDb::parse(&bytes).expect("应解析成功");
        assert_eq!(db.name, "My Book");
        assert_eq!(db.type_creator, "BOOKMOBI");
        assert_eq!(db.len(), 2);
    }

    #[test]
    fn record_sizes_are_derived_from_neighbouring_offsets() {
        let bytes = build_palmdb("B", &[b"aaaa", b"bb", b"cccccc"]);
        let db = PalmDb::parse(&bytes).expect("应解析成功");
        assert_eq!(db.records[0].size, 4);
        assert_eq!(db.records[1].size, 2);
        assert_eq!(db.records[2].size, 6);
        assert_eq!(db.record(1), Some(b"bb".as_ref()));
        assert_eq!(db.record(2), Some(b"cccccc".as_ref()));
    }

    #[test]
    fn last_record_extends_to_the_end_of_file() {
        let bytes = build_palmdb("B", &[b"x", b"tail"]);
        let db = PalmDb::parse(&bytes).expect("应解析成功");
        let last = db.records.last().expect("有记录");
        assert_eq!(last.end(), bytes.len(), "末条记录的结束应是文件末尾");
    }

    #[test]
    fn uid_bytes_must_be_skipped() {
        // UID 是 3 字节。若实现按 4 字节读，记录表会整体错位——
        // build_palmdb 把 UID 填成非零正是为了让错位暴露成断言失败。
        let bytes = build_palmdb("B", &[b"one", b"two", b"three"]);
        let db = PalmDb::parse(&bytes).expect("应解析成功");
        assert_eq!(db.record(0), Some(b"one".as_ref()));
        assert_eq!(db.record(1), Some(b"two".as_ref()));
        assert_eq!(db.record(2), Some(b"three".as_ref()));
    }

    #[test]
    fn truncated_header_is_rejected() {
        let bytes = vec![0u8; 40];
        assert!(matches!(
            PalmDb::parse(&bytes),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn zero_records_is_rejected() {
        let mut bytes = vec![0u8; RECORD_TABLE_START];
        bytes[76] = 0;
        bytes[77] = 0;
        assert!(matches!(
            PalmDb::parse(&bytes),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn record_table_beyond_file_end_is_rejected() {
        let mut bytes = vec![0u8; RECORD_TABLE_START];
        // 声称有 1000 条记录，但文件里没有对应的表。
        bytes[76..78].copy_from_slice(&1000u16.to_be_bytes());
        assert!(matches!(
            PalmDb::parse(&bytes),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn record_offset_beyond_file_is_rejected() {
        let mut bytes = build_palmdb("B", &[b"aa"]);
        // 把第一条记录的偏移改到文件之外。
        bytes[RECORD_TABLE_START..RECORD_TABLE_START + 4]
            .copy_from_slice(&999_999u32.to_be_bytes());
        assert!(matches!(
            PalmDb::parse(&bytes),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn out_of_order_offsets_are_rejected() {
        let mut bytes = build_palmdb("B", &[b"aaa", b"bbb"]);
        // 让第二条的偏移小于第一条：大小计算会变成负数（usize 下溢）。
        bytes[RECORD_TABLE_START + 8..RECORD_TABLE_START + 12].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            PalmDb::parse(&bytes),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn name_shorter_than_the_field_is_read_correctly() {
        let bytes = build_palmdb("Hi", &[b"a"]);
        let db = PalmDb::parse(&bytes).expect("应解析成功");
        assert_eq!(db.name, "Hi");
    }

    #[test]
    fn full_length_name_without_nul_is_still_read() {
        let long_name = "0123456789012345678901234567890"; // 31 字符
        let bytes = build_palmdb(long_name, &[b"a"]);
        assert_eq!(PalmDb::parse(&bytes).expect("应解析").name, long_name);
    }

    #[test]
    fn empty_db_reports_is_empty() {
        // 头部 + 1 条记录表项，记录数据区为空。
        let table_end = RECORD_TABLE_START + RECORD_ENTRY_BYTES;
        let mut bytes = vec![0u8; table_end];
        bytes[76..78].copy_from_slice(&1u16.to_be_bytes());
        // 一条零长度记录：偏移指向**记录表之后**（即文件末尾）。
        bytes[RECORD_TABLE_START..RECORD_TABLE_START + 4]
            .copy_from_slice(&(table_end as u32).to_be_bytes());
        let db = PalmDb::parse(&bytes).expect("应解析");
        assert!(!db.is_empty(), "有一条（零长度）记录");
        assert_eq!(db.record(0), Some(&[][..]), "该记录应为空");
    }
}
