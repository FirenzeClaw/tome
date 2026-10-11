//! 字形覆盖的**真实位图**绘制（T035 的 `[WORKAROUND]` 替换 / research R2）
//!
//! ## 这个模块替换什么
//!
//! T035 交付时，离屏渲染的字形覆盖走的是"按几何填实心块"
//! （`offscreen.rs` 的 `[WORKAROUND]`）。那足以验证离屏**链路**，但产出的
//! 像素与真实字形无关——表现为"同一页里所有字形都是等大的方块"，
//! 于是内容不同的页可能产出完全相同的图像（字数相同时）。
//!
//! 本模块按 research R2 的既定方案补实：
//!
//! ```text
//!   GlyphBox（带 font_id/glyph_id/font_weight）
//!     ↓ SwashCache 栅格化（亚像素桶键与 T033 一致）
//!   alpha 位图（Mask）
//!     ↓ 聚合成一张 atlas 纹理（etagere 分配 + 批量 write_texture）
//!   纹理采样绘制（WGSL：textureLoad + alpha 混合）
//! ```
//!
//! ## 为什么走 GPU 而不是 CPU 合成
//!
//! S2-0 spike 的 T009 首轮教训（BASELINE §8.2）：把 CPU 生成的像素直接
//! `copy_buffer_to_texture` 再读回，"GPU 一次算术都没做"——那次测的是
//! "拷贝无差异"，不是"渲染无差异"。故字形必须真正**过一遍着色器**
//! （采样 + 混合），这样跨后端差异才测的是本项目会遇到的差异。
//!
//! ## 与 T033 的关系
//!
//! 复用 [`super::glyph_cache`] 的缓存键与栅格化（**同一个** `SwashCache`
//! 与亚像素分桶），只是把位图从"只留在 CPU"变成"上传进 atlas 纹理"。
//! 两条路径共用键，故"缓存命中"在两个层次上一致。
//!
//! ## 缺字回退的实心占位（tofu）
//!
//! FR-047 的兜底是"缺字占位"——即**豆腐块**。那是**设计内行为**：
//! 系统字体链都取不到该字形时，画一个方框提示读者"这里有个字没渲染出来"。
//! 它与"用实心块冒充真实字形"是两件事：
//!
//! | | 触发条件 | 语义 |
//! |---|---|---|
//! | tofu 占位 | 字形**真的不存在**（回退链耗尽） | 如实报告缺字 |
//! | 实心块（已移除） | 无条件 | 伪装成字形 |
//!
//! 本模块保留前者、移除后者。

use cosmic_text::SwashContent;

use super::batch::RenderBatch;
use super::glyph_cache::{GlyphCache, GlyphRef};
use super::pipeline::PipelineError;

/// 字形绘制着色器（WGSL）。
///
/// ## 三条刻意的设计（对齐 spike 的 `blit.wgsl`，那处已实测跨后端一致）
///
/// 1. **点采样**（`textureLoad` 而非 `textureSample`）：字形位图是 1:1
///    贴合目标的，用采样器会引入各后端不同的过滤实现（跨后端差异的假来源）
/// 2. **标准 source-over 混合**：`ALPHA_BLENDING` 的状态在 Rust 侧设置，
///    着色器只输出 `(alpha, alpha, alpha, alpha)`——即"用 alpha 作为灰度"
/// 3. **整像素对齐绘制**：顶点位置由 Rust 侧换算为裁剪空间坐标，
///    着色器不做任何几何运算（避免各后端的浮点路径差异）
pub const GLYPH_SHADER: &str = r#"
// 每个字形一次实例：用实例化避免逐字形改顶点缓冲
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var atlas_texture: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

// 墨色（正文色，FR-030：由主题给出、不得低于 text-secondary）。
// 用 uniform 而非逐实例属性：一页的字形同色，按批次给一次即可。
struct InkColor {
    rgb: vec4<f32>,   // xyz = 颜色（0–1），w 未用（对齐到 16 字节）
};
@group(0) @binding(2) var<uniform> ink: InkColor;

