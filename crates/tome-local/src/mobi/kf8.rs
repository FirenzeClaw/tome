//! [DONE] KF8 的 FDST primary flow（tasks.md T081 / research R4 阶段三）
//!
//! # research R4 划定的范围
//!
//! > **阶段三（~8 人日）**：+ KF8 的 **FDST primary flow**（取整段 HTML，
//! > **跳过 SKEL/FRAG 拼接**）→ AZW3 纯文本 + INDX/NCX TOC
//!
//! "跳过 SKEL/FRAG 拼接"是本模块的**范围声明**，不是偷工减料：KF8 的完整
//! 还原需要把 `SKEL`（骨架片段）与 `FRAG`（碎片文本）按 FDST 分段表重新
//! 拼装，而 R4 明确把它列为"阶段四（可选）"。
//!
//! 本模块按 R4 的指引取**整段 HTML**：FDST 表给出的分段边界之内，
//! 文本记录本身已经是完整的 HTML 文档（只是被切成了若干记录）。
//!
//! # FDST 是什么
//!
//! ```text
//!   FDST 记录
//!     0x00  4  魔数 `FDST`
//!     0x04  4  头长度（含魔数，通常 12）
//!     0x08  4  分段数 n
//!     0x0C  8×n  分段表：（起始偏移, 结束偏移）对，各 4 字节大端
//! ```
//!
//! 偏移是**相对 KF8 部分的文本起点**的字节偏移（未压缩后）。故它的用途
//! 是把"解压后的连续文本流"切成若干段——每段是一个独立的 HTML 文档。
//!
//! # 为什么"取整段"是可用的
//!
//! KF8 的文本流解压后**本身**就是完整的 HTML（`<html>...<body>...`），
//! FDST 的分段只是把长文档切成了便于随机访问的块。只读纯文本的场景下，
//! 把全部分段按顺序拼起来（= 解压后的完整文本流）与"逐段渲染"得到的
//! 可见文本**内容相同**，差别只在跨段的节点边界。
//!
//! 故本模块提供：
//!
//! - [`parse_fdst`] —— 读分段表（供上层做分段级随机访问）
//! - [`primary_flow_range`] —— 取主流程（primary flow）的字节范围
//!
//! 而"把范围转成纯文本"由 [`crate::mobi`] 的 `html_to_text` 完成。

use crate::error::{LocalError, LocalResult};

/// FDST 记录魔数。
const FDST_MAGIC: &[u8; 4] = b"FDST";

/// 一段（FDST 表里的一对偏移）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FdstSection {
    /// 起始字节偏移（相对 KF8 文本流起点）
    pub start: u32,
    /// 结束字节偏移
    pub end: u32,
}

