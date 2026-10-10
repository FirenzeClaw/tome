//! [DONE] CLI 参数解析（tasks.md T028/T029 / `contracts/cli.md`）
//!
//! # 为什么手写解析而不引第三方库
//!
//! `contracts/cli.md` 定义的参数面很小（十来个选项），而引 `clap` 会带来
//! 编译期与体积成本——体积纪律（宪法「附加约束」）把它列为需要权衡的事。
//! 手写的代价是要自己保证"用法错误 → 退出码 2"，故本模块的每条解析分支
//! 都显式返回 [`ParseOutcome::Usage`]，由调用方转成对应的退出码。
//!
//! # 退出码（`contracts/cli.md` 的全局约定）
//!
//! | 码 | 含义 |
//! |---|---|
//! | 0 | 成功 |
//! | 1 | 业务错误（源失效/规则错） |
//! | 2 | 用法错误 |
//! | 3 | 不支持能力 |
//!
//! 退出码的映射在 [`crate::exit_code_for`]——它只依赖 [`ErrorKind`]，
//! 故"哪种错误对应哪个码"是**单一处**的定义。

use std::path::PathBuf;

use tome_core::{ErrorCapability, ErrorKind};

/// 输出格式（`contracts/cli.md`：默认 JSON，可选 human）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    /// 结构化 JSON（可 diff，默认）
    #[default]
    Json,
    /// 人类可读
    Human,
}

/// 网络录制/回放模式（`--record` / `--replay` / `--offline` 的接线结果）。
///
/// ## 与 `tome_net::ReplayMode` 的关系
///
/// 本枚举是 CLI 层面的**意图**，`tome_net::ReplayMode` 是执行层面的**模式**。
/// 分开是为了让 CLI 的三个开关（`--record` / `--replay` / `--offline`）
/// 的组合语义在一处确定，而不是散在各个 `if` 里。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NetworkMode {
    /// 真实联网，不录制（默认）
    #[default]
    Live,
    /// 录制到目录（可同时真实联网）
    Record(PathBuf),
    /// 从目录回放；`offline` 为真时**绝不**触网，未命中即失败
    Replay {
        /// 语料目录
        root: PathBuf,
        /// 是否完全离线（`--offline`）
        offline: bool,
    },
}

impl NetworkMode {
    /// 对应的执行层模式与语料目录。
    pub fn to_executor_mode(&self) -> Option<(PathBuf, tome_net::ReplayMode)> {
        match self {
            Self::Live => None,
            Self::Record(root) => Some((root.clone(), tome_net::ReplayMode::Record)),
            // `--replay` 不带 `--offline` 时用 `RecordOnce`：命中回放、未命中补抓。
            // 带 `--offline` 时用 `Replay`：未命中即失败——那才是"完全离线"的语义，
            // 也是 T029 验收项"录制后可完全离线重放"的判据。
            Self::Replay { root, offline } => Some((
                root.clone(),
                if *offline {
                    tome_net::ReplayMode::Replay
                } else {
                    tome_net::ReplayMode::RecordOnce
                },
            )),
        }
    }
}

