# Quickstart: S2 排版渲染层验证指南

**面向**: 实现者与审查者——按本指南可端到端验证 S2 的每一条独立测试路径。全部命令无需窗口系统（渲染验证经离屏）。
**前置**: S1（001）已完成的仓库（`cargo build --workspace` 绿）；本层代码在 `crates/tome-reader/`。

---

## 0. 一分钟冒烟

```bash
# 排版与分页（headless，纯计算）：contracts §2.1
cargo run -p tome-cli -- layout --chapter tests/fixtures/chapter-zh.txt#0 --width 1180 --height 800
# 期望：JSON 含 pages[]（char_start/char_len/lines[]/side/is_padding），退出码 0

# 滚动流计算：同章以滚动形态输出视口序列
cargo run -p tome-cli -- layout --chapter tests/fixtures/chapter-zh.txt#0 --mode scroll --width 800 --height 800
# 期望：scroll.viewports[] 覆盖区间并集与正文逐字符相等（SC-006b）

# 锚点换算：同一字符在滚动与翻页两形态下的定位（contracts §2.2）
cargo run -p tome-cli -- layout-anchor --chapter tests/fixtures/chapter-zh.txt#0 \
  --char 37 --target-mode scroll
# 期望：anchor_value_unchanged=true 且 to.visible=true（SC-005b）
```

## 1. 中文排版正确性（S2-1 切片）

```bash
# 规则清单逐项判定（SC-001：每项判定例+反例）
cargo test -p tome-reader rules:: -- --nocapture

# 标点密集语料禁则零违反（SC-002；--corpus 为目录，contracts §2.5）
cargo run -p tome-cli -- verify-layout --corpus crates/tome-reader/tests/corpus/
# 期望：violations=0；如有例外，rule_exceptions 可枚举且每条有 rule_id
# 同一语料含中西文混排用例：无任一行超出容器宽度（SC-003）
```

## 2. 分页与锚点不变量（S2-2 切片）

```bash
# 锚点矩阵（SC-004：字号×行高×单双页×形态×段模式 全组合锚点不变）
cargo test -p tome-reader --test anchor_matrix

# 呈现参数零依赖（SC-005：昼/夜/字体/动效下页序列逐字节相同）
cargo test -p tome-reader theme_independence

# 分页不丢字不重字（SC-006：页区间拼接 == 原文逐字符）
cargo test -p tome-reader pagination_lossless
```

## 3. 滚动模式（S2-3 切片）

```bash
# 滚动流完整性（SC-006b：视口区间并集 == 原文，相邻无缺口无重叠）
cargo test -p tome-reader scroll_flow_integrity

# 翻页⇄滚动切换保锚点（SC-005b）
cargo test -p tome-reader mode_switch_anchor

# 阅读记录跨模式一致（SC-013b：同一字符两模式只计一次）
cargo test -p tome-reader record_dedup_cross_mode
```

## 4. 渲染与像素快照（S2-4 切片）

```bash
# 离屏渲染一页并产出 PNG（无需窗口/显示器）
cargo run -p tome-cli -- render --chapter tests/fixtures/chapter-zh.txt#0 --page 1 --out /tmp/page1.png

# 像素快照回归（SC-011；后端不可用时诚实报告并跳过，不伪装通过）
cargo test -p tome-reader --test snapshot
# CI 环境：probe 探针先行（adapter 缺失则 job 红，不静默绿）

# 渲染确定性（SC-012：同页两次渲染指令序列逐条相同）
cargo test -p tome-reader render_deterministic

# 字体检查（内嵌子集信息、缺字回退链、体积）
cargo run -p tome-cli -- fonts
# 期望：内嵌 TomeSerifCN-subset.otf 信息 + 体积 3–5MB + 回退链列出；零网络请求
```

## 5. 主题/痕迹/交互（S2-5 切片）

```bash
# 痕迹渲染精确性（SC-013：渲染覆盖字符集合 == 区间，含跨页/跨视口）
cargo test -p tome-reader trace_exactness

# 命中测试互逆（FR-040：char→rect→char 往返 ≤ 半字形）
cargo test -p tome-reader hit_test_inverse
```

## 6. 性能与收口（S2-6 切片）

```bash
# 性能基线（SC-007 翻页/SC-008 首屏/SC-009 重排/SC-010 内存，目标+红线分档）
cargo test -p tome-reader --release --test perf_baseline -- --nocapture

# 三平台一致（SC-015：同字体同参数分页结构与滚动映射一致——CI 矩阵断言）
# 体积记录（SC-017：本层净增量写入报告）
```

---

## 验证环境说明

| 环境 | 能验什么 |
|---|---|
| 无图形 Linux/macOS/Windows | 全部排版/分页/滚动/映射/记录（SC-001~006b/013b/013/014） |
| + lavapipe 或 Metal | 另加离屏渲染与像素快照（SC-011/012） |
| GitHub Actions | 全矩阵：ubuntu=lavapipe、windows=lavapipe、macos=Metal |

**诚实跳过约定**：离屏渲染不可用时，快照测试必须输出「SKIPPED: <原因>」并以探针硬失败或显式跳过标记呈现——**永远不把跳过程序当通过**（spec §Edge Cases / SC-014）。
