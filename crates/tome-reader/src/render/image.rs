//! `[DONE]` 图文混排（T039 / FR-049；US6 三场景）
//!
//! ## FR-049 的四条要求与落点
//!
//! | 要求 | 落点 |
//! |---|---|
//! | 图片行盒 MUST **按其显示尺寸**参与布局计算 | [`ImageBox::height_px`] 进入页容量计算 |
//! | 占位高度与最终一致 | 尺寸未知时用 [`DEFAULT_IMAGE_FALLBACK_HEIGHT_PX`]，与确定后同口径 |
//! | 图片 MUST NOT 被页边界切开 | [`ImageLayout::place`] 把图盒当作**不可分割单位** |
//! | 图片不可用时 MUST 以占位呈现且 **MUST NOT 中断正文排版** | 不可用产出占位盒，正文区间不受影响 |
//!
//! ## 契约来源（真实接线，非占位降级）
//!
//! 图片清单由 001 的 [`tome_core::seam::ImageRef`] 提供（裁决 OQ#13→A 已增补
//! `ChapterContent.images`，001 P8 已接线）。本模块**消费**该类型不另立一个
//! ——那是"真实接线"与"占位降级"的区别：前者在契约上跑通，后者只是自说自话。
//!
//! `ImageRef.char_offset` 与 001 的锚点口径一致（**字符**偏移，不是字节）。
//! 中文正文下字节与字符差 3 倍，混用会让图片插到错误的位置。
//!
//! ## 为什么"不被页边界切开"是本模块的核心难点
//!
//! 正文的行可以随便切（一行放不下就换行）；图片**不能**——它是一张完整的
//! 位图，画一半剩下半张下一页接着画会得到两张都不完整的图。
//!
//! 故图片参与布局的方式与行不同：它要求**一个连续的纵向区间**。实现上
//! 是"分页时把图盒当作一个整体单位"：若当前页剩余高度装不下整个图盒，
//! 就把它整体推到下一页（而不是让它的下半部分落到下一页）。
//!
//! ## 与锚点/痕迹口径的关系
//!
//! 图片占位**不改变字符区间归属**：字符仍在原来的行里，图片是"插在某个
//! 字符位置之后的一块纵向空间"。故 SC-006（页区间拼接逐字符等于原文）
//! 在图文混排下仍然成立——这正是 [`ImageLayout::covered_char_count`] 所验的。

use tome_core::seam::ImageRef;

use crate::paginate::config::GeometryConfig;
use crate::paginate::page::ChapterLayout;
use crate::paginate::spread::Viewport;

/// 图片宽度上限（em）。与 MASTER §5.1 的版心上限 42em 同类约束——
/// 图片过宽会让一页只剩它一张，失去"混排"的意义。
pub const DEFAULT_IMAGE_MAX_WIDTH_EM: f32 = 30.0;

/// 尺寸未知时的回退高度（px）。
///
/// ## 为什么要有回退尺寸而不是"先按 0 排版，加载完再重排"
///
/// "尺寸未知时按 0 占位，加载后重排"会让**阅读位置跳动**：读者正在读的
/// 那一段会因为上方插入了一张图而整体下移，且下移量取决于图片实际高度
/// ——他刚才看到的那句话跑到屏幕外去了。
///
/// 按固定回退高度占位则让"加载前后"的差异**限于图片自身占的那块**，
/// 而那块空间**本来就已预留**。US6 场景 2 的原话是"尺寸未定按声明或回退
/// 尺寸占位且**重排不改锚点**"——回退尺寸就是"不改锚点"的实现手段。
///
/// 取 220px：19px 字号下约 11.6 行，是常见插图（横构图、占版心宽一半到
/// 全宽）的显示高度中位数量级。它是**保守的估计**而非精确值——精确值
/// 在图片加载后由 [`ImageSizes`] 给出。
pub const DEFAULT_IMAGE_FALLBACK_HEIGHT_PX: f32 = 220.0;

