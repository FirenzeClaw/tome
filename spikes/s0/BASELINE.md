# S0 技术栈可行性基线（T006-T008）

**特性**: 001-pc-engine-core | **测量日期**: 2026-10-09 | **测量机**: Windows 10+ / x86_64-pc-windows-gnu

本文件是 S0 可行性门的**证据记录**：tasks.md T006/T007/T008 的验收数据与结论。
上游 `research.md` 的 R1/R2 两项"遗留待办"（rquickjs 实测体积增量、Jsoup↔html5ever
真实差异率）在此闭环。

> **数据来源说明**：所有数字均为**本机实测**，命令与原始输出见各节。
> 未实测的项明确标注为"遗留"，不以推断冒充实测。

---

## 1. 环境与工具链

| 项 | 值 |
|---|---|
| OS | Windows（10+） |
| 工具链 | `cargo 1.99.0` / `rustc 1.99.0`（stable-x86_64-pc-windows-gnu） |
| 构建 profile | 继承根 `Cargo.toml` 的 `[profile.release]`：`lto="fat"`、`codegen-units=1`、`strip=true`、`panic="abort"` |
| 便携 JDK（T008 用） | Temurin **21.0.12.1+1**，解到 `D:\dev\toolchain\jdk\jdk-21.0.12.1+1`（免安装、不进 C 盘、不改系统环境变量） |
| Jsoup（T008 用） | **1.23.2**（版本对齐上游 `gradle/libs.versions.toml` 的声明；从 `maven.aliyun.com` 取得） |

**spike 依赖**（`spikes/s0/Cargo.toml`）：`eframe 0.36.2` / `rusqlite 0.40.2 (bundled)` /
`reqwest 0.13.5 (rustls, blocking)` / `rquickjs 0.14.0`（可选 feature `spike-js`）/ `html5ever 0.40.1`。

> **注**：spike 刻意**脱离主 workspace**（见其 `Cargo.toml` 文件头说明）：
> eframe 是图形依赖，绝不能进引擎的依赖图（FR-036）。

---

## 2. T006 验证项结论

### 2.1 四类基础能力可跑通（Windows-gnu 实测）

命令与输出：

```
$ cargo run --quiet --features spike-js -- --headless
[spike] headless 模式：不开窗口
[spike] rusqlite OK：SQLite 3.53.2，写读往返值 = tome-s0-roundtrip，库位于 C:\...\Temp\tome-s0-spike\spike.sqlite
[spike] rquickjs OK：eval 结果 = quickjs:ok:42
[spike] headless 全部通过；本次运行未发起任何网络请求

$ cargo run --quiet -- --http
[spike] reqwest OK：GET https://example.com/ → 200 OK，577 字节
```

| 能力 | 结论 |
|---|---|
| eframe 开窗口 | ✅ 实测通过：进程存活且主窗口句柄有效（标题 `tome S0 spike`，`MainWindowHandle=12978334`） |
| rusqlite 写读 | ✅ SQLite 3.53.2（bundled C 源码自编译，无系统依赖） |
| reqwest HTTP | ✅ rustls + HTTP/2，`example.com` 200 |
| rquickjs eval | ✅ QuickJS 0.14.0，表达式求值正确 |

**窗口路径的证据取法**（不是"跑起来没报错"这种弱证据）：

```powershell
$p = Start-Process -FilePath '.\target\release\tome-s0-spike.exe' -PassThru
Start-Sleep -Seconds 3
Get-Process -Id $p.Id | Select-Object ProcessName, MainWindowTitle, MainWindowHandle
# → ProcessName=tome-s0-spike  MainWindowTitle=[tome S0 spike]  MainWindowHandle=12978334
```

非零的 `MainWindowHandle` 才是"窗口真的创建了"的判据——进程存活可能只是
事件循环在空转。

### 2.2 rquickjs 三平台编译

