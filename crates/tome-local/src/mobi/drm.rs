//! \[DONE] MOBI DRM 检测与格式拒绝（tasks.md T080 / research R4）
//!
//! # research R4 点名的陷阱
//!
//! > **DRM 判断**：PalmDOC 头偏移 0x0C 的 Encryption Type（0 无/1 单 PID/
//! > 2 多 PID）。**陷阱：compression=17480("DH") 是 HUFF/CDIC 不是 DRM**。
//! > KFX/Topaz 显式拒绝。
//!
//! 这两个东西在**同一个结构里相邻**（PalmDOC 头的 0x00 是压缩算法、
//! 0x0C 是加密类型），而 `17480` 这个数值看上去很像某种"加密标识"。
//! 实测过的实现里，把 `17480` 当 DRM 报错的案例正是这个错觉导致的。
//!
//! # 三类"不能读"必须可区分
//!
//! | 情形 | 用户能做什么 | 错误的报法会让用户做什么 |
//! |---|---|---|
//! | **DRM 加密** | 用官方工具去 DRM / 换无 DRM 的副本 | 报"格式不支持"→ 用户去转格式，白费功夫 |
//! | **KFX / Topaz** | 换成 EPUB/MOBI 版本 | 报"文件损坏"→ 用户以为文件坏了，重新下载得到同一个文件 |
//! | **真的损坏** | 重新获取文件 | 报"DRM"→ 用户去找已买的密钥，找不到 |
//!
//! 故本模块的判据按**结构证据**而不是"大概像什么"：
//! 加密类型读的是 MOBI 头声明的那一个字段，格式拒绝读的是容器魔数。
//!
//! # 什么情况算"检测到 DRM"
//!
//! 两个独立证据都指向加密时才判定：
//!
//! 1. PalmDOC 头的 `encryption_type != 0`
//! 2. MOBI 头声明了 DRM 偏移与数量（`drm_offset != 0 && drm_count != 0`）
//!
//! 只用 (1) 会误报：部分**未加密**的旧文件在 0x0C 处留有非零的垃圾值
//! （生成工具的未初始化字段）。(2) 是独立的结构证据，两者同时成立才可信。
//! 只有 (1) 时**降级为警告**并继续解析——若真的是 DRM，解压出来会是乱码，
//! 而那时用户看到的是乱码书而不是错误，所以这个降级要保守。

use crate::error::{LocalError, LocalResult};
use crate::format::LocalFormat;
use crate::mobi::header::{Compression, MobiHeader};

/// DRM 检测的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrmVerdict {
    /// 未加密，可以解析。
    Clear,
    /// 疑似加密但证据不完整（只在加密类型字段上看到非零）。
    ///
    /// 调用方**继续解析**，但把 `reason` 报给用户——若结果是乱码，
    /// 这条警告就是解释。
    Suspect {
        /// 诊断说明
        reason: String,
    },
    /// 确定加密，无法解析。
    Encrypted {
        /// 加密类型码（0x0C 处的值）
        encryption_type: u16,
        /// DRM 偏移（来自 MOBI 头）
        drm_offset: u32,
        /// DRM 记录数量
        drm_count: u32,
    },
}

/// 检查 DRM（T080 的主入口）。
///
/// # 为什么把 compression 也纳进来
///
/// 本函数是"这个文件能不能读"的唯一判据点，而 HUFF/CDIC 的**可读性**取决于
/// 阶段（T079 之前 HUFF/CDIC 读不了）。把两者的判定放一起，让调用方在一个
/// 地方就能得到"能读 / 为什么不能读"，而不是散在两处。
pub fn check_drm(header: &MobiHeader) -> DrmVerdict {
    let has_drm_records = header.drm_offset != 0 && header.drm_count != 0;

    if header.encryption_type != 0 {
        if has_drm_records {
            return DrmVerdict::Encrypted {
                encryption_type: header.encryption_type,
                drm_offset: header.drm_offset,
                drm_count: header.drm_count,
            };
        }
        // 只有加密类型字段非零：证据不足，降级为警告（见模块文档）。
        return DrmVerdict::Suspect {
            reason: format!(
                "PalmDOC 头声明加密类型 {}，但 MOBI 头没有 DRM 记录（偏移 {}、数量 {}）——\
                 按未加密处理，若正文为乱码则说明该文件确实受 DRM 保护",
                header.encryption_type, header.drm_offset, header.drm_count
            ),
        };
    }

    DrmVerdict::Clear
}

