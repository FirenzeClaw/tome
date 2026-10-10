//! [DONE] 字形缓存（T033 / FR-043、FR-047；research R2 第 1、3、5 条）
//!
//! ## 缓存键的四个维度（research R2 第 3 条）
//!
//! ```text
//! 键 =（字体 ID, 字形 ID, 字号, 亚像素桶）
//! ```
//!
//! **亚像素桶只有 x 轴分 4 档、y 轴恒为 `Zero`**。理由见
//! [`SubpixelBucket::for_y`]：正文的 y 是行基线，本来就落在整数位置，
//! 给它分桶只会为同一字形制造无意义的重复条目。
//!
//! ## 栅格化走 `SwashCache`（R2 第 1 条）
//!
//! 不新增字体解析栈：`SwashCache` 是 cosmic-text 自带的 swash 封装，
//! 而 swash 已在排版路径上（整形用 harfrust，栅格化用 swash，两者同栈）。
//!
//! ## 每帧聚合上传（R2 第 5 条）
//!
//! 新字形不逐次 `write_texture`（每次都是一次 GPU 提交），而是攒进
//! [`GlyphCache::take_pending_uploads`]，由调用方在**一帧的开头**一次性拷。
//! 这既是性能选择也是**确定性**要求：SC-012 要"同输入同指令序列"，
//! 而逐次上传会让指令序列依赖"上次渲染后缓存里剩了什么"。
//!
//! ## 回退与位置（FR-047）
//!
//! 缺字时 `cosmic-text` 在**排版阶段**就已选了回退字体（它有自己的回退
//! 链）。故到了本层，每个字形都带 `font_id`——本层**不需要**自己实现回退
//! 选择，只需要"按 `font_id` 找到字体并栅格化"。这正是 FR-047 "回退 MUST
//! NOT 改变该行其余字符的位置"在架构上成立的原因：回退在 shape 层解决，
//! 到了 layout 输出，位置已经定死了。

use std::collections::HashMap;

use cosmic_text::{CacheKey, CacheKeyFlags, FontSystem, SwashCache, SwashContent};

// `fontdb` 经 cosmic-text 再导出（它自己 `pub use fontdb`）。走这条路径而
// 不把 fontdb 加成本 crate 的直接依赖：本层只需要它的 `ID` 类型，而
// "经同一个再导出取得"能保证与 cosmic-text 内部用的是**同一个 crate 实例**
// （两个不同版本的 fontdb 会让 ID 无法互相比较）。
use cosmic_text::fontdb;

use super::atlas::{AtlasRegion, GlyphAtlas, GrowOutcome};

/// 两个图集区域是否重叠（半开区间：右/下边界不算重叠）。
#[must_use]
fn regions_overlap(left: &AtlasRegion, right: &AtlasRegion) -> bool {
    left.x < right.right()
        && right.x < left.right()
        && left.y < right.bottom()
        && right.y < left.bottom()
}
use super::pipeline::RenderPipeline;

/// 亚像素分桶（x 轴 4 档；y 轴恒 `Zero`）。
///
/// ## 为什么是 4 档
///
/// research R2 定的是 `swash::SubpixelBin` 的 4 分桶（每档 1/4 像素）。
/// 更细的桶按比例膨胀缓存条目数，而 1/4 像素已到亚像素定位的可见精度下限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SubpixelBucket {
    /// `[0, 0.125) ∪ [0.875, 1)` 归此档（swash 的 `SubpixelBin::Zero`）
    Zero,
    /// `[0.125, 0.375)`
    One,
    /// `[0.375, 0.625)`
    Two,
    /// `[0.625, 0.875)`
    Three,
}

