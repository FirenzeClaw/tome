# tome — AGENTS.md

<!-- AUTO:PROJECT-META -->
## 项目元信息

- 项目：tome（Legado 阅读器的桌面版原生重写）
- 技术栈：Rust（单体原生桌面应用，纯原生全栈重写）
- 类型：跨平台桌面应用（Rust workspace，当前 9 个 crate 已落地；S3/S4 的 `tome-app`/`tome-extra` 待建）
- 目标平台：Windows 10+ / macOS 12+ / 现代 Linux
- 初始化日期：2026-10-09
<!-- /AUTO:PROJECT-META -->

## 项目定位

**tome** —— 一个跨平台桌面阅读器，把 Android 版 Legado（Kotlin，约 19 万行）的功能转化为 **PC 端原生实现**。核心差异化价值是**书源生态**——书源（一份含选择规则与脚本片段的定义文件）除依赖内嵌浏览器的少数能力外，须开箱即用。

**与上游的关系：完整重构，非移植。** 本仓库是全新独立仓库，与上游**无代码血缘**——不 fork、不共享分支、不逐文件对应。上游仓库（`D:/Code/legado`）的角色已降为**功能基线索引参照**：用来回答"上游有这项能力吗、它的行为边界在哪"，而不是用来取材。

因此：

- **不追求结构对应**。上游的包划分、类型命名、模块切分对本案没有约束力；桌面场景该有的样子优先。
- **不追求逐功能对等**。上游受移动端限制而做的取舍（SAF 强制复制、Service 后台形态、单窗口交互）在桌面没有沿用理由；上游没有而桌面该有的能力（多库根目录、可移植模式、命令面板、大屏双栏）应当补上。
- **只追求行为兼容**，且范围明确限定在**书源与书源资产**上：既有书源开箱即用、既有规则语义一致。这是唯一需要向"上游怎么做的"看齐的地方。
- 项目的功能范围由 `specs/` 下的规格定义，**不以"上游有/没有"为判据**。

### 三条铁律（优先级不可调换）

1. **优先级次序**：兼容性 > 性能 ≈ 视效与体验完整性。冲突时按此排序裁决。其中"兼容性"特指**书源兼容性**（见上），而非与上游代码或结构的对应。
2. **体积**：`<20 MB` 是**目标基线，不是硬门槛**。为它牺牲兼容性或体验是不允许的；超出需记录原因。单个二进制的纯原生方案是达成前提（不含系统运行时依赖）。
3. **干净室纪律（合规红线）**：许可证为 MIT/Apache-2.0，因此**禁止阅读或逐行移植上游 Kotlin 实现**。上游代码只允许两种用途：**黑盒运行**以产生对照输出数据；**行为存在性证据**（据此构造测试用例）。语义须依据**书源作者的原始规则**与**公开文档**独立推导。

   > 上游仓库位于 `D:/Code/legado`，仅作只读参考数据源使用。需要对照行为时，运行它产生输入→输出数据文件，而非打开它的 `.kt` 源码。

<!-- AUTO:STRUCTURE -->
## 目录结构

```
tome/
├── crates/                    # Rust workspace（9 个 crate，见下）
├── specs/
│   ├── 001-pc-engine-core/    # 特性 001：S0 可行性验证 + S1 引擎内核
│   │   ├── spec.md            # 规格说明
│   │   └── checklists/        # 质量校验记录
│   └── 002-typesetting-renderer/  # 特性 002：S2 排版渲染层
│       ├── spec.md / plan.md / tasks.md
│       ├── quickstart.md      # 走查手册（命令逐条实跑）
│       ├── research.md / data-model.md / contracts/
│       ├── size-report.md     # 体积报告（铁律 2 的记录处）
│       ├── OPEN-QUESTIONS.md / SOURCES.md
│       └── checklists/        # 质量校验记录
├── assets/fonts/              # 内嵌字体（TomeSerifCN-subset.otf）+ 校验和 + OFL 许可
├── design-system/             # 设计系统（MASTER.md / tokens.css / MOTION.md + 截图脚本）
├── scripts/                   # 依赖守卫与度量脚本（reader 门控 / rquickjs / S2 栈体积）
├── spikes/                    # S0 / S2 先行验证工程（不属 workspace）
├── testdata/                  # baselines（入仓库）+ captures（抓取产物，gitignore）
├── tests/                     # corpus（合成语料差分，SC-002a）
├── tools/                     # ref-runner（上游黑盒参考运行器）+ subset-font.py
├── .specify/                  # speckit 流程脚手架（模板/脚本/宪法/内存）
├── .codegraph/                # 代码索引（自动维护，勿手改）
└── .kimi-tunnel/              # 项目内存（SQLite）
```

