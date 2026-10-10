//! Android 备份书源导入的**端到端**验收（tasks.md T088 / FR-034）
//!
//! # spec 的验收场景 3
//!
//! > **Given** 一份来自既有 Android 版本的备份文件中的书源，
//! > **When** 用户导入，**Then** 书源被正确识别并可用
//!
//! 三段各自对应本文的一条断言：
//!
//! | 场景 | 断言 |
//! |---|---|
//! | Given 备份文件 | 从 zip 字节里提取（`extract_sources`） |
//! | When 用户导入 | 提取出的 JSON 交给**真实的**书源解析器（`parse_sources`） |
//! | Then 识别并可用 | 解析出的 `Source` 字段正确、能经管线求值 |
//!
//! # 为什么这条测试在 tome-source 而不是 tome-store
//!
//! 它要同时用两个 crate，而依赖方向是 `tome-source → tome-store`。
//! 放在 tome-store 侧就得反向依赖，违反架构铁律（crate 边界即架构）。
//! `tome-source` 本来就依赖 `tome-store`，故这里是唯一能同时触达两者的位置。
//!
//! # 干净室说明
//!
//! 样本 zip 是**按公开描述的形态自行构造**的（内含按类型分的 JSON 文件），
//! 不是从上游仓库取来的真实备份。公开资料对该形态的描述见
//! `crates/tome-store/src/backup_android.rs` 的模块文档。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::io::{Cursor, Write};

/// 造一个"Android 备份"zip（内存）。
///
/// 形态依据公开描述：一批按类型分文件的 JSON。
fn make_backup_zip(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(*name, options).expect("应能建条目");
            writer.write_all(content.as_bytes()).expect("应能写条目");
        }
        writer.finish().expect("应能收尾");
    }
    buffer
}

/// 一份典型的 Android 备份：多类型 JSON + 一些非 JSON 资源。
fn typical_backup() -> Vec<u8> {
    make_backup_zip(&[
        (
            "bookSource.json",
            r#"[
                {
                    "bookSourceName": "安卓备份源甲",
                    "bookSourceUrl": "https://android-a.example.com",
                    "bookSourceGroup": "备份组",
                    "enabled": true,
                    "searchUrl": "/search?q={{key}}",
                    "ruleSearch": "{\"bookList\":\"@css:li.book\",\"name\":\"@css:.title@text\",\"author\":\"@css:.author@text\"}",
                    "ruleToc": "{\"chapterList\":\"@css:#list a\",\"chapterName\":\"@css:#list a@text\"}",
                    "ruleContent": "{\"content\":\"@css:#content@text\"}",
                    "concurrentRate": "1500",
                    "header": "{\"User-Agent\":\"legado/3.0\"}"
                },
                {
                    "bookSourceName": "安卓备份源乙",
                    "bookSourceUrl": "https://android-b.example.com",
                    "enabled": false
                }
            ]"#,
        ),
        // 备份包里必然存在的其他类型，本任务**不导入**它们（范围是书源）
        ("book.json", r#"[{"name":"某本书","author":"某人"}]"#),
        ("bookGroup.json", r#"[{"groupName":"默认"}]"#),
        ("replaceRule.json", r#"[{"name":"净化","pattern":"广告"}]"#),
        (
            "readRecord.json",
            r#"[{"bookName":"某本书","readTime":3600}]"#,
        ),
        // 非 JSON 资源（字体等），应被跳过
        ("font/custom.ttf", "not a font really"),
    ])
}

#[test]
fn 安卓备份里的书源可被识别并可用() {
    let zip = typical_backup();

    // --- Given: 从备份里提取 ---
    let extracted = tome_store::backup_android::extract_sources(&zip).expect("应能提取");
    assert_eq!(
        extracted.source_count, 2,
        "备份里有两个书源，跳过项见 {:?}",
        extracted.skipped
    );

    // 不该把书包/规则/记录当书源
    for (name, reason) in &extracted.skipped {
        if name.ends_with(".json") {
            assert!(
                reason.contains("不含书源"),
                "{name} 的跳过原因应是「不含书源」，实际：{reason}"
            );
        }
    }

    // --- When: 交给真实的书源解析器（用户"导入"这一步）---
    let outcome = tome_source::source::parse_sources(&extracted.sources_json)
        .expect("提取结果应是可被解析的书源文本");
    assert_eq!(outcome.sources.len(), 2, "两个源都应解析成功");
    assert_eq!(outcome.report.skipped, 0, "不该有源被跳过");

    // --- Then: 书源被正确识别 ---
    let first = outcome
        .sources
        .iter()
        .find(|parsed| parsed.source.id == "https://android-a.example.com")
        .expect("应认出源甲");
    assert_eq!(first.source.name, "安卓备份源甲");
    assert!(first.source.enabled, "enabled: true 应被读到");
    assert_eq!(
        first.source.search_url.as_deref(),
        Some("/search?q={{key}}"),
        "searchUrl 应被读到（R4：它是顶层键，下沉为 Source 字段）"
    );
    assert!(first.source.rule_search.is_some(), "搜索规则应被读到");
    assert!(first.source.rule_content.is_some(), "正文规则应被读到");
    // `concurrentRate` 是字符串毫秒值，应被解析为限速
    assert_eq!(
        first.source.rate_limit_ms,
        Some(1500),
        "concurrentRate 的字符串毫秒值应被解析成限速（FR-008）"
    );
    assert!(first.source.header.is_some(), "请求头应被读到（FR-008）");

    // 第二个源：enabled: false 也要如实读到（启停状态属于书源数据）
    let second = outcome
        .sources
        .iter()
        .find(|parsed| parsed.source.id == "https://android-b.example.com")
        .expect("应认出源乙");
    assert!(!second.source.enabled, "enabled: false 应被读到");
}

#[test]
fn 提取的书源能被规则引擎求值() {
    // "可用"的更强证据：不只是解析成 DTO，而是**规则真的能跑通**。
    // 这条把 FR-034 的"可用"落到引擎行为上，而不是停在字段比对。
    let zip = make_backup_zip(&[(
        "bookSource.json",
        r#"[{
            "bookSourceName": "可求值源",
            "bookSourceUrl": "https://eval.example.com",
            "ruleSearch": "{\"bookList\":\"@css:li.book\",\"name\":\"@css:.title@text\",\"bookUrl\":\"@css:.title>a@href\"}"
        }]"#,
    )]);

    let extracted = tome_store::backup_android::extract_sources(&zip).expect("应能提取");
    let outcome = tome_source::source::parse_sources(&extracted.sources_json).expect("应能解析");
    let parsed = &outcome.sources[0];

    // 用真实规则引擎跑一遍搜索结果的解析。
    // 注意结果里必须含 bookUrl —— 没有详情页地址的结果会被有意跳过
    // （见 parse_search_result：点不动的结果比少一条更糟）
    let html =
        r#"<ul><li class="book"><span class="title"><a href="/b/1">备份书</a></span></li></ul>"#;
    let hits =
        tome_source::search::parse_search_result(&parsed.source, html).expect("规则应能求值");
    assert_eq!(hits.len(), 1, "应解析出一条搜索结果");
    assert_eq!(hits[0].name, "备份书");
}

