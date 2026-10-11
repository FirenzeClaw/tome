//! T047 验收：键位集 + 栈式 Esc（FR-035）
//!
//! ## FR-035 的原文（拆成两组判据）
//!
//! > 系统 MUST 支持一组**确定的键位集**驱动阅读导航与工具栏唤出，并 MUST
//! > 按**栈式语义**处理 `Esc`：阅读区聚焦时 `Esc` 先退出沉浸态/隐藏工具栏；
//! > 当目录或设置以抽屉/弹层形态打开时（栈顶），`Esc` MUST 关闭该层。
//! > 键位集 MUST 显式成表且可逐项测试，至少覆盖：向前/向后翻页、上/下一章、
//! > 章节首/末、滚动模式下的行滚动与页滚动、工具栏唤出、`Esc` 的栈式取消。
//!
//! | # | 要求 | 本文件的测试 |
//! |---|---|---|
//! | 1 | 键位集**显式成表** | `keymap_is_an_explicit_table` |
//! | 2 | 逐项可测（八类动作全覆盖） | `every_required_action_has_a_binding` |
//! | 3 | `Esc` **栈式取消** | `esc_pops_the_topmost_layer_first` |
//! | 4 | 无层时 `Esc` 退出沉浸/隐藏工具栏 | `esc_hides_toolbar_when_stack_is_empty` |
//!
//! ## 为什么"显式成表"要单独断言
//!
//! "键位散在 `match` 分支里"与"键位是一张表"对用户观感的差别在于**可发现性**
//! ——设置页要能列出快捷键，帮助文档要能生成，冲突要能被检测。散在分支里的
//! 实现做这三件事都得把逻辑再抄一遍。故本模块把键位做成一个可枚举的常量表，
//! 并断言表与动作集合**一一对应**（不重不漏）。

use std::collections::BTreeSet;

use tome_reader::keys::{
    keymap, resolve_key, EscLayer, EscStack, KeyBinding, KeyEvent, ReadingAction, KEYMAP,
};

// ===========================================================================
// 1. 键位集显式成表
// ===========================================================================

/// **键位集是一张可枚举的表**（FR-035："MUST 显式成表"）。
#[test]
fn keymap_is_an_explicit_table() {
    let table = keymap();
    assert!(!table.is_empty(), "键位表不得为空（FR-035）");
    // 表是可枚举的静态数据（可被设置页列出、可被生成帮助文本）
    assert_eq!(table.len(), KEYMAP.len(), "keymap() 应与 KEYMAP 常量同源");
}

/// **表内无重复键**（同一个键不得映射到两个动作）。
///
/// ## 为什么这条必须测
///
/// 重复键在 `match` 式实现里表现为"后一个分支永远不生效"——用户按了没反应，
/// 而代码看起来两条都在。表式实现若不去重，同样会有这个问题，且更难发现
/// （表大了之后人眼扫不出来）。
#[test]
fn keymap_has_no_duplicate_bindings() {
    let mut seen: Vec<KeyEvent> = Vec::new();
    for binding in KEYMAP {
        assert!(
            !seen.contains(&binding.key),
            "键 {:?} 被绑定了多次（表内不得有重复键——见 KEYMAP 文档：本层不复用键）",
            binding.key
        );
        seen.push(binding.key);
    }
}

/// **每个绑定都有非空的说明**（供设置页与帮助文本直接使用）。
#[test]
fn every_binding_has_a_description() {
    for binding in KEYMAP {
        assert!(
            !binding.action.as_str().is_empty(),
            "动作 {:?} 缺少稳定标识",
            binding.action
        );
    }
}

// ===========================================================================
// 2. 八类动作全覆盖
// ===========================================================================

/// **FR-035 列举的八类动作全部有绑定**（逐项可测）。
///
/// ## 八类怎么定的
///
/// 直接照 FR-035 的措辞拆：向前/向后翻页（2）、上/下一章（2）、章节首/末（2）、
/// 滚动模式下的行滚动与页滚动（4：上/下 × 行/页）、工具栏唤出（1）、
/// `Esc` 栈式取消（1）。共 12 个动作。
#[test]
fn every_required_action_has_a_binding() {
    let required = [
        // 向前/向后翻页
        ReadingAction::PageNext,
        ReadingAction::PagePrev,
        // 上/下一章
        ReadingAction::ChapterNext,
        ReadingAction::ChapterPrev,
        // 章节首/末
        ReadingAction::ChapterStart,
        ReadingAction::ChapterEnd,
        // 滚动模式：行滚动
        ReadingAction::ScrollLineDown,
        ReadingAction::ScrollLineUp,
        // 滚动模式：页滚动
        ReadingAction::ScrollPageDown,
        ReadingAction::ScrollPageUp,
        // 工具栏唤出
        ReadingAction::RevealToolbar,
        // Esc 的栈式取消
        ReadingAction::Esc,
    ];

    for action in required {
        assert!(
            KEYMAP.iter().any(|binding| binding.action == action),
            "动作 `{}` 缺少键位绑定（FR-035 要求逐项可测）",
            action.as_str()
        );
    }
}

