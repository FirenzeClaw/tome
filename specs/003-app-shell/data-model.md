# S3 应用外壳 — 数据模型与应用状态机（data-model.md）

**Date**: 2026-10-11 | **Spec**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md)

## 1. 实体（spec Key Entities 的落地形态）

> 全部为**纯值类型**（serde 可序列化；UI 线程与引擎线程间经消息通道按值传递，零共享可变状态）。锚点类型复用 S1/S2 的 `(chapter_index, char_offset)` 契约，不在本层重定义。

### 1.1 ShelfEntry（书架条目）

| 字段 | 类型 | 语义 |
|---|---|---|
| `book_id` | String | 书籍标识（S1 存储主键；本地书 = 库根条目 id，网络书 = 书源 id + 书籍 URL 的规范化形式） |
| `origin` | `BookOrigin` | `Local { library_root }` / `Source { source_id, book_url }` |
| `title` / `author` | String | 展示用（导入时落定；网络书以详情页为准） |
| `cover` | `Option<CoverRef>` | 封面引用（本地路径或图片缓存键；无封面走 MASTER 占位形态） |
| `progress` | `ProgressSummary` | 锚点 + 百分比（百分比 = 已完成章数与章内位置的合成，纯展示用） |
| `last_read_at` | `Option<i64>` | 最近阅读时刻（排序键；从未打开为 None） |

**校验**：`title` 非空（空标题回退文件名/书名的 S1 语义）；`origin` 二选一判别联合，字段不混用。
**关系**：本地书 ↔ tome-store 库根条目（BookDao）；网络书 ↔ 书源注册表条目 + 书籍 URL。

### 1.2 ReadingProgress（阅读进度）

| 字段 | 类型 | 语义 |
|---|---|---|
| `book_id` | String | 所属书 |
| `anchor` | `(usize, usize)` | `(chapter_index, char_offset)`——S1/S2 锚点契约原样 |
| `updated_at` | i64 | 最后变动时刻 |
| `reading_params` | `ReadingParams` | 该书最近的阅读参数快照（FR-007b 的「跟随该书最近设置」载体） |

**ReadingParams**（值类型）：`font_size_px: f32` / `line_height: f32` / `mode: ReadingMode`（`Paginated | Scroll`）/ `page_layout: Single | Spread` / `theme: ThemeChoice`（`Day | Night | FollowSystem`）。
**状态流转**：`书首次打开 → 取全局默认`；`阅读中调整 → 更新本书快照（节流落盘）`；`全局默认修改 → 不回写已有快照的书`（FR-007b）。
**持久化**：经 tome-store（TraceDao/MiscDao 面），节流窗口值在任务阶段定（预期秒级；spec 假设节）。

### 1.3 UserSettings（用户设置）

| 字段 | 类型 | 语义 |
|---|---|---|
| `theme` | ThemeChoice | 昼 / 夜 / 跟随系统（OS 探测映射 Day/Night，R7） |
| `default_font_size_px` / `default_line_height` | f32 | 全局默认字号/行高 |
| `default_mode` | ReadingMode | 默认翻页/滚动 |
| `default_page_layout` | PageLayout | 默认单/双页 |

仅四组（spec FR-018），持久化于数据目录配置（S1 配置注入），重启生效。

### 1.4 SourceRef（书源引用）

| 字段 | 类型 | 语义 |
|---|---|---|
| `source_id` | String | 源标识 |
| `name` | String | 展示名 |
| `imported_at` | i64 | 导入时刻 |

外壳不持有源定义本体（S1 注册表持有）；启停/分组不存在（D1）。

### 1.5 SearchHit（搜索结果）

| 字段 | 类型 | 语义 |
|---|---|---|
| `source_id` | String | 命中来源 |
| `book_url` | String | 书籍 URL（详情/加架键） |
| `title` / `author` | String | 展示 |
| `cover` | Option<CoverRef> | 有则显示 |

按 `source_id` 分组呈现；搜索代际号（generation）使过期结果自动作废（FR-013 可取消）。

### 1.6 Bookmark（书签，G1 裁决进 S3）

| 字段 | 类型 | 语义 |
|---|---|---|
| `book_id` + `anchor` | — | 定位（与阅读进度同一锚点类型，FR-008d 同一路径） |
| `preview` | String | 位置预览文本（锚点处截取，定长） |
| `created_at` | i64 | 创建时刻 |

