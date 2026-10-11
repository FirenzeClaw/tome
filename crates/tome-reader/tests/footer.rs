//! T045 验收：页脚信息（FR-042）
//!
//! ## FR-042 的原文（拆成三条判据）
//!
//! > 系统 MUST 支持**页脚信息呈现**，含当前章号与章名、章内页码的位置、
//! > 以及**剩余阅读时间估算**。剩余时间的口径 MUST 为：当前章剩余字符数 ÷
//! > 近期阅读速度（基于 001 §FR-026b 的阅读记录计算）；**无足够数据时该项
//! > MUST 隐藏**，MUST NOT 显示无依据的估算值。章号 MUST 统一为当前章
//! > （不得出现页脚与顶栏章号不一致）。页脚页码的语义 MUST 明确为**章内页码**
//! > （全书页码不作为需求）。
//!
//! | # | 判据 | 本文件的测试 |
//! |---|---|---|
//! | 1 | 章号统一为当前章 | `chapter_number_is_the_current_chapter` |
//! | 2 | 页码是**章内**页码 | `page_number_is_within_chapter` |
//! | 3 | 剩余时间 = 剩余字符 ÷ 近期速度 | `remaining_time_uses_recent_speed` |
//! | 4 | 无足够数据**隐藏**该项（不出无依据估算） | `hidden_when_data_insufficient` |
//!
//! ## "无足够数据"的三重含义
//!
//! 缺一不可地检查：
//!
//! 1. **没有记录** → 隐藏
//! 2. **记录太少**（如只有 5 秒，不足以推出速度） → 隐藏
//! 3. **速度为零或负**（记录里 `read_chars == 0`） → 隐藏
//!
//! 三者任一都不得让页脚显示一个凭空的"约 X 分钟"。

use tome_core::model::ReadRecord;
use tome_reader::footer::{FooterInfo, FooterInput};

// ===========================================================================
// 夹具
// ===========================================================================

fn record(date: &str, seconds: i64, chars: i64) -> ReadRecord {
    ReadRecord {
        book_id: 1,
        date: date.to_string(),
        read_seconds: seconds,
        read_chars: chars,
    }
}

/// 一份典型的输入：第 3 章（书名与章名都给好），本章共 5000 字符，
/// 已在页内读到第 2000 字符，本章第 5 页（章内页码 4，0 起）。
fn sample_input(records: Vec<ReadRecord>) -> FooterInput {
    FooterInput {
        chapter_index: 3,
        chapter_title: "第三章 · 雨夜".to_string(),
        chapter_char_len: 5000,
        chapter_char_offset: 2000,
        page_in_chapter: 4,
        page_count_in_chapter: 12,
        recent_records: records,
    }
}

// ===========================================================================
// 1. 章号统一为当前章
// ===========================================================================

/// **章号是当前章**（FR-042 明文：不得与顶栏章号不一致）。
///
/// ## 判据为什么带"章名"
///
/// 章号与章名必须来自**同一处输入**（同一个 `chapter_index` /
/// `chapter_title`）。若实现分别从两处取，就会出现"页脚写第 3 章、顶栏
/// 写第 4 章"这种不一致——而它的表现是用户看到两个不同的章号，
/// 排查时却找不到哪一处是对的。
#[test]
fn chapter_number_is_the_current_chapter() {
    let info = FooterInfo::build(&sample_input(vec![]));

    assert_eq!(
        info.chapter_index, 3,
        "章号应是当前章（0 起，与 001 口径一致）"
    );
    // 面向用户的章号从 1 起（人类习惯）；0 起是内部口径
    assert_eq!(
        info.display_chapter_number, 4,
        "显示章号应是 1 起的第 4 章（内部 3 + 1）"
    );
    assert_eq!(info.chapter_title, "第三章 · 雨夜", "章名应是当前章名");
}

// ===========================================================================
// 2. 页码是章内页码
// ===========================================================================

/// **页码是章内页码**（FR-042：全书页码不作为需求）。
#[test]
fn page_number_is_within_chapter() {
    let info = FooterInfo::build(&sample_input(vec![]));

    // 显示用 1 起
    assert_eq!(
        info.display_page_number, 5,
        "章内页码显示应为 1 起（4 + 1）"
    );
    assert_eq!(
        info.page_count_in_chapter, 12,
        "章内总页数应如实呈现（不换算成全书页码）"
    );
}

