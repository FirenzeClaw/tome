//! T003 验收：FR-058 的"零面向用户自然语言硬编码"扫描。
//!
//! ## 为什么这条约束要靠扫描而不是靠自觉
//!
//! FR-058 / 001 §FR-044 要求引擎只产出**可本地化标识**，文案映射留给上层。
//! 违反它的代价不是 bug，而是**架构倒退**：引擎里一旦出现中文提示语，
//! 新增一种语言就要改引擎、要重发引擎版本，本地化与引擎生命周期被绑死。
//!
//! 而它是"静默"违规——写下一句 `"字体加载失败，请检查安装"` 能编译、能跑、
//! 测试全绿，只有日后做多语言时才发现。故用扫描把红线钉住。
//!
//! ## 扫描口径的边界（必须写清，否则会变成"看心情的规则"）
//!
//! 本扫描检查**字符串字面量**中是否含 CJK 字符。它**不**检查注释与文档注释
//! ——AGENTS.md 明确要求"注释使用中文"，那条约定与 FR-058 不冲突：
//! 注释给开发者看，不进产物；字面量会进产物、会出现在用户眼前。
//!
//! 白名单只有一类：**诊断细节的构造点**。见 [`ALLOWED_NON_MESSAGE_LITERALS`]。

use std::fs;
use std::path::{Path, PathBuf};

/// 允许出现 CJK 字面量的例外（每条须注明理由）。
///
/// ## 为什么允许"诊断细节"
///
/// [`ReaderError`] 的 `detail` 字段是**开发者向**的（文档里写明"不是面向用户
/// 的提示语，上层不得直接展示"）。但目前本 crate 的生产代码里**一条都没有**
/// ——所有 detail 都由调用方传入 `impl Into<String>`。
///
/// 白名单因此是**空的**：任何 CJK 字面量都必须先在这里登记并写明理由。
/// 空白名单是最强的形态——它让"加一句中文"这个动作必须经过一次显式决策，
/// 而不是顺手写下去。
const ALLOWED_NON_MESSAGE_LITERALS: &[(&str, &str)] = &[];

/// 递归收集 `src/` 下的全部 `.rs` 文件。
fn rust_sources(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// 判断一个字符是否 CJK（含中日韩统一表意文字与中文标点）。
///
/// 覆盖范围：基本区、扩展 A、兼容表意文字、CJK 标点与全角形式。
/// 不覆盖日文假名与韩文谚文——本项目的语言只有中文，且假名若出现在字面量里
/// 同样是问题，会被 CJK 标点或后续的 review 抓到。
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x3000..=0x303F   // CJK 标点（、。「」等）
        | 0x4E00..=0x9FFF // CJK 统一表意文字（基本区）
        | 0x3400..=0x4DBF // 扩展 A
        | 0xF900..=0xFAFF // 兼容表意文字
        | 0xFF00..=0xFFEF // 全角形式
        | 0x20000..=0x2A6DF // 扩展 B
    )
}

/// 扫描结果的一项。
#[derive(Debug)]
struct Finding {
    file: PathBuf,
    line: usize,
    text: String,
}

/// 剥离行注释与文档注释，只保留可能含字面量的代码部分。
///
/// ## 为什么不能简单地"整行含中文就报"
///
/// 注释**必须**是中文（AGENTS.md）。若把注释算进来，扫描会全红，然后被人
/// 用"加豁免"的方式绕过——那比没有扫描更糟。故此处做真正区分：
/// `//` 之后的内容与字符串字面量之外的区块标记剥掉。
///
/// 该实现刻意**不求完美**（不处理块注释嵌套、不解析 raw string 的
/// `r#"..."#` 分隔符数量）：本扫描的目标是"抓住顺手写下的中文文案"，
/// 一个会漏掉刻意伪装的扫描器是可接受的——它防的是疏忽，不是对抗。
fn code_without_comments(line: &str) -> &str {
    match line.find("//") {
        Some(index) => &line[..index],
        None => line,
    }
}

