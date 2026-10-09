# S0+S1 引擎内核 实现计划

> **面向 AI 代理的工作者：** 使用 subagent-driven-development（推荐）或 executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。本计划只到切片级；逐任务（TDD 微步骤）由 `/speckit-tasks` 生成 tasks.md 承担。

**目标：** 构建 tome 的引擎内核——一个无界面、三平台、书源兼容的小说阅读引擎库 + CLI 验证器，并先用 S0 spike 验证技术栈可行性。

**架构：** 单体 Rust workspace，10 crate 单向依赖；引擎以 4 个接缝 trait（SourceEngine / ContentProvider / BookRepository / JsRuntime 宿主注入面）暴露能力；规则引擎纯化（产出 RequestSpec，不亲自发请求）；JS 沙箱宿主能力注入式；网络书与本地书共用 ContentProvider seam。一致性靠差分测试（合成语料入仓库 + 真实书源对照不入仓库）。

**技术栈：** Rust 1.98（stable-x86_64-pc-windows-gnu）· rquickjs(QuickJS-ng, ≥0.14.0) · html5ever(自建 DOM) + Servo selectors/cssparser · serde_json_path(+Jayway shim) · regex + fancy-regex · reqwest(+rustls, HTTP/2) · rusqlite(bundled, WAL) · TOML 配置 · 三平台 GitHub Actions

**Branch**: `001-pc-engine-core` | **Date**: 2026-10-09 | **Spec**: [spec.md](./spec.md)

**输入制品**：`research.md`（5 项选型验证）· `data-model.md`（数据模型）· `contracts/seams.md` + `contracts/cli.md`（接缝与 CLI 契约）· `quickstart.md`（上手指引）

---

## Summary

把 Legado（Android，Kotlin ~19 万行）重写为三平台桌面原生应用 `tome`。**本轮交付 S0（技术可行性验证）+ S1（引擎内核）**：书源规则引擎（CSS/JSONPath/XPath/正则/JS）+ 网络抓取管线 + SQLite 持久化 + Pack 章节缓存 + 本地书（TXT/EPUB/MOBI）解析 + 差分一致性验证，经 CLI 验证器独立可测。S2（排版渲染）/S3（界面）/S4（可选模块）不在本轮。

最高优先级是**书源兼容性**（宪法 I）——通过差分测试度量（SC-002）；**干净室纪律**（宪法 II）是合规红线——禁止读上游源码，只黑盒运行产生对照数据。

## Technical Context

**Language/Version**: Rust 1.98.0，stable-x86_64-pc-windows-gnu（Windows 目标必须 gnu 工具链——rquickjs 对 msvc 是 experimental）

**Primary Dependencies**: rquickjs(≥0.14.0, **禁 rust-alloc**) · html5ever + selectors + cssparser · serde_json_path · regex + fancy-regex · reqwest + rustls(+rustls-platform-verifier) · rusqlite(bundled) · serde/serde_json · toml · encoding_rs + chardetng · zip + quick-xml（EPUB）· thiserror · tokio（net 异步面）

**Storage**: SQLite（结构化数据，WAL，actor 线程独占）+ 文件系统（Pack 章节缓存 / 图片缓存 / 归一化本地书）+ TOML 配置

**Testing**: cargo test + **差分测试**（合成语料入仓库 `tests/corpus/`，真实书源对照数据不入仓库）+ 像素/结构断言 + 崩溃安全故障注入（kill -9 测试）

**Target Platform**: Windows 10+ / macOS 12+ / 现代 Linux（三平台一致）

**Project Type**: desktop-app 的引擎内核（library + CLI 验证器，无界面）

**Performance Goals**: 目录 5 万章 <1s（SC-003）；TXT 100MB 导入 <8s（SC-004）；搜索首结果尽快流式；Pack 存储开销 ≤1.01×（SC-005）

