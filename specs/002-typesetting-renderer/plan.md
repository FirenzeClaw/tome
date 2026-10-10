# S2 排版渲染层 实现计划

> **面向 AI 代理的工作者：** 使用 subagent-driven-development（推荐）或 executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。

**目标：** 交付 `tome-reader` 排版渲染层——cosmic-text 整形 + 自研中文排版规则（禁则/挤压/混排/两端对齐）+ 翻页与滚动双形态 + wgpu 自绘渲染管线，分页以字符区间表达、锚点与排版参数解耦，排版计算全程可无图形环境运行。

**架构：** 排版层（headless，依赖仅 tome-core + cosmic-text）与渲染层（wgpu/swash/etagere）严格分层：排版产出页序列/滚动流/位置映射（全部字符区间口径），渲染消费为绘制指令。中文规则经「ShapeLine 克隆 + ShapeWord 合并 + x_advance 改写」注入 cosmic-text（research R1）；渲染为自研字形缓存管线（R2）；正文衬线字体子集入 git 钉版（R3）；像素快照走 lavapipe/Metal CI 矩阵（R4）。

**技术栈：** Rust 1.99 stable-gnu · cosmic-text 0.19（harfrust 整形）· wgpu 30 · swash 0.2 · etagere · fontdb · skera（构建期字体子集化）· 既有 tome-core 接缝（ContentProvider/BookRepository/Theme/ImageRef）

**Branch**: `002-typesetting-renderer` | **Date**: 2026-10-10 | **Spec**: [spec.md](./spec.md)

---

## Summary

S2 是四条子项目线的第二条（S1 引擎内核已完成 P1-P7）。核心交付：**翻页/滚动双形态的中文阅读排版与渲染层**，三大不变量——① 分页与位置一律字符区间（消费 001 §FR-027/028/SC-008）；② 分页只依赖几何参数（主题切换不重排）；③ 排版计算无图形环境可完整运行（宪法 IV）。设计面以 `design-system/MASTER.md`（用户拍板的唯一设计规范）为基线；阅读工具栏只交付状态机与几何（绘制归 S3，FR-034）。范围护栏：不做竖排、不做浏览器内核级排版、不做 S3 外壳页面（spec §Out of Scope）。

## Technical Context

**Language/Version**: Rust 1.99.0（stable-x86_64-pc-windows-gnu；MSRV 声明 1.98 待实测，001 遗留）

**Primary Dependencies**: cosmic-text 0.19.0（整形，含 harfrust 0.5/fontdb 0.24/skrifa/unicode-linebreak 0.1.5/unicode-segmentation）· wgpu 30.0 · swash 0.2（栅格化，经 cosmic-text `SwashCache` 复用）· etagere（atlas 打包）· skera 0.8（构建期字体子集化，备选 fonttools）· 不引入 glyphon（R2：Buffer 强制路径冲突）

**Storage**: 无新增存储。消费 001 契约：`Book.read_config`（ReadingConfig schema 载体）、`BookRepository` 痕迹/进度/记录、`ChapterContent.images`（001 §seams.md 已增补，实现随 001 P8 接线）

**Testing**: `cargo test --workspace`（TDD 铁律）· 像素快照（自研 pixelmatch-YIQ 双闸门，R4）· 排版结构断言（输入→输出）· 参数化不变量验证（锚点矩阵）· CLI 验证器五命令（contracts/cli.md）

**Target Platform**: Windows 10+ / macOS 12+ / 现代 Linux（gnu/macos/ubuntu CI 矩阵）；排版层全 headless

**Project Type**: desktop-app 的库 crate（`tome-reader`）+ CLI 验证器扩展

**Performance Goals**: 翻页 ≤16ms（红线 33）· 首屏 ≤400ms（红线 800）· 重排 ≤150ms（红线 300）· 单章排版内存 ≤ 章节字节 ×8（SC-007~010，分档口径见 spec）

**Constraints**: <20MB 体积目标基线（字体子集 3–5MB + cosmic-text 栈净增量须 spike 实测）· 零遥测零非用户触发网络（字体获取不例外）· `panic = "abort"`（release，不得依赖 catch_unwind）· hinting 关闭（挤压语义与三平台一致性前提，R1/R4）

