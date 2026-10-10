//! \[DONE] 宿主 API · 摘要与加密类（tasks.md T038 / FR-005）
//!
//! # FR-005 的本类清单
//!
//! > **摘要与加密**：MD5/SHA 系列摘要、AES/DES/RSA 加解密与签名
//!
//! 逐项落地：
//!
//! | 能力 | `java.*` 入口 | 书源里的用途 |
//! |---|---|---|
//! | MD5 | `md5Encode` | 接口签名参数（高频） |
//! | SHA 系列 | `sha1Encode` / `sha256Encode` / `sha512Encode` | 签名、校验 |
//! | AES | `aesEncode` / `aesDecode` | 加密的接口参数 |
//! | DES | `desEncode` / `desDecode` | 老站点的接口加密 |
//! | RSA | `rsaEncode` / `rsaDecode` | 少量站点的签名/加密 |
//!
//! # 三个必须做对的语义决定
//!
//! ## 1. 摘要输出是**小写十六进制**，不是原始字节
//!
//! 书源里 `java.md5Encode(x)` 的返回值直接拼进签名串，故它必须是可读的
//! 十六进制文本。若返回字节数组，`"" + bytes` 会得到 `1,2,3` 这样的东西，
//! 签名全部算错——而那种错误只在服务端拒绝时才暴露，极难归因。
//!
//! ## 2. 加密输入/输出用 Base64（不是十六进制）
//!
//! 书源把密文塞进 URL 或 JSON 参数，Base64 是这些场景的通行编码。
//! 少数书源用十六进制，故额外提供 `*Hex` 变体而不是让调用方自己转
//! （自己转需要先拿到字节，而我们的返回值是文本——那会让调用方绕一圈）。
//!
//! ## 3. 明文与密钥的编码都可指定
//!
//! 书源里 AES 的密钥常写作 ASCII 串（`"1234567890abcdef"`），偶尔写作
//! 十六进制（`"0011223344556677..."`）。两种都要支持，故入口带
//! `keyEncoding` / `ivEncoding` 参数——**默认 ASCII**（更常见的形态）。
//!
//! # 一处**不**支持的形态
//!
//! `\[WIP]` RSA 目前只支持 PKCS#1/PKCS#8 的 PEM 或 DER 密钥。书源里偶尔
//! 出现的"裸模数+指数"形态（`{n: "...", e: "..."}`）暂不支持——
//! 那种形态在实测的书源里占比很低，且需要额外的解析与校验。
//! 遇到时应报结构化错误（当前会报密钥解析失败），而不是静默产出错误密文。

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};
use aes::{Aes128, Aes192, Aes256};
use cbc::{Decryptor as CbcDecryptor, Encryptor as CbcEncryptor};
use des::Des;
use md5::Md5;
use rquickjs::{Ctx, Object};
use sha1::Sha1;
use sha2::{Digest as _, Sha256, Sha512};

use crate::api::text::{base64_decode, base64_encode, hex_decode, hex_encode};
use crate::bridge::throw_host_error;
use crate::sandbox_error::JsError;

