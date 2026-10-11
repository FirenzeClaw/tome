//! [DONE] 内嵌正文衬线的**唯一**装载点（FR-050 / FR-051）
//!
//! ## 本模块为什么存在（C1 缺陷修复的落点）
//!
//! 内嵌子集原先只在**渲染层**注册（`render::fonts::FontRegistry`），排版层
//! （`layout::engine::LayoutEngine`）走的是裸 `FontSystem::new()` + 通用族
//! 查询——于是排版度量由**系统字体**决定，三平台必然分叉（FR-050 失守），
//! 且每台用户机器的版面随其系统字体漂移。
//!
//! 修复的做法是本模块：把**内嵌字节**与**FontSystem 的构造**提到 feature
//! 无关的位置，排版与渲染**共用同一次构造**。本模块的两条不变量：
//!
//! | 不变量 | 为什么 |
//! |---|---|
//! | 内嵌子集在 `fontdb` 里**先于**系统字体注册 | `fontdb` 按注册顺序解析同名家族（`find_best_match` 在全部候选同分时取**最先插入**者），顺序反了会被系统同名家族静默抢走 |
//! | 通用族 `Serif` **重指向**内嵌子集 | 排版与渲染都按 `Family::Serif` 取字体；不重指向的话该查询解析到的是 `fontdb` 的默认别名（`DejaVu Serif`），内嵌字体**根本不会被查到** |
//!
//! 第二条是本次缺陷的真正机制：即便内嵌字体注册进 `fontdb`，只要通用族别名
//! 不指向它，`Attrs::new().family(Family::Serif)` 就永远解析到系统字体。
//! 「注册了」与「被查到」是两件事。
//!
//! ## 与 `render::fonts` 的分工
//!
//! 本模块只管**装载**（字节、注册、别名、构造）。回退链的**语义**
//! （缺字时按平台依次尝试哪些家族）、覆盖率判定、`tome fonts` 的报告形态
//! 都留在 `render::fonts`——那是渲染面的关切，且需要 `render` feature 下的
//! `SwashCache` 能力。headless 构建只需要"排版拿到确定的度量"，不需要回退链。
//!
//! ## 零网络（FR-051/FR-056）
//!
//! 装载路径只有两条：`include_bytes!`（编译期嵌入）与 fontdb 的系统扫描
//! （本地文件系统）。本模块无任何"从哪取"的输入参数，故结构上不可能取网络。

use std::sync::OnceLock;

use cosmic_text::fontdb::{self, Source as FontdbSource};

/// 内嵌子集的字体家族名（OFL RFN 合规的改名结果）。
///
/// 思源宋体带 Reserved Font Name「Source」，OFL 规定修改版不得用它作字体标识。
/// T008 的子集化脚本已改家族名并随分发附 OFL 全文；本常量是该改名结果的
/// **运行时**落点，`render::fonts` 的 RFN 断言据此防"重新子集化时脚本回退到原名"。
pub const READING_SERIF_FAMILY: &str = "Tome Serif CN";

/// 内嵌子集的字节（编译期嵌入，**不读文件系统**）。
///
/// ## 为什么用 `include_bytes!` 而不是运行时读文件
///
/// 三个理由，缺一不可：
///
/// 1. **单二进制分发**（宪法「附加约束 · 体积纪律」的前提）：运行时不依赖
///    外部资源路径，用户移动/删除 `assets/` 不影响排版与渲染
/// 2. **零网络**（FR-051）：字节在二进制里，不存在"去取"的路径
/// 3. **三平台一致**（FR-050）：同一个二进制在三个平台上嵌的是同一份字节
///
/// ## 为什么放在本模块而不是 `render::fonts`
///
/// headless 构建（`--no-default-features`）里 `render` 模块**不存在**，而
/// `layout` 与 `verify-layout` 照常可用（SC-014）。字节放在渲染模块里会让
/// 排版路径够不到它——那正是原缺陷的形态之一。
pub const EMBEDDED_FONT: &[u8] = include_bytes!("../../../assets/fonts/TomeSerifCN-subset.otf");

/// 内嵌子集的字节。
#[must_use]
pub fn embedded_font_bytes() -> &'static [u8] {
    EMBEDDED_FONT
}

/// 内嵌子集的家族名。
#[must_use]
pub fn family_name() -> &'static str {
    READING_SERIF_FAMILY
}

/// 装载失败的原因（内嵌字节无法解析为可用字体）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FontSetupError {
    /// 内嵌字节无法解析为可用字体。
    ///
    /// 这**不应发生**（字节是编译期 `include_bytes!` 进来的合法 OpenType），
    /// 故它是"构建产物被破坏"的信号——对外映射为 `fontUnavailable`
    /// 能力错误（FR-057：字体不可用要明确报告而非静默失败）。
    #[error("embedded font subset unavailable: {0}")]
    EmbeddedFontUnusable(&'static str),
}

