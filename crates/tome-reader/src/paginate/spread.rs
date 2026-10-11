//! `[DONE]` 单双页判据与手动覆盖（T019/T020 / FR-014、FR-015）
//!
//! ## FR-014 的判据（MASTER §5.2，逐字对应）
//!
//! ```text
//! available    = 阅读区可用宽度（已扣侧栏与阅读区左右内边距）
//! singleNeeded = 36em（em 随阅读字号缩放）
//! 双页启用条件：available >= 2 × singleNeeded + --reading-gutter
//! ```
//!
//! ### 三个容易做错的点
//!
//! 1. **`available` 是阅读区可用宽度，不是窗口宽度**（裁决 OQ#22→A）。
//!    两者在本机差 612px——按窗口宽度理解会在侧栏常驻时过早切双页。
//!    本模块只接受调用方给的这个量，不提供"从窗口宽度推导"的入口：那
//!    会把扣除逻辑藏进来，而"扣多少"是外壳（S3）的布局知识。
//! 2. **em 制，不是 px 断点**。判据里出现的每个长度都随字号缩放，
//!    故字号一变阈值就右移。若把它写成 `if width >= 1432`，24px 字号下
//!    会过早切双页（正确阈值 1792）。
//! 3. **`>=` 而非 `>`**。MASTER 写的是"≥"，边界值必须判双页。
//!    差一个等号的表现是"用户把窗口拖到刚好时看不到双页"，很难归因。
//!
//! ## 版心换算（MASTER §5.1 与 §5.3 的裁决 I7）
//!
//! 单页与双页的版心都受**两道约束**：目标（`measure_em`）与绝对上限
//! （`measure_max_em` = 42em）。超出部分**留白，不拉伸**——裁决 I7 让
//! "超过 42em 则必须分栏"（§5.1）与"手动可强制单页"（§5.2）不再冲突：
//! 单页模式下版心封顶 42em 并居中，不自动强制分栏。

use crate::paginate::config::{GeometryConfig, SpreadMode};

/// 阅读区视口（**可用**宽高，不是窗口宽高）。
///
/// ## 为什么单独一个类型而不是两个 `f32` 参数
///
/// `available_width_px` 的口径是 FR-014 最容易搞错的地方（见模块文档）。
/// 给它一个具名类型，让"这个数是可用宽度"在签名上可见；而两个裸 `f32`
/// 参数在调用点可以随手互换（宽高互换的表现是"所有内容挤成一行")。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// 阅读区**可用**宽度（px）——已扣侧栏与阅读区内边距。
    pub available_width_px: f32,
    /// 阅读区**可用**高度（px）。
    pub available_height_px: f32,
}

impl Viewport {
    /// 构造。
    #[must_use]
    pub fn new(available_width_px: f32, available_height_px: f32) -> Self {
        Self {
            available_width_px,
            available_height_px,
        }
    }
}

/// 单双页的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Columns {
    /// 单页（一栏）。
    Single,
    /// 双页（两栏，中缝隔开）。
    Double,
}

impl Columns {
    /// 栏数。
    #[must_use]
    pub fn count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Double => 2,
        }
    }

    /// 是否双页。
    #[must_use]
    pub fn is_double(self) -> bool {
        matches!(self, Self::Double)
    }
}

/// 双页左右页的归属（FR-016 的书页语义：左偶右奇）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadSide {
    /// 左页（**偶数**页）。
    Left,
    /// 右页（**奇数**页）。
    Right,
}

impl SpreadSide {
    /// 该侧是否对应偶数页。
    #[must_use]
    pub fn is_even_page(self) -> bool {
        matches!(self, Self::Left)
    }

    /// 序列化用的标识（CLI 输出）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// 按**章内页序号**（0 起）决定左右归属。
    ///
    /// ## 这里的"偶/奇"指页序号本身（它与 MASTER §5.2 的表述对齐）
    ///
    /// MASTER §5.2 说"左页为偶数页、右页为奇数页，章节从偶数页（左页）开始"。
    /// 本层的数组下标从 0 起，且 `contracts/cli.md` §2.1 的示例把
    /// `page_index: 0` 与 `side: "left"` 配在一起——故约定为：
    ///
    /// | 页序号（0 起） | 奇偶 | 归属 |
    /// |---|---|---|
    /// | 0, 2, 4, … | **偶** | [`SpreadSide::Left`] |
    /// | 1, 3, 5, … | 奇 | [`SpreadSide::Right`] |
    ///
    /// 于是"章节从偶数页（左页）开始"= 章节的首页序号使它在左页——
    /// 这正是 [`Self::for_chapter_start`] 要保证的事。若这里取反（0 = Right），
    /// 每个章节都会从右页起，与 FR-016 直接冲突。
    #[must_use]
    pub fn for_page_index(page_index: u32) -> Self {
        if page_index.is_multiple_of(2) {
            Self::Left
        } else {
            Self::Right
        }
    }

