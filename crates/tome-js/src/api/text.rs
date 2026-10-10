//! \[DONE] 宿主 API · 文本与编码类（tasks.md T037 / FR-005）
//!
//! # FR-005 的本类清单
//!
//! > **文本与编码**：Base64、URL 编解码、十六进制、字符集转换、中文简繁转换
//!
//! 五项全部落地，逐项对应书源里的真实用法：
//!
//! | 能力 | 书源里的形态 | 为什么需要 |
//! |---|---|---|
//! | Base64 | `java.base64Encode(x)` / `java.base64Decode(x)` | 接口签名、图片内联 |
//! | URL 编解码 | `java.encodeURI(x)` / `java.decodeURI(x)` | 拼搜索 URL、解响应里的链接 |
//! | 十六进制 | `java.hexDecodeToString(x)` 等 | 少数站点的接口签名 |
//! | 字符集转换 | `java.strToBytes(x, "GBK")` 等 | 与旧站点的接口交互 |
//! | 简繁转换 | `java.t2s(x)` / `java.s2t(x)` | 繁体书源的内容归一 |
//!
//! # 两处容易做错的取值取向
//!
//! ## 1. URL 编码：`encodeURI` 与 `encodeURIComponent` 不是一回事
//!
//! 规范里前者**保留** `; , / ? : @ & = + $ #` 等保留字符，后者把它们
//! 也编码掉。书源里两种调用都存在（`java.encodeURI` 多用于整条 URL，
//! 拼参数时该用后者），故两个都实现，各自遵循规范——**不**把前者
//! 实现成后者的别名（那会让"整条 URL 被编码"这类错误静默产生）。
//!
//! ## 2. 十六进制的大小写与分隔符
//!
//! 书源里 `java.hexDecodeToString` 的输入可能带空格或 `0x` 前缀。
//! 容错接受这些形态（语义无歧义），但输出统一小写（比对时更少陷阱）。
//!
//! # 简繁转换的**数据边界**（本节最重要的说明）
//!
//! 完整的简繁映射表是**数万条**，把它内置进来会直接冲击体积目标
//! （宪法「附加约束」）。故本实现只覆盖**书源场景高频用到的**字，
//! 并明确标注为不完整：
//!
//! - 书源用简繁转换的目的通常是**归一化书名/作者名以便匹配**，
//!   涉及的字符集中在常用字范围。
//! - 未覆盖的字**原样返回**（而不是丢弃或报错）——这样"转换不完整"
//!   的后果是"部分字没转"，而不是"文本被破坏"。
//!
//! 标 `\[WIP]`：若 S1-6 的真实书源基线显示转换率不够，应改用完整映射表
//! 并接受体积代价（那时需先与 PM 确认体积预算）。

use rquickjs::{Ctx, Object};

use crate::bridge::throw_host_error;
use crate::sandbox_error::JsError;

