//! R5-4：Windows-gnu + wgpu 构建与**离屏初始化**验证（T007）
//! 与 R5-6：单基线快照容差初判的**探针部分**（T009 补容差结论）
//!
//! ## R5-4 要验证什么
//!
//! wgpu 官方 CI 只测 **msvc**（`.github/workflows/ci.yml` 的证据），本项目
//! 钉 **gnu**。故"Windows-gnu + wgpu 能否构建、能否在无窗口（离屏）下
//! 初始化 device/queue"是未实测的遗留项。
//!
//! ## 离屏初始化的关键点（research R4 已列，此处实测）
//!
//! 1. `Instance::request_adapter` 无 surface —— 这决定"CI 无 GPU 能否跑"
//! 2. `request_device` 成功
//! 3. `Rgba8Unorm` + `RENDER_ATTACHMENT | COPY_SRC` 纹理可建
//! 4. `copy_texture_to_buffer` 的 `bytes_per_row` **必须 256 对齐**，读回要
//!    去 padding（这是最容易踩的坑：不对齐会直接报 validation error）
//! 5. `map_async` 后**必须** `device.poll(wait)` 才会完成映射
//!
//! ## 诚实模式（对应 spec「不得伪装通过」）
//!
//! 本模块**不**把"拿不到 adapter"当成成功。它区分两种结局：
//! - 拿到 adapter → 继续完整初始化并渲染一帧，报告后端名与像素校验结果
//! - 拿不到 adapter → 报告 `SKIPPED: <原因>` 并返回**非零**退出码
//!
//! 上层（CI / S2-1 的探针）据此把"环境无 GPU"与"能跑但输出错"分开——
//! 这正是 FR-053「后端不可用时明确报告不可用并允许测试跳过，MUST NOT 伪装通过」
//! 的落点。

/// 探针的结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 拿到后端并完成离屏渲染，携带后端名与像素统计
    Available {
        /// 后端名（如 `vulkan` / `dx12` / `metal`）
        backend: String,
        /// 适配器名（如 `llvmpipe` / `Microsoft Basic Render Driver`）
        adapter: String,
        /// 读回图像里非零像素的个数（**必须 > 0**，否则是黑屏）
        non_black_pixels: usize,
        /// 读回图像的总像素数
        total_pixels: usize,
    },
    /// 无可用后端（诚实报告，不伪装通过）
    Unavailable {
        /// 失败原因（人类可读；**不是**面向用户的本地化文案，而是 CI 日志）
        reason: String,
    },
}

impl ProbeOutcome {
    /// 是否可用。
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    /// 供 CI 直接打印的一行结论。
    #[must_use]
    pub fn report_line(&self) -> String {
        match self {
            Self::Available {
                backend,
                adapter,
                non_black_pixels,
                total_pixels,
            } => {
                let ratio = if *total_pixels == 0 {
                    0.0
                } else {
                    *non_black_pixels as f64 / *total_pixels as f64 * 100.0
                };
                format!(
                    "PROBE OK backend={backend} adapter={adapter} \
                     non_black={non_black_pixels}/{total_pixels} ({ratio:.2}%)"
                )
            }
            Self::Unavailable { reason } => format!("SKIPPED: {reason}"),
        }
    }
}

/// 计算 `bytes_per_row` 的 256 字节对齐值（wgpu 的硬要求）。
///
/// ## 为什么这条算术要单独成函数
///
/// research R4 点名它是"离屏读回最容易踩的坑"：不对齐时 wgpu 直接报
/// validation error，而错误信息（`bytes_per_row must be a multiple of 256`）
/// 不会告诉你"正确值应该是多少"。故把推导写成可单测的函数，并把
/// "读回时要去掉哪些字节"一并给出。
#[must_use]
pub fn aligned_bytes_per_row(width: u32, bytes_per_pixel: u32) -> u32 {
    const COPY_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let unpadded = width * bytes_per_pixel;
    unpadded.div_ceil(COPY_ALIGNMENT) * COPY_ALIGNMENT
}

