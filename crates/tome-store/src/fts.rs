//! \[DONE] 书内全文搜索（tasks.md T090 / FR-019b）
//!
//! # FR-019b 的原文里有一条**硬约束**
//!
//! > 系统 MUST 支持书内全文搜索：对一本书的**已缓存**章节内容进行全文检索，
//! > 返回命中的章节与位置（供上层实现"书内 Ctrl+F"）；检索 MUST **限定在
//! > 已缓存内容，不触发额外抓取**
//!
//! 最后一句是本模块所有设计的出发点。它排除了一个看起来更简单、也更常见的
//! 实现："搜索时遍历目录，未命中的章节按需抓取"。那样做的后果是：
//! **用户按一次 Ctrl+F 会触发几百次网络请求**——对一个 5 万章的书，
//! 那既是几十分钟的等待，也是对源站的滥用（违反"遵守书源声明的请求间隔"）。
//!
//! 故本模块的结构是：
//!
//! ```text
//!   索引（写入侧）                    检索（读取侧）
//!   ─────────────                    ─────────────
//!   章节落缓存时一并入 FTS           只查 FTS，**不碰 NetHost**
//!   ↑ 唯一的写入入口                  ↑ 签名里根本没有网络能力
//! ```
//!
//! 检索侧签名里没有 `NetHost`，故"不触发抓取"是**结构上不可能违反**的，
//! 而不是靠实现者记得不要抓。
//!
//! # 分词方案：**单字切分**（实测选定，非默认选项）
//!
//! 这一条是本模块最关键的技术决定，且它来自实测而非预判。
//!
//! ## 实测：FTS5 的两个内置分词器都**不满足**中文书内搜索
//!
//! | 分词器 | 2 字中文查询（如"主角"） | 说明 |
//! |---|---|---|
//! | `trigram` | **搜不到** | trigram 按 3 字滑窗建索引，查询短于 3 字符时无索引项可匹配 |
//! | `unicode61` | **搜不到** | 它按空白/标点切分，整段中文被当成**一个词**，"主角"不是它的前缀 |
//!
//! 而"主角"这样的 2 字词恰恰是中文 Ctrl+F 里**最常见**的输入
//! （人名、地名的绝大多数）。若沿用任一内置分词器，功能等于没做。
//!
//! ## 采用的方案：写入时按**单字**切分，查询时用**短语**约束
//!
//! ```text
//!   写入："主角登场"  →  索引文本 "主 角 登 场"
//!   查询："主角"      →  短语    ""主 角""     （相邻即命中）
//! ```
//!
//! 选 `unicode61` 作分词器（它按空格切，故单字各成一项），代价与收益：
//!
//! - **收益**：任意长度（含 1 字、2 字）的查询都能命中，且短语约束保证
//!   **连续性**——实测 "主是角" 不会被 "主角" 搜到，而 "这是主角" 会。
//! - **代价**：索引比按词切大（每个字一项）。但书内搜索的索引规模由
//!   **已缓存章节**决定，而缓存本身有容量上限（FR-029），故这个放大
//!   被缓存边界兜住了。
//!
//! 自建倒排索引能达到同样效果，但要自己实现增量维护、崩溃恢复与淘汰
//! 联动——这三件事 FTS5 已经做对了。
//!
//! # 为什么用 SQLite FTS5 而不是另建一套索引
//!
//! 1. **事务一致性**——索引与章节缓存索引在**同一个数据库**里，故"缓存被
//!    淘汰"与"索引还会搜到它"之间的不一致可以用事务消除。分离的索引
//!    （或内存索引）会在崩溃/淘汰后产生悬空命中。
//! 2. **不引新依赖**——rusqlite 的 bundled 构建已含 FTS5（`schema.rs` 的
//!    迁移器早就用它建过表），零新增体积。
//! 3. **删除语义可用**——`contentless_delete=1` 让"内容不存副本"与
//!    "能删条目"同时成立（`content=''` 的默认形态**不允许 DELETE**，
//!    实测报 `cannot DELETE from contentless fts5 table`）。
//!
//! # 位置口径（与 FR-028 一致）
//!
//! 命中的 `char_offset` 是**章内 Unicode 字符偏移**，不是字节偏移——
//! 与高亮/批注/书签/进度的口径一致（`tome-core` 的 `CharLocation` 同理）。
//! 中文正文下字节与字符差 3 倍，混淆会让"跳转到命中处"整体偏移。
//!
//! # 索引的可见性边界
//!
//! 索引表 `chapter_fts` 是**可用内容描述**，不是**内容本身**的可搜索副本
//! （FTS5 存的是分词后的倒排，原文需从 Pack 缓存读）。故一条记录存在
//! **不等于**内容还在——缓存淘汰会同时删索引（见
//! [`CachedTextIndex::remove_chapter`]），但若进程在淘汰中途被杀，
//! 可能留下"索引在、内容没了"的条目。检索时**校验内容可取**再返回命中，
//! 缺内容的条目顺手清掉。这样"搜到但点不开"不会发生。

