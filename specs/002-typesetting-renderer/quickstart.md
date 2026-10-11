# Quickstart: S2 排版渲染层验证指南

**面向**: 实现者与审查者——按本指南可端到端验证 S2 的每一条独立测试路径。全部命令无需窗口系统（渲染验证经离屏）。
**前置**: S1（001）已完成的仓库（`cargo build --workspace` 绿）；本层代码在 `crates/tome-reader/`。

> **正文来源口径（T053 走查修正，2026-10-11）**：本层全部子命令的正文来源是
> **`--text <file>`**（一份纯文本正文）。契约 `contracts/cli.md` 定义的
> `--chapter <book>#<n>` 书架形式**尚未接线**——它需要 001 的 `BookRepository`
> 与本地书扫描，属 S3 的工作。当前 `--chapter` 的任何写法（含 `#N` 后缀、
> 含本地路径）都返回结构化能力错误
> （`UnsupportedCapability { capability: "chapter-by-bookshelf" }`）与**退出码 3**。
> `cargo run -p tome-cli -- --help` 也把该选项标为 `WIP`。
>
> 故本指南一律用 `--text tests/fixtures/chapter-zh.txt`；该文件随仓库提供
> （约 20 KB 中文正文，7383 字符，14 页 / 14 视口，足以触发换行、禁则与多页切分）。

---

## 0. 一分钟冒烟

```bash
# 排版与分页（headless，纯计算）：contracts §2.1
cargo run -p tome-cli -- layout --text tests/fixtures/chapter-zh.txt --width 1180 --height 800
# 期望：JSON 含 pages[]（char_start/char_len/lines[]/side/is_padding），退出码 0

# 滚动流计算：同章以滚动形态输出视口序列
cargo run -p tome-cli -- layout --text tests/fixtures/chapter-zh.txt --mode scroll --width 800 --height 800
# 期望：scroll.viewports[] 覆盖区间并集与正文逐字符相等（SC-006b）

# 锚点换算：同一字符在滚动与翻页两形态下的定位（contracts §2.2）
cargo run -p tome-cli -- layout-anchor --text tests/fixtures/chapter-zh.txt \
  --char 37 --target-mode scroll
# 期望：anchor_value_unchanged=true 且 to.visible=true（SC-005b）
```

实测（2026-10-11）：三条命令**退出码均为 0**；上例的 1180×800 翻页得 14 页、
800×800 滚动得 14 视口，两者 `char_len` 之和均为 7383（== 正文逐字符）；
锚点命令得 `anchor_value_unchanged: true` / `to.visible: true`。

> 顶层 JSON 实际字段（`--format json`，默认）：`chapter_index` / `mode` /
> `geometry` / `paragraphs` / `pages` / `scroll` / `rule_exceptions` /
> `degraded_words` / `text_split` / `anchor_hit`。
> `scroll` 内含 `content_height` / `measure_px` / `viewport_count` / `viewports`。
> `pages[].side` 在 `auto` 且只有一栏时为 `null`。

## 1. 中文排版正确性（S2-1 切片）

```bash
# 规则清单逐项判定（SC-001：每项判定例+反例）
# 实测：匹配 56 个单元测试（README 与 squeeze/justify/mixed/cjk 的 #[cfg(test)] 模块）
cargo test -p tome-reader rules:: -- --nocapture

# 标点密集语料禁则零违反（SC-002；--corpus 为目录，contracts §2.5）
cargo run -p tome-cli -- verify-layout --corpus crates/tome-reader/tests/corpus/
# 期望：violations=0；如有例外，rule_exceptions 可枚举且每条有 rule_id
# 同一语料含中西文混排用例：无任一行超出容器宽度（SC-003）
```

实测：`verify-layout` 退出码 0，`total=8` / `passed=8` / `pass_rate=1.0`，
分栏统计 `{"kinsoku":3/3, "kinsoku_exceptions":1/1, "mixed":2/2, "squeeze":2/2}`。

> **输出无 `violations` 字段**——语料的判据是「逐条用例是否全通过」，
> 报告字段是 `mode` / `source` / `total` / `passed` / `pass_rate` /
> `by_category` / `outcomes[]`（每条含 `name` / `category` / `passed` / `diffs[]`）。
> 有未通过条目时退出码 1，且报告照常输出（不吞差异）。

## 2. 分页与锚点不变量（S2-2 切片）

```bash
# 锚点矩阵（SC-004：字号×行高×单双页×形态×段模式 全组合锚点不变）
# 注意：Rust 测试名过滤是**子串匹配**，且匹配的是**函数名**不是文件名。
# 下列名字都是**集成测试文件名**，必须配 `--test <file>` 才能选中整文件。
cargo test -p tome-reader --test anchor_matrix

# 呈现参数零依赖（SC-005：昼/夜/字体/动效下页序列逐字节相同）
cargo test -p tome-reader --test theme_independence

# 分页不丢字不重字（SC-006：页区间拼接 == 原文逐字符）
cargo test -p tome-reader --test pagination_lossless
```

实测：`anchor_matrix` 13 通过 / `theme_independence` 10 通过 /
`pagination_lossless` 23 通过，全部 0 失败。