| 平台 | 结论 |
|---|---|
| windows-gnu | ✅ **本机实测通过**（含 eval 运行） |
| macos / linux | ⏳ **遗留**：CI job 已就绪（`.github/workflows/ci.yml` 的 `s0-spike` job），**尚未实跑** |

> **为什么尚未实跑**（2026-10-09 核实）：本仓库配置了 remote
> `git@github.com:FirenzeClaw/tome.git`，但
> - 本地 `origin/main` 引用停留在 Phase 1 的 `19f6086`（无本次任何提交）
> - `git ls-remote --heads origin` 对 `refs/heads/main` 返回全零哈希
> - `git show origin/main:.github/workflows/ci.yml` 中**不含** `s0-spike` job
>
> 即：**新增的 spike job 从未到达远端**，故 macOS/Linux 的 spike 构建在任何
> 地方都未曾运行。本项**不能**标为已验证——只标"job 就绪 + 待 push 后实跑"。

### 2.3 无非用户触发的网络外发（FR-041 / SC-012）

**方法**：用两套独立工具监控 spike 进程的全部 TCP 连接。

1. `netstat -ano` 轮询进程 PID（60 次采样）
2. `Get-NetTCPConnection -OwningProcess <pid>`（PowerShell 7，5ms 间隔）

第二个工具的脚本已随结论入仓库（`monitor-process-network.ps1`）——结论若不
可复现就等于没有证据。复现方式：

```powershell
$p = Start-Process -FilePath '.\target\release\tome-s0-spike.exe' -ArgumentList '--headless' -PassThru
pwsh -NoProfile -File monitor-process-network.ps1 -TargetPid $p.Id -Seconds 1
# → TOTAL_DISTINCT=0
```

**结论**：headless 路径**零 TCP 连接**（含未建立的 SYN 尝试）。

**监控器自身有效性已验证**（防止"永远为 0"的假阴性）：对同一监控器观察一个
已知联网的进程（`curl https://example.com`），检出 4 条连接
（`Bound|:::0`、`SynSent|::1:443`、`Bound|0.0.0.0:0`、`SynSent|127.0.0.1:443`）。
故 `TOTAL_DISTINCT=0` 是真实的"无外发"，而非监控失效。

**局限**：只覆盖 TCP。spike 的 headless 路径不做任何名称服务调用，
且其代码中不存在 HTTP 调用（见下），故 UDP/DNS 方向无外发来源。

**设计层保障**：`--headless`（CI 路径）**不含任何 HTTP 调用**；HTTP 走独立的
`--http` 子命令，须用户显式触发。即"无外发"不是靠纪律，而是路径本身不存在外发。

### 2.4 产物体积与启动耗时

**体积**（release，含 rquickjs；见 §3 的两臂对照）：

| 产物 | 字节 | MiB |
|---|---|---|
| `tome-s0-spike.exe`（含 rquickjs） | 13,483,520 | 12.86 |

**启动耗时**（release，`--headless`，10 次采样，进程起止计时）：

| 统计量 | 值 |
|---|---|
| 中位数 | **116 ms** |
| 最小 | 107 ms |
| 最大 | 125 ms |

原始样本（ms）：`107, 110, 110, 114, 115, 116, 121, 124, 125, 125`

> **口径说明**：此为**进程冷启动到退出的全耗时**（含 SQLite 建库 + QuickJS
> runtime 创建 + 一次 eval），比"窗口可交互时间"更严格——后者需人工观察，
> 不具可复现性。故取前者作为基线。窗口路径的首帧耗时由 spike 内部记录，
> 但未纳入本基线（无自动化采集手段，不编造数字）。

---

## 3. T007：rquickjs 接入的体积增量（research.md R1 遗留待办）

**方法**：同一 spike 骨架，仅切换 `spike-js` feature，两次 release 构建。
**唯一变量是 rquickjs 是否在依赖图中**（默认臂不含 rquickjs 依赖，
见 `spikes/s0/Cargo.toml` 的 `[features]` 说明）。

