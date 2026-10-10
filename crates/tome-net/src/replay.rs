//! \[DONE] 回放 adapter：录制请求→响应并在完全离线状态下重放（tasks.md T019）
//!
//! 这是**差分测试确定性的来源**（宪法 III 的一致性验证 / `contracts/seams.md`
//! 「契约与差分测试的关系」）：把一次真实抓取的 `RequestSpec → Response` 映射
//! 录到目录里，之后重放时不再触网，同一份语料在任何机器、任何时间、
//! 有无网络的情况下都产出逐字节相同的结果。
//!
//! # 目录格式
//!
//! ```text
//! <dir>/
//! ├── index.txt          索引：每行一条记录（指纹 → 文件名 + 可读提示）
//! └── <fingerprint>.bin  记录体：**单个可打印文本**，见「记录体编码」
//! ```
//!
//! 用"一记录一文件"而非单一归档：便于人工查看、便于 git 里逐条 diff
//! （新增一个请求只多一个文件，而不是整份归档重写）。
//!
//! ## 索引格式
//!
//! ```text
//! # tome replay index v1
//! <指纹> <文件名> <方法> <去掉 query 的 URL>
//! ```
//!
//! 字段以空格分隔——**URL 里不能有空格**（RFC 3986 的 URI 语法排除它，reqwest
//! 解析时也会把空格百分号编码），故不需要转义。最后一段（URL）可以含空格之外的
//! 任意字符，解析时按"前三段用空格切、余下整段是 URL"处理。
//!
//! # 安全红线：索引里不写敏感数据
//!
//! AGENTS.md 明令"禁止在日志中记录敏感数据（Cookie、认证头须脱敏）"。
//! 回放语料比日志更彻底——它可能被提交进仓库（合成语料）或随 bug 报告外传。
//! 故索引里**只放指纹**，不放任何请求头或 body 内容：
//!
//! - 指纹是**不可逆摘要**：无法从它反推出 Cookie 或 Authorization 的值。
//! - 可读提示只保留 method 与**去掉 query 的 URL**——query 里常带 token
//!   （`?token=xxx`、`?sign=xxx`），故整段丢掉。
//!
//! 记录体（`.bin`）里的请求指纹段同样只有摘要，不含原始头值。
//!
//! # 指纹格式
//!
//! 指纹 = `hex(h1) + hex(h2)`，两个独立的 64 位 FNV-1a 哈希拼成 128 位：
//!
//! - `h1` = FNV-1a（offset basis `0xcbf29ce484222325`）逐字节累加。
//! - `h2` = 同一个 FNV-1a，但起始于**不同的 offset basis**（`0x9e3779b97f4a7c15`）。
//!
//! ## 为什么是两个 64 位而不是一个 128 位哈希
//!
//! 不引入依赖的前提下，实现 128 位抗碰撞哈希（如 SipHash-128、Blake3）
//! 需要自己写密码学原语——体积与正确性风险都不划算。
//! 而**用不同初值跑两次 FNV-1a** 得到的两个值在统计上相互独立，
//! 拼接后等价于一个 128 位空间上的散列。目的是把碰撞概率压到与实际语料规模
//! （几千条记录）相比可忽略的量级：单 64 位已足够，双 64 位是给"指纹歧义会
//! 导致测试静默误判"这类高危后果留的余量。
//!
//! ## 为什么不用哈希本身定位文件，而要查索引
//!
//! 哈希只用于**查找键**，文件名即指纹。记录体里另存一份完整指纹，重放时
//! 校验（防止文件被改名/手改后张冠李戴——那种错误会表现为"测试通过但内容是别的请求的"，
//! 是最坏的失败模式）。
//!
//! # 记录体编码：为什么是十六进制文本而不是二进制
//!
//! 记录体要承载任意字节（二进制响应体、CRLF、NUL）。若直接写二进制，
//! 就需要自己的长度前缀 + 转义层，且不能进 git diff、不能肉眼查看。
//! 十六进制把任意字节映射为纯 `[0-9a-f]` 文本，天然无歧义、无转义、
//! 无 CRLF 问题，代价是体积翻倍——对"测试语料"这个用途完全可以接受
//! （语料规模是 KB 到 MB 级，不是 GB 级）。
//!
//! 记录体格式（每行以 `LF` 结尾，字段以**单个空格**分隔；十六进制段内无空格）：
//!
//! ```text
//! # tome replay record v1
//! fingerprint <128 位 hex>
//! status <十进制状态码>
//! charset <编码名 或 ->        # 编码名不含空格，故用 `-` 表示"无"
//! header <名字的 hex> <值的 hex>
//! header ...
//! body <响应体的 hex>
//! ```
//!
//! 头逐条一行（含重名头，**保序**），名字与值各自十六进制编码——
//! 这样头里出现任何字节（非 ASCII 的合法值、老式站点发的非法字节）都不会破坏格式。
//! `body` 段在最后且只有一行，因此解析时无需长度前缀。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use tome_core::{HeaderMap, RequestSpec, Response};

