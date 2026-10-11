# Implementation Plan: S3 应用外壳（tome-app）

**Branch**: `003-app-shell` | **Date**: 2026-10-11 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/003-app-shell/spec.md`

> **面向 AI 代理的工作者：** 使用 subagent-driven-development（推荐）或 executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法跟踪进度。

**目标：** 为 tome 构建桌面应用外壳（`tome-app` crate）——egui 单窗口双视图（书架 ⇄ 阅读），消费 S1 引擎与 S2 排版渲染的全部已验收能力，交付「能日常读书」的桌面阅读器。

**架构：** eframe 0.36.2 单窗口应用（wgpu 30 与 S2 同源）；UI 线程持 egui + 渲染状态，引擎线程独占 Paginator/store/source 阻塞工作，消息通道交互；S2 渲染经 PaintCallback 嵌入 egui 画布（同 device，零拷贝）；外壳只依赖 tome-core 接缝（为 BookRepository/SourceEngine 补生产 adapter）。

**技术栈：** Rust 1.99 / eframe 0.36.2 / wgpu 30.0.1 / cosmic-text 0.19（既有）/ rusqlite（既有）/ app-single-instance / dark-light（跟随系统主题）

## Summary

S3 把 S1（书源管线/本地书/存储/锚点契约）与 S2（双形态排版/wgpu 渲染/主题）经 egui 外壳交付为可用桌面阅读器：书架与本地书闭环（US1）、阅读中个性化调整且锚点保持（US2）、网络书源最薄获取闭环（US3）、键盘可达与最小设置（US4）。关键风险前置：S3-0 spike 先证「eframe + S2 wgpu 管线同 device」三平台可行；两个阻塞缺口（接缝无生产实现、panic=abort 与 panic 捕获冲突）在计划中显式修复。

## Technical Context

**Language/Version**: Rust 1.99.0（stable-gnu，rust-toolchain.toml 钉版；MSRV 声明 1.98）

**Primary Dependencies**: eframe 0.36.2（egui-wgpu，默认 wgpu 渲染器）/ wgpu 30.0.1（与 S2 同一份）/ app-single-instance 0.1.x / dark-light 1.x / egui_extras（DataTable）；既有：cosmic-text 0.19、rusqlite、rquickjs

**Storage**: SQLite（tome-store 既有；书架/进度/设置/书源注册表同库，不引第二份存储）

**Testing**: cargo test（workspace 双形态：default + `--no-default-features`）；GUI 逻辑以「视图状态机 + 消息协议」纯 Rust 可测形态落地，像素级验证复用 S2 快照体系；三平台 CI（windows-gnu / macos / ubuntu-lavapipe）

**Target Platform**: Windows 10+ / macOS 12+ / 现代 Linux（X11/Wayland；Wayland 单实例抢焦降级 RequestUserAttention）

**Project Type**: desktop-app（单窗口双视图）

**Performance Goals**: SC-001 冷启动 ≤1.5s；SC-002 首屏 ≤400/800ms；SC-005 书架 1000 册 60fps；SC-006 调整 ≤150ms

**Constraints**: 体积增量可审计（基线裁决允许跨越，size-report.md 续账）；MASTER.md 合规 100%；干净室红线；引擎层零自然语言

**Scale/Scope**: 1 个新 crate（tome-app，~25 文件）+ 2 个接缝 adapter + 既有守卫适配；4 用户故事 / FR 28 / SC 12

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 判定 | 依据 |
|---|---|---|
| I. 兼容性优先（书源） | ✅ | FR-012~016 全部经 S1 书源管线既有语义；无自研规则 |
| II. 干净室纪律 | ✅ | 交互语义依 spec + MASTER.md 独立推导；调研未读上游源码（R1-R3 均为本仓库与公开文档） |
| III. 测试先行 | ✅ | Implementation Strategy 的切片全部 TDD 微步骤排序（/speckit-tasks 强制约束） |
| IV. 简单性与单向依赖 | ✅（一处**计划内修复**） | tome-app 只依赖 tome-core 接缝；**BookRepository/SourceEngine 无生产实现**属 S1 遗留缺口，本计划补 adapter（R4）——修复后铁律完整成立 |
| V. 安全无例外 | ✅ | 无新网络面；书源/JS 沙箱形态不变；单实例 named pipe 为本机语义 |
| VI. 原子提交与主干开发 | ✅ | 每任务独立 commit（TDD 微步骤）；main 直接工作 |
| VII. 不审查不合并 | ✅ | PM 验收关卡与 CI 全绿为合并前提 |
| VIII. 可读性与显式契约 | ✅ | contracts/ui.md 显式登记视图状态机/键盘表/文案表规则 |
| 体积纪律 | ✅（基线已裁决允许跨越） | 增量记账 size-report.md；eframe/winit 增量属 S3 必然成本 |
| 平台承诺 | ✅ | 三平台 CI；Wayland 降级已记录 |
| 范围纪律 | ✅ | 严格对齐 spec「明确排除」（D1/D2/D3/S4） |

**高风险决策（R6）已批准（2026-10-11 用户）**：`[profile.release] panic = "abort"` → `"unwind"`（spec Edge Case「引擎 panic 兜底捕获」的前提；unwind 表体积代价实测后计入 size-report.md）。UI 字体决策（R7）同日批准：铬文本走平台 UI 字体链，不内嵌。

## Dependency Graph（实施顺序自底向上）

```
tome-core 接缝（既有）+ theme 门面（既有）
    │
    ├── tome-store: BookRepository adapter（新）
    ├── tome-source: SourceEngine adapter（新）
    │
    ├── tome-reader: from_shared_device 构造（新，render feature 下）
    │
    └── tome-app（新 crate）
            ├── engine/   引擎线程 + 消息协议（依赖接缝 + tome-reader 排版面）
            ├── render/   PaintCallback + 字形暂存 + 纹理缓存（依赖 tome-reader 渲染面）
            ├── state/    书架/进度/设置（经引擎线程到 adapter）
            ├── views/    书架/阅读/抽屉/设置（依赖 state + render + theme_bridge）
            ├── widgets/  自绘组件（依赖 theme_bridge）
            ├── keys.rs + i18n.rs + theme_bridge.rs
            └── main.rs   单实例 + ViewportBuilder + App 启动
