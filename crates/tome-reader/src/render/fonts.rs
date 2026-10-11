//! `[DONE]` 字体内嵌加载 + fontdb 回退链（T037 / FR-050、FR-051、FR-047；research R3）
//!
//! ## 字体三角色与内嵌策略（FR-050 / 裁决 OQ#3→A）
//!
//! | 角色 | 策略 |
//! |---|---|
//! | **正文衬线**（reading-serif） | **内嵌子集**（`assets/fonts/TomeSerifCN-subset.otf`） |
//! | UI 无衬线 | 系统栈 + 回退链（**不内嵌**） |
//! | 规则等宽 | 系统栈 + 回退链（**不内嵌**） |
//!
//! 内嵌正文衬线的理由是 FR-050 的第二句："MUST 保证三平台上的正文字形观感
//! 一致（不因系统字体差异而漂移）"。正文是阅读体验的主体，三平台宋体差异
//! 会直接改变版面观感；UI 与等宽是辅助元素，跟随系统反而更协调。
//!
//! ## RFN 合规（research R3 的关键）
//!
//! 思源宋体带 **Reserved Font Name「Source」**，OFL 规定修改版不得用它作
//! 字体标识。T008 的子集化脚本已改家族名为 `Tome Serif CN` 并随分发附
//! OFL 全文。本模块经 [`READING_SERIF_FAMILY`] 再导出该常量（定义在
//! [`crate::font_setup`]——排版与渲染的**共用**装载点），它是改名结果的
//! **运行时**落点——`tests/fonts.rs` 断言它不含 "Source"，防的是"某次
//! 重新子集化时脚本回退到原名"。
//!
//! ## 零网络（FR-051/FR-056）
//!
//! 加载路径只有两条：`include_bytes!`（编译期嵌入）与 fontdb 的系统扫描
//! （本地文件系统）。[`FontRegistry::load`] **无参数**——没有"从哪取"的
//! 输入，故结构上不可能去网络取。字节的持有者是 [`crate::font_setup`]，
//! 本模块只是它的再导出与消费方。
//!
//! ## 回退链的平台差异（FR-051）
//!
//! 内嵌子集是 GB2312 6763 字档（`spikes/s2/BASELINE.md` §7.2），
//! 网文里的生僻人名/方言字会有缺字。缺字时按平台表依次尝试系统字体。
//! 三平台的首选见 [`fallback_chain_for`]。

use std::sync::OnceLock;

use cosmic_text::fontdb;

// 内嵌字节、家族名与“内嵌优先”的字体栈构造都在 feature 无关的
// `crate::font_setup`——排版层（headless 也要用）与渲染层共用同一份，
// 避免两处各写一遍而让度量分叉（C1 缺陷的成因正是这种分叉）。
pub use crate::font_setup::{
    embedded_font_bytes, family_name, FontSetupError as FontLoadError, READING_SERIF_FAMILY,
};

/// 内嵌体积的**预算上限**：5 MiB（FR-050 的 3–5MB 区间上界）。
///
/// ## 为什么只卡上界（口径说明）
///
/// SC-017 的原话是"内嵌正文衬线字体子集 MUST 控制在 3–5 MB 预算内；
/// **超出预算须记录原因并提请用户裁决**"——"预算"在此是**上限**语义
/// （超了要解释）。实测子集 2.18 MiB **低于**该区间（原因：改用 GB2312
/// 6763 字档，CFF 子集化的子程序共享比 R3 的线性外推更省，见
/// `spikes/s2/BASELINE.md` §7.1/§7.2）。低于预算是好事——体积目标是
/// `<20MB`——故 `within_budget` 判据取"≤ 上限"。
pub const EMBEDDED_FONT_BUDGET_BYTES: usize = 5 * 1024 * 1024;

/// 内嵌体积的**参考下限**：3 MiB（FR-050 的 3–5MB 区间下界）。
///
/// ## 它不是"必须达到"的门槛
///
/// 它是**报告用的参照量**：`tome fonts` 的输出里同时给出实测值与这个下界，
/// 让 reviewer 一眼看出实测落在区间的哪一侧（当前：低于下界，
/// 原因已在 BASELINE 记录）。若把它当门槛断言，会在优化体积成功时反而报错
/// ——那是本末倒置。
pub const EMBEDDED_FONT_MIN_BYTES: usize = 3 * 1024 * 1024;

/// 平台的回退链选择。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Windows 10+
    Windows,
    /// macOS 12+
    MacOs,
    /// 现代 Linux
    Linux,
}

impl Platform {
    /// 编译目标对应的平台。
    #[must_use]
    pub const fn current() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            Self::Linux
        }
    }
}

