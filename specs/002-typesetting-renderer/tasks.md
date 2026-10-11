# Tasks: S2 排版渲染层

**Input**: Design documents from `/specs/002-typesetting-renderer/`

**Prerequisites**: plan.md ✓, spec.md ✓, research.md ✓, data-model.md ✓, contracts/cli.md ✓, quickstart.md ✓

**对齐声明**: 本文件任务粒度与排序**严格对齐** plan.md 的 Implementation Strategy 节（切片划分 S2-0~S2-6、检查点位置、XS-XL 拆分线索），无偏离。审查员 4 条建议（滚动 FR 归属/错误类型先行/S2-1·S2-6 检查点补强/quickstart 修正）已体现在任务与检查点中。

**Tests**: 本项目宪法原则 III（测试先行，NON-NEGOTIABLE）强制 TDD。**每个实现任务的验收标准都内置"先写失败测试"**。像素快照是排版层的特化测试形式（AGENTS.md）。

**组织方式**: 按 plan.md 的**垂直切片**分 Phase（每片 = 一个可经 CLI/测试独立验证的端到端能力路径）。任务用 `[USx]` 标注其所服务的用户故事以便追溯。切片即切片，`[USx]` 是追溯标签。

**用户故事映射**（spec.md §User Scenarios）：
- US1(P1) 翻页阅读中文小说 · US2(P2) 调参不丢阅读位置 · US3(P3) 渲染管线稳定帧 · US4(P4) 痕迹与词典呈现 · US5(P5) 无图形环境可验证 · US6(P6) 图文混排 · US7(P7) 滚动模式

**TDD 微步骤（每个实现任务遵守）**：写失败测试 → 运行确认失败（RED）→ 最少实现 → 运行确认通过（GREEN）→ commit。每个任务以独立 commit 结束。

**环境约定**: hinting 默认关闭（research R1/R4 已定：挤压语义与三平台一致性前提）；快照后端不可用时诚实报告绝不伪装通过；`release` profile 下 `panic=abort`，不得依赖 catch_unwind。

---

## Phase 1: Setup（tome-reader 骨架与横切地基）

**Purpose**: crate 骨架、错误契约、语料脚手架。**无用户故事标签。**

- [x] T001 创建 `crates/tome-reader/` crate（Cargo.toml + src/lib.rs）——依赖仅 tome-core + cosmic-text 0.19 + unicode-segmentation；wgpu/swash/etagere 依赖挂 `render` feature 门控（默认关，headless 构建不含图形栈）
  - [x] `cargo build -p tome-reader`（feature off）与 `--features render` 两种构建均通过
  - [x] feature off 时依赖树不含 wgpu/winit 等图形包（`cargo tree` 断言）
- [x] T002 [P] tome-core 增补 `Theme` 纯值类型于 `crates/tome-core/src/model/theme.rs`（裁决 OQ#11→D）——承载 design-system/MASTER.md §2.1/2.2 全部语义色、§3 字体三角色、§4 间距/圆角/阴影/动效常量；零图形依赖
  - [x] 字段与 MASTER.md token 表逐一对应（无遗漏断言）；昼/夜两实例构造单测
  - [x] `cargo test -p tome-core` 通过；`cargo tree -p tome-core` 仍零图形依赖
- [x] T003 [P] tome-reader 错误类型 + →ErrorKind 映射表于 `crates/tome-reader/src/error.rs`（FR-057/058）——含 `UnsupportedCapability` 能力标识命名约定（`offscreenRendering` 等）、`panic=abort` 下错误路径设计注释（不得以 catch_unwind 为处理手段）
  - [x] 全部变体映射 ErrorKind 的测试；**无面向用户自然语言硬编码**（grep 断言形态）
- [x] T004 [P] 排版语料脚手架于 `crates/tome-reader/tests/corpus/`——JSON 条目 = 输入正文 + 几何参数 + 期望（分页结构/规则命中/禁则例外），与 001 `tests/corpus/` 同风格；`basis` 依据字段强制（类型系统拒绝缺依据条目）
  - [x] 加载器单测；≥3 条示例条目（禁则/挤压/混排各一）

---

## Phase 2: S2-0 可行性 spike ⚠️ 阻塞所有用户故事

**Purpose**: 用最小骨架验证 R5 全部 7 项技术风险（die 早 die 便宜）；产物 `spikes/s2/` 为一次性验证代码，**不进主 workspace**。

- [x] T005 R5-1：ShapeWord 合并行为验证于 `spikes/s2/src/word_merge.rs`——`Buffer::line_shape` 取 ShapeLine 克隆体 → 合并相邻 ShapeWord → `layout()`，断言断点遵守合并边界且 decorations 不破坏（research R1）
  - [x] 合并后 layout 断点只在 word 边界（多组参数断言）
  - [x] 结论写入 `spikes/s2/BASELINE.md`；**若证伪：停止并上报 PM**（备选方案 C 自写 layout 层，须用户批准才执行）
- [x] T006 [P] R5-2：hinting 对 x_advance 挤压的量化影响于 `spikes/s2/src/hinting.rs`——同参数开/关 hinting 各排一版对比；R5-7：贪心换行+挤压的排版质量评测（次优断点出现率口径）
  - [x] 两组数据写入 BASELINE.md；默认关 hinting 的决策被数据确认（或上报 PM）
- [x] T007 [P] R5-3：cosmic-text 全栈体积对照（空壳 vs 接入，workspace 同款 release profile）；R5-4：Windows-gnu + wgpu 构建与离屏初始化验证
  - [x] 体积净增量实测值 + 构建/初始化结果写入 BASELINE.md
