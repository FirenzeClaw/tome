//! `[WIP]` 外壳文案表（S3 T031 的 Phase 3 子集）
//!
//! ## 为什么文案必须收在一处
//!
//! 引擎层的红线是**零自然语言**（001 FR-044 / 002 FR-058）：错误只产
//! `error.xxx` 标识，文案留给上层。这条红线的对偶是：外壳是**唯一**的文案
//! 来源，界面上的每一个字都得从这里取。
//!
//! 收在一处有三个具体收益，都不是"整洁"这种软理由：
//!
//! 1. **可审计**——"界面上一共说了几句话"变成一个可数的事实，而不是散在
//!    几十个 `ui.label(...)` 里（`contracts/ui.md` §3.1 的守卫靠它成立：
//!    `views/` 与 `widgets/` 下 grep 不到中文字面串）。
//! 2. **标识永不翻译**——错误以 `error.storage` 抵达，文案表做 `id → 文案`
//!    映射，而**标识本身随文案一并呈现**（可捕获、可检索）。若文案在组件里
//!    拼，标识就很容易被"顺手美化掉"。
//! 3. **数字格式只定义一次**——进度百分比、册数、页码的呈现形态是**产品
//!    决策**而非各处实现的巧合。散着写就必然漂移（有的带 `%` 有的不带、
//!    有的四舍五入有的截断）。
//!
//! ## 为什么是"键 → 文案"而不是一堆 `pub const` 直接给文案
//!
//! 键是**稳定的标识**，文案是可替换的。多语言框架不在本层范围（003 假设节），
//! 但把键与文案分开让"以后加一种语言"变成加一张表，而不是翻遍组件。
//! 取不到键时返回**键本身**——那是可见的失败（界面上出现
//! `shelf.empty.title`），而不是静默显示空白。
//!
//! ## 本模块的覆盖范围
//!
//! 当前只覆盖 Phase 3（US1：书架与本地书闭环）用到的键；T031 补齐其余
//! （设置/搜索/书源/键盘表）并加零字面值守卫。**先建表再写视图**是刻意的
//! 顺序：否则视图会先长出字面串，回头再"抽出去"就得逐条改。

use tome_core::ErrorKind;

/// 文案键（稳定标识；文案本体在本模块的键值表里）。
///
/// 常量与表分开列：`TABLE` 是穷举的键值对，常量是**调用点能用到的名字**。
/// 两者不一致时由 `tests/i18n.rs` 的表完整性断言抓出来。
pub mod key {
    // --- 书架 ---
    /// 空态标题
    pub const SHELF_EMPTY_TITLE: &str = "shelf.empty.title";
    /// 空态说明（下一步做什么）
    pub const SHELF_EMPTY_HINT: &str = "shelf.empty.hint";
    /// 空态主操作：导入本地书
    pub const SHELF_EMPTY_IMPORT_LOCAL: &str = "shelf.empty.import_local";
    /// 空态次操作：导入书源
    pub const SHELF_EMPTY_IMPORT_SOURCE: &str = "shelf.empty.import_source";

    /// 过滤框提示
    pub const SHELF_FILTER_HINT: &str = "shelf.filter.hint";
    /// 排序切换按钮（显示当前排序键）
    pub const SHELF_SORT_LABEL: &str = "shelf.sort.label";
    /// 排序键：最近阅读
    pub const SHELF_SORT_RECENT: &str = "shelf.sort.recent";
    /// 排序键：标题升序
    pub const SHELF_SORT_TITLE_ASC: &str = "shelf.sort.title_asc";
    /// 排序键：标题降序
    pub const SHELF_SORT_TITLE_DESC: &str = "shelf.sort.title_desc";

    /// 书架加载中
    pub const SHELF_LOADING: &str = "shelf.loading";
    /// 过滤无结果
    pub const SHELF_NO_MATCH: &str = "shelf.no_match";
    /// 作者缺失时的占位
    pub const SHELF_UNKNOWN_AUTHOR: &str = "shelf.unknown_author";
    /// 封面缺失时的占位
    pub const SHELF_COVER_PLACEHOLDER: &str = "shelf.cover_placeholder";

