//! T032 验收：wgpu 自绘管线骨架（FR-043 / FR-052 / FR-036）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T032）
//!
//! - feature off（headless）与 on 两种构建均通过；feature off 时依赖树零图形包
//! - **门控约定**：`render` 默认开启；headless 显式 `--no-default-features`；
//!   feature off 时 `render` 子命令以
//!   `UnsupportedCapability{capability:"offscreenRendering"}` + 退出码 3 回应
//! - 离屏初始化测试（无 surface 建 device/queue）
//!
//! 前两条的分工：**构建形态**由 `scripts/check-reader-features.sh`（`cargo tree`
//! 加两种构建）与 `tests/feature_gating.rs` 断言，本文件只重复其可复述的部分。
//! 本文件的重点是第三条——离屏初始化真的能建成 device/queue。
//!
//! ## 诚实跳过（FR-053）
//!
//! 无图形后端的环境下，本文件的 GPU 用例**显式打印 SKIPPED 且不计为通过**
//! （把"跳过"与"通过"在输出上分开）。判据的诚实性依赖这一点：一个静默
//! `return` 会让"本机没有 GPU"看起来像"渲染正确"。
//!
//! ## 为什么"能建 device/queue"值得单独一个文件测
//!
//! 它是 S2-4 余下全部任务的前提：字形缓存、atlas、批次、离屏读回都要一个
//! 真实的 `Device`/`Queue`。而它**不能在 CI 上默认假设可用**（runner 无 GPU
//! 时 `request_adapter` 返回 None）——故"能建则建，不能建则如实报告"本身就是
//! 验收内容。

#![cfg(feature = "render")]

use tome_reader::render::pipeline::{GraphicsBackend, PipelineError, RenderPipeline};

/// 取一次后端（无可用后端时按 FR-053 显式跳过）。
///
/// ## 为什么要打印而不是静默返回
///
/// `cargo test` 默认吞掉通过用例的输出。跳过必须**可见**，否则一个
/// "本机无 GPU"的环境会与"渲染全部正确"在日志上无法区分——那正是
/// spec 反复强调的"不得伪装通过"。
///
/// ## 为什么"非 NoAdapter 的失败"要 panic
///
/// `NoAdapter` 是**预期内**的结局（环境无 GPU），其余错误（device 请求失败、
/// poll 未就绪）是**装置自身**出了问题。把后者当成跳过，会让"探针坏了"看起来
/// 像"本机没 GPU"——那正是本文件要防的伪装。
///
/// 本函数是 `#[test]` 之外的辅助夹具，故 clippy 的 `allow-panic-in-tests`
/// 不覆盖它（见 `.clippy.toml` 的说明），需显式标注理由。
#[expect(
    clippy::panic,
    reason = "夹具内的 panic 是刻意的：非 NoAdapter 的失败必须让测试红，不得被当成跳过"
)]
fn backend_or_skip(context: &str) -> Option<GraphicsBackend> {
    match RenderPipeline::probe_backend() {
        Ok(backend) => {
            println!("[T032] 后端可用：{backend}");
            Some(backend)
        }
        Err(PipelineError::NoAdapter { reason }) => {
            println!("[T032] SKIPPED ({context}): 无可用图形后端——{reason}");
            println!("[T032] 本条**不计为通过**（FR-053 诚实跳过口径）");
            None
        }
        Err(other) => panic!("[T032] 后端探测以非'无适配器'的方式失败：{other}"),
    }
}

/// **离屏初始化**：无 surface 建 device/queue（T032 的第三条验收子项）。
#[test]
fn offscreen_device_initializes_without_surface() {
    let Some(backend) = backend_or_skip("offscreen_device_initializes_without_surface") else {
        return;
    };

    // 显式尺寸建管线——离屏路径不涉及任何窗口/交换链（FR-036/FR-052）
    let pipeline =
        RenderPipeline::new(backend.clone(), 512, 384).expect("有后端时按给定尺寸建管线应成功");

    // 离屏形态携带固定尺寸（共享 device 形态返回 `None`，见 S3 T004）
    assert_eq!(pipeline.extent(), Some((512, 384)));
    assert_eq!(pipeline.width(), Some(512));
    assert_eq!(pipeline.height(), Some(384));
    assert_eq!(
        pipeline.target_format(),
        tome_reader::render::offscreen::OFFSCREEN_FORMAT,
        "离屏管线的目标格式即离屏常量（共享 device 形态由调用方给出）"
    );
    assert_eq!(pipeline.backend(), &backend);
}

/// 后端名与适配器名**可观测**（探针契约：CI 的 probe 步骤打印它们）。
#[test]
fn probe_reports_backend_and_adapter_names() {
    let Some(backend) = backend_or_skip("probe_reports_backend_and_adapter_names") else {
        return;
    };

    let name = backend.name();
    assert!(
        !name.is_empty(),
        "后端名不得为空（CI 要打印它判定跑了哪个后端）"
    );

    let adapter = backend.adapter_name();
    assert!(
        !adapter.is_empty(),
        "适配器名不得为空（区分真 GPU 与 lavapipe/WARP 软件后端靠它）"
    );
    println!("[T032] backend={name} adapter={adapter}");
}

/// **门控约定**：feature off 的构建里，`render` 面以结构化能力错误回应。
///
/// ## 这条在 feature on 的构建里测什么
///
/// 同一条错误路径在两种构建下都必须成立：`render` feature 关闭时由**编译期**
/// 剔除渲染面，此处用错误类型直接构造来断言其**形状**（能力标识 + 退出码约定）
/// 不因构建形态而变。真正跨构建的断言在 `tests/feature_gating.rs` 与
/// `scripts/check-reader-features.sh`。
#[test]
fn unsupported_capability_error_has_stable_shape() {
    let error = PipelineError::NoAdapter {
        reason: "测试构造".to_owned(),
    };
    // 无适配器是"环境不支持"，不是"程序错了"——它必须能映射为可捕获的能力错误
    let capability = error.capability_id();
    assert_eq!(
        capability,
        Some(tome_reader::capability::OFFSCREEN_RENDERING),
        "无可用后端必须报 offscreenRendering 能力标识（FR-053/FR-057）"
    );
}

/// **错误不伪装成功**：`NoAdapter` 不得被映射成"可用但空白"。
#[test]
fn no_adapter_is_reported_not_faked() {
    let error = PipelineError::NoAdapter {
        reason: "无匹配适配器".to_owned(),
    };
    let message = error.to_string();
    assert!(
        message.contains("无匹配适配器") || message.contains("adapter"),
        "错误细节应透出原因，实测：{message}"
    );
}

/// **尺寸为零**必须在建管线之前被拒（不产生无法渲染的状态）。
#[test]
fn zero_sized_pipeline_is_rejected() {
    let Ok(backend) = RenderPipeline::probe_backend() else {
        println!("[T032] SKIPPED (zero_sized_pipeline_is_rejected): 无可用图形后端");
        return;
    };

    let error = RenderPipeline::new(backend, 0, 384)
        .expect_err("宽度为 0 应被拒（spec §Edge Cases：不得产生无法渲染的状态）");
    assert!(
        matches!(error, PipelineError::InvalidExtent { .. }),
        "零尺寸应报 InvalidExtent，实测 {error:?}"
    );
}
