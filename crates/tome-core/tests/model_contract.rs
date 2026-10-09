//! T009 验收测试：领域 DTO 与 `data-model.md` 18 实体的字段对齐 + 字符区间不变量
//!
//! ## 这些测试在守什么
//!
//! DTO 是引擎的公共语言（接缝 trait 的签名由它们构成），字段一旦漏掉或语义
//! 偏移，会以"上层拿不到某个数据"的形式在很远的地方爆出来。故此处逐条对照
//! `specs/001-pc-engine-core/data-model.md` 的实体定义。
//!
//! ## 为什么单测放在 tests/ 而不是 mod tests
//!
//! 这些断言检查的是 crate 的**公开 API 形状**（字段是否可访问、类型是否正确），
//! 用集成测试才能真实覆盖"外部消费方看到的接口"——内联单测能看到私有项，
//! 会掩盖"字段忘了 pub"这类问题。

use tome_core::model::{
    Book, BookOrigin, Bookmark, Chapter, Cookie, Highlight, ImageCacheRef, LibraryRoot, Memo,
    ReadProgress, ReadRecord, RuleTier, Source, SourceGroup, SourceKind,
};

/// 阅读痕迹一律字符区间，且 `char_offset`/`length` 是**字符**单位而非字节单位。
///
/// FR-028 明确 MUST NOT 用渲染坐标。此处用中文（每字 3 字节）验证单位语义——
/// 若实现方误用字节偏移，高亮会整体偏移，这正是本断言要挡住的错误。
#[test]
fn 阅读痕迹使用字符区间且中文按字符计数() {
    let chapter_text = "零一二三四五六七八九十甲乙丙丁"; // 15 个字符，45 字节

    // 单位语义的证明：字符数与字节数在中文下必然不同
    assert_eq!(chapter_text.chars().count(), 15);
    assert_eq!(chapter_text.len(), 45);
    assert_ne!(
        chapter_text.chars().count(),
        chapter_text.len(),
        "中文下字符数与字节数不同——混淆二者会让高亮整体偏移"
    );

    // 字符区间可**直接**用于按字符切分（不是字节切分）
    let highlight = Highlight {
        id: 1,
        book_id: 7,
        chapter_index: 3,
        char_offset: 10,
        length: 4,
        style: "underline".to_owned(),
        note: Some("重点".to_owned()),
        created_at: 1_700_000_000,
    };

    let selected: String = chapter_text
        .chars()
        .skip(highlight.char_offset as usize)
        .take(highlight.length as usize)
        .collect();
    assert_eq!(selected, "十甲乙丙", "字符区间应能直接按字符切片");
}

/// 阅读进度锚点 = (chapter_index, char_offset) + anchor_text（FR-027 / SC-008）。
///
/// `anchor_text` 是换源特征段匹配的依据（FR-010b），必须存在于 DTO 中。
#[test]
fn 阅读进度携带锚点文本供换源匹配() {
    let progress = ReadProgress {
        book_id: 7,
        chapter_index: 12,
        char_offset: 340,
        anchor_text: Some("他缓缓抬起头".to_owned()),
        updated_at: 1_700_000_000,
    };

    assert_eq!(progress.chapter_index, 12);
    assert_eq!(
        progress.anchor_text.as_deref(),
        Some("他缓缓抬起头"),
        "锚点文本是 FR-010b 特征段匹配的输入，不可缺"
    );
}

