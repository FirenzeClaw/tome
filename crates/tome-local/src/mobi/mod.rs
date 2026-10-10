//! \[DONE] MOBI / AZW3 解析（tasks.md T078-T081 / FR-022）
//!
//! # 规格原文
//!
//! > **FR-022**: 系统 MUST 支持 MOBI/AZW3 导入：解析目录与正文并归一化，
//! > 使其可被**随机定位**
//!
//! "随机定位"是本模块的结构主线：解析的产物不是"一整段文本"，而是
//! **可寻址的章节列表**（每章记着它在文本里的偏移）。
//!
//! # 分阶段（research R4 的降险路径）
//!
//! | 阶段 | 内容 | 状态 |
//! |---|---|---|
//! | 一 | PalmDB + 头 + EXTH + trailing 剥离 + PalmDOC + MOBI6 纯文本 + guide 型 TOC | **\[DONE]** |
//! | 二 | HUFF/CDIC 解压（`-c2` 文件） | **\[DONE]** |
//! | 三 | KF8 FDST primary flow | **\[DONE]** |
//! | 四（可选） | SKEL/FRAG 拼接、图片提取 | 不属本切片 |
//!
//! # 三个格式形态与它们的正文位置
//!
//! ```text
//!   ① 纯 MOBI6（file_version ≤ 6）
//!      记录 0 = 头，记录 1..firstNonBook = 全文文本
//!
//!   ② 纯 KF8（AZW3）
//!      记录 0 = 头（firstContentRecord = 0xFFFFFFFF，本部分无文本）
//!      记录 kf8_boundary.. = KF8 部分（另一套头 + FDST + 文本记录）
//!
//!   ③ 双格式（file_version = 8）
//!      记录 0 = MOBI6 头 + 少量引导文本（"空壳"）
//!      记录 kf8_boundary.. = KF8 部分（真正的正文）
//! ```
//!
//! 形态 ③ 是实测的 `368.kf8.mobi`（Gutenberg 公有领域）的实际构造。故
//! 解析器的策略是：**优先找 KF8 部分**；没有才用 MOBI6 部分。
//!
//! # 目录的三种来源（按可靠性递减）
//!
//! 1. **KF8 的 INDX 索引**（`nav` 型）——最完整，但解析成本高
//! 2. **guide 型目录**（EXTH 或 MOBI 头的指南引用）
//! 3. **从正文里的 `<h1>`/标题推断**——兜底
//!
//! 本实现做 2 与 3（阶段三的 INDX 解析属"阶段四可选"范围）。这不是
//! 降级：MOBI6 文件本就只有 guide 型目录，而 KF8 文件的 HTML 里
//! `<h1>`–`<h6>` 已能给出可用的章节目录。
//!
//! # 关键入口
//!
//! - [`parse`] —— MOBI 字节 → [`ParsedMobi`]（含可寻址的章节）
//! - [`ParsedMobi::read_text_range`] —— 按偏移取文本（"随机定位"的落点）
//! - [`extract_text`] —— 全文 HTML → 纯文本（供 provider 的章节切分）

pub mod drm;
pub mod exth;
pub mod header;
pub mod huffcdic;
pub mod kf8;
pub mod palmdb;
pub mod text;

use tome_core::{Book, Chapter};

use crate::error::{LocalError, LocalResult};
use drm::{CompressionSupport, DrmVerdict};
use header::{Compression, MobiHeader, TextEncoding};
use palmdb::PalmDb;

/// 从正文里推断章节的标题标签（`<h1>`–`<h6>`）。
const HEADING_TAGS: [&str; 6] = ["h1", "h2", "h3", "h4", "h5", "h6"];

/// 一次 MOBI 解析的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMobi {
    /// 书籍元信息
    pub book: Book,
    /// 可寻址的章节列表
    pub toc: Vec<MobiChapter>,
    /// 解压后的**完整纯文本**（章节偏移以它为基准）
    pub text: String,
    /// 解析诊断（**不静默**：可疑的 DRM、截断、跳过的记录都在这里）
    pub warnings: Vec<String>,
    /// 实际采用的形态（供 CLI 展示与测试断言）
    pub layout: MobiLayout,
}

/// MOBI 文件的构造形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobiLayout {
    /// 纯 MOBI6
    Mobi6,
    /// 双格式容器的 MOBI6 部分（含 KF8）
    Mobi6InDual,
    /// KF8 部分（AZW3 或双格式的后半）
    Kf8,
}

impl MobiLayout {
    /// 落库用的标识。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mobi6 => "mobi6",
            Self::Mobi6InDual => "mobi6+dual",
            Self::Kf8 => "kf8",
        }
    }
}

/// 一个可寻址的章节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobiChapter {
    /// 章节标题
    pub title: String,
    /// 在完整纯文本里的**起始字节偏移**
    pub byte_offset: usize,
    /// 在完整纯文本里的**起始字符偏移**
    pub char_offset: usize,
}

impl ParsedMobi {
    /// 按字节偏移读取一段文本（"随机定位"的落点，FR-022）。
    ///
    /// `length` 为 `None` 时读到文本末尾。
    ///
    /// # 偏移落在非字符边界时的处置
    ///
    /// 直接从 `&str` 切片会在非边界处 panic。章节偏移由本模块自己算出，
    /// 理应总在边界上，但损坏文件或调用方传入的任意偏移都可能不在边界。
    /// 故这里向前调整到最近的字符边界——**不 panic，也不返回空**。
    pub fn read_text_range(
        &self,
        byte_offset: usize,
        char_length: Option<usize>,
    ) -> LocalResult<String> {
        if byte_offset >= self.text.len() {
            return Ok(String::new());
        }
        let start = adjust_to_char_boundary(&self.text, byte_offset);
        // `char_length` 是**字符**数（与 `Chapter.word_count` 的单位一致，
        // 也与 FR-028 的"字符区间"一致——中文下字符与字节差 3 倍）。
        let end = match char_length {
            Some(length) => {
                let mut remaining = length;
                let mut end = start;
                for (offset, _) in self.text[start..].char_indices() {
                    if remaining == 0 {
                        break;
                    }
                    remaining -= 1;
                    end = start + offset;
                }
                // 上面的循环把 `end` 停在**最后一个待取字符的起点**，
                // 故还要按该字符的宽度推进一次才算"取满 length 个字符"。
                if remaining == 0 {
                    end += self
                        .text
                        .get(end..)
                        .and_then(|tail| tail.chars().next())
                        .map(char::len_utf8)
                        .unwrap_or(0);
                }
                end.min(self.text.len())
            }
            None => self.text.len(),
        };
        let end = end.max(start);
        Ok(self.text[start..end].to_string())
    }
}

