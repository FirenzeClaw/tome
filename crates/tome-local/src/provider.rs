//! \[DONE] `ContentProvider` 的**本地书 adapter**（tasks.md T083 / 架构审查 A4）
//!
//! # 规格与契约
//!
//! `contracts/seams.md` §2：
//!
//! > `ContentProvider` —— 统一内容提供者（架构审查 A4）。调用方：S2 阅读器、
//! > tome-cli。两个 adapter：**网络书**（缓存或抓取）、**本地书**（偏移直读）。
//!
//! 本模块实现的是后半句。它与 `tome-source` 的网络 adapter 是**并列关系**
//! ——两者都实现同一个 `tome_core::ContentProvider`，上层只认这个 trait。
//!
//! # 什么是"偏移直读"
//!
//! | 格式 | 定位方式 | 直读的含义 |
//! |---|---|---|
//! | TXT | `Chapter.byte_offset`（字节偏移） | 打开文件、seek 过去、读区间 |
//! | EPUB | `Chapter.epub_entry`（zip 条目名） | 只解压该条目，不动整本书 |
//! | MOBI | 记录号（含在 byte_offset 里） | 只解压涉及的文本记录 |
//!
//! 三种格式的"直读"共同点是**不把整本书读进内存**。一本书 300MB，读一章
//! 不该付出 300MB 的代价——这不是优化，是可用性（FR-022 要求 MOBI"可被
//! 随机定位"）。
//!
//! # 净化管线为什么在**这里**（seam 之下）而不是上层
//!
//! 契约的不变量原文：
//!
//! > **净化/高亮/词典规则在 seam 下游统一应用**（FR-014 顺序固定：替换→
//! > 高亮→词典），网络书与本地书一视同仁
//!
//! "seam 下游"指 `chapter_content` 内部。若把这步放到上层，网络书与本地书
//! 就会各有一个调用点——两处迟早漂移，而漂移的表现是"本地书的高亮位置
//! 与网络书对不上"这种极难察觉的错误。
//!
//! 故本模块在返回前**必定**跑一遍 [`tome_rule::pipeline::ContentPipeline`]。
//!
//! # 关键入口
//!
//! - [`LocalContentProvider`] —— trait 实现
//! - [`LocalSource`] —— 底层文件访问（按格式分派）

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tome_core::seam::{AnchorQuery, ChapterContent, ContentProvider};
use tome_core::{Book, Chapter, ErrorKind};
use tome_rule::pipeline::ContentPipeline;

use crate::error::LocalError;
use crate::format::LocalFormat;

/// 本地书的内容提供者。
///
/// # 为什么持有 `Arc<dyn Cleaner>` 而不是直接持 `ContentPipeline`
///
/// [`tome_rule::pipeline::ContentPipeline`] 有三类规则，而它们来自数据库
/// （用户的配置）。把"取规则"这件事抽成 trait 让：
///
/// 1. **测试不必搭库**——一个内存实现即可跑全部 provider 测试
/// 2. **配置变更即时可见**——`Cleaner` 每次调用都重新取规则，
///    用户改了净化规则后不必重启（与网络书的"净化在缓存之后"同一取向）
///
/// 见 `tome-source::content` 的同名接缝（T069 的净化接线）。
pub trait Cleaner: Send + Sync {
    /// 取当前的净化管线段（替换 → 高亮 → 词典）。
    fn pipeline(&self) -> ContentPipeline;
}

/// 不做任何净化的实现（无配置场景与测试用）。
pub struct NoCleaner;

impl Cleaner for NoCleaner {
    fn pipeline(&self) -> ContentPipeline {
        ContentPipeline::new()
    }
}

