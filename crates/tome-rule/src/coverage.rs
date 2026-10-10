//! \[DONE] FR-005b 能力清单的覆盖矩阵（tasks.md T085 / FR-038）
//!
//! # 它解决什么问题
//!
//! SC-002a 的通过率是个**单一数字**：它回答"跑过的用例对不对"，却不回答
//! "该测的测到了没有"。一个 96% 的通过率完全可能来自"只有 CSS 有 284 条
//! 用例、其余能力一条没有"——数字好看，兼容性却没被度量。
//!
//! FR-005b 的原文要求正是补上这一面：
//!
//! > 该能力集 MUST 有一份显式的、可被测试逐项覆盖的清单，作为一致性验证
//! > 用例集的**覆盖矩阵**（见 FR-038）
//!
//! 因此本模块产出的是"能力 → 用例数"的矩阵，并把**没有任何用例的能力**
//! 单列为缺口（gap）——而不是让它们静默消失。
//!
//! # 清单从哪来
//!
//! 单一事实来源是 [`CAPABILITY_MANIFEST`]：它逐项列出 FR-005 八类能力下的
//! **规则语法面**（书源作者实际书写的形态）。每一项都有：
//!
//! - `id`：稳定标识（报告里用它，改名会破坏回归对比）
//! - `category`：FR-005 的八个类别之一
//! - `probe`：**如何判定语料覆盖了这一项**——见下
//!
//! ## 为什么用"探针谓词"而不是"用例命名约定"
//!
//! 两种做法都能实现覆盖统计：
//!
//! | 做法 | 缺陷 |
//! |---|---|
//! | 约定用例名前缀含能力 id | 要求 284 条既有用例全部改名；且"名字写了但内容没测"无从发现 |
//! | **看规则原文里是否出现该能力的语法标记**（本模块采用） | 略保守（宁可判"未覆盖"也不误判"已覆盖"） |
//!
//! 选后者：**误判"已覆盖"是危险的**（它会掩盖缺口），误判"未覆盖"只是吵一点。
//! 每个探针都是对 `rule` 原文的纯函数判定，可被独立审查与测试。
//!
//! # 与 `--corpus` 的关系
//!
//! 本模块只做统计，不做求值——求值在 [`crate::corpus`]。两者在 CLI 侧合并
//! 成同一份报告（`verify --corpus` 输出一致率 + 覆盖矩阵）。

use crate::corpus::CorpusCase;

/// FR-005 的八个能力类别。
///
/// 与 spec 的 FR-005 逐条对应。新增类别时**必须**同时更新
/// [`CAPABILITY_MANIFEST`]，否则该类下的能力项无处归属——有一个测试
/// 断言"清单里每个类别的 id 都在本枚举里"来守这件事。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CapabilityCategory {
    /// 网络：单次请求、并发批量请求、下载到缓存、下载到文件
    Network,
    /// 文本与编码：Base64、URL 编解码、十六进制、字符集转换、中文简繁转换
    TextEncoding,
    /// 摘要与加密：MD5/SHA 系列摘要、AES/DES/RSA 加解密与签名
    Crypto,
    /// 结构化数据：JSON/XML 解析与路径取值、网页标记解析与选择
    StructuredData,
    /// 正则：匹配、查找、替换、捕获组提取
    Regex,
    /// 时间日期：时间戳与格式化字符串互转、日期计算
    TimeDate,
    /// 杂项：随机数、压缩与解压、HTML 实体处理
    Misc,
    /// 宿主状态：缓存/文件/Cookie/外部脚本/变量池/上下文
    HostState,
}

impl CapabilityCategory {
    /// 类别标识（报告里用，稳定）。
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::TextEncoding => "textEncoding",
            Self::Crypto => "crypto",
            Self::StructuredData => "structuredData",
            Self::Regex => "regex",
            Self::TimeDate => "timeDate",
            Self::Misc => "misc",
            Self::HostState => "hostState",
        }
    }

    /// 中文名（报告的 human 输出用）。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Network => "网络",
            Self::TextEncoding => "文本与编码",
            Self::Crypto => "摘要与加密",
            Self::StructuredData => "结构化数据",
            Self::Regex => "正则",
            Self::TimeDate => "时间日期",
            Self::Misc => "杂项",
            Self::HostState => "宿主状态",
        }
    }

    /// 全部类别（供遍历与报告分组）。
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Network,
            Self::TextEncoding,
            Self::Crypto,
            Self::StructuredData,
            Self::Regex,
            Self::TimeDate,
            Self::Misc,
            Self::HostState,
        ]
    }
}

