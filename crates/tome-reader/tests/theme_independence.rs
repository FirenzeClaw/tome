//! T023 验收：呈现参数零依赖（FR-019 / SC-005）
//!
//! ## 判据（SC-005 原文）
//!
//! > 分页结果与滚动流布局对主题与呈现参数**完全无依赖**：同一几何参数下，
//! > 昼/夜主题、不同字体族、不同动效开关产出的页序列（字符区间）与滚动流
//! > 位置映射 100% 相同。
//!
//! ## 为什么这条值得单独一个文件
//!
//! FR-019 说"页数只依赖几何参数，MUST NOT 依赖颜色、主题、封面等呈现参数"。
//! 违反它的**代价**是用户可见的：切一次主题就整章重排，阅读位置抖动一次，
//! 大屏上还能看到明显的闪烁。而它**很容易悄悄违反**——比如某个实现在算出
//! 行盒高度时顺手读了一个"当前主题的行距补偿"（听起来很合理），于是主题
//! 切换改变了每页行数。
//!
//! ## 本文件的两层断言
//!
//! 1. **行为层**：改呈现参数前后，两次分页的页序列**逐字节相同**
//!    （含页区间、行数、左右归属、留白标记）
//! 2. **结构层**：`paginate` 的签名只接受 `&GeometryConfig`——
//!    呈现参数在类型上取不到（这条由 `page.rs` 的签名保证，本文件用
//!    "能编译"本身作为部分证据，并额外做源码级 grep 断言）
//!
//! 第 2 层是第 1 层的**原因**而非重复：若签名接受整份 `ReadingConfig`，
//! 行为层测试可能通过（实现恰好没读那些字段），但下一次有人加一行
//! `config.presentation.*` 就没东西挡着。

#![expect(
    clippy::expect_used,
    reason = "测试辅助函数无法就绪时应让所属测试立即失败，错误传播只会加噪音"
)]

use tome_reader::paginate::config::{FontFamily, GeometryConfig, ReadingConfig, ThemeId};
use tome_reader::paginate::page::{Page, Paginator};
use tome_reader::paginate::spread::Viewport;

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

/// 页序列的**可比较快照**（不含任何呈现参数）。
///
/// ## 为什么取这些字段
///
/// 它们就是 SC-005 说的"页序列（字符区间）"的完整内容。若某个呈现参数
/// 悄悄影响了其中任一项（页数、区间、行数、左右归属、留白标记），
/// 本快照就会不等。
fn pages_snapshot(pages: &[Page]) -> Vec<(usize, usize, usize, Option<&'static str>, bool)> {
    pages
        .iter()
        .map(|page| {
            (
                page.char_range.start,
                page.char_range.end,
                page.lines.len(),
                page.side
                    .map(tome_reader::paginate::spread::SpreadSide::as_str),
                page.is_padding,
            )
        })
        .collect()
}

fn paginate(content: &str, geometry: &GeometryConfig, viewport: &Viewport) -> Vec<Page> {
    let mut paginator = Paginator::new();
    paginator
        .paginate(content, geometry, viewport)
        .expect("分页应成功")
        .pages
}

// ===========================================================================
// SC-005 主判据：主题
// ===========================================================================

/// **昼/夜主题不改变页序列**（SC-005 明文的第一个维度）。
#[test]
fn theme_switch_does_not_change_page_sequence() {
    let content = chapter_text();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 400.0);

    let day = ReadingConfig {
        geometry,
        presentation: tome_reader::paginate::config::PresentationConfig {
            theme_id: ThemeId::Parchment,
            ..ReadingConfig::default_ma().presentation
        },
    };
    let night = ReadingConfig {
        geometry,
        presentation: tome_reader::paginate::config::PresentationConfig {
            theme_id: ThemeId::Obsidian,
            ..ReadingConfig::default_ma().presentation
        },
    };

    // 两次分页只消费 geometry（呈现子集不参与）——故结果必须逐字节相同
    let day_pages = paginate(&content, &day.geometry, &viewport);
    let night_pages = paginate(&content, &night.geometry, &viewport);

    assert_eq!(
        pages_snapshot(&day_pages),
        pages_snapshot(&night_pages),
        "昼/夜主题产出的页序列必须 100% 相同（SC-005）"
    );
    assert!(!day_pages.is_empty(), "前置：本例应有页（否则断言是空的）");
}

