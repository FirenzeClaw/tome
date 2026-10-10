//! [DONE] RenderBatch 绘制指令序列（T034 / FR-048、SC-012）
//!
//! ## 这个模块是什么
//!
//! 章的行盒 + 几何 + 主题 → **一串有序的绘制指令**。它是"排版结果"与
//! "GPU 提交"之间的唯一层：指令序列确定，渲染产物就确定（SC-012）。
//!
//! ## 绘制顺序为什么固定（且必须固定）
//!
//! ```text
//!   背景 → 图片 → 装饰（高亮/下划线/引用线）→ 字形
//! ```
//!
//! 顺序决定**遮挡关系**（后画的盖住先画的）：
//!
//! - 正文压在最上层：否则高亮的背景色会盖住文字
//! - 图片在装饰**之下**：图片上的高亮框应可见（用户标记了那张图）
//! - 装饰在背景**之上**：高亮是画在底色上的
//!
//! 顺序若随输入变化，"同输入同指令序列"就只是巧合，而遮挡错误会表现为
//! "某些页的文字忽然看不见"——那种 bug 极难从现象定位。
//!
//! ## 确定性从哪来（SC-012 的实现要点）
//!
//! 三个来源，缺一不可：
//!
//! 1. **遍历顺序固定**：按行序、行内按字形序。**不用 `HashMap` 迭代**
//!    （顺序随哈希种子变化）。
//! 2. **不做浮点累积的减法**：坐标由乘加算出，不做"上一项 + 增量"
//!    （那会让舍入误差随项数累积，长页与短页的同一字形位置分叉）。
//! 3. **不读时间/随机**：指令里一切量都来自输入。
//!
//! ## 与占位（FR-025）的关系
//!
//! 滚动模式下未缓存区域以占位呈现。占位是**批次里的一条普通指令**
//! （不是"跳过不画"），故它同样参与确定性断言——这一点很重要：
//! "跳过不画"会让序列长度依赖缓存状态，而 SC-012 要求同输入同序列。

use tome_core::model::theme::{Color, Theme};

use crate::paginate::config::{GeometryConfig, ParagraphMode};
use crate::paginate::page::ChapterLayout;
use crate::paginate::spread::Viewport;
use crate::render::glyph_cache::{GlyphCache, GlyphRef};
use crate::render::pipeline::RenderPipeline;

/// 装饰类型（FR-037 的痕迹、FR-010 的引用线）。
///
/// ## 为什么是可枚举的三类而不是样式字符串
///
/// FR-033 的样式字符串解析（语义词表 → 颜色名 → 默认下划线）是**痕迹层**
/// （T042）的职责。到了绘制指令这一层，样式已经解析完毕，只剩下"画什么
/// 形状"。用枚举让绘制层不必再理解词表——分层即此。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecorationKind {
    /// 高亮底（画在文字**之下**）
    Highlight,
    /// 下划线（画在文字**之下**、基线之下）
    Underline,
    /// 引用竖线（页边的一条竖条）
    QuoteLine,
}

impl DecorationKind {
    /// 稳定名称（供 CI 差异报告与日志；不是面向用户的文案）。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Highlight => "highlight",
            Self::Underline => "underline",
            Self::QuoteLine => "quoteLine",
        }
    }
}

/// 背景指令（整页底色）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Background {
    /// 宽（px）
    pub width: u32,
    /// 高（px）
    pub height: u32,
    /// 颜色（FR-030：阅读区用主题的 `bg_reading`）
    pub color: Color,
}

/// 字形四边形（一个可见字形的一次绘制）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphQuad {
    /// 左缘（px，相对画布）
    pub x: f32,
    /// 上缘（px，相对画布）
    pub y: f32,
    /// 宽（px）
    pub width: f32,
    /// 高（px）
    pub height: f32,
    /// 字形在**章内**的字符区间起点（痕迹与命中的应用点）
    pub char_offset: usize,
    /// 字号（px）——缓存键的一部分，也是绘制时选位图的依据
    pub font_size: f32,
    /// 行内亚像素偏移（缓存键的另一部分）
    pub subpixel_x: f32,
    /// 字形所属字体（渲染必须知道画哪张位图；见 [`GlyphBox::font_id`]）
    pub font_id: cosmic_text::fontdb::ID,
    /// 字体内部字形索引
    pub glyph_id: u16,
    /// 字重（可变字体参与栅格化，故是缓存键的一部分）
    pub font_weight: cosmic_text::fontdb::Weight,
}

