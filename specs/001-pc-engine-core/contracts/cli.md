# Contracts: CLI 验证器命令契约

**特性**: 001-pc-engine-core | **日期**: 2026-10-09 | **对象**: `tome` CLI（tome-cli crate）

命令行工具是 S1 的**独立验证方式**（FR-037）：不启动界面即可对给定书源执行各抓取动作并输出结构化结果，供自动化比对与书源调试。

## 全局约定

- 输出默认 **JSON**（`--format json`，结构化、可 diff），或 `--format human` 人类可读
- 退出码：`0` 成功；`1` 业务错误（源失效/规则错）；`2` 用法错误；`3` 不支持能力
- 错误输出到 stderr，含 `ErrorKind` 与可本地化标识
- 数据目录：`--data-dir <path>` 覆盖（默认可移植模式）；`--config <path>` 指定配置
- 网络：`--offline`（只用录制/缓存，不发真实请求）；`--replay <dir>`（回放录制的响应，差分测试用）；`--record <dir>`（录制响应）

## 命令

### 书源管理

```
tome source import <file|--url <u>|--stdin>   # 三渠道导入（FR-001），输出导入报告
tome source list [--group <g>] [--enabled]    # 列出书源
tome source check <id|--all>                  # 可用性检查（FR-011），输出可判定结论
```

### 抓取管线（核心验证面）

```
tome search <source-id> <keyword> [--record dir|--replay dir]
    # 多源/单源搜索，流式输出；JSON 数组，每项 {source, name, author, cover, intro, book_url}

tome info <source-id> <book-url>
    # 书籍详情：{name, author, intro, cover_url, toc_url, ...}

tome toc <source-id> <book-url>
    # 目录：[{index, title, url}...]

tome content <source-id> <chapter-url> [--no-purify]
    # 正文纯文本（默认已应用净化规则）；--no-purify 看原始

tome run <source-id> --action <search|info|toc|content> --input <...> [--trace]
    # 通用执行；--trace 输出规则求值轨迹（逐步中间值，FR-037 扩展，供调试器）
```

### 一致性验证（宪章 III 的落地）

```
tome verify --corpus <dir> [--report <file>]
    # 跑合成语料差分：逐条比对"引擎输出 vs 期望"，输出一致率 + 逐条 diff
    # 对应 SC-002(a)：合成用例通过率 ≥95%

tome verify --real-sources <dir> --golden <dir> [--report <file>]
    # 真实书源端到端对照（对照数据不入仓库，本地/受控环境）：对应 SC-002(b) 回归基线
```

### 本地书

```
tome local import <path> [--copy]         # 导入 TXT/EPUB/MOBI，输出解析出的目录与元数据
tome local toc <path>                      # 只输出目录识别结果（含用到的目录规则）
tome local scan <dir>                      # 库根扫描，列出可识别的书
```

### 数据

```
tome data export [--output <file>]        # 导出全部阅读数据（FR-033）
tome data import <file>                   # 导入
tome data clear-cache <chapters|images>   # 分别清理两种缓存（SC-017）
```

## 差分测试的接线

`--replay`/`--record` 让 CLI 在不联网的情况下确定性地重放抓取——这是差分测试（兼容性验证）确定性的来源。参考实现（上游 legado，只读运行产生对照数据）与本实现各自跑同一批输入，比对输出。**对照数据不入仓库**（.gitignore 已排除 `testdata/captures/`）。
