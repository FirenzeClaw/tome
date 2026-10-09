//! [DONE] 书源的本地注册表（S1-1 阶段的轻量持久化）
//!
//! # 为什么需要它，以及它不是 `tome-store`
//!
//! `contracts/cli.md` 定义的流程是 `source import` **之后** `search <source-id>`
//! ——那要求导入结果被持久化。而 `tome-store`（SQLite 库）属后续切片，
//! 本阶段尚未实现。故这里给一个**文件式的最小注册表**，让 T028/T029 的
//! 命令行契约与 T026 的"重复 id 按更新处理"能真正成立：
//!
//! - 没有它，`search <id>` 无法实现（只能退化成 `--source-file`），
//!   而那是**契约之外的替代路径**，不是契约本身；
//! - 没有它，"同 id 更新"这条验收项无处检验（内存里的导入是一次性的，
//!   谈"更新已存在的条目"没有参照物）。
//!
//! ## 与 `tome-store` 的边界（避免重复造轮子）
//!
//! 本模块**不**做 `tome-store` 的职责：无 SQLite、无迁移器、无分组表、
//! 无并发 actor、无与书籍/进度/痕迹的关联。它只维护"id → 书源 JSON"这一张
//! 映射，供本阶段的书源载入使用。`tome-store` 落地后，本模块应改为它的
//! 一个薄适配（或直接删除，让 CLI 走 `BookRepository` 接缝）。
//!
//! 标 `[WIP]`：这是过渡形态，**不是**最终的存储设计。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::SourceError;
use crate::source::{parse_sources, LoadOutcome, ParsedSource};

/// 注册表在数据目录下的文件名。
pub const REGISTRY_FILE_NAME: &str = "sources.json";

/// 数据目录下的默认注册表路径。
///
/// ## 为什么默认是"可移植模式"（相对当前目录）
///
/// `contracts/cli.md` 的全局约定写着"数据目录：`--data-dir <path>` 覆盖
/// （默认可移植模式）"。可移植模式的含义是数据跟着程序走，故默认落在
/// 当前工作目录下的 `.tome/`。用户用 `--data-dir` 指向别处时才是"安装模式"。
pub fn default_registry_path() -> PathBuf {
    PathBuf::from(".tome").join(REGISTRY_FILE_NAME)
}

/// 一次注册表写入的结果（与 [`tome_core::seam::ImportReport`] 对应）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistryUpdate {
    /// 新增的条目数
    pub added: usize,
    /// 覆盖的条目数（同 id 已存在）
    pub updated: usize,
}

/// 书源注册表（内存表示 + 磁盘读写）。
#[derive(Debug, Clone, Default)]
pub struct SourceRegistry {
    /// id → 书源。用 `BTreeMap` 保证序列化顺序确定（输出可 diff）。
    entries: BTreeMap<String, ParsedSource>,
}

