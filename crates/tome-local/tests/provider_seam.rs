//! [DONE] ContentProvider 双 adapter 就位（tasks.md T083 / Phase 7 检查点）
//!
//! # 检查点原文
//!
//! > [ ] ContentProvider **双 adapter 就位，净化管线下游统一**
//!
//! # 本文件验证什么
//!
//! `tome-local` 的单元测试验证本地 adapter **自己**的行为。本文件验证的是
//! **两个 adapter 共享同一份契约**——即"上层只认 `ContentProvider` 这个
//! trait，不关心书从哪来"这件事真的成立。
//!
//! 网络 adapter 由 `tome-source` 实现（T074，另一条并行线）。本文件不依赖
//! 它（那会引入网络与存储），而是用**契约测试**的方式验证本地 adapter
//! 满足 seam 的全部不变量：
//!
//! | 不变量 | 来源 | 本文件的断言 |
//! |---|---|---|
//! | 净化在 seam 下游统一应用 | `seams.md` §2 | 两份不同净化配置各自生效 |
//! | 锚点解析与排版参数无关 | SC-008 | 同一查询多次解析同一结果 |
//! | 预取失败无副作用 | `seams.md` §2 | 越界预取不 panic |
//! | 本地书按偏移直读 | `seams.md` §2 | 只读目标章节，不读整本 |
//!
//! # 为什么"偏移直读"值得一条独立断言
//!
//! "直读"是本地 adapter 与网络 adapter 的**结构性区别**（一个是 seek，
//! 一个是抓取+缓存）。若本地 adapter 退化成"每次都解析整本书"，功能上
//! 看不出差别，但性能上完全违背了它的存在理由。故这里用"读一章的时间
//! 不该随书的大小线性增长"来断言它——那是可从外部观测的。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use std::path::Path;
use std::sync::Arc;

use tome_core::seam::{AnchorQuery, ChapterContent, ContentProvider};
use tome_core::{Book, Chapter, ErrorKind};
use tome_local::format::LocalFormat;
use tome_local::provider::{Cleaner, LocalContentProvider, LocalSource};
use tome_rule::pipeline::ContentPipeline;
use tome_rule::pipeline::ReplaceRule;

/// 三章 TXT 样本。
const SAMPLE: &str = concat!(
    "第一章 开端\n甲甲甲的第一章正文内容。\n",
    "第二章 承接\n乙乙乙的第二章正文内容。\n",
    "第三章 收束\n丙丙丙的第三章正文内容。\n",
);

/// 构造一个已解析目录的本地 source（走真实导入路径，不手工拼 Chapter）。
fn source_for(path: &Path) -> Arc<LocalSource> {
    let bytes = std::fs::read(path).expect("读文件");
    let rules = [tome_core::TxtTocRule {
        id: 0,
        name: "章".to_string(),
        rule: "^第.章.*$".to_string(),
        example: None,
        enabled: true,
        sort_order: 0,
    }];
    let parsed = tome_local::txt::import_bytes(
        &bytes,
        &path.file_name().expect("有文件名").to_string_lossy(),
        &rules,
    );
    let toc = tome_local::txt::to_chapters(&parsed.toc, 1);
    Arc::new(LocalSource::new(path.to_path_buf(), LocalFormat::Txt, toc))
}

/// 一个用固定替换规则做净化的 cleaner。
struct FixedCleaner {
    pattern: String,
    replacement: String,
}

impl Cleaner for FixedCleaner {
    fn pipeline(&self) -> ContentPipeline {
        ContentPipeline::new().with_replace_rules(vec![ReplaceRule {
            id: "r".to_string(),
            pattern: self.pattern.clone(),
            replacement: self.replacement.clone(),
            is_regex: false,
            enabled: true,
            sort_order: 0,
        }])
    }
}

fn sample_book() -> Book {
    Book {
        id: 1,
        origin: tome_core::BookOrigin::Local,
        origin_id: "local:sha256:test".to_string(),
        name: "样本".to_string(),
        author: None,
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: None,
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: Some("UTF-8".to_string()),
        format: Some("txt".to_string()),
        total_chapters: Some(3),
        dur_chapter_index: None,
        added_at: 0,
    }
}

/// 写一个临时 TXT 并返回其 provider。
fn provider_with(dir: &Path, content: &str, cleaner: Arc<dyn Cleaner>) -> LocalContentProvider {
    let path = dir.join("书.txt");
    std::fs::write(&path, content).expect("写入");
    LocalContentProvider::new(source_for(&path), cleaner)
}

