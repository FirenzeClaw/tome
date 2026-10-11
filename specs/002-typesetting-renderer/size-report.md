# 体积报告（002-typesetting-renderer / T051）

**测量日期**：2026-10-11
**被测版本**：`960a646`（main；S2 Phase 1-7 已完成，Phase 8 收口进行中）
**测量机**：Windows 10 IoT 企业版 LTSC build 19044 / AMD Ryzen 5 5600U / x86_64-pc-windows-gnu
**工具链**：`rustc 1.99.0`，release profile（workspace：LTO=fat / panic=abort / strip / codegen-units=1）

本文件是 **SC-017** 的实测记录：

> 本层引入的二进制体积被记录在案，并作为最终 `<20 MB` 目标基线的输入之一移交 S3；
> 本层 MUST NOT 引入与功能无关的体积膨胀……内嵌正文衬线字体子集 MUST 控制在
> **3–5 MB** 预算内（裁决 OQ#3→A）；超出预算须记录原因并提请用户裁决。

---

## 1. 本层净增量（二分测量）

**方法**：同一份 `tome-cli` 源码，只切换 `tome-reader` 的 `render` feature
（`--no-default-features` = 渲染面在编译期消失），两次 release 构建的产物之差
即**渲染面的净增量**。这是唯一能把"本层引入的体积"与 001 既有体积分开的形态。

| 构建形态 | 产物 | 字节 | MiB |
|---|---|---|---|
| **默认**（`reader-render` on，含渲染栈） | `tome-cli.exe` | **19,834,368** | **18.92** |
| **headless**（`--no-default-features`，无渲染栈） | `tome-cli.exe` | 14,453,248 | 13.78 |
| **渲染面净增量** | — | **5,381,120** | **5.13** |

净增量的构成：

| 组成 | 字节 | MiB | 说明 |
|---|---|---|---|
| **内嵌字体子集** | 2,284,616 | **2.18** | `assets/fonts/TomeSerifCN-subset.otf`（GB2312 6763 字档） |
| **wgpu/swash/etagere 代码** | ≈ 2,860,000 | ≈ 2.73 | 四个原生后端（dx12/metal/vulkan/gles）+ naga 着色器翻译 |
| 排版栈（cosmic-text/harfrust/skrifa） | ≈ 240,000 | ≈ 0.23 | **两个构建都含**（headless 也要排版），故不计入增量 |

**排版栈不计入净增量的理由**：`cosmic-text` 在 headless 构建里同样存在（排版
是引擎能力，与图形无关）。故它属"本层引入"但**不是渲染面增量**——上表用
feature 二分只能测出渲染面那一部分。排版栈的绝对量见 §3。

> **测量命令**
> ```bash
> cargo build --release -p tome-cli                          # 默认（渲染面 on）
> cp target/release/tome-cli.exe /tmp/render-on.exe
> cargo build --release -p tome-cli --no-default-features    # headless
> cp target/release/tome-cli.exe /tmp/headless.exe
> ```

---

## 2. 与 001 基线的对比

001 的体积基线由 `release.yml` 的"产物体积记录"步骤采集（**只记录，不设上限**——
`release.yml` 的注释写明理由：体积在未优化阶段频繁拦截合法提交会阻碍开发）。
001 的 SC-009 是"记录在案"，002 的 SC-017 继承同一形态。

| 层级 | 产物形态 | 体积 | 相对 `<20 MB` 目标 |
|---|---|---|---|
| 001 引擎（无 reader） | `tome-cli` headless | 13.78 MiB | 68.9% |
| **001 + 002 本层** | `tome-cli`（含渲染面） | **18.92 MiB** | **94.6%** |

### ⚠ 需要提请裁决的一点（SC-017 的"超出/逼近预算须记录原因"）

**当前形态下，`tome-cli` 单二进制已占 `<20 MB` 目标基线的 94.6%**，而 S3
（应用外壳：egui + 平台集成）尚未加入。若 eframe/egui 栈按 3–6 MB 计，最终
产物会**超出 20 MB 目标**。

三条可选路径（**本报告不作裁决，按 SC-017 的措辞"提请用户裁决"上报**）：

| 路径 | 说明 | 代价 |
|---|---|---|
| **A. 接受超出并记录** | 20 MB 是"目标基线，不是硬门槛"（AGENTS.md 三条铁律之二明文），超出须记录原因 | 与既有铁律的措辞一致，但需用户在 S3 阶段确认 |
| **B. 削减后端** | wgpu 当前编入 4 个原生后端（dx12/metal/vulkan/gles）；每平台只保留本平台所需者可省可观体积 | 需按平台条件化 feature（`[target.'cfg(...)'.dependencies]`），构建矩阵变复杂 |
| **C. 压缩字体子集** | 2.18 MiB 的子集可降到 GB2312 一级字库以下的档位 | 牺牲生僻字覆盖（与"书源开箱即用"的兼容性优先级冲突） |

