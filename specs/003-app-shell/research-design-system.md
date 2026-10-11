# Research: S3 应用外壳的设计系统约束提取

**Date**: 2026-10-11 | **输入**: `design-system/MASTER.md`（唯一设计规范，v1.0 · 2026-10-10）、`design-system/MOTION.md`、`design-system/tokens.css`、`design-system/preview.html`（四页原型 + 24 张状态基线）、`specs/003-app-shell/spec.md`
**用途**: 把 plan.md 需要编码的设计约束（token / 组件 / 布局 / 动效 / 键盘焦点 / 三态 / 主题机制）落成可逐项对照的清单，并显式登记设计系统对「桌面阅读器外壳」的**未覆盖处**（§8 gap 清单）与 egui 落点策略（§9）。
**方法**: 通读四份设计系统文件逐行取数（不依赖任何转述）；对 MASTER §2.3 的对比度声明**用 WCAG 2.1 公式独立复算**（§1.2，含 8 行声明值与 5 处偏差）；对 `tome-core::model::theme` 已落地的 token 投影做交叉核对（避免 S3 重复定义）。

**与 S3 spec 的锚点**：FR-021 / SC-011 要求界面 100% 出自 MASTER；FR-017/SC-008 要求键盘可达与焦点态逐项核对；FR-018 要求「主题（昼/夜/跟随系统）」；FR-007 要求阅读视图内即时调整五组参数。

---

## 0 · 读法与三条前提

1. **`tome-core` 已是 MASTER 的完整 token 投影，S3 不得再抄一份常量**。`crates/tome-core/src/model/theme.rs` 已落地：`Palette`（20 个语义色，`theme.rs:127-170`）、`Fonts`（`:281-356`）、`TypeScale` 八档（`:368-404`）、`SpacingScale` 九档（`:458-490`）、`Radii` 五档（`:505-518`）、`ShellDims`（`:529-540`）、`Shadows`（`:565-624`）、`Texture`（`:626-664`）、`Swatches`（`:665-684`）、`Motion`（`:739-764`）、`ReadingMetrics`（`:790-870`）、`ReadingToolbar`（`:891-902`），门面 `Theme`（`:935-948`）与 `DayTheme` / `NightTheme` 单例（`:1050` / `:1054`）。MASTER §10（`MASTER.md:438-445`）要求「单一 `theme` 模块产出 `Theme` 结构体，昼/夜两实例；**禁止各控件散读常量**」——S3 的合规做法是**消费**它，而不是在 `tome-app` 里重新声明十六进制。
2. **口径纪律**：本文所有宽度若用于单双页判据，一律区分「窗口宽」与「阅读区可用宽（available）」——MASTER §5.2 的阈值 1432px 指的是**后者**（`MASTER.md:250-261`）。
3. **引文格式**：`MASTER.md §N` 表章节，`MASTER.md:行` / `tokens.css:行` / `preview.html:行` 表精确出处。凡数值只在 `preview.html` 出现而 MASTER/tokens.css 未定义的，一律进 §8 gap 清单，**不得**被当作已批准 token 使用。

---

## 1 · Token 清单

### 1.1 配色（昼 / 夜全套）

出处：`tokens.css:53-89`（昼）、`:94-129`（夜）；同值表见 `MASTER.md §2.1`（`:37-60`）、`§2.2`（`:62-85`）。`tome-core` 投影见 `theme.rs:171-194`（PARCHMENT）/ `:195-226`（OBSIDIAN）。

| 变量 | 昼 Parchment | 夜 Obsidian | 用途 |
|---|---|---|---|
| `--bg-base` | `#E8DFC8` | `#000000` | 应用最底层（夜为 OLED 真黑） |
| `--bg-surface` | `#F2EAD6` | `#0D0B0B` | 卡片 / 面板 / 侧栏 |
| `--bg-elevated` | `#FAF4E4` | `#171414` | 悬浮层 / 弹窗 / 下拉 / 抽屉 |
| `--bg-reading` | `#F5EEDC` | `#000000` | **正文阅读区（最纯净层）** |
| `--bg-inset` | `#DFD4B8` | `#131010` | 输入槽 / 搜索框底 / 斑马纹 / 骨架块 |
| `--border-subtle` | `#D6C9A8` | `#231E1E` | 分割线 / 弱描边 |
| `--border-strong` | `#BCA97F` | `#3A3232` | 强调描边 / 输入框边框 |
| `--text-primary` | `#2E2418` | `#E6E4DE` | 正文墨色（夜为暖白，非纯白） |
| `--text-secondary` | `#5C4B33` | `#9A9790` | 次要文本 / 标签 |
| `--text-tertiary` | `#8A7658` | `#6A6761` | 说明 / 元信息（⚠ 见 §1.2 缺陷 D3） |
| `--text-inverse` | `#F5EEDC` | `#0D0B0B` | 深/亮底上的反白字 |
| `--accent-primary` | `#8C3A1E` | `#D9794A` | 主操作 / 当前项 / **焦点环** |
| `--accent-hover` | `#A34524` | `#E88F5F` | 主色 hover |
| `--accent-soft` | `#E4D3B8` | `#2A1A12` | 主色浅底（选中行 / 标签底 / 分段高亮） |
| `--accent-secondary` | `#3F5C4A` | `#7FA88C` | 苔绿：成功 / 已读 / 次要状态 |
| `--accent-clash` | `#B8862B` | `#D9AE5C` | 鎏金：徽记 / 星级 / 进度条高光（⚠ 见 §1.2 缺陷 D1/D4） |
| `--state-success` | `#3F5C4A` | `#7FA88C` | 成功 |
| `--state-warning` | `#B8862B` | `#D9AE5C` | 警告（昼与 `--accent-clash` 同值） |
| `--state-error` | `#A32B1F` | `#E06B5C` | 错误 / 危险 |
| `--state-info` | `#665A4F` | `#B8AFA0` | 中性信息（**暖中性灰**） |

**恒定色卡**（不随当前主题变，用于设置页「外观主题」）：`--swatch-day-bg/a/b/c = #F5EEDC / #8C3A1E / #3F5C4A / #B8862B`，`--swatch-night-bg/a/b/c = #000000 / #D9794A / #7FA88C / #D9AE5C`（`tokens.css:136-145`，`MASTER.md:194-203`）。规矩：色卡**必须引用变量**，内联十六进制视为「说谎的预览」（`MASTER.md:196`）。

**硬性禁令**（`MASTER.md §8:380-387`，S2 已有对应测试 `crates/tome-reader/tests/theme_accessibility.rs:290-315`）：
- ❌ 禁止蓝色系（`--state-info` 因此从蓝灰 `#4A5A6B`/`#8FA3B8` 重调为暖中性灰，裁决见 `MASTER.md:100-107`）——判据是「蓝分量同时大于红与绿且差 ≥24」（测试实现口径）。
- ❌ 禁止纯白 `#FFFFFF` 作昼底；❌ 禁止夜主题用深灰代替真黑。
- ❌ 禁止页面里直接写十六进制（`tokens.css:4`、`MASTER.md:35`）——SC-011 的可机检部分。

### 1.2 对比度与无障碍（MASTER 声明 + 独立复算）

**MASTER §2.3 声明表**（`MASTER.md:89-98`，原样转录）：

| 组合 | 声明值 | 判定 |
|---|---|---|
| 昼 `text-primary` on `bg-reading` | 11.8:1 | ✅ AAA |
| 昼 `text-secondary` on `bg-reading` | 6.4:1 | ✅ AA |
| 昼 `accent-primary` on `bg-reading` | 6.1:1 | ✅ AA |
| 夜 `text-primary` on `bg-base` | 15.2:1 | ✅ AAA |
| 夜 `text-secondary` on `bg-base` | 5.6:1 | ✅ AA |
| 夜 `accent-primary` on `bg-base` | 6.8:1 | ✅ AA |
| 昼 `state-info` on `bg-reading` | 5.8:1 | ✅ AA |
| 夜 `state-info` on `bg-base` | 9.7:1 | ✅ AA |

**复算结果（WCAG 2.1 相对亮度公式，本文件独立计算）**：8 行中 3 行吻合（±0.1），5 行**偏低 0.3–1.6**（昼 primary 13.13、昼 secondary 7.23、昼 accent 6.62、夜 primary 16.52、夜 secondary 7.20）。**偏差方向一致为「声明值更保守」**，故全部判定成立、无 AA 违规；但**声明值不可复现**，意味着：
- SC-011 的「逐项对照检查表」**不得断言精确相等**，须断言 `实测 ≥ 门槛`（4.5:1），否则会假阳性。
- 建议 S3 沿用 S2 的做法：断言门槛而非断言数字（`crates/tome-reader/tests/theme_accessibility.rs:345-360` 即「六组关键组合 ≥4.5」）。

**未列入 §2.3 的色 —— 本文件补算（对 5 个背景层取范围，单位为对比度）**：

| 色 | 昼（min–max） | 夜（min–max） | 结论 |
|---|---|---|---|
| `--text-tertiary` | 2.96（on inset）– 3.97（on elevated） | 3.25（on elevated）– 3.73 | ⚠ **全部 < 4.5**，见缺陷 D3 |
| `--accent-secondary` = `--state-success` | 5.01 – 6.73 | 6.87 – 7.88 | ✅ 可作文本色 |
| `--state-error` | 4.87 – 6.54 | 5.61 – 6.43 | ✅ 可作文本色（昼 on `bg-inset` 4.87 最紧） |
| `--accent-clash` = `--state-warning` | **2.20 – 2.95** | 8.86 – 10.16 | ⚠ **昼不可作文本前景**，见缺陷 D1/D4 |
| `--border-strong` | 1.56 – 2.10 | 1.47 – 1.68 | ⚠ < 3:1（WCAG 1.4.11），见缺陷 D2 |
| `--border-subtle` | 1.11 – 1.50 | 1.11 – 1.28 | 同上（分割线，非必要边界） |
| `--accent-primary`（焦点环） | 5.20 – 6.98 | 5.92 – 6.78 | ✅ 两主题均 ≥3:1，焦点环可见性成立 |

**主操作按钮的反白字**（按钮底 = 主色，字 = `--text-inverse`）：昼 6.62:1 ✅、夜 6.34:1 ✅。
**`--accent-soft` 底上的当前项文字**（`MASTER §7` Sidebar 当前项）：昼 `accent-primary` 5.22:1 / `text-primary` 10.36:1 ✅；夜 5.41:1 / 13.16:1 ✅。

**缺陷清单（本文档新发现，plan 必须处置）**：

- **D1 · 鎏金/警告色在昼主题只能作填充，不能作文本前景**。`--accent-clash` / `--state-warning` = `#B8862B` 对昼主题全部 5 个背景仅 2.20–2.95:1。MASTER §8 只说「图表/进度用鎏金点缀」（`:374`），未写明这条边界。约束：鎏金/警告色出现处，其上的文字必须用 `--text-primary`（`#2E2418` on `#B8862B` = 4.69:1 ✅）或 `--text-inverse`（昼 2.80:1 ❌ 不可用）。
- **D2 · 描边色不足以独立承担可见边界**。`--border-strong` 最大仅 2.10:1（< 3:1）。而 `MASTER §2.1:47` 把 `--border-strong` 定义为「输入框边框」→ 输入框的可见性不能只靠边框，须同时给 `--bg-inset` 底色（`preview.html:62-64` 的搜索框正是「bg-inset 底 + 无边框」这一组合）。
- **D3 · `--text-tertiary` 不达 AA，却承担 11–12px 说明文字**。昼 2.96–3.97、夜 3.25–3.73，全部 < 4.5。而 `MASTER §2.3:109` 只承诺「正文色永不低于 `--text-secondary`」——tertiary 被排除在承诺外，但 `preview.html` 把它用于 `.author`（12px）、`.note`（12px）、`.rfoot`（11px）、`.kbd`（11px）、`.chapno`（13px）等**需要阅读的**文本。plan 需二选一：① 明确 tertiary 只用于非必要装饰（则上述位置须换 `--text-secondary`）；② 调亮 tertiary 至 ≥4.5（会改变设计观感，属设计变更）。
- **D4 · 状态徽记/状态药丸的前景配对在夜主题下崩坏**（`preview.html` 实测缺陷，见 §2 组件表脚注）：`.badge`（金底 + `text-primary`）夜主题 = **1.63:1**；`.badge.off`（tertiary 底 + `text-inverse`）昼 3.77 / 夜 3.48；`StatusPill 警告`（warning 底 + `text-primary`）夜 **1.63:1**。MASTER §9.1 称状态页「是可直接照抄的基线」（`:432`），但此处**不可照抄**——前景色必须随主题切换（夜警告底应配 `--text-inverse` = 9.5:1）。