impl SubpixelBucket {
    /// x 偏移落哪个桶（与 cosmic-text 的 `SubpixelBin::new` **同口径**）。
    ///
    /// ## 为什么这里重写一遍而不直接调 `SubpixelBin::new`
    ///
    /// `SubpixelBin` 不实现 `Hash`（它只 `Clone/Copy/PartialEq/Eq/Debug`），
    /// 而缓存键需要 `Hash`。故本层用自己的一致枚举 + 同样的分档边界。
    ///
    /// **边界必须逐条对齐**：偏离会让"本层认为同桶"与"cosmic-text 认为
    /// 同桶"分叉，于是两个本该复用同一位图的字形各栅格化一次（正确但变慢），
    /// 或更糟——复用了一个用*不同*偏移栅格化的位图（错位）。
    /// `tests/glyph_cache.rs` 的边界断言就是防这个。
    #[must_use]
    pub fn for_x(offset: f32) -> Self {
        if !offset.is_finite() {
            return Self::Zero;
        }
        let fract = offset - offset.floor();
        // fract 现在在 [0, 1)
        if !(0.125..0.875).contains(&fract) {
            Self::Zero
        } else if fract < 0.375 {
            Self::One
        } else if fract < 0.625 {
            Self::Two
        } else {
            Self::Three
        }
    }

    /// y 偏移的桶——**恒为 `Zero`**（research R2 第 3 条）。
    ///
    /// ## 为什么 y 不分桶
    ///
    /// 正文的 y 位置是**行基线**：分页与滚动都以整行为单位排布，行内的 y
    /// 在绘制时对齐到行盒的整数位置。故同字号字形的 y 只有极少数取值，
    /// 分桶不带来任何复用收益，只为同一字形制造 4 份重复位图。
    ///
    /// R2 的原话是"滚动场景只在 x 轴亚像素、y 对齐行基线取整"——本函数是
    /// 那个决策的落点。参数保留是为了让调用点的语义完整（"y 的这一档是
    /// 哪个桶"），而不是因为取值会变。
    #[must_use]
    pub fn for_y(_offset: f32) -> Self {
        Self::Zero
    }

    /// 该桶的**代表偏移**（桶中心的像素小数部分）。
    ///
    /// 传给 swash 做栅格化时用哪个偏移？用桶中心而非原始偏移，才能让
    /// "同桶复用同一位图"这件事在视觉上自洽（否则第一个入桶的偏移会
    /// 决定整桶的形态，结果依赖访问顺序——那会破坏渲染确定性）。
    #[must_use]
    pub fn center_px(self) -> f32 {
        match self {
            Self::Zero => 0.0,
            Self::One => 0.25,
            Self::Two => 0.5,
            Self::Three => 0.75,
        }
    }
}

/// 缓存键。
///
/// ## 为什么 `font_size` 存 bits
///
/// `f32` 不实现 `Eq`/`Hash`，而缓存键必须可哈希。存 `to_bits()` 是
/// cosmic-text 的 `CacheKey` 同款做法——它让"精确相同的字号"命中，
/// 而"相差一个 ULP"不命中（那是正确的：它们确实会产生不同位图）。
// `Ord` 是**搬迁用**的：重生分配必须按稳定顺序进行（HashMap 的迭代顺序
// 随机，会让搬迁结果随哈希种子变化，破坏 SC-012 的渲染确定性）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphCacheKey {
    /// 字体 ID（来自 cosmic-text 的字体库）
    ///
    /// ## 为什么直接用 `fontdb::ID` 而不是转成 `u64`
    ///
    /// 它已实现 `Hash + Eq + Ord`（fontdb 0.23 的 derive 列表），故可
    /// 直接作键。转 u64 需要 `slotmap` 的内部表示（`ID` 是 newtype，没有
    /// 公开的 `to_u64`），只能经 `Display` 解析——那既绕又脆。
    pub font_id: fontdb::ID,
    /// 字形 ID（字体内部的字形索引）
    pub glyph_id: u16,
    /// 字号的 `f32` 位模式
    pub font_size_bits: u32,
    /// 亚像素桶
    pub bucket: SubpixelBucket,
}

/// 字形位图的**来源**（诊断与 T037 的字体策略断言用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontSource {
    /// 内嵌子集（`assets/fonts/TomeSerifCN-subset.otf`，FR-050）
    Embedded,
    /// 系统字体（含回退链取到的，FR-047/051）
    System,
}

/// 已栅格化的字形条目。
#[derive(Debug, Clone)]
pub struct GlyphEntry {
    /// 缓存键
    pub key: GlyphCacheKey,
    /// 在图集里的区域
    pub rect: AtlasRegion,
    /// 位图数据（`Mask` 时是 8 位 alpha；`Color` 时是 RGBA）
    pub pixels: Vec<u8>,
    /// 内容格式（单通道 mask 还是彩色位图——emoji 会是后者）
    pub content: SwashContent,
    /// 位图相对笔位置的左偏移（swash 的 `placement.left`）
    pub placement_left: i32,
    /// 位图相对笔位置的**上**偏移（swash 的 `placement.top`，向下为负）
    pub placement_top: i32,
    /// 该字形的来源字体类别
    pub source: FontSource,
}