    /// 章节起始页的归属（FR-016：章起偶数页 = 左页）。
    #[must_use]
    pub fn for_chapter_start() -> Self {
        Self::Left
    }

    /// 相反的一侧。
    #[must_use]
    pub fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    /// 该侧是否为"偶数页"（左）。
    #[must_use]
    pub fn is_left(self) -> bool {
        matches!(self, Self::Left)
    }
}

/// 双页启用所需的最小阅读区宽度（px）。
///
/// `2 × measure_em × font_size + gutter`。**纯几何函数**——不含任何
/// 固定 px 常量，故字号/版心/中缝任一变化都会让它变（FR-014 的
/// "MUST NOT 使用固定像素断点"）。
#[must_use]
pub fn required_width_px(geometry: &GeometryConfig) -> f32 {
    2.0 * geometry.measure_width_px() + f32::from(geometry.gutter_px)
}

/// 自动判据（FR-014）：可用宽度是否容得下两栏 + 中缝。
#[must_use]
pub fn auto_columns(viewport: &Viewport, geometry: &GeometryConfig) -> Columns {
    if viewport.available_width_px >= required_width_px(geometry) {
        Columns::Double
    } else {
        Columns::Single
    }
}

/// 单栏的实际版心宽度（px）。
///
/// 三重约束取最小：
///
/// | 约束 | 来源 | 超出时的行为 |
/// |---|---|---|
/// | `measure_em × font_size` | MASTER §5.1 的**目标**行长 | 不再拉伸（正文行长有可读上限） |
/// | 本例可用宽（双页时扣中缝对半） | 窗口容量的硬下界 | 随窗口收缩 |
/// | `measure_max_em × font_size` | MASTER §5.1 的**绝对上限** 42em | 封顶留白（裁决 I7） |
///
/// 最后一项在**双页**下几乎总被前两项掩盖（两栏各 36em 时总宽已超 42em），
/// 但它在"目标被调大到 60em"这类配置下是唯一的防线，故两道都要在。
///
/// ## 为什么结果保证 ≥ 1px
///
/// spec §Edge Cases："窗口极窄（小于一个字宽 × 2）：必须给出明确的最小尺寸
/// 约束反馈，**不得进入除零或负宽度计算**。" 调用方拿到 1px 的版心后可以
/// 自行决定是否报"窗口过窄"；本层不 panic、不产出 NaN/负数——那种值会
/// 一路传播到字形位置计算里，表现为"字符全部叠在原点"。
#[must_use]
pub fn column_measure_px(columns: Columns, viewport: &Viewport, geometry: &GeometryConfig) -> f32 {
    let usable = match columns {
        Columns::Single => viewport.available_width_px,
        Columns::Double => (viewport.available_width_px - f32::from(geometry.gutter_px)) / 2.0,
    };

    let target = geometry.measure_width_px().min(usable);
    let capped = target.min(geometry.measure_max_width_px());

    capped.max(1.0)
}

// ===========================================================================
// 手动覆盖三态与语义回落（T020 / FR-015，裁决 OQ#14→C）
// ===========================================================================

/// 应用手动覆盖（FR-015 的**三态**），返回实际生效的栏数。
///
/// 只需要栏数时用它；需要知道"手动选择是否仍成立"时用
/// [`resolve_override`]（它多给一个 [`OverrideResolution::override_active`]）。
#[must_use]
pub fn resolve_columns(viewport: &Viewport, geometry: &GeometryConfig) -> Columns {
    resolve_override(viewport, geometry).columns
}

