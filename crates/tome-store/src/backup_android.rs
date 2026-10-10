//! [DONE] Android 备份的**书源**导入（tasks.md T088 / FR-034）
//!
//! # FR-034 与它在验收里的落点
//!
//! > 系统 MUST 支持从既有 Android 版本应用产生的备份中导入**书源**
//!
//! spec 的验收场景 3：*"Given 一份来自既有 Android 版本的备份文件中的书源，
//! When 用户导入，Then 书源被正确识别并可用"*。注意本任务的范围**是书源**，
//! 不是"导入整份备份"——整份备份的导入导出是 T087（本程序自己的格式）。
//! 本模块解决的是"用户手里有个旧 App 的备份包，只想把里面的源拿过来"。
//!
//! # 备份包的形态（依据公开信息）
//!
//! 公开说明与社区资料描述的一致形态是：**一个 zip，内含一批 JSON 文件**，
//! 按数据类型分文件（书源、书架、分组、替换规则、目录规则…）。参阅
//! 《【开源阅读】阅读备份 恢复 换机全过程说明》对"备份文件长啥样"的说明
//! （<https://www.bilibili.com/opus/1019022252732055552>）与服务器版部署
//! 文档列出的还原清单。
//!
//! ## 干净室说明
//!
//! 本模块的格式认识来自**公开文档与社区对其产物的描述**，未读取上游
//! `.kt`/`.java` 源码（宪法原则 II）。故实现上采取**宽容策略**：
//!
//! - 不假设 zip 里书源文件叫什么确切名字（按**内容**判定，见下）
//! - 不假设顶层是数组还是对象
//! - 不认识的文件一律跳过，不影响书源的提取
//!
//! 宽容是有意的：我们只能从公开描述知道"大概有哪些文件"，而实际用户
//! 手里的备份来自多个版本、多种来源。**按内容识别**比按文件名识别更能
//! 适应这种多样性——而"书源 JSON 长什么样"是公开且稳定的（`bookSourceUrl`
//! 等键名是书源格式的一部分）。
//!
//! # 按内容而非按文件名识别
//!
//! 判据是"这个 JSON 里的对象带没带书源的特征键"。选 `bookSourceUrl`
//! 作主判据，因为它是书源的**唯一标识**（`tome-core` 的 `Source::id` 也
//! 用它），任何合法书源必然有它。这比"文件名含 bookSource"稳健：
//! 后者遇到重命名、大小写差异、或用户把多个源的 JSON 合并成一个文件时就失效。

use std::io::{Cursor, Read};

use crate::error::StoreError;

/// 书源的特征键。
///
/// 取 `bookSourceUrl` 而非"文件名"作为判据——见模块文档。缺它的对象
/// 不可能是可用的书源（id 是主键，没有它落不了库）。
const SOURCE_MARKER_KEY: &str = "bookSourceUrl";

/// 从 Android 备份里提取书源的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidImport {
    /// 提取到的书源 JSON 文本（数组形态，可直接交给 `parse_sources`）
    pub sources_json: String,
    /// 识别出的书源条数
    pub source_count: usize,
    /// 扫描过的 zip 条目名（诊断用：告诉用户包里到底有什么）
    pub scanned_entries: Vec<String>,
    /// 被跳过的条目（非 JSON / 不含书源 / 读不了），附原因
    pub skipped: Vec<(String, String)>,
}

impl AndroidImport {
    /// 是否提取到书源。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.source_count == 0
    }
}

