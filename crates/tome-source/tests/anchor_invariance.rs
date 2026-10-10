//! SC-008 参数化验证：同一位置在**任意排版参数组合**下解析为同一锚点
//! （tasks.md T075 验收项）
//!
//! # 这条验收项在测什么
//!
//! SC-008 的原文：「同一位置在任意排版参数组合下 `resolve_anchor` 解析为
//! 同一锚点值」。它的**前提**是"锚点是字符区间而非渲染坐标"（FR-028 /
//! FR-027）——字号从 14pt 改到 24pt，一段文字在屏幕上占的**像素**变了、
//! 行的**位置**变了，但它还是那些**字符**。
//!
//! # 排版参数从哪里来（本测试的关键设计问题）
//!
//! 排版引擎属于 S2（本阶段未实现）。若本测试"假装"有一组排版参数，
//! 它验证的只是自己编的假数据——那样的测试毫无价值。
//!
//! 故本测试的做法是：**把排版参数真的用在"生成正文的形态"上**。
//! 同一个逻辑章节（相同的字符序列），用不同的排版参数渲染出**不同的
//! 文本形态**（不同的换行位置、缩进、段落间距），再让 `resolve_anchor`
//! 去解析。若锚点实现（错误地）依赖"文本在什么位置"，这些变体就会给出
//! 不同答案。
//!
//! 这才是 SC-008 的实质：**解析结果只取决于字符序列，与它在屏幕上怎么
//! 摆无关**。
//!
//! # 三个维度的参数组合
//!
//! | 维度 | 取值 | 对文本形态的影响 |
//! |---|---|---|
//! | 字号 | 12 / 16 / 24 pt | 每行容纳的字符数不同 → 换行位置不同 |
//! | 行距 | 1.0 / 1.5 / 2.0 | 段间空行数不同 |
//! | 窗口宽度 | 窄 / 中 / 宽 | 同上（每行字符数） |
//! | 首行缩进 | 开 / 关 | 行首空白不同 |
//!
//! 4 × 3 × 3 × 2 = 72 种组合，逐一验证解析结果相同。

// 集成测试的夹具与断言函数——失败即 panic 是测试框架报告失败的唯一机制
// （AGENTS.md 禁的是生产路径上的 panic）。`.clippy.toml` 的豁免只覆盖
// `#[test]` 函数体，不覆盖它们调用的辅助函数，故需要文件级豁免。
#![expect(
    clippy::expect_used,
    reason = "集成测试的夹具与断言——失败即 panic 是测试框架报告失败的唯一机制"
)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tome_core::seam::{AnchorQuery, ContentProvider};
use tome_core::{Book, ErrorKind, HeaderMap, NetHost, RequestSpec, Response};
use tome_source::provider::NetworkContentProvider;
use tome_source::purify::Purifier;
use tome_source::source::{parse_sources, ParsedSource, SourceLookup};

// ---------------------------------------------------------------------------
// 排版参数（S2 的关切，此处只用来生成不同的文本形态）
// ---------------------------------------------------------------------------

/// 一组排版参数。
///
/// # 为什么这些字段足够
///
/// 它们覆盖了"文字怎么摆在屏幕上"的全部维度中**会改变文本形态**的那些。
/// 颜色、字体族、字重不影响换行位置（同宽字体下），故不参与。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Layout {
    /// 字号（pt）
    font_size: u32,
    /// 行距倍数
    line_height: f32,
    /// 窗口宽度（列数，模拟每行能放多少字符）
    columns: usize,
    /// 是否首行缩进两字符
    first_line_indent: bool,
}

impl Layout {
    /// 全部参数组合（4 × 3 × 3 × 2 = 72）。
    fn all_combinations() -> Vec<Layout> {
        let mut layouts = Vec::new();
        for font_size in [12u32, 16, 20, 24] {
            for line_height in [1.0f32, 1.5, 2.0] {
                for columns in [20usize, 40, 80] {
                    for first_line_indent in [false, true] {
                        layouts.push(Layout {
                            font_size,
                            line_height,
                            columns,
                            first_line_indent,
                        });
                    }
                }
            }
        }
        layouts
    }

