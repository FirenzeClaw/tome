//! 字符集声明嗅探、检测与容错解码（tasks.md T015 / FR-009）
//!
//! FR-009 要求"**含声明与实际不符**、以及非 UTF-8 内容"都能正确处理。
//! 这句话的重心在"不符"：书源抓到的页面里，声明 `utf-8` 而实际是 GBK 的
//! 情况非常常见（服务端模板改了、CDN 缓存串了、GBK 页面套 UTF-8 模板）。
//! 因此本模块的立场是：
//!
//! > **声明是线索，不是事实；解码结果的可读性才是判据。**
//!
//! 具体做法是"多候选解码 + 按替换字符数择优"：每个候选编码各自解一遍，
//! 谁产出的 U+FFFD 少就用谁；同分时倾向声明方（尊重声明，但不容忍它把正文
//! 解成乱码）。这样既能在声明正确时不折腾，也能在声明错误时自愈。
//!
//! # 为什么不用 `Response::text()`（reqwest 的 `charset` 特性）
//!
//! reqwest 的 `text()` 只做"按 Content-Type 的 charset 解码、失败则回退
//! UTF-8"，**不做检测**（它没有 chardetng）。对 FR-009 的核心场景——声明
//! 与实际不符——它的行为恰恰是最糟的那种：照声明解出一片乱码且不自知。
//! 故本层自己解码，`Response::text()` 不在本 crate 使用。
//!
//! # 关键入口
//!
//! - [`decode_body`] —— 字节 + 声明 → 文本 + 实际编码名
//! - [`sniff_declared_charset`] —— 从 `Content-Type` 与 HTML `<meta>` 提取声明
//! - [`DecodedBody`] —— 解码结果（文本与编码名），供填充 `Response.detected_charset`

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8};

/// HTML 编码预扫描的字节上限。
///
/// 依据 HTML 规范的 encoding sniffing algorithm：预扫描只检查文档**前 1024 字节**
/// （"prescan a byte stream to determine its encoding"）。取这个值而非"全文找"
/// 有两个理由：一是与浏览器行为一致（书源选择器依赖的正是浏览器渲染出的 DOM），
/// 二是全文扫描会在大响应体上白做功。
const HTML_PRESCAN_BYTES: usize = 1024;

/// 无声明时的回退候选次序。
///
/// 依据：本项目面向中文书源生态，UTF-8（含无声明直出）与 GB18030（兼容 GBK/GB2312）
/// 覆盖了绝大多数站点。次序即同分时的优先级——先 UTF-8 是因为现代站点的事实标准
/// 已是 UTF-8，把它排在前面能少走一次检测。
const FALLBACK_CANDIDATES: [&Encoding; 2] = [UTF_8, encoding_rs::GB18030];

/// 解码结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBody {
    /// 解码后的文本。**永远可用**——即使所有候选都失败也会给出 UTF-8 lossy 的结果，
    /// 绝不返回空正文（FR-009：不能因为编码问题把正文变成空白）。
    pub text: String,
    /// 实际采用的编码名（形如 `UTF-8` / `GBK` / `gb18030` / `Big5`），
    /// 供填充 `tome_core::Response.detected_charset`。
    pub charset: String,
}

impl DecodedBody {
    /// 以指定编码名构造。
    fn new(text: String, encoding: &'static Encoding) -> Self {
        Self {
            text,
            charset: encoding.name().to_string(),
        }
    }
}

/// 字节 + 声明 → 文本 + 实际编码名（FR-009）。
///
/// 决策次序（每一步命中即返回）：
///
/// 1. **BOM 优先**：BOM 是字节层面的**事实**，优先级高于任何声明。
///    依据 HTML 规范：BOM 会覆盖 `<meta>`/`Content-Type` 的声明。
/// 2. **声明可解析**：按其解码。**若无替换字符即采信**；若产生了替换字符，
///    说明声明与实际不符，转入检测并与声明对比择优（FR-009 的容错要求）。
/// 3. **无声明 / 声明不可解析**：`chardetng` 检测；检测给出的若是**单字节编码**，
///    视同"没猜出来"（见下）；否则采信检测结果。
/// 4. **兜底**：`FALLBACK_CANDIDATES`（UTF-8 → GB18030）逐个解，取替换字符最少者。
///
/// # 为什么"单字节编码的检测结果"等于没猜出来
///
/// `chardetng` 的兜底答案就是 windows-1252 这类单字节码页。单字节编码几乎对任何
/// 字节序列都不报错（每个字节都映射到某个字符），所以它能"成功解码"GBK 页面并
/// 产出一整篇乱码，替换字符数为 0，用"择优"规则根本挑不出它的错。
/// 换句话说：**单字节编码的零错误是个假信号**。故把它降级为"未确信"，
/// 交给 UTF-8 → GB18030 这条更贴合中文站点的次序去裁决。
pub fn decode_body(bytes: &[u8], declared: Option<&str>) -> DecodedBody {
    decode_body_with_tld(bytes, declared, None)
}

