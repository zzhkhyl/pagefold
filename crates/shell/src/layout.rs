//! 整体布局：工具栏的组合，以及主内容区。
//!
//! 响应式断点（PLAN.md「响应式」章节）在本阶段先落地了「侧边栏可折叠」这一条：
//! 折叠状态由用户控制并持久化。按窗口宽度自动折叠、命令条随宽度压缩等，
//! 需等虚拟化视图稳定后再统一接入，避免同时改动布局与性能敏感代码。

use std::path::PathBuf;
use std::rc::Rc;

use fm_core::{Entry, Location, ViewMode};
use fm_views::{
    Colors, OpenHandler, computer, details_table, empty_state, icon_grid, virtual_rows,
};
use gpui::{
    AnyElement, App, Context, IntoElement, ParentElement as _, Styled as _, Window, div, px,
};
use gpui_component::ActiveTheme as _;
use gpui_kit_assets::IconName;

use crate::shell::{ShellView, palette};
use crate::sidebar::{SIDEBAR_COLLAPSED_WIDTH, SIDEBAR_WIDTH};

/// 内容区右侧需要为滚动条预留的宽度。
///
/// 网格列数按「整窗宽 - 侧边栏宽」推算，若不预留滚动条宽度，
/// 滚动条一出现就会把最后一列的右半边挤到可视区之外。
/// 取 16px 对齐 Windows 11 Fluent 滚动条的默认宽度。
const SCROLLBAR_RESERVE: f32 = 16.0;

impl ShellView {
    /// 顶部工具栏：标签条（上）+ 命令条（下）。
    ///
    /// 这里的窗口参数只能是 `&mut Window`：它转交的两个子渲染函数
    /// （`render_tab_strip` / `render_command_bar`）签名要求可变借用，
    /// 单独把这里收窄成 `&Window` 会直接编译不过。
    /// 渲染路径其实全程只读窗口，等那两个签名一并收窄后，这里也应同步收窄。
    pub(crate) fn render_toolbar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab_strip = self.render_tab_strip(window, cx);
        let command_bar = self.render_command_bar(window, cx);
        let border = cx.theme().border;

