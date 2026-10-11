# 宪章合规复检（002-typesetting-renderer / T052）

**复检日期**：2026-10-11
**被测版本**：main @ `16aae0f`（S2 Phase 1-8 主体完成）
**复检对象**：`.specify/memory/constitution.md`（v1.0.0）8 条核心原则 + 3 项附加约束
**复检依据**：宪法 Governance「每个实施计划必须包含宪法合规检查」+ 原则 VII「不审查不合并」
**复检范围**：S2 层（`crates/tome-reader/`、`crates/tome-cli/` 的 S2 部分）

---

## 总览

| 原则 | 名称 | 可强制性 | S2 结论 |
|---|---|---|---|
| I | 兼容性优先 | NON-NEGOTIABLE | ✅ 达标 |
| II | 干净室纪律 | NON-NEGOTIABLE（合规红线） | ✅ 达标（独立轴见 `cleanroom.md`） |
| III | 测试先行 | NON-NEGOTIABLE | ✅ 达标 |
| IV | 简单性与单向依赖 | — | ✅ 达标 |
| V | 安全无例外 | — | ✅ 达标 |
| VI | 原子提交与主干开发 | — | ✅ 达标 |
| VII | 不审查不合并 | — | ✅ 达标 |
| VIII | 可读性与显式契约 | — | ✅ 达标 |
| 附 | 体积纪律 / 平台承诺 / 范围纪律 | — | ✅ 达标（体积见 `size-report.md`） |

**8 条全部达标。** 本阶段发现并整改了 **1 处原则 VIII 的实质违反**（见原则 VIII 与 §整改记录）。

---

## 原则 I：兼容性优先（NON-NEGOTIABLE）

> 兼容性 > 性能 ≈ 视效与体验完整性；"兼容性"特指**书源兼容性**。

| 判据 | 证据 | 结论 |
|---|---|---|
| 书源的 `HighlightRule.style` 字符串被正确解释 | FR-033 的三级解析（语义词表 → 颜色名 → 默认下划线 + 诊断），`tests/trace_exactness.rs` 全测 | ✅ |
| 书源不因本层而失效 | 本层**不接触书源**——正文唯一来源是 001 的 `ContentProvider` 接缝（FR-004），排版与渲染全在其下游 | ✅ |
| 未知样式不静默 | 第三级回退**必产诊断**且携带原样式串（`unknown_styles_fall_back_to_underline_with_diagnostic`） | ✅ |
| 性能判据让位于兼容性 | 性能基线（SC-007~010）全达标且余量充足（见 `perf-baseline.md`），未发生"为性能牺牲兼容"的取舍 | ✅ |

---

## 原则 II：干净室纪律（NON-NEGOTIABLE，合规红线）

见独立轴 `cleanroom.md`：**通过**。零移植痕迹、零上游包名/类名、零 `.kt` 文件，
6 处「移植表述」全部是否定式或测试夹具。

---

## 原则 III：测试先行（NON-NEGOTIABLE）

> 没有失败的测试就不写生产代码。

| 判据 | 证据 | 结论 |
|---|---|---|
| 每个切片有先失败的测试 | S2 全程 TDD：43 个集成测试文件 + 各模块单测；本阶段 8 个任务的每个 commit 都含新测试 | ✅ |
| 测试可脱离显示器运行 | headless 构建（`--no-default-features`）全绿：2944 passed（T049 实测） | ✅ |
| 装置自检（防空心判据） | 多处：验证器**双向验证**（错期望必被抓、对期望必通过）、计数分配器自检、快照 harness 注入差异自检、判别器自检 | ✅ |

---

## 原则 IV：简单性与单向依赖

| 判据 | 证据 | 结论 |
|---|---|---|
| 依赖方向单向（星型无环） | `tome-reader` 直接依赖仅 `tome-core` + 排版/渲染第三方库；**不依赖** `tome-source`/`tome-store`/`tome-net` | ✅ |
| 引擎不依赖图形栈 | `scripts/check-reader-features.sh`：`--no-default-features` 的依赖树零图形包（CI `headless` job 断言） | ✅ |
| 逻辑不在 CLI 重复实现 | T048 的 `verify-layout` 把求值器放 `tome-reader/src/corpus_eval.rs`，CLI 只装载/序列化（契约 §1 薄壳） | ✅ |
| 无重复实现 | 排版只有一份；渲染面经 feature 二分可完全剥离（T051 的净增量测量即其证据） | ✅ |

---

## 原则 V：安全无例外

| 判据 | 证据 | 结论 |
|---|---|---|
| 生产代码零 `unsafe` | `crates/tome-reader/src/lib.rs` 有 `#![forbid(unsafe_code)]`；唯一 `unsafe` 在**测试**的计数分配器（`GlobalAlloc` 类型上必需，范围最小豁免并注明理由） | ✅ |
| **零网络外发**（SC-016） | `crates/tome-reader/src/` 无任何 HTTP 客户端引用（0 命中）；本层正文来源是 001 的接缝 | ✅ |
| 沙箱设限 | 本层不含 JS 执行（那属 001 的 `tome-js`）；渲染/排版不接受不可信输入面 | ✅ |
| 外部输入校验 | 语料 JSON 全部字段必填（`#[serde]` 无 default），`basis` 空串在解析期被拒 | ✅ |

