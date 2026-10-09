//! Jsoup ↔ html5ever DOM 差异量化（T008，research.md R2 遗留待办）
//!
//! ## 要回答的问题
//!
//! research.md R2 论证了「自建 arena DOM（实现 html5ever `TreeSink`）」的路线，
//! 但对 **Jsoup 与 html5ever 解析同一份乱网页时 DOM 是否一致**只有文档层面的
//! 推断。这是书源兼容性的地基：若两边树形分叉，"用 CSS 选择器取到的节点"就会
//! 系统性不同，兼容性无从谈起。本模块把它变成可量化数据。
//!
//! ## 为什么用「先序序列化签名」而不是结构化 diff
//!
//! spike 要的是**差异率**，不是 diff 文本。把两棵树各自序列化成一行行
//! 「缩进 + 节点类型 + 名称」的记录，再逐行比对，即可同时得到：
//!   - 差异行数 / 总行数 → 差异率
//!   - 首个差异行的上下文 → 系统性差异点的定位线索
//!
//! ## 两侧的序列化必须**同构**
//!
//! 差异率只有在两侧采用同一套输出约定时才有意义——任何一侧偏离都会制造假差异。
//! 约定见 [`SpikeDom::describe_node`]，Java 侧导出器（`tools/JsoupDomDump.java`）
//! 逐条对齐同一约定。
//!
//! ## 本模块的 DOM 是**最小实现**，不是 S1 的 tome-rule DOM
//!
//! 完整 DOM（含 CSS 选择器所需的 `Element` trait、节点类型细分 DataNode/CDataNode
//! 的完整语义）属 S1 的 T020。此处只实现序列化所需的最小 `TreeSink`，
//! 这正是 spike 应有的深度——但**结构形态（arena + 兄弟链）与 S1 保持一致**，
//! 以便本模块的结论能外推。

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{parse_document, Attribute, QualName};

/// 逐样本比对结果。
#[derive(Debug)]
pub struct SampleReport {
    /// 样本文件名
    pub name: String,
    /// Jsoup 侧序列化行数
    pub jsoup_lines: usize,
    /// html5ever 侧序列化行数
    pub html5ever_lines: usize,
    /// 逐行不一致的行数（含行数差异，按较长一侧对齐）
    pub differing_lines: usize,
    /// Rust 侧树中规范化后为空的文本节点数（与结构差异不同源，单列）
    pub html5ever_empty_text_nodes: usize,
    /// 首个差异的定位描述（无差异时为 `None`）
    pub first_difference: Option<String>,
}

impl SampleReport {
    /// 差异率 = 不一致行数 / 两侧行数的**较大者**。
    ///
    /// 为什么按较大者归一：按其中一侧归一会在两侧行数差得远时给出大于 1
    /// 或无意义的比率。取较大者能保证比率落在 `[0,1]`。
    pub fn difference_ratio(&self) -> f64 {
        let denominator = self.jsoup_lines.max(self.html5ever_lines);
        if denominator == 0 {
            // 两侧都空：差异率无定义。返回 0 而非 NaN——空样本的"一致"
            // 是事实（都没有输出），不该污染聚合统计。
            return 0.0;
        }
        self.differing_lines as f64 / denominator as f64
    }
}

/// 对比给定目录下的全部 `*.html` 样本，打印逐样本与汇总报告。
///
/// 期望语料目录内两侧产物的命名：
///   - `<name>.html`      —— 原始样本
///   - `<name>.jsoup.txt` —— Java 侧产出（由 `tools/JsoupDomDump.java` 生成）
///
/// html5ever 侧在本模块内当场解析，无需预生成文件。
pub fn run(corpus_directory: &str) -> Result<(), String> {
    let directory = Path::new(corpus_directory);
    if !directory.is_dir() {
        return Err(format!("语料目录不存在：{corpus_directory}"));
    }

    let samples = collect_samples(directory)?;
    if samples.is_empty() {
        return Err(format!("语料目录 {corpus_directory} 内没有 *.html 样本"));
    }

    let mut reports = Vec::with_capacity(samples.len());
    for sample in &samples {
        reports.push(compare_sample(sample)?);
    }

    print_report(&reports);
    Ok(())
}

/// 收集语料目录下的 `*.html` 样本，按文件名排序保证报告可复现。
fn collect_samples(directory: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("读取目录 {directory:?} 失败：{error}"))?;

    let mut samples = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("遍历目录项失败：{error}"))?;
        let path = entry.path();
        let is_html = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("html"));
        if is_html {
            samples.push(path);
        }
    }
    samples.sort();
    Ok(samples)
}