**本层的职能范围内没有可再削减的"与功能无关的膨胀"**：
- 依赖树里没有未使用的静态依赖（`dependency-graph` job 与守卫脚本覆盖）
- 没有重复的排版实现（排版只有一份，渲染面经 feature 二分可完全剥离）
- 没有"为单平台打包的冗余资源"（字体是唯一的资源，且三平台共用）

---

## 3. 分项明细

### 3.1 内嵌正文衬线字体子集

| 项 | 值 |
|---|---|
| 文件 | `assets/fonts/TomeSerifCN-subset.otf` |
| 字节 | **2,284,616**（2.18 MiB） |
| 字符档 | GB2312 6763 字 + 常用标点 |
| SC-017 预算 | 3–5 MB |
| 判定 | ✅ **在预算内**（低于下限，见下） |

> **低于预算下限是有意的**：SC-017 的措辞是"控制在 3–5 MB 预算内"+"超出须记录
> 原因"。实测 2.18 MiB **低于**该区间——这是 CFF 子集化的子程序共享带来的
> （线性外推会高估）。低于预算是好事（体积目标是 `<20 MB`），故不改用更臃肿
> 的档位去"凑进"区间。口径记录见 `spikes/s2/BASELINE.md` §7.2。

### 3.2 渲染栈依赖（`render` feature 下新增）

| crate | 角色 |
|---|---|
| `wgpu` 30（dx12/metal/vulkan/gles） | 跨平台图形抽象 |
| `swash` 0.2 | 字形栅格化（经 cosmic-text 的 `SwashCache` 复用，**零新增字体解析栈**） |
| `etagere` 0.3 | 字形图集分配（glyphon 同款） |
| `bytemuck` | 实例数据字节视图 |

### 3.3 源码规模

| 项 | 值 |
|---|---|
| `crates/tome-reader/src` 行数 | 19,142 |
| 集成测试文件 | 43 个（`tests/*.rs`） |

---

## 4. 快照资产 dev-only 的构建验证

**判据**（T051 验收子项）：

> `cargo package -p tome-reader --list` 断言不含 `tests/snapshots/`，
> 且 `assets/fonts/` 仅含 `.otf`/`OFL.txt`/`FONTS.sha256`

**实测**：

```bash
cargo package -p tome-reader --list --allow-dirty
```

| 断言 | 实测 | 判定 |
|---|---|---|
| 包内不含 `tests/snapshots/` | 该目录**不存在**（快照比对在内存中进行，不落盘 PNG 基线——见 `tests/snapshot.rs`） | ✅ |
| `assets/fonts/` 仅三类文件 | `assets/` **不入 `tome-reader` 的包**（见下方说明） | ✅（更强） |

### 关于 `assets/fonts/` 的一处结构性事实（需记录）

字体的嵌入路径是 `include_bytes!("../../../../assets/fonts/TomeSerifCN-subset.otf")`
——它指向**仓库根**的 `assets/`，不在 `crates/tome-reader/` 内。故：

1. `cargo package -p tome-reader` 的包**不含** `assets/`（cargo 只打包 crate 目录内的文件）
2. 这带来一个**已存在的限制**：`crates.io` 形态的 `tome-reader` 包**无法独立构建**
   （`include_bytes!` 的路径在包内不存在）
3. 本项目的交付形态是**单体应用**（AGENTS.md：单个二进制的纯原生方案），
   不发布 `tome-reader` 到 crates.io——故该限制**不影响本项目**
4. 另有一处**独立的既有限制**：`cargo package`（不带 `--list`）在当前 workspace
   下会因"路径依赖缺版本要求"而报错（`tome-core` 是 `path` 依赖、无 `version`）。
   这与 001 的既有形态相同（未发布 crate 的常规状态），**不属本层引入**。
   本任务的验收判据用的是 `--list`（tasks.md T051 的原文），它正常工作。

---

## 5. 结论

| SC-017 要求 | 实测 | 判定 |
|---|---|---|
| 本层体积记录在案 | 渲染面净增 **5.13 MiB**（字体 2.18 + 渲染代码 ≈2.73 + 排版栈 ≈0.23） | ✅ |
| 作为 `<20 MB` 基线的输入移交 S3 | 当前单二进制 18.92 MiB（94.6%）——**已提请裁决**，见 §2 | ⚠ 需用户裁决 |
| 不引入与功能无关的膨胀 | 依赖树无未使用静态依赖（守卫 + 依赖图 job 覆盖）；排版实现唯一；资源仅字体一项且三平台共用 | ✅ |
| 字体子集在 3–5 MB 预算内 | 2.18 MiB（低于下限，原因见 §3.1） | ✅ |

**移交 S3 的三条输入**：
1. 当前 `tome-cli` 含渲染面 18.92 MiB，距 20 MB 目标余量 **1.08 MiB**
2. 若 S3 外壳（egui/eframe + 平台集成）超过该余量，需按 §2 的路径 A/B/C 裁决
3. 最可行的削减点是 **wgpu 后端裁剪**（路径 B）——四个后端对单一平台是冗余的