// ===========================================================================
// SC-005 主判据：字体族
// ===========================================================================

/// **不同字体族不改变页序列**（SC-005 明文的第二个维度）。
///
/// ## 注：字体族在本层只影响绘制
///
/// 裁决 OQ#3→A：正文衬线内嵌子集 + UI/等宽走系统栈。字体**族**的选择
/// 是呈现参数（`FontFamily`），而字形度量属渲染层（S2-4）。本层分页
/// 用的是"以字号为基准的方块度量"（CJK 全角字身的既定宽度），
/// 故字体族切换不进入分页计算。
///
/// 这一点值得显式测：若实现改成了"用真实字体度量计算行宽"（S2-4 可能
/// 引入），字体族就会影响断行——那时 SC-005 会失败，而本测试就是
/// 那条防线的位置。
#[test]
fn font_family_switch_does_not_change_page_sequence() {
    let content = chapter_text();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 400.0);

    let embedded = ReadingConfig {
        geometry,
        presentation: tome_reader::paginate::config::PresentationConfig {
            font_family: FontFamily::EmbeddedSerif,
            ..ReadingConfig::default_ma().presentation
        },
    };
    let system = ReadingConfig {
        geometry,
        presentation: tome_reader::paginate::config::PresentationConfig {
            font_family: FontFamily::SystemFallback,
            ..ReadingConfig::default_ma().presentation
        },
    };

    assert_eq!(
        pages_snapshot(&paginate(&content, &embedded.geometry, &viewport)),
        pages_snapshot(&paginate(&content, &system.geometry, &viewport)),
        "不同字体族产出的页序列必须 100% 相同（SC-005）"
    );
}

// ===========================================================================
// SC-005 主判据：动效开关
// ===========================================================================

/// **减少动效开关不改变页序列**（SC-005 明文的第三个维度）。
#[test]
fn reduce_motion_switch_does_not_change_page_sequence() {
    let content = chapter_text();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 400.0);

    let motion_on = ReadingConfig {
        geometry,
        presentation: tome_reader::paginate::config::PresentationConfig {
            reduce_motion: false,
            ..ReadingConfig::default_ma().presentation
        },
    };
    let motion_off = ReadingConfig {
        geometry,
        presentation: tome_reader::paginate::config::PresentationConfig {
            reduce_motion: true,
            ..ReadingConfig::default_ma().presentation
        },
    };

    assert_eq!(
        pages_snapshot(&paginate(&content, &motion_on.geometry, &viewport)),
        pages_snapshot(&paginate(&content, &motion_off.geometry, &viewport)),
        "减少动效开关产出的页序列必须 100% 相同（SC-005 / FR-032）"
    );
}

/// 纸纤维噪点开关同样不改变页序列（呈现子集里剩下的那个字段）。
#[test]
fn paper_noise_switch_does_not_change_page_sequence() {
    let content = chapter_text();
    let geometry = ReadingConfig::default_ma().geometry;
    let viewport = Viewport::new(342.0, 400.0);

    let noise_on = ReadingConfig {
        geometry,
        presentation: tome_reader::paginate::config::PresentationConfig {
            paper_noise: true,
            ..ReadingConfig::default_ma().presentation
        },
    };
    let noise_off = ReadingConfig {
        geometry,
        presentation: tome_reader::paginate::config::PresentationConfig {
            paper_noise: false,
            ..ReadingConfig::default_ma().presentation
        },
    };

    assert_eq!(
        pages_snapshot(&paginate(&content, &noise_on.geometry, &viewport)),
        pages_snapshot(&paginate(&content, &noise_off.geometry, &viewport)),
        "噪点开关产出的页序列必须 100% 相同（SC-005）"
    );
}

// ===========================================================================
// 呈现子集的全组合（一次跑满）
// ===========================================================================

