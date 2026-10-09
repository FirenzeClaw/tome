# Research: S0+S1 引擎内核技术选型验证

**日期**: 2026-10-09 | **特性**: 001-pc-engine-core | **方法**: 5 个并行研究代理（均受干净室约束：只查公开文档/crates/规范，未读上游 Kotlin 源码）

本文件解决 Technical Context 中的所有高风险未知项。每项为 Decision / Rationale / Alternatives。

---

## R1 · JS 沙箱引擎

**Decision**: 使用 **QuickJS**，经 **`rquickjs` ≥ 0.14.0** 集成。

**Rationale**: 满足全部五项硬性需求：

| 硬性需求 | 结论 | 依据 |
|---|---|---|
| 内存上限 | ✅ | `Runtime::set_memory_limit()` → `JS_SetMemoryLimit`；OOM 抛**可捕获** InternalError |
| 可中断死循环（不杀线程） | ✅ | `Runtime::set_interrupt_handler(Box<dyn FnMut()->bool>)`；返回 true 抛**不可捕获**异常；检查点覆盖 `OP_goto` ⇒ 裸 `while(true){}` 可断 |
| 同步宿主函数 | ✅ | `Function::new` + `IntoJsFunc` 默认同步；异步需显式开 `futures` feature |
| ES 超集 | ✅ | quickjs-ng 追到 ES2020+→ES2025；Rhino(ES5) 是严格子集。**唯一缺口：`Intl`(ECMA-402) 不支持**，书源几乎不用，如需则 polyfill |
| 三平台可编译 | ✅ | Linux/macOS/Windows-gnu 官方 tested；预生成 bindings，默认不用 bindgen |

体积约 200KB–1MB（官方 210 KiB 口径；ComponentizeJS 实测 WASM 场景 1.9MB，桌面更小）。维护活跃（0.14.0 于 2026-09 发布）。

**四条硬缓解措施（必须落实，否则防线失效）**：
1. **锁死 `rquickjs >= 0.14.0`** —— 0.14.0 才含中断处理触发的 GC 引用计数崩溃修复（PR #664）。写进 CI 断言。
2. **绝对禁止 `rust-alloc` / `allocator` feature** —— 一开 `set_memory_limit` 静默变 noop（用自定义分配器时不查 limit）。CI 加 `cargo tree -e features` 守卫。
3. **Windows 目标定为 `x86_64-pc-windows-gnu`**（与 stable-gnu + WinLibs MinGW 工具链吻合）；不赌 msvc（上游标 experimental）；**不引入 wasm target**（构建期会 curl 下载 wasi-sdk）。
4. **显式 `set_max_stack_size`**（如 256KiB–1MiB），深递归靠可捕获 RangeError 而非宿主栈溢出。

**已知残余风险（须实测）**：上游 Bellard QuickJS 的 OOM 销毁路径缺陷（bellard/quickjs#468 / CVE-2025-69654）在 quickjs-ng master 仍可见该断言。缓解：**触发过 OOM 的 runtime 整个丢弃、不复用**；把"低内存限额 + 畸形脚本"列为必测项。

**宿主函数阻塞 I/O 注意**：rquickjs 单 runtime 用锁序列化访问；宿主函数里同步阻塞（如发 HTTP）期间锁被持有 = 该引擎全局停摆。**对策：每个书源脚本用独立 runtime**（沙箱本就一次一脚本）。宿主函数状态放 `Ctx::userdata` 或闭包捕获，避免嵌套可变借用（否则 ClassBorrow/FunctionBorrow）。宿主函数返回 `Err` 会沿 JS 异常栈展开——HTTP 失败映射成 JS 侧抛错/null 均可。

**Alternatives considered**: `boa`（纯 Rust 但无字节级内存上限、无墙钟中断——硬需求不满足，排除）；`deno_core`/V8（体积 ~30-50MB 失控，排除）；Bellard 原版 quickjs（Windows 支持弱，quickjs-ng 更优）；`quickjs_runtime`（只是事件循环封装层，能力不超 rquickjs）。

---

## R2 · HTML 解析 / CSS / XPath（规则引擎的 DOM 基座）

**Decision**: **一棵 DOM，两个查询引擎**——自建 arena DOM（实现 html5ever 的 `TreeSink`），CSS 用 Servo `selectors` + `cssparser`，**XPath 1.0 子集自研**跑在同一棵 DOM 上。