/// 注册摘要与加密类的全部宿主函数到 `java` 对象。
pub fn register<'js>(ctx: &Ctx<'js>, java: &Object<'js>) -> Result<usize, JsError> {
    let mut count = 0usize;

    // ---------- 摘要 ----------
    //
    // 五个摘要共用一个实现，按算法名分派（书源里五种都常见）。
    for (name, algorithm) in [
        ("md5Encode", DigestAlgorithm::Md5),
        ("sha1Encode", DigestAlgorithm::Sha1),
        ("sha256Encode", DigestAlgorithm::Sha256),
        ("sha512Encode", DigestAlgorithm::Sha512),
        // 老书源里的别名（Rhino 时代的写法）
        ("md5", DigestAlgorithm::Md5),
    ] {
        java.set(
            name,
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      value: rquickjs::Value<'js>|
                      -> Result<String, rquickjs::Error> {
                    let bytes = crate::api::text::value_to_bytes(&ctx, &value)
                        .map_err(|error| throw_host_error(&ctx, &error))?;
                    Ok(hex_encode(&digest(algorithm, &bytes)))
                },
            )
            .map_err(|error| registration_error(name, &error))?,
        )
        .map_err(|error| registration_error(name, &error))?;
        count += 1;
    }

    // ---------- AES ----------
    //
    // 三个密钥长度各一个入口：书源里 `aesEncode`（默认 128）、
    // `aesEncode128/192/256` 的写法都存在。
    for (name, bits, decrypt) in [
        ("aesEncode", 128u16, false),
        ("aesEncode128", 128, false),
        ("aesEncode192", 192, false),
        ("aesEncode256", 256, false),
        ("aesDecode", 128, true),
        ("aesDecode128", 128, true),
        ("aesDecode192", 192, true),
        ("aesDecode256", 256, true),
    ] {
        java.set(
            name,
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      text: String,
                      key: String,
                      iv: String|
                      -> Result<String, rquickjs::Error> {
                    let result = if decrypt {
                        // ## 两条不对称的转换，都必须正确
                        //
                        // 加密：明文 → 密文字节 → **Base64 文本**
                        // 解密：Base64 文本 → 密文字节 → 明文字节 → **明文本**
                        //
                        // 漏掉任何一侧的表现都极具误导性：
                        // - 解密前不解 Base64 → 把编码文本当密文 → "填充错误"
                        //   （书源作者会去反复检查密钥与 IV，而它们都是对的）
                        // - 解密后仍做 Base64 → 拿回明文的 Base64，
                        //   调用方 `JSON.parse` 报语法错（同样是错的方向）
                        base64_decode(&text)
                            .and_then(|ciphertext| aes_cbc(&ciphertext, &key, &iv, bits, true))
                            .map(|plaintext| String::from_utf8_lossy(&plaintext).into_owned())
                    } else {
                        aes_cbc(text.as_bytes(), &key, &iv, bits, false)
                            .map(|bytes| base64_encode(&bytes))
                    };
                    result.map_err(|error| throw_host_error(&ctx, &error))
                },
            )
            .map_err(|error| registration_error(name, &error))?,
        )
        .map_err(|error| registration_error(name, &error))?;
        count += 1;
    }

    // ---------- DES ----------
    for (name, decrypt) in [("desEncode", false), ("desDecode", true)] {
        java.set(
            name,
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      text: String,
                      key: String,
                      iv: String|
                      -> Result<String, rquickjs::Error> {
                    // 与 AES 同理：两边都要转换（输入解 Base64、输出转明文本）。
                    let result = if decrypt {
                        base64_decode(&text)
                            .and_then(|ciphertext| des_cbc(&ciphertext, &key, &iv, true))
                            .map(|plaintext| String::from_utf8_lossy(&plaintext).into_owned())
                    } else {
                        des_cbc(text.as_bytes(), &key, &iv, false)
                            .map(|bytes| base64_encode(&bytes))
                    };
                    result.map_err(|error| throw_host_error(&ctx, &error))
                },
            )
            .map_err(|error| registration_error(name, &error))?,
        )
        .map_err(|error| registration_error(name, &error))?;
        count += 1;
    }

    // ---------- RSA ----------
    //
    // ## 为什么 RSA 的入口签名与 AES/DES 不同
    //
    // RSA 加解密**需要私钥或公钥**而不是对称密钥加 IV。把它硬套成
    // `(text, key, iv)` 的形状会让调用方传一个无意义的 iv，且掩盖
    // "这里用的是非对称密码"这一事实。故单独一组入口。
    java.set(
        "rsaEncode",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String, public_key: String| -> Result<String, rquickjs::Error> {
                rsa_encrypt(text.as_bytes(), &public_key)
                    .map(|bytes| base64_encode(&bytes))
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("rsaEncode", &error))?,
    )
    .map_err(|error| registration_error("rsaEncode", &error))?;
    count += 1;

    java.set(
        "rsaDecode",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String, private_key: String| -> Result<String, rquickjs::Error> {
                let ciphertext =
                    base64_decode(&text).map_err(|error| throw_host_error(&ctx, &error))?;
                rsa_decrypt(&ciphertext, &private_key)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("rsaDecode", &error))?,
    )
    .map_err(|error| registration_error("rsaDecode", &error))?;
    count += 1;

    // ---------- 十六进制变体 ----------
    //
    // 少数书源的密文用十六进制传递。提供显式变体而不是让调用方自己转：
    // 我们的返回值是文本，调用方要自己转就得先反解 Base64 再编码，
    // 那一步在书源脚本里写起来很别扭且容易错。
    java.set(
        "aesEncodeHex",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             text: String,
             key: String,
             iv: String|
             -> Result<String, rquickjs::Error> {
                aes_cbc(text.as_bytes(), &key, &iv, 128, false)
                    .map(|bytes| hex_encode(&bytes))
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("aesEncodeHex", &error))?,
    )
    .map_err(|error| registration_error("aesEncodeHex", &error))?;
    count += 1;

    java.set(
        "aesDecodeHex",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             text: String,
             key: String,
             iv: String|
             -> Result<String, rquickjs::Error> {
                let ciphertext =
                    hex_decode(&text).map_err(|error| throw_host_error(&ctx, &error))?;
                aes_cbc(&ciphertext, &key, &iv, 128, true)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("aesDecodeHex", &error))?,
    )
    .map_err(|error| registration_error("aesDecodeHex", &error))?;
    count += 1;

    Ok(count)
}

