# Tasks: S3 应用外壳（tome-app）

**Input**: Design documents from `/specs/003-app-shell/`

**Prerequisites**: plan.md ✅ / spec.md ✅ / research.md（+三份专题）✅ / data-model.md ✅ / contracts/ui.md ✅

**Tests**: 含（宪法原则 III 测试先行——每个任务按 TDD 微步骤：先写失败测试 → 确认失败 → 最少实现 → 确认通过 → commit）。

**Organization**: 按用户故事分组（US1-US4 对应 spec.md 四个故事）；Phase 1/2 为 plan.md 的 P0 切片（基座），Phase 7 为 plan.md 的 P4 收口。

**已批准决策（2026-10-11，勿再开）**：eframe 0.36.2 / PaintCallback 路径 A（fallback B 须用户批准）/ release `panic="unwind"` / UI 铬文本平台字体链 / MASTER 四处对比度规则 / 单双页阈值窗口宽 1512px。

---

## Phase 1: Setup（tome-app 骨架）

**Purpose**: crate 落地与守卫适配——本阶段不改任何引擎代码。

- [ ] T001 创建 `crates/tome-app/` crate 骨架（`Cargo.toml` + `src/main.rs` 可启动空窗口）
  - [ ] `Cargo.toml`：eframe 钉 `0.36.2`（默认 wgpu 渲染器）、egui_extras、app-single-instance、dark-light、serde/tome-core/tome-reader 依赖；`render` feature 门控形态与 tome-cli 一致（`default = ["render"]`、`render = ["tome-reader/render"]`）
  - [ ] 根 `Cargo.toml` workspace members 注册 tome-app；`[workspace.dependencies]` 表内声明（成员使用点的 `default-features = false` 写这里——S2 实测陷阱）
  - [ ] `cargo run -p tome-app` 打开空 egui 窗口可关闭；`cargo build --workspace` 与 `cargo build --workspace --no-default-features` 均通过
- [ ] T002 [P] 守卫适配：`crates/tome-cli/tests/dependency_graph.rs` 的 `ENGINE_CRATES`（`:146`）排除名单加入 tome-app（与 tome-cli 同口径：组合根合法依赖 tome-reader）
  - [ ] `cargo test -p tome-cli --test dependency_graph` 绿
  - [ ] `bash scripts/check-reader-features.sh` PASS（包作用域不受影响，实测确认）
- [ ] T003 [P] `[profile.release]` `panic = "abort"` → `"unwind"`（根 `Cargo.toml:43`，R6 已批准）+ 注释更新（说明 GUI 兜底需求与体积代价记账处）
  - [ ] `cargo build --release -p tome-cli` 通过；release 体积实测记录（供 T035 size-report 引用）
  - [ ] 注释写明：001 选 abort 时尚无 GUI 进程；S3 引擎线程 catch_unwind 需要 unwind（决策 R6，research.md）

---

## Phase 2: Foundational（P0 基座——阻断全部故事）

**Purpose**: 渲染嵌入实证 + 引擎线程 + 主题桥 + 应用骨架。**⚠️ 未完成前任何故事不得开工。**

- [ ] T004 tome-reader 渲染管线新增 `from_shared_device` 构造于 `crates/tome-reader/src/render/pipeline.rs`（render feature 下）
  - [ ] 新构造接受外部 `wgpu::Device`/`wgpu::Queue`/target format 建管线（不自建 Instance）；headless 离屏路径（Rgba8Unorm + readback）行为不变
  - [ ] 先写失败测试：共享 device 构造的管线能渲染一帧与离屏路径逐像素一致（同字体同内容）
  - [ ] `cargo test -p tome-reader` 全绿；`cargo test -p tome-reader --no-default-features` 全绿（cfg 门控正确）