/// 把偏移向后调整到最近的字符边界（越界则取文本末尾）。
fn adjust_to_char_boundary(text: &str, offset: usize) -> usize {
    let mut candidate = offset.min(text.len());
    while candidate < text.len() && !text.is_char_boundary(candidate) {
        candidate += 1;
    }
    candidate.min(text.len())
}

/// \[DONE] MOBI 解析主入口（T078-T081）。
pub fn parse(bytes: &[u8]) -> LocalResult<ParsedMobi> {
    let mut warnings = Vec::new();

    // ① PalmDB 容器。
    let db = PalmDb::parse(bytes)?;
    if db.type_creator != "BOOKMOBI" {
        return Err(LocalError::UnsupportedFormat {
            format: format!("palmdb:{}", db.type_creator),
        });
    }

    let record0 = db
        .record(0)
        .ok_or_else(|| LocalError::damaged("PalmDB 没有记录 0（头部缺失）"))?;
    let mobi6_header = MobiHeader::parse(record0)?;

    // ② DRM 检测（T080）——**早于**任何解压尝试。
    //
    // ## 为什么必须在解压前
    //
    // 加密文件的文本记录用密钥加密过，直接喂给 PalmDOC 解压器会产出乱码
    // 而**不报错**（解压算法对任意字节都"成功"）。那不报错的结果是
    // 一本乱码书 + 零条错误信息，用户完全无从判断。故先判 DRM。
    match drm::check_drm(&mobi6_header) {
        DrmVerdict::Encrypted {
            encryption_type, ..
        } => {
            return Err(LocalError::DrmProtected { encryption_type });
        }
        DrmVerdict::Suspect { reason } => warnings.push(reason),
        DrmVerdict::Clear => {}
    }

    // ③ 找 KF8 部分（形态 ②/③）。
    let kf8 = locate_kf8_part(&db, &mobi6_header, &mut warnings);

    // ④ 决定用哪一部分的正文。
    let (header, text_records, layout) = match &kf8 {
        Some(part) => (
            part.header.clone(),
            part.text_records.clone(),
            if mobi6_header.is_kf8_capable() {
                MobiLayout::Mobi6InDual
            } else {
                MobiLayout::Kf8
            },
        ),
        None => {
            if mobi6_header.is_kf8_capable() {
                warnings.push(
                    "文件声明为双格式（版本 8）但找不到 KF8 部分，已按 MOBI6 部分解析".to_string(),
                );
            }
            (
                mobi6_header.clone(),
                collect_text_records(&db, &mobi6_header),
                MobiLayout::Mobi6,
            )
        }
    };

    // ⑤ 压缩支持度（T079）。HUFF/CDIC 与 DRM 是**两个独立的轴**——
    // 前者是"能不能解压"，后者是"该不该解压"（加密的文件解出来是乱码）。
    //
    // 这里**只拦未知算法**：HUFF/CDIC 由 ⑥ 的 `decompress_text` 分派到
    // `huffcdic::decompress_document`。首版在这里对 HUFF/CDIC 直接报错，
    // 使那条路径成了**死代码**（实测发现的缺陷：真实 `-c2` 文件会走到
    // 这里就返回错误，永远到不了 ⑥）。
    if let CompressionSupport::Unknown(code) = CompressionSupport::of(header.compression) {
        return Err(LocalError::damaged(format!("未知的压缩算法码 {code}")));
    }

    // ⑥ 解压文本。
    let (raw_text, decompress_warnings, encoding) = decompress_text(&header, &text_records, &db)?;
    warnings.extend(decompress_warnings);

    // ⑥b T081：KF8 的 FDST primary flow。
    //
    // FDST 的分段表描述文本区的结构（哪几段是正文、哪几段是辅助流程）。
    // 按 R4 的指引取**主流程**——即分段表覆盖的整段 HTML。
    //
    // ## 为什么在主流程被取窄时要报告
    //
    // 若 FDST 的主流程明显短于完整文本（比如只占 10%），说明这个文件的
    // 辅助流程里可能藏着正文的一部分（分段表把目录/封面与正文混在一份
    // 文本流里）。此时截断会**静默丢内容**，故必须留诊断让用户能发现。
    let raw_text = match kf8.as_ref().and_then(|part| part.fdst_index) {
        Some(index) => match db.record(index) {
            Some(record) => match kf8::parse_fdst(record) {
                Ok(fdst) => {
                    let sections = fdst.sections.len();
                    // 借出一个具名的 `Cow`，让切片活过这个表达式。
                    let decoded = String::from_utf8_lossy(&raw_text);
                    let flow = kf8::primary_flow(&fdst, &decoded);
                    let kept = flow.len();
                    let total = raw_text.len();
                    if total > 0 && kept * 10 < total * 9 {
                        warnings.push(format!(
                            "FDST primary flow 仅覆盖 {kept}/{total} 字节（{sections} 段），\
                             辅助流程未纳入正文"
                        ));
                    }
                    flow.to_string().into_bytes()
                }
                Err(error) => {
                    warnings.push(format!("FDST 解析失败，按整段文本处理：{error}"));
                    raw_text
                }
            },
            None => {
                warnings.push(format!("FDST 记录 {index} 不存在，按整段文本处理"));
                raw_text
            }
        },
        None => raw_text,
    };

    // ⑦ 归一化：HTML → 纯文本（FR-022 的"归一化"），并保留偏移映射。
    let (plain, _map) = html_to_text(&raw_text);

    // ⑧ 目录（guide 型 / 标题推断）。
    let toc = build_toc(&plain, &mobi6_header, &raw_text);

    // ⑨ 元数据（EXTH）。
    let exth = read_exth(record0, &mobi6_header, &mut warnings);
    let title = exth
        .as_ref()
        .and_then(exth::Exth::title)
        .unwrap_or_else(|| db.name.clone());
    let author = exth.as_ref().and_then(exth::Exth::author);
    let intro = exth.as_ref().and_then(exth::Exth::description);
    let language = exth.as_ref().and_then(exth::Exth::language);

    let book = Book {
        id: 0,
        origin: tome_core::BookOrigin::Local,
        origin_id: crate::hash::content_origin_id(bytes),
        name: if title.trim().is_empty() {
            "未命名".to_string()
        } else {
            title
        },
        author,
        cover_url: None,
        intro,
        kind: language,
        book_group: None,
        read_config: None,
        toc_url: None,
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: Some(encoding.charset_name()),
        format: Some("mobi".to_string()),
        total_chapters: Some(toc.len() as i32),
        dur_chapter_index: None,
        added_at: 0,
    };

    Ok(ParsedMobi {
        book,
        toc,
        text: plain,
        warnings,
        layout,
    })
}

