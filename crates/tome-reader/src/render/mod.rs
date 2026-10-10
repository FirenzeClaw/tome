//! [WIP] 渲染管线入口（`render` feature 门控；实体随 S2-4 落地）
//!
//! 本模块是排版层与渲染层的**物理分界**：它在 `render` feature 下才存在，
//! 故 `--no-default-features` 的构建连"能否绘制"这个问题都不必回答——
//! 图形栈在编译期就不在依赖图里（宪法 IV / FR-036）。
//!
//! S2-4 的实体落点（见 `specs/002-typesetting-renderer/plan.md` 的 Project
//! Structure）：`pipeline.rs`（device/queue）· `glyph_cache.rs` · `atlas.rs` ·
//! `batch.rs` · `offscreen.rs` · `animate.rs` · `image.rs` · `fonts.rs`。
//!
//! ## 当前状态
//!
//! 仅提供 feature 门控的**可观测面**（[`backend_name`]），供 T001 的门控断言与
//! T009 的离屏探针复用同一入口。渲染实体实现属 S2-4，不在 S2-0 spike 范围。

/// 当前构建可用的图形后端名。
///
/// ## 为什么先返回静态名而不是真去要 adapter
///
/// 真探针要 `wgpu::Instance::request_adapter`（T009 的 `offscreen_probe` 做这件事，
/// 因为它需要在**有/无后端**两种环境里都给出结论）。本函数是 crate 内的**门控
/// 可观测面**：它回答的是"这个构建里渲染面在不在"，不是"这台机器有没有 GPU"。
/// 两者混起来会让 T001 的门控断言依赖机器环境而 flaky。
///
/// `wgpu` 30 的多后端选择由实例在运行时决定，编译期只知道**候选集**。
#[must_use]
pub fn backend_name() -> &'static str {
    // wgpu 30 一次构建可含多个后端（本 crate 的 features 同时开了 dx12/metal/
    // vulkan/gles，以便同一份 Cargo.toml 三平台通用），具体用哪个由运行时
    // `Instance::request_adapter` 决定——那正是 T009 探针要打印的东西。
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