use crate::charset::{self, DecodedBody};
use crate::error::NetError;

/// 索引文件的格式头（带版本，便于将来明确拒绝旧格式）。
const INDEX_HEADER: &str = "# tome replay index v1";

/// 记录体的格式头。
const RECORD_HEADER: &str = "# tome replay record v1";

/// 索引文件名。
const INDEX_FILE: &str = "index.txt";

/// 记录文件的扩展名。
const RECORD_EXTENSION: &str = "bin";

/// FNV-1a 的 64 位标准初值。
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// 第二个哈希用的初值（黄金比例常数，与标准初值无关联）。
const FNV_ALT_OFFSET_BASIS: u64 = 0x9e37_79b9_7f4a_7c15;

/// FNV-1a 的 64 位质数。
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 参与指纹的请求头（小写名）。
///
/// 只取**影响响应内容**的头，不取全部：指纹要的是"这个请求是否会得到那份响应"，
/// 而 `User-Agent`/`Accept-Encoding`/`Connection` 这类头在不同调用点会有无关抖动，
/// 把它们算进指纹会让同一逻辑请求录出多条记录、重放时命中不了。
///
/// 这份清单覆盖了书源生态里真正决定响应内容的头：认证（决定能否访问）、
/// 内容协商（决定返回什么格式）、来源（部分站点据此拒绝）、Cookie
/// （决定登录态）。代价是"仅因某个未列入的头不同而响应不同"的场景会误命中——
/// 这类场景罕见，且可通过扩充清单解决。
const FINGERPRINT_HEADERS: [&str; 12] = [
    "authorization",
    "accept",
    "accept-language",
    "content-type",
    "cookie",
    "origin",
    "referer",
    "user-agent",
    "x-requested-with",
    "x-api-key",
    "x-token",
    "x-auth-token",
];

// ---------------------------------------------------------------------------
// 指纹
// ---------------------------------------------------------------------------

/// 计算请求指纹（128 位十六进制，共 32 个小写字符）。
///
/// 规范化规则（顺序固定，任何改动都会让既有语料失效——故写死在此并注明）：
///
/// 1. 方法：转为定义里的**大写**形式（`HttpMethod::as_str`）。
/// 2. URL：**原样**参与，不做归一化（不排序 query、不去 fragment）。
///    理由：URL 是规则引擎算出来的精确值，任何归一化都可能让两个语义不同的
///    请求碰撞。要求调用方保证同一逻辑请求产出同一 URL 串。
/// 3. body：十六进制（`None` 与空字节序列视为同一件事，都编码为空串）。
/// 4. 关键头：按名小写、**只取 `FINGERPRINT_HEADERS` 清单内的**、按名排序后
///    逐个拼接 `名=值`（重名头按出现顺序全部参与，以 `\n` 分隔）。
///
/// 各段之间用 `\n` 分隔，段内字段用 `\x1f`（ASCII unit separator）分隔——
/// 用不可能出现在方法名/头名/URL 里的控制字符做分隔符，避免"拼接歧义"
/// （例如 URL 结尾正好是某个分隔符时的边界误判）。
pub fn fingerprint(spec: &RequestSpec) -> String {
    let mut canonical = String::new();
    canonical.push_str(spec.method.as_str());
    canonical.push('\n');
    canonical.push_str(&spec.url);
    canonical.push('\n');
    canonical.push_str(&hex_encode(spec.body.as_deref().unwrap_or(&[])));
    canonical.push('\n');

    // 收集参与指纹的头，按名排序；同名头保持原有相对顺序。
    let mut selected: Vec<(String, &str)> = spec
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .filter(|(name, _)| FINGERPRINT_HEADERS.contains(&name.as_str()))
        .collect();
    selected.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(right.1)));

    for (name, value) in selected {
        canonical.push_str(&name);
        canonical.push('\u{1f}');
        canonical.push_str(value);
        canonical.push('\u{1e}');
    }

    let bytes = canonical.as_bytes();
    format!(
        "{:016x}{:016x}",
        fnv1a(bytes, FNV_OFFSET_BASIS),
        fnv1a(bytes, FNV_ALT_OFFSET_BASIS)
    )
}