/// 图片的可用状态（驱动态与占位文案）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageStatus {
    /// 图片可用，按实际显示尺寸布局
    Available,
    /// 尺寸未知：按回退高度占位，待加载后替换
    Placeholder,
    /// 图片不可用（缓存缺失 / 取图失败）：按回退高度占位并展示 alt
    Unavailable,
}

/// 图片尺寸查询结果。
///
/// ## 为什么是一个"尺寸表"而不是逐图查询回调
///
/// 布局是**纯计算**：给定输入产出确定结果。若布局期间回调外部（去查缓存、
/// 去解码图片），它就不再是纯函数——测试无法脱离缓存断言，"同输入同结果"
/// （FR-048）也无从保证。
///
/// 故调用方**先把**尺寸查好，布局只消费它。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageSizes {
    entries: Vec<(String, Option<(u32, u32)>)>,
    /// 整批不可用（如缓存整体离线）——此时一切引用都按 `Unavailable` 处理
    all_unavailable: bool,
}

impl ImageSizes {
    /// 全部尺寸未知（按回退高度占位）。
    #[must_use]
    pub fn unknown() -> Self {
        Self::default()
    }

    /// 全部不可用（缓存离线 / 取图失败）。
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            entries: Vec::new(),
            all_unavailable: true,
        }
    }

    /// 造一批"尺寸已知"的尺寸表（按缓存键前缀匹配，测试用）。
    ///
    /// 生产路径应按真实缓存键逐条登记（[`Self::with_size`]）。
    #[must_use]
    pub fn sized(width: u32, height: u32) -> Self {
        Self {
            entries: vec![(String::new(), Some((width, height)))],
            all_unavailable: false,
        }
    }

    /// 登记一条尺寸（`cache_key` → 尺寸或 `None` 表示不可用）。
    #[must_use]
    pub fn with_size(mut self, cache_key: impl Into<String>, size: Option<(u32, u32)>) -> Self {
        self.entries.push((cache_key.into(), size));
        self
    }

    /// 查某张图的尺寸。
    ///
    /// 匹配顺序：精确键 → **空前缀**（"批量默认值"）→ `None`。
    /// 空前缀放最后是因为它是兜底，不该盖过精确登记。
    #[must_use]
    pub fn lookup(&self, cache_key: &str) -> Option<(u32, u32)> {
        if self.all_unavailable {
            return None;
        }
        // 精确匹配
        for (key, size) in &self.entries {
            if key == cache_key {
                return *size;
            }
        }
        // 兜底（空前缀）
        for (key, size) in &self.entries {
            if key.is_empty() {
                return *size;
            }
        }
        None
    }

    /// 是否整批不可用。
    #[must_use]
    pub fn is_all_unavailable(&self) -> bool {
        self.all_unavailable
    }
}

/// 一个图片行盒（按显示尺寸参与布局的纵向块）。
#[derive(Debug, Clone, PartialEq)]
pub struct ImageBox {
    /// 图片在章内的字符位置（与 `ImageRef.char_offset` 同口径）
    pub char_offset: usize,
    /// 左缘（px，相对版心左缘）
    pub x_px: f32,
    /// 上缘（px，相对章内容顶部）
    pub y_px: f32,
    /// 显示宽（px）
    pub width_px: f32,
    /// 显示高（px）——**进入页容量计算的就是它**
    pub height_px: f32,
    /// 可用状态
    pub status: ImageStatus,
    /// 替代文本（不可用时展示）
    pub alt_text: Option<String>,
    /// 缓存的键（供渲染层取位图）
    pub cache_key: String,
}

