//! \[DONE] CLI 参数解析（tasks.md T028/T029 / `contracts/cli.md`）
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
    /// `--char <offset>`（`layout-anchor` 的必需选项，`contracts/cli.md` §2.2）
    pub anchor_char: Option<usize>,
    /// `--target-mode paginated|scroll`（`layout-anchor` 的目标形态，§2.2）
    pub target_mode: Option<String>,
}

/// `render` 子命令的参数（`contracts/cli.md` §2.3）。
///
/// 复用 [`LayoutArgs`] 的几何选项（离屏渲染要按同一套几何排版），
/// 另加渲染专有项：目标页/视口、输出路径、主题。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RenderArgs {
    /// 几何与正文来源（与 `layout` 完全同一套）
    pub layout: LayoutArgs,
    /// `--page <n>`（翻页模式的目标页）
    pub page: Option<u32>,
    /// `--viewport <n>`（滚动模式的目标视口）
    pub viewport_index: Option<u32>,
    /// `--out <path>`（输出 PNG）
    pub out: Option<PathBuf>,
    /// `--theme day|night`（默认 day）
    pub theme: Option<String>,
    /// `--reduced-motion`（关闭动效；不影响静态渲染结果，FR-032）
    pub reduced_motion: bool,
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
        /// 输出规则求值轨迹（FR-037，逐步中间值）
        trace: bool,
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
    /// `render --text <f> --page <n> --out <png>`（`contracts/cli.md` §2.3）
    ///
    /// 离屏渲染一页为图像。后端不可用时以退出码 3 + 结构化能力错误回应
    /// （FR-053：MUST NOT 伪装成功）。
    Render(Box<RenderArgs>),
    /// `fonts`（`contracts/cli.md` §2.4）
    ///
    /// 报告内嵌字体子集与回退链状态（FR-050/051、SC-017）。
    Fonts,
    /// `layout-anchor --text <file> --char <offset>`（`contracts/cli.md` §2.2）
    ///
    /// 位置换算与阅读形态切换：输出指定字符在给定几何下**两种形态**各自的
    /// 定位，并报告锚点值是否不变（验证 SC-004 / SC-005b / FR-022 / FR-023）。
    LayoutAnchor(LayoutArgs),
    /// `verify-layout --corpus <dir> [--report <file>]`（`contracts/cli.md` §2.5）
    ///
    /// 跑排版层合成语料的一致性比对，输出通过率与逐条差异
    /// （对齐 001 的 `verify` 形态与 spec §SC-001~SC-003）。
    VerifyLayout {
        /// 语料目录（含 `*.json` 用例）
        corpus: PathBuf,
        /// 报告输出文件（`None` 表示写 stdout）
        report: Option<PathBuf>,
    },
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

    // ---- T093：契约里定义但此前未落地的命令面 ----
    /// `run --rule <规则> --input <文本> --trace`（FR-037 的轨迹模式）
    ///
    /// # 为什么它不是 `Run` 的一个字段
    ///
    /// `Run` 的语义是"对**某个书源**执行某个动作"（需要 source-id）。
    /// 轨迹模式的语义是"对**一条规则**求值并展开过程"——它不需要书源，
    /// 也不需要动作。两者共用 `run` 这个入口词（契约如此），但**参数前提
    /// 完全不同**，塞进同一个变体会让每个字段都变成"看情况可空"。
    RuleTrace {
        /// 规则原文
        rule: String,
        /// 求值输入（HTML 或 JSON 文本）
        input: String,
    },

    /// `source group <源id...> --name <分组|->`（FR-001：分组与启停）
    ///
    /// `--name -` 表示**移出分组**（置空）。用 `-` 而非额外的 flag 形态：
    /// 与 shell 惯例一致（`-` 多表示"空/标准输入"），且少一个 flag。
    SourceGroup {
        /// 要归组的书源 id（可多个）
        source_ids: Vec<String>,
        /// 目标分组名；`-` 表示移出分组
        name: String,
    },
    /// `source enable|disable <源id...|--all>`（FR-001：启停）
    SourceEnable {
        /// 要启停的书源 id
        source_ids: Vec<String>,
        /// 是否作用于全部书源
        all: bool,
        /// `true` 启用、`false` 停用
        enabled: bool,
    },
    /// `source groups`（列出分组）
    SourceGroups,
    /// `keyword list`（FR-018：搜索关键词收藏与复用）
    KeywordList,
    /// `keyword add <词>`
    KeywordAdd {
        /// 关键词
        keyword: String,
    },
    /// `keyword remove <词>`
    KeywordRemove {
        /// 关键词
        keyword: String,
    },
    /// `data export [--output <file>]`（FR-033）
    DataExport {
        /// 输出文件；`None` 表示写 stdout
        output: Option<PathBuf>,
    },
    /// `data import <file>`（FR-033）
    DataImport {
        /// 备份文件
        file: PathBuf,
    },
    /// `data clear-cache <chapters|images>`（SC-017）
    DataClearCache {
        /// 要清理的缓存类别（`chapters` / `images`）
        kind: String,
    },
    /// `data delete [--yes]`（FR-035 / T091）
    ///
    /// # 为什么需要 `--yes`
    ///
    /// 这是**不可撤销**的完整删除。CLI 会被脚本调用，而脚本里一个空变量
    /// 就可能导致参数错位——无确认地执行会让用户丢光数据。故默认要求显式
    /// `--yes`，否则只打印"将要删除什么"并以**用法错误（2）**退出，
    /// 让脚本能区分"没执行"与"执行失败"。
    DataDelete {
        /// 确认执行
        yes: bool,
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
    /// 配置文件路径（`--config <path>`）
    ///
    /// `None` 时用数据目录下的 `config.toml`。
    ///
    /// # 为什么契约要求它（`contracts/cli.md` 全局约定）
    ///
    /// 它让"同一份数据 + 两套配置"可被对照运行（比如开了代理与没开代理），
    /// 而差分测试正需要这种能力——否则验配置影响只能改数据目录里的文件，
    /// 那会污染被对照的那一份。
    pub config: Option<PathBuf>,
    /// 界面语言（`--lang <zh|en>`，FR-044）
    ///
    /// # 为什么语言是 CLI 的全局选项而不是配置项
    ///
    /// 它要能**逐次调用**地切（CI 里跑同一个命令对比中英输出）。
    /// 只放配置里就得为每次切换改文件。
    pub language: crate::i18n::Language,
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
    /// 该命令在**本次构建中不存在**（能力在编译期缺席，退出码 3）。
    ///
    /// # 为什么它是一个独立的解析结果，而不是伪造一个 `Run`
    ///
    /// headless 构建（`--no-default-features`）里渲染面在编译期消失，
    /// 于是 `render` / `fonts` 这两个命令**根本不存在**——不是一个"会
    /// 失败的命令"，而是"没有这个命令"。
    ///
    /// 首版实现让 `parse` 返回 `Run { command: Command::Render/Fonts,
    /// options: GlobalOptions::default() }`，指望 `main` 走到 stub 去报能力
    /// 错误。那是**假短路**，两处都错：
    ///
    /// 1. `dispatch` 对 `Run` 无条件调 `execute` → `AppRoot::assemble`，
    ///    组装根照常装配、IO 照常发生。注释里"提到任何 IO 之前"是假话。
    /// 2. 为凑出那个 `Command` 只能填 `GlobalOptions::default()`——用户给的
    ///    `--data-dir` 等全局选项被**悄悄丢掉**，装配跑去打默认数据目录。
    ///    本机上默认目录可用，于是看起来一切正常；CI 上多个测试进程撞进
    ///    同一个默认库，才以 `database is locked` 现形。
    ///
    /// 独立变体让"命令不存在"成为解析器能表达的**一等结果**：`dispatch`
    /// 直接报能力错误 + 退 3，不经 `execute`、不建组合根。连 options 都
    /// 无从谈起——因为这次调用根本没走到需要它的地方。
    CapabilityUnavailable {
        /// 不可用能力的标识（如 `offscreenRendering`）
        capability: String,
        /// 命令名（用于诊断文案；如 `render`）
        command: String,
    },
}