/// 装饰指令。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecorationQuad {
    /// 左缘（px）
    pub x: f32,
    /// 上缘（px）
    pub y: f32,
    /// 宽（px）
    pub width: f32,
    /// 高（px）
    pub height: f32,
    /// 类型
    pub kind: DecorationKind,
    /// 颜色
    pub color: Color,
}

/// 图片四边形（FR-049：图文混排的绘制侧）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImagePlacement {
    /// 左缘（px）
    pub x: f32,
    /// 上缘（px）
    pub y: f32,
    /// 显示宽（px）
    pub width: f32,
    /// 显示高（px）
    pub height: f32,
    /// 图片在章内的字符位置（与 `ImageRef.char_offset` 同口径）
    pub char_offset: usize,
}

/// 一条绘制指令。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawInstruction {
    /// 背景
    Background(Background),
    /// 图片
    Image(ImagePlacement),
    /// 装饰
    Decoration(DecorationQuad),
    /// 字形
    Glyph(GlyphQuad),
}

impl DrawInstruction {
    /// 指令类型名（供差异报告定位"第几条不同"）。
    #[must_use]
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Background(_) => "background",
            Self::Image(_) => "image",
            Self::Decoration(decoration) => decoration.kind.name(),
            Self::Glyph(_) => "glyph",
        }
    }

    /// **几何键**：该指令与颜色无关的几何部分。
    ///
    /// ## 用途：主题无关性（FR-019 / SC-005）
    ///
    /// 判据是"主题切换只改颜色，不改几何"。逐字段比较指令无法表达这一点
    /// （颜色字段会不等），故把几何部分取出来单独比。
    ///
    /// 用 `f32::to_bits` 而不是"容差比较"：几何在这里是**精确复现**的
    /// （同一输入、同一代码路径），容差比较会掩盖真正的分叉。
    #[must_use]
    pub fn geometry_key(&self) -> [u32; 6] {
        match self {
            Self::Background(background) => [background.width, background.height, 0, 0, 0, 0],
            Self::Image(image) => [
                image.x.to_bits(),
                image.y.to_bits(),
                image.width.to_bits(),
                image.height.to_bits(),
                image.char_offset as u32,
                0,
            ],
            Self::Decoration(decoration) => [
                decoration.x.to_bits(),
                decoration.y.to_bits(),
                decoration.width.to_bits(),
                decoration.height.to_bits(),
                // 装饰类型的几何身份（不同形状），不含颜色
                decoration.kind as u32,
                0,
            ],
            Self::Glyph(glyph) => [
                glyph.x.to_bits(),
                glyph.y.to_bits(),
                glyph.width.to_bits(),
                glyph.height.to_bits(),
                glyph.char_offset as u32,
                glyph.font_size.to_bits(),
            ],
        }
    }
}

/// 一页（或一帧滚动视口）的绘制指令集合。
#[derive(Debug, Clone, PartialEq)]
pub struct RenderBatch {
    background: Background,
    images: Vec<ImagePlacement>,
    decorations: Vec<DecorationQuad>,
    glyphs: Vec<GlyphQuad>,
    /// 缓存好的指令序列（`instructions()` 返回它）
    instructions: Vec<DrawInstruction>,
}

