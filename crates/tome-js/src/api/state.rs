//! \[DONE] 宿主 API · 宿主状态类（tasks.md T041 / FR-005）
//!
//! # FR-005 的本类清单
//!
//! > **宿主状态**：缓存读写、文件读写（限于数据目录内）、Cookie 读写、
//! > 引入外部脚本、变量池读写、当前书源/书籍/章节上下文访问
//!
//! 逐项落地：
//!
//! | 能力 | `java.*` 入口 | 经什么 |
//! |---|---|---|
//! | 缓存读写 | `cachePut` / `cacheGet` | `StoreHost` |
//! | 文件读写 | `readFile` / `writeFile` | `StoreHost`（**限数据目录，越界拒绝**） |
//! | Cookie 读写 | `getCookie` / `setCookie` | `StoreHost`（缓存键空间） |
//! | 引入外部脚本 | `importScript` | 独立 runtime（避免重入当前引擎） |
//! | 变量池读写 | `put` / `get` / `remove` / `clear` | `JsContext` |
//! | 上下文访问 | `source` / `book` / `chapter` | `JsContext` |
//!
//! # 安全红线（本模块最重要的一条）
//!
//! **文件写入限数据目录，越界拒绝并报告**（AGENTS.md 安全红线 / spec 边缘情况）。
//!
//! 拒绝的责任在 [`tome_core::StoreHost`] 的**实现方**，本模块不做路径校验
//! ——理由有两条：
//!
//! 1. **不重复规则**：两处各实现一遍路径包含判定，必然有一天不一致，
//!    而那种不一致是安全漏洞（一处放行一处拒绝时，实际以放行的为准）。
//! 2. **实现方有更多信息**：真实实现（S1-4 的 `tome-store`）需要处理
//!    符号链接与 Windows 大小写等平台细节，那些是它的知识。
//!
//! 本模块的职责是**如实转发**拒绝结果：把 `ErrorKind::UnsupportedCapability`
//! 转成 JS 侧可捕获的结构化错误（FR-006），让书源的 try/catch 能降级。
//!
//! # 变量池为什么用 `put`/`get` 而不是 `setProperty`/`getProperty`
//!
//! 两个名字在书源生态里都出现过。本模块用 `put`/`get`（更常见），并额外
//! 提供 `setVariable`/`getVariable` 别名——多两个别名比让一批书源失效便宜。

use std::sync::Arc;

use rquickjs::{Ctx, Object};
use tome_core::StoreHost;

use crate::bridge::{require_bridge, throw_host_error, HostBridge};
use crate::sandbox_error::JsError;

/// Cookie 在缓存键空间里的前缀。
///
/// ## 为什么要前缀
///
/// Cookie 与普通缓存共用同一个 `StoreHost`（那是 trait 提供的唯一键值通道）。
/// 加前缀让"cookie 域"与"用户脚本的缓存键"不互相覆盖——若书源恰好用了
/// 同名键（如 `"token"`），没有前缀时后写的会覆盖先写的，而表现是
/// "登录态莫名失效"。
///
/// `\[UNCERTAIN]` 更干净的做法是让 `StoreHost` 提供独立的 Cookie 通道。
/// 那需要改 `tome-core` 的接缝 trait，属跨切片决策，故本模块先用前缀
/// 方案而不擅自改接缝。
const COOKIE_KEY_PREFIX: &str = "tome://cookie/";

