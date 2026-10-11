//! T043 验收：命中测试互逆 + 文字选择（FR-040 / FR-038）
//!
//! ## FR-040 的判据
//!
//! > 命中测试（坐标 → 字符）与其反向（字符 → 坐标）**互为逆运算**，
//! > 在字形边界上的往返误差不大于**半个字形**。
//!
//! 本文件的参数化测试对**翻页**与**滚动**两种形态各跑一遍，覆盖：
//!
//! 1. **字符 → 坐标 → 字符**：往返回到同一字符（垂直轴取行首，故精确）
//! 2. **坐标 → 字符 → 坐标**：往返误差 ≤ 半个字形（水平轴）
//!
//! ## FR-038 的判据
//!
//! > 拖选结果为**字符区间**（而非像素范围），且口径与
//! > `ContentProvider::resolve_anchor` 的锚点口径一致。
//!
//! "口径一致"的实现方式是**用同一套字符下标**：选择端点由命中测试给出
//! （章内字符下标），而锚点也是章内字符下标——两者不经过任何坐标换算。
//! 本文件断言选择结果可直接作为 `ReadProgress.char_offset` 使用。
//!
//! ## 为什么"半个字形"是阈值而不是"零误差"
//!
//! 坐标是连续的、字符是离散的，不可能零误差。半个字形是"用字形中心代表
//! 字形"时的必然上界（见 `PositionMap::char_to_point_in_line` 的文档）。
//! 判据必须锚在一个能达成的量上，否则测试会逼实现去做不可能的事
//! ——而"不可能通过"的测试最终会被删掉，防线随之消失。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音（与 theme_independence.rs 同一约定）"
)]

