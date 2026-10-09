//! 书籍与章节 DTO（data-model.md 的 `book` / `chapter` / `library_root` 表）

/// 书籍来源类型（data-model.md 的 `book.origin`）。
///
/// 用枚举而非 `i32`：查询与匹配时无需记忆 `0=网络 1=本地` 的约定，
/// 且新增来源时编译器会指出所有需处理的分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BookOrigin {
    /// 网络书：内容来自书源抓取
    Network,
    /// 本地书：内容来自本地文件（TXT/EPUB/MOBI）
    Local,
}

impl BookOrigin {
    /// 转为存储层使用的整数编码（data-model.md 定义 `0=网络 1=本地`）。
    ///
    /// 显式提供转换而不派生 `From<u8>`：编码归属存储层的 schema，
    /// 放在这里是为了让"编码值"只有一处定义，避免各实现 crate 各写一份。
    pub fn as_storage_code(self) -> i32 {
        match self {
            Self::Network => 0,
            Self::Local => 1,
        }
    }

    /// 从存储层整数编码还原。未知编码返回 `None`（调用方须报数据损坏，
    /// 而不是猜一个默认值）。
    pub fn from_storage_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Network),
            1 => Some(Self::Local),
            _ => None,
        }
    }
}

/// 书架条目（data-model.md 的 `book` 表）。
///
/// ## 身份规则
///
/// 网络书以 `(书源 id, 书籍详情 URL)` 唯一；本地书以**归一化文件内容哈希**唯一
/// ——移动文件不产生重复书目（spec 边缘情况）。两者共同落在 `origin_id` 上。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Book {
    /// 主键。网络书新加入时为 `None`（由存储层分配）。
    pub id: i64,
    /// 来源类型
    pub origin: BookOrigin,
    /// 来源标识：网络书为 `书源id|书籍详情URL`；本地书为归一化内容哈希
    pub origin_id: String,
    /// 书名
    pub name: String,
    /// 作者
    pub author: Option<String>,
    /// 封面地址
    pub cover_url: Option<String>,
    /// 简介
    pub intro: Option<String>,
    /// 分类标签
    pub kind: Option<String>,
    /// 书架分组
    pub book_group: Option<String>,
    /// 该书生效的排版配置（覆盖全局默认；内容由 S2 排版层解释，此处不透明）
    pub read_config: Option<String>,
    /// 目录页地址（网络书）
    pub toc_url: Option<String>,
    /// 最新章节标题（网络书）
    pub latest_chapter_title: Option<String>,
    /// 最新章节时间戳（网络书）
    pub latest_chapter_time: Option<i64>,
    /// 所属库根目录（本地书）
    pub library_root_id: Option<i64>,
    /// 相对库根的路径（本地书）
    pub relative_path: Option<String>,
    /// 识别出的编码（本地书）
    pub charset: Option<String>,
    /// 文件格式：`txt` / `epub` / `mobi` / `azw3`（本地书）
    pub format: Option<String>,
    /// 总章节数
    pub total_chapters: Option<i32>,
    /// 当前读到第几章（冗余字段，便于列表查询而不必 join 进度表）
    pub dur_chapter_index: Option<i32>,
    /// 加入书架时间戳
    pub added_at: i64,
}

/// 章条目（data-model.md 的 `chapter` 表）。
///
/// ## 定位方式的二分
///
/// - **网络书**用 `url` 指向章节内容地址
/// - **本地书**用 `byte_offset`（TXT，字节偏移）或 `epub_entry`（EPUB，zip 目录项名）
///
/// 三者互斥但同表，故都是 `Option`。存储层的 `UNIQUE(book_id, index_in_book)`
/// 保证同一本书内序号唯一。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    /// 主键
    pub id: i64,
    /// 所属书籍
    pub book_id: i64,
    /// 书内序号（从 0 起）
    pub index_in_book: i32,
    /// 章节标题
    pub title: String,
    /// 章节内容地址（网络书）
    pub url: Option<String>,
    /// 文件内字节偏移（本地 TXT）
    pub byte_offset: Option<i64>,
    /// zip 目录项名（本地 EPUB）
    pub epub_entry: Option<String>,
    /// 内容是否已在缓存容器中（FR-029）
    pub is_cached: bool,
    /// 是否已读
    pub is_read: bool,
    /// 字数
    pub word_count: Option<i32>,
}

/// 本地书库根目录（data-model.md 的 `library_root` 表）。
///
/// FR-023：支持添加多个库目录，**只索引不移动或改写原文件**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryRoot {
    /// 主键
    pub id: i64,
    /// 绝对路径（存储层保证唯一）
    pub path: String,
    /// 添加时间戳
    pub added_at: i64,
}
