//! [DONE] 源可用性检查（tasks.md T071 / FR-011）
//!
//! # "可判定的结论"是什么意思
//!
//! FR-011 原文：源可用性检查须"给出**可判定**的结论"。这句话排除了
//! "输出一段日志让人自己看"这种形态——界面要能据此给源打上"可用/不可用"
//! 的标记、CLI 要能据此给出退出码、批量检查要能统计"100 个源里 73 个可用"。
//!
//! 故结论是 [`CheckVerdict`]（`tome-core` 的接缝枚举），而判定必须**只用
//! 一次搜索能否产出结构化结果**这一条标准——不掺"响应时间是否够快"
//! 这类需要主观阈值的判断。
//!
//! # 检查什么：一次最小搜索
//!
//! 只发**一次**请求（搜索），不发详情/目录/正文。理由：
//!
//! | 方案 | 每源请求数 | 问题 |
//! |---|---|---|
//! | 只查首页可达性 | 1 | 首页 200 不代表搜索规则还能用（源改版后首页照常） |
//! | 走完五类动作 | 5+ | 100 个源的批量检查要 500+ 请求，用户等不起 |
//! | **一次搜索 + 规则解析** | **1** | 搜不到书也可能是"这个关键词没书" |
//!
//! 最后一行是真实的局限，故判定要区分三种结局（见 [`CheckVerdict`] 的
//! 判定口径说明）：规则跑通但没结果算**可用**（源是好的，只是这个关键词
//! 没书）；规则报错才算不可用。
//!
//! # 批量检查是流式的（A6）
//!
//! 100 个源的检查要发 100 次请求。返回 `Vec` 会把"检查进度"藏起来，
//! 用户面对一个长时间无反馈的界面。故 [`check_sources`] 返回流——逐源
//! 产出，可取消，单源失败不终止整体。

use tome_core::seam::{CheckReport, CheckStream, CheckVerdict, SourceCheckFilter};
use tome_core::{CancelToken, ErrorKind, NetHost, Source};

use crate::error::SourceError;
use crate::source::ParsedSource;
use tome_core::SourceKind;

/// 检查用的搜索关键词。
///
/// # 为什么用一个"必然有结果"的常见词
///
/// 检查要判断的是**规则是否还能跑通**，而不是"这个词有没有书"。用
/// 一个通常有结果的词，能把"规则坏了"与"这个词没书"更好地区分开。
///
/// 但**不能依赖这一点**——没有哪个词保证每个源都有结果（玄幻源可能没有
/// 某本言情书）。故判定逻辑（见 [`verdict_from_search`]）把"跑通但无结果"
/// 也算可用。
///
/// [WORKAROUND] 硬编码常量：若日后要做"按源类型选关键词"（玄幻源用玄幻词），
/// 应改为按 `SourceKind` 或源自定义字段选择，而非继续改这一个常量。
pub const CHECK_KEYWORD: &str = "的";

/// 检查单个书源（FR-011）。
///
/// [DONE] T071 的单源入口。
pub fn check_source(parsed: &ParsedSource, host: &dyn NetHost) -> Result<CheckReport, SourceError> {
    let verdict = check_source_verdict(parsed, host);
    Ok(CheckReport {
        source: parsed.source.clone(),
        verdict,
    })
}

/// 单源检查的判定（`CheckReport` 的核）。
///
/// 不返回 `Result`：检查本身就产出一个"可用/不可用"的结论，
/// 而"检查过程出错"就是"不可用"的一种——把它做成 `Err` 会让调用方
/// 还要处理"我检查不了它"这个第三态，而 FR-011 只要两态。
pub fn check_source_verdict(parsed: &ParsedSource, host: &dyn NetHost) -> CheckVerdict {
    // 纯脚本源：走 mainJs 的入口约定
    if parsed.source.kind == SourceKind::Js {
        return check_script_source(parsed);
    }

    // 规则源：至少要有 searchUrl 与 ruleSearch 才谈得上"可用"
    if parsed.source.search_url.is_none() {
        return CheckVerdict::Unavailable {
            reason: ErrorKind::Rule,
        };
    }
    if parsed.source.rule_search.is_none() {
        return CheckVerdict::Unavailable {
            reason: ErrorKind::Rule,
        };
    }

    match crate::search::search_one_source(parsed, CHECK_KEYWORD, 1, host) {
        Ok(_hits) => {
            // ## 跑通但没结果也算可用
            //
            // "这个关键词在这个源没书"与"这个源的规则坏了"是两回事。
            // 前者是正常的（源上线了、规则能跑），后者才是不可用。
            // 把前者判成不可用会让检查结果大面积误报——尤其对小众源。
            CheckVerdict::Available
        }
        Err(error) => CheckVerdict::Unavailable {
            reason: error.to_error_kind(),
        },
    }
}