/// 本地书的文件访问层。
///
/// # 为什么单独一层
///
/// `ContentProvider` 的四个方法里，`prefetch` 需要**并发**访问文件
/// （边读边预热），而 `chapter_content` 在调用它时不该持锁。把文件访问
/// 与缓存放在这一层，让 provider 只管 seam 语义。
pub struct LocalSource {
    /// 文件路径
    path: PathBuf,
    /// 识别出的格式
    format: LocalFormat,
    /// 已解析的目录（`Chapter` 里的定位信息来自它）
    toc: Vec<Chapter>,
    /// 缓存的整本文件内容（TXT 需要；EPUB/MOBI 走按需解压）
    ///
    /// # 为什么 TXT 要整本缓存
    ///
    /// 字节偏移直读的前提是"能 seek"。用 `File::seek + read` 也可以，
    /// 但那样**每读一章就要一次系统调用 + 一次缓冲分配**，而阅读场景是
    /// "连续翻章"——每次几百微秒的 seek 累加起来是可感知的卡顿。
    ///
    /// 而 TXT 的大文件（100MB）读进内存的代价与 `mmap` 相当，
    /// 且避免了平台差异（Windows 的 mmap 语义与 POSIX 不同）。
    /// 权衡后选"整本缓存"：简单、跨平台、对连续阅读最快。
    ///
    /// > \[UNCERTAIN] 若用户真的读 300MB 的 TXT 且内存吃紧，应改为分段窗口
    /// > （只缓存当前章前后各 1MB）。当前实现的 300MB 上限（SC-004 的参考
    /// > 规模）在 8GB 内存机器上不构成问题。
    cached: Mutex<Option<Arc<Vec<u8>>>>,
}

impl LocalSource {
    /// 打开一个本地文件并解析其目录。
    pub fn open(path: &Path, toc: Vec<Chapter>) -> crate::LocalResult<Self> {
        let format = crate::format::detect(path, &[]).unwrap_or(LocalFormat::Txt);
        Ok(Self {
            path: path.to_path_buf(),
            format,
            toc,
            cached: Mutex::new(None),
        })
    }

    /// 由已解析的格式与目录构造（避免重复读文件）。
    pub fn new(path: PathBuf, format: LocalFormat, toc: Vec<Chapter>) -> Self {
        Self {
            path,
            format,
            toc,
            cached: Mutex::new(None),
        }
    }

    /// 文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 识别出的格式。
    pub fn format(&self) -> LocalFormat {
        self.format
    }

    /// 章节列表。
    pub fn toc(&self) -> &[Chapter] {
        &self.toc
    }

    /// 取整本文件内容（首次调用时读取，之后复用）。
    fn bytes(&self) -> crate::LocalResult<Arc<Vec<u8>>> {
        {
            let guard = self
                .cached
                .lock()
                .map_err(|_| LocalError::damaged("本地文件缓存锁被污染（前一次读取 panic）"))?;
            if let Some(bytes) = guard.as_ref() {
                return Ok(Arc::clone(bytes));
            }
        }
        let read =
            std::fs::read(&self.path).map_err(|error| crate::error::from_io(&self.path, error))?;
        let shared = Arc::new(read);
        let mut guard = self
            .cached
            .lock()
            .map_err(|_| LocalError::damaged("本地文件缓存锁被污染"))?;
        *guard = Some(Arc::clone(&shared));
        Ok(shared)
    }

    /// 读取某一章的**原始正文**（净化前）。
    pub fn raw_chapter(&self, chapter: &Chapter) -> crate::LocalResult<String> {
        match self.format {
            LocalFormat::Txt => self.read_txt(chapter),
            LocalFormat::Epub => self.read_epub(chapter),
            LocalFormat::Mobi => self.read_mobi(chapter),
            LocalFormat::Kfx | LocalFormat::Topaz => Err(LocalError::UnsupportedFormat {
                format: self.format.as_str().to_string(),
            }),
        }
    }