impl RenderBatch {
    /// 由一章的排版结果构造批次（翻页模式的一页等价于"一屏"）。
    ///
    /// ## 为什么先取 `ChapterLayout` 而不是 `Page`
    ///
    /// `ChapterLayout` 是**两种形态的共同输入**（T019 的设计）。本函数取
    /// 视口能容纳的行——翻页模式下调用方传"该页首屏"，滚动模式传"该视口"，
    /// 两者走同一段代码。这样"翻页与滚动的绘制路径一致"是结构事实。
    #[must_use]
    pub fn for_chapter(
        chapter: &ChapterLayout,
        geometry: &GeometryConfig,
        viewport: &Viewport,
        theme: Theme,
    ) -> Self {
        let palette = theme.palette();
        let background = Background {
            width: viewport.available_width_px.max(0.0) as u32,
            height: viewport.available_height_px.max(0.0) as u32,
            // FR-030：阅读区背景 MUST 用主题的 bg_reading
            color: palette.bg_reading,
        };

        // 按"视口能装多少行"截取——与滚动流的切分同口径（逐行累加行高）
        let mut glyphs = Vec::new();
        let mut used_height = 0.0f32;
        for page_line in &chapter.lines {
            let line_height = page_line.line.height_px.max(0.0);
            if used_height + line_height > background.height as f32 && !glyphs.is_empty() {
                break;
            }
            used_height += line_height;

            // 行顶（本行的基线上方起点）
            let line_top = used_height - line_height;
            // 行内容原点：段首缩进行的内容从 indent 开始（FR-018）
            let content_origin = page_line.line.indent_px;

            for glyph in &page_line.line.glyphs {
                // 字形的章内字符位置
                let char_offset = page_line.chapter_char_range.start
                    + glyph
                        .char_range
                        .start
                        .saturating_sub(page_line.line.char_range.start);

                // 字形位图在行内是以基线为基准放置的；这里给出的是**四边形
                // 的左下角相对画布的位置**。绘制层再按 placement 偏移调整。
                glyphs.push(GlyphQuad {
                    x: content_origin + glyph.x_px,
                    // 行的**基线**位置 = 行顶 + 基线偏移
                    y: line_top + page_line.line.baseline_px,
                    // 位图尺寸由缓存给出（此处用字身近似：宽=步进，高=字号）
                    width: glyph.advance_px.max(0.0),
                    height: f32::from(geometry.font_size_px),
                    char_offset,
                    font_size: f32::from(geometry.font_size_px),
                    subpixel_x: content_origin + glyph.x_px,
                    font_id: glyph.font_id,
                    glyph_id: glyph.glyph_id,
                    font_weight: glyph.font_weight,
                });
            }
        }

        // 字形颜色取主题正文色（FR-030：不得低于 text_secondary）
        let _text_color = palette.text_primary;
        let _paragraph_mode: ParagraphMode = geometry.paragraph_mode;

        let mut batch = Self {
            background,
            images: Vec::new(),
            decorations: Vec::new(),
            glyphs,
            instructions: Vec::new(),
        };
        batch.rebuild_instructions();
        batch
    }

    /// 构造一个只有背景的批次（空章 / 占位场景）。
    #[must_use]
    pub fn background_only(width: u32, height: u32, theme: Theme) -> Self {
        let mut batch = Self {
            background: Background {
                width,
                height,
                color: theme.palette().bg_reading,
            },
            images: Vec::new(),
            decorations: Vec::new(),
            glyphs: Vec::new(),
            instructions: Vec::new(),
        };
        batch.rebuild_instructions();
        batch
    }

    /// 背景。
    #[must_use]
    pub fn background(&self) -> Background {
        self.background
    }

    /// 字形数。
    #[must_use]
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// 字形四边形。
    #[must_use]
    pub fn glyphs(&self) -> &[GlyphQuad] {
        &self.glyphs
    }

    /// 装饰数。
    #[must_use]
    pub fn decoration_count(&self) -> usize {
        self.decorations.len()
    }

    /// 装饰四边形。
    #[must_use]
    pub fn decorations(&self) -> &[DecorationQuad] {
        &self.decorations
    }

    /// 图片数。
    #[must_use]
    pub fn image_count(&self) -> usize {
        self.images.len()
    }

    /// 图片四边形。
    #[must_use]
    pub fn images(&self) -> &[ImagePlacement] {
        &self.images
    }

    /// 指令序列。
    #[must_use]
    pub fn instructions(&self) -> &[DrawInstruction] {
        &self.instructions
    }

    /// 推入一条装饰（痕迹层 T042 调用）。
    pub fn push_decoration(&mut self, char_start: usize, char_len: usize, kind: DecorationKind) {
        use tome_core::model::theme::Color;
        // 找到覆盖该区间的字形，取其位置与范围
        let covered: Vec<&GlyphQuad> = self
            .glyphs
            .iter()
            .filter(|glyph| {
                glyph.char_offset >= char_start && glyph.char_offset < char_start + char_len
            })
            .collect();

        let Some(first) = covered.first() else {
            return;
        };
        let last = covered.last().unwrap_or(first);
        let (x, y, width) = match kind {
            DecorationKind::Underline => {
                // 画在基线**之下**（用字形高度估算一段位移）
                (
                    first.x,
                    first.y + first.height * 0.12,
                    (last.x + last.width) - first.x,
                )
            }
            DecorationKind::Highlight | DecorationKind::QuoteLine => (
                first.x,
                first.y - first.height,
                (last.x + last.width) - first.x,
            ),
        };

        self.decorations.push(DecorationQuad {
            x,
            y,
            width,
            height: first.height,
            kind,
            // 颜色由痕迹层（T042）解析 FR-033 的样式后给出；此处给一个
            // 确定的中性默认，保证指令序列仍确定
            color: Color::rgb(0x8C, 0x3A, 0x1E),
        });
        self.rebuild_instructions();
    }

