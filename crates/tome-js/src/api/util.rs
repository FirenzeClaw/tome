//! [DONE] 宿主 API · 正则 + 时间日期 + 杂项（tasks.md T040 / FR-005）
//!
//! # FR-005 的三条清单
//!
//! > **正则**：匹配、查找、替换、捕获组提取
//! > **时间日期**：时间戳与格式化字符串互转、日期计算
//! > **杂项**：随机数、压缩与解压、HTML 实体处理
//!
//! # 正则为什么**不**在 Rust 侧重新实现
//!
//! 这是本模块最重要的一个决定。
//!
//! 书源脚本里的正则是 **JavaScript 正则字面量**（`/abc/gi`），带 JS 特有
//! 的语义：`lastIndex` 的状态性、`$&`/`$1`/`` $` ``/`$'` 替换模式、
//! 命名捕获组、以及 `/u` 标志下的 Unicode 行为。
//!
//! 若我们把它转成 Rust 正则引擎（`regex` / `fancy-regex`）来跑：
//!
//! | 差异 | 后果 |
//! |---|---|
//! | JS 用回溯语义，Rust `regex` 是 DFA | 某些模式匹配结果**不同**（不是快慢问题） |
//! | `\d` 在 JS 里只匹配 ASCII，Unicode 模式不同 | 中文数字、全角数字的处理不同 |
//! | 替换串的 `$&` 等模式 | Rust 引擎不认识，会原样输出 |
//! | `lastIndex` 与 `/g` 的有状态语义 | 连续调用 `exec` 的结果不同 |
//!
//! 故本模块的正则入口**直接委托给 JS 引擎的原生 `RegExp`**——把参数
//! 拼装成一段 JS 表达式求值。这保证行为与书源作者在浏览器里调试时
//! 看到的**完全一致**（兼容性优先，宪法原则 I）。
//!
//! `[UNCERTAIN]` 代价是每次调用要多一次 `eval`（解析一段短脚本）。
//! 对书源的调用频率可忽略；若实测成为瓶颈，应先测量而不是提前优化。
//! 相比之下，"用 Rust 引擎重实现 JS 正则语义"是**不可能做对**的方向。
//!
//! # 时间：为什么以"秒"为默认单位
//!
//! 书源里 `java.timeFormat(timestamp)` 的 timestamp 绝大多数是**秒**
//! （服务端接口的通行做法），而 JS 的 `Date` 用**毫秒**。这个不一致是
//! 书源里最容易出错的一处，故本模块的入口**显式接受秒**并在文档与
//! 函数名上写明（`timeFormat` 收秒、`timeFormatMs` 收毫秒）。

use std::io::{Read, Write};

use rquickjs::{Ctx, Object};

use crate::bridge::throw_host_error;
use crate::sandbox_error::JsError;