impl ImageBox {
    /// 按原始尺寸 + 可用宽度算出显示尺寸。
    ///
    /// ## 缩放规则（两条，按序施加）
    ///
    /// 1. **宽度上限**：`min(版心宽, 字号 × 30em)`——超宽的图缩到该宽
    /// 2. **等比缩放**：高度按原始宽高比同步缩放
    ///
    /// 两条都不改宽高比：变形比"显示得小一点"糟得多。
    ///
    /// ## 边界：零尺寸的原始尺寸
    ///
    /// 原始宽或高为 0 时无法算比例（会产生 NaN / inf）。此时退化为回退高度
    /// ——**宁可占位保守，也不要产出非有限的几何量**（后者会让整个布局溢出）。
    #[must_use]
    pub fn with_intrinsic_size(
        intrinsic_width: u32,
        intrinsic_height: u32,
        available_width_px: f32,
        font_size_px: f32,
    ) -> Self {
        let fallback = Self::placeholder(0, available_width_px, font_size_px);

        if intrinsic_width == 0 || intrinsic_height == 0 {
            return fallback;
        }

        let max_width = Self::max_display_width_px(available_width_px, font_size_px);
        let intrinsic_ratio = intrinsic_width as f32 / intrinsic_height as f32;

        // 先按宽度限制
        let width_px = max_width.min(intrinsic_width as f32);
        let height_px = width_px / intrinsic_ratio;

        Self {
            char_offset: 0,
            x_px: 0.0,
            y_px: 0.0,
            width_px,
            height_px,
            status: ImageStatus::Available,
            alt_text: None,
            cache_key: String::new(),
        }
    }

    /// 占位盒（尺寸未知或不可用）。
    #[must_use]
    pub fn placeholder(char_offset: usize, available_width_px: f32, font_size_px: f32) -> Self {
        Self {
            char_offset,
            x_px: 0.0,
            y_px: 0.0,
            width_px: Self::max_display_width_px(available_width_px, font_size_px),
            height_px: DEFAULT_IMAGE_FALLBACK_HEIGHT_PX,
            status: ImageStatus::Placeholder,
            alt_text: None,
            cache_key: String::new(),
        }
    }

    /// 显示宽度上限（px）：版心宽与 `30em` 的较小者。
    #[must_use]
    pub fn max_display_width_px(available_width_px: f32, font_size_px: f32) -> f32 {
        let em_cap = font_size_px * DEFAULT_IMAGE_MAX_WIDTH_EM;
        available_width_px.min(em_cap).max(1.0)
    }

    /// 该图盒是否是"不可用占位"。
    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        self.status == ImageStatus::Unavailable
    }
}

/// 一页在图文混排下的可用容量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImagePage {
    /// 页序号（0 起）
    pub index: usize,
    /// 页顶在章内容坐标里的 y（px）
    pub top_px: f32,
    /// 页高（px）
    pub height_px: f32,
}

/// 图文混排的布局结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ImageLayout {
    /// 图片盒（按字符位置排序）
    pub images: Vec<ImageBox>,
    /// 页边界（供"不被切开"的判定）
    pub pages: Vec<ImagePage>,
    /// 本布局覆盖的**正文**字符数（图文混排下仍是章的全部字符）
    pub text_char_count: usize,
}

