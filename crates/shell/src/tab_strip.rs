//! 标签条：图标 + 标题 + 关闭按钮，最右侧常驻「+」新建按钮。
//!
//! 形态照搬设计稿的 `.tab`：32px 高、顶部 8px 圆角、激活标签用 surface 底色
//! 与 2px 强调色顶边「连」到下方的命令条，未激活标签是次级文字色。
//!
//! 关于两次点击的处理顺序：点击关闭按钮时，父级标签的点击处理器**也会**被触发。
//! 因此关闭处理器里显式调用 `stop_propagation`，并且所有定位都走 `TabId` 而不是下标——
//! 这样即便处理器顺序变化，也不会出现「关掉标签后又选中了别的标签」。
//!
//! 待补：标签过多时的横向滚动与左右箭头（当前是压缩标签宽度直到最小宽度）。

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px, prelude::FluentBuilder as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use fm_views::metrics;

use crate::shell::ShellView;

impl ShellView {
    /// 渲染标签条。
    pub(crate) fn render_tab_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        // 悬停样式会被存进元素里，因此先把用到的色值拷出来（`Tokens` 不是 `Copy`）。
        let hover_fill = tokens.hover;
        let foreground = tokens.fg;
        let active_id = self.tabs.active().id();

        let tabs = self.tabs.tabs().iter().map(|tab| {
            let id = tab.id();
            let is_active = id == active_id;
            let title: SharedString = tab.state().title().into();
            let icon = if is_active {
                IconName::FolderOpen
            } else {
                IconName::FolderClosed
            };

            let close = div()
                .id(("tab-close", id.0 as usize))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(18.0))
                .h(px(18.0))
                .mr(px(4.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .opacity(if is_active { 1.0 } else { 0.55 })
                .text_color(tokens.muted_fg)
                .hover(move |style| style.bg(hover_fill).text_color(foreground))
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    // 阻止冒泡到父级标签：否则会在标签已被关闭之后又对它做一次选中。
                    cx.stop_propagation();
                    this.close_tab(id, cx);
                }))
                .child(Icon::new(IconName::X).with_size(px(11.0)));

            div()
                .id(("tab", id.0 as usize))
                .flex()
                .flex_col()
                .flex_shrink(1.0)
                .min_w(px(96.0))
                .max_w(px(metrics::TAB_MAX_WIDTH))
                .h(px(metrics::TAB_HEIGHT))
                .rounded_t(px(8.0))
                .overflow_hidden()
                .cursor_pointer()
                .text_color(if is_active { tokens.fg } else { tokens.muted_fg })
                .border_l_2()
                .border_r_2()
                .border_color(if is_active {
                    tokens.border
                } else {
                    tokens.surface
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.select_tab(id, cx);
                }))
                .when(!is_active, |element| {
                    element.hover(move |style| style.bg(hover_fill).text_color(foreground))
                })
                .when(is_active, |element| element.bg(tokens.surface))
                // 激活标签的顶边画成强调色；未激活时同高透明，
                // 保证两种状态的内容高度完全一致，文字不会上下跳。
                .child(
                    div()
                        .flex_none()
                        .w_full()
                        .h(px(2.0))
                        .bg(if is_active {
                            tokens.accent
                        } else {
                            tokens.surface
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .flex_grow(1.0)
                        .min_w(px(0.0))
                        .child(
                            div()
                                .flex_grow(1.0)
                                .min_w(px(0.0))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .pl(px(10.0))
                                .overflow_hidden()
                                .child(
                                    Icon::new(icon)
                                        .with_size(px(14.0))
                                        .text_color(if is_active {
                                            tokens.accent
                                        } else {
                                            tokens.muted_fg
                                        }),
                                )
                                .child(
                                    div()
                                        .flex_grow(1.0)
                                        .min_w(px(0.0))
                                        .overflow_hidden()
                                        .truncate()
                                        .text_size(px(12.5))
                                        .child(title),
                                ),
                        )
                        .child(close),
                )
                .into_any_element()
        });

        div()
            .relative()
            .flex_none()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(8.0))
            .h(px(metrics::TABSTRIP_HEIGHT))
            .px(px(8.0))
            .pt(px(metrics::TABSTRIP_PADDING_TOP))
            .bg(tokens.panel)
            // 底部分隔线做成「绝对定位的第一个子元素」，而不是父级的 border：
            // 设计稿把它画在 `.tabstrip::after`（z-index 0）上，激活标签（z-index 2）
            // 会盖住它，从而与下方的命令条连成一片。若写成 border，
            // 描边落在标签盒子之外，标签再高的层级也盖不住这条线。
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.0))
                    .bg(tokens.border),
            )
            .child(
                div()
                    .id("tab-list")
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(2.0))
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .h(px(metrics::TAB_HEIGHT))
                    .overflow_hidden()
                    .children(tabs),
            )
            .child(fm_views::widgets::plain_icon_button(
                "tab-new",
                IconName::Plus,
                16.0,
                "新建标签页",
                tokens,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.open_tab(cx);
                }),
            ))
            .into_any_element()
    }
}