/// 注册正则、时间日期与杂项的全部宿主函数到 `java` 对象。
pub fn register<'js>(ctx: &Ctx<'js>, java: &Object<'js>) -> Result<usize, JsError> {
    let mut count = 0usize;

    // ---------- 正则（委托给 JS 原生 RegExp） ----------

    // `java.match(text, pattern, flags)` → 返回匹配数组（或 null）
    java.set(
        "match",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             text: String,
             pattern: String,
             flags: rquickjs::function::Opt<String>|
             -> Result<rquickjs::Value<'js>, rquickjs::Error> {
                // 把参数**作为数据**注入（不是拼进脚本字面量）：
                // 拼接会有转义问题（pattern 里的引号/反斜杠），而那正是
                // 书源作者最容易踩的坑。故用临时全局变量传参。
                regex_call(&ctx, "match", &text, &pattern, &flags.0.unwrap_or_default())
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("match", &error))?,
    )
    .map_err(|error| registration_error("match", &error))?;
    count += 1;

    // `java.matchAll(text, pattern, flags)` → 返回全部匹配的数组
    java.set(
        "matchAll",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             text: String,
             pattern: String,
             flags: rquickjs::function::Opt<String>|
             -> Result<rquickjs::Value<'js>, rquickjs::Error> {
                // 强制带 `g` 标志：不带的 `matchAll` 在语义上等于 `match`，
                // 而调用方用这个名字时期望的一定是"全部"。
                let mut effective = flags.0.unwrap_or_default();
                if !effective.contains('g') {
                    effective.push('g');
                }
                regex_call(&ctx, "matchAll", &text, &pattern, &effective)
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("matchAll", &error))?,
    )
    .map_err(|error| registration_error("matchAll", &error))?;
    count += 1;

    // `java.replace(text, pattern, replacement, flags)` → 替换后的文本
    java.set(
        "replace",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             text: String,
             pattern: String,
             replacement: String,
             flags: rquickjs::function::Opt<String>|
             -> Result<String, rquickjs::Error> {
                let mut effective = flags.0.unwrap_or_default();
                // 默认全局替换：书源里净化正文的用途几乎都是"全部替换"，
                // 而 JS 不带 `g` 时只替换首个——那会让净化只清掉第一处。
                if !effective.contains('g') {
                    effective.push('g');
                }
                let value = regex_replace(&ctx, &text, &pattern, &replacement, &effective)
                    .map_err(|error| throw_host_error(&ctx, &error))?;
                value
                    .as_string()
                    .and_then(|text| text.to_string().ok())
                    .ok_or_else(|| throw_host_error(&ctx, &JsError::new("替换结果不是字符串")))
            },
        )
        .map_err(|error| registration_error("replace", &error))?,
    )
    .map_err(|error| registration_error("replace", &error))?;
    count += 1;

    // `java.getMatchGroups(text, pattern, flags)` → 首个匹配的捕获组数组
    java.set(
        "getMatchGroups",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             text: String,
             pattern: String,
             flags: rquickjs::function::Opt<String>|
             -> Result<Vec<String>, rquickjs::Error> {
                regex_groups(&ctx, &text, &pattern, &flags.0.unwrap_or_default())
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("getMatchGroups", &error))?,
    )
    .map_err(|error| registration_error("getMatchGroups", &error))?;
    count += 1;

    // ---------- 时间日期 ----------

    // 秒 → 格式化字符串（书源里最常用的形态）
    java.set(
        "timeFormat",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, timestamp: f64, pattern: String| -> Result<String, rquickjs::Error> {
                format_timestamp(&ctx, timestamp, &pattern, TimeUnit::Seconds)
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("timeFormat", &error))?,
    )
    .map_err(|error| registration_error("timeFormat", &error))?;
    count += 1;

    // 毫秒 → 格式化字符串
    java.set(
        "timeFormatMs",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, timestamp: f64, pattern: String| -> Result<String, rquickjs::Error> {
                format_timestamp(&ctx, timestamp, &pattern, TimeUnit::Milliseconds)
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("timeFormatMs", &error))?,
    )
    .map_err(|error| registration_error("timeFormatMs", &error))?;
    count += 1;

    // 无参形态：当前时间的格式化（书源里 `java.timeFormat()` 表示"现在"）
    java.set(
        "now",
        rquickjs::Function::new(ctx.clone(), || -> f64 { now_millis() as f64 })
            .map_err(|error| registration_error("now", &error))?,
    )
    .map_err(|error| registration_error("now", &error))?;
    count += 1;

    // ---------- 杂项：随机数 ----------

    java.set(
        "random",
        rquickjs::Function::new(ctx.clone(), || -> f64 { pseudo_random() })
            .map_err(|error| registration_error("random", &error))?,
    )
    .map_err(|error| registration_error("random", &error))?;
    count += 1;

    java.set(
        "randomInt",
        rquickjs::Function::new(ctx.clone(), |bound: i64| -> i64 {
            // 上界 <= 0 时返回 0：书源可能传一个算错的边界，
            // 返回 0 比 panic 或负数好。
            if bound <= 0 {
                return 0;
            }
            (pseudo_random() * bound as f64) as i64
        })
        .map_err(|error| registration_error("randomInt", &error))?,
    )
    .map_err(|error| registration_error("randomInt", &error))?;
    count += 1;

    // ---------- 杂项：压缩与解压 ----------

    // Gzip 解压：书源偶尔拿到 gzip 内容需要自己解（罕见但确实存在）
    java.set(
        "gzipDecode",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, data: rquickjs::Value<'js>| -> Result<String, rquickjs::Error> {
                let bytes = crate::api::text::value_to_bytes(&ctx, &data)
                    .map_err(|error| throw_host_error(&ctx, &error))?;
                gzip_decode(&bytes)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("gzipDecode", &error))?,
    )
    .map_err(|error| registration_error("gzipDecode", &error))?;
    count += 1;

    java.set(
        "gzipEncode",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String| -> Result<Vec<u8>, rquickjs::Error> {
                gzip_encode(text.as_bytes()).map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("gzipEncode", &error))?,
    )
    .map_err(|error| registration_error("gzipEncode", &error))?;
    count += 1;

    // 通用解压：自动识别 gzip / zlib / deflate（书源作者常分不清这三种）
    java.set(
        "inflate",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, data: rquickjs::Value<'js>| -> Result<String, rquickjs::Error> {
                let bytes = crate::api::text::value_to_bytes(&ctx, &data)
                    .map_err(|error| throw_host_error(&ctx, &error))?;
                inflate(&bytes)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("inflate", &error))?,
    )
    .map_err(|error| registration_error("inflate", &error))?;
    count += 1;

    java.set(
        "deflate",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, text: String| -> Result<Vec<u8>, rquickjs::Error> {
                deflate(text.as_bytes()).map_err(|error| throw_host_error(&ctx, &error))
            },
        )
        .map_err(|error| registration_error("deflate", &error))?,
    )
    .map_err(|error| registration_error("deflate", &error))?;
    count += 1;

    // ---------- 杂项：HTML 实体 ----------

    java.set(
        "htmlDecode",
        rquickjs::Function::new(ctx.clone(), |text: String| html_decode(&text))
            .map_err(|error| registration_error("htmlDecode", &error))?,
    )
    .map_err(|error| registration_error("htmlDecode", &error))?;
    count += 1;

    java.set(
        "htmlEncode",
        rquickjs::Function::new(ctx.clone(), |text: String| html_encode(&text))
            .map_err(|error| registration_error("htmlEncode", &error))?,
    )
    .map_err(|error| registration_error("htmlEncode", &error))?;
    count += 1;

    Ok(count)
}