/// 解析出的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `source import <file|--url <u>|--stdin>`
    SourceImport {
        /// 文件渠道（位置参数）
        file: Option<PathBuf>,
        /// URL 渠道
        url: Option<String>,
        /// 是否从标准输入读（第三渠道）
        stdin: bool,
    },
    /// `search <source-id> <keyword>`（`--url-of-source` 可换成临时源文件）
    Search {
        /// 书源 id（与 `--source-file` 二选一）
        source_id: Option<String>,
        /// 临时书源文件（S1-1 阶段书源尚未落库，故支持直接给文件）
        source_file: Option<PathBuf>,
        /// 搜索关键词
        keyword: String,
    },
    /// `run <source-id> --action <search|info|toc|content> [--input <s>]`
    ///
    /// `contracts/cli.md` 定义的通用执行入口。S1-2 阶段实现 `search`
    /// （与 `search` 子命令等价，但走 `--action` 形态以便脚本化），
    /// 其余动作属后续切片（它们的规则求值路径相同，但需要详情/目录/
    /// 正文的 DTO 映射，那在 S1-1 未落地）。
    Run {
        /// 书源 id
        source_id: String,
        /// 动作（`search` / `info` / `toc` / `content`）
        action: String,
        /// 动作的输入（`search` 时是关键词）
        input: Option<String>,
    },
    /// `info <source-id> <book-url>`（`contracts/cli.md` 的详情动作）
    Info {
        /// 书源 id
        source_id: String,
        /// 书籍详情页地址
        book_url: String,
    },
    /// `toc <source-id> <book-url>`（`contracts/cli.md` 的目录动作）
    Toc {
        /// 书源 id
        source_id: String,
        /// 书籍详情页地址（目录地址从详情页取）
        book_url: String,
    },
    /// `content <source-id> <chapter-url>`（`contracts/cli.md` 的正文动作）
    Content {
        /// 书源 id
        source_id: String,
        /// 章节地址
        chapter_url: String,
        /// 是否跳过净化（`--no-purify`）
        no_purify: bool,
    },
    /// `source list [--group <g>] [--enabled]`
    SourceList {
        /// 限定分组名
        group: Option<String>,
        /// 只列启用的
        enabled_only: bool,
    },
    /// `source check <id|--all>`
    SourceCheck {
        /// 单个源 id；`None` 且 `all` 为真时检查全部
        source_id: Option<String>,
        /// 是否检查全部启用的源
        all: bool,
    },
    /// `verify --corpus <dir> [--report <file>]`（`contracts/cli.md`）
    ///
    /// 跑合成语料差分：逐条比对引擎输出与期望，输出一致率与逐条 diff。
    /// 对应 SC-002(a)：合成用例通过率 ≥95%。
    Verify {
        /// 语料目录
        corpus: PathBuf,
        /// 报告输出文件（`None` 表示写 stdout）
        report: Option<PathBuf>,
    },
}

/// 全局选项。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlobalOptions {
    /// 输出格式
    pub format: OutputFormat,
    /// 网络模式
    pub network: NetworkMode,
    /// 数据目录
    pub data_dir: Option<PathBuf>,
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseOutcome {
    /// 解析成功
    Run {
        /// 全局选项
        options: GlobalOptions,
        /// 要执行的命令
        command: Command,
    },
    /// 用法错误（退出码 2），附带可读说明
    Usage(String),
    /// 请求帮助（退出码 0）
    Help(String),
}

