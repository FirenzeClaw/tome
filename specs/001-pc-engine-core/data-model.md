# Data Model: S0+S1 引擎内核

**特性**: 001-pc-engine-core | **来源**: spec.md §Key Entities | **日期**: 2026-10-09

存储引擎：SQLite（结构化数据）+ 文件系统（Pack 章节缓存 / 图片缓存 / 归一化本地书 / 配置 TOML）。Schema 从零设计（不兼容上游 legado.db），首个版本为 v1，迁移器用 `PRAGMA user_version` + 内嵌 SQL 脚本数组。

> 字段一律以"与排版无关"为准——阅读痕迹存字符区间/锚点，不存渲染坐标（宪法 §生产级护栏 / FR-028）。

## 实体总览

| 实体 | 表 | 说明 |
|---|---|---|
| 书源 | `source` | 统一表，`kind` 区分书源/订阅源(JS 源)/词典源 |
| 书源分组 | `source_group` | 书源归类，批量启停 |
| 书籍 | `book` | 书架条目，`origin` 区分网络/本地 |
| 章节 | `chapter` | 章条目 |
| 章节缓存容器 | `chapter_cache` | Pack 索引（pack_id + offset + len） |
| 阅读进度 | `read_progress` | 锚点（chapter_index + char_offset） |
| 阅读记录 | `read_record` | 阅读时长/字数（FR-026b） |
| 书签 | `bookmark` | 位置 + 备注 |
| 高亮 | `highlight` | 字符区间 + 样式 + 备注 |
| 批注 | `memo` | 文本区间 + 笔记 |
| 替换/净化规则 | `replace_rule` | 匹配 + 替换 + 生效范围 |
| 高亮规则 | `highlight_rule` | 自动高亮规则 |
| 词典规则 | `dict_rule` | 查词来源定义 |
| 目录识别规则 | `txt_toc_rule` | TXT 目录识别（正则/JS） |
| 搜索关键词 | `search_keyword` | 收藏检索词 |
| 请求日志 | `http_log` | 请求/响应记录，滚动清理 |
| Cookie | `cookie` | 按源落盘的登录态（FR-026，Q6） |
| 库根目录 | `library_root` | 本地书库根 |

## 关键实体详述

### 书源（Source）— 皇冠上的明珠

统一存储三类源。规则字段整体存 JSON 列（规则结构会演进，不值得拆表）。

```
source
  id            TEXT PK          -- 书源 URL 或 JS 源标识（唯一）
  name          TEXT NOT NULL
  kind          INTEGER NOT NULL -- 0=书源 1=JS 源 2=订阅源 3=词典源
  group_id      INTEGER FK → source_group
  enabled       INTEGER NOT NULL DEFAULT 1
  book_source_type  INTEGER      -- 文本/音频/图片(预留，本期音频/图片源经能力错误降级)
  book_url_pattern  TEXT         -- 书籍详情页 URL 匹配（用于识别"这个链接属于哪个源"）
  -- 五类动作的规则，整体 JSON
  rule_search    TEXT   -- 搜索规则（JSON：列表/书名/作者/封面/简介/详情URL 等字段）
  rule_explore   TEXT   -- 发现规则
  rule_book_info TEXT   -- 书籍详情规则
  rule_toc       TEXT   -- 目录规则
  rule_content   TEXT   -- 正文规则
  -- 请求定制
  header        TEXT    -- 自定义请求头 JSON
  login_url     TEXT    -- 登录地址（WebView 类能力 → 降级为能力错误）
  login_ui      TEXT
  login_check_js TEXT
  -- 脚本
  js            TEXT    -- 纯脚本源的主脚本（mainJs）
  -- 行为参数
  charset       TEXT
  rate_limit_ms INTEGER  -- 请求间隔
  weight        INTEGER  -- 排序权重
  explore_screen TEXT    -- 发现页配置
  -- 元信息
  comment       TEXT
  last_update_time INTEGER
  created_at    INTEGER NOT NULL
```

**唯一性**：`id` 唯一。导入重复书源（同 id）按"更新"处理（spec 边缘情况）。

### 书籍（Book）

```
book
  id            INTEGER PK
  origin        INTEGER NOT NULL  -- 0=网络 1=本地
  origin_id     TEXT NOT NULL     -- 网络: 书源 id + 书籍详情 URL；本地: 归一化文件哈希
  name          TEXT NOT NULL
  author        TEXT
  cover_url     TEXT
  intro         TEXT
  kind          TEXT              -- 分类标签
  book_group    TEXT              -- 书架分组
  -- 阅读偏好（该书生效的排版参数，覆盖全局默认；存 JSON，S2 使用）
  read_config   TEXT
  -- 网络书
  toc_url       TEXT              -- 目录页地址
  latest_chapter_title TEXT
  latest_chapter_time  INTEGER
  -- 本地书
  library_root_id INTEGER FK → library_root
  relative_path TEXT              -- 本地书: 相对路径
  charset       TEXT              -- 本地书: 识别出的编码
  format        TEXT              -- txt / epub / mobi / azw3
  total_chapters INTEGER
  dur_chapter_index INTEGER       -- 当前读到第几章（冗余便于查询）
  added_at      INTEGER NOT NULL
  UNIQUE(origin, origin_id)
```

**身份规则**：网络书以 `(书源id, 书籍详情URL)` 唯一；本地书以归一化文件内容哈希唯一（移动文件不产生重复书目——spec 边缘情况）。

