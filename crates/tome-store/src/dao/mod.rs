//! # DAO —— 各表的读写（tasks.md T056-T059）
//!
//! 每个 DAO 持一个 [`crate::db::Db`]（廉价克隆），方法取 `&self`：写走 actor
//! 通道、读走线程局部只读连接。DAO 不做跨表的业务编排（那属管线与组合根），
//! 只保证"一条记录进、一条记录出"的形状与约束正确。
//!
//! | 模块 | 表 | 任务 |
//! |---|---|---|
//! | [`source`] | `source` / `source_group` | T056 |
//! | [`book`] | `book` / `chapter` / `library_root` | T057 |
//! | [`trace`] | `read_progress` / `read_record` / `bookmark` / `highlight` / `memo` | T058 |
//! | [`misc`] | 规则四类 / `search_keyword` / `http_log` / `cookie` | T059 |

pub mod book;
pub mod misc;
pub mod read_record;
pub mod source;
pub mod trace;

pub use book::BookDao;
pub use misc::MiscDao;
pub use read_record::{DailyTotals, ReadRecordStats, ReadTotals};
pub use source::SourceDao;
pub use trace::TraceDao;
