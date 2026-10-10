//! [DONE] 排版合成语料（tasks.md T004）
//!
//! 与 001 的 `tome_rule::corpus`（`tests/corpus/`）同构：一组 JSON 用例，
//! 每条 = 输入 + 参数 + 期望 + **强制依据**。它是 S2-1 起各切片验收的载体，
//! 也是「期望值不靠实现者直觉填」这条纪律的执行点。
//!
//! ## 与 001 语料的三处**刻意不同**
//!
//! 1. **期望值不再是字符串列表，而是结构**（分页结构 / 规则命中 / 禁则例外）。
//!    排版的输出天然是结构化的（行盒、字符区间、例外记录），把它压成字符串
//!    再比对会丢掉"错在哪一位"的定位能力。
//! 2. **多了几何参数**：同一段正文在不同版心宽度下断点不同，参数是输入的一部分。
//!    不写参数就只能测"某种默认"，覆盖不到边界。
//! 3. **多了"禁则例外"这一类期望**：FR-008 走到硬序第 ④ 步（放弃禁则）时
//!    MUST 记录例外，供 SC-002 计数。语料因此要能表达"此处允许违反+原因"。
//!
//! ## `basis` 为什么仍是硬要求
//!
//! 排版规则依据的是**公开规范**（UAX#14 / LineBreak.txt / CLreq）与自有设计稿
//! （`design-system/MASTER.md`），不是上游实现。`basis` 字段让每条期望值都能
//! 追溯到具体条款——既支撑干净室纪律（宪法 II），也让日后"这条为什么这样"
//! 不必重新查一遍规范。
//!
//! 缺 `basis` 或 `basis` 为空的条目在 [`parse_corpus`] 处**被拒绝**，
//! 不是静默通过。这条约束交给类型系统与解析器，不靠代码审查。
//!
//! ## 语料格式
//!
//! 见 `crates/tome-reader/tests/corpus/README.md`。

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 排版语料的一个用例。
///
/// ## 字段存在性为什么比 001 语料强
///
/// 001 的 `CorpusCase` 全部字段必填。这里同样全部必填（`expect` 的三种形态
/// 用枚举表达而非 `Option` 堆叠）——**语料的完整性不该靠 `#[serde(default)]`
/// 兜底**：一条只写了输入忘了期望的用例，默认值会让它"通过"（空期望 = 空实际），
/// 从而伪装成有效覆盖。
#[derive(Debug, Clone, Deserialize)]
pub struct TypesettingCase {
    /// 唯一标识（冒号前缀标语法类别，如 `kinsoku:` / `squeeze:` / `mixed:`）
    pub name: String,
    /// 输入的构造方式
    pub input: CaseInput,
    /// 几何参数
    pub geometry: Geometry,
    /// 期望
    pub expect: CaseExpectation,
    /// **期望值的依据**（必填，见模块文档）
    pub basis: String,
}

/// 用例的输入。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CaseInput {
    /// 直接内联的正文
    Text {
        /// 段落文本序列（每条一个自然段；段内换行按 `\n` 展开由 S2-1 的
        /// 段落切分决定，本字段不预先切段）
        paragraphs: Vec<String>,
    },
}

impl CaseInput {
    /// 取全部段落文本。
    #[must_use]
    pub fn paragraphs(&self) -> &[String] {
        match self {
            Self::Text { paragraphs } => paragraphs,
        }
    }

    /// 拼接为完整正文（段间以 `\n` 连接）。
    ///
    /// S2-1 的段落切分（FR-001）以换行符为边界，故语料的"段落序列"在拼接后
    /// 必须仍能被切回同一序列——本函数与 [`Self::paragraphs`] 互为逆运算，
    /// 有专门的往返测试守住。
    #[must_use]
    pub fn joined_text(&self) -> String {
        self.paragraphs().join("\n")
    }
}

/// 用例的几何参数（只列会对断行结果产生影响的部分）。
///
/// 呈现参数（颜色/主题/字体族）**刻意不在其中**：FR-019 要求它们不影响分页，
/// 让它们出现在语料里会诱导出"主题相关的期望值"，与不变量冲突。
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Geometry {
    /// 版心宽度（px）
    pub measure_px: f32,
    /// 阅读字号（px）
    pub font_size_px: f32,
    /// 行高（字号倍数）
    pub line_height: f32,
    /// 段首缩进（em）；0 表示段间距模式
    pub paragraph_indent_em: f32,
}

