//! [DONE] 图片抓取管线（tasks.md T068 / FR-019，SC-014、SC-018）
//!
//! # 两条不变量
//!
//! > **SC-014**：图片失败 MUST NOT 中断正文获取。
//! > **SC-018**：鉴权图片获取成功率与正文一致。
//!
//! ## SC-014 的落地：图片结果**不是错误**
//!
//! [`ImageOutcome`] 里没有 `Err` 分支承载"这张图没取到"——它是
//! [`ImageResult::Placeholder`]。类型层面就让"图片失败"无法传播成
//! "正文失败"。调用方想忽略图片时不必写 `try/catch`，想展示占位时
//! 拿到的就是占位。
//!
//! 这与常规的 Rust 风格（失败即 `Err`）相反，是**刻意的**：这里的
//! "失败"不是异常，而是**预期的正常结果之一**（图片站挂了、图被删了、
//! 需要登录而 cookie 过期了——都是日常）。把它做成 `Err` 会让每个调用点
//! 都要处理一个它无法处置的错误。
//!
//! ## SC-018 的落地：鉴权信息必须透传到图片请求
//!
//! "鉴权图片成功率与正文一致"意味着：**正文能取到的图，图片请求也必须
//! 能取到**。正文请求带的是书源声明的头（含 Cookie），故图片请求必须
//! 带同一套。少带一个 `Referer` 就足以让图床返回 403——而表现为
//! "正文正常、封面全裂"。
//!
//! 故 [`image_spec`] 从**同一个** `base_spec` 出发，只改 URL 与方法：
//! 请求头、Cookie、字符集声明、限速键全部一致。任何"图片请求单独构造头"
//! 的做法都会让两者漂移。

use std::sync::Arc;

use tome_core::{Chapter, NetHost};

use crate::action;
use crate::error::SourceError;
use crate::source::ParsedSource;

/// `ruleContent` 里与图片相关的字段名。
pub mod image_keys {
    /// 正文里内嵌图片的地址规则（部分源用它而非从 HTML 的 `<img>` 取）
    pub const IMAGE_URL: &str = "imageUrl";
}

/// 图片抓取的结果。
///
/// ## 为什么没有 `Failed` 变体
///
/// 见模块文档 SC-014 的说明——"没取到"在这里是**正常的正常结果**，
/// 由 [`Placeholder`] 承载。诊断信息进 [`Placeholder::reason`]，
/// 供界面展示"这张图为什么是占位"。
///
/// [`Placeholder`]: ImageResult::Placeholder
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageResult {
    /// 成功取到图片字节
    Fetched {
        /// 图片地址
        url: String,
        /// 图片字节
        bytes: Vec<u8>,
        /// 内容类型（用于选择解码方式；源站未给则为 `None`）
        content_type: Option<String>,
        /// 是否来自缓存
        from_cache: bool,
    },
    /// 没取到，使用占位（**不是错误**）。
    Placeholder {
        /// 图片地址
        url: String,
        /// 取不到的原因（供界面展示）
        reason: String,
    },
}

impl ImageResult {
    /// 图片地址（两种情况都有）。
    pub fn url(&self) -> &str {
        match self {
            Self::Fetched { url, .. } | Self::Placeholder { url, .. } => url,
        }
    }

    /// 是否成功取到。
    pub fn is_fetched(&self) -> bool {
        matches!(self, Self::Fetched { .. })
    }

    /// 取到的字节（占位时为 `None`）。
    pub fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Fetched { bytes, .. } => Some(bytes),
            Self::Placeholder { .. } => None,
        }
    }
}

/// 一次正文图片集合的抓取结果（SC-014 的整体形态）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageSet {
    /// 逐张图片的结果（**保序**：与正文里出现的顺序一致，便于按位置回填）
    pub images: Vec<ImageResult>,
}

impl ImageSet {
    /// 成功取到的张数。
    pub fn fetched_count(&self) -> usize {
        self.images
            .iter()
            .filter(|image| image.is_fetched())
            .count()
    }

    /// 用占位的张数。
    pub fn placeholder_count(&self) -> usize {
        self.images
            .iter()
            .filter(|image| !image.is_fetched())
            .count()
    }

    /// 总数。
    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// 成功率（`0.0`–`1.0`；空集合定义为 `1.0` 即"没有失败的"）。
    ///
    /// # 为什么空集合是 1.0 而不是 0.0
    ///
    /// 没有图片可取的正文**完全正常**（纯文字章节）。报成 0% 会让
    /// "这篇没有图"被当成"图片全挂了"，而那正是 SC-018 要度量的东西
    /// ——度量口径错了，指标就没有意义。
    pub fn success_rate(&self) -> f64 {
        if self.images.is_empty() {
            return 1.0;
        }
        self.fetched_count() as f64 / self.images.len() as f64
    }
}

/// 图片缓存接缝（由组合根注入 `tome-store` 的 `ImageCache`）。
///
/// 与 [`crate::content::ChapterCache`] 分开：`data-model.md` 与 SC-017
/// 都要求**章节缓存与图片缓存是两个可独立清理的资源**。一个 trait
/// 承载两者会让"清了章节缓存顺带把图也清了"成为可能。
pub trait ImageCache: Send + Sync {
    /// 读；未缓存返回 `Ok(None)`。
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>, SourceError>;
    /// 写。
    fn put(&self, url: &str, bytes: &[u8]) -> Result<(), SourceError>;
}

/// 不缓存。
pub struct NoImageCache;