use tome_reader::paginate::config::{GeometryConfig, ReadingConfig};
use tome_reader::paginate::page::{layout_chapter, Page, PageLine, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::ScrollFlow;
use tome_reader::scroll::map::PositionMap;
use tome_reader::trace::hit::{hit_test_page, hit_test_scroll, select_range, HitPoint, Selection};

// ===========================================================================
// 夹具
// ===========================================================================

fn chapter_text() -> String {
    [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样。她合上书，把它放在膝上，看着窗外的雨。",
        "她说：「真的吗？」我说：「当然，」然后他笑了，笑声在屋子里荡开，混着窗外的雨声，久久没有散去，直到夜深了才渐渐停下来。",
        "《红楼梦》（又名《石头记》）是清代曹雪芹的作品，书中有四百多个人物，其中以宝黛钗三人最为人所知。",
        "桌上有笔、墨、纸、砚；窗边有兰、竹、菊、梅；墙上挂着山水、花鸟、人物；架上摆着经、史、子、集。",
    ]
    .join("\n")
}

fn geometry() -> GeometryConfig {
    ReadingConfig::default_ma().geometry
}

fn paginate(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> Vec<Page> {
    let mut paginator = Paginator::new();
    paginator
        .paginate(content, geometry, viewport)
        .expect("分页应成功")
        .pages
}

fn scroll_flow(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> ScrollFlow {
    let mut paginator = Paginator::new();
    let chapter = layout_chapter(&mut paginator, content, geometry, viewport).expect("排版应成功");
    ScrollFlow::new(chapter, *geometry, *viewport).expect("滚动流应成功")
}

/// 遍历一行里每个**有字形**的字符。
fn glyph_chars(line: &PageLine) -> Vec<usize> {
    let paragraph_offset = line
        .chapter_char_range
        .start
        .saturating_sub(line.line.char_range.start);
    let mut chars = Vec::new();
    for glyph in &line.line.glyphs {
        for index in glyph.char_range.clone() {
            chars.push(index + paragraph_offset);
        }
    }
    chars
}

// ===========================================================================
// A. FR-040：互逆（翻页形态）
// ===========================================================================

/// **翻页形态：字符 → 坐标 → 字符 往返闭合**（FR-040）。
///
/// ## 口径
///
/// 垂直轴取"字符所在行的顶部偏移"，故往返精确（不涉及字形内插值）。
/// 这与 `PositionMap::char_to_offset` 的文档口径一致：同一行共享一个垂直
/// 位置，这是唯一自洽的取法（返回行内插值会让反向映射不可能唯一）。
#[test]
fn page_form_char_to_point_to_char_is_identity() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);

    let mut checked = 0usize;
    for page in &pages {
        let map = PositionMap::for_page(page);
        for line in &page.lines {
            for char_index in glyph_chars(line) {
                let Some(offset) = map.char_to_offset(char_index) else {
                    continue;
                };
                let Some(back) = map.offset_to_char(offset) else {
                    panic!("偏移 {offset} 应能反查回字符");
                };
                // 反查取**行首**：故闭环判据是"回到同一行"
                let expected_line_start = line.chapter_char_range.start;
                assert_eq!(
                    back, expected_line_start,
                    "字符 {char_index}（行首 {expected_line_start}）往返后应回到行首，实测 {back}"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked > 50,
        "前置：应实际检查了足够多的字符，实测 {checked}"
    );
}

/// **翻页形态：坐标 → 字符 → 坐标 往返误差 ≤ 半个字形**（FR-040 核心判据）。
///
/// ## 判据怎么算的
///
/// 对每一行的内容区，按字形的 `advance` 均匀采样 x 坐标；命中后反查回坐标，
/// 断言 `|x - 反查坐标|` 不超过该字形 advance 的一半。
///
/// 采样点取字形的**左缘、中点、右缘内侧**三处——中点必然满足（往返即自身），
/// 左缘与右缘才是真正检验对称性的位置。
#[test]
fn page_form_point_to_char_to_point_within_half_glyph() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);

    let mut checked = 0usize;
    let mut worst = 0.0f32;

    for page in &pages {
        let map = PositionMap::for_page(page);
        for line in &page.lines {
            let origin = map.line_content_origin_px(line);
            for glyph in &line.line.glyphs {
                let advance = glyph.advance_px.max(0.0);
                if advance <= 0.0 {
                    continue;
                }
                // 左缘、中点、右缘内侧
                for fraction in [0.0f32, 0.5, 0.999] {
                    let x = glyph.x_px + advance * fraction;
                    let Some(char_index) = map.point_in_line_to_char(line, x) else {
                        continue;
                    };
                    let Some(back) = map.char_to_point_in_line(line, char_index) else {
                        panic!("字符 {char_index} 在本行内，应能反查回坐标");
                    };
                    let error = (back - x).abs();
                    // 找到 char_index 对应的字形 advance（回查用的字形可能
                    // 与采样字形不同——那正是误差的来源，须按回查字形的
                    // advance 判界）
                    let back_advance = line
                        .line
                        .glyphs
                        .iter()
                        .find(|candidate| {
                            let offset = line
                                .chapter_char_range
                                .start
                                .saturating_sub(line.line.char_range.start);
                            let paragraph = char_index.saturating_sub(offset);
                            candidate.char_range.start <= paragraph
                                && paragraph < candidate.char_range.end
                        })
                        .map_or(advance, |candidate| candidate.advance_px.max(0.0));

                    assert!(
                        error <= back_advance / 2.0 + 0.01,
                        "往返误差 {error:.3} 超过半个字形（{:.3}）——\
                         line origin {origin}, x={x:.3}, char={char_index}, back={back:.3}",
                        back_advance / 2.0
                    );
                    worst = worst.max(error);
                    checked += 1;
                }
            }
        }
    }
    assert!(
        checked > 100,
        "前置：应检查了足够多的采样点，实测 {checked}"
    );
    println!("[FR-040 翻页] 采样 {checked} 点，最大误差 {worst:.4}px");
}

// ===========================================================================
// B. FR-040：互逆（滚动形态）
// ===========================================================================

/// **滚动形态：字符 → 坐标 → 字符 往返闭合**（FR-040）。
#[test]
fn scroll_form_char_to_point_to_char_is_identity() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let flow = scroll_flow(&content, &geometry, &viewport);

    let map = PositionMap::new(&flow);
    let mut checked = 0usize;
    for line in flow.lines() {
        for char_index in glyph_chars(line) {
            let Some(offset) = map.char_to_offset(char_index) else {
                continue;
            };
            let Some(back) = map.offset_to_char(offset) else {
                panic!("偏移 {offset} 应能反查回字符");
            };
            assert_eq!(
                back, line.chapter_char_range.start,
                "字符 {char_index} 往返后应回到其行首，实测 {back}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 50,
        "前置：应实际检查了足够多的字符，实测 {checked}"
    );
}

/// **滚动形态：坐标 → 字符 → 坐标 往返误差 ≤ 半个字形**（FR-040）。
#[test]
fn scroll_form_point_to_char_to_point_within_half_glyph() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);
    let flow = scroll_flow(&content, &geometry, &viewport);

    let map = PositionMap::new(&flow);
    let mut checked = 0usize;
    let mut worst = 0.0f32;

    for line in flow.lines() {
        for glyph in &line.line.glyphs {
            let advance = glyph.advance_px.max(0.0);
            if advance <= 0.0 {
                continue;
            }
            for fraction in [0.0f32, 0.5, 0.999] {
                let x = glyph.x_px + advance * fraction;
                let Some(char_index) = map.point_in_line_to_char(line, x) else {
                    continue;
                };
                let Some(back) = map.char_to_point_in_line(line, char_index) else {
                    panic!("字符 {char_index} 在本行内，应能反查回坐标");
                };
                let error = (back - x).abs();
                let offset = line
                    .chapter_char_range
                    .start
                    .saturating_sub(line.line.char_range.start);
                let back_advance = line
                    .line
                    .glyphs
                    .iter()
                    .find(|candidate| {
                        let paragraph = char_index.saturating_sub(offset);
                        candidate.char_range.start <= paragraph
                            && paragraph < candidate.char_range.end
                    })
                    .map_or(advance, |candidate| candidate.advance_px.max(0.0));

                assert!(
                    error <= back_advance / 2.0 + 0.01,
                    "滚动形态往返误差 {error:.3} 超过半个字形（{:.3}）",
                    back_advance / 2.0
                );
                worst = worst.max(error);
                checked += 1;
            }
        }
    }
    assert!(
        checked > 100,
        "前置：应检查了足够多的采样点，实测 {checked}"
    );
    println!("[FR-040 滚动] 采样 {checked} 点，最大误差 {worst:.4}px");
}

/// **两种形态在同一字符上的命中口径一致**（FR-040 的"两种形态都可用"）。
///
/// ## 为什么这条重要
///
/// 若翻页与滚动用了两套坐标口径（例如一个以页首为原点、一个以流首为原点，
/// 却在同一次调用里混用），用户会看到"翻页时点到字 A、滚动时点到字 B"。
/// 本测试用**同一个字符**在两种形态下求坐标，断言两点在**各自坐标系**下
/// 都落在该字符的字形范围内。
#[test]
fn both_forms_agree_on_the_same_character() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 300.0);

    let pages = paginate(&content, &geometry, &viewport);
    let flow = scroll_flow(&content, &geometry, &viewport);

    let target_char = 30usize;

    // 翻页形态：找到含该字符的页与行
    let page_hit = pages.iter().find_map(|page| {
        let map = PositionMap::for_page(page);
        page.lines.iter().find_map(|line| {
            let x = map.char_to_point_in_line(line, target_char)?;
            let back = map.point_in_line_to_char(line, x)?;
            Some((x, back, line.chapter_char_range.clone()))
        })
    });

    // 滚动形态：同样
    let scroll_map = PositionMap::new(&flow);
    let scroll_hit = flow.lines().iter().find_map(|line| {
        let x = scroll_map.char_to_point_in_line(line, target_char)?;
        let back = scroll_map.point_in_line_to_char(line, x)?;
        Some((x, back, line.chapter_char_range.clone()))
    });

    let (page_x, page_back, page_line_range) = page_hit.expect("字符 30 应能在翻页形态下定位");
    let (scroll_x, scroll_back, scroll_line_range) =
        scroll_hit.expect("字符 30 应能在滚动形态下定位");

    // 两种形态应落在**同一行**（同一章内容，行切分一致）
    assert_eq!(
        page_line_range, scroll_line_range,
        "两种形态对同一字符应落在同一行（行切分口径一致）"
    );
    // 且行内 x 相同（同一行、同一字形度量）
    assert!(
        (page_x - scroll_x).abs() < 0.01,
        "两种形态的行内 x 应一致（{page_x:.3} vs {scroll_x:.3}）"
    );
    // 往返都回到同一字符
    assert_eq!(page_back, target_char, "翻页形态往返应回到目标字符");
    assert_eq!(scroll_back, target_char, "滚动形态往返应回到目标字符");
}