/// 比对单个样本：读 Jsoup 侧产物，当场跑 html5ever，逐行比对。
fn compare_sample(sample_path: &Path) -> Result<SampleReport, String> {
    let name = sample_path
        .file_name()
        .map(|file_name| file_name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("<unknown>"));

    let jsoup_dump_path = sample_path.with_extension("jsoup.txt");
    let jsoup_serialized = std::fs::read_to_string(&jsoup_dump_path).map_err(|error| {
        format!(
            "读取 Jsoup 侧产物 {jsoup_dump_path:?} 失败：{error}\n\
             （需先运行 tools/JsoupDomDump.java 生成该文件，见 spikes/s0/tools/README.md）"
        )
    })?;

    let html = std::fs::read_to_string(sample_path)
        .map_err(|error| format!("读取样本 {sample_path:?} 失败：{error}"))?;
    let (html5ever_serialized, html5ever_empty_text_nodes) = parse_with_stats(&html)?;

    let jsoup_lines = split_lines(&jsoup_serialized);
    let html5ever_lines = split_lines(&html5ever_serialized);

    let (differing_lines, first_difference) = line_diff(&jsoup_lines, &html5ever_lines);

    Ok(SampleReport {
        name,
        jsoup_lines: jsoup_lines.len(),
        html5ever_lines: html5ever_lines.len(),
        differing_lines,
        html5ever_empty_text_nodes,
        first_difference,
    })
}

/// 把序列化产物切成行，忽略空行。
fn split_lines(serialized: &str) -> Vec<&str> {
    serialized
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect()
}

/// 逐行比对，返回（不一致行数, 首个差异描述）。
fn line_diff(expected: &[&str], actual: &[&str]) -> (usize, Option<String>) {
    let differing = expected
        .iter()
        .zip(actual.iter())
        .filter(|(left, right)| left != right)
        .count();
    // 行数不等时，多出来的部分全是差异——否则"一侧少了几十个节点"
    // 只会被记成 1 行差异，严重低估。
    let length_gap = expected.len().abs_diff(actual.len());

    let first = expected
        .iter()
        .zip(actual.iter())
        .enumerate()
        .find(|(_, (left, right))| left != right)
        .map(|(index, (left, right))| {
            format!(
                "第 {} 行：jsoup={:?} / html5ever={:?}",
                index + 1,
                left,
                right
            )
        })
        .or_else(|| {
            (length_gap > 0).then(|| {
                format!(
                    "行数不同：jsoup={} 行 / html5ever={} 行（前 {} 行一致）",
                    expected.len(),
                    actual.len(),
                    expected.len().min(actual.len())
                )
            })
        });

    (differing + length_gap, first)
}

/// 打印逐样本与汇总报告（Markdown 表格，可直接粘进 BASELINE.md）。
fn print_report(reports: &[SampleReport]) {
    println!();
    println!("| 样本 | jsoup 行 | html5ever 行 | 差异行 | 差异率 | html5ever 空文本节点 |");
    println!("|---|---|---|---|---|---|");
    for report in reports {
        println!(
            "| {} | {} | {} | {} | {:.1}% | {} |",
            report.name,
            report.jsoup_lines,
            report.html5ever_lines,
            report.differing_lines,
            report.difference_ratio() * 100.0,
            report.html5ever_empty_text_nodes
        );
    }

    let total_differing: usize = reports.iter().map(|report| report.differing_lines).sum();
    let total_lines: usize = reports
        .iter()
        .map(|report| report.jsoup_lines.max(report.html5ever_lines))
        .sum();
    let overall = if total_lines == 0 {
        0.0
    } else {
        total_differing as f64 / total_lines as f64
    };
    println!();
    println!(
        "汇总：{} 个样本，{} / {} 行不一致，总体差异率 {:.2}%",
        reports.len(),
        total_differing,
        total_lines,
        overall * 100.0
    );

    let with_differences: Vec<&SampleReport> = reports
        .iter()
        .filter(|report| report.first_difference.is_some())
        .collect();
    if with_differences.is_empty() {
        println!("所有样本的 DOM 序列化完全一致（差异率 0%）");
        return;
    }

    println!();
    println!("首个差异（前 10 个样本）：");
    for report in with_differences.iter().take(10) {
        if let Some(difference) = &report.first_difference {
            println!("  - {} → {}", report.name, difference);
        }
    }
}

/// 调试入口：把单个 HTML 文件的 html5ever 侧序列化打印到 stdout。
///
/// 存在的理由：差异率是一个聚合数字，它无法区分"两侧解析真的不同"与
/// "两侧序列化约定不同"。排查后者必须能并排看两侧原始输出。
pub fn dump_file(file_path: &str) -> Result<(), String> {
    let html = std::fs::read_to_string(file_path)
        .map_err(|error| format!("读取 {file_path} 失败：{error}"))?;
    print!("{}", serialize_html5ever(&html)?);
    Ok(())
}

