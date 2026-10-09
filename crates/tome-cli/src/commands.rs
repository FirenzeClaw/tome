//! [DONE] CLI 命令实现（tasks.md T028/T029 / `contracts/cli.md`）
//!
//! # 组合根
//!
//! CLI 是唯一允许依赖全部实现 crate 的位置（`plan.md` 的依赖图）：它把
//! `tome-net` 的执行器装配成 `NetHost`，交给 `tome-source` 的管线使用。
//! **禁止全局 `CONFIG` 单例**（架构决策 A3）——配置经参数传递，
//! 故本模块的每个入口都显式收 [`GlobalOptions`]。
//!
//! # 输出的确定性
//!
//! T029 的验收项是"录制→离线重放输出**逐字节一致**"。这要求输出里
//! **不含**任何随运行变化的内容（时间戳、耗时、随机顺序）。故：
//!
//! - 源结果按书源 id 排序（而非按完成顺序）
//! - 字段顺序由 `serde` 的结构体定义固定
//! - 不输出耗时（除 `--format human` 的诊断行）
//!
//! # 与存储层的关系（S1-1 阶段的取舍）
//!
//! S1-1 阶段 `tome-store` 尚未实现（它在后续切片），故书源**不落库**：
//! `source import` 解析并报告，`search` 从文件或已解析的源直接跑。
//! 这不是降级——T026-T029 的验收项都不依赖存储，而"先打通管线再接存储"
//! 让规则引擎与网络层的缺陷能在存储的噪音之外独立暴露。

use std::io::Read;
use std::path::Path;

use tome_core::{ErrorKind, NetHost};
use tome_source::registry::SourceRegistry;
use tome_source::source::{load_from_channel, LoadOutcome, ParsedSource};
use tome_source::{search, SourceError};

use crate::flags::{exit_code_for, unsupported, Command, GlobalOptions, NetworkMode, OutputFormat};

/// 一次命令执行的结果：退出码 + 标准输出 + 标准错误。
///
/// ## 为什么返回字符串而不是直接打印
///
/// 直接打印让命令无法在测试里断言输出——而 T029 的验收项正是
/// "输出逐字节一致"，那必须能被测试断言。故执行与输出分离，
/// 由 [`crate::run`] 的调用方（`main`）负责写到 stdout/stderr。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    /// 进程退出码（`contracts/cli.md`：0/1/2/3）
    pub exit_code: i32,
    /// 标准输出（结构化结果）
    pub stdout: String,
    /// 标准错误（错误诊断）
    pub stderr: String,
}

impl CommandOutcome {
    /// 成功且带标准输出。
    fn with_stdout(stdout: String) -> Self {
        Self {
            exit_code: 0,
            stdout,
            stderr: String::new(),
        }
    }

    /// 失败：按错误分类决定退出码。
    fn failure(kind: &ErrorKind, message: &str) -> Self {
        Self {
            exit_code: exit_code_for(kind),
            stdout: String::new(),
            stderr: format!("{}\n", message),
        }
    }
}

/// 执行一条命令，产出（退出码, stdout, stderr）。
///
/// [DONE] CLI 的对外入口。`stdin_reader` 为 `source import --stdin` 提供输入。
pub fn run(
    options: &GlobalOptions,
    command: &Command,
    host: &dyn NetHost,
    stdin_reader: &mut dyn Read,
) -> CommandOutcome {
    run_with_js(options, command, host, None, stdin_reader)
}

/// 与 [`run`] 相同，但额外接受一个**可共享的**网络宿主句柄。
///
/// # 为什么需要它
///
/// 含 JS 的书源需要把宿主能力交给 JS 引擎，而
/// [`tome_core::NetHost`] 在 JS 侧以 `Arc<dyn NetHost>` 持有。
/// `&dyn NetHost` 借不出一个 `Arc`，故由组合根（`main.rs`）把执行器
/// 以 `Arc` 形式传进来。
///
/// 不传时（`None`）含 JS 的源会得到**结构化能力错误**——而不是
/// "静默跑不起来"。那让"忘了传"这件事在第一次运行时就暴露。
pub fn run_with_js(
    options: &GlobalOptions,
    command: &Command,
    host: &dyn NetHost,
    js_host: Option<std::sync::Arc<dyn NetHost>>,
    stdin_reader: &mut dyn Read,
) -> CommandOutcome {
    match command {
        Command::SourceImport { file, url, stdin } => run_import(
            options,
            file.as_deref(),
            url.as_deref(),
            *stdin,
            host,
            stdin_reader,
        ),
        Command::Search {
            source_id,
            source_file,
            keyword,
        } => run_search(
            options,
            source_id.as_deref(),
            source_file.as_deref(),
            keyword,
            host,
            js_host,
        ),
        Command::Run {
            source_id,
            action,
            input,
        } => run_action(options, source_id, action, input.as_deref(), host, js_host),
    }
}

