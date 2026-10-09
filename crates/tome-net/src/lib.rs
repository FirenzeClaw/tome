//! # tome-net —— 网络层
//! 负责一切对外 HTTP 通信：请求执行、Cookie 管理、代理配置、按书源声明的限速、
//! 响应字符集探测与解码，并提供 `RequestSpec` 的**回放 adapter**（录制→回放，
//! 使规则引擎的测试无需真实联网）。
//!
//! ## 在本项目中的位置
//!
//! - 实现 `tome-core` 定义的 `NetHost` 接缝，供 `tome-js` 沙箱内的脚本调用宿主网络能力。
//! - `tome-source` 是唯一发起真实请求的管线；本 crate 提供能力，不决定"何时抓取"。
//!
//! ## 安全护栏
//!
//! - 外部通信一律 HTTPS；遵守书源声明的请求间隔，**不得绕过限速**（宪法原则 V）。
//! - Cookie 与认证头在日志中须脱敏。
//! - 除用户明确触发的抓取外不得产生任何网络外发（FR-041 / SC-012）。
//!
//! ## 关键入口
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`client`] | HTTP 客户端 + 阻塞外观（T013） |
//! | [`executor`] | `RequestSpec` 执行器，实现 `NetHost`（T014） |
//! | [`charset`] | 字符集嗅探/检测/容错解码（T015 / FR-009） |
//! | [`cookie`] | 按源隔离的 Cookie jar（T016 / FR-008） |
//! | [`proxy`] | 代理路线归约：系统代理 + 按源覆盖（T017 / FR-008） |
//! | [`ratelimit`] | 按 `rate_limit_key` 独立的限速调度（T018 / FR-008） |
//! | [`replay`] | 录制/离线回放，差分测试的确定性来源（T019） |
//! | [`error`] | 本 crate 的错误类型与 `ErrorKind` 映射 |
//!
//! 起步用 [`executor::RequestExecutor`]：它是组合起来的那一个入口。
//!
//! ## 已知缺口（已上报 PM 裁决，本 crate 不擅自改 tome-core）
//!
//! 1. `ErrorKind` 没有 Proxy 类目，故"代理故障"借 `Configuration` 承载
//!    （见 [`error::NetError::to_error_kind`] 的 `[WORKAROUND]` 注释）。
//! 2. `RequestSpec` 没有请求间隔字段（FR-008 要求书源可声明），
//!    故间隔由执行器侧注入（见 [`ratelimit`] 的模块文档）。

#![forbid(unsafe_code)]

pub mod charset;
pub mod client;
pub mod cookie;
pub mod error;
pub mod executor;
pub mod proxy;
pub mod ratelimit;
pub mod replay;

pub use charset::{decode_body, sniff_declared_charset, DecodedBody};
pub use client::{BlockingClient, HttpClient};
pub use cookie::{Cookie, CookieJar};
pub use error::NetError;
pub use executor::{ExecutionPolicy, RequestExecutor, RequestExecutorBuilder};
pub use proxy::{DefaultProxyPolicy, ProxyRoute};
pub use ratelimit::RateLimiter;
pub use replay::{ReplayMode, ReplayRecord, ReplayStore};
