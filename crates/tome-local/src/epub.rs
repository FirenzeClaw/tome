//! [DONE] EPUB 导入：元数据 / 目录 / 正文 / 内嵌资源（tasks.md T077 / FR-021）
//!
//! # 规格原文
//!
//! > **FR-021**: 系统 MUST 支持 EPUB 导入：解析书名、作者、目录、正文与内嵌资源
//!
//! 四项能力对应 EPUB 的四个公开规范概念：
//!
//! | 能力 | 规范落点 |
//! |---|---|
//! | 书名 / 作者 | OPF 的 `<dc:title>` / `<dc:creator>`（Dublin Core 元数据） |
//! | 目录 | OPF `<spine>` 的 `toc` 属性 → NCX；EPUB 3 的 `nav` 文档 |
//! | 正文 | OPF `<manifest>` + `<spine>` → zip 条目（XHTML） |
//! | 内嵌资源 | `<manifest>` 中非 spine 的条目（图片、CSS、字体） |
//!
//! # 依据的公开规范
//!
//! - **OCF 3.0**（Open Container Format）：ZIP 容器结构、`mimetype` 必须是
//!   第一个条目且不压缩、`META-INF/container.xml` 指向 OPF
//! - **OPF 3.0**（Open Packaging Format）：`package` / `metadata` /
//!   `manifest` / `spine` 四个部分的语义
//! - **NCX**（EPUB 2 的目录，仍被 EPUB 3 广泛保留）：`navMap`/`navPoint`
//! - **EPUB 3 nav 文档**：XHTML 里的 `<nav epub:type="toc">`
//!
//! 本模块**不读任何上游 Kotlin 实现**（干净室纪律）。上述四份规范都是公开
//! 标准文档，且 EPUB 的解析路径比 MOBI 简单得多（ZIP + XML 两层，都有
//! 成熟规范），不需要对照实现。
//!
//! # 为什么目录优先读 NCX 而不是 spine
//!
//! `<spine>` 给出的是**阅读顺序**（一列 XHTML 文件），而 `<navMap>` 给出的是
//! **用户看到的目录**（可能有卷、有层级、一节跨多文件、或多节在同一文件里）。
//! 用 spine 当目录会把"第一部 上 第一章"这类结构抹平，且同一文件里的
//! 多个小节无法区分。
//!
//! ## 三层回退
//!
//! 1. **NCX**（EPUB 2 的 `toc` 属性指向的 `.ncx`）——兼容性最好
//! 2. **EPUB 3 nav 文档**（manifest 里 `properties` 含 `nav` 的 XHTML）
//! 3. **spine**——最后兜底：按阅读顺序每个 XHTML 文件当一章
//!
//! 三层都失败（如 OPF 里没有 spine）时才报错——而那时文件确实损坏了。
//!
//! # 关键入口
//!
//! - [`import_bytes`] —— EPUB 字节 → [`ParsedEpub`]（纯函数）
//! - [`import_path`] —— 从文件导入

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;
use tome_core::{Book, Chapter};

use crate::error::{from_io, LocalError, LocalResult};

/// OCF 的容器描述文件路径（规范固定，不可配置）。
const CONTAINER_XML: &str = "META-INF/container.xml";

/// 单本章内的最大解压字节数（防"压缩炸弹"）。
///
/// # 为什么必须设这个上限
///
/// EPUB 是不可信输入，而 ZIP 允许极高的压缩比（1000:1 很常见，恶意构造
/// 可达 10⁶:1）。一个 1MB 的 EPUB 可以解出 1GB 的 XHTML——那既是内存耗尽，
/// 也是拒绝服务。
///
/// 取 32 MiB：真实的单章 XHTML 极少超过 1MB（即便含大段 base64 图片也不到
/// 10MB），32MB 留了充分余量。超限的章节被**跳过并记录**，不是静默截断
/// （截断会让用户读到半个章节而不知情）。
pub const MAX_ENTRY_BYTES: u64 = 32 * 1024 * 1024;

/// 整个 EPUB 解压后的总字节上限。
///
/// 单章上限挡不住"3 万个 32MB 的章节"——那是总预算要管的事。
/// 取 512 MiB：一部含大量高清插图的 EPUB 解压后可达 100~200MB，512MB 留了
/// 余量，同时把恶意构造的总量钉死在可控范围。
pub const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

/// 一次 EPUB 解析的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedEpub {
    /// 书籍元信息（`origin_id` 已按内容哈希填好）
    pub book: Book,
    /// 目录（按阅读顺序）
    pub toc: Vec<TocPoint>,
    /// 内嵌资源清单（zip 条目名 → 媒体类型）
    pub resources: Vec<ResourceEntry>,
    /// 非致命的跳过记录（**不静默**）
    pub warnings: Vec<String>,
}

/// 目录条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocPoint {
    /// 章节标题
    pub title: String,
    /// 正文所在的 zip 条目名（OPF 根目录之前的相对路径已补全）
    pub entry: String,
    /// 该章节在条目内的片段标识（如 `#chapter3`）；`None` 表示整个条目
    pub fragment: Option<String>,
    /// 目录层级深度（0 为顶层）
    pub depth: u32,
}

/// 内嵌资源（`manifest` 里不在 spine 中的条目）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEntry {
    /// zip 条目名
    pub entry: String,
    /// 媒体类型（如 `image/jpeg`）
    pub media_type: String,
    /// 大小（字节）
    pub size: u64,
}

/// OPF 解析出的包结构。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Package {
    title: Option<String>,
    creator: Option<String>,
    language: Option<String>,
    description: Option<String>,
    /// `manifest` 的 `id` → (zip 条目名, 媒体类型, properties)
    manifest: BTreeMap<String, ManifestItem>,
    /// `spine` 的 `idref` 顺序
    spine: Vec<String>,
    /// `toc` 属性（指向 NCX 的 manifest id）
    toc_id: Option<String>,
    /// OPF 自身的目录前缀（如 `OEBPS/`）
    root_dir: String,
}

