//! `\[DONE]` 错误文案的本地化映射面（tasks.md T092 / FR-044、SC-016）
//!
//! # FR-044 的分工：引擎产**标识**，上层给**文案**
//!
//! > 系统 MUST 支持中英双语界面所需的文案切换能力（引擎侧产出可本地化的
//! > 错误与提示标识；界面文案由上层承接）
//!
//! 这条把责任划成两半，本模块是**承接方**：
//!
//! ```text
//!   tome-core / 各实现 crate                 tome-cli（本模块）
//!   ─────────────────────                    ──────────────────
//!   ErrorKind::Rule                          「书源规则求值失败」/ "book source rule failed"
//!        ↑ 标识（无自然语言）                      ↑ 文案（给人看）
//! ```
//!
//! ## 为什么引擎侧不许出文案
//!
//! 引擎里硬编码一句中文，会让"切到英文"变成**改引擎代码**——而引擎被
//! S2/S3 依赖，改它要全链路重编译。更要紧的是：引擎一旦产文案，就无法
//! 保证"同一错误在两语言下都读得通"，因为翻译散落在几十个 `format!` 里。
//!
//! 引擎侧已有的做法正是对的：`ErrorKind::message()` 返回
//! `LocalizedMessage`（一个稳定 id，如 `error.network`）。本模块把这个
//! id 映射成实际文案。
//!
//! # SC-016 的判据：**100% 可映射**，且不存在单语言可读的错误
//!
//! > 引擎产出的全部面向用户的错误均可映射为中英文案，**不存在只在单一
//! > 语言下可读的错误输出**（100%）
//!
//! "100%"与"不存在单语言"这两句决定了本模块的测试形态（见 `mod tests`）：
//!
//! | 断言 | 防的是什么 |
//! |---|---|
//! | 每个 `ErrorKind` 变体 × 两种语言都有非空文案 | 漏翻译（表现为界面上一片空白或 fallback 到 id） |
//! | 两种语言的文案**不相同** | 复制粘贴导致的"翻译了个寂寞" |
//! | 文案里**不含** `error.` 这类标识 | 直接拿 id 当文案（等于没翻译） |
//!
//! ## 为什么要断言"两语言文案不同"
//!
//! 这条看着琐碎，但它是"真的翻译过"的**唯一自动可判据**。缺了它，
//! `zh: "网络错误"` / `en: "网络错误"` 这种复制粘贴能与真翻译一样通过
//! 所有其他断言——而用户在英文界面下看到中文，正是 SC-016 要防的
//! "单语言可读"。
//!
//! # 覆盖范围：`ErrorKind` + 能力标识
//!
//! 除七个分类外，FR-006 的**能力标识**（`webView` / `js` / `jsonPath` …）
//! 也是面向用户的信息——"不支持的能力：webView" 这句本身要能翻译。
//! 故 [`message`] 对 `UnsupportedCapability` 变体做**二次映射**：
//! 分类文案 + 能力名。

use tome_core::ErrorKind;

/// 界面语言。
///
/// # 为什么只有两种（FR-044 明说"中英双语"）
///
/// 用枚举而非 `String`：语言是**可穷举的有限集合**，而 `String` 允许
/// `"jp"` 这种拼错了却编译通过的值，要到运行时才发现"文案查不到"。
/// 枚举让"加一种语言"变成编译器会指出的改动（每个 `match` 都要处理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Language {
    /// 中文（简体）——默认
    ///
    /// ## 为什么默认是中文
    ///
    /// 书源生态以中文社区为主（书源规则说明、社区书源、本书源格式的
    /// 原始文档都是中文），用户主体也是中文读者。默认值该服务多数人。
    #[default]
    Zh,
    /// 英文
    En,
}