impl FontSetupError {
    /// 映射为本层统一错误面（FR-057/FR-058）。
    #[must_use]
    pub fn to_reader_error(self) -> crate::error::ReaderError {
        crate::error::ReaderError::unsupported_capability(crate::capability::FONT_UNAVAILABLE)
            .with_detail(self.to_string())
    }
}

/// 构造一个**内嵌子集优先**的 `FontSystem`。
///
/// ## 装载顺序（语义正确性，不是概率问题）
///
/// 1. `FontSystem::new()` 先扫系统字体（它的默认行为，无法从外面拆开——
///    `load_fonts` 是私有的）
/// 2. 再把内嵌子集 `load_font_source` 进去
/// 3. 最后把**通用族 `Serif` 重指向内嵌子集**
///
/// 第 3 步是关键：`fontdb` 的通用族默认指向别名（`Serif` → `"DejaVu Serif"`，
/// `SansSerif` → `"Open Sans"`，由 `FontSystem::new_with_fonts` 设定），按**家族名**
/// 查询。不重指向，`Family::Serif` 就永远解析到那个系统别名，内嵌字体虽有
/// ID 也查不到。
///
/// ## 第 1 步在"系统字体里有同名家族"时的影响
///
/// 内嵌子集的家族名是改名后的专有串（`Tome Serif CN`），与任何系统字体都不
/// 撞名；且第 3 步按**我们选定的那个 ID** 反查出家族名去重指向，故系统里
/// 即便有同名字体也不会被选中。判据落在 ID 上，不落在"谁先加载"的概率上。
///
/// ## 返回 ID 而非只返回 `FontSystem`
///
/// 调用方（渲染层）要知道**哪一个** face 是内嵌子集，才能做"该字形来自内嵌
/// 还是系统回退"的归因（`FontSource`）。把 ID 一起返回避免调用方自己重查。
pub fn build_embedded_first_font_system(
    system_fonts: impl IntoIterator<Item = FontdbSource>,
) -> Result<(cosmic_text::FontSystem, fontdb::ID), FontSetupError> {
    let mut font_system = cosmic_text::FontSystem::new_with_fonts(system_fonts);

    let ids = font_system
        .db_mut()
        .load_font_source(FontdbSource::Binary(std::sync::Arc::new(
            EMBEDDED_FONT.to_vec().into_boxed_slice(),
        )));
    let embedded_id = ids
        .into_iter()
        .next()
        .ok_or(FontSetupError::EmbeddedFontUnusable(
            "load_font_source returned no font id",
        ))?;

    rewrite_generic_serif_to_embedded(&mut font_system, embedded_id);

    Ok((font_system, embedded_id))
}

/// 把通用族 `Serif` 的别名重指向**内嵌子集的实际家族名**。
///
/// ## 为什么要从 face 里反查家族名而不是直接用常量
///
/// `FontdbSource::Binary` 装载时家族名由 fontdb **从字节的 name 表解析**得出。
/// 常量与字节里的名字若因某次重新子集化而不一致，直接写常量会让 alias 指向
/// 一个不存在的家族——`query` 返回 `None`，字体静默失效。反查 face 里的
/// 实际名让两者**在结构上不可能分叉**。
///
/// 拿不到 face 信息时退回到常量：那是"fontdb 内部状态异常"的情形，用常量
/// 至少保持行为与常量文档一致，且 `embedded_id` 仍可用于直接查询。
fn rewrite_generic_serif_to_embedded(
    font_system: &mut cosmic_text::FontSystem,
    embedded_id: fontdb::ID,
) {
    let family = font_system
        .db()
        .face(embedded_id)
        .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
        .unwrap_or_else(|| READING_SERIF_FAMILY.to_owned());

    font_system.db_mut().set_serif_family(family);
}

/// 进程内共享的内嵌优先字体栈。
///
/// ## 为什么共享而不是每次新建
///
/// `FontSystem` 的构造要扫系统字体目录（实测耗时可观）。排版（分页预计算对
/// 多章反复排版）与渲染（每次离屏绘制）都想要同一个——字体库加载后**只读**，
/// 共享安全。
///
/// ## 为什么是 `Mutex` 而不是 `Arc<FontSystem>`
///
/// 整形（`Buffer::shape_until_scroll`）需要 `&mut FontSystem`。共享可变状态的
/// 最简形式是 `Mutex`；`cosmic_text::FontSystem` 的整形本身也不便宜，锁竞争
/// 不是这里的瓶颈。用 `OnceLock` 而非 `lazy_static`：标准库方案，零额外依赖。
///
/// ## 失败时的行为
///
/// 加载失败（内嵌字体不可用）时返回 `None`——那是**不应发生**的状态，返回值
/// 让调用方能走 `fontUnavailable` 能力错误而不是 panic
/// （`panic = "abort"` 下 panic 会终止进程）。
#[must_use]
pub fn shared_font_system() -> Option<&'static std::sync::Mutex<SharedFontStack>> {
    static SHARED: OnceLock<Option<std::sync::Mutex<SharedFontStack>>> = OnceLock::new();
    SHARED
        .get_or_init(|| SharedFontStack::load().ok().map(std::sync::Mutex::new))
        .as_ref()
}