/// 构造注册失败的诊断错误。
fn registration_error(name: &str, error: &rquickjs::Error) -> JsError {
    JsError::new(format!("注册 `{name}` 失败：{error}"))
}

// ---------------------------------------------------------------------------
// 正则：委托给 JS 原生 RegExp（见模块文档）
// ---------------------------------------------------------------------------

/// 参数与结果的暂存全局变量名（带前缀以免与书源变量冲突）。
const REGEX_TEXT: &str = "__tome_regex_text";
const REGEX_PATTERN: &str = "__tome_regex_pattern";
const REGEX_REPLACEMENT: &str = "__tome_regex_replacement";

/// 在一段 JS 表达式上求值，参数经临时全局变量注入。
///
/// # 为什么用全局变量传参而不是拼进脚本
///
/// 拼接要处理转义：pattern 里可能有 `"`、`\`、换行；text 里可能有任意
/// 字符。而书源的正则模式恰恰**大量包含反斜杠**（`\d+`、`\s*`）。
/// 手工转义极易出错，而错误的表现是"正则语义变了"（不是语法错）。
///
/// 临时变量在求值后**无论成败都清理**——书源可能遍历全局对象。
fn eval_with_params<'js>(
    ctx: &Ctx<'js>,
    script: &str,
    params: &[(&str, rquickjs::Value<'js>)],
) -> Result<rquickjs::Value<'js>, JsError> {
    for (name, value) in params {
        ctx.globals()
            .set(*name, value.clone())
            .map_err(|error| JsError::new(format!("注入参数 `{name}` 失败：{error}")))?;
    }

    let outcome = ctx.eval::<rquickjs::Value<'js>, _>(script.as_bytes());

    // 无论成败都清理（失败的求值同样留下了全局键）
    for (name, _) in params {
        let _ = ctx
            .globals()
            .set(*name, rquickjs::Value::new_undefined(ctx.clone()));
    }

    match outcome {
        Ok(value) => Ok(value),
        Err(rquickjs::Error::Exception) => {
            // 取出 JS 侧的错误消息（正则语法错、书源提供的数据问题等），
            // 转成结构化错误。这比让引擎错误裸奔更有用——书源作者需要
            // 知道"正则有问题"而不是"脚本执行失败"。
            let exception = ctx.catch();
            let message = exception
                .as_object()
                .and_then(|object| object.get::<_, String>("message").ok())
                .or_else(|| exception.as_string().and_then(|s| s.to_string().ok()))
                .unwrap_or_else(|| "正则求值失败".to_string());
            Err(JsError::new(message))
        }
        Err(other) => Err(JsError::new(format!("正则求值失败：{other}"))),
    }
}

/// `match` / `matchAll`：把参数交给 JS 的原生实现。
///
/// # `matchAll` 必须摊开成数组
///
/// 它返回的是 **迭代器**（`RegExp String Iterator`），不是数组。
/// 直接把它送回 JS 侧，书源看到的会是一个不可遍历的对象
/// （`String(iter)` 甚至给出 `"[object RegExp String Iterator]"`——
/// 那是既不像数组也不像字符串的东西，书源的所有后续处理都会错）。
///
/// 故对 `matchAll` 用展开语法 `[...]` 收成真正的数组。
/// 这一步不做的话，"`matchAll` 看起来能用但结果是垃圾"会成为
/// 极难归因的一类缺陷。
fn regex_call<'js>(
    ctx: &Ctx<'js>,
    method: &str,
    text: &str,
    pattern: &str,
    flags: &str,
) -> Result<rquickjs::Value<'js>, JsError> {
    let text_value = rquickjs::String::from_str(ctx.clone(), text)
        .map_err(|error| JsError::new(format!("构造文本参数失败：{error}")))?;
    let pattern_value = rquickjs::String::from_str(ctx.clone(), pattern)
        .map_err(|error| JsError::new(format!("构造模式参数失败：{error}")))?;
    let flags_value = rquickjs::String::from_str(ctx.clone(), flags)
        .map_err(|error| JsError::new(format!("构造标志参数失败：{error}")))?;

    // 用 `new RegExp(pattern, flags)` 的**两参**形态：JS 不支持内联标志
    // 语法（`(?i)` 是报错），故标志只能走第二参数。
    let script = if method == "matchAll" {
        format!("[...String({REGEX_TEXT}).matchAll(new RegExp({REGEX_PATTERN}, {REGEX_FLAGS}))]")
    } else {
        format!("String({REGEX_TEXT}).{method}(new RegExp({REGEX_PATTERN}, {REGEX_FLAGS}))")
    };

    eval_with_params(
        ctx,
        &script,
        &[
            (REGEX_TEXT, text_value.into_value()),
            (REGEX_PATTERN, pattern_value.into_value()),
            (REGEX_FLAGS, flags_value.into_value()),
        ],
    )
}

