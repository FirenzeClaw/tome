//! `[DONE]` 离屏渲染 + 读回（T035 / FR-053；research R4）
//!
//! ## 这条路径要做什么
//!
//! ```text
//!   建纹理（Rgba8Unorm, RENDER_ATTACHMENT | COPY_SRC）
//!     ↓ 渲染通道：清底 + 画字形四边形
//!     ↓ copy_texture_to_buffer（bytes_per_row 对齐 256）
//!     ↓ map_async + device.poll(wait)
//!   紧凑 RGBA 像素（去掉行 padding）
//! ```
//!
//! ## R4 点名的两个坑（都在本模块处理并有单测）
//!
//! 1. **`bytes_per_row` 必须 256 对齐**，读回后要去 padding。不对齐时 wgpu
//!    直接报校验错，而错误信息不会告诉你正确值是多少。
//! 2. **`map_async` 之后必须 `device.poll(wait)`**，否则映射永不完成——
//!    表现为挂起而非报错。
//!
//! ## 为什么用 `Rgba8Unorm` 而非 `Rgba8UnormSrgb`（R4 的明确选择）
//!
//! sRGB 格式会让 GPU 在写纹理时自动做线性→sRGB 变换，于是"我传进去的 0.5"
//! 读回来不是 128。像素快照要的是**可复现的字节**，故走非 sRGB 格式，
//! gamma 处理留给上层的色彩管理（本层不做）。
//!
//! ## 诚实跳过（FR-053）
//!
//! 无后端时返回能力错误而非"一张全黑图"。全黑图是最坏的伪装形态：它看起来
//! 像"渲染成功了但内容为空"，而上层会把它当有效快照存成基线。

use crate::error::ReaderError;
use crate::render::batch::RenderBatch;
use crate::render::glyph_cache::GlyphCache;
use crate::render::pipeline::{PipelineError, RenderPipeline};

/// 离屏目标纹理的格式（R4 指定，见模块文档）。
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// 一张读回的图像。
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedImage {
    /// 宽（px）
    pub width: u32,
    /// 高（px）
    pub height: u32,
    /// **紧凑** RGBA 像素（每行无 padding，长度 = `width × height × 4`）
    pub pixels: Vec<u8>,
}

impl RenderedImage {
    /// 非黑像素数（"缓冲里有东西"的判据）。
    ///
    /// ## 为什么这是离屏路径的**必要但不充分**判据
    ///
    /// 未真正渲染的纹理读回的是**全零**——那是合法的缓冲区内容，长度断言
    /// 会通过。只有看像素值才能发现"渲染没发生"（spike T009 的教训）。
    ///
    /// 但它**不充分**：昼主题的底色是米黄（`#F5EEDC`），本身就不是黑，
    /// 故"背景清色成功"会让本计数等于全图。要判"前景真的画上去了"须用
    /// [`Self::foreground_pixels`]（与背景色比较）。
    #[must_use]
    pub fn non_black_pixels(&self) -> usize {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0)
            .count()
    }

    /// 与给定背景色**不同**的像素数（"前景真的画上去了吗"的判据）。
    ///
    /// ## 为什么必须与背景比较而不是用"非黑"
    ///
    /// 昼主题的阅读区底色是米黄——"非黑"会把整张图都数进来，于是
    /// "前景覆盖率"恒为 100%，判据完全失效（实测：初版用非黑口径时
    /// 一个**只清底、没画任何字形**的实现也能通过）。
    #[must_use]
    pub fn foreground_pixels(&self, background: [u8; 3]) -> usize {
        self.pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| {
                pixel[0] != background[0] || pixel[1] != background[1] || pixel[2] != background[2]
            })
            .count()
    }

    /// 与背景不同的像素占比（前景覆盖率）。
    #[must_use]
    pub fn foreground_coverage(&self, background: [u8; 3]) -> f64 {
        if self.total_pixels() == 0 {
            return 0.0;
        }
        self.foreground_pixels(background) as f64 / self.total_pixels() as f64
    }

    /// 总像素数。
    #[must_use]
    pub fn total_pixels(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// 非黑像素占比。
    #[must_use]
    pub fn coverage(&self) -> f64 {
        if self.total_pixels() == 0 {
            return 0.0;
        }
        self.non_black_pixels() as f64 / self.total_pixels() as f64
    }

    /// 像素数是否与尺寸自洽（廉价不变量检查）。
    #[must_use]
    pub fn is_consistent(&self) -> bool {
        self.pixels.len() == (self.width as usize) * (self.height as usize) * 4
    }
}

/// 计算 `bytes_per_row` 的 256 字节对齐值（wgpu 的硬要求）。
///
/// 与 spike 的同名函数**逐字一致**——那处已在 T009 实测可用
/// （`spikes/s2/src/offscreen_probe.rs`）。
#[must_use]
pub fn aligned_bytes_per_row(width: u32, bytes_per_pixel: u32) -> u32 {
    const COPY_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded = width.saturating_mul(bytes_per_pixel);
    unpadded.div_ceil(COPY_ALIGNMENT) * COPY_ALIGNMENT
}

