//! [DONE] MOBI 真实文件验证（tasks.md T078-T081 / Phase 7 检查点）
//!
//! # 为什么这些断言必须跑在真实文件上
//!
//! Phase 7 的检查点原文：
//!
//! > [ ] MOBI 用**真实文件**验证（非仅单元测试）
//!
//! research R4 也点明了理由：
//!
//! > **HUFF/CDIC 与 KF8 路径必须用真实文件验证，不能只靠单元测试**
//!
//! 单元测试用的是"我按规范构造的字节"——它验证的是**实现与我的理解一致**，
//! 而真实文件验证的是**我的理解与规范一致**。两者缺一不可，且第二件事
//! 单元测试原理上做不到（构造数据的那个"理解"已经写进构造代码里了）。
//!
//! ## 本次验证抓到的三个真实缺陷（单元测试全部漏过）
//!
//! | 缺陷 | 单元测试为何漏过 |
//! |---|---|---|
//! | `firstContent` 不是正文起点（本文件里指向 INDX） | 我按"它应该是起点"构造数据 |
//! | 单头 + FDST 构造下没有第二个 `MOBI` 头 | 我按"双格式必有两个头"构造数据 |
//! | INDX 记录带前导填充（`\x00\x00INDX`） | 我按"魔数在偏移 0"构造数据 |
//!
//! 三条都只能靠真实文件暴露——这正是 Phase 7 把它列为检查点的价值。
//!
//! # 语料来自哪里
//!
//! Project Gutenberg 的公有领域电子书（`gutenberg.org/ebooks/<id>.kf8.images`）。
//! 这些文件是 Kindle 格式产物，由 Gutenberg 官方重新打包，**含 KF8 部分**。
//!
//! ## 语料不入仓库
//!
//! research R4 与 `.gitignore` 都要求测试大文件不入仓库（第三方资产）。
//! 故本测试在**找不到文件时跳过**（`return`），而不是失败——这样：
//!
//! - CI（无网络、无大文件）不会因缺语料而红
//! - 本地有语料时（`.tome-mobi/`，见测试里的获取说明）**自动**跑真实验证
//!
//! 获取方式（一次性，约 60MB）：
//!
//! ```text
//! mkdir -p .tome-mobi
//! for id in 368 2701 1342 580; do
//!   curl -sL -o ".tome-mobi/$id.kf8.mobi" \
//!     -x http://127.0.0.1:7890 --ssl-no-revoke \
//!     "https://www.gutenberg.org/ebooks/$id.kf8.images"
//! done
//! ```

use std::path::{Path, PathBuf};

use tome_local::mobi::{self, MobiLayout};

/// 语料目录（已被 `.gitignore` 覆盖；由开发者本地准备）。
///
/// 放在 `/testdata/captures/local-books/`——那里本就是 `.gitignore` 为
/// "第三方抓取产物"划的目录，本地书语料同属这一类。
fn corpus_dir() -> PathBuf {
    // 从 crate 目录往仓库根走；`CARGO_MANIFEST_DIR` 是 `crates/tome-local`。
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("testdata/captures/local-books"))
        .unwrap_or_else(|| PathBuf::from("testdata/captures/local-books"))
}

/// 取一个真实语料；不存在时返回 `None`（测试随即跳过）。
fn corpus(name: &str) -> Option<Vec<u8>> {
    let path = corpus_dir().join(name);
    std::fs::read(&path).ok()
}

/// 所有语料都不可用时报出**明确**的跳过原因（而不是静默通过）。
fn report_skip(name: &str) {
    eprintln!(
        "跳过：真实语料 `{name}` 不在 {}（获取方式见本文件的模块文档）",
        corpus_dir().display()
    );
}