/// 同 [`decode_body`]，额外接受 TLD 提示供 `chardetng` 使用。
///
/// TLD 是有效的检测线索（`chardetng::EncodingDetector::guess` 的文档明说了
/// `.cn`/`.jp` 这类 TLD 会显著影响猜测质量，且 [`EncodingDetector::tld_may_affect_guess`]
/// 可用于先判断是否值得传）。调用方从 URL 主机名取最后一段传进来即可。
pub fn decode_body_with_tld(
    bytes: &[u8],
    declared: Option<&str>,
    tld: Option<&[u8]>,
) -> DecodedBody {
    if bytes.is_empty() {
        let encoding = declared
            .and_then(resolve_label)
            .unwrap_or(encoding_rs::UTF_8);
        return DecodedBody::new(String::new(), encoding);
    }

    // 1. BOM 是字节层面的事实，优先于任何声明。
    if let Some((bom_encoding, bom_len)) = Encoding::for_bom(bytes) {
        let (text, _had_errors) = bom_encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return DecodedBody::new(text.into_owned(), bom_encoding);
    }

    // 2. 声明可解析时先按其解码。
    if let Some(declared_encoding) = declared.and_then(resolve_label) {
        // 声明是**遗留编码**而字节整体是合法 UTF-8：据 UTF-8。
        //
        // 依据：GBK/Big5 的双字节序列与 UTF-8 的多字节序列在字节模式上高度互斥——
        // 一段含中文的文本若整体构成合法 UTF-8 序列，它几乎不可能是 GBK 编码
        // （概率随文本长度指数下降）。反向不成立：GBK 解码 UTF-8 字节往往**不报错**
        // 却产出乱码（每个字节对都能凑成合法双字节），故"能解码"不能作为判据。
        // 因此把"合法 UTF-8"当作**强判据**，用来纠正"声明着遗留编码、实际发 UTF-8"
        // 这个在书源生态里极高频的错配（老模板 + 新内容）。
        //
        // 只在含非 ASCII 字节时才启用：纯 ASCII 文本对任何 ASCII 兼容编码都成立，
        // 判它"是 UTF-8"没有信息量，还会无谓改动 `detected_charset` 的取值。
        if declared_encoding != UTF_8 && looks_like_utf8(bytes) {
            let (text, _had_errors) = UTF_8.decode_without_bom_handling(bytes);
            return DecodedBody::new(text.into_owned(), UTF_8);
        }

        let (text, had_errors) = declared_encoding.decode_without_bom_handling(bytes);
        if !had_errors {
            return DecodedBody::new(text.into_owned(), declared_encoding);
        }
        // 声明把正文解出了替换字符 = 声明与实际不符（FR-009 的核心场景）。
        return reconcile_with_detection(bytes, declared_encoding, tld);
    }

    // 3. 无声明 / 声明不可解析 → 检测。
    match detect(bytes, tld) {
        Some(detected) => {
            let (text, _had_errors) = detected.decode_without_bom_handling(bytes);
            DecodedBody::new(text.into_owned(), detected)
        }
        None => pick_fewest_replacements(bytes, &FALLBACK_CANDIDATES),
    }
}

/// 声明解出了替换字符：让检测结果与声明各自解码，取替换字符更少的一方。
///
/// 平手时保留**声明**：声明解出的替换字符若与检测相同，说明两者表现一致，
/// 此时没有理由推翻服务端的显式声明（也避免给同样的字节流贴上不稳定的编码名，
/// 那会让差分测试的结果随检测器的内部状态漂移）。
///
/// 检测未给出结论（单字节兜底）时，把中文站点的常见编码作为额外候选——
/// 这正是声明出错时最可能需要的那一个。
fn reconcile_with_detection(
    bytes: &[u8],
    declared_encoding: &'static Encoding,
    tld: Option<&[u8]>,
) -> DecodedBody {
    let mut candidates: Vec<&'static Encoding> = vec![declared_encoding];
    if let Some(detected) = detect(bytes, tld) {
        candidates.push(detected);
    }
    for fallback in FALLBACK_CANDIDATES {
        if !candidates.contains(&fallback) {
            candidates.push(fallback);
        }
    }
    pick_fewest_replacements(bytes, &candidates)
}