/// 用 html5ever 解析并序列化（T008 的 Rust 侧入口）。
pub fn serialize_html5ever(html: &str) -> Result<String, String> {
    let arena = typed_arena_lite();
    let dom = SpikeDom::parse(html, &arena);
    Ok(dom.serialize())
}

/// 用 html5ever 解析并返回（序列化文本, 空文本节点数）。
///
/// 空文本节点数单独返回：它与结构差异不同源（Jsoup 保留空白文本节点、
/// html5ever 多数字置丢弃），混入差异率会污染该指标。分开报告既不丢信息，
/// 又让差异率只反映**结构性**兼容性。
pub fn parse_with_stats(html: &str) -> Result<(String, usize), String> {
    let arena = typed_arena_lite();
    let dom = SpikeDom::parse(html, &arena);
    let empty_text_nodes = dom.count_empty_text_nodes();
    Ok((dom.serialize(), empty_text_nodes))
}

// ---------------------------------------------------------------------------
// 最小 arena DOM
// ---------------------------------------------------------------------------

/// arena 存储。
///
/// ## 为什么自建 arena 而不用 `typed_arena` crate
///
/// spike 对 arena 的唯一需求是"能分配稳定地址的节点并活到解析结束"。
/// 用一个 `RefCell<Vec<Box<Node>>>` 即可满足——好处是**不引入额外依赖**，
/// 让 T007 的体积差值是纯粹的"rquickjs 代价"。
///
/// ## 为什么节点用裸指针互相引用（此处需要 unsafe 吗？）
///
/// **不需要**。html5ever 的 `TreeSink::Handle` 只需 `Clone`。这里选
/// `Rc<Node>` 作 handle：Rc 的指针相等即节点相等，父子/兄弟链都能安全表达，
/// 且解析结束（`finish`）后整棵树由 Rc 引用计数自动释放。
/// 这与 `examples/arena.rs` 用 arena 生引用的思路等价，但无需 unsafe——
/// spike 更看重"能安全跑通"，性能留到 S1 用真正的 arena 优化。
///
/// `SpikeDom` 只是 `TreeSink` 的实现载体（承载 quirk mode 等解析器状态），
/// 树本身由 `Rc<Node>` 承载。
#[derive(Default)]
struct SpikeArena;

/// 兼容早期写法的占位——实际树存储见 [`Node`] 的 `Rc` 链。
fn typed_arena_lite() -> SpikeArena {
    SpikeArena
}

/// DOM 节点。字段用 `Cell`/`RefCell` 承载可变性——`TreeSink` 的方法签名是
/// `&self`（解析器持不可变引用），故可变性必须内化。这是 html5ever 的既定
/// 契约，不是本实现的取舍。
struct Node {
    /// 父节点。用 `RefCell` 而非 `Cell`：`Cell::get` 要求 `Copy`，而 `Rc` 不是
    /// `Copy`（`Cell::take` 又会破坏兄弟链的共享引用）。`RefCell` + 显式
    /// `clone` 是这里唯一安全的表达，代价仅为一次引用计数递增。
    parent: RefCell<Option<Rc<Node>>>,
    /// 前一个兄弟
    previous_sibling: RefCell<Option<Rc<Node>>>,
    /// 后一个兄弟
    next_sibling: RefCell<Option<Rc<Node>>>,
    /// 第一个子节点
    first_child: RefCell<Option<Rc<Node>>>,
    /// 最后一个子节点（`append` 需要 O(1) 取尾）
    last_child: RefCell<Option<Rc<Node>>>,
    /// 节点负载
    data: NodeData,
}

