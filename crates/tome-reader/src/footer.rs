//! \[DONE] 页脚信息（T045 / FR-042）
//!
//! # 本模块算什么
//!
//! 页脚要展示四样东西：当前章号与章名、**章内**页码、剩余阅读时间。
//! 前两样是**搬运**（从 `PaginationResult` / 目录取），第三样是**估算**
//! （需要阅读记录）。本模块把三者算成一份 [`FooterInfo`] 供外壳渲染。
//!
//! ## FR-042 的三条纪律
//!
//! 1. **章号统一为当前章**：`chapter_index` 与 `chapter_title` 从同一处
//!    输入来，故"页脚说第 3 章、顶栏说第 4 章"不可能发生——它们会是同一个
//!    字段的两种渲染。
//! 2. **页码是章内页码**：`page_in_chapter` 而非全书累计页码。全书页码在
//!    本规格里"不作为需求"——更重要的是它**不可靠**（章节缓存是增量的，
//!    未缓存章节的页数未知，算不出全书页码）。
//! 3. **无依据就不显示**：`remaining_seconds: Option<f64>`，`None` 表示
//!    该隐藏。**MUST NOT** 回落到"假设一个平均速度"——那是凭空估算。
//!
//! ## 剩余时间的口径（FR-042 明文）
//!
//! > 当前章剩余字符数 ÷ 近期阅读速度（基于 001 §FR-026b 的阅读记录计算）
//!
//! 三个词各有落点：
//!
//! - **当前章剩余字符数** = `chapter_char_len - chapter_char_offset`
//!   （钳到 `[0, chapter_char_len]`）
//! - **近期**：只取**最近若干天**的记录（见 [`RECENT_WINDOW`]），不用
//!   全程平均——一个月前的慢速记录不该永久拖长今天的估算
//! - **阅读速度** = 窗口内 `read_chars` 合计 ÷ `read_seconds` 合计
//!   （字符/秒）
//!
//! ## 为什么"速度为零"要隐藏而不是"显示 0 分钟"
//!
//! 速度为 0 意味着"读了这么久一个字没读"——它推不出任何有限时间
//! （数学上是无穷）。显示"0 分钟"或"∞"都是在编造。隐藏才是诚实的。

use tome_core::model::ReadRecord;

/// 参与速度估算的**近期窗口**（取最后 N 条记录）。
///
/// ## 为什么是"条数"而不是"天数"
///
/// 记录按天聚合（001 `read_record` 的主键是 `(book_id, date)`），故"条数"
/// 与"天数"在本数据上等价。取条数让本模块不必做日期解析——日期是字符串，
/// 解析它要引时间库，而"最后 N 条"用切片就够了。
///
/// ## 为什么是 7
///
/// 一周是"近期"的自然粒度：短于它会被单次长会话主导，长于它会被已经
/// 遗忘的阅读习惯拖累。这个值在测试里被间接固定（`speed_reflects_recent`
/// 用 2 条记录验证"取近期"这一性质本身，与具体的 7 无关）。
const RECENT_WINDOW: usize = 7;

/// 推算速度所需的最小**时间**样本（秒）。
///
/// 2 秒的记录推不出速度（误差极大）。30 秒是"至少读了半分钟"的下限，
/// 低于它的记录不足以让"字符/秒"有意义。
const MIN_SECONDS_FOR_SPEED: i64 = 30;

/// 页脚的输入（上层从分页结果与阅读记录组装）。
#[derive(Debug, Clone, PartialEq)]
pub struct FooterInput {
    /// 当前章序号（0 起，与 001 锚点同口径）
    pub chapter_index: usize,
    /// 当前章名
    pub chapter_title: String,
    /// 当前章的**字符总数**
    pub chapter_char_len: usize,
    /// 当前阅读位置在本章的字符偏移
    pub chapter_char_offset: usize,
    /// 当前页的**章内**页码（0 起）
    pub page_in_chapter: u32,
    /// 当前章的总页数
    pub page_count_in_chapter: u32,
    /// 近期阅读记录（顺序不重要，本模块只按总量算；空表示无数据）
    pub recent_records: Vec<ReadRecord>,
}