- [x] T008 [P] R5-5：字体子集实产于 `assets/fonts/`——skera（备选 fonttools）子集化思源宋体 CN 区域 OTF（6500 字档=通用规范一级 3500+二级+常用标点+基本西文）→ **改家族名去 "Source"**（OFL RFN 合规，research R3）→ `TomeSerifCN-subset.otf` + `OFL.txt` + `FONTS.sha256` 入 git
  - [x] 产物 ≤5MB；name 表不含 "Source" 家族名（校验测试）；sha256 清单在案
- [x] T009 [P] R5-6：离屏渲染探针 + 单基线容差初判于 `spikes/s2/src/offscreen_probe.rs`——离屏渲染一页样本，ubuntu/lavapipe 生成基线，本机以 YIQ 0.6 比对
  - [x] 探针可跑并打印后端名；容差校准结论（单基线可行性）写入 BASELINE.md

> **⛔ 检查点：S2-0 之后（可行性门）**
> - [x] R5 七项全部有实测数据（BASELINE.md）
> - [x] ShapeWord 合并行为证实（否则已上报 PM 并获批备选路径）
> - [x] 字体子集 ≤5MB 且合规入仓
> - [x] **人工审查 spike 数据后再继续**
>   - PM 注记（2026-10-10）：S2-0 门审查通过——R5 七项无一项证伪：ShapeWord 合并证实（方案 A）、hinting 量化机制修正（在 layout 层，默认关 hinting 决策获数据支撑）、全栈净增 5.33MiB、Windows-gnu+wgpu 离屏证实、字体子集 2.18MiB RFN 合规（FONTS.sha256 经 PM 干净上下文独立复核 MATCH）、单基线容差本机逐比特相同、贪心换行纯中文 0% 次优。三条装置缺陷（假绿）已固化进 BASELINE §10.1。

---

## Phase 3: S2-1 排版核心（US1）🎯 MVP

**Goal**: 文本输入 → 段落切分 → 中文规则（禁则/挤压/混排/硬序/两端对齐）→ 行盒序列，全程 headless。

**Independent Test**: `tome layout --chapter <样本>#0` 输出结构 JSON：标点密集语料禁则违反 0；混排无超宽；样式组合下标不变。

- [x] T010 [US1] 规范化文本模型于 `crates/tome-reader/src/text/model.rs`（FR-003：段落→字形簇→字形；对外一律字符下标，与 001 `char_offset` 同口径）
  - [x] grapheme cluster 不在簇内断开（emoji/组合字符/变体选择符用例）
  - [x] 公开 API 无字节偏移/UTF-16 码元偏移暴露（签名审查测试）
- [x] T011 [US1] 段落切分于 `crates/tome-reader/src/text/paragraph.rs`（FR-001 换行符边界；FR-002 全角空格启发式：双条件门槛 + 诊断记录 + 命名常量阈值）
  - [x] 启发式启用/不启用两路径正反测试；不启用时零副作用断言
  - [x] 前导空格在默认路径下按段内文本处理（不误切空段）
- [x] T012 [US1] 禁则字符清单于 `crates/tome-reader/src/rules/cjk.rs`（FR-005/007：行首/行尾禁则表 = UAX#14 CL/OP 类 + 破折号对（U+2014 B2）/省略号等补丁条目；清单可枚举逐项测试）
  - [x] SC-001：每条规则附判定例与反例，100% 通过
  - [x] 清单条数与内容成文（模块文档引用 UAX#14 与 CLreq 公开资料为依据）
- [x] T013 [US1] ShapeLine 克隆后处理引擎于 `crates/tome-reader/src/layout/engine.rs` + `crates/tome-reader/src/layout/linebox.rs`（research R1 方案 A：line_shape→克隆→按禁则表合并 ShapeWord→layout；使用 T005 已验证的行为；LineBox 结构定义在 linebox.rs，对齐 data-model.md §4 七字段）
  - [x] 端到端：行首无闭标点、行尾无开标点（SC-002 标点密集语料违反 0）
  - [x] 例外走硬序第④步时有 `RuleExceptionRecord` 产出（char_index + rule_id）
- [x] T014 [P] [US1] 标点挤压于 `crates/tome-reader/src/rules/squeeze.rs`（FR-006：x_advance 改写 + 压缩下限命名常量 + 不改字符顺序与区间归属）+ 中西文混排于 `crates/tome-reader/src/rules/mixed.rs`（FR-009：交界处最小间隙；西文整词优先、超行长词退化按字符断行）
  - [x] US1 场景 1：行长在版心内浮动 ≤1 挤压单位且无超宽（SC-003）
  - [x] 挤压前后字符下标一致（区间归属不变断言）
- [x] T015 [US1] 两端对齐两段式 + 硬序消解于 `crates/tome-reader/src/rules/justify.rs`（FR-011 正文 justify 默认、居中用于标题/引用；FR-008 硬序 ①挤压→②悬挂（≤半字身）→③整字回退→④放弃禁则，固定可预测）
  - [x] 硬序四步各有触发用例；第④步例外记录可枚举可计数（供 SC-002）
  - [x] justify 与挤压协同：不出现标点被拉散的稀疏行（视觉断言语料）
- [x] T016 [US1] 行内样式最小集合于 `crates/tome-reader/src/layout/style.rs`（FR-010：标题/强调/引用/居中；样式只影响外观与行盒属性；标题字号随阅读字号等比缩放，禁硬编码 px）
  - [x] FR-010 参数化断言：同段文本任意样式组合下字符下标一致
  - [x] 标题字号随 14/19/24/30 等比（四档断言）