impl GlyphEntry {
    /// 位图像素数。
    #[must_use]
    pub fn pixel_count(&self) -> usize {
        self.pixels.len()
    }

    /// 该项的位图是否是单通道（灰度 AA）。
    ///
    /// ## 为什么这条要暴露
    ///
    /// research R4 的"防 flaky"清单里有"灰度 AA（非 LCD 亚像素）"一条。
    /// 本层一律用 `Format::Alpha`（单通道），故彩色位图只可能来自
    /// `Source::ColorBitmap`（emoji/CBDT）。这条断言让"我们真的在用灰度
    /// AA"可在测试里核对，而不是靠阅读实现来相信。
    #[must_use]
    pub fn is_grayscale(&self) -> bool {
        matches!(self.content, SwashContent::Mask)
    }
}

/// 待上传的位图（一帧的聚合单位）。
#[derive(Debug, Clone)]
pub struct PendingUpload {
    /// 目标区域
    pub rect: AtlasRegion,
    /// 位图数据
    pub pixels: Vec<u8>,
    /// 内容格式
    pub content: SwashContent,
}

/// 字形缓存（CPU 位图 + 图集分配簿记）。
///
/// ## 为什么 CPU 位图要留着（而不是只留 GPU 纹理里的拷贝）
///
/// 两个理由，且都是承重的：
///
/// 1. **搬迁不需要重新栅格化**（R2 第 4 条）：atlas 增长时把旧位图重新
///    分配进新纹理即可，无需再调 swash。丢掉 CPU 副本就退化成 glyphon
///    式的全量重栅格化。
/// 2. **测试与诊断**可脱离 GPU 断言位图内容（像素数、灰度、非空）。
pub struct GlyphCache {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: GlyphAtlas,
    entries: HashMap<GlyphCacheKey, GlyphEntry>,
    /// 本帧新增、尚未上传的位图（按插入顺序，供确定性上传）
    pending: Vec<PendingUpload>,
    hit_count: u64,
    miss_count: u64,
    /// 每个 font_id 的来源分类（T037 接线后由字体加载器登记）
    font_sources: HashMap<fontdb::ID, FontSource>,
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for GlyphCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GlyphCache")
            .field("entries", &self.entries.len())
            .field("hits", &self.hit_count)
            .field("misses", &self.miss_count)
            .field("atlas", &self.atlas)
            .finish_non_exhaustive()
    }
}

impl GlyphCache {
    /// 新建缓存（加载系统字体库）。
    #[must_use]
    pub fn new() -> Self {
        Self::with_atlas_size(super::atlas::INITIAL_ATLAS_SIZE)
    }