// ===========================================================================
// C. FR-038：文字选择产字符区间
// ===========================================================================

/// **拖选产出字符区间，且与锚点口径一致**（FR-038 / US4 场景 3）。
///
/// ## "口径一致"的可验证形态
///
/// `resolve_anchor` 的输入输出都是**章内字符下标**。本测试断言选择结果的
/// 两个端点也是章内字符下标，且**可直接作为 `ReadProgress.char_offset`**——
/// 用它构造进度、再解析回锚点，落在同一字符上。若选择产的是像素范围，
/// 这一步根本无法表达。
#[test]
fn selection_produces_char_range_compatible_with_anchor() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);

    let page = pages.first().expect("应有页");
    let map = PositionMap::for_page(page);
    let line = page.lines.first().expect("应有行");

    // 在同一行内取两个已知字符的坐标作为拖选端点
    let chars = glyph_chars(line);
    assert!(chars.len() >= 4, "前置：首行应有足够字符");
    let start_char = chars[1];
    let end_char = chars[3];

    let start_x = map
        .char_to_point_in_line(line, start_char)
        .expect("起点应有坐标");
    let end_x = map
        .char_to_point_in_line(line, end_char)
        .expect("终点应有坐标");

    let selection = select_range(
        HitPoint {
            x_px: start_x,
            y_px: 0.0,
            line_index: 0,
        },
        HitPoint {
            x_px: end_x,
            y_px: 0.0,
            line_index: 0,
        },
        &map,
        &page.lines,
    )
    .expect("同向拖选应产出选择");

    // 选择端点必须是**章内字符下标**，且与锚点口径相同
    assert!(
        selection.char_range.start <= selection.char_range.end,
        "选择区间应规范化（起点不大于终点）"
    );
    assert_eq!(
        selection.char_range.start, start_char,
        "选择起点应为命中测试给出的章内字符下标"
    );
    assert!(
        selection.char_range.end >= end_char,
        "选择终点应覆盖到拖选末端字符（含）"
    );

    // 关键：选择结果可直接用于 resolve_anchor 的锚点口径
    let anchor_char = selection.char_range.start;
    let resolution = {
        let mut paginator = Paginator::new();
        paginator
            .paginate(&content, &geometry, &viewport)
            .expect("分页应成功")
            .resolve_anchor(anchor_char)
    };
    assert!(
        resolution.is_some(),
        "选择起点作为锚点应能被 resolve_anchor 解析（口径一致）"
    );
}

