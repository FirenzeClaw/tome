//! [DONE] JS 沙箱的组合根装配（S1-2 的接线点）
//!
//! # 本模块为什么在 CLI 而不在某实现 crate
//!
//! 它是**组合根**的职责：把 `tome-js` 的沙箱、`tome-rule` 的规则引擎、
//! `tome-net` 的网络能力、`tome-source` 的管线装配起来。
//!
//! 按架构铁律，实现 crate 之间只依赖 `tome-core`（星型无环），故"把
//! 它们接在一起"这件事只能发生在唯一允许依赖全部实现 crate 的位置——
//! CLI（`plan.md` 的依赖图：`tome-app`/`CLI` 是组合根）。
//!
//! # 它做三件事
//!
//! 1. **建引擎**：为某个书源建 `SourceJsEngine`，并把宿主能力（网络 + 存储）
//!    注入进去（[`build_source_engine`]）。
//! 2. **注册宿主函数**：把 `java.*` 的八类 API 挂到全局
//!    （[`register_host_api`]）。
//! 3. **装配上下文对象**：`source`/`book`/`chapter`（[`build_source_engine`]
//!    内部完成）。
//!
//! # 顺序为什么不能改
//!
//! ```text
//!   Sandbox::new            → 建 runtime（三道护栏就位）
//!   install_host_bridge     → 注入宿主能力（必须在任何脚本求值之前）
//!   register_host_api       → 注册 java.*（在 Ctx 上操作）
//!   eval(书源脚本)          → 脚本可以用到上面两样
//! ```
//!
//! 顺序反了的后果由 `tome_js::Sandbox::install_host_bridge` 的守卫挡住
//! （它会报"必须在首次书源脚本求值之前注入"），而不是留下一个
//! "运行时才发现的宿主桥未注入"的谜。

use std::sync::Arc;

use tome_core::{NetHost, StoreHost};
use tome_js::Ctx;
use tome_source::SourceError;

/// 为一个书源建 JS 引擎，并完成全部装配。
///
/// 供 CLI 的 JS 引擎工厂调用（见 `tome_source::search::JsEngineFactory`）。
///
/// # 参数为什么是 `Option`
///
/// "不提供网络能力"是**正常配置**（跑纯计算脚本时的最小权限），故两者
/// 都是可选。缺失时对应的宿主函数会抛结构化的能力错误（FR-006）。
pub fn build_source_engine(
    source: &tome_core::Source,
    net: Option<Arc<dyn NetHost>>,
    store: Option<Arc<dyn StoreHost>>,
) -> Result<tome_source::js::SourceJsEngine, SourceError> {
    // ## 存储能力由组合根注入（T063）
    //
    // 此前这里硬编码 `tome_js::MemoryStoreHost`——进程内内存替身，退出即丢，
    // 于是脚本的 `java.setCookie` **不会**真的落盘（FR-026 的"重启后保留
    // 登录态"在脚本路径上是空头支票）。
    //
    // 现在改为由调用方传入：组合根给的是 `tome_store::SqliteStoreHost`
    // （按书源分区），测试给的是内存替身。本函数不关心是哪一个——
    // 那正是接缝存在的意义（A1）。
    //
    // ## 为什么保留"不传"这条路径
    //
    // `None` 表示"本次运行不提供存储能力"。此时脚本的 `java.cachePut` /
    // `setCookie` 抛**结构化能力错误**（FR-006），书源的 `try/catch`
    // 能据此降级——而不是拿到一个"写成功了但其实没落盘"的假象。
    let engine =
        tome_source::js::SourceJsEngine::for_source(source.id.clone(), source, net, store)?;

    // 注册宿主函数与上下文对象（必须在脚本求值之前）。
    engine.with_context(|ctx| {
        register_host_api(&ctx)?;
        // 上下文对象（`source`/`book`/`chapter`）需要宿主桥里的 JsContext，
        // 故经 `install_context_objects` 显式装配。
        let bridged = ctx
            .userdata::<tome_js::HostBridge>()
            .ok_or_else(|| "宿主桥应已注入".to_string())?;
        tome_js::api::state::install_context_objects(&ctx, &bridged)
            .map_err(|error| format!("装配上下文对象失败：{error}"))
    })?;

    Ok(engine)
}