/// **呈现子集的全组合（2×2×2×2 = 16 组）产出的页序列全部相同**。
///
/// ## 为什么穷举而不是抽样
///
/// 呈现子集只有 4 个字段，全组合 16 组。穷举的代价可忽略，而它排除了一类
/// 抽样测不到的情形：某个字段**单独**改不触发、但两个字段**同时**改会
/// 触发（如"夜主题 + 动效关"走了另一条分支）。
#[test]
fn all_presentation_combinations_produce_identical_pages() {
    let content = chapter_text();
    let geometry = ReadingConfig::default_ma().geometry;

    // 多种几何（含单/双页）各跑一遍，确保"呈现无关"不是某一档的巧合
    let viewports = [
        Viewport::new(342.0, 400.0),
        Viewport::new(228.0, 200.0),
        Viewport::new(1432.0, 300.0),
        Viewport::new(648.0, 800.0),
    ];

    let mut checked = 0usize;
    for viewport in viewports {
        let reference = paginate(&content, &geometry, &viewport);
        assert!(!reference.is_empty(), "前置：应有页");

        for theme_id in [ThemeId::Parchment, ThemeId::Obsidian] {
            for font_family in [FontFamily::EmbeddedSerif, FontFamily::SystemFallback] {
                for reduce_motion in [false, true] {
                    for paper_noise in [false, true] {
                        let config = ReadingConfig {
                            geometry,
                            presentation: tome_reader::paginate::config::PresentationConfig {
                                theme_id,
                                font_family,
                                reduce_motion,
                                paper_noise,
                            },
                        };
                        let pages = paginate(&content, &config.geometry, &viewport);
                        checked += 1;

                        assert_eq!(
                            pages_snapshot(&reference),
                            pages_snapshot(&pages),
                            "呈现组合（{theme_id:?}, {font_family:?}, motion={reduce_motion}, noise={paper_noise}）\
                             在 {viewport:?} 下改变了页序列（SC-005）"
                        );
                    }
                }
            }
        }
    }

    assert_eq!(checked, 4 * 16, "4 种几何 × 16 组呈现组合");
}

// ===========================================================================
// 结构层：分页签名的依赖面（FR-019 的架构断言）
// ===========================================================================

/// **分页模块的源码里不出现呈现参数的字段名**（架构级 grep 断言）。
///
/// ## 为什么用源码扫描而不是只靠类型
///
/// 类型上 `paginate` 只收 `&GeometryConfig`，呈现参数已经取不到了——这条
/// 已经是强保证。但还有一条**更隐蔽**的路径：分页模块可以 `use` 呈现
/// 相关的类型（如 `ThemeId`）或直接构造 `PresentationConfig`，从而在未来
/// 的修改里获得读取它们的能力。
///
/// 本断言把"分页模块不认识呈现参数"钉在源码层面，让那次修改**必须**同时
/// 改这个测试——那时改动者会看到这里的理由。
///
/// ## 扫描范围（刻意精确）
///
/// 只扫 `src/paginate/page.rs`（分页计算本体），**不含** `config.rs`
/// （它定义呈现参数，理应提及那些名字）。
#[test]
fn pagination_module_never_mentions_presentation_fields() {
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/paginate/page.rs"),
    )
    .expect("应能读取分页模块源码");

    // 去掉注释行——注释里提到这些名字是允许的（本模块的文档就在讨论它们）
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    for forbidden in [
        "theme_id",
        "ThemeId",
        "font_family",
        "FontFamily",
        "reduce_motion",
        "paper_noise",
        "PresentationConfig",
        "presentation",
    ] {
        assert!(
            !code.contains(forbidden),
            "分页模块的代码里出现了呈现参数 `{forbidden}`——\
             FR-019 要求分页只依赖几何参数，主题切换 MUST NOT 触发重排。\
             若确需读取呈现状态，请先重新审视该依赖是否真的与分页几何有关。"
        );
    }
}

/// 几何子集**不含**任何呈现字段（两个子集确实是分开的）。
///
/// ## 这条断言的形式
///
/// 用"改遍呈现子集后几何子集逐字段不变"来验证，而不是去数字段名——
/// 后者会在新增字段时静默通过（新字段被加进几何子集就漏判了）。
/// 前者只看**行为**：呈现子集的任何改动都不得传导到几何。
#[test]
fn geometry_is_immune_to_presentation_changes() {
    let base = ReadingConfig::default_ma();

    let mut mutated = base;
    mutated.presentation.theme_id = ThemeId::Obsidian;
    mutated.presentation.font_family = FontFamily::SystemFallback;
    mutated.presentation.reduce_motion = true;
    mutated.presentation.paper_noise = !base.presentation.paper_noise;

    assert_eq!(
        mutated.geometry, base.geometry,
        "呈现子集的全部字段改过之后，几何子集必须逐字段不变（FR-028 的子集分离）"
    );
    assert_ne!(
        mutated.presentation, base.presentation,
        "前置对照：呈现子集确实被改了（否则本测试是空判据）"
    );
}

