# 合成差分语料

`tome verify --corpus tests/corpus/` 的自检数据集。它是 **SC-002a（合成语料
差分通过率 ≥95%）的载体**，也是「兼容性门」的判据来源。

## 为什么是"合成"的

真实书源的对照数据是**第三方资产**，不入仓库（`AGENTS.md` 的 gitignore 约定、
宪法原则 III）。故入仓库的是**合成用例**：输入→期望输出，全部可复现、
可审查、可追溯到公开依据。

## 文件格式

每个 `.json` 文件是一个用例数组：

```json
[
  {
    "name": "用例名（唯一，会出现在 diff 报告里）",
    "kind": "html | json",
    "input": "输入文本（HTML 片段或 JSON 文本）",
    "rule": "书源规则原文（可含前缀、后缀、切片、替换链）",
    "expected": ["期望值 1", "期望值 2"],
    "basis": "期望值的依据（必填，见下）"
  }
]
```

### 字段说明

| 字段 | 必填 | 说明 |
|---|---|---|
| `name` | 是 | 唯一标识。冒号前缀是语法类别（`css:` / `jsonpath:` / `xpath:` / `regex:` / `js:` / `pipeline:` / `text:` / `crypto:` / `data:` / `time:` / `misc:` / `state:`） |
| `kind` | 是 | `html` 走 HTML 解析；`json` 把输入当 JSON 文本喂给 `@json:` |
| `input` | 是 | 输入原文 |
| `rule` | 是 | 书源规则原文 |
| `expected` | 是 | 期望的**字符串列表**（顺序敏感） |
| `basis` | 是 | **期望值的依据**（见下） |
| `reversed` | 否 | 是否反序（等价于规则前加 `-`），默认 false |

## `basis` 是硬性要求

tasks.md T052 的原文：

> **期望值依据成文**：以 research.md R5/R3 的语义差异表为依据，逐条注明
> 来源（crate 版本 / Java 行为文档 / 书源规则原文），不靠实现者直觉填期望值。

故 `basis` 必填，且须是**可核对的来源**，例如：

- `Jsoup Selector 公开文档（jsoup.org/apidocs）`
- `W3C XPath 1.0 §3.3`
- `RFC 9535 §2.3.5`
- `RFC 4648 §4`（Base64/Base16 编码表）
- `RFC 3986 §2.1`（URI 百分号编码）
- `RFC 8259`（JSON 语法）
- `RFC 3339 §5.7`（时间戳与 Unix 纪元）
- `RFC 1952 §2.3`（gzip 成员结构）
- `FIPS 180-4` / `FIPS 197`（SHA 系列 / AES）
- `WHATWG HTML §13.5`（命名实体表）
- `ECMAScript 语言规范（String.prototype.replace / RegExp）`
- `Jayway JsonPath README（Aggregate Functions）`
- `书源规则公开说明（社区流通的《书源规则说明》）`
- `research.md R5 的正则语义差异表`
- `research.md R3 的 JSONPath 差异清单`
- `research.md R2 的 Jsoup 方言清单`
- `api/util.rs` / `api/text.rs` / `api/state.rs` / `api/data.rs`（**本实现自身**的约定与集成测试；当某能力的语义是"我们自己定的"而非来自外部规范时，依据就是它——这仍然可核对，因为读得到）

**不允许**写"直觉"、"看起来应该是"、"试出来的"。若某条期望值找不到公开
依据，它就不该进语料——那是"不知道该期望什么"，而不是"知道但懒得写"。

> `corpus_gate.rs` 的「每条用例都有依据来源」测试会检查 `basis` 是否提到
> 上述来源之一。新增依据类别时须同步扩它的 `SOURCE_MARKERS` 清单——
> 那道检查的存在正是为了挡住"随手填个期望值"。

## 通过率口径（SC-002a）

一条用例**通过**当且仅当引擎输出的列表与 `expected` **逐元素相等**
（顺序敏感）。通过率 = 通过条数 / 总条数，要求 **≥95%**。

顺序敏感是有意的：书源规则里结果顺序决定"取第几个"，顺序错了就是兼容性
缺陷，即便集合相同。
