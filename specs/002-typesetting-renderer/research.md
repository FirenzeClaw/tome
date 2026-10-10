# Research: S2 排版渲染层技术选型验证

**Date**: 2026-10-10 | **输入**: spec.md §Assumptions 已批准的技术约束（cosmic-text 排版 + 自研中文规则 + wgpu 自绘管线 + 内嵌正文衬线子集 + 像素快照测试）
**调研方式**: 4 个独立研究代理联网调研（全部结论附官方文档/repo 链接与版本号），本文件合并其结论为 Decision / Rationale / Alternatives 三段式。

---

## R1: cosmic-text 自研中文断行规则接入点

**Decision**: 采用「ShapeLine 克隆 + ShapeWord 合并 + `ShapeGlyph.x_advance` 改写」路径（下称方案 A）：

1. `Buffer::line_shape()` 取出 `ShapeLine` **克隆体**（不改 Buffer 原文）
2. 在克隆体上**合并相邻 `ShapeWord`**（拼接 `glyphs`）实现行首/行尾禁则——wrap 只认 word 边界，合并即禁止断行
3. **shape 后、layout 前改写 `x_advance`**（配合 `x_offset` 调视觉位置）实现标点挤压与中西文间隙——换行结果自动按压缩后宽度计算
4. 两端对齐用两段式：先 `layout(Align::Left)` 拿断点，再在 `LayoutLine` 克隆体上按「标点优先/字间均摊」二次分配余白
5. **hinting 关闭**（否则亚像素挤压量被整像素量化吃掉，且三平台一致性最优）

**Rationale**（全部已证实自 cosmic-text 0.19.0 源码）:
- `ShapeLine/ShapeSpan/ShapeWord/ShapeGlyph` 全部字段 `pub` 且 `#[derive(Clone)]`；`ShapeGlyph.x_advance/x_offset/start/end` 可写（shape.rs:654-1217）
- `ShapeLine::layout()` 是 `pub fn` 且**不需要 FontSystem**（shape.rs:1589）——可无图形环境运行
- 断行发生在 shape 阶段（`unicode_linebreak::linebreaks`，硬依赖无 feature 开关），**无注入点**——故只能在克隆体上做后处理
- glyph 的 `start/end` 是原文字节区间，天然满足「阅读痕迹字符区间」铁律（001 §FR-028）
- **基础避头尾 UAX#14 已覆盖**（`，。`为 CL 类、开括号为 OP 类，LineBreak-18.0.0.txt 已核实）——自研规则是**补丁式**：破折号对（U+2014 B2 类前后都可断）、省略号、悬挂标点、strict/loose 变体，不需要重写整张规则表
- 两端对齐内建 `Align::Justified` 只均摊空格（shape.rs:2851），纯中文行 spaces==0 无效——必须两段式自做

**Alternatives considered**:
- B. 文本预处理插 U+2060/U+200B：改字节偏移须全程维护索引映射（字符区间铁律高风险）；U+2060 在部分字体渲染为豆腐块；**弃**
- C. fork cosmic-text 或 harfrust+unicode-linebreak 自写布局层：重写 bidi/字体回退/缓存约 3000 行，长期追上游成本高；rustybuzz 已 unmaintained（RUSTSEC-2026-0206）只能用 harfrust；**弃**
- D. icu_segmenter（有 strictness/content_locale 可配禁则档位）：可作为方案 A 的「断点裁判」叠加件，但进不了 cosmic-text 注入点，且数据体积未测量；**列为后续可选增强，本期不引入**

**版本钉版**: cosmic-text **0.19.0**（2026-04-22，MSRV 1.89；harfrust 0.5 整形，全栈纯 Rust 无 C 依赖）。WGPU 无关、无窗口系统依赖（架构层面已证实）。

---

## R2: wgpu 阅读页渲染管线（glyphon vs 自研）

**Decision**: **自研字形缓存渲染层**，glyphon 源码（~1.5K 行）作参考架构，不引入 glyphon：

1. 栅格化直接复用 cosmic-text 的 `SwashCache`（底层 swash 0.2.x）——零新增字体解析栈
2. atlas 用 **etagere**（BucketedAtlasAllocator，glyphon 同款）
3. 缓存键 =（字体 + 字号 + 字形 + SubpixelBin 亚像素桶）；滚动场景只在 x 轴亚像素、y 轴对齐行基线取整
4. atlas 增长改用 `copy_texture_to_texture` 整体搬迁（避免 glyphon 式全量重栅格化+逐字形 write_texture）
5. 每帧聚合新字形批量上传，单 encoder 多次 copy

