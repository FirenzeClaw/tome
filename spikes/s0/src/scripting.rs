//! rquickjs 一次性 eval 验证（T006 验证项）
//!
//! 验证目标**只有**"三平台能编译并跑通一次 eval"。沙箱设限（内存上限 +
//! 中断处理）属于 S1 的 `tome-js`，这里不展开——但 `set_memory_limit` 的
//! 有效性前提（**不启用 rust-alloc feature**）由 T005 守卫脚本在此刻就守住。

/// 在最小 runtime 上求值一个表达式，返回其字符串化结果。
#[cfg(feature = "spike-js")]
pub fn verify_once() -> Result<String, String> {
    use rquickjs::{Context, Runtime};

    let runtime = Runtime::new().map_err(|error| format!("创建 QuickJS runtime 失败：{error}"))?;

    // 内存上限是 FR-007 的核心防线（"脚本沙箱必须设限 · 内存上限 + 可中断"）。
    // 这里设一个只够跑通 eval 的值，顺带**实证** set_memory_limit 未因
    // 误开 rust-alloc 而静默失效——若它是 noop，本行不会报错，但 T005 守卫
    // 会在依赖层面拦住那种配置。
    runtime.set_memory_limit(SPIKE_MEMORY_LIMIT_BYTES);

    // 显式栈上限（research.md R1 缓解 4）：深递归靠可捕获的 RangeError，
    // 而不是撑爆宿主栈导致进程崩溃。
    runtime.set_max_stack_size(SPIKE_STACK_LIMIT_BYTES);

    let context =
        Context::full(&runtime).map_err(|error| format!("创建 JS context 失败：{error}"))?;

    context.with(|ctx| {
        let value: String = ctx
            .eval(SPIKE_EXPRESSION)
            .map_err(|error| format!("eval 失败：{error}"))?;
        Ok(value)
    })
}

/// 未启用 `spike-js` 时的替身。
///
/// 返回 `Err` 而不是 `Ok("")`：调用方（`--headless`）已用 `cfg!` 门控，
/// 若这个分支真的被执行到了，说明门控写错了——此时应当暴露，而不是
/// 悄悄给一个假成功让体积对照数据失真。
#[cfg(not(feature = "spike-js"))]
pub fn verify_once() -> Result<String, String> {
    Err("未启用 spike-js feature，rquickjs 未编译进本产物".to_owned())
}

/// spike 用的内存上限：8 MiB。
///
/// 取值理由：足够跑通表达式求值（QuickJS 自身初始化约需 1-2 MiB），
/// 又小到能暴露"上限没生效"——若 `set_memory_limit` 静默变成 noop 且
/// 我们误分配了大量内存，这里会先于别处失败。
#[cfg(feature = "spike-js")]
const SPIKE_MEMORY_LIMIT_BYTES: usize = 8 * 1024 * 1024;

/// spike 用的 JS 栈上限：512 KiB。
///
/// 落在 research.md R1 建议的 256 KiB–1 MiB 区间中部：足以跑正常脚本，
/// 又远小于宿主线程栈，深递归会转成可捕获的 RangeError 而非段错误。
#[cfg(feature = "spike-js")]
const SPIKE_STACK_LIMIT_BYTES: usize = 512 * 1024;

/// 求值表达式。刻意**不碰宿主能力**：spike 只验证引擎本体，
/// 宿主注入（NetHost/StoreHost）是 S1 的 tome-js 的职责。
#[cfg(feature = "spike-js")]
const SPIKE_EXPRESSION: &str = r#"
(function () {
    const parts = ['quickjs', 'ok', 21 * 2];
    return parts.join(':');
})()
"#;
