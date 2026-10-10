//! \[DONE] PalmDOC 解压与 **trailing data 剥离**（tasks.md T078 / research R4）
//!
//! # research R4 点名的最大风险
//!
//! > **真正的难点**：trailing data 剥离（**错了不报错、静默丢字**）
//!
//! 这是整个 MOBI 实现里唯一一个"错了也不会崩、只会让正文少一截"的地方。
//! 正因为没有错误信号，它必须被**显式测试**而不是靠"读起来正常"来判断。
//!
//! # 什么是 trailing data
//!
//! ```text
//!   PalmDOC 头声明 text_length = 329435 字节（未压缩）
//!   ┌────────────────────────────────────────────────────┐
//!   │ 80 条文本记录，各解压到 ≤4096 字节                    │
//!   │ ...（第 81 条解压后跨过 329435 这个边界）...          │
//!   │ 第 81 条：[ 真正的正文尾部 ][ 多余的 1 字节 ]          │  ← trailing data
//!   └────────────────────────────────────────────────────┘
//! ```
//!
//! MOBI 用**定长记录**存文本：每条记录解压后最多 `record_size`（通常 4096）
//! 字节。而文本总长度 `text_length` 通常不是 4096 的整数倍，故**最后一条
//! 记录解压出来会比 `text_length` 多出若干字节**。多出的那部分是生成工具
//! 的填充（或缓冲残留），不属于正文。
//!
//! ## 实测（本仓库 `.tome-mobi/368.kf8.mobi`，Gutenberg 公有领域）
//!
//! ```text
//!   text_length          = 329435
//!   81 条记录解压后总长   = 329436
//!   多出                  = 1 字节（末尾的 NUL）
//! ```
//!
//! 1 字节看似无害，但如果实现**不裁剪**：
//!
//! - 正文末尾多一个 NUL，落进章节内容后会成为渲染层的意外字符
//! - 更严重的是**偏移错位**：若某个版本的生成工具多出的是几百字节，
//!   而实现按"总解压长度"来算章节偏移，所有偏移就会整体偏移
//! - 反向的错误（**裁多了**）更隐蔽：正文末尾被砍掉几个字，用户不会察觉
//!
//! 故本模块的 [`strip_trailing`] 是 T078 必须过的关卡，且有专门测试对照
//! 真实文件与合成边界。
//!
//! # PalmDOC 解压算法（公开逐字节算法）
//!
//! MobileRead Wiki 的 PalmDOC 词条给出完整算法。字节分四类：
//!
//! | 字节 | 含义 |
//! |---|---|
//! | `0x00` | 字面 NUL |
//! | `0x01`–`0x08` | 后随 n 个字面字节（原样复制） |
//! | `0x09`–`0x7F` | 字面字符（ASCII 大部分） |
//! | `0x80`–`0xBF` | 双字节序列：`(c & 0x3F) << 8 \| next` → 距离/长度的 LZ77 回引 |
//! | `0xC0`–`0xFF` | 字面字符 = `c ^ 0x80`（带空格压缩：输出空格 + 该字符） |
//!
//! ## 双字节序列的位域
//!
//! ```text
//!   16 位值 v = (b1 << 8) | b2   （b1 在 0x80-0xBF）
//!   距离 m = (v >> 3) & 0x7FF    ← 11 位
//!   长度 n = (v & 0x07) + 3      ← 3 位，加 3
//! ```
//!
//! **`m == 0` 是特例**（MobileRead 明确的边界）：表示"输出 n 个空格"
//! （而非回引）。若实现漏掉这条，会去读"距离 0"导致无限循环或错误复制。

use crate::error::LocalResult;

/// 一条记录解压时的默认目标大小（PalmDOC 头的 `record_size` 的常见值）。
pub const DEFAULT_RECORD_SIZE: usize = 4096;