**Rationale**:
- glyphon 主路径**强制 `&cosmic_text::Buffer` 输入**（text_render.rs `prepare()` 遍历 `buffer.layout_runs()`），**无任何注入外部排版结果的公开 API**——与「自研中文规则后处理」硬冲突（已证实，源码级）
- glyphon 缓存不可配（无界 LRU）、grow 全量重传、无 LCD 亚像素 AA、锁定 wgpu 30 + cosmic-text 0.19 双版本——wgpu 每季度一大版本，双重锁版本是长期负担
- iced 已 fork 出走（cryoglyph）证明下游控制排版定制的需求是真实且普遍的
- swash `Render::new([ColorOutline, ColorBitmap(BestFit), Outline])` + `SubpixelBin` 4 分桶与 cosmic-text 一致（官方模块文档已证实）

**Alternatives considered**:
- B. glyphon + `Wrap::None` 自切 BufferLine：排版可控但缓存不可配/版本双锁/grow 卡顿仍在；**弃**
- C. glyphon CustomGlyph 通道：u16 id 空间、无 bearing、形态扭曲；**弃**
- D. fork glyphon：继承其 Buffer 假设需先做手术，不如参考重写；**弃**
- E. wgpu_text（glyph_brush/ab_glyph）：无复杂整形，CJK 阅读器不合格；**弃**
- F. fontdue/ab_glyph 自研栅格：重复字体栈、无彩色 emoji；**弃**

---

## R3: 正文衬线字体内嵌（子集化）

**Decision**: 构建期子集化**思源宋体 CN 区域 OTF**（`14_SourceHanSerifCN.zip`，2.003R，65.8MB 单字重源，避开 TTC），子集产物**入 git 钉版**：

1. 字符集：**通用规范汉字表一级 3500 字 + 二级（合计 6500）+ 常用全角/半角标点 + 基本西文**——实测 8105 字全表子集化 ~3MB（fonttools，TTF-Min 仓库 dist 可复核），6500 字推断 ~2.5MB，均落 3–5MB 预算
2. 工具链：**skera**（harfbuzz subset 纯 Rust 移植，可在 build.rs 调）为首选；产物**一次性生成后连同自算 sha256 + OFL.txt 提交入仓库**——构建期与 CI 零下载、三平台一致、可复现
3. **合规（关键）**：思源宋体有 **Reserved Font Name「Source」**——子集属 Modified Version，**主字体名必须改名**（如 "Tome Serif CN"），随分发附 OFL 全文（许可页 + 资源内文件）；OFL 字体入 git 合法
4. 运行时回退链：**fontdb**（cosmic-text 同栈）注册内嵌子集（`load_font_data`）+ `load_system_fonts()` 系统回退（Windows 宋体/macOS Songti SC/Linux Noto Serif CJK）

**Rationale**:
- Adobe 官方不发布哈希（GitHub API digest 全 null，已实测）——「构建期下载+校验」不可行，入仓库钉版是唯一可复现路径
- 本机 GitHub 直连不稳（用户级 AGENTS.md 已记录），CI 不应依赖 GitHub 可达性
- 3500 字对网文生僻人名/方言字会有可见缺字，6500 字显著改善且仍 ≤3MB——**取 6500 字档**，缺字由回退链兜底

**Alternatives considered**:
- fonttools/pyftsubset（Python 前置）：全部实测数据的生产工具，但引入非 Rust 构建依赖；**作 skera 不可用时的备选**
- hb-subset（C 库封装）：停更近 3 年、需 C 编译链；**弃**
- WOFF2 再省 25-30%：运行期解码支持未核实，OTF 3MB 已达标；**不引入**
- 3500 字档：余量最大但生僻字覆盖弱；**弃，取 6500**

---

## R4: 像素快照测试方案（含 CI 无 GPU）

**Decision**: 离屏渲染 + hand-rolled pixelmatch-YIQ 双闸门比对 + CI 三平台后端矩阵：