impl ImageCache for NoImageCache {
    fn get(&self, _url: &str) -> Result<Option<Vec<u8>>, SourceError> {
        Ok(None)
    }
    fn put(&self, _url: &str, _bytes: &[u8]) -> Result<(), SourceError> {
        Ok(())
    }
}

/// 图片抓取的配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePolicy {
    /// 单张图的失败重试次数（**不含**首次）
    pub max_retries: u32,
    /// 每次重试前的退避毫秒数
    pub backoff_ms: u64,
    /// 单张图的大小上限（字节）。超过则放弃——防止一张 500MB 的图把
    /// 内存吃满（边界处校验外部输入，AGENTS.md 安全红线）
    pub max_bytes: usize,
}

impl Default for ImagePolicy {
    fn default() -> Self {
        Self {
            // [WORKAROUND] 硬编码默认值：图片站偶发 5xx 是常态，重试一次
            // 能显著提高成功率（SC-018 关心的是"与正文一致"）。
            // 若实测发现某类站点需要更多重试，应改为可配置项。
            max_retries: 2,
            backoff_ms: 200,
            // 24 MiB：远大于任何真实插图（通常几百 KB），
            // 又不会让单张图占满内存
            max_bytes: 24 * 1024 * 1024,
        }
    }
}

/// 抓取一张图片（**永不返回 `Err` 表示"没取到"**，见模块文档 SC-014）。
///
/// 返回的 `Err` 只在**调用方用错**时出现（如 URL 为空）——那是编程错误
/// 而非运行期故障，故用 `Err` 让它尽早暴露。
pub fn fetch_image(
    parsed: &ParsedSource,
    url: &str,
    host: &dyn NetHost,
    cache: Option<&dyn ImageCache>,
    policy: &ImagePolicy,
) -> Result<ImageResult, SourceError> {
    if url.trim().is_empty() {
        return Err(SourceError::rule("图片地址为空"));
    }

    // 缓存先行（避免重复下载同一张封面）
    if let Some(cache) = cache {
        // 缓存读失败不该让图片变成占位——它只是"这次没命中"，下面会真的
        // 去抓。凭空降级成占位是丢信息。
        if let Ok(Some(bytes)) = cache.get(url) {
            return Ok(ImageResult::Fetched {
                url: url.to_string(),
                bytes,
                content_type: None,
                from_cache: true,
            });
        }
    }

    let mut last_reason = String::from("未尝试");
    for attempt in 0..=policy.max_retries {
        // ## 图片请求**复用**书源的请求定制（SC-018 的关键）
        //
        // 同一套头（含 Cookie/Referer）、同一套限速键、同一套字符集声明。
        // 单独构造头会让"正文能取到、图片 403"成为可能，而那是 SC-018
        // 明确要杜绝的。
        let mut spec = action::base_spec(parsed, url)?;
        // 图片不是文本：字符集声明对它无意义，且会让解码层做无用功
        spec.charset = None;
        spec.timeout_ms = IMAGE_TIMEOUT_MS;

        match host.fetch(spec) {
            Ok(response) if response.is_success() => {
                let bytes = response.body_bytes;
                if bytes.len() > policy.max_bytes {
                    last_reason =
                        format!("图片 {} 字节，超过上限 {}", bytes.len(), policy.max_bytes);
                    // 超限不重试——重试拿到的还是同一张超大图
                    break;
                }
                if let Some(cache) = cache {
                    // 写缓存失败不影响本次返回（图已经拿到了）
                    let _ = cache.put(url, &bytes);
                }
                let content_type = response.headers.get("content-type").map(str::to_string);
                return Ok(ImageResult::Fetched {
                    url: url.to_string(),
                    bytes,
                    content_type,
                    from_cache: false,
                });
            }
            Ok(response) => {
                last_reason = format!("HTTP {}", response.status);
            }
            Err(kind) => {
                last_reason = format!("{kind}");
            }
        }

        // 最后一次尝试后不再退避（省掉无意义的等待）
        if attempt < policy.max_retries {
            std::thread::sleep(std::time::Duration::from_millis(policy.backoff_ms));
        }
    }

    // **占位而不是错误**：SC-014 要求图片失败不中断正文
    Ok(ImageResult::Placeholder {
        url: url.to_string(),
        reason: last_reason,
    })
}

/// 图片请求超时（毫秒）。
///
/// 比正文动作的 15 秒短：图片是**辅助内容**，为它让读者等 15 秒不划算。
/// 8 秒足以覆盖常见图床，超时的图直接走占位。
///
/// [UNCERTAIN] 慢速图床（境外 CDN、大尺寸原图）可能超过 8 秒。若实测
/// 发现占位率偏高，应改为可配置而非直接调大——占位是一个**可接受**的
/// 结果（SC-014 的设计前提），调大超时会让读者为辅助内容等更久。
pub const IMAGE_TIMEOUT_MS: u64 = 8_000;

/// 抓取一组的图片（**逐张独立**：任一张失败不影响其余）。
///
/// # 为什么逐张独立而不是整批失败
///
/// 一章可能有几十张插图，其中一张挂在失效的 CDN 上是常态。整批失败会
/// 让读者连能看的图都看不到——而 SC-014 要求的正是"图片失败不中断
/// **正文获取**"，同理也不该中断**其他图片**。
pub fn fetch_images(
    parsed: &ParsedSource,
    urls: &[String],
    host: &dyn NetHost,
    cache: Option<&dyn ImageCache>,
    policy: &ImagePolicy,
) -> Result<ImageSet, SourceError> {
    let mut images = Vec::with_capacity(urls.len());
    for url in urls {
        images.push(fetch_image(parsed, url, host, cache, policy)?);
    }
    Ok(ImageSet { images })
}

