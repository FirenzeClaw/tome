//! `[WIP]` 阅读视图（S3 T014）
//!
//! ## 契约（`contracts/ui.md` §1.2）
//!
//! | # | 状态 | 本模块的落点 |
//! |---|---|---|
//! | R1 | 加载中 | 骨架 + 可取消（返回书架）；不白屏（FR-010） |
//! | R2 | 阅读中 | 版心（PaintCallback）+ 页脚（章标题 + 进度） |
//! | R3 | 加载失败 | 结构化错误（标识 + 文案）+ 三个动作 |
//! | R4 | 章间边界 | FR-006b 由引擎处理（本视图只呈现结果） |
//! | R5/R6 | 抽屉 | US2（T019/T020） |
//!
//! ## 版心是怎么画上去的（路径 A 的全貌）
//!
//! ```text
//!   引擎线程                        UI 线程（本模块）
//!   ────────                        ────────────────
//!   Paginator::paginate
//!        ↓ ChapterLayout（当前页的行盒）
//!   ChapterReady{laid_out, version}
//!        ↓ ──────────────────────►  TomeApp 留存 laid_out
//!                                       ↓
//!                                  PageFrameCache::frame(version, ...)
//!                                       ↓ RenderBatch（缓存命中则复用）
//!                                  draw_reading_area(ui, batch)
//!                                       ↓ egui_wgpu::Callback
//!                                  GlyphCallback::prepare / paint
//! ```
//!
//! 关键的一点：**切页发生在引擎线程**（见 `engine/worker.rs` 的说明），本模块
//! 拿到的 `laid_out.lines` 就是"这一页该画的行"。UI 侧不做任何分页决策。
//!
//! ## feature 门控
//!
//! 渲染面（`RenderBatch` / `PaintCallback`）只在 `reader-render` 下存在。
//! headless 构建里 `Stage` 是 `None`，本视图退化为"内容就绪但无法呈现"的
//! 结构化能力错误——那正是 002 §FR-053 的"诚实失败"形态，而不是白屏或崩溃。

use egui::Ui;

use crate::app::{ErrorAction, ViewState};
use crate::engine::messages::ContentState;
use crate::engine::messages::EngineRequest;
use crate::i18n;
use crate::theme_bridge::{BridgeTheme, SurfaceEmphasis};

/// 页脚高度（逻辑点）。
///
/// MASTER 未给阅读页脚的数值；这是**本模块的取值**（故硬编码在此并注明，
/// 不当成设计系统常量使用）。
const FOOTER_HEIGHT_PX: f32 = 32.0;

/// 渲染面上下文（仅在 `reader-render` 下存在）。
///
/// ## 为什么参数是一个包而不是三个
///
/// 三者必须**同时**可用才有意义（有排版结果、有缓存、有主题），缺一就画不出
/// 版心。合成一个类型让"三个都在"成为类型层面的事实，而不是调用点的约定。
#[cfg(feature = "reader-render")]
pub struct Stage<'a> {
    /// 引擎给来的排版结果（`None` = 还没排过版，此时应显示加载态）
    pub laid_out: Option<&'a tome_reader::paginate::page::ChapterLayout>,
    /// 版心帧缓存（`content_version` 未变时复用）
    pub frames: &'a mut crate::render::texture::PageFrameCache,
    /// 当前主题
    pub theme: BridgeTheme,
}

/// headless 形态下的 `Stage`：**空类型**（`Option<Stage>` 恒为 `None`）。
///
/// ## 为什么保留这个参数而不是把它 cfg 掉
///
/// 若把参数整条 cfg 掉，`draw` 就有了两个不同的签名（headless 两参、
/// 带渲染三参），而**调用点**（`app.rs` 的 `ui()`）也就得跟着分叉。分叉的
/// 代价是"改了签名只改了一边"这类错误会静默通过一次构建（另一边红）。
///
/// 用空类型让签名**恒定**：headless 下 `Option<Stage<'_>>` 恒为 `None`，
/// 视图按"没有渲染面"分支走（结构化能力错误），逻辑与带渲染时一致。
#[cfg(not(feature = "reader-render"))]
pub type Stage<'a> = std::marker::PhantomData<&'a ()>;