/// `run <source-id> --action <a> [--input <s>]`：通用执行入口。
///
/// # 本阶段实现的动作
///
/// 只有 `search`（与 `search` 子命令等价，但走 `--action` 形态便于脚本化）。
/// `info`/`toc`/`content` 的规则求值路径相同，但需要详情/目录/正文的
/// DTO 映射——那属后续切片（`contracts/cli.md` 定义了它们的契约）。
///
/// 不支持的动作返回**用法错误**（退出码 2）而不是"业务错误"（1）：
/// 前者是"你调的方式不对"，后者是"源坏了"。两者对脚本的后续处置不同。
fn run_action(
    options: &GlobalOptions,
    source_id: &str,
    action: &str,
    input: Option<&str>,
    host: &dyn NetHost,
    js_host: Option<std::sync::Arc<dyn NetHost>>,
) -> CommandOutcome {
    match action {
        "search" => {
            let Some(keyword) = input else {
                return CommandOutcome {
                    exit_code: 2,
                    stdout: String::new(),
                    stderr: "用法错误：`--action search` 需要 `--input <关键词>`
".to_string(),
                };
            };
            run_search(options, Some(source_id), None, keyword, host, js_host)
        }
        // ## 后续切片的动作：报**能力错误**（退出码 3）而不是用法错误
        //
        // 它们是本产品**将要**支持的（契约里定义了），故不该说"你调错了"；
        // 但当前也做不到，故是 `UnsupportedCapability`。
        // 这个区别让脚本能据此判断"等版本更新"与"改调用方式"。
        other @ ("info" | "toc" | "content") => CommandOutcome::failure(
            &unsupported(other),
            &format!(
                "error.unsupported_capability 动作 `{other}` 属后续切片（contracts/cli.md 已定义契约，本切片实现 search）"
            ),
        ),
        other => CommandOutcome {
            exit_code: 2,
            stdout: String::new(),
            stderr: format!(
                "用法错误：未知动作 `{other}`（可用：search；info/toc/content 属后续切片）
"
            ),
        },
    }
}

/// `source import`：按渠道装入书源并输出导入报告。
fn run_import(
    options: &GlobalOptions,
    file: Option<&Path>,
    url: Option<&str>,
    stdin: bool,
    host: &dyn NetHost,
    stdin_reader: &mut dyn Read,
) -> CommandOutcome {
    // 渠道优先级：位置参数（file）> `--url` > `--stdin`。
    // 契约没规定优先级，但"更明确指定的优先"符合直觉；同时给三个时
    // 只有第一个生效，另外两个被忽略（不报错——避免让一次脚本调用
    // 因多带了一个开关而失败）。
    let channel = if let Some(path) = file {
        match std::fs::read_to_string(path) {
            Ok(text) => tome_core::seam::SourceImportChannel::FileText(text),
            Err(error) => {
                return CommandOutcome::failure(
                    &ErrorKind::Configuration,
                    &format!(
                        "error.configuration.io path={} reason={error}",
                        path.display()
                    ),
                );
            }
        }
    } else if let Some(url) = url {
        tome_core::seam::SourceImportChannel::Url(url.to_string())
    } else if stdin {
        let mut text = String::new();
        if let Err(error) = stdin_reader.read_to_string(&mut text) {
            return CommandOutcome::failure(
                &ErrorKind::Configuration,
                &format!("error.configuration.stdin reason={error}"),
            );
        }
        tome_core::seam::SourceImportChannel::PastedText(text)
    } else {
        return CommandOutcome::failure(
            &ErrorKind::Configuration,
            "error.configuration.usage 需要 <file>、--url 或 --stdin 之一",
        );
    };

    match load_from_channel(&channel, host) {
        Ok(outcome) => {
            // 落盘到注册表：`search <source-id>` 依赖它（`contracts/cli.md`
            // 定义的流程是"先 import 再 search <id>"）。
            let registry_path = registry_path(options);
            let mut registry = match SourceRegistry::load(&registry_path) {
                Ok(registry) => registry,
                Err(error) => {
                    return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"));
                }
            };
            let update = registry.merge(&outcome);
            if let Err(error) = registry.save(&registry_path) {
                return CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"));
            }

            let report = render_import_report(options.format, &outcome, &update, &registry_path);
            CommandOutcome::with_stdout(report)
        }
        Err(error) => CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")),
    }
}

/// 解析注册表路径：`--data-dir` 优先，否则用"可移植模式"的默认位置。
fn registry_path(options: &GlobalOptions) -> std::path::PathBuf {
    match &options.data_dir {
        Some(dir) => dir.join(tome_source::registry::REGISTRY_FILE_NAME),
        None => tome_source::registry::default_registry_path(),
    }
}