/// 解析命令行参数（**不含**程序名）。
///
/// [DONE] T028/T029 的入口。参数解析的全部规则集中在这里，
/// 便于让"用法错误 → 退出码 2"这条契约只需在一处保证。
pub fn parse(args: &[String]) -> ParseOutcome {
    let mut options = GlobalOptions::default();
    let mut positionals: Vec<String> = Vec::new();
    let mut record: Option<PathBuf> = None;
    let mut replay: Option<PathBuf> = None;
    let mut offline = false;
    let mut url: Option<String> = None;
    let mut stdin = false;
    let mut source_file: Option<PathBuf> = None;
    let mut action: Option<String> = None;
    let mut input: Option<String> = None;
    let mut corpus: Option<PathBuf> = None;
    let mut report: Option<PathBuf> = None;
    let mut group: Option<String> = None;
    let mut enabled_only = false;
    let mut check_all = false;
    let mut no_purify = false;

    // ## 迭代协议：每个分支返回"消费了几个参数"，循环统一推进
    //
    // 首版让每个分支自己 `index += n`，同时循环末尾又 `+= 1`——
    // 于是每个选项都多吃掉一个参数（`--format json` 会把 `json` 当位置参数）。
    // 改为"分支只报消费数、循环只推进一次"，参数消费就只有一个推进点。
    let mut index = 0usize;
    while index < args.len() {
        let arg = args[index].as_str();
        let consumed = match arg {
            "-h" | "--help" => return ParseOutcome::Help(help_text()),
            "--format" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.as_str(),
                    None => return ParseOutcome::Usage("--format 需要一个值".to_string()),
                };
                options.format = match value {
                    "json" => OutputFormat::Json,
                    "human" => OutputFormat::Human,
                    other => {
                        return ParseOutcome::Usage(format!(
                            "--format 只接受 json 或 human，实际 `{other}`"
                        ));
                    }
                };
                2
            }
            "--record" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--record 需要一个目录".to_string()),
                };
                record = Some(PathBuf::from(value));
                2
            }
            "--replay" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--replay 需要一个目录".to_string()),
                };
                replay = Some(PathBuf::from(value));
                2
            }
            "--offline" => {
                offline = true;
                1
            }
            "--data-dir" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--data-dir 需要一个路径".to_string()),
                };
                options.data_dir = Some(PathBuf::from(value));
                2
            }
            "--url" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--url 需要一个值".to_string()),
                };
                url = Some(value.clone());
                2
            }
            "--stdin" => {
                stdin = true;
                1
            }
            // `source list --group <名>`：按**分组名**过滤（不是 id）
            //
            // 用名而非 id：用户看到的是分组名，而 id 是数据库分配的、
            // 用户看不到。让人从界面上抄一个 id 来过滤是别扭的。
            "--group" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--group 需要一个分组名".to_string()),
                };
                group = Some(value.clone());
                2
            }
            "--enabled" => {
                enabled_only = true;
                1
            }
            // `source check --all`：检查全部启用的源
            "--all" => {
                check_all = true;
                1
            }
            // `content --no-purify`：看原始正文（调试净化规则用）
            "--no-purify" => {
                no_purify = true;
                1
            }
            "--action" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.as_str(),
                    None => return ParseOutcome::Usage("--action 需要一个值".to_string()),
                };
                action = Some(value.to_string());
                2
            }
            "--input" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.as_str(),
                    None => return ParseOutcome::Usage("--input 需要一个值".to_string()),
                };
                input = Some(value.to_string());
                2
            }
            "--source-file" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--source-file 需要一个路径".to_string()),
                };
                source_file = Some(PathBuf::from(value));
                2
            }
            "--corpus" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--corpus 需要一个目录".to_string()),
                };
                corpus = Some(PathBuf::from(value));
                2
            }
            "--report" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--report 需要一个路径".to_string()),
                };
                report = Some(PathBuf::from(value));
                2
            }
            // 未知的 `--xxx` 报用法错
            other if other.starts_with("--") => {
                return ParseOutcome::Usage(format!("未知选项 `{other}`"));
            }
            other => {
                positionals.push(other.to_string());
                1
            }
        };
        index += consumed;
    }

    // 组合 `--record`/`--replay`/`--offline` 的语义。
    //
    // `--offline` 单独出现（无 `--replay`）是用法错误：完全离线却没有语料源，
    // 结果是"所有请求都失败"——那不该被当成合法的调用方式。
    // `--record` 与 `--replay` 同时给也是用法错误：一个要写、一个要读，
    // 同一份目录同时读写会让语料自相矛盾。
    options.network = match (record, replay, offline) {
        (Some(_), Some(_), _) => {
            return ParseOutcome::Usage(
                "--record 与 --replay 不能同时使用（一个写一个读，同一目录会自相矛盾）".to_string(),
            );
        }
        (Some(root), None, _) => NetworkMode::Record(root),
        (None, Some(root), offline) => NetworkMode::Replay { root, offline },
        (None, None, true) => {
            return ParseOutcome::Usage(
                "--offline 需与 --replay 同用（没有语料源时完全离线会让所有请求失败）".to_string(),
            );
        }
        (None, None, false) => NetworkMode::Live,
    };

    // 命令
    let Some(subcommand) = positionals.first().map(String::as_str) else {
        return ParseOutcome::Usage(
            "缺少子命令（可用：source import|list|check / search / info / toc / content / run / verify）"
                .to_string(),
        );
    };

    let rest = &positionals[1..];
    let command = match subcommand {
        "source" => {
            let Some(action) = rest.first().map(String::as_str) else {
                return ParseOutcome::Usage("`source` 需要一个动作（import）".to_string());
            };
            match action {
                "import" => Command::SourceImport {
                    file: rest.get(1).map(PathBuf::from),
                    url,
                    stdin,
                },
                // `source list [--group <g>] [--enabled]`
                "list" => Command::SourceList {
                    group: group.clone(),
                    enabled_only,
                },
                // `source check <id|--all>`
                "check" => Command::SourceCheck {
                    source_id: rest.get(1).cloned(),
                    all: check_all,
                },
                other => {
                    return ParseOutcome::Usage(format!(
                        "未知的 source 动作 `{other}`（可用：import / list / check）"
                    ));
                }
            }
        }
        "search" => {
            // 位置参数可为：`<source-id> <keyword>` 或 `--source-file <f> <keyword>`
            match (rest.first(), source_file) {
                (Some(keyword), Some(path)) => Command::Search {
                    source_id: None,
                    source_file: Some(path),
                    keyword: keyword.clone(),
                },
                (Some(source_id), None) => {
                    let Some(keyword) = rest.get(1) else {
                        return ParseOutcome::Usage(
                            "search 需要 <source-id> <keyword>".to_string(),
                        );
                    };
                    Command::Search {
                        source_id: Some(source_id.clone()),
                        source_file: None,
                        keyword: keyword.clone(),
                    }
                }
                (None, Some(path)) => Command::Search {
                    source_id: None,
                    source_file: Some(path),
                    // 关键词可由 --url 之外的渠道给？此处要求显式关键词
                    keyword: match url {
                        Some(keyword) => keyword,
                        None => {
                            return ParseOutcome::Usage(
                                "search 需要关键词（`search --source-file <f> <keyword>`）"
                                    .to_string(),
                            );
                        }
                    },
                },
                (None, None) => {
                    return ParseOutcome::Usage(
                        "search 需要 <source-id> <keyword> 或 --source-file <f> <keyword>"
                            .to_string(),
                    );
                }
            }
        }
        "run" => {
            let Some(source_id) = rest.first() else {
                return ParseOutcome::Usage(
                    "run 需要 <source-id>（用法：run <source-id> --action search --input <关键词>）"
                        .to_string(),
                );
            };
            let Some(action) = action else {
                return ParseOutcome::Usage(
                    "run 需要 --action <search|info|toc|content>".to_string(),
                );
            };
            Command::Run {
                source_id: source_id.clone(),
                action,
                input,
            }
        }
        // `info <source-id> <book-url>`
        "info" => {
            let (Some(source_id), Some(book_url)) = (rest.first(), rest.get(1)) else {
                return ParseOutcome::Usage("info 需要 <source-id> <book-url>".to_string());
            };
            Command::Info {
                source_id: source_id.clone(),
                book_url: book_url.clone(),
            }
        }
        // `toc <source-id> <book-url>`
        "toc" => {
            let (Some(source_id), Some(book_url)) = (rest.first(), rest.get(1)) else {
                return ParseOutcome::Usage("toc 需要 <source-id> <book-url>".to_string());
            };
            Command::Toc {
                source_id: source_id.clone(),
                book_url: book_url.clone(),
            }
        }
        // `content <source-id> <chapter-url> [--no-purify]`
        "content" => {
            let (Some(source_id), Some(chapter_url)) = (rest.first(), rest.get(1)) else {
                return ParseOutcome::Usage("content 需要 <source-id> <chapter-url>".to_string());
            };
            Command::Content {
                source_id: source_id.clone(),
                chapter_url: chapter_url.clone(),
                no_purify,
            }
        }
        "verify" => {
            // `verify` 需要 `--corpus <dir>`（`contracts/cli.md` 的唯一形态；
            // `--real-sources`/`--golden` 形态属 S1-6 的真实基线，本阶段不做）
            let Some(corpus) = corpus else {
                return ParseOutcome::Usage(
                    "verify 需要 --corpus <dir>（用法：verify --corpus tests/corpus/）".to_string(),
                );
            };
            Command::Verify { corpus, report }
        }
        other => {
            return ParseOutcome::Usage(format!(
                "未知子命令 `{other}`（已实现 source import|list|check / search / info / toc / content / run / verify）"
            ));
        }
    };

    ParseOutcome::Run { options, command }
}

