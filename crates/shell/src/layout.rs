//! 整体布局：响应式断点、内容区（选中条 / 表头 / 虚拟列表 / 空状态）与预览面板的定位。
//!
//! 断点数值与设计稿的 `@media` 一一对应（见 [`fm_views::breakpoints`]）：
//! ≤1240 隐藏「类型」列、≤1100 预览改浮层、≤980 隐藏「修改日期」、
//! ≤920 侧边栏改浮层、≤720 隐藏同步状态与状态栏的视图控件、≤560 隐藏品牌文字。
//!
//! 本文件的职责边界：**只决定「放什么、放在哪」**，具体控件形态由 `fm_views` 提供。

use std::path::PathBuf;
use std::rc::Rc;

use fm_core::{Entry, Location, ViewMode};
use fm_views::{
    SelectHandler, breakpoints, computer, details_table, empty_state, icon_grid, metrics,
    preview_panel, virtual_rows, widgets,
};
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_kit_assets::IconName;

use crate::shell::ShellView;
use crate::sidebar::{SIDEBAR_COLLAPSED_WIDTH, SIDEBAR_WIDTH};

/// 内容区右侧需要为滚动条预留的宽度。
///
/// 网格列数按「整窗宽 - 侧边栏宽」推算，若不预留滚动条宽度，
/// 滚动条一出现就会把最后一列的右半边挤到可视区之外。
/// 取 16px 对齐 Windows 11 Fluent 滚动条的默认宽度。
const SCROLLBAR_RESERVE: f32 = 16.0;

/// 当前视口的响应式状态。
///
/// 一次性算好所有断点判断，而不是在各个渲染函数里反复比对视口宽度：
/// 「哪些控件在当前宽度下存在」是同一个决策，分散判断必然会出现两处不一致。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// 视口宽度。
    pub width: f32,
    /// 侧边栏以浮层呈现（≤920）。
    pub sidebar_overlay: bool,
    /// 命令条压缩：隐藏同步状态、搜索框收窄（≤720）。
    pub compact: bool,
    /// 极窄：隐藏品牌文字（≤560）。
    pub narrow: bool,
    /// 预览面板以浮层呈现（≤1100）。
    pub preview_overlay: bool,
    /// 面包屑只显示首段与末段（≤720）。
    pub compact_crumbs: bool,
    /// 隐藏状态栏的视图 / 缩放控件（≤720）。
    pub hide_status_extras: bool,
}

impl Viewport {
    pub fn for_width(width: f32) -> Self {
        Self {
            width,
            sidebar_overlay: width <= breakpoints::SIDEBAR_OVERLAY,
            compact: width <= breakpoints::COMPACT,
            narrow: width <= breakpoints::NARROW,
            preview_overlay: width <= breakpoints::PREVIEW_OVERLAY,
            compact_crumbs: width <= breakpoints::COMPACT,
            hide_status_extras: width <= breakpoints::COMPACT,
        }
    }

    /// 侧边栏当前占用的横向像素。
    ///
    /// 浮层模式下侧边栏不占位（它压在内容之上），因此这里返回 0——
    /// 否则窄屏的内容区会凭空少掉 238px。
    pub fn sidebar_width(&self, collapsed: bool) -> f32 {
        if self.sidebar_overlay {
            return 0.0;
        }
        if collapsed {
            SIDEBAR_COLLAPSED_WIDTH
        } else {
            SIDEBAR_WIDTH
        }
    }

    /// 预览面板当前占用的横向像素（浮层模式不占位）。
    pub fn preview_width(&self, open: bool) -> f32 {
        if !open {
            return 0.0;
        }
        if self.preview_overlay {
            0.0
        } else {
            metrics::PREVIEW_WIDTH
        }
    }
}

impl ShellView {
    /// 内容区可用的横向像素（**推算值**，非实测值）。
    ///
    /// 已知局限：GPUI 未向上层暴露子元素的实测宽度，因此这里只能用推算。
    /// 代价是侧边栏 180ms 折叠动画期间会出现最多一列的瞬时误差 ——
    /// 折叠时内容区在**变宽**，而这里已按最终宽度算好了列数，落在动画前中段的
    /// 最后一列会被裁掉一部分。要彻底消除需要把侧边栏的动画进度暴露给内容区，
    /// 属虚拟化网格的固有取舍，此处先以注释记录。
    pub(crate) fn content_width(&self, viewport: Viewport) -> f32 {
        let sidebar = viewport.sidebar_width(self.config.sidebar_collapsed);
        let preview = viewport.preview_width(self.preview_open);
        (viewport.width - sidebar - preview - SCROLLBAR_RESERVE).max(0.0)
    }

