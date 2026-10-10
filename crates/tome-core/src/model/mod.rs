//! # 领域 DTO —— 引擎的公共语言
//!
//! 对应 `specs/001-pc-engine-core/data-model.md` 的 18 个实体。字段与该文件
//! **逐一对齐**：存储层（tome-store）据此建表，接缝 trait 据此定义签名，
//! 上层据此渲染。任何字段的增删都是跨 crate 的协议变更。
//!
//! ## 三条硬约束
//!
//! 1. **阅读痕迹一律字符区间**（FR-028）：`chapter_index` + `char_offset` + `length`，
//!    **MUST NOT** 出现渲染坐标。这是改字号/改窗口后不错位的前提。
//!    `char_offset` 与 `length` 的单位是 **Unicode 字符数**，不是字节数——
//!    中文正文下两者差 3 倍，混淆会导致高亮整体偏移。
//! 2. **纯数据，无 DB/UI 注解**：不派生 serde，不带 SQL 映射，不带图形类型。
//!    序列化与持久化是各实现 crate 的关切（宪法 IV 单向依赖）。
//! 3. **不藏业务逻辑**：这里只放数据形状。求值、净化、匹配的算法属于
//!    tome-rule / tome-source。DTO 上的方法仅限不变量可自证的简单访问器。
//!
//! ## 单元一览
//!
//! | 模块 | 实体 |
//! |---|---|
//! | `book` | `Book` / `BookOrigin` / `Chapter` / `LibraryRoot` |
//! | `source` | `Source` / `SourceKind` / `SourceGroup` |
//! | `rule` | `ReplaceRule` / `HighlightRule` / `DictRule` / `TxtTocRule` / `RuleScope` / `RuleTier` |
//! | `trace` | `ReadProgress` / `Bookmark` / `Highlight` / `Memo` / `ReadRecord` |
//! | `asset` | `Cookie` / `ImageCacheRef` |
//! | `auxiliary` | `ChapterCacheEntry` / `HttpLogEntry` / `SearchKeyword` |
//! | [`theme`] | `Theme`（设计 token 纯值类型，零图形依赖；OQ#11→D） |

mod asset;
mod auxiliary;
mod book;
mod rule;
mod source;
pub mod theme;
mod trace;

pub use asset::{Cookie, ImageCacheRef};
pub use auxiliary::{ChapterCacheEntry, HttpLogEntry, SearchKeyword};
pub use book::{Book, BookOrigin, Chapter, LibraryRoot};
pub use rule::{DictRule, HighlightRule, ReplaceRule, RuleScope, RuleTier, TxtTocRule};
pub use source::{Source, SourceGroup, SourceKind};
pub use theme::{
    Color, DayTheme, FontRole, FontStack, Fonts, Motion, NightTheme, Palette, Radii,
    ReadingMetrics, ReadingToolbar, Rgba, Shadow, Shadows, ShellDims, SpacingScale, Swatches,
    Texture, Theme, TypeScale, TypeStep, Weight,
};
pub use trace::{Bookmark, Highlight, Memo, ReadProgress, ReadRecord};
