//! `[WIP]` 空态（S3 T012；MASTER `EmptyState`）
//!
//! ## MASTER 的硬要求
//!
//! > `EmptyState` | 插画位 + 标题 + 说明 + 主操作（**禁止只有一行灰字**）
//!
//! 这条不是风格偏好。空态是用户**第一次**打开应用看到的东西，也是"书架被
//! 清空后"看到的唯一东西——一行灰字"暂无数据"在这里等于把用户丢在原地。
//! 故本组件把"下一步做什么"作为**签名的一部分**：主操作是必填参数，不是
//! 可选装饰。
//!
//! ## 为什么插画位是一个带边框的矩形而不是一张图
//!
//! 内嵌插画会推高二进制体积（铁律 2），而 MASTER 说的是"插画**位**"——它
//! 要求的是布局上留出位置（而不是一张具体的图）。用 token 色画的占位矩形
//! 既满足布局要求，又不引入资源。

use egui::{Ui, Vec2};

use crate::theme_bridge::{BridgeTheme, SurfaceEmphasis};

/// 插画位尺寸（逻辑点）。
///
/// MASTER 未给具体数值；这里取一个与空态整体比例协调的尺寸，并在注释里
/// 注明它是**本组件的取值**而非 token（免得日后被误当作设计系统常量）。
const ILLUSTRATION_SIZE: Vec2 = Vec2::new(160.0, 120.0);

/// 画空态。
///
/// 返回主操作的点击响应（`None` = 用户没点）。
///
/// ## 为什么把"次操作"做成可选参数
///
/// 空书架有两条路（导入本地书 / 导入书源），而"过滤无结果"只有一条（清空
/// 过滤词）。用一个必填的主操作 + 一个可选的次操作覆盖两者，比两套布局函数
/// 更不容易漂移。
pub fn draw(
    ui: &mut Ui,
    theme: &BridgeTheme,
    title: &str,
    hint: &str,
    primary_label: &str,
    secondary_label: Option<&str>,
) -> EmptyResponse {
    let mut response = EmptyResponse::default();

    ui.vertical_centered(|ui| {
        ui.add_space(theme.theme().spacing().sp_10 as f32);

        // 插画位
        let (rect, _) = ui.allocate_exact_size(ILLUSTRATION_SIZE, egui::Sense::hover());
        let palette = theme.theme().palette();
        ui.painter().rect_stroke(
            rect,
            egui::CornerRadius::same(theme.theme().radii().lg_px as u8),
            egui::Stroke::new(1.0, theme.color(palette.border_subtle)),
            egui::StrokeKind::Inside,
        );

        ui.add_space(theme.theme().spacing().sp_4 as f32);
        ui.label(
            egui::RichText::new(title)
                .size(theme.theme().type_scale().display.size_px)
                .color(theme.text_color_for(SurfaceEmphasis::Normal)),
        );
        ui.add_space(theme.theme().spacing().sp_2 as f32);
        ui.label(
            egui::RichText::new(hint)
                .size(theme.theme().type_scale().body.size_px)
                .color(theme.text_color_for(SurfaceEmphasis::Required)),
        );
        ui.add_space(theme.theme().spacing().sp_4 as f32);

        ui.horizontal(|ui| {
            // 居中一组按钮：`vertical_centered` 之下 `horizontal` 会左对齐，
            // 故先按内容宽度留白（这是 egui 里做"居中按钮组"的常规做法）
            let buttons = if secondary_label.is_some() { 2 } else { 1 };
            let estimated = buttons as f32 * 96.0;
            let available = ui.available_width();
            if available > estimated {
                ui.add_space((available - estimated) / 2.0);
            }
            if ui.button(primary_label).clicked() {
                response.primary_clicked = true;
            }
            if let Some(label) = secondary_label {
                if ui.button(label).clicked() {
                    response.secondary_clicked = true;
                }
            }
        });
    });

    response
}

/// 空态的交互结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EmptyResponse {
    /// 主操作被点击
    pub primary_clicked: bool,
    /// 次操作被点击（无次操作时恒为 `false`）
    pub secondary_clicked: bool,
}