/// **动作集合与键位表一一对应**（表里没有"孤儿动作"）。
#[test]
fn actions_and_bindings_are_in_bijection() {
    let bound: BTreeSet<&str> = KEYMAP
        .iter()
        .map(|binding| binding.action.as_str())
        .collect();
    let required: BTreeSet<&str> = ReadingAction::ALL
        .iter()
        .map(|action| action.as_str())
        .collect();
    assert_eq!(
        bound, required,
        "键位表覆盖的动作应与 ReadingAction::ALL 完全一致（不重不漏）"
    );
}

// ===========================================================================
// 3. 逐项解析
// ===========================================================================

/// **每个绑定都能被解析回它的动作**（表与解析器一致）。
#[test]
fn every_binding_resolves_back_to_its_action() {
    for binding in KEYMAP {
        let resolved = resolve_key(binding.key);
        assert_eq!(
            resolved,
            Some(binding.action),
            "键 {:?} 应解析为 `{}`",
            binding.key,
            binding.action.as_str()
        );
    }
}

/// **具体键位符合桌面惯例**（逐个断言，防无声改动）。
#[test]
fn key_bindings_follow_desktop_conventions() {
    let expect = |key: KeyEvent, action: ReadingAction, label: &str| {
        assert_eq!(
            resolve_key(key),
            Some(action),
            "{label} 应绑定到 `{}`",
            action.as_str()
        );
    };

    // 翻页：PageUp/PageDown（桌面阅读器的既定惯例）
    expect(KeyEvent::PageDown, ReadingAction::PageNext, "PageDown");
    expect(KeyEvent::PageUp, ReadingAction::PagePrev, "PageUp");
    // 章节：方向键 + 修饰（Ctrl/Alt 在 Mac/Linux/Windows 上不统一，故用方向键）
    expect(
        KeyEvent::ArrowRight,
        ReadingAction::ChapterNext,
        "ArrowRight",
    );
    expect(KeyEvent::ArrowLeft, ReadingAction::ChapterPrev, "ArrowLeft");
    // 章节首末：Home / End
    expect(KeyEvent::Home, ReadingAction::ChapterStart, "Home");
    expect(KeyEvent::End, ReadingAction::ChapterEnd, "End");
    // 滚动：方向键上下（行）
    expect(
        KeyEvent::ArrowDown,
        ReadingAction::ScrollLineDown,
        "ArrowDown",
    );
    expect(KeyEvent::ArrowUp, ReadingAction::ScrollLineUp, "ArrowUp");
    // 滚动：空格/退格（页）——不与 PageUp/PageDown 复用，见 KEYMAP 文档
    expect(KeyEvent::Space, ReadingAction::ScrollPageDown, "Space");
    expect(
        KeyEvent::Backspace,
        ReadingAction::ScrollPageUp,
        "Backspace",
    );
    // 工具栏唤出：Tab（沉浸态下唤出控件的惯例）
    expect(KeyEvent::Tab, ReadingAction::RevealToolbar, "Tab");
    // Esc
    expect(KeyEvent::Escape, ReadingAction::Esc, "Escape");
}

/// **未绑定的键解析为 `None`**（不静默吞掉）。
#[test]
fn unbound_keys_resolve_to_none() {
    assert_eq!(
        resolve_key(KeyEvent::Character('q')),
        None,
        "未绑定的键应返回 None（让外壳可以继续处理它）"
    );
}

// ===========================================================================
// 4. Esc 栈式语义
// ===========================================================================

/// **栈顶是普通层时 `Esc` 先关闭它**（FR-035 的栈式语义核心）。
#[test]
fn esc_pops_the_topmost_layer_first() {
    let mut stack = EscStack::new();
    // 打开目录抽屉（栈顶普通层）
    stack.push(EscStack::TOC_DRAWER);
    // 再打开设置弹层（更上一层）
    stack.push(EscStack::SETTINGS_MODAL);

    // 第一次 Esc：关掉**设置**（栈顶）
    let first = stack.handle_escape();
    assert_eq!(
        first,
        Some(EscStack::SETTINGS_MODAL),
        "Esc 应先关栈顶的设置层"
    );
    assert_eq!(stack.depth(), 1, "深度应减一");

    // 第二次 Esc：关掉目录
    let second = stack.handle_escape();
    assert_eq!(second, Some(EscStack::TOC_DRAWER), "再次 Esc 应关掉目录层");
    assert!(stack.is_empty(), "两层关完后栈应空");
}