按书持久化（tome-store）；锚点失效按 S2 钳制语义定位最近有效字符（spec Edge Case）。

## 2. 应用状态机（视图层契约的模型基础）

```text
AppView
├── Shelf { filter_text, sort_key, scroll_offset, selected: Option<book_id>,
│           search: Option<SearchSession> }
├── Reading { book_id, anchor, params: ReadingParams,
│             content: ContentState, drawer: DrawerState }
└── overlays（正交叠加，不属于视图切换）：
    ├── SettingsOverlay { draft: UserSettings }
    ├── ImportDialog { kind: Local | SourceFile | SourceUrl | SourceClipboard }
    └── ErrorToast { error_id, detail, actions: [Retry | SwitchSource | Back] }

ContentState = Loading | Ready { rendered: RenderedChapter, content_version: u64 }
             | Failed { error: ClassifiedErrorView }
DrawerState  = Closed | Open { tab: Toc | Bookmarks }
SearchSession = { generation: u64, keyword: String,
                  groups: Vec<(SourceRef, Vec<SearchHit>)>, done: bool }
```

**不变量**（状态机层断言，headless 可测）：

1. `Shelf ⇄ Reading` 互斥切换，双向往返后 Shelf 的 `filter_text/scroll_offset` 与 Reading 的 `anchor/params` 不变（SC-010）。
2. `ContentState::Ready` 的 `content_version` 单调递增；纹理缓存以 version 失效（静态帧零重绘）。
3. 参数调整只改 `params`，`anchor` 不变（SC-006 锚点保持的状态机表达）；重排完成事件回来后 `content_version +1`。
4. `SearchSession.generation` 全局单调；UI 只呈现当前 generation 的结果（取消/新搜索使旧结果作废）。
5. 抽屉开合不改 `params` 与版心几何（FR-008 覆盖式）。
6. `ErrorToast.actions` 非空（每个错误必有可行动作，FR-010）。

## 3. 引擎消息协议（tome-app/src/engine/messages.rs）

```text
EngineRequest（UI → 引擎线程）
├── Shelf( Load | ImportLocal{path} | Remove{book_id, purge_cache} )
├── Reading( Open{book_id} | Close | SetParams{book_id, params}
│          | TurnPage{delta} | SeekAnchor{anchor} | LoadChapter{book_id, chapter_index} )
├── Toc( Load{book_id} )
├── Bookmark( Add{book_id, anchor, preview} | Remove{book_id, anchor} | List{book_id} )
├── Source( ImportFile{path} | ImportUrl{url} | ImportText{text}
│         | Search{generation, keyword} | CancelSearch{generation}
│         | Detail{source_id, book_url} | AddToShelf{source_id, book_url}
│         | SwitchSource{book_id, new_source_id} )
├── Progress( Flush{book_id} )   # 节流窗口到点/退出前的落盘
└── Settings( Load | Save{settings} )

EngineEvent（引擎线程 → UI）
├── ShelfLoaded{entries} | ShelfEntryChanged{entry} | ImportFailed{error}
├── ChapterReady{book_id, chapter_index, laid_out, content_version}
├── ChapterFailed{book_id, chapter_index, error}
├── ProgressRestored{book_id, progress} | TocLoaded{book_id, toc}
├── Bookmarks{book_id, list}
├── SearchResult{generation, source: SourceRef, hits} | SearchDone{generation}
├── DetailLoaded{source_id, book_url, detail} | AddedToShelf{entry}
├── SourceSwitched{book_id, anchor} | SettingsLoaded{settings} | Saved
└── EnginePanic{context}   # catch_unwind 兜底后的结构化呈现（spec Edge Case）
```

**线程纪律**：`Paginator`/FontSystem/store/source 仅存在于引擎线程；UI 线程只发请求、收事件、持有渲染所需的最新的 `laid_out` 快照（PaintCallback 消费）。

## 4. 错误视图模型

`ClassifiedErrorView { id: String（error.xxx 标识）, detail: Option<String>, capability: Option<String> }`——引擎层 `ClassifiedError`/能力错误经消息协议原样抵达，文案映射在外壳 i18n 表（R10）；标识永不翻译（FR-019/002 FR-058 对齐）。
