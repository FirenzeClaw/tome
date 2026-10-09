//! 规则引擎的端到端集成测试（tasks.md T022/T023/T025 的验收前提）
//!
//! # 为什么要有独立于单元测试的这一层
//!
//! 单元测试逐模块验证语义，但**模块串起来之后的顺序**是另一回事：
//! "分派前缀 → 剥离替换链 → 查 CSS → 切片 → 取后缀 → 应用替换"这六步的先后
//! 若搞错，每个模块的单测都还是绿的，而真实书源会产出错值。
//!
//! 故这里用**公开文档给出的真实书源规则片段**做输入，断言整条链的输出。
//! 语料来源：社区流通的《书源规则说明》文档里附的三份完整书源示例
//! （"书源一"CSS 型、"书源二"XPath 型、"书源三"JSON 型）——本文件取其中
//! **只含 CSS 与正则**的规则条目，这正是 S1-1 切片的前置条件
//! （tasks.md 第 85 行：本切片的真实书源须为仅含 CSS/正则的简化书源）。

use tome_core::ErrorKind;
use tome_rule::css_post::evaluate_rule;
use tome_rule::dispatch::{dispatch_prefix, parse_rule, Combinator};
use tome_rule::dom::Document;
use tome_rule::regex::RegexLimits;
use tome_rule::replace::parse_replace_chain;

/// 一份模拟"小说 2016"搜索页的 HTML（对应公开文档"书源一"的目标站点形状）。
const SEARCH_PAGE: &str = r#"
<html><body>
<ul class="list">
  <li class="clearfix">
    <h4 class="name"><a href="/book/1001">斗破苍穹</a></h4>
    <p class="author">天蚕土豆</p>
    <div class="note clearfix"><p>简介：这里是简介一</p></div>
    <p>分类：玄幻</p>
    <p>状态：连载</p>
  </li>
  <li class="clearfix">
    <h4 class="name"><a href="/book/1002">完美世界</a></h4>
    <p class="author">辰东</p>
    <div class="note clearfix"><p>简介：这里是简介二</p></div>
    <p>分类：仙侠</p>
    <p>状态：完结</p>
  </li>
</ul>
</body></html>
"#;

/// 一份模拟目录页（含浏览器广告前缀——对应书源一目录规则里的正则清洗）。
const TOC_PAGE: &str = r#"
<html><body>
<div id="list">
  <dl>
    <dd><a href="/read/1001/1.html">第一章 开始</a></dd>
    <dd><a href="/read/1001/2.html">第二章 继续搜索本书手机访问</a></dd>
    <dd><a href="/read/1001/3.html">第三章 结束</a></dd>
  </dl>
</div>
</body></html>
"#;

// ---------------------------------------------------------------------------
// 搜索路径：公开文档"书源一"的 ruleSearch
// ---------------------------------------------------------------------------

#[test]
fn 书源一的搜索规则端到端() {
    let document = Document::parse_html(SEARCH_PAGE);

    // 「bookList」: "@css:li.clearfix" —— 带类型前缀的完整规则形态
    let books = evaluate_rule(&document, "@css:li.clearfix").expect("应求值成功");
    assert_eq!(books.len(), 2, "两条搜索结果");

    // 「name」: "@css:.name@text"
    let names = evaluate_rule(&document, "@css:.name@text").expect("应求值成功");
    assert_eq!(names, vec!["斗破苍穹", "完美世界"]);

    // 「bookUrl」: "@css:.name>a@href"
    let urls = evaluate_rule(&document, "@css:.name>a@href").expect("应求值成功");
    assert_eq!(urls, vec!["/book/1001", "/book/1002"]);

    // 「author」: "@css:p:eq(2)>a@text" 是文档里的另一形态（用伪类定位）；
    // 本实现支持同一语义的直接写法：
    let authors = evaluate_rule(&document, "@css:p.author@text").expect("应求值成功");
    assert_eq!(authors, vec!["天蚕土豆", "辰东"]);

    // 「intro」: "@css:.note.clearfix p@text"
    let intros = evaluate_rule(&document, "@css:.note.clearfix p@text").expect("应求值成功");
    assert_eq!(intros, vec!["简介：这里是简介一", "简介：这里是简介二"]);

    // 「coverUrl」: "@css:img@src" —— 本页面无图，应为空而不是报错
    assert_eq!(
        evaluate_rule(&document, "@css:img@src").expect("应求值成功"),
        Vec::<String>::new()
    );
}