/// T078：PalmDOC 真实文件——元数据、目录、正文全部可用。
#[test]
fn real_palmdoc_mobi_yields_metadata_and_text() {
    let Some(bytes) = corpus("368.kf8.mobi") else {
        report_skip("368.kf8.mobi");
        return;
    };

    let parsed = mobi::parse(&bytes).expect("真实 MOBI 应能解析");

    // 元数据：EXTH 的书名与作者必须解出来（不是兜底的数据库名）。
    assert_eq!(
        parsed.book.name, "Acres of Diamonds: Our Every-day Opportunities",
        "EXTH 的 tag 503 应给出书名"
    );
    assert_eq!(
        parsed.book.author.as_deref(),
        Some("Russell H. Conwell"),
        "EXTH 的 tag 100 应给出作者"
    );
    assert_eq!(
        parsed.book.kind.as_deref(),
        Some("en"),
        "EXTH 的 tag 524 应给出语言"
    );

    // 正文：必须包含书里真实存在的句子。
    assert!(
        parsed.text.contains("ACRES OF DIAMONDS"),
        "正文应含真实标题"
    );
    assert!(
        parsed.text.contains("Russell H. Conwell"),
        "正文应含真实作者名"
    );
    // 正文不该含二进制垃圾（JPEG 头、INDX 魔数）——那是"取错记录区"的信号。
    assert!(
        !parsed.text.contains("JFIF"),
        "正文不该包含 JPEG 头（说明取错了记录区）"
    );
    assert!(!parsed.text.contains("INDX"), "正文不该包含 INDX 魔数");

    // 目录：guide 型 / 标题推断应产出多章。
    assert!(
        parsed.toc.len() >= 5,
        "应识别出多个章节，实际 {}",
        parsed.toc.len()
    );
    // 章节偏移必须能在正文上直接定位。
    for chapter in &parsed.toc {
        assert!(
            chapter.byte_offset <= parsed.text.len(),
            "章节偏移 {} 超出正文长度 {}",
            chapter.byte_offset,
            parsed.text.len()
        );
    }
}

/// T078 的核心：**trailing data 剥离正确**（R4 的高危点）。
///
/// 断言方式是"正文末尾必须是可读文本、不含末尾填充"——剥离错了不会报错，
/// 只会让末尾多出（或少掉）内容，故必须用内容特征来判。
#[test]
fn real_palmdoc_trailing_data_is_stripped() {
    let Some(bytes) = corpus("368.kf8.mobi") else {
        report_skip("368.kf8.mobi");
        return;
    };
    let parsed = mobi::parse(&bytes).expect("应解析");

    // 1. 末尾不该有 NUL（trailing data 的典型形态）。
    assert_ne!(
        parsed.text.as_bytes().last(),
        Some(&0x00),
        "正文末尾不该残留 NUL 填充"
    );
    // 2. 末尾不该有替换字符。
    assert!(!parsed.text.ends_with('\u{FFFD}'), "正文末尾不该有替换字符");
    // 3. 正文长度应落在合理区间（不是"只解压了第一条记录"）。
    assert!(
        parsed.text.len() > 200_000,
        "正文应有 20 万字符以上，实际 {}",
        parsed.text.len()
    );
    // 4. 书名出现的位置在正文里（说明解压不是从中间开始的）。
    let position = parsed.text.find("ACRES OF DIAMONDS").unwrap_or(usize::MAX);
    assert!(position < 1000, "标题应出现在正文开头附近，实际 {position}");
}

/// T081：KF8 形态被正确识别（单头 + FDST 构造）。
#[test]
fn real_kf8_layout_is_detected() {
    let Some(bytes) = corpus("368.kf8.mobi") else {
        report_skip("368.kf8.mobi");
        return;
    };
    let parsed = mobi::parse(&bytes).expect("应解析");
    // 该文件 `file_version = 8`，故它必须是双格式容器。
    assert_eq!(
        parsed.layout,
        MobiLayout::Mobi6InDual,
        "版本 8 的文件应识别为双格式，实际 {:?}",
        parsed.layout
    );
    // 且**不该**报"找不到 KF8 部分"（那是首版的真实缺陷）。
    assert!(
        !parsed
            .warnings
            .iter()
            .any(|warning| warning.contains("找不到 KF8")),
        "不该误报找不到 KF8 部分，实际警告 {:?}",
        parsed.warnings
    );
}

