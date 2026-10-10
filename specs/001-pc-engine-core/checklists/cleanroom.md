# 干净室轴审查（001-pc-engine-core / T101）

**审查日期**：2026-10-11
**审查对象**：`D:/Code/tome` 全库（main @ `cfbdac8`）
**上游基准**：`D:/Code/legado` @ `5198645f0`（只读参照，仅黑盒/文档用途）
**审查依据**：宪法原则 II（干净室纪律，NON-NEGOTIABLE）+ AGENTS.md 三条铁律第 3 条
**审查性质**：独立于五轴（正确性/可读性/架构/安全/性能）的**额外一轴**——宪法原则 VII 要求

---

## 结论

**通过。** 全库扫描未发现上游 Kotlin 移植痕迹，未发现注释/文档/提交消息引用或摘录上游源码。
唯一的 Java 文件是本项目自研的 Jsoup 公开 API oracle（`spikes/s0/tools/JsoupDomDump.java`、
`tools/ref-runner/JsoupRuleOracle.java`），与上游源码无血缘。

---

## 扫描覆盖面

本审查用 10 组模式扫描全库，覆盖"直接引用"与"移植痕迹"两类风险。

| # | 扫描项 | 模式 | 命中数 | 判定 |
|---|---|---|---|---|
| 1 | 上游项目名 | `legado`（大小写不敏感） | 16 文件 | **全部合规**（见 §附：逐条） |
| 2 | Kotlin 源码扩展名 | `\.kt\b` / `\.kts\b` | 5 处 | **全部合规**（均为"不读 Kotlin"的声明） |
| 3 | 语言名 | `kotlin` | 同上集合 | 全部合规 |
| 4 | 上游包名/类名 | `io\.legado` / `com\.gedoor` / `me\.ag2s` / `Gedoor` | **0** | 无 |
| 5 | 上游 Java 类名（Jsoup 之外） | `AnalyzeRule` / `RuleAnalyzer` / `BookSourceBean` / `SourceRule` / `SearchBook` | **0** | 无 |
| 6 | 上游仓库路径引用 | `/d/Code/legado` / `D:/Code/legado` | 1（`ref-runner.sh` 的 `UPSTREAM` 变量） | **合规**（黑盒运行器的指向，非源码引用） |
| 7 | 移植表述 | `照抄` / `逐行移植` / `移植自` / `搬运自` / `ported from` | 6 处 | **全部是否定式表述**（见 §附） |
| 8 | Kotlin 语法特征出现在 Rust 注释 | `fun x(` / `companion object` / `@JvmStatic` / `suspend fun` / `when(` | **0** | 无 |
| 9 | 英文移植表述 | `original implementation` / `derived from` / `based on the upstream` / `source code of` | **0** | 无 |
| 10 | 提交消息（160 条，含 body） | `legado` / `kotlin` / `\.kt` / `照抄` / `逐行移植` | 0 条含源码引用 | **合规** |

### 补充核对

| 项 | 结果 |
|---|---|
| 本仓库 `*.kt` / `*.kts` 文件 | **0 个** |
| 本仓库 `*.java` 文件 | 2 个，均为自研 Jsoup oracle（见下） |
| `Cargo.lock` 中的上游派生 crate | **0**（无 `legado`/`gedoor`/`ag2s` 相关） |
| 引入 Java/Kotlin 生态依赖 | **0** |
| 许可证声明 | workspace 级 `MIT OR Apache-2.0`，9 个 crate 全部继承（`license.workspace = true`） |

---

## 附：命中逐条裁定

### 上游项目名的 16 个文件（扫描 #1）

按性质分四类，**全部属允许用途**：

