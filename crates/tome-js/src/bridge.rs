//! \[DONE] 宿主函数桥：Rust ↔ JS 的调用约定（tasks.md T034）
//!
//! # 这个模块解决三个具体问题
//!
//! ## 1. 宿主状态的存放：不能靠闭包捕获
//!
//! 宿主函数需要拿到宿主能力（`NetHost` / `StoreHost`）以及脚本级上下文
//! （当前书源 id、变量池…）。最直观的写法是 `Function::new(ctx, move |…| …)`
//! 让闭包捕获它们——但那会掉进 rquickjs 的**嵌套可变借用**：
//!
//! ```text
//! ctx.with(|ctx| {                       // ← Ctx 已借出
//!     let pool: Rc<RefCell<VariablePool>> = …;
//!     let f = Function::new(ctx.clone(), move || { pool.borrow_mut() })?;
//!     //                                        ^^^^^^^^^^^^^^^^
//!     //   宿主函数被调用时，若它再去碰 ctx（例如抛异常、建对象），
//!     //   就与这里的借用叠在一起 → 运行时 panic
//! })
//! ```
//!
//! 故本模块的做法是把宿主状态放进 **`Ctx::userdata`**（rquickjs 为此提供的
//! 官方机制），宿主函数只持有 `Ctx`，需要状态时 `ctx.userdata()` 取。
//! 状态的所有权归运行时，生命周期由引擎管。
//!
//! ## 2. 同步阻塞 I/O 必须安全
//!
//! 书源脚本是同步语义（research.md R1）：`java.get(url)` 必须**当场**拿到
//! 结果。故宿主函数里会阻塞在 HTTP 请求上。三条约束：
//!
//! - **不得在阻塞期间持有引擎的锁**：宿主函数的整个执行期都在
//!   `Ctx::with` 的借用内（引擎保证），这是设计使然——故契约要求
//!   **每个书源脚本用独立 runtime**（A1/R1），阻塞只影响自己那一个脚本。
//! - **不得在异步执行器线程上 block_on**：`tome-net` 已对此返回结构化错误
//!   （`NetError::BlockingContext`），本模块只要把错误如实转成 JS 异常即可。
//! - **不得重入**：宿主函数不得回头去 eval 新脚本（那会再借一次）。
//!   需要执行外部脚本时（`java.import` 之类）走 [`HostBridge::with_nested_runtime`]
//!   提供的沙箱工厂，它起一个**独立的 runtime**，不重入当前引擎。
//!
//! ## 3. 宿主错误必须沿 JS 异常栈展开
//!
//! 宿主侧错误（网络失败、文件越界）要变成 JS 里的**可捕获异常**，
//! 而不是"静默返回 undefined"。书源的 `try/catch` 降级逻辑（FR-006）依赖
//! 这一点：`try { java.ajax(url) } catch (e) { 换个地址 }`。
//!
//! 实现上有一个坑：`rquickjs` 的 `Function::new` 闭包返回 `Result<T, Error>`
//! 时，只有返回 `Error::Exception` 才会让**当前**异常继续展开；若闭包
//! 直接返回 `Err(Error::FromJs{..})` 之类的引擎错误，表现是 Rust 侧收到
//! 错误而不是 JS 侧抛出。故本模块统一走 [`throw_host_error`]：
//! **先 `ctx.throw(...)` 建异常，再返回 `Error::Exception`**。

use rquickjs::Ctx;

use crate::sandbox_error::JsError;