```
$ cargo build --release                          # 空壳臂
$ cargo build --release --features spike-js      # 含 rquickjs 臂
```

| 臂 | 字节 | MiB | 说明 |
|---|---|---|---|
| 空壳（eframe + rusqlite + reqwest + html5ever） | 12,083,200 | 11.52 | |
| 含 rquickjs | 13,483,520 | 12.86 | |
| **增量** | **+1,400,320** | **+1.335 MiB（+1367.5 KiB）** | |

**相对增幅**：**+11.6%**

### 结论与解读

- research.md R1 引用的"官方 210 KiB 口径"指的是 **QuickJS 引擎本体的编译产物**，
  不含 rquickjs 的 Rust 绑定层、`rquickjs-macro` 过程宏、`rquickjs-core` 的
  类型桥接，以及（关键）**静态链接后被 LTO 保留的 sys 绑定与字符串表**。
  实测 **1.34 MiB** 是"接入 rquickjs 的真实代价"，应以此为准。
- **对体积纪律的影响**：宪法「附加约束·体积纪律」以 `<20 MB` 为目标基线。
  spike 的 12.86 MiB 已包含 eframe（图形栈，占大头且**不进引擎**）。
  引擎侧（tome-core 及以下）无图形依赖，1.34 MiB 的 QuickJS 代价完全可承受。
- **未纳入本差值**：`spike-js` 臂与空壳臂的差异**仅** rquickjs 及其传递依赖，
  因为 spike 未引入其他可选依赖——这是刻意的实验设计。

---

## 4. T008：Jsoup ↔ html5ever DOM 差异量化（research.md R2 遗留待办）

### 4.1 方法

**语料**：11 个乱网页 HTML（`spikes/s0/corpus/`），覆盖典型解析歧义：

| 样本 | 覆盖的错误类型 |
|---|---|
| `01-unclosed-tags.html` | 未闭合的 `<p>`/`<li>`/`<span>`/`<div>` |
| `02-misnested.html` | 行内与块级交错嵌套、`<table>` 缺 `tbody` |
| `03-entity-abuse.html` | 非法/不完整实体、裸 `&`、属性内实体、`&nbsp;` |
| `04-table-misalignment.html` | 表格错位（触发 foster parenting）、未闭合表格 |
| `05-comment-script-style.html` | 注释内双减号、条件注释、脚本/样式内容含标签 |
| `06-cdata-and-data.html` | `<svg>`/`<math>` 外部内容、`<textarea>`/`<template>` |
| `07-head-body-autofix.html` | 无 `html/head/body` 包装，孤立 `<title>`/`<meta>`/`<style>` |
| `08-duplicate-attributes.html` | 重复属性名、属性名大小写、空属性名 |
| `09-deep-nesting.html` | 30 层深嵌套 |
| `10-real-world-messy.html` | 真实书源页形态（未加引号的属性、`<br/>` 混用、中文内容） |
| `11-large-stress.html` | 227 KB / 300 章 / 14,423 节点的大规模压力样本（混合上述全部乱构造） |

**两侧实现**：

- **Jsoup 侧**（对照臂）：`tools/JsoupDomDump.java`，Jsoup **1.23.2** 解析后
  按统一约定先序序列化。**干净室合规**：只调 Jsoup 的**公开 API**
  （`Jsoup.parse` + 节点访问器），未读取或移植上游任何 Kotlin 源码。
- **html5ever 侧**（被测臂）：`src/dom_diff.rs`，html5ever 0.40.1 +
  **自建最小 arena DOM**（实现 `TreeSink`），**未使用 `markup5ever_rcdom`**
  （research.md R2 已定）。

**度量**：两侧序列化为「深度 + 节点类型 + 名称/内容」的行序列，逐行比对。
两侧**共用同一套序列化约定**（元素名小写、属性按名排序、文本折叠空白），
否则差异率测的是"约定不同"而非"解析不同"。

### 4.2 结果

```
$ cargo run --quiet -- dom-diff corpus
```