    /// 移除确认标题
    pub const SHELF_REMOVE_TITLE: &str = "shelf.remove.title";
    /// 移除确认正文
    pub const SHELF_REMOVE_BODY: &str = "shelf.remove.body";
    /// 移除确认按钮
    pub const SHELF_REMOVE_CONFIRM: &str = "shelf.remove.confirm";

    // --- 阅读 ---
    /// 章节加载中
    pub const READING_LOADING: &str = "reading.loading";
    /// 加载中可取消
    pub const READING_LOADING_CANCEL: &str = "reading.loading.cancel";
    /// 加载失败标题
    pub const READING_FAILED_TITLE: &str = "reading.failed.title";
    /// 重试
    pub const READING_RETRY: &str = "reading.retry";
    /// 换源
    pub const READING_SWITCH_SOURCE: &str = "reading.switch_source";
    /// 返回书架
    pub const READING_BACK: &str = "reading.back";
    /// 章标题（`{}` 为章序号）
    pub const READING_CHAPTER_LABEL: &str = "reading.chapter_label";
    /// 页码（`{}`/`{}` 为当前页/总页）
    pub const READING_PAGE_LABEL: &str = "reading.page_label";
    /// 滚动形态的位置（百分比）
    pub const READING_SCROLL_POSITION: &str = "reading.scroll_position";

    // --- 导入 ---
    /// 本地导入对话框标题
    pub const IMPORT_LOCAL_TITLE: &str = "import.local.title";
    /// 本地导入说明（含拖入提示）
    pub const IMPORT_LOCAL_HINT: &str = "import.local.hint";
    /// 选择文件按钮
    pub const IMPORT_LOCAL_CHOOSE: &str = "import.local.choose";
    /// 文件选择对话框的"电子书"过滤器名
    pub const IMPORT_LOCAL_FILTER_BOOKS: &str = "import.local.filter_books";
    /// 文件选择对话框的"全部文件"过滤器名
    pub const IMPORT_LOCAL_FILTER_ALL: &str = "import.local.filter_all";
    /// 导入中
    pub const IMPORT_LOCAL_BUSY: &str = "import.local.busy";
    /// 书源导入对话框标题
    pub const IMPORT_SOURCE_TITLE: &str = "import.source.title";
    /// 书源导入占位说明（T024 落地）
    pub const IMPORT_SOURCE_HINT: &str = "import.source.hint";

    // --- 通用 ---
    /// 关闭
    pub const COMMON_CLOSE: &str = "common.close";
    /// 取消
    pub const COMMON_CANCEL: &str = "common.cancel";
    /// 设置叠加层标题
    pub const SETTINGS_TITLE: &str = "settings.title";
    /// 设置项占位说明（T030 落地）
    pub const SETTINGS_HINT: &str = "settings.hint";
    /// 主题
    pub const SETTINGS_THEME: &str = "settings.theme";
    /// 默认字号
    pub const SETTINGS_DEFAULT_FONT_SIZE: &str = "settings.default_font_size";
    /// 未知错误（兜底）
    pub const ERROR_UNKNOWN: &str = "error.unknown";
}

