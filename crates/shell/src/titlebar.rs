//! 标签条右端的主题与窗口动作组。
//!
//! 窗口使用透明自绘标题栏，最小化、最大化和关闭仍通过 `WindowControlArea`
//! 交给系统处理；这里只负责 Fluent 外观与窄屏可见性。

use fm_views::{metrics, widgets};
use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, WindowControlArea, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::shell::ShellView;

impl ShellView {
    pub(crate) fn render_window_actions(
        &self,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &self.tokens;
        let theme_toggle = widgets::icon_button(
            "tabstrip-theme",
            if self.config.dark {
                IconName::Sun
            } else {
                IconName::Moon
            },
            18.0,
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

        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .pl(px(8.0))
            .pr(px(4.0))
            .when(!compact, |element| {
                element
                    .child(theme_toggle)
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
            })
            .child(window_control(
                "window-close",
                IconName::X,
                WindowControlArea::Close,
                true,
                tokens,
            ))
            .into_any_element()
    }
}

fn window_control(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    danger: bool,
    tokens: &fm_views::Tokens,
) -> AnyElement {
    let hover = tokens.hover;
    let danger_hover = tokens.danger;
    let foreground = tokens.fg;
    let on_accent = tokens.on_accent;

    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(metrics::CONTROL_SIZE))
        .h(px(metrics::CONTROL_SIZE))
        .rounded(px(metrics::RADIUS))
        .text_color(tokens.secondary_fg)
        .hover(move |style| {
            if danger {
                style.bg(danger_hover).text_color(on_accent)
            } else {
                style.bg(hover).text_color(foreground)
            }
        })
        .window_control_area(area)
        .child(Icon::new(icon).with_size(px(16.0)))
        .into_any_element()
}
