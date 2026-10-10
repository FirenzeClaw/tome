//! \[DONE] Cookie jar：按源隔离、可持久化序列化（tasks.md T016 / FR-008）
//!
//! FR-008 把 Cookie 列为书源可声明的请求定制项；spec 的修订项进一步要求
//! **按源持久化落盘，重启后保留登录态**（FR-026 修订）。本模块实现两件事：
//! 一是"哪些 cookie 该发给这个请求"，二是"怎么把它们存下来再读回来"。
//!
//! # 为什么自己实现而不用 `cookie` crate
//!
//! tasks.md 与 research.md 未点名 `cookie`/`cookie_store` 依赖，本项目对
//! 未点名的第三方依赖持保守态度（体积纪律 + 供应链面）。RFC 6265 里本层需要的
//! 匹配规则（域匹配、路径匹配、Secure、过期）体量很小，自实现是可控的。
//! **依据仅为 RFC 6265 公开规范**，不参考任何第三方实现。
//!
//! # 按源隔离的键为什么是 `rate_limit_key`
//!
//! \[UNCERTAIN] 理想情况下 Cookie 应按 **origin**（scheme + host + port）隔离，
//! 因为 RFC 6265 的域匹配本身就是围绕 host 定义的。但本层拿到的只有
//! `RequestSpec`，而它**只有 `rate_limit_key` 这一个每源稳定的键**
//! （`RequestSpec` 没有 book source id 字段）。
//! 故隔离键取 `rate_limit_key`，含义是"一个书源一个 jar"：
//!
//! - 满足"按源隔离"的字面要求与安全意图（A 源的登录态不会漏给 B 源）。
//! - 比 origin 更严：同一源访问两个域时也会**在源内**按域再匹配
//!   （[`CookieJar::header_for`] 仍做完整 RFC 6265 域/路径匹配），
//!   所以严格性只体现在"不同源不共享"，不会错发。
//! - 代价：同一站点被两个书源使用时不共享登录态，用户需分别登录。
//!   这是**安全侧**的取舍（宁可多登录一次，不可串站）。
//!
//! # 序列化格式
//!
//! 自定文本格式，每行一条 cookie，字段以 `\t` 分隔——理由：
//!
//! - **确定性**：行序由 key 与域名排序固定，同一状态两次序列化逐字节相同，
//!   可 diff、可作为测试断言（否则持久化文件会随 HashMap 迭代顺序抖动）。
//! - **可读**：出问题时能直接打开看，不需要专用工具。
//! - **无歧义**：`\t` 不可能出现在 cookie 名/值/域/路径里
//!   （RFC 6265 的 `token`/`cookie-octet` 语法都排除控制字符），故不需要转义。
//!
//! 格式（`#` 开头为注释行）：
//!
//! ```text
//! # tome cookie jar v1
//! <key>\t<domain>\t<path>\t<secure>\t<host_only>\t<expires_unix|-1>\t<name>\t<value>
//! ```

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::error::NetError;

/// `Expires` 缺失时的时间戳标记。
const NO_EXPIRY: i64 = -1;

/// 序列化格式的版本标记（首行注释）。
///
/// 带版本号是为了将来改格式时能明确拒绝旧文件（而不是猜着解析出错误结果）。
const FORMAT_HEADER: &str = "# tome cookie jar v1";

/// 把时间点截断到整秒。
///
/// 持久化格式以 Unix 秒为单位（cookie 的过期时间本来就只需秒级精度）。
/// 若内存里保留亚秒部分，往返一次就会不相等——那会让"往返后逐字段相同"
/// 这条断言随机失败，也会让同一份状态的两次序列化产生不同结果。
fn truncate_to_second(time: SystemTime) -> Option<SystemTime> {
    let seconds = unix_from_system_time(time)?;
    system_time_from_unix(seconds)
}

/// 时间戳到 `SystemTime` 的换算基准。
///
/// 用 `checked_add` 而非直接加减：Unix 时间戳是**外部输入**
/// （持久化文件可能被手改坏），溢出必须走错误路径而不是 panic。
fn system_time_from_unix(seconds: i64) -> Option<SystemTime> {
    if seconds < 0 {
        return None;
    }
    UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))
}

/// `SystemTime` 到 Unix 秒。早于纪元的时间点返回 `None`（视为已过期）。
fn unix_from_system_time(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}

/// 一条 cookie。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    /// 名字
    pub name: String,
    /// 值
    pub value: String,
    /// 作用域域（RFC 6265 的 domain，已去前导点）
    pub domain: String,
    /// 作用路径
    pub path: String,
    /// 是否仅经安全信道发送
    pub secure: bool,
    /// 是否 HttpOnly（对脚本不可见）。
    ///
    /// 本层保留并持久化它，是因为**沙箱内的脚本**可能与网络层共享 jar
    /// （JS 的 cookie 宿主函数）；HttpOnly 正是"脚本不得读取"的标记，
    /// 丢掉它等于把这道保护抹掉。
    pub http_only: bool,
    /// 宿主标记：为真表示域必须精确相等（`Domain` 属性缺失时的行为）
    pub host_only: bool,
    /// 过期时刻；`None` 表示会话 cookie（随进程结束失效）
    pub expires_at: Option<SystemTime>,
}