/// `Book` 必须同时覆盖网络书与本地书两种来源的字段（data-model.md 的 book 表）。
#[test]
fn 书籍同时覆盖网络与本地来源字段() {
    let network_book = Book {
        id: 1,
        origin: BookOrigin::Network,
        origin_id: "source-a|https://example.com/book/1".to_owned(),
        name: "网络书".to_owned(),
        author: Some("作者".to_owned()),
        cover_url: Some("https://example.com/cover.jpg".to_owned()),
        intro: Some("简介".to_owned()),
        kind: Some("玄幻".to_owned()),
        book_group: Some("在看".to_owned()),
        read_config: None,
        toc_url: Some("https://example.com/book/1/toc".to_owned()),
        latest_chapter_title: Some("第100章".to_owned()),
        latest_chapter_time: Some(1_700_000_000),
        library_root_id: None,
        relative_path: None,
        charset: None,
        format: None,
        total_chapters: Some(100),
        dur_chapter_index: Some(5),
        added_at: 1_699_000_000,
    };

    assert_eq!(network_book.origin, BookOrigin::Network);
    assert!(network_book.toc_url.is_some());
    assert!(
        network_book.relative_path.is_none(),
        "网络书没有本地相对路径"
    );

    let local_book = Book {
        origin: BookOrigin::Local,
        origin_id: "sha256:abc".to_owned(),
        toc_url: None,
        library_root_id: Some(1),
        relative_path: Some("novels/某书.txt".to_owned()),
        charset: Some("GBK".to_owned()),
        format: Some("txt".to_owned()),
        ..network_book
    };

    assert_eq!(local_book.origin, BookOrigin::Local);
    assert!(local_book.relative_path.is_some());
    assert_eq!(local_book.charset.as_deref(), Some("GBK"));
}

/// `Chapter` 必须能表达网络（url）与本地（byte_offset / epub_entry）两种定位。
#[test]
fn 章节支持网络与本地两种定位方式() {
    let network_chapter = Chapter {
        id: 1,
        book_id: 1,
        index_in_book: 0,
        title: "第一章".to_owned(),
        url: Some("https://example.com/c/1".to_owned()),
        byte_offset: None,
        epub_entry: None,
        is_cached: false,
        is_read: false,
        word_count: Some(3000),
    };
    assert!(network_chapter.url.is_some());
    assert!(network_chapter.byte_offset.is_none());

    let local_chapter = Chapter {
        url: None,
        byte_offset: Some(4096),
        epub_entry: None,
        ..network_chapter
    };
    assert_eq!(local_chapter.byte_offset, Some(4096));
}

