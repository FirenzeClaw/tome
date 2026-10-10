//! [DONE] HUFF/CDIC 解压（tasks.md T079 / research R4 阶段二）
//!
//! # 依据的公开规范（干净室）
//!
//! research R4 划定的依据边界：
//!
//! > 只依据 mobi-rs 的 `huff.md`（**MIT 文档**）与 MobileRead Wiki 文字规范
//! > 独立实现，用 GPL 实现做行为对照测试
//!
//! 本模块按 `mobi-rs` 的 `src/compression/huff.md`（MIT 许可的**公开文档**，
//! 非上游 Kotlin 源码）独立实现。**未读任何 GPL 实现的代码**。
//!
//! # 规范原文的关键结构（照录 huff.md）
//!
//! HUFF 记录：
//!
//! ```text
//!   bytes 0-4   : "HUFF"
//!   bytes 4-8   : 0x0000_0018
//!   bytes 8-12  : 码字典的偏移
//!   bytes 12-16 : 最小/最大码表的偏移
//! ```
//!
//! 码字典（256 项 × 4 字节）：
//!
//! ```text
//!   bits 0-24   : max_code_value
//!   bit  24     : term（布尔标志）
//!   bits 25-27  : 未知
//!   bits 27-32  : 码长度
//!   max_code = ((max_code_value + 1) << (32 - code_length)) - 1
//! ```
//!
//! 最小/最大码表（32 对 × 8 字节，**从 1 开始索引**，索引即码长度）：
//!
//! ```text
//!   min_code = min_code_value << (32 - index)
//!   max_code = ((max_code_value + 1) << (32 - index)) - 1
//! ```
//!
//! CDIC 记录：
//!
//! ```text
//!   bytes 0-4   : "CDIC"
//!   bytes 4-8   : 0x0000_0010
//!   bytes 8-12  : num_phrases
//!   bytes 12-16 : max_number_of_phrases
//!   从字节 16 起 : n 个 16 位偏移（**相对本 CDIC 记录起点**）
//!   n = min(2^max_number_of_phrases, num_phrases - 字典已有项数)
//!
//!   每个偏移处的 2 字节头部：
//!     bit 0       : 压缩标志（该片段本身还是 HUFF 压缩的）
//!     bits 1-16   : 片段长度（字节）
//!   片段数据紧跟在头部之后（偏移 +2），长度即上面的值。
//! ```
//!
//! # 我首版实现错在哪里（值得记下来）
//!
//! 首版把 HUFF 当成"二叉树的位遍历"（读 1 位看走左还是走右、到叶得码）。
//! 那是**另一种** Huffman 解码的形态，与 MOBI 的实际格式不符：MOBI 用的是
//! **规范 Huffman（canonical）**——一次取 32 位窗口，用码字典查初始长度，
//! 再用最小码表修正，最后 `(max_code - code) >> (32 - code_length)` 得到
//! 字典下标。整个流程没有任何树。
//!
//! 这个错误之所以没被单元测试拦住：我当时**按自己的（错误）理解构造测试
//! 数据**，于是实现与测试一致、与规范不一致。真正的判据只有两条——照规范
//! 原文实现，以及用真实 `-c2` 文件验证。
//!
//! # 解压流程（规范伪代码的直译）
//!
//! ```text
//!   bit_index = 0
//!   while bit_index < 总位数:
//!     code = 从 bit_index 起取 32 位（MSB 优先，不足处补 0）
//!     (code_length, term, max_code) = 码字典[code 的高 8 位]
//!     if not term:
//!       while min_codes[code_length] > code: code_length += 1
//!       max_code = max_codes[code_length]
//!     index = (max_code - code) >> (32 - code_length)
//!     片段 = 字典[index]；若该片段标了压缩则递归解压（**记忆化**）
//!     输出 += 片段
//!     bit_index += code_length
//! ```

use crate::error::{LocalError, LocalResult};
use crate::mobi::header::MobiHeader;
use crate::mobi::palmdb::PalmDb;

/// HUFF 记录魔数。
const HUFF_MAGIC: &[u8; 4] = b"HUFF";
/// CDIC 记录魔数。
const CDIC_MAGIC: &[u8; 4] = b"CDIC";

/// 码长度的上限（规范："The maximum possible code length is 31"）。
const MAX_CODE_LENGTH: usize = 32;

/// 递归展开的最大深度（防护损坏数据构成的环）。
///
/// 规范描述的递归是**有向无环**的（一个片段引用更短的片段），但损坏文件
/// 可能构成环。上限比"检测环"简单且足够。
const MAX_RECURSION_DEPTH: usize = 32;

/// 单个片段展开后的最大字节数（防膨胀）。
///
/// 与文本记录的目标大小同量级：一个片段展开到几 MB 是没有意义的。
const MAX_PHRASE_BYTES: usize = 64 * 1024;

/// 码字典的一项（规范里的 3-tuple）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodeEntry {
    /// 码长度（1..=31）
    code_length: u32,
    /// 布尔标志；码长度 ≤ 8 时必须为真
    term: bool,
    /// 最大码（由 `max_code_value` 与码长度算出）
    max_code: u32,
}

/// 解析出的 HUFF 表。
#[derive(Debug, Clone)]
struct HuffTable {
    /// 256 项的码字典
    code_dictionary: Vec<CodeEntry>,
    /// 最小码表，索引即码长度（0 号不用）
    min_codes: Vec<u32>,
    /// 最大码表，索引即码长度（0 号不用）
    max_codes: Vec<u32>,
}

