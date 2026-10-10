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

/// `layout` 子命令的参数（`contracts/cli.md` §2.1 的选项面）。
///
/// ## 两个正文来源
///
/// - `chapter`：契约定义的形式（`<book>#<index>`）。要求该书在书架里——
///   那是 S3 与 001 组合根的职责，S2-1 阶段排版层尚无书架接线。
/// - `text_file`：**本阶段的替代入口**，直接给一份正文文件。它的存在
///   让本切片可独立验证（Independent Test 的要求），但不改变 `chapter`
///   的契约地位。
///
/// ## 为什么选项收进结构体而不摊在枚举变体里
///
/// 十二个字段共同构成"一次 layout 调用的参数"——给它们一个名字比摊平
/// 清楚；且摊平会让该变体显著大于其它变体（clippy 的
/// `large_enum_variant` 会告警，而它的 Box 建议只是把问题挪走）。
///
/// `[WIP]` `chapter` 的接线待书架就绪。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LayoutArgs {
    /// `--chapter <book>#<index>`（书架形式，待接线）
    pub chapter: Option<String>,
    /// `--text <file>`（本阶段入口）
    pub text_file: Option<PathBuf>,
    /// `--mode paginated|scroll`（默认 paginated）
    pub mode: Option<String>,
    /// `--width <px>` 阅读区**可用**宽度
    pub width: Option<f32>,
    /// `--height <px>` 阅读区**可用**高度
    pub height: Option<f32>,
    /// `--font-size <px>`（14-30，默认 19）
    pub font_size: Option<f32>,
    /// `--line-height <x>`（1.75-2.2，默认 1.85）
    pub line_height: Option<f32>,
    /// `--paragraph-mode indent|gap`
    pub paragraph_mode: Option<String>,
    /// `--measure-em <n>`（默认 36，上限 42）
    pub measure_em: Option<f32>,
    /// `--gutter <px>`（默认 64）
    pub gutter: Option<f32>,
    /// `--spread auto|single|double`
    pub spread: Option<String>,
    /// `--anchor <char_offset>`
    pub anchor: Option<usize>,
}