**workspace 结构**（已落地；各 crate 的 `Cargo.toml` description 为职责单一来源）：

```
crates/
├── tome-core/     # 接缝 trait + DTO + ErrorKind 错误分类（窄；无工具函数；零图形依赖；**零依赖**）
├── tome-net/      # HTTP、Cookie、代理、限速、字符集 + RequestSpec + 回放 adapter
├── tome-store/    # SQLite actor、迁移器、DAO、Pack 容器、图片缓存、配置注入
├── tome-js/       # QuickJS 沙箱 + 宿主能力注入 trait + 兼容前导脚本
├── tome-rule/     # 规则 DSL：分派/切分/CSS/JSONPath(+shim)/XPath(自研)/正则(双引擎)/替换链
├── tome-source/   # 书源管线：搜索/发现/详情/目录/正文/净化/换源/源检查
├── tome-local/    # TXT/EPUB/MOBI(分阶段) 解析、库根扫描、TOC 识别
├── tome-reader/   # [S2] 排版引擎 + 渲染 + 动效（详见下方 reader 子结构）
└── tome-cli/      # CLI 验证器（search/info/toc/content/run/verify/local/data）+ 差分测试入口
```

`tome-reader` 的模块划分（`crates/tome-reader/src/`）：

```
├── lib.rs / error.rs / font_setup.rs / theme.rs / precalc.rs   # 入口、错误、字体、主题、预计算
├── text/      # 段落模型与文本模型（整形输入的规范化形态）
├── layout/    # 整形与断行：engine / linebox / paragraph_mode / style
├── rules/     # 中文排版规则：cjk / justify / mixed / squeeze
├── paginate/  # 分页：booksem（书页语义）/ config / page / spread（跨页）
├── scroll/    # 滚动模式：flow / map / placeholder / relayout / switch / throttle
├── render/    # wgpu 自绘：pipeline / atlas / glyph_atlas / glyph_cache / batch / offscreen /
│              #   fonts / image / animate / pixel_diff
├── trace/     # 命中测试与渲染轨迹：dict / hit / render
└── corpus.rs / corpus_eval.rs / footer.rs / keys.rs / record.rs / toolbar.rs
```

**架构铁律**：依赖方向单向——实现 crate 依赖 `tome-core`，core 不依赖任何实现（箭头指向被依赖方）：`{tome-rule, tome-js, tome-net, tome-store, tome-local, tome-reader} → tome-core`，`tome-rule → tome-js`，`tome-store → tome-net`（仅依赖其 `HttpLogSink` trait），`tome-source → {tome-rule, tome-js, tome-net, tome-store}`，`tome-local → {tome-store, tome-rule}`；`tome-reader → tome-core` **且仅此一条**（排版不感知缓存与抓取——正文唯一来源是 `ContentProvider` 接缝，由组合根注入）。`tome-cli` 是唯一允许依赖全部实现 crate 的**组合根**（搜索/验证器入口）。`tome-app`/`tome-extra`（S3/S4，未落地）只依赖 `tome-core` 的接缝 trait，**永不直接构造** `tome-source`/`tome-store` 的内部类型。crate 边界即架构——用编译期强制，不靠约定。另两条接缝决策：宿主能力 trait（NetHost/StoreHost/CryptoHost）与共享值对象 RequestSpec/Response **定义在 tome-core**（接缝中枢），tome-net/tome-store 实现它们，tome-js 经组合根被注入——一切实现 crate 只依赖 tome-core，星型无环；tome-rule 求值产出 RequestSpec 不亲自发请求，管线是唯一发请求处。详见 `specs/001-pc-engine-core/plan.md` 依赖图与 `contracts/seams.md`。

**依赖门控**：`tome-reader` 的 `render` feature（默认开）收拢全部图形依赖（wgpu / swash / etagere），关闭后依赖树零图形包——`bash scripts/check-reader-features.sh` 对此有断言，是 FR-052「能在无窗口系统上完整运行」的机器可验证形式。
<!-- /AUTO:STRUCTURE -->

<!-- AUTO:BUILD -->
## 构建与运行

### 构建与检查（均已在仓库根实跑验证）

