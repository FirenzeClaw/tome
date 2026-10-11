//! `[DONE]` 本层错误类型与 `→ErrorKind` 映射（FR-057 / FR-058）
//!
//! ## 为什么本层要有自己的错误类型
//!
//! 与其余实现 crate 同构（AGENTS.md：每个 crate 拥有自己的错误类型；core 只
//! 承载**粗粒度分类**）。`tome-reader` 的错误细节（哪一步排版失败、哪个后端
//! 缺席）在此表达，接缝处映射为 [`tome_core::ErrorKind`] 供上层分支。
//!
//! ## 三条硬约束
//!
//! 1. **零面向用户的自然语言**（FR-058）：本模块只产出 [`tome_core::LocalizedMessage`]
//!    风格的**标识**（如 `error.reader.layout_failed`），文案映射留给 S3 外壳。
//!    引擎内出现中文提示语即违规——新增语言不该改引擎。
//! 2. **能力错误必须结构化**（FR-058 后半）："本层不支持的能力"抛携带能力标识的
//!    [`ReaderError::UnsupportedCapability`]，而不是一个笼统的失败。上层的
//!    `try/catch` 降级逻辑（如无渲染后端时改走结构验证）依赖这个标识。
//! 3. **不得以 `catch_unwind` 为错误处理手段**：release profile 是
//!    `panic = "abort"`（根 `Cargo.toml` / 001 §Context），`panic!` 直接终止
//!    进程，捕获无从谈起。故本层把"可能失败"的每一步都表达为 `Result`：
//!    详见下方 [`ReaderError`] 各变体的"来源"列——**没有一条依赖 panic 传播**。
//!
//! ## `panic = "abort"` 下的错误路径设计（约束 3 的展开）
//!
//! 在 unwind 可用的构建里，一个常见偷懒是"让远处的一层先 panic，近处
//! `catch_unwind` 兜住"。本层**结构上不给这条路**：
//!
//! - 第三方库（wgpu / cosmic-text）在资源缺席时**返回 `Result` 或 `Option`**，
//!   本层就地分类；它们的 panic 属库缺陷，不被当作流程控制。
//! - 断言式校验（几何参数为负、字号越界）一律走 [`ReaderError::InvalidGeometry`]
//!   等显式错误，不用 `assert!`——`debug_assert!` 只在 debug 生效，
//!   release 下静默通过，会造成"测试绿、发布崩"的分裂。
//! - 因此本 crate 的公共 API **没有**依赖 panic 展开的契约；`abort` 与 `unwind`
//!   两种构建的错误行为一致。
//!
//! ## 与上层文案的衔接
//!
//! 每类错误对应一个稳定标识（[`ReaderError::message_id`]），日志按标识检索。
//! 标识是**契约**，措辞不是。

use std::fmt;

use tome_core::error::ClassifiedError;
use tome_core::ErrorKind;

/// 本层错误的能力标识（见 crate 根模块的 [`crate::capability`]）。
pub use crate::capability;