/// 解析出的命令。
/// `[NOTE]` **不派生 `Eq`**：`layout` 的几何选项含 `f32`（宽高/字号/
/// 行高），而浮点没有全序——`Eq` 会让"NaN == NaN 为假"这类语义混进来。
/// `PartialEq` 足够（解析测试与命令比较都只用它）。
#[derive(Debug, Clone, PartialEq)]
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
    /// `local import <path> [--copy] [--library <dir>]`（`contracts/cli.md`）
    ///
    /// T076-T083 的本地书面（P7 合并）。实现在 `crate::local`。
    LocalImport {
        /// 要导入的本地文件
        path: PathBuf,
        /// FR-024：是否把文件复制到库目录（可选行为）
        copy: bool,
        /// `--copy` 的目标库根
        library: Option<PathBuf>,
    },
    /// `local toc <path>`（`contracts/cli.md`）——只输出目录识别结果。
    LocalToc {
        /// 要解析的本地文件
        path: PathBuf,
    },
    /// `local scan <dir> [--library <dir>...]`（`contracts/cli.md`）——库根扫描。
    LocalScan {
        /// 要扫描的库根目录
        directory: PathBuf,
        /// 额外的库根（多库根扫描，FR-023）
        roots: Vec<PathBuf>,
        /// `--copy` 用于 `local import`，此处仅为签名一致而占位（未使用）
        _unused: (),
    },
    /// `layout --chapter <book>#<n>` 或 `layout --text <file>`（`contracts/cli.md` §2.1）
    ///
    /// 排版与分页/滚动流计算。**不渲染**（纯计算，无需图形环境）。
    Layout(LayoutArgs),
    /// `verify --corpus <dir> [--report <file>]`（`contracts/cli.md`）
    ///
    /// 跑合成语料差分：逐条比对引擎输出与期望，输出一致率与逐条 diff。
    /// 对应 SC-002(a)：合成用例通过率 ≥95%。
    ///
    /// 同时输出 **FR-005b 覆盖率矩阵**（T085）。
    Verify {
        /// 语料目录
        corpus: PathBuf,
        /// 报告输出文件（`None` 表示写 stdout）
        report: Option<PathBuf>,
    },
    /// `verify --real-sources <dir> --golden <dir> [--report <file>]`（T086 / SC-002b）
    ///
    /// 真实书源端到端对照：跑真实书源抓取，与对照基线比对。
    /// 与 `--corpus` 的区别是它**真的发请求**（合成语料是纯计算），
    /// 故需要网络宿主。
    VerifyReal {
        /// 真实书源目录
        real_sources: PathBuf,
        /// 对照基线目录
        golden: PathBuf,
        /// 报告输出文件
        report: Option<PathBuf>,
        /// 是否把本次结果写为新基线（首轮基线落盘）
        write_baseline: bool,
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
/// `[NOTE]` 同 [`Command`]：不派生 `Eq`（含 `Command`，后者含 `f32`）。
///
/// ## 为什么在此豁免 `clippy::large_enum_variant`
///
/// `Run` 变体内联 `Command`（最大约 280 字节），而 `Usage`/`Help` 只有
/// 24 字节——clippy 因此告警。**不在本任务内改 Box**，理由：
///
/// 1. `ParseOutcome` 是**解析路径上的短命值**：`parse()` 产出后立刻在
///    `main` 里被解构消费，不做存储、不跨线程传递。按值大小的影响是
///    栈上一次拷贝，而 Box 会为此引入一次堆分配——**这个交换不划算**。
/// 2. Box 会把改动扩散到 5 处调用点（含两个既有的集成测试），
///    而收益仅是消除一个 lint。这属于"为 lint 而改结构"，不是改进。
///
/// `[NOTE]` 若日后 `Command` 继续膨胀（如再加几个带多字段的子命令），
/// **应当**重新评估——那时的 Box 是有意义的。判据是 `Command` 的最大
/// 变体尺寸，而非 lint 是否报警。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
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
    let mut real_sources: Option<PathBuf> = None;
    let mut golden: Option<PathBuf> = None;
    let mut write_baseline_flag = false;
    let mut report: Option<PathBuf> = None;
    // --- `layout` 子命令的选项（contracts/cli.md §2.1）---
    let mut chapter: Option<String> = None;
    let mut text_file: Option<PathBuf> = None;
    let mut layout_mode: Option<String> = None;
    let mut width: Option<f32> = None;
    let mut height: Option<f32> = None;
    let mut font_size: Option<f32> = None;
    let mut line_height: Option<f32> = None;
    let mut paragraph_mode: Option<String> = None;
    let mut measure_em: Option<f32> = None;
    let mut gutter: Option<f32> = None;
    let mut spread: Option<String> = None;
    let mut anchor_offset: Option<usize> = None;
    let mut group: Option<String> = None;
    let mut enabled_only = false;
    let mut check_all = false;
    let mut no_purify = false;
    // 本地书面（T076-T083，P7 合并）
    let mut copy = false;
    let mut library: Vec<PathBuf> = Vec::new();

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
            // --- layout 子命令的选项（contracts/cli.md §2.1）---
            "--chapter" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.clone(),
                    None => return ParseOutcome::Usage("--chapter 需要一个值".to_string()),
                };
                chapter = Some(value);
                2
            }
            "--text" => {
                // 排版验证需要一个**正文来源**。`--chapter <book>#<n>` 要求
                // 书在书架里（S3 与 001 组合根的职责）；S2-1 阶段排版层尚无
                // 书架接线，故提供 `--text <file>` 直接给正文，使本切片可
                // 独立验证（Independent Test 的要求）。
                let value = match args.get(index + 1) {
                    Some(value) => PathBuf::from(value),
                    None => return ParseOutcome::Usage("--text 需要一个文件路径".to_string()),
                };
                text_file = Some(value);
                2
            }
            "--mode" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.as_str(),
                    None => return ParseOutcome::Usage("--mode 需要一个值".to_string()),
                };
                if value != "paginated" && value != "scroll" {
                    return ParseOutcome::Usage(format!(
                        "--mode 只接受 paginated 或 scroll，实际 `{value}`"
                    ));
                }
                layout_mode = Some(value.to_string());
                2
            }
            "--width" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--width 需要一个数值".to_string()),
                };
                match value.parse::<f32>() {
                    Ok(parsed) if parsed > 0.0 => width = Some(parsed),
                    Ok(_) => return ParseOutcome::Usage("--width 必须为正数".to_string()),
                    Err(_) => {
                        return ParseOutcome::Usage(format!("--width 不是数值：`{value}`"));
                    }
                }
                2
            }
            "--height" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--height 需要一个数值".to_string()),
                };
                match value.parse::<f32>() {
                    Ok(parsed) if parsed > 0.0 => height = Some(parsed),
                    Ok(_) => return ParseOutcome::Usage("--height 必须为正数".to_string()),
                    Err(_) => {
                        return ParseOutcome::Usage(format!("--height 不是数值：`{value}`"));
                    }
                }
                2
            }
            "--font-size" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--font-size 需要一个数值".to_string()),
                };
                match value.parse::<f32>() {
                    // FR-013 的范围 14-30
                    Ok(parsed) if (14.0..=30.0).contains(&parsed) => font_size = Some(parsed),
                    Ok(parsed) => {
                        return ParseOutcome::Usage(format!(
                            "--font-size 须在 14-30 之间（FR-013），实际 {parsed}"
                        ));
                    }
                    Err(_) => {
                        return ParseOutcome::Usage(format!("--font-size 不是数值：`{value}`"));
                    }
                }
                2
            }
            "--line-height" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--line-height 需要一个数值".to_string()),
                };
                match value.parse::<f32>() {
                    // FR-013 的范围 1.75-2.2（下限是无障碍强制，裁决 OQ#19→A）
                    Ok(parsed) if (1.75..=2.2).contains(&parsed) => line_height = Some(parsed),
                    Ok(parsed) => {
                        return ParseOutcome::Usage(format!(
                            "--line-height 须在 1.75-2.2 之间（FR-013 / MASTER §2.3 强制下限），实际 {parsed}"
                        ));
                    }
                    Err(_) => {
                        return ParseOutcome::Usage(format!("--line-height 不是数值：`{value}`"));
                    }
                }
                2
            }
            "--paragraph-mode" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.as_str(),
                    None => {
                        return ParseOutcome::Usage("--paragraph-mode 需要一个值".to_string());
                    }
                };
                if value != "indent" && value != "gap" {
                    return ParseOutcome::Usage(format!(
                        "--paragraph-mode 只接受 indent 或 gap，实际 `{value}`"
                    ));
                }
                paragraph_mode = Some(value.to_string());
                2
            }
            "--measure-em" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--measure-em 需要一个数值".to_string()),
                };
                match value.parse::<f32>() {
                    // FR-013：目标 36em，上限 42em
                    Ok(parsed) if parsed > 0.0 && parsed <= 42.0 => measure_em = Some(parsed),
                    Ok(parsed) => {
                        return ParseOutcome::Usage(format!(
                            "--measure-em 须在 (0, 42] 之间（FR-013 上限 42em），实际 {parsed}"
                        ));
                    }
                    Err(_) => {
                        return ParseOutcome::Usage(format!("--measure-em 不是数值：`{value}`"));
                    }
                }
                2
            }
            "--gutter" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--gutter 需要一个数值".to_string()),
                };
                match value.parse::<f32>() {
                    Ok(parsed) if parsed >= 0.0 => gutter = Some(parsed),
                    Ok(_) => return ParseOutcome::Usage("--gutter 不能为负".to_string()),
                    Err(_) => {
                        return ParseOutcome::Usage(format!("--gutter 不是数值：`{value}`"));
                    }
                }
                2
            }
            "--spread" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.as_str(),
                    None => return ParseOutcome::Usage("--spread 需要一个值".to_string()),
                };
                if value != "auto" && value != "single" && value != "double" {
                    return ParseOutcome::Usage(format!(
                        "--spread 只接受 auto/single/double，实际 `{value}`"
                    ));
                }
                spread = Some(value.to_string());
                2
            }
            "--anchor" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--anchor 需要一个数值".to_string()),
                };
                match value.parse::<usize>() {
                    Ok(parsed) => anchor_offset = Some(parsed),
                    Err(_) => {
                        return ParseOutcome::Usage(format!("--anchor 不是非负整数：`{value}`"));
                    }
                }
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
            // T086：真实书源档（SC-002b）
            "--real-sources" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--real-sources 需要一个目录".to_string()),
                };
                real_sources = Some(PathBuf::from(value));
                2
            }
            "--golden" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--golden 需要一个目录".to_string()),
                };
                golden = Some(PathBuf::from(value));
                2
            }
            // 首轮基线落盘（T086）
            "--write-baseline" => {
                write_baseline_flag = true;
                1
            }
            // [P7-MERGE] 本地书：`--copy` 与 `--library`（可重复）
            "--copy" => {
                copy = true;
                1
            }
            "--library" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--library 需要一个路径".to_string()),
                };
                library.push(PathBuf::from(value));
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
            // `verify` 有两种形态（`contracts/cli.md`）：
            //   --corpus <dir>                            合成语料差分（SC-002a，纯计算）
            //   --real-sources <dir> --golden <dir>        真实书源对照（SC-002b，发请求）
            //
            // 两者互斥：同时给是用法错误（"你要跑哪一档"没有歧义余地，
            // 静默选一个会让脚本以为另一档也跑了）。
            if corpus.is_some() && real_sources.is_some() {
                return ParseOutcome::Usage(
                    "verify 的 --corpus 与 --real-sources 互斥（合成档与真实源档是两回事）"
                        .to_string(),
                );
            }

            if let Some(real_sources) = real_sources {
                let Some(golden) = golden else {
                    return ParseOutcome::Usage(
                        "verify --real-sources 需要 --golden <dir>（对照基线目录）".to_string(),
                    );
                };
                Command::VerifyReal {
                    real_sources,
                    golden,
                    report,
                    write_baseline: write_baseline_flag,
                }
            } else {
                let Some(corpus) = corpus else {
                    return ParseOutcome::Usage(
                        "verify 需要 --corpus <dir> 或 --real-sources <dir> --golden <dir>"
                            .to_string(),
                    );
                };
                Command::Verify { corpus, report }
            }
        }
        // [P7-MERGE] `local` 子命令（T076-T083）
        // `layout`（contracts/cli.md §2.1）
        "layout" => Command::Layout(LayoutArgs {
            chapter,
            text_file,
            mode: layout_mode,
            width,
            height,
            font_size,
            line_height,
            paragraph_mode,
            measure_em,
            gutter,
            spread,
            anchor: anchor_offset,
        }),
        "local" => {
            let Some(action) = rest.first().map(String::as_str) else {
                return ParseOutcome::Usage(
                    "local 需要一个动作（import / toc / scan）".to_string(),
                );
            };
            match action {
                "import" => {
                    let Some(path) = rest.get(1) else {
                        return ParseOutcome::Usage(
                            "local import 需要 <path>（用法：local import <file> [--copy --library <dir>]）"
                                .to_string(),
                        );
                    };
                    Command::LocalImport {
                        path: PathBuf::from(path),
                        copy,
                        library: library.first().cloned(),
                    }
                }
                "toc" => {
                    let Some(path) = rest.get(1) else {
                        return ParseOutcome::Usage(
                            "local toc 需要 <path>（用法：local toc <file>）".to_string(),
                        );
                    };
                    Command::LocalToc {
                        path: PathBuf::from(path),
                    }
                }
                "scan" => {
                    let Some(directory) = rest.get(1) else {
                        return ParseOutcome::Usage(
                            "local scan 需要 <dir>（用法：local scan <dir> [--library <dir>]）"
                                .to_string(),
                        );
                    };
                    Command::LocalScan {
                        directory: PathBuf::from(directory),
                        roots: library.clone(),
                        _unused: (),
                    }
                }
                other => {
                    return ParseOutcome::Usage(format!("未知的 local 动作 `{other}`"));
                }
            }
        }
        other => {
            return ParseOutcome::Usage(format!(
                "未知子命令 `{other}`（已实现 source import|list|check / search / info / toc / content / run / verify / local）"
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
        "  tome local import <path> [--copy] [--library <dir>] 导入本地书（TXT/EPUB/MOBI）",
        "  tome local toc <path>                              本地书目录识别",
        "  tome local scan <dir> [--library <dir>...]         库根扫描（多库根）",
        "  tome run <source-id> --action <a> [--input <s>]     通用执行（--action search）",
        "  tome layout --text <file> [几何选项]                排版一章（不渲染）",
        "  tome layout --chapter <book>#<n> [几何选项]         同上（书架形式，WIP）",
        "  tome verify --corpus <dir> [--report <file>]       跑合成语料差分（SC-002a）",
        "  tome verify --real-sources <dir> --golden <dir>     真实书源对照基线（SC-002b）",
        "              [--write-baseline] [--report <file>]",
        "",
        "全局选项：",
        "  --format json|human    输出格式（默认 json）",
        "  --record <dir>         把请求/响应录进目录",
        "  --replay <dir>         从目录回放（未命中时补抓，除非加 --offline）",
        "  --offline              完全离线（须与 --replay 同用）",
        "  --data-dir <path>      数据目录",
        "  --corpus <dir>         合成语料目录（verify 用）",
        "  --real-sources <dir>   真实书源目录（verify --real-sources 用）",
        "  --golden <dir>         对照基线目录（verify --real-sources 用）",
        "  --write-baseline       把本次结果写为新基线（首轮基线落盘用）",
        "  --report <file>        报告输出文件（verify 用，默认为 stdout）",
        "",
        "layout 选项（contracts/cli.md §2.1）：",
        "  --text <file>          正文文件（本阶段入口）",
        "  --chapter <book>#<n>   目标章节（书架形式，WIP）",
        "  --mode paginated|scroll  阅读形态（默认 paginated）",
        "  --width/--height <px>  阅读区**可用**宽高（不是窗口宽高）",
        "  --font-size <px>       阅读字号 14-30（默认 19）",
        "  --line-height <x>      行高 1.75-2.2（默认 1.85）",
        "  --paragraph-mode indent|gap  段首缩进（默认）或段间距",
        "  --measure-em <n>       版心目标 em（默认 36，上限 42）",
        "  --gutter <px>          双页中缝（默认 64）",
        "  --spread auto|single|double  单双页模式（默认 auto）",
        "  --anchor <offset>      报告该锚点落在哪一行",
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
        // 用未实现的子命令（`contracts/cli.md` 定义了它，但本切片不实现）。
        //
        // 注意：`local` 已在 T076-T083 落地，故这里改用 `data`——它仍在
        // `contracts/cli.md` 里被定义而尚未实现，是这条测试的好样本。
        let text = parse_usage(&["data", "export"]);
        assert!(text.contains("data"), "{text}");
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