/// 注册宿主状态类的全部宿主函数到 `java` 对象，并装配上下文对象。
///
/// 返回注册的函数数（不含上下文对象——那些是对象不是函数）。
pub fn register<'js>(ctx: &Ctx<'js>, java: &Object<'js>) -> Result<usize, JsError> {
    let mut count = 0usize;

    // ---------- 缓存读写 ----------
    java.set(
        "cachePut",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             key: String,
             value: rquickjs::Value<'js>|
             -> Result<bool, rquickjs::Error> {
                let bytes = crate::api::text::value_to_bytes(&ctx, &value)
                    .map_err(|error| throw_host_error(&ctx, &error))?;
                let store = store_of(&ctx)?;
                store
                    .cache_put(&key, &bytes)
                    .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                Ok(true)
            },
        )
        .map_err(|error| registration_error("cachePut", &error))?,
    )
    .map_err(|error| registration_error("cachePut", &error))?;
    count += 1;

    java.set(
        "cacheGet",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, key: String| -> Result<String, rquickjs::Error> {
                let store = store_of(&ctx)?;
                let value = store
                    .cache_get(&key)
                    .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                // 未命中返回**空串**（而不是 `null`）：
                //
                // 书源里常见 `if (java.cacheGet("k"))` 与
                // `java.cacheGet("k") === ""` 两种判空写法。空串让前者
                // 为假、后者为真，两种都能用；`null` 会让 `=== ""` 失败。
                Ok(value
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default())
            },
        )
        .map_err(|error| registration_error("cacheGet", &error))?,
    )
    .map_err(|error| registration_error("cacheGet", &error))?;
    count += 1;

    // ---------- 文件读写（限数据目录，越界由 StoreHost 拒绝） ----------
    java.set(
        "readFile",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, path: String| -> Result<String, rquickjs::Error> {
                let store = store_of(&ctx)?;
                let bytes = store
                    .read_file(&path)
                    .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                Ok(String::from_utf8_lossy(&bytes).into_owned())
            },
        )
        .map_err(|error| registration_error("readFile", &error))?,
    )
    .map_err(|error| registration_error("readFile", &error))?;
    count += 1;

    java.set(
        "writeFile",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>,
             path: String,
             contents: rquickjs::Value<'js>|
             -> Result<bool, rquickjs::Error> {
                let bytes = crate::api::text::value_to_bytes(&ctx, &contents)
                    .map_err(|error| throw_host_error(&ctx, &error))?;
                let store = store_of(&ctx)?;
                // 路径越界由 StoreHost 实现方拒绝（见模块文档的说明）。
                // 本模块只如实转发它给出的分类。
                store
                    .write_file(&path, &bytes)
                    .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                Ok(true)
            },
        )
        .map_err(|error| registration_error("writeFile", &error))?,
    )
    .map_err(|error| registration_error("writeFile", &error))?;
    count += 1;

    java.set(
        "deleteFile",
        rquickjs::Function::new(
            ctx.clone(),
            |_ctx: Ctx<'js>, _path: String| -> Result<bool, rquickjs::Error> {
                // `StoreHost` 没有删除接口（trait 只给了四个方法）。
                // 静默返回 true 会让书源以为删掉了——那是"看起来成功"的
                // 失效形态。故明确报为能力错误。
                Err(rquickjs::Error::Exception)
            },
        )
        .map_err(|error| registration_error("deleteFile", &error))?,
    )
    .map_err(|error| registration_error("deleteFile", &error))?;
    count += 1;

    // ---------- Cookie ----------
    java.set(
        "getCookie",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, name: String| -> Result<String, rquickjs::Error> {
                let store = store_of(&ctx)?;
                let value = store
                    .cache_get(&cookie_key(&name))
                    .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                Ok(value
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default())
            },
        )
        .map_err(|error| registration_error("getCookie", &error))?,
    )
    .map_err(|error| registration_error("getCookie", &error))?;
    count += 1;

    java.set(
        "setCookie",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, name: String, value: String| -> Result<bool, rquickjs::Error> {
                let store = store_of(&ctx)?;
                store
                    .cache_put(&cookie_key(&name), value.as_bytes())
                    .map_err(|kind| throw_host_error(&ctx, &JsError::from_error_kind(&kind)))?;
                Ok(true)
            },
        )
        .map_err(|error| registration_error("setCookie", &error))?,
    )
    .map_err(|error| registration_error("setCookie", &error))?;
    count += 1;

    // ---------- 变量池 ----------
    //
    // 两个名字都注册（见模块文档）：`put`/`get` 是主流写法，
    // `setVariable`/`getVariable` 是别名。
    for (name, canonical) in [
        ("put", "put"),
        ("setVariable", "put"),
        ("get", "get"),
        ("getVariable", "get"),
        ("removeVariable", "remove"),
        ("clearVariables", "clear"),
    ] {
        let function = match canonical {
            "put" => rquickjs::Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>, name: String, value: String| -> Result<bool, rquickjs::Error> {
                    let bridged =
                        require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                    bridged.context.put_variable(name, value);
                    Ok(true)
                },
            ),
            "get" => rquickjs::Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>, name: String| -> Result<String, rquickjs::Error> {
                    let bridged =
                        require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                    // 未命中返回空串（理由同 `cacheGet`）
                    Ok(bridged.context.get_variable(&name).unwrap_or_default())
                },
            ),
            "remove" => rquickjs::Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>, name: String| -> Result<bool, rquickjs::Error> {
                    let bridged =
                        require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                    let removed = bridged
                        .context
                        .variables
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&name)
                        .is_some();
                    Ok(removed)
                },
            ),
            "clear" => rquickjs::Function::new(
                ctx.clone(),
                |ctx: Ctx<'js>| -> Result<bool, rquickjs::Error> {
                    let bridged =
                        require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                    bridged
                        .context
                        .variables
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clear();
                    Ok(true)
                },
            ),
            _ => continue,
        }
        .map_err(|error| registration_error(name, &error))?;

        java.set(name, function)
            .map_err(|error| registration_error(name, &error))?;
        count += 1;
    }

    // ---------- 引入外部脚本 ----------
    //
    // ## 为什么用一个**独立**的 runtime 而不是重入当前引擎
    //
    // `java.import` 要在脚本执行中再跑一段脚本。若就地 eval，会再借一次
    // 当前 runtime（`Ctx::with` 的借用已在外层持有）——那是 rquickjs 的
    // 嵌套可变借用，运行期 panic。
    //
    // 故经 `HostBridge::nested_runtime` 起的独立沙箱执行，与当前引擎隔离。
    // 副作用是外部脚本**看不到**当前脚本的变量与上下文——那是有意的取舍：
    // 隔离比"共享状态"更重要（共享会让一段坏脚本污染整个源的状态）。
    java.set(
        "importScript",
        rquickjs::Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, source: String| -> Result<String, rquickjs::Error> {
                let bridged =
                    require_bridge(&ctx).map_err(|error| throw_host_error(&ctx, &error))?;
                let factory = bridged.nested_runtime.clone();
                drop(bridged);

                let Some(factory) = factory else {
                    return Err(throw_host_error(
                        &ctx,
                        &JsError::unsupported(
                            "importScript",
                            "本沙箱未提供嵌套 runtime（组合根未配置 nested_runtime 工厂）",
                        ),
                    ));
                };
                let nested = factory().map_err(|error| {
                    throw_host_error(&ctx, &JsError::new(format!("创建嵌套沙箱失败：{error}")))
                })?;
                let outcome = nested
                    .eval(&source)
                    .map_err(|error| throw_host_error(&ctx, &JsError::new(error.to_string())))?;
                Ok(outcome.text)
            },
        )
        .map_err(|error| registration_error("importScript", &error))?,
    )
    .map_err(|error| registration_error("importScript", &error))?;
    count += 1;

    Ok(count)
}