/// 一项能力的覆盖判定探针。
///
/// 探针拿到一条用例的 `rule` 原文，返回"这条用例是否触达了本能力"。
/// 必须是**纯函数**——覆盖统计不该依赖求值结果（求值失败也可能确实测到了
/// 该语法面；那是 SC-002a 要管的事，不是覆盖矩阵要管的）。
pub type Probe = fn(&str) -> bool;

/// 一条能力项。
pub struct CapabilityItem {
    /// 稳定标识（报告用；改名会破坏回归对比，故视为契约）。
    pub id: &'static str,
    /// 所属 FR-005 类别。
    pub category: CapabilityCategory,
    /// 人类可读说明。
    pub description: &'static str,
    /// 覆盖判定探针。
    pub probe: Probe,
}

/// 探针集合：每个都是对 `rule` 原文的纯判定。
///
/// ## 为什么写成具名函数而不是闭包
///
/// [`Probe`] 是 `fn(&str) -> bool`（函数指针），不是 `Box<dyn Fn>`。
/// 选函数指针是为了让清单能写成 `const`/`static`，且无分配、无生命周期
/// 纠缠。代价是每个探针都得具名——但这也让每个探针可被单独测试。
pub mod probes {
    /// 规则里含某标记。
    fn has(rule: &str, marker: &str) -> bool {
        rule.contains(marker)
    }

    /// `java.ajax(...)` —— 单次请求。
    pub fn net_ajax(rule: &str) -> bool {
        has(rule, "ajax(") || has(rule, "java.get(") || has(rule, "java.post(")
    }

    /// `java.ajaxAll(...)` —— 并发批量请求。
    pub fn net_ajax_all(rule: &str) -> bool {
        has(rule, "ajaxAll(")
    }

    /// `java.connect(...)` —— 连接（底层请求形态）。
    pub fn net_connect(rule: &str) -> bool {
        has(rule, "connect(") || has(rule, "head(")
    }

    /// `java.cacheFile` / `java.downloadFile` —— 下载到缓存/文件。
    pub fn net_download(rule: &str) -> bool {
        has(rule, "cacheFile(") || has(rule, "downloadFile(")
    }

    /// Base64 编解码。
    pub fn text_base64(rule: &str) -> bool {
        has(rule, "base64Encode(") || has(rule, "base64Decode(")
    }

    /// URL 编解码。
    pub fn text_url(rule: &str) -> bool {
        has(rule, "encodeURI(") || has(rule, "decodeURI(")
    }

    /// 十六进制编解码。
    pub fn text_hex(rule: &str) -> bool {
        has(rule, "hexEncodeToString(") || has(rule, "hexDecodeToString(")
    }

    /// 字符集转换。
    pub fn text_charset(rule: &str) -> bool {
        // 见 `crates/tome-js/src/api/net.rs` 的字符集参数用法（utf8/gbk/big5）
        has(rule, "gbk") || has(rule, "GBK") || has(rule, "big5")
    }

    /// 中文简繁转换。
    pub fn text_trad_simp(rule: &str) -> bool {
        has(rule, "s2t(") || has(rule, "t2s(")
    }

    /// MD5 摘要。
    pub fn crypto_md5(rule: &str) -> bool {
        has(rule, "md5(") || has(rule, "md5Encode(")
    }

    /// SHA 系列摘要。
    pub fn crypto_sha(rule: &str) -> bool {
        has(rule, "sha1Encode(") || has(rule, "sha256Encode(") || has(rule, "sha512Encode(")
    }