**Rationale**:
- **HTML 解析**：`html5ever` 近乎全过 html5lib-tests（tree builder 跳过清单仅 2 条且是极新元素），逐条对照 WHATWG 规范实现，活跃维护（0.40.1，2026-09）。**这是唯一"浏览器级"选择**。**禁用 `markup5ever_rcdom`**（官方明说不受支持、未 fuzz、非生产级 DOM）。
- **CSS**：Servo `selectors`（Firefox 同款引擎）标准覆盖极全。但 **Jsoup 专有伪类一律不支持，必须自研**：`@text/@ownText/@html/@all` 后缀、索引切片（`.0/.-1/[0:3]/[!0:3]`）、`text.关键字`（=`:containsOwn`）、`:eq/:lt/:gt`、`[attr~=regex]` 的正则重载（CSS 里 `~=` 是空白分词，Jsoup 重载为正则——**同一条规则两边行为不同，必须特判**）、`[^attrPrefix]`。
- **XPath 自研而非 sxd-xpath**：sxd-xpath 的 DOM 类型被写死（`sxd_xpath::nodeset::Node` 直接引用 sxd_document 具体类型），必须 HTML→sxd_document 转换 = **两棵树** → CSS 与 XPath 结果会系统性分叉，这正是兼容性最怕的。`libxml2` 已 2025-12 停止维护且 HTML 解析是 HTML4 时代，双输。**自研 XPath 1.0 子集约 1.5–3k 行**，可控且能对齐 JsoupXpath 扩展（`allText()`/`html()` 等节点测试、`following-sibling-one` 等轴）。**这正是上游 JsoupXpath 的架构（Antlr4 + 自研求值器跑在自家 DOM 上）。**

**DOM 形状一致性（Jsoup vs html5ever）**：tbody 自动插入、noscript 默认文本化、自闭合标签（jsoup 1.20.1 起）、空白文本节点——**均对齐**。差异风险点：Jsoup 有 `maxDepth=512` 默认上限（html5ever 无）、节点类型模型不同（jsoup 有 DataNode/CDataNode，scraper 只有 Element/Text/Comment——`:containsData()`/`::data`/`::comment` 需自建 DOM 保留类型信息，**这是自建 DOM 的又一理由**）、SVG/MathML 外部内容是双方历史 bug 高发区。

**关键前置决策**：**钉死对齐目标为 Jsoup 1.2x 的 Selector 行为**，并据此写一份"非标准伪类实现清单"。

**差分测试基建是必须的**：公开资料无 Jsoup↔html5ever 对比数据集。做法：真实乱网页 HTML → Java 侧跑 Jsoup 输出序列化 DOM + 规则结果，Rust 侧跑本实现，逐条比对。

**Alternatives considered**: `scraper` 直接用（省事但 ElementRef API 窄，拿不到节点类型细分）——可作 CSS 层的过渡；`dom_query`（多送 `:contains/:has-text/:only-text`，最贴近 Jsoup 语义，值得评估）；`xmloxide`（一体化 HTML5+CSS+XPath，但太新 0.4.x 仅 4 依赖方——列观察对象）；`html5gum`(只 tokenizer)/`tl`(不规范)/`kuchiki`(已死)/`lol_html`(流式无 DOM)/`xee`(36-49MB XML-only)——均排除。

---

## R3 · JSONPath（对标 Jayway JsonPath）

**Decision**: 引擎用 **`serde_json_path`**（RFC 9535 最字面实现），前置一个**词法感知的 Jayway→RFC9535 重写 shim**；`jsonpath-rust` 作为备选引擎编译进（feature 门控）做差分；`min/max/avg/stddev/sum` 经 `serde_json_path` 的 `#[function]` 注册为一等函数。

**Rationale**:
- `jsonpath_lib` 最贴近 Jayway 的**解析**行为，但 2021-06 后停更——只作备选。
- `serde_json_path` 的过滤器实现最完整（`&&`/`||`/`!`/括号表达式/函数注册 API）。
- **Jayway 专有而两个 crate 都会 parse error 的语法**：脚本索引 `[(@.length-1)]`、终端函数 `min()/max()/avg()/stddev()/sum()`、`.length()`/`.length`、`in/nin/subsetof/anyof/noneof`、`=~ /re/i`、内联脚本 `[?(@.x.indexOf('a')>=0)]`。
- **语义（非语法）差异**：absent vs null、不可比较类型比较、definite/indefinite 结果形状（Jayway 对 definite 路径返回标量、indefinite 返回列表——shim 须复现此扁平化规则）。
- 估计这些合计影响约 1%~10% 的规则，且几乎全部可机械翻译。
- **重写 shim 不是正则替换**：须跟踪字符串字面量、转义引号、括号嵌套深度，否则会破坏含 `=`/`(` 的过滤器。