| 用途 | 命令 |
|---|---|
| 构建 | `cargo build --workspace` |
| 测试 | `cargo test --workspace` |
| Lint（CI 同命令） | `cargo clippy --workspace --all-targets -- -D warnings` |
| 格式检查（CI 同命令） | `cargo fmt --all -- --check` |
| 文档构建（T099 门） | `cargo doc --no-deps --workspace` |
| reader 门控守卫 | `bash scripts/check-reader-features.sh`（断言 headless 依赖树零图形包 + `--self-test` 自检） |
| rquickjs 依赖守卫 | `bash scripts/check-rquickjs-features.sh`（`--self-test` 校验守卫自身有效） |

### 运行 CLI

`cargo run -p tome-cli -- <子命令>`（二进制名 `tome-cli`；直接跑 `--help` 打全部命令面）。

全局选项：`--format json|human`、`--record/--replay/--offline`（录制回放）、`--data-dir <path>`、`--config <path>`、`--lang zh|en`、`-h/--help`。
退出码：`0` 成功 / `1` 业务错误 / `2` 用法错误 / `3` 不支持能力。

常用子命令：

- `source import <file|--url <u>|--stdin> [--data-dir <dir>]` —— 导入书源并落注册表
- `search <source-id> <keyword> [--source-file <f>] [--format json|human]` —— 搜索
- `info|toc|content <source-id> <url>` —— 详情 / 目录 / 正文（净化后）
- `run <source-id> --action <a> [--input <s>]` —— 通用执行入口
- `run --rule <规则> --input <文本> --trace` —— 规则求值轨迹（FR-037）
- `local import|toc|scan` —— 本地书（TXT/EPUB/MOBI）
- `data export|import|clear-cache <chapters|images>|delete` —— 备份与清理
- `source list|check|group|enable|disable|groups`、`keyword list|add|remove` —— 书源与关键词管理
- `verify --corpus <dir> [--report <file>]` —— 合成语料差分（SC-002a，≥95%）

S2 排版渲染子命令（选项与契约见 `specs/002-typesetting-renderer/contracts/cli.md`）：

- `layout --text <file> [--mode paginated|scroll]` —— 排版单章正文（本阶段入口是 `--text`）
- `layout --chapter <book>#<n>` —— 书架取章形式（**WIP**，需 S3 书架接线；当前退出码 3）
- `layout-anchor --char <offset> [--target-mode …]` —— 报告某字符锚点落在哪一行/哪一屏
- `render --out <png> --page <n>|--viewport <n> [--theme day|night]` —— 渲染一页/一屏到 PNG
- `verify-layout --corpus <dir> [--report <file>]` —— 排版合成语料差分（无硬门槛，如实报告）
- `fonts` —— 列出内嵌字体与回退链

### 差分测试与对照数据

| 入口 | 命令 | 说明 |
|---|---|---|
| 合成语料差分 | `cargo run -p tome-cli -- verify --corpus tests/corpus/` | 入仓库语料；不达标退出码 1（SC-002a） |
| 真实书源基线 | `cargo test -p tome-cli --test capture_real_baseline -- --ignored --nocapture` | 重建 `testdata/baselines/fingerprint.json`（基线入仓库） |
| 基线回归门 | `cargo run -p tome-cli -- verify --real-sources testdata/captures/sources --golden testdata/baselines` | 回退则退出码 1（SC-002b） |
| 上游参考运行器 | `bash tools/ref-runner/ref-runner.sh probe\|jvm\|gradle-test` | **只黑盒运行，不读源码**（见 `tools/ref-runner/CLEANROOM.md`） |
| 对照产物落盘 | `cargo test -p tome-cli --test capture_synthetic -- --ignored --nocapture` | 写 `testdata/captures/`（**不入仓库**） |

注意 `testdata/captures/` 被 `.gitignore` 排除，**干净检出下不存在**——需要它时先跑上面两条 `--ignored` 测试生成；`testdata/baselines/fingerprint.json` 入仓库，始终可用。

### CI

- `.github/workflows/ci.yml`：windows-gnu / macos / ubuntu matrix——fmt、clippy、rquickjs 守卫、reader 门控守卫、测试、依赖图断言（`cargo build -p tome-core` 独立 + `cargo test -p tome-cli --test dependency_graph` 星型无环 + 传递依赖无图形栈）
- `.github/workflows/release.yml`：三平台 release 产物 + 体积基线监控

**工具链现状（2026-10-10 P9 复核）**：`cargo 1.99.0` / `rustc 1.99.0`（stable-x86_64-pc-windows-gnu，由 `rust-toolchain.toml` 钉版 channel=stable）。MSRV 声明 `rust-version = "1.98"` 尚未经 1.98 实测。