> **⛔ 检查点：S2-1 之后（排版正确性门）**
> - [x] SC-001 规则清单逐项判定例反例 100%；SC-002 禁则违反 0；SC-003 混排无超宽
> - [x] FR-010 样式组合下标不变（参数化断言）+ 标题字号等比
> - [x] 四命令全绿；`tome layout` 出结构 JSON
> - [x] 人工审查后进入下一阶段
>   - PM 注记（2026-10-10）：排版正确性门通过（SC-001/002/003 + FR-010 双断言 + `tome layout` 出契约 JSON）。遗留 KNOWN_DEFECT_COUNT=1（混排行未吸纳可放的下一单元）以只减不增棘轮钉住，修复方向归 S2-2 或 PM 裁决；`--chapter` 入口待 S3/001 书架接线（现以 `--text` 为载体 + 结构化能力错误）。

---

## Phase 4: S2-2 分页与参数集（US1/US2）

**Goal**: ReadingConfig → 页序列（字符区间）→ 单双页判据/手动覆盖 → 书页语义 → 锚点解耦 + 预计算/增量重排。

**Independent Test**: `tome layout` 页区间拼接逐字符等于原文（SC-006）；锚点矩阵全组合不变（SC-004）；昼/夜下页序列逐字节相同（SC-005）。

- [x] T017 [US2] ReadingConfig schema 于 `crates/tome-reader/src/paginate/config.rs`（FR-013/028：几何/呈现子集分离；FR-018 段模式互斥默认缩进 2em；白名单校验拒绝非法值并回退上一有效值）
  - [x] 全字段范围校验测试（字号 0/行高负/版心负宽拒绝）
  - [x] 序列化往返（作为 `Book.read_config` 的 schema）
- [x] T018 [US2] `Book.read_config` 接线（tome-store 读写给 S2 schema；全局默认 vs 单书覆盖语义，裁决 OQ#12）于 `crates/tome-store/src/dao/` 既有 book DAO 的扩展 + `crates/tome-reader/src/paginate/config.rs` 的加载入口
  - [x] 未设置时回退全局默认；显式"为这本书设置"后覆盖生效（经 BookRepository 测试）
- [x] T019 [US1] 页序列计算于 `crates/tome-reader/src/paginate/page.rs` + `spread.rs`（FR-012 页=字符区间；FR-014 em 制判据：阅读区可用宽度 ≥ 2×measure+gutter，禁固定 px 断点，字号变化重算判据；FR-004 唯一内容源为 ContentProvider）
  - [x] 内容页无损：页区间拼接与该章正文逐字符相等（`crates/tome-reader/tests/pagination_lossless.rs`；SC-006 的留白页口径在 T021 的测试中补齐）
  - [x] 判据随字号重算测试（19px→24px 双页阈值右移）
  - [x] 排版全程不自读缓存容器、不发请求（计数宿主/回放断言，FR-004）
- [x] T020 [US1] 手动覆盖三态 + 语义落于 `crates/tome-reader/src/paginate/spread.rs`（FR-015：自动/强制单页/强制双页；矛盾时回落自动，非矛盾变化不清除用户选择）
  - [x] 回落路径与保留路径各有测试（`crates/tome-reader/tests/spread_override.rs`，宽度变化两类）
- [x] T021 [US1] 书页语义于 `crates/tome-reader/src/paginate/booksem.rs`（FR-016：左偶右奇；章起偶数页留白补足；命名设置项「章节从新页开始」可关；单页模式不应用）
  - [x] 双页章起位置测试（`crates/tome-reader/tests/book_semantics.rs`：上章末单页时留白补足，留白页 is_padding=true 显式标记且不计入内容区间——SC-006 完整口径在此补齐）
  - [x] 开关关闭时无留白；单页模式断言无留白（裁决 OQ#5）
- [x] T022 [US2] 锚点解耦 + 参数矩阵验证于 `crates/tome-reader/tests/anchor_matrix.rs`（FR-017/SC-004：同锚点任意两组几何参数分别定位同字符且锚点值不变；矩阵 = 字号 14/19/24/30 × 行高 1.75/1.85/2.2 × 单/双页 × 翻页/滚动 × 段缩进/段间距）
  - [x] 全组合 100% 锚点值不变且字符落在可见范围
- [x] T023 [US2] 呈现参数零依赖断言（FR-019/SC-005：昼/夜主题、不同字体族、不同动效开关下页序列（字符区间）100% 相同）
  - [x] `crates/tome-reader/tests/theme_independence.rs` 通过（主题切换不触发重排的断言）
- [x] T024 [US2] 分页预计算 + 增量重排于 `crates/tome-reader/src/precalc.rs`（FR-044：当前位置附近章节后台预计算、可取消、失败无副作用、不持久化；FR-045：参数变化只重排受影响页即可呈现首帧）
  - [x] 取消后无部分状态泄漏（`crates/tome-reader/tests/precalc.rs`）
  - [x] 增量重排首帧时间 < 整章重排（同文件的测量测试，口径写注释）

> **⛔ 检查点：S2-2 之后（锚点不变量门）**
> - [x] SC-004 锚点矩阵 100%；SC-005 呈现参数零依赖 100%；SC-006 分页不丢不重 100%
> - [x] 人工审查后进入下一阶段
>   - PM 注记（2026-10-11）：锚点不变量门通过（SC-004 480 断言/SC-005 64 组逐字节相同/SC-006 库内+CLI 1656 组逐字符相等）。KNOWN_DEFECT 1→0——诊断为第四次测量装置缺陷（漏算空格宽），引擎断行本身正确，变异验证在案（BASELINE §11.4）。`AnchorTarget::Scroll` 占位 [WIP] 待 Phase 5 补实。

---

## Phase 5: S2-3 滚动模式（US7，FR-020~FR-027）

**Goal**: 同一排版结果 → 滚动流 → 视口区间 → 双向映射 → 翻页⇄滚动切换保锚点 → 按需释放/节流/阅读记录去重。