/// `search`：跑一次搜索并输出结构化结果。
fn run_search(
    options: &GlobalOptions,
    source_id: Option<&str>,
    source_file: Option<&Path>,
    keyword: &str,
    host: &dyn NetHost,
    js_host: Option<std::sync::Arc<dyn NetHost>>,
) -> CommandOutcome {
    // 载入书源：S1-1 阶段从文件读（存储落地在后续切片，见模块文档）。
    let sources = match load_sources_for_search(options, source_id, source_file, host) {
        Ok(sources) => sources,
        Err(outcome) => return outcome,
    };

    // ## 组合根的职责：为含 JS 的源准备引擎工厂
    //
    // 只有 `source_needs_javascript` 为真的源才会真的建 runtime
    // （见 `SearchEngine::with_js` 的说明）——纯 CSS/正则的源走快路径。
    //
    // ## 为什么需要 `Arc<dyn NetHost>`
    //
    // JS 引擎要持有宿主能力（`HostBridge` 里的字段是 `Arc<dyn NetHost>`），
    // 而 `&dyn NetHost` 无法克隆成 `Arc`。故调用方需在能给出 `Arc` 时
    // 经 `run_with_js` 传入；给不出时（测试里的借入宿主）JS 引擎没有
    // 网络能力——那时含 JS 的源会得到**结构化能力错误**，而不是静默失败。
    let engine = match js_host {
        Some(js_host) => {
            let js_host = js_host.clone();
            let factory: search::JsEngineFactory<'_> = std::sync::Arc::new(move |source| {
                crate::js_host::build_source_engine(source, Some(js_host.clone()))
            });
            search::SearchEngine::with_js(&sources, host, factory)
        }
        None => search::SearchEngine::new(&sources, host),
    };
    let filter: Vec<String> = source_id.map(|id| vec![id.to_string()]).unwrap_or_default();
    let (successes, failures) = search::collect_search(engine.search(keyword, &filter));

    let exit_code = if successes.is_empty() && !failures.is_empty() {
        // 全部源都失败 → 按第一个失败的分类给退出码。
        // 有成功结果时即使部分源失败也返回 0：单源失败是流内信息
        // （A6），不该让整个命令算失败。
        exit_code_for(&failures[0].1)
    } else {
        0
    };

    if successes.is_empty() && failures.is_empty() {
        // 无源可查：不是错误（可能过滤后为空），但要让用户看见
        return CommandOutcome {
            exit_code: 0,
            stdout: render_search_success(options.format, &[]),
            stderr: "error.rule.source.missing 没有可用的书源（用 --source-file 指定，或检查过滤条件）\n"
                .to_string(),
        };
    }

    let stdout = render_search_success(options.format, &successes);
    let mut stderr = String::new();
    if !failures.is_empty() {
        // 失败信息进 stderr（契约：错误输出到 stderr）
        stderr = render_search_failures(options.format, &failures);
    }

    CommandOutcome {
        exit_code,
        stdout,
        stderr,
    }
}