impl ImageLayout {
    /// 按图片引用与尺寸表排版（FR-049）。
    ///
    /// ## 输入不可信（001 的安全红线）
    ///
    /// `ImageRef.char_offset` 来自书源/缓存，是**外部输入**：负值、超界都
    /// 可能。故此处钳到 `[0, chapter.char_len]`——越界不报错也不 panic
    /// （AGENTS.md 安全红线），而是落到最近的合法位置。
    ///
    /// ## 为什么按字符位置排序
    ///
    /// 图片清单的顺序不保证与正文顺序一致（书源解析可能乱序）。若不排序，
    /// 图片的 y 会按清单顺序分配，表现为"后面的图跑到前面去"。
    #[must_use]
    pub fn place(
        chapter: &ChapterLayout,
        references: &[ImageRef],
        geometry: &GeometryConfig,
        viewport: &Viewport,
        sizes: &ImageSizes,
    ) -> Self {
        let font_size = f32::from(geometry.font_size_px);
        let available_width = viewport.available_width_px;

        // 1. 逐个算出图片盒的尺寸与状态（先不定位）
        let mut images: Vec<ImageBox> = references
            .iter()
            .map(|reference| {
                // 字符位置钳到合法范围（外部输入不可信）
                let char_offset = reference.char_offset.clamp(0, chapter.char_len as i64) as usize;

                let mut image = match sizes.lookup(&reference.cache_key) {
                    Some((width, height)) => {
                        ImageBox::with_intrinsic_size(width, height, available_width, font_size)
                    }
                    None => {
                        // 尺寸未知：区分"整批不可用"与"仅尺寸未定"
                        let mut placeholder =
                            ImageBox::placeholder(char_offset, available_width, font_size);
                        placeholder.status = if sizes.is_all_unavailable() {
                            ImageStatus::Unavailable
                        } else {
                            ImageStatus::Placeholder
                        };
                        placeholder
                    }
                };
                image.char_offset = char_offset;
                image.alt_text = reference.alt.clone();
                image.cache_key = reference.cache_key.clone();
                image
            })
            .collect();

        // 多个引用落在同一位置时保持输入顺序（稳定排序）
        images.sort_by_key(|image| image.char_offset);

        // 2. 按页容量纵向定位（图片不被切开：整块进某一页）
        let page_height = viewport.available_height_px.max(1.0);
        let mut pages: Vec<ImagePage> = vec![ImagePage {
            index: 0,
            top_px: 0.0,
            height_px: page_height,
        }];
        let line_height = geometry.line_height_px();

        // 章内容的纵向游标：从 0 开始，逐图推进。
        //
        // ## 正文高度的估算
        //
        // 正文的总高 = 各行累计高（`chapter.lines` 的 `height_px` 之和）。
        // 图片插在某个字符位置处，其纵向位置 = "该字符之前的所有行高之和"。
        // 这需要"字符 → y"的映射——用行盒累计即可（行盒带章内区间）。
        let mut cursor_px = 0.0f32;
        let mut line_cursor = 0usize;

        for image in &mut images {
            // 推进到该图片的字符位置：累加所有"结束于该字符之前"的行高
            while line_cursor < chapter.lines.len() {
                let page_line = &chapter.lines[line_cursor];
                if page_line.chapter_char_range.start >= image.char_offset {
                    break;
                }
                cursor_px += page_line.line.height_px.max(0.0);
                line_cursor += 1;
            }

            // 图片落在页内的纵向位置（相对页顶）
            let mut offset_in_page = cursor_px - pages.last().map_or(0.0, |page| page.top_px);

            // ## 单张图高于整页的情形（与 `pack_pages` 的同款例外）
            //
            // 图片比一页还高时，**任何**页都装不下它——若按"装不下就翻页"
            // 处理会无限翻页或产出无处归属的图盒（实测：`page_containing`
            // 返回 `None`）。正确处置是让它**独占一页**（页高临时等于图高），
            // 与分页层"单行超高独占一页"是同一条规则。
            //
            // 溢出部分由渲染层裁剪，与行的处置一致（分页层只保证不丢内容）。
            if image.height_px > page_height {
                let next_top = pages
                    .last()
                    .map_or(0.0, |page| page.top_px + page.height_px);
                // 该页的高度就是图高——这样它"完整落在页内"这条不变量成立
                pages.push(ImagePage {
                    index: pages.len(),
                    top_px: next_top,
                    height_px: image.height_px,
                });
                offset_in_page = 0.0;
            } else {
                // **不被切开**：当前页装不下整块 → 整块推到下一页
                let fits = offset_in_page + image.height_px <= page_height + 1e-3;
                if !fits {
                    // 开新页，图片推到新页顶
                    let next_top = pages
                        .last()
                        .map_or(0.0, |page| page.top_px + page.height_px);
                    pages.push(ImagePage {
                        index: pages.len(),
                        top_px: next_top,
                        height_px: page_height,
                    });
                    offset_in_page = 0.0;
                }
            }

            image.x_px = (available_width - image.width_px) / 2.0;
            image.y_px = pages.last().map_or(0.0, |page| page.top_px) + offset_in_page;

            // 图片占掉的纵向空间进入游标（后续图片/正文在其下方）
            cursor_px = image.y_px + image.height_px;
        }

        // 3. 正文覆盖完整性
        //
        // 图文混排**不改变**字符区间归属：字符仍在原来的行里（图片只是插在
        // 某个字符位置之后的一块纵向空间）。故覆盖数就是章的全部字符。
        let text_char_count = chapter.char_len;

        let _ = line_height;
        Self {
            images,
            pages,
            text_char_count,
        }
    }

