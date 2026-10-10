//! T033 验收：字形缓存 + atlas（FR-043、FR-047；research R2）
//!
//! ## 本文件覆盖的验收子项（tasks.md 002 T033）
//!
//! - 缓存键 = 字体 + 字号 + 字形 + **SubpixelBin（x 轴 4 分桶、y 对齐行基线）**
//! - 缓存命中/未命中路径测试
//! - atlas 增长后既有字形仍可渲染（`copy_texture_to_texture` 搬迁）
//! - **US3 场景 2**：缺字回退字形呈现且该行其余字符位置不变（不整行跳动）
//!
//! ## 亚像素分桶为什么是 4 个而不是更多
//!
//! research R2 定的键是 `swash::SubpixelBin` 的 4 分桶（`Zero/One/Two/Three`，
//! 每桶 1/4 像素）。更细的桶会让缓存条目按比例膨胀，而 1/4 像素已是
//! 亚像素定位的可见精度下限——再多分的收益被内存代价吃掉。
//!
//! **y 轴不用分桶**：正文的 y 位置是行基线（本来就对齐到行的整数位置），
//! 分桶只会为同一个字形制造无意义的重复条目。故 y 恒取 `Zero` 桶——
//! 这是"滚动场景只在 x 轴亚像素"（R2 的原话）的落点。
//!
//! ## 缓存判据必须能实际失败
//!
//! "命中率 100%"这种断言是**空**的——一个永远返回同一张位图的实现也能通过。
//! 故本文件的判据形态是**差分**：不同键必须产出不同条目（未命中路径），
//! 相同键必须复用同一条目（命中路径），且两条路径的计数都要被观测。

#![cfg(feature = "render")]

use cosmic_text::fontdb;
use tome_reader::render::glyph_cache::{GlyphCache, GlyphCacheKey, SubpixelBucket};
use tome_reader::render::pipeline::RenderPipeline;

/// 取一次后端（无可用后端时按 FR-053 显式跳过，且**不计为通过**）。
///
/// ## 为什么只有 `NoAdapter` 算跳过
///
/// `NoAdapter` 是**预期内**的结局（环境无 GPU、CI 无 lavapipe）。其余失败
/// （device 请求被拒、尺寸非法、poll 未就绪）是**装置或代码**的问题——把它们
/// 也算成跳过，会让"渲染装置坏了"看起来像"本机没 GPU"，那正是 FR-053 要防的
/// 伪装。故那些分支 panic，让测试红。
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "夹具内的 panic/expect 是刻意的：非能力缺失的失败必须让测试红，不得被当成跳过"
)]
fn pipeline_or_skip(context: &str) -> Option<RenderPipeline> {
    use tome_reader::render::pipeline::PipelineError;

    match RenderPipeline::probe_backend() {
        Ok(backend) => Some(RenderPipeline::new(backend, 512, 512).expect("有后端时应能建管线")),
        Err(PipelineError::NoAdapter { reason }) => {
            println!("[T033] SKIPPED ({context}): 无可用图形后端——{reason}");
            println!("[T033] 本条**不计为通过**（FR-053）");
            None
        }
        Err(other) => panic!("[T033] 后端探测以非'无适配器'的方式失败：{other}"),
    }
}

/// 建一个用**真实系统字体**的字形缓存（字形来源与 T037 的字体栈同源）。
fn cache_with_font() -> GlyphCache {
    GlyphCache::new()
}

/// 建一个**小图集**的缓存（让增长路径真的被走到）。
///
/// 默认 1024² 能装数千个字形，用它测增长要么塞几千个（慢）要么走不到
/// 增长分支——后者是 spike §10.1 记的"全绿但没测到"。
fn small_cache() -> GlyphCache {
    GlyphCache::with_atlas_size(64)
}

// ===========================================================================
// 缓存键：分桶语义
// ===========================================================================

/// **x 轴 4 分桶**：0.0 / 0.25 / 0.5 / 0.75 各落一个桶，且相邻桶不同。
#[test]
fn x_axis_has_four_subpixel_buckets() {
    let lanes = [0.0f32, 0.25, 0.5, 0.75];
    let buckets: Vec<SubpixelBucket> = lanes
        .iter()
        .map(|offset| SubpixelBucket::for_x(*offset))
        .collect();

    assert_eq!(
        buckets,
        vec![
            SubpixelBucket::Zero,
            SubpixelBucket::One,
            SubpixelBucket::Two,
            SubpixelBucket::Three,
        ],
        "0.0/0.25/0.5/0.75 应依次落入四个不同的桶，实测 {buckets:?}"
    );

    // 每个桶的边界各归一侧（不重叠、无空洞）
    for lane in lanes {
        let bucket = SubpixelBucket::for_x(lane);
        let center = bucket.center_px();
        assert!(
            (center - lane).abs() < 0.25,
            "桶 {bucket:?} 的中心 {center} 应靠近 {lane}"
        );
    }
}

