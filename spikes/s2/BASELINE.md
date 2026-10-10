# S2-0 可行性基线（T005-T009）

**特性**: 002-typesetting-renderer | **测量日期**: 2026-10-10 | **测量机**: Windows 10+ / x86_64-pc-windows-gnu

本文件是 S2-0 可行性门的**证据记录**：tasks.md 002 T005-T009 的验收数据与结论。
上游 `research.md` 的 R5 七项「遗留待办」在此闭环——每项要么有实测数据，
要么明确标为"遗留"并写明原因。

> **数据来源说明**：所有数字均为**本机实测**，命令与原始输出见各节。
> 未实测的项明确标注为"遗留"，不以推断冒充实测（对齐 001 `spikes/s0/BASELINE.md` 的纪律）。

---

## 1. 环境与工具链

| 项 | 值 |
|---|---|
| OS | Windows（10+） |
| 工具链 | `cargo 1.99.0` / `rustc 1.99.0`（stable-x86_64-pc-windows-gnu） |
| 构建 profile | 继承根 `Cargo.toml` 的 `[profile.release]`：`lto="fat"`、`codegen-units=1`、`strip=true`、`panic="abort"` |
| spike 落点 | `spikes/s2/`（`tome-s2-spike`，**刻意脱离主 workspace**，见其 `Cargo.toml` 文件头） |

**spike 依赖**：`cosmic-text 0.19.0`（`std` + `swash` + `fontconfig`）/ `wgpu 30.0.1` /
`swash 0.2.10` / `etagere 0.2.15` / `unicode-segmentation 1.12`。

**复跑命令**：

```bash
cd spikes/s2 && cargo test              # T005/T006 的判据（断言即结论）
cd spikes/s2 && cargo test -- --ignored --nocapture   # 诊断探针（打印结构）
```

---

## 2. R5-1：ShapeWord 合并行为（T005）—— **✅ 证实**（方案 A 成立）

### 2.1 结论

**research.md R1 的方案 A 成立**：`Buffer::line_shape()` 取 `ShapeLine` 克隆体 →
合并相邻 `ShapeWord`（拼接 `glyphs`）→ `layout()`，**断点确实遵守合并边界**。
S2-1 的禁则实现可以按原计划建在这条路径上。

### 2.2 判据与证据

| 测试 | 判据 | 结果 |
|---|---|---|
| `t005_baseline_unmerged_breaks_between_words` | 基准：未合并的中文应以每 3 字断行 | ✅ 实测 `[0..9, 9..18, 18..27, 27..30]` |
| `t005_word_merge_restricts_line_breaks` | **主判据**：合并 word[2,4) 后，「丙丁」须同行且断点位移 | ✅ |
| `t005_merge_does_not_lose_glyphs` | 合并不丢字形、不改原文区间序列 | ✅ |
| `t005_merged_word_width_is_sum_of_parts` | 合并后宽度 = 各部分之和（无隐形间隙） | ✅ |
| `t005_decorations_survive_merge` | decorations 数量与 layout 输出均不被合并破坏 | ✅ |
| `t005_merged_layout_covers_all_text` | 合并后逐行区间覆盖到原文末尾、无重叠倒退 | ✅ |
| `t005_glyph_wrap_ignores_word_boundaries` | `Wrap::Glyph` 切词 / `Wrap::Word` 不切（对照事实） | ✅ |

**主判据的实测值**（复跑命令：`cargo test probe_merge_shifts_break -- --ignored --nocapture`）：

```
未合并 lines  = [0..6, 6..15, 15..24, 24..30]   ← 合并 word[2,4) 之后
   合并后 word 区间 = [0..3, 3..6, 6..12, 12..15, ...]
```

对照基准（不合并）为 `[0..9, 9..18, 18..27, 27..30]`——每 3 字一行。
合并 word[2,4)（「丙」「丁」）后，第 1 行只能放「甲乙」（`0..6` = 2 字身），
**{丙,丁} 整体下移**。断点确实随合并而移动 → 合并是有效的断行控制手段。

### 2.3 判据本身的可证伪性（防"假绿"）

主判据若只断言"word 内部无断点"，在 `Wrap::Word` 下会**平凡通过**——
因为实测显示 `Wrap::Word` 对超宽 word 是**溢出而非切分**，该量恒为 0。
故判据改为**直接比较断点位置**（合并后断点必须位移，且合并对必须同行）。

**证伪路径已实测有效**：把 `merge_words` 换成"只设 `blank` 不拼接 glyphs"
的伪实现后，`t005_word_merge_restricts_line_breaks` **失败**（exit 1）；
还原后复绿。这证明判据不是空转的。

### 2.4 ⚠️ 由此发现的**实现约束**（S2-1 必须遵守，否则禁则失效）