/// 画阅读视图，返回本帧产生的引擎请求。
pub fn draw(ui: &mut Ui, state: &mut ViewState, stage: Option<Stage<'_>>) -> Vec<EngineRequest> {
    let mut requests = Vec::new();
    let theme = BridgeTheme::from_theme(state.theme());
    let content = state.content().clone();

    match &content {
        ContentState::Loading => {
            draw_loading(ui, &theme, state, &mut requests);
        }
        ContentState::Ready { content_version } => {
            #[cfg(feature = "reader-render")]
            draw_ready(ui, &theme, state, stage, *content_version, &mut requests);
            #[cfg(not(feature = "reader-render"))]
            {
                // headless：`stage` 恒为 `None`，而"内容就绪却无渲染面"是
                // 诚实的能力错误（002 §FR-053），不是白屏
                let _ = (stage, content_version);
                draw_headless_capability_error(ui, &theme, state, &mut requests);
            }
        }
        ContentState::Failed { error } => {
            draw_failed(ui, &theme, state, error, &mut requests);
        }
    }

    requests
}

/// R1：加载中（**不白屏**，且可取消）。
///
/// ## 为什么加载态也要报告视口（一个必须写清楚的时序）
///
/// 引擎**不知道阅读区多大时无法排版**（版心高度决定一页装几行），而 UI 在
/// 内容就绪前处于加载态。若加载态不报告视口，两者就互相等待：引擎等尺寸、
/// UI 等内容——界面永远停在"正在加载"。
///
/// 故报告视口这件事必须发生在**每一条**阅读态分支上（加载/就绪/失败），
/// 而不是只挂在就绪分支。这是本视图最容易写错的一处。
fn draw_loading(
    ui: &mut Ui,
    theme: &BridgeTheme,
    state: &mut ViewState,
    requests: &mut Vec<EngineRequest>,
) {
    requests.push(viewport_request(ui));
    ui.vertical_centered(|ui| {
        ui.add_space(theme.theme().spacing().sp_10 as f32);
        crate::widgets::skeleton::block(ui, theme, egui::vec2(320.0, 20.0), 0.0, false);
        ui.add_space(theme.theme().spacing().sp_2 as f32);
        crate::widgets::skeleton::block(ui, theme, egui::vec2(280.0, 20.0), 0.5, false);
        ui.add_space(theme.theme().spacing().sp_4 as f32);
        ui.label(
            egui::RichText::new(i18n::t(i18n::key::READING_LOADING))
                .color(theme.text_color_for(SurfaceEmphasis::Required)),
        );
        if ui
            .button(i18n::t(i18n::key::READING_LOADING_CANCEL))
            .clicked()
        {
            // 取消 = 返回书架（spec 边缘情况："加载期间可取消返回书架"）
            if let Some(flush) = state.back_to_shelf_request() {
                requests.push(flush);
            }
            requests.push(EngineRequest::ReadingClose);
        }
    });
}

/// R2：就绪（版心 + 页脚）。
#[cfg(feature = "reader-render")]
fn draw_ready(
    ui: &mut Ui,
    theme: &BridgeTheme,
    state: &mut ViewState,
    stage: Option<Stage<'_>>,
    content_version: u64,
    requests: &mut Vec<EngineRequest>,
) {
    use crate::render::callback::draw_reading_area;

    let Some(stage) = stage else {
        draw_headless_capability_error(ui, theme, state, requests);
        return;
    };

    // 阅读区 = 可用区 - 页脚
    let available = ui.available_size();
    let reading_height = (available.y - FOOTER_HEIGHT_PX).max(0.0);

    // 把**净区**尺寸告知引擎（它据此排版）。每帧发一次是廉价的：引擎侧对
    // 未变化的尺寸去重（见 `worker.rs` 的 `ReadingViewport` 分支）。
    let request = viewport_request(ui);
    let (physical_width, physical_height) = match &request {
        EngineRequest::ReadingViewport {
            width_px,
            height_px,
        } => (*width_px, *height_px),
        _ => unreachable!("viewport_request 只产 ReadingViewport"),
    };
    requests.push(request);

    let geometry = geometry_of(state);
    let viewport = tome_reader::paginate::spread::Viewport::new(physical_width, physical_height);

    // 版心：`StageResources` 未注册时不画（回调里有可见的日志报告），
    // 但**仍然显示页脚**——否则用户看到的是"一片空白 + 没有页脚"，
    // 无法区分"渲染失败"与"没有内容"。
    if let Some(laid_out) = stage.laid_out {
        let batch = stage.frames.frame(
            content_version,
            laid_out,
            &geometry,
            &viewport,
            theme.theme(),
        );
        // 响应值此处不用（它是 hover 响应，命中测试属 002 FR-040）
        let _ = draw_reading_area(ui, batch);
    } else {
        ui.allocate_space(egui::vec2(available.x, reading_height));
    }

    draw_footer(ui, theme, state);
}