```

守卫适配（dependency_graph.rs 排除名单 / AGENTS.md）在收口切片。

## Project Structure

### Documentation (this feature)

```text
specs/003-app-shell/
├── plan.md                    # 本文件
├── research.md                # Phase 0 汇总（决策 R1-R10）
├── research-eframe.md         # R1 专题：eframe/wgpu/单实例/键盘
├── research-api-surface.md    # R2 专题：S1/S2 API 消费面测绘
├── research-design-system.md  # R3 专题：设计系统约束与 egui 落点
├── data-model.md              # Phase 1：实体与应用状态机
├── contracts/
│   └── ui.md                  # Phase 1：视图契约/键盘表/文案表规则
├── quickstart.md              # Phase 1：验证走查
├── checklists/
│   └── requirements.md        # 规格质量（已存在）
└── tasks.md                   # /speckit-tasks 产出
```

### Source Code (repository root)

```text
crates/
├── tome-store/src/
│   └── book_repository.rs     # [新] impl tome_core::BookRepository（BookDao/TraceDao/MiscDao 之上）
├── tome-source/src/
│   └── source_engine.rs       # [新] impl tome_core::SourceEngine（search/info/toc/content/change_source 自由函数之上）
├── tome-reader/src/render/
│   └── pipeline.rs            # [改] 新增 from_shared_device（egui device 注入；headless 路径不动）
└── tome-app/                  # [新 crate]
    ├── Cargo.toml             # eframe 0.36.2 钉版 + feature 门控（render 同 tome-cli 形态）
    └── src/
        ├── main.rs            # 薄壳：单实例 → ViewportBuilder → eframe::run_native（ui+logic trait）
        ├── app.rs             # TomeApp：视图状态机（Shelf/Reading + 抽屉/设置 overlay）、消息分发
        ├── engine/
        │   ├── mod.rs         # 引擎线程句柄（EngineHandle：send/collect）
        │   ├── thread.rs      # 线程循环：Paginator/store/source 独占；catch_unwind 边界
        │   └── messages.rs    # EngineRequest/EngineEvent（全部引擎交互的消息协议）
        ├── render/
        │   ├── callback.rs    # PaintCallback 三阶段（prepare/finish_prepare/paint）
        │   ├── glyph_stage.rs # UI 线程字形暂存 + atlas（egui device 上，SwashCache）
        │   └── texture.rs     # 纹理缓存 + 静态帧复用（内容版本号失效才重渲染）
        ├── views/
        │   ├── shelf.rs       # 书架视图（虚拟化网格/过滤/排序/空态/进度卡片）
        │   ├── reading.rs     # 阅读视图（版心/页脚/加载态/错误态/章间接续）
        │   ├── drawer.rs      # 覆盖式抽屉（目录/书签双页签）
        │   └── settings.rs    # 设置视图（四组默认 + 快捷键表展示）
        ├── state/
        │   ├── shelf.rs       # 书架状态（ShelfEntry 列表/过滤/排序，引擎事件驱动更新）
        │   ├── progress.rs    # 进度记忆 + 节流落盘调度
        │   └── settings.rs    # UserSettings 读写（经引擎线程到 MiscDao/配置注入）
        ├── widgets/
        │   ├── book_card.rs   # 书卡（封面/标题/作者/进度）
        │   ├── focus.rs       # 焦点环自绘（2px+2px offset，键盘来源才显示）
        │   ├── stepper.rs     # 字号/行高步进器（MASTER gap：无 Slider，用步进器）
        │   ├── badge.rs       # 状态徽章（按 R7 规则重建，不照抄 preview）
        │   ├── skeleton.rs    # 加载骨架（循环动效，MOTION 规则）
        │   └── empty.rs       # 空态（引导动作）
        ├── keys.rs            # 快捷键表 + 键盘分发（消费键事件）
        ├── i18n.rs            # 外壳文案表（zh 主；引擎错误 id → 文案映射）
        └── theme_bridge.rs    # tome_core::theme（Day/Night）→ egui Visuals/Style/字体注册
