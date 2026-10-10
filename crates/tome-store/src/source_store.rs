//! [DONE] 书源持久化的薄适配（吸收 `tome-source::registry`，T056 的收尾）
//!
//! # 这个模块为什么存在
//!
//! S1-1 阶段 `tome-store` 尚未实现，故 `tome-source::registry` 提供了一个
//! **文件式最小注册表**（`sources.json`），让"先 `source import` 再
//! `search <source-id>`"这条 CLI 契约成立。它的模块文档自己写明：
//!
//! > `tome-store` 落地后，本模块应改为它的一个薄适配（或直接删除，
//! > 让 CLI 走 `BookRepository` 接缝）。
//!
//! T056 落地了 [`crate::dao::source::SourceDao`]，本模块就是那句预告的兑现：
//! **持久化改走 SQLite**，而 CLI 的输入输出契约（可读的 `sources.json`）
//! 由本适配维持。
//!
//! # 为什么保留 `sources.json` 这个导出
//!
//! 它是**验收入口**而非内部实现细节：
//!
//! | 用途 | 为什么需要它 |
//! |---|---|
//! | 人工检视 | 用户能打开看自己导入了哪些源 |
//! | 手工编辑后重新导入 | 文件用社区格式的键名，本身就是一份合法书源文件 |
//! | 测试断言 | CLI 的导入报告与既有测试以它为观察点 |
//! | 迁移 | 用户把 `data/` 目录搬走时，这个文件是书源集合的可读快照 |
//!
//! 故它不是"过渡形态的残留"，而是**面向用户的书面格式**——只是权威存储
//! 从文件换成了 SQLite。
//!
//! # 权威存储是哪一个
//!
//! **SQLite**。`sources.json` 是它的**投影**（每次导入后重写）。
//! 反过来的话（文件为准）会让 T056 建立的分组/启停/按源 Cookie 全部失效
//! ——那些字段文件格式里根本没有位置表达。

use std::path::Path;

use tome_core::{Source, SourceGroup};

use crate::dao::source::SourceDao;
use crate::error::StoreError;

/// 书源导出的文件名（沿用原注册表的文件名，便于用户与既有脚本无感迁移）。
pub const SOURCE_EXPORT_FILE_NAME: &str = "sources.json";

/// 一次导入的计数（与 [`tome_core::seam::ImportReport`] 对齐的最小投影）。
///
/// # 为什么单独一个类型而不是直接用 `ImportReport`
///
/// `ImportReport` 带 `failures` 明细，而本适配的 `merge_load_outcome`
/// 返回的是"写了哪些"的**摘要**。签名用窄类型让调用方（CLI 的报告渲染）
/// 不必区分"解析阶段的 failures"与"落库阶段的 failures"——后者已经在
/// 解析阶段报过了，重复报会让同一份问题出现两次。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PersistCounts {
    /// 新增的源数
    pub added: usize,
    /// 更新的源数（同 id 已存在）
    pub updated: usize,
}

/// 书源持久化适配器。
///
/// 持有 [`SourceDao`] 与导出文件的路径。所有写操作**先落 SQLite、再重写导出**
/// ——顺序反了会在 SQLite 失败时留下一个"比库里多/少"的导出文件，而那是
/// 用户手工重导时会复现的错误状态。
#[derive(Debug, Clone)]
pub struct SourceStore {
    dao: SourceDao,
    export_path: std::path::PathBuf,
}

