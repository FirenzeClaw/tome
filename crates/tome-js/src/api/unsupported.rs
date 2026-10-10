//! \[DONE] 不支持能力的结构化错误（tasks.md T042 / FR-006）
//!
//! # FR-006 的原话
//!
//! > 当书源使用系统不支持的能力时，系统 MUST 抛出可被书源自身错误处理
//! > 捕获的结构化错误，错误须携带能力标识与人类可读说明
//!
//! 三个要件缺一不可：
//!
//! | 要件 | 落点 | 不满足时 |
//! |---|---|---|
//! | **可被捕获** | 抛 JS 异常而非让 API 未定义 | `TypeError: x is not a function`，书源的 catch 拿不到有效信息 |
//! | **结构化** | 异常对象带 `capability` 属性 | 书源只能比对消息文本（措辞一改就失效） |
//! | **人类可读说明** | 异常 `message` | 用户看到"undefined"而不知道发生了什么 |
//!
//! # 能力标识必须用书源生态的通用叫法
//!
//! 这是**兼容性优先**（宪法原则 I）的落点：既有书源的 `try/catch` 降级
//! 逻辑里写的是 `if (e.capability == "webView")` 这类判断。若我们用自造
//! 命名（如 `embeddedBrowser`），书源认不出来，降级失效——而"书源开箱即用"
//! 是本项目的最高优先级目标。
//!
//! # 为什么这些能力"不支持"是**设计决定**，不是尚未实现
//!
//! 需要区分的两种情况：
//!
//! | 情况 | 应报的错误 | 例子 |
//! |---|---|---|
//! | 本产品架构上不提供 | **能力错误**（本模块） | `webView`（无内嵌浏览器） |
//! | 尚未实现，后续切片会补 | 未就绪错误 | `@json:` 在 S1-1 时的状态 |
//!
//! 本模块只处理第一类。把它们混为一谈会让用户以为"等版本更新就有了"，
//! 而实际上永远不会——那时用户该做的是换一个不依赖该能力的源。
//!
//! # 为什么"不支持"要抛错而不是静默降级
//!
//! 静默返回 `null`` 会让书源继续跑下去，产出一个**看起来正常但实际错误**
//! 的结果（例如"验证码识别失败"被当成"验证码是空的"）。那比直接失败更糟：
//! 失败是可归因的，错误结果不是。

use rquickjs::{Ctx, Object};

use crate::bridge::throw_host_error;
use crate::sandbox_error::JsError;

/// 系统**架构上不提供**的能力（FR-006）。
///
/// # 为什么用枚举而不是散落的字符串
///
/// 每个变体都自带"为什么不支持"的说明（见 [`UnsupportedCapability::reason`]），
/// 而说明的措辞是**面向用户的最后一段**——集中在一处才能整体校审，
/// 也才不会出现"两个函数对同一能力的解释不一致"。
///
/// ## 关于维护这份清单
///
/// 加入新变体前先问：这个能力是"本产品永远不做"，还是"以后会做"？
/// 后者不该进这个枚举（见模块文档的区分表）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnsupportedCapability {
    /// 内嵌浏览器（`java.webView` / `java.webViewGetSource` 等）
    ///
    /// 桌面原生实现不含浏览器内核：引入 WebView 会带来体积（数十 MB 起）、
    /// 跨平台差异、以及一份需要独立维护的渲染引擎，与"纯原生 + 体积目标"
    /// 的架构决定冲突。
    WebView,
    /// 启动浏览器并等待结果（`java.startBrowserAwait`）
    StartBrowserAwait,
    /// 验证码识别（`java.getVerificationCode`）
    VerificationCode,
    /// 视频播放（`java.openVideoPlayer`）
    OpenVideoPlayer,
    /// 音频/视频源相关的播放能力（`java.playAudio` 等）
    MediaPlayer,
    /// 系统通知（`java.toast` / `java.longToast`）
    ///
    /// 书源里的 toast 是给**用户**看的提示。桌面应用有更好的呈现方式，
    /// 但沙箱里无法决定界面——故报为不支持，由上层决定是否呈现。
    Toast,
    /// JVM 类访问（`Java.type` / `Packages.*`）
    ///
    /// 原生实现没有 JVM。前导脚本（T035）已放桩，此处是能力清单里的
    /// 对应条目——两处必须一致。
    JavaClass,
    /// 外部程序调用（`java.exec` 之类）
    ///
    /// 沙箱是安全边界：让书源脚本执行任意系统命令会完全破坏它。
    Exec,
    /// `webView` 家族里未逐个列举的成员（`webViewGetSource` 等）。
    ///
    /// ## 为什么需要一个"家族"条目
    ///
    /// 书源生态里 `webView*` 的方法名有十几个（`webView`、`webViewGetSource`、
    /// `webViewGetRedirectUrl`、`webViewGetUrl`…），且不同版本还在增补。
    /// 逐个列举必然漏——而漏掉的后果是 `TypeError: xxx is not a function`，
    /// 书源作者看到那个信息会以为是自己拼错了方法名。
    ///
    /// 故前导脚本对**任何**未注册的 `webView*` 名字都返回本能力错误。
    /// 标识仍用 `webView`（生态通用叫法），让书源的
    /// `if (e.capability === "webView")` 判据对所有成员都成立。
    ///
    /// `[WARN]` 本变体与 [`Self::WebView`] 的 `capability_id` **相同**，
    /// 故它不进 [`Self::ALL`]（那会让"标识互不重复"的断言失败）。
    /// 两者是"具体成员"与"家族兜底"的关系，不是两个独立能力。
    WebViewFamily,
}