- [ ] T005 [高风险] S3-0 spike：`crates/tome-app/examples/spike_render.rs`——eframe 窗口内 PaintCallback 跑通 S2 渲染管线（T004 的共享 device），CI 三平台快照 job 扩展 spike 场景（`.github/workflows/ci.yml`）
  - [ ] spike 窗内呈现的排版帧与 S2 离屏输出逐像素一致（probe 步骤先行打印后端名，沿用 T049 形态）
  - [ ] CI 三平台（windows-lavapipe/linux-lavapipe/macos-metal）spike job 绿；`render` feature-off 时 spike 目标不参与构建（cfg 门控）
  - [ ] **门：本任务不过则启动 fallback 路径 B（readback 纹理）——须 PM 报用户批准后执行**
- [ ] T006 [P] 消息协议 `crates/tome-app/src/engine/messages.rs`（EngineRequest/EngineEvent 全量值类型，serde 可序列化；按 data-model.md §3 定义）
  - [ ] 协议字段与 data-model.md §3 逐条一致（自检表）；`ClassifiedErrorView` 含 id/detail/capability 三字段
  - [ ] 序列化 round-trip 测试绿（全变体）
- [ ] T007 引擎线程 `crates/tome-app/src/engine/thread.rs` + `mod.rs`（EngineHandle：send/collect；独占 Paginator/FontSystem/store/source；catch_unwind 边界 → EnginePanic 事件）
  - [ ] 先写失败测试：发送会导致 panic 的合成请求 → 收到 `EnginePanic` 事件且线程存活可继续服务（T003 的 unwind 前提）
  - [ ] 线程模型断言：Paginator 不离开引擎线程（编译期 + 文档双重）
  - [ ] 启动期 IO 失败（数据目录不可写/库损坏）以结构化错误呈现并给出指引（S1 FR-043 语义，spec FR-023）——引擎线程启动失败不静默
- [ ] T008 [P] 主题桥 `crates/tome-app/src/theme_bridge.rs`（tome_core::theme Day/Night → egui Visuals/Style；平台 UI 字体链注册——Windows 雅黑/macOS PingFang/Linux Noto Sans CJK；FollowSystem 经 dark-light 探测映射）
  - [ ] 消费 tome-core theme 门面，**零色值重声明**（grep 守卫断言：tome-app 源码无 hex 色值字面量）
  - [ ] R7 四规则落实为常量注释与 API：鎏金禁作文本前景、text-tertiary 禁必要信息、border-strong 不作唯一边界、badge 重建
  - [ ] FollowSystem 在 OS 主题切换时返回对应 Day/Night token（单元测试以注入探测源模拟）
- [ ] T009 应用骨架 `crates/tome-app/src/main.rs` + `app.rs`（单实例激活已有实例；ViewportBuilder 最小尺寸/标题；AppView 状态机骨架含 §2 六条不变量的 headless 断言；eframe App trait 的 ui+logic 两段——0.34 新形态）
  - [ ] 单实例实测：二次启动激活已有窗口不新开（Windows 本机实测；Wayland 降级 RequestUserAttention 注释在案）
  - [ ] AppView 状态机六条不变量（data-model.md §2）的单元断言全绿
  - [ ] `cargo run -p tome-app` 呈现空书架视图骨架（MASTER 空态 + 引导按钮，wired 到 T008 主题）

**检查点：S3-0 门（P0 末）**
- [ ] spike 三平台 CI 绿 + 基座四命令双守卫绿（`cargo build --workspace` / `cargo test --workspace -j 2` / clippy 0 告警 / fmt / 两守卫脚本）
- [ ] 未过 → fallback 路径 B 须用户批准

---

## Phase 3: User Story 1 - 书架与本地书阅读闭环 (Priority: P1) 🎯 MVP

**Goal**: 导入本地书 → 书架呈现 → 打开阅读 → 进度记忆恢复 → 移除。

**Independent Test**: quickstart.md §1 全节（导入 ≤3 步、首屏 ≤400/800ms、位置逐字符恢复、冷启动 ≤1.5s）。