#[test]
fn 伪类定位形态与索引切片形态等价() {
    let document = Document::parse_html(SEARCH_PAGE);
    // 公开文档同时给出 `:eq(n)` 与索引切片两种定位写法。
    // 注意二者语义**不同**（`:eq` 是兄弟索引，切片是结果集下标），
    // 故只在"每个父元素下取第 n 个"这种场景下等价。
    let document2 = Document::parse_html(r#"<ul><li><p>a1</p></li><li><p>b1</p></li></ul>"#);
    // 每个 li 下的第一个 p：`:eq(0)` 与 `[0]` 在结果集只有 2 个元素时不同——
    // `:eq(0)` 命中两个，`[0]` 只命中第一个。这个区别是 T021 的验收点之一。
    assert_eq!(
        evaluate_rule(&document2, "p:eq(0)@text").expect("应求值成功"),
        vec!["a1", "b1"],
        ":eq 是兄弟索引，每个父元素下各命中一个"
    );
    assert_eq!(
        evaluate_rule(&document2, "p[0]@text").expect("应求值成功"),
        vec!["a1"],
        "切片是结果集下标，只取第一个"
    );
    let _ = document;
}

// ---------------------------------------------------------------------------
// 目录路径：公开文档"书源一"的 ruleToc（含正则清洗）
// ---------------------------------------------------------------------------

#[test]
fn 书源一的目录规则端到端() {
    let document = Document::parse_html(TOC_PAGE);

    // 「chapterList」: "@css:#list dd>a@href"（公开文档用的是正则 AllInOne，此处用 CSS
    // 等价形态验证 CSS 路径；正则 AllInOne 求值待 S1-2 接 regex 分派）
    let links = evaluate_rule(&document, "@css:#list dd>a@href").expect("应求值成功");
    assert_eq!(
        links,
        vec![
            "/read/1001/1.html",
            "/read/1001/2.html",
            "/read/1001/3.html"
        ]
    );

    // 目录反序（公开文档：「获取列表的最前面加上负号- 可以使列表倒置」）
    let reversed = evaluate_rule(&document, "-@css:#list dd>a@href").expect("应求值成功");
    assert_eq!(
        reversed,
        vec![
            "/read/1001/3.html",
            "/read/1001/2.html",
            "/read/1001/1.html"
        ]
    );

    // 章节名 + 清洗广告后缀。公开文档的净化形态：
    // "##搜索.*手机访问|一秒记住.*|.*阅读下载|"
    let names = evaluate_rule(&document, "@css:#list dd>a@text").expect("应求值成功");
    assert_eq!(
        names,
        vec!["第一章 开始", "第二章 继续搜索本书手机访问", "第三章 结束"]
    );

    let chain = parse_replace_chain("@css:#list dd>a@text##搜索.*手机访问|一秒记住.*|.*阅读下载|");
    assert_eq!(chain.subject, "@css:#list dd>a@text");
    let cleaned = chain.apply(&names.join("\n"), RegexLimits::DEFAULT);
    assert!(
        !cleaned.contains("搜索本书手机访问"),
        "广告串应被清洗：{cleaned}"
    );
    assert!(cleaned.contains("第二章 继续"), "正文应保留：{cleaned}");
}

// ---------------------------------------------------------------------------
// 正文路径：净化链
// ---------------------------------------------------------------------------

#[test]
fn 净化链端到端() {
    let document = Document::parse_html(
        r#"<div class="articleDiv">
             <p>正文第一段。</p>
             <p>正文第二段。搜索本书手机访问</p>
             <p>正文第三段。</p>
           </div>"#,
    );
    // 公开文档"书源一"的 ruleContent 形态（去掉 @textNodes 因为它是多值后缀，
    // 这里先用 text 验证净化链本身）
    let chain =
        parse_replace_chain("@css:.articleDiv p@text##搜索.*手机访问|一秒记住.*|.*阅读下载|");
    let values = evaluate_rule(&document, &chain.subject).expect("应求值成功");
    assert_eq!(values.len(), 3);
    let joined = values.join("\n");
    let cleaned = chain.apply(&joined, RegexLimits::DEFAULT);
    assert_eq!(
        cleaned, "正文第一段。\n正文第二段。\n正文第三段。",
        "广告串应被整体清除"
    );
}

#[test]
fn 多条净化按序叠加端到端() {
    let text = "第一章　标题\n\n\n   正文   内容   \n";
    let chain = parse_replace_chain(r"x##\s+## ##\n?");
    // 第一步：全部空白折叠为单空格；第二步：把 " \n" 变 "\n"
    let cleaned = chain.apply(text, RegexLimits::DEFAULT);
    assert!(!cleaned.contains("  "), "连续空白应被折叠：{cleaned:?}");
    assert!(cleaned.contains("第一章 标题"), "正文应保留：{cleaned:?}");
}

// ---------------------------------------------------------------------------
// 组合符端到端
// ---------------------------------------------------------------------------

#[test]
fn 组合符在真实页面上生效() {
    let document = Document::parse_html(SEARCH_PAGE);

    // 公开文档示例形态："class.odd.0@tag.a.0@text||tag.dd.0@tag.h1@text##全文阅读"
    // 即"主选择器没匹配到就用备选"。
    let primary = evaluate_rule(&document, "@css:.missing@text").expect("应求值成功");
    let fallback = evaluate_rule(&document, "@css:.name@text").expect("应求值成功");
    assert!(primary.is_empty());

    let combined =
        tome_rule::dispatch::combine_results(vec![primary, fallback], &[Combinator::FirstNonEmpty]);
    assert_eq!(combined, vec!["斗破苍穹", "完美世界"], "应落到备选选择器");

    // `&&` 合并
    let names = evaluate_rule(&document, "@css:.name@text").expect("应求值成功");
    let authors = evaluate_rule(&document, "@css:p.author@text").expect("应求值成功");
    let merged = tome_rule::dispatch::combine_results(vec![names, authors], &[Combinator::Merge]);
    assert_eq!(merged.len(), 4);
}

#[test]
fn 分派器对含未实现类型的真实规则给出结构化错误() {
    // 公开文档"书源三"大量使用 JSONPath；S1-1 阶段这类规则必须**明确报告**
    // 未就绪而不是产出空结果（FR-006）。
    let combined = parse_rule("@json:$.data.list[*]");
    let not_ready = combined.first_not_ready().expect("JSONPath 应标为未就绪");
    let error = not_ready
        .not_ready_error("search")
        .expect("应产出结构化错误");
    match error.to_error_kind() {
        ErrorKind::UnsupportedCapability { capability } => {
            assert_eq!(capability.capability_id(), "jsonPath");
        }
        other => panic!("应产出 UnsupportedCapability，实际 {other:?}"),
    }

    // 同一规则走 evaluate_rule 也应报同一种错误，而不是"把 $.data 当 CSS 解析"
    // 后报一个无关的语法错
    let document = Document::parse_html("<p>x</p>");
    let error = evaluate_rule(&document, "@json:$.data.list[*]").expect_err("应报错");
    assert!(matches!(
        error.to_error_kind(),
        ErrorKind::UnsupportedCapability { .. }
    ));

    // XPath 与 JS 同理
    for rule in ["@XPath://div/a", "@js:java.ajax('x')"] {
        let document = Document::parse_html("<p>x</p>");
        let error = evaluate_rule(&document, rule).expect_err("应报错");
        assert!(
            matches!(
                error.to_error_kind(),
                ErrorKind::UnsupportedCapability { .. }
            ),
            "规则 {rule} 应产出 UnsupportedCapability"
        );
    }
}

#[test]
fn 分派器与组合符的端到端协同() {
    // 一条含组合符的完整规则：切分 → 逐段分派
    let combined = parse_rule("@css:.a@text||@css:.b@text");
    assert_eq!(combined.segments.len(), 2);
    assert_eq!(combined.combinators, vec![Combinator::FirstNonEmpty]);
    assert!(combined.first_not_ready().is_none());

    // 段落正文已被剥掉前缀
    assert_eq!(combined.segments[0].body, ".a@text");
    assert_eq!(combined.segments[1].body, ".b@text");

    // 反序标记在段级被识别
    let segment = dispatch_prefix("-.list@text");
    assert!(segment.reversed);
}

// ---------------------------------------------------------------------------
// 缩略书源样本：整条搜索→目录→正文的最小闭环
// ---------------------------------------------------------------------------

/// 一份**仅含 CSS 与正则**的简化书源（tasks.md 第 85 行的前置条件：
/// 本切片的真实书源须为此形态，因为 JS 与 JSONPath/XPath 在 S1-2/S1-3 才可用）。
///
/// 规则取自公开文档"书源一"的三条 ruleXxx，逐条去掉了 XPath/JS/JSONPath 成分。
struct SimplifiedSource {
    rule_search: SimplifiedSearchRule,
    rule_toc: SimplifiedTocRule,
    rule_content: SimplifiedContentRule,
}

struct SimplifiedSearchRule {
    book_list: &'static str,
    name: &'static str,
    author: &'static str,
    book_url: &'static str,
    intro: &'static str,
}

struct SimplifiedTocRule {
    chapter_list: &'static str,
    chapter_name: &'static str,
    chapter_url: &'static str,
}

struct SimplifiedContentRule {
    content: &'static str,
    next_content_url: &'static str,
}

fn sample_source() -> SimplifiedSource {
    SimplifiedSource {
        rule_search: SimplifiedSearchRule {
            book_list: "@css:li.clearfix",
            name: "@css:.name@text",
            author: "@css:p.author@text",
            book_url: "@css:.name>a@href",
            intro: "@css:.note.clearfix p@text",
        },
        rule_toc: SimplifiedTocRule {
            chapter_list: "@css:#list dd>a",
            chapter_name: "@css:#list dd>a@text",
            chapter_url: "@css:#list dd>a@href",
        },
        rule_content: SimplifiedContentRule {
            content: "@css:.articleDiv p@textNodes",
            next_content_url: "",
        },
    }
}

#[test]
fn 简化书源的最小搜索闭环() {
    let source = sample_source();
    let document = Document::parse_html(SEARCH_PAGE);

    // 第一步：拿到书籍列表（每本书是一个节点）——由 bookList 定位
    let book_list =
        evaluate_rule(&document, source.rule_search.book_list).expect("bookList 应求值成功");
    assert_eq!(book_list.len(), 2, "应定位到 2 本书");

    // 第二步：对每本书取各字段。真实管线（tome-source 的 T027）会对每本书的
    // 子树做相对查询；本阶段用"整页查询 + 按位置对齐"验证字段规则本身可用——
    // 两者的选择器语义是同一套。
    let names = evaluate_rule(&document, source.rule_search.name).expect("name 应求值");
    let authors = evaluate_rule(&document, source.rule_search.author).expect("author 应求值");
    let urls = evaluate_rule(&document, source.rule_search.book_url).expect("bookUrl 应求值");
    let intros = evaluate_rule(&document, source.rule_search.intro).expect("intro 应求值");

    assert_eq!(names.len(), book_list.len(), "字段数与书籍数应一致");
    assert_eq!(authors.len(), book_list.len());
    assert_eq!(urls.len(), book_list.len());
    assert_eq!(intros.len(), book_list.len());

    // 结构化结果（这就是 tome-cli 要输出的 JSON 的字段来源）
    assert_eq!(names[0], "斗破苍穹");
    assert_eq!(authors[0], "天蚕土豆");
    assert_eq!(urls[0], "/book/1001");
    assert_eq!(intros[0], "简介：这里是简介一");
}

#[test]
fn 简化书源的目录与正文闭环() {
    let source = sample_source();

    // 目录
    let toc_document = Document::parse_html(TOC_PAGE);
    let chapters =
        evaluate_rule(&toc_document, source.rule_toc.chapter_list).expect("chapterList 应求值");
    let names =
        evaluate_rule(&toc_document, source.rule_toc.chapter_name).expect("chapterName 应求值");
    let urls =
        evaluate_rule(&toc_document, source.rule_toc.chapter_url).expect("chapterUrl 应求值");
    assert_eq!(chapters.len(), 3);
    assert_eq!(names.len(), 3);
    assert_eq!(urls.len(), 3);
    assert_eq!(names[0], "第一章 开始");
    assert_eq!(urls[2], "/read/1001/3.html");

    // 正文：@textNodes 是多值后缀，故应逐个文本节点给出（而非拼接）
    let content_document =
        Document::parse_html(r#"<div class="articleDiv"><p>第一段</p><p>第二段</p></div>"#);
    let paragraphs =
        evaluate_rule(&content_document, source.rule_content.content).expect("content 应求值");
    assert_eq!(
        paragraphs,
        vec!["第一段", "第二段"],
        "@textNodes 应逐个给值（多值后缀）"
    );

    // nextContentUrl 为空是合法配置（表示无下一页）
    assert!(source.rule_content.next_content_url.is_empty());
}

#[test]
fn 简化书源的全部规则都可求值() {
    // tasks.md 第 85 行的前置条件验证：整份简化书源不含任何未就绪的能力。
    // 这条断言的价值在于：若日后有人往样本里加了 @js: 规则，CI 会立刻失败，
    // 而不是让"最小闭环"在 S1-2 之前悄悄失效。
    let source = sample_source();
    let rules = [
        source.rule_search.book_list,
        source.rule_search.name,
        source.rule_search.author,
        source.rule_search.book_url,
        source.rule_search.intro,
        source.rule_toc.chapter_list,
        source.rule_toc.chapter_name,
        source.rule_toc.chapter_url,
        source.rule_content.content,
    ];

    for rule in rules {
        let combined = parse_rule(rule);
        assert!(
            combined.first_not_ready().is_none(),
            "简化书源的规则 `{rule}` 不该含未就绪的能力"
        );
    }
}