**Constraints**: 无遥测零外发（SC-012）；引擎无图形依赖（SC-011）；体积为目标基线非硬门槛（S1 不引无关膨胀，SC-009）；脚本沙箱内存上限+可中断（FR-007）；干净室纪律（不读上游源码）

**Scale/Scope**: 48 FR + 19 SC + 6 用户故事；上游引擎约 8.7 万行非 UI 代码中约 3-3.5 万行为可移植的真实逻辑（规则引擎/网络/存储/JS），重写规模同量级

## Constitution Check

*GATE: 立项前必须通过；Phase 1 设计后复检。*

| 宪章条款 | 门 | 判定 | 依据 |
|---|---|---|---|
| **I 兼容性优先** | 兼容性判定必须可复现度量 | ✅ PASS | 差分测试贯穿（S1-3 起每切片挂语料；S1-6 收口度量 SC-002a/SC-002b）；RequestSpec 纯化使规则引擎离线可重放 |
| **II 干净室纪律** | 禁止读上游源码；只黑盒运行 | ✅ PASS | 研究代理全程受约束；MOBI 仅依据 MIT 文档+MobileRead Wiki 实现，GPL 实现（Calibre/KindleUnpack）只作行为对照 oracle；`.gitignore` 排除真实书源对照数据 |
| **III 测试先行** | TDD；差分测试为落地手段 | ✅ PASS | 每切片 TDD 排序；合成语料覆盖 FR-005b 清单；故障注入测试缓存崩溃安全 |
| **IV 简单性与单向依赖** | crate 边界编译期强制；引擎无图形依赖 | ✅ PASS | 单向依赖 DAG（见下）；上层只依赖 tome-core 接缝；引擎可无界面运行 |
| **V 安全无例外** | 沙箱设限；不可信输入校验；无遥测；密钥脱敏 | ✅ PASS | JS 内存上限+中断（FR-007）；脚本文件访问限数据目录（边缘情况）；Cookie/认证头日志脱敏；无遥测（FR-041） |
| **VI 原子提交** | 一次提交一件逻辑完整的事 | ✅ PASS | 任务级 TDD 提交（tasks.md 强制） |
| **VII 不审查不合并** | 五轴审查 + 干净室轴 | ✅ PASS | 检查点含人工审查；行为对齐变更过干净室轴 |
| **VIII 可读性与显式契约** | 接口先定义契约；注释注明兼容依据 | ✅ PASS | contracts/ 先行；为兼容上游语义的非常规代码须注明依据来源 |
| **附加·体积纪律** | 体积目标基线，CI 监控 | ✅ PASS | S0 实测基线；release LTO=fat+panic=abort+strip |
| **附加·平台承诺** | 三平台一致；不用系统 WebView 做界面 | ✅ PASS | 三平台 CI；S1 无界面 |
| **附加·范围纪律** | 不顺手做其他子项目 | ✅ PASS | 明确排除段遵守；S2/S3/S4 不在本轮 |

**门结论：无违规，无需 Complexity Tracking 豁免。** 无未解决澄清（spec 经 clarify 完成）。

## Project Structure

### Documentation (this feature)

```text
specs/001-pc-engine-core/
├── plan.md              # 本文件
├── spec.md              # 需求（48 FR / 19 SC / 6 用户故事 / 6 澄清）
├── research.md          # Phase 0：5 项选型验证
├── data-model.md        # Phase 1：数据模型
├── quickstart.md        # Phase 1：上手指引
├── contracts/           # Phase 1：接缝契约 + CLI 契约
│   ├── seams.md
│   └── cli.md
├── checklists/          # 规格质量校验
└── tasks.md             # /speckit-tasks 产出（本计划不生成）
```

### Source Code (repository root)

