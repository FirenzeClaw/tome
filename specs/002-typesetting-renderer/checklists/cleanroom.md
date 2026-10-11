# 干净室轴审查（002-typesetting-renderer / T052）

**审查日期**：2026-10-11
**审查对象**：S2 排版渲染层（`crates/tome-reader/`、`crates/tome-cli/` 的 S2 部分、`specs/002-typesetting-renderer/`）
**被测版本**：main @ `16aae0f`
**上游基准**：`D:/Code/legado`（只读参照，仅黑盒运行与行为存在性用途）
**审查依据**：宪法原则 II（干净室纪律，NON-NEGOTIABLE）+ AGENTS.md 三条铁律第 3 条
**审查性质**：独立于五轴（正确性/可读性/架构/安全/性能）的**额外一轴**

---

## 结论

**通过。** S2 层未发现上游 Kotlin 移植痕迹，未发现注释/文档/提交消息引用或摘录上游源码。
零 `.kt`/`.kts` 文件，零上游包名/类名，`legado` 在 S2 两个 crate 中**零命中**。

---

## 扫描覆盖面

| # | 扫描项 | 模式 | S2 范围内命中 | 判定 |
|---|---|---|---|---|
| 1 | 上游项目名 | `legado`（大小写不敏感） | `crates/tome-reader/`、`crates/tome-cli/` **0** | ✅ |
| 2 | Kotlin 源码扩展名 | `*.kt` / `*.kts` 文件 | **0**（全库） | ✅ |
| 3 | 上游包名/类名 | `io\.legado` / `com\.gedoor` / `me\.ag2s` / `Gedoor` | **0** | ✅ |
| 4 | 上游规则类名 | `AnalyzeRule` / `BookSourceBean` / `SourceRule` | **0** | ✅ |
| 5 | 移植表述 | `照抄` / `逐行移植` / `移植自` / `搬运自` / `ported from` | 6 处 | **全部是否定式或测试夹具**（见 §附 1） |
| 6 | Kotlin 语法特征出现在 Rust | `fun x(` / `companion object` / `@JvmStatic` / `suspend fun` | **0** | ✅ |
| 7 | 英文移植表述 | `original implementation` / `derived from` / `source code of` | **0** | ✅ |
| 8 | 层内提交消息 | `legado` / `kotlin` / `照抄` | 0 条含源码引用 | ✅ |

### 补充核对

| 项 | 结果 |
|---|---|
| S2 层的依据来源形态 | 全部指向**公开规范**（UAX#14 LineBreak.txt / CLreq / CSS Text / MASTER.md）或**项目裁决编号**（OQ/FR/SC） |
| `basis` 字段的强制 | 由 `tome-reader/src/corpus.rs` 的解析器**在类型层拒绝**空 `basis`（不是靠代码审查） |
| 语料里的依据 | 逐条可追溯到公开规范或 MASTER（见 `tests/corpus/kinsoku.json` 的 `basis` 字段） |

---

## §附 1：6 处「移植表述」逐条核对

| 位置 | 原文 | 判定 |
|---|---|---|
| `crates/tome-reader/src/rules/cjk.rs:3` | "依据必须是公开规范，**不得照抄**任何实现" | ✅ 否定式（纪律声明） |
| `crates/tome-reader/src/rules/cjk.rs:711` | `for placeholder in ["照抄", ...]` | ✅ 测试夹具（断言禁用词不出现在语料里） |
| `crates/tome-reader/tests/corpus/README.md:84` | "**不得**写「照抄上游某文件」" | ✅ 否定式（语料编写规范） |
| `crates/tome-reader/tests/corpus_loading.rs:89` | "不得写「照抄上游」" | ✅ 否定式（断言消息） |
| `crates/tome-reader/tests/corpus_loading.rs:95` | `for placeholder in ["照抄", ...]` | ✅ 测试夹具 |
| `crates/tome-rule/src/lib.rs:17` | "不得写「照抄某实现」" | ✅ 否定式（001 既有，非 S2 新增） |

**零处**是"本文照抄自上游某文件"这类肯定式引用。

---

## §附 2：S2 规则来源的可核对性（干净室的正面证据）

干净室纪律要求语义"依据书源作者的原始规则与公开文档独立推导"。S2 的中文排版
规则逐条落在**可核对的公开来源**上：

| 规则 | 落点 | 依据 |
|---|---|---|
| 禁则（行首/行尾不可出现的标点） | `src/rules/cjk.rs` | UAX#14 LineBreak.txt 的 CL/OP/EX 类；CLreq §3.1.1 |
| 标点挤压 | `src/rules/squeeze.rs` | CLreq §3.1.5（半字身压缩） |
| 中西混排间隙 | `src/rules/mixed.rs` | CLreq §3.2；CSS Text 的字间处理 |
| 两端对齐 | `src/rules/justify.rs` | CSS Text Level 3 的 `text-align: justify` |
| 视效参数（颜色/间距/行高/单双页判据） | `design-system/MASTER.md`（用户拍板） | MASTER 是唯一设计规范来源 |
| 项目级取舍 | `specs/002-typesetting-renderer/OPEN-QUESTIONS.md` 的 OQ 编号 | 22 项已裁决 |

**偏离规范的非常规处置**（如破折号对与省略号的补丁条目）在 `basis` 里注明
**偏离方向与理由**，而非假装是规范原文——这是宪法原则 II 对"非常规代码"的要求。

---

## §附 3：本层的黑盒用途记录

S2 期间对上游客体的使用**仅限**：

| 用途 | 形态 | 位置 |
|---|---|---|
| 行为存在性证据 | 查"上游有这项能力吗、行为边界在哪" | 规格讨论（`spec.md` 的 Context A） |
| 黑盒运行 | 运行产生输入→输出数据 | `tools/ref-runner/`（**不读源码**） |

**未发生**：打开上游 `.kt` 源码、逐行比对实现、把上游结构搬进本层。

---

## 未通过项

**无。**