实测量化（`probe_word_or_glyph`，长西文单词 `supercalifragilisticexpialidocious`
在 60px 版心下，单词自身宽 268.2px）：

| `Wrap` | 结果 |
|---|---|
| `Word` | **1 行，宽 268.2px**（溢出容器，**不切词**） |
| `WordOrGlyph` | 7 行，退化为按字形断行 |
| `Glyph` | 6 行，无条件按字形断行 |

**两个必须记住的推论**：

1. **禁则后处理必须在 `Wrap::Word` 下做**。`WordOrGlyph` 看似"更宽容"，
   但长西文单词会走 glyph 路径，而那条路径**无视 word 边界**——合并 word
   的禁则手段对它失效。
2. **FR-009 的"超长单词退化为按字符断行"不能靠 `WordOrGlyph` 获得**。
   它是整词/字形两套逻辑的**同一次调用内的隐式切换**，S2-1 无法控制切换点。
   须由 S2-1 自己判定"该词是否长于整行"，再显式选择 wrap（或自行按字形切）。

> 这两条已作为 S2-1 design 输入记录于此。它们**不**改变方案 A 的成立性，
> 但改变了它的**使用方式**——故属于本 spike 的核心产出之一。

### 2.5 中文的 word 切分粒度（实测）

`probe_word_structure` 输出（节选）：

```
--- 纯中文-窄 (width=60) ---
  word[0] blank=false w=19.0 text="甲"
  word[1] blank=false w=19.0 text="乙"
  ...
  lines = [0..9, 9..18, ..., 63..66]
```

**每个汉字是独立 word**（各 19px = 1 字身）——这正是中文排版需要的断行粒度：
断点自然落在字与字之间。标点则与前面的字**绑成多字 word**：

```
--- 中文标点 (width=60) ---
  word[3] blank=false w=38.0 text="点，"
  word[5] blank=false w=38.0 text="缩。"
```

即 unicode-linebreak 已把「点，」「缩。」各自作为一个 word——**这本身就是
UAX#14 的基础避头尾**（research R1 已指出）。故自研规则确实只需做**补丁**
（破折号对、悬挂、strict/loose 变体、挤压），不需要重写整张规则表。

---

## 3. R5-2：hinting 对挤压量的量化影响（T006）—— **✅ 证实**（默认关 hinting 成立）

### 3.1 结论

**"hinting 会吃掉亚像素挤压量"成立**，默认关 hinting 的决策得到数据支撑。
但**机制与 research R1 的描述有出入**，且**风险面比预期小**——见下。

### 3.2 机制修正（本 spike 的关键产出之一）

research R1 说「hinting 关闭，否则亚像素挤压量被整像素量化吃掉」，
并引 `x_advance.round()` 为据。实测发现**量化发生在 layout 层**，
而**挤压改写点（shape 层）不受影响**：

| 层 | 字段 | 单位 | hinting 开 | hinting 关 |
|---|---|---|---|---|
| shape（改写点） | `ShapeGlyph::x_advance` | **em** | 0.5229492 | 0.5229492（**完全相同**） |
| layout（断行依据） | `LayoutGlyph::w` | px | **10.0**（全整像素） | **9.936035**（亚像素保留） |

源码依据（cosmic-text 0.19.0 `shape.rs`）：

```ignore
// 2896-2912：先乘字号转 px，再量化
let mut x_advance = glyph_font_size.mul_add(glyph.x_advance, ...);
if hinting == Hinting::Enabled {
    x_advance = x_advance.round();      // ← 量化在这里
}
```

**含义**：

1. **好消息**：S2-1 的挤压改写（写 `ShapeGlyph::x_advance`，em 单位）在开/关
   hinting 下写入同一个值——改写动作本身不被 hinting 干扰。
2. **坏消息仍在**：改写后的 em 值在 `layout()` 里乘字号后会**被 round**，
   故挤压想表达的亚像素宽度（半字身 9.5px）在开 hinting 下表达不出来。

### 3.3 量化量级实测

| 量 | 值 |
|---|---|
| layout 层最大单字形量化偏差 | **0.3984px** |
| 半字身（19px 字号的挤压单位） | 9.50px |
| **偏差 / 半字身** | **4.19%** |
| 关闭 hinting 时非整像素字形数 | 7 / 27 |

单字形示例（半角 `g`，天然约 0.59em）：

```
hinting 关 = 11.18848px   ← 亚像素精度保留
hinting 开 = 11.00000px   ← 被 round
  量化偏差 = 0.18848px（相对半字身 2.0%）
```

### 3.4 判据清单