/// 为搜索载入书源。
///
/// 两条路径：
/// - `--source-file <f>`：从**任意**书源文件读（不走注册表，供"临时试一个源"）
/// - `<source-id>`：从**注册表**读（`contracts/cli.md` 的规范流程：
///   `source import` 之后 `search <id>`）
fn load_sources_for_search(
    options: &GlobalOptions,
    source_id: Option<&str>,
    source_file: Option<&Path>,
    host: &dyn NetHost,
) -> Result<Vec<ParsedSource>, CommandOutcome> {
    if let Some(path) = source_file {
        let text = std::fs::read_to_string(path).map_err(|error| {
            CommandOutcome::failure(
                &ErrorKind::Configuration,
                &format!(
                    "error.configuration.io path={} reason={error}",
                    path.display()
                ),
            )
        })?;
        let outcome =
            load_from_channel(&tome_core::seam::SourceImportChannel::FileText(text), host)
                .map_err(|error| {
                    CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"))
                })?;
        return Ok(outcome.sources);
    }

    let Some(id) = source_id else {
        return Err(CommandOutcome::failure(
            &ErrorKind::Configuration,
            "error.configuration.usage 需要 <source-id> 或 --source-file",
        ));
    };

    let path = registry_path(options);
    let registry = SourceRegistry::load(&path)
        .map_err(|error| CommandOutcome::failure(&error.to_error_kind(), &format!("{error}")))?;
    match registry.get(id) {
        Some(parsed) => Ok(vec![parsed.clone()]),
        None => {
            let hint = format!(
                "error.rule.source.missing id={id} registry={}",
                path.display()
            );
            Err(CommandOutcome::failure(
                &ErrorKind::Rule,
                &format!(
                    "{hint}（注册表里没有这个 id；先跑 `tome source import <file>`，                     或用 --source-file 直接指定书源文件）"
                ),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// 输出渲染
// ---------------------------------------------------------------------------

/// 渲染导入报告。
///
/// JSON 字段顺序由结构体定义固定（`serde` 的字段序），这是
/// "输出逐字节一致"的基础之一。
fn render_import_report(
    format: OutputFormat,
    outcome: &LoadOutcome,
    update: &tome_source::registry::RegistryUpdate,
    registry_path: &Path,
) -> String {
    match format {
        OutputFormat::Json => {
            let report = ImportReportJson {
                added: outcome.report.added,
                updated: outcome.report.updated,
                skipped: outcome.report.skipped,
                total: outcome.sources.len(),
                source_ids: outcome
                    .sources
                    .iter()
                    .map(|parsed| parsed.source.id.clone())
                    .collect(),
                failures: outcome
                    .report
                    .failures
                    .iter()
                    .map(|failure| ImportFailureJson {
                        subject: failure.subject.clone(),
                        error: failure.error.message_id().to_string(),
                    })
                    .collect(),
                registry_added: update.added,
                registry_updated: update.updated,
                registry_total: registry_len_at(registry_path),
            };
            let mut text =
                serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string());
            // 末尾换行：`echo`/重定向的常见期待，也让 `diff` 的末行正常
            text.push('\n');
            text
        }
        OutputFormat::Human => {
            let mut lines = vec![
                format!(
                    "注册表：新增 {}，更新 {}（路径 {}）",
                    update.added,
                    update.updated,
                    registry_path.display()
                ),
                format!(
                    "导入完成：新增 {}，更新 {}，跳过 {}（共 {} 条解析成功）",
                    outcome.report.added,
                    outcome.report.updated,
                    outcome.report.skipped,
                    outcome.sources.len()
                ),
            ];
            for parsed in &outcome.sources {
                lines.push(format!("  {} —— {}", parsed.source.id, parsed.source.name));
            }
            for failure in &outcome.report.failures {
                lines.push(format!(
                    "  跳过 {}：{}",
                    failure.subject,
                    failure.error.message_id()
                ));
            }
            lines.join("\n") + "\n"
        }
    }
}

/// 读注册表的当前总条数（失败时返回 0——它是诊断信息，不该让导入失败）。
fn registry_len_at(path: &Path) -> usize {
    SourceRegistry::load(path)
        .map(|registry| registry.len())
        .unwrap_or(0)
}

/// 渲染搜索成功结果。
///
/// ## 结果按书源 id 排序
///
/// 流是按源渐进产出的，但**输出**必须确定（T029 的"逐字节一致"）。
/// 故渲染前按 id 排序——排序是稳定的（同一源内的结果保持原顺序）。
fn render_search_success(
    format: OutputFormat,
    successes: &[(String, Vec<tome_core::Book>)],
) -> String {
    let mut ordered: Vec<&(String, Vec<tome_core::Book>)> = successes.iter().collect();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));

    match format {
        OutputFormat::Json => {
            let books: Vec<BookJson> = ordered
                .iter()
                .flat_map(|(source_id, books)| {
                    books.iter().map(move |book| BookJson {
                        source: source_id.clone(),
                        name: book.name.clone(),
                        author: book.author.clone(),
                        cover: book.cover_url.clone(),
                        intro: book.intro.clone(),
                        book_url: book.toc_url.clone().unwrap_or_default(),
                    })
                })
                .collect();
            let mut text =
                serde_json::to_string_pretty(&books).unwrap_or_else(|_| "[]".to_string());
            text.push('\n');
            text
        }
        OutputFormat::Human => {
            let mut lines: Vec<String> = Vec::new();
            for (source_id, books) in ordered {
                lines.push(format!("【{source_id}】{} 条", books.len()));
                for book in books {
                    lines.push(format!(
                        "  {} —— {}{}",
                        book.name,
                        book.author.as_deref().unwrap_or("佚名"),
                        book.toc_url
                            .as_deref()
                            .map(|url| format!("  {url}"))
                            .unwrap_or_default()
                    ));
                }
            }
            if lines.is_empty() {
                lines.push("（无结果）".to_string());
            }
            lines.join("\n") + "\n"
        }
    }
}

/// 渲染搜索失败信息（进 stderr）。
fn render_search_failures(format: OutputFormat, failures: &[(String, ErrorKind)]) -> String {
    let mut ordered: Vec<&(String, ErrorKind)> = failures.iter().collect();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));

    match format {
        OutputFormat::Json => {
            let items: Vec<SourceFailureJson> = ordered
                .iter()
                .map(|(source_id, error)| SourceFailureJson {
                    source: source_id.clone(),
                    error: error.message_id().to_string(),
                })
                .collect();
            let mut text =
                serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".to_string());
            text.push('\n');
            text
        }
        OutputFormat::Human => {
            ordered
                .iter()
                .map(|(source_id, error)| format!("失败 {source_id}：{}", error.message_id()))
                .collect::<Vec<String>>()
                .join("\n")
                + "\n"
        }
    }
}