/// 从文件解析。
pub fn parse_path(path: &std::path::Path) -> LocalResult<ParsedMobi> {
    let bytes = std::fs::read(path).map_err(|error| crate::error::from_io(path, error))?;
    let mut parsed = parse(&bytes)?;
    parsed.book.relative_path = Some(crate::library::normalize_for_storage(path));
    Ok(parsed)
}

/// 找到的 KF8 部分。
struct Kf8Part {
    /// 该部分的头（单头构造时就是记录 0 的头）
    header: MobiHeader,
    /// 文本记录（压缩态）
    text_records: Vec<Vec<u8>>,
    /// FDST 记录号（单头构造时存在，双头构造时为 `None`）
    fdst_index: Option<usize>,
}

/// 定位 KF8 部分（阶段三，T081）。
///
/// # 两种定位方式
///
/// 定位 KF8 部分（阶段三，T081）。
///
/// # 两种文件构造（实测确认）
///
/// | 构造 | 特征 | KF8 正文所在 |
/// |---|---|---|
/// | **单头 + FDST** | `kf8_boundary > 0` 且该记录是 `FDST`（**不是** MOBI 头） | 就是记录 0 头所描述的文本区 |
/// | **双头** | 某条记录在偏移 16 处有第二个 `MOBI` 魔数 | 第二条头之后的记录 |
///
/// ## 实测（本仓库 `.tome-mobi/368.kf8.mobi`，Gutenberg 公有领域）
///
/// ```text
///   file_version   = 8
///   firstNonBook   = 0xFFFFFFFF     ← 表示"文本区不连续"，须靠 FDST 定界
///   firstContent   = 83
///   kf8_boundary   = 96             ← 该记录是 FDST，不是 MOBI 头
///   fdst_index     = 5
///   fdst_count     = 98
///
///   记录 96 = FDST：5 个分段，覆盖 0..329435（正是整个文本区）
///   记录 97 = FLIS，98 = FCIS，99 = EOF 标记
/// ```
///
/// 也就是说：这个文件把 MOBI6 与 KF8 的文本**共用同一个文本区**，用
/// FDST 的分段表描述其结构。故"找第二个 MOBI 头"的朴素策略在这里
/// 找不到任何东西——首版实现因此在真实文件上误报"找不到 KF8 部分"。
///
/// # 两种构造的处理
///
/// - **单头 + FDST**：文本区就是记录 0 头描述的区间（`firstContent`
///   到 `firstNonBook` 或 FDST 段表的末尾），解析后按 FDST 取主流程。
///   此时返回的 `header` 仍是记录 0 的头（编码、压缩算法都从它读）。
/// - **双头**：用第二条头的字段。
fn locate_kf8_part(
    db: &PalmDb<'_>,
    mobi6_header: &MobiHeader,
    warnings: &mut Vec<String>,
) -> Option<Kf8Part> {
    // ---- 构造一：单头 + FDST ----
    //
    // 判据是"`kf8_boundary` 指向的记录以 `FDST` 开头"。若如此，本文件
    // 的文本区就是记录 0 头所描述的区间，不需要（也没有）第二条头。
    if mobi6_header.is_kf8_capable() {
        let boundary = mobi6_header.kf8_boundary as usize;
        if boundary > 0 && boundary < db.len() {
            let is_fdst = db
                .record(boundary)
                .is_some_and(|record| record.starts_with(b"FDST"));
            if is_fdst {
                let text_records = collect_kf8_fdst_records(db, mobi6_header);
                if !text_records.is_empty() {
                    return Some(Kf8Part {
                        header: mobi6_header.clone(),
                        text_records,
                        fdst_index: Some(boundary),
                    });
                }
            }
        }
    }

    // ---- 构造二：第二条 MOBI 头 ----
    let mut candidate_starts: Vec<usize> = Vec::new();
    if mobi6_header.kf8_boundary as usize > 0 && (mobi6_header.kf8_boundary as usize) < db.len() {
        candidate_starts.push(mobi6_header.kf8_boundary as usize);
    }

    // 扫描：记录 1 之后的每条记录的偏移 16 处是否有 `MOBI` 魔数。
    for index in 1..db.len() {
        let Some(record) = db.record(index) else {
            continue;
        };
        if record.len() > header::MOBI_MAGIC_OFFSET + 4
            && &record[header::MOBI_MAGIC_OFFSET..header::MOBI_MAGIC_OFFSET + 4] == b"MOBI"
        {
            candidate_starts.push(index);
        }
    }
    candidate_starts.sort_unstable();
    candidate_starts.dedup();

    for start in candidate_starts {
        let Some(record) = db.record(start) else {
            continue;
        };
        let Ok(kf8_header) = MobiHeader::parse(record) else {
            continue;
        };
        // KF8 部分的版本必须 ≥ 8；否则这不是 KF8 而是另一个 MOBI6 头（罕见）。
        if !kf8_header.is_kf8_capable() {
            continue;
        }
        // DRM 检查同样适用于 KF8 部分。
        if let DrmVerdict::Encrypted { .. } = drm::check_drm(&kf8_header) {
            warnings.push("KF8 部分同样受 DRM 保护".to_string());
            continue;
        }

        // KF8 的文本记录从 `record_start_offset`（0xA0，相对 KF8 头记录）
        // 或紧随其后的记录开始。两种写法都要支持。
        let text_start = if kf8_header.record_start_offset as usize > 0 {
            start + kf8_header.record_start_offset as usize
        } else {
            start + 1
        };
        if text_start >= db.len() {
            continue;
        }
        // 文本记录的终点由 firstNonBook 决定（它相对**文件**而不是相对部分）。
        let text_end = if kf8_header.first_non_book_index as usize > text_start {
            (kf8_header.first_non_book_index as usize).min(db.len())
        } else {
            db.len()
        };

        let text_records: Vec<Vec<u8>> = (text_start..text_end)
            .filter_map(|index| db.record(index).map(<[u8]>::to_vec))
            .collect();
        if text_records.is_empty() {
            continue;
        }
        return Some(Kf8Part {
            header: kf8_header,
            text_records,
            fdst_index: None,
        });
    }

    None
}