    /// 以指定图集边长新建。
    ///
    /// ## 为什么暴露这个（而不是只在测试里用 `#[cfg(test)]`）
    ///
    /// "增长后既有字形仍可用"这条判据要有**真实的增长**才成立，而默认
    /// 1024² 的图集能装下数千个字形——测试要么塞几千个（慢），要么永远
    /// 测不到增长分支（spike §10.1 记的"全绿但没测到"）。
    ///
    /// 故把它做成**公开的构造入口**：集成测试无法访问 `#[cfg(test)]` 的
    /// 私有构造，而"小图集"是验证增长路径的唯一实用手段。生产路径仍用
    /// [`Self::new`]（初始尺寸由 `atlas::INITIAL_ATLAS_SIZE` 定）。
    #[must_use]
    pub fn with_atlas_size(atlas_size: u32) -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            atlas: GlyphAtlas::with_size(atlas_size),
            entries: HashMap::new(),
            pending: Vec::new(),
            hit_count: 0,
            miss_count: 0,
            font_sources: HashMap::new(),
        }
    }

    /// 字体库的可变句柄（T037 用它注册内嵌子集）。
    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// 登记某字体的来源分类（T037 的字体加载器调用）。
    pub fn register_font_source(&mut self, font_id: fontdb::ID, source: FontSource) {
        self.font_sources.insert(font_id, source);
    }

    /// 缓存命中次数。
    #[must_use]
    pub fn hit_count(&self) -> u64 {
        self.hit_count
    }

    /// 缓存未命中次数。
    #[must_use]
    pub fn miss_count(&self) -> u64 {
        self.miss_count
    }

    /// 当前条目数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 图集当前尺寸 `(宽, 高)`。
    #[must_use]
    pub fn atlas_size(&self) -> (u32, u32) {
        (self.atlas.size(), self.atlas.size())
    }

    /// 图集增长次数。
    #[must_use]
    pub fn grow_count(&self) -> u32 {
        self.atlas.grow_count()
    }

    /// **取一个可栅格化的字形**：用给定字符在系统字体里查字形 ID。
    ///
    /// ## 这个入口为什么存在
    ///
    /// 正式的渲染路径由**排版结果**驱动（每个 `GlyphBox` 带 `font_id`/
    /// `glyph_id`），不需要"按字符查字形"。本函数是给**测试与诊断**用的：
    /// 让"缓存命中/未命中"这类判据不必先跑一遍排版就能构造输入。
    ///
    /// 返回 `None` 表示系统字体里没有该字符——调用方应显式跳过而非假装成功。
    pub fn lookup_first_glyph(&mut self, ch: char) -> Option<GlyphRef> {
        use cosmic_text::{Attrs, Family, Shaping};

        // 用 Buffer 走一次最小整形：这是拿到 (font_id, glyph_id) 的可靠途径
        // （直接查 fontdb 需要自己处理回退链，而回退链正是 cosmic-text 的职责）
        let mut buffer = cosmic_text::Buffer::new(
            &mut self.font_system,
            cosmic_text::Metrics::new(19.0, 35.15),
        );
        buffer.set_size(Some(1000.0), None);
        buffer.set_text(
            &ch.to_string(),
            &Attrs::new().family(Family::Serif),
            Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut self.font_system, false);

        let line = buffer.line_shape(&mut self.font_system, 0)?;
        let span = line.spans.first()?;
        let word = span.words.first()?;
        let glyph = word.glyphs.first()?;

        Some(GlyphRef {
            font_id: glyph.font_id,
            glyph_id: glyph.glyph_id,
            font_weight: glyph.font_weight,
        })
    }

    /// 栅格化一个字形并登记进缓存（命中则直接返回既有条目）。
    ///
    /// ## 参数里的 `x_offset` / `y_offset`
    ///
    /// 是**行内小数位置**（px）。x 走 4 分桶，y 忽略（见 [`SubpixelBucket::for_y`]）。
    /// 传桶中心给 swash 而非原始偏移，是"同桶复用同一位图"的确定性前提。
    pub fn rasterize(
        &mut self,
        _pipeline: &RenderPipeline,
        glyph: GlyphRef,
        font_size: f32,
        x_offset: f32,
        y_offset: f32,
    ) -> Option<&GlyphEntry> {
        let key = GlyphCacheKey {
            font_id: glyph.font_id,
            glyph_id: glyph.glyph_id,
            font_size_bits: font_size.to_bits(),
            bucket: SubpixelBucket::for_x(x_offset),
        };

        if self.entries.contains_key(&key) {
            self.hit_count += 1;
            return self.entries.get(&key);
        }

        self.miss_count += 1;

        // y 桶恒 Zero（其代表偏移为 0），故传给 swash 的 y 偏移是 0——
        // "y 对齐行基线"的落点
        let _ = SubpixelBucket::for_y(y_offset);
        let (cache_key, _x_int, _y_int) = CacheKey::new(
            glyph.font_id,
            glyph.glyph_id,
            font_size,
            (key.bucket.center_px(), 0.0),
            glyph.font_weight,
            // hinting 关闭（research R1/R4 的共同决策：挤压语义与三平台一致性）
            CacheKeyFlags::DISABLE_HINTING,
        );

        let image = self
            .swash_cache
            .get_image(&mut self.font_system, cache_key)
            .clone()?;

        let region = match self
            .atlas
            .allocate(image.placement.width, image.placement.height)
        {
            Some(region) => region,
            None => {
                // 放不下 → 增长。增长后**重新分配全部既有条目**（搬迁语义）
                match self.atlas.request_grow() {
                    GrowOutcome::Grew { .. } => {
                        self.reallocate_all_entries();
                        self.atlas
                            .allocate(image.placement.width, image.placement.height)?
                    }
                    // 到上限：本次栅格化失败（调用方应走淘汰；淘汰策略随
                    // 长章节实测后再定，见 tasks.md T030 的内存纪律）
                    GrowOutcome::AtCapacity => return None,
                }
            }
        };

        let entry = GlyphEntry {
            key,
            rect: region,
            pixels: image.data.clone(),
            content: image.content,
            placement_left: image.placement.left,
            placement_top: image.placement.top,
            source: self
                .font_sources
                .get(&key.font_id)
                .copied()
                .unwrap_or(FontSource::System),
        };

        // 聚合成待上传（不立即 write_texture——见模块文档）
        self.pending.push(PendingUpload {
            rect: region,
            pixels: entry.pixels.clone(),
            content: entry.content,
        });

        self.entries.insert(key, entry);
        self.entries.get(&key)
    }

    /// **全部条目两两不重叠**（搬迁正确性的真判据）。
    ///
    /// ## 为什么"落在纹理内"不够
    ///
    /// 增长只增不减，故**旧坐标在更大的纹理里仍然合法**——"坐标没更新"
    /// 这个缺陷能通过"落在纹理内"的断言（实测：把重新分配改成 no-op，
    /// 那条断言照样通过）。而重叠是真实的损坏形态：新分配不知道旧区域
    /// 被占用，会把新字形放进旧位置上，覆盖既有内容。
    ///
    /// 这个判据与 `tests/glyph_cache.rs` 的 `existing_glyphs_survive_atlas_growth`
    /// 是**同一条不变量**的两侧：那里从"内容没被覆盖"看，这里从"区域不重叠"看。
    #[must_use]
    pub fn entries_are_non_overlapping(&self) -> bool {
        let regions: Vec<AtlasRegion> = self.entries.values().map(|entry| entry.rect).collect();
        for (index, left) in regions.iter().enumerate() {
            for right in regions.iter().skip(index + 1) {
                if regions_overlap(left, right) {
                    return false;
                }
            }
        }
        true
    }

    /// 取既有条目（不触发栅格化）。
    #[must_use]
    pub fn entry_for(
        &self,
        glyph: &GlyphRef,
        font_size: f32,
        x_offset: f32,
    ) -> Option<&GlyphEntry> {
        let key = GlyphCacheKey {
            font_id: glyph.font_id,
            glyph_id: glyph.glyph_id,
            font_size_bits: font_size.to_bits(),
            bucket: SubpixelBucket::for_x(x_offset),
        };
        self.entries.get(&key)
    }

    /// 取走本帧待上传的位图（调用方一次性拷进纹理）。
    ///
    /// ## 为什么"取走"而不是"借用"
    ///
    /// 调用方拷完纹理就该清空队列（否则同一批会重复上传）。用 `take` 把
    /// "拷了就清"编码进签名——借用语义会留一个"忘了清"的坑，症状是
    /// 每帧重复上传同一批位图（性能问题，很难从现象定位）。
    pub fn take_pending_uploads(&mut self) -> Vec<PendingUpload> {
        std::mem::take(&mut self.pending)
    }

    /// 待上传的位图数。
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// **搬迁**：atlas 增长后把全部既有条目重新分配一遍。
    ///
    /// ## 为什么必须重新分配（而不是照搬旧坐标）
    ///
    /// 见 [`super::atlas::requested_size_for`] 的说明：新分配器的空闲表是
    /// 全新的，不知道旧区域已被占用；照搬坐标会让后续分配覆盖既有内容。
    ///
    /// 分配顺序取**条目键的稳定顺序**（不是 `HashMap` 的迭代顺序——那是
    /// 随机的），否则搬迁结果会随哈希种子变化，破坏渲染确定性（SC-012）。
    fn reallocate_all_entries(&mut self) {
        let mut keys: Vec<GlyphCacheKey> = self.entries.keys().copied().collect();
        keys.sort_unstable();

        for key in keys {
            let Some(entry) = self.entries.get(&key) else {
                continue;
            };
            let (width, height) = (entry.rect.width, entry.rect.height);
            match self.atlas.allocate(width, height) {
                Some(region) => {
                    if let Some(entry) = self.entries.get_mut(&key) {
                        entry.rect = region;
                    }
                }
                // 新图集仍放不下 → 丢弃该条目（它会在下次访问时重新栅格化）。
                // 这不是"静默失败"：条目消失是**可观测**的（缓存条目数下降），
                // 且重建成本有界（一次 swash 调用）。
                None => {
                    self.entries.remove(&key);
                }
            }
        }

        // 搬迁后待上传队列作废（新纹理里全部内容待重建）——调用方据此
        // 重新拷一遍**全部**条目。故这里把全部条目重新入队。
        self.pending = self
            .entries
            .values()
            .map(|entry| PendingUpload {
                rect: entry.rect,
                pixels: entry.pixels.clone(),
                content: entry.content,
            })
            .collect();
    }
}