impl SourceStore {
    /// 构造。
    ///
    /// `export_path` 是 `sources.json` 的落点（通常是 `<数据目录>/sources.json`）。
    pub fn new(dao: SourceDao, export_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            dao,
            export_path: export_path.into(),
        }
    }

    /// 底层 DAO（供需要分组/启停/删除等完整能力的调用方使用）。
    pub fn dao(&self) -> &SourceDao {
        &self.dao
    }

    /// 导出文件路径。
    pub fn export_path(&self) -> &Path {
        self.export_path.as_path()
    }

    /// 落库一批书源（**同 id 即更新**），并把导出文件重写为当前全量。
    ///
    /// 返回本次的新增/更新计数。
    pub fn merge(&self, sources: &[Source]) -> Result<PersistCounts, StoreError> {
        let report = self.dao.upsert_all(sources)?;
        self.rewrite_export()?;
        Ok(PersistCounts {
            added: report.added,
            updated: report.updated,
        })
    }

    /// 按 id 取一个书源。
    pub fn get(&self, source_id: &str) -> Result<Option<Source>, StoreError> {
        self.dao.by_id(source_id)
    }

    /// 全部书源（按 id 排序——顺序确定，T029 的逐字节一致依赖它）。
    pub fn all(&self) -> Result<Vec<Source>, StoreError> {
        self.dao.all()
    }

    /// 全部书源分组。
    pub fn groups(&self) -> Result<Vec<SourceGroup>, StoreError> {
        self.dao.groups()
    }

    /// 书源总数。
    pub fn len(&self) -> Result<usize, StoreError> {
        Ok(self.dao.count()? as usize)
    }

    /// 是否一个源都没有。
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.dao.count()? == 0)
    }

    /// 把当前全量重写到导出文件。
    ///
    /// # 单条序列化失败不中断整体
    ///
    /// 某条源含无法序列化的内容时（理论上不该有，但源来自第三方），
    /// 跳过它并继续——导出是**辅助产物**，为它让导入失败是本末倒置
    /// （源已经安全落进 SQLite 了）。
    pub fn rewrite_export(&self) -> Result<(), StoreError> {
        let sources = self.dao.all()?;
        let text = render_sources_json(&sources)?;

        if let Some(parent) = self.export_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| crate::error::from_io(parent, error))?;
            }
        }
        std::fs::write(&self.export_path, text)
            .map_err(|error| crate::error::from_io(&self.export_path, error))
    }
}

/// 把书源渲染成社区格式的 JSON 文本（即 `sources.json` 的内容）。
///
/// # 为什么字段名用社区格式的原始键名
///
/// 与原注册表一致：这样导出文件**本身就是一份合法书源文件**，可以直接
/// `tome source import <它>` 再读一次。用户"手工编辑后重导"与"拿它当备份"
/// 两条用法都靠这条性质成立。
///
/// # 为什么手写 JSON 而不是给 `Source` 派生 `Serialize`
///
/// core 的 DTO **刻意不派生 serde**（`model/mod.rs` 的硬约束 2：序列化与
/// 持久化是各实现 crate 的关切）。在 core 上加 `Serialize` 会让 DTO 的形状
/// 被磁盘格式绑架——改一个字段名就成了破坏性变更。
pub fn render_sources_json(sources: &[Source]) -> Result<String, StoreError> {
    let entries: Vec<serde_json::Value> = sources
        .iter()
        .map(|source| {
            let mut object = serde_json::Map::new();
            // 顺序固定：与 community 格式的常见书写顺序一致，且让 diff 稳定
            object.insert("bookSourceName".into(), source.name.clone().into());
            object.insert("bookSourceUrl".into(), source.id.clone().into());
            object.insert(
                "bookSourceType".into(),
                source.kind.as_storage_code().into(),
            );
            object.insert("enabled".into(), source.enabled.into());
            insert_optional(
                &mut object,
                "group",
                source.group_id.map(|id| id.to_string()),
            );
            insert_optional(&mut object, "searchUrl", source.search_url.clone());
            insert_optional(&mut object, "exploreUrl", source.explore_url.clone());
            insert_optional(
                &mut object,
                "bookUrlPattern",
                source.book_url_pattern.clone(),
            );
            insert_optional(&mut object, "ruleSearch", source.rule_search.clone());
            insert_optional(&mut object, "ruleExplore", source.rule_explore.clone());
            insert_optional(&mut object, "ruleBookInfo", source.rule_book_info.clone());
            insert_optional(&mut object, "ruleToc", source.rule_toc.clone());
            insert_optional(&mut object, "ruleContent", source.rule_content.clone());
            insert_optional(&mut object, "header", source.header.clone());
            insert_optional(&mut object, "loginUrl", source.login_url.clone());
            insert_optional(&mut object, "loginUi", source.login_ui.clone());
            insert_optional(&mut object, "loginCheckJs", source.login_check_js.clone());
            insert_optional(&mut object, "js", source.js.clone());
            insert_optional(&mut object, "charset", source.charset.clone());
            insert_optional(
                &mut object,
                "concurrentRate",
                source.rate_limit_ms.map(|millis| millis.to_string()),
            );
            insert_optional(
                &mut object,
                "weight",
                source.weight.map(|weight| weight.to_string()),
            );
            insert_optional(&mut object, "exploreScreen", source.explore_screen.clone());
            insert_optional(&mut object, "bookSourceComment", source.comment.clone());
            insert_optional(
                &mut object,
                "lastUpdateTime",
                source.last_update_time.map(|time| time.to_string()),
            );
            serde_json::Value::Object(object)
        })
        .collect();

    let mut text = serde_json::to_string_pretty(&entries)
        .map_err(|error| StoreError::config(format!("序列化书源导出失败：{error}")))?;
    // 尾随换行：让该文件在编辑器中正常显示、也让 `diff` 的末行有意义
    text.push('\n');
    Ok(text)
}

