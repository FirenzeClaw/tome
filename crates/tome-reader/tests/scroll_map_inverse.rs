//! T026 验收：滚动偏移 ⇄ 字符位置双向映射（FR-022 / FR-040）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T026）
//!
//! > 互逆往返误差 ≤ 半个字形（`crates/tome-reader/tests/scroll_map_inverse.rs`，
//! > **翻页与滚动两形态都可用**）
//!
//! ## 「半个字形」在哪条轴上可满足（本文件的判据分离，必须读）
//!
//! FR-040 的"往返误差 ≤ 半个字形"是**水平方向**的约束——那里字形步进给出
//! 亚行分辨率。垂直方向**不可满足**该字面阈值，这不是实现问题而是信息论
//! 下界：
//!
//! | 轴 | 一个"单位"的高度/宽度 | 谁能分辨 |
//! |---|---|---|
//! | 垂直 | **行高**（19px 字号 × 1.85 = 35.15px） | 行——同一行的字符共享同一垂直位置 |
//! | 水平 | **字形步进**（CJK 全角 = 19px） | 字符——同一行的字符各有自己的 x |
//!
//! 故垂直旋转到"行"这一分辨率：任意垂直偏移最多只能确定到**行**。若要求
//! `offset → char → offset` 的垂直误差 ≤ 半个字形（9.5px），而一行高 35.15px，
//! 则**没有任何实现**能满足——把偏移取整到行首，误差可达一整行。
//!
//! 故本文件按轴分离判据：
//!
//! | 方向 | 判据 | 阈值 |
//! |---|---|---|
//! | 垂直 `offset → char → offset` | 误差 ≤ **一字身（行盒高）** | 行分辨率的必然结果 |
//! | 水平 `x → char → x`（行内） | 误差 ≤ **半个字形** | FR-040 的字面约束 |
//! | 垂直 `offset → char → offset`（**幂等**） | 第二次往返误差 **= 0** | 规范化后不再变化 |
//! | `char → offset → char` | 落在**同一行** | 位置不跳到别的行 |
//!
//! "幂等"那条是关键：第一次往返把偏移规范化到行首（误差 ≤ 一行），但**再
//! 往返一次就不动了**。若无这条，映射会在每次滚动事件里累积漂移。
//!
//! ## 为什么两形态都要可用（FR-040 末句）
//!
//! > 且在翻页与滚动两种形态下 MUST 都可用。
//!
//! 翻页模式的"屏幕坐标"是"页内坐标"（页首为原点），滚动是"流内坐标"
//! （流首为原点）。本文件的 `paged` 用例把页内坐标喂给同一套映射函数，
//! 验证它对两形态都成立。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{layout_chapter, PageLine, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::ScrollFlow;
use tome_reader::scroll::map::PositionMap;

// ===========================================================================
// 夹具
// ===========================================================================

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

