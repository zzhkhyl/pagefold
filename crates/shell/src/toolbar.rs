//! 命令条：导航按钮组 + 地址区（面包屑 / 可编辑地址栏）+ 搜索框 + 视图切换 + 动作按钮。
//!
//! 形态照搬设计稿的 `.toolbar`：7/10px 内边距、32px 高的地址栏与搜索框、
//! 6px 圆角、1px 描边，控件之间 4px 间距。窄窗时允许折行（设计稿的 `flex-wrap`），
//! 而不是把右侧按钮挤出窗口。
//!
//! 搜索框与地址栏都采用「先静态文本、点一下才创建输入框」的做法：
//! `InputState` 需要窗口句柄，渲染期无法创建；而且未编辑时也不需要文本编辑状态。

use fm_core::ViewMode;
use fm_views::breadcrumb;
use fm_views::metrics;
use fm_views::widgets;
use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::button::{
    Button, ButtonCustomVariant, ButtonVariant, ButtonVariants as _, DropdownButton,
};
use gpui_component::input::Input;
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::layout::Viewport;
use crate::shell::ShellView;

impl ShellView {
    /// 渲染命令条。
    pub(crate) fn render_toolbar(&self, viewport: Viewport, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
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
        let icons_view = tab.state().view_mode() == ViewMode::Icons;
        let sidebar_active = if viewport.sidebar_overlay {
            self.sidebar_overlay
        } else {
            !self.config.sidebar_collapsed
        };

        let navigation = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .child(widgets::icon_button(
                "nav-back",
                IconName::ArrowLeft,
                16.0,
                "后退",
                false,
                can_back,
                tokens,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.go_back(cx);
                }),
            ))
            .child(widgets::icon_button(
                "nav-forward",
                IconName::ArrowRight,
                16.0,
                "前进",
                false,
                can_forward,
                tokens,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.go_forward(cx);
                }),
            ))
            .child(widgets::icon_button(
                "nav-up",
                IconName::ArrowUp,
                16.0,
                "上一级",
                false,
                can_up,
                tokens,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.go_up(cx);
                }),
            ))
            .child(widgets::plain_icon_button(
                "nav-refresh",
                IconName::RefreshCw,
                16.0,
                "刷新",
                tokens,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.reload(cx);
                }),
            ));

        let sidebar_toggle = widgets::icon_button(
            "sidebar-toggle",
            if sidebar_active {
                IconName::PanelLeftClose
            } else {
                IconName::PanelLeft
            },
            16.0,
            "显示 / 隐藏导航区",
            sidebar_active,
            true,
            tokens,
            // `Viewport` 是 `Copy`，闭包按值捕获即可，无需先做一次多余的再绑定。
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.toggle_sidebar(viewport, cx);
            }),
        );

        let address = self.render_address(viewport, cx);

        let search = self.render_search(viewport, cx);

        let view_switch = widgets::segmented(
            vec![
                widgets::icon_button_with_size(
                    "view-list",
                    IconName::List,
                    15.0,
                    metrics::SEGMENT_CONTROL_SIZE,
                    "详细信息视图",
                    !icons_view,
                    true,
                    tokens,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.set_view_mode(ViewMode::Details, cx);
                    }),
                ),
                widgets::icon_button_with_size(
                    "view-grid",
                    IconName::LayoutGrid,
                    15.0,
                    metrics::SEGMENT_CONTROL_SIZE,
                    "图标视图",
                    icons_view,
                    true,
                    tokens,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.set_view_mode(ViewMode::Icons, cx);
                    }),
                ),
            ],
            tokens,
        );

        // 排序按钮直接提示当前排序键：本阶段它用「循环切换」代替弹出菜单，
        // 因此提示文案必须是动态的，用户才知道下一次点击会切到哪。
        let sort_label: SharedString = match self.sort_key() {
            fm_core::SortKey::Name => "排序方式：名称（点击切换）".into(),
            fm_core::SortKey::Modified => "排序方式：修改日期（点击切换）".into(),
            fm_core::SortKey::Kind => "排序方式：类型（点击切换）".into(),
            fm_core::SortKey::Size => "排序方式：大小（点击切换）".into(),
        };
        let sort = widgets::plain_icon_button(
            "sort-toggle",
            IconName::ArrowDownUp,
            16.0,
            sort_label,
            tokens,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.cycle_sort(cx);
            }),
        );

        let preview_toggle = widgets::icon_button(
            "preview-toggle",
            IconName::PanelRight,
            16.0,
            "显示 / 隐藏预览面板",
            self.preview_open,
            true,
            tokens,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_preview(cx);
            }),
        );

        // 设置入口放在命令条右端：设计稿的「设置 / 帮助」原本在品牌栏上，
        // 那一栏整条删掉之后它们一直没有落点，这里补上（标签条菜单里还有一份）。
        let on_settings = self.in_settings();
        let settings = widgets::icon_button(
            "open-settings",
            IconName::Settings,
            16.0,
            "设置（Ctrl+,）",
            on_settings,
            true,
            tokens,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.open_settings(cx);
            }),
        );

        // 命令条上的「新建」是一个**下拉**：设计稿的 `$('#btnNew').onclick`
        // 会弹出「新建文件夹 / 新建文本文档 / 新建 Markdown 文档 / 粘贴」四项。
        //
        // 用组件库的 `DropdownButton` 承载，而不是自己拼一个弹出层：定位、
        // 边缘翻转、Esc 与点击外部关闭、上下键导航都由它与 `PopupMenu` 提供——
        // 与内容区右键菜单复用同一套机制，不重复造轮子。
        //
        // 触发按钮的配色走 `ButtonCustomVariant`：这样它和旁边那些手绘按钮
        // 用**同一套设计令牌**（同样的悬停底色、选中底色与文字色），
        // 不会因为组件库的默认按钮配色而在命令条上显得突兀。
        let new_button = DropdownButton::new("new-item")
            .button(
                Button::new("new-item-trigger")
                    .with_variant(ButtonVariant::Custom(
                        ButtonCustomVariant::new(cx)
                            .color(gpui::transparent_black())
                            .foreground(tokens.muted_fg)
                            .hover(tokens.hover)
                            .active(tokens.accent_soft),
                    ))
                    .icon(Icon::new(IconName::Plus).with_size(px(16.0)))
                    .tooltip("新建")
                    // 尺寸对齐设计稿的 30px 命中区，与相邻按钮等高。
                    .w(px(metrics::CONTROL_SIZE))
                    .h(px(metrics::CONTROL_SIZE)),
            )
            .dropdown_menu(crate::context_menu::new_menu_builder(cx));

        let mut bar = div()
            .flex_none()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(4.0))
            .gap_y(px(6.0))
            .px(px(metrics::TOOLBAR_PADDING_X))
            .py(px(metrics::TOOLBAR_PADDING_Y))
            .bg(tokens.surface)
            .border_b_1()
            .border_color(tokens.border)
            .child(navigation)
            .child(sidebar_toggle)
            .child(address);

        // 设置页不是文件夹：搜索、视图切换、排序、预览与「新建」在那里
        // 全都是无对象可作用的操作。留着一个点了没反应的按钮，
        // 比让它消失更像故障，因此整组隐藏。
        if !on_settings {
            bar = bar
                .child(search)
                .child(view_switch)
                .child(sort)
                .child(preview_toggle);
        }

        bar.child(settings)
            .when(!on_settings, |element| element.child(new_button))
            .into_any_element()
    }

    /// 地址区：面包屑（或「此电脑」）+ 复制路径按钮；编辑态下换成输入框。
    fn render_address(&self, viewport: Viewport, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        let location = self.tabs.active_state().location().clone();

        if self.address_editing {
            let editor = match self.address_input.clone() {
                Some(state) => div()
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .child(Input::new(&state).appearance(false))
                    .into_any_element(),
                // 理论上进入编辑态就一定有输入状态；真出现缺失也不能渲染成半截界面。
                None => div().flex_grow(1.0).into_any_element(),
            };

            // 编辑态把描边换成强调色，对应设计稿的 `:focus-within`。
            return address_box(tokens)
                .border_color(tokens.accent)
                .child(editor)
                .into_any_element();
        }

        let body = match &location {
            // 虚拟位置没有层级可拆，直接给一个静态标签。
            // 文案不在这里硬编码：`Location::title()` 是「位置叫什么」的唯一来源，
            // 标签页标题与面包屑因此不会各写一份而慢慢分叉。
            fm_core::Location::Computer
            | fm_core::Location::RecycleBin
            | fm_core::Location::Settings => div()
                .flex_grow(1.0)
                .min_w(px(0.0))
                .px(px(2.0))
                .text_size(px(12.5))
                .text_color(tokens.fg)
                .child(location.title())
                .into_any_element(),
            fm_core::Location::Directory(path) => {
                let crumbs = breadcrumb::fold(
                    &breadcrumb::segments(path),
                    if viewport.compact_crumbs {
                        breadcrumb::COMPACT_VISIBLE_SEGMENTS
                    } else {
                        breadcrumb::MAX_VISIBLE_SEGMENTS
                    },
                );
                let navigate = {
                    let view = cx.entity().downgrade();
                    move |target: std::path::PathBuf, _window: &mut Window, cx: &mut gpui::App| {
                        view.update(cx, |view, cx| view.navigate_to(target, cx)).ok();
                    }
                };
                breadcrumb::render(&crumbs, tokens, navigate)
            }
        };

        let copy = widgets::icon_button_with_size(
            "copy-path",
            IconName::Copy,
            14.0,
            26.0,
            "复制当前路径",
            false,
            true,
            tokens,
            cx.listener(|this, _: &ClickEvent, window, cx| {
                // 阻止冒泡：否则复制路径会顺带把地址栏切换成可编辑态。
                cx.stop_propagation();
                this.copy_current_path(window, cx);
            }),
        );

        address_box(tokens)
            // 点击空白处切换为可编辑地址栏：设计稿的地址栏没有「编辑」按钮，
            // 但用户需要一条输入完整路径的途径。
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.begin_address_edit(window, cx);
            }))
            .child(body)
            .child(copy)
            .into_any_element()
    }

    /// 搜索框：编辑态下是真实输入框，否则是「图标 + 占位符 / 关键词 + 清除按钮」。
    fn render_search(&self, viewport: Viewport, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        let has_query = !self.query.trim().is_empty();

        let mut box_ = div()
            .id("search-box")
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .h(px(metrics::ADDRESS_HEIGHT))
            .px(px(8.0))
            .border_1()
            .border_color(tokens.border)
            .rounded(px(metrics::RADIUS))
            .bg(tokens.bg)
            .cursor_text();

        // 窄窗下按设计稿收窄，但保留输入能力（设计稿在更窄时会收成图标按钮，
        // 那需要一个额外的小输入弹层，本阶段以「收窄」替代并在 README 记录）。
        box_ = if viewport.compact {
            box_.flex_shrink(1.0).min_w(px(metrics::SEARCH_MIN_WIDTH))
        } else {
            box_.flex_shrink(1.0).w(px(metrics::SEARCH_WIDTH))
        };

        if self.search_editing {
            let editor = match self.search_input.clone() {
                Some(state) => div()
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .child(Input::new(&state).appearance(false))
                    .into_any_element(),
                None => div().flex_grow(1.0).into_any_element(),
            };
            // 编辑态把描边换成强调色，对应设计稿的 `input:focus`。
            return box_
                .border_color(tokens.accent)
                .child(editor)
                .into_any_element();
        }

        let text: SharedString = if has_query {
            self.query.clone().into()
        } else {
            "搜索当前文件夹".into()
        };
        let text_color = if has_query {
            tokens.fg
        } else {
            tokens.muted_fg
        };

        box_ = box_
            .cursor_pointer()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.begin_search_edit(window, cx);
            }))
            .child(
                Icon::new(IconName::Search)
                    .with_size(px(15.0))
                    .text_color(tokens.muted_fg),
            )
            .child(
                div()
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(text_color)
                    .child(text),
            );

        if has_query {
            box_ = box_.child(widgets::icon_button_with_size(
                "search-clear",
                IconName::X,
                13.0,
                20.0,
                "清除搜索",
                false,
                true,
                tokens,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    // 阻止冒泡：否则清除搜索会立刻又把搜索框切换成编辑态。
                    cx.stop_propagation();
                    this.clear_query(cx);
                }),
            ));
        }

        box_.into_any_element()
    }
}

/// 地址栏外框：与输入框共用同一套描边 / 圆角 / 底色。
fn address_box(tokens: &fm_views::Tokens) -> gpui::Stateful<gpui::Div> {
    div()
        .id("address-bar")
        .flex_grow(1.0)
        .flex_basis(px(metrics::ADDRESS_MIN_WIDTH))
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .h(px(metrics::ADDRESS_HEIGHT))
        .pl(px(6.0))
        .pr(px(2.0))
        .border_1()
        .border_color(tokens.border)
        .rounded(px(metrics::RADIUS))
        .bg(tokens.bg)
        .cursor_text()
}