/// 宿主能力与脚本级上下文的容器（放进 `Ctx::userdata`）。
///
/// # 为什么字段是 `Arc<dyn Trait>` 而不是泛型参数
///
/// 泛型会让"每个宿主 trait 组合"都产生一个新的单态化实例，而宿主能力是
/// **运行期由组合根注入**的（A1）——编译期无从知道具体类型。用 `dyn` 让
/// `HostBridge` 只有一种类型，注入方式简单（构造时给一组 trait 对象）。
///
/// # 为什么脚本上下文与宿主能力放在一起
///
/// 两者都是"宿主函数运行时要拿到的东西"，分开存会让每个宿主函数都要
/// 取两次 userdata。放在一个结构里，取一次就够。
pub struct HostBridge {
    /// 网络能力（`None` 表示本次沙箱未启用网络——宿主函数会抛能力错误）
    ///
    /// `None` 不是"忘了注入"，而是一个**正常配置**：跑纯文本处理的脚本时
    /// 不该给它网络能力（最小权限）。故它有明确语义，不是缺失。
    pub net: Option<std::sync::Arc<dyn tome_core::NetHost>>,
    /// 存储能力
    pub store: Option<std::sync::Arc<dyn tome_core::StoreHost>>,
    /// 加密能力（纯计算，通常总是可用）
    pub crypto: Option<std::sync::Arc<dyn tome_core::CryptoHost>>,
    /// 脚本级上下文（书源 id、变量池、Cookie 视图…）
    pub context: JsContext,
    /// 可复用的独立沙箱工厂（供 `java.import` 等需要**另起引擎**的场合）
    ///
    /// 用 `Arc<dyn Fn>` 而不是直接持有 `Sandbox`：需要的是"能造一个新的"，
    /// 而不是"复用同一个"——复用会重入（见模块文档的约束 3）。
    pub nested_runtime: Option<
        std::sync::Arc<
            dyn Fn() -> Result<crate::runtime::Sandbox, crate::runtime::SandboxError> + Send + Sync,
        >,
    >,
}

impl std::fmt::Debug for HostBridge {
    /// 手写 `Debug` 以避免打印宿主实现（可能含 Cookie 等敏感状态）。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostBridge")
            .field("net", &self.net.is_some())
            .field("store", &self.store.is_some())
            .field("crypto", &self.crypto.is_some())
            .field("context", &self.context)
            .field("nested_runtime", &self.nested_runtime.is_some())
            .finish()
    }
}

impl HostBridge {
    /// 构造一个只有上下文的桥（无宿主能力）。
    ///
    /// 用于纯计算脚本与测试——"没有网络能力"是明确的配置，不是缺省遗漏。
    pub fn bare(context: JsContext) -> Self {
        Self {
            net: None,
            store: None,
            crypto: None,
            context,
            nested_runtime: None,
        }
    }

    /// 挂上网络能力。
    pub fn with_net(mut self, net: std::sync::Arc<dyn tome_core::NetHost>) -> Self {
        self.net = Some(net);
        self
    }

    /// 挂上存储能力。
    pub fn with_store(mut self, store: std::sync::Arc<dyn tome_core::StoreHost>) -> Self {
        self.store = Some(store);
        self
    }

    /// 挂上加密能力。
    pub fn with_crypto(mut self, crypto: std::sync::Arc<dyn tome_core::CryptoHost>) -> Self {
        self.crypto = Some(crypto);
        self
    }

    /// 提供"另起沙箱"的工厂（供 `java.import` 执行外部脚本）。
    pub fn with_nested_runtime(
        mut self,
        factory: std::sync::Arc<
            dyn Fn() -> Result<crate::runtime::Sandbox, crate::runtime::SandboxError> + Send + Sync,
        >,
    ) -> Self {
        self.nested_runtime = Some(factory);
        self
    }

    /// 取网络能力，缺失时给出**结构化能力错误**（而不是 panic）。
    ///
    /// 返回 `JsError` 而非直接抛：抛异常需要 `Ctx`，而错误构造不该依赖它
    /// （便于在无 ctx 的单元测试里断言分类）。
    pub fn require_net(&self) -> Result<&std::sync::Arc<dyn tome_core::NetHost>, JsError> {
        self.net.as_ref().ok_or_else(|| {
            JsError::unsupported("network", "本沙箱未启用网络能力（组合根未注入 NetHost）")
        })
    }

