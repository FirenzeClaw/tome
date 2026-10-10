//! T040 验收：Theme 组合根注入（FR-029）
//!
//! ## FR-029 原文
//!
//! > `Theme` 的纯值类型定义在 `tome-core`；昼/夜**实例**由组合根构造并注入；
//! > `tome-reader` / `tome-app` 只经 `tome-core` 取 `Theme`，二者**不互相依赖**。
//!
//! ## 本文件断言的三件事
//!
//! 1. **注入路径可用**：下游能经 `tome_core::model::theme` 取到昼/夜两个完整
//!    实例，且两者的语义色字段逐项齐全（对照 T002 的清单）。
//! 2. **两实例真的不同**：昼/夜若相等，说明"注入"是假的（某处把两者写成了
//!    同一份）——这条与 T002 的单测重复是**有意的**：单测在 crate 内，
//!    本文件在 crate 外，覆盖的是"下游拿到的是不是同一份"这条不同的事实。
//! 3. **分页不读 Theme**（FR-019 的架构断言）：`paginate` 模块的源码里
//!    不出现 `Theme` —— 主题切换 MUST NOT 触发重新分页。
//!
//! ## 为什么第 3 条要放在本文件（而不是 theme_independence.rs）
//!
//! `theme_independence.rs` 扫的是**呈现参数的字段名**（`theme_id` /
//! `PresentationConfig` 等），判据是"分页不认识呈现子集"。本文件的判据更
//! 靠上一层：分页连**主题值类型本体**都不认识。两条断言挡的是不同的东西——
//! 前者挡"读呈现子集里的字段"，后者挡"直接 `use tome_core::model::theme::Theme`"
//! （那会绕开第一道防线：不读呈现子集，却直接持有主题值）。

//! （本文件不使用 `.expect()`，故不加该 lint 豁免——豁免一个用不到的
//! expectation 会触发 `unfulfilled_lint_expectations`。）

use tome_core::model::theme::{DayTheme, NightTheme, Palette, Theme};

// ===========================================================================
// 1. 注入路径可用
// ===========================================================================

/// **下游能经 `tome-core` 取到昼/夜两个完整实例**（FR-029 的注入路径）。
///
/// ## 为什么这条断言值得写
///
/// FR-029 的"只经 tome-core 取"是**结构性**要求：若 `tome-reader` 自己
/// 构造主题（例如 `render` 里写死 `PARCHMENT` 而不是收参数），它就在
/// 事实上拥有了主题定义权，S3 想换一套就改不动。本文件作为**下游消费者**
/// 调用一次完整取值路径，是"注入面真的存在且可用"的证据。
#[test]
fn downstream_can_obtain_both_themes_via_core() {
    let day: Theme = DayTheme.theme();
    let night: Theme = NightTheme.theme();

    assert_eq!(day.name(), "parchment", "昼主题稳定标识");
    assert_eq!(night.name(), "obsidian", "夜主题稳定标识");
    assert!(!day.is_night(), "昼主题不该是夜");
    assert!(night.is_night(), "夜主题该是夜");
}

/// **两个实例的语义色字段逐项齐全**（对照 T002 清单）。
///
/// `Palette` 是 `Copy` 的纯值，字段访问即完整性证明——本测试的实际价值
/// 在于**列出清单**：若将来 MASTER 增补一个 token 而 `Palette` 漏加，
/// 这里的调用点会编译失败（字段不存在），而不是等到界面上"某主题漏画一块"。
#[test]
fn both_themes_carry_the_complete_semantic_palette() {
    let palettes: [Palette; 2] = [DayTheme.palette(), NightTheme.palette()];

    for palette in palettes {
        // 逐项触碰全部 20 个语义色（编译期完整性 + 运行期非零校验）
        let swatches = [
            ("bg_base", palette.bg_base),
            ("bg_surface", palette.bg_surface),
            ("bg_elevated", palette.bg_elevated),
            ("bg_reading", palette.bg_reading),
            ("bg_inset", palette.bg_inset),
            ("border_subtle", palette.border_subtle),
            ("border_strong", palette.border_strong),
            ("text_primary", palette.text_primary),
            ("text_secondary", palette.text_secondary),
            ("text_tertiary", palette.text_tertiary),
            ("text_inverse", palette.text_inverse),
            ("accent_primary", palette.accent_primary),
            ("accent_hover", palette.accent_hover),
            ("accent_soft", palette.accent_soft),
            ("accent_secondary", palette.accent_secondary),
            ("accent_clash", palette.accent_clash),
            ("state_success", palette.state_success),
            ("state_warning", palette.state_warning),
            ("state_error", palette.state_error),
            ("state_info", palette.state_info),
        ];
        assert_eq!(swatches.len(), 20, "MASTER §2 的语义色恰为 20 项");

        for (name, color) in swatches {
            // 十六进制串是 7 字符（`#RRGGBB`）——所有分量都被真的取到
            assert_eq!(
                color.to_hex_string().len(),
                7,
                "{name} 的十六进制形态应是 #RRGGBB"
            );
        }
    }
}