/// 从 256 对齐的读回缓冲里剥掉每行 padding，得到紧凑的像素数据。
///
/// 返回 `(紧凑数据, 每行 padding 字节数)`。
#[must_use]
pub fn strip_row_padding(
    padded: &[u8],
    width: u32,
    bytes_per_pixel: u32,
    bytes_per_row: u32,
) -> (Vec<u8>, u32) {
    let tight_row = width * bytes_per_pixel;
    let padding = bytes_per_row.saturating_sub(tight_row);
    let mut tight =
        Vec::with_capacity((tight_row as usize) * (padded.len() / bytes_per_row.max(1) as usize));
    for row in padded.chunks_exact(bytes_per_row as usize) {
        tight.extend_from_slice(&row[..tight_row as usize]);
    }
    (tight, padding)
}

/// 统计非零像素个数（黑屏判据）。
///
/// ## 为什么用"非黑像素"而非"非空缓冲"
///
/// 离屏纹理若未被真正渲染，读回的是**全零**——那是合法的缓冲区内容，
/// 长度断言会通过。只有看像素值才能发现"渲染没发生"。故本函数是
/// "离屏路径真的工作了吗"的唯一有效判据。
#[must_use]
pub fn count_non_black_pixels(rgba: &[u8]) -> usize {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[0] != 0 || pixel[1] != 0 || pixel[2] != 0)
        .count()
}

/// 跑离屏探针（**同步**外观；wgpu 的 async 在此用 block_on 收口）。
///
/// 返回进程退出码：0 = 可用；3 = 无可用后端（对齐 CLI 的"不支持能力"码）。
#[must_use]
pub fn run_probe() -> i32 {
    let outcome = probe_blocking();
    println!("{}", outcome.report_line());
    match outcome {
        ProbeOutcome::Available { .. } => 0,
        // 退出码 3 与 001 CLI 的"不支持能力"约定一致（FR-058）
        ProbeOutcome::Unavailable { .. } => 3,
    }
}

/// 探针主体（编译期不依赖 tokio/asyncio：用 `pollster` 式手工驱动）。
fn probe_blocking() -> ProbeOutcome {
    use wgpu::util::DeviceExt as _;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());

    let adapter = match poll_once(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    })) {
        Ok(Ok(adapter)) => adapter,
        Ok(Err(error)) => {
            return ProbeOutcome::Unavailable {
                reason: format!("request_adapter 失败：{error}"),
            };
        }
        Err(error) => {
            return ProbeOutcome::Unavailable {
                reason: format!("request_adapter 未完成：{error}"),
            };
        }
    };

    let info = adapter.get_info();

    let (device, queue) = match poll_once(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("tome-s2-spike-probe"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    })) {
        Ok(Ok(pair)) => pair,
        Ok(Err(error)) => {
            return ProbeOutcome::Unavailable {
                reason: format!("request_device 失败：{error}"),
            };
        }
        Err(error) => {
            return ProbeOutcome::Unavailable {
                reason: format!("request_device 未完成：{error}"),
            };
        }
    };

    // --- 离屏渲染一帧纯色，验证整条读回链路 ---
    const WIDTH: u32 = 64;
    const HEIGHT: u32 = 32;
    const BYTES_PER_PIXEL: u32 = 4;

    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen-target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        // Rgba8Unorm：research R4 指定的非 sRGB 格式（避免自动 gamma 干扰像素比对）
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });

    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    // 一个只做 clear 的 render pass——足以验证"离屏渲染 + 读回"链路，
    // 且不需要写 WGSL（T009 的探针才需要真画字形）
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("offscreen-encoder"),
    });
    {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("offscreen-clear-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 中灰——确保读回的是非零像素（黑屏判据的有效性前提）
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.5,
                        g: 0.25,
                        b: 0.125,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    let bytes_per_row = aligned_bytes_per_row(WIDTH, BYTES_PER_PIXEL);
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("readback-buffer"),
        contents: &vec![0u8; (bytes_per_row * HEIGHT) as usize],
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
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
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(Some(encoder.finish()));

    // map_async + poll：research R4 点名的第 5 条坑（不 poll 则映射永不完成）
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
        return ProbeOutcome::Unavailable {
            reason: format!("device.poll 失败：{error}"),
        };
    }

    match receiver.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            return ProbeOutcome::Unavailable {
                reason: format!("map_async 失败：{error}"),
            };
        }
        Err(error) => {
            return ProbeOutcome::Unavailable {
                reason: format!("map_async 回执未到达：{error}"),
            };
        }
    }

    let mapped = match slice.get_mapped_range() {
        Ok(mapped) => mapped,
        Err(error) => {
            return ProbeOutcome::Unavailable {
                reason: format!("get_mapped_range 失败：{error}"),
            };
        }
    };
    let (tight, _padding) = strip_row_padding(&mapped, WIDTH, BYTES_PER_PIXEL, bytes_per_row);
    drop(mapped);
    let _ = slice;
    buffer.unmap();

    ProbeOutcome::Available {
        backend: format!("{:?}", info.backend),
        adapter: info.name,
        non_black_pixels: count_non_black_pixels(&tight),
        total_pixels: (WIDTH * HEIGHT) as usize,
    }
}