/// 共享字体栈：内嵌优先的 `FontSystem` + 内嵌 face 的 ID。
#[derive(Debug)]
pub struct SharedFontStack {
    font_system: cosmic_text::FontSystem,
    embedded_id: fontdb::ID,
}

impl SharedFontStack {
    /// 装载内嵌子集 + 系统字体（无参数 = 零网络的结构保证）。
    pub fn load() -> Result<Self, FontSetupError> {
        let (font_system, embedded_id) = build_embedded_first_font_system(std::iter::empty())?;
        Ok(Self {
            font_system,
            embedded_id,
        })
    }

    /// 底层字体栈的可变句柄。
    pub fn font_system_mut(&mut self) -> &mut cosmic_text::FontSystem {
        &mut self.font_system
    }

    /// 内嵌子集的字体 ID。
    #[must_use]
    pub fn embedded_id(&self) -> fontdb::ID {
        self.embedded_id
    }

    /// 数据库里的字体面数（诊断用）。
    #[must_use]
    pub fn face_count(&self) -> usize {
        self.font_system.db().len()
    }
}

/// 排版取字体时使用的**通用族**。
///
/// ## 为什么集中成一处常量
///
/// 排版（`layout::engine`）与渲染（`render::glyph_cache`）都按这个族取字体。
/// 它必须是 [`build_embedded_first_font_system`] 重指向过的那个（`Serif`）——
/// 两处各写一次 `Family::Serif` 时，改一处漏一处会让排版与渲染分叉到不同
/// 度量上，而现象是"快照与分页对不上"，排查成本极高。
#[must_use]
pub const fn reading_family() -> cosmic_text::Family<'static> {
    cosmic_text::Family::Serif
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_bytes_are_the_font_file() {
        let bytes = embedded_font_bytes();
        assert!(bytes.len() > 100_000);
        assert_eq!(&bytes[..4], b"OTTO", "子集应是 CFF 轮廓的 OTF");
    }

    #[test]
    fn family_name_is_rfn_compliant() {
        assert_eq!(family_name(), "Tome Serif CN");
        assert!(!family_name().contains("Source"));
    }

    #[test]
    fn build_succeeds_and_registers_embedded() {
        let (font_system, embedded_id) =
            build_embedded_first_font_system(std::iter::empty()).expect("装载应成功");
        assert!(!font_system.db().is_empty(), "至少应有内嵌字体面");
        assert!(font_system.db().face(embedded_id).is_some());
    }

    /// **通用族 `Serif` 必须解析到内嵌子集**——这是 C1 的核心不变量。
    ///
    /// 若这条红了，说明 `rewrite_generic_serif_to_embedded` 没生效，
    /// 排版又回到"跟着系统字体漂移"的老路上（FR-050 失守）。
    #[test]
    fn generic_serif_resolves_to_embedded() {
        let (font_system, embedded_id) =
            build_embedded_first_font_system(std::iter::empty()).expect("装载应成功");

        let query = fontdb::Query {
            families: &[fontdb::Family::Serif],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        let resolved = font_system.db().query(&query).expect("Serif 应可解析");
        assert_eq!(
            resolved, embedded_id,
            "通用族 Serif 必须解析到内嵌子集（否则排版度量随系统字体漂移）"
        );
    }

    #[test]
    fn shared_font_stack_is_stable_across_calls() {
        let first = shared_font_system();
        let second = shared_font_system();
        assert!(first.is_some(), "共享字体栈应可用");
        let same = match (first, second) {
            (Some(left), Some(right)) => std::ptr::eq(left, right),
            _ => false,
        };
        assert!(same, "共享字体栈应在两次调用间返回同一个实例");
    }

    #[test]
    fn setup_error_maps_to_capability() {
        let error = FontSetupError::EmbeddedFontUnusable("测试构造");
        let reader_error = error.to_reader_error();
        assert_eq!(
            reader_error.capability_id(),
            Some(crate::capability::FONT_UNAVAILABLE),
            "字体不可用必须报 fontUnavailable 能力标识（FR-057）"
        );
    }
}
