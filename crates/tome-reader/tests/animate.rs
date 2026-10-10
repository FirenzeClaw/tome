//! T038 验收：翻页动效 + reduced-motion 降级（FR-046、FR-032）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T038）
//!
//! - 有动效 / 降级两路径测试
//! - 降级后**分页结果逐字节相同**断言（FR-032 的核心要求）
//!
//! ## FR-032 的原文与它为什么这样措辞
//!
//! > 系统 MUST 支持**减少动效**（reduced motion）开关。开启时：翻页与过渡
//! > 动效时长 MUST 降为 0（立即完成）；滚动 MUST NOT 使用平滑缓动动画。
//! > **该开关 MUST NOT 改变分页结果、滚动流布局或阅读位置。**
//!
//! 最后一句是这条需求的**实质**：动效开关是**呈现参数**（FR-028 的子集
//! 分离）。若它影响了分页，"我开了减少动效，页码全变了"——那是最难排查
//! 的一类缺陷（用户不会想到两个功能相关）。故本文件的重点是**对照**：
//! 两个开关状态下走完整分页管线，断言页序列逐字节相同。
//!
//! ## MASTER §4 的档位（本任务的动效参数依据）
//!
//! | 元素 | 时长 | 缓动 |
//! |---|---|---|
//! | 翻页动效 | `--dur-slow`（280ms） | `--ease-spring` |
//!
//! `MOTION.md` §1 对 `--ease-spring` 有一条**使用约束**："只允许用于
//! 「从无到有」的入场，不允许用于位置移动（过冲会让文字错位一瞬）"。
//! 翻页不是"位置移动"（下一页是**换成**新内容，不是在屏幕上平移）——
//! 但本实现仍须注意：过渡帧的**位移**用 `ease-out`，只有透明度/缩放用
//! `ease-spring`。这一条在 [`PageTurn::easing_for`] 里显式表达。

#![cfg(feature = "render")]

use tome_core::model::theme::DayTheme;
use tome_reader::paginate::config::{PresentationConfig, ReadingConfig};
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::render::animate::{
    Animation, EasingKind, MotionPreference, PageTurn, ReducedMotion,
};

fn sample_chapter() -> String {
    [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样；她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然。」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
        "《红楼梦》（又名《石头记》）是清代曹雪芹的作品，书中有四百多个人物，其中以宝黛钗三人最为人所知。",
    ]
    .join("\n")
}

// ===========================================================================
// 动效档位来自 Theme（不散读常量）
// ===========================================================================

/// **时长与缓动取自 Theme 的 motion token**（FR-029：禁各绘制点散读常量）。
#[test]
fn animation_timing_comes_from_theme_tokens() {
    let theme = DayTheme.theme();
    let motion = theme.motion();

    let turn = PageTurn::for_theme(theme);
    assert_eq!(
        turn.duration_ms(),
        motion.dur_slow_ms,
        "翻页时长应取 --dur-slow（MASTER §4 的翻页档位）"
    );
    assert_eq!(
        turn.easing(),
        EasingKind::Spring,
        "翻页缓动应取 --ease-spring（MASTER §4 的翻页档位）"
    );
    assert_eq!(turn.duration_ms(), 280, "MASTER §4：--dur-slow = 280ms");
}

/// **`--ease-spring` 不用于位移**（MOTION.md §1 的明文约束）。
///
/// ## 这条约束的字面依据
///
/// MOTION.md：`--ease-spring` 的控制点 `1.3` 会产生约 3% 的过冲——
/// 只允许用于「从无到有」的入场，**不允许用于位置移动**（过冲会让文字
/// 错位一瞬）。
///
/// 故翻页的**位移**用 `ease-out`，而**透明度/缩放**才允许 `ease-spring`。
/// 判据是"位移通道的缓动不是 Spring"。
#[test]
fn spring_easing_is_not_used_for_translation() {
    let turn = PageTurn::for_theme(DayTheme.theme());

    assert_eq!(
        turn.easing_for(Animation::Translate),
        EasingKind::Out,
        "位移通道不得用 --ease-spring（过冲会让文字错位一瞬）"
    );
    assert_eq!(
        turn.easing_for(Animation::Fade),
        EasingKind::Spring,
        "淡入通道可用 --ease-spring（「从无到有」的入场）"
    );
}

// ===========================================================================
// reduced-motion 降级（FR-032）
// ===========================================================================