/// 页脚：章标题 + 进度（FR-006 要求"显示当前章标题与阅读进度"）。
///
/// headless 构建下没有版心可画，但页脚**仍然显示**——否则用户看到的是
/// "一片空白且没有页脚"，无法区分"渲染面不存在"与"这本书没有内容"。
#[cfg_attr(not(feature = "reader-render"), allow(dead_code))]
fn draw_footer(ui: &mut Ui, theme: &BridgeTheme, state: &ViewState) {
    ui.separator();
    ui.horizontal(|ui| {
        let chapter_title = state
            .toc()
            .get(state.current_chapter().max(0) as usize)
            .cloned()
            .unwrap_or_else(|| i18n::chapter_label(state.current_chapter()));
        ui.label(
            egui::RichText::new(chapter_title)
                .color(theme.text_color_for(SurfaceEmphasis::Required)),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let percent = if state.chapter_char_len() == 0 {
                0.0
            } else {
                (state.anchor().char_offset.max(0) as f32 / state.chapter_char_len() as f32) * 100.0
            };
            ui.label(
                egui::RichText::new(i18n::scroll_position(percent))
                    .color(theme.text_color_for(SurfaceEmphasis::Required)),
            );
        });
    });
}

/// R3：失败（结构化错误 + 可行动作）。
fn draw_failed(
    ui: &mut Ui,
    theme: &BridgeTheme,
    state: &mut ViewState,
    error: &crate::engine::messages::ClassifiedErrorView,
    requests: &mut Vec<EngineRequest>,
) {
    ui.vertical_centered(|ui| {
        ui.add_space(theme.theme().spacing().sp_10 as f32);
        ui.label(
            egui::RichText::new(i18n::t(i18n::key::READING_FAILED_TITLE))
                .size(theme.theme().type_scale().h2.size_px)
                .color(theme.text_color_for(SurfaceEmphasis::Normal)),
        );
        // 标识**永不翻译**且随文案一并呈现（`contracts/ui.md` §3.2）
        ui.label(
            egui::RichText::new(i18n::error_line(&error.id))
                .size(theme.theme().type_scale().sm.size_px)
                .color(theme.text_color_for(SurfaceEmphasis::Required)),
        );
        if let Some(capability) = &error.capability {
            ui.label(
                egui::RichText::new(i18n::capability_badge(capability))
                    .size(theme.theme().type_scale().caption.size_px)
                    .color(theme.text_color_for(SurfaceEmphasis::Decorative)),
            );
        }
        if let Some(detail) = &error.detail {
            ui.label(
                egui::RichText::new(detail)
                    .size(theme.theme().type_scale().caption.size_px)
                    .color(theme.text_color_for(SurfaceEmphasis::Decorative)),
            );
        }

        ui.add_space(theme.theme().spacing().sp_4 as f32);
        ui.horizontal(|ui| {
            if ui.button(i18n::t(i18n::key::READING_RETRY)).clicked() {
                state.dismiss_error();
                requests.push(EngineRequest::ReadingLoadChapter {
                    book_id: state.book_id().to_owned(),
                    chapter_index: state.current_chapter(),
                });
            }
            if ui
                .button(i18n::t(i18n::key::READING_SWITCH_SOURCE))
                .clicked()
            {
                // 换源面板属 US3（T027）。此处**不伪造成功**：动作存在、
                // 但如实告知该能力尚未落地（`error.unsupported` 由引擎给出）。
                requests.push(EngineRequest::SourceSwitch {
                    book_id: state.book_id().to_owned(),
                    new_source_id: String::new(),
                });
            }
            if ui.button(i18n::t(i18n::key::READING_BACK)).clicked() {
                if let Some(flush) = state.back_to_shelf_request() {
                    requests.push(flush);
                }
                requests.push(EngineRequest::ReadingClose);
            }
        });
    });
}