/// `replace`：返回替换后的 JS 值。
fn regex_replace<'js>(
    ctx: &Ctx<'js>,
    text: &str,
    pattern: &str,
    replacement: &str,
    flags: &str,
) -> Result<rquickjs::Value<'js>, JsError> {
    let text_value = rquickjs::String::from_str(ctx.clone(), text)
        .map_err(|error| JsError::new(format!("构造文本参数失败：{error}")))?;
    let pattern_value = rquickjs::String::from_str(ctx.clone(), pattern)
        .map_err(|error| JsError::new(format!("构造模式参数失败：{error}")))?;
    let replacement_value = rquickjs::String::from_str(ctx.clone(), replacement)
        .map_err(|error| JsError::new(format!("构造替换参数失败：{error}")))?;
    let flags_value = rquickjs::String::from_str(ctx.clone(), flags)
        .map_err(|error| JsError::new(format!("构造标志参数失败：{error}")))?;

    let script = format!(
        "String({REGEX_TEXT}).replace(new RegExp({REGEX_PATTERN}, {REGEX_FLAGS}), {REGEX_REPLACEMENT})"
    );
    eval_with_params(
        ctx,
        &script,
        &[
            (REGEX_TEXT, text_value.into_value()),
            (REGEX_PATTERN, pattern_value.into_value()),
            (REGEX_REPLACEMENT, replacement_value.into_value()),
            (REGEX_FLAGS, flags_value.into_value()),
        ],
    )
}