    /// 主内容区。
    ///
    /// 状态优先级由 `if / else if` 的顺序表达：
    /// 错误 → 「此电脑」→ 筛选无结果 → 空目录 / 骨架 → 双视图之一。
    ///
    /// 本函数只决定「放什么」，容器装配统一交给末尾那一次 [`content_column`]。
    /// 这样任何新增视图分支都不可能漏掉容器样式 ——
    /// 漏装的表现是内容把状态栏顶出窗口，且不会有任何编译期提示。
    pub(crate) fn render_content(
        &self,
        viewport: Viewport,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &self.tokens;

        let mut children: Vec<AnyElement> = Vec::new();

        if let Some(batch) = self.render_batch_bar(cx) {
            children.push(batch);
        }

        let body = if let Some(error) = self.error.clone() {
            // 错误优先展示：出错时列表本来就是空的，先显示骨架会误导。
            empty_state::error(error, tokens)
        } else if matches!(self.tabs.active_state().location(), Location::Computer) {
            // 「此电脑」不是目录，没有条目可扫，它的内容是驱动器总览。
            // 这一分支必须排在 `entries.is_empty()` 之前：虚拟位置的 `entries` 恒为空，
            // 否则会被误判成「空文件夹」而显示错误的提示。
            self.render_computer(cx)
        } else if self.visible().is_empty() {
            if !self.query.trim().is_empty() {
                empty_state::no_results(self.query.trim(), tokens)
            } else if self.loading {
                empty_state::loading(empty_state::SKELETON_ROWS, tokens)
            } else {
                empty_state::empty_folder(tokens)
            }
        } else {
            // 加载中且已有条目的情况会走到这里：分批到达的条目继续追加到列表，
            // 不退回骨架屏，避免已显示的内容闪回。
            match self.tabs.active_state().view_mode() {
                ViewMode::Details => self.render_details(viewport, cx),
                ViewMode::Icons => self.render_icon_grid(viewport, window, cx),
            }
        };

        children.push(body);
        content_column(children)
    }

    /// 选中项操作条（设计稿的 `.batch`）。没有选中项时返回 `None`，条不占高度。
    fn render_batch_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = self.selection.len();
        if count == 0 {
            return None;
        }
        let tokens = &self.tokens;
        let size = fm_core::format_size(self.selection_size());

        // 复制 / 剪切 / 重命名 / 移到回收站都属于文件写操作，本阶段尚未接入
        // （见 README 已知缺口）。它们仍然渲染出来并保持设计稿的视觉位置，
        // 但以降不透明度的禁用态呈现，避免出现「点了没反应」的控件。
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .child(widgets::text_button(
                "batch-copy",
                "复制",
                Some(IconName::Copy),
                false,
                tokens,
                |_, _, _| {},
            ))
            .child(widgets::text_button(
                "batch-cut",
                "剪切",
                Some(IconName::Scissors),
                false,
                tokens,
                |_, _, _| {},
            ))
            .child(widgets::text_button(
                "batch-rename",
                "重命名",
                Some(IconName::Pencil),
                false,
                tokens,
                |_, _, _| {},
            ))
            .child(widgets::text_button(
                "batch-delete",
                "移到回收站",
                Some(IconName::Trash),
                false,
                tokens,
                |_, _, _| {},
            ))
            .child(widgets::text_button(
                "batch-clear",
                "取消选择",
                None,
                true,
                tokens,
                cx.listener(|this, _: &gpui::ClickEvent, _window, cx| {
                    this.clear_selection(cx);
                }),
            ));