**测试规模（2026-10-11 S2 Phase 9 实测，含 S2 排版层）**：`cargo test --workspace -j 2` **3110 passed / 0 failed / 17 ignored**（108 个测试二进制合计）；headless 形态 `cargo test --workspace --no-default-features -j 2` **2958 passed / 0 failed / 9 ignored**（差值即 `render` feature 及其测试）。S2 前的基线为 2320 passed——增量即 `tome-reader` 与其 CLI 集成测试。默认形态与 headless 双形态均须绿。

**性能基线**：SC-003/SC-004/SC-005 实测数据见 `specs/001-pc-engine-core/perf-baseline.md`；S2 体积报告见 `specs/002-typesetting-renderer/size-report.md`。
<!-- /AUTO:BUILD -->

## 生产级护栏（本项目特有）

以下来自规格与设计共识，违反即阻断合并：

- **`MUST NOT` 遥测与崩溃上报**：除用户明确触发的书源抓取外，不得产生任何网络外发（spec §FR-041 / §SC-012）。
- **引擎不得依赖图形环境**：`tome-core` 及以下必须能在无窗口系统上完整运行，可被无界面调用（spec §FR-036 / §SC-011）。
- **阅读痕迹禁止存渲染坐标**：高亮/批注/书签一律以**字符区间**表达（spec §FR-028）。这是改字号后不错位的前提。
- **缓存写入顺序不可颠倒**：内容追加落盘并 fsync **先于** 索引事务提交。反过来会产生悬空引用（spec §FR-030）。
- **脚本沙箱必须设限**：内存上限 + 可中断。Rust 中杀线程是未定义行为，绝不可用线程终止来兜住死循环脚本（spec §FR-007 / §SC-013）。
- **不支持的能力要抛可捕获的结构化错误**，而非让 API 未定义。书源的 `try/catch` 降级逻辑依赖这一点（spec §FR-006）。
- **错误须可本地化**：引擎产出带标识的错误分类，文案映射留给上层（spec §FR-044 / §SC-016）。引擎内禁止硬编码面向用户的自然语言。

## 工具优先级

1. **代码结构理解优先用 codegraph**（`codegraph_explore` / `codegraph_search` / `codegraph_node`），不可用时降级为 grep/glob/Read
2. **会话开始先读项目内存**：`memory_get(namespace="project/meta")` 与 `memory_get(namespace="project/decisions")`，只读不写；新的架构决策由负责编排的会话写入
3. 文件操作用专用工具（Read/Write/Edit/Glob/Grep），不滥用 shell

<!-- AUTO:CONVENTIONS -->
## 编码约定

### 命名

- 函数：动宾结构（`fetch_source_by_id`、`is_valid_encoding`）
- 变量：名词短语（`active_source_count`），禁缩写、禁 1-2 字符名
- crate 前缀统一 `tome-`；模块 snake_case；类型 PascalCase；常量 UPPER_SNAKE_CASE

### 控制流

- Guard clauses 优先，嵌套 ≤3 层
- 复杂条件提取为布尔变量
- 函数超过 50 行考虑拆分；magic number 改为命名常量

### 注释

核心原则：**代码说 how，注释说 why**。注释不是代码的翻译，而是代码无法自解释的决策、约束和上下文。

#### 功能标注

完成状态标注按层级执行（2026-10-10 PM 仲裁，对齐 Phase 1-3 实践）：

- **模块级必须标注**：每个模块的 docstring 首行带状态标记
- **函数级 `[DONE]` 仅标关键入口**（crate 的主要 pub API），不要求逐函数
- **`[WIP]`/`[WORKAROUND]`/`[STUB]`/`[UNCERTAIN]` 函数级强制**——未完成、临时、占位、存疑的必须标，不得隐匿

| 标记 | 含义 | 示例 |
|------|------|------|
| `[DONE]` | 已实现并通过验收 | `/// [DONE] 按关键词搜索单个书源` |
| `[WIP]` | 开发中，未完成 | `// [WIP] 待接入连接池复用` |
| `[WORKAROUND]` | 临时方案，需后续替换 | `// [WORKAROUND] 绕过上游方言差异` |
| `[STUB]` | 占位实现 | `/// [STUB] 待实现` |
| `[UNCERTAIN]` | 行为不确定，待验证 | `// [UNCERTAIN] 并发下是否幂等` |

#### 注释语言

- **注释使用中文**，标识符保留英文
- 模块级 docstring：说明模块职责 + 关键入口函数一览
- 函数 docstring：参数/返回值/副作用/错误条件

