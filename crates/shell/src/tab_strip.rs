//! 标签条：图标 + 标题 + 关闭按钮，最右侧常驻「+」新建按钮。
//!
//! 待补：标签过多时的横向滚动与左右箭头（当前为硬截断）。
//!
//! 关于两次点击的处理顺序：点击关闭按钮时，父级标签的点击处理器**也会**被触发。
//! 因此关闭处理器里显式调用 `stop_propagation`，并且所有定位都走 `TabId` 而不是下标——
//! 这样即便处理器顺序变化，也不会出现「关掉标签后又选中了别的标签」。

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::shell::ShellView;

/// 标签条高度（不含选中指示条）。
const TAB_HEIGHT: f32 = 34.0;

/// 选中标签底部的强调色指示条高度。
const INDICATOR_HEIGHT: f32 = 2.0;

impl ShellView {
    /// 渲染标签条。
    pub(crate) fn render_tab_strip(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let accent = theme.accent;
        let transparent = theme.transparent;
        let muted_foreground = theme.muted_foreground;
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
                .p_1()
                .cursor_pointer()
                .text_color(muted_foreground)
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    // 阻止冒泡到父级标签：否则会在标签已被关闭之后又对它做一次选中。
                    cx.stop_propagation();
                    this.close_tab(id, cx);
                }))
                .child(Icon::new(IconName::X).small());

            div()
                .id(("tab", id.0 as usize))
                .flex()
                .flex_col()
                .flex_none()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.select_tab(id, cx);
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .h(px(TAB_HEIGHT))
                        .child(Icon::new(icon).small())
                        .child(div().text_sm().child(title))
                        .child(close),
                )
                .child(
                    // 选中指示条用独立元素而不是 2px 边框：边框会改变布局尺寸，
                    // 导致选中/未选中之间标签宽度抖动。
                    div()
                        .h(px(INDICATOR_HEIGHT))
                        .w_full()
                        .bg(if is_active { accent } else { transparent }),
                )
                .into_any_element()
        });

        let new_tab = div()
            .id("tab-new")
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .px_3()
            .h(px(TAB_HEIGHT))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.open_tab(cx);
            }))
            .child(Icon::new(IconName::Plus).small())
            .into_any_element();

        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .children(tabs),
            )
            .child(new_tab)
            .into_any_element()
    }
}