/// headless（`reader-render` 关闭）时的诚实失败。
///
/// ## 为什么这里是一个"错误"而不是"骨架"
///
/// 002 §FR-053 的语义是：**做不到的事要说出来**。渲染面在编译期不存在时，
/// 界面若显示"正在加载…"就是撒谎——内容永远不会到达。故按能力错误呈现，
/// 并给出唯一的可行动作（返回书架）。
fn draw_headless_capability_error(
    ui: &mut Ui,
    theme: &BridgeTheme,
    state: &mut ViewState,
    requests: &mut Vec<EngineRequest>,
) {
    let error = crate::engine::messages::ClassifiedErrorView {
        id: tome_core::ErrorKind::unsupported_capability(tome_core::ErrorCapability::new(
            "offscreenRendering",
        ))
        .message_id()
        .to_owned(),
        detail: None,
        capability: Some("offscreenRendering".to_owned()),
    };
    draw_failed(ui, theme, state, &error, requests);
}

/// 构造"告知引擎阅读区尺寸"的请求。
///
/// ## 为什么阅读区高度要扣掉页脚
///
/// 版心高度决定一页装几行，而页脚是**界面**占的位置、不是版心的一部分。
/// 不扣的话每页会多排一行，那一行会被页脚盖住——表现为"每页最后一行看不全"。
///
/// 宽度不扣：阅读区横向没有其它常驻元素（抽屉是覆盖式的，FR-008 明令它
/// MUST NOT 改变版心几何）。
#[must_use]
pub fn viewport_request(ui: &Ui) -> EngineRequest {
    let available = ui.available_size();
    let reading_height = (available.y - FOOTER_HEIGHT_PX).max(0.0);
    let pixels_per_point = ui.ctx().pixels_per_point();
    EngineRequest::ReadingViewport {
        width_px: available.x * pixels_per_point,
        height_px: reading_height * pixels_per_point,
    }
}

/// 由阅读参数与窗口几何构造排版几何。
///
/// ## 为什么与 `engine/worker.rs` 的那份看起来重复
///
/// 两份的**输入不同**：引擎侧只知道 UI 告知的净区尺寸（它是权威），而本侧
/// 的 `ReadingGeometry` 还带着"窗口宽"这一 UI 专有的量（单双页阈值用它）。
/// 合并成一个函数就得让其中一侧拿到它不需要的东西。
///
/// 真正的权威是**引擎侧**那一份：UI 这份只用于页脚的进度换算与视图分支。
#[cfg(feature = "reader-render")]
fn geometry_of(state: &ViewState) -> tome_reader::paginate::config::GeometryConfig {
    use crate::engine::messages::PageLayout;
    use tome_reader::paginate::config::{GeometryConfig, ReadingMode, SpreadMode};

    let params = state.params();
    GeometryConfig {
        font_size_px: params.font_size_px.round().clamp(1.0, f32::from(u16::MAX)) as u16,
        line_height: params.line_height,
        spread_mode: match params.page_layout {
            PageLayout::Auto => SpreadMode::Auto,
            PageLayout::Single => SpreadMode::ForceSingle,
            PageLayout::Spread => SpreadMode::ForceSpread,
        },
        // 滚动形态的呈现属 US2（T018/T021）；本切片只走翻页
        reading_mode: ReadingMode::Paged,
        ..GeometryConfig::default()
    }
}

/// 错误动作的展示标签（供断言"每个动作都有文案"）。
#[must_use]
pub fn action_label(action: ErrorAction) -> &'static str {
    match action {
        ErrorAction::Retry => i18n::t(i18n::key::READING_RETRY),
        ErrorAction::SwitchSource => i18n::t(i18n::key::READING_SWITCH_SOURCE),
        ErrorAction::Back => i18n::t(i18n::key::READING_BACK),
    }
}