    /// 取存储能力，缺失时报结构化能力错误。
    pub fn require_store(&self) -> Result<&std::sync::Arc<dyn tome_core::StoreHost>, JsError> {
        self.store.as_ref().ok_or_else(|| {
            JsError::unsupported("store", "本沙箱未启用存储能力（组合根未注入 StoreHost）")
        })
    }

    /// 取加密能力，缺失时报结构化能力错误。
    pub fn require_crypto(&self) -> Result<&std::sync::Arc<dyn tome_core::CryptoHost>, JsError> {
        self.crypto.as_ref().ok_or_else(|| {
            JsError::unsupported("crypto", "本沙箱未启用加密能力（组合根未注入 CryptoHost）")
        })
    }
}

/// # 为什么需要 `unsafe impl JsLifetime`
///
/// `Ctx::userdata::<U>()` 要求 `U: JsLifetime<'js>`——那是 rquickjs 用来
/// 追踪"哪些类型携带了 JS 值的生命周期"的机制。`HostBridge` 不携带任何
/// JS 值（它装的是宿主 trait 对象与普通数据），故它的生命周期与 `'js` 无关。
///
/// rquickjs 的文档明确说明这是安全的实现方式：
///
/// > If a type does not have any lifetimes associated with it or all the
/// > lifetimes are `'static` then it is always safe to implement `JsLifetime`.
///
/// ## SAFETY
///
/// `HostBridge` 的全部字段都是 `'static`（`Arc<dyn Trait>`、`String`、
/// `Option<String>`、`Arc<Mutex<HashMap>>`），既不含 `'js` 值也不含借用，
/// 故 `Changed<'to> = HostBridge` 恒成立——不存在"把某个 `'js` 换成别的
/// 生命周期"的可能。本 crate 因此允许这一处 `unsafe`（见 lib.rs 的说明）。
#[allow(unsafe_code)]
unsafe impl<'js> rquickjs::JsLifetime<'js> for HostBridge {
    type Changed<'to> = HostBridge;
}

/// 脚本级上下文（宿主函数运行时要拿到的东西）。
///
/// # 为什么变量池用 `Arc<Mutex<..>>` 而不是 `Rc<RefCell<..>>`
///
/// JS 是单线程的，`Rc<RefCell>` 看起来够用。但宿主函数**可能跨线程**：
/// `NetHost: Send + Sync`，而某些实现会在自己的线程池里回调。且
/// `Sandbox` 的未来用途包括"多个源并行跑各自的沙箱"（每个沙箱一个线程），
/// 那时若某个上下文被共享（如共享的变量池），`Rc` 会直接编译不过。
///
/// 另一个更具体的原因：`RefCell` 的借用在**同一线程**内也不能嵌套，
/// 而宿主函数之间可能相互调用（`java.ajax` → 内部触发 `java.base64`）。
/// `Mutex` 在同一线程重入会**死锁**，故本模块约定：变量池的访问点
/// **不得嵌套**（每个宿主函数取用后立即释放，不跨调用持有）。
#[derive(Clone, Debug, Default)]
pub struct JsContext {
    /// 当前书源 id（限速键、诊断、缓存键都可能用到）
    pub source_id: String,
    /// 脚本变量池（`java.put`/`java.get` 的共享存储）
    ///
    /// ## 为什么它必须是 per-source 的
    ///
    /// 书源用它在同一次抓取的不同阶段之间传值（搜索页存 token，
    /// 目录页取用）。跨源共享会让两个源的变量互相污染——
    /// 那是极难排查的一类缺陷（表现为"偶尔取到别的源的值"）。
    pub variables: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    /// 当前书籍/章节上下文（`baseUrl` 等）
    pub base_url: Option<String>,
    /// 书源声明的字符集（FR-009）
    ///
    /// 放在这里而不是"每次请求都从书源对象现取"：宿主函数只有桥，
    /// 拿不到书源对象（那会让 tome-js 依赖 tome-source 的类型）。
    /// 组合根在装配时把值填进来。
    pub declared_charset: Option<String>,
    /// 书源声明的请求间隔（毫秒，来自 R5 的 `concurrentRate`）
    ///
    /// 同 `declared_charset`：由组合根在装配时填入，供网络类入口构造
    /// `RequestSpec` 时使用（FR-008）。
    pub request_interval_ms: Option<u32>,
    /// 书源名（脚本里 `source.bookSourceName`）
    pub source_name: Option<String>,
    /// 书源分组（`source.bookSourceGroup`）
    pub source_group: Option<String>,
    /// 当前书籍名（`book.name`）
    pub book_name: Option<String>,
    /// 当前书籍作者（`book.author`）
    pub book_author: Option<String>,
    /// 当前书籍详情页地址（`book.bookUrl`）
    pub book_url: Option<String>,
    /// 当前章节标题（`chapter.title`）
    pub chapter_title: Option<String>,
    /// 当前章节地址（`chapter.url`）
    pub chapter_url: Option<String>,
    /// 当前章节序号（`chapter.index`）
    ///
    /// `Option<u32>` 而非 `u32`：0 是合法序号，用 `Option` 区分
    /// "没有章节上下文"与"第 0 章"。
    pub chapter_index: Option<u32>,
}