use crate::db::sql;
use crate::error::StoreError;

/// FTS5 表的建表语句。
///
/// # 为什么 `content=''`（外部内容表）
///
/// 若存内容副本，一份正文会在库里存两遍（Pack 里一遍、FTS 里一遍）——
/// 直接冲击 SC-005 的"缓存开销 ≤ 内容 × 1.01"。用 `content=''` 的
/// **内容表**模式让 FTS 只存倒排索引（不含原文），查询时按 rowid 关联
/// 回 `chapter` 表取元信息、从 Pack 取正文。
///
/// # 为什么是 `unicode61` + 单字切分（而非 `trigram`）
///
/// 见模块文档的实测对比：`trigram` 对不足 3 字符的查询**无索引项可匹配**，
/// 而 2 字中文词（"主角"）是书内搜索最常见的输入。改用 `unicode61`
/// 并**在写入时把正文按单字空格隔开**，查询时用短语约束连续性——
/// 任意长度可搜且保证相邻。
///
/// `contentless_delete = 1` 是必需的：`content=''` 的默认形态**不允许
/// DELETE**（实测报 `cannot DELETE from contentless fts5 table`），
/// 而缓存淘汰必须能移除索引条目。
const FTS_SCHEMA: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS chapter_fts USING fts5(
    chars,
    tokenize = 'unicode61',
    content = '',
    contentless_delete = 1
);
";

/// 一条命中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookSearchHit {
    /// 章节在书内的序号（0 起）
    pub chapter_index: i32,
    /// 章节标题（便于上层直接展示，不必再查一次）
    pub chapter_title: String,
    /// 命中处在章内的 **Unicode 字符**偏移
    pub char_offset: i64,
    /// 命中处的上下文片段（供上层展示）
    pub snippet: String,
}

/// 书内全文搜索的索引与检索。
///
/// # 两个入口为什么在同类型上
///
/// 索引与检索共用同一张表的定义（`FTS_SCHEMA`）。分到两个类型里会让
/// "建表语句只有一处"这件事变成约定——而两处表名写错一个字母的后果是
/// 索引写进了 A 表、检索查 B 表，表现为"搜索永远没结果"且不报错。
#[derive(Debug, Clone)]
pub struct CachedTextIndex {
    db: crate::db::Db,
    /// 章节内容读取（用于校验命中处内容仍在）
    pack: Option<std::sync::Arc<crate::pack::PackStore>>,
}

impl CachedTextIndex {
    /// 只做索引读写，不带内容校验。
    ///
    /// **适用场景有限**：纯索引测试，或调用方自己保证内容仍在。
    /// 生产路径应走 [`Self::with_pack`]——否则会搜到已被淘汰的章节。
    #[must_use]
    pub fn new(db: crate::db::Db) -> Self {
        Self { db, pack: None }
    }

    /// 带内容校验的构造（推荐）。
    #[must_use]
    pub fn with_pack(db: crate::db::Db, pack: std::sync::Arc<crate::pack::PackStore>) -> Self {
        Self {
            db,
            pack: Some(pack),
        }
    }

    /// 建表（幂等）。`Db::open` 的迁移器不建它——见本模块的
    /// [`ensure_schema`] 说明。
    pub fn ensure_schema(&self) -> Result<(), StoreError> {
        self.db.write(|connection| {
            connection.execute_batch(FTS_SCHEMA).map_err(sql)?;
            Ok(())
        })
    }