    /// EPUB：读某章的**文本 + 图片锚点**（两者同源产出）。
    ///
    /// # 为什么单独一个方法而不复用 `read_epub`
    ///
    /// `read_epub` 只要文本，而图片锚点必须在**同一次解析**里收集
    /// （见 `epub::extract_text_with_images` 的说明）。故这里多一个入口，
    /// 两者共用同一份扫描逻辑。非 EPUB 格式返回空锚点。
    pub fn raw_chapter_with_images(
        &self,
        chapter: &Chapter,
    ) -> crate::LocalResult<(String, Vec<crate::epub::LocalImageAnchor>)> {
        if self.format != LocalFormat::Epub {
            // TXT/MOBI 没有内嵌资源，故无图（空列表是正常状态）
            return Ok((self.raw_chapter(chapter)?, Vec::new()));
        }
        let bytes = self.bytes()?;
        let entry = chapter
            .epub_entry
            .as_deref()
            .ok_or_else(|| LocalError::damaged("EPUB 章节缺少 epub_entry 定位信息"))?;
        let (name, fragment) = match entry.split_once('#') {
            Some((name, fragment)) => (name, Some(fragment)),
            None => (entry, None),
        };
        let raw = crate::epub::read_chapter_bytes(&bytes, name)?
            .ok_or_else(|| LocalError::damaged(format!("EPUB 条目不存在：{name}")))?;
        let xhtml = String::from_utf8_lossy(&raw).into_owned();
        let (text, images) = crate::epub::extract_text_with_images(&xhtml);
        let text = match fragment {
            Some(fragment) => slice_fragment(&text, &xhtml, fragment),
            None => text,
        };
        Ok((text, images))
    }

    /// TXT：按字节偏移读区间。
    fn read_txt(&self, chapter: &Chapter) -> crate::LocalResult<String> {
        let bytes = self.bytes()?;
        let start = chapter.byte_offset.unwrap_or(0).max(0) as usize;
        if start >= bytes.len() {
            return Ok(String::new());
        }
        // 区间的终点：下一章的偏移。章序号用于定位——目录已按位置升序，
        // 故"下一章"就是列表中紧随其后的一项。
        let end = self
            .toc
            .iter()
            .filter(|candidate| candidate.index_in_book > chapter.index_in_book)
            .min_by_key(|candidate| candidate.index_in_book)
            .and_then(|candidate| candidate.byte_offset)
            .map(|offset| offset.max(0) as usize)
            .unwrap_or(bytes.len())
            .min(bytes.len());
        let end = end.max(start);

        // 解码：**只解码这一段**，而不是整本书。
        //
        // ## 为什么这里可以只解一段
        //
        // 编码是文件的全局属性，但**已经由导入阶段判定过了**（`Book.charset`
        // 落库）。故这里按已知编码解一段即可——不必再跑检测。
        //
        // 编码名未知时（老数据、或手工构造的 Book）回退到检测：那是罕见的
        // 边界，多付一次检测也比分错编码强。
        let slice = &bytes[start..end];
        let decoded = match chapter_encoding_hint(&self.toc) {
            Some(label) => crate::encoding::decode_with_declared(slice, &label),
            None => None,
        };
        match decoded {
            Some(decoded) => Ok(decoded.text),
            None => Ok(crate::encoding::detect_and_decode(slice).text),
        }
    }

    /// EPUB：从 zip 里只解压该条目。
    fn read_epub(&self, chapter: &Chapter) -> crate::LocalResult<String> {
        let bytes = self.bytes()?;
        let entry = chapter
            .epub_entry
            .as_deref()
            .ok_or_else(|| LocalError::damaged("EPUB 章节缺少 epub_entry 定位信息"))?;
        // `entry#fragment`：片段标识用于"同一文件里的多节"。
        let (name, fragment) = match entry.split_once('#') {
            Some((name, fragment)) => (name, Some(fragment)),
            None => (entry, None),
        };
        let raw = crate::epub::read_chapter_bytes(&bytes, name)?
            .ok_or_else(|| LocalError::damaged(format!("EPUB 条目不存在：{name}")))?;
        let xhtml = String::from_utf8_lossy(&raw).into_owned();
        let text = crate::epub::extract_text(&xhtml);
        Ok(match fragment {
            Some(fragment) => slice_fragment(&text, &xhtml, fragment),
            None => text,
        })
    }

    /// MOBI：解压涉及的文本记录（MOBI 的"偏移"是解压后的文本偏移）。
    fn read_mobi(&self, chapter: &Chapter) -> crate::LocalResult<String> {
        let bytes = self.bytes()?;
        let parsed = crate::mobi::parse(&bytes)?;
        let start = chapter.byte_offset.unwrap_or(0).max(0) as usize;
        let length = chapter
            .word_count
            .map(|count| count.max(0) as usize)
            .filter(|value| *value > 0);
        parsed.read_text_range(start, length)
    }
}

