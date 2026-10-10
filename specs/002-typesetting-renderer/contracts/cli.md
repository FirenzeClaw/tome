# 契约 · S2 排版渲染层 CLI 验证器（`tome-cli` 扩展）

**特性**: `002-typesetting-renderer` | **日期**: 2026-10-10 | **状态**: 定稿

**依据**：裁决 OQ#17→C（PM，2026-10-10）——可验证入口的形态为**库 API 为内核 + `tome-cli` 薄壳**。

**与 001 的关系**：本文件**新建**，不改动 `specs/001-pc-engine-core/contracts/cli.md`。本文件是对 001 CLI 契约的**扩展声明**：001 的子命令（`source import` / `search` / `run` / `verify`）语义不变；本文件新增排版层子命令。

---

## 1. 设计约束

- **无窗口系统**：全部子命令 MUST 在无图形环境下运行（spec §FR-052）。渲染子命令走**离屏渲染**；图形后端不可用时 MUST 以退出码 `3`（不支持能力）报告，MUST NOT 伪装成功（spec §FR-053）。
- **结构化输出**：默认 `--format json`，供脚本与差分/快照测试消费；`--format human` 为可读摘要。
- **字符下标口径**：输出中出现的一切位置一律是**字符下标**（Unicode 标量值序列下标），与 001 的 `char_offset` 同口径（spec §FR-003）。MUST NOT 输出字节偏移或渲染坐标作为位置（spec 继承 001 §FR-028）。
- **退出码**沿用 001 的约定：`0` 成功 / `1` 业务错误 / `2` 用法错误 / `3` 不支持能力。
- **薄壳**：子命令只做参数解析、调用 `tome-reader` 的公开 API、序列化结果。逻辑 MUST NOT 在 CLI 内重复实现（宪法 IV DRY）。

## 2. 子命令

### 2.1 `tome layout --chapter <book>#<index> [--mode paginated|scroll] [options]`

排版与分页/滚动流计算。**不渲染**（纯计算，无需图形环境）。

| 选项 | 说明 |
|---|---|
| `--chapter <book_ref>#<index>` | 目标章节（`book_ref` 为书架 id 或本地文件路径） |
| `--mode paginated\|scroll` | 阅读形态，默认 `paginated`（spec §FR-020） |
| `--width <px>` / `--height <px>` | **阅读区可用宽高**（不是窗口宽高——spec §FR-014 的口径） |
| `--font-size <px>` | 阅读字号，默认 19（范围 14–30） |
| `--line-height <x>` | 行高，默认 1.85（范围 **1.75–2.2**，spec §FR-013） |
| `--paragraph-mode indent\|gap` | 段首缩进（默认）或段间距（spec §FR-018） |
| `--measure-em <n>` | 版心目标 em，默认 36；上限 42（spec §FR-013） |
| `--gutter <px>` | 双页中缝，默认 64 |
| `--spread auto\|single\|double` | 单双页模式，默认 `auto`（spec §FR-014/#015） |
| `--anchor <char_offset>` | 可选：计算后报告该锚点落在第几页/视口何处（验证 SC-004/SC-005b） |
| `--reading-config <file>` | 可选：从 `Book.read_config` 的 JSON 加载参数（CLI 选项优先） |

**输出（JSON）**：

```jsonc
{
  "chapter_index": 21,
  "mode": "paginated",
  "geometry": { "font_size": 19, "line_height": 1.85, "measure_em": 36, "gutter": 64,
                "width": 1180, "height": 800, "paragraph_mode": "indent", "spread": "double" },
  "pages": [                          // mode=paginated
    { "page_index": 0, "char_start": 0, "char_len": 412, "is_padding": false,
      "side": "left", "lines": [ { "char_start": 0, "char_len": 24, "paragraph_index": 0 } ] }
  ],
  "scroll": {                         // mode=scroll：视口序列（分页语义的滚动对应物）
    "content_height": 48213,
    "viewports": [ { "top": 0, "height": 800, "char_start": 0, "char_len": 415 } ]
  },
  "rule_exceptions": [ { "char_offset": 1204, "rule": "line-end-no-open-punct" } ],
  "anchor_hit": { "char_offset": 300, "page_index": 0, "offset_in_page": 300 }   // 仅 --anchor 时
}
```

