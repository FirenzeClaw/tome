//! \[DONE] 本地文件的字符集检测与解码（T076 / FR-020）
//!
//! 本地 TXT 与 HTTP 响应在编码问题上是**同一类问题**但**条件不同**：
//!
//! | 条件 | HTTP 响应（tome-net） | 本地文件（本模块） |
//! |---|---|---|
//! | 声明来源 | `Content-Type` 头 / `<meta>` | 无（TXT 没有地方声明） |
//! | BOM | 可能有 | 可能有 |
//! | TLD 线索 | 有（主机名） | 无 |
//! | 可否全文检测 | 通常不能（响应流） | **可以**（文件在本地，可全量采样） |
//!
//! ## 为什么不复用 `tome-net::charset`
//!
//! 架构铁律：`tome-local` **不依赖** `tome-net`（见 crate 文档：本地书全程离线，
//! 这条依赖边若出现即为架构违规）。故本模块自带检测路径，但**判据与取向
//! 与 tome-net 保持一致**——两边都用 `chardetng` + "替换字符数择优"，
//! 差异只在"没有声明"这一前提（本地文件本就无声明）。
//!
//! ## 本模块的核心取向：**永远给出可读文本**
//!
//! 编码检测的失败模式不是"报错"而是"静默乱码"——用户看到一整本乱码书，
//! 却没有任何错误提示。故本层的契约是：**绝不返回 Err**。所有分支最终都会
//! 落到某个候选编码的 lossy 解码结果上（FR-020 要求 TXT 导入可用）。
//!
//! 唯一的例外是 `decode_with_declared`——当调用方**显式**指定编码而该编码
//! 不存在时返回 `None`（而非静默改用别的方式），因为那是调用方的输入错误，
//! 应当被报告而不是被猜测覆盖。
//!
//! # 关键入口
//!
//! - [`detect_and_decode`] —— 字节 → 文本 + 实际编码名（主入口）
//! - [`DecodedText`] —— 解码结果

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, GB18030, UTF_8};

/// 无声明时的回退候选次序（与 tome-net 的取向一致）。
///
/// 本项目面向中文阅读场景，UTF-8（含无声明直出）与 GB18030（兼容 GBK/GB2312）
/// 覆盖绝大多数文件。次序即同分时的优先级。
const FALLBACK_CANDIDATES: [&Encoding; 3] = [UTF_8, GB18030, encoding_rs::BIG5];

/// 检测采样上限（字节）。
///
/// ## 为什么采样而不是全量检测
///
/// SC-004 要求 100MB TXT 的导入 + 目录识别在 8 秒内完成。`chardetng` 的
/// 检测是 O(n) 的字节扫描，全量喂 100MB 是实打实的几百毫秒开销，而**检测
/// 精度在采样之后就不再提升**——编码是文件的全局属性，前若干 MB 足以判定
/// （`chardetng` 自身的文档即按"喂一个块"设计，并推荐大块喂入以省去
/// feed 调用开销）。
///
/// 取 1 MiB：对中文文本约 33 万汉字，远超任何统计检测器的收敛所需
/// （`chardetng` 在几 KB 量级即收敛），同时把大文件的检测开销钉死为常数。
const DETECTION_SAMPLE_BYTES: usize = 1024 * 1024;

/// 解码结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    /// 解码后的文本。**永远可读**——最坏情况也是 lossy 解码的结果。
    pub text: String,
    /// 实际采用的编码名（形如 `UTF-8` / `GBK` / `gb18030` / `Big5`）。
    ///
    /// 该值写入 `Book.charset`（data-model.md），供上层展示与故障排查。
    pub charset: String,
    /// 解码时是否出现了替换字符（U+FFFD）。
    ///
    /// 非零意味着"这段字节在本编码下并非全部合法"——正文可能有损。
    /// 本层把文本照常给出（可用性优先），但让调用方**能**据此提示用户。
    pub had_errors: bool,
}