| 测试 | 判据 | 结果 |
|---|---|---|
| `r5_2_shape_layer_is_hinting_independent` | shape 层开/关完全相同 | ✅ |
| `r5_2_layout_layer_is_quantized_by_hinting` | 开 hinting 时 layout 宽度全为整像素 | ✅ |
| `r5_2_subpixel_is_preserved_when_hinting_disabled` | 关 hinting 时存在非整像素宽度 | ✅ |
| `r5_2_quantization_delta_is_quantified` | 偏差 ≥ 0.1px（支撑"关"的决策） | ✅ 实测 0.3984px |
| `r5_2_half_em_squeeze_is_quantized_when_hinting_on` | 亚像素字身被量化 | ✅ 11.188 → 11.000 |
| `r5_2_hinting_may_change_break_positions` | 断点是否随 hinting 变 | 本用例**未变**（31/8 vs 31/8） |

> **判据载体的两次修正**（记录以免后人重踩）：初版用孤立的 `，`——它的步进
> 是整字身 19px（独立标点占满一字）；二版用 `测试标点，压缩。`——8 个字形
> **全是 19px**，因为**全角标点的天然步进就是 1em**，挤压是 S2-1 要**施加**
> 的改写而非 cosmic-text 的既有行为。最终载体取天然带亚像素步进的半角字形。

### 3.5 对 S2-1 的输入

- **hinting 默认关闭**（数据支撑：偏差达半字身挤压单位的 4.19%）
- 挤压改写本身与 hinting 无关，**无需**在改写点做额外处理
- 断点位置在本用例下未随 hinting 变，但**不能据此推断安全**——
  量化偏差 0.4px 在临界宽度下足以翻转一个断点，属需在 S2-1 用真实语料
  回归的风险点（已记入 §12 遗留）

---

## 4. R5-7：贪心换行 + 挤压的排版质量（T006）—— **✅ 有实质发现**

### 4.1 结论

**贪心（first-fit）在纯中文下最优，在混排下确有次优断点**——这是本 spike
观测到的、research R5-7 预期中的现象，其量级如下。

### 4.2 判据与在案的实测数据

**次优断点的口径（两次修正后）**：一行排完后，**下一个 word 本可放进本行**
却未被放入。这是可判定的真次优，不依赖留白阈值猜测。

> 初版口径是"非末行留白 ≥ 1 整字身"。实测发现它**系统性高估**：行末下一个
> word 常是长西文词（如 `position` 59px），留白 20px 时它放不下（不是次优），
> 却被旧口径误判。故改用"下一个 word 的实际宽度"直接判定。

| 语料 | 版心 | 行数 | 次优率 |
|---|---|---|---|
| 等宽中文（60 字） | 3 字身（57px，整除） | 22 | **0.0%** |
| 等宽中文（60 字） | 300–437px（非整除） | 4–6 | **0.0%** |
| 等宽中文（20 字） | 4 字身（76px） | 5 | **0.0%** |
| 混排（中英） | 320px | 4 | **33.3%** |
| 混排 | 361px | 4 | **66.7%** |
| 混排 | 400px | 3 | **50.0%** |
| 混排 | 437px | 3 | **0.0%** |
| 混排 | 480px | 3 | **0.0%** |

### 4.3 混排次优的**具体证据**（`probe_mixed_break_detail`，版心 320px）

```
line[0] w=301.9 留白=18.1 (6%)  文本="阅读 reader 与排版 typesetting 的关"
line[1] w=318.0 留白= 2.0 (1%)  文本="系 relationship 在于字形 glyph 的度量"
line[2] w=248.2 留白=71.8 (22%) 文本="metric 决定了断行 break 位置"     ← 次优
line[3] w=210.7 留白=109.3(34%) 文本="position 的优劣 quality。"        ← 末行，不算
```

`line[2]` 是**真次优**：下一个 word 是 `position`（59px），
`248.2 + 59 = 307.2 ≤ 320` —— **它本可放进本行**，贪心却推到了下一行。
随后 `position` 那句又留下 109.3px 空白。这是 first-fit 不回溯的直接代价：
它只顾"当前行能否再多塞一个"，不顾"整体是否更均匀"。

### 4.4 判据清单

| 测试 | 判据 | 结果 |
|---|---|---|
| `r5_7_uniform_cjk_has_no_missed_fit` | 等宽中文整除版心下次优率 = 0 | ✅ |
| `r5_7_non_integral_measure_has_no_missed_fit_for_cjk` | 非整除版心下中文仍 0 | ✅ |
| `r5_7_mixed_script_does_have_missed_fits` | 混排**确有**次优（>0） | ✅ 实测最差 66.7% |
| `r5_7_greedy_fills_lines_for_uniform_cjk` | 等宽中文非末行填满（留白 0） | ✅ |
| `r5_7_greedy_does_not_backtrack` | 贪心至少填满一行（first-fit 特征） | ✅ |
| `r5_7_word_granularity_is_unchanged` | word 粒度自检（中文逐字） | ✅ |

### 4.5 对 S2-1 的输入（含范围声明）