**Independent Test**: `tome layout --mode scroll` 视口区间并集逐字符等于原文（SC-006b）；两方向切换锚点不变（SC-005b）；滚动到未缓存区零抓取（FR-025）。

- [x] T025 [US7] 滚动流排布于 `crates/tome-reader/src/scroll/flow.rs`（FR-020 连续流无分页空白；FR-021 视口=字符区间与 Page 同构，不引入第二套坐标）
  - [x] SC-006b：`crates/tome-reader/tests/scroll_flow_integrity.rs`——视口区间并集与正文逐字符相等；相邻视口连续无缺口无重叠
- [x] T026 [US7] 滚动偏移↔字符位置双向映射于 `crates/tome-reader/src/scroll/map.rs`（FR-022/040：与 001 锚点口径一致）
  - [x] 互逆往返误差 ≤ 半个字形（`crates/tome-reader/tests/scroll_map_inverse.rs`，翻页与滚动两形态都可用）
- [x] T027 [US7] 翻页⇄滚动切换于 `crates/tome-reader/src/scroll/flow.rs`（FR-023：双向位置不丢；进度以切换稳定后的锚点为准，中间态不写库）
  - [x] SC-005b：`crates/tome-reader/tests/mode_switch_anchor.rs`——两方向切换锚点值 100% 不变且字符 100% 可见
  - [x] 切换过程中无进度写库（同文件断言）
- [x] T028 [US7] 滚动重排保锚点（FR-024：几何参数/窗口尺寸变化后锚点字符仍落视口内；resize 合并/节流不累积）
  - [x] 参数化测试（`crates/tome-reader/tests/scroll_relayout_anchor.rs`，含窗口连续变化）；无"保留像素偏移"式漂移断言
- [x] T029 [US7] 滚动不触发抓取（FR-025：未缓存区以明确占位/加载态呈现；继承 001 §FR-019b）
  - [x] 请求计数断言：`crates/tome-reader/tests/scroll_no_fetch.rs`——滚动到未缓存区抓取次数 = 0
  - [x] 加载态/占位可辨识（同文件断言其存在与文案标识）
- [x] T030 [US7] 按需释放 + 事件节流（FR-026 视口外的行可释放、超长章限定内存；FR-027 滚动事件合并/节流不累积排队）
  - [x] 20 万字章滚动：内存 ≤ 章节字节 ×8（`crates/tome-reader/tests/scroll_memory.rs`，SC-010 实测）
  - [x] 事件风暴（惯性滚动模拟）后无排队任务断言（同文件）
- [x] T031 [US7] 阅读记录两量产出与去重口径于 `crates/tome-reader/src/record.rs`（FR-039：已展示字符区间去重累计、回翻/回滚不重复、跨模式统一、上报节流；经 001 §FR-026b 接口落库，无界面可用）
  - [x] SC-013b：`crates/tome-reader/tests/record_dedup_cross_mode.rs`——同段字符两模式阅读字数相同；模式间来回切换只计一次
  - [x] 快速翻页不产生高频写库（同文件节流断言）

> **⛔ 检查点：S2-3 之后（形态一致性门）**
> - [x] SC-005b 切换保锚点 100%；SC-006b 滚动流完整 100%；SC-013b 记录跨模式一致 100%
> - [x] FR-025 滚动到未缓存区不发起抓取（请求计数断言）且以明确占位/加载态呈现
> - [x] 人工审查后进入下一阶段
>   - PM 注记（2026-10-11）：形态一致性门通过（SC-005b/006b/013b 全 100%，FR-025 零抓取三断言）。滚动维度锚点矩阵 [WIP] 已转正（真实 ScrollFlow）。**遗留补漏**：`ParagraphMode`（FR-018）schema 已备但引擎未施加缩进/间距——归 Phase 6 前导任务。scroll_memory.rs 的 unsafe 计数分配器批准（测试装置固有条件，生产代码仍 forbid）。

---

## Phase 6: S2-4 渲染管线 + 字体（US3/US5/US6）

**Goal**: 行盒 → wgpu 绘制（字形缓存/atlas/批次确定性）→ 离屏 + 像素快照 harness → 字体内嵌 + 回退链 → 翻页动效 → 图文混排。

**Independent Test**: `tome render --chapter <样本>#0 --page 1 --out <png>` 离屏产出图像；同页两次渲染指令序列逐条相同（SC-012）；快照差异 ≤0.1% 或诚实跳过报告（SC-011）；`tome fonts` 报告内嵌子集 3–5MB。

- [x] T032 [US3] wgpu 自绘管线骨架于 `crates/tome-reader/src/render/pipeline.rs`（FR-043；`render` feature 门控；FR-036 架构注释：阅读页内容由本层产出，不经外壳 UI 框架文本栈）
  - [x] feature off（headless）与 on 两种构建均通过；feature off 时依赖树零图形包
  - [x] **门控约定**：`render` feature **默认开启**（开发/发布构建含渲染栈）；headless CI job 显式 `--no-default-features`；feature off 时 `render` 子命令以 `UnsupportedCapability{capability:"offscreenRendering"}` + 退出码 3 回应（该路径即 T048 第三条验收的被测对象）
  - [x] 离屏初始化测试（无 surface 建 device/queue）
- [x] T033 [US3] 字形缓存 + atlas 于 `crates/tome-reader/src/render/glyph_cache.rs` + `atlas.rs`（research R2：SwashCache 栅格化；缓存键 = 字体+字号+字形+SubpixelBin（x 轴 4 分桶、y 对齐行基线）；etagere atlas；增长用 copy_texture_to_texture 搬迁；每帧聚合批量上传）
  - [x] 缓存命中/未命中路径测试；atlas 增长后既有字形仍可渲染
  - [x] US3 场景 2：缺字回退字形呈现且该行其余字符位置不变（不整行跳动）