/// 手动覆盖的解析结果：实际生效的栏数 + 手动选择是否仍在生效。
///
/// ## 为什么要把"仍在生效"单独给出来（而不是让调用方自己比较）
///
/// FR-015 的第二个半句要求"宽度仅发生不改变该矛盾的变化时 MUST NOT 清除
/// 用户的手动选择"。故调用方（会话状态）在每次变化后都要问一句：
/// **"我的手动选择还成立吗？"**
///
/// 若它靠比较"`resolve_columns` 的结果 == 我选的那个"来回答，两种情形会
/// 判错：
///
/// | 情形 | `resolve_columns` 结果 | "== 我选的" | 正确判断 |
/// |---|---|---|---|
/// | 宽窗 + ForceSpread | Double | 相等 → 以为成立 | 成立 ✓（恰巧对） |
/// | 窄窗 + ForceSpread | Single（回落） | 不等 → 以为失效 | 失效 ✓ |
/// | 宽窗 + **Auto** | Double | 相等 → 以为成立 | **不成立** ✗ |
///
/// 第三行是反例：`Auto` 模式下根本没有手动选择，但"结果恰好等于双页"会让
/// 它看起来像有一个。后果是界面可能显示"已强制双页"——用户从没选过。
///
/// [`Self::override_active`] 直接回答那个问题，语义不含糊。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverrideResolution {
    /// 实际生效的栏数。
    pub columns: Columns,
    /// 手动选择是否**仍然成立**（`Auto` 恒为 `false`）。
    pub override_active: bool,
}

/// 解析手动覆盖与自动判据的冲突（FR-015 的完整语义）。
///
/// ## 三态的处置
///
/// | `spread_mode` | 结果 | `override_active` |
/// |---|---|---|
/// | [`SpreadMode::Auto`] | 判据结果 | `false` |
/// | [`SpreadMode::ForceSingle`] | 恒 [`Columns::Single`] | `true`（恒成立） |
/// | [`SpreadMode::ForceSpread`] | 容得下则双页，否则判据结果 | 容得下 `true`，否则 `false` |
///
/// ## 为什么 ForceSingle 永不回落
///
/// "回落"的定义是"手动选择与当前可用宽度**相矛盾**"。单页在任何宽度下
/// 都成立——它不要求任何宽度下限。故它与宽度不存在矛盾。
///
/// 若把 ForceSingle 也做成"窄窗时回落"，那它回落的结果恰好也是单页（判据
/// 在窄窗下必然给单页），行为上无从区分；但这个"回落"会**清掉用户的选择**
/// （见 [`override_decision_after_resize`]），于是用户把窗口拉宽后会发现自己
/// 又回到双页了——而他从没改过设置。
///
/// ## 为什么 ForceSpread 回落的是判据结果而不是硬性单页
///
/// 矛盾的定义就是"判据说单页"，故两者在本函数里结果必然相同。差别在
/// **语义**：回落是说"这次不生效"，而非"用户的选择改成了单页"。调用方据
/// `override_active == false` 清除会话内的选择，而不是去写一个"单页"的设置。
#[must_use]
pub fn resolve_override(viewport: &Viewport, geometry: &GeometryConfig) -> OverrideResolution {
    let automatic = auto_columns(viewport, geometry);

    match geometry.spread_mode {
        SpreadMode::Auto => OverrideResolution {
            columns: automatic,
            override_active: false,
        },
        SpreadMode::ForceSingle => OverrideResolution {
            columns: Columns::Single,
            override_active: true,
        },
        SpreadMode::ForceSpread => {
            if automatic == Columns::Double {
                OverrideResolution {
                    columns: Columns::Double,
                    override_active: true,
                }
            } else {
                OverrideResolution {
                    columns: automatic,
                    override_active: false,
                }
            }
        }
    }
}

/// 宽度/几何变化后手动选择是否应当被保留（FR-015 第二半句）。
///
/// ## 判据是"矛盾是否改变"，不是"宽度是否改变"
///
/// MASTER §5.2 的原条款是「窗口尺寸变化超过一定幅度时自动模式重新接管」，
/// 该"幅度阈值"已被判为**凭空数字且会误伤微调**（裁决 OQ#14→C）。正确语义
/// 只看一件事：**当前几何下手动选择还成立吗？**
///
/// - 仍成立 → [`OverrideDecision::Keep`]（**无论宽度变了多少**）
/// - 已不成立 → [`OverrideDecision::Clear`]（此时才移除选择，回到自动）
///
/// 故本函数不需要"变化幅度"这个输入——它的缺席是**刻意的**，而不是漏了参数。
/// 加回一个 `previous_width` 参数会让实现者有机会去比较宽度，而那正是被
/// 判为错误的做法。
///
/// ## 无状态（幂等）
///
/// 判据只依赖**当前**几何，不依赖任何历史。故同一宽度反复问答案恒相同——
/// 调用方不必担心"反复清除"导致的界面抖动。
#[must_use]
pub fn override_decision_after_resize(
    viewport: &Viewport,
    geometry: &GeometryConfig,
) -> OverrideDecision {
    if resolve_override(viewport, geometry).override_active {
        OverrideDecision::Keep
    } else {
        OverrideDecision::Clear
    }
}