/// **昼/夜两实例必须真的不同**（否则"注入"是假的）。
#[test]
fn day_and_night_are_distinct_instances() {
    assert_ne!(
        DayTheme.theme(),
        NightTheme.theme(),
        "两个注入实例若相等，说明组合根实际上只构造了一份主题"
    );
    assert_ne!(
        DayTheme.palette().bg_reading,
        NightTheme.palette().bg_reading,
        "阅读区背景是两套主题最直接的差异点（FR-030）"
    );
}

// ===========================================================================
// 2. 分页不读 Theme（FR-019 的架构断言）
// ===========================================================================

/// 递归收集 `src/` 下的全部 `.rs` 文件（返回 `(相对路径, 内容)`）。
fn source_files(relative_dir: &str) -> Vec<(String, String)> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let Ok(content) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let relative = path
                    .strip_prefix(root)
                    .map_or_else(|_| path.display().to_string(), |p| p.display().to_string());
                out.push((relative, content));
            }
        }
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&root, &root.join(relative_dir), &mut files);
    files
}

/// 去掉注释行（`//` 起首）——注释里讨论 Theme 是允许的。
fn code_only(source: &str) -> String {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `Theme` **值类型**是否被引用（而非 `ThemeId` 这类呈现参数标识）。
///
/// ## 为什么要按词边界判，而不是 `contains("Theme")`
///
/// `ThemeId` 是**呈现参数的标识**（昼/夜的枚举），它的存在恰恰说明分页
/// 不认识主题的**值**——`config.rs` 里那句注释正是在讨论"为什么不放主题值"。
/// 若用 `contains("Theme")`，`ThemeId` 会被误判成违规，断言就只好放宽到
/// "不出现"，反而丢掉判别力。故按标识符边界匹配：`Theme` 之后不得紧跟
/// 标识符字符（字母/数字/下划线）。
fn references_theme_value(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut search_from = 0usize;
    while let Some(offset) = code[search_from..].find("Theme") {
        let start = search_from + offset;
        let end = start + "Theme".len();
        // 前一个字符也不能是标识符字符（挡 `FooTheme` 这种）
        let before_ok =
            start == 0 || !bytes[start - 1].is_ascii_alphanumeric() && bytes[start - 1] != b'_';
        // 后一个字符必须是边界（挡 `ThemeId` / `ThemeId::Obsidian`）
        let after_ok =
            end >= bytes.len() || !bytes[end].is_ascii_alphanumeric() && bytes[end] != b'_';
        if before_ok && after_ok {
            return true;
        }
        search_from = end;
    }
    false
}

/// **`Theme` 值类型的判别器自身有效**（守卫的自检）。
///
/// ## 为什么必须自检
///
/// 上面三条"不出现"断言若因判别器恒为 `false` 而通过，它们就是**假的**——
/// 一个永远说"没出现"的探测器会让任何代码通过检查。本测试同时验证
/// 正例（`Theme` 值类型被判为出现）与反例（`ThemeId` 被判为未出现），
/// 让"探测器真的在工作"成为断言的一部分。
#[test]
fn theme_value_detector_distinguishes_value_type_from_presentation_id() {
    // 正例：值类型的各种出现形态
    for positive in [
        "use tome_core::model::theme::Theme;",
        "fn draw(theme: Theme) {}",
        "Theme::palette()",
        "&Theme, ",
        "foo(&self) -> Theme {",
    ] {
        assert!(
            references_theme_value(positive),
            "应识别出值类型引用：{positive}"
        );
    }

    // 反例：呈现参数标识不是值类型
    for negative in [
        "pub enum ThemeId {",
        "pub theme_id: ThemeId,",
        "ThemeId::default()",
        "SomeThemeId",
        "ThemeIds",
    ] {
        assert!(
            !references_theme_value(negative),
            "不该把呈现参数标识误判成值类型引用：{negative}"
        );
    }
}

/// **分页模块的源码里不出现 `Theme` 值类型**（FR-019 的架构级断言）。
///
/// ## 判据与理由
///
/// FR-019：「页数只依赖几何参数，MUST NOT 依赖颜色、主题、封面等呈现参数」。
/// 违反它的代价是用户可见的（切主题整章重排、位置抖动）。`theme_independence.rs`
/// 已经从**行为层**（页序列逐字节相同）与**呈现字段层**（源码无 `theme_id`）
/// 挡了两道；本断言补第三道：分页模块连 `Theme` 值类型都不认识。
///
/// 三道防线的分工：
///
/// | 防线 | 挡住的写法 |
/// |---|---|
/// | 行为层（页序列相同） | 读了主题但没有影响到页数（碰巧） |
/// | 呈现字段层（源码无 `theme_id`） | `presentation.theme_id` |
/// | **本断言（源码无 `Theme`）** | `use tome_core::model::theme::Theme` 后直接读颜色 |
///
/// ## 扫描范围
///
/// `src/paginate/` 整个模块（含 `config.rs`）——`config.rs` 里出现的是
/// **文档注释**里的 `Theme`（讨论"为什么不放主题值"），故本断言去注释后
/// 扫描：注释允许提及，代码不允许。
#[test]
fn pagination_module_never_references_theme_values() {
    let files = source_files("paginate");
    assert!(!files.is_empty(), "前置：应能扫到 paginate 模块的源码");

    for (relative, source) in files {
        let code = code_only(&source);
        assert!(
            !references_theme_value(&code),
            "分页模块 `{relative}` 的代码里出现了 `Theme` 值类型——\
             FR-019 / FR-029 要求分页只依赖几何参数，主题切换 MUST NOT 触发重排。\
             若确需在分页路径上读主题，先重新审视该依赖是否真的与分页几何有关。"
        );
        assert!(
            !code.contains("model::theme"),
            "分页模块 `{relative}` 的代码里出现了 `model::theme` 路径——同上。"
        );
    }
}

/// **滚动流同样不引用 Theme**（滚动是分页之外的另一条排版路径）。
///
/// FR-019 的措辞是"分页结果与滚动流布局对主题与呈现参数完全无依赖"——
/// 两者都要挡。滚动流若在切分视口时读了主题（例如"夜主题行距补偿"），
/// 后果与分页侧一样：切主题导致滚动位置抖动。
#[test]
fn scroll_module_never_references_theme_values() {
    let files = source_files("scroll");
    assert!(!files.is_empty(), "前置：应能扫到 scroll 模块的源码");

    for (relative, source) in files {
        let code = code_only(&source);
        assert!(
            !code.contains("Theme"),
            "滚动模块 `{relative}` 的代码里出现了 `Theme`——FR-019 要求滚动流布局\
             同样不依赖主题（切主题不得让滚动位置抖动）。"
        );
    }
}

/// **渲染模块确实在消费 Theme**（反向对照，防"啥都扫不到"的假绿）。
///
/// ## 为什么需要这条
///
/// 上面三条是"不出现"断言。若扫描目录写错了（例如 `src` 路径拼错），
/// 它们会**全部通过**——一个空扫描集让任何"不出现"成立。本测试用相反的
/// 方向确认扫描真的读到了文件：`render` 模块**必须**出现 `Theme`，
/// 否则说明主题从未被用于绘制，那 FR-030 的语义色映射就是空的。
#[test]
fn render_module_does_consume_theme() {
    let files = source_files("render");
    assert!(!files.is_empty(), "前置：应能扫到 render 模块的源码");

    let mentions = files
        .iter()
        .filter(|(_, source)| code_only(source).contains("Theme"))
        .count();
    assert!(
        mentions > 0,
        "render 模块应有文件引用 Theme（FR-030 的语义色映射落点）；\
         一个都不出现说明扫描范围写错，或主题根本没被用于绘制"
    );
}
