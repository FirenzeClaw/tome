//! [DONE] wgpu 自绘管线骨架（T032 / FR-043、FR-036、FR-052）
//!
//! ## 本模块的职责边界
//!
//! 持有**设备与队列**，并给出"本机能不能离屏渲染"的确定性答案。字形栅格化、
//! atlas 打包、绘制指令生成分别在 [`super::glyph_cache`] / [`super::atlas`] /
//! [`super::batch`]——本模块只负责它们的共同前提：一个可用的 `Device`/`Queue`。
//!
//! ## 为什么 device 与 queue 必须一起持有
//!
//! wgpu 的 `Queue::submit` 消费 `CommandBuffer`（由 `Device` 的 encoder 产出），
//! 而离屏读回还需要 `Device::poll`（等 `map_async` 完成）。三者在一次渲染里
//! 交替使用，分开持有只会让每个调用点都要求两样东西——故合成一个对象。
//!
//! ## 离屏（FR-036/FR-052 的落点）
//!
//! 全程**无 surface**：`Instance::request_adapter` 的 `compatible_surface` 为
//! `None`，纹理用 `RENDER_ATTACHMENT | COPY_SRC` 自建。故本路径在无窗口系统上
//! 可运行——这是"排版渲染层能被无界面调用"的具体形态。
//!
//! ## 诚实模式（FR-053 / research R4 §5）
//!
//! 拿不到 adapter 时返回 [`PipelineError::NoAdapter`] 而**不是**一个"可用但
//! 画不出东西"的管线。上层据此显式报告 SKIPPED 而非伪装通过。
//!
//! ## 与 S3 外壳的边界（FR-036）
//!
//! 阅读页的正文字形由**本层**产出，不经外壳 UI 框架（egui）的文本栈。本模块
//! 存在的理由即此：外壳只提供"贴在哪儿"的纹理句柄，正文内容由本层绘制。

use crate::error::ReaderError;

/// 管线层错误。
///
/// ## 为什么单独一个错误类型而不直接用 `ReaderError`
///
/// 渲染层的失败形态与排版层不同（后端缺失、尺寸非法、GPU 校验失败），而
/// **对外**它们都要映射成同一套 `ReaderError`（FR-058）。分开定义让映射
/// 关系是显式的、可单测的——而不是把渲染细节塞进 `ReaderError` 的变体里
/// 让排版层也看见 GPU 概念。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PipelineError {
    /// 无可用图形后端（CI runner 无 GPU / 驱动缺失 / 后端不支持离屏）。
    ///
    /// **这是"环境不支持"，不是"程序错了"**——故对外映射为能力错误而非
    /// 内部错误（FR-053："图形后端不可用时 MUST 明确报告不可用并允许测试
    /// 跳过，MUST NOT 伪装通过"）。
    #[error("无可用图形后端（adapter）：{reason}")]
    NoAdapter {
        /// 失败原因（**开发者向诊断**，不是面向用户的本地化文案）
        reason: String,
    },

    /// 请求 device/queue 失败（有 adapter 但驱动拒绝）。
    #[error("请求图形设备失败：{reason}")]
    DeviceRequest {
        /// 失败原因（**开发者向**）
        reason: String,
    },

    /// 纹理尺寸非法（0 或超出 `max_texture_dimension_2d`）。
    ///
    /// spec §Edge Cases："必须在接受前校验并拒绝……不得产生无法渲染的状态"。
    /// 零宽纹理在 wgpu 里是校验错误，但那时报的是 GPU 内部消息；本层在建
    /// 管线之前就拒绝，给出**指向参数**的诊断。
    #[error("纹理尺寸非法：{width}×{height}（{detail}）")]
    InvalidExtent {
        /// 请求宽
        width: u32,
        /// 请求高
        height: u32,
        /// 为何非法（**开发者向**）
        detail: &'static str,
    },

    /// GPU 校验/映射失败（读回路径的运行期错误）。
    #[error("离屏读回失败：{detail}")]
    Readback {
        /// 失败细节（**开发者向**）
        detail: String,
    },
}