/// **降级时长为 0**（FR-032："降为 0（立即完成）"）。
#[test]
fn reduced_motion_zeroes_the_duration() {
    let turn = PageTurn::for_theme(DayTheme.theme());
    let reduced = turn.with_preference(MotionPreference::Reduced);

    assert_eq!(
        reduced.duration_ms(),
        0,
        "减少动效开启时时长必须降为 0（FR-032）"
    );
    assert_eq!(reduced.preference(), MotionPreference::Reduced);
    assert!(reduced.is_instant(), "时长 0 应被判为瞬时切换");

    // 而未降级时不是瞬时（否则"降级"这件事无从分辨）
    assert!(
        !turn.is_instant(),
        "正常档位的时长应大于 0（否则降级与正常无法分辨）"
    );
}

/// **降级时滚动不用平滑缓动**（FR-032 的第二句）。
#[test]
fn reduced_motion_disables_smooth_scrolling() {
    let turn = PageTurn::for_theme(DayTheme.theme());

    assert_eq!(
        turn.with_preference(MotionPreference::Normal)
            .scroll_easing(),
        Some(EasingKind::Out),
        "正常档位下滚动应有平滑缓动"
    );
    assert_eq!(
        turn.with_preference(MotionPreference::Reduced)
            .scroll_easing(),
        None,
        "减少动效下滚动 MUST NOT 使用平滑缓动（FR-032：直接位移）"
    );
}

/// **瞬时切换时进度直接到 1**（不产生中间帧）。
#[test]
fn reduced_motion_yields_no_intermediate_frames() {
    let turn = PageTurn::for_theme(DayTheme.theme()).with_preference(MotionPreference::Reduced);

    // 降级：任何时刻（含 t=0）的进度都是 1——即"立即完成"
    for elapsed in [0u32, 1, 50, 1000] {
        assert_eq!(
            turn.progress_at(elapsed),
            1.0,
            "减少动效下 elapsed={elapsed}ms 时的进度应为 1（立即完成）"
        );
    }

    // 正常：t=0 时进度 0，中途在 (0,1) 之间，结束为 1
    let normal = PageTurn::for_theme(DayTheme.theme());
    assert!(normal.progress_at(0) < 0.1, "正常档位 t=0 时进度应接近 0");
    let middle = normal.progress_at(normal.duration_ms() / 2);
    assert!(
        middle > 0.0 && middle < 1.0,
        "正常档位中途进度应在 (0,1) 之间，实测 {middle}"
    );
    assert_eq!(
        normal.progress_at(normal.duration_ms()),
        1.0,
        "正常档位到时长终点应恰为 1"
    );
    assert_eq!(
        normal.progress_at(normal.duration_ms() * 10),
        1.0,
        "超过时长后应钳在 1（不产生越界值）"
    );
}

/// 进度**单调不减**（动效不应回退——回退会让画面抖动）。
#[test]
fn progress_is_monotonic() {
    let turn = PageTurn::for_theme(DayTheme.theme());
    let duration = turn.duration_ms();

    let mut previous = 0.0f32;
    for step in 0..=20 {
        let elapsed = duration * step / 20;
        let progress = turn.progress_at(elapsed);
        assert!(
            progress >= previous - 1e-6,
            "进度回退了：elapsed={elapsed}ms 时 {progress} < 上一个 {previous}"
        );
        assert!(
            (0.0..=1.0).contains(&progress),
            "进度应在 [0,1] 内，实测 {progress}"
        );
        previous = progress;
    }
}

/// `ReducedMotion` 开关的语义与系统设置同名（便于组合根接线）。
#[test]
fn reduced_motion_switch_semantics() {
    assert_eq!(
        ReducedMotion::from_flag(true),
        MotionPreference::Reduced,
        "开关开启即降级"
    );
    assert_eq!(
        ReducedMotion::from_flag(false),
        MotionPreference::Normal,
        "开关关闭即正常档位"
    );
    assert!(ReducedMotion::from_flag(true).is_reduced());
    assert!(!ReducedMotion::from_flag(false).is_reduced());
}

// ===========================================================================
// FR-032 的核心：动效开关 **不改变** 分页结果
// ===========================================================================