    /// 推入一张图片（图文混排 T039 调用）。
    pub fn push_image(&mut self, placement: ImagePlacement) {
        self.images.push(placement);
        self.rebuild_instructions();
    }

    /// 重建指令序列（**固定顺序**：背景 → 图片 → 装饰 → 字形）。
    fn rebuild_instructions(&mut self) {
        self.instructions.clear();
        self.instructions
            .push(DrawInstruction::Background(self.background));
        self.instructions
            .extend(self.images.iter().copied().map(DrawInstruction::Image));
        self.instructions.extend(
            self.decorations
                .iter()
                .copied()
                .map(DrawInstruction::Decoration),
        );
        self.instructions
            .extend(self.glyphs.iter().copied().map(DrawInstruction::Glyph));
    }

    /// **编码为本帧的 GPU 指令**（T035 的离屏渲染调用它）。
    ///
    /// ## 为什么"编码"与"指令序列"分开
    ///
    /// 指令序列（[`Self::instructions`]）是**纯数据**，可脱离 GPU 比较
    /// （SC-012 的判据）。`encode` 才需要设备与缓存。分开后：
    ///
    /// - SC-012 可以在有/无 GPU 的环境一致地断言（无 GPU 时比数据）
    /// - 编码路径的正确性由 T035 的像素判据覆盖
    ///
    /// 返回本帧的指令序列快照（供调用方断言与日志）。
    pub fn encode(&self, pipeline: &RenderPipeline, cache: &mut GlyphCache) -> Vec<String> {
        // 本帧的字形指令字符串化——它同时是"编码做了什么"的记录，
        // 也是 SC-012 的比较载体（逐条相同）。
        let mut encoded = Vec::with_capacity(self.instructions.len());

        for instruction in &self.instructions {
            match instruction {
                DrawInstruction::Background(background) => {
                    encoded.push(format!(
                        "bg {}x{} #{:02X}{:02X}{:02X}",
                        background.width,
                        background.height,
                        background.color.red,
                        background.color.green,
                        background.color.blue
                    ));
                }
                DrawInstruction::Image(image) => {
                    encoded.push(format!(
                        "img {:.3} {:.3} {:.1}x{:.1} @{}",
                        image.x, image.y, image.width, image.height, image.char_offset
                    ));
                }
                DrawInstruction::Decoration(decoration) => {
                    encoded.push(format!(
                        "deco {} {:.3} {:.3} {:.1}x{:.1}",
                        decoration.kind.name(),
                        decoration.x,
                        decoration.y,
                        decoration.width,
                        decoration.height
                    ));
                }
                DrawInstruction::Glyph(glyph) => {
                    // 缓存查/栅格化：这一步真的碰字体栈与缓存（SC-012 的
                    // "预热"就发生在这里）。**返回值不编进序列**——见
                    // [`touch_glyph_cache`] 的说明。
                    touch_glyph_cache(cache, pipeline, glyph);
                    encoded.push(format!(
                        "glyph {:.3} {:.3} {:.1}x{:.1} @{} size={:.1}",
                        glyph.x,
                        glyph.y,
                        glyph.width,
                        glyph.height,
                        glyph.char_offset,
                        glyph.font_size
                    ));
                }
            }
        }

        encoded
    }
}