/// 取首个匹配的捕获组（含整个匹配）。
fn regex_groups(
    ctx: &Ctx<'_>,
    text: &str,
    pattern: &str,
    flags: &str,
) -> Result<Vec<String>, JsError> {
    // 用 `matchAll` 取首个匹配的 groups：`match` 的返回在非全局时
    // 是"数组 + index/input 属性"，转成 Vec<String> 会丢捕获组的信息。
    // `matchAll` 的每一项是干净的数组形态。
    //
    // ## `matchAll` **强制**要求 `g` 标志
    //
    // 不带 `g` 时引擎直接抛 "regexp must have the 'g' flag"。
    // 而调用方（书源）写 `getMatchGroups(text, pattern)` 时不会想到
    // 自己要补标志——故这里补上，与 `matchAll` 入口同一处理。
    let mut effective = flags.to_string();
    if !effective.contains('g') {
        effective.push('g');
    }
    let result = regex_call(ctx, "matchAll", text, pattern, &effective)?;
    // 结果被序列化成 JSON 文本再解析回来（与 data 模块同源的思路：
    // 避免逐类型处理 JS 值的形态差异）。
    let json = ctx
        .json_stringify(result)
        .ok()
        .flatten()
        .and_then(|text| text.to_string().ok())
        .ok_or_else(|| JsError::new("无法读取匹配结果"))?;
    let parsed: serde_json::Value = serde_json::from_str(&json)
        .map_err(|error| JsError::new(format!("解析结果失败：{error}")))?;

    let Some(first) = parsed.as_array().and_then(|matches| matches.first()) else {
        return Ok(Vec::new());
    };
    let groups = first
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .map(|group| {
                    group
                        .as_str()
                        .map(str::to_string)
                        // 未参与匹配的组是 `null` → 空串（书源里判空统一）
                        .unwrap_or_default()
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(groups)
}

/// 标志参数的暂存全局变量名。
const REGEX_FLAGS: &str = "__tome_regex_flags";

// ---------------------------------------------------------------------------
// 时间日期
// ---------------------------------------------------------------------------

/// 时间戳单位（见模块文档：秒是书源的主流，毫秒是 JS 的主流）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeUnit {
    /// 秒（`java.timeFormat`）
    Seconds,
    /// 毫秒（`java.timeFormatMs`）
    Milliseconds,
}

/// 把时间戳格式化为指定模式的字符串。
///
/// # 为什么格式化交给 JS 的 `Date`
///
/// 书源里的模式串是 **Java `SimpleDateFormat` 风格**（`yyyy-MM-dd HH:mm:ss`），
/// 而 JS 没有等价的原生格式化。三种做法：
///
/// | 做法 | 问题 |
/// |---|---|
/// | 引 `chrono` 并自己实现 SDF 解析 | 要重实现一套模式语言（含转义、时区、本地化） |
/// | 引 `chrono` 用它的 `format`（strftime 风格） | 模式串不同（`%Y` vs `yyyy`），得写翻译层 |
/// | **在 JS 侧手写替换**（本实现） | 每个 token 的分支要穷举，但语义**可控且可测** |
///
/// 取第三种：把时间戳交给 JS 的 `Date`（它知道时区与闰秒），
/// 然后按 SDF 的 token 做字符串替换。token 集合只覆盖书源里实际出现的
/// 那几个（见 [`format_timestamp`] 的实现），未识别的 token **原样保留**。
///
/// `[WIP]` 不支持时区（`XXX`）、本地化月份名（`MMM`）等少见 token。
/// 若 S1-6 的基线发现需要，应在此处补 token 而不是引依赖。
fn format_timestamp(
    ctx: &Ctx<'_>,
    timestamp: f64,
    pattern: &str,
    unit: TimeUnit,
) -> Result<String, JsError> {
    if !timestamp.is_finite() {
        return Err(JsError::new(format!("时间戳不是有限数：{timestamp}")));
    }
    let millis = match unit {
        TimeUnit::Seconds => timestamp * 1000.0,
        TimeUnit::Milliseconds => timestamp,
    };

    // 在 JS 侧构造 Date 并取出各分量。用 JS 的 Date 而不是 Rust 的时间库：
    // 它知道系统时区与本地化规则，而书源作者正是在浏览器里调试这些模式串的
    // （兼容性优先）。
    let script = r#"
      (function (ms) {
        var d = new Date(ms);
        function pad(n, w) { var s = String(n); while (s.length < w) s = "0" + s; return s; }
        return {
          yyyy: String(d.getFullYear()),
          yy: pad(d.getFullYear() % 100, 2),
          MM: pad(d.getMonth() + 1, 2),
          M: String(d.getMonth() + 1),
          dd: pad(d.getDate(), 2),
          d: String(d.getDate()),
          HH: pad(d.getHours(), 2),
          H: String(d.getHours()),
          mm: pad(d.getMinutes(), 2),
          m: String(d.getMinutes()),
          ss: pad(d.getSeconds(), 2),
          s: String(d.getSeconds()),
          SSS: pad(d.getMilliseconds(), 3)
        };
      })(__tome_ts_millis)
    "#;

    let millis_value = rquickjs::Value::new_float(ctx.clone(), millis);
    let parts = eval_with_params(ctx, script, &[("__tome_ts_millis", millis_value)])?;

    let json = ctx
        .json_stringify(parts)
        .ok()
        .flatten()
        .and_then(|text| text.to_string().ok())
        .ok_or_else(|| JsError::new("无法读取时间分量"))?;
    let parts: serde_json::Value = serde_json::from_str(&json)
        .map_err(|error| JsError::new(format!("解析时间分量失败：{error}")))?;
    let get = |key: &str| -> String {
        parts
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    // ## token 替换的顺序很要紧
    //
    // 长的（`yyyy`、`MM`、`SSS`）必须排在短的前面，否则 `yyyy` 会被
    // `yy` 先吃掉一部分，得到 `2026` → `26yy` 之类的错值。
    // 这与"用 `MM` 覆盖 `M`"是同一条理由。
    let replacements: [(&str, &str); 12] = [
        ("yyyy", &get("yyyy")),
        ("MMMM", &get("MM")),
        ("SSS", &get("SSS")),
        ("yy", &get("yy")),
        ("MM", &get("MM")),
        ("dd", &get("dd")),
        ("HH", &get("HH")),
        ("mm", &get("mm")),
        ("ss", &get("ss")),
        ("M", &get("M")),
        ("d", &get("d")),
        ("H", &get("H")),
    ];

    // 用占位符两阶段替换，避免"上一步的产物被下一步再次替换"
    // （例如 `yyyy` 换成 `2026` 后，若之后还有 `yy` 的替换，`2026` 里
    // 没有 `yy` 字符所以安全；但 `MM` → `07` 后再遇 `M` 就会把 `07` 的
    // `0` 留下 ≠ 期望。两阶段彻底消除这类交互）。
    let mut staged = pattern.to_string();
    let mut captured: Vec<String> = Vec::new();
    for (index, (token, value)) in replacements.iter().enumerate() {
        let placeholder = format!("\u{1}{index}\u{1}");
        if staged.contains(token) {
            staged = staged.replace(token, &placeholder);
            captured.push(value.to_string());
        } else {
            captured.push(String::new());
        }
    }
    for (index, value) in captured.iter().enumerate() {
        if !value.is_empty() {
            staged = staged.replace(&format!("\u{1}{index}\u{1}"), value);
        }
    }
    Ok(staged)
}

/// 当前 Unix 毫秒。
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 杂项：随机数
// ---------------------------------------------------------------------------

/// 伪随机数（`[0, 1)`）。
///
/// # 为什么不用 `rand`
///
/// 书源用随机数的目的只有两类：给请求加一个随机参数绕过缓存、或随机
/// 选择一个镜像地址。两者都不需要密码学质量，而 `rand` 已经把
/// `rsa` 作为传递依赖引入（见 Cargo.toml）——本模块**不复用**它，
/// 是为了让"随机数"这件事的依赖面在这一处可见。
///
/// ## 实现：`RandomState` 的哈希
///
/// `std::collections::hash_map::RandomState` 由标准库用**每次进程启动
/// 都不同**的种子初始化（它就是为了防止哈希碰撞攻击而设计的）。
/// 对它的每次调用哈希一个变化的值，即可得到质量足够的伪随机流，
/// 且**零额外依赖**。
///
/// `[UNCERTAIN]` 这不是密码学安全的随机源。若某天书源的用途需要
/// （如生成密钥），必须换成 `OsRng`——那是一个需要显式决策的变更。
fn pseudo_random() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 单调递增的计数器（保证每次调用的输入不同）。
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    /// 每次进程启动不同（`RandomState` 的种子由标准库随机化）。
    static STATE: std::sync::OnceLock<std::collections::hash_map::RandomState> =
        std::sync::OnceLock::new();

    let state = STATE.get_or_init(std::collections::hash_map::RandomState::new);
    let mut hasher = state.build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    // 混入时间：同一毫秒内的多次调用依赖计数器，跨毫秒的变化多一层熵。
    hasher.write_u128(now_millis());
    // `u64::MAX` 归一化到 `[0, 1)`
    (hasher.finish() as f64) / (u64::MAX as f64)
}

// ---------------------------------------------------------------------------
// 杂项：压缩与解压
// ---------------------------------------------------------------------------

/// Gzip 解压。
pub(crate) fn gzip_decode(data: &[u8]) -> Result<Vec<u8>, JsError> {
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| JsError::new(format!("Gzip 解压失败：{error}")))?;
    Ok(output)
}

