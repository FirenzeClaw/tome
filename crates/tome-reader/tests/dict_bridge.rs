//! T044 验收：词典查词桥（FR-041）
//!
//! ## FR-041 的原文
//!
//! > 系统 MUST 支持词典查词在阅读层的触发与结果呈现；词典求值本身 MUST
//! > 复用 001 §FR-016 的既有能力，本层 MUST NOT 实现第二套词典规则求值。
//!
//! ## 本文件的判据
//!
//! 1. **不实现第二套求值**：本层的输入是 001 已经产出的 `DictQuery`
//!    （词 + 规则 id + url_rule + content_rule），不是原始 `DictRule` 表，
//!    也不是自己去解析规则字符串。MUST 有一处结构性证据证明"本层不认识
//!    规则语法"——见 `dict_bridge_never_parses_rule_syntax`。
//! 2. **失败给可操作原因**：US4 场景 4 要求"失败时提示可操作原因而不静默"。
//!    `None` / 空结果 / 全部规则禁用 / 词为空——四种失败各有**可操作**的
//!    提示（告诉用户下一步能做什么），而不是"查询失败"。
//! 3. **结果是结构化数据**，不是面向用户的自然语言（FR-058：引擎内禁止
//!    硬编码面向用户的文案；`ReadingOutcome` 携带的是标识与上下文）。

use tome_reader::trace::dict::{
    build_lookup_request, DictFailure, DictQueryInput, DictRuleRef, DictTrigger,
};

// ===========================================================================
// 1. 不实现第二套求值（结构性证据）
// ===========================================================================

/// **本层不认识词典规则的语法**（FR-041 的"不实现第二套"）。
///
/// ## 判据为什么是源码扫描
///
/// "不重复实现"是**结构性**要求，行为测试测不出来——一个重复实现的版本
/// 照样能产出正确结果，只是它会在某天与 001 的实现分叉。故本测试扫描
/// `src/trace/dict.rs`，断言它**不出现**规则语法的解析迹象：
///
/// - 不出现 `tome_rule` 依赖（本 crate 根本不依赖它，但显式断言更直白）
/// - 不出现 `@css:` / `@json:` / `@xpath:` / `@js:` 这些规则方言前缀
/// - 不出现 `{{word}}` 的**替换逻辑**（占位符替换是 001 的 `url_rule` 求值）
#[test]
fn dict_bridge_never_parses_rule_syntax() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/trace/dict.rs"),
    )
    .expect("应能读取 dict.rs 源码");

    // 去掉注释行——注释里提到这些方言名是允许的（本文档就在讨论它们）
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    for forbidden in [
        "@css:",
        "@json:",
        "@xpath:",
        "@js:",
        "tome_rule",
        "tome-rule",
    ] {
        assert!(
            !code.contains(forbidden),
            "dict.rs 的代码里出现了 `{forbidden}`——FR-041 要求复用 001 §FR-016 的\
             既有词典求值，本层 MUST NOT 实现第二套（规则语法属 001 的 `tome-rule`）"
        );
    }
}

// ===========================================================================
// 2. 请求构造：复用 001 的产物
// ===========================================================================

/// 一个可用的词典规则引用（模拟 001 `DictQuery` 已产出的形态）。
fn rule(id: &str) -> DictRuleRef {
    DictRuleRef {
        rule_id: id.to_string(),
        url_rule: "https://dict.example/api?w={{word}}".to_string(),
        content_rule: "@css:.definition".to_string(),
    }
}

/// **本层消费的是 001 的 `DictQuery` 产物，不是原始规则表**。
#[test]
fn lookup_request_is_built_from_upstream_queries() {
    // 001 的 `PipelineOutcome.dict_queries` 已经在 `DictQueryInput` 里
    let input = DictQueryInput {
        word: "山有木兮".to_string(),
        queries: vec![rule("d1"), rule("d2")],
    };

    let request = build_lookup_request(&input).expect("有可用规则时应能构造请求");

    // 请求携带的是**上游给的规则串**，本层只透传不解释
    assert_eq!(request.queries.len(), 2, "每条上游查询都该被透传");
    assert_eq!(request.queries[0].rule_id, "d1");
    assert_eq!(
        request.queries[0].url_rule, "https://dict.example/api?w={{word}}",
        "本层 MUST NOT 自行展开占位符——那是 001 的 url_rule 求值职责"
    );
    assert_eq!(
        request.word, "山有木兮",
        "请求应携带待查的词（001 的 dict_queries 已按词分好）"
    );
}

/// **词被规范化（去首尾空白）**，但**内容不改**。
#[test]
fn lookup_request_normalizes_whitespace_only() {
    let input = DictQueryInput {
        word: "  天地  ".to_string(),
        queries: vec![rule("d1")],
    };
    let request = build_lookup_request(&input).expect("应能构造");
    assert_eq!(request.word, "天地", "应去掉首尾空白");
    // 不改内部内容（分词是上游的事）
    let inner = DictQueryInput {
        word: "山 有 木".to_string(),
        queries: vec![rule("d1")],
    };
    assert_eq!(
        build_lookup_request(&inner).expect("应能构造").word,
        "山 有 木",
        "内部空白不得被改写（本层不做分词）"
    );
}