/// 收集"单头 + FDST"构造下的 KF8 文本记录。
///
/// # 记录区间怎么定
///
/// 实测（`.tome-mobi/368.kf8.mobi`）的字段组合：
///
/// ```text
///   text_length   = 329435        ← 解压后的正文长度
///   record_count  = 81            ← PalmDOC 头的文本记录数
///   record_size   = 4096
///   firstNonBook  = 0xFFFFFFFF    ← "文本区不连续"的约定值，**不可用**
///   firstContent  = 83            ← 指向 INDX（索引）记录，**不是正文起点**
///   kf8_boundary  = 96            ← 指向 FDST 记录
///
///   记录 1..81  = 压缩的正文（解压后 329436 字节，含 1 字节 trailing）
///   记录 82     = INDX 索引
///   记录 83 起   = 更多 INDX / 图片 / FDST / FLIS / FCIS
/// ```
///
/// 也就是说：**正文区间就是 PalmDOC 头自己给的 `1..1+record_count`**，
/// 与 MOBI6 路径完全一致。`firstContent = 83` 在这里指的是"第一条索引
/// 记录"，若拿它当正文起点就会去解压 INDX 与 JPEG 缩略图（实测的失败：
/// 正文变成了 `JFIF` 与二进制垃圾）。
///
/// 故本函数复用 `text_record_end()`（它优先用 `firstNonBook`，不可用时
/// 退回 PalmDOC 头的记录数），再加结构性魔数过滤做二次保险。
fn collect_kf8_fdst_records(db: &PalmDb<'_>, header: &MobiHeader) -> Vec<Vec<u8>> {
    let start = header.first_text_record();
    let end = header.text_record_end().min(db.len());
    if start >= end {
        return Vec::new();
    }

    // 二次过滤：结构性记录（FDST/FLIS/FCIS/INDX/HUFF…）不属于文本区。
    //
    // 实测中记录 82 以 `\x00\x00INDX` 开头（前面有两个填充字节），
    // 故过滤必须**容忍前导填充**——只比对开头 4 字节会漏掉它。这里在
    // 前 4 字节里做滑动查找。
    (start..end)
        .filter_map(|index| {
            let record = db.record(index)?;
            if is_structural_record(record) {
                return None;
            }
            Some(record.to_vec())
        })
        .collect()
}

/// 记录是否是 MOBI 的结构性记录（而非文本）。
///
/// 这些魔数都在 MobileRead Wiki 的 MOBI 词条里有记载，它们**只**出现在
/// 非文本记录上，故可作为排除依据。
fn is_structural_record(record: &[u8]) -> bool {
    const STRUCTURAL_MAGICS: [&[u8; 4]; 8] = [
        b"FDST", b"FLIS", b"FCIS", b"INDX", b"TAGX", b"IDXT", b"CTOC", b"HUFF",
    ];
    // 在**前 4 字节**里做滑动查找：实测的记录 82 是 `\x00\x00INDX...`
    // （对齐填充在前），只比对开头 4 字节会漏掉它，于是 INDX 会被当成
    // 正文喂给解压器，产出二进制垃圾。
    let head = &record[..record.len().min(4)];
    STRUCTURAL_MAGICS
        .iter()
        .any(|magic| head.windows(4).any(|window| window == *magic))
}

/// 收集 MOBI6 部分的文本记录。
fn collect_text_records(db: &PalmDb<'_>, header: &MobiHeader) -> Vec<Vec<u8>> {
    let start = header.first_text_record();
    let end = header.text_record_end().min(db.len());
    (start..end)
        .filter_map(|index| db.record(index).map(<[u8]>::to_vec))
        .collect()
}

/// 解压文本记录（T078/T079）。
fn decompress_text(
    header: &MobiHeader,
    records: &[Vec<u8>],
    db: &PalmDb<'_>,
) -> LocalResult<(Vec<u8>, Vec<String>, TextEncoding)> {
    let target = if header.record_size == 0 {
        text::DEFAULT_RECORD_SIZE
    } else {
        header.record_size as usize
    };
    let slices: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();

    let (raw, warnings) = match header.compression {
        Compression::None => {
            let (raw, warning) = text::concat_uncompressed(&slices, header.text_length);
            let mut warnings = Vec::new();
            if let Some(warning) = warning {
                warnings.push(warning);
            }
            (raw, warnings)
        }
        Compression::PalmDoc => text::decompress_all(&slices, target, header.text_length)?,
        Compression::HuffCdic => {
            // T079：HUFF/CDIC。需要从文件的非文本记录里找 HUFF 与 CDIC 两张表。
            let (raw, warnings) = huffcdic::decompress_document(db, header, &slices, target)?;
            (raw, warnings)
        }
        Compression::Unknown(code) => {
            return Err(LocalError::damaged(format!("未知的压缩算法码 {code}")));
        }
    };

    Ok((raw, warnings, header.encoding))
}

/// 读取 EXTH 元数据。
fn read_exth(
    record0: &[u8],
    header: &MobiHeader,
    warnings: &mut Vec<String>,
) -> Option<exth::Exth> {
    if !header.has_exth() {
        return None;
    }
    let offset = header.exth_offset();
    if offset >= record0.len() {
        warnings.push(format!(
            "MOBI 头声明存在 EXTH，但其偏移 {offset} 超出记录 0 的长度 {}",
            record0.len()
        ));
        return None;
    }
    match exth::parse(&record0[offset..]) {
        Ok(exth) => {
            warnings.extend(exth.warnings.iter().cloned());
            Some(exth)
        }
        Err(error) => {
            warnings.push(format!("EXTH 解析失败（元数据缺失）：{error}"));
            None
        }
    }
}