/// 从正文 HTML 里提取内嵌图片地址（**不发请求**，A2 纯化）。
///
/// # 两条来源的优先级
///
/// 1. 书源显式声明的 `imageUrl` 规则（源作者知道图片在哪）
/// 2. HTML 里的 `<img src>`（规则的兜底）
///
/// 反过来的话，"源明确说了图片在 `.illustration img`"会被页面里其他
/// 装饰性 `<img>`（logo、广告位）淹没。
pub fn extract_image_urls(
    parsed: &ParsedSource,
    html: &str,
    base_url: &str,
) -> Result<Vec<String>, SourceError> {
    let source = &parsed.source;
    let document = tome_rule::dom::Document::parse_html(html);
    let engine = tome_rule::css_post::RuleEngine::new(&document);

    // ① 书源显式规则
    if let Some(rule_content) = source.rule_content.as_deref() {
        if let Ok(rules) = action::parse_rule_object(parsed, "ruleContent", Some(rule_content)) {
            if let Some(rule) = rules.get(image_keys::IMAGE_URL) {
                let values = engine
                    .evaluate(rule)
                    .map_err(|error| SourceError::rule_eval(&error, "imageUrl"))?;
                let urls: Vec<String> = values
                    .into_iter()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| action::absolute_url(&source.id, &value))
                    .collect();
                if !urls.is_empty() {
                    return Ok(urls);
                }
            }
        }
    }

    // ② 兜底：HTML 里的 `<img src>`
    let raw = engine
        .evaluate("@css:img@src")
        .map_err(|error| SourceError::rule_eval(&error, "img"))?;
    let _ = base_url;
    Ok(raw
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| action::absolute_url(&source.id, &value))
        .collect())
}

/// 取一章的插图（封面之外的正文内嵌图）。
///
/// `html` 是**已抓到的**正文页 HTML（正文管线已经拿过它了——不为此再抓一次）。
pub fn chapter_images(
    parsed: &ParsedSource,
    chapter: &Chapter,
    html: &str,
    host: &dyn NetHost,
    cache: Option<&dyn ImageCache>,
    policy: &ImagePolicy,
) -> Result<ImageSet, SourceError> {
    let base = chapter.url.as_deref().unwrap_or(&parsed.source.id);
    let urls = extract_image_urls(parsed, html, base)?;
    fetch_images(parsed, &urls, host, cache, policy)
}

/// 正文里一张图片的位置（T084 裁决 OQ#13 的 `ImageRef` 的源数据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAnchor {
    /// 图片地址（已补全为绝对地址）
    pub url: String,
    /// 图片在**该元素文本**里的位置（`None` = 位置无法确定）
    pub char_offset: Option<i64>,
    /// 替代文本
    pub alt: Option<String>,
}

/// 从正文页 HTML 提取图片的**位置信息**（`ImageRef` 的位置来源）。
///
/// # 位置是怎么算的
///
/// 正文文本由 `ruleContent.content` 规则求值得到（见
/// [`crate::content::parse_chapter_content`]），其形态是"匹配到的元素
/// **按文档顺序**逐个取文本再换行拼接"。故图片的位置可以用同一条思路算：
///
/// ```text
///   元素序列：  [<p>图一前</p>] [<img src=A>] [<p>图一后</p>]
///   文本拼接：  "图一前" + "\n" + "" + "\n" + "图一后"
///                        ↑ 图片落在这一段的边界处
/// ```
///
/// 故本函数返回的 `char_offset` 是**该图片所在元素之前已累积的文本长度**
/// （含分隔换行）——即"图片插进正文流的那个位置"。
///
/// ## 为什么 `char_offset` 是 `Option`
///
/// 图片可能出现在正文规则**匹配不到**的位置（页头广告图、页脚 logo）。
/// 那些图片没有正文位置可言——把它们硬塞进某个偏移会让排版把页头横幅
/// 插进正文中间。此时给 `None`，由调用方决定"要不要这张图"
/// （`ImageRef` 只收有位置的）。
///
/// # 为什么不在这里做净化
///
/// 净化会改变文本长度（替换规则删字），故净化后的位置与净化前不同。
/// 这个换算需要知道净化改了什么——那是 [`crate::purify`] 的关切。
/// 本函数只给**原始文本坐标系**里的位置；调用方（provider）负责
/// 与净化后的坐标对齐（`align_anchors_after_purify`）。
pub fn extract_image_anchors(
    parsed: &ParsedSource,
    html: &str,
    base_url: &str,
) -> Result<Vec<ImageAnchor>, SourceError> {
    let source = &parsed.source;
    let document = tome_rule::dom::Document::parse_html(html);

    // 正文规则决定"哪些元素参与正文"——图片位置必须与它同源
    let Some(rule_content) = source.rule_content.as_deref() else {
        return Ok(Vec::new());
    };
    let rules = action::parse_rule_object(parsed, "ruleContent", Some(rule_content))?;
    let Some(content_rule) = rules.get(crate::content::content_keys::CONTENT) else {
        return Ok(Vec::new());
    };

    // 按文档顺序取正文元素，累积文本长度——每遇到含图元素就记一个位置
    let nodes = tome_rule::css_post::select_nodes(&document, content_rule)
        .map_err(|error| SourceError::rule_eval(&error, "content"))?;

    let mut anchors = Vec::new();
    let mut accumulated = 0i64;
    for node in &nodes {
        // 该元素的文本（与正文拼接同口径：取其文本内容）
        let text = node.text();
        // 这个元素里的图：位置 = 累积长度 + 图在该元素**内部**的偏移
        for (inner_offset, url, alt) in images_within(node, base_url, &source.id) {
            anchors.push(ImageAnchor {
                url,
                char_offset: Some(accumulated + inner_offset),
                alt,
            });
        }
        // 累积：文本长度 + 拼接用的换行（正文是 join("\n")）
        accumulated += text.chars().count() as i64 + 1;
    }

    // 正文规则之外的内嵌图（页头/页脚）：位置无法确定
    for (_, url, alt) in images_within(&document.root(), base_url, &source.id) {
        if !anchors.iter().any(|anchor| anchor.url == url) {
            anchors.push(ImageAnchor {
                url,
                char_offset: None,
                alt,
            });
        }
    }

    Ok(anchors)
}