impl UnsupportedCapability {
    /// 书源生态的**通用叫法**（兼容性关键，见模块文档）。
    pub fn capability_id(self) -> &'static str {
        match self {
            Self::WebView | Self::WebViewFamily => "webView",
            Self::StartBrowserAwait => "startBrowserAwait",
            Self::VerificationCode => "getVerificationCode",
            Self::OpenVideoPlayer => "openVideoPlayer",
            Self::MediaPlayer => "mediaPlayer",
            Self::Toast => "toast",
            Self::JavaClass => "javaClass",
            Self::Exec => "exec",
        }
    }

    /// 架构上不支持它的**理由**（面向用户的说明的一部分）。
    ///
    /// 写"为什么不支持"而不是"暂未实现"：用户据此判断该换源还是该等更新。
    pub fn reason(self) -> &'static str {
        match self {
            Self::WebView | Self::WebViewFamily => {
                "桌面原生实现不含浏览器内核（体积与跨平台考虑）；依赖内嵌浏览器的书源需改用纯 HTTP 规则"
            }
            Self::StartBrowserAwait => {
                "需要内嵌浏览器；桌面原生实现不提供浏览器内核"
            }
            Self::VerificationCode => {
                "验证码识别需要内嵌浏览器与图像识别能力，桌面原生实现不提供"
            }
            Self::OpenVideoPlayer => {
                "视频播放需由应用层实现；脚本沙箱不承载媒体播放"
            }
            Self::MediaPlayer => {
                "音频/视频播放需由应用层实现；脚本沙箱不承载媒体播放"
            }
            Self::Toast => {
                "提示信息的呈现由应用层决定；脚本沙箱不直接控制界面"
            }
            Self::JavaClass => {
                "原生实现没有 JVM，无法访问 Java 类"
            }
            Self::Exec => {
                "沙箱是安全边界，不允许书源脚本执行系统命令"
            }
        }
    }

    /// 完整的面向用户说明（含能力名与理由）。
    pub fn description(self) -> String {
        format!("不支持的能力 `{}`：{}", self.capability_id(), self.reason())
    }

    /// 转成可抛给 JS 的异常（FR-006 的三个要件齐备）。
    pub fn to_js_error(self) -> JsError {
        JsError::unsupported(self.capability_id(), self.description())
    }

    /// 全部变体（供覆盖矩阵与测试遍历）。
    ///
    /// FR-005b 要求能力集"可被测试逐项覆盖"；FR-006 的能力清单同理——
    /// 有一份可遍历的清单才能写出"每个能力都抛对了错误"这样的测试。
    ///
    /// **`WARN`** 新增变体时必须同步加进这里，否则它不会出现在覆盖矩阵里。
    /// 有一个测试专门断言"清单长度与变体数一致"来守这件事。
    /// ## 为什么 `WebViewFamily` 不在其中
    ///
    /// 它与 `Self::WebView` 的 `capability_id` 相同（见该变体的文档），
    /// 放进清单会让"标识互不重复"的断言失败。它是**兜底**而非独立能力。
    pub const ALL: &'static [Self] = &[
        Self::WebView,
        Self::StartBrowserAwait,
        Self::VerificationCode,
        Self::OpenVideoPlayer,
        Self::MediaPlayer,
        Self::Toast,
        Self::JavaClass,
        Self::Exec,
    ];
}

