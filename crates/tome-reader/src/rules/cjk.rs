//! `[DONE]` 中文禁则字符清单（T012 / FR-005、FR-007）
//!
//! ## 依据（干净室纪律：依据必须是公开规范，不得照抄任何实现）
//!
//! | 依据来源 | 用于 |
//! |---|---|
//! | **UAX#14**（Unicode Line Breaking Algorithm）的 `LineBreak.txt` 属性 | CL / OP / EX / NS 类字符的归类 |
//! | **CLreq**（W3C《中文排版需求》）§3.1.1「行首行尾禁则」 | 中文惯例的规则表述与例外 |
//! | **GB/T 15834**《标点符号用法》 | 中文标点符号的完整清单 |
//!
//! ## 与 UAX#14 的关系：**补丁式**，不是重写
//!
//! T005 的 spike 实测证实：cosmic-text 依赖的 `unicode-linebreak` 已经把
//! UAX#14 的基础避头尾做进了 **word 切分**——中文标点与其前一个字绑成
//! 多字 word（实测「点，」= 38px = 2 字身），故断点天然不会落在
//! 「，」与「点」之间。
//!
//! 那么本模块存在的理由是什么？——**UAX#14 是通用规范，不覆盖中文惯例的
//! 全部条目**。两者冲突处，中文排版惯例优先（FR-005 明列"破折号对、省略号"
//! 为需补丁的条目）。故本清单分两类：
//!
//! - [`RuleKind::LineStartForbidden`] / [`RuleKind::LineEndForbidden`]：
//!   UAX#14 已覆盖的基础条目，本层**显式列出**以便逐项测试（FR-007 要求
//!   "显式、可枚举、可逐项测试"，不能靠"依赖库应该做了"）
//! - [`RuleKind::Patch`]：**本层相对 UAX#14 的增补**，每条在
//!   [`KINSOKU_RULES`] 的 `basis` 里注明它偏离了什么、依据是哪个公开条款
//!
//! ## SC-001 的落点
//!
//! 每条规则带**判定例**（该规则生效的例子）与**反例**（不该生效的例子）。
//! 反例的存在是本清单最有价值的部分：没有反例的规则无法区分"规则生效"
//! 与"规则永远返回真"。

/// 规则类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    /// 行首禁则：该字符不得出现在行首
    LineStartForbidden,
    /// 行尾禁则：该字符不得出现在行尾
    LineEndForbidden,
    /// 相对 UAX#14 的增补条目
    Patch,
}

/// 一条禁则规则的**可测试形态**。
///
/// ## 为什么规则要带判据与依据，而不是只带一个字符集
///
/// FR-007 要求"清单 MUST 附每项的判定例与反例"。把判据与字符集放在同一个
/// 结构里，意味着**新增字符必须同时给出判据**——否则编译期就报缺字段。
/// 若把判据写在测试文件里，二者会漂移（加字符忘了加测试）。
#[derive(Debug, Clone, Copy)]
pub struct KinsokuRule {
    /// 规则标识（稳定、可 grep——用于 `RuleExceptionRecord::rule_id`）
    pub id: &'static str,
    /// 规则类别
    pub kind: RuleKind,
    /// 受本规则约束的字符集
    pub chars: &'static str,
    /// **依据**（公开规范的具体条款；补丁条目须注明偏离方向）
    pub basis: &'static str,
    /// **判定例**：本规则应当生效的例子（含该字符在禁则位置的情形）
    pub positive_case: &'static str,
    /// **反例**：本规则**不应**生效的例子（该字符在允许位置）
    pub negative_case: &'static str,
}

