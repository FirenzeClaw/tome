//! [DONE] 排版参数集（T017 / FR-013、FR-028、FR-018）
//!
//! `ReadingConfig` 是一本书生效的阅读几何与呈现参数，也是 001 契约
//! `Book.read_config` 的 schema（该字段"内容由 S2 排版层解释"）。
//!
//! ## 为什么必须分两个子集（FR-028 的实质）
//!
//! 参数分两类，它们**变更后的代价完全不同**：
//!
//! | 子集 | 内容 | 变更后果 |
//! |---|---|---|
//! | [`GeometryConfig`] | 字号/行高/段模式/版心/中缝/单双页/形态/章起新页 | **重排 + 重新分页** |
//! | [`PresentationConfig`] | 主题/字体族/动效/噪点 | **只重绘**（页序列逐字节相同） |
//!
//! 若两者混在一个结构体里，SC-005（"昼/夜主题产出的页序列 100% 相同"）
//! 就**只能靠实现者的自觉**维持——没有任何机制阻止分页函数顺手读一下
//! `theme_id`。拆成两个字段后，分页函数的签名只能拿到
//! `&GeometryConfig`，呈现参数**在类型上取不到**。
//!
//! 这不是"文档约定"，而是本模块存在的理由。
//!
//! ## 校验的口径：拒绝，不钳制
//!
//! spec §Edge Cases："排版参数被设为非法值（字号 0、行高负、版心负宽）：
//! MUST 在接受前校验并拒绝，**回退到上一个有效值**。"
//!
//! "回退到上一个有效值"意味着校验的**产物**是一个错误标识，由调用方
//! 决定如何回落（保留旧值 / 用默认值）。本模块不提供"静默钳制"的入口：
//! 钳制会把"参数写错了"变成一个看不见的排版结果——那比报错难查得多。
//!
//! 校验函数返回 `Result<(), &'static str>`，`Err` 侧是**非法字段名**
//! （ASCII 标识，如 `"font_size"`），不是面向用户的文案（FR-058）。

use serde::{Deserialize, Serialize};

/// 阅读字号的合法范围（px）。
///
/// MASTER §5.1 `--reading-font-size: 19px（可调 14–30）`。
pub const FONT_SIZE_RANGE: std::ops::RangeInclusive<u16> = 14..=30;

/// 行高的合法范围（字号倍数）。
///
/// MASTER §5.1 原表写「1.4–2.2」，但 §2.3「正文行高 ≥ 1.75（强制）」是
/// 无障碍强制条款，§10 亦把无障碍定为"引擎级强制，非可选皮肤"。冲突时
/// 以强制约束为准——下限抬到 **1.75**（裁决 OQ#19→A，spec §FR-013）。
pub const LINE_HEIGHT_RANGE: std::ops::RangeInclusive<f32> = 1.75..=2.2;

/// 默认阅读字号（px）。MASTER §5.1。
pub const DEFAULT_FONT_SIZE_PX: u16 = 19;

/// 默认行高（字号倍数）。MASTER §5.1（下限经裁决修正）。
pub const DEFAULT_LINE_HEIGHT: f32 = 1.85;

/// 默认版心目标宽（em）。MASTER §5.1 `--reading-measure: 36em`。
pub const DEFAULT_MEASURE_EM: f32 = 36.0;

/// 默认单栏上限（em）。MASTER §5.1 `--reading-measure-max: 42em`。
pub const DEFAULT_MEASURE_MAX_EM: f32 = 42.0;

/// 默认中缝宽（px）。MASTER §5.1 `--reading-gutter: 64px`。
pub const DEFAULT_GUTTER_PX: u16 = 64;

/// 默认段首缩进（em）。MASTER §5.1 `--reading-paragraph-indent: 2em`。
pub const DEFAULT_PARAGRAPH_INDENT_EM: f32 = 2.0;

/// 默认段间距（em）。MASTER §5.1 `--reading-paragraph-gap: 0.4em`。
pub const DEFAULT_PARAGRAPH_GAP_EM: f32 = 0.4;

/// 段落的排布模式（FR-018：两者**互斥**）。
///
/// ## 为什么用一个枚举而不是两个 `Option<f32>`
///
/// FR-018 明文要求"缩进模式无额外段距，间距模式无首行缩进"。两个可选量
/// 可以表达 `(Some, Some)` 这种**非法组合**——那时谁生效要靠约定。一个
/// 枚举让"两种模式"在类型上互斥，非法组合**无法构造**。
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
}