```text
tome/
├── Cargo.toml                 # workspace：LTO=fat / panic=abort / strip / opt-level
├── rust-toolchain.toml        # 钉 stable-gnu；CI 钉三平台
├── .github/workflows/         # 三平台 CI（构建+测试+体积基线+feature 守卫）
├── crates/
│   ├── tome-core/             # 接缝 trait + DTO + ErrorKind（窄；无工具函数；零图形依赖）
│   ├── tome-net/              # HTTP/Cookie/代理/限速/字符集 + RequestSpec + 回放 adapter
│   ├── tome-store/            # SQLite actor + 迁移器 + DAO + Pack 容器 + 图片缓存 + 配置注入
│   ├── tome-js/               # QuickJS 沙箱 + 宿主注入接线（经 tome-core 的 NetHost/StoreHost trait）+ compat prelude
│   ├── tome-rule/             # 规则 DSL：分派/组合符切分/CSS/JSONPath(+shim)/XPath(自研)/正则(双引擎)/替换链
│   ├── tome-source/           # 书源管线：搜索/发现/详情/目录/正文/净化/换源/源检查
│   ├── tome-local/            # TXT/EPUB/MOBI(分阶段) 解析 + 库根扫描 + TOC 识别
│   └── tome-cli/              # CLI 验证器（search/info/toc/content/run/verify/local/data）+ 差分测试入口
└── tests/
    ├── corpus/                # 合成差分语料（入仓库；覆盖 FR-005b 能力清单 + 各语法点）
    └── fixtures/              # 测试夹具（乱网页 HTML / JSON / EPUB / TXT / MOBI 样本）
```

**Structure Decision**: 单一 Rust workspace，按 **crate = module** 拆分。依赖方向单向（见依赖图）。`tome-app`（S3 界面）/ `tome-reader`（S2 排版）/ `tome-extra`（S4）**本轮不建**，但依赖图预留其位置（它们将只依赖 tome-core）。

## Dependency Graph（依赖图）

自下而上构建——先建基础。箭头 `A → B` 表示 **A 依赖 B**（A 用到 B 的类型/trait）：

```
                        ┌──────────────────────────────────────────┐
                        │ tome-core（接缝 trait + DTO + ErrorKind）│  ← 无依赖，被所有人依赖
                        └──────────────────────────────────────────┘
                              ▲              ▲              ▲
        ┌─────────────────────┘              │              └─────────────────┐
        │                                    │                                │
  ┌───────────┐                        ┌───────────┐                    ┌───────────┐
  │ tome-net  │                        │ tome-store│                    │  tome-js  │
  │ HTTP/     │                        │ SQLite/   │                    │  QuickJS  │
  │ RequestSpec                       │ Pack 容器 │                    │  +宿主注入 │
  └───────────┘                        └───────────┘                    └───────────┘
        ▲                                    ▲                                ▲
        │  实现 tome-core 定义的              │  实现 tome-core 定义的           │  被组合根注入
        │  NetHost（seam，见 tome-core）      │  StoreHost（seam，见 tome-core） │  NetHost/StoreHost
        └────────────────────────────────────┴────────────────────────────────┘
                                             │
                                       ┌───────────┐
                                       │ tome-rule │  ← 依赖 core(RequestSpec) / js(eval)
                                       │ 规则 DSL  │
                                       └───────────┘
                                             ▲
                                       ┌───────────┐         ┌────────────┐
                                       │tome-source│         │ tome-local │  ← 依赖 store(+rule 用于 TOC)
                                       │ 书源管线  │         │ 本地书解析  │
                                       └───────────┘         └────────────┘
                                             ▲                      ▲
                                             └──────────┬───────────┘
                                                  ┌───────────┐
                                                  │  tome-cli │  ← 依赖全部（组合根 + 差分测试入口）
                                                  └───────────┘
```

**关键接缝决策（来自架构审查，已并入设计）**：
- **A1**：宿主能力 trait（NetHost/StoreHost/CryptoHost）与共享值对象 RequestSpec/Response **定义在 tome-core**（接缝中枢），由组合根注入实现——tome-js 不依赖 tome-net/tome-store，tome-net/tome-store 也不依赖 tome-js（一切实现 crate 只依赖 tome-core，星型无环）
- **A2**：tome-rule 求值产出 `RequestSpec` 不亲自发请求；管线（tome-source）是唯一发请求处 → 规则引擎离线可重放
- **A3**：配置注入——组合根读 config.toml 一次，类型化子集注入各模块；禁止全局 CONFIG 单例
- **A4**：ContentProvider 一个 seam 两个 adapter（网络书缓存或抓取 / 本地书偏移直读），净化管线下游统一
- **A6**：`search` 返回**流 + 取消令牌**，单源失败是流内条目

