# Contracts: 引擎接缝（Seam）契约

**特性**: 001-pc-engine-core | **日期**: 2026-10-09 | **对象**: tome-core 暴露给上层（tome-app / tome-extra / tome-cli）的稳定接口

引擎以**无界面库**形式提供能力（FR-036）。上层只通过这些 seam trait 交互，永不直接构造下层内部类型（宪法 IV / AGENTS.md 架构铁律）。

> 这些是**接口契约**（调用方须知的一切：签名、不变量、错误模式、性能特征），不是 Rust `trait` 关键字的字面定义——但那是最直接的落点。

## 接缝总览

| Seam | 职责 | 适配器（实现） | 深度 |
|---|---|---|---|
| `SourceEngine` | 书源五动作 + 换源 + 源检查 | tome-source | 深 |
| `ContentProvider` | 统一内容提供者（网络书/本地书两个 adapter） | tome-source / tome-local | 深 |
| `BookRepository` | 书架/进度/痕迹/规则的读写 | tome-store | 深 |
| `JsRuntime`（宿主注入面） | JS 沙箱，宿主能力注入 | tome-js | 深 |

错误一律经 `ErrorKind` 分类（网络/规则/不支持能力/内容/存储/配置），携带可本地化标识（FR-044），上层据此映射文案。

---

## 1. SourceEngine — 书源引擎

调用方：S3 界面（搜索/发现/详情/换源）、tome-cli。

```
search(keyword, scope) → SearchStream
discover(source_id, kind, page) → Result<ExplorePage>
book_info(source_id, book_url) → Result<BookInfo>
toc(source_id, book) → Result<Vec<Chapter>>
change_source(book, candidate) → Result<ChangeSourceOutcome>   -- 含进度映射（FR-010b 三级降级）
check_source(source_id) → Result<CheckReport>                   -- 单源
check_sources(filter) → CheckStream                             -- 批量，流式
```

**关键不变量**：
- `search` 返回**流 + 取消令牌**（架构审查 A6），不阻塞至全部源完成；单源失败是流内的一个条目（带 ErrorKind），不终止整个流
- `toc` 是目录获取；正文不在此 seam（在 ContentProvider）
- `change_source` 返回的 `ChangeSourceOutcome` 必须标明采用了哪一级进度映射（特征段/标题/降级）及其结果——换源过程可观测（FR-010b）

**错误模式**：源不存在 → 规则错；网络失败 → 网络；WebView 类能力 → `UnsupportedCapability("webView")`（结构化、可被上层识别，FR-006）。

**性能特征**：`search` 增量返回（首个结果尽快）；`toc` 5 万章 <1s（SC-003）。

---

## 2. ContentProvider — 统一内容提供者（架构审查 A4）

调用方：S2 阅读器、tome-cli。两个 adapter：**网络书**（缓存或抓取）、**本地书**（偏移直读）。

```
toc(book) → Result<Vec<Chapter>>
chapter_content(book, chapter_index) → Result<ChapterContent>   -- 净化管线已下游统一应用
resolve_anchor(book, anchor) → Result<(chapter_index, char_offset)>  -- 锚点解析（SC-008）
prefetch(book, around_index, depth)   -- 预取（顺读预热），无副作用失败
```

**关键不变量**：
- **净化/高亮/词典规则在 seam 下游统一应用**（FR-014 顺序固定：替换→高亮→词典），网络书与本地书一视同仁
- `chapter_content` 对网络书：先查 Pack 缓存，未命中则抓取并写入缓存（崩溃安全 FR-030）；对本地书：按偏移直读
- 锚点解析结果与排版参数无关（任意排版参数下解析为同一锚点）

**为什么是真 seam**：两个 adapter（网络 / 本地）——不是假想 seam。

---

## 3. BookRepository — 数据读写

调用方：S3 界面、tome-cli。实现：tome-store（SQLite actor 线程 + Pack 文件）。

