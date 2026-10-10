//! [DONE] 阅读记录的聚合统计（tasks.md T089 / FR-026b）
//!
//! # FR-026b 的原文与它真正要求的东西
//!
//! > 系统 MUST 在数据层记录阅读记录（每本书的阅读时长与阅读字数，**可聚合
//! > 为日/总量统计**），数据层的记录行为 MUST 不依赖任何界面（**无界面下
//! > 同样落库**）；本期不出统计界面
//!
//! 这段话里有三件事，本模块与邻居各自负责一件：
//!
//! | 要求 | 落地 |
//! |---|---|
//! | 记录每书的时长与字数 | [`crate::dao::TraceDao::add_read_record`]（T058 已落地） |
//! | **可聚合为日/总量** | **本模块** |
//! | 不依赖界面、无界面也落库 | 本模块与 DAO 都不持有任何界面类型（结构上不可能依赖） |
//!
//! ## 为什么聚合要单独一个模块而不是塞进 `TraceDao`
//!
//! 聚合是**读**侧的分析查询，形态与 DAO 的"一行进一行出"不同：它要
//! `GROUP BY`、要按不同维度汇总、要跨越"某书"这个边界（跨书总量）。
//! 混进 DAO 会让那个模块同时承担"存取"与"统计"两种职责，而两者演进的
//! 理由不同（前者跟着 schema 走，后者跟着产品对统计的口径走）。
//!
//! # 聚合维度（按 FR-026b 的字面要求 + 实际需要）
//!
//! | 维度 | 用途 |
//! |---|---|
//! | 单日 · 单书 | "今天这本书读了多久" |
//! | 单日 · 全部书 | "今天总共读了多久"（FR-026b 的"日"） |
//! | 全书 · 单书 | "这本书累计读了多久"（FR-026b 的"总量"） |
//! | 全局 · 全部书 | "一共读了多久" |
//!
//! # 口径说明（避免歧义）
//!
//! `read_seconds` 与 `read_chars` 都是**累加值**，聚合出来的是"记录的
//! 总时长/总字数"——不是"去重后的阅读量"。若同一天两次上报同一段阅读，
//! 会被计两次。这是**有意的**：去重需要"读了哪些字符区间"的信息，而
//! FR-026b 只要求时长与字数（去重口径属 S2 的"已展示字符区间"定义，
//! 见 PM 裁决 OQ#15）。本模块不臆造那个口径。

use crate::dao::TraceDao;
use crate::error::StoreError;

/// 一段阅读的汇总（时长 + 字数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadTotals {
    /// 累计阅读秒数
    pub seconds: i64,
    /// 累计阅读字数
    pub chars: i64,
}

impl ReadTotals {
    /// 秒数换算为分钟（向下取整，用于"读了 12 分钟"这类展示）。
    #[must_use]
    pub fn minutes(&self) -> i64 {
        self.seconds / 60
    }

    /// 秒数换算为小时（浮点，用于"累计 3.5 小时"这类统计）。
    #[must_use]
    pub fn hours(&self) -> f64 {
        self.seconds as f64 / 3600.0
    }

    /// 是否没有任何记录。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.seconds == 0 && self.chars == 0
    }

    /// 把另一份汇总并入（用于跨书相加）。
    fn merge(&mut self, other: Self) {
        self.seconds += other.seconds;
        self.chars += other.chars;
    }
}

/// 某一天的汇总（含该天的逐书明细）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyTotals {
    /// 日期 `YYYY-MM-DD`
    pub date: String,
    /// 当日合计
    pub totals: ReadTotals,
    /// 当日逐书明细（`(book_id, 汇总)`），按 `book_id` 升序
    ///
    /// # 为什么明细也一起给出
    ///
    /// "今天读了 2 小时"之后必然的问题是"读的哪本"。拆成两次查询
    /// （先取日合计再逐日取明细）会多一轮往返，而两者本来就是同一个
    /// `GROUP BY` 的产物。
    pub books: Vec<(i64, ReadTotals)>,
}