- **纯中文场景无需担心**：贪心在等宽中文下 0% 次优（这正是本项目的主场景）
- **混排场景的注意点**：次优出现在"行末紧跟长西文单词"时，本质是
  "整词回退"与"行均齐"的取舍。**混排次优率的量化评测依赖两端对齐（FR-011）
  尚未实现**——当前测的是 `Align::Left` 下的留白分布；实现 justify 后需重测
- **不建议引入 Knuth-Plass**：research R1 的备选 D（icu_segmenter 作断点裁判）
  与全局重排都不解决"整词回退"这一根因；tasks.md 的既有交代是
  "如出现不可接受的次优断点，备选 = 在 justify 两段式中做局部回退重排
  （不引入 Knuth-Plass）"——本 spike 的数据支持这一范围的判断
- **是否可接受属产品判断**：33–67% 的数字看着高，但它是"行数"口径（3–4 行的
  短段落里 1 行次优即为 33%）；长段落下方差会被摊薄。**未做长语料评测，
  记入 §12 遗留**

---

## 5. R5-3：cosmic-text 全栈体积净增量（T007）—— **✅ 远低于 20MB 基线**

### 5.1 实测数据（release profile 继承根 `Cargo.toml`）

| 臂 | feature | 二进制字节数 | MiB |
|---|---|---|---|
| 空壳 | （无） | 290,816 | 0.28 |
| 排版栈 | `cosmic-text-stack` | 1,338,880 | 1.28 |
| 渲染栈 | `render-stack` | 4,859,904 | 4.63 |
| 全栈 | `full-stack` | 5,880,320 | 5.61 |

**净增量**（相对空壳臂，口径见 `spikes/s2/src/size.rs`）：

| 增量 | 字节 | MiB |
|---|---|---|
| cosmic-text 全栈（harfrust+skrifa+fontdb+unicode 表） | 1,048,064 | **1.00** |
| wgpu 全栈（wgpu-core/hal/naga + swash + etagere） | 4,569,088 | **4.36** |
| 两栈合计（full-stack） | 5,589,504 | **5.33** |

**结论**：两栈合计净增 **5.33 MiB**，加上字体子集 2.18 MiB（§8）共约
**7.5 MiB**，距 20 MB 目标基线有充裕余量。R5-3 从"无权威数据"变为
**实测通过**。

### 5.2 ⚠️ 首轮实测的方法论错误（已修正，记录以免重演）

**首轮四臂结果**：`empty` 与 `cosmic-text-stack` **同为 290,816 字节**
——即"接入 cosmic-text 的代价为零"。那是错的。

**根因**：`main.rs` 当时只引用 `size`（恒在）与 `offscreen_probe`
（仅 render-stack）。`cosmic-text-stack` 开启时 `word_merge`/`hinting`
虽被编译进 lib，但**二进制从不触碰它们**，`lto="fat"` 把整条依赖链裁光。
那条臂测的是"什么都没接入"。

**修正**：新增 `stack-touch` 子命令，按 feature **无条件引用**对应模块的
函数并让其参与运行期计算（返回一个由各栈参与的值，防止被常量折叠）。
修正后四臂单调递增，差值有意义。

**一般教训**：对照实验必须让被测对象**真的出现在产物里**——"编译进去了"
不等于"留在二进制里"，LTO 会让"未使用的依赖"完全消失。这与 T005 的
"判据必须可证伪"、T009 的"样本必须真过 GPU"是同一类问题：**测量装置
本身需要被验证**。

---

## 6. R5-4：Windows-gnu + wgpu 构建与离屏初始化（T007）—— **✅ 证实**

### 6.1 结论

**在本机（Windows / stable-x86_64-pc-windows-gnu）实测通过**：
wgpu 30.0.1 编译、`request_adapter` / `request_device`、`Rgba8Unorm` 离屏
纹理、`copy_texture_to_buffer` 读回全链路可用。research R5-4 的顾虑
（"wgpu 官方 CI 只测 msvc"）在本机**未成为问题**。

### 6.2 探针输出（`cargo run --features render-stack -- offscreen-probe`）

```
PROBE OK backend=Vulkan adapter=AMD Radeon(TM) Graphics non_black=2048/2048 (100.00%)
```

退出码 0。**无窗口、无 surface** —— 排版层与渲染层都能在无图形会话下
初始化并产出图像（FR-052/FR-053）。

### 6.3 本机可用适配器（`-- list-adapters`）

| 后端 | 适配器 |
|---|---|
| Vulkan | AMD Radeon(TM) Graphics |
| Dx12 | AMD Radeon(TM) Graphics |
| **Dx12** | **Microsoft Basic Render Driver**（软件光栅化） |
| Gl | AMD Radeon(TM) Graphics |

`Microsoft Basic Render Driver` 是**纯软件实现**，可作 lavapipe 的本机代理
（T009 用到了这一点）。

