# Research: S3 应用外壳的 eframe/egui 技术调研

**Date**: 2026-10-11 | **输入**: `crates/tome-reader`（S2 wgpu 30 离屏渲染栈）、`Cargo.toml` / `Cargo.lock`（版本实解值）、`specs/003-app-shell/spec.md`
**用途**: 回答 plan.md 前置的三个问题——Q1 eframe 钉版与 wgpu 30 兼容性、Q2 S2 渲染嵌入 egui 的路径选型、Q3 桌面工程化要件（单实例/键盘/AccessKit/窗口配置/省电帧模型）。
**方法**: 一手出处优先——egui 官方仓库四个 CHANGELOG（egui / eframe / egui-wgpu / egui-winit）逐版本取数；docs.rs 的 crate 清单与源码（0.36.2 实版）核对 API 形态；crates.io API 查单实例 crate 版本史。本仓库 `Cargo.lock`/`pipeline.rs`/`offscreen.rs` 行号均已实读。

**版本坐标（先行结论）**：当前最新稳定 egui/eframe = **0.36.2（2026-09-08）**，其 wgpu 依赖 = **"30.0"**；tome-reader 钉 `wgpu = "30.0"`，`Cargo.lock:3461-3464` 实解 **30.0.1**——**两者 semver 相容，cargo 会合并为单份 wgpu 构建**。0.37.0 尚在开发（CHANGELOG 标 UNRELEASED，彩色 emoji），不作为钉版对象。

---

## Q1 · eframe/egui 版本钉版与 wgpu 兼容性

### Decision

**钉 `eframe = "0.36.2"`（egui 0.36.2、egui-wgpu 0.36.2、winit 0.30.13）。** 这是唯一与 wgpu 30 同 major 的 egui 版本线，与 S2 渲染栈零裂缝共存。

### Rationale