fn long_chapter() -> String {
    let paragraph = "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，\
                     后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。";
    (0..8)
        .map(|index| format!("第{}段。{paragraph}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_flow(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("构造滚动流应成功")
}

/// 一行的高度（= 一个"字身"在垂直方向的量）。
fn line_box_height_px(geometry: &GeometryConfig) -> f32 {
    geometry.line_height_px()
}

/// 半个字形宽（水平方向的 FR-040 阈值）。
fn half_glyph_px(geometry: &GeometryConfig) -> f32 {
    f32::from(geometry.font_size_px) / 2.0
}

/// 字符所属的行盒（章内区间包含该字符）。
fn owning_line(flow: &ScrollFlow, char_offset: usize) -> Option<&PageLine> {
    flow.lines()
        .iter()
        .find(|line| line.chapter_char_range.contains(&char_offset))
}

// ===========================================================================
// 垂直：offset → char → offset（行分辨率）
// ===========================================================================

/// **`char_to_offset` 返回该字符所属行的顶部偏移**（规范化契约）。
///
/// ## 这条是"规范化"的定义，不是精度检查
///
/// 垂直方向只能分辨到行，故 `char_to_offset` 的唯一自洽取值是**行顶**。
/// 任何"行内插值"（如按字符在行内的列号加一点偏移）都会破坏幂等性——但
/// 那种破坏**可能不可见**：若插值幅度小于行高，`offset → char → offset`
/// 会回到同一个字符、同一个插值量，往返仍"稳定"。
///
/// 变异验证揭示了这一点：把 `char_to_offset` 改成 `行顶 + (字符号 % 3)` 后，
/// 往返类断言**全部照过**（插值对往返不可观测——`offset_to_char` 只返回行首
/// 字符）。故必须有这条**直接**断言把取值钉住。
///
/// ## 为什么必须是行顶（而不是"行内任意一致取值"）
///
/// `char_to_offset` 的用途是"滚动到某字符"：调用方拿它当滚动目标位置。取
/// 行顶即"让该行出现在视口顶部"；取别的值会让内容偏移若干像素，而行数一多
/// 就累积成明显错位。
#[test]
fn char_to_offset_returns_the_owning_line_top_exactly() {
    let content = long_chapter();
    let char_len = content.chars().count();

    for (width, height, font_size) in [
        (342.0f32, 200.0f32, 19u16),
        (342.0, 400.0, 19),
        (684.0, 600.0, 24),
    ] {
        let mut geometry = geometry();
        geometry.font_size_px = font_size;
        let viewport = Viewport::new(width, height);
        let flow = build_flow(&content, &geometry, &viewport);
        let map = PositionMap::new(&flow);

        // 逐行算出该行的顶部偏移（行高累加），核对行内每个字符都返回它
        let mut expected_top = 0.0f32;
        for line in flow.lines() {
            for char_offset in line.chapter_char_range.clone() {
                let actual = map
                    .char_to_offset(char_offset)
                    .unwrap_or_else(|| panic!("字符 {char_offset} 应可映射"));
                assert!(
                    (actual - expected_top).abs() < 1e-3,
                    "{width}x{height}/{font_size}px：字符 {char_offset} 的偏移 {actual:.4}                      应**恰好**是该行行顶 {expected_top:.4}（规范化契约）"
                );
            }
            expected_top += line.line.height_px.max(0.0);
        }

        // 末尾（读完了）应恰好是总高
        let end = map.char_to_offset(char_len).expect("末尾应可映射");
        assert!(
            (end - flow.content_height_px()).abs() < 1e-3,
            "末尾字符的偏移应恰好等于流总高"
        );
    }
}

/// **垂直往返误差 ≤ 一字身（行盒高）**——行分辨率的必然结果。
#[test]
fn vertical_roundtrip_is_within_one_line_box() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let flow = build_flow(&content, &geometry, &viewport);
    let map = PositionMap::new(&flow);
    let tolerance = line_box_height_px(&geometry);

    let total = flow.content_height_px();
    assert!(total > 100.0, "前置：正文应有多屏高");

    let mut offset_px = 0.0f32;
    let mut checked = 0usize;
    while offset_px < total {
        let char_offset = map
            .offset_to_char(offset_px)
            .unwrap_or_else(|| panic!("偏移 {offset_px} 应可映射"));
        let back_px = map
            .char_to_offset(char_offset)
            .unwrap_or_else(|| panic!("字符 {char_offset} 应可映射"));
        assert!(
            (back_px - offset_px).abs() <= tolerance + 1e-3,
            "偏移 {offset_px:.2} → 字符 {char_offset} → 偏移 {back_px:.2}，\
             误差 {:.2}px 超过一行高 {tolerance:.2}px",
            (back_px - offset_px).abs()
        );
        offset_px += 3.0;
        checked += 1;
    }
    assert!(checked > 30, "应采样足够多的偏移，实测 {checked}");
}

/// **垂直往返是幂等的**：第二次往返误差恒为 0（不累积漂移）。
///
/// ## 为什么这条比"误差 ≤ 一行"更强也更重要
///
/// "误差 ≤ 一行"允许每次都漂一点点。滚动是一个**高频重复**的动作，
/// 累积漂移的表现是"慢慢滚着滚着位置就不对了"——而且越滚越偏。
///
/// 幂等性保证：一旦往返过一次，位置就固定下来。这是"规范化"（canonicalization）
/// 的标准性质，也是实现里必须真的把偏移**吸附到行首**的证据。
#[test]
fn vertical_roundtrip_is_idempotent() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let flow = build_flow(&content, &geometry, &viewport);
    let map = PositionMap::new(&flow);

    let mut offset_px = 0.0f32;
    let mut checked = 0usize;
    while offset_px < flow.content_height_px() {
        // 第一次往返
        let char_once = map.offset_to_char(offset_px).expect("应可映射");
        let offset_once = map.char_to_offset(char_once).expect("应可映射");
        // 第二次往返
        let char_twice = map.offset_to_char(offset_once).expect("应可映射");
        let offset_twice = map.char_to_offset(char_twice).expect("应可映射");

        assert_eq!(
            char_twice, char_once,
            "第二次往返改变了字符（第一次 {char_once}，第二次 {char_twice}）"
        );
        assert!(
            (offset_twice - offset_once).abs() < 1e-3,
            "垂直往返非幂等：第一次 → {offset_once:.2}，第二次 → {offset_twice:.2}\
             （起始偏移 {offset_px:.2}）——会累积漂移"
        );
        offset_px += 7.0;
        checked += 1;
    }
    assert!(checked > 20, "应采样足够多，实测 {checked}");
}