### 6.4 实测踩到的三个 wgpu 30 API/验证坑（S2-4 会再遇到）

| 现象 | 原因 | 处理 |
|---|---|---|
| `bytes_per_row must be a multiple of 256` | `copy_*_to_buffer` 的硬对齐要求 | `aligned_bytes_per_row()`，并记读回后 `strip_row_padding` |
| `Usage flags ... do not contain COPY_DST` | `copy_buffer_to_texture` 要求目标纹理声明 `COPY_DST` | 纹理用法加 `COPY_DST`（样本用 CPU 生成像素再上传） |
| `buffer is still mapped`（panic） | `unmap` 前未释放 `get_mapped_range` 的视图 | 显式 `drop(mapped)` 再 `unmap()` |

另有 0.19/30 的 API 形态差异（`Buffer::set_text` 不再收 `&mut FontSystem`、
`enumerate_adapters` 返回 Future、`PipelineLayoutDescriptor` 用
`immediate_size` 而非 `push_constant_ranges`、`multiview_mask` 为必填字段），
均已在本 spike 中适配，可直接作为 S2-4 的接线参照。

---

## 7. R5-5：字体子集实产（T008）—— **✅ 达标**

### 7.1 产物

| 项 | 值 |
|---|---|
| 文件 | `assets/fonts/TomeSerifCN-subset.otf` |
| 体积 | **2,284,616 字节（2.18 MiB）** |
| 字形数 | 31,058（原字体） → **7,356**（子集） |
| 字符数 | **6,948**（GB2312 6763 + 标点/西文/排版必备） |
| 清单 | `assets/fonts/FONTS.sha256`（1 行，SHA-256，64 字符） |
| 许可 | `assets/fonts/OFL.txt`（SIL OFL 1.1 全文，随源包分发） |

**相对 research R3 的推断**：R3 估"6500 字档约 2.5MB"、"8105 字全表约 3MB"。
实测 6763 字档为 **2.18 MiB**，比推断小约 13%——CFF 子集化后子程序
共享比线性外推更省。**推断被实测取代**（体积更优）。

### 7.2 字表口径的修正（相对 research R3）

R3 的原话是「通用规范汉字表一级 3500 + 二级（合计 6500）」。实测发现
**通用规范汉字表没有可程序化获取的公开机器可读版本**（2013 年国务院
公布的规范性文件，字表以 PDF 形式发布；网上二手整理版来源不可核对，
违反干净室纪律）。

**改用 GB2312**——等价且更强的选择：

| 口径 | 字符数 | 可程序化枚举 |
|---|---|---|
| 通用规范一级+二级 | 6500 | ✗ |
| **GB2312 一级** | **3755** | ✓ |
| **GB2312 二级** | **3008** | ✓ |
| **GB2312 合计** | **6763** | ✓ |

GB2312 由 Python 标准库的 `gb2312` 编解码器枚举（字节区 0xB0A1–0xF7FE），
**零外部依赖、任何人任何时间可复现同一份字表**。6763 字略多于 R3 规划的
6500，覆盖更广而体积仍达标。

### 7.3 OFL RFN 合规（research R3 的关键）

思源宋体带 **Reserved Font Name「Source」**，OFL 规定修改版不得使用它。
`tools/subset-font.py` 的改名结果（实测）：

| name ID | 原值 | 新值 |
|---|---|---|
| 1 (Family) | `Source Han Serif CN` | **`Tome Serif CN`** |
| 1 (Family, zh) | `思源宋体 CN` | **`Tome Serif CN`** |
| 3 (Unique ID) | `2.003;ADBO;SourceHanSerifCN-Regular;ADOBE` | `TomeSerifCN;ADBO;HanSerifCN-Regular;ADOBE` |
| 4 (Full name) | `Source Han Serif CN` / `思源宋体 CN` | **`Tome Serif CN`** |
| 6 (PostScript) | `SourceHanSerifCN-Regular` | **`TomeSerifCN`** |

**校验范围的一处修正**：初版 verifier 把 name **ID 0（版权声明）与
ID 7（商标声明）**里的 "Source" 也判为违规——那是**错的**。OFL §1 要求
保留版权与许可声明，OFL 的 RFN 条款约束的是"用保留名作**字体标识**"，
不是"不得提及保留名"。故校验范围收窄为标识类名称
（ID 1/4/6/16/18），版权与商标声明**必须**保留原文。修正后：
`[通过] name 表不含保留字体名 "Source"（OFL RFN 合规）`。

### 7.4 OpenType 特性保留

`--layout-features=*` 保留了全部 10 个 GSUB 特性（实测枚举）：
`aalt ccmp dlig fwid hwid liga locl pwid vert vrt2`。

