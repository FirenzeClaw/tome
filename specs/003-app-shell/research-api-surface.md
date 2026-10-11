# 研究：S3 应用外壳（tome-app）公共 API 入口测绘

**特性**：003-app-shell ｜ **日期**：2026-10-11 ｜ **测绘基准**：`git HEAD = 8084087`（工作树另有 002/tasks.md 的未提交改动与 003 的并行产出，不影响本文的行号）

**方法**：只读通读 `crates/*/src/**`、`crates/*/Cargo.toml`、根 `Cargo.toml`、`scripts/check-reader-features.sh`、`crates/tome-cli/tests/dependency_graph.rs` 与 `specs/001/contracts/seams.md`。行号均为 `HEAD` 下的真实行号，可直接跳转。未改动任何代码。

**同批次姊妹文档**：`specs/003-app-shell/research-eframe.md`（egui/eframe 栈选型与钉版）、`research-design-system.md`（设计系统消费）。本文**只测引擎/渲染层的可调用面**，不重复 GUI 选型；附录 A 的 eframe 版本取自 `research-eframe.md` 的结论。

**口径**：
- `feature` 列写 `—` 表示无门控（任何构建都可用）；写 `render` 表示仅在 `tome-reader/render` 开启时存在。
- 「语义」列只写外壳要用到的判定信息（不变量、返回形态、失败模式），不复述 doc 全文。

**三条贯穿全文的关键结论（先读）**

1. **`BookRepository` 与 `SourceEngine` 两个接缝 trait 至今没有生产实现**。前者只有 `tome-core` 测试里的 `FakeRepository`，后者只有 `tome-core` 测试里的 `FakeSourceEngine`（`grep -rn "impl BookRepository" crates/` 只命中 `crates/tome-core/tests/seam_contract.rs:635`）。真实能力在**具体 DAO**（`tome-store::dao::*`）与**自由函数**（`tome-source::{search,info,toc,content,change_source,check}`）上。外壳若只依赖接缝 trait，**拿不到书架与书源动作**——这是 S3 必须先解的前置缺口（见 §2、§4 的标注与文末「待解缺口」）。
2. **唯一有真实双 adapter 的接缝是 `ContentProvider`**（`tome_source::provider::NetworkContentProvider` / `tome_local::provider::LocalContentProvider`）。它是 S2 排版层与 S3 之间的**真正可用接缝**，也是外壳正文加载的唯一推荐入口。
3. **渲染出口只有离屏一条路**：`OffscreenRenderer::render(...) -> RenderedImage { width, height, pixels: Vec<u8> }`（RGBA8、行紧凑）。管线用 `InstanceDescriptor::new_without_display_handle()` 构造，**没有任何 surface / winit / 窗口集成路径**。嵌进 egui 的可行做法是把 `pixels` 转 `egui::ColorImage` 上传纹理；`wgpu` 设备与 egui 的设备**不共享**。

---

## 1. 组合根与接缝

### 1.1 tome-core 暴露给外壳的全部接缝面

`tome-core` 零依赖、零图形（`crates/tome-core/Cargo.toml` 无任何依赖项），可安全进入 tome-app 的依赖图。

**顶层接缝 trait（`tome_core::seam`）**

- `trait SourceEngine` — `crates/tome-core/src/seam/source.rs:25` — `—` — 书源五动作 + 换源 + 源检查。**无生产实现**（见结论 1）。方法：`search` `:30`、`discover` `:33`、`book_info` `:42`、`toc` `:45`、`change_source` `:48`、`check_source` `:55`、`check_sources` `:69`。注意 `:24` 有 `#[allow(async_fn_in_trait)]`，但 trait 内**没有任何 `async fn`**（残留属性，不构成异步接缝）。
- `trait ContentProvider` — `crates/tome-core/src/seam/source.rs:181` — `—` — **外壳正文加载的唯一接缝**。四方法：`toc(&self, &Book) -> Result<Vec<Chapter>, ErrorKind>` `:183`；`chapter_content(&self, &Book, chapter_index: i32) -> Result<ChapterContent, ErrorKind>` `:186`（净化管线已在实现内跑完）；`resolve_anchor(&self, &Book, &AnchorQuery) -> Result<(i32, i64), ErrorKind>` `:192`；`prefetch(&self, &Book, around_index: i32, depth: u32)` `:195`（返回 `()`，失败无副作用）。
- `trait BookRepository` — `crates/tome-core/src/seam/repository.rs:23` — `—` — 书架/章节/进度/痕迹/规则/书源/备份/缓存清理的**全量**读写面（43 个方法）。**无生产实现**。
- `trait SearchStream` — `crates/tome-core/src/seam/stream.rs:82` — `—` — 拉取式流：`next_item(&mut self) -> Option<SearchResultItem>` `:84`、`cancel_token(&self) -> &CancelToken` `:87`。对象安全（外壳存 `Box<dyn SearchStream>`）。
- `trait CheckStream` — `crates/tome-core/src/seam/source.rs:88` — `—` — 同形：`next_report(&mut self) -> Option<CheckReport>` `:90`、`cancel_token` `:93`。
- `struct CancelToken` — `crates/tome-core/src/seam/stream.rs:26` — `—` — `new()` `:32` / `cancel()` `:37`（幂等）/ `is_cancelled()` `:42`。`Arc<AtomicBool>` 内部，`Clone` 廉价，可跨线程。
- `enum SearchResultItem` — `crates/tome-core/src/seam/stream.rs:53` — `—` — `SourceResults { source_id, books }` / `SourceFailed { source_id, error: ErrorKind }`。**单源失败是流内条目，不是流终止条件**——外壳的搜索结果列表必须按变体分别渲染（失败源要有可见行）。

**宿主能力 trait 与共享值对象（`tome_core::seam::host`）**

- `trait NetHost: Send + Sync` — `crates/tome-core/src/seam/host.rs:282` — `—` — `fetch(&self, RequestSpec) -> Result<Response, ErrorKind>` `:284`。**全系统唯一发请求出口**（A2）。外壳不实现它，只把组合根造的实例透传给 `tome-source`/`tome-store`。
- `trait StoreHost: Send + Sync` — `:291` — `—` — `cache_get` `:293` / `cache_put` `:295` / `read_file` `:299` / `write_file` `:301`。**按源分区**（一个实例绑一个 `source_id`），路径越界须返回 `UnsupportedCapability`。
- `trait CryptoHost: Send + Sync` — `:313` — `—` — `digest` `:315` / `base64_encode` `:318` / `base64_decode` `:321`。纯计算，`tome-js` 内建，外壳**不需要**碰它。
- `struct RequestSpec` — `:187` — `—` — 方法/URL/头/体/charset/proxy_override/rate_limit_key/request_interval_ms/timeout_ms/retry 全字段。构造捷径 `RequestSpec::get(url, rate_limit_key)` `:222`；默认超时 `DEFAULT_TIMEOUT_MS = 30_000` `:245`。
- `struct Response` — `:252` — `—` — `status: u16` / `headers` / `body_bytes: Vec<u8>` / `detected_charset`。`is_success()` `:265`（2xx）。**非 2xx 也返回 `Ok`**——分类权在调用方。
- `struct HeaderMap` — `:87` — `—` — 保序、允重名：`append` `:98` / `set` `:104` / `get` `:111` / `iter` `:119`。
- `enum HttpMethod` — `:37` — `—` — Get/Post/Put/Delete/Head + `as_str` `:52` / `from_str_ignore_case` `:63`。
- `enum ProxyOverride` — `:141` — `—` — `Direct` / `Custom(String)`。`None` 表示跟随系统代理。
- `struct RetryPolicy` — `:150` — `—` — `NONE` `:159` / `new(max_retries, backoff_ms)` `:165`。

**crate 根重导出（外壳的最短 use 路径）**

- `tome_core::{ErrorKind, ErrorCapability, LocalizedMessage}` — `crates/tome-core/src/lib.rs:39`。
- `tome_core::{CryptoHost, HeaderMap, HttpMethod, NetHost, ProxyOverride, RequestSpec, Response, RetryPolicy, StoreHost}` — `lib.rs:41-44`。
- `tome_core::{BookRepository, CancelToken, ChangeSourceOutcome, ContentProvider, SearchStream, SourceEngine}` — `lib.rs:45-47`。
- `tome_core::model::*` 全量 DTO 重导出 — `lib.rs:40`。
- ⚠️ **`ClassifiedError` 不在根重导出里**，只能写 `tome_core::error::ClassifiedError`（`lib.rs:39` 只列了 `ErrorCapability`/`ErrorKind`/`LocalizedMessage`）。而 `ErrorKind::with_detail(...)` `error.rs:163` 的返回类型正是它——外壳的错误呈现层必然要写这个全路径。

### 1.2 CLI 组合根里**可复用**的构建块（GUI 不能用 `AppRoot`）

`AppRoot`（`crates/tome-cli/src/root.rs:71`）**不能**被外壳复用，三条硬理由：

1. `AppRoot::assemble(options: &GlobalOptions, ...)` `root.rs:271` 的入参是 `tome_cli::flags::GlobalOptions` —— 外壳要调用它就得依赖 `tome-cli`，直接违反「tome-app 只依赖 tome-core 接缝」的架构铁律。
2. 它**返回具体类型**：`db() -> &Db` `:398`、`sources() -> &SourceStore` `:430`、`data_dir() -> &DataDir` `:393` —— 上层被迫看见 `tome-store` 内部类型。
3. 它是 CLI 形态的：字段含 `config: Config`、`store_factory`、`themes`、`warnings`，生命周期按「一次进程一次命令」设计，没有重载/重启语义。

**可复用**的是它内部调用的那些**独立构建块函数**（外壳应照抄 root.rs 的装配顺序，但用自己的一组入参）：

| 构建块 | 签名 | 位置 | 语义 |
|---|---|---|---|
| `DataDir::resolve_for_current_exe` | `(explicit: Option<&Path>) -> Resolved` | `crates/tome-store/src/datadir.rs:197` | 便携优先的数据目录判定（`PORTABLE_DIR_NAME = "data"` `:39`，`APP_DIR_NAME = "tome"` `:42`）。返回 `Resolved` 枚举 `:61`，**不失败**——失败是 `Unusable` 变体 |
| `DataDir::from_resolved` | `(&Resolved) -> Option<Self>` | `datadir.rs:209` | `Resolved` → `DataDir`；`Unusable` 时 `None` |
| `DataDir::prepare` | `(&self) -> Result<Option<SpaceWarning>, StoreError>` | `datadir.rs:291` | 建目录布局 + 空间检测。**GUI 应调用**（CLI 在 `root.rs:325` 调） |
| `DataDir::root/database_path/config_path/packs_dir/images_dir` | `(&self) -> &Path` / `-> PathBuf` | `datadir.rs:220/230/235/240/245` | 各落点路径 |
| `DataDir::space_warning` / `SpaceWarning::describe` | `(&self) -> Option<SpaceWarning>` / `(&self) -> String` | `datadir.rs:276` / `:314` | 磁盘告警。`describe()` 产的是**英文/中性诊断串**，非面向用户文案 |
| `Config::load` | `(path: &Path) -> Result<Self, StoreError>` | `crates/tome-store/src/config.rs:165` | 读 `config.toml`；非法即错（`root.rs:766` 的测试钉住「不静默用默认值」） |
| `Config::network/storage/display/http_log` | `(&self) -> XSettings` | `config.rs:227/236/245/253` | 类型化子集（外壳**只应消费这些**，不读裸 `Config`） |
| `Db::open` | `(path: &Path) -> Result<Self, StoreError>` | `crates/tome-store/src/db.rs:113` | 建连接 + PRAGMA + **迁移在调用方线程跑**；失败即无 `Db`（无「半开的库」） |
| `RequestExecutor::builder` → `build` | `() -> RequestExecutorBuilder` / `(self) -> Result<RequestExecutor, NetError>` | `crates/tome-net/src/executor.rs:156` / `:621` | 网络执行器。builder 面：`default_interval` `:615`、`replay` `:595`、`cookies` `:602`、`limiter` `:608`、`fail_on_status` `:583`。**外壳不需要 replay** |
| `LoggingNetHost::wrap` | `(Arc<dyn NetHost>, Arc<dyn HttpLogSink>, enabled: bool) -> LoggingNetHost` + `.with_policy(RedactionPolicy)` | `crates/tome-net/src/log.rs:206`（类型 `:176`；经 `root.rs:362` 使用） | 把执行器包成带 http_log 落点的 `NetHost` |
| `HttpLogSinkAdapter::new` | `(MiscDao, keep: usize) -> Self` | `crates/tome-store/src/dao/misc.rs:799` | http_log 落点（实现 `tome_net::HttpLogSink`） |
| `SourceStore::new` | `(SourceDao, export_path: impl Into<PathBuf>) -> Self` | `crates/tome-store/src/source_store.rs:77` | 书源存储（SQLite + `sources.json` 投影，`SOURCE_EXPORT_FILE_NAME = "sources.json"` `:44`） |
| `StoreHostFactory::new` / `for_source` | `(Db, &Path) -> Self` / `(&self, source_id: &str) -> SqliteStoreHostHandle` | `root.rs:186` / `:200` | **按源建 `StoreHost`**（缓存与 Cookie 按源分区）。外壳应保留这个「构造器而非实例」的形态 |
| `SqliteStoreHostHandle::as_store_host` | `(&self) -> Arc<dyn StoreHost>` | `root.rs:224` | 取接缝形态 |
| `ThemeSet::new` / `day` / `night` / `by_id` / `AVAILABLE_IDS` | `() -> Self` / `(&self) -> Theme` ×2 / `(&self, id: &str) -> Option<Theme>` / `[&str; 2]` | `root.rs:120/129/135/147/156` | **FR-029 的组合根注入端**。`by_id` 对未知标识返回 `None`（**不回落**），外壳据此报结构化错误。⚠️ `ThemeSet` 定义在 **tome-cli**（`root.rs:112`），外壳需自建等价物（`tome_core::model::theme::{DayTheme, NightTheme}` 是 `tome-core` 的，可直接用） |
| `ConfigProxyOverride::wrap` | `(Arc<dyn NetHost>, Option<String>) -> Self` | `root.rs:566` | 把配置里的默认代理补到 `spec.proxy_override`（源 > 配置 > 系统三级优先）。**外壳需要它**（GUI 用户不写命令行） |