**强制尺寸**（`MASTER.md:109-111`，S2 已断言 `crates/tome-reader/tests/theme_accessibility.rs`）：正文行高 ≥1.75；焦点环 2px `--accent-primary` + 2px offset，两主题均可见；触达目标 ≥32px（桌面），**图标按钮 ≥36px**。

### 1.3 字体与排印

**字体族**（`tokens.css:22-24`、`MASTER.md:119-123`、`theme.rs:314-355`）：

| 角色 | 字体栈 | 用途 |
|---|---|---|
| `--font-ui` | `"Segoe UI Variable Text", "Segoe UI", -apple-system, "PingFang SC", "Microsoft YaHei", system-ui, sans-serif` | UI 框架（无衬线） |
| `--font-reading` | `"Iowan Old Style", "Source Han Serif SC", "Songti SC", "Noto Serif SC", Georgia, "SimSun", serif` | **正文**（衬线） |
| `--font-mono` | `"Cascadia Code", "JetBrains Mono", "SF Mono", Consolas, monospace` | 书源规则 / 正则 / 数字 / kbd |

核心决策（`MASTER.md:125`）：正文衬线、UI 无衬线、规则等宽——**三角色刻意分离**，是本设计的辨识度。MASTER §10 要求「打包内嵌字体（衬线正文 + 无衬线 UI + 等宽）确保三平台一致，注意 <20MB 体积预算」（`:441`）——⚠ 仓库当前只内嵌了正文衬线子集 `assets/fonts/TomeSerifCN-subset.otf`（2.28MB），见 gap G9。

**字号层级**（`MASTER.md:129-141`、`tokens.css:27-28`、`theme.rs:405-457`；值为 `字号 / 行高倍数 / 字重`）：

| Token | 值 | 用途 |
|---|---|---|
| `--fs-display` | `30px / 1.15 / 700` | 空状态标题、页面主标题（罕用） |
| `--fs-h1` | `22px / 1.30 / 650` | 页面标题 |
| `--fs-h2` | `17px / 1.40 / 600` | 区块标题 |
| `--fs-h3` | `15px / 1.45 / 600` | 卡片标题 |
| `--fs-body` | `14px / 1.60 / 400` | 界面正文 |
| `--fs-sm` | `13px / 1.55 / 400` | 次要正文 / 表格 |
| `--fs-caption` | `12px / 1.50 / 400` | 说明 / 标签 |
| `--fs-micro` | `11px / 1.40 / 500` | 徽记 / 计数 |

字重白名单 **400 / 500 / 600 / 650 / 700**，禁止 800+（`MASTER.md:141`）。
⚠ 实现注意（`preview.html:91`、`:160`、`:194` 等）：原型多处用 `font-weight: 700` 覆盖 `--fs-h1`（650）与 `--fs-h3`（600）；S3 应以 MASTER 表的三元组为准，或以原型为准——需在 plan 明确（列 gap G12）。

### 1.4 间距 / 圆角 / 骨架尺寸 / 阴影 / 质感 / 色卡

**间距阶**（4px 网格九档，`tokens.css:15-16`、`MASTER.md:147-153`、`theme.rs:491-503`）：
`--sp-1:4` `--sp-2:8` `--sp-3:12` `--sp-4:16` `--sp-5:20` `--sp-6:24` `--sp-8:32` `--sp-10:40` `--sp-12:48`。
用法约束：组件内边距 `--sp-3`（紧凑）/ `--sp-4`（标准）；区块间距 `--sp-6` / `--sp-8`；页面内容左右留白 `--sp-6`（窄）/ `--sp-10`（宽）。

**圆角**（`tokens.css:19`、`MASTER.md:155-163`、`theme.rs:519-527`）：
`--r-xs:4`（徽记/小标签）· `--r-sm:6`（按钮/输入框）· `--r-md:8`（卡片/面板）· `--r-lg:12`（大容器/弹窗）· `--r-full:999`（药丸/头像）。

**布局骨架尺寸**（`tokens.css:9-12`、`MASTER.md:165-174`、`theme.rs:541-548`）：

| Token | 值 | 用途 |
|---|---|---|
| `--topbar-h` | `56px` | TopBar 高度 |
| `--sidebar-w` | `240px` | 侧栏展开宽 |
| `--sidebar-w-collapsed` | `64px` | 侧栏折叠为图标栏 |
| `--control-h` | `32px` | **统一控件高**（按钮/输入框/分段控件；曾出现 32/34 并存，已统一，`MASTER.md:174`） |

⚠ `tokens.css` 把骨架尺寸块**重复声明了两次**（`:9-12` 与 `:44-47`）——漂移风险，列 gap G8。

**阴影**（克制，仅悬浮层；`MASTER.md:176-184`、`theme.rs:575-624`）：

| Token | 昼 | 夜 |
|---|---|---|
| `--shadow-sm` | `0 1px 2px rgba(70,52,28,.10)` | `0 1px 2px rgba(0,0,0,.60)` |
| `--shadow-md` | `0 6px 16px -4px rgba(70,52,28,.16)` | `0 6px 16px -4px rgba(0,0,0,.75)` |
| `--shadow-lg` | `0 16px 40px -12px rgba(70,52,28,.22)` | `0 16px 40px -12px rgba(0,0,0,.85)` |

**质感与遮罩**（`MASTER.md:186-192`）：`--paper-noise`（SVG fractalNoise data-URI，昼主题专属，`mix-blend-mode: multiply`）· `--paper-noise-opacity: .045`（夜 0）· `--overlay-bg`：昼 `rgba(46,36,24,.28)` / 夜 `rgba(0,0,0,.66)`（抽屉/弹窗遮罩）。

**阅读层 token**（`tokens.css:36-42`、`MASTER.md §5.1:225-237`、`theme.rs:871-882`）：`--reading-font-size: 19px`（可调 **14–30**）· `--reading-line-height: 1.85`（可调 **1.75–2.2**，下限由 `MASTER.md:109` 的无障碍强制条款钉死，裁决 OQ#19→A）· `--reading-measure: 36em` · `--reading-measure-max: 42em` · `--reading-gutter: 64px` · `--reading-paragraph-indent: 2em`（默认）· `--reading-paragraph-gap: 0.4em`（与缩进二选一）。

### 1.5 图标体系 ⚠ 无 MASTER 章节

MASTER **没有图标规范章节**；唯一数值来源是 `preview.html`：

| 项 | 值 | 出处 |
|---|---|---|
| 通用图标类 `.ico` | `16×16`，`viewBox="0 0 24 24"`，`stroke: currentColor`，`fill: none`，`stroke-width: 1.7` | `preview.html:85` |
| 阅读工具栏图标 `.ricon` | `18×18`，其余同上 | `preview.html:162` |
| 行内小图标（内联尺寸） | `14px`（plus/alert）/ `15px`（moon/check）/ `16px`（left/close） | `preview.html:369-378` |
| 图标集合 | 17 个内联 SVG：shelf / globe / folder / chart / gear / search / moon / plus / left / font / dots / book / set / check / alert / close / dot | `preview.html:362-380` |
| 品牌标识 | 22×22 双页书本 SVG，页框 `--accent-primary` + 书脊 `--accent-clash` | `preview.html:439-443` |
| 空/错态插画位 | 72×72 圆角 `--r-lg` 底（`--bg-inset`）+ 32×32 图标 `stroke-width 1.5`（`--text-tertiary`） | `preview.html:254-256` |

**这一节的合规风险**：MASTER 与 `tokens.css` 的一致性纪律是「本文件不得出现 `tokens.css` 未定义的 token（反向亦然）」（`MASTER.md:8`），但图标尺寸/线宽/集合**只存在于原型**。S3 需要一次显式裁决（列 gap G1）。

### 1.6 动效 token

`--ease-out: cubic-bezier(.22,.61,.36,1)` · `--ease-spring: cubic-bezier(.34,1.3,.64,1)` · `--dur-fast: 150ms` · `--dur-base: 200ms` · `--dur-slow: 280ms`（`tokens.css:31-33`、`MASTER.md:209-215`、`theme.rs:765-771`：`ease_out [0.22,0.61,0.36,1.0]`、`ease_spring [0.34,1.30,0.64,1.0]`、`dur_fast_ms 150`、`dur_base_ms 200`、`dur_slow_ms 280`）。元素级映射见 §4。

### 1.7 全局 CSS 行为（S3 须逐项对齐）

| 行为 | 规格 | 出处 |
|---|---|---|
| 焦点环 | `:focus-visible { outline: 2px solid var(--accent-primary); outline-offset: 2px; border-radius: var(--r-xs); }`，**禁止抹掉** | `tokens.css:167-172`；`MASTER.md:110`、`:350` |
| 主题切换过渡 | 只过渡 `background-color` / `color`，`--dur-slow` `--ease-out`；**不过渡布局属性** | `tokens.css:158-160`；`MASTER.md:217`、`MOTION.md:43` |
| 纸纤维噪点 | `body::after` 固定覆盖，`z-index:9999`、`pointer-events:none`、`mix-blend-mode:multiply`、`opacity:.045`（仅昼） | `tokens.css:174-183` |
| 滚动条 | 宽/高 `10px`；轨道透明；滑块 `--border-strong`，`border-radius:--r-full`，`border:3px solid transparent` + `background-clip:content-box`；hover 变 `--accent-primary` | `tokens.css:186-194` |
| 减少动效 | `@media (prefers-reduced-motion: reduce)` → 全部动画/过渡压到 `.01ms`、迭代 1 次 | `tokens.css:196-202`；语义见 `MOTION.md §4:45-57` |
| 基础重置 | `box-sizing:border-box`；`html,body` 高 100%、`font-family:var(--font-ui)`、`font-size:var(--fs-body)`、底 `--bg-base`、字 `--text-primary`；`button/input/select/textarea` 继承字体 | `tokens.css:150-165` |

---

## 2 · 组件清单（含状态）

**总规矩**（`MASTER.md §7:325-350`）：所有可交互元素**必须**实现 `default / hover / focus-visible / active / disabled`，其中 `focus-visible` 必须是**真实可见的环**，不得用 `outline:none` 抹掉。四态的**具体视觉**（原型基线，`preview.html:344-351`）：

| 态 | 视觉 |
|---|---|
| default | 主按钮：底 `--accent-primary`、字 `--text-inverse`、`--r-sm`、`--control-h` 高 |
| hover | 底 `--accent-hover` |
| active | 底回到 `--accent-primary` + `transform: translateY(1px)` + `box-shadow: inset 0 2px 4px rgba(0,0,0,.25)` |
| focus-visible | `outline: 2px solid var(--accent-primary); outline-offset: 2px` |
| disabled | 底 `--bg-inset`、字 `--text-tertiary`、`cursor: not-allowed` |

### 2.1 骨架与导航