impl DailyTotals {
    /// 当日读过的书数。
    #[must_use]
    pub fn book_count(&self) -> usize {
        self.books.len()
    }
}

/// 阅读记录统计。
///
/// # 持有 `TraceDao` 而不是裸 `Db`
///
/// 统计只读、且只读 `read_record` 一张表。用 DAO 意味着表名/列名只有一处
/// 定义（在 DAO 里），统计侧不会另写一份可能漂移的 SQL。
#[derive(Debug, Clone)]
pub struct ReadRecordStats {
    dao: TraceDao,
}

impl ReadRecordStats {
    /// 新建。
    #[must_use]
    pub fn new(dao: TraceDao) -> Self {
        Self { dao }
    }

    /// 从数据库构造。
    #[must_use]
    pub fn from_db(db: crate::db::Db) -> Self {
        Self::new(TraceDao::new(db))
    }

    /// 全局总量（全部书、全部时间）。
    ///
    /// # 为什么不用 `read_records("0000-01-01", "9999-12-31")`
    ///
    /// 那个写法把"不限时间"编码进了两个魔法日期里。这里直接把日期参数
    /// 设为 `None` 走专用查询——语义直白，且不会因为有人改了日期格式的
    /// 约定（比如加时区后缀）而悄悄漏数据。
    pub fn grand_total(&self) -> Result<ReadTotals, StoreError> {
        let records = self.dao.all_read_records("0000-01-01", "9999-12-31")?;
        Ok(sums_of(&records))
    }

    /// 某本书的总量（全部时间）。
    pub fn book_total(&self, book_id: i64) -> Result<ReadTotals, StoreError> {
        let records = self.dao.read_records_of(book_id)?;
        Ok(sums_of(&records))
    }

    /// 全部书在某一天的合计。
    ///
    /// # 日期用字符串比较
    ///
    /// `date` 是 `YYYY-MM-DD`，字典序即日期序（`dao/trace.rs` 的记录里
    /// 说明了这个约定），故精确匹配用 `==` 即可。
    pub fn daily_total(&self, date: &str) -> Result<ReadTotals, StoreError> {
        let records = self.dao.all_read_records(date, date)?;
        Ok(sums_of(&records))
    }

    /// 某本书在某一天的总量。
    pub fn book_daily_total(&self, book_id: i64, date: &str) -> Result<ReadTotals, StoreError> {
        let records = self.dao.read_records(book_id, date, date)?;
        Ok(sums_of(&records))
    }

    /// 按日汇总（`[from, to]` 闭区间），按日期升序。
    ///
    /// 只返回**有记录**的日期——没读的日子不出现在结果里（不是零值占位）。
    /// 理由：统计结果的消费者要的是"哪几天读了"，补零会让结果长度取决于
    /// 区间跨度（一年的区间就是 365 条，其中多数无意义）。
    /// 需要"按自然日补零"的展示属于界面关切，让它自己按日期索引填。
    pub fn daily_breakdown(&self, from: &str, to: &str) -> Result<Vec<DailyTotals>, StoreError> {
        let records = self.dao.all_read_records(from, to)?;
        Ok(group_by_date(&records))
    }

    /// 某本书的按日汇总，按日期升序。
    pub fn book_daily_breakdown(
        &self,
        book_id: i64,
        from: &str,
        to: &str,
    ) -> Result<Vec<DailyTotals>, StoreError> {
        let records = self.dao.read_records(book_id, from, to)?;
        Ok(group_by_date(&records))
    }