/// 在给定候选编码中挑出替换字符最少者；同分取靠前者。
///
/// "替换字符数"是这里唯一可用的客观判据：把一个编码解错了，通常会把多字节序列
/// 拆成非法单元，产出成片的 U+FFFD；解对了则一个也不会有。相比"猜得对不对"这种
/// 无法直接观测的量，它是可直接数出来的。
fn pick_fewest_replacements(bytes: &[u8], candidates: &[&'static Encoding]) -> DecodedBody {
    let mut best: Option<(DecodedBody, usize)> = None;
    for candidate in candidates {
        let (text, _had_errors) = candidate.decode_without_bom_handling(bytes);
        let text = text.into_owned();
        let score = count_replacement_characters(&text);
        // 严格小于：同分不替换，保持候选次序（即调用方给出的优先级）。
        if best
            .as_ref()
            .is_none_or(|(_previous, best_score)| score < *best_score)
        {
            best = Some((DecodedBody::new(text, candidate), score));
        }
    }
    match best {
        Some((decoded, _score)) => decoded,
        // 候选列表为空是内部编程错误；此时给出 UTF-8 lossy 的可读文本，
        // 而不是返回 Err 让正文变空（FR-009 的硬要求）。
        None => {
            let text = String::from_utf8_lossy(bytes).into_owned();
            DecodedBody::new(text, UTF_8)
        }
    }
}

/// 统计文本中的替换字符数量（解码质量的度量）。
fn count_replacement_characters(text: &str) -> usize {
    text.matches(char::REPLACEMENT_CHARACTER).count()
}

/// 字节是否构成合法的 UTF-8（且含至少一个非 ASCII 字节）。
///
/// "含非 ASCII"这个附加条件不可省：纯 ASCII 文本在 GBK/Big5/UTF-8 下都合法，
/// 判它"是 UTF-8"没有信息量，反而会把"声明 GBK"的纯英文页面改标成 UTF-8，
/// 让 `detected_charset` 变得不稳定（同一个页面在不同响应间给出不同编码名）。
fn looks_like_utf8(bytes: &[u8]) -> bool {
    if bytes.is_ascii() {
        return false;
    }
    std::str::from_utf8(bytes).is_ok()
}

/// 用 `chardetng` 检测编码。返回 `None` 表示"未确信"（含单字节兜底答案）。
fn detect(bytes: &[u8], tld: Option<&[u8]>) -> Option<&'static Encoding> {
    // ISO-2022-JP 对可执行脚本的内容是已知风险面（浏览器因此默认拒绝它，
    // 见 chardetng::Iso2022JpDetection 的文档）；书源抓到的页面正是可执行脚本的内容，
    // 故与浏览器取同一条线：Deny。
    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    // 允许把 UTF-8 作为检测结果。这与浏览器默认（Deny）相反，是刻意的：
    // 浏览器禁它是怕网页内容开始依赖"未标注的 UTF-8"这一非标准行为；
    // 而本模块是消费方，无声明直出 UTF-8 的站点很多，允许它只提升准确率。
    let guessed = detector.guess(tld, Utf8Detection::Allow);
    if guessed.is_single_byte() {
        return None;
    }
    Some(guessed)
}

/// 把声明的编码标签解析为 `encoding_rs` 的编码。
///
/// 用 `for_label_no_replacement` 而非 `for_label`：后者会把 `replacement` 这个
/// 标签解析成 `REPLACEMENT`（一个用于安全屏蔽的伪编码，解码后全是 U+FFFD）。
/// 把它当作正常声明采信会让正文变成一片替换字符，故视为不可解析。
fn resolve_label(label: &str) -> Option<&'static Encoding> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return None;
    }
    Encoding::for_label_no_replacement(trimmed.as_bytes())
}

// ---------------------------------------------------------------------------
// 声明嗅探：Content-Type 头与 HTML <meta>
// ---------------------------------------------------------------------------