**Scale/Scope**: 58 FR / 20 SC / 7 用户故事；新 crate `tome-reader` + tome-core 增补（Theme 纯值）+ tome-cli 五子命令；预估 6 切片 ~40 任务

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 判定 | 依据 |
|---|---|---|
| I 兼容性优先 | ✅ 不适用但无违反 | 排版是桌面新增能力，spec §Assumptions 已声明「上游行为不构成本层约束」；唯一兼容性继承：净化顺序不改写（ContentProvider 唯一内容源，FR-004）、字符区间铁律（FR-003） |
| II 干净室纪律 | ✅ | 规则依据为公开排版规范（UAX#14/LineBreak.txt/CLreq 公开资料）与自有设计稿；spec 明确无兼容性基线可对；研究代理调研对象为开源库官方文档（cosmic-text/wgpu/思源宋体），与上游 Kotlin 无关 |
| III 测试先行 | ✅ | 每切片以失败测试开头；像素快照 + 结构断言 + 参数化不变量为三层测试面；SC 全部可度量 |
| IV 简单性与单向依赖 | ✅ 见注 | `tome-reader` 只依赖 `tome-core` 接缝（ContentProvider/BookRepository/Theme/ErrorKind）；**`Theme` 纯值类型放 tome-core 属 DTO 增补非接缝违反**（裁决 OQ#11→D：core 是 DTO 之家，纯值类型零图形依赖，组合根注入实例）；排版层 headless 可完整运行（FR-052） |
| V 安全无例外 | ✅ | 字体获取零网络（FR-051）；渲染后端/字体不可用明确报告（FR-057）；书源侧排版参数经 ReadingConfig 白名单 schema 校验拒绝非法值（Edge Cases） |
| VI 原子提交 | ✅ | 每任务独立 commit；切片内 TDD 微步骤 |
| VII 不审查不合并 | ✅ | 每切片检查点含人工审查；干净室轴在 S2-6 收口复检 |
| VIII 可读性与显式契约 | ✅ | 中文规则集为显式可枚举清单（FR-007）；例外记录可枚举（FR-008）；依据来源成文（research.md/SOURCES.md 既有惯例） |
| 体积纪律 | ✅ | 字体 3–5MB 预算 + cosmic-text 栈净增量 spike 实测 + 快照基线资产 dev-only 不进发布包 |
| 平台承诺 | ✅ | wgpu 自绘（非系统 WebView）；无系统运行时依赖 |
| 范围纪律 | ✅ | 不做 S3 外壳（工具栏仅状态机/几何，FR-034）；竖排/滚动模式取舍均已经裁决记录 |

**无 Complexity Tracking 条目**（无未经裁决的违规）。

## Dependency Graph

实施顺序自底向上；箭头指向被依赖方。

```
tome-core（增补：Theme 纯值类型 + ImageRef 已由 001 P8 接线）
    │
    ├── assets/fonts/（思源宋体子集 + OFL.txt + sha256 清单，入 git）
    │
    ├── tome-reader/text（文本模型：段落切分/字形簇/字符下标规范）
    │       │
    │       ├── tome-reader/rules（中文规则集：禁则表/挤压/混排/硬序/两端对齐）
    │       │       │
    │       │       └── tome-reader/layout（ShapeLine 克隆后处理引擎 → 行盒）
    │       │               │
    │       │               ├── tome-reader/paginate（页序列/单双页判据/书页语义/ReadingConfig）
    │       │               │       │
    │       │               │       ├── tome-reader/scroll（滚动流/视口/双向映射/按需释放）
    │       │               │       │
    │       │               │       └── tome-reader/precalc（分页预计算/增量重排）
    │       │               │
    │       │               └── tome-reader/render（wgpu 管线：字形缓存/atlas/批次/离屏/动效）
    │       │                       │
    │       │                       └── tome-reader/trace（痕迹渲染/命中测试/选择/词典桥）
    │       │
    │       └── tome-reader/toolbar（工具栏状态机与几何——绘制归 S3）
    │
    └── tome-cli（五子命令薄壳：layout / layout-anchor / render / fonts / verify-layout）
            │
            └── CI：三平台构建 + lavapipe/Metal 快照矩阵 + 体积监控
```