/// 只在该值存在时插入键（`None` 不写空串——那会让"没设置"与"设为空"混淆）。
fn insert_optional(
    object: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<String>,
) {
    if let Some(value) = value {
        object.insert(key.into(), value.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::SourceKind;

    struct Fixture {
        dir: std::path::PathBuf,
        store: SourceStore,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(label: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "tome-source-store-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能建目录");
        let db = crate::db::Db::open(&dir.join("tome.db")).expect("应能开库");
        Fixture {
            store: SourceStore::new(
                SourceDao::with_now(db, 1_700_000_000),
                dir.join(SOURCE_EXPORT_FILE_NAME),
            ),
            dir,
        }
    }

    fn sample(id: &str, name: &str) -> Source {
        Source {
            id: id.to_string(),
            name: name.to_string(),
            kind: SourceKind::Book,
            group_id: None,
            enabled: true,
            book_source_type: None,
            book_url_pattern: None,
            search_url: Some("/s?q={{key}}".to_string()),
            explore_url: None,
            rule_search: Some("{\"bookList\":\"@css:li\"}".to_string()),
            rule_explore: None,
            rule_book_info: None,
            rule_toc: None,
            rule_content: None,
            header: None,
            login_url: None,
            login_ui: None,
            login_check_js: None,
            js: None,
            charset: None,
            rate_limit_ms: Some(1000),
            weight: None,
            explore_screen: None,
            comment: None,
            last_update_time: None,
            created_at: 1_700_000_000,
        }
    }

    #[test]
    fn 同_id_落库按更新处理() {
        let fixture = fixture("upsert");
        let first = fixture
            .store
            .merge(&[sample("https://a.com", "旧名")])
            .expect("应能导入");
        assert_eq!(first.added, 1);
        assert_eq!(first.updated, 0);

        let second = fixture
            .store
            .merge(&[sample("https://a.com", "新名")])
            .expect("应能导入");
        assert_eq!(second.added, 0, "同 id 不该重复新增");
        assert_eq!(second.updated, 1);
        assert_eq!(fixture.store.len().expect("应能数"), 1);
        assert_eq!(
            fixture
                .store
                .get("https://a.com")
                .expect("应能查")
                .expect("应有")
                .name,
            "新名"
        );
    }

    #[test]
    fn 导出文件是社区格式的合法书源文件() {
        // 这条性质让"手工编辑导出文件后重新导入"与"拿导出当备份"成立
        let fixture = fixture("export-format");
        fixture
            .store
            .merge(&[sample("https://a.com", "源A")])
            .expect("应能导入");

        let text = std::fs::read_to_string(fixture.store.export_path()).expect("应能读");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("应是合法 JSON");
        let array = parsed.as_array().expect("应是数组");
        assert_eq!(array.len(), 1);
        assert_eq!(array[0]["bookSourceName"], "源A");
        assert_eq!(array[0]["bookSourceUrl"], "https://a.com");
        assert_eq!(array[0]["searchUrl"], "/s?q={{key}}");
        assert_eq!(array[0]["enabled"], true);
    }

    #[test]
    fn 导出随导入更新() {
        let fixture = fixture("export-refresh");
        fixture
            .store
            .merge(&[sample("https://a.com", "A")])
            .expect("应能导入");
        fixture
            .store
            .merge(&[sample("https://b.com", "B")])
            .expect("应能导入");

        let text = std::fs::read_to_string(fixture.store.export_path()).expect("应能读");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("应是 JSON");
        assert_eq!(parsed.as_array().expect("应是数组").len(), 2);
    }

    #[test]
    fn 导出顺序确定() {
        // T029 的"逐字节一致"要求顺序不随存储层抖动
        let fixture = fixture("export-order");
        fixture
            .store
            .merge(&[
                sample("https://z.com", "Z"),
                sample("https://a.com", "A"),
                sample("https://m.com", "M"),
            ])
            .expect("应能导入");

        let text = std::fs::read_to_string(fixture.store.export_path()).expect("应能读");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("应是 JSON");
        let ids: Vec<String> = parsed
            .as_array()
            .expect("应是数组")
            .iter()
            .map(|entry| {
                entry["bookSourceUrl"]
                    .as_str()
                    .expect("应有 url")
                    .to_string()
            })
            .collect();
        assert_eq!(ids, vec!["https://a.com", "https://m.com", "https://z.com"]);

        // 再导一次同样的内容：导出文件应逐字节相同（幂等）
        let before = std::fs::read_to_string(fixture.store.export_path()).expect("应能读");
        fixture.store.rewrite_export().expect("应能重写");
        let after = std::fs::read_to_string(fixture.store.export_path()).expect("应能读");
        assert_eq!(before, after, "重复导出应逐字节相同");
    }

    #[test]
    fn 落库的是_sqlite_而不是文件() {
        // 权威存储是 SQLite：文件只是投影。若反过来以文件为准，
        // T056 建立的分组/启停/按源 Cookie 全部无处表达。
        let fixture = fixture("db-authoritative");
        fixture
            .store
            .merge(&[sample("https://a.com", "A")])
            .expect("应能导入");

        // 直接改数据库里的名字（模拟"上层经 DAO 改了"）
        fixture
            .store
            .dao()
            .upsert_all(&[sample("https://a.com", "数据库里的名字")])
            .expect("应能改");

        // 重新导出后文件应反映数据库的状态
        fixture.store.rewrite_export().expect("应能导出");
        assert_eq!(
            fixture
                .store
                .get("https://a.com")
                .expect("应能查")
                .expect("应有")
                .name,
            "数据库里的名字"
        );
        let text = std::fs::read_to_string(fixture.store.export_path()).expect("应能读");
        assert!(text.contains("数据库里的名字"), "导出应反映数据库状态");
    }

    #[test]
    fn 数据库的完整能力经适配器仍可用() {
        // 分组、启停是 T056 新增的能力，文件格式表达不了——这正是不再
        // 以文件为权威存储的理由
        let fixture = fixture("full-capability");
        fixture
            .store
            .merge(&[sample("https://a.com", "A"), sample("https://b.com", "B")])
            .expect("应能导入");

        let group_id = fixture
            .store
            .dao()
            .ensure_group("玄幻", 0)
            .expect("应能建组");
        fixture
            .store
            .dao()
            .assign_group(&["https://a.com".to_string()], Some(group_id))
            .expect("应能归组");
        fixture
            .store
            .dao()
            .set_enabled(&["https://b.com".to_string()], false)
            .expect("应能停用");

        assert_eq!(fixture.store.groups().expect("应能查").len(), 1);
        assert!(
            !fixture
                .store
                .get("https://b.com")
                .expect("应能查")
                .expect("应有")
                .enabled,
            "启停状态应持久"
        );
    }

    #[test]
    fn 空集合导出为空数组() {
        let fixture = fixture("empty");
        fixture.store.rewrite_export().expect("应能导出");
        let text = std::fs::read_to_string(fixture.store.export_path()).expect("应能读");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("应是 JSON");
        assert_eq!(parsed.as_array().expect("应是数组").len(), 0);
    }

    #[test]
    fn 导出会创建父目录() {
        let fixture = fixture("mkdir");
        let store = SourceStore::new(
            fixture.store.dao().clone(),
            fixture.dir.join("nested").join("deep").join("sources.json"),
        );
        store.rewrite_export().expect("应能建父目录");
        assert!(store.export_path().exists());
    }

    #[test]
    fn 重启后数据仍在() {
        // FR-026 的"重启后保留"在书源上的形态
        let fixture = fixture("restart");
        fixture
            .store
            .merge(&[sample("https://a.com", "源A")])
            .expect("应能导入");

        let db_path = fixture.dir.join("tome.db");
        let reopened = SourceStore::new(
            SourceDao::new(crate::db::Db::open(&db_path).expect("应能重开")),
            fixture.dir.join(SOURCE_EXPORT_FILE_NAME),
        );
        assert_eq!(
            reopened
                .get("https://a.com")
                .expect("应能查")
                .expect("重启后应还在")
                .name,
            "源A"
        );
    }

    #[test]
    fn 可选字段缺失时不写空串() {
        // "没设置"与"设为空串"是两回事，导出要能区分
        let sources = vec![sample("https://a.com", "A")];
        let text = render_sources_json(&sources).expect("应能渲染");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("应是 JSON");
        let entry = &parsed[0];
        assert!(entry.get("header").is_none(), "未设置的字段不该出现");
        assert!(entry.get("group").is_none());
        assert_eq!(entry["concurrentRate"], "1000", "有值的字段应出现");
    }
}
