//! # [WIP] tome-store —— 存储层
//!
//! 负责全部本地数据持久化：SQLite actor（单写者串行化）、schema 迁移器、各表 DAO、
//! 书籍 Pack 容器、图片缓存，以及配置的类型化注入。
//!
//! ## 在本项目中的位置
//!
//! 实现 `tome-core` 定义的 `StoreHost` 接缝（`seams.md` §4）与 `BookRepository`
//! 接缝（`seams.md` §3）；`tome-source` / `tome-local` 经它读写数据。
//!
//! ## 护栏
//!
//! - 数据库查询全部参数化（宪法原则 V）。
//! - **缓存写入顺序不可颠倒**：内容追加落盘并 fsync **先于**索引事务提交，
//!   反过来会产生悬空引用（FR-030）。
//! - 阅读痕迹（高亮/批注/书签）一律以**字符区间**表达，禁存渲染坐标（FR-028）。
//! - 缓存清除 MUST NOT 影响个人阅读数据（FR-031 / SC-015）。
//!
//! ## 关键入口
//!
//! | 模块 | 职责 | 任务 |
//! |---|---|---|
//! | [`db`] | SQLite actor 线程 + 迁移器 | T053 |
//! | [`schema`] | v1 schema 与迁移脚本 | T053 |
//! | [`host`] | `StoreHost` 实现（JS 沙箱的存储面） | T054 |
//! | [`datadir`] | 数据目录解析与启动期检测 | T055 |
//! | [`dao`] | 各表 DAO | T056-T059 |
//! | [`pack`] | 章节缓存容器（崩溃安全） | T060 |
//! | [`image_cache`] | 图片缓存（可独立清理） | T061 |
//! | [`config`] | 配置 TOML 加载与原子写 | T062 |
//!
//! [WIP] 本 crate 的 T053-T062 已落地；`BookRepository` 接缝的完整实现
//! （备份导入导出、书内搜索）属后续切片 T087-T091。

// ## 关于 unsafe
//
// 本 crate 只有**一处** unsafe：`datadir::free_bytes` 调用平台的"取卷可用
// 空间"接口（Win32 的 `GetDiskFreeSpaceExW` / POSIX 的 `statvfs`）。它是
// FR-043「磁盘空间不足时明确报告」的唯一实现手段，没有安全的 std 等价物
// （`std::fs` 至今不提供卷容量查询）。
//
// 用 `deny` 而非 `forbid`：前者允许在**那一处**用 `#[expect(unsafe_code)]`
// 显式豁免并附 SAFETY 注释；后者连豁免都不允许，会把唯一需要 FFI 的地方
// 逼成"绕开空间检测"，那等于放弃 FR-043。
#![deny(unsafe_code)]

pub mod config;
pub mod dao;
pub mod datadir;
pub mod db;
pub mod error;
pub mod host;
pub mod image_cache;
pub mod pack;
pub mod schema;
pub mod source_store;

pub use datadir::DataDir;
pub use db::Db;
pub use error::StoreError;
pub use host::SqliteStoreHost;
pub use image_cache::ImageCache;
pub use pack::{EvictionReport, PackLocation, PackStore, PackWrite};
pub use source_store::SourceStore;

/// 当前 Unix 时间戳（秒）。
///
/// # 为什么容忍系统时钟异常
///
/// 时间戳回退（NTP 校正、用户手动改、跨时区搬迁）在桌面场景很常见。
/// 这类时刻**不该**让写操作失败——时间戳只是排序与淘汰的依据，取一个
/// "看起来不对但自洽"的值远比丢失用户数据好。故这里只在系统时间早于
/// Unix 纪元（明显异常）时归零，其余取值一律放行。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