/// \[DONE] FR-020 的主入口：字节 → 文本 + 编码名。
///
/// 决策次序（每一步命中即返回）：
///
/// 1. **BOM 优先**：BOM 是字节层面的**事实**，优先于任何检测。
/// 2. **检测**：`chardetng` 喂入采样块；给出**单字节编码**视为"没猜出来"
///    （理由见下）。
/// 3. **兜底**：`FALLBACK_CANDIDATES` 逐个解码，取替换字符最少者。
///
/// # 为什么"单字节编码的检测结果"等于没猜出来
///
/// `chardetng` 的兜底答案就是 windows-1252 这类单字节码页。单字节编码几乎
/// 对任何字节序列都不报错（每个字节都映射到某个字符），所以它能"成功解码"
/// 一个 GBK 文件并产出一整篇乱码、替换字符数为 0，用"择优"规则根本挑不出
/// 它的错。**单字节编码的零错误是个假信号**，故降级为"未确信"。
///
/// 这与 `tome-net::charset::detect` 的处置完全一致——两条路径共享同一判据，
/// 以免同一份内容经网络与经本地得到不同的编码结论。
pub fn detect_and_decode(bytes: &[u8]) -> DecodedText {
    if bytes.is_empty() {
        return DecodedText {
            text: String::new(),
            charset: UTF_8.name().to_string(),
            had_errors: false,
        };
    }

    // 1. BOM 是字节层面的事实。
    if let Some((bom_encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, had_errors) = bom_encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return DecodedText {
            text: text.into_owned(),
            charset: bom_encoding.name().to_string(),
            had_errors,
        };
    }

    // 2. 检测。
    if let Some(detected) = detect_encoding(bytes) {
        let (text, had_errors) = detected.decode_without_bom_handling(bytes);
        return DecodedText {
            text: text.into_owned(),
            charset: detected.name().to_string(),
            had_errors,
        };
    }

    // 3. 兜底：替换字符最少者。
    pick_fewest_replacements(bytes, &FALLBACK_CANDIDATES)
}

/// 按调用方**显式指定**的编码名解码；编码名不可解析时返回 `None`。
///
/// 用于导入时用户手动指定编码（FR-020 的边缘情形：自动检测猜错，
/// 用户知道文件是什么编码）。返回 `None` 而不是回退到检测：用户显式给了
/// 一个不存在的编码名是**输入错误**，静默改用别的编码只会让用户困惑。
pub fn decode_with_declared(bytes: &[u8], encoding_label: &str) -> Option<DecodedText> {
    let encoding = Encoding::for_label_no_replacement(encoding_label.trim().as_bytes())?;
    let (text, had_errors) = encoding.decode_without_bom_handling(bytes);
    Some(DecodedText {
        text: text.into_owned(),
        charset: encoding.name().to_string(),
        had_errors,
    })
}

/// 用 `chardetng` 检测；返回 `None` 表示"未确信"（含单字节兜底答案）。
fn detect_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    // ISO-2022-JP 是已知风险面（浏览器据此默认拒绝它，见
    // `chardetng::Iso2022JpDetection` 的文档）。文本框内容可能含可执行脚本，
    // 故与浏览器取同一条线：Deny。
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    // `last=true` 让检测器知道这是全部输入；采样块与大文件的首块在
    // `feed` 的语义上无差别（检测器只看统计特征）。
    let sample = &bytes[..bytes.len().min(DETECTION_SAMPLE_BYTES)];
    detector.feed(sample, true);
    // 允许把 UTF-8 作为检测结果。与浏览器默认（Deny）相反是刻意的：
    // 浏览器禁它是怕网页内容开始依赖"未标注的 UTF-8"这一非标准行为，
    // 而本层是**消费方**，无声明直出 UTF-8 的文件很多，允许它只提升准确率。
    let guessed = detector.guess(None, Utf8Detection::Allow);
    if guessed.is_single_byte() {
        return None;
    }
    Some(guessed)
}