**egui 版本 ↔ wgpu 依赖对照**（逐条出自 [egui-wgpu CHANGELOG](https://github.com/emilk/egui/blob/main/crates/egui-wgpu/CHANGELOG.md)，0.36.2 另经 [docs.rs 上的 crate 清单](https://docs.rs/crate/egui-wgpu/0.36.2/source/Cargo.toml)复核 `[dependencies.wgpu] version = "30.0"`）：

| egui/eframe | 发布 | wgpu |
|---|---|---|
| **0.36.0 / 0.36.1 / 0.36.2** | 2026-08-05 / 08-07 / 09-08 | **30**（0.36.0 #8289 升级） |
| 0.35.0 | 2026-06-25 | 29（未再升） |
| 0.34.0–0.34.3 | 2026-03-26 ~ 05-27 | 28→29（0.34.0 #7853/#7990，0.34.2 钉 29.0.1） |
| 0.33.0–0.33.3 | 2025-10-09 ~ 12-11 | 26→27（#7540/#7580） |
| 0.32.0–0.32.3 | 2025-07-10 ~ 09-12 | 25（#6744） |
| 0.31.x | 2025-02-04 | 24（#5610） |

配套事实（均影响 S3 写法）：

- **MSRV**：egui 0.36 全系 `rust-version = "1.95"`（[0.36.0 #8348](https://github.com/emilk/egui/blob/main/CHANGELOG.md)），tome 工具链 1.99 / 声明 MSRV 1.98，有余量。
- **wgpu 已是 eframe 默认渲染器**：自 eframe 0.34.0（#7615，[eframe CHANGELOG](https://github.com/emilk/egui/blob/main/crates/eframe/CHANGELOG.md)）；glow 退为可选 feature。走 wgpu 不需要额外开关。
- **`App` trait 形态在 0.34.0 已换**（#7775）：`App::update` 被拆为必需方法 `fn ui(&mut self, ui: &mut Ui, frame: &mut Frame)` + 可选 `fn logic(&mut self, ctx, frame)`（[docs.rs App 0.36.2](https://docs.rs/eframe/0.36.2/eframe/trait.App.html)）。**S3 代码必须按新形态写**；网上 0.33 及以前的示例（`fn update(&mut self, ctx, frame)`）不再编译。
- **feature 并集**：tome-reader 的 wgpu 是 `default-features = false` + `["std","wgsl","dx12","metal","vulkan","gles"]`（`crates/tome-reader/Cargo.toml:58-65`）；eframe 默认的 `wgpu` feature = `wgpu_no_default_features` + `egui-wgpu/default`，后者含 `wgpu/default` + `wgpu/webgl`（[eframe 0.36.2 清单 features 表](https://docs.rs/crate/eframe/0.36.2/source/Cargo.toml)）。同 major 下 cargo 做 feature 并集后仍是**一份** wgpu 30.0.x。若要按体积纪律裁到与 S2 完全对齐的面，用 `eframe = { default-features = false, features = ["accesskit","default_fonts","wgpu_no_default_features","x11","wayland"] }`（eframe 0.34.0 起专门提供 `wgpu_no_default_features` 裁剪口，#7700）。
- **同版本兜底手段**：`egui-wgpu` 根部 `pub use wgpu`（[lib.rs 源码 L19](https://docs.rs/egui-wgpu/0.36.2/src/egui_wgpu/lib.rs.html)），`eframe::egui_wgpu::wgpu` 可直接引用——万一未来版本漂移，S3 侧可经 re-export 拿 egui 的那份 wgpu 类型。

**「两个 wgpu 主版本同进程共存」的答案**：能编译、能运行，但只适合做逃生舱。cargo 允许同一依赖图并存多个 semver 不兼容版本（wgpu 30 与 wgpu 27 会成为两个独立 crate 实例），各自建 `Instance`/`Device`/`Queue`、互不感知；两家的类型不互通（编译期就挡住混用），**唯一合法的桥是 CPU 侧字节**（即 Q2 路径 B 的 readback→上传）。代价：① 二进制里两份 wgpu 代码（wgpu 是全依赖树最大包之一，与 <20MB 体积基线直接冲突）；② 双驱动上下文、双份显存驻留；③ 任何「共享 device」的优化（路径 A/C）都不可能。minor 级差异不会发生此问题（同一 major 内 cargo 自动统一到兼容的最新版）。

### Alternatives considered

- 钉 0.35.x + 把 S2 降级到 wgpu 29：否决——改动 S2 已验收的渲染层去迁就 UI 框架，方向反了；且 0.36 已稳定两个月。
- 双 wgpu 共存（eframe 用旧 wgpu、S2 保持 30）：技术上成立，代价见上；仅在「未来 eframe 升级滞后于 S2 所需 wgpu」时作为临时逃生舱。
- eframe 退 glow 后端、S2 独用 wgpu 30：同样完全解耦，但丢掉 A 路径（glow 后端的 callback 是另一套 API），且多引入 OpenGL 栈；不如直接等 egui 版本跟上。

### 出处

[egui-wgpu CHANGELOG](https://github.com/emilk/egui/blob/main/crates/egui-wgpu/CHANGELOG.md) · [egui-wgpu 0.36.2 Cargo.toml](https://docs.rs/crate/egui-wgpu/0.36.2/source/Cargo.toml) · [eframe 0.36.2 Cargo.toml](https://docs.rs/crate/eframe/0.36.2/source/Cargo.toml) · [egui CHANGELOG](https://github.com/emilk/egui/blob/main/CHANGELOG.md) · [eframe CHANGELOG](https://github.com/emilk/egui/blob/main/crates/eframe/CHANGELOG.md) · [egui releases](https://github.com/emilk/egui/releases) · 本仓库 `Cargo.lock:3461`、`crates/tome-reader/Cargo.toml:58`

---

## Q2 · S2 离屏渲染嵌入 egui 的路径选型

### Decision

**主路径 A（`egui_wgpu` PaintCallback，同一 device/queue 内嵌渲染）；路径 B（自持有 wgpu → readback → `TextureHandle`）保留为 fallback**——B 是 S2 headless 快照路径的既有形态，代码已在，切换成本仅损失翻页动效期性能、不损失正确性。不采用路径 C。

### Rationale

**路径 A：PaintCallback + CallbackTrait（共享 device，直绘 egui 帧）**

接入点 API 形态（0.36.2 源码逐行核对，官方示例 [custom3d_wgpu.rs](https://github.com/emilk/egui/blob/main/crates/egui_demo_app/src/apps/custom3d_wgpu.rs)）：

1. **建资源（启动一次）**：`eframe::CreationContext::wgpu_render_state` → `egui_wgpu::RenderState { instance, adapter, device, queue, target_format, renderer, surface_config }`（全 pub，[lib.rs L107-137](https://docs.rs/egui-wgpu/0.36.2/src/egui_wgpu/lib.rs.html)）。自研 pipeline 的 fragment target 用 `target_format`。GPU 资源（pipeline/bind group/buffer）存入 `render_state.renderer.write().callback_resources.insert(...)`（TypeMap，生命周期跟随 egui `Renderer`——这是官方示例放置 pipeline 的位置，因为 callback 要求 `'static`）。
2. **每帧**：`let (rect, response) = ui.allocate_exact_size(...)` 后 `ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, MyCallback {...}))`（[renderer.rs L31-42](https://docs.rs/egui-wgpu/0.36.2/src/egui_wgpu/renderer.rs.html)）。
3. **三阶段回调**（[CallbackTrait](https://docs.rs/egui-wgpu/latest/egui_wgpu/trait.CallbackTrait.html)）：`prepare(device, queue, screen_descriptor, egui_encoder, resources) -> Vec<CommandBuffer>`（上传 uniform/顶点/字形 atlas，可直接用 egui 的 encoder 或返回自有 command buffer）→ `finish_prepare`（可选，所有 callback 之后一次）→ `paint(info: PaintCallbackInfo, render_pass: &mut RenderPass<'static>, resources)`（在 egui 主 render pass 上 set_pipeline/draw；`info.viewport` 给物理像素视口）。eframe 保证主 command buffer 与全部 callback 返回的 buffer 单次 submit。

对 tome-reader 的适配面（实读代码后的清单）：

- `RenderPipeline::new` 目前自建 Instance→adapter→device→queue（`crates/tome-reader/src/render/pipeline.rs:248-302`）。需新增 `from_shared_device(device, queue, target_format)` 构造——wgpu 的 `Device`/`Queue` 是 Arc 语义的可 Clone 句柄，借用零成本；headless 构造保留（快照测试不依赖窗口）。
- 离屏格式 `OFFSCREEN_FORMAT = Rgba8Unorm`（`offscreen.rs:37`）是像素快照「字节可复现」的决策（模块文档 L20-24）；共享 device 路径不 readback，格式参数化为 `target_format` 即可，两条路径并存、headless 快照语义不变。
- 字形 atlas/glyph cache/批次缓冲全部建到共享 device 上；S2 的翻页动效（`render/animate.rs PageTurn`）在 `prepare` 里推进、在 `paint` 里直绘，零拷贝。

性能：无 readback、无纹理上传，S2 绘制与 egui UI 合成在同一个 render pass / 同一次 submit。三条路径中唯一「动效期也无额外成本」的。

**路径 B：自持有 wgpu → readback → `egui::ColorImage` → `TextureHandle`**

- S2 现状即此形态（`offscreen.rs`：copy_texture_to_buffer → map_async → poll，已处理 `bytes_per_row` 256 对齐坑）。egui 侧 `ctx.load_texture(name, ImageData, options)` 建纹理（[Context::load_texture 文档](https://docs.rs/egui/0.36.2/egui/struct.Context.html)，明确「每张图只调一次」）、内容变化时 `TextureHandle::set` 增量更新、`ui.image()` 显示。
- 版本完全解耦（wgpu major 裂缝的唯一合法桥），且是 headless CI 的既有路径——**无论 A 是否成功，B 的代码都已存在**。
- 性能量级（工程估算，[UNCERTAIN] 需 S3 spike 实测钉数）：1080p RGBA = 7.9 MiB/帧。GPU→CPU readback 在 dGPU 上走 PCIe（4.0 x16 理论 32 GB/s，实际有效量级 5–15 GB/s）约 0.6–1.6 ms，iGPU 统一内存更快；CPU→GPU `queue.write_texture` 上传同量级。合计每个**内容变化帧**约 1–3 ms 额外开销 + 8MB memcpy。阅读器 99% 的帧是静态的（静态帧只显示缓存纹理，零成本）；成本集中在翻页/滚动/改排版参数的瞬间与动效期间。60fps 翻页动效每帧 +1–3ms 仍在 16.7ms 预算内，但叠加 S2 自身渲染后余量明显小于 A。

**路径 C：`WgpuSetup::Existing` 共享 device（S2 建栈、eframe 借用）**

- API 实存：`NativeOptions::wgpu_options`（`egui_wgpu::WgpuConfiguration`）`.wgpu_setup = WgpuSetup::Existing(WgpuSetupExisting { instance, adapter, device, queue })`（[WgpuSetup 文档](https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/enum.WgpuSetup.html) / [WgpuSetupExisting](https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/struct.WgpuSetupExisting.html)；0.30.0 #5319 引入，0.36.2 源码 `RenderState::create` 分支确认）。
- 与 A 的本质差别只在**谁建 device**：A 是 eframe 建、S2 经 `wgpu_render_state` 借；C 是 app 建、eframe 借。耦合点（同 wgpu major）与渲染路径（仍走 A 的 PaintCallback 或自建 surface 通道）与 A 完全相同。
- 不采用的理由：S2 的 device 诉求（`PowerPreference::LowPower` + `Features::empty()` + `downlevel_defaults()`，`pipeline.rs:282-289`）与 egui 的 CreateNew 默认配置没有冲突——egui 对 limits/features 要求极低。C 把「device 必须同时满足两家」的责任从 eframe 挪到 app，失败面变大而无增量收益。A 的适配器/power 策略如需干预，`WgpuSetupCreateNew` 的 `native_adapter_selector`/`device_descriptor` 回调已够用。

### Alternatives considered

（即三条路径互斥比较，见上；补充一条）egui 自带文本渲染替代 S2：不在考虑范围——spec 明确排版正确性是命门，阅读区必须复用 S2 管线。

### 出处

[CallbackTrait 文档](https://docs.rs/egui-wgpu/latest/egui_wgpu/trait.CallbackTrait.html) · [egui-wgpu renderer.rs 源码（Callback::new_paint_callback）](https://docs.rs/egui-wgpu/0.36.2/src/egui_wgpu/renderer.rs.html) · [egui-wgpu lib.rs 源码（RenderState/WgpuConfiguration）](https://docs.rs/egui-wgpu/0.36.2/src/egui_wgpu/lib.rs.html) · [官方 custom3d_wgpu 示例](https://github.com/emilk/egui/blob/main/crates/egui_demo_app/src/apps/custom3d_wgpu.rs) · [WgpuSetup](https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/enum.WgpuSetup.html) / [WgpuSetupExisting](https://docs.rs/egui-wgpu/0.36.2/egui_wgpu/struct.WgpuSetupExisting.html) · [egui::PaintCallback](https://docs.rs/egui/0.36.2/egui/struct.PaintCallback.html) · [Context::load_texture](https://docs.rs/egui/0.36.2/egui/struct.Context.html) · 本仓库 `pipeline.rs:248-302`、`offscreen.rs:1-37`

---

## Q3 · eframe 桌面工程化要件现状

### Decision

单实例用 **`app-single-instance`**（备胎 `single-instance` 0.3.3 + 自建 IPC）；键盘走「阅读区可聚焦 Response + `consume_key` 消费」模式；AccessKit 保持默认开（惰性激活，正文可及性需 S3 自行喂节点）；窗口配置全走 `ViewportBuilder`/`ViewportCommand`；省电靠 eframe 天生的 reactive 模型 + `request_repaint` 族。

### Rationale（按要件逐项）

**1. 单实例激活（第二实例唤起已有窗口）**

eframe **无内置**（eframe CHANGELOG 全量检索无任何 single-instance 条目；[winit#3964](https://github.com/rust-windowing/winit/issues/3964) 官方立场也是「用 single-instance 类 crate + 自行通知原实例」）。生态两个候选：

| crate | 版本/活跃度 | 机制 | 平台覆盖 |
|---|---|---|---|
| `app-single-instance` | 0.1.x，2026-02 仍更新 | 第二实例经 named pipe(Win)/UDS(mac/Linux) 发唤醒信号后自行退出；主实例回调或 `check_show()` 轮询；README 直接给 eframe 完整例子 | Win/mac/Linux(X11)；**Wayland 抢焦被协议禁止** → 降级 `ViewportCommand::RequestUserAttention`（其 README 明示） |
| `single-instance` | 0.3.3（2021-12，85 行单文件，稳定） | 只检测（Win 命名 mutex / Linux abstract UDS / macOS flock），**不含跨实例通知** | Win/mac/Linux |

唤起三件套（两案通用）：`ctx.send_viewport_cmd(ViewportCommand::Visible(true))` + `Minimized(false)` + `Focus`；`app-single-instance` 例子里 `ctx.request_repaint()` 唤醒沉睡的 UI 线程再 `check_show()`。推荐 app-single-instance（信号-唤起闭环现成、API 与 eframe 对齐）；顾虑其年轻（0.1）则退 single-instance + 自建 IPC（命名管道/UDS 发一字节即可）。`docs.rs`：[app-single-instance](https://docs.rs/crate/app-single-instance/latest) · [single-instance](https://docs.rs/single-instance/latest/single_instance/)；版本史经 crates.io API 核实（single-instance max_stable 0.3.3 @ 2021-12-16）。

**2. 键盘焦点与 Tab 导航**

- 能力水位：Tab/Shift+Tab 焦点循环为长期内置；**方向键移动焦点自 0.23.0**（2023-09-27，#3272「Change focused widget with arrow keys」，[egui CHANGELOG](https://github.com/emilk/egui/blob/main/CHANGELOG.md)）；0.25.0 严格化修饰键匹配（#3769：多按 Shift/Alt 会匹配失败，键位表须写精确 `Modifiers`）；0.35.0 把修饰键本身纳入 `egui::Key`（#8127）并重做 IME（#8083）。
- 阅读区按键模式：阅读区 `ui.allocate_exact_size(size, Sense::click())` 拿 Response，`response.request_focus()`；然后 `ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowRight))` / `Key::ArrowLeft` / `Key::PageUp` / `Key::PageDown` / `Key::Space`——**consume 是关键**，否则 egui 的焦点导航会把方向键同时用于移动控件焦点。`ctx.wants_keyboard_input()`（[Context 文档](https://docs.rs/egui/0.36.2/egui/struct.Context.html)）用于判「当前是否有 TextEdit 在收字」，有则不抢键。兜底：`App::raw_input_hook(ctx, raw_input)`（[App trait 文档](https://docs.rs/eframe/0.36.2/eframe/trait.App.html)）可在 egui 处理前拦截/注入事件（全局快捷键的最后手段）。

**3. AccessKit（屏幕阅读器）**

- 默认**开**：eframe 0.36.2 default features 首项即 `accesskit`（[crate 清单](https://docs.rs/crate/eframe/0.36.2/source/Cargo.toml)）；egui 0.34.0 起 accesskit 从 feature 改为恒依赖（#7701）。
- 运行时**惰性激活**：辅助技术（AT）连接前不生成树更新（`Context` 的 accesskit 节点访问器「Returns None if accesskit is off」；accesskit crate 的 `ActivationHandler`/`DeactivationHandler` trait 即按需激活机制，[docs.rs/accesskit](https://docs.rs/accesskit/latest/accesskit/)）。常态开销可忽略。
- ⚠ 阅读区正文是 wgpu 自绘像素，AT 天然看不到——S3 需用 `UiBuilder::accessibility_parent` / 隐藏 Label / `Role`（0.37 起 `egui::Role` 替代 `WidgetType`）把正文字符区间喂成 accesskit 节点。这是 S3 的任务项，本调研只确认框架侧通道存在。

**4. 窗口图标/标题/最小尺寸/高分屏**

集中在 `NativeOptions::viewport`（`egui::ViewportBuilder`，[0.36.2 文档](https://docs.rs/egui/0.36.2/egui/viewport/struct.ViewportBuilder.html)，31 个 pub 字段 + builder 方法）：

- 标题 `with_title`、图标 `with_icon(Arc<IconData>)`（不设则是 egui 白 e 默认图标；`IconData` 需 RGBA 字节，可复用 assets 管线）、初始/最小/最大尺寸 `with_inner_size`/`with_min_inner_size`/`with_max_inner_size`、`with_app_id`（**双重作用**：eframe persistence 存储位置 key；Wayland 下关联 `.desktop` 图标分组）。
- 运行时改：`ctx.send_viewport_cmd(ViewportCommand::Title(...)/Icon(...)/MinInnerSize(...)/Focus/RequestUserAttention(...))`。
- 高分屏：winit scale factor → egui `pixels_per_point` 自动处理；应用内缩放用 `ctx.set_zoom_factor(...)`（用户设置项落点）。S2 渲染目标尺寸 = 逻辑 rect × `pixels_per_point`（PaintCallback 的 `PaintCallbackInfo.viewport` 直接给物理像素）。

**5. 帧驱动模型与省电**

eframe 原生即 **reactive（按需重绘）**：无输入事件、无 `request_repaint` 就睡死——阅读器静态页零 GPU/CPU 空转。配套机制（[Context 文档](https://docs.rs/egui/0.36.2/egui/struct.Context.html) + eframe CHANGELOG）：

- `Context::request_repaint()`：本帧后再画一帧（多次调用合并一次；非 UI 线程调用经 `set_request_repaint_callback` 唤醒，eframe 已配好）——翻页动效、进度刷新每帧调它。
- `Context::request_repaint_after(d)`：至多 d 后重绘（多次取最小值），文档明确其用途即省电场景——适合时钟、进度同步这类低频刷新。
- 最小化/遮挡优化：0.36.0 #8387「Never run an egui pass when nothing will be shown」+ `App::logic`（不可见时只跑逻辑不跑 UI pass，见 Q1 的 trait 形态）；0.36.2 #8398 修了等待重绘时的 CPU 空转。

### Alternatives considered

- 自研单实例（lockfile/命名 mutex + 命名管道）：可行但重复造 `app-single-instance` 已封装的三平台分支与 Wayland 降级处理；仅当该 crate 出现兼容问题再收回自研。
- 键盘用全局快捷键框架（如 `global-hotkey`）：阅读区翻页键是**应用内焦点语义**，不该占全局热键；`consume_key` 模式已够。
- 关 AccessKit 省体积：否决——它是惰性零成本且 egui 0.34 起恒依赖，裁剪只省依赖体积、无运行时收益；无障碍是阅读器该有的能力。

### 出处

[app-single-instance docs.rs](https://docs.rs/crate/app-single-instance/latest) · [single-instance docs.rs](https://docs.rs/single-instance/latest/single_instance/) · [winit#3964](https://github.com/rust-windowing/winit/issues/3964) · [egui CHANGELOG](https://github.com/emilk/egui/blob/main/CHANGELOG.md) · [eframe CHANGELOG](https://github.com/emilk/egui/blob/main/crates/eframe/CHANGELOG.md) · [egui-winit CHANGELOG](https://github.com/emilk/egui/blob/main/crates/egui-winit/CHANGELOG.md) · [eframe App trait 0.36.2](https://docs.rs/eframe/0.36.2/eframe/trait.App.html) · [egui Context 0.36.2](https://docs.rs/egui/0.36.2/egui/struct.Context.html) · [ViewportBuilder 0.36.2](https://docs.rs/egui/0.36.2/egui/viewport/struct.ViewportBuilder.html) · [accesskit docs.rs](https://docs.rs/accesskit/latest/accesskit/)

---

## 给 plan.md 的三行结论

1. **eframe 钉 0.36.2**（egui/egui-wgpu 0.36.2，winit 0.30.13，MSRV 1.95）——其 wgpu 依赖 "30.0" 与 tome-reader 实解的 wgpu 30.0.1 单份合并，无版本裂缝；注意 `App` trait 已是 0.34 的 `ui`+`logic` 新形态。
2. **集成走路径 A**：`egui_wgpu` PaintCallback/CallbackTrait，给 `tome-reader::RenderPipeline` 加 `from_shared_device(device, queue, target_format)` 构造，挂到 `CreationContext::wgpu_render_state` 的共享 device/queue 上直绘；路径 B（readback → `TextureHandle`）保留为 fallback（headless 快照路径代码已在，切换不损失正确性）。
3. **单实例用 `app-single-instance`**：`main` 首行 `notify_if_running(APP_ID)` 命中即退；`start_primary` + 每帧 `check_show()` → `ViewportCommand::Visible(true)/Minimized(false)/Focus` 三件套，Wayland 降级 `RequestUserAttention`；备胎 `single-instance` 0.3.3 + 自建 IPC。