/// 退出码映射（`contracts/cli.md` 的全局约定）。
///
/// [DONE] 错误分类 → 退出码的**唯一**落点。
///
/// ## 为什么"不支持能力"单独一个码（3）
///
/// 它不是失败，而是"此路不通"的可编程信号：脚本可以据此区分"源坏了"
/// 与"这个能力本产品做不到"，后者不该触发重试或告警。
pub fn exit_code_for(kind: &ErrorKind) -> i32 {
    match kind {
        // 0 保留给成功，故业务错误的码从 1 起
        ErrorKind::UnsupportedCapability { .. } => 3,
        // 其余都是"业务错误"：源失效、规则错、网络不通、内容坏、存储坏、配置错。
        // 契约只定义了 1/2/3，故不细分——细分的粒度在**错误分类**里（stderr 的
        // `ErrorKind` 标识），退出码只需让脚本能区分"该不该重试"这一档。
        ErrorKind::Network
        | ErrorKind::Proxy
        | ErrorKind::Rule
        | ErrorKind::Content
        | ErrorKind::Storage
        | ErrorKind::Configuration => 1,
    }
}

/// 构造"不支持能力"的错误（供 CLI 对未实现的子命令使用）。
pub fn unsupported(capability: &str) -> ErrorKind {
    ErrorKind::unsupported_capability(ErrorCapability::new(capability))
}

