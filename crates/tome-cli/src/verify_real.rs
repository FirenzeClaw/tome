//! [DONE] `verify --real-sources`：真实书源端到端对照与回归基线（T086 / SC-002b）
//!
//! # 它和 `verify --corpus` 的分工
//!
//! | | `--corpus`（T085） | `--real-sources`（本模块） |
//! |---|---|---|
//! | 输入 | 合成用例（入仓库） | 真实书源（不入仓库） |
//! | 是否发请求 | 否（纯计算） | **是**（端到端抓取） |
//! | 判据 | 通过率 ≥95%（SC-002a） | **不得回退**（SC-002b） |
//! | 产物 | 可入仓库 | 特征指纹可入仓库，正文不可 |
//!
//! SC-002b 的原文：
//!
//! > 规则语义一致性（真实书源档）：真实书源端到端抓取的结果一致性作为
//! > 持续回归指标，**不设硬门槛**，首轮基线记录在案并在后续迭代中不得回退
//!
//! 注意"不设硬门槛"**不等于**"不判定"：它说的是"首次跑出来的数字不必达标"，
//! 但一旦记录下来，后续就**不能比它差**。故本模块的核心是**基线比对**，
//! 不是通过率阈值。
//!
//! # 基线的两种落盘形态（数据性质决定入不入 git）
//!
//! | 内容 | 落哪 | 理由 |
//! |---|---|---|
//! | 抓到的正文、书名、目录 | `testdata/captures/`（gitignore） | 第三方站点内容，属第三方资产 |
//! | **特征指纹**（条数/长度/结构骨架/散列） | `testdata/baselines/`（入 git） | 不含第三方内容，且必须随代码走才能做回归 |
//!
//! 这个切分是 SC-002b "首轮基线记录在案"能落地的前提：基线若也进 captures，
//! 换台机器克隆下来就没有基线可比，回归门形同虚设。
//!
//! # 文件约定
//!
//! ```text
//! <real-sources>/                    真实书源目录（*.json，不入仓库）
//!     <源名>.json
//! <golden>/                          基线目录
//!     fingerprint.json               特征指纹基线（每源每动作一条）
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tome_core::{ErrorKind, NetHost};

use crate::commands::{CommandOutcome, HostCapabilities};
use crate::flags::GlobalOptions;

/// 基线的文件名。
const FINGERPRINT_FILE: &str = "fingerprint.json";

/// 基线比对的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 与基线一致，或首次建立基线
    Pass,
    /// 比基线差（回归）
    Regressed,
}

/// 单个"源 × 动作"的指纹。
///
/// # 为什么是这几个字段
///
/// 指纹要同时满足两件事：**能发现回归**（条数骤降、正文截断）且
/// **不含第三方内容**（否则不能入 git）。故只取：
///
/// - `entries`：结果条数（书名列表长度、目录长度）
/// - `total_chars`：正文字符总数（能发现"正文抓空了"）
/// - `digest`：内容散列（能发现"抓到的不是同一份内容"）
/// - `ok`：该动作是否成功
///
/// ## 为什么不用"逐字比对"
///
/// 真实站点内容会变（更新章节、改广告位），逐字比对会天天变红，
/// 最后没人看——那比不设门更糟。指纹比对的是**结构量级**，
/// 站点改版导致的量级变化才是我们真正关心的。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Fingerprint {
    /// 用例标识（`bookSourceUrl/动作`）
    ///
    /// # 键为什么用 `bookSourceUrl`（2026-10-10 PM 裁决）
    ///
    /// 键的职责是**在多次运行之间认出"这是同一条用例"**。源名换不得
    /// ——书源改名（作者改了、用户自己改了、导入时被规范化了）都会让
    /// 这条基线配不上，表现为"基线里没有这条"，于是那次运行**静默失去
    /// 回归保护**（不报错、不报警，只是不再比对）。
    ///
    /// `bookSourceUrl` 是书源的主键（`tome-core` 的 `Source::id`），
    /// 按定义它标识这个源，改名不影响它。
    ///
    /// ## 代价与补偿
    ///
    /// URL 作键在报告里可读性差（`https://a.example.com/search`
    /// 不如 `我的书源/search` 直观）。故指纹另带 [`Self::label`] 存源名
    /// ——**键求稳定、标签求可读**，两者不必是同一个字段。
    ///
    /// ## wave1 的旧基线
    ///
    /// wave1 的基线用源名作键，与本次不兼容。处置见 `load_baseline`：
    /// 旧键的条目**无法自动映射**（源名到 URL 不是可逆转换），故首次
    /// 运行会视为"基线里没有这条"并提示重新生成。这是有意选择的
    /// **明确失效**而非"尽力猜测"——猜错的键会让回归门静默比对错对象。
    pub case: String,
    /// 源名（仅供报告阅读，不参与比对）
    ///
    /// `#[serde(default)]`：wave1 的基线没有这个字段，读旧文件不该报错
    /// （虽然它的键也用不上，但**读得进来**才能给出"请重新生成"的提示，
    /// 而不是一个反序列化错误）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// 动作是否成功
    pub ok: bool,
    /// 结果条数（搜索命中数、目录章节数、正文段落数）
    pub entries: usize,
    /// 总字符数
    pub total_chars: usize,
    /// 内容散列（FNV-1a 十六进制；不求密码学强度，只求稳定与短）
    pub digest: String,
}

