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

    /// 逐字段校验，**返回第一个非法字段的名字**（ASCII 标识）。
    ///
    /// ## 为什么校验落在几何子集上（而不是 `ReadingConfig`）
    ///
    /// 分页函数的签名只接受 `&GeometryConfig`（FR-028 的子集分离），故它
    /// 只能调用这里的校验。若校验留在 `ReadingConfig` 上，分页就只剩两个
    /// 选择：要么不校验（非法几何静默进入计算），要么要求调用方传整份配置
    /// （破坏子集分离）。放在几何上两个问题一起消掉。
    ///
    /// ## 为什么逐个查而不是合并判断
    ///
    /// 合并判断（一个大 `if` 里 `&&` 所有条件）只能告诉调用方"配置非法"，
    /// 而调用方的处置是"回退那个非法字段"——它需要知道**是哪一个**。
    pub fn validate(&self) -> Result<(), &'static str> {
        validate_font_size(self.font_size_px)?;
        validate_line_height(self.line_height)?;
        validate_measure_em(self.measure_em)?;
        validate_measure_em(self.measure_max_em)?;
        validate_gutter_px(self.gutter_px)?;
        validate_paragraph_mode(&self.paragraph_mode)?;
        Ok(())
    }

    /// 从 JSON 反序列化并校验（供单独存几何的场合）。
    pub fn from_json(json: &str) -> Result<Self, ReaderConfigError> {
        let geometry: Self = serde_json::from_str(json)
            .map_err(|error| ReaderConfigError::parse(error.to_string()))?;
        geometry
            .validate()
            .map_err(ReaderConfigError::invalid_field)?;
        Ok(geometry)
    }

    /// 序列化为 JSON 文本。
    pub fn to_json(&self) -> Result<String, ReaderConfigError> {
        serde_json::to_string(self).map_err(|error| ReaderConfigError::serialize(error.to_string()))
    }
}

// --- 字段级校验 ------------------------------------------------------------
//
// 自由函数而非 `GeometryConfig` 的方法：调用方常需要**单独验一个值**
// （CLI 解析出一个字号后立刻判断合法性），那时手里还没有整份配置。

/// 校验字号（px）。`Err` 侧是字段名（ASCII 标识）。
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
/// `LINE_HEIGHT_RANGE.contains(&f32::NAN)` 返回 `false`（NaN 与任何值比较
/// 都是 false），故 NaN 天然被拒。此处把该性质写出来，是为了让"NaN 也被拒"
/// 成为**有意为之**而非巧合——它由测试钉住。
pub fn validate_line_height(value: f32) -> Result<(), &'static str> {
    if value.is_nan() || !LINE_HEIGHT_RANGE.contains(&value) {
        Err("line_height")
    } else {
        Ok(())
    }
}

/// 校验版心（em）。
///
/// 上界取 [`DEFAULT_MEASURE_MAX_EM`]（42em）：版心目标超过单栏上限在语义上
/// 自相矛盾（"目标比上限还大"），属配置错误。
pub fn validate_measure_em(value: f32) -> Result<(), &'static str> {
    let valid = value.is_finite() && value > 0.0 && value <= DEFAULT_MEASURE_MAX_EM;
    if valid {
        Ok(())
    } else {
        Err("measure_em")
    }
}

