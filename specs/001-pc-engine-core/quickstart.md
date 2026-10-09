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

- Rust `1.98.0`（stable-**x86_64-pc-windows-gnu**，本机）/ 三平台目标 Win10+/mac12+/现代 Linux
- 构建：`cargo build`；测试：`cargo test`；Lint：`cargo clippy -- -D warnings`；格式：`cargo fmt --check`
- **Windows 目标必须用 `x86_64-pc-windows-gnu`**（rquickjs 对 msvc 是 experimental）

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
│   └── tome-cli/     # CLI 验证器（差分测试入口）
└── tests/
    ├── corpus/       # 合成差分语料（入仓库，覆盖 FR-005b 清单）
    └── fixtures/     # 测试夹具（HTML/JSON/EPUB/TXT 样本）
```

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