/// 中文禁则规则清单（FR-007 的"显式、可枚举、可逐项测试"）。
///
/// ## 清单规模的记账
///
/// 当前 **12 条**禁则规则 + **3 项**非禁则覆盖记账（见 [`NON_KINSOKU_COVERAGE`]）：
///
/// | 类别 | 条数 |
/// |---|---|
/// | 行首禁则（CL/EX/PO 类） | 7 |
/// | 行尾禁则（OP 类） | 2 |
/// | 补丁（相对 UAX#14 的增补） | 3 |
/// | **禁则小计** | **12** |
/// | 非禁则（挤压/间隙/空白折叠） | 3（记账，落点在 T011/T014） |
///
/// 条数写在这里是**刻意的**：它让"清单被意外缩减"变成可见的变化
/// （`rule_count_is_documented` 断言逐类条数）。
pub const KINSOKU_RULES: &[KinsokuRule] = &[
    // ------------------------------------------------------------------
    // 行首禁则（闭标点 / 句末标点 / 后引号 / 后括号）——UAX#14 CL 类
    // ------------------------------------------------------------------
    KinsokuRule {
        id: "kinsoku.line_start.closing_bracket_cjk",
        kind: RuleKind::LineStartForbidden,
        chars: "）］｝〉》」』】〗〙〛",
        basis: "UAX#14 LineBreak.txt：Pe/Ps 配对的闭括号属 CL（Close Punctuation）类，\
                不得置于行首；CLreq §3.1.1 同口径",
        positive_case: "行，「甲」）不得以「）」起行",
        negative_case: "「（甲）」——开括号可在行首，本规则不管开括号",
    },
    KinsokuRule {
        id: "kinsoku.line_start.quotation_close",
        kind: RuleKind::LineStartForbidden,
        chars: "”’",
        basis: "UAX#14：Pi/Pf 类中的后引号属 CL 类，不得置于行首；\
                CLreq §3.1.1「后引号不入行首」",
        positive_case: "他说「好」。”不得以「”」起行",
        negative_case: "“好”——前引号可在行首，本规则不管前引号",
    },
    KinsokuRule {
        id: "kinsoku.line_start.comma_full_width",
        kind: RuleKind::LineStartForbidden,
        chars: "，、",
        basis: "UAX#14：全角逗号、顿号属 CL 类（Po），不得置于行首；\
                CLreq §3.1.1「逗号顿号不入行首」",
        positive_case: "甲乙，不得以「，」起行",
        negative_case: "，甲乙——逗号在行首位置时属于本规则要修的情形",
    },
    KinsokuRule {
        id: "kinsoku.line_start.period_full_width",
        kind: RuleKind::LineStartForbidden,
        chars: "。．",
        basis: "UAX#14：句号属 CL 类（Po），不得置于行首；CLreq §3.1.1",
        positive_case: "结束。不得以「。」起行",
        negative_case: "。结束——句号在行首属违规情形",
    },
    KinsokuRule {
        id: "kinsoku.line_start.sentence_end_marks",
        kind: RuleKind::LineStartForbidden,
        chars: "？！",
        basis: "UAX#14：问号、叹号属 EX（Exclamation/Question）类，与 CL 同样不得\
                置于行首；CLreq §3.1.1",
        positive_case: "真的吗？不得以「？」起行",
        negative_case: "？真的——问号在行首属违规情形",
    },
    KinsokuRule {
        id: "kinsoku.line_start.semicolon_colon",
        kind: RuleKind::LineStartForbidden,
        chars: "；：",
        basis: "UAX#14：分号、冒号属 CL/IS 类，中文惯例上与逗号同等处理（不得置于行首）；\
                CLreq §3.1.1",
        positive_case: "如下；不得以「；」起行",
        negative_case: "；如下——分号在行首属违规情形",
    },
    KinsokuRule {
        id: "kinsoku.line_start.percent_and_unit",
        kind: RuleKind::LineStartForbidden,
        chars: "％‰℃°",
        basis: "FR-005 明文列「百分号与单位」为行首禁则字符；\
                UAX#14 中 ％ 属 PO（Postfix Numeric）类，与 CL 同样不得置于行首",
        positive_case: "增长了 30％不得以「％」起行",
        negative_case: "％30——百分号在前属违规情形",
    },
    // 注：省略号（U+2026）与破折号（U+2014）**不在此表**。
    //
    // 它们作为配对起点时起行是**合法**的（中文排版里 `……` 常在行首），
    // 故只由下方的 Patch 条目约束其**配对内部**不可断。
    // 初版把 `…` 放进本表，导致「久……然后」的省略号被判为不可断——
    // 那会让省略号永远无法起行（由 `patch_pair_boundary_precedence` 抓出）。
    // ------------------------------------------------------------------
    // 行尾禁则（开标点 / 前引号 / 前括号）——UAX#14 OP 类
    // ------------------------------------------------------------------
    KinsokuRule {
        id: "kinsoku.line_end.opening_bracket_cjk",
        kind: RuleKind::LineEndForbidden,
        chars: "（［｛〈《「『【〖〘〚",
        basis: "UAX#14：Ps（Open Punctuation）类不得置于行末；CLreq §3.1.1\
                「前括号不入行尾」",
        positive_case: "他（不得以「（」结行",
        negative_case: "甲）——闭括号可在行尾，本规则不管闭括号",
    },
    KinsokuRule {
        id: "kinsoku.line_end.quotation_open",
        kind: RuleKind::LineEndForbidden,
        chars: "“‘",
        basis: "UAX#14：前引号属 Pi（Initial Punctuation）类，与 OP 同样不得置于行末；\
                CLreq §3.1.1「前引号不入行尾」",
        positive_case: "他说“不得以「“」结行",
        negative_case: "好”——后引号可在行尾，本规则不管后引号",
    },
    // ------------------------------------------------------------------
    // 补丁条目：UAX#14 与中文惯例冲突处，**中文惯例优先**
    // ------------------------------------------------------------------
    KinsokuRule {
        id: "kinsoku.patch.em_dash_pair_not_splittable",
        kind: RuleKind::Patch,
        chars: "—",
        basis: "【相对 UAX#14 的增补，非规范原文】UAX#14 把 U+2014 EM DASH 归 B2 类，\
                该类前后**都可**断行——与中文惯例（破折号不断开）不符。CLreq §3.1.2\
                「破折号占两个字身、不断开」是中文惯例依据。故本层显式成对处理：\
                连续两个 U+2014 视为一个不可断整体",
        positive_case: "他说——那是很久以前的事——人这一生不得在破折号内部断开",
        negative_case: "他说——那是很久以前的事——破折号**之前**可断（那是它的起点）",
    },
    KinsokuRule {
        id: "kinsoku.patch.ellipsis_pair_not_splittable",
        kind: RuleKind::Patch,
        chars: "…",
        basis: "【相对 UAX#14 的增补，非规范原文】UAX#14 把 U+2026 归 IN 类，\
                IN 类可与前字绑定但自身可被断开。中文惯例上省略号占两个字身、\
                不断开（CLreq §3.1.2）。故连续两个 U+2026 视为不可断整体",
        positive_case: "他想了很久……然后说不得在省略号内部断开",
        negative_case: "很久……然后——省略号之前可断",
    },
    KinsokuRule {
        id: "kinsoku.patch.digit_group_not_splittable",
        kind: RuleKind::Patch,
        chars: "0123456789０１２３４５６７８９",
        basis: "【相对 UAX#14 的增补，非规范原文】数位串（如「2024」）不应被断成两行；\
                UAX#14 的 NU（Numeric）类已把数位与**后随的** CL 绑定，但对\
                「全角数位混排」（如「２０２４」）与中文数位的组合无保证。\
                CLreq §3.1.3「西文与数字不拆行」是中文惯例依据",
        positive_case: "2024年不得在数位内部断开",
        negative_case: "2024 年——数位与汉字之间可断",
    },
];