    /// AES 加解密。
    pub fn crypto_aes(rule: &str) -> bool {
        (has(rule, "aesEncode") && !has(rule, "aesEncodeHttp"))
            || (has(rule, "aesDecode") && !has(rule, "aesDecodeHttp"))
    }

    /// DES 加解密。
    pub fn crypto_des(rule: &str) -> bool {
        has(rule, "desEncode(") || has(rule, "desDecode(")
    }

    /// RSA 加解密。
    pub fn crypto_rsa(rule: &str) -> bool {
        has(rule, "rsaEncode(") || has(rule, "rsaDecode(")
    }

    /// JSON 解析与路径取值。
    pub fn data_json(rule: &str) -> bool {
        has(rule, "@json:") || has(rule, "jsonParse(") || has(rule, "jsonPath(")
    }

    /// XML 解析与选择。
    pub fn data_xml(rule: &str) -> bool {
        has(rule, "xmlParse(") || has(rule, "xml(")
    }

    /// 网页标记解析与选择（CSS / Default / XPath）。
    pub fn data_markup(rule: &str) -> bool {
        has(rule, "@css:")
            || has(rule, "@XPath:")
            || has(rule, "getElement(")
            || has(rule, "getElements(")
    }

    /// 正则匹配。
    pub fn regex_match(rule: &str) -> bool {
        has(rule, "match(") || has(rule, "matches(")
    }

    /// 正则查找与替换。
    pub fn regex_replace(rule: &str) -> bool {
        has(rule, "##") || has(rule, "replace(")
    }

    /// 捕获组提取。
    pub fn regex_groups(rule: &str) -> bool {
        has(rule, "getMatchGroups(") || has(rule, "$1") || has(rule, "$2")
    }

    /// 全部正则能力（三合一探针）。
    pub fn regex_any(rule: &str) -> bool {
        regex_match(rule) || regex_replace(rule) || regex_groups(rule)
    }

    /// 时间戳与格式化互转。
    pub fn time_format(rule: &str) -> bool {
        has(rule, "timeFormat(") || has(rule, "timeFormatMs(")
    }

    /// 当前时间。
    pub fn time_now(rule: &str) -> bool {
        has(rule, "now()") || has(rule, "Date.now(")
    }

    /// 日期计算（时间戳算术）。
    pub fn time_arithmetic(rule: &str) -> bool {
        has(rule, "timeFormat(") && (has(rule, "+") || has(rule, "-"))
    }

    /// 随机数。
    pub fn misc_random(rule: &str) -> bool {
        has(rule, "random(") || has(rule, "randomInt(") || has(rule, "Math.random(")
    }

    /// 压缩与解压。
    pub fn misc_compress(rule: &str) -> bool {
        has(rule, "gzipEncode(")
            || has(rule, "gzipDecode(")
            || has(rule, "inflate(")
            || has(rule, "deflate(")
    }

    /// HTML 实体处理。
    pub fn misc_html_entity(rule: &str) -> bool {
        has(rule, "htmlEncode(") || has(rule, "htmlDecode(")
    }

    /// 缓存读写。
    pub fn state_cache(rule: &str) -> bool {
        has(rule, "cacheGet(") || has(rule, "cachePut(")
    }

    /// 文件读写。
    pub fn state_file(rule: &str) -> bool {
        has(rule, "readFile(") || has(rule, "writeFile(")
    }

    /// Cookie 读写。
    pub fn state_cookie(rule: &str) -> bool {
        has(rule, "getCookie(") || has(rule, "setCookie(")
    }

    /// 引入外部脚本。
    pub fn state_import_script(rule: &str) -> bool {
        has(rule, "importScript(")
    }

    /// 变量池读写。
    pub fn state_variables(rule: &str) -> bool {
        has(rule, "getVariable(") || has(rule, "setVariable(")
    }

    /// 当前书源/书籍/章节上下文访问。
    pub fn state_context(rule: &str) -> bool {
        has(rule, "java.source")
            || has(rule, "java.book")
            || has(rule, "java.chapter")
            || has(rule, "baseUrl")
    }

    /// JS 脚本求值本身（`@js:` 与替换链里的 JS 片段）。
    pub fn js_eval(rule: &str) -> bool {
        has(rule, "@js:") || has(rule, "{{")
    }