**装配顺序（照抄 `root.rs:298-385`，外壳版本）**：① 数据目录判定（命令行/显式 > 便携 > 系统）→ ② `DataDir::prepare` → ③ 读 `config.toml` → ④ `Db::open` → ⑤ `RequestExecutor` + 日志包装 + 代理兜底 → ⑥ `SourceStore` → ⑦ `StoreHostFactory` → ⑧ `ThemeSet` → ⑨ 收集 `warnings`（供 UI 展示）。

`HostCapabilities`（`crates/tome-cli/src/commands.rs:123`）是 CLI 侧「注入给 JS 沙箱的宿主能力包」，字段 `net: Option<Arc<dyn NetHost>>` / `store_factory: Option<Arc<StoreHostFactory>>` / `themes: Option<ThemeSet>`，构造 `none()` `:157`、`store_host_for(source_id)` `:161`。**它是外壳应模仿的形状**（一个包而非多个 `Option` 参数），但类型本身在 `tome-cli`，外壳需自建。

---

## 2. 书架与进度

⚠️ **前置事实**：`BookRepository` 接缝（§1.1）**没有生产实现**。外壳要读写书架，当前唯一可走的路是**直接持有 `tome-store` 的 DAO**（每个 DAO 都 `Clone`，内部共享 `Db` 的 actor 通道）。这违反「只依赖接缝 trait」的铁律，是 S3 必须先决的缺口——见文末「待解缺口 §A」。

### 2.1 书架 / 书籍 / 章节（`tome_store::dao::book::BookDao`，`crates/tome-store/src/dao/book.rs:89`）

- `BookDao::new(db: Db) -> Self` — `book.rs:96` — `—` — 构造。另有 `with_now(db, now)` `:104` 供测试钉时间戳。
- `bookshelf(&self) -> Result<Vec<Book>, StoreError>` — `book.rs:111` — `—` — **书架全量**（P1 书架视图的主数据源）。
- `by_id(&self, book_id: i64) -> Result<Option<Book>, StoreError>` — `book.rs:149` — `—` — 单书。
- `by_origin(&self, ...) -> Result<Option<Book>, StoreError>` — `book.rs:128` — `—` — 按 `origin_id` 查（**重复导入识别**：TXT 的内容哈希、网络书的 `书源id|详情URL`）。
- `add_book(&self, &Book) -> Result<i64, StoreError>` — `book.rs:169` — `—` — 加入书架，返回主键。
- `update_book(&self, &Book) -> Result<(), StoreError>` — `book.rs:270` — `—` — 更新元信息。
- `remove_book(&self, book_id: i64) -> Result<(), StoreError>` — `book.rs:322` — `—` — 移除（含章节/痕迹/进度）。
- `set_read_config(&self, book_id: i64, json: Option<&str>) -> Result<(), StoreError>` — `book.rs:247` — `—` — **写 `Book.read_config`（单书排版参数覆盖）**。`None` 表示清除覆盖、回落全局（对应 `ReadingConfig::resolve_for_book` 的语义，§5.1）。
- `chapters(&self, book_id: i64) -> Result<Vec<Chapter>, StoreError>` — `book.rs:337` — `—` — 目录。
- `chapter_at(&self, book_id, index_in_book) -> Result<Option<Chapter>, StoreError>` — `book.rs:352` — `—` — 单章（目录跳转/定位用）。
- `save_chapters(&self, book_id: i64, &[Chapter]) -> Result<(), StoreError>` — `book.rs:387` — `—` — 批量落章（抓完目录一次性写）。
- `mark_read(&self, chapter_id: i64, is_read: bool)` — `book.rs:431` — `—` — 已读标记。
- `chapter_count(&self, book_id: i64) -> Result<i64, StoreError>` — `book.rs:447` — `—` — 章数（进度百分比的分母）。
- `library_roots(&self) -> Result<Vec<LibraryRoot>, StoreError>` — `book.rs:462` — `—` — 本地库根列表（FR-023 支持多个）。
- `add_library_root(&self, path: &str) -> Result<i64, StoreError>` — `book.rs:488` — `—`；`add_library_root_at` `:496`；`remove_library_root(root_id)` `:518`。

### 2.2 进度 / 书签 / 高亮 / 批注 / 阅读记录（`tome_store::dao::trace::TraceDao`，`crates/tome-store/src/dao/trace.rs:61`）

- `TraceDao::new(db) -> Self` — `trace.rs:68`；`with_now(db, now)` `:76`。
- `progress(&self, book_id: i64) -> Result<Option<ReadProgress>, StoreError>` — `trace.rs:83` — `—` — **读进度**（P1「回到上次位置」的入口）。
- `save_progress(&self, &ReadProgress) -> Result<(), StoreError>` — `trace.rs:99` — `—` — 写进度。`ReadProgress { book_id, chapter_index: i32, char_offset: i64, anchor_text: Option<String>, updated_at }`（`crates/tome-core/src/model/trace.rs:26`）。**锚点即 `(chapter_index, char_offset)` + 语义校验文本**，与 S2 的 `AnchorResolution` 同口径。
- `add_read_record(&self, &ReadRecord) -> Result<(), StoreError>` — `trace.rs:140` — `—` — 累加一条阅读记录（FR-026b）。`ReadRecord { book_id, date: String, read_seconds: i64, read_chars: i64 }`（`model/trace.rs:45`）。
- `read_records(&self, book_id, from_date: &str, to_date: &str) -> Result<Vec<ReadRecord>, StoreError>` — `trace.rs:165` — `—` — 按日期范围查。
- `read_records_of(&self, book_id) -> Result<Vec<ReadRecord>, StoreError>` — `trace.rs:200`；`all_read_records` `:217` — 全量。
- `bookmarks(&self, book_id) -> Result<Vec<Bookmark>, StoreError>` — `trace.rs:245` — `—` — **书签列表**。
- `add_bookmark(&self, &Bookmark) -> Result<i64, StoreError>` — `trace.rs:274` — `—` — 新增，返回主键。`Bookmark { id, book_id, chapter_index, char_offset, note: Option<String>, created_at }`（`model/trace.rs:59`）。
- `remove_bookmark(&self, bookmark_id: i64)` — `trace.rs:301` — `—` — 删除。
- `highlights` `:308` / `add_highlight` `:342` / `remove_highlight` `:372` — 高亮（`char_offset + length`，FR-028）。
- `memos` `:379` / `add_memo` `:409` / `remove_memo` `:438` — 批注。
- `trace_count(&self, book_id, chapter_index) -> Result<usize, StoreError>` — `trace.rs:460` — 某章痕迹数（渲染前的快速判空）。

### 2.3 阅读记录聚合（`ReadRecordStats`，`crates/tome-store/src/dao/read_record.rs:111`）

- `ReadRecordStats::new(dao: TraceDao) -> Self` — `read_record.rs:118`；`from_db(db)` `:124`。
- `grand_total()` `:135` / `book_total(book_id)` `:141` / `daily_total(date)` `:152` / `book_daily_total(book_id, date)` `:158` — 各返回 `ReadTotals`（`read_record.rs:46`，含 `minutes()` `:56`、`hours()` `:62`、`is_empty()` `:68`）。
- `daily_breakdown(from, to) -> Vec<DailyTotals>` `:169` / `book_daily_breakdown(...)` `:175` / `per_book_totals(...)` `:194` / `active_day_count()` `:237` — 阅读统计视图（P4 设置页/统计用）。
- **页脚剩余时间**由 S2 侧算，不经这里：`FooterInfo::build(&FooterInput)`（§8.2）吃 `recent_records: Vec<ReadRecord>`。

### 2.4 规则与配置注入

- `MiscDao::new(db)` — `crates/tome-store/src/dao/misc.rs:67`；`with_now` `:75`。
- 四类规则 CRUD（净化/高亮/词典/TXT 目录）：`replace_rules` `misc.rs:84` / `save_replace_rule` `:125` / `remove_replace_rule` `:173`；`highlight_rules` `:182` / `save_highlight_rule` `:208` / `remove_highlight_rule` `:250`；`dict_rules` `:259` / `save_dict_rule` `:284` / `remove_dict_rule` `:323`；`txt_toc_rules` `:332` / `save_txt_toc_rule` `:358` / `remove_txt_toc_rule` `:400`。**这三类规则是净化管线的输入**——外壳改规则后，净化器须重建（见 §4.4 的 `Cleaner` 说明）。
- 搜索关键词：`search_keywords` `misc.rs:422` / `add_search_keyword` `:454` / `touch_search_keyword` `:478` / `remove_search_keyword` `:495`（搜索框历史，P3 需要）。
- Cookie：`cookie` `:647` / `cookies_of_source` `:673` / `put_cookie` `:691` / `clear_cookies_of_source` `:737`（一般由脚本路径自动走，外壳不直接调）。

### 2.5 书源存储（`SourceStore`，`crates/tome-store/src/source_store.rs:68`）

- `SourceStore::new(dao: SourceDao, export_path) -> Self` — `source_store.rs:77` — `—`。
- `merge(&self, &[Source]) -> Result<PersistCounts, StoreError>` — `:97` — **同 id 即更新**；成功后重写 `sources.json` 投影。
- `get(source_id)` `:107` / `all()` `:112` / `groups()` `:117` / `len()` `:122` / `is_empty()` `:127` / `export_path()` `:90` / `rewrite_export()` `:138`。
- `SourceDao`（`crates/tome-store/src/dao/source.rs:92`）：`all` `:136` / `by_id` `:151` / `enabled_only` `:165` / `by_group` `:180` / `upsert_all` `:214` / `remove` `:256` / `set_enabled` `:280` / `assign_group` `:304` / `groups` `:334` / `ensure_group` `:364` / `count` `:398`。

### 2.6 备份 / 清理（P1「移除书」与设置页）

- `BackupStore::new(db)` — `crates/tome-store/src/backup.rs:769`；`export()` `:788` → `Backup`（`:645`）；`export_json()` `:941` → `String`；`import(&Backup)` `:963` → `BackupStats`（`:678`）；`import_json(&str)` `:1173`。统计 `stats_of(&Backup)` `:720`。这组对应接缝的 `export_all`/`import_backup`（`repository.rs:170/173`，`BackupJson { json: String }` `:192`）。
- 缓存清理：`PackStore::clear_book_cache(book_id)` — `crates/tome-store/src/pack.rs:675`；`clear_all()` `:708`；`evict_to(target_bytes)` `:602`；`usage_bytes()` `:751`。图片：`ImageCache::clear()` — `crates/tome-store/src/image_cache.rs:198`；`evict_to` `:218`。**对应 `CacheKind::Chapter` / `CacheKind::Image`（`repository.rs:206`）两个可独立清理的资源**（SC-017）。
- `UserDataDeleter::new(db)` — `crates/tome-store/src/delete.rs:146`；`with_data_dir(db, data_dir)` `:152`；`delete_all()` `:173` → `DeleteReport`（`:60`）；`remaining_rows()` `:251`。

---

## 3. 本地书（tome-local）

`tome-local` 依赖 `tome-core` + `tome-store` + `tome-rule`，**不依赖 `tome-net`**（离线）。外壳经 `tome-local` 导入本地书，产出 `Book` + `Chapter` 后写库（§2.1）。

### 3.1 格式识别与导入（三格式）

- `format::detect(path: &Path, bytes: &[u8]) -> LocalResult<LocalFormat>` — `crates/tome-local/src/format.rs:67` — `—` — 按扩展名 + 内容嗅探。`LocalFormat` 枚举 `:28`：`Txt`/`Epub`/`Mobi`/`Kfx`/`Topaz`；`as_str()` `:43`、`is_supported()` `:57`（**KFX/Topaz 显式拒绝**，`is_supported() == false`）。
- `format::sniff(bytes) -> Option<LocalFormat>` — `format.rs:111`；`looks_like_text(bytes) -> bool` `:211`。
- **TXT**：`txt::import_path(path: &Path, rules: &[TxtTocRule]) -> LocalResult<ParsedTxt>` — `crates/tome-local/src/txt.rs:198` — `—` — 编码检测 → 目录规则回退 → 按大小切分。`txt::import_bytes(bytes, file_name, rules) -> ParsedTxt` `:138`（**不失败**，编码不可识别时也产出结果 + 诊断）。
  - `ParsedTxt { book: Book, toc: Vec<TocPoint>, detection: TocDetection, encoding: EncodingSummary, ... }` — `txt.rs:94`。
  - `TocPoint { title, byte_offset, char_offset, ... }` — `txt.rs:83`。
  - `txt::to_chapters(&[TocPoint], book_id) -> Vec<Chapter>` — `txt.rs:251` — 转接缝 DTO。
  - `txt::word_counts(&str, &[TocPoint]) -> Vec<i32>` — `txt.rs:279` — 各章字数（进度百分比）。
- **EPUB**：`epub::import_path(path) -> LocalResult<ParsedEpub>` — `crates/tome-local/src/epub.rs:220`；`epub::import_bytes(bytes) -> LocalResult<ParsedEpub>` `:144`。`ParsedEpub { book, toc: Vec<TocPoint>, resources: Vec<ResourceEntry>, warnings }`（`:83`/`:96`/`:109`）。
  - `epub::extract_text(xhtml: &str) -> String` — `epub.rs:246`；`extract_text_with_images(xhtml) -> (String, Vec<LocalImageAnchor>)` `:423`。
  - `epub::to_chapters(&[TocPoint], book_id) -> Vec<Chapter>` — `epub.rs:1195`；`read_chapter_bytes(bytes, entry)` `:1188`。
  - 上限常量：`MAX_ENTRY_BYTES = 32 MiB` `:72`、`MAX_TOTAL_BYTES = 512 MiB` `:79`（**zip bomb 防护**，外壳应在导入前做文件大小预检）。