/// 在候选编码中挑替换字符最少者；同分取靠前者。
///
/// "替换字符数"是这里唯一可用的客观判据：解错了通常会把多字节序列拆成
/// 非法单元、产出成片 U+FFFD；解对了则一个也不会有。相比"猜得对不对"
/// 这种无法直接观测的量，它是可直接数出来的。
fn pick_fewest_replacements(bytes: &[u8], candidates: &[&'static Encoding]) -> DecodedText {
    let mut best: Option<(DecodedText, usize)> = None;
    for candidate in candidates {
        let (text, had_errors) = candidate.decode_without_bom_handling(bytes);
        let text = text.into_owned();
        let score = text.matches(char::REPLACEMENT_CHARACTER).count();
        // 严格小于：同分不替换，保持候选次序（即调用方给出的优先级）。
        if best
            .as_ref()
            .is_none_or(|(_previous, best_score)| score < *best_score)
        {
            best = Some((
                DecodedText {
                    text,
                    charset: candidate.name().to_string(),
                    had_errors,
                },
                score,
            ));
        }
    }
    match best {
        Some((decoded, _score)) => decoded,
        // 候选列表为空是内部编程错误；此时给出 UTF-8 lossy 的可读文本，
        // 而不是让正文变空（本模块的契约是"永远可读"）。
        None => {
            let text = String::from_utf8_lossy(bytes).into_owned();
            let had_errors = std::str::from_utf8(bytes).is_err();
            DecodedText {
                text,
                charset: UTF_8.name().to_string(),
                had_errors,
            }
        }
    }
}

/// 统计文本中的替换字符数量（供上层与测试判定解码质量）。
pub fn count_replacement_characters(text: &str) -> usize {
    text.matches(char::REPLACEMENT_CHARACTER).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GBK 的 "中文" 为 `D6 D0 CE C4`。
    const GBK_ZHONG_WEN: [u8; 4] = [0xD6, 0xD0, 0xCE, 0xC4];
    const UTF8_ZHONG_WEN: [u8; 6] = [0xE4, 0xB8, 0xAD, 0xE6, 0x96, 0x87];

    /// 足够长的中文段落，让检测器真正收敛（几字节的样本对任何统计检测器
    /// 都太短，只会给单字节兜底答案）。
    const LONG_ZH: &str = "第一章 测试章节。这是一段足够长的中文内容，用于字符集检测验证。\
                           文本需要一定的长度才能让统计检测器收敛到正确的编码结论，\
                           因此这里反复加入中文句子以增加样本量。第二章 继续测试。\
                           中文编码的识别在本地书导入里很关键，因为没有 HTTP 头可供参考。";

    fn gbk_bytes(text: &str) -> Vec<u8> {
        let (encoded, _, _) = encoding_rs::GBK.encode(text);
        encoded.into_owned()
    }

    #[test]
    fn utf8_without_bom_decodes() {
        let decoded = detect_and_decode(&UTF8_ZHONG_WEN);
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "UTF-8");
    }

    #[test]
    fn utf8_bom_is_consumed_and_not_left_in_text() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(UTF8_ZHONG_WEN.as_ref());
        let decoded = detect_and_decode(&bytes);
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "UTF-8");
    }

    #[test]
    fn utf16_bom_is_honoured() {
        // 本地 TXT 里 UTF-16 并不罕见（Windows 记事本的"Unicode"另存为）。
        let mut little_endian = vec![0xFF, 0xFE];
        little_endian.extend_from_slice(&[0x2D, 0x4E, 0x87, 0x65]);
        let decoded = detect_and_decode(&little_endian);
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "UTF-16LE");

        let mut big_endian = vec![0xFE, 0xFF];
        big_endian.extend_from_slice(&[0x4E, 0x2D, 0x65, 0x87]);
        assert_eq!(detect_and_decode(&big_endian).text, "中文");
    }

    #[test]
    fn gbk_text_is_detected_without_any_declaration() {
        let bytes = gbk_bytes(LONG_ZH);
        let decoded = detect_and_decode(&bytes);
        assert_eq!(decoded.text, LONG_ZH);
        assert_eq!(count_replacement_characters(&decoded.text), 0);
        assert!(
            matches!(decoded.charset.as_str(), "GBK" | "gb18030"),
            "应识别为中文编码，实际为 {}",
            decoded.charset
        );
    }

    #[test]
    fn perfectly_valid_gbk_is_not_mistaken_for_utf8() {
        // 反向的经典坑：GBK 字节恰好构成合法 UTF-8 序列时不应误判。
        // 用一段长文本使误判概率可以忽略。
        let bytes = gbk_bytes(LONG_ZH);
        assert_eq!(detect_and_decode(&bytes).text, LONG_ZH);
    }

    #[test]
    fn ascii_only_decodes_as_utf8() {
        let decoded = detect_and_decode(b"plain ascii text");
        assert_eq!(decoded.text, "plain ascii text");
        assert!(!decoded.had_errors);
    }

    #[test]
    fn empty_input_yields_empty_text() {
        let decoded = detect_and_decode(&[]);
        assert!(decoded.text.is_empty());
        assert_eq!(decoded.charset, "UTF-8");
    }

    #[test]
    fn undecodable_bytes_still_produce_text() {
        // 孤立的连续字节：任何多字节编码都会报错，但必须仍给出文本——
        // 本模块的契约是"永远可读"，让上层能提示用户而不是拿到空白。
        let bytes = [0xFF, 0xFE, 0xFD, 0xFC, 0xFF, 0xFF, 0x80, 0x81];
        let decoded = detect_and_decode(&bytes);
        assert!(!decoded.text.is_empty() || bytes.is_empty());
    }

    #[test]
    fn explicit_encoding_is_honoured() {
        let decoded = decode_with_declared(&GBK_ZHONG_WEN, "gbk").expect("gbk 应可解析");
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "GBK");
    }

    #[test]
    fn explicit_encoding_accepts_common_aliases() {
        // 用户会写 `GB2312` / `gb18030` / `UTF8` 这些别名。
        assert!(decode_with_declared(&GBK_ZHONG_WEN, "GB2312").is_some());
        assert!(decode_with_declared(&GBK_ZHONG_WEN, "utf8").is_some());
        assert!(decode_with_declared(&GBK_ZHONG_WEN, "   ").is_none());
    }

    #[test]
    fn unknown_explicit_encoding_is_reported_not_guessed() {
        // 输入错误必须可报告：不能静默改用自动检测（那会让用户以为指定生效了）。
        assert!(decode_with_declared(&GBK_ZHONG_WEN, "not-a-charset").is_none());
    }

    #[test]
    fn replacement_label_is_treated_as_absent() {
        // `replacement` 是 encoding_rs 的安全屏蔽伪编码，采信它会让正文全变 U+FFFD。
        assert!(decode_with_declared(&UTF8_ZHONG_WEN, "replacement").is_none());
    }

    #[test]
    fn large_input_is_sampled_and_still_correct() {
        // 采样路径必须仍然给出正确结论（检测只喂前 1 MiB）。
        let mut bytes = gbk_bytes(LONG_ZH);
        let one_chunk = bytes.clone();
        while bytes.len() < DETECTION_SAMPLE_BYTES + 4096 {
            bytes.extend_from_slice(&one_chunk);
        }
        let decoded = detect_and_decode(&bytes);
        assert_eq!(count_replacement_characters(&decoded.text), 0);
        assert!(
            matches!(decoded.charset.as_str(), "GBK" | "gb18030"),
            "大文件采样后仍应识别为中文编码，实际为 {}",
            decoded.charset
        );
    }
}
