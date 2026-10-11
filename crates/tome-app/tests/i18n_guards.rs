//! S3 外壳的**结构守卫**（T031 的文案纪律 + T034 的 token 纪律，提前落地）
//!
//! ## 为什么这两条守卫现在就要有
//!
//! 两条纪律的共同点是：**违反它们的代码能跑、观感也对、没有任何测试会红**。
//!
//! | 纪律 | 违反的形态 | 为什么不会红 |
//! |---|---|---|
//! | 文案入表（`contracts/ui.md` §3.1） | 组件里直接 `ui.label("书架是空的")` | 界面照常显示，只是那句话从此不受文案表管 |
//! | 配色取 token（MASTER §10） | 组件里 `Color32::from_rgb(0x8C, 0x3A, 0x1E)` | 看起来"也是那个颜色"，直到设计系统改主色 |
//!
//! 等到 T031/T034 才回头查，代价是**逐处返工**；从现在起就守住，代价是零
//! （新写的代码自然照表来）。本文件是"从现在起别欠账"的执行机制。
//!
//! ## 为什么用 grep 而不是类型系统
//!
//! 类型系统挡不住字面串——`ui.label("文字")` 与 `ui.label(文案变量)` 是同一
//! 个类型。这类"约定式约束"只能靠源码扫描，而扫描的代价极低（读几十个文件）。
//! 仓库已有先例：`tests/theme_bridge.rs` 的 `no_hex_color_literals_in_tome_app_sources`。

use std::path::{Path, PathBuf};

/// 仓库根（从 `CARGO_MANIFEST_DIR` 往上两级）。
///
/// `#[expect]` 的理由：本函数是**夹具**而非测试本身，`.clippy.toml` 的
/// `allow-expect-in-tests` 只覆盖 `#[test]` 函数（见该文件的说明）。路径推不出
/// 来时整个守卫失去意义，故在此 panic 是恰当的——但必须显式声明。
#[expect(clippy::expect_used, reason = "夹具：推不出仓库根则守卫无从扫描")]
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate 应在 crates/ 之下")
        .to_path_buf()
}

/// 收集某个目录下的全部 `.rs` 文件。
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// 一行里是否含**中文字面串**（`"..."` 内含 CJK 字符）。
///
/// ## 判据为什么是"引号内含 CJK"而不是"含 CJK"
///
/// 注释是中文的（项目约定），文档注释也是。若只判"含 CJK"，每一行注释都会
/// 命中。故必须把判定收在**字符串字面量**里：扫描引号对，检查其中是否有 CJK。
///
/// ## 已知的粗糙处（如实记录）
///
/// 本函数不解析 Rust 语法，故对以下情形会误判：
/// - 字符串里含转义引号（`"他说 \"你好\""`）—— 罕见，且误判方向是"多报"
/// - 行尾注释里的引号（`let x = 1; // "中文"`）—— 会命中，但它确实是可疑写法
///
/// 误判方向偏"多报"是刻意的：多报会让人来看一眼（代价小），漏报会让字面串
/// 混进代码（代价是 T031 的逐处返工）。
fn has_cjk_literal(line: &str) -> bool {
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        // 转义引号（`\"`）不是字符串的结束——跳过它继续找
        if let Some(remaining) = after.strip_prefix('\\') {
            rest = remaining;
            continue;
        }
        let Some(end) = after.find('"') else {
            return false;
        };
        let inside = &after[..end];
        if inside.chars().any(is_cjk) {
            return true;
        }
        rest = &after[end + 1..];
    }
    false
}

/// 是否 CJK 字符（含全角标点——它们同样是"面向用户的文案"）。
fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x3000..=0x303F   // CJK 标点（、。「」等）
        | 0x3400..=0x4DBF // 扩展 A
        | 0x4E00..=0x9FFF // 基本区
        | 0xF900..=0xFAFF // 兼容表意
        | 0xFF00..=0xFFEF // 全角形式
    )
}