| 样本 | jsoup 块 | html5ever 块 | 差异块 | 差异率 | html5ever 空文本节点 |
|---|---|---|---|---|---|
| 01-unclosed-tags.html | 24 | 24 | 0 | 0.0% | 6 |
| 02-misnested.html | 26 | 26 | 0 | 0.0% | 8 |
| 03-entity-abuse.html | 15 | 15 | 0 | 0.0% | 7 |
| 04-table-misalignment.html | 32 | 32 | 0 | 0.0% | 13 |
| 05-comment-script-style.html | 14 | 14 | 0 | 0.0% | 8 |
| 06-cdata-and-data.html | 22 | 22 | 0 | 0.0% | 7 |
| 07-head-body-autofix.html | 12 | 12 | 0 | 0.0% | 5 |
| 08-duplicate-attributes.html | 11 | 11 | 0 | 0.0% | 6 |
| 09-deep-nesting.html | 35 | 35 | 0 | 0.0% | 8 |
| 10-real-world-messy.html | 9 | 9 | 0 | 0.0% | 2 |
| **11-large-stress.html** | **14423** | **14423** | **0** | **0.0%** | 0 |

**汇总：11 个样本，0 / 14623 块不一致，总体差异率 0.00%**

> **单位说明**："块"而非"行"：超过 256 字符的序列化行会被切块后比对，
> 使长文本节点内部的差异也能被检出（见 §4.2.2 的踩坑记录）。常规节点
> 一节点即一块，故小样本的块数等于节点数。

### 4.2.1 为什么需要 `11-large-stress.html`

前 10 个样本合计仅 3.8 KB——用它们得出"0% 差异"不足以排除"小样本恰好一致"。
故追加一个 **227 KB / 300 章 / 14,423 节点** 的合成压力样本，混合了全部乱构造
（未闭合的 `<h3>`、交错的 `<div>`/`<span>`、缺引号的属性、缺 `<tr>` 的表格、
实体滥用、10 层深嵌套）。**该样本同样 0 差异**。

### 4.2.2 度量方法本身的两个缺陷（实测发现并已修）

这一节记录的是**工具**的缺陷，不是两侧解析的差异——但若不修，结论就不可信。

| # | 现象 | 根因 | 处置 |
|---|---|---|---|
| 6 | 压力样本第一版只报 7 块、差异率 0.0%，**却什么也没测到** | 该版语料漏写 `</title>`，整篇文档落入 `<title>` 的文本节点。按"一行 = 一个节点"比对时，6 个巨长文本节点就是 6 行，内部差异被静默吞掉 | 语料构造修正（补 `</title>`）；同时把比对改为**长行按 256 字符切块**，使长文本内部的差异也能被检出（新增 4 个回归测试） |
| 7 | 切块后差异率算出 **283%** 这种无意义数值 | 分母用**原始行数**、分子用**分块后的差异数**——两者单位不一致 | 分子分母统一为"块数"，并加测试断言差异率恒 ≤ 100% |
| 8 | 首差异的输出左右两侧**看起来一模一样**，读者无从判断差异在哪 | 只印两个块的前 120 字符前缀，而两块共享该前缀 | 改为跳过公共前缀、**报出首个不同字符的位置**（如"首异于第 223 字符"）并附上下文 |

这三个缺陷都属同一类：**度量工具自身的 bug 会伪装成"结论良好"**。
故修复后各加了回归测试（spike 单测从 7 个增至 11 个）。

### 4.3 结论

**在本次语料范围内，Jsoup 1.23.2 与 html5ever 0.40.1 + 自建 DOM 的 DOM 树形完全一致
（198/198 行逐字节相同）。** 这是对 research.md R2「html5ever + 自建 arena DOM」
路线的**正面实测支持**。

### 4.4 系统性差异点（**实测发现，已全部处置**）