impl Cookie {
    /// 在给定时刻是否已过期。
    fn is_expired_at(&self, now: SystemTime) -> bool {
        match self.expires_at {
            Some(expiry) => expiry <= now,
            None => false,
        }
    }

    /// 同一条 cookie 的身份（域 + 路径 + 名，RFC 6265 §5.3 的 cookie 唯一性判据）。
    ///
    /// 不含值：值会被同名的后续 `Set-Cookie` 覆盖，而覆盖必须落到同一条上。
    fn identity(&self) -> (String, String, String) {
        (self.domain.clone(), self.path.clone(), self.name.clone())
    }
}

/// 按源隔离的 cookie 容器。
///
/// # 为什么方法取 `&self` 而不是 `&mut self`
///
/// 执行器把它放在 `Arc<CookieJar>` 里跨线程共享（`NetHost: Send + Sync`），
/// 因此所有方法只能是 `&self`。内部用 [`Mutex`] 提供可变性。
///
/// 锁的粒度是**整个容器**：cookie 操作是纯内存的小表操作（几条到几十条），
/// 而调用它前后的网络 I/O 才是耗时大头。为它做细粒度分片属于优化错了地方。
///
/// # 关于死锁
///
/// 所有公共方法都在**进入时取锁、返回前放锁**，不在持锁期间调用任何其他
/// 公共方法，故不存在可重入取锁导致的死锁。
#[derive(Debug, Default)]
pub struct CookieJar {
    inner: Mutex<HashMap<String, Vec<Cookie>>>,
}

/// 解析 `Set-Cookie` 得到的属性（RFC 6265 §5.2 的 algorithm）。
#[derive(Debug, Default)]
struct ParsedSetCookie {
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
    secure: bool,
    http_only: bool,
    expires_at: Option<SystemTime>,
    /// `Max-Age=0` 或负数 → 立即删除。
    delete: bool,
}

impl CookieJar {
    /// \[DONE] 空 jar。
    pub fn new() -> Self {
        Self::default()
    }

    /// 吸收一次响应的全部 `Set-Cookie` 头。
    ///
    /// 按 RFC 6265 §5.3 的 storage model 处理：解析 → 域/路径默认与规范化 →
    /// 覆盖同名条目 → 删除过期条目。
    ///
    /// `response_headers` 传**全部**头（不是单个值），因为 HTTP 允许
    /// `Set-Cookie` 重名——只取第一个会静默丢掉登录态里靠后的 cookie。
    pub fn store_from_response(
        &self,
        key: &str,
        request_url: &str,
        response_headers: &tome_core::HeaderMap,
    ) -> Result<usize, NetError> {
        let parsed_url = reqwest::Url::parse(request_url).map_err(|error| NetError::Url {
            url: request_url.to_string(),
            reason: format!("Cookie 存储时 URL 无法解析: {error}"),
        })?;

        let mut stored = 0usize;
        let entries: Vec<Cookie> = response_headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .filter_map(|(_, value)| self.build_cookie(&parsed_url, value).transpose())
            .collect::<Result<Vec<_>, NetError>>()?;

        for cookie in entries {
            self.upsert(key, cookie);
            stored += 1;
        }
        // 存储后顺手清理过期项：避免"已过期但未删"的条目被持久化后
        // 在下次启动时又活过来。
        self.remove_expired(key, SystemTime::now());
        Ok(stored)
    }

    /// 把一条 cookie 写入指定源（同名则以新值覆盖）。
    fn upsert(&self, key: &str, cookie: Cookie) {
        // 空名 cookie 无法回传（`name=value` 里没有名字），直接丢弃而不是存下来。
        if cookie.name.is_empty() {
            return;
        }
        let mut guard = self.lock();
        let jar = guard.entry(key.to_string()).or_default();
        jar.retain(|existing| existing.identity() != cookie.identity());
        jar.push(cookie);
    }

    /// 删除指定源的过期 cookie，返回删除数量。
    pub fn remove_expired(&self, key: &str, now: SystemTime) -> usize {
        let mut guard = self.lock();
        let Some(jar) = guard.get_mut(key) else {
            return 0;
        };
        let before = jar.len();
        jar.retain(|cookie| !cookie.is_expired_at(now));
        before - jar.len()
    }