- [x] T034 [US3] RenderBatch 绘制指令序列于 `crates/tome-reader/src/render/batch.rs`（背景/字形四边形/装饰（高亮/下划线/引用线）/图片四边形；FR-048 同输入同序列）
  - [x] SC-012：`crates/tome-reader/tests/render_deterministic.rs`——同页（或同滚动视口）同参数两次渲染指令逐条相同（预热后，100%）
- [x] T035 [P] [US5] 离屏渲染 + 读回于 `crates/tome-reader/src/render/offscreen.rs`（research R4：Rgba8Unorm；`RENDER_ATTACHMENT|COPY_SRC`；bytes_per_row 对齐 256 读回去 padding；map_async + `device.poll(wait)`；后端不可用报 `UnsupportedCapability{capability:"offscreenRendering"}`，不伪装成功）
  - [x] 离屏产出 PNG（读回像素非全零断言）
  - [x] 无后端环境（mock 注入）下结构化错误测试
- [x] T036 [US5] 像素快照 harness 于 `crates/tome-reader/tests/snapshot.rs`（research R4：hand-rolled pixelmatch-YIQ 双闸门 per-pixel 0.6 + 失败比例 ≤0.1%（初期 0.05% 从严，误报驱动放宽）；probe 探针落 `crates/tome-reader/examples/probe.rs`（`cargo run -p tome-reader --example probe` 打印后端名，CI 以该命令触发，失败则 job 红）；测试体内 adapter 缺失即 panic 诚实模式；CI 上传 actual/difference artifacts 约定写入注释）
  - [x] 自校验：造差异图断言检出、造相同图断言通过（两侧）
  - [x] 后端不可用时输出「SKIPPED: <原因>」显式报告，**不计为通过**
- [x] T037 [US3] 字体内嵌加载 + fontdb 回退链于 `crates/tome-reader/src/render/fonts.rs`（FR-050/051：`assets/fonts/TomeSerifCN-subset.otf` 经 fontdb `load_font_data` 注册为 reading-serif 首选；系统回退链平台表（Windows 宋体 / macOS Songti SC / Linux Noto Serif CJK）；字体获取零网络）
  - [x] 子集缺字时落回退链且行位不变（FR-047）
  - [x] `tome fonts` 数据可用：内嵌体积 3–5MB 断言 + `within_budget` 为 true
- [x] T038 [P] [US3] 翻页动效于 `crates/tome-reader/src/render/animate.rs`（FR-046：MASTER §4 `--ease-spring`/`--dur-slow` 档位；FR-032：reduced-motion 开启时降级瞬时切换且不改变分页结果与位置）
  - [x] 有动效/降级两路径测试；降级后分页结果逐字节相同断言
- [x] T039 [US6] 图文混排于 `crates/tome-reader/src/render/image.rs`（FR-049：图片行盒按显示尺寸参与布局、占位高度与最终一致、不被页边界切开、不可用占位不中断正文；消费 `ChapterContent.images`，001 契约已增补、P8 接线）
  - [x] US6 三场景：不被页边界切开 / 尺寸未定按声明或回退尺寸占位且重排不改锚点 / 与首段成组不孤立
  - [x] ~~**若 001 P8 的 images 接线未就绪**~~ **N/A**：001 P8 已接线 `ChapterContent.images`，本任务走**真实接线**（无占位降级）

> **⛔ 检查点：S2-4 之后（渲染与快照门）**
> - [x] SC-011 快照 ≤0.1%（或诚实跳过报告）；SC-012 指令序列逐条相同；字体内嵌 3–5MB 实测
>   - PM 注记（2026-10-11）：S2-4 门审查通过——三项裁决落地。① **`--page` 真实消费**（先分页再取第 N 页；越界报结构化错误且不产出文件；三页哈希互不相同实测在案）② **真实字形位图路径**（R2 既定方案：SwashCache → etagere → WGSL 点采样 + alpha 混合；移除 T035 的实心块 [WORKAROUND]；装置自检两条各带变异验证：'一' 912px vs '龘' 4960px 墨迹可分辨、抗锯齿 199–211 级）③ **字体体积 2.18 MiB 接受**（「3–5MB 预算」为上限语义，2.18 在限内且 GB2312 6763 字覆盖达标）。**关键发现**：实心块阶段（T035–T039）的 SC-011 快照是**内容盲**的——字数相同的两页产出逐字节相同的图像，即那条判据"能失败"但它失败的**原因集合不含"字形画错"**（BASELINE §13.2 记为本项目第四次同型问题、形态不同于 §10.1 的前三次）。历史数据保留在案，当前数据见 BASELINE §13。
> - [x] 人工审查后进入下一阶段
>   - PM 复核（2026-10-11，session wd_legado）：tome-reader 623 测试绿、变异验证在案、S2-4 门正式关闭。遗留 `--viewport` 未消费（与 --page 同类）归 Phase 7 前导修复；字形缓存跨帧复用/彩色字形/图集淘汰策略记 BASELINE §13.5 归后续性能与增强任务。

---

## Phase 7: S2-5 主题 + 痕迹交互（US1/US2/US4）

**Goal**: Theme 组合根注入 → 昼夜语义映射/无障碍强制/reduced-motion → 痕迹渲染/选择/命中互逆/词典 → 页脚/工具栏/键位。

**Independent Test**: 痕迹渲染覆盖字符集合 == 区间 100%（SC-013）；MASTER §8 Don'ts 逐项断言过；命中往返 ≤半字形。