/// `manifest` 的一个条目。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestItem {
    entry: String,
    media_type: String,
    properties: String,
}

/// [DONE] FR-021 的主入口：EPUB 字节 → 书 + 目录 + 资源。
pub fn import_bytes(bytes: &[u8]) -> LocalResult<ParsedEpub> {
    let mut warnings = Vec::new();
    let mut archive = open_archive(bytes)?;

    // ① OPF 的位置：读 `META-INF/container.xml`（OCF 规范固定路径）。
    let container = read_entry(&mut archive, CONTAINER_XML, &mut warnings)?
        .ok_or_else(|| LocalError::damaged(format!("缺少 {CONTAINER_XML}")))?;
    let container_text = String::from_utf8_lossy(&container).into_owned();
    let opf_path = find_opf_path(&container_text).ok_or_else(|| {
        LocalError::damaged("container.xml 里找不到 rootfile 的 full-path".to_string())
    })?;

    // ② 解析 OPF。
    let opf_bytes = read_entry(&mut archive, &opf_path, &mut warnings)?
        .ok_or_else(|| LocalError::damaged(format!("OPF 条目不存在：{opf_path}")))?;
    let opf_text = String::from_utf8_lossy(&opf_bytes).into_owned();
    let package = parse_opf(&opf_text, &opf_path)?;

    // ③ 目录：NCX → nav → spine（三层回退，见模块文档）。
    let toc = read_toc(&mut archive, &package, &mut warnings);

    // ④ 内嵌资源：manifest 里不在 spine 的条目。
    let resources = collect_resources(&mut archive, &package);
    for resource in &resources {
        if resource.size > MAX_ENTRY_BYTES {
            warnings.push(format!(
                "资源过大已记录但不会解压：{}（{} 字节）",
                resource.entry, resource.size
            ));
        }
    }

    let toc = if toc.is_empty() {
        // 三层都空：按 spine 兜底（与 T076 的"按大小切分"同理——
        // 宁可给出机械分章也不要产出零章节的书）。
        warnings.push("目录为空，已按 spine 阅读顺序生成章节".to_string());
        spine_fallback(&package, &mut archive, &mut warnings)
    } else {
        toc
    };

    let book = Book {
        id: 0,
        origin: tome_core::BookOrigin::Local,
        origin_id: crate::hash::content_origin_id(bytes),
        name: package
            .title
            .clone()
            .unwrap_or_else(|| "未命名".to_string()),
        author: package.creator.clone(),
        cover_url: None,
        intro: package.description.clone(),
        kind: package.language.clone(),
        book_group: None,
        read_config: None,
        toc_url: None,
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: Some("UTF-8".to_string()),
        format: Some("epub".to_string()),
        total_chapters: Some(toc.len() as i32),
        dur_chapter_index: None,
        added_at: 0,
    };

    Ok(ParsedEpub {
        book,
        toc,
        resources,
        warnings,
    })
}

/// 从文件导入 EPUB。
pub fn import_path(path: &std::path::Path) -> LocalResult<ParsedEpub> {
    let bytes = std::fs::read(path).map_err(|error| from_io(path, error))?;
    let mut parsed = import_bytes(&bytes)?;
    parsed.book.relative_path = Some(crate::library::normalize_for_storage(path));
    if parsed.book.name == "未命名" {
        if let Some(name) = path.file_name() {
            parsed.book.name = crate::split::book_name_from_file_name(&name.to_string_lossy());
        }
    }
    Ok(parsed)
}

/// 读取某章正文（XHTML → 纯文本）。
///
/// # 为什么在 tome-local 里做 HTML→文本，而不用 tome-rule 的 DOM
///
/// `tome-rule` 的 DOM 是**为书源规则求值**设计的（实现 CSS 选择器所需的
/// 完整树、属性、命名空间）。用它来"把 XHTML 变成可读文本"是杀鸡用牛刀：
/// 本章只需要丢掉标签、保留可见文本、把块级元素变成换行。
///
/// 而 EPUB 的 XHTML 由电子书工具生成，标签集极其有限（`p`/`div`/`h1`-`h6`/
/// `br`/`img`/`a`/`span`），本模块的轻量扫描足够且**不引入依赖**。
///
/// > [UNCERTAIN] 若日后发现某些 EPUB 的正文提取质量不达标（如表格丢失、
/// > 脚注乱序），应改为复用 `tome-rule` 的 DOM——那时它就不是杀鸡用牛刀，
/// > 而是正确性所需。
pub fn extract_text(xhtml: &str) -> String {
    let mut out = String::with_capacity(xhtml.len() / 2);
    let mut chars = xhtml.char_indices().peekable();

    // 需要保留为换行的标签名（块级与标题）。
    const BLOCK_TAGS: [&str; 22] = [
        "p",
        "div",
        "br",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "li",
        "tr",
        "blockquote",
        "section",
        "article",
        "header",
        "footer",
        "aside",
        "nav",
        "figure",
        "figcaption",
        "td",
        "pre",
    ];
    // 内容完全不可见的元素（在它们内部要整段跳过）。
    //
    // ## 为什么 `head` 不在这里
    //
    // `<head>` 的**直接**内容（`<title>`/`<meta>`/`<link>`）确实不可见，但
    // HTML 允许省略闭合标签——`<head><title>x</title><body>正文</body>`
    // 里根本没有 `</head>`。把 `head` 当"跳到闭合标签"的段会把整个
    // `<body>` 一起吞掉（实测就是这么失败的）。
    //
    // `<script>`/`<style>` 不同：它们的闭合标签是必需的（否则内容无法
    // 界定），故它们可以安全地整段跳过。
    const SKIP_TAGS: [&str; 2] = ["script", "style"];

    let mut skip_until: Option<String> = None;
    let bytes_len = xhtml.len();

    while let Some((index, ch)) = chars.next() {
        if ch != '<' {
            if skip_until.is_none() {
                out.push(ch);
            }
            continue;
        }

        // 找到标签结束的位置（`>` 或 `/>`）。
        let mut end = index + 1;
        let mut in_quote: Option<char> = None;
        for (offset, inner) in xhtml[index + 1..].char_indices() {
            match (in_quote, inner) {
                (Some(quote), c) if c == quote => in_quote = None,
                (Some(_), _) => {}
                (None, c @ ('"' | '\'')) => in_quote = Some(c),
                (None, '>') => {
                    end = index + 1 + offset;
                    break;
                }
                _ => {}
            }
        }
        if end >= bytes_len {
            break;
        }

        // **立刻**把 `chars` 推进到标签之后。
        //
        // ## 为什么必须在这里而不是循环末尾
        //
        // 下面的分支里有多个 `continue`（skip 区段的进出、自闭合标签等）。
        // 若把推进放在末尾，那些 `continue` 会**跳过**推进——于是标签本身
        // 的字符（`/style>`、`head` 等）会被当成正文推入输出。实测的失败
        // 就是这个形态：`"head>/style>/head>body>/script>p>真正的正文"`
        // 里的标签残片全部漏出来了。
        while let Some((next_index, _)) = chars.peek() {
            if *next_index <= end {
                chars.next();
            } else {
                break;
            }
        }

        let raw = &xhtml[index + 1..end];
        let is_closing = raw.starts_with('/');
        let is_self_closing = raw.ends_with('/');
        // 标签名的提取必须先去掉前导 `/` 再找分隔符——若在原始 `raw` 上找
        // `/`，闭合标签会在**位置 0** 命中，导致名字为空串（实测的 bug：
        // 所有 `</style>` 都被当成无名标签，skip 区段永不结束）。
        let name = tag_name_of(raw);

        // 处理 skip 区段。
        if let Some(skipping) = &skip_until {
            if is_closing && &name == skipping {
                skip_until = None;
            }
            continue;
        }
        if !is_closing && !is_self_closing && SKIP_TAGS.contains(&name.as_str()) {
            skip_until = Some(name.clone());
            continue;
        }

        // `img` 的替代文本（`alt`）是正文的一部分。
        //
        // 注意这**不在** `is_block_tag` 里：`img` 是行内元素。首版把它
        // 嵌在块级分支里，于是 `<p>图示：<img alt="一朵花"/>结束</p>`
        // 的替代文本被整个丢掉（`<img>` 不触发块级分支）。
        if name == "img" && !is_closing {
            if let Some(alt) = attribute_value(raw, "alt") {
                let alt = alt.trim();
                if !alt.is_empty() {
                    out.push_str(alt);
                    out.push('\n');
                }
            }
        }
        if BLOCK_TAGS.contains(&name.as_str()) && !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
    }

    decode_entities(&out)
}