#### 必须加注释的位置（强制）

1. **每条规格需求的落点** — 标注对应的 spec 编号，如 `// spec §FR-030 崩溃安全：内容落盘先于索引提交`
2. **复杂条件分支（≥3 个条件）** — 说明每种走向的后果
3. **与上游行为对齐处** — 凡是为兼容上游语义而写的非常规代码，必须注明**依据来源**（书源规则原文或公开文档），**不得写"照抄上游某文件"**（违反干净室纪律）
4. **任何硬编码常量/阈值** — 标 `[WORKAROUND]` 并说明替换计划
5. **不安全代码块** — 必须写 SAFETY 注释说明前置条件

#### 禁止事项

- ❌ 注释写 what（代码本身已说明的）
- ❌ TODO 注释（待办进 issue 或任务列表）
- ❌ 在注释中引用或摘录上游 Kotlin 源码（干净室红线）

### 类型与错误处理

- 显式类型签名；禁止生产路径上的 `unwrap()` / `expect()` / `panic!()`
- 每个 crate 拥有自己的错误类型（thiserror）；在接缝处映射为 tome-core 的小型 `ErrorKind` 分类枚举（网络/规则/不支持能力/内容/存储/配置），携带可本地化的错误标识与上下文。**不做全局上帝错误枚举**——core 变更不应波及全员
- 每个外部调用必须处理错误并给出有意义的信息；第三方响应不可信，边界处必须校验
- 优先用类型系统表达不变量（如"只有缓存命中的章节才持有内容"），而非运行时检查

### 测试

- TDD：**没有失败的测试就不写生产代码**
- 一致性验证走 `tests/` 下的差分测试（合成用例入仓库；真实书源对照数据不入仓库）
- 排版层（S2）用像素快照测试；引擎层用输入→输出断言
- 修 bug 必须先写重现 bug 的失败测试
<!-- /AUTO:CONVENTIONS -->

## Git 约定

- 提交格式：`<type>: <描述>`，type ∈ `feat`/`fix`/`refactor`/`test`/`docs`/`chore`，消息解释 why
- 原子提交：一次提交只做一件逻辑完整的事；理想 ~100 行，~300 可接受，~1000 必须拆分
- 格式化与逻辑、重构与功能分开提交
- 分支：`feature/` `fix/` `chore/` `refactor/` 前缀，短命分支（1-3 天），main 始终可部署
- 提交前：`git diff --staged` 自查 → 查密钥（password/secret/api_key/token）→ 跑测试

### 本仓库必须 gitignore 的项

- `.kimi-tunnel/`（项目内存，含会话数据）
- `.kimi-code/`（agent 配置，可能含凭据 —— `specify init` 明确提示）
- `.codegraph/`（本地索引）
- 真实书源的对照数据与抓取产物（第三方资产，不入仓库）

## 安全红线

- 边界校验所有外部输入（书源规则、响应内容、本地文件均不可信）
- **脚本沙箱是安全边界**：JS 执行须设内存上限与执行中断；脚本的文件访问限制在数据目录内，越界须拒绝并报告（spec §边缘情况）
- 禁止提交密钥；禁止在日志记录敏感数据（Cookie、认证头须脱敏）
- 书源抓取涉及第三方站点：遵守书源声明的请求间隔，不得绕过限速
- 外部通信一律 HTTPS

## 质量门

- 没有失败的测试就不写生产代码（TDD 铁律）
- 不审查不合并：五轴审查（正确性/可读性/架构/安全/性能），三级标注（[必须修复]/[建议修改]/[仅供参考]）
- **干净室审查是独立一轴**：合并前确认新增代码无上游 Kotlin 移植痕迹、注释无上游源码引用
- 详细原则见 `.specify/memory/constitution.md`

## 规格驱动流程

本项目使用 speckit 流程。当前特性目录由 `.specify/feature.json` 指定（当前值：`specs/002-typesetting-renderer`）。

- 特性 001 规格：`specs/001-pc-engine-core/spec.md`（S0 + S1）
- 特性 002 规格：`specs/002-typesetting-renderer/spec.md`（S2 排版渲染层）
- 下游：`speckit-clarify` → `speckit-plan` → `speckit-tasks` → `speckit-implement`
- S3（应用外壳）/ S4（可选模块）各自独立规格，不在特性 001/002 范围内

### 子项目顺序

`S0 可行性验证` → `S1 引擎内核` → `S2 排版渲染` → `S3 应用外壳` → `S4 可选模块`