```
bookshelf() → Result<Vec<Book>>
add_book / remove_book / update_book
get_progress(book) / save_progress(book, anchor)
bookmarks / highlights / memos (CRUD, 字符区间)
rules (replace/highlight/dict/toc CRUD)
import_sources(channel: SourceImportChannel) → Result<ImportReport>   -- 三渠道 file|url|text（FR-001）；url 渠道的拉取与解析在接缝实现侧完成（A2 唯一发请求处），不得退化为"上层解析好再传 Source"
import_parsed_sources(sources: &[Source]) → Result<ImportReport>      -- 已解析书源的快路径落库；与 import_sources 共用"同 id 即更新"语义
export_all() → Result<BackupJson>                           -- 全部阅读数据（FR-033）
import_backup(json) → Result<ImportReport>
read_records(book, range) → Result<Vec<ReadRecord>>         -- FR-026b
search_in_book(book, query) → Result<Vec<SearchHit>>        -- FR-019b 书内全文搜索（已缓存章节）
clear_cache(CacheKind)     -- Chapter / Image 两个可独立清理的资源（SC-017）
```

**关键不变量**：
- 阅读痕迹一律字符区间，不存渲染坐标
- 缓存写入崩溃安全（FR-030）；缓存清除不影响个人数据（FR-031/SC-015）
- 导入重复书源（同 id）按"更新"处理
- 所有写操作走 DB actor 线程，调用方拿到的是结果而非连接

---

## 4. JsRuntime — JS 沙箱（宿主注入面，架构审查 A1）

**注意方向**：宿主能力 trait（NetHost/StoreHost/CryptoHost）与共享值对象 `RequestSpec`/`Response` **定义在 tome-core**（接缝中枢），不在 tome-js——否则 tome-net 实现 NetHost 需依赖 tome-js、而 NetHost 又引用 RequestSpec 构成循环依赖。一切实现 crate 只依赖 tome-core；tome-js 被组合根注入实现，不依赖 tome-net/tome-store。

```
// tome-core 定义的宿主能力 trait（seam）
trait NetHost    { fn fetch(spec: RequestSpec) -> Result<Response> }   -- 由 tome-net 实现
trait StoreHost  { fn cache_get/put/path ... }                          -- 由 tome-store 实现
trait CryptoHost { ... }                                                -- 纯计算，tome-js 内建

// tome-js 对外能力
Runtime::new(limits) → Runtime            -- 内存上限 + 栈上限 + 中断 handler
runtime.register_host(impl NetHost, ...)  -- 组合根注入宿主实现
runtime.eval(source_code, ctx) → Result<Value, JsError>
```

**硬性约束（R1 研究结论）**：
- `rquickjs >= 0.14.0`（中断 GC 修复）；**禁止 rust-alloc/allocator feature**
- 内存上限 + 显式栈上限 + 中断 handler（死循环可断，FR-007/SC-013）
- 宿主函数同步阻塞期间引擎停摆——**每个书源脚本用独立 runtime**
- 触发过 OOM 的 runtime 整个丢弃不复用
- 宿主 API 约 80 个方法（FR-005 八类清单）；不支持能力抛结构化错误（FR-006）
- 注入 **compat prelude** 补齐 Rhino 方言差异

---

## 契约与差分测试的关系

**RequestSpec 纯化（架构审查 A2）**：规则求值器不亲自发请求，产出 `RequestSpec`（方法/URL/头/体/编码/Cookie/间隔）；管线是唯一发请求处。这让规则引擎离线可重放（录制响应→重放），差分测试（兼容性验证，宪章 III）由此确定。

`RequestSpec` / `Response` 是 **tome-core 的共享值对象**（规则引擎产出、网络层执行、JS 宿主转发，三方都引用它）：

```
RequestSpec { method, url, headers, body, charset, proxy_override, rate_limit_key, timeout, retry }
Response    { status, headers, body_bytes, detected_charset }
```