- **MOBI**：`mobi::parse_path(path) -> LocalResult<ParsedMobi>` — `crates/tome-local/src/mobi/mod.rs:351`；`mobi::parse(bytes) -> LocalResult<ParsedMobi>` `:181`。`ParsedMobi { book, chapters: Vec<MobiChapter>, layout: MobiLayout, ... }`（`:74`/`:111`/`:89`）。
  - `mobi::to_chapters(&[MobiChapter], book_id) -> Vec<Chapter>` — `mobi/mod.rs:1164`；`MobiChapter::read_text_range` `:130`。

### 3.2 目录解析（无目录时的回退与规则）

- `toc_rule::detect(text: &str, rules: &[TxtTocRule]) -> TocDetection` — `crates/tome-local/src/toc_rule.rs:97` — `—` — 多条规则按 `sort_order` **回退**；`TocDetection { points, rule_name, split_by_size, attempts }` `:73`。`MIN_TOC_POINTS = 2` `:39`（少于 2 个点即判为「没识别出目录」，转 `split`）。`RuleAttempt` `:52` 记录每条规则为何被拒（导入诊断要展示）。`RuleForm` `:43`、`classify(rule)` `:240`。
- `split::split_by_size(text: &str) -> Vec<TocPoint>` — `crates/tome-local/src/split.rs:46`；`split_by_size_with(text, segment_chars)` `:51`；`DEFAULT_SEGMENT_CHARS = 3000` `:33`；`generated_title(ordinal)` `:170`；`book_name_from_file_name(name)` `:178`。
- `hash::normalize_and_hash(bytes) -> String` — `crates/tome-local/src/hash.rs:59` — 归一化内容哈希（**本地书 `origin_id`**，用于「同一本书重复导入识别为同一条目」）。`content_origin_id(bytes)` `:54`、`normalize(bytes)` `:76`、`sha256_hex` `:162`。
- `encoding::detect_and_decode(bytes) -> DecodedText` — `crates/tome-local/src/encoding.rs:91`；`decode_with_declared(bytes, label) -> Option<DecodedText>` `:129`；`count_replacement_characters(text) -> usize` `:202`。`DecodedText` `:59`。

### 3.3 库根扫描（FR-023~025）

- `library::scan_root(root: &Path) -> ScanReport` — `crates/tome-local/src/library.rs:140`；`scan_roots(&[LibraryRoot]) -> ScanReport` `:166`。`ScanReport { books: Vec<ScannedBook>, skipped: Vec<SkippedFile>, ... }` `:125`。
- `ScannedBook::to_book(library_root_id: Option<i64>, relative_path: Option<String>) -> Book` — `library.rs:88` — 转 DTO。
- `library::copy_into_library(source: &Path, library_root: &Path) -> LocalResult<PathBuf>` — `library.rs:388` — `--copy` 语义（FR-024）。**GUI 拖入文件时可选**。
- `library::relative_to_root(root, file) -> Option<String>` — `library.rs:446`；`normalize_for_storage(path) -> String` `:472`（写库前的路径规范化，**必须用它**，否则 Windows 反斜杠会写进库）。
- `MAX_BOOK_BYTES = 1 GiB` — `library.rs:55`；`ImportMode` `:59`。
- `library::should_ignore_directory(path) -> bool` — `crates/tome-cli/src/local.rs:663`（**在 CLI 里**，扫描时要忽略的目录名集合；外壳需自备一份或复制该逻辑）。

### 3.4 本地书正文读取（`LocalContentProvider`）

- `provider::Cleaner` trait — `crates/tome-local/src/provider.rs:65` — `—` — `pipeline(&self) -> ContentPipeline`。**窄接缝**：每次调用重新取规则，故「用户改了净化规则不必重启」（组合根适配到 `MiscDao` 的四类规则读取）。`NoCleaner` `:71` 是空实现。
- `LocalSource::open(path: &Path, toc: Vec<Chapter>) -> LocalResult<Self>` — `provider.rs:113`；`new(path: PathBuf, format: LocalFormat, toc)` `:124`；`path()` `:134` / `format()` `:139` / `toc()` `:144` / `raw_chapter(&Chapter)` `:171` / `raw_chapter_with_images(&Chapter)` `:189`。三种格式都是**偏移直读**（TXT seek / EPUB 单条目解压 / MOBI 记录），**不整本进内存**。
- `LocalContentProvider::new(source: Arc<LocalSource>, cleaner: Arc<dyn Cleaner>) -> Self` — `provider.rs:329`；`without_cleaning(source)` `:334`；`source()` `:339`。
- `impl ContentProvider for LocalContentProvider` — `provider.rs:385` — `toc` `:386`（来自导入结果，**不重抓**）/ `chapter_content` `:392`（**必跑净化管线** + 图片锚点位置校正）/ `resolve_anchor` `:435` / `prefetch` `:470`（本地书也要预取——解压不免费）。
- `epub_image_refs(raw: &str, purified: &str, anchors: &[LocalImageAnchor]) -> Vec<ImageRef>` — `crates/tome-local/src/lib.rs:70` — `—` — 把 EPUB 图片锚点从**原始文本**偏移校正到**净化后文本**偏移（PM 裁决 OQ#13）。外壳若自己拼 `ChapterContent` 必须用它，否则图文位置会错。
- crate 根重导出：`tome_local::{LocalError, LocalResult}` `lib.rs:57`；`LocalFormat` `:58`；`txt::{import_bytes, import_path, ParsedTxt, TocPoint}` `:59`。

---

## 4. 书源管线（tome-source）

⚠️ **前置事实**：`SourceEngine` trait（§1.1）**没有生产实现**。真实入口是**自由函数 + `SearchEngine`**，且它们的 JS 变体要求外壳**自建 JS 引擎工厂**（组合根职责）。见文末「待解缺口 §B」。

### 4.1 书源导入（三渠道，FR-001）

- `source::load_from_channel(channel: &SourceImportChannel, host: &dyn NetHost) -> Result<LoadOutcome, SourceError>` — `crates/tome-source/src/source.rs:565` — `—` — **FR-001 三渠道统一入口**。`SourceImportChannel`（`crates/tome-core/src/seam/repository.rs:219`）：`FileText(String)` / `Url(String)` / `PastedText(String)`。
  - `Url` 渠道经 `NetHost` 发请求（**A2 唯一发请求处**），限速键 `IMPORT_RATE_LIMIT_KEY = "import://source-import"` `source.rs:598`；非 2xx 报 `SourceError::Fetch`。
  - 外壳用法：文件 → 读成 `String` → `FileText`；剪贴板 → `PastedText`；URL 输入框 → `Url`。
- `source::parse_sources(text: &str) -> Result<LoadOutcome, SourceError>` — `source.rs:220` — 纯解析（不落库）。
- `source::report_for_persistence(&LoadOutcome) -> ImportReport` — `source.rs:623` — 解析报告 → 接缝报告（`ImportReport { added, updated, skipped, failures: Vec<ImportFailure> }`，`repository.rs:233`/`:246`）。**外壳的导入结果提示直接用这个形状**（逐条失败带 `ErrorKind`）。
- `ParsedSource { source: Source }` — `source.rs:54` — 管线各处要的「已解析书源」包装。`LoadOutcome` `:61`。
- 落库：`SourceStore::merge(&[Source])`（§2.5）。
- `source::parse_headers(Option<&str>) -> Result<HeaderMap, SourceError>` — `source.rs:633`（外壳一般不需要）。

### 4.2 搜索（流式，A6）

- `SearchEngine::new(sources: &'a [ParsedSource], host: &'a dyn NetHost) -> Self` — `crates/tome-source/src/search.rs:720` — `—` — **无 JS** 的搜索执行器（纯 CSS/正则源足够）。
- `SearchEngine::with_js(sources, host, js_factory: JsEngineFactory<'a>) -> Self` — `search.rs:738` — 带 JS 工厂。`JsEngineFactory<'f> = Arc<dyn Fn(&Source) -> Result<SourceJsEngine, SourceError> + Send + Sync + 'f>` — `search.rs:712`。**用工厂而非实例**：隔离粒度是「每源脚本一个 runtime」。
- `SearchEngine::search(&self, keyword: &str, source_ids: &[String]) -> Box<dyn SearchStream + 'a>` — `search.rs:753` — **增量流**；`source_ids` 空 = 全部源。**串行逐源产出**（`SourceSearchStream`，`search.rs:783`），首结果时间 = 第一个完成的源。
- `search_stream::parallel_search(sources: Vec<ParsedSource>, keyword: String, host: Arc<dyn NetHost>, js_factory: Option<JsEngineFactory<'static>>, max_workers: usize) -> ParallelSearchStream` — `crates/tome-source/src/search_stream.rs:221` — **并行版**：共享队列 + N worker（`std::thread`，`search_stream.rs:274`），按 **source_id 排序**确定性产出。**外壳的搜索应优先用它**（多源并发是 GUI 的响应性关键）。`collect_parallel` `:378`。
- `search::collect_search(...)` — `search.rs:856` — 攒齐（CLI 用；GUI 不要用，会阻塞）。
- `SearchHit::to_book(&self, source_id: &str) -> Book` — `search.rs:433` — 搜索结果 → `Book`（**加入书架**时用）。

### 4.3 详情 / 目录 / 正文（五个动作）

- `info::book_info(parsed: &ParsedSource, book_url: &str, host: &dyn NetHost) -> Result<Book, SourceError>` — `crates/tome-source/src/info.rs:156`；带 JS：`book_info_with_js(parsed, book_url, host, socket: Option<&SourceJsEngine>)` `:165`。返回 **`Book`**（不是独立 `BookInfo`——`Book` 即详情实体，见 `seam/source.rs:37` 的说明）。
- `toc::toc(parsed, book: &Book, host) -> Result<Vec<Chapter>, SourceError>` — `crates/tome-source/src/toc.rs:162`；`toc_with_js(parsed, book, host, socket)` `:171`。`directory_url(book) -> Option<String>` `:202`（目录地址从 `book.toc_url` 取）。
- `content::chapter_content(parsed, chapter: &Chapter, host, cache: &Option<Arc<dyn ChapterCache>>, purifier: ...) -> Result<ChapterOutcome, SourceError>` — `crates/tome-source/src/content.rs:199`；`chapter_content_with_js(...)` `:210`。`ChapterOutcome { raw, raw_html, text, ... }` `:92`（`raw()` `:122` / `raw_html()` `:133`）。
  - `trait ChapterCache: Send + Sync` — `content.rs:58` — `NoChapterCache` `:72`；`PackChapterCache::new(Arc<PackStore>)` `:317`；`pack_cache(store) -> Arc<dyn ChapterCache>` `:348`。**外壳应注入 `pack_cache`**，否则正文不落缓存。
- `discover::discover(parsed, kind: &str, page: u32, host) -> Result<ExplorePage, SourceError>` — `crates/tome-source/src/discover.rs:249`；`discover_with_js(...)` `:259`。`ExplorePage { books: Vec<Book>, next_page: Option<u32> }`（`seam/source.rs:98`）。**S3 范围外**（发现页浏览属延后项 D1），列出仅供完整性。

### 4.4 净化（管线已在 seam 之下，但外壳要构造净化器）

- `Purifier::new(replace_rules: Vec<ReplaceRule>, highlight_rules: Vec<HighlightRule>, dict_rules: Vec<DictRule>) -> Self` — `crates/tome-source/src/purify.rs:110` — `—` — **构造净化器**（自动过滤 `!enabled` 并按 `sort_order`/`id` 排序 → 同配置结果确定）。规则来自 `MiscDao`（§2.4）。
- `Purifier::empty() -> Self` — `purify.rs:141`；`rule_counts() -> (usize, usize, usize)` `:146`；`purify(text, target: PurifyTarget, dictionary_words: &[String]) -> Result<PurifyOutcome, SourceError>` `:159`；`purify_text_only(text, target) -> Result<String, SourceError>` `:176`（书名/章标题只需替换，不产高亮区间）。
- `SharedPurifier = Arc<Purifier>` — `purify.rs:279` — 网络 provider 的构造参数类型。
- `PurifyTarget` 枚举 — `purify.rs:56`；`applies_to(scope, target) -> bool` `:81`。
- `highlights_to_traces(...)` — `purify.rs:254` — 净化产出的高亮区间 → 阅读痕迹（外壳做高亮渲染时的桥）。
- ⚠️ **规则变更后必须重建 `Purifier`/`Cleaner`**：`Purifier` 在构造时就快照了规则（`purify.rs:110-140`）。外壳改规则后要重建 provider 或走 `Cleaner::pipeline()` 的每次重取路径（§3.4）。

### 4.5 换源（FR-010 / FR-010b）

- `change_source::change_source<F>(old_book: &Book, progress: Option<&ReadProgress>, candidate: &ParsedSource, new_chapters: Vec<Chapter>, anchor: Option<ChapterMatch>, probe_content: F) -> Result<ChangeSourceOutcome, SourceError> where F: FnMut(i32) -> Result<Option<String>, SourceError>` — `crates/tome-source/src/change_source.rs:273` — `—` — 换源执行。**`probe_content` 是回调**（第一级特征段匹配要在新源邻域章节正文里搜），外壳用 `ContentProvider::chapter_content` 适配它。
- `ChangeSourceOutcome { book, chapters, progress_mapping }` — `seam/source.rs:113`；`ProgressMapping` `:124` = `AnchorSegment { chapter_index, char_offset, similarity_percent }` / `ChapterTitle { chapter_index }` / `FallbackToStart`。**第三级必须明确提示用户**（不静默跳转）。
- `map_by_anchor` `change_source.rs:157` / `map_by_title` `:200` / `map_progress` `:241` / `similarity_percent` `:91` — 三级映射的零件（一般不需要直接调）。
- `ChangeReason` 枚举 `:336` + `should_change(...)` `:349` — 「这个候选源值不值得换」的判据（界面列出候选源时用）。
- `network_probe(...)` `:384` / `neighborhood_probe(...)` `:370` — 现成的 probe 适配器（后者签名见源码，返回 `impl FnMut(i32) -> Result<Option<String>, SourceError>`）。
- **候选源从哪来**：`search` 同一书名 → 取各源结果的 `Book` → 逐个 `toc` 拿 `new_chapters`。外壳的换源面板即此流程。

### 4.6 源检查（FR-011，S3 范围外，列出备用）

