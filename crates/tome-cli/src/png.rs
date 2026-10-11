//! `[DONE]` 最小 PNG 编码（`tome render --out <png>` 的产物写入）
//!
//! ## 为什么手写而不引 `png` crate
//!
//! `contracts/cli.md` §2.3 只要求"输出 PNG"。引一个编码库会把它带进
//! `tome-cli` 的依赖面，而 CLI 是发布物的一部分——体积纪律要求
//! "MUST NOT 引入与功能无关的体积膨胀"。
//!
//! ## 格式：最小合法 PNG
//!
//! ```text
//!   签名(8) │ IHDR │ IDAT │ IEND
//! ```
//!
//! - `IHDR`：宽、高、位深 8、颜色类型 6（RGBA）、无隔行
//! - `IDAT`：zlib 流，内容为"每行前置一个过滤字节 0 的原始像素"
//! - `IEND`：空块
//!
//! `IDAT` 用 zlib 的 **stored**（未压缩）模式——它仍是**合法**的 zlib 流，
//! 任何 PNG 解码器都能读，只是文件较大（约裸像素 + 每 64KB 五字节开销）。
//!
//! `[WORKAROUND]` 若快照基线要入 git 且嫌体积大，换成 deflate 压缩能小
//! 一个数量级（文字页面压缩率极高）。替换点只有 [`zlib_stored`] 一个函数，
//! 其余块不变。**当前的取舍理由**：本产物是验证中间物，不入仓库
//! （`testdata/` 与 `tests/snapshots/` 的纪律见 AGENTS.md），故文件大小
//! 不是约束；而"零新依赖"是约束。
//!
//! ## 校验和的正确性怎么保证
//!
//! PNG 的每个块都带 CRC-32，zlib 流末尾带 Adler-32。**写错校验和会让
//! 解码器拒绝文件**，而症状是"文件看起来生成了但打不开"——那种失败在
//! 自动化里很难定位。故这两个校验和都有**已知答案测试**
//! （CRC-32 用标准向量 `"123456789" → 0xCBF43926`；Adler-32 用
//! `"Wikipedia" → 0x11E60398`）。

