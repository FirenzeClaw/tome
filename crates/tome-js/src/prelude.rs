//! 兼容前导脚本（compat prelude）：补齐 Rhino 与 QuickJS 的方言差异
//! （tasks.md T035）
//!
//! # 依据来源
//!
//! 补齐项取自**书源生态的公开用法**（社区流通的书源里实际出现的写法）
//! 与 **Mozilla Rhino 的公开文档**（Rhino 是上游引擎，书源脚本按它的
//! 方言书写）。不参考任何具体实现代码（干净室纪律）。
//!
//! # 为什么必须有它：三类真实差异
//!
//! ## 1. 缺失的宿主全局会直接抛 `ReferenceError`
//!
//! 书源脚本开头常写 `java.put("x", "y")` 或 `source.bookSourceName`。
//! 在 QuickJS 里 `java` 不存在 → `ReferenceError: java is not defined`，
//! 整段脚本当场失败。而**该脚本本可以降级运行**（书源通常把这类调用
//! 包在 `try/catch` 里）——但因为抛在**调用之前**（引用解析阶段），
//! 某些写法下 catch 拦不住，或拦住了也拿不到有用信息。
//!
//! 故前导脚本先放一个"会抛结构化能力错误"的桩：调用它得到
//! `capability` 属性齐全的异常（FR-006），书源的降级逻辑据此判断。
//!
//! ## 2. 引擎方言差异：`Java.type` 与 `Packages.*`
//!
//! Rhino 提供 `Java.type("java.lang.String")` 与 `Packages.java.util.*`
//! 来访问 JVM 类。本项目是原生实现，**没有 JVM**。这类书源需要 WebView
//! 或 JVM 才能工作，属"明确不支持的能力"——故同样抛结构化能力错误，
//! 而不是让脚本撞上 `TypeError: Cannot read property 'type' of undefined`。
//!
//! ## 3. 已废弃/别名的方法名
//!
//! Rhino 与老版 JS 引擎提供 `trimLeft` / `trimRight`（非标准），
//! 而 ES5+ 规范用 `trimStart` / `trimEnd`。QuickJS 遵循规范，
//! 故 `"x ".trimLeft()` 在旧写法下会 `TypeError`。
//! 补别名即可，语义完全一致。
//!
//! # 注入时机
//!
//! **每个 runtime 在 eval 书源脚本之前注入一次**（tasks.md 验收项）。
//! 注入失败必须报出来——静默跳过会让所有依赖前导脚本的书源以
//! 难以理解的 `ReferenceError` 失败。
//!
//! # 为什么前导脚本是 JS 而不是 Rust 逐条 `Object::set`
//!
//! 前导脚本里有条件分支、循环与闭包（见 `java` 桩的实现）。用 Rust 写
//! 会变成一堆 `Function::new` + `Object::set`，可读性差且容易与
//! "实际生效的语义"脱节。写成 JS 则能直接跑、直接读、直接改。

/// 兼容前导脚本的源码。
///
/// 它是一个**立即执行的函数表达式**，不往全局留任何额外名字
/// （不留 `__tome_prelude__` 之类）——书源脚本可能 `for (var k in this)`
/// 遍历全局，多出的名字会污染它的视图。
pub const PRELUDE_JS: &str = include_str!("prelude.js");

/// 能力错误的标识（供 prelude.js 与本模块共用，避免两边写歪）。
///
/// 用**书源生态的通用叫法**（兼容性优先）：自造命名会让既有书源的
/// `catch` 判断不出来。
pub mod capabilities {
    /// 内嵌浏览器（`webView` 系列）
    pub const WEB_VIEW: &str = "webView";
    /// 无头浏览器等待（`startBrowserAwait`）
    pub const START_BROWSER_AWAIT: &str = "startBrowserAwait";
    /// 验证码识别（`getVerificationCode`）
    pub const VERIFICATION_CODE: &str = "getVerificationCode";
    /// 视频播放（`openVideoPlayer`）
    pub const VIDEO_PLAYER: &str = "openVideoPlayer";
    /// JVM 类访问（`Java.type` / `Packages.*`）——原生实现没有 JVM
    pub const JAVA_CLASS: &str = "javaClass";
    /// 宿主运行时全局（`java` 对象本身，未注入时的兜底桩）
    pub const HOST_RUNTIME: &str = "hostRuntime";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 前导脚本非空且是_utf8() {
        // `include_str!` 在编译期就会因文件缺失/非 UTF-8 而失败，
        // 这条测试守的是"文件被清空"这种更隐蔽的情况。
        assert!(
            PRELUDE_JS.len() > 500,
            "前导脚本不应为空壳，实际 {} 字节",
            PRELUDE_JS.len()
        );
        assert!(PRELUDE_JS.contains("trimLeft"), "应包含 trim 别名补齐");
        assert!(PRELUDE_JS.contains("Java"), "应包含 Java.type 桩");
    }

    #[test]
    fn 能力标识与_prelude_一致() {
        // prelude.js 里的字面量必须与本模块的常量一致，否则书源
        // 判断出的能力名与引擎上报的名字会分岔。
        for (capability, needle) in [
            (capabilities::JAVA_CLASS, "javaClass"),
            (capabilities::HOST_RUNTIME, "hostRuntime"),
        ] {
            assert_eq!(capability, needle, "常量值应与书源生态叫法一致");
            assert!(
                PRELUDE_JS.contains(needle),
                "prelude.js 应提到能力 `{needle}`（否则桩会报出另一个名字）"
            );
        }
    }

    #[test]
    fn 能力标识常量与生态叫法一致() {
        // 这四个是 T042 的桩会用到的不支持能力标识。它们必须**保持**
        // 书源生态的通用叫法（兼容性优先）：自造命名会让既有书源的
        // catch 判断不出来。此测试与 T042 的实现测试互为约束。
        assert_eq!(capabilities::WEB_VIEW, "webView");
        assert_eq!(capabilities::START_BROWSER_AWAIT, "startBrowserAwait");
        assert_eq!(capabilities::VERIFICATION_CODE, "getVerificationCode");
        assert_eq!(capabilities::VIDEO_PLAYER, "openVideoPlayer");
    }
}
