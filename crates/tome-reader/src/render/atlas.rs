//! [DONE] 字形图集（T033 / research R2 第 2、4 条）
//!
//! ## 这个模块管什么
//!
//! 一张（或几张）GPU 纹理的**区域分配**：字形位图往哪放、放不放得下、
//! 放不下时怎么长。位图本身的内容由 [`super::glyph_cache`] 供给。
//!
//! ## 为什么用 etagere 的 `BucketedAtlasAllocator`
//!
//! research R2 的选型：glyphon 同款。理由是这个分配器把**尺寸相近的分配**
//! 归到同一个桶与 shelf，从而让"字形位图普遍是十几到几十像素的小矩形"这一
//! 分布得到高填充率——按面积排序的自由矩形装箱反而因碎片化更差。
//!
//! ## 增长策略：`copy_texture_to_texture` 整体搬迁（不是全量重栅格化）
//!
//! R2 第 4 条点名的**与 glyphon 的关键差异**。glyphon 在 atlas 满时把所有
//! 缓存条目重新栅格化一遍再写进新纹理——那会让"翻到第 N 页时突然卡一下"，
//! 且栅格化成本随已缓存字形数线性增长。
//!
//! 搬迁不需要重新栅格化：**位图还在 CPU 侧**（`GlyphCache` 持有），故只需
//! `copy_texture_to_texture` 把旧纹理整体拷到新纹理，再更新各条目的坐标。
//! 代价是一次 GPU 拷贝（几十微秒量级）而非几百次 CPU 光栅化。
//!
//! ## 为什么"搬迁后旧条目仍可用"必须有测试
//!
//! 搬迁的实现风险在**簿记**：搬完之后每个条目的 `rect` 必须仍指向有效区域。
//! 漏更新会表现为"翻页后先前页的字形变成空白或错位"——症状出现在**别的
//! 页面**上，极难从现象反推。故 `tests/glyph_cache.rs` 专门断言它。

use etagere::{size2, BucketedAtlasAllocator, Size};

/// 初始图集边长（px）。
///
/// ## 取值依据
///
/// 一页正文按 19px 字号、约 1000 字、平均 20×20px 位图估算约 400 KB；
/// 1024² 的 R8 纹理是 1 MB，足够容纳一页并留余量给相邻页的预取。
/// 取 2048 起步会让首帧分配一张 4 MB 纹理，而多数页面用不到它的 1/4——
/// 那是"用内存换零增长"的取舍，对本层（<20MB 体积目标）不利。
pub const INITIAL_ATLAS_SIZE: u32 = 1024;

/// 图集最大边长（px）。
///
/// ## 为什么要有上限
///
/// 无上限的增长在极端场景（用户连续翻几百页、每页都含大量生僻字）下会
/// 吃掉全部显存并使分配失败。到达上限时的**正确行为是淘汰**（LRU）而
/// 不是"继续长到 OOM"——淘汰会让少数页重新栅格化，但那是**有界的代价**。
///
/// 4096² 的 R8 是 16 MB，已是单个桌面阅读器可接受的显存上限。
pub const MAX_ATLAS_SIZE: u32 = 4096;

/// 图集增长的结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrowOutcome {
    /// 成长到新边长。旧纹理须搬迁到新纹理。
    Grew {
        /// 新边长（px）
        new_size: u32,
    },
    /// 已达上限，无法增长（调用方应改为淘汰）。
    AtCapacity,
}

/// 字形图集（区域分配 + 增长簿记）。
///
/// ## 为什么不持有 wgpu 纹理
///
/// 纹理的持有者是 [`super::glyph_cache`]（它同时持有 CPU 位图与 GPU 纹理，
/// 两者必须同步变更）。本模块只管**区域分配**——那是纯几何计算，可以
/// 完全脱离 GPU 单测（`allocate` 的边界、增长阈值、上限都无需设备）。
pub struct GlyphAtlas {
    size: u32,
    allocator: BucketedAtlasAllocator,
    grow_count: u32,
}

impl std::fmt::Debug for GlyphAtlas {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GlyphAtlas")
            .field("size", &self.size)
            .field("grow_count", &self.grow_count)
            .field("free_space", &self.allocator.free_space())
            .finish()
    }
}

impl Default for GlyphAtlas {
    fn default() -> Self {
        Self::new()
    }
}

impl GlyphAtlas {
    /// 新建图集（初始边长 [`INITIAL_ATLAS_SIZE`]）。
    #[must_use]
    pub fn new() -> Self {
        Self::with_size(INITIAL_ATLAS_SIZE)
    }

    /// 以指定边长新建（测试用：小尺寸可快速触发增长）。
    #[must_use]
    pub fn with_size(size: u32) -> Self {
        Self {
            size,
            allocator: BucketedAtlasAllocator::new(size2(size as i32, size as i32)),
            grow_count: 0,
        }
    }