/// 纯脚本源的检查（不实际执行脚本——那需要建沙箱，代价过高）。
///
/// # 为什么纯脚本源的检查比规则源浅
///
/// 规则源的检查要真发一次请求（验证规则还能跑），而纯脚本源要验证的是
/// "脚本能否执行到出结果"——那需要建 QuickJS 沙箱并跑一遍脚本的
/// `search()`。在 100 个源的批量检查里为每个源建一次沙箱是**不可接受**
/// 的代价（每个沙箱都有初始化开销，且脚本可能真的发请求）。
///
/// 故此处只做**静态检查**（有没有 `js` 字段、能否过基本形状校验），
/// 并把它记为可用。真实执行留给"用户点开这个源"时。
///
/// `[UNCERTAIN]` 这会让"脚本语法错但字段齐全"的源被判为可用。若要更严
/// 的检查，应提供一个**可选**的深检模式（`--deep`）而不是默认加重批量
/// 检查的代价。
fn check_script_source(parsed: &ParsedSource) -> CheckVerdict {
    match parsed.source.js.as_deref() {
        Some(script) if !script.trim().is_empty() => CheckVerdict::Available,
        // 纯脚本源没有 js 字段 = 它什么也做不了
        _ => CheckVerdict::Unavailable {
            reason: ErrorKind::Rule,
        },
    }
}

/// 按过滤条件筛出要检查的源。
///
/// # 过滤在**检查之前**发生
///
/// 反过来的话（先全查再过滤）会为一个用户明确排除的源发请求——那既是
/// 浪费，也可能触发对方风控（100 个源的批量检查已经够重了）。
pub fn select_sources(sources: &[ParsedSource], filter: &SourceCheckFilter) -> Vec<ParsedSource> {
    sources
        .iter()
        .filter(|parsed| {
            // ① 分组过滤
            if let Some(group_id) = filter.group_id {
                if parsed.source.group_id != Some(group_id) {
                    return false;
                }
            }
            // ② id 过滤（空表示全部）
            if !filter.source_ids.is_empty() && !filter.source_ids.contains(&parsed.source.id) {
                return false;
            }
            // ③ 默认只检查**启用**的源
            //
            // 用户把源停用就是"别用它了"。检查停用的源既是浪费，
            // 也会在报告里制造"这些源不可用"的噪音——而它们本来就被
            // 用户主动停掉了。
            parsed.source.enabled
        })
        .cloned()
        .collect()
}

/// 批量检查（FR-011），**流式**返回（A6）。
///
/// [DONE] T071 的批量入口。
pub fn check_sources<'a>(
    sources: &'a [ParsedSource],
    filter: &SourceCheckFilter,
    host: &'a dyn NetHost,
) -> Box<dyn CheckStream + 'a> {
    Box::new(SourceCheckStream {
        pending: select_sources(sources, filter),
        host,
        token: CancelToken::new(),
    })
}

/// 逐个源产出的检查流。
///
/// # 为什么串行而不是并发
///
/// 与搜索流（`search.rs` 的 `SourceSearchStream`）同源的理由：A6 只要求
/// "增量返回 + 可取消 + 单源失败不终止"，串行已满足这三条。而并发会让
/// 100 个源同时打向各自的站点——那是**对第三方的压力**，且让同源限速
/// 的语义变复杂。串行下每个源的请求间隔由它自己声明的 `concurrentRate`
/// 控制，纪律更清楚。
struct SourceCheckStream<'a> {
    pending: Vec<ParsedSource>,
    host: &'a dyn NetHost,
    token: CancelToken,
}

impl CheckStream for SourceCheckStream<'_> {
    fn next_report(&mut self) -> Option<CheckReport> {
        // 取消后立即收尾（A6：实现方须尽快响应取消）
        if self.token.is_cancelled() {
            return None;
        }
        if self.pending.is_empty() {
            return None;
        }
        let parsed = self.pending.remove(0);
        // 单源检查**永不失败**（判定本身承载了失败），故这里无需 match Err
        Some(CheckReport {
            source: parsed.source.clone(),
            verdict: check_source_verdict(&parsed, self.host),
        })
    }

    fn cancel_token(&self) -> &CancelToken {
        &self.token
    }
}

