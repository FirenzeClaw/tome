#!/usr/bin/env bash
#
# rquickjs 依赖守卫（对应 tasks.md T005，落实 research.md R1 缓解 1/2）
#
# ## 这个脚本防的是什么
#
# 书源脚本沙箱的两条防线都依赖 rquickjs 的特定配置，且**失效时不会报错**，
# 只会在运行时静默降级。所以必须用 CI 断言把它们钉住：
#
# 1. **禁止 `rust-alloc` / `allocator` feature**
#    一旦启用，rquickjs 改用自定义分配器管理 JS 堆，
#    `Runtime::set_memory_limit()` 随之**静默变成 noop**——
#    内存上限看起来设了，实际不生效，脚本可耗尽宿主内存。
#    这是"静默失效"，没有任何编译期或运行期提示，只能靠本守卫拦。
#
# 2. **rquickjs 版本 >= 0.14.0**
#    0.14.0 才包含"中断处理器触发的 GC 引用计数崩溃"修复（PR #664）。
#    低于此版本时，用 `set_interrupt_handler` 打断死循环脚本会崩溃进程。
#
# ## 为什么用 `cargo tree -e features` 而不是读 Cargo.toml
#
# 读 Cargo.toml 只能看到**直接声明**的 feature，看不到传递依赖启用的 feature
# （例如某个第三方 crate 打开 `rquickjs/full`，或未来某个依赖间接拉入
# `rust-alloc`）。`cargo tree -e features` 展示的是**解析后的真实 feature 图**，
# 这是唯一能覆盖传递引入的检查方式。
#
# ## 用法
#
#   scripts/check-rquickjs-features.sh            # 检查 workspace + S0 spike
#   scripts/check-rquickjs-features.sh --self-test # 自检守卫自身有效性
#
# 退出码：0 = 通过；1 = 发现违规；2 = 脚本自身/环境错误
#
# ## 如何验证守卫真的有效（T005 验收项）
#
# 自动化手段是 `--self-test`（用合成 tree 输出断言判断逻辑，含 6 个用例）。
# 需要端到端验证时，可对真实 workspace 注入违规后跑守卫，例如在
# `crates/tome-js/Cargo.toml` 临时写入：
#
#   rquickjs = { version = "0.14.0", features = ["rust-alloc"] }   # 预期 exit 1
#   rquickjs = { version = "=0.13.0", features = ["std"] }         # 预期 exit 1（有版本下限）
#
# 注意：写 `version = "0.13.0"`（无 `=`）**不会**触发版本违规——
# caret 语义会解析到 0.14.0。要验证版本下限必须用 `=` 精确钉版。
# 验证后务必还原 Cargo.toml 与 Cargo.lock。

set -euo pipefail

# 允许违规的实测下限；低于此版本会崩溃（见上）
readonly MIN_RQUICKJS_VERSION="0.14.0"
# 禁止启用的 feature：两者都会让 set_memory_limit 静默失效
readonly FORBIDDEN_FEATURES=("rust-alloc" "allocator")

readonly EXIT_OK=0
readonly EXIT_VIOLATION=1
readonly EXIT_ERROR=2

log_info()  { printf '[guard] %s\n' "$*"; }
log_pass()  { printf '[guard] PASS  %s\n' "$*"; }
log_fail()  { printf '[guard] FAIL  %s\n' "$*" >&2; }
log_error() { printf '[guard] ERROR %s\n' "$*" >&2; }

# 版本比较：$1 >= $2 时返回 0。
# 用 sort -V 做语义化比较，避免手写数值解析对 "0.14.0-beta" 之类的误判。
version_ge() {
    local actual="$1" required="$2"
    [[ "$(printf '%s\n%s\n' "$required" "$actual" | sort -V | head -n1)" == "$required" ]]
}

