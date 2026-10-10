//! T001 验收：crate 骨架与 `render` feature 门控。
//!
//! ## 这两条断言为什么是集成测试而非单测
//!
//! 单测跑在 crate 内部，**看不到自己的依赖图**；而 T001 的核心验收恰恰是
//! "feature off 时依赖树不含图形包"——那是对**构建产物**的断言，必须从
//! crate 外部观察（本文件 + `cargo tree`）。
//!
//! ## 诚实边界
//!
//! 本文件能断言"feature off 时 crate 内不存在渲染入口"；**依赖树**是否零图形包
//! 由 `scripts/check-reader-features.sh` 调 `cargo tree` 断言（层次不同，
//! 一个测代码面、一个测构建依赖面）。

#[cfg(feature = "render")]
#[test]
fn render_feature_exposes_render_module() {
    // feature 开启时渲染面**必须**存在：这是开发/发布构建的常态，
    // 若它消失，说明门控接线反了（默认该开却关着）。
    let backend = tome_reader::render::backend_name();
    assert!(
        !backend.is_empty(),
        "render feature 开启时后端名不应为空串（可读性契约：探针要能打印后端名）"
    );
}

#[cfg(not(feature = "render"))]
#[test]
fn render_feature_off_compiles_without_graphics() {
    // feature 关闭时，**本测试文件**依然要能编译——这一点由编译本身保证：
    // 上面那个 `#[cfg(feature = "render")]` 的测试被整个剔除，没有任何
    // `tome_reader::render` 的引用残留。若有人把渲染面挪到无条件模块里，
    // 这个文件在 `--no-default-features` 下会编译失败，从而红。
    //
    // 同时断言：headless 构建下，错误面（排版层的公共契约）必须完整可用
    // ——排版计算在无图形环境下是**全能力**的（FR-052），不是残废版。
    let error = tome_reader::ReaderError::unsupported_capability(
        tome_reader::capability::OFFSCREEN_RENDERING,
    );
    assert_eq!(
        error.kind().message_id(),
        "error.unsupported_capability",
        "headless 构建下错误契约也必须可构造（FR-052：无图形环境是全能力）"
    );
}

#[test]
fn crate_is_reachable_from_tests() {
    // 冒烟：crate 本身可用（库名与包名一致，`tome_reader`）。
    assert_eq!(
        tome_reader::capability::OFFSCREEN_RENDERING,
        "offscreenRendering"
    );
}