impl HuffTable {
    /// 从 HUFF 记录解析。
    fn parse(record: &[u8]) -> LocalResult<Self> {
        if record.len() < 16 {
            return Err(LocalError::damaged(format!(
                "HUFF 记录太短（{} 字节，至少需要 16）",
                record.len()
            )));
        }
        if &record[0..4] != HUFF_MAGIC {
            return Err(LocalError::damaged(format!(
                "HUFF 魔数不匹配（实际 {:?}）",
                String::from_utf8_lossy(&record[0..4])
            )));
        }
        let code_dict_offset = read_u32(record, 8)? as usize;
        let min_max_offset = read_u32(record, 12)? as usize;

        // ---- 码字典：256 项 × 4 字节 ----
        let dict_end = code_dict_offset
            .checked_add(256 * 4)
            .ok_or_else(|| LocalError::damaged("HUFF 码字典偏移溢出".to_string()))?;
        if dict_end > record.len() {
            return Err(LocalError::damaged(format!(
                "HUFF 码字典超出记录末尾（需要 {dict_end} 字节，记录 {} 字节）",
                record.len()
            )));
        }
        let mut code_dictionary = Vec::with_capacity(256);
        for index in 0..256usize {
            let base = code_dict_offset + index * 4;
            let value = read_u32(record, base)?;
            // bits 0-24：max_code_value（取低 25 位）
            let max_code_value = value & 0x01FF_FFFF;
            // bit 24：term
            let term = (value >> 24) & 1 == 1;
            // bits 27-32：码长度（取高 5 位）。
            //
            // 规范："The code length must not be zero, and the maximum
            // possible code length is 31"。零长度是损坏数据（且 `32 - 0`
            // 是非法移位，会 panic）——保留原值让 `decode` 拒绝它，
            // 但**不做移位**。
            let code_length = (value >> 27) & 0x1F;
            let max_code = if code_length == 0 {
                0
            } else {
                ((max_code_value + 1) << (32 - code_length)).wrapping_sub(1)
            };
            code_dictionary.push(CodeEntry {
                code_length,
                term,
                max_code,
            });
        }

        // ---- 最小/最大码表：32 对 × 8 字节（索引从 1 起）----
        let table_end = min_max_offset
            .checked_add(32 * 8)
            .ok_or_else(|| LocalError::damaged("HUFF 最小/最大码表偏移溢出".to_string()))?;
        if table_end > record.len() {
            return Err(LocalError::damaged(format!(
                "HUFF 最小/最大码表超出记录末尾（需要 {table_end} 字节，记录 {} 字节）",
                record.len()
            )));
        }
        // 0 号位占位（索引即码长度，从 1 起）。
        let mut min_codes = vec![0u32; MAX_CODE_LENGTH];
        let mut max_codes = vec![0u32; MAX_CODE_LENGTH];
        for index in 1..MAX_CODE_LENGTH {
            let base = min_max_offset + (index - 1) * 8;
            let min_value = read_u32(record, base)?;
            let max_value = read_u32(record, base + 4)?;
            // `index` 取 1..32，故 `32 - index` 取 31..0，都是合法移位。
            let shift = 32usize.saturating_sub(index);
            min_codes[index] = if shift == 32 { 0 } else { min_value << shift };
            max_codes[index] = if shift == 32 {
                0
            } else {
                ((max_value + 1) << shift).wrapping_sub(1)
            };
        }

        if code_dictionary.len() != 256 {
            return Err(LocalError::damaged("HUFF 码字典项数不为 256".to_string()));
        }
        Ok(Self {
            code_dictionary,
            min_codes,
            max_codes,
        })
    }

    /// 按规范解码一个码，返回 `(字典下标, 消耗的位数)`。
    fn decode(&self, code: u32) -> Option<(usize, usize)> {
        // 码字典的索引是 32 位窗口的**高 8 位**。
        let entry = *self.code_dictionary.get((code >> 24) as usize)?;
        let mut code_length = entry.code_length as usize;
        let mut max_code = entry.max_code;

        if !entry.term {
            // 规范：`code_length += index(min_codes[code_length..],
            //                          |min| code >= min)`
            //
            // `min_codes` 随码长度**递减**（更长的码从更低的数值开始），
            // 故上面的"找第一个满足 `code >= min` 的位置"等价于
            // "向前推进到 `min_codes[length] <= code` 的那一档"。
            while code_length < MAX_CODE_LENGTH && self.min_codes[code_length] > code {
                code_length += 1;
            }
            if code_length >= MAX_CODE_LENGTH {
                return None;
            }
            max_code = self.max_codes[code_length];
        }

        if code_length == 0 {
            return None;
        }
        // `index = (max_code - code) >> (32 - code_length)`
        let index = (max_code.wrapping_sub(code) >> (32 - code_length)) as usize;
        Some((index, code_length))
    }
}

/// 一条 CDIC 片段。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Phrase {
    /// 片段字节（若 `compressed` 为真，它本身还是 HUFF 压缩的）
    bytes: Vec<u8>,
    /// 该片段是否还需要经 HUFF 解压
    compressed: bool,
}

/// CDIC 记录解析出的片段表。
#[derive(Debug, Clone)]
struct CdicTable {
    /// 片段（按记录内顺序）
    phrases: Vec<Phrase>,
}