        div()
            .flex()
            .flex_col()
            .flex_none()
            .border_b_1()
            .border_color(border)
            .child(tab_strip)
            .child(command_bar)
            .into_any_element()
    }

    /// 主内容区：自适应剩余空间。
    ///
    /// 状态优先级由 `if / else if` 的顺序表达：错误 → 加载骨架 → 空目录 → 双视图之一。
    ///
    /// 本函数只决定「放什么」，容器装配统一交给末尾那一次 [`content_surface`]。
    /// 这样任何新增视图分支都不可能漏掉容器样式 ——
    /// 漏装的表现是内容把状态栏顶出窗口，且不会有任何编译期提示。
    ///
    /// 只接收 `&Window`：本函数与其调用的 `render_icon_grid` 都只读窗口，
    /// 声明可变借用是多余的。调用方持有的 `&mut Window` 会自动降级成 `&Window`。
    pub(crate) fn render_content(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = palette(cx);

        let children: Vec<AnyElement> = if let Some(error) = self.error.clone() {
            // 错误优先展示：出错时列表本来就是空的，先显示骨架会误导。
            vec![empty_state::error(error, colors)]
        } else if matches!(self.tabs.active_state().location(), Location::Computer) {
            // 「此电脑」不是目录，没有条目可扫，它的内容是驱动器总览。
            // 这一分支必须排在 `entries.is_empty()` 之前：虚拟位置的 `entries` 恒为空，
            // 否则会被误判成「空文件夹」而显示错误的提示。
            vec![self.render_computer(colors, cx)]
        } else if self.entries.is_empty() {
            vec![if self.loading {
                empty_state::loading(empty_state::SKELETON_ROWS, colors)
            } else {
                empty_state::empty_folder(colors)
            }]
        } else {
            // 加载中且已有条目的情况会走到这里：分批到达的条目继续追加到列表，
            // 不退回骨架屏，避免已显示的内容闪回。
            match self.tabs.active_state().view_mode() {
                ViewMode::Details => self.render_details(cx),
                ViewMode::Icons => self.render_icon_grid(window, cx),
            }
        };

        content_surface(children)
    }

    /// 详细信息视图：固定表头 + 虚拟化数据行。
    ///
    /// 返回内容片段而非成品元素：容器由 [`ShellView::render_content`] 统一装配。
    fn render_details(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = palette(cx);
        let tab = self.tabs.active_state();

        let header = details_table::header(
            tab.sort_key(),
            tab.sort_direction(),
            colors,
            {
                let view = cx.entity().downgrade();
                move |key, _window, cx: &mut App| {
                    // `update` 只在视图已被销毁（窗口关闭）时失败。
                    // 不静默丢弃：表头「点了没反应」是最难定位的一类问题，
                    // 留一条 debug 日志能让它在 `RUST_LOG=debug` 下立刻暴露。
                    if view.update(cx, |view, cx| view.set_sort(key, cx)).is_err() {
                        tracing::debug!("切换排序时视图已销毁，本次点击被忽略");
                    }
                }
            },
        );

        // 只有视口内的行会进入这个闭包，因此在这里做类型/时间格式化是安全的。
        // `colors` 在外层算一次再 move 进来：配色与具体是哪一行无关，
        // 留在闭包内会变成「每个可见行都读一遍主题字段」。
        let open = open_handler(cx);
        let list = virtual_rows(
            "details-list",
            cx,
            self.entries.len(),
            move |view: &mut Self, index, _window, _cx| {
                let Some(entry) = view.entries.get(index) else {
                    return div().into_any_element();
                };
                let kind = view.kind_label(entry);
                let modified = Self::modified_label(entry);
                details_table::row(entry, &kind, modified.as_deref(), false, colors, index, &open)
            },
        )
        // `virtual_rows` 返回的是 `impl IntoElement` 不透明类型，
        // 统一转成 `AnyElement` 才能和表头一起交给内容区容器。
        .into_any_element();

        vec![header, list]
    }

    /// 图标视图：按「行」虚拟化的自适应网格。
    ///
    /// 返回内容片段而非成品元素：容器由 [`ShellView::render_content`] 统一装配。
    fn render_icon_grid(&self, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let colors = palette(cx);
        let columns = icon_grid::columns_for_width(available_width(
            window,
            self.config.sidebar_collapsed,
        ));
        let rows = icon_grid::row_count(self.entries.len(), columns);

        // 同 `render_details`：配色与打开处理器都在闭包外准备一次。
        let open = open_handler(cx);
        let list = virtual_rows(
            "icon-grid",
            cx,
            rows,
            move |view: &mut Self, row_index, _window, _cx| {
                let start = row_index * columns;
                icon_grid::row(
                    &view.entries,
                    start,
                    columns,
                    |index| {
                        view.entries
                            .get(index)
                            .and_then(|entry| view.extension_label(entry))
                    },
                    colors,
                    &open,
                )
            },
        )
        .into_any_element();

        // 列表 id 固定为 "icon-grid"，而 `rows` 由当前列数决定，
        // 因此滚动位置是按**行号**锚定的：调整窗口宽度改变列数时，
        // 「第 N 行」对应的条目会整批换掉，表现为视图跳位。
        // 改成按条目锚定需要滚动句柄的配合（见 `fm_views::icon_grid` 的说明），
        // 属虚拟化网格的固有取舍，此处先记录成因。
        vec![list]
    }

    /// 「此电脑」：驱动器总览。
    fn render_computer(&self, colors: Colors, cx: &mut Context<Self>) -> AnyElement {
        if self.drives.is_empty() {
            // 枚举还在进行时给骨架，否则会出现「闪一下空状态再冒出卡片」。
            return if self.loading {
                empty_state::loading(4, colors)
            } else {
                empty_state::notice(IconName::HardDrive, "未检测到驱动器", colors)
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

        computer::drives_overview(&self.drives, colors, on_open)
    }
}

/// 构造「双击打开」处理器，两个视图共用。
///
/// 只有目录会被导航进入。文件的双击**明确不做任何事**：
/// 用默认程序打开需要 `ShellExecute`（属 `fm-platform` 的后续工作），
/// 在那之前绝不能把文件也交给 `navigate_to` —— 那会去扫描一个文件路径，
/// 用户看到的是「路径不存在」这种莫名其妙的报错。
fn open_handler(cx: &Context<ShellView>) -> OpenHandler {
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

/// 内容区容器：统一裁剪与最小高度，避免长内容把状态栏顶出窗口。
///
/// 接受一组**纵向排列**的子元素，而不是单个元素：这样「表头 + 列表」这类多段内容
/// 与「空状态」这类单段内容就能走同一条路径。
///
/// 约定：这是内容区容器的**唯一装配点**，只由 [`ShellView::render_content`] 在末尾调用一次，
/// 各视图函数只返回内容片段、不得自行包容器。
/// 早先的写法是「部分分支用容器、另两个视图各复制一份样式链」，
/// 结果是样式调整只对一半界面生效，而且漏包的后果（把状态栏顶出窗口）
/// 没有任何编译期提示 —— 收敛到单一装配点就是为了消除这一整类缺陷。
fn content_surface(children: impl IntoIterator<Item = AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .min_w(px(0.0))
        .h_full()
        .overflow_hidden()
        .children(children)
        .into_any_element()
}

/// 内容区可用的横向像素（**推算值**，非实测值）。
///
/// 必须减去侧边栏宽度：否则窗口较窄时按整窗宽度算出的列数会多一列，
/// 最后一列被裁在可视区之外，表现为「右边缺一块」。再减去滚动条预留宽度。
///
/// 已知局限：GPUI 未向上层暴露子元素的实测宽度，因此这里只能用推算。
/// 代价是侧边栏 180ms 折叠动画期间会出现最多一列的瞬时误差 ——
/// 折叠时内容区在**变宽**，而这里已按最终宽度算好了列数，落在动画前中段的
/// 最后一列会被裁掉一部分。要彻底消除需要把侧边栏的动画进度暴露给内容区
/// （或在 `ShellView` 上记录动画时间窗），两者都超出本文件范围，故先以注释记录。
fn available_width(window: &Window, sidebar_collapsed: bool) -> f32 {
    let sidebar = if sidebar_collapsed {
        SIDEBAR_COLLAPSED_WIDTH
    } else {
        SIDEBAR_WIDTH
    };
    // 用 `f32::from` 而不是取字段：`Pixels` 的内部表示是私有的，只能走转换。
    let width = f32::from(window.viewport_size().width);
    // `max(0.0)` 兜住窗口极窄或最小化时的负值，避免下游用负数算列数。
    (width - sidebar - SCROLLBAR_RESERVE).max(0.0)
}
