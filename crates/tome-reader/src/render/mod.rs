//! [DONE] 渲染管线入口（`render` feature 门控；T032 起落地）
//!
//! 本模块是排版层与渲染层的**物理分界**：它在 `render` feature 下才存在，
//! 故 `--no-default-features` 的构建连"能否绘制"这个问题都不必回答——
//! 图形栈在编译期就不在依赖图里（宪法 IV / FR-036）。
//!
//! ## 模块一览
//!
//! | 模块 | 职责 | 需求 | 状态 |
//! |---|---|---|---|
//! | [`pipeline`] | device/queue + 后端探测（无 surface） | FR-043 / FR-053 | [DONE] T032 |
//! | [`atlas`] | 字形图集区域分配 + 增长簿记 | research R2-2/R2-4 | [DONE] T033 |
//! | [`glyph_cache`] | 字形缓存键 + swash 栅格化 + 聚合上传 | FR-043 / FR-047 | [DONE] T033 |
//!
//! 后续落点（见 `specs/002-typesetting-renderer/plan.md` Project Structure）：
//! `batch.rs`（T034）· `offscreen.rs`（T035）· `animate.rs`（T038）·
//! `image.rs`（T039）· `fonts.rs`（T037）。

pub mod atlas;
pub mod glyph_cache;
pub mod pipeline;

/// 当前构建可用的图形后端名。
///
/// ## 为什么先返回静态名而不是真去要 adapter
///
/// 真探针要 `wgpu::Instance::request_adapter`（[`pipeline::RenderPipeline::probe_backend`]
/// 做这件事，因为它需要在**有/无后端**两种环境里都给出结论）。本函数是 crate 内的
/// **门控可观测面**：它回答的是"这个构建里渲染面在不在"，不是"这台机器有没有 GPU"。
/// 两者混起来会让门控断言依赖机器环境而 flaky。
///
/// `wgpu` 30 的多后端选择由实例在运行时决定，编译期只知道**候选集**。
#[must_use]
pub fn backend_name() -> &'static str {
    // wgpu 30 一次构建可含多个后端（本 crate 的 features 同时开了 dx12/metal/
    // vulkan/gles，以便同一份 Cargo.toml 三平台通用），具体用哪个由运行时
    // [`pipeline::RenderPipeline::probe_backend`] 决定——那正是探针要打印的东西。
    "wgpu (candidates: dx12, metal, vulkan, gles)"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_non_empty_for_probe_output() {
        // 探针契约：CI 的 probe 步骤直接打印这个值，空串会让"后端名可见"
        // 这条验收变成假通过。
        assert!(!backend_name().is_empty());
    }
}
