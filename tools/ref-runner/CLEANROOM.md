# 上游参考运行器 —— 干净室执行约定

> 本文件是 **tasks.md T084** 的硬性交付物，也是宪法原则 II（干净室纪律）
> 与 plan.md「干净室执行边界」的执行层落地。
>
> 它不是说明文档，而是**一份可被核对人签字确认的约定**：约定运行器怎么跑、
> 产物落盘在哪、以及在什么条件下才允许增加新的黑盒触达面。

---

## 1. 允许做的事（白名单）

对上游参考实现（`D:/Code/legado`，**只读**）只允许以下两类操作：

| 类别 | 允许内容 | 依据 |
|---|---|---|
| **黑盒运行** | 经由命令行 / HTTP / WebSocket / MCP 调用上游**已构建的可执行产物**（APK 装入模拟器、Gradle 测试任务、HTTP API），取得输入→输出对照数据 | tasks.md T084 正文：①「仅命令行/API 调用上游可执行产物」 |
| **构建元数据** | 读取构建元数据（`build.gradle` / `libs.versions.toml` / `settings.gradle` / workflow YAML / `api.md` 等**公开文档**），用于确定依赖版本、任务名、端口与接口形态 | P1-P7 既有实践（S0 的 `spikes/s0/tools/README.md` 已确立：Jsoup 版本号取自 `libs.versions.toml`，属构建元数据而非实现源码） |

### 唯一被显式授权的源码文件

**`D:/Code/legado/avd.bat` 与 `D:/Code/legado/avd.sh`** 允许阅读。

理由：它们是**运行器操作脚本**（内容为 `emulator -avd %1 -no-snapshot-load` 之类的
启动命令），不是应用实现。T084 需要知道"上游打算怎么跑模拟器"，而这个信息只在这
两个文件里。授权范围**严格限于这两个文件**，不因"它旁边还有其他脚本"而扩张。

> 本波实际读到的全部内容（`avd.bat` 54 字节 / `avd.sh` 34 字节）：
> `emulator -avd %1 -dns-server 8.8.8.8 -no-snapshot-load` 与
> `emulator -avd $1 -no-snapshot-load`。仅此两条启动命令，无其他信息。

## 2. 禁止做的事（红线）

- **禁止打开、阅读、检索上游任何 `.kt` / `.kts` / `.java` 源码文件的内容**。
  包括但不限于：`app/src/main/java/**`、`app/src/test/**`、`modules/**/src/**`。
- 禁止从上游源码**逐行移植**任何逻辑（许可证为 MIT/Apache-2.0，本项目为干净室重写）。
- 禁止在注释、文档或提交信息中**引用或摘录**上游 Kotlin 源码。
- 禁止把对照产物提交进仓库（见 §4）。

### 允许的"结构性"操作（明确豁免）

以下操作**不读取文件内容**，故不构成源码接触：

- 用 `find` / `ls` **按文件名**定位产物（例如"上游有没有一个已构建的 APK"）。
- 用 `grep` 检索**构建元数据与文档**（`.gradle` / `.toml` / `.yml` / `.md`）。
- 用 `git log` / `git remote` / `git tag` 查看仓库版本信息。

> 判据是**是否读取实现源码的内容**，而不是"是否碰到那个目录"。
> 本波的每一次此类操作都逐条记在 §6 的操作台账里，供核对人复核。

## 3. 运行器形态（本机实测结论）

### 3.1 本机探测结果（2026-10-10 实测）

| 项 | 结果 | 命令 |
|---|---|---|
| `adb` | **存在**（`D:\dev\toolchain\platform-tools`） | `which adb` |
| 连接设备 | **0 台** | `adb devices` |
| `emulator` / `sdkmanager` / `avdmanager` | **不存在** | `which` |
| `ANDROID_HOME` / `ANDROID_SDK_ROOT` / `ANDROID_AVD_HOME` | **均未设置** | `env` |
| AVD 镜像 | **不存在**（无 `*.avd`、无 `.android/avd`） | `find` |
| 已构建 APK | **不存在**（C: 与 D: 全盘搜索无结果） | `find -iname "*.apk"` |
| JDK | **存在**（Temurin 21.0.12.1+1，`D:\dev\toolchain\jdk`） | `java -version` |
| Gradle wrapper | 存在，直连下载 Gradle 发行版**失败**（TLS）；经代理可达 | `./gradlew --version` |

### 3.2 走模拟器路线需要的下载量（实测尺寸，非估算）

尺寸取自 Google 官方仓库清单 `dl.google.com/android/repository/repository2-3.xml`
与 `sys-img/google_apis/sys-img2-3.xml`（2026-10-10 拉取）：

| 组件 | 实测大小 |
|---|---|
| `platform-tools` | 8.6 MB |
| `platforms;android-36` | 62.8 MB |
| `build-tools;36.0.0` | ~56 MB |
| `cmdline-tools;latest` | ~148 MB |
| `emulator` | ~334 MB |
| `system-images;android-36;google_apis;x86_64` | **1807.6 MB** |
| Gradle 8.14.4 发行版 | ~150 MB |
| Gradle 依赖缓存（AGP+Kotlin+Room+cronet） | ~3–6 GB |
| 构建产物 | ~1–2 GB |
| AVD 数据 | ~2–4 GB |
| **合计** | **约 8–15 GB** |

**本机 D 盘剩余 14 GB**（`df -h /d`）。

