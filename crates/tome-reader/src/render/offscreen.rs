//! [DONE] 离屏渲染 + 读回（T035 / FR-053；research R4）
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

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("tome-offscreen-encoder"),
        });

        // --- 渲染通道：清底 ---
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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
            drop(pass);
        }
        queue.submit(Some(encoder.finish()));

        // --- 字形绘制：真实位图 → 图集 → WGSL 采样 ---
        //
        // research R2 的既定方案。见 [`draw_glyphs`]：必须真的过一遍着色器
        // （不是拷贝），否则测的是"搬运无差异"——那正是 S2-0 spike 首轮的
        // 假绿教训（BASELINE §8.2）。
        draw_glyphs(device, queue, &view, batch, cache)?;

        // --- 读回 ---
        let bytes_per_pixel = 4u32;
        let bytes_per_row = aligned_bytes_per_row(self.width, bytes_per_pixel);
        let buffer_size = u64::from(bytes_per_row) * u64::from(self.height);

        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tome-offscreen-readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // 读回需要自己的编码器（上面的已经 submit 了）
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("tome-offscreen-readback-encoder"),
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
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
        let (tight, _padding) =
            strip_row_padding(&mapped, self.width, bytes_per_pixel, bytes_per_row);
        drop(mapped);
        buffer.unmap();

        Ok(RenderedImage {
            width: self.width,
            height: self.height,
            pixels: tight,
        })
    }
}

