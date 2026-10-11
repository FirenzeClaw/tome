//! `[DONE]` 键位集与栈式 `Esc`（T047 / FR-035）
//!
//! # 本模块的两样东西
//!
//! 1. **一张显式的键位表**（[`KEYMAP`]）——FR-035 要求"MUST 显式成表且可
//!    逐项测试"。表是可枚举的静态数据，故设置页能列出、帮助文本能生成、
//!    冲突能检测，不必把逻辑再抄一遍。
//! 2. **一个 `Esc` 栈**（[`EscStack`]）——FR-035 要求"栈式语义"。
//!
//! ## 为什么键位是"表 + 解析函数"而不是一个大 `match`
//!
//! `match event { Key::PageDown => PageNext, ... }` 能实现同样的功能，但它
//! 把**键位定义**藏进了控制流：要回答"翻页是哪个键"必须读代码；要列出
//! 全部快捷键必须把分支抄一遍。表式实现让"键位集"本身是一个可查询的值。
//!
//! 这一点在桌面端尤其重要：快捷键进设置页、进帮助、被用户重绑，都是常规
//! 需求（FR-035 的"可逐项测试"是它们的最基础形态）。
//!
//! ## `Esc` 为什么必须是栈而不是一个 `if`
//!
//! 用户在"打开设置弹层 → 又打开目录抽屉"之后按 `Esc`，期望关掉**最后打开
//! 的那一层**。用一个布尔量记"有没有弹层"无法表达顺序，结果是关错层——
//! 而这类 bug 只在嵌套场景下出现，手工测试很容易漏掉。
//!
//! ## 本层不分发动作
//!
//! `resolve_key` 只回答"这个键对应哪个动作"，**不执行**它。动作的执行需要
//! 分页器/滚动流等状态，那是外壳（或未来的阅读会话类型）的职责。保持本层
//! 纯函数让键位与动作的对应关系可被独立测试——本模块的全部断言都建立在这条上。

use std::collections::BTreeSet;

/// 一个可被绑定的按键（**不含修饰键**）。
///
/// ## 为什么不建模修饰键
///
/// FR-035 列举的动作都有单键绑定（翻页用 `PageUp`/`PageDown`、章节用方向键、
/// 首末用 `Home`/`End`）。引入 `Ctrl+S` 这类组合键会让"哪个键绑定到哪个
/// 动作"依赖平台惯例（macOS 用 `Cmd`、Windows 用 `Ctrl`），而三平台一致性
/// （FR-055）要求键位表本身跨平台确定。故本层的键位是**裸键**——将来若确实
/// 需要组合键，`KeyEvent` 加字段即可，表结构不必变。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyEvent {
    /// 上方向键
    ArrowUp,
    /// 下方向键
    ArrowDown,
    /// 左方向键
    ArrowLeft,
    /// 右方向键
    ArrowRight,
    /// PageUp
    PageUp,
    /// PageDown
    PageDown,
    /// Home
    Home,
    /// End
    End,
    /// Page 键以外的翻页用键：空格（阅读器惯例的"下一屏"）
    Space,
    /// 退格键（滚动模式的"上一屏"）
    Backspace,
    /// 制表键（沉浸态下唤出控件的惯例）
    Tab,
    /// `Esc`
    Escape,
    /// 一个普通字符键（仅用于"未绑定"的判定，不参与键位表）
    Character(char),
}

/// 阅读导航动作（FR-035 列举的全部动作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReadingAction {
    /// 向后翻页（翻页模式）
    PageNext,
    /// 向前翻页
    PagePrev,
    /// 下一章
    ChapterNext,
    /// 上一章
    ChapterPrev,
    /// 章节首
    ChapterStart,
    /// 章节末
    ChapterEnd,
    /// 滚动模式下向下滚一行
    ScrollLineDown,
    /// 滚动模式下向上滚一行
    ScrollLineUp,
    /// 滚动模式下向下滚一屏
    ScrollPageDown,
    /// 滚动模式下向上滚一屏
    ScrollPageUp,
    /// 唤出阅读工具栏
    RevealToolbar,
    /// `Esc` 的栈式取消
    Esc,
}

impl ReadingAction {
    /// 全部动作（枚举视图，供键位表完整性断言）。
    pub const ALL: [Self; 12] = [
        Self::PageNext,
        Self::PagePrev,
        Self::ChapterNext,
        Self::ChapterPrev,
        Self::ChapterStart,
        Self::ChapterEnd,
        Self::ScrollLineDown,
        Self::ScrollLineUp,
        Self::ScrollPageDown,
        Self::ScrollPageUp,
        Self::RevealToolbar,
        Self::Esc,
    ];