        Some(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(metrics::ROW_PADDING_X))
                .py(px(7.0))
                .bg(tokens.panel)
                .border_b_1()
                .border_color(tokens.border)
                .text_size(px(12.5))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(tokens.fg)
                        .child(widgets::dot(tokens.accent, 6.0))
                        .child(format!("已选择 {count} 项")),
                )
                .child(widgets::mono(
                    size,
                    metrics::FONT_XS,
                    tokens.muted_fg,
                    tokens,
                ))
                .child(div().flex_grow(1.0))
                .child(actions)
                .into_any_element(),
        )
    }

    /// 详细信息视图：表头 + 虚拟化数据行。
    fn render_details(&self, viewport: Viewport, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        let tab = self.tabs.active_state();
        let columns = details_table::Columns::for_viewport(viewport.width, tokens);

        let header = details_table::header(
            tab.sort_key(),
            tab.sort_direction(),
            self.all_selected(),
            columns,
            tokens,
            {
                let view = cx.entity().downgrade();
                Rc::new(move |key, _window, cx: &mut App| {
                    // `update` 只在视图已被销毁（窗口关闭）时失败。
                    // 不静默丢弃：表头「点了没反应」是最难定位的一类问题，
                    // 留一条 debug 日志能让它在 `RUST_LOG=debug` 下立刻暴露。
                    if view.update(cx, |view, cx| view.set_sort(key, cx)).is_err() {
                        tracing::debug!("切换排序时视图已销毁，本次点击被忽略");
                    }
                })
            },
            {
                let view = cx.entity().downgrade();
                Rc::new(move |selected, _window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_select_all(selected, cx))
                        .is_err()
                    {
                        tracing::debug!("全选时视图已销毁，本次点击被忽略");
                    }
                })
            },
        );

        // 只有视口内的行会进入这个闭包，因此在这里做类型/时间格式化是安全的。
        let open = open_handler(cx);
        let select = select_handler(cx);
        let list = virtual_rows(
            "details-list",
            cx,
            self.visible().len(),
            move |view: &mut Self, index, _window, _cx| {
                let Some(entry) = view.visible().get(index) else {
                    return div().into_any_element();
                };
                let name = view.display_name(entry);
                let kind = view.kind_label(entry);
                let modified = Self::modified_label(entry);
                details_table::row(
                    entry,
                    &name,
                    &kind,
                    modified.as_deref(),
                    view.selection.contains(index),
                    view.cursor == Some(index),
                    columns,
                    &view.tokens,
                    index,
                    &open,
                    &select,
                )
            },
        )
        .into_any_element();

        div()
            .flex()
            .flex_col()
            .flex_grow(1.0)
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(header)
            .child(list)
            .into_any_element()
    }

    /// 图标视图：按「行」虚拟化的自适应网格。
    fn render_icon_grid(
        &self,
        viewport: Viewport,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &self.tokens;
        // 网格自身有 12px 外衬（设计稿 `.files.grid` 的 padding）。
        let inner = (self.content_width(viewport) - metrics::GRID_PADDING * 2.0).max(0.0);
        let columns = icon_grid::columns_for_width(inner, tokens.cell);
        let cell_width = icon_grid::cell_width(inner, columns, tokens.cell);
        let rows = icon_grid::row_count(self.visible().len(), columns);

        let open = open_handler(cx);
        let select = select_handler(cx);
        let list = virtual_rows(
            "icon-grid",
            cx,
            rows,
            move |view: &mut Self, row_index, _window, _cx| {
                let start = row_index * columns;
                icon_grid::row(
                    view.visible(),
                    start,
                    columns,
                    cell_width,
                    |index| {
                        view.visible()
                            .get(index)
                            .and_then(|entry| view.extension_label(entry))
                    },
                    |index| view.selection.contains(index),
                    view.cursor,
                    &view.tokens,
                    &open,
                    &select,
                )
            },
        )
        .px(px(metrics::GRID_PADDING))
        .pt(px(metrics::GRID_PADDING))
        .pb(px(20.0))
        .into_any_element();

        // 列表 id 固定为 "icon-grid"，而 `rows` 由当前列数决定，
        // 因此滚动位置是按**行号**锚定的：调整窗口宽度改变列数时，
        // 「第 N 行」对应的条目会整批换掉，表现为视图跳位。
        // 改成按条目锚定需要滚动句柄的配合，属虚拟化网格的固有取舍，此处先记录成因。
        list
    }

    /// 「此电脑」：驱动器总览。
    fn render_computer(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        if self.drives.is_empty() {
            // 枚举还在进行时给骨架，否则会出现「闪一下空状态再冒出卡片」。
            return if self.loading {
                empty_state::loading(4, tokens)
            } else {
                empty_state::notice(IconName::HardDrive, "未检测到驱动器", tokens)
            };
        }

        let view = cx.entity().downgrade();
        let on_open = move |path: PathBuf, _window: &mut Window, cx: &mut App| {
            if view
                .update(cx, |view, cx| view.navigate_to(path, cx))
                .is_err()
            {
                tracing::debug!("打开驱动器时视图已销毁，本次操作被忽略");
            }
        };

        computer::drives_overview(&self.drives, tokens, on_open)
    }

    /// 右侧预览面板。
    ///
    /// 宽屏时它是「第三栏」；窄屏（≤1100）时改为压在内容区之上的浮层，
    /// 定位依赖 `render` 里挂在 `body` 上的 `relative()`。
    pub(crate) fn render_preview(
        &self,
        viewport: Viewport,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.preview_open {
            return None;
        }
        let tokens = &self.tokens;
        let content = self.preview_content();

        let on_open = {
            let view = cx.entity().downgrade();
            Rc::new(move |path: PathBuf, _window: &mut Window, cx: &mut App| {
                if view
                    .update(cx, |view, cx| view.navigate_to(path, cx))
                    .is_err()
                {
                    tracing::debug!("预览面板打开目录时视图已销毁，本次操作被忽略");
                }
            })
        };

        let inner = preview_panel::panel(&content, tokens, Some(on_open));

        let container = div()
            .flex_none()
            .flex()
            .flex_col()
            .h_full()
            .bg(tokens.panel)
            .border_l_1()
            .border_color(tokens.border)
            .child(inner);

        Some(if viewport.preview_overlay {
            container
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(px(metrics::PREVIEW_OVERLAY_WIDTH))
                .shadow_lg()
                .into_any_element()
        } else {
            container.w(px(metrics::PREVIEW_WIDTH)).into_any_element()
        })
    }

    /// 窄屏侧边栏浮层下的遮罩：点它即收起侧边栏。
    pub(crate) fn render_scrim(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("sidebar-scrim")
            .absolute()
            .inset_0()
            // 设计稿的遮罩是 `color-mix(fg 40%, transparent)`：
            // 40% 透明度的前景色，而不是把两个不透明色混合——后者会把底下的内容盖死。
            .bg(fm_views::tokens::with_alpha(self.tokens.fg, 0.4))
            .cursor_pointer()
            .on_click(cx.listener(|this, _: &gpui::ClickEvent, _window, cx| {
                this.close_sidebar_overlay(cx);
            }))
            .into_any_element()
    }
}

