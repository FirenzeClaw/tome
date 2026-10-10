#!/usr/bin/env bash
# 上游参考运行器 —— 黑盒产出「输入→输出」对照数据（tasks.md T084）
#
# 干净室纪律（宪法原则 II）：本脚本只调上游**可执行产物**或**公开 API**，
# 绝不读取上游 `.kt`/`.kts`/`.java` 源码。约定与操作台账见同目录 CLEANROOM.md。
#
# 用法：
#   ./ref-runner.sh probe                     # 环境探测，报告哪条臂可用（不接触上游）
#   ./ref-runner.sh jvm <out-dir>             # 本波默认臂：Jsoup 公开 API oracle
#   ./ref-runner.sh gradle-test <out-dir>     # 经上游 Gradle 任务取对照输出（需代理）
#   ./ref-runner.sh emulator <out-dir>        # 模拟器臂（需 SDK+AVD，本波不可用）
#   ./ref-runner.sh fetch-jsoup               # 取 Jsoup jar 到 lib/（首次用 jvm 臂前置）
#
# 产物一律落 `testdata/captures/`（.gitignore 已排除，不入仓库）。

set -euo pipefail

# --- 路径 -------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# 上游参考实现：只读。**本脚本从不读取其源码文件**。
UPSTREAM="${UPSTREAM_ROOT:-/d/Code/legado}"

CAPTURES="$REPO_ROOT/testdata/captures"
LIB_DIR="$SCRIPT_DIR/lib"

# JDK：优先复用 S0 已装好的便携版（不改系统环境变量）
JAVA_HOME_DEFAULT="/d/dev/toolchain/jdk/jdk-21.0.12.1+1"
JAVA_HOME="${JAVA_HOME:-$JAVA_HOME_DEFAULT}"

# Jsoup 版本。取值来源 = 上游构建元数据 `gradle/libs.versions.toml` 的依赖声明，
# **不是**其实现源码（与 S0 的 spikes/s0/tools/README.md 同一做法）。
JSOUP_VERSION="${JSOUP_VERSION:-1.23.2}"

# 代理：GitHub/Maven 直连 TLS 会被掐，故走本机代理（端口以实际监听为准）
PROXY_HOST="${PROXY_HOST:-127.0.0.1}"
PROXY_PORT="${PROXY_PORT:-7890}"

# --- 输出 -------------------------------------------------------------------

say()  { printf '%s\n' "$*"; }
warn() { printf '[警告] %s\n' "$*" >&2; }
die()  { printf '[错误] %s\n' "$*" >&2; exit 1; }

# --- 探测 -------------------------------------------------------------------

has_cmd() { command -v "$1" >/dev/null 2>&1; }

check_java() {
    [[ -x "$JAVA_HOME/bin/java" ]] || return 1
    "$JAVA_HOME/bin/java" -version >/dev/null 2>&1 || return 1
    return 0
}

check_jsoup_jar() {
    [[ -f "$LIB_DIR/jsoup-$JSOUP_VERSION.jar" ]]
}