/// 多本真实书一起验证（覆盖不同长度与不同目录密度）。
#[test]
fn several_real_books_all_parse() {
    let names = ["368.kf8.mobi", "2701.kf8.mobi", "1342.kf8.mobi"];
    let mut parsed_any = false;

    for name in names {
        let Some(bytes) = corpus(name) else {
            report_skip(name);
            continue;
        };
        let parsed =
            mobi::parse(&bytes).unwrap_or_else(|error| panic!("{name} 应能解析，实际 {error}"));
        parsed_any = true;

        assert!(
            parsed.text.len() > 10_000,
            "{name} 的正文应有一定长度，实际 {}",
            parsed.text.len()
        );
        assert!(
            !parsed.text.contains("JFIF"),
            "{name} 的正文不该含二进制垃圾"
        );
        assert!(!parsed.toc.is_empty(), "{name} 应至少产出一个章节");
        // 每一章的偏移都要是合法的字符边界（切片不会 panic）。
        for chapter in &parsed.toc {
            if chapter.byte_offset < parsed.text.len() {
                assert!(
                    parsed.text.is_char_boundary(chapter.byte_offset),
                    "{name} 的章节偏移 {} 不是字符边界",
                    chapter.byte_offset
                );
            }
        }
        eprintln!(
            "{name}: 书名={:?} 作者={:?} 章节={} 正文字符={} 形态={:?}",
            parsed.book.name,
            parsed.book.author,
            parsed.toc.len(),
            parsed.text.chars().count(),
            parsed.layout
        );
    }

    if !parsed_any {
        eprintln!("没有可用的真实 MOBI 语料——本测试跳过（不算通过真实验证）");
    }
}

/// FR-022 的"可被随机定位"：按偏移取出的文本必须与章节标题对得上。
#[test]
fn real_mobi_offsets_support_random_access() {
    let Some(bytes) = corpus("368.kf8.mobi") else {
        report_skip("368.kf8.mobi");
        return;
    };
    let parsed = mobi::parse(&bytes).expect("应解析");

    // 抽查首、中、末三章。
    let indexes = [0usize, parsed.toc.len() / 2, parsed.toc.len() - 1];
    for index in indexes {
        let chapter = &parsed.toc[index];
        let tail = &parsed.text[chapter.byte_offset..];
        assert!(
            tail.starts_with(&chapter.title),
            "第 {index} 章的偏移处应能直接读到它的标题；\
             期望 {:?}，实际 {:?}",
            chapter.title,
            &tail[..tail.len().min(60)]
        );
    }
}