/// 一次性收集检查结果（供 CLI 与测试用）。
///
/// 返回 `(可用的源, 不可用的源及原因)`——两类都要给调用方，
/// 因为"哪些源不可用"正是用户要处置的对象。
pub fn collect_check(
    mut stream: Box<dyn CheckStream + '_>,
) -> (Vec<Source>, Vec<(Source, ErrorKind)>) {
    let mut available = Vec::new();
    let mut unavailable = Vec::new();
    while let Some(report) = stream.next_report() {
        match report.verdict {
            CheckVerdict::Available => available.push(report.source),
            CheckVerdict::Unavailable { reason } => unavailable.push((report.source, reason)),
        }
    }
    (available, unavailable)
}

/// 检查结果的统计摘要（CLI 的人读输出与批量报告的载体）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckSummary {
    /// 可用的源数
    pub available: usize,
    /// 不可用的源数
    pub unavailable: usize,
}

impl CheckSummary {
    /// 总数。
    pub fn total(&self) -> usize {
        self.available + self.unavailable
    }

    /// 可用率（空集合定义为 `1.0`——没有源可查不是"全都坏了"）。
    pub fn available_rate(&self) -> f64 {
        if self.total() == 0 {
            return 1.0;
        }
        self.available as f64 / self.total() as f64
    }
}