```

**Structure Decision**: 单一新 crate `tome-app` + 两个既有 crate 的 adapter 文件 + tome-reader 一处构造扩展。tome-app 内部按「引擎/渲染/视图/状态/组件」分层，文件小而专（每个 <300 行预期）；视图状态机与消息协议为纯 Rust 值类型（headless 可测）。tome-app 的 render 路径 feature 门控与 tome-cli 同形态（workspace `--no-default-features` 门不红）。

## Implementation Strategy

### 垂直切片划分

| 切片 | 端到端功能路径 | 对应故事 |
|---|---|---|
| **P0：S3-0 spike + 外壳基座** | eframe 0.36.2 窗口 → PaintCallback 内跑通 S2 渲染管线（同 device）→ 三平台 CI 实证 + 引擎线程骨架 + theme_bridge + 单实例 | 基座（S3-0 门） |
| **P1：书架与本地书阅读闭环** | BookRepository adapter → 书架视图 → 本地导入 → 打开 → 阅读视图（S2 渲染嵌入）→ 进度记忆与恢复 | US1（FR-001~005/006/009/020） |
| **P2：阅读调整与抽屉书签** | 五组调整（锚点保持）→ 抽屉目录/书签 → 章间接续 → 窗口几何适配 | US2（FR-006b/007/007b/008 系列/011） |
| **P3：书源获取闭环** | SourceEngine adapter → 书源导入 → 搜索 → 详情 → 加架 → 正文加载 → 换源 | US3（FR-012~016） |
| **P4：键盘设置与收口** | 键盘表 + 焦点环 → 设置视图与持久化 → 文案表审计 → 错误/空态全覆盖 → 体积报告 → MASTER 合规检查表 → 守卫/AGENTS.md | US4（FR-017~019/021~023 + 收口） |

### XS-XL 工作量预估

- **P0 = L**（spike 三平台 + 基座骨架）→ 拆为：spike（M：eframe 窗口 + from_shared_device + PaintCallback 跑通 + CI 三平台）、引擎线程与消息协议（M）、theme_bridge + 字体注册（M）、单实例 + ViewportBuilder（S）
- **P1 = L** → 拆为：BookRepository adapter（M）、书架状态与视图（M）、本地导入路径（M）、阅读视图嵌入与打开恢复（M）、进度节流落盘（S）
- **P2 = M** → 五组调整 + 锚点断言（M）、抽屉双页签（M）、章间接续 + 几何适配（S）
- **P3 = L** → SourceEngine adapter（M）、导入三径（S）、搜索与详情（M）、正文加载与缓存复用（M）、换源（S）
- **P4 = M** → 键盘/焦点（M）、设置视图（S）、收口（文案审计/体积报告/MASTER 检查表/守卫，M）

无 XL。每片以「可工作、可独立验证」为界：P1 后「能读本地书」，P3 后「能搜书读书」。

### 检查点规划

1. **S3-0 门（P0 末）**：spike 三平台 CI 绿（PaintCallback 渲染 S2 产出帧）+ 基座四命令双守卫绿。**未过 → 启动 fallback 路径 B（须用户批准）**。
2. **检查点（P1 末）**：US1 端到端走查（quickstart §1）+ SC-001/002/003 实测 + 双形态测试绿。
3. **检查点（P2 末）**：US2 锚点保持断言全绿（复用 002 SC-004/005b 判定形态于 GUI 消息层）+ SC-006 实测。
4. **检查点（P3 末）**：US3 闭环走查（合成书源）+ SC-004/007 实测。
5. **S3 完成门（P4 末）**：SC-005/008/009/010/011/012 全验 + MASTER 合规检查表签字 + 体积报告 + CI 全绿。

高风险任务排前：spike（P0 首任务）> 引擎线程消息协议（P0）> BookGrid 虚拟化（P1）> 覆盖式 Drawer（P2）。

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 补 BookRepository/SourceEngine 生产 adapter | 架构铁律要求 tome-app 只经 tome-core 接缝消费；接缝无生产实现外壳拿不到书架与书源动作（S1 遗留缺口） | 放宽铁律直连 tome-store/tome-source（违 AGENTS.md）；facade crate（多一跳无收益） |
| `panic = "abort"` → `"unwind"`（已批准 2026-10-11） | spec Edge Case 要求外壳兜底引擎 panic；abort 下 catch_unwind 在 release 不可能 | 进程守护重启（改变故障语义且实现更重）；引擎子进程（IPC 全量化，过重） |