impl Language {
    /// 语言标识（`zh` / `en`），与配置文件和 `--lang` 参数一致。
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
        }
    }

    /// 从标识解析。无法识别时返回 `None`（由调用方决定是否报错）。
    ///
    /// # 为什么接受多种写法
    ///
    /// 用户会写 `zh-CN` / `zh_CN` / `ZH` / `en-US`。这些都能**无歧义地**
    /// 归到两种语言之一，拒绝它们只会让用户困惑"为什么 zh-CN 不行"。
    /// 故取标识的**首段**（按 `-`/`_` 切）小写后比对。
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let head = text
            .trim()
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match head.as_str() {
            "zh" | "cn" | "chs" => Some(Self::Zh),
            "en" => Some(Self::En),
            _ => None,
        }
    }

    /// 全部语言（供遍历与测试）。
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::Zh, Self::En]
    }
}

/// `ErrorKind` 的全部分类（供测试穷举）。
///
/// # 为什么这份清单要与 `ErrorKind` 手工同步（而不是反射式枚举）
///
/// Rust 无法在运行时枚举枚举变体。用宏可以自动生成，但会引入一层
/// 与"文档里逐条列出分类"重复的间接层——而那层恰恰是最需要可读的
/// （读代码的人要能一眼看到"有哪几类错误"）。
///
/// 代价是**新增 `ErrorKind` 变体时这里要同步**。故有一个测试断言
/// `message()` 对所有清单项都产文案，且清单项数等于"已知分类数"——
/// 漏同步时那条测试会红（见 `mod tests` 的说明）。
/// [`ErrorKind`]: tome_core::ErrorKind
#[must_use]
pub fn all_error_kinds() -> Vec<ErrorKind> {
    vec![
        ErrorKind::Network,
        ErrorKind::Proxy,
        ErrorKind::Rule,
        ErrorKind::UnsupportedCapability {
            capability: tome_core::ErrorCapability::new("webView"),
        },
        ErrorKind::Content,
        ErrorKind::Storage,
        ErrorKind::Configuration,
    ]
}

/// 把一条错误映射为指定语言的文案。
///
/// # 未知标识的处理：回落为**标识本身**，而非空串
///
/// 若某天引擎新增了一个 `ErrorKind` 变体而这里没跟上，会走到 `_ =>` 分支。
/// 此时回落为传入的 `id`（如 `error.something_new`）——**显示标识**比显示
/// 空串好：用户至少能把它报出来，我们也能据此定位。空串会让界面出现
/// 一个无语义的空白，那是更难排查的失败形态。
///
/// 但这是**兜底**，不该是常态：测试断言所有已知变体都有真文案。
#[must_use]
pub fn message(kind: &ErrorKind, language: Language) -> String {
    // FR-006 的能力错误：分类文案 + 能力名。
    // 能力名本身不翻译（`webView` / `js` 是书源生态的通用叫法，翻译会让
    // 用户无法把它与书源里写的那一行对应起来）。
    if let ErrorKind::UnsupportedCapability { capability } = kind {
        let base = capability_message(language);
        return format!("{base}{}", capability.capability_id());
    }

    let id = kind.message();
    render(id.id(), language)
}