impl JsContext {
    /// 为某个书源构造上下文。
    pub fn for_source(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            variables: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            base_url: None,
            declared_charset: None,
            request_interval_ms: None,
            source_name: None,
            source_group: None,
            book_name: None,
            book_author: None,
            book_url: None,
            chapter_title: None,
            chapter_url: None,
            chapter_index: None,
        }
    }

    /// 带上书源声明的字符集与请求间隔（组合根装配时调用）。
    ///
    /// 两者都不影响脚本可见的接口，只影响宿主函数构造 `RequestSpec` 时的
    /// 默认值（FR-008/FR-009）。
    pub fn with_source_request_config(
        mut self,
        charset: Option<String>,
        request_interval_ms: Option<u32>,
    ) -> Self {
        self.declared_charset = charset;
        self.request_interval_ms = request_interval_ms;
        self
    }

    /// 读一个变量。`Mutex` 毒化时用 `into_inner` 继续（数据是脚本变量，
    /// 一个宿主函数 panic 不该让整个源的所有变量不可读）。
    pub fn get_variable(&self, name: &str) -> Option<String> {
        self.variables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    /// 写一个变量。
    pub fn put_variable(&self, name: impl Into<String>, value: impl Into<String>) {
        self.variables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.into(), value.into());
    }

    /// 全部变量（诊断与测试用）。
    pub fn all_variables(&self) -> std::collections::HashMap<String, String> {
        self.variables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// 把宿主侧错误抛成 JS 异常（见模块文档的约束 3）。
///
/// # 必须"先 throw 再返回 `Error::Exception`"
///
/// `rquickjs` 的宿主函数若直接返回 `Err(其他变体)`，Rust 侧会收到那个错误，
/// 而 **JS 侧不会进入 catch**——书源的降级逻辑因此失效。唯一能让异常
/// 沿 JS 栈展开的方式是：先把异常值塞进引擎（`ctx.throw`），再返回
/// `Error::Exception`。
///
/// 返回值固定是 `Error::Exception`：调用方直接 `return` 它即可。
///
/// # 为什么异常属性设置失败不报错
///
/// `object.set(..)` 在极端情况下（内存耗尽）可能失败。此时**已有的异常
/// 仍然有效**——只是少了 `capability` 属性。为了把属性设置成功而放弃
/// 抛异常，会让脚本连 `catch` 都进不去（更糟）。故这里刻意忽略那些错误，
/// 保证"异常一定会抛出去"这条首要语义。
pub fn throw_host_error<'js>(ctx: &Ctx<'js>, error: &JsError) -> rquickjs::Error {
    // 异常值用 `Error` 对象而不是字符串：书源里常见的写法是
    // `catch (e) { e.message }` 或 `e.name`，字符串会得到 `undefined`。
    let message = error.message();
    let exception = match rquickjs::Exception::from_message(ctx.clone(), &message) {
        Ok(exception) => exception,
        // 连异常对象都建不出来（内存耗尽）时退回字符串异常：
        // 脚本的 catch 仍会收到一个值（`e` 是字符串），比不抛好。
        Err(_) => {
            let value: rquickjs::Value<'js> = rquickjs::String::from_str(ctx.clone(), &message)
                .map(|text| text.into_value())
                .unwrap_or_else(|_| rquickjs::Value::new_undefined(ctx.clone()));
            return ctx.throw(value);
        }
    };

    // 能力错误单独打标：书源可以据此判断"是本产品做不到"而不是"这次失败了"
    // （FR-006 的目的：降级而不是重试）。
    if let Some(capability) = error.capability() {
        let object = exception.as_object();
        let _ = object.set("capability", capability);
        let _ = object.set("isUnsupportedCapability", true);
        let _ = object.set("name", "UnsupportedCapabilityError");
    }
    ctx.throw(exception.into_value())
}