### 章节（Chapter）

```
chapter
  id            INTEGER PK
  book_id       INTEGER NOT NULL FK → book
  index_in_book INTEGER NOT NULL  -- 书内序号
  title         TEXT NOT NULL
  -- 网络书: 章节内容地址；本地书: 字节偏移定位
  url           TEXT              -- 网络
  byte_offset   INTEGER           -- 本地 TXT
  epub_entry    TEXT              -- 本地 EPUB: zip 目录项名
  is_cached     INTEGER NOT NULL DEFAULT 0
  is_read       INTEGER NOT NULL DEFAULT 0
  word_count    INTEGER
  UNIQUE(book_id, index_in_book)
```

### 章节缓存容器（Chapter Cache Pack 索引）

```
chapter_cache
  chapter_id    INTEGER NOT NULL FK → chapter
  book_id       INTEGER NOT NULL FK → book
  pack_id       INTEGER NOT NULL  -- 所属容器（写入顺序分配）
  offset        INTEGER NOT NULL  -- 容器内偏移
  length        INTEGER NOT NULL
  cached_at     INTEGER NOT NULL  -- 写入时间（淘汰按此 LRU）
  PRIMARY KEY(chapter_id)
```

**Pack 文件**：`cache/packs/<book_hash>/<pack_id>.pk`，头部 `[magic 8B][version u16][pack_id u32]`，正文为顺序追加的 `[u32 len][bytes]`。下载区 `download/packs/...`（pinned，不参与淘汰）。

**崩溃安全**：先 `文件追加 + fsync`，后 SQLite 事务提交索引行（FR-030）。

### 阅读进度（Read Progress）— 锚点

```
read_progress
  book_id       INTEGER PK FK → book
  chapter_index INTEGER NOT NULL
  char_offset   INTEGER NOT NULL   -- 章内字符偏移（排版无关）
  anchor_text   TEXT               -- 锚点处文本片段（换源特征段匹配用，FR-010b）
  updated_at    INTEGER NOT NULL
```

**关键不变量**：锚点 = `(chapter_index, char_offset)` + `anchor_text`。字号/窗口变化后仍指向同一处文本（SC-008）。换源时用 `anchor_text` 在新源邻域缓存内做特征段匹配（FR-010b）。

### 阅读记录（Read Record，FR-026b）

```
read_record
  book_id       INTEGER NOT NULL FK → book
  date          TEXT NOT NULL      -- YYYY-MM-DD
  read_seconds  INTEGER NOT NULL DEFAULT 0
  read_chars    INTEGER NOT NULL DEFAULT 0
  PRIMARY KEY(book_id, date)
```

### 阅读痕迹（书签 / 高亮 / 批注）

三者统一用**字符区间**（chapter_index + char_offset + length），不存渲染坐标。

```
bookmark   (id, book_id, chapter_index, char_offset, note, created_at)
highlight  (id, book_id, chapter_index, char_offset, length, style, note, created_at)
memo       (id, book_id, chapter_index, char_offset, length, content, created_at)
```

### 规则（净化 / 高亮 / 词典 / 目录识别）

```
replace_rule  (id, name, pattern, replacement, scope, is_regex, enabled, sort_order)
highlight_rule(id, name, pattern, style, enabled, sort_order)
dict_rule     (id, name, url_rule, content_rule, enabled)
txt_toc_rule  (id, name, rule, example, enabled, sort_order)  -- rule 为正则或 JS
```

### Cookie（FR-026 持久化）

```
cookie
  source_id     TEXT NOT NULL FK → source
  name          TEXT NOT NULL
  value         TEXT NOT NULL      -- 敏感：日志中脱敏
  domain        TEXT
  expires_at    INTEGER
  PRIMARY KEY(source_id, name, domain)
```

### 库根目录（Library Root）

```
library_root
  id            INTEGER PK
  path          TEXT NOT NULL UNIQUE  -- 绝对路径
  added_at      INTEGER NOT NULL
```

### 请求日志（HTTP Log）

```
http_log
  id            INTEGER PK
  source_id     TEXT
  url           TEXT
  method        TEXT
  request_headers  TEXT   -- 敏感头脱敏
  response_code INTEGER
  response_body TEXT       -- 可能截断
  duration_ms   INTEGER
  created_at    INTEGER
```
滚动清理（FR-017，限制条数）。

### 图片缓存（Image Cache，文件系统 + 索引）

文件存 `cache/images/<xx>/<hash>`；索引入 `image_cache(url PK, source_id, path, fetched_at)`。与章节缓存为两个可独立清理的资源（SC-017）。

## 配置（不入库）

`config.toml`（原子写）：网络（代理）、缓存上限（章节/图片）、界面语言、行为偏好、Pack 容量默认值。**禁止全局 CONFIG 单例**——组合根加载一次，类型化子集注入各模块（架构审查 A3）。

## Schema 治理

- 版本从 v1 起；迁移器 = `PRAGMA user_version` + 内嵌 SQL 脚本数组（~100 行，不引第三方框架）
- 写并发：数据库归**专属 actor 线程**所有，消息通道收发（rusqlite Connection 非 Sync；GUI/CLI 不阻塞）
- 索引：常用查询（chapter by book_id+index、chapter_cache by chapter_id、read_record by date）建索引；FTS5 用于书内全文搜索（FR-019b）与本地书库检索