1. **离屏流程**：无 surface，渲染到 `Rgba8Unorm` 纹理（`RENDER_ATTACHMENT | COPY_SRC`）→ `copy_texture_to_buffer`（`bytes_per_row` 对齐 256，读回去 padding）→ `map_async` 后必须 `device.poll(wait_indefinitely())`
2. **比对**：hand-rolled pixelmatch-YIQ（egui_kittest 同款算法，~150 行自写，零新依赖）：**per-pixel YIQ 阈值 0.6 + 失败像素比例 ≤0.1%** 双闸门（Playwright `maxDiffPixelRatio` 模式；SC-011 的 0.1% 即此第二闸门）。初期比例从严 0.05%，误报驱动放宽
3. **CI 后端矩阵**：ubuntu = **lavapipe**（`mesa-vulkan-drivers` + `VK_DRIVER_FILES`）；windows = **lavapipe**（mesa-dist-win，与 Linux 共享同一光栅化器利于单基线；备选 WARP 需 NuGet 钉版下载）；macos = **Metal 真 GPU**（macos-14+ runner 自带，禁用 GL——runner-images#9712）
4. **基线策略**：基线 PNG 入 git；**先试行单基线（ubuntu/lavapipe 生成）+ 0.6 YIQ 容差全平台共享**（egui_kittest 已证实该容差可跨 wgpu 后端），实测不够再退每平台一份（Playwright 模式）；CI 失败时上传 `*-actual.png`/`*-difference.png` artifacts（wgpu CI 同款）
5. **诚实模式（对应 spec「不得伪装通过」）**：CI 测试前跑 **probe 探针**（小二进制 request_adapter 打印后端名，失败则 job 直接红）；测试体内 adapter 缺失即 **panic**（诚实失败），绝不静默 return Ok；本地无 GPU 开发机用 env 开关显式跳过
6. **防 flaky**：测试字体打包进 repo（runner 镜像系统字体漂移是最大不稳定源）+ **关 hinting** + 灰度 AA（非 LCD 亚像素）+ glyph 位置量化整像素

**Rationale**:
- wgpu 官方 CI 矩阵已证实三平台无 GPU 测试在 GitHub 免费 runner 全部成立（Windows/DX12=WARP、Linux/Vulkan=lavapipe、macOS=Metal 硬件）
- 本项目文字走 CPU 光栅化（swash）+ GPU 仅 blit/blend 的架构下，跨后端像素差异天然小——单基线策略有实证支撑
- insta-image 是 GPL-3.0 且精确比对无容差，**否决**；nv-flip（wgpu 自家感知度量）列为误报多时的升级选项

**Alternatives considered**: 全 CPU 渲染（tiny-skia）绕过 GPU：位级确定但测的不是真实 wgpu 路径，集成层测不到；**作排版逻辑层补充测试，非替代**。

---

## R5: 遗留待办（全部落到 S2-0 spike 验证，die 早 die 便宜）

| # | 待验证项 | 依据 | 验证方式 |
|---|---|---|---|
| 1 | ShapeWord 合并后 `layout()` 的完整行为（含 decorations/ellipsize 路径） | 源码阅读推断，未实测 | spike 单元：合并 → layout → 断言断点遵守合并边界 |
| 2 | hinting 对 x_advance 挤压的量化影响（`x_advance.round()` 分支） | shape.rs:2935 | spike：同参数开/关 hinting 各排一版对比 |
| 3 | cosmic-text 全栈（harfrust+skrifa+swash+fontdb+unicode 表）release 体积净增量 | 无权威数据 | spike：空壳 vs 接入对照（对齐 001 S0 的 rquickjs 测法） |
| 4 | Windows-gnu + wgpu/lavapipe 构建与运行（wgpu 官方 CI 只测 msvc） | ci.yml 证据 | CI 首个 job 即验证 |
| 5 | 字体子集实际体积（6500 字档）与改名后的 name 表完整性 | 推断 ~2.5MB | spike：skera 实跑产出量体积 + fc-scan/自校验 |
| 6 | 单基线快照在 Metal 真 GPU 上的 YIQ 容差校准（0.6 是否够） | egui UI 场景经验值，整页文字未证实 | CI 首轮实测，不够则退每平台基线 |
| 7 | 贪心换行 + 挤压的排版质量（次优断点出现率） | cosmic-text 是 first-fit 非 Knuth-Plass | 像素快照语料评测 |

---

**结论**: 四项选型的「NEEDS CLARIFICATION」全部消解为可执行决策；无推翻 spec §Assumptions 已批准基线项。全部遗留风险集中于 S2-0 spike 可验证范围内。