/// 校验中缝（px）。
///
/// `u16` 本身非负，故当前恒通过。本函数的存在让"中缝也走白名单"在调用点
/// 一致（而非"这个字段恰好不用校验"），并给日后"中缝不得超过版心"这类
/// 约束留了落点。
pub fn validate_gutter_px(value: u16) -> Result<(), &'static str> {
    let _ = value;
    Ok(())
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

    /// 校验整份配置（委托给几何子集的校验）。
    ///
    /// 呈现子集**不参与校验**——它的字段都是枚举与 `bool`，非法状态在类型上
    /// 不可构造。这个不对称是"子集分离"的附带好处：呈现参数不可能让配置非法。
    pub fn validate(&self) -> Result<(), &'static str> {
        self.geometry.validate()
    }

    /// 校验字号（px）——转发到字段级校验（[`validate_font_size`]）。
    pub fn validate_font_size(value: u16) -> Result<(), &'static str> {
        validate_font_size(value)
    }

    /// 校验行高——转发到字段级校验（[`validate_line_height`]）。
    pub fn validate_line_height(value: f32) -> Result<(), &'static str> {
        validate_line_height(value)
    }

    /// 校验版心——转发到字段级校验（[`validate_measure_em`]）。
    pub fn validate_measure_em(value: f32) -> Result<(), &'static str> {
        validate_measure_em(value)
    }

    /// 校验中缝——转发到字段级校验（[`validate_gutter_px`]）。
    pub fn validate_gutter_px(value: u16) -> Result<(), &'static str> {
        validate_gutter_px(value)
    }

    /// 校验段模式——转发到字段级校验（[`validate_paragraph_mode`]）。
    pub fn validate_paragraph_mode(value: &ParagraphMode) -> Result<(), &'static str> {
        validate_paragraph_mode(value)
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

    /// 解析一本书**实际生效**的配置：全局默认 + 单书覆盖（裁决 OQ#12→A）。
    ///
    /// ## 作用域语义（FR-013 的括号条款）
    ///
    /// | `read_config` | 生效配置 | `is_book_override` |
    /// |---|---|---|
    /// | `None` | `global`（参数传入的那份） | `false` |
    /// | `Some(json)` | 该书自己的配置 | `true` |
    ///
    /// `global` 由调用方作为参数传入而非本模块持有一个全局单例：全局默认
    /// 的来源（用户设置文件）属组合根关切，本层不该持有可变全局状态——
    /// 那会让测试之间互相污染，且让"同一进程里两个不同的全局默认"无法表达。
    ///
    /// ## 覆盖是**整份替换**，不是逐字段合并
    ///
    /// 逐字段合并需要一个"该字段未被设置"的表示。JSON 里 `"font_size_px":19`
    /// 与"没写这个字段"可以区分，但"用户把字号**调回** 19"与"用户没设置字号"
    /// 就无法区分了——于是用户显式调回默认值的操作会被理解成"跟随全局"，
    /// 全局一改这本书就跟着变。整份替换没有这个歧义：有配置就用它，全用它。
    ///
    /// ## 配置损坏时的处置
    ///
    /// **报错，不静默回退**。`read_config` 是用户可手工编辑的文本（001 契约
    /// 定义为不透明字符串），损坏是可能的。静默用默认值会让用户以为自己的
    /// 设置"没保存"——那是比报错更差的体验。调用方拿到错误后可以决定提示
    /// 方式（本层只给可本地化标识，FR-058）。
    pub fn resolve_for_book(
        global: &Self,
        read_config: Option<&str>,
    ) -> Result<ResolvedConfig, ReaderConfigError> {
        match read_config {
            None => Ok(ResolvedConfig {
                config: *global,
                is_book_override: false,
            }),
            Some(text) => Ok(ResolvedConfig {
                config: Self::from_json(text)?,
                is_book_override: true,
            }),
        }
    }
}

/// 一次配置解析的结果：生效配置 + 它的来源。
///
/// ## 为什么不用 `(ReadingConfig, bool)`
///
/// 元组让调用点写成 `resolve_for_book(g, r).0` / `.1`——两个位置的语义
/// 无从自证，而 `bool` 尤其容易被读反（`true` 是"有覆盖"还是"跟随全局"？）。
/// 命名字段把这个问题消掉，且日后加"覆盖来自哪个版本"之类的信息时
/// 不必改所有调用点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedConfig {
    /// 实际生效的配置（覆盖或全局默认）。
    pub config: ReadingConfig,
    /// 该配置是否来自 `Book.read_config` 的单书覆盖。
    ///
    /// 供 UI 区分"跟随全局"与"已为本书设置"——两者的呈现不同，
    /// 即使用户设置的恰好是默认值。
    pub is_book_override: bool,
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

    /// 未设置 `read_config` → 传入的全局默认，且标记为"非单书覆盖"。
    #[test]
    fn missing_read_config_falls_back_to_global_default() {
        let global = ReadingConfig::default_ma();
        let resolved = ReadingConfig::resolve_for_book(&global, None).expect("未设置应成功");
        assert_eq!(resolved.config, global);
        assert!(!resolved.is_book_override, "未设置时不得标记为单书覆盖");
    }
}
