//! T010 验收测试：`ErrorKind` 分类 + 可本地化标识（FR-006 / FR-044 / SC-016）
//!
//! ## 这些测试在守什么
//!
//! 1. **七类齐备**（网络/代理/规则/不支持能力/内容/存储/配置）——缺一类就会让某处
//!    错误被迫塞进不合适的分类，上层文案映射随之出错。
//!    （代理类是 2026-10-10 PM 仲裁新增，见 `ErrorKind::Proxy` 的文档。）
//! 2. **可本地化标识而非文案**（FR-044 / SC-016）——引擎内 MUST NOT 硬编码
//!    面向用户的自然语言。故断言的是"标识符"，且标识须是稳定的 ASCII 点分路径。
//! 3. **能力错误携带能力标识**（FR-006）——书源的 `try/catch` 降级逻辑依赖
//!    这个结构化错误，缺了它书源无法可捕获地降级。

use tome_core::error::{ErrorCapability, ErrorKind, LocalizedMessage};

/// 七类分类均存在且互不相等。
///
/// 注意 `UnsupportedCapability` 是**带载荷的变体**（必须携带能力标识，FR-006），
/// 故此处用构造出的值参与比较，而非裸的变体名。
#[test]
fn 错误分类覆盖七类且互异() {
    let all = [
        ErrorKind::Network,
        ErrorKind::Proxy,
        ErrorKind::Rule,
        ErrorKind::unsupported_capability(ErrorCapability::new("webView")),
        ErrorKind::Content,
        ErrorKind::Storage,
        ErrorKind::Configuration,
    ];

    assert_eq!(all.len(), 7, "PM 仲裁（2026-10-10）规定错误分七类");
    for (index, left) in all.iter().enumerate() {
        for right in all.iter().skip(index + 1) {
            assert_ne!(left, right, "分类必须互异：{left:?} 与 {right:?}");
        }
    }
}

/// 代理故障与网络故障在**分类层**就必须可分（FR-008 边缘情况）。
///
/// 这条断言的意义不只是"两个枚举值不同"：它锁住的是"上层无需下探诊断细节
/// 就能区分该改代理设置还是该查网络"。若日后有人把 `Proxy` 并入 `Network`
/// 以求简化，这里会失败并指向 FR-008。
#[test]
fn 代理故障与网络故障分类可分() {
    assert_ne!(
        ErrorKind::Proxy,
        ErrorKind::Network,
        "FR-008 要求代理故障与目标站故障可区分"
    );
    assert_ne!(ErrorKind::Proxy, ErrorKind::Configuration);
    assert_eq!(ErrorKind::Proxy.message().id(), "error.proxy");
}

/// 每一类都有**稳定的**可本地化标识。
///
/// 标识是对外契约（界面据此查文案表），故断言其精确值而非"非空"——
/// 否则改名不会失败，会静默破坏上层的文案映射。
#[test]
fn 每类错误都有稳定标识() {
    assert_eq!(ErrorKind::Network.message().id(), "error.network");
    assert_eq!(ErrorKind::Proxy.message().id(), "error.proxy");
    assert_eq!(ErrorKind::Rule.message().id(), "error.rule");
    assert_eq!(
        ErrorKind::unsupported_capability(ErrorCapability::new("webView"))
            .message()
            .id(),
        "error.unsupported_capability"
    );
    assert_eq!(ErrorKind::Content.message().id(), "error.content");
    assert_eq!(ErrorKind::Storage.message().id(), "error.storage");
    assert_eq!(
        ErrorKind::Configuration.message().id(),
        "error.configuration"
    );
}

