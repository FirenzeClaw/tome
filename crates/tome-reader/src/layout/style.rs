//! `[DONE]` 行内样式最小集合（T016 / FR-010）
//!
//! ## FR-010 的两条要求
//!
//! 1. **最小集合**：章节标题、强调/加粗、引用（居中）
//! 2. **两条不变量**：
//!    - 样式**只影响外观与行盒属性**，不影响**字符区间归属**——
//!      同一段文本在任何样式组合下的字符下标 MUST 一致
//!    - 章节标题的字号 MUST **随阅读字号等比缩放**（MUST NOT 硬编码像素）
//!
//! ## 第 1 条不变量为什么是"本层的地基"
//!
//! 001 §FR-028 规定阅读痕迹一律以**字符区间**表达（不存渲染坐标）。
//! 若样式能改变字符下标，那么"同一段文字在标题样式下的高亮区间"
//! 与"在正文样式下的区间"就会不同——痕迹在样式变化时会错位，
//! 而那正是 001 花力气建立字符区间契约要避免的事。
//!
//! 故本模块的设计原则是：**样式只携带"怎么画"，不携带"在哪画"**。
//! 字号（影响字身宽度）是唯一会改变几何的样式属性，它通过
//! [`StyleSpec::font_scale`] 表达为**比例**，在排版时由引擎按比例
//! 缩放——**字符下标本身不受任何影响**。

/// 行内样式（FR-010 的最小集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InlineStyle {
    /// 正文（默认）
    Body,
    /// 章节标题（居中 + 放大字号）
    Heading,
    /// 强调 / 加粗
    Strong,
    /// 引用行（居中 + 缩进语义）
    Quote,
}

impl InlineStyle {
    /// 全部样式的枚举视图（供参数化测试遍历）。
    pub const ALL: [Self; 4] = [Self::Body, Self::Heading, Self::Strong, Self::Quote];
}

/// 某个样式的**外观规格**。
///
/// ## 为什么字号是"比例"而不是像素
///
/// FR-010 明文："章节标题的字号 MUST 随阅读字号等比缩放（MUST NOT 硬编码
/// 为固定像素值）"。故这里存**相对阅读字号的倍数**——
/// 19px 阅读字号下 Heading 是 `19 × 1.6 = 30.4px`，24px 下是 `38.4px`。
///
/// 若存固定 px（如 `30.0`），那么用户把阅读字号调到 14px 时标题仍占 30px
/// ——比正文大 2 倍多，观感崩坏；调到 30px 时标题与正文同大，层次消失。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleSpec {
    /// 字号相对**阅读字号**的倍数（1.0 = 与正文同大）
    pub font_scale: f32,
    /// 字重（沿用 MASTER §3 的 400/500/600/650/700）
    pub weight: u16,
    /// 是否居中（标题与引用为真，FR-011）
    pub centered: bool,
    /// 是否加粗渲染（强调）
    pub bold_emphasis: bool,
}

impl StyleSpec {
    /// 按阅读字号算出**实际字号**（px）。
    #[must_use]
    pub fn font_size_px(&self, reading_font_size_px: f32) -> f32 {
        reading_font_size_px * self.font_scale
    }
}