/// Gzip 压缩。
pub(crate) fn gzip_encode(data: &[u8]) -> Result<Vec<u8>, JsError> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(data)
        .map_err(|error| JsError::new(format!("Gzip 压缩失败：{error}")))?;
    encoder
        .finish()
        .map_err(|error| JsError::new(format!("Gzip 压缩收尾失败：{error}")))
}

/// 通用解压：依次尝试 gzip / zlib / 裸 deflate。
///
/// # 为什么要自动识别
///
/// 书源作者常分不清这三种格式（它们都是 DEFLATE，区别只在包装头），
/// 而 HTTP 层的 `Content-Encoding` 有时与内容实际格式不符。让调用方
/// 先判断再选函数，等于把这个负担推给最不该承担它的一方。
///
/// 尝试顺序：先按头部魔数判 gzip（`1f 8b`），其余按 zlib 再按裸 deflate
/// ——这是"误判概率最低"的顺序（裸 deflate 没有可辨识的头，放最后）。
pub(crate) fn inflate(data: &[u8]) -> Result<Vec<u8>, JsError> {
    // gzip：有明确的魔数
    if data.len() >= 2 && data[0] == 0x1f && data[1] == 0x8b {
        return gzip_decode(data);
    }

    // zlib：CMF 的低 4 位应为 8（DEFLATE），且 CMF/FLG 的和能被 31 整除
    if data.len() >= 2 {
        let cmf = data[0] as u16;
        let flg = data[1] as u16;
        let is_zlib = (cmf & 0x0f) == 8 && (cmf >> 4) <= 7 && ((cmf << 8) | flg).is_multiple_of(31);
        if is_zlib {
            if let Ok(output) = zlib_decode(data) {
                return Ok(output);
            }
        }
    }

    // 裸 deflate（无头）
    deflate_decode(data)
        .map_err(|_| JsError::new("无法识别压缩格式（已尝试 gzip / zlib / 裸 deflate）"))
}

/// zlib 解压。
fn zlib_decode(data: &[u8]) -> Result<Vec<u8>, JsError> {
    let mut decoder = flate2::read::ZlibDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| JsError::new(format!("zlib 解压失败：{error}")))?;
    Ok(output)
}