    /// 生成某请求应携带的 `Cookie` 头值。
    ///
    /// 匹配规则依 RFC 6265 §5.4：
    ///
    /// 1. **域匹配**：宿主 cookie 要求域精确相等；带 `Domain` 的 cookie 允许
    ///    域本身及其子域（前导点已在存储时规范化去掉）。
    /// 2. **路径匹配**：cookie 路径是请求路径的前缀，且边界落在 `/` 上
    ///    （`/foo` 匹配 `/foo/bar` 与 `/foo`，不匹配 `/foobar`）。
    /// 3. **Secure**：仅 https 请求可携带。
    ///
    /// 输出按**路径长度降序、同长度按创建顺序**排列（RFC 6265 §5.4 的排序要求），
    /// 因为服务端可能依赖"更具体的路径排在前面"这一惯例。
    ///
    /// 返回 `None` 表示无匹配 cookie（调用方不应设置该头）。
    pub fn header_for(&self, key: &str, request_url: &str) -> Result<Option<String>, NetError> {
        let parsed_url = reqwest::Url::parse(request_url).map_err(|error| NetError::Url {
            url: request_url.to_string(),
            reason: format!("生成 Cookie 头时 URL 无法解析: {error}"),
        })?;

        let now = SystemTime::now();
        let guard = self.lock();
        let Some(jar) = guard.get(key) else {
            return Ok(None);
        };

        let mut matched: Vec<&Cookie> = jar
            .iter()
            .filter(|cookie| !cookie.is_expired_at(now))
            .filter(|cookie| domain_matches(cookie, &parsed_url))
            .filter(|cookie| path_matches(&cookie.path, parsed_url.path()))
            .filter(|cookie| !cookie.secure || parsed_url.scheme() == "https")
            .collect();

        // 路径长的在前（更具体优先）；同长度保持 jar 内原始顺序（稳定排序）。
        matched.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));

        if matched.is_empty() {
            return Ok(None);
        }
        let header = matched
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        Ok(Some(header))
    }

    /// 枚举某源的 cookie（供持久化与调试）。结果按域名+路径+名排序，保证确定性。
    pub fn cookies_for(&self, key: &str) -> Vec<Cookie> {
        let guard = self.lock();
        let mut cookies = guard.get(key).cloned().unwrap_or_default();
        drop(guard);
        cookies.sort_by(|left, right| {
            (&left.domain, &left.path, &left.name).cmp(&(&right.domain, &right.path, &right.name))
        });
        cookies
    }

    /// 全部已登记的源标识（排序后）。
    pub fn source_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.lock().keys().cloned().collect();
        keys.sort();
        keys
    }

    /// 序列化为确定性文本（\[WORKAROUND] 格式自定，见模块文档）。
    ///
    /// 一次性持锁完成全部读取：若分多次取锁（每次 `cookies_for` 取一次），
    /// 并发的写入可能让同一份输出里混进不同时刻的快照，破坏确定性。
    pub fn to_text(&self) -> String {
        let mut out = String::from(FORMAT_HEADER);
        out.push('\n');

        let guard = self.lock();
        let mut keys: Vec<&String> = guard.keys().collect();
        keys.sort();
        for key in keys {
            let mut cookies = guard.get(key).cloned().unwrap_or_default();
            cookies.sort_by(|left, right| {
                (&left.domain, &left.path, &left.name).cmp(&(
                    &right.domain,
                    &right.path,
                    &right.name,
                ))
            });
            for cookie in cookies {
                let expires = cookie
                    .expires_at
                    .and_then(unix_from_system_time)
                    .unwrap_or(NO_EXPIRY);
                out.push_str(key);
                out.push('\t');
                out.push_str(&cookie.domain);
                out.push('\t');
                out.push_str(&cookie.path);
                out.push('\t');
                out.push_str(if cookie.secure { "1" } else { "0" });
                out.push('\t');
                out.push_str(if cookie.host_only { "1" } else { "0" });
                out.push('\t');
                out.push_str(if cookie.http_only { "1" } else { "0" });
                out.push('\t');
                out.push_str(&expires.to_string());
                out.push('\t');
                out.push_str(&cookie.name);
                out.push('\t');
                out.push_str(&cookie.value);
                out.push('\n');
            }
        }
        out
    }

    /// 从文本反序列化。格式不符的行会被跳过而非中止整个加载——
    /// 持久化文件可能因进程被杀而截断末行，那不该让用户丢掉**全部**登录态。
    ///
    /// 但格式版本不匹配是**硬错误**：那说明文件来自不兼容的版本，
    /// 猜着解析会产生语义错误的 cookie（比直接报错危险得多）。
    pub fn from_text(text: &str) -> Result<Self, NetError> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default();
        if header.trim() != FORMAT_HEADER {
            return Err(NetError::Corrupt {
                asset: "cookie-jar".to_string(),
                reason: format!("格式头不匹配，期望 {FORMAT_HEADER:?}，实际 {header:?}"),
            });
        }

        let jar = Self::new();
        for line in lines {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() != 9 {
                continue;
            }
            let expires_at = match fields[6].parse::<i64>() {
                Ok(NO_EXPIRY) => None,
                Ok(seconds) => system_time_from_unix(seconds),
                Err(_) => continue,
            };
            let cookie = Cookie {
                name: fields[7].to_string(),
                value: fields[8].to_string(),
                domain: fields[1].to_string(),
                path: fields[2].to_string(),
                secure: fields[3] == "1",
                http_only: fields[5] == "1",
                host_only: fields[4] == "1",
                expires_at,
            };
            jar.upsert(fields[0], cookie);
        }
        Ok(jar)
    }

    /// 清空指定源（登出），返回被清除的 cookie 数量。
    pub fn clear_source(&self, key: &str) -> usize {
        self.lock().remove(key).map(|jar| jar.len()).unwrap_or(0)
    }

    /// 取容器的锁。
    ///
    /// 锁中毒只可能由持锁 panic 引起；本模块锁内不做任何可能 panic 的操作
    /// （无 unwrap、无索引、无算术溢出）。即便真的中毒，也不该让一个 cookie
    /// 容器的状态把整个抓取流程带崩——取回内部数据继续工作。
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Vec<Cookie>>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// `Set-Cookie` 解析与域规范化（RFC 6265 §5.2 / §5.3）。
impl CookieJar {
    /// 由 `Set-Cookie` 值构造一条 cookie。返回 `Ok(None)` 表示该头应被忽略。
    fn build_cookie(&self, url: &reqwest::Url, raw: &str) -> Result<Option<Cookie>, NetError> {
        let Some(parsed) = parse_set_cookie(raw) else {
            return Ok(None);
        };
        if parsed.name.is_empty() {
            return Ok(None);
        }
        let Some(request_host) = url.host_str() else {
            return Ok(None);
        };

        // 域：无 Domain 属性 → 宿主 cookie；有则须与请求域匹配（RFC 6265 §5.3 step 6），
        // 否则该 cookie 必须被**拒绝**——接受它就是引入了跨站注入面。
        let (domain, host_only) = match parsed.domain {
            Some(domain) => {
                let canonical = canonical_domain(&domain);
                if !domain_matches_request(&canonical, request_host) {
                    return Ok(None);
                }
                (canonical, false)
            }
            None => (request_host.to_ascii_lowercase(), true),
        };

        // 路径：无 Path 属性 → 取请求路径的"目录"部分（RFC 6265 §5.1.4 的默认路径算法）。
        let path = parsed
            .path
            .filter(|path| path.starts_with('/'))
            .unwrap_or_else(|| default_path(url.path()));

        Ok(Some(Cookie {
            name: parsed.name,
            value: parsed.value,
            domain,
            path,
            secure: parsed.secure,
            http_only: parsed.http_only,
            host_only,
            expires_at: if parsed.delete {
                expired_marker()
            } else {
                parsed.expires_at
            },
        }))
    }
}