/// 节点负载。
///
/// **刻意区分 `Text` / `Data` / `CData`**——research.md R2 指出 Jsoup 有
/// DataNode/CDataNode 节点类型（`:containsData()` / `::data` 语义需要），
/// 而 scraper 一类库把它们压平成 Text。保留类型细分正是自建 DOM 的理由之一。
enum NodeData {
    /// 文档根
    Document,
    /// 元素
    Element {
        /// 元素名（含命名空间）
        name: QualName,
        /// 属性（解析期会被 `add_attrs_if_missing` 追加）
        attributes: RefCell<Vec<Attribute>>,
        /// `<template>` 的内容片段（html5ever 契约要求独立承载）
        template_contents: Option<Rc<Node>>,
        /// MathML 注解 XML 集成点标记（外部内容处理用）
        mathml_annotation_xml_integration_point: bool,
    },
    /// 文本节点（对应 Jsoup 的 TextNode）
    Text {
        /// 文本内容，解析期会被拼接追加
        contents: RefCell<StrTendril>,
    },
    /// 数据节点（对应 Jsoup 的 DataNode）：`<script>` / `<style>` 的内容。
    ///
    /// **为什么必须与 `Text` 区分**：实测（spike 语料 05）Jsoup 把脚本/样式内容
    /// 建模为 `DataNode`（`nodeName()` 为 `#data`），html5ever 一律走 `Text`。
    /// 书源的 `::data` / `:containsData()` 选择器依赖这个区分（research.md R2
    /// 把"保留节点类型细分"列为自建 DOM 的理由）。
    Data {
        /// 数据内容
        contents: RefCell<StrTendril>,
    },
    /// 注释
    Comment {
        /// 注释内容
        contents: StrTendril,
    },
    /// 文档类型声明
    Doctype {
        /// 名称
        name: StrTendril,
    },
    /// 处理指令（HTML 中罕见，但 trait 要求提供承载）
    ProcessingInstruction,
}

impl Node {
    fn new(data: NodeData) -> Self {
        Self {
            parent: RefCell::new(None),
            previous_sibling: RefCell::new(None),
            next_sibling: RefCell::new(None),
            first_child: RefCell::new(None),
            last_child: RefCell::new(None),
            data,
        }
    }

    /// 该节点是否为承载原始数据内容的元素（`<script>` / `<style>`）。
    ///
    /// 依据公开事实而非上游实现：HTML 规范把这两个元素的内容定义为
    /// "raw text / raw text element"；Jsoup 的公开 API 文档亦说明其内容
    /// 以 `DataNode` 承载。两者一致，故此处按元素名判定。
    fn is_data_element(&self) -> bool {
        match &self.data {
            NodeData::Element { name, .. } => {
                let local: &str = name.local.as_ref();
                local.eq_ignore_ascii_case("script") || local.eq_ignore_ascii_case("style")
            }
            _ => false,
        }
    }

    /// 从当前父节点上摘下自己，并修复兄弟链与父节点的首尾指针。
    fn detach(&self) {
        let parent = self.parent.borrow_mut().take();
        let previous_sibling = self.previous_sibling.borrow_mut().take();
        let next_sibling = self.next_sibling.borrow_mut().take();

        if let Some(ref next) = next_sibling {
            *next.previous_sibling.borrow_mut() = previous_sibling.clone();
        } else if let Some(parent_node) = &parent {
            *parent_node.last_child.borrow_mut() = previous_sibling.clone();
        }

        if let Some(previous) = previous_sibling {
            *previous.next_sibling.borrow_mut() = next_sibling;
        } else if let Some(parent_node) = &parent {
            *parent_node.first_child.borrow_mut() = next_sibling;
        }
    }

    /// 追加为最后一个子节点（若已有父节点则先摘下）。
    fn append(self: &Rc<Self>, new_child: &Rc<Self>) {
        new_child.detach();
        *new_child.parent.borrow_mut() = Some(Rc::clone(self));

        match self.last_child.borrow_mut().take() {
            Some(last) => {
                *new_child.previous_sibling.borrow_mut() = Some(Rc::clone(&last));
                *last.next_sibling.borrow_mut() = Some(Rc::clone(new_child));
            }
            None => {
                *self.first_child.borrow_mut() = Some(Rc::clone(new_child));
            }
        }
        *self.last_child.borrow_mut() = Some(Rc::clone(new_child));
    }

    /// 插入为本节点的前一个兄弟。
    fn insert_before(self: &Rc<Self>, new_sibling: &Rc<Self>) {
        new_sibling.detach();
        *new_sibling.parent.borrow_mut() = self.parent.borrow().clone();
        *new_sibling.next_sibling.borrow_mut() = Some(Rc::clone(self));

        match self.previous_sibling.borrow_mut().take() {
            Some(previous) => {
                *new_sibling.previous_sibling.borrow_mut() = Some(Rc::clone(&previous));
                *previous.next_sibling.borrow_mut() = Some(Rc::clone(new_sibling));
            }
            None => {
                let parent = self.parent.borrow().clone();
                if let Some(parent_node) = parent {
                    *parent_node.first_child.borrow_mut() = Some(Rc::clone(new_sibling));
                }
            }
        }
        *self.previous_sibling.borrow_mut() = Some(Rc::clone(new_sibling));
    }
}

/// `TreeSink` 的实现载体。
struct SpikeDom {
    /// 文档根节点
    document: Rc<Node>,
    /// 怪癖模式（解析器写入）
    quirks_mode: RefCell<QuirksMode>,
}