# 取 feature 图。失败时返回非零，由调用方转成 EXIT_ERROR——
# 网络不可达或 Cargo.lock 缺失会让 cargo tree 失败，那种情况必须报错而非放行。
#
# ## 为什么要覆盖 S0 spike（T006 要求）
#
# `spikes/s0/` 刻意脱离主 workspace（它是图形依赖，不进引擎的依赖图）。
# 但它**同样使用 rquickjs**，故主 workspace 的检查对它无效——若只查 workspace，
# 会出现"守卫全绿而 spike 里悄悄开了 rust-alloc"的盲区。而 spike 正是
# 首个实际编译 QuickJS 的地方，也是最先能暴露该问题的位置。
#
# 用 `-p` 指定包名而非路径：命令在仓库根执行，与调用者 cwd 无关。
feature_tree_workspace() {
    cargo tree -e features --workspace 2>&1
}

feature_tree_spike() {
    # 必须带 --features spike-js：spike 的 rquickjs 是可选依赖（见其 Cargo.toml
    # 的 [features] 注释），不开这个 feature 时依赖图里根本没有 rquickjs。
    # 若不带，本检查会误报"未引入"从而放过违规。
    cargo tree -e features --manifest-path spikes/s0/Cargo.toml --features spike-js 2>&1
}

# 对一棵依赖树施加全部检查。返回 0 全过 / 1 有违规。
check_tree() {
    local label="$1"
    local tree="$2"
    local failed=0

    log_info "── 检查 ${label} ──"
    check_forbidden_features "$tree" || failed=1
    check_min_version "$tree" || failed=1
    return "$failed"
}

# ---------- 检查 1：禁止的 feature ----------
check_forbidden_features() {
    local tree="$1"
    local failed=0
    local feature

    for feature in "${FORBIDDEN_FEATURES[@]}"; do
        # 匹配 `rquickjs feature "rust-alloc"` / `rquickjs-core feature "allocator"`
        # 两种形态：feature 可能由 rquickjs 自身或其 core 子 crate 承载。
        local hits
        hits="$(printf '%s\n' "$tree" \
            | grep -E "rquickjs(-core)? feature \"${feature}\"" || true)"
        if [[ -n "$hits" ]]; then
            log_fail "检测到被禁止的 feature \"${feature}\"（会使 set_memory_limit 静默失效）"
            printf '%s\n' "$hits" >&2
            failed=1
        else
            log_pass "未启用被禁止的 feature \"${feature}\""
        fi
    done

    return "$failed"
}

# ---------- 检查 2：版本下限 ----------
check_min_version() {
    local tree="$1"

    # 从树中提取 rquickjs 自身版本。`rquickjs-core`/`-sys`/`-macro` 也含
    # "rquickjs" 前缀，故用 `rquickjs v` 精确匹配主 crate 行。
    local version
    version="$(printf '%s\n' "$tree" \
        | grep -oE '(^|[^a-z-])rquickjs v[0-9]+\.[0-9]+\.[0-9]+' \
        | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' \
        | sort -Vu | tail -n1 || true)"

    if [[ -z "$version" ]]; then
        # 骨架阶段属预期的"未引入"状态，不是通过也不是违规。
        log_info "rquickjs 尚未出现在依赖图中（骨架阶段属预期；Phase 2 引入后本检查转为强断言）"
        return 0
    fi

    if version_ge "$version" "$MIN_RQUICKJS_VERSION"; then
        log_pass "rquickjs 版本 ${version} >= ${MIN_RQUICKJS_VERSION}"
        return 0
    fi

    log_fail "rquickjs 版本 ${version} < ${MIN_RQUICKJS_VERSION}（0.14.0 才含中断处理器 GC 崩溃修复 PR #664）"
    return 1
}

# ---------- 主流程 ----------
main() {
    log_info "检查 rquickjs 依赖约束（禁 ${FORBIDDEN_FEATURES[*]}；版本 >= ${MIN_RQUICKJS_VERSION}）"

    local failed=0

    local workspace_tree
    if ! workspace_tree="$(feature_tree_workspace)"; then
        log_error "cargo tree -e features --workspace 执行失败，无法判定依赖约束（不放行）"
        printf '%s\n' "$workspace_tree" >&2
        return "$EXIT_ERROR"
    fi
    check_tree "主 workspace" "$workspace_tree" || failed=1

    # S0 spike：独立 crate，其 rquickjs 不在 workspace 的依赖图里。
    # 仅当该目录存在时检查——spike 是一次性验证产物，日后清理掉不应让守卫报错。
    if [[ -f spikes/s0/Cargo.toml ]]; then
        local spike_tree
        if ! spike_tree="$(feature_tree_spike)"; then
            log_error "cargo tree（spikes/s0）执行失败，无法判定依赖约束（不放行）"
            printf '%s\n' "$spike_tree" >&2
            return "$EXIT_ERROR"
        fi
        check_tree "S0 spike" "$spike_tree" || failed=1
    else
        log_info "未发现 spikes/s0/Cargo.toml，跳过 spike 检查（一次性产物，可能已清理）"
    fi

    if (( failed )); then
        log_fail "存在违规，构建被阻断"
        return "$EXIT_VIOLATION"
    fi
    log_pass "全部 rquickjs 依赖约束满足"
    return "$EXIT_OK"
}

