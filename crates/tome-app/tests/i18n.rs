//! T031 的 Phase 3 子集验收：文案表（`crates/tome-app/src/i18n.rs`）
//!
//! ## 本文件守的是什么
//!
//! `contracts/ui.md` §3 的四条文案契约。T031 才落地完整的守卫（grep 断言组件
//! 文件无中文字面串），但**表本身的完整性**现在就该有断言——否则表会先长出
//! 空洞（某个键只有常量没有文案），而空洞要到界面上出现 `shelf.empty.title`
//! 这种字样时才被发现。
//!
//! ## 为什么"错误标识必须随文案一并呈现"要单独断言
//!
//! 那是 §3.2 的明文要求，而它极易被"顺手美化掉"：把
//! `数据读写失败 [error.storage]` 写成只显示前半句，代码更干净、观感更好、
//! **没有任何测试会红**——直到用户报障时说不出那个标识，而开发者无法从
//! 他的描述里定位到是哪一类失败。

use tome_app::i18n;
use tome_core::{ErrorCapability, ErrorKind};

/// 每个键都有非空文案，且**不含**键名本身（键名出现在界面上是可见的失败）。
#[test]
fn every_key_resolves_to_non_empty_text() {
    let keys = i18n::all_keys();
    assert!(
        keys.len() >= 30,
        "Phase 3 的键数应已成规模，实测 {}",
        keys.len()
    );

    for key in keys {
        let text = i18n::t(key);
        assert!(!text.is_empty(), "键 {key} 的文案为空");
        assert_ne!(
            text, key,
            "键 {key} 未在表里定义（取不到时返回键名，那是可见的失败）"
        );
    }
}

/// 键名唯一（重复的键会让 `find` 命中第一个，后面的静默失效）。
#[test]
fn keys_are_unique() {
    let mut keys = i18n::all_keys();
    let total = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), total, "文案表里有重复的键");
}

/// 未定义的键返回**键本身**（可见的失败，而不是空白）。
#[test]
fn an_unknown_key_returns_the_key_itself() {
    let text = i18n::t("no.such.key");
    assert_eq!(
        text, "no.such.key",
        "取不到文案时应返回键名——界面上出现 shelf.empty.title 比出现空白可排查得多"
    );
}

/// **全部 `ErrorKind` 都有文案**（`ErrorKind` 是闭集，漏一个就是一句无法行动的话）。
///
/// ## 交叉校验（为什么有两份穷举清单）
///
/// `i18n::all_error_kinds()` 是手工列的（`ErrorKind` 带载荷，无法派生穷举）。
/// 本测试用**另一个**穷举清单（下面这个 `match`）交叉校验：那个 `match` 是
/// 穷尽的，故"新增一类错误"时**本文件编译失败**——那就逼着人来更新
/// `i18n::all_error_kinds()` 与文案表。
#[test]
fn every_error_kind_has_text() {
    let kinds = i18n::all_error_kinds();
    for kind in &kinds {
        let id = kind.message_id();
        let text = i18n::error_text(id);
        assert!(!text.is_empty(), "错误标识 {id} 没有文案");
        assert_ne!(
            text,
            i18n::t(i18n::key::ERROR_UNKNOWN),
            "错误标识 {id} 落到了兜底文案上——它应有一句能据以行动的话"
        );
    }

    // 交叉校验：下面的 `match` 是穷尽的，新增 `ErrorKind` 变体会让它编译失败
    let mut covered = 0usize;
    for kind in &kinds {
        covered += match kind {
            ErrorKind::Network => 1,
            ErrorKind::Proxy => 1,
            ErrorKind::Rule => 1,
            ErrorKind::UnsupportedCapability { .. } => 1,
            ErrorKind::Content => 1,
            ErrorKind::Storage => 1,
            ErrorKind::Configuration => 1,
        };
    }
    assert_eq!(
        covered,
        kinds.len(),
        "i18n::all_error_kinds() 的清单与本文件的穷举不一致"
    );
    assert!(i18n::error_kinds_covered(), "文案表未覆盖全部 ErrorKind");
}

/// 文案里**不含** `error.` 标识（标识由 [`i18n::error_line`] 单独拼上）。
///
/// ## 为什么这条要断言
///
/// 若文案自己写了标识（如 `"数据读写失败 [error.storage]"`），那么
/// `error_line` 再拼一次就变成 `[error.storage] [error.storage]`——而那种
/// 重复在代码里看不出来（两处各写了一半）。
#[test]
fn the_text_itself_does_not_embed_the_identifier() {
    for kind in i18n::all_error_kinds() {
        let id = kind.message_id();
        let text = i18n::error_text(id);
        assert!(
            !text.contains("error."),
            "文案 {text:?} 里嵌了标识——标识应由 error_line 统一拼上（否则会重复）"
        );
    }
}

/// **标识永不翻译**：`error_line` 的输出必含原标识。
#[test]
fn the_identifier_is_never_translated_away() {
    for kind in i18n::all_error_kinds() {
        let id = kind.message_id();
        let line = i18n::error_line(id);
        assert!(
            line.contains(id),
            "错误行 {line:?} 里丢了标识 {id}——用户将无法向开发者描述问题"
        );
    }

    // 能力错误的标识同样必须出现
    let capability = "webView";
    let badge = i18n::capability_badge(capability);
    assert_eq!(
        badge, "[capability: webView]",
        "能力徽标形态应对齐 002 §2.3"
    );
}

/// 能力错误经 `ClassifiedErrorView` 抵达时，`capability` 字段非空。
#[test]
fn a_capability_error_carries_its_identifier() {
    let kind = ErrorKind::unsupported_capability(ErrorCapability::new("offscreenRendering"));
    let view = tome_app::engine::messages::ClassifiedErrorView::from_kind(&kind);
    assert_eq!(view.capability.as_deref(), Some("offscreenRendering"));
    assert_eq!(view.id, "error.unsupported_capability");
    // 文案表能查到这一条
    assert_ne!(
        i18n::error_text(&view.id),
        i18n::t(i18n::key::ERROR_UNKNOWN)
    );
}

/// 数字/进度的呈现格式统一在文案表（不在组件里拼）。
#[test]
fn numeric_formats_are_defined_once() {
    // 百分比
    assert_eq!(i18n::progress_percent(0.0), "0%");
    assert_eq!(i18n::progress_percent(42.4), "42%");
    assert_eq!(i18n::progress_percent(42.6), "43%");

    // 越界钳制（进度来自存储层，可能被外部工具改过）
    assert_eq!(i18n::progress_percent(-10.0), "0%");
    assert_eq!(i18n::progress_percent(1234.0), "100%");
    assert_eq!(
        i18n::progress_percent(f32::NAN),
        "0%",
        "NaN 不得渲染成 NaN%（那是一个用户无法理解的字符串）"
    );

    // 章号：内部 0 起 → 显示 1 起
    assert_eq!(i18n::chapter_label(0), "第 1 章");
    assert_eq!(i18n::chapter_label(7), "第 8 章");

    // 页码
    assert_eq!(i18n::page_label(3, 12), "3 / 12");

    // 书卡第二行（作者 · 进度）
    assert_eq!(i18n::card_meta("刘慈欣", 42.0), "刘慈欣 · 42%");
}