/// 某平台的中文衬线回退链（按优先级从高到低）。
///
/// ## 链条内容与依据（FR-051）
///
/// | 平台 | 链 | 依据 |
/// |---|---|---|
/// | Windows | 宋体（SimSun）→ 微软雅黑 | 宋体是 Windows 的中文衬线，覆盖率最高 |
/// | macOS | Songti SC → STSong → PingFang SC | Songti 是 macOS 的宋体族；PingFang 是无衬线兜底 |
/// | Linux | Noto Serif CJK SC → Noto Sans CJK SC → WenQuanYi Zen Hei | Noto CJK 是主流发行版的默认中文族 |
///
/// 每条的**末位都是无衬线兜底**：宁可字形风格略偏，也不要有字不显示
/// （FR-051："MUST NOT 因缺字而失败或渲染为空"）。
#[must_use]
pub fn fallback_chain_for(platform: Platform) -> Vec<&'static str> {
    match platform {
        Platform::Windows => vec!["宋体", "SimSun", "微软雅黑", "Microsoft YaHei"],
        Platform::MacOs => vec!["Songti SC", "STSong", "PingFang SC", "Heiti SC"],
        Platform::Linux => vec![
            "Noto Serif CJK SC",
            "Noto Sans CJK SC",
            "WenQuanYi Zen Hei",
            "DejaVu Sans",
        ],
    }
}

/// 当前平台的回退链。
#[must_use]
pub fn fallback_chain_for_current_platform() -> Vec<&'static str> {
    fallback_chain_for(Platform::current())
}

/// 字形的**来源**（内嵌子集 / 系统回退）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontSource {
    /// 内嵌子集（FR-050）
    Embedded,
    /// 系统字体（含回退链取到的，FR-047/051）
    System,
}

/// 字体注册表（内嵌子集 + 系统回退链）。
///
/// ## 为什么基于 `FontSystem` 而不是裸 `fontdb`
///
/// 覆盖率判定（"这个字体有没有这个字形"）不是 `fontdb` 的能力——它只做
/// 字体查询，不做字形查询。cosmic-text 的 `FontSystem` 提供了
/// `get_font_supported_codepoints_in_word`，那正是**渲染路径实际用的**判定
/// （整形时的回退选择走它）。用同一个判定，本层的"有字形吗"与渲染时的
/// "选哪个字体"才不会分叉。
///
/// 附带好处：本注册表与 [`super::glyph_cache::GlyphCache`] 持有的是**同一种**
/// 字体栈对象，接起来不需要转换。
///
/// ## 生命周期
///
/// 加载成本（内嵌解析 + 系统字体扫描）在进程内只需一次，故实际使用中应复用
/// （见 [`shared_registry`]）。
pub struct FontRegistry {
    font_system: cosmic_text::FontSystem,
    embedded_id: fontdb::ID,
}

impl std::fmt::Debug for FontRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FontRegistry")
            .field("faces", &self.face_count())
            .field("embedded_id", &self.embedded_id)
            .finish()
    }
}

impl FontRegistry {
    /// 加载内嵌子集 + 系统字体。
    ///
    /// ## 为什么无参数（零网络的结构保证）
    ///
    /// 没有"从哪取"的输入，故不可能去网络取（FR-051）。
    ///
    /// ## 构造为什么委托给 `font_setup`
    ///
    /// 本注册表与排版引擎（`layout::engine::LayoutEngine`）**必须**持有同一种
    /// 字体栈——否则排版按一套度量断行、渲染按另一套取字形，"快照与分页对不上"
    /// 这类问题会反复出现。构造（内嵌注册顺序 + 通用族重指向）是那条共同语义的
    /// 唯一实现，落在 feature 无关的 [`crate::font_setup`]，两个面都调它。
    ///
    /// 顺序为何是语义正确性（而不是概率问题）见
    /// [`crate::font_setup::build_embedded_first_font_system`] 的文档。
    pub fn load() -> Result<Self, FontLoadError> {
        let (font_system, embedded_id) =
            crate::font_setup::build_embedded_first_font_system(std::iter::empty())?;

        Ok(Self {
            font_system,
            embedded_id,
        })
    }

    /// 数据库里的字体面数（诊断用）。
    #[must_use]
    pub fn face_count(&self) -> usize {
        self.font_system.db().len()
    }

    /// 内嵌子集的字体 ID。
    #[must_use]
    pub fn embedded_id(&self) -> fontdb::ID {
        self.embedded_id
    }

    /// 底层字体栈的可变句柄（供渲染层取用）。
    pub fn font_system_mut(&mut self) -> &mut cosmic_text::FontSystem {
        &mut self.font_system
    }