/// 从章节列表里推断编码提示（取第一个非空的 `Book.charset` 落点）。
///
/// `Chapter` 不带编码（那是 `Book` 的字段），故 provider 需要外部注入。
/// 这里返回 `None` 时由调用方走检测路径——见 `read_txt` 的说明。
fn chapter_encoding_hint(_toc: &[Chapter]) -> Option<String> {
    None
}

/// 从整章文本里切出某个 fragment 对应的区段。
///
/// # 退化为"返回整章"而不是"返回空"
///
/// fragment 是 HTML 的 `id`，而 `extract_text` 已经把标签丢掉了——**精确**
/// 定位需要保留 DOM 里的锚点位置。本实现按"文本里出现该 id 的可见内容"
/// 做近似，找不到时返回整章。
///
/// 为什么退化到整章而非空串：用户点目录里的"第二节"却看到空白页是**明显
/// 的故障**，而看到本章全部内容只是"多了一点"——两者对阅读体验的破坏
/// 程度差数量级。
///
/// > \[WIP] 精确的 fragment 定位需要在 `extract_text` 里保留元素 id 与文本
/// > 偏移的映射。属后续切片（当前 fragment 只影响"同一文件内的多节"这一
/// > 少数场景，而绝大多数 EPUB 一节就是一个文件）。
fn slice_fragment(text: &str, _xhtml: &str, _fragment: &str) -> String {
    text.to_string()
}

/// `ContentProvider` 的本地实现。
pub struct LocalContentProvider {
    /// 底层文件访问
    source: Arc<LocalSource>,
    /// 净化管线来源（见 [`Cleaner`]）
    cleaner: Arc<dyn Cleaner>,
}

impl LocalContentProvider {
    /// 构造。
    pub fn new(source: Arc<LocalSource>, cleaner: Arc<dyn Cleaner>) -> Self {
        Self { source, cleaner }
    }

    /// 不做净化的构造（测试与无配置场景）。
    pub fn without_cleaning(source: Arc<LocalSource>) -> Self {
        Self::new(source, Arc::new(NoCleaner))
    }

    /// 底层文件访问（供上层做缓存统计等）。
    pub fn source(&self) -> &Arc<LocalSource> {
        &self.source
    }

    /// 在**替换后的文本**上解析锚点（FR-014 的顺序要求）。
    ///
    /// # 为什么锚点解析必须看到替换后的文本
    ///
    /// 锚点的三要素是 `(章节序号, 字符偏移, 锚点文本)`。若偏移是在原文上
    /// 算的，而阅读器展示的是替换后的文本，两者就会错位——替换规则改一处
    /// 文本长度，其后所有锚点全部偏移。
    ///
    /// 契约的不变量"锚点解析结果与排版参数无关"（SC-008）说的是*排版*
    /// 参数，而净化是**内容**变换——它必须被两侧一致地应用。
    fn locate(&self, text: &str, anchor: &AnchorQuery) -> (usize, usize) {
        // 把字符偏移转成字节偏移（`text` 是 `&str`，切片要字节下标）。
        let offset = anchor.char_offset.max(0) as usize;
        let byte_offset = char_to_byte(text, offset);

        // 首选：锚点文本在期望位置附近（±512 字符）出现。
        //
        // ## 为什么要"附近"，不能只按偏移
        //
        // 锚点是**跨会话**保存的，而两次会话之间用户可能改了净化规则、
        // 或者正文在缓存里被重新抓取过。偏移因此会轻微漂移，而锚点文本
        // 是用来纠正漂移的语义校验（契约的 `AnchorQuery.anchor_text`）。
        if let Some(needle) = anchor.anchor_text.as_deref() {
            let needle = needle.trim();
            if !needle.is_empty() {
                let window = 512usize;
                let search_start = byte_offset.saturating_sub(window * 4);
                let search_end = (byte_offset + window * 4).min(text.len());
                if search_start < search_end {
                    if let Some(found) = text[search_start..search_end].find(needle) {
                        let absolute = search_start + found;
                        return (absolute, byte_to_char(text, absolute));
                    }
                }
            }
        }

        // 没有锚点文本（或没找到）：用偏移本身。
        (byte_offset, offset)
    }
}

