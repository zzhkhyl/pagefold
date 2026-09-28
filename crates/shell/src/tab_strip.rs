//! 标签条：标签（图标 + 标题 + 关闭按钮）+ 紧随其后的「+」新建按钮，
//! 右端是窗口动作组（主题切换 + 最小化 / 最大化 / 关闭）。
//!
//! 这一栏现在也是**窗口顶部唯一的栏**：品牌栏整条删掉之后，
//! 窗口的拖动区域落在标签与动作组之间的空白段上（见 [`render_tab_strip`] 末尾的说明）。
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
    SharedString, StatefulInteractiveElement as _, Styled as _, WindowControlArea, div, px,
    prelude::FluentBuilder as _,
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

        // 收集成 `Vec` 而不是留着惰性迭代器：下面的 `render_window_actions` 需要
        // 可变借用 `cx`，而迭代器里的 `cx.listener(..)` 会一直持有 `cx` 的不可变借用。
        let tabs: Vec<AnyElement> = self.tabs.tabs().iter().map(|tab| {
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
        }).collect();

        // 右端动作组：主题切换 + 三个窗口按钮。它们原先在标题栏（第一栏），
        // 随第一栏精简为「只剩品牌标志」一起下移到本栏——这也是本次唯一一处布局性调整。
        let window_actions = self.render_window_actions(cx);

        div()
            .relative()
            .flex_none()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(8.0))
            .h(px(metrics::TABSTRIP_HEIGHT))
            // 只留左边距：右端是关闭按钮，要贴到窗口右边缘（Windows 惯例），
            // 所以这一栏不再有右内边距。
            .pl(px(8.0))
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
                // 标签区：标签 + 紧跟其后的「＋」。这一组**可压缩**（`flex_shrink`）
                // 而不是占满整栏——剩下的宽度留给右边的拖拽段，
                // 「＋」才会紧挨着最后一个标签，而不是被推到最右端。
                //
                // 顶部内衬挂在这里而不是整栏上：窗口按钮要占满 38px 整栏高度，
                // 整栏带内衬会把它们压缩 6px、向上溢出。
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(6.0))
                    .flex_shrink(1.0)
                    .min_w(px(0.0))
                    .pt(px(metrics::TABSTRIP_PADDING_TOP))
                    .child(
                        div()
                            .id("tab-list")
                            .flex()
                            .flex_row()
                            .items_end()
                            .gap(px(2.0))
                            .flex_shrink(1.0)
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
                    )),
            )
            // 拖拽段：品牌栏删掉之后，这是窗口**唯一**的拖动区域。
            // 只能挂在这一段空白上——挂在整栏会抢在标签与窗口按钮之前命中
            // （命中区按注册顺序匹配，父元素先于子元素，原因见 `titlebar` 的说明）。
            //
            // 留 32px 最小宽度：标签多到铺满时其余项都会被压到底，
            // 这一段不能再被挤没，否则整扇窗口就抓不动了。
            .child(
                div()
                    .flex_grow(1.0)
                    .min_w(px(32.0))
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(window_actions)
            .into_any_element()
    }
}