/// 压缩算法的支持度。
///
/// # 三个变体与它们的处置
///
/// | 变体 | 可以解压 | 调用方动作 |
/// |---|---|---|
/// | [`Self::Supported`] | 是（无压缩 / PalmDOC） | 直接解 |
/// | [`Self::HuffCdic`] | 是（T079 已落地） | 走 `huffcdic` 分派 |
/// | [`Self::Unknown`] | **否** | 明确报错 |
///
/// 注意 `HuffCdic` **不是**"不支持"——首版把它叫 `NeedsHuffCdic` 并在
/// 解析流程里直接返回错误，使 `huffcdic::decompress_document` 成了死代码
/// （实测发现：真实 `-c2` 文件走不到解压那一步）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionSupport {
    /// 无压缩或 PalmDOC——本实现直接解压
    Supported,
    /// HUFF/CDIC——由 `mobi::huffcdic` 解压（T079）
    HuffCdic,
    /// 未知的压缩算法码
    Unknown(u16),
}

impl CompressionSupport {
    /// 按压缩算法判定。
    pub fn of(compression: Compression) -> Self {
        match compression {
            Compression::None | Compression::PalmDoc => Self::Supported,
            Compression::HuffCdic => Self::HuffCdic,
            Compression::Unknown(code) => Self::Unknown(code),
        }
    }

    /// 转为可报告的错误（**仅未知算法**时给出；见枚举文档）。
    pub fn into_error(self) -> Option<LocalError> {
        match self {
            Self::Supported | Self::HuffCdic => None,
            Self::Unknown(code) => Some(LocalError::Damaged {
                reason: format!("未知的压缩算法码 {code}"),
            }),
        }
    }
}