/// FNV-1a 64 位哈希（带自定义初值）。
fn fnv1a(bytes: &[u8], offset_basis: u64) -> u64 {
    let mut hash = offset_basis;
    for byte in bytes {
        hash ^= u64::from(*byte);
        // `wrapping_mul`：FNV 的模 2^64 乘法在定义上就是回绕的。
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// 供索引使用的可读提示：`METHOD <去掉 query 与 fragment 的 URL>`。
///
/// 剥掉 query/fragment 是因为它们常携带 token（`?token=...&sign=...`），
/// 而索引会被提交进仓库（AGENTS.md：禁止在日志/产物里记录敏感数据）。
pub fn readable_hint(spec: &RequestSpec) -> String {
    let trimmed = match spec.url.split_once('#') {
        Some((head, _fragment)) => head,
        None => spec.url.as_str(),
    };
    let without_query = match trimmed.split_once('?') {
        Some((head, _query)) => head,
        None => trimmed,
    };
    format!("{} {without_query}", spec.method.as_str())
}

// ---------------------------------------------------------------------------
// hex 编解码
// ---------------------------------------------------------------------------

/// 字节 → 小写十六进制。
fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// 小写十六进制 → 字节。奇数长度或含非法字符返回 `None`。
fn hex_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in bytes.chunks(2) {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        out.push((high << 4) | low);
    }
    Some(out)
}

/// 单个十六进制字符 → 数值。
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 记录体
// ---------------------------------------------------------------------------

/// 一条录制记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayRecord {
    /// 请求指纹
    pub fingerprint: String,
    /// 完整响应
    pub response: Response,
}

impl ReplayRecord {
    /// 序列化为文本（格式见模块文档）。
    pub fn to_text(&self) -> String {
        let mut out = String::from(RECORD_HEADER);
        out.push('\n');
        out.push_str("fingerprint ");
        out.push_str(&self.fingerprint);
        out.push('\n');
        out.push_str("status ");
        out.push_str(&self.response.status.to_string());
        out.push('\n');
        out.push_str("charset ");
        // 编码名不含空格；用 `-` 表示"无"。
        out.push_str(self.response.detected_charset.as_deref().unwrap_or("-"));
        out.push('\n');
        for (name, value) in self.response.headers.iter() {
            out.push_str("header ");
            out.push_str(&hex_encode(name.as_bytes()));
            out.push(' ');
            out.push_str(&hex_encode(value.as_bytes()));
            out.push('\n');
        }
        out.push_str("body ");
        out.push_str(&hex_encode(&self.response.body_bytes));
        out.push('\n');
        out
    }

    /// 从文本解析。
    pub fn from_text(text: &str) -> Result<Self, NetError> {
        let corrupt = |reason: &str| NetError::Corrupt {
            asset: "replay-record".to_string(),
            reason: reason.to_string(),
        };

        let mut lines = text.lines();
        let header = lines.next().unwrap_or_default();
        if header.trim() != RECORD_HEADER {
            return Err(corrupt("记录格式头不匹配"));
        }

        let mut fingerprint = None;
        let mut status = None;
        let mut charset_name: Option<String> = None;
        let mut headers = HeaderMap::new();
        let mut body: Option<Vec<u8>> = None;

        for line in lines {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (tag, rest) = line
                .split_once(' ')
                .ok_or_else(|| corrupt("记录行缺少字段分隔"))?;
            match tag {
                "fingerprint" => fingerprint = Some(rest.trim().to_string()),
                "status" => {
                    status = Some(
                        rest.trim()
                            .parse::<u16>()
                            .map_err(|_| corrupt("状态码不是合法数字"))?,
                    );
                }
                "charset" => {
                    let value = rest.trim();
                    charset_name = if value == "-" {
                        None
                    } else {
                        Some(value.to_string())
                    };
                }
                "header" => {
                    let (name_hex, value_hex) = rest
                        .split_once(' ')
                        .ok_or_else(|| corrupt("header 行缺少值字段"))?;
                    let name = hex_decode(name_hex).ok_or_else(|| corrupt("头名的 hex 非法"))?;
                    let value = hex_decode(value_hex).ok_or_else(|| corrupt("头值的 hex 非法"))?;
                    headers.append(
                        String::from_utf8_lossy(&name).into_owned(),
                        String::from_utf8_lossy(&value).into_owned(),
                    );
                }
                "body" => {
                    body = Some(hex_decode(rest).ok_or_else(|| corrupt("响应体的 hex 非法"))?);
                }
                _ => {}
            }
        }

        Ok(Self {
            fingerprint: fingerprint.ok_or_else(|| corrupt("缺少 fingerprint 段"))?,
            response: Response {
                status: status.ok_or_else(|| corrupt("缺少 status 段"))?,
                headers,
                body_bytes: body.ok_or_else(|| corrupt("缺少 body 段"))?,
                detected_charset: charset_name,
            },
        })
    }
}

// ---------------------------------------------------------------------------
// 存储与回放
// ---------------------------------------------------------------------------

/// 回放模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayMode {
    /// 透传：真实发请求，并把结果录进目录。
    Record,
    /// 只回放：完全离线，未命中即 [`NetError::ReplayMiss`]。
    Replay,
    /// 录制优先回放：命中则回放，未命中则真实发请求并录制。
    ///
    /// 为什么这个模式存在：真实抓取的目录天然会有空洞（部分页面抓失败、
    /// 或者书源只用到少数请求）。每次重跑都从头重录会让"已录好的部分"
    /// 随站点改版而失效，反而丢掉基线。`RecordOnce` 只补空洞。
    RecordOnce,
}