    /// 把一章的正文写入索引（幂等：同章重复写会先删旧条目）。
    ///
    /// # 为什么不需要 `chapter_index`
    ///
    /// FTS 表的 rowid 就是 `chapter.id`，检索时经 `JOIN chapter` 取序号。
    /// 把序号也塞进索引等于存了冗余信息——而冗余就是漂移的来源
    /// （章序号变了，索引里还留着旧的）。
    ///
    /// # 为什么是"先删再插"而不是 `INSERT OR REPLACE`
    ///
    /// FTS5 的虚拟表不支持 `REPLACE` 语义（它的 rowid 与索引块不是一对一）。
    /// 重复插入会在索引里留下**两份**该章的分词，于是同一处命中会返回两次。
    pub fn index_chapter(
        &self,
        chapter_id: i64,
        title: &str,
        content: &str,
    ) -> Result<(), StoreError> {
        // 索引文本 = 标题 + 正文，**按单字切分**（见模块文档的实测结论）。
        // 标题也进去，让"按章节标题搜"一并可用。
        let indexed = segment_chars(&format!(
            "{title}
{content}"
        ));
        self.db.write(move |connection| {
            connection
                .execute("DELETE FROM chapter_fts WHERE rowid=?1", [chapter_id])
                .map_err(sql)?;
            connection
                .execute(
                    "INSERT INTO chapter_fts(rowid, chars) VALUES (?1, ?2)",
                    rusqlite::params![chapter_id, indexed],
                )
                .map_err(sql)?;
            Ok(())
        })
    }

    /// 从索引里移除一章（缓存淘汰时调用）。
    pub fn remove_chapter(&self, chapter_id: i64) -> Result<(), StoreError> {
        self.db.write(move |connection| {
            connection
                .execute("DELETE FROM chapter_fts WHERE rowid=?1", [chapter_id])
                .map_err(sql)?;
            Ok(())
        })
    }

    /// 从索引里移除一本书的全部章节。
    pub fn remove_book(&self, book_id: i64) -> Result<usize, StoreError> {
        self.db.write(move |connection| {
            // FTS5 的 rowid 是 chapter.id，故按 chapter 表筛出该书的 id 再删
            let removed = connection
                .execute(
                    "DELETE FROM chapter_fts WHERE rowid IN
                     (SELECT id FROM chapter WHERE book_id=?1)",
                    [book_id],
                )
                .map_err(sql)?;
            Ok(removed)
        })
    }

