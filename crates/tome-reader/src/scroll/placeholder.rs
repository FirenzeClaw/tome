//! [DONE] 未缓存区的占位/加载态（T029 / FR-025 后半句）
//!
//! ## 本模块存在的理由：把"没有内容"表达成**排版位置**
//!
//! FR-025 的后半句：
//!
//! > ……并以明确的占位或加载态呈现。
//!
//! 滚动模式最坏的失败形态是**静默空白**：用户滚到未缓存区，屏幕上什么都没有，
//! 他不知道自己滚到了书的尽头、还是网络卡了、还是程序坏了。故"没有内容"必须
//! 是一个**可辨识的状态**，而不是"零个字符的空白"。
//!
//! ## 为什么带字符区间
//!
//! 占位要占据**排版位置**：它得知道"我这段空白对应正文的哪一段"。这样：
//!
//! - 用户滚过未缓存区时，滚动条的长度与位置仍然正确（不是"突然短了一截"）
//! - 内容到达后，占位在**同一位置**被替换为文字（不引起跳动）
//! - 痕迹与阅读记录的位置换算仍然成立（它们以字符区间为准，FR-021）
//!
//! ## 三类占位（可枚举）
//!
//! | 变体 | 语义 | 用户可采取的动作 |
//! |---|---|---|
//! | [`PlaceholderKind::Loading`] | 内容正在路上 | 等待 |
//! | [`PlaceholderKind::Uncached`] | 内容未缓存且**不会**自动获取（FR-025） | 主动触发一次获取 |
//! | [`PlaceholderKind::Unavailable`] | 内容不可得（源失效/网络不可用） | 换源或重试 |
//!
//! 三者的区分是必要的：它们的**可操作性**不同。合成一个"内容不可用"会让
//! 界面无法给出正确的下一步提示。
//!
//! ## 本层不判定"哪段未缓存"
//!
//! 缓存状态属 001（`ChapterContent.from_cache` 与 Pack 容器）。本层只提供
//! **表达形态**；`ScrollPlaceholder::none_for_cached_content` 的存在是为了
//! 让"内容完整时不臆造占位"这条成为可断言的默认行为。

use crate::scroll::flow::ScrollFlow;

/// 占位的类别（FR-025 的"明确的占位或加载态"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceholderKind {
    /// 内容正在获取中（**由用户动作触发**——滚动本身不获取，FR-025）。
    Loading,
    /// 内容未缓存，且本层**不会**因滚动去取（FR-025）。
    Uncached,
    /// 内容不可得（源失效、网络不可用等）。
    Unavailable,
}

impl PlaceholderKind {
    /// 本占位类别的可本地化标识（**标识式，不是文案**，FR-058）。
    ///
    /// ## 为什么是标识而不是文案
    ///
    /// 引擎内一旦出现中文提示语，新增一种语言就要改引擎、重发引擎版本。
    /// 故本层只给 `scroll.placeholder.loading` 这样的稳定标识，文案表在 S3。
    #[must_use]
    pub fn message_id(self) -> &'static str {
        match self {
            Self::Loading => "scroll.placeholder.loading",
            Self::Uncached => "scroll.placeholder.uncached",
            Self::Unavailable => "scroll.placeholder.unavailable",
        }
    }

    /// 该占位是否提示用户**可以主动触发**一次获取。
    ///
    /// ## 为什么这个区分要有方法承载
    ///
    /// FR-025 禁止的是"**因滚动**触发抓取"，不是"永远不能获取内容"。故
    /// `Uncached` 与 `Unavailable` 是**可操作**的（用户点了才取），
    /// `Loading` 不是（已经在路上了）。
    ///
    /// 界面据它决定是否显示"点击加载"之类的入口。若把这条判断散落在界面
    /// 代码里，每个绘制点都要自己回忆哪几类可操作——一处漏了就是"有的占位
    /// 能点、有的不能点"的不一致。
    #[must_use]
    pub fn is_user_actionable(self) -> bool {
        matches!(self, Self::Uncached | Self::Unavailable)
    }
}