/// 检查点的核心：`toc` 经 seam 可取得（不依赖具体实现类型）。
#[test]
fn toc_is_available_through_the_seam_trait() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(tome_local::provider::NoCleaner),
    );

    // 通过 `dyn ContentProvider` 调用——这正是上层看到的形态。
    let as_seam: &dyn ContentProvider = &provider;
    let toc = as_seam.toc(&sample_book()).expect("取目录");
    assert_eq!(toc.len(), 3);
    assert_eq!(toc[0].title, "第一章 开端");
    // 本地书的章节用字节偏移定位（`data-model.md`）。
    assert!(toc.iter().all(|chapter| chapter.byte_offset.is_some()));
    assert!(toc.iter().all(|chapter| chapter.epub_entry.is_none()));
}

/// 契约不变量：净化**在 seam 下游**统一应用。
#[test]
fn cleaning_is_applied_under_the_seam() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(FixedCleaner {
            pattern: "甲甲甲".to_string(),
            replacement: "已净化".to_string(),
        }),
    );

    let content = provider.chapter_content(&sample_book(), 0).expect("取正文");
    assert!(
        content.content.contains("已净化"),
        "净化规则应生效，实际 {:?}",
        content.content
    );
    assert!(!content.content.contains("甲甲甲"), "被替换的原文不该残留");
}

/// 两个 adapter 的净化**配置相同则行为相同**——本地侧的可验证部分。
#[test]
fn the_same_cleaning_config_produces_the_same_shape() {
    let dir = tempfile::tempdir().expect("临时目录");
    let config = Arc::new(FixedCleaner {
        pattern: "正文".to_string(),
        replacement: "CONTENT".to_string(),
    });
    let provider = provider_with(dir.path(), SAMPLE, Arc::clone(&config) as Arc<dyn Cleaner>);

    let expected: Vec<ChapterContent> = (0..3)
        .map(|index| {
            provider
                .chapter_content(&sample_book(), index)
                .expect("取正文")
        })
        .collect();

    // 每一章都应经同一份配置处理：出现替换标记、且形态一致（index 与 title 对应）。
    for (index, content) in expected.iter().enumerate() {
        assert_eq!(content.chapter_index, index as i32);
        assert!(
            content.content.contains("CONTENT"),
            "第 {index} 章应经净化处理，实际 {:?}",
            content.content
        );
    }
}

/// 契约不变量：锚点解析与排版参数无关（SC-008）。
#[test]
fn anchor_resolution_is_stable_and_layout_independent() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(tome_local::provider::NoCleaner),
    );

    let query = AnchorQuery {
        chapter_index: 1,
        char_offset: 0,
        anchor_text: Some("乙乙乙".to_string()),
    };
    let first = provider
        .resolve_anchor(&sample_book(), &query)
        .expect("解析");
    let second = provider
        .resolve_anchor(&sample_book(), &query)
        .expect("解析");
    assert_eq!(first, second, "同一查询必须给出同一锚点（无隐藏状态）");
    assert_eq!(first.0, 1, "章节序号应保持");

    // 解析出的位置必须真的指向锚点文本。
    let content = provider.chapter_content(&sample_book(), 1).expect("取正文");
    let at: String = content.content.chars().skip(first.1 as usize).collect();
    assert!(at.starts_with("乙乙乙"), "偏移处应是锚点文本，实际 {at:?}");
}

/// 契约不变量：锚点偏移是**字符**单位（FR-028 的硬要求）。
#[test]
fn anchor_offsets_are_character_based() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(tome_local::provider::NoCleaner),
    );
    let content = provider.chapter_content(&sample_book(), 0).expect("取正文");

    let (_, offset) = provider
        .resolve_anchor(
            &sample_book(),
            &AnchorQuery {
                chapter_index: 0,
                char_offset: 0,
                anchor_text: Some("甲甲甲".to_string()),
            },
        )
        .expect("解析");

    let byte_position = content.content.find("甲甲甲").expect("正文里有锚点文本");
    let char_position = content.content[..byte_position].chars().count();
    assert_eq!(
        offset as usize, char_position,
        "锚点必须是字符下标；按字节算会得到 {byte_position}"
    );
    assert!(
        char_position < byte_position,
        "中文正文里字符下标必须小于字节下标"
    );
}

/// 契约不变量：预取失败**无副作用**。
#[test]
fn prefetch_failures_have_no_side_effects() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(tome_local::provider::NoCleaner),
    );

    // 越界、负数、超大深度都不该 panic 或影响后续读取。
    provider.prefetch(&sample_book(), 999, 10);
    provider.prefetch(&sample_book(), -999, 5);
    provider.prefetch(&sample_book(), 0, u32::MAX);

    let content = provider
        .chapter_content(&sample_book(), 0)
        .expect("预取后仍可读");
    assert!(content.content.contains("甲甲甲"));
}