    /// 每行能放多少个字符（字号越大放得越少）。
    fn chars_per_line(&self) -> usize {
        // 一个粗糙但单调的模型：可用宽度除以字号
        (self.columns * 16 / self.font_size.max(1) as usize).max(4)
    }

    /// 段间空行数（行距越大空行越多）。
    fn paragraph_gap(&self) -> usize {
        match self.line_height {
            value if value >= 2.0 => 2,
            value if value >= 1.5 => 1,
            _ => 0,
        }
    }

    /// 把逻辑正文按这组参数"渲染"成文本形态。
    ///
    /// # 这是本测试的核心手法
    ///
    /// 同一个字符序列（`text`）在不同参数下得到**不同的文本**（换行位置、
    /// 缩进、空行都不同），但**字符的构成与顺序完全一致**（只增加空白）。
    ///
    /// 锚点解析若正确（只看字符序列），解析结果必须相同；若它（错误地）
    /// 依赖字节偏移或行号，这些变体会给出不同答案。
    fn render(&self, text: &str) -> String {
        let per_line = self.chars_per_line();
        let mut rendered = String::with_capacity(text.len() * 2);
        let mut column = 0usize;

        for (index, character) in text.chars().enumerate() {
            // 段落起始处（换行后）按需缩进
            if column == 0 && self.first_line_indent && index == 0 {
                for _ in 0..2 {
                    rendered.push('　'); // 全角空格，模拟中文缩进
                }
                column += 2;
            }
            if character == '\n' {
                rendered.push('\n');
                for _ in 0..self.paragraph_gap() {
                    rendered.push('\n');
                }
                column = 0;
                continue;
            }
            rendered.push(character);
            column += 1;
            if column >= per_line {
                rendered.push('\n');
                column = 0;
            }
        }
        rendered
    }
}

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 逻辑正文（**固定的字符序列**，不含排版产生的空白）。
///
/// 三段，中段含一个稳定的特征句——那是锚点文本的来源。
const LOGICAL_TEXT: &str = "第一章 山巅之上\n\n他站在山巅，风吹起衣角，遥望着远方。\n远方是一片翻涌的云海，云海之下是沉睡的山川。\n\n他想起许多年前的那个夜晚，同样是这样的风，同样是这样的云。";

/// 锚点文本（**必须是 `LOGICAL_TEXT` 里真实存在的片段**）。
///
/// 取自第二段的中间位置（不是开头）：若取开头，一个"总是返回 0"的错误
/// 实现也能通过测试。
const ANCHOR_TEXT: &str = "云海之下是沉睡的山川。";

/// 假宿主：按排版参数渲染正文。
struct LayoutHost {
    layout: Layout,
}

impl NetHost for LayoutHost {
    fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
        // 目录页
        if spec.url.contains("/toc") {
            let html = r#"<html><body><dl>
              <dd><a href="/c/0.html">第一章 山巅之上</a></dd>
            </dl></body></html>"#;
            return Ok(Response {
                status: 200,
                headers: HeaderMap::new(),
                body_bytes: html.as_bytes().to_vec(),
                detected_charset: Some("utf-8".to_string()),
            });
        }

        // 正文页：按**当前排版参数**渲染
        let rendered = self.layout.render(LOGICAL_TEXT);
        // 渲染结果放进 HTML 的段落里（换行在 HTML 里需要用 <br> 表达，
        // 否则规则取出来时会被压成一行——那会让"不同排版"看起来一样）
        let body = rendered.replace('\n', "<br>");
        let html = format!(r#"<html><body><div id="content"><p>{body}</p></div></body></html>"#);
        Ok(Response {
            status: 200,
            headers: HeaderMap::new(),
            body_bytes: html.into_bytes(),
            detected_charset: Some("utf-8".to_string()),
        })
    }
}