/// 本层错误的分类。
///
/// ## 变体怎么切
///
/// 切分依据是**上层要做的处置不同**，不是"出错的位置不同"：
///
/// - [`Self::FontUnavailable`] → 提示用户字体资产缺失（可操作）
/// - [`Self::TextModel`] → 输入本身有问题（可为空正文，通常静默降级）
/// - [`Self::LayoutFailed`] → 排版内部失败（重试无意义，记录并降级）
/// - [`Self::InvalidGeometry`] → 调用方传了非法几何（**调用方的 bug**，应修参数）
/// - `Self::Storage`（存储/内容通道） → 下层存储/内容通道失败（提示检查数据目录）
///
/// 把 `InvalidGeometry` 与 `LayoutFailed` 分开尤其重要：前者重试一万次也不会好，
/// 后者可能是瞬时状态。若合成一类，上层只能一律刷新，浪费且误导。
/// 把本层错误交给上层的**唯一**转换：
/// `ReaderError → tome_core::ClassifiedError`（跨 crate 边界的交付形态）。
///
/// ## 为什么手写 `Display` 而不 derive
///
/// `thiserror::Error` 的 derive 会生成一个 `Display`。本类型的 `Display` 要
/// 保证"**标识在前**"（日志按标识检索），而 derive 的 `#[error(...)]` 属性
/// 只能写死格式串、无法表达"detail 为 None 时不带冒号"这类分支。
/// 故 `Display` 由下方手写，本 derive 只用于生成 `Error` trait 实现本身。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderError {
    /// 排版输入/输出模型层面的失败（正文不可排、字符区间越界等）。
    TextModel {
        /// 诊断细节（**开发者向**，非面向用户文案）
        detail: Option<String>,
    },

    /// 排版计算失败（行盒/页序列/滚动流无法产出）。
    LayoutFailed {
        /// 诊断细节（**开发者向**）
        detail: Option<String>,
    },

    /// 调用方传入的几何参数非法（负宽高、字号越界、行高越界等）。
    ///
    /// 这类错误是**调用方的 bug**：合法参数由 `ReadingConfig` 白名单 schema
    /// 保证（FR-013），越界值应在更早处被拒。此处兜底而非静默钳制——
    /// 钳制会把参数的 bug 变成一个看不见的排版结果。
    InvalidGeometry {
        /// 非法参数名（如 `font_size`；用配置字段名，便于调用方定位）
        parameter: &'static str,
        /// 诊断细节（**开发者向**）
        detail: Option<String>,
    },

    /// 字体不可用：内嵌子集缺失且系统回退链也无可用字体（FR-047 / FR-057）。
    FontUnavailable {
        /// 诊断细节（**开发者向**，可含字体族名——那是可检索的技术标识，非文案）
        detail: Option<String>,
    },

    /// 渲染后端不可用（`render` feature 关闭，或图形栈初始化失败）（FR-053/057）。
    ///
    /// **带能力标识**：调用方据此区分"本机构建没有渲染能力"与"渲染过程中出错"，
    /// 前者应改走结构验证（FR-053 的诚实跳过），后者才值得重试或上报。
    UnsupportedCapability {
        /// 不可用能力的标识（见 [`crate::capability`]）
        capability: &'static str,
        /// 诊断细节（**开发者向**）
        detail: Option<String>,
    },

    /// 与下层（内容通道 / 仓库 / 配置）交互失败，透传其分类。
    ///
    /// ## 为什么透传而不是重分类
    ///
    /// 下层已经知道自己是网络还是存储故障，本层没有额外信息能做得更准。
    /// 重新分类只会丢掉原分类的精度（如把 Proxy 并进 Network），
    /// 而上层恰恰按原分类分支。
    Underlying {
        /// 下层错误的分类
        kind: ErrorKind,
        /// 诊断细节（**开发者向**）
        detail: Option<String>,
    },
}

impl ReaderError {
    /// 构造"内容模型失败"。
    #[must_use]
    pub fn text_model(detail: impl Into<String>) -> Self {
        Self::TextModel {
            detail: Some(detail.into()),
        }
    }

    /// 构造"排版失败"。
    #[must_use]
    pub fn layout_failed(detail: impl Into<String>) -> Self {
        Self::LayoutFailed {
            detail: Some(detail.into()),
        }
    }

    /// 构造"几何参数非法"。
    #[must_use]
    pub fn invalid_geometry(parameter: &'static str, detail: impl Into<String>) -> Self {
        Self::InvalidGeometry {
            parameter,
            detail: Some(detail.into()),
        }
    }

    /// 构造"字体不可用"。
    #[must_use]
    pub fn font_unavailable(detail: impl Into<String>) -> Self {
        Self::FontUnavailable {
            detail: Some(detail.into()),
        }
    }

    /// 构造"不支持的能力"（FR-058 的结构化能力错误）。
    ///
    /// 能力标识取 [`crate::capability`] 的常量，避免各处手写字符串漂移。
    #[must_use]
    pub fn unsupported_capability(capability: &'static str) -> Self {
        Self::UnsupportedCapability {
            capability,
            detail: None,
        }
    }