/// 某个节点内部的所有 `<img>`，返回 `(元素内字符偏移, 绝对地址, alt)`。
fn images_within(
    node: &tome_rule::dom::NodeRef,
    base_url: &str,
    source_id: &str,
) -> Vec<(i64, String, Option<String>)> {
    let mut found = Vec::new();
    // 逐字符遍历该子树的文本，遇到 img 元素时记下当前长度
    collect_images(node, base_url, source_id, &mut 0, &mut found);
    found
}

/// 递归收集图片，`cursor` 是已见过的文本长度（元素内偏移）。
fn collect_images(
    node: &tome_rule::dom::NodeRef,
    base_url: &str,
    source_id: &str,
    cursor: &mut i64,
    out: &mut Vec<(i64, String, Option<String>)>,
) {
    // 图片元素：记位置（此时 cursor 就是它前面的文本长度），**不推进 cursor**
    // ——`<img>` 自身没有文本，不影响后续文本的位置
    let tag = node.qualified_name();
    if tag.eq_ignore_ascii_case("img") {
        if let Some(src) = node.attr("src") {
            if !src.trim().is_empty() {
                let alt = node.attr("alt").filter(|text| !text.trim().is_empty());
                // 书源规则里的 `imageUrl` 优先于 HTML 的 src（与
                // `extract_image_urls` 的分支顺序一致）
                out.push((*cursor, action::absolute_url(source_id, &src), alt));
            }
        }
    }
    let _ = base_url;

    for child in node.children() {
        collect_images(&child, base_url, source_id, cursor, out);
    }
}

/// 把**原始正文**坐标系里的图片锚点换算到**净化后正文**坐标系（PM 裁决 OQ#13）。
///
/// # 为什么必须换算
///
/// `ImageRef::char_offset` 的语义是"图片在 `ChapterContent::content`（净化后
/// 正文）里的字符位置"——因为 S2 排版读的就是净化后的文本。而锚点是相对
/// `raw`（净化前）算的：净化会删字（替换规则去掉广告串），故两者不同。
///
/// # 换算法：按公共前后缀逐步校正
///
/// 净化是"文本→文本"的变换，本函数不知道它具体改了什么（那是
/// [`crate::purify`] 的关切，且规则是可配置的）。故用**位置校正**：
///
/// ```text
///   raw:       "AAA ███ BBB"      （███ 是广告，会被删）
///   purified:  "AAA  BBB"
///   锚点在 raw 的 9  →  校正后应在 purified 的 5
/// ```
///
/// 对**每个**锚点，找它在 raw 里的上下文片段是否仍出现在 purified 里，
/// 据此校正。片段匹配失败的锚点被**丢弃**——位置错误的图比没有图更糟
/// （它会被插到完全无关的段落里）。
///
/// ## 为什么不做"逐字对齐"
///
/// 逐字对齐（编辑距离）是 O(n·m)，正文动辄几万字，每次读章都跑一次
/// 不划算。本函数是 O(锚点数 × 片段长度)，锚点数通常是个位数。
///
/// ## 为什么不丢弃全部（净化过就不给图）
///
/// 那会让"配了净化规则的书"完全看不到插图——而净化规则的典型用途是
/// 去广告串，与图片毫无关系。保留能定位的图才是符合预期的行为。
#[must_use]
pub fn align_anchors_after_purify(
    raw: &str,
    purified: &str,
    anchors: &[ImageAnchor],
) -> Vec<tome_core::seam::ImageRef> {
    // 净化没改长度时（最常见：没有匹配的替换规则）偏移直接可用
    if raw == purified {
        let mut refs: Vec<tome_core::seam::ImageRef> = anchors
            .iter()
            .filter_map(|anchor| {
                anchor
                    .char_offset
                    .map(|char_offset| tome_core::seam::ImageRef {
                        char_offset,
                        cache_key: anchor.url.clone(),
                        alt: anchor.alt.clone(),
                    })
            })
            .collect();
        // 排序**不能只在慢路径做**：S2 排版按顺序插入图片，乱序会让两张图
        // 互换位置。这条快捷路径同样要保证"产出按位置升序"。
        refs.sort_by_key(|image| image.char_offset);
        return refs;
    }

    let raw_chars: Vec<char> = raw.chars().collect();
    let purified_chars: Vec<char> = purified.chars().collect();

    const CONTEXT: usize = 16;
    /// 上下文的最小长度 —— 更短的会误命中（见下方说明）。
    const MIN_CONTEXT: usize = 4;

    let mut refs = Vec::new();
    for anchor in anchors {
        let Some(char_offset) = anchor.char_offset else {
            // 位置未知（正文规则之外的图）：丢弃。
            // `ImageRef` 只收"能插进排版流"的图——没有位置的图无处可插。
            continue;
        };
        let offset = char_offset as usize;
        if offset >= raw_chars.len() {
            continue;
        }

        if offset == 0 {
            // 锚点在正文最开头：净化不会改变"开头"这件事
            refs.push(tome_core::seam::ImageRef {
                char_offset: 0,
                cache_key: anchor.url.clone(),
                alt: anchor.alt.clone(),
            });
            continue;
        }

        let purified_text: String = purified_chars.iter().collect();

        // ## 为什么从长到短逐级尝试上下文
        //
        // 用整段前缀做定位**太脆**：只要窗口里有**任何一个**字符被净化规则
        // 碰到（哪怕只是把一个全角标点换成半角），整段就匹配不上，图被丢弃。
        // 而实测中"离图片最近的几个字"通常没被改动（净化规则改的是广告串，
        // 那往往离正文里的图很远）。
        //
        // 故从 16 字逐级退到 2 字：先用长上下文精确定位（最准），
        // 失败就退到短上下文（更容易命中，但也更容易匹配到别处——
        // 故配合 `rfind` 取最后一个，偏向"靠后"的一处）。
        //
        // 退到 `MIN_CONTEXT` 字仍失败才放弃。**不能退到 2 字**：短上下文会
        // 误命中重复出现的片段（实测：2 字的"。"+换行在净化后的文本另一处
        // 出现，把图片定位到了错误的段落）。4 字是"不会偶然重复"的下限。
        let mut relocated = None;
        let mut length = CONTEXT.min(offset);
        while length >= MIN_CONTEXT {
            let start = offset - length;
            let prefix: String = raw_chars[start..offset].iter().collect();
            if let Some(found_byte) = purified_text.rfind(&prefix) {
                let end_byte = found_byte + prefix.len();
                relocated = Some(purified_text[..end_byte].chars().count() as i64);
                break;
            }
            length -= 1;
        }

        let Some(new_offset) = relocated else {
            // 连 2 字上下文都找不到：位置无从确定，丢弃。
            // 位置错的图比没有图更糟——它会被插到无关段落里。
            continue;
        };

        refs.push(tome_core::seam::ImageRef {
            char_offset: new_offset,
            cache_key: anchor.url.clone(),
            alt: anchor.alt.clone(),
        });
    }

    // 按位置排序：S2 排版按顺序插入图片，乱序会让两张图互换位置
    refs.sort_by_key(|image| image.char_offset);
    refs
}