impl Fingerprint {
    /// 从一批文本构造指纹。
    #[must_use]
    pub fn from_texts(case: &str, texts: &[String]) -> Self {
        Self::with_label(case, "", texts)
    }

    /// 从一批文本构造指纹，并带一个可读标签。
    #[must_use]
    pub fn with_label(case: &str, label: &str, texts: &[String]) -> Self {
        let total_chars: usize = texts.iter().map(|text| text.chars().count()).sum();
        Self {
            case: case.to_string(),
            label: label.to_string(),
            ok: true,
            entries: texts.len(),
            total_chars,
            digest: fnv1a_hex(&texts.join("\u{1f}")),
        }
    }

    /// 失败用例的指纹。
    #[must_use]
    pub fn failed(case: &str) -> Self {
        Self::failed_with_label(case, "")
    }

    /// 失败用例的指纹，带可读标签。
    #[must_use]
    pub fn failed_with_label(case: &str, label: &str) -> Self {
        Self {
            case: case.to_string(),
            label: label.to_string(),
            ok: false,
            entries: 0,
            total_chars: 0,
            digest: String::new(),
        }
    }

    /// 与基线比对，判断是否回退。
    ///
    /// # 回退的判据
    ///
    /// | 情形 | 判定 | 理由 |
    /// |---|---|---|
    /// | 基线失败、本次成功 | 不算回退 | 变好永远可以 |
    /// | 基线成功、本次失败 | **回退** | 从能用变成不能用 |
    /// | 条数/字符数减少 | **回退** | 抓漏了 |
    /// | 条数/字符数增加 | 不算回退 | 站点新增章节是正常的 |
    /// | 散列变化但量级未降 | 不算回退 | 内容更新（改广告）不该判回归 |
    ///
    /// 只对**减少**判回退是有意的：SC-002b 要的是"不得回退"，
    /// 而把"内容变了"也判成回归会让这个门因噪音而失去意义。
    #[must_use]
    pub fn regressed_against(&self, baseline: &Self) -> bool {
        if !self.ok && baseline.ok {
            return true;
        }
        if !self.ok {
            // 基线也失败：无从判回退
            return false;
        }
        self.entries < baseline.entries || self.total_chars < baseline.total_chars
    }
}

/// 一次比对的逐条结果。
#[derive(Debug, Clone)]
pub struct CaseComparison {
    /// 用例标识
    pub case: String,
    /// 基线指纹（首轮时为 `None`）
    pub baseline: Option<Fingerprint>,
    /// 本次指纹
    pub current: Fingerprint,
    /// 是否回退
    pub regressed: bool,
}

/// 整份报告的汇总。
#[derive(Debug, Clone, Default)]
pub struct ReportSummary {
    /// 逐条比对
    pub comparisons: Vec<CaseComparison>,
    /// 是否首轮（基线文件原本不存在）
    pub first_run: bool,
    /// 基线文件路径
    pub baseline_path: Option<PathBuf>,
    /// 产生基线时的 git 版本（若可得）
    pub baseline_revision: Option<String>,
}

impl ReportSummary {
    /// 回退的条目。
    pub fn regressions(&self) -> impl Iterator<Item = &CaseComparison> {
        self.comparisons.iter().filter(|item| item.regressed)
    }