/// **`char → offset → char` 落在同一行**（位置不跳到别的行）。
///
/// ## 阈值是"同一行"而非"同一字符"
///
/// 垂直方向无法区分同一行内的字符（它们共享同一垂直位置），故 `char → offset`
/// 只能给出**行**的信息。要求回到同一字符是不成立的（信息在映射中丢失）。
///
/// 可断言的是：回来的字符**仍在原来那一行**——即"滚动到某处不会跳到别的
/// 段落"。这是用户可感知的性质：位置标识（页脚/进度）指向的是同一行。
#[test]
fn char_to_offset_to_char_stays_on_the_same_line() {
    let content = long_chapter();
    let char_len = content.chars().count();

    for (width, height, font_size) in [
        (342.0f32, 200.0f32, 19u16),
        (342.0, 400.0, 19),
        (228.0, 150.0, 14),
        (684.0, 600.0, 24),
    ] {
        let mut geometry = geometry();
        geometry.font_size_px = font_size;
        let viewport = Viewport::new(width, height);
        let flow = build_flow(&content, &geometry, &viewport);
        let map = PositionMap::new(&flow);

        let mut checked = 0usize;
        for char_offset in 0..char_len {
            let offset_px = map.char_to_offset(char_offset).unwrap_or_else(|| {
                panic!("{width}x{height}/{font_size}px：字符 {char_offset} 无法映射")
            });
            let back = map.offset_to_char(offset_px).unwrap_or_else(|| {
                panic!("{width}x{height}/{font_size}px：偏移 {offset_px} 无法映射回字符")
            });

            let original_line = owning_line(&flow, char_offset);
            let returned_line = owning_line(&flow, back);
            // 字符不在任何行盒内（段间分隔符）：允许映射到相邻行的边界
            if let (Some(a), Some(b)) = (original_line, returned_line) {
                assert_eq!(
                    a.chapter_char_range, b.chapter_char_range,
                    "{width}x{height}/{font_size}px：字符 {char_offset} 在行 {:?}，\
                     往返后落在行 {:?}（跳行了）",
                    a.chapter_char_range, b.chapter_char_range
                );
            }
            checked += 1;
        }
        assert!(checked > 50, "本例应检查足够多的字符，实测 {checked}");
    }
}

// ===========================================================================
// 水平：x → char → x（FR-040 的字面约束在这条轴上）
// ===========================================================================