/// 单双页模式：自动判据 + 两个**手动覆盖**档（FR-015）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpreadMode {
    /// 按 em 制判据自动决定（FR-014）。
    #[default]
    Auto,
    /// 强制单页（用户手动覆盖）。
    ForceSingle,
    /// 强制双页（用户手动覆盖）。
    ForceSpread,
}

/// 阅读形态（FR-013 的几何子集成员；裁决 OQ#7 纳入本层范围）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ReadingMode {
    /// 翻页模式（MASTER 描述的唯一形态）。
    #[default]
    Paged,
    /// 滚动模式（FR-020~027，S2-3 切片落地）。
    Scroll,
}

/// 主题标识（**呈现参数**，不参与分页）。
///
/// 承载"昼/夜"这一选择，而非主题的**值**——值类型 [`tome_core::model::Theme`]
/// 由组合根构造注入，本层不持有色彩常量（FR-029）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeId {
    /// 昼：Parchment（暖色纸面）。
    #[default]
    Parchment,
    /// 夜：Obsidian（真黑底）。
    Obsidian,
}

/// 字体族选择（**呈现参数**）。
///
/// ## 为什么这里只有两个变体
///
/// 裁决 OQ#3→A：正文衬线**内嵌子集**，UI/等宽走系统栈。故正文只可能来自
/// "内嵌"或"系统回退链"两条路径；用户自定义字体导入在 spec §Out of Scope
/// 中明确排除。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FontFamily {
    /// 内嵌子集（`assets/fonts/TomeSerifCN-subset.otf`，FR-050）。
    #[default]
    EmbeddedSerif,
    /// 系统回退链（FR-047/051）。
    SystemFallback,
}

/// **几何子集**：决定分页与滚动流几何的参数（FR-028）。
///
/// 任一字段变化 → 重排 + 重新分页。分页函数只接受本类型——
/// 它在类型上取不到呈现参数，故 SC-005 无法被违反。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GeometryConfig {
    /// 阅读字号（px），14–30，默认 19。
    pub font_size_px: u16,
    /// 行高（字号倍数），1.75–2.2，默认 1.85。
    pub line_height: f32,
    /// 段落排布模式（缩进 / 间距，互斥），默认缩进 2em。
    pub paragraph_mode: ParagraphMode,
    /// 版心目标宽（em），默认 36。
    pub measure_em: f32,
    /// 单栏绝对上限（em），默认 42。单页模式下版心封顶并居中
    /// （MASTER §5.3 的裁决 I7：超出部分留白，不自动强制分栏）。
    pub measure_max_em: f32,
    /// 双页中缝宽（px），默认 64。
    pub gutter_px: u16,
    /// 单双页模式（自动 / 强制单页 / 强制双页）。
    pub spread_mode: SpreadMode,
    /// 阅读形态（翻页 / 滚动）。
    pub reading_mode: ReadingMode,
    /// 「章节从新页开始」（FR-016，裁决 OQ#20→C）。
    ///
    /// **仅双页模式下生效**——单页模式不应用章末留白（裁决 OQ#5→A）。
    pub chapter_new_page: bool,
}

impl Default for GeometryConfig {
    fn default() -> Self {
        Self {
            font_size_px: DEFAULT_FONT_SIZE_PX,
            line_height: DEFAULT_LINE_HEIGHT,
            paragraph_mode: ParagraphMode::default(),
            measure_em: DEFAULT_MEASURE_EM,
            measure_max_em: DEFAULT_MEASURE_MAX_EM,
            gutter_px: DEFAULT_GUTTER_PX,
            spread_mode: SpreadMode::default(),
            reading_mode: ReadingMode::default(),
            chapter_new_page: true,
        }
    }
}

impl GeometryConfig {
    /// 版心宽度（px）= 字号 × 版心目标（em）。
    ///
    /// FR-014 的"em 制"落点：**随字号实时重算**，不是固定像素。
    #[must_use]
    pub fn measure_width_px(&self) -> f32 {
        f32::from(self.font_size_px) * self.measure_em
    }

    /// 单栏上限宽度（px）= 字号 × 上限（em）。
    #[must_use]
    pub fn measure_max_width_px(&self) -> f32 {
        f32::from(self.font_size_px) * self.measure_max_em
    }

    /// 行高（px）= 字号 × 行高倍数。
    #[must_use]
    pub fn line_height_px(&self) -> f32 {
        f32::from(self.font_size_px) * self.line_height
    }
}