/// FR-007 要求覆盖、但**不属于禁则表**的语法点（供清单完整性记账）。
///
/// ## 为什么单列而不塞进 [`KINSOKU_RULES`]
///
/// FR-007 的"至少覆盖"里有七类语法点，但它们的**性质不同**：
///
/// | 语法点 | 性质 | 落点 |
/// |---|---|---|
/// | 开标点行尾禁则 | 断点约束 | `KINSOKU_RULES` |
/// | 闭标点行首禁则 | 断点约束 | `KINSOKU_RULES` |
/// | 行末标点悬挂或压缩的选择 | 断点约束+几何 | `KINSOKU_RULES` + T015 硬序 |
/// | 超长不可断行的强制断点策略 | 断点约束 | T014（混排退化） |
/// | **连续标点压缩** | **字身几何**（不是断点约束） | T014 `squeeze` |
/// | **行内中西文之间的最小间隙** | **字身几何** | T014 `mixed` |
/// | **行首行尾空白折叠** | **文本预处理** | T011（前导空格按段内处理） |
///
/// 把"连续标点压缩"塞进禁则表会造成**实质错误**：它的 `chars` 与逗号/
/// 句号规则重叠，`break_decision` 无法确定该报哪个 rule_id（实测初版即
/// 此 bug，`forbidden_sets_match_individual_rules` 直接抓出归属冲突）。
///
/// 故此处把三类**非断点**语法点显式记账，指向它们的真实落点——
/// FR-007 的"清单"因此是"禁则表 + 本记账"两部分，覆盖完整且无重复归属。
pub const NON_KINSOKU_COVERAGE: &[(&str, &str)] = &[
    (
        "consecutive_punctuation_squeeze",
        "连续标点压缩：字身几何，见 rules::squeeze（T014）",
    ),
    (
        "mixed_script_minimum_gap",
        "行内中西文最小间隙：字身几何，见 rules::mixed（T014）",
    ),
    (
        "leading_trailing_whitespace_folding",
        "行首行尾空白折叠：文本预处理，见 text::paragraph（T011）",
    ),
];