/// 构造「双击打开」处理器，两个视图共用。
///
/// 只有目录会被导航进入。文件的双击**明确不做任何事**：
/// 用默认程序打开需要 `ShellExecute`（属 `fm-platform` 的后续工作），
/// 在那之前绝不能把文件也交给 `navigate_to` —— 那会去扫描一个文件路径，
/// 用户看到的是「路径不存在」这种莫名其妙的报错。
pub(crate) fn open_handler(cx: &Context<ShellView>) -> fm_views::OpenHandler {
    let view = cx.entity().downgrade();
    Rc::new(move |entry: Entry, _window: &mut Window, cx: &mut App| {
        if !entry.is_dir() {
            return;
        }
        let path = entry.path.as_ref().clone();
        if view
            .update(cx, |view, cx| view.navigate_to(path, cx))
            .is_err()
        {
            // 与表头排序一致：不静默吞错，留一条可定位的 debug 日志。
            tracing::debug!("双击进入目录时视图已销毁，本次操作被忽略");
        }
    })
}

/// 构造「单击选中」处理器。
pub(crate) fn select_handler(cx: &Context<ShellView>) -> SelectHandler {
    let view = cx.entity().downgrade();
    Rc::new(
        move |index: usize, mode: fm_views::SelectMode, _window: &mut Window, cx: &mut App| {
            if view
                .update(cx, |view, cx| view.select(index, mode, cx))
                .is_err()
            {
                tracing::debug!("选择条目时视图已销毁，本次操作被忽略");
            }
        },
    )
}

/// 内容区容器：统一裁剪与最小高度，避免长内容把状态栏顶出窗口。
///
/// 接受一组**纵向排列**的子元素，而不是单个元素：这样「选中条 + 表头 + 列表」
/// 这类多段内容与「空状态」这类单段内容就能走同一条路径。
///
/// 约定：这是内容区容器的**唯一装配点**，各视图函数只返回内容片段、不得自行包容器。
fn content_column(children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .min_w(px(0.0))
        .min_h(px(0.0))
        .h_full()
        .overflow_hidden()
        .children(children)
        .into_any_element()
}