/// HTML → 纯文本（FR-022 的"归一化"）。
///
/// 返回 `(纯文本, 每个输出字符在原 HTML 里的字节偏移)`。
///
/// # 为什么要保留偏移映射
///
/// FR-022 要求 MOBI"可被随机定位"。定位的坐标必须能在**上层看到的文本**
/// 上寻址，而解析产出的文本是"去掉标签之后"的。若章节偏移在 HTML 上算、
/// 却在纯文本上使用，两者会因标签长度而不一致。
///
/// ## 为什么映射是"每个字符一个条目"
///
/// 更省内存的做法是只记"块级边界"的映射，但那样字符级定位（高亮、批注、
/// 书签都是字符级）就得在块内插值——而插值在标签边界处会错。
/// `usize` 每条 8 字节，一本书 30 万字符即 2.4MB，可接受。
fn html_to_text(html: &[u8]) -> (String, Vec<usize>) {
    let source = decode_html(html);
    let mut out = String::with_capacity(source.len() / 2);
    let mut map: Vec<usize> = Vec::with_capacity(source.len() / 2);

    let bytes = source.as_bytes();
    let mut index = 0usize;
    // 当前是否在 `<script>`/`<style>` 内部（内容不可见）。
    let mut skip_tag: Option<String> = None;

    while index < bytes.len() {
        if bytes[index] == b'<' {
            let tag_end = find_tag_end(&source, index);
            if tag_end <= index {
                // 不是标签（孤立的 `<`）：当普通字符。
                out.push('<');
                map.push(index);
                index += 1;
                continue;
            }
            let raw = &source[index + 1..tag_end];
            let is_closing = raw.starts_with('/');
            let name = tag_name(raw);

            if let Some(skipping) = skip_tag.clone() {
                if is_closing && name == skipping {
                    skip_tag = None;
                }
                index = tag_end + 1;
                continue;
            }
            // `head` **不在**这里：HTML 允许省略 `</head>`，而"跳到闭合
            // 标签"的跳过逻辑会在没有闭合标签时把后面的正文一起吞掉。
            // `script`/`style` 的闭合标签是必需的，故它们可以安全整段跳过。
            if !is_closing && matches!(name.as_str(), "script" | "style") {
                skip_tag = Some(name);
                index = tag_end + 1;
                continue;
            }

            // 块级标签 → 换行（章节切分依赖换行边界）。
            if is_block_tag(&name) && !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
                map.push(index);
            }
            // `img` 的替代文本是正文的一部分（`<img>` 不会出现在
            // `is_block_tag` 里——它是行内元素，故单独处理）。
            if name == "img" && !is_closing {
                if let Some(alt) = tag_attribute(raw, "alt") {
                    let alt = alt.trim();
                    if !alt.is_empty() {
                        if !out.ends_with('\n') && !out.is_empty() {
                            out.push('\n');
                            map.push(index);
                        }
                        for (offset, ch) in alt.char_indices() {
                            out.push(ch);
                            map.push(index + offset);
                        }
                        out.push('\n');
                        map.push(index);
                    }
                }
            }
            index = tag_end + 1;
            continue;
        }

        // 普通字符。
        //
        // ## 必须在这里也检查跳过状态
        //
        // `skip_tag` 的判定只写在标签分支里是不够的：`<style>` 的**内容**
        // 是普通字符（`p{color:red}`），它们走的是这条路径。若这里不检查，
        // 跳过只在"标签"上生效，而文本内容照样被输出——实测的 bug 正是
        // 如此（`p{color:red}var a=1;` 全部漏进正文）。
        if skip_tag.is_some() {
            let ch = source[index..].chars().next().unwrap_or(' ');
            index += ch.len_utf8();
            continue;
        }
        let ch = source[index..].chars().next().unwrap_or(' ');
        if ch == '&' {
            // 实体解码：解出的字符映射回实体起点。
            if let Some((decoded, consumed)) = decode_entity_at(&source[index..]) {
                out.push(decoded);
                map.push(index);
                index += consumed;
                continue;
            }
        }
        if ch == '\r' {
            // 行尾归一：`\r\n` 与孤立的 `\r` 都变一个 `\n`。
            if bytes.get(index + 1) == Some(&b'\n') {
                index += 1;
                continue;
            }
            out.push('\n');
            map.push(index);
            index += 1;
            continue;
        }
        // 折叠连续的空白（HTML 的空白语义）：任意空白序列折叠为**一个空格**。
        //
        // ## 两个都要做的动作
        //
        // 1. **折叠**：连续的空白只保留一个
        // 2. **归一化**：保留的那一个必须是空格（制表符与换行都要变成空格）
        //
        // 首版只做了 (1) 且只在"前一同类空白"时判定，于是
        // `"a\tb"` 会原样保留制表符、`"a \tb"` 会保留成 `"a \tb"`。
        // HTML 的空白折叠**不分种类**——空格、制表符、换行在渲染时
        // 是同一类分隔符，最终形态是一个普通空格。
        if ch == ' ' || ch == '\t' {
            let previous_is_whitespace = out
                .chars()
                .next_back()
                .is_some_and(|previous| previous == ' ' || previous == '\t' || previous == '\n');
            // 行首的空白直接丢掉（缩进不是内容）。
            if previous_is_whitespace || out.is_empty() {
                index += ch.len_utf8();
                continue;
            }
            // 归一化为空格（而不是推入原始的 `\t`）。
            out.push(' ');
            map.push(index);
            index += ch.len_utf8();
            continue;
        }
        out.push(ch);
        map.push(index);
        index += ch.len_utf8();
    }

    // 收尾：折叠连续空行并裁掉首尾空白。
    //
    // ## 为什么裁剪必须与映射一起做
    //
    // `trimmed_out.trim()` 会丢掉首尾的空白字符，而 `trimmed_map` 不做
    // 对应调整——两者就此错位（实测表现为 `map.len()` 比字符数多 1）。
    // 故裁剪放进 `collapse_blank_lines` 里，与映射同步进行。
    collapse_blank_lines(&out, &map)
}

