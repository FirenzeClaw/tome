//! 错误分类 —— 七类粗粒度分类 + 可本地化标识
//!
//! 对应 tasks.md T010（FR-006 / FR-044 / SC-016）。
//!
//! # 为什么只有七类，且**刻意不做上帝枚举**
//!
//! 各实现 crate 自带 thiserror 错误类型（`tome-net` 的 `NetError`、
//! `tome-store` 的 `StoreError`...），在接缝处映射为本模块的 [`ErrorKind`]。
//! core 只承载**跨 crate 的粗粒度分类**，理由：
//!
//! - **变更隔离**：若把"HTTP 412 的具体处理""SQLite 的某个 error code"也塞进来，
//!   任何一个实现 crate 的错误演进都会要求 core 改版本，波及全体（违反宪法 IV）。
//! - **上层只需粗粒度**：界面要做的是"网络错→提示检查网络""规则错→提示检查书源"，
//!   它需要的是分类，不是细节。细节放 [`ErrorKind::detail`]。
//!
//! # 为什么产出的是"标识"而不是文案
//!
//! FR-044 / SC-016 要求引擎产出可本地化标识，文案映射留给上层。
//! 这带来两个实际好处：
//!
//! 1. **引擎内零自然语言**：新增语言不需要改引擎（AGENTS.md 生产级护栏明令）。
//! 2. **日志可检索**：`error.storage` 是稳定 grep 目标，而中文文案会随措辞改动。

use std::fmt;

/// 可本地化消息标识。
///
/// 约定格式为点分小写 ASCII 路径（如 `error.network.timeout`），由**上层**
/// 查文案表映射为具体语言。引擎内不得出现面向用户的自然语言（FR-044）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocalizedMessage {
    id: String,
}

impl LocalizedMessage {
    /// 以稳定标识构造。
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }

    /// 取消息标识，供上层查文案表。
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl fmt::Display for LocalizedMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.id)
    }
}

/// 系统不支持的能力的标识（FR-006）。
///
/// 书源里对"不支持的能力"做 `try/catch` 降级时，靠的是这个标识来判断
/// **是哪一项**能力不可用（如 `webView` 不可用则改走纯 HTTP 规则）。
/// 故它是结构化错误的核心载荷，不能省成一句文案。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ErrorCapability {
    capability_id: String,
}

impl ErrorCapability {
    /// 以能力标识构造。标识用书源生态里的通用叫法（如 `webView`）。
    pub fn new(capability_id: impl Into<String>) -> Self {
        Self {
            capability_id: capability_id.into(),
        }
    }

    /// 取能力标识，供书源的降级逻辑判断。
    pub fn capability_id(&self) -> &str {
        &self.capability_id
    }
}

impl fmt::Display for ErrorCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.capability_id)
    }
}

/// 引擎错误的**分类**（七类），对齐 AGENTS.md 与 spec 的定义。
///
/// 各实现 crate 的错误类型在接缝处映射到本枚举；上层据此做粗粒度处理
/// 与文案映射，不依赖任何实现 crate 的错误细节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// 网络：连接失败、超时、DNS、重定向、非成功状态码等**传输与协议层**问题。
    ///
    /// 注意**不含**代理故障——那是 [`Self::Proxy`] 的关切（FR-008 边缘情况
    /// 明确要求两者可区分）。
    Network,
    /// 代理：代理不可达、代理握手失败、代理认证失败等。
    ///
    /// ## 为什么代理故障不能并入 Network
    ///
    /// 上层要据此给**完全不同的**处置建议：`Network` 时用户该去看目标站是否
    /// 可达、网络是否通；`Proxy` 时用户该去看本机代理设置是否正确——可能是
    /// 代理没开、地址填错、或某个源在代理下反而不可达（那需要按源设
    /// `ProxyOverride::Direct`）。两者混为一类会让提示指向错误的方向。
    ///
    /// FR-008 的边缘情况把这条区分写成了硬要求。
    Proxy,
    /// 规则：书源不存在、规则语法错误、选择器无匹配、脚本抛错等。
    Rule,
    /// 不支持的能力：系统明确做不到的能力（FR-006）。
    ///
    /// **必须**携带能力标识——这是与其馀**无载荷的**分类之间的结构性区别：
    /// 它不是一个"出错"信号，而是一个"此路不通，请改道"的可编程信号。
    UnsupportedCapability {
        /// 不可用能力的标识
        capability: ErrorCapability,
    },
    /// 内容：编码无法解码、正文为空、文件格式损坏、内容超出预期范围等。
    Content,
    /// 存储：数据库错误、磁盘不可写、空间不足、缓存容器损坏等。
    Storage,
    /// 配置：配置项缺失或非法、路径无效、语言包不存在等。
    Configuration,
}