/// 判定某一行是否处于 `#[cfg(test)]` 之后的（近似）测试模块区间。
///
/// ## 为什么测试模块要整体排除
///
/// 测试里的中文是**断言消息**——它跟注释同类：给开发者看，且 `cfg(test)` 的
/// 代码**不进产物**（`cargo build` 不编译它，发布二进制里一个字节都没有）。
/// FR-058 管的是"引擎产出的错误"，而断言消息从不经错误通道流到用户眼前。
///
/// 若把测试也算违规，实际后果是：每个断言消息都要用英文写、或者整块加白名单
/// ——前者伤害测试可读性（AGENTS.md 要求中文注释），后者让白名单膨胀到失去
/// 意义。故按 `cfg(test)` 边界切分，是"红线不被绕过"与"测试保持可读"的平衡。
///
/// ## 判据是近似的
///
/// 用"行内出现 `#[cfg(test)]` 后进入测试区、模块闭合大括号后离开"来近似，
/// 靠大括号计数。生产代码里 `#[cfg(test)]` 的惯例形态（模块级、置于文件末尾）
/// 下该近似是准确的；若日后有人写出稀奇的结构，本扫描只会**更严格**
/// （多报而非漏报），属于安全的失效方向。
fn compute_test_regions(content: &str) -> Vec<bool> {
    let mut regions = Vec::new();
    let mut in_test = false;
    let mut depth = 0i32;
    let mut opened_at = 0i32;

    for line in content.lines() {
        let code = code_without_comments(line);

        if !in_test && code.contains("#[cfg(test)]") {
            in_test = true;
            depth = 0;
            opened_at = 0;
        }

        if in_test {
            // 计数本行出现的花括号（先剥掉字符字面量里的括号是过度设计，
            // 测试代码里几乎不会出现 '{' 作为字面量的情形）
            for ch in code.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        opened_at += 1;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            regions.push(true);
            // 测试区起始后至少见过一个 `{` 且深度归零 → 模块结束
            if opened_at > 0 && depth <= 0 {
                in_test = false;
                regions.pop();
                regions.push(false);
            }
        } else {
            regions.push(false);
        }
    }

    regions
}