| 组件 | 关键样式 | 状态/规则 | 出处 |
|---|---|---|---|
| `AppShell` | TopBar 56px + Sidebar 240/64 + Content；TopBar **不用彩色底**，仅 1px 下描边 `--border-subtle`；品牌用 `--font-reading` 衬线 17px/700 | — | `MASTER.md:303-321`；`preview.html:57-59`、`:61` |
| `TopBar` | `height:56px; padding:0 20px; background:var(--bg-surface); border-bottom:1px solid var(--border-subtle)`；含品牌 / 全局搜索 / 主题按钮 / 导入主操作 / 头像（32×32 `--r-full` `--accent-secondary`） | 主题按钮：底 `--bg-base`、字 `--text-secondary`、12.5px；主操作：`--accent-primary` + `--text-inverse` 12.5px/500 | `MASTER.md:319`、`:327`；`preview.html:58-72` |
| 全局搜索框 | `width:380px; height:var(--control-h); background:var(--bg-inset); border-radius:var(--r-sm); padding:0 12px`；占位 13px `--text-tertiary`；右侧 kbd `Ctrl K`（`--font-mono` 11px） | 无边框（见缺陷 D2） | `preview.html:62-66`、`:446` |
| `Sidebar` | `width:240px; padding:16px 12px; background:var(--bg-surface)`；分组标签 11px/500 `--text-tertiary`；项 `height:38px; gap:10px; padding:0 10px; border-radius:var(--r-sm)`；计数 `--font-mono` 11px 右对齐 | **当前项**：`background:var(--accent-soft)` + 字 `--accent-primary` + 500 字重 + `box-shadow: inset 3px 0 0 var(--accent-primary)`（左侧 3px 竖条，**不用整块实色填充**）；三态：展开/折叠/覆盖抽屉 | `MASTER.md:320-321`；`preview.html:75-82`；折叠态规格 ❌ 待补（`MASTER.md:427`） |
| 侧栏底部 | 固定「用户 / 本地库根目录」区 | — | `MASTER.md:322` |
| 分割线 `.divider` | `height:1px; background:var(--border-subtle); margin:8px 0` | — | `preview.html:87` |
| `PageHeader` | `padding:24px 32px 14px`；h1 22px/700；desc 13px `--text-secondary`，上距 6px；右侧主操作（**唯一 primary**） | — | `MASTER.md:329`；`preview.html:90-92` |
| 按钮族 | `--control-h`(32) 高、`--r-sm`、12.5px；`primary` 底 `--accent-primary` 字 `--text-inverse` 500；`secondary` 底 `--bg-surface` + `1px --border-strong`；`ghost` 底 `--bg-surface` 无描边字 `--text-secondary`；`danger` 底 `--state-error` 字 `--text-inverse`；内边距 `0 12px`（primary/ghost 见 `:94-97` 为 `0 14px`，见 gap G7）；尺寸 **sm/base**（sm 未定义具体高，见 gap G6） | 五态见上表 | `MASTER.md:335`；`preview.html:70-71`、`:94-97`、`:290-295` |
| `ContextBar` | `padding:0 32px 16px`；含 `Segmented` / 筛选 chips / 视图切换 | — | `MASTER.md:330`；`preview.html:99` |
| `Segmented` | 外框 `background:var(--bg-inset); border-radius:var(--r-sm); padding:3px; gap:2px`；段高 26px（阅读工具栏用 24px）、内边距 `0 12px`、`--r-xs`、12.5px；选中：底 `--bg-elevated`、字 `--text-primary`、500 | 选中态位移动效 `--dur-fast` `--ease-out`；**仅用于 2–4 个互斥枚举，不得表达布尔** | `MASTER.md:338`、`:399`；`preview.html:100-103`、`:541` |
| `Switch` | **布尔专用**：`.sw{min-height:32px; gap:8px}`；轨道 `36×20px`、`--r-full`、底 `--bg-inset` + `1px --border-strong`；滑块 **18px**（原型 CSS 为 `14px` 且 `left:18px`，见 gap G5）、`--r-full`、底 `--text-tertiary`；开：轨道底/描边 `--accent-primary`、滑块底 `--text-inverse`；命中区 ≥32px | 四态 `default / hover / focus-visible / disabled`；禁用 = `opacity:.45`；focus 环 `2px --accent-primary` + offset 2px；`--dur-fast` `--ease-out` 过渡 | `MASTER.md:337`、`:399`；`preview.html:228-237`、`:867-868` |
| 滑块 `.slider` | `width:240px; height:4px; border-radius:2px; background:var(--bg-inset)`，已填段 `--accent-primary` | **MASTER §7 组件清单无 Slider**，只在原型设置页出现 → gap G2 | `preview.html:199-200`、`:582-583` |
| 分段/过滤 chip | `height:30px; padding:0 14px; border-radius:999px; 12px`；`border:1px --border-subtle`；选中：底 `--accent-soft`、字 `--accent-primary`、500、去描边 | MASTER 只提「筛选 chips」，无尺寸规格 → gap G6 | `preview.html:105-107` |

### 2.2 数据展示

| 组件 | 关键样式 | 状态/规则 | 出处 |
|---|---|---|---|
| `DataTable` | 表头 `height:40px; padding:0 16px; 12px/500; background:var(--bg-inset)`；行 `height:48px; padding:0 16px; 13px` + `border-top:1px --border-subtle`；表体底 `--bg-surface`、外框 `1px --border-subtle` + `--r-md`；数字列 `--font-mono` 12px 右对齐；URL 列 mono 12px `--text-tertiary` | 斑马纹可选（MASTER）；hover 行 `--bg-inset`；**行内操作默认 `opacity:.55`，`tr:hover` 或 `tr:focus-within` 时 `opacity:1`**；粘性表头；列宽自适应；状态列须「色点 + 可读文本标签」（色盲可达，`MASTER.md §9`） | `MASTER.md:331`；`preview.html:128-152`、`:141-148` |
| `Card` | 两种：内容卡（书籍）/ 统计卡（数字 + 说明）；`background:var(--bg-surface); border:1px solid var(--border-subtle); border-radius:var(--r-md); padding:18px 20px`；卡内 h2 用 `--fs-h3`(15px)/600、下距 14px | 错误态卡片：`border-color:var(--state-error)` | `MASTER.md:332`；`preview.html:192-194`、`:640` |
| `BookGrid` | 容器 `display:grid; gap:18px 24px; grid-template-columns:repeat(auto-fill, minmax(clamp(132px,11vw,168px),1fr)); padding:0 32px 24px`；卡内 `gap:8px`；封面 `height:200px; border-radius:var(--r-md)`；书名 `--font-reading` 700 `--text-inverse`，按字数取 30/26/22/18px；作者 12px `--text-tertiary`；徽记 `top:10px left:10px; height:22px; padding:0 8px; --r-xs; 11px/500`；进度细线 `bottom:0; height:3px`，已读段 `--accent-clash` | 悬停浮起 + 进度细线 + 连载状态徽记（MASTER 只给语义，无浮起量值）；⚠ 封面「2:3 比例」（`MASTER.md:333`）与原型 `200px` 定高在 132–168px 宽下为 0.66–0.84，**仅在 132px 宽时成立** → gap G3 | `MASTER.md:333`、`§6:288`；`preview.html:110-124`、`:456-468` |
| 徽记 `.badge` | 金底 `--accent-clash` + 字 `--text-primary`；`done`：底 `--state-success` + 字 `--text-inverse`；`off`：底 `--text-tertiary` + 字 `--text-inverse` | ⚠ 夜主题下 `.badge` = 1.63:1、`.badge.off` = 3.48:1（昼 3.77:1）→ 缺陷 D4 | `preview.html:117-120` |
| `StatusPill` | 4 态：成功（苔绿）/ 警告（鎏金）/ 错误（朱红）/ 中性（**暖中性灰** `--state-info`）；原型 `height:22px; padding:0 10px; --r-xs; 11px/500`；前景配对：成功 `--text-inverse`、警告 `--text-primary`、错误 `--text-inverse`、中性 `--text-inverse` | ⚠ 警告态前景在夜主题 = 1.63:1（缺陷 D4）；其余配对实测 5.78–9.05:1 ✅ | `MASTER.md:334`；`preview.html:851`、`:874-880` |
| `ProgressBar` | 细线式，`--accent-clash` 高光（原型为 3px 高细线） | 无显式高度 token → gap G6 | `MASTER.md:345`；`preview.html:121-123` |
| 设置行 `.row` / 目录行 `.drow` | `.row{height:36px; gap:16px}`，标签 13px `--text-secondary`，值 13px/500，数值 `--font-mono` 12px 定宽 60px；`.drow{height:44px; gap:10px; border-top:1px --border-subtle}`，路径 mono 12.5px，计数 mono 12px `--text-tertiary`，文件夹图标 16px `--accent-clash` | 36/44px 非 4px 网格值 → gap G6 | `preview.html:195-198`、`:219-222` |
| 主题色卡 | `--swatch-*` 恒定值；色卡块 `height:64px; --r-md; padding:10px`，三个色点 `28×28 --r-sm`；选中：`border:2px solid --accent-primary` | 必须引用变量，禁内联十六进制 | `MASTER.md:194-203`；`preview.html:204-216` |

### 2.3 浮层

| 组件 | 关键样式 | 状态/规则 | 出处 |
|---|---|---|---|
| 遮罩 `.scrim` | `inset:0; background:var(--overlay-bg); z-index:40`（昼 `rgba(46,36,24,.28)` / 夜 `rgba(0,0,0,.66)`） | — | `MASTER.md:192`；`preview.html:280-282` |
| `Drawer` | 右侧滑入；`width:min(420px, 80%)`；底 `--bg-elevated`；`border-left:1px --border-subtle`；`box-shadow:--shadow-lg`；头 `height:var(--topbar-h)`、`padding:0 20px`、下描边、标题 `--fs-h3`/600；体 `padding:16px 20px` 可滚 | 含遮罩；**`Esc` 关闭**；滑入 `--dur-base` `--ease-out`，位移 = 自身宽度；宽度值无 token → gap G4 | `MASTER.md:339`；`MOTION.md:24`；`preview.html:308-314` |
| `Modal` | 居中；`width:min(560px, calc(100vw - 48px))`；底 `--bg-elevated`、`1px --border-subtle`、`--r-lg`、`--shadow-lg`；头 `padding:18px 20px 0` 用 `--fs-h2`(17px)/600；体 `padding:10px 20px 18px` 13px `--text-secondary`/1.7；脚 `padding:14px 20px` 底 `--bg-surface` + 上描边，右对齐按钮 | 开关动效 `--dur-base` `--ease-spring`：缩放 `.96 → 1` + 透明度 `0 → 1`；用于**破坏性操作二次确认**（如「移除库根目录」「移除书籍」「删除书签」） | `MASTER.md:340`、`:364`；`MOTION.md:25`；`preview.html:283-295`、`:792-799` |
| `Toast` | 右下角（原型 `right:24px; bottom:24px; gap:10px`）；`min-width:260px; max-width:360px; padding:12px 14px`；底 `--bg-elevated`、`1px --border-subtle`、**`border-left:3px` 状态色**、`--r-md`、`--shadow-md`；标题 13px/500、描述 12px `--text-secondary` | 4 态（info/success/error/warning）；**4s 自动消失**；进出：从右下上移 8px + 淡入，`--dur-base` `--ease-out`；设置保存成功用 Toast（2s） | `MASTER.md:341`、`:361`；`MOTION.md:26`；`preview.html:297-306`、`:746-751` |
| `CommandPalette` | 居中弹层；`width:min(560px, calc(100% - 48px))`；底 `--bg-elevated`、`1px --border-strong`、`--r-lg`、`--shadow-lg`；输入行 48px；列表项 `height:36px; --r-sm`；选中项 `--accent-soft` + `--accent-primary` + 500；kbd 右对齐 mono 11px `--text-tertiary`；`Ctrl+K` 唤起，模糊搜索 + 分组 + 上下键 + `Esc` 关闭 | ⚠ **S3 明确排除**（D3 延后，`spec.md:219`）→ 见 gap G10 | `MASTER.md:346`、`:426`；`preview.html:317-330` |
| 阅读工具栏热区 `.hotzone` | `top:0; left:0; right:0; height:8px; z-index:30`；中间 96×3px 把手（`--border-strong`，`opacity:.5`，下圆角 3px） | 热区为**整窗顶部全域**，不随单双页版心变化（裁决 OQ#21/I6） | `MASTER.md:274`；`preview.html:333-335` |