/// 表示"立即过期"的时间点（纪元时刻）。
fn expired_marker() -> Option<SystemTime> {
    Some(UNIX_EPOCH)
}

/// 去掉域的前导点并转小写（RFC 6265 §5.1.2 / §5.2.3）。
fn canonical_domain(domain: &str) -> String {
    domain.trim().trim_start_matches('.').to_ascii_lowercase()
}

/// RFC 6265 §5.1.4 的默认路径算法。
fn default_path(request_path: &str) -> String {
    if !request_path.starts_with('/') {
        return "/".to_string();
    }
    match request_path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(index) => request_path[..index].to_string(),
    }
}

/// 请求域能否接受该 cookie 的 `Domain` 属性（RFC 6265 §5.3 step 6）。
fn domain_matches_request(cookie_domain: &str, request_host: &str) -> bool {
    let host = request_host.to_ascii_lowercase();
    if host == cookie_domain {
        return true;
    }
    // 子域匹配：`Domain=example.com` 可作用于 `www.example.com`。
    // 注意必须比对**点边界**，否则 `notexample.com` 会被误判为 `example.com` 的子域。
    host.strip_suffix(cookie_domain)
        .is_some_and(|prefix| prefix.ends_with('.'))
}

/// cookie 能否随该请求发出（RFC 6265 §5.4 step 1：域匹配）。
fn domain_matches(cookie: &Cookie, url: &reqwest::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    if cookie.host_only {
        // 宿主 cookie：精确相等，不接受子域。
        return host == cookie.domain;
    }
    domain_matches_request(&cookie.domain, &host)
}

/// 路径匹配（RFC 6265 §5.1.4）。
fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    // 边界必须在 `/` 上：`/foo` 匹配 `/foo/bar` 但不匹配 `/foobar`。
    if cookie_path.ends_with('/') {
        return true;
    }
    request_path[cookie_path.len()..].starts_with('/')
}