    /// 稳定标识（驼峰，**不是**面向用户的文案；FR-058）。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PageNext => "pageNext",
            Self::PagePrev => "pagePrev",
            Self::ChapterNext => "chapterNext",
            Self::ChapterPrev => "chapterPrev",
            Self::ChapterStart => "chapterStart",
            Self::ChapterEnd => "chapterEnd",
            Self::ScrollLineDown => "scrollLineDown",
            Self::ScrollLineUp => "scrollLineUp",
            Self::ScrollPageDown => "scrollPageDown",
            Self::ScrollPageUp => "scrollPageUp",
            Self::RevealToolbar => "revealToolbar",
            Self::Esc => "escape",
        }
    }
}

impl std::fmt::Display for ReadingAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 一条键位绑定（键 → 动作）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    /// 键
    pub key: KeyEvent,
    /// 该键触发的动作
    pub action: ReadingAction,
}

/// **键位表**（FR-035：MUST 显式成表且可逐项测试）。
///
/// ## 键位的选择依据（桌面惯例）
///
/// | 动作 | 键 | 依据 |
/// |---|---|---|
/// | 翻页 | `PageUp` / `PageDown` | 桌面阅读器与浏览器的既定惯例 |
/// | 章节 | `←` / `→` | 章节是比页更大的单位，用水平方向键；避免与滚动模式的上下键冲突 |
/// | 章节首/末 | `Home` / `End` | 文本编辑的既定惯例 |
/// | 行滚动 | `↑` / `↓` | 滚动模式的自然映射 |
/// | 页滚动 | `Space` / `Backspace` | 阅读器惯例的"下一屏 / 上一屏"；不与 `PageUp`/`PageDown` 复用，见下 |
/// | 唤出工具栏 | `Tab` | 沉浸态唤出控件的惯例 |
/// | 取消 | `Esc` | 唯一选择 |
///
/// ## 为什么页滚动**不**复用 `PageUp` / `PageDown`
///
/// 复用是常见的做法（"同一个键在不同形态下语义不同"），但它要求解析器
/// **知道当前形态**才能给出正确动作——于是 `resolve_key(key)` 不再是一个
/// 纯函数，而"键位表"也不足以回答"按 PageDown 会发生什么"。
///
/// 本层选择**不复用**：`PageDown` 恒为 `PageNext`、`Space` 恒为
/// `ScrollPageDown`。外壳按当前形态只处理相应的那组动作（翻页模式忽略滚动
/// 动作、反之亦然），键位表本身保持形态无关。代价是两套键，收益是
/// "键 → 动作"是纯函数、表可完整回答"这个键做什么"。
///
/// 这同时让"表 == `ReadingAction::ALL`"（一一对应）成立——一个键一个动作，
/// 一个动作一个键，`resolve_key` 有唯一答案。
pub const KEYMAP: &[KeyBinding] = &[
    // --- 翻页 ---
    KeyBinding {
        key: KeyEvent::PageDown,
        action: ReadingAction::PageNext,
    },
    KeyBinding {
        key: KeyEvent::PageUp,
        action: ReadingAction::PagePrev,
    },
    // --- 章节 ---
    KeyBinding {
        key: KeyEvent::ArrowRight,
        action: ReadingAction::ChapterNext,
    },
    KeyBinding {
        key: KeyEvent::ArrowLeft,
        action: ReadingAction::ChapterPrev,
    },
    KeyBinding {
        key: KeyEvent::Home,
        action: ReadingAction::ChapterStart,
    },
    KeyBinding {
        key: KeyEvent::End,
        action: ReadingAction::ChapterEnd,
    },
    // --- 滚动：行 ---
    KeyBinding {
        key: KeyEvent::ArrowDown,
        action: ReadingAction::ScrollLineDown,
    },
    KeyBinding {
        key: KeyEvent::ArrowUp,
        action: ReadingAction::ScrollLineUp,
    },
    // --- 滚动：页 ---
    KeyBinding {
        key: KeyEvent::Space,
        action: ReadingAction::ScrollPageDown,
    },
    KeyBinding {
        key: KeyEvent::Backspace,
        action: ReadingAction::ScrollPageUp,
    },
    // --- 工具栏 ---
    KeyBinding {
        key: KeyEvent::Tab,
        action: ReadingAction::RevealToolbar,
    },
    // --- 取消 ---
    KeyBinding {
        key: KeyEvent::Escape,
        action: ReadingAction::Esc,
    },
];

/// 取键位表（可枚举，供设置页与帮助文本使用）。
#[must_use]
pub const fn keymap() -> &'static [KeyBinding] {
    KEYMAP
}

