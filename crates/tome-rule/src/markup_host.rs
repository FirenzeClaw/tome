//! [DONE] `MarkupHost` 的 tome-rule 真实实现（tasks.md T039 的实现侧 / P4 遗留）
//!
//! # 这是一个**反向**桥
//!
//! `tome-rule` 依赖 `tome-js`（既定方向），而"网页标记解析"的能力需求
//! 恰好相反：JS 侧的 `java.getElement(...)` / `java.getElements(...)`
//! 要落到规则引擎的 DOM 上。
//!
//! ```text
//!   tome-js  ──需要──▶ MarkupHost（trait 定义在 tome-js）
//!      ▲                    │
//!      │ 依赖               │ 实现
//!   tome-rule ──────────────┘  （本模块）
//! ```
//!
//! trait 定义在**可被依赖的一侧**（`tome-js`），实现在**依赖它的那一侧**
//! （`tome-rule`），由组合根注入。这样架构铁律 A1 的星型依赖不被破坏
//! ——`tome-js` 依然不认识 `tome-rule`。
//!
//! # 接口是"字符串进字符串出"的代价与取舍
//!
//! trait 的签名是 `select(&self, markup: &str, rule: &str) -> Vec<String>`，
//! 故每次调用都要**解析一次 HTML**。对书源脚本的调用频率（一次抓取最多
//! 几十次选择）这个代价可忽略，而换来的是类型边界清晰。
//!
//! ## `[UNCERTAIN]` 若调用方对同一份 HTML 反复选择
//!
//! 那会重复解析。书源里的 `java.getElement(markup, rule)` 通常是
//! "一份 HTML 抽一个字段"，故当前不做缓存。若 S1-6 的真实基线显示有
//! 反复选择同一份 HTML 的脚本，应加一个按 HTML 指纹键控的小缓存
//! ——先测量再决定，而不是现在为假想的用法引入缓存与失效逻辑。
//!
//! # `MarkupHost` trait 的原文要求（实现方须遵守）
//!
//! > - `rule` 是**书源规则原文**（可含 `@css:` 前缀、后缀、索引切片、
//! >   替换链）。实现方应当直接把它交给规则引擎，而不是自己解析——
//! >   解析逻辑重复一份必然与规则引擎漂移。
//! > - 求值失败须返回 `Err`，由本模块转成 JS 异常。**不得**返回空列表
//! >   来掩盖失败。
//!
//! 两条都在 [`RuleMarkupHost::select`] 里落实。

use tome_js::api::data::MarkupHost;
use tome_js::JsError;

/// 用 [`crate::css_post::RuleEngine`] 实现 [`MarkupHost`]。
///
/// # 为什么不带 JS 运行时
///
/// 本类型被注入到 **JS 沙箱自己**（`java.*` 的宿主函数经它落到 DOM）。
/// 若它也持有一个 JS 运行时，同一个 runtime 会在自己的宿主函数里被重入
/// ——rquickjs 的 `Function::call` 期间再次求值会死锁或 panic。
///
/// 那正是"嵌套沙箱"（`HostBridge::nested_runtime`）要解决的问题，且它由
/// 组合根在**别处**处理。故本实现只用 CSS 与替换链求值（`@css:` 与
/// Default 两类），`rule` 里若含 `@js:` 会得到明确的能力错误——
/// 那比一个"死锁"好得多。
///
/// 书源里 `java.getElement(markup, rule)` 的 `rule` 参数几乎都是 CSS 规则，
/// 故此限制不影响真实书源。
#[derive(Debug, Clone, Copy, Default)]
pub struct RuleMarkupHost;

impl RuleMarkupHost {
    /// 新建（零状态，可自由共享）。
    pub fn new() -> Self {
        Self
    }
}