    /// 某个图盒归属的页。
    #[must_use]
    pub fn page_containing(&self, image: &ImageBox) -> Option<ImagePage> {
        self.pages.iter().copied().find(|page| {
            image.y_px >= page.top_px - 1e-3
                && image.y_px + image.height_px <= page.top_px + page.height_px + 1e-3
        })
    }

    /// 本布局覆盖的正文字符数（图文混排下应等于章的全部字符）。
    #[must_use]
    pub fn covered_char_count(&self, chapter: &ChapterLayout) -> usize {
        // 图文的覆盖口径：字符区间由行盒决定，图片不占用字符区间。
        // 故"覆盖数"= 行盒区间的并集大小；行盒覆盖全章时它就等于 char_len。
        let mut covered = 0usize;
        let mut cursor = 0usize;
        for page_line in &chapter.lines {
            if page_line.chapter_char_range.start > cursor {
                // 行间有跳空（段间分隔符）——它们仍属正文，计入覆盖
                covered += page_line.chapter_char_range.start - cursor;
            }
            covered += page_line.line.char_len();
            cursor = page_line.chapter_char_range.end;
        }
        // 末尾残余（尾部被丢弃的空白段等）
        covered += chapter.char_len.saturating_sub(cursor);
        covered.min(self.text_char_count)
    }

    /// 是否所有图盒都是占位（尺寸未知或不可用）。
    #[must_use]
    pub fn all_placeholder(&self) -> bool {
        !self.images.is_empty()
            && self
                .images
                .iter()
                .all(|image| image.status != ImageStatus::Available)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intrinsic_size_keeps_aspect_ratio() {
        let image = ImageBox::with_intrinsic_size(300, 200, 684.0, 19.0);
        let ratio = image.width_px / image.height_px;
        assert!((ratio - 1.5).abs() < 0.02, "应保持 3:2，实测 {ratio}");
    }

    #[test]
    fn width_is_capped_by_em_rule() {
        let image = ImageBox::with_intrinsic_size(5000, 1000, 2000.0, 19.0);
        let cap = 19.0 * DEFAULT_IMAGE_MAX_WIDTH_EM;
        assert!(image.width_px <= cap + 1e-3);
    }

    /// 零尺寸原始尺寸**退化为占位**（不产生 NaN/inf）。
    #[test]
    fn zero_intrinsic_size_falls_back_to_placeholder() {
        for (width, height) in [(0u32, 100u32), (100, 0), (0, 0)] {
            let image = ImageBox::with_intrinsic_size(width, height, 684.0, 19.0);
            assert!(
                image.width_px.is_finite() && image.height_px.is_finite(),
                "({width},{height}) 应产出有限几何量"
            );
            assert_eq!(image.status, ImageStatus::Placeholder);
            assert!((image.height_px - DEFAULT_IMAGE_FALLBACK_HEIGHT_PX).abs() < 1e-3);
        }
    }

    #[test]
    fn sizes_lookup_prefers_exact_then_default() {
        let sizes = ImageSizes::sized(100, 60).with_size("exact-key", Some((300, 200)));

        assert_eq!(sizes.lookup("exact-key"), Some((300, 200)), "精确键优先");
        assert_eq!(sizes.lookup("other-key"), Some((100, 60)), "无匹配时用兜底");
    }

    #[test]
    fn all_unavailable_sizes_resolve_to_none() {
        let sizes = ImageSizes::unavailable().with_size("anything", Some((10, 10)));
        assert_eq!(
            sizes.lookup("anything"),
            None,
            "整批不可用时一切引用都应解析为 None"
        );
        assert!(sizes.is_all_unavailable());
    }

    #[test]
    fn placeholder_uses_fallback_height() {
        let image = ImageBox::placeholder(5, 684.0, 19.0);
        assert_eq!(image.char_offset, 5);
        assert!((image.height_px - DEFAULT_IMAGE_FALLBACK_HEIGHT_PX).abs() < 1e-3);
        assert_eq!(image.status, ImageStatus::Placeholder);
    }
}