/// 显式拒绝的格式（KFX / Topaz）。
///
/// # 为什么要在**解析前**拒绝
///
/// KFX 与 Topaz **也是** PalmDB 容器（Topaz 的 type/creator 是 `TEXtREAd`），
/// 若不加拒绝，它们会被送去 MOBI 解析器，在那里因为 `MOBI` 魔数缺失而报
/// "记录 0 处不是 MOBI 魔数"——用户拿到的是这个技术细节，而不是
/// "这种格式本产品不支持"。两条信息对用户的价值差得很远。
pub fn reject_unsupported(format: LocalFormat) -> LocalResult<()> {
    match format {
        LocalFormat::Kfx => Err(LocalError::UnsupportedFormat {
            format: "kfx".to_string(),
        }),
        LocalFormat::Topaz => Err(LocalError::UnsupportedFormat {
            format: "topaz".to_string(),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobi::header::TextEncoding;

    /// 构造一个各字段可控的 `MobiHeader`。
    fn header_with(encryption_type: u16, drm_offset: u32, drm_count: u32) -> MobiHeader {
        MobiHeader {
            compression: Compression::PalmDoc,
            text_length: 1000,
            record_count: 5,
            record_size: 4096,
            encryption_type,
            header_length: 232,
            mobi_type: 2,
            encoding: TextEncoding::Utf8,
            unique_id: 1,
            file_version: 6,
            first_non_book_index: 6,
            first_content_record: 1,
            exth_flags: 0x40,
            drm_offset,
            drm_count,
            record_start_offset: 0,
            original_file_length: 2000,
            kf8_boundary: 0,
            fdst_index: 0,
            fdst_count: 0,
        }
    }

    #[test]
    fn clear_file_is_not_flagged() {
        assert_eq!(
            check_drm(&header_with(0, 0, 0)),
            DrmVerdict::Clear,
            "加密类型 0 = 未加密"
        );
    }

    #[test]
    fn encryption_type_with_drm_records_is_encrypted() {
        let verdict = check_drm(&header_with(2, 0x1234, 3));
        match verdict {
            DrmVerdict::Encrypted {
                encryption_type,
                drm_offset,
                drm_count,
            } => {
                assert_eq!(encryption_type, 2, "2 = 多 PID 加密");
                assert_eq!(drm_offset, 0x1234);
                assert_eq!(drm_count, 3);
            }
            other => panic!("应判定为加密，实际 {other:?}"),
        }
    }

    #[test]
    fn single_pid_encryption_is_also_detected() {
        assert!(matches!(
            check_drm(&header_with(1, 100, 1)),
            DrmVerdict::Encrypted { .. }
        ));
    }

    #[test]
    fn nonzero_encryption_without_drm_records_is_only_a_warning() {
        // 未初始化的垃圾值会造成误报，故证据不足时降级而不是硬判。
        let verdict = check_drm(&header_with(1, 0, 0));
        match verdict {
            DrmVerdict::Suspect { reason } => {
                assert!(reason.contains("加密类型 1"), "实际 {reason}");
            }
            other => panic!("应降级为疑似，实际 {other:?}"),
        }
    }

    #[test]
    fn drm_records_alone_do_not_trigger_encryption() {
        // 加密类型为 0 时，即便有 DRM 字段也不判加密（字段可能是残留）。
        assert_eq!(check_drm(&header_with(0, 500, 2)), DrmVerdict::Clear);
    }

    #[test]
    fn huffcdic_is_not_reported_as_drm() {
        // **R4 点名的陷阱**：17480 是压缩算法，与 DRM 无关。
        let mut header = header_with(0, 0, 0);
        header.compression = Compression::HuffCdic;

        // DRM 判定必须保持不变。
        assert_eq!(
            check_drm(&header),
            DrmVerdict::Clear,
            "compression=17480 绝不能被当成 DRM"
        );

        // 而它的**可读性**由压缩支持度表达，与 DRM 是两个独立的轴。
        assert_eq!(
            CompressionSupport::of(header.compression),
            CompressionSupport::HuffCdic
        );
    }

    #[test]
    fn huffcdic_is_treated_as_supported() {
        // T079 已落地，故 HUFF/CDIC **不是**"不支持"——首版把它当成错误
        // 直接返回，使 `huffcdic::decompress_document` 成了死代码。
        assert!(
            CompressionSupport::of(Compression::HuffCdic)
                .into_error()
                .is_none(),
            "HUFF/CDIC 应被分派给解压器，而不是在此报错"
        );
    }

    #[test]
    fn unknown_compression_error_message_never_mentions_drm() {
        // 错误文案里不能出现 DRM——那会把用户引向完全错误的方向。
        let error = CompressionSupport::of(Compression::Unknown(17481))
            .into_error()
            .expect("未知算法应报错");
        let text = error.to_string().to_lowercase();
        assert!(!text.contains("drm"), "实际 {text}");
        assert!(text.contains("17481"), "应指明码位，实际 {text}");
    }

    #[test]
    fn drm_error_is_a_capability_error_with_a_distinct_id() {
        let error = LocalError::DrmProtected { encryption_type: 1 };
        match error.kind() {
            tome_core::ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "localFormat:drm");
            }
            other => panic!("DRM 应是能力类错误，实际 {other:?}"),
        }
    }

    #[test]
    fn palmdoc_and_none_are_supported() {
        assert_eq!(
            CompressionSupport::of(Compression::PalmDoc),
            CompressionSupport::Supported
        );
        assert_eq!(
            CompressionSupport::of(Compression::None),
            CompressionSupport::Supported
        );
        assert!(CompressionSupport::of(Compression::None)
            .into_error()
            .is_none());
    }

    #[test]
    fn unknown_compression_is_reported_with_its_code() {
        let support = CompressionSupport::of(Compression::Unknown(77));
        assert_eq!(support, CompressionSupport::Unknown(77));
        let error = support.into_error().expect("应报错");
        assert!(error.to_string().contains("77"));
    }

    #[test]
    fn kfx_is_explicitly_rejected() {
        let error = reject_unsupported(LocalFormat::Kfx).expect_err("KFX 应被拒绝");
        match error {
            LocalError::UnsupportedFormat { format } => assert_eq!(format, "kfx"),
            other => panic!("应为不支持格式，实际 {other:?}"),
        }
    }

    #[test]
    fn topaz_is_explicitly_rejected() {
        let error = reject_unsupported(LocalFormat::Topaz).expect_err("Topaz 应被拒绝");
        match error {
            LocalError::UnsupportedFormat { format } => assert_eq!(format, "topaz"),
            other => panic!("应为不支持格式，实际 {other:?}"),
        }
    }

    #[test]
    fn supported_formats_pass_the_rejection_gate() {
        assert!(reject_unsupported(LocalFormat::Txt).is_ok());
        assert!(reject_unsupported(LocalFormat::Epub).is_ok());
        assert!(reject_unsupported(LocalFormat::Mobi).is_ok());
    }

    #[test]
    fn the_three_failure_modes_have_distinct_error_kinds() {
        // 三种"读不了"必须能被上层区分（见模块文档的表）。
        use tome_core::ErrorKind;

        let drm = LocalError::DrmProtected { encryption_type: 1 }.kind();
        let kfx = LocalError::UnsupportedFormat {
            format: "kfx".to_string(),
        }
        .kind();
        let damaged = LocalError::damaged("truncated").kind();

        assert!(matches!(drm, ErrorKind::UnsupportedCapability { .. }));
        assert!(matches!(kfx, ErrorKind::UnsupportedCapability { .. }));
        assert_eq!(damaged, ErrorKind::Content);

        // DRM 与 KFX 同属能力类，但 capability_id 必须不同——
        // 否则上层无法区分"去 DRM"与"换格式"这两种建议。
        let (
            ErrorKind::UnsupportedCapability { capability: a },
            ErrorKind::UnsupportedCapability { capability: b },
        ) = (drm, kfx)
        else {
            unreachable!("上面已断言两者都是能力类");
        };
        assert_ne!(a.capability_id(), b.capability_id());
    }
}