迭代过程中出现的差异**全部不是"两侧解析不同"，而是本 spike 的实现/约定缺陷**。
逐条记录，因为它们每一条都是 S1 实现 `tome-rule` 时的**已知坑**：

| # | 现象 | 根因 | 处置 | 对 S1 的意义 |
|---|---|---|---|---|
| 1 | 差异率虚高 ~99% | 根 `Document` 节点在 Rust 侧占深度 0、Jsoup 侧子节点从 0 起编号 → 全表错位 1 层 | 两侧统一"根不出行但占一层" | 序列化/编号约定必须两侧同构 |
| 2 | 差异率虚高 ~90% | Jsoup **保留**空白文本节点，html5ever **丢弃** → 后续行全部错位 | 两侧同时**剔除空文本节点**，并把它**单列为独立指标**（见上表最后一列） | 空白保留策略是真差异，但属"策略"非"结构"；单独统计，不污染差异率 |
| 3 | `<template>` 内容缺失 | html5ever 把模板内容挂在**独立内容片段节点**上，Jsoup 视作模板的普通子节点 | Rust 侧序列化时**内联**内容片段 | `get_template_contents` 的语义差异必须显式处理 |
| 4 | 脚本/样式内容类型不符 | Jsoup 用 **`DataNode`**，html5ever 一律走 `Text` | 自建 DOM 增设 `Data` 节点类型，按**父元素名**判定 | research.md R2 预判的"节点类型细分"**实测确认必要**；且**不能靠"前一个兄弟"推断**（首个文本子节点会误判），必须看父元素 |
| 5 | `&nbsp;` 语义分叉 | Rust `char::is_whitespace(U+00A0)` = **true**，HTML/Java = **false** → `&nbsp;` 被折叠成普通空格 | 显式采用 WHATWG「ASCII whitespace」集合（tab/LF/FF/CR/space） | **重要**：ASCII 空白集合只有 5 个字符，不含 U+00A0。正文净化若用 Rust 内建判定，会与上游分叉（违反宪法 I） |

**剩余系统性差异（结构性，非缺陷）**：仅"空白文本节点保留策略"
（Jsoup 保留、html5ever 丢弃，本批语料合计 Rust 侧 70 个空文本节点）。
**影响评估**：书源的 CSS 选择器**不会**因空白文本节点而匹配到不同元素
（CSS 选择针对元素节点；`:contains()` 只看文本内容，空文本无内容）。
故对书源兼容性**无实际影响**，但若日后需要逐字节对齐 Jsoup 的 DOM，
这是一项已知工作量。

### 4.5 结论的边界（诚实声明）

- **语料规模**：11 个样本 / 14,623 块。这是**可行性门**的量级，不是覆盖率证明。
  样本刻意选取"最易分叉"的构造（错位表格、外部内容、模板、脚本、深嵌套），
  其中一个 227 KB 的合成压力样本用于排除"小样本恰好一致"。故 0% 差异率是有
  信息量的；但它**不能**外推为"任意网页都一致"。
- **度量方法已修但仍有代价**：长行按固定宽度（256 字符）切块而非按语义边界，
  故差异的**定位**是"第 N 块第 M 字符"，而非精确的节点路径。正式差分测试应
  改为按**节点**遍历并比对（每节点一个哈希），既能定位又不受行宽影响。
- **未覆盖的已知风险区**（research.md R2 点名的）：SVG/MathML 的**外部内容
  属性调整细节**（本批只验证了简单 `<circle>`/`<mi>`）、Jsoup 的 `maxDepth=512`
  默认上限（html5ever 无此上限）、以及**罕见字符引用**（如 Windows-1252
  映射区 0x80–0x9F）。
- **语料全部为合成内容**：`10-real-world-messy.html` 与 `11-large-stress.html`
  虽模仿真实书源页形态，但仍非真实抓取页面（真实书源的对照数据按仓库约定
  不入版本库）。
- **后续**：S1 的 T020（自建 arena DOM）落地时应**扩大语料**并把这些区域纳入
  差分语料库（`tests/corpus/`），届时以正式差分测试替代本 spike。