**关键方向约束**：render 依赖 paginate/scroll 的产出，**paginate 绝不依赖 render**（headless 铁律）；trace 只消费 tome-core 的字符区间数据与 render 的 PositionMap；toolbar 独立小模块只暴露状态接口。

## Project Structure

### Documentation (this feature)

```text
specs/002-typesetting-renderer/
├── plan.md              # 本文件
├── research.md          # Phase 0 产出（R1-R4 选型 + R5 spike 清单）
├── data-model.md        # Phase 1 产出（13 实体）
├── quickstart.md        # Phase 1 产出
├── contracts/cli.md     # 已存在（spec 阶段产出：五子命令契约）
└── tasks.md             # /skill:speckit-tasks 产出（本计划不创建）
```

### Source Code (repository root)

```text
spikes/s2/                        # S2-0 可行性 spike（一次性，不进主 workspace；对齐 001 S0 模式）
└── src/                          # word 合并行为 / hinting 影响 / 体积对照 / 离屏后端探针

assets/fonts/                     # 字体资产（入 git）
├── TomeSerifCN-subset.otf        # 思源宋体 CN 子集（6500 字档，改名后产物）
├── OFL.txt                       # SIL OFL 1.1 全文（RFN 合规）
└── FONTS.sha256                  # 钉版哈希清单

crates/tome-core/src/
└── model/theme.rs                # 【增补】Theme 纯值类型（色彩/间距/圆角/阴影/动效常量/字体三角色，零图形依赖）

crates/tome-reader/src/
├── lib.rs                        # crate 门面；[DONE]/[WIP] 标注按 AGENTS.md
├── text/
│   ├── model.rs                  # 规范化文本模型（段落→字形簇→字形；字符下标规范，FR-003）
│   └── paragraph.rs              # 段落切分 + 全角空格启发式（FR-001/002）
├── rules/
│   ├── cjk.rs                    # 禁则字符清单（行首/行尾禁则表，FR-005/007）+ 破折号对等补丁
│   ├── squeeze.rs                # 标点挤压（x_advance 改写 + 压缩下限，FR-006）
│   ├── mixed.rs                  # 中西文混排间隙与断行（FR-009）
│   └── justify.rs                # 两端对齐两段式 + 硬序消解（FR-008/011）
├── layout/
│   ├── engine.rs                 # ShapeLine 克隆→word 合并→x_advance→layout 的后处理引擎（R1）
│   ├── linebox.rs                # LineBox 结构（行内区间/基线/压缩量/禁则例外记录）
│   └── style.rs                  # 行内样式最小集合（标题/强调/居中；标题随字号等比，FR-010）
├── paginate/
│   ├── config.rs                 # ReadingConfig schema（几何/呈现子集分离，read_config 序列化，FR-013）
│   ├── page.rs                   # Page（字符区间/左右页归属/留白页标记，FR-012）
│   ├── spread.rs                 # 单双页判据（em 制/阅读区可用宽度/手动覆盖语义回落，FR-014/015）
│   └── booksem.rs                # 书页语义（左偶右奇/章起偶数页/「章节从新页开始」开关，FR-016）
├── scroll/
│   ├── flow.rs                   # 滚动流排布（无分页空白，FR-020/021）
│   ├── viewport.rs               # ScrollViewport（视口区间/可见行盒，FR-021/026）
│   └── map.rs                    # 滚动偏移↔字符位置双向映射（FR-022/040）
├── precalc.rs                    # 分页预计算（可取消/失败无副作用，FR-044）+ 增量重排（FR-045）
├── render/
│   ├── pipeline.rs               # wgpu 自绘管线（设备/队列/通道，FR-043）
│   ├── glyph_cache.rs            # 字形缓存（SwashCache 栅格化 + 亚像素桶键，FR-043）
│   ├── atlas.rs                  # etagere atlas + copy_texture_to_texture 搬迁（R2）
│   ├── batch.rs                  # RenderBatch 绘制指令序列（渲染确定性，FR-048/SC-012）
│   ├── offscreen.rs              # 离屏渲染 + 256 对齐读回（FR-053）
│   ├── animate.rs                # 翻页动效 + reduced-motion 降级（FR-046/032）
│   └── image.rs                  # 图文混排（图片行盒/占位/不被页边界切开，FR-049）
├── trace/
│   ├── render.rs                 # 痕迹区间→可见片段（跨页/跨视口分段，FR-037）
│   ├── hit.rs                    # 命中测试与反查互逆（FR-040）+ 文字选择（FR-038）
│   └── dict.rs                   # 词典查词桥（复用 001 词典求值，FR-041）
├── toolbar.rs                    # 阅读工具栏状态机与几何（隐形/唤出/热区/入口清单，FR-034）
├── footer.rs                     # 页脚（章号/章内页码/剩余时间口径，FR-042）
├── record.rs                     # 阅读记录两量产出与节流（FR-039/SC-013b）
└── keys.rs                       # 键位集 + 栈式 Esc（FR-035）

crates/tome-reader/tests/
├── snapshot.rs                   # pixelmatch-YIQ 双闸门 harness（R4）+ 诚实跳过/probe
├── snapshots/                    # 基线 PNG（入 git，dev-only）
├── anchor_matrix.rs              # SC-004/005/005b 参数化不变量
└── corpus/                       # 排版合成语料（禁则/挤压/混排/对齐各语法点）

crates/tome-cli/src/
└── reader_commands.rs            # 五子命令薄壳（layout/layout-anchor/render/fonts/verify-layout）
```