impl Geometry {
    /// 单字身宽度（px）—— 中文全角字符的 `em` 就是字号本身。
    ///
    /// 断言里大量用到"半字身""整字身"，故把它作为一个具名派生量，
    /// 避免各处重复推导（也避免有人误用"版心宽 ÷ 字数"。
    #[must_use]
    pub fn em_px(&self) -> f32 {
        self.font_size_px
    }

    /// 单行可容纳的字身数（向上取整前的连续量）。
    #[must_use]
    pub fn chars_per_line(&self) -> f32 {
        if self.font_size_px <= 0.0 {
            return 0.0;
        }
        self.measure_px / self.font_size_px
    }
}

/// 用例的期望（三种形态之一）。
///
/// ## 为什么是枚举而非"三个可空字段"
///
/// 三者互斥且各自形态不同：混在一个结构里，读到的是"哪些字段非空"的推断，
/// 而不是"这条用例在断言什么类别"。枚举让类别在类型上可见，也让新增类别
/// 时有编译期提示（`match` 穷尽性）。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaseExpectation {
    /// 断言断行位置：每行的字符数（或断点索引）
    LineBreaks {
        /// 逐行的字符数（首行含缩进）
        chars_per_line: Vec<usize>,
    },
    /// 断言禁则：行首/行尾不得出现的字符集合
    Kinsoku {
        /// 不得出现在任何行**行首**的字符
        forbidden_at_line_start: Vec<String>,
        /// 不得出现在任何行**行尾**的字符
        forbidden_at_line_end: Vec<String>,
    },
    /// 断言硬序第 ④ 步的例外记录（FR-008：放弃禁则必须留痕）
    KinsokuExceptions {
        /// 例外条数（SC-002 的计数口径）
        count: usize,
        /// 逐条例外的规则标识（可枚举，便于定位是哪条规则被放弃）
        rule_ids: Vec<String>,
    },
    /// 断言挤压：某行的实际宽度相对版心的压缩量
    Squeeze {
        /// 逐行的压缩量（px），按行序
        compression_px: Vec<f32>,
    },
    /// 断言混排：中西文交界处的最小间隙
    MixedScript {
        /// 逐行是否超宽（任何一行 `true` 即违反 SC-003）
        overflow: Vec<bool>,
    },
}

impl CaseExpectation {
    /// 类别名（与 `name` 的冒号前缀对齐，用于分类统计）。
    #[must_use]
    pub fn category(&self) -> &'static str {
        match self {
            Self::LineBreaks { .. } => "line_breaks",
            Self::Kinsoku { .. } => "kinsoku",
            Self::KinsokuExceptions { .. } => "kinsoku_exceptions",
            Self::Squeeze { .. } => "squeeze",
            Self::MixedScript { .. } => "mixed_script",
        }
    }
}

/// 一条用例的装载/校验错误。
///
/// ## 为什么单独一个错误类型
///
/// 语料格式错误与"排版失败"是两类事：前者是**语料作者写错了**（应改文件），
/// 后者是**实现有问题**（应改代码）。混在一个错误里会让 CI 报错时指不清方向。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorpusError {
    /// JSON 语法或字段缺失（含缺 `basis`）
    Parse {
        /// 文件路径（可能在合并多文件时为空）
        source: Option<PathBuf>,
        /// 解析器给的细节
        detail: String,
    },
    /// 用例名重复
    DuplicateName {
        /// 重复的名字
        name: String,
    },
    /// `basis` 为空串——"有字段但没内容"是最常见的伪合规形态，须单独拦
    EmptyBasis {
        /// 用例名
        name: String,
    },
    /// `name` 缺类别前缀（如 `kinsoku:`）
    MissingCategoryPrefix {
        /// 用例名
        name: String,
    },
    /// 读到文件但目录为空
    EmptyDirectory {
        /// 目录路径
        path: PathBuf,
    },
}