/// 同桶内的两个偏移**映射到同一桶**（合并的意义所在）。
#[test]
fn same_bucket_merges_nearby_offsets() {
    // 0.02 与 0.12 都在 [0, 0.125) 之外的 Zero 桶区间内？
    // 按 SubpixelBin 的划分：fract < 0.125 → Zero。故 0.02 与 0.10 同桶。
    assert_eq!(
        SubpixelBucket::for_x(0.02),
        SubpixelBucket::for_x(0.10),
        "同在 [0, 0.125) 的两个偏移应合并为一桶"
    );
    assert_eq!(
        SubpixelBucket::for_x(0.02),
        SubpixelBucket::Zero,
        "偏移 0.02 应落 Zero 桶"
    );
}

/// **y 轴不分桶**：任何 y 偏移都归 `Zero`（行基线对齐）。
///
/// 这是 research R2 "滚动场景只在 x 轴亚像素、y 对齐行基线取整"的直接落点。
#[test]
fn y_axis_always_uses_the_zero_bucket() {
    for y in [0.0f32, 0.1, 0.25, 0.5, 0.75, 0.9, 1.25] {
        assert_eq!(
            SubpixelBucket::for_y(y),
            SubpixelBucket::Zero,
            "y={y} 也应归 Zero 桶（y 对齐行基线，不分桶）"
        );
    }
}

/// 缓存键的**相等性**：四个维度各自参与（字体/字号/字形/桶）。
#[test]
fn cache_key_distinguishes_each_dimension() {
    let base = GlyphCacheKey {
        font_id: fontdb::ID::dummy(),
        glyph_id: 42,
        font_size_bits: 19.0f32.to_bits(),
        bucket: SubpixelBucket::Zero,
    };

    let different_size = GlyphCacheKey {
        font_size_bits: 24.0f32.to_bits(),
        ..base
    };
    let different_glyph = GlyphCacheKey {
        glyph_id: 43,
        ..base
    };
    let different_font = GlyphCacheKey {
        font_id: fontdb::ID::default(),
        ..base
    };
    let different_bucket = GlyphCacheKey {
        bucket: SubpixelBucket::Two,
        ..base
    };

    for (name, other) in [
        ("字号", different_size),
        ("字形", different_glyph),
        ("字体", different_font),
        ("亚像素桶", different_bucket),
    ] {
        assert_ne!(base, other, "{name} 应参与缓存键的相等性判定");
    }
    assert_eq!(base, GlyphCacheKey { ..base }, "同键应相等");
}

// ===========================================================================
// 缓存命中/未命中（判据必须能实际失败）
// ===========================================================================

/// **命中与未命中两条路径都被观测到**（差分判据，不是"永远命中"）。
#[test]
fn cache_hit_and_miss_are_both_observable() {
    let Some(pipeline) = pipeline_or_skip("cache_hit_and_miss_are_both_observable") else {
        return;
    };
    let mut cache = cache_with_font();

    // 取一个真实字体 ID 与两个真实字形（用 ASCII 与汉字各一）
    let Some(first) = cache.lookup_first_glyph('A') else {
        println!("[T033] SKIPPED: 系统字体库取不到可栅格化的字形");
        return;
    };
    let Some(second) = cache.lookup_first_glyph('B') else {
        println!("[T033] SKIPPED: 系统字体库取不到第二个字形");
        return;
    };

    let miss_before = cache.miss_count();
    let hit_before = cache.hit_count();

    // 首次访问两个不同字形 → 两次未命中
    let _ = cache
        .rasterize(&pipeline, first, 19.0, 0.0, 0.0)
        .expect("应能栅格化首个字形");
    let _ = cache
        .rasterize(&pipeline, second, 19.0, 0.0, 0.0)
        .expect("应能栅格化次个字形");

    assert_eq!(
        cache.miss_count() - miss_before,
        2,
        "两个不同字形应各产生一次未命中"
    );

    // 再访问同一个键 → 命中
    let _ = cache
        .rasterize(&pipeline, first, 19.0, 0.0, 0.0)
        .expect("重复访问应命中");
    assert!(
        cache.hit_count() > hit_before,
        "重复访问同一键应产生命中（实测 hit {} → {}）",
        hit_before,
        cache.hit_count()
    );
}