按任务书「模拟器镜像/SDK 组件下载前先报体积并等 PM 确认」的纪律，
**本波未执行该下载**，故模拟器路线在本波处于**待 PM 裁决**状态（见 §5）。

### 3.3 本波采用的形态

运行器按「**先建骨架、按环境自动选臂**」实现：

| 臂 | 触发条件 | 说明 |
|---|---|---|
| `probe` | 总是可用 | 环境探测：报告哪条臂当前可用。**不接触上游**。 |
| `jvm` | 本机 JDK 可达 | 用 `javac` 直接编译并运行 **Jsoup 公开 API** 的规则求值 oracle，产出对照数据。**这是 S0 已确立的做法**（`spikes/s0/tools/JsoupDomDump.java` 先例），只依赖 Jsoup 公开 API，不接触上游源码。 |
| `gradle-test` | JDK + Gradle（经代理）可达 | 经上游的 Gradle 任务跑其 **JVM 单元测试**取对照输出。**执行的是上游产物的黑盒运行**，不读其源码。 |
| `emulator` | SDK + AVD 就位 | 启动 AVD、装 APK、经 `api.md` 记载的 HTTP/WS 接口驱动。**本波不可用**（§3.2）。 |

设计意图：**模拟器就位后无需改运行器**，只是多一条可用的臂。
这样 T084 的"接线"工作不会因为一次磁盘紧张而作废。

## 4. 产物落盘位置

| 产物 | 位置 | 是否入 git |
|---|---|---|
| 对照数据（上游输出） | `testdata/captures/` | **否**（`.gitignore` 已排除；属第三方资产） |
| 真实书源样本 | `testdata/captures/sources/` | **否** |
| T086 回归基线 | 含第三方站点内容 → `testdata/captures/`；**仅特征指纹** → `testdata/baselines/` | 视数据性质（见 T086） |
| 运行器脚本与本文档 | `tools/ref-runner/` | **是** |

**核对方式**：`git status` 干净时，`testdata/captures/` 下的任何文件都不应出现。

## 5. 裁决记录（2026-10-10 用户拍板）

**模拟器路线：不推进，永久关闭。** 用户裁决原文：「无需推进，本项目本就不需要支持移动端，只需要兼容书源等」。

- T086 真实书源档的形态**就此定稿**：以「公开文档 + 社区书源预期行为 + 合成站点 + Jsoup 公开 API oracle」构造，不引入上游 App 级对照——这不视为降级，因为兼容性对象是**书源规则语义**（经合成/真实语料与公开 oracle 度量），不是上游 App 的行为表面
- 运行器的 `emulator` 臂保留代码但**冻结**：未来除非用户重开此裁决，不投入任何 SDK/镜像下载
- 原列于此节的「待 PM 裁决事项」两条均作废

## 6. 操作台账（核对人复核用）

本波（T084）对 `D:/Code/legado` 的**全部**接触，逐条列明：

| # | 操作 | 是否读实现源码内容 |
|---|---|---|
| 1 | `cat avd.bat avd.sh` | **否**（§1 显式授权） |
| 2 | `ls` 仓库根、`ls app/`、`ls modules/` | 否（仅文件名/目录名） |
| 3 | `cat build.gradle` / `settings.gradle` / `libs.versions.toml` / `gradle-wrapper.properties` | 否（构建元数据） |
| 4 | `cat api.md` / `README.md` / `modules/web/README.md` | 否（公开文档） |
| 5 | `cat .github/workflows/*.yml`（按名检索） | 否（CI 配置，非实现） |
| 6 | `cat package.json` / `modules/web/package.json` | 否（构建元数据） |
| 7 | `find ... -iname "*.apk" / -iname "*.kt"`（**只看文件名，不读内容**） | 否（结构性检索） |
| 8 | `git log --oneline` / `git remote -v` / `git tag` | 否（版本元数据） |
| 9 | `grep -rn "robolectric" --include=*.toml --include=*.gradle` | 否（构建元数据） |

**未执行**：任何对 `.kt`/`.kts`/`.java` 文件的 `cat`/`head`/`less`/`Read`/内容 `grep`。
**未执行**：运行 Gradle 构建、下载 SDK、启动模拟器。

---

## 核对人签字

> 执行本波（T084）的 session 在此声明：上述台账完整、无遗漏，
> 期间未打开任何上游 `.kt`/`.kts`/`.java` 源文件，未阅读其内容。

| 项 | 内容 |
|---|---|
| 执行 session | `2150180b`（P8 第一波 · T084-T086，其完成汇报即为本声明的陈述） |
| 执行日期 | 2026-10-10 |
| 上游基准 | `D:/Code/legado` @ `5198645f0` |
| 执行者签字 | ✔（以该 session 的完成汇报为签） |
| **独立核对人签字** | ✔ **PM session（kimi-code/k3 · wd_legado）**——已逐条复核 §6 台账：9 条操作均为结构性检索/构建元数据/公开文档/显式授权的启动脚本，无一读取 `.kt`/`.kts`/`.java` 实现源码内容；认可本波无源码接触 |
| 核对日期 | 2026-10-10 |

> **核对人须知**：签字即确认你已独立复核 §6 台账与 §2 红线，并认可本波无源码接触。
> 若无法确认，**不要签字**——改成记录疑点，交 PM 处置。缺签字则 T084 不算闭合。