impl MarkupHost for RuleMarkupHost {
    fn select(&self, markup: &str, rule: &str) -> Result<Vec<String>, JsError> {
        let document = crate::dom::Document::parse_html(markup);
        // 用不带 JS 的引擎（理由见类型文档：避免 runtime 重入）。
        //
        // `RuleEngine` 支持 `@css:` / Default / `@json:` / `@XPath:` 四类
        // ——标记解析里用得到的所有类型。`@js:` 会得到结构化能力错误，
        // 而不是静默给空。
        let engine = crate::css_post::RuleEngine::new(&document);
        engine
            .evaluate(rule)
            .map_err(|error| JsError::new(format!("选择规则求值失败：{error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css规则经它落到真实_dom() {
        let host = RuleMarkupHost::new();
        let html = r#"<ul><li class="i"><a href="/1">一</a></li><li class="i"><a href="/2">二</a></li></ul>"#;
        assert_eq!(
            host.select(html, "@css:.i>a@href").expect("应求值成功"),
            vec!["/1", "/2"],
            "带 @css: 前缀的规则直接交给规则引擎"
        );
        assert_eq!(
            host.select(html, ".i>a@text").expect("应求值成功"),
            vec!["一", "二"],
            "裸选择器形态"
        );
    }

    #[test]
    fn 替换链在标记解析里也生效() {
        // trait 文档要求"直接交给规则引擎"——替换链因而自动可用
        let host = RuleMarkupHost::new();
        let html = r#"<a href="/b/1">x</a>"#;
        assert_eq!(
            host.select(html, r"a@href##^##https://x.com")
                .expect("应求值成功"),
            vec!["https://x.com/b/1"]
        );
    }

    #[test]
    fn xpath规则经它落到同一棵_dom() {
        // research R2 的"一棵 DOM，两个引擎"在这里直接可见：
        // 同一个 MarkupHost 上，CSS 与 XPath 的规则都能求值
        let host = RuleMarkupHost::new();
        let html = "<ul><li>一</li><li>二</li></ul>";
        assert_eq!(
            host.select(html, "@XPath://li").expect("应求值成功"),
            host.select(html, "@css:li").expect("应求值成功"),
        );
    }

    #[test]
    fn json_规则经它可用() {
        let host = RuleMarkupHost::new();
        let html = r#"{"list":[{"n":"a"},{"n":"b"}]}"#;
        assert_eq!(
            host.select(html, "@json:$.list[*].n").expect("应求值成功"),
            vec!["a", "b"]
        );
    }

    #[test]
    fn 规则求值失败时返回错误而非空列表() {
        // trait 文档明确：「**不得**返回空列表来掩盖失败：
        // "没有匹配"与"规则写错了"对书源作者是完全不同的信号。」
        let host = RuleMarkupHost::new();
        let error = host
            .select("<p>x</p>", "@css:div >")
            .expect_err("非法选择器应报错");
        assert!(
            error.message().contains("选择规则求值失败"),
            "错误应指明来源：{}",
            error.message()
        );
    }

    #[test]
    fn 无匹配时才给空列表() {
        let host = RuleMarkupHost::new();
        assert_eq!(
            host.select("<p>x</p>", "@css:.missing@text")
                .expect("应求值成功"),
            Vec::<String>::new(),
            "「没有匹配」是空列表，「规则写错」才是错误"
        );
    }

    #[test]
    fn 含js的规则得到明确的能力错误() {
        // 本实现不带 JS 运行时（避免 runtime 重入，见类型文档）。
        // 故 `@js:` 规则应得到**结构化能力错误**而不是死锁或静默空。
        let host = RuleMarkupHost::new();
        let error = host
            .select("<p>x</p>", "@js:1+1")
            .expect_err("应报能力错误");
        assert!(
            error.message().contains("js") || error.message().contains("JS"),
            "错误应指明是 JS 能力：{}",
            error.message()
        );
    }

    #[test]
    fn 畸形标记不崩溃() {
        // 外部 HTML 不可信（AGENTS.md 的安全红线），畸形输入是常态
        let host = RuleMarkupHost::new();
        for markup in ["", "<div>", "<<<>>>", "<a href="] {
            // 不 panic 即可；结果是什么不重要
            let _ = host.select(markup, "@css:a@href");
        }
    }
}