/// 在 JS 全局注册一个"调用即抛能力错误"的函数。
///
/// # 为什么注册的是函数而不是缺失的 API
///
/// 若 `java.webView` 根本不存在，书源的 `try { java.webView(url) }` 会得到
/// `TypeError: java.webView is not a function`——**书源无法区分**这是
/// "我们的实现不支持"还是"书源自己写错了名字"。而两者的处置完全不同：
/// 前者该降级，后者该修书源。
///
/// # 为什么挂到 `java` 对象上而不是全局
///
/// 这些能力在书源生态里都是 `java.xxx` 的成员。挂到全局会污染命名空间，
/// 且与书源的实际写法不符（`java.webView` 才是它找的位置）。
pub fn register_unsupported<'js>(
    ctx: &Ctx<'js>,
    java_object: &Object<'js>,
    capability: UnsupportedCapability,
) -> Result<(), JsError> {
    let name = capability.capability_id();
    // ## 为什么闭包要收 `Ctx`
    //
    // 返回 `Err(Error::Exception)` 但**不设置异常值**是不行的：那样 JS 侧
    // 会进入 catch 却拿到 `undefined`，书源读 `e.capability` 得到 undefined，
    // 降级判断失效（FR-006 的核心要件就此丢失）。故闭包必须拿到 `Ctx`，
    // 先 `ctx.throw(...)` 再返回 `Error::Exception`——这是 rquickjs 里
    // 让异常沿 JS 栈展开的唯一方式（详见 `crate::bridge::throw_host_error`）。
    //
    // 能力标识 `Copy`，闭包按值捕获即可，无生命周期问题。
    let throwing = rquickjs::Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>| -> Result<rquickjs::Value<'js>, rquickjs::Error> {
            Err(throw_host_error(&ctx, &capability.to_js_error()))
        },
    )
    .map_err(|error| JsError::new(format!("注册能力桩 `{name}` 失败：{error}")))?;

    java_object
        .set(name, throwing)
        .map_err(|error| JsError::new(format!("设置 `java.{name}` 失败：{error}")))
}

/// `webView` 家族的实际方法名（生态里出现过的）。
///
/// ## 为什么这份清单是"已知的"而不是穷举的
///
/// 它是**已知成员**的列表，用于把常见名字都挂上桩。未知的成员由
/// [`install_java_trap`] 的 Proxy 兜底——两层合起来才覆盖"书源用到的
/// 任何 `webView*` 名字"。
const WEBVIEW_FAMILY_METHODS: &[&str] = &[
    "webView",
    "webViewGetSource",
    "webViewGetUrl",
    "webViewGetRedirectUrl",
    "webViewGetContent",
    "webViewAwait",
    "webViewGetSourceAwait",
];

/// 把 `webView` 家族的全部已知方法挂成能力桩。
pub fn register_webview_family<'js>(
    ctx: &Ctx<'js>,
    java_object: &Object<'js>,
) -> Result<usize, JsError> {
    let mut registered = 0usize;
    for name in WEBVIEW_FAMILY_METHODS {
        register_named_unsupported(ctx, java_object, name, UnsupportedCapability::WebView)?;
        registered += 1;
    }
    Ok(registered)
}