---

## 5. S0 检查点对照（tasks.md 第 51-56 行）

| 检查项 | 状态 | 证据 |
|---|---|---|
| 三平台均产出可运行骨架产物 | ⚠️ **部分** | Windows 实测 ✅；macOS/Linux CI job 就绪、**从未实跑**（见 §2.2 的核实说明） |
| 体积与启动耗时基线已记录 | ✅ | §2.4 / §3 |
| 无非用户触发的网络外发 | ✅ | §2.3（双工具监控 + 监控器有效性验证；脚本已入仓库可复现） |
| rquickjs 在三平台编译通过 | ⚠️ **部分** | Windows-gnu 实测 ✅；macOS/Linux 同上 |
| **人工审查技术栈实测数据后再继续** | ⏳ | **本文件的读者即审查者** |

**遗留问题（须 PM/人工裁决）**：

1. **spike 的三平台一致性未经实跑**：`s0-spike` CI job 在本地已就绪并经 YAML
   语法校验、`cargo` 命令逐条本机复现，但**从未到达远端**（`origin/main` 上
   的 `ci.yml` 不含该 job）。故 macOS/Linux 的 spike 构建**没有任何实跑证据**。
   （附带发现：远端 CI 的 `lint` job 因 `rust-toolchain.toml` 钉了
   `stable-x86_64-pc-windows-gnu` 而在 ubuntu runner 上失败——该 job 未像
   `build` job 那样覆盖 `RUSTUP_TOOLCHAIN`。此项不在 T006-T012 范围内，
   留给负责 CI 的会话处理。）
2. **`panic="abort"` 与测试的交互**：spike 的 release profile 继承 `panic="abort"`，
   而 `cargo test --release` 需要 unwind。本 spike 的测试用 debug profile 跑
   （已验证通过）；CI 的 spike 单测步骤未指定 `--release`，故不受影响。
   **但 S1 若引入 `release` 模式的测试需留意此点**。
3. **未固化的观测项**：`09-deep-nesting` 只到 30 层，未触及 Jsoup 的
   `maxDepth=512`；该上限与 html5ever 的差异属已知未验证区（见 §4.5）。

---

## 6. 可复现命令速查

```bash
# spike 构建与验证（Windows 本机）
cd spikes/s0
cargo build --release                          # 空壳臂（T007 对照）
cargo build --release --features spike-js      # 含 rquickjs 臂
cargo test                                     # 7 个单测
cargo run --quiet -- --headless                # 无窗口、无网络外发（CI 路径）
cargo run --quiet -- --http                    # 显式一次 HTTP

# T008 DOM 差异（需先跑 Java 侧导出）
export JAVA_HOME=/d/dev/toolchain/jdk/jdk-21.0.12.1+1
cd tools
"$JAVA_HOME/bin/javac" -encoding UTF-8 -cp lib/jsoup-1.23.2.jar -d . JsoupDomDump.java
"$JAVA_HOME/bin/java" -Dfile.encoding=UTF-8 -cp "lib/jsoup-1.23.2.jar;." JsoupDomDump ../corpus
cd ..
cargo run --quiet -- dom-diff corpus           # 逐样本差异率 + 汇总
cargo run --quiet -- dom-dump corpus/01-unclosed-tags.html   # 单样本 html5ever 侧序列化（调试用）

# rquickjs 依赖约束守卫（含 spike 覆盖）
cd ../..
bash scripts/check-rquickjs-features.sh

# T006 无网络外发验证（脚本随结论入仓库，保证可复现）
cd spikes/s0
pwsh -NoProfile -Command "
  \$p = Start-Process -FilePath '.\target\release\tome-s0-spike.exe' -ArgumentList '--headless' -PassThru
  pwsh -NoProfile -File monitor-process-network.ps1 -TargetPid \$p.Id -Seconds 1"
# → TOTAL_DISTINCT=0
```
