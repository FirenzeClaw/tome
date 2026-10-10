#!/usr/bin/env bash
#
# T007 体积对照测量（R5-3）
#
# ## 为什么是脚本而不是 cargo 子命令
#
# 「编译四种 feature 组合各一次、分别 stat、算差分」这件事用 shell 表达
# 最直接，也最容易被 CI 复用。算术本身在 `spikes/s2/src/size.rs` 里有单测
# 覆盖，本脚本只做编排与取值。
#
# ## 为什么必须逐臂 `cargo clean`
#
# 不 clean 时 cargo 会复用上一次的中间产物，而 feature 组合不同会让
# codegen 结果**不同**——但 cargo 的增量缓存在跨 feature 组合时并不总是
# 保守失效。实测出现过"增量构建"与"干净构建"体积不一致的情形，故这里
# 每臂都 clean。代价是慢（每臂一次全量 release 编译），但数字可信。
#
# ## 用法
#
#   bash scripts/measure-s2-stacks.sh            # 全量测量（约 5-15 分钟）
#   bash scripts/measure-s2-stacks.sh --quick    # 跳过 clean（仅供快速查看）
#
# 退出码：0 = 成功；1 = 测量失败；2 = 环境错误

set -euo pipefail

readonly SPIKE_DIR="spikes/s2"
readonly BIN_NAME="tome-s2-spike"
readonly QUICK="${1:-}"

log_info()  { printf '[size] %s\n' "$*"; }
log_error() { printf '[size] ERROR %s\n' "$*" >&2; }

if [[ ! -f "$SPIKE_DIR/Cargo.toml" ]]; then
    log_error "未找到 $SPIKE_DIR/Cargo.toml（脚本须在仓库根执行）"
    exit 2
fi

# 逐臂：编译 → 取二进制字节数 → 打印一行机器可读的结果
measure_arm() {
    local arm="$1"
    local features="$2"

    if [[ "$QUICK" != "--quick" ]]; then
        cargo clean --manifest-path "$SPIKE_DIR/Cargo.toml" --release >/dev/null 2>&1 || true
    fi

    local -a args=(build --manifest-path "$SPIKE_DIR/Cargo.toml" --release --quiet)
    if [[ -n "$features" ]]; then
        args+=(--features "$features")
    fi

    if ! cargo "${args[@]}" 2>&1; then
        log_error "编译失败：arm=$arm features=${features:-<none>}"
        return 1
    fi

    # 定位产物：Windows 是 .exe，其他平台无后缀
    local bin="$SPIKE_DIR/target/release/$BIN_NAME"
    if [[ -f "$bin.exe" ]]; then
        bin="$bin.exe"
    elif [[ ! -f "$bin" ]]; then
        log_error "未找到产物 $bin（或 .exe）"
        return 1
    fi

    local bytes
    bytes="$(stat -c '%s' "$bin" 2>/dev/null || stat -f '%z' "$bin")"
    printf 'ARM\t%s\t%s\t%s\n' "$arm" "${features:-<none>}" "$bytes"
}

log_info "T007 体积对照（release profile 继承自 spikes/s2/Cargo.toml）"
if [[ "$QUICK" == "--quick" ]]; then
    log_info "⚠️  --quick 模式：跳过 cargo clean，数字可能因增量构建而失真"
fi

measure_arm "empty" ""
measure_arm "cosmic-text-stack" "cosmic-text-stack"
measure_arm "render-stack" "render-stack"
measure_arm "full-stack" "full-stack"

log_info "测量完成。把上面的 ARM 行按 size.rs 的口径算差分后写入 BASELINE.md §6。"