    /// 是否有回退（回归门的判据）。
    #[must_use]
    pub fn has_regressions(&self) -> bool {
        self.regressions().next().is_some()
    }
}

/// 内容散列（FNV-1a 64 位，十六进制）。
///
/// # 为什么不用 SHA-256
///
/// 指纹要入 git 且要短。它只需**稳定**（同内容同值）与**够散**
/// （不同内容几乎不碰撞），不需要抗碰撞——这里没有攻击者。
/// 引一个 crypto 依赖只为算个内容标识不划算。
#[must_use]
pub fn fnv1a_hex(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// 从目录读全部基线指纹。文件不存在时返回空表（首轮）。
fn load_baseline(path: &Path) -> Result<BTreeMap<String, Fingerprint>, String> {
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("读基线文件 {} 失败：{error}", path.display()))?;
    let list: Vec<Fingerprint> = serde_json::from_str(&text)
        .map_err(|error| format!("基线文件 {} 不是合法指纹数组：{error}", path.display()))?;
    Ok(list
        .into_iter()
        .map(|item| (item.case.clone(), item))
        .collect())
}

/// 把指纹表写到基线文件（按键排序，让文件可 diff）。
fn save_baseline(path: &Path, entries: &[Fingerprint]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("建基线目录 {} 失败：{error}", parent.display()))?;
    }
    let sorted: Vec<&Fingerprint> = {
        let mut list: Vec<&Fingerprint> = entries.iter().collect();
        list.sort_by(|left, right| left.case.cmp(&right.case));
        list
    };
    let text = serde_json::to_string_pretty(&sorted)
        .map_err(|error| format!("序列化基线失败：{error}"))?;
    std::fs::write(path, format!("{text}\n"))
        .map_err(|error| format!("写基线文件 {} 失败：{error}", path.display()))
}

/// 渲染报告为 JSON（与 `verify --corpus` 同风格，可 diff）。
#[must_use]
pub fn render_report(summary: &ReportSummary) -> String {
    let cases: Vec<serde_json::Value> = summary
        .comparisons
        .iter()
        .map(|item| {
            serde_json::json!({
                "case": item.case,
                // 可读标签（源名）；键是 URL，报告里两个都给便于人读
                "label": item.current.label,
                "regressed": item.regressed,
                "baseline": item.baseline.as_ref().map(|baseline| serde_json::json!({
                    "ok": baseline.ok,
                    "entries": baseline.entries,
                    "total_chars": baseline.total_chars,
                    "digest": baseline.digest,
                })),
                "current": {
                    "ok": item.current.ok,
                    "entries": item.current.entries,
                    "total_chars": item.current.total_chars,
                    "digest": item.current.digest,
                },
            })
        })
        .collect();

    let regressions: Vec<serde_json::Value> = summary
        .regressions()
        .map(|item| {
            serde_json::json!({
                "case": item.case,
                "label": item.current.label,
                "baseline": item.baseline.as_ref().map(|baseline| serde_json::json!({
                    "entries": baseline.entries,
                    "total_chars": baseline.total_chars,
                })),
                "current": {
                    "entries": item.current.entries,
                    "total_chars": item.current.total_chars,
                },
            })
        })
        .collect();

    let payload = serde_json::json!({
        "mode": "real-sources",
        "criterion": "SC-002b：不设硬门槛，首轮基线记录在案，后续不得回退",
        "first_run": summary.first_run,
        "baseline": summary.baseline_path.as_ref().map(|path| path.display().to_string()),
        "baseline_revision": summary.baseline_revision,
        "total": summary.comparisons.len(),
        "regressed": summary.regressions().count(),
        "cases": cases,
        "regressions": regressions,
    });

    match serde_json::to_string_pretty(&payload) {
        Ok(text) => format!("{text}\n"),
        Err(error) => {
            let fallback = serde_json::json!({ "error": format!("报告序列化失败：{error}") });
            format!("{fallback}\n")
        }
    }
}