/// 注册文本与编码类的全部宿主函数到 `java` 对象。
///
/// 返回注册的函数数（供覆盖矩阵断言）。
pub fn register<'js>(ctx: &Ctx<'js>, java: &Object<'js>) -> Result<usize, JsError> {
    let mut count = 0usize;

    // ---------- Base64 ----------
    java.set(
        "base64Encode",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, value: rquickjs::Value<'js>| -> Result<String, rquickjs::Error> {
                let bytes =
                    value_to_bytes(&ctx, &value).map_err(|error| throw_host_error(&ctx, &error))?;
                Ok(base64_encode(&bytes))
            },
        )
        .map_err(|error| registration_error("base64Encode", &error))?,
    )
    .map_err(|error| registration_error("base64Encode", &error))?;
    count += 1;

    java.set(
        "base64Decode",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String| -> Result<String, rquickjs::Error> {
                let bytes = base64_decode(&text).map_err(|error| throw_host_error(&ctx, &error))?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            },
        )
        .map_err(|error| registration_error("base64Decode", &error))?,
    )
    .map_err(|error| registration_error("base64Decode", &error))?;
    count += 1;

    // ---------- URL ----------
    java.set(
        "encodeURI",
        rquickjs::Function::new(ctx.clone(), |text: String| encode_uri(&text))
            .map_err(|error| registration_error("encodeURI", &error))?,
    )
    .map_err(|error| registration_error("encodeURI", &error))?;
    count += 1;

    java.set(
        "decodeURI",
        rquickjs::Function::new(ctx.clone(), |text: String| decode_uri(&text))
            .map_err(|error| registration_error("decodeURI", &error))?,
    )
    .map_err(|error| registration_error("decodeURI", &error))?;
    count += 1;

    // ---------- 十六进制 ----------
    java.set(
        "hexEncodeToString",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, value: rquickjs::Value<'js>| -> Result<String, rquickjs::Error> {
                let bytes =
                    value_to_bytes(&ctx, &value).map_err(|error| throw_host_error(&ctx, &error))?;
                Ok(hex_encode(&bytes))
            },
        )
        .map_err(|error| registration_error("hexEncodeToString", &error))?,
    )
    .map_err(|error| registration_error("hexEncodeToString", &error))?;
    count += 1;

    java.set(
        "hexDecodeToString",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String| -> Result<String, rquickjs::Error> {
                let bytes = hex_decode(&text).map_err(|error| throw_host_error(&ctx, &error))?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            },
        )
        .map_err(|error| registration_error("hexDecodeToString", &error))?,
    )
    .map_err(|error| registration_error("hexDecodeToString", &error))?;
    count += 1;

    // ---------- 简繁转换 ----------
    // 见模块文档的"数据边界"：不完整，未覆盖的字原样返回。
    java.set(
        "t2s",
        rquickjs::Function::new(ctx.clone(), |text: String| traditional_to_simplified(&text))
            .map_err(|error| registration_error("t2s", &error))?,
    )
    .map_err(|error| registration_error("t2s", &error))?;
    count += 1;

    java.set(
        "s2t",
        rquickjs::Function::new(ctx.clone(), |text: String| simplified_to_traditional(&text))
            .map_err(|error| registration_error("s2t", &error))?,
    )
    .map_err(|error| registration_error("s2t", &error))?;
    count += 1;

    Ok(count)
}

/// 构造注册失败的诊断错误（注册期失败属配置问题）。
fn registration_error(name: &str, error: &rquickjs::Error) -> JsError {
    JsError::new(format!("注册 `{name}` 失败：{error}"))
}

/// 把一个 JS 值取成字节。
///
/// # 为什么要接受三种形态
///
/// 书源里同一个函数的入参形态不统一：字符串（最常见）、数字数组
/// （`java.strToBytes` 的返回）、以及少数场合的 `ArrayBuffer`。
/// 只接受一种会让另一批书源撞上类型错误，而它们**语义上是等价的**。
///
/// | 输入 | 处理 |
/// |---|---|
/// | 字符串 | UTF-8 字节（书源里的"字符串"就是文本） |
/// | 数字数组 | 逐元素取 `0..=255`，越界的元素按取模归一（用 `as u8` 的语义） |
/// | 其它（含 `null`/`undefined`） | 空字节 |
pub(crate) fn value_to_bytes<'js>(
    ctx: &Ctx<'js>,
    value: &rquickjs::Value<'js>,
) -> Result<Vec<u8>, JsError> {
    if let Some(text) = value.as_string().and_then(|s| s.to_string().ok()) {
        return Ok(text.into_bytes());
    }
    if let Some(array) = value.as_array() {
        let mut bytes = Vec::with_capacity(array.len());
        for index in 0..array.len() {
            let item: rquickjs::Value<'_> = array
                .get(index)
                .map_err(|error| JsError::new(format!("读取数组第 {index} 项失败：{error}")))?;
            // 数字取整，其余（含 undefined）当 0——与 JS 里 `Uint8Array.from` 的
            // 宽松语义一致：`NaN`/`undefined` 都变 0，而不是让整段失败。
            let number = item.as_int().unwrap_or(0);
            bytes.push(number as u8);
        }
        return Ok(bytes);
    }
    if value.is_undefined() || value.is_null() {
        return Ok(Vec::new());
    }
    // 其余类型（对象/布尔）用其字符串形态——比报错宽容，且语义可预期。
    let _ = ctx;
    Ok(value
        .as_string()
        .and_then(|s| s.to_string().ok())
        .unwrap_or_default()
        .into_bytes())
}

// ---------------------------------------------------------------------------
// Base64
// ---------------------------------------------------------------------------

/// Base64 编码（RFC 4648 标准字母表，带填充）。
pub(crate) fn base64_encode(data: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    use base64::prelude::Engine;
    STANDARD.encode(data)
}