impl SpikeDom {
    /// 解析 HTML 为 DOM 树。
    fn parse(html: &str, _arena: &SpikeArena) -> Rc<Node> {
        let document = Rc::new(Node::new(NodeData::Document));
        let sink = SpikeDom {
            document: Rc::clone(&document),
            quirks_mode: RefCell::new(QuirksMode::NoQuirks),
        };

        parse_document(sink, Default::default()).one(html)
    }

    fn new_node(&self, data: NodeData) -> Rc<Node> {
        Rc::new(Node::new(data))
    }

    /// `append` 与 `append_before_sibling` 的公共逻辑。
    ///
    /// **关键点 1：相邻文本节点合并**。这是 `TreeSink::append` 的契约要求
    /// （"If this would produce adjacent sibling text nodes, it should
    /// concatenate the text instead"）。不合并会让字符引用（`&amp;` 之类）
    /// 产生"一个字符一个文本节点"的碎片，两侧行数完全不可比。
    ///
    /// **关键点 2：脚本/样式内容建模为 `Data` 而非 `Text`**。html5ever 把
    /// `<script>`/`<style>` 的文本内容也走 `append` 的文本路径，但 Jsoup 将其
    /// 建模为 `DataNode`。此处按**父元素名**修正（由调用方判定并传入），
    /// 使两侧节点类型可比——否则每个含脚本的页面都会产生固定差异
    /// （实测语料 05/07 即此）。
    fn append_common<P, A>(
        &self,
        child: NodeOrText<Rc<Node>>,
        previous: P,
        inside_data_element: bool,
        append: A,
    ) where
        P: FnOnce() -> Option<Rc<Node>>,
        A: FnOnce(Rc<Node>),
    {
        let previous_node = previous();

        let new_node = match child {
            NodeOrText::AppendText(text) => {
                if let Some(existing) = previous_node {
                    match &existing.data {
                        NodeData::Text { contents } if !inside_data_element => {
                            contents.borrow_mut().push_tendril(&text);
                            return;
                        }
                        NodeData::Data { contents } if inside_data_element => {
                            contents.borrow_mut().push_tendril(&text);
                            return;
                        }
                        _ => {}
                    }
                }
                if inside_data_element {
                    self.new_node(NodeData::Data {
                        contents: RefCell::new(text),
                    })
                } else {
                    self.new_node(NodeData::Text {
                        contents: RefCell::new(text),
                    })
                }
            }
            NodeOrText::AppendNode(node) => node,
        };
        append(new_node);
    }
}

impl TreeSink for SpikeDom {
    type Handle = Rc<Node>;
    type Output = Rc<Node>;
    type ElemName<'a>
        = &'a QualName
    where
        Self: 'a;

    fn finish(self) -> Rc<Node> {
        self.document
    }

    fn parse_error(&self, _message: Cow<'static, str>) {
        // spike 不收集解析错误：目标是对比**树形**，不是对比错误报告。
        // S1 的实现需要把它们暴露给规则求值轨迹（FR-037）。
    }

    fn get_document(&self) -> Rc<Node> {
        Rc::clone(&self.document)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        *self.quirks_mode.borrow_mut() = mode;
    }

    /// 节点身份即指针相等（`Rc` 的语义）。html5ever 靠这个判断
    /// "两个 handle 是否同一个节点"，是整棵树的身份基础。
    fn same_node(&self, left: &Rc<Node>, right: &Rc<Node>) -> bool {
        Rc::ptr_eq(left, right)
    }

    fn elem_name<'a>(&'a self, target: &'a Rc<Node>) -> &'a QualName {
        match &target.data {
            NodeData::Element { name, .. } => name,
            // trait 契约保证只在元素节点上调用（见 markup5ever 的文档注释：
            // "Should never be called on a non-element node; feel free to panic!"）
            _ => panic!("elem_name 只在元素节点上调用"),
        }
    }