// ===========================================================================
// 3. 剩余时间 = 剩余字符 ÷ 近期速度
// ===========================================================================

/// **剩余时间按"当前章剩余字符数 ÷ 近期阅读速度"计算**（FR-042 口径）。
///
/// ## 算式
///
/// 取近期记录：`read_chars` 合计 / `read_seconds` 合计 = 字符/秒。
/// 剩余字符 = `chapter_char_len - chapter_char_offset`。
/// 剩余秒数 = 剩余字符 ÷ 速度。
///
/// 本例：近期 600 秒读了 3000 字符 → 5 字符/秒。
/// 剩余 3000 字符 → 600 秒 = 10 分钟。
#[test]
fn remaining_time_uses_recent_speed() {
    let input = sample_input(vec![
        record("2026-10-10", 300, 1500),
        record("2026-10-11", 300, 1500),
    ]);
    let info = FooterInfo::build(&input);

    let remaining = info.remaining_seconds.expect("有足够数据时应给出估算");
    assert!(
        (remaining - 600.0).abs() < 1.0,
        "剩余秒数应为 600（3000 字符 ÷ 5 字符/秒），实测 {remaining}"
    );
    assert_eq!(
        info.remaining_minutes(),
        Some(10),
        "剩余分钟应四舍五入为 10"
    );
}

/// **速度取"近期"记录**（不是全部历史平均）。
///
/// ## 为什么强调"近期"
///
/// FR-042 的措辞是"近期阅读速度"。用全部历史平均的话，一个月前读得慢的
/// 记录会把今天的估算永久拖长——估算会随阅读能力变化而失效。
///
/// ## 判据必须是"窗口外 vs 窗口内"的差分
///
/// 若记录条数少于窗口大小（7 条），"取近期"与"取全部"结果相同——那样的
/// 测试是假绿的（本测试的初版就栽在这里：只给 2 条记录，断言失败于实测
/// 545 秒）。故本例给 **10 条**：早期 8 条很慢，最近 2 条很快，只有真正
/// 实现"截窗口"才会得到小于 450 秒的结果。
#[test]
fn speed_reflects_recent_records_not_lifetime_average() {
    let mut records = Vec::new();
    // 早期 8 条：每条 100 秒读 100 字符 → 1 字符/秒（应被窗口排除）
    for day in 1..=8 {
        records.push(record(&format!("2026-09-{day:02}"), 100, 100));
    }
    // 最近 2 条：每条 300 秒读 3000 字符 → 10 字符/秒
    records.push(record("2026-10-10", 300, 3000));
    records.push(record("2026-10-11", 300, 3000));

    let input = sample_input(records);
    let info = FooterInfo::build(&input);
    let remaining = info.remaining_seconds.expect("应有估算");

    // 窗口取后 7 条 = 5 条早期(共 500s/500c) + 2 条近期(共 600s/6000c)
    //   → 6500 / 1100 ≈ 5.91 字符/秒 → 3000 ÷ 5.91 ≈ 507.7 秒
    // 若用全部 10 条：6800 / 1400 ≈ 4.86 → ≈ 617.6 秒
    assert!(
        remaining < 560.0,
        "估算应偏向近期（截 7 条窗口约 508 秒），实测 {remaining}——\
         若接近 618 说明用了全程平均而非近期窗口"
    );
}

/// **剩余时间基于"当前章"而非全书**（FR-042 明文"当前章剩余字符数"）。
#[test]
fn remaining_time_is_scoped_to_current_chapter() {
    let input = FooterInput {
        chapter_index: 0,
        chapter_title: "第一章".to_string(),
        chapter_char_len: 1000,
        chapter_char_offset: 800,
        page_in_chapter: 0,
        page_count_in_chapter: 1,
        recent_records: vec![record("2026-10-11", 100, 1000)],
    };
    let info = FooterInfo::build(&input);
    let remaining = info.remaining_seconds.expect("应有估算");
    // 剩余 200 字符 ÷ 10 字符/秒 = 20 秒
    assert!(
        (remaining - 20.0).abs() < 0.5,
        "剩余应按当前章剩余字符算（200÷10=20），实测 {remaining}"
    );
}

// ===========================================================================
// 4. 无足够数据时隐藏
// ===========================================================================