    /// 索引里有多少条记录（**全局**，不限书）。
    pub fn indexed_count(&self) -> Result<i64, StoreError> {
        self.db.read(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM chapter_fts", [], |row| row.get(0))
                .map_err(sql)
        })
    }

    /// 某本书已索引的章节数。
    pub fn indexed_count_of_book(&self, book_id: i64) -> Result<i64, StoreError> {
        self.db.read(move |connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM chapter_fts WHERE rowid IN
                     (SELECT id FROM chapter WHERE book_id=?1)",
                    [book_id],
                    |row| row.get(0),
                )
                .map_err(sql)
        })
    }

    /// 书内搜索（FR-019b）。
    ///
    /// # 签名里没有 `NetHost`（"不触发抓取"的结构性保证）
    ///
    /// 见模块文档。本方法与它调用的私有方法都不接受任何网络能力。
    ///
    /// # 只搜**已缓存**的章节
    ///
    /// SQL 里的 `JOIN chapter_cache` 就是这条约束的执行点：只有进了缓存
    /// 容器的章节才在索引里（索引由 [`Self::index_chapter`] 在落缓存时
    /// 写入）。即便有人手工往 FTS 里塞了未缓存章节的文本，这里也搜不到。
    pub fn search_in_book(
        &self,
        book_id: i64,
        query: &str,
    ) -> Result<Vec<BookSearchHit>, StoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let pattern = fts_query(query);
        // 纯标点/空白的查询切完是空串，拿去 MATCH 会让 FTS5 报语法错。
        // 提前返回空结果（用户只是搜了个括号，不是出错）
        if pattern.trim_matches('"').trim().is_empty() {
            return Ok(Vec::new());
        }
        let rows: Vec<(i64, i32, String)> = self.db.read({
            let pattern = pattern.clone();
            move |connection| {
                // JOIN chapter 取章节序号与标题；JOIN chapter_cache 限定"已缓存"
                let mut statement = connection
                    .prepare_cached(
                        "SELECT c.id, c.index_in_book, c.title
                         FROM chapter_fts f
                         JOIN chapter c       ON c.id = f.rowid
                         JOIN chapter_cache k ON k.chapter_id = c.id
                         WHERE chapter_fts MATCH ?1 AND c.book_id = ?2
                         ORDER BY c.index_in_book",
                    )
                    .map_err(sql)?;
                let mapped = statement
                    .query_map(rusqlite::params![pattern, book_id], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })
                    .map_err(sql)?;
                let mut collected = Vec::new();
                for row in mapped {
                    collected.push(row.map_err(sql)?);
                }
                Ok(collected)
            }
        })?;

        let mut hits = Vec::new();
        for (chapter_id, chapter_index, title) in rows {
            // 取正文：命中处的偏移与片段都要从**原文**算，FTS 的倒排里
            // 只有位置信息，没有可展示的文本。
            //
            // ## 没有 `pack` 与"内容取不到"是**两件不同的事**
            //
            // - **没配 pack**（`Self::new`）：调用方自己保证内容在，但本方法
            //   读不到原文。此时**不能删索引**——那样"跑一次不带 pack 的搜索"
            //   会静默清空索引。给一条只有章节级信息的命中（偏移 0、无片段），
            //   让调用方看到"这一章命中了"而不是什么都没有。
            // - **配了 pack 但内容没了**：那是缓存淘汰留下的悬空条目
            //   （淘汰中途崩溃）。清掉它，避免下次再搜到——这是真的该删。
            let content = match self.load_content(chapter_id) {
                Ok(Some(content)) => Some(content),
                Ok(None) if self.pack.is_some() => {
                    // 悬空条目：清掉
                    let _ = self.remove_chapter(chapter_id);
                    None
                }
                Ok(None) => None,
                Err(error) => return Err(error),
            };

            let Some(content) = content else {
                // 没有原文：给章节级命中（见上）
                hits.push(BookSearchHit {
                    chapter_index,
                    chapter_title: title,
                    char_offset: 0,
                    snippet: String::new(),
                });
                continue;
            };

            // 一章可能多处命中——上层做 Ctrl+F 时"这一章有几处"是有用信息
            for char_offset in find_all_char_offsets(&content, query) {
                hits.push(BookSearchHit {
                    chapter_index,
                    chapter_title: title.clone(),
                    char_offset,
                    snippet: snippet_around(&content, char_offset, query),
                });
            }
        }
        Ok(hits)
    }

    /// 读某章的正文（从 Pack 缓存）。
    ///
    /// 没有 `pack` 时返回 `None`——见调用点对"没配 pack"与"内容取不到"
    /// 的区分：前者不该删索引，后者才该。
    fn load_content(&self, chapter_id: i64) -> Result<Option<String>, StoreError> {
        let Some(pack) = &self.pack else {
            return Ok(None);
        };
        let Some(bytes) = pack.read_chapter(chapter_id)? else {
            return Ok(None);
        };
        // 缓存里存的是 UTF-8 正文。非法 UTF-8 说明容器损坏——
        // 按"内容取不到"处理（跳过该命中），而不是让整次搜索失败：
        // 一章坏了不该让用户搜不了别的章。
        Ok(String::from_utf8(bytes).ok())
    }
}

/// 建索引表（供迁移器与测试调用，幂等）。
pub fn ensure_schema(db: &crate::db::Db) -> Result<(), StoreError> {
    CachedTextIndex::new(db.clone()).ensure_schema()
}

/// 把索引文本按**单字**切分（用空格隔开）。
///
/// # 为什么只切"需要索引的字符"
///
/// 空白与标点本身不必成项——它们永远不会是用户搜索的目标，而进来只会
/// 让索引变大。故跳过 `char::is_whitespace()` 与常见标点，其余逐字成项。
///
/// ASCII 字母数字连在一起（如 `hello`）**整词保留**：英文单词按字母切会
/// 让 `hello` 的索引项是 `h e l l o`，查询 `he` 会因短语约束要求相邻
/// 而**命中 "he" 开头的任何词**——那是对的。但让 `ell` 也命中 `hello`
/// 也是对的（子串搜索）。故整词保留对英文反而更精确，且索引更小。
///
/// 判据是"这个字符是 CJK 吗"：CJK 逐字切，其余（拉丁字母、数字）
/// 按连续段保留。
#[must_use]
fn segment_chars(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut latin_run = String::new();

    let flush = |out: &mut String, run: &mut String| {
        if !run.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(run);
            run.clear();
        }
    };

    for ch in text.chars() {
        if is_cjk(ch) {
            // CJK：逐字成项
            flush(&mut out, &mut latin_run);
            if !out.is_empty() {
                out.push(' ');
            }
            out.push(ch);
        } else if ch.is_alphanumeric() {
            latin_run.push(ch);
        } else {
            // 空白/标点：既不入索引，也打断拉丁词
            flush(&mut out, &mut latin_run);
        }
    }
    flush(&mut out, &mut latin_run);
    out
}

