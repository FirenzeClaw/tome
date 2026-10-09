//! # tome-rule —— 规则 DSL 引擎
//!
//! 书源规则的统一求值层，覆盖：规则分派（按前缀选择引擎）、组合符切分（`&&` / `||` / `%%`）、
//! CSS 选择器（含 Jsoup 专有伪类与索引切片）、JSONPath（含 shim）、自研 XPath 1.0 子集、
//! 双正则引擎路由（`regex` 与 `fancy-regex`）、以及替换链（`##` / `{{}}` 等）。
//!
//! ## 在本项目中的位置
//!
//! - 依赖 `tome-core`（`RequestSpec` 等共享值对象）与 `tome-js`（执行规则内嵌脚本片段）。
//! - **求值产出 `RequestSpec` 而不亲自发请求**——唯一发请求处是 `tome-source` 管线，
//!   这使规则引擎可离线重放、可用合成语料做差分测试（架构决策 A2）。
//!
//! ## 兼容性纪律（本项目最高优先级）
//!
//! - 对齐目标为 **Jsoup 1.2x 的 Selector 行为**，非标准伪类逐条实现（见 research.md R2）。
//! - 凡为兼容上游语义而写的非常规代码，**必须注明依据来源**（书源规则原文或公开文档），
//!   不得写"照抄某实现"（干净室纪律，宪法原则 II）。
//! - `[attr~=regex]` 在 CSS 标准中是空白分词、在 Jsoup 中被重载为正则——
//!   此差异必须特判并注释说明。
//!
//! [DONE] S1-1 阶段实现的模块：
//! - [`dom`] —— 自建 arena DOM（T020）
//! - [`regex`] —— 双正则引擎（T024）
//! - [`error`] —— 本 crate 错误类型与结构化「不支持能力」（FR-006）
//!
//! ## 关键入口
//!
//! - [`dom`] —— 自建 arena DOM（T020），承载节点类型细分（Text/Data/CData）
//! - [`css`] —— CSS 选择器引擎（T021），servo `selectors` + `cssparser` 跑在自建 DOM 上
//! - [`css_post`] —— CSS 后缀与索引切片（T022）
//! - [`dispatch`] —— 规则分派与组合符切分（T023）
//! - [`regex`] —— 双正则引擎 + 规范化层 + 替换串翻译层（T024）
//! - [`replace`] —— 替换链与 `{{}}` 插值占位（T025）
//!
//! [WIP] S1-1 阶段落地 CSS + 正则 + 替换链最小子集；JSONPath（S1-2）与 XPath（S1-3）
//! 尚未实现——但分派器已能识别它们并抛出**结构化「不支持能力」错误**（FR-006），
//! 供书源的 `try/catch` 降级逻辑判断。

#![forbid(unsafe_code)]

pub mod css;
pub mod css_post;
pub mod dispatch;
pub mod dom;
pub mod error;
pub mod regex;
pub mod replace;