**Structure Decision**: 落点 `crates/tome-reader/`（AGENTS.md 规划的 workspace 结构第 9 位）；排版（text/rules/layout/paginate/scroll/precalc）与渲染（render/）物理分层——排版模块不依赖 render（headless 铁律由模块边界强制，render 只在 lib.rs 经 feature `render` 暴露，headless 构建可关）；tome-core 仅增补 `model/theme.rs`（OQ#11 裁决），不触碰 001 已有 API。

## Implementation Strategy

### 垂直切片划分

| 切片 | 端到端功能路径 | 规模 | 拆分 |
|---|---|---|---|
| **S2-0 可行性 spike** | R5 全部 7 项验证（word 合并行为/hinting 影响/体积对照/离屏后端/字体子集实产/单基线容差/贪心换行质量）→ `spikes/s2/BASELINE.md` | **M** | 不拆（一次性骨架） |
| **S2-1 排版核心（headless）** | 文本输入 → 段落切分 → 中文规则（禁则/挤压/混排/硬序/两端对齐）→ 行盒序列 → `tome layout` CLI 输出结构 JSON | **L** | ⓪ **错误类型 + →ErrorKind 映射表（S，先行——FR-057/058：`UnsupportedCapability` 标识命名、禁硬编码自然语言、`panic=abort` 下的错误路径设计）** ① text 模型+段落（M）② 禁则表+word 合并（M）③ 挤压+混排（M）④ 两端对齐+硬序+例外记录（M） |
| **S2-2 分页与参数集** | ReadingConfig → 页序列（字符区间）→ 单双页判据/手动覆盖 → 书页语义 → 锚点矩阵验证（SC-004/005）+ 预计算/增量重排 | **L** | ① ReadingConfig+read_config 接线（S）② 页序列+判据（M）③ 书页语义（M）④ 锚点解耦+预计算/增量（M） |
| **S2-3 滚动模式**（FR-020~FR-027） | 同一排版结果 → 滚动流 → 视口区间 → 双向映射 → 翻页⇄滚动切换保锚点 → 按需释放/节流/阅读记录去重 | **L** | ① 滚动流+视口+映射（M）② 切换+重排保锚点（M）③ 释放/节流/记录口径（M） |
| **S2-4 渲染管线 + 字体** | 行盒 → wgpu 绘制（字形缓存/atlas/批次确定性）→ 离屏+像素快照 harness → 字体内嵌+回退链 → 翻页动效 → 图文混排 | **L** | ① 管线+字形缓存+atlas（M）② 批次确定性+离屏+快照 harness（M）③ 字体内嵌+fontdb 回退（M）④ 动效（S）⑤ 图文混排（M，依赖 001 P8 的 images 接线，若未就绪则排到最后） |
| **S2-5 主题 + 痕迹交互** | Theme 入 core+组合根 → 昼夜语义映射/无障碍强制/reduced-motion → 痕迹渲染/选择/命中互逆/词典 → 页脚/工具栏/键位 | **L** | ① Theme 纯值+注入（M）② 痕迹渲染+词表（M）③ 命中/选择/词典（M）④ 页脚+工具栏+键位（M） |
| **S2-6 收口** | CLI 五命令全接线 → 三平台 CI（含 lavapipe/Metal 快照矩阵）→ 性能基线（SC-007~010）→ 体积记录（SC-017）→ 干净室轴+宪章复检 | **M** | 不拆 |