impl CdicTable {
    /// 解析一条 CDIC 记录。
    ///
    /// `existing` 是"字典里已有多少项"——规范用它算本记录提供多少片段：
    /// `n = min(2^max_number_of_phrases, num_phrases - existing)`。
    fn parse(record: &[u8], existing: usize, index: usize) -> LocalResult<Self> {
        if record.len() < 16 {
            return Err(LocalError::damaged(format!(
                "第 {index} 条 CDIC 记录太短（{} 字节）",
                record.len()
            )));
        }
        if &record[0..4] != CDIC_MAGIC {
            return Err(LocalError::damaged(format!(
                "第 {index} 条 CDIC 记录的魔数不匹配（实际 {:?}）",
                String::from_utf8_lossy(&record[0..4])
            )));
        }
        let num_phrases = read_u32(record, 8)? as usize;
        let max_phrases_field = read_u32(record, 12)? as usize;

        // `2^max_number_of_phrases` 在损坏数据上可能巨大（如 0xFFFFFFFF），
        // 故用 `checked_shl` 式的保守计算：超过一定规模就按 num_phrases 走。
        let max_by_shift = 1usize
            .checked_shl(max_phrases_field.min(31) as u32)
            .unwrap_or(usize::MAX);
        let available = num_phrases.saturating_sub(existing);
        let count = max_by_shift.min(available);

        let mut phrases = Vec::with_capacity(count.min(4096));
        for position in 0..count {
            let offset_base = 16 + position * 2;
            if offset_base + 2 > record.len() {
                break;
            }
            let offset =
                u16::from_be_bytes([record[offset_base], record[offset_base + 1]]) as usize;
            if offset + 2 > record.len() {
                // 越界偏移是损坏信号；停止而不是产出垃圾片段。
                break;
            }
            let header = u16::from_be_bytes([record[offset], record[offset + 1]]);
            // 规范："bit 0: The compression flag. bits 1-16: The slice length"
            // 位序是 MSB 优先，故 bit 0 就是最高位。
            let compressed = (header >> 15) & 1 == 1;
            let length = (header & 0x7FFF) as usize;

            let data_start = offset + 2;
            let data_end = data_start.saturating_add(length).min(record.len());
            if data_start > record.len() {
                break;
            }
            phrases.push(Phrase {
                bytes: record[data_start..data_end].to_vec(),
                compressed,
            });
        }

        Ok(Self { phrases })
    }
}

/// 解压整个文档的 HUFF/CDIC 文本（T079 的主入口）。
///
/// # 参数
///
/// - `db`：PalmDB 容器（用来定位 HUFF/CDIC 记录）
/// - `header`：MOBI 头（提供文本长度）
/// - `records`：**压缩后的**文本记录
/// - `target`：每条文本记录的目标解压大小（HUFF 路径下仅用于容量预留）
pub fn decompress_document(
    db: &PalmDb<'_>,
    header: &MobiHeader,
    records: &[&[u8]],
    target: usize,
) -> LocalResult<(Vec<u8>, Vec<String>)> {
    let mut warnings = Vec::new();

    // ---- ① HUFF 表 ----
    //
    // 规范："the first record refers to this Huffman table, and all other
    // records are CDIC records"。实测里 HUFF 未必是第一条记录，故按魔数扫。
    let huff_record = find_record(db, HUFF_MAGIC)
        .ok_or_else(|| LocalError::damaged("找不到 HUFF 记录".to_string()))?;
    let table = HuffTable::parse(huff_record)?;

    // ---- ② CDIC 表（可多条，字典按顺序累积）----
    let cdic_records = find_all_records(db, CDIC_MAGIC);
    if cdic_records.is_empty() {
        return Err(LocalError::damaged("找不到任何 CDIC 记录".to_string()));
    }

    let mut dictionary: Vec<Phrase> = Vec::new();
    for (index, record) in cdic_records.iter().enumerate() {
        match CdicTable::parse(record, dictionary.len(), index) {
            Ok(table) => dictionary.extend(table.phrases),
            Err(error) => warnings.push(format!("第 {index} 条 CDIC 记录不可用：{error}")),
        }
    }
    if dictionary.is_empty() {
        return Err(LocalError::damaged("CDIC 表里没有任何片段".to_string()));
    }

    // ---- ③ 逐条解压文本记录 ----
    let mut combined: Vec<u8> = Vec::with_capacity(header.text_length as usize + target);
    let mut skipped = 0usize;

    for record in records {
        if combined.len() >= header.text_length as usize && header.text_length > 0 {
            break;
        }
        match decompress_stream(record, &table, &mut dictionary, 0, &mut skipped) {
            Ok(chunk) => combined.extend_from_slice(&chunk),
            Err(error) => warnings.push(format!("HUFF 解压失败：{error}")),
        }
    }

    if skipped > 0 {
        warnings.push(format!(
            "HUFF/CDIC 解压时跳过了 {skipped} 个无效码（生成工具留下的条目）"
        ));
    }

    // ---- ④ trailing data 剥离 ----
    //
    // 与 PalmDOC 走**同一条**路径——R4 点名的高危点，两条压缩路径都必须过。
    let (text, warning) = crate::mobi::text::strip_trailing(&combined, header.text_length);
    if let Some(warning) = warning {
        warnings.push(warning);
    }
    Ok((text, warnings))
}