#[test]
fn 不含书源的备份给出空结果与完整扫描记录() {
    // 用户拿错文件是常见情形。此时应能告诉用户"包里到底有什么"，
    // 而不是只说"导入失败"
    let zip = make_backup_zip(&[
        ("book.json", r#"[{"name":"书"}]"#),
        ("themeConfig.json", r##"{"bg":"#fff"}"##),
    ]);

    let extracted = tome_store::backup_android::extract_sources(&zip).expect("应能处理");
    assert!(extracted.is_empty(), "包里没有书源");
    assert_eq!(extracted.source_count, 0);
    assert_eq!(
        extracted.scanned_entries.len(),
        2,
        "扫描记录应列出包内全部条目：{:?}",
        extracted.scanned_entries
    );
    assert_eq!(extracted.skipped.len(), 2, "两条都应说明被跳过");
}

#[test]
fn 旧版无压缩备份同样可导入() {
    // 公开说明提到"旧版备份没压缩"。未压缩的 zip（stored）应同样能读——
    // `zip` crate 按条目声明的压缩方法处理，与我们的代码无关，但值得钉住：
    // 若将来有人为了省事去掉 zip 依赖改用自写的解析器，这条会红。
    let mut buffer = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer
            .start_file("bookSource.json", options)
            .expect("应能建条目");
        let stored_json =
            r#"[{"bookSourceName":"未压缩源","bookSourceUrl":"https://stored.example.com"}]"#;
        writer.write_all(stored_json.as_bytes()).expect("应能写");
        writer.finish().expect("应能收尾");
    }

    let extracted = tome_store::backup_android::extract_sources(&buffer).expect("应能提取");
    assert_eq!(extracted.source_count, 1);
    let outcome = tome_source::source::parse_sources(&extracted.sources_json).expect("应能解析");
    assert_eq!(outcome.sources[0].source.id, "https://stored.example.com");
}