cmd_probe() {
    say "=== 上游参考运行器 · 环境探测 ==="
    say "上游仓库：$UPSTREAM"
    say "产物目录：$CAPTURES"
    say ""

    local jvm_ok=no gradle_ok=no emu_ok=no

    if check_java; then
        say "[可用] JDK：$("$JAVA_HOME/bin/java" -version 2>&1 | head -1)"
        jvm_ok=yes
    else
        warn "JDK 不可用（找过 $JAVA_HOME）"
    fi

    if check_jsoup_jar; then
        say "[可用] Jsoup jar：lib/jsoup-$JSOUP_VERSION.jar"
    else
        say "[缺失] Jsoup jar —— 跑 \`$0 fetch-jsoup\` 获取（jvm 臂前置）"
    fi

    if has_cmd adb; then
        local devices
        devices="$(adb devices 2>/dev/null | tail -n +2 | grep -c 'device$' || true)"
        if [[ "$devices" -gt 0 ]]; then
            say "[可用] adb：$devices 台设备已连接"
            emu_ok=yes
        else
            say "[缺失] adb 存在但**无设备**（0 台）"
        fi
    else
        say "[缺失] adb 不在 PATH"
    fi

    has_cmd emulator || say "[缺失] emulator 不在 PATH（模拟器臂不可用）"
    [[ -n "${ANDROID_AVD_HOME:-}" ]] && say "ANDROID_AVD_HOME=$ANDROID_AVD_HOME" \
        || say "[缺失] ANDROID_AVD_HOME 未设置"

    if [[ -x "$UPSTREAM/gradlew" ]]; then
        say "[可用] 上游 gradlew 存在（gradle-test 臂在代理可达时可用）"
        gradle_ok=yes
    else
        say "[缺失] 上游 gradlew 不存在"
    fi

    say ""
    say "=== 可用臂 ==="
    [[ "$jvm_ok" == yes && "$(check_jsoup_jar && echo y || echo n)" == y ]] \
        && say "  jvm          —— 可用（本波默认）" \
        || say "  jvm          —— 需先 \`$0 fetch-jsoup\`"
    [[ "$gradle_ok" == yes ]] \
        && say "  gradle-test  —— 可用（经代理）" \
        || say "  gradle-test  —— 不可用"
    [[ "$emu_ok" == yes ]] \
        && say "  emulator     —— 可用" \
        || say "  emulator     —— **不可用**（无 SDK/AVD；需约 8-15 GB 下载，待 PM 裁决）"
    say ""
    say "详见 CLEANROOM.md §3 与 §5。"
}

# --- Jsoup jar --------------------------------------------------------------

cmd_fetch_jsoup() {
    mkdir -p "$LIB_DIR"
    local jar="$LIB_DIR/jsoup-$JSOUP_VERSION.jar"
    if [[ -f "$jar" ]]; then
        say "已存在：$jar"
        return 0
    fi

    # Maven Central 与阿里云镜像，逐个尝试（直连优先，失败退化到代理）
    local urls=(
        "https://repo1.maven.org/maven2/org/jsoup/jsoup/$JSOUP_VERSION/jsoup-$JSOUP_VERSION.jar"
        "https://maven.aliyun.com/repository/central/org/jsoup/jsoup/$JSOUP_VERSION/jsoup-$JSOUP_VERSION.jar"
    )
    local url
    for url in "${urls[@]}"; do
        say "尝试：$url"
        if curl -fsSL --max-time 120 -o "$jar" "$url" 2>/dev/null; then
            say "已下载：$jar"
            return 0
        fi
        if curl -fsSL --max-time 120 -x "http://$PROXY_HOST:$PROXY_PORT" --ssl-no-revoke \
             -o "$jar" "$url" 2>/dev/null; then
            say "已下载（经代理）：$jar"
            return 0
        fi
        warn "失败，换下一个源"
    done
    die "Jsoup jar 下载失败——请检查网络或手动放到 $jar"
}

# --- jvm 臂：Jsoup 公开 API oracle ------------------------------------------

cmd_jvm() {
    local out_dir="${1:-$CAPTURES/jvm}"
    check_java || die "JDK 不可用：$JAVA_HOME"
    check_jsoup_jar || die "缺 Jsoup jar，先跑：$0 fetch-jsoup"

    mkdir -p "$out_dir"

    local src="$SCRIPT_DIR/JsoupRuleOracle.java"
    [[ -f "$src" ]] || die "缺 oracle 源码：$src"

    local classes="$SCRIPT_DIR/.build"
    mkdir -p "$classes"

    # 走相对路径：Git Bash 的 MSYS 运行时会**改写** -cp 里以 `/d/...` 形态出现的
    # 绝对路径，且只在分隔符 `;` 前后被改写，产出 `D:\...;D:\...` 却把盘符弄坏，
    # 表现为 ClassNotFoundException（类明明编译出来了）。
    # 用相对路径彻底绕开改写——这是路径转换问题，不是代码问题。
    (
        cd "$SCRIPT_DIR"
        say "编译 oracle（-encoding UTF-8）..."
        # Windows 的 classpath 分隔符是 ';'
        "$JAVA_HOME/bin/javac" -encoding UTF-8 \
            -cp "lib/jsoup-$JSOUP_VERSION.jar" -d ".build" "JsoupRuleOracle.java" || exit 1

        say "运行 oracle → $out_dir"
        "$JAVA_HOME/bin/java" -Dfile.encoding=UTF-8 \
            -cp "lib/jsoup-$JSOUP_VERSION.jar;.build" \
            JsoupRuleOracle "$out_dir"
    )
}