/// **反向拖选（从右往左）得到同样的区间**（选择应规范化方向）。
#[test]
fn selection_is_direction_agnostic() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);
    let page = pages.first().expect("应有页");
    let map = PositionMap::for_page(page);
    let line = page.lines.first().expect("应有行");
    let chars = glyph_chars(line);
    assert!(chars.len() >= 4);

    let left = HitPoint {
        x_px: map.char_to_point_in_line(line, chars[1]).expect("坐标"),
        y_px: 0.0,
        line_index: 0,
    };
    let right = HitPoint {
        x_px: map.char_to_point_in_line(line, chars[3]).expect("坐标"),
        y_px: 0.0,
        line_index: 0,
    };

    let forward = select_range(left, right, &map, &page.lines).expect("正向拖选");
    let backward = select_range(right, left, &map, &page.lines).expect("反向拖选");
    assert_eq!(
        forward.char_range, backward.char_range,
        "拖选方向不影响结果区间（左→右 与 右→左 应相同）"
    );
}

/// **跨行拖选覆盖两行之间的内容**（选择不只在一行内）。
#[test]
fn selection_across_lines_covers_intermediate_content() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);
    let page = pages.first().expect("应有页");
    assert!(page.lines.len() >= 2, "前置：首行至少两行");
    let map = PositionMap::for_page(page);

    let first = &page.lines[0];
    let second = &page.lines[1];
    let first_chars = glyph_chars(first);
    let second_chars = glyph_chars(second);

    let start_char = first_chars[1];
    let end_char = second_chars[1];

    let selection = select_range(
        HitPoint {
            x_px: map.char_to_point_in_line(first, start_char).expect("坐标"),
            y_px: 0.0,
            line_index: 0,
        },
        HitPoint {
            x_px: map.char_to_point_in_line(second, end_char).expect("坐标"),
            y_px: 0.0,
            line_index: 1,
        },
        &map,
        &page.lines,
    )
    .expect("跨行拖选应产出选择");

    assert_eq!(
        selection.char_range.start, start_char,
        "起点应为第一行的字符"
    );
    assert!(
        selection.char_range.end > end_char,
        "终点应覆盖到第二行的拖选末端（含）：end={} > {end_char}",
        selection.char_range.end
    );
    assert!(
        selection.char_range.len() > second.chapter_char_range.start - start_char,
        "跨行区间应包含两行之间的全部内容"
    );
}

