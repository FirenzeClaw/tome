//! `[WIP]` 版心帧缓存（S3 T014；`contracts/ui.md` §4.2「静态帧零重绘」）
//!
//! ## 缓存的到底是什么（路径 A 下"纹理"的确切含义）
//!
//! `plan.md` 把本文件命名为 `texture.rs`，那是**离屏读回**路径（fallback B）
//! 的形态：渲染一页到纹理、读回像素、上传给 egui。S3-0 门实证了路径 A
//! （PaintCallback 在 egui 的 device 上直绘）三平台可行，于是没有可读回的
//! 纹理——GPU 侧的画布在 egui 的 render pass 里每帧现画。
//!
//! 故本模块缓存的是**路径 A 下真正昂贵的那部分**：从排版结果构造出的
//! [`RenderBatch`]（遍历全部行盒、逐字形算四边形、决定墨色与背景）。
//! 它按 `content_version` 失效——版本没变就复用同一份 `Arc<RenderBatch>`。
//!
//! ## "零重绘"的准确口径（不说满）
//!
//! 判据是 **S2 的重算不被触发**，而不是"GPU 一个像素都不重画"：
//!
//! | 项 | 版本未变时 |
//! |---|---|
//! | 构造 `RenderBatch`（遍历行盒 × 字形） | **跳过**（缓存命中） |
//! | 字形栅格化 | **跳过**（`GlyphCache` 跨帧命中） |
//! | egui 的 render pass 里重画这一帧 | 仍然发生 |
//!
//! 最后一项是 egui 的重绘节奏决定的（它重画整个窗口），不是 S2 的重算——
//! 把它算进"重绘"会让这条判据变成"窗口永远不能重绘"，那既不可能也无意义。
//! 所以本模块提供 [`PageFrameCache::rebuild_count`] 作为**可断言的口径**：
//! 版本未变时它不增长。
//!
//! ## 为什么版本号是唯一的失效键
//!
//! `data-model.md` §2 不变量 2：`content_version` 单调递增。凡是会改变版心的
//! 事（翻页、跳章、改字号、改单双页、窗口尺寸变化）都由引擎线程产出一份新
//! 的排版结果并**递增**版本号。故"版本号相同 ⇒ 版心逐字节相同"是引擎侧的
//! 保证，缓存只需比对这一个数——不需要（也不该）去比对行盒内容。

use std::sync::Arc;

use tome_core::model::theme::Theme;
use tome_reader::paginate::config::GeometryConfig;
use tome_reader::paginate::page::ChapterLayout;
use tome_reader::paginate::spread::Viewport;
use tome_reader::render::batch::RenderBatch;

/// 一帧的缓存条目。
struct CachedFrame {
    /// 内容版本号（失效键）
    version: u64,
    /// 版心尺寸（宽, 高）——**第二失效键**
    ///
    /// ## 为什么版本号之外还要比对尺寸
    ///
    /// 版本号由引擎线程递增，而尺寸变化要经一次往返（UI → 引擎 → 事件）
    /// 才体现为新的版本号。在那之前，若窗口已被拖大，UI 拿到的批次尺寸仍是
    /// 旧的——按它分配矩形会让版心在窗口里"缩在一角"（拉大窗口后内容不跟着
    /// 变大，直到引擎的事件回来）。带上尺寸做二次校验，这一帧就按新尺寸
    /// 重算背景（字形不变），消除那一帧的视觉跳变。
    extent: (u32, u32),
    /// 已构造好的批次
    batch: Arc<RenderBatch>,
}

/// 版心帧缓存。
#[derive(Default)]
pub struct PageFrameCache {
    frame: Option<CachedFrame>,
    /// 构造批次的次数（**可断言的口径**：版本未变时它不增长）
    rebuild_count: u64,
    /// 命中次数（复用缓存）
    hit_count: u64,
}

impl PageFrameCache {
    /// 空缓存。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 取当前版本对应的批次（必要时构造）。
    ///
    /// ## 为什么用 `Arc` 而不是把批次存在这里、借出去
    ///
    /// `PaintCallback` 要求 `Clone`（`epaint` 的形状要被克隆），且回调的
    /// 生命周期跨越 egui 的一次绘制——借用是活不过去的。`Arc` 让"每帧克隆
    /// 一整页绘制指令"变成"克隆一个引用计数"。
    pub fn frame(
        &mut self,
        version: u64,
        laid_out: &ChapterLayout,
        geometry: &GeometryConfig,
        viewport: &Viewport,
        theme: Theme,
    ) -> Arc<RenderBatch> {
        let extent = (
            viewport.available_width_px.max(0.0) as u32,
            viewport.available_height_px.max(0.0) as u32,
        );

        if let Some(cached) = &self.frame {
            if cached.version == version && cached.extent == extent {
                self.hit_count += 1;
                return Arc::clone(&cached.batch);
            }
        }

        let batch = Arc::new(RenderBatch::for_line_slice(
            &laid_out.lines,
            geometry,
            viewport,
            theme,
        ));
        self.rebuild_count += 1;
        self.frame = Some(CachedFrame {
            version,
            extent,
            batch: Arc::clone(&batch),
        });
        batch
    }

    /// 当前缓存的版本号（无缓存为 `None`）。
    #[must_use]
    pub fn version(&self) -> Option<u64> {
        self.frame.as_ref().map(|cached| cached.version)
    }

    /// 构造批次的累计次数。
    #[must_use]
    pub fn rebuild_count(&self) -> u64 {
        self.rebuild_count
    }

    /// 缓存命中的累计次数。
    #[must_use]
    pub fn hit_count(&self) -> u64 {
        self.hit_count
    }

    /// 清空（换书 / 关闭阅读视图时调用）。
    ///
    /// ## 为什么换书必须清
    ///
    /// `content_version` 是**会话内**单调的，跨书没有可比性（新书的版本号
    /// 会从头开始）。不清的话，打开一本新书若恰好拿到相同的版本号，界面会
    /// 显示**上一本书**的版心——这是最糟糕的一类缺陷（看起来像渲染错乱，
    /// 实则是一个忘了失效的缓存）。
    pub fn clear(&mut self) {
        self.frame = None;
    }
}