    /// 替换链（`##` 与 `{{}}` 插值）。
    pub fn replace_chain(rule: &str) -> bool {
        has(rule, "##")
    }

    /// 组合符（`&&` / `||` / `%%`）。
    pub fn combinator(rule: &str) -> bool {
        has(rule, "&&") || has(rule, "||") || has(rule, "%%")
    }

    /// 反序标记（规则前导 `-`）。
    pub fn reversed(rule: &str) -> bool {
        rule.trim_start().starts_with('-')
    }
}

/// FR-005b 能力清单（**单一事实来源**）。
///
/// 逐项覆盖 FR-005 的八类。每项绑定一个探针（见 [`probes`]）。
/// 顺序即报告的呈现顺序，**稳定**。
pub static CAPABILITY_MANIFEST: &[CapabilityItem] = &[
    // ---- 网络 ------------------------------------------------------------
    CapabilityItem {
        id: "network.ajax",
        category: CapabilityCategory::Network,
        description: "单次请求（java.ajax/java.get/java.post）",
        probe: probes::net_ajax,
    },
    CapabilityItem {
        id: "network.ajaxAll",
        category: CapabilityCategory::Network,
        description: "并发批量请求（java.ajaxAll）",
        probe: probes::net_ajax_all,
    },
    CapabilityItem {
        id: "network.connect",
        category: CapabilityCategory::Network,
        description: "底层连接（java.connect/java.head）",
        probe: probes::net_connect,
    },
    CapabilityItem {
        id: "network.download",
        category: CapabilityCategory::Network,
        description: "下载到缓存/文件（cacheFile/downloadFile）",
        probe: probes::net_download,
    },
    // ---- 文本与编码 ------------------------------------------------------
    CapabilityItem {
        id: "text.base64",
        category: CapabilityCategory::TextEncoding,
        description: "Base64 编解码",
        probe: probes::text_base64,
    },
    CapabilityItem {
        id: "text.url",
        category: CapabilityCategory::TextEncoding,
        description: "URL 编解码",
        probe: probes::text_url,
    },
    CapabilityItem {
        id: "text.hex",
        category: CapabilityCategory::TextEncoding,
        description: "十六进制编解码",
        probe: probes::text_hex,
    },
    CapabilityItem {
        id: "text.charset",
        category: CapabilityCategory::TextEncoding,
        description: "字符集转换",
        probe: probes::text_charset,
    },
    CapabilityItem {
        id: "text.tradSimp",
        category: CapabilityCategory::TextEncoding,
        description: "中文简繁转换",
        probe: probes::text_trad_simp,
    },
    // ---- 摘要与加密 ------------------------------------------------------
    CapabilityItem {
        id: "crypto.md5",
        category: CapabilityCategory::Crypto,
        description: "MD5 摘要",
        probe: probes::crypto_md5,
    },
    CapabilityItem {
        id: "crypto.sha",
        category: CapabilityCategory::Crypto,
        description: "SHA 系列摘要",
        probe: probes::crypto_sha,
    },
    CapabilityItem {
        id: "crypto.aes",
        category: CapabilityCategory::Crypto,
        description: "AES 加解密",
        probe: probes::crypto_aes,
    },
    CapabilityItem {
        id: "crypto.des",
        category: CapabilityCategory::Crypto,
        description: "DES 加解密",
        probe: probes::crypto_des,
    },
    CapabilityItem {
        id: "crypto.rsa",
        category: CapabilityCategory::Crypto,
        description: "RSA 加解密与签名",
        probe: probes::crypto_rsa,
    },
    // ---- 结构化数据 ------------------------------------------------------
    CapabilityItem {
        id: "data.json",
        category: CapabilityCategory::StructuredData,
        description: "JSON 解析与路径取值",
        probe: probes::data_json,
    },
    CapabilityItem {
        id: "data.xml",
        category: CapabilityCategory::StructuredData,
        description: "XML 解析与选择",
        probe: probes::data_xml,
    },
    CapabilityItem {
        id: "data.markup",
        category: CapabilityCategory::StructuredData,
        description: "网页标记解析与选择（CSS/Default/XPath）",
        probe: probes::data_markup,
    },
    // ---- 正则 ------------------------------------------------------------
    CapabilityItem {
        id: "regex.match",
        category: CapabilityCategory::Regex,
        description: "正则匹配",
        probe: probes::regex_match,
    },
    CapabilityItem {
        id: "regex.replace",
        category: CapabilityCategory::Regex,
        description: "正则查找与替换",
        probe: probes::regex_replace,
    },
    CapabilityItem {
        id: "regex.groups",
        category: CapabilityCategory::Regex,
        description: "捕获组提取",
        probe: probes::regex_groups,
    },
    // ---- 时间日期 --------------------------------------------------------
    CapabilityItem {
        id: "time.format",
        category: CapabilityCategory::TimeDate,
        description: "时间戳与格式化字符串互转",
        probe: probes::time_format,
    },
    CapabilityItem {
        id: "time.now",
        category: CapabilityCategory::TimeDate,
        description: "当前时间",
        probe: probes::time_now,
    },
    CapabilityItem {
        id: "time.arithmetic",
        category: CapabilityCategory::TimeDate,
        description: "日期计算",
        probe: probes::time_arithmetic,
    },
    // ---- 杂项 ------------------------------------------------------------
    CapabilityItem {
        id: "misc.random",
        category: CapabilityCategory::Misc,
        description: "随机数",
        probe: probes::misc_random,
    },
    CapabilityItem {
        id: "misc.compress",
        category: CapabilityCategory::Misc,
        description: "压缩与解压",
        probe: probes::misc_compress,
    },
    CapabilityItem {
        id: "misc.htmlEntity",
        category: CapabilityCategory::Misc,
        description: "HTML 实体处理",
        probe: probes::misc_html_entity,
    },
    // ---- 宿主状态 --------------------------------------------------------
    CapabilityItem {
        id: "state.cache",
        category: CapabilityCategory::HostState,
        description: "缓存读写",
        probe: probes::state_cache,
    },
    CapabilityItem {
        id: "state.file",
        category: CapabilityCategory::HostState,
        description: "文件读写（限数据目录内）",
        probe: probes::state_file,
    },
    CapabilityItem {
        id: "state.cookie",
        category: CapabilityCategory::HostState,
        description: "Cookie 读写",
        probe: probes::state_cookie,
    },
    CapabilityItem {
        id: "state.importScript",
        category: CapabilityCategory::HostState,
        description: "引入外部脚本",
        probe: probes::state_import_script,
    },
    CapabilityItem {
        id: "state.variables",
        category: CapabilityCategory::HostState,
        description: "变量池读写",
        probe: probes::state_variables,
    },
    CapabilityItem {
        id: "state.context",
        category: CapabilityCategory::HostState,
        description: "当前书源/书籍/章节上下文访问",
        probe: probes::state_context,
    },
];