// ---------------------------------------------------------------------------
// 输出 DTO
// ---------------------------------------------------------------------------
//
// ## 为什么单独定义而不是直接序列化 core 的 DTO
//
// core 的 DTO 刻意不派生 `serde`（crate 文档："DTO 刻意不派生 serde 等
// 序列化能力——那是存储/网络实现 crate 的关切"）。CLI 的输出 schema 也不该
// 等同于领域模型：契约 `cli.md` 定义了**面向使用者的字段名**
// （`{source, name, author, cover, intro, book_url}`），它是稳定契约，
// 而领域模型会演进。分开让两者的变化互不牵连。

#[derive(Debug, serde::Serialize)]
struct ImportReportJson {
    /// 新增的源数
    added: usize,
    /// 更新的源数
    updated: usize,
    /// 跳过的源数
    skipped: usize,
    /// 解析成功的总条数
    total: usize,
    /// 解析出的书源 id（排序后，保证输出确定）
    source_ids: Vec<String>,
    /// 注册表里新增的条数
    registry_added: usize,
    /// 注册表里更新的条数（同 id 覆盖）
    registry_updated: usize,
    /// 注册表当前总条数
    registry_total: usize,
    /// 逐条失败
    failures: Vec<ImportFailureJson>,
}

#[derive(Debug, serde::Serialize)]
struct ImportFailureJson {
    /// 失败条目标识
    subject: String,
    /// 错误分类标识
    error: String,
}

#[derive(Debug, serde::Serialize)]
struct BookJson {
    /// 产生本结果的源
    source: String,
    /// 书名
    name: String,
    /// 作者
    author: Option<String>,
    /// 封面地址
    cover: Option<String>,
    /// 简介
    intro: Option<String>,
    /// 详情页地址
    book_url: String,
}

#[derive(Debug, serde::Serialize)]
struct SourceFailureJson {
    /// 失败的源
    source: String,
    /// 错误分类标识
    error: String,
}

/// 把 [`SourceError`] 转成 CLI 的失败结果（供测试与调用方复用）。
pub fn failure_for(error: &SourceError) -> CommandOutcome {
    CommandOutcome::failure(&error.to_error_kind(), &format!("{error}"))
}