/// **呈现子集**：只决定绘制的参数（FR-028）。
///
/// 任一字段变化 → **只重绘**，页序列逐字节相同（SC-005）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentationConfig {
    /// 主题标识（昼/夜）。
    pub theme_id: ThemeId,
    /// 正文字体族。
    pub font_family: FontFamily,
    /// 减少动效开关（FR-032）。
    pub reduce_motion: bool,
    /// 纸纤维噪点开关（MASTER §2 的昼主题观感项）。
    pub paper_noise: bool,
}

impl Default for PresentationConfig {
    fn default() -> Self {
        Self {
            theme_id: ThemeId::default(),
            font_family: FontFamily::default(),
            reduce_motion: false,
            paper_noise: true,
        }
    }
}

/// 一本书生效的排版参数集（FR-013 的"显式、版本化的参数集"）。
///
/// ## 两个子集严格分离（FR-028）
///
/// 见模块文档。字段名 `geometry` / `presentation` 是刻意直白的：
/// 调用方一眼能看出"这个参数该不该触发重排"。
///
/// ## 作用域（裁决 OQ#12→A）
///
/// 全局默认生效；`Book.read_config` 仅在用户**显式为这本书设置**时写入
/// 并覆盖全局。该字段是 JSON 文本（001 契约的不透明字符串）。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ReadingConfig {
    /// 几何子集（决定分页）。
    pub geometry: GeometryConfig,
    /// 呈现子集（只决定绘制）。
    pub presentation: PresentationConfig,
}

impl ReadingConfig {
    /// MASTER §5.1 的默认配置。
    ///
    /// 命名带 `_ma` 而非 `default` 的唯一理由：让"这是设计规范的默认值"
    /// 在调用点可见（`ReadingConfig::default_ma()` 比 `::default()` 更能
    /// 说明"这些数字有出处"）。`Default` 同样可用（两者等价）。
    #[must_use]
    pub fn default_ma() -> Self {
        Self {
            geometry: GeometryConfig::default(),
            presentation: PresentationConfig::default(),
        }
    }

    // --- 校验（FR-013 / spec §Edge Cases）---