/// 规则语法面的覆盖项（非 FR-005 的 JS 宿主 API，但同属书源兼容性）。
///
/// ## 为什么单列而不混进 `CAPABILITY_MANIFEST`
///
/// FR-005 的八类说的是**脚本里可调用的能力**；而 CSS/XPath/JSONPath/替换链
/// 是**规则本身的语法**——它们不写在 `java.*` 里。把两者混成一张表会让
/// "某类 JS API 没测"与"某语法点没测"这两件不同的事在报告里分不开。
///
/// 二者在报告里都是覆盖矩阵的一部分。
pub static RULE_SYNTAX_MANIFEST: &[CapabilityItem] = &[
    CapabilityItem {
        id: "syntax.js",
        category: CapabilityCategory::StructuredData,
        description: "JS 脚本求值（@js: 与 {{}} 片段）",
        probe: probes::js_eval,
    },
    CapabilityItem {
        id: "syntax.replaceChain",
        category: CapabilityCategory::Regex,
        description: "替换链（## 与 {{}} 插值）",
        probe: probes::replace_chain,
    },
    CapabilityItem {
        id: "syntax.combinator",
        category: CapabilityCategory::StructuredData,
        description: "组合符（&& / || / %%）",
        probe: probes::combinator,
    },
    CapabilityItem {
        id: "syntax.reversed",
        category: CapabilityCategory::StructuredData,
        description: "反序标记（规则前导 -）",
        probe: probes::reversed,
    },
];