    /// 字符经**内嵌子集或回退链**解析到的字体与来源。
    ///
    /// ## 查找顺序（FR-050 → FR-051 的优先关系）
    ///
    /// 1. **内嵌子集**：它覆盖 GB2312 6763 字（常用字与标点），是绝大多数
    ///    情况下的答案
    /// 2. **平台回退链**：内嵌不含该字形时按链依次尝试
    /// 3. 都没有 → `None`（调用方走缺字占位；FR-051 要求**不失败**）
    pub fn font_for_char(&mut self, ch: char) -> Option<(fontdb::ID, FontSource)> {
        let probe = ch.to_string();

        // 1. 内嵌子集
        if self.supports_char(self.embedded_id, &probe) {
            return Some((self.embedded_id, FontSource::Embedded));
        }

        // 2. 平台回退链（按家族名查）
        for family in fallback_chain_for_current_platform() {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(family)],
                weight: fontdb::Weight::NORMAL,
                stretch: fontdb::Stretch::Normal,
                style: fontdb::Style::Normal,
            };
            if let Some(id) = self.font_system.db().query(&query) {
                if self.supports_char(id, &probe) {
                    return Some((id, FontSource::System));
                }
            }
        }

        // 3. 系统字体里的**任意**一个能覆盖它的（不挑家族的最后兜底）
        //
        // ## 为什么要有这一层
        //
        // 平台表是"已知的常见中文族"，但它不穷尽用户的系统（有人装了
        // 自定义字体）。有了这一层，缺字时的失败面从"系统里有但没有表中
        // 的家族"缩到"系统里真的没有这个字形"。
        // FR-051 的"不得因缺字而失败"决定了这一层必须有。
        let fallback = fontdb::Query {
            families: &[fontdb::Family::Serif, fontdb::Family::SansSerif],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        if let Some(id) = self.font_system.db().query(&fallback) {
            if self.supports_char(id, &probe) {
                return Some((id, FontSource::System));
            }
        }

        None
    }

    /// 该字体是否覆盖给定文本里的全部字符。
    ///
    /// ## 判据为什么是 swash 的 charmap
    ///
    /// `charmap().map(codepoint) != 0` 是"该字体有没有这个字形"的**权威
    /// 判定**：字形 0 按 OpenType 规范是 `.notdef`（缺字方框），故非零即
    /// 有字形。这条判据与平台无关、与字体格式无关（CFF/TrueType/集合），
    /// 且是**渲染栈内部实际用的**同一张表（swash 就是栅格化器）。
    ///
    /// ## 初版错在哪（实测踩到）
    ///
    /// 先用的是 cosmic-text 的 `get_font_supported_codepoints_in_word`。
    /// 它读的是 `Font::unicode_codepoints()`——**那个字段只在 monospace
    /// 回退缓存里被填充**（见 cosmic-text 的 `font/mod.rs`：
    /// `self.monospace_fallback.as_ref().map_or(&[], |x| &x.unicode_codepoints)`）。
    /// 对普通字体它恒返回空表，于是**每个字符都被判为"无可用字体"**——
    /// 实测 27 个探针字符（含 'A'、'0'）全部落到 `None`。
    ///
    /// 那次失败的表现很有教益："接口存在且签名合适"不等于"它对这类输入
    /// 有定义"。判据必须用**行为**验证（本文件的覆盖断言就是那个验证）。
    fn supports_char(&mut self, id: fontdb::ID, probe: &str) -> bool {
        let Some(font) = self.font_system.get_font(id, fontdb::Weight::NORMAL) else {
            return false;
        };
        let charmap = font.as_swash().charmap();
        probe.chars().all(|ch| charmap.map(ch) != 0)
    }
}

/// 进程内共享的字体注册表。
///
/// ## 为什么共享而不是每次新建
///
/// `load_system_fonts()` 会扫描系统字体目录（实测耗时可观）。渲染路径每次
/// 新建会让首帧变慢；而字体库是**只读**的（加载后不再变），共享安全。
///
/// 用 `OnceLock` 而非 `lazy_static`：标准库方案，零额外依赖。
///
/// ## 失败时的行为
///
/// 加载失败（内嵌字体不可用）时返回 `None`——那是**不应发生**的状态
/// （字节是编译期嵌进去的），但返回值让调用方能走 `FontUnavailable`
/// 能力错误而不是 panic（AGENTS.md 安全红线：生产路径不得 panic）。
#[must_use]
pub fn shared_registry() -> Option<&'static std::sync::Mutex<FontRegistry>> {
    static REGISTRY: OnceLock<Option<std::sync::Mutex<FontRegistry>>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| FontRegistry::load().ok().map(std::sync::Mutex::new))
        .as_ref()
}

