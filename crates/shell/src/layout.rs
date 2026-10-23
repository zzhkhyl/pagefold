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
use fm_services::clipboard::ClipboardMode;
use fm_views::{
    AreaMenuBuilder, ColumnResizeStart, DragSelectHandler, RevealHandler, SelectHandler,
    breakpoints, computer, details_table, empty_state, icon_grid, metrics, preview_panel,
    settings_page, virtual_rows, widgets,
};
use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    Pixels, Point, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::WindowExt as _;
use gpui_component::input::Input;
use gpui_component::menu::ContextMenuExt as _;
use gpui_kit_assets::IconName;

use crate::shell::ShellView;
use crate::sidebar::{SIDEBAR_COLLAPSED_WIDTH, SIDEBAR_WIDTH};

/// 内容区右侧需要为滚动条预留的宽度。
///
/// 网格列数按「整窗宽 - 侧边栏宽」推算，若不预留滚动条宽度，
/// 滚动条一出现就会把最后一列的右半边挤到可视区之外。
/// 取 16px 对齐 Windows 11 Fluent 滚动条的默认宽度。
const SCROLLBAR_RESERVE: f32 = 16.0;

/// 结束拖动会话的回调（框选与列宽拖拽共用）。
type ReleaseHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 内容区的指针移动回调。
///
/// 之所以提到内容区这一层：列宽分隔线只有 6px 宽，鼠标一离开细线就收不到
/// 后续的 `on_mouse_move`，拖动会「卡住」在半路上。
type PointerMoveHandler = Rc<dyn Fn(Point<Pixels>, &mut Window, &mut App)>;

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

        // 设置页是独立的内容形态：没有条目、没有选中条、也没有空白处菜单。
        // 单独返回而不是塞进下面那条 if/else 链——否则「未选择项目」的常驻条
        // 与「新建 / 粘贴」菜单都会跟着出现在设置页上，而它们在那里全都不可用。
        if self.in_settings() {
            return div()
                .flex()
                .flex_col()
                .flex_grow(1.0)
                .min_w(px(0.0))
                .min_h(px(0.0))
                .h_full()
                .overflow_hidden()
                .child(self.render_settings(cx))
                .into_any_element();
        }

        let mut children: Vec<AnyElement> = Vec::new();

        // 选中条常驻（见 `render_batch_bar` 的说明：它不再随选中状态出现/消失，
        // 否则下面的列表每点一次条目就上下跳一次）。
        children.push(self.render_batch_bar(cx));

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
            } else if self.in_recycle_bin() {
                // 「回收站是空的」与「这个文件夹是空的」不是一回事：
                // 后者在暗示用户往里放东西，而空回收站什么也不用做。
                empty_state::recycle_bin_empty(tokens)
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

        // 内容区空白处的右键菜单。只有「没命中行」的右键才会走到它：
        // 行上的右键会在 `fm_views::context_menu` 的外层包装里被截断。
        let area_menu = crate::context_menu::area_menu_builder(cx);

        // 左键抬起（含在内容区外抬起）时结束**所有**拖动会话：
        // 框选与列宽拖拽共用这一个出口。分成两个处理器也能工作，
        // 但「漏掉其中一个」的后果是某一类拖动状态卡住，而那种 bug 只在
        // 拖到内容区外松手时才会出现。
        let release: ReleaseHandler = {
            let view = cx.entity().downgrade();
            Rc::new(move |_window, cx: &mut App| {
                if view
                    .update(cx, |view, cx| {
                        view.end_rubber_band();
                        view.end_column_resize(cx);
                    })
                    .is_err()
                {
                    tracing::debug!("结束拖动时视图已销毁");
                }
            })
        };

        // 列宽拖拽的位移在内容区这一层收：分隔线只有 6px 宽，
        // 挂在自己的 `on_mouse_move` 上时，鼠标一离开那条细线就收不到事件了。
        let pointer_move: PointerMoveHandler = {
            let view = cx.entity().downgrade();
            Rc::new(move |position, _window, cx: &mut App| {
                if view
                    .update(cx, |view, cx| {
                        view.update_column_resize(f32::from(position.x), cx);
                    })
                    .is_err()
                {
                    tracing::debug!("拖动列宽时视图已销毁");
                }
            })
        };

        children.push(body);
        content_column(children, &area_menu, release, pointer_move)
    }

    /// 选中项操作条（设计稿的 `.batch`）。
    ///
    /// **常驻**：没有选中项时也占着同样的高度——左端写「未选择项目」，动作全部禁用。
    /// 设计稿是 `hidden` 掉整条（`.batch[hidden]{display:none}`），但那样每点一次
    /// 条目，下面的列表就会上下跳一次；这种跳动的代价（手感）比省下的 40px 大得多。
    ///
    /// 高度**定死**（与预览面板表头共用 [`metrics::BATCH_BAR_HEIGHT`]）而不是靠内边距
    /// 撑出来：两栏并排，差一两个像素就会被看出来。
    fn render_batch_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        let count = self.selection.len();
        let has_selection = count > 0;
        let size = fm_core::format_size(self.selection_size());

        // 写操作全部作用于「当前目录」，而虚拟位置没有目录可写：回收站里的条目
        // **看着**有路径，但那只指向 `$Recycle.Bin` 内部，复制 / 剪切 / 改名 /
        // 再删一次都没有意义。因此整组置为禁用并说明原因，而不是让它们点了没反应。
        let writable = self.current_dir().is_some();
        let blocked = "回收站与「此电脑」里的项目不支持该操作";
        // 不能点的原因按优先级取：**没选中**排在位置不可写之前——
        // 一个都没选的时候说「回收站不支持该操作」是答非所问。
        let why = if has_selection {
            blocked
        } else {
            "请先选择要操作的项目"
        };
        let can_write = has_selection && writable;
        // 重命名一次只能改一项：批量重命名需要一个「前缀 + 起始序号 + 预览」的
        // 对话框（设计稿的 `#dlgRename`），属后续增量。
        let single = self.selection.single().is_some();

        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .child(widgets::text_button(
                "batch-copy",
                "复制",
                Some(IconName::Copy),
                if can_write {
                    "复制选中项到剪贴板"
                } else {
                    why
                },
                can_write,
                tokens,
                cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                    this.clip_selection(ClipboardMode::Copy, window, cx);
                }),
            ))
            .child(widgets::text_button(
                "batch-cut",
                "剪切",
                Some(IconName::Scissors),
                if can_write {
                    "剪切选中项（粘贴后原位置移除）"
                } else {
                    why
                },
                can_write,
                tokens,
                cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                    this.clip_selection(ClipboardMode::Cut, window, cx);
                }),
            ))
            .child(widgets::text_button(
                "batch-rename",
                "重命名",
                Some(IconName::Pencil),
                if !can_write {
                    why
                } else if single {
                    "重命名该项"
                } else {
                    "一次只能重命名一项（批量重命名尚未接入）"
                },
                can_write && single,
                tokens,
                cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                    this.rename_selected(window, cx);
                }),
            ))
            .child(widgets::text_button(
                "batch-delete",
                "移到回收站",
                Some(IconName::Trash),
                if can_write {
                    "移到回收站（需确认）"
                } else {
                    why
                },
                can_write,
                tokens,
                cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                    this.delete_selection(window, cx);
                }),
            ))
            .child(widgets::text_button(
                "batch-clear",
                "取消选择",
                None,
                if has_selection {
                    "取消选择"
                } else {
                    "没有选中项"
                },
                has_selection,
                tokens,
                cx.listener(|this, _: &gpui::ClickEvent, _window, cx| {
                    this.clear_selection(cx);
                }),
            ));

        let mut bar = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .px(px(metrics::ROW_PADDING_X))
            .h(px(metrics::BATCH_BAR_HEIGHT))
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
                    // 圆点只在真有选中项时出现：它是「你有选中」的指示器，
                    // 常驻的空态里留着它只会让人以为选上了什么。
                    .when(has_selection, |element| {
                        element.child(widgets::dot(tokens.accent, 6.0))
                    })
                    .child(if has_selection {
                        format!("已选择 {count} 项").into_any_element()
                    } else {
                        // 常驻的空态必须有一句话：只留一片空白，用户既看不出
                        // 「这里以后会出现操作」，也不知道按钮为什么都是灰的。
                        div()
                            .text_color(tokens.muted_fg)
                            .child("未选择项目")
                            .into_any_element()
                    }),
            );

        // 合计大小只在有选中项时出现：「0 字节」既没有意义又占位置。
        if has_selection {
            bar = bar.child(widgets::mono(
                size,
                metrics::FONT_XS,
                tokens.muted_fg,
                tokens,
            ));
        }

        bar.child(div().flex_grow(1.0))
            .child(actions)
            .into_any_element()
    }

    /// 详细信息视图：表头 + 虚拟化数据行。
    fn render_details(&self, viewport: Viewport, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        let tab = self.tabs.active_state();
        // 列宽 = 断点默认值 + 用户拖动偏好，并按内容区可用宽度收敛。
        // 三个输入都是「当前一帧的实时值」，因此拖动一步就能看到结果。
        let columns = details_table::Columns::for_viewport(
            viewport.width,
            tokens,
            self.config.detail_columns,
            self.content_width(viewport),
        );

        let on_resize: ColumnResizeStart = {
            let view = cx.entity().downgrade();
            Rc::new(move |column, x, window, cx: &mut App| {
                if view
                    .update(cx, |view, cx| {
                        view.begin_column_resize(column, x, window, cx);
                    })
                    .is_err()
                {
                    tracing::debug!("开始拖动列宽时视图已销毁");
                }
            })
        };

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
            Some(on_resize),
        );

        // 只有视口内的行会进入这个闭包，因此在这里做类型/时间格式化是安全的。
        let open = open_handler(cx);
        let select = select_handler(cx);
        let menu = crate::context_menu::entry_menu_builder(cx);
        let right_press = crate::context_menu::right_press_handler(cx);
        let (drag_start, drag_move) = drag_handlers(cx);
        let drag_out = self.drag_out_builder();
        let list = virtual_rows(
            "details-list",
            &self.details_scroll,
            cx,
            self.visible().len(),
            move |view: &mut Self, index, _window, _cx| {
                let Some(entry) = view.visible().get(index) else {
                    return div().into_any_element();
                };
                let name = view.display_name(entry);
                let kind = view.kind_label(entry);
                let modified = Self::modified_label(entry);
                // 正在被重命名的那一行，把名称位换成输入框。
                //
                // 除下标外**还要比对路径**：下标是发起重命名时记下的，而重命名
                // 期间可能有一次在途扫描落地并改变排序，那时下标会指向别的条目，
                // 输入框就会跑到错误的那一行上（而提交时用的仍是正确路径，
                // 结果会更让人困惑：改的是 A，输入框却在 B 上）。
                let editing = view
                    .renaming
                    .as_ref()
                    .filter(|renaming| {
                        renaming.index == index && entry.path.as_ref() == &renaming.path
                    })
                    .map(|renaming| &renaming.input);
                details_table::row(
                    entry,
                    &name,
                    &kind,
                    modified.as_deref(),
                    view.selection.contains(index),
                    view.cursor == Some(index),
                    editing,
                    columns,
                    &view.tokens,
                    index,
                    &open,
                    &select,
                    &menu,
                    &right_press,
                    &drag_start,
                    &drag_move,
                    Some(&drag_out),
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
    /// 当前视口下网格视图的列数。
    ///
    /// 抽成方法而不是在两处各算一遍：`scroll_into_view` 要把条目下标换算成行下标，
    /// 用的必须是**同一个**列数——两处各算一遍，迟早会因为某次改动而错位。
    pub(crate) fn grid_columns_for(&self, viewport: Viewport) -> usize {
        // 网格自身有 12px 外衬（设计稿 `.files.grid` 的 padding）。
        let inner = (self.content_width(viewport) - metrics::GRID_PADDING * 2.0).max(0.0);
        icon_grid::columns_for_width(inner, self.tokens.cell)
    }

    fn render_icon_grid(
        &self,
        viewport: Viewport,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = &self.tokens;
        let columns = self.grid_columns_for(viewport);
        let inner = (self.content_width(viewport) - metrics::GRID_PADDING * 2.0).max(0.0);
        let cell_width = icon_grid::cell_width(inner, columns, tokens.cell);
        let rows = icon_grid::row_count(self.visible().len(), columns);

        let open = open_handler(cx);
        let select = select_handler(cx);
        let menu = crate::context_menu::entry_menu_builder(cx);
        let right_press = crate::context_menu::right_press_handler(cx);
        let (drag_start, drag_move) = drag_handlers(cx);
        let drag_out = self.drag_out_builder();
        let list = virtual_rows(
            "icon-grid",
            &self.grid_scroll,
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
                    // 网格的一行里有多个单元格，因此这里交出「下标 + 输入框」，
                    // 由视图自己判断哪一个格子要换成输入框。
                    //
                    // 与详情视图同理，下标之外再核一次路径：在途扫描落地后
                    // 下标可能指向别的条目，那时输入框会跑到错误的格子上。
                    view.renaming
                        .as_ref()
                        .filter(|renaming| {
                            view.visible()
                                .get(renaming.index)
                                .is_some_and(|entry| entry.path.as_ref() == &renaming.path)
                        })
                        .map(|renaming| (renaming.index, &renaming.input)),
                    &view.tokens,
                    &open,
                    &select,
                    &menu,
                    &right_press,
                    &drag_start,
                    &drag_move,
                    Some(&drag_out),
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
        // 设置页隐藏预览面板：表单在 306px 的第三栏旁边会被挤成一条，
        // 而「当前选中项的属性」在设置页也没有意义。
        if !self.preview_open || self.in_settings() {
            return None;
        }
        let tokens = &self.tokens;
        let content = self.preview_content();

        let on_open = {
            let view = cx.entity().downgrade();
            Rc::new(move |path: PathBuf, window: &mut Window, cx: &mut App| {
                if view
                    .update(cx, |view, cx| view.open_path(path, window, cx))
                    .is_err()
                {
                    tracing::debug!("预览面板打开条目时视图已销毁，本次操作被忽略");
                }
            })
        };

        // 「在列表中定位」：选中项可能已经滚出视野（排序、筛选或键盘导航之后），
        // 这个按钮把用户带回那一行。滚动的换算在 `ShellView::scroll_into_view` 里，
        // 网格「下标 → 行号」的除法也在那里，这里不再重复。
        let on_reveal: RevealHandler = {
            let view = cx.entity().downgrade();
            Rc::new(move |_window, cx: &mut App| {
                if view
                    .update(cx, |view, cx| view.reveal_selected(cx))
                    .is_err()
                {
                    tracing::debug!("定位选中项时视图已销毁");
                }
            })
        };

        let inner = preview_panel::panel(&content, tokens, Some(on_open), Some(on_reveal));

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

    /// 设置页：把当前配置与一组回调交给 `fm_views::settings_page`。
    ///
    /// 快照是**值拷贝**：设置页每帧重建，读到的永远是这一帧的配置。
    /// 回调统一走 `WeakEntity::update`，因为点击发生在渲染之后，
    /// 那时不能假设视图还活着（窗口可能已经关掉）。
    pub(crate) fn render_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = settings_page::SettingsSnapshot {
            dark: self.config.dark,
            density: self.config.density,
            zoom: self.config.zoom,
            show_ext: self.config.show_ext,
            show_hidden: self.config.show_hidden,
            // 用运行期的面板状态而不是配置里的值：两者在窄屏浮层下可能短暂不同步。
            show_preview: self.preview_open,
            view_mode: self.tabs.active_state().view_mode(),
            config_path: fm_services::config::Config::path()
                .map(|path| path.display().to_string().into()),
            version: env!("CARGO_PKG_VERSION").into(),
            // 接管状态缓存在外壳里：设置页每帧渲染，不能每帧读四次注册表。
            folder_integration: settings_page::FolderIntegration {
                context_menu: self.folder_integration.context_menu,
                default_handler: self.folder_integration.default_handler,
            },
            shortcuts: {
                // 合并默认值 + 用户覆盖，并顺手算一次冲突。
                // 这些都在视图之外算好：视图只画，不做判断。
                let items = fm_core::resolve(&self.config.shortcuts);
                let conflicts = fm_core::conflicts(&items);

                // 全局热键的问题只做**纯规则**判断（语法、Win 组合、与内部命令撞车）。
                //
                // 刻意**不在这里调用 `probe_hotkey`**：那个函数会真的去注册一次再注销，
                // 而设置页每帧都会重建快照——每帧一次注册/注销既不必要，
                // 而且宿主正常情况下已经占着这个组合，探测必然失败，会误报成「被占用」。
                // 「是否被别的程序占着」只在用户**尝试改成新组合**时探测（见 `assign_global_hotkey`）。
                let global_keys = self.config.global_hotkey.keys.clone();
                let global_problem = if self.config.global_hotkey.enabled {
                    match fm_core::KeyCombo::parse(&global_keys) {
                        None => Some(String::from("键位无法识别，请在设置里重新录制")),
                        Some(combo) if combo.win => {
                            Some(String::from("Win 组合被系统占用，无法注册"))
                        }
                        Some(_) => fm_core::shortcut::occupant(&items, &global_keys)
                            .map(|other| format!("与「{}」重复", other.label)),
                    }
                } else {
                    None
                };

                settings_page::ShortcutsSnapshot {
                    items,
                    conflicts,
                    global_keys,
                    global_enabled: self.config.global_hotkey.enabled,
                    global_problem,
                }
            },
        };

        let handlers = settings_page::SettingsHandlers {
            set_dark: {
                let view = cx.entity().downgrade();
                Rc::new(move |dark, window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.set_dark(dark, window, cx))
                        .is_err()
                    {
                        tracing::debug!("设置主题时视图已销毁");
                    }
                })
            },
            set_density: {
                let view = cx.entity().downgrade();
                Rc::new(move |density, _window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.set_density(density, cx))
                        .is_err()
                    {
                        tracing::debug!("设置密度时视图已销毁");
                    }
                })
            },
            set_zoom: {
                let view = cx.entity().downgrade();
                Rc::new(move |zoom, _window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.set_zoom(zoom, cx))
                        .is_err()
                    {
                        tracing::debug!("设置缩放时视图已销毁");
                    }
                })
            },
            toggle_ext: {
                let view = cx.entity().downgrade();
                Rc::new(move |_window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_show_ext(cx))
                        .is_err()
                    {
                        tracing::debug!("切换扩展名显示时视图已销毁");
                    }
                })
            },
            toggle_hidden: {
                let view = cx.entity().downgrade();
                Rc::new(move |_window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_show_hidden(cx))
                        .is_err()
                    {
                        tracing::debug!("切换隐藏文件显示时视图已销毁");
                    }
                })
            },
            toggle_preview: {
                let view = cx.entity().downgrade();
                Rc::new(move |_window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_preview(cx))
                        .is_err()
                    {
                        tracing::debug!("切换预览面板时视图已销毁");
                    }
                })
            },
            set_view_mode: {
                let view = cx.entity().downgrade();
                Rc::new(move |mode, _window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.set_view_mode(mode, cx))
                        .is_err()
                    {
                        tracing::debug!("设置视图模式时视图已销毁");
                    }
                })
            },
            // 复制而不是「打开」配置文件夹：打开目录需要 `ShellExecute`，
            // 那属平台层（P4 的「双击文件用默认程序打开」一并接入）。
            // 复制路径已经能解决「我想看看配置文件」这个真实需求。
            copy_config_path: Rc::new(|window, cx: &mut App| {
                let Some(path) = fm_services::config::Config::path() else {
                    window.push_notification(
                        gpui_component::notification::Notification::warning(
                            "当前环境读不到 APPDATA，没有配置文件",
                        ),
                        cx,
                    );
                    return;
                };
                let text = path.display().to_string();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                window.push_notification(
                    gpui_component::notification::Notification::success(format!(
                        "已复制配置路径 {text}"
                    )),
                    cx,
                );
            }),
            toggle_folder_menu: {
                let view = cx.entity().downgrade();
                Rc::new(move |window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_folder_menu(window, cx))
                        .is_err()
                    {
                        tracing::debug!("切换文件夹右键菜单时视图已销毁");
                    }
                })
            },
            toggle_folder_default: {
                let view = cx.entity().downgrade();
                Rc::new(move |window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_folder_default(window, cx))
                        .is_err()
                    {
                        tracing::debug!("切换文件夹默认打开程序时视图已销毁");
                    }
                })
            },
            restore_folder_integration: {
                let view = cx.entity().downgrade();
                Rc::new(move |window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.restore_folder_integration(window, cx))
                        .is_err()
                    {
                        tracing::debug!("恢复资源管理器默认时视图已销毁");
                    }
                })
            },
            edit_shortcut: {
                let view = cx.entity().downgrade();
                Rc::new(move |id: String, window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| {
                            view.begin_shortcut_capture(
                                crate::shortcut_handlers::Target::Command(id),
                                window,
                                cx,
                            )
                        })
                        .is_err()
                    {
                        tracing::debug!("打开快捷键录制时视图已销毁");
                    }
                })
            },
            toggle_shortcut: {
                let view = cx.entity().downgrade();
                Rc::new(move |id: String, window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_shortcut_enabled(&id, window, cx))
                        .is_err()
                    {
                        tracing::debug!("切换快捷键启用态时视图已销毁");
                    }
                })
            },
            edit_global_hotkey: {
                let view = cx.entity().downgrade();
                Rc::new(move |window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| {
                            view.begin_shortcut_capture(
                                crate::shortcut_handlers::Target::Global,
                                window,
                                cx,
                            )
                        })
                        .is_err()
                    {
                        tracing::debug!("打开全局热键录制时视图已销毁");
                    }
                })
            },
            toggle_global_hotkey: {
                let view = cx.entity().downgrade();
                Rc::new(move |window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_global_hotkey(window, cx))
                        .is_err()
                    {
                        tracing::debug!("切换全局热键时视图已销毁");
                    }
                })
            },
            reset_shortcuts: {
                let view = cx.entity().downgrade();
                Rc::new(move |window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.reset_shortcuts(window, cx))
                        .is_err()
                    {
                        tracing::debug!("恢复默认快捷键时视图已销毁");
                    }
                })
            },
        };

        settings_page::page(&snapshot, &self.tokens, handlers)
    }

    /// 批量重命名浮层：遮罩 + 居中卡片。
    ///
    /// 为什么是自绘浮层而不是组件库的对话框：面板里有两个输入框和一段
    /// **实时预览**，而对话框的内容只在打开时构建一次。自绘浮层每帧随
    /// `render` 重建，预览天然跟着输入更新。
    pub(crate) fn render_batch_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let batch = self.batch.as_ref()?;
        let tokens = &self.tokens;
        let preview = self.batch_preview(cx);

        // 预览最多 6 行：再多会把面板撑过一屏，而「还剩几项」用一个数字就够了——
        // 用户要看的是「命名规则对不对」，不是把整批名字读一遍。
        const PREVIEW_ROWS: usize = 6;
        let rows: Vec<AnyElement> = preview
            .rows
            .iter()
            .take(PREVIEW_ROWS)
            .map(|row| {
                let color = if row.conflict { tokens.danger } else { tokens.fg };
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(metrics::FONT_SM))
                    .child(
                        div()
                            .flex_none()
                            .text_color(tokens.muted_fg)
                            .child(row.old.clone()),
                    )
                    .child(div().flex_none().text_color(tokens.muted_fg).child("→"))
                    .child(
                        div()
                            .flex_grow(1.0)
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_color(color)
                            .child(row.new.clone()),
                    )
                    .into_any_element()
            })
            .collect();
        let remaining = preview.rows.len().saturating_sub(PREVIEW_ROWS);

        // 一行状态：错误优先于撞名提示，两者都用危险色——它们都意味着「不能执行」。
        let status = match (&preview.error, preview.conflicts) {
            (Some(error), _) => Some(error.clone()),
            (None, conflicts) if conflicts > 0 => {
                Some(format!("{conflicts} 个目标名称已存在，改一下前缀或起始序号"))
            }
            _ => None,
        };

        let can_apply = preview.can_apply();

        let field = |label: &'static str, control: AnyElement| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .flex_none()
                        .w(px(72.0))
                        .text_size(px(metrics::FONT_SM))
                        .text_color(tokens.muted_fg)
                        .child(label),
                )
                .child(
                    div()
                        .flex_grow(1.0)
                        .min_w(px(0.0))
                        .h(px(28.0))
                        .px(px(8.0))
                        .rounded(px(metrics::RADIUS))
                        .border_1()
                        .border_color(tokens.border)
                        .bg(tokens.bg)
                        .flex()
                        .items_center()
                        .child(control),
                )
                .into_any_element()
        };

        let prefix = field(
            "前缀",
            Input::new(&batch.prefix).appearance(false).into_any_element(),
        );
        let start = field(
            "起始序号",
            Input::new(&batch.start).appearance(false).into_any_element(),
        );
        let keep_extension = {
            let view = cx.entity().downgrade();
            let control = widgets::switch(
                "batch-keep-ext",
                batch.keep_extension,
                tokens,
                move |_event: &gpui::ClickEvent, _window, cx: &mut App| {
                    if view
                        .update(cx, |view, cx| view.toggle_batch_extension(cx))
                        .is_err()
                    {
                        tracing::debug!("切换保留扩展名时视图已销毁");
                    }
                },
            );
            field("扩展名", control)
        };

        let cancel = widgets::text_button(
            "batch-cancel",
            "取消",
            None,
            "关闭（Esc）",
            true,
            tokens,
            cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                this.cancel_batch_rename(window, cx);
            }),
        );

        let apply = {
            let accent = tokens.accent;
            let surface = tokens.surface;
            div()
                .id("batch-apply")
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .px(px(14.0))
                .py(px(5.0))
                .rounded(px(metrics::RADIUS))
                .text_size(px(metrics::FONT_SM))
                .bg(if can_apply {
                    accent
                } else {
                    fm_views::tokens::with_alpha(tokens.fg, 0.2)
                })
                .text_color(surface)
                .when(can_apply, |element| {
                    element
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _: &gpui::ClickEvent, window, cx| {
                            this.apply_batch_rename(window, cx);
                        }))
                })
                .child("执行")
        };

        let card = div()
            .id("batch-card")
            .occlude()
            .w(px(520.0))
            .max_w_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(18.0))
            .rounded(px(metrics::RADIUS_LG))
            .border_1()
            .border_color(tokens.border)
            .bg(tokens.surface)
            .text_color(tokens.fg)
            .font_family(tokens.font_family.clone())
            .shadow_lg()
            .on_key_down(cx.listener(
                |this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.cancel_batch_rename(window, cx);
                    }
                },
            ))
            .child(
                div()
                    .flex_none()
                    .text_size(px(metrics::FONT_LG))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("批量重命名 · {} 项", preview.rows.len())),
            )
            .child(prefix)
            .child(start)
            .child(keep_extension)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .p(px(10.0))
                    .rounded(px(metrics::RADIUS))
                    .bg(tokens.bg)
                    .children(rows)
                    .when(remaining > 0, |element| {
                        element.child(
                            div()
                                .text_size(px(metrics::FONT_XS))
                                .text_color(tokens.muted_fg)
                                .child(format!("……另有 {remaining} 项")),
                        )
                    })
                    .when(preview.rows.is_empty() && preview.error.is_none(), |element| {
                        element.child(
                            div()
                                .text_size(px(metrics::FONT_XS))
                                .text_color(tokens.muted_fg)
                                .child("没有可预览的项目"),
                        )
                    }),
            )
            .when_some(status, |element, status| {
                element.child(
                    div()
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.danger)
                        .child(status),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(cancel)
                    .child(apply),
            );

        Some(
            div()
                .id("batch-overlay")
                .absolute()
                .inset_0()
                // 遮罩挡住底下的一切：浮层里的输入框拿不到「点空白处」的事件，
                // 也就不会误触后面的列表。
                .occlude()
                .bg(fm_views::tokens::with_alpha(tokens.fg, 0.35))
                .flex()
                .items_center()
                .justify_center()
                .child(card)
                .into_any_element(),
        )
    }

    /// 右下角的传输进度卡（复制 / 移动进行中时存在）。
    ///
    /// 刻意**不画百分比进度条**：复制开始前我们并不知道一棵目录里有多少文件，
    /// 而为了知道就得先完整遍历一遍（那是第二次 IO）。画一条按「顶层条目数」
    /// 推进的条会在「只复制一个 5GB 目录」时几乎不动，比不画更让人焦虑。
    /// 这里给出的是确定的信息：已复制多少文件、当前是哪一个。
    pub(crate) fn render_transfer_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let transfer = self.transfer.as_ref()?;
        let (label, files_done, total, current, cancelling, bytes_done, bytes_total) = {
            let state = transfer.lock().ok()?;
            (
                state.label.clone(),
                state.files_done,
                state.total,
                state.current.clone(),
                state.cancelling,
                state.bytes_done,
                state.bytes_total,
            )
        };

        let current = if current.as_os_str().is_empty() {
            String::from("准备中…")
        } else {
            crate::shell::truncate(&current.display().to_string(), 46)
        };

        // 字节进度：总大小已知时给百分比，未知时只给「已复制多少」。
        //
        // 为什么会有「未知」：源里含目录时，总大小要遍历整棵树才知道，
        // 而多遍历一遍等于第二次 IO。此时宁可少给一个百分比，
        // 也不要给一个随发现不断跳动的假数字。
        let fraction = if bytes_total > 0 {
            (bytes_done as f32 / bytes_total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let byte_text = if bytes_total > 0 {
            format!(
                "{}% · {} / {}",
                (fraction * 100.0).floor() as u32,
                fm_core::format_size(bytes_done),
                fm_core::format_size(bytes_total)
            )
        } else if bytes_done > 0 {
            format!("已复制 {}", fm_core::format_size(bytes_done))
        } else {
            String::from("准备中…")
        };

        let cancel = if cancelling {
            widgets::text_button(
                "transfer-cancel",
                "正在取消…",
                None,
                "下一块写完就停下（块级取消）",
                false,
                &self.tokens,
                |_: &gpui::ClickEvent, _: &mut Window, _: &mut App| {},
            )
        } else {
            widgets::text_button(
                "transfer-cancel",
                "取消",
                None,
                "停止复制（写到一半的文件会被删掉，不会留下半截）",
                true,
                &self.tokens,
                cx.listener(|this, _: &gpui::ClickEvent, _window, cx| {
                    this.cancel_transfer(cx);
                }),
            )
        };

        let tokens = &self.tokens;
        Some(
            div()
                .id("transfer-card")
                .occlude()
                .absolute()
                // 与通知统一到**底部居中**：通知由 `NotificationSettings::placement`
                // 设为 BottomCenter，这里用「左右都贴 0 + auto 外边距」把定宽卡片推回中线。
                .bottom(px(16.0))
                .left_0()
                .right_0()
                .mx_auto()
                .w(px(340.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .p(px(12.0))
                .rounded(px(metrics::RADIUS_LG))
                .border_1()
                .border_color(tokens.border)
                .bg(tokens.surface)
                .shadow_lg()
                .text_color(tokens.fg)
                .font_family(tokens.font_family.clone())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_grow(1.0)
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .text_size(px(metrics::FONT_SM))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(label),
                        )
                        .child(cancel),
                )
                // 进度条：只在总大小已知时出现。未知时留一条空槽会把
                // 「不知道」画成「0%」，比没有进度条更误导。
                .when(bytes_total > 0, |element| {
                    element.child(
                        div()
                            .w_full()
                            .h(px(4.0))
                            .rounded(px(2.0))
                            .bg(tokens.border)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(fraction))
                                    .rounded(px(2.0))
                                    .bg(tokens.accent),
                            ),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.muted_fg)
                        .child(format!("已处理 {files_done} 个文件 · 共 {total} 项"))
                        .child(div().child("·"))
                        .child(byte_text),
                )
                .child(
                    div()
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.muted_fg)
                        .child(current),
                )
                .into_any_element(),
        )
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
/// 双击的语义是「打开这一项」，与类型无关：目录进入、文件交给系统默认程序
/// （`ShellView::open_entry`）。文件**绝不能**交给 `navigate_to`——
/// 那会去扫描一个文件路径，用户看到的是「路径不存在」这种莫名其妙的报错。
pub(crate) fn open_handler(cx: &Context<ShellView>) -> fm_views::OpenHandler {
    let view = cx.entity().downgrade();
    Rc::new(move |entry: Entry, window: &mut Window, cx: &mut App| {
        // 目录进入与文件「用默认程序打开」都走同一个入口：
        // 双击的语义是「打开这一项」，而不该按类型分成两条看起来不同的路。
        if view
            .update(cx, |view, cx| view.open_entry(&entry, window, cx))
            .is_err()
        {
            // 与表头排序一致：不静默吞错，留一条可定位的 debug 日志。
            tracing::debug!("打开条目时视图已销毁，本次操作被忽略");
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

/// 构造「框选开始 / 框选经过」处理器，两个视图共用。
///
/// 两个处理器都要拿到鼠标坐标：阈值判断靠它区分「单击」与「框选」。
pub(crate) fn drag_handlers(cx: &Context<ShellView>) -> (DragSelectHandler, DragSelectHandler) {
    let start_view = cx.entity().downgrade();
    let start: DragSelectHandler = Rc::new(
        move |index: usize, position, _window: &mut Window, cx: &mut App| {
            if start_view
                .update(cx, |view, _cx| view.begin_rubber_band(index, position))
                .is_err()
            {
                tracing::debug!("框选起始时视图已销毁，本次操作被忽略");
            }
        },
    );

    let drag_view = cx.entity().downgrade();
    let drag: DragSelectHandler = Rc::new(
        move |index: usize, position, _window: &mut Window, cx: &mut App| {
            if drag_view
                .update(cx, |view, cx| view.update_rubber_band(index, position, cx))
                .is_err()
            {
                tracing::debug!("框选时视图已销毁，本次操作被忽略");
            }
        },
    );

    (start, drag)
}

/// 内容区容器：统一裁剪与最小高度，避免长内容把状态栏顶出窗口。
///
/// 接受一组**纵向排列**的子元素，而不是单个元素：这样「选中条 + 表头 + 列表」
/// 这类多段内容与「空状态」这类单段内容就能走同一条路径。
///
/// 约定：这是内容区容器的**唯一装配点**，各视图函数只返回内容片段、不得自行包容器。
///
/// `menu` 是空白处的右键菜单（内容由 [`crate::context_menu`] 决定）。
/// 容器必须带 `.id()`：`context_menu` 要用它派生稳定的元素 id，
/// 否则菜单的展开状态会在每一帧重渲染时丢失。
fn content_column(
    children: impl IntoIterator<Item = AnyElement>,
    menu: &AreaMenuBuilder,
    release: ReleaseHandler,
    pointer_move: PointerMoveHandler,
) -> AnyElement {
    let menu = menu.clone();
    let end_up = Rc::clone(&release);
    let end_out = Rc::clone(&release);
    div()
        .id("content-area")
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .min_w(px(0.0))
        .min_h(px(0.0))
        .h_full()
        .overflow_hidden()
        // 拖动位移在这一层收：列宽分隔线只有 6px 宽，
        // 挂在自己的处理器上时鼠标一离开就收不到后续事件。
        .on_mouse_move(move |event, window, cx| {
            (*pointer_move)(event.position, window, cx);
        })
        // 左键抬起即结束框选与列宽拖拽。`on_mouse_up_out` 兜住在内容区外松开的情况
        // （例如一路拖进侧边栏才松手）——那一次的鼠标抬起不会命中内容区本身。
        .on_mouse_up(MouseButton::Left, move |_event, window, cx| {
            (*end_up)(window, cx);
        })
        .on_mouse_up_out(MouseButton::Left, move |_event, window, cx| {
            (*end_out)(window, cx);
        })
        .children(children)
        .context_menu(move |popup, window, cx| (*menu)(popup, window, cx))
        .into_any_element()
}