/// 解析 `Set-Cookie` 头（RFC 6265 §5.2）。
///
/// 返回 `None` 表示该头格式非法、应被忽略（规范允许客户端丢弃无法解析的 cookie）。
fn parse_set_cookie(raw: &str) -> Option<ParsedSetCookie> {
    let mut parts = raw.split(';');
    let first = parts.next()?;
    let (name, value) = first.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || name.contains(|ch: char| ch.is_whitespace()) {
        return None;
    }

    let mut parsed = ParsedSetCookie {
        name: name.to_string(),
        // 值可带引号（历史写法），去掉最外层引号；内部引号原样保留。
        value: value.trim().trim_matches('"').to_string(),
        ..ParsedSetCookie::default()
    };

    for attribute in parts {
        let attribute = attribute.trim();
        if attribute.is_empty() {
            continue;
        }
        let (attribute_name, attribute_value) = match attribute.split_once('=') {
            Some((name, value)) => (name.trim(), Some(value.trim())),
            None => (attribute, None),
        };
        let lower = attribute_name.to_ascii_lowercase();
        match lower.as_str() {
            "domain" => {
                if let Some(domain) = attribute_value {
                    if !domain.is_empty() {
                        parsed.domain = Some(domain.to_string());
                    }
                }
            }
            "path" => {
                if let Some(path) = attribute_value {
                    parsed.path = Some(path.to_string());
                }
            }
            "secure" => parsed.secure = true,
            "httponly" => parsed.http_only = true,
            // Max-Age 优先于 Expires（RFC 6265 §5.3 step 3）。
            "max-age" => {
                if let Some(seconds) = attribute_value.and_then(parse_max_age_seconds) {
                    // 零或负数按规范表示"立即删除"。
                    if seconds <= 0 {
                        parsed.delete = true;
                    } else if let Some(expiry) = SystemTime::now()
                        // 截断到整秒：`Max-Age` 的粒度是秒，保留亚秒部分会让
                        // 序列化（整秒）与内存值不符，往返后不再相等。
                        .checked_add(Duration::from_secs(seconds as u64))
                        .and_then(truncate_to_second)
                    {
                        parsed.expires_at = Some(expiry);
                    }
                }
            }
            // Expires 只在 Max-Age 未给出时生效（RFC 6265 §5.3 step 3：
            // Max-Age 优先于 Expires）。
            "expires" if parsed.expires_at.is_none() && !parsed.delete => {
                parsed.expires_at = attribute_value.and_then(parse_http_date);
            }
            // SameSite 与其余未知属性：本层不做站点隔离判断（那是浏览器 UI 的关切，
            // 书源抓取是"用户明确触发的取数"，不存在第三方上下文）。
            // 保留原值不解析，避免把未知属性误当成有效语义。
            _ => {}
        }
    }
    Some(parsed)
}

/// 解析 `Max-Age` 的秒数。非数字或缺失返回 `None`（该属性被忽略）。
fn parse_max_age_seconds(value: &str) -> Option<i64> {
    value.trim().parse().ok()
}

/// 解析 `Expires` 的 HTTP-date。
///
/// 支持 RFC 9110 §5.6.7 规定的三种格式：
///
/// - IMF-fixdate：`Wed, 09 Jun 2021 10:18:14 GMT`
/// - RFC 850：`Wednesday, 09-Jun-21 10:18:14 GMT`
/// - asctime：`Wed Jun  9 10:18:14 2021`
///
/// 全部手工解析，不引入日期库——本层只需读出年月日时分秒，而三种格式的差异
/// 只在"星期几怎么分隔"与"日期段里日/月/年的先后"。故做法是先把三段共性
/// （剥星期几 → 找时间 token → 剩下一段日期描述）提取出来，再按形状分支。
///
/// 时区一律按 UTC 处理：规范要求 HTTP-date 用 GMT，其他时区表示属非法输入。
fn parse_http_date(value: &str) -> Option<SystemTime> {
    let value = value.trim();
    // 剥掉星期几：IMF-fixdate 与 RFC 850 用逗号分隔，asctime 用一个空格分隔。
    let rest = match value.split_once(',') {
        Some((_weekday, rest)) => rest.trim(),
        None => value.split_once(' ').map(|(_weekday, rest)| rest)?.trim(),
    };

    let tokens: Vec<&str> = rest.split_whitespace().collect();
    // 时间 token 是唯一含冒号的那个。
    let time_text = *tokens.iter().find(|token| token.contains(':'))?;
    let date_tokens: Vec<&str> = tokens
        .iter()
        .filter(|token| !token.contains(':') && !is_zone_token(token))
        .copied()
        .collect();

    let (day, month, year_text) = match date_tokens.as_slice() {
        // 三段式：IMF-fixdate `09 Jun 2021` 或 asctime `Jun 9 2021`。
        // 两者的区别只在日与月的先后，用"第一段能否当数字解析"来分辨。
        [first, second, third] => match first.parse::<u32>() {
            Ok(day) => (day, *second, *third),
            Err(_) => (second.parse::<u32>().ok()?, *first, *third),
        },
        // RFC 850 的日期段是一整段带连字符的 `09-Jun-21`。
        [hyphenated] => {
            let parts: Vec<&str> = hyphenated.split('-').collect();
            match parts.as_slice() {
                [day, month, year] => (day.parse::<u32>().ok()?, *month, *year),
                _ => return None,
            }
        }
        _ => return None,
    };

    let month = month_from_name(month)?;
    let mut year: i32 = year_text.parse().ok()?;
    // RFC 850 的两位年份：按 RFC 9110 的规则，相差 50 年以内的年份视为过去。
    // 以 1970 为界而非"当前年"是刻意的：解析必须是纯函数，否则同一份
    // 响应在不同时间解析出不同结果，差额测试就失去确定性。
    if year_text.len() == 2 {
        year += if year < 70 { 2000 } else { 1900 };
    }

    let mut clock = time_text.split(':');
    let hour: u32 = clock.next()?.parse().ok()?;
    let minute: u32 = clock.next()?.parse().ok()?;
    let second: u32 = clock.next()?.parse().ok()?;
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let seconds = days_from_civil(year, month, day)? * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        // 闰秒（60）向下钳到 59：Unix 时间戳无法表示它，且规范允许这样接受。
        + i64::from(second.min(59));
    system_time_from_unix(seconds)
}