/// 一项能力的覆盖结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageEntry {
    /// 能力标识
    pub id: String,
    /// 所属类别
    pub category: CapabilityCategory,
    /// 说明
    pub description: String,
    /// 命中该能力的用例数
    pub covered: usize,
    /// 命中用例的名字（供缺口排查；限前若干条）
    pub case_names: Vec<String>,
}

impl CoverageEntry {
    /// 是否有用例覆盖。
    #[must_use]
    pub fn is_covered(&self) -> bool {
        self.covered > 0
    }
}

/// 覆盖矩阵。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverageMatrix {
    /// 逐项覆盖结果（顺序同清单）
    pub entries: Vec<CoverageEntry>,
}

impl CoverageMatrix {
    /// 覆盖了的能力数。
    #[must_use]
    pub fn covered_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.is_covered())
            .count()
    }

    /// 覆盖项总数。
    #[must_use]
    pub fn total(&self) -> usize {
        self.entries.len()
    }

    /// 覆盖率（0.0–1.0）。空清单算 1.0（没有缺口可言）。
    #[must_use]
    pub fn coverage_rate(&self) -> f64 {
        if self.entries.is_empty() {
            return 1.0;
        }
        self.covered_count() as f64 / self.total() as f64
    }

    /// **缺口**：清单里有、但语料里无对应用例的能力。
    ///
    /// 这是 T085 的核心产出——tasks.md 的验收项原文：
    /// 「任一能力无对应用例则报告缺口」。
    pub fn gaps(&self) -> impl Iterator<Item = &CoverageEntry> {
        self.entries.iter().filter(|entry| !entry.is_covered())
    }

    /// 按类别聚合：`类别 → (已覆盖, 总数)`。
    #[must_use]
    pub fn by_category(&self) -> Vec<(CapabilityCategory, usize, usize)> {
        let mut out: Vec<(CapabilityCategory, usize, usize)> = Vec::new();
        for entry in &self.entries {
            let covered = usize::from(entry.is_covered());
            match out
                .iter_mut()
                .find(|(category, _, _)| *category == entry.category)
            {
                Some((_, covered_count, total_count)) => {
                    *covered_count += covered;
                    *total_count += 1;
                }
                None => out.push((entry.category, covered, 1)),
            }
        }
        out
    }
}

/// 每条能力在报告里保留的用例名上限（防止报告里塞进 284 个名字）。
const CASE_NAME_SAMPLE_LIMIT: usize = 3;

/// 对一批用例算覆盖矩阵。
///
/// `manifest` 允许传 `CAPABILITY_MANIFEST`、`RULE_SYNTAX_MANIFEST`，或二者拼接。
#[must_use]
pub fn compute_coverage(cases: &[CorpusCase], manifest: &[CapabilityItem]) -> CoverageMatrix {
    let entries = manifest
        .iter()
        .map(|item| {
            let mut covered = 0usize;
            let mut case_names = Vec::new();
            for case in cases {
                if (item.probe)(&case.rule) {
                    covered += 1;
                    if case_names.len() < CASE_NAME_SAMPLE_LIMIT {
                        case_names.push(case.name.clone());
                    }
                }
            }
            CoverageEntry {
                id: item.id.to_string(),
                category: item.category,
                description: item.description.to_string(),
                covered,
                case_names,
            }
        })
        .collect();

    CoverageMatrix { entries }
}

/// 全部清单（FR-005b 能力面 + 规则语法面）。
///
/// CLI 侧默认用这个，一次算完两份清单的覆盖。
#[must_use]
pub fn full_manifest() -> Vec<&'static CapabilityItem> {
    CAPABILITY_MANIFEST
        .iter()
        .chain(RULE_SYNTAX_MANIFEST.iter())
        .collect()
}