# ---------- 自检：验证守卫本身确实能抓到违规 ----------
#
# 为什么需要自检：守卫脚本最大的风险是"永远通过"——正则写错、字段名变了、
# 输出格式变了，都会让它静默失效，而失效的守卫比没有守卫更危险
# （会制造"已防护"的假象）。故此处用**合成的 tree 输出**断言守卫的判断逻辑，
# 不依赖真实网络与真实依赖解析。
self_test() {
    log_info "自检模式：用合成输入断言守卫的判断逻辑"

    local failures=0

    # 用例 1：干净输入必须通过
    local clean_tree="rquickjs v0.14.0
└── rquickjs-core v0.14.0
    └── rquickjs-sys v0.14.0"
    if check_min_version "$clean_tree" >/dev/null 2>&1 \
        && check_forbidden_features "$clean_tree" >/dev/null 2>&1; then
        log_pass "用例1 干净输入 → 通过"
    else
        log_fail "用例1 干净输入被误判为违规"
        failures=$((failures + 1))
    fi

    # 用例 2：启用 rust-alloc 必须被抓到
    local rust_alloc_tree="rquickjs v0.14.0
└── rquickjs feature \"rust-alloc\"
    └── rquickjs-core feature \"rust-alloc\"
        └── rquickjs-core v0.14.0"
    if check_forbidden_features "$rust_alloc_tree" >/dev/null 2>&1; then
        log_fail "用例2 rust-alloc 违规未被抓到（守卫失效！）"
        failures=$((failures + 1))
    else
        log_pass "用例2 rust-alloc 违规被抓到"
    fi

    # 用例 3：启用 allocator 必须被抓到
    local allocator_tree="rquickjs v0.14.0
└── rquickjs feature \"allocator\"
    └── rquickjs-core feature \"allocator\"
        └── rquickjs-core v0.14.0"
    if check_forbidden_features "$allocator_tree" >/dev/null 2>&1; then
        log_fail "用例3 allocator 违规未被抓到（守卫失效！）"
        failures=$((failures + 1))
    else
        log_pass "用例3 allocator 违规被抓到"
    fi

    # 用例 4：版本过低必须被抓到
    local old_tree="rquickjs v0.13.0
└── rquickjs-core v0.13.0"
    if check_min_version "$old_tree" >/dev/null 2>&1; then
        log_fail "用例4 低版本未被抓到（守卫失效！）"
        failures=$((failures + 1))
    else
        log_pass "用例4 低版本 0.13.0 被抓到"
    fi

    # 用例 5：恰好等于下限必须放行（边界值，防止写成 > 而非 >=）
    local exact_tree="rquickjs v0.14.0
└── rquickjs-core v0.14.0"
    if check_min_version "$exact_tree" >/dev/null 2>&1; then
        log_pass "用例5 版本恰为下限 ${MIN_RQUICKJS_VERSION} → 通过（边界正确）"
    else
        log_fail "用例5 版本恰为下限被误判为违规（>= 写成了 >）"
        failures=$((failures + 1))
    fi

    # 用例 6：不相关 crate 的 allocator feature 不应误报（防正则过宽）
    local unrelated_tree="rquickjs v0.14.0
└── allocator-api2 feature \"alloc\"
    └── allocator-api2 v0.2.21"
    if check_forbidden_features "$unrelated_tree" >/dev/null 2>&1; then
        log_pass "用例6 无关 crate 的同名 feature 未误报"
    else
        log_fail "用例6 无关 crate 的 allocator 被误报为 rquickjs 违规（正则过宽）"
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