/// 键 → 文案（zh 主）。
const TABLE: &[(&str, &str)] = &[
    (key::SHELF_EMPTY_TITLE, "书架是空的"),
    (
        key::SHELF_EMPTY_HINT,
        "把一本 TXT / EPUB 拖进窗口，或用下面的按钮选择文件。",
    ),
    (key::SHELF_EMPTY_IMPORT_LOCAL, "导入本地书"),
    (key::SHELF_EMPTY_IMPORT_SOURCE, "导入书源"),
    (key::SHELF_FILTER_HINT, "按标题过滤"),
    (key::SHELF_SORT_LABEL, "排序"),
    (key::SHELF_SORT_RECENT, "最近阅读"),
    (key::SHELF_SORT_TITLE_ASC, "标题 A→Z"),
    (key::SHELF_SORT_TITLE_DESC, "标题 Z→A"),
    (key::SHELF_LOADING, "正在读取书架…"),
    (key::SHELF_NO_MATCH, "没有匹配的书"),
    (key::SHELF_UNKNOWN_AUTHOR, "佚名"),
    (key::SHELF_COVER_PLACEHOLDER, "无封面"),
    (key::SHELF_REMOVE_TITLE, "移除这本书？"),
    (
        key::SHELF_REMOVE_BODY,
        "将从书架移除，并清理它的缓存内容。阅读进度与书签一并删除，此操作不可撤销。",
    ),
    (key::SHELF_REMOVE_CONFIRM, "移除"),
    (key::READING_LOADING, "正在排版这一章…"),
    (key::READING_LOADING_CANCEL, "取消并返回书架"),
    (key::READING_FAILED_TITLE, "这一章打不开"),
    (key::READING_RETRY, "重试"),
    (key::READING_SWITCH_SOURCE, "换源"),
    (key::READING_BACK, "返回书架"),
    (key::READING_CHAPTER_LABEL, "第 {} 章"),
    (key::READING_PAGE_LABEL, "{} / {}"),
    (key::READING_SCROLL_POSITION, "{}%"),
    (key::IMPORT_LOCAL_TITLE, "导入本地书"),
    (
        key::IMPORT_LOCAL_HINT,
        "支持 TXT / EPUB / MOBI。也可以直接把文件拖进窗口。",
    ),
    (key::IMPORT_LOCAL_CHOOSE, "选择文件…"),
    (key::IMPORT_LOCAL_FILTER_BOOKS, "电子书"),
    (key::IMPORT_LOCAL_FILTER_ALL, "全部文件"),
    (key::IMPORT_LOCAL_BUSY, "正在导入…"),
    (key::IMPORT_SOURCE_TITLE, "导入书源"),
    (key::IMPORT_SOURCE_HINT, "（书源导入在后续切片落地）"),
    (key::COMMON_CLOSE, "关闭"),
    (key::COMMON_CANCEL, "取消"),
    (key::SETTINGS_TITLE, "设置"),
    (key::SETTINGS_HINT, "（四组全局默认在后续切片落地）"),
    (key::SETTINGS_THEME, "主题"),
    (key::SETTINGS_DEFAULT_FONT_SIZE, "默认字号"),
    (key::ERROR_UNKNOWN, "发生了未分类的错误"),
];

/// 错误标识 → 文案（`error.*` 标识由引擎产出）。
///
/// ## 为什么这里必须覆盖**全部** `ErrorKind`
///
/// 漏一个的后果不是"少一句话"：`ErrorKind` 是**闭集**（七类），漏掉的那类
/// 会走兜底分支显示"发生了未分类的错误"——用户看到的是一个无法据以行动的
/// 说法，而真实原因是已知且可说的。`tests/i18n.rs` 有穷举断言钉住这一点。
const ERROR_TABLE: &[(&str, &str)] = &[
    ("error.network", "网络不可用或请求失败"),
    ("error.proxy", "代理连接失败"),
    ("error.rule", "书源规则无法执行"),
    ("error.unsupported_capability", "这项能力当前不可用"),
    ("error.content", "内容无法解析"),
    ("error.storage", "数据读写失败"),
    ("error.configuration", "配置有问题"),
    // reader 侧（002）的配置错误：反序列化即校验，故失败必带字段名
    ("error.reader.read_config_parse", "阅读参数无法解析"),
    (
        "error.reader.read_config_invalid_field",
        "阅读参数超出允许范围",
    ),
    ("error.reader.read_config_serialize", "阅读参数无法保存"),
];

/// 取文案。键不存在时返回**键本身**（可见的失败，不静默给空白）。
#[must_use]
pub fn t(key: &str) -> &str {
    TABLE
        .iter()
        .find(|(candidate, _)| *candidate == key)
        .map_or(key, |(_, text)| text)
}

/// 错误标识的文案（标识本身不在此处翻译）。
#[must_use]
pub fn error_text(id: &str) -> &str {
    ERROR_TABLE
        .iter()
        .find(|(candidate, _)| *candidate == id)
        .map_or(t(key::ERROR_UNKNOWN), |(_, text)| text)
}