/// 从 256 对齐的读回缓冲里剥掉每行 padding，得到紧凑像素。
///
/// 返回 `(紧凑数据, 每行 padding 字节数)`。
#[must_use]
pub fn strip_row_padding(
    padded: &[u8],
    width: u32,
    bytes_per_pixel: u32,
    bytes_per_row: u32,
) -> (Vec<u8>, u32) {
    let tight_row = width.saturating_mul(bytes_per_pixel) as usize;
    if bytes_per_row == 0 {
        return (Vec::new(), 0);
    }
    let padding = (bytes_per_row as usize).saturating_sub(tight_row);
    let mut tight = Vec::with_capacity(tight_row * (padded.len() / bytes_per_row as usize));
    for row in padded.chunks_exact(bytes_per_row as usize) {
        tight.extend_from_slice(&row[..tight_row]);
    }
    (tight, padding as u32)
}

/// 离屏渲染器（持有一个批次 + 一台管线，产出图像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffscreenRenderer {
    width: u32,
    height: u32,
}

impl OffscreenRenderer {
    /// 以给定尺寸新建。
    ///
    /// ## 尺寸校验
    ///
    /// 零尺寸在 wgpu 里是校验错误（报的是纹理描述符的内部字段）。此处先拒，
    /// 给出指向参数的诊断（与 [`RenderPipeline::new`] 同口径）。
    pub fn new(width: u32, height: u32) -> Result<Self, PipelineError> {
        if width == 0 || height == 0 {
            return Err(PipelineError::InvalidExtent {
                width,
                height,
                detail: "width/height must be positive",
            });
        }
        Ok(Self { width, height })
    }

    /// 输出宽。
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 输出高。
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 渲染一个批次并读回像素。
    ///
    /// ## 字形绘制为什么是"实心矩形"而不是真实位图
    ///
    /// 真实位图渲染需要把 atlas 纹理绑到着色器上（T033 已把位图备好，
    /// 但把整条管线接起来属 S2-5 的绘制细化）。本任务（T035）的验收子项是
    /// **"读回像素非全零"**与"后端不可用报能力错误"——即离屏**链路**可用。
    /// 用实心矩形画字形四边形足以验证链路，且它让"渲染真的发生了"这件事
    /// 有一个确定性的、与字体无关的判据（跨平台一致，不受系统字体漂移影响）。
    ///
    /// 这一步的产物（几何与颜色的可复现光栅化）正是像素快照（T036）需要的
    /// 输入：SC-011 的基线比对要的是**我们自己的绘制结果**稳定，不是
    /// "系统字体渲染出来长什么样"。
    pub fn render(
        &self,
        pipeline: &RenderPipeline,
        batch: &RenderBatch,
        cache: &mut GlyphCache,
    ) -> Result<RenderedImage, PipelineError> {
        // 走一遍编码（它负责字形缓存的查/栅格化，也就是 SC-012 的"预热"）
        let _instructions = batch.encode(pipeline, cache);

        let device = pipeline.device();
        let queue = pipeline.queue();

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tome-offscreen-target"),
            size: wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // R4 指定：非 sRGB（见模块文档）
            format: OFFSCREEN_FORMAT,
            // `COPY_DST` 是 `write_texture` 的必要条件（字形覆盖走该路径）；
            // `COPY_SRC` 供读回。三者一起构成"离屏可写可读"的最小集合。
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let background = batch.background();
        let clear = wgpu::Color {
            r: f64::from(background.color.red) / 255.0,
            g: f64::from(background.color.green) / 255.0,
            b: f64::from(background.color.blue) / 255.0,
            a: 1.0,
        };

        // --- 渲染通道：清底 + 画字形（与共享 device 路径**同一份**绘制代码）---
        //
        // 这一段此前是本模块的私有实现；S3 T004 把它抽到
        // [`super::glyph_atlas::GlyphRenderer`]，因为 egui PaintCallback 要在
        // **别人的** render pass 上画同一批字形。两条路径的判据是逐像素一致
        // （`tests/shared_device.rs`），共用一份代码是它成立的前提。
        let glyphs = super::glyph_atlas::GlyphRenderer::prepare(
            device,
            queue,
            OFFSCREEN_FORMAT,
            batch,
            cache,
        )?;

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("tome-offscreen-encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("tome-offscreen-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(glyphs) = &glyphs {
                glyphs.draw(&mut pass);
            }
        }
        queue.submit(Some(encoder.finish()));

        // --- 读回 ---
        read_texture_rgba(device, queue, &texture, self.width, self.height)
    }
}

