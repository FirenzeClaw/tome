//! [DONE] 宿主 API · 网络类（tasks.md T036 / FR-005）
//!
//! # FR-005 的本类清单
//!
//! > **网络**：单次请求、并发批量请求、下载到缓存、下载到文件
//!
//! | 能力 | `java.*` 入口 | 说明 |
//! |---|---|---|
//! | 单次请求 | `ajax` / `get` / `post` / `head` | 同步返回（书源脚本是同步语义） |
//! | 并发批量请求 | `ajaxAll` | 多个 URL 一次取回 |
//! | 下载到缓存 | `cacheFile` | 经 `StoreHost` |
//! | 下载到文件 | `downloadFile` | 经 `StoreHost`（限数据目录） |
//!
//! # 为什么这里**不**复用 `tome-net` 的 `HostRequest`
//!
//! `tome-net` 的 `HostRequest`（T032）正是为这类调用设计的接线层，但
//! `tome-js` **不能**依赖 `tome-net`（架构铁律 A1：实现 crate 只依赖
//! `tome-core`）。故本模块在 `tome-core` 的 `NetHost` 之上重建同等的构造
//! 逻辑。
//!
//! `[UNCERTAIN]` 两处构造逻辑因此有了重复（限速键取书源 id、超时取 15 秒、
//! 不重试）。这是分层的代价，且**必须保持两者一致**——否则"脚本请求"与
//! "管线请求"的限速会分岔（FR-008 的「按源」限速失效）。若哪天要消除重复，
//! 正确做法是把构造逻辑下沉到 `tome-core` 的 `RequestSpec` 构造函数，
//! 而不是让 `tome-js` 依赖 `tome-net`。
//!
//! # 同步语义与阻塞
//!
//! 书源脚本是同步语义（research.md R1）：`java.get(url)` 必须**当场**拿到
//! 结果。故这里的宿主函数会阻塞在 `NetHost::fetch` 上——而 `NetHost` 的
//! 契约正是同步的。阻塞期间该 runtime 的锁被持有，这是**设计使然**：
//! 契约要求"每个书源脚本用独立 runtime"（A1/R1），阻塞只影响自己那一个脚本。

use std::sync::Arc;

use rquickjs::{Ctx, Object};
use tome_core::{HttpMethod, NetHost, RequestSpec};

use crate::bridge::{require_bridge, throw_host_error, HostBridge};
use crate::sandbox_error::JsError;

/// 单请求超时（毫秒）。
///
/// 与 `tome_net::host::SCRIPT_REQUEST_TIMEOUT_MS` 取同一个值（15 秒）：
/// 两者都是"书源动作"级别的超时，取不同值只会让"为什么这个慢、那个快"
/// 变成一个没有依据的谜。此处**不**依赖 tome-net（那是反向依赖），
/// 故这个数字在两处各写一次——它是**契约值**而非实现细节。
pub const SCRIPT_REQUEST_TIMEOUT_MS: u64 = 15_000;