/// 算好的页脚信息（供外壳直接渲染）。
///
/// ## 为什么同时给"内部口径"与"显示口径"
///
/// `chapter_index` 是 0 起（与 001 锚点一致），而人类看到的章号是 1 起。
/// 两者都给出，且都在这里算好——若让外壳自己 `+1`，某天就会有一处忘了加。
#[derive(Debug, Clone, PartialEq)]
pub struct FooterInfo {
    /// 当前章序号（0 起，内部口径）
    pub chapter_index: usize,
    /// 显示用章号（1 起）
    pub display_chapter_number: usize,
    /// 当前章名
    pub chapter_title: String,
    /// 显示用章内页码（1 起）
    pub display_page_number: u32,
    /// 章内总页数
    pub page_count_in_chapter: u32,
    /// 剩余阅读秒数；`None` 表示**隐藏该项**（数据不足）
    pub remaining_seconds: Option<f64>,
}

impl FooterInfo {
    /// 组装页脚信息（纯计算）。
    ///
    /// ## 为什么不返回 `Result`
    ///
    /// 本函数没有可失败的前置条件：所有输入都是值（章名可以是空串、偏移
    /// 可以越界、记录可以是空的），且每种情况都有确定处置（钳制 / 隐藏）。
    /// 返回 `Result` 会逼调用方处理一个永不发生的错误分支。
    #[must_use]
    pub fn build(input: &FooterInput) -> Self {
        let remaining_chars = input
            .chapter_char_len
            .saturating_sub(input.chapter_char_offset);

        Self {
            chapter_index: input.chapter_index,
            display_chapter_number: input.chapter_index.saturating_add(1),
            chapter_title: input.chapter_title.clone(),
            display_page_number: input.page_in_chapter.saturating_add(1),
            page_count_in_chapter: input.page_count_in_chapter,
            remaining_seconds: estimate_remaining_seconds(remaining_chars, &input.recent_records),
        }
    }

    /// 剩余分钟（四舍五入）；无估算时为 `None`。
    ///
    /// ## 为什么给整数分钟而不是秒
    ///
    /// 秒级精度是**假精度**：速度本身是从几天的记录里估的，误差远大于一秒。
    /// 给"约 10 分钟"是诚实的，给"9 分 47 秒"不是。返回 `Option<u64>` 让
    /// 调用方无法绕过"无数据则无值"。
    #[must_use]
    pub fn remaining_minutes(&self) -> Option<u64> {
        self.remaining_seconds
            .map(|seconds| (seconds / 60.0).round() as u64)
    }
}

/// 估算剩余阅读秒数；数据不足时返回 `None`（**不编造**）。
///
/// ## 判定顺序（每一步都是"隐藏"的独立理由）
///
/// 1. 无记录 → `None`
/// 2. 窗口内合计秒数 < [`MIN_SECONDS_FOR_SPEED`] → `None`
/// 3. 窗口内合计字符数 ≤ 0（速度为 0） → `None`
///
/// 三者通过后：`剩余字符 ÷ 速度`。剩余为零时返回 `Some(0.0)`——那是有据的
/// 确定值（读完了），不是"数据不足"。
fn estimate_remaining_seconds(remaining_chars: usize, records: &[ReadRecord]) -> Option<f64> {
    if records.is_empty() {
        return None;
    }

    // 取最后 N 条（`recent_records` 按日期升序时，末尾即最近）
    let window = if records.len() > RECENT_WINDOW {
        &records[records.len() - RECENT_WINDOW..]
    } else {
        records
    };

    let total_seconds: i64 = window.iter().map(|record| record.read_seconds.max(0)).sum();
    let total_chars: i64 = window.iter().map(|record| record.read_chars.max(0)).sum();

    if total_seconds < MIN_SECONDS_FOR_SPEED {
        return None;
    }
    if total_chars <= 0 {
        return None;
    }

    let chars_per_second = total_chars as f64 / total_seconds as f64;
    Some(remaining_chars as f64 / chars_per_second)
}