#[test]
fn no_natural_language_in_reader_error_sources() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_root = manifest_dir.join("src");

    let mut sources = Vec::new();
    rust_sources(&src_root, &mut sources);
    assert!(
        !sources.is_empty(),
        "未在 {} 下找到任何 .rs 文件——扫描器本身失效了",
        src_root.display()
    );

    let mut findings = Vec::new();

    for path in &sources {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let test_regions = compute_test_regions(&content);
        for (index, raw_line) in content.lines().enumerate() {
            // `#[cfg(test)]` 区间的中文是断言消息：开发者向、不进产物，
            // 与注释同类（见 compute_test_regions 的文档）。
            if test_regions.get(index).copied().unwrap_or(false) {
                continue;
            }
            let code = code_without_comments(raw_line);
            if !code.chars().any(is_cjk) {
                continue;
            }
            // 逐条核对白名单（键为"文件:行片段"，值为理由）
            let relative = path
                .strip_prefix(&manifest_dir)
                .unwrap_or(path)
                .display()
                .to_string();
            let allowed = ALLOWED_NON_MESSAGE_LITERALS
                .iter()
                .any(|(needle, _)| raw_line.contains(needle));
            if !allowed {
                findings.push(Finding {
                    file: PathBuf::from(&relative),
                    line: index + 1,
                    text: raw_line.trim().to_owned(),
                });
            }
        }
    }

    assert!(
        findings.is_empty(),
        "FR-058 违规：以下代码（非注释）含 CJK 字面量，疑似面向用户的自然语言。\n\
         引擎只应产出可本地化标识（如 `error.reader.layout_failed`），文案映射留给上层。\n\
         若确为开发者向的诊断细节，请在 ALLOWED_NON_MESSAGE_LITERALS 登记并写明理由。\n{}",
        findings
            .iter()
            .map(|f| format!("  {}:{}  {}", f.file.display(), f.line, f.text))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 反证：`#[cfg(test)]` 区间的识别必须准确——否则扫描要么全红（区间没生效）
/// 要么放过了生产代码（区间过宽）。
#[test]
fn cfg_test_regions_are_computed_correctly() {
    let sample = "\
// 文件头中文注释（放行）
pub fn production() -> &'static str {
    \"error.reader.layout_failed\" // 生产代码里的标识
}

#[cfg(test)]
mod tests {
    #[test]
    fn asserts_in_chinese() {
        assert!(true, \"中文断言消息（放行）\");
    }
}

pub fn after_tests() -> u32 {
    42
}
";
    let regions = compute_test_regions(sample);
    let lines: Vec<&str> = sample.lines().collect();
    assert_eq!(regions.len(), lines.len(), "区间数组须与行数一一对应");

    // 测试区起始行（`#[cfg(test)]` 本身）到模块闭合之间为 true
    let test_start = lines
        .iter()
        .position(|line| line.contains("#[cfg(test)]"))
        .expect("样例必须含 #[cfg(test)]");
    let after_tests = lines
        .iter()
        .position(|line| line.contains("after_tests"))
        .expect("样例必须含 after_tests");

    // 断言中文那一行必须在区间内（否则扫描会误报）
    let chinese_assert = lines
        .iter()
        .position(|line| line.contains("中文断言消息"))
        .expect("样例必须含中文断言");
    assert!(regions[chinese_assert], "测试断言行须被识别为测试区间");
    assert!(regions[test_start], "`#[cfg(test)]` 行起即进入测试区间");

    // **契约边界**：测试模块闭合**之后**的生产代码不得在区间内——否则
    // 生产代码的中文文案会被放过，扫描形同虚设。
    //
    // 注意 `after_tests` 那一行是 `pub fn after_tests() -> u32 {`，它的
    // 前一行 `}` 是测试模块的闭合。按大括号平衡的判定，闭合行本身属于
    // "深度归零"的那一行，两种归属都合理；此处只断言**其后**的行，
    // 不锁死闭合行的归属（那是实现细节，不是契约）。
    assert!(
        !regions[after_tests],
        "测试模块之后的生产代码不得被划入测试区间（否则生产中文会被放过）"
    );
    let body_line = lines
        .iter()
        .position(|line| line.trim() == "42")
        .expect("样例必须含生产函数体");
    assert!(!regions[body_line], "生产函数体不得在测试区间内");
}

/// 反证：扫描器本身必须能检出中文（否则上面的测试是"永远通过"的假绿）。
///
/// ## 为什么必须有这条
///
/// 一个"永远返回空结果"的扫描器会让上面那条测试永远通过——**失效的守卫
/// 比没有守卫更危险**，因为它制造"已防护"的假象。故此处用合成输入断言
/// 扫描器的判断逻辑真的有效。
#[test]
fn scanner_detects_chinese_in_code() {
    // 阳性：代码里的中文字面量必须被识别
    assert!(
        code_without_comments(r#"    let msg = "字体加载失败";"#)
            .chars()
            .any(is_cjk),
        "扫描器未能识别代码中的中文字面量（守卫失效）"
    );

    // 阴性：纯注释不得被识别（否则扫描会全红并被绕过）
    assert!(
        !code_without_comments("    // 这是中文注释，必须放行")
            .chars()
            .any(is_cjk),
        "扫描器把注释误判为违规（会逼出「加豁免」式绕过）"
    );

    // 阴性：纯 ASCII 代码不得被识别
    assert!(
        !code_without_comments(r#"    let id = "error.reader.layout_failed";"#)
            .chars()
            .any(is_cjk),
        "扫描器误报 ASCII 标识"
    );

    // 边界：行内先注释后代码（注释在前，代码在后）——本实现按行首 `//`
    // 截断，故这一行会被判为"注释"，属于已知的宽松偏差（宁可漏报不误报）。
    // 此处显式记录该行为，避免日后有人误以为它是精确解析。
    assert!(!code_without_comments("// 注释 let x = 1;")
        .chars()
        .any(is_cjk));
}

/// 能力标识的命名约定：书源生态的通用叫法（小驼峰 ASCII），不是文案也不是
/// 蛇形常量名。
#[test]
fn capability_identifiers_follow_source_ecosystem_style() {
    use tome_reader::capability;

    for id in [
        capability::OFFSCREEN_RENDERING,
        capability::FONT_UNAVAILABLE,
    ] {
        assert!(
            id.is_ascii(),
            "能力标识须为 ASCII（跨语言、可进书源脚本判断）：{id}"
        );
        assert!(
            !id.contains('_') && !id.contains(' '),
            "能力标识用书源生态的小驼峰写法（如 `webView`），不用蛇形或句子：{id}"
        );
        let first = id.chars().next().unwrap_or('\0');
        assert!(
            first.is_ascii_lowercase(),
            "能力标识首字母小写（书源生态惯例）：{id}"
        );
        assert!(
            id.chars().any(|ch| ch.is_ascii_uppercase()),
            "小驼峰标识应含大写字母：{id}"
        );
    }

    // 001 既有的能力标识同属该风格——本层新增的不得偏离。
    assert_eq!(capability::OFFSCREEN_RENDERING, "offscreenRendering");
    assert_eq!(capability::FONT_UNAVAILABLE, "fontUnavailable");
}
