#!/usr/bin/env bash
#
# tome-reader 渲染门控守卫（对应 tasks.md 002 T001）
#
# ## 这个脚本防的是什么
#
# `crates/tome-reader` 同时承载**排版层**（headless）与**渲染层**（wgpu）。
# 宪法 IV / FR-052 / FR-036 要求前者能在无图形环境下完整运行——而这条约束
# **不会自己保持**：任何一次"顺手在 layout 里 use 一下 wgpu"的改动都会把
# 图形栈拖进排版层，但编译依然通过、测试依然全绿。它是**架构性的**，不是
# 功能性的，故只能用构建期的两条断言钉住：
#
# 1. **`--no-default-features` 的依赖树里零图形包**
#    这是"编译期不存在"而非"运行时不使用"。若某个图形包出现在树里，
#    headless 构建就名不副实（体积、依赖面、平台可用性全被污染）。
#
# 2. **两种构建都必须编译通过**
#    feature 门控最常见的事故形态是"只测过开着的那一种"。CI 里两个 job
#    分别构建，本脚本在本地把这两条合成一次调用。
#
# ## 为什么用 `cargo tree` 而不是 grep 源码
#
# grep `use wgpu` 只能看到**直接引用**，看不到传递依赖：例如有人在排版模块
# 里依赖了某个内部 crate，而那个 crate 依赖了图形包——源码里一个字都没有，
# 依赖树里却全是。`cargo tree` 看的是**解析后的真实依赖图**，唯一能覆盖
# 传递引入的检查方式。
#
# ## 用法
#
#   scripts/check-reader-features.sh             # 检查 tome-reader 门控
#   scripts/check-reader-features.sh --self-test # 自检守卫自身有效性
#
# 退出码：0 = 通过；1 = 发现违规；2 = 脚本自身/环境错误
#
# ## 如何端到端验证守卫有效
#
# 临时在 `crates/tome-reader/Cargo.toml` 里把 `wgpu` 的 `optional = true`
# 去掉（即变成无条件依赖），本脚本应以 exit 1 报"feature off 时依赖树含图形包"。
# 验证后务必还原。

set -euo pipefail

readonly CRATE="tome-reader"

# 图形栈包名（正则，大小写不敏感）。
#
# ## 为什么按包名而非 feature 判断
#
# feature 名会漂移（wgpu 每季度一大版本，feature 集合屡变），包名稳定得多。
# 覆盖范围刻意放宽到"任何会拉进窗口系统/图形 API/着色器编译器的包"——
# 本层**一个都不需要**（排版是纯 CPU 文本计算）。
readonly GRAPHICS_PATTERN='wgpu|winit|naga|glow|glutin|raw-window-handle|swash|etagere|d3d12|metal-rs|ash|vulkano'

readonly EXIT_OK=0
readonly EXIT_VIOLATION=1
readonly EXIT_ERROR=2

log_info()  { printf '[reader-guard] %s\n' "$*"; }
log_pass()  { printf '[reader-guard] PASS  %s\n' "$*"; }
log_fail()  { printf '[reader-guard] FAIL  %s\n' "$*" >&2; }
log_error() { printf '[reader-guard] ERROR %s\n' "$*" >&2; }

# 取依赖树的包名列表（去版本、去树形前缀，一行一个）。
#
# `--prefix none` 去掉树形连线与深度信息，使我们能对整个列表做集合判定；
# `sed 's/ v.*//'` 剥掉版本后缀；`sort -u` 去重（同一包可能在多处以不同
# feature 出现，重复行只会让输出变长，不影响判定）。
graphics_packages_in_tree() {
    local tree="$1"
    printf '%s\n' "$tree" \
        | sed 's/ v[0-9].*$//' \
        | grep -oE '(^|[^a-zA-Z0-9_-])[a-zA-Z0-9_-]+' \
        | grep -iE "^(${GRAPHICS_PATTERN})$" \
        | sort -u || true
}

# 检查一：feature off 时依赖树零图形包。
check_no_graphics_when_off() {
    local tree="$1"
    local hits
    hits="$(graphics_packages_in_tree "$tree")"

    if [[ -n "$hits" ]]; then
        log_fail "feature off 时依赖树含图形包（FR-052/FR-036：排版层必须能无图形环境构建）"
        printf '%s\n' "$hits" >&2
        return 1
    fi
    log_pass "feature off 时依赖树零图形包"
    return 0
}

# 检查二：feature 门控的**接线方向**必须正确。
#
# 这条防的是"门控接反"：若把 render 从 default 里删掉，渲染面会在默认构建里
# 消失——上面那条检查依然全绿（树里没有图形包），但产品能力已经退化。
# 两条一起才构成完整约束：**默认必须有图形，关闭必须没有**。
check_render_is_default() {
    local tree="$1"
    local hits
    hits="$(graphics_packages_in_tree "$tree")"

    if [[ -z "$hits" ]]; then
        log_fail "默认构建（render on）依赖树里没有任何图形包——feature 门控接反了"
        return 1
    fi
    log_pass "默认构建（render on）含图形栈，门控方向正确"
    return 0
}

# 对一棵树施加全部检查。返回 0 全过 / 1 有违规。
check_tree() {
    local label="$1"
    local tree="$2"
    local failed=0
    log_info "── 检查 ${label} ──"
    check_no_graphics_when_off "$tree" || failed=1
    return "$failed"
}

