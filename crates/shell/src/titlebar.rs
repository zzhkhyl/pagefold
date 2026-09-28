//! 窗口动作组：主题切换 + 最小化 / 最大化 / 关闭。
//!
//! 窗口以 `appears_transparent: true` 创建（见 `fm-app` 的 `bootstrap`），
//! **这三个按钮必须由自己提供**——它们是自绘窗口边框的配套义务。
//! 具体动作交给平台（`WindowControlArea`），不自己实现窗口缩放逻辑。
//!
//! 挂载位置在**标签条那一栏**的右端：原来它们属于独立的第一栏（品牌栏），
//! 那一栏已按反馈整条删除，动作组随之下移到本栏。
//! 因此本模块现在只提供这一组动作，不再负责任何整栏的布局。

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, WindowControlArea, div, px,
};
use fm_views::widgets;
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::shell::ShellView;

/// 窗口控制按钮的宽度。46px 是 Windows 11 系统按钮的标准宽度。
const WINDOW_CONTROL_WIDTH: f32 = 46.0;

impl ShellView {
    /// 渲染窗口动作组。
    ///
    /// 调用方负责把它放在标签条那一栏的右端，并保证那一栏**没有右内边距**：
    /// 关闭按钮要贴到窗口右边缘，这是 Windows 的惯例。
    pub(crate) fn render_window_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;

        let theme_toggle = widgets::icon_button(
            "tabstrip-theme",
            if self.config.dark {
                IconName::Sun
            } else {
                IconName::Moon
            },
            16.0,
            if self.config.dark {
                "切换到浅色主题"
            } else {
                "切换到深色主题"
            },
            false,
            true,
            tokens,
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.toggle_theme(window, cx);
            }),
        );

        let controls = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .child(window_control(
                "window-min",
                IconName::Minus,
                WindowControlArea::Min,
                false,
                tokens,
            ))
            .child(window_control(
                "window-max",
                IconName::Square,
                WindowControlArea::Max,
                false,
                tokens,
            ))
            .child(window_control(
                "window-close",
                IconName::X,
                WindowControlArea::Close,
                true,
                tokens,
            ));

        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .child(theme_toggle)
            .child(separator(tokens))
            .child(controls)
            .into_any_element()
    }
}

/// 动作组里的竖分隔线。
fn separator(tokens: &fm_views::Tokens) -> AnyElement {
    div()
        .flex_none()
        .w(px(1.0))
        .h(px(16.0))
        .mx(px(6.0))
        .bg(tokens.border)
        .into_any_element()
}

/// 窗口控制按钮。
///
/// 动作全部交给平台：`WindowControlArea` 会把这些命中区直接报告给系统，
/// 由系统完成最小化 / 最大化 / 关闭。自己实现那套逻辑会在多显示器、贴靠布局、
/// 系统菜单等边界上不断出问题，而平台实现天然是对的。
///
/// 高度取 `TABSTRIP_HEIGHT`：它挂载在标签条那一栏，取同一常量才能正好占满整栏高度
/// （品牌栏删除前用的是 `TITLEBAR_HEIGHT`，两个常量数值相同，但语义上现在归标签条）。
fn window_control(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    danger: bool,
    tokens: &fm_views::Tokens,
) -> AnyElement {
    let foreground = tokens.fg;
    let hover = tokens.hover;
    let danger_soft = tokens.danger_soft;
    let danger = danger.then_some(tokens.danger);

    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(WINDOW_CONTROL_WIDTH))
        .h(px(fm_views::metrics::TABSTRIP_HEIGHT))
        .text_color(tokens.muted_fg)
        .hover(move |style| {
            let style = style.text_color(foreground);
            // 关闭键悬停变红：与系统一致，避免误点。
            match danger {
                Some(danger) => style.bg(danger_soft).text_color(danger),
                None => style.bg(hover),
            }
        })
        .window_control_area(area)
        .child(Icon::new(icon).with_size(px(14.0)))
        .into_any_element()
}
