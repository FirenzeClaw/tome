// WGSL 着色器：把 CPU 生成的"字形位图"以 alpha 混合贴到目标纹理上。
//
// ## 为什么必须过一遍 GPU 着色器（T009 首轮的教训）
//
// 首个版本把 CPU 生成的像素直接 `copy_buffer_to_texture` 上传，再
// `copy_texture_to_buffer` 读回。两个后端的结果当然逐像素相同——因为
// **GPU 一次算术都没做**，数据只是过了一遍搬运。那次测量把"拷贝路径
// 无差异"当成了"后端渲染无差异"，是假绿。
//
// ## 本项目的真实渲染路径（research R2/R4）
//
// 字形由 cosmic-text 的 `SwashCache` 在 **CPU** 上栅格化（swash 0.2），
// 产出 alpha 位图；GPU 的职责是**把位图作为纹理贴到页面上**（alpha 混合）。
// 故跨后端差异的真实来源是：
//
//   1. 纹理采样的插值（本项目用 1:1 点采样，无插值）
//   2. **alpha 混合的算术**（`src * srcAlpha + dst * (1 - srcAlpha)` 的实现细节）
//   3. 纹理坐标到像素中心的映射约定（半像素偏移）
//
// 本着色器刻意复现这三条：点采样（`textureLoad`，非 `textureSample`）、
// 标准 source-over 混合、按 1:1 像素对齐绘制。这样"跨后端像素差异"
// 才测的是本项目真实会遇到的差异。

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// 全屏三角形（比全屏四边形少一条对角线，是常见的 offscreen 技巧）
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // 三个顶点覆盖整个裁剪空间：(-1,-1) (3,-1) (-1,3)
    let x = f32(i32(vertex_index & 1u) * 4 - 1);
    let y = f32(i32(vertex_index >> 1u) * 4 - 1);

    var output: VertexOutput;
    output.position = vec4<f32>(x, y, 0.0, 1.0);
    // UV 与裁剪空间坐标的线性映射：(-1,-1) → (0,1)，(3,-1) → (2,1)
    output.uv = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return output;
}

@group(0) @binding(0) var glyph_texture: texture_2d<f32>;
@group(0) @binding(1) var glyph_sampler: sampler;

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // 点采样：取最近纹素（与 swash 位图 1:1 贴合的语义）。
    // 不用 textureSample（那会引入各后端不同的过滤实现）。
    let coords = vec2<i32>(input.uv * vec2<f32>(textureDimensions(glyph_texture, 0)));
    let texel = textureLoad(glyph_texture, coords, 0);
    return texel;
}