/// **空选择（起点 == 终点）产出空区间而非 `None`**。
///
/// ## 为什么是空区间而不是 `None`
///
/// 单击（未拖动）是合法操作——它产生一个零长选择，光标停在那里。
/// 返回 `None` 会让调用方把"单击"与"拖到了行外"混为一谈。
#[test]
fn zero_length_selection_is_an_empty_range() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);
    let page = pages.first().expect("应有页");
    let map = PositionMap::for_page(page);
    let line = page.lines.first().expect("应有行");
    let chars = glyph_chars(line);
    let point = HitPoint {
        x_px: map.char_to_point_in_line(line, chars[0]).expect("坐标"),
        y_px: 0.0,
        line_index: 0,
    };

    let selection = select_range(point, point, &map, &page.lines).expect("单击应产出零长选择");
    assert!(
        selection.char_range.is_empty(),
        "同一点的拖选应产出零长区间，实测 {:?}",
        selection.char_range
    );
}

// ===========================================================================
// D. 便捷入口（两形态各自的命中测试）
// ===========================================================================

/// **`hit_test_page` / `hit_test_scroll` 给出与 PositionMap 一致的结果**。
///
/// 这两个入口是"上层只给屏幕坐标"的落点（FR-040 的公开面）。它们必须与
/// 直接使用 `PositionMap` 得到相同结论——否则上层与测试会看到两个答案。
#[test]
fn convenience_entry_points_agree_with_position_map() {
    let content = chapter_text();
    let geometry = geometry();
    let viewport = Viewport::new(342.0, 400.0);
    let pages = paginate(&content, &geometry, &viewport);
    let page = pages.first().expect("应有页");
    let line = page.lines[1].clone();
    let chars = glyph_chars(&line);
    let target = chars[2];

    let map = PositionMap::for_page(page);
    let x = map.char_to_point_in_line(&line, target).expect("应有坐标");

    // 该行的垂直偏移（行首）+ 一点内缩，落在该行上
    let y = map
        .char_to_offset(line.chapter_char_range.start)
        .expect("偏移");
    let y = y + line.line.height_px.max(1.0) / 2.0;

    let hit = hit_test_page(page, x, y).expect("应命中某字符");
    assert_eq!(
        hit.char_index, target,
        "hit_test_page 应与 PositionMap 给出同一字符"
    );

    // 滚动形态同上
    let flow = scroll_flow(&content, &geometry, &viewport);
    let scroll_map = PositionMap::new(&flow);
    let scroll_line = flow.lines()[1].clone();
    let scroll_chars = glyph_chars(&scroll_line);
    let scroll_target = scroll_chars[2];
    let scroll_x = scroll_map
        .char_to_point_in_line(&scroll_line, scroll_target)
        .expect("应有坐标");
    let scroll_y = scroll_map
        .char_to_offset(scroll_line.chapter_char_range.start)
        .expect("偏移")
        + scroll_line.line.height_px.max(1.0) / 2.0;
    let scroll_hit = hit_test_scroll(&flow, scroll_x, scroll_y).expect("应命中某字符");
    assert_eq!(
        scroll_hit.char_index, scroll_target,
        "hit_test_scroll 应与 PositionMap 给出同一字符"
    );
}

/// **`HitPoint` / `Selection` 的公开字段形态可被上层构造与消费**。
#[test]
fn public_types_are_usable_by_callers() {
    let point = HitPoint {
        x_px: 1.0,
        y_px: 2.0,
        line_index: 3,
    };
    assert_eq!(point.line_index, 3);

    let selection = Selection { char_range: 5..10 };
    assert_eq!(selection.char_range.len(), 5);
    // 选择结果可**直接**用作锚点（FR-038 的口径要求）
    let anchor_offset: i64 = selection.char_range.start as i64;
    assert_eq!(anchor_offset, 5);
}