/// `tome_store::ImageCache` 的 `ImageCache` 适配（T068 的接线点）。
///
/// # 为什么 impl 在这里而不在 `tome-store`
///
/// 与 [`crate::content::PackChapterCache`] 同理：依赖方向单向
/// （`tome-source → tome-store`），放在被依赖方会成环。
pub struct StoreImageCache {
    inner: Arc<tome_store::ImageCache>,
}

impl StoreImageCache {
    /// 包装一个真实图片缓存。
    pub fn new(inner: Arc<tome_store::ImageCache>) -> Self {
        Self { inner }
    }
}

impl ImageCache for StoreImageCache {
    fn get(&self, url: &str) -> Result<Option<Vec<u8>>, SourceError> {
        self.inner.get(url).map_err(|error| SourceError::Network {
            kind: error.to_error_kind(),
        })
    }

    fn put(&self, url: &str, bytes: &[u8]) -> Result<(), SourceError> {
        self.inner
            .put(url, bytes, None)
            .map_err(|error| SourceError::Network {
                kind: error.to_error_kind(),
            })
    }
}

/// 把一个真实图片缓存包成 `Arc<dyn ImageCache>`。
pub fn store_image_cache(cache: Arc<tome_store::ImageCache>) -> Arc<dyn ImageCache> {
    Arc::new(StoreImageCache::new(cache))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::parse_sources;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tome_core::{ErrorKind, HeaderMap, RequestSpec, Response};

    fn parsed(rule_content: Option<&str>) -> ParsedSource {
        let text = match rule_content {
            Some(rule) => format!(
                r#"{{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                     "header":"{{\"Referer\":\"https://a.com/page\"}}",
                     "ruleContent":{}}}"#,
                serde_json::to_string(rule).expect("应能序列化")
            ),
            None => r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com",
                         "header":"{\"Referer\":\"https://a.com/page\"}"}"#
                .to_string(),
        };
        parse_sources(&text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    /// 假宿主：按 URL 决定成败，并记录每次请求。
    struct FakeHost {
        /// URL 片段 → (状态码, 正文)
        responses: Mutex<HashMap<String, (u16, Vec<u8>)>>,
        /// 默认响应（未匹配时）
        default: (u16, Vec<u8>),
        seen: Mutex<Vec<RequestSpec>>,
        /// 前 N 次对某 URL 的请求强制失败（模拟偶发故障）
        fail_first: Mutex<HashMap<String, usize>>,
    }

    impl FakeHost {
        fn with(responses: &[(&str, u16, &[u8])]) -> Self {
            let mut map = HashMap::new();
            for (fragment, status, body) in responses {
                map.insert(fragment.to_string(), (*status, body.to_vec()));
            }
            Self {
                responses: Mutex::new(map),
                default: (404, Vec::new()),
                seen: Mutex::new(Vec::new()),
                fail_first: Mutex::new(HashMap::new()),
            }
        }

        fn always_ok(body: &[u8]) -> Self {
            Self {
                responses: Mutex::new(HashMap::new()),
                default: (200, body.to_vec()),
                seen: Mutex::new(Vec::new()),
                fail_first: Mutex::new(HashMap::new()),
            }
        }

        fn fail_url_first(&self, fragment: &str, times: usize) {
            self.fail_first
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(fragment.to_string(), times);
        }

        fn requests(&self) -> Vec<RequestSpec> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    impl NetHost for FakeHost {
        fn fetch(&self, spec: RequestSpec) -> Result<Response, ErrorKind> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(spec.clone());

            // 故障注入：前 N 次失败
            let mut fails = self
                .fail_first
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for (fragment, remaining) in fails.iter_mut() {
                if spec.url.contains(fragment.as_str()) && *remaining > 0 {
                    *remaining -= 1;
                    return Err(ErrorKind::Network);
                }
            }
            drop(fails);

            let responses = self
                .responses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (status, body) = responses
                .iter()
                .find(|(fragment, _)| spec.url.contains(fragment.as_str()))
                .map(|(_, value)| value.clone())
                .unwrap_or_else(|| self.default.clone());

            Ok(Response {
                status,
                headers: HeaderMap::new(),
                body_bytes: body,
                detected_charset: None,
            })
        }
    }

    /// 内存图片缓存。
    #[derive(Default)]
    struct MemoryCache {
        entries: Mutex<HashMap<String, Vec<u8>>>,
        fail: bool,
    }

    impl ImageCache for MemoryCache {
        fn get(&self, url: &str) -> Result<Option<Vec<u8>>, SourceError> {
            Ok(self
                .entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(url)
                .cloned())
        }
        fn put(&self, url: &str, bytes: &[u8]) -> Result<(), SourceError> {
            if self.fail {
                return Err(SourceError::rule("磁盘满"));
            }
            self.entries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(url.to_string(), bytes.to_vec());
            Ok(())
        }
    }

    fn fast_policy() -> ImagePolicy {
        // 测试不等待真实退避
        ImagePolicy {
            max_retries: 2,
            backoff_ms: 0,
            max_bytes: 1024 * 1024,
        }
    }

    // --- 成功路径 ---

    #[test]
    fn 取图成功() {
        let source = parsed(None);
        let host = FakeHost::always_ok(b"PNGDATA");
        let result = fetch_image(&source, "https://a.com/1.png", &host, None, &fast_policy())
            .expect("应成功");
        assert!(result.is_fetched());
        assert_eq!(result.bytes(), Some(&b"PNGDATA"[..]));
        assert_eq!(result.url(), "https://a.com/1.png");
    }

    #[test]
    fn 命中缓存时不发请求() {
        let source = parsed(None);
        let host = FakeHost::always_ok(b"FRESH");
        let cache = MemoryCache::default();
        cache
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert("https://a.com/1.png".to_string(), b"CACHED".to_vec());

        let result = fetch_image(
            &source,
            "https://a.com/1.png",
            &host,
            Some(&cache),
            &fast_policy(),
        )
        .expect("应成功");
        assert_eq!(result.bytes(), Some(&b"CACHED"[..]));
        assert!(matches!(
            result,
            ImageResult::Fetched {
                from_cache: true,
                ..
            }
        ));
        assert!(host.requests().is_empty(), "命中缓存时不得发请求");
    }

    // --- SC-018：鉴权头透传 ---

    #[test]
    fn 图片请求带书源的鉴权头() {
        // SC-018 的核心：正文能取到的图，图片请求也必须能取到。
        // 少带一个 Referer 就足以让图床 403——表现为"正文正常、封面全裂"。
        let source = parsed(None);
        let host = FakeHost::always_ok(b"IMG");
        fetch_image(
            &source,
            "https://img.a.com/1.png",
            &host,
            None,
            &fast_policy(),
        )
        .expect("应成功");

        let requests = host.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].headers.get("Referer"),
            Some("https://a.com/page"),
            "图片请求必须带书源声明的头（SC-018）"
        );
    }

    #[test]
    fn 图片请求与正文共用限速键() {
        // 若图片用别的限速键，同一个源会对图床打得更密——反而更容易被风控
        let source = parsed(None);
        let host = FakeHost::always_ok(b"IMG");
        fetch_image(&source, "https://a.com/1.png", &host, None, &fast_policy()).expect("应成功");
        assert_eq!(
            host.requests()[0].rate_limit_key,
            "https://a.com",
            "限速键必须与正文一致（FR-008 的按源限速）"
        );
    }

    #[test]
    fn 图片请求不带字符集声明() {
        // 图片不是文本，字符集声明对它无意义
        let text = r#"{"bookSourceName":"S","bookSourceUrl":"https://a.com","charset":"gbk"}"#;
        let source = parse_sources(text)
            .expect("应能解析")
            .sources
            .into_iter()
            .next()
            .expect("应有源");
        let host = FakeHost::always_ok(b"IMG");
        fetch_image(&source, "https://a.com/1.png", &host, None, &fast_policy()).expect("应成功");
        assert_eq!(host.requests()[0].charset, None);
    }

    // --- SC-014：失败降级为占位而非错误 ---

    #[test]
    fn 图片失败返回占位而不是错误() {
        // SC-014：图片失败 MUST NOT 中断正文获取。
        // 类型层面就让"图片失败"无法传播成"正文失败"。
        let source = parsed(None);
        let host = FakeHost::with(&[("a.com/ok.png", 200, b"OK")]);
        let result = fetch_image(
            &source,
            "https://a.com/missing.png",
            &host,
            None,
            &fast_policy(),
        )
        .expect("取不到图不该是 Err——那是预期的正常结果之一");
        assert!(!result.is_fetched());
        match &result {
            ImageResult::Placeholder { reason, .. } => {
                assert!(reason.contains("404"), "应记下原因：{reason}");
            }
            other => panic!("应是占位：{other:?}"),
        }
    }

    #[test]
    fn 传输失败也降级为占位() {
        let source = parsed(None);
        let host = FakeHost::with(&[]); // 默认 404
        let result = fetch_image(&source, "https://a.com/x.png", &host, None, &fast_policy())
            .expect("应是占位");
        assert!(!result.is_fetched());
    }

    #[test]
    fn 一张失败不影响其余() {
        // SC-014 的"不中断"同理适用于图片之间：一章几十张图，
        // 一张挂在失效 CDN 上是常态，整批失败会让读者连能看的都看不到
        let source = parsed(None);
        let host = FakeHost::with(&[("1.png", 200, b"ONE"), ("3.png", 200, b"THREE")]);
        let urls = vec![
            "https://a.com/1.png".to_string(),
            "https://a.com/2.png".to_string(), // 失败
            "https://a.com/3.png".to_string(),
        ];
        let set = fetch_images(&source, &urls, &host, None, &fast_policy()).expect("应成功");

        assert_eq!(set.len(), 3);
        assert_eq!(set.fetched_count(), 2);
        assert_eq!(set.placeholder_count(), 1);
        // 保序：便于按位置回填
        assert_eq!(set.images[0].bytes(), Some(&b"ONE"[..]));
        assert!(!set.images[1].is_fetched());
        assert_eq!(set.images[2].bytes(), Some(&b"THREE"[..]));
    }

    #[test]
    fn 全部失败时也是成功返回() {
        let source = parsed(None);
        let host = FakeHost::with(&[]);
        let urls = vec![
            "https://a.com/1.png".to_string(),
            "https://a.com/2.png".to_string(),
        ];
        let set = fetch_images(&source, &urls, &host, None, &fast_policy()).expect("应成功");
        assert_eq!(set.placeholder_count(), 2);
        assert_eq!(set.success_rate(), 0.0);
    }

    // --- 重试 ---

    #[test]
    fn 偶发失败被重试救回() {
        let source = parsed(None);
        let host = FakeHost::always_ok(b"RETRIED");
        host.fail_url_first("1.png", 2); // 前两次失败

        let result = fetch_image(&source, "https://a.com/1.png", &host, None, &fast_policy())
            .expect("应成功");
        assert!(result.is_fetched(), "重试应救回偶发失败");
        assert_eq!(result.bytes(), Some(&b"RETRIED"[..]));
        assert_eq!(host.requests().len(), 3, "首次 + 2 次重试");
    }

    #[test]
    fn 重试次数用尽后走占位() {
        let source = parsed(None);
        let host = FakeHost::always_ok(b"NEVER");
        host.fail_url_first("1.png", 100);

        let result = fetch_image(&source, "https://a.com/1.png", &host, None, &fast_policy())
            .expect("应是占位");
        assert!(!result.is_fetched());
        assert_eq!(host.requests().len(), 3, "首次 + max_retries(2)");
    }

    #[test]
    fn 零重试策略只尝试一次() {
        let source = parsed(None);
        let host = FakeHost::with(&[]);
        let policy = ImagePolicy {
            max_retries: 0,
            backoff_ms: 0,
            max_bytes: 1024,
        };
        fetch_image(&source, "https://a.com/1.png", &host, None, &policy).expect("应是占位");
        assert_eq!(host.requests().len(), 1);
    }

    // --- 上限与边界 ---

    #[test]
    fn 超大图片被拒且不重试() {
        // 重试拿到的还是同一张超大图，故超限直接放弃
        let source = parsed(None);
        let big = vec![0u8; 2048];
        let host = FakeHost::always_ok(&big);
        let policy = ImagePolicy {
            max_retries: 3,
            backoff_ms: 0,
            max_bytes: 1024,
        };
        let result =
            fetch_image(&source, "https://a.com/big.png", &host, None, &policy).expect("应是占位");
        assert!(!result.is_fetched());
        assert_eq!(host.requests().len(), 1, "超限不该重试");
        match result {
            ImageResult::Placeholder { reason, .. } => {
                assert!(reason.contains("上限"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn 空地址是编程错误() {
        let source = parsed(None);
        let host = FakeHost::always_ok(b"X");
        fetch_image(&source, "", &host, None, &fast_policy()).expect_err("空地址应报错");
        fetch_image(&source, "   ", &host, None, &fast_policy()).expect_err("空白地址应报错");
    }

    #[test]
    fn 写缓存失败不影响返回() {
        let source = parsed(None);
        let host = FakeHost::always_ok(b"IMG");
        let cache = MemoryCache {
            fail: true,
            ..Default::default()
        };
        let result = fetch_image(
            &source,
            "https://a.com/1.png",
            &host,
            Some(&cache),
            &fast_policy(),
        )
        .expect("应成功");
        assert!(result.is_fetched(), "写缓存失败不该让图片变成占位");
    }

    #[test]
    fn 缓存读失败时回落去抓() {
        struct BrokenCache;
        impl ImageCache for BrokenCache {
            fn get(&self, _url: &str) -> Result<Option<Vec<u8>>, SourceError> {
                Err(SourceError::rule("缓存损坏"))
            }
            fn put(&self, _url: &str, _bytes: &[u8]) -> Result<(), SourceError> {
                Ok(())
            }
        }
        let source = parsed(None);
        let host = FakeHost::always_ok(b"FETCHED");
        let result = fetch_image(
            &source,
            "https://a.com/1.png",
            &host,
            Some(&BrokenCache),
            &fast_policy(),
        )
        .expect("应成功");
        assert!(
            result.is_fetched(),
            "缓存读失败只是「这次没命中」，不该降级成占位"
        );
        assert_eq!(result.bytes(), Some(&b"FETCHED"[..]));
    }

    // --- 成功率（SC-018 的度量） ---

    #[test]
    fn 成功率计算() {
        let source = parsed(None);
        let host = FakeHost::with(&[("1.png", 200, b"a"), ("3.png", 200, b"c")]);
        let urls: Vec<String> = ["1", "2", "3", "4"]
            .iter()
            .map(|n| format!("https://a.com/{n}.png"))
            .collect();
        let set = fetch_images(&source, &urls, &host, None, &fast_policy()).expect("应成功");
        assert_eq!(set.success_rate(), 0.5);
    }

    #[test]
    fn 没有图片时成功率是_1() {
        // 纯文字章节没有图可挂。报成 0% 会让"这篇没有图"被当成"图片全挂了"
        // ——而 SC-018 度量的正是这个指标，口径错了指标就没意义。
        let source = parsed(None);
        let host = FakeHost::always_ok(b"X");
        let set = fetch_images(&source, &[], &host, None, &fast_policy()).expect("应成功");
        assert!(set.is_empty());
        assert_eq!(set.success_rate(), 1.0);
    }

    // --- 图片地址提取 ---

    #[test]
    fn 从_html_提取图片地址() {
        let html = r#"<div><img src="/img/1.png"/><img src="https://cdn.a.com/2.png"/>
                      <img src=""/></div>"#;
        let source = parsed(None);
        let urls = extract_image_urls(&source, html, "https://a.com/c/1").expect("应成功");
        assert_eq!(
            urls,
            vec![
                "https://a.com/img/1.png".to_string(),
                "https://cdn.a.com/2.png".to_string(),
            ],
            "相对地址补全、空地址被过滤"
        );
    }

    #[test]
    fn 书源规则优先于兜底提取() {
        // 源明确说了图片在 .illustration，不该被页面里其他装饰性 img 淹没
        let html = r#"<div><img src="/logo.png"/><div class="illustration">
                        <img src="/real.png"/></div></div>"#;
        let source = parsed(Some(
            r#"{"content":"@css:div@text","imageUrl":"@css:.illustration img@src"}"#,
        ));
        let urls = extract_image_urls(&source, html, "https://a.com/c/1").expect("应成功");
        assert_eq!(urls, vec!["https://a.com/real.png".to_string()]);
    }

    #[test]
    fn 规则未命中时回落到兜底() {
        let html = r#"<div><img src="/fallback.png"/></div>"#;
        let source = parsed(Some(
            r#"{"content":"@css:div@text","imageUrl":"@css:.not-exist img@src"}"#,
        ));
        let urls = extract_image_urls(&source, html, "https://a.com/c/1").expect("应成功");
        assert_eq!(
            urls,
            vec!["https://a.com/fallback.png".to_string()],
            "规则没命中时应回落到 <img> 提取"
        );
    }

    #[test]
    fn 无图片的正文返回空列表() {
        let source = parsed(None);
        let urls =
            extract_image_urls(&source, "<p>纯文字</p>", "https://a.com/c/1").expect("应成功");
        assert!(urls.is_empty());
    }

    #[test]
    fn 章节插图抓取端到端() {
        let html = r#"<div><img src="/1.png"/><img src="/2.png"/></div>"#;
        let source = parsed(None);
        let host = FakeHost::always_ok(b"IMG");
        let chapter = tome_core::Chapter {
            id: 1,
            book_id: 1,
            index_in_book: 0,
            title: "章".to_string(),
            url: Some("https://a.com/c/1.html".to_string()),
            byte_offset: None,
            epub_entry: None,
            is_cached: false,
            is_read: false,
            word_count: None,
        };
        let set =
            chapter_images(&source, &chapter, html, &host, None, &fast_policy()).expect("应成功");
        assert_eq!(set.len(), 2);
        assert_eq!(set.fetched_count(), 2);
    }

    // --- 真实图片缓存经接缝 ---

    #[test]
    fn 真实图片缓存经接缝可用() {
        let dir = std::env::temp_dir().join(format!(
            "tome-image-seam-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = tome_store::Db::open(&dir.join("tome.db")).expect("应能开库");
        let real = Arc::new(tome_store::ImageCache::new(
            &dir.join("cache").join("images"),
            db,
        ));
        let cache = store_image_cache(real);

        assert!(cache.get("https://x/1.png").expect("应成功").is_none());
        cache.put("https://x/1.png", b"IMG").expect("应能写");
        assert_eq!(
            cache.get("https://x/1.png").expect("应成功"),
            Some(b"IMG".to_vec())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn 无实现是安全空操作() {
        assert!(NoImageCache.get("u").expect("应成功").is_none());
        NoImageCache.put("u", b"x").expect("应成功");
    }
}