> **不要**省掉 `--test`：`cargo test -p tome-reader anchor_matrix` 会把
> filter 当**函数名**去匹配全部 45 个测试目标，结果每个目标都是
> `running 0 tests ... filtered out`，整体报「ok」而**实际一个测试都没跑**
> ——这是最容易误判成「通过」的形态。

## 3. 滚动模式（S2-3 切片）

```bash
# 滚动流完整性（SC-006b：视口区间并集 == 原文，相邻无缺口无重叠）
cargo test -p tome-reader --test scroll_flow_integrity

# 翻页⇄滚动切换保锚点（SC-005b）
cargo test -p tome-reader --test mode_switch_anchor

# 阅读记录跨模式一致（SC-013b：同一字符两模式只计一次）
cargo test -p tome-reader --test record_dedup_cross_mode
```

实测：14 / 12 / 19 通过，全部 0 失败。

## 4. 渲染与像素快照（S2-4 切片）

```bash
# 离屏渲染一页并产出 PNG（无需窗口/显示器）
cargo run -p tome-cli -- render --text tests/fixtures/chapter-zh.txt --page 0 --out /tmp/page1.png

# 像素快照回归（SC-011；后端不可用时诚实报告并跳过，不伪装通过）
cargo test -p tome-reader --test snapshot
# CI 环境：probe 探针先行（adapter 缺失则 job 红，不静默绿）

# 渲染确定性（SC-012：同页两次渲染指令序列逐条相同）
cargo test -p tome-reader --test render_deterministic

# 字体检查（内嵌子集信息、缺字回退链、体积）
cargo run -p tome-cli -- fonts
# 期望：内嵌子集文件信息 + 体积 2.18 MiB（低于 3–5 MB 预算，见 size-report §3.1）
#       + 回退链列出；零网络请求
```

实测（本机 Windows / AMD Radeon Graphics / Vulkan 硬后端）：

- `render` **退出码 0**，产出 1180×800 RGBA PNG（3,777,153 B），
  报告字段 `out` / `width` / `height` / `bytes` / `foreground_pixels` /
  `total_pixels` / `backend`（`"Vulkan"`）/ `adapter`（显卡名）/
  `software`（`false`）/ `theme` / `reduced_motion` / `page_index` / `viewport_index`。
  **无需窗口**这条在本机成立（离屏渲染不建窗口）。
- `--test snapshot` 9 通过；`--test render_deterministic` 13 通过。
- **`--page` 是章内页码，从 0 起**（与 `layout` 输出的 `page_index` 同口径），
  故文档旧写的 `--page 1` 指的是「第二页」。越界页码给结构化错误与退出码 1。
- `fonts` 字段：`embedded[]`（`role` / `family` / `subset_coverage` / `bytes`）/
  `system_fallbacks`（按角色分组的族名列表）/ `budget_bytes` / `within_budget`。
  实测内嵌资产 `assets/fonts/TomeSerifCN-subset.otf` = **2,284,616 B（2.18 MiB）**，
  族名 `Tome Serif CN`，覆盖「GB2312 6763 字 + 常用标点 + 基本西文」，
  回退链 `宋体` / `SimSun` / `微软雅黑` / `Microsoft YaHei`，`within_budget: true`。

## 5. 主题/痕迹/交互（S2-5 切片）

```bash
# 痕迹渲染精确性（SC-013：渲染覆盖字符集合 == 区间，含跨页/跨视口）
cargo test -p tome-reader --test trace_exactness

# 命中测试互逆（FR-040：char→rect→char 往返 ≤ 半字形）
cargo test -p tome-reader --test hit_test_inverse
```

实测：11 / 11 通过，全部 0 失败。

## 6. 性能与收口（S2-6 切片）

```bash
# 性能基线（SC-007 翻页/SC-008 首屏/SC-009 重排/SC-010 内存，目标+红线分档）
# 八个用例都带 #[ignore]（性能需独占机器），**必须**加 `-- --ignored`——
# 不加则 8 个全被忽略，报「ok」而实测数为 0。
cargo test -p tome-reader --release --test perf_baseline -- --ignored --nocapture

# 三平台一致（SC-015：同字体同参数分页结构与滚动映射一致——CI 矩阵断言）
# 体积记录（SC-017：本层净增量写入报告）
```

实测（2026-10-11，本机）：8 通过 0 失败。关键数字（多次运行有波动，下为实测区间）
——SC-007 翻页 0.02–0.03 ms（红线 ≤ 33 ms）；SC-008 首屏 42–55 ms（红线 ≤ 800 ms）；
SC-009 重排 43–57 ms（红线 ≤ 300 ms）；SC-010 滚动释放 600/634 行、
20 万字章节 289 页 / 0.65–0.77 s。体积记录见
`specs/002-typesetting-renderer/size-report.md`。

---

## 验证环境说明

| 环境 | 能验什么 |
|---|---|
| 无图形 Linux/macOS/Windows | 全部排版/分页/滚动/映射/记录（SC-001~006b/013b/013/014） |
| + lavapipe 或 Metal | 另加离屏渲染与像素快照（SC-011/012） |
| GitHub Actions | 全矩阵：ubuntu=lavapipe、windows=lavapipe、macos=Metal |

**诚实跳过约定**：离屏渲染不可用时，快照测试必须输出「SKIPPED: <原因>」并以探针硬失败或显式跳过标记呈现——**永远不把跳过程序当通过**（spec §Edge Cases / SC-014）。