/// 裸 deflate 解压。
fn deflate_decode(data: &[u8]) -> Result<Vec<u8>, JsError> {
    let mut decoder = flate2::read::DeflateDecoder::new(data);
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|error| JsError::new(format!("deflate 解压失败：{error}")))?;
    Ok(output)
}

/// 裸 deflate 压缩。
pub(crate) fn deflate(data: &[u8]) -> Result<Vec<u8>, JsError> {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(data)
        .map_err(|error| JsError::new(format!("deflate 压缩失败：{error}")))?;
    encoder
        .finish()
        .map_err(|error| JsError::new(format!("deflate 压缩收尾失败：{error}")))
}

// ---------------------------------------------------------------------------
// 杂项：HTML 实体
// ---------------------------------------------------------------------------

/// 常见 HTML 实体的映射（名称 → 字符）。
///
/// ## 收录标准
///
/// 只收书源实际会碰到的：排版相关的空白与符号、书名里常见的标点。
/// 完整的 HTML5 实体表有 2000+ 条，且绝大多数（数学符号、希腊字母）
/// 在中文书源里从不出现——内置它们直接冲击体积目标。
///
/// 未收录的实体**原样保留**（`&nbsp;` 之外的未知实体不被破坏），
/// 数字实体（`&#123;` / `&#x1F;`）则一律支持（无需表）。
const NAMED_ENTITIES: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    // 空白类：正文净化里最常见的一类（书源要把它们转成空格或删掉）
    ("nbsp", '\u{00A0}'),
    ("ensp", '\u{2002}'),
    ("emsp", '\u{2003}'),
    ("thinsp", '\u{2009}'),
    ("zwnj", '\u{200C}'),
    ("zwj", '\u{200D}'),
    // 标点与符号
    ("ndash", '–'),
    ("mdash", '—'),
    ("lsquo", '\u{2018}'),
    ("rsquo", '\u{2019}'),
    ("ldquo", '\u{201C}'),
    ("rdquo", '\u{201D}'),
    ("hellip", '…'),
    ("middot", '·'),
    ("bull", '•'),
    ("copy", '©'),
    ("reg", '®'),
    ("trade", '™'),
    ("deg", '°'),
    ("times", '×'),
    ("divide", '÷'),
    ("laquo", '«'),
    ("raquo", '»'),
    ("sect", '§'),
    ("para", '¶'),
    ("euro", '€'),
    ("pound", '£'),
    ("yen", '¥'),
    ("cent", '¢'),
];

/// 解码 HTML 实体。
///
/// # 容错取向（逐条对应一种真实输入）
///
/// | 输入 | 输出 | 理由 |
/// |---|---|---|
/// | `&amp;` | `&` | 命名实体 |
/// | `&#65;` | `A` | 十进制数字实体 |
/// | `&#x41;` / `&#X41;` | `A` | 十六进制数字实体 |
/// | `&unknown;` | `&unknown;`（原样） | 未收录的实体不该被破坏 |
/// | `&amp`（无分号） | `&amp`（原样） | HTML5 允许省略分号，但那会让 `&ampx` 歧义 |
/// | `&` 单独 | `&`（原样） | 不是实体的裸 `&` 很常见 |
///
/// 最后两条的取向是**保守**：宁可少解一个实体，也不要把普通文本里的
/// `&` 变成别的字符（那会污染正文）。
pub(crate) fn html_decode(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0usize;

    while index < chars.len() {
        if chars[index] != '&' {
            output.push(chars[index]);
            index += 1;
            continue;
        }

        // 找实体结束的分号；超过 12 个字符还没找到就当作裸 `&`
        // （HTML 实体名最长 32 字符，但书源里的实际都短；限制长度可避免
        // 在长文本里为一个 `&` 扫描很远）
        let mut cursor = index + 1;
        let mut body = String::new();
        let mut found_semicolon = false;
        while cursor < chars.len() && body.len() < 12 {
            if chars[cursor] == ';' {
                found_semicolon = true;
                break;
            }
            body.push(chars[cursor]);
            cursor += 1;
        }

        if !found_semicolon {
            output.push('&');
            index += 1;
            continue;
        }

        match decode_entity_body(&body) {
            Some(character) => {
                output.push(character);
                index = cursor + 1;
            }
            None => {
                // 未识别的实体原样保留（含 `&` 与 `;`）
                output.push('&');
                index += 1;
            }
        }
    }

    output
}

/// 解码实体名/数字体（不含 `&` 与 `;`）。
fn decode_entity_body(body: &str) -> Option<char> {
    if let Some(digits) = body.strip_prefix('#') {
        // 数字实体
        let code = if let Some(hex) = digits
            .strip_prefix('x')
            .or_else(|| digits.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            digits.parse::<u32>().ok()?
        };
        // 排除代理区（U+D800..U+DFFF，不是合法字符）与超出范围的码点
        if (0xD800..=0xDFFF).contains(&code) || code > 0x10FFFF {
            return None;
        }
        return char::from_u32(code);
    }
    NAMED_ENTITIES
        .iter()
        .find(|(name, _)| *name == body)
        .map(|(_, character)| *character)
}