/// 所有「行首禁则」字符的并集。
#[must_use]
pub fn line_start_forbidden_chars() -> String {
    KINSOKU_RULES
        .iter()
        .filter(|rule| rule.kind == RuleKind::LineStartForbidden)
        .flat_map(|rule| rule.chars.chars())
        .collect()
}

/// 所有「行尾禁则」字符的并集。
#[must_use]
pub fn line_end_forbidden_chars() -> String {
    KINSOKU_RULES
        .iter()
        .filter(|rule| rule.kind == RuleKind::LineEndForbidden)
        .flat_map(|rule| rule.chars.chars())
        .collect()
}

/// 某字符是否不得置于行首；命中时返回**具体规则 id**。
///
/// ## 为什么返回规则 id 而不只是 `bool`
///
/// FR-008 的第 ④ 步（放弃禁则）要求记录的例外含"触发的规则"。
/// 若此处只返回 `bool`，下游就得反查是哪条规则——那等于把同一套判定
/// 实现两遍（且两处会漂移）。返回 id 让"判定"与"记账"用同一个来源。
#[must_use]
pub fn line_start_rule_for(ch: char) -> Option<&'static str> {
    KINSOKU_RULES
        .iter()
        .find(|rule| rule.kind == RuleKind::LineStartForbidden && rule.chars.contains(ch))
        .map(|rule| rule.id)
}

/// 某字符是否不得置于行尾；命中时返回具体规则 id。
#[must_use]
pub fn line_end_rule_for(ch: char) -> Option<&'static str> {
    KINSOKU_RULES
        .iter()
        .find(|rule| rule.kind == RuleKind::LineEndForbidden && rule.chars.contains(ch))
        .map(|rule| rule.id)
}

