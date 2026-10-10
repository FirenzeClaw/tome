//! \[DONE] 完整管线的 CLI 命令（tasks.md T065-T072 的接线）
//!
//! # 本模块覆盖 `contracts/cli.md` 的哪些命令
//!
//! | 命令 | 动作 | 任务 |
//! |---|---|---|
//! | `info <source-id> <book-url>` | 书籍详情 | T065 |
//! | `toc <source-id> <book-url>` | 目录 | T066 |
//! | `content <source-id> <chapter-url>` | 正文 | T067 |
//! | `source list [--group <名>] [--enabled]` | 书源列表 | T056 |
//! | `source check <id\|--all>` | 可用性检查 | T071 |
//!
//! # 为什么这些命令都要求 `<source-id>`
//!
//! 它们作用于"某本书的某个地址"，而地址本身**不含"它属于哪个源"**的信息
//! ——详情页/章节 URL 只说明它在哪，不说明用哪套规则去解析它。
//! `data-model.md` 的网络书身份 `书源id|详情页地址` 正是这个道理的体现。
//!
//! # 输出格式约定
//!
//! 字段顺序由 `serde_json::json!` 的书写顺序固定（`serde_json` 的
//! `Map` 默认保序），故同一份输入两次运行的输出逐字节一致——这是
//! T029「录制→重放输出逐字节一致」对**所有**命令的要求，不只 `search`。

use tome_core::{ErrorKind, NetHost};

use crate::commands::{open_source_store, CommandOutcome, HostCapabilities};
use crate::flags::{exit_code_for, GlobalOptions, OutputFormat};

/// 从库载入**一个**书源。
pub fn load_one_source(
    options: &GlobalOptions,
    source_id: &str,
) -> Result<tome_source::source::ParsedSource, CommandOutcome> {
    let source_store = open_source_store(options)?;
    match source_store.get(source_id) {
        Ok(Some(source)) => Ok(tome_source::source::ParsedSource { source }),
        Ok(None) => Err(CommandOutcome::failure(
            &ErrorKind::Rule,
            &format!(
                "error.rule.source.missing id={source_id}\
                 （库里没有这个 id；先跑 `tome source import <file>`）"
            ),
        )),
        Err(error) => Err(CommandOutcome::failure(
            &error.to_error_kind(),
            &format!("{error}"),
        )),
    }
}