impl fmt::Display for CorpusError {
    /// ## 为什么这里也是**标识 + 诊断**而不是中文文案
    ///
    /// T004 初版在这里写了中文提示（"语料格式错误：…""用例 `x` 的 basis 为空…"），
    /// 随后 T003 的 FR-058 扫描在 `cargo test --workspace` 下把它抓了出来。
    /// **扫描是对的、初版是错的**——FR-058 管的是"引擎产出的错误"，而
    /// `CorpusError` 经 `tome_reader::corpus` 的公开 API 产出，属引擎产物。
    ///
    /// "它是构建期/测试期工具不是运行时错误"这个辩解不成立：
    /// 分类是**上游要按它分支**的契约（哪个文件、哪条用例、哪条校验失败），
    /// 而文案是**给人看的**。把两者混在一句中文里，等于让上游无法稳定分支。
    ///
    /// 故改为与 [`crate::error`] 同构：**稳定标识在前、诊断细节在后**。
    /// 标识是可 grep 的契约，细节（路径、用例名、serde 报错原文）供人定位。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse { source, detail } => match source {
                Some(path) => write!(
                    formatter,
                    "error.reader.corpus.parse: {}: {detail}",
                    path.display()
                ),
                None => write!(formatter, "error.reader.corpus.parse: {detail}"),
            },
            Self::DuplicateName { name } => {
                write!(formatter, "error.reader.corpus.duplicate_name: {name}")
            }
            Self::EmptyBasis { name } => {
                write!(formatter, "error.reader.corpus.empty_basis: {name}")
            }
            Self::MissingCategoryPrefix { name } => write!(
                formatter,
                "error.reader.corpus.missing_category_prefix: {name}"
            ),
            Self::EmptyDirectory { path } => write!(
                formatter,
                "error.reader.corpus.empty_directory: {}",
                path.display()
            ),
        }
    }
}

impl CorpusError {
    /// 本错误的**可本地化标识**（稳定契约，供上层查文案表）。
    ///
    /// 与 [`crate::error::ReaderError::message_id`] 同一约定：标识是契约，
    /// 措辞不是——改措辞即破坏性变更。
    #[must_use]
    pub fn message_id(&self) -> &'static str {
        match self {
            Self::Parse { .. } => "error.reader.corpus.parse",
            Self::DuplicateName { .. } => "error.reader.corpus.duplicate_name",
            Self::EmptyBasis { .. } => "error.reader.corpus.empty_basis",
            Self::MissingCategoryPrefix { .. } => "error.reader.corpus.missing_category_prefix",
            Self::EmptyDirectory { .. } => "error.reader.corpus.empty_directory",
        }
    }
}

impl std::error::Error for CorpusError {}

/// 从 JSON 文本解析一组用例（含全部校验）。
///
/// ## 校验清单（每条都在测试里有反例）
///
/// | 校验 | 为什么 |
/// |---|---|
/// | `basis` 非空 | 空串是"字段填了没内容"的伪合规形态，必须与缺字段同等对待 |
/// | 用例名唯一 | 重名会让失败报告指不清是哪条 |
/// | 名字带类别前缀 | 分类统计按前缀分组；无前缀的用例会掉进 `other` 不被注意 |
///
/// ## 为什么"缺 `basis`"由 serde 拒、而"空 `basis`"由这里拒
///
/// 缺字段是**结构**问题，serde 的 `Deserialize` 天然报错，无需手写检查；
/// 空串是**内容**问题，serde 看不见。两者都要拒，故分两处——这不是重复。
pub fn parse_corpus(json: &str) -> Result<Vec<TypesettingCase>, CorpusError> {
    let cases: Vec<TypesettingCase> =
        serde_json::from_str(json).map_err(|error| CorpusError::Parse {
            source: None,
            detail: error.to_string(),
        })?;
    validate_cases(&cases)?;
    Ok(cases)
}