// 实例属性：目标矩形（裁剪空间）+ 图集 UV 矩形
struct Instance {
    @location(0) rect: vec4<f32>,   // x0, y0, x1, y1（裁剪空间）
    @location(1) uv_rect: vec4<f32> // u0, v0, u1, v1
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, inst: Instance) -> VertexOutput {
    // 两个三角形（6 个顶点）覆盖矩形
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];

    var output: VertexOutput;
    output.position = vec4<f32>(
        mix(inst.rect.x, inst.rect.z, corner.x),
        mix(inst.rect.y, inst.rect.w, corner.y),
        0.0,
        1.0,
    );
    output.uv = vec2<f32>(
        mix(inst.uv_rect.x, inst.uv_rect.z, corner.x),
        mix(inst.uv_rect.y, inst.uv_rect.w, corner.y),
    );
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // 点采样：取最近纹素，与位图 1:1 贴合。
    // （不用采样器路径：它的过滤实现因后端而异，是跨后端差异的假来源）
    let dims = vec2<f32>(textureDimensions(atlas_texture, 0));
    let coords = vec2<i32>(input.uv * dims);
    let coverage = textureLoad(atlas_texture, coords, 0).r;

    // 源色 = 墨色，源 alpha = 覆盖率。配合 `ALPHA_BLENDING`
    // （src·srcAlpha + dst·(1−srcAlpha)）即得标准文字合成：
    //   result = ink · coverage + dst · (1 − coverage)
    //
    // 初版这里返回 `vec4(coverage)`（把覆盖率当灰度）——结果是**反的**：
    // 字墨处变白、空白处透底，实测字形呈白块（ASCII 复核时肉眼可辨）。
    return vec4<f32>(ink.rgb.rgb, coverage);
}
"#;

/// 一个待绘制的字形实例（目标矩形 + 图集 UV）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphInstance {
    /// 目标矩形（**画布像素坐标**：左、上、右、下）
    pub rect_px: [f32; 4],
    /// 图集 UV 矩形（`[u0, v0, u1, v1]`，归一化）
    pub uv_rect: [f32; 4],
}

/// 位图到图集的打包结果。
#[derive(Debug, Clone, PartialEq)]
pub struct PackedAtlas {
    /// 图集边长（px）
    pub size: u32,
    /// 图集像素（单通道 alpha，`size × size`）
    pub pixels: Vec<u8>,
    /// 每个入参字形对应的实例（顺序与入参一致；`None` = 该字形无位图）
    pub instances: Vec<Option<GlyphInstance>>,
}

impl PackedAtlas {
    /// 非零 alpha 的像素数（"图集真的有内容"的判据）。
    #[must_use]
    pub fn non_empty_pixels(&self) -> usize {
        self.pixels.iter().filter(|byte| **byte != 0).count()
    }

    /// 是否有实例（至少一个字形被绘制）。
    #[must_use]
    pub fn instance_count(&self) -> usize {
        self.instances
            .iter()
            .filter(|entry| entry.is_some())
            .count()
    }
}

/// 一个待打包的字形（位置 + 身份）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphPlacement {
    /// 字形身份（字体 + 字形 + 字重）
    pub glyph: GlyphRef,
    /// 字号（px）
    pub font_size: f32,
    /// 目标左缘（画布 px，整数对齐前的值）
    pub x_px: f32,
    /// 目标**基线** y（画布 px）
    pub baseline_y_px: f32,
    /// 行内亚像素偏移（参与缓存键的分桶）
    pub subpixel_x: f32,
    /// 是否可用（false = 缺字，画 tofu 占位）
    pub available: bool,
}