- [ ] T010 [US1] BookRepository 生产 adapter `crates/tome-store/src/book_repository.rs`（impl tome_core::BookRepository over BookDao/TraceDao/MiscDao；书架列表/进度读写/移除含缓存清理）
  - [ ] 先写失败测试：复用 `tome-core/tests/seam_contract.rs` 的假体契约形态对生产实现逐条断言（书架增删查、进度往返）
  - [ ] `cargo test -p tome-store` 全绿；接缝契约断言与假体同一判据
- [ ] T011 [US1] 书架状态 `crates/tome-app/src/state/shelf.rs`（ShelfEntry 列表/过滤/排序；引擎事件驱动更新；AppView::Shelf 字段全量落地）
  - [ ] 状态机断言：过滤/排序/滚动位置在 Shelf⇄Reading 往返后不变（SC-010 的 headless 形态）
  - [ ] 1000 册合成数据的过滤/排序响应断言（状态层，不涉渲染）
- [ ] T012 [US1] 书架视图 `crates/tome-app/src/views/shelf.rs` + `widgets/book_card.rs`（虚拟化网格/封面占位/进度卡片/空态引导/过滤框/排序切换）
  - [ ] 空态必有引导动作（contracts/ui.md V1）；封面缺失走 MASTER 占位形态
  - [ ] 虚拟化断言：1000 册仅实例化可视区卡片（帧时间分布记录供 SC-005 判定）
- [ ] T013 [US1] 本地导入路径（拖入 + 文件选择双径 → `EngineRequest::Shelf::ImportLocal` → 失败结构化呈现；`widgets/skeleton.rs` 导入中骨架）
  - [ ] 坏文件（伪造 EPUB/乱码 TXT）给出具体错误（S1 local 错误分类），书架不留半成品（spec Edge Case）
  - [ ] 重复导入识别为同一条目（更新非重复卡片）
- [ ] T014 [US1] 阅读视图 `crates/tome-app/src/views/reading.rs` + `render/glyph_stage.rs`（UI 线程字形暂存 + atlas，egui device 上）+ `render/texture.rs`（PaintCallback 消费 laid_out 快照渲染版心；页脚：章标题+进度；`ContentState` 三态呈现；纹理缓存按 content_version 失效）
  - [ ] 静态帧零重绘断言：version 未变时 S2 重渲染不被触发（contracts/ui.md §4.2）
  - [ ] 加载中骨架 + 失败错误态（重试/返回动作）按 contracts/ui.md R1/R3 呈现
- [ ] T015 [US1] 进度记忆 `crates/tome-app/src/state/progress.rs`（翻页/滚动改变位置 → 节流落盘；打开时 ProgressRestored 恢复；`EngineRequest::Progress::Flush` 退出前冲刷）
  - [ ] 先写失败测试：正常退出→重开位置逐字符相等（SC-009）；节流窗口内杀进程（模拟）回退 ≤ 一个窗口
  - [ ] 节流窗口值定为 **3 秒**（spec 假设节的量化落定，常量注明可调整路径）
- [ ] T016 [US1] US1 集成走查：quickstart.md §1 全节人工+机检执行，SC-001/002/003 实测值记录于本任务回执
  - [ ] 导入到开始阅读 ≤3 步（逐步计数记录）
  - [ ] 首屏 ≤400/800ms、冷启动 ≤1.5s 实测达标（release 形态测量，口径写入回执）

**检查点：P1 末（US1 端到端）**
- [ ] quickstart §1 全过 + 双形态测试绿 + SC-001/002/003 实测达标

---

## Phase 4: User Story 2 - 阅读中的个性化调整 (Priority: P2)

**Goal**: 五组调整不离开阅读视图即时生效且锚点保持；抽屉目录/书签；章间接续；窗口几何适配。

**Independent Test**: quickstart.md §2 全节（SC-006 时延、锚点 100% 不变、抽屉不改版心、书签持久化）。