### 2.4 状态组件（每个列表/表格都必须有三态）

| 组件 | 关键样式 | 规则 | 出处 |
|---|---|---|---|
| `EmptyState` | 容器 `padding:48px 32px`、居中、`gap:12px`；插画位 72×72 `--r-lg` 底 `--bg-inset` + 32px 图标（`--text-tertiary`，`stroke-width:1.5`）；h3 `--fs-h3`/600；说明 13px `--text-secondary`、`max-width:42ch`、行高 1.7；主操作按钮组；末行 hint 12px `--text-tertiary` | **禁止只有一行灰字**（`MASTER.md:342`）；须给下一步动作引导 | `MASTER.md:342`；`preview.html:252-259`、`:665-674` |
| `LoadingSkeleton` | 骨架块 `height:14px; border-radius:var(--r-xs)`，底为 `linear-gradient(90deg, var(--bg-inset) 0%, var(--bg-surface) 50%, var(--bg-inset) 100%)`，`background-size:220% 100%`；变体：标题 22px×30%宽、行 44px、`w60/w80/w45` | 微光扫过 **1400ms `linear` 无限循环**——**唯一允许的循环动画**（`MOTION.md:36`）；`prefers-reduced-motion` 降级为**静态 `--bg-inset` 灰块** | `MASTER.md:343`；`MOTION.md:31`、`:36`；`preview.html:265-275` |
| `ErrorState` | 同 EmptyState 结构，`.err` 变体：插画位底 `color-mix(in srgb, var(--state-error) 12%, transparent)`、图标描边 `--state-error`、标题色 `--state-error`；说明 + 重试按钮 + 备选动作 | 须给「重试」与可行动作；原型末行 hint 显示**错误标识**（如 `Storage.DatabaseLocked`、`Storage.RegistryCorrupted`）——标识式、非自然语言，与 AGENTS「引擎零自然语言」红线一致 | `MASTER.md:344`；`preview.html:260-262`、`:687-696`、`:735-744` |
| 内联错误 `.inline-err` | `gap:6px; 12px; color:var(--state-error)`；「重试 / 换目录」为 `--accent-primary` 500 + 下划线 | 设置保存失败**必须显式报错，不得静默**；保留用户输入并标记「未保存」，不回滚 | `MASTER.md:362-363`；`preview.html:240-241`、`:648-651`、`:883-888` |
| 保存状态条 `.savebar` | `gap:8px; 12px; color:var(--text-tertiary)`；状态点 6px `--state-info`；失败态转 `--state-error` | 「保存中…」不得阻塞交互 | `MASTER.md:360`；`preview.html:243-246`、`:637` |

### 2.5 设置型页面的交互契约（改完即存）

出处 `MASTER.md:352-366`。**S3 的设置视图（FR-018）整体受此约束**：

- **不设「保存」按钮**；控件值变更**即刻写入配置**。
- 该页**不强制**「唯一 primary action」——§9 的「有且仅有一个清晰 primary」对设置页按此条款解释（`MASTER.md:359`、`:396`）。
- 保存中：轻量状态（`保存中…`），不阻塞交互。
- 保存成功：复用 `Toast`，**2s** 后自动消失。
- 保存失败：该项旁内联 `ErrorState`（图标 + 原因 + 重试）+ 一条 `Toast`；**不得静默失败**，**不得回滚用户输入而不说明**。
- 回滚语义：保留输入值并标记「未保存」。
- 危险项：仅「删除库根目录」这类破坏性操作用 `Modal` 二次确认，其余一律即时生效。
- 布尔用 `Switch`，2–4 个互斥枚举用 `Segmented`——**不得混用**（`MASTER.md:337-338`、`:399`）。

### 2.6 阅读层专用（S2 已实现几何，S3 负责绘制）

| 组件 | 关键样式 | 出处 |
|---|---|---|
| 阅读器容器 | 底 `--bg-reading`；**全窗浸入式**（原型中无 Sidebar，`preview.html:1021` 明示「不得再减 240px」） | `preview.html:155`、`:1020-1025` |
| 阅读顶栏 `.rtop` | `height:52px; padding:0 20px`；左：返回（13px `--text-secondary`，图标 16px）+ 书名/章（13px/500）；右：进度（mono 12px `--text-tertiary`）+ 单双页 `Segmented`（30px 外/24px 段/12px）+ 字号/主题/更多图标（`.ricon` 18px） | `preview.html:156-162`、`:534-544` |
| 阅读区 `.rpage` | `padding:40px var(--sp-10)`（**即 2×40px，单双页判据的 padding 口径**）；`justify-content:center` | `preview.html:164`、`:1016` |
| 版心 `.measure` | `font-size:var(--reading-font-size)`（**em 相对阅读字号解析**，否则 36em 会按 UI 14px 算错）；`width:100%; max-width:var(--reading-measure)`；段间 `gap:20px` | `MASTER.md:229-237`；`preview.html:165-170` |
| 章号/章题/分隔线 | 章号 13px/500 `--text-tertiary` 居中；章题 `--font-reading` `1.58em`/700、行高 1.45、居中（**随阅读字号等比缩放**，不硬编码像素）；分隔线 `48×2px`、`--border-strong`、`border-radius:1px` | `preview.html:171-174` |
| 正文段 `.p` | `--font-reading`；`font-size:var(--reading-font-size)`；`line-height:var(--reading-line-height)`；`text-align:justify`；`text-indent:var(--reading-paragraph-indent)`；`margin:0` | `preview.html:175-176` |
| 段间距模式 | 与段首缩进**二选一**：`.gapmode` → `text-indent:0; margin-top:var(--reading-paragraph-gap)`，首段 `margin-top:0` | `MASTER.md:235`；`preview.html:178-179` |
| 双页 `.spread` | `display:flex; gap:var(--reading-gutter)(64px); padding:40px var(--sp-10)`；每页版心 `width:var(--reading-measure)`；中缝线 `1px --border-subtle`，`left:-32px` | `preview.html:185-188` |
| 阅读页脚 `.rfoot` | `height:40px; padding:0 40px`；11px `--text-tertiary`；三格：百分比 / 章名 + 页码 / 剩余时间 | `preview.html:180-181`、`:546-550` |
| 工具栏隐形态 | `.rtop.hidden{opacity:0; pointer-events:none}`；`.rtop{transition:opacity var(--dur-base) var(--ease-out)}`；默认隐形（鼠标静止 2.5s / 移出），只留 8px 热区 | `MASTER.md:271-274`；`MOTION.md:32`；`preview.html:336-337` |
| 工具栏入口清单（**10 项，稳定标识**） | `back / toc / progress / font_size / line_height / theme / spread / bookmark / change_source / settings`；几何：热区高 8px、静止 2500ms 淡出、淡出时长 = `--dur-base` | `theme.rs:903-920`（`READING_TOOLBAR`）；`MASTER.md:273` |
| 痕迹与选词（S2/S4 范围） | 三种呈现：高亮底 + 下划线（`--accent-soft` + `2px --accent-clash`）/ 字色方块（底 `--state-warning` + `--text-primary`，或底 `--accent-primary` + `--text-inverse`）/ 下划线（`--accent-primary`，offset 3px、粗细 2px）；词典浮层：底 `--bg-elevated`、`1px --border-strong`、`--r-md`、`--shadow-md`、`padding:10px 12px` | `MASTER.md §9.1:425`；`preview.html:629-630`、`:836-841` |

---

## 3 · 布局规范

### 3.1 应用骨架

`MASTER.md §7:303-317` 的骨架图（TopBar 56 → Sidebar 240/64 + PageHeader → ContextBar → Content，侧栏底部用户区）。阅读视图为**全窗浸入式**（无侧栏，`preview.html:1020-1025`）。

### 3.2 响应式规则（全应用，连续自适应）

出处 `MASTER.md §6:281-295`。**禁止固定 px 断点堆砌**，布局必须连续：

| 区域 | 自适应策略 |
|---|---|
| **AppShell** | 侧栏 `240px` ↔ 折叠 `64px`：容器宽 `< 1024px` 自动折叠为图标栏；`< 768px` 侧栏变为**覆盖式抽屉（Overlay）**，不占流式宽度 |
| **书架网格** | `grid-template-columns: repeat(auto-fill, minmax(clamp(132px, 11vw, 168px), 1fr))`——列数随宽度连续增减，**永不出现半截卡片** |
| **书源/设置表格** | `≥ 1080px` 全列；`< 1080px` 隐藏次要列（收进行展开）；`< 768px` 转卡片式列表 |
| **详情/编辑面板** | `≥ 1280px` 右侧常驻分栏；`< 1280px` 改抽屉（Drawer）从右滑入 |
| **阅读器** | 见 §3.3；侧栏目录在 `≥ 1672px` 常驻，否则抽屉 |
| **工具栏/操作条** | 空间不足时次要按钮收进「⋯ 更多」溢出菜单，**主操作永不隐藏** |
| **弹窗** | 宽 `min(560px, calc(100vw - 48px))`，高度超限内部滚动 |

实现要点（`MASTER.md:295`）：以 **Container Queries + `clamp()` + `auto-fill` 网格**为主，辅以少量 `ResizeObserver` 驱动（阅读器模式判定、抽屉态）。
验收档位（`MASTER.md §9:402`）：**1440 / 1280 / 1024 / 768 四档不崩**，本设计扩展至 **1920 与 640**。原型已归档 1440 与 600 / 1760（`MASTER.md:429`）。

### 3.3 阅读层版心与单双页（核心判据）

**判据不是屏幕宽度，而是「正文版心可用宽度能否容纳两个不小于最小行长的栏」**（`MASTER.md:241`）：

```
available    = 阅读区可用宽度 = 窗口宽度 - 侧栏宽度 - 阅读区左右内边距(2×--sp-10)
singleNeeded = 36em（em 随阅读字号缩放）
双页启用条件：available >= 2 × singleNeeded + --reading-gutter(64px)
```

以默认 19px 换算：`36em = 684px`，门槛 `= 2×684 + 64 = 1432px`（**available 口径**，`MASTER.md:252-261`）。

| 阅读区可用宽度 | 排布 |
|---|---|
| `< 640px` | 单页 · 窄版心 |
| `640–1431px` | 单页 · 版心居中，最大 36em（最舒适单页区） |
| `1432–1671px` | **双页**（左/右并排，中缝 64px） |
| `≥ 1672px` | 双页 · 版心加宽 + **可选目录常驻左侧**（门槛 = 1432 + 目录栏 240） |

**换算到窗口宽（S3 必需）**：阅读区左右内边距 = `2 × --sp-10 = 80px`（`preview.html:1016`），且 S3 的阅读视图**无侧栏**（`spec.md:22` 抽屉式覆盖侧栏不占版心宽）→ `窗口宽 = available + 80`，即 19px 下**双页需窗口宽 ≥ 1512px**、目录常驻档需 ≥ 1752px。原型 `spreadThreshold()` 的算式与之一致（`preview.html:1019`：`36em×2 + 64 + 40×2 = 1512`），但原型提示文案把 1512 标成了「阅读区可用宽度」（`preview.html:1047`）——标签错 80px，见 gap G11。