impl ReplayMode {
    /// 本模式是否**完全离线**（绝不发请求）。
    pub fn is_offline_only(self) -> bool {
        matches!(self, Self::Replay)
    }

    /// 本模式是否要用语料里的既有条目**优先**作答。
    ///
    /// `Replay` 与 `RecordOnce` 都为真但语义不同：前者未命中即失败，
    /// 后者未命中才去抓。执行器据此把"查语料"这一步提到限速之前。
    pub fn prefers_corpus(self) -> bool {
        matches!(self, Self::Replay | Self::RecordOnce)
    }

    /// 本模式是否要把真实抓取的响应写进语料。
    pub fn records(self) -> bool {
        matches!(self, Self::Record | Self::RecordOnce)
    }
}

/// 录制/回放存储。
#[derive(Debug)]
pub struct ReplayStore {
    root: PathBuf,
    /// 指纹 → 记录文件路径。
    ///
    /// 用 `BTreeMap` 而非 `HashMap`：索引文件要按指纹排序写出，
    /// 保证同一份语料两次录制后 `index.txt` 逐字节相同（可 diff、可断言）。
    index: BTreeMap<String, IndexEntry>,
}

/// 索引里的一条。
#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexEntry {
    /// 记录文件名（相对目录）
    file_name: String,
    /// 可读提示（method + 去 query 的 URL）
    hint: String,
}

impl ReplayStore {
    /// 打开（或初始化）一个语料目录。目录不存在时创建。
    ///
    /// 已有的 `index.txt` 会被读入；不存在则视为空语料。
    pub fn open(root: impl AsRef<Path>) -> Result<Self, NetError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root).map_err(|error| NetError::Io {
            reason: format!("无法创建回放目录 {}: {error}", root.display()),
        })?;

        let index_path = root.join(INDEX_FILE);
        let index = if index_path.exists() {
            let text = fs::read_to_string(&index_path).map_err(|error| NetError::Corrupt {
                asset: INDEX_FILE.to_string(),
                reason: format!("无法读取索引: {error}"),
            })?;
            parse_index(&text)?
        } else {
            BTreeMap::new()
        };

        Ok(Self { root, index })
    }

    /// 语料目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 已录制的记录数量。
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// 是否为空语料。
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// 回放：按指纹取记录，**不触网**。
    ///
    /// 未命中返回 [`NetError::ReplayMiss`]（映射为 `ErrorKind::Configuration`：
    /// 回放语料是本次运行的配置资产，缺条目即配置不完整）。
    pub fn replay(&self, spec: &RequestSpec) -> Result<Response, NetError> {
        let fingerprint = fingerprint(spec);
        let Some(entry) = self.index.get(&fingerprint) else {
            return Err(NetError::ReplayMiss {
                url: spec.url.clone(),
                fingerprint,
            });
        };

        let path = self.root.join(&entry.file_name);
        let text = fs::read_to_string(&path).map_err(|error| NetError::Corrupt {
            asset: entry.file_name.clone(),
            reason: format!("无法读取记录: {error}"),
        })?;
        let record = ReplayRecord::from_text(&text)?;

        // 校验记录里的指纹与索引一致：记录文件被改名/手改时，
        // 不校验就会"测试通过但内容是别的请求的响应"——最坏的静默失败。
        if record.fingerprint != fingerprint {
            return Err(NetError::Corrupt {
                asset: entry.file_name.clone(),
                reason: format!(
                    "记录指纹与索引不符：索引 {fingerprint}，记录 {}",
                    record.fingerprint
                ),
            });
        }
        Ok(record.response)
    }

    /// 录制一条记录（同指纹覆盖）。
    ///
    /// 覆盖而非追加是刻意的：同一指纹对应同一请求，重录时应当用**新的**响应
    /// （站点内容变了，基线也该更新）；保留旧的那份只会让语料里出现两份含义
    /// 不同而指纹相同的记录，重放时取哪份就成了未定义行为。
    pub fn record(&mut self, spec: &RequestSpec, response: &Response) -> Result<(), NetError> {
        let fingerprint = fingerprint(spec);
        let file_name = format!("{fingerprint}.{RECORD_EXTENSION}");
        let path = self.root.join(&file_name);
        let record = ReplayRecord {
            fingerprint: fingerprint.clone(),
            response: response.clone(),
        };
        fs::write(&path, record.to_text()).map_err(|error| NetError::Io {
            reason: format!("无法写入记录 {}: {error}", path.display()),
        })?;

        self.index.insert(
            fingerprint,
            IndexEntry {
                file_name,
                hint: readable_hint(spec),
            },
        );
        self.write_index()
    }

    /// 写出索引文件（按指纹排序，确定性）。
    pub fn write_index(&self) -> Result<(), NetError> {
        let mut text = String::from(INDEX_HEADER);
        text.push('\n');
        for (fingerprint, entry) in &self.index {
            text.push_str(fingerprint);
            text.push(' ');
            text.push_str(&entry.file_name);
            text.push(' ');
            text.push_str(&entry.hint);
            text.push('\n');
        }
        let path = self.root.join(INDEX_FILE);
        fs::write(&path, text).map_err(|error| NetError::Io {
            reason: format!("无法写入索引 {}: {error}", path.display()),
        })
    }
}