/// 变化后对手动选择的处置（FR-015）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideDecision {
    /// 手动选择仍成立——保留（宽度是否变化过无关）。
    Keep,
    /// 手动选择已与当前几何矛盾——清除（回到自动）。
    Clear,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paginate::config::ReadingConfig;

    fn geometry() -> GeometryConfig {
        ReadingConfig::default_ma().geometry
    }

    #[test]
    fn threshold_matches_master_5_2_table() {
        // MASTER §5.2 的换算表：19px 下 1432px
        let threshold = required_width_px(&geometry());
        assert!((threshold - 1432.0).abs() < 1e-3, "实测 {threshold}");
    }

    /// 阈值随字号缩放（em 制的核心）。
    #[test]
    fn threshold_scales_with_font_size_quadratically_in_measure_terms() {
        let mut config = geometry();
        config.font_size_px = 14;
        let at_14 = required_width_px(&config);
        config.font_size_px = 30;
        let at_30 = required_width_px(&config);

        assert!((at_14 - (2.0 * 14.0 * 36.0 + 64.0)).abs() < 1e-3);
        assert!((at_30 - (2.0 * 30.0 * 36.0 + 64.0)).abs() < 1e-3);
        assert!(at_30 > at_14);
    }

    /// 边界值判双页（`>=` 而非 `>`）。
    #[test]
    fn exact_threshold_is_double() {
        let g = geometry();
        let threshold = required_width_px(&g);
        assert_eq!(
            auto_columns(&Viewport::new(threshold, 800.0), &g),
            Columns::Double
        );
    }

    /// 版心上限在两条路径上都生效。
    #[test]
    fn measure_cap_applies_on_both_paths() {
        let mut g = geometry();
        g.measure_em = 60.0; // 超过 42em 上限
        let viewport = Viewport::new(4000.0, 800.0);

        let single = column_measure_px(Columns::Single, &viewport, &g);
        assert!(
            (single - 42.0 * 19.0).abs() < 1e-3,
            "单页版心应封顶 42em = 798px，实测 {single}"
        );

        let double = column_measure_px(Columns::Double, &viewport, &g);
        assert!(double <= 42.0 * 19.0 + 1e-3, "双页版心也受上限约束");
    }

    /// 页序号 → 左右归属（0 起：偶左奇右，与 MASTER §5.2 的表述对齐）。
    #[test]
    fn side_alternates_by_page_index() {
        assert_eq!(SpreadSide::for_page_index(0), SpreadSide::Left);
        assert_eq!(SpreadSide::for_page_index(1), SpreadSide::Right);
        assert_eq!(SpreadSide::for_page_index(2), SpreadSide::Left);
        assert_eq!(SpreadSide::for_page_index(3), SpreadSide::Right);
    }

    /// 章节起始页在**左页**（FR-016：章起偶数页）。
    #[test]
    fn chapter_start_is_left_side() {
        assert_eq!(SpreadSide::for_chapter_start(), SpreadSide::Left);
        assert!(SpreadSide::for_chapter_start().is_even_page());
        // 与"0 起序号 0"一致——章节从第 0 页（左页）开始
        assert_eq!(
            SpreadSide::for_chapter_start(),
            SpreadSide::for_page_index(0)
        );
    }

    /// 相反侧与左/右判据自洽。
    #[test]
    fn opposite_and_is_left_agree() {
        assert_eq!(SpreadSide::Left.opposite(), SpreadSide::Right);
        assert_eq!(SpreadSide::Right.opposite(), SpreadSide::Left);
        assert!(SpreadSide::Left.is_left());
        assert!(!SpreadSide::Right.is_left());
        // 相反两次回到自身（对两个变体都成立——只测一个会漏掉实现里写死的分支）
        assert_eq!(
            SpreadSide::Left,
            SpreadSide::Left.opposite().opposite(),
            "左页相反两次应回到左页"
        );
        assert_eq!(
            SpreadSide::Right,
            SpreadSide::Right.opposite().opposite(),
            "右页相反两次应回到右页"
        );
    }

    // --- T020：手动覆盖三态与语义回落 ---

    /// Auto 模式下没有手动选择可保留（`override_active` 恒 false）。
    ///
    /// ## 这条防的是一个很隐蔽的误判
    ///
    /// 若调用方用"`resolve_columns` 的结果 == 我选的那个"来判断选择是否
    /// 成立，那么宽窗下的 Auto 模式会被判成"有个双页选择"——因为判据恰好
    /// 返回 Double。后果是界面显示"已强制双页"，而用户从没选过。
    #[test]
    fn auto_mode_reports_no_active_override() {
        let g = geometry();
        // 宽到判据必然给双页
        let wide = Viewport::new(5000.0, 800.0);
        let resolution = resolve_override(&wide, &g);
        assert_eq!(resolution.columns, Columns::Double, "判据应给双页");
        assert!(
            !resolution.override_active,
            "Auto 模式下 override_active 必须为 false（不得因判据恰好给双页而误报）"
        );
        assert_eq!(
            override_decision_after_resize(&wide, &g),
            OverrideDecision::Clear,
            "Auto 模式无选择可 Keep"
        );
    }

    /// ForceSpread 在矛盾出现时回落，且报告"选择已不成立"。
    #[test]
    fn force_spread_falls_back_when_too_narrow() {
        let mut g = geometry();
        g.spread_mode = SpreadMode::ForceSpread;

        let wide = resolve_override(&Viewport::new(2000.0, 800.0), &g);
        assert_eq!(wide.columns, Columns::Double);
        assert!(wide.override_active);

        let narrow = resolve_override(&Viewport::new(800.0, 800.0), &g);
        assert_eq!(narrow.columns, Columns::Single);
        assert!(
            !narrow.override_active,
            "窄窗下 ForceSpread 与宽度矛盾，必须报告选择不成立"
        );
    }

    /// ForceSingle 在任何宽度下都成立（单页无宽度下限）。
    #[test]
    fn force_single_never_falls_back() {
        let mut g = geometry();
        g.spread_mode = SpreadMode::ForceSingle;

        for width in [0.0f32, 400.0, 1432.0, 5000.0] {
            let resolution = resolve_override(&Viewport::new(width, 800.0), &g);
            assert_eq!(
                resolution.columns,
                Columns::Single,
                "宽度 {width} 下应强制单页"
            );
            assert!(
                resolution.override_active,
                "宽度 {width} 下 ForceSingle 不得被判为失效（它无宽度下限）"
            );
        }
    }

    /// **FR-015 第二半句**：宽度变但矛盾未变 → **不清除**选择。
    #[test]
    fn keep_decision_when_contradiction_unchanged() {
        let mut g = geometry();
        g.spread_mode = SpreadMode::ForceSpread;

        // 三档都容得下两栏（宽度变了，矛盾没变）
        for width in [1500.0f32, 2400.0, 3000.0] {
            assert_eq!(
                override_decision_after_resize(&Viewport::new(width, 800.0), &g),
                OverrideDecision::Keep,
                "宽度 {width} 下仍容得下两栏——不得清除用户的双页选择"
            );
        }
    }

    /// 矛盾出现时清除选择。
    #[test]
    fn clear_decision_when_contradiction_appears() {
        let mut g = geometry();
        g.spread_mode = SpreadMode::ForceSpread;

        for width in [300.0f32, 800.0, 1431.0] {
            assert_eq!(
                override_decision_after_resize(&Viewport::new(width, 800.0), &g),
                OverrideDecision::Clear,
                "宽度 {width} 下容不下两栏——应清除选择（回落自动）"
            );
        }
    }

    /// 判定是**无状态的纯函数**：同一输入反复问，答案恒相同。
    ///
    /// "反复清除"在调用方可能表现为状态抖动（界面反复提示"已回落"）。
    #[test]
    fn override_decision_is_stateless_and_idempotent() {
        let mut g = geometry();
        g.spread_mode = SpreadMode::ForceSpread;
        let viewport = Viewport::new(900.0, 800.0);

        let first = override_decision_after_resize(&viewport, &g);
        for _ in 0..5 {
            assert_eq!(override_decision_after_resize(&viewport, &g), first);
        }
        assert_eq!(first, OverrideDecision::Clear);
    }
}
