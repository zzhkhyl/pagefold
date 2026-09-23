//! 命令条：导航按钮 + 地址区 + 视图切换 + 主题切换。
//!
//! 地址区当前只显示完整路径文本。可编辑地址栏与逐级面包屑由双视图任务接入
//! （`fm-views::breadcrumb`），届时这里只需替换中间那一段。

use std::path::PathBuf;

use fm_core::{Location, ViewMode};
use fm_views::breadcrumb;
use gpui::{
    AnyElement, App, ClickEvent, Context, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::button::Button;
use gpui_component::input::Input;
use gpui_component::{ActiveTheme as _, Icon};
use gpui_kit_assets::IconName;

use crate::shell::{ShellView, palette};

/// 命令条高度。
const COMMAND_BAR_HEIGHT: f32 = 44.0;

impl ShellView {
    /// 渲染命令条。
    pub(crate) fn render_command_bar(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let muted_foreground = theme.muted_foreground;
        let accent = theme.accent;
        let accent_foreground = theme.accent_foreground;

        let tab = self.tabs.active();
        let can_back = tab.state().can_go_back();
        let can_forward = tab.state().can_go_forward();
        // 已在文件系统根时「上级」不可用，否则会原地打转。
        // 「此电脑」是虚拟位置，同样没有上级（`directory()` 返回 `None`）。
        let can_up = tab
            .state()
            .directory()
            .and_then(|dir| dir.parent())
            .is_some_and(|parent| !parent.as_os_str().is_empty());
        // 「此电脑」没有路径可拆成面包屑，单独处理。
        let location = tab.state().location().clone();
        let colors = palette(cx);
        let icons_view = tab.state().view_mode() == ViewMode::Icons;
        let dark = self.config.dark;

        let back = nav_button(
            "nav-back",
            IconName::ArrowLeft,
            "后退",
            can_back,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.go_back(cx);
            }),
        );
        let forward = nav_button(
            "nav-forward",
            IconName::ArrowRight,
            "前进",
            can_forward,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.go_forward(cx);
            }),
        );
        let up = nav_button(
            "nav-up",
            IconName::ArrowUp,
            "上级目录",
            can_up,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.go_up(cx);
            }),
        );
        let refresh = nav_button(
            "nav-refresh",
            IconName::RefreshCw,
            "刷新",
            true,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.reload(cx);
            }),
        );

        let address = if self.address_editing {
            let editor = match self.address_input.clone() {
                Some(state) => div()
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .child(Input::new(&state))
                    .into_any_element(),
                // 理论上进入编辑态就一定有输入状态；真出现缺失也不能渲染成半截界面。
                None => div().flex_grow(1.0).into_any_element(),
            };

            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_grow(1.0)
                .min_w(px(0.0))
                .gap_1()
                .child(editor)
                .child(Button::new("address-go").label("跳转").on_click(
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.commit_address_edit(cx);
                    }),
                ))
                .child(Button::new("address-cancel").label("取消").on_click(
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.cancel_address_edit(cx);
                    }),
                ))
                .into_any_element()
        } else {
            let address_body = match &location {
                // 虚拟位置没有层级可拆，直接给一个不可点击的静态标签。
                Location::Computer => div()
                    .px_2()
                    .text_color(colors.foreground)
                    .child("此电脑")
                    .into_any_element(),
                Location::Directory(path) => {
                    let crumbs = breadcrumb::fold(
                        &breadcrumb::segments(path),
                        breadcrumb::MAX_VISIBLE_SEGMENTS,
                    );
                    let navigate = {
                        let view = cx.entity().downgrade();
                        move |target: PathBuf, _window: &mut Window, cx: &mut App| {
                            view.update(cx, |view, cx| view.navigate_to(target, cx)).ok();
                        }
                    };
                    breadcrumb::render(&crumbs, colors, navigate)
                }
            };

            div()
                .id("address-bar")
                .flex()
                .flex_row()
                .items_center()
                .flex_grow(1.0)
                .min_w(px(0.0))
                .h(px(26.0))
                .border_1()
                .border_color(border)
                .rounded(px(6.0))
                .text_sm()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    // 点击空白处切换为可编辑地址栏；
                    // 点击具体层级时面包屑会自己阻止冒泡，不会被当成「点空白」。
                    this.begin_address_edit(window, cx);
                }))
                .child(address_body)
                .into_any_element()
        };

        let view_icons = toggle_button(
            "view-icons",
            IconName::LayoutGrid,
            "图标视图",
            icons_view,
            accent,
            accent_foreground,
            muted_foreground,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.set_view_mode(ViewMode::Icons, cx);
            }),
        );
        let view_details = toggle_button(
            "view-details",
            IconName::List,
            "详细信息视图",
            !icons_view,
            accent,
            accent_foreground,
            muted_foreground,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.set_view_mode(ViewMode::Details, cx);
            }),
        );
        let theme_toggle = nav_button(
            "toggle-theme",
            if dark { IconName::Sun } else { IconName::Moon },
            if dark {
                "切换到浅色主题"
            } else {
                "切换到深色主题"
            },
            true,
            cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.toggle_theme(window, cx);
            }),
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(COMMAND_BAR_HEIGHT))
            .flex_none()
            .child(back)
            .child(forward)
            .child(up)
            .child(refresh)
            .child(address)
            .child(view_icons)
            .child(view_details)
            .child(theme_toggle)
            .into_any_element()
    }
}

/// 导航类图标按钮。
///
/// 不可用时只降低不透明度并**不挂点击处理器**——不使用空处理器是为了让「不可用」
/// 这件事在元素树上就是可判定的，而不是靠运行时判断。
pub(crate) fn nav_button<F>(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    enabled: bool,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    div()
        .flex_none()
        .opacity(if enabled { 1.0 } else { 0.35 })
        .child(
            Button::new(id)
                .icon(Icon::new(icon))
                .tooltip(tooltip)
                .when(enabled, move |button| button.on_click(on_click)),
        )
        .into_any_element()
}

/// 带选中态的切换按钮，用于视图模式这类「二选一」的入口。
#[allow(clippy::too_many_arguments)]
pub(crate) fn toggle_button<F>(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    active: bool,
    active_bg: Hsla,
    active_foreground: Hsla,
    idle_foreground: Hsla,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    div()
        .flex_none()
        .rounded(px(6.0))
        .text_color(if active { active_foreground } else { idle_foreground })
        .when(active, |element| element.bg(active_bg))
        .child(
            Button::new(id)
                .icon(Icon::new(icon))
                .tooltip(tooltip)
                .on_click(on_click),
        )
        .into_any_element()
}