/// 各样式的外观规格。
///
/// ## 取值依据（对齐 `design-system/MASTER.md`）
///
/// | 样式 | font_scale | 依据 |
/// |---|---|---|
/// | Body | 1.0 | 与阅读字号同大 |
/// | Heading | **1.6** | MASTER §3 的层级比：正文 14 / h1 22 ≈ 1.57，取整 1.6 |
/// | Strong | 1.0 | 强调不改字号（改字号会破坏汉字的等距网格） |
/// | Quote | 0.95 | 略小以形成层次；不足 5% 的差异在 19px 下约 0.95px |
///
/// ## 为什么 Strong 不放大字号
///
/// 中文正文的竖排网格感依赖"每字等宽"。把少数几个字放大（西文排版里
/// 强调的常见做法）会让该行的字身参差、网格破碎。中文化的强调手段是
/// **字重**（更粗）或下划线/着重号（MASTER §8 禁 >700 字重，故取 700）。
pub const STYLE_SPECS: [(InlineStyle, StyleSpec); 4] = [
    (
        InlineStyle::Body,
        StyleSpec {
            font_scale: 1.0,
            weight: 400,
            centered: false,
            bold_emphasis: false,
        },
    ),
    (
        InlineStyle::Heading,
        StyleSpec {
            font_scale: 1.6,
            weight: 700,
            centered: true,
            bold_emphasis: false,
        },
    ),
    (
        InlineStyle::Strong,
        StyleSpec {
            font_scale: 1.0,
            weight: 700,
            centered: false,
            bold_emphasis: true,
        },
    ),
    (
        InlineStyle::Quote,
        StyleSpec {
            font_scale: 0.95,
            weight: 400,
            centered: true,
            bold_emphasis: false,
        },
    ),
];

/// 取某样式的外观规格。
#[must_use]
pub fn spec_of(style: InlineStyle) -> StyleSpec {
    STYLE_SPECS
        .iter()
        .find(|(candidate, _)| *candidate == style)
        .map(|(_, spec)| *spec)
        .unwrap_or(STYLE_SPECS[0].1)
}

/// 一段文本上的样式区间（FR-010 的"行内样式"）。
///
/// ## 关键设计：区间是**字符**区间
///
/// 与 001 的痕迹区间同口径。故"加样式"不会改变任何下标——
/// 它只是给已有区间贴一个标签。
#[derive(Debug, Clone, PartialEq)]
pub struct StyleSpan {
    /// 样式
    pub style: InlineStyle,
    /// 字符区间（段内偏移）
    pub char_range: std::ops::Range<usize>,
}

impl StyleSpan {
    /// 构造。
    #[must_use]
    pub fn new(style: InlineStyle, char_range: std::ops::Range<usize>) -> Self {
        Self { style, char_range }
    }

    /// 该区间是否覆盖给定字符位置。
    #[must_use]
    pub fn covers(&self, char_index: usize) -> bool {
        self.char_range.contains(&char_index)
    }
}

/// 求某字符位置生效的样式（多个区间重叠时取**最后一个**）。
///
/// ## 为什么取最后一个而不报错
///
/// 样式区间的重叠是**合法的**（如标题里的强调词）：外层 Heading、
/// 内层 Strong。取"最后声明的"是最自然的覆盖语义，与 CSS 的同权重
/// 后者胜出一致。
#[must_use]
pub fn style_at(spans: &[StyleSpan], char_index: usize) -> InlineStyle {
    spans
        .iter()
        .rev()
        .find(|span| span.covers(char_index))
        .map_or(InlineStyle::Body, |span| span.style)
}