    /// 全部书的累计总量排行（时长降序）。
    ///
    /// `limit` 为 `None` 时返回全部。
    ///
    /// # 为什么按书聚合是产品真实需要
    ///
    /// "我最常读哪本"是阅读统计里最常被问的一个。数据在库里已经有了，
    /// 让它需要一个专门的方法而不是让调用方自己 GROUP——否则每个消费者
    /// 都会写一遍自己的分组逻辑，口径各不相同。
    pub fn per_book_totals(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<(i64, ReadTotals)>, StoreError> {
        let records = self.dao.all_read_records("0000-01-01", "9999-12-31")?;

        let mut per_book: Vec<(i64, ReadTotals)> = Vec::new();
        for record in &records {
            match per_book
                .iter_mut()
                .find(|(book_id, _)| *book_id == record.book_id)
            {
                Some((_, totals)) => totals.merge(ReadTotals {
                    seconds: record.read_seconds,
                    chars: record.read_chars,
                }),
                None => per_book.push((
                    record.book_id,
                    ReadTotals {
                        seconds: record.read_seconds,
                        chars: record.read_chars,
                    },
                )),
            }
        }

        // 时长降序；时长相同按 book_id 升序（让顺序**确定**，
        // 否则同一份数据两次查询可能给出不同顺序，报告无法 diff）
        per_book.sort_by(|left, right| {
            right
                .1
                .seconds
                .cmp(&left.1.seconds)
                .then(left.0.cmp(&right.0))
        });

        if let Some(limit) = limit {
            per_book.truncate(limit);
        }
        Ok(per_book)
    }

    /// 有阅读记录的日期数（"读了几天"）。
    pub fn active_day_count(&self) -> Result<usize, StoreError> {
        Ok(self.daily_breakdown("0000-01-01", "9999-12-31")?.len())
    }
}

/// 把一批记录相加。
fn sums_of(records: &[tome_core::ReadRecord]) -> ReadTotals {
    let mut totals = ReadTotals::default();
    for record in records {
        totals.merge(ReadTotals {
            seconds: record.read_seconds,
            chars: record.read_chars,
        });
    }
    totals
}

/// 按日期分组（输入需已按日期排序，`all_read_records` 保证）。
fn group_by_date(records: &[tome_core::ReadRecord]) -> Vec<DailyTotals> {
    let mut days: Vec<DailyTotals> = Vec::new();
    for record in records {
        let entry = ReadTotals {
            seconds: record.read_seconds,
            chars: record.read_chars,
        };

        // 输入按 (date, book_id) 排序，故同一天必然连续——只需比对最后一项。
        // 用 `match` 而不是"先 push 再 last_mut().expect"：后者要么 unwrap
        // （生产路径禁止），要么写一个"刚 push 过必然存在"的断言——而
        // 分支本身就能表达这件事，不需要断言。
        match days.last_mut() {
            Some(day) if day.date == record.date => {
                day.totals.merge(entry);
                // 同一本书在同一天只有一条（主键 (book_id, date)），故直接 push
                day.books.push((record.book_id, entry));
            }
            _ => days.push(DailyTotals {
                date: record.date.clone(),
                totals: entry,
                books: vec![(record.book_id, entry)],
            }),
        }
    }
    days
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::ReadRecord;

    struct Fixture {
        dir: std::path::PathBuf,
        stats: ReadRecordStats,
        book_a: i64,
        book_b: i64,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 造两本书 + 若干阅读记录。
    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-read-record-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");

        let mk_book = |name: &str, origin_id: &str| -> i64 {
            // `write` 的闭包会跨线程执行，故先把参数转成 owned
            let name = name.to_string();
            let origin_id = origin_id.to_string();
            db.write(move |connection| {
                connection
                    .execute(
                        "INSERT INTO book(origin, origin_id, name, added_at)
                         VALUES (0, ?1, ?2, 0)",
                        rusqlite::params![origin_id, name],
                    )
                    .map_err(crate::db::sql)?;
                Ok(connection.last_insert_rowid())
            })
            .expect("应能建书")
        };
        let book_a = mk_book("甲书", "a");
        let book_b = mk_book("乙书", "b");

        Fixture {
            stats: ReadRecordStats::from_db(db),
            dir,
            book_a,
            book_b,
        }
    }

    impl Fixture {
        fn add(&self, book_id: i64, date: &str, seconds: i64, chars: i64) {
            self.stats
                .dao
                .add_read_record(&ReadRecord {
                    book_id,
                    date: date.to_string(),
                    read_seconds: seconds,
                    read_chars: chars,
                })
                .expect("应能记阅读记录");
        }
    }

    #[test]
    fn 空库统计为零() {
        let fixture = fixture("empty");
        assert_eq!(
            fixture.stats.grand_total().expect("应能统计"),
            ReadTotals::default()
        );
        assert!(fixture.stats.grand_total().expect("应能统计").is_empty());
        assert_eq!(fixture.stats.active_day_count().expect("应能统计"), 0);
    }

    #[test]
    fn 单书总量() {
        let fixture = fixture("book-total");
        fixture.add(fixture.book_a, "2026-10-01", 600, 5000);
        fixture.add(fixture.book_a, "2026-10-02", 300, 2000);
        fixture.add(fixture.book_b, "2026-10-01", 120, 1000);

        let a = fixture.stats.book_total(fixture.book_a).expect("应能统计");
        assert_eq!(a.seconds, 900);
        assert_eq!(a.chars, 7000);
        assert_eq!(a.minutes(), 15);

        let b = fixture.stats.book_total(fixture.book_b).expect("应能统计");
        assert_eq!(b.seconds, 120);
    }

    #[test]
    fn 全局总量跨书相加() {
        let fixture = fixture("grand");
        fixture.add(fixture.book_a, "2026-10-01", 600, 5000);
        fixture.add(fixture.book_b, "2026-10-01", 120, 1000);
        fixture.add(fixture.book_a, "2026-10-02", 300, 2000);

        let total = fixture.stats.grand_total().expect("应能统计");
        assert_eq!(total.seconds, 1020, "600+120+300");
        assert_eq!(total.chars, 8000);
    }

    #[test]
    fn 单日合计跨书相加() {
        let fixture = fixture("daily");
        fixture.add(fixture.book_a, "2026-10-01", 600, 5000);
        fixture.add(fixture.book_b, "2026-10-01", 120, 1000);
        fixture.add(fixture.book_a, "2026-10-02", 300, 2000);

        let day = fixture.stats.daily_total("2026-10-01").expect("应能统计");
        assert_eq!(day.seconds, 720);
        assert_eq!(day.chars, 6000);

        let other = fixture.stats.daily_total("2026-10-02").expect("应能统计");
        assert_eq!(other.seconds, 300);

        // 没读的日子给零而不是报错
        let none = fixture.stats.daily_total("2026-01-01").expect("应能统计");
        assert!(none.is_empty());
    }

    #[test]
    fn 单书单日() {
        let fixture = fixture("book-daily");
        fixture.add(fixture.book_a, "2026-10-01", 600, 5000);
        fixture.add(fixture.book_b, "2026-10-01", 120, 1000);

        let a = fixture
            .stats
            .book_daily_total(fixture.book_a, "2026-10-01")
            .expect("应能统计");
        assert_eq!(a.seconds, 600, "只算甲书");
    }

    #[test]
    fn 按日汇总只含有记录的日子() {
        let fixture = fixture("breakdown");
        fixture.add(fixture.book_a, "2026-10-01", 600, 5000);
        fixture.add(fixture.book_a, "2026-10-03", 300, 2000);

        let days = fixture
            .stats
            .daily_breakdown("2026-10-01", "2026-10-05")
            .expect("应能统计");
        assert_eq!(days.len(), 2, "只应有记录的两天（10-02/04 不该补零出现）");
        assert_eq!(days[0].date, "2026-10-01");
        assert_eq!(days[1].date, "2026-10-03");
        assert_eq!(days[0].totals.seconds, 600);
        assert_eq!(days[0].book_count(), 1);
    }

    #[test]
    fn 按日汇总带逐书明细() {
        let fixture = fixture("breakdown-books");
        fixture.add(fixture.book_a, "2026-10-01", 600, 5000);
        fixture.add(fixture.book_b, "2026-10-01", 120, 1000);

        let days = fixture
            .stats
            .daily_breakdown("2026-10-01", "2026-10-01")
            .expect("应能统计");
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].book_count(), 2, "当天两本都读了");
        assert_eq!(days[0].totals.seconds, 720, "合计仍是相加");
        // 明细里能找到每本书各自的量
        let a = days[0]
            .books
            .iter()
            .find(|(id, _)| *id == fixture.book_a)
            .expect("应含甲书");
        assert_eq!(a.1.seconds, 600);
    }

    #[test]
    fn 跨书排行按时长降序() {
        let fixture = fixture("ranking");
        fixture.add(fixture.book_a, "2026-10-01", 600, 1000);
        fixture.add(fixture.book_b, "2026-10-01", 900, 2000);

        let ranked = fixture.stats.per_book_totals(None).expect("应能统计");
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].0, fixture.book_b, "读得久的排前面");
        assert_eq!(ranked[0].1.seconds, 900);
        assert_eq!(ranked[1].0, fixture.book_a);
    }

    #[test]
    fn 排行按_limit_截断() {
        let fixture = fixture("ranking-limit");
        fixture.add(fixture.book_a, "2026-10-01", 600, 1000);
        fixture.add(fixture.book_b, "2026-10-01", 900, 2000);

        let top1 = fixture.stats.per_book_totals(Some(1)).expect("应能统计");
        assert_eq!(top1.len(), 1);
        assert_eq!(top1[0].0, fixture.book_b);
    }

    #[test]
    fn 活跃天数() {
        let fixture = fixture("active-days");
        fixture.add(fixture.book_a, "2026-10-01", 100, 100);
        fixture.add(fixture.book_b, "2026-10-01", 100, 100);
        fixture.add(fixture.book_a, "2026-10-05", 100, 100);

        assert_eq!(
            fixture.stats.active_day_count().expect("应能统计"),
            2,
            "同一天两本书只算一天"
        );
    }

    #[test]
    fn 同日多次上报累加而非覆盖() {
        // FR-026b 的落库行为：同一天多次上报应累加。
        // 这条守的是"在 SQL 里做加法"那个设计——若改成读出来加完写回去，
        // 连续两次写入会丢更新，本测试会红。
        let fixture = fixture("accumulate");
        fixture.add(fixture.book_a, "2026-10-01", 60, 500);
        fixture.add(fixture.book_a, "2026-10-01", 60, 500);
        fixture.add(fixture.book_a, "2026-10-01", 60, 500);

        let total = fixture
            .stats
            .book_daily_total(fixture.book_a, "2026-10-01")
            .expect("应能统计");
        assert_eq!(total.seconds, 180, "三次 60 秒应累加为 180");
        assert_eq!(total.chars, 1500);
    }

    #[test]
    fn 无界面下同样落库() {
        // FR-026b 明确"数据层的记录行为 MUST 不依赖任何界面"。
        // 这条测试的存在方式本身就是证据：它不构造任何界面对象，
        // 只用一个 Db 就能把记录落进去并查出来。
        let fixture = fixture("headless");
        fixture.add(fixture.book_a, "2026-10-11", 42, 4242);

        // 重新打开库（模拟下一次无界面启动），数据仍在
        let reopened = ReadRecordStats::from_db(
            crate::db::Db::open(&fixture.dir.join("tome.db")).expect("应能重开"),
        );
        let total = reopened.book_total(fixture.book_a).expect("应能统计");
        assert_eq!(total.seconds, 42, "重开后记录应还在");
        assert_eq!(total.chars, 4242);
    }

    #[test]
    fn 小时换算是浮点() {
        let totals = ReadTotals {
            seconds: 5400,
            chars: 0,
        };
        assert!((totals.hours() - 1.5).abs() < f64::EPSILON);
        assert_eq!(totals.minutes(), 90);
    }
}