/// 把一张 RGBA 纹理读回成紧凑像素（256 对齐 → 剥 padding）。
///
/// ## 为什么单独成函数（S3 T004）
///
/// 读回这段（建缓冲 → `copy_texture_to_buffer` → `map_async` + `poll` →
/// 剥 padding）与"画什么"无关，S3 的 `tests/shared_device.rs` 也要用它比对
/// 两条绘制路径的像素。留成 `OffscreenRenderer::render` 的内联段就只能复制一份。
///
/// ## R4 的两个坑（都在本函数里处理）
///
/// 1. `bytes_per_row` 必须 256 对齐（不对齐时 wgpu 直接报校验错）
/// 2. `map_async` 之后必须 `device.poll(wait)`，否则映射永不完成（表现为挂起）
pub fn read_texture_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<RenderedImage, PipelineError> {
    {
        let bytes_per_pixel = 4u32;
        let bytes_per_row = aligned_bytes_per_row(width, bytes_per_pixel);
        let buffer_size = u64::from(bytes_per_row) * u64::from(height);

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tome-offscreen-readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // 读回需要自己的编码器（上面那些已经 submit 了）
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("tome-offscreen-readback-encoder"),
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        queue.submit(Some(encoder.finish()));

        // map_async + poll（R4 的第 2 号坑：不 poll 映射永不完成）
        let slice = buffer.slice(..);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            // 发送失败只意味着接收端已放弃等待，不是错误
            let _ = sender.send(result);
        });

        if let Err(error) = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        }) {
            return Err(PipelineError::Readback {
                detail: format!("device.poll failed: {error}"),
            });
        }

        match receiver.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                return Err(PipelineError::Readback {
                    detail: format!("map_async failed: {error}"),
                });
            }
            Err(error) => {
                return Err(PipelineError::Readback {
                    detail: format!("map_async callback not delivered: {error}"),
                });
            }
        }

        let mapped = slice
            .get_mapped_range()
            .map_err(|error| PipelineError::Readback {
                detail: format!("get_mapped_range failed: {error}"),
            })?;
        let (tight, _padding) = strip_row_padding(&mapped, width, bytes_per_pixel, bytes_per_row);
        drop(mapped);
        buffer.unmap();

        Ok(RenderedImage {
            width,
            height,
            pixels: tight,
        })
    }
}

/// 离屏渲染失败时的**能力错误**映射（FR-053/FR-057）。
#[must_use]
pub fn readback_failure_error(error: &PipelineError) -> ReaderError {
    error.to_reader_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_per_row_is_256_aligned() {
        assert_eq!(aligned_bytes_per_row(32, 4), 256);
        assert_eq!(aligned_bytes_per_row(64, 4), 256);
        assert_eq!(aligned_bytes_per_row(65, 4), 512);
        assert_eq!(aligned_bytes_per_row(1, 4), 256);

        // 属性：任何宽度下结果都是 256 的倍数且 ≥ 紧凑行长
        for width in 1..500u32 {
            let aligned = aligned_bytes_per_row(width, 4);
            assert_eq!(aligned % 256, 0, "宽度 {width} 的对齐结果未对齐");
            assert!(aligned >= width * 4, "宽度 {width} 的对齐结果小于紧凑行长");
        }
    }

    #[test]
    fn row_padding_is_stripped() {
        let bytes_per_row = aligned_bytes_per_row(8, 4);
        assert_eq!(bytes_per_row, 256);

        let mut padded = vec![0u8; (bytes_per_row * 2) as usize];
        for byte in &mut padded[..bytes_per_row as usize] {
            *byte = 0xAA;
        }
        for byte in &mut padded[bytes_per_row as usize..] {
            *byte = 0xBB;
        }

        let (tight, padding) = strip_row_padding(&padded, 8, 4, bytes_per_row);
        assert_eq!(padding, 256 - 32);
        assert_eq!(tight.len(), 8 * 4 * 2, "紧凑数据长度应为 2 行 × 32 字节");
        assert!(tight[..32].iter().all(|byte| *byte == 0xAA));
        assert!(tight[32..].iter().all(|byte| *byte == 0xBB));
    }

    #[test]
    fn zero_width_strip_is_safe() {
        let (tight, padding) = strip_row_padding(&[], 0, 4, 256);
        assert!(tight.is_empty());
        assert_eq!(padding, 256);
    }

    #[test]
    fn zero_sized_renderer_is_rejected() {
        assert!(OffscreenRenderer::new(0, 100).is_err());
        assert!(OffscreenRenderer::new(100, 0).is_err());
        assert!(OffscreenRenderer::new(100, 100).is_ok());
    }

    #[test]
    fn non_black_count_detects_all_zero_buffer() {
        let image = RenderedImage {
            width: 4,
            height: 4,
            pixels: vec![0u8; 4 * 4 * 4],
        };
        assert_eq!(image.non_black_pixels(), 0);
        assert_eq!(image.coverage(), 0.0);
        assert!(image.is_consistent());
    }

    #[test]
    fn coverage_counts_painted_pixels() {
        let mut pixels = vec![0u8; 4 * 4 * 4];
        // 涂 4 个像素（每像素 4 字节）
        for index in 0..4 {
            pixels[index * 4] = 255;
        }
        let image = RenderedImage {
            width: 4,
            height: 4,
            pixels,
        };
        assert_eq!(image.non_black_pixels(), 4);
        assert!((image.coverage() - 0.25).abs() < 1e-9);
    }

    /// 尺寸与像素数不一致时 `is_consistent` 报 false（判据能失败）。
    #[test]
    fn inconsistent_image_is_detected() {
        let image = RenderedImage {
            width: 4,
            height: 4,
            pixels: vec![0u8; 10],
        };
        assert!(!image.is_consistent());
    }
}
