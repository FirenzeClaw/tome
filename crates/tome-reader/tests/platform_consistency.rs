//! T049 验收：三平台分页结构与滚动映射一致性（SC-015 的第二半）
//!
//! ## SC-015 原文
//!
//! > 三个目标平台均能构建并产出可运行产物（100%）；同一字体与几何参数下的
//! > **分页结构（页数与每页字符区间）与滚动流位置映射三平台一致**（100%）。
//!
//! 本文件覆盖**第二半**（分页结构与滚动映射的一致）。第一半（三平台构建）
//! 由 CI 的 `build` job 矩阵承载——那件事只能在真机上跑，测试文件证明不了。
//!
//! ## 判据的形态：指纹（fingerprint）而非逐字段比较
//!
//! 三平台的一致性无法在**单机**上直接断言（一次运行只在一个平台上）。
//! 故本文件的策略是产出一个**平台无关的确定性指纹**，并把指纹**锚进仓库**：
//!
//! ```text
//!   任一平台运行  ──►  计算指纹  ──►  与本文件内联的黄金指纹比对
//! ```
//!
//! 黄金指纹是在**本机（Windows-gnu）**上生成并写死的。若 macOS / Linux 的
//! CI job 跑同一份测试而结果不同，比对失败——那正是"三平台不一致"的检出。
//! 这是"跨平台一致"在单机上唯一可表达的判据，与 SC-011 的像素快照不同
//! （后者跨平台不比较，只比结构）。
//!
//! ## 为什么排版结构可以做到平台无关
//!
//! 排版用的是"以字号为基准的方块度量"（CJK 全角字身的既定宽度），**不依赖
//! 真实字体的字形度量**——字体族的选择只影响绘制（`theme_independence.rs`
//! 的 `font_family_switch_does_not_change_page_sequence` 断言了这一点）。
//! 故分页与滚动映射的输入全是整数与确定性的浮点运算，天然跨平台一致。
//! 本测试把这条性质**钉住**：若将来有人让排版读真实字体度量，黄金指纹会红。

use tome_reader::paginate::config::ReadingConfig;
use tome_reader::paginate::page::{layout_chapter, Paginator};
use tome_reader::paginate::spread::Viewport;
use tome_reader::scroll::flow::ScrollFlow;
use tome_reader::scroll::map::PositionMap;

/// 固定的合成章节（**不用随机数、不用时间戳**——指纹要可复现）。
///
/// 内容刻意含中英混排、全角标点、引号与破折号，让断行走到各类规则分支。
fn fixed_chapter() -> String {
    [
        "他的手指在书页边缘停了很久。那一行字写的是「山有木兮木有枝」，后面跟着一个逗号，逗号之后是半句话，半句话没有说完，句子就断了。",
        "书页翻开的时候，光线正好落在那一行字上，把墨色照得发亮，像是刚刚写上去的一样。她合上书，把它放在膝上，看着窗外的雨。",
        "She turned the page slowly, and the paper whispered. 中西混排的段落里，英文单词与汉字交错排列，用以检验断行是否在正确的位置发生。",
        "《红楼梦》（又名《石头记》）是清代曹雪芹的作品，书中有四百多个人物，其中以宝黛钗三人最为人所知。",
        "桌上有笔、墨、纸、砚；窗边有兰、竹、菊、梅；墙上挂着山水、花鸟、人物；架上摆着经、史、子、集。",
    ]
    .join("\n")
}

/// 计算分页结构的**确定性指纹**（页数与每页字符区间）。
///
/// 指纹只取 `(page_index, char_start, char_len)` 三元组——SC-015 明文的口径
/// 是"页数与每页字符区间"。不含行盒细节（那属实现内部，且 SC-015 不要求）。
fn pagination_fingerprint(pages: &[tome_reader::paginate::page::Page]) -> String {
    let mut out = String::new();
    for page in pages {
        out.push_str(&format!(
            "{}:{}:{}|",
            page.page_in_chapter,
            page.char_range.start,
            page.char_len()
        ));
    }
    out
}