- `check::check_source(parsed, host) -> Result<CheckReport, SourceError>` — `crates/tome-source/src/check.rs:58`；`check_source_verdict(parsed, host) -> CheckVerdict` `:71`（**不失败**，只给判定）；`check_sources(sources, filter, host, ...) -> impl CheckStream` `:163`；`select_sources(&[ParsedSource], &SourceCheckFilter) -> Vec<ParsedSource>` `:135`；`summarize(available, unavailable) -> CheckSummary` `:255`。
- `CheckReport { source, verdict }` — `seam/source.rs:148`；`CheckVerdict::Available` / `Unavailable { reason: ErrorKind }` `:157`。

### 4.7 内容提供者（**外壳正文加载的唯一推荐入口**）

- `NetworkContentProvider::new(sources: Arc<dyn SourceLookup>, host: Arc<dyn NetHost>, cache: Option<Arc<dyn ChapterCache>>, purifier: SharedPurifier) -> Self` — `crates/tome-source/src/provider.rs:87` — `—` — 网络书 adapter。
- `NetworkContentProvider::bare(sources, host) -> Self` — `provider.rs:103` — 不净化不缓存（测试用）。
- `source_id_of(book: &Book) -> Result<&str, ErrorKind>` — `provider.rs:112` — 从 `book.origin_id`（`书源id|详情URL`）解出源 id。
- `impl ContentProvider for NetworkContentProvider` — `provider.rs:212`。
- `trait SourceLookup: Send + Sync { fn find(&self, source_id: &str) -> Option<ParsedSource>; }` — `crates/tome-source/src/source.rs:1362` — **外壳必须自己实现它**（组合根职责：同时看见 `tome-source` 的 trait 与 `tome-store` 的 DAO）。`SliceLookup::new(Vec<ParsedSource>)` `source.rs:1373` 是内存实现（测试用）。**参考实现**：`tome_cli::pipeline_commands::DaoSourceLookup` — `crates/tome-cli/src/pipeline_commands.rs:353`，`new(SourceDao)` `:372`，内含 `Mutex<HashMap>` 缓存且**不失效**（注释明确：长命场景应在源变更时重建 provider）。
- `image::chapter_images(...)` `crates/tome-source/src/image.rs:364`；`extract_image_anchors` `:418`；`align_anchors_after_purify` `:543`；`StoreImageCache::new(Arc<ImageCache>)` `:656` + `store_image_cache(...)` `:678` — 图片抓取与锚点校正（图文混排的缓存键来源）。`ImageRef { char_offset, cache_key, alt }`（`seam/source.rs:248`）。
- `js::source_needs_javascript(&Source) -> bool` — `crates/tome-source/src/js.rs:58` — **先判再建**（无 JS 的源建 runtime 是浪费）。
- `js::SourceJsEngine::for_source(...)` — `js.rs:145`；`with_context` `:201`；`eval` `:211`；`is_poisoned` `:218`；`runtime()` `:196`。**外壳的 JS 工厂参考实现**：`tome_cli::js_host::build_source_engine(...)` — `crates/tome-cli/src/js_host.rs:48`（它注入 `NetHost` + `StoreHost` + `MarkupHost`）。⚠️ **它在 tome-cli 里，外壳要复制这段装配**。

---

## 5. 排版与渲染（tome-reader）

### 5.1 排版参数集（`ReadingConfig`，几何/呈现严格分离）

`tome-reader` **不读 Theme**——分页结果只依赖几何（FR-019/SC-005 由类型强制：分页函数只收 `&GeometryConfig`）。

- `GeometryConfig` — `crates/tome-reader/src/paginate/config.rs:126` — `—` — 字段：`font_size_px: u16`、`line_height: f32`、`paragraph_mode: ParagraphMode`、`measure_em: f32`、`measure_max_em: f32`、`gutter_px: u16`、`spread_mode: SpreadMode`、`reading_mode: ReadingMode`、`chapter_new_page: bool`。**任一变化 → 重排 + 重新分页**。
  - 方法：`measure_width_px()` `:171`、`measure_max_width_px()` `:177`、`line_height_px()` `:183`、`validate() -> Result<(), &'static str>` `:200`（**Err 侧是字段名 ASCII 标识**，如 `"font_size"`）、`from_json(&str) -> Result<Self, ReaderConfigError>` `:211`、`to_json()` `:221`。
- `PresentationConfig` — `config.rs:282` — `—` — `theme_id: ThemeId`、`font_family: FontFamily`、`reduce_motion: bool`、`paper_noise: bool`。**任一变化 → 只重绘**（页序列逐字节相同）。
- `ReadingConfig` — `config.rs:316` — `—` — `{ geometry, presentation }`。`default_ma()` `:330`、`validate()` `:343`、`to_json()` `:382`、`from_json()` `:393`（**反序列化即校验**）、`resolve_for_book(global: &Self, read_config: Option<&str>) -> Result<ResolvedConfig, ReaderConfigError>` `:428`（全局默认 + 单书覆盖，**整份替换不逐字段合并**）。
  - `ResolvedConfig { config, is_book_override: bool }` `:454` — `is_book_override` 供 UI 区分「跟随全局」与「已为本书设置」。
- 常量：`FONT_SIZE_RANGE = 14..=30` `:39`、`LINE_HEIGHT_RANGE = 1.75..=2.2` `:46`（**下限 1.75 是无障碍强制条款**，MASTER §2.3）、`DEFAULT_FONT_SIZE_PX = 19` `:49`、`DEFAULT_LINE_HEIGHT = 1.85` `:52`、`DEFAULT_MEASURE_EM = 36.0` `:55`、`DEFAULT_MEASURE_MAX_EM = 42.0` `:58`、`DEFAULT_GUTTER_PX = 64` `:61`。
- 字段级校验（供设置控件即时反馈）：`validate_font_size(u16)` `:232`、`validate_line_height(f32)` `:247`（NaN 显式拒绝）、`validate_measure_em(f32)` `:259`、`validate_gutter_px(u16)` `:273`、`validate_paragraph_mode`（转发，`:368`）。
- 枚举：`SpreadMode { Auto, ForceSingle, ForceSpread }` `:72`；`ReadingMode { Paged, Scroll }` `:84`；`ThemeId { Parchment, Obsidian }` `:97`；`FontFamily { EmbeddedSerif, SystemFallback }` `:113`。**全部 `Default`**（`SpreadMode::Auto` / `ReadingMode::Paged` / `ThemeId::Parchment` / `FontFamily::EmbeddedSerif`）。
- `ReaderConfigError` — `config.rs:471` — `Parse { detail }` / `Serialize { detail }` / `InvalidField { field: &'static str }`；`message_id()` `:514`（`error.reader.read_config_invalid_field` 等）、`field()` `:524`。**配置损坏时报错不静默回落**。
- `ParagraphMode` — `crates/tome-reader/src/layout/paragraph_mode.rs`（经 `config.rs:66` 重导出）— `Indent { em }` / `Gap { em }` 互斥；默认 `Indent { em: 2.0 }`；`DEFAULT_PARAGRAPH_INDENT_EM` / `DEFAULT_PARAGRAPH_GAP_EM`。

### 5.2 排版入口与分页（`Paginator`）

- `Paginator` — `crates/tome-reader/src/paginate/page.rs:352` — `—` — **长生命周期对象**（持 `LayoutEngine` → `FontSystem`，构造开销可观）。`Paginator::new() -> Self` `:485`（**不失败**：内嵌字体装载失败时降级到系统字体栈而非 panic）。`#[derive(Default)]`。
  - ⚠️ **`Paginator` 的所有排版方法都要 `&mut self`** → 它不能被多个线程共享（除非 `Mutex`），外壳必须给它一个**专属线程/单点所有权**（见 §9 线程模型）。
- `layout_chapter(paginator: &mut Paginator, content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> Result<ChapterLayout, ReaderError>` — `page.rs:400` — `—` — **唯一的排版入口**（FR-021 的结构性保证：翻页与滚动都从它出发，没有第二条排版路径）。不做：不切页、不取内容、不读 Theme。
  - `ChapterLayout { lines: Vec<PageLine>, char_len: usize, measure_px: f32, columns: Columns, override_active: bool, rule_exceptions, split_diagnostic, degraded_words }` — `page.rs:368`。
- `Paginator::paginate(&mut self, content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> Result<PaginationResult, ReaderError>` — `page.rs:510` — `—` — **翻页形态**：内部走 `layout_chapter` + 贪心装页 + 页区间收尾 + 左右归属。空正文产出一页（**绝不零页**）。
  - `PaginationResult { pages: Vec<Page>, spread: SpreadResolution, rule_exceptions, split_diagnostic, degraded_words, char_len }` — `page.rs:256`。
  - `Page { chapter_index: usize, page_in_chapter: u32, char_range: Range<usize>, lines: Vec<PageLine>, side: Option<SpreadSide>, is_padding: bool }` — `page.rs:71`。`char_len()` `:89` / `is_empty()` `:102`。
  - `PageLine { line: LineBox, chapter_char_range: Range<usize> }` — `page.rs:62`。⚠️ **`LineBox::char_range` 是段内偏移，`PageLine::chapter_char_range` 才是章内偏移**——外壳做高亮/命中/进度换算时**一律用后者**。
  - `LineBox` — `crates/tome-reader/src/layout/linebox.rs:106` — `paragraph_index` / `char_range`（段内）/ `glyphs: Vec<GlyphBox>` / `height_px` / `baseline_px` / `squeeze_px` / `justify_extra_px` / `indent_px` / `rule_exception`。
- `PaginationResult::resolve_anchor(&self, char_offset: usize) -> Option<AnchorResolution>` — `page.rs:298` — `—` — **字符锚点 → 页定位**。越界**钳制**到 `[0, char_len]`（不 panic）。返回 `AnchorResolution { chapter_index, char_offset, target: AnchorTarget }` `:127`；便捷判据 `is_visible()` `:139` / `page_index()` `:145` / `offset_in_page()` `:151`。
  - `AnchorTarget` 枚举 — `page.rs:164` — `Paged { page_index, page_in_chapter, offset_in_page, side, visible }` / 滚动变体；访问器 `visible()` `:197` / `page_index()` `:205` / `viewport_index()` `:214` / `offset_in_target()` `:223` / `side()` `:234` / `char_range()` `:243`。
- `SpreadResolution { columns, measure_px, override_active }` — `page.rs:109`。

### 5.3 单双页（spread）

- `Viewport::new(available_width_px: f32, available_height_px: f32) -> Self` — `crates/tome-reader/src/paginate/spread.rs:50` — `—` — **阅读区可用宽高**（扣掉侧栏与内边距之后的净区）。这是外壳每次窗口 resize 都要重算的输入。
- `Columns { Single, Double }` — `spread.rs:60` — `count()` `:70` / `is_double()` `:79`。
- `SpreadSide` — `spread.rs:86` — `for_page_index(u32)` `:126` / `for_chapter_start()` `:136` / `opposite()` `:142` / `is_left()` `:151` / `is_even_page()` `:96` / `as_str()` `:102`。
- `required_width_px(&GeometryConfig) -> f32` — `spread.rs:162` — 双页所需的**最小**可用宽（em 制判据）。
- `auto_columns(&Viewport, &GeometryConfig) -> Columns` — `spread.rs:168` — **FR-014 的 em 制自动判据**。
- `column_measure_px(columns, &Viewport, &GeometryConfig) -> f32` — `spread.rs:196` — 单栏版心宽。
- `resolve_columns(&Viewport, &GeometryConfig) -> Columns` — `spread.rs:217` — 含**手动覆盖**的最终栏数。
- `resolve_override(&Viewport, &GeometryConfig) -> OverrideResolution` — `spread.rs:276`；`OverrideResolution { columns, override_active, ... }` `:243`。**手动覆盖在窗口变化后是否仍生效**的语义在此。
- `override_decision_after_resize(...)` `:324` + `OverrideDecision` `:337` — resize 后对「强制双页」的处置判定（窗口变窄到放不下双页时怎么办）。**外壳 resize 处理必须调它**，否则「强制双页」在小窗口下会破版。
- `BookSemantics` — `crates/tome-reader/src/paginate/booksem.rs:51` — `—` — 书页语义（左偶右奇 / 章起偶数页留白，FR-016）。`apply_book_semantics(&[Page], BookSemantics) -> BookSemanticsResult` `:123`；`BookSemanticsResult` `:60`；`padding_needed_for_chapter_start(previous_chapter_pages: usize) -> usize` `:88`。

### 5.4 滚动形态

- `ScrollFlow::new(chapter: ChapterLayout, geometry: GeometryConfig, viewport: Viewport) -> Result<Self, ReaderError>` — `crates/tome-reader/src/scroll/flow.rs:146` — `—` — **输入是 `ChapterLayout` 而非正文**（类型上杜绝第二条排版路径）。空章产出一个空视口。
- `ScrollFlow` 访问器：`viewports() -> &[ScrollViewport]` `:183`、`content_height_px() -> f32` `:189`、`measure_px()` `:195`、`char_len()` `:201`、`lines() -> &[PageLine]` `:213`、`resident_glyph_lines() -> usize` `:219`（**释放的可观测证据**）。
- `ScrollViewport` — `flow.rs:77` — `viewport_index` / `top_px` / `height_px` / `char_range` / `line_range: Range<usize>`（**行在流里的下标区间，不是行的副本**）。`char_len()` `:111` / `is_empty()` `:117`。
- `ScrollFlow::viewport_lines(&self, &ScrollViewport) -> &[PageLine]` — `flow.rs:233` — 零拷贝取某屏可见行（渲染批次直接吃它）。
- `ScrollFlow::release_outside(&mut self, viewport_index: usize, radius: usize) -> usize` — `flow.rs:275` — **FR-026 按需释放**：清空窗口外的 `glyphs`，**保留字符区间与行高**（位置语义不变）。`radius` = 前后各保留几屏。**超长章节必须调它**（20 万字不释放必然超 SC-010 的内存预算）。
- `ScrollFlow::char_ranges_needing_relayout(&self, from_viewport, to_viewport) -> Option<Range<usize>>` — `flow.rs:321` — 回滚到已释放区域时**需要重排**的字符范围（释放不可逆，恢复 = 重排）。
- `PositionMap::new(flow: &ScrollFlow) -> Self` — `crates/tome-reader/src/scroll/map.rs:59`；`PositionMap::for_page(page: &Page) -> Self` `:70` — `—` — **双向映射**（FR-022/040）。
  - `total_height_px()` `:99` / `line_count()` `:105` / `char_to_offset(char_index) -> Option<f32>` `:123` / `offset_to_char(offset_px) -> Option<usize>` `:163` / `point_in_line_to_char(&PageLine, x_px) -> Option<usize>` `:205` / `line_content_origin_px(&PageLine) -> f32` `:248` / `char_to_point_in_line(&PageLine, char_index) -> Option<f32>` `:269`。