/// 折叠连续空行（深缩进的 HTML 会产生大量空行）。
fn collapse_blank_lines(text: &str, map: &[usize]) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut mapping = Vec::with_capacity(map.len());
    let mut newline_run = 0usize;
    for (offset, ch) in text.char_indices() {
        if ch == '\n' {
            newline_run += 1;
            // 折叠：最多保留两个连续换行（一个空行）。
            if newline_run > 2 {
                continue;
            }
            // 行首的换行也丢掉（首尾裁剪的一部分）。
            if out.is_empty() {
                continue;
            }
        } else {
            newline_run = 0;
        }
        out.push(ch);
        mapping.push(map.get(offset).copied().unwrap_or(0));
    }
    // 尾部裁剪：与映射同步地丢掉末尾的空白。
    while out.ends_with(['\n', ' ', '\t']) {
        out.pop();
        mapping.pop();
    }
    (out, mapping)
}

/// 解码 HTML 字节（MOBI 文本已是声明的编码，先按 UTF-8 lossy）。
///
/// # 为什么这里是 lossy UTF-8 而不是走 `encoding` 模块
///
/// MOBI 的文本在解压后**应该**已经是声明的编码（MOBI6 常见为 cp1252，
/// KF8 为 UTF-8）。本实现统一按 UTF-8 处理：若编码字段不是 UTF-8，
/// 由 [`crate::mobi::header::TextEncoding`] 报告，调用方据此展示。
///
/// > \[UNCERTAIN] 非 UTF-8 的 MOBI6 文件（`encoding = 1252` 且含非 ASCII）
/// > 会在这里产生替换字符。真实语料里这类文件极少（Kindle 生态早已
/// > UTF-8 化），故当前不做转码。若日后出现，在此处按 `encoding` 字段
/// > 转码即可（接口已经把它传下来了）。
fn decode_html(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// 找到标签的结束位置（`>` 的下标）；考虑属性值里的引号。
fn find_tag_end(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut index = start + 1;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(q) if byte == q => quote = None,
            Some(_) => {}
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'>' => return index,
                _ => {}
            },
        }
        // 标签里出现 `<` 说明这不是标签（文本里的 `<`）。
        if quote.is_none() && byte == b'<' {
            return start;
        }
        index += 1;
    }
    start
}

/// 从标签原文里取标签名（小写，无属性）。
fn tag_name(raw: &str) -> String {
    let trimmed = raw.trim_start_matches('/');
    let end = trimmed
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(trimmed.len());
    trimmed[..end].to_ascii_lowercase()
}

/// 取标签的属性值。
fn tag_attribute(raw: &str, name: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find(name) {
        let position = cursor + offset;
        let preceded_ok = position == 0
            || lower[..position]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace());
        if preceded_ok {
            let after = &raw[position + name.len()..];
            if let Some(rest) = after.trim_start().strip_prefix('=') {
                let value = rest.trim_start();
                for quote in ['"', '\''] {
                    if let Some(inner) = value.strip_prefix(quote) {
                        if let Some(end) = inner.find(quote) {
                            return Some(inner[..end].to_string());
                        }
                    }
                }
                let end = value
                    .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                    .unwrap_or(value.len());
                return Some(value[..end].to_string());
            }
        }
        cursor = position + name.len();
    }
    None
}

/// 标签是否为块级（需要在前后换行）。
fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "br"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "li"
            | "tr"
            | "td"
            | "blockquote"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "aside"
            | "nav"
            | "figure"
            | "figcaption"
            | "pre"
            | "hr"
            | "table"
            | "ul"
            | "ol"
            | "dl"
            | "dt"
            | "dd"
    ) || HEADING_TAGS.contains(&name)
}

/// 在文本的某个位置尝试解码一个 HTML 实体，返回 `(字符, 消耗字节数)`。
fn decode_entity_at(text: &str) -> Option<(char, usize)> {
    let after = text.strip_prefix('&')?;
    let limit = after.len().min(12);
    let semicolon = after[..limit].find(';')?;
    let entity = &after[..semicolon];
    if entity.is_empty() {
        return None;
    }
    let decoded = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{A0}',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "ldquo" => '“',
        "rdquo" => '”',
        "lsquo" => '‘',
        "rsquo" => '’',
        "middot" => '·',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "sect" => '§',
        "para" => '¶',
        "deg" => '°',
        _ => {
            let number = entity.strip_prefix('#')?;
            let value = if let Some(hex) = number.strip_prefix(['x', 'X']) {
                u32::from_str_radix(hex, 16).ok()?
            } else {
                number.parse::<u32>().ok()?
            };
            char::from_u32(value)?
        }
    };
    Some((decoded, 1 + semicolon + 1))
}

/// 构建目录（guide 型 / 标题推断）。
fn build_toc(plain: &str, header: &MobiHeader, raw_html: &[u8]) -> Vec<MobiChapter> {
    let _ = header;

    // 方案 1：从归一化后的纯文本里找标题行。
    //
    // 为什么不用原始 HTML 里的 `<h1>`：指南型目录在 MOBI6 里常表现为
    // 正文中独占一行的标题文本，而 KF8 的 `<h1>` 在归一化后也会变成
    // 独占一行。两种形态在**纯文本**上统一了，故在这里做一次即可。
    let source = decode_html(raw_html);
    let headings = find_headings_in_html(&source).unwrap_or_default();

    if headings.len() >= 2 {
        // 有 HTML 标题：用它们的文本在纯文本里定位。
        let mut chapters: Vec<MobiChapter> = Vec::new();
        let mut search_from = 0usize;
        for title in headings {
            let title = title.trim().to_string();
            if title.is_empty() {
                continue;
            }
            let Some(found) = plain[search_from..].find(&title) else {
                continue;
            };
            let offset = search_from + found;
            chapters.push(MobiChapter {
                title: title.clone(),
                byte_offset: offset,
                char_offset: plain[..offset].chars().count(),
            });
            search_from = offset + title.len();
        }
        if chapters.len() >= 2 {
            return chapters;
        }
    }

    // 方案 2：按"独占一行且较短的非空行"推断章节。
    infer_chapters_from_lines(plain)
}