/// 在 headless 构建里识别**渲染面**命令（`render` / `fonts`），供 `parse` 前置短路。
///
/// ## 覆盖范围是「渲染面」而非只有 `render`
///
/// 这两个命令同属渲染面：`render` 出自 `render` 模块、`fonts` 出自
/// `render::fonts`——`tome-reader` 的 `render` feature 一关，**两者同时在
/// 编译产物里消失**。故能力错误必须对两者同源同形态。
///
/// 函数名如实反映这一范围（原为 `headless_render_subcommand`，只认 `render`，
/// 把 `fonts` 漏在短路之外——见下）。
///
/// ## 为什么要跳过选项找子命令
///
/// 全局选项可出现在子命令之前（`tome --format json render`），故子命令未必是
/// `args[0]`。这里只做"跳过 `--xxx` 及其取值"的最小识别——目标是回答"这次调用
/// 指向的是不是渲染面命令"，不是重建一份完整的参数语义（后者是主循环的职责）。
///
/// ## 为什么选项取值也要跳过
///
/// 若只跳过 `--` 开头的 token，`--format json render` 里的 `json` 会被误认成
/// 子命令。故遇到 `--xxx` 后**再多跳一个 token**（它的取值）。代价是
/// `--flag=value`（等号形态）与不带取值的布尔开关会识别偏——那在本函数里
/// 只影响"是否短路"，不影响正确性：识别偏了就走原有解析路径，行为与今天一致。
///
/// ## 短路必须早于 bootstrap（`fonts` 曾漏在这里）
///
/// 只把 `render` 纳入短路时，`fonts` 仍走"组合根装配 → 分发 → 能力错误"的旧路。
/// 那让它的退出码**取决于装配成不成**：装配成功才退 3，装配失败（如数据目录
/// 写探测撞上杀软瞬时占用）就退 1 并报 `error.storage`。同一命令两种退出码、
/// 取决于本机 IO 运气——那不是契约。
///
/// ## 本函数只负责**识别**，不负责"不经装配"
///
/// 识别出子命令之后，返回的是 [`ParseOutcome::CapabilityUnavailable`]——
/// "不经装配"这条保证由**那个变体 + `main::dispatch` 对它的处理**共同提供，
/// 不由本函数提供。分开写清楚是因为首版把两件事混为一谈：它返回伪造的
/// `Run`，于是"识别到了"和"不装配"脱节——识别对了，装配照做。
///
/// 仅用于 `#[cfg(not(feature = "reader-render"))]` 分支，故在默认构建下
/// 本函数不存在（`dead_code` 不触发）。
#[cfg(not(feature = "reader-render"))]
fn headless_render_face_subcommand(args: &[String]) -> Option<&str> {
    let mut index = 0usize;
    while index < args.len() {
        let argument = args[index].as_str();
        if argument.starts_with("--") {
            index += 2;
            continue;
        }
        if argument.starts_with('-') && argument.len() > 1 {
            index += 1;
            continue;
        }
        return matches!(argument, "render" | "fonts").then_some(argument);
    }
    None
}