**工作量**：重写器 + 结果适配约 **3–5 人日**；200 例一致性语料 +2–3 人日；函数/操作符层 +2–3 人日。合计 **6–10 人日**。

**Alternatives considered**: 自研 Jayway 子集求值器（~1.5-2.5k LOC，12-20 人日，边缘易错——仅当 shim 残余失败率不可接受时）；fork 某 crate（继承 RFC 语义再偏离，最差）；`jsonpath-plus`（非 JS 移植，无 Jayway 特性，排除）。

---

## R4 · MOBI / AZW3 解析

**Decision**: **自研，分阶段**，站在 `kindling-mobi`（dev-dependency 作 oracle）与公开规范的肩膀上。不用现成 crate 作生产依赖。

**Rationale**:
- **没有一个 crate 提供"打开 AZW3 → 书名/作者/TOC/正文"的完整可用 API**。`mobi`(mobi-rs) 4 年未更新、无 TOC API、无 KF8、HUFF 路径残留调试代码；`iepub` 无 HUFF/CDIC（遇 `-c2` 文件输出乱码）；`ebook`/`ebook-rs` 太新不可信。
- **`kindling-mobi` 把最难的部分实现了**（HUFF/CDIC 解码 + INDX + KF8 结构原语），但 `indx.rs`/`kf8.rs` 是 `pub(crate)`，公开 API 只有 `huffcdic` 和 `mobi_dump`。**用法：加为 dev-dependency，用 `kindling dump` 作为解析结果的对照 oracle（输出设计成可 `diff -u` 比较）。**
- **格式公开资料足够自研**：MobileRead Wiki 的 PDB/MOBI/KF8 词条覆盖 PalmDB 容器、MOBI 头（0x00–0x10B 全表）、约 80 个 EXTH tag、变宽整数、INDX/TAGX/IDXT。缺口（KF8 的 FDST/SKEL/FRAG）靠对照开源实现补。
- **PalmDOC 解压半天**（公开逐字节算法）。**HUFF/CDIC 是硬骨头但非最硬**（有 mobi-rs `huff.md` 完整文字规范 + 4 份可交叉验证实现，Calibre Python 版最易读）。**真正的难点**：trailing data 剥离（错了不报错、静默丢字）、INDX/TAGX 解析、KF8 SKEL/FRAG 拼接。
- **DRM 判断**：PalmDOC 头偏移 0x0C 的 Encryption Type（0 无/1 单 PID/2 多 PID）。**陷阱：compression=17480("DH")是 HUFF/CDIC 不是 DRM**。KFX/Topaz 显式拒绝。

**分阶段降险**（对"只要纯文本"的需求）：
1. **阶段一（~8 人日）**：PalmDB + header + EXTH + trailing + PalmDOC → MOBI6 纯文本 + 元数据 + guide 型 TOC
2. **阶段二（~5 人日）**：+ HUFF/CDIC → 覆盖 `-c2` 文件与大部分老 AZW
3. **阶段三（~8 人日）**：+ KF8 的 **FDST primary flow**（取整段 HTML，**跳过 SKEL/FRAG 拼接**）→ AZW3 纯文本 + INDX/NCX TOC
4. 阶段四（可选）：SKEL/FRAG 完整还原、图片提取

**法律边界（干净室）**：libmobi 是 LGPL、Calibre/KindleUnpack 是 GPLv3——**阅读以理解算法没问题，但不逐行抄进代码库**；只依据 mobi-rs 的 `huff.md`（MIT 文档）与 MobileRead Wiki 文字规范独立实现，用 GPL 实现做行为对照测试（测试数据是字节序列，不受 GPL 传染）。

**测试语料**：PalmDOC MOBI6（Gutenberg 公有领域）、`kindlegen -c2` HUFF/CDIC MOBI、纯 KF8 AZW3、MOBI6+KF8 双格式、DRM 文件、损坏文件——**HUFF/CDIC 与 KF8 路径必须用真实文件验证，不能只靠单元测试**。

**合计约 23–43 人日**（跳 SKEL/FRAG 时约 20–35 人日）。**这是 S1 中最大的单项工作量，且阶段三可拆为独立后续任务。**