**四条关键行为**（`MASTER.md:263-267`）：
1. 阈值随**阅读字号**实时重算（em 变），经统一 `relayout()` 驱动，**不用固定 px 断点**。
2. 模式切换**保持阅读位置**——字符偏移锚点（002 §FR-027），切换后滚到同一字符。
3. 用户可手动覆盖「自动 / 强制单页 / 强制双页」；手动选择在本次会话内优先，按**语义回落**失效（仅当与 available 相矛盾时回落），宽度发生不改变该矛盾的变化时**不清除**手动选择（裁决 OQ#14→C）。
4. 双页下章节衔接遵循书页语义（左偶右奇、章末留白补足至偶数页起新章），**可关闭**（设置项「章节从新页开始」）；**单页模式不应用该留白**（裁决 OQ#20→C）。

**单页模式版心封顶**（裁决 I7，`MASTER.md:277`）：单页下版心**封顶 42em 并居中**，**不自动强制分栏**，超出部分留白。

### 3.4 阅读工具栏热区

**整窗顶部全域**（对齐窗口上沿），**不随单页/双页版心变化**——若热区跟版心走，双页下中缝上方会出现热区缺口导致闪烁（裁决 OQ#21/I6，`MASTER.md:274`）。

---

## 4 · 动效规范（MOTION.md）

### 4.1 时长与缓动

| Token | 值 | 语义 |
|---|---|---|
| `--dur-fast` | 150ms | 即时反馈 |
| `--dur-base` | 200ms | 结构变化 |
| `--dur-slow` | 280ms | 场景级转换 |
| `--ease-out` | `cubic-bezier(.22,.61,.36,1)` | 通用 |
| `--ease-spring` | `cubic-bezier(.34,1.3,.64,1)` | 弹入（含轻微过冲） |

`--ease-spring` 控制点 `1.3` 产生约 **3% 过冲**——**只允许用于「从无到有」的入场，不允许用于位置移动**（过冲会让文字错位一瞬）（`MOTION.md:16`）。

### 4.2 元素 → 动效映射（`MOTION.md:18-32`，逐行转录）

| 元素 / 交互 | 时长 | 缓动 | 说明 |
|---|---|---|---|
| hover / 状态色变化 | `--dur-fast` | `--ease-out` | 不移动位置，只改色 |
| focus 环出现 | `--dur-fast` | `--ease-out` | 键盘用户须立即看到 |
| 抽屉滑入 / 滑出 | `--dur-base` | `--ease-out` | 从右滑入，位移 = 自身宽度 |
| 弹窗（Modal）开 / 关 | `--dur-base` | `--ease-spring` | 缩放 `.96 → 1` + 透明度 `0 → 1` |
| Toast 进出 | `--dur-base` | `--ease-out` | 从右下角上移 8px + 淡入 |
| 侧栏展开 / 折叠 | `--dur-base` | `--ease-out` | 宽度 `240 ↔ 64` |
| 分段控件选中态 | `--dur-fast` | `--ease-out` | 高亮块位移 |
| 翻页动效 | `--dur-slow` | `--ease-spring` | 「翻页像纸」 |
| 主题切换 | `--dur-slow` | `--ease-out` | 全站颜色过渡，「像晨昏交替」 |
| LoadingSkeleton 微光 | **1400ms** | `linear` | **唯一允许的循环动画** |
| 阅读工具栏淡出 | `--dur-base` | `--ease-out` | 鼠标静止 2.5s 后 |

### 4.3 禁令（`MOTION.md:38-43`）

- 禁止为**位置移动**使用 `--ease-spring`。
- 禁止直接过渡 `width`/`height`/`top`/`left`——改用 `transform`；确需过渡尺寸时（侧栏折叠）须接受重排代价并加 `will-change`。
- 禁止动效时长超过 `--dur-slow`（骨架屏除外）——桌面应用不是展示页。
- 禁止在主题切换时对**布局属性**做过渡（只过渡颜色）。
- 禁止无意义弹跳与循环动画（`MASTER.md §4:217`）。

### 4.4 减少动效（`prefers-reduced-motion`）

语义：系统开启时**全站降级为 0ms**（`MASTER.md §4` / 002 §FR-032）。逐项对照（`MOTION.md:49-55`）：

| 交互 | 正常 | 减少动效 |
|---|---|---|
| 翻页 | 280ms 翻页动效 | 瞬时换页 |
| 滚动 | 平滑缓动 | 直接位移 |
| 抽屉 / 弹窗 / Toast | 200ms | 瞬时出现 |
| 骨架屏微光 | 1400ms 循环 | **静态灰块** |
| 主题切换 | 280ms 渐变 | 瞬时 |

**关键不变量**（`MOTION.md:57`）：**减少动效不得改变任何几何结果**——分页、滚动流布局、阅读位置在两种设置下必须完全一致。该开关是**渲染层**开关，**不是排版层**开关。`tome-core` 已把它做成类型不变量：`DurationTicks::INSTANT` + `effective(reduced_motion)`（`theme.rs:705-737`）——**所有**动效取值都必须经由 `effective()`，这是 FR-032 唯一安全的实现方式。

---

## 5 · 键盘与焦点规范

### 5.1 焦点态（强制）

| 项 | 规格 | 出处 |
|---|---|---|
| 焦点环 | `outline: 2px solid var(--accent-primary)`，`outline-offset: 2px`，`border-radius: var(--r-xs)`；**两主题均保证可见**；**禁止 `outline:none`** | `tokens.css:167-172`；`MASTER.md:110`、`:350` |
| 焦点环实测对比度 | 昼 5.20–6.98:1 / 夜 5.92–6.78:1（本文件复算，对全部 5 个背景层） | §1.2 |
| 出现动效 | `--dur-fast` + `--ease-out`（键盘用户须立即看到） | `MOTION.md:23` |
| 四态强制 | 所有可交互元素：`default / hover / focus-visible / active / disabled` | `MASTER.md:348-350` |
| 触达目标 | ≥32px（桌面）；**图标按钮 ≥36px** | `MASTER.md:111` |
| 键盘可操作 | §9 验收项「键盘可操作，`focus-visible` 可见」 | `MASTER.md:401` |

### 5.2 焦点顺序 ⚠ 规范缺失

- `MASTER.md` **没有**「焦点顺序」章节；仅 §9 要求「键盘可操作，`focus-visible` 可见」（`:401`）。
- 但 S3 的 US4-AC1 与 FR-017 要求「焦点顺序符合 MASTER.md 的焦点规范」（`spec.md:94`、`:150`）——**该引用悬空**，SC-008 又要求「每个可交互元素有可见焦点态（按 MASTER.md 焦点规范逐项核对）」。plan 必须补一份焦点顺序定义（或把 SC-008 的核对基准改为「元素可达 + 焦点态可见 + 不落入焦点陷阱」），见 gap G13。
- 原型里唯一相关的实现证据：表格行内操作在 `tr:focus-within` 时浮现（`preview.html:144-145`）——即**行内操作也须可 Tab 到达**。

### 5.3 快捷键的呈现规范

| 项 | 规格 | 出处 |
|---|---|---|
| kbd 样式 | `--font-mono`、11px、`--text-tertiary`；在搜索框内右对齐；在命令面板项内 `margin-left:auto` | `preview.html:66`、`:329-330` |
| 键位写法 | 原型用空格分隔且不大写修饰键组合之外的部分：`Ctrl K`、`G S`、`G B`、`G ,`、`Esc` | `preview.html:446`、`:775-780` |
| 阅读键位集 | **必须显式成表**（「`Esc` 之外的任意导航键」是排除法定义、不可验收）。至少覆盖：向前/向后翻页、上/下一章、章首/章末、滚动模式下**行滚动与页滚动**、工具栏唤出、`Esc` 的**栈式取消** | `MASTER.md:275`；002 §FR-035（`specs/002-typesetting-renderer/spec.md:287`） |
| `Esc` 栈式语义 | 阅读区聚焦时 `Esc` 先退出沉浸态/隐藏工具栏；目录或设置以抽屉/弹层形态打开时（**栈顶**），`Esc` 关闭该层 | `MASTER.md:275` |
| 已有实现 | S2 已落地键位表与 `Esc` 栈：`ReadingAction` 12 项（`crates/tome-reader/src/keys.rs:74-133`：`pageNext/pagePrev/chapterNext/chapterPrev/chapterStart/chapterEnd/scrollLineDown/scrollLineUp/scrollPageDown/scrollPageUp/revealToolbar/escape`）、`KeyBinding`（`:146`）、`resolve_key`（`:251`）、`EscLayer` + `EscStack`（`:271`、`:302`） | 复用，勿重造 |
| 设置视图查看快捷键表 | FR-017 要求「快捷键表 MUST 内置并可在设置视图查看」——**MASTER §7 组件清单无「快捷键表」组件**（最接近的是 kbd 样式）→ gap G14 | `spec.md:150` |
| `Ctrl+K` 命令面板 | MASTER §7 定义、§9.1 标 ❌ 待补基线（`:426`）；但 S3 **延后到 D3**（`spec.md:219`）→ 空态里**不得**宣称该快捷键（原型空态 hint 写了「快捷键 Ctrl+K 可打开命令面板」，`preview.html:673`） | gap G10 |

---

## 6 · 空态 / 错误态 / 加载态规范

### 6.1 通用要求

- **每个列表/表格都有空/载/错三态**（`MASTER.md §8 Do:377`）；§9 验收项「空数据 / 加载中 / 错误三态完整」（`:398`）。
- 状态页**复用与正式页面相同的组件与 token**，不引入只为演示而存在的样式——「可直接照抄的基线」（`MASTER.md:432`）。
- ⚠ 该「可直接照抄」在**状态药丸/徽记的前景配对**上有例外（缺陷 D4），须按 §1.2 修正后照抄。

### 6.2 三态的具体形态

见 §2.4 表格（`EmptyState` / `LoadingSkeleton` / `ErrorState` / 内联错误 / 保存状态条）。补充要点：

- **空态禁止只有一行灰字**（`MASTER.md:342`）：必须「插画位 + 标题 + 说明 + 主操作」，说明文本 `max-width:42ch`。
- **错误态必须给「重试」**（`MASTER.md:344`），并显示**错误标识**（标识式、可本地化；引擎不产出自然语言——AGENTS.md「错误须可本地化」红线 + 002 §FR-058）。原型示例：`Storage.DatabaseLocked`、`Storage.RegistryCorrupted`。
- **加载态不白屏**（S3 FR-010）：章节内容加载（本地解析或网络抓取）期间必须有明确加载态；失败给结构化错误 + 可行动作（重试 / 换源 / 返回）。
- 骨架屏是**唯一允许的循环动画**，且必须在减少动效下降级为**静态灰块**。

### 6.3 S3 特有的三态落点（按 spec 逐条映射）

| S3 需求 | 三态落点 | 依据 |
|---|---|---|
| 书架为空 | `EmptyState`：插画位 + 「书架还是空的」+ 说明（支持 TXT/EPUB + 书源）+ 主操作「导入书籍」+ 次操作「导入书源」 | `spec.md:120`（FR-001）；`preview.html:665-674` |
| 书架 1000 册滚动/过滤 | 过滤无结果需**独立空态**（与「书架为空」文案/动作不同）→ 需文案表补条目 | `spec.md:124`（FR-005）；gap G15 |
| 书架读取失败 | `ErrorState`：图标 + 原因 + 「重试」/「打开数据目录」+ 错误标识 | `spec.md:159`（FR-023）；`preview.html:687-696` |
| 搜索无结果 / 搜索中 / 搜索失败 | 需三态；搜索**可取消**（FR-013）→ 需取消入口，MASTER 无对应组件 | `spec.md:143`；gap G16 |
| 书源导入失败 | 结构化原因 + 不留半成品（FR-002/FR-012） | `spec.md:121`、`:142` |
| 章节加载中 / 失败 | 加载态（骨架或占位）+ 错误态（重试 / 换源 / 返回） | `spec.md:137`（FR-010） |
| 目录为空 / 书签为空 | 抽屉内空态（尺寸/文案规格未定义） | `spec.md:109`；gap G17 |
| 设置保存失败 | 内联 `ErrorState` + `Toast` + 保留输入 | `spec.md:151`（FR-018）；`MASTER.md:362-363` |
| 超大章节（≥20 万字） | 加载期间**可取消返回书架**——MASTER 无取消控件规格 | `spec.md:104`；gap G16 |