/// 用 `Ctx::userdata` 取宿主桥。
///
/// # 为什么是自由函数而不是 `Ctx` 的扩展方法
///
/// `Ctx` 是 rquickjs 的类型，本项目不能给它加 inherent impl（孤儿规则），
/// 而 trait 扩展方法会让每个调用点多一处 `use`。自由函数最简单，
/// 也让"取桥"这件事在代码里显式可见。
///
/// # 返回类型为什么写全路径、为什么借期绑在 `&Ctx` 上
///
/// `UserDataGuard` 未被 rquickjs 从 crate 根重导出（它在
/// `rquickjs::runtime` 下）。它的生命周期参数绑在**对 `Ctx` 的借用**上
/// （而非仅 `'js`）——这不是笔误：守卫存活期间同一份 userdata 不能被
/// 第二次借用，而"存活多久"只能由调用方手里的那个 `&Ctx` 界定。
/// 故签名写成 `&'a Ctx<'js> -> UserDataGuard<'a, HostBridge>`。
///
/// 返回 `None` 表示宿主桥未注入——那是**组合根的配置问题**（生产路径禁止
/// panic，故不 unwrap）。调用方应转成 [`JsError::unsupported`]。
pub fn host_bridge<'a, 'js>(
    ctx: &'a Ctx<'js>,
) -> Option<rquickjs::runtime::UserDataGuard<'a, HostBridge>> {
    ctx.userdata::<HostBridge>()
}