/// `verify --real-sources` 的入口。
///
/// # 退出码
///
/// | 情形 | 码 |
/// |---|---|
/// | 无回退（含首轮建基线） | 0 |
/// | 有回退 | 1（业务错误——门没通过） |
/// | 目录/基线读不了 | 1，带上原因 |
///
/// # `options` 只用来定位数据目录
///
/// 真实书源档会经完整管线抓取，故需要数据目录（缓存/注册表）。
/// 网络宿主由调用方（组合根）注入——本模块不自行构造。
pub fn run(
    real_sources: &Path,
    golden: &Path,
    report_path: Option<&Path>,
    write_baseline: bool,
    options: &GlobalOptions,
    host: &dyn NetHost,
    hosts: HostCapabilities,
) -> CommandOutcome {
    if !real_sources.is_dir() {
        return CommandOutcome::failure(
            &ErrorKind::Configuration,
            &format!("真实书源目录不存在或不是目录：{}", real_sources.display()),
        );
    }

    // 载入真实书源（*.json）
    let sources = match load_real_sources(real_sources) {
        Ok(sources) => sources,
        Err(message) => return CommandOutcome::failure(&ErrorKind::Configuration, &message),
    };

    if sources.is_empty() {
        return CommandOutcome::failure(
            &ErrorKind::Configuration,
            &format!(
                "真实书源目录 {} 下没有可用的 *.json 书源",
                real_sources.display()
            ),
        );
    }

    let baseline_path = golden.join(FINGERPRINT_FILE);
    let baseline = match load_baseline(&baseline_path) {
        Ok(baseline) => baseline,
        Err(message) => return CommandOutcome::failure(&ErrorKind::Storage, &message),
    };
    let first_run = baseline.is_empty();

    // ## wave1 旧基线的处置（键格式变更的迁移）
    //
    // wave1 用**源名**作键（`我的书源/search`），本次改为 `bookSourceUrl`
    // （`https://a.example.com/search`）。两者**不可自动映射**——源名到 URL
    // 不是可逆转换（同一个名字可能对应多个源，反过来也不行）。
    //
    // 故这里只做**检测与明示**：发现旧格式就告诉用户"这份基线用不了，
    // 请用 --write-baseline 重新生成"。不猜、不静默、不假装比对过。
    //
    // 判据是"键里不含 `://`"——新键必然含（书源 URL 一定有协议前缀），
    // 而旧键是纯名字。这个判据简单且不会误伤（书源名里出现 `://` 的可能
    // 性可忽略，且即便有，报"请重新生成"也只是多一次无害的操作）。
    let stale_keys: Vec<String> = baseline
        .keys()
        .filter(|key| !key.contains("://"))
        .cloned()
        .collect();

    // 逐源跑动作，算指纹
    let mut fingerprints = Vec::new();
    for parsed in &sources {
        let source_id = parsed.source.id.clone();
        let name = parsed.source.name.clone();

        // 搜索（T084 的对照动作之一）
        //
        // ## 键用 `bookSourceUrl`（源 id）而非源名（2026-10-10 PM 裁决）
        //
        // wave1 用源名作键，当时标了 [UNCERTAIN]：改名会让基线配不上、
        // 静默失去回归保护。PM 裁决改键。
        //
        // 源名仍然保留在指纹里（`label` 字段）供人读报告——**键求稳定、
        // 标签求可读**，两者不必是同一个字段。
        let search_case = format!("{source_id}/search");
        match run_search_fingerprint(options, &source_id, host, &hosts) {
            Ok(texts) => fingerprints.push(Fingerprint::with_label(&search_case, &name, &texts)),
            Err(reason) => {
                eprintln!("[警告] {search_case}（{name}）失败：{reason}");
                fingerprints.push(Fingerprint::failed_with_label(&search_case, &name));
            }
        }
    }

    // 与基线比对
    let comparisons: Vec<CaseComparison> = fingerprints
        .iter()
        .map(|current| {
            let base = baseline.get(&current.case).cloned();
            let regressed = base
                .as_ref()
                .is_some_and(|baseline| current.regressed_against(baseline));
            CaseComparison {
                case: current.case.clone(),
                baseline: base,
                current: current.clone(),
                regressed,
            }
        })
        .collect();

    let summary = ReportSummary {
        comparisons,
        first_run,
        baseline_path: Some(baseline_path.clone()),
        baseline_revision: None,
    };

    // `--write-baseline`：把本次结果落为新基线（首轮基线落盘）
    if write_baseline {
        if let Err(message) = save_baseline(&baseline_path, &fingerprints) {
            return CommandOutcome::failure(&ErrorKind::Storage, &message);
        }
        eprintln!("已写入基线：{}", baseline_path.display());
    }

    let rendered = render_report(&summary);

    if let Some(path) = report_path {
        if let Err(error) = std::fs::write(path, &rendered) {
            return CommandOutcome::failure(
                &ErrorKind::Storage,
                &format!("报告写入 {} 失败：{error}", path.display()),
            );
        }
    }

    // 首轮且未写基线：不算失败，但要说清"还没有基线可比"。
    // 静默通过会让"门装了吗"变成未知——那正是 SC-002b 最怕的。
    if first_run && !write_baseline {
        let notice = format!(
            "{rendered}\
             \n注意：基线文件 {} 不存在，本次为**首轮对照**——尚无基线可比。\n\
             请用 `--write-baseline` 落盘首轮基线，后续迭代才有回归判据。\n",
            baseline_path.display()
        );
        return CommandOutcome::with_stdout(notice);
    }

    // 旧键格式的基线：明确告知"这份配不上"，而不是让它静默失效
    if !stale_keys.is_empty() && !write_baseline {
        let notice = format!(
            "{rendered}\
             \n注意：基线文件 {} 里有 {} 条**旧格式**条目（键为源名，本版起改用 \
             bookSourceUrl）。\n旧键无法自动映射到新键，这些条目本次未被比对——\
             受影响的源暂时没有回归保护。\n请用 `--write-baseline` 重新生成基线。\n\
             旧键（前 3 条）：{}\n",
            baseline_path.display(),
            stale_keys.len(),
            stale_keys
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<String>>()
                .join("、")
        );
        return CommandOutcome::with_stdout(notice);
    }

    if summary.has_regressions() {
        CommandOutcome::failure(&ErrorKind::Rule, rendered.trim_end())
    } else {
        CommandOutcome::with_stdout(rendered)
    }
}