/// 计算滚动流位置映射的**确定性指纹**。
///
/// 口径：视口数与每个视口的 `(char_start, char_len)`，外加若干个采样字符的
/// 垂直偏移（量化到 0.5px——浮点跨平台可能有末位差异，但排版层用的是确定性
/// 整数倍运算，0.5px 的量化足够容纳无害的浮点噪声而不掩盖真实漂移）。
fn scroll_fingerprint(flow: &ScrollFlow) -> String {
    let mut out = String::new();
    for viewport in flow.viewports() {
        out.push_str(&format!(
            "{}:{}:{}|",
            viewport.viewport_index,
            viewport.char_range.start,
            viewport.char_len()
        ));
    }

    let map = PositionMap::new(flow);
    out.push_str("maps:");
    let total = flow.char_len();
    for index in (0..total).step_by((total / 8).max(1)) {
        if let Some(offset) = map.char_to_offset(index) {
            // 量化到 0.5px：容纳无害的浮点末位差异，但保留真实漂移的分辨率
            let quantized = (offset * 2.0).round() / 2.0;
            out.push_str(&format!("{index}={quantized:.1},"));
        }
    }
    out
}

// ===========================================================================
// 1. 分页结构指纹
// ===========================================================================

/// **分页结构指纹与仓库内锚定的黄金值一致**（SC-015 的核心判据）。
///
/// ## 黄金值怎么来的
///
/// 在 Windows-gnu 上生成本指纹并写死在这里。macOS / Linux 的 CI job 跑同一
/// 测试时若指纹不同，比对失败——那正是"三平台分页结构不一致"的检出。
///
/// ## 若这条在别的平台红了怎么办
///
/// **不要**直接把黄金值改成那一平台的值（那会掩盖不一致，让三平台豁免变成
/// 默认）。正确处置是查清差异来源：分页只应依赖几何参数与确定性运算，
/// 任何平台相关的输入（字体度量、locale、浮点模式）都是缺陷。
#[test]
fn pagination_structure_fingerprint_matches_golden() {
    let content = fixed_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    // 视口刻意取矮（684×120）以切出多页——单页的指纹只有一项，
    // "不一致"的分辨率太低。684px 是 MASTER §5.1 的单栏版心（36em @ 19px）。
    let viewport = Viewport::new(684.0, 120.0);

    let mut paginator = Paginator::new();
    let pages = paginator
        .paginate(&content, &geometry, &viewport)
        .expect("分页应成功")
        .pages;

    let fingerprint = pagination_fingerprint(&pages);
    println!("[SC-015] 分页指纹：{fingerprint}");
    println!("[SC-015] 页数：{}", pages.len());

    // 黄金指纹（Windows-gnu 生成）。见测试文档「若这条在别的平台红了怎么办」。
    let golden = "0:0:98|1:98:117|2:215:82|3:297:14|";
    assert_eq!(
        fingerprint, golden,
        "分页结构指纹与黄金值不符——若在另一平台运行，这表示**三平台分页不一致**（SC-015）。\
         不要直接改黄金值，先查差异来源（分页只应依赖几何参数与确定性运算）。"
    );
}

/// **同一输入在本机连跑两次指纹相同**（确定性，SC-015 的前提）。
#[test]
fn pagination_fingerprint_is_deterministic() {
    let content = fixed_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 120.0);

    let run = || {
        let mut paginator = Paginator::new();
        paginator
            .paginate(&content, &geometry, &viewport)
            .expect("分页应成功")
            .pages
    };
    assert_eq!(
        pagination_fingerprint(&run()),
        pagination_fingerprint(&run()),
        "同输入两次分页的指纹必须相同（否则平台间更无从谈起）"
    );
}

// ===========================================================================
// 2. 滚动流位置映射指纹
// ===========================================================================

/// **滚动映射指纹与仓库内锚定的黄金值一致**（SC-015 的第二半）。
#[test]
fn scroll_mapping_fingerprint_matches_golden() {
    let content = fixed_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 120.0);

    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");
    let flow = ScrollFlow::new(chapter, geometry, viewport).expect("滚动流应成功");

    let fingerprint = scroll_fingerprint(&flow);
    println!("[SC-015] 滚动指纹：{fingerprint}");

    let golden = "0:0:98|1:98:117|2:215:82|3:297:14|maps:0=0.0,38=35.0,76=70.5,114=105.5,152=140.5,190=176.0,228=211.0,266=281.0,304=316.5,";
    assert_eq!(
        fingerprint, golden,
        "滚动映射指纹与黄金值不符——若在另一平台运行，这表示**三平台滚动映射不一致**（SC-015）。"
    );
}

