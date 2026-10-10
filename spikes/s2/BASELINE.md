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
  回归的风险点（已记入 §10 遗留）

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
  记入 §10 遗留**

---

## 5. 待补章节（T007-T009）

以下各节随对应任务落地后填充。当前状态：**未开始**。

- §6 R5-3 cosmic-text 全栈体积净增量（T007）
- §7 R5-4 Windows-gnu + wgpu 构建与离屏初始化（T007）
- §8 R5-5 字体子集体积与改名合规（T008）
- §9 R5-6 单基线快照 YIQ 容差校准（T009）

---

## 附：与 001 S0 spike 的组织对照

| 维度 | S0（001） | S2-0（002） |
|---|---|---|
| 落点 | `spikes/s0/`，脱离 workspace | `spikes/s2/`，脱离 workspace（同理由） |
| release profile | 逐项复制根配置 | 逐项复制根配置（T007 体积对照的前提） |
| 结论载体 | `BASELINE.md` | `BASELINE.md`（本文件） |
| 证伪处理 | 无证伪门 | **T005 是证伪门**：证伪即停止上报 PM，备选方案 C 须用户批准 |
| 判据形态 | 命令输出 + 表格 | 断言即结论（`cargo test` 复跑），数据由诊断探针打印 |