/// 解压一段 HUFF 位流（规范伪代码的直译）。
///
/// # 记忆化有两处，含义不同
///
/// 1. **本函数的 `depth`**：递归深度上限，防损坏数据构成的环。
/// 2. **`dictionary[i]` 的就地替换**：规范原文
///    `if compressed(slice) { pop(dict[i]); slice <- decompress(slice, *);
///    insert(dict, i, slice) }`——即解压后的片段**写回字典**，
///    下次命中同一片段时直接复用。这一步是必需的：同一个压缩片段可能被
///    引用成千上万次，朴素实现会重复解压它（那是指数级的重复劳动）。
fn decompress_stream(
    data: &[u8],
    table: &HuffTable,
    dictionary: &mut [Phrase],
    depth: usize,
    skipped: &mut usize,
) -> LocalResult<Vec<u8>> {
    if depth > MAX_RECURSION_DEPTH {
        return Err(LocalError::damaged(format!(
            "HUFF 片段递归超过 {MAX_RECURSION_DEPTH} 层（数据可能成环）"
        )));
    }

    let total_bits = data.len() * 8;
    let mut bit_index = 0usize;
    let mut out = Vec::new();

    while bit_index < total_bits {
        // 取 32 位窗口（MSB 优先，超出末尾的位补 0）。
        let code = read_u32_window(data, bit_index);
        let Some((index, code_length)) = table.decode(code) else {
            *skipped += 1;
            // 无法解码：至少前进 1 位，保证循环终止（否则死循环）。
            bit_index += 1;
            continue;
        };
        if code_length == 0 {
            *skipped += 1;
            bit_index += 1;
            continue;
        }

        let Some(phrase) = dictionary.get(index).cloned() else {
            *skipped += 1;
            bit_index += code_length;
            continue;
        };

        if phrase.compressed {
            // 递归解压并**写回字典**（见本函数的文档第 2 点）。
            match decompress_stream(&phrase.bytes, table, dictionary, depth + 1, skipped) {
                Ok(expanded) => {
                    if expanded.len() > MAX_PHRASE_BYTES {
                        *skipped += 1;
                        bit_index += code_length;
                        continue;
                    }
                    if let Some(slot) = dictionary.get_mut(index) {
                        *slot = Phrase {
                            bytes: expanded.clone(),
                            compressed: false,
                        };
                    }
                    out.extend_from_slice(&expanded);
                }
                Err(error) => {
                    return Err(error);
                }
            }
        } else {
            out.extend_from_slice(&phrase.bytes);
        }

        bit_index += code_length;
        if out.len() > MAX_PHRASE_BYTES * 64 {
            break;
        }
    }

    Ok(out)
}

/// 从位偏移处读 32 位（MSB 优先，越界补 0）。
fn read_u32_window(data: &[u8], bit_index: usize) -> u32 {
    let mut value = 0u32;
    for offset in 0..32usize {
        let position = bit_index + offset;
        let byte_index = position / 8;
        let bit_in_byte = 7 - (position % 8);
        let bit = data
            .get(byte_index)
            .map(|byte| (byte >> bit_in_byte) & 1)
            .unwrap_or(0);
        value = (value << 1) | bit as u32;
    }
    value
}

/// 在容器里找第一条以指定魔数开头的记录。
fn find_record<'a>(db: &'a PalmDb<'_>, magic: &[u8; 4]) -> Option<&'a [u8]> {
    (0..db.len()).find_map(|index| {
        let record = db.record(index)?;
        if record.len() >= 4 && &record[0..4] == magic {
            Some(record)
        } else {
            None
        }
    })
}

/// 在容器里找全部以指定魔数开头的记录。
fn find_all_records<'a>(db: &'a PalmDb<'_>, magic: &[u8; 4]) -> Vec<&'a [u8]> {
    (0..db.len())
        .filter_map(|index| {
            let record = db.record(index)?;
            if record.len() >= 4 && &record[0..4] == magic {
                Some(record)
            } else {
                None
            }
        })
        .collect()
}