- `relayout::relayout(paginator: &mut Paginator, content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> Result<ScrollFlow, ReaderError>` — `crates/tome-reader/src/scroll/relayout.rs:140` — **滚动形态的重排便捷入口**（`layout_chapter` + `ScrollFlow::new` 一步，复用同一 `Paginator`）。**不接收也不返回锚点**——锚点由调用方在重排外持有。
- `RelayoutRequest { width_px, height_px }` — `relayout.rs:44`；`viewport()` `:54`。
- `ResizeCoalescer` — `relayout.rs:82` — `new()` `:89` / `request(width, height)` `:94`（**覆盖而非追加**）/ `take() -> Option<RelayoutRequest>` `:104` / `pending_count()` `:113`（恒 ≤ 1）/ `has_pending()` `:119`。**把 egui 每帧的 resize 事件合并成一次重排**——外壳应每帧 `request()` + 在合适时机 `take()`。
- `ScrollEventThrottle` — `crates/tome-reader/src/scroll/throttle.rs:90` — `new()` `:100` / `on_scroll_event(offset_px)` `:113` / `take() -> Option<ThrottleDecision>` `:127` / `pending_count()` `:154` / `last_processed_offset()` `:160`。`ThrottleDecision` `:54`（`offset_px()` `:70` / `needs_relayout()` `:79`）；`NEGLIGIBLE_DELTA_PX = 1.0` `:50`。**不累积排队**（FR-027）。⚠️ 时间维度的节流**留给外壳的帧循环**（本层是纯几何，无时钟）。
- `PlaceholderKind` — `crates/tome-reader/src/scroll/placeholder.rs:42` — `message_id()` `:59`（**标识而非文案**）、`is_user_actionable()` `:79`。`ScrollPlaceholder { kind, char_range }` `:86`，`new(kind, range)` `:96`，`char_len()` `:102`。`none_for_cached_content(&ScrollFlow, content) -> Vec<Self>` `:127`。
- `TRANSCRIPTION_SETTLE_EVENTS = 3` — `flow.rs:60`。

### 5.5 形态切换（翻页 ⇄ 滚动，FR-023）

- `trait ProgressSink` — `crates/tome-reader/src/scroll/switch.rs:56` — `—` — `save_progress(&mut self, chapter_index: i32, char_offset: i64)`。**窄 trait**：外壳适配到 `TraceDao::save_progress`。⚠️ 它**无返回值**（写失败只能吞掉或自行记录），与 `BookRepository::save_progress` 的 `Result` 不同。
- `trait AnchorTarget` — `switch.rs:197` — `—` — `resolve_anchor(&self, anchor: usize) -> usize`（钳制）/ `anchor_is_visible(&self, anchor: usize) -> bool`。已为 `PaginationResult`（`:209`）与 `ScrollFlow`（`:225`）实现。
- `ModeSwitcher::begin(from_mode: ReadingMode, anchor: usize) -> Self` — `switch.rs:88` — `—` — 进入**过渡态**。
  - `observe_event()` `:102` / `observed_events()` `:108` / `is_settled()` `:114` / `settle(to_mode: ReadingMode, target: &impl AnchorTarget) -> usize` `:135`（在目标形态里定位锚点）/ `anchor()` `:144` / `target_visible()` `:150` / `from_mode()` `:156` / `to_mode()` `:162` / `commit(sink: &mut impl ProgressSink) -> bool` `:179`（**过渡态期间拒绝写入**；幂等）。
- **外壳用法**：切形态时 `begin` → 构造目标形态 → `settle` → 渲染稳定后 `commit`。`commit` 返回 `bool` 表示是否真的写了。

### 5.6 重排与预计算（改字号/行高后的入口）

- `incremental_reflow(paginator: &mut Paginator, content: &str, geometry: &GeometryConfig, viewport: &Viewport, anchor: usize, baseline: &PaginationResult, token: &CancelToken) -> Result<ReflowOutcome, ReaderError>` — `crates/tome-reader/src/precalc.rs:325` — `—` — **改字号/行高后的重排入口**（增量：首帧只排锚点所在段及之后）。`token` 可取消。
  - `ReflowOutcome` — `precalc.rs:234` — `Pending(PendingReflow)` / `Complete(PaginationResult)` / `Cancelled`。
  - `PendingReflow::first_frame_end() -> usize` `:272` / `complete(self, &mut Paginator) -> Result<PaginationResult, ReaderError>` `:297` — 首帧渲染完后再补齐。
- `PrecalcRequest { chapters: &[String], around_index: usize, depth: usize, geometry, viewport }` — `precalc.rs:62` — `—` — **预取请求**（正文由调用方从 `ContentProvider` 取好传入，本层不取）。
- `precalc_chapters(paginator: &mut Paginator, request: PrecalcRequest, token: &CancelToken) -> PrecalcOutcome` — `precalc.rs:169` — `—` — 批量预排（**单章失败不终止批次**）。
  - `PrecalcOutcome { Completed { cache, computed } , Cancelled { cache, computed } }` `:77`。取消时**已完成的章仍有效**。
- `PaginationCache` — `precalc.rs:103` — `new()` `:110` / `pages_of(chapter_index) -> Option<&[Page]>` `:116` / `is_empty()` `:122` / `len()` `:128`。**进程内、不持久化**（裁决 OQ#18→A），值类型不是全局单例——**参数一变就得重算**。

### 5.7 阅读记录（跨形态去重）

- `trait RecordSink` — `crates/tome-reader/src/record.rs:54` — `—` — `add_read_record(&mut self, read_seconds: i64, read_chars: i64)`（累加语义）。外壳适配到 `TraceDao::add_read_record`。
- `ReadingRecordAccumulator` — `record.rs:66` — `new()` `:80` / `observe_range(range: Range<usize>)` `:98`（**已展示字符区间的有序不相交并集**——重叠与相接不重复计入；翻页传 `page.char_range`，滚动传 `viewport.char_range`）/ `add_seconds(seconds: i64)` `:156` / `read_chars()` `:164` / `read_seconds()` `:175` / `pending_chars()` `:181` / `commit(&mut impl RecordSink) -> bool` `:202` / `span_count()` `:215`。
  - **外壳节奏**：每呈现一页/一屏 `observe_range`，每 30 秒或退出阅读视图时 `commit`。

### 5.8 命中测试与文字选择（FR-038/040）

- `HitPoint { x_px, y_px, line_index }` — `crates/tome-reader/src/trace/hit.rs:41`；`HitResult { char_index, line_index }` `:52`；`Selection { char_range: Range<usize> }` `:67`。
- `hit_test_page(page: &Page, x_px: f32, y_px: f32) -> Option<HitResult>` — `hit.rs:79` — `—` — 翻页形态命中。y 落在留白处返回 `None`（**不静默归给某行**）。
- `hit_test_scroll(flow: &ScrollFlow, x_px: f32, y_px: f32) -> Option<HitResult>` — `hit.rs:86` — `—` — 滚动形态命中。
- `select_range(start: HitPoint, end: HitPoint, map: &PositionMap, lines: &[PageLine]) -> Option<Selection>` — `hit.rs:151` — 拖选 → **字符区间**（半开）。单击产零长区间（光标位置），**不是 `None`**。
- 坐标口径：`x_px` 相对**行左边缘**，`y_px` 相对**页首/流首**。外壳要把 egui 的屏幕坐标换算成这个局部坐标（减去版心起点与滚动偏移）。

### 5.9 痕迹渲染（高亮/书签可视化，FR-037）

- `TraceRange::new(start: usize, length: usize)` — `crates/tome-reader/src/trace/render.rs:67`；`from_range(Range<usize>)` `:73`；`end()` `:82` / `as_range()` `:88` / `is_empty()` `:94` / `intersect(&Range<usize>) -> Option<Range<usize>>` `:102`。
- `resolve_style(style: &str) -> StyleResolution` — `render.rs:335` — `—` — 样式词表解析（语义色 / 颜色名 / 回落）。`StyleResolution` `:237`：`is_semantic()` `:251` / `is_color_name()` `:257` / `is_fallback()` `:263` / `effect()` `:269` / `has_background()` `:275` / `has_underline()` `:281` / `diagnostic()` `:287`。`SEMANTIC_STYLE_VOCABULARY` `:173`。
- `render_traces(traces: &[TraceRange], pages: &[Page], &GeometryConfig, &Viewport, theme: Theme, resolution: &StyleResolution) -> Vec<TraceSegment>` — `render.rs:513` — `—` — 翻页形态；**接收多页**（跨页区间自动分段）。
- `render_traces_scroll(traces, flow: &ScrollFlow, &GeometryConfig, &Viewport, theme, resolution) -> Vec<TraceSegment>` — `render.rs:542` — 滚动形态（跨视口分段）。
- `render_traces_to_lines(traces, lines: &[PageLine], theme, resolution) -> Vec<TraceSegment>` — `render.rs:561` — 调用方已取好行的场景。
- `covered_chars(&[TraceSegment]) -> BTreeSet<usize>` — `render.rs:579` — 覆盖的章内字符集合（诊断/断言）。
- `TraceSegment` `:394` / `DecorationKind` `:410` / `StyleEffect` `:125` / `StyleLevel` `:226`。
- ⚠️ **`GeometryConfig` 与 `Viewport` 在 `render_traces*` 里是 `_` 未用参数**（`render.rs:516` 与 `:545` 的 `_geometry`）——签名保留是为了将来加几何相关的分段规则，外壳传当前值即可。
- 词典查词桥：`DictQueryInput` `crates/tome-reader/src/trace/dict.rs:57`；`DictTrigger::from_selection(range)` `:83` / `at_cursor(char_offset)` `:89` / `from_selection_result(&Selection)` `:97` / `char_range()` `:105`；`build_lookup_request(&DictQueryInput) -> Result<DictLookupRequest, DictFailure>` `:201`；`DictFailure::as_str()` `:149` / `guidance()` `:172`。**本层只产查询请求，不产释义**（发请求是外层的事）。

### 5.10 渲染出口（嵌进 GUI 的关键）

- `render::backend_name() -> &'static str` — `crates/tome-reader/src/render/mod.rs:47` — `render` — 编译期候选后端名（**不是**本机能力探测）。
- `RenderPipeline::probe_backend() -> Result<GraphicsBackend, PipelineError>` — `crates/tome-reader/src/render/pipeline.rs:198` — `render` — 探后端（**不建 device**，廉价、失败只说明没后端）。
- `RenderPipeline::probe_backend_with(backends: wgpu::Backends) -> Result<GraphicsBackend, PipelineError>` — `pipeline.rs:213` — `render` — 限定后端集（传 `Backends::empty()` 可**确定性**走到 `NoAdapter`，CI 靠它验证「诚实报告」）。
- `GraphicsBackend` — `pipeline.rs:131` — `name()` `:143` / `adapter_name()` `:149`。含 `is_software` 标记（`DeviceType::Cpu`）。
- `RenderPipeline::new(backend: GraphicsBackend, width: u32, height: u32) -> Result<Self, PipelineError>` — `pipeline.rs:248` — `render` — **建离屏管线**。尺寸校验在建 device 之前；用 `InstanceDescriptor::new_without_display_handle()`（**无 surface**）+ `PowerPreference::LowPower` + `force_fallback_adapter: false`（允许软件光栅）。尺寸上限取**本机 adapter 的 `max_texture_dimension_2d`**。
- `RenderPipeline` 访问器：`width()` `:306` / `height()` `:312` / `backend()` `:318` / `device() -> &wgpu::Device` `:324` / `queue() -> &wgpu::Queue` `:330` — `render` — **`device()`/`queue()` 是公开的**，外壳可据此把 reader 的纹理与 egui 的渲染器共享（进阶路径）。
- `PipelineError` — `pipeline.rs:42` — `render` — `capability_id() -> Option<&'static str>` `:104` / `to_reader_error() -> ReaderError` `:113`。变体含 `NoAdapter { reason }`、`DeviceRequest { reason }`、`InvalidExtent { width, height, detail }`。
- `RenderBatch::for_page(page: &Page, &GeometryConfig, &Viewport, theme: Theme) -> Self` — `crates/tome-reader/src/render/batch.rs:276` — `render` — **翻页形态取第 N 页**（只消费分页层的页边界，**不重算**）。
- `RenderBatch::for_line_slice(lines: &[PageLine], &GeometryConfig, &Viewport, theme: Theme) -> Self` — `batch.rs:309` — `render` — **滚动视口**（零拷贝直入 `ScrollFlow::viewport_lines` 的返回值）。
- `RenderBatch::for_chapter(&ChapterLayout, &GeometryConfig, &Viewport, theme) -> Self` — `batch.rs:240` — `render` — 章首屏（一般不用，滚动用 `for_line_slice`）。
- `RenderBatch::for_lines(&[&PageLine], ...)` — `batch.rs:291`；`background_only(width, height, theme)` `:387`。
- `RenderBatch` 访问器：`background()` `:406` / `text_color()` `:412` / `glyph_count()` `:418` / `glyphs()` `:424` / `decoration_count()` `:430` / `image_count()` `:442` / `instructions()` `:454` / `push_decoration(char_start, char_len, kind)` `:459` / `push_image(placement)` `:504` / `encode(&self, &RenderPipeline, &mut GlyphCache) -> Vec<String>` `:537`。
  - **批次自带颜色**（`text_color` + `Background`）——「一个批次 = 一次确定的绘制」，外壳**不要**再传主题给渲染层。