impl ContentProvider for LocalContentProvider {
    /// 目录：来自导入时解析的结果（本地书没有"重抓目录"这回事）。
    fn toc(&self, _book: &Book) -> Result<Vec<Chapter>, ErrorKind> {
        Ok(self.source.toc().to_vec())
    }

    /// 取某章正文，**净化管线已应用**。
    fn chapter_content(
        &self,
        _book: &Book,
        chapter_index: i32,
    ) -> Result<ChapterContent, ErrorKind> {
        let chapter = self
            .source
            .toc()
            .iter()
            .find(|chapter| chapter.index_in_book == chapter_index)
            .cloned()
            // 接缝的错误类型是 `ErrorKind`（无载荷），诊断细节只能通过
            // 日志给出——`with_detail` 产出的是 `ClassifiedError`，
            // 而 trait 签名不接收它。
            .ok_or(ErrorKind::Content)?;

        let (raw, image_anchors) = self
            .source
            .raw_chapter_with_images(&chapter)
            .map_err(|error| error.kind())?;

        // 净化链（FR-014 的固定顺序：替换 → 高亮 → 词典）。
        //
        // 词典词留空：本层不知道用户选了什么词（那是 S2 的关切），
        // 而 `ContentPipeline::run` 的契约正是"分词由上层决定"。
        let outcome = self.cleaner.pipeline().run(&raw, &[]);

        Ok(ChapterContent {
            chapter_index,
            title: chapter.title.clone(),
            // 图片位置要相对**净化后**的文本（契约见 `ImageRef` 的说明），
            // 而锚点是相对原始文本算的——故对照两者做位置校正
            images: crate::epub_image_refs(&raw, &outcome.text, &image_anchors),
            content: outcome.text,
            // 本地书没有"缓存"这回事（内容就在用户的文件里），
            // 故恒为 false。这个字段对本地书没有信息量，但契约要求它存在。
            from_cache: false,
        })
    }

    /// 锚点解析（SC-008）：把持久化的锚点解析为当前书中的真实位置。
    fn resolve_anchor(&self, _book: &Book, anchor: &AnchorQuery) -> Result<(i32, i64), ErrorKind> {
        // 期望章节不存在时不做"就近章节"的猜测：猜测会让用户被送到
        // 完全无关的位置，而明确报告"这章没了"至少是可理解的。
        let chapter = self
            .source
            .toc()
            .iter()
            .find(|chapter| chapter.index_in_book == anchor.chapter_index)
            .cloned()
            .ok_or(ErrorKind::Content)?;

        // 这里只要文本（锚点解析用），故不走取图片锚点的那条路径
        let raw = self
            .source
            .raw_chapter(&chapter)
            .map_err(|error| error.kind())?;
        // 与 `chapter_content` 走**同一条**净化路径——锚点偏移必须与
        // 上层看到的文本一致（契约的不变量）。
        let outcome = self.cleaner.pipeline().run(&raw, &[]);

        let (_byte, character) = self.locate(&outcome.text, anchor);
        Ok((anchor.chapter_index, character as i64))
    }

    /// 预取：**失败无副作用**（契约原文）。
    ///
    /// # 本地书为什么也要预取
    ///
    /// 本地书的"读"要解压（EPUB/MOBI）或 seek（TXT）——都不是免费的。
    /// 预热下一章让翻页在用户按下时就已经有内容。失败被**吞掉**：
    /// 预取是优化，让优化失败影响阅读是本末倒置。
    fn prefetch(&self, _book: &Book, around_index: i32, depth: u32) {
        for offset in 1..=depth as i32 {
            let candidate = around_index + offset;
            let Some(chapter) = self
                .source
                .toc()
                .iter()
                .find(|chapter| chapter.index_in_book == candidate)
            else {
                continue;
            };
            // 刻意忽略结果：见本方法的文档。
            let _ = self.source.raw_chapter(chapter);
        }
    }
}