/// 校验一组已解析的用例（供多文件合并后统一校验）。
fn validate_cases(cases: &[TypesettingCase]) -> Result<(), CorpusError> {
    for (position, case) in cases.iter().enumerate() {
        if case.basis.trim().is_empty() {
            return Err(CorpusError::EmptyBasis {
                name: case.name.clone(),
            });
        }
        if !case.name.contains(':') {
            return Err(CorpusError::MissingCategoryPrefix {
                name: case.name.clone(),
            });
        }
        if cases[..position]
            .iter()
            .any(|other| other.name == case.name)
        {
            return Err(CorpusError::DuplicateName {
                name: case.name.clone(),
            });
        }
    }
    Ok(())
}

/// 取用例的类别（`name` 的冒号前缀）。
#[must_use]
pub fn category_of(case: &TypesettingCase) -> &str {
    case.name
        .split_once(':')
        .map_or("other", |(prefix, _)| prefix)
}

/// 读一个语料目录下的全部 `.json` 文件并合并。
///
/// ## 为什么按文件名排序
///
/// 合并顺序决定报告里的条目顺序。若不排序，文件系统返回顺序在不同平台上
/// 可能不同（SC-015 要求三平台一致的排版结果，虽然语料顺序不在其列，但
/// 报告顺序漂移会让 diff 难以比对）。按名排序是确定性的最小代价。
pub fn load_corpus_directory(directory: &Path) -> Result<Vec<TypesettingCase>, CorpusError> {
    let entries = std::fs::read_dir(directory).map_err(|error| CorpusError::Parse {
        source: Some(directory.to_path_buf()),
        detail: error.to_string(),
    })?;

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();

    if files.is_empty() {
        return Err(CorpusError::EmptyDirectory {
            path: directory.to_path_buf(),
        });
    }

    let mut all = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).map_err(|error| CorpusError::Parse {
            source: Some(file.clone()),
            detail: error.to_string(),
        })?;
        let mut cases: Vec<TypesettingCase> =
            serde_json::from_str(&text).map_err(|error| CorpusError::Parse {
                source: Some(file.clone()),
                detail: error.to_string(),
            })?;
        all.append(&mut cases);
    }

    // 跨文件重名在此拦——单文件内校验看不见它。
    validate_cases(&all)?;
    Ok(all)
}