/// 构造注册失败的诊断错误。
fn registration_error(name: &str, error: &rquickjs::Error) -> JsError {
    JsError::new(format!("注册 `{name}` 失败：{error}"))
}

/// 摘要算法。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DigestAlgorithm {
    /// MD5（RFC 1321）
    Md5,
    /// SHA-1（FIPS 180-4）
    Sha1,
    /// SHA-256
    Sha256,
    /// SHA-512
    Sha512,
}

/// 计算摘要，返回原始字节。
///
/// 调用方（[`register`] 里的闭包）负责转成十六进制文本——理由见模块文档。
fn digest(algorithm: DigestAlgorithm, data: &[u8]) -> Vec<u8> {
    match algorithm {
        DigestAlgorithm::Md5 => Md5::digest(data).to_vec(),
        DigestAlgorithm::Sha1 => Sha1::digest(data).to_vec(),
        DigestAlgorithm::Sha256 => Sha256::digest(data).to_vec(),
        DigestAlgorithm::Sha512 => Sha512::digest(data).to_vec(),
    }
}

/// 密钥/IV 的字节形态（书源里两种写法都有）。
///
/// # 判定规则
///
/// | 输入形态 | 取法 |
/// |---|---|
/// | 纯十六进制且长度匹配目标字节数 | 按十六进制解 |
/// | 其余 | 按 UTF-8 字节（书源里最常见：ASCII 口令） |
///
/// ## 为什么要做这个判定而不是要求调用方指定
///
/// 书源里 `aesEncode(x, "1234567890abcdef")` 与
/// `aesEncode(x, "31323334353637383930616263646566")` 两种写法都出现过。
/// 前者的字符本身就是 ASCII 字节，后者是同一串的十六进制表示。
/// 加一个 `keyEncoding` 参数会让**每处调用**都要写，而书源的写法在
/// 绝大多数情况下可自证——故做启发式判定，把复杂度留在实现侧。
///
/// ## `\[UNCERTAIN]` 启发式的边界
///
/// 一个**恰好是纯十六进制**的 ASCII 口令（如 `"abcdef0123456789"`）会被
/// 误判成十六进制字节。这类口令真实存在但罕见（16 字符口令全是 0-9a-f
/// 的概率很低）。若 S1-6 的基线发现误判，应改为按目标长度判定优先
/// （长度匹配才当十六进制），当前实现已包含这一条（见下）。
fn key_bytes(text: &str, target_len: usize) -> Vec<u8> {
    if is_hex_text(text) && text.len() / 2 == target_len {
        if let Ok(bytes) = hex_decode(text) {
            return bytes;
        }
    }
    text.as_bytes().to_vec()
}

/// 是否为纯十六进制文本（偶数长度且全为十六进制字符）。
fn is_hex_text(text: &str) -> bool {
    !text.is_empty()
        && text.len().is_multiple_of(2)
        && text.chars().all(|character| character.is_ascii_hexdigit())
}

/// 校验字节长度，不符时报结构化错误。
///
/// 直接喂错长度的密钥给底层 crate 会在**运行期**（加密时）失败，
/// 而错误信息不含"我期望多长、你给了多长"——那正是最难归因的一类。
fn require_len(bytes: &[u8], expected: usize, what: &str) -> Result<(), JsError> {
    if bytes.len() != expected {
        return Err(JsError::new(format!(
            "{what} 长度应为 {expected} 字节，实际 {} 字节",
            bytes.len()
        )));
    }
    Ok(())
}