/// **无记录时剩余时间隐藏**（FR-042：MUST NOT 显示无依据的估算值）。
#[test]
fn hidden_when_no_records() {
    let info = FooterInfo::build(&sample_input(vec![]));
    assert!(
        info.remaining_seconds.is_none(),
        "无阅读记录时剩余时间必须隐藏（不得凭空给一个估算）"
    );
    assert!(
        info.remaining_minutes().is_none(),
        "隐藏应一贯到底（分钟视图同样为空）"
    );
    // 其余字段仍然可用（隐藏的只是剩余时间这一项）
    assert_eq!(info.chapter_index, 3, "隐藏剩余时间不影响章号");
    assert_eq!(info.display_page_number, 5, "隐藏剩余时间不影响页码");
}

/// **记录不足以推出速度时隐藏**（秒数太少 / 字符数为零）。
#[test]
fn hidden_when_records_are_insufficient() {
    // ① 秒数太少：2 秒的记录无法推速度
    let too_few_seconds = FooterInfo::build(&sample_input(vec![record("2026-10-11", 2, 100)]));
    assert!(
        too_few_seconds.remaining_seconds.is_none(),
        "记录秒数过少时剩余时间应隐藏（2 秒不足以推速度）"
    );

    // ② 字符数为零：速度为 0，除法无意义
    let zero_chars = FooterInfo::build(&sample_input(vec![record("2026-10-11", 600, 0)]));
    assert!(
        zero_chars.remaining_seconds.is_none(),
        "记录里读字数为零时剩余时间应隐藏（速度为零，估算无依据）"
    );

    // ③ 全部记录字符数为零（多条也一样）
    let all_zero = FooterInfo::build(&sample_input(vec![
        record("2026-10-10", 300, 0),
        record("2026-10-11", 300, 0),
    ]));
    assert!(
        all_zero.remaining_seconds.is_none(),
        "全部记录读字数为零时剩余时间应隐藏"
    );
}

/// **已读完本章时剩余为零**（不是隐藏——"读完了"是有依据的确定值）。
#[test]
fn zero_remaining_when_chapter_finished() {
    let input = FooterInput {
        chapter_index: 0,
        chapter_title: "第一章".to_string(),
        chapter_char_len: 1000,
        chapter_char_offset: 1000,
        page_in_chapter: 9,
        page_count_in_chapter: 10,
        recent_records: vec![record("2026-10-11", 100, 1000)],
    };
    let info = FooterInfo::build(&input);
    assert_eq!(
        info.remaining_seconds,
        Some(0.0),
        "读完之后剩余应为零（这是有依据的值，不该隐藏）"
    );
}

/// **越界偏移被钳制**（字符偏移超过章长时不产生负的剩余）。
#[test]
fn out_of_range_offset_is_clamped() {
    let input = FooterInput {
        chapter_index: 0,
        chapter_title: "第一章".to_string(),
        chapter_char_len: 1000,
        chapter_char_offset: 9999,
        page_in_chapter: 0,
        page_count_in_chapter: 1,
        recent_records: vec![record("2026-10-11", 100, 1000)],
    };
    let info = FooterInfo::build(&input);
    assert_eq!(
        info.remaining_seconds,
        Some(0.0),
        "越界偏移应钳到章末（剩余为零），不得产生负值"
    );
}

// ===========================================================================
// 展示与稳定性
// ===========================================================================

/// **构造是确定性的且不 panic**（纯计算，无隐藏状态）。
#[test]
fn build_is_deterministic() {
    let input = sample_input(vec![record("2026-10-11", 600, 3000)]);
    let first = FooterInfo::build(&input);
    let second = FooterInfo::build(&input);
    assert_eq!(first, second, "同输入的两次构造必须逐字段相同");
}

/// **页脚信息可序列化字段完整**（供 S3 外壳直接渲染）。
#[test]
fn footer_info_exposes_all_display_fields() {
    let info = FooterInfo::build(&sample_input(vec![record("2026-10-11", 600, 3000)]));
    // 四个必需的展示字段
    assert_eq!(info.display_chapter_number, 4);
    assert!(!info.chapter_title.is_empty());
    assert_eq!(info.display_page_number, 5);
    assert_eq!(info.page_count_in_chapter, 12);
}