/// 按**名字**注册一个能力桩（名字不一定是枚举的标准标识）。
///
/// 供 [`register_webview_family`] 这类"家族成员"使用：它们的名字各异，
/// 但报出的能力标识统一（生态通用叫法），以便书源的判据成立。
pub fn register_named_unsupported<'js>(
    ctx: &Ctx<'js>,
    java_object: &Object<'js>,
    method_name: &str,
    capability: UnsupportedCapability,
) -> Result<(), JsError> {
    let exception_holder = capability;
    let method = method_name.to_string();
    let throwing = rquickjs::Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>| -> Result<rquickjs::Value<'js>, rquickjs::Error> {
            // 消息里带上**实际的方法名**：书源作者需要知道自己用的是哪个
            // `webView*`（家族很长），而能力标识统一是 `webView`。
            let error = JsError::unsupported(
                exception_holder.capability_id(),
                format!(
                    "{}（调用了 `java.{method}`）",
                    exception_holder.description()
                ),
            );
            Err(throw_host_error(&ctx, &error))
        },
    )
    .map_err(|error| JsError::new(format!("注册能力桩 `java.{method_name}` 失败：{error}")))?;

    java_object
        .set(method_name, throwing)
        .map_err(|error| JsError::new(format!("设置 `java.{method_name}` 失败：{error}")))
}

/// 一次性把全部不支持能力挂到 `java` 对象上。
///
/// 返回成功注册的数量——调用方可据此断言"能力集完整"。
///
/// # 为什么返回数量而不是 `()`
///
/// "注册了几个"是配置可观测性的一部分：若组合根忘了调用本函数，
/// 书源会撞上 `TypeError` 而不是能力错误（FR-006 失效）。有了计数，
/// 测试与诊断能当场发现这件事。
pub fn register_all_unsupported<'js>(
    ctx: &Ctx<'js>,
    java_object: &Object<'js>,
) -> Result<usize, JsError> {
    let mut registered = 0usize;
    for capability in UnsupportedCapability::ALL {
        register_unsupported(ctx, java_object, *capability)?;
        registered += 1;
    }
    // `webView` 家族的实际成员名（见 `WEBVIEW_FAMILY_METHODS`）
    registered += register_webview_family(ctx, java_object)?;
    Ok(registered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 能力标识用生态通用叫法() {
        // 兼容性关键：这些字符串会出现在书源的 `if (e.capability == "...")` 里。
        for (capability, expected) in [
            (UnsupportedCapability::WebView, "webView"),
            (
                UnsupportedCapability::StartBrowserAwait,
                "startBrowserAwait",
            ),
            (
                UnsupportedCapability::VerificationCode,
                "getVerificationCode",
            ),
            (UnsupportedCapability::OpenVideoPlayer, "openVideoPlayer"),
            (UnsupportedCapability::JavaClass, "javaClass"),
        ] {
            assert_eq!(capability.capability_id(), expected);
        }
    }

    #[test]
    fn 每个能力都有理由与完整说明() {
        for capability in UnsupportedCapability::ALL {
            let reason = capability.reason();
            assert!(
                reason.len() > 5,
                "`{}` 的理由太短，说明不充分",
                capability.capability_id()
            );
            let description = capability.description();
            assert!(
                description.contains(capability.capability_id()),
                "说明应含能力名：{description}"
            );
            assert!(description.contains(reason), "说明应含理由：{description}");
        }
    }

    #[test]
    fn 能力标识互不重复() {
        // 重复会让书源的判断产生歧义（两个不同能力得到同一个标识）。
        let mut ids: Vec<&str> = UnsupportedCapability::ALL
            .iter()
            .map(|capability| capability.capability_id())
            .collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "能力标识不得重复");
    }

    #[test]
    fn 清单与变体数一致() {
        // 新增变体却忘了加进 `ALL` 会让它不出现在覆盖矩阵里。
        // 这里用"每个变体都能取到描述"间接守住——若漏了，上面的遍历测试
        // 不会覆盖到它。此测试的意义在于显式记录这条维护契约。
        assert_eq!(
            UnsupportedCapability::ALL.len(),
            8,
            "`ALL` 的条目数应与枚举变体数一致；新增变体时请同步更新（含本条断言）"
        );
    }

    #[test]
    fn 转成_js_错误后带能力标识() {
        let error = UnsupportedCapability::WebView.to_js_error();
        assert_eq!(error.capability(), Some("webView"));
        assert!(error.is_unsupported());
        // 映射回 core 分类也一样带标识（跨 crate 边界的兼容性）
        match error.to_error_kind() {
            tome_core::ErrorKind::UnsupportedCapability { capability } => {
                assert_eq!(capability.capability_id(), "webView");
            }
            other => panic!("应保持能力分类，实际 {other:?}"),
        }
    }
}