- [ ] T017 [P] [US2] 调整消息路径（`EngineRequest::Reading::SetParams` → 引擎重排 → `ChapterReady{content_version+1}`；`ReadingParams` 值类型全字段）
  - [ ] 锚点保持断言（消息层 headless）：五组参数逐一改变，响应事件中的 anchor 与请求前逐字符相等（SC-006 状态机表达）
  - [ ] FR-007b 规则断言：首开取全局默认；该书调整后重启仍按该书快照；全局默认修改不回写已读书
- [ ] T018 [US2] 调整 UI：`widgets/stepper.rs`（字号/行高步进器，MASTER gap 处置——无 Slider 用步进器）+ 阅读工具条（五组调整控件，tome-reader keys 的十项工具栏标识复用）
  - [ ] 工具条不离开阅读视图可操作全部五组（contracts/ui.md R2）
  - [ ] 步进器步长与边界（字号 12–32px、行高 1.4–2.6，MASTER ReadingMetrics 口径）断言
- [ ] T019 [P] [US2] 覆盖式抽屉 `crates/tome-app/src/views/drawer.rs` + 目录页签（Toc 加载/当前章高亮/跳章）
  - [ ] 抽屉开合前后版心几何逐像素不变（覆盖式断言，FR-008）
  - [ ] 跳章后目标章首字符可见；动效遵循 MOTION.md 时长/缓动
- [ ] T020 [P] [US2] 书签页签（`EngineRequest::Bookmark` 三动作；预览文本生成；删除确认；锚点失效钳制）
  - [ ] 书签增/跳/删全路径断言（含重启后持久化）；失效锚点按 S2 钳制定位最近有效字符（spec Edge Case）
  - [ ] 书签跳转与目录跳转走同一锚点定位路径（FR-008d 断言）
- [ ] T021 [US2] 章间接续 + 窗口几何适配（FR-006b：翻页末页→下章首页/首页前翻→上章末页；滚动章末自动接续；FR-011 窗口尺寸变化实时适配；1512px 单双页阈值）
  - [ ] 章间接续锚点连续断言（两形态各双向）；接续加载态/失败态复用 ContentState 三态
  - [ ] 阈值断言：窗口宽 ≥1512px（19px 字号）才允许双页呈现，低于阈值自动单页（contracts/ui.md §4.3）
- [ ] T022 [US2] US2 检查点：quickstart.md §2 全节执行 + SC-006 实测（调整到新帧 ≤150ms，中位数）记录于回执
  - [ ] 锚点保持断言全绿（T017 消息层 + GUI 层抽查）
  - [ ] SC-006 实测达标

**检查点：P2 末**

---

## Phase 5: User Story 3 - 网络书源获取闭环 (Priority: P3)

**Goal**: 书源导入 → 搜索 → 详情 → 加架 → 正文加载 → 换源。

**Independent Test**: quickstart.md §3 全节（合成书源；首屏 ≤3s；≤4 步到可读；换源同位置）。

- [ ] T023 [US3] SourceEngine 生产 adapter `crates/tome-source/src/source_engine.rs`（impl tome_core::SourceEngine over 既有 search/info/toc/content/change_source 自由函数 + source import 三径）
  - [ ] 先写失败测试：接缝契约形态断言生产实现（搜索/详情/目录/正文/换源五动作 + 导入三径）
  - [ ] `cargo test -p tome-source` 全绿
- [ ] T024 [US3] 书源导入 UI（`ImportDialog` 三形态：文件/URL/剪贴板文本；失败结构化原因呈现）
  - [ ] 坏源文件给出具体失败原因；成功后可立即搜索（spec US3 场景 1）
  - [ ] 三径各有验收用例（URL 用本地 HTTP 测试服，不入外网）
- [ ] T025 [US3] 搜索（`SearchSession{generation}`；分组流式呈现；取消与过期作废）+ `widgets/badge.rs`（源分组头部徽章，按 R7 规则重建不照抄 preview）
  - [ ] generation 不变量断言：取消/新搜索后旧 generation 结果不再呈现（data-model.md §2 不变量 4）
  - [ ] 首屏 ≤3s 实测记录（SC-007；合成源 + 本地 HTTP 服）
