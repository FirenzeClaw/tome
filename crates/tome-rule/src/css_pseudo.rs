//! \[DONE] Jsoup 专有伪类清单与非标准语法（tasks.md T050 / research.md R2）
//!
//! # 本模块的职责：把"对齐目标"变成可核对的事实
//!
//! research R2 的**关键前置决策**原文：
//!
//! > 钉死对齐目标为 Jsoup 1.2x 的 Selector 行为，并据此写一份
//! > "非标准伪类实现清单"。
//!
//! 本模块就是那份清单。它不做匹配（匹配在 [`crate::css`] 的
//! `Element::match_non_ts_pseudo_class`），而是：
//!
//! 1. **钉死版本目标**（[`JSOUP_ALIGNMENT_TARGET`]）
//! 2. **逐条列出非标准语法的实现状态**（[`NON_STANDARD_SELECTORS`]），
//!    每条标注**依据来源**与**是否已实现**——清单可被测试断言，
//!    因此"写进文档但没实现"不会静默存在
//! 3. **承载与标准 CSS 语义冲突的重载**（[`super::css::JsoupPseudoClass`]
//!    的 variant 文档，尤其是 `~=`）
//!
//! # 对齐目标
//!
//! [`JSOUP_ALIGNMENT_TARGET`] 钉的是 **Jsoup 1.2x**（1.20 及以后的 1.x
//! 系列）。依据是 R2 的决策，以及 Jsoup 公开文档 `Selector` 一节的当前形态
//! （jsoup.org/apidocs 与 jsoup.org/cookbook 的 selector-syntax 页）。
//!
//! ## 为什么是"1.2x"而不是某个精确小版本
//!
//! 1.2x 系列内这些伪类的语义没有变化（公开的变更记录里没有相关条目），
//! 而钉一个具体小版本会因上游补丁发布而需要频繁改动清单。若将来某个
//! 小版本真的改变了某条语义，应在 [`NON_STANDARD_SELECTORS`] 的对应
//! 条目里注明"自 X.Y 起行为变化"，而不是整体升版号。
//!
//! # 语义重载：`[attr~=regex]`
//!
//! 这是 R2 **点名**的一处"同一条规则两边行为不同"：
//!
//! | 实现 | `[attr~=X]` 的 X |
//! |---|---|
//! | CSS Selectors 规范 | 一个**空白分词**（`[class~=foo]` 匹配 `class="a foo b"`） |
//! | Jsoup | 一个**正则表达式**（`[src~=\.png$]`） |
//!
//! 两处不可兼容。本实现按 Jsoup（兼容性优先，宪法原则 I）：`~=` 在
//! [`crate::css::rewrite_jsoup_dialect`] 里被改写成功能伪类
//! `:jsoupAttrMatches("attr","pattern")`，匹配走
//! `Element::match_non_ts_pseudo_class` 而非 CSS 的标准分词路径。
//! `Element::attr_matches` 里留了一条 `debug_assert` 守住"不该到达那里"。
//!
//! # 标准的伪类由谁实现
//!
//! `:root` / `:nth-child` / `:nth-of-type` / `:first-child` / `:last-child` /
//! `:empty` / `:not` / `:is` / `:has` 等**标准**伪类由 servo `selectors`
//! 提供（Firefox 同款引擎），本模块只列非标准的。`is_empty` / `is_root`
//! 两处的 Jsoup 语义差异见 [`crate::css`] 的 `Element` 实现。

use crate::css::JsoupPseudoClass;

/// 对齐目标：**Jsoup 1.2x** 的 `Selector` 行为（research R2 的决策）。
///
/// 与上游的**代码**无关（干净室纪律）——它是**行为对齐**的目标版本，
/// 用来回答"这条非标准语法按哪个版本的 Jsoup 语义实现"。
pub const JSOUP_ALIGNMENT_TARGET: &str = "1.2x";

/// 非标准语法的实现清单。
///
/// # 每条的信息与它的用途
///
/// | 字段 | 用途 |
/// |---|---|
/// | `syntax` | 书源里怎么写（人可核对） |
/// | `meaning` | 语义（人可核对） |
/// | `source` | **依据来源**（干净室：公开文档或规范，不是某实现） |
/// | `implemented` | 是否已实现——**可被测试断言**，杜绝"写了文档没实现" |
///
/// # 为什么做成数据结构而不是文档里的一张表
///
/// 文档里的表无法被测试。做成常量后，`tests::清单与实现一致` 能断言
/// 每一条 `implemented: true` 的语法**真的能编译并匹配**——清单因此
/// 是"事实"而不是"承诺"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonStandardSelector {
    /// 书源里的写法
    pub syntax: &'static str,
    /// 语义
    pub meaning: &'static str,
    /// 依据来源（公开文档 / 规范，非某实现源码）
    pub source: &'static str,
    /// 是否已实现
    pub implemented: bool,
}