/// 判断**相邻两字符之间**能否断行（禁则层面）。
///
/// 这是禁则表的主要消费入口：`left` 是断点前一个字符，`right` 是断点后
/// 一个字符。返回 [`BreakDecision`] 说明能否断、若不能则因何规则。
///
/// ## 判据顺序（顺序本身是契约）
///
/// 1. **补丁配对优先**：`left` 与 `right` 构成破折号对/省略号对 → 不可断。
///    这一条必须**先判**——因为配对的首字符（`—`/`…`）同时也在行首禁则表里，
///    若先判行首禁则会得到"配对起点也不可断"的错误结论（实测初版即此 bug：
///    「久……然后」的「…」被判为不可断，导致省略号永远无法起行）。
/// 2. `right` 是行首禁则字符 → 不可断（会把 `right` 推到行首）
/// 3. `left` 是行尾禁则字符 → 不可断（会把 `left` 留在行尾）
/// 4. 其余 → 可断
///
/// ## 与行首禁则表的关系（省略号 / 破折号的特殊性）
///
/// `…` 与 `—` 在**行首禁则表里**表示"一个省略号/破折号的后半不可起行"；
/// 但它们作为**配对起点**时起行是合法的（中文排版里 `……` 常在行首）。
/// 上方的判据顺序正是为表达这条：配对内部由第 1 条拦下，
/// 配对起点由第 2 条放行（因为 `right` 是后半个字符时才在表里命中——
/// 而那种情形已被第 1 条处理）。
///
/// 注意本函数**只做禁则判定**，不含挤压/悬挂/整字回退（那些是 T014/T015
/// 的硬序消解）。故它返回"理想情形下能否断"。
#[must_use]
pub fn break_decision(left: char, right: char) -> BreakDecision {
    // 1. 配对内部优先（见文档：顺序是契约）
    if is_inside_patch_pair(left, right) {
        let rule_id = if left == '\u{2014}' {
            "kinsoku.patch.em_dash_pair_not_splittable"
        } else {
            "kinsoku.patch.ellipsis_pair_not_splittable"
        };
        return BreakDecision::Forbidden {
            rule_id,
            reason: ForbiddenSide::PatchPair,
        };
    }

    // 2/3. 行首 / 行尾禁则
    if let Some(rule_id) = line_start_rule_for(right) {
        return BreakDecision::Forbidden {
            rule_id,
            reason: ForbiddenSide::RightWouldStartLine,
        };
    }
    if let Some(rule_id) = line_end_rule_for(left) {
        return BreakDecision::Forbidden {
            rule_id,
            reason: ForbiddenSide::LeftWouldEndLine,
        };
    }

    BreakDecision::Allowed
}

/// 断点判定的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakDecision {
    /// 可断
    Allowed,
    /// 不可断（禁则约束）
    Forbidden {
        /// 触发的规则 id（供 FR-008 第 ④ 步的例外记录）
        rule_id: &'static str,
        /// 违反哪一侧
        reason: ForbiddenSide,
    },
}

impl BreakDecision {
    /// 是否可断。
    #[must_use]
    pub fn is_allowed(self) -> bool {
        matches!(self, Self::Allowed)
    }
}

/// 禁则违反的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForbiddenSide {
    /// 断开会把右字符推到行首
    RightWouldStartLine,
    /// 断开会把左字符留在行尾
    LeftWouldEndLine,
    /// 落在补丁条目的不可断配对内部
    PatchPair,
}