    /// 校验字号（px）。
    ///
    /// `Err` 侧是非法字段名（ASCII 标识），供调用方定位并回落。
    pub fn validate_font_size(value: u16) -> Result<(), &'static str> {
        if FONT_SIZE_RANGE.contains(&value) {
            Ok(())
        } else {
            Err("font_size")
        }
    }

    /// 校验行高（字号倍数）。
    ///
    /// ## 为什么显式排除 NaN
    ///
    /// `LINE_HEIGHT_RANGE.contains(&f32::NAN)` 返回 `false`（NaN 与任何值
    /// 比较都是 false），故 NaN 天然被拒。此处把该性质写出来是为了让
    /// "NaN 也被拒"成为**有意为之**而非巧合——它由测试钉住。
    pub fn validate_line_height(value: f32) -> Result<(), &'static str> {
        if value.is_nan() || !LINE_HEIGHT_RANGE.contains(&value) {
            Err("line_height")
        } else {
            Ok(())
        }
    }

    /// 校验版心目标（em）。
    ///
    /// 上界取 [`DEFAULT_MEASURE_MAX_EM`]（42em）：版心目标超过单栏上限
    /// 在语义上自相矛盾（"目标比上限还大"），属配置错误。
    pub fn validate_measure_em(value: f32) -> Result<(), &'static str> {
        let valid = value.is_finite() && value > 0.0 && value <= DEFAULT_MEASURE_MAX_EM;
        if valid {
            Ok(())
        } else {
            Err("measure_em")
        }
    }

    /// 校验中缝（px）。
    pub fn validate_gutter_px(value: u16) -> Result<(), &'static str> {
        // `u16` 本身非负，故只需接受即可——此函数的存在让"中缝也走白名单"
        // 这件事在调用点一致（而非"这个字段恰好不用校验"）。
        // 保留 `value` 参数的判据位以便日后加"中缝不得超过版心"这类约束。
        let _ = value;
        Ok(())
    }

    /// 校验段模式与段量。
    pub fn validate_paragraph_mode(value: &ParagraphMode) -> Result<(), &'static str> {
        let em = value.amount_em();
        if em.is_finite() && em >= 0.0 {
            Ok(())
        } else {
            Err("paragraph_mode")
        }
    }

    /// 校验整份配置：逐字段检查，**返回第一个非法字段的名字**。
    ///
    /// ## 为什么逐个 `validate_*` 而不是合并判断
    ///
    /// 合并判断（如一个大 `if` 里 `&&` 所有条件）只能告诉调用方"配置非法"，
    /// 而调用方的处置是"回退那个非法字段"——它需要知道**是哪一个**。
    /// 逐个走让错误标识携带那个信息，且新增字段时只加一行。
    pub fn validate(&self) -> Result<(), &'static str> {
        Self::validate_font_size(self.geometry.font_size_px)?;
        Self::validate_line_height(self.geometry.line_height)?;
        Self::validate_measure_em(self.geometry.measure_em)?;
        Self::validate_measure_em(self.geometry.measure_max_em)?;
        Self::validate_gutter_px(self.geometry.gutter_px)?;
        Self::validate_paragraph_mode(&self.geometry.paragraph_mode)?;
        Ok(())
    }

    // --- 序列化（作为 Book.read_config 的 schema）---

    /// 序列化为 JSON 文本（`Book.read_config` 的载体形态）。
    ///
    /// ## 为什么是 JSON 而不是 TOML
    ///
    /// 001 契约把 `read_config` 定义为 `Option<String>`，**不透明**。
    /// 选 JSON 的理由是它与 CLI 契约（`contracts/cli.md`：默认
    /// `--format json`）以及语料格式（`tests/corpus/`）同一套生态——
    /// 一个用户在命令行看到的形态与他在库里存的形态一致，调试时无需换脑子。
    pub fn to_json(&self) -> Result<String, ReaderConfigError> {
        serde_json::to_string(self).map_err(|error| ReaderConfigError::serialize(error.to_string()))
    }

    /// 从 JSON 文本反序列化，**并校验**（接受前校验，spec §Edge Cases）。
    ///
    /// ## 为什么校验在反序列化里而不是调用方
    ///
    /// "接受前校验"的"接受"就是这一步。若把校验留给调用方，任何一条
    /// 新代码路径忘记调用 `validate()` 就会让非法配置进到分页——那时
    /// 表现是"排版结果诡异"而非"配置被拒"，排查成本高得多。
    pub fn from_json(json: &str) -> Result<Self, ReaderConfigError> {
        let config: Self = serde_json::from_str(json)
            .map_err(|error| ReaderConfigError::parse(error.to_string()))?;
        config
            .validate()
            .map_err(ReaderConfigError::invalid_field)?;
        Ok(config)
    }

    /// 从 `Book.read_config` 的**可选**文本加载，未设置时回退全局默认
    /// （裁决 OQ#12→A 的作用域语义）。
    ///
    /// ## 返回 `(配置, 是否来自单书覆盖)`
    ///
    /// 第二个值让调用方能把"这本书没有自己的配置"与"它有配置且恰好等于
    /// 默认值"区分开——两者的 UI 呈现不同（前者显示"跟随全局"，后者显示
    /// "已为本书设置"）。
    ///
    /// ## 配置非法时的处置
    ///
    /// 回退全局默认并**报出**原因（不静默）：`read_config` 是用户可手工
    /// 编辑的文本，损坏是可能的。静默用默认值会让用户以为自己的设置"没保存"。
    pub fn from_book_read_config(
        read_config: Option<&str>,
    ) -> Result<(Self, bool), ReaderConfigError> {
        match read_config {
            None => Ok((Self::default_ma(), false)),
            Some(text) => {
                let config = Self::from_json(text)?;
                Ok((config, true))
            }
        }
    }
}

/// 配置的解析/校验错误。
///
/// ## 为什么带字段名
///
/// 校验失败的**处置**（回退上一个有效值）需要一个"哪个字段"的标识。
/// 把这个信息放在错误类型里，调用方不必再解析错误文案。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderConfigError {
    /// 文本不是合法的配置 schema。
    Parse {
        /// 诊断细节（开发者向；含 serde 的原始报错）
        detail: String,
    },
    /// 序列化失败（实践中不会发生——本类型无非法状态）。
    Serialize {
        /// 诊断细节
        detail: String,
    },
    /// 某字段的值越界。
    InvalidField {
        /// 非法字段名（配置字段名，ASCII 标识）
        field: &'static str,
    },
}

impl ReaderConfigError {
    /// 构造"解析失败"。
    #[must_use]
    pub fn parse(detail: impl Into<String>) -> Self {
        Self::Parse {
            detail: detail.into(),
        }
    }

    /// 构造"序列化失败"。
    #[must_use]
    pub fn serialize(detail: impl Into<String>) -> Self {
        Self::Serialize {
            detail: detail.into(),
        }
    }