/// 为单源动作准备 JS 引擎（含 JS 的源需要它）。
///
/// # 为什么 `None` 也是正常结果
///
/// 大多数书源（纯 CSS/正则）用不到沙箱，给它们建 runtime 是浪费
/// （QuickJS 初始化有实打实的开销）。故只有
/// [`tome_source::js::source_needs_javascript`] 为真的源才建。
pub fn engine_for(
    source: &tome_core::Source,
    hosts: &HostCapabilities,
) -> Result<Option<tome_source::js::SourceJsEngine>, CommandOutcome> {
    if !tome_source::js::source_needs_javascript(source) {
        return Ok(None);
    }
    let store = hosts.store_host_for(&source.id);
    crate::js_host::build_source_engine(source, hosts.net.clone(), store)
        .map(Some)
        .map_err(|error| CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")))
}

/// `info <source-id> <book-url>`：书籍详情（FR-002 第三类动作）。
pub fn run_info(
    options: &GlobalOptions,
    source_id: &str,
    book_url: &str,
    host: &dyn NetHost,
    hosts: HostCapabilities,
) -> CommandOutcome {
    let source = match load_one_source(options, source_id) {
        Ok(source) => source,
        Err(outcome) => return outcome,
    };
    let socket = match engine_for(&source.source, &hosts) {
        Ok(socket) => socket,
        Err(outcome) => return outcome,
    };

    match tome_source::info::book_info_with_js(&source, book_url, host, socket.as_ref()) {
        Ok(book) => CommandOutcome::with_stdout(render_book(options.format, &book)),
        Err(error) => CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    }
}

/// `toc <source-id> <book-url>`：目录（FR-002 第四类动作）。
pub fn run_toc(
    options: &GlobalOptions,
    source_id: &str,
    book_url: &str,
    host: &dyn NetHost,
    hosts: HostCapabilities,
) -> CommandOutcome {
    let source = match load_one_source(options, source_id) {
        Ok(source) => source,
        Err(outcome) => return outcome,
    };
    let socket = match engine_for(&source.source, &hosts) {
        Ok(socket) => socket,
        Err(outcome) => return outcome,
    };

    // 目录动作需要一个 `Book`——用详情页地址合成最小的一本。
    //
    // ## 为什么不先跑详情
    //
    // 那会多发一次请求，而目录规则只需要 `toc_url`（此处即详情页地址：
    // 大量书源的详情页就是目录页）。真要取"书的元信息"由 `info` 负责。
    let book = minimal_book(&source.source.id, book_url);
    match tome_source::toc::toc_with_js(&source, &book, host, socket.as_ref()) {
        Ok(chapters) => CommandOutcome::with_stdout(render_chapters(options.format, &chapters)),
        Err(error) => CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    }
}

/// `content <source-id> <chapter-url> [--no-purify]`：正文（第五类动作）。
pub fn run_content(
    options: &GlobalOptions,
    source_id: &str,
    chapter_url: &str,
    no_purify: bool,
    host: &dyn NetHost,
    hosts: HostCapabilities,
) -> CommandOutcome {
    let source = match load_one_source(options, source_id) {
        Ok(source) => source,
        Err(outcome) => return outcome,
    };
    let socket = match engine_for(&source.source, &hosts) {
        Ok(socket) => socket,
        Err(outcome) => return outcome,
    };

    let book = minimal_book(&source.source.id, chapter_url);
    let chapter = tome_core::Chapter {
        id: 0,
        book_id: 0,
        index_in_book: 0,
        title: String::new(),
        url: Some(chapter_url.to_string()),
        byte_offset: None,
        epub_entry: None,
        is_cached: false,
        is_read: false,
        word_count: None,
    };

    // ## 为什么 CLI 的 content 不走缓存
    //
    // 缓存要 `chapter.id`（未入库的章节没有缓存身份），而 CLI 的 `content`
    // 只给一个 URL、不涉及书籍入库。硬要缓存就得先建一本书，那超出了
    // "看这一章正文"的语义。缓存路径由 `ContentProvider`（T074）覆盖
    // ——那是阅读器的入口。
    let outcome = match tome_source::content::chapter_content_with_js(
        &source,
        &book,
        &chapter,
        host,
        None,
        socket.as_ref(),
    ) {
        Ok(outcome) => outcome,
        Err(error) => return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    };

    // 净化：默认应用（契约：正文默认已应用净化规则）
    let text = if no_purify {
        outcome.raw
    } else {
        match apply_purify(options, &outcome.raw) {
            Ok(text) => text,
            Err(outcome) => return outcome,
        }
    };
    CommandOutcome::with_stdout(render_content(options.format, chapter_url, &text))
}

/// 应用库里配置的净化规则。
///
/// # 规则读不出来时为什么用空净化器而不是报错
///
/// 用户要的是"能读这一章"。规则表损坏（罕见的磁盘问题）不该让他读不了书
/// ——那把一个存储问题放大成了可用性问题。故回落并**在 stderr 提示**
/// （不静默，否则"我的净化规则怎么没生效"无从查起）。
pub fn apply_purify(options: &GlobalOptions, raw: &str) -> Result<String, CommandOutcome> {
    let source_store = open_source_store(options)?;
    let dao = tome_store::dao::MiscDao::new(source_store.dao().db().clone());

    let replace = dao.replace_rules();
    let highlight = dao.highlight_rules();
    let dict = dao.dict_rules();

    let purifier = match (replace, highlight, dict) {
        (Ok(replace), Ok(highlight), Ok(dict)) => {
            tome_source::purify::Purifier::new(replace, highlight, dict)
        }
        (replace, highlight, dict) => {
            let reason = replace
                .err()
                .or_else(|| highlight.err())
                .or_else(|| dict.err());
            eprintln!(
                "warning: 读取净化规则失败（{}），本次不净化",
                reason
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "未知".to_string())
            );
            tome_source::purify::Purifier::empty()
        }
    };

    purifier
        .purify_text_only(raw, tome_source::purify::PurifyTarget::Content)
        .map_err(|error| CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")))
}

/// `source list [--group <名>] [--enabled]`
pub fn run_source_list(
    options: &GlobalOptions,
    group: Option<&str>,
    enabled_only: bool,
) -> CommandOutcome {
    let source_store = match open_source_store(options) {
        Ok(store) => store,
        Err(outcome) => return outcome,
    };

    // 不给 `--group` 时列**全部**（而不是"未分组"）。
    //
    // ## 为什么这条分支要显式写
    //
    // `by_group(None)` 的语义是"未分组的源"，而 `source list` 的语义是
    // "列出书源"——用户不给分组时想看的是全部。两者混用会让默认输出只
    // 显示未分组的源，而"我明明有 100 个源却只列了 3 条"是个很难联想到
    // 过滤语义的现象。
    let mut sources = if let Some(name) = group {
        let groups = match source_store.groups() {
            Ok(groups) => groups,
            Err(error) => {
                return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"))
            }
        };
        let Some(group) = groups.iter().find(|group| group.name == name) else {
            return CommandOutcome::failure(
                &ErrorKind::Configuration,
                &format!("error.configuration.group.missing name={name}"),
            );
        };
        match source_store.dao().by_group(Some(group.id)) {
            Ok(sources) => sources,
            Err(error) => {
                return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"))
            }
        }
    } else {
        match source_store.all() {
            Ok(sources) => sources,
            Err(error) => {
                return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"))
            }
        }
    };

    if enabled_only {
        sources.retain(|source| source.enabled);
    }

    CommandOutcome::with_stdout(render_source_list(options.format, &sources))
}

/// `source check <id|--all>`：可用性检查（FR-011）。
pub fn run_source_check(
    options: &GlobalOptions,
    source_id: Option<&str>,
    all: bool,
    host: &dyn NetHost,
) -> CommandOutcome {
    let source_store = match open_source_store(options) {
        Ok(store) => store,
        Err(outcome) => return outcome,
    };

    let targets: Vec<tome_source::source::ParsedSource> = if all {
        match source_store.all() {
            Ok(sources) => sources
                .into_iter()
                .filter(|source| source.enabled)
                .map(|source| tome_source::source::ParsedSource { source })
                .collect(),
            Err(error) => {
                return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"))
            }
        }
    } else {
        let Some(id) = source_id else {
            return CommandOutcome::failure(
                &ErrorKind::Configuration,
                "error.configuration.usage 需要 <id> 或 --all",
            );
        };
        match load_one_source(options, id) {
            Ok(source) => vec![source],
            Err(outcome) => return outcome,
        }
    };

    if targets.is_empty() {
        return CommandOutcome::with_stdout(render_check_report(options.format, &[], &[], 1.0));
    }

    // 批量检查走**流式**接口（A6/T071）——CLI 一次性收集是它的一个使用方式
    let filter = tome_core::seam::SourceCheckFilter::default();
    let stream = tome_source::check::check_sources(&targets, &filter, host);
    let (available, unavailable) = tome_source::check::collect_check(stream);

    let summary = tome_source::check::summarize(available.len(), unavailable.len());
    // 全部不可用时给业务错误退出码（脚本据此判断"这批源还能不能用"）
    let exit_code = if available.is_empty() && !unavailable.is_empty() {
        exit_code_for(&unavailable[0].1)
    } else {
        0
    };

    let stdout = render_check_report(
        options.format,
        &available,
        &unavailable,
        summary.available_rate(),
    );
    CommandOutcome {
        exit_code,
        stdout,
        stderr: String::new(),
    }
}

/// 从 `tome-store` 的书源 DAO 查源（`SourceLookup` 的生产实现）。
///
/// # 为什么放在组合根侧
///
/// `SourceLookup` 定义在 `tome-source`，而它的这个实现要同时看见
/// `tome-source` 的 trait 与 `tome-store` 的 DAO——只有组合根能同时
/// 依赖两者（实现 crate 之间只通过 `tome-core` 相接）。
pub struct DaoSourceLookup {
    dao: tome_store::dao::SourceDao,
    /// 缓存的源（一次读取，避免同一本书的每次动作都查库）
    ///
    /// # 为什么要缓存
    ///
    /// `ContentProvider` 的每个方法（toc / chapter_content / resolve_anchor
    /// / prefetch）都要拿到源定义，而 prefetch 会对每一章调一次。
    /// 不缓存的话，"预取 3 章"就要查 3 次库。
    ///
    /// 缓存**不失效**：CLI 是短命进程（一次命令一次装配），而
    /// 长命场景（S3 界面）应当在源变更时重建 provider——那是组合根的
    /// 职责，不是这个查找器的。
    cache: std::sync::Mutex<std::collections::HashMap<String, tome_source::source::ParsedSource>>,
}

impl DaoSourceLookup {
    /// 构造。
    pub fn new(dao: tome_store::dao::SourceDao) -> Self {
        Self {
            dao,
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl tome_source::source::SourceLookup for DaoSourceLookup {
    fn find(&self, source_id: &str) -> Option<tome_source::source::ParsedSource> {
        {
            let cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(parsed) = cache.get(source_id) {
                return Some(parsed.clone());
            }
        }
        let source = self.dao.by_id(source_id).ok().flatten()?;
        let parsed = tome_source::source::ParsedSource { source };
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(source_id.to_string(), parsed.clone());
        Some(parsed)
    }
}

/// 合成一本最小的网络书（只有身份与目录地址）。
pub fn minimal_book(source_id: &str, url: &str) -> tome_core::Book {
    tome_core::Book {
        id: 0,
        origin: tome_core::BookOrigin::Network,
        origin_id: format!("{source_id}|{url}"),
        name: String::new(),
        author: None,
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: Some(url.to_string()),
        latest_chapter_title: None,
        latest_chapter_time: None,
        library_root_id: None,
        relative_path: None,
        charset: None,
        format: None,
        total_chapters: None,
        dur_chapter_index: None,
        added_at: 0,
    }
}

// ---------------------------------------------------------------------------
// 输出渲染（字段顺序固定，保证同一输入的输出逐字节一致）
// ---------------------------------------------------------------------------

/// 渲染书籍详情。
pub fn render_book(format: OutputFormat, book: &tome_core::Book) -> String {
    match format {
        OutputFormat::Json => json_line(&serde_json::json!({
            "name": book.name,
            "author": book.author,
            "intro": book.intro,
            "cover_url": book.cover_url,
            "toc_url": book.toc_url,
            "kind": book.kind,
            "origin_id": book.origin_id,
            "last_chapter": book.latest_chapter_title,
        })),
        OutputFormat::Human => {
            let mut lines = vec![
                format!("书名：{}", book.name),
                format!("作者：{}", book.author.as_deref().unwrap_or("（未提供）")),
                format!("分类：{}", book.kind.as_deref().unwrap_or("（未提供）")),
                format!(
                    "目录页：{}",
                    book.toc_url.as_deref().unwrap_or("（未提供）")
                ),
                format!(
                    "封面：{}",
                    book.cover_url.as_deref().unwrap_or("（未提供）")
                ),
            ];
            if let Some(intro) = &book.intro {
                lines.push(format!("简介：{intro}"));
            }
            lines.join("\n") + "\n"
        }
    }
}

/// 渲染目录。
pub fn render_chapters(format: OutputFormat, chapters: &[tome_core::Chapter]) -> String {
    match format {
        OutputFormat::Json => {
            let items: Vec<serde_json::Value> = chapters
                .iter()
                .map(|chapter| {
                    serde_json::json!({
                        "index": chapter.index_in_book,
                        "title": chapter.title,
                        "url": chapter.url,
                    })
                })
                .collect();
            json_line(&serde_json::Value::Array(items))
        }
        OutputFormat::Human => {
            let mut lines: Vec<String> = chapters
                .iter()
                .map(|chapter| format!("{:>5}  {}", chapter.index_in_book + 1, chapter.title))
                .collect();
            lines.push(format!("（共 {} 章）", chapters.len()));
            lines.join("\n") + "\n"
        }
    }
}

/// 渲染正文。
pub fn render_content(format: OutputFormat, url: &str, text: &str) -> String {
    match format {
        OutputFormat::Json => json_line(&serde_json::json!({
            "url": url,
            "content": text,
            "length": text.chars().count(),
        })),
        OutputFormat::Human => format!("{text}\n"),
    }
}

/// 渲染书源列表。
pub fn render_source_list(format: OutputFormat, sources: &[tome_core::Source]) -> String {
    match format {
        OutputFormat::Json => {
            let items: Vec<serde_json::Value> = sources
                .iter()
                .map(|source| {
                    serde_json::json!({
                        "id": source.id,
                        "name": source.name,
                        "enabled": source.enabled,
                        "group_id": source.group_id,
                        "kind": source.kind.as_storage_code(),
                    })
                })
                .collect();
            json_line(&serde_json::Value::Array(items))
        }
        OutputFormat::Human => {
            let mut lines: Vec<String> = sources
                .iter()
                .map(|source| {
                    format!(
                        "{} {}  {}",
                        if source.enabled {
                            "[启用]"
                        } else {
                            "[停用]"
                        },
                        source.name,
                        source.id
                    )
                })
                .collect();
            lines.push(format!("（共 {} 条）", sources.len()));
            lines.join("\n") + "\n"
        }
    }
}

/// 渲染源检查报告。
pub fn render_check_report(
    format: OutputFormat,
    available: &[tome_core::Source],
    unavailable: &[(tome_core::Source, ErrorKind)],
    available_rate: f64,
) -> String {
    match format {
        OutputFormat::Json => {
            let items: Vec<serde_json::Value> = available
                .iter()
                .map(|source| {
                    serde_json::json!({
                        "source": source.id,
                        "name": source.name,
                        "verdict": "available",
                    })
                })
                .chain(unavailable.iter().map(|(source, reason)| {
                    serde_json::json!({
                        "source": source.id,
                        "name": source.name,
                        "verdict": "unavailable",
                        "reason": reason.message_id(),
                    })
                }))
                .collect();
            json_line(&serde_json::json!({
                "total": available.len() + unavailable.len(),
                "available": available.len(),
                "unavailable": unavailable.len(),
                "available_rate": available_rate,
                "results": items,
            }))
        }
        OutputFormat::Human => {
            let mut lines: Vec<String> = available
                .iter()
                .map(|source| format!("  OK   {}", source.name))
                .collect();
            lines.extend(unavailable.iter().map(|(source, reason)| {
                format!("  FAIL {}（{}）", source.name, reason.message_id())
            }));
            lines.push(format!(
                "可用 {}/{}（{:.0}%）",
                available.len(),
                available.len() + unavailable.len(),
                available_rate * 100.0
            ));
            lines.join("\n") + "\n"
        }
    }
}

/// 渲染一行 JSON（末尾换行，让重定向与 `diff` 的末行正常）。
pub fn json_line(value: &serde_json::Value) -> String {
    match serde_json::to_string_pretty(value) {
        Ok(text) => format!("{text}\n"),
        Err(error) => format!("{{\"error\":\"序列化失败：{error}\"}}\n"),
    }
}

// ---------------------------------------------------------------------------
// 书源分组与启停（T093 / FR-001）
// ---------------------------------------------------------------------------

/// `source group <源id...> --name <分组|->`（FR-001）。
///
/// # `-` 表示移出分组
///
/// 见 `flags.rs` 的说明。移出的语义是 `group_id = None`——这与
/// "分到一个名叫 `-` 的分组"完全不同，故 `-` 是**保留值**，不会真的
/// 建出叫 `-` 的分组。
///
/// # 分组按名字寻址而不是 id
///
/// 用户从 CLI 操作时手里是分组名（`--group 玄幻`），不是数据库 id。
/// 故这里先 `ensure_group` 拿到（或建出）id 再赋值——这也让 CLI 能**顺手
/// 建新分组**，不必先有个"建分组"的独立命令。
pub fn run_source_group(
    options: &GlobalOptions,
    source_ids: &[String],
    name: &str,
) -> CommandOutcome {
    let source_store = match open_source_store(options) {
        Ok(store) => store,
        Err(outcome) => return outcome,
    };

    // 验证源都存在（比"改完了才发现 id 写错"好得多）
    let known = match source_store.all() {
        Ok(sources) => sources,
        Err(error) => return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    };
    let missing: Vec<&String> = source_ids
        .iter()
        .filter(|id| !known.iter().any(|source| &source.id == *id))
        .collect();
    if !missing.is_empty() {
        return CommandOutcome::failure(
            &ErrorKind::Rule,
            &format!(
                "error.rule.source.missing ids={}",
                missing
                    .iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
    }

    let group_id = if name == "-" {
        None
    } else {
        match source_store.dao().ensure_group(name, 0) {
            Ok(id) => Some(id),
            Err(error) => {
                return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"))
            }
        }
    };

    // 一次赋一组：`assign_group` 收的是**切片**，故批量赋值是单次调用
    // （也让"要么全成要么全不成"的事务性更容易保证）
    let updated = match source_store.dao().assign_group(source_ids, group_id) {
        Ok(updated) => updated,
        Err(error) => {
            return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"));
        }
    };

    let report = serde_json::json!({
        "updated": updated,
        "group": if name == "-" { serde_json::Value::Null } else { serde_json::json!(name) },
    });
    CommandOutcome::with_stdout(format!("{report}\n"))
}

/// `source enable|disable <源id...|--all>`（FR-001）。
///
/// # 为什么 `--all` 与显式 id 互斥
///
/// 同时给会让意图含糊（"只启用我列的那几个"还是"全都启用"？）。报用法
/// 错误比静默选一个安全——静默选错会让用户以为批量操作成功了。
pub fn run_source_enable(
    options: &GlobalOptions,
    source_ids: &[String],
    all: bool,
    enabled: bool,
) -> CommandOutcome {
    if all && !source_ids.is_empty() {
        return CommandOutcome {
            exit_code: 2,
            stdout: String::new(),
            stderr: "用法错误：`--all` 与显式书源 id 不能同时给\n".to_string(),
        };
    }

    let source_store = match open_source_store(options) {
        Ok(store) => store,
        Err(outcome) => return outcome,
    };

    let targets: Vec<String> = if all {
        match source_store.all() {
            Ok(sources) => sources.into_iter().map(|source| source.id).collect(),
            Err(error) => {
                return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"));
            }
        }
    } else {
        if source_ids.is_empty() {
            return CommandOutcome {
                exit_code: 2,
                stdout: String::new(),
                stderr: "用法错误：需要至少一个书源 id，或用 `--all`\n".to_string(),
            };
        }
        source_ids.to_vec()
    };

    let changed = match source_store.dao().set_enabled(&targets, enabled) {
        Ok(changed) => changed,
        Err(error) => return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    };

    let report = serde_json::json!({
        "enabled": enabled,
        "matched": targets.len(),
        "changed": changed,
    });
    CommandOutcome::with_stdout(format!("{report}\n"))
}

/// `source groups`（列出分组）。
pub fn run_source_groups(options: &GlobalOptions) -> CommandOutcome {
    let source_store = match open_source_store(options) {
        Ok(store) => store,
        Err(outcome) => return outcome,
    };
    let groups = match source_store.groups() {
        Ok(groups) => groups,
        Err(error) => return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    };
    let payload: Vec<serde_json::Value> = groups
        .iter()
        .map(|group| {
            serde_json::json!({
                "id": group.id,
                "name": group.name,
                "sort_order": group.sort_order,
                "enabled": group.enabled,
            })
        })
        .collect();
    match serde_json::to_string_pretty(&payload) {
        Ok(text) => CommandOutcome::with_stdout(format!("{text}\n")),
        Err(error) => CommandOutcome::failure(
            &ErrorKind::Configuration,
            &format!("error.configuration reason={error}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// 搜索关键词（T093 / FR-018）
// ---------------------------------------------------------------------------

/// 打开书源库的**同一份**数据库句柄（关键词与书源同库）。
fn open_misc_dao(options: &GlobalOptions) -> Result<tome_store::dao::MiscDao, CommandOutcome> {
    let data_dir = crate::data_commands::resolve_data_dir_for_keywords(options)?;
    match tome_store::Db::open(&data_dir.database_path()) {
        Ok(db) => Ok(tome_store::dao::MiscDao::new(db)),
        Err(error) => Err(CommandOutcome::failure(
            &error.to_error_kind(),
            &format!("error.storage.sqlite reason={error}"),
        )),
    }
}

/// `keyword list`（FR-018：收藏的关键词）。
pub fn run_keyword_list(options: &GlobalOptions) -> CommandOutcome {
    let dao = match open_misc_dao(options) {
        Ok(dao) => dao,
        Err(outcome) => return outcome,
    };
    let keywords = match dao.search_keywords() {
        Ok(keywords) => keywords,
        Err(error) => return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    };
    let payload: Vec<serde_json::Value> = keywords
        .iter()
        .map(|keyword| {
            serde_json::json!({
                "keyword": keyword.keyword,
                "created_at": keyword.created_at,
                "use_count": keyword.use_count,
            })
        })
        .collect();
    match serde_json::to_string_pretty(&payload) {
        Ok(text) => CommandOutcome::with_stdout(format!("{text}\n")),
        Err(error) => CommandOutcome::failure(
            &ErrorKind::Configuration,
            &format!("error.configuration reason={error}"),
        ),
    }
}

/// `keyword add <词>`（FR-018）。
///
/// # 重复收藏是"更新使用时间"而不是报错
///
/// 用户重复运行 `keyword add 斗破` 是常见操作（从历史里再选一次）。
/// 报"已存在"会打断这个流程，而那是无害的。
pub fn run_keyword_add(options: &GlobalOptions, keyword: &str) -> CommandOutcome {
    let dao = match open_misc_dao(options) {
        Ok(dao) => dao,
        Err(outcome) => return outcome,
    };
    let outcome = match dao.add_search_keyword(keyword) {
        Ok(id) => id,
        Err(error) => return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    };
    let report = serde_json::json!({ "keyword": keyword, "id": outcome });
    CommandOutcome::with_stdout(format!("{report}\n"))
}

/// `keyword remove <词>`（FR-018）。
pub fn run_keyword_remove(options: &GlobalOptions, keyword: &str) -> CommandOutcome {
    let dao = match open_misc_dao(options) {
        Ok(dao) => dao,
        Err(outcome) => return outcome,
    };
    if let Err(error) = dao.remove_search_keyword(keyword) {
        return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"));
    }
    let report = serde_json::json!({ "removed": keyword });
    CommandOutcome::with_stdout(format!("{report}\n"))
}

// ---------------------------------------------------------------------------
// 规则求值轨迹（T093 / FR-037）——`run --rule <规则> --input <文本> --trace`
// ---------------------------------------------------------------------------

/// 输出一条规则的求值轨迹（FR-037）。
///
/// # 输入形态：`--rule <规则> --input <文本> --trace`
///
/// 轨迹作用于"一条规则 + 一段输入"，故它需要**同时**拿到两者。契约里
/// `run` 的形态是 `<source-id> --action <动作> --input <...>`，而轨迹要的是
/// 规则原文——故用 `--rule` 区分两种模式：给了 `--rule` 走轨迹。
///
/// # `--json` 由调用方从 `Content-Type` 推断（此处是参数）
///
/// 轨迹的输入可能是 HTML 也可能是 JSON 文本。判据是规则本身：
/// `@json:` 前缀说明作者认为输入是 JSON。故这里**由规则前缀推断**
/// 而不额外要一个 flag——少一个要记的开关。
pub fn run_rule_trace(rule: &str, input: &str) -> CommandOutcome {
    let is_json = rule.trim().to_ascii_lowercase().starts_with("@json:");
    let kind = if is_json {
        tome_rule::trace::TraceInput::Json
    } else {
        tome_rule::trace::TraceInput::Html
    };
    let trace = tome_rule::trace::trace_rule(rule, kind, input);

    match serde_json::to_string_pretty(&trace) {
        Ok(text) => {
            // 有失败步骤时给业务错误码（1）：轨迹本身算出来了，但规则没跑通。
            // 用 1 而非 0 让脚本能据此判断"这条规则有问题"。
            if trace.is_clean() {
                CommandOutcome::with_stdout(format!("{text}\n"))
            } else {
                CommandOutcome::failure(&ErrorKind::Rule, format!("{text}\n").trim_end())
            }
        }
        Err(error) => CommandOutcome::failure(
            &ErrorKind::Configuration,
            &format!("error.configuration reason={error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_book() -> tome_core::Book {
        tome_core::Book {
            id: 0,
            origin: tome_core::BookOrigin::Network,
            origin_id: "https://a.com|https://a.com/book/1".to_string(),
            name: "斗破苍穹".to_string(),
            author: Some("天蚕土豆".to_string()),
            cover_url: Some("https://a.com/c.jpg".to_string()),
            intro: Some("简介".to_string()),
            kind: Some("玄幻".to_string()),
            book_group: None,
            read_config: None,
            toc_url: Some("https://a.com/toc".to_string()),
            latest_chapter_title: None,
            latest_chapter_time: None,
            library_root_id: None,
            relative_path: None,
            charset: None,
            format: None,
            total_chapters: None,
            dur_chapter_index: None,
            added_at: 0,
        }
    }

    fn sample_source(id: &str, enabled: bool) -> tome_core::Source {
        tome_core::Source {
            id: id.to_string(),
            name: format!("源{id}"),
            kind: tome_core::SourceKind::Book,
            group_id: None,
            enabled,
            book_source_type: None,
            book_url_pattern: None,
            search_url: None,
            explore_url: None,
            rule_search: None,
            rule_explore: None,
            rule_book_info: None,
            rule_toc: None,
            rule_content: None,
            header: None,
            login_url: None,
            login_ui: None,
            login_check_js: None,
            js: None,
            charset: None,
            rate_limit_ms: None,
            weight: None,
            explore_screen: None,
            comment: None,
            last_update_time: None,
            created_at: 0,
        }
    }

    #[test]
    fn 详情渲染成_json_且字段齐全() {
        let text = render_book(OutputFormat::Json, &sample_book());
        let value: serde_json::Value = serde_json::from_str(&text).expect("应是 JSON");
        assert_eq!(value["name"], "斗破苍穹");
        assert_eq!(value["author"], "天蚕土豆");
        assert_eq!(value["toc_url"], "https://a.com/toc");
    }

    #[test]
    fn 详情渲染人读形态() {
        let text = render_book(OutputFormat::Human, &sample_book());
        assert!(text.contains("书名：斗破苍穹"));
        assert!(text.contains("作者：天蚕土豆"));
    }

    #[test]
    fn 详情缺字段时人读形态给占位() {
        let mut book = sample_book();
        book.author = None;
        book.kind = None;
        let text = render_book(OutputFormat::Human, &book);
        assert!(text.contains("（未提供）"));
    }

    #[test]
    fn 目录渲染() {
        let chapters = vec![
            tome_core::Chapter {
                id: 0,
                book_id: 0,
                index_in_book: 0,
                title: "第一章".to_string(),
                url: Some("https://a.com/1".to_string()),
                byte_offset: None,
                epub_entry: None,
                is_cached: false,
                is_read: false,
                word_count: None,
            },
            tome_core::Chapter {
                id: 0,
                book_id: 0,
                index_in_book: 1,
                title: "第二章".to_string(),
                url: None,
                byte_offset: None,
                epub_entry: None,
                is_cached: false,
                is_read: false,
                word_count: None,
            },
        ];
        let json = render_chapters(OutputFormat::Json, &chapters);
        let value: serde_json::Value = serde_json::from_str(&json).expect("应是 JSON");
        assert_eq!(value.as_array().expect("应是数组").len(), 2);
        assert_eq!(value[0]["index"], 0);
        assert_eq!(value[1]["title"], "第二章");

        let human = render_chapters(OutputFormat::Human, &chapters);
        assert!(human.contains("第一章"));
        assert!(human.contains("共 2 章"));
    }

    #[test]
    fn 正文渲染带字符数() {
        let text = render_content(OutputFormat::Json, "https://a.com/1", "中文正文");
        let value: serde_json::Value = serde_json::from_str(&text).expect("应是 JSON");
        assert_eq!(value["content"], "中文正文");
        assert_eq!(value["length"], 4, "长度按字符数而非字节数");
    }

    #[test]
    fn 正文人读形态是纯文本() {
        // 契约：`content --format human` 应是可直接阅读的正文
        let text = render_content(OutputFormat::Human, "https://a.com/1", "正文内容");
        assert_eq!(text, "正文内容\n");
    }

    #[test]
    fn 书源列表渲染() {
        let sources = vec![
            sample_source("https://on.com", true),
            sample_source("https://off.com", false),
        ];
        let json = render_source_list(OutputFormat::Json, &sources);
        let value: serde_json::Value = serde_json::from_str(&json).expect("应是 JSON");
        assert_eq!(value.as_array().expect("应是数组").len(), 2);
        assert_eq!(value[0]["enabled"], true);
        assert_eq!(value[1]["enabled"], false);

        let human = render_source_list(OutputFormat::Human, &sources);
        assert!(human.contains("[启用]"));
        assert!(human.contains("[停用]"));
    }

    #[test]
    fn 检查报告渲染() {
        let available = vec![sample_source("https://ok.com", true)];
        let unavailable = vec![(sample_source("https://bad.com", true), ErrorKind::Network)];
        let json = render_check_report(OutputFormat::Json, &available, &unavailable, 0.5);
        let value: serde_json::Value = serde_json::from_str(&json).expect("应是 JSON");
        assert_eq!(value["total"], 2);
        assert_eq!(value["available"], 1);
        assert_eq!(value["unavailable"], 1);
        assert_eq!(value["results"][1]["verdict"], "unavailable");

        let human = render_check_report(OutputFormat::Human, &available, &unavailable, 0.5);
        assert!(human.contains("OK"));
        assert!(human.contains("FAIL"));
        assert!(human.contains("50%"));
    }

    #[test]
    fn 空检查报告也能渲染() {
        let json = render_check_report(OutputFormat::Json, &[], &[], 1.0);
        let value: serde_json::Value = serde_json::from_str(&json).expect("应是 JSON");
        assert_eq!(value["total"], 0);
        assert_eq!(value["available_rate"], 1.0);
    }

    #[test]
    fn 输出末尾有换行() {
        // 让重定向与 `diff` 的末行正常
        assert!(json_line(&serde_json::json!({"a": 1})).ends_with('\n'));
        assert!(render_book(OutputFormat::Human, &sample_book()).ends_with('\n'));
        assert!(render_content(OutputFormat::Human, "u", "t").ends_with('\n'));
        assert!(
            render_source_list(OutputFormat::Human, &[sample_source("s", true)]).ends_with('\n')
        );
    }

    #[test]
    fn 同一输入的输出逐字节一致() {
        // T029 的"逐字节一致"对**所有**命令都成立，不只 search
        let first = render_book(OutputFormat::Json, &sample_book());
        for _ in 0..5 {
            assert_eq!(render_book(OutputFormat::Json, &sample_book()), first);
        }
    }

    #[test]
    fn 最小书籍只填身份与目录地址() {
        let book = minimal_book("https://a.com", "https://a.com/book/1");
        assert_eq!(book.origin_id, "https://a.com|https://a.com/book/1");
        assert_eq!(book.toc_url.as_deref(), Some("https://a.com/book/1"));
        assert_eq!(book.id, 0, "未入库的书籍没有 id");
        assert_eq!(book.total_chapters, None, "不臆造章节数（那要跑目录）");
    }
}