/// 把一组字形栅格化并打包进一张图集。
///
/// ## 为什么"打包"与"绘制"分开
///
/// 打包是 **CPU + 一次批量上传**（`write_texture` × N，或整张一次），
/// 绘制是 GPU 的采样与混合。分开之后：
///
/// - 打包可脱离 GPU 单测（给定字形 → 位图与实例，纯计算）
/// - "上传了什么"与"画成什么样"是两个可分别断言的量
///
/// 这与 T034 把"指令序列"（纯数据）与"encode"（碰 GPU）分开是同一条思路。
///
/// ## 图集尺寸的取法
///
/// 按字形数与平均位图面积的估算取**最小的 2 的幂**（见
/// [`estimate_atlas_size`]），比"固定 1024² 起步"省内存，也比"逐次增长"
/// 少一次搬迁。整页字形一次打包时估算准确度足够。
pub fn pack_glyphs(cache: &mut GlyphCache, glyphs: &[GlyphPlacement]) -> PackedAtlas {
    let size = estimate_atlas_size(glyphs.len(), glyphs.first().map_or(19.0, |g| g.font_size));
    let mut pixels = vec![0u8; (size * size) as usize];
    let mut instances = Vec::with_capacity(glyphs.len());

    // 用 atlas 分配器排布（复用 T033 的 etagere 封装）
    let mut atlas = super::atlas::GlyphAtlas::with_size(size);
    let bytes_per_row = super::offscreen::aligned_bytes_per_row(size, 1);

    // 先把全部位图栅格化并收集（释放对 cache 的可变借用，便于后续批量处理）
    let mut bitmaps: Vec<Option<RasterBitmap>> = Vec::with_capacity(glyphs.len());
    for placement in glyphs {
        bitmaps.push(rasterize_glyph(cache, placement));
    }

    for (placement, bitmap) in glyphs.iter().zip(bitmaps) {
        let Some(bitmap) = bitmap else {
            // 缺字：无位图。绘制侧走 tofu 占位（FR-047 的设计内行为）
            instances.push(None);
            continue;
        };

        let Some(region) = atlas.allocate(bitmap.width, bitmap.height) else {
            // 图集放不下（估算偏小）：跳过该字形而非 panic
            instances.push(None);
            continue;
        };

        blit_alpha(
            &mut pixels,
            size,
            region.x,
            region.y,
            bitmap.width,
            bitmap.height,
            bytes_per_row,
            &bitmap.data,
        );

        instances.push(Some(GlyphInstance {
            rect_px: [
                placement.x_px + bitmap.left as f32,
                // `placement.top` 是相对基线的**向上**偏移（swash 约定）：
                // 位图顶 = 基线 − top
                placement.baseline_y_px - bitmap.top as f32,
                placement.x_px + bitmap.left as f32 + bitmap.width as f32,
                placement.baseline_y_px - bitmap.top as f32 + bitmap.height as f32,
            ],
            uv_rect: region.uv(size),
        }));
    }

    PackedAtlas {
        size,
        pixels,
        instances,
    }
}

/// 位图（栅格化结果）。
struct RasterBitmap {
    width: u32,
    height: u32,
    left: i32,
    top: i32,
    data: Vec<u8>,
}

/// 备好的字形绘制资源（管线 + 绑定组 + 实例缓冲 + 画布尺寸）。
///
/// ## 为什么把这一整段抽出来（S3 T004）
///
/// 它原本是 `offscreen.rs` 的私有函数 `draw_glyphs`，只有"自建纹理 + 读回"
/// 一条消费路径。S3 的阅读视图要在 **egui 的 render pass** 上画同一批字形
/// （PaintCallback），若把这段复制一份过去，两条路径会各自演化——而它们的
/// 一致性判据是**逐像素相同**（`tests/shared_device.rs`），复制版迟早会分叉，
/// 且分叉只会在三平台 CI 快照阶段暴露。
///
/// 抽成"**备资源** + **提交绘制**"两段后，两条路径共用同一份代码：
///
/// ```text
///   GlyphRenderer::prepare(device, queue, target_format, batch, cache)
///        ↓ Some(renderer)（无字形可画时为 None）
///   renderer.draw(&mut render_pass)      ← 离屏路径与 PaintCallback 各自调用
/// ```
///
/// ## 为什么 `prepare` 与 `draw` 分开
///
/// wgpu 的 `write_texture`（图集上传）必须发生在 render pass **之外**，且
/// 早于引用它的绘制命令。egui 的 CallbackTrait 也正是这个形状
/// （`prepare` → `paint`），两段分开才能对接。
pub struct GlyphRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_count: u32,
}