/// 附带诊断细节的分类错误。
///
/// ## 为什么细节与分类分离
///
/// 分类是**契约**（上层按它分支），细节是**诊断**（给开发者/日志）。
/// 合在一起会让上层不得不关心细节，也让细节的措辞变化变成破坏性变更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedError {
    kind: ErrorKind,
    detail: Option<String>,
}

impl ErrorKind {
    /// 取本分类对应的可本地化消息标识。
    pub fn message(&self) -> LocalizedMessage {
        match self {
            Self::Network => LocalizedMessage::new("error.network"),
            Self::Proxy => LocalizedMessage::new("error.proxy"),
            Self::Rule => LocalizedMessage::new("error.rule"),
            Self::UnsupportedCapability { .. } => {
                LocalizedMessage::new("error.unsupported_capability")
            }
            Self::Content => LocalizedMessage::new("error.content"),
            Self::Storage => LocalizedMessage::new("error.storage"),
            Self::Configuration => LocalizedMessage::new("error.configuration"),
        }
    }

    /// 构造"不支持某能力"的错误（FR-006）。
    ///
    /// 用关联函数而非让调用方手写枚举字面量：能力错误的构造点很多
    /// （登录/音频/图片/文件访问等），集中一处便于日后统一补字段。
    pub fn unsupported_capability(capability: ErrorCapability) -> Self {
        Self::UnsupportedCapability { capability }
    }

    /// 附加诊断细节，返回可传播的错误值。
    ///
    /// `detail` 是给开发者/日志的诊断信息，**不是**面向用户的文案
    /// （后者由上层按 [`Self::message`] 的标识映射）。
    pub fn with_detail(self, detail: impl Into<String>) -> ClassifiedError {
        ClassifiedError {
            kind: self,
            detail: Some(detail.into()),
        }
    }

    /// 本分类的可本地化标识（等价于 `self.message().id()` 的便捷写法）。
    pub fn message_id(&self) -> &'static str {
        match self {
            Self::Network => "error.network",
            Self::Proxy => "error.proxy",
            Self::Rule => "error.rule",
            Self::UnsupportedCapability { .. } => "error.unsupported_capability",
            Self::Content => "error.content",
            Self::Storage => "error.storage",
            Self::Configuration => "error.configuration",
        }
    }
}

impl ClassifiedError {
    /// 本错误的分类（上层按它分支）。
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// 诊断细节（无则 `None`）。
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// 本错误的可本地化标识。
    pub fn message_id(&self) -> &'static str {
        self.kind.message_id()
    }
}

impl fmt::Display for ClassifiedError {
    /// 输出形如 `error.storage: pack 索引损坏`。
    ///
    /// 标识在前、细节在后：日志按标识检索，细节供人阅读。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(formatter, "{}: {detail}", self.kind.message_id()),
            None => formatter.write_str(self.kind.message_id()),
        }
    }
}

impl std::error::Error for ClassifiedError {}

impl fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedCapability { capability } => {
                write!(formatter, "{}: {capability}", self.message_id())
            }
            _ => formatter.write_str(self.message_id()),
        }
    }
}

impl std::error::Error for ErrorKind {}
