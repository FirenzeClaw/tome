//! reqwest 一次性 HTTP 验证（T006 验证项）
//!
//! **本模块只能由用户显式触发**（`--http` 子命令）。理由：FR-041/SC-012 要求
//! 「除用户明确触发的书源抓取外，MUST NOT 产生任何网络外发」。spike 若在
//! 启动路径上自动发请求，它自己就违反了这条约束——而这条约束正是本次要验证的。
//! 故这里是显式入口，不是隐式副作用。
//!
//! 为什么选 `example.com`：IANA 保留的示例域，无追踪、无 Cookie、响应体极小，
//! 用它做连通性验证不会给任何第三方留下访问痕迹。

/// 发起一次 GET，返回人类可读的摘要。
pub fn fetch_once(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        // 显式超时：spike 卡住会让人误判为"技术栈不可行"，而实际只是网络慢。
        .timeout(std::time::Duration::from_secs(SPIKE_HTTP_TIMEOUT_SECONDS))
        // 只用 rustls，不用 native-tls：避免三平台各自依赖系统 TLS 栈
        // （Windows schannel / macOS Security.framework / Linux OpenSSL），
        // 与"单一二进制、无系统运行时依赖"的体积纪律一致。
        .build()
        .map_err(|error| format!("构建 HTTP 客户端失败：{error}"))?;

    let response = client
        .get(url)
        .send()
        .map_err(|error| format!("请求 {url} 失败：{error}"))?;

    let status = response.status();
    let body_bytes = response
        .bytes()
        .map_err(|error| format!("读取 {url} 响应体失败：{error}"))?;

    if !status.is_success() {
        return Err(format!("请求 {url} 返回非成功状态：{status}"));
    }

    // 只回摘要，不回内容：spike 不需要正文，打印完整 HTML 只会淹没日志。
    Ok(format!(
        "GET {url} → {status}，{} 字节",
        body_bytes.len()
    ))
}

/// spike 的 HTTP 超时：30 秒。
///
/// 取值理由：example.com 正常在 1 秒内返回；30 秒足够覆盖慢速网络的握手+传输，
/// 又不至于让"网络不通"被误判成"spike 卡死"。
const SPIKE_HTTP_TIMEOUT_SECONDS: u64 = 30;