/// 手工驱动一个 future 到就绪（避免为一个验证项引入 async 运行时依赖）。
fn poll_once<F: std::future::Future>(future: F) -> Result<F::Output, String> {
    use std::task::{Context, Poll, Waker};

    let mut future = Box::pin(future);
    // 空 waker：wgpu 的 request_* 在无外部事件源时会自行完成，
    // 若未完成则会走到下面的 Pending 分支并如实报告
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => Ok(output),
        Poll::Pending => Err("future 未在一次 poll 内就绪".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 256 对齐的算术（research R4 点名的第一号坑）。
    #[test]
    fn bytes_per_row_is_256_aligned() {
        // 32px 宽 × 4 字节 = 128 → 对齐到 256
        assert_eq!(aligned_bytes_per_row(32, 4), 256);
        // 64px × 4 = 256 → 已对齐
        assert_eq!(aligned_bytes_per_row(64, 4), 256);
        // 65px × 4 = 260 → 对齐到 512
        assert_eq!(aligned_bytes_per_row(65, 4), 512);
        // 1px × 4 = 4 → 对齐到 256
        assert_eq!(aligned_bytes_per_row(1, 4), 256);

        // 属性：任何宽度下结果都是 256 的倍数且 ≥ 紧凑行长
        for width in 1..200u32 {
            let aligned = aligned_bytes_per_row(width, 4);
            assert_eq!(aligned % 256, 0, "宽度 {width} 的对齐结果未对齐");
            assert!(aligned >= width * 4, "宽度 {width} 的对齐结果小于紧凑行长");
        }
    }

    /// 读回去 padding（research R4 点名的第二号坑）。
    #[test]
    fn row_padding_is_stripped() {
        // 2 行 × 8px 紧凑（32 字节），对齐到 256
        let bytes_per_row = aligned_bytes_per_row(8, 4);
        assert_eq!(bytes_per_row, 256);

        let mut padded = vec![0u8; (bytes_per_row * 2) as usize];
        // 第 1 行填 0xAA，第 2 行填 0xBB（含 padding 与像素区都填，便于验证剥离）
        for byte in &mut padded[..bytes_per_row as usize] {
            *byte = 0xAA;
        }
        for byte in &mut padded[bytes_per_row as usize..] {
            *byte = 0xBB;
        }

        let (tight, padding) = strip_row_padding(&padded, 8, 4, bytes_per_row);
        assert_eq!(padding, 256 - 32);
        assert_eq!(tight.len(), 8 * 4 * 2, "紧凑数据长度应为 2 行 × 32 字节");

        // 前 32 字节应来自第 1 行、后 32 字节来自第 2 行
        assert!(tight[..32].iter().all(|byte| *byte == 0xAA));
        assert!(tight[32..].iter().all(|byte| *byte == 0xBB));
    }

    #[test]
    fn non_black_count_detects_all_zero_buffer() {
        // 全零缓冲（"渲染没发生"的典型形态）→ 非黑像素数 0
        assert_eq!(count_non_black_pixels(&[0u8; 64]), 0);
        // 单个非零像素
        let mut pixels = [0u8; 16];
        pixels[4] = 255; // 第 2 个像素的 R 通道
        assert_eq!(count_non_black_pixels(&pixels), 1);
    }

    #[test]
    fn outcome_report_line_is_skipped_when_unavailable() {
        let outcome = ProbeOutcome::Unavailable {
            reason: "no adapter".to_owned(),
        };
        assert!(!outcome.is_available());
        assert!(
            outcome.report_line().starts_with("SKIPPED:"),
            "不可用必须以 SKIPPED 前缀显式报告（绝不伪装通过）：{}",
            outcome.report_line()
        );
    }

    #[test]
    fn outcome_report_line_for_available_includes_backend() {
        let outcome = ProbeOutcome::Available {
            backend: "Vulkan".to_owned(),
            adapter: "llvmpipe".to_owned(),
            non_black_pixels: 2048,
            total_pixels: 2048,
        };
        let line = outcome.report_line();
        assert!(outcome.is_available());
        assert!(line.contains("Vulkan"), "{line}");
        assert!(line.contains("llvmpipe"), "{line}");
        assert!(line.contains("100.00%"), "{line}");
    }
}

/// 枚举本机 wgpu 可见的全部适配器（T009 的跨后端对照前提）。
///
/// ## 为什么要枚举而不是只取一个
///
/// T009 要回答"单基线能否跨后端共享"（research R4 的第 4 条策略）。
/// 本机没有 lavapipe/Metal，但**在 Windows 上同时有 Vulkan 与 DX12**——
/// 分别渲染同一页样本再比对，是对该问题的**直接代理实验**：
/// 若两个不同后端在 CPU 光栅化 + GPU blit 架构下像素差异极小，
/// 则"单基线可共享"的推断在本机可得实证支持。
#[must_use]
pub fn enumerate_adapters() -> Vec<(String, String)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    // 0.19/30 起 `enumerate_adapters` 返回 Future（与其他 request_* 一致）
    let adapters = match poll_once(instance.enumerate_adapters(wgpu::Backends::all())) {
        Ok(adapters) => adapters,
        Err(_) => return Vec::new(),
    };
    adapters
        .into_iter()
        .map(|adapter| {
            let info = adapter.get_info();
            (format!("{:?}", info.backend), info.name)
        })
        .collect()
}

