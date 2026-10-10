# Data Model: S2 排版渲染层

**Date**: 2026-10-10 | **来源**: spec.md §Key Entities（13 实体）+ Context A 契约表
**口径铁律**：本层一切位置表达为 **Unicode 标量值序列下标**（字符下标），与 001 `char_offset` 同口径；**禁止对外暴露字节偏移或 UTF-16 码元偏移**（FR-003）。

---

## 1. ReadingConfig（排版参数集）

一本书生效的阅读几何与呈现参数。**两个子集严格分离**（FR-028）：

```
ReadingConfig
  -- 几何子集（决定分页与滚动流几何；变化触发重排）
  font_size        u16      -- 阅读字号 px，14–30，默认 19
  line_height      f32      -- 行高，1.75–2.2，默认 1.85（下限经裁决 OQ#19 修正）
  paragraph_mode   enum     -- Indent{ em: f32 = 2.0 } | Gap{ em: f32 = 0.4 }（互斥，默认 Indent，OQ#12）
  measure_em       f32      -- 版心目标宽，默认 36
  measure_max_em   f32      -- 单栏绝对上限，默认 42（单页封顶并居中，I7）
  gutter_px        u16      -- 双页中缝宽，默认 64
  spread_mode      enum     -- Auto | ForceSingle | ForceSpread（手动覆盖，语义回落 OQ#14）
  reading_mode     enum     -- Paged | Scroll（阅读形态，OQ#7）
  chapter_new_page bool     -- 「章节从新页开始」开关，默认开（OQ#20；仅双页生效 OQ#5）
  -- 呈现子集（只决定绘制；变化不触发重排，FR-019）
  theme_id         enum     -- Parchment | Obsidian
  font_family      enum     -- EmbeddedSerif（内嵌子集）| 系统回退链标识
  reduce_motion    bool     -- reduced-motion 开关（FR-032）
  paper_noise      bool     -- 纸纤维噪点开关（昼主题观感项）
```

- **校验**：接受前白名单校验（字号 0/行高负/版心负宽等非法值拒绝并回退上一有效值，spec §Edge Cases）
- **序列化**：TOML/JSON 可序列化；作为 `Book.read_config`（001 契约，该字段"内容由 S2 排版层解释"）的 schema；作用域 = 全局默认，`Book.read_config` 仅在用户显式"为这本书设置"时写入覆盖（OQ#12）

## 2. Theme（主题）

单一主题模块产出的**纯值类型**（`tome-core::model::theme`，零图形依赖；昼/夜两实例由组合根构造注入，OQ#11→D）：

```
Theme
  colors       SemanticColors   -- bg_base/bg_surface/bg_elevated/bg_reading/bg_inset
                                 -- border_subtle/border_strong
                                 -- text_primary/secondary/tertiary/inverse
                                 -- accent_primary/hover/soft/secondary/clash
                                 -- state_success/warning/error/info（全部语义色，MASTER §2.1/2.2）
  spacing      [u16; 10]        -- 4px 网格 sp-1..sp-12（MASTER §4）
  radii        Radii            -- xs 4 / sm 6 / md 8 / lg 12 / full 999
  shadows      Shadows          -- sm/md/lg（昼暖褐投影/夜纯黑投影）
  motion       Motion           -- ease_out/ease_spring 曲线常量 + dur_fast 150/dur_base 200/dur_slow 280
  fonts        FontRoles        -- reading（衬线）/ ui（无衬线）/ mono（等宽）三角色
```

- **不变量**：分页逻辑只读 ReadingConfig 几何子集，**禁止读 Theme**（FR-019/SC-005）；正文色不低于 `--text-secondary`（FR-030）；行高 ≥1.75 由 ReadingConfig 校验强制（FR-031）

## 3. Paragraph（段落）

```
Paragraph
  char_range   (usize, usize)   -- 在章节正文中的字符区间 [start, end)
  text         String           -- 段内文本（或共享正文的区间视图，实现时定）
  style        ParaStyle        -- Body | ChapterTitle | Emphasis | Quote | Centered（FR-010 最小集）
```

- 段落边界：默认仅换行符（FR-001）；启发式启用时含「前导两全角空格」边界 + 诊断记录（FR-002）

