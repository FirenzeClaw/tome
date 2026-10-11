# S3 应用外壳 — Phase 0 调研汇总（research.md）

**Date**: 2026-10-11 | **Spec**: [spec.md](./spec.md)

三份专题调研已完成并落盘（本文件为其决策汇总，细节以专题文件为准）：

| 专题 | 文件 | 覆盖 |
|---|---|---|
| R1 eframe/egui 与 wgpu 集成 | [research-eframe.md](./research-eframe.md) | 版本钉版、三条嵌入路径、单实例/键盘/AccessKit/省电 |
| R2 S1/S2 公共 API 消费面 | [research-api-surface.md](./research-api-surface.md) | 全部接缝/DAO/管线入口（文件:行）、特征门控、线程模型 |
| R3 设计系统约束 | [research-design-system.md](./research-design-system.md) | token/组件/动效/焦点清单、21 条 gap、egui 落点映射 |

---

## R1（命门）：eframe 钉版与 wgpu 共存

**Decision**: eframe/egui **钉 0.36.2**（2026-09-08，当前最新稳定）。

**Rationale**: 其 `egui-wgpu` 依赖 `wgpu = "30.0"`，与 tome-reader 实解的 **wgpu 30.0.1**（`Cargo.lock:3461`）semver 相容，cargo 合并为单份构建——**无版本裂缝**。双 wgpu 主版本同进程虽可编译，但资源不互通、双倍体积与双驱动上下文，只配做逃生舱。

**注意**：0.34 起 `eframe::App` trait 从 `update` 拆为 `ui` + `logic` 两方法，网上旧示例（单 `update`）不再编译；0.34 起 wgpu 是默认渲染器（glow 退为可选）。

**Alternatives considered**: egui 0.35（wgpu 29，降级 S2，拒）/ 0.33（wgpu 26-27，同拒）/ 双 wgpu 实例（体积×2，仅逃生舱）。

## R2：S2 渲染嵌入 egui 的路径

**Decision**: **路径 A——`egui_wgpu::Callback`（PaintCallback）**，S2 渲染管线在 egui 的 wgpu device/queue 上执行。`tome-reader` 新增 `from_shared_device(device, queue, target_format)` 构造（现 `RenderPipeline::new` 自建 Instance，`pipeline.rs:248-302`）；headless 快照路径（Rgba8Unorm + readback）原样保留。**路径 B（离屏 readback → `egui::ColorImage` 纹理上传）为 fallback**，仅在 S3-0 spike 证明 A 在三平台不可行时启用，且须用户批准（每变化帧约 1–3ms 量级，静态帧零成本）。弃路径 C（`WgpuSetup::Existing`）：与 A 仅差 device 创建权归属，责任倒置无收益。

**Rationale**: 兼容性 > 性能 > 体验。A 零拷贝、同 device 资源直通；B 版本解耦但有 CPU 往返。API 形态已核实（`CreationContext::wgpu_render_state` → `callback_resources.insert` → `ui.painter().add(Callback::new_paint_callback(rect, cb))`，prepare/finish_prepare/paint 三阶段）。

## R3：单实例、键盘、AccessKit、省电

**Decision**: 单实例用 `app-single-instance` crate（named pipe/UDS 唤醒；Wayland 抢焦被协议禁止，降级 `RequestUserAttention`）。键盘：阅读区 `Response::request_focus` + `ctx.input_mut(|i| i.consume_key(...))` 消费翻页键；方向键焦点导航自 egui 0.23 内建。AccessKit 在 eframe 默认 features 内、AT 连接才惰性激活；自绘正文的可及性节点 S3 自行喂（本层只保 FR-017 键盘可达，专项审计不做——spec 假设节）。省电：eframe 原生 reactive 模型 + `request_repaint`/`request_repaint_after`，静态阅读帧零重绘。

## R4（阻塞缺口）：接缝 trait 无生产实现

**Decision**: **为 `BookRepository` 与 `SourceEngine` 补生产 adapter**——`crates/tome-store/src/book_repository.rs`（impl BookRepository over BookDao/TraceDao/MiscDao）与 `crates/tome-source/src/source_engine.rs`（impl SourceEngine over 既有自由函数）。架构铁律不破（tome-app 只依赖 tome-core 接缝）。

**Rationale**: 调研实锤两个接缝 trait 至今只有测试假体（`tome-core/tests/seam_contract.rs:635`）；真实能力在 tome-store DAO 与 tome-source 自由函数上。不补 adapter，外壳拿不到书架与书源动作。

**Alternatives considered**: 放宽铁律让 tome-app 直连 tome-store/tome-source（违反 AGENTS.md，拒）/ 新建 facade crate（多一跳无收益，拒）。

## R5：线程/调用模型