impl FdstSection {
    /// 长度（字节）。
    pub fn len(&self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    /// 是否为空段。
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// 解析出的 FDST 分段表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fdst {
    /// 分段列表（按文件里的顺序）
    pub sections: Vec<FdstSection>,
    /// 主流程的分段下标（通常是第一个非空段）
    pub primary_index: Option<usize>,
}

impl Fdst {
    /// 主流程的字节范围。
    ///
    /// # 为什么主流程不止一段
    ///
    /// KF8 的分段表里，前几段常是封面与目录等辅助流程，正文（primary flow）
    /// 可能跨多段。故本方法返回**从主流程起点到最后一节结束**的范围——
    /// 拼起来就是完整正文。
    pub fn primary_range(&self) -> Option<(u32, u32)> {
        let primary = self.primary_index?;
        let start = self.sections.get(primary)?.start;
        let end = self.sections.last()?.end;
        Some((start, end))
    }
}

/// 解析 FDST 记录（T081 的主入口）。
pub fn parse_fdst(record: &[u8]) -> LocalResult<Fdst> {
    if record.len() < 12 {
        return Err(LocalError::damaged(format!(
            "FDST 记录太短（{} 字节，至少需要 12）",
            record.len()
        )));
    }
    if &record[0..4] != FDST_MAGIC {
        return Err(LocalError::damaged(format!(
            "FDST 魔数不匹配（实际 {:?}）",
            String::from_utf8_lossy(&record[0..4])
        )));
    }

    let header_length = u32::from_be_bytes([record[4], record[5], record[6], record[7]]) as usize;
    let section_count = u32::from_be_bytes([record[8], record[9], record[10], record[11]]) as usize;

    if header_length < 12 {
        return Err(LocalError::damaged(format!(
            "FDST 头长度 {header_length} 小于 12"
        )));
    }
    let table_end = header_length
        .checked_add(section_count * 8)
        .ok_or_else(|| LocalError::damaged("FDST 分段表长度溢出".to_string()))?;
    if table_end > record.len() {
        return Err(LocalError::damaged(format!(
            "FDST 分段表超出记录末尾（需要 {table_end} 字节，记录 {} 字节）",
            record.len()
        )));
    }

    let mut sections = Vec::with_capacity(section_count);
    for index in 0..section_count {
        let base = header_length + index * 8;
        let start = u32::from_be_bytes([
            record[base],
            record[base + 1],
            record[base + 2],
            record[base + 3],
        ]);
        let end = u32::from_be_bytes([
            record[base + 4],
            record[base + 5],
            record[base + 6],
            record[base + 7],
        ]);
        if end < start {
            return Err(LocalError::damaged(format!(
                "FDST 第 {index} 段的结束 {end} 小于起点 {start}"
            )));
        }
        sections.push(FdstSection { start, end });
    }

    // 主流程：第一个非空段。实测（368.kf8.mobi 的 FDST）：
    // 段表为 [(0, 324647), (324647, 324704), ...]，第 0 段即正文。
    let primary_index = sections.iter().position(|section| !section.is_empty());

    Ok(Fdst {
        sections,
        primary_index,
    })
}

/// 从完整解压文本里取出主流程的范围。
///
/// # 越界处置
///
/// FDST 的偏移是**声明值**，而解压结果长度可能因 trailing data 剥离而与
/// 之不符。范围的边界被夹紧到实际长度——多出来的部分丢掉，缺少的部分
/// 保留可用的一段。这里**不报错**：FDST 与文本长度的轻微不一致在真实
/// 文件里常见，为此让整本书失败不划算。
pub fn primary_flow_range(fdst: &Fdst, text_length: usize) -> Option<(usize, usize)> {
    let (start, end) = fdst.primary_range()?;
    let start = (start as usize).min(text_length);
    let end = (end as usize).min(text_length).max(start);
    Some((start, end))
}

/// 抽取主流程的文本切片。
pub fn primary_flow<'a>(fdst: &Fdst, text: &'a str) -> &'a str {
    match primary_flow_range(fdst, text.len()) {
        Some((start, end)) => {
            // 两端都调整到字符边界，避免切片 panic。
            let start = adjust_boundary(text, start, true);
            let end = adjust_boundary(text, end, false);
            &text[start..end.max(start)]
        }
        None => text,
    }
}