其中 **`fwid`/`hwid`（全角/半角形式）与 `palt`/`halt` 系列**是 FR-006
标点挤压的字体侧支持——丢掉它们会让挤压只能靠 `x_advance` 硬改（T006 已
量化其精度限制）。**`vert`/`vrt2`** 对立排有支持（本项目不做竖排，保留无损）。

### 7.5 ⚠️ 哈希观测的一条环境经验（PM 勘误后记录）

本机在工具输出里**看到的**十六进制哈希恒为全零——那是输出层对形似 token
的长十六进制串的打码，**计算本身正常**。我据此误判为"环境哈希损坏"并
上报了阻塞，属**观测层误判**（PM 已勘误，实测五个工具均输出正确 FIPS 值）。

**规则**：凡需哈希「眼见为实」的场景，一律**直写文件、不在 stdout 展示**；
本任务即按此办理（`FONTS.sha256` 由 Python 写入并只回报布尔校验结果，
PM 在干净上下文独立复核）。

---

## 8. R5-6：单基线快照容差初判（T009）—— **✅ 单基线可行（本机证据）**

### 8.1 结论

**0.6 的 per-pixel YIQ 阈值在本机跨后端场景下有充分余量**：Vulkan（基线）
vs Dx12、Vulkan vs GL 的像素差异**恰好为 0**（最大 YIQ 距离 0.000）。
research R4 的"单基线共享"策略得到本机实证支持。

### 8.2 测量装置的两轮修正（否则结论是假绿）

| 轮次 | 样本路径 | 结果 | 问题 |
|---|---|---|---|
| 首轮 | CPU 生成像素 → `copy_buffer_to_texture` → 读回 | 差异 0% | **GPU 一次算术都没做**，测的是"拷贝无差异" |
| 修正后 | CPU 生成字形位图 → **GPU 着色器点采样 + alpha 混合** → 读回 | 差异 0% | 有意义 |

修正后的样本复现本项目真实渲染路径（research R2/R4：字形由 cosmic-text 的
`SwashCache` 在 **CPU** 上栅格化，GPU 只负责**混合**）。着色器
（`spikes/s2/src/blit.wgsl`）刻意复现三条真实差异来源：点采样
（`textureLoad` 而非 `textureSample`，排除各后端不同的过滤实现）、
标准 source-over `ALPHA_BLENDING`、1:1 像素对齐绘制。

**样本有效性自检**（防"两边都是底色所以差异为 0"）：GPU 混合确实改变了
**35,920 / 350,208 像素（10.25%）**——与真实正文页的"低覆盖率前景"特征相符。

### 8.3 跨后端实测

```
CROSS	Vulkan -> Dx12/Dx12	AMD Radeon(TM) Graphics	差异 0/350208 像素（0.0000%），最大 YIQ 距离 0.000（阈值 0.6）
CROSS	Vulkan -> Gl/Gl   	AMD Radeon(TM) Graphics	差异 0/350208 像素（0.0000%），最大 YIQ 距离 0.000（阈值 0.6）
```

两对后端**逐比特相同**。原因是本项目文字走 **CPU 光栅化 + GPU 仅混合**
的架构（research R4 的原判断）：字形位图由 CPU 生成、上传后无插值点采样、
混合算术本身是 IEEE 精确的——跨后端差异天然趋近于零。

### 8.4 pixelmatch-YIQ 算法的自校验（`spikes/s2/src/pixel_diff.rs`）

算法本身有 10 条单测，其中三条是**已知答案**：

| 测试 | 判据 | 结果 |
|---|---|---|
| `identical_pixels_have_zero_distance` | 同像素距离 = 0 | ✅ |
| `black_versus_white_is_maximal` | 黑白距离 > 0.9 | ✅ |
| `luminance_differences_weigh_more_than_chroma` | 同幅度的亮度差 > 色度差 | ✅ |
| `tiny_perturbation_is_below_threshold` | 1/255 扰动 < 0.6 | ✅ |
| **`per_pixel_gate_catches_few_but_gross_differences`** | 闸门 1 抓"少数像素差得离谱" | ✅ |
| **`ratio_gate_catches_many_small_differences`** | 闸门 2 抓"很多像素各差一点" | ✅ |
| `size_mismatch_reports_total_difference` | 尺寸不等 → 100% 差异 | ✅ |

后两条证明**双闸门不可互相替代**（research R4 的设计前提）。

### 8.5 对 S2-4/T036 的输入与**范围声明**

- 0.6 阈值本机余量充足，**支持单基线起步**；T036 仍按 research R4
  从严取 0.05% 比例闸门
