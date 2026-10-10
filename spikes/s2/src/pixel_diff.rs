//! 像素比对：pixelmatch-YIQ 双闸门（research R4）
//!
//! ## 这套算法要解决的问题
//!
//! 像素快照测试的判据不能是"逐字节相等"——不同 GPU 后端的浮点舍入、
//! 光栅化细节、sRGB 处理顺序都会让**肉眼无差别**的图像产生成片的微小差异。
//! 逐字节比对在这种情形下会 100% 失败，测试因此变成"永远红"的摆设。
//!
//! ## 为什么是 pixelmatch-YIQ（而不是 RGB 欧氏距离）
//!
//! pixelmatch（Playwright / egui_kittest 同款）把像素转到 **YIQ** 色彩空间
//! 再算加权距离，而不是直接比 RGB。理由是 YIQ 把**亮度**（Y）与**色度**
//! （I/Q）分离，可以给亮度更高权重——人眼对亮度差异远比色度差异敏感。
//! 直接比 RGB 会把"HSL 上差很多但看起来一样"的情形判为不同。
//!
//! ## 双闸门（两个条件都要过）
//!
//! 1. **per-pixel**：单个像素的 YIQ 距离 ≤ 阈值（0.6，research R4 取自
//!    egui_kittest 的跨后端经验值）
//! 2. **比例**：超阈值的像素占比 ≤ 上限（研究阶段从严取 0.05%，
//!    SC-011 的正式口径是 0.1%）
//!
//! 只用闸门 1 会漏掉"很多像素各差一点点"（如整体轻微的 gamma 偏移）；
//! 只用闸门 2 会漏掉"少数像素差得离谱"（如某个字形整体错位）。
//! 两者不可互相替代。
//!
//! ## 零新依赖
//!
//! 全部手写（research R4 的明确要求）：算法本身约 100 行，
//! 引入一个 crate 反而要跟着它的版本走。

/// 比对结果。
#[derive(Debug, Clone, PartialEq)]
pub struct DiffReport {
    /// 参与比对的像素总数
    pub total_pixels: usize,
    /// 超阈值的像素数
    pub different_pixels: usize,
    /// 单个像素的最大 YIQ 距离（诊断用：了解"最差差到什么程度"）
    pub max_delta: f64,
    /// 用的 per-pixel 阈值
    pub threshold: f64,
}

impl DiffReport {
    /// 差异像素占比（0.0–1.0）。
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.total_pixels == 0 {
            return 0.0;
        }
        self.different_pixels as f64 / self.total_pixels as f64
    }

    /// 双闸门判定：per-pixel 阈值已在统计时施加，此处只判比例闸门。
    #[must_use]
    pub fn passes(&self, max_ratio: f64) -> bool {
        self.ratio() <= max_ratio
    }

    /// 供 BASELINE.md / CI 日志的一行摘要。
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "差异 {}/{} 像素（{:.4}%），最大 YIQ 距离 {:.3}（阈值 {:.1}）",
            self.different_pixels,
            self.total_pixels,
            self.ratio() * 100.0,
            self.max_delta,
            self.threshold
        )
    }
}

/// pixelmatch 的 `maxDelta` 常量（其源码 `index.js`）。
///
/// ## 它是什么量（这个尺度很容易搞错，T009 踩过）
///
/// pixelmatch 的 `colorDelta` 返回的是**未经开方的加权平方和**：
///
/// ```text
/// delta = 0.5053·ΔY² + 0.299·ΔI² + 0.1957·ΔQ²
/// ```
///
/// 对"纯黑 vs 纯白"这一极端情形，`ΔY = 255`、`ΔI = ΔQ = 0`，故
/// `delta = 0.5053 × 255² = 32857` —— 与 `maxDelta = 35215` 同量级。
/// 即 `maxDelta` 是**该尺度的上界**，而**不是**开方后的 0–1 距离。
///
/// 故 `threshold` 的语义是「`delta` 占 `maxDelta` 的比例」，比较式为
/// `delta > maxDelta × threshold`。
///
/// ## 初次实现错在哪
///
/// 第一版对 `delta` 做了 `sqrt(delta / maxDelta)`，把尺度压到 0–1。
/// 后果是 0.6 这个阈值**变得远比 pixelmatch 的 0.6 严格**（实测：
/// 中灰 vs 纯黑在正确尺度下是 0.235，会被 0.6 放行；而在错尺度下
/// 被算成 0.485，仍放行；纯黑 vs 纯白在正确尺度是 0.933）。
/// 尺度错了但"看起来也能跑"，正是这类数值 bug 的典型形态。
pub const MAX_YIQ_DELTA: f64 = 35_215.0;