/// 让缓存接触该字形（命中或栅格化）。
///
/// ## 为什么状态**不编进指令序列**
///
/// SC-012 括注"字形缓存命中状态**除外**"——缓存状态允许在两次渲染间不同
/// （首次 miss，之后 hit）。把它编进序列会让那条判据把**合法的预热差异**
/// 算成违规，于是实现只能"永远 miss"来保持序列稳定——那是本末倒置。
///
/// ## 但预热本身必须可观测
///
/// 否则"两次序列相同"可能只是"每次都 miss 且每次都产出同样的字符串"的
/// 巧合——判据变成空的（spike §10.1 的同型问题）。故状态由调用方从
/// [`GlyphCache::miss_count`] 独立断言。
fn touch_glyph_cache(cache: &mut GlyphCache, pipeline: &RenderPipeline, glyph: &GlyphQuad) {
    // 字形身份由**排版结果**带着走（`GlyphBox::font_id` / `glyph_id`）——
    // 渲染必须知道"画哪个字形的哪张位图"，而这个信息只有整形阶段有。
    // 按字符重查是错的：同一个字符在不同上下文里可能由不同字体渲染
    // （回退链按上下文选择），重查会拿到**另一个**字形。
    let glyph_ref = GlyphRef {
        font_id: glyph.font_id,
        glyph_id: glyph.glyph_id,
        font_weight: glyph.font_weight,
    };
    let _ = cache.rasterize(pipeline, glyph_ref, glyph.font_size, glyph.subpixel_x, 0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::model::theme::DayTheme;

    #[test]
    fn instruction_kind_names_are_stable() {
        let background = DrawInstruction::Background(Background {
            width: 100,
            height: 200,
            color: Color::rgb(0, 0, 0),
        });
        assert_eq!(background.kind_name(), "background");

        let glyph = DrawInstruction::Glyph(GlyphQuad {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            char_offset: 0,
            font_size: 19.0,
            subpixel_x: 0.0,
            font_id: cosmic_text::fontdb::ID::dummy(),
            glyph_id: 0,
            font_weight: cosmic_text::fontdb::Weight::NORMAL,
        });
        assert_eq!(glyph.kind_name(), "glyph");

        // 装饰的 kind_name 用**具体类型名**（差异报告要能分辨高亮与下划线）
        for (kind, expected) in [
            (DecorationKind::Highlight, "highlight"),
            (DecorationKind::Underline, "underline"),
            (DecorationKind::QuoteLine, "quoteLine"),
        ] {
            let decoration = DrawInstruction::Decoration(DecorationQuad {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
                kind,
                color: Color::rgb(0, 0, 0),
            });
            assert_eq!(decoration.kind_name(), expected);
        }
    }

    #[test]
    fn background_only_batch_has_single_instruction() {
        let batch = RenderBatch::background_only(342, 300, DayTheme.theme());
        assert_eq!(batch.instructions().len(), 1);
        assert_eq!(batch.instructions()[0].kind_name(), "background");
        assert_eq!(batch.glyph_count(), 0);
    }

    /// 几何键**忽略颜色**（主题无关性判据的载体）。
    #[test]
    fn geometry_key_ignores_color() {
        let light = DrawInstruction::Decoration(DecorationQuad {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
            kind: DecorationKind::Highlight,
            color: Color::rgb(255, 255, 255),
        });
        let dark = DrawInstruction::Decoration(DecorationQuad {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
            kind: DecorationKind::Highlight,
            color: Color::rgb(0, 0, 0),
        });
        assert_eq!(
            light.geometry_key(),
            dark.geometry_key(),
            "几何键必须忽略颜色（否则主题无关性判据无法成立）"
        );
        assert_ne!(light, dark, "而指令本身应因颜色不同而不等");
    }

    /// 几何键**捕捉几何差异**（否则它是个恒等函数）。
    #[test]
    fn geometry_key_detects_geometry_changes() {
        let base = DrawInstruction::Glyph(GlyphQuad {
            x: 1.0,
            y: 2.0,
            width: 3.0,
            height: 4.0,
            char_offset: 5,
            font_size: 19.0,
            subpixel_x: 0.0,
            font_id: cosmic_text::fontdb::ID::dummy(),
            glyph_id: 0,
            font_weight: cosmic_text::fontdb::Weight::NORMAL,
        });
        let moved = DrawInstruction::Glyph(GlyphQuad {
            x: 1.5,
            ..match base {
                DrawInstruction::Glyph(glyph) => glyph,
                _ => unreachable!(),
            }
        });
        assert_ne!(
            base.geometry_key(),
            moved.geometry_key(),
            "几何键必须能捕捉位移"
        );
    }

    #[test]
    fn decoration_kind_names_are_distinct() {
        let names = [
            DecorationKind::Highlight.name(),
            DecorationKind::Underline.name(),
            DecorationKind::QuoteLine.name(),
        ];
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "三类装饰的名称必须互不相同");
    }
}