- **未在本机验证的部分（诚实标注）**：
  - 本机只有 AMD 单卡的三后端，**没有 lavapipe / Metal**。R4 的正式矩阵是
    ubuntu(lavapipe) / windows(lavapipe) / macos(Metal)，需 CI 首轮实测
  - 样本是**几何图形模拟的字形位图**，不是真实字形。真字形的抗锯齿边缘
    分布更复杂，差异可能更大——T036 用真实管线复测
  - 本机三后端同为 AMD 驱动栈，**不构成"跨厂商"证据**。单基线策略在
    AMD vs NVIDIA vs Apple 之间是否仍成立，需 CI 证据

---

## 9. R5 七项汇总

| # | 验证项 | 状态 | 结论要点 |
|---|---|---|---|
| 1 | ShapeWord 合并行为 | ✅ **证实** | 方案 A 成立；禁则须在 `Wrap::Word` 下做 |
| 2 | hinting 量化影响 | ✅ 证实 | 量化发生在 **layout 层**；偏差达半字身 4.19%；默认关 |
| 3 | cosmic-text 全栈体积 | ✅ 实测 | 两栈净增 5.33 MiB，远低于 20MB 基线 |
| 4 | Windows-gnu + wgpu | ✅ 证实 | 本机 Vulkan 后端离屏全链路通过 |
| 5 | 字体子集体积与合规 | ✅ 达标 | 2.18 MiB；name 表去 "Source" 合规 |
| 6 | 单基线快照容差 | ✅ 单基线可行 | 跨后端逐比特相同（本机）；CI 三平台待实测 |
| 7 | 贪心换行质量 | ✅ 有发现 | 纯中文 0% 次优；混排 33–67%，justify 后需重测 |

**七项全部有实测数据**（S2-0 检查点第 1 条）。**无一项证伪**——
T005 的备选方案 C 未启动（S2-0 检查点第 2 条）。

---

## 10. S2-0 检查点证据（tasks.md 002 Phase 2 末尾逐条）

| 检查点条目 | 证据 | 判定 |
|---|---|---|
| R5 七项全部有实测数据（BASELINE.md） | §2–§8 各节；§9 汇总表 | ✅ |
| ShapeWord 合并行为证实（否则已上报 PM 并获批备选路径） | §2.2 主判据 + §2.3 证伪路径实测有效 | ✅ 证实，**未启动备选方案 C** |
| 字体子集 ≤5MB 且合规入仓 | §7.1：2.18 MiB；§7.3：name 表去 "Source" 合规 | ✅ |
| **人工审查 spike 数据后再继续** | 本文件即审查对象（PM 复核） | ⏳ **待人工审查** |

**七项逐项结论**（§9 的复述，便于检查点核对）：

| # | 项 | 结论 |
|---|---|---|
| 1 | ShapeWord 合并 | ✅ 证实——方案 A 成立 |
| 2 | hinting 量化 | ✅ 证实——量化在 layout 层，偏差达半字身 4.19% |
| 3 | 体积净增量 | ✅ 实测——两栈 +5.33 MiB |
| 4 | Windows-gnu + wgpu | ✅ 证实——离屏全链路本机通过 |
| 5 | 字体子集 | ✅ 达标——2.18 MiB、RFN 合规 |
| 6 | 单基线容差 | ✅ 单基线可行——本机跨后端逐比特相同 |
| 7 | 贪心换行质量 | ✅ 有发现——纯中文 0% 次优；混排 33–67% |

**无一项证伪** → T005 的备选方案 C（自写 layout 层）**不需要批准，
不需要启动**。

### 10.1 本 spike 的"测量装置自验"清单（三次踩坑的共同教训）

本 spike 的三处主要错误都不是"结论错"，而是**测量装置无效**——
它们都表现为"全绿但没测到东西"：

| 任务 | 装置缺陷 | 现象 | 修正 |
|---|---|---|---|
| T005 | 判据用"word 内部断点数"，而 `Wrap::Word` 对超宽 word 是溢出而非切分 | 该量恒为 0 → 断言平凡通过 | 改为直接比较断点位置；并实测证伪路径有效 |
| T007 | 二进制从不引用 cosmic-text-stack 的模块，`lto="fat"` 整条裁光 | 该臂与空壳同为 290816 字节 | 新增 `stack-touch` 无条件引用并参与运行期计算 |
| T009 | 样本走 `copy_buffer_to_texture` 即读回，GPU 一次算术都没做 | 差异恒为 0% | 改为过 GPU 着色器（点采样 + alpha 混合），并加样本有效性自检（10.25% 像素真的被混合改变） |

**共同教训**：对照实验必须让被测对象**真的出现在结果里**。
"编译进去了"≠"留在二进制里"（LTO 会裁掉未使用的依赖）；
"跑过了"≠"测到了"（数据可能只是过了一遍搬运）。

已固化为可复用的做法：
- 每条判据都要能**实际失败**（T005 用注入伪实现验证了这一点）
- 每个测量都带**有效性自检**（T009 的样本非底色像素计数、T007 的臂间单调性）

---

## 11. S2-1 阶段的追加实测（T013 产出，回填 spike）