// ===========================================================================
// T009：离屏渲染同一页样本 → 跨后端比对（R5-6 的容差初判）
// ===========================================================================

/// 用**指定后端**离屏渲染一张样本图，返回 `(像素 RGBA, 后端名, 适配器名)`。
///
/// ## 样本图像的构造（为什么是"模拟排版"而不是空白）
///
/// 快照容差校准必须用**接近真实阅读页**的样本，否则测的是"两个后端都能
/// 清空颜色"这个无信息量的命题。真实阅读页的像素特征是：
///
/// - 大面积均匀背景（`--bg-reading`，昼 #F5EEDC）
/// - 小面积高对比前景（字形笔画，宽约 1–2px）
/// - 前景占页面面积比例很低
///
/// 故样本用**几何图形模拟字形笔画**：在背景上画细竖条构成的网格。
/// 它不复现真实字形，但复现了"细笔画 + 低覆盖率 + 高对比"这三条与跨后端
/// 差异最相关的特征。真字形的快照容差在 S2-4 的 T036 用真实管线复测。
fn render_sample_with(backend: wgpu::Backends) -> Result<(Vec<u8>, String, String), String> {
    use wgpu::util::DeviceExt as _;

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: backend,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });

    let adapter = match poll_once(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    })) {
        Ok(Ok(adapter)) => adapter,
        Ok(Err(error)) => return Err(format!("request_adapter 失败：{error}")),
        Err(error) => return Err(format!("request_adapter 未完成：{error}")),
    };
    let info = adapter.get_info();

    let (device, queue) = match poll_once(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sample-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults(),
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::Off,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
    })) {
        Ok(Ok(pair)) => pair,
        Ok(Err(error)) => return Err(format!("request_device 失败：{error}")),
        Err(error) => return Err(format!("request_device 未完成：{error}")),
    };

    // 样本尺寸取真实阅读页量级（版心 684px = 36em @ 19px）
    const WIDTH: u32 = 684;
    const HEIGHT: u32 = 512;
    const BYTES_PER_PIXEL: u32 = 4;

    // --- ① 字形层：CPU 生成的 alpha 位图（模拟 swash 的栅格化产出）---
    let mut glyph_layer = vec![0u8; (WIDTH * HEIGHT * BYTES_PER_PIXEL) as usize];
    fill_glyph_layer(&mut glyph_layer, WIDTH, HEIGHT);

    let glyph_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glyph-layer"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let padded_glyphs = pad_rows(&glyph_layer, WIDTH, HEIGHT, BYTES_PER_PIXEL);
    let glyph_upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("glyph-upload"),
        contents: &padded_glyphs,
        usage: wgpu::BufferUsages::COPY_SRC,
    });

    // --- ② 目标层：底色 + 字形混合后的结果 ---
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("page-target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());

    // --- ③ 管线：点采样 + source-over 混合 ---
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("blit-shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("blit-bgl"),
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
        ],
    });

    let glyph_view = glyph_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("point-sampler"),
        // 点采样：不引入各后端不同的过滤实现
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("blit-bg"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&glyph_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("blit-pl"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("blit-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                // source-over 的**实现细节**是跨后端差异的真实来源之一
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("sample-encoder"),
    });

    encoder.copy_buffer_to_texture(
        wgpu::TexelCopyBufferInfo {
            buffer: &glyph_upload,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(aligned_bytes_per_row(WIDTH, BYTES_PER_PIXEL)),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::TexelCopyTextureInfo {
            texture: &glyph_texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );

    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 先清成昼主题阅读底色（MASTER §2.1 的 #F5EEDC）
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0xF5 as f64 / 255.0,
                        g: 0xEE as f64 / 255.0,
                        b: 0xDC as f64 / 255.0,
                        a: 1.0,
                    }),
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
        pass.draw(0..3, 0..1);
    }

    let bytes_per_row = aligned_bytes_per_row(WIDTH, BYTES_PER_PIXEL);
    let readback = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("sample-readback"),
        contents: &vec![0u8; (bytes_per_row * HEIGHT) as usize],
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(Some(encoder.finish()));

    let slice = readback.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    if let Err(error) = device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    }) {
        return Err(format!("device.poll 失败：{error}"));
    }
    match receiver.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(format!("map_async 失败：{error}")),
        Err(error) => return Err(format!("map_async 回执未到达：{error}")),
    }

    let mapped = match slice.get_mapped_range() {
        Ok(mapped) => mapped,
        Err(error) => return Err(format!("get_mapped_range 失败：{error}")),
    };
    let (tight, _padding) = strip_row_padding(&mapped, WIDTH, BYTES_PER_PIXEL, bytes_per_row);
    // **必须先释放映射视图再 unmap**——否则 wgpu 在 unmap 时断言失败
    // （buffer.rs:802 的 panic："buffer is still mapped"）。
    drop(mapped);
    let _ = slice;
    readback.unmap();

    Ok((tight, format!("{:?}", info.backend), info.name))
}