/// 对全部清单算覆盖矩阵。
#[must_use]
pub fn compute_full_coverage(cases: &[CorpusCase]) -> CoverageMatrix {
    let manifest = full_manifest();
    let items: Vec<CapabilityItem> = manifest
        .into_iter()
        .map(|item| CapabilityItem {
            id: item.id,
            category: item.category,
            description: item.description,
            probe: item.probe,
        })
        .collect();
    compute_coverage(cases, &items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::InputKind;

    fn case(name: &str, rule: &str) -> CorpusCase {
        CorpusCase {
            name: name.to_string(),
            kind: InputKind::Html,
            input: "<p>x</p>".to_string(),
            rule: rule.to_string(),
            expected: vec![],
            basis: "测试内置".to_string(),
            reversed: false,
        }
    }

    #[test]
    fn 清单标识互不重复() {
        let all = full_manifest();
        for (position, item) in all.iter().enumerate() {
            assert!(
                !all[..position].iter().any(|other| other.id == item.id),
                "能力标识重复：{}（标识是报告的契约键，不能重复）",
                item.id
            );
        }
    }

    #[test]
    fn 清单每项都有说明() {
        for item in full_manifest() {
            assert!(
                !item.description.trim().is_empty(),
                "能力 {} 缺说明",
                item.id
            );
        }
    }

    #[test]
    fn fr005_八类都有能力项() {
        // FR-005 的八类若有一类整类为空，那类能力就永远不会出现在报告里
        for category in CapabilityCategory::all() {
            assert!(
                CAPABILITY_MANIFEST
                    .iter()
                    .any(|item| item.category == *category),
                "FR-005 类别 {} 在清单里没有任何能力项",
                category.id()
            );
        }
    }

    #[test]
    fn 探针命中对应规则() {
        // 每个探针**至少**能被一条构造出的规则命中——否则它是个死项，
        // 会在报告里永远显示为缺口（假缺口比漏报更浪费时间）
        let pairs: &[(&str, &str)] = &[
            ("network.ajax", "@js:java.ajax('http://x')"),
            ("network.ajaxAll", "@js:java.ajaxAll(['a'])"),
            ("network.connect", "@js:java.connect('http://x')"),
            ("network.download", "@js:java.downloadFile('u','p')"),
            ("text.base64", "@js:java.base64Encode('x')"),
            ("text.url", "@js:java.encodeURI('x')"),
            ("text.hex", "@js:java.hexEncodeToString('x')"),
            ("text.charset", "@js:java.ajax('u',{charset:'gbk'})"),
            ("text.tradSimp", "@js:java.t2s('漢')"),
            ("crypto.md5", "@js:java.md5Encode('x')"),
            ("crypto.sha", "@js:java.sha256Encode('x')"),
            ("crypto.aes", "@js:java.aesEncode('k','v')"),
            ("crypto.des", "@js:java.desEncode('k','v')"),
            ("crypto.rsa", "@js:java.rsaEncode('k','v')"),
            ("data.json", "@json:$.a"),
            ("data.xml", "@js:java.xmlParse('<a/>')"),
            ("data.markup", "@css:.x@text"),
            ("regex.match", "@js:java.match('a','b')"),
            ("regex.replace", "p@text##a|b"),
            ("regex.groups", "@js:java.getMatchGroups('a','b')"),
            ("time.format", "@js:java.timeFormat(1)"),
            ("time.now", "@js:java.now()"),
            ("time.arithmetic", "@js:java.timeFormat(1+2)"),
            ("misc.random", "@js:java.randomInt(1,2)"),
            ("misc.compress", "@js:java.gzipDecode('x')"),
            ("misc.htmlEntity", "@js:java.htmlDecode('&amp;')"),
            ("state.cache", "@js:java.cacheGet('k')"),
            ("state.file", "@js:java.readFile('p')"),
            ("state.cookie", "@js:java.getCookie('u')"),
            ("state.importScript", "@js:java.importScript('u')"),
            ("state.variables", "@js:java.getVariable('k')"),
            ("state.context", "@js:java.source.name"),
            ("syntax.js", "@js:1+1"),
            ("syntax.replaceChain", "p@text##a|b"),
            ("syntax.combinator", "p@text&&.x@text"),
            ("syntax.reversed", "-li@text"),
        ];

        for (id, rule) in pairs {
            let matrix = compute_full_coverage(&[case("t", rule)]);
            let entry = matrix
                .entries
                .iter()
                .find(|entry| entry.id == *id)
                .unwrap_or_else(|| panic!("清单里没有 {id}"));
            assert!(
                entry.is_covered(),
                "探针 {id} 未被规则 `{rule}` 命中——它会是永久假缺口"
            );
        }
    }

    #[test]
    fn 空语料全部报缺口() {
        let matrix = compute_full_coverage(&[]);
        assert_eq!(matrix.covered_count(), 0);
        assert_eq!(matrix.gaps().count(), matrix.total());
        assert!(!matrix.gaps().next().is_none());
    }

    #[test]
    fn 缺口被如实报告() {
        // 只有 CSS 用例：其余能力都该报缺口
        let matrix = compute_full_coverage(&[case("css:x", "@css:.x@text")]);
        let gap_ids: Vec<&str> = matrix.gaps().map(|entry| entry.id.as_str()).collect();
        assert!(gap_ids.contains(&"network.ajax"), "网络能力无用例应报缺口");
        assert!(gap_ids.contains(&"crypto.md5"), "加密能力无用例应报缺口");
        assert!(
            !gap_ids.contains(&"data.markup"),
            "CSS 用例应覆盖网页标记解析"
        );
    }

    #[test]
    fn 覆盖率与计数() {
        let cases = vec![
            case("a", "@css:.x@text"),
            case("b", "@json:$.a"),
            case("c", "@js:java.md5Encode('x')"),
        ];
        let matrix = compute_full_coverage(&cases);
        assert!(matrix.total() >= 30, "清单应有足够规模");
        // 三条用例各自触达若干项：CSS→data.markup、JSON→data.json、
        // MD5→crypto.md5，且 `@js:` 另触达 syntax.js。断言的是"部分覆盖"
        // 这一性质，而非某个具体数字（清单增删项不该让本测试变红）。
        assert!(matrix.covered_count() > 0, "应至少覆盖一项");
        assert!(
            matrix.covered_count() < matrix.total(),
            "三条用例不可能覆盖全部能力"
        );
        assert!(matrix.coverage_rate() > 0.0 && matrix.coverage_rate() < 1.0);
    }

    #[test]
    fn 按类别聚合() {
        let cases = vec![case("a", "@js:java.md5Encode('x')")];
        let matrix = compute_full_coverage(&cases);
        let stats = matrix.by_category();
        let crypto = stats
            .iter()
            .find(|(category, _, _)| *category == CapabilityCategory::Crypto)
            .expect("应有加密类");
        assert_eq!(crypto.1, 1, "只覆盖了 MD5 一项");
        assert_eq!(crypto.2, 5, "加密类共五项");
    }

    #[test]
    fn 用例名留样且不超过上限() {
        let many: Vec<CorpusCase> = (0..10)
            .map(|index| case(&format!("css:第{index}条"), "@css:.x@text"))
            .collect();
        let matrix = compute_full_coverage(&many);
        let entry = matrix
            .entries
            .iter()
            .find(|entry| entry.id == "data.markup")
            .expect("应有标记解析项");
        assert_eq!(entry.covered, 10, "命中数应完整");
        assert_eq!(
            entry.case_names.len(),
            CASE_NAME_SAMPLE_LIMIT,
            "留样应限流，否则报告会被名字撑爆"
        );
    }

    #[test]
    fn 规则语法面与能力面分列() {
        // 两个清单不重叠：能力 id 用 `类.项`，语法 id 用 `syntax.`
        for item in CAPABILITY_MANIFEST {
            assert!(
                !item.id.starts_with("syntax."),
                "能力清单不该含语法项：{}",
                item.id
            );
        }
        for item in RULE_SYNTAX_MANIFEST {
            assert!(
                item.id.starts_with("syntax."),
                "语法清单的 id 应以 syntax. 开头：{}",
                item.id
            );
        }
    }
}