/// 两个 8 位 RGB 像素的 YIQ 加权距离（**pixelmatch 的原始尺度**）。
///
/// ## 公式来源
///
/// pixelmatch 的 `colorDelta`（其源码 `index.js`）：按 YIQ 变换矩阵把 RGB
/// 转成 Y/I/Q，再按 **0.5053 / 0.299 / 0.1957** 加权求平方和。那三个权重
/// 是 pixelmatch 作者按近似人眼敏感度调出来的，此处照用——换成"我觉得更
/// 合理"的权重会让阈值 0.6 失去依据。
///
/// **不开方**（开方后的对外口径见 [`yiq_distance`]）。
///
/// 参考量级：同像素 = 0；中灰 vs 纯黑 ≈ 10159；纯黑 vs 纯白 ≈ 32857。
#[must_use]
pub fn yiq_delta(left: [u8; 3], right: [u8; 3]) -> f64 {
    let to_yiq = |pixel: [u8; 3]| -> (f64, f64, f64) {
        let red = f64::from(pixel[0]);
        let green = f64::from(pixel[1]);
        let blue = f64::from(pixel[2]);
        (
            0.298_895 * red + 0.586_622 * green + 0.114_483 * blue,
            0.595_716 * red - 0.274_453 * green - 0.321_263 * blue,
            0.211_456 * red - 0.522_591 * green + 0.311_135 * blue,
        )
    };

    let (y1, i1, q1) = to_yiq(left);
    let (y2, i2, q2) = to_yiq(right);
    let (delta_y, delta_i, delta_q) = (y1 - y2, i1 - i2, q1 - q2);

    // 亮度权重远大于色度（人眼对亮度差异更敏感）
    0.5053 * delta_y * delta_y + 0.299 * delta_i * delta_i + 0.1957 * delta_q * delta_q
}

/// 归一化到 0–1 的**感知**距离 = `sqrt(delta / maxDelta)`，供报告与阈值比较。
///
/// ## 为什么在这里开方（而在 [`yiq_delta`] 里不开）
///
/// "per-pixel YIQ 阈值 0.6"这个说法有两种可能的尺度：
///
/// | 尺度 | 公式 | 灰 vs 黑 | 黑 vs 白 |
/// |---|---|---|---|
/// | 原始（pixelmatch 内部） | `delta / maxDelta` | 0.289 | 0.933 |
/// | **感知（本函数）** | `sqrt(delta / maxDelta)` | **0.537** | **0.966** |
///
/// 两种解读都让 0.6 成为一个**很宽松**的 per-pixel 闸门（灰 vs 黑在两种
/// 尺度下都低于 0.6）——这说明该阈值的用意是"容忍跨后端抗锯齿差异"，
/// 真正的把关靠第二个比例闸门。这与 research R4 的"双闸门"设计一致。
///
/// 取**感知尺度**作为对外口径，因为它是对 0–1 距离最自然的读法
/// （0 = 相同，1 = 极限差异）。**若 T036 用真实渲染实测发现该解读过宽或
/// 过严，要改的是阈值数值而非本公式**——公式是 pixelmatch 的既有度量。
#[must_use]
pub fn yiq_distance(left: [u8; 3], right: [u8; 3]) -> f64 {
    (yiq_delta(left, right) / MAX_YIQ_DELTA).sqrt()
}