---

## R5 · 正则引擎（对标 Java java.util.regex）

**Decision**: **双引擎**——默认 `regex` crate（线性时间），编译期特性预扫描，含 fancy 特性的路由到 `fancy-regex`。前置一个**替换串翻译层**与**语法规范化层**。

**Rationale**:
- **`regex` 不支持** lookaround/backreference（设计上，保证 O(m·n)），运行时报 `UnsupportedLookAround`。但**关键陷阱**：`regex` **静默把 `a++` 解析为嵌套重复 `(a+)+` 而非占有量词**——实测 `a++a` on "aaa" → `Some("aaa")`，而 Java 占有量词语义下不匹配。**静默语义分叉，无编译错误**。规范化层必须把 `a++`/`a*+`/`a?+` 显式转成 `(?>a+)`（原子组）或拒绝。
- **`fancy-regex`（0.19.0，活跃维护）** 混合引擎：无 fancy 特性时委托给 `regex` 快路径（线性）。支持 lookahead/lookbehind（含变长）/backreference/原子组/占有量词/子程序/`\K \G \Z \R \N`。**实测分叉**：`\Q...\E` 不支持（parse error）；`\h` 在 fancy-regex 是 hex 数字（与 Java 的水平空白冲突）；`(?=a)b` on "ab" → `None`（Java 匹配 "b"）。
- **灾难性回溯是真实风险**：实测 `(?=a)(a+)+b` 对 5,000 字符耗 1.82s，对 **200,000 字符直接挂起 >120s**。`fancy-regex` 的 `backtrack_limit`（默认 100 万 states）是唯一进程内护栏，但它是**状态计数**而非墙钟超时，且 `find()`/`is_match()` 同步不可中断。**缓解：fancy 模式强制低 backtrack_limit + 输入长度上限/分块执行 + 编译期预扫描只把含 fancy 特性的路由过去**。
- **语义差异表**（同语法不同行为）：`\d\w\s` Unicode(Rust `\p{Nd}` 等) vs ASCII(Java 默认)；`.` 换行（Java 排除 6 种行终止符，Rust 默认只排 `\n`）；`$`/`^` 多行模式（Java `$` 匹配末行终止符前，Rust `a$` on "a\n" → None）；大小写折叠（Java ASCII-only unless UNICODE_CASE，Rust Unicode simple folding）；`\b` 词边界定义。

**替换串翻译层（`$1`→Rust）注意点（kill case）**：
- Rust 用**最长可能名**解析：`$1a` 读作名为 `1a` 的组 → 空串；Java 读作组 1 + 字面 `a`。**朴素透传会静默出错**。
- 翻译规则：`$`+数字 → `${N}`；字面 `$` → `$$`；Java 的 `\$` 转义 → `$$`；处理非法组引用（Rust 非法引用 → 空串）。
- `fancy-regex` 的 `Expander` 与 `regex` 用同一最长名规则，**两个引擎共用同一翻译层**。

**Alternatives considered**: 纯 `regex`（快但缺 lookaround/backref 且静默占有量词分叉，不满足兼容）；纯 `fancy-regex`（全但长文挂起风险无墙钟护栏，不可接受）。

---

## 综合：对设计共识的修订

研究与设计共识有两处**实质性修订**（不违反宪章"禁止退而求其次"——这是基于证据的更优解，已在上方论证）：

| 项 | 设计共识原述 | 研究后修订 | 理由 |
|---|---|---|---|
| XPath | `sxd-document + sxd-xpath` | **自研 XPath 1.0 子集，跑在共享 DOM 上** | sxd-xpath DOM 类型写死需 HTML→XML 转换=两棵树，CSS/XPath 结果系统性分叉；libxml2 已停止维护且 HTML4 |
| HTML DOM | `html5ever`（用 rcdom） | **自建 arena DOM（实现 TreeSink），禁用 markup5ever_rcdom** | rcdom 官方明说不受支持、未 fuzz、非生产级 |

其余（QuickJS/rquickjs、html5ever、serde_json_path、regex+fancy-regex、reqwest+rustls、rusqlite、egui/wgpu/cosmic-text 归 S2/S3）均获验证通过。

## 遗留待办（S0 spike 验证）

- **rquickjs 接入后的实际二进制体积**——官方 210KiB 口径，需自测 baseline 对照（唯一需本机量化项）
- **Jsoup↔自建DOM 的真实差异率**——需小型 spike 量化（研究为文档推断，非实测）