/// 解析一个按键为动作；未绑定的键返回 `None`。
///
/// ## 为什么未绑定返回 `None` 而不是"无操作"动作
///
/// `None` 让外壳知道"这个键不归我管"，于是它可以把事件继续往上传递
/// （例如 `Space` 可能被外壳用于播放控制）。返回一个 `ReadingAction::None`
/// 会吞掉事件——那正是"某个键在阅读区里突然失灵"的成因。
#[must_use]
pub fn resolve_key(key: KeyEvent) -> Option<ReadingAction> {
    KEYMAP
        .iter()
        .find(|binding| binding.key == key)
        .map(|binding| binding.action)
}

// ===========================================================================
// Esc 栈
// ===========================================================================

/// `Esc` 栈上的一层。
///
/// ## 为什么"沉浸态"也在枚举里
///
/// FR-035 的语义是：栈非空时 `Esc` 关栈顶；栈空时 `Esc` 作用于沉浸态。
/// 把沉浸态也表达为枚举的一个变体，让"`Esc` 做了什么"永远是**同一个类型
/// 的返回值**——调用方写一次 `match` 就覆盖全部情形，不必"先查栈再决定
/// 返回什么类型"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EscLayer {
    /// 目录抽屉
    TocDrawer,
    /// 设置弹层
    SettingsModal,
    /// 书签面板
    BookmarkPanel,
    /// 换源面板
    ChangeSourcePanel,
    /// 阅读进度面板
    ProgressPanel,
    /// 沉浸态 / 工具栏（栈空时的落点）
    Immersion,
}

/// `Esc` 的**栈式取消**（FR-035 / 裁决 OQ#21→A）。
///
/// ## 语义
///
/// | 栈状态 | `handle_escape()` 的行为 |
/// |---|---|
/// | 非空 | 弹出栈顶并返回它 |
/// | 空 | 返回 [`EscLayer::Immersion`]（退出沉浸态/隐藏工具栏），栈不变 |
///
/// ## 为什么要处理"重复压入同一层"
///
/// 外壳可能在层已被打开时又收到一次打开请求（例如用户重复点了目录按钮）。
/// 若实现允许无限重复压入，`Esc` 就要按很多次才关得掉——用户会以为
/// `Esc` 坏了。本实现**去重**（同层已在栈中则不重复压入），让"按一次 `Esc`
/// 关一层"始终成立。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EscStack {
    layers: Vec<EscLayer>,
}

impl EscStack {
    /// 目录抽屉层标识（便捷常量）。
    pub const TOC_DRAWER: EscLayer = EscLayer::TocDrawer;
    /// 设置弹层层标识（便捷常量）。
    pub const SETTINGS_MODAL: EscLayer = EscLayer::SettingsModal;
    /// 书签面板层标识（便捷常量）。
    pub const BOOKMARK_PANEL: EscLayer = EscLayer::BookmarkPanel;

    /// 新建空栈。
    #[must_use]
    pub fn new() -> Self {
        Self { layers: Vec::new() }
    }

    /// 压入一层（**去重**：已在栈中则不重复压入）。
    ///
    /// 去重的理由见本类型的文档（重复压入会让 `Esc` 需要按多次）。
    pub fn push(&mut self, layer: EscLayer) {
        if !self.layers.contains(&layer) {
            self.layers.push(layer);
        }
    }

    /// 弹出栈顶。
    pub fn pop(&mut self) -> Option<EscLayer> {
        self.layers.pop()
    }

    /// 处理一次 `Esc`：非空则弹栈顶，空则返回沉浸态。
    ///
    /// ## 返回值总是 `Some`
    ///
    /// `Esc` 永远有确定的作用（要么关一层、要么退沉浸态），故返回
    /// `Option` 是多余的。这里返回 `Option` 是为了与 [`Self::pop`] 的
    /// 形态一致，且为将来"某些状态下 `Esc` 确实无事可做"留出表达空间。
    /// 当前实现恒为 `Some`。
    pub fn handle_escape(&mut self) -> Option<EscLayer> {
        match self.layers.pop() {
            Some(layer) => Some(layer),
            None => Some(EscLayer::Immersion),
        }
    }

    /// 栈顶层（不弹出）。
    #[must_use]
    pub fn top(&self) -> Option<EscLayer> {
        self.layers.last().copied()
    }

    /// 当前深度。
    #[must_use]
    pub fn depth(&self) -> usize {
        self.layers.len()
    }

    /// 是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// 栈内全部层（自底向上；供诊断与测试）。
    #[must_use]
    pub fn layers(&self) -> &[EscLayer] {
        &self.layers
    }

    /// 栈内层集合（供断言"某层是否已打开"）。
    #[must_use]
    pub fn layer_set(&self) -> BTreeSet<EscLayer> {
        self.layers.iter().copied().collect()
    }
}