impl PipelineError {
    /// 本错误对应的**能力标识**（`None` 表示这不是"能力不可用"）。
    ///
    /// ## 为什么只有 `NoAdapter` 返回能力标识
    ///
    /// 能力标识的语义是"**这台机器/这个构建**没有这项能力"（FR-053 的跳过
    /// 口径）。`DeviceRequest` 与 `Readback` 是运行期故障，`InvalidExtent`
    /// 是调用方传错参数——三者都不该让上层"跳过"（那会掩盖真实缺陷）。
    #[must_use]
    pub fn capability_id(&self) -> Option<&'static str> {
        match self {
            Self::NoAdapter { .. } => Some(crate::capability::OFFSCREEN_RENDERING),
            Self::DeviceRequest { .. } | Self::InvalidExtent { .. } | Self::Readback { .. } => None,
        }
    }

    /// 映射为本层的统一错误面（FR-058）。
    #[must_use]
    pub fn to_reader_error(&self) -> ReaderError {
        match self.capability_id() {
            Some(capability) => {
                ReaderError::unsupported_capability(capability).with_detail(self.to_string())
            }
            None => ReaderError::underlying(tome_core::ErrorKind::Content, self.to_string()),
        }
    }
}

/// 可用的图形后端（含适配器名与后端种类）。
///
/// ## 为什么把适配器名一起带着
///
/// CI 的后端矩阵里"真有 GPU"与"lavapipe/WARP 软件光栅"在**结果**上可能
/// 都可用，但在**证据**上必须区分：单基线快照策略的成立性依赖"跑的是哪个
/// 光栅化器"（research R4 §4）。故探针必须能打印它，而不是只知道"有后端"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicsBackend {
    /// 后端种类名（如 `Vulkan` / `Dx12` / `Metal` / `Gl`）
    pub kind: String,
    /// 适配器名（如 `AMD Radeon(TM) Graphics` / `llvmpipe`）
    pub adapter_name: String,
    /// 是否为软件（软件光栅化器没有真 GPU 的浮点差异，快照更稳）
    pub is_software: bool,
}

impl GraphicsBackend {
    /// 后端名（供 CI 日志与探针打印）。
    #[must_use]
    pub fn name(&self) -> &str {
        &self.kind
    }

    /// 适配器名。
    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }
}

impl std::fmt::Display for GraphicsBackend {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} ({}{})",
            self.kind,
            self.adapter_name,
            if self.is_software {
                ", 软件光栅"
            } else {
                ""
            }
        )
    }
}

/// 离屏渲染管线（device + queue，无 surface）。
pub struct RenderPipeline {
    device: wgpu::Device,
    queue: wgpu::Queue,
    backend: GraphicsBackend,
    width: u32,
    height: u32,
}