**Decision**: **引擎线程独占**——`Paginator`（持昂贵 FontSystem，全方法 `&mut self`）、store/source 的阻塞调用全部在一条专用引擎线程；UI 线程（egui）持 wgpu render state 与字形暂存/atlas，经消息通道（`EngineRequest`/`EngineEvent`）交互。排版结果（定位字形）由引擎线程产出、渲染在 UI 线程的 PaintCallback 内消费。

## R6：panic 策略变更（已批准 2026-10-11，用户于计划审查关卡）

**Decision（用户已批准）**: workspace `[profile.release]` 由 `panic = "abort"` 改为 **`panic = "unwind"`**，引擎线程边界 `catch_unwind` 兜底（spec Edge Case「引擎 panic 经外壳兜底捕获」的直接兑现）；unwind 表的体积代价实测后记入 size-report.md（体积基线已裁决允许跨越）。

**Rationale**: spec 要求捕获 panic，而 `panic = "abort"`（`Cargo.toml:43`）下 catch_unwind 在 release 不可能。001 选 abort 时尚无 GUI 进程；GUI 的健壮性要求高于 CLI。

**Alternatives considered**: 改 spec 语义为进程守护重启（退而求其次，须用户批准，不推荐）/ 引擎子进程（IPC 全量化，过重，拒）。

## R7：设计系统消费与缺陷处置

**Decision**: **消费 `tome-core::theme`（DayTheme/NightTheme 门面）**，禁止在 tome-app 重声明色值（MASTER §10）。四处已核实缺陷的处置规则：① 鎏金 `#B8862B`（对比度 2.20–2.95:1）只作填充/高光，**禁作文本前景**；② `--text-tertiary`（<4.5）禁用于必要信息文本（用 secondary 替代）；③ `--border-strong`（<3:1）不作输入框唯一可见边界（叠加 focus 环）；④ preview 的 badge/StatusPill 样式**不可照抄**（夜主题崩坏），按 egui 重建。SC-011 检查表断言对比度 **≥4.5** 而非精确相等（MASTER 声明值普遍偏保守）。

**「跟随系统」主题**：设计系统无此机制（tokens.css 无 `prefers-color-scheme`）——S3 以 OS 主题探测（winit `window.theme()` / `dark-light`）映射到 Day/Night token 实现，MASTER gap 记录在案。

**UI 字体**：界面铬文本（chrome）用**平台 UI 字体链**（Windows 雅黑 / macOS PingFang / Linux Noto Sans CJK）——桌面惯例（VSCode 等同款），阅读正文用内嵌 Tome Serif CN。UI 字体不内嵌（省 MB 级体积），三平台铬文本观感差异属可接受范围（阅读区不受影响）。

**Alternatives considered**: 内嵌 UI 无衬线子集（+1–2MB，三平台铬文本一致）——**用户 2026-10-11 批准平台链方案**，内嵌为已否备选。

## R8：egui 落点硬约束（实现期须知）

- `--shadow-lg` 的 spread = −12px 无法用 `epaint::Shadow`（u8）表达 → 近似值替代并记录偏差；`--r-full = 999` 超 u8 需 clamp。
- egui 无 `focus-visible` 概念 → 自绘 2px 环 + 2px offset + 输入来源判定（键盘才显示）。
- `egui::FontId` 无字重维度 → 400/500/600/650/700 按独立 `FontFamily` 注册。
- `cubic-bezier`/`--ease-spring` 过冲无求值器 → 自写插值；纸噪点（SVG data-URI + mix-blend-mode）→ 预生成噪点纹理。
- 自绘/半自绘组件估 **12–16 个**，四个风险点：BookGrid 虚拟化、覆盖式 Drawer、LoadingSkeleton 循环动效、DataTable（引 `egui_extras`）。
- 单双页阈值口径：MASTER 的 1432px 是**阅读区可用宽**；S3 阅读视图窗口宽阈值 = **1512px**（19px 字号，含 2×sp-10 内边距）。

## R9：守卫与断言的适配

- `crates/tome-cli/tests/dependency_graph.rs` 的 `ENGINE_CRATES`（`:146`）须把 `tome-app` 列入排除名单（与 tome-cli 同口径：组合根合法依赖 tome-reader 装配渲染）。
- `scripts/check-reader-features.sh` 是包作用域（不受影响）；`cargo test --workspace --no-default-features` 是 workspace 级门——tome-app 的渲染路径必须 feature 门控（同 tome-cli 形态）。
- AGENTS.md 目录结构与依赖图在收口任务更新（同 S2 T054 形态）。

## R10：i18n 文案表

**Decision**: tome-app **自建外壳文案表**（`crates/tome-app/src/i18n.rs`，zh 主）；引擎错误 id → 文案的映射表在外壳侧维护（tome-cli 的 i18n.rs 属 CLI，不共享、不下沉——UI 文案是外壳职责，引擎层零自然语言红线不变）。