/// 把 `java.*` 的八类宿主 API 注册到全局。
///
/// # 覆盖矩阵（FR-005 的八类 → 各模块的 `register`）
///
/// | 类别 | 模块 | 任务 |
/// |---|---|---|
/// | 网络 | [`tome_js::api::net`] | T036 |
/// | 文本与编码 | [`tome_js::api::text`] | T037 |
/// | 摘要与加密 | [`tome_js::api::crypto`] | T038 |
/// | 结构化数据 | [`tome_js::api::data`] | T039 |
/// | 正则/时间/杂项 | [`tome_js::api::util`] | T040 |
/// | 宿主状态 | [`tome_js::api::state`] | T041 |
/// | 不支持的能力 | [`tome_js::api::unsupported`] | T042 |
///
/// # 三处需要显式传入的宿主
///
/// `net` 与 `data` 的注册接受一个"能力句柄"：
///
/// - `net`（T036）需要 `Arc<dyn NetHost>`
/// - `data`（T039）需要 `Arc<dyn MarkupHost>`（网页标记解析）
///
/// 本函数从**宿主桥**里取出它们，而不是再要一遍参数——那样组合根就
/// 只需在一处（`HostBridge`）配置能力，避免"两处配置不一致"。
///
/// `MarkupHost` 由 [`tome_rule::markup_host::RuleMarkupHost`] 提供
/// （P4 遗留的收口）：`java.getElement` / `java.getElements` 因此真的落到
/// 规则引擎的 DOM 上，而不是抛能力错误。
pub fn register_host_api(ctx: &Ctx<'_>) -> Result<(), String> {
    let java =
        tome_js::Object::new(ctx.clone()).map_err(|error| format!("建 java 对象失败：{error}"))?;

    // 从宿主桥取能力（缺失时不报错——那些入口会各自抛能力错误）
    let (net, _store) = {
        let bridged = ctx.userdata::<tome_js::HostBridge>().ok_or_else(|| {
            "宿主桥未注入（register_host_api 必须在 install_host_bridge 之后调用）".to_string()
        })?;
        let net = bridged.net.clone();
        let store = bridged.store.clone();
        (net, store)
    };

    let mut registered = 0usize;

    // 网络类（T036）
    registered += tome_js::api::net::register(ctx, &java, net)
        .map_err(|error| format!("注册网络类 API 失败：{error}"))?;
    // 文本与编码（T037）
    registered += tome_js::api::text::register(ctx, &java)
        .map_err(|error| format!("注册文本类 API 失败：{error}"))?;
    // 摘要与加密（T038）
    registered += tome_js::api::crypto::register(ctx, &java)
        .map_err(|error| format!("注册加密类 API 失败：{error}"))?;
    // 结构化数据（T039）——MarkupHost 由 tome-rule 实现（P4 遗留收口）
    registered += tome_js::api::data::register(ctx, &java, Some(markup_host()))
        .map_err(|error| format!("注册数据类 API 失败：{error}"))?;
    // 正则/时间/杂项（T040）
    registered += tome_js::api::util::register(ctx, &java)
        .map_err(|error| format!("注册工具类 API 失败：{error}"))?;
    // 宿主状态（T041）
    registered += tome_js::api::state::register(ctx, &java)
        .map_err(|error| format!("注册状态类 API 失败：{error}"))?;
    // 不支持的能力（T042）——挂在同一个 `java` 对象上
    registered += tome_js::api::register_all_unsupported(ctx, &java)
        .map_err(|error| format!("注册能力桩失败：{error:?}"))?;

    // 会话变量池的初始值：把书源的上下文灌进 `java`（书源脚本读它）
    //
    // `java.put`/`java.get` 经宿主状态类注册（T041），而 `source` 等
    // 上下文对象由 `install_context_objects` 装配（见调用方）。

    ctx.globals()
        .set("java", java)
        .map_err(|error| format!("设置全局 java 失败：{error}"))?;

    // 至少应注册几十个函数；太少说明某个模块的注册静默失败了。
    debug_assert!(
        registered >= 40,
        "宿主 API 的注册数量异常（{registered}），检查各模块的 register 是否都被调用"
    );
    Ok(())
}

/// 网页标记解析能力的实现（`RuleMarkupHost` 的无状态共享实例）。
///
/// # 为什么是"每次新建"而不是全局单例
///
/// `RuleMarkupHost` 是零状态类型（`Copy`），构造它没有任何开销——
/// 不需要一个 `OnceLock` 来"省下"这次构造。而全局单例会让
/// "本进程用了哪个 MarkupHost"变成隐式事实，与 A3（禁止全局 CONFIG
/// 单例）的取向相反。
fn markup_host() -> Arc<dyn tome_js::api::data::MarkupHost> {
    Arc::new(tome_rule::markup_host::RuleMarkupHost::new())
}

/// 为一个纯脚本源建执行器（T044）。
///
/// # 与 [`build_source_engine`] 的区别
///
/// 前者给"规则文本里含 JS"的源；本函数给"整个源就是一个脚本"的源
/// （`SourceKind::Js`）。后者的脚本是**入口函数集合**（`search`/
/// `bookInfo`/…），经 `tome_rule::mainjs` 的约定调用。
pub fn build_mainjs_runner(
    source: &tome_core::Source,
    net: Option<Arc<dyn NetHost>>,
    store: Option<Arc<dyn StoreHost>>,
) -> Result<tome_rule::mainjs::MainJsRunner, SourceError> {
    let script = source
        .js
        .as_deref()
        .ok_or_else(|| SourceError::incomplete(source.id.clone(), "纯脚本源缺少 js/mainJs 字段"))?;

    let context = tome_js::JsContext::for_source(source.id.clone()).with_source_request_config(
        source.charset.clone(),
        source
            .rate_limit_ms
            .and_then(|millis| u32::try_from(millis).ok()),
    );

    // 与 `build_source_engine` 同理：存储能力由组合根注入（T063）。
    // 纯脚本源（`SourceKind::Js`）同样会用到 `java.cachePut` 与 Cookie，
    // 故它也必须拿到真实存储——否则整类源在重启后丢失登录态。
    let mut bridge = match store {
        Some(store) => tome_js::HostBridge::bare(context).with_store(store),
        None => tome_js::HostBridge::bare(context),
    };
    if let Some(net) = net {
        bridge = bridge.with_net(net);
    }

    let mut runner = tome_rule::mainjs::MainJsRunner::new(tome_js::Limits::default())
        .map_err(|error| SourceError::rule(format!("创建 mainJs 运行时失败：{error}")))?;
    runner
        .install_bridge(bridge)
        .map_err(|error| SourceError::rule(format!("装配 mainJs 宿主桥失败：{error}")))?;
    // 注册宿主 API（必须在脚本求值之前）
    runner
        .with_context(|ctx| register_host_api(&ctx))
        .map_err(SourceError::rule)?;
    runner
        .load(script)
        .map_err(|error| SourceError::rule(format!("载入 mainJs 脚本失败：{error}")))?;
    Ok(runner)
}
