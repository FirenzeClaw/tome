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
//! | [`format`] | 格式识别（含 KFX/Topaz 显式拒绝） | T077-T081 |
//! | [`epub`] | EPUB 解析（zip + OPF/NCX） | T077 / FR-021 |
//! | [`mobi`] | MOBI/AZW3 解析（分阶段） | T078-T081 / FR-022 |
//! | [`library`] | 库根扫描与 `--copy` 导入 | T082 / FR-023~025 |
//! | [`provider`] | `ContentProvider` 本地 adapter | T083 / A4 |
//!
//! ## 完成度
//!
//! [DONE] [`encoding`] [`toc_rule`] [`split`] [`txt`] [`hash`] [`format`]
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
