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

## 3. 待补章节（T006-T009）

以下各节随对应任务落地后填充。当前状态：**未开始**。

- §4 R5-2 hinting 对 x_advance 挤压的量化影响（T006）
- §5 R5-7 贪心换行 + 挤压的排版质量（T006）
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