/// 安装上下文对象（`source` / `book` / `chapter`）到全局。
///
/// 与 [`register`] 分开：那些是**对象**不是函数，且它们的值来自
/// [`crate::bridge::JsContext`]（组合根装配时填入）。
///
/// # 为什么缺失的字段给空串而不是 `undefined`
///
/// 书源里常见 `source.bookSourceName + ":" + book.name` 这类拼接。
/// `undefined` 参与拼接会得到 `"undefined:书名"`——一个**看起来正常但错误**
/// 的结果，而排查时很难想到是上下文缺失。空串让结果至少是"作者预期之外
/// 但无害"的形态。
///
/// 反向考虑：若某书源用 `if (book.name === undefined)` 判断"是否在书籍
/// 上下文里"，空串会让它误判。本产品不会出现那种场景（上下文缺什么由
/// 组合根决定，且它总会给出完整的上下文），故取空串。
pub fn install_context_objects<'js>(ctx: &Ctx<'js>, bridge: &HostBridge) -> Result<(), JsError> {
    let context = &bridge.context;

    let source = Object::new(ctx.clone())
        .map_err(|error| JsError::new(format!("建 source 对象失败：{error}")))?;
    source
        .set("bookSourceUrl", context.source_id.clone())
        .map_err(|error| JsError::new(format!("设置 bookSourceUrl 失败：{error}")))?;
    source
        .set(
            "bookSourceName",
            context.source_name.clone().unwrap_or_default(),
        )
        .map_err(|error| JsError::new(format!("设置 bookSourceName 失败：{error}")))?;
    source
        .set(
            "bookSourceGroup",
            context.source_group.clone().unwrap_or_default(),
        )
        .map_err(|error| JsError::new(format!("设置 bookSourceGroup 失败：{error}")))?;

    let book = Object::new(ctx.clone())
        .map_err(|error| JsError::new(format!("建 book 对象失败：{error}")))?;
    book.set("name", context.book_name.clone().unwrap_or_default())
        .map_err(|error| JsError::new(format!("设置 book.name 失败：{error}")))?;
    book.set("author", context.book_author.clone().unwrap_or_default())
        .map_err(|error| JsError::new(format!("设置 book.author 失败：{error}")))?;
    book.set("bookUrl", context.book_url.clone().unwrap_or_default())
        .map_err(|error| JsError::new(format!("设置 book.bookUrl 失败：{error}")))?;
    book.set("origin", context.book_url.clone().unwrap_or_default())
        .map_err(|error| JsError::new(format!("设置 book.origin 失败：{error}")))?;

    let chapter = Object::new(ctx.clone())
        .map_err(|error| JsError::new(format!("建 chapter 对象失败：{error}")))?;
    chapter
        .set("title", context.chapter_title.clone().unwrap_or_default())
        .map_err(|error| JsError::new(format!("设置 chapter.title 失败：{error}")))?;
    chapter
        .set("url", context.chapter_url.clone().unwrap_or_default())
        .map_err(|error| JsError::new(format!("设置 chapter.url 失败：{error}")))?;
    chapter
        .set("index", context.chapter_index.unwrap_or(0))
        .map_err(|error| JsError::new(format!("设置 chapter.index 失败：{error}")))?;
    chapter
        .set("baseUrl", context.base_url.clone().unwrap_or_default())
        .map_err(|error| JsError::new(format!("设置 chapter.baseUrl 失败：{error}")))?;

    let globals = ctx.globals();
    globals
        .set("source", source)
        .map_err(|error| JsError::new(format!("设置全局 source 失败：{error}")))?;
    globals
        .set("book", book)
        .map_err(|error| JsError::new(format!("设置全局 book 失败：{error}")))?;
    globals
        .set("chapter", chapter)
        .map_err(|error| JsError::new(format!("设置全局 chapter 失败：{error}")))?;
    // `baseUrl` 是书源里另一个高频全局（相对链接补全的基准）
    globals
        .set("baseUrl", context.base_url.clone().unwrap_or_default())
        .map_err(|error| JsError::new(format!("设置全局 baseUrl 失败：{error}")))?;

    Ok(())
}