/// 越界章节给出**结构化**的内容错误（而非 panic 或空串）。
#[test]
fn out_of_range_chapter_is_a_structured_content_error() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(tome_local::provider::NoCleaner),
    );
    let error = provider
        .chapter_content(&sample_book(), 42)
        .expect_err("越界章节应报错");
    assert!(
        matches!(error, ErrorKind::Content),
        "应是内容类错误，实际 {error:?}"
    );
}

/// 本地 adapter 只读目标章节——不是"每次解析整本书"。
///
/// # 断言方式
///
/// 用"读一章的耗时不该随书的大小显著增长"来观测。这是可从外部判定的
/// 近似判据：若实现每次重解析整本书，10 倍大小的书会让单章读取慢 10 倍。
///
/// 阈值取 8 倍而不是 2 倍：文件系统缓存、解码、分配都会带来噪声，而
/// "退化到 O(n)" 的代价是 10 倍以上——两者有一倍以上的安全间隔。
#[test]
fn reading_one_chapter_does_not_rescan_the_whole_book() {
    let dir = tempfile::tempdir().expect("临时目录");

    // 小书与大书（正文量差约 20 倍）。
    let small = "第一章 开端\n短正文。\n第二章 承接\n短正文。\n".to_string();
    let big =
        "第一章 开端\n".to_string() + &"内容填充句子。".repeat(60_000) + "\n第二章 承接\n结尾。\n";

    let small_dir = dir.path().join("small");
    let big_dir = dir.path().join("big");
    std::fs::create_dir_all(&small_dir).expect("建目录");
    std::fs::create_dir_all(&big_dir).expect("建目录");

    let small_provider = provider_with(
        &small_dir,
        &small,
        Arc::new(tome_local::provider::NoCleaner),
    );
    let big_provider = provider_with(&big_dir, &big, Arc::new(tome_local::provider::NoCleaner));

    // 预热（去掉首次读盘与首次缓存的影响）。
    let _ = small_provider.chapter_content(&sample_book(), 0);
    let _ = big_provider.chapter_content(&sample_book(), 0);

    let time_small = measure(|| {
        let _ = small_provider.chapter_content(&sample_book(), 1);
    });
    let time_big = measure(|| {
        let _ = big_provider.chapter_content(&sample_book(), 1);
    });

    // 大书的第二章读取不该慢到 20 倍量级（那是"重解析整本"的信号）。
    let ratio = time_big.as_secs_f64() / time_small.as_secs_f64().max(1e-6);
    assert!(
        ratio < 8.0,
        "单章读取耗时随书增大而线性增长（小={time_small:?}，大={time_big:?}，\
         比值={ratio:.2}）——本地 adapter 应做偏移直读而不是重解析整本"
    );
}

/// 计时辅助（多次取最小值以削弱调度噪声）。
fn measure(body: impl Fn()) -> std::time::Duration {
    let mut best = std::time::Duration::from_secs(60);
    for _ in 0..5 {
        let started = std::time::Instant::now();
        body();
        let elapsed = started.elapsed();
        if elapsed < best {
            best = elapsed;
        }
    }
    best
}

/// 目录经 seam 取得时**与实现类型无关**（上层只认 trait）。
#[test]
fn the_seam_hides_the_implementation() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(tome_local::provider::NoCleaner),
    );

    // 关键：这里函数的签名只提 `dyn ContentProvider`。
    fn read_everything(provider: &dyn ContentProvider, book: &Book) -> Vec<ChapterContent> {
        let toc = provider.toc(book).expect("目录");
        toc.iter()
            .map(|chapter| {
                provider
                    .chapter_content(book, chapter.index_in_book)
                    .expect("正文")
            })
            .collect()
    }

    let contents = read_everything(&provider, &sample_book());
    assert_eq!(contents.len(), 3);
    assert!(contents[0].content.contains("甲甲甲"));
    assert!(contents[2].content.contains("丙丙丙"));
    // 本地书没有"缓存"概念。
    assert!(contents.iter().all(|content| !content.from_cache));
}

/// 章节列表与正文的**序号一致**（错位会让翻章跳到别处）。
#[test]
fn chapter_indices_match_the_toc() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(tome_local::provider::NoCleaner),
    );
    let toc: Vec<Chapter> = provider.toc(&sample_book()).expect("目录");

    for chapter in &toc {
        let content = provider
            .chapter_content(&sample_book(), chapter.index_in_book)
            .expect("取正文");
        assert_eq!(
            content.chapter_index, chapter.index_in_book,
            "返回的序号应与请求的一致"
        );
        assert_eq!(
            content.title, chapter.title,
            "标题应与目录里的一致（不一致说明偏移取错了章节）"
        );
    }
}