- `GlyphCache::new() -> Self` — `crates/tome-reader/src/render/glyph_cache.rs:266` — `render` — 字形栅格化缓存（**跨帧持有**）。`with_atlas_size(u32)` `:294` / `font_system_mut()` `:313` / `register_font_source` `:318` / `rasterize(...)` `:404` / `take_pending_uploads()` `:584` / `hit_count()` `:324` / `miss_count()` `:330`。
- `OffscreenRenderer::new(width: u32, height: u32) -> Result<Self, PipelineError>` — `crates/tome-reader/src/render/offscreen.rs:168` — `render` — 零尺寸在 wgpu 校验前先拒。`OffscreenRenderer` 是 `Copy`（只存尺寸）。
- **`OffscreenRenderer::render(&self, pipeline: &RenderPipeline, batch: &RenderBatch, cache: &mut GlyphCache) -> Result<RenderedImage, PipelineError>`** — `offscreen.rs:204` — `render` — **渲染出口**。内部：`batch.encode(...)` 预热缓存 → 建纹理 → 清底 → `draw_glyphs`（真实位图 → 图集 → WGSL 采样）→ 256 对齐读回 → 剥 padding。
- `RenderedImage { width: u32, height: u32, pixels: Vec<u8> }` — `offscreen.rs:41` — `render` — **紧凑 RGBA8**（每行无 padding，长度 = `width × height × 4`）。判据方法：`non_black_pixels()` `:62` / `foreground_pixels(background: [u8;3])` `:79` / `foreground_coverage(background)` `:92` / `total_pixels()` `:101` / `coverage()` `:107` / `is_consistent()` `:116`。
  - `OFFSCREEN_FORMAT = TextureFormat::Rgba8Unorm` — `offscreen.rs:37` — **非 sRGB**。像素即最终显示值，**不要再做 sRGB 转换**。
  - 辅助：`aligned_bytes_per_row(width, bpp) -> u32` `:126`（wgpu 256 字节对齐）；`strip_row_padding(padded, width, bpp, bpr) -> (Vec<u8>, u32)` `:136`。
- `readback_failure_error(&PipelineError) -> ReaderError` — `offscreen.rs:690` — `render` — 读回失败 → 接缝错误。
- 动效：`PageTurn::for_theme(theme: Theme) -> Self` — `crates/tome-reader/src/render/animate.rs:127`；`duration_ms()` `:140` / `easing()` `:149` / `easing_for(&Animation)` `:159` / `scroll_easing()` `:176` / `preference()` `:186` / `is_instant()` `:192` / `with_preference(MotionPreference)` `:198` / `progress_at(elapsed_ms) -> f32` `:214`。`MotionPreference` `:79`、`ReducedMotion::from_flag(bool)` `:102`、`EasingKind::control_points(theme)` `:50`。**纯值，无图形依赖**。
- 图文混排：`ImageLayout::place(...)` `crates/tome-reader/src/render/image.rs:287` / `page_containing(&ImageBox) -> Option<ImagePage>` `:422` / `covered_char_count(&ChapterLayout)` `:431` / `all_placeholder()` `:451`；`ImageBox::with_intrinsic_size(...)` `:191` / `placeholder(char_offset, available_width_px, font_size_px)` `:224` / `is_unavailable()` `:246`；`ImageSizes` `:84` / `ImageStatus` `:65`。
- 字形图集（进阶）：`pack_glyphs(&mut GlyphCache, &[GlyphPlacement]) -> PackedAtlas` — `crates/tome-reader/src/render/glyph_atlas.rs:195`；`GLYPH_SHADER` `:60`；`estimate_atlas_size(glyph_count, font_size) -> u32` `:353`。
- 像素比对（**外壳不需要**，差分测试用）：`compare_rgba(&[u8], &[u8], threshold) -> DiffReport` — `crates/tome-reader/src/render/pixel_diff.rs:190`。

### 5.11 字体（共享字体栈）

- `font_setup::embedded_font_bytes() -> &'static [u8]` — `crates/tome-reader/src/font_setup.rs:65` — `—` — 内嵌正文衬线（`EMBEDDED_FONT` = `include_bytes!` 的 `assets/fonts/TomeSerifCN-subset.otf`，`:61`）。
- `font_setup::family_name() -> &'static str` — `:71`；`READING_SERIF_FAMILY = "Tome Serif CN"` `:43`；`reading_family() -> cosmic_text::Family<'static>` `:237`。
- `font_setup::build_embedded_first_font_system() -> Result<..., FontSetupError>` — `:120` — **唯一装载点**（排版与渲染共用；feature 无关）。
- `font_setup::shared_font_system() -> Option<&'static Mutex<SharedFontStack>>` — `:186` — 进程级共享栈。`SharedFontStack::load()` `:202` / `font_system_mut()` `:211` / `embedded_id()` `:217` / `face_count()` `:223`。`FontSetupError::to_reader_error()` `:90`。
- `render::fonts::FontRegistry::load() -> Result<Self, FontLoadError>` — `crates/tome-reader/src/render/fonts.rs:189` — `render` — 内嵌 + 平台回退链注册。`face_count()` `:201` / `embedded_id()` `:207` / `font_system_mut()` `:212` / `font_for_char(char)` `:224`。`shared_registry()` `:314`；`Platform::current()` `:84`；`fallback_chain_for(Platform)` `:113`；`FontsReport::collect()` `:344` / `to_json()` `:362`。`EMBEDDED_FONT_BUDGET_BYTES = 5 MiB` `:58` / `EMBEDDED_FONT_MIN_BYTES = 3 MiB` `:68`。

### 5.12 主题（值类型在 tome-core，实例由组合根注入）

- `Theme` — `crates/tome-core/src/model/theme.rs:935` — `—` — **`Copy` 纯值**。访问器：`name()` `:955`（`"parchment"` / `"obsidian"`，**稳定标识非文案**）、`palette()` `:961`、`fonts()` `:967`、`type_scale()` `:973`、`spacing()` `:979`、`radii()` `:985`、`shell()` `:991`、`shadows()` `:997`、`texture()` `:1003`、`motion()` `:1009`、`reading()` `:1015`、`reading_toolbar()` `:1021`、`swatches()` `:1030`、`is_night()` `:1040`。
- `DayTheme.theme() -> Theme` — `theme.rs:1131`（`name = "parchment"`）；`NightTheme.theme() -> Theme` `:1225`（`name = "obsidian"`）。`From<DayTheme>` `:1243` / `From<NightTheme>` `:1249`。
- 外壳会用到的子结构：`Palette` `:127`（`bg_reading` 是阅读区底色）；`ReadingMetrics` `:790` + `measure_px()` `:818` / `measure_max_px()` `:824` / `two_page_threshold_px()` `:836` / `enables_two_page(available_width_px)` `:842` / `with_font_size(f32)` `:850`；`ReadingToolbar` `:891`（`hot_zone_height_px` / `idle_fade_after_ms` / `fade_duration_ms` / `entries: [&str; 10]`）；`Motion` `:739` + `durations()` `:755`；`DurationTicks::effective(reduced_motion: bool)` `:728`；`ShellDims` `:529`（`topbar_h_px` / `sidebar_w_px` / `sidebar_w_collapsed_px` / `control_h_px`）；`TypeScale` `:368` + `steps()` `:390`；`SpacingScale` `:458` + `scale()` `:479`；`Radii` `:505`；`Shadows` `:565`；`Texture` `:626`；`Swatches` `:665`；`Fonts` `:281` + `for_role(FontRole)` `:293`；`FontRole::ALL` `:267`；`Weight::value()` `:243`。
- 常量：`PARCHMENT` `:171` / `OBSIDIAN` `:195` / `FONTS` `:314` / `TYPE_SCALE` `:405` / `SPACING` `:491` / `RADII` `:519` / `SHELL` `:541` / `SHADOWS_DAY` `:575` / `SHADOWS_NIGHT` `:600` / `TEXTURE_DAY` `:643` / `TEXTURE_NIGHT` `:650` / `SWATCHES` `:685` / `MOTION` `:765` / `READING` `:871` / `READING_TOOLBAR` `:903`。
- `ReadingPalette::for_theme(theme: &Theme) -> Self` — `crates/tome-reader/src/theme.rs:99` — `—` — 阅读区专用的对比度安全色。`relative_luminance(Color) -> f64` `:143` / `contrast_ratio(Color, Color) -> f64` `:155` / `thresholds::{AA_BODY_TEXT: 4.5, AA_LARGE_TEXT_OR_UI: 3.0}` `:173-177`。**外壳若自绘正文外的 UI 元素，应用它校验对比度**。

---

## 6. 错误与能力

### 6.1 分类与标识

- `enum ErrorKind` — `crates/tome-core/src/error.rs:88` — `—` — **七类**：`Network` / `Proxy`（FR-008 要求与 Network 可区分）/ `Rule` / `UnsupportedCapability { capability: ErrorCapability }` / `Content` / `Storage` / `Configuration`。
  - `message() -> LocalizedMessage` `:137`；`message_id() -> &'static str` `:171`（`error.network` / `error.proxy` / `error.rule` / `error.unsupported_capability` / `error.content` / `error.storage` / `error.configuration`）；`unsupported_capability(ErrorCapability) -> Self` `:155`；`with_detail(detail) -> ClassifiedError` `:163`。
- `struct LocalizedMessage` — `error.rs:31` — `new(id)` `:37` / `id()` `:42`。**点分小写 ASCII 路径**。
- `struct ErrorCapability` — `error.rs:59` — `new(capability_id)` `:65` / `capability_id()` `:70`。用**书源生态的通用叫法**（驼峰，如 `webView`）。
- `struct ClassifiedError` — `error.rs:130` — `—` — `kind() -> &ErrorKind` `:186` / `detail() -> Option<&str>` `:191` / `message_id()` `:196`。`Display` 输出 `error.storage: <细节>` `:205`。**不在 crate 根重导出**（用 `tome_core::error::ClassifiedError`）。

### 6.2 能力错误的产生点与呈现约定

**产生点（外壳会遇到的）**

| 来源 | 标识 | 位置 |
|---|---|---|
| `render` feature 关闭 / 图形后端不可用 | `"offscreenRendering"` | `crates/tome-reader/src/lib.rs:86`（`capability::OFFSCREEN_RENDERING`） |
| 内嵌子集缺失且系统回退链也取不到字体 | `"fontUnavailable"` | `crates/tome-reader/src/lib.rs:89`（`capability::FONT_UNAVAILABLE`） |
| `--chapter <book>#<n>` 需书架接线 | `"chapter-by-bookshelf"` | `crates/tome-cli/src/reader_commands.rs:78`（**CLI 自造**，S3 接线后即消失） |
| 书源脚本要 WebView / 登录 / 音频等 | 由 `tome-js` 的 `register_all_unsupported` 抛 | `crates/tome-js/src/lib.rs:57`；`tome_js::api::UnsupportedCapability` |
| 脚本文件访问越界 | `UnsupportedCapability` | `crates/tome-core/src/seam/host.rs:299` |

**呈现约定（红线）**

- 引擎**只产标识，不产自然语言**（FR-044 / SC-016，`AGENTS.md` 生产级护栏）。
- `ReaderError` — `crates/tome-reader/src/error.rs:72` — `kind() -> ErrorKind` `:215` / `message_id() -> &'static str` `:238` / `detail() -> Option<&str>` `:254` / `capability_id() -> Option<&str>` `:267` / `to_classified() -> ClassifiedError` `:277`。构造：`text_model` `:132` / `layout_failed` `:140` / `invalid_geometry(param, detail)` `:148` / `font_unavailable` `:157` / `unsupported_capability(&'static str)` `:167` / `with_detail` `:176` / `underlying(ErrorKind, detail)` `:191`。`ReaderErrorKind(pub ErrorKind)` `:311`。
- `ReaderConfigError::message_id()` — `paginate/config.rs:514` — `error.reader.read_config_parse` / `..._serialize` / `..._invalid_field`；`field()` `:524`。
- `PlaceholderKind::message_id()` — `scroll/placeholder.rs:59`；`DictFailure::as_str()` — `trace/dict.rs:149`、`guidance()` `:172`；`ReadingAction::as_str()` — `keys.rs:120`；`Theme::name()` — `theme.rs:955`；`ThemeSet::by_id` 的可用标识 `["day","night"]` — `root.rs:156`。
- **参考文案表**：`tome_cli::i18n` — `crates/tome-cli/src/i18n.rs`。`Language { Zh, En }` `:66`（`code()` `:80`）；`message(kind: &ErrorKind, language: Language) -> String` `:154`；`render(id: &str, language: Language) -> String` `:177`；`hint(id: &str, language: Language) -> String` `:242`；`all_error_kinds() -> Vec<ErrorKind>` `:129`；`capability_message(language)` `:228`。
  - ⚠️ **它在 `tome-cli` 里，外壳不能依赖**。外壳必须自建等价的文案表（或把这张表下沉到 `tome-core`——那会违反 core「不放工具函数」的窄接口约束，需 PM 裁决）。测试口径（`i18n.rs` 的 `mod tests`）值得照抄：**每个 `ErrorKind` × 两种语言都有非空且互不相同的文案，且文案里不含 `error.` 标识**。

---

## 7. 特征门控

### 7.1 tome-reader 的 feature 组合

- `tome-reader` 的 `[features]` — `crates/tome-reader/Cargo.toml` 尾部 — `default = ["render"]`；`render = ["dep:wgpu", "dep:swash", "dep:etagere", "cosmic-text/swash"]`。
- **根 `Cargo.toml` 的 `[workspace.dependencies]` 把 `tome-reader` 声明为 `default-features = false`** — `Cargo.toml:33`：`tome-reader = { path = "crates/tome-reader", default-features = false }`。
  - ⚠️ **这是外壳最容易踩的坑**：成员 crate 写 `tome-reader = { workspace = true }` 时，**workspace 表里的 `default-features = false` 生效**，`render` 面**默认是关的**。CLI 的注释（`crates/tome-cli/Cargo.toml:23-25`）明确记录了 2026-10-11 CI 依赖图断言实测抓出的这个形态。外壳必须显式开启。
