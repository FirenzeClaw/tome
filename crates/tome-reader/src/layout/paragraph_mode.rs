//! `[DONE]` 段落排布模式（前导 2 / FR-018）
//!
//! ## 这个类型为什么在 layout 而不在 paginate
//!
//! `ParagraphMode` 的两条效果都落在**行盒**上（首行左偏移与段末行高），
//! 而产出行盒的是排版引擎（[`crate::layout::engine`]）。若类型定义留在
//! `paginate::config`，`layout` 就要反过来依赖 `paginate`——与模块文档声明的
//! `layout → paginate` 方向相反，且让"排版层可独立于分页层使用"这条
//! 性质失效（引擎单测就得拉起整个配置栈）。
//!
//! 故类型下沉到本模块，`paginate::config` 经 `pub use` 再导出——对既有调用点
//! （`use tome_reader::paginate::config::ParagraphMode`）完全透明。
//!
//! ## 互斥为什么靠一个枚举而不是两个 `Option<f32>`
//!
//! FR-018 明文要求"缩进模式无额外段距，间距模式无首行缩进"。两个可选量
//! 可以表达 `(Some, Some)` 这种**非法组合**——那时谁生效要靠约定。
//! 一个枚举让两种模式在类型上互斥，非法组合**无法构造**。
//!
//! ## 施加方式（两条效果各自的落点）
//!
//! | 模式 | 效果 | 施加点 | 为什么在那里 |
//! |---|---|---|---|
//! | 缩进 | 段首行左移 `amount × 字号` | **shape 层的 `x_advance`** + layout 后校正 `x_px` | 缩进占掉首行可用宽，**断行必须知道**；layout 之后再加就只能让行变宽 |
//! | 间距 | 段末行高 `+amount × 字号` | 行盒的 `height_px` | 页/屏装填逐行累加行高，记进行高即自动进入分页与滚动流 |
//!
//! 缩进这一条与 T014 的中西文间隙、T013 的标点挤压共用同一个注入点
//! （research R1 方案 A 第 3 步："shape 后、layout 前改写 `x_advance`"）。

use serde::{Deserialize, Serialize};

/// 默认段首缩进（em）。MASTER §5.1 `--reading-paragraph-indent: 2em`。
pub const DEFAULT_PARAGRAPH_INDENT_EM: f32 = 2.0;

/// 默认段间距（em）。MASTER §5.1 `--reading-paragraph-gap: 0.4em`。
pub const DEFAULT_PARAGRAPH_GAP_EM: f32 = 0.4;

/// 段落的排布模式（FR-018：两者**互斥**）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ParagraphMode {
    /// 段首缩进（默认 2em），段间无额外间距。
    Indent {
        /// 缩进量（em）
        em: f32,
    },
    /// 段间间距（默认 0.4em），首行不缩进。
    Gap {
        /// 间距量（em）
        em: f32,
    },
}

impl Default for ParagraphMode {
    /// FR-018 / 裁决 OQ#12→A：**默认段首缩进 2em**。
    fn default() -> Self {
        Self::Indent {
            em: DEFAULT_PARAGRAPH_INDENT_EM,
        }
    }
}

impl ParagraphMode {
    /// 该模式的量（em）。
    #[must_use]
    pub fn amount_em(&self) -> f32 {
        match *self {
            Self::Indent { em } | Self::Gap { em } => em,
        }
    }

    /// 是否为首行缩进模式。
    #[must_use]
    pub fn is_indent(self) -> bool {
        matches!(self, Self::Indent { .. })
    }

    /// 段首行的左缩进量（px）。间距模式恒为 0。
    #[must_use]
    pub fn first_line_indent_px(self, font_size_px: f32) -> f32 {
        match self {
            Self::Indent { em } => (em * font_size_px).max(0.0),
            Self::Gap { .. } => 0.0,
        }
    }

    /// 段末行的额外竖向间距（px）。缩进模式恒为 0。
    ///
    /// ## 记在**段末行**而非段首行
    ///
    /// 两种记账（记段前 / 记段后）在视觉上等价，但记段后有一条实现优势：
    /// 页与滚动视口的装填逻辑都**逐行累加 `height_px`**，间距进入行高即
    /// 自动被两者同时遵守，无需在分页与滚动各写一遍。记段前则需要"知道
    /// 自己是不是某段的首行"——那是分页层的信息，排版层没有。
    #[must_use]
    pub fn paragraph_gap_px(self, font_size_px: f32) -> f32 {
        match self {
            Self::Gap { em } => (em * font_size_px).max(0.0),
            Self::Indent { .. } => 0.0,
        }
    }
}

/// 校验段模式与段量（负值/NaN 被拒）。
pub fn validate_paragraph_mode(value: &ParagraphMode) -> Result<(), &'static str> {
    let em = value.amount_em();
    if em.is_finite() && em >= 0.0 {
        Ok(())
    } else {
        Err("paragraph_mode")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_indent_two_em() {
        assert_eq!(
            ParagraphMode::default(),
            ParagraphMode::Indent {
                em: DEFAULT_PARAGRAPH_INDENT_EM
            }
        );
        assert!(ParagraphMode::default().is_indent());
    }

    /// 两模式的**效果量**互斥：缩进模式无段距、间距模式无首行缩进。
    #[test]
    fn effects_are_mutually_exclusive() {
        let indent = ParagraphMode::Indent { em: 2.0 };
        let gap = ParagraphMode::Gap { em: 0.4 };

        assert!((indent.first_line_indent_px(19.0) - 38.0).abs() < 1e-4);
        assert_eq!(indent.paragraph_gap_px(19.0), 0.0);

        assert_eq!(gap.first_line_indent_px(19.0), 0.0);
        assert!((gap.paragraph_gap_px(19.0) - 7.6).abs() < 1e-4);
    }

    #[test]
    fn negative_amounts_are_rejected() {
        assert!(validate_paragraph_mode(&ParagraphMode::Indent { em: -1.0 }).is_err());
        assert!(validate_paragraph_mode(&ParagraphMode::Gap { em: f32::NAN }).is_err());
        assert!(validate_paragraph_mode(&ParagraphMode::Indent { em: 0.0 }).is_ok());
    }

    /// 负量在**施加**路径上不得产出负偏移（钳到 0）。
    #[test]
    fn negative_amounts_are_clamped_on_apply() {
        assert_eq!(
            ParagraphMode::Indent { em: -2.0 }.first_line_indent_px(19.0),
            0.0
        );
        assert_eq!(ParagraphMode::Gap { em: -1.0 }.paragraph_gap_px(19.0), 0.0);
    }
}