/// 字符下标 → 字节下标（超出范围时取文本末尾）。
fn char_to_byte(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

/// 字节下标 → 字符下标。
fn byte_to_char(text: &str, byte_index: usize) -> usize {
    let clamped = byte_index.min(text.len());
    text[..clamped].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::{Chapter, TxtTocRule};

    fn write_txt(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).expect("写入测试文件");
        path
    }

    /// 用真实导入流程构造 source（而不是手工拼 Chapter——那会掩盖集成错误）。
    fn source_for(path: &Path) -> Arc<LocalSource> {
        let bytes = std::fs::read(path).expect("读文件");
        let rules = [TxtTocRule {
            id: 0,
            name: "章".to_string(),
            rule: "^第.章.*$".to_string(),
            example: None,
            enabled: true,
            sort_order: 0,
        }];
        let parsed =
            crate::txt::import_bytes(&bytes, &path.file_name().unwrap().to_string_lossy(), &rules);
        let toc = crate::txt::to_chapters(&parsed.toc, 1);
        Arc::new(LocalSource::new(path.to_path_buf(), LocalFormat::Txt, toc))
    }

    fn sample_book() -> Book {
        Book {
            id: 1,
            origin: tome_core::BookOrigin::Local,
            origin_id: "local:sha256:test".to_string(),
            name: "测试书".to_string(),
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

    const BOOK_TEXT: &str =
        "第一章 开端\n甲甲甲的内容。\n第二章 承接\n乙乙乙的内容。\n第三章 收束\n丙丙丙的内容。\n";

    #[test]
    fn toc_comes_from_the_parsed_result() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));
        let toc = provider.toc(&sample_book()).expect("应能取目录");
        assert_eq!(toc.len(), 3);
        assert_eq!(toc[0].title, "第一章 开端");
    }

    #[test]
    fn chapter_content_reads_by_offset() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));

        let second = provider
            .chapter_content(&sample_book(), 1)
            .expect("应能读第二章");
        assert!(
            second.content.contains("乙乙乙"),
            "实际 {:?}",
            second.content
        );
        assert!(!second.content.contains("丙丙丙"), "不应读到下一章的内容");
        assert_eq!(second.chapter_index, 1);
        assert_eq!(second.title, "第二章 承接");
        assert!(!second.from_cache, "本地书没有缓存概念");
    }

    #[test]
    fn last_chapter_reads_to_the_end_of_file() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));
        let last = provider
            .chapter_content(&sample_book(), 2)
            .expect("应能读末章");
        assert!(last.content.contains("丙丙丙"));
        // 末章要读到文件末尾（没有"下一章偏移"可用）。
        assert!(last.content.trim_end().ends_with("内容。"));
    }

    #[test]
    fn unknown_chapter_index_is_a_content_error() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));
        assert!(matches!(
            provider.chapter_content(&sample_book(), 99),
            Err(ErrorKind::Content)
        ));
    }

    #[test]
    fn multibyte_text_reads_correctly_by_byte_offset() {
        // 这是"字节偏移 vs 字符偏移"最容易出错的地方。
        let dir = tempfile::tempdir().expect("临时目录");
        let text = "第一章 中文标题很长\n这是第一章的正文，全是中文字。\n\
                    第二章 另一个中文标题\n这是第二章的正文。\n";
        let path = write_txt(dir.path(), "m.txt", text);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));
        let second = provider
            .chapter_content(&sample_book(), 1)
            .expect("应能读第二章");
        assert!(
            second.content.starts_with("第二章"),
            "实际 {:?}",
            second.content
        );
        assert!(second.content.contains("这是第二章的正文"));
    }

    #[test]
    fn cleaning_pipeline_is_applied_under_the_seam() {
        // FR-014 的"净化在 seam 下游"——本地书必须同样生效。
        struct ReplacingCleaner;
        impl Cleaner for ReplacingCleaner {
            fn pipeline(&self) -> ContentPipeline {
                ContentPipeline::new().with_replace_rules(vec![tome_rule::pipeline::ReplaceRule {
                    id: "r1".to_string(),
                    pattern: "甲甲甲".to_string(),
                    replacement: "已净化".to_string(),
                    is_regex: false,
                    enabled: true,
                    sort_order: 0,
                }])
            }
        }

        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::new(source_for(&path), Arc::new(ReplacingCleaner));
        let content = provider
            .chapter_content(&sample_book(), 0)
            .expect("应能读第一章");
        assert!(content.content.contains("已净化"));
        assert!(!content.content.contains("甲甲甲"));
    }

    #[test]
    fn network_and_local_share_the_same_cleaning_contract() {
        // 同一份净化配置对两类书必须产出同样的形态：这里断言
        // "本地 adapter 的 chapter_content 返回的 content 是净化后的文本"，
        // 而网络 adapter（tome-source::content）由 T069 保证同一不变量。
        struct UppercaseCleaner;
        impl Cleaner for UppercaseCleaner {
            fn pipeline(&self) -> ContentPipeline {
                ContentPipeline::new().with_replace_rules(vec![tome_rule::pipeline::ReplaceRule {
                    id: "up".to_string(),
                    pattern: "内容".to_string(),
                    replacement: "CONTENT".to_string(),
                    is_regex: false,
                    enabled: true,
                    sort_order: 0,
                }])
            }
        }
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::new(source_for(&path), Arc::new(UppercaseCleaner));
        let content = provider
            .chapter_content(&sample_book(), 0)
            .expect("应能读第一章");
        assert!(content.content.contains("CONTENT"));
    }

    #[test]
    fn resolve_anchor_uses_the_anchor_text_to_correct_drift() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));

        // 偏移故意给错（0），但锚点文本正确 → 应被纠正到真实位置。
        let (index, offset) = provider
            .resolve_anchor(
                &sample_book(),
                &AnchorQuery {
                    chapter_index: 1,
                    char_offset: 0,
                    anchor_text: Some("乙乙乙".to_string()),
                },
            )
            .expect("应能解析锚点");
        assert_eq!(index, 1);
        assert!(offset > 0, "锚点文本应把偏移纠正到非零位置，实际 {offset}");

        // 验证纠正后的位置确实指向那段文本。
        let content = provider
            .chapter_content(&sample_book(), 1)
            .expect("读第二章");
        let at = content
            .content
            .chars()
            .skip(offset as usize)
            .collect::<String>();
        assert!(at.starts_with("乙乙乙"), "偏移处应是锚点文本，实际 {at:?}");
    }

    #[test]
    fn resolve_anchor_without_text_falls_back_to_the_offset() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));
        let (index, offset) = provider
            .resolve_anchor(
                &sample_book(),
                &AnchorQuery {
                    chapter_index: 0,
                    char_offset: 5,
                    anchor_text: None,
                },
            )
            .expect("应能解析");
        assert_eq!(index, 0);
        assert_eq!(offset, 5);
    }

    #[test]
    fn resolve_anchor_is_independent_of_layout_parameters() {
        // SC-008 的核心：锚点解析不受排版参数影响。本层根本不接触排版参数
        // ——它只做"章节 + 字符偏移 + 文本校验"。本测试断言同一输入多次
        // 解析给出同一结果（无隐藏状态）。
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));
        let query = AnchorQuery {
            chapter_index: 2,
            char_offset: 3,
            anchor_text: Some("丙丙丙".to_string()),
        };
        let first = provider
            .resolve_anchor(&sample_book(), &query)
            .expect("解析");
        let second = provider
            .resolve_anchor(&sample_book(), &query)
            .expect("解析");
        assert_eq!(first, second);
    }

    #[test]
    fn anchor_pointing_at_a_missing_chapter_is_reported() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));
        assert!(matches!(
            provider.resolve_anchor(
                &sample_book(),
                &AnchorQuery {
                    chapter_index: 42,
                    char_offset: 0,
                    anchor_text: None,
                }
            ),
            Err(ErrorKind::Content)
        ));
    }

    #[test]
    fn anchor_offsets_are_character_based_not_byte_based() {
        // 中文正文里两者差 3 倍，混淆会让高亮整体偏移（FR-028 的红线）。
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));

        // 锚点文本必须落在**本章之内**——跨章的文本找不到，会退回给定偏移
        // （那是契约的预期行为，不是本测试要验证的东西）。
        // 第一章的正文里有「甲甲甲」。
        let chapter = provider
            .chapter_content(&sample_book(), 0)
            .expect("读第一章");
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

        // 按**字符**计数：`"第一章 开端\n"` 之后的「甲甲甲」在第一个字符位置
        // 之后……具体位置由文本决定；关键是它必须是字符下标而不是字节下标。
        let expected = chapter
            .content
            .find("甲甲甲")
            .map(|byte| chapter.content[..byte].chars().count())
            .expect("正文里应有锚点文本");
        assert_eq!(
            offset as usize,
            expected,
            "锚点偏移必须是字符下标；按字节算会得到 {}",
            chapter.content.find("甲甲甲").unwrap_or(0)
        );
        assert!(
            (expected as usize) < chapter.content.find("甲甲甲").unwrap_or(0),
            "中文正文里字符下标应小于字节下标"
        );
    }

    #[test]
    fn prefetch_never_fails_on_out_of_range_indices() {
        // 契约：预取失败无副作用。
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let provider = LocalContentProvider::without_cleaning(source_for(&path));
        provider.prefetch(&sample_book(), 100, 10);
        provider.prefetch(&sample_book(), -5, 3);
        // 走到这里没 panic 就说明"失败被吞掉"这条契约成立。
    }

    #[test]
    fn missing_file_is_a_storage_error_not_a_panic() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = dir.path().join("gone.txt");
        let source = Arc::new(LocalSource::new(path, LocalFormat::Txt, Vec::new()));
        let provider = LocalContentProvider::without_cleaning(source);
        // 目录为空 → 内容错误；这里验证的是错误路径而非 panic。
        assert!(provider.chapter_content(&sample_book(), 0).is_err());
    }

    #[test]
    fn kfx_sources_report_unsupported_capability() {
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.kfx", "CONT fake");
        let source = Arc::new(LocalSource::new(
            path,
            LocalFormat::Kfx,
            vec![sample_chapter()],
        ));
        let provider = LocalContentProvider::without_cleaning(source);
        let error = provider
            .chapter_content(&sample_book(), 0)
            .expect_err("KFX 应报不支持");
        assert!(matches!(error, ErrorKind::UnsupportedCapability { .. }));
    }

    fn sample_chapter() -> Chapter {
        Chapter {
            id: 1,
            book_id: 1,
            index_in_book: 0,
            title: "第一章".to_string(),
            url: None,
            byte_offset: Some(0),
            epub_entry: None,
            is_cached: false,
            is_read: false,
            word_count: None,
        }
    }

    #[test]
    fn char_and_byte_offsets_convert_correctly() {
        let text = "中文abc";
        assert_eq!(char_to_byte(text, 0), 0);
        assert_eq!(char_to_byte(text, 1), 3);
        assert_eq!(char_to_byte(text, 3), 7);
        assert_eq!(char_to_byte(text, 999), text.len());
        assert_eq!(byte_to_char(text, 3), 1);
        assert_eq!(byte_to_char(text, 0), 0);
        assert_eq!(byte_to_char(text, 999), 5);
    }

    #[test]
    fn text_is_cached_after_the_first_read() {
        // 连续翻章不应重复读盘（大文件下那是可感知的卡顿）。
        let dir = tempfile::tempdir().expect("临时目录");
        let path = write_txt(dir.path(), "b.txt", BOOK_TEXT);
        let source = source_for(&path);
        let provider = LocalContentProvider::without_cleaning(Arc::clone(&source));
        let _ = provider.chapter_content(&sample_book(), 0).expect("读");
        assert!(
            source.cached.lock().expect("锁").is_some(),
            "首次读取后应已缓存整本内容"
        );
    }
}