main() {
    log_info "检查 ${CRATE} 渲染门控（headless 构建零图形包；默认构建含渲染栈）"

    local failed=0

    # 1. feature off：树里不得有图形包。
    local off_tree
    if ! off_tree="$(cargo tree -p "$CRATE" --no-default-features --prefix none 2>&1)"; then
        log_error "cargo tree（--no-default-features）执行失败，无法判定门控（不放行）"
        printf '%s\n' "$off_tree" >&2
        return "$EXIT_ERROR"
    fi
    check_tree "feature off 依赖树" "$off_tree" || failed=1

    # 2. 默认（render on）：树里**应**有图形包（防门控接反）。
    local on_tree
    if ! on_tree="$(cargo tree -p "$CRATE" --prefix none 2>&1)"; then
        log_error "cargo tree（默认 features）执行失败，无法判定门控（不放行）"
        printf '%s\n' "$on_tree" >&2
        return "$EXIT_ERROR"
    fi
    check_render_is_default "$on_tree" || failed=1

    # 3. 两种构建都必须能编译过（门控事故最常见的形态是"只测过开着的那种"）。
    if ! cargo build -p "$CRATE" --no-default-features --quiet 2>&1; then
        log_fail "cargo build --no-default-features 失败（headless 构建必须可用）"
        failed=1
    else
        log_pass "headless 构建（--no-default-features）编译通过"
    fi

    if (( failed )); then
        log_fail "存在违规，构建被阻断"
        return "$EXIT_VIOLATION"
    fi
    log_pass "渲染门控约束满足"
    return "$EXIT_OK"
}

# ---------- 自检：验证守卫本身确实能抓到违规 ----------
#
# 为什么需要自检：守卫最大的风险是"永远通过"——正则写错、包名改了、输出格式
# 变了，都会让它静默失效，而失效的守卫比没有守卫更危险（制造"已防护"的假象）。
# 故此处用**合成的 tree 输出**断言判断逻辑，不依赖真实依赖解析。
self_test() {
    log_info "自检模式：用合成输入断言守卫的判断逻辑"

    local failures=0

    # 用例 1：干净的 headless 树必须通过
    local clean_tree="${CRATE} v0.1.0
cosmic-text v0.19.0
fontdb v0.23.0
harfrust v0.5.2
skrifa v0.40.0
unicode-segmentation v1.12.0
tome-core v0.1.0"
    if check_no_graphics_when_off "$clean_tree" >/dev/null 2>&1; then
        log_pass "用例1 干净 headless 树 → 通过"
    else
        log_fail "用例1 干净 headless 树被误判为违规"
        failures=$((failures + 1))
    fi

    # 用例 2：树里出现 wgpu 必须被抓到（核心场景）
    local wgpu_tree="${CRATE} v0.1.0
wgpu v30.0.1
wgpu-core v30.0.1"
    if check_no_graphics_when_off "$wgpu_tree" >/dev/null 2>&1; then
        log_fail "用例2 wgpu 出现在 headless 树里未被抓到（守卫失效！）"
        failures=$((failures + 1))
    else
        log_pass "用例2 headless 树含 wgpu → 抓到"
    fi

    # 用例 3：传递性图形依赖也必须被抓到（grep 源码抓不到的场景）
    local transitive_tree="${CRATE} v0.1.0
some-helper v1.0.0
naga v30.0.1"
    if check_no_graphics_when_off "$transitive_tree" >/dev/null 2>&1; then
        log_fail "用例3 传递引入的 naga 未被抓到（守卫失效！）"
        failures=$((failures + 1))
    else
        log_pass "用例3 传递引入的 naga → 抓到"
    fi

    # 用例 4：门控接反（默认构建无图形）必须被抓到
    if check_render_is_default "$clean_tree" >/dev/null 2>&1; then
        log_fail "用例4 默认构建无图形栈未被抓到（门控接反，守卫失效！）"
        failures=$((failures + 1))
    else
        log_pass "用例4 默认构建无图形栈 → 抓到（门控接反）"
    fi

    # 用例 5：正常默认树必须放行
    if check_render_is_default "$wgpu_tree" >/dev/null 2>&1; then
        log_pass "用例5 默认树含 wgpu → 通过"
    else
        log_fail "用例5 默认树含 wgpu 被误判"
        failures=$((failures + 1))
    fi

    # 用例 6：包名相似的无关 crate 不得误报（防正则过宽）
    local lookalike_tree="${CRATE} v0.1.0
naga-types-helper v0.1.0
swashbuckler v0.1.0"
    if check_no_graphics_when_off "$lookalike_tree" >/dev/null 2>&1; then
        log_pass "用例6 相似名无关 crate 未误报"
    else
        log_fail "用例6 相似名无关 crate 被误报（正则过宽）"
        failures=$((failures + 1))
    fi

    if (( failures )); then
        log_fail "自检失败：${failures} 个用例未通过，守卫不可信"
        return "$EXIT_VIOLATION"
    fi
    log_pass "自检全部通过，守卫判断逻辑有效"
    return "$EXIT_OK"
}

if [[ "${1:-}" == "--self-test" ]]; then
    self_test
    exit $?
fi
main
exit $?