---

## 7 · 昼夜主题机制

### 7.1 token 切换机制

- 两套主题以属性选择器切换：`:root, [data-theme="day"]`（`tokens.css:53`）与 `[data-theme="night"]`（`:94`）；**`:root` 默认即昼**。
- 同时设置 `color-scheme: light / dark`（`tokens.css:54`、`:95`）——影响原生控件与滚动条的默认外观。
- 切换**只过渡颜色**（`background-color` / `color`，`--dur-slow` + `--ease-out`），**不过渡布局属性**（`tokens.css:158-160`；`MASTER.md:217`；`MOTION.md:43`）。
- 质感随主题变：纸纤维噪点**仅昼**（`--paper-noise` SVG / `--paper-noise-opacity: .045`；夜为 `none` / `0`）；遮罩 `--overlay-bg` 昼夜不同。
- 阴影昼夜不同（昼暖褐投影 / 夜纯黑投影），「暗场靠描边而非阴影分层」（`MASTER.md:178`）。
- 预览色卡**恒定**、不随当前主题变（`tokens.css:136-145`）。
- 主题切换**不得触发重新分页**（002 §FR-019；`theme.rs:1030-1045` 的 `is_night()` 文档明确「不得用于分页」）。

### 7.2 跟随系统的语义 ⚠ 设计系统未定义

- S3 FR-018 要求主题三选：**昼 / 夜 / 跟随系统**（`spec.md:151`）。
- 设计系统侧**只有两态**：`tokens.css` **没有** `@media (prefers-color-scheme: dark)` 分支，`preview.html` 的预览控制台也只有两个主题按钮（`preview.html:967-970`），设置页色卡只有两张（`preview.html:564-577`）。
- 唯一的系统偏好媒体查询是 `prefers-reduced-motion`（`tokens.css:196`）。
- 因此「跟随系统」**必须在应用层解析**为具体 `day` / `night` 两态之一，并订阅系统变更事件；设计系统不提供第三套 token。同时：设置页需要**第三个选项的呈现**（三选一 UI 形态未定义——两张色卡 + 一个「跟随系统」开关？还是三个选项？）→ gap G18。

### 7.3 S3 落点

`tome-core` 已提供 `DayTheme.theme()` / `NightTheme.theme()`（`theme.rs:1131`、`:1225`）产出完整 `Theme` 值；`Theme::is_night()`（`:1030-1045`）供呈现层分流（如是否叠纸噪点）。**S3 不得在 `tome-app` 里重新声明任何十六进制**——MASTER §10 的「单一 theme 模块，禁止各控件散读常量」（`:440`）是 SC-011 的核心可检项。

---

## 8 · Gap 清单（plan 阶段须补设计决策）

按「决策类型」分组。**每条都标了为什么是 gap 与建议处置方向**。

### G1 · 图标体系无规范来源（无 MASTER 章节）

MASTER 全文无图标规范；尺寸/线宽/集合只存在于 `preview.html:85`、`:162`、`:362-380`。与「MASTER 与 tokens.css 双向一致、不得有未定义 token」的纪律（`MASTER.md:8`）冲突。
**须决策**：① 是否把「16/18px 两档 + stroke 1.7 + viewBox 24 + currentColor」升格为 MASTER 条目与 token；② S3 的图标载体（内嵌 SVG path 集合 / 图标字体 / 代码绘制）；③ 17 个图标是否够用（S3 还需：目录、书签、换源、抽屉关闭、过滤、排序、导入、剪贴板、文件、URL、加载取消等，原型集合里没有）。

### G2 · Slider 不在 MASTER §7 组件清单

`MASTER.md §7:325-346` 的组件表**没有 Slider**；但原型设置页用了滑块（`preview.html:199-200`、`:582-583`），S3 FR-007 又要求阅读视图内调字号/行高，FR-018 要求设置视图有默认字号/行高——**必然需要滑块**。
**须决策**：滑块的尺寸（240×4px 轨道？）、已填段色（`--accent-primary`）、把手规格、四态、键盘可操作性（方向键步进值？）、数值读数的呈现（mono 12px 定宽 60px）。同时确认 MASTER 与 tokens.css 是否需要同步补条目。

### G3 · BookGrid 封面比例自相矛盾

`MASTER.md §7:333` 写「封面 **2:3** 比例」，`§6:288` 给列宽 `clamp(132px, 11vw, 168px)`，而原型封面是**定高 200px**（`preview.html:114`）→ 实际比例 0.66（132px 时）~0.84（168px 时），**只有最窄时才是 2:3**。1440px 窗口下 11vw = 158px → 0.79。
**须决策**：用 `aspect-ratio: 2/3`（则行高随列宽变，网格需 `align-content:start` 与行高自适应）还是保持定高（则须改 MASTER 措辞）。此决策直接影响 egui 侧网格的行高计算与虚拟滚动。

### G4 · Drawer 宽度无 token

`MASTER.md:339` 只写「右侧滑入，含遮罩，`Esc` 关闭」；宽度只在原型：`min(420px, 80%)`（`preview.html:308`）。
**须决策**：是否升格为 token；以及**三种抽屉用途是否共用同一宽度**——S3 需要「目录 + 书签」抽屉（`FR-008`）、「详情」抽屉（若采用）、「阅读参数」面板（若采用），宽度需求不同。

### G5 · Switch 滑块尺寸两处不一致

`MASTER.md:337` 明确「滑块 **18px**、轨道 36×20px、命中区 ≥32px」；原型 CSS 是 `width/height: 14px` 且开启态 `left: 18px`（`preview.html:232-236`）——18px 与 14px 差 4px（若按 18px，开启态 `left` 应为 36−2−18 = 16px）。
**须决策**：以 MASTER 为准（18px / left 16px）并修正原型，还是承认原型。S3 实现须有唯一值。

### G6 · 一批常用尺寸只在原型，未进 MASTER/tokens.css

| 值 | 原型出处 | 用途 |
|---|---|---|
| 按钮 `sm` 高度 | 无（MASTER 只说 sm/base，未给值） | 按钮尺寸档 |
| chip `30px` 高 / `0 14px` 内边距 / `--r-full` | `preview.html:105-106` | 筛选 chip |
| Segmented 外框 30px / 段 26px（阅读工具栏 24px） | `preview.html:100-101`、`:541` | 分段控件 |
| 设置行 `36px` / 目录行 `44px` | `preview.html:195`、`:219` | 表单行 |
| 滑块 `240×4px`、数值列 `60px` | `preview.html:198-200` | 设置页 |
| 色卡 `64px` 高、色点 `28px` | `preview.html:204-206` | 主题选择 |
| 搜索框 `380px` 宽 | `preview.html:62` | TopBar |
| Toast `260–360px` 宽、`right/bottom:24px`、`gap:10px` | `preview.html:297-299` | 浮层 |
| 阅读顶栏 `52px` 高 | `preview.html:156` | 阅读视图（`--topbar-h` 是 56px，不适用） |
| 阅读页脚 `40px` 高 | `preview.html:180` | 阅读视图 |
| 书架网格 `gap:18px 24px`、封面 `200px`、徽记 `22px`、进度线 `3px` | `preview.html:110-123` | 书架 |
| 空态 `padding:48px 32px`、插画位 `72px`、图标 `32px` | `preview.html:252-256` | 三态 |
| 热区 `8px` + 把手 `96×3px` | `preview.html:333-335` | 阅读工具栏（几何已在 `theme.rs:903-921`） |
| `14px` 内边距（非 4px 网格值） | `preview.html:94-97`、`:105` | 按钮/chip |
| 表格行 `48px`、表头 `40px` | `preview.html:130-132` | 表格 |

**须决策**：哪些升格为 token（写进 MASTER + tokens.css + `theme.rs`），哪些明确作为「布局内部值」允许实现自定。**注意**：升格 token 会同时改动三份文件（MASTER/tokens.css/`theme.rs`），且 `theme.rs` 有 `tests/theme_contract.rs` 一致性断言——属跨层变更，应在 plan 里单列任务。

### G7 · 原型与 MASTER 的间距纪律冲突（`14px` 非 4px 网格值）

MASTER 规定间距走 4px 网格（`--sp-1..12`，`MASTER.md:147-149`），但原型多处用 `0 14px`（`.ghost-btn`/`.cta`/`.btn-sec`/`.btn-danger`/`.chip`）与 `padding:24px 32px 14px`（`.pagehead`）。
**须决策**：改为 `--sp-3`(12)/`--sp-4`(16) 之一，或承认 14px 为例外并记录。

### G8 · `tokens.css` 骨架尺寸块重复声明

`:9-12` 与 `:44-47` 重复声明 `--topbar-h` / `--sidebar-w` / `--sidebar-w-collapsed`（后者漏 `--control-h`）。CSS 后声明覆盖前声明，当前无害但属漂移源。
**须处置**：合并为一处（对 S3 无功能影响，但 SC-011 的一致性检查会看到）。

### G9 · 内嵌字体只有正文衬线，UI 无衬线与等宽未内嵌

`MASTER.md:441` 要求「打包内嵌字体（衬线正文 + 无衬线 UI + 等宽），确保三平台一致；注意 <20MB 体积预算」；`assets/fonts/` 只有 `TomeSerifCN-subset.otf`（2.28MB）+ 校验和 + OFL。
**为什么对 S3 是硬问题**：界面文案全中文（`spec.md:191`），而 `--font-ui` 首选项 `"Segoe UI Variable Text"` 是 **Windows 专有**、`"PingFang SC"` 是 macOS 专有——**三平台一致性不成立**；egui 默认字体亦不覆盖 CJK。
**须决策**：内嵌 UI 无衬线中文字体子集（体积 +N MB，与铁律 2 的 20MB 基线权衡）还是接受平台字体回退（违反 MASTER §10）。这是 S3 独有的、S2 未遇到的新增体积项。

### G10 · 命令面板：MASTER 有、S3 不做

`MASTER.md §7:346` 定义 `CommandPalette`，§9.1:426 标基线 ❌ 待补；S3 `spec.md:219` 把命令面板延后到 D3。
**须处置**：① S3 界面**不得出现** `Ctrl+K` 命令面板入口或宣称（原型空态 hint、TopBar 搜索框右侧 kbd 都写了 `Ctrl+K`——`preview.html:446`、`:673`）；② TopBar 的「全局搜索」在 S3 里的语义需重新定义（原型是「搜索书架、书源或输入关键词发现新书」三合一，而 S3 只做书架视图内的关键词搜索 FR-013）——否则会出现「点了没反应」的控件。

### G11 · 单双页阈值的口径标签错误

`MASTER.md:252-261` 的 1432px 是**阅读区可用宽度**；原型 `spreadThreshold()` 返回的是**窗口宽** 1512px（含 `2×40px` 内边距，`preview.html:1019`），但提示文案标成「阅读区可用宽度」（`:1047`）。
**须处置**：S3 实现须以 available 判定（`available = 窗口宽 − 2×--sp-10`，因阅读视图无侧栏）；对外文案/调试读数须标对口径，避免 plan 里再出现 612px 级的换算错误（§5.2 的口径声明正是为此而设）。