/// PNG 签名（8 字节，固定）。
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// 把 RGBA 像素编码为 PNG。
///
/// ## 像素数据短于声明尺寸时**补零**而非报错
///
/// 那不该发生（`RenderedImage::is_consistent` 会先被断言），但产出合法
/// PNG 比产出半截文件好——后者在某些查看器里会被静默接受并显示错乱内容。
#[must_use]
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() + rgba.len() / 64 + 128);
    out.extend_from_slice(&PNG_SIGNATURE);

    // IHDR：宽(4) 高(4) 位深(1) 颜色类型(1) 压缩(1) 过滤(1) 隔行(1)
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_chunk(&mut out, b"IHDR", &ihdr);

    // 原始扫描线：每行前置过滤字节 0（None）
    let stride = width as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for row in 0..height as usize {
        raw.push(0);
        let start = row * stride;
        if start < rgba.len() {
            let end = (start + stride).min(rgba.len());
            raw.extend_from_slice(&rgba[start..end]);
        }
        raw.resize((row + 1) * (stride + 1), 0);
    }

    write_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// 写一个 PNG 块（长度 + 类型 + 数据 + CRC-32）。
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// zlib 流的 **stored**（未压缩）封装。
///
/// 格式：2 字节头（`0x78 0x01`）+ 若干 stored 块 + 4 字节 Adler-32。
/// 每块头为 1 字节 `BFINAL|BTYPE=00` + 2 字节 `LEN`（小端）+
/// 2 字节 `NLEN`（`LEN` 的反码）。
#[must_use]
pub fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let block_count = data.len() / 65535 + 1;
    let mut out = Vec::with_capacity(data.len() + block_count * 5 + 16);

    // CMF = 0x78（deflate，32K 窗口）；FLG = 0x01（校验位合法、级别 0）
    out.push(0x78);
    out.push(0x01);

    let mut offset = 0usize;
    loop {
        let remaining = data.len().saturating_sub(offset);
        let len = remaining.min(65_535);
        let is_final = offset + len >= data.len();
        out.push(u8::from(is_final));
        out.extend_from_slice(&(len as u16).to_le_bytes());
        out.extend_from_slice(&(!(len as u16)).to_le_bytes());
        out.extend_from_slice(&data[offset..offset + len]);
        offset += len;
        if is_final {
            break;
        }
    }

    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Adler-32 校验（zlib 流尾的校验和）。
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a = 1u32;
    let mut b = 0u32;
    for byte in data {
        a = (a + u32::from(*byte)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// CRC-32（PNG 块校验，IEEE 802.3 多项式）。
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **已知答案**：CRC-32 的标准测试向量。
    ///
    /// `"123456789"` → `0xCBF43926` 是 CRC-32/ISO-HDLC 的标准校验值
    /// （见 ITU-T V.42 / 广泛引用的 CRC 目录）。本表错了会让**每个** PNG
    /// 块校验失败，而症状是"文件生成了但打不开"。
    #[test]
    fn crc32_matches_the_standard_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    /// **已知答案**：Adler-32 的常见测试向量。
    #[test]
    fn adler32_matches_known_vectors() {
        // "Wikipedia" → 0x11E60398（zlib 手册与该算法广泛引用的例子）
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // 空输入 → 初始值 1
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn png_starts_with_the_signature() {
        let png = encode_png(2, 2, &[0u8; 16]);
        assert_eq!(&png[..8], &PNG_SIGNATURE);
    }

    #[test]
    fn png_contains_required_chunks() {
        let png = encode_png(2, 2, &[0u8; 16]);
        // 三块的类型串都应出现
        for kind in [b"IHDR".as_slice(), b"IDAT".as_slice(), b"IEND".as_slice()] {
            assert!(
                png.windows(4).any(|window| window == kind),
                "PNG 应含 {} 块",
                String::from_utf8_lossy(kind)
            );
        }
    }

    /// IHDR 的字段与请求一致（宽/高按大端写入）。
    #[test]
    fn ihdr_encodes_dimensions_big_endian() {
        let png = encode_png(0x0102, 0x0304, &[0u8; 4 * 0x0102 * 0x0304]);
        let ihdr_start = png
            .windows(4)
            .position(|window| window == b"IHDR")
            .expect("应有 IHDR");
        // 类型串之后就是数据
        let data = &png[ihdr_start + 4..];
        assert_eq!(&data[..4], &[0x00, 0x00, 0x01, 0x02], "宽应为大端");
        assert_eq!(&data[4..8], &[0x00, 0x00, 0x03, 0x04], "高应为大端");
        assert_eq!(data[8], 8, "位深应为 8");
        assert_eq!(data[9], 6, "颜色类型应为 6（RGBA）");
        assert_eq!(&data[10..13], &[0, 0, 0], "压缩/过滤/隔行都应为 0");
    }

    /// 每个块的 CRC 必须能被**独立重算**校验通过。
    ///
    /// ## 为什么这条比"含 IHDR/IDAT/IEND"强得多
    ///
    /// 只断言块类型串存在的话，CRC 写错（或漏写）也照样通过——而那样的
    /// 文件是**打不开**的。故按 PNG 的块结构逐块走一遍，重算每个 CRC。
    #[test]
    fn every_chunk_has_a_valid_crc() {
        let png = encode_png(3, 2, &[0xABu8; 4 * 3 * 2]);
        let mut offset = PNG_SIGNATURE.len();
        let mut chunks = 0usize;

        while offset + 8 <= png.len() {
            let length = u32::from_be_bytes([
                png[offset],
                png[offset + 1],
                png[offset + 2],
                png[offset + 3],
            ]) as usize;
            let kind = &png[offset + 4..offset + 8];
            let data_end = offset + 8 + length;
            assert!(
                data_end + 4 <= png.len(),
                "块 {} 的长度字段越界",
                String::from_utf8_lossy(kind)
            );

            let mut crc_input = Vec::new();
            crc_input.extend_from_slice(kind);
            crc_input.extend_from_slice(&png[offset + 8..data_end]);
            let expected = crc32(&crc_input);
            let actual = u32::from_be_bytes([
                png[data_end],
                png[data_end + 1],
                png[data_end + 2],
                png[data_end + 3],
            ]);
            assert_eq!(
                actual,
                expected,
                "块 {} 的 CRC 不匹配",
                String::from_utf8_lossy(kind)
            );

            chunks += 1;
            offset = data_end + 4;
            if kind == b"IEND" {
                break;
            }
        }

        assert_eq!(chunks, 3, "应有三个块（IHDR / IDAT / IEND）");
        assert_eq!(offset, png.len(), "块应恰好覆盖整个文件，无尾随字节");
    }

    /// zlib 流的结构：`0x78 0x01` 头 + stored 块 + Adler-32 尾。
    #[test]
    fn zlib_stream_has_valid_framing() {
        let payload = vec![0x5Au8; 100];
        let stream = zlib_stored(&payload);

        assert_eq!(stream[0], 0x78, "CMF 应为 0x78");
        assert_eq!(stream[1], 0x01, "FLG 应为 0x01");
        // stored 块头：BFINAL=1（单块）、LEN、NLEN
        assert_eq!(stream[2], 1, "单块的 BFINAL 应为 1");
        let len = u16::from_le_bytes([stream[3], stream[4]]);
        let nlen = u16::from_le_bytes([stream[5], stream[6]]);
        assert_eq!(len, 100);
        assert_eq!(nlen, !100u16, "NLEN 应为 LEN 的反码");
        assert_eq!(&stream[7..107], &payload[..]);

        // 尾部 Adler-32
        let tail = &stream[stream.len() - 4..];
        let expected = adler32(&payload);
        assert_eq!(
            u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]),
            expected
        );
    }

    /// 超过 65535 字节的输入要切成多个 stored 块（**只在最后一块标 final**）。
    #[test]
    fn large_payload_splits_into_multiple_blocks() {
        let payload = vec![0x11u8; 70_000];
        let stream = zlib_stored(&payload);

        // 第一块：非 final，LEN = 65535
        assert_eq!(stream[2], 0, "第一块不应是 final");
        assert_eq!(u16::from_le_bytes([stream[3], stream[4]]), 65_535);

        // 走到第二块
        let second = 7 + 65_535;
        assert_eq!(stream[second], 1, "第二块应是 final");
        let remaining = 70_000 - 65_535;
        assert_eq!(
            u16::from_le_bytes([stream[second + 1], stream[second + 2]]),
            remaining as u16
        );
    }

    #[test]
    fn empty_payload_still_produces_a_final_block() {
        let stream = zlib_stored(&[]);
        assert_eq!(stream[0], 0x78);
        assert_eq!(stream[1], 0x01);
        assert_eq!(stream[2], 1, "空输入也要有一个 final 块");
        assert_eq!(u16::from_le_bytes([stream[3], stream[4]]), 0);
        assert_eq!(adler32(&[]), 1);
    }

    /// 像素短于声明尺寸时**补零**（产出仍合法，不越界）。
    #[test]
    fn short_pixel_buffer_is_padded_not_truncated() {
        // 声明 4×4 但只给 4 个像素（16 字节）
        let png = encode_png(4, 4, &[0xFFu8; 16]);
        assert_eq!(&png[..8], &PNG_SIGNATURE);
        // 仍能逐块校验通过
        let mut offset = PNG_SIGNATURE.len();
        while offset + 8 <= png.len() {
            let length = u32::from_be_bytes([
                png[offset],
                png[offset + 1],
                png[offset + 2],
                png[offset + 3],
            ]) as usize;
            let kind = &png[offset + 4..offset + 8];
            let data_end = offset + 8 + length;
            let mut crc_input = Vec::new();
            crc_input.extend_from_slice(kind);
            crc_input.extend_from_slice(&png[offset + 8..data_end]);
            let actual = u32::from_be_bytes([
                png[data_end],
                png[data_end + 1],
                png[data_end + 2],
                png[data_end + 3],
            ]);
            assert_eq!(actual, crc32(&crc_input), "补零后的文件仍应 CRC 合法");
            offset = data_end + 4;
            if kind == b"IEND" {
                break;
            }
        }
    }
}