### XS-XL 工作量预估

各切片拆分后为 **S-M 级任务**（预估 ~40 个任务）；无 XL 残留。最大单任务是 S2-4①（渲染管线+字形缓存，M=3-5 文件），其复杂度由 R2 的 glyphon 参考架构（1.5K 行先例）兜底。

### 检查点规划

```
⛔ S2-0 之后（可行性门）：R5 七项全部有实测数据；word 合并/hinting/体积三项任一证伪 → 回 PM 裁决替代路径（不得降级硬闯）
⛔ S2-1 之后（排版正确性门）：SC-001 规则清单逐项判定例反例 100%；SC-002 禁则违反 0；SC-003 混排无超宽；**FR-010 样式组合下标不变（参数化断言）+ 标题字号随 14/19/24/30 等比缩放**
⛔ S2-2 之后（锚点不变量门）：SC-004 锚点矩阵 100%；SC-005 呈现参数零依赖 100%；SC-006 分页不丢不重 100%
⛔ S2-3 之后（形态一致性门）：SC-005b 切换保锚点 100%；SC-006b 滚动流完整 100%；SC-013b 记录跨模式一致 100%；**FR-025 滚动到未缓存区不发起抓取（001 回放/请求计数断言）且以明确占位/加载态呈现**
⛔ S2-4 之后（渲染与快照门）：SC-011 快照 ≤0.1%（或诚实跳过报告）；SC-012 指令序列逐条相同；字体内嵌 3-5MB 实测
⛔ S2-5 之后（体验完整门）：SC-013 痕迹精确 100%；FR-031 无障碍强制项逐项过
⛔ S2-6 之后（S2 完成门）：SC-007~010 性能分档达标；**SC-014 无图形环境全能力可调用 100%**；SC-015 三平台一致；SC-016 零外发；SC-017 体积在案；**FR-057/058 错误分类全覆盖、无面向用户自然语言硬编码**；干净室轴+宪章复检通过
```

**高风险前置**：S2-0（die 早）与 S2-1（中文规则正确性是整个 S2 的地基）排最前；S2-4⑤图文混排依赖 001 P8 接线（外部依赖），放最后并备降级（先以占位行盒实现，接线就绪后补实）。

## Complexity Tracking

无——Constitution Check 全部通过且无需辩解项。

---

## 风险登记（高风险步骤与备选方案）

| 风险 | 等级 | 应对 |
|---|---|---|
| ShapeWord 合并行为未实测（R5-1） | 高 | S2-0 spike 首个验证项；证伪则备选 = 自写 layout 层（harfrust+unicode-linebreak，R1 方案 C），**备选执行前须用户批准** |
| hinting 量化吃掉挤压（R5-2） | 中 | 默认关 hinting；若低 DPI 小字锐度投诉再评估，属体验权衡非阻塞 |
| cosmic-text 栈体积超基线（R5-3） | 中 | spike 实测；超标记录原因（体积是目标基线非硬门槛，宪法附加约束允许） |
| Windows-gnu + wgpu/lavapipe（R5-4） | 中 | CI 首 job 验证；不通行则 Windows 走 WARP（NuGet 钉版） |
| 001 P8 的 `ChapterContent.images` 接线未就绪 | 低 | S2-4⑤ 排最后 + 占位降级路径 |
| 单基线快照容差不够（R5-6） | 低 | 退每平台基线（Playwright 模式），基线体积 ×3 仍可接受 |