/// 标识不得是自然语言文案（FR-044：引擎内禁止硬编码面向用户的自然语言）。
///
/// 用"点分小写 ASCII 路径"作为判据：既是本文档定义的格式，也天然排除
/// 中文文案与英文句子。
#[test]
fn 错误标识是点分路径而非自然语言() {
    for kind in [
        ErrorKind::Network,
        ErrorKind::Proxy,
        ErrorKind::Rule,
        ErrorKind::unsupported_capability(ErrorCapability::new("webView")),
        ErrorKind::Content,
        ErrorKind::Storage,
        ErrorKind::Configuration,
    ] {
        let message = kind.message();
        let id = message.id();
        assert!(
            id.chars()
                .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'),
            "标识 {id} 必须是小写 ASCII 点分路径"
        );
        assert!(id.starts_with("error."), "标识 {id} 必须以 error. 开头");
        assert!(
            !id.contains(' '),
            "标识 {id} 不得含空格——含空格即说明写成了文案"
        );
    }
}

/// 能力错误必须携带能力标识（FR-006）。
///
/// 书源里的降级写法形如"用 `java.webView` 拿不到就换 `@css:` 规则"，
/// 它靠的是**可捕获的结构化错误 + 能力标识**。标识缺失则书源只能靠
/// 错误文案做字符串匹配，规范不允许。
#[test]
fn 能力错误携带能力标识与说明() {
    let kind = ErrorKind::unsupported_capability(ErrorCapability::new("webView"));

    match &kind {
        ErrorKind::UnsupportedCapability { capability } => {
            assert_eq!(capability.capability_id(), "webView");
        }
        other => panic!("应构造出能力错误，实际是 {other:?}"),
    }

    // 分类标识仍是统一的 unsupported_capability（供上层做粗粒度映射）
    assert_eq!(kind.message().id(), "error.unsupported_capability");
}

/// 携带上下文的结构化错误：除分类标识外，还须有可注明细节的槽位。
///
/// 用于"网络错误 + 具体 URL""存储错误 + 表名"这类场景。细节**不是**用户文案，
/// 而是给开发者/日志的诊断信息。
#[test]
fn 错误可携带诊断细节() {
    let error = ErrorKind::Network.with_detail("GET https://example.com/x 超时");
    assert_eq!(error.message_id(), "error.network");
    assert_eq!(error.detail(), Some("GET https://example.com/x 超时"));
    assert_eq!(error.kind(), &ErrorKind::Network);

    // 不带细节时 detail() 为 None —— 分类本身仍是完整的错误
    let bare = ErrorKind::Content;
    assert_eq!(bare.message_id(), "error.content");
}

/// `ErrorKind` 实现 `std::error::Error` 与 `Display`，才能在各 crate 的
/// thiserror 类型里作 `#[from]` 目标，并被 `?` 顺畅传播。
#[test]
fn 错误类型可与标准错误生态互操作() {
    fn takes_std_error(_error: &dyn std::error::Error) {}

    let kind = ErrorKind::Storage.with_detail("pack 索引损坏");
    takes_std_error(&kind);

    // Display 输出的应是**标识**（可本地化标识，不是文案），便于日志检索
    let displayed = format!("{kind}");
    assert!(
        displayed.contains("error.storage"),
        "Display 应含分类标识，实际：{displayed}"
    );
}

/// `LocalizedMessage` 可独立构造——实现 crate 在需要自建消息时用得到。
#[test]
fn 可本地化消息可独立构造() {
    let message = LocalizedMessage::new("error.network.timeout");
    assert_eq!(message.id(), "error.network.timeout");
}

/// 能力标识的类型归属：它在 `error` 模块而非 `model`。
///
/// 归属理由：`ErrorCapability` 是**错误载荷**（FR-006），不是领域数据。
/// 放进 `model` 会让"领域实体"与"错误语义"混在一起，也让 `model` 的
/// 消费者被迫了解错误体系。
#[test]
fn 能力标识归属错误模块且可构造() {
    let capability = ErrorCapability::new("webView");
    assert_eq!(capability.capability_id(), "webView");

    // crate 根的 re-export 与 error:: 路径指向同一类型
    let via_root: tome_core::ErrorCapability = capability.clone();
    assert_eq!(via_root.capability_id(), "webView");
}