    /// 当前边长（px）——纹理是正方形，故单值够用。
    #[must_use]
    pub fn size(&self) -> u32 {
        self.size
    }

    /// 已发生的增长次数。
    #[must_use]
    pub fn grow_count(&self) -> u32 {
        self.grow_count
    }

    /// 剩余可用面积（px²）。
    #[must_use]
    pub fn free_space(&self) -> i32 {
        self.allocator.free_space()
    }

    /// 分配一块区域；放不下时返回 `None`（调用方据此触发增长）。
    ///
    /// ## 为什么返回 `None` 而不内部自动增长
    ///
    /// 增长需要**搬迁**（拷纹理 + 更新全部条目坐标），那是 `GlyphCache` 的
    /// 职责（它同时持有纹理与条目表）。本模块若自行增长，调用方拿到的
    /// `Allocation` 会指向一个刚被作废的坐标系。
    pub fn allocate(&mut self, width: u32, height: u32) -> Option<AtlasRegion> {
        if width == 0 || height == 0 {
            return None;
        }
        // 单个分配超过整张图集 → 直接放弃（增长也放不下，除非超过上限）
        let allocation = self
            .allocator
            .allocate(size2(width as i32, height as i32))?;
        Some(AtlasRegion {
            x: allocation.rectangle.min.x as u32,
            y: allocation.rectangle.min.y as u32,
            width,
            height,
        })
    }

    /// 请求增长到下一档。
    ///
    /// ## 为什么翻倍而不是按需算最小尺寸
    ///
    /// "恰好够这次分配"的尺寸会让下一次分配立刻再次触发增长——每次增长都
    /// 要一次完整纹理搬迁，连续小步增长会把搬迁成本放大成 O(n²)。
    /// 翻倍把增长次数压到 O(log n)，代价是最后一张纹理最多空一半
    /// （2048² 空一半 = 2 MB，可接受）。
    pub fn request_grow(&mut self) -> GrowOutcome {
        if self.size >= MAX_ATLAS_SIZE {
            return GrowOutcome::AtCapacity;
        }
        let new_size = (self.size * 2).min(MAX_ATLAS_SIZE);
        self.size = new_size;
        self.grow_count += 1;
        // 分配器重建：旧区域分配在搬迁后按新尺寸**重新分配**（见 GlyphCache）
        self.allocator = BucketedAtlasAllocator::new(size2(new_size as i32, new_size as i32));
        GrowOutcome::Grew { new_size }
    }

    /// 清空全部分配（重建到初始尺寸）。
    pub fn clear(&mut self) {
        self.size = INITIAL_ATLAS_SIZE;
        self.allocator = BucketedAtlasAllocator::new(size2(self.size as i32, self.size as i32));
    }

    /// 图集尺寸是否仍是一致的正方形（不变量的廉价检查）。
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.size > 0 && self.size.is_power_of_two()
    }
}

/// 图集里的一块矩形区域（左上角 + 尺寸）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasRegion {
    /// 左边缘（px）
    pub x: u32,
    /// 上边缘（px）
    pub y: u32,
    /// 宽（px）
    pub width: u32,
    /// 高（px）
    pub height: u32,
}

impl AtlasRegion {
    /// 右边缘（px）。
    #[must_use]
    pub fn right(&self) -> u32 {
        self.x + self.width
    }

    /// 下边缘（px）。
    #[must_use]
    pub fn bottom(&self) -> u32 {
        self.y + self.height
    }

    /// 像素数。
    #[must_use]
    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    /// 该区域是否完全落在给定边长的纹理内。
    #[must_use]
    pub fn fits_within(&self, atlas_size: u32) -> bool {
        self.right() <= atlas_size && self.bottom() <= atlas_size
    }

    /// 归一化 UV 范围（`[u0, v0, u1, v1]`），供着色器采样。
    #[must_use]
    pub fn uv(&self, atlas_size: u32) -> [f32; 4] {
        let size = atlas_size.max(1) as f32;
        [
            self.x as f32 / size,
            self.y as f32 / size,
            self.right() as f32 / size,
            self.bottom() as f32 / size,
        ]
    }
}