/// DRM 检测在真实文件上不误报（T080）。
#[test]
fn real_files_are_not_falsely_flagged_as_drm() {
    let names = [
        "368.kf8.mobi",
        "2701.kf8.mobi",
        "1342.kf8.mobi",
        "580.kf8.mobi",
    ];
    for name in names {
        let Some(bytes) = corpus(name) else {
            continue;
        };
        // 这些是 Gutenberg 的公版书，**没有** DRM。若解析报 DrmProtected，
        // 说明判据过宽（R4 点名的 17480 误判就是这一类的极端情形）。
        match mobi::parse(&bytes) {
            Ok(_) => {}
            Err(tome_local::LocalError::DrmProtected { encryption_type }) => {
                panic!("{name} 是公版书，不该被判为 DRM（encryption_type={encryption_type}）");
            }
            Err(error) => {
                // 其它错误（如压缩不支持）是可接受的，但要说清是哪一个。
                eprintln!("{name}: 解析未完成 —— {error}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// T077：EPUB 真实文件验证
// ---------------------------------------------------------------------------

/// 真实 EPUB 的元数据 / 目录 / 正文 / 内嵌资源四项都要可用（FR-021）。
#[test]
fn real_epub_yields_metadata_toc_and_resources() {
    let Some(bytes) = corpus("2701.epub") else {
        report_skip("2701.epub");
        return;
    };

    let parsed = tome_local::epub::import_bytes(&bytes).expect("真实 EPUB 应能解析");

    // 元数据：Dublin Core 的 title / creator。
    assert_eq!(parsed.book.name, "Moby Dick; Or, The Whale");
    assert_eq!(parsed.book.author.as_deref(), Some("Herman Melville"));

    // 目录（NCX / nav）：应产出上百章且**条目路径已解析为 zip 条目名**。
    assert!(
        parsed.toc.len() > 50,
        "应识别出大量章节，实际 {}",
        parsed.toc.len()
    );
    for point in &parsed.toc {
        assert!(
            !point.entry.starts_with('/'),
            "条目名不该以 / 开头（实际 {:?}）",
            point.entry
        );
        // 最经典的错误：直接用 OPF 的 `href` 而不补 OPF 所在目录。
        // 那会得到 `Text/ch1.xhtml` 而不是 `OEBPS/Text/ch1.xhtml`。
        assert!(
            point.entry.starts_with("OEBPS/"),
            "条目名应含 OPF 目录前缀，实际 {:?}",
            point.entry
        );
    }

    // 内嵌资源（FR-021 明确要求解析）。
    assert!(!parsed.resources.is_empty(), "应解析出内嵌资源清单");

    // 正文：按目录里的条目名读一章，必须是可读文本（不含标签）。
    let first = &parsed.toc[0];
    let raw = tome_local::epub::read_chapter_bytes(&bytes, &first.entry)
        .expect("读条目")
        .expect("条目应存在");
    let text = tome_local::epub::extract_text(&String::from_utf8_lossy(&raw));
    assert!(
        !text.contains("<p>") && !text.contains("<html"),
        "提取的正文应已去掉标签"
    );
    assert!(
        text.len() > 100,
        "首章正文应有一定长度，实际 {} 字节",
        text.len()
    );

    eprintln!(
        "2701.epub: 书名={:?} 作者={:?} 章节={} 资源={} 首章字符={}",
        parsed.book.name,
        parsed.book.author,
        parsed.toc.len(),
        parsed.resources.len(),
        text.chars().count()
    );
}

// ---------------------------------------------------------------------------
// T079：HUFF/CDIC 的真实文件验证（**当前缺语料，测试会明确报告**）
// ---------------------------------------------------------------------------

/// HUFF/CDIC（`compression = 17480`）的真实文件验证。
///
/// # 这条测试**故意**在缺语料时失败（与其它测试的"跳过"不同）
///
/// T079 的验收项原文是"真实 `kindlegen -c2` 文件验证；与 oracle
/// （kindling dump）diff 一致"。那是一件**尚未完成**的事——本仓当前
/// **没有**可用的 `-c2` 语料：
///
/// - Project Gutenberg 的 Kindle 格式（`kf8.images` / `kindle.noimages` /
///   `kindle.images`）实测**全部**是 `compression = 2`（PalmDOC）
/// - 本机无 `kindlegen`，也无 Calibre（`ebook-convert`）可以生成 `-c2`
/// - Calibre 源码树里没有 `.mobi` 测试夹具（`.gitignore` 排除了）
///
/// 故 HUFF/CDIC 目前只有**按 `mobi-rs/huff.md` 规范实现的单元测试**覆盖，
/// 尚未经真实文件端到端验证。把这条测试写成"缺语料即失败"是**刻意的**：
/// 它让这个缺口在每次 `cargo test` 里都可见，而不是伪装成"跳过=通过"。
///
/// 语料就位后（把任意 `kindlegen -c2` 产物命名 `huffcdic.mobi` 放进
/// `testdata/captures/local-books/`），用下面这条命令跑它：
///
/// ```text
/// cargo test -p tome-local --test mobi_real_files -- --ignored huffcdic
/// ```
///
/// ## 为什么用 `#[ignore]` 而不是直接失败
///
/// 直接失败会让四道质量门（build/clippy/fmt/test）全红——那不是"暴露缺口"，
/// 而是"让缺口挡住其余全部工作"。`#[ignore]` + `--nocapture` 的输出是
/// Rust 里表达"需要外部夹具"的标准做法：测试计数里显示为 `ignored`
/// （而不是 `passed`），且 `report_which_compression_paths_the_corpus_covers`
/// 每次运行都会打印 `HUFF/CDIC=false` 让缺口**持续可见**。
#[test]
#[ignore = "T079 真实文件验证缺语料：需要一个 kindlegen -c2（compression=17480）的 MOBI；本机与 Gutenberg 语料均无法提供，见本测试的文档"]
fn real_huffcdic_file_verifies_the_phase_two_path() {
    let Some(bytes) = corpus("huffcdic.mobi") else {
        panic!(
            "T079 的真实文件验证**尚未完成**：{} 下没有 `huffcdic.mobi`\
             （compression=17480 的 kindlegen -c2 产物）。\
             本仓当前的 Gutenberg 语料全部是 PalmDOC，无法覆盖 HUFF/CDIC 路径。\
             把一份 `-c2` 文件放到该目录即可让本测试开始验证。",
            corpus_dir().display()
        );
    };

    // 语料就位时的真正断言。
    let parsed = mobi::parse(&bytes).expect("真实 HUFF/CDIC 文件应能解析");
    assert!(
        parsed.text.len() > 10_000,
        "HUFF/CDIC 正文应有实际长度，实际 {}",
        parsed.text.len()
    );
    let replacement_count = parsed.text.matches('\u{FFFD}').count();
    assert!(
        replacement_count < 10,
        "解压结果不该有大量替换字符（那是解压错误的信号），实际 {replacement_count} 个"
    );
    assert!(!parsed.toc.is_empty(), "应识别出章节");
    eprintln!(
        "huffcdic.mobi: 书名={:?} 章节={} 正文字符={}",
        parsed.book.name,
        parsed.toc.len(),
        parsed.text.chars().count()
    );
}

/// 报告当前语料里各文件的压缩算法（诊断用，始终通过）。
///
/// 这条测试的价值在于把"哪些压缩路径被真实覆盖了"变成**可读的输出**，
/// 而不是要人去逐个文件手工检查。
#[test]
fn report_which_compression_paths_the_corpus_covers() {
    let names = [
        "368.kf8.mobi",
        "2701.kf8.mobi",
        "1342.kf8.mobi",
        "580.kf8.mobi",
        "2701.epub",
        "huffcdic.mobi",
    ];
    let mut covered_palmdoc = false;
    let mut covered_huffcdic = false;

    for name in names {
        let Some(bytes) = corpus(name) else {
            eprintln!("  {name}: （无）");
            continue;
        };
        if name.ends_with(".epub") {
            eprintln!("  {name}: EPUB");
            continue;
        }
        // PalmDOC 头的前 2 字节是压缩算法（大端）。
        let record0 = u32::from_be_bytes([
            bytes.get(78).copied().unwrap_or(0),
            bytes.get(79).copied().unwrap_or(0),
            bytes.get(80).copied().unwrap_or(0),
            bytes.get(81).copied().unwrap_or(0),
        ]) as usize;
        let compression = bytes
            .get(record0..record0 + 2)
            .map(|slice| u16::from_be_bytes([slice[0], slice[1]]))
            .unwrap_or(0);
        match compression {
            2 => covered_palmdoc = true,
            17480 => covered_huffcdic = true,
            _ => {}
        }
        let label = match compression {
            2 => "（PalmDOC）",
            17480 => "（HUFF/CDIC）",
            _ => "",
        };
        eprintln!("  {name}: compression={compression}{label}");
    }

    eprintln!("真实语料覆盖：PalmDOC={covered_palmdoc} HUFF/CDIC={covered_huffcdic}");
    assert!(covered_palmdoc, "至少要覆盖 PalmDOC（当前语料应满足）");
    if !covered_huffcdic {
        eprintln!(
            "⚠ HUFF/CDIC 路径**未**被真实文件覆盖——见 \
             `real_huffcdic_file_verifies_the_phase_two_path` 的说明"
        );
    }
}