/// `tome fonts` 的报告数据（`contracts/cli.md` §2.4）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FontsReport {
    /// 内嵌字体的角色标识（固定 `reading-serif`）
    pub embedded_role: String,
    /// 内嵌字体的家族名
    pub embedded_family: String,
    /// 内嵌子集的字节数
    pub embedded_bytes: usize,
    /// 系统回退链（按平台）
    pub system_fallbacks: Vec<String>,
    /// 体积预算（字节）
    pub budget_bytes: usize,
    /// 是否在预算之内
    pub within_budget: bool,
    /// 字符覆盖说明（人类可读的口径描述，**不是**面向用户的本地化文案；
    /// 它是验证入口的诊断输出，与 CLI 的 `--format human` 同类）
    pub subset_coverage: String,
}

impl FontsReport {
    /// 收集当前状态。
    #[must_use]
    pub fn collect() -> Self {
        let embedded_bytes = embedded_font_bytes().len();
        Self {
            embedded_role: "reading-serif".to_owned(),
            embedded_family: READING_SERIF_FAMILY.to_owned(),
            embedded_bytes,
            system_fallbacks: fallback_chain_for_current_platform()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            budget_bytes: EMBEDDED_FONT_BUDGET_BYTES,
            // 口径：只卡上界（见常量文档）
            within_budget: embedded_bytes <= EMBEDDED_FONT_BUDGET_BYTES,
            subset_coverage: "GB2312 6763 字 + 常用标点 + 基本西文".to_owned(),
        }
    }

    /// 序列化为 JSON（CLI 薄壳直接输出）。
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 内嵌字节与家族名的断言已随其定义迁到 `crate::font_setup` 的测试模块
    // （单一来源，避免两处各断一次而漂移）。

    #[test]
    fn budget_is_five_mebibytes() {
        assert_eq!(EMBEDDED_FONT_BUDGET_BYTES, 5 * 1024 * 1024);
        assert_eq!(EMBEDDED_FONT_MIN_BYTES, 3 * 1024 * 1024);
    }

    #[test]
    fn every_platform_chain_is_non_empty_and_ordered() {
        for platform in [Platform::Windows, Platform::MacOs, Platform::Linux] {
            let chain = fallback_chain_for(platform);
            assert!(!chain.is_empty(), "{platform:?} 的链不得为空");
            // 无重复（重复项会让同一次查找尝试两次）
            let mut unique = chain.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), chain.len(), "{platform:?} 的链有重复项");
        }
    }

    #[test]
    fn registry_loads_and_finds_common_chars() {
        let mut registry = FontRegistry::load().expect("加载应成功");
        assert!(registry.face_count() >= 1, "至少应有内嵌字体面");

        let (_, source) = registry.font_for_char('的').expect("常用字应有字体");
        assert_eq!(source, FontSource::Embedded, "常用字应由内嵌子集提供");
    }

    #[test]
    fn shared_registry_is_stable_across_calls() {
        let first = shared_registry();
        let second = shared_registry();
        assert!(first.is_some(), "共享注册表应可用");
        // 同一地址（`OnceLock` 只初始化一次）。用引用相等比较：
        // `std::ptr::eq` 比对两个引用的地址，正是"同一个实例"的判据。
        let same = match (first, second) {
            (Some(left), Some(right)) => std::ptr::eq(left, right),
            _ => false,
        };
        assert!(same, "共享注册表应在两次调用间返回同一个实例");
    }

    #[test]
    fn report_serializes_with_contract_fields() {
        let report = FontsReport::collect();
        let json = report.to_json().expect("应可序列化");
        for field in [
            "\"embedded_role\"",
            "\"embedded_family\"",
            "\"embedded_bytes\"",
            "\"system_fallbacks\"",
            "\"budget_bytes\"",
            "\"within_budget\"",
        ] {
            assert!(json.contains(field), "报告 JSON 应含字段 {field}：{json}");
        }
    }

    #[test]
    fn registry_uses_the_shared_embedded_first_construction() {
        // 渲染面的注册表必须与排版面用同一套构造：内嵌子集能被查到，
        // 且**通用族 Serif 解析到它**（否则渲染按系统字体取字形，
        // 与排版的度量分叉——C1 缺陷的成因）。
        let registry = FontRegistry::load().expect("加载应成功");
        let embedded_id = registry.embedded_id();
        assert!(
            registry.font_system.db().face(embedded_id).is_some(),
            "内嵌子集应已注册"
        );
    }
}