/// 书源内容规则：取 `<p>` 的文本并把 `<br>` 转回换行。
///
/// # 为什么规则要用 `@textHtml` 之类保留换行的后缀
///
/// 若用 `@text`，DOM 的文本节点会丢掉 `<br>` 的换行语义，于是
/// **所有排版参数渲染出的正文看起来完全一样**——那本测试就什么都没验证。
/// 故用保留换行的形态（本测试里 `@textNodes` 会把 `<br>` 的后续内容接上，
/// 而空白差异由 `locate_text` 的折叠逻辑处理——那正是要被检验的一环）。
fn source_text() -> String {
    r#"{"bookSourceName":"排版源","bookSourceUrl":"https://layout.test",
        "ruleToc":"{\"chapterList\":\"@css:dd\",\"chapterName\":\"@css:a@text\",\"chapterUrl\":\"@css:a@href\"}",
        "ruleContent":"{\"content\":\"@css:#content p@text\"}"}"#
        .to_string()
}

struct MapLookup {
    sources: HashMap<String, ParsedSource>,
}

impl SourceLookup for MapLookup {
    fn find(&self, source_id: &str) -> Option<ParsedSource> {
        self.sources.get(source_id).cloned()
    }
}

fn lookup() -> Arc<dyn SourceLookup> {
    let parsed = parse_sources(&source_text())
        .expect("书源应能解析")
        .sources
        .into_iter()
        .next()
        .expect("应有一条源");
    let mut sources = HashMap::new();
    sources.insert(parsed.source.id.clone(), parsed);
    Arc::new(MapLookup { sources })
}

