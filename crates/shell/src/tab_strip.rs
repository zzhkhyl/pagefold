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
    AnyElement, App, ClickEvent, Context, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, WindowControlArea, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::button::{
    Button, ButtonCustomVariant, ButtonVariant, ButtonVariants as _, DropdownButton,
};
use gpui_component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use fm_core::TabId;
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
                // 中键关闭标签（与浏览器一致）。阻止冒泡，否则会顺带触发左键点击的选中。
                .on_mouse_down(MouseButton::Middle, cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.close_tab(id, cx);
                }))
                // 拖拽排序：左键按下记起始，滑过其它标签时把被拖的标签移到它的位置。
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, _cx| {
                    this.begin_tab_drag(id, event.position);
                }))
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    this.drag_tab_over(id, event.position, cx);
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
                // 右键菜单：与「更多」按钮同一份，但「关闭其它/右侧」以本标签为参照。
                .context_menu(tab_menu_builder(cx, Some(id)))
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
                    ))
                    // 「更多」：标签页操作菜单（新建 / 重新打开 / 关闭其他 / 关闭右侧）。
                    .child(tab_more_button(tokens, cx)),
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
            // 左键抬起（含在标签条外抬起）结束拖拽排序。
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _window, _cx| {
                this.end_tab_drag();
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _window, _cx| {
                this.end_tab_drag();
            }))
            .into_any_element()
    }
}

/// 标签页操作菜单里的动作。
#[derive(Clone, Copy)]
enum TabCommand {
    New,
    ReopenClosed,
    /// 关闭其它标签页。`Some(id)` 以该标签为参照（右键菜单），`None` 用当前标签（「更多」按钮）。
    CloseOthers(Option<TabId>),
    /// 关闭右侧标签页。参照同 [`TabCommand::CloseOthers`]。
    CloseRight(Option<TabId>),
}

/// 标签条「更多」按钮（设计稿的 `#tabMore`）。
///
/// 用 `DropdownButton` 承载弹出层，与命令条「新建」复用同一套定位 / Esc 关闭 /
/// 上下键导航机制。触发按钮配色走 `ButtonCustomVariant`，与旁边手绘的「＋」按钮
/// 用同一套设计令牌，视觉上是一组。
fn tab_more_button(tokens: &fm_views::Tokens, cx: &mut Context<ShellView>) -> impl IntoElement {
    DropdownButton::new("tab-more")
        .button(
            Button::new("tab-more-trigger")
                .with_variant(ButtonVariant::Custom(
                    ButtonCustomVariant::new(cx)
                        .color(gpui::transparent_black())
                        .foreground(tokens.muted_fg)
                        .hover(tokens.hover)
                        .active(tokens.accent_soft),
                ))
                .icon(Icon::new(IconName::Ellipsis).with_size(px(16.0)))
                .tooltip("标签页操作")
                // 与「＋」按钮同高，视觉上并排一组。
                .w(px(30.0))
                .h(px(32.0)),
        )
        .dropdown_menu(tab_menu_builder(cx, None))
}

/// 构造标签页操作菜单的内容。
fn tab_menu_builder(
    cx: &Context<ShellView>,
    target: Option<TabId>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let shell = cx.entity().downgrade();
    move |menu, _window, _cx| {
        let mut menu = menu;
        menu = menu.item(
            PopupMenuItem::new("新建标签页")
                .icon(Icon::new(IconName::Plus))
                // 设 action 让组件库在右侧渲染绑定的键位（Ctrl+T）。
                .action(Box::new(crate::actions::NewTab))
                .on_click(tab_command_handler(&shell, TabCommand::New)),
        );
        menu = menu.item(
            PopupMenuItem::new("重新打开关闭的标签页")
                .icon(Icon::new(IconName::RotateCcw))
                .action(Box::new(crate::actions::ReopenClosedTab))
                .on_click(tab_command_handler(&shell, TabCommand::ReopenClosed)),
        );
        menu = menu.separator();
        menu = menu.item(
            PopupMenuItem::new("关闭其他标签页")
                .icon(Icon::new(IconName::X))
                .on_click(tab_command_handler(&shell, TabCommand::CloseOthers(target))),
        );
        menu = menu.item(
            PopupMenuItem::new("关闭右侧标签页")
                .icon(Icon::new(IconName::X))
                .on_click(tab_command_handler(&shell, TabCommand::CloseRight(target))),
        );
        menu
    }
}

/// 标签页操作菜单项的回调。
fn tab_command_handler(
    shell: &WeakEntity<ShellView>,
    command: TabCommand,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let shell = shell.clone();
    move |_event: &ClickEvent, _window: &mut Window, cx: &mut App| {
        let applied = shell.update(cx, |view, cx| match command {
            TabCommand::New => view.open_tab(cx),
            TabCommand::ReopenClosed => view.reopen_closed_tab(cx),
            TabCommand::CloseOthers(target) => view.close_others(target, cx),
            TabCommand::CloseRight(target) => view.close_right(target, cx),
        });
        if applied.is_err() {
            tracing::debug!("执行标签页操作时视图已销毁");
        }
    }
}
