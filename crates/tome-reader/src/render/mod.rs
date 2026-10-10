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
//! | [`batch`] | 绘制指令序列（顺序固定 + 确定性） | FR-048 / SC-012 | [DONE] T034 |
//! | [`offscreen`] | 离屏渲染 + 256 对齐读回（能力错误诚实报告） | FR-053 | [DONE] T035 |
//! | [`pixel_diff`] | pixelmatch-YIQ 双闸门（快照比对算法） | SC-011 | [DONE] T036 |
//! | [`fonts`] | 内嵌子集注册 + 平台回退链（零网络） | FR-050/051/047 | [DONE] T037 |
//! | [`animate`] | 翻页动效档位 + reduced-motion 降级 | FR-046 / FR-032 | [DONE] T038 |
//! | [`image`] | 图文混排（图片行盒 / 不被切开 / 占位） | FR-049 | [DONE] T039 |
//! | [`glyph_atlas`] | 真实字形位图 → 图集 → WGSL 采样绘制 | research R2 | [DONE] 关门波次 |
//!
//! 后续落点（见 `specs/002-typesetting-renderer/plan.md` Project Structure）：
//! `animate.rs`（T038）· `image.rs`（T039）· `fonts.rs`（T037）。

pub mod animate;
pub mod atlas;
pub mod batch;
pub mod fonts;
pub mod glyph_atlas;
pub mod glyph_cache;
pub mod image;
pub mod offscreen;
pub mod pipeline;
pub mod pixel_diff;

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