impl SourceRegistry {
    /// 从磁盘载入。文件不存在时返回**空注册表**而非错误——
    /// "还没导入过任何源"是正常初始状态，不是错误。
    pub fn load(path: &Path) -> Result<Self, SourceError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(SourceError::InvalidJson {
                    reason: format!("读取注册表 {} 失败：{error}", path.display()),
                });
            }
        };

        if text.trim().is_empty() {
            return Ok(Self::default());
        }

        // 注册表存的就是书源数组（与导入的输入格式同形）——
        // 复用同一套解析器，故格式只有一处定义。
        let outcome = parse_sources(&text)?;
        let mut entries = BTreeMap::new();
        for parsed in outcome.sources {
            entries.insert(parsed.source.id.clone(), parsed);
        }
        Ok(Self { entries })
    }

    /// 写入磁盘（父目录会自动创建）。
    pub fn save(&self, path: &Path) -> Result<(), SourceError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|error| SourceError::InvalidJson {
                    reason: format!("创建数据目录 {} 失败：{error}", parent.display()),
                })?;
            }
        }

        let sources: Vec<RegisteredSource> = self
            .entries
            .values()
            .map(RegisteredSource::from_parsed)
            .collect();
        // 尾随换行：让该文件在编辑器中正常显示、也让 `diff` 的末行有意义
        let text =
            serde_json::to_string_pretty(&sources).map_err(|error| SourceError::InvalidJson {
                reason: format!("序列化注册表失败：{error}"),
            })?;
        std::fs::write(path, format!("{text}\n")).map_err(|error| SourceError::InvalidJson {
            reason: format!("写入注册表 {} 失败：{error}", path.display()),
        })
    }

    /// 合并一批书源：**同 id 覆盖**（spec 边缘情况：「导入重复书源（同 id）
    /// 按"更新"处理」）。
    pub fn merge(&mut self, outcome: &LoadOutcome) -> RegistryUpdate {
        let mut update = RegistryUpdate::default();
        for parsed in &outcome.sources {
            let id = parsed.source.id.clone();
            if self.entries.insert(id, parsed.clone()).is_some() {
                update.updated += 1;
            } else {
                update.added += 1;
            }
        }
        update
    }

    /// 按 id 取书源。
    pub fn get(&self, id: &str) -> Option<&ParsedSource> {
        self.entries.get(id)
    }

    /// 全部书源（按 id 排序）。
    pub fn all(&self) -> Vec<&ParsedSource> {
        self.entries.values().collect()
    }

    /// 条目数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 注册表里的单条记录。
///
/// ## 为什么不用 `ParsedSource` 直接序列化
///
/// `ParsedSource` 是**领域类型**（core 的 `Source` 刻意不派生 serde），
/// 而本文件是**磁盘格式**——两者的演进节奏不同。分开让磁盘格式可以稳定
/// 而领域模型继续改（格式变更只需在这里加转换）。
///
/// 字段名用书源社区格式的**原始键名**（`bookSourceName` 等）：这样注册表
/// 文件可以直接拿去当书源文件用、也便于人工检视与手工编辑。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct RegisteredSource {
    #[serde(rename = "bookSourceName")]
    name: String,
    #[serde(rename = "bookSourceUrl")]
    url: String,
    #[serde(rename = "bookSourceType", skip_serializing_if = "Option::is_none")]
    kind: Option<i32>,
    #[serde(rename = "enabled")]
    enabled: bool,
    #[serde(rename = "searchUrl", skip_serializing_if = "Option::is_none")]
    search_url: Option<String>,
    #[serde(rename = "exploreUrl", skip_serializing_if = "Option::is_none")]
    explore_url: Option<String>,
    #[serde(rename = "bookUrlPattern", skip_serializing_if = "Option::is_none")]
    book_url_pattern: Option<String>,
    #[serde(rename = "ruleSearch", skip_serializing_if = "Option::is_none")]
    rule_search: Option<String>,
    #[serde(rename = "ruleExplore", skip_serializing_if = "Option::is_none")]
    rule_explore: Option<String>,
    #[serde(rename = "ruleBookInfo", skip_serializing_if = "Option::is_none")]
    rule_book_info: Option<String>,
    #[serde(rename = "ruleToc", skip_serializing_if = "Option::is_none")]
    rule_toc: Option<String>,
    #[serde(rename = "ruleContent", skip_serializing_if = "Option::is_none")]
    rule_content: Option<String>,
    #[serde(rename = "header", skip_serializing_if = "Option::is_none")]
    header: Option<String>,
    #[serde(rename = "loginUrl", skip_serializing_if = "Option::is_none")]
    login_url: Option<String>,
    #[serde(rename = "js", skip_serializing_if = "Option::is_none")]
    js: Option<String>,
    #[serde(rename = "charset", skip_serializing_if = "Option::is_none")]
    charset: Option<String>,
    #[serde(rename = "weight", skip_serializing_if = "Option::is_none")]
    weight: Option<i32>,
    #[serde(rename = "bookSourceComment", skip_serializing_if = "Option::is_none")]
    comment: Option<String>,
    /// 落库时间（诊断用）
    #[serde(rename = "lastUpdateTime", skip_serializing_if = "Option::is_none")]
    last_update_time: Option<i64>,
}