---

## 原则 VI：原子提交与主干开发

| 判据 | 证据 | 结论 |
|---|---|---|
| 主干可部署 | main 分支直接开发，全程 `cargo test --workspace` 绿（T048-T052 后 3098 passed / 0 failed） | ✅ |
| 提交原子 | 本阶段 8 个提交，每个是一个逻辑完整单元（`7c2d27c..HEAD`），单提交规模均在 ~100–600 行区间 | ✅ |
| 提交消息解释 why | 逐条检查：全部说明"为什么这么改"而非"改了什么" | ✅ |

---

## 原则 VII：不审查不合并

| 判据 | 证据 | 结论 |
|---|---|---|
| 五轴审查 | 每个任务经五轴（正确性/可读性/架构/安全/性能）自审；本复检即其独立一轴 | ✅ |
| 干净室审查独立成轴 | `cleanroom.md`（本次产出） | ✅ |
| 三级标注 | 本阶段发现的缺陷按 [必须修复] 处置（见 §整改记录） | ✅ |

---

## 原则 VIII：可读性与显式契约

> 错误须可本地化；引擎内禁止硬编码面向用户的自然语言（FR-044 / FR-058）。

| 判据 | 证据 | 结论 |
|---|---|---|
| 错误分类全覆盖 | `ReaderError` → `ErrorKind` 的映射覆盖七类（Network/Proxy/Rule/UnsupportedCapability/Content/Storage/Configuration）；`tests/error.rs` 的映射表逐项断言 | ✅ |
| 无面向用户的自然语言硬编码 | `no_natural_language.rs` 扫描：**0 处**真正的 `#[error]` 中文串（见 §整改记录） | ✅（整改后） |
| 能力错误结构化 | `render`/`fonts` 在 feature-off 时返回退出码 3 + `offscreenRendering` 标识（CI `headless` job 对二进制断言） | ✅ |
| 契约显式 | `contracts/cli.md` 五子命令全部接线并有 golden 测试 | ✅ |

---

## §整改记录：原则 VIII 的一处实质违反（本阶段发现并修复）

### 发现

T052 的兜底复审要求"对全部模块做一轮全量审查"。执行时发现
`tests/no_natural_language.rs` 的 `MESSAGE_BEARING_SOURCES` 表**漏登记**三个
渲染模块（`render/pipeline.rs`、`render/offscreen.rs`、`render/fonts.rs`）。

后果是**静默的**：扫描只覆盖表内文件，故这三个模块的中文错误消息从未被检查
——**表本身文档里警告的"守卫形同虚设"失效形态**（"漏登记的后果：该模块的
错误消息不会被本扫描覆盖，FR-058 在此处失守而无人察觉"）。

### 违反规模

补入三模块后扫描立即报出 **18 处** CJK 字面量：

| 类别 | 处数 | 处置 |
|---|---|---|
| 中文 `#[error(...)]` 串 | 5 | **改英文技术串**（它们是 `Display`，会被上层展示 → 落在 FR-058 管辖内） |
| 中文 `format!` 诊断细节 | 7 | **改英文** |
| `&'static str` 诊断常量 | 3 | **改英文** |
| 适配器描述的诊断后缀 | 1 | **改英文**（一致性） |
| 系统字体族名（`"宋体"` 等） | 1 | 登记白名单（是**数据**，fontdb 据此查找，非文案） |
| 报告字段值（字体覆盖度描述） | 1 | 登记白名单（报告数据，非错误消息） |

### 修复

1. 15 处中文错误串改为英文技术串（文案映射归上层，这与 FR-058 的意图一致）
2. 2 处非消息（字体族名、覆盖度描述）登记 `ALLOWED_NON_MESSAGE_LITERALS` 并写明理由
3. 三模块补入 `MESSAGE_BEARING_SOURCES`（表是白名单式收窄，其正确性依赖维护规则被遵守）
4. 一处断言旧中文串的测试同步更新

### 复查

整改后全量扫描 **0 处**违规；`cargo test -p tome-reader` 738 passed / 0 failed。

### 纪律沉淀

本表的维护规则（"新增能产出错误的模块时必须更新本表"）此前是**文档约定**。
本次违反说明文档约定不足以守住它。建议（**供 PM 裁决**）：把"模块是否产出
错误"的判定做成**编译期或 CI 断言**（如用 `#[error]` 的出现扫描全 `src/`，
与 `MESSAGE_BEARING_SOURCES` 求差集），让漏登记在 CI 上直接红。

---

## 未通过项

**无。** 一处实质违反已在本阶段发现并整改完毕。
