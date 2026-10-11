# S3 应用外壳 — 验证走查（quickstart.md）

**Date**: 2026-10-11 | **Spec**: [spec.md](./spec.md)

S3 完成门的端到端走查脚本。每节对应一个切片的验收；全部人工可复现（GUI 部分）+ 机检命令（headless 部分）并记。前置：仓库根 `cargo build --workspace` 通过；`cargo run -p tome-app` 可启动。

## §0 S3-0 spike（P0 门）

1. `cargo run -p tome-app --bin tome-app-spike`（或 spike 示例目标）启动演示窗：egui 窗口内嵌一块由 S2 渲染管线（PaintCallback，同 device）绘制的排版帧。
2. CI：`阅读层快照` 三 job 全绿（spike 场景入快照矩阵）；三平台 build 绿。
3. 窗口关闭/重开正常；单实例：重复启动激活已有窗口而非新开。

## §1 书架与本地书闭环（P1 末检查点 / US1）

1. 空启动 → 空态引导可见；拖入 `tests/fixtures/chapter-zh.txt`（或任一 TXT）→ 书卡出现。**≤3 步**（SC-003）。
2. 点开书卡 → 首屏可读，计时 **≤400/800ms**（SC-002）。
3. 读到中部 → `Esc` 回书架（书卡显示进度）→ 重开 → **位置逐字符恢复**（SC-009）。
4. 冷启动计时：进程启动到书架可交互 **≤1.5s**（SC-001）。
5. （规模）生成 1000 册的测试库 → 滚动 + 标题过滤 **60fps**（SC-005，帧时间分布断言）。
6. 移除（确认）→ 书架消失，缓存按 S1 语义清理。

## §2 阅读调整与抽屉书签（P2 末检查点 / US2）

1. 阅读中 `Ctrl+=` 调大字号 → 新版心 **≤150ms** 呈现；调整前记住的那行字仍可见（SC-006）。
2. `Ctrl+M` 翻页→滚动 → 同一字符在视口内（锚点不变）。
3. 窗口拖宽过 1512px → 双页出现；拖窄 → 单页回归（contracts/ui.md §4「呈现不变量」第 3 条）。
4. `Ctrl+L` 昼夜切换 → 配色反转，页内容不变。
5. `Ctrl+B` 抽屉目录 → 跳第三章 → 章首可见；`Ctrl+D` 存书签 → 读两章 → `Ctrl+Shift+B` 跳回 → 书签位置可见；重启应用书签仍在。
6. 窗口尺寸连续拖动 → 版心实时适配，阅读位置不丢（FR-011）。

## §3 书源获取闭环（P3 末检查点 / US3）

1. 书源导入（文件）→ 成功；故意导入坏文件 → 结构化错误可见。
2. 搜索关键词 → 结果按源分组流式到达（首屏 ≤3s，SC-007）；中途取消 → 旧结果不再出现。
3. 详情 → 加入书架（全程 ≤4 步到可读，SC-004）→ 书架出现该书。
4. 打开 → 加载态 → 首章正文（净化后）可读。
5. 断网模拟当前源失效 → 错误带「重试/换源/返回」→ 换源 → 同位置继续阅读。

## §4 键盘、设置与收口（P4 末 / US4 / S3 完成门）

1. 全程不用鼠标完成「打开书→读两页→改字号→存书签→回书架」（SC-008 逐项核对表）。
2. 设置改主题为「跟随系统」→ OS 切深浅色 → 应用跟随；重启设置保持（SC-012）。
3. 空态/错误态/加载态逐视图走查（contracts/ui §1 全表）。
4. MASTER 合规检查表逐项签字（SC-011；含对比度 ≥4.5 抽测、鎏金/tertiary/border-strong 规则）。
5. 机检收尾：
   - `cargo test --workspace -j 2` 与 `cargo test --workspace --no-default-features -j 2` 全绿
   - `cargo clippy --workspace --all-targets -- -D warnings` 0 告警；`cargo fmt --all -- --check` 干净
   - `bash scripts/check-rquickjs-features.sh`、`bash scripts/check-reader-features.sh` PASS
   - `cargo doc --no-deps --workspace` 0 告警
   - 依赖图断言（tome-app 排除名单生效）绿
6. 体积：`size-report.md` 增补 S3 段（eframe/winit/wgpu 增量、unwind 表代价、UI 字体决策）落账。
7. `AGENTS.md` 目录结构与依赖图更新（tome-app 入图）。