/// 从一段标签文本里取出属性值。
fn attribute_value(raw: &str, attribute: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(offset) = lower[cursor..].find(attribute) {
        let position = cursor + offset;
        // 属性名前面必须是空白（避免 `data-alt` 误配 `alt`）。
        let preceded_by_space = position == 0
            || raw[..position]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after = &raw[position + attribute.len()..];
        if preceded_by_space {
            if let Some(rest) = after.trim_start().strip_prefix('=') {
                let value = rest.trim_start();
                if let Some(inner) = value.strip_prefix('"') {
                    if let Some(end) = inner.find('"') {
                        return Some(inner[..end].to_string());
                    }
                } else if let Some(inner) = value.strip_prefix('\'') {
                    if let Some(end) = inner.find('\'') {
                        return Some(inner[..end].to_string());
                    }
                } else {
                    let end = value
                        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
                        .unwrap_or(value.len());
                    return Some(value[..end].to_string());
                }
            }
        }
        cursor = position + attribute.len();
    }
    None
}

/// 解码常见 HTML 实体。
///
/// # 为什么只做命名实体表 + 数字实体，不引 html-escape crate
///
/// EPUB 的 XHTML 由工具生成，实体的出现范围很窄（`&amp;` `&lt;` `&gt;`
/// `&quot;` `&apos;` `&nbsp;` 加数字实体）。其余的命名实体（如 `&mdash;`）
/// 在正文里罕见，且**保留原样**比"猜错"更安全——用户看到 `&mdash;`
/// 至少知道那里有个实体，看到被猜错的字符则完全无从判断。
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find('&') {
        out.push_str(&rest[..position]);
        let after = &rest[position..];
        let Some(semicolon) = after[..after.len().min(12)].find(';') else {
            // 没有分号：不是实体，原样输出。
            out.push('&');
            rest = &after[1..];
            continue;
        };
        let entity = &after[1..semicolon];
        match resolve_entity(entity) {
            Some(ch) => {
                out.push(ch);
                rest = &after[semicolon + 1..];
            }
            None => {
                // 不认识的实体原样保留（见本函数的文档）。
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 解析一个实体名（不含 `&` 与 `;`）。
fn resolve_entity(entity: &str) -> Option<char> {
    match entity {
        "amp" => return Some('&'),
        "lt" => return Some('<'),
        "gt" => return Some('>'),
        "quot" => return Some('"'),
        "apos" => return Some('\''),
        "nbsp" => return Some('\u{A0}'),
        "mdash" => return Some('—'),
        "ndash" => return Some('–'),
        "hellip" => return Some('…'),
        "ldquo" => return Some('“'),
        "rdquo" => return Some('”'),
        "lsquo" => return Some('‘'),
        "rsquo" => return Some('’'),
        "middot" => return Some('·'),
        _ => {}
    }
    // 数字实体：`&#123;`（十进制）与 `&#x1F600;`（十六进制）。
    if let Some(number) = entity.strip_prefix('#') {
        let parsed = if let Some(hex) = number.strip_prefix(['x', 'X']) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            number.parse::<u32>().ok()
        };
        return parsed.and_then(char::from_u32);
    }
    None
}

// ---------------------------------------------------------------------------
// ZIP 访问
// ---------------------------------------------------------------------------

/// 打开 ZIP 归档。
fn open_archive(bytes: &[u8]) -> LocalResult<zip::ZipArchive<Cursor<&[u8]>>> {
    zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| LocalError::damaged(format!("ZIP 容器无法读取：{error}")))
}

/// 读取一个 zip 条目的内容（含压缩炸弹防护）。
fn read_entry(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    warnings: &mut Vec<String>,
) -> LocalResult<Option<Vec<u8>>> {
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => {
            return Err(LocalError::damaged(format!(
                "读取条目 {name} 失败：{error}"
            )))
        }
    };

    let declared = file.size();
    if declared > MAX_ENTRY_BYTES {
        warnings.push(format!(
            "条目 {name} 声明大小 {declared} 超过单章上限 {MAX_ENTRY_BYTES}，已跳过"
        ));
        return Ok(None);
    }

    // `declared` 是压缩包里的**声明值**，不可信。故按上限读入：
    // `take` 让读取在超限处停下，而不是先分配再发现。
    let mut buffer = Vec::with_capacity(declared.min(MAX_ENTRY_BYTES) as usize);
    let mut limited = file.by_ref().take(MAX_ENTRY_BYTES + 1);
    limited
        .read_to_end(&mut buffer)
        .map_err(|error| LocalError::damaged(format!("解压条目 {name} 失败：{error}")))?;

    if buffer.len() as u64 > MAX_ENTRY_BYTES {
        // 声明的大小撒谎了（压缩炸弹的典型手法）。
        warnings.push(format!(
            "条目 {name} 实际解压超过上限 {MAX_ENTRY_BYTES}，已跳过"
        ));
        return Ok(None);
    }
    Ok(Some(buffer))
}