| 类别 | 文件 | 性质 |
|---|---|---|
| **公开文档引用** | `crates/tome-rule/src/default.rs` | 语义依据是公开的《Legado 书源规则说明》§1「JSOUP之Default」——**书源规则的公开语法文档**，是宪法原则 II 明确允许的"公开文档" |
| **公开文档引用** | `tests/corpus/css.json`、`tests/corpus/default.json` | 合成用例的 `basis` 字段引用同一份公开文档，是语料可追溯性的要求 |
| **规格/流程文档** | `.specify/memory/constitution.md`、`AGENTS.md`、`specs/001-pc-engine-core/*.md`（plan/data-model/contracts/quickstart） | 描述"上游只作功能基线索引参照"的纪律本身 |
| **黑盒运行器 + 项目说明** | `tools/ref-runner/*`、`spikes/s0/tools/*` | 运行器指向与干净室约定 |
| **产品文案 / 测试数据** | `design-system/preview.html`（"支持 Legado 书源格式"）、`crates/tome-source/tests/android_backup_import.rs`（`"legado/3.0"` 作为 User-Agent 测试字符串） | 前者是产品对用户的事实陈述；后者是构造 Android 备份样本的**输入数据**，非实现 |

> **判据**：宪法原则 II 允许"依据书源作者的原始规则与公开文档独立推导"。
> 上述引用**全部指向公开文档或产品事实**，无一指向上游实现源码。

### 「不读 Kotlin」的 5 处声明（扫描 #2/#3）

| 文件 | 原文要点 |
|---|---|
| `crates/tome-local/src/epub.rs:25` | "本模块**不读任何上游 Kotlin 实现**（干净室纪律）。上述四份规范都是公开…" |
| `crates/tome-local/src/mobi/header.rs:7` | "…不读上游 Kotlin 实现**（干净室纪律）" |
| `crates/tome-local/src/mobi/huffcdic.rs:11` | "…非上游 Kotlin 源码）独立实现。**未读任何 GPL 实现的代码**" |
| `crates/tome-local/src/mobi/palmdb.rs:8` | "本模块**不读任何上游 Kotlin 实现**（干净室纪律）" |
| `crates/tome-store/src/backup_android.rs:23` | "…`.kt`/`.java` 源码（宪法原则 II）。故实现上采取宽容策略" |

> 这 5 处是**干净室纪律在代码里的显式落点**（AGENTS.md 要求"为兼容上游语义而写的非常规代码必须注明依据来源"）。
> 它们声明"没有读"，而非"读了什么"——是纪律的正面证据。

### 移植表述的 6 处（扫描 #7）

全部为**否定式或无关**用法：

| 文件:行 | 原文 | 判定 |
|---|---|---|
| `crates/tome-reader/tests/corpus_loading.rs:89` | "不得写「照抄上游」，那违反干净室纪律" | 否定式（测试断言的一部分） |
| `crates/tome-reader/tests/corpus_loading.rs:95` | `for placeholder in ["照抄", "上游源码", "TODO", "待补", "无所谓"]` | **守卫实现**：显式拒绝这些占位词 |
| `crates/tome-rule/src/lib.rs:17` | "不得写"照抄某实现"（干净室纪律，宪法原则 II）" | 否定式（规范陈述） |
| `crates/tome-cli/tests/headless.rs:560` | "逐行找依赖声明" | 无关（描述解析算法） |
| `crates/tome-local/src/txt.rs:51` | "**不逐行 split 再正则**" | 否定式（性能设计说明） |
| `crates/tome-reader/src/corpus.rs:139/158/163` | "逐行的字符数/压缩量/是否超宽" | 无关（排版度量术语） |

> 第 2 条尤其值得记下：`corpus_loading.rs` 有一条**自动化守卫**——语料的 `basis`
> 字段若写成"照抄"或"上游源码"这类占位词，测试直接失败。这把干净室纪律从
> 人工审查提升为**可执行的断言**。

### 两个 Java 文件的来源确认

| 文件 | 行数 | import 清单 | 判定 |
|---|---|---|---|
| `spikes/s0/tools/JsoupDomDump.java` | 284 | 仅 `org.jsoup.*` + JDK（`java.io`/`java.nio`/`java.util`） | 自研，只用 Jsoup 公开 API |
| `tools/ref-runner/JsoupRuleOracle.java` | 431 | 同上，无任何 `io.legado` 包 | 自研，只用 Jsoup 公开 API |