/// 帮助文本。
pub fn help_text() -> String {
    [
        "tome —— 书源引擎的 CLI 验证器",
        "",
        "用法：",
        "  tome source import <file> [--url <u>] [--stdin]   导入书源（三渠道）",
        "  tome source list [--group <g>] [--enabled]        列出书源（可按分组/启用过滤）",
        "  tome source check <id|--all>                       源可用性检查",
        "  tome search <source-id> <keyword>                  按书源搜索",
        "  tome search --source-file <f> <keyword>            用临时书源文件搜索",
        "  tome info <source-id> <book-url>                   书籍详情",
        "  tome toc <source-id> <book-url>                    目录",
        "  tome content <source-id> <chapter-url>             章节正文（净化后）",
        "  tome run <source-id> --action <a> [--input <s>]     通用执行（--action search）",
        "  tome verify --corpus <dir> [--report <file>]       跑合成语料差分（SC-002a）",
        "",
        "全局选项：",
        "  --format json|human    输出格式（默认 json）",
        "  --record <dir>         把请求/响应录进目录",
        "  --replay <dir>         从目录回放（未命中时补抓，除非加 --offline）",
        "  --offline              完全离线（须与 --replay 同用）",
        "  --data-dir <path>      数据目录",
        "  --corpus <dir>         合成语料目录（verify 用）",
        "  --report <file>        报告输出文件（verify 用，默认为 stdout）",
        "  -h, --help             显示本帮助",
        "",
        "退出码：0 成功 / 1 业务错误 / 2 用法错误 / 3 不支持能力",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    fn parse_ok(items: &[&str]) -> (GlobalOptions, Command) {
        match parse(&args(items)) {
            ParseOutcome::Run { options, command } => (options, command),
            other => panic!("应解析成功，实际 {other:?}"),
        }
    }

    fn parse_usage(items: &[&str]) -> String {
        match parse(&args(items)) {
            ParseOutcome::Usage(text) => text,
            other => panic!("应为用法错误，实际 {other:?}"),
        }
    }

    // ---------- 帮助 ----------

    #[test]
    fn help_被识别() {
        for flag in ["-h", "--help"] {
            match parse(&args(&[flag])) {
                ParseOutcome::Help(text) => assert!(text.contains("用法")),
                other => panic!("{flag} 应产出帮助，实际 {other:?}"),
            }
        }
    }

    // ---------- source import 三渠道 ----------

    #[test]
    fn 导入的文件渠道() {
        let (_, command) = parse_ok(&["source", "import", "src.json"]);
        assert_eq!(
            command,
            Command::SourceImport {
                file: Some(PathBuf::from("src.json")),
                url: None,
                stdin: false,
            }
        );
    }

    #[test]
    fn 导入的_url_渠道() {
        let (_, command) = parse_ok(&["source", "import", "--url", "https://x/src.json"]);
        assert_eq!(
            command,
            Command::SourceImport {
                file: None,
                url: Some("https://x/src.json".to_string()),
                stdin: false,
            }
        );
    }

    #[test]
    fn 导入的_stdin_渠道() {
        let (_, command) = parse_ok(&["source", "import", "--stdin"]);
        assert_eq!(
            command,
            Command::SourceImport {
                file: None,
                url: None,
                stdin: true,
            }
        );
    }

    #[test]
    fn 三渠道可同时给出() {
        let (_, command) = parse_ok(&[
            "source",
            "import",
            "a.json",
            "--url",
            "https://x",
            "--stdin",
        ]);
        match command {
            Command::SourceImport { file, url, stdin } => {
                assert!(file.is_some());
                assert!(url.is_some());
                assert!(stdin);
            }
            other => panic!("应为导入命令，实际 {other:?}"),
        }
    }

    #[test]
    fn 未知_source_动作报用法错() {
        // 用一个**确实不存在**的动作（`list`/`check` 在 T063-T075 已实现）
        let text = parse_usage(&["source", "不存在的动作"]);
        assert!(text.contains("不存在的动作"), "{text}");
        assert!(text.contains("import"), "应列出可用动作：{text}");
    }

    // ---------- 完整管线的命令（T065-T072 接线） ----------

    #[test]
    fn info_需要两个位置参数() {
        let (_, command) = parse_ok(&["info", "https://a.com", "https://a.com/b/1"]);
        match command {
            Command::Info {
                source_id,
                book_url,
            } => {
                assert_eq!(source_id, "https://a.com");
                assert_eq!(book_url, "https://a.com/b/1");
            }
            other => panic!("应是 Info：{other:?}"),
        }
    }

    #[test]
    fn info_缺参数报用法错() {
        let text = parse_usage(&["info", "https://a.com"]);
        assert!(text.contains("book-url"), "{text}");
    }

    #[test]
    fn toc_需要两个位置参数() {
        let (_, command) = parse_ok(&["toc", "https://a.com", "https://a.com/b/1"]);
        assert!(matches!(command, Command::Toc { .. }));
        let text = parse_usage(&["toc", "https://a.com"]);
        assert!(text.contains("book-url"), "{text}");
    }

    #[test]
    fn content_支持_no_purify() {
        let (_, command) = parse_ok(&["content", "https://a.com", "https://a.com/c/1"]);
        match command {
            Command::Content { no_purify, .. } => assert!(!no_purify, "默认应净化"),
            other => panic!("应是 Content：{other:?}"),
        }

        let (_, command) = parse_ok(&[
            "content",
            "https://a.com",
            "https://a.com/c/1",
            "--no-purify",
        ]);
        match command {
            Command::Content { no_purify, .. } => assert!(no_purify),
            other => panic!("应是 Content：{other:?}"),
        }
    }

    #[test]
    fn content_缺参数报用法错() {
        let text = parse_usage(&["content", "https://a.com"]);
        assert!(text.contains("chapter-url"), "{text}");
    }

    #[test]
    fn source_list_支持分组与仅启用() {
        let (_, command) = parse_ok(&["source", "list"]);
        match command {
            Command::SourceList {
                group,
                enabled_only,
            } => {
                assert_eq!(group, None);
                assert!(!enabled_only);
            }
            other => panic!("应是 SourceList：{other:?}"),
        }

        let (_, command) = parse_ok(&["source", "list", "--group", "玄幻", "--enabled"]);
        match command {
            Command::SourceList {
                group,
                enabled_only,
            } => {
                assert_eq!(group.as_deref(), Some("玄幻"));
                assert!(enabled_only);
            }
            other => panic!("应是 SourceList：{other:?}"),
        }
    }

    #[test]
    fn source_list_的_group_需要值() {
        let text = parse_usage(&["source", "list", "--group"]);
        assert!(text.contains("--group"), "{text}");
    }

    #[test]
    fn source_check_支持单源与_all() {
        let (_, command) = parse_ok(&["source", "check", "https://a.com"]);
        match command {
            Command::SourceCheck { source_id, all } => {
                assert_eq!(source_id.as_deref(), Some("https://a.com"));
                assert!(!all);
            }
            other => panic!("应是 SourceCheck：{other:?}"),
        }

        let (_, command) = parse_ok(&["source", "check", "--all"]);
        match command {
            Command::SourceCheck { source_id, all } => {
                assert_eq!(source_id, None);
                assert!(all);
            }
            other => panic!("应是 SourceCheck：{other:?}"),
        }
    }

    #[test]
    fn source_缺动作报用法错() {
        let text = parse_usage(&["source"]);
        assert!(text.contains("动作"), "{text}");
    }

    // ---------- search ----------

    #[test]
    fn 按书源_id_搜索() {
        let (_, command) = parse_ok(&["search", "https://x.com", "斗破"]);
        assert_eq!(
            command,
            Command::Search {
                source_id: Some("https://x.com".to_string()),
                source_file: None,
                keyword: "斗破".to_string(),
            }
        );
    }

    #[test]
    fn 用临时书源文件搜索() {
        let (_, command) = parse_ok(&["search", "--source-file", "s.json", "斗破"]);
        assert_eq!(
            command,
            Command::Search {
                source_id: None,
                source_file: Some(PathBuf::from("s.json")),
                keyword: "斗破".to_string(),
            }
        );
    }

    #[test]
    fn search_缺关键词报用法错() {
        let text = parse_usage(&["search", "https://x.com"]);
        assert!(text.contains("keyword"), "{text}");
    }

    #[test]
    fn 未知子命令报用法错() {
        // 用未实现的子命令（`contracts/cli.md` 定义了它，但本切片不实现）
        let text = parse_usage(&["local", "import", "x"]);
        assert!(text.contains("local"), "{text}");
        assert!(text.contains("source import"), "应提示可用命令：{text}");
    }

    // ---------- verify（SC-002a 的载体） ----------

    #[test]
    fn verify_接受语料目录() {
        let (_, command) = parse_ok(&["verify", "--corpus", "tests/corpus"]);
        assert_eq!(
            command,
            Command::Verify {
                corpus: PathBuf::from("tests/corpus"),
                report: None,
            }
        );
    }

    #[test]
    fn verify_可指定报告文件() {
        let (_, command) = parse_ok(&[
            "verify",
            "--corpus",
            "tests/corpus",
            "--report",
            "/tmp/report.json",
        ]);
        match command {
            Command::Verify { corpus, report } => {
                assert_eq!(corpus, PathBuf::from("tests/corpus"));
                assert_eq!(report, Some(PathBuf::from("/tmp/report.json")));
            }
            other => panic!("应为 verify 命令，实际 {other:?}"),
        }
    }

    #[test]
    fn verify_缺语料目录报用法错() {
        // `--corpus` 是 verify 的唯一必需参数——没它跑不了差分
        let text = parse_usage(&["verify"]);
        assert!(text.contains("--corpus"), "{text}");
    }

    #[test]
    fn 缺子命令报用法错() {
        assert!(parse_usage(&[]).contains("子命令"));
    }

    // ---------- 全局选项 ----------

    #[test]
    fn 输出格式可切换() {
        let (options, _) = parse_ok(&["--format", "human", "search", "s", "k"]);
        assert_eq!(options.format, OutputFormat::Human);
        let (options, _) = parse_ok(&["--format", "json", "search", "s", "k"]);
        assert_eq!(options.format, OutputFormat::Json);
    }

    #[test]
    fn 非法输出格式报用法错() {
        let text = parse_usage(&["--format", "yaml", "search", "s", "k"]);
        assert!(text.contains("yaml"), "{text}");
    }

    #[test]
    fn 缺值的选项报用法错() {
        for flag in [
            "--format",
            "--record",
            "--replay",
            "--data-dir",
            "--url",
            "--source-file",
            "--corpus",
            "--report",
        ] {
            let text = parse_usage(&[flag]);
            assert!(text.contains("需要一个"), "{flag} 应报缺值：{text}");
        }
    }

    #[test]
    fn 未知选项报用法错() {
        let text = parse_usage(&["--nope", "search", "s", "k"]);
        assert!(text.contains("--nope"), "{text}");
    }

    #[test]
    fn 数据目录可指定() {
        let (options, _) = parse_ok(&["--data-dir", "/tmp/data", "search", "s", "k"]);
        assert_eq!(options.data_dir, Some(PathBuf::from("/tmp/data")));
    }

    // ---------- 网络模式（T029 的开关组合） ----------

    #[test]
    fn 默认是真实联网() {
        let (options, _) = parse_ok(&["search", "s", "k"]);
        assert_eq!(options.network, NetworkMode::Live);
    }

    #[test]
    fn record_接线到录制模式() {
        let (options, _) = parse_ok(&["--record", "/tmp/cap", "search", "s", "k"]);
        assert_eq!(
            options.network,
            NetworkMode::Record(PathBuf::from("/tmp/cap"))
        );
        let (root, mode) = options
            .network
            .to_executor_mode()
            .expect("录制模式应有执行层映射");
        assert_eq!(root, PathBuf::from("/tmp/cap"));
        assert_eq!(mode, tome_net::ReplayMode::Record);
    }

    #[test]
    fn replay_不带offline时未命中会补抓() {
        // 这是"补空洞"的用法：已录好的回放、缺的真实抓取
        let (options, _) = parse_ok(&["--replay", "/tmp/cap", "search", "s", "k"]);
        let (_, mode) = options.network.to_executor_mode().expect("应有映射");
        assert_eq!(mode, tome_net::ReplayMode::RecordOnce);
    }

    #[test]
    fn replay_带offline时完全离线() {
        // T029 的验收项："录制后可完全离线重放"
        let (options, _) = parse_ok(&["--replay", "/tmp/cap", "--offline", "search", "s", "k"]);
        let (_, mode) = options.network.to_executor_mode().expect("应有映射");
        assert_eq!(
            mode,
            tome_net::ReplayMode::Replay,
            "带 --offline 时必须用绝不触网的模式"
        );
        assert!(mode.is_offline_only());
    }

    #[test]
    fn record与replay不能同用() {
        let text = parse_usage(&["--record", "/a", "--replay", "/b", "search", "s", "k"]);
        assert!(text.contains("不能同时"), "{text}");
    }

    #[test]
    fn 单独offline报用法错() {
        let text = parse_usage(&["--offline", "search", "s", "k"]);
        assert!(text.contains("--replay"), "{text}");
    }

    // ---------- 退出码 ----------

    #[test]
    fn 退出码按契约映射() {
        assert_eq!(exit_code_for(&ErrorKind::Network), 1);
        assert_eq!(exit_code_for(&ErrorKind::Proxy), 1);
        assert_eq!(exit_code_for(&ErrorKind::Rule), 1);
        assert_eq!(exit_code_for(&ErrorKind::Content), 1);
        assert_eq!(exit_code_for(&ErrorKind::Storage), 1);
        assert_eq!(exit_code_for(&ErrorKind::Configuration), 1);
        assert_eq!(
            exit_code_for(&unsupported("webView")),
            3,
            r#"不支持能力应单独一个码（脚本据此区分「源坏了」与「本产品做不到」）"#
        );
    }

    #[test]
    fn 退出码不含零() {
        for kind in [
            ErrorKind::Network,
            ErrorKind::Proxy,
            ErrorKind::Rule,
            ErrorKind::Content,
            ErrorKind::Storage,
            ErrorKind::Configuration,
            unsupported("x"),
        ] {
            assert_ne!(exit_code_for(&kind), 0, "{kind:?} 不该映射到 0");
        }
    }
}