- [ ] T026 [US3] 详情 + 加入书架（DetailLoaded 呈现简介/目录；`AddToShelf` → 书架出现该书进度 0%）
  - [ ] 加入书架后该书打开按 FR-007b 取全局默认参数
  - [ ] 搜索→详情→加架→开始阅读 ≤4 步（逐步计数，SC-004）
- [ ] T027 [US3] 正文加载与换源（逐章加载含净化；缓存复用不重复抓取；当前源失败 → 换源列表 → 同位置继续）
  - [ ] 缓存复用断言：同一章二次打开零网络请求（录制回放形态）
  - [ ] 换源后锚点映射到同章节位置（S1 换源语义）；换源列表只列同书名可用源
- [ ] T028 [US3] US3 检查点：quickstart.md §3 全节执行 + SC-004/007 实测记录
  - [ ] 合成书源闭环全过（导入/搜索/加架/正文/换源）
  - [ ] 网络离线形态：结构化错误 + 本地书与已缓存不受影响（spec Edge Case）

**检查点：P3 末**

---

## Phase 6: User Story 4 - 键盘可达与最小设置 (Priority: P4)

**Goal**: 全部核心操作纯键盘完成；四组全局默认持久化；文案表落地。

**Independent Test**: quickstart.md §4.1/4.2（SC-008 核对表 + 设置重启保持）。

- [ ] T029 [P] [US4] 快捷键表与键盘分发 `crates/tome-app/src/keys.rs`（contracts/ui.md §2 全表逐条实现；`widgets/focus.rs` 焦点环自绘：2px 环 + 2px offset、仅键盘来源显示）
  - [ ] 快捷键全表逐条 headless 断言（键事件 → 预期 EngineRequest/视图动作）
  - [ ] **负例断言**：`Ctrl+K` 无绑定（命令面板延后 D3）；焦点环仅在键盘导航后出现（鼠标点击不出现）
- [ ] T030 [P] [US4] 设置叠加层 `crates/tome-app/src/views/settings.rs` + `state/settings.rs`（四组且仅四组；快捷键表只读展示；读写通道经 `EngineRequest::Settings`；`Save` 持久化）
  - [ ] 四组修改→保存→重启进程→设置保持（SC-012 断言）
  - [ ] 负例断言：设置视图不存在第五组选项（范围纪律）
- [ ] T031 [P] [US4] 外壳文案表 `crates/tome-app/src/i18n.rs`（zh 主；引擎 error id → 文案映射；`[capability: xxx]` 呈现规则）
  - [ ] 零字面值守卫：grep 断言组件文件（views/widgets）无中文字面串（contracts/ui.md §3.1）
  - [ ] 错误标识永不翻译断言（映射输出必含原 id）
- [ ] T032 [US4] US4 走查：SC-008 键盘可达核对表（contracts/ui.md §2 全表人工+机检逐项）+ quickstart.md §4.1/4.2
  - [ ] 全程无鼠标完成「打开书→读两页→改字号→存书签→回书架」
  - [ ] 核对表全项签字记录于回执

---

## Phase 7: Polish & Cross-Cutting（P4 收口）

**Purpose**: 三态覆盖、MASTER 合规、体积落账、文档收口——跨故事项。

- [ ] T033 [P] 三态全覆盖走查（contracts/ui.md §1 视图契约 V1-V4/R1-R6/设置层逐条：空态/加载态/错误态）
  - [ ] 每条契约行有对应断言或走查记录；错误必有可行动作（data-model.md §2 不变量 6）
- [ ] T034 [P] MASTER 合规检查表 `specs/003-app-shell/checklists/master-compliance.md`（SC-011 逐项；对比度 ≥4.5 抽测；R7 四规则；token 零重声明守卫复核）
  - [ ] 检查表全项通过签字；发现的例外逐项记录并修或裁决