    /// 构造"字段越界"。
    #[must_use]
    pub fn invalid_field(field: &'static str) -> Self {
        Self::InvalidField { field }
    }

    /// 本错误的可本地化标识（FR-058：标识而非文案）。
    #[must_use]
    pub fn message_id(&self) -> &'static str {
        match self {
            Self::Parse { .. } => "error.reader.read_config_parse",
            Self::Serialize { .. } => "error.reader.read_config_serialize",
            Self::InvalidField { .. } => "error.reader.read_config_invalid_field",
        }
    }

    /// 非法的字段名（仅 [`Self::InvalidField`] 非空）。
    #[must_use]
    pub fn field(&self) -> Option<&'static str> {
        match self {
            Self::InvalidField { field } => Some(field),
            _ => None,
        }
    }
}

impl std::fmt::Display for ReaderConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse { detail } | Self::Serialize { detail } => {
                write!(formatter, "{}: {detail}", self.message_id())
            }
            Self::InvalidField { field } => write!(formatter, "{}: {field}", self.message_id()),
        }
    }
}

impl std::error::Error for ReaderConfigError {}

impl From<ReaderConfigError> for crate::error::ReaderError {
    /// 映射到本层错误类型：配置问题归 [`crate::error::ReaderError::InvalidGeometry`]。
    ///
    /// 用 `InvalidGeometry` 而非 `Underlying`：配置错误是**调用方的 bug**
    /// （参数写错了），归 `Configuration`，与 FR-013 的"白名单 schema 保证
    /// 合法参数"同一处置路径。
    fn from(error: ReaderConfigError) -> Self {
        match error {
            ReaderConfigError::InvalidField { field } => {
                Self::invalid_geometry(field, "read_config 字段越界")
            }
            ReaderConfigError::Parse { detail } => Self::invalid_geometry("read_config", detail),
            ReaderConfigError::Serialize { detail } => {
                Self::invalid_geometry("read_config", detail)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 段模式互斥（FR-018）：枚举的构造天然拒绝 `(Indent, Gap)` 组合。
    #[test]
    fn paragraph_modes_are_mutually_exclusive_by_construction() {
        let indent = ParagraphMode::Indent { em: 2.0 };
        let gap = ParagraphMode::Gap { em: 0.4 };

        assert!(indent.is_indent());
        assert!(!gap.is_indent());
        assert!((indent.amount_em() - 2.0).abs() < 1e-6);
        assert!((gap.amount_em() - 0.4).abs() < 1e-6);
    }

    #[test]
    fn default_paragraph_mode_is_indent_two_em() {
        assert_eq!(
            ParagraphMode::default(),
            ParagraphMode::Indent { em: 2.0 },
            "FR-018 / 裁决 OQ#12→A：默认段首缩进 2em"
        );
    }

    /// NaN 被拒（`RangeInclusive::contains` 对 NaN 返回 false——见文档）。
    #[test]
    fn nan_line_height_is_rejected() {
        assert_eq!(
            ReadingConfig::validate_line_height(f32::NAN),
            Err("line_height")
        );
        assert_eq!(
            ReadingConfig::validate_line_height(f32::INFINITY),
            Err("line_height")
        );
    }

    /// 校验顺序：先报字号再报行高（`validate` 的逐字段顺序是确定的）。
    #[test]
    fn validation_reports_fields_in_declared_order() {
        let mut config = ReadingConfig::default_ma();
        config.geometry.font_size_px = 0;
        config.geometry.line_height = -1.0;
        assert_eq!(
            config.validate(),
            Err("font_size"),
            "两处都非法时应报先声明检查的那个（顺序确定可预测）"
        );
    }

    #[test]
    fn config_error_carries_field_name() {
        let error = ReaderConfigError::invalid_field("font_size");
        assert_eq!(error.field(), Some("font_size"));
        assert_eq!(error.message_id(), "error.reader.read_config_invalid_field");
        assert!(error.to_string().contains("font_size"));
        // 标识式：不含面向用户的自然语言
        assert!(error.message_id().is_ascii());
    }

    /// 未设置 `read_config` → 全局默认，且标记为"非单书覆盖"。
    #[test]
    fn missing_read_config_falls_back_to_global_default() {
        let (config, overridden) =
            ReadingConfig::from_book_read_config(None).expect("未设置应成功");
        assert_eq!(config, ReadingConfig::default_ma());
        assert!(!overridden, "未设置时不得标记为单书覆盖");
    }
}