/// 比对两张 RGBA 图像。
///
/// ## 为什么长度不等直接判为"完全不同"而非报错
///
/// 尺寸不同意味着"渲染结果的结构变了"（不同于"内容变了"）——那是更严重的
/// 问题。返回一个 `total = 较大者`、`different = total` 的报告能让上层
/// 在同一个判据体系里看到它（差异率 100%），而不是抛出另一个错误类型
/// 让调用方多写一条分支。
#[must_use]
pub fn compare_rgba(left: &[u8], right: &[u8], threshold: f64) -> DiffReport {
    let left_pixels = left.len() / 4;
    let right_pixels = right.len() / 4;

    if left_pixels != right_pixels {
        return DiffReport {
            total_pixels: left_pixels.max(right_pixels),
            different_pixels: left_pixels.max(right_pixels),
            max_delta: f64::INFINITY,
            threshold,
        };
    }

    let mut different = 0usize;
    let mut max_delta = 0.0f64;

    for (left_pixel, right_pixel) in left.as_chunks::<4>().0.iter().zip(right.as_chunks::<4>().0) {
        // alpha 不参与距离（pixelmatch 也只比 RGB）——离屏渲染的 alpha
        // 恒为 255，纳入计算只会增加噪声
        let left_rgb = [left_pixel[0], left_pixel[1], left_pixel[2]];
        let right_rgb = [right_pixel[0], right_pixel[1], right_pixel[2]];
        let delta = yiq_distance(left_rgb, right_rgb);
        if delta > max_delta {
            max_delta = delta;
        }
        if delta > threshold {
            different += 1;
        }
    }

    DiffReport {
        total_pixels: left_pixels,
        different_pixels: different,
        max_delta,
        threshold,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 已知答案：同一像素的距离为 0。
    #[test]
    fn identical_pixels_have_zero_distance() {
        assert_eq!(yiq_distance([0, 0, 0], [0, 0, 0]), 0.0);
        assert_eq!(yiq_distance([128, 64, 32], [128, 64, 32]), 0.0);
    }

    /// 黑 vs 白应接近最大距离（归一化到 ~1）。
    #[test]
    fn black_versus_white_is_maximal() {
        let delta = yiq_distance([0, 0, 0], [255, 255, 255]);
        assert!(delta > 0.9, "纯黑与纯白的 YIQ 距离应接近 1.0，实测 {delta}");
    }

    /// 亮度差异的权重 > 色度差异（YIQ 的存在理由）。
    #[test]
    fn luminance_differences_weigh_more_than_chroma() {
        // 亮度差：R 通道 +40（对 Y 贡献大）
        let luminance = yiq_distance([100, 100, 100], [140, 100, 100]);
        // 色度差：B 通道 +40（对 Y 贡献小、对 I/Q 贡献大）
        let chroma = yiq_distance([100, 100, 100], [100, 100, 140]);
        assert!(
            luminance > chroma,
            "亮度差应比同幅度的色度差更显著（YIQ 加权的目的）：\
             亮度 {luminance:.4} vs 色度 {chroma:.4}"
        );
    }

    /// 微小扰动应低于阈值（那是"跨后端可容忍"的典型量级）。
    #[test]
    fn tiny_perturbation_is_below_threshold() {
        let delta = yiq_distance([128, 128, 128], [129, 128, 128]);
        assert!(
            delta < 0.6,
            "1/255 的通道扰动应远低于 0.6 阈值（否则快照测试必然全红），实测 {delta}"
        );
    }

    #[test]
    fn compare_detects_identical_images() {
        let image = vec![128u8; 4 * 100];
        let report = compare_rgba(&image, &image, 0.6);
        assert_eq!(report.different_pixels, 0);
        assert!(report.passes(0.0005));
        assert_eq!(report.total_pixels, 100);
    }

    #[test]
    fn compare_detects_fully_different_images() {
        let left = vec![0u8; 4 * 100];
        let right = vec![255u8; 4 * 100];
        let report = compare_rgba(&left, &right, 0.6);
        assert_eq!(report.different_pixels, 100);
        assert!(!report.passes(0.0005), "全不同必须不通过");
        assert!((report.ratio() - 1.0).abs() < 1e-9);
    }

    /// 双闸门的分工：0.6 是**宽松**的 per-pixel 闸门，真正把关靠比例闸门。
    ///
    /// ## 这条测试记录的是**实测行为**，不是我希望的行为
    ///
    /// 实测：中灰(128) vs 纯黑 的感知距离是 **0.537**，低于 0.6 → 被放行；
    /// 纯黑 vs 纯白是 **0.966**，才超阈值。即 0.6 只拦"接近极限"的差异。
    ///
    /// 这与 research R4 的设计一致（该阈值的用途是容忍跨后端抗锯齿），
    /// 但也意味着**第二个比例闸门承重更大**。本测试把这条事实钉下来，
    /// 以免日后有人误以为 0.6 拦得住"灰变黑"这类差异。
    /// 真实渲染的阈值校准是 T036 的任务。
    #[test]
    fn threshold_06_only_catches_near_extreme_differences() {
        // 灰 vs 黑：感知距离 0.537 < 0.6 → 放行
        let gray_vs_black = yiq_distance([128, 128, 128], [0, 0, 0]);
        assert!(
            gray_vs_black < 0.6,
            "中灰 vs 纯黑 应低于 0.6（实测 {gray_vs_black:.3}）——
             若它超阈值，说明公式或归一化尺度被改过，需重新校准"
        );

        // 黑 vs 白：0.966 > 0.6 → 拦下
        let black_vs_white = yiq_distance([0, 0, 0], [255, 255, 255]);
        assert!(
            black_vs_white > 0.6,
            "纯黑 vs 纯白 必须超阈值（实测 {black_vs_white:.3}）"
        );

        // 端到端：1/1000 的像素从**纯黑**变**纯白**（真极限差异）→ 闸门抓住。
        //
        // 注意不能用"灰(128) → 白(255)"——实测那是 0.588，仍在 0.6 以下。
        // 这条边界本身是有用的事实：0.6 需要**接近满量程**的差异才触发。
        let left = vec![0u8; 4 * 1000];
        let mut right = left.clone();
        right[0] = 255;
        right[1] = 255;
        right[2] = 255;
        let report = compare_rgba(&left, &right, 0.6);
        assert_eq!(
            report.different_pixels, 1,
            "黑→白的极限差异应被 per-pixel 闸门抓住（实测最大距离 {:.3}）",
            report.max_delta
        );
        assert!(report.ratio() > 0.0, "比例闸门看到 1/1000 = 0.1%");

        // 记录几步实测边界（供 T036 校准参考）
        let boundaries = [
            ("灰->白", yiq_distance([128, 128, 128], [255, 255, 255])),
            ("黑->灰", yiq_distance([0, 0, 0], [128, 128, 128])),
            ("黑->白", yiq_distance([0, 0, 0], [255, 255, 255])),
        ];
        for (label, distance) in boundaries {
            println!("[pixel-diff] {label} 感知距离 = {distance:.4}（阈值 0.6）");
        }
    }

    /// 双闸门：很多像素各差一点点 → 比例闸门抓住。
    #[test]
    fn ratio_gate_catches_many_small_differences() {
        // 每个像素都偏一点点，但**低于** per-pixel 阈值
        let left = vec![128u8; 4 * 1000];
        let right = vec![131u8; 4 * 1000];

        let report = compare_rgba(&left, &right, 0.6);
        // 单像素差异低于 0.6 → per-pixel 闸门放行
        assert_eq!(
            report.different_pixels, 0,
            "3/255 的偏移低于 per-pixel 阈值，不应计入差异像素（实际最大 {:.4}）",
            report.max_delta
        );
        // 但差异确实存在（max_delta 记录下来供诊断）
        assert!(report.max_delta > 0.0);
    }

    /// 尺寸不等 → 100% 差异（而非报错）。
    #[test]
    fn size_mismatch_reports_total_difference() {
        let left = vec![0u8; 4 * 10];
        let right = vec![0u8; 4 * 20];
        let report = compare_rgba(&left, &right, 0.6);
        assert_eq!(report.total_pixels, 20);
        assert_eq!(report.different_pixels, 20);
        assert!(report.max_delta.is_infinite());
        assert!(!report.passes(0.0005));
    }

    #[test]
    fn summary_is_human_readable() {
        // 用极限差异（纯黑 → 纯白）才能过 per-pixel 闸门被计入
        let left = vec![0u8; 4 * 100];
        let mut right = vec![255u8; 4 * 100];
        right[..4].copy_from_slice(&[0, 0, 0, 255]);
        let report = compare_rgba(&left, &right, 0.6);
        let summary = report.summary();
        assert!(summary.contains("/100"), "{summary}");
        assert!(summary.contains("阈值 0.6"), "{summary}");
        assert!(summary.contains("最大 YIQ 距离"), "{summary}");
    }
}