/// 解析命令行参数（**不含**程序名）。
///
/// \[DONE] T028/T029 的入口。参数解析的全部规则集中在这里，
/// 便于让"用法错误 → 退出码 2"这条契约只需在一处保证。
pub fn parse(args: &[String]) -> ParseOutcome {
    // 能力缺失压过用法错误（FR-053/FR-057/FR-058、contracts/cli.md §2.3）。
    //
    // ## 为什么这个短路在解析**最前面**
    //
    // headless 构建（`--no-default-features`）里渲染面在**编译期**就不存在，
    // 于是 `render` / `fonts` 这两个命令**根本不存在**。对一个不存在的命令，
    // "参数写错了"这一判断无从谈起——故无论参数是否合法，都应回"不支持能力"
    // （退出码 3 + `offscreenRendering` 标识）。
    //
    // 位置必须在最前：参数校验散在本函数各处（`--theme` 白名单在选项循环里、
    // `--out` 必需项在子命令分支里、未知选项在循环兜底里），任何一处先触发
    // 都会把退出码变成 2。放在最前，判别标准就只有一个：**首个子命令是不是
    // `render`/`fonts`**——与两命令各自的参数面无关。
    //
    // ## 为什么不能只认 `args[0]`
    //
    // 全局选项可以出现在子命令之前（`tome --format json render`），故须跳过
    // 选项与其取值找到真正的子命令。这里不复用解析循环（那会把本函数的职责
    // 变成两遍解析），只做"跳过 `--xxx` 及其值"的最小识别：渲染面命令
    // 在 headless 下不需要任何选项语义，认出来即返回。
    //
    // ## 为什么必须早于 bootstrap，而非只在分发时兜住
    //
    // `main` 的执行顺序是"装配组合根 → 分发命令"。只在分发点退 3（而不短路）
    // 会让退出码**取决于装配成不成**——`fonts` 曾正是如此：装配成功退 3，
    // 装配撞上数据目录写探测失败就退 1 并报 `error.storage`。同一命令两种
    // 退出码、取决于本机 IO 运气，那不是契约。
    //
    // 返回 [`ParseOutcome::CapabilityUnavailable`] 而非伪造 `Run`：后者会让
    // `dispatch` 照常装配（IO 照常发生），且为凑出 `Command` 必须丢掉用户
    // 的全局选项——那是**假短路**，注释里这句"早于 bootstrap"也就成了假话。
    // 详见该变体的文档。
    #[cfg(not(feature = "reader-render"))]
    if let Some(subcommand) = headless_render_face_subcommand(args) {
        return ParseOutcome::CapabilityUnavailable {
            capability: tome_reader::capability::OFFSCREEN_RENDERING.to_string(),
            command: subcommand.to_string(),
        };
    }

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
    let mut config: Option<PathBuf> = None;
    let mut language = crate::i18n::Language::default();
    let mut trace = false;
    let mut rule: Option<String> = None;
    let mut output: Option<PathBuf> = None;
    let mut yes = false;
    let mut group_name: Option<String> = None;
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
    let mut anchor_char: Option<usize> = None;
    let mut target_mode: Option<String> = None;
    // --- `render` / `fonts` 子命令的选项（contracts/cli.md §2.3/§2.4）---
    let mut render_page: Option<u32> = None;
    let mut render_viewport: Option<u32> = None;
    let mut render_out: Option<PathBuf> = None;
    let mut render_theme: Option<String> = None;
    let mut render_reduced_motion = false;
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
            "--page" => {
                let value = match args.get(index + 1).and_then(|v| v.parse::<u32>().ok()) {
                    Some(value) => value,
                    None => {
                        return ParseOutcome::Usage("--page 需要一个非负整数".to_string());
                    }
                };
                render_page = Some(value);
                2
            }
            "--viewport" => {
                let value = match args.get(index + 1).and_then(|v| v.parse::<u32>().ok()) {
                    Some(value) => value,
                    None => {
                        return ParseOutcome::Usage("--viewport 需要一个非负整数".to_string());
                    }
                };
                render_viewport = Some(value);
                2
            }
            "--out" => {
                let value = match args.get(index + 1) {
                    Some(value) => PathBuf::from(value),
                    None => return ParseOutcome::Usage("--out 需要一个文件路径".to_string()),
                };
                render_out = Some(value);
                2
            }
            "--theme" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.to_string(),
                    None => return ParseOutcome::Usage("--theme 需要一个值".to_string()),
                };
                if value != "day" && value != "night" {
                    return ParseOutcome::Usage(format!(
                        "--theme 只接受 day 或 night，实际 `{value}`"
                    ));
                }
                render_theme = Some(value);
                2
            }
            "--reduced-motion" => {
                render_reduced_motion = true;
                1
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
            "--char" => {
                // `layout-anchor` 的必需选项（contracts/cli.md §2.2）
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--char 需要一个数值".to_string()),
                };
                match value.parse::<usize>() {
                    Ok(parsed) => anchor_char = Some(parsed),
                    Err(_) => {
                        return ParseOutcome::Usage(format!(
                            "--char 需要一个非负整数，实际 `{value}`"
                        ))
                    }
                }
                2
            }
            "--target-mode" => {
                let value = match args.get(index + 1) {
                    Some(value) => value.as_str(),
                    None => return ParseOutcome::Usage("--target-mode 需要一个值".to_string()),
                };
                if value != "paginated" && value != "scroll" {
                    return ParseOutcome::Usage(format!(
                        "--target-mode 只接受 paginated 或 scroll，实际 `{value}`"
                    ));
                }
                target_mode = Some(value.to_string());
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
            // T093：`run --trace`（FR-037）
            "--trace" => {
                trace = true;
                1
            }
            // `--config <path>`（contracts/cli.md 全局约定）
            "--config" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--config 需要一个路径".to_string()),
                };
                config = Some(PathBuf::from(value));
                2
            }
            // `--lang <zh|en>`（FR-044）
            "--lang" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--lang 需要 zh 或 en".to_string()),
                };
                let Some(parsed) = crate::i18n::Language::parse(value) else {
                    return ParseOutcome::Usage(format!("不支持的语言 `{value}`（可用：zh / en）"));
                };
                language = parsed;
                2
            }
            // T093：`run --rule <规则> --input <文本> --trace`（轨迹模式）
            "--rule" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--rule 需要一条规则".to_string()),
                };
                rule = Some(value.clone());
                2
            }
            // T093：`data export --output <file>`
            "--output" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => return ParseOutcome::Usage("--output 需要一个路径".to_string()),
                };
                output = Some(PathBuf::from(value));
                2
            }
            // T093：`data delete --yes`（不可撤销操作的确认）
            "--yes" => {
                yes = true;
                1
            }
            // T093：`source group --name <分组|->`
            "--name" => {
                let value = match args.get(index + 1) {
                    Some(value) => value,
                    None => {
                        return ParseOutcome::Usage(
                            "--name 需要一个分组名（或用 `-` 移出分组）".to_string(),
                        )
                    }
                };
                group_name = Some(value.clone());
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

    options.config = config;
    options.language = language;

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
                // `source group <源id...> --name <分组|->`（FR-001）
                "group" => {
                    let ids: Vec<String> = rest[1..].to_vec();
                    if ids.is_empty() {
                        return ParseOutcome::Usage(
                            "source group 需要至少一个书源 id（用法：source group <id...> --name <分组>）"
                                .to_string(),
                        );
                    }
                    let Some(name) = group_name else {
                        return ParseOutcome::Usage(
                            "source group 需要 --name <分组>（用 `-` 表示移出分组）".to_string(),
                        );
                    };
                    Command::SourceGroup {
                        source_ids: ids,
                        name,
                    }
                }
                // `source enable|disable <源id...|--all>`（FR-001）
                "enable" | "disable" => {
                    let enabled = action == "enable";
                    Command::SourceEnable {
                        source_ids: rest[1..].to_vec(),
                        all: check_all,
                        enabled,
                    }
                }
                // `source groups`
                "groups" => Command::SourceGroups,
                other => {
                    return ParseOutcome::Usage(format!(
                        "未知的 source 动作 `{other}`（可用：import / list / check / group /                          enable / disable / groups）"
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
        "keyword" => {
            let Some(action) = rest.first().map(String::as_str) else {
                return ParseOutcome::Usage(
                    "keyword 需要一个动作（list / add / remove）".to_string(),
                );
            };
            match action {
                "list" => Command::KeywordList,
                "add" => {
                    let Some(keyword) = rest.get(1) else {
                        return ParseOutcome::Usage("keyword add 需要 <关键词>".to_string());
                    };
                    Command::KeywordAdd {
                        keyword: keyword.clone(),
                    }
                }
                "remove" => {
                    let Some(keyword) = rest.get(1) else {
                        return ParseOutcome::Usage("keyword remove 需要 <关键词>".to_string());
                    };
                    Command::KeywordRemove {
                        keyword: keyword.clone(),
                    }
                }
                other => {
                    return ParseOutcome::Usage(format!(
                        "未知的 keyword 动作 `{other}`（可用：list / add / remove）"
                    ));
                }
            }
        }
        "data" => {
            let Some(action) = rest.first().map(String::as_str) else {
                return ParseOutcome::Usage(
                    "data 需要一个动作（export / import / clear-cache / delete）".to_string(),
                );
            };
            match action {
                "export" => Command::DataExport { output },
                "import" => {
                    let Some(file) = rest.get(1) else {
                        return ParseOutcome::Usage("data import 需要 <file>".to_string());
                    };
                    Command::DataImport {
                        file: PathBuf::from(file),
                    }
                }
                "clear-cache" => {
                    let Some(kind) = rest.get(1) else {
                        return ParseOutcome::Usage(
                            "data clear-cache 需要 <chapters|images>".to_string(),
                        );
                    };
                    if kind != "chapters" && kind != "images" {
                        return ParseOutcome::Usage(format!(
                            "未知的缓存类别 `{kind}`（可用：chapters / images）"
                        ));
                    }
                    Command::DataClearCache { kind: kind.clone() }
                }
                "delete" => Command::DataDelete { yes },
                other => {
                    return ParseOutcome::Usage(format!(
                        "未知的 data 动作 `{other}`（可用：export / import / clear-cache / delete）"
                    ));
                }
            }
        }
        "run" => {
            // `--rule` 存在时是**轨迹模式**：不需要书源与动作，
            // 只要规则原文 + 输入文本（FR-037 的调试用法）
            if let Some(rule) = rule {
                if !trace {
                    return ParseOutcome::Usage(
                        "`--rule` 需与 `--trace` 同用（否则不知道要输出什么）".to_string(),
                    );
                }
                return ParseOutcome::Run {
                    options,
                    command: Command::RuleTrace {
                        rule,
                        input: input.unwrap_or_default(),
                    },
                };
            }
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
                trace,
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
            anchor_char,
            target_mode: target_mode.clone(),
        }),
        // `render`（contracts/cli.md §2.3）
        "render" => {
            let Some(out) = render_out.clone() else {
                return ParseOutcome::Usage("render 需要 --out <png>（输出路径）".to_string());
            };
            let has_target = render_page.is_some() || render_viewport.is_some();
            if !has_target {
                return ParseOutcome::Usage(
                    "render 需要 --page <n>（翻页）或 --viewport <n>（滚动）之一".to_string(),
                );
            }
            Command::Render(Box::new(RenderArgs {
                layout: LayoutArgs {
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
                    anchor_char,
                    target_mode: target_mode.clone(),
                },
                page: render_page,
                viewport_index: render_viewport,
                out: Some(out),
                theme: render_theme,
                reduced_motion: render_reduced_motion,
            }))
        }
        // `fonts`（contracts/cli.md §2.4）
        "fonts" => Command::Fonts,
        // `verify-layout`（contracts/cli.md §2.5）
        //
        // 与 001 的 `verify --corpus` 共用 `--corpus` / `--report` 两个全局
        // 选项（都是"给一个目录、出一份报告"的形态），故本分支只做必需项校验。
        "verify-layout" => {
            let Some(corpus) = corpus else {
                return ParseOutcome::Usage(
                    "verify-layout 需要 --corpus <dir>（排版合成语料目录）".to_string(),
                );
            };
            Command::VerifyLayout { corpus, report }
        }
        // `layout-anchor`（contracts/cli.md §2.2）
        "layout-anchor" => {
            let Some(char_offset) = anchor_char.or(anchor_offset) else {
                return ParseOutcome::Usage(
                    "layout-anchor 需要 --char <offset>（要换算的章内字符位置）".to_string(),
                );
            };
            Command::LayoutAnchor(LayoutArgs {
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
                anchor: Some(char_offset),
                anchor_char: Some(char_offset),
                target_mode,
            })
        }
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
/// \[DONE] 错误分类 → 退出码的**唯一**落点。
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
        "  tome source group <id...> --name <分组|->          书源分组（`-` 移出分组）",
        "  tome source enable|disable <id...|--all>           书源启停",
        "  tome source groups                                 列出分组",
        "  tome search <source-id> <keyword>                  按书源搜索",
        "  tome search --source-file <f> <keyword>            用临时书源文件搜索",
        "  tome info <source-id> <book-url>                   书籍详情",
        "  tome toc <source-id> <book-url>                    目录",
        "  tome content <source-id> <chapter-url>             章节正文（净化后）",
        "  tome local import <path> [--copy] [--library <dir>] 导入本地书（TXT/EPUB/MOBI）",
        "  tome local toc <path>                              本地书目录识别",
        "  tome local scan <dir> [--library <dir>...]         库根扫描（多库根）",
        "  tome run <source-id> --action <a> [--input <s>]     通用执行（search/info/toc/content）",
        "  tome run --rule <规则> --input <文本> --trace       规则求值轨迹（FR-037）",
        "  tome keyword list|add|remove <词>                  搜索关键词收藏（FR-018）",
        "  tome data export [--output <file>]                 导出全部阅读数据（FR-033）",
        "  tome data import <file>                            导入备份",
        "  tome data clear-cache <chapters|images>            分别清理两种缓存（SC-017）",
        "  tome data delete [--yes]                           完整删除用户数据（FR-035）",
        "  tome layout --text <file> [几何选项]                排版一章（不渲染）",
        "  tome layout --chapter <book>#<n> [几何选项]         同上（书架形式，WIP）",
        "  tome layout-anchor --text <f> --char <n>           位置换算与形态切换（SC-005b）",
        "                   [--mode p|scroll] [--target-mode p|scroll] [几何选项]",
        "  tome render --text <f> --page <n> --out <png>      离屏渲染一页为 PNG（SC-011）",
        "                   [--theme day|night] [--reduced-motion] [几何选项]",
        "  tome fonts                                         内嵌字体子集与回退链报告（SC-017）",
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
        "  --config <path>        配置文件（默认数据目录下的 config.toml）",
        "  --lang zh|en           界面语言（默认 zh，FR-044）",
        "  --corpus <dir>         合成语料目录（verify 用）",
        "  --real-sources <dir>   真实书源目录（verify --real-sources 用）",
        "  --golden <dir>         对照基线目录（verify --real-sources 用）",
        "  --write-baseline       把本次结果写为新基线（首轮基线落盘用）",
        "  --report <file>        报告输出文件（verify 用，默认为 stdout）",
        "  --rule <规则>          规则求值轨迹的规则（需与 --trace 同用）",
        "  --trace                输出规则求值轨迹（FR-037）",
        "  --output <file>        数据导出目标文件（data export 用）",
        "  --yes                  确认执行不可撤销的操作（data delete 用）",
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
        "  --anchor <offset>      报告该锚点落在哪一行/哪一屏",
        "  --char <offset>        layout-anchor 的目标字符位置（§2.2）",
        "  --target-mode paginated|scroll  layout-anchor 的目标形态（§2.2）",
        "",
        "render 选项（contracts/cli.md §2.3）：",
        "  --page <n>             目标页（翻页模式）",
        "  --viewport <n>         目标视口（滚动模式），与 --page 二选一",
        "  --out <png>            输出 PNG 路径（必需）",
        "  --theme day|night      主题（默认 day，FR-030）",
        "  --reduced-motion       关闭动效（不影响静态渲染结果，FR-032）",
        "",
        "验证子命令（contracts/cli.md §2.5）：",
        "  verify-layout --corpus <dir> [--report <file>]",
        "                         跑排版合成语料，输出通过率与逐条差异",
        "                         （与 001 的 verify 同形态；无硬门槛，如实报告）",
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
        // 用**真正不存在的**子命令（T093 起 `data` 已落地，故不能再用它）。
        let text = parse_usage(&["definitely-not-a-command"]);
        assert!(
            text.contains("缺少子命令") || text.contains("definitely"),
            "{text}"
        );
    }

    // ---------- T093：补齐的命令面 ----------

    #[test]
    fn 规则轨迹需要_trace_与_rule_同用() {
        // `--rule` 单独给：不知道要输出什么，报用法错误
        let text = parse_usage(&["run", "--rule", "p@text", "--input", "<p>x</p>"]);
        assert!(text.contains("--trace"), "{text}");

        // 两者同用：解析成 RuleTrace
        let (_, command) = parse_ok(&["run", "--rule", "p@text", "--input", "<p>x</p>", "--trace"]);
        match command {
            Command::RuleTrace { rule, input } => {
                assert_eq!(rule, "p@text");
                assert_eq!(input, "<p>x</p>");
            }
            other => panic!("应为 RuleTrace，实际 {other:?}"),
        }
    }

    #[test]
    fn run_的动作模式仍可解析() {
        let (_, command) = parse_ok(&["run", "src-id", "--action", "search", "--input", "词"]);
        match command {
            Command::Run {
                source_id,
                action,
                input,
                trace,
            } => {
                assert_eq!(source_id, "src-id");
                assert_eq!(action, "search");
                assert_eq!(input.as_deref(), Some("词"));
                assert!(!trace, "没给 --trace 时不该为真");
            }
            other => panic!("应为 Run，实际 {other:?}"),
        }
    }

    #[test]
    fn 书源分组解析() {
        let (_, command) = parse_ok(&[
            "source",
            "group",
            "https://a.com",
            "https://b.com",
            "--name",
            "玄幻",
        ]);
        match command {
            Command::SourceGroup { source_ids, name } => {
                assert_eq!(source_ids.len(), 2);
                assert_eq!(name, "玄幻");
            }
            other => panic!("应为 SourceGroup，实际 {other:?}"),
        }
    }

    #[test]
    fn 书源分组移出用连字符() {
        let (_, command) = parse_ok(&["source", "group", "https://a.com", "--name", "-"]);
        match command {
            Command::SourceGroup { name, .. } => assert_eq!(name, "-"),
            other => panic!("应为 SourceGroup，实际 {other:?}"),
        }
    }

    #[test]
    fn 书源分组缺_name_是用法错误() {
        let text = parse_usage(&["source", "group", "https://a.com"]);
        assert!(text.contains("--name"), "{text}");
    }

    #[test]
    fn 书源分组缺_id_是用法错误() {
        let text = parse_usage(&["source", "group", "--name", "组"]);
        assert!(text.contains("书源 id") || text.contains("id"), "{text}");
    }

    #[test]
    fn 书源启停解析() {
        let (_, command) = parse_ok(&["source", "enable", "https://a.com"]);
        match command {
            Command::SourceEnable {
                source_ids,
                all,
                enabled,
            } => {
                assert_eq!(source_ids, vec!["https://a.com"]);
                assert!(!all);
                assert!(enabled, "enable 应为 true");
            }
            other => panic!("应为 SourceEnable，实际 {other:?}"),
        }

        let (_, command) = parse_ok(&["source", "disable", "--all"]);
        match command {
            Command::SourceEnable { all, enabled, .. } => {
                assert!(all);
                assert!(!enabled, "disable 应为 false");
            }
            other => panic!("应为 SourceEnable，实际 {other:?}"),
        }
    }

    #[test]
    fn 列出分组可解析() {
        let (_, command) = parse_ok(&["source", "groups"]);
        assert!(matches!(command, Command::SourceGroups));
    }

    #[test]
    fn 关键词命令解析() {
        let (_, command) = parse_ok(&["keyword", "list"]);
        assert!(matches!(command, Command::KeywordList));

        let (_, command) = parse_ok(&["keyword", "add", "斗破"]);
        match command {
            Command::KeywordAdd { keyword } => assert_eq!(keyword, "斗破"),
            other => panic!("应为 KeywordAdd，实际 {other:?}"),
        }

        let (_, command) = parse_ok(&["keyword", "remove", "斗破"]);
        match command {
            Command::KeywordRemove { keyword } => assert_eq!(keyword, "斗破"),
            other => panic!("应为 KeywordRemove，实际 {other:?}"),
        }
    }

    #[test]
    fn 关键词缺词是用法错误() {
        assert!(parse_usage(&["keyword", "add"]).contains("关键词"));
        assert!(parse_usage(&["keyword", "remove"]).contains("关键词"));
    }

    #[test]
    fn 数据命令解析() {
        let (_, command) = parse_ok(&["data", "export"]);
        assert!(matches!(command, Command::DataExport { output: None }));

        let (_, command) = parse_ok(&["data", "export", "--output", "/tmp/b.json"]);
        match command {
            Command::DataExport { output } => {
                assert_eq!(output, Some(PathBuf::from("/tmp/b.json")));
            }
            other => panic!("应为 DataExport，实际 {other:?}"),
        }

        let (_, command) = parse_ok(&["data", "import", "/tmp/b.json"]);
        match command {
            Command::DataImport { file } => assert_eq!(file, PathBuf::from("/tmp/b.json")),
            other => panic!("应为 DataImport，实际 {other:?}"),
        }

        for kind in ["chapters", "images"] {
            let (_, command) = parse_ok(&["data", "clear-cache", kind]);
            match command {
                Command::DataClearCache { kind: got } => assert_eq!(got, kind),
                other => panic!("应为 DataClearCache，实际 {other:?}"),
            }
        }

        let (_, command) = parse_ok(&["data", "delete"]);
        match command {
            Command::DataDelete { yes } => assert!(!yes, "没给 --yes 时应为 false"),
            other => panic!("应为 DataDelete，实际 {other:?}"),
        }

        let (_, command) = parse_ok(&["data", "delete", "--yes"]);
        match command {
            Command::DataDelete { yes } => assert!(yes),
            other => panic!("应为 DataDelete，实际 {other:?}"),
        }
    }

    #[test]
    fn 未知缓存类别是用法错误() {
        let text = parse_usage(&["data", "clear-cache", "everything"]);
        assert!(
            text.contains("chapters") && text.contains("images"),
            "{text}"
        );
    }

    #[test]
    fn 全局_flag_齐全() {
        // contracts/cli.md 的全局约定：--format / --data-dir / --config / --offline
        let (options, _) = parse_ok(&[
            "--format",
            "human",
            "--data-dir",
            "/tmp/d",
            "--config",
            "/tmp/c.toml",
            "--lang",
            "en",
            "keyword",
            "list",
        ]);
        assert_eq!(options.format, OutputFormat::Human);
        assert_eq!(options.data_dir, Some(PathBuf::from("/tmp/d")));
        assert_eq!(options.config, Some(PathBuf::from("/tmp/c.toml")));
        assert_eq!(options.language, crate::i18n::Language::En);
    }

    #[test]
    fn 不支持的语言是用法错误() {
        let text = parse_usage(&["--lang", "jp", "keyword", "list"]);
        assert!(text.contains("zh") && text.contains("en"), "{text}");
    }

    #[test]
    fn config_缺路径是用法错误() {
        assert!(parse_usage(&["--config"]).contains("--config"));
    }

    #[test]
    fn lang_缺值是用法错误() {
        assert!(parse_usage(&["--lang"]).contains("--lang"));
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