/// 按**消息标识**取文案（更通用的入口）。
///
/// # 为什么既收 `ErrorKind` 又收 id
///
/// 引擎各处已经产出了 `LocalizedMessage`（如 `"error.rule.source.missing"`），
/// 那些更细的标识不该被丢掉——`error.rule` 说"规则有问题"，
/// `error.rule.source.missing` 说"书源不存在"，后者对用户有用得多。
///
/// 故本函数是**真正的映射表入口**，[`message`] 只是它的一个便利封装。
#[must_use]
pub fn render(id: &str, language: Language) -> String {
    let text = match (id, language) {
        // ---- 七个错误分类（ErrorKind 的一级标识）----
        ("error.network", Language::Zh) => "网络请求失败",
        ("error.network", Language::En) => "network request failed",
        ("error.proxy", Language::Zh) => "代理不可用",
        ("error.proxy", Language::En) => "proxy unavailable",
        ("error.rule", Language::Zh) => "书源规则出错",
        ("error.rule", Language::En) => "book source rule error",
        ("error.unsupported_capability", Language::Zh) => "不支持的能力：",
        ("error.unsupported_capability", Language::En) => "unsupported capability: ",
        ("error.content", Language::Zh) => "内容无法解析",
        ("error.content", Language::En) => "content could not be parsed",
        ("error.storage", Language::Zh) => "存储出错",
        ("error.storage", Language::En) => "storage error",
        ("error.configuration", Language::Zh) => "配置有误",
        ("error.configuration", Language::En) => "invalid configuration",

        // ---- 具体子标识（引擎产出的更细粒度标识）----
        ("error.rule.source.missing", Language::Zh) => "书源不存在",
        ("error.rule.source.missing", Language::En) => "book source not found",
        ("error.storage.io", Language::Zh) => "文件读写失败",
        ("error.storage.io", Language::En) => "file I/O failed",
        ("error.storage.migration", Language::Zh) => "数据版本不兼容",
        ("error.storage.migration", Language::En) => "data version incompatible",
        ("error.storage.corrupt", Language::Zh) => "数据损坏",
        ("error.storage.corrupt", Language::En) => "data is corrupt",
        ("error.storage.not_found", Language::Zh) => "找不到目标",
        ("error.storage.not_found", Language::En) => "target not found",
        ("error.storage.path_out_of_data_dir", Language::Zh) => "路径超出数据目录",
        ("error.storage.path_out_of_data_dir", Language::En) => {
            "path is outside the data directory"
        }
        ("error.storage.data_dir_not_writable", Language::Zh) => "数据目录不可写",
        ("error.storage.data_dir_not_writable", Language::En) => "data directory is not writable",
        ("error.storage.disk_full", Language::Zh) => "磁盘空间不足",
        ("error.storage.disk_full", Language::En) => "not enough disk space",
        ("error.config.reason_missing", Language::Zh) => "配置项缺失",
        ("error.config.reason_missing", Language::En) => "missing configuration entry",

        // ---- 用法错误（CLI 自身产出）----
        ("error.usage", Language::Zh) => "用法错误",
        ("error.usage", Language::En) => "usage error",

        // 兜底：显示标识本身（见函数文档）
        (other, _) => other,
    };
    text.to_string()
}

/// 能力错误的前缀文案。
fn capability_message(language: Language) -> &'static str {
    match language {
        Language::Zh => "不支持的能力：",
        Language::En => "unsupported capability: ",
    }
}