/// 生成"字形层"位图：透明底 + 墨色笔画（模拟 swash 的 alpha 位图产出）。
///
/// 真实阅读页的像素特征：大面积均匀背景、小面积高对比前景（1–2px 细笔画）、
/// 前景覆盖率低。CPU 生成的位图复现这三条，GPU 侧负责把它们混合上去。
fn fill_glyph_layer(pixels: &mut [u8], width: u32, height: u32) {
    // 全透明底（alpha = 0）——混合时完全不覆盖底色
    for pixel in pixels.as_chunks_mut::<4>().0.iter_mut() {
        pixel[0] = 0;
        pixel[1] = 0;
        pixel[2] = 0;
        pixel[3] = 0;
    }

    // 昼主题墨色（MASTER §2.1 的 --text-primary #2E2418）
    const INK: [u8; 3] = [0x2E, 0x24, 0x18];
    // 字形边缘的中间透明度——**这是混合算术差异的放大点**：
    // alpha 非 0 非 255 时，`src*a + dst*(1-a)` 的实现细节才会暴露
    const EDGE_ALPHA: u8 = 128;

    let line_height = 12u32;
    let mut y = 8u32;
    let mut line_index = 0u32;
    while y + 10 < height {
        let start_x = 16 + (line_index % 4) * 6;
        let mut x = start_x;
        while x + 2 < width - 16 {
            let stroke_width = 2 + (line_index % 2);
            for dy in 0..10 {
                for dx in 0..stroke_width {
                    let px = x + dx;
                    let py = y + dy;
                    if px >= width || py >= height {
                        continue;
                    }
                    let offset = ((py * width + px) * 4) as usize;
                    pixels[offset] = INK[0];
                    pixels[offset + 1] = INK[1];
                    pixels[offset + 2] = INK[2];
                    // 笔画的最外一列给半透明，模拟抗锯齿边缘
                    pixels[offset + 3] = if dx == 0 || dx == stroke_width - 1 {
                        EDGE_ALPHA
                    } else {
                        255
                    };
                }
            }
            x += 19;
        }
        y += line_height;
        line_index += 1;
    }
}