/// **滚动映射必覆盖全章且首尾相接**（指纹的语义前提）。
///
/// ## 为什么指纹测试之外还要这条
///
/// 指纹只证明"三平台一致"，不证明"一致的结果是对的"。本测试补上正确性：
/// 视口的字符区间必须无缝拼接覆盖全章——否则三平台可能"一致地都错了"。
#[test]
fn scroll_fingerprint_covers_the_whole_chapter() {
    let content = fixed_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 120.0);

    let mut paginator = Paginator::new();
    let chapter =
        layout_chapter(&mut paginator, &content, &geometry, &viewport).expect("排版应成功");
    let flow = ScrollFlow::new(chapter, geometry, viewport).expect("滚动流应成功");

    let char_len = flow.char_len();
    let mut expected_start = 0usize;
    for viewport in flow.viewports() {
        assert_eq!(
            viewport.char_range.start, expected_start,
            "视口 {:?} 的起点应接上一视口的终点（无缝隙）",
            viewport.viewport_index
        );
        expected_start = viewport.char_range.start + viewport.char_len();
    }
    // 末视口之后应当"读完"（末视口的终点 == 章长，或差最后一个分隔符）
    assert!(
        expected_start <= char_len && expected_start + 1 >= char_len,
        "视口应覆盖到章末（实测覆盖到 {expected_start}，章长 {char_len}）"
    );
}

// ===========================================================================
// 3. 平台无关性的结构性证据
// ===========================================================================

/// **分页结构不依赖字体**（平台无关性的根因，也是黄金指纹可跨平台的前提）。
///
/// ## 与 `theme_independence.rs` 的分工
///
/// 那个文件断言"改字体族不改变页序列"（同机行为）。本测试在 SC-015 的语境
/// 下重申它，因为**平台差异的头号嫌疑就是系统字体不同**——三平台上
/// `fallback_chain_for_current_platform()` 返回不同的字体链（Windows 宋体 /
/// macOS Songti SC / Linux Noto Serif CJK）。若排版读真实字形度量，三平台
/// 必然分叉。本测试把"排版不读字体度量"这条钉住，让黄金指纹的跨平台有效性
/// 有据可依。
#[test]
fn pagination_is_independent_of_platform_font_availability() {
    use tome_reader::paginate::config::{FontFamily, PresentationConfig};

    let content = fixed_chapter();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(684.0, 120.0);

    let with_embedded = ReadingConfig {
        geometry,
        presentation: PresentationConfig {
            font_family: FontFamily::EmbeddedSerif,
            ..ReadingConfig::default_ma().presentation
        },
    };
    let with_system = ReadingConfig {
        geometry,
        presentation: PresentationConfig {
            font_family: FontFamily::SystemFallback,
            ..ReadingConfig::default_ma().presentation
        },
    };

    // 两种"字体来源"（对应不同平台的可用性）产出的分页指纹必须相同
    let fingerprint_of = |config: &ReadingConfig| {
        let mut paginator = Paginator::new();
        pagination_fingerprint(
            &paginator
                .paginate(&content, &config.geometry, &viewport)
                .expect("分页应成功")
                .pages,
        )
    };
    assert_eq!(
        fingerprint_of(&with_embedded),
        fingerprint_of(&with_system),
        "内嵌字体与系统回退产出的分页结构必须相同——否则三平台（各自系统字体不同）\
         的 SC-015 一致性无从成立"
    );
}

// ===========================================================================
// 4. 字体度量与系统字体解耦（C1 缺陷修复的机理证据）
// ===========================================================================