/// 计算一块 `width × height` 的区域在两个图集尺寸之间的搬迁去向。
///
/// ## 为什么搬迁时**重新分配**而不是"原坐标照搬"
///
/// 原坐标照搬在新纹理里也是合法的（尺寸只增不减），但会让新纹理的分配
/// 呈现"集中在左上角、右侧大片空闲"的形态，而分配器的空闲表是全新的——
/// 它不知道那些区域已被隐式占用，下一次分配会把新字形放进旧区域**中间**，
/// 覆盖已有内容。
///
/// 故搬迁的正确做法是：新分配器 + 按旧条目顺序**重新分配**同尺寸区域。
/// 由于分配顺序一致且分配器确定，新区域通常与旧的接近，但**必须按返回值
/// 更新条目**——这正是"搬迁后旧条目仍可用"的判据所测的。
#[must_use]
pub fn requested_size_for(width: u32, height: u32) -> Size {
    size2(width.max(1) as i32, height.max(1) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_atlas_is_consistent_and_empty() {
        let atlas = GlyphAtlas::new();
        assert_eq!(atlas.size(), INITIAL_ATLAS_SIZE);
        assert_eq!(atlas.grow_count(), 0);
        assert!(atlas.is_consistent());
        assert_eq!(
            atlas.free_space(),
            (INITIAL_ATLAS_SIZE * INITIAL_ATLAS_SIZE) as i32
        );
    }

    #[test]
    fn allocation_returns_region_inside_the_atlas() {
        let mut atlas = GlyphAtlas::new();
        let region = atlas.allocate(20, 24).expect("首次分配应成功");
        assert_eq!(region.width, 20);
        assert_eq!(region.height, 24);
        assert!(
            region.fits_within(atlas.size()),
            "分配的区域必须落在图集内：{region:?} vs {}",
            atlas.size()
        );
    }

    #[test]
    fn zero_sized_allocation_is_rejected() {
        let mut atlas = GlyphAtlas::new();
        assert!(atlas.allocate(0, 10).is_none(), "零宽应被拒");
        assert!(atlas.allocate(10, 0).is_none(), "零高应被拒");
    }

    /// 填满后分配失败（**判据必须能实际失败**）。
    #[test]
    fn allocation_fails_when_full() {
        let mut atlas = GlyphAtlas::with_size(64);
        let mut allocated = 0usize;
        // 16×16 的块在 64² 里最多 16 个
        while atlas.allocate(16, 16).is_some() {
            allocated += 1;
            assert!(allocated < 1000, "分配应终止（防死循环）");
        }
        assert!(
            allocated >= 4,
            "64² 至少应能放下 4 个 16×16 的块，实测 {allocated}"
        );
    }

    #[test]
    fn grow_doubles_until_capacity_then_reports() {
        let mut atlas = GlyphAtlas::with_size(64);
        assert_eq!(atlas.request_grow(), GrowOutcome::Grew { new_size: 128 });
        assert_eq!(atlas.size(), 128);
        assert_eq!(atlas.grow_count(), 1);

        // 一路长到上限
        while let GrowOutcome::Grew { .. } = atlas.request_grow() {}
        assert_eq!(atlas.size(), MAX_ATLAS_SIZE);
        assert_eq!(
            atlas.request_grow(),
            GrowOutcome::AtCapacity,
            "到上限后应报 AtCapacity（调用方据此改走淘汰）"
        );
    }

    #[test]
    fn grow_never_exceeds_capacity() {
        let mut atlas = GlyphAtlas::with_size(MAX_ATLAS_SIZE);
        assert_eq!(atlas.request_grow(), GrowOutcome::AtCapacity);
        assert_eq!(atlas.size(), MAX_ATLAS_SIZE);
    }

    /// 增长后**新尺寸下能分配更大的区域**（增长真的有效，不是只改了个数）。
    #[test]
    fn grown_atlas_accepts_larger_allocations() {
        let mut atlas = GlyphAtlas::with_size(64);
        // 64² 放不下 60×60 之外的东西——先填满
        let _ = atlas.allocate(64, 64);
        assert!(atlas.allocate(64, 64).is_none(), "填满后应分配失败");

        atlas.request_grow();
        assert_eq!(atlas.size(), 128);
        assert!(
            atlas.allocate(64, 64).is_some(),
            "增长到 128² 后应能再放一个 64×64"
        );
    }

    #[test]
    fn region_geometry_helpers_are_consistent() {
        let region = AtlasRegion {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        assert_eq!(region.right(), 40);
        assert_eq!(region.bottom(), 60);
        assert_eq!(region.pixel_count(), 1200);
        assert!(region.fits_within(64));
        assert!(!region.fits_within(32), "区域右下角 40×60 超出 32²");

        let uv = region.uv(100);
        assert!((uv[0] - 0.1).abs() < 1e-6);
        assert!((uv[1] - 0.2).abs() < 1e-6);
        assert!((uv[2] - 0.4).abs() < 1e-6);
        assert!((uv[3] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn clear_resets_to_initial_size() {
        let mut atlas = GlyphAtlas::with_size(64);
        atlas.request_grow();
        assert_eq!(atlas.size(), 128);
        atlas.clear();
        assert_eq!(atlas.size(), INITIAL_ATLAS_SIZE);
    }
}