- [x] T040 [US1] Theme 组合根注入（tome-cli `root.rs` 装配昼/夜实例；`tome-reader` 只经 tome-core 取 Theme，不依赖 tome-app；FR-029）
  - [x] 注入路径测试（`crates/tome-reader/tests/theme_injection.rs`）；两实例语义色字段完整（对照 T002 清单）
  - [x] 分页逻辑不读 Theme（FR-019 的架构断言：paginate 模块无 Theme 引用，grep 断言）
- [x] T041 [P] [US1] 语义色映射 + 无障碍强制断言（FR-030：阅读区背景用主题阅读层色、禁纯白昼底、禁深灰替真黑、正文色不低于 text-secondary；FR-031：对比度/focus 环 2px/触达 ≥32px 渲染层强制）
  - [x] MASTER §8 Don'ts 逐项验收断言（禁蓝/禁纯白昼底/禁深灰夜底/禁 >700 字重）
  - [x] §2.3 对比度表六组组合全部达标断言
- [x] T042 [US4] 痕迹区间→可见片段 + 样式词表于 `crates/tome-reader/src/trace/render.rs`（FR-037：跨页/跨视口分段渲染且拼接无缝隙无重叠；FR-033：`HighlightRule.style` 解析序 = 语义词表→颜色名→默认下划线+诊断，词表可枚举）
  - [x] SC-013：`crates/tome-reader/tests/trace_exactness.rs`——渲染像素范围覆盖的字符集合与区间逐字符相等（100%，含跨页/跨视口）
  - [x] 未知样式回退可辨识（诊断记录断言）；不静默不渲染
- [x] T043 [US4] 命中测试与反查互逆 + 文字选择于 `crates/tome-reader/src/trace/hit.rs`（FR-040：两形态下互为逆运算，往返 ≤半字形；FR-038：选择产字符区间，口径与 `resolve_anchor` 一致）
  - [x] 互逆参数化测试（`crates/tome-reader/tests/hit_test_inverse.rs`，翻页/滚动各一组）
  - [x] US4 场景 3：拖选结果为字符区间且与锚点口径一致（同文件）
- [x] T044 [P] [US4] 词典查词桥于 `crates/tome-reader/src/trace/dict.rs`（FR-041：复用 001 §FR-016 词典求值，不实现第二套；失败提示可操作原因）
  - [x] US4 场景 4：命中词典展示；失败时结构化原因（不静默）
- [x] T045 [US1] 页脚信息于 `crates/tome-reader/src/footer.rs`（FR-042：章号统一为当前章；章内页码；剩余时间 = 当前章剩余字符数 ÷ 近期阅读速度（001 §FR-026b 记录），无足够数据时该项隐藏）
  - [x] 各分支测试（`crates/tome-reader/tests/footer.rs`）：正常估算 / 无数据隐藏（不出现无依据估算值）
- [x] T046 [P] [US1] 阅读工具栏状态机与几何于 `crates/tome-reader/src/toolbar.rs`（FR-034：默认隐形、静止 2.5s 淡出、整窗顶部 8px 热区（不随单/双页变化）、入口清单；对 S3 暴露状态与边界接口，S3 无需反查内部状态）
  - [x] 状态迁移测试（隐形→唤出→淡出）；热区为整窗顶部全域断言
  - [x] 接口形态断言：状态与几何经公开接口输出（不暴露内部实现类型）
- [x] T047 [P] [US1] 键位集 + 栈式 Esc 于 `crates/tome-reader/src/keys.rs`（FR-035：显式键位表逐项可测——前/后翻页、上/下一章、章节首/末、滚动行滚动与页滚动、工具栏唤出、Esc 栈式取消）
  - [x] 键位表逐项测试；Esc 栈式语义两层场景断言（沉浸/工具栏先退，抽屉/弹层在栈顶先关）

> **⛔ 检查点：S2-5 之后（体验完整门）**
> - [x] SC-013 痕迹精确 100%；FR-031 无障碍强制项逐项过
> - [x] 人工审查后进入下一阶段
>   - PM 注记（2026-10-11）：体验完整门通过（SC-013 痕迹精确含跨页/跨视口、FR-031 无障碍六组对比度 AA、MASTER §8 Don'ts 逐项、FR-040 互逆误差 11.08px 贴限不越）。`--viewport` 已真实消费（三视口哈希互异 + 越界不产出文件）。rustdoc 状态标注被误判为 intra-doc link 的 ~70 条告警归 T055 处理。

---

## Phase 8: S2-6 一致性收口（US5/全局）

**Goal**: CLI 五命令全接线 → 三平台 CI（含快照后端矩阵）→ 性能基线 → 体积记录 → 干净室轴 + 宪章复检。

**Independent Test**: contracts/cli.md 全部子命令 golden 测试过；CI 三平台绿（含快照）；perf_baseline 分档达标。

- [ ] T048 [US5] CLI 五子命令薄壳于 `crates/tome-cli/src/reader_commands.rs`（layout / layout-anchor / render / fonts / verify-layout；contracts/cli.md 全选项接线 + `flags.rs` 注册与帮助文本；逻辑不在 CLI 重复实现）
  - [ ] 契约 §4：每子命令 golden 测试（输出 JSON 与期望逐字段比对）
  - [ ] `layout` 确定性断言：同参数两次运行输出逐字节相同
  - [ ] 渲染子命令后端不可用时退出码 3 + 结构化能力错误（不伪装成功）
- [ ] T049 [P] [US5] CI 三平台矩阵 + 快照后端于 `.github/workflows/ci.yml` 扩展（ubuntu = lavapipe（mesa-vulkan-drivers + VK_DRIVER_FILES）；windows = lavapipe（mesa-dist-win；备选 WARP NuGet 钉版）；macos = Metal（禁 GL）；probe 步骤先行打印后端名；R5-4 Windows-gnu + wgpu 构建验证）
  - [ ] 三平台快照测试实际运行（非跳过）；probe 日志可见后端名
  - [ ] feature 组合写明：默认构建（render on）跑快照；另设一个 headless job 以 `--no-default-features` 构建并断言 feature off 时 `render` 子命令返回退出码 3
  - [ ] SC-015：同字体同参数分页结构与滚动映射三平台一致断言