/// 从备份 zip 字节里提取书源（FR-034）。
///
/// # 为什么收字节而不是路径
///
/// 备份可能来自"用户从别处拷来的文件"或"下载流"——调用方拿到什么就传什么。
/// 收字节让本函数不关心文件系统，也让测试用内存构造样本（见本文件的测试）。
pub fn extract_sources(zip_bytes: &[u8]) -> Result<AndroidImport, StoreError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip_bytes)).map_err(|error| StoreError::Corrupt {
            subject: "android-backup".to_string(),
            reason: format!("不是合法的 zip 包：{error}"),
        })?;

    let mut result = AndroidImport {
        sources_json: String::new(),
        source_count: 0,
        scanned_entries: Vec::new(),
        skipped: Vec::new(),
    };
    let mut collected: Vec<serde_json::Value> = Vec::new();

    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(error) => {
                result
                    .skipped
                    .push((format!("#{index}"), format!("读取条目失败：{error}")));
                continue;
            }
        };

        let name = entry.name().to_string();
        // 目录条目本身不含数据，但记进 scanned 让用户知道包里有什么
        if entry.is_dir() {
            result.scanned_entries.push(name);
            continue;
        }

        // 只处理 `.json`：备份包里还有字体/图片等资源，解析它们纯属浪费。
        // 但**不因扩展名不匹配就断定非书源**——有些导出会把 JSON 存成 .txt
        // 或没有扩展名。故对非 .json 也读一小段看首字符是不是 `[`/`{`。
        let looks_like_json = name.to_ascii_lowercase().ends_with(".json");
        let mut bytes = Vec::new();
        if let Err(error) = entry.read_to_end(&mut bytes) {
            result
                .skipped
                .push((name.clone(), format!("读取失败：{error}")));
            continue;
        }
        result.scanned_entries.push(name.clone());

        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                result
                    .skipped
                    .push((name.clone(), "不是 UTF-8 文本".to_string()));
                continue;
            }
        };

        if !looks_like_json {
            let head = text.trim_start().chars().next();
            if !matches!(head, Some('[') | Some('{')) {
                result
                    .skipped
                    .push((name.clone(), "不是 JSON 文本".to_string()));
                continue;
            }
        }

        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                result
                    .skipped
                    .push((name.clone(), format!("JSON 解析失败：{error}")));
                continue;
            }
        };

        let found = collect_sources(value);
        if found.is_empty() {
            result.skipped.push((
                name.clone(),
                "不含书源（无 bookSourceUrl 特征键）".to_string(),
            ));
        }
        collected.extend(found);
    }

    result.source_count = collected.len();
    if !collected.is_empty() {
        // 输出为数组文本：与 `source import` 的既有输入格式一致，
        // 故备份里的源与手工准备的源走**同一条**导入路径（不多一条分支）
        result.sources_json =
            serde_json::to_string(&collected).map_err(|error| StoreError::Corrupt {
                subject: "android-backup".to_string(),
                reason: format!("书源序列化失败：{error}"),
            })?;
    }

    Ok(result)
}

/// 从一个 JSON 值里收集书源对象。
///
/// # 接受三种形态（公开说明里都出现过）
///
/// | 形态 | 例子 |
/// |---|---|
/// | 顶层数组 | `[{...}, {...}]` —— 最常见的导出形态 |
/// | 单对象 | `{...}` —— 只导出一个源时 |
/// | 带数组字段的对象 | `{"bookSources": [...]}` —— 某些打包形态 |
///
/// # 为什么不递归深搜
///
/// 递归会把"书源对象里的某个字段恰好也叫 bookSourceUrl"这类嵌套误认成
/// 顶层书源。限定在一层数组/一层已知字段，够覆盖公开描述过的形态，
/// 且不会把结构里的东西捞出来。
fn collect_sources(value: serde_json::Value) -> Vec<serde_json::Value> {
    match value {
        serde_json::Value::Array(items) => items.into_iter().filter(is_source_object).collect(),
        // 单对象：本身是书源就直接收
        serde_json::Value::Object(_) => {
            if is_source_object(&value) {
                vec![value]
            } else {
                // 带数组字段的容器：找一层里"元素是书源的数组"的字段
                let mut found = Vec::new();
                if let serde_json::Value::Object(map) = &value {
                    for (_, field) in map {
                        if let serde_json::Value::Array(items) = field {
                            for item in items {
                                if is_source_object(item) {
                                    found.push(item.clone());
                                }
                            }
                        }
                    }
                }
                found
            }
        }
        _ => Vec::new(),
    }
}