/// 从 `Content-Type` 与 HTML 前若干字节中提取声明的字符集。
///
/// 次序按 HTML 规范：**HTTP 头优先于 `<meta>`**（规范中 transport layer 的
/// 编码信息优先级高于文档内的 prescan 结果）。
///
/// `head_bytes` 传入响应体的**开头一段**即可（本函数只看前 1024 字节），
/// 不需要整份正文。
pub fn sniff_declared_charset(content_type: Option<&str>, head_bytes: &[u8]) -> Option<String> {
    if let Some(declared) = content_type.and_then(charset_from_content_type) {
        return Some(declared);
    }
    charset_from_html_meta(head_bytes)
}

/// 从 `Content-Type` 头的值里取出 `charset` 参数。
///
/// 依据 RFC 9110 的 `media-type` 语法：参数形如 `;charset=GBK` 或 `;charset="GBK"`，
/// 参数名大小写不敏感，值可加双引号。
pub fn charset_from_content_type(content_type: &str) -> Option<String> {
    let lower = content_type.to_ascii_lowercase();
    let position = lower.find("charset")?;
    let after_name = &content_type[position + "charset".len()..];
    let after_equals = after_name.trim_start().strip_prefix('=')?;
    let value = after_equals.trim_start();
    let value = match value.strip_prefix('"') {
        Some(quoted) => quoted.split('"').next().unwrap_or(""),
        None => value.split([';', ',']).next().unwrap_or(""),
    };
    normalize_declared(value)
}

/// 从 HTML 前若干字节里取出 `<meta>` 声明的字符集。
///
/// 依据 HTML 规范的 prescan 算法，支持两种写法：
///
/// - HTML5 简写：`<meta charset="gbk">`
/// - 旧式 http-equiv：`<meta http-equiv="Content-Type" content="text/html; charset=gbk">`
///
/// 实现是**不区分大小写的子串扫描**，不是真正的 HTML 解析。这是刻意与规范对齐的：
/// 规范明确要求 prescan 用受限的状态机（"prescan"），因为真正的解析需要先知道编码，
/// 而编码正是这里要找的东西——鸡生蛋。
pub fn charset_from_html_meta(head_bytes: &[u8]) -> Option<String> {
    let head = &head_bytes[..head_bytes.len().min(HTML_PRESCAN_BYTES)];
    let text = String::from_utf8_lossy(head).into_owned();
    let lower = text.to_ascii_lowercase();

    // 两种写法的共同点：`charset` 之后紧跟 `=`。直接扫所有 `charset` 出现处，
    // 就近取 `=` 后的值——对上述两种写法都成立，且不必先切分 `<meta>` 标签。
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find("charset") {
        let position = cursor + offset;
        // `<meta charset=x>` 与 `content="...; charset=x"` 在 `charset` 之后
        // 都只允许出现空白与 `=`；出现别的字符说明这不是编码声明（如 `charsetfoo`）。
        let after_name = &text[position + "charset".len()..];
        if let Some(after_equals) = after_name.trim_start().strip_prefix('=') {
            if let Some(declared) = extract_attribute_value(after_equals) {
                return Some(declared);
            }
        }
        cursor = position + "charset".len();
    }
    None
}

/// 从一个刚跳过 `=` 的位置取出属性值，并清洗为编码名。
///
/// 三种写法都要支持（HTML 对属性值的引号不作强制）：
///
/// - `"gbk"` → 取到下一个 `"` 为止
/// - `'gbk'` → 取到下一个 `'` 为止
/// - `gbk>` / `gbk` → 取到首个分隔符为止
fn extract_attribute_value(after_equals: &str) -> Option<String> {
    let value = after_equals.trim_start();
    // 带引号：必须成对，否则视为格式不全（值被截断）而放弃。
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote) {
            let end = inner.find(quote)?;
            return normalize_declared(&inner[..end]);
        }
    }
    // 无引号：取到首个分隔符。
    let end = value
        .find(['"', '\'', '>', '/', ';', ' ', '\t', '\n', '\r'])
        .unwrap_or(value.len());
    normalize_declared(&value[..end])
}