两者的文件头均含独立声明：*"本文件不读取、不移植上游任何 Kotlin 源码。Jsoup 是公开的
第三方库，此处只使用其公开 API"*。`grep` 确认无 `io.legado`/`AnalyzeRule`/`BookSource`
等上游标识符。

**Jsoup 的正当性**：上游用 Jsoup 解析 HTML 并跑 CSS 选择器；本项目改用 html5ever + 自建 DOM
（research.md R2）。两边解析同一份乱网页得到的树是否一致，是书源兼容性的地基——
故用 Jsoup 作对照臂是**黑盒 oracle**，不构成源码接触。

### 运行器的实际源码接触（复核 T084 台账）

`tools/ref-runner/ref-runner.sh` 全文检索 `cat` / `head` / `less` / `Read` 与 `.kt` 相关调用：

| 命中 | 判定 |
|---|---|
| 第 70 行 `java -version 2>&1 \| head -1` | 用于取 JDK 版本号，**不涉及上游文件** |
| 第 225 行 `cat >&2 <<'EOF'` | heredoc 输出帮助文本，**不读任何文件** |

`UPSTREAM="${UPSTREAM_ROOT:-/d/Code/legado}"` 是该变量唯一出现处，用作**路径标注**（运行器
不进入它，只在 §环境探测里报告"上游仓库存在"）。与 T084 的 `CLEANROOM.md` §6 台账一致。

---

## 复核 T084 的既存结论

`tools/ref-runner/CLEANROOM.md`（T084 交付物）已记录：

- §1 白名单：只允许黑盒运行可执行产物 + 读构建元数据/公开文档；唯一显式授权的源码文件是
  `avd.bat`/`avd.sh`（模拟器启动脚本，共 88 字节，已摘录全文——**非应用实现**）
- §2 红线：禁止打开任何 `.kt`/`.kts`/`.java` 源码
- §6 操作台账：本波对上游的全部 9 类接触逐条列明，**无一读取实现源码内容**
- 签字：执行 session `2150180b` 声明 + 独立核对人 **PM session** 复核确认（2026-10-10，上游 @ `5198645f0`）

**本次审查复核该台账的结论：一致。** `ref-runner.sh` 的静态审查与台账描述相符；
上游基准 commit 与台账记载相同（`5198645f0`）。

---

## 遗留与说明

1. **本审查是静态扫描，不能证明"从未打开过上游源码"**——那只能靠过程记录（T084 台账 + 核对人签字）。
   本审查能证明的是：**当前全库不存在上游源码的移植痕迹或引用**。两者互补。

2. **`tome-reader` 的骨架**属 S2 范围，本次一并扫描（其 `corpus_loading.rs` 的干净室守卫
   是正面证据）。S2 的完整干净室审查应由 S2 自己的收口任务执行。

3. **扫描未发现"逐段结构对应"型移植**。这类痕迹（变量名顺序、分支结构、注释措辞逐条对应）
   难以用 grep 检出，本次通过抽查各 crate 模块文档的**依据来源标注**判断：
   实现文档普遍标注公开规范（RFC / OCF 3.0 / OPF 3.0 / NCX / XPath 1.0 等）或公开书源规则文档，
   **未见"对应上游某文件"型标注**。

---

## 审查签署

| 项 | 内容 |
|---|---|
| 审查 session | P9（本条即 T101 执行会话） |
| 审查日期 | 2026-10-11 |
| 被测版本 | main @ `cfbdac8` |
| 上游基准 | `D:/Code/legado` @ `5198645f0` |
| 扫描模式数 | 10 组 + 5 项补充核对 |
| 结论 | **通过**（无移植痕迹、无上游源码引用） |
| 待 PM 复核 | 本文件结论需 PM 在合并前独立复核（宪法原则 VII：不审查不合并） |