/// **栈空时 `Esc` 退出沉浸态/隐藏工具栏**（FR-035 明文）。
#[test]
fn esc_hides_toolbar_when_stack_is_empty() {
    let mut stack = EscStack::new();
    assert!(stack.is_empty(), "前置：栈为空");

    let result = stack.handle_escape();
    assert_eq!(
        result,
        Some(EscLayer::Immersion),
        "栈空时 Esc 应作用于沉浸态/工具栏（而非无事发生）"
    );
    // 且这次不改变栈深（沉浸态不是一个"层"）
    assert!(stack.is_empty(), "退出沉浸态不改变栈深");
}

/// **栈式语义的完整三层场景**（本任务验收子项的"两层场景"的扩展）。
#[test]
fn esc_is_strictly_last_in_first_out() {
    let mut stack = EscStack::new();
    stack.push(EscStack::TOC_DRAWER);
    stack.push(EscStack::BOOKMARK_PANEL);
    stack.push(EscStack::SETTINGS_MODAL);

    assert_eq!(stack.handle_escape(), Some(EscStack::SETTINGS_MODAL));
    assert_eq!(stack.handle_escape(), Some(EscStack::BOOKMARK_PANEL));
    assert_eq!(stack.handle_escape(), Some(EscStack::TOC_DRAWER));
    // 三层关完，第四次 Esc 落到沉浸态
    assert_eq!(stack.handle_escape(), Some(EscLayer::Immersion));
    assert!(stack.is_empty());
}

/// **`pop` 可单独使用**（外壳在层自己关闭时同步栈）。
#[test]
fn stack_can_be_popped_directly() {
    let mut stack = EscStack::new();
    stack.push(EscStack::TOC_DRAWER);
    assert_eq!(stack.pop(), Some(EscStack::TOC_DRAWER));
    assert_eq!(stack.pop(), None, "空栈 pop 应返回 None");
    assert!(stack.is_empty());
}

/// **栈可查询顶层**（外壳据此决定显示哪一层）。
#[test]
fn stack_exposes_the_top_layer() {
    let mut stack = EscStack::new();
    assert_eq!(stack.top(), None, "空栈顶层为 None");
    stack.push(EscStack::TOC_DRAWER);
    assert_eq!(stack.top(), Some(EscStack::TOC_DRAWER));
    stack.push(EscStack::SETTINGS_MODAL);
    assert_eq!(
        stack.top(),
        Some(EscStack::SETTINGS_MODAL),
        "顶层应是最新压入的"
    );
}

/// **重复压入同一层不产生未定义行为**（幂等或拒绝，二选一但必须明确）。
#[test]
fn pushing_the_same_layer_twice_is_defined() {
    let mut stack = EscStack::new();
    stack.push(EscStack::TOC_DRAWER);
    stack.push(EscStack::TOC_DRAWER);
    // 无论实现选择"拒绝重复"还是"允许重复"，都必须能被确定地断言
    let depth = stack.depth();
    assert!(
        depth == 1 || depth == 2,
        "重复压入应有确定行为（去重或允许），实测深度 {depth}"
    );
    // 连续 Esc 必须把栈清空（不得卡住）
    let mut pops = 0;
    while stack.handle_escape() != Some(EscLayer::Immersion) {
        pops += 1;
        assert!(pops < 10, "Esc 应能在有限步内清空栈");
    }
    assert!(stack.is_empty());
}

// ===========================================================================
// 5. 表与栈的接口形态
// ===========================================================================

/// **`KeyBinding` 是可公开消费的纯值**（供设置页列出）。
#[test]
fn key_binding_is_a_public_value() {
    let binding = KeyBinding {
        key: KeyEvent::PageDown,
        action: ReadingAction::PageNext,
    };
    assert_eq!(binding.action.as_str(), "pageNext");
    assert_eq!(resolve_key(binding.key), Some(binding.action));
}

/// **`ReadingAction` 的稳定标识是驼峰**（FR-058：非用户文案）。
#[test]
fn action_identifiers_are_stable_and_not_localized() {
    for action in ReadingAction::ALL {
        let id = action.as_str();
        assert!(
            !id.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
            "动作标识不得含中文（本地化留给 S3），实测：{id}"
        );
        assert!(
            id.chars().next().is_some_and(char::is_lowercase),
            "动作标识应是驼峰式稳定标识，实测：{id}"
        );
    }
}