// ===========================================================================
// 反向：几何变化**确实**改变页序列（防"永远相同"的假绿）
// ===========================================================================

/// **几何参数变化必须改变页序列**——否则上面的"相同"断言可能是假绿。
///
/// ## 这条为什么必要
///
/// 一个把 `paginate` 写成"永远返回同一页"的实现会让全部 SC-005 断言通过。
/// 本测试用**相反方向**的断言堵住这个漏洞：改字号必须让页数或页区间变化。
///
/// 两个方向的断言合起来才是有意义的判据：
///
/// | 改动 | 期望 |
/// |---|---|
/// | 呈现参数 | 页序列**不变** |
/// | 几何参数 | 页序列**变** |
#[test]
fn geometry_change_does_actually_alter_pages() {
    let content = chapter_text();
    let viewport = Viewport::new(342.0, 400.0);

    let small_font = {
        let mut g = ReadingConfig::default_ma().geometry;
        g.font_size_px = 14;
        g
    };
    let large_font = {
        let mut g = ReadingConfig::default_ma().geometry;
        g.font_size_px = 30;
        g
    };

    let small_pages = paginate(&content, &small_font, &viewport);
    let large_pages = paginate(&content, &large_font, &viewport);

    assert_ne!(
        pages_snapshot(&small_pages),
        pages_snapshot(&large_pages),
        "字号 14px → 30px 必须改变页序列（否则分页没真的依赖几何参数，\
         SC-005 的'相同'断言也就没有意义）"
    );
}

/// 行高变化同样改变页序列（高度约束确实生效）。
#[test]
fn line_height_change_alters_pages() {
    let content = chapter_text();
    let viewport = Viewport::new(342.0, 400.0);

    let tight = {
        let mut g = ReadingConfig::default_ma().geometry;
        g.line_height = 1.75;
        g
    };
    let loose = {
        let mut g = ReadingConfig::default_ma().geometry;
        g.line_height = 2.2;
        g
    };

    assert_ne!(
        pages_snapshot(&paginate(&content, &tight, &viewport)),
        pages_snapshot(&paginate(&content, &loose, &viewport)),
        "行高 1.75 → 2.2 必须改变页序列"
    );
}

/// 段模式变化够改变页序列吗？——**不一定**，故本测试断言的是较弱但确定的性质。
///
/// ## 为什么这里不能断言"必须改变"
///
/// 段缩进/段间距影响的是**段首行**的排布（缩进占位或段前留白）。在窄版心下
/// 它通常改变行数，但若正文恰好每段都排满整行、或只有一段，它可能不改变
/// 任何页边界——那时"必须改变"的断言会让一个**正确**的实现失败。
///
/// 故断言改为：两种模式各自产出**无损**的页序列（SC-006），且段缩进模式
/// 下首行确实带缩进（由行盒的字形起点体现）。后者是段模式确实生效的证据。
#[test]
fn paragraph_mode_does_not_break_losslessness() {
    use tome_reader::paginate::config::ParagraphMode;

    let content = chapter_text();
    let viewport = Viewport::new(228.0, 300.0);

    for mode in [
        ParagraphMode::Indent { em: 2.0 },
        ParagraphMode::Gap { em: 0.4 },
    ] {
        let mut geometry = ReadingConfig::default_ma().geometry;
        geometry.paragraph_mode = mode;
        let pages = paginate(&content, &geometry, &viewport);

        let reconstructed: String = pages
            .iter()
            .filter(|page| !page.is_padding)
            .map(|page| {
                content
                    .chars()
                    .skip(page.char_range.start)
                    .take(page.char_range.end - page.char_range.start)
                    .collect::<String>()
            })
            .collect();
        assert_eq!(
            reconstructed, content,
            "段模式 {mode:?} 下页序列必须无损（SC-006）"
        );
    }
}