    fn get_template_contents(&self, target: &Rc<Node>) -> Rc<Node> {
        if let NodeData::Element {
            template_contents: Some(contents),
            ..
        } = &target.data
        {
            Rc::clone(contents)
        } else {
            // 同上：trait 承诺只会用 `<template>` 元素调用
            panic!("get_template_contents 只在 template 元素上调用")
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, target: &Rc<Node>) -> bool {
        match &target.data {
            NodeData::Element {
                mathml_annotation_xml_integration_point,
                ..
            } => *mathml_annotation_xml_integration_point,
            _ => false,
        }
    }

    fn create_element(
        &self,
        name: QualName,
        attributes: Vec<Attribute>,
        flags: ElementFlags,
    ) -> Rc<Node> {
        self.new_node(NodeData::Element {
            name,
            attributes: RefCell::new(attributes),
            template_contents: if flags.template {
                Some(self.new_node(NodeData::Document))
            } else {
                None
            },
            mathml_annotation_xml_integration_point: flags.mathml_annotation_xml_integration_point,
        })
    }

    fn create_comment(&self, text: StrTendril) -> Rc<Node> {
        self.new_node(NodeData::Comment { contents: text })
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Rc<Node> {
        // HTML 文档里 PI 会被解析器丢弃；用无内容节点承载以免丢 handle。
        self.new_node(NodeData::ProcessingInstruction)
    }

    fn append(&self, parent: &Rc<Node>, child: NodeOrText<Rc<Node>>) {
        // 从**父节点**判定是否处于 script/style 内——不能用"前一个兄弟"推断：
        // 当内容是该元素的第一个子节点时（语料 05/07 正是如此），前一个兄弟是
        // `<script>` 标签之前的节点，会误判为普通文本。
        let inside_data_element = parent.is_data_element();
        self.append_common(
            child,
            || parent.last_child.borrow().clone(),
            inside_data_element,
            |new_node| parent.append(&new_node),
        );
    }

    fn append_before_sibling(&self, sibling: &Rc<Node>, child: NodeOrText<Rc<Node>>) {
        // 兄弟链上的插入同样以父节点为判据（script/style 内容极少走这条路径，
        // 但保持同一判据可避免两处逻辑分叉）
        let inside_data_element = sibling
            .parent
            .borrow()
            .as_ref()
            .is_some_and(|parent| parent.is_data_element());
        self.append_common(
            child,
            || sibling.previous_sibling.borrow().clone(),
            inside_data_element,
            |new_node| sibling.insert_before(&new_node),
        );
    }

    fn append_based_on_parent_node(
        &self,
        element: &Rc<Node>,
        previous_element: &Rc<Node>,
        child: NodeOrText<Rc<Node>>,
    ) {
        // foster parenting（表格内错位内容的处置）。**完整实现**——
        // 表格错位正是本次语料要覆盖的典型乱网页场景，此处简化会直接
        // 掩盖 R2 关心的差异。
        if element.parent.borrow().is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(previous_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
        // 只保留名称：两侧序列化都只输出名称（Jsoup 的 DocumentType 同样
        // 只在输出里体现 name），保留 public/system id 反而会制造假差异。
        let doctype = self.new_node(NodeData::Doctype { name });
        self.document.append(&doctype);
    }

    fn add_attrs_if_missing(&self, target: &Rc<Node>, attributes: Vec<Attribute>) {
        if let NodeData::Element {
            attributes: existing,
            ..
        } = &target.data
        {
            let mut existing = existing.borrow_mut();
            let existing_names: HashSet<_> =
                existing.iter().map(|item| item.name.clone()).collect();
            existing.extend(
                attributes
                    .into_iter()
                    .filter(|attribute| !existing_names.contains(&attribute.name)),
            );
        }
    }

    fn remove_from_parent(&self, target: &Rc<Node>) {
        target.detach();
    }

    fn reparent_children(&self, node: &Rc<Node>, new_parent: &Rc<Node>) {
        let mut next_child = node.first_child.borrow().clone();
        while let Some(child) = next_child {
            next_child = child.next_sibling.borrow().clone();
            new_parent.append(&child);
        }
    }
}

// ---------------------------------------------------------------------------
// 序列化
// ---------------------------------------------------------------------------

/// 先序遍历序列化，产出一行一节点的文本。
///
/// ## 输出约定（必须与 `tools/JsoupDomDump.java` 逐条一致）
///
/// - 每行 `<深度>|<类型字母>|<名称或内容>`，深度从 0 起
/// - 类型字母：`E`=元素 / `T`=文本 / `X`=注释 / `O`=文档类型 / `P`=处理指令
/// - 元素名小写；属性按名排序后以 `name=value` 用逗号相连（无属性则为空段）
/// - 文本内容折叠连续空白并去首尾——两侧都要做，否则源码缩进产生的空白
///   差异会淹没真正的结构性差异
///
/// 根 `Document` 节点**不出行**：两侧的"根"概念对应关系不直观，
/// 输出它只会引入无谓的对齐噪音。
impl SerializeDom for Rc<Node> {
    fn serialize(&self) -> String {
        let mut output = String::new();
        self.write_preorder(0, &mut output);
        output
    }
}

/// 序列化能力（用 trait 承载，便于后续把同一约定复用到其他 DOM 形态）。
trait SerializeDom {
    /// 产出先序序列化文本。
    fn serialize(&self) -> String;
}

impl Node {
    fn write_preorder(&self, depth: usize, output: &mut String) {
        if let Some(line) = self.describe_node(depth) {
            output.push_str(&line);
            output.push('\n');
        }

        // `<template>` 的内容在 html5ever 中挂在独立的内容片段节点上（见
        // `create_element` 的 `flags.template` 分支）。Jsoup 则把模板内容当作
        // 模板元素的普通子节点（实测确认）。此处把内容片段**内联**到模板元素下，
        // 使两侧树形可比——否则模板内容在 Rust 侧永远不出现，产生假差异。
        if let NodeData::Element {
            template_contents: Some(contents),
            ..
        } = &self.data
        {
            let mut child = contents.first_child.borrow().clone();
            while let Some(current) = child {
                current.write_preorder(depth + 1, output);
                child = current.next_sibling.borrow().clone();
            }
        }

        let mut child = self.first_child.borrow().clone();
        while let Some(current) = child {
            current.write_preorder(depth + 1, output);
            child = current.next_sibling.borrow().clone();
        }
    }

    /// 把单个节点渲染成一行；返回 `None` 表示该节点不出现在序列化中。
    fn describe_node(&self, depth: usize) -> Option<String> {
        let line = match &self.data {
            // 根文档不出行（理由见模块注释）
            NodeData::Document => return None,
            NodeData::Element { name, attributes, .. } => {
                let mut attribute_pairs: Vec<String> = attributes
                    .borrow()
                    .iter()
                    .map(|attribute| {
                        format!(
                            "{}={}",
                            attribute.name.local.to_ascii_lowercase(),
                            normalize_whitespace(&attribute.value)
                        )
                    })
                    .collect();
                attribute_pairs.sort();
                format!(
                    "{depth}|E|{}|{}",
                    name.local.to_ascii_lowercase(),
                    attribute_pairs.join(",")
                )
            }
            NodeData::Text { contents } => {
                let normalized = normalize_whitespace(&contents.borrow());
                // 规范化后为空的文本节点不出行。**这一步两侧必须同时做**：
                // Jsoup 会保留源码缩进产生的纯空白文本节点，而 html5ever 的
                // 树构造器在多数位置直接丢弃它们——这是真实的解析差异，但它是
                // "空白保留策略"差异而非"结构"差异。若不一并剔除，它会让每一行
                // 的下标错位，把差异率顶到 ~90%（实测），完全掩盖结构差异。
                // 该差异本身改由 `empty_text` 计数单独报告，不丢失信息。
                if normalized.is_empty() {
                    return None;
                }
                format!("{depth}|T|{normalized}")
            }
            NodeData::Comment { contents } => {
                format!("{depth}|X|{}", normalize_whitespace(contents))
            }
            NodeData::Data { contents } => {
                format!("{depth}|D|{}", normalize_whitespace(&contents.borrow()))
            }
            NodeData::Doctype { name } => format!("{depth}|O|{}", normalize_whitespace(name)),
            NodeData::ProcessingInstruction => format!("{depth}|P|"),
        };
        Some(line)
    }

    /// 统计树中规范化后为空的文本节点数。
    ///
    /// 为什么单独统计而不并进差异率：两侧对空白文本节点的保留策略系统性不同
    /// （Jsoup 保留、html5ever 多数字置丢弃，实测样本 01 为 5 vs 1）。
    /// 把它算进差异率会让这个数字反映"空白策略"而非"结构兼容性"。
    /// 它仍然是有价值的观测值——若某天真要对齐，这就是工作量。
    ///
    /// 当前由单元测试调用；保留为公开接口是因为它同时是可复用的诊断工具。
    pub fn count_empty_text_nodes(&self) -> usize {
        let own = matches!(
            &self.data,
            NodeData::Text { contents } if normalize_whitespace(&contents.borrow()).is_empty()
        ) as usize;

        let mut child = self.first_child.borrow().clone();
        let mut total = own;
        while let Some(current) = child {
            total += current.count_empty_text_nodes();
            child = current.next_sibling.borrow().clone();
        }
        total
    }
}

/// 折叠连续空白并去首尾。
///
/// ## 为什么不能直接用 `char::is_whitespace`
///
/// Rust 的 `char::is_whitespace` 用的是 Unicode `White_Space` 属性，它把
/// **U+00A0 不换行空格**也算作空白；而 HTML/Java 侧的空白定义（`Character.isWhitespace`）
/// **不含** U+00A0。若直接用 Rust 的判定，`&nbsp;` 解码出的 U+00A0 会被折叠成
/// 普通空格，与浏览器/Jsoup 的行为分叉——实测正是本 spike 语料 03 的唯一差异。
///
/// 这不只是 spike 的对齐问题：书源规则里 `&nbsp;` 很常见，正文净化的替换链
/// 若把 U+00A0 当普通空白处理，会产出与上游不一致的结果（违反宪法 I 兼容性优先）。
/// 故此处显式采用 HTML 规范的空白集合（WHATWG Infra：tab/LF/FF/CR/space），
/// 与 `Character.isWhitespace` 对 ASCII 的行为一致。
fn normalize_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut previous_was_whitespace = true; // 起始视为空白，从而抑制前导空格
    for character in text.chars() {
        if is_html_whitespace(character) {
            if !previous_was_whitespace {
                result.push(' ');
            }
            previous_was_whitespace = true;
        } else {
            result.push(character);
            previous_was_whitespace = false;
        }
    }
    while result.ends_with(' ') {
        result.pop();
    }
    result
}

/// HTML 规范定义的空白字符（WHATWG Infra "ASCII whitespace"）。
///
/// 刻意**不含** U+00A0：按规范它是不换行空格，属于内容而非可折叠空白。
/// 与 Java 的 `Character.isWhitespace` 行为一致，确保两侧归一化同构。
fn is_html_whitespace(character: char) -> bool {
    matches!(character, '\t' | '\n' | '\u{0C}' | '\r' | ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空样本的差异率为零而非_nan() {
        let report = SampleReport {
            name: "empty".to_owned(),
            jsoup_lines: 0,
            html5ever_lines: 0,
            differing_lines: 0,
            html5ever_empty_text_nodes: 0,
            first_difference: None,
        };
        assert_eq!(report.difference_ratio(), 0.0);
    }

    #[test]
    fn 空白规范化折叠连续空白并去首尾() {
        assert_eq!(normalize_whitespace("  a \n\t b  "), "a b");
        assert_eq!(normalize_whitespace(""), "");
        assert_eq!(normalize_whitespace("   "), "");
    }

    /// 回归：U+00A0（`&nbsp;`）**不是** HTML 空白，必须原样保留。
    ///
    /// 这条测试守的是一个真实踩到的坑：`char::is_whitespace` 对 U+00A0 返回
    /// true，而 HTML/Java 侧返回 false。若用前者，`&nbsp;` 会被折叠成普通空格，
    /// 与浏览器/Jsoup 的书源正文处理分叉（宪法 I 兼容性优先）。
    #[test]
    fn 不换行空格不被折叠() {
        assert_eq!(normalize_whitespace("a\u{a0}b"), "a\u{a0}b");
        // 与真空白混用：空白折叠、U+00A0 保留
        assert_eq!(normalize_whitespace("a  \u{a0}  b"), "a \u{a0} b");
    }

    /// 回归：`<script>` 内容建模为 Data 节点，而非 Text。
    ///
    /// 依据公开事实：HTML 规范把 script/style 内容定义为 raw text；Jsoup 的公开
    /// 文档说明其以 `DataNode` 承载。若不区分，每个含脚本的页面都会产生固定差异。
    #[test]
    fn 脚本内容建模为数据节点() {
        let serialized = serialize_html5ever("<script>var a=1;</script>").expect("解析应成功");
        assert!(
            serialized.contains("|D|var a=1;"),
            "脚本内容应为 D 节点：{serialized}"
        );
    }

    #[test]
    fn 简单文档解析出预期树形() {
        let serialized = serialize_html5ever("<p>hi</p>").expect("解析应成功");
        // 浏览器级解析器会自动补全 html/head/body —— 这里正是要验证这一点
        // （Jsoup 亦然，两侧的"自动补全"是否一致是本 spike 的核心问题之一）
        assert!(serialized.contains("|E|p|"), "应含 p 元素：{serialized}");
        assert!(serialized.contains("|E|body|"), "应自动补 body：{serialized}");
    }

    #[test]
    fn 相邻文本节点被合并() {
        let serialized = serialize_html5ever("<p>a&amp;b</p>").expect("解析应成功");
        let text_lines: Vec<&str> = serialized
            .lines()
            .filter(|line| line.contains("|T|"))
            .collect();
        // 字符引用若不被合并，会碎成多个文本节点；契约要求合并成一个
        assert_eq!(text_lines.len(), 1, "文本应合并为单个节点：{serialized}");
    }

    #[test]
    fn 行数不同时差异计入行数差() {
        let expected = ["1|E|a|", "2|E|b|"];
        let actual = ["1|E|a|"];
        let (differing, first) = line_diff(&expected, &actual);
        assert_eq!(differing, 1, "缺一行应计 1 个差异");
        assert!(first.is_some());
    }
}
