# 上游参考运行器（T084）

差分测试（SC-002）的**对照臂**：驱动上游参考实现产生「输入→输出」数据，
供 tome 侧比对，度量书源兼容性（宪法原则 I）。

## 干净室纪律

**先读 [`CLEANROOM.md`](./CLEANROOM.md)**——那里有成文的允许/禁止清单、
本波的操作台账，以及**核对人签字行**（T084 的检查点硬性要求）。
一句话：只黑盒运行上游**可执行产物**，绝不打开其 `.kt`/`.kts`/`.java` 源码。

## 用法

```bash
./ref-runner.sh probe                  # 环境探测：报告哪条臂可用（不接触上游）
./ref-runner.sh fetch-jsoup            # 取 Jsoup jar 到 lib/（jvm 臂前置，首次）
./ref-runner.sh jvm                    # 【本波默认】Jsoup 公开 API oracle
./ref-runner.sh gradle-test            # 经上游 Gradle 任务取对照输出（需代理）
./ref-runner.sh emulator               # 模拟器臂（本波不可用，见 §3.2）
```

产物落 `testdata/captures/`（`.gitignore` 已排除，不入仓库）。

## 本机实测结论（2026-10-10）

| 臂 | 状态 | 说明 |
|---|---|---|
| `jvm` | **可用** | 只需 JDK 21（本机已有）+ Jsoup jar |
| `gradle-test` | 可用（经代理） | 上游 `gradlew` 存在；下载要走代理 |
| `emulator` | **不可用** | 无 Android SDK / emulator / AVD / 已构建 APK |

模拟器路线需下载 **约 8–15 GB**（实测尺寸见 `CLEANROOM.md` §3.2），
本机 D 盘余 14 GB。按下载纪律**未执行**，等 PM 裁决（`CLEANROOM.md` §5）。

## 两侧如何比对

```
testdata/captures/site/*.html          合成站点（输入，两侧共用）
         │
   ┌─────┴─────┐
   ▼           ▼
tome 管线   ref-runner 的 Jsoup oracle
   │           │
   ▼           ▼
captures/tome/   captures/jvm/jsoup-oracle.json
        └────┬────┘
             ▼
     T086 的真实书源档基线
```

`captures/tome/` 由 `crates/tome-cli/tests/capture_synthetic.rs` 产出：

```bash
cargo test -p tome-cli --test capture_synthetic -- --ignored --nocapture
```