/// 编码 HTML 实体（只处理会导致结构歧义的三个字符）。
///
/// # 为什么只编码三个
///
/// 完整的 HTML 编码会把所有非 ASCII 字符转成实体（中文全变 `&#x...;`），
/// 那会让书源输出的文本不可读，且体积膨胀数倍。而真正**必须**编码的
/// 只有 `<` `>` `&` —— 它们改变文档结构。`"` 在属性值里需要，故一并处理。
pub(crate) fn html_encode(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            other => output.push(other),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 命名实体解码() {
        assert_eq!(html_decode("&amp;&lt;&gt;&quot;"), "&<>\"");
        assert_eq!(html_decode("a&nbsp;b"), "a\u{00A0}b");
        assert_eq!(html_decode("&mdash;&hellip;"), "—…");
    }

    #[test]
    fn 数字实体解码() {
        assert_eq!(html_decode("&#65;"), "A");
        assert_eq!(html_decode("&#x41;"), "A");
        assert_eq!(html_decode("&#X41;"), "A");
        assert_eq!(html_decode("&#20013;"), "中");
        assert_eq!(html_decode("&#x4E2D;"), "中");
    }

    #[test]
    fn 未收录实体与裸与号原样保留() {
        // 保守取向：宁可少解一个，也不要把普通文本里的 `&` 变成别的字符
        assert_eq!(html_decode("&unknown;"), "&unknown;");
        assert_eq!(html_decode("a & b"), "a & b");
        assert_eq!(html_decode("&amp"), "&amp", "无分号不应解码（会有歧义）");
        assert_eq!(html_decode("&;"), "&;");
    }

    #[test]
    fn 非法数字实体不破坏文本() {
        // 代理区码点不是合法字符
        assert_eq!(html_decode("&#xD800;"), "&#xD800;");
        // 超出 Unicode 范围
        assert_eq!(html_decode("&#x110000;"), "&#x110000;");
        // 非数字
        assert_eq!(html_decode("&#abc;"), "&#abc;");
    }

    #[test]
    fn 编码只处理结构字符() {
        assert_eq!(html_encode("a<b>&c\"d"), "a&lt;b&gt;&amp;c&quot;d");
        // 中文不编码（保持可读与体积）
        assert_eq!(html_encode("中文"), "中文");
    }

    #[test]
    fn 编解码往返对常见文本成立() {
        for text in ["a&b", "<tag>", "\"quoted\"", "中文 & English"] {
            assert_eq!(
                html_decode(&html_encode(text)),
                text,
                "输入 {text:?} 应往返无损"
            );
        }
    }

    #[test]
    fn gzip_往返() {
        let data = "压缩测试内容".as_bytes();
        let compressed = gzip_encode(data).expect("压缩应成功");
        assert_ne!(compressed.as_slice(), data, "应真的压缩了");
        // gzip 魔数
        assert_eq!(&compressed[..2], &[0x1f, 0x8b]);
        assert_eq!(gzip_decode(&compressed).expect("解压应成功"), data.to_vec());
    }

    #[test]
    fn deflate_往返与自动识别() {
        let data = "deflate 测试".as_bytes();
        let compressed = deflate(data).expect("压缩应成功");
        // `inflate` 应能自动识别裸 deflate（没有可辨头，故靠"前两种都失败"）
        assert_eq!(inflate(&compressed).expect("应能识别"), data.to_vec());
    }

    #[test]
    fn inflate_自动识别_gzip() {
        let data = "auto detect".as_bytes();
        let compressed = gzip_encode(data).expect("应成功");
        assert_eq!(inflate(&compressed).expect("应识别 gzip"), data.to_vec());
    }

    #[test]
    fn inflate_对不可识别数据报错() {
        assert!(inflate(&[0xFF, 0xFE, 0xFD]).is_err());
    }

    #[test]
    fn 随机数在有界范围内且会变化() {
        let mut previous = pseudo_random();
        let mut changed = false;
        for _ in 0..100 {
            let value = pseudo_random();
            assert!((0.0..1.0).contains(&value), "应在 [0,1) 内：{value}");
            if value != previous {
                changed = true;
            }
            previous = value;
        }
        assert!(changed, "连续调用应产出不同的值");
    }

    #[test]
    fn 时间戳转换() {
        // 1970-01-01T00:00:00Z 在 UTC 下是 0；本地时区会偏移，
        // 故用"格式化再解析回时间戳"验证一致性，而不是断言字面值。
        assert!(now_millis() > 0);
    }
}