- [ ] T050 [US5] 性能基线于 `crates/tome-reader/tests/perf_baseline.rs`（release 模式；SC-007 翻页目标 16ms/红线 33ms；SC-008 首屏 400/800；SC-009 重排 150/300；SC-010 内存 ≤ 章节字节 ×8 且 20 万字不失败；口径注释完整：预热稳态中位数、不含抓取/动效）
  - [ ] 分档判据输出（目标/红线分列）；超红线测试失败
- [ ] T051 体积记录（SC-017：本层净增量（cosmic-text 栈 + 字体子集 + wgpu 栈）测量写入 `specs/002-typesetting-renderer/size-report.md`；快照基线资产确认 dev-only 不进发布包）
  - [ ] 报告含与 001 基线的对比；快照资产排除出发布物的构建验证（`cargo package -p tome-reader --list` 断言不含 `tests/snapshots/`，且 `assets/fonts/` 仅含 `.otf`/`OFL.txt`/`FONTS.sha256`）
- [ ] T052 干净室轴 + 宪章复检于 `specs/002-typesetting-renderer/checklists/`（新建 `constitution.md` 与 `cleanroom.md`；FR-057/058 扫描：错误分类全覆盖、无面向用户自然语言硬编码）
  - [ ] 两份复检文档落盘并 `git add` 随本任务 commit；扫描零命中

> **⛔ 检查点：S2-6 之后（S2 完成门）**
> - [ ] SC-007~010 性能分档达标；SC-014 无图形环境全能力可调用 100%；SC-015 三平台一致；SC-016 零外发；SC-017 体积在案
> - [ ] FR-057/058 错误分类全覆盖、无自然语言硬编码
> - [ ] 人工审查 + 干净室轴审查通过

---

## Phase 9: Polish & Cross-Cutting Concerns

- [x] T053 [P] quickstart.md 从零验证（按 `specs/002-typesetting-renderer/quickstart.md` 逐节走一遍，修正不符处）
  - [x] 每条命令真实可运行；期望与实际一致；修正处随本任务 commit
  - PM 注记（2026-10-11）：逐节实走，7 处不符已修（`--chapter`→`--text`、测试选择子缺 `--test` 导致 45 个目标零执行、`--page` 0 起口径、fonts 体积 2.18MiB、verify-layout 字段名、perf 需 `--ignored`、渲染环境结论）；另补 `tests/fixtures/chapter-zh.txt` 走查语料（仓库原本不存在该文件）。
- [x] T054 [P] AGENTS.md 更新——`crates/tome-reader/` 入目录结构与依赖图描述（AUTO:STRUCTURE 段）
  - [x] 文档与真实结构一致
  - PM 注记（2026-10-11）：AUTO 段**无生成脚本**（`.specify/scripts/` 仅 speckit 流程脚本），故手工更新并在 commit message 注明。除 STRUCTURE 段外一并修正同文件已过时事实（AUTO:BUILD 测试规模 2320→实测 3110/2958、PROJECT-META 的 crate 数、规格驱动流程的当前特性 001→002）。
- [x] T055 [P] rustdoc 完整性——`cargo doc --no-deps -p tome-reader` 无警告
  - [x] 公开 API 均有 docstring（模块职责 + 关键入口）
  - PM 注记（2026-10-11）：修前 71 告警（60 条 `[DONE]` 等状态标注被误判 intra-doc link + 11 条失效链接/私有项/缺文档），修后 0（连续两次稳定）。状态标注一律反引号包裹、**未删任何信息**（76 处改前改后一致）。同源告警顺带清零 `tome-cli`，故 `--workspace` 亦为 0。
- [x] T056 修复 headless 构建下 `render` 退 3 缺口（PM 新增 2026-10-11）
  - [x] 能力缺失压过用法错误：headless 构建里 `render`/`fonts` 无论参数是否合法均退 3 + `offscreenRendering` 能力标识（FR-053/FR-057/FR-058、contracts §2.3）
  - [x] `fonts`/`render` 两条路径行为对称；CI headless job 断言脚本同步为参数形态矩阵
  - 落点：`crates/tome-cli/src/flags.rs`（`parse()` 入口前置短路 + `headless_render_face_subcommand` 辅助）、`crates/tome-cli/src/commands.rs`（`CommandOutcome::failure` 带出 `ErrorCapability` 标识）、`crates/tome-cli/tests/render_headless_capability.rs`（新建，6 用例；已实测无修复时变红）、`.github/workflows/ci.yml`（断言矩阵 + 短路未扩大的反例 + 装配必然失败的反例）
  - 发现（超出 T056 复述范围，一并修正）：CI 断言要求 stderr 携带 `offscreenRendering`，但 `fonts` 此前**也不带**该标识（`CommandOutcome::failure` 从未渲染 `ErrorCapability`）——只退码对、标识缺。故“fonts 已正常”仅对退出码成立。
  - **PM 验收打回后的二次修复（2026-10-11）**：
    - C1 不对称：首版只把 `render` 纳入 parse 短路，`fonts` 仍走「装配 → 分发 → 能力错误」，退出码取决于**装配成不成**（PM 机上 8 轮 2 轮复现为退 1 + 裸 `error.storage`）。现 `fonts` 一并纳入短路，辅助函数更名为 `headless_render_face_subcommand` 以如实反映覆盖范围；两命令的能力错误路径现在都**早于任何 IO**。
    - C2 产品级 flake：`probe()`（`crates/tome-store/src/datadir.rs`）对 write/remove_file/read_dir 三个 IO 点加有限重试（3 次尝试、间隔 50ms；常量 `PROBE_ATTEMPTS`/`PROBE_RETRY_DELAY` 注明 Windows AV 对新建文件瞬时占用的理由）。**面向真实用户机器的产品级加固**，非测试化妆（真实用户同样跑杀软）。
    - C2 诊断可见性：`AppRoot::assemble`/`assemble_with` 由 `Result<_, ErrorKind>` 改为 `Result<_, ClassifiedError>`，`StoreError::to_classified()` 保留细节；`main.rs` 打印 `error.storage: <原因>；<指引>`（标识前缀不变，`error_contract.rs` 断言语义不受影响——已实测 9/9 通过）。类型系统现**结构性阻止**再次丢细节（旧 `.kind().clone()` 已无法编译）。
    - 稳定性证据：修复后 `render_headless_capability` **连跑 20 轮全绿**；`layout_cli`/`render_cli`/`verify_layout_cli`/`theme_injection_cli` 全绿（28/12/12/2）。
    - 另记录不处理（PM 已立项）：`scroll_memory.rs:296` 既有 flaky、`chapter-by-bookshelf` 标识未登记入 `contracts/cli.md`。