/// 判断一个 JSON 值是不是书源对象。
///
/// 判据只有一个但足够：**含 `bookSourceUrl` 且它是字符串**。
/// 只要类型对（字符串），不必非空——「有个空 URL 的源」是数据问题，
/// 该由 `parse_sources` 给出具体的失败原因（"缺 bookSourceUrl"会误导用户），
/// 而不是在这里静默丢弃。
#[must_use]
fn is_source_object(value: &serde_json::Value) -> bool {
    value
        .get(SOURCE_MARKER_KEY)
        .is_some_and(serde_json::Value::is_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 造一个备份 zip（内存）。
    fn make_zip(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in entries {
                writer.start_file(*name, options).expect("应能建条目");
                writer.write_all(content.as_bytes()).expect("应能写条目");
            }
            writer.finish().expect("应能收尾");
        }
        buffer
    }

    const ONE_SOURCE: &str =
        r#"[{"bookSourceName":"源甲","bookSourceUrl":"https://a.example.com","enabled":true}]"#;

    #[test]
    fn 从典型备份里提取书源() {
        let zip = make_zip(&[
            ("bookSource.json", ONE_SOURCE),
            ("book.json", r#"[{"name":"某书","author":"某人"}]"#),
            ("replaceRule.json", r#"[{"name":"规则","pattern":"x"}]"#),
            ("config.xml", "<config/>"),
        ]);
        let result = extract_sources(&zip).expect("应能提取");
        assert_eq!(result.source_count, 1, "应认出一个书源");
        assert!(result.sources_json.contains("https://a.example.com"));

        // 非书源的 JSON 应被明确跳过并说明原因（而不是静默）
        let skipped_names: Vec<&str> = result.skipped.iter().map(|(n, _)| n.as_str()).collect();
        assert!(skipped_names.contains(&"book.json"), "书架 JSON 应被跳过");
        assert!(skipped_names.contains(&"config.xml"), "非 JSON 应被跳过");
        // 扫描记录要含全部条目名，便于用户核对"包里有什么"
        assert!(result
            .scanned_entries
            .iter()
            .any(|n| n == "bookSource.json"));
    }

    #[test]
    fn 多文件里的书源被合并() {
        let zip = make_zip(&[
            (
                "bookSource.json",
                r#"[{"bookSourceName":"甲","bookSourceUrl":"https://a.example.com"}]"#,
            ),
            (
                "bookSource2.json",
                r#"[{"bookSourceName":"乙","bookSourceUrl":"https://b.example.com"}]"#,
            ),
        ]);
        let result = extract_sources(&zip).expect("应能提取");
        assert_eq!(result.source_count, 2, "两个文件里的源都应收到");
    }

    #[test]
    fn 单对象形态也被接受() {
        let zip = make_zip(&[(
            "one.json",
            r#"{"bookSourceName":"独源","bookSourceUrl":"https://only.example.com"}"#,
        )]);
        let result = extract_sources(&zip).expect("应能提取");
        assert_eq!(result.source_count, 1, "只导出一个源时是单对象");
    }

    #[test]
    fn 带数组字段的容器形态也被接受() {
        let zip = make_zip(&[(
            "wrapper.json",
            r#"{"bookSources":[{"bookSourceName":"包内源","bookSourceUrl":"https://tucked.example.com"}]}"#,
        )]);
        let result = extract_sources(&zip).expect("应能提取");
        assert_eq!(result.source_count, 1);
        assert!(result.sources_json.contains("tucked.example.com"));
    }

    #[test]
    fn 混杂的数组只取书源对象() {
        let zip = make_zip(&[(
            "mixed.json",
            r#"[{"bookSourceName":"源","bookSourceUrl":"https://x.example.com"},{"name":"不是源"},{"foo":1}]"#,
        )]);
        let result = extract_sources(&zip).expect("应能提取");
        assert_eq!(result.source_count, 1, "只应取带 bookSourceUrl 的那个");
    }

    #[test]
    fn 按内容识别而非按文件名() {
        // 文件名完全不相干，但内容是书源 —— 应被认出
        let zip = make_zip(&[("data_2026.txt", ONE_SOURCE)]);
        let result = extract_sources(&zip).expect("应能提取");
        assert_eq!(result.source_count, 1, "应按内容识别：文件名变了不影响");
    }

    #[test]
    fn 不含书源的包给出零结果而非报错() {
        // "用户拿错了文件"是常见情形，报错不如给一个空结果 +
        // 完整的 scanned/skipped 让上层告诉用户"包里没有书源"
        let zip = make_zip(&[("readme.txt", "这个包里没有书源")]);
        let result = extract_sources(&zip).expect("应能处理");
        assert!(result.is_empty());
        assert_eq!(result.source_count, 0);
        assert!(!result.scanned_entries.is_empty(), "应报告扫过什么");
    }

    #[test]
    fn 非_zip_报错而非静默空() {
        let error = extract_sources(b"this is not a zip at all").expect_err("非 zip 应报错");
        let message = format!("{error}");
        assert!(message.contains("zip"), "错误应说明不是 zip：{message}");
    }

    #[test]
    fn 损坏的_json_被跳过而不影响其他条目() {
        let zip = make_zip(&[
            ("broken.json", "{ 这不是合法 JSON"),
            ("good.json", ONE_SOURCE),
        ]);
        let result = extract_sources(&zip).expect("应能提取");
        assert_eq!(result.source_count, 1, "坏的条目不该拖累好的");
        assert!(
            result.skipped.iter().any(|(n, _)| n == "broken.json"),
            "坏条目应被记录"
        );
    }

    #[test]
    fn 空_url_的源被保留交由上层报错() {
        // 空 bookSourceUrl 仍是"有该键的字符串"，故被认出。
        // 具体为什么不合法由 parse_sources 说明——那里能给出
        // "缺 bookSourceUrl" 之外的准确原因
        let zip = make_zip(&[(
            "s.json",
            r#"[{"bookSourceName":"坏源","bookSourceUrl":""}]"#,
        )]);
        let result = extract_sources(&zip).expect("应能提取");
        assert_eq!(result.source_count, 1, "空 URL 不该在这里被静默丢弃");
    }

    // ## 关于"提取结果能否被书源解析器吃掉"
    //
    // 那条端到端断言放在 `tome-source` 的测试里（`android_backup_roundtrip.rs`）
    // ——依赖方向是 `tome-source → tome-store`，本 crate 不能反向依赖它。
    // 这里只测"提取出的是一段合法 JSON 且含预期的源"，格式可被解析由
    // 那边的测试保证。

    #[test]
    fn 提取结果是合法_json() {
        let zip = make_zip(&[("bookSource.json", ONE_SOURCE)]);
        let result = extract_sources(&zip).expect("应能提取");
        let parsed: serde_json::Value =
            serde_json::from_str(&result.sources_json).expect("应输出合法 JSON");
        let items = parsed.as_array().expect("应是数组形态");
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0]["bookSourceUrl"].as_str(),
            Some("https://a.example.com")
        );
    }

    #[test]
    fn 目录条目被记录不再报错() {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            writer
                .add_directory("assets/", options)
                .expect("应能建目录条目");
            writer
                .start_file("bookSource.json", options)
                .expect("应能建文件");
            writer.write_all(ONE_SOURCE.as_bytes()).expect("应能写");
            writer.finish().expect("应能收尾");
        }
        let result = extract_sources(&buffer).expect("应能提取");
        assert_eq!(result.source_count, 1);
        assert!(
            result.scanned_entries.iter().any(|n| n.contains("assets")),
            "目录条目也应出现在扫描记录里"
        );
    }
}