/// 从 HTML 里抽出 `<h1>`–`<h6>` 的纯文本内容。
fn find_headings_in_html(html: &str) -> Option<Vec<String>> {
    let mut headings = Vec::new();
    let bytes = html.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let end = find_tag_end(html, index);
        if end <= index {
            index += 1;
            continue;
        }
        let raw = &html[index + 1..end];
        if raw.starts_with('/') {
            index = end + 1;
            continue;
        }
        let name = tag_name(raw);
        if !HEADING_TAGS.contains(&name.as_str()) {
            index = end + 1;
            continue;
        }
        // 找到闭合标签。
        let closing = format!("</{name}");
        let Some(close_start) = html[end + 1..].to_ascii_lowercase().find(&closing) else {
            index = end + 1;
            continue;
        };
        let inner = &html[end + 1..end + 1 + close_start];
        let text = strip_tags(inner);
        let text = text.trim();
        if !text.is_empty() {
            headings.push(text.to_string());
        }
        index = end + 1 + close_start;
    }
    if headings.is_empty() {
        None
    } else {
        Some(headings)
    }
}

/// 去掉一段 HTML 里的标签（供标题提取）。
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'<' {
            let end = find_tag_end(html, index);
            if end > index {
                index = end + 1;
                continue;
            }
        }
        let ch = html[index..].chars().next().unwrap_or(' ');
        if ch == '&' {
            if let Some((decoded, consumed)) = decode_entity_at(&html[index..]) {
                out.push(decoded);
                index += consumed;
                continue;
            }
        }
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// 按"独占一行且较短"的行推断章节（无 HTML 标题时的兜底）。
///
/// # 判据为什么是"独占一行 + 短"而不是"含第X章"
///
/// 标题的写法五花八门（`第一章`、`CHAPTER I`、`1.`、`卷之一`），而
/// "独占一行且长度适中"是所有标题的共同形态。用形态而不是模式，能覆盖
/// 用户实际遇到的各种书而不需要维护一张模式表。
///
/// 长度上限取 60 字符：标题通常不超过 30 字，60 留了余量；而正文段落
/// 极少短到 60 字以内、又恰好独占一行。
fn infer_chapters_from_lines(plain: &str) -> Vec<MobiChapter> {
    const MAX_TITLE_CHARS: usize = 60;
    const MIN_TITLE_CHARS: usize = 2;

    let mut chapters = Vec::new();
    let mut char_cursor = 0usize;
    for line in plain.split_inclusive('\n') {
        let trimmed = line.trim();
        let char_count = trimmed.chars().count();
        if (MIN_TITLE_CHARS..=MAX_TITLE_CHARS).contains(&char_count) {
            // 标题不该以句末标点结尾（那是正文的特征）。
            let ends_like_sentence = trimmed.ends_with('。')
                || trimmed.ends_with('.')
                || trimmed.ends_with('！')
                || trimmed.ends_with('？')
                || trimmed.ends_with('!')
                || trimmed.ends_with('?');
            // 且不该是逗号连接的长句残片。
            let has_sentence_comma = trimmed.contains('，') && char_count > 30;
            if !ends_like_sentence && !has_sentence_comma {
                let byte_offset = plain[char_cursor..]
                    .find(trimmed)
                    .map(|offset| char_cursor + offset)
                    .unwrap_or(char_cursor);
                chapters.push(MobiChapter {
                    title: trimmed.to_string(),
                    byte_offset,
                    char_offset: plain[..byte_offset].chars().count(),
                });
            }
        }
        char_cursor += line.len();
    }

    if chapters.is_empty() {
        // 实在找不到：整本当一章（与 TXT 的按大小切分同一取向——
        // 宁可给出机械分章也不要产出零章节的书）。
        chapters.push(MobiChapter {
            title: "全文".to_string(),
            byte_offset: 0,
            char_offset: 0,
        });
    }
    chapters
}

/// 把 [`MobiChapter`] 映射为落库的 [`Chapter`] 列表。
pub fn to_chapters(chapters: &[MobiChapter], book_id: i64) -> Vec<Chapter> {
    chapters
        .iter()
        .enumerate()
        .map(|(index, chapter)| Chapter {
            id: 0,
            book_id,
            index_in_book: index as i32,
            title: chapter.title.clone(),
            url: None,
            // MOBI 用**字节偏移**定位（与 TXT 同一字段）——因为解析产出的
            // 纯文本是内存里的一个 `String`，偏移直接可用。
            byte_offset: Some(chapter.byte_offset as i64),
            epub_entry: None,
            is_cached: false,
            is_read: false,
            word_count: None,
        })
        .collect()
}