/// 时区标记（HTTP-date 只允许 GMT，但 `UTC` 在实践中也出现）。
fn is_zone_token(token: &str) -> bool {
    token.eq_ignore_ascii_case("gmt") || token.eq_ignore_ascii_case("utc")
}

/// 月份名 → 月号（RFC 9110 的月份缩写）。
fn month_from_name(name: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = name.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|month| lower.starts_with(month))
        .map(|index| index as u32 + 1)
}

/// 公历日期 → 自纪元起的天数（Howard Hinnant 的 days_from_civil 算法）。
///
/// 用算法的原式而非 `chrono`：本层只需要一个纯函数式换算，
/// 不为一个日期解析引入日期库依赖（体积纪律）。
///
/// 返回值以 1970-01-01 为 0；公元前/非法月份返回 `None`。
fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || day == 0 || day > 31 {
        return None;
    }
    // 把一月二月挪到上一年，使闰日落在年末，简化后续算式。
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = (adjusted_year - era * 400) as i64;
    let month_offset = if month > 2 {
        month as i64 - 3
    } else {
        month as i64 + 9
    };
    let day_of_year = (153 * month_offset + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era as i64 * 146_097 + day_of_era - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tome_core::HeaderMap;

    fn headers_with_set_cookie(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append("Set-Cookie", *value);
        }
        headers
    }

    #[test]
    fn stores_and_sends_session_cookie() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "source-a",
            "https://example.com/login",
            &headers_with_set_cookie(&["sid=abc123; Path=/"]),
        )
        .expect("存储应成功");

        let header = jar
            .header_for("source-a", "https://example.com/book/1")
            .expect("查询应成功");
        assert_eq!(header.as_deref(), Some("sid=abc123"));
    }

    /// T016 验收：按源隔离——A 源的 cookie 不出现在 B 源请求里。
    #[test]
    fn cookies_are_isolated_per_source() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "source-a",
            "https://example.com/",
            &headers_with_set_cookie(&["token=aaa", "who=alice"]),
        )
        .expect("存储应成功");
        jar.store_from_response(
            "source-b",
            "https://example.com/",
            &headers_with_set_cookie(&["token=bbb", "who=bob"]),
        )
        .expect("存储应成功");

        let a = jar
            .header_for("source-a", "https://example.com/x")
            .expect("查询应成功");
        let b = jar
            .header_for("source-b", "https://example.com/x")
            .expect("查询应成功");
        assert!(a.as_deref().unwrap_or_default().contains("token=aaa"));
        assert!(!a.as_deref().unwrap_or_default().contains("bbb"));
        assert!(b.as_deref().unwrap_or_default().contains("token=bbb"));
        assert!(!b.as_deref().unwrap_or_default().contains("aaa"));

        // 未登记的源不应拿到任何 cookie。
        assert_eq!(
            jar.header_for("source-c", "https://example.com/x")
                .expect("查询应成功"),
            None
        );
    }

    #[test]
    fn domain_attribute_allows_subdomains() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["wide=1; Domain=example.com"]),
        )
        .expect("存储应成功");

        assert!(jar
            .header_for("s", "https://www.example.com/")
            .expect("查询应成功")
            .is_some());
        assert!(jar
            .header_for("s", "https://example.com/")
            .expect("查询应成功")
            .is_some());
        // 点边界必须成立：notexample.com 不是 example.com 的子域。
        assert_eq!(
            jar.header_for("s", "https://notexample.com/")
                .expect("查询应成功"),
            None
        );
    }

    #[test]
    fn host_only_cookie_does_not_leak_to_subdomain() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["narrow=1"]),
        )
        .expect("存储应成功");
        assert_eq!(
            jar.header_for("s", "https://www.example.com/")
                .expect("查询应成功"),
            None,
            "无 Domain 属性的 cookie 不应发往子域"
        );
    }

    #[test]
    fn domain_attribute_for_unrelated_host_is_rejected() {
        let jar = CookieJar::new();
        // 请求 example.com 却被要求设置 evil.com 的 cookie —— 必须拒绝。
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["attack=1; Domain=evil.com"]),
        )
        .expect("解析应成功");
        assert!(
            jar.cookies_for("s").is_empty(),
            "跨站 Domain 属性必须被拒绝（RFC 6265 §5.3）"
        );
    }

    #[test]
    fn path_matching_respects_segment_boundary() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["scoped=1; Path=/foo"]),
        )
        .expect("存储应成功");

        let matched = |url: &str| jar.header_for("s", url).expect("查询应成功").is_some();
        assert!(matched("https://example.com/foo"));
        assert!(matched("https://example.com/foo/bar"));
        assert!(!matched("https://example.com/foobar"), "点边界不成立");
        assert!(!matched("https://example.com/"));
        assert!(!matched("https://example.com/bar"));
    }

    #[test]
    fn secure_cookie_requires_https() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["sec=1; Secure"]),
        )
        .expect("存储应成功");

        assert!(jar
            .header_for("s", "https://example.com/")
            .expect("查询应成功")
            .is_some());
        assert_eq!(
            jar.header_for("s", "http://example.com/")
                .expect("查询应成功"),
            None,
            "Secure cookie 不得经 http 发出"
        );
    }

    #[test]
    fn expired_cookie_is_not_sent() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["gone=1; Expires=Thu, 01 Jan 1970 00:00:00 GMT"]),
        )
        .expect("存储应成功");
        assert_eq!(
            jar.header_for("s", "https://example.com/")
                .expect("查询应成功"),
            None
        );
    }

    #[test]
    fn max_age_zero_deletes_existing_cookie() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["sid=abc"]),
        )
        .expect("存储应成功");
        assert!(jar
            .header_for("s", "https://example.com/")
            .expect("查询应成功")
            .is_some());

        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["sid=; Max-Age=0"]),
        )
        .expect("存储应成功");
        assert_eq!(
            jar.header_for("s", "https://example.com/")
                .expect("查询应成功"),
            None,
            "Max-Age=0 应删除该 cookie"
        );
    }

    #[test]
    fn max_age_sets_future_expiry() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["sid=abc; Max-Age=3600"]),
        )
        .expect("存储应成功");
        assert!(jar
            .header_for("s", "https://example.com/")
            .expect("查询应成功")
            .is_some());
    }

    #[test]
    fn later_set_cookie_overwrites_same_identity() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["sid=old"]),
        )
        .expect("存储应成功");
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["sid=new"]),
        )
        .expect("存储应成功");

        let cookies = jar.cookies_for("s");
        assert_eq!(cookies.len(), 1, "同名同域同路径应覆盖而非并存");
        assert_eq!(cookies[0].value, "new");
    }

    #[test]
    fn multiple_set_cookie_headers_all_land() {
        let jar = CookieJar::new();
        let stored = jar
            .store_from_response(
                "s",
                "https://example.com/",
                &headers_with_set_cookie(&["a=1", "b=2", "c=3"]),
            )
            .expect("存储应成功");
        assert_eq!(stored, 3, "重名的 Set-Cookie 头不得被合并丢失");
        let header = jar
            .header_for("s", "https://example.com/")
            .expect("查询应成功")
            .expect("应有 cookie");
        for name in ["a=1", "b=2", "c=3"] {
            assert!(header.contains(name), "缺少 {name}，实际 {header}");
        }
    }

    #[test]
    fn attributes_are_parsed_case_insensitively() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/dir/page",
            &headers_with_set_cookie(&["X=1; PATH=/dir; SECURE; HTTPONLY"]),
        )
        .expect("存储应成功");
        let cookie = jar
            .cookies_for("s")
            .into_iter()
            .next()
            .expect("应有 cookie");
        assert_eq!(cookie.path, "/dir");
        assert!(cookie.secure);
        assert!(cookie.http_only);
    }

    #[test]
    fn quoted_value_is_unquoted() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["q=\"hello world\""]),
        )
        .expect("存储应成功");
        let cookie = jar
            .cookies_for("s")
            .into_iter()
            .next()
            .expect("应有 cookie");
        assert_eq!(cookie.value, "hello world");
    }

    #[test]
    fn default_path_follows_rfc6265() {
        assert_eq!(default_path("/dir/page.html"), "/dir");
        assert_eq!(default_path("/page.html"), "/");
        assert_eq!(default_path("/"), "/");
        assert_eq!(default_path("/a/b/c"), "/a/b");
    }

    #[test]
    fn cookie_without_path_attribute_uses_default_path() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/dir/page.html",
            &headers_with_set_cookie(&["x=1"]),
        )
        .expect("存储应成功");
        let cookie = jar
            .cookies_for("s")
            .into_iter()
            .next()
            .expect("应有 cookie");
        assert_eq!(cookie.path, "/dir");
    }

    #[test]
    fn malformed_set_cookie_is_ignored() {
        let jar = CookieJar::new();
        let stored = jar
            .store_from_response(
                "s",
                "https://example.com/",
                &headers_with_set_cookie(&["no-equals-sign", "=novalue", "= = "]),
            )
            .expect("解析应成功");
        assert_eq!(stored, 0);
        assert!(jar.cookies_for("s").is_empty());
    }

    /// T016 验收：往返序列化。
    #[test]
    fn serialization_round_trips() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "source-a",
            "https://example.com/dir/",
            &headers_with_set_cookie(&[
                "sid=abc123; Path=/; Secure; HttpOnly; Max-Age=3600",
                "pref=dark; Path=/dir",
                "wide=1; Domain=example.com; Expires=Wed, 09 Jun 2030 10:18:14 GMT",
            ]),
        )
        .expect("存储应成功");
        jar.store_from_response(
            "source-b",
            "https://other.test/",
            &headers_with_set_cookie(&["other=xyz"]),
        )
        .expect("存储应成功");

        let text = jar.to_text();
        let restored = CookieJar::from_text(&text).expect("反序列化应成功");

        assert_eq!(
            restored.cookies_for("source-a"),
            jar.cookies_for("source-a")
        );
        assert_eq!(
            restored.cookies_for("source-b"),
            jar.cookies_for("source-b")
        );
        assert_eq!(restored.to_text(), text, "往返后文本应逐字节相同");
    }

    #[test]
    fn serialization_is_deterministic_regardless_of_insertion_order() {
        let first = CookieJar::new();
        first
            .store_from_response(
                "s",
                "https://example.com/",
                &headers_with_set_cookie(&["b=2", "a=1", "c=3"]),
            )
            .expect("存储应成功");

        let second = CookieJar::new();
        second
            .store_from_response(
                "s",
                "https://example.com/",
                &headers_with_set_cookie(&["c=3", "a=1", "b=2"]),
            )
            .expect("存储应成功");

        assert_eq!(first.to_text(), second.to_text());
    }

    #[test]
    fn session_cookies_survive_round_trip_without_expiry() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["session=1"]),
        )
        .expect("存储应成功");
        let restored = CookieJar::from_text(&jar.to_text()).expect("反序列化应成功");
        assert_eq!(
            restored.cookies_for("s")[0].expires_at,
            None,
            "会话 cookie 不应获得伪造的过期时间"
        );
    }

    #[test]
    fn from_text_rejects_wrong_format_header() {
        let error =
            CookieJar::from_text("# some other format\nx\ty\tz").expect_err("格式头不符应报错");
        assert!(matches!(error, NetError::Corrupt { .. }));
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Configuration);
    }

    #[test]
    fn from_text_skips_malformed_lines() {
        let text = format!(
            "{FORMAT_HEADER}\n\
             s\texample.com\t/\t0\t1\t0\t-1\tgood\t1\n\
             broken line without tabs\n\
             s\texample.com\t/\t0\t1\t0\tnot-a-number\tbad\t2\n\
             s\texample.com\t/\t0\t1\t0\t-1\talso-good\t3\n"
        );
        let jar = CookieJar::from_text(&text).expect("应容忍坏行");
        let names: Vec<String> = jar.cookies_for("s").into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["also-good".to_string(), "good".to_string()]);
    }

    #[test]
    fn clear_source_removes_only_that_source() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "a",
            "https://example.com/",
            &headers_with_set_cookie(&["x=1", "y=2"]),
        )
        .expect("存储应成功");
        jar.store_from_response(
            "b",
            "https://example.com/",
            &headers_with_set_cookie(&["z=3"]),
        )
        .expect("存储应成功");

        assert_eq!(jar.clear_source("a"), 2);
        assert_eq!(
            jar.header_for("a", "https://example.com/")
                .expect("查询应成功"),
            None
        );
        assert!(jar
            .header_for("b", "https://example.com/")
            .expect("查询应成功")
            .is_some());
    }

    #[test]
    fn longer_path_sorts_first() {
        let jar = CookieJar::new();
        jar.store_from_response(
            "s",
            "https://example.com/",
            &headers_with_set_cookie(&["root=1; Path=/"]),
        )
        .expect("存储应成功");
        jar.store_from_response(
            "s",
            "https://example.com/deep/path/",
            &headers_with_set_cookie(&["deep=1; Path=/deep/path"]),
        )
        .expect("存储应成功");

        let header = jar
            .header_for("s", "https://example.com/deep/path/x")
            .expect("查询应成功")
            .expect("应有 cookie");
        assert!(
            header.starts_with("deep=1"),
            "更具体的路径应排在前面，实际 {header}"
        );
    }

    #[test]
    fn invalid_url_is_reported_not_panicked() {
        let jar = CookieJar::new();
        let error = jar
            .header_for("s", "not a url")
            .expect_err("非法 URL 应报错");
        assert!(matches!(error, NetError::Url { .. }));
    }

    #[test]
    fn http_date_parsing_handles_rfc_formats() {
        let check = |input: &str, expected: i64| {
            let parsed = parse_http_date(input).expect("应能解析");
            let seconds = unix_from_system_time(parsed).expect("应在纪元之后");
            assert_eq!(seconds, expected, "输入 {input}");
        };
        // IMF-fixdate：2021-06-09T10:18:14Z = 1623233894
        check("Wed, 09 Jun 2021 10:18:14 GMT", 1_623_233_894);
        // asctime 格式（无逗号）
        check("Wed Jun  9 10:18:14 2021", 1_623_233_894);
        // RFC 850 格式（两位年份 + 连字符日期）
        check("Wednesday, 09-Jun-21 10:18:14 GMT", 1_623_233_894);
        // 两位年份的窗口：`00` 应落在 2000 年（2000-06-09T10:18:14Z = 960545894）
        check("Wed, 09 Jun 00 10:18:14 GMT", 960_545_894);
    }

    #[test]
    fn http_date_parsing_rejects_garbage() {
        assert_eq!(parse_http_date("not a date"), None);
        assert_eq!(parse_http_date("Wed, 09 Foo 2021 10:18:14 GMT"), None);
        assert_eq!(parse_http_date("Wed, 32 Jan 2021 10:18:14 GMT"), None);
    }

    #[test]
    fn days_from_civil_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(days_from_civil(1970, 1, 2), Some(1));
        assert_eq!(days_from_civil(1969, 12, 31), Some(-1));
        // 2000-03-01 = 11017（闰年 2 月之后）
        assert_eq!(days_from_civil(2000, 3, 1), Some(11_017));
        assert_eq!(days_from_civil(2000, 13, 1), None);
        assert_eq!(days_from_civil(2000, 1, 0), None);
    }
}