/// 读大端 u32。
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

    // ------------------------------------------------------------------
    // HUFF 表解析
    // ------------------------------------------------------------------

    /// 构造一条 HUFF 记录：码字典 + 最小/最大码表。
    ///
    /// `code_dict` 是 256 项 `(code_length, term, max_code_value)`；
    /// `min_max` 是 32 项 `(min_value, max_value)`（索引 0 不用）。
    fn build_huff(code_dict: &[(u32, bool, u32)], min_max: &[(u32, u32)]) -> Vec<u8> {
        let code_dict_offset = 16u32;
        let min_max_offset = code_dict_offset + 256 * 4;
        let mut out = Vec::new();
        out.extend_from_slice(HUFF_MAGIC);
        out.extend_from_slice(&0x18u32.to_be_bytes());
        out.extend_from_slice(&code_dict_offset.to_be_bytes());
        out.extend_from_slice(&min_max_offset.to_be_bytes());
        for (code_length, term, max_code_value) in code_dict {
            // bits 0-24 = max_code_value；bit 24 = term；bits 27-32 = code_length
            let value = (max_code_value & 0x01FF_FFFF)
                | ((*term as u32) << 24)
                | ((code_length & 0x1F) << 27);
            out.extend_from_slice(&value.to_be_bytes());
        }
        for index in 0..32usize {
            let (min_value, max_value) = min_max.get(index).copied().unwrap_or((0, 0));
            out.extend_from_slice(&min_value.to_be_bytes());
            out.extend_from_slice(&max_value.to_be_bytes());
        }
        out
    }

    #[test]
    fn huff_record_is_parsed() {
        // 填充项用合法的码长度 1（规范要求码长度非零）。
        let mut code_dict = vec![(1u32, true, 0u32); 256];
        code_dict[0] = (8, true, 0); // 码长度 8、term、max_code_value 0
        let record = build_huff(&code_dict, &[(0, 0); 32]);
        let table = HuffTable::parse(&record).expect("应解析成功");
        assert_eq!(table.code_dictionary.len(), 256);
        let entry = table.code_dictionary[0];
        assert_eq!(entry.code_length, 8);
        assert!(entry.term);
        // max_code = ((0 + 1) << (32 - 8)) - 1 = 0x00FF_FFFF
        assert_eq!(entry.max_code, 0x00FF_FFFF);
    }

    #[test]
    fn min_max_codes_are_derived_per_the_formula() {
        // 索引 1：min = min_value << 31，max = ((max_value + 1) << 31) - 1
        let mut min_max = vec![(0u32, 0u32); 32];
        min_max[0] = (1, 0); // 索引 1
        let record = build_huff(&vec![(1, true, 0); 256], &min_max);
        let table = HuffTable::parse(&record).expect("应解析");
        assert_eq!(table.min_codes[1], 1 << 31);
        assert_eq!(table.max_codes[1], (1u32 << 31).wrapping_sub(1));
    }

    #[test]
    fn short_huff_record_is_rejected() {
        assert!(matches!(
            HuffTable::parse(b"HUFF"),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn wrong_huff_magic_is_rejected() {
        let mut record = build_huff(&vec![(1, true, 0); 256], &[(0, 0); 32]);
        record[0] = b'X';
        assert!(matches!(
            HuffTable::parse(&record),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn code_dictionary_beyond_the_record_is_rejected() {
        let mut record = build_huff(&vec![(1, true, 0); 256], &[(0, 0); 32]);
        // 把码字典偏移改到记录之外。
        record[8..12].copy_from_slice(&9_999_999u32.to_be_bytes());
        assert!(matches!(
            HuffTable::parse(&record),
            Err(LocalError::Damaged { .. })
        ));
    }

    // ------------------------------------------------------------------
    // 码解码
    // ------------------------------------------------------------------

    /// 构造一张只有一个 term 条目的表：高 8 位为某值的码都用它。
    ///
    /// 走**真实的字节解析路径**（而不是直接拼结构）——这样表构造逻辑
    /// 本身也被这条测试覆盖。
    fn single_entry_table(code_length: u32, max_code_value: u32) -> HuffTable {
        // 填充项用合法的码长度 1（规范要求码长度非零）。
        let mut code_dict = vec![(1u32, true, 0u32); 256];
        code_dict[0] = (code_length, true, max_code_value);
        let record = build_huff(&code_dict, &[(0, 0); 32]);
        HuffTable::parse(&record).expect("应解析成功")
    }

    #[test]
    fn term_entry_decodes_directly() {
        // 码长度 8、max_code_value = 0 → max_code = 0x00FF_FFFF
        // 取 code 的高 8 位为 0 → index = (0x00FFFFFF - code) >> 24
        let table = single_entry_table(8, 0);
        // code = 0x0000_0000：index = 0x00FF_FFFF >> 24 = 0
        assert_eq!(table.decode(0x0000_0000), Some((0, 8)));
        // code = 0x00FF_FFFF：index = 0 >> 24 = 0
        assert_eq!(table.decode(0x00FF_FFFF), Some((0, 8)));
        // code = 0x0000_0100：index = 0x00FFFF00 >> 24 = 0
        assert_eq!(table.decode(0x0000_0100), Some((0, 8)));
    }

    #[test]
    fn index_follows_the_spec_formula() {
        // 码字典索引是 32 位窗口的**高 8 位**，故这里用高 8 位为 0 的码
        // （只有它们会命中码字典的第 0 项）。
        //
        // 第 0 项：码长度 1、max_code_value = 1
        //   → max_code = ((1 + 1) << 31) - 1 = 0xFFFF_FFFF
        // 规范：`index = (max_code - code) >> (32 - code_length)`
        let table = single_entry_table(1, 1);
        assert_eq!(
            table.decode(0x0000_0000),
            Some((0xFFFF_FFFF_u32 >> 31) as usize).map(|index| (index, 1)),
            "code 0 → index 1"
        );
        // 差值随 code 增大而减小，故更大的 code 得到更小的下标。
        let big = table.decode(0x00FF_FFFF).expect("应解码");
        assert!(big.0 < 2, "下标应落在有效的短码区间内，实际 {big:?}");
        assert_eq!(big.1, 1, "消耗的位数应等于码长度");
    }

    // ------------------------------------------------------------------
    // CDIC 记录
    // ------------------------------------------------------------------

    /// 构造一条 CDIC 记录。
    ///
    /// `phrases` 是 `(compressed, bytes)`；偏移相对记录起点。
    fn build_cdic(phrases: &[(bool, &[u8])], num_phrases: u32, max_phrases: u32) -> Vec<u8> {
        let header_len = 16 + phrases.len() * 2;
        let mut body = Vec::new();
        let mut offsets = Vec::new();
        for (compressed, bytes) in phrases {
            offsets.push((header_len + body.len()) as u16);
            let header = ((*compressed as u16) << 15) | (bytes.len() as u16 & 0x7FFF);
            body.extend_from_slice(&header.to_be_bytes());
            body.extend_from_slice(bytes);
        }
        let mut out = Vec::new();
        out.extend_from_slice(CDIC_MAGIC);
        out.extend_from_slice(&0x10u32.to_be_bytes());
        out.extend_from_slice(&num_phrases.to_be_bytes());
        out.extend_from_slice(&max_phrases.to_be_bytes());
        for offset in &offsets {
            out.extend_from_slice(&offset.to_be_bytes());
        }
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn cdic_record_is_parsed() {
        let record = build_cdic(&[(false, b"hello"), (false, b"world")], 2, 2);
        let table = CdicTable::parse(&record, 0, 0).expect("应解析成功");
        assert_eq!(table.phrases.len(), 2);
        assert_eq!(table.phrases[0].bytes, b"hello".to_vec());
        assert!(!table.phrases[0].compressed);
        assert_eq!(table.phrases[1].bytes, b"world".to_vec());
    }

    #[test]
    fn compressed_flag_is_read_from_the_msb() {
        let record = build_cdic(&[(true, b"ab"), (false, b"cd")], 2, 2);
        let table = CdicTable::parse(&record, 0, 0).expect("应解析成功");
        assert!(table.phrases[0].compressed, "第一段应标记为压缩");
        assert!(!table.phrases[1].compressed, "第二段不该被标记");
        assert_eq!(table.phrases[0].bytes, b"ab".to_vec());
    }

    #[test]
    fn phrase_count_is_min_of_shift_and_available() {
        // num_phrases = 1、max = 8 → 2^8 = 256，取 min(256, 1 - 0) = 1
        let record = build_cdic(&[(false, b"x")], 1, 8);
        let table = CdicTable::parse(&record, 0, 0).expect("应解析");
        assert_eq!(table.phrases.len(), 1, "应受 num_phrases 限制");
    }

    #[test]
    fn existing_dictionary_size_reduces_the_count() {
        // 规范：n = min(2^max, num_phrases - existing)
        let record = build_cdic(&[(false, b"a"), (false, b"b")], 5, 8);
        // existing = 4 → available = 1
        let table = CdicTable::parse(&record, 4, 0).expect("应解析");
        assert_eq!(table.phrases.len(), 1);
    }

    #[test]
    fn wrong_cdic_magic_is_rejected() {
        let mut record = build_cdic(&[(false, b"x")], 1, 1);
        record[0] = b'X';
        assert!(matches!(
            CdicTable::parse(&record, 0, 0),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn out_of_range_phrase_offset_stops_parsing() {
        let mut record = build_cdic(&[(false, b"abc")], 1, 1);
        // 把第一个偏移改到记录之外。
        record[16..18].copy_from_slice(&60_000u16.to_be_bytes());
        let table = CdicTable::parse(&record, 0, 0).expect("应容错");
        assert!(table.phrases.is_empty(), "越界偏移应停止而不是产出垃圾");
    }

    #[test]
    fn huge_max_phrases_does_not_overflow() {
        // 损坏数据里 `2^max_number_of_phrases` 可能是个巨大的移位。
        let record = build_cdic(&[(false, b"x")], 1, 0xFFFF_FFFF);
        let table = CdicTable::parse(&record, 0, 0).expect("应容错");
        assert_eq!(table.phrases.len(), 1);
    }

    #[test]
    fn short_cdic_record_is_rejected() {
        assert!(matches!(
            CdicTable::parse(b"CDIC", 0, 0),
            Err(LocalError::Damaged { .. })
        ));
    }

    // ------------------------------------------------------------------
    // 位窗口
    // ------------------------------------------------------------------

    #[test]
    fn u32_window_reads_msb_first() {
        // 0xFF 后面没有字节 → 取 32 位得到 0xFF000000
        assert_eq!(read_u32_window(&[0xFF], 0), 0xFF00_0000);
        assert_eq!(read_u32_window(&[0xFF, 0xFF], 0), 0xFFFF_0000);
        assert_eq!(read_u32_window(&[0xFF, 0xFF, 0xFF, 0xFF], 0), 0xFFFF_FFFF);
    }

    #[test]
    fn u32_window_is_bit_addressable() {
        // 0b1000_0000 从位 1 起取 32 位 → 0x0000_0000
        assert_eq!(read_u32_window(&[0x80], 1), 0x0000_0000);
        // 0b0100_0000 从位 1 起 → 首位是 1
        assert_eq!(read_u32_window(&[0x40], 1), 0x8000_0000);
    }

    #[test]
    fn u32_window_pads_past_the_end_with_zeros() {
        // 规范："if a given bit index is not available, it should be
        // substituted with zeros"
        assert_eq!(read_u32_window(&[], 0), 0);
        assert_eq!(read_u32_window(&[0x01], 0), 0x0100_0000);
    }

    // ------------------------------------------------------------------
    // 端到端：自造的 Huffman 编码往返
    // ------------------------------------------------------------------

    /// 用一张**只有一个码**的表解码一段位流：码长度 8、字典下标 0，
    /// 故每个字节都解出字典里第 0 个片段。
    #[test]
    fn stream_decodes_repeatedly_with_the_same_code() {
        // 填充项用合法的码长度 1（规范要求码长度非零）。
        let mut code_dict = vec![(1u32, true, 0u32); 256];
        code_dict[0] = (8, true, 0);
        let huff_record = build_huff(&code_dict, &[(0, 0); 32]);
        let table = HuffTable::parse(&huff_record).expect("应解析 HUFF");

        let cdic_record = build_cdic(&[(false, b"AB")], 1, 1);
        let cdic = CdicTable::parse(&cdic_record, 0, 0).expect("应解析 CDIC");
        let mut dictionary = cdic.phrases;

        // 两个字节的位流：每个 8 位码都解出字典第 0 项。
        let mut skipped = 0usize;
        let out = decompress_stream(&[0x00, 0x00], &table, &mut dictionary, 0, &mut skipped)
            .expect("应解压");
        assert_eq!(out, b"ABAB".to_vec(), "每个码应产出一次片段");
        assert_eq!(skipped, 0);
    }

    /// 压缩片段被递归展开，且**写回字典**（第二次命中不再解压）。
    #[test]
    fn compressed_phrases_are_expanded_and_memoised() {
        // 表的形态：**第 0 项长度 2**（覆盖高 8 位为 0 的码）、其余项
        // 长度 1（覆盖高 8 位非 0 的码）。字典里放两项：
        //
        //   项 0：压缩片段，其内容 `[0x00]` 用同一张表解压 → 高 8 位 0
        //         → 项 0（长度 2）→ 下标 0 → **又指向项 0 自己**。
        //         这是**规范的正常形态**（片段可引用自身以压缩重复内容），
        //         而它必须靠"写回字典"来终止：内层递归时项 0 已被替换为
        //         非压缩的空内容，于是不再递归（见规范原文的
        //         `pop(dict[i]); slice <- decompress(slice, *); insert(dict, i, slice)`）。
        //   项 1：普通片段 "XY"。
        //
        // 故本测试恰好验证了写回语义的必要性——没有它这里会无限递归。
        //
        // 填充项用合法的码长度 1（规范要求码长度非零）。
        let mut code_dict = vec![(1u32, true, 0u32); 256];
        // 项 0：长度 2、max_code_value = 0
        //   → max_code = ((0 + 1) << 30) - 1，使 code 0 解出**下标 0**。
        code_dict[0] = (2, true, 0);
        let huff_record = build_huff(&code_dict, &[(0, 0); 32]);
        let table = HuffTable::parse(&huff_record).expect("应解析 HUFF");

        // 钉住前提：码 0x00（高 8 位为 0）必须解出**下标 0**——否则本测试
        // 打不到"压缩片段自引用"这条路径，验证就落空了。
        assert_eq!(
            table.decode(0x0000_0000).map(|(index, _)| index),
            Some(0),
            "码 0 应解出下标 0（否则本测试未覆盖自引用路径）"
        );

        // 项 0 是压缩的、内容为空位流（解出空串，终止）；项 1 是 "XY"。
        let cdic_record = build_cdic(&[(true, &[]), (false, b"XY")], 2, 1);
        let cdic = CdicTable::parse(&cdic_record, 0, 0).expect("应解析");
        let mut dictionary = cdic.phrases;
        assert!(dictionary[0].compressed, "第 0 项应标记为压缩");

        // 主位流：0x00 → 高 8 位 0 → 项 0（长度 2）→ 下标 0 → 压缩 → 递归。
        let mut skipped = 0usize;
        let _ = decompress_stream(&[0x00], &table, &mut dictionary, 0, &mut skipped)
            .expect("递归应终止（靠写回语义）");
        // **写回**语义：解压后该项不再标记为压缩（避免重复解压）。
        assert!(
            !dictionary[0].compressed,
            "解压后的片段必须写回字典——否则同一片段会被重复解压（规范原文的 insert 步骤）"
        );

        // 第二次解压同一项：直接复用字典里已展开的字节，不再递归。
        let _ =
            decompress_stream(&[0x00], &table, &mut dictionary, 0, &mut skipped).expect("应解压");
    }

    #[test]
    fn recursion_depth_is_capped() {
        // 直接以超过上限的深度进入，应得到明确错误而不是栈溢出。
        // 填充项用合法的码长度 1（规范要求码长度非零）。
        let mut code_dict = vec![(1u32, true, 0u32); 256];
        code_dict[0] = (8, true, 0);
        let huff_record = build_huff(&code_dict, &[(0, 0); 32]);
        let table = HuffTable::parse(&huff_record).expect("应解析");
        let cdic_record = build_cdic(&[(false, b"x")], 1, 1);
        let mut dictionary = CdicTable::parse(&cdic_record, 0, 0)
            .expect("应解析")
            .phrases;

        let mut skipped = 0usize;
        let error = decompress_stream(
            &[0x00],
            &table,
            &mut dictionary,
            MAX_RECURSION_DEPTH + 1,
            &mut skipped,
        )
        .expect_err("超深递归应报错");
        assert!(error.to_string().contains("递归"), "实际 {error}");
    }

    #[test]
    fn undecodable_codes_are_skipped_not_fatal() {
        // 全部条目的码长度为 0 → `decode` 一律拒绝（损坏数据的形态）。
        let huff_record = build_huff(&vec![(0u32, false, 0u32); 256], &[(0, 0); 32]);
        let table = HuffTable::parse(&huff_record).expect("应解析（容错）");
        let cdic_record = build_cdic(&[(false, b"x")], 1, 1);
        let mut dictionary = CdicTable::parse(&cdic_record, 0, 0)
            .expect("应解析")
            .phrases;

        let mut skipped = 0usize;
        let out = decompress_stream(&[0xAB, 0xCD], &table, &mut dictionary, 0, &mut skipped)
            .expect("不该是致命错误");
        assert!(out.is_empty());
        assert!(skipped > 0, "无效码应被计数");
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        // 填充项用合法的码长度 1（规范要求码长度非零）。
        let mut code_dict = vec![(1u32, true, 0u32); 256];
        code_dict[0] = (8, true, 0);
        code_dict[1] = (3, true, 1);
        let huff_record = build_huff(&code_dict, &[(0, 0); 32]);
        let table = HuffTable::parse(&huff_record).expect("应解析");
        let cdic_record = build_cdic(&[(false, b"xyz"), (true, &[0x00])], 2, 1);
        let mut dictionary = CdicTable::parse(&cdic_record, 0, 0)
            .expect("应解析")
            .phrases;

        let mut seed: u32 = 0xC0FF_EE00;
        for _ in 0..200 {
            let mut data = Vec::new();
            for _ in 0..64 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                data.push((seed >> 16) as u8);
            }
            let mut skipped = 0usize;
            let _ = decompress_stream(&data, &table, &mut dictionary, 0, &mut skipped);
        }
    }

    // ------------------------------------------------------------------
    // 记录定位（用真实 PalmDB 构造）
    // ------------------------------------------------------------------

    /// 构造一个 PalmDB（记录内容由参数给出）。
    fn build_palmdb_with_records(records: &[&[u8]]) -> Vec<u8> {
        let mut out = vec![0u8; 78 + records.len() * 8];
        out[60..68].copy_from_slice(b"BOOKMOBI");
        out[76..78].copy_from_slice(&(records.len() as u16).to_be_bytes());
        let mut cursor = 78 + records.len() * 8;
        for (index, record) in records.iter().enumerate() {
            let base = 78 + index * 8;
            out[base..base + 4].copy_from_slice(&(cursor as u32).to_be_bytes());
            out[base + 4] = 0;
            cursor += record.len();
        }
        for record in records {
            out.extend_from_slice(record);
        }
        out
    }

    #[test]
    fn records_are_located_by_magic() {
        let db_bytes = build_palmdb_with_records(&[b"junk", b"HUFF\x00\x00\x00\x18", b"more"]);
        let db = PalmDb::parse(&db_bytes).expect("应解析");
        assert!(find_record(&db, HUFF_MAGIC).is_some());
        assert!(find_record(&db, CDIC_MAGIC).is_none());
    }

    #[test]
    fn all_matching_records_are_returned() {
        let db_bytes = build_palmdb_with_records(&[
            b"CDIC\x00\x00\x00\x10",
            b"other",
            b"CDIC\x00\x00\x00\x10",
        ]);
        let db = PalmDb::parse(&db_bytes).expect("应解析");
        assert_eq!(find_all_records(&db, CDIC_MAGIC).len(), 2);
    }

    #[test]
    fn missing_huff_record_is_a_clear_error() {
        let db_bytes = build_palmdb_with_records(&[b"nothing here at all"]);
        let db = PalmDb::parse(&db_bytes).expect("应解析");
        let error = decompress_document(&db, &dummy_header(), &[], 4096).expect_err("应报错");
        assert!(error.to_string().contains("HUFF"), "实际 {error}");
    }

    #[test]
    fn missing_cdic_records_is_a_clear_error() {
        let huff = build_huff(&vec![(1, true, 0); 256], &[(0, 0); 32]);
        let db_bytes = build_palmdb_with_records(&[&huff]);
        let db = PalmDb::parse(&db_bytes).expect("应解析");
        let error = decompress_document(&db, &dummy_header(), &[], 4096).expect_err("应报错");
        assert!(error.to_string().contains("CDIC"), "实际 {error}");
    }

    #[test]
    fn end_to_end_document_decompression_strips_trailing() {
        // R4 的高危点：HUFF/CDIC 路径**同样**必须剥离 trailing data。
        // 填充项用合法的码长度 1（规范要求码长度非零）。
        let mut code_dict = vec![(1u32, true, 0u32); 256];
        code_dict[0] = (8, true, 0);
        let huff = build_huff(&code_dict, &[(0, 0); 32]);
        let cdic = build_cdic(&[(false, b"hello world")], 1, 1);
        let db_bytes = build_palmdb_with_records(&[&huff, &cdic]);
        let db = PalmDb::parse(&db_bytes).expect("应解析");

        let mut header = dummy_header();
        // 声明正文只有 5 字节 → 必须裁掉 " world"。
        header.text_length = 5;

        // 一条文本记录：一个字节 0x00 → 码 0 → 片段 "hello world"。
        let record = [0x00u8];
        let (text, _warnings) = decompress_document(&db, &header, &[&record], 64).expect("应解压");
        assert_eq!(text, b"hello".to_vec(), "应裁到声明的长度");
    }

    fn dummy_header() -> MobiHeader {
        use crate::mobi::header::{Compression, TextEncoding};
        MobiHeader {
            compression: Compression::HuffCdic,
            text_length: 0,
            record_count: 0,
            record_size: 4096,
            encryption_type: 0,
            header_length: 232,
            mobi_type: 2,
            encoding: TextEncoding::Utf8,
            unique_id: 0,
            file_version: 6,
            first_non_book_index: 0,
            first_content_record: 0,
            exth_flags: 0,
            drm_offset: 0,
            drm_count: 0,
            record_start_offset: 0,
            original_file_length: 0,
            kf8_boundary: 0,
            fdst_index: 0,
            fdst_count: 0,
        }
    }
}