/// 供 provider 使用的全文提取（对外暴露的稳定接口）。
pub fn extract_text(bytes: &[u8]) -> LocalResult<String> {
    Ok(parse(bytes)?.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_drops_tags_and_keeps_content() {
        let (text, _) = html_to_text(b"<p>Hello <b>world</b></p>");
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn block_tags_become_newlines() {
        let (text, _) = html_to_text(b"<p>First</p><p>Second</p>");
        assert_eq!(text, "First\nSecond");
    }

    #[test]
    fn script_and_style_are_dropped() {
        let (text, _) = html_to_text(
            b"<head><style>p{color:red}</style></head><body><script>var a=1;</script><p>Real</p></body>",
        );
        assert_eq!(text, "Real");
    }

    #[test]
    fn entities_are_decoded_in_text() {
        let (text, _) = html_to_text(b"<p>A &amp; B &lt;tag&gt; &#x4E2D;</p>");
        assert_eq!(text, "A & B <tag> 中");
    }

    #[test]
    fn whitespace_is_collapsed() {
        let (text, _) = html_to_text(b"<p>a     b\t\tc</p>");
        assert_eq!(text, "a b c");
    }

    #[test]
    fn blank_line_runs_are_collapsed() {
        let (text, _) = html_to_text(b"<p>a</p><br/><br/><br/><br/><p>b</p>");
        assert!(!text.contains("\n\n\n"), "连续空行应被折叠，实际 {text:?}");
    }

    #[test]
    fn offset_map_tracks_every_output_character() {
        let source = b"<p>abc</p>";
        let (text, map) = html_to_text(source);
        assert_eq!(text, "abc");
        assert_eq!(map.len(), text.chars().count());
        // 'a' 在 HTML 里的偏移是 3。
        assert_eq!(map[0], 3);
    }

    #[test]
    fn offset_map_points_into_the_source() {
        let source = b"<div>xyz</div>";
        let (text, map) = html_to_text(source);
        for (index, ch) in text.chars().enumerate() {
            let offset = map[index];
            if ch.is_alphabetic() {
                assert_eq!(source[offset], ch as u8, "偏移 {offset} 处应是 {ch:?}");
            }
        }
    }

    #[test]
    fn crlf_is_normalised() {
        let (text, _) = html_to_text(b"<p>a\r\nb</p>");
        assert!(!text.contains('\r'));
        assert_eq!(text, "a\nb");
    }

    #[test]
    fn image_alt_text_is_kept() {
        let (text, _) = html_to_text(r#"<p><img src="x.png" alt="一朵花"/></p>"#.as_bytes());
        assert!(text.contains("一朵花"), "实际 {text:?}");
    }

    #[test]
    fn tags_with_attributes_containing_gt_are_handled() {
        // 属性值里出现 `>` 是合法的（如 `title="a > b"`）。
        let (text, _) = html_to_text(br#"<p title="a > b">content</p>"#);
        assert_eq!(text, "content");
    }

    #[test]
    fn unknown_entities_are_left_alone() {
        let (text, _) = html_to_text(b"<p>&weird;</p>");
        assert_eq!(text, "&weird;");
    }

    #[test]
    fn headings_are_extracted_from_html() {
        let headings =
            find_headings_in_html("<h1>Chapter One</h1><p>x</p><h2>Section A</h2><p>y</p>")
                .expect("应有标题");
        assert_eq!(headings, vec!["Chapter One", "Section A"]);
    }

    #[test]
    fn headings_with_inline_markup_are_cleaned() {
        let headings = find_headings_in_html("<h1><i>Italic</i> Title</h1>").expect("应有标题");
        assert_eq!(headings, vec!["Italic Title"]);
    }

    #[test]
    fn infer_chapters_skips_sentences() {
        let text = "第一章 开端\n这是一段正文，里面有逗号与句号。\n第二章 承接\n更多的正文内容。\n";
        let chapters = infer_chapters_from_lines(text);
        let titles: Vec<&str> = chapters.iter().map(|c| c.title.as_str()).collect();
        assert!(titles.contains(&"第一章 开端"), "实际 {titles:?}");
        assert!(titles.contains(&"第二章 承接"), "实际 {titles:?}");
        // 以句号结尾的行不该被当成标题。
        assert!(!titles.iter().any(|t| t.ends_with('。')), "实际 {titles:?}");
    }

    #[test]
    fn infer_chapters_falls_back_to_a_single_chapter() {
        let chapters = infer_chapters_from_lines("");
        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].byte_offset, 0);
    }

    #[test]
    fn build_toc_uses_html_headings_when_available() {
        let raw = b"<h1>Chapter One</h1><p>text one</p><h1>Chapter Two</h1><p>text two</p>";
        let (plain, _) = html_to_text(raw);
        let header = dummy_header();
        let toc = build_toc(&plain, &header, raw);
        assert_eq!(toc.len(), 2);
        assert_eq!(toc[0].title, "Chapter One");
        assert!(toc[1].byte_offset > toc[0].byte_offset);
    }

    fn dummy_header() -> MobiHeader {
        MobiHeader {
            compression: Compression::PalmDoc,
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

    #[test]
    fn chapter_offsets_are_usable_for_range_reads() {
        let raw = b"<h1>Chapter One</h1><p>first body</p><h1>Chapter Two</h1><p>second body</p>";
        let (plain, _) = html_to_text(raw);
        let toc = build_toc(&plain, &dummy_header(), raw);
        assert!(toc.len() >= 2);
        let second = &toc[1];
        let slice = &plain[second.byte_offset..];
        assert!(
            slice.starts_with(&second.title),
            "偏移处应是标题，实际 {:?}",
            &slice[..slice.len().min(40)]
        );
    }

    #[test]
    fn char_offsets_match_byte_offsets_through_multibyte_text() {
        let raw = "<h1>第一章</h1><p>中文正文</p><h1>第二章</h1><p>更多正文</p>".as_bytes();
        let (plain, _) = html_to_text(raw);
        let toc = build_toc(&plain, &dummy_header(), raw);
        if let Some(second) = toc.get(1) {
            assert!(
                second.byte_offset > second.char_offset,
                "中文下字节偏移应更大"
            );
            assert_eq!(
                plain[..second.byte_offset].chars().count(),
                second.char_offset
            );
        }
    }

    #[test]
    fn to_chapters_maps_positions_and_indices() {
        let chapters = vec![
            MobiChapter {
                title: "A".to_string(),
                byte_offset: 0,
                char_offset: 0,
            },
            MobiChapter {
                title: "B".to_string(),
                byte_offset: 100,
                char_offset: 50,
            },
        ];
        let mapped = to_chapters(&chapters, 3);
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].index_in_book, 0);
        assert_eq!(mapped[1].byte_offset, Some(100));
        assert!(mapped.iter().all(|c| c.book_id == 3));
        assert!(mapped.iter().all(|c| c.epub_entry.is_none()));
    }

    #[test]
    fn tag_name_extraction_handles_attributes_and_closing() {
        assert_eq!(tag_name("p"), "p");
        assert_eq!(tag_name("/p"), "p");
        assert_eq!(tag_name("img src=\"x\""), "img");
        assert_eq!(tag_name("H1 class=\"a\""), "h1");
        assert_eq!(tag_name("br/"), "br");
    }

    #[test]
    fn block_tag_recognition() {
        assert!(is_block_tag("p"));
        assert!(is_block_tag("h3"));
        assert!(is_block_tag("li"));
        assert!(!is_block_tag("span"));
        assert!(!is_block_tag("b"));
    }

    #[test]
    fn adjust_to_char_boundary_clamps_safely() {
        let text = "中文";
        assert_eq!(adjust_to_char_boundary(text, 0), 0);
        assert_eq!(adjust_to_char_boundary(text, 3), 3);
        assert_eq!(adjust_to_char_boundary(text, 999), text.len());
        // 落在多字节字符中间时向前调整。
        assert!(text.is_char_boundary(adjust_to_char_boundary(text, 1)));
    }
}