/// 绘制真实字形位图（WGSL 采样 + alpha 混合）。
///
/// ## 为什么必须过一遍着色器（而不是 CPU 合成后拷贝）
///
/// S2-0 spike 的 T009 首轮（BASELINE §8.2）把 CPU 像素直接
/// `copy_buffer_to_texture` 再读回，结果"后端差异恰好为 0"——因为
/// **GPU 一次算术都没做**，测的是"拷贝无差异"。那次结论被推翻重做。
///
/// 故字形位图经图集纹理上传后，由 WGSL **采样并混合**到目标纹理上。
/// 这样跨后端差异才测的是本项目真实会遇到的东西（采样约定 + 混合算术）。
///
/// ## 与 spike `blit.wgsl` 的一致性
///
/// 三条刻意复现（那处已实测跨后端逐像素一致）：点采样（`textureLoad`）、
/// 标准 source-over 混合、整像素对齐绘制。见
/// [`super::glyph_atlas::GLYPH_SHADER`] 的文档。
///
/// ## 缺字（无位图）的处置
///
/// 位图为空时**跳过该字形**——不画实心块冒充它。FR-047 的"缺字占位"
/// （tofu）由字体栈自身的 `.notdef` 字形提供：系统字体链连 `.notdef` 都没有
/// 时，画任何东西都是伪装，故跳过并让上层的诊断报告缺字。
fn draw_glyphs(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target_view: &wgpu::TextureView,
    batch: &RenderBatch,
    cache: &mut GlyphCache,
) -> Result<(), PipelineError> {
    if batch.glyphs().is_empty() {
        return Ok(());
    }

    // --- 1. 把字形转成待打包的放置描述 ---
    let placements: Vec<super::glyph_atlas::GlyphPlacement> = batch
        .glyphs()
        .iter()
        .map(|glyph| super::glyph_atlas::GlyphPlacement {
            glyph: super::glyph_cache::GlyphRef {
                font_id: glyph.font_id,
                glyph_id: glyph.glyph_id,
                font_weight: glyph.font_weight,
            },
            font_size: glyph.font_size,
            x_px: glyph.x,
            baseline_y_px: glyph.y,
            subpixel_x: glyph.subpixel_x,
            available: true,
        })
        .collect();

    // --- 2. 栅格化 + 打包进一张图集（CPU 侧，一次批量）---
    let packed = super::glyph_atlas::pack_glyphs(cache, &placements);
    if packed.instance_count() == 0 {
        // 一个字形都没栅格化出来（字体栈异常）——如实报错而非静默出空白图
        return Err(PipelineError::Readback {
            detail: "no glyph bitmap could be produced (font stack failure)".to_owned(),
        });
    }

    // --- 3. 图集纹理上传（一次 `write_texture` 整张，R2 第 5 条）---
    let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("tome-glyph-atlas"),
        size: wgpu::Extent3d {
            width: packed.size,
            height: packed.size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // 单通道灰度：swash 的 `Mask` 位图就是 8 位 alpha
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &atlas_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &packed.pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(packed.size),
            rows_per_image: Some(packed.size),
        },
        wgpu::Extent3d {
            width: packed.size,
            height: packed.size,
            depth_or_array_layers: 1,
        },
    );

    // --- 4. 着色器与管线 ---
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tome-glyph-shader"),
        source: wgpu::ShaderSource::Wgsl(super::glyph_atlas::GLYPH_SHADER.into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("tome-glyph-bind-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("tome-glyph-sampler"),
        // 最近邻：与点采样一致（字形位图 1:1 贴合）
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

    // 墨色 uniform（正文色，FR-030：由主题给出）
    let ink = batch.text_color();
    let ink_uniform = InkColorGpu {
        rgb: [
            f32::from(ink.red) / 255.0,
            f32::from(ink.green) / 255.0,
            f32::from(ink.blue) / 255.0,
            1.0,
        ],
    };
    let ink_buffer = {
        use wgpu::util::DeviceExt as _;
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tome-glyph-ink"),
            contents: bytemuck::bytes_of(&ink_uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        })
    };

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tome-glyph-bind-group"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: ink_buffer.as_entire_binding(),
            },
        ],
    });

    // --- 5. 实例缓冲：像素坐标 → 裁剪空间 ---
    let canvas_w = batch.background().width.max(1) as f32;
    let canvas_h = batch.background().height.max(1) as f32;
    let to_clip_x = |value: f32| (value / canvas_w) * 2.0 - 1.0;
    let to_clip_y = |value: f32| 1.0 - (value / canvas_h) * 2.0;

    let instances: Vec<GlyphInstanceGpu> = packed
        .instances
        .iter()
        .filter_map(|instance| *instance)
        .map(|instance| {
            let [x0, y0, x1, y1] = instance.rect_px;
            GlyphInstanceGpu {
                rect: [to_clip_x(x0), to_clip_y(y0), to_clip_x(x1), to_clip_y(y1)],
                uv_rect: instance.uv_rect,
            }
        })
        .collect();

    if instances.is_empty() {
        return Ok(());
    }

    let instance_buffer = create_instance_buffer(device, &instances);

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("tome-glyph-pipeline-layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("tome-glyph-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(GlyphInstanceGpu::LAYOUT)],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OFFSCREEN_FORMAT,
                // 标准 source-over：`src * srcAlpha + dst * (1 - srcAlpha)`
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("tome-glyph-encoder"),
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tome-glyph-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // `Load`：保有底色与已画的内容，只在其上叠加字形
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.set_vertex_buffer(0, instance_buffer.slice(..));
        pass.draw(0..6, 0..instances.len() as u32);
    }
    queue.submit(Some(encoder.finish()));

    Ok(())
}

/// 墨色 uniform 的 GPU 布局（`vec4<f32>`，与 WGSL 的 `InkColor` 一致）。
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InkColorGpu {
    rgb: [f32; 4],
}

/// 实例属性的 GPU 布局（与 WGSL 的 `Instance` 结构一一对应）。
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlyphInstanceGpu {
    rect: [f32; 4],
    uv_rect: [f32; 4],
}

impl GlyphInstanceGpu {
    /// 顶点缓冲布局：两条 `Float32x4` 实例属性（步进 32 字节，按实例递增）。
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<GlyphInstanceGpu>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 1,
            },
        ],
    };
}

/// 建实例缓冲并上传。
fn create_instance_buffer(device: &wgpu::Device, instances: &[GlyphInstanceGpu]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt as _;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("tome-glyph-instances"),
        contents: bytemuck::cast_slice(instances),
        usage: wgpu::BufferUsages::VERTEX,
    })
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