### G12 · 字重：MASTER 表 vs 原型

MASTER 表给 `--fs-h1 = 22px/1.3/650`、`--fs-h3 = 15px/1.45/600`（`MASTER.md:132`、`:134`）；原型多处写 `font-weight: 700`（`.pagehead h1` `:91`、`.rbook`…）。
**须决策**：以 650/600 为准（则原型需修正），或以 700 为准（则 MASTER 表需改）。两者都在白名单内（≤700），不违禁令，但**同一元素只能有一个值**。

### G13 · 焦点顺序规范悬空

见 §5.2：FR-017 / US4-AC1 / SC-008 引用「MASTER.md 的焦点规范」，但 MASTER 无该章节。
**须决策**：① 在 plan 里补一份 S3 的焦点顺序表（书架 → 阅读 → 抽屉 → 设置 的 Tab 序列、抽屉/弹层的焦点陷阱与归还、Esc 栈与焦点的关系）；② 或把 SC-008 的核对基准改为可判定的三条（元素可达 / 焦点态可见 / 无焦点陷阱）。**建议前者**——否则 SC-008「逐项核对」无基准。

### G14 · 快捷键表的呈现形态未定义

FR-017 要求快捷键表内置且可在设置视图查看（`spec.md:150`）；MASTER §7 无「快捷键表」组件（只有 kbd 样式）。
**须决策**：用 `DataTable`（两列：动作 / 键位）、还是设置页的一组 `.row`（左标签右 kbd）、还是 `Modal` 内的列表；以及是否可打印/复制。

### G15 · 书架「过滤无结果」空态缺文案与动作

MASTER 只有通用 `EmptyState`；S3 需要至少三个语义不同的书架空态：书架为空（引导导入）、过滤无结果（引导清空过滤词）、库根不可访问（错误态）。
**须决策**：三者的标题/说明/主操作/次操作，落进应用层文案表（FR-019 要求全部文案出自文案表）。

### G16 · 「可取消的进行中操作」无组件

FR-013 要求搜索可取消（`spec.md:143`）；Edge Cases 要求超大章节加载期间可取消返回书架（`spec.md:104`）。MASTER 的加载态只有 `LoadingSkeleton`（无取消按钮的形态规定）。
**须决策**：取消入口的形态（内联「取消」按钮？`Toast`？工具栏的「返回」兼作取消？）、以及取消后的状态（结果部分到达时的呈现——FR-013 说「首屏不等待全部」，即**部分结果**是正常态，需要「陆续到达」的呈现规范）。

### G17 · 目录 / 书签页签的列表规格缺失

FR-008/008b/008c 要求抽屉内含目录与书签两页签：目录需**当前章高亮**、跳转；书签需位置预览文本、跳转、删除（需确认）。MASTER §7 无「目录列表」「书签列表」「页签（Tab）」组件。
**须决策**：① 页签控件（MASTER 无 Tab 组件——用 `Segmented`？）；② 目录项的行高/缩进/当前章高亮样式（可参考 Sidebar 当前项：左侧 3px 竖条 + `--accent-soft` 底）；③ 书签项的两行结构（预览文本 + 位置）与删除按钮的确认方式（`Modal` 还是内联二次点击）。

### G18 · 「跟随系统」主题的三态呈现与解析

见 §7.2。设计系统无 `prefers-color-scheme` 分支、无色卡第三项。
**须决策**：① 设置页主题选择的 UI（两张色卡 + 「跟随系统」开关，还是三个平级选项）；② 「跟随系统」开启时用户能否再点具体主题（即三态互斥还是二态 + 开关）；③ 系统主题变化的响应时机（即时 vs 重启）。

### G19 · 详情/编辑面板与「单窗口双视图」的冲突

`MASTER.md §6:290` 规定「详情/编辑面板：`≥1280px` 右侧常驻分栏；`<1280px` 改抽屉」；但 S3 的范围切刀是「**单窗口双视图**（书架 ⇄ 阅读互斥），目录/书签用**覆盖式**侧栏」，且 D2 明确把「三栏常驻」延后（`spec.md:22`、`:218`）。
**须决策**：S3 的「书籍详情（简介/目录 + 加入书架）」（FR-014）用**全屏视图**、**Modal**、还是**抽屉**；并显式声明「§6 的 ≥1280px 常驻分栏在 S3 不适用」（否则 SC-011 的逐项对照会在这一行判违规）。

### G20 · MASTER §9.1 自认未覆盖的四项（其中三项落在 S3）

`MASTER.md:426-429`：命令面板（→ D3，见 G10）、**侧栏折叠 / 覆盖抽屉**（S3 需要：`<1024px` 折叠、`<768px` 覆盖抽屉、阅读视图目录抽屉）、**双主题切换过渡中间帧**、**多宽度布局基线（1920/1280/1024/768）**。
**须处置**：这三项（折叠侧栏、覆盖抽屉、多宽度基线）在 S3 是**必须实现**的（§6 规则 + FR-008），但没有像素基线可对照 → 验收方式需在 plan 明确（自建基线 or 结构化断言：几何关系断言而非像素断言）。

### G21 · 沉浸态的进入/退出语义

MASTER §5.3 提到「阅读区聚焦时 `Esc` 先退出**沉浸态**/隐藏工具栏」，但**「沉浸态」本身未定义**（是否隐藏顶栏？是否全屏？与「工具栏隐形」的默认态如何区分？）。S2 的 `EscLayer` 里有 `Immersion` 层（`crates/tome-reader/src/keys.rs:271-345`）。
**须决策**：沉浸态的视觉定义（隐藏哪些 chrome）、进入方式（快捷键？双击？）、与阅读视图本就「全窗无侧栏」的关系。注意 S3 的阅读视图默认已是全窗浸入（无侧栏），所以「沉浸态」可能退化为「隐藏工具栏」——需明确，否则 FR-035 的 `Esc` 栈语义在 S3 落不下去。

---

## 9 · egui 落点建议

前提：S3 的目标栈是 **egui/eframe**（`spec.md:9`），spike 已钉版 `eframe 0.36.2`（`spikes/s0/Cargo.toml:34`）。以下按「token → egui 机制」给出映射策略与自定义 widget 的范围预估。

### 9.1 单一 token 源（先决条件）

- **`tome-app` 消费 `tome_core::model::theme::Theme`，不新声明任何色值/尺寸**（MASTER §10:440）。`DayTheme.theme()` / `NightTheme.theme()` 已是昼/夜两实例。
- 阅读区若由 S2 绘制，其用色走 `tome_reader::theme::ReadingPalette::for_theme(&theme)`（`crates/tome-reader/src/theme.rs`，已含 focus 环 2px、触达 ≥32px、行高 ≥1.75 等强制项）；S3 若在阅读区叠加自己的元素（工具栏、页脚、进度），也应取同一 `ReadingPalette`，避免阅读区出现两套配色来源。
- **可机检的合规断言**（建议进 SC-011 的自动化部分）：① `tome-app` 源码零十六进制色字面量（grep/测试断言）；② 所有间距取自 `SpacingScale`；③ 对比度断言复算 §1.2 的 ≥4.5 门槛（可复用 S2 的 `reference_contrast`）；④ 触达目标 ≥32 / 图标 ≥36；⑤ focus 环宽 = 2。

### 9.2 `egui::Visuals` 映射（逐项）

| MASTER token | egui 字段 | 备注 |
|---|---|---|
| `--bg-base` | `Visuals.extreme_bg_color`（同时 `Panel` 的 `Frame::fill`） | 面板底需显式给 `Frame`，egui 不会自动取 |
| `--bg-surface` | `Visuals.panel_fill` / `Visuals.widgets.inactive.weak_bg_fill` | 侧栏、卡片 |
| `--bg-elevated` | `Visuals.window_fill` + `Visuals.menu_corner_radius`/`window_corner_radius = --r-lg` | 弹窗/抽屉/Toast |
| `--bg-inset` | `Visuals.faint_bg_color`（输入槽、骨架块、表头底） | — |
| `--text-primary` | `Visuals.widgets.noninteractive.fg_stroke.color` / `override_text_color` | 用 `override_text_color` 会抹掉语义色差异，**不建议** |
| `--text-secondary` / `--text-tertiary` | `Visuals.widgets.noninteractive.weak_fg_stroke` / 逐处 `RichText::color()` | egui 只有两级弱化，第三级须逐处指定 |
| `--accent-primary` | `Visuals.selection.bg_fill`（浅底）+ `Visuals.selection.stroke`（焦点/选中描边）+ `Visuals.hyperlink_color` | ⚠ 焦点环 offset 2px **无法**由 `selection.stroke` 表达（见 9.4） |
| `--accent-hover` | `Visuals.widgets.hovered.bg_fill`（仅主按钮；全局 hover 底应留 `--bg-inset`/`--accent-soft`） | egui 的 hover 底是全局的，主按钮的「底变 `--accent-hover`」需自定义 widget |
| `--accent-soft` | `Visuals.widgets.hovered.weak_bg_fill` / `Visuals.selection.bg_fill` | 选中行、当前项、分段高亮 |
| `--border-subtle` | `Visuals.widgets.noninteractive.bg_stroke` / `Visuals.window_stroke` | — |
| `--border-strong` | `Visuals.widgets.inactive.bg_stroke` / `Visuals.widgets.hovered.bg_stroke` | ⚠ 见缺陷 D2：不可作唯一边界 |
| `--state-*` | `Visuals.error_fg_color`（仅 error 有原生位）；其余须逐处 `RichText::color()` | egui 无 warning/success/info 语义位 |
| `--shadow-sm/md/lg` | `epaint::Shadow { offset: [i8;2], blur: u8, spread: u8, color }` | ⚠ **`--shadow-lg` 的 spread = −12px 不可表达**（`u8` 无负值）→ 需自绘阴影或近似（把 blur 调大、spread 置 0） |
| `--r-xs/sm/md/lg/full` | `CornerRadius { nw, ne, sw, se: u8 }`（0.31+；旧名 `Rounding`） | ⚠ `--r-full = 999` 超出 `u8` 且需按元素高的一半取；须写 `min(999, h/2)` 的适配层 |
| `--sp-1..12` | `Style.spacing.item_spacing` / `button_padding` / `window_margin: Margin` / `indent` | 只覆盖部分；区块间距与内边距仍需逐处 `add_space()` |
| `--control-h = 32px` | `Style.spacing.interact_size = vec2(32, 32)` | 图标按钮 36px 需逐处 `min_size`（egui 无「图标按钮」概念） |
| 滚动条 `10px` + `--border-strong` + hover `--accent-primary` | `Style.spacing.scroll: ScrollStyle { bar_width: 10.0, floating: false, ... }` | ⚠ 滑块配色走 `widgets.inactive/hovered.bg_fill`，**与普通控件共用** → 要么接受偏差，要么自绘滚动条 |
| 斑马纹（可选） | `Visuals.striped: bool`（或 `egui_extras::TableBuilder::striped`） | 原生支持 |
| `--fs-*` 八档 | `Style.text_styles: BTreeMap<TextStyle, FontId>`；`TextStyle::Name("h1".into())` 可注册自定义档 | ⚠ egui 的 `FontId.size` 是**点**，与 px 在 1.0 缩放比下等值；须按 `pixels_per_point` 处理 HiDPI |
| 字重 400/500/600/650/700 | `FontId` 无字重维度；`FontFamily::Name("ui-medium")` 等**按字重注册独立字体族** | ⚠ egui 不支持同一字体的多字重选择——需把每档字重作为独立 `FontFamily` 注册（或用可变字体 + `FontTweak`，能力有限）。这是 egui 侧最实质的排版妥协点 |
| `--font-reading` / `--font-ui` / `--font-mono` | `FontDefinitions.families`：`Proportional` ← UI、`Monospace` ← mono、`FontFamily::Name("reading")` ← 衬线 | 见 G9：UI/等宽 CJK 字体未内嵌 |
| `--paper-noise`（昼） | **无对应**：egui 无 `mix-blend-mode`，也不能渲染 SVG data-URI | 需**预生成**可平铺噪点位图（`ColorImage`）或程序化噪声，再以 `opacity≈.045` 叠加；multiply 与 alpha 混合的差异在 .045 下可忽略但须记录 |
| `--dur-*` / `--ease-*` | `Context::animate_bool_with_time(id, b, secs)` / `Style.animation_time` / `animate_value_with_time` | ⚠ egui 提供 `emath::easing` 函数族但**无 cubic-bezier 求值器**；`cubic-bezier(.22,.61,.36,1)` 需自写求值（或近似为 `easing::cubic_out` 并记录偏差）。`--ease-spring` 的过冲必须自写（egui 无 spring） |
| 减少动效 | `Style.animation_time = 0.0` + 所有 `animate_*` 传 0；对应 `DurationTicks::INSTANT` / `effective(reduced_motion)`（`theme.rs:705-737`） | 建议把 `effective()` 作为唯一取值口，egui 侧不得自行判断 |