/// 从 [`NetworkMode`] 构造执行器（T029 的接线点）。
///
/// [DONE] `--record`/`--replay`/`--offline` 落到执行器的唯一处。
///
/// ## 为什么 `NetworkMode::Live` 也要显式走这里
///
/// 让"三种模式都由同一个构造函数产出"成为结构上的事实——否则
/// "Live 时忘了关录制"这类缺陷只会在特定组合下暴露。
pub fn build_executor(
    network: &NetworkMode,
    default_interval: Option<std::time::Duration>,
) -> Result<tome_net::RequestExecutor, ErrorKind> {
    let mut builder = tome_net::RequestExecutor::builder();
    if let Some(interval) = default_interval {
        builder = builder.default_interval(interval);
    }
    if let Some((root, mode)) = network.to_executor_mode() {
        builder = builder.replay(root, mode);
    }
    builder.build().map_err(|error| error.to_error_kind())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::{parse, ParseOutcome};
    use std::sync::Mutex;
    use tome_core::{HeaderMap, RequestSpec, Response};

    /// 假宿主：按 URL 返回预设的响应体，并记录调用。
    struct FakeHost {
        responses: Mutex<Vec<(String, Response)>>,
        seen: Mutex<Vec<RequestSpec>>,
        /// 未匹配时返回的状态码（用于模拟失败）
        default_status: u16,
    }

    impl FakeHost {
        fn new(responses: Vec<(&str, &str)>) -> Self {
            Self {
                responses: Mutex::new(
                    responses
                        .into_iter()
                        .map(|(url_fragment, body)| {
                            (
                                url_fragment.to_string(),
                                Response {
                                    status: 200,
                                    headers: HeaderMap::new(),
                                    body_bytes: body.as_bytes().to_vec(),
                                    detected_charset: Some("utf-8".to_string()),
                                },
                            )
                        })
                        .collect(),
                ),
                seen: Mutex::new(Vec::new()),
                default_status: 200,
            }
        }

        fn seen(&self) -> Vec<RequestSpec> {
            self.seen.lock().expect("锁未被毒化").clone()
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            self.seen.lock().expect("锁未被毒化").push(spec.clone());
            let responses = self.responses.lock().expect("锁未被毒化");
            for (fragment, response) in responses.iter() {
                if spec.url.contains(fragment.as_str()) {
                    return Ok(response.clone());
                }
            }
            Ok(Response {
                status: self.default_status,
                headers: HeaderMap::new(),
                body_bytes: Vec::new(),
                detected_charset: None,
            })
        }
    }

    const SEARCH_HTML: &str = r#"<html><body><ul>
      <li class="item"><h3 class="name"><a href="/book/1">斗破苍穹</a></h3>
        <p class="author">天蚕土豆</p><p class="intro">简介一</p><img src="/c/1.jpg"></li>
      <li class="item"><h3 class="name"><a href="/book/2">完美世界</a></h3>
        <p class="author">辰东</p><p class="intro">简介二</p><img src="/c/2.jpg"></li>
    </ul></body></html>"#;

    fn sample_source_json() -> &'static str {
        r#"{
          "bookSourceName": "示例源",
          "bookSourceUrl": "https://example.com",
          "searchUrl": "/search?q={{key}}",
          "ruleSearch": "{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"author\":\"@css:.author@text\",\"intro\":\"@css:.intro@text\",\"coverUrl\":\"@css:img@src\",\"bookUrl\":\"@css:.name>a@href\"}"
        }"#
    }

    /// 跑一次 CLI。
    ///
    /// ## 为什么强制注入 `data_dir`
    ///
    /// 未指定 `--data-dir` 时注册表会落到**工作目录**的 `.tome/`——测试会污染
    /// 工作树（本测试最初就踩了这个坑：跑完一遍测试后仓库里冒出个 `.tome/`）。
    /// 故除非参数里显式给了 `--data-dir`，一律重定向到临时目录。
    fn run_args(items: &[&str], host: &dyn NetHost, stdin: &str) -> CommandOutcome {
        let parsed = parse(
            &items
                .iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<String>>(),
        );
        match parsed {
            ParseOutcome::Run {
                mut options,
                command,
            } => {
                if options.data_dir.is_none() {
                    let dir = tempfile::tempdir().expect("应能建临时数据目录");
                    options.data_dir = Some(dir.path().to_path_buf());
                    // 目录必须活到 run 结束：故把 guard 绑在局部变量上，
                    // 而不是 `.into_path()`（那会泄漏临时目录）。
                    let mut reader = std::io::Cursor::new(stdin.as_bytes().to_vec());
                    let outcome = run(&options, &command, host, &mut reader);
                    drop(dir);
                    return outcome;
                }
                let mut reader = std::io::Cursor::new(stdin.as_bytes().to_vec());
                run(&options, &command, host, &mut reader)
            }
            other => panic!("参数应解析成功，实际 {other:?}"),
        }
    }

    // ---------- source import ----------

    #[test]
    fn 导入报告_json_输出() {
        let host = FakeHost::new(vec![]);
        let outcome = run_args(
            &["source", "import", "--stdin", "--format", "json"],
            &host,
            sample_source_json(),
        );
        assert_eq!(outcome.exit_code, 0, "stderr={}", outcome.stderr);

        let parsed: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("输出应是合法 JSON");
        assert_eq!(parsed["added"], 1);
        assert_eq!(parsed["total"], 1);
        assert_eq!(parsed["source_ids"][0], "https://example.com");
    }

    #[test]
    fn 导入报告_human_输出() {
        let host = FakeHost::new(vec![]);
        let outcome = run_args(
            &["source", "import", "--stdin", "--format", "human"],
            &host,
            sample_source_json(),
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.stdout.contains("新增 1"), "{}", outcome.stdout);
        assert!(outcome.stdout.contains("示例源"));
    }

    #[test]
    fn 导入非法_json_给出退出码_1() {
        let host = FakeHost::new(vec![]);
        let outcome = run_args(&["source", "import", "--stdin"], &host, "{ 不是 JSON");
        assert_eq!(outcome.exit_code, 1, "业务错误应是 1");
        assert!(outcome.stderr.contains("error."), "stderr 应含分类标识");
        assert!(outcome.stdout.is_empty());
    }

    #[test]
    fn 导入缺渠道给出退出码_1() {
        let host = FakeHost::new(vec![]);
        let outcome = run_args(&["source", "import"], &host, "");
        assert_eq!(outcome.exit_code, 1);
        assert!(outcome.stderr.contains("--url") || outcome.stderr.contains("--stdin"));
    }

    #[test]
    fn 导入不存在的文件给出退出码_1() {
        let host = FakeHost::new(vec![]);
        let outcome = run_args(&["source", "import", "/definitely/missing.json"], &host, "");
        assert_eq!(outcome.exit_code, 1);
        assert!(outcome.stderr.contains("error.configuration.io"));
    }

    #[test]
    fn 导入_url_渠道经宿主拉取() {
        let host = FakeHost::new(vec![("src.json", sample_source_json())]);
        let outcome = run_args(
            &[
                "source",
                "import",
                "--url",
                "https://raw.example.com/src.json",
            ],
            &host,
            "",
        );
        assert_eq!(outcome.exit_code, 0, "stderr={}", outcome.stderr);
        let seen = host.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].url, "https://raw.example.com/src.json");
    }

    #[test]
    fn 导入对非_2xx_给出退出码_1() {
        let mut host = FakeHost::new(vec![]);
        host.default_status = 404;
        let outcome = run_args(
            &[
                "source",
                "import",
                "--url",
                "https://raw.example.com/missing.json",
            ],
            &host,
            "",
        );
        assert_eq!(outcome.exit_code, 1);
    }

    // ---------- search ----------

    /// 把书源样本写到临时文件，返回路径与清理守卫。
    fn write_source_file() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("应能建临时目录");
        let path = dir.path().join("source.json");
        std::fs::write(&path, sample_source_json()).expect("应能写文件");
        (dir, path)
    }

    #[test]
    fn 搜索输出结构化_json() {
        let host = FakeHost::new(vec![("/search", SEARCH_HTML)]);
        let (_guard, path) = write_source_file();
        let outcome = run_args(
            &[
                "search",
                "--source-file",
                path.to_str().expect("路径应为 UTF-8"),
                "斗破",
                "--format",
                "json",
            ],
            &host,
            "",
        );
        assert_eq!(outcome.exit_code, 0, "stderr={}", outcome.stderr);

        let parsed: serde_json::Value =
            serde_json::from_str(&outcome.stdout).expect("输出应是 JSON 数组");
        let books = parsed.as_array().expect("应是数组");
        assert_eq!(books.len(), 2);

        // 契约 cli.md 定义的字段名
        assert_eq!(books[0]["source"], "https://example.com");
        assert_eq!(books[0]["name"], "斗破苍穹");
        assert_eq!(books[0]["author"], "天蚕土豆");
        assert_eq!(books[0]["cover"], "/c/1.jpg");
        assert_eq!(books[0]["intro"], "简介一");
        assert_eq!(books[0]["book_url"], "https://example.com/book/1");
    }

    #[test]
    fn 搜索的关键词被编码进请求() {
        let host = FakeHost::new(vec![("/search", SEARCH_HTML)]);
        let (_guard, path) = write_source_file();
        let outcome = run_args(
            &[
                "search",
                "--source-file",
                path.to_str().unwrap(),
                "斗破 苍穹",
            ],
            &host,
            "",
        );
        assert_eq!(outcome.exit_code, 0);
        let seen = host.seen();
        assert!(
            seen[0].url.contains("%E6%96%97") && !seen[0].url.contains(' '),
            "关键词应被编码：{}",
            seen[0].url
        );
    }

    #[test]
    fn 搜索_human_输出() {
        let host = FakeHost::new(vec![("/search", SEARCH_HTML)]);
        let (_guard, path) = write_source_file();
        let outcome = run_args(
            &[
                "search",
                "--source-file",
                path.to_str().unwrap(),
                "斗破",
                "--format",
                "human",
            ],
            &host,
            "",
        );
        assert_eq!(outcome.exit_code, 0);
        assert!(outcome.stdout.contains("斗破苍穹"), "{}", outcome.stdout);
        assert!(outcome.stdout.contains("天蚕土豆"));
    }

    #[test]
    fn 搜索缺书源文件给出退出码_1() {
        let host = FakeHost::new(vec![]);
        let outcome = run_args(
            &["search", "--source-file", "/missing.json", "k"],
            &host,
            "",
        );
        assert_eq!(outcome.exit_code, 1);
    }

    #[test]
    fn 搜索未导入的_id_报源缺失() {
        // 按 id 搜索走**注册表**（契约流程）。这里用一个指向空数据目录的
        // 选项，故注册表为空 → 应报"源不存在"并提示先导入。
        let host = FakeHost::new(vec![]);
        let dir = tempfile::tempdir().expect("应能建临时目录");
        let options = GlobalOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..GlobalOptions::default()
        };
        let parsed = parse(&[
            "search".to_string(),
            "https://example.com".to_string(),
            "k".to_string(),
        ]);
        let ParseOutcome::Run { command, .. } = parsed else {
            panic!("参数应解析成功");
        };
        let mut reader = std::io::Cursor::new(Vec::new());
        let outcome = run(&options, &command, &host, &mut reader);

        assert_eq!(outcome.exit_code, 1, "源不存在是业务错误");
        assert!(
            outcome.stderr.contains("error.rule.source.missing"),
            "应给出源缺失分类：{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.contains("source import"),
            "应提示正确的下一步（先导入）：{}",
            outcome.stderr
        );
    }

    #[test]
    fn 搜索的业务错误输出到stderr而非stdout() {
        // 书源规则坏掉
        let bad_source = r#"{
          "bookSourceName": "坏源",
          "bookSourceUrl": "https://bad.com",
          "searchUrl": "/s?q={{key}}",
          "ruleSearch": "{\"bookList\":\"div >\"}"
        }"#;
        let dir = tempfile::tempdir().expect("应能建目录");
        let path = dir.path().join("bad.json");
        std::fs::write(&path, bad_source).expect("应能写");

        let host = FakeHost::new(vec![]);
        let outcome = run_args(
            &["search", "--source-file", path.to_str().unwrap(), "k"],
            &host,
            "",
        );
        assert_eq!(outcome.exit_code, 1);
        assert!(
            outcome.stderr.contains("error."),
            "stderr={}",
            outcome.stderr
        );
    }

    // ---------- 输出的确定性（T029 的前提） ----------

    #[test]
    fn 同一输入产出逐字节相同的输出() {
        // T029 的验收项是"录制→离线重放输出逐字节一致"，
        // 而它的前提是"同一份响应产出同一份输出"。
        let (_guard, path) = write_source_file();
        let source_path = path.to_str().unwrap().to_string();

        let host_a = FakeHost::new(vec![("/search", SEARCH_HTML)]);
        let first = run_args(
            &[
                "search",
                "--source-file",
                &source_path,
                "斗破",
                "--format",
                "json",
            ],
            &host_a,
            "",
        );

        let host_b = FakeHost::new(vec![("/search", SEARCH_HTML)]);
        let second = run_args(
            &[
                "search",
                "--source-file",
                &source_path,
                "斗破",
                "--format",
                "json",
            ],
            &host_b,
            "",
        );

        assert_eq!(first.stdout, second.stdout, "同一输入应产出同一输出");
        assert_eq!(first.exit_code, second.exit_code);
    }

    #[test]
    fn 输出末尾有换行() {
        // 末尾换行让 `diff` 与 `echo` 的行为符合预期（便于脚本比对）
        let host = FakeHost::new(vec![("/search", SEARCH_HTML)]);
        let (_guard, path) = write_source_file();
        let outcome = run_args(
            &["search", "--source-file", path.to_str().unwrap(), "k"],
            &host,
            "",
        );
        assert!(outcome.stdout.ends_with('\n'));
    }

    #[test]
    fn json_输出可被严格解析() {
        let host = FakeHost::new(vec![("/search", SEARCH_HTML)]);
        let (_guard, path) = write_source_file();
        let outcome = run_args(
            &["search", "--source-file", path.to_str().unwrap(), "k"],
            &host,
            "",
        );
        // 严格模式（拒绝多余内容）也能解析 → 输出里没有日志噪音
        let mut deserializer = serde_json::Deserializer::from_str(&outcome.stdout);
        let _: serde_json::Value =
            serde::Deserialize::deserialize(&mut deserializer).expect("应能解析");
        deserializer.end().expect("不应有尾随内容");
    }

    // ---------- 执行器构造（T029 的接线） ----------

    #[test]
    fn 三种网络模式都能构造执行器() {
        for mode in [
            NetworkMode::Live,
            NetworkMode::Record(std::path::PathBuf::from("target/tmp/rec")),
            NetworkMode::Replay {
                root: std::path::PathBuf::from("target/tmp/rep"),
                offline: true,
            },
            NetworkMode::Replay {
                root: std::path::PathBuf::from("target/tmp/rep2"),
                offline: false,
            },
        ] {
            let executor = build_executor(&mode, None);
            assert!(executor.is_ok(), "{mode:?} 应能构造执行器");
        }
    }

    #[test]
    fn 离线模式映射到绝不触网的执行模式() {
        let executor = build_executor(
            &NetworkMode::Replay {
                root: std::path::PathBuf::from("target/tmp/x"),
                offline: true,
            },
            None,
        )
        .expect("应能构造");
        assert_eq!(
            executor.replay_mode(),
            Some(tome_net::ReplayMode::Replay),
            "离线模式必须是绝不触网的那个"
        );
    }

    #[test]
    fn 录制模式映射到录制执行模式() {
        let executor = build_executor(
            &NetworkMode::Record(std::path::PathBuf::from("target/tmp/rec2")),
            None,
        )
        .expect("应能构造");
        assert_eq!(executor.replay_mode(), Some(tome_net::ReplayMode::Record));
    }

    #[test]
    fn live_模式不带回放目录() {
        let executor = build_executor(&NetworkMode::Live, None).expect("应能构造");
        assert_eq!(executor.replay_mode(), None);
    }

    #[test]
    fn 失败结果可复用构造() {
        let error = SourceError::rule("bad");
        let outcome = failure_for(&error);
        assert_eq!(outcome.exit_code, 1);
        assert!(outcome.stderr.starts_with("error."));
    }
}