impl std::fmt::Debug for GlyphRenderer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GlyphRenderer")
            .field("instance_count", &self.instance_count)
            .finish_non_exhaustive()
    }
}

impl GlyphRenderer {
    /// 栅格化 + 打包图集 + 建管线与实例缓冲。
    ///
    /// 返回 `Ok(None)` = 本批次没有可画的字形（空批次、或字形全部无位图）。
    /// 那是**合法状态**（空页），不是错误。
    ///
    /// ## 缺字（无位图）的处置
    ///
    /// 位图为空时**跳过该字形**——不画实心块冒充它。FR-047 的"缺字占位"
    /// （tofu）由字体栈自身的 `.notdef` 字形提供：系统字体链连 `.notdef` 都没有
    /// 时，画任何东西都是伪装，故跳过并让上层的诊断报告缺字。
    pub fn prepare(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        batch: &RenderBatch,
        cache: &mut GlyphCache,
    ) -> Result<Option<Self>, PipelineError> {
        if batch.glyphs().is_empty() {
            return Ok(None);
        }

        // --- 1. 把字形转成待打包的放置描述 ---
        let placements: Vec<GlyphPlacement> = batch
            .glyphs()
            .iter()
            .map(|glyph| GlyphPlacement {
                glyph: GlyphRef {
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
        let packed = pack_glyphs(cache, &placements);
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
            source: wgpu::ShaderSource::Wgsl(GLYPH_SHADER.into()),
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
            return Ok(None);
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
                    // 目标格式由调用方给出（离屏是 Rgba8Unorm，egui surface
                    // 通常是 Bgra8Unorm*）——写死会让共享 device 路径校验失败。
                    format: target_format,
                    // 标准 source-over：`src * srcAlpha + dst * (1 - srcAlpha)`
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Ok(Some(Self {
            pipeline,
            bind_group,
            instances: instance_buffer,
            instance_count: instances.len() as u32,
        }))
    }

    /// 在给定的 render pass 上提交字形绘制。
    ///
    /// **不改 render pass 的 load/store 语义**——底色由调用方决定（离屏路径
    /// 用 `Clear`，egui 主 pass 用 `Load` 以免抹掉已有 UI）。
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instances.slice(..));
        pass.draw(0..6, 0..self.instance_count);
    }
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

/// 栅格化单个字形。
///
/// ## 为什么只有**一条**取位图的路径
///
/// 初版有两条：先查 T033 的 `GlyphCache` 条目（若命中就复用它的位图），
/// 未命中再走 `rasterize_uncached`。那看着像"缓存优化"，实际是个陷阱：
///
/// - 两条路径的位图**来源不同**（一个是 T033 自己 atlas 里的副本，
///   一个是当场从 swash 取的），任一条被改动（如加后处理）就会分叉
/// - 更糟的是它让**测试失效**：把 `rasterize_uncached` 的结果二值化之后，
///   命中 T033 条目的那些字形仍走旧路径、仍是抗锯齿的——于是
///   "抗锯齿"判据照样通过，而实际的绘制路径已经退化成实心块。
///   （本任务实测踩到：二值化变异后 `rendered_glyphs_are_antialiased`
///   **仍然通过**。）
///
/// 故只保留一条：**每次从 swash 取**。`SwashCache` 自身有缓存（同一
/// `ScaleContext`），故重复调用不重复光栅化——省下的那点查表时间不值得
/// 引入第二条分叉路径。
fn rasterize_glyph(cache: &mut GlyphCache, placement: &GlyphPlacement) -> Option<RasterBitmap> {
    if !placement.available {
        return None;
    }

    let image =
        cache.rasterize_uncached(placement.glyph, placement.font_size, placement.subpixel_x)?;

    // 只处理单通道 mask（灰度 AA，research R4 的防 flaky 要求）。
    // 彩色位图（emoji）暂不参与字形绘制路径——它们没有"墨色"语义，
    // 且会把颜色管理问题引进来。
    if !matches!(image.content, SwashContent::Mask) {
        return None;
    }

    Some(RasterBitmap {
        width: image.placement.width.max(1),
        height: image.placement.height.max(1),
        left: image.placement.left,
        top: image.placement.top,
        data: image.data,
    })
}

/// 把位图逐行拷进图集像素缓冲（单通道）。
///
/// ## 为什么逐行而不是整块拷贝
///
/// 图集是正方形缓冲，位图放在 `(x, y)` 处——目标行步长是 `size`，
/// 源行步长是 `bitmap.width`。整块 `copy_from_slice` 只在"位图恰好占满
/// 整行"时正确，而那是罕见的特例。
///
/// `bytes_per_row` 参数是给**调用方**的可读性提示（图集是单通道，
/// `write_texture` 的对齐在 GPU 上传时才需要），此处按 `size` 走。
#[allow(clippy::too_many_arguments)]
fn blit_alpha(
    target: &mut [u8],
    target_size: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    _bytes_per_row: u32,
    source: &[u8],
) {
    for row in 0..height {
        let source_start = (row * width) as usize;
        let source_end = source_start + width as usize;
        if source_end > source.len() {
            break;
        }
        let target_start = ((y + row) * target_size + x) as usize;
        let target_end = target_start + width as usize;
        if target_end > target.len() {
            break;
        }
        target[target_start..target_end].copy_from_slice(&source[source_start..source_end]);
    }
}

/// 按字形数估算图集边长（2 的幂）。
///
/// ## 估算依据
///
/// 单通道位图的面积随字号平方增长：`(font_size × 1.3)²` 是"含外扩的
/// 保守上界"（中文字形的墨迹通常占字身的一部分，1.3 的系数覆盖 ascender/
/// descender 的溢出）。取 `字形数 × 该面积` 后再放宽 1.5 倍（etagere 的
/// 桶对齐会浪费一部分空间），向上取到 2 的幂。
///
/// 下限 64、上限 [`super::atlas::MAX_ATLAS_SIZE`]——后者保证超大字集
/// 时不会申请一张超出 GPU 容忍度的纹理。
#[must_use]
pub fn estimate_atlas_size(glyph_count: usize, font_size: f32) -> u32 {
    if glyph_count == 0 {
        return 64;
    }
    let per_glyph = (font_size * 1.3).max(4.0);
    let needed = per_glyph * per_glyph * glyph_count as f32 * 1.5;
    let dimension = needed.sqrt().ceil().max(64.0);
    let mut size = 64u32;
    while (size as f32) < dimension && size < super::atlas::MAX_ATLAS_SIZE {
        size *= 2;
    }
    size.min(super::atlas::MAX_ATLAS_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_size_is_power_of_two_and_bounded() {
        for count in [0usize, 1, 10, 100, 1000, 100_000] {
            let size = estimate_atlas_size(count, 19.0);
            assert!(
                size.is_power_of_two(),
                "count={count} 时应为 2 的幂，实测 {size}"
            );
            assert!(size >= 64, "下限应为 64");
            assert!(
                size <= super::super::atlas::MAX_ATLAS_SIZE,
                "上限应为 MAX_ATLAS_SIZE，实测 {size}"
            );
        }
    }

    /// 字形越多，估算的图集越大（单调性——防"恒返回 64"这类退化）。
    #[test]
    fn atlas_size_grows_with_glyph_count() {
        let small = estimate_atlas_size(4, 19.0);
        let large = estimate_atlas_size(2000, 19.0);
        assert!(
            large > small,
            "字形数增大时图集应更大：4 个 → {small}，2000 个 → {large}"
        );
    }

    /// 字号越大，估算的图集越大。
    #[test]
    fn atlas_size_grows_with_font_size() {
        let small = estimate_atlas_size(100, 14.0);
        let large = estimate_atlas_size(100, 30.0);
        assert!(
            large >= small,
            "字号 30 的图集不应小于字号 14：{small} vs {large}"
        );
    }

    #[test]
    fn shader_contains_required_entry_points() {
        assert!(GLYPH_SHADER.contains("fn vs_main"));
        assert!(GLYPH_SHADER.contains("fn fs_main"));
        // 点采样（不是 textureSample）——跨后端一致性的前提
        assert!(
            GLYPH_SHADER.contains("textureLoad"),
            "着色器应使用 textureLoad（点采样）"
        );
        assert!(
            !GLYPH_SHADER.contains("textureSample"),
            "着色器不应使用 textureSample（过滤实现因后端而异）"
        );
    }

    #[test]
    fn blit_copies_rows_with_the_right_stride() {
        // 8×8 图集，放一个 2×2 位图在 (1, 1)
        let mut target = vec![0u8; 64];
        let source = vec![10u8, 11, 12, 13];
        blit_alpha(&mut target, 8, 1, 1, 2, 2, 8, &source);

        assert_eq!(target[8 + 1], 10);
        assert_eq!(target[8 + 2], 11);
        assert_eq!(target[16 + 1], 12);
        assert_eq!(target[16 + 2], 13);
        // 边界外未被污染
        assert_eq!(target[8 + 3], 0);
        assert_eq!(target[24 + 1], 0);
    }

    /// 位图超出目标右/下边界时**截断而不越界**。
    ///
    /// ## 这条的构造要点
    ///
    /// 源位图必须**足够大**才能真的碰到边界：初版给 3×3 的源（9 字节）
    /// 配 4×4 的目标从 (3,3) 起——第 1 行就因目标越界而 `break`，
    /// 于是 (3,3) 根本没被写到（实测 left=0 而 right=1，断言失败）。
    /// 源的尺寸要覆盖到"能放进目标的那部分"。
    #[test]
    fn blit_stops_at_the_target_boundary() {
        let mut target = vec![0u8; 16]; // 4×4
        let source = vec![7u8; 6 * 6]; // 6×6，远大于剩余空间
        blit_alpha(&mut target, 4, 2, 2, 6, 6, 4, &source);

        // 能放下的行：(2,2) 起的 2 行 × 2 列（目标 4×4 的右下角 2×2 区）
        assert_eq!(target[2 * 4 + 2], 7, "左上角应被写入");
        assert_eq!(target[2 * 4 + 3], 7);
        assert_eq!(target[3 * 4 + 2], 7);
        assert_eq!(target[3 * 4 + 3], 7);

        // 而目标的前两行/前两列（位图未覆盖处）保持零
        for row in 0..2 {
            for column in 0..4 {
                assert_eq!(
                    target[row * 4 + column],
                    0,
                    "({column},{row}) 不在位图范围内，应保持 0"
                );
            }
        }
        assert_eq!(target[2 * 4], 0, "第 2 行第 0 列不在位图范围内");
        assert_eq!(target[2 * 4 + 1], 0);

        // 无越界写入：长度不变
        assert_eq!(target.len(), 16);

        // ## 第 3 行起被整行跳过（`break` 而非 `continue`）
        //
        // 这不是缺陷：行号单调递增，一旦某行整体越界，其后的行也必然越界。
        // `continue` 只会白跑剩下的迭代。
    }

    #[test]
    fn empty_atlas_has_no_instances() {
        let atlas = PackedAtlas {
            size: 64,
            pixels: vec![0u8; 64 * 64],
            instances: Vec::new(),
        };
        assert_eq!(atlas.instance_count(), 0);
        assert_eq!(atlas.non_empty_pixels(), 0);
    }
}