- **CLI 的做法（外壳应照抄）**：`tome-reader = { workspace = true }` + 自己的 feature 转发 —
  ```toml
  [features]
  default = ["reader-render"]
  reader-render = ["tome-reader/render"]
  ```
  好处：`cargo build -p tome-app --no-default-features` 能产出一个**渲染面在编译期不存在**的构建，与 CLI 的 headless 形态一致。
- 关闭 `render` 时的行为约定：调用渲染入口应返回 `UnsupportedCapability{capability:"offscreenRendering"}` + CLI 退出码 3（`crates/tome-cli/Cargo.toml:19-25` 的注释与 `contracts/cli.md` §2.3）。外壳对应「显示一个结构化错误页，不崩溃」。
- **headless 断言对 tome-app 的影响**：
  - `scripts/check-reader-features.sh` 的两条检查都作用在 `cargo tree -p tome-reader` 与 `cargo build -p tome-reader`（脚本 `main()` 里 `readonly CRATE="tome-reader"`），**包作用域**——新增 `tome-app` **不会**让这个守卫变红。
  - 但 `AGENTS.md` 记录的双形态测试（`cargo test --workspace` 3110 passed / `--no-default-features` 2958 passed）是 workspace 级的。若 `tome-app` 的 `default` 里带了渲染面而**代码里无条件 `use tome_reader::render::*`**，`cargo test --workspace --no-default-features` 会**编译失败**。故外壳必须像 CLI 一样：把渲染路径 `#[cfg(feature = "reader-render")]` 门控，并在关闭时给出结构化能力错误。
  - 守卫的图形包正则（脚本 `GRAPHICS_PATTERN`）含 `winit|raw-window-handle|wgpu|naga|glow|glutin|...`。`eframe` 必然拉进 `winit` + `raw-window-handle`——**这正是 `tome-app` 必须独立于 tome-reader 门控断言的原因**；不要去扩展那个脚本的 `-p` 范围。
- **`crates/tome-cli/tests/dependency_graph.rs` 的断言范围**：`ENGINE_CRATES` 数组（`:146-155`）**不含 `tome-cli`**，注释明确「组合根按定义要装配全部实现 crate」。**S3 落地时必须把 `tome-app` 同样排除**（它是第二个组合根），否则 `实现_crate_不依赖上层` 那条会把 `tome-app → tome-reader` 判为违规。

### 7.2 无门控的模块（外壳可直接用，任何构建都有）

`text` / `rules` / `layout` / `paginate` / `scroll` / `precalc` / `record` / `trace` / `toolbar` / `footer` / `keys` / `theme` / `error` / `font_setup` — `crates/tome-reader/src/lib.rs:54-69`。`render` 是唯一 `#[cfg(feature = "render")]` 的模块（`:71-72`）。

---

## 8. 现成的 UI 构件线索

**结论：`toolbar.rs` / `footer.rs` / `keys.rs` / `trace/` 全部是可复用的纯逻辑，不是 CLI 契约测试专用。** 三条证据：① 它们**不在** `#[cfg(feature = "render")]` 下（`lib.rs:54-69` 无条件）；② 它们的输入输出全是值类型（无图形句柄、无时钟、无 egui 类型）；③ 模块文档明说「本类型是纯值，可被 `Copy`/比较/跨帧持有」（`toolbar.rs:207`）。CLI 只是把它们当作契约测试的靶子（`crates/tome-cli/tests/` 下有 `minimal_loop.rs` 等），并非唯一消费者。

### 8.1 阅读工具栏（`crates/tome-reader/src/toolbar.rs`）

- `ToolbarVisibility { Hidden, Visible, FadingOut }` — `toolbar.rs:47` — 三态；`FadingOut` 折进 `Visible` 会让「活动取消淡出」无处挂载。
- `ToolbarInput { PointerInHotZone, PointerLeftReadingArea, RevealRequested, HideRequested }` — `toolbar.rs:58` — **输入事件枚举**（外壳把 egui 事件翻译成它）。
- `ToolbarBounds` — `toolbar.rs:77` — 几何。`TOOLBAR_HEIGHT_PX = 44.0` `:98`；`for_window(window_width_px, window_height_px, measure_px, hot_zone_height_px) -> Self` `:108`；`for_window_with_theme(window_width_px, window_height_px, measure_px, theme: &Theme) -> Self` `:125`（**推荐用这个**，走主题注入）；`contains_in_hot_zone(x_px, y_px) -> bool` `:147`（半开区间，`y == height` 判给正文）。
- `TOOLBAR_ENTRIES: &[&str]` — `toolbar.rs:171` — **十项稳定标识**：`back` / `toc` / `progress` / `font_size` / `line_height` / `theme` / `spread` / `bookmark` / `change_source` / `settings`。**外壳的工具栏按钮即按此表布局**，文案由外壳查表（引擎不产自然语言）。与 `Theme::reading_toolbar().entries`（`theme.rs:899`）同源。
- `ReadingToolbar` — `toolbar.rs:200` — `new()` `:210` / `set_bounds(ToolbarBounds)` `:220` / `bounds()` `:226` / `visibility()` `:232` / `is_visible()` `:238` / `entries()` `:244` / `opacity() -> f32` `:261`（**每帧取它做淡出**）/ `apply(input: ToolbarInput)` `:282`（状态迁移）/ `advance(elapsed_ms: u32)` `:315`（**外壳每帧传 delta**）。「本类型是纯值，可被 `Copy`/比较/跨帧持有」见 `toolbar.rs:198`。`#[derive(Default)]` `:339`。

### 8.2 页脚（`crates/tome-reader/src/footer.rs`）

- `FooterInput` — `footer.rs:63` — `chapter_index: usize`（0 起）/ `chapter_title: String` / `chapter_char_len: usize` / `chapter_char_offset: usize` / `page_in_chapter: u32` / `page_count_in_chapter: u32` / `recent_records: Vec<ReadRecord>`。
- `FooterInfo::build(input: &FooterInput) -> Self` — `footer.rs:111` — **纯计算、不返回 `Result`**（所有输入都有确定处置：钳制/隐藏）。
- `FooterInfo` — `footer.rs:87` — `chapter_index`（内部口径）/ `display_chapter_number`（**1 起，已 +1**）/ `chapter_title` / `display_page_number`（1 起）/ `page_count_in_chapter` / `remaining_seconds: Option<f64>`（`None` = **隐藏该项**，数据不足）。`remaining_minutes() -> Option<u64>` `:134`（四舍五入；秒级是假精度）。`MIN_SECONDS_FOR_SPEED = 30` `:58`。

### 8.3 键盘（`crates/tome-reader/src/keys.rs`）

- `KeyEvent` — `keys.rs:43` — **平台无关的裸键枚举**：`ArrowUp/Down/Left/Right`、`PageUp/PageDown`、`Home/End`、`Space`、`Backspace`、`Tab`、`Escape`、`Character(char)`。⚠️ **不建模修饰键**（FR-055 三平台一致性的要求）。**外壳要做 egui `Key` → `KeyEvent` 的翻译表**。
- `ReadingAction` — `keys.rs:74` — **12 个动作**：`PageNext/PagePrev`、`ChapterNext/ChapterPrev`、`ChapterStart/ChapterEnd`、`ScrollLineDown/ScrollLineUp`、`ScrollPageDown/ScrollPageUp`、`RevealToolbar`、`Esc`。`ALL: [Self; 12]` `:103`；`as_str()` `:120`（驼峰稳定标识）。
- `KeyBinding { key, action }` — `keys.rs:146`；`KEYMAP: &[KeyBinding]` `:180`（**一键一动作，表与 `ALL` 一一对应**）；`keymap() -> &'static [KeyBinding]` `:239`；`resolve_key(key: KeyEvent) -> Option<ReadingAction>` `:251`（**纯函数**）。
- ⚠️ **页滚动不复用 `PageUp`/`PageDown`**：`PageDown` 恒为 `PageNext`、`Space` 恒为 `ScrollPageDown`。**外壳按当前形态只处理相应那组动作**（翻页模式忽略滚动动作，反之亦然）——`keys.rs:169-180` 的文档写明了这个取舍的理由。
- `EscLayer { TocDrawer, SettingsModal, BookmarkPanel, ChangeSourcePanel, ProgressPanel, Immersion }` — `keys.rs:271`。**这六个变体正好枚举了外壳的抽屉/弹层**（S3 的目录抽屉 + 书签面板 + 换源面板 + 设置 + 进度）。
- `EscStack` — `keys.rs:302` — `new()` `:316` / `push(EscLayer)` `:323`（**同层去重**）/ `pop()` `:330` / `handle_escape() -> Option<EscLayer>` `:342`（非空弹栈顶，空则返回 `Immersion` 且栈不变）/ `top()` `:351` / `depth()` `:357` / `is_empty()` `:363` / `layers()` `:369` / `layer_set() -> BTreeSet<EscLayer>` `:375`。常量 `TOC_DRAWER` `:308` / `SETTINGS_MODAL` `:310` / `BOOKMARK_PANEL` `:312`。**外壳的 Esc 处理直接委托给它**。

### 8.4 不适合直接复用的

- `corpus.rs` / `corpus_eval.rs` — 排版合成语料装载与差分（`verify-layout` 用），**外壳不需要**。
- `render/pixel_diff.rs` — 像素快照比对（CI 用），**外壳不需要**。
- `tome_cli::i18n` / `tome_cli::png` / `tome_cli::root::ThemeSet` / `tome_cli::pipeline_commands::DaoSourceLookup` / `tome_cli::js_host::build_source_engine` — **在 tome-cli 里**，逻辑可照抄但类型不可引用（依赖方向禁止）。

---

## 附录 A：tome-app 的依赖声明草案

```toml
[package]
name = "tome-app"
description = "tome 应用外壳：egui/eframe 桌面窗口、书架视图与阅读视图"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[dependencies]
# --- 接缝与 DTO（唯一允许的引擎依赖面，铁律）---
tome-core = { workspace = true }

# --- 组合根：外壳是第二个组合根，允许依赖全部实现 crate ---
# 与 tome-cli 同一授权（crates/tome-cli/tests/dependency_graph.rs 的 ENGINE_CRATES
# 不含组合根）。**S3 落地时必须把 "tome-app" 加入该测试的排除集**，否则
# `实现_crate_不依赖上层` 那条会把 tome-app → tome-reader 判为违规。
tome-net = { workspace = true }
tome-store = { workspace = true }
tome-js = { workspace = true }        # 仅当外壳自建 JS 引擎工厂时需要（见 §4.7）
tome-rule = { workspace = true }      # 仅当需要直接构造 ContentPipeline / Cleaner 时需要
tome-source = { workspace = true }
tome-local = { workspace = true }

# --- 排版渲染层 ---
# ⚠️ 根 Cargo.toml 已把 tome-reader 声明为 default-features = false，
# 故 `render` 面**默认关闭**，必须经下面的 feature 转发显式开启
# （形态与 tome-cli 一致；见 crates/tome-cli/Cargo.toml:19-26 的注释）。
tome-reader = { workspace = true }

# --- GUI 栈（全新依赖，Cargo.lock 里当前 0 个 egui/eframe 条目）---
# 版本取自同批次的 specs/003-app-shell/research-eframe.md（钉 0.36.2，2026-09-08 最新稳定）。
# 选它的硬理由：egui-wgpu 0.36.2 依赖 wgpu "30.0"，与 tome-reader 钉的 wgpu 30.0
# **semver 相容**（Cargo.lock 实解 30.0.1）——cargo 会合并为单份 wgpu 构建，
# 这是 §B.3 第 10 条「共享 device」那条进阶路径的前提。
eframe = { version = "0.36.2", default-features = false, features = ["wgpu", "wayland", "x11"] }
egui = "0.36.2"
# 拖放导入本地书（P1 的「拖入 TXT/EPUB」）——eframe 的 `dropped_files` 已够用，
# 若需文件选择对话框/更细的 MIME 语义再引入（版本须在 plan 阶段查证，勿照抄此行）

# 外壳需要序列化（书源导入报告、设置文件、read_config 的 JSON 透传）
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# 单实例（spec 边缘情况：「第二个实例 MUST 激活已有实例」）
single-instance = "0.3"   # 或等价方案；需在 plan 阶段定版

[dev-dependencies]
tempfile = "3"

[features]
# 默认带渲染面（与 tome-cli 同形）。
#
# headless 形态 `cargo build -p tome-app --no-default-features` 必须**编译通过**
# 且渲染入口返回 UnsupportedCapability{capability:"offscreenRendering"}
# ——因为 `cargo test --workspace --no-default-features` 是既有的 workspace 级门。
default = ["reader-render"]
reader-render = ["tome-reader/render"]

[lints]
workspace = true
```

**声明要点与陷阱**

1. **`tome-reader` 的 `render` 必须经 feature 转发开启**（`reader-render = ["tome-reader/render"]`），不能靠 `default`。写成 `tome-reader = { workspace = true, features = ["render"] }` 也能编译，但那样 `--no-default-features` **不会**关掉渲染面，破坏 headless 双形态门。**照抄 CLI 的写法**。
2. **不要给 `tome-reader` 加 `default-features = true`**：workspace 表已固定为 `false`，成员点的 `default-features` 会被 cargo 忽略（`crates/tome-cli/Cargo.toml:23-25` 记录了这个实测）。
3. `tome-js` / `tome-rule` 只有在**外壳自建 JS 引擎工厂**（`SourceJsEngine::for_source` 的注入）与**自建 `Cleaner`**（`ContentPipeline`）时才需要。若 plan 阶段决定把这两件事下沉到新 crate（见待解缺口 §B/§C），可以删掉这两行。
4. **`eframe` 的 feature 选择**：`wgpu` 后端而非 `glow`——因为 `tome-reader` 的离屏路径也是 wgpu，同栈便于后续共享 device（`RenderPipeline::device()`/`queue()` 是公开的）。`default-features = false` 以避免拉进不需要的 `accesskit`/`persistence` 等。版本与 feature 细节以 `specs/003-app-shell/research-eframe.md` 的钉版结论为准（egui-wgpu 0.36.2 → wgpu "30.0"，与 tome-reader 的 wgpu 30.0 semver 相容）。
5. **体积纪律（铁律 2）**：`eframe` + `winit` + `wgpu` 会显著推高二进制体积。release profile 已有 `lto = "fat"` + `codegen-units = 1` + `strip` + `panic = "abort"`（根 `Cargo.toml`）。**S3 落地后必须出一份 `size-report.md`**（002 有先例：`specs/002-typesetting-renderer/size-report.md`），并核 `<20MB` 基线。
6. **`panic = "abort"` 与 GUI 兜底**：release 下 panic 即进程退出，无法 `catch_unwind`。spec 边缘情况要求「引擎 panic 经外壳兜底捕获」——**该要求在 release profile 下不可实现**，需在 plan 阶段裁决（选项：给 tome-app 单独关掉 `panic = "abort"`，或把兜底改述为「单章失败隔离在错误态」而非 panic 捕获）。
7. **`unsafe_code = "warn"`** 与 `tome-store` 的 `#![deny(unsafe_code)]` 先例：外壳若要用 Win32 单实例 API，需按同样形态写 `#![deny(unsafe_code)]` + 单点 `#[expect(unsafe_code)]` + SAFETY 注释。