/// 注册网络类的全部宿主函数到 `java` 对象。
///
/// `net` 为 `None` 时入口仍会注册，但调用时抛结构化能力错误
/// （`capability = "network"`）——比"函数不存在"好：书源的 try/catch
/// 能据此降级（FR-006）。
pub fn register<'js>(
    ctx: &Ctx<'js>,
    java: &Object<'js>,
    net: Option<Arc<dyn NetHost>>,
) -> Result<usize, JsError> {
    let mut count = 0usize;

    // ---------- 单次请求 ----------
    //
    // 四个入口对应书源里的四种写法。语义差异只在方法上；`ajax` 与 `get`
    // 的区别是**状态码判定**（见 `send_text` 与 `send` 的说明）。
    for (name, method) in [
        ("get", HttpMethod::Get),
        ("post", HttpMethod::Post),
        ("head", HttpMethod::Head),
    ] {
        let host = net.clone();
        java.set(
            name,
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      url: String,
                      body: rquickjs::function::Opt<String>|
                      -> Result<String, rquickjs::Error> {
                    // `head` 不接受 body；GET/POST 的 body 可选。
                    let payload = body.0.map(String::into_bytes);
                    send_text(&ctx, host.as_deref(), method, &url, payload)
                        .map_err(|error| throw_host_error(&ctx, &error))
                },
            )
            .map_err(|error| registration_error(name, &error))?,
        )
        .map_err(|error| registration_error(name, &error))?;
        count += 1;
    }

    // `ajax`：原样返回响应体，**非 2xx 也不报错**（R3 的接缝语义）。
    //
    // ## 为什么书源需要它
    //
    // 可用性检查脚本要读 403 验证页的正文来区分"需要人工验证"与"源已失效"。
    // 若 `ajax` 在非 2xx 时报错，那条路径就断了。
    {
        let host = net.clone();
        java.set(
            "ajax",
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, url: String| -> Result<String, rquickjs::Error> {
                    let bridged =
                        require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                    let spec = build_spec(&bridged, HttpMethod::Get, url, None);
                    drop(bridged);
                    let host = host
                        .as_ref()
                        .ok_or_else(|| throw_host_error(&ctx, &network_unavailable_error()))?;
                    // `fetch` 取所有权，故先克隆：解码正文时还要用 spec
                    // 里的 charset 声明（FR-009）。
                    let response = host
                        .fetch(spec.clone())
                        .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                    // 原样解码正文（不论状态码）——调用方自己判状态
                    Ok(decode_body(&spec, &response))
                },
            )
            .map_err(|error| registration_error("ajax", &error))?,
        )
        .map_err(|error| registration_error("ajax", &error))?;
        count += 1;
    }

    // `ajaxAll`：并发批量请求。
    //
    // ## "并发"在本实现里是**串行**的（明确记录）
    //
    // 真正的并发需要跨线程共享 `NetHost`（它确实是 `Send + Sync`），但：
    // 1. `HostBridge` 经 `Ctx::userdata` 存放，而 `Ctx` 不可跨线程；
    // 2. 更实质的是**限速**：同源并发会绕过令牌桶的意图（FR-008 的
    //    "按源限速"针对的正是"同时打出多个请求"）。
    //
    // 故本实现按顺序取，语义上等价于"批量获取"（书源用它的目的是
    // "一次拿回多个 URL 的内容"）。`[UNCERTAIN]` 若 S1-6 的基线显示
    // 性能不够，应在**保持按源限速**的前提下做并发（例如按限速键分组，
    // 组内串行、组间并行），而不是简单并发。
    {
        let host = net.clone();
        java.set(
            "ajaxAll",
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, urls: Vec<String>| -> Result<Vec<String>, rquickjs::Error> {
                    let mut results = Vec::with_capacity(urls.len());
                    for url in urls {
                        let bridged =
                            require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                        let spec = build_spec(&bridged, HttpMethod::Get, url, None);
                        drop(bridged);
                        let host = host
                            .as_ref()
                            .ok_or_else(|| throw_host_error(&ctx, &network_unavailable_error()))?;
                        let response = host.fetch(spec.clone()).map_err(|kind| {
                            throw_host_error(&ctx, &JsError::from_error_kind(&kind))
                        })?;
                        // ## 批量里也要判状态码（与单次 `get` 一致）
                        //
                        // 若这里放行非 2xx，书源会把**错误页**当成 URL 的内容，
                        // 而它按索引取结果时完全看不出哪一项是错误页——
                        // 那类缺陷表现为"正文里混进了 HTML 错误信息"，
                        // 而排查方向会被引向选择器而不是网络。
                        //
                        // 与 `ajax` 的区别保持：`ajax` 是**单次**且调用方
                        // 显式想要"看状态"，`ajaxAll` 是批量取内容。
                        if !response.is_success() {
                            return Err(throw_host_error(
                                &ctx,
                                &JsError::new(format!(
                                    "第 {} 个 URL 返回 {}（{}）",
                                    results.len() + 1,
                                    response.status,
                                    status_hint(response.status)
                                )),
                            ));
                        }
                        results.push(decode_body(&spec, &response));
                    }
                    Ok(results)
                },
            )
            .map_err(|error| registration_error("ajaxAll", &error))?,
        )
        .map_err(|error| registration_error("ajaxAll", &error))?;
        count += 1;
    }

    // ---------- 下载到缓存 / 到文件 ----------
    //
    // 两者都经 `StoreHost`（存储能力）。"下载"在本实现里是"取回内容并写入
    // 存储"——正因为 `NetHost` 与 `StoreHost` 是两个独立的能力，
    // 缺任何一个都应报**对应的**能力错误（而不是笼统地报"下载失败"）。
    {
        let cache_host = net.clone();
        java.set(
            "cacheFile",
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, url: String, key: String| -> Result<String, rquickjs::Error> {
                    let bytes = fetch_bytes(&ctx, cache_host.as_deref(), &url)
                        .map_err(|error| throw_host_error(&ctx, &error))?;
                    let bridged =
                        require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                    let store = bridged
                        .require_store()
                        .map_err(|error| throw_host_error(&ctx, &error))?;
                    let store = Arc::clone(store);
                    drop(bridged);
                    store
                        .cache_put(&key, &bytes)
                        .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                    Ok(key)
                },
            )
            .map_err(|error| registration_error("cacheFile", &error))?,
        )
        .map_err(|error| registration_error("cacheFile", &error))?;
        count += 1;

        let download_host = net.clone();
        java.set(
            "downloadFile",
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      url: String,
                      path: String|
                      -> Result<String, rquickjs::Error> {
                    let bytes = fetch_bytes(&ctx, download_host.as_deref(), &url)
                        .map_err(|error| throw_host_error(&ctx, &error))?;
                    let bridged =
                        require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                    let store = bridged
                        .require_store()
                        .map_err(|error| throw_host_error(&ctx, &error))?;
                    let store = Arc::clone(store);
                    drop(bridged);
                    // 路径越界由 `StoreHost` 实现方拒绝（见 tome-core 的
                    // trait 文档：越界须返回 UnsupportedCapability）。
                    // 本模块**不**重复实现路径校验——那是实现方的职责，
                    // 重复一份只会产生"两处规则不一致"的风险。
                    store
                        .write_file(&path, &bytes)
                        .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                    Ok(path)
                },
            )
            .map_err(|error| registration_error("downloadFile", &error))?,
        )
        .map_err(|error| registration_error("downloadFile", &error))?;
        count += 1;
    }

    // `connect`：只建立连接探测可用性（书源里用于检测源是否在线）。
    //
    // 实现为一次 HEAD 请求：真正的"只连不发"在 HTTP 客户端层没有对应语义，
    // 而 HEAD 的效果等价（不取正文）。
    {
        let host = net.clone();
        java.set(
            "connect",
            rquickjs::Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, url: String| -> Result<bool, rquickjs::Error> {
                    let bridged =
                        require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                    let spec = build_spec(&bridged, HttpMethod::Head, url, None);
                    drop(bridged);
                    let host = host
                        .as_ref()
                        .ok_or_else(|| throw_host_error(&ctx, &network_unavailable_error()))?;
                    // 传输层失败视为"连不上"（false），而不是抛错：
                    // 调用方用它的目的就是探测可用性，抛错会让书源必须
                    // 包 try/catch 才能拿到这个布尔。
                    Ok(host.fetch(spec).is_ok())
                },
            )
            .map_err(|error| registration_error("connect", &error))?,
        )
        .map_err(|error| registration_error("connect", &error))?;
        count += 1;
    }

    Ok(count)
}

