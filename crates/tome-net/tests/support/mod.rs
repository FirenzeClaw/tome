//! 回环测试服务器：手写 HTTP/1.1 响应，**不引入任何依赖**（tasks.md T013/T014 验收）。
//!
//! 为什么不用现成的 HTTP 测试库（如 `wiremock`、`mockito`）：tasks.md 与
//! research.md 未点名任何此类依赖，而本项目对未点名依赖持保守态度
//! （体积纪律 + 供应链面）。手写一个只支持"读请求 → 回一个固定响应"的服务器
//! 足够覆盖 net 层的验收点（方法/头/体/超时/重试/限速），体量可控。
//!
//! # 它必须支持的最小集合
//!
//! - 读请求行与头（用于断言"POST 的体确实发出去了"、"自定义头到了"）。
//! - 读定长体（`Content-Length`）。
//! - 回一个可参数化的响应（状态、头、体、或"故意不回"以触发超时）。
//! - 记录收到的请求，供测试断言重试次数与限速时序。
//!
//! # 只监听回环地址
//!
//! 绑定 `127.0.0.1:0`（端口由内核分配），测试不会外发任何流量——
//! 这也是 `client::validate_url` 允许 http 的唯一例外场景。
//!
//! # 关于夹具里的 `expect`
//!
//! [WORKAROUND] `.clippy.toml` 的 `allow-expect-in-tests` 只覆盖 `#[test]`
//! 函数与 `#[cfg(test)]` 模块，**不覆盖 tests/ 下的公共夹具函数**（那正是
//! 本文件）。夹具失败就意味着测试环境本身有问题（绑定不了回环端口），
//! 此时 panic 是唯一合理的反应——返回 `Result` 会逼每个测试写一遍
//! 无意义的错误传播。故在此显式豁免并说明理由。
#![expect(
    clippy::expect_used,
    reason = "测试夹具无法构造时应立即失败，错误传播只会给每个调用点加噪音"
)]
// 本模块被**多个**集成测试二进制共用（`net_integration.rs` / `host_wiring.rs`），
// 每个二进制独立编译它一遍，于是"对某个二进制而言未使用"的项会被 dead_code
// 点名。那些项对另一个二进制是必需的（如 `Reply::HangFor` 只在超时测试里用）。
// 为迁就单个二进制的可见性而删掉它们，等于让另一个测试失去覆盖手段。
#![expect(
    dead_code,
    reason = "共享夹具：各项由不同测试二进制分别使用，汇合后全部有用"
)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 服务器要回给客户端的东西。
#[derive(Debug, Clone)]
pub enum Reply {
    /// 正常响应：状态行 + 头 + 体。
    Ok {
        /// 状态码
        status: u16,
        /// 状态短语（如 `OK`）
        reason: String,
        /// 额外响应头（`(名字, 值)`），可重名
        headers: Vec<(String, String)>,
        /// 响应体
        body: Vec<u8>,
    },
    /// 只回状态行与头，然后在读体之前断开（用于触发读体失败/重试）。
    AbortAfterHeaders {
        /// 状态码
        status: u16,
    },
    /// 什么都不回，让客户端超时。参数是保持连接的时长。
    HangFor(Duration),
    /// 收到连接立刻关闭（模拟目标站不可达的另一种形态）。
    Close,
}

impl Reply {
    /// 便捷构造：200 + 纯文本体。
    pub fn text(body: impl Into<String>) -> Self {
        let body = body.into().into_bytes();
        Self::Ok {
            status: 200,
            reason: "OK".to_string(),
            headers: vec![(
                "Content-Type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            )],
            body,
        }
    }

    /// 便捷构造：指定状态码 + 纯文本体。
    pub fn status_text(status: u16, body: impl Into<String>) -> Self {
        Self::Ok {
            status,
            reason: "Status".to_string(),
            headers: vec![(
                "Content-Type".to_string(),
                "text/plain; charset=utf-8".to_string(),
            )],
            body: body.into().into_bytes(),
        }
    }
}

/// 服务器收到的一次请求（供断言）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    /// HTTP 方法（大写）
    pub method: String,
    /// 请求目标（路径 + query）
    pub target: String,
    /// 请求头（保序，含重名）
    pub headers: Vec<(String, String)>,
    /// 请求体
    pub body: Vec<u8>,
    /// 收到该请求的时刻
    pub received_at: Instant,
}

impl RecordedRequest {
    /// 取某个头（大小写不敏感，返回首个）。
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// 取某个头的全部值（含重名）。
    pub fn header_values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(existing, _)| existing.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect()
    }
}