/// **行内水平往返误差 ≤ 半个字形**（FR-040 的字面阈值）。
///
/// ## 为什么这条能达阈值而垂直那条不能
///
/// 同一行内每个字符有**自己的**水平范围（字形步进），故像素坐标能分辨到
/// 字符级。映射到某字符后取该字符的起点，误差至多是它的半个宽度。
///
/// 这是 FR-040 "在字形边界上的往返误差不大于半个字形"的**原本语境**——
/// 命中测试与文字选择关心的是"点在哪个字上"，那是一个水平问题。
#[test]
fn horizontal_roundtrip_is_within_half_glyph() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let flow = build_flow(&content, &geometry, &viewport);
    let map = PositionMap::new(&flow);
    let _tolerance_calibration = half_glyph_px(&geometry); // 恒定阈值仅作对照

    // 取若干行，在其行内宽度上逐点采样
    //
    // ## 采样上界取"末字形右边缘"而非"各 advance 之和"
    //
    // 两端对齐会**伸展**行（`justify_extra_px`）：字形数量不变，但字间的
    // 空白被撑开，故 `x_px` 的间距大于 `advance_px`。于是
    // `sum(advance)` 会**小于**行的实际视觉宽度，用它当采样上界会漏掉行尾
    // 那一段；反之若用它做上界而映射用 `x`，两者对不上。
    //
    // 正确的上界是"末字形的右边缘"（含伸展后的实际位置）。
    //
    // ## 阈值取"该字形**实际排布后**的 advance 之半"
    //
    // 见本函数文档：两端对齐把余白加到字形的 `advance_px` 上，故首字形的
    // `advance_px` 可达 23.75px（字号 19 + 伸展 4.75），其中心在 11.88px——
    // 超过恒定 `font_size/2 = 9.5px`。那不是映射误差：用户点 x=0 确实落在
    // 首字形内，反向给出的"该字形的代表点"就是它的中心。用恒定阈值会把
    // **正确的排布**判成违规。
    let mut checked = 0usize;
    for line in flow.lines().iter().take(20) {
        let line_width: f32 = line
            .line
            .glyphs
            .last()
            .map_or(0.0, |glyph| glyph.x_px + glyph.advance_px.max(0.0));
        if line_width <= 0.0 {
            continue;
        }
        let paragraph_offset = line
            .chapter_char_range
            .start
            .saturating_sub(line.line.char_range.start);

        let mut x = 0.0f32;
        while x <= line_width {
            let Some(char_offset) = map.point_in_line_to_char(line, x) else {
                panic!("行内 x={x} 应可映射到字符");
            };
            // 越出行内容宽度的 x 给出**行末光标位**（不是本行的字符）——
            // 那一情形由 `x_past_line_end_yields_caret_position_not_rejected`
            // 专门覆盖，此处跳过以免把"正确的语义"当违规。
            let Some(back_x) = map.char_to_point_in_line(line, char_offset) else {
                assert_eq!(
                    char_offset, line.chapter_char_range.end,
                    "反向失败只允许发生在行末光标位上，实测字符 {char_offset}"
                );
                x += 2.0;
                checked += 1;
                continue;
            };

            // 该字符所属字形的**实际** advance（含两端对齐的伸展）
            let glyph_advance = char_offset
                .checked_sub(paragraph_offset)
                .and_then(|paragraph_index| {
                    line.line.glyphs.iter().find(|glyph| {
                        glyph.char_range.start <= paragraph_index
                            && paragraph_index < glyph.char_range.end
                    })
                })
                .map_or(0.0, |glyph| glyph.advance_px.max(0.0));

            let allowed = glyph_advance / 2.0 + 1e-3;
            assert!(
                (back_x - x).abs() <= allowed,
                "行内 x={x:.2} → 字符 {char_offset} → x={back_x:.2}，\
                 误差 {:.2}px 超过阈值 {allowed:.2}px（该字形 advance {glyph_advance:.2}px 之半）",
                (back_x - x).abs()
            );
            x += 2.0;
            checked += 1;
        }
    }
    assert!(checked > 50, "应采样足够多的行内点，实测 {checked}");
}

/// **越出行内容宽度的 x 给出"行末光标位"**，且这一情形被显式区分。
///
/// ## 为什么这条要单独一个测试（它不是"边界 case"而是语义）
///
/// `point_in_line_to_char(x)` 在 x 超过行的内容宽度时返回 `line.chapter_char_range.end`
/// ——那是**行末光标位**（caret at end of line），不是本行的字符。故
/// `char_to_point_in_line` 对它返回 `None` 是**正确的**（该字符属于下一行）。
///
/// 这个不对称是有意义的：
///
/// | 方向 | x 越界时的答案 |
/// |---|---|
/// | `x → char` | 行末光标位（用户点了行尾空白，光标停在行末） |
/// | `char → x` | `None`（那个字符不在本行，本行无法给出它的位置） |
///
/// 若为了"往返自洽"而把越界 x 钳到**最后一个字符**，那个字形的中心与用户
/// 点击位置会差近一个字身——反而违反 FR-040 的误差界，且"点行尾空白"会
/// 选中最后一个字（用户预期是"光标在末尾"）。
#[test]
fn x_past_line_end_yields_caret_position_not_rejected() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let flow = build_flow(&content, &geometry, &viewport);
    let map = PositionMap::new(&flow);

    for line in flow.lines().iter().take(10) {
        let content_width: f32 = line
            .line
            .glyphs
            .last()
            .map_or(0.0, |glyph| glyph.x_px + glyph.advance_px.max(0.0));
        if content_width <= 0.0 {
            continue;
        }

        // 远超内容宽度 → 行末光标位
        let caret = map
            .point_in_line_to_char(line, content_width + 500.0)
            .expect("越界 x 应给出光标位而非 None");
        assert_eq!(
            caret, line.chapter_char_range.end,
            "越界 x 应给出**行末光标位**（区间上界），实测 {caret}（本行区间 {:?}）",
            line.chapter_char_range
        );

        // 该位置作为"本行的字符"确实不存在——反向返回 None 是对的
        assert_eq!(
            map.char_to_point_in_line(line, caret),
            None,
            "行末光标位不是本行的字符，反向应返回 None（它属于下一行或行尾边界）"
        );

        // 而本行的最后一个**字符**往返正常
        let last_char = line.chapter_char_range.end - 1;
        let last_x = map
            .char_to_point_in_line(line, last_char)
            .expect("本行末字符应可映射");
        assert_eq!(
            map.point_in_line_to_char(line, last_x),
            Some(last_char),
            "本行末字符的往返应闭合"
        );
    }
}