    /// 给"不支持的能力"补诊断细节。
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = Some(detail.into());
        match &mut self {
            Self::TextModel { detail: slot }
            | Self::LayoutFailed { detail: slot }
            | Self::InvalidGeometry { detail: slot, .. }
            | Self::FontUnavailable { detail: slot }
            | Self::UnsupportedCapability { detail: slot, .. }
            | Self::Underlying { detail: slot, .. } => *slot = detail,
        }
        self
    }

    /// 透传下层错误分类。
    #[must_use]
    pub fn underlying(kind: ErrorKind, detail: impl Into<String>) -> Self {
        Self::Underlying {
            kind,
            detail: Some(detail.into()),
        }
    }

    /// 映射为 core 的粗粒度分类（FR-058：沿用 001 的 `ErrorKind`）。
    ///
    /// ## 映射表（每条都是刻意的）
    ///
    /// | 本层错误 | `ErrorKind` | 为什么是这一类 |
    /// |---|---|---|
    /// | `TextModel` | `Content` | 正文内容层面的问题（空正文、越界），不是规则也不是存储 |
    /// | `LayoutFailed` | `Rule` | 排版规则的求值失败——规则集的失败归 `Rule`（与书源规则同义：一套判定逻辑没跑通） |
    /// | `InvalidGeometry` | `Configuration` | 参数非法属配置问题（FR-013 的 schema 在白名单层已定义何为合法） |
    /// | `FontUnavailable` | `UnsupportedCapability{fontUnavailable}` | 字体拿不到 = 一项能力不可用，**必须**带标识（FR-058） |
    /// | `UnsupportedCapability` | `UnsupportedCapability{..}` | 原样透传能力标识 |
    /// | `Underlying` | 原 `kind` | 透传（见该变体文档） |
    ///
    /// 注意 `TextModel → Content` 而非 `Rule`：正文为空是**内容**问题，
    /// 用户侧可操作（换源/检查缓存）；若并入 `Rule`，上层会建议"检查书源规则"，
    /// 指错方向。
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::TextModel { .. } => ErrorKind::Content,
            Self::LayoutFailed { .. } => ErrorKind::Rule,
            Self::InvalidGeometry { .. } => ErrorKind::Configuration,
            Self::FontUnavailable { .. } => ErrorKind::unsupported_capability(
                tome_core::ErrorCapability::new(capability::FONT_UNAVAILABLE),
            ),
            Self::UnsupportedCapability { capability, .. } => {
                ErrorKind::unsupported_capability(tome_core::ErrorCapability::new(*capability))
            }
            Self::Underlying { kind, .. } => kind.clone(),
        }
    }

    /// 本错误的可本地化标识（稳定契约，供上层查文案表）。
    ///
    /// ## 为什么不用 `ErrorKind::message_id()`
    ///
    /// core 的分类只有七类，粒度不足以区分"字体缺失"与"后端缺失"这类
    /// **用户处置不同**的情形。本层标识更细：前缀 `error.reader.` 表明来自
    /// 排版渲染层，后缀语义化。标识是契约，措辞不是（改措辞即破坏性变更）。
    #[must_use]
    pub fn message_id(&self) -> &'static str {
        match self {
            Self::TextModel { .. } => "error.reader.text_model",
            Self::LayoutFailed { .. } => "error.reader.layout_failed",
            Self::InvalidGeometry { .. } => "error.reader.invalid_geometry",
            Self::FontUnavailable { .. } => "error.reader.font_unavailable",
            Self::UnsupportedCapability { .. } => "error.reader.unsupported_capability",
            Self::Underlying { .. } => "error.reader.underlying",
        }
    }

    /// 诊断细节（无则 `None`）。
    ///
    /// 细节是**开发者向**：日志、断言输出。**不是**面向用户的提示语，
    /// 上层不得直接展示（FR-058）。
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::TextModel { detail }
            | Self::LayoutFailed { detail }
            | Self::FontUnavailable { detail }
            | Self::InvalidGeometry { detail, .. }
            | Self::UnsupportedCapability { detail, .. }
            | Self::Underlying { detail, .. } => detail.as_deref(),
        }
    }

    /// 能力标识（仅能力类错误非空）。
    #[must_use]
    pub fn capability_id(&self) -> Option<&str> {
        match self {
            Self::FontUnavailable { .. } => Some(capability::FONT_UNAVAILABLE),
            Self::UnsupportedCapability { capability, .. } => Some(capability),
            _ => None,
        }
    }

    /// 转成 core 的 [`ClassifiedError`](tome_core::error::ClassifiedError "ClassifiedError")（跨 crate 边界的交付形态）。
    #[must_use]
    pub fn to_classified(&self) -> ClassifiedError {
        let detail = self
            .detail()
            .map_or_else(|| self.message_id().to_owned(), str::to_owned);
        self.kind().with_detail(detail)
    }
}

impl fmt::Display for ReaderError {
    /// 输出形如 `error.reader.layout_failed: line 42 overflow`。
    ///
    /// 标识在前、细节在后：日志按标识检索，细节供人阅读（与 core 的
    /// `ClassifiedError` 同构）。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.detail() {
            Some(detail) => write!(formatter, "{}: {detail}", self.message_id()),
            None => formatter.write_str(self.message_id()),
        }
    }
}

impl std::error::Error for ReaderError {}

impl From<ReaderErrorKind> for ReaderError {
    fn from(kind: ReaderErrorKind) -> Self {
        Self::underlying(kind.0, "mapped from lower layer")
    }
}

/// 下层错误的分类透传壳。
///
/// 存在理由：本层接缝（内容通道 / 仓库）返回的是 [`ErrorKind`]，在不丢失
/// 原分类的前提下进入本层错误类型。[`From`] 实现让 `?` 直接可用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderErrorKind(pub ErrorKind);

#[cfg(test)]
mod tests {
    use super::*;