/// 载入目录下的全部 `*.json` 书源。
fn load_real_sources(dir: &Path) -> Result<Vec<tome_source::source::ParsedSource>, String> {
    let mut files: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("读目录 {} 失败：{error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("遍历目录 {} 失败：{error}", dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) == Some("json") {
            files.push(path);
        }
    }
    files.sort();

    let mut sources = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("读书源 {} 失败：{error}", path.display()))?;
        let outcome = tome_source::source::parse_sources(&text)
            .map_err(|error| format!("解析书源 {} 失败：{error}", path.display()))?;
        sources.extend(outcome.sources);
    }
    Ok(sources)
}

/// 跑一次搜索并返回可算指纹的文本列表（书名 + 作者）。
///
/// # 为什么失败也返回 `Result` 而不是让调用方 panic
///
/// 真实书源会因站点不可达而失败——那是**预期内**的情形（这正是真实档
/// 与合成档的区别）。失败要被记成指纹（`ok: false`）并参与比对，
/// 而不是让整个过程崩溃。
fn run_search_fingerprint(
    options: &GlobalOptions,
    source_id: &str,
    host: &dyn NetHost,
    hosts: &HostCapabilities,
) -> Result<Vec<String>, String> {
    // 搜索关键词：取一个通用词。真实书源的搜索大多对关键词无强约束。
    let keyword = "测试";

    let parsed = match crate::pipeline_commands::load_one_source(options, source_id) {
        Ok(parsed) => parsed,
        Err(outcome) => return Err(format!("载入书源失败：{}", outcome.stderr.trim())),
    };

    let engine = match crate::pipeline_commands::engine_for(&parsed.source, hosts) {
        Ok(engine) => engine,
        Err(outcome) => return Err(format!("建引擎失败：{}", outcome.stderr.trim())),
    };

    match tome_source::search::search_one_source_with_js(&parsed, keyword, 1, host, engine.as_ref())
    {
        Ok(hits) => Ok(hits
            .into_iter()
            .flat_map(|hit| [Some(hit.name), hit.author].into_iter().flatten())
            .collect()),
        Err(error) => Err(format!("{error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 指纹从文本算出条数与字符数() {
        let texts = vec!["书一".to_string(), "作者甲".to_string()];
        let fingerprint = Fingerprint::from_texts("源/search", &texts);
        assert_eq!(fingerprint.entries, 2);
        assert_eq!(fingerprint.total_chars, 5, "「书一」+「作者甲」= 5 字符");
        assert!(fingerprint.ok);
        assert!(!fingerprint.digest.is_empty());
    }

    #[test]
    fn 指纹散列稳定且区分内容() {
        let one = Fingerprint::from_texts("c", &["甲".to_string()]);
        let same = Fingerprint::from_texts("c", &["甲".to_string()]);
        let other = Fingerprint::from_texts("c", &["乙".to_string()]);
        assert_eq!(one.digest, same.digest, "同内容应得同散列");
        assert_ne!(one.digest, other.digest, "不同内容应得不同散列");
    }

    #[test]
    fn 条数减少判为回退() {
        let baseline = Fingerprint::from_texts("c", &["a".to_string(), "b".to_string()]);
        let current = Fingerprint::from_texts("c", &["a".to_string()]);
        assert!(current.regressed_against(&baseline));
    }

    #[test]
    fn 条数增加不算回退() {
        let baseline = Fingerprint::from_texts("c", &["a".to_string()]);
        let current = Fingerprint::from_texts("c", &["a".to_string(), "b".to_string()]);
        assert!(
            !current.regressed_against(&baseline),
            "站点新增内容不是回归"
        );
    }

    #[test]
    fn 从成功变失败是回退() {
        let baseline = Fingerprint::from_texts("c", &["a".to_string()]);
        let current = Fingerprint::failed("c");
        assert!(current.regressed_against(&baseline));
    }

    #[test]
    fn 从失败变成功不算回退() {
        let baseline = Fingerprint::failed("c");
        let current = Fingerprint::from_texts("c", &["a".to_string()]);
        assert!(!current.regressed_against(&baseline), "变好永远可以");
    }

    #[test]
    fn 内容变化但量级不降不算回退() {
        // 站点改了广告/更新了章节：散列会变，但条数与字符数没降
        let baseline = Fingerprint::from_texts("c", &["旧内容一样长".to_string()]);
        let current = Fingerprint::from_texts("c", &["新内容一样长".to_string()]);
        assert_ne!(baseline.digest, current.digest);
        assert!(
            !current.regressed_against(&baseline),
            "内容更新不该判成回归——否则门会因噪音失去意义"
        );
    }

    #[test]
    fn 双方都失败不算回退() {
        let baseline = Fingerprint::failed("c");
        let current = Fingerprint::failed("c");
        assert!(!current.regressed_against(&baseline), "无从判回退");
    }

    #[test]
    fn 基线往返读写() {
        let dir = tempfile::tempdir().expect("应能建临时目录");
        let path = dir.path().join(FINGERPRINT_FILE);
        let entries = vec![
            Fingerprint::from_texts("b/search", &["x".to_string()]),
            Fingerprint::from_texts("a/search", &["y".to_string()]),
        ];
        save_baseline(&path, &entries).expect("应能写基线");

        let loaded = load_baseline(&path).expect("应能读回");
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains_key("a/search"));
        assert!(loaded.contains_key("b/search"));

        // 文件应按 case 排序（可 diff）
        let text = std::fs::read_to_string(&path).expect("应能读文件");
        let a_at = text.find("a/search").expect("应含 a/search");
        let b_at = text.find("b/search").expect("应含 b/search");
        assert!(a_at < b_at, "基线文件应按 case 排序，便于 diff");
    }

    #[test]
    fn 缺基线文件时返回空表() {
        let dir = tempfile::tempdir().expect("应能建临时目录");
        let path = dir.path().join("不存在.json");
        let loaded = load_baseline(&path).expect("缺文件不是错误（首轮）");
        assert!(loaded.is_empty());
    }

    #[test]
    fn 报告含回退逐条() {
        let baseline = Fingerprint::from_texts("c", &["a".to_string(), "b".to_string()]);
        let current = Fingerprint::from_texts("c", &["a".to_string()]);
        let summary = ReportSummary {
            comparisons: vec![CaseComparison {
                case: "c".to_string(),
                regressed: current.regressed_against(&baseline),
                baseline: Some(baseline),
                current,
            }],
            first_run: false,
            baseline_path: None,
            baseline_revision: None,
        };
        assert!(summary.has_regressions());

        let rendered = render_report(&summary);
        let parsed: serde_json::Value = serde_json::from_str(&rendered).expect("应是合法 JSON");
        assert_eq!(parsed["regressed"].as_u64(), Some(1));
        assert_eq!(parsed["total"].as_u64(), Some(1));
        assert!(
            parsed["regressions"]
                .as_array()
                .is_some_and(|list| !list.is_empty()),
            "报告应逐条列出回退"
        );
    }
}