- [ ] T035 [P] 体积报告 S3 段（`specs/003-app-shell/size-report.md`：eframe/winit/egui_extras 增量、unwind 表代价实测、UI 平台字体决策记账；`cargo build --release -p tome-app` 双形态实测）
  - [ ] 增量构成可审计（二分测量同 002 §1 形态）；与 S2 末态（默认 18.92MiB/headless 15.97MiB）对比记账
- [ ] T036 收口：`AGENTS.md` 目录结构与依赖图更新（tome-app 入图）+ quickstart.md 全量走查签字 + tasks.md 勾选复核
  - [ ] AGENTS.md 结构/依赖图/构建命令反映 S3 落地现状（含 tome-app 运行方式）
  - [ ] quickstart.md §0-§4 全节走查记录完成

**检查点：S3 完成门（P4 末）**
- [ ] SC-005/008/009/010/011/012 全验 + MASTER 合规签字 + 体积报告在案
- [ ] CI 全绿（含 spike/快照矩阵）+ 四命令双守卫 + `cargo doc --no-deps --workspace` 0 告警

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1（Setup）**：无依赖，立即开工
- **Phase 2（Foundational/P0）**：依赖 Phase 1——**阻断全部故事**；S3-0 门不过则启动 fallback（须批准）
- **Phase 3-6（US1-US4）**：均依赖 Phase 2；故事间按 P1→P2→P3→P4 顺序（US2/US3 可有限并行：T019/T020 与 T023-T025 不同文件）
- **Phase 7（Polish）**：依赖 US1-US4 全部完成

### User Story Dependencies

- **US1（P1）**：Phase 2 后开工，无其它故事依赖——MVP
- **US2（P2）**：依赖 US1 的阅读视图与参数状态（T014/T015）；抽屉/书签/调整在其上
- **US3（P3）**：依赖 US1 的书架接口（T011）与引擎消息协议；独立于 US2
- **US4（P4）**：依附 US1-US3 的界面存在（键盘/设置是横切），可并行准备（T029/T030/T031 [P]）

### Parallel Opportunities

- Phase 2 内：T004 ∥ T006 ∥ T008（不同 crate/文件）
- Phase 4 内：T017 ∥ T019 ∥ T020（消息路径 ∥ 抽屉骨架 ∥ 书签）
- Phase 5 内：T024 ∥ T025（导入 UI ∥ 搜索会话，同依赖 T023）
- Phase 6 内：T029 ∥ T030 ∥ T031（keys ∥ settings ∥ i18n）
- Phase 7 内：T033 ∥ T034 ∥ T035

### Parallel Example: Phase 2 基座

```bash
Task: "T004 tome-reader from_shared_device"
Task: "T006 engine/messages.rs 消息协议"
Task: "T008 theme_bridge.rs 主题桥"
```

---

## Implementation Strategy

### MVP First（US1 Only）

1. Phase 1 + Phase 2（S3-0 门）→ 2. Phase 3（US1）→ **停下验证**（quickstart §1 + SC-001/002/003）→ 本地书可读即 MVP

### Incremental Delivery

1. P0 基座 → 2. US1 本地书闭环 → 3. US2 阅读体验 → 4. US3 书源获取 → 5. US4 键盘设置 → 6. Polish 收口 → S3 完成门

### 粒度与检查点对齐声明

本 tasks.md 的 Phase 组织、检查点位置与 plan.md「Implementation Strategy」节逐条对齐（P0=Phase1+2、P1=Phase3、P2=Phase4、P3=Phase5、P4=Phase6+7；S3-0 门/P1 末/P2 末/P3 末/S3 完成门五检查点原位）。无偏离。无 XL 任务（最大 M）。

## Notes

- 每任务 TDD 微步骤（失败测试先行）为强制；验收子项即该任务的独立验收标准
- `tome-app` 渲染路径一律 feature 门控（workspace `--no-default-features` 门不红）
- 干净室红线全程有效；交互语义以 spec + MASTER.md 为准
- 体积增量记账到 `specs/003-app-shell/size-report.md`（T035）；20MB 已裁决非红灯但须可审计