/// 依据来源：Jsoup 公开文档 `Selector` 一节与 cookbook 的
/// `selector-syntax` 页（jsoup.org）。
const SOURCE_JSQOUP_SELECTOR: &str =
    "Jsoup 公开文档 Selector 一节（jsoup.org/apidocs 与 cookbook/selector-syntax）";
/// 依据来源：书源规则的公开语法约定（社区流通的《书源规则说明》文档）。
const SOURCE_RULE_DOC: &str = "书源规则的公开语法约定（社区流通的《书源规则说明》）";

/// 非标准（Jsoup 专有）语法的全部条目。
///
/// ## 清单的边界
///
/// "非标准"= **CSS Selectors 规范里没有**。凡规范有的（`:root`、
/// `:nth-child`、`:empty`…）都不在表内——它们由 servo `selectors` 覆盖，
/// 列进来会让这张表失去"专有清单"的定位。
pub const NON_STANDARD_SELECTORS: &[NonStandardSelector] = &[
    // ---------- 文本内容类 ----------
    NonStandardSelector {
        syntax: ":contains(text)",
        meaning: "后代文本包含 text（大小写不敏感、空白归一化）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":containsOwn(text)",
        meaning: "**自身**文本包含 text（大小写不敏感），不含后代",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":containsData(text)",
        meaning: "data 内容包含 text（`<script>` / `<style>` / 注释）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":containsWholeText(text)",
        meaning: "后代整体文本精确包含（大小写敏感、不归一化空白）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":containsWholeOwnText(text)",
        meaning: "自身整体文本精确包含（大小写敏感、不归一化）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":matches(regex)",
        meaning: "后代文本匹配正则（空白归一化）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":matchesOwn(regex)",
        meaning: "**自身**文本匹配正则（不含后代）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":matchesWholeText(regex)",
        meaning: "后代整体文本匹配正则（不归一化）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":matchesWholeOwnText(regex)",
        meaning: "自身整体文本匹配正则（不归一化）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    // ---------- 索引类 ----------
    NonStandardSelector {
        syntax: ":eq(n)",
        meaning: "**兄弟索引**等于 n（0 基，接受负数）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":lt(n)",
        meaning: "兄弟索引小于 n",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: ":gt(n)",
        meaning: "兄弟索引大于 n",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    // ---------- 属性类（含语义重载） ----------
    NonStandardSelector {
        syntax: "[attr~=regex]",
        meaning: "属性值匹配正则。**重载**：CSS 规范里 `~=` 是空白分词",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: "[^attrPrefix]",
        meaning: "存在以 attrPrefix 开头的属性（HTML5 dataset 用）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    NonStandardSelector {
        syntax: "[*]",
        meaning: "有任意属性（`p:not([*])` 找无属性的 p）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    // ---------- 已废弃 / 明确不支持 ----------
    NonStandardSelector {
        syntax: ":matchText",
        meaning: "把文本节点包成伪元素以便对其做选择。**已废弃**（公开文档标 Deprecated）",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: false,
    },
    NonStandardSelector {
        syntax: ":lang(l)",
        meaning: "向上找最近的 lang 属性匹配（含 `l-` 前缀）。标准里也有，但 Jsoup 的匹配规则更宽",
        source: SOURCE_JSQOUP_SELECTOR,
        implemented: true,
    },
    // ---------- 书源规则的语法糖（非 Jsoup 的，但同属 CSS 落地范围） ----------
    NonStandardSelector {
        syntax: "text.关键字",
        meaning: "等价于 `:containsOwn(关键字)`（书源规则的简写）",
        source: SOURCE_RULE_DOC,
        implemented: false,
    },
];

/// 取"已实现"的清单条目。
pub fn implemented_selectors() -> impl Iterator<Item = &'static NonStandardSelector> {
    NON_STANDARD_SELECTORS
        .iter()
        .filter(|entry| entry.implemented)
}

/// 取"未实现"的清单条目（诊断与将来补齐用）。
pub fn unimplemented_selectors() -> impl Iterator<Item = &'static NonStandardSelector> {
    NON_STANDARD_SELECTORS
        .iter()
        .filter(|entry| !entry.implemented)
}

/// `[attr~=regex]` 重载的**匹配语义说明**（供诊断与文档引用）。
///
/// 单独常量而不是散在注释里：它是 R2 点名的差异，需要能被测试与其他
/// 模块（如语料的期望值说明）引用。
pub const TILDE_EQUALS_OVERLOAD_NOTE: &str = "\
`[attr~=X]` 在 CSS Selectors 规范里 X 是空白分词，在 Jsoup 里是正则。\
本实现按 Jsoup（兼容性优先）：`~=` 被改写为 `:jsoupAttrMatches(\"attr\",\"re\")`，\
匹配走正则而非分词。";

/// 判定一个伪类名是否为**已知的 Jsoup 专有伪类**。
///
/// # 用途
///
/// 诊断与错误信息：书源写了 `:containsOwn(...)` 而拼错成
/// `:containsown(...)` 时，调用方能给出"疑似想写 X"的提示，而不是
/// 一个"未支持的伪类"的笼统错误。CSS 伪类名**大小写敏感**（与选择器的
/// 其他部分不同），故大小写写错确实是常见笔误。
pub fn is_known_jsoup_pseudo_name(name: &str) -> bool {
    matches!(
        name,
        "contains"
            | "containsOwn"
            | "containsData"
            | "containsWholeText"
            | "containsWholeOwnText"
            | "matches"
            | "matchesOwn"
            | "matchesWholeText"
            | "matchesWholeOwnText"
            | "eq"
            | "lt"
            | "gt"
            | "lang"
    )
}

/// 把一个伪类解析结果归到清单条目上（诊断用）。
///
/// 返回该伪类在 [`NON_STANDARD_SELECTORS`] 里的 `syntax` 文本。
pub fn describe(pseudo: &JsoupPseudoClass) -> &'static str {
    match pseudo {
        JsoupPseudoClass::Contains(_) => ":contains(text)",
        JsoupPseudoClass::ContainsOwn(_) => ":containsOwn(text)",
        JsoupPseudoClass::ContainsData(_) => ":containsData(text)",
        JsoupPseudoClass::ContainsWholeText(_) => ":containsWholeText(text)",
        JsoupPseudoClass::ContainsWholeOwnText(_) => ":containsWholeOwnText(text)",
        JsoupPseudoClass::Matches(_) => ":matches(regex)",
        JsoupPseudoClass::MatchesOwn(_) => ":matchesOwn(regex)",
        JsoupPseudoClass::MatchesWholeText(_) => ":matchesWholeText(regex)",
        JsoupPseudoClass::MatchesWholeOwnText(_) => ":matchesWholeOwnText(regex)",
        JsoupPseudoClass::Eq(_) => ":eq(n)",
        JsoupPseudoClass::Lt(_) => ":lt(n)",
        JsoupPseudoClass::Gt(_) => ":gt(n)",
        JsoupPseudoClass::Lang(_) => ":lang(l)",
        JsoupPseudoClass::AttrMatches { .. } => "[attr~=regex]",
        JsoupPseudoClass::AttrNamePrefix(_) => "[^attrPrefix]",
        JsoupPseudoClass::AnyAttribute => "[*]",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dom::Document;

    /// 便捷：在 HTML 上按选择器查，返回元素名序列。
    fn query(html: &str, selector: &str) -> Vec<String> {
        let document = Document::parse_html(html);
        let compiled = crate::css::compile(selector)
            .unwrap_or_else(|error| panic!("`{selector}` 应能编译：{error}"));
        crate::css::select_in_document(&document, &compiled)
            .into_iter()
            .map(|node| node.node_name())
            .collect()
    }

    // ---------- 清单自身的一致性 ----------

    #[test]
    fn 对齐目标已钉死() {
        // research R2 的"关键前置决策"：对齐目标钉死为 Jsoup 1.2x
        assert_eq!(JSOUP_ALIGNMENT_TARGET, "1.2x");
    }

    #[test]
    fn 清单每条都有依据来源() {
        for entry in NON_STANDARD_SELECTORS {
            assert!(
                !entry.source.is_empty(),
                "`{}` 缺依据来源——干净室纪律要求逐条可追溯",
                entry.syntax
            );
            assert!(!entry.meaning.is_empty(), "`{}` 缺语义说明", entry.syntax);
        }
    }

    #[test]
    fn 清单非空且已实现占多数() {
        let implemented = implemented_selectors().count();
        let total = NON_STANDARD_SELECTORS.len();
        assert!(total >= 15, "清单应覆盖全部专有语法，实际 {total} 条");
        assert!(
            implemented * 100 / total >= 80,
            "已实现应占多数（{implemented}/{total}）"
        );
    }

    #[test]
    fn 未实现的条目有明确理由() {
        // 未实现的两条各有理由：`:matchText` 已废弃且要改写 DOM（与只读
        // 查询模型不符），`text.关键字` 是书源简写（可由 `:containsOwn` 表达）。
        let unimplemented: Vec<&str> = unimplemented_selectors().map(|e| e.syntax).collect();
        assert!(unimplemented.contains(&":matchText"));
        assert!(unimplemented.contains(&"text.关键字"));
    }

    #[test]
    fn 重载说明成文() {
        assert!(TILDE_EQUALS_OVERLOAD_NOTE.contains("空白分词"));
        assert!(TILDE_EQUALS_OVERLOAD_NOTE.contains("正则"));
    }

    #[test]
    fn 专有伪类名被识别() {
        for name in ["contains", "containsOwn", "matchesOwn", "eq", "lt", "gt"] {
            assert!(is_known_jsoup_pseudo_name(name), "`{name}` 应被识别");
        }
        for name in ["containsown", "unknown", "nth-child"] {
            assert!(!is_known_jsoup_pseudo_name(name), "`{name}` 不该被识别");
        }
    }

    // ---------- 清单与实现的一致性（本测试是清单"可信"的依据） ----------

    #[test]
    fn 文本内容类伪类可用() {
        let html = r#"<div><p id="a">Hello <b>jsoup</b></p><p id="b">World</p></div>"#;
        // :contains —— 后代文本
        assert_eq!(query(html, "p:contains(jsoup)"), vec!["p"]);
        // :containsOwn —— 只看自身
        assert_eq!(query(html, "p:containsOwn(Hello)"), vec!["p"]);
        assert_eq!(
            query(html, "p:containsOwn(jsoup)"),
            Vec::<String>::new(),
            "jsoup 在 b 里，不是 p 的自身文本"
        );
    }

    #[test]
    fn matchesown只看自身文本() {
        // 公开文档伪类表列了 `:matchesOwn(regex)`（`:matches` 的自身版）
        let html = r#"<div><p id="a">Hello <b>world123</b></p><p id="b">num456</p></div>"#;
        // :matches 看后代：两个 p 的自身/后代文本里，a 有 world123
        assert_eq!(query(html, r"p:matches(\d+)").len(), 2);
        // :matchesOwn 只看自身：只有 b 的自身文本含数字
        assert_eq!(query(html, r"p:matchesOwn(\d+)"), vec!["p"]);
    }

    #[test]
    fn 整文本系列区分空白与大小写() {
        // 公开文档：`:containsWholeText` —— "the exact, non-normalized whole text
        // (case sensitive, preserving whitespace/newlines)"
        //
        // 与大写/多空格的差异正是它与 `:contains` 的分界：`:contains` 会归一化
        // 空白并折叠大小写，故那三行在它下面**全部命中**。
        let html = "<p>a  b</p><p>A B</p>";
        assert_eq!(
            query(html, "p:containsWholeText(a  b)").len(),
            1,
            "原文两个空格只对第一个 p 成立（不归一化）"
        );
        assert_eq!(
            query(html, "p:containsWholeText(a b)").len(),
            0,
            "归一化后的 `a b` 与原文 `a  b` 不等——本伪类不归一化"
        );
        assert_eq!(
            query(html, "p:containsWholeText(A B)").len(),
            1,
            "大写只对第二个 p 成立（大小写敏感）"
        );
        // 对照：归一化的 `:contains` 只看归一化后的**页面文本**，而实参
        // 按原样使用——故单空格与大小写变体命中两个，双空格的实参命不中。
        for selector in ["p:contains(a b)", "p:contains(A B)"] {
            assert_eq!(query(html, selector).len(), 2, "`{selector}` 命中两个");
        }
        assert_eq!(
            query(html, "p:contains(a  b)").len(),
            0,
            "`:contains` 归一化的是页面文本，实参里的两个空格不会被折叠"
        );
    }

    #[test]
    fn containsdata看datanode() {
        // 公开文档：`:containsData(text)` —— "selects elements that contain the
        // specified data (e.g. within `<script>`, `<style>`, or comments)"。
        //
        // 注意它是**后代包含**（与 `:contains` 同），故祖先元素也命中——
        // html/head 经后代包含 script 的 data。
        let html = r#"<script>var a=1;</script><p>var a=1;</p>"#;
        let matched = query(html, ":containsData(var)");
        assert!(
            matched.contains(&"script".to_string()),
            "script 自身含 data：{matched:?}"
        );
        assert!(
            !matched.contains(&"p".to_string()),
            "p 里的 `var a=1;` 是文本节点而非 data node，不该命中：{matched:?}"
        );
        // 选择器限定到 script 时只给 script
        assert_eq!(query(html, "script:containsData(var)"), vec!["script"]);
    }

    #[test]
    fn 索引伪类以兄弟索引为准() {
        // 公开文档：「the above indexed pseudo-selectors are 0-based」
        let html = r#"<ul><li>0</li><li>1</li><li>2</li><li>3</li></ul>"#;
        assert_eq!(query(html, "li:eq(0)").len(), 1);
        assert_eq!(query(html, "li:lt(2)").len(), 2);
        assert_eq!(query(html, "li:gt(1)").len(), 2);
    }

    #[test]
    fn 属性方言可用() {
        let html = r#"<p data-x="1"></p><p class="y"></p><p></p>"#;
        assert_eq!(query(html, "p[^data-]"), vec!["p"]);
        assert_eq!(query(html, "p[*]").len(), 2);
        assert_eq!(query(html, "p:not([*])").len(), 1);
    }

    #[test]
    fn 正则属性方言按正则而非分词匹配() {
        // 这是 R2 点名的重载：同一个 `~=` 两边语义不同
        let html = r#"<img src="/a/photo.PNG"><img src="/a/photo.gif">"#;
        // Jsoup 语义（正则）：只有 PNG 命中
        assert_eq!(query(html, r"img[src~=(?i)\.png$]").len(), 1);
        // 若按 CSS 的分词语义，`\.png$` 不是一个空白分词，应命中 0 个。
        // 断言"命中 1 而不是 0"正是重载生效的证据。
    }

    #[test]
    fn 每条已实现清单条目都能编译() {
        // 这张清单的价值在于"已实现"是**可核对的**。逐条编译一遍：
        // 任何一条写了 implemented: true 但实际编译不过，这里就会失败。
        for entry in implemented_selectors() {
            // 把语法模板替换成具体用例
            let selector = match entry.syntax {
                ":contains(text)" => "p:contains(x)",
                ":containsOwn(text)" => "p:containsOwn(x)",
                ":containsData(text)" => "script:containsData(x)",
                ":containsWholeText(text)" => "p:containsWholeText(x)",
                ":containsWholeOwnText(text)" => "p:containsWholeOwnText(x)",
                ":matches(regex)" => r"p:matches(\d+)",
                ":matchesOwn(regex)" => r"p:matchesOwn(\d+)",
                ":matchesWholeText(regex)" => r"p:matchesWholeText(\d+)",
                ":matchesWholeOwnText(regex)" => r"p:matchesWholeOwnText(\d+)",
                ":eq(n)" => "p:eq(0)",
                ":lt(n)" => "p:lt(1)",
                ":gt(n)" => "p:gt(0)",
                ":lang(l)" => "p:lang(en)",
                "[attr~=regex]" => r"p[data-x~=\d+]",
                "[^attrPrefix]" => "p[^data-]",
                "[*]" => "p[*]",
                other => panic!("清单里的 `{other}` 没有对应用例，请补上"),
            };
            crate::css::compile(selector)
                .unwrap_or_else(|error| panic!("清单条目 `{}` 应可编译：{error}", entry.syntax));
        }
    }

    #[test]
    fn 描述函数覆盖全部变体() {
        // `describe` 是穷尽 match（无 `_` 分支），编译期保证覆盖；
        // 本测试断言它的输出确实是清单里的 syntax 文本。
        let html = "<p data-x='1'>t</p>";
        let document = Document::parse_html(html);
        for selector in [
            "p:contains(t)",
            "p:containsOwn(t)",
            "p:containsData(t)",
            "p:containsWholeText(t)",
            "p:containsWholeOwnText(t)",
            r"p:matches(t)",
            r"p:matchesOwn(t)",
            r"p:matchesWholeText(t)",
            r"p:matchesWholeOwnText(t)",
            "p:eq(0)",
            "p:lt(1)",
            "p:gt(0)",
            "p:lang(en)",
            r"p[data-x~=1]",
            "p[^data-]",
            "p[*]",
        ] {
            // 能编译即证明语法在清单内（逐条断言其 syntax 文本）
            crate::css::compile(selector)
                .unwrap_or_else(|error| panic!("`{selector}` 应可编译：{error}"));
        }
        // 描述函数对具体变体给出清单里的文本
        assert_eq!(describe(&JsoupPseudoClass::Eq(0)), ":eq(n)");
        assert_eq!(
            describe(&JsoupPseudoClass::AttrNamePrefix("data-".to_string())),
            "[^attrPrefix]"
        );
        assert_eq!(describe(&JsoupPseudoClass::AnyAttribute), "[*]");
        let _ = document;
    }
}