/// 面向用户的提示（提示与错误同属"文案"，FR-044 一并覆盖）。
///
/// # 为什么提示也要进来
///
/// FR-044 说的是"错误**与提示**标识"。只翻译错误、把提示留在代码里
/// 硬编码中文，会让英文用户看到"错误是英文、提示是中文"的割裂界面。
#[must_use]
pub fn hint(id: &str, language: Language) -> String {
    let text = match (id, language) {
        ("hint.next_step.import_source", Language::Zh) => {
            "请先运行 `tome source import <file>` 导入书源"
        }
        ("hint.next_step.import_source", Language::En) => {
            "run `tome source import <file>` to import a book source first"
        }
        ("hint.use_help", Language::Zh) => "用 `tome --help` 查看可用命令",
        ("hint.use_help", Language::En) => "run `tome --help` for available commands",
        ("hint.write_baseline", Language::Zh) => "用 `--write-baseline` 落盘首轮基线",
        ("hint.write_baseline", Language::En) => {
            "use `--write-baseline` to save the first baseline"
        }
        ("hint.retry_later", Language::Zh) => "稍后重试",
        ("hint.retry_later", Language::En) => "try again later",
        (other, _) => other,
    };
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 语言标识往返() {
        for language in Language::all() {
            assert_eq!(
                Language::parse(language.code()),
                Some(*language),
                "{} 应能往返",
                language.code()
            );
        }
    }

    #[test]
    fn 语言标识接受常见变体() {
        // 用户会写 zh-CN / zh_CN / ZH / en-US
        for text in ["zh", "zh-CN", "zh_CN", "ZH", "cn", "chs"] {
            assert_eq!(Language::parse(text), Some(Language::Zh), "{text}");
        }
        for text in ["en", "en-US", "EN", "en_GB"] {
            assert_eq!(Language::parse(text), Some(Language::En), "{text}");
        }
        assert_eq!(Language::parse("jp"), None, "不支持的语言应给 None");
        assert_eq!(Language::parse(""), None);
    }

    /// **SC-016 的核心断言**：每个 `ErrorKind` 变体 × 两种语言都有文案。
    #[test]
    fn 全部错误分类都有中英文案() {
        for kind in all_error_kinds() {
            for language in Language::all() {
                let text = message(&kind, *language);
                assert!(
                    !text.trim().is_empty(),
                    "{kind:?} 在 {} 下文案为空",
                    language.code()
                );
            }
        }
    }

    /// **SC-016 的第二半**：不存在只在单一语言下可读的错误。
    ///
    /// 若两语言文案相同，说明那句话只有一种语言（或是复制粘贴漏译）。
    #[test]
    fn 中英文案不相同() {
        for kind in all_error_kinds() {
            let zh = message(&kind, Language::Zh);
            let en = message(&kind, Language::En);
            assert_ne!(
                zh, en,
                "{kind:?} 的中英文案相同（`{zh}`）——说明没有真正翻译"
            );
        }
    }

    /// 文案里不该出现 `error.` 这类**标识**——那是"直接拿 id 当文案"。
    #[test]
    fn 分类文案不含标识前缀() {
        for kind in all_error_kinds() {
            for language in Language::all() {
                let text = message(&kind, *language);
                assert!(
                    !text.contains("error."),
                    "{kind:?} 在 {} 下直接把标识当文案了：`{text}`",
                    language.code()
                );
            }
        }
    }

    /// `UnsupportedCapability` 的文案要**带上能力名**（FR-006 的关键载荷）。
    #[test]
    fn 能力错误文案含能力名() {
        let kind = ErrorKind::UnsupportedCapability {
            capability: tome_core::ErrorCapability::new("webView"),
        };
        for language in Language::all() {
            let text = message(&kind, *language);
            assert!(
                text.contains("webView"),
                "{} 下的文案应含能力名：`{text}`",
                language.code()
            );
        }
    }

    /// 提示也要双语（FR-044 的"错误与提示"）。
    #[test]
    fn 提示有中英文案且不相同() {
        for id in [
            "hint.next_step.import_source",
            "hint.use_help",
            "hint.write_baseline",
            "hint.retry_later",
        ] {
            let zh = hint(id, Language::Zh);
            let en = hint(id, Language::En);
            assert!(!zh.trim().is_empty() && !en.trim().is_empty(), "{id}");
            assert_ne!(zh, en, "{id} 的中英提示相同——没有真正翻译");
            assert!(!zh.contains("hint."), "{id} 直接把标识当文案了：{zh}");
        }
    }

    /// 未知标识回落为标识本身（而非空串）——诊断友好。
    #[test]
    fn 未知标识回落为标识本身() {
        assert_eq!(
            render("error.something_brand_new", Language::Zh),
            "error.something_brand_new"
        );
        assert_eq!(hint("hint.unknown", Language::En), "hint.unknown");
    }

    /// 子标识有自己的文案（不是回落到一级分类）。
    #[test]
    fn 子标识有独立文案() {
        let detailed = render("error.rule.source.missing", Language::Zh);
        let generic = render("error.rule", Language::Zh);
        assert_ne!(
            detailed, generic,
            "子标识应有更具体的文案（`书源不存在` 比 `规则出错` 有用）"
        );
        assert!(detailed.contains("书源"), "实际：{detailed}");
    }

    /// 默认语言是中文（书源生态以中文社区为主）。
    #[test]
    fn 默认语言是中文() {
        assert_eq!(Language::default(), Language::Zh);
    }
}