/// `Source` 必须能承载五类动作规则 + 请求定制 + 脚本（data-model.md 的 source 表）。
#[test]
fn 书源承载五类动作规则与请求定制() {
    let source = Source {
        id: "source-a".to_owned(),
        name: "示例源".to_owned(),
        kind: SourceKind::Book,
        group_id: Some(1),
        enabled: true,
        book_source_type: Some(0),
        book_url_pattern: Some("https://example\\.com/book/\\d+".to_owned()),
        search_url: Some("/search?q={{key}}&page={{page}}".to_owned()),
        explore_url: Some("/explore/{{page}}".to_owned()),
        rule_search: Some("{}".to_owned()),
        rule_explore: Some("{}".to_owned()),
        rule_book_info: Some("{}".to_owned()),
        rule_toc: Some("{}".to_owned()),
        rule_content: Some("{}".to_owned()),
        header: Some(r#"{"User-Agent":"x"}"#.to_owned()),
        login_url: Some("https://example.com/login".to_owned()),
        login_ui: None,
        login_check_js: None,
        js: None,
        charset: Some("UTF-8".to_owned()),
        rate_limit_ms: Some(1000),
        weight: Some(0),
        explore_screen: None,
        comment: Some("测试源".to_owned()),
        last_update_time: Some(1_700_000_000),
        created_at: 1_699_000_000,
    };

    assert_eq!(source.id, "source-a");
    assert!(source.enabled);
    assert_eq!(
        source.rate_limit_ms,
        Some(1000),
        "请求间隔供限速器使用（FR-008）"
    );
    // 搜索地址与发现地址是公开书源格式的**顶层**字段（R4，PM 仲裁 2026-10-10）：
    // 它们与 `rule_search` / `rule_explore` 是并列的顶层键，不是规则 JSON 的子键。
    // 放在 DTO 里是为了让消费方（搜索管线、P6 的发现管线）无需再包一层解析结果。
    assert_eq!(
        source.search_url.as_deref(),
        Some("/search?q={{key}}&page={{page}}"),
        "搜索地址必须是可承载 `{{}}` 占位符的模板串"
    );
    assert_eq!(
        source.explore_url.as_deref(),
        Some("/explore/{{page}}"),
        "发现地址同样是顶层字段（公开格式的 `exploreUrl`）"
    );
    // 五类动作规则齐全（FR-002）
    for (label, rule) in [
        ("搜索", &source.rule_search),
        ("发现", &source.rule_explore),
        ("详情", &source.rule_book_info),
        ("目录", &source.rule_toc),
        ("正文", &source.rule_content),
    ] {
        assert!(rule.is_some(), "{label}规则字段必须存在");
    }
}

/// 书源 `kind` 覆盖三类源 + 分组。
#[test]
fn 书源类型与分组可表达() {
    assert_ne!(SourceKind::Book, SourceKind::Subscribe);
    assert_ne!(SourceKind::Subscribe, SourceKind::Dictionary);

    let group = SourceGroup {
        id: 1,
        name: "默认分组".to_owned(),
        sort_order: 0,
        enabled: true,
    };
    assert!(group.enabled);
}

/// 四类规则实体字段齐备（data-model.md 的 replace_rule / highlight_rule /
/// dict_rule / txt_toc_rule）。`RuleTier` 表达规则所属层级。
#[test]
fn 四类规则实体字段齐备() {
    let replace = tome_core::model::ReplaceRule {
        id: 1,
        name: "去广告".to_owned(),
        pattern: "广告".to_owned(),
        replacement: String::new(),
        scope: tome_core::model::RuleScope::Content,
        is_regex: false,
        enabled: true,
        sort_order: 0,
    };
    assert_eq!(replace.scope, tome_core::model::RuleScope::Content);

    let highlight_rule = tome_core::model::HighlightRule {
        id: 1,
        name: "高亮人名".to_owned(),
        pattern: "\\b[A-Z]\\w+".to_owned(),
        style: "red".to_owned(),
        enabled: true,
        sort_order: 0,
    };
    assert!(highlight_rule.enabled);

    let dict_rule = tome_core::model::DictRule {
        id: 1,
        name: "本地词典".to_owned(),
        url_rule: "https://dict.example.com/{}".to_owned(),
        content_rule: "@css:.content".to_owned(),
        enabled: true,
    };
    assert!(dict_rule.enabled);

    let toc_rule = tome_core::model::TxtTocRule {
        id: 1,
        name: "章节正则".to_owned(),
        rule: "^第.+章".to_owned(),
        example: Some("第一章 起始".to_owned()),
        enabled: true,
        sort_order: 0,
    };
    assert!(toc_rule.enabled);

    // RuleTier 标识规则所属层级，供 FR-014 施加固定顺序
    assert_eq!(RuleTier::Replace, RuleTier::Replace);
}

/// `Cookie` 与 `LibraryRoot` 字段对齐 data-model.md。
#[test]
fn cookie_与库根目录字段对齐() {
    let cookie = Cookie {
        source_id: "source-a".to_owned(),
        name: "session".to_owned(),
        value: "secret".to_owned(),
        domain: Some("example.com".to_owned()),
        expires_at: Some(1_800_000_000),
    };
    assert_eq!(cookie.name, "session");

    let root = LibraryRoot {
        id: 1,
        path: "D:/books".to_owned(),
        added_at: 1_699_000_000,
    };
    assert_eq!(root.path, "D:/books");
}

/// `ReadRecord` 主键是 (book_id, date)，`date` 为 YYYY-MM-DD 字符串（FR-026b）。
#[test]
fn 阅读记录以日期为聚合粒度() {
    let record = ReadRecord {
        book_id: 1,
        date: "2026-10-09".to_owned(),
        read_seconds: 3600,
        read_chars: 12_000,
    };
    assert_eq!(record.date, "2026-10-09");
    assert_eq!(record.read_seconds, 3600);
}

/// `Bookmark` / `Memo` 同样是字符区间（FR-028）。
#[test]
fn 书签与批注使用字符区间() {
    let bookmark = Bookmark {
        id: 1,
        book_id: 1,
        chapter_index: 2,
        char_offset: 50,
        note: Some("记住这里".to_owned()),
        created_at: 1_700_000_000,
    };
    assert_eq!(bookmark.char_offset, 50);

    let memo = Memo {
        id: 1,
        book_id: 1,
        chapter_index: 2,
        char_offset: 50,
        length: 12,
        content: "这里是批注".to_owned(),
        created_at: 1_700_000_000,
    };
    assert_eq!(memo.length, 12);
}

/// `ImageCacheRef` 是图片缓存的索引条目（data-model.md 的 image_cache）。
#[test]
fn 图片缓存条目可表达来源与落盘位置() {
    let image = ImageCacheRef {
        url: "https://example.com/a.jpg".to_owned(),
        source_id: Some("source-a".to_owned()),
        path: "cache/images/ab/abcdef".to_owned(),
        fetched_at: 1_700_000_000,
    };
    assert_eq!(image.url, "https://example.com/a.jpg");
    assert!(image.path.starts_with("cache/images/"));
}

/// `ReadProgress` 通过公共 re-export 可见（避免只有 `model::` 路径可用）。
#[test]
fn 领域类型经_crate_根可见() {
    let progress = ReadProgress {
        book_id: 1,
        chapter_index: 0,
        char_offset: 0,
        anchor_text: None,
        updated_at: 0,
    };
    assert_eq!(progress.book_id, 1);

    // crate 根的 re-export 与 model:: 路径指向同一类型
    let _via_root: tome_core::ReadProgress = progress.clone();
    let _via_module: tome_core::model::ReadProgress = progress;
}

// ---------------------------------------------------------------------------
// 补齐 data-model.md 18 实体中其余三个（不直接参与书源语义，但同属清单）
// ---------------------------------------------------------------------------

/// `chapter_cache` 是**索引**：只记录内容在 Pack 容器中的位置（FR-029）。
///
/// 崩溃安全（FR-030）的顺序约束在此处体现为字段的可表达性：
/// `cached_at` 与位置信息齐备，实现方才能保证"内容落盘先于索引提交"。
#[test]
fn 章节缓存条目记录容器内位置与淘汰时间() {
    use tome_core::model::ChapterCacheEntry;

    let entry = ChapterCacheEntry {
        chapter_id: 42,
        book_id: 7,
        pack_id: 3,
        offset: 65536,
        length: 4096,
        cached_at: 1_700_000_000,
    };

    assert_eq!(entry.chapter_id, 42);
    assert_eq!(entry.offset + entry.length, 69632, "位置与长度应能定位区间");
    assert!(entry.cached_at > 0, "淘汰按 cached_at 做 LRU（FR-031）");
}

/// `http_log` 的字段名带 `_redacted` 后缀——脱敏是写入前的强制步骤（FR-017）。
#[test]
fn 请求日志字段明示已脱敏() {
    use tome_core::model::HttpLogEntry;

    let entry = HttpLogEntry {
        id: 1,
        source_id: Some("source-a".to_owned()),
        url: "https://example.com/x".to_owned(),
        method: "GET".to_owned(),
        request_headers_redacted: Some(r#"{"User-Agent":"tome"}"#.to_owned()),
        response_code: Some(200),
        response_body_redacted: None,
        duration_ms: Some(123),
        created_at: 1_700_000_000,
    };

    assert_eq!(entry.response_code, Some(200));
    // 字段名本身即约束：实现方看见 _redacted 后缀就不该塞原始头进来
    assert!(entry.request_headers_redacted.is_some());
}

/// `search_keyword` 支持收藏与复用（FR-018）。
#[test]
fn 搜索关键词支持收藏与复用计数() {
    use tome_core::model::SearchKeyword;

    let keyword = SearchKeyword {
        id: 1,
        keyword: "剑来".to_owned(),
        created_at: 1_700_000_000,
        use_count: 3,
    };

    assert_eq!(keyword.keyword, "剑来");
    assert_eq!(keyword.use_count, 3, "复用次数供排序");
}