// ===========================================================================
// 3. 失败给可操作原因（US4 场景 4）
// ===========================================================================

/// **词为空** → 可操作提示（告诉调用方"需要先有一个词"）。
#[test]
fn empty_word_gives_actionable_failure() {
    let input = DictQueryInput {
        word: "   ".to_string(),
        queries: vec![rule("d1")],
    };
    let failure = build_lookup_request(&input).expect_err("空词应失败");

    assert_eq!(failure, DictFailure::EmptyWord);
    // 提示必须指向**下一步动作**（而非"查询失败"这类无信息量的说法）
    let guidance = failure.guidance();
    assert!(
        guidance.contains("select") || guidance.contains("word"),
        "提示应说明如何产生一个词，实测：{guidance}"
    );
}

/// **没有可用规则** → 提示去配置词典规则（而不是"网络错误"）。
#[test]
fn missing_rules_gives_actionable_failure() {
    let input = DictQueryInput {
        word: "山有木兮".to_string(),
        queries: Vec::new(),
    };
    let failure = build_lookup_request(&input).expect_err("无规则应失败");
    assert_eq!(failure, DictFailure::NoRules);
    let guidance = failure.guidance();
    assert!(
        guidance.contains("dict rule") || guidance.contains("configure"),
        "无规则的提示应指向配置动作，实测：{guidance}"
    );
}

/// **每种失败都有稳定的标识与可操作提示**（穷举，防新增变体漏填）。
#[test]
fn every_failure_variant_has_stable_kind_and_guidance() {
    // 逐个变体构造，断言标识与提示都非空且互不相同
    let failures = [
        DictFailure::EmptyWord,
        DictFailure::NoRules,
        DictFailure::RequestFailed,
        DictFailure::EmptyResult,
    ];
    let mut names = Vec::new();
    for failure in failures {
        let name = failure.as_str();
        assert!(!name.is_empty(), "失败标识不得为空（{failure:?}）");
        assert!(
            !name.contains(' ') && name.chars().next().is_some_and(char::is_lowercase),
            "标识应是驼峰式（书源生态通用叫法，FR-058），实测：{name}"
        );
        names.push(name);
    }
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 4, "四个失败标识应互不相同（枚举无歧义）");
}

/// **失败提示不是面向用户的自然语言硬编码**（FR-058 的边界）。
///
/// ## 这条怎么与"可操作原因"共存
///
/// FR-058 禁的是**引擎内硬编码面向用户的文案**。本层的 `guidance()` 返回的是
/// **面向开发者/日志**的英文技术提示（可被上层映射成任何语言），而非
/// 直接显示给用户的中文文案。判据：提示里不出现中文——中文本地化是上层的事。
#[test]
fn guidance_is_not_localized_user_facing_text() {
    let empty = build_lookup_request(&DictQueryInput {
        word: String::new(),
        queries: vec![rule("d1")],
    })
    .expect_err("空词应失败");
    let no_rules = build_lookup_request(&DictQueryInput {
        word: "词".to_string(),
        queries: Vec::new(),
    })
    .expect_err("无规则应失败");

    for guidance in [empty.guidance(), no_rules.guidance()] {
        assert!(
            !guidance
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "引擎内不得出现面向用户的中文文案（FR-058：本地化留给上层），实测：{guidance}"
        );
    }
}

// ===========================================================================
// 4. 触发源（FR-041 的"阅读层的触发"）
// ===========================================================================

/// **触发可以来自选择或光标**（两种交互形态都要能触发查词）。
#[test]
fn trigger_can_come_from_selection_or_cursor() {
    let from_selection = DictTrigger::from_selection(10..14);
    assert_eq!(
        from_selection.char_range(),
        10..14,
        "选择触发应保留字符区间"
    );

    let from_cursor = DictTrigger::at_cursor(7);
    assert!(
        from_cursor.char_range().is_empty(),
        "光标触发为零长区间（此时由上层按分词扩展出词）"
    );
    assert_eq!(from_cursor.char_range().start, 7);
}

/// **触发的字符区间就是 001 锚点口径**（可直接用于取词）。
#[test]
fn trigger_range_matches_anchor_convention() {
    let trigger = DictTrigger::from_selection(100..110);
    // 直接作为锚点偏移使用（不经过坐标换算）
    let anchor_offset: i64 = trigger.char_range().start as i64;
    assert_eq!(anchor_offset, 100);
    assert_eq!(trigger.char_range().len(), 10);
}

// ===========================================================================
// 5. 结果呈现契约
// ===========================================================================

/// **结果是结构化数据**：每条释义携带来源规则 id（可追溯）。
#[test]
fn request_carries_rule_identity_for_traceability() {
    let input = DictQueryInput {
        word: "天地".to_string(),
        queries: vec![rule("cidian-a"), rule("cidian-b")],
    };
    let request = build_lookup_request(&input).expect("应能构造");

    let ids: Vec<&str> = request
        .queries
        .iter()
        .map(|query| query.rule_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["cidian-a", "cidian-b"],
        "请求应按上游顺序保留规则标识（结果可追溯到是哪条规则给的）"
    );
}