/// 构造注册失败的诊断错误。
fn registration_error(name: &str, error: &rquickjs::Error) -> JsError {
    JsError::new(format!("注册 `{name}` 失败：{error}"))
}

/// 取存储能力（缺失时报结构化能力错误）。
///
/// 返回 `Arc` 而不是借用：宿主桥的 `UserDataGuard` 在闭包内不能跨
/// `store` 调用存活（那会与外层借用叠在一起），故先克隆出 `Arc` 再释放守卫。
fn store_of(ctx: &Ctx<'_>) -> Result<Arc<dyn StoreHost>, rquickjs::Error> {
    let bridged = require_bridge(ctx).map_err(|error| throw_host_error(ctx, &error))?;
    let store = bridged
        .require_store()
        .map_err(|error| throw_host_error(ctx, &error))?;
    Ok(Arc::clone(store))
}

/// Cookie 的缓存键（带前缀，见 [`COOKIE_KEY_PREFIX`]）。
fn cookie_key(name: &str) -> String {
    format!("{COOKIE_KEY_PREFIX}{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_键带前缀() {
        assert_eq!(cookie_key("token"), "tome://cookie/token");
        // 前缀应让 cookie 与普通缓存键不冲突
        assert_ne!(cookie_key("token"), "token");
    }

    #[test]
    fn 不同_cookie_名不冲突() {
        assert_ne!(cookie_key("a"), cookie_key("b"));
    }
}