/// 回环测试服务器。
///
/// 析构时置停止标志并主动连一次自己，让 `accept` 循环醒来退出——
/// 避免测试结束时留下悬挂线程（否则 `cargo test` 会因线程未结束而拖慢）。
pub struct TestServer {
    address: String,
    received: Arc<Mutex<Vec<RecordedRequest>>>,
    connection_count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    accept_thread: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    /// 启动服务器。`replies` 按**连接序**逐个使用；用尽后重复最后一个。
    ///
    /// 按连接序（而非按请求序）分发是刻意的：重试场景下，客户端会新建连接，
    /// 正是"不同连接回不同响应"才能测出重试确实发生了。
    pub fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("应能绑定回环端口");
        let address = listener.local_addr().expect("应能取本地地址").to_string();

        let received = Arc::new(Mutex::new(Vec::new()));
        let connection_count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let received_for_thread = Arc::clone(&received);
        let count_for_thread = Arc::clone(&connection_count);
        let stop_for_thread = Arc::clone(&stop);

        let accept_thread = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_for_thread.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else {
                    continue;
                };
                let index = count_for_thread.fetch_add(1, Ordering::SeqCst);
                let reply = replies
                    .get(index)
                    .or_else(|| replies.last())
                    .cloned()
                    .unwrap_or_else(|| Reply::text("ok"));
                let received = Arc::clone(&received_for_thread);
                thread::spawn(move || serve(stream, reply, received));
            }
        });

        Self {
            address,
            received,
            connection_count,
            stop,
            accept_thread: Some(accept_thread),
        }
    }

    /// 服务器的 base URL（形如 `http://127.0.0.1:34567`）。
    pub fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// 构造某个路径的完整 URL。
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    /// 已接受的连接数（重试会让它增长）。
    pub fn connection_count(&self) -> usize {
        self.connection_count.load(Ordering::SeqCst)
    }

    /// 收到的全部请求（按到达顺序）。
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.received.lock().expect("锁不应中毒").clone()
    }

    /// 收到的请求数。
    pub fn request_count(&self) -> usize {
        self.received.lock().expect("锁不应中毒").len()
    }

    /// 等待至少 `count` 个请求到达（带上限，避免测试永久挂住）。
    pub fn wait_for_requests(&self, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.request_count() >= count {
                return true;
            }
            thread::sleep(Duration::from_millis(2));
        }
        self.request_count() >= count
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // 连一次自己把 accept 循环从阻塞中叫醒。
        let _ = TcpStream::connect(&self.address);
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
    }
}

/// 处理单个连接：读请求、记录、回响应。
fn serve(mut stream: TcpStream, reply: Reply, received: Arc<Mutex<Vec<RecordedRequest>>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let Some(request) = read_request(&stream) else {
        return;
    };
    received.lock().expect("锁不应中毒").push(request);

    match reply {
        Reply::Ok {
            status,
            reason,
            headers,
            body,
        } => {
            let mut response = Vec::new();
            let head = format!("HTTP/1.1 {status} {reason}\r\n");
            response.extend_from_slice(head.as_bytes());
            // 显式给出 Content-Length：客户端据此知道体何时结束。
            response.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
            response.extend_from_slice(b"Connection: close\r\n");
            for (name, value) in headers {
                response.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
            }
            response.extend_from_slice(b"\r\n");
            response.extend_from_slice(&body);
            let _ = stream.write_all(&response);
            let _ = stream.flush();
        }
        Reply::AbortAfterHeaders { status } => {
            // 声明一个长度却只发头——客户端读体时必然失败。
            let head =
                format!("HTTP/1.1 {status} X\r\nContent-Length: 100\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();
            let _ = stream.shutdown(Shutdown::Both);
        }
        Reply::HangFor(duration) => {
            // 不回任何字节，让客户端等到超时。
            thread::sleep(duration);
            let _ = stream.shutdown(Shutdown::Both);
        }
        Reply::Close => {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

/// 读一个 HTTP/1.1 请求（请求行 + 头 + 定长体）。
fn read_request(stream: &TcpStream) -> Option<RecordedRequest> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();

    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).ok()?;
    }

    Some(RecordedRequest {
        method,
        target,
        headers,
        body,
        received_at: Instant::now(),
    })
}