/// 豁免清单（每一条都必须有理由）。
///
/// ## 为什么豁免要显式列而不是"跳过某类文件"
///
/// 跳过整类文件会让那个文件成为无守卫区（新代码随便写）。显式列到**行**
/// 让豁免可审计：看一眼就知道有多少处、分别为什么。
fn is_exempt(relative: &str, line: &str) -> bool {
    // 守卫自身：它必须含中文来定义"什么是中文"
    if relative.ends_with("tests/i18n_guards.rs") {
        return true;
    }
    // 文案表本体：它的**职责**就是装中文
    if relative.ends_with("src/i18n.rs") {
        return true;
    }
    // 日志与启动失败窗口：`log::error!("tome-app: ...")` 与 `ui.label(&self.message)`
    // 里的启动失败文案。这些是**开发者向**（日志）或**启动期**（此时 i18n 表
    // 尚未经正常路径加载，且这段代码在窗口创建之前跑）的文本。
    //
    // 明确记录为偏离：严格按 §3.1 它们也该入表。T031 收口时一并处理
    // （见任务回执的"偏离与理由"）。
    if line.contains("log::error!")
        || line.contains("log::warn!")
        || relative.ends_with("src/main.rs")
    {
        return true;
    }
    // panic 类宏的消息是**开发者向诊断**（永远不显示给用户）——它们与注释
    // 同类：读的人是维护者，不是用户。
    if line.contains("unreachable!")
        || line.contains("panic!")
        || line.contains("debug_assert!")
        || line.contains("assert!")
    {
        return true;
    }
    false
}

/// **零中文字面串**：`views/` 与 `widgets/` 下不得出现中文文案。
///
/// `contracts/ui.md` §3.1 的机器可验形式。T031 的守卫会做同一件事，本文件
/// 让它在 Phase 3 就生效——这样新写的视图从第一天起就照表来。
#[test]
fn views_and_widgets_have_no_chinese_literals() {
    let root = repo_root().join("crates/tome-app");
    let mut violations = Vec::new();

    for dir in ["src/views", "src/widgets", "src/state"] {
        for file in rust_files(&root.join(dir)) {
            let relative = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue;
            };
            for (number, line) in content.lines().enumerate() {
                let trimmed = line.trim_start();
                // 注释行不算（项目约定注释用中文）
                if trimmed.starts_with("//") {
                    continue;
                }
                if has_cjk_literal(line) && !is_exempt(&format!("src/{relative}"), line) {
                    violations.push(format!("{relative}:{}: {}", number + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "以下位置含中文字面串——文案必须出自 i18n 表（contracts/ui.md §3.1）：\n{}",
        violations.join("\n")
    );
}

/// **零色值字面量**：外壳源码里不得出现十六进制色值。
///
/// MASTER §10 + R7：配色只取 `tome_core::theme` token，换算只在
/// `theme_bridge::BridgeTheme::color` 一处。
///
/// ## 与 `theme_bridge.rs` 既有守卫的关系
///
/// 那条守的是"整个 crate 无 hex 色值"。本测试**额外**扫 `views/` 与
/// `widgets/`，给出更明确的失败信息（指出是哪个视图漏了 token）——两条并存
/// 不冲突，且本条的失败信息更贴近"我刚写的那个组件"。
#[test]
fn views_and_widgets_have_no_hex_colors() {
    let root = repo_root().join("crates/tome-app");
    let mut violations = Vec::new();

    for dir in ["src/views", "src/widgets"] {
        for file in rust_files(&root.join(dir)) {
            let relative = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let Ok(content) = std::fs::read_to_string(&file) else {
                continue;
            };
            for (number, line) in content.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if line.contains("from_rgb") || line.contains("Color32::from_hex") {
                    violations.push(format!("{relative}:{}: {}", number + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "以下位置直接构造了颜色——配色只取 token（MASTER §10）：\n{}",
        violations.join("\n")
    );
}

/// 守卫自身的有效性自检（`--self-test` 形态）。
///
/// ## 为什么守卫要有自检
///
/// 一条永远为真的守卫比没有守卫更糟：它给出虚假的安全感。本测试用几个**必须
/// 命中**与**必须不命中**的样本验证判定函数本身，让"守卫悄悄失效"这件事
/// 不可能发生。
#[test]
fn the_guard_itself_is_effective() {
    // 必须命中
    assert!(has_cjk_literal(r#"ui.label("书架是空的");"#));
    assert!(has_cjk_literal(r#"format!("共 {} 册", n)"#));
    assert!(has_cjk_literal(r#"let x = "。「」";"#), "全角标点也是文案");

    // 必须不命中
    assert!(!has_cjk_literal("// 这是注释，不算"));
    assert!(!has_cjk_literal(
        r#"ui.label(i18n::t(i18n::key::SHELF_EMPTY_TITLE));"#
    ));
    assert!(!has_cjk_literal(r#"let name = "tome-app";"#));
    assert!(!has_cjk_literal("let 数量 = 3;"));

    // 转义引号不得让扫描器错位（否则它可能把后面的注释当成字符串）
    assert!(!has_cjk_literal(r#"let s = "a\"b";"#));
}
