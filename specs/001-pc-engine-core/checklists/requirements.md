# Specification Quality Checklist: 桌面版阅读引擎内核（S0 可行性验证 + S1 引擎内核）

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-10-09
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details (languages, frameworks, APIs)
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria are technology-agnostic (no implementation details)
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] No implementation details leak into specification

## Notes

- Items marked incomplete require spec updates before `/speckit-clarify` or `/speckit-plan`

### 校验记录（2026-10-09）

**第 1 轮校验结果：全部通过（无需迭代修复）**

针对"无实现细节"一项的说明：功能需求段落（FR-001 ~ FR-044）刻意回避了技术名词——例如不写"SQLite/QuickJS/rquickjs/wgpu"，而写"结构化存储""脚本运行时""渲染后端"。已批准的技术选型统一收纳在文末 **Assumptions → 已批准的技术约束** 一节，标注为"供规划阶段承接"，属于对规划阶段的输入而非功能需求，因此不违反本项。这是对规范中"避免 HOW"与"保留已批准决策"之间张力的自觉取舍，已在此明示以便后续审查者理解。

针对"成功标准技术无关"一项的说明：SC-001 ~ SC-018 均以用户可观察的结果与量化指标表述（耗时、一致率、可读比例、平台覆盖数），未出现框架、语言或数据库名称。SC-009 体积基线以 MB 表述属产品级指标，非实现细节。

### 独立审查员复核（2026-10-09）

由独立子代理复核，结论 **通过**。审查员确认：需求编号连续无缺口无重复、四层（用户故事/边界情况/功能需求/成功标准）交叉覆盖良好、无 TODO 与占位符、范围聚焦且排除边界清晰、无 YAGNI 问题。

审查员提出 4 项"进入规划前应钉死"的定义缺口，均已就地修复：

1. **FR-005 能力集不可测** → 已展开为八个类别下的逐项可枚举清单，并新增 FR-005b 要求该清单作为一致性验证的覆盖矩阵
2. **SC-005 表述不可执行**（"名义磁盘占用"未定义）→ 已改写为可量化形式，并新增 SC-005b 校验容器确实降低了文件数量
3. **FR-019 覆盖不全**（图片鉴权获取、失败重试、缓存独立清理无对应 SC）→ 已补 SC-017（两种缓存可独立清理）与 SC-018（鉴权图片成功率）
4. **SC-004 规模依据未定** → 已明确 100 MB 为典型规模硬性目标，300 MB 为上限参考

同时修正 SC-008 的验证方式：原文"改变字号或窗口尺寸"在无界面的 S1 中不可执行，已改为"锚点在不同排版参数下解析为同一值"的参数化不变性检查。

### Clarify 澄清记录（2026-10-09，spec 已更新）

本轮通过 speckit-clarify 深度澄清，**6 问 6 答**，全部落地进规格。新增/修订需求：

| 澄清点 | 结论 | 落地 |
|---|---|---|
| 书源导入渠道 | 文件 + URL + 粘贴文本三渠道 | FR-001 |
| 书内全文搜索归属 | S1 引擎提供检索能力，S3 挂界面 | FR-019b |
| 阅读记录/时长统计 | 做，S1 只落数据层，界面后置 | FR-026b + 实体 |
| 阅读数据导出范围 | 全部阅读数据，对称可导回 | FR-033 |
| 换源进度映射 | 特征段→标题→降级三级，可观测 | FR-010b |
| Cookie 持久化 | 按源落盘，重启保留登录态 | FR-026 |

同时直接补录（无需提问的明显遗漏）：FR-037 增加规则求值轨迹输出（G6）；明确排除"书源变更追踪"（G7，YAGNI）。

**结果：FR 由 45 条增至 48 条，SC 维持 19 条。澄清后无新增 [NEEDS CLARIFICATION] 标记，无矛盾残留。**

### 残余风险（更新后，供规划阶段处理）

1. ~~FR-005 能力集边界~~ — 已修复（逐项清单 + FR-005b）
2. **SC-002 的"精选语料"定义**：合成用例与真实书源的数量下限需规划阶段确定（建议合成用例覆盖 FR-005 清单全部条目，真实书源不少于 30 份）
3. ~~SC-004 的 TXT 规模~~ — 已修复（100 MB 硬性目标 + 300 MB 上限参考）
4. **FR-029 容器容量默认值**：默认容量与章节数上限（设计共识 2 MiB / 128 章）需规划阶段确定
5. **宪法已填充**（2026-10-09）：`.specify/memory/constitution.md` 已按项目定制完成，本规格与之一致性已核对（无冲突）
6. **干净室纪律的执行边界**：可运行上游参考实现产生对照数据、但不可阅读源码——需在规划阶段落为可执行的团队约定与仓库结构约束

> 宪法缺失项（原第 5 条）已随 project-bootstrap Step 4 完成而消解。