# --- gradle-test 臂 --------------------------------------------------------

cmd_gradle_test() {
    local out_dir="${1:-$CAPTURES/gradle-test}"
    check_java || die "JDK 不可用"

    [[ -x "$UPSTREAM/gradlew" ]] || die "上游 gradlew 不可用：$UPSTREAM/gradlew"
    say "经上游 Gradle 任务取对照输出（黑盒运行其产物，不读其源码）"
    say "  任务：:app:testAppReleaseUnitTest"
    say "  产物：$out_dir"

    mkdir -p "$out_dir"
    # Gradle 与依赖下载走代理；GRADLE_USER_HOME 指 D 盘（磁盘纪律）
    (
        cd "$UPSTREAM"
        JAVA_HOME="$JAVA_HOME" \
        GRADLE_USER_HOME="${GRADLE_USER_HOME:-/d/dev/cache/gradle}" \
        HTTPS_PROXY="http://$PROXY_HOST:$PROXY_PORT" \
        HTTP_PROXY="http://$PROXY_HOST:$PROXY_PORT" \
        ./gradlew :app:testAppReleaseUnitTest --no-daemon \
            -Dhttp.keepAlive=false 2>&1 | tee "$out_dir/gradle-test.log"
    ) || warn "Gradle 任务非零退出（日志见 $out_dir/gradle-test.log）"

    say "完成。JUnit 报告通常在上游 app/build/test-results/ 下，"
    say "如需留存请手动复制到 $out_dir（注意：那是对照产物，不入 git）。"
}

# --- emulator 臂 -----------------------------------------------------------

cmd_emulator() {
    local out_dir="${1:-$CAPTURES/emulator}"
    warn "模拟器臂本波不可用。"

    if ! has_cmd emulator; then
        warn "原因：emulator 不在 PATH；本机无 Android SDK、无 AVD 镜像、无已构建 APK。"
    elif [[ -z "${ANDROID_AVD_HOME:-}" ]]; then
        warn "原因：ANDROID_AVD_HOME 未设置。"
    fi

    cat >&2 <<'EOF'

按 tasks.md 与 CLEANROOM.md §3.2/§5，走该路线需先下载（实测尺寸）：
  platform-tools 8.6MB + android-36 62.8MB + build-tools ~56MB
  + cmdline-tools ~148MB + emulator ~334MB
  + system image 1807.6MB + Gradle 发行版 ~150MB
  + Gradle 依赖缓存 3-6GB + 构建产物 1-2GB + AVD 2-4GB
  ≈ 8-15 GB（本机 D 盘余 14 GB）

该下载须先报 PM 确认，故本波未执行。就位后本臂即可用，无需改运行器。
EOF
    exit 2
}

# --- 分派 -------------------------------------------------------------------

main() {
    local action="${1:-probe}"
    shift || true
    case "$action" in
        probe)       cmd_probe ;;
        fetch-jsoup) cmd_fetch_jsoup ;;
        jvm)         cmd_jvm "$@" ;;
        gradle-test) cmd_gradle_test "$@" ;;
        emulator)    cmd_emulator "$@" ;;
        -h|--help|help)
            sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            ;;
        *) die "未知动作：$action（可用：probe / fetch-jsoup / jvm / gradle-test / emulator）" ;;
    esac
}

main "$@"