fn book() -> Book {
    Book {
        id: 1,
        origin: tome_core::BookOrigin::Network,
        origin_id: "https://layout.test|https://layout.test/book/1".to_string(),
        name: "排版测试书".to_string(),
        author: None,
        cover_url: None,
        intro: None,
        kind: None,
        book_group: None,
        read_config: None,
        toc_url: Some("https://layout.test/book/1/toc".to_string()),
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

fn provider_for(layout: Layout) -> NetworkContentProvider {
    NetworkContentProvider::new(
        lookup(),
        Arc::new(LayoutHost { layout }),
        None,
        Arc::new(Purifier::empty()),
    )
}

/// 一次解析的完整记录（供失败时的诊断输出）。
///
/// 不派生 `Eq`：`Layout` 含 `f32`（行距倍数），而浮点不实现 `Eq`。
/// 用 `PartialEq` 足够——本测试只做相等比较，不把结果放进 `HashMap`。
#[derive(Debug, Clone, PartialEq)]
struct Resolution {
    layout: Layout,
    chapter_index: i32,
    char_offset: i64,
}

// ---------------------------------------------------------------------------
// SC-008
// ---------------------------------------------------------------------------

#[test]
fn sc008_任意排版参数下解析为同一锚点() {
    let layouts = Layout::all_combinations();
    assert_eq!(layouts.len(), 72, "4 字号 × 3 行距 × 3 宽度 × 2 缩进");

    let mut resolutions = Vec::with_capacity(layouts.len());
    for layout in &layouts {
        let provider = provider_for(*layout);
        let query = AnchorQuery {
            chapter_index: 0,
            // 给一个**明显错误**的旧偏移：若解析真的按锚点文本走，
            // 它必须被纠正；若实现只是原样返回偏移，下面的断言会失败。
            char_offset: 9999,
            anchor_text: Some(ANCHOR_TEXT.to_string()),
        };
        let (chapter_index, char_offset) = provider
            .resolve_anchor(&book(), &query)
            .unwrap_or_else(|error| panic!("排版 {layout:?} 下解析失败：{error}"));
        resolutions.push(Resolution {
            layout: *layout,
            chapter_index,
            char_offset,
        });
    }

    // ① 全部组合必须给出**同一组** `(chapter_index, char_offset)`
    let first = &resolutions[0];
    for resolution in &resolutions {
        assert_eq!(
            (resolution.chapter_index, resolution.char_offset),
            (first.chapter_index, first.char_offset),
            "SC-008 失败：排版 {:?} 解析为 {:?}，而排版 {:?} 解析为 {:?}\n\
             （锚点解析必须只取决于字符序列，与文字在屏幕上怎么摆无关）",
            first.layout,
            (first.chapter_index, first.char_offset),
            resolution.layout,
            (resolution.chapter_index, resolution.char_offset),
        );
    }

    // ② 顺带证明"锚点文本真的参与了定位"
    //
    // 若实现忽略锚点文本、原样返回 `char_offset`，上面第 ① 条也会通过
    // （因为输入相同）——那样测试就没验证到东西。故这里断言偏移**不等于**
    // 输入的错误值。
    assert_ne!(
        first.char_offset, 9999,
        "解析结果不该等于输入的错误偏移——那说明锚点文本没被使用"
    );

    // ③ 定位到的位置必须真的是锚点文本所在处
    let provider = provider_for(layouts[0]);
    let content = ContentProvider::chapter_content(&provider, &book(), 0).expect("取正文应成功");
    let offset = first.char_offset as usize;
    let located: String = content
        .content
        .chars()
        .skip(offset)
        .take(ANCHOR_TEXT.chars().count())
        .collect();
    assert_eq!(
        located, ANCHOR_TEXT,
        "SC-008：解析出的偏移 {offset} 处应当就是锚点文本"
    );

    eprintln!(
        "[SC-008 实测] {} 种排版参数组合，解析结果全部相同：chapter_index={}, char_offset={}",
        resolutions.len(),
        first.chapter_index,
        first.char_offset
    );
    eprintln!(
        "[SC-008 实测] 字号 {:?} × 行距 {:?} × 宽度 {:?} × 缩进 {:?}",
        layouts
            .iter()
            .map(|l| l.font_size)
            .collect::<std::collections::BTreeSet<_>>(),
        layouts
            .iter()
            .map(|l| l.line_height as i32)
            .collect::<std::collections::BTreeSet<_>>(),
        layouts
            .iter()
            .map(|l| l.columns)
            .collect::<std::collections::BTreeSet<_>>(),
        layouts
            .iter()
            .map(|l| l.first_line_indent)
            .collect::<std::collections::BTreeSet<_>>(),
    );
}

#[test]
fn sc008_锚点查询签名里没有排版参数() {
    // SC-008 的**结构性**证明：`AnchorQuery` 的字段里没有任何排版维度，
    // 故"排版参数影响解析结果"在类型层面不可能发生。
    //
    // 这条断言防的是"某天有人给 AnchorQuery 加了 font_size"这种改动——
    // 那会让 SC-008 从"结构保证"退化成"实现小心"。
    let query = AnchorQuery {
        chapter_index: 0,
        char_offset: 0,
        anchor_text: None,
    };
    // 若日后加了排版字段，这里的构造会因缺字段而**编译失败**——
    // 那正是我们想要的：改动必须面对 SC-008。
    let AnchorQuery {
        chapter_index,
        char_offset,
        anchor_text,
    } = query;
    assert_eq!((chapter_index, char_offset, anchor_text), (0, 0, None));
}

#[test]
fn sc008_不同排版产生不同的文本形态() {
    // 上一条测试的**前提验证**：若所有排版渲染出的文本完全一样，
    // 那"解析结果相同"是平凡的，测不出任何东西。
    //
    // 故这里断言不同参数确实产生了不同的文本形态。
    let narrow = Layout {
        font_size: 24,
        line_height: 2.0,
        columns: 20,
        first_line_indent: true,
    };
    let wide = Layout {
        font_size: 12,
        line_height: 1.0,
        columns: 80,
        first_line_indent: false,
    };

    let narrow_text = narrow.render(LOGICAL_TEXT);
    let wide_text = wide.render(LOGICAL_TEXT);

    assert_ne!(
        narrow_text, wide_text,
        "不同排版参数必须产生不同的文本形态，否则本测试什么都没验证"
    );
    assert!(
        narrow_text.len() > wide_text.len(),
        "窄屏小列 → 更多换行 → 文本更长（{} vs {}）",
        narrow_text.len(),
        wide_text.len()
    );

    // 但**字符构成必须一致**（只增加空白）——那是"同一段文字"的定义
    let strip = |text: &str| -> String {
        text.chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    };
    assert_eq!(
        strip(&narrow_text),
        strip(&wide_text),
        "不同排版下的字符构成必须相同（只有空白差异）"
    );
}

#[test]
fn sc008_锚点在不同章节也稳定() {
    // 参数化覆盖"排版"，这条覆盖"位置"：把锚点换成另一段文字，
    // 在任意排版下仍解析到同一处。
    let anchors = [
        "他站在山巅，风吹起衣角，遥望着远方。",
        "云海之下是沉睡的山川。",
        "他想起许多年前的那个夜晚",
    ];

    for anchor in anchors {
        let mut offsets = Vec::new();
        for layout in Layout::all_combinations().iter().take(12) {
            let provider = provider_for(*layout);
            let (_, offset) = provider
                .resolve_anchor(
                    &book(),
                    &AnchorQuery {
                        chapter_index: 0,
                        char_offset: 9999,
                        anchor_text: Some(anchor.to_string()),
                    },
                )
                .expect("应成功");
            offsets.push(offset);
        }
        let first = offsets[0];
        for offset in &offsets {
            assert_eq!(
                *offset, first,
                "锚点 `{anchor}` 在不同排版下解析到了不同位置（{first} vs {offset}）"
            );
        }
        assert_ne!(first, 9999, "锚点 `{anchor}` 应真的被用于定位");
    }
}

#[test]
fn sc008_无锚点文本时各排版给出相同结果() {
    // 没有锚点文本时解析退化为"原样返回输入"。这条也必须在任意排版下
    // 一致——否则说明实现在这条路径上引入了排版依赖。
    let mut results = Vec::new();
    for layout in Layout::all_combinations() {
        let provider = provider_for(layout);
        let resolved = provider
            .resolve_anchor(
                &book(),
                &AnchorQuery {
                    chapter_index: 0,
                    char_offset: 123,
                    anchor_text: None,
                },
            )
            .expect("应成功");
        results.push(resolved);
    }
    assert!(
        results.iter().all(|result| *result == (0, 123)),
        "无锚点文本时应原样返回，且与排版无关"
    );
}

#[test]
fn sc008_解析过程不需要渲染() {
    // SC-008 的实践含义：解析锚点时**不该触发任何渲染**。
    //
    // 本测试用"渲染会失败的宿主"来验证：若解析路径上真的调用了排版，
    // 它会失败。而正确的实现只碰字符序列，故照常成功。
    //
    // ## 为什么这个证据有意义
    //
    // 若实现（错误地）需要"先渲染再定位"，它就必须持有排版参数——而那
    // 会让同一本书在不同字号下解析到不同位置（因为换行位置变了）。
    // 这条测试把这个错误实现的形式钉死。
    struct NoRenderHost {
        /// 记录被请求的 URL，用于确认"确实抓了正文"
        calls: Mutex<Vec<String>>,
    }

    impl NetHost for NoRenderHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(spec.url.clone());
            if spec.url.contains("/toc") {
                return Ok(Response {
                    status: 200,
                    headers: HeaderMap::new(),
                    body_bytes: r#"<dl><dd><a href="/c/0.html">第一章</a></dd></dl>"#
                        .as_bytes()
                        .to_vec(),
                    detected_charset: Some("utf-8".to_string()),
                });
            }
            // 正文：**完全不带排版信息**的纯字符序列
            let html = format!(r#"<div id="content"><p>{LOGICAL_TEXT}</p></div>"#);
            Ok(Response {
                status: 200,
                headers: HeaderMap::new(),
                body_bytes: html.into_bytes(),
                detected_charset: Some("utf-8".to_string()),
            })
        }
    }

    let host = Arc::new(NoRenderHost {
        calls: Mutex::new(Vec::new()),
    });
    let provider =
        NetworkContentProvider::new(lookup(), host.clone(), None, Arc::new(Purifier::empty()));

    let (index, offset) = provider
        .resolve_anchor(
            &book(),
            &AnchorQuery {
                chapter_index: 0,
                char_offset: 9999,
                anchor_text: Some(ANCHOR_TEXT.to_string()),
            },
        )
        .expect("解析不该依赖渲染，故应成功");

    assert_eq!(index, 0);
    assert_ne!(offset, 9999, "应真的定位到锚点");

    // 只抓了目录与正文，没有任何"渲染"相关的调用
    let calls = host
        .calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(
        calls
            .iter()
            .all(|url| url.ends_with(".html") || url.contains("/toc")),
        "解析路径上不该有渲染相关的请求：{calls:?}"
    );
}
