# T008 对照工具：Jsoup 侧 DOM 导出器

本目录是 `spikes/s0` 的 **T008（Jsoup ↔ html5ever DOM 差异量化）** 的 Java 对照臂。

## 干净室声明

本工具**只使用 Jsoup 的公开 API**（`Jsoup.parse` 与节点访问器），
**未读取、未移植**上游 `D:/Code/legado` 的任何 Kotlin 源码。
序列化约定由本项目自行定义（见 `JsoupDomDump.java` 的类注释与
`../src/dom_diff.rs` 的 `describe_node`）。

唯一参照上游之处是 **Jsoup 的版本号**（1.23.2），取自其构建元数据
`gradle/libs.versions.toml` 的依赖声明——这是构建元数据，不是实现源码。

## 依赖

| 项 | 版本 | 位置 |
|---|---|---|
| 便携 JDK | Temurin 21.0.12.1+1 | `D:\dev\toolchain\jdk\jdk-21.0.12.1+1`（免安装、不进 C 盘、不改系统环境变量） |
| Jsoup | 1.23.2 | `lib/jsoup-1.23.2.jar`（取自 `maven.aliyun.com/repository/central`） |

`lib/` 下的 jar 与编译产物（`.class`）**不入仓库**，已由仓库根 `.gitignore` 的两条
规则覆盖：`*.class` 与 `/spikes/s0/tools/lib/`。故 `mvn`/`javac` 的产物不会污染提交。
若首次克隆后要跑本工具，先按下方"依赖"获取 jar 与 JDK。

## 用法

```bash
export JAVA_HOME=/d/dev/toolchain/jdk/jdk-21.0.12.1+1

# 编译（-encoding UTF-8 必须：源码与输出含中文）
"$JAVA_HOME/bin/javac" -encoding UTF-8 -cp lib/jsoup-1.23.2.jar -d . JsoupDomDump.java

# 对语料目录批量导出（生成 <name>.jsoup.txt 与 <name>.jsoup.stats.txt）
"$JAVA_HOME/bin/java" -Dfile.encoding=UTF-8 -cp "lib/jsoup-1.23.2.jar;." JsoupDomDump ../corpus
```

> Windows 的 classpath 分隔符是 `;`（Linux/macOS 是 `:`）。
> 用**全路径**调 java，不改系统环境变量。

随后在 `spikes/s0` 下跑 Rust 侧的比对：

```bash
cargo run --quiet -- dom-diff corpus
```

## 输出约定（必须与 Rust 侧逐条一致）

- 每行 `<深度>|<类型字母>|<名称或内容>`，深度从 1 起（`Document` 不出行但占深度 0）
- 类型字母：`E`=元素 / `T`=文本 / `D`=数据（script/style 内容）/ `X`=注释 /
  `O`=文档类型 / `P`=处理指令 / `?`=未知类型（**不静默丢弃**——静默丢弃会把
  "两侧处理不同"粉饰成"一致"）
- 元素名小写；属性按**名**排序后以 `name=value` 用逗号相连（无属性则为空段）
- 文本内容折叠空白并去首尾；**折叠只针对 HTML 的 ASCII 空白**
  （tab/LF/FF/CR/space），**不含 U+00A0**（`&nbsp;` 是内容不是可折叠空白）
- **规范化后为空的文本节点不出行**，其数量写入 `.jsoup.stats.txt`

## 两侧约定为何必须同构

任何一侧偏离约定，测出的就是"约定差异"而非"解析差异"。
实测教训（详见 `../BASELINE.md` §4.4）：深度起点差 1、空文本节点未对齐，
曾分别把差异率虚高到 99% 与 90%，完全掩盖真实结构差异。

## 为什么单独产出 `.stats.txt`

空白文本节点保留策略是**真实的系统性差异**（Jsoup 保留、html5ever 多数字置丢弃），
但属"策略"而非"结构"——把它算进差异率会让该指标失去意义，
而丢弃它又会隐藏信息。故单列成指标：既不污染差异率，信息也不丢失。