/// 行内映射落在**该行**的字符范围内（不越到别的行）。
#[test]
fn point_in_line_maps_within_that_line() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let flow = build_flow(&content, &geometry, &viewport);
    let map = PositionMap::new(&flow);

    for line in flow.lines() {
        // 行内区间的两端都应映射到本行内的字符
        for x in [0.0f32, 5.0, 100.0, 1000.0] {
            if let Some(char_offset) = map.point_in_line_to_char(line, x) {
                assert!(
                    line.chapter_char_range.contains(&char_offset)
                        || char_offset == line.chapter_char_range.end,
                    "x={x} 映射到字符 {char_offset}，超出本行区间 {:?}",
                    line.chapter_char_range
                );
            }
        }
    }
}

// ===========================================================================
// FR-040：翻页形态也可用（同一套映射）
// ===========================================================================

/// **翻页形态下同一套映射可用**（FR-040 末句）。
#[test]
fn same_mapping_works_in_paged_form() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let mut paginator = Paginator::new();
    let paginated = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功");

    let tolerance_line = line_box_height_px(&geometry);
    let mut checked = 0usize;

    for page in &paginated.pages {
        if page.lines.is_empty() {
            continue;
        }
        let map = PositionMap::for_page(page);

        // 页内垂直往返（阈值同滚动：一行高）
        for char_offset in page.char_range.clone() {
            let back = map
                .char_to_offset(char_offset)
                .and_then(|offset_px| map.offset_to_char(offset_px))
                .unwrap_or_else(|| panic!("页内字符 {char_offset} 应可往返"));
            // 同一页内（页是位置的主要单位）
            assert!(
                page.char_range.contains(&back) || back == page.char_range.end,
                "页内字符 {char_offset} 往返后落到 {back}，超出本页 {:?}",
                page.char_range
            );
            checked += 1;
        }

        // 页内垂直往返的误差界
        for char_offset in page.char_range.clone().step_by(3) {
            let offset_px = map.char_to_offset(char_offset).expect("应可映射");
            let back_px = map
                .char_to_offset(map.offset_to_char(offset_px).expect("应可映射"))
                .expect("应可映射");
            assert!(
                (back_px - offset_px).abs() <= tolerance_line + 1e-3,
                "页内垂直往返误差 {:.2}px 超过一行高 {tolerance_line:.2}px",
                (back_px - offset_px).abs()
            );
        }
    }
    assert!(checked > 50, "应检查足够多的页内字符，实测 {checked}");
}

// ===========================================================================
// 与 001 锚点口径一致（FR-022）
// ===========================================================================

/// 映射的键是**章内字符下标**（与 001 `ReadProgress.char_offset` 同口径）。
#[test]
fn mapping_keys_are_chapter_char_offsets() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);
    let map = PositionMap::new(&flow);

    assert!(
        map.char_to_offset(char_len + 10_000).is_some(),
        "越界字符应被钳制而非返回 None"
    );
    let end_offset = map.char_to_offset(char_len).expect("末尾应可映射");
    assert!(end_offset.is_finite());
    assert!(
        end_offset <= flow.content_height_px() + 1.0,
        "末尾字符的偏移不得超出流总高"
    );
}