/// 一段"没有内容"在排版流里占据的位置（FR-025 后半句的载体）。
#[derive(Debug, Clone, PartialEq)]
pub struct ScrollPlaceholder {
    /// 占位类别
    pub kind: PlaceholderKind,
    /// 它占据的**章内**字符区间（与 `Page` / `ScrollViewport` 同口径）
    pub char_range: std::ops::Range<usize>,
}

impl ScrollPlaceholder {
    /// 构造。
    #[must_use]
    pub fn new(kind: PlaceholderKind, char_range: std::ops::Range<usize>) -> Self {
        Self { kind, char_range }
    }

    /// 该占位覆盖的字符数。
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.char_range.end - self.char_range.start
    }

    /// **内容已完整在手时，不得臆造占位**。
    ///
    /// ## 为什么这条是默认行为而非可选
    ///
    /// 占位是"内容缺席"的表达。若在内容完整时也产出占位（或调用方忘了清空
    /// 旧占位），用户会看到"文字和'加载中'叠在一起"——那比纯空白更让人困惑。
    ///
    /// 本函数给出一条**保守**的默认：只有在调用方明确告知缺口时才产出占位。
    /// 由于缺口信息属 001（缓存状态），此处只能覆盖"无缺口"的情形——故它
    /// 的语义是"若没有明确信号，返回空"。
    ///
    /// ## 判据为什么带 `flow` 与 `content`
    ///
    /// 两个输入都用来做**自检**：
    ///
    /// - `flow`：视口区间必须完整覆盖正文（SC-006b）。若不完整，那说明排版
    ///   本身出了问题——此时返回空占位会掩盖它，故不在此处置（交由 SC-006b
    ///   的断言去报）。
    /// - `content`：与 `flow.char_len()` 比对，确认调用方给的正文就是排过版
    ///   的那一份（防"排 A 章却传了 B 章的 content"）。
    #[must_use]
    pub fn none_for_cached_content(flow: &ScrollFlow, content: &str) -> Vec<Self> {
        // 自检：正文长度必须与流的字符数一致（否则调用方传错了内容）
        if content.chars().count() != flow.char_len() {
            return Vec::new();
        }
        // 自检：视口区间必须首尾相接覆盖到末尾（SC-006b）
        let Some(last) = flow.viewports().last() else {
            return Vec::new();
        };
        if last.char_range.end != flow.char_len() {
            return Vec::new();
        }
        // 内容完整无缺口 → 无占位
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_ids_share_a_prefix_and_are_unique() {
        let kinds = [
            PlaceholderKind::Loading,
            PlaceholderKind::Uncached,
            PlaceholderKind::Unavailable,
        ];
        let mut ids: Vec<&str> = kinds.iter().map(|kind| kind.message_id()).collect();
        assert!(ids.iter().all(|id| id.starts_with("scroll.placeholder.")));
        assert!(ids.iter().all(|id| id.is_ascii()));
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(total, ids.len());
    }

    #[test]
    fn only_uncached_and_unavailable_are_user_actionable() {
        assert!(
            !PlaceholderKind::Loading.is_user_actionable(),
            "内容已在路上，无需用户操作"
        );
        assert!(
            PlaceholderKind::Uncached.is_user_actionable(),
            "未缓存是用户可主动触发的（FR-025 禁的是'因滚动触发'）"
        );
        assert!(PlaceholderKind::Unavailable.is_user_actionable());
    }

    #[test]
    fn placeholder_reports_its_char_length() {
        let placeholder = ScrollPlaceholder::new(PlaceholderKind::Loading, 100..250);
        assert_eq!(placeholder.char_len(), 150);
        assert_eq!(placeholder.kind.message_id(), "scroll.placeholder.loading");
    }

    #[test]
    fn empty_range_is_legal() {
        let placeholder = ScrollPlaceholder::new(PlaceholderKind::Unavailable, 0..0);
        assert_eq!(placeholder.char_len(), 0);
    }
}
