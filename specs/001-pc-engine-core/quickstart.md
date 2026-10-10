# Quickstart: tome 引擎（S1）开发上手

**面向**：对代码库零上下文的工程师 / 新会话 AI 代理。读完此文件即可开始。

## 这个项目是什么

`tome` —— Legado 阅读器的**桌面端原生完整重写**（Rust，单体桌面应用，三平台）。当前特性 `001-pc-engine-core` 交付 **S0 技术验证 spike + S1 引擎内核**。

S1 的产物是**无界面的引擎库 + 命令行验证器**，不是用户可用的应用（界面在 S2/S3）。

## 先读这三个文件（按序）

1. `.specify/memory/constitution.md` — **宪章**，含 3 条 NON-NEGOTIABLE（兼容性优先 / 干净室纪律 / 测试先行）。**干净室纪律是合规红线：禁止阅读上游 Kotlin 源码**。
2. `specs/001-pc-engine-core/spec.md` — 需求（48 FR + 19 SC + 6 用户故事）
3. `AGENTS.md` — 编码约定、架构铁律、生产级护栏

## 工具链

- Rust `1.99.0`（stable-**x86_64-pc-windows-gnu**，本机，由 `rust-toolchain.toml` 钉版 channel=stable）/ 三平台目标 Win10+/mac12+/现代 Linux。MSRV 声明 `rust-version = "1.98"` 尚未经 1.98 实测
- 构建：`cargo build --workspace`；测试：`cargo test --workspace`；Lint：`cargo clippy --workspace --all-targets -- -D warnings`；格式：`cargo fmt --all -- --check`；文档：`cargo doc --no-deps --workspace`（零警告是 T099 的门）
- **Windows 目标必须用 `x86_64-pc-windows-gnu`**（rquickjs 对 msvc 是 experimental）

## 五分钟跑通（用干净数据目录）

以下命令逐条实跑通过（T098 验证，2026-10-10）。关键是**每条都带 `--data-dir`**
指向临时目录——默认路径会写用户真实数据目录，验证时不该污染它。

```bash
DD=/tmp/tome-qs          # 干净数据目录（首次运行时为空）

# 1. 导入一份书源（自备 JSON；格式见 spec.md 的书源字段说明）
cargo run -p tome-cli -- --data-dir $DD source import my-source.json

# 2. 确认已落注册表
cargo run -p tome-cli -- --data-dir $DD source list

# 3. 搜索（需要能访问书源声明的站点）
cargo run -p tome-cli -- --data-dir $DD search <source-id> "关键词"

# 4. 目录与正文
cargo run -p tome-cli -- --data-dir $DD toc <source-id> <book-url>
cargo run -p tome-cli -- --data-dir $DD content <source-id> <chapter-url>
```

**不想联网也能验证全链路**：先 `--record <dir>` 录一次，之后 `--replay <dir>
--offline` 重放。回放是**真离线**——把站点关掉再跑，结果照常来自录制。

## 差分测试入口（兼容性门）

```bash
# 合成语料（入仓库，覆盖 FR-005b 能力清单）——不达标退出码 1
cargo run -p tome-cli -- verify --corpus tests/corpus/

# 上游参考运行器：只黑盒运行，绝不读源码（详见 tools/ref-runner/CLEANROOM.md）
bash tools/ref-runner/ref-runner.sh probe
```

真实书源的对照数据**不入仓库**（`.gitignore` 排除 `testdata/captures/`）——
干净检出下该目录不存在，需要时按 `AGENTS.md`「差分测试与对照数据」一节的
`--ignored` 测试重新生成。


## 硬性护栏（违反即阻断合并）

1. **干净室**：禁止阅读/移植上游 Kotlin 源码；上游只可黑盒运行产生对照数据 / 作行为存在性证据。注释/提交/文档不得引用上游源码。
2. **rquickjs**：锁 `>= 0.14.0`；**禁止 `rust-alloc`/`allocator` feature**（会让内存上限失效）；显式设栈上限；触发过 OOM 的 runtime 丢弃不复用。
3. **无遥测**：除用户明确触发的抓取外，零网络外发。
4. **引擎无图形依赖**：可无界面运行。
5. **阅读痕迹存字符区间**，不存渲染坐标。
6. **缓存写入顺序**：内容落盘 + fsync **先于**索引事务提交。
7. **crate 边界即架构**：单向依赖；上层只依赖 tome-core 接缝 trait。

## 目录结构

```
tome/
├── Cargo.toml                 # workspace（LTO=fat / panic=abort / strip）
├── crates/
│   ├── tome-core/    # 接缝 trait + DTO + ErrorKind（窄，无工具函数）
│   ├── tome-rule/    # 规则 DSL：分派/切分/CSS/JSONPath/XPath(自研)/正则/替换链
│   ├── tome-js/      # QuickJS 沙箱 + 宿主能力注入 trait + compat prelude
│   ├── tome-net/     # HTTP(reqwest+rustls)/Cookie/代理/限速/字符集 + RequestSpec
│   ├── tome-store/   # SQLite + 迁移 + DAO + Pack 容器 + 图片缓存 + 配置 TOML
│   ├── tome-source/  # 书源管线：搜索/发现/详情/目录/正文/净化/换源/源检查
│   ├── tome-local/   # TXT/EPUB/MOBI 解析 + 库根扫描
│   ├── tome-cli/     # CLI 验证器（差分测试入口）
│   └── tome-reader/  # [S2 排版层] 骨架已在，实现由 S2 规格推进——引擎不依赖它
├── tests/
│   └── corpus/       # 合成差分语料（入仓库，覆盖 FR-005b 清单）
├── testdata/
│   ├── baselines/    # 回归基线指纹（入仓库，不含第三方内容）
│   └── captures/     # 对照数据与合成站点（**gitignore**，干净检出下不存在）
└── tools/ref-runner/ # 上游参考运行器（只黑盒运行，见其 CLEANROOM.md）
```

各 crate 另有自己的 `tests/` 目录（集成测试就地放）；测试夹具随用它的
crate 走，没有全局 `tests/fixtures/`。


## 实现顺序（依赖图自底向上）

```
tome-core（接缝/DTO/错误）
  ├─ tome-net ──┐
  ├─ tome-store │
  └─ tome-js ───┴─→ tome-rule → tome-source ─┐
                        │                     ├─→ tome-cli
                        └─→ tome-local ───────┘
```

先建基础（core/net/store/js），再规则（rule），再管线（source），再本地书（local），最后 CLI。

## TDD 铁律

**没有失败的测试就不写生产代码。** 每个任务：写失败测试 → 看失败 → 最少实现 → 看通过 → commit。一致性验证用差分测试（合成语料入仓库；真实书源对照数据不入仓库）。

## 参考资源

- `specs/001-pc-engine-core/research.md` — 5 项技术选型的验证结论（含 rquickjs 四缓解、XPath 自研理由、MOBI 分阶段、正则双引擎）
- `specs/001-pc-engine-core/contracts/` — seam 契约 + CLI 契约
- `specs/001-pc-engine-core/data-model.md` — 数据模型
- 上游参考运行器（产生差分对照数据）：`D:/Code/legado`（**只读运行，不读源码**）
