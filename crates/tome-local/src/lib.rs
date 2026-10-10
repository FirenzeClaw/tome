//! # tome-local —— 本地书解析
//!
//! 解析本地书籍文件（TXT / EPUB / MOBI，MOBI 分阶段）、扫描库根目录、识别 TOC。
//!
//! ## 在本项目中的位置
//!
//! - 依赖 `tome-core` / `tome-store`（持久化解析结果）与 `tome-rule`
//!   （复用规则引擎既有的切分语义做 TOC 识别，避免同一逻辑实现两遍）。
//! - **不依赖 `tome-net`**：本地书全程离线，这条依赖边若出现即为架构违规。
//!   故字符集检测在本 crate 内自带一份（[`encoding`]），判据与 `tome-net`
//!   一致但不共享代码——那条依赖边比一次重复更贵。
//!
//! ## 风险提示
//!
//! MOBI/AZW3 是 S1 最大单项工作量（research.md R4）。HUFF/CDIC 与 KF8 路径
//! 必须用真实文件验证；KF8 可拆为后续任务。本地文件一律视为不可信输入。
//!
//! ## 关键入口
//!
//! | 模块 | 职责 | 任务 |
//! |---|---|---|
//! | [`encoding`] | 字符集检测与解码（本地文件无声明） | T076 / FR-020 |
//! | [`toc_rule`] | 目录规则求值与多条回退 | T076 / FR-020 |
//! | [`split`] | 无目录时的按大小切分 | T076 / FR-020 |
//! | [`txt`] | TXT 导入统领（编码 → 目录 → 切分） | T076 / FR-020 |
//! | [`hash`] | 归一化内容哈希（`origin_id`） | T082 / FR-025 |
//! | `format` | 格式识别（含 KFX/Topaz 显式拒绝） | T077-T081 |
//! | [`epub`] | EPUB 解析（zip + OPF/NCX） | T077 / FR-021 |
//! | [`mobi`] | MOBI/AZW3 解析（分阶段） | T078-T081 / FR-022 |
//! | [`library`] | 库根扫描与 `--copy` 导入 | T082 / FR-023~025 |
//! | [`provider`] | `ContentProvider` 本地 adapter | T083 / A4 |
//!
//! ## 完成度
//!
//! \[DONE] [`encoding`] [`toc_rule`] [`split`] [`txt`] [`hash`] `format`
//! ——T076 与 T082 的身份层已落地。

#![forbid(unsafe_code)]

pub mod encoding;
pub mod epub;
pub mod error;
pub mod format;
pub mod hash;
pub mod library;
pub mod mobi;
pub mod provider;
pub mod split;
pub mod toc_rule;
pub mod txt;

pub use error::{LocalError, LocalResult};
pub use format::LocalFormat;
pub use txt::{import_bytes, import_path, ParsedTxt, TocPoint};

/// 把 EPUB 的图片锚点转成接缝的 `ImageRef`，并把位置从**原始文本**校正到
/// **净化后文本**（PM 裁决 OQ#13）。
///
/// # 为什么放在 crate 根
///
/// 它连接两个模块：`epub`（产出锚点）与 `provider`（消费 `ChapterContent`）。
/// 放在任一侧都会让那个模块依赖另一侧的细节；放根部是"两模块的公共下游"。
///
/// # 与网络书侧的对应关系
///
/// `tome-source` 的 `image::align_anchors_after_purify` 做同样的事。两处
/// 各自实现是因为依赖方向不允许共享（`tome-local` 与 `tome-source` 互不
/// 依赖，都只依赖 `tome-store`/`tome-core`）。算法一致，各有测试钉住。
#[must_use]
pub fn epub_image_refs(
    raw: &str,
    purified: &str,
    anchors: &[epub::LocalImageAnchor],
) -> Vec<tome_core::seam::ImageRef> {
    if anchors.is_empty() {
        return Vec::new();
    }

    // 净化没改文本：偏移直接可用（最常见情形）
    if raw == purified {
        let mut refs: Vec<tome_core::seam::ImageRef> = anchors
            .iter()
            .map(|anchor| tome_core::seam::ImageRef {
                char_offset: anchor.char_offset,
                cache_key: anchor.src.clone(),
                alt: anchor.alt.clone(),
            })
            .collect();
        refs.sort_by_key(|image| image.char_offset);
        return refs;
    }

    let raw_chars: Vec<char> = raw.chars().collect();
    const CONTEXT: usize = 16;
    /// 上下文的最小长度 —— 更短的会误命中（见下方说明）。
    const MIN_CONTEXT: usize = 4;

    let mut refs = Vec::new();
    for anchor in anchors {
        let offset = anchor.char_offset as usize;
        if offset > raw_chars.len() {
            continue;
        }
        if offset == 0 {
            refs.push(tome_core::seam::ImageRef {
                char_offset: 0,
                cache_key: anchor.src.clone(),
                alt: anchor.alt.clone(),
            });
            continue;
        }

        // 从长到短逐级尝试上下文（与 `tome-source` 的实现同一策略）：
        // 整段前缀太脆，窗口里任何一个字符被净化碰到就匹配不上。
        // 退到 `MIN_CONTEXT` 字仍失败才放弃——更短的上下文会误命中。
        let mut relocated = None;
        let mut length = CONTEXT.min(offset);
        while length >= MIN_CONTEXT {
            let start = offset - length;
            let prefix: String = raw_chars[start..offset].iter().collect();
            // 用 `rfind`：上下文可能在净化后重复出现，取最后一个更接近原位置
            if let Some(found) = purified.rfind(&prefix) {
                relocated = Some(purified[..found + prefix.len()].chars().count() as i64);
                break;
            }
            length -= 1;
        }

        let Some(new_offset) = relocated else {
            // 连 2 字上下文都找不到：位置无从确定，丢弃。
            // 位置错的图比没有图更糟——它会被插到无关段落里。
            continue;
        };
        refs.push(tome_core::seam::ImageRef {
            char_offset: new_offset,
            cache_key: anchor.src.clone(),
            alt: anchor.alt.clone(),
        });
    }

    refs.sort_by_key(|image| image.char_offset);
    refs
}