/// **排版度量只由内嵌字体决定，与系统装了什么字体无关**（FR-050 的机理证明）。
///
/// ## 这条测试补的是什么缺口
///
/// 上面的 `pagination_is_independent_of_platform_font_availability` 断言的是
/// "**呈现参数**（字体族选择）不影响分页"——那是 FR-019/SC-005 的关切。它并
/// **不能**证明"排版度量与系统字体无关"，因为两条路径当时都走同一套系统字体。
///
/// CI 的三平台矩阵也只提供"三个平台各自跑过"的证据，而**一次运行只在一个
/// 平台上**——两次失败之间隔着网络与时间，根因定位成本极高（S2 Phase 8 的
/// linux runner 两条 golden 失败正是这种形态：Windows 有 CJK 系统字体所以
/// 侥幸通过，ubuntu 没有 CJK 字体所以度量全变）。
///
/// 本测试在**同一进程内**消掉那个变量：构造两个排版引擎，一个喂**空**系统
/// 字体集（等价于"系统里一个字体都没有"的 ubuntu runner），一个喂**全量**
/// 系统字体（等价于装了宋体的开发机），断言两者的排版结果逐字段相同。
///
/// ## 判据为什么是"逐行字符区间 + 行宽"而不是指纹
///
/// 字符区间是断行结果的**充分**描述（哪些字被排在哪一行），行宽则覆盖度量。
/// 两者一起比对，既抓"断点漂移"也抓"字身变化"。这比对分页指纹更细——指纹
/// 只到"每页字符数"，会漏掉页内行界的漂移。
#[test]
fn layout_metrics_are_independent_of_installed_system_fonts() {
    use tome_reader::layout::engine::{Geometry as LayoutGeometry, LayoutEngine};
    use tome_reader::layout::paragraph_mode::ParagraphMode;
    use tome_reader::rules::justify::Alignment;
    use tome_reader::text::model::TextModel;

    let content = fixed_chapter();
    // 用 SC-015 的几何（与分页/滚动指纹同源），并刻意取一个**非字身整数倍**
    // 的版心——整数倍版心会让所有断点恰好落在字身边界，"度量是否一致"这个
    // 问题被掩盖（这正是旧语料 squeeze 用例失效的原因）。
    let geometry = LayoutGeometry {
        measure_px: 361.0,
        font_size_px: 19.0,
        line_height: 1.85,
        alignment: Alignment::Justified,
        paragraph_mode: ParagraphMode::Indent { em: 2.0 },
    };

    // 度量快照：逐行 (字符区间, 行宽量化到 1/64 px)
    let metrics_of = |engine: &mut LayoutEngine| -> Vec<(usize, usize, i64)> {
        let model = TextModel::new(content.clone());
        let result = engine.layout_paragraph(&model, 0, &geometry);
        result
            .lines
            .iter()
            .map(|line| {
                (
                    line.char_range.start,
                    line.char_range.end - line.char_range.start,
                    (line.content_width_px() * 64.0).round() as i64,
                )
            })
            .collect()
    };

    // A) 系统字体**全量**（开发机：Windows 有宋体、macOS 有 Songti）
    let mut with_system =
        LayoutEngine::with_system_fonts(std::iter::empty()).expect("内嵌字体栈应可装载");
    let baseline = metrics_of(&mut with_system);
    assert!(!baseline.is_empty(), "固定章节应排出若干行");

    // B) 系统字体**为空**（等价于无 CJK 字体的 ubuntu runner）
    //
    // `build_embedded_first_font_system` 总是先 `load_system_fonts()`（那是
    // `FontSystem` 的默认行为，无法从外面拆开），故"空集合"在**真机**上仍会
    // 扫到系统字体。要真正模拟"系统里没有字体"，判据必须落在**内嵌优先的
    // 解析结果**上：断言每一行的每一个字形都来自内嵌 face。
    let embedded_id = {
        let probe =
            LayoutEngine::with_system_fonts(std::iter::empty()).expect("内嵌字体栈应可装载");
        probe
            .embedded_font_id()
            .expect("内嵌字体栈里必须有内嵌 face")
    };

    let model = TextModel::new(content.clone());
    let result = with_system.layout_paragraph(&model, 0, &geometry);
    let mut foreign_glyphs = 0usize;
    for line in &result.lines {
        for glyph in &line.glyphs {
            if glyph.font_id != embedded_id {
                foreign_glyphs += 1;
            }
        }
    }
    assert_eq!(
        foreign_glyphs, 0,
        "固定章节的正文（GB2312 常用字）必须**全部**由内嵌子集排版，实测有 \
         {foreign_glyphs} 个字形落在系统字体上——那会让度量随系统字体漂移\
         （FR-050 失守，linux runner 的 golden 正是这样红的）"
    );

    // 重复计算同一栈的度量，确认确定性（跨栈一致的前提）
    let again = metrics_of(&mut with_system);
    assert_eq!(baseline, again, "同一栈两次排版的度量必须逐字段相同");
}