/// 是否属于需要逐字切分的表意文字。
///
/// 覆盖 CJK 统一表意文字及其扩展区、兼容表意文字、日文假名、韩文音节。
/// 判据取"表意文字"而非"非 ASCII"：西里尔字母/希腊字母按词切分是对的，
/// 逐字切它们只会让索引膨胀。
#[must_use]
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3400..=0x4DBF      // CJK 扩展 A
        | 0x4E00..=0x9FFF    // CJK 统一表意文字
        | 0xF900..=0xFAFF    // CJK 兼容表意文字
        | 0x20000..=0x2FA1F  // CJK 扩展 B~F
        | 0x3040..=0x30FF    // 日文平假名/片假名
        | 0xAC00..=0xD7AF    // 韩文音节
    )
}

/// 把用户输入转成 FTS5 查询串。
///
/// # 为什么要转义而不是直接传
///
/// FTS5 的查询语法有特殊字符（`"`、`*`、`^`、`-`、`(`、`)`、`AND`/`OR`/`NOT`）。
/// 用户搜 `C++` 或 `"引号"` 时，直接把原文拼进 `MATCH` 会得到语法错误——
/// 表现为"搜索报错"，而用户只是搜了个带加号的词。
///
/// # 为什么是"单字短语"
///
/// 索引侧按单字切分（见 [`segment_chars`]），故查询侧也要用同样口径，
/// 且必须用**短语**（整体加引号）而不是空格分隔——后者是 `AND` 语义，
/// 会把"主"与"角"分开匹配（"主是角"会误命中）。短语约束要求相邻，
/// 与用户对"子串搜索"的预期一致。
///
/// 内部双引号翻倍转义，避免用户输入的引号破坏短语边界。
#[must_use]
fn fts_query(query: &str) -> String {
    let segmented = segment_chars(query);
    format!("\"{}\"", segmented.replace('"', "\"\""))
}

/// 在一段文本里找出某个字面串的**全部**字符偏移。
///
/// 返回的是字符偏移（非字节），与 FR-028 的口径一致。
#[must_use]
fn find_all_char_offsets(haystack: &str, needle: &str) -> Vec<i64> {
    if needle.is_empty() {
        return Vec::new();
    }
    let mut offsets = Vec::new();
    // 逐**字符**推进而不是逐字节：直接拿到字符偏移，且天然支持重叠
    // （"aa" 在 "aaa" 里应有 0 与 1 两处——按字节跳过 needle 长度会漏掉后者）
    for (char_offset, (byte_offset, _)) in haystack.char_indices().enumerate() {
        if haystack[byte_offset..].starts_with(needle) {
            offsets.push(char_offset as i64);
        }
    }
    offsets
}