/// 解压一条 PalmDOC 记录。
///
/// # 参数
///
/// - `input`：压缩后的字节
/// - `target`：期望解压出的字节数上限。到达上限即停止——
///   这是 PalmDOC 的约定（记录大小固定），且能防损坏数据的无限膨胀。
pub fn decompress_record(input: &[u8], target: usize) -> LocalResult<Vec<u8>> {
    let target = target.max(1);
    // 预留目标大小：记录几乎总是解压到接近 target。
    let mut out: Vec<u8> = Vec::with_capacity(target);
    let mut cursor = 0usize;

    while cursor < input.len() && out.len() < target {
        let byte = input[cursor];
        cursor += 1;

        match byte {
            // 1. 字面 NUL。
            0x00 => out.push(0x00),

            // 2. 后随 n 个字面字节。
            0x01..=0x08 => {
                let count = byte as usize;
                let available = input.len().saturating_sub(cursor).min(count);
                if available < count {
                    // 损坏：声明的字面串超出输入末尾。按可用部分输出，
                    // 不报错——末尾记录被截断是真实存在的情形，
                    // 让用户读到大部分总比整章失败好。
                    out.extend_from_slice(&input[cursor..cursor + available]);
                    cursor += available;
                } else {
                    out.extend_from_slice(&input[cursor..cursor + count]);
                    cursor += count;
                }
            }

            // 3. 字面字符（ASCII 可打印区）。
            0x09..=0x7F => out.push(byte),

            // 4. 双字节 LZ77 回引。
            0x80..=0xBF => {
                if cursor >= input.len() {
                    // 只有半个双字节序列：损坏，停止。
                    break;
                }
                let next = input[cursor];
                cursor += 1;
                let value = ((byte as u16) << 8) | next as u16;
                let distance = ((value >> 3) & 0x07FF) as usize;
                let length = ((value & 0x0007) as usize) + 3;

                if distance == 0 {
                    // **MobileRead 明确的特例**：距离 0 表示 n 个空格。
                    // 漏掉这条会去读 out[-0]（越界或错复制）。
                    let spaces = length.min(target.saturating_sub(out.len()));
                    out.extend(std::iter::repeat_n(b' ', spaces));
                } else if distance <= out.len() {
                    // 逐字节复制（不能用 `copy_from_slice`：源与目标重叠，
                    // 且距离可能小于长度——那正是 LZ77 的"自引用"用法，
                    // 是压缩率的来源之一）。
                    let start = out.len() - distance;
                    for offset in 0..length {
                        if out.len() >= target {
                            break;
                        }
                        let source = start + offset;
                        if source >= out.len() {
                            break;
                        }
                        let value = out[source];
                        out.push(value);
                    }
                } else {
                    // 回引超出了已产出的内容：数据损坏。停止而不 panic。
                    break;
                }
            }

            // 5. 带空格压缩的字面字符。
            0xC0..=0xFF => {
                out.push(0x20);
                out.push(byte ^ 0x80);
            }
        }
    }

    Ok(out)
}

/// 从解压后的完整文本里裁掉 trailing data（**R4 的高危点**）。
///
/// # 判据
///
/// `text_length` 是 PalmDOC 头声明的**未压缩文本长度**（权威值——
/// 生成工具按它写入）。裁掉它之后的一切。
///
/// ## 为什么信任 `text_length` 而不是"解压结果本身就是正文"
///
/// 定长记录是**存储**的机制，不是文本的语义。最后一条记录必然被填充到
/// 记录大小（或至少超出真实文本），故解压结果**必然**包含非正文的尾巴。
/// `text_length` 是唯一权威的正文边界。
///
/// ## 两个方向的防御
///
/// 1. **声明值大于实际**（文本比声明的短）：不裁，但要报诊断——
///    那意味着文件损坏（截断），而**静默接受**会让用户读到不完整的书
/// 2. **声明值小于实际**（文本比声明的长）：裁到声明值。这正是正常工作路径
pub fn strip_trailing(text: &[u8], text_length: u32) -> (Vec<u8>, Option<String>) {
    let declared = text_length as usize;
    match declared.cmp(&text.len()) {
        std::cmp::Ordering::Less => {
            let mut trimmed = text[..declared].to_vec();
            // 二次防御：定长记录有时留下 NUL 填充。**只裁末尾的 NUL**，
            // 不动正文里的（正文里 NUL 不可能出现，但保守起见只裁尾部）。
            while trimmed.last() == Some(&0x00) {
                trimmed.pop();
            }
            (trimmed, None)
        }
        // 声明长度与实际相同：正文完整，但定长记录的 NUL 填充可能被算进了
        // 声明长度里（部分生成工具如此）。故同样要裁掉末尾的 NUL。
        std::cmp::Ordering::Equal => {
            let mut trimmed = text.to_vec();
            while trimmed.last() == Some(&0x00) {
                trimmed.pop();
            }
            (trimmed, None)
        }
        std::cmp::Ordering::Greater => (
            text.to_vec(),
            Some(format!(
                "PalmDOC 头声明文本长度 {declared}，但实际解压出 {} 字节（文件可能被截断）",
                text.len()
            )),
        ),
    }
}