### 11.1 禁则合并的**实际效力范围**（重要，影响 T015 的设计）

T013 实现禁则合并后，用"禁用合并 vs 启用合并"的对照实测发现：

| 语料 | 宽度扫描 | 启用合并违规 | 禁用合并违规 |
|---|---|---|---|
| 5 段标点密集中文（引号/逗号/问号/括号/顿号/破折号/省略号） | 8 档（228–418px） | 0 | **0** |
| 破折号对 / 省略号对贴近行边界（24 种 padding × 4 宽度） | 96 组合 | 0 | **0** |

**结论：在实测语料上，合并对断点**没有产生任何可见影响**。

**原因**（原始数据实测，见下）：

```
=== word 切分（"甲乙乙——丙丙丙丙丙丙丙丙丙丙丁", 190px 版心）===
  word0 "甲"  w=19.0
  word1 "乙"  w=19.0
  word2 "乙"  w=19.0
  word3 "——" w=38.0      ← 破折号对**已经是一个 word**
  word4 "丙"  w=19.0
  ...
```

即 `unicode-linebreak` 已经把：
- 中文标点与其**前一个字**绑成多字 word（「点，」= 38px）
- **破折号对与省略号对各自成单一 word**（「——」= 38px）

故 T012 的补丁条目（`em_dash_pair_not_splittable` / `ellipsis_pair_not_splittable`）
在当前依赖版本下**不存在需要修补的断点**——它们的价值是**显式契约**
（若依赖升级改变了切分粒度，补丁条目是兜底），而非当前的生效路径。

### 11.2 这对 SC-002 的判定意味着什么

SC-002 的"违反次数为 0"在本实现下**成立，但主因不是本层的合并**——
而是 cosmic-text 的 word 切分。这一点必须说清楚，否则会读成
"合并在起作用"。

**故 SC-002 的测试不能只断言"违规 == 0"**——那是**空的**判据（禁用合并
也通过）。有效的判据形态是：

1. **独立重算 == 引擎记录**（交叉验证，防核验逻辑失效）
2. **例外可达**（段落以闭标点开头时例外必须产出——证明记录机制接线）
3. **失效模式记录在案**（窄版心下合并单元溢出而非违规）

这三条已全部落进 `tests/kinsoku_engine.rs`。

### 11.3 合并路线的**失效模式是溢出**（不是违规）

实测：版心 9px（< 半字身 9.5px）下，合并后的多字单元无法放进任何一行，
cosmic-text 让它**独自成行并溢出**——产出的行宽可以超过版心。

这与 research R5-1 的结论一致（`Wrap::Word` 对超宽 word 是溢出而非切分），
且是 **SC-003（混排无超宽）** 的关切，不是 SC-002 的。
T014 的挤压与混排退化路径要处理它。

---

## 12. 遗留与边界（诚实标注）

| 项 | 说明 |
|---|---|
| **CI 三平台未实跑** | 本文件全部数据来自**本机 Windows/gnu**。macOS/Linux 的构建与快照需 CI 首轮实测（对齐 001 S0 的同一遗留） |
| **无 lavapipe / Metal 证据** | R5-6 的跨后端结论基于本机 AMD 的三后端；CI 矩阵的正式后端未验 |
| **真实字形未参与快照** | T009 的样本是模拟字形位图；T036 用真实渲染管线复测容差 |
| **混排次优率是短段落口径** | 33–67% 来自 3–4 行段落（1 行次优即 33%）；长段落方差被摊薄，未测 |
| **混排次优依赖 justify** | 当前测的是 `Align::Left` 的留白分布；FR-011 两端对齐实现后需重测 |
| **hinting 与断点的交互** | T006 的本机用例未观察到断点位置随 hinting 变化；0.4px 量化偏差在临界宽度下可能翻转断点，需 S2-1 用真实语料回归 |
| **`skera` 未采用** | research R3 首选 skera（纯 Rust 可在 build.rs 调），但当前生态无稳定可用版本；按 R3 既定备选走 **fonttools**（Python 前置，仅构建期一次性使用，产物入 git 故 CI 零依赖） |

---

## 附：与 001 S0 spike 的组织对照

| 维度 | S0（001） | S2-0（002） |
|---|---|---|
| 落点 | `spikes/s0/`，脱离 workspace | `spikes/s2/`，脱离 workspace（同理由） |
| release profile | 逐项复制根配置 | 逐项复制根配置（T007 体积对照的前提） |
| 结论载体 | `BASELINE.md` | `BASELINE.md`（本文件） |
| 证伪处理 | 无证伪门 | **T005 是证伪门**：证伪即停止上报 PM，备选方案 C 须用户批准 |
| 判据形态 | 命令输出 + 表格 | 断言即结论（`cargo test` 复跑），数据由诊断探针打印 |