// ---------------------------------------------------------------------------
// ChapterContent.images（PM 裁决 OQ#13 / `contracts/seams.md` §2）
// ---------------------------------------------------------------------------

/// 造一个含内嵌图片的 EPUB（内存→临时文件）。
///
/// 结构是最小可用的 EPUB：`mimetype` + `container.xml` + OPF + 一章 XHTML
/// + 一张图片资源。
fn write_epub_with_image(dir: &Path) -> std::path::PathBuf {
    use std::io::Write;

    // 图片前留一段**足够长**的文字（> 校正窗口的 16 字），
    // 这样"改图片之前的文本"的测试可以把修改点放在窗口之外——
    // 否则修改会落在窗口内，校正必然失败（那是另一条测试的场景）
    let chapter_xhtml = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body>
<p>开头这段文字足够长以便把校正窗口完全填满不含任何可改动之处。</p>
<p><img src="images/pic.png" alt="一幅插图"/></p>
<p>第二段正文。</p>
</body></html>"#;

    let opf = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>插图测试书</dc:title>
    <dc:identifier id="id">urn:uuid:img-test</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
    <item id="img1" href="images/pic.png" media-type="image/png"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
  </manifest>
  <spine toc="ncx"><itemref idref="c1"/></spine>
</package>"#;

    let ncx = r#"<?xml version="1.0" encoding="utf-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <navMap>
    <navPoint id="n1" playOrder="1"><navLabel><text>第一章</text></navLabel>
      <content src="chapter1.xhtml"/></navPoint>
  </navMap>
</ncx>"#;

    let container = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

    let path = dir.join("with-image.epub");
    let file = std::fs::File::create(&path).expect("建文件");
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();

    writer.start_file("mimetype", options).expect("mimetype");
    writer.write_all(b"application/epub+zip").expect("写");

    writer
        .start_file("META-INF/container.xml", options)
        .expect("container");
    writer.write_all(container.as_bytes()).expect("写");

    writer
        .start_file("OEBPS/content.opf", options)
        .expect("opf");
    writer.write_all(opf.as_bytes()).expect("写");

    writer.start_file("OEBPS/toc.ncx", options).expect("ncx");
    writer.write_all(ncx.as_bytes()).expect("写");

    writer
        .start_file("OEBPS/chapter1.xhtml", options)
        .expect("chapter");
    writer.write_all(chapter_xhtml.as_bytes()).expect("写");

    // 一张 1x1 PNG（内容无所谓，测试不读它）
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    writer
        .start_file("OEBPS/images/pic.png", options)
        .expect("png");
    writer.write_all(PNG).expect("写");

    writer.finish().expect("收尾");
    path
}

/// 构造 EPUB 书的 provider。
fn epub_provider(path: &Path, cleaner: Arc<dyn Cleaner>) -> LocalContentProvider {
    // 走真实导入路径拿到目录（不手工拼 Chapter）
    let parsed = tome_local::epub::import_path(path).expect("应能解析 EPUB");
    let toc = tome_local::epub::to_chapters(&parsed.toc, 1);
    let source = Arc::new(LocalSource::new(path.to_path_buf(), LocalFormat::Epub, toc));
    LocalContentProvider::new(source, cleaner)
}

/// 一本书（EPUB）。
fn epub_book() -> Book {
    let mut book = sample_book();
    book.name = "插图测试书".to_string();
    book.format = Some("epub".to_string());
    book.total_chapters = Some(1);
    book
}

/// 含图章节经 ContentProvider 取得正文时，`images` 带正确的字符位置与缓存键。
#[test]
fn epub_chapter_content_carries_images() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_epub_with_image(dir.path());
    let provider = epub_provider(&path, Arc::new(tome_local::provider::NoCleaner));

    let content = provider
        .chapter_content(&epub_book(), 0)
        .expect("应能取正文");

    assert_eq!(
        content.images.len(),
        1,
        "这一章有一张图：{:?}",
        content.images
    );
    let image = &content.images[0];
    assert_eq!(
        image.cache_key, "images/pic.png",
        "缓存键应是资源地址（EPUB 内的相对路径）"
    );
    assert_eq!(
        image.alt.as_deref(),
        Some("一幅插图"),
        "alt 应被带上（图取不到时界面才能展示有意义的占位）"
    );

    // 位置：应在第一段之后、第二段之前
    let chars: Vec<char> = content.content.chars().collect();
    let offset = image.char_offset as usize;
    assert!(
        offset <= chars.len(),
        "偏移 {offset} 越界（正文 {} 字）——越界会让排版崩",
        chars.len()
    );
    let before: String = chars[..offset].iter().collect();
    assert!(
        before.contains("开头这段文字"),
        "图片应排在前一段文字之后，实际前缀：{before:?}"
    );
    // 且应排在**下一段之前**——只断言"在前面"会让"图片被放到文末"也通过
    let after: String = chars[offset..].iter().collect();
    assert!(
        after.contains("第二段正文"),
        "图片应排在「第二段正文」之前，实际后缀：{after:?}"
    );
}