/// 解压全部文本记录并剥离 trailing data。
///
/// # 参数
///
/// - `records`：文本记录的**压缩**字节（按顺序）
/// - `target`：每条记录的目标解压大小
/// - `text_length`：PalmDOC 头声明的正文长度
pub fn decompress_all(
    records: &[&[u8]],
    target: usize,
    text_length: u32,
) -> LocalResult<(Vec<u8>, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut combined: Vec<u8> = Vec::with_capacity(text_length as usize + target);
    for (index, record) in records.iter().enumerate() {
        // 已经够长了就不必再解压后续记录（`text_length` 是权威边界，
        // 多解压出来的部分反正会被裁掉）。这是一条实打实的性能优化：
        // 有些文件把图片也放在"文本记录"范围内。
        if combined.len() >= text_length as usize && text_length > 0 {
            warnings.push(format!(
                "第 {index} 条起的文本记录未解压（正文已达到声明的 {text_length} 字节）"
            ));
            break;
        }
        match decompress_record(record, target) {
            Ok(chunk) => combined.extend_from_slice(&chunk),
            Err(error) => {
                warnings.push(format!("第 {index} 条文本记录解压失败：{error}"));
            }
        }
    }

    let (text, warning) = strip_trailing(&combined, text_length);
    if let Some(warning) = warning {
        warnings.push(warning);
    }
    Ok((text, warnings))
}