**关键风险与未知项**：
- **MOBI/AZW3**（R4）：S1 最大单项工作量（~20-35 人日）。HUFF/CDIC 与 KF8 路径必须用真实文件验证。**风险最高，但阶段三（KF8）可拆为后续任务。**
- **Jsoup↔自建DOM 差异率**（R2）：文档推断非实测，需 spike 量化。
- **rquickjs 体积**（R1）：官方口径 210KiB，需 S0 实测。
- **正则长文挂起**（R5）：fancy-regex 对 20 万字正文可挂起 >120s——必须低 backtrack_limit + 输入上限 + 特性预扫描路由。

## Implementation Strategy

### 垂直切片划分

每个切片交付一条**端到端可经 CLI 独立验证**的能力路径（schema + 引擎 + CLI 验证），不是水平分层。切片排序遵循依赖图；高风险项以 **S0 spike 前置**与**可拆阶段**（MOBI 阶段三、KF8 可后置）化解——依赖约束与风险顺序本就冲突，此为本计划的取舍。

| 切片 | 端到端能力路径 | 验证方式（CLI） | 规模 |
|---|---|---|---|
| **S0** | 最小骨架：窗口+DB 读写+一次 HTTP+一次 JS 求值，三平台构建+体积基线 | spike 产物三平台跑通，记录体积/启动耗时 | **M** |
| **S1-1 地基+最小搜索闭环** | tome-core 接缝 + tome-net（HTTP/Cookie/字符集/RequestSpec/回放）+ tome-rule 最小（CSS+正则+替换链）+ tome-source 搜索 + tome-cli `import`/`search` | 导入一个真实书源 → 搜索出结果 → `--replay` 离线重放一致 | **XL**→拆 |
| **S1-2 JS 沙箱** | tome-js（QuickJS+内存上限+中断+宿主注入+compat prelude+宿主 API 分期）+ tome-rule JS 集成（`@js:`/`{{}}`/`mainJs`） | 含 JS 的书源跑通；死循环脚本被终止（FR-007）；WebView 能力抛结构化错误（FR-006） | **XL**→拆 |
| **S1-3 规则 DSL 全量** | JSONPath（serde_json_path+Jayway shim）+ XPath（自研，共享 DOM）+ 组合符全量 + Jsoup 专有伪类/索引切片 | 合成语料差分通过率 ≥95%（SC-002a）；规则求值轨迹（FR-037）可输出 | **XL**→拆 |
| **S1-4 存储+完整管线** | tome-store（SQLite actor+迁移+DAO+Pack 容器+图片缓存+Cookie 持久化+配置注入）+ tome-source 全管线（详情/目录/正文/净化/换源三级映射/源检查/发现） | 完整阅读路径；崩溃安全故障注入（FR-030）；Pack 容量开销（SC-005）；锚点不变性（SC-008） | **XL**→拆 |
| **S1-5 本地书** | tome-local：TXT（编码检测+TOC 规则回退）+ EPUB + MOBI 分阶段（PalmDOC→HUFF/CDIC→KF8 FDST）+ 库根扫描 | 导入三类格式样本读出目录与正文；移动库目录后索引重建不重复（FR-025） | **XL**→拆 |
| **S1-6 一致性验证+收口** | 差分 harness 接线（上游参考运行器+回放）+ 覆盖率矩阵（FR-005b）+ 全部阅读数据导入导出（FR-033）+ 错误本地化（ErrorKind→中英）+ 阅读记录落库（FR-026b）+ 书内全文搜索（FR-019b，FTS5）+ 三平台 CI 产物+体积基线 | SC-002b 真实书源回归基线；SC-010 三平台产物；S1 完成 | **L**→拆 |