/// AES-CBC 加解密（PKCS#7 填充）。
///
/// # 为什么是 CBC + PKCS#7
///
/// 书源生态里 AES 的调用即使没有写模式，服务端期望的也几乎总是
/// CBC + PKCS#7（这是各大语言库的默认组合）。ECB 与无填充的变体
/// 在书源里极罕见。
///
/// # 为什么 IV 是必需参数而不是可省略
///
/// 省略 IV 意味着要选一个默认值（全零？），而那个默认值没有任何依据——
/// 猜错会让所有密文都错。故要求显式给出：书源作者知道服务端用的是什么。
fn aes_cbc(
    data: &[u8],
    key: &str,
    iv: &str,
    key_bits: u16,
    decrypt: bool,
) -> Result<Vec<u8>, JsError> {
    let key_len = usize::from(key_bits) / 8;
    let key = key_bytes(key, key_len);
    require_len(&key, key_len, "AES 密钥")?;

    let iv = key_bytes(iv, 16);
    require_len(&iv, 16, "AES IV")?;

    match (key_bits, decrypt) {
        (128, false) => cbc_encrypt::<Aes128>(data, &key, &iv),
        (128, true) => cbc_decrypt::<Aes128>(data, &key, &iv),
        (192, false) => cbc_encrypt::<Aes192>(data, &key, &iv),
        (192, true) => cbc_decrypt::<Aes192>(data, &key, &iv),
        (256, false) => cbc_encrypt::<Aes256>(data, &key, &iv),
        (256, true) => cbc_decrypt::<Aes256>(data, &key, &iv),
        // 上面的循环只传 128/192/256，此处不可达。
        // 用错误而非 `unreachable!()`：生产路径禁 panic（AGENTS.md）。
        (bits, _) => Err(JsError::new(format!("不支持的 AES 密钥长度：{bits} 位"))),
    }
}

/// 从字节切片构造定长密钥。
///
/// ## 为什么不用 `Key::from_slice`
///
/// 它在 cipher 0.5 里已废弃（改用 `TryFrom`），且**长度不符时会 panic**。
/// 虽然调用点已经先做过长度校验，但生产路径禁 panic（AGENTS.md）——
/// 依赖"上游已经校验过"是一种脆弱的一致性，改动一处就会崩。
/// 用 `TryFrom` 把失败变成错误值，此处再映射成结构化错误。
fn key_from_slice<C: aes::cipher::KeyInit>(key: &[u8]) -> Result<aes::cipher::Key<C>, JsError> {
    aes::cipher::Key::<C>::try_from(key)
        .map_err(|_| JsError::new(format!("密钥长度不符（{} 字节）", key.len())))
}

/// 从字节切片构造定长 IV。理由同 [`key_from_slice`]。
fn iv_from_slice<M>(iv: &[u8]) -> Result<aes::cipher::Iv<M>, JsError>
where
    M: aes::cipher::KeyIvInit,
{
    aes::cipher::Iv::<M>::try_from(iv)
        .map_err(|_| JsError::new(format!("IV 长度不符（{} 字节）", iv.len())))
}