/// 解压"无压缩"（compression = 1）的文本记录。
///
/// 不压缩的 MOBI 极少，但规范允许，且实现在这里退化会让用户完全读不了。
pub fn concat_uncompressed(records: &[&[u8]], text_length: u32) -> (Vec<u8>, Option<String>) {
    let mut combined = Vec::with_capacity(text_length as usize);
    for record in records {
        combined.extend_from_slice(record);
    }
    strip_trailing(&combined, text_length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_copied_verbatim() {
        // 0x09-0x7F 直接是字面字符。
        let input = b"Hello world";
        assert_eq!(
            decompress_record(input, 64).expect("解压"),
            b"Hello world".to_vec()
        );
    }

    #[test]
    fn nul_byte_is_literal() {
        assert_eq!(decompress_record(&[0x00], 8).expect("解压"), vec![0x00]);
    }

    #[test]
    fn short_literal_run_is_expanded() {
        // 0x03 表示"后随 3 个字面字节"。
        let input = [0x03, b'a', b'b', b'c', b'Z'];
        assert_eq!(
            decompress_record(&input, 64).expect("解压"),
            b"abcZ".to_vec()
        );
    }

    #[test]
    fn all_literal_run_lengths_work() {
        for count in 1u8..=8 {
            let mut input = vec![count];
            input.extend(std::iter::repeat_n(b'x', count as usize));
            let out = decompress_record(&input, 64).expect("解压");
            assert_eq!(out, vec![b'x'; count as usize], "长度 {count} 的字面串");
        }
    }

    #[test]
    fn space_compression_produces_space_plus_char() {
        // 0xC0-0xFF：输出空格 + (c ^ 0x80)。
        // 0xE1 = 0xC0 | 0x21 → 空格 + 'a'。
        let out = decompress_record(&[0xE1], 8).expect("解压");
        assert_eq!(out, b" a".to_vec());
    }

    #[test]
    fn distance_zero_means_spaces_not_backreference() {
        // MobileRead 明确的特例。构造 value 使 distance = 0。
        // value = 0x8000 → distance = (0x8000 >> 3) & 0x7FF = 0；length = 0 + 3 = 3。
        let input = [0x80, 0x00];
        let out = decompress_record(&input, 64).expect("解压");
        assert_eq!(out, b"   ".to_vec(), "距离 0 应输出 3 个空格");
    }

    #[test]
    fn backreference_copies_from_earlier_output() {
        // 先写 "abc"，再用距离 3 回引 3 字节 → "abcabc"。
        //
        // length 的编码是 `(value & 7) + 3`。要 length = 3 则低 3 位为 0。
        // distance = 3 → value 的高 11 位 &= 0x7FF 后为 3 → value = 3 << 3 = 0x18。
        // 但首字节必须在 0x80-0xBF：0x80 | (0x18 >> 8) = 0x80，第二字节 0x18。
        let input = [b'a', b'b', b'c', 0x80, 0x18];
        let out = decompress_record(&input, 64).expect("解压");
        assert_eq!(out, b"abcabc".to_vec());
    }

    #[test]
    fn overlapping_backreference_repeats_correctly() {
        // LZ77 的经典用法：距离 1、长度 5 → 从已有的最后一个字节起,
        // **边复制边增长**地重复 5 次，故总长 = 1 + 5 = 6。
        //
        // 这正是"必须逐字节复制而不能用 `copy_from_slice`"的原因：
        // 源区间在复制过程中被目标区间不断扩展（自引用）。
        // distance = 1 → value = 1 << 3 = 0x08（低 3 位为 0 → length = 3）。
        // 要 length = 5，低 3 位需为 2：value = 0x08 | 2 = 0x0A。
        let input = [b'x', 0x80, 0x0A];
        let out = decompress_record(&input, 64).expect("解压");
        assert_eq!(out, b"xxxxxx".to_vec(), "自引用回引必须逐字节复制");
    }

    #[test]
    fn compression_ratio_is_achieved_on_repetitive_text() {
        // 一段重复文本应显著短于原文——这是"解压实现了"的粗判。
        let mut compressed = vec![0x08, b'p', b'a', b't', b't', b'e', b'r', b'n', b'!'];
        // 8 字节字面量之后回引：距离必须 ≤ 已产出长度（8）。
        // 距离 8、长度 8：value = (8 << 3) | (8 - 3) = 0x40 | 5 = 0x45
        // 首字节需 >= 0x80：0x80 | (0x45 >> 8) = 0x80，第二字节 0x45。
        compressed.extend_from_slice(&[0x80, 0x45]);
        let out = decompress_record(&compressed, 256).expect("解压");
        assert!(out.starts_with(b"pattern!"));
        assert_eq!(out.len(), 16, "8 字节字面量 + 8 字节回引");
        assert_eq!(&out[..16], b"pattern!pattern!");
    }

    #[test]
    fn output_is_capped_at_the_target_size() {
        // 到达目标大小即停（PalmDOC 的定长记录约定）。
        let input = vec![b'z'; 100];
        let out = decompress_record(&input, 10).expect("解压");
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn truncated_literal_run_does_not_panic() {
        // 声明 5 个字面字节但只有 2 个：按可用部分输出。
        let input = [0x05, b'a', b'b'];
        let out = decompress_record(&input, 64).expect("解压");
        assert_eq!(out, b"ab".to_vec());
    }

    #[test]
    fn dangling_two_byte_sequence_does_not_panic() {
        let input = [b'a', 0x80];
        let out = decompress_record(&input, 64).expect("解压");
        assert_eq!(out, b"a".to_vec());
    }

    #[test]
    fn backreference_beyond_output_does_not_panic() {
        // 距离大于已产出内容：损坏数据。停止而不是越界。
        let input = [b'a', 0x80, 0xFF];
        let out = decompress_record(&input, 64).expect("解压");
        assert_eq!(out, b"a".to_vec());
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert!(decompress_record(&[], 64).expect("解压").is_empty());
    }

    #[test]
    fn binary_marker_then_literals() {
        // 0x01 是"1 个后续字面字节"，与 0x00（字面 NUL）必须区分开。
        let input = [0x01, 0x41, 0x42];
        assert_eq!(decompress_record(&input, 64).expect("解压"), b"AB".to_vec());
    }

    // ------------------------------------------------------------------
    // trailing data 剥离：R4 点名的高危点
    // ------------------------------------------------------------------

    #[test]
    fn trailing_bytes_are_removed() {
        // 这是 R4 的核心场景：解压结果比声明的正文长。
        let text = b"real content PLUS EXTRA";
        let (trimmed, warning) = strip_trailing(text, 12);
        assert_eq!(trimmed, b"real content".to_vec());
        assert!(warning.is_none(), "正常裁剪不该产生诊断");
    }

    #[test]
    fn exact_length_is_left_untouched() {
        let text = b"exactly ten".to_vec();
        let (trimmed, warning) = strip_trailing(&text, text.len() as u32);
        assert_eq!(trimmed, text);
        assert!(warning.is_none());
    }

    #[test]
    fn trailing_nul_padding_is_also_trimmed() {
        // 定长记录常留下 NUL 填充。声明长度若把它算进去了，末尾还会有 NUL。
        let text = b"content\x00\x00";
        let (trimmed, _) = strip_trailing(text, text.len() as u32);
        assert_eq!(trimmed, b"content".to_vec(), "末尾 NUL 应被裁掉");
    }

    #[test]
    fn short_text_is_reported_not_silently_accepted() {
        // 声明比实际长 = 文件被截断。**不能静默**——用户会读到残缺的书。
        let text = b"short";
        let (kept, warning) = strip_trailing(text, 100);
        assert_eq!(kept, b"short".to_vec(), "已解压的内容全部保留");
        assert!(warning.is_some(), "截断必须报告");
        let warning = warning.unwrap_or_default();
        assert!(
            warning.contains("100") && warning.contains("5"),
            "诊断应含声明的与实际的两个长度，实际 {warning:?}"
        );
    }

    #[test]
    fn empty_text_with_zero_length_is_fine() {
        let (trimmed, warning) = strip_trailing(&[], 0);
        assert!(trimmed.is_empty());
        assert!(warning.is_none());
    }

    #[test]
    fn real_file_boundary_shape_is_handled() {
        // 复现实测到的形态（368.kf8.mobi）：声明 329435，实际解压 329436，
        // 多出的 1 字节是末尾的 NUL。
        let mut text = vec![b'x'; 329435];
        text.push(0x00);
        let (trimmed, warning) = strip_trailing(&text, 329435);
        assert_eq!(trimmed.len(), 329435);
        assert_eq!(trimmed.last(), Some(&b'x'));
        assert!(warning.is_none());
    }

    // ------------------------------------------------------------------
    // 组合路径
    // ------------------------------------------------------------------

    #[test]
    fn decompress_all_strips_trailing_across_records() {
        // 两条记录，各产出 4 字节字面内容；声明正文只有 6 字节。
        let record_a: Vec<u8> = vec![0x04, b'a', b'b', b'c', b'd'];
        let record_b: Vec<u8> = vec![0x04, b'e', b'f', b'g', b'h'];
        let records = [record_a.as_slice(), record_b.as_slice()];
        let (text, warnings) = decompress_all(&records, 64, 6).expect("解压");
        assert_eq!(text, b"abcdef".to_vec());
        assert!(warnings.is_empty());
    }

    #[test]
    fn decompress_all_stops_early_once_the_declared_length_is_reached() {
        let record: Vec<u8> = vec![0x04, b'a', b'b', b'c', b'd'];
        let records = [record.as_slice(), record.as_slice(), record.as_slice()];
        let (text, warnings) = decompress_all(&records, 64, 4).expect("解压");
        assert_eq!(text, b"abcd".to_vec());
        assert!(
            !warnings.is_empty(),
            "提前停止应留诊断（否则用户不知道有记录未解压）"
        );
    }

    #[test]
    fn uncompressed_records_are_concatenated_and_trimmed() {
        let a = b"hello".as_slice();
        let b = b" world extra".as_slice();
        let (text, warning) = concat_uncompressed(&[a, b], 11);
        assert_eq!(text, b"hello world".to_vec());
        assert!(warning.is_none());
    }

    #[test]
    fn decompression_never_panics_on_arbitrary_bytes() {
        // 不可信输入的粗暴验证：一堆随机字节不该让解压 panic。
        let mut seed: u32 = 0x1234_5678;
        for _ in 0..200 {
            let mut input = Vec::new();
            for _ in 0..64 {
                // 简单的线性同余，避免引入 rand 依赖。
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                input.push((seed >> 16) as u8);
            }
            let _ = decompress_record(&input, 4096);
        }
    }
}