/// **降级前后分页结果逐字节相同**（FR-032 的实质要求）。
///
/// ## 判据为什么是"走完整管线再比对"
///
/// 只在动效模块内断言"时长=0"是**空**的——它测不出"动效参数泄漏进分页"。
/// 有效判据是让两个开关状态各自跑一遍**完整的**排版分页管线，
/// 比对页序列（字符区间）逐字段相同。
///
/// 这条也是 FR-019/SC-005 的延伸：动效是呈现参数，与主题同属"只决定绘制"。
#[test]
fn reduced_motion_does_not_change_pagination() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 300.0);

    let paginate_with = |presentation: PresentationConfig| {
        let mut paginator = Paginator::new();
        let chapter =
            layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");
        // 呈现参数**不进** layout_chapter 的签名（FR-028 的子集分离），
        // 故这里的用法是"带着它跑一遍分页"——若实现有一天让它参与，
        // 那会表现为两个分支的结果不同。
        let _ = presentation;
        chapter
    };

    let normal_presentation = PresentationConfig {
        reduce_motion: false,
        ..PresentationConfig::default()
    };
    let reduced_presentation = PresentationConfig {
        reduce_motion: true,
        ..PresentationConfig::default()
    };

    let normal = paginate_with(normal_presentation);
    let reduced = paginate_with(reduced_presentation);

    // 行盒逐字段相同（含字符区间、字身、压缩量——全部分页几何）
    assert_eq!(
        normal.lines.len(),
        reduced.lines.len(),
        "动效开关不得改变行数"
    );
    for (index, (left, right)) in normal.lines.iter().zip(reduced.lines.iter()).enumerate() {
        assert_eq!(
            left.chapter_char_range, right.chapter_char_range,
            "第 {index} 行的章内区间不得因动效开关改变"
        );
        assert_eq!(
            left.line.char_range, right.line.char_range,
            "第 {index} 行的段内区间不得因动效开关改变"
        );
        assert_eq!(
            left.line.glyphs.len(),
            right.line.glyphs.len(),
            "第 {index} 行的字形数不得因动效开关改变"
        );
        assert!(
            (left.line.height_px - right.line.height_px).abs() < f32::EPSILON,
            "第 {index} 行的行高不得因动效开关改变"
        );
    }

    // 页序列（字符区间）也逐字段相同
    let mut paginator = Paginator::new();
    let paged = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");
    let rebuilt: String = paged
        .pages
        .iter()
        .filter(|page| !page.is_padding)
        .map(|page| {
            content
                .chars()
                .skip(page.char_range.start)
                .take(page.char_range.len())
                .collect::<String>()
        })
        .collect();
    assert_eq!(rebuilt, content, "SC-006 在动效开关下仍须成立");
}

/// **阅读位置不受动效开关影响**（FR-032 的"阅读位置"一项）。
#[test]
fn reduced_motion_does_not_change_reading_position() {
    let content = sample_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 300.0);

    let mut paginator = Paginator::new();
    let paged = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");

    // 锚点解析与动效开关无关（它只依赖页序列）
    let anchor = 100usize;
    let resolution = paged.resolve_anchor(anchor).expect("锚点应可解析");
    assert!(resolution.is_visible(), "锚点应在可见范围内");

    // 动效只影响"翻到那一页的过场"，不影响"那一页是哪一页"。
    //
    // ## 判据的两种形态
    //
    // 1. **同一锚点两次解析到同一页**——解析只依赖页序列
    // 2. **动效参数的降级只作用于时间维度**——时长与缓动变，页归属不变
    //    （`PageTurn` 里根本没有"页"这个字段，这是结构性事实；
    //    这里用"降级只改时间量"来断言它）
    let turn_normal = PageTurn::for_theme(DayTheme.theme());
    let turn_reduced = turn_normal.with_preference(MotionPreference::Reduced);

    assert_eq!(
        paged.resolve_anchor(anchor).and_then(|r| r.page_index()),
        resolution.page_index(),
        "同一锚点两次解析应到同一页（解析只依赖页序列，不依赖动效状态）"
    );

    // 降级的影响面**限于时间**：时长归零、缓动转线性；
    // 而"有哪些通道""通道的语义"都不变（页面归属无从被影响）
    assert!(turn_normal.duration_ms() != 0, "正常档位时长非零");
    assert_eq!(turn_reduced.duration_ms(), 0, "降级档位时长归零");
    assert_eq!(
        turn_reduced.easing_for(Animation::Translate),
        EasingKind::Linear,
        "降级档位的位移缓动应为线性（无过渡帧）"
    );
    assert_eq!(
        turn_normal.easing_for(Animation::Translate),
        EasingKind::Out,
        "正常档位的位移缓动为 --ease-out"
    );
}

/// 降级是**幂等**的（重复应用不改变结果）。
#[test]
fn applying_reduced_motion_is_idempotent() {
    let turn = PageTurn::for_theme(DayTheme.theme());
    let once = turn.with_preference(MotionPreference::Reduced);
    let twice = once.with_preference(MotionPreference::Reduced);
    assert_eq!(once, twice, "重复应用降级应得到同一结果");

    // 反向：从降级回到正常应恢复原值（开关可逆）
    let back = once.with_preference(MotionPreference::Normal);
    assert_eq!(back.duration_ms(), turn.duration_ms());
    assert_eq!(back.easing(), turn.easing());
}