---

## 附录 B：外壳与引擎的线程 / 调用模型建议

### B.1 现有代码里的既定模式（全部同步，无 async 接缝）

| 位置 | 模式 | 证据 |
|---|---|---|
| `tome-store::Db` | **单写者 actor 线程** + 同步请求-应答。`Db::open` 起一条 `tome-store-db` 线程；`Db::call(op) -> Result<T, StoreError>` 用 `mpsc` 发闭包、**阻塞等结果** | `crates/tome-store/src/db.rs:126`（spawn）、`:183`（call）、`:202`（write 别名）、`:214`（read 走**线程局部只读连接**，绕过 actor 队列，WAL 下读写并发） |
| `tome-net::RequestExecutor` | **自持 tokio runtime + `block_on`**，对外仍是同步 `NetHost::fetch` | `crates/tome-net/src/executor.rs:479`、`:813` |
| `tome-source::search_stream` | **共享队列 + N worker 线程**（`std::thread`），结果经 `mpsc` 回主线程，按 source_id 排序产出 | `crates/tome-source/src/search_stream.rs:274` |
| `tome-reader` | **纯同步 CPU 计算**，无内部线程 | 全 crate 无 `thread::spawn` |
| `BookRepository` 接缝 | 方法全同步；注释明确「上层的『不阻塞』由**上层自己的线程**保证（GUI 把调用放到工作线程）」 | `crates/tome-store/src/db.rs:183` 的 doc |

**结论：接缝层没有任何 async。** 阻塞隔离是**调用方的责任**——这条已在 `Db::call` 的文档里被显式写成契约。

### B.2 egui 主线程的约束

- `eframe::App::update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame)` 跑在**主线程**且每帧必须返回。任何阻塞调用（`Db::call`、`NetHost::fetch`、`Paginator::paginate`、`OffscreenRenderer::render`）在主线程上都会**冻结 UI**。
- `Paginator` 持 `FontSystem`（装载系统字体库，**实测耗时可观**，`page.rs:347` 的注释），且所有方法要 `&mut self` → **不能** `Arc<Mutex<Paginator>>` 每帧锁（会与 UI 线程争锁）。**必须给它一个专属线程**。
- `GlyphCache` 与 `RenderPipeline` 也是长生命周期 + `&mut`/`&` 混合使用，应与 `Paginator` 同线程（`OffscreenRenderer::render` 一次要吃 `&RenderPipeline` + `&RenderBatch` + `&mut GlyphCache`）。

### B.3 建议的模型：一条「引擎线程」+ 双向 channel

```
┌─────────────────────────┐         ┌──────────────────────────────────────┐
│  egui 主线程 (eframe)   │         │  引擎线程 (std::thread, 常驻)         │
│                         │         │                                      │
│  ui state (视图/焦点/   │  Cmd →  │  Paginator (唯一所有权)               │
│  锚点/主题/参数)        │ ──────► │  GlyphCache / RenderPipeline          │
│                         │         │  ContentProvider (Arc, 见下)          │
│  ←  Evt (结果/进度)     │ ◄────── │  PaginationResult / ScrollFlow 持有    │
│                         │         │  ReadingRecordAccumulator             │
│  ctx.request_repaint()  │         │                                      │
└─────────────────────────┘         └──────────────────────────────────────┘
                                                    │ 经 Arc<dyn ...> 调
                                                    ▼
                                    Db actor 线程 / tokio(net) / 搜索 worker 线程
```

**要点**

1. **用 `std::sync::mpsc`**（仓库既有惯例：`db.rs`、`search_stream.rs` 都是）。不要引 `crossbeam` / `flume`。
2. **`Cmd` / `Evt` 用枚举**，一次交互一条。命令侧：`LoadBook{book_id}`、`Paginate{geometry, viewport}`、`TurnPage{delta}`、`ScrollTo{anchor}`、`Relayout{geometry, viewport}`、`SwitchMode{to}`、`RenderCurrent`、`Cancel`。事件侧：`Loaded{chapter_content}`、`Paginated{result}`、`Rendered{image}`、`Failed{kind, detail}`、`Progress{fraction}`、`SearchBatch{items}`。
3. **`ctx.request_repaint()` 由引擎线程在产出事件后调用**（`egui::Context` 是 `Clone + Send`），否则 UI 不会在等待期间刷新——这是 GUI 从「阻塞式 CLI」迁移到「异步流水线」最容易漏的一步。
4. **`Arc<dyn ContentProvider>` 可跨线程**：`NetworkContentProvider` 内部用 `Mutex<HashMap>` 做邻域暂存（`crates/tome-source/src/provider.rs:71`），`LocalSource` 用 `Mutex<Option<Arc<Vec<u8>>>>` 缓存当前章（`crates/tome-local/src/provider.rs:108`）——两者因此都是 `Sync`。可在引擎线程独占持有或 `Arc` 共享。**推荐：引擎线程独占持有**（省掉每次调用的锁）。
5. **取消**：接缝层已有 `CancelToken`（`seam/stream.rs:26`）——搜索与预排都吃它。**改字号触发的重排也要能取消**（`incremental_reflow` 吃 `token`，`precalc.rs:325`）。外壳应在 UI 上暴露「取消」（spec 边缘情况：「加载期间可取消返回书架」）。
6. **`Db::call` 与 UI 的交互**：书架列表、进度读写这类短操作可以直接在主线程调（单次 SQLite 查询是微秒级），但**首次 `Db::open`（含迁移）与 `SourceStore::merge` 必须放引擎线程或启动线程**。搜索/正文加载**必须**离开主线程。
7. **搜索用 `parallel_search`**（`search_stream.rs:221`），它的 `max_workers` 参数由外壳给（建议 `min(源数, 4~8)`）；它内部已起线程，外壳**不要**再包一层线程——但要包一层「把 `SearchStream` 的 `next_item` 拉取循环搬到引擎线程」的驱动，因为 `next_item` 会阻塞到下一个源完成。
8. **渲染的每帧预算**：`OffscreenRenderer::render` 是**同步阻塞**的（含 GPU 提交与读回）。若每帧都调，帧率会被读回拖住。建议：**只在内容/几何变化时重渲**（缓存 `RenderedImage` 或 `egui::TextureHandle`），滚动时用 `ScrollEventThrottle` 合并事件后再重渲。
9. **`RenderedImage` → egui 的转换**：
   ```rust
   let color_image = egui::ColorImage::from_rgba_unmultiplied(
       [image.width as usize, image.height as usize], &image.pixels);
   let handle = ctx.load_texture("page", color_image, egui::TextureOptions::LINEAR);
   ```
   `OFFSCREEN_FORMAT = Rgba8Unorm`（**非 sRGB**，`offscreen.rs:37`），像素即最终显示值——**不要再做 sRGB→linear 转换**。`pixels` 是**行紧凑**的（`strip_row_padding` 已剥掉 256 对齐 padding），长度必须是 `w*h*4`（先用 `RenderedImage::is_consistent()` 断言，`offscreen.rs:116`）。
   `ctx.load_texture` 每帧会分配新纹理——**应缓存 `TextureHandle` 并在尺寸/内容不变时复用**（`handle.set(image, options)`）。
10. **进阶路径（若性能不足）**：`RenderPipeline::device()` / `queue()` 是公开的（`pipeline.rs:324/330`），可把 `tome-reader` 的纹理直接注册到 egui-wgpu 的 renderer，避免 CPU 读回。代价是耦合 wgpu 版本（`tome-reader` 用 wgpu 30，`eframe` 的 wgpu 版本必须一致）——**这是一条有风险的优化，建议 S3 先走读回路径，把它列为性能后备**。

### B.4 一处需要在 plan 阶段裁决的架构摩擦

`ContentProvider` 是唯一可用接缝，但**它的两个实现都在实现 crate 里**（`tome_source::provider::NetworkContentProvider`、`tome_local::provider::LocalContentProvider`）。外壳要构造它们，就必须 `use` 这两个具体类型 —— 这与「永不直接构造 tome-source/tome-store 内部类型」的铁律**有张力**（虽然 `NetworkContentProvider` 是 `pub` 且实现的是接缝 trait，可辩称它是「接缝的 adapter 而非内部类型」）。三条出路，请 PM 裁决：

- **(a) 外壳直接构造两个 adapter**，并用一个自己的 `enum BookContentProvider { Network(NetworkContentProvider), Local(LocalContentProvider) }` 分派 `impl ContentProvider`。最省事，但把「书从哪来」的判定（`book.origin == BookOrigin::Local`）写进外壳。
- **(b) 在 `tome-core` 或新 crate 里加一个 `ContentProviderRouter`**（按 `BookOrigin` 分派到两个 adapter）——但 adapter 的构造仍需网络/存储句柄，故组合根职责不变，只是把分派逻辑从外壳移出。
- **(c) 补 `BookRepository` 与 `SourceEngine` 的生产实现**（见下），让外壳只经接缝工作。最符合铁律，但工作量最大。

---

## 待解缺口（S3 必须先决的三项）

### §A. `BookRepository` 无生产实现

`tome-store` 有全部 DAO，但**没有** `impl tome_core::BookRepository`。证据：`grep -rn "impl BookRepository" crates/` 只命中 `crates/tome-core/tests/seam_contract.rs:635`（测试假体）。`crates/tome-store/src/lib.rs` 的模块文档自述「`BookRepository` 接缝的完整实现（备份导入导出、书内搜索）属后续切片 T087-T091」（`lib.rs:38-40`）。
**影响**：§2 的全部入口都是 DAO 直连，违反「只依赖接缝 trait」。**建议**：S3 前置一个薄适配层（`tome-store` 加 `impl BookRepository for SqliteBookRepository`，约 43 个方法的转发），或由 PM 明确豁免并记录。

### §B. `SourceEngine` 无生产实现

`tome-source` 有全部自由函数，但**没有** `impl tome_core::SourceEngine`。证据：同上 grep 只命中测试假体。且 `search` 的接缝签名返回 `Box<dyn SearchStream>` 而自由函数是 `SearchEngine::search` —— 适配需要包一层。
**另外**：`SourceEngine::search` 的接缝签名**没有** `js_factory` 参数，而 `SearchEngine::with_js` 需要它；生产实现必须自己持有一个 JS 工厂（组合根注入），且 `JsEngineFactory` 是 `Arc<dyn Fn + Send + Sync>` —— 可满足。
**影响**：§4 的全部入口是自由函数直连 + 外壳自建 JS 工厂。**建议**同 §A。

### §C. `i18n` 文案表在 `tome-cli` 里

`tome_cli::i18n::message/render/hint` 是 SC-016 的现成实现，但类型在组合根 crate 里，外壳不能依赖。**建议**：把纯映射函数（`Language` + `message` + `hint`，无 CLI 依赖）下沉到一个共享位置——`tome-core` 违反「窄接口、无工具函数」，故更合适的是**新 crate `tome-i18n`**，或由外壳自建（复制表 + 复制测试口径）。**这是 PM 裁决项。**

---

## 附：本文引用的关键文件清单（供复核）

```
crates/tome-core/src/{lib.rs, error.rs, seam/{mod,host,source,repository,stream}.rs, model/{mod,book,trace,theme,rule,source,auxiliary,asset}.rs}
crates/tome-net/src/{lib.rs, executor.rs, log.rs, client.rs, charset.rs, cookie.rs, proxy.rs, ratelimit.rs, replay.rs}
crates/tome-store/src/{lib.rs, db.rs, datadir.rs, config.rs, host.rs, source_store.rs, pack.rs, image_cache.rs, backup.rs, delete.rs, dao/{book,trace,read_record,source,misc}.rs}
crates/tome-js/src/{lib.rs, runtime.rs, bridge.rs, api/*}
crates/tome-source/src/{lib.rs, source.rs, search.rs, search_stream.rs, info.rs, toc.rs, content.rs, discover.rs, change_source.rs, check.rs, purify.rs, provider.rs, image.rs, js.rs, action.rs}
crates/tome-local/src/{lib.rs, format.rs, txt.rs, epub.rs, mobi/mod.rs, toc_rule.rs, split.rs, hash.rs, encoding.rs, library.rs, provider.rs, error.rs}
crates/tome-reader/src/{lib.rs, error.rs, font_setup.rs, theme.rs, precalc.rs, record.rs, toolbar.rs, footer.rs, keys.rs,
                      text/*, rules/*, layout/{engine,linebox,paragraph_mode,style}.rs,
                      paginate/{config,page,spread,booksem}.rs, scroll/{flow,map,placeholder,relayout,switch,throttle}.rs,
                      trace/{dict,hit,render}.rs, render/{mod,pipeline,offscreen,batch,glyph_cache,glyph_atlas,fonts,animate,image,atlas,pixel_diff}.rs}
crates/tome-cli/src/{lib.rs, root.rs, commands.rs, pipeline_commands.rs, reader_commands.rs, local.rs, i18n.rs, js_host.rs, png.rs, flags.rs}
crates/tome-cli/tests/dependency_graph.rs
scripts/check-reader-features.sh
Cargo.toml / crates/*/Cargo.toml
specs/001-pc-engine-core/contracts/seams.md
```