/// 按类别统计（`类别 → 用例数`）。
#[must_use]
pub fn count_by_category(cases: &[TypesettingCase]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for case in cases {
        *counts.entry(category_of(case).to_owned()).or_insert(0) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
      {
        "name": "kinsoku:闭标点不入行首",
        "input": { "kind": "text", "paragraphs": ["测试。"] },
        "geometry": { "measure_px": 240.0, "font_size_px": 19.0, "line_height": 1.85, "paragraph_indent_em": 0.0 },
        "expect": { "kind": "kinsoku", "forbidden_at_line_start": ["，", "。"], "forbidden_at_line_end": ["（", "“"] },
        "basis": "UAX#14 LineBreak.txt 18.0.0：`，` 为 CL 类（不得置于行首）"
      },
      {
        "name": "squeeze:行末标点压缩",
        "input": { "kind": "text", "paragraphs": ["测试标点压缩。"] },
        "geometry": { "measure_px": 240.0, "font_size_px": 19.0, "line_height": 1.85, "paragraph_indent_em": 2.0 },
        "expect": { "kind": "squeeze", "compression_px": [0.0, 9.5] },
        "basis": "CLreq §3.1.5 标点挤压：行末标点占半字身"
      }
    ]"#;

    #[test]
    fn parses_valid_corpus() {
        let cases = parse_corpus(SAMPLE).expect("样例语料应能解析");
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].name, "kinsoku:闭标点不入行首");
        assert_eq!(category_of(&cases[0]), "kinsoku");
        assert_eq!(cases[0].expect.category(), "kinsoku");
        assert_eq!(cases[1].expect.category(), "squeeze");
    }

    /// **缺 `basis` 字段**必须被拒（serde 结构检查）。
    #[test]
    fn rejects_missing_basis_field() {
        let json = r#"[{
            "name": "kinsoku:x",
            "input": { "kind": "text", "paragraphs": ["a"] },
            "geometry": { "measure_px": 100.0, "font_size_px": 19.0, "line_height": 1.85, "paragraph_indent_em": 0.0 },
            "expect": { "kind": "kinsoku", "forbidden_at_line_start": [], "forbidden_at_line_end": [] }
        }]"#;
        let error = parse_corpus(json).expect_err("缺 basis 必须被拒");
        assert!(
            matches!(error, CorpusError::Parse { .. }),
            "实际：{error:?}"
        );
    }

    /// **`basis` 为空串**必须被拒——这是"字段填了没内容"的伪合规形态，
    /// serde 看不见它，故必须由 [`validate_cases`] 拦。
    #[test]
    fn rejects_empty_basis_string() {
        let json = r#"[{
            "name": "kinsoku:x",
            "input": { "kind": "text", "paragraphs": ["a"] },
            "geometry": { "measure_px": 100.0, "font_size_px": 19.0, "line_height": 1.85, "paragraph_indent_em": 0.0 },
            "expect": { "kind": "kinsoku", "forbidden_at_line_start": [], "forbidden_at_line_end": [] },
            "basis": "   "
        }]"#;
        let error = parse_corpus(json).expect_err("空 basis 必须被拒");
        assert_eq!(
            error,
            CorpusError::EmptyBasis {
                name: "kinsoku:x".to_owned()
            }
        );
    }

    #[test]
    fn rejects_duplicate_names() {
        let json = r#"[
          { "name": "kinsoku:dup",
            "input": { "kind": "text", "paragraphs": ["a"] },
            "geometry": { "measure_px": 100.0, "font_size_px": 19.0, "line_height": 1.85, "paragraph_indent_em": 0.0 },
            "expect": { "kind": "kinsoku", "forbidden_at_line_start": [], "forbidden_at_line_end": [] },
            "basis": "b" },
          { "name": "kinsoku:dup",
            "input": { "kind": "text", "paragraphs": ["b"] },
            "geometry": { "measure_px": 100.0, "font_size_px": 19.0, "line_height": 1.85, "paragraph_indent_em": 0.0 },
            "expect": { "kind": "kinsoku", "forbidden_at_line_start": [], "forbidden_at_line_end": [] },
            "basis": "b" }
        ]"#;
        let error = parse_corpus(json).expect_err("重名必须被拒");
        assert_eq!(
            error,
            CorpusError::DuplicateName {
                name: "kinsoku:dup".to_owned()
            }
        );
    }

    #[test]
    fn rejects_name_without_category_prefix() {
        let json = r#"[{
            "name": "无前缀",
            "input": { "kind": "text", "paragraphs": ["a"] },
            "geometry": { "measure_px": 100.0, "font_size_px": 19.0, "line_height": 1.85, "paragraph_indent_em": 0.0 },
            "expect": { "kind": "kinsoku", "forbidden_at_line_start": [], "forbidden_at_line_end": [] },
            "basis": "b"
        }]"#;
        let error = parse_corpus(json).expect_err("无前缀必须被拒");
        assert_eq!(
            error,
            CorpusError::MissingCategoryPrefix {
                name: "无前缀".to_owned()
            }
        );
    }

    /// 段落序列与拼接文本互为逆运算（S2-1 的段落切分以 `\n` 为边界）。
    #[test]
    fn paragraph_roundtrip_is_stable() {
        let input = CaseInput::Text {
            paragraphs: vec!["第一段".to_owned(), "第二段".to_owned()],
        };
        assert_eq!(input.joined_text(), "第一段\n第二段");
        assert_eq!(input.paragraphs().len(), 2);
    }

    #[test]
    fn geometry_derived_values() {
        let geometry = Geometry {
            measure_px: 684.0,
            font_size_px: 19.0,
            line_height: 1.85,
            paragraph_indent_em: 2.0,
        };
        assert!((geometry.em_px() - 19.0).abs() < f32::EPSILON);
        assert!((geometry.chars_per_line() - 36.0).abs() < f32::EPSILON);

        // 字号为 0 时不得除零（返回 0 而非 inf/NaN）——语料可能被写错，
        // 解析期不该崩，交给断言层报"覆盖率异常"。
        let degenerate = Geometry {
            font_size_px: 0.0,
            ..geometry
        };
        assert_eq!(degenerate.chars_per_line(), 0.0);
    }
}