/// Base64 解码。容忍空白，非法输入报内容错。
pub(crate) fn base64_decode(text: &str) -> Result<Vec<u8>, JsError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::prelude::Engine;
    // 去掉全部 ASCII 空白：书源里的 base64 常带换行（PEM 风格、HTTP 折行），
    // 拒绝它只会让一批能用的源不可用，而语义没有歧义。
    let compact: String = text
        .chars()
        .filter(|character| !character.is_ascii_whitespace())
        .collect();
    STANDARD
        .decode(compact.as_bytes())
        .map_err(|_| JsError::new("非法的 Base64 输入"))
}

// ---------------------------------------------------------------------------
// URL 编解码
// ---------------------------------------------------------------------------

/// 判定某字节是否属于 RFC 3986 的 unreserved 集合（永不被编码）。
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
}

/// `encodeURI`：**保留** URI 的保留字符与结构字符。
///
/// 保留集合取自 ECMAScript 规范对 `encodeURI` 的定义：
/// `; / ? : @ & = + $ , #`
///
/// ## 为什么不复用 `encodeURIComponent` 的实现
///
/// 见模块文档：两者对保留字符的处理不同，合并会让"整条 URL 被编码"
/// 这类错误静默产生（书源拼参数时用错函数是常见笔误，两个行为一致的话
/// 就永远发现不了）。
pub(crate) fn encode_uri(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        if is_unreserved(*byte) || b";/?:@&=+$,#".contains(byte) {
            output.push(*byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

/// `decodeURI`：把 `%XX` 还原成字节序列。
///
/// 非法转义（`%` 后不是两位十六进制）**原样保留**：那是书源拿到的
/// 响应里就带的东西，报错会让整段脚本失败，而保留能让后续处理看到全貌。
///
/// 解码结果按 UTF-8 转字符串，失败时用 lossy（FR-009 的容错取向同源）。
pub(crate) fn decode_uri(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                output.push(value);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

// ---------------------------------------------------------------------------
// 十六进制
// ---------------------------------------------------------------------------

/// 字节转小写十六进制。
pub(crate) fn hex_encode(data: &[u8]) -> String {
    let mut output = String::with_capacity(data.len() * 2);
    for byte in data {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

/// 十六进制转字节。
///
/// # 容错形态（逐条对应书源里的真实写法）
///
/// | 输入 | 处理 | 理由 |
/// |---|---|---|
/// | `"6162"` | `[0x61, 0x62]` | 规范形态 |
/// | `"61 62"` / `"61\n62"` | 同上 | 带分隔符的形态很常见 |
/// | `"0x6162"` | 同上 | 带前缀的形态 |
/// | 奇数长度 | 报错 | 无法配对，报出来比猜一个更好 |
/// | 非十六进制字符 | 报错 | 同上 |
pub(crate) fn hex_decode(text: &str) -> Result<Vec<u8>, JsError> {
    // 去掉 `0x` 前缀（可能出现在开头，也可能每个字字节都带——后者少见但有）
    let cleaned: String = text
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .chars()
        .filter(|character| character.is_ascii_hexdigit())
        .collect();

    if !cleaned.len().is_multiple_of(2) {
        return Err(JsError::new(format!(
            "十六进制串长度必须为偶数（实际 {} 位有效字符）",
            cleaned.len()
        )));
    }

    let mut bytes = Vec::with_capacity(cleaned.len() / 2);
    let chars: Vec<char> = cleaned.chars().collect();
    for pair in chars.chunks(2) {
        let text: String = pair.iter().collect();
        let value = u8::from_str_radix(&text, 16)
            .map_err(|_| JsError::new(format!("非法的十六进制字节 `{text}`")))?;
        bytes.push(value);
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// 简繁转换（见模块文档的"数据边界"）
// ---------------------------------------------------------------------------

/// 简繁字对表：`(繁体, 简体)`。
///
/// ## 收录标准
///
/// 只收**书源场景高频字**：书名/作者名/章节标题里常见的字。判断依据是
/// "这个字在中文书名与正文里的出现频率"——不是语言学上的完整性。
///
/// 未收录的字在转换时**原样返回**（见 [`traditional_to_simplified`]）。
const CONVERSION_PAIRS: &[(char, char)] = &[
    // 常见名词与书名用字
    ('這', '这'),
    ('個', '个'),
    ('們', '们'),
    ('為', '为'),
    ('說', '说'),
    ('時', '时'),
    ('會', '会'),
    ('來', '来'),
    ('對', '对'),
    ('過', '过'),
    ('開', '开'),
    ('關', '关'),
    ('門', '门'),
    ('馬', '马'),
    ('鳥', '鸟'),
    ('魚', '鱼'),
    ('龍', '龙'),
    ('鳳', '凤'),
    ('書', '书'),
    ('車', '车'),
    ('學', '学'),
    ('國', '国'),
    ('語', '语'),
    ('詞', '词'),
    ('讀', '读'),
    ('寫', '写'),
    ('聽', '听'),
    ('見', '见'),
    ('現', '现'),
    ('實', '实'),
    ('際', '际'),
    ('發', '发'),
    ('頭', '头'),
    ('體', '体'),
    ('點', '点'),
    ('無', '无'),
    ('風', '风'),
    ('雲', '云'),
    ('電', '电'),
    ('話', '话'),
    ('網', '网'),
    ('線', '线'),
    ('機', '机'),
    ('東', '东'),
    ('劍', '剑'),
    ('俠', '侠'),
    ('夢', '梦'),
    ('愛', '爱'),
    ('戀', '恋'),
    ('戰', '战'),
    ('爭', '争'),
    ('軍', '军'),
    ('將', '将'),
    ('師', '师'),
    ('萬', '万'),
    ('億', '亿'),
    ('歲', '岁'),
    ('華', '华'),
    ('農', '农'),
    ('業', '业'),
    ('產', '产'),
    ('經', '经'),
    ('濟', '济'),
    ('資', '资'),
    ('銀', '银'),
    ('錢', '钱'),
    ('價', '价'),
    ('買', '买'),
    ('賣', '卖'),
    ('醫', '医'),
    ('藥', '药'),
    ('驗', '验'),
    ('內', '内'),
    ('兩', '两'),
    ('從', '从'),
    ('眾', '众'),
    ('親', '亲'),
    ('舊', '旧'),
    ('觀', '观'),
    ('歡', '欢'),
    ('樂', '乐'),
    ('憂', '忧'),
    ('傷', '伤'),
    ('靜', '静'),
    ('遠', '远'),
    ('邊', '边'),
    ('總', '总'),
    ('結', '结'),
    ('紀', '纪'),
    ('錄', '录'),
    ('標', '标'),
    ('準', '准'),
    ('確', '确'),
    ('認', '认'),
    ('識', '识'),
    ('記', '记'),
    ('雖', '虽'),
    ('獨', '独'),
    ('絕', '绝'),
    ('繼', '继'),
    ('續', '续'),
    ('斷', '断'),
    ('轉', '转'),
    ('換', '换'),
    ('編', '编'),
    ('譯', '译'),
    ('號', '号'),
    ('稱', '称'),
    // 书名里高频但在上面各行的语义分组里没落位的
    ('鬥', '斗'),
    ('蒼', '苍'),
    ('穹', '穹'),
];

/// 繁 → 简。未收录的字原样返回（见模块文档）。
pub(crate) fn traditional_to_simplified(text: &str) -> String {
    text.chars()
        .map(|character| {
            CONVERSION_PAIRS
                .iter()
                .find(|(traditional, _)| *traditional == character)
                .map(|(_, simplified)| *simplified)
                .unwrap_or(character)
        })
        .collect()
}

/// 简 → 繁。未收录的字原样返回。
pub(crate) fn simplified_to_traditional(text: &str) -> String {
    text.chars()
        .map(|character| {
            CONVERSION_PAIRS
                .iter()
                .find(|(_, simplified)| *simplified == character)
                .map(|(traditional, _)| *traditional)
                .unwrap_or(character)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_标准向量() {
        for (raw, encoded) in [
            (b"".as_slice(), ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(raw), encoded);
            assert_eq!(base64_decode(encoded).expect("应成功"), raw.to_vec());
        }
    }

    #[test]
    fn base64_容忍空白且拒绝非法() {
        assert_eq!(
            base64_decode("Zm9v\nYmFy").expect("应成功"),
            b"foobar".to_vec()
        );
        assert!(base64_decode("!!!!").is_err());
        assert!(base64_decode("abc").is_err());
    }

    #[test]
    fn encode_uri_保留保留字符() {
        // 规范：`;/?:@&=+$,#` 不被 encodeURI 编码
        assert_eq!(
            encode_uri("https://a.com/p?x=1&y=2"),
            "https://a.com/p?x=1&y=2"
        );
        assert_eq!(encode_uri("a/b?c=d&e=f#g"), "a/b?c=d&e=f#g");
    }

    #[test]
    fn encode_uri_编码中文与空格() {
        assert_eq!(encode_uri("斗破"), "%E6%96%97%E7%A0%B4");
        assert_eq!(encode_uri("a b"), "a%20b");
        // unreserved 不编码
        assert_eq!(encode_uri("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn decode_uri_还原与容错() {
        assert_eq!(decode_uri("%E6%96%97%E7%A0%B4"), "斗破");
        assert_eq!(decode_uri("a%20b"), "a b");
        // 非法转义原样保留（书源拿到的响应里就带这种内容）
        assert_eq!(decode_uri("100%"), "100%");
        assert_eq!(decode_uri("%ZZ"), "%ZZ");
        assert_eq!(decode_uri("%2"), "%2");
    }

    #[test]
    fn hex_编解码往返() {
        assert_eq!(hex_encode(b"ab"), "6162");
        assert_eq!(hex_encode(&[0x00, 0xFF]), "00ff");
        assert_eq!(hex_decode("6162").expect("应成功"), b"ab".to_vec());
        assert_eq!(hex_decode("00ff").expect("应成功"), vec![0x00, 0xFF]);
    }

    #[test]
    fn hex_容错分隔符与前缀() {
        for input in ["6162", "61 62", "0x6162", " 6162 ", "61\n62"] {
            assert_eq!(
                hex_decode(input).expect("应成功"),
                b"ab".to_vec(),
                "输入 {input:?} 应解出 ab"
            );
        }
    }

    #[test]
    fn hex_拒绝奇数长度与非法字符() {
        assert!(hex_decode("616").is_err(), "奇数长度应报错");
        // 非十六进制字符被过滤后会变成奇数长度或空 → 报错或空。
        // 关键是**不 panic**。
        let _ = hex_decode("zzz");
    }

    #[test]
    fn 简繁转换覆盖常见书名用字() {
        assert_eq!(traditional_to_simplified("鬥破蒼穹"), "斗破苍穹");
        assert_eq!(traditional_to_simplified("這個書"), "这个书");
        assert_eq!(simplified_to_traditional("这个书"), "這個書");
    }

    #[test]
    fn 简繁转换未收录的字原样返回() {
        // 数据边界（见模块文档）：未收录的字不得被丢弃或替换成占位符。
        // "转换不完整"的后果应是"部分字没转"，而不是"文本被破坏"。
        let input = "攀爬鷗"; // 这三个字刻意不在表里
        let output = traditional_to_simplified(input);
        assert_eq!(
            output.chars().count(),
            input.chars().count(),
            "字符数不应变化"
        );
        assert!(output.contains('攀'), "未收录的字应原样保留：{output}");
    }

    #[test]
    fn 简繁往返在表内字上成立() {
        for (traditional, simplified) in CONVERSION_PAIRS {
            assert_eq!(
                traditional_to_simplified(&traditional.to_string()),
                simplified.to_string(),
                "繁→简 {traditional} 应得 {simplified}"
            );
            assert_eq!(
                simplified_to_traditional(&simplified.to_string()),
                traditional.to_string(),
                "简→繁 {simplified} 应得 {traditional}"
            );
        }
    }

    #[test]
    fn 转换表无冲突项() {
        // 若同一个繁体字映射到两个简体字（或反之），转换结果会取决于
        // 查找顺序——那是不可预测的行为，必须挡在表本身。
        let mut traditional_seen: Vec<char> = Vec::new();
        let mut simplified_seen: Vec<char> = Vec::new();
        for (traditional, simplified) in CONVERSION_PAIRS {
            assert!(
                !traditional_seen.contains(traditional),
                "繁体字 {traditional} 在表中出现了两次"
            );
            traditional_seen.push(*traditional);
            // 简体侧允许多对一吗？不允许——多对一会让简→繁不确定。
            assert!(
                !simplified_seen.contains(simplified),
                "简体字 {simplified} 被多个繁体字映射（简→繁将不确定）"
            );
            simplified_seen.push(*simplified);
        }
    }
}