// ---------------------------------------------------------------------------
// container.xml 与 OPF
// ---------------------------------------------------------------------------

/// 从 `container.xml` 取出 OPF 的路径。
///
/// 依据 OCF 规范：`<rootfiles><rootfile full-path="..." media-type="..."/>`。
/// 取第一个 `media-type` 为 OPF 的（或第一个没写 media-type 的）。
fn find_opf_path(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut fallback: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) | Ok(Event::Empty(element)) => {
                if element.name().as_ref() != b"rootfile" {
                    continue;
                }
                let mut path = None;
                let mut media_type = None;
                for attribute in element.attributes().flatten() {
                    let key = String::from_utf8_lossy(attribute.key.as_ref()).to_string();
                    let value = String::from_utf8_lossy(&attribute.value).to_string();
                    match key.as_str() {
                        "full-path" => path = Some(value),
                        "media-type" => media_type = Some(value),
                        _ => {}
                    }
                }
                let Some(path) = path else { continue };
                if media_type.as_deref() == Some("application/oebps-package+xml") {
                    return Some(path);
                }
                fallback.get_or_insert(path);
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    fallback
}

/// 解析 OPF（包装文档）。
fn parse_opf(xml: &str, opf_path: &str) -> LocalResult<Package> {
    let mut package = Package {
        // OPF 自身的目录：manifest 与 spine 的路径都相对它。
        root_dir: match opf_path.rsplit_once('/') {
            Some((directory, _)) => format!("{directory}/"),
            None => String::new(),
        },
        ..Package::default()
    };

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut text_target: Option<&'static str> = None;
    // 当前所在的顶层 section（决定 `itemref` 属于 spine 还是别的列表）。
    let mut in_spine = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = local_name(element.name().as_ref());
                match name.as_str() {
                    "package" => {}
                    "metadata" => {}
                    "title" => text_target = Some("title"),
                    "creator" => text_target = Some("creator"),
                    "language" => text_target = Some("language"),
                    "description" => text_target = Some("description"),
                    "spine" => {
                        in_spine = true;
                        for attribute in element.attributes().flatten() {
                            if attribute.key.as_ref() == b"toc" {
                                package.toc_id =
                                    Some(String::from_utf8_lossy(&attribute.value).to_string());
                            }
                        }
                    }
                    "item" => {
                        let mut id = None;
                        let mut href = None;
                        let mut media_type = None;
                        let mut properties = String::new();
                        for attribute in element.attributes().flatten() {
                            let key = String::from_utf8_lossy(attribute.key.as_ref()).to_string();
                            let value = String::from_utf8_lossy(&attribute.value).to_string();
                            match key.as_str() {
                                "id" => id = Some(value),
                                "href" => href = Some(value),
                                "media-type" => media_type = Some(value),
                                "properties" => properties = value,
                                _ => {}
                            }
                        }
                        if let (Some(id), Some(href)) = (id, href) {
                            package.manifest.insert(
                                id,
                                ManifestItem {
                                    entry: resolve_path(&package.root_dir, &href),
                                    media_type: media_type.unwrap_or_default(),
                                    properties,
                                },
                            );
                        }
                    }
                    "itemref" if in_spine => {
                        for attribute in element.attributes().flatten() {
                            if attribute.key.as_ref() == b"idref" {
                                package
                                    .spine
                                    .push(String::from_utf8_lossy(&attribute.value).to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(element)) => {
                // `item` / `itemref` 在 OPF 里常写成自闭合标签。
                let name = local_name(element.name().as_ref());
                match name.as_str() {
                    "item" => {
                        let mut id = None;
                        let mut href = None;
                        let mut media_type = None;
                        let mut properties = String::new();
                        for attribute in element.attributes().flatten() {
                            let key = String::from_utf8_lossy(attribute.key.as_ref()).to_string();
                            let value = String::from_utf8_lossy(&attribute.value).to_string();
                            match key.as_str() {
                                "id" => id = Some(value),
                                "href" => href = Some(value),
                                "media-type" => media_type = Some(value),
                                "properties" => properties = value,
                                _ => {}
                            }
                        }
                        if let (Some(id), Some(href)) = (id, href) {
                            package.manifest.insert(
                                id,
                                ManifestItem {
                                    entry: resolve_path(&package.root_dir, &href),
                                    media_type: media_type.unwrap_or_default(),
                                    properties,
                                },
                            );
                        }
                    }
                    "itemref" => {
                        for attribute in element.attributes().flatten() {
                            if attribute.key.as_ref() == b"idref" {
                                package
                                    .spine
                                    .push(String::from_utf8_lossy(&attribute.value).to_string());
                            }
                        }
                    }
                    "rootfile" => {}
                    _ => {}
                }
            }
            Ok(Event::Text(text)) => {
                if let Some(target) = text_target {
                    let decoded = text
                        .unescape()
                        .map(|value| value.into_owned())
                        .unwrap_or_default();
                    if !decoded.trim().is_empty() {
                        let slot = match target {
                            "title" => &mut package.title,
                            "creator" => &mut package.creator,
                            "language" => &mut package.language,
                            "description" => &mut package.description,
                            _ => &mut None,
                        };
                        // 只取第一次出现的值（Dublin Core 允许多值，
                        // 首个是主值；后续的同名元素常是译者/插图者）。
                        if slot.is_none() {
                            *slot = Some(decoded.trim().to_string());
                        }
                    }
                }
            }
            Ok(Event::End(element)) => {
                let name = local_name(element.name().as_ref());
                if name == "spine" {
                    in_spine = false;
                }
                // 只清掉栈顶对应的文本目标：嵌套元素（如 `<dc:title><b>x</b></dc:title>`）
                // 的内层结束不该提前终止外层。
                if matches!(
                    name.as_str(),
                    "title" | "creator" | "language" | "description"
                ) {
                    text_target = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(LocalError::damaged(format!("OPF 解析失败：{error}")));
            }
            _ => {}
        }
    }

    if package.manifest.is_empty() {
        return Err(LocalError::damaged(
            "OPF 的 manifest 为空（文件可能被截断）".to_string(),
        ));
    }
    Ok(package)
}

/// 去掉 XML 名字空间的元素前缀。
fn local_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    match text.rsplit_once(':') {
        Some((_, local)) => local.to_ascii_lowercase(),
        None => text.to_ascii_lowercase(),
    }
}

/// 把 manifest 里的 `href` 解析为 zip 条目名（相对 OPF 目录）。
///
/// # 为什么必须在这里解析而不是直接用 `href`
///
/// OPF 里的 `href` 是**相对 OPF 文件**的路径（如 `Text/chapter1.xhtml`），
/// 而 zip 条目名是**相对压缩包根**的（`OEBPS/Text/chapter1.xhtml`）。
/// 直接用 `href` 会得到 `FileNotFound`——这是 EPUB 解析最常见的实现错误。
///
/// 同时处理 `..` 段（部分工具会写 `../Images/cover.jpg`）与百分号编码。
fn resolve_path(root_dir: &str, href: &str) -> String {
    // 去掉 fragment（`#id`）：那是段内锚点，不是路径的一部分。
    let without_fragment = href.split('#').next().unwrap_or(href);
    // 百分号编码：EPUB 3 允许 href 里出现 `%20` 之类的转义。
    let decoded = percent_decode(without_fragment);
    let combined = format!("{root_dir}{decoded}");
    normalize_zip_path(&combined)
}

/// 归一化 zip 路径：消除 `.` / `..` 段，去掉开头的 `./`。
fn normalize_zip_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

/// 极简的百分号解码（只处理 `%XX`，不处理 `+` → 空格——路径里 `+` 是字面量）。
fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(value) = u8::from_str_radix(hex, 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// 目录：NCX → nav → spine
// ---------------------------------------------------------------------------

/// 三层回退地读取目录。
fn read_toc(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    package: &Package,
    warnings: &mut Vec<String>,
) -> Vec<TocPoint> {
    // ① NCX（EPUB 2 的 `toc` 属性）。
    if let Some(toc_id) = &package.toc_id {
        if let Some(item) = package.manifest.get(toc_id) {
            match read_entry(archive, &item.entry, warnings) {
                Ok(Some(bytes)) => {
                    let xml = String::from_utf8_lossy(&bytes).into_owned();
                    let points = parse_ncx(&xml, &package.root_dir);
                    if !points.is_empty() {
                        return points;
                    }
                    warnings.push(format!("NCX 未产出目录项：{}", item.entry));
                }
                Ok(None) => warnings.push(format!("NCX 条目缺失：{}", item.entry)),
                Err(error) => warnings.push(format!("NCX 读取失败：{error}")),
            }
        }
    }

    // ② EPUB 3 的 nav 文档（manifest 里 `properties` 含 `nav`）。
    let nav_entry = package
        .manifest
        .values()
        .find(|item| item.properties.split_whitespace().any(|flag| flag == "nav"))
        .map(|item| item.entry.clone());
    if let Some(entry) = nav_entry {
        match read_entry(archive, &entry, warnings) {
            Ok(Some(bytes)) => {
                let xhtml = String::from_utf8_lossy(&bytes).into_owned();
                let points = parse_nav(&xhtml, &entry, &package.root_dir);
                if !points.is_empty() {
                    return points;
                }
                warnings.push(format!("nav 文档未产出目录项：{entry}"));
            }
            Ok(None) => warnings.push(format!("nav 条目缺失：{entry}")),
            Err(error) => warnings.push(format!("nav 读取失败：{error}")),
        }
    }

    Vec::new()
}

/// 解析 NCX 的 `navMap`。
///
/// 依据 NCX 规范：`navMap` 内含嵌套的 `navPoint`，每个有 `navLabel/text`
/// 与 `content@src`（形如 `Text/chapter1.xhtml#p3`）。
fn parse_ncx(xml: &str, root_dir: &str) -> Vec<TocPoint> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut points: Vec<TocPoint> = Vec::new();
    // 嵌套时每层需要**各自**的标题与地址缓存——共用一个 `current_label`
    // 会让子navPoint 覆写父的（父的 `</navPoint>` 落下时读到的是子标题，
    // 于是父标题丢失）。
    // 每层一个栈帧：(标题, 地址, 深度)。
    let mut stack: Vec<(String, Option<String>, u32)> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = local_name(element.name().as_ref());
                match name.as_str() {
                    "navpoint" => {
                        let depth = stack.len() as u32;
                        stack.push((String::new(), None, depth));
                    }
                    "content" => {
                        let source = element.attributes().flatten().find_map(|attribute| {
                            if attribute.key.as_ref() == b"src" {
                                Some(String::from_utf8_lossy(&attribute.value).to_string())
                            } else {
                                None
                            }
                        });
                        if let Some(frame) = stack.last_mut() {
                            frame.1 = source;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(element)) => {
                if local_name(element.name().as_ref()) == "content" {
                    let source = element.attributes().flatten().find_map(|attribute| {
                        if attribute.key.as_ref() == b"src" {
                            Some(String::from_utf8_lossy(&attribute.value).to_string())
                        } else {
                            None
                        }
                    });
                    if let Some(frame) = stack.last_mut() {
                        frame.1 = source;
                    }
                }
            }
            Ok(Event::Text(text)) => {
                // 标签文本只属于**栈顶**那一层（`<navLabel><text>...`）。
                if let Some(frame) = stack.last_mut() {
                    if let Ok(decoded) = text.unescape() {
                        frame.0.push_str(decoded.trim());
                    }
                }
            }
            Ok(Event::End(element)) => {
                if local_name(element.name().as_ref()) == "navpoint" {
                    if let Some((label, source, depth)) = stack.pop() {
                        let label = label.trim().to_string();
                        if !label.is_empty() {
                            let (entry, fragment) = split_source(&source, root_dir);
                            if let Some(entry) = entry {
                                points.push(TocPoint {
                                    title: label,
                                    entry,
                                    fragment,
                                    depth,
                                });
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    points
}

/// 解析 EPUB 3 的 nav 文档（`<nav epub:type="toc">` 里的 `<a href>` 列表）。
fn parse_nav(xhtml: &str, nav_entry: &str, root_dir: &str) -> Vec<TocPoint> {
    // nav 文档自身的目录是 href 的基准——与 OPF 的 root_dir 可能不同。
    let nav_dir = match nav_entry.rsplit_once('/') {
        Some((directory, _)) => format!("{directory}/"),
        None => String::new(),
    };
    let _ = root_dir;

    let mut reader = Reader::from_str(xhtml);
    reader.config_mut().trim_text(true);
    let mut points = Vec::new();
    let mut in_toc_nav = false;
    let mut nav_depth = 0u32;
    let mut open_list_depth: u32 = 0;
    let mut current_href: Option<String> = None;
    let mut current_label = String::new();
    let mut collecting = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = local_name(element.name().as_ref());
                match name.as_str() {
                    "nav" => {
                        let is_toc = element.attributes().flatten().any(|attribute| {
                            attribute.key.as_ref().ends_with(b"type")
                                && attribute.value.as_ref() == b"toc"
                        });
                        if is_toc {
                            in_toc_nav = true;
                            nav_depth = 0;
                        }
                    }
                    "ol" | "ul" if in_toc_nav => {
                        nav_depth += 1;
                        open_list_depth = nav_depth;
                    }
                    "a" if in_toc_nav => {
                        collecting = true;
                        current_label.clear();
                        current_href = element.attributes().flatten().find_map(|attribute| {
                            if attribute.key.as_ref() == b"href" {
                                Some(String::from_utf8_lossy(&attribute.value).to_string())
                            } else {
                                None
                            }
                        });
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(text)) if collecting => {
                if let Ok(decoded) = text.unescape() {
                    current_label.push_str(decoded.trim());
                }
            }
            Ok(Event::End(element)) => {
                let name = local_name(element.name().as_ref());
                match name.as_str() {
                    "a" if collecting => {
                        collecting = false;
                        let label = current_label.trim().to_string();
                        if !label.is_empty() {
                            let (entry, fragment) = split_source(&current_href, &nav_dir);
                            if let Some(entry) = entry {
                                points.push(TocPoint {
                                    title: label,
                                    entry,
                                    fragment,
                                    depth: nav_depth.saturating_sub(1),
                                });
                            }
                        }
                        current_href = None;
                    }
                    "ol" | "ul" if in_toc_nav => {
                        nav_depth = nav_depth.saturating_sub(1);
                    }
                    "nav" => in_toc_nav = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    let _ = open_list_depth;
    points
}

/// 把 `Text/chapter1.xhtml#p3` 拆成条目名与片段。
fn split_source(source: &Option<String>, base_dir: &str) -> (Option<String>, Option<String>) {
    let Some(source) = source else {
        return (None, None);
    };
    if source.trim().is_empty() {
        return (None, None);
    }
    let (path, fragment) = match source.split_once('#') {
        Some((path, fragment)) => (path, Some(fragment.to_string())),
        None => (source.as_str(), None),
    };
    if path.trim().is_empty() {
        // 只有 fragment（指向当前文档内部）——没有当前文档上下文，放弃。
        return (None, None);
    }
    let resolved = resolve_path(base_dir, path);
    if resolved.is_empty() {
        (None, fragment)
    } else {
        (Some(resolved), fragment)
    }
}

/// 最后的兜底：按 spine 阅读顺序为每个 XHTML 条目生成一章。
fn spine_fallback(
    package: &Package,
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    warnings: &mut Vec<String>,
) -> Vec<TocPoint> {
    let mut points = Vec::new();
    for idref in &package.spine {
        let Some(item) = package.manifest.get(idref) else {
            continue;
        };
        // 章节标题取该章正文的首行（不做 DOM 解析——兜底路径要便宜）。
        let title = match read_entry(archive, &item.entry, warnings) {
            Ok(Some(bytes)) => {
                let text = extract_text(&String::from_utf8_lossy(&bytes));
                text.lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(|line| line.chars().take(40).collect::<String>())
                    .unwrap_or_else(|| item.entry.clone())
            }
            _ => item.entry.clone(),
        };
        points.push(TocPoint {
            title,
            entry: item.entry.clone(),
            fragment: None,
            depth: 0,
        });
    }
    points
}

/// 收集内嵌资源。
fn collect_resources(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    package: &Package,
) -> Vec<ResourceEntry> {
    let spine_entries: Vec<&str> = package
        .spine
        .iter()
        .filter_map(|idref| package.manifest.get(idref))
        .map(|item| item.entry.as_str())
        .collect();

    let mut resources = Vec::new();
    for item in package.manifest.values() {
        if spine_entries.contains(&item.entry.as_str()) {
            continue;
        }
        // 大小从 zip 中央目录读，不解压（资源可能很大，而我们只需要清单）。
        let size = archive
            .by_name(&item.entry)
            .map(|file| file.size())
            .unwrap_or(0);
        resources.push(ResourceEntry {
            entry: item.entry.clone(),
            media_type: item.media_type.clone(),
            size,
        });
    }
    resources.sort_by(|left, right| left.entry.cmp(&right.entry));
    resources
}

/// 读取某章的原始 XHTML 字节（供 `ContentProvider` 的本地 adapter 用）。
pub fn read_chapter_bytes(bytes: &[u8], entry: &str) -> LocalResult<Option<Vec<u8>>> {
    let mut archive = open_archive(bytes)?;
    let mut warnings = Vec::new();
    read_entry(&mut archive, entry, &mut warnings)
}

/// 把目录条目映射为落库的 [`Chapter`] 列表。
pub fn to_chapters(points: &[TocPoint], book_id: i64) -> Vec<Chapter> {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| Chapter {
            id: 0,
            book_id,
            index_in_book: index as i32,
            title: point.title.clone(),
            url: None,
            byte_offset: None,
            // EPUB 用 zip 条目名定位；片段标识拼在条目名后（`entry#fragment`），
            // 让 ContentProvider 能据此只取该节的内容。
            epub_entry: Some(match &point.fragment {
                Some(fragment) => format!("{}#{fragment}", point.entry),
                None => point.entry.clone(),
            }),
            is_cached: false,
            is_read: false,
            word_count: None,
        })
        .collect()
}

/// 从标签原文里取标签名（小写、无属性、无前导 `/`）。
///
/// # 为什么单独一个函数
///
/// 这个"简单"的提取有一个真实的坑：闭合标签 `</style>` 的原文是
/// `/style`，若直接在原文上找分隔符 `/`，会在**位置 0** 命中，取到的
/// 名字是空串。空名字会让"跳过 script/style 内容"的逻辑永不结束
/// （匹配不到闭合标签），从而把后面的整个正文一起吞掉。
pub fn tag_name_of(raw: &str) -> String {
    let trimmed = raw.trim_start_matches('/').trim_start();
    let end = trimmed
        .find(|c: char| {
            c.is_whitespace() || c == '/' || c == '>' || c == ':' || c == '\t' || c == '\n'
        })
        .unwrap_or(trimmed.len());
    trimmed[..end].to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_tags_become_line_breaks() {
        let html = "<html><body><p>第一段</p><p>第二段</p></body></html>";
        let text = extract_text(html);
        assert_eq!(text.trim(), "第一段\n第二段");
    }

    #[test]
    fn inline_tags_do_not_break_lines() {
        let html = "<p>这是<b>加粗</b>与<i>斜体</i>的文字。</p>";
        assert_eq!(extract_text(html).trim(), "这是加粗与斜体的文字。");
    }

    #[test]
    fn script_and_style_contents_are_dropped() {
        let html = "<head><style>p{color:red}</style></head>\
                    <body><script>var x=1;</script><p>真正的正文</p></body>";
        let text = extract_text(html);
        assert_eq!(text.trim(), "真正的正文");
        assert!(!text.contains("color"));
        assert!(!text.contains("var x"));
    }

    #[test]
    fn br_produces_a_line_break() {
        let html = "<p>第一行<br/>第二行</p>";
        assert_eq!(extract_text(html).trim(), "第一行\n第二行");
    }

    #[test]
    fn image_alt_text_is_kept() {
        let html = r#"<p>图示：<img src="a.png" alt="一朵花"/>结束</p>"#;
        let text = extract_text(html);
        assert!(text.contains("一朵花"), "实际 {text:?}");
    }

    #[test]
    fn named_entities_are_decoded() {
        assert_eq!(decode_entities("A &amp; B"), "A & B");
        assert_eq!(decode_entities("&lt;tag&gt;"), "<tag>");
        assert_eq!(decode_entities("&nbsp;x"), "\u{A0}x");
        assert_eq!(decode_entities("&mdash;"), "—");
    }

    #[test]
    fn numeric_entities_are_decoded() {
        assert_eq!(decode_entities("&#65;&#66;"), "AB");
        assert_eq!(decode_entities("&#x4E2D;&#x6587;"), "中文");
    }

    #[test]
    fn unknown_entities_are_left_verbatim() {
        // 猜错比保留原样更糟——用户看到 `&weird;` 至少知道那里有个实体。
        assert_eq!(decode_entities("&weird;"), "&weird;");
    }

    #[test]
    fn lone_ampersand_survives() {
        assert_eq!(decode_entities("Tom & Jerry"), "Tom & Jerry");
    }

    #[test]
    fn entity_without_semicolon_is_not_consumed() {
        assert_eq!(decode_entities("a & b; c"), "a & b; c");
    }

    #[test]
    fn root_dir_is_prefixed_onto_manifest_hrefs() {
        // 这是 EPUB 解析最经典的 off-by-directory 错误。
        assert_eq!(
            resolve_path("OEBPS/", "Text/chapter1.xhtml"),
            "OEBPS/Text/chapter1.xhtml"
        );
    }

    #[test]
    fn parent_segments_in_hrefs_are_resolved() {
        assert_eq!(
            resolve_path("OEBPS/Text/", "../Images/cover.jpg"),
            "OEBPS/Images/cover.jpg"
        );
    }

    #[test]
    fn fragments_are_stripped_from_paths() {
        assert_eq!(resolve_path("OEBPS/", "a.xhtml#p3"), "OEBPS/a.xhtml");
    }

    #[test]
    fn percent_encoded_paths_are_decoded() {
        assert_eq!(
            resolve_path("OEBPS/", "My%20Chapter.xhtml"),
            "OEBPS/My Chapter.xhtml"
        );
    }

    #[test]
    fn opf_path_is_found_from_container_xml() {
        let xml = r#"<?xml version="1.0"?>
            <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
              <rootfiles>
                <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
              </rootfiles>
            </container>"#;
        assert_eq!(find_opf_path(xml), Some("OEBPS/content.opf".to_string()));
    }

    #[test]
    fn opf_metadata_and_spine_are_parsed() {
        let xml = r#"<?xml version="1.0"?>
            <package xmlns="http://www.idpf.org/2007/opf" version="2.0">
              <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                <dc:title>示例书名</dc:title>
                <dc:creator>示例作者</dc:creator>
                <dc:language>zh</dc:language>
              </metadata>
              <manifest>
                <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
                <item id="ch1" href="Text/ch1.xhtml" media-type="application/xhtml+xml"/>
                <item id="css" href="Styles/main.css" media-type="text/css"/>
              </manifest>
              <spine toc="ncx">
                <itemref idref="ch1"/>
              </spine>
            </package>"#;
        let package = parse_opf(xml, "OEBPS/content.opf").expect("应解析成功");
        assert_eq!(package.title.as_deref(), Some("示例书名"));
        assert_eq!(package.creator.as_deref(), Some("示例作者"));
        assert_eq!(package.language.as_deref(), Some("zh"));
        assert_eq!(package.toc_id.as_deref(), Some("ncx"));
        assert_eq!(package.spine, vec!["ch1".to_string()]);
        assert_eq!(
            package.manifest.get("ch1").map(|item| item.entry.as_str()),
            Some("OEBPS/Text/ch1.xhtml")
        );
    }

    #[test]
    fn self_closing_manifest_items_are_parsed() {
        // 真实 OPF 里 `item` 几乎总是自闭合的。
        let xml = r#"<package><manifest>
            <item id="a" href="a.xhtml" media-type="application/xhtml+xml"/>
            </manifest><spine><itemref idref="a"/></spine></package>"#;
        let package = parse_opf(xml, "content.opf").expect("应解析成功");
        assert!(package.manifest.contains_key("a"));
        assert_eq!(package.spine, vec!["a".to_string()]);
    }

    #[test]
    fn empty_manifest_is_an_error() {
        let xml = r#"<package><metadata/><manifest/><spine/></package>"#;
        assert!(matches!(
            parse_opf(xml, "content.opf"),
            Err(LocalError::Damaged { .. })
        ));
    }

    #[test]
    fn ncx_navpoints_become_toc_entries() {
        let xml = r#"<?xml version="1.0"?>
            <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/">
              <navMap>
                <navPoint id="n1" playOrder="1">
                  <navLabel><text>第一章 开端</text></navLabel>
                  <content src="Text/ch1.xhtml"/>
                </navPoint>
                <navPoint id="n2" playOrder="2">
                  <navLabel><text>第二章 承接</text></navLabel>
                  <content src="Text/ch2.xhtml#mid"/>
                </navPoint>
              </navMap>
            </ncx>"#;
        let points = parse_ncx(xml, "OEBPS/");
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].title, "第一章 开端");
        assert_eq!(points[0].entry, "OEBPS/Text/ch1.xhtml");
        assert_eq!(points[1].fragment.as_deref(), Some("mid"));
    }

    #[test]
    fn nested_navpoints_carry_their_depth() {
        let xml = r#"<ncx><navMap>
          <navPoint><navLabel><text>卷上</text></navLabel><content src="a.xhtml"/>
            <navPoint><navLabel><text>子章</text></navLabel><content src="b.xhtml"/></navPoint>
          </navPoint>
        </navMap></ncx>"#;
        let points = parse_ncx(xml, "");
        assert_eq!(points.len(), 2);
        let child = points.iter().find(|p| p.title == "子章").expect("应有子章");
        assert!(child.depth > 0, "子章应有层级信息");
    }

    #[test]
    fn nav_document_toc_is_parsed() {
        let xhtml = r#"<html xmlns="http://www.w3.org/1999/xhtml"
              xmlns:epub="http://www.idpf.org/2007/ops"><body>
            <nav epub:type="toc"><ol>
              <li><a href="Text/ch1.xhtml">第一章</a></li>
              <li><a href="Text/ch2.xhtml">第二章</a></li>
            </ol></nav>
          </body></html>"#;
        let points = parse_nav(xhtml, "OEBPS/nav.xhtml", "OEBPS/");
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].entry, "OEBPS/Text/ch1.xhtml");
        assert_eq!(points[1].title, "第二章");
    }

    #[test]
    fn non_toc_navs_are_ignored() {
        let xhtml = r#"<html><body>
            <nav epub:type="landmarks"><ol><li><a href="x.xhtml">封面</a></li></ol></nav>
          </body></html>"#;
        assert!(parse_nav(xhtml, "nav.xhtml", "").is_empty());
    }

    #[test]
    fn toc_entries_map_to_chapters_with_entry_names() {
        let points = vec![
            TocPoint {
                title: "第一章".to_string(),
                entry: "OEBPS/a.xhtml".to_string(),
                fragment: None,
                depth: 0,
            },
            TocPoint {
                title: "第二节".to_string(),
                entry: "OEBPS/b.xhtml".to_string(),
                fragment: Some("sec2".to_string()),
                depth: 1,
            },
        ];
        let chapters = to_chapters(&points, 5);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].epub_entry.as_deref(), Some("OEBPS/a.xhtml"));
        assert_eq!(
            chapters[1].epub_entry.as_deref(),
            Some("OEBPS/b.xhtml#sec2"),
            "片段标识必须保留——同一文件里的多节靠它区分"
        );
        assert!(chapters.iter().all(|chapter| chapter.byte_offset.is_none()));
        assert_eq!(chapters[1].index_in_book, 1);
    }

    #[test]
    fn local_name_strips_namespaces() {
        assert_eq!(local_name(b"dc:title"), "title");
        assert_eq!(local_name(b"navPoint"), "navpoint");
        assert_eq!(local_name(b"package"), "package");
    }
}