impl std::fmt::Debug for RenderPipeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderPipeline")
            .field("backend", &self.backend)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl RenderPipeline {
    /// 探测本机可用的图形后端（**不建** device）。
    ///
    /// ## 为什么探针与建管线分开
    ///
    /// CI 的 probe 步骤只要"有没有后端"这一个问题的答案，而建 device 比
    /// 取 adapter 慢且可能因 limits 不合而失败。分开后 probe 是一个廉价、
    /// 不会因别的因素失败的动作——它失败就**只**说明没有后端。
    pub fn probe_backend() -> Result<GraphicsBackend, PipelineError> {
        Self::probe_backend_with(wgpu::Backends::all())
    }

    /// 限定后端集探测（FR-053 的**可失败判据**载体）。
    ///
    /// ## 为什么需要这个入口
    ///
    /// "无后端时如实报告"这条判据若只能用"本机没有 GPU"来触发，就**无法被
    /// 验证**——开发机有 GPU，CI 有 lavapipe，任何一处都走不到那条分支。
    /// 那就成了 spike §10.1 记的"全绿但没测到"的同一种形态。
    ///
    /// 传 `Backends::empty()` 即可**确定性地**走到 `NoAdapter`，从而让
    /// "诚实报告"路径有真实断言覆盖（见本模块测试
    /// `empty_backend_set_reports_no_adapter`）。
    pub fn probe_backend_with(backends: wgpu::Backends) -> Result<GraphicsBackend, PipelineError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let adapter = poll_once(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            // `false`：允许软件光栅（lavapipe/WARP）——CI 正是靠它们
            force_fallback_adapter: false,
            // **无 surface**：离屏路径的全部要点（FR-036/FR-052）
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|reason| PipelineError::NoAdapter { reason })?
        .map_err(|error| PipelineError::NoAdapter {
            reason: error.to_string(),
        })?;

        let info = adapter.get_info();
        Ok(GraphicsBackend {
            kind: format!("{:?}", info.backend),
            adapter_name: info.name,
            // `DeviceType::Cpu` 即软件光栅化器（llvmpipe 报 Cpu）
            is_software: matches!(info.device_type, wgpu::DeviceType::Cpu),
        })
    }

    /// 建离屏管线（给定输出尺寸）。
    ///
    /// ## 尺寸校验在建 device **之前**
    ///
    /// 零宽/超大尺寸是调用方参数问题，报 `InvalidExtent` 给出**指向参数**的
    /// 诊断。若先建 device 再建纹理，报的会是 wgpu 的校验消息（提到
    /// `TextureDescriptor` 与若干内部字段），对定位参数错误帮助不大。
    pub fn new(backend: GraphicsBackend, width: u32, height: u32) -> Result<Self, PipelineError> {
        if width == 0 || height == 0 {
            return Err(PipelineError::InvalidExtent {
                width,
                height,
                detail: "宽高必须为正",
            });
        }

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());

        let adapter = poll_once(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|reason| PipelineError::NoAdapter { reason })?
        .map_err(|error| PipelineError::NoAdapter {
            reason: error.to_string(),
        })?;

        // 尺寸上限来自**本机适配器**的 limits（不同后端上限不同，硬编码会
        // 在一个平台上通过、另一个上以 GPU 校验错误失败）
        let limits = adapter.limits();
        let max_dimension = limits.max_texture_dimension_2d;
        if width > max_dimension || height > max_dimension {
            return Err(PipelineError::InvalidExtent {
                width,
                height,
                detail: "超出本机 max_texture_dimension_2d",
            });
        }

        let (device, queue) = poll_once(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("tome-reader-offscreen"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .map_err(|reason| PipelineError::DeviceRequest { reason })?
        .map_err(|error| PipelineError::DeviceRequest {
            reason: error.to_string(),
        })?;

        Ok(Self {
            device,
            queue,
            backend,
            width,
            height,
        })
    }

    /// 输出宽度（px）。
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 输出高度（px）。
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// 所用后端。
    #[must_use]
    pub fn backend(&self) -> &GraphicsBackend {
        &self.backend
    }

    /// 设备句柄（供 glyph_cache / atlas / offscreen 使用）。
    #[must_use]
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// 队列句柄。
    #[must_use]
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}