**关键不变量（须被测试断言）**：
- `pages[].char_len` 按序拼接 MUST 与该章正文逐字符相等（留白页 `is_padding=true` 不计入内容区间）——spec §SC-006。
- `--mode scroll` 的 `viewports[].char_len` 并集 MUST 与正文逐字符相等，且相邻视口区间连续无缺口无重叠——spec §SC-006b。
- 同参数两次运行的输出 MUST 逐字节相同（确定性，spec §FR-048）。

### 2.2 `tome layout-anchor --chapter <book>#<index> --char <offset> [几何选项同上] [--target-mode paginated|scroll]`

位置换算与阅读形态切换（验证 SC-004 / SC-005b / §FR-022 / §FR-023）。输出指定的字符在给定几何与形态下的定位，以及在另一形态下的定位。

```jsonc
{
  "char_offset": 300,
  "from": { "mode": "scroll", "viewport_index": 0, "offset_in_viewport": 300, "visible": true },
  "to":   { "mode": "paginated", "page_index": 0, "offset_in_page": 300, "visible": true },
  "anchor_value_unchanged": true
}
```

**判据**：`anchor_value_unchanged` MUST 为 `true`，且 `to.visible` MUST 为 `true`（spec §SC-005b）。

### 2.3 `tome render --chapter <book>#<index> --page <n> --out <png> [几何/主题选项]`

离屏渲染一页（或滚动模式的一个视口）为图像，供像素快照比对（spec §SC-011）。

| 选项 | 说明 |
|---|---|
| `--page <n>` / `--viewport <n>` | 目标页（翻页模式）或视口（滚动模式） |
| `--out <path>` | 输出 PNG |
| `--theme day\|night` | 默认 `day`（spec §FR-030） |
| `--reduced-motion` | 关闭动效（不影响静态渲染结果，用于断言 spec §FR-032） |
| `--geometry-json <file>` | 完整几何参数（避免长命令行） |

**图形后端不可用时**：MUST 输出结构化错误并退出码 `3`（`UnsupportedCapability { capability: "offscreenRendering" }`），MUST NOT 产出空文件或伪造成功（spec §FR-053）。

### 2.4 `tome fonts`

报告内嵌字体子集与回退链状态（验证 spec §FR-050/FR-051、§SC-017 的体积预算）。

```jsonc
{
  "embedded": [ { "role": "reading-serif", "family": "...", "subset_coverage": "GB2312+常用标点",
                  "bytes": 3820544 } ],
  "system_fallbacks": { "ui-sans": ["..."], "mono": ["..."] },
  "budget_bytes": 5242880,
  "within_budget": true
}
```

### 2.5 `tome verify-layout --corpus <dir> [--report <file>]`

跑排版层合成语料的一致性/快照比对，输出通过率与逐条差异（对齐 001 的 `verify` 形态与 spec §SC-001~SC-003）。语料格式与 001 的 `tests/corpus/` 同风格，条目声明输入正文 + 参数 + 期望（分页结构 / 规则命中 / 禁则例外）。

## 3. 与 001 CLI 的关系

| 项 | 处置 |
|---|---|
| 001 的 `source import` / `search` / `run` / `verify` | **语义不变**，本规格不修改 |
| 001 `run --action <a>` 的 `info`/`toc`/`content` | 属 001 后续切片，与本文件无关 |
| `tome-cli` 的依赖方向 | 它是组合根，依赖全部 crate；新增 `tome-reader` **不违反**架构铁律（`tome-reader` 只依赖 `tome-core`）——AGENTS §架构铁律 |
| 新增子命令的注册位置 | 001 的 `crates/tome-cli/src/flags.rs` 的 `Command` 枚举（S2 实现阶段落盘） |
| 帮助文本 | 须登记（001 的 `flags.rs::help_text()`），保持与既有子命令同格式 |

## 4. 可测试性要求

- 每个子命令 MUST 有 golden 测试：给定输入目录，输出 JSON 与期望文件逐字段比对。
- `layout` 的确定性 MUST 被断言：同参数两次运行输出逐字节相同。
- `render` 的像素差 MUST 可在无图形环境下比对（spec §SC-011）；后端不可用时测试 MUST 显式跳过并报告（MUST NOT 记为通过）。
- 除 `--chapter <本地文件路径>` 外，全部子命令 MUST NOT 触发网络（spec §FR-056）；`--chapter` 指向网络书时，内容 MUST 来自 001 的既有缓存，MUST NOT 因本层调用而发起抓取（继承 001 §FR-019b）。
