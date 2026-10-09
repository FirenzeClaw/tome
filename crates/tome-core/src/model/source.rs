//! 书源 DTO（data-model.md 的 `source` / `source_group` 表）

/// 书源类型（data-model.md 的 `source.kind`：`0=书源 1=JS 源 2=订阅源 3=词典源`）。
///
/// 四类源共用一张表：它们的元信息（名称/分组/启用/请求定制）高度重合，
/// 差异集中在规则字段的有无，拆表反而制造 join。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// 普通网页规则源
    Book,
    /// 纯脚本源（主脚本在 `Source::js`）
    Js,
    /// 订阅源（RSS 类）
    Subscribe,
    /// 词典源
    Dictionary,
}

impl SourceKind {
    /// 转为存储层整数编码。
    pub fn as_storage_code(self) -> i32 {
        match self {
            Self::Book => 0,
            Self::Js => 1,
            Self::Subscribe => 2,
            Self::Dictionary => 3,
        }
    }

    /// 从存储层整数编码还原；未知编码返回 `None`。
    pub fn from_storage_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Book),
            1 => Some(Self::Js),
            2 => Some(Self::Subscribe),
            3 => Some(Self::Dictionary),
            _ => None,
        }
    }
}

/// 书源分组（data-model.md 的 `source_group` 表）。用于组织与批量启停。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceGroup {
    /// 主键
    pub id: i64,
    /// 分组名
    pub name: String,
    /// 排序权重
    pub sort_order: i32,
    /// 组内书源是否启用（批量启停）
    pub enabled: bool,
}

/// 书源定义（data-model.md 的 `source` 表）—— "皇冠上的明珠"。
///
/// ## 为什么规则字段是 `String` 而非结构化类型
///
/// data-model.md 明确"规则字段整体存 JSON 列（规则结构会演进，不值得拆表）"。
/// DTO 沿用这一决策：规则的**解析**属于 tome-rule，本 crate 只承载原始文本。
/// 若在此处引入结构化规则类型，core 就会被规则 DSL 的演进拖着重编译，
/// 违背"core 变更不应波及全员"。
///
/// ## 五类动作规则
///
/// `rule_search` / `rule_explore` / `rule_book_info` / `rule_toc` / `rule_content`
/// 对应 FR-002 的五类抓取动作。它们的键名由书源格式定义，内容由 tome-rule 解释。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// 主键：书源 URL 或 JS 源标识（**唯一**，导入重复 id 按"更新"处理）
    pub id: String,
    /// 书源名
    pub name: String,
    /// 类型
    pub kind: SourceKind,
    /// 所属分组
    pub group_id: Option<i64>,
    /// 是否启用
    pub enabled: bool,
    /// 书籍类型：文本/音频/图片（本期音频/图片源经能力错误降级）
    pub book_source_type: Option<i32>,
    /// 书籍详情页 URL 匹配模式（用于识别"这个链接属于哪个源"）
    pub book_url_pattern: Option<String>,
    /// 搜索规则（JSON）
    pub rule_search: Option<String>,
    /// 发现规则（JSON）
    pub rule_explore: Option<String>,
    /// 书籍详情规则（JSON）
    pub rule_book_info: Option<String>,
    /// 目录规则（JSON）
    pub rule_toc: Option<String>,
    /// 正文规则（JSON）
    pub rule_content: Option<String>,
    /// 自定义请求头（JSON，FR-008）
    pub header: Option<String>,
    /// 登录地址（WebView 类能力 → 降级为能力错误，FR-006）
    pub login_url: Option<String>,
    /// 登录界面定义
    pub login_ui: Option<String>,
    /// 登录态校验脚本
    pub login_check_js: Option<String>,
    /// 纯脚本源的主脚本
    pub js: Option<String>,
    /// 声明的字符集（FR-009：声明与实际不符时须容错）
    pub charset: Option<String>,
    /// 请求间隔，毫秒（FR-008：限速依据）
    pub rate_limit_ms: Option<i64>,
    /// 排序权重
    pub weight: Option<i32>,
    /// 发现页配置（JSON）
    pub explore_screen: Option<String>,
    /// 备注
    pub comment: Option<String>,
    /// 最后更新时间戳
    pub last_update_time: Option<i64>,
    /// 创建时间戳
    pub created_at: i64,
}