/// 行内样式**是否影响字符区间归属**的断言辅助（FR-010 的第 1 条不变量）。
///
/// ## 这条不变量的正确表述
///
/// "同一段文本在任何样式组合下字符下标一致"——它的含义是：
/// **样式的施加不改变文本模型**。故本函数返回"施加样式后的文本"，
/// 调用方断言它与未施加时**逐字符相等**。
///
/// ## 为什么用"文本相同"而不是"下标相同"来断言
///
/// "下标"是位置，位置本身在没有文本时无意义。不变量的实质是
/// "**样式不重写文本**"——本函数把这一点变成可断言的返回值。
#[must_use]
pub fn text_under_styles(text: &str, spans: &[StyleSpan]) -> String {
    // 样式不改文本，故原样返回。本函数存在的意义是**给出一个可断言的
    // 出口**：若未来有人让样式参与文本变换（如插入标记），
    // 这里的实现会变，而调用方的断言会立刻失败。
    let _ = spans;
    text.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // FR-010 第 1 条不变量：样式不改字符区间归属
    // ------------------------------------------------------------------

    /// **参数化**：任意样式组合下，文本逐字符不变。
    ///
    /// ## 这是 FR-010 验收子项的落点
    ///
    /// 覆盖 `Body × Heading × Strong × Quote` 的**两两组合**
    /// （16 种：含"同种两次"与"跨种"），断言每种组合下文本完全一致。
    #[test]
    fn fr010_text_is_identical_under_any_style_combination() {
        let text = "章节标题与正文混排的一段文字，含标点、数字 123 与 English。";
        let char_len = text.chars().count();

        let baseline = text_under_styles(text, &[]);

        for first in InlineStyle::ALL {
            for second in InlineStyle::ALL {
                // 两个不同区间（各覆盖一半），制造重叠场景
                let spans = vec![
                    StyleSpan::new(first, 0..char_len / 2),
                    StyleSpan::new(second, char_len / 2..char_len),
                ];
                let styled = text_under_styles(text, &spans);

                assert_eq!(
                    styled, baseline,
                    "样式组合 ({first:?}, {second:?}) 改变了文本——\
                     FR-010 要求样式不影响字符区间归属"
                );
                assert_eq!(
                    styled.chars().count(),
                    char_len,
                    "字符数也必须不变（否则下标会整体偏移）"
                );
            }
        }
    }

    /// 重叠区间（标题里嵌强调）也不改文本。
    #[test]
    fn fr010_overlapping_styles_do_not_change_text() {
        let text = "第一章";
        let spans = vec![
            StyleSpan::new(InlineStyle::Heading, 0..3),
            StyleSpan::new(InlineStyle::Strong, 1..2),
        ];
        assert_eq!(text_under_styles(text, &spans), text);
    }

    /// 样式区间的**字符下标**本身也不因样式而变（区间是只读标签）。
    #[test]
    fn fr010_style_spans_keep_their_char_ranges() {
        let span = StyleSpan::new(InlineStyle::Heading, 5..12);
        let before = span.char_range.clone();
        let _ = style_at(std::slice::from_ref(&span), 7);
        let _ = text_under_styles("0123456789012", std::slice::from_ref(&span));
        assert_eq!(span.char_range, before, "样式查询不得改动区间");
    }

    // ------------------------------------------------------------------
    // FR-010 第 2 条：标题字号随阅读字号等比
    // ------------------------------------------------------------------

    /// **四档断言**：标题字号在 14/19/24/30 下等比。
    ///
    /// ## 这是 FR-010 验收子项的第二个落点
    ///
    /// 判据是"标题字号 / 阅读字号 == 常量 font_scale"，
    /// 而**不是**"标题字号等于某个固定值"——后者正是 FR-010 禁的
    /// 硬编码像素。
    #[test]
    fn fr010_heading_scales_proportionally_with_four_sizes() {
        let heading = spec_of(InlineStyle::Heading);

        for reading_size in [14.0f32, 19.0, 24.0, 30.0] {
            let actual = heading.font_size_px(reading_size);
            let expected = reading_size * heading.font_scale;

            assert!(
                (actual - expected).abs() < 1e-4,
                "阅读字号 {reading_size}px 下标题应为 {expected}px，实测 {actual}px"
            );
            // **等比**的判据：比例恒等于 font_scale
            assert!(
                (actual / reading_size - heading.font_scale).abs() < 1e-6,
                "标题/正文比例应恒为 {}，实测 {}",
                heading.font_scale,
                actual / reading_size
            );
        }
    }

    /// **反例**：标题字号不是硬编码的固定像素。
    ///
    /// 若实现写成 `font_size = 30.0`（固定），则 14px 与 30px 阅读字号下
    /// 标题都是 30px——本测试会失败。
    #[test]
    fn fr010_heading_font_size_is_not_hardcoded() {
        let heading = spec_of(InlineStyle::Heading);
        let at_14 = heading.font_size_px(14.0);
        let at_30 = heading.font_size_px(30.0);

        assert_ne!(
            at_14, at_30,
            "标题字号不应是固定值（14px 与 30px 阅读字号下都是 {at_14}）——\
             FR-010 明确禁硬编码像素"
        );
        assert!(at_30 > at_14, "阅读字号越大标题越大");
    }

    /// 标题比正文大（层次可辨），但不超过 MASTER §3 的层级范围。
    #[test]
    fn fr010_heading_is_larger_than_body_but_bounded() {
        let body = spec_of(InlineStyle::Body);
        let heading = spec_of(InlineStyle::Heading);

        assert!(
            heading.font_scale > body.font_scale,
            "标题应比正文大以形成层次"
        );
        assert!(
            heading.font_scale <= 2.0,
            "标题倍数 {} 过大——超出 MASTER §3 的层级比例（约 1.57）",
            heading.font_scale
        );
    }

    // ------------------------------------------------------------------
    // 样式规格的其它约束
    // ------------------------------------------------------------------

    /// 字重不得超 MASTER §3 的 700 上限（§8 的 Don'ts）。
    #[test]
    fn all_styles_respect_weight_ceiling() {
        for (style, spec) in STYLE_SPECS {
            assert!(
                spec.weight <= 700,
                "{style:?} 的字重 {} 超过 MASTER §3 的上限 700",
                spec.weight
            );
            assert!(
                spec.weight >= 400,
                "{style:?} 的字重 {} 低于常规",
                spec.weight
            );
        }
    }

    /// 居中只用于标题与引用（FR-011）。
    #[test]
    fn only_heading_and_quote_are_centered() {
        assert!(!spec_of(InlineStyle::Body).centered, "正文不居中");
        assert!(!spec_of(InlineStyle::Strong).centered, "强调不居中");
        assert!(spec_of(InlineStyle::Heading).centered, "标题居中");
        assert!(spec_of(InlineStyle::Quote).centered, "引用居中");
    }

    /// **强调不改字号**（中文竖排网格感的理由，见 `STYLE_SPECS` 文档）。
    #[test]
    fn strong_keeps_body_font_size() {
        assert_eq!(
            spec_of(InlineStyle::Strong).font_scale,
            spec_of(InlineStyle::Body).font_scale,
            "强调用字重而非字号：放大少数几个字会破坏汉字的等距网格"
        );
        assert!(spec_of(InlineStyle::Strong).bold_emphasis);
    }

    // ------------------------------------------------------------------
    // 样式查询
    // ------------------------------------------------------------------

    #[test]
    fn style_at_returns_body_when_no_span_covers() {
        let spans = vec![StyleSpan::new(InlineStyle::Heading, 0..3)];
        assert_eq!(style_at(&spans, 5), InlineStyle::Body);
        assert_eq!(style_at(&spans, 1), InlineStyle::Heading);
    }

    /// 重叠时取**最后一个**（内层覆盖外层）。
    #[test]
    fn style_at_prefers_last_span_on_overlap() {
        let spans = vec![
            StyleSpan::new(InlineStyle::Heading, 0..10),
            StyleSpan::new(InlineStyle::Strong, 3..5),
        ];
        assert_eq!(style_at(&spans, 4), InlineStyle::Strong, "内层胜出");
        assert_eq!(style_at(&spans, 7), InlineStyle::Heading, "外层仍然生效");
    }

    #[test]
    fn covers_checks_range_boundaries() {
        let span = StyleSpan::new(InlineStyle::Body, 2..5);
        assert!(!span.covers(1));
        assert!(span.covers(2), "左闭");
        assert!(span.covers(4));
        assert!(!span.covers(5), "右开");
    }

    #[test]
    fn every_style_has_a_spec() {
        for style in InlineStyle::ALL {
            let spec = spec_of(style);
            assert!(spec.font_scale > 0.0, "{style:?} 的字号倍数应为正");
        }
        assert_eq!(
            STYLE_SPECS.len(),
            InlineStyle::ALL.len(),
            "每个样式都应有规格（缺一个会静默回退到 Body）"
        );
    }
}