/// 从检查结果算摘要。
pub fn summarize(available: usize, unavailable: usize) -> CheckSummary {
    CheckSummary {
        available,
        unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tome_core::{HeaderMap, RequestSpec, Response};

    const SEARCH_HTML: &str = r#"<html><body><ul>
      <li class="item"><h3 class="name"><a href="/book/1">斗破苍穹</a></h3></li>
    </ul></body></html>"#;

    fn source_json(id: &str, enabled: bool, extra: &str) -> String {
        format!(
            r#"{{"bookSourceName":"{id}","bookSourceUrl":"{id}","enabled":{enabled},
                 "searchUrl":"/s?q={{{{key}}}}",
                 "ruleSearch":"{{\"bookList\":\"@css:li.item\",\"name\":\"@css:.name@text\",\"bookUrl\":\"@css:.name>a@href\"}}"
                 {extra}}}"#
        )
    }

    fn parsed(text: &str) -> ParsedSource {
        crate::source::parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    /// 假宿主：按 URL 决定成败。
    struct FakeHost {
        status: u16,
        body: String,
        calls: Mutex<usize>,
    }

    impl FakeHost {
        fn ok() -> Self {
            Self {
                status: 200,
                body: SEARCH_HTML.to_string(),
                calls: Mutex::new(0),
            }
        }
        fn failing(status: u16) -> Self {
            Self {
                status,
                body: String::new(),
                calls: Mutex::new(0),
            }
        }
        fn call_count(&self) -> usize {
            *self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, _spec: RequestSpec) -> Result<Response, ErrorKind> {
            *self
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            Ok(Response {
                status: self.status,
                headers: HeaderMap::new(),
                body_bytes: self.body.as_bytes().to_vec(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    // --- 单源 ---

    #[test]
    fn 能搜出结果即可用() {
        let source = parsed(&source_json("https://a.com", true, ""));
        let host = FakeHost::ok();
        let report = check_source(&source, &host).expect("应成功");
        assert_eq!(report.verdict, CheckVerdict::Available);
        assert_eq!(host.call_count(), 1, "只发一次请求");
    }

    #[test]
    fn 跑通但无结果也算可用() {
        // "这个关键词在这个源没书"与"这个源规则坏了"是两回事。
        // 把前者判成不可用会让检查结果大面积误报。
        let source = parsed(&source_json("https://a.com", true, ""));
        let host = FakeHost {
            status: 200,
            body: "<html><body><ul></ul></body></html>".to_string(),
            calls: Mutex::new(0),
        };
        let report = check_source(&source, &host).expect("应成功");
        assert_eq!(
            report.verdict,
            CheckVerdict::Available,
            "规则跑通即可用（空结果是正常的）"
        );
    }

    #[test]
    fn 非_2xx_判为不可用且分类是网络() {
        let source = parsed(&source_json("https://a.com", true, ""));
        let host = FakeHost::failing(404);
        let report = check_source(&source, &host).expect("应成功");
        assert_eq!(
            report.verdict,
            CheckVerdict::Unavailable {
                reason: ErrorKind::Network
            }
        );
    }

    #[test]
    fn 缺_searchurl_判为不可用且分类是规则() {
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                       "ruleSearch":"{\"bookList\":\"@css:li\"}"}"#;
        let source = parsed(text);
        let host = FakeHost::ok();
        let report = check_source(&source, &host).expect("应成功");
        assert_eq!(
            report.verdict,
            CheckVerdict::Unavailable {
                reason: ErrorKind::Rule
            }
        );
        assert_eq!(host.call_count(), 0, "配置就不全时不该发请求");
    }

    #[test]
    fn 缺_rulesearch_判为不可用() {
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                       "searchUrl":"/s?q={{key}}"}"#;
        let source = parsed(text);
        let host = FakeHost::ok();
        let report = check_source(&source, &host).expect("应成功");
        assert_eq!(
            report.verdict,
            CheckVerdict::Unavailable {
                reason: ErrorKind::Rule
            }
        );
        assert_eq!(host.call_count(), 0);
    }

    #[test]
    fn 规则语法错判为不可用() {
        // 源改版后选择器失效——这是"不可用"的典型形态
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                       "searchUrl":"/s?q={{key}}",
                       "ruleSearch":"{\"bookList\":\"@css:(((\",\"name\":\"@css:a@text\"}"}"#;
        let source = parsed(text);
        let host = FakeHost::ok();
        let report = check_source(&source, &host).expect("应成功");
        assert!(
            matches!(report.verdict, CheckVerdict::Unavailable { .. }),
            "语法坏的规则应判不可用：{:?}",
            report.verdict
        );
    }

    #[test]
    fn 报告带上源本体() {
        // 上层要据报告在界面上定位那个源
        let source = parsed(&source_json("https://a.com", true, ""));
        let host = FakeHost::ok();
        let report = check_source(&source, &host).expect("应成功");
        assert_eq!(report.source.id, "https://a.com");
    }

    // --- 纯脚本源 ---

    #[test]
    fn 纯脚本源有脚本即可用() {
        let text = r#"{"bookSourceName":"JS","bookSourceUrl":"https://js.test",
                       "bookSourceType":1,
                       "js":"function search(k,p){ return []; }"}"#;
        let source = parsed(text);
        let host = FakeHost::ok();
        let report = check_source(&source, &host).expect("应成功");
        assert_eq!(report.verdict, CheckVerdict::Available);
        assert_eq!(
            host.call_count(),
            0,
            "纯脚本源只做静态检查（为 100 个源各建一次沙箱代价不可接受）"
        );
    }

    #[test]
    fn 纯脚本源无脚本判为不可用() {
        let text = r#"{"bookSourceName":"JS","bookSourceUrl":"https://js.test",
                       "bookSourceType":1}"#;
        let source = parsed(text);
        let host = FakeHost::ok();
        let report = check_source(&source, &host).expect("应成功");
        assert_eq!(
            report.verdict,
            CheckVerdict::Unavailable {
                reason: ErrorKind::Rule
            }
        );
    }

    // --- 过滤 ---

    #[test]
    fn 默认只检查启用的源() {
        // 用户把源停用就是"别用它了"。检查它既是浪费，也会在报告里
        // 制造"这些源不可用"的噪音。
        let enabled = parsed(&source_json("https://on.com", true, ""));
        let disabled = parsed(&source_json("https://off.com", false, ""));
        let selected = select_sources(&[enabled, disabled], &SourceCheckFilter::default());
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].source.id, "https://on.com");
    }

    #[test]
    fn 按_id_过滤() {
        let a = parsed(&source_json("https://a.com", true, ""));
        let b = parsed(&source_json("https://b.com", true, ""));
        let filter = SourceCheckFilter {
            source_ids: vec!["https://b.com".to_string()],
            group_id: None,
        };
        let selected = select_sources(&[a, b], &filter);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].source.id, "https://b.com");
    }

    #[test]
    fn 按分组过滤() {
        // 分组 id 是**存储层**分配的（`source_group` 是独立表），
        // 解析阶段拿不到（见 `source.rs` 里 `group_id: None` 的说明），
        // 故测试里直接设字段——这正是入库后的形状。
        let mut a = parsed(&source_json("https://a.com", true, ""));
        a.source.group_id = Some(1);
        let mut b = parsed(&source_json("https://b.com", true, ""));
        b.source.group_id = Some(2);

        let filter = SourceCheckFilter {
            source_ids: vec![],
            group_id: Some(1),
        };
        let selected = select_sources(&[a, b], &filter);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].source.id, "https://a.com");
    }

    #[test]
    fn 过滤在检查之前发生() {
        // 为一个用户明确排除的源发请求，既浪费也可能触发对方风控
        let a = parsed(&source_json("https://a.com", true, ""));
        let filter = SourceCheckFilter {
            source_ids: vec!["https://never.com".to_string()],
            group_id: None,
        };
        assert!(select_sources(&[a], &filter).is_empty());
    }

    // --- 流式批量 ---

    #[test]
    fn 批量检查逐源产出() {
        let sources = vec![
            parsed(&source_json("https://a.com", true, "")),
            parsed(&source_json("https://b.com", true, "")),
            parsed(&source_json("https://c.com", true, "")),
        ];
        let host = FakeHost::ok();
        let stream = check_sources(&sources, &SourceCheckFilter::default(), &host);

        let mut ids = Vec::new();
        let mut boxed = stream;
        while let Some(report) = boxed.next_report() {
            ids.push(report.source.id);
        }
        assert_eq!(ids.len(), 3);
        assert_eq!(host.call_count(), 3, "每个源一次请求");
    }

    #[test]
    fn 单源失败不终止流() {
        // A6：单源失败是流内的一个条目
        let sources = vec![
            parsed(&source_json("https://a.com", true, "")),
            parsed(&source_json("https://b.com", true, "")),
        ];
        // 宿主对所有请求返回 500
        let host = FakeHost::failing(500);
        let stream = check_sources(&sources, &SourceCheckFilter::default(), &host);
        let (available, unavailable) = collect_check(stream);
        assert!(available.is_empty());
        assert_eq!(unavailable.len(), 2, "两个源都应产出报告（而非流中断）");
    }

    #[test]
    fn 取消后立即收尾() {
        // A6：实现方须尽快响应取消，不应把已取消的流跑完
        let sources: Vec<ParsedSource> = (0..10)
            .map(|index| parsed(&source_json(&format!("https://s{index}.com"), true, "")))
            .collect();
        let host = FakeHost::ok();
        let mut stream = check_sources(&sources, &SourceCheckFilter::default(), &host);

        let first = stream.next_report().expect("应有第一个");
        assert_eq!(first.source.id, "https://s0.com");
        let calls_after_first = host.call_count();

        stream.cancel_token().cancel();
        assert!(
            stream.next_report().is_none(),
            "取消后不该再产出（应立即收尾）"
        );
        assert_eq!(host.call_count(), calls_after_first, "取消后不得再发请求");
    }

    #[test]
    fn 取消令牌可重复调用() {
        let sources = vec![parsed(&source_json("https://a.com", true, ""))];
        let host = FakeHost::ok();
        let stream = check_sources(&sources, &SourceCheckFilter::default(), &host);
        assert!(!stream.cancel_token().is_cancelled());
        stream.cancel_token().cancel();
        stream.cancel_token().cancel();
        assert!(stream.cancel_token().is_cancelled());
    }

    #[test]
    fn 空源列表产出空流() {
        let host = FakeHost::ok();
        let mut stream = check_sources(&[], &SourceCheckFilter::default(), &host);
        assert!(stream.next_report().is_none());
        assert_eq!(host.call_count(), 0);
    }

    #[test]
    fn 收集结果区分两类() {
        let sources = vec![
            parsed(&source_json("https://a.com", true, "")),
            parsed(&source_json("https://b.com", true, "")),
        ];
        let host = FakeHost::ok();
        let stream = check_sources(&sources, &SourceCheckFilter::default(), &host);
        let (available, unavailable) = collect_check(stream);
        assert_eq!(available.len(), 2);
        assert!(unavailable.is_empty());
    }

    // --- 摘要 ---

    #[test]
    fn 摘要计算() {
        let summary = summarize(7, 3);
        assert_eq!(summary.total(), 10);
        assert!((summary.available_rate() - 0.7).abs() < 1e-9);
    }

    #[test]
    fn 空集合的可用率是_1() {
        // 没有源可查不是"全都坏了"
        let summary = summarize(0, 0);
        assert_eq!(summary.total(), 0);
        assert_eq!(summary.available_rate(), 1.0);
    }

    #[test]
    fn 全不可用时的可用率是_0() {
        let summary = summarize(0, 5);
        assert_eq!(summary.available_rate(), 0.0);
    }
}