/// **不同亚像素桶是同一条目的不同版本**（4 分桶真的生效）。
#[test]
fn different_subpixel_buckets_are_different_entries() {
    let Some(pipeline) = pipeline_or_skip("different_subpixel_buckets_are_different_entries")
    else {
        return;
    };
    let mut cache = cache_with_font();

    let Some(glyph) = cache.lookup_first_glyph('A') else {
        println!("[T033] SKIPPED: 系统字体库取不到字形");
        return;
    };

    let before = cache.miss_count();
    // 四个桶各访问一次 → 四次未命中（桶若未参与键，后三次会是命中）
    for offset in [0.0f32, 0.25, 0.5, 0.75] {
        let _ = cache
            .rasterize(&pipeline, glyph, 19.0, offset, 0.0)
            .expect("应能栅格化");
    }

    assert_eq!(
        cache.miss_count() - before,
        4,
        "四个亚像素桶应各产生一次未命中（若桶未参与键则只有 1 次）"
    );
}

// ===========================================================================
// atlas：增长后既有字形仍可用
// ===========================================================================

/// **atlas 增长后既有字形仍可渲染**（搬迁不丢内容）。
///
/// ## 为什么这条是本任务的核心判据
///
/// research R2 的第 4 条决策是"atlas 增长用 `copy_texture_to_texture` 整体搬迁，
/// 避免 glyphon 式全量重栅格化"。搬迁的实现风险在于：搬完之后**旧条目的
/// 坐标/origin 必须更新**，否则它们会指向不复存在的区域（表现为"翻页后
/// 先前页面的字形变成空白或错位"）。故判据是"搬迁后取旧条目，其 atlas 区域
/// 仍在纹理内且数据非空"。
#[test]
fn existing_glyphs_survive_atlas_growth() {
    let Some(pipeline) = pipeline_or_skip("existing_glyphs_survive_atlas_growth") else {
        return;
    };
    let mut cache = small_cache();

    // 先塞入若干字形，记下第一个条目的位置
    let Some(first_glyph) = cache.lookup_first_glyph('W') else {
        println!("[T033] SKIPPED: 系统字体库取不到字形");
        return;
    };
    let first_entry = cache
        .rasterize(&pipeline, first_glyph, 19.0, 0.0, 0.0)
        .expect("应能栅格化");
    let first_rect = first_entry.rect;
    let first_pixels = first_entry.pixel_count();

    assert!(
        first_rect.width > 0 && first_rect.height > 0,
        "首个条目应有非空区域，实测 {first_rect:?}"
    );
    assert!(first_pixels > 0, "首个条目应有像素数据");

    // 强制增长：塞入远超初始容量的不同字形×字号组合
    let mut inserted = 0usize;
    for ch in 'A'..='Z' {
        for size in [14.0f32, 17.0, 19.0, 24.0, 30.0] {
            if let Some(glyph) = cache.lookup_first_glyph(ch) {
                if cache.rasterize(&pipeline, glyph, size, 0.0, 0.0).is_some() {
                    inserted += 1;
                }
            }
        }
    }
    assert!(
        inserted > 20,
        "应插入足够多的字形以触发 atlas 增长，实测 {inserted}"
    );

    // 增长之后：旧条目仍在纹理内，且数据仍非空
    let after = cache
        .entry_for(&first_glyph, 19.0, 0.0)
        .expect("增长后旧字形应仍在缓存里");
    assert_eq!(
        after.pixel_count(),
        first_pixels,
        "搬迁后内容长度应不变（搬迁不丢数据）"
    );
    assert!(
        cache.atlas_size().0 >= after.rect.x + after.rect.width
            && cache.atlas_size().1 >= after.rect.y + after.rect.height,
        "搬迁后旧条目的区域应落在当前纹理内：条目 {:?} 而纹理 {:?}",
        after.rect,
        cache.atlas_size()
    );
    assert!(
        cache.grow_count() > 0,
        "本用例应至少触发一次 atlas 增长，实测 {}",
        cache.grow_count()
    );

    // ## 真正的判据：**没有两个条目重叠**
    //
    // 上面那条"旧条目落在纹理内"**不足以**证明搬迁正确——坐标没更新时它们
    // 照样在更大的纹理内（旧尺寸是新的子集）。真正的损坏形态是**重叠**：
    // 新分配不知道旧区域被占用，会把新字形放进旧条目的位置上，覆盖它的内容。
    //
    // 症状是"翻页后先前页面的字形变成别的字"，出现在**别的页面**上，
    // 极难从现象反推。故判据必须直接测重叠。
    assert!(
        cache.entries_are_non_overlapping(),
        "搬迁后全部条目不得重叠——重叠意味着新分配覆盖了既有字形的内容"
    );
    assert!(
        cache.len() > 1,
        "应有多个条目参与重叠判定，实测 {}",
        cache.len()
    );
}