### XS-XL 工作量预估与拆分方案

L 及以上切片在 `/speckit-tasks` 阶段拆为 XS–M 任务（单文件/单组件/单功能切片，TDD 微步骤，2-5 分钟/步）。各 XL 切片的拆分线索：

- **S1-1**：core 接缝 → net（blocking 客户端→cookie→charset→代理→限速→回放 adapter）→ rule-css（DOM→selectors→后缀/切片）→ rule-regex（双引擎+翻译层）→ source.search → cli。每步一个独立 commit。
- **S1-2**：runtime 包装（上限+中断）→ 宿主 trait 定义 → compat prelude → 宿主 API 分期（网络类→文本/编码→摘要/加密→结构化数据→正则→时间→杂项→宿主状态，按 FR-005 八类逐组 TDD）→ 能力错误 → mainJs 源。
- **S1-3**：JSONPath shim（重写器→结果适配→函数注册）→ XPath 自研（lexer→parser→求值器，子集）→ 组合符（引号/括号感知切分）→ Jsoup 伪类逐项 → 差分语料挂载。
- **S1-4**：迁移器 → DAO 逐实体 → Pack 容器（追加→索引→淘汰→崩溃安全）→ 图片缓存 → Cookie → 管线逐动作（info→toc→content→purify→change-source→check-source→discover）。
- **S1-5**：TXT → EPUB → MOBI 阶段一（PalmDOC）→ 阶段二（HUFF/CDIC）→ 阶段三（KF8 FDST）。**MOBI 阶段三可拆为独立后续任务（不阻塞 S1 主线）。**
- **S1-6**：上游参考运行器接线 → 语料 harness → 导入导出 → 本地化 → 阅读记录 → 书内搜索 → CI。

### 检查点规划

每 2-3 个任务一个验证检查点；切片边界为硬检查点。高风险项的化解方式：S0 可行性门前置；S1-2 沙箱、S1-3 规则保真各自独占切片并配专属检查点；MOBI（风险最高但依赖靠后）以"阶段三可拆为后续任务"降级其阻塞性。

```
## 检查点：S0 之后（可行性门）
- [ ] 三平台均产出可运行骨架产物
- [ ] 体积与启动耗时基线已记录
- [ ] 无非用户触发的网络外发
- [ ] rquickjs 在三平台编译通过（gnu/macos/linux）
- [ ] 人工审查技术栈实测数据后再继续

## 检查点：S1-1 之后（最小闭环）
- [ ] 导入真实书源 → 搜索出结果（`--replay` 离线重放一致）
  - 注意：此处的 replay 一致性是**自洽性验证**（同输入同输出），**不等于**规则语义已对齐上游。兼容性验证（与参考实现的差分比对）在 S1-3（合成语料）与 S1-6（真实书源基线）才发生
- [ ] Android 备份样本可导入（FR-034）
- [ ] 全部测试通过；无全局 CONFIG；RequestSpec 纯化成立
- [ ] 配置注入成立；CLI 输出结构化 JSON

## 检查点：S1-2 之后（JS 沙箱）
- [ ] 含 JS 的书源跑通；死循环被中断；OOM runtime 被丢弃
- [ ] 宿主 API 分期覆盖 FR-005 清单；能力错误结构化
- [ ] rquickjs 四缓解措施在 CI 有守卫（版本/rust-alloc/gnu/栈上限）

## 检查点：S1-3 之后（规则保真——兼容性门）
- [ ] 合成语料差分通过率 ≥95%（SC-002a）
- [ ] CSS/JSONPath/XPath/正则/JS 各语法点均有用例
- [ ] Jsoup 版本对齐目标已钉死；非标准伪类清单已实现

## 检查点：S1-4 之后（存储与管线）
- [ ] 完整阅读路径端到端；崩溃安全故障注入通过
- [ ] Pack 容量开销 ≤1.01×；两种缓存可独立清理
- [ ] 锚点不变性参数化验证通过；Cookie 持久化重启保留

## 检查点：S1-5 之后（本地书）
- [ ] TXT/EPUB/MOBI(阶段一/二) 导入可读；MOBI 阶段三范围已确认
- [ ] 库根扫描；移动目录索引重建不重复
- [ ] MOBI 用真实文件验证（非仅单元测试）

## 检查点：S1-6 之后（S1 完成）
- [ ] SC-002b 真实书源回归基线已记录
- [ ] 三平台 CI 产物；体积基线在监控
- [ ] spec 全部 SC 可度量项通过；宪章合规复检通过
- [ ] 人工审查 + 干净室轴审查通过
- [ ] 上游参考运行器接线的干净室约定已成文：运行器怎么跑、产物落盘位置、确认未读源码的核对人——见下
```