### 9.3 布局与响应式的 egui 化

- MASTER §6 的 `clamp()` / `auto-fill` / Container Queries 在 egui 无对应 → 必须写成**「容器可用宽度 → 列数/分栏」的连续函数**（MASTER §10:442 正是此要求）。
  - 书架列数：`cols = max(1, floor((avail + gap) / (clamp(132, 0.11*avail, 168) + gap)))`，卡片宽 = 均分余量；`gap = (18, 24)` 取自原型（gap G6）。
  - 侧栏：`avail < 1024 → 折叠(64)`、`avail < 768 → 覆盖抽屉`；阅读器双页：`avail - 80 >= 2*36*reading_font + 64`（§3.3）。
- **虚拟滚动**：SC-005 要求 1000 册 60fps → 用 `ScrollArea::show_viewport` + 自算网格（`show_rows` 只适合等高行）；表头粘性用 `egui_extras::TableBuilder`（新增依赖，需按 MASTER §6/§7 的 DataTable 要求核对：斑马纹、hover 行、列宽自适应、行内 hover 操作）。
- **最小窗口尺寸未定义**（gap，MASTER §6 只说连续自适应，原型最小画板 600px）→ plan 需给 `min_inner_size` 一个值（建议 ≥ 640，对齐 §5.2 的 `<640px` 窄版心档）。
- **窗口控制**：MASTER §7 骨架图写了 TopBar 含「窗口控制」（`MASTER.md:306`），但无规范 → 若走 `decorations: false` + 自绘标题栏，需处理拖拽区（`ViewportCommand::StartDrag`）、最大化/最小化/关闭、以及 macOS 交通灯位置；否则用系统装饰栏、TopBar 不放窗口控制。gap，需 plan 定夺。

### 9.4 焦点与键盘（egui 的具体缺口）

- **无 `focus-visible` 概念**：egui 用 `Response::has_focus()` 与 `widgets.active/hovered` 混合表达。MASTER 要求「真实可见的环 + 2px offset」→ 须自绘：`ui.painter().rect_stroke(rect.expand(2.0), corner, Stroke::new(2.0, accent), StrokeKind::Outside)`，且**只在键盘导航来源时**绘制（记录最近输入来源），否则鼠标点击也会亮环（与原型 `:focus-visible` 语义不符）。
- **Tab 顺序无显式 API**：egui 的焦点顺序 = 当帧 widget 注册顺序（`Sense`/`interact` 调用序）。因此「焦点顺序」在 egui 里是**实现涌现**而非声明——这正是 G13 要求补一份显式焦点顺序表的现实理由：否则 SC-008 的「逐项核对」无法在 egui 上稳定复现。建议把焦点顺序做成**可测**的：以 `egui::Context::run` + 合成 `RawInput`（Tab 序列）离线驱动，断言焦点落点序列（egui 可无窗口运行，天然适合 headless 测试）。
- **`Esc` 栈**：`EscStack`（`crates/tome-reader/src/keys.rs:302`）已实现分层取消；egui 侧需把 `Key::Escape` 优先交给该栈（注意 egui 自身的 `Modal`/`Area` 也消费 Esc → 需在输入分发顺序上明确「S3 的栈优先于 egui 默认行为」）。
- **快捷键表**：S2 的 `KeyBinding` / `resolve_key`（`keys.rs:146`、`:251`）已是键位集的单一来源；S3 的 egui 事件→`KeyEvent` 适配层应薄，**不得**在 S3 另建一份键位映射（否则 FR-017 的「内置快捷键表」与阅读行为会漂移）。
- **AccessKit**：`eframe` 默认开启 accesskit；**自绘 widget（Switch/Segmented/StatusPill/书架卡/进度线/骨架）必须显式提供无障碍信息**（`Response::widget_info` / `Ui::label` 语义），否则读屏丢失。003 Assumptions 已声明「按 eframe 既有 AccessKit 能力接入」（`spec.md:196`）——自绘部分的 `widget_info` 是兑现该声明的最小工作量。

### 9.5 自定义 widget 范围预估

| 组件 | egui 原生可用的部分 | 需自绘/自管的部分 | 规模 |
|---|---|---|---|
| `Switch` | 无（egui 只有 `Checkbox`） | 全自绘：36×20 轨道 + 18px 滑块 + 开/关/四态 + 焦点环 + `widget_info` | S（~150 行） |
| `Segmented` | `SelectableLabel` 可作段 | 容器底/内边距/选中位移动效/键盘左右切换 | S–M |
| `StatusPill` | `Label` + `Frame` | 底/前景配对（**须按缺陷 D4 按主题分流**）+ `--r-xs` + 22px 高 | S |
| `Button` 四态 | `Button`（`Visuals` 已覆盖 hover/active/disabled） | active 的 `translateY(1px)` + 内阴影、focus 环、`--accent-hover` 底 | S |
| `Card` / `PageHeader` / 设置行 | `Frame` + `Ui` 布局 | 精确内边距/描边/圆角 + 行高 36/44 | S |
| `DataTable` | `egui_extras::TableBuilder`（**新依赖**） | 斑马纹、粘性表头、行内 hover 操作、状态列「色点 + 文本」、列宽自适应 | M |
| `BookGrid` | `ScrollArea::show_viewport` | 网格列数连续函数、封面（渐变色/占位图/比例决策 G3）、徽记、3px 进度线、悬停浮起、虚拟化 | M–L |
| `EmptyState` / `ErrorState` | `Frame` + `Label` + `Button` | 插画位 72×72、`max-width:42ch` 折行、错误标识行 | S–M |
| `LoadingSkeleton` | 无 | 自绘块 + 1400ms `linear` 微光（`request_repaint` 驱动）+ 减少动效降级为静态 | M（动效驱动易错） |
| `Toast` | 无 | `Area`（`Order::Foreground`）+ 右下堆叠 + 4s 计时 + 进出动效 + 4 态左边线 | M |
| `Modal` | `egui::Modal`（0.30+ 原生，自带遮罩） | `--r-lg`/`--shadow-lg`（阴影 spread 负值不可表达）/头体脚三段内边距 | S–M |
| `Drawer` | `SidePanel` **不适用**（会占布局宽，与「覆盖式不占版心」冲突） | 须用 `Area` + 手绘遮罩 + 右滑入动效 + `Esc` 关 + 焦点陷阱 | M |
| 焦点环 helper | 无 | 键盘来源判定 + 2px 环 + 2px offset 绘制 | S（但需全局一致，建议做成 `Ui` 扩展方法） |
| kbd 小标签 | `Label` + `Frame` | mono 11px + `--text-tertiary` + 内边距 | S |
| 纸噪点层（昼） | 无 | 噪点纹理生成 + 全窗叠加 | M |
| 自绘滚动条（若要求 hover 变主色） | 部分（`ScrollStyle`） | 与普通控件配色解耦需自绘 | M（可选） |
| 阅读页正文 | **不应用 egui 重排** | 见 9.6 | — |

合计：**约 12–16 个自绘/半自绘组件**，其中 `BookGrid`（虚拟化）、`Drawer`（覆盖式 + 焦点陷阱）、`LoadingSkeleton`（循环动效）、`DataTable`（新依赖）是四个风险点，建议在 plan 里各自留独立任务与验收。

### 9.6 与 S2 渲染层的集成（阅读页）

- **正文不得由 egui 排版**：S2 才是唯一排版源（中文禁则/挤压/两端对齐/字符锚点/单双页），`tome-reader` 的 `paginate`/`layout` 是唯一正确性来源；egui 重排会同时破坏锚点契约（001 §FR-028 / 002 §FR-027）与 SC-006。
- 两条可行集成路径：
  1. **离屏纹理 → egui 贴图**：S2 已有 `render/offscreen.rs`（`specs/002-typesetting-renderer/plan.md` 与 `crates/tome-reader/src/render/` 中的离屏路径，`MASTER`/002 的截图 harness 即走此路）→ 把每页渲染到 `wgpu::Texture`，再交给 egui 显示。改动面最小，但需处理每帧/每页的纹理上传（`egui_wgpu::Renderer::register_native_texture` 可零拷贝，需 S2 暴露 texture 句柄）。
  2. **`egui_wgpu::CallbackTrait` 自定义 paint callback**：在 egui 的 render pass 中直接跑 S2 的管线。零拷贝、无中间纹理，但要求 S2 的渲染管线可接受外部 `RenderPass`（当前 S2 的 API 未必暴露此形态）→ 需先做 spike。
- **建议**：路径 1 优先（与 S2 既有离屏产物一致、风险低），把「零拷贝」作为后续优化；无论哪条，**主题色必须同源**——阅读页走 `ReadingPalette::for_theme(&theme)`，chrome 走 `Visuals` 映射，两者都由同一个 `Theme` 值派生，切换主题时只需换 `Theme` 实例（对应 tokens.css 的 `data-theme` 切换）。
- **依赖与体积纪律**：egui/eframe 是图形依赖，**只能出现在 `tome-app`**（组合根），绝不能进 `tome-core`/`tome-reader` 的依赖图——AGENTS.md 的依赖图断言（`cargo build -p tome-core` 独立 + 传递依赖零图形栈 + `scripts/check-reader-features.sh`）必须继续通过。新增 `tome-app` 后，CI 的依赖断言需相应扩展（把 `tome-app` 纳入「允许含图形栈」的白名单，同时断言 `tome-core`/`tome-reader --no-default-features` 仍零图形包）。体积账按 003 既有裁决在 `size-report.md` 滚动记录（`spec.md:194`）。

---

## 附：本文档的自检边界

- 已逐行读取：`MASTER.md`(451)、`MOTION.md`(57)、`tokens.css`(202)、`preview.html`(1080)；交叉核对 `crates/tome-core/src/model/theme.rs`、`crates/tome-reader/src/{theme.rs,keys.rs,toolbar.rs}`、`crates/tome-reader/tests/theme_accessibility.rs`。
- 对比度全部由本文件用 WCAG 2.1 公式独立复算（未采信 MASTER 声明值，见 §1.2）。
- **未做**：`design-system/shots/` 的 24 张 PNG 未逐张像素核对（本文档只依据 `preview.html` 的源码数值）；`preview.html` 中与 S3 无关的两页（书源管理、阅读统计）未纳入组件清单；设计系统的「书源管理/本地库/阅读统计」页不属 S3 范围（D1/D3 延后），其组件仅在与 S3 重叠处（DataTable、Card、三态、抽屉）被引用。
- 本文件为**只读提取**，未改动仓库任何代码或设计系统文件。