impl RegisteredSource {
    fn from_parsed(parsed: &ParsedSource) -> Self {
        let source = &parsed.source;
        Self {
            name: source.name.clone(),
            url: source.id.clone(),
            kind: Some(source.kind.as_storage_code()),
            enabled: source.enabled,
            search_url: parsed.search_url.clone(),
            explore_url: parsed.explore_url.clone(),
            book_url_pattern: source.book_url_pattern.clone(),
            rule_search: source.rule_search.clone(),
            rule_explore: source.rule_explore.clone(),
            rule_book_info: source.rule_book_info.clone(),
            rule_toc: source.rule_toc.clone(),
            rule_content: source.rule_content.clone(),
            header: source.header.clone(),
            login_url: source.login_url.clone(),
            js: source.js.clone(),
            charset: source.charset.clone(),
            weight: source.weight,
            comment: source.comment.clone(),
            last_update_time: source.last_update_time,
        }
    }
}

// `RegisteredSource` → 磁盘文本 → `parse_sources` 的往返：
// 落盘时用社区格式的键名，读回时走同一套解析器。故只需实现序列化方向，
// 反序列化由 `parse_sources` 承担（它认得这些键名）。

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str, name: &str) -> ParsedSource {
        let text = format!(
            r#"{{
              "bookSourceName": "{name}",
              "bookSourceUrl": "{id}",
              "searchUrl": "/s?q={{{{key}}}}",
              "ruleSearch": "{{\"bookList\":\"@css:li\",\"name\":\"@css:.n@text\"}}"
            }}"#
        );
        parse_sources(&text)
            .expect("样例应解析成功")
            .sources
            .into_iter()
            .next()
            .expect("应有一条源")
    }

    /// 建一个临时目录（不依赖 tempfile crate：本 crate 的 dev-deps 保持精简）。
    fn temp_dir(label: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("tome-registry-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("应能建临时目录");
        base
    }

    #[test]
    fn 不存在的注册表载入为空() {
        let path = temp_dir("missing").join("nope.json");
        let registry = SourceRegistry::load(&path).expect("应成功");
        assert!(registry.is_empty(), "文件不存在是正常初始状态");
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn 空文件载入为空() {
        let dir = temp_dir("empty");
        let path = dir.join("sources.json");
        std::fs::write(&path, "   \n").expect("应能写");
        let registry = SourceRegistry::load(&path).expect("应成功");
        assert!(registry.is_empty());
    }

    #[test]
    fn 往返保存与载入() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("sources.json");

        let mut registry = SourceRegistry::default();
        let outcome = LoadOutcome {
            sources: vec![
                sample("https://a.com", "源A"),
                sample("https://b.com", "源B"),
            ],
            report: Default::default(),
        };
        let update = registry.merge(&outcome);
        assert_eq!(update.added, 2);
        assert_eq!(update.updated, 0);
        registry.save(&path).expect("应能保存");

        let loaded = SourceRegistry::load(&path).expect("应能载入");
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded.get("https://a.com").expect("应有源A").source.name,
            "源A"
        );
        assert_eq!(
            loaded.get("https://b.com").expect("应有源B").source.name,
            "源B"
        );
        // 关键字段在往返后不丢
        assert_eq!(
            loaded
                .get("https://a.com")
                .expect("应有源A")
                .search_url
                .as_deref(),
            Some("/s?q={{key}}")
        );
    }

    #[test]
    fn 同_id_合并时按更新计() {
        // spec 边缘情况：「导入重复书源（同 id）按"更新"处理」
        let mut registry = SourceRegistry::default();
        let first = LoadOutcome {
            sources: vec![sample("https://a.com", "旧名")],
            report: Default::default(),
        };
        assert_eq!(registry.merge(&first).added, 1);

        let second = LoadOutcome {
            sources: vec![sample("https://a.com", "新名")],
            report: Default::default(),
        };
        let update = registry.merge(&second);
        assert_eq!(update.added, 0, "同 id 不该重复新增");
        assert_eq!(update.updated, 1, "应计为更新");
        assert_eq!(registry.len(), 1, "条目数不该增长");
        assert_eq!(
            registry.get("https://a.com").expect("应有源").source.name,
            "新名",
            "内容应被覆盖"
        );
    }

    #[test]
    fn 合并不同_id_各自新增() {
        let mut registry = SourceRegistry::default();
        let outcome = LoadOutcome {
            sources: vec![sample("https://a.com", "A"), sample("https://b.com", "B")],
            report: Default::default(),
        };
        registry.merge(&outcome);
        let update = registry.merge(&LoadOutcome {
            sources: vec![sample("https://c.com", "C")],
            report: Default::default(),
        });
        assert_eq!(update.added, 1);
        assert_eq!(update.updated, 0);
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn 按_id_排序输出() {
        let mut registry = SourceRegistry::default();
        let outcome = LoadOutcome {
            sources: vec![sample("https://z.com", "Z"), sample("https://a.com", "A")],
            report: Default::default(),
        };
        registry.merge(&outcome);
        let ids: Vec<&str> = registry
            .all()
            .iter()
            .map(|parsed| parsed.source.id.as_str())
            .collect();
        assert_eq!(ids, vec!["https://a.com", "https://z.com"], "顺序应确定");
    }

    #[test]
    fn 保存时会创建父目录() {
        let dir = temp_dir("mkdir");
        let path = dir.join("nested").join("deep").join("sources.json");
        let registry = SourceRegistry::default();
        registry.save(&path).expect("应能创建父目录并写入");
        assert!(path.exists());
    }

    #[test]
    fn 重复保存是幂等的() {
        // T029 的"输出逐字节一致"扩展到磁盘：同一份注册表保存两次应产出
        // 相同字节（否则重放时读到的语料会因落盘格式抖动而不同）。
        let dir = temp_dir("idempotent");
        let path = dir.join("sources.json");
        let mut registry = SourceRegistry::default();
        let outcome = LoadOutcome {
            sources: vec![sample("https://a.com", "A"), sample("https://b.com", "B")],
            report: Default::default(),
        };
        registry.merge(&outcome);

        registry.save(&path).expect("应能保存");
        let first = std::fs::read_to_string(&path).expect("应能读");
        registry.save(&path).expect("应能保存");
        let second = std::fs::read_to_string(&path).expect("应能读");
        assert_eq!(first, second, "同一份注册表的两次保存应逐字节相同");
    }

    #[test]
    fn 损坏的注册表报结构化错误() {
        let dir = temp_dir("corrupt");
        let path = dir.join("sources.json");
        std::fs::write(&path, "{ 这不是 JSON").expect("应能写");
        let error = SourceRegistry::load(&path).expect_err("应报错");
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Rule);
    }

    #[test]
    fn 注册表文件可被当书源文件使用() {
        // 落盘用社区格式的键名，故该文件本身就能被 `source import` 再读一次。
        // 这条性质让"手工编辑注册表"与"用注册表当导入源"都成立。
        let dir = temp_dir("reimport");
        let path = dir.join("sources.json");
        let mut registry = SourceRegistry::default();
        registry.merge(&LoadOutcome {
            sources: vec![sample("https://a.com", "源A")],
            report: Default::default(),
        });
        registry.save(&path).expect("应能保存");

        let text = std::fs::read_to_string(&path).expect("应能读");
        let reparsed = parse_sources(&text).expect("注册表应能被当书源文件解析");
        assert_eq!(reparsed.sources.len(), 1);
        assert_eq!(reparsed.sources[0].source.name, "源A");
        assert_eq!(
            reparsed.sources[0].source.kind,
            tome_core::SourceKind::Book,
            "书源类型应在往返后保持"
        );
    }
}