/// 把偏移调整到合法字符边界。
fn adjust_boundary(text: &str, offset: usize, forward: bool) -> usize {
    let mut candidate = offset.min(text.len());
    if forward {
        while candidate < text.len() && !text.is_char_boundary(candidate) {
            candidate += 1;
        }
    } else {
        while candidate > 0 && !text.is_char_boundary(candidate) {
            candidate -= 1;
        }
    }
    candidate.min(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个 FDST 记录。
    fn build_fdst(sections: &[(u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(FDST_MAGIC);
        out.extend_from_slice(&12u32.to_be_bytes());
        out.extend_from_slice(&(sections.len() as u32).to_be_bytes());
        for (start, end) in sections {
            out.extend_from_slice(&start.to_be_bytes());
            out.extend_from_slice(&end.to_be_bytes());
        }
        out
    }

    #[test]
    fn parses_a_simple_section_table() {
        let record = build_fdst(&[(0, 100), (100, 200)]);
        let fdst = parse_fdst(&record).expect("应解析成功");
        assert_eq!(fdst.sections.len(), 2);
        assert_eq!(fdst.sections[0], FdstSection { start: 0, end: 100 });
        assert_eq!(fdst.sections[1].len(), 100);
    }

    #[test]
    fn primary_index_is_the_first_non_empty_section() {
        let record = build_fdst(&[(0, 0), (0, 500), (500, 600)]);
        let fdst = parse_fdst(&record).expect("应解析");
        assert_eq!(fdst.primary_index, Some(1), "空段不算主流程");
    }

    #[test]
    fn primary_range_spans_to_the_last_section() {
        let record = build_fdst(&[(0, 100), (100, 250)]);
        let fdst = parse_fdst(&record).expect("应解析");
        assert_eq!(fdst.primary_range(), Some((0, 250)));
    }

    #[test]
    fn real_world_fdst_shape_is_parsed() {
        // 复现实测的 368.kf8.mobi：5 段，头长度 12。
        let record = build_fdst(&[
            (0, 324_647),
            (324_647, 324_704),
            (324_704, 324_704),
            (324_704, 330_012),
            (330_012, 330_012),
        ]);
        let fdst = parse_fdst(&record).expect("应解析");
        assert_eq!(fdst.sections.len(), 5);
        assert_eq!(fdst.primary_index, Some(0));
        assert_eq!(fdst.primary_range(), Some((0, 330_012)));
    }

    #[test]
    fn wrong_magic_is_rejected() {
        let mut record = build_fdst(&[(0, 10)]);
        record[0] = b'X';
        assert!(matches!(
            parse_fdst(&record),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn too_short_record_is_rejected() {
        assert!(matches!(
            parse_fdst(b"FDST"),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn header_length_below_twelve_is_rejected() {
        let mut record = build_fdst(&[(0, 10)]);
        record[4..8].copy_from_slice(&8u32.to_be_bytes());
        assert!(matches!(
            parse_fdst(&record),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn section_table_beyond_the_record_is_rejected() {
        let mut record = build_fdst(&[(0, 10)]);
        // 声称有 100 段，但记录里只有 1 段的数据。
        record[8..12].copy_from_slice(&100u32.to_be_bytes());
        assert!(matches!(
            parse_fdst(&record),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn reversed_offsets_are_rejected() {
        let record = build_fdst(&[(500, 100)]);
        assert!(matches!(
            parse_fdst(&record),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn empty_section_table_parses_with_no_primary() {
        let record = build_fdst(&[]);
        let fdst = parse_fdst(&record).expect("应解析");
        assert!(fdst.sections.is_empty());
        assert_eq!(fdst.primary_index, None);
        assert_eq!(fdst.primary_range(), None);
    }

    #[test]
    fn all_empty_sections_have_no_primary() {
        let record = build_fdst(&[(0, 0), (10, 10)]);
        let fdst = parse_fdst(&record).expect("应解析");
        assert_eq!(fdst.primary_index, None);
    }

    #[test]
    fn section_helpers_report_correctly() {
        let empty = FdstSection { start: 5, end: 5 };
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        let normal = FdstSection { start: 5, end: 15 };
        assert!(!normal.is_empty());
        assert_eq!(normal.len(), 10);
    }

    #[test]
    fn primary_flow_slice_extracts_the_body() {
        let text = "PREFACE-BODY-CONTENT-END";
        let fdst = Fdst {
            sections: vec![FdstSection { start: 8, end: 24 }],
            primary_index: Some(0),
        };
        assert_eq!(primary_flow(&fdst, text), "BODY-CONTENT-END");
    }

    #[test]
    fn primary_flow_clamps_to_actual_text_length() {
        // FDST 声明的范围比实际文本长（trailing data 剥离之后常见）。
        let text = "short";
        let fdst = Fdst {
            sections: vec![FdstSection {
                start: 0,
                end: 9999,
            }],
            primary_index: Some(0),
        };
        assert_eq!(primary_flow(&fdst, text), "short", "应夹紧而不 panic");
    }

    #[test]
    fn primary_flow_without_primary_returns_everything() {
        let text = "the whole thing";
        let fdst = Fdst {
            sections: vec![],
            primary_index: None,
        };
        assert_eq!(primary_flow(&fdst, text), text);
    }

    #[test]
    fn primary_flow_handles_multibyte_text_without_panicking() {
        let text = "中文中文中文中文";
        // 偏移刻意落在多字节字符中间。
        let fdst = Fdst {
            sections: vec![FdstSection { start: 1, end: 13 }],
            primary_index: Some(0),
        };
        let slice = primary_flow(&fdst, text);
        // 能取到切片本身就证明落在字符边界上。
        assert!(!slice.is_empty());
    }

    #[test]
    fn range_is_clamped_when_text_is_shorter_than_declared() {
        let fdst = Fdst {
            sections: vec![FdstSection {
                start: 100,
                end: 200,
            }],
            primary_index: Some(0),
        };
        assert_eq!(primary_flow_range(&fdst, 50), Some((50, 50)));
    }

    #[test]
    fn range_is_exact_when_text_is_long_enough() {
        let fdst = Fdst {
            sections: vec![FdstSection { start: 10, end: 20 }],
            primary_index: Some(0),
        };
        assert_eq!(primary_flow_range(&fdst, 100), Some((10, 20)));
    }

    #[test]
    fn adjust_boundary_moves_to_valid_positions() {
        let text = "中";
        assert_eq!(adjust_boundary(text, 1, true), 3);
        assert_eq!(adjust_boundary(text, 2, false), 0);
        assert_eq!(adjust_boundary(text, 999, false), 3);
    }
}