> **干净室执行边界（tasks.md 生成时的硬性任务）**：S1-6 涉及唯一一处黑盒运行上游参考实现（`D:/Code/legado`，只读）。`/speckit-tasks` 必须为此生成一条明确任务，约束三点：① 运行器仅通过命令行/API 调用上游**可执行产物**，全程不打开上游 `.kt` 源码文件；② 对照输出落盘到 `.gitignore` 排除的 `testdata/captures/`（不入仓库）；③ 指定一名核对人确认该任务的实现过程未触碰上游源码。这是宪法原则 II 的执行层落地，不是可选建议。

### 研究驱动的设计修订（须用户知晓）

`research.md` 验证后，对设计共识有两处**实质性修订**（基于证据的更优解，非降级）：

1. **XPath**：设计共识原为 `sxd-document + sxd-xpath` → **改为自研 XPath 1.0 子集，跑在共享 DOM 上**。理由：sxd-xpath DOM 类型写死需 HTML→XML 转换（两棵树）导致 CSS/XPath 结果系统性分叉；libxml2 已停止维护且 HTML4 时代。
2. **HTML DOM**：设计共识原为 `html5ever`（rcdom）→ **改为自建 arena DOM（实现 TreeSink），禁用 markup5ever_rcdom**（官方明说不受支持、未 fuzz、非生产级）。

另有四条 **rquickjs 硬缓解措施**（锁版本/禁 rust-alloc/Windows 走 gnu/显式栈上限）与**正则双引擎 + 语法规范化层**（regex 静默把 `a++` 当嵌套重复而非占有量词——必须显式转 `(?>a+)`）已纳入设计。

### 明确不做（本轮范围护栏，来自 spec 排除段）

WebView 类书源能力（结构化错误降级）· PDF/UMD/压缩包扫描 · 竖排 · 界面/排版/渲染（S2/S3）· RSS/朗读/词典界面/定时调度（S4）· 音频/漫画/视频（S4 默认关）· HTTP/WS/MCP 服务 · 应用自升级/跨设备同步/WebDAV/LAN · 遥测/崩溃上报 · 书源变更追踪（G7，YAGNI）

## Phase 0 & 1 制品索引

- **research.md** — 5 项高风险选型验证（QuickJS / HTML+CSS+XPath / JSONPath / MOBI / 正则），全部含 Decision/Rationale/Alternatives 与来源
- **data-model.md** — 18 实体 schema（SQLite v1 + Pack 容器 + 配置 TOML）
- **contracts/seams.md** — 4 个接缝契约（SourceEngine / ContentProvider / BookRepository / JsRuntime）+ RequestSpec/Response 值对象
- **contracts/cli.md** — CLI 验证器命令契约（含 verify/replay/record 差分测试接线）
- **quickstart.md** — 零上下文上手指引（工具链 / 护栏 / 目录 / 顺序 / TDD）

## 下一步

→ `/speckit-tasks`（按本计划 Implementation Strategy 的切片划分 / XS-XL 拆分线索 / 检查点位置生成 tasks.md，逐任务 TDD 微步骤）