---

## Dependencies & Execution Order

### Phase 依赖

- **Phase 1 Setup**：无依赖，立即开工
- **Phase 2 S2-0 spike**：依赖 Setup。**阻塞所有用户故事**——可行性门（对齐 001 S0 模式）
- **Phase 3 S2-1（US1）**：依赖 S2-0 门通过（尤其 T005 的 ShapeWord 行为证实）
- **Phase 4 S2-2（US1/US2）**：依赖 S2-1（行盒产出是分页输入）
- **Phase 5 S2-3（US7）**：依赖 S2-2（滚动流复用同一排版/分页结果）
- **Phase 6 S2-4（US3/US5/US6）**：依赖 S2-2（行盒→绘制）；T039 另依赖 001 P8 的 `ChapterContent.images` 接线（未就绪走占位降级）
- **Phase 7 S2-5（US1/US2/US4）**：依赖 S2-4（痕迹渲染需要 PositionMap/RenderBatch）
- **Phase 8 S2-6（US5/全局）**：依赖全部前置切片
- **Phase 9 Polish**：依赖 S2-6 门通过

### 切片内 TDD 排序

每任务：写失败测试（RED）→ 最少实现（GREEN）→ commit。模型先于服务、服务先于接线（text → rules → layout engine → paginate → scroll → render → trace → CLI）。

### 并行机会

- Phase 1：T002/T003/T004 [P] 可并行（不同 crate/文件）
- Phase 2：T006/T007/T008/T009 [P] 可并行（spike 独立验证项）
- Phase 3：T011/T014 [P]（paragraph 与 squeeze/mixed 不互相依赖）；T012 → T013 → T015 有依赖链
- Phase 4：T017 → T018 有依赖；T019 → {T020, T021, T022, T023} 有依赖（页序列是四者的输入）
- Phase 5：T025 → T026 → T027 有依赖链；T028 依赖 T025/T026
- Phase 6：T035 [P]（offscreen 可与 glyph_cache 并行）；T038 [P]
- Phase 7：T041/T044/T046/T047 [P]
- Phase 9：全部 [P]

## Parallel Example

```bash
# Phase 2 spike 验证项（T005 是证伪门先行，其余并行）：
Task: "T006 hinting 影响 + 贪心换行质量"
Task: "T007 体积对照 + Windows-gnu wgpu"
Task: "T008 字体子集实产"
Task: "T009 离屏探针 + 容差初判"

# Phase 7 的四个独立模块：
Task: "T041 无障碍断言" | Task: "T044 词典桥" | Task: "T046 工具栏状态机" | Task: "T047 键位集"
```

## Implementation Strategy

### MVP First（最小闭环 = Phase 3 / S2-1 + Phase 4 / S2-2）

翻页模式的 headless 排版与分页：文本 → 中文规则 → 行盒 → 页序列 → 锚点不丢——这是"引擎能力"变"可读页面"的最小路径（spec US1+US2），经 `tome layout` 完整可验证，不依赖渲染管线。

### 增量交付

1. Setup + S2-0 门 → 技术风险清零
2. S2-1 → 排版正确（可演示：标点密集语料零违反）
3. S2-2 → 翻页阅读 headless 完整（MVP）
4. S2-3 → 滚动形态（复用 MVP 成果）
5. S2-4 → 可视渲染 + 字体内嵌 + 快照
6. S2-5 → 主题/痕迹/交互完整
7. S2-6 → 三平台 CI + 性能/体积收口

### 高风险化解（与 plan.md 一致）

- T005（ShapeWord 合并证伪风险）排最前；证伪 → 上报 PM，备选方案 C 须用户批准
- T039（001 P8 images 外部依赖）排 S2-4 末尾 + 占位降级路径
- 快照容差校准（R5-6）在 T009 早判、T036 从严起步

## Notes

- hinting 默认关闭（research R1/R4 共同决策：标点挤压的亚像素量与三平台一致性都依赖它）；低 DPI 锐度属后续体验权衡
- 贪心换行 + 挤压的排版质量经 R5-7 评测记录；如出现不可接受的次优断点，备选 = 在 justify 两段式中做局部回退重排（不引入 Knuth-Plass 全局重排）
- 渲染层一切测试须可在 lavapipe/Metal 之外的环境诚实跳过（SC-011 口径）；**跳过必须显式报告，永远不计为通过**
- 001 侧待办联动：`ChapterContent.images` 的 P8 接线完成后，T039 的 [WIP] 占位须补实并去标注