/// 取宿主桥，缺失时报结构化错误。
pub fn require_bridge<'a, 'js>(
    ctx: &'a Ctx<'js>,
) -> Result<rquickjs::runtime::UserDataGuard<'a, HostBridge>, JsError> {
    host_bridge(ctx).ok_or_else(|| {
        JsError::unsupported(
            "hostBridge",
            "宿主桥未注入（组合根需在 eval 前调用 install_host_bridge）",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn 缺失的宿主能力给出结构化能力错误() {
        // 不给网络能力是**正常配置**（最小权限），但要报得清楚：
        // "本沙箱未启用网络能力"而不是"未定义的方法"。
        //
        // ## 为什么用 `match` 而不是 `expect_err`
        //
        // `Result::expect_err` 要求 `Ok` 侧实现 `Debug`，而 `Arc<dyn NetHost>`
        // 之类的 trait 对象没有 `Debug`（trait 没要求它）。这不是可以
        // 顺手加上的东西——给接缝 trait 加 `Debug` 约束会波及所有实现方。
        let bridge = HostBridge::bare(JsContext::for_source("s"));
        match bridge.require_net() {
            Ok(_) => panic!("未注入网络能力时 require_net 应报错"),
            Err(error) => {
                assert_eq!(error.capability(), Some("network"));
                // 能力错误映射到 `UnsupportedCapability` 而**不是** `Rule`：
                // FR-006 要求它是可编程信号（"此路不通，请改道"）而非普通失败。
                // 上层据此给退出码 3 与不同的处置建议。
                assert!(matches!(
                    error.to_error_kind(),
                    tome_core::ErrorKind::UnsupportedCapability { .. }
                ));
            }
        }
        match bridge.require_store() {
            Ok(_) => panic!("未注入存储能力时 require_store 应报错"),
            Err(error) => assert_eq!(error.capability(), Some("store")),
        }
        match bridge.require_crypto() {
            Ok(_) => panic!("未注入加密能力时 require_crypto 应报错"),
            Err(error) => assert_eq!(error.capability(), Some("crypto")),
        }
    }

    #[test]
    fn 注入后的能力可取出() {
        let bridge = HostBridge::bare(JsContext::for_source("s"))
            .with_store(std::sync::Arc::new(
                crate::testdouble::MemoryStoreHost::new(),
            ))
            .with_crypto(std::sync::Arc::new(
                crate::testdouble::MemoryCryptoHost::new(),
            ));
        assert!(bridge.store.is_some());
        assert!(bridge.crypto.is_some());
        // 未注入的仍应报错（不是"注入了一个就全都通"）
        match bridge.require_net() {
            Ok(_) => panic!("网络能力未注入，应报错"),
            Err(error) => assert_eq!(error.capability(), Some("network")),
        }
    }

    #[test]
    fn debug_输出只报告有无不打印宿主() {
        // 宿主实现可能持有 Cookie 等敏感状态（AGENTS.md 安全红线：
        // 禁止在日志记录敏感数据）。故 `Debug` 只报告"有没有"。
        let bridge = HostBridge::bare(JsContext::for_source("https://s.test"));
        let rendered = format!("{bridge:?}");
        assert!(rendered.contains("net: false"), "{rendered}");
        assert!(rendered.contains("https://s.test"), "{rendered}");
    }

    #[test]
    fn 变量池按源隔离() {
        let first = JsContext::for_source("https://a.test");
        let second = JsContext::for_source("https://b.test");
        first.put_variable("token", "aaa");
        assert_eq!(first.get_variable("token").as_deref(), Some("aaa"));
        assert_eq!(
            second.get_variable("token"),
            None,
            "不同书源的变量池必须隔离（共享会让变量相互污染）"
        );
    }

    #[test]
    fn 变量池可覆盖与列举() {
        let context = JsContext::for_source("s");
        context.put_variable("a", "1");
        context.put_variable("a", "2");
        context.put_variable("b", "3");
        assert_eq!(context.get_variable("a").as_deref(), Some("2"));
        let all: HashMap<String, String> = context.all_variables();
        assert_eq!(all.len(), 2);
        assert_eq!(all.get("b").map(String::as_str), Some("3"));
    }

    #[test]
    fn 克隆的上下文共享变量池() {
        // 宿主函数拿到的是上下文的克隆（放进 userdata 后每次取出来是同一份）。
        // 若克隆各自持一份，脚本 put 的变量在另一个宿主函数里就读不到——
        // 那是"变量池看起来能用但随机失效"的成因。
        let original = JsContext::for_source("s");
        let cloned = original.clone();
        original.put_variable("k", "v");
        assert_eq!(cloned.get_variable("k").as_deref(), Some("v"));
    }

    #[test]
    fn 缺省上下文是空书源_id() {
        let context = JsContext::default();
        assert_eq!(context.source_id, "");
        assert!(context.base_url.is_none());
    }
}