/// 取命中处附近的上下文片段。
///
/// 片段按**字符**截取（不是字节），避免把多字节字符切一半产生乱码。
fn snippet_around(content: &str, char_offset: i64, query: &str) -> String {
    const BEFORE: usize = 20;
    const AFTER: usize = 20;

    let chars: Vec<char> = content.chars().collect();
    let start = (char_offset as usize).saturating_sub(BEFORE);
    let query_chars = query.chars().count();
    let end = (char_offset as usize + query_chars + AFTER).min(chars.len());
    let start = start.min(chars.len());

    let mut snippet: String = chars[start..end].iter().collect();
    // 换行会把"一行片段"变成两行，展示时排版乱——折成空格
    snippet = snippet.replace(['\n', '\r'], " ");
    if start > 0 {
        snippet.insert(0, '…');
    }
    if end < chars.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: std::path::PathBuf,
        db: crate::db::Db,
        index: CachedTextIndex,
        /// 真实 Pack 缓存 —— 生产路径下正文从这里读，故测试也该用它，
        /// 否则测不到"从缓存取原文算偏移/片段"这一段
        pack: std::sync::Arc<crate::pack::PackStore>,
        book_id: i64,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-fts-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");
        let pack = std::sync::Arc::new(crate::pack::PackStore::new(
            &dir.join("cache").join("chapters"),
            db.clone(),
        ));
        let index = CachedTextIndex::with_pack(db.clone(), std::sync::Arc::clone(&pack));
        index.ensure_schema().expect("应能建索引表");

        let book_id: i64 = db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at)
                         VALUES (0, 'x', '书', 0)",
                        [],
                    )
                    .map_err(sql)?;
                Ok(connection.last_insert_rowid())
            })
            .expect("应能建书");

        Fixture {
            index,
            db,
            pack,
            dir,
            book_id,
        }
    }

    impl Fixture {
        /// 造一章：写 chapter 行、标记已缓存、建 chapter_cache 行、入索引。
        fn add_cached_chapter(&self, index_in_book: i32, title: &str, content: &str) -> i64 {
            let book_id = self.book_id;
            let content_len = content.len() as i64;
            let write_title = title.to_string();
            let chapter_id: i64 = self
                .db
                .write(move |connection| {
                    connection
                        .execute(
                            "INSERT INTO chapter(book_id, index_in_book, title, is_cached)
                             VALUES (?1, ?2, ?3, 1)",
                            rusqlite::params![book_id, index_in_book, write_title],
                        )
                        .map_err(sql)?;
                    let id = connection.last_insert_rowid();
                    connection
                        .execute(
                            "INSERT INTO chapter_cache(chapter_id, book_id, pack_id, offset, length, cached_at)
                             VALUES (?1, ?2, 1, 0, ?3, 0)",
                            rusqlite::params![id, book_id, content_len],
                        )
                        .map_err(sql)?;
                    Ok(id)
                })
                .expect("应能建章节");
            // 正文写进真实 Pack 缓存（生产路径同样如此：先落缓存、再入索引）
            self.pack
                .write_chapter(chapter_id, content.as_bytes())
                .expect("应能写缓存");
            self.index
                .index_chapter(chapter_id, title, content)
                .expect("应能建索引");
            chapter_id
        }

        /// 造一章但**不缓存**（只建 chapter 行与索引，不建 chapter_cache）。
        ///
        /// 用于验证"未缓存的不该被搜到"。
        fn add_uncached_chapter(&self, index_in_book: i32, title: &str, content: &str) -> i64 {
            let book_id = self.book_id;
            let write_title = title.to_string();
            let chapter_id: i64 = self
                .db
                .write(move |connection| {
                    connection
                        .execute(
                            "INSERT INTO chapter(book_id, index_in_book, title, is_cached)
                             VALUES (?1, ?2, ?3, 0)",
                            rusqlite::params![book_id, index_in_book, write_title],
                        )
                        .map_err(sql)?;
                    Ok(connection.last_insert_rowid())
                })
                .expect("应能建章节");
            self.index
                .index_chapter(chapter_id, title, content)
                .expect("应能建索引");
            chapter_id
        }
    }

    #[test]
    fn 搜到已缓存章节的命中() {
        let fixture = fixture("basic");
        fixture.add_cached_chapter(0, "第一章 起点", "主角登场，故事开始。");
        fixture.add_cached_chapter(1, "第二章 转折", "主角遇到了对手。");

        let hits = fixture
            .index
            .search_in_book(fixture.book_id, "主角")
            .expect("应能搜索");
        assert_eq!(hits.len(), 2, "两章都含「主角」");
        assert_eq!(hits[0].chapter_index, 0);
        assert_eq!(hits[1].chapter_index, 1);
    }

    #[test]
    fn 命中带章内字符偏移() {
        let fixture = fixture("offset");
        // "前面五个字" 恰好 5 个字符，故「目标」的章内偏移是 5。
        // 若实现用了**字节**偏移，中文每字 3 字节，这里会得到 15
        fixture.add_cached_chapter(0, "第一章", "前面五个字目标在后面");

        let hits = fixture
            .index
            .search_in_book(fixture.book_id, "目标")
            .expect("应能搜索");
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].char_offset, 5,
            "应是字符偏移而非字节偏移（字节口径会给出 15）"
        );
        assert!(hits[0].snippet.contains("目标"), "片段应含命中词");
    }

    #[test]
    fn 中文子串可搜() {
        // trigram 分词器的价值：unicode61 会把整段中文当一词，"主角"搜不到
        let fixture = fixture("cjk");
        fixture.add_cached_chapter(0, "第一章", "这是一段很长的中文正文，主角在中间登场。");

        let hits = fixture
            .index
            .search_in_book(fixture.book_id, "主角")
            .expect("应能搜索");
        assert_eq!(hits.len(), 1, "中文子串应能搜到（trigram 分词）");
    }

    #[test]
    fn 未缓存章节不被搜到() {
        // FR-019b 的核心约束：只检索已缓存内容
        let fixture = fixture("uncached");
        fixture.add_uncached_chapter(0, "第一章", "主角登场");

        let hits = fixture
            .index
            .search_in_book(fixture.book_id, "主角")
            .expect("应能搜索");
        assert!(
            hits.is_empty(),
            "未缓存的章节不该被搜到（否则会诱导上层去抓取）"
        );
    }

    #[test]
    fn 只搜指定书() {
        let fixture = fixture("per-book");
        fixture.add_cached_chapter(0, "第一章", "主角登场");

        // 另建一本同名的另一本书
        let other_book: i64 = fixture
            .db
            .write(|connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at)
                         VALUES (0, 'y', '另一本', 0)",
                        [],
                    )
                    .map_err(sql)?;
                Ok(connection.last_insert_rowid())
            })
            .expect("应能建书");

        let hits = fixture
            .index
            .search_in_book(other_book, "主角")
            .expect("应能搜索");
        assert!(hits.is_empty(), "不该搜到别的书的章节");
    }

    #[test]
    fn 一章多处命中各自成条() {
        let fixture = fixture("multi");
        fixture.add_cached_chapter(0, "第一章", "主角走了。主角又回来了。");

        let hits = fixture
            .index
            .search_in_book(fixture.book_id, "主角")
            .expect("应能搜索");
        assert_eq!(hits.len(), 2, "同一章的两处命中应各成一条");
        assert!(hits[0].char_offset < hits[1].char_offset, "偏移应递增");
    }

    #[test]
    fn 空查询返回空() {
        let fixture = fixture("empty-query");
        fixture.add_cached_chapter(0, "第一章", "内容");
        assert!(fixture
            .index
            .search_in_book(fixture.book_id, "")
            .expect("应能搜索")
            .is_empty());
        assert!(fixture
            .index
            .search_in_book(fixture.book_id, "   ")
            .expect("应能搜索")
            .is_empty());
    }

    #[test]
    fn 无结果不报错() {
        let fixture = fixture("no-hit");
        fixture.add_cached_chapter(0, "第一章", "主角登场");
        let hits = fixture
            .index
            .search_in_book(fixture.book_id, "完全不存在的词")
            .expect("应能搜索");
        assert!(hits.is_empty());
    }

    #[test]
    fn 特殊字符不被当成查询语法() {
        // 用户搜 `C++`、带引号的词不该让 FTS 报语法错
        let fixture = fixture("specials");
        fixture.add_cached_chapter(0, "第一章", "他写 C++ 代码，也写 \"引号\" 和 AND。");

        for query in ["C++", "\"引号\"", "AND"] {
            let hits = fixture
                .index
                .search_in_book(fixture.book_id, query)
                .unwrap_or_else(|error| panic!("搜 `{query}` 不该报错：{error}"));
            assert!(!hits.is_empty(), "`{query}` 应能搜到");
        }
    }

    #[test]
    fn 纯标点查询给空结果而非报错() {
        // 单字切分会丢掉标点，故"全是标点"的查询切完是空串。
        // 此时应给空结果——不能拿空串去 MATCH（FTS5 会报语法错），
        // 也不能报错（用户只是搜了个括号）。
        let fixture = fixture("punct-only");
        fixture.add_cached_chapter(0, "第一章", "内容里有括号（像这样）");

        for query in ["（括号）", "。，、", "　"] {
            let hits = fixture
                .index
                .search_in_book(fixture.book_id, query)
                .unwrap_or_else(|error| panic!("搜 `{query}` 不该报错：{error}"));
            assert!(hits.is_empty(), "纯标点查询应给空结果：`{query}`");
        }
    }

    #[test]
    fn 带连字符的词不被当成取反语法() {
        // FTS5 里 `-` 是取反运算符。若查询串未做处理，搜 "a-b" 会变成
        // "含 a 但不含 b"——而用户要的是**字面串** "a-b"。
        // 单字切分把 `-` 当标点丢掉，故查询退化为 "ab"…… 那也不对。
        // 正确做法：查不到就查不到（`a-b` 的字面串本就不在索引里），
        // 但**不能报错**、也不能返回语法错误。
        let fixture = fixture("hyphen");
        fixture.add_cached_chapter(0, "第一章", "编号 ab 和 a-b 都在这里");

        let result = fixture.index.search_in_book(fixture.book_id, "a-b");
        assert!(
            result.is_ok(),
            "带连字符的查询不该报语法错：{:?}",
            result.err()
        );
    }

    #[test]
    fn 重复索引同一章不产生重复命中() {
        let fixture = fixture("reindex");
        let chapter_id = fixture.add_cached_chapter(0, "第一章", "主角登场");
        // 再索引一次（模拟重新落缓存）
        fixture
            .index
            .index_chapter(chapter_id, "第一章", "主角登场")
            .expect("应能重索引");

        let hits = fixture
            .index
            .search_in_book(fixture.book_id, "主角")
            .expect("应能搜索");
        assert_eq!(hits.len(), 1, "重复索引不该让同一处命中两次");
    }

    #[test]
    fn 移除章节后不再命中() {
        let fixture = fixture("remove");
        let chapter_id = fixture.add_cached_chapter(0, "第一章", "主角登场");
        assert_eq!(fixture.index.indexed_count().expect("应能计数"), 1);

        fixture.index.remove_chapter(chapter_id).expect("应能移除");
        assert_eq!(fixture.index.indexed_count().expect("应能计数"), 0);
        assert!(fixture
            .index
            .search_in_book(fixture.book_id, "主角")
            .expect("应能搜索")
            .is_empty());
    }

    #[test]
    fn 移除整本书的索引() {
        let fixture = fixture("remove-book");
        fixture.add_cached_chapter(0, "第一章", "主角登场");
        fixture.add_cached_chapter(1, "第二章", "主角继续");

        let removed = fixture
            .index
            .remove_book(fixture.book_id)
            .expect("应能移除");
        assert_eq!(removed, 2);
        assert_eq!(
            fixture
                .index
                .indexed_count_of_book(fixture.book_id)
                .expect("应能计数"),
            0
        );
    }

    #[test]
    fn 索引计数按书统计() {
        let fixture = fixture("count");
        fixture.add_cached_chapter(0, "第一章", "甲");
        fixture.add_cached_chapter(1, "第二章", "乙");
        assert_eq!(
            fixture
                .index
                .indexed_count_of_book(fixture.book_id)
                .expect("应能计数"),
            2
        );
        assert_eq!(fixture.index.indexed_count().expect("应能计数"), 2);
    }

    #[test]
    fn 索引表可由_ensure_schema_重复调用() {
        let fixture = fixture("ensure");
        // 幂等：重复建表不该报错
        fixture.index.ensure_schema().expect("应能重复建表");
        fixture.index.ensure_schema().expect("应能重复建表");
    }

    #[test]
    fn 字符偏移计算正确处理多字节() {
        // 中文三字节，"前两个字符"在字节上跨 6 字节——
        // 若实现用了字节偏移，这里的断言会给出 6 而不是 2
        let offsets = find_all_char_offsets("甲乙丙丁", "丙");
        assert_eq!(offsets, vec![2]);

        // 允许重叠
        let overlapping = find_all_char_offsets("aaa", "aa");
        assert_eq!(overlapping, vec![0, 1]);
    }

    #[test]
    fn 片段折叠换行() {
        let content = "第一行\n第二行主角登场\n第三行";
        let offsets = find_all_char_offsets(content, "主角");
        assert_eq!(offsets.len(), 1);
        let snippet = snippet_around(content, offsets[0], "主角");
        assert!(!snippet.contains('\n'), "片段应折成单行：{snippet}");
        assert!(snippet.contains("主角"));
    }

    #[test]
    fn 查询被转义为短语() {
        // 口径：按单字切分（与索引侧同口径）后整体加引号成短语。
        // 短语约束是必需的——空格分隔是 AND 语义，"主是角"会被误命中。
        assert_eq!(fts_query("主角"), "\"主 角\"");
        assert_eq!(fts_query("abc"), "\"abc\"", "拉丁词整词保留");
    }

    #[test]
    fn 单字切分跳过空白与标点() {
        assert_eq!(segment_chars("主角 登场，故事。"), "主 角 登 场 故 事");
        assert_eq!(segment_chars("hello world"), "hello world");
        assert_eq!(segment_chars("混合 mixed 文本"), "混 合 mixed 文 本");
    }

    #[test]
    fn 短语约束要求相邻() {
        // 这条守的是"用短语而非 AND"这个决定：
        // "主是角"不该被 "主角" 搜到
        let fixture = fixture("phrase");
        fixture.add_cached_chapter(0, "第一章", "主是角");
        fixture.add_cached_chapter(1, "第二章", "这是主角");

        let hits = fixture
            .index
            .search_in_book(fixture.book_id, "主角")
            .expect("应能搜索");
        assert_eq!(hits.len(), 1, "只应命中连续出现的「主角」");
        assert_eq!(hits[0].chapter_index, 1);
    }
}