    /// 全变体 → `ErrorKind` 的映射表逐条断言（T003 验收子项）。
    #[test]
    fn every_variant_maps_to_error_kind() {
        let cases: Vec<(ReaderError, ErrorKind)> = vec![
            (ReaderError::text_model("empty chapter"), ErrorKind::Content),
            (ReaderError::layout_failed("line overflow"), ErrorKind::Rule),
            (
                ReaderError::invalid_geometry("font_size", "out of 14..=30"),
                ErrorKind::Configuration,
            ),
            (
                ReaderError::font_unavailable("no fallback in chain"),
                ErrorKind::UnsupportedCapability {
                    capability: tome_core::ErrorCapability::new(capability::FONT_UNAVAILABLE),
                },
            ),
            (
                ReaderError::unsupported_capability(capability::OFFSCREEN_RENDERING),
                ErrorKind::UnsupportedCapability {
                    capability: tome_core::ErrorCapability::new(capability::OFFSCREEN_RENDERING),
                },
            ),
            (
                ReaderError::underlying(ErrorKind::Network, "channel closed"),
                ErrorKind::Network,
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.kind(), expected, "映射不符：{error}");
        }
    }

    /// 能力类错误**必须**携带能力标识（FR-058 的结构化错误要求）。
    #[test]
    fn capability_errors_carry_identifier() {
        let offscreen = ReaderError::unsupported_capability(capability::OFFSCREEN_RENDERING);
        assert_eq!(offscreen.capability_id(), Some("offscreenRendering"));
        assert_eq!(
            offscreen.kind().message_id(),
            "error.unsupported_capability"
        );

        let font = ReaderError::font_unavailable("chain exhausted");
        assert_eq!(font.capability_id(), Some("fontUnavailable"));

        // 非能力类错误不得凭空长出能力标识（否则上层的降级分支会误命中）
        assert_eq!(ReaderError::layout_failed("x").capability_id(), None);
        assert_eq!(ReaderError::text_model("x").capability_id(), None);
        assert_eq!(
            ReaderError::invalid_geometry("font_size", "x").capability_id(),
            None
        );
        assert_eq!(
            ReaderError::underlying(ErrorKind::Storage, "x").capability_id(),
            None
        );
    }

    /// 标识独立于分类：不同能力错误共享同一 `ErrorKind` 但标识可区分。
    #[test]
    fn message_ids_are_distinct_per_variant() {
        let ids = [
            ReaderError::text_model("a").message_id(),
            ReaderError::layout_failed("a").message_id(),
            ReaderError::invalid_geometry("font_size", "a").message_id(),
            ReaderError::font_unavailable("a").message_id(),
            ReaderError::unsupported_capability(capability::OFFSCREEN_RENDERING).message_id(),
            ReaderError::underlying(ErrorKind::Storage, "a").message_id(),
        ];
        let mut unique: Vec<&str> = ids.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "标识必须两两不同：{ids:?}");

        for id in ids {
            assert!(
                id.starts_with("error.reader."),
                "标识须带层前缀，便于日志检索：{id}"
            );
        }
    }

    /// 透传变体不丢失原分类——尤其 Proxy/Network 的区分（001 §FR-008）。
    #[test]
    fn underlying_preserves_classification() {
        for kind in [
            ErrorKind::Network,
            ErrorKind::Proxy,
            ErrorKind::Content,
            ErrorKind::Storage,
            ErrorKind::Configuration,
            ErrorKind::Rule,
        ] {
            let error = ReaderError::underlying(kind.clone(), "lower layer said so");
            assert_eq!(error.kind(), kind, "透传不得重分类");
        }
    }

    /// `with_detail` 对**每一个**变体都生效（防止新增变体时漏改 match 分支）。
    #[test]
    fn with_detail_attaches_for_every_variant() {
        let errors = vec![
            ReaderError::TextModel { detail: None },
            ReaderError::LayoutFailed { detail: None },
            ReaderError::InvalidGeometry {
                parameter: "measure_em",
                detail: None,
            },
            ReaderError::FontUnavailable { detail: None },
            ReaderError::unsupported_capability(capability::OFFSCREEN_RENDERING),
            ReaderError::Underlying {
                kind: ErrorKind::Storage,
                detail: None,
            },
        ];
        for error in errors {
            let with = error.clone().with_detail("probe-detail");
            assert_eq!(
                with.detail(),
                Some("probe-detail"),
                "变体 {error} 的 with_detail 未生效"
            );
        }
    }

    #[test]
    fn to_classified_carries_message_id_and_kind() {
        let classified =
            ReaderError::unsupported_capability(capability::OFFSCREEN_RENDERING).to_classified();
        assert_eq!(classified.message_id(), "error.unsupported_capability");
        assert_eq!(
            classified.detail(),
            Some("error.reader.unsupported_capability")
        );
    }

    #[test]
    fn display_puts_identifier_first() {
        let rendered = ReaderError::layout_failed("line 42 overflow").to_string();
        assert!(
            rendered.starts_with("error.reader.layout_failed:"),
            "标识须在细节之前（日志按标识检索）：{rendered}"
        );
        assert_eq!(
            ReaderError::unsupported_capability(capability::OFFSCREEN_RENDERING).to_string(),
            "error.reader.unsupported_capability"
        );
    }
}