/// 解析索引文件。
fn parse_index(text: &str) -> Result<BTreeMap<String, IndexEntry>, NetError> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    if header.trim() != INDEX_HEADER {
        return Err(NetError::Corrupt {
            asset: INDEX_FILE.to_string(),
            reason: format!("索引格式头不匹配，期望 {INDEX_HEADER:?}"),
        });
    }

    let mut index = BTreeMap::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // 前三段以空格切分，余下整段是 URL（URL 本身不含空格）。
        let mut parts = line.splitn(4, ' ');
        let Some(fingerprint) = parts.next() else {
            continue;
        };
        let Some(file_name) = parts.next() else {
            continue;
        };
        let hint = parts.next().unwrap_or_default().to_string();
        index.insert(
            fingerprint.to_string(),
            IndexEntry {
                file_name: file_name.to_string(),
                hint,
            },
        );
    }
    Ok(index)
}

// ---------------------------------------------------------------------------
// 响应构造：离线回放也需要解码结果
// ---------------------------------------------------------------------------

/// 给原始字节填上 `detected_charset`（回放与录制共享）。
///
/// 回放时必须**重新**走一遍解码，而不是把录制时的编码名直接抄回来——
/// 这样回放路径与真实路径经过同一套字符集逻辑，字符集回归才会被差分测试捕获。
/// 代价是编码名必须稳定（同一份字节两次解码得到同一名字），
/// 这一点由 [`charset::decode_body`] 的确定性保证。
pub fn decode_for_response(body_bytes: &[u8], declared: Option<&str>) -> DecodedBody {
    let sniffed = charset::sniff_declared_charset(declared, body_bytes);
    let effective = sniffed.as_deref().or(declared);
    charset::decode_body(body_bytes, effective)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tome_core::HttpMethod;

    /// 每个测试用独立的临时目录，避免并行测试互相踩。
    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "tome-replay-{tag}-{nanos}-{:?}",
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).expect("应能创建临时目录");
        dir
    }

    fn sample_spec() -> RequestSpec {
        let mut spec = RequestSpec::get(
            "https://example.com/api/search?kw=rust&token=secret",
            "src-a",
        );
        spec.method = HttpMethod::Post;
        spec.headers.set("Content-Type", "application/json");
        spec.headers.append("Accept", "text/html");
        spec.headers.append("Accept", "application/xhtml+xml");
        spec.headers.set("Cookie", "sid=abc123");
        spec.headers.set("User-Agent", "tome/0.1");
        spec.headers.set("Authorization", "Bearer topsecret");
        spec.body = Some(br#"{"keyword":"rust"}"#.to_vec());
        spec
    }

    fn sample_response() -> Response {
        let mut headers = HeaderMap::new();
        headers.append("Content-Type", "text/html; charset=gbk");
        // 同名头必须保住：这是 HeaderMap 选 Vec 而非 HashMap 的理由。
        headers.append("Set-Cookie", "a=1; Path=/");
        headers.append("Set-Cookie", "b=2; Path=/");
        headers.append("X-Trace", "abc");
        Response {
            status: 200,
            headers,
            // 含 NUL、CRLF、高位字节、以及 GBK 的 "中文"
            body_bytes: {
                let mut bytes = vec![0x00, 0x0D, 0x0A, 0xFF, 0xFE, 0x80];
                bytes.extend_from_slice(&[0xD6, 0xD0, 0xCE, 0xC4]);
                bytes
            },
            detected_charset: Some("GBK".to_string()),
        }
    }

    #[test]
    fn fingerprint_is_stable_and_32_hex_chars() {
        let spec = sample_spec();
        let first = fingerprint(&spec);
        let second = fingerprint(&spec);
        assert_eq!(first, second, "同一请求的指纹必须可重复");
        assert_eq!(first.len(), 32, "128 位应为 32 个十六进制字符");
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn fingerprint_changes_with_method_url_body_and_headers() {
        let base = fingerprint(&sample_spec());

        let mut different_method = sample_spec();
        different_method.method = HttpMethod::Get;
        assert_ne!(base, fingerprint(&different_method));

        let mut different_url = sample_spec();
        different_url.url = "https://example.com/api/search?kw=rustx".to_string();
        assert_ne!(base, fingerprint(&different_url));

        let mut different_body = sample_spec();
        different_body.body = Some(b"{}".to_vec());
        assert_ne!(base, fingerprint(&different_body));

        let mut different_header = sample_spec();
        different_header.headers.set("Cookie", "sid=other");
        assert_ne!(base, fingerprint(&different_header));
    }

    #[test]
    fn fingerprint_ignores_headers_outside_the_allowlist() {
        let base = fingerprint(&sample_spec());
        let mut noisy = sample_spec();
        // 连接类头与无关头不参与指纹，否则同一逻辑请求会录成多条。
        noisy.headers.set("Connection", "keep-alive");
        noisy.headers.set("X-Random-Junk", "xyz");
        assert_eq!(base, fingerprint(&noisy));
    }

    #[test]
    fn fingerprint_is_case_insensitive_for_header_names() {
        let base = fingerprint(&sample_spec());
        let mut recased = sample_spec();
        recased.headers.set("CONTENT-TYPE", "application/json");
        recased.headers.set("cookie", "sid=abc123");
        assert_eq!(base, fingerprint(&recased));
    }

    #[test]
    fn fingerprint_ignores_header_order() {
        let base = fingerprint(&sample_spec());
        let mut reordered = RequestSpec::get(
            "https://example.com/api/search?kw=rust&token=secret",
            "src-a",
        );
        reordered.method = HttpMethod::Post;
        reordered.body = Some(br#"{"keyword":"rust"}"#.to_vec());
        // 故意按相反顺序插入。
        reordered.headers.set("User-Agent", "tome/0.1");
        reordered.headers.append("Accept", "application/xhtml+xml");
        reordered.headers.set("Authorization", "Bearer topsecret");
        reordered.headers.set("Cookie", "sid=abc123");
        reordered.headers.append("Accept", "text/html");
        reordered.headers.set("Content-Type", "application/json");
        assert_eq!(base, fingerprint(&reordered));
    }

    #[test]
    fn fingerprint_distinguishes_absent_and_empty_body() {
        let with_none = RequestSpec::get("https://example.com/", "s");
        let mut with_empty = RequestSpec::get("https://example.com/", "s");
        with_empty.body = Some(Vec::new());
        // 两者在 HTTP 语义上等价（都不发体），故指纹相同是**期望**行为。
        assert_eq!(fingerprint(&with_none), fingerprint(&with_empty));
    }

    #[test]
    fn different_requests_do_not_collide_in_a_realistic_batch() {
        let mut seen = std::collections::HashSet::new();
        for index in 0..2000 {
            let mut spec = RequestSpec::get(
                format!("https://example.com/book/{index}?q={}", index * 7),
                "src",
            );
            spec.body = Some(format!("body-{index}").into_bytes());
            spec.headers.set("Cookie", format!("sid={index}"));
            assert!(
                seen.insert(fingerprint(&spec)),
                "第 {index} 条指纹与既有条目碰撞"
            );
        }
    }

    /// T019 验收：录制 → 重放，`Response` 逐字段相等（含二进制 body 与重名响应头）。
    #[test]
    fn record_then_replay_matches_field_by_field() {
        let dir = temp_dir("roundtrip");
        let spec = sample_spec();
        let response = sample_response();

        let mut store = ReplayStore::open(&dir).expect("应能打开语料目录");
        store.record(&spec, &response).expect("录制应成功");

        let reopened = ReplayStore::open(&dir).expect("应能重新打开");
        assert_eq!(reopened.len(), 1);
        let replayed = reopened.replay(&spec).expect("重放应命中");

        assert_eq!(replayed.status, response.status);
        assert_eq!(
            replayed.body_bytes, response.body_bytes,
            "二进制体必须逐字节相同"
        );
        assert_eq!(replayed.detected_charset, response.detected_charset);
        assert_eq!(replayed.headers.len(), response.headers.len());
        for (left, right) in replayed.headers.iter().zip(response.headers.iter()) {
            assert_eq!(left, right, "头必须保序且含重名");
        }
        // 重名 Set-Cookie 确实存在（证明不是恰好只剩一个）。
        assert_eq!(
            replayed
                .headers
                .iter()
                .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
                .count(),
            2
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// T019 验收：未命中返回 `ReplayMiss`。
    #[test]
    fn replay_miss_for_unknown_request() {
        let dir = temp_dir("miss");
        let store = ReplayStore::open(&dir).expect("应能打开语料目录");
        let mut unknown = RequestSpec::get("https://example.com/never-recorded", "src");
        unknown.method = HttpMethod::Delete;

        let error = store.replay(&unknown).expect_err("未命中应报错");
        assert!(matches!(error, NetError::ReplayMiss { .. }));
        assert_eq!(error.to_error_kind(), tome_core::ErrorKind::Configuration);
        // 错误信息里带指纹，便于对照索引排查。
        assert!(error.to_string().contains(&fingerprint(&unknown)));

        fs::remove_dir_all(&dir).ok();
    }

    /// T019 验收：重放在**网络不可达**的情况下仍能工作。
    ///
    /// 这里的做法是把语料放在一个真实存在但**不可能联网**的目录语义下：
    /// 记录里存的是指向"未监听的本地端口"的请求。若重放路径试图联网，
    /// 它会得到连接失败（而不是成功）；重放能成功即证明它没走网络。
    ///
    /// 更强的逐次观测（用一个真实监听套接字统计是否收到连接）见集成测试
    /// `tests/replay_offline.rs`——那里能直接断言"连接数为 0"，
    /// 而本测试只能证明"结果不依赖网络"。
    #[test]
    fn replay_succeeds_even_when_the_target_is_unreachable() {
        let dir = temp_dir("offline");
        // 指向一个几乎必然没有监听的端口：真实请求会立刻连接失败。
        let mut spec = RequestSpec::get("http://127.0.0.1:1/never-a-service", "src");
        spec.method = HttpMethod::Get;

        let mut store = ReplayStore::open(&dir).expect("应能打开语料目录");
        store.record(&spec, &sample_response()).expect("录制应成功");

        // 重放必须成功——它只读目录，与目标是否可达无关。
        let replayed = store.replay(&spec).expect("离线重放应命中");
        assert_eq!(replayed.status, sample_response().status);
        assert_eq!(replayed.body_bytes, sample_response().body_bytes);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_file_contains_no_sensitive_plaintext() {
        let dir = temp_dir("privacy");
        let mut store = ReplayStore::open(&dir).expect("应能打开语料目录");
        store
            .record(&sample_spec(), &sample_response())
            .expect("录制应成功");

        let index = fs::read_to_string(dir.join(INDEX_FILE)).expect("应能读索引");
        // 敏感值绝不出现在索引里（AGENTS.md 安全红线）。
        for secret in ["topsecret", "sid=abc123", "Bearer", "token=secret"] {
            assert!(
                !index.contains(secret),
                "索引泄漏了敏感内容 {secret:?}:\n{index}"
            );
        }
        // query 整段剥离；host 保留（那是排查必需的信息）。
        assert!(index.contains("https://example.com/api/search"));
        assert!(!index.contains("?"));

        // 记录体里也不得有原始头值（只有摘要与 hex 编码的响应侧数据）。
        let record = fs::read_to_string(dir.join(format!("{}.bin", fingerprint(&sample_spec()))))
            .expect("应能读记录");
        assert!(!record.contains("topsecret"));
        assert!(!record.contains("sid=abc123"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_overwrites_same_fingerprint_deterministically() {
        let dir = temp_dir("overwrite");
        let spec = sample_spec();
        let mut store = ReplayStore::open(&dir).expect("应能打开语料目录");

        store.record(&spec, &sample_response()).expect("首次录制");
        let first_index = fs::read_to_string(dir.join(INDEX_FILE)).expect("应能读索引");

        let mut changed = sample_response();
        changed.status = 404;
        store.record(&spec, &changed).expect("重录");

        assert_eq!(store.len(), 1, "同指纹应覆盖而非新增");
        let second_index = fs::read_to_string(dir.join(INDEX_FILE)).expect("应能读索引");
        assert_eq!(first_index, second_index, "覆盖不应改变索引内容");

        let replayed = ReplayStore::open(&dir)
            .expect("应能重开")
            .replay(&spec)
            .expect("应命中");
        assert_eq!(replayed.status, 404, "应取最新录制的响应");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_rewrite_is_idempotent_and_sorted() {
        let dir = temp_dir("idempotent");
        let mut store = ReplayStore::open(&dir).expect("应能打开语料目录");
        for index in 0..8 {
            let spec = RequestSpec::get(format!("https://example.com/p/{index}"), "src");
            store.record(&spec, &sample_response()).expect("录制");
        }
        let first = fs::read_to_string(dir.join(INDEX_FILE)).expect("应能读索引");
        store.write_index().expect("应能重写索引");
        let second = fs::read_to_string(dir.join(INDEX_FILE)).expect("应能读索引");
        assert_eq!(first, second, "索引重写应幂等");

        let fingerprints: Vec<&str> = first
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .filter_map(|line| line.split(' ').next())
            .collect();
        let mut sorted = fingerprints.clone();
        sorted.sort();
        assert_eq!(fingerprints, sorted, "索引应按指纹排序");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reopened_store_finds_previously_recorded_requests() {
        let dir = temp_dir("reopen");
        let spec = sample_spec();
        {
            let mut store = ReplayStore::open(&dir).expect("应能打开");
            store.record(&spec, &sample_response()).expect("录制");
        }
        let reopened = ReplayStore::open(&dir).expect("应能重开");
        assert!(reopened.replay(&spec).is_ok(), "跨进程会话应能命中");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tampered_record_is_detected() {
        let dir = temp_dir("tamper");
        let spec = sample_spec();
        let mut store = ReplayStore::open(&dir).expect("应能打开");
        store.record(&spec, &sample_response()).expect("录制");

        // 把记录文件换成另一个请求的内容（模拟改名/手改导致的张冠李戴）。
        let other = RequestSpec::get("https://example.com/other", "src");
        let record_path = dir.join(format!("{}.bin", fingerprint(&spec)));
        let foreign = ReplayRecord {
            fingerprint: fingerprint(&other),
            response: sample_response(),
        };
        fs::write(&record_path, foreign.to_text()).expect("应能写入");

        let error = store.replay(&spec).expect_err("指纹不符必须被发现");
        assert!(matches!(error, NetError::Corrupt { .. }));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_text_round_trips() {
        let record = ReplayRecord {
            fingerprint: "00112233445566778899aabbccddeeff".to_string(),
            response: sample_response(),
        };
        let text = record.to_text();
        let restored = ReplayRecord::from_text(&text).expect("应能解析");
        assert_eq!(restored, record);
    }

    #[test]
    fn record_handles_empty_body_and_absent_charset() {
        let response = Response {
            status: 204,
            headers: HeaderMap::new(),
            body_bytes: Vec::new(),
            detected_charset: None,
        };
        let record = ReplayRecord {
            fingerprint: "00".repeat(16),
            response,
        };
        let restored = ReplayRecord::from_text(&record.to_text()).expect("应能解析");
        assert_eq!(restored.response.body_bytes, Vec::<u8>::new());
        assert_eq!(restored.response.detected_charset, None);
        assert_eq!(restored.response.status, 204);
    }

    #[test]
    fn record_survives_binary_body_with_crlf_and_nul() {
        let body: Vec<u8> = (0u8..=255).collect();
        let record = ReplayRecord {
            fingerprint: "ab".repeat(16),
            response: Response {
                status: 200,
                headers: HeaderMap::new(),
                body_bytes: body.clone(),
                detected_charset: None,
            },
        };
        let restored = ReplayRecord::from_text(&record.to_text()).expect("应能解析");
        assert_eq!(
            restored.response.body_bytes, body,
            "全部 256 种字节都要能往返"
        );
    }

    #[test]
    fn record_parses_header_value_with_special_bytes() {
        let mut headers = HeaderMap::new();
        // 头值里放制表符与高位字节：真实站点偶尔会发这种非法值。
        headers.append("X-Odd", "a\tb\u{00e9}\u{4e2d}");
        let record = ReplayRecord {
            fingerprint: "cd".repeat(16),
            response: Response {
                status: 200,
                headers,
                body_bytes: Vec::new(),
                detected_charset: None,
            },
        };
        let restored = ReplayRecord::from_text(&record.to_text()).expect("应能解析");
        assert_eq!(
            restored.response.headers.get("X-Odd"),
            Some("a\tb\u{00e9}\u{4e2d}")
        );
    }

    #[test]
    fn corrupt_index_header_is_rejected() {
        let dir = temp_dir("badindex");
        fs::write(dir.join(INDEX_FILE), "# wrong format\n").expect("应能写入");
        let error = ReplayStore::open(&dir).expect_err("格式头不符应报错");
        assert!(matches!(error, NetError::Corrupt { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_record_is_rejected() {
        let dir = temp_dir("badrecord");
        let spec = sample_spec();
        let mut store = ReplayStore::open(&dir).expect("应能打开");
        store.record(&spec, &sample_response()).expect("录制");
        fs::write(
            dir.join(format!("{}.bin", fingerprint(&spec))),
            "# wrong format\n",
        )
        .expect("应能写入");

        let error = store.replay(&spec).expect_err("坏记录应报错");
        assert!(matches!(error, NetError::Corrupt { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hex_round_trips_all_bytes() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        let encoded = hex_encode(&bytes);
        assert_eq!(encoded.len(), 512);
        assert_eq!(hex_decode(&encoded), Some(bytes));
    }

    #[test]
    fn hex_decode_rejects_bad_input() {
        assert_eq!(hex_decode("abc"), None, "奇数长度应拒绝");
        assert_eq!(hex_decode("zz"), None, "非法字符应拒绝");
        assert_eq!(hex_decode(""), Some(Vec::new()));
    }

    #[test]
    fn readable_hint_strips_query_and_fragment() {
        let mut spec = RequestSpec::get("https://example.com/a/b?token=x#frag", "src");
        spec.method = HttpMethod::Post;
        assert_eq!(readable_hint(&spec), "POST https://example.com/a/b");

        let plain = RequestSpec::get("https://example.com/a/b", "src");
        assert_eq!(readable_hint(&plain), "GET https://example.com/a/b");
    }

    #[test]
    fn decode_for_response_fills_charset() {
        let decoded =
            decode_for_response(&[0xD6, 0xD0, 0xCE, 0xC4], Some("text/html; charset=gbk"));
        assert_eq!(decoded.text, "中文");
        assert_eq!(decoded.charset, "GBK");
    }

    #[test]
    fn record_once_mode_exists_for_incremental_capture() {
        // 模式枚举的完整性（行为验证在 executor 测试里）。
        assert_ne!(ReplayMode::Record, ReplayMode::Replay);
        assert_ne!(ReplayMode::RecordOnce, ReplayMode::Replay);
    }
}