/// 把紧凑的逐行像素补成 256 对齐（`copy_buffer_to_texture` 的硬要求）。
#[must_use]
pub fn pad_rows(pixels: &[u8], width: u32, height: u32, bytes_per_pixel: u32) -> Vec<u8> {
    let tight_row = width * bytes_per_pixel;
    let padded_row = aligned_bytes_per_row(width, bytes_per_pixel);
    if padded_row == tight_row {
        return pixels.to_vec();
    }
    let mut padded = vec![0u8; (padded_row * height) as usize];
    for row in 0..height as usize {
        let source = row * tight_row as usize;
        let target = row * padded_row as usize;
        padded[target..target + tight_row as usize]
            .copy_from_slice(&pixels[source..source + tight_row as usize]);
    }
    padded
}

/// 一次跨后端比对的记录。
#[derive(Debug, Clone)]
pub struct BackendComparison {
    /// 基线后端名
    pub baseline_backend: String,
    /// 对照后端名
    pub candidate_backend: String,
    /// 适配器名（对照侧）
    pub candidate_adapter: String,
    /// 比对报告
    pub report: crate::pixel_diff::DiffReport,
}

/// 跑跨后端比对：先以 Vulkan 渲染作基线，再逐后端比对。
///
/// 返回 `(逐条比对记录, 未能渲染的后端及原因)`。
///
/// ## 为什么基线选 Vulkan
///
/// research R4 的 CI 矩阵里 ubuntu 是 lavapipe（Vulkan），而**单基线策略**
/// 正是"以 lavapipe 生成的基线共享给全平台"。故本机以 Vulkan 为基线、
/// DX12/GL 为对照，恰好模拟"CI 基线 vs 其他平台后端"的关系。
#[must_use]
pub fn run_cross_backend_comparison(
    threshold: f64,
) -> (Vec<BackendComparison>, Vec<(String, String)>) {
    let (baseline_pixels, baseline_backend, _baseline_adapter) =
        match render_sample_with(wgpu::Backends::VULKAN) {
            Ok(result) => result,
            Err(error) => return (Vec::new(), vec![("Vulkan(基线)".to_owned(), error)]),
        };

    let mut comparisons = Vec::new();
    let mut failures = Vec::new();

    for (name, backends) in [("Dx12", wgpu::Backends::DX12), ("Gl", wgpu::Backends::GL)] {
        match render_sample_with(backends) {
            Ok((pixels, backend, adapter)) => {
                let report = crate::pixel_diff::compare_rgba(&baseline_pixels, &pixels, threshold);
                comparisons.push(BackendComparison {
                    baseline_backend: baseline_backend.clone(),
                    candidate_backend: format!("{name}/{backend}"),
                    candidate_adapter: adapter,
                    report,
                });
            }
            Err(error) => failures.push((name.to_owned(), error)),
        }
    }

    (comparisons, failures)
}

/// 样本有效性自检：渲染结果里"与纯底色不同"的像素数 / 总像素数。
///
/// ## 为什么必须有这条
///
/// 跨后端差异为 0 有两种解释：① 两后端真的一致（好结论）；
/// ② **样本根本没渲染出来**（两边都是同一个底色，差异当然为 0）。
/// 后者是 T009 首轮踩过的坑（当时样本压根没过 GPU）。故回报"有多少像素
/// 真的被混合改变了"，让 0 差异这个数字有前提。
#[must_use]
pub fn sample_pixel_stats() -> (usize, usize) {
    let (pixels, _backend, _adapter) = match render_sample_with(wgpu::Backends::VULKAN) {
        Ok(result) => result,
        Err(_) => return (0, 0),
    };
    let total = pixels.len() / 4;
    // 昼主题底色 #F5EEDC 对应的字节
    const BACKGROUND: [u8; 3] = [0xF5, 0xEE, 0xDC];
    let changed = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| {
            pixel[0] != BACKGROUND[0] || pixel[1] != BACKGROUND[1] || pixel[2] != BACKGROUND[2]
        })
        .count();
    (changed, total)
}