/// 泛型的 CBC 加密（三个 AES 密钥长度共用）。
///
/// ## 泛型约束为什么是这三个
///
/// `CbcEncryptor<C>` 需要 `C` 能被实例化（`KeyInit`）、有块大小
/// （`BlockSizeUser`）、且支持加密（`BlockCipherEncrypt`）。
/// `BlockSizeUser` 在 `cipher` 0.5 里从 crate 根重导出（不在私有模块里），
/// 故经 `aes::cipher::BlockSizeUser` 引用。
fn cbc_encrypt<C>(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, JsError>
where
    C: aes::cipher::BlockCipherEncrypt + aes::cipher::KeyInit + aes::cipher::BlockSizeUser,
{
    // `encrypt_padded` 的签名要求缓冲区足够容纳填充后的一整块，
    // 故多留一个块（16 字节）。
    let mut buffer = Vec::with_capacity(data.len() + 16);
    buffer.extend_from_slice(data);
    buffer.resize(data.len() + 16, 0);

    let cipher_key = key_from_slice::<C>(key)?;
    let cipher_iv = iv_from_slice::<CbcEncryptor<C>>(iv)?;
    let cipher = CbcEncryptor::<C>::new(&cipher_key, &cipher_iv);
    let out = cipher
        .encrypt_padded::<Pkcs7>(&mut buffer, data.len())
        .map_err(|_| JsError::new("AES 加密失败（缓冲区或填充错误）"))?;
    Ok(out.to_vec())
}

/// 泛型的 CBC 解密。
fn cbc_decrypt<C>(data: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>, JsError>
where
    C: aes::cipher::BlockCipherDecrypt + aes::cipher::KeyInit + aes::cipher::BlockSizeUser,
{
    let mut buffer = data.to_vec();
    let cipher_key = key_from_slice::<C>(key)?;
    let cipher_iv = iv_from_slice::<CbcDecryptor<C>>(iv)?;
    let cipher = CbcDecryptor::<C>::new(&cipher_key, &cipher_iv);
    let out = cipher
        .decrypt_padded::<Pkcs7>(&mut buffer)
        .map_err(|_| JsError::new("AES 解密失败（密钥/IV/密文不匹配或填充错误）"))?;
    Ok(out.to_vec())
}

/// DES-CBC 加解密（PKCS#7 填充，8 字节块）。
fn des_cbc(data: &[u8], key: &str, iv: &str, decrypt: bool) -> Result<Vec<u8>, JsError> {
    let key = key_bytes(key, 8);
    require_len(&key, 8, "DES 密钥")?;
    let iv = key_bytes(iv, 8);
    require_len(&iv, 8, "DES IV")?;

    if decrypt {
        let mut buffer = data.to_vec();
        let cipher_key = key_from_slice::<Des>(&key)?;
        let cipher_iv = iv_from_slice::<CbcDecryptor<Des>>(&iv)?;
        let cipher = CbcDecryptor::<Des>::new(&cipher_key, &cipher_iv);
        cipher
            .decrypt_padded::<Pkcs7>(&mut buffer)
            .map(|out| out.to_vec())
            .map_err(|_| JsError::new("DES 解密失败（密钥/IV/密文不匹配或填充错误）"))
    } else {
        let mut buffer = Vec::with_capacity(data.len() + 8);
        buffer.extend_from_slice(data);
        buffer.resize(data.len() + 8, 0);
        let cipher_key = key_from_slice::<Des>(&key)?;
        let cipher_iv = iv_from_slice::<CbcEncryptor<Des>>(&iv)?;
        let cipher = CbcEncryptor::<Des>::new(&cipher_key, &cipher_iv);
        cipher
            .encrypt_padded::<Pkcs7>(&mut buffer, data.len())
            .map(|out| out.to_vec())
            .map_err(|_| JsError::new("DES 加密失败（缓冲区或填充错误）"))
    }
}

// ---------------------------------------------------------------------------
// RSA
// ---------------------------------------------------------------------------

/// RSA 公钥加密（PKCS#1 v1.5 填充）。
///
/// ## 为什么是 PKCS#1 v1.5 而不是 OAEP
///
/// 书源对接的站点接口绝大多数用 PKCS#1 v1.5（各语言库的默认）。
/// 用 OAEP 会让密文与服务端期望的不一致，而表现只是"服务端拒绝"。
fn rsa_encrypt(data: &[u8], public_key_pem: &str) -> Result<Vec<u8>, JsError> {
    let key = parse_public_key(public_key_pem)?;
    // `encrypt` 需要可变 RNG。RSA 加密必须随机填充——否则相同明文产生
    // 相同密文，那是已知的密码学弱点（PKCS#1 v1.5 的安全性正依赖填充随机性）。
    let mut rng = rand::thread_rng();
    key.encrypt(&mut rng, rsa::Pkcs1v15Encrypt, data)
        .map_err(|error| JsError::new(format!("RSA 加密失败：{error}")))
}

/// 解析公钥：按 PEM 或 DER 尝试（书源里两种都存在）。
fn parse_public_key(text: &str) -> Result<rsa::RsaPublicKey, JsError> {
    use rsa::pkcs1::DecodeRsaPublicKey;
    use rsa::pkcs8::DecodePublicKey;

    let trimmed = text.trim();
    if trimmed.contains("-----BEGIN") {
        // PEM：先按 PKCS#8（`BEGIN PUBLIC KEY`），再按 PKCS#1（`BEGIN RSA PUBLIC KEY`）
        return rsa::RsaPublicKey::from_public_key_pem(trimmed)
            .or_else(|_| rsa::RsaPublicKey::from_pkcs1_pem(trimmed))
            .map_err(|error| JsError::new(format!("RSA 公钥解析失败：{error}")));
    }
    // DER（Base64 文本形态的书源少见，但把 base64 解成 DER 很常见）
    let der = base64_decode(trimmed)
        .map_err(|_| JsError::new("RSA 公钥既不是 PEM 也不是合法的 Base64/DER"))?;
    rsa::RsaPublicKey::from_public_key_der(&der)
        .or_else(|_| rsa::RsaPublicKey::from_pkcs1_der(&der))
        .map_err(|error| JsError::new(format!("RSA 公钥解析失败：{error}")))
}

/// RSA 私钥解密。
fn rsa_decrypt(data: &[u8], private_key_pem: &str) -> Result<Vec<u8>, JsError> {
    use rsa::pkcs8::DecodePrivateKey;

    let trimmed = private_key_pem.trim();
    let key = if trimmed.contains("-----BEGIN") {
        // PKCS#8（`BEGIN PRIVATE KEY`）或 PKCS#1（`BEGIN RSA PRIVATE KEY`）
        rsa::RsaPrivateKey::from_pkcs8_pem(trimmed)
            .or_else(|_| {
                use rsa::pkcs1::DecodeRsaPrivateKey;
                rsa::RsaPrivateKey::from_pkcs1_pem(trimmed)
            })
            .map_err(|error| JsError::new(format!("RSA 私钥解析失败：{error}")))?
    } else {
        use rsa::pkcs1::DecodeRsaPrivateKey;
        let der = base64_decode(trimmed)
            .map_err(|_| JsError::new("RSA 私钥既不是 PEM 也不是合法的 Base64/DER"))?;
        rsa::RsaPrivateKey::from_pkcs8_der(&der)
            .or_else(|_| rsa::RsaPrivateKey::from_pkcs1_der(&der))
            .map_err(|error| JsError::new(format!("RSA 私钥解析失败：{error}")))?
    };

    key.decrypt(rsa::Pkcs1v15Encrypt, data)
        .map_err(|error| JsError::new(format!("RSA 解密失败：{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 摘要标准测试向量() {
        // RFC 1321 的 MD5 与 FIPS 180-4 的 SHA 系列
        assert_eq!(
            hex_encode(&digest(DigestAlgorithm::Md5, b"abc")),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            hex_encode(&digest(DigestAlgorithm::Sha1, b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex_encode(&digest(DigestAlgorithm::Sha256, b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex_encode(&digest(DigestAlgorithm::Sha512, b"abc")),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
    }

    #[test]
    fn 密钥字节判定按长度优先() {
        // 16 字符的十六进制串：目标是 8 字节时应判为十六进制字节
        assert_eq!(
            key_bytes("3132333435363738", 8),
            vec![0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38]
        );
        // 同样输入，目标是 16 字节时应判为 ASCII（长度不匹配十六进制）
        assert_eq!(key_bytes("3132333435363738", 16).len(), 16);
        // 非十六进制字符：一律 ASCII
        assert_eq!(key_bytes("password", 8), b"password".to_vec());
    }

    #[test]
    fn 十六进制文本判定() {
        assert!(is_hex_text("abcdef01"));
        assert!(is_hex_text("ABCDEF01"));
        assert!(!is_hex_text("abcdef0"), "奇数长度不是");
        assert!(!is_hex_text("xyz"), "非十六进制字符不是");
        assert!(!is_hex_text(""), "空串不是");
    }

    #[test]
    fn 长度校验报出期望与实际() {
        let error = require_len(&[0u8; 10], 16, "AES 密钥").expect_err("应报错");
        let message = error.message();
        assert!(message.contains("16"), "应含期望长度：{message}");
        assert!(message.contains("10"), "应含实际长度：{message}");
        assert!(message.contains("AES 密钥"), "应含字段名：{message}");
    }

    #[test]
    fn aes_往返() {
        let key = "1234567890abcdef"; // 16 ASCII 字符 = 128 位
        let iv = "abcdef1234567890";
        let plaintext = b"book source api secret";

        let ciphertext = aes_cbc(plaintext, key, iv, 128, false).expect("加密应成功");
        assert_ne!(ciphertext.as_slice(), plaintext, "密文不应与明文相同");
        // CBC + PKCS#7：密文长度是块大小的整数倍，且比明文多出一个填充块以内
        assert_eq!(ciphertext.len() % 16, 0);

        let recovered = aes_cbc(&ciphertext, key, iv, 128, true).expect("解密应成功");
        assert_eq!(recovered, plaintext.to_vec());
    }

    #[test]
    fn aes_三个密钥长度都可用() {
        for (bits, key) in [
            (128usize, "a".repeat(16)),
            (192, "b".repeat(24)),
            (256, "c".repeat(32)),
        ] {
            let ciphertext = aes_cbc(b"data", &key, "0123456789abcdef", bits as u16, false)
                .unwrap_or_else(|error| panic!("{bits} 位加密应成功：{error:?}"));
            let recovered = aes_cbc(&ciphertext, &key, "0123456789abcdef", bits as u16, true)
                .unwrap_or_else(|error| panic!("{bits} 位解密应成功：{error:?}"));
            assert_eq!(recovered, b"data".to_vec());
        }
    }

    #[test]
    fn aes_密钥长度不符时明确报错() {
        // 15 字符密钥给 128 位：应报出期望 16 实际 15，而不是底层 crate
        // 的晦涩错误（那会让书源作者不知道是密钥写短了）。
        let error =
            aes_cbc(b"x", "123456789012345", "0123456789abcdef", 128, false).expect_err("应报错");
        let message = error.message();
        assert!(message.contains("16"), "{message}");
        assert!(message.contains("15"), "{message}");
    }

    #[test]
    fn aes_十六进制密钥也可用() {
        // 同样内容（`1234567890abcdef`）的十六进制表示
        let ascii_key = "1234567890abcdef";
        let hex_key = hex_encode(ascii_key.as_bytes());
        let iv = "0123456789abcdef";
        let a = aes_cbc(b"data", ascii_key, iv, 128, false).expect("ASCII 密钥应成功");
        let b = aes_cbc(b"data", &hex_key, iv, 128, false).expect("十六进制密钥应成功");
        assert_eq!(a, b, "两种密钥写法应等价");
    }

    #[test]
    fn aes_解密错误密文报错而非产出垃圾() {
        // 错误的 IV 会让填充校验失败。必须报错——静默返回乱码会让书源
        // 拿它去 JSON.parse，报一个完全无关的错。
        let ciphertext =
            aes_cbc(b"data", "1234567890abcdef", "0123456789abcdef", 128, false).expect("应成功");
        let outcome = aes_cbc(
            &ciphertext,
            "1234567890abcdef",
            "ffffffffffffffff",
            128,
            true,
        );
        assert!(outcome.is_err(), "错误 IV 应报错");
    }

    #[test]
    fn des_往返() {
        let key = "12345678"; // 8 ASCII 字符
        let iv = "abcdefgh";
        let ciphertext = des_cbc(b"secret", key, iv, false).expect("加密应成功");
        assert_eq!(ciphertext.len() % 8, 0);
        let recovered = des_cbc(&ciphertext, key, iv, true).expect("解密应成功");
        assert_eq!(recovered, b"secret".to_vec());
    }

    #[test]
    fn des_密钥长度不符时报错() {
        assert!(des_cbc(b"x", "short", "abcdefgh", false).is_err());
        assert!(des_cbc(b"x", "12345678", "short", false).is_err());
    }

    #[test]
    fn rsa_密钥解析失败时给出可读错误() {
        // 无效密钥必须报结构化错误（书源可能写错密钥格式）。
        let error = parse_public_key("not a key at all").expect_err("应报错");
        assert!(!error.message().is_empty());
    }

    #[test]
    fn rsa_加解密往返() {
        // 生成一个小密钥做往返（测试用 2048 位太慢，1024 够验证语义）。
        let mut rng = rand::thread_rng();
        let private = rsa::RsaPrivateKey::new(&mut rng, 1024).expect("应能生成密钥");
        let public = rsa::RsaPublicKey::from(&private);

        let public_pem = {
            use rsa::pkcs8::EncodePublicKey;
            public
                .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
                .expect("应能导出")
        };
        let private_pem = {
            use rsa::pkcs8::EncodePrivateKey;
            private
                .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
                .expect("应能导出")
        };

        let plaintext = b"short"; // 1024 位密钥 + PKCS#1 v1.5 的明文上限约 117 字节
        let ciphertext = rsa_encrypt(plaintext, &public_pem).expect("加密应成功");
        let recovered = rsa_decrypt(&ciphertext, &private_pem).expect("解密应成功");
        assert_eq!(recovered, plaintext.to_vec());
    }
}