/// 两个字符是否构成"不可断的补丁配对"（破折号对 / 省略号对）。
///
/// ## 判据
///
/// 中文排版里 `——`（两个 U+2014）与 `……`（两个 U+2026）是**一个字**的
/// 两个半身，必须整体进退。故配对内部（第一个与第二个之间）不可断。
///
/// 本函数只判"这两个字符是否构成配对内部"，不判"配对是否完整"——
/// 后者需要看整行上下文，属 T013 的引擎职责。
#[must_use]
pub fn is_inside_patch_pair(left: char, right: char) -> bool {
    (left == '\u{2014}' && right == '\u{2014}') || (left == '\u{2026}' && right == '\u{2026}')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SC-001 的主体：**每条**规则都有判定例与反例。
    ///
    /// ## 这条测试在断言什么
    ///
    /// 断言的是"清单的**完整性**"——即每个条目都填了 `basis` /
    /// `positive_case` / `negative_case`，且它们真的提到了该规则的字符。
    /// 空字符串或与规则无关的占位文本会被抓出来。
    #[test]
    fn every_rule_has_basis_and_cases() {
        assert!(!KINSOKU_RULES.is_empty(), "清单不得为空");

        for rule in KINSOKU_RULES {
            assert!(
                rule.id.starts_with("kinsoku."),
                "规则 id 须以 kinsoku. 起（可 grep 的稳定标识）：{}",
                rule.id
            );
            assert!(
                !rule.basis.trim().is_empty(),
                "规则 {} 缺依据（干净室纪律要求依据成文）",
                rule.id
            );
            assert!(
                !rule.positive_case.trim().is_empty(),
                "规则 {} 缺判定例（SC-001 要求）",
                rule.id
            );
            assert!(
                !rule.negative_case.trim().is_empty(),
                "规则 {} 缺反例（SC-001 要求）",
                rule.id
            );
            assert!(
                !rule.chars.is_empty(),
                "规则 {} 的字符集为空（该规则永不生效）",
                rule.id
            );

            // 判定例必须真的含该规则字符集里的某个字符——否则它是占位文本
            let mentions_own_char = rule.positive_case.chars().any(|ch| rule.chars.contains(ch));
            assert!(
                mentions_own_char,
                "规则 {} 的判定例未提及本规则的任何字符，疑似占位：{:?}",
                rule.id, rule.positive_case
            );
        }
    }

    /// 规则 id 唯一（重名会让例外记录指不清是哪条）。
    #[test]
    fn rule_ids_are_unique() {
        let mut ids: Vec<&str> = KINSOKU_RULES.iter().map(|rule| rule.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "规则 id 有重复");
    }

    /// 三种类别都非空（FR-007 要求覆盖多个语法点）。
    #[test]
    fn all_rule_kinds_are_present() {
        for kind in [
            RuleKind::LineStartForbidden,
            RuleKind::LineEndForbidden,
            RuleKind::Patch,
        ] {
            assert!(
                KINSOKU_RULES.iter().any(|rule| rule.kind == kind),
                "清单缺少类别 {kind:?}"
            );
        }
    }

    /// 补丁条目的依据必须**显式声明它是增补**（干净室纪律的落点）。
    ///
    /// ## 为什么单列一条测试
    ///
    /// 为兼容中文惯例而写的非常规规则，若在依据里伪装成"规范原文"，
    /// 就失去了"为什么偏离"的可追溯性。故强制要求补丁条目的依据里
    /// 出现"增补"或"非规范原文"字样。
    #[test]
    fn patch_rules_declare_themselves_as_supplements() {
        for rule in KINSOKU_RULES
            .iter()
            .filter(|rule| rule.kind == RuleKind::Patch)
        {
            assert!(
                rule.basis.contains("增补") || rule.basis.contains("非规范原文"),
                "补丁规则 {} 的依据未声明它是相对规范的增补：{:?}",
                rule.id,
                rule.basis
            );
        }
    }

    /// 判定例与反例必须**结论相反**——这是 SC-001 的真正含义。
    ///
    /// ## 怎么验证"结论相反"
    ///
    /// 对行首禁则规则：判定例应描述"该字符在行首是不允许的"，反例应描述
    /// "该字符在另一个位置是允许的"（或涉及不受本规则约束的字符）。
    /// 无法从自然语言自动判定，故此处做**结构性**检查：
    /// 两条用例的文本必须不同（相同则显然是复制粘贴）。
    #[test]
    fn positive_and_negative_cases_differ() {
        for rule in KINSOKU_RULES {
            assert_ne!(
                rule.positive_case, rule.negative_case,
                "规则 {} 的判定例与反例相同（SC-001 要求反例能在规则失效时失败）",
                rule.id
            );
        }
    }

    // ------------------------------------------------------------------
    // 判定函数的行为（逐项）
    // ------------------------------------------------------------------

    /// 行首禁则：闭标点后不可断。
    #[test]
    fn closing_punctuation_cannot_start_a_line() {
        let decision = break_decision('甲', '）');
        assert!(!decision.is_allowed());
        match decision {
            BreakDecision::Forbidden { rule_id, reason } => {
                assert_eq!(rule_id, "kinsoku.line_start.closing_bracket_cjk");
                assert_eq!(reason, ForbiddenSide::RightWouldStartLine);
            }
            BreakDecision::Allowed => unreachable!(),
        }

        assert!(!break_decision('甲', '，').is_allowed(), "逗号");
        assert!(!break_decision('甲', '。').is_allowed(), "句号");
        assert!(!break_decision('甲', '？').is_allowed(), "问号");
        assert!(!break_decision('甲', '”').is_allowed(), "后引号");
    }

    /// 行尾禁则：开标点前不可断。
    #[test]
    fn opening_punctuation_cannot_end_a_line() {
        let decision = break_decision('（', '甲');
        assert!(!decision.is_allowed());
        match decision {
            BreakDecision::Forbidden { rule_id, reason } => {
                assert_eq!(rule_id, "kinsoku.line_end.opening_bracket_cjk");
                assert_eq!(reason, ForbiddenSide::LeftWouldEndLine);
            }
            BreakDecision::Allowed => unreachable!(),
        }

        assert!(!break_decision('「', '甲').is_allowed(), "前引号");
        assert!(!break_decision('“', '甲').is_allowed());
    }

    /// 普通汉字之间可断（反例：禁则不该过度约束）。
    #[test]
    fn ordinary_chars_break_freely() {
        assert!(break_decision('甲', '乙').is_allowed());
        assert!(break_decision('，', '甲').is_allowed(), "逗号后可以断");
        assert!(break_decision('甲', '（').is_allowed(), "开括号前可以断");
        assert!(break_decision('）', '甲').is_allowed(), "闭括号后可以断");
    }

    /// 补丁：破折号对内部不可断。
    #[test]
    fn em_dash_pair_internal_break_is_forbidden() {
        let decision = break_decision('\u{2014}', '\u{2014}');
        assert!(!decision.is_allowed());
        match decision {
            BreakDecision::Forbidden { reason, .. } => {
                assert_eq!(reason, ForbiddenSide::PatchPair);
            }
            BreakDecision::Allowed => unreachable!(),
        }

        // 破折号**之前**可断（配对起点）
        assert!(
            break_decision('说', '\u{2014}').is_allowed(),
            "破折号之前（配对起点）可以断"
        );
    }

    /// 补丁：省略号对内部不可断。
    #[test]
    fn ellipsis_pair_internal_break_is_forbidden() {
        assert!(!break_decision('\u{2026}', '\u{2026}').is_allowed());
        assert!(
            break_decision('久', '\u{2026}').is_allowed(),
            "省略号之前（配对起点）必须可断——否则省略号永远无法起行。\n\
             这是初版的真实 bug：配对起点同时命中行首禁则表，\n\
             若判据顺序把行首禁则排在配对判定之前，就会得到错误结论。"
        );
    }

    /// **判据顺序是契约**：配对起点可断、配对内部不可断。
    ///
    /// 这条把"顺序"这件事本身写成断言——它是唯一能区分
    /// 「`——` 起点可断、内部不可断」的判据。
    #[test]
    fn patch_pair_boundary_precedence() {
        // 起点可断
        assert!(break_decision('说', '\u{2014}').is_allowed(), "破折号起点");
        assert!(break_decision('久', '\u{2026}').is_allowed(), "省略号起点");
        // 内部不可断
        assert!(!break_decision('\u{2014}', '\u{2014}').is_allowed());
        assert!(!break_decision('\u{2026}', '\u{2026}').is_allowed());
        // 终点可断
        assert!(break_decision('\u{2014}', '那').is_allowed(), "破折号终点");
        assert!(break_decision('\u{2026}', '然').is_allowed(), "省略号终点");
    }

    /// 禁则集合的并集与逐条规则一致（防两处独立计算漂移）。
    #[test]
    fn forbidden_sets_match_individual_rules() {
        let start = line_start_forbidden_chars();
        for rule in KINSOKU_RULES
            .iter()
            .filter(|rule| rule.kind == RuleKind::LineStartForbidden)
        {
            for ch in rule.chars.chars() {
                assert!(
                    start.contains(ch),
                    "字符 {ch} 属于规则 {} 但不在行首禁则并集里",
                    rule.id
                );
                assert_eq!(
                    line_start_rule_for(ch),
                    Some(rule.id),
                    "字符 {ch} 的规则归属与并集不一致"
                );
            }
        }
        // 并集里不该有非行首禁则的字符
        assert!(!start.contains('（'), "开括号是行尾禁则，不该在行首禁则里");
    }

    /// `line_start_rule_for` / `line_end_rule_for` 对无关字符返回 `None`。
    #[test]
    fn unrelated_chars_have_no_rule() {
        assert_eq!(line_start_rule_for('甲'), None);
        assert_eq!(line_start_rule_for('a'), None);
        assert_eq!(line_end_rule_for('甲'), None);
        assert_eq!(line_end_rule_for('）'), None, "闭括号不是行尾禁则");
    }

    /// 清单条数与内容成文（SC-001 的"条目数在实现时确定并记录"）。
    #[test]
    fn rule_count_is_documented() {
        // 条数写死在此：清单被意外增删时这条测试会红，迫使改动者
        // 同步更新模块文档里的记账。
        assert_eq!(
            KINSOKU_RULES.len(),
            12,
            "清单条数变了——须同步更新本模块文档的「清单规模」记账"
        );

        let start_count = KINSOKU_RULES
            .iter()
            .filter(|rule| rule.kind == RuleKind::LineStartForbidden)
            .count();
        let end_count = KINSOKU_RULES
            .iter()
            .filter(|rule| rule.kind == RuleKind::LineEndForbidden)
            .count();
        let patch_count = KINSOKU_RULES
            .iter()
            .filter(|rule| rule.kind == RuleKind::Patch)
            .count();
        assert_eq!(start_count, 7, "行首禁则 7 条（省略号已移出，见清单注释）");
        assert_eq!(end_count, 2, "行尾禁则 2 条");
        assert_eq!(patch_count, 3, "补丁 3 条（破折号对 / 省略号对 / 数位组）");
    }

    /// FR-007 的**非禁则**语法点也必须有落点（记账完整）。
    ///
    /// ## 为什么这条测试存在
    ///
    /// FR-007 的"至少覆盖"含七个语法点，其中三个不是断点约束
    /// （连续标点压缩 / 中西文间隙 / 空白折叠）。它们若没有显式记账，
    /// 就会出现"清单看起来齐全、实际漏了半条需求"的缺口——
    /// 而那正是 SC-001 想防的事。
    #[test]
    fn non_kinsoku_coverage_is_recorded() {
        assert_eq!(
            NON_KINSOKU_COVERAGE.len(),
            3,
            "FR-007 的三个非断点语法点都须记账"
        );
        for (name, location) in NON_KINSOKU_COVERAGE {
            assert!(!name.is_empty(), "记账项须有名字");
            assert!(
                location.contains("T0") || location.contains("见"),
                "记账项 {name} 须指向具体落点（模块 + 任务号），实际：{location}"
            );
        }
    }

    /// 依据必须引用**可核对的来源**（防"随意"这类占位）。
    #[test]
    fn bases_cite_known_sources() {
        const KNOWN: &[&str] = &["UAX#14", "CLreq", "GB/T 15834", "FR-", "增补"];
        for rule in KINSOKU_RULES {
            assert!(
                KNOWN.iter().any(|source| rule.basis.contains(source)),
                "规则 {} 的依据未引用已知来源（{KNOWN:?}）：{:?}",
                rule.id,
                rule.basis
            );
            for placeholder in ["照抄", "上游源码", "TODO", "待补"] {
                assert!(
                    !rule.basis.contains(placeholder),
                    "规则 {} 的依据含占位词 `{placeholder}`",
                    rule.id
                );
            }
        }
    }
}