/// 一个待栅格化的字形的**身份**（字体 + 字形 + 字重）。
///
/// ## 为什么不直接用 cosmic-text 的 `LayoutGlyph`
///
/// 本模块要能被不依赖排版结果的用例驱动（`lookup_first_glyph`）。用一个小
/// 结构体让"缓存与栅格化"这条线**不依赖排版类型**，从而可独立单测。
/// 排版路径只需把 `LayoutGlyph` 的三个字段填进来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphRef {
    /// 字体 ID
    pub font_id: fontdb::ID,
    /// 字形 ID
    pub glyph_id: u16,
    /// 字重（可变字体参与栅格化）
    pub font_weight: cosmic_text::fontdb::Weight,
}

impl GlyphRef {
    /// 从 cosmic-text 的排版字形构造。
    #[must_use]
    pub fn from_layout_glyph(glyph: &cosmic_text::LayoutGlyph) -> Self {
        Self {
            font_id: glyph.font_id,
            glyph_id: glyph.glyph_id,
            font_weight: glyph.font_weight,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x_buckets_cover_the_unit_interval_without_gaps() {
        // 每一档的代表偏移都应落回该档（分档自洽）
        for bucket in [
            SubpixelBucket::Zero,
            SubpixelBucket::One,
            SubpixelBucket::Two,
            SubpixelBucket::Three,
        ] {
            assert_eq!(
                SubpixelBucket::for_x(bucket.center_px()),
                bucket,
                "桶 {bucket:?} 的中心 {:?} 应落回该桶",
                bucket.center_px()
            );
        }
    }

    #[test]
    fn x_bucket_boundaries_match_cosmic_text() {
        // 边界：0.125 是 One 的下界，0.375 是 Two，0.625 是 Three，0.875 回 Zero
        assert_eq!(SubpixelBucket::for_x(0.124), SubpixelBucket::Zero);
        assert_eq!(SubpixelBucket::for_x(0.126), SubpixelBucket::One);
        assert_eq!(SubpixelBucket::for_x(0.374), SubpixelBucket::One);
        assert_eq!(SubpixelBucket::for_x(0.376), SubpixelBucket::Two);
        assert_eq!(SubpixelBucket::for_x(0.624), SubpixelBucket::Two);
        assert_eq!(SubpixelBucket::for_x(0.626), SubpixelBucket::Three);
        assert_eq!(SubpixelBucket::for_x(0.874), SubpixelBucket::Three);
        assert_eq!(SubpixelBucket::for_x(0.876), SubpixelBucket::Zero);
    }

    #[test]
    fn negative_and_large_offsets_are_handled() {
        // -0.25 → fract 0.75 → Three；1.25 → fract 0.25 → One
        assert_eq!(SubpixelBucket::for_x(-0.25), SubpixelBucket::Three);
        assert_eq!(SubpixelBucket::for_x(1.25), SubpixelBucket::One);
        assert_eq!(SubpixelBucket::for_x(f32::NAN), SubpixelBucket::Zero);
    }

    #[test]
    fn y_always_zero_regardless_of_offset() {
        for y in [0.0f32, 0.3, 0.7, -1.4, f32::NAN] {
            assert_eq!(SubpixelBucket::for_y(y), SubpixelBucket::Zero);
        }
    }
}