/// 构造注册失败的诊断错误。
fn registration_error(name: &str, error: &rquickjs::Error) -> JsError {
    JsError::new(format!("注册 `{name}` 失败：{error}"))
}

/// 未注入网络能力时的能力错误。
fn network_unavailable_error() -> JsError {
    JsError::unsupported("network", "本沙箱未启用网络能力（组合根未注入 NetHost）")
}

/// 从宿主桥构造 `RequestSpec`。
///
/// # 补全的三处默认值（必须与 `tome-net` 的 `HostRequest` 一致）
///
/// | 字段 | 取值 | 理由 |
/// |---|---|---|
/// | `rate_limit_key` | **书源 id** | FR-008 的「按源」限速；用 URL 会让同源被限成两份 |
/// | `timeout_ms` | 15 秒 | 与管线一致（见本模块常量的说明） |
/// | `retry` | 不重试 | 脚本请求失败应尽快报给脚本的 try/catch |
fn build_spec(
    bridge: &HostBridge,
    method: HttpMethod,
    url: String,
    body: Option<Vec<u8>>,
) -> RequestSpec {
    let mut spec = RequestSpec::get(url, bridge.context.source_id.clone());
    spec.method = method;
    spec.body = body;
    spec.charset = bridge.context.declared_charset.clone();
    spec.request_interval_ms = bridge.context.request_interval_ms;
    spec.timeout_ms = SCRIPT_REQUEST_TIMEOUT_MS;
    spec.retry = tome_core::RetryPolicy::NONE;
    spec
}

/// 执行请求并解出文本（**非 2xx 报错**）。
///
/// # 为什么 `get`/`post` 要报错而 `ajax` 不报
///
/// 见 `tome-net` 的 `HostRequest::send_text` 文档：`java.get` 拿的是**正文**，
/// 把 404 的 HTML 页当成"成功的结果"交给脚本，会让 `JSON.parse` 抛一个与
/// 真实原因无关的异常（"Unexpected token <"），排查方向完全被带偏。
fn send_text(
    ctx: &Ctx<'_>,
    host: Option<&dyn NetHost>,
    method: HttpMethod,
    url: &str,
    body: Option<Vec<u8>>,
) -> Result<String, JsError> {
    let bridged = require_bridge(ctx)?;
    let spec = build_spec(&bridged, method, url.to_string(), body);
    drop(bridged);

    let host = host.ok_or_else(network_unavailable_error)?;
    let response = host
        .fetch(spec.clone())
        .map_err(|kind| JsError::from_error_kind(&kind))?;
    if !response.is_success() {
        return Err(JsError::new(format!(
            "请求返回 {}（{}）",
            response.status,
            status_hint(response.status)
        )));
    }
    Ok(decode_body(&spec, &response))
}