/// 错误的**完整呈现行**：文案 + 标识。
///
/// ## 为什么标识必须跟着出现
///
/// `contracts/ui.md` §3.2：标识**永不翻译**且随文案一并呈现。理由是它同时是
/// 三件事的键：文案表的键、日志里的检索词、用户向开发者描述问题时的凭据。
/// 只显示"数据读写失败"的话，用户与开发者之间就没有可对齐的对象了。
#[must_use]
pub fn error_line(id: &str) -> String {
    format!("{} [{}]", error_text(id), id)
}

/// 能力错误的徽标形态（对齐 002 §2.3 的 `[capability: xxx]`）。
#[must_use]
pub fn capability_badge(capability: &str) -> String {
    format!("[capability: {capability}]")
}

/// 进度百分比（**唯一**的百分比呈现口径）。
///
/// 钳制到 `[0, 100]`：进度来自存储层，而存储层的数据可能被外部工具改过
/// （001 假设节：CLI 与外壳共享数据目录）。渲染层不该因为一个越界数值
/// 显示出"1234%"。
#[must_use]
pub fn progress_percent(percent: f32) -> String {
    let clamped = if percent.is_nan() {
        0.0
    } else {
        percent.clamp(0.0, 100.0)
    };
    format!("{:.0}%", clamped)
}

/// 册数（书架计数）。
#[must_use]
pub fn book_count(count: usize) -> String {
    format!("共 {count} 册")
}

/// 书卡的第二行（作者 · 进度）。
///
/// ## 为什么这个拼接要在文案表里
///
/// `contracts/ui.md` §3.4：数字/进度/时间的**呈现格式**统一在文案表，不在
/// 组件内拼。这条的价值在这里很具体——`·` 两侧的空格、进度是否带 `%`、
/// 缺失作者时用什么占位，这三件事若散在组件里，早晚会出现"书架上的卡片用
/// 一种写法、搜索结果里用另一种"。
#[must_use]
pub fn card_meta(author: &str, progress_percent_value: f32) -> String {
    format!("{} · {}", author, progress_percent(progress_percent_value))
}

/// 章标题（`第 N 章`；内部 0 起口径 → 显示 1 起）。
#[must_use]
pub fn chapter_label(chapter_index: i32) -> String {
    format!("第 {} 章", chapter_index + 1)
}

/// 页码（`当前 / 总数`）。
#[must_use]
pub fn page_label(page: u32, total: u32) -> String {
    format!("{} / {}", page, total)
}

/// 滚动形态的位置百分比。
#[must_use]
pub fn scroll_position(percent: f32) -> String {
    progress_percent(percent)
}

/// 文案表里全部键（供完整性断言与 T031 的守卫使用）。
#[must_use]
pub fn all_keys() -> Vec<&'static str> {
    TABLE.iter().map(|(key, _)| *key).collect()
}

/// 全部 `ErrorKind` 是否都有文案（供 `tests/i18n.rs` 的穷举断言）。
#[must_use]
pub fn error_kinds_covered() -> bool {
    all_error_kinds()
        .iter()
        .all(|kind| ERROR_TABLE.iter().any(|(id, _)| *id == kind.message_id()))
}

/// 全部 `ErrorKind` 变体（`ErrorKind` 无 `ALL` 常量，故此处穷举）。
///
/// ## 为什么手工穷举而不是遍历
///
/// `ErrorKind` 是**带载荷的枚举**（`UnsupportedCapability { capability }`），
/// 无法用 `strum` 之类派生穷举。手工列出意味着"新增一类错误"时这里会漏——
/// 而那正是 `error_kinds_covered()` 想抓的。补偿手段是：`tests/i18n.rs` 用
/// 一个**独立的**穷举清单（`match` 表达式，编译器会在新增变体时报错）交叉
/// 校验本函数。两处不一致时测试红。
#[must_use]
pub fn all_error_kinds() -> Vec<ErrorKind> {
    vec![
        ErrorKind::Network,
        ErrorKind::Proxy,
        ErrorKind::Rule,
        ErrorKind::unsupported_capability(tome_core::ErrorCapability::new("testCapability")),
        ErrorKind::Content,
        ErrorKind::Storage,
        ErrorKind::Configuration,
    ]
}