// ===========================================================================
// US3 场景 2：缺字回退不整行跳动（FR-047）
// ===========================================================================

/// **回退字形的存在不改变该行其余字符的位置**（FR-047 / US3 场景 2）。
///
/// ## 判据为什么是"位置不变"而不是"画出来了"
///
/// FR-047 的原话是"回退 MUST NOT 改变该行其余字符的位置"——即回退的**代价
/// 被限制在被回退的那个字形上**。若实现让回退字形带上不同的前进量，
/// 后续字符会整行右移（"整行跳动"），那正是本条要防的。
///
/// 判据取**布局层**的字符区间与 x 位置：同一段文本里，把某个字换成
/// 内嵌子集缺字的生僻字之后，其余字符的 `x_px` 与字符区间必须逐字相同。
#[test]
fn missing_glyph_fallback_does_not_shift_the_line() {
    use tome_reader::layout::engine::{Geometry, LayoutEngine};
    use tome_reader::text::model::TextModel;

    // 「」内的字是内嵌子集**很可能**不含的（CJK 扩展 B 区），用于触发回退；
    // 但本判据不依赖"它一定缺字"——即使系统字体覆盖了它，位置也不该变。
    let normal = "读书记录";
    let with_rare = "读书𠀋录"; // U+2000B：CJK 扩展 B

    let mut engine = LayoutEngine::new();
    let geometry = Geometry::default_ma();

    let normal_layout = engine.layout_paragraph(&TextModel::new(normal), 0, &geometry);
    let rare_layout = engine.layout_paragraph(&TextModel::new(with_rare), 0, &geometry);

    // 「录」在两个文本里的**相对位置**应一致（其字形 x 由前面的字符决定；
    // 回退字形若带不同的前进量，这里会分叉）
    let position_of =
        |result: &tome_reader::layout::engine::LayoutResult, char_index: usize| -> Option<f32> {
            result
                .lines
                .first()?
                .glyphs
                .get(char_index)
                .map(|glyph| glyph.x_px)
        };

    // 索引 2 是「记」/「𠀋」，索引 3 是「录」
    let normal_last = position_of(&normal_layout, 3).expect("正常文本应有第 4 个字形");
    let rare_last = position_of(&rare_layout, 3).expect("含生僻字的文本应有第 4 个字形");

    // 若生僻字走了回退**且回退字形的前进量与正常字不同**，两者会分叉
    let delta = (normal_last - rare_last).abs();
    assert!(
        delta < 0.5,
        "缺字回退不得让该行其余字符位移：正常文本中「录」在 x={normal_last:.2}，\
         含生僻字时在 x={rare_last:.2}（差 {delta:.2}px）"
    );

    // 字符归属不变（回退不改区间——那是 FR-010 的同款不变量）
    assert_eq!(
        normal_layout.lines[0].char_range.len(),
        4,
        "字符区间长度不受回退影响"
    );
    assert_eq!(rare_layout.lines[0].char_range.len(), 4);
}

/// 缓存条目携带**来源标记**（内嵌子集 / 系统回退），供 T037 与诊断使用。
#[test]
fn entries_record_their_font_source() {
    let Some(pipeline) = pipeline_or_skip("entries_record_their_font_source") else {
        return;
    };
    let mut cache = cache_with_font();

    let Some(glyph) = cache.lookup_first_glyph('A') else {
        println!("[T033] SKIPPED: 系统字体库取不到字形");
        return;
    };
    let entry = cache
        .rasterize(&pipeline, glyph, 19.0, 0.0, 0.0)
        .expect("应能栅格化");

    // 来源标记只在 T037 接线后才有意义；此处只断言字段存在且可读
    let _source: tome_reader::render::glyph_cache::FontSource = entry.source;
}