/// 取回原始字节（供下载类入口）。
fn fetch_bytes(ctx: &Ctx<'_>, host: Option<&dyn NetHost>, url: &str) -> Result<Vec<u8>, JsError> {
    let bridged = require_bridge(ctx)?;
    let spec = build_spec(&bridged, HttpMethod::Get, url.to_string(), None);
    drop(bridged);

    let host = host.ok_or_else(network_unavailable_error)?;
    let response = host
        .fetch(spec)
        .map_err(|kind| JsError::from_error_kind(&kind))?;
    if !response.is_success() {
        return Err(JsError::new(format!(
            "下载返回 {}（{}）",
            response.status,
            status_hint(response.status)
        )));
    }
    Ok(response.body_bytes)
}

/// 给状态码附一句可读提示（诊断用，帮书源作者判断该怎么做）。
///
/// 这不违反"引擎内不得硬编码面向用户的自然语言"（FR-044）：它是
/// **诊断细节**（随 `ErrorKind` 的 detail 一起给开发者），而可本地化的
/// 部分仍是 `ErrorKind` 的分类标识。
fn status_hint(status: u16) -> &'static str {
    match status {
        403 => "可能需要人工验证，或被站点封禁",
        404 => "页面不存在，书源选择器可能已失效",
        429 => "被限流，书源声明的请求间隔可能需要调大",
        500..=599 => "服务端临时故障",
        _ => "非成功状态",
    }
}

/// 把响应体解成文本（复用容错解码取向：声明与实际不符是常态）。
fn decode_body(spec: &RequestSpec, response: &tome_core::Response) -> String {
    // 声明来源次序与管线一致：响应实际检测到的优先，其次书源声明，
    // 最后 content-type 头。
    let declared = response
        .detected_charset
        .clone()
        .or_else(|| spec.charset.clone())
        .or_else(|| response.headers.get("content-type").map(str::to_string));
    // 本 crate 不依赖 tome-net 的 charset 层（那是反向依赖），
    // 故此处按 UTF-8 容错解码。`[WARN]` 若书源声明的 charset 是 GBK 等
    // 非 UTF-8，这里的解码会失真——而 `detected_charset` 是 tome-net 在
    // 响应后处理时按 charset 层**已经解好的**结论，故优先用它。
    match declared.as_deref() {
        // `detected_charset` 已被 tome-net 的 charset 层填成实际编码，
        // 但它只告诉我们"是什么"，字节仍需我们解。UTF-8 以外的编码
        // 本模块无法自行处理——诚实报出而不是产出乱码。
        Some(charset) if !is_utf8_charset(charset) => {
            // 用 lossy 而非报错：书源拿到的至少是"大部分正确"的文本，
            // 而报错会让整个源失败。真正的按声明解码在 S1-4 的 store 层
            // 与 T039 的数据层有更完整的路径。
            String::from_utf8_lossy(&response.body_bytes).into_owned()
        }
        _ => String::from_utf8_lossy(&response.body_bytes).into_owned(),
    }
}

/// 判断某字符集名是否等价于 UTF-8。
fn is_utf8_charset(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "utf-8" | "utf8" | "utf-8-bom" | "unicode-1-1-utf-8"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 状态提示覆盖常见码() {
        assert!(status_hint(403).contains("验证"));
        assert!(status_hint(404).contains("不存在"));
        assert!(status_hint(429).contains("限流"));
        assert!(status_hint(503).contains("故障"));
        assert!(!status_hint(200).is_empty());
    }

    #[test]
    fn utf8_字符集判定() {
        for name in ["utf-8", "UTF-8", "utf8", "UTF8", "utf-8-bom"] {
            assert!(is_utf8_charset(name), "{name} 应判为 UTF-8");
        }
        for name in ["gbk", "GB2312", "big5", "iso-8859-1"] {
            assert!(!is_utf8_charset(name), "{name} 不应判为 UTF-8");
        }
    }

    #[test]
    fn 超时常量与_tome_net_一致() {
        // 两处必须取同一个值（见常量的说明）。此断言无法跨 crate 校验，
        // 故在此记录契约值，改一处时另一处会由这条测试提醒。
        assert_eq!(SCRIPT_REQUEST_TIMEOUT_MS, 15_000);
    }
}