/// 清洗声明值：去空白与引号，空值视为无声明。
fn normalize_declared(value: &str) -> Option<String> {
    let cleaned = value.trim().trim_matches(['"', '\'']).trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GBK 的 "中文" 为 `D6 D0 CE C4`（GBK 与 GB2312 在此处一致）。
    const GBK_ZHONG_WEN: [u8; 4] = [0xD6, 0xD0, 0xCE, 0xC4];
    /// Big5 的 "中文" 为 `A4 A4 A4 E5`。
    const BIG5_ZHONG_WEN: [u8; 4] = [0xA4, 0xA4, 0xA4, 0xE5];
    /// UTF-8 的 "中文"。
    const UTF8_ZHONG_WEN: [u8; 6] = [0xE4, 0xB8, 0xAD, 0xE6, 0x96, 0x87];

    /// 一段足够长的 GBK 中文文本，用于让 chardetng 真正识别出中文编码
    /// （4 字节的样本对任何检测器都太短，检测器只会给单字节兜底答案）。
    const GBK_LONG: &str = "中文测试文本，用于字符集检测验证。这是一段足够长的中文内容，\
                            以便检测器能够正确识别出编码为 GBK 或 GB18030。";

    fn gbk_long_bytes() -> Vec<u8> {
        let (encoded, _, had_errors) = encoding_rs::GBK.encode(GBK_LONG);
        assert!(!had_errors, "GBK 应能编码测试文本");
        encoded.into_owned()
    }

    #[test]
    fn utf8_bom_wins_over_declared() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(&UTF8_ZHONG_WEN);
        // 声明一个明显错误的编码，BOM 必须压过它。
        let decoded = decode_body(&bytes, Some("Big5"));
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "UTF-8");
    }

    #[test]
    fn utf16_bom_is_honoured() {
        let mut little_endian = vec![0xFF, 0xFE];
        little_endian.extend_from_slice(&[0x2D, 0x4E, 0x87, 0x65]);
        assert_eq!(decode_body(&little_endian, None).text, "中文");

        let mut big_endian = vec![0xFE, 0xFF];
        big_endian.extend_from_slice(&[0x4E, 0x2D, 0x65, 0x87]);
        assert_eq!(decode_body(&big_endian, None).text, "中文");
    }

    #[test]
    fn utf8_without_declaration_decodes() {
        let decoded = decode_body(&UTF8_ZHONG_WEN, None);
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "UTF-8");
    }

    #[test]
    fn gbk_with_correct_declaration_decodes() {
        let decoded = decode_body(&GBK_ZHONG_WEN, Some("gbk"));
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "GBK");
    }

    #[test]
    fn big5_with_correct_declaration_decodes() {
        let decoded = decode_body(&BIG5_ZHONG_WEN, Some("big5"));
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "Big5");
    }

    #[test]
    fn gb18030_four_byte_sequence_decodes() {
        // GB18030 的四字节区：`81 30 81 30` 是 U+0080 的 GB18030 表示。
        // 这段字节在 GBK 里非法（GBK 无四字节序列），故它同时也是
        // "GB18030 与 GBK 必须区分对待"的样本。
        let bytes = [0x81, 0x30, 0x81, 0x30];
        let decoded = decode_body(&bytes, Some("gb18030"));
        assert_eq!(decoded.text, "\u{80}");
        assert_eq!(decoded.charset, "gb18030");
        assert_eq!(count_replacement_characters(&decoded.text), 0);
    }

    #[test]
    fn gb18030_four_byte_cjk_decodes() {
        // U+9FA6 在 GB18030 中为四字节 `82 35 8F 33`。
        let bytes = [0x82, 0x35, 0x8F, 0x33];
        let decoded = decode_body(&bytes, Some("GB18030"));
        assert_eq!(decoded.text, "\u{9FA6}");
        assert_eq!(decoded.charset, "gb18030");
    }

    /// FR-009 的核心场景：声明与实际不符仍须产出可读文本。
    #[test]
    fn wrong_declaration_still_yields_readable_text() {
        // GBK 字节却声明 utf-8。
        let bytes = gbk_long_bytes();
        let decoded = decode_body(&bytes, Some("utf-8"));
        assert_eq!(decoded.text, GBK_LONG);
        assert_eq!(count_replacement_characters(&decoded.text), 0);
        assert_ne!(decoded.charset, "UTF-8", "应采用检测出的中文编码");
    }

    #[test]
    fn ascii_only_body_with_legacy_declaration_keeps_the_declaration() {
        // 纯 ASCII 对任何 ASCII 兼容编码都成立，故没有理由推翻声明
        // （否则 detected_charset 会随内容在 UTF-8 与其他之间抖动）。
        let decoded = decode_body(b"<html><body>hello</body></html>", Some("gbk"));
        assert_eq!(decoded.charset, "GBK");
        assert_eq!(decoded.text, "<html><body>hello</body></html>");
    }

    #[test]
    fn utf8_bytes_declared_as_gbk_falls_back_to_utf8() {
        // 反向场景：UTF-8 字节却声明 gbk。
        let decoded = decode_body(&UTF8_ZHONG_WEN, Some("gbk"));
        assert_eq!(decoded.text, "中文");
    }

    #[test]
    fn unknown_declaration_is_ignored() {
        let decoded = decode_body(&UTF8_ZHONG_WEN, Some("x-not-a-charset"));
        assert_eq!(decoded.text, "中文");
    }

    #[test]
    fn replacement_label_is_treated_as_absent() {
        // `replacement` 是 encoding_rs 的安全屏蔽伪编码，采信它会让正文全变 U+FFFD。
        let decoded = decode_body(&UTF8_ZHONG_WEN, Some("replacement"));
        assert_eq!(decoded.text, "中文");
    }

    #[test]
    fn long_gbk_without_declaration_is_detected() {
        let bytes = gbk_long_bytes();
        let decoded = decode_body(&bytes, None);
        assert_eq!(decoded.text, GBK_LONG);
        assert!(
            matches!(decoded.charset.as_str(), "GBK" | "gb18030"),
            "应识别为中文编码，实际为 {}",
            decoded.charset
        );
    }

    #[test]
    fn empty_body_is_readable() {
        let decoded = decode_body(&[], Some("gbk"));
        assert!(decoded.text.is_empty());
        assert_eq!(decoded.charset, "GBK");
    }

    #[test]
    fn undecodable_bytes_still_produce_text() {
        // 孤立的连续字节：任何多字节编码都会报错，但必须仍给出文本（lossy）。
        let bytes = [0xFF, 0xFE, 0xFD, 0xFC, 0xFF, 0xFF];
        let decoded = decode_body(&bytes, Some("utf-8"));
        assert!(!decoded.text.is_empty());
    }

    #[test]
    fn content_type_charset_is_extracted() {
        assert_eq!(
            charset_from_content_type("text/html; charset=gbk"),
            Some("gbk".to_string())
        );
        assert_eq!(
            charset_from_content_type("text/html;charset=\"GB18030\""),
            Some("GB18030".to_string())
        );
        assert_eq!(
            charset_from_content_type("text/html; CHARSET=UTF-8"),
            Some("UTF-8".to_string())
        );
        assert_eq!(
            charset_from_content_type("application/json"),
            None,
            "无 charset 参数时不应给出声明"
        );
        assert_eq!(charset_from_content_type("text/html; charset="), None);
    }

    #[test]
    fn html_meta_charset_is_extracted() {
        let html5 = br#"<html><head><meta charset="gbk"><title>t</title>"#;
        assert_eq!(charset_from_html_meta(html5), Some("gbk".to_string()));

        let legacy = br#"<meta http-equiv="Content-Type" content="text/html; charset=Big5">"#;
        assert_eq!(charset_from_html_meta(legacy), Some("Big5".to_string()));

        let upper = br#"<META CHARSET='UTF-8'>"#;
        assert_eq!(charset_from_html_meta(upper), Some("UTF-8".to_string()));

        let none = br#"<html><head><title>no charset here</title>"#;
        assert_eq!(charset_from_html_meta(none), None);
    }

    #[test]
    fn sniff_prefers_header_over_meta() {
        let html = br#"<meta charset="big5">"#;
        assert_eq!(
            sniff_declared_charset(Some("text/html; charset=gbk"), html),
            Some("gbk".to_string())
        );
        assert_eq!(sniff_declared_charset(None, html), Some("big5".to_string()));
    }

    #[test]
    fn meta_charset_lookalike_is_not_mistaken_for_declaration() {
        // `charsetfoo=` 不是 `charset=`，不应被当作声明。
        let html = br#"<script>var charsetfoo="shift_jis";</script>"#;
        assert_eq!(charset_from_html_meta(html), None);
    }

    #[test]
    fn prescan_only_reads_the_first_kilobyte() {
        let mut html = vec![b'a'; HTML_PRESCAN_BYTES + 64];
        html.extend_from_slice(br#"<meta charset="big5">"#);
        assert_eq!(
            charset_from_html_meta(&html),
            None,
            "限额之外的 meta 不应被读取（与 HTML 规范的 prescan 对齐）"
        );
    }

    #[test]
    fn tld_hint_is_accepted() {
        let bytes = gbk_long_bytes();
        let decoded = decode_body_with_tld(&bytes, None, Some(b"cn"));
        assert_eq!(decoded.text, GBK_LONG);
    }
}