/// 把 wgpu 的异步请求驱动到就绪（**不引 async 运行时**）。
///
/// ## 为什么用空 waker 单次 poll
///
/// `request_adapter` / `request_device` 在原生后端上是"立即就绪或立即失败"
/// 的（无外部事件源参与）。故一次 poll 足够；若返回 `Pending`，说明该后端
/// 需要事件循环驱动——那是**本层不支持**的形态（离屏路径必须无窗口），
/// 如实报错好过挂起。
///
/// 这与 spike T009 的探针同款做法（`spikes/s2/src/offscreen_probe.rs`），
/// 已在该处实测可用。
fn poll_once<F: std::future::Future>(future: F) -> Result<F::Output, String> {
    use std::task::{Context, Poll, Waker};

    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(output) => Ok(output),
        Poll::Pending => Err("异步请求未在一次 poll 内就绪（该后端需要事件循环驱动）".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 零尺寸在建 device **之前**被拒（不依赖有无 GPU）。
    #[test]
    fn zero_extent_is_rejected_without_backend() {
        let error = RenderPipeline::new(
            GraphicsBackend {
                kind: "Test".to_owned(),
                adapter_name: "test-adapter".to_owned(),
                is_software: true,
            },
            0,
            100,
        )
        .expect_err("零宽应被拒");
        assert!(matches!(error, PipelineError::InvalidExtent { .. }));
    }

    /// 能力标识的映射关系（FR-053/FR-058 的核心）。
    #[test]
    fn only_no_adapter_maps_to_capability() {
        let no_adapter = PipelineError::NoAdapter {
            reason: "no gpu".to_owned(),
        };
        assert_eq!(
            no_adapter.capability_id(),
            Some(crate::capability::OFFSCREEN_RENDERING)
        );

        // 其余三类是**故障/参数错**，不该被上层当作"能力不可用"跳过
        for not_capability in [
            PipelineError::DeviceRequest {
                reason: "denied".to_owned(),
            },
            PipelineError::InvalidExtent {
                width: 0,
                height: 0,
                detail: "零",
            },
            PipelineError::Readback {
                detail: "map failed".to_owned(),
            },
        ] {
            assert_eq!(
                not_capability.capability_id(),
                None,
                "{not_capability:?} 不该映射为能力错误（那会让上层静默跳过真实故障）"
            );
        }
    }

    /// 映射到 `ReaderError` 时能力错误带标识、其余走 underlying。
    #[test]
    fn reader_error_mapping_is_structured() {
        let capability_error = PipelineError::NoAdapter {
            reason: "no gpu".to_owned(),
        }
        .to_reader_error();
        assert_eq!(
            capability_error.kind().message_id(),
            "error.unsupported_capability"
        );

        let readback = PipelineError::Readback {
            detail: "map failed".to_owned(),
        }
        .to_reader_error();
        assert_ne!(
            readback.kind().message_id(),
            "error.unsupported_capability",
            "读回失败不是能力问题，不该报成可跳过的能力错误"
        );
    }

    /// **`NoAdapter` 路径可达**（否则 FR-053 的"诚实报告"无法被验证）。
    ///
    /// ## 这条测试为什么重要
    ///
    /// 开发机有 GPU、CI 有 lavapipe——"没有后端"这条分支在正常环境里**永远
    /// 走不到**。若它不被测，"无后端时如实报错"就只是一句没被执行过的代码。
    /// 传空后端集让这条路径确定性地被触发。
    #[test]
    fn empty_backend_set_reports_no_adapter() {
        let result = RenderPipeline::probe_backend_with(wgpu::Backends::empty());
        let error = result.expect_err("空后端集不该拿到适配器");
        assert!(
            matches!(error, PipelineError::NoAdapter { .. }),
            "应报 NoAdapter，实测 {error:?}"
        );
        assert_eq!(
            error.capability_id(),
            Some(crate::capability::OFFSCREEN_RENDERING),
            "无后端必须映射为 offscreenRendering 能力标识"
        );
    }

    /// 后端显示含软件光栅标记（CI 日志要能一眼分辨真 GPU 与 lavapipe）。
    #[test]
    fn software_backend_is_marked_in_display() {
        let software = GraphicsBackend {
            kind: "Vulkan".to_owned(),
            adapter_name: "llvmpipe".to_owned(),
            is_software: true,
        };
        let rendered = software.to_string();
        assert!(rendered.contains("llvmpipe"));
        assert!(rendered.contains("软件光栅"));
    }
}