## 4. LineBox（行盒）

```
LineBox
  para_index      usize         -- 所属段
  char_range      (usize, usize)
  glyphs          Vec<LayoutGlyph> -- 行内字形簇序列及相对位置（x/baseline）
  height          f32           -- 行高（px）
  baseline        f32
  squeeze_amount  f32           -- 本行标点压缩总量（px）
  rule_exception  Option<RuleExceptionRecord>  -- 触发硬序第 ④ 步时记录
```

## 5. Page（页，翻页模式）

```
Page
  chapter_index  usize
  page_in_chapter u32           -- 章内页码
  char_range     (usize, usize) -- 页内容的字符区间（留白页为空区间）
  lines          Vec<LineBox>
  side           Option<SpreadSide>  -- 双页模式：Left(偶) | Right(奇)；单页为 None
  is_padding     bool           -- 书页语义留白页标记（契约字段名 is_padding；SC-006 不计入内容区间但须显式标记）
```

## 6. ScrollViewport（滚动视口）

```
ScrollViewport
  offset_px      f32            -- 在滚动流中的像素偏移
  char_range     (usize, usize) -- 视口覆盖的字符区间（与 Page 区间同构，FR-021）
  visible_lines  Vec<LineBox>
```

- 相邻视口区间连续无缺口无重叠（SC-006b）；视口外的行可释放（FR-026）

## 7. ReadingMode（阅读形态）

`Paged | Scroll` 枚举；两形态共享同一排版结果与锚点口径（FR-020~027）。

## 8. GlyphCacheEntry（字形缓存条目）

```
GlyphCacheEntry
  key        (FontKey, PxSize, GlyphId, SubpixelBin)  -- 亚像素桶（x 轴 4 分桶；y 对齐行基线）
  atlas_pos  (u32, u32, u32, u32)                     -- atlas 矩形
  bearing    (f32, f32)
  source     FontSource                                -- EmbeddedSubset | SystemFallback | Tofu
```

## 9. RenderBatch（渲染批次）

一页（或一帧滚动视口）的绘制指令集合：`Vec<DrawCmd>` = 背景 | 字形四边形 | 装饰（高亮/下划线/引用线）| 图片四边形。**同一输入 → 相同指令序列**（FR-048/SC-012 的载体；字形缓存命中状态差异须在预热后消除）。

## 10. PositionMap（字符位置映射）

```
PositionMap  -- 两形态共用同一类型（FR-040）
  char_to_rect(char_index) -> Rect      -- 命中反查
  point_to_char(point)     -> char_index -- 命中测试
```

- 往返误差 ≤ 半个字形（互逆约束）

## 11. TraceRange（痕迹区间）

消费自 S1 的字符区间（Bookmark/Highlight/Memo），在排版结果上解析为若干「当前页/当前视口内的可见片段」（跨边界分段，拼接无缝隙无重叠，FR-037/SC-013）。样式解析顺序：语义词表 → 颜色名 → 默认下划线 + 诊断（FR-033）。

## 12. RuleExceptionRecord（禁则例外记录）

```
RuleExceptionRecord
  char_index  usize
  rule_id     &'static str   -- 触发的规则（规则集清单内可枚举）
  context     String         -- 诊断用上下文（前后字符）
```

- FR-008 第 ④ 步（放弃禁则）时产生；可枚举可计数（SC-002 判定依据）

## 13. ImageRef（消费自 001 契约增补）

`ChapterContent.images` 的元素：字符位置（char_offset）+ 缓存键（ImageCacheRef）+ alt 文本。排版侧按显示尺寸参与行盒布局（占位高度与最终一致），不被页边界切开（FR-049）。

---

## 关系图

```
ReadingConfig ─┬─► paginate（页序列/判据/书页语义）
               └─► scroll flow（滚动流）
Theme ────────► render（仅绘制；分页禁止读）
Paragraph ──► LineBox ──► Page / ScrollViewport ──► RenderBatch
                                                  ▲
TraceRange ──（解析为可见片段）──────────────────┘
PositionMap ── Page/ScrollViewport 的双向索引
RuleExceptionRecord ── LineBox 的可选附件，聚合供 SC-002
ImageRef ──► 图片行盒（与 LineBox 同层参与布局）
```