/// 偏移越界（负值 / 超出总高）返回钳制结果而非 panic。
#[test]
fn out_of_range_offset_is_clamped() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);
    let map = PositionMap::new(&flow);

    assert_eq!(map.offset_to_char(-100.0), Some(0), "负偏移应钳到首字符");
    assert_eq!(map.offset_to_char(0.0), Some(0));

    let last = map
        .offset_to_char(flow.content_height_px() + 10_000.0)
        .expect("超界应可映射");
    assert_eq!(last, content.chars().count(), "超界偏移应钳到正文末尾");
}

/// 空章的映射可用（不得 panic、不得 NaN）。
#[test]
fn empty_chapter_mapping_is_safe() {
    let flow = build_flow("", &geometry(), &Viewport::new(342.0, 200.0));
    let map = PositionMap::new(&flow);

    assert_eq!(map.char_to_offset(0), Some(0.0));
    assert_eq!(map.offset_to_char(0.0), Some(0));
    assert_eq!(
        map.offset_to_char(1000.0),
        Some(0),
        "空章的任意偏移都映射到 0"
    );
}

/// 所有映射结果都是**有限**的（无 NaN/Inf，防除零）。
#[test]
fn all_mappings_are_finite() {
    for height in [0.0f32, 1.0, 150.0, 800.0] {
        let content = long_chapter();
        let geometry = geometry();
        let viewport = Viewport::new(342.0, height);
        let flow = build_flow(&content, &geometry, &viewport);
        let map = PositionMap::new(&flow);

        for char_offset in 0..content.chars().count() {
            let offset_px = map.char_to_offset(char_offset).expect("应可映射");
            assert!(
                offset_px.is_finite(),
                "高度 {height}px 下字符 {char_offset} 的偏移 {offset_px} 非有限值"
            );
        }
    }
}

// ===========================================================================
// 跨屏一致性（与 T025 的区间口径对齐）
// ===========================================================================

/// 字符的偏移落在**包含它的那一屏**的像素范围内。
#[test]
fn char_offset_lands_inside_its_owning_viewport() {
    let content = long_chapter();
    let char_len = content.chars().count();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);
    let flow = build_flow(&content, &geometry, &viewport);

    let mut checked = 0usize;
    for viewer in flow.viewports() {
        let bottom = viewer.top_px + viewer.height_px;
        for char_offset in viewer.char_range.clone() {
            // ## 判据：该字符**所在行的像素跨度**与所属屏相交
            //
            // 不能断言"字符偏移落在 [top, bottom) 内"——跨屏行的**顶部**在上一屏
            // 之内（它从那里开始），故它的偏移合法地小于本屏 top。那是 T025
            // "可见行可跨屏"设计的直接后果，不是错位。
            //
            // 有意义的性质是：字符所属行与它的屏**确实相交**（否则该字符归属了
            // 一个根本不显示它的屏）。
            if let Some(line) = owning_line(&flow, char_offset) {
                let index = flow
                    .lines()
                    .iter()
                    .position(|candidate| candidate.chapter_char_range == line.chapter_char_range)
                    .expect("行应存在于流中");
                let line_top: f32 = flow.lines()[..index]
                    .iter()
                    .map(|previous| previous.line.height_px)
                    .sum();
                let line_bottom = line_top + line.line.height_px;
                assert!(
                    line_top < bottom + 1.0 && line_bottom > viewer.top_px - 1.0,
                    "字符 {char_offset} 所属行 [{line_top:.2}, {line_bottom:.2}] \
                     与屏 [{:.2}, {bottom:.2}] 不相交",
                    viewer.top_px
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= char_len.saturating_sub(1), "应覆盖全部字符");
}

/// 映射的**确定性**（FR-048 的形态扩展）。
#[test]
fn mapping_is_deterministic() {
    let content = long_chapter();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 250.0);

    let first = build_flow(&content, &geometry, &viewport);
    let second = build_flow(&content, &geometry, &viewport);
    let map_a = PositionMap::new(&first);
    let map_b = PositionMap::new(&second);

    for char_offset in 0..content.chars().count() {
        assert_eq!(
            map_a.char_to_offset(char_offset),
            map_b.char_to_offset(char_offset),
            "字符 {char_offset} 的映射不确定"
        );
    }
}