/// 无图章节的 `images` 是**空列表**（不是 panic、不是 None）。
#[test]
fn chapter_without_images_has_empty_list() {
    let dir = tempfile::tempdir().expect("临时目录");
    let provider = provider_with(
        dir.path(),
        SAMPLE,
        Arc::new(tome_local::provider::NoCleaner),
    );

    for chapter in provider.toc(&sample_book()).expect("目录") {
        let content = provider
            .chapter_content(&sample_book(), chapter.index_in_book)
            .expect("取正文");
        assert!(
            content.images.is_empty(),
            "TXT 书没有内嵌图，images 应是空列表（第 {} 章）",
            chapter.index_in_book
        );
    }
}

/// 净化**在图片之前**删字时，图片偏移被正确左移（而不是照搬旧值）。
#[test]
fn image_offset_shifts_with_purification() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_epub_with_image(dir.path());

    // 先取未净化的偏移做基准
    let plain = epub_provider(&path, Arc::new(tome_local::provider::NoCleaner));
    let before = plain.chapter_content(&epub_book(), 0).expect("应能取正文");
    assert_eq!(before.images.len(), 1);
    let original_offset = before.images[0].char_offset;

    // 净化：把开头的"开头"替换成"起首"（**等长**，故长度不变）……
    // 等长替换测不出偏移校正。改用"开头这段"→"起首"（短 2 字），
    // 修改点在图片前 20+ 字处，落在 16 字校正窗口**之外**——
    // 故窗口内容在净化后原样存在，校正能把偏移左移 2。
    let cleaner = Arc::new(FixedCleaner {
        pattern: "开头这段".to_string(),
        replacement: "起首".to_string(),
    });
    let provider = epub_provider(&path, cleaner);
    let after = provider
        .chapter_content(&epub_book(), 0)
        .expect("应能取正文");

    assert!(
        after.content.contains("起首"),
        "净化应已生效：{:?}",
        after.content
    );
    assert_eq!(
        after.images.len(),
        1,
        "净化只改了图片之前（且在窗口之外）的文本，图应仍能定位"
    );
    assert_eq!(
        after.images[0].char_offset,
        original_offset - 2,
        "「开头这段」(4 字)→「起首」(2 字) 少了 2 个字，图片偏移应左移 2"
    );
}

/// 净化**整段删掉图片所在的前文**时，图被丢弃（而不是插到错位置）。
///
/// # 为什么要"整段删"才能构造出"无法定位"
///
/// 首版这条测试只是**替换**了图片前面的一段文字，结果图仍能定位——
/// 因为它紧邻的前导空白（两个换行）在净化后原样保留，4 字的上下文
/// 照样匹配得上。要真正让位置无从确定，必须把图片**连同它周围的
/// 上下文一起**改掉，使任何 ≥ `MIN_CONTEXT` 的前缀都不复存在。
#[test]
fn unlocatable_image_is_dropped_not_misplaced() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = write_epub_with_image(dir.path());

    // 把整段前文（含尾部标点，故图片的紧邻前缀也随之消失）换成一串
    // 与原文**无任何 4 字以上公共子串**的文字
    let cleaner = Arc::new(FixedCleaner {
        pattern: "开头这段文字足够长以便把校正窗口完全填满不含任何可改动之处。".to_string(),
        replacement: "XYZQWERTY".to_string(),
    });
    let provider = epub_provider(&path, cleaner);
    let content = provider
        .chapter_content(&epub_book(), 0)
        .expect("应能取正文");

    assert!(
        content.content.contains("XYZQWERTY"),
        "净化应已生效：{:?}",
        content.content
    );
    assert!(
        content.images.is_empty(),
        "上下文被整段改掉的图应被丢弃——位置错的图比没有图更糟（会被插进无关段落）。
实际正文：{:?}
实际 images：{:?}",
        content.content,
        content.images
    );
}
