//! 详细信息视图：固定表头 + 虚拟化数据行。
//!
//! 列宽与列序直接来自设计稿的 `--cols: 30px minmax(0,1fr) 138px 104px 86px`，
//! 对应「勾选框 / 名称 / 修改日期 / 类型 / 大小」。窄屏下逐列退让（见 [`Columns::for_viewport`]）。
//!
//! 行高由密度决定；`uniform_list` 实测的是第一项，因此只要全表等高，换密度不会让滚动漂移。
//!
//! ## 列宽拖拽的两个刻意设计
//!
//! 1. **可见性与宽度分离**：断点仍然决定「这一列在不在」（`Option<f32>`），
//!    用户的拖动偏好只覆盖**可见列**的宽度。否则窄屏拖动一下就会把宽屏的布局也改掉。
//! 2. **拖动只改样式**：拖拽期间不重排、不重扫，只更新一个 `f32` 并重绘。
//!    行内容只依赖条目本身，列宽变化不影响虚拟化的行高。
//!
//! 待补：按列右键菜单（隐藏/自适应列宽）。

use std::rc::Rc;

use fm_core::{Column, ColumnWidths, Entry, SortDirection, SortKey, format_size};
use gpui::{
    AnyElement, App, ClickEvent, CursorStyle, Entity, Hsla, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px, prelude::FluentBuilder as _,
};
use gpui_component::input::InputState;
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::tokens::{Tokens, breakpoints, metrics};
use crate::widgets;
use crate::{DragSelectHandler, EntryMenuBuilder, OpenHandler, RightPressHandler, SelectHandler, SelectMode};

/// 点击列名排序的回调。
///
/// 用类型别名而不是把 `Rc<dyn Fn(..)>` 写进签名：同一个签名在表头与各列里出现多次，
/// 写全了既难读，也让人难以一眼看出「这几个参数是同一个东西」。
pub type SortHandler = Rc<dyn Fn(SortKey, &mut Window, &mut App)>;

/// 表头勾选框（全选 / 全不选）的回调，参数是目标选中状态。
pub type SelectAllHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// 拖动列宽分隔线时的回调。参数是被拖的列与**按下时的窗口横坐标**。
///
/// 坐标而不是 delta：拖动状态（起点与起始宽度）由 shell 持有，
/// 视图层每帧重建，把状态放在这里会每帧清零。
pub type ColumnResizeStart = Rc<dyn Fn(Column, f32, &mut Window, &mut App)>;

/// 各列的可见性与宽度。
///
/// 用 `Option<f32>` 表达「这一列在当前宽度下是否显示」，
/// 比在渲染时反复比对视口宽度更不容易漏判——列宽与可见性本就该一起决定。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Columns {
    /// 修改日期列宽；`None` = 不显示。
    pub date: Option<f32>,
    /// 类型列宽；`None` = 不显示。
    pub kind: Option<f32>,
    /// 大小列宽。
    pub size: f32,
    /// 行高（随界面密度）。
    pub row_height: f32,
}

impl Columns {
    /// 按视口宽度选择列布局，断点与设计稿的 `@media` 一致。
    ///
    /// `overrides` 只覆盖**可见列**的宽度；`content_width` 用于在名称列被挤到
    /// 180px 以下之前收敛三列之和（见 [`ColumnWidths::fitted`]）。
    /// 两者都取「用户偏好」与「当前窗口」的实时值，因此拖动一步就能看到结果，
    /// 不需要先把偏好写盘。
    pub fn for_viewport(
        viewport_width: f32,
        tokens: &Tokens,
        overrides: ColumnWidths,
        content_width: f32,
    ) -> Self {
        let row_height = tokens.row_height;
        // 先算断点默认值：它决定「这一列在不在」，也决定没拖过时的宽度。
        let base = if viewport_width <= breakpoints::NARROW {
            Self {
                date: None,
                kind: None,
                size: metrics::SIZE_COLUMN_TINY,
                row_height,
            }
        } else if viewport_width <= breakpoints::HIDE_DATE {
            Self {
                date: None,
                kind: None,
                size: metrics::SIZE_COLUMN_NARROW,
                row_height,
            }
        } else if viewport_width <= breakpoints::HIDE_TYPE {
            Self {
                date: Some(metrics::DATE_COLUMN_NARROW),
                kind: None,
                size: metrics::SIZE_COLUMN,
                row_height,
            }
        } else {
            Self {
                date: Some(metrics::DATE_COLUMN),
                kind: Some(metrics::TYPE_COLUMN),
                size: metrics::SIZE_COLUMN,
                row_height,
            }
        };

        let fitted = overrides.fitted(content_width);
        Self {
            // 隐藏列不应用偏好：`None` 保持为「不在」，而不是「宽度为某个值」。
            date: base
                .date
                .map(|width| fitted.get(Column::Date).unwrap_or(width)),
            kind: base
                .kind
                .map(|width| fitted.get(Column::Kind).unwrap_or(width)),
            size: fitted.get(Column::Size).unwrap_or(base.size),
            row_height,
        }
    }
}

/// 表头：勾选框 + 可点击排序的列名 + 可拖动的列宽分隔线。
#[allow(clippy::too_many_arguments)]
pub fn header(
    sort_key: SortKey,
    direction: SortDirection,
    all_selected: bool,
    columns: Columns,
    tokens: &Tokens,
    on_sort: SortHandler,
    on_select_all: SelectAllHandler,
    on_resize: Option<ColumnResizeStart>,
) -> AnyElement {
    let hover = tokens.hover;

    let select_all = div()
        .id("select-all")
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(metrics::CHECKBOX_COLUMN))
        .h_full()
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_event: &ClickEvent, window, cx| {
            (*on_select_all)(!all_selected, window, cx);
        })
        .child(widgets::checkbox(all_selected, tokens));

    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .w_full()
        .h(px(metrics::LIST_HEAD_HEIGHT))
        // 与数据行使用同一组左右内边距，表头文字才能和列内容对齐——
        // 只给数据行加内边距会让表头整体左移 12px，看上去像两套网格。
        .px(px(metrics::ROW_PADDING_X))
        .border_b_1()
        .border_color(tokens.border)
        .font_family(tokens.mono_font.clone())
        .text_size(px(metrics::FONT_XS))
        .text_color(tokens.muted_fg)
        .child(select_all);

    row = row.child(header_cell(
        "名称",
        SortKey::Name,
        None,
        false,
        sort_key,
        direction,
        tokens,
        Rc::clone(&on_sort),
        None,
    ));
    if let Some(width) = columns.date {
        row = row.child(header_cell(
            "修改日期",
            SortKey::Modified,
            Some(width),
            false,
            sort_key,
            direction,
            tokens,
            Rc::clone(&on_sort),
            on_resize.as_ref().map(|handler| (Column::Date, handler)),
        ));
    }
    if let Some(width) = columns.kind {
        row = row.child(header_cell(
            "类型",
            SortKey::Kind,
            Some(width),
            false,
            sort_key,
            direction,
            tokens,
            Rc::clone(&on_sort),
            on_resize.as_ref().map(|handler| (Column::Kind, handler)),
        ));
    }
    row = row.child(header_cell(
        "大小",
        SortKey::Size,
        Some(columns.size),
        true,
        sort_key,
        direction,
        tokens,
        on_sort,
        on_resize.as_ref().map(|handler| (Column::Size, handler)),
    ));

    row.into_any_element()
}

/// 渲染一行数据。
///
/// `name` / `kind_label` / `modified` 都由调用方预备好：名称要处理「是否显示扩展名」，
/// 类型需要扩展名驻留表，时间需要平台层的时区换算——三者都不该由视图层自己去取。
///
/// `index` 用作元素 id：虚拟化列表每帧都会重建行，id 必须在这两帧之间保持稳定，
/// 否则点击命中会错位。
///
/// `menu` / `right_press` 见 [`crate::context_menu::entry_menu_row`]：
/// 菜单内容由 shell 决定，这里只负责把菜单挂到行上。
///
/// `editing` 是「这一行正在被内联重命名」时的输入框实体（由 shell 持有）。
/// 视图层不创建也不销毁它——输入框的状态（文本、光标、焦点）必须跨帧存活，
/// 而视图每帧都在重建行，任何在这里 `cx.new` 的做法都会让每帧拿到一个新输入框。
#[allow(clippy::too_many_arguments)]
pub fn row(
    entry: &Entry,
    name: &str,
    kind_label: &str,
    modified: Option<&str>,
    selected: bool,
    current: bool,
    editing: Option<&Entity<InputState>>,
    columns: Columns,
    tokens: &Tokens,
    index: usize,
    open: &OpenHandler,
    select: &SelectHandler,
    menu: &EntryMenuBuilder,
    right_press: &RightPressHandler,
    drag_start: &DragSelectHandler,
    drag_move: &DragSelectHandler,
) -> AnyElement {
    // 目录的「大小」没有意义（系统给出的目录 metadata.len() 与内容无关），显示为破折号。
    let size = if entry.is_dir() {
        String::from("—")
    } else {
        format_size(entry.size)
    };
    let modified = modified.unwrap_or("—");
    let icon = crate::widgets::entry_icon(entry, None);

    let hover = tokens.hover;
    let accent_soft = tokens.accent_soft;
    let accent_line = tokens.accent_line;
    let muted = tokens.muted_fg;

    let opened = Rc::clone(open);
    let target = entry.clone();
    let row_select = Rc::clone(select);
    let checkbox_select = Rc::clone(select);
    let drag_start = Rc::clone(drag_start);
    let drag_move = Rc::clone(drag_move);

    // 勾选框在未选中且未悬停时隐藏（设计稿的 `.row:hover .cbxwrap input`）。
    // 每个行是独立的 group，避免悬停一行时所有行的勾选框一起冒出来。
    let group_name: SharedString = format!("row-{index}").into();
    let checkbox_group = group_name.clone();

    let checkbox = div()
        .id(("row-cbx", index))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(metrics::CHECKBOX_COLUMN))
        .h_full()
        .opacity(if selected { 1.0 } else { 0.0 })
        .group_hover(checkbox_group, |style| style.opacity(1.0))
        .cursor_pointer()
        .on_click(move |_event: &ClickEvent, window, cx| {
            // 阻止冒泡到整行：否则「勾选」会先切换、再被整行的替换选择覆盖掉。
            cx.stop_propagation();
            (*checkbox_select)(index, SelectMode::Toggle, window, cx);
        })
        .child(widgets::checkbox(selected, tokens));

    let name_cell = div()
        .flex_grow(1.0)
        .min_w(px(0.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(9.0))
        .overflow_hidden()
        .child(widgets::glyph(
            entry.icon,
            icon,
            16.0,
            if entry.is_dir() {
                tokens.accent
            } else {
                muted
            },
        ))
        // 重命名时**只替换名称文本，图标留着**：与设计稿的标记一致
        // （`<span class="ic">` 在编辑态里保留），也让用户仍能一眼看出
        // 自己改的是哪一项，而不是失去所有视觉参照。
        .child(match editing {
            Some(state) => widgets::rename_input(state, tokens),
            None => div()
                .flex_grow(1.0)
                .min_w(px(0.0))
                .overflow_hidden()
                .truncate()
                .text_size(px(12.5))
                .text_color(tokens.fg)
                .child(name.to_string())
                .into_any_element(),
        });

    let selected_hover = crate::tokens::with_alpha(tokens.fg, 0.1);
    let mut row = div()
        .id(("details-row", index))
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .w_full()
        .h(px(columns.row_height))
        .px(px(metrics::ROW_PADDING_X))
        .group(group_name)
        .when(editing.is_none(), |element| element.cursor_pointer())
        // hover 每个元素只能挂**一次**：GPUI 的 `hover()` 带
        // `debug_assert!(hover_style.is_none())`，第二次调用会直接 panic。
        // release 把断言编译掉了，所以这个写法一直没暴露；dev 构建下
        // 只要有任何一行处于选中态就会崩。选中态因此不是「再挂一个 hover」，
        // 而是让同一个闭包按选中与否给出不同底色。
        .hover(move |style| {
            if selected {
                style.bg(selected_hover)
            } else {
                style.bg(hover)
            }
        })
        .when(selected, |element| element.bg(accent_soft))
        .when(current, |element| {
            element.border_1().border_color(accent_line)
        })
        // 框选：左键按下记锚点，滑过每一行按「锚点..当前」替换选中。
        // 锚点只记不选（见 `ShellView::begin_rubber_band`），因此不与下方的
        // 单击 / 双击冲突——单击仍走 on_click，双击仍走打开。
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            (*drag_start)(index, event.position, window, cx);
        })
        .on_mouse_move(move |event, window, cx| {
            (*drag_move)(index, event.position, window, cx);
        })
        // 编辑态下整行不再响应点击：输入框就在行内，而点击会**冒泡**到行上，
        // 那会顺带改变选中集（把用户正在改的那一项取消选中），
        // 更糟的是双击会触发「打开」——在改名的过程中把文件打开。
        .when(editing.is_none(), |element| {
            element.on_click(move |event: &ClickEvent, window, cx| {
                // 单击与双击共用同一个处理器，必须用 click_count 区分：
                // 只有双击才是「打开」，单击只更新选中集。
                if event.click_count() == 2 {
                    (*opened)(target.clone(), window, cx);
                    return;
                }
                (*row_select)(index, crate::select_mode(event), window, cx);
            })
        })
        .child(checkbox)
        .child(name_cell);

    if let Some(width) = columns.date {
        row = row.child(plain_cell(
            modified.to_string(),
            width,
            metrics::FONT_XS,
            muted,
            &tokens.mono_font,
            false,
        ));
    }
    if let Some(width) = columns.kind {
        row = row.child(plain_cell(
            kind_label.to_string(),
            width,
            metrics::FONT_SM,
            muted,
            &tokens.mono_font,
            false,
        ));
    }
    row = row.child(plain_cell(
        size,
        columns.size,
        metrics::FONT_XS,
        muted,
        &tokens.mono_font,
        true,
    ));

    // 包装层的布局必须与原来放在行上的约束一致（`flex_none` + `w_full`）：
    // 它现在才是虚拟列表的直接子项，行高与宽度都由它决定。
    let wrapper = div()
        .id(("details-row-wrap", index))
        .flex()
        .flex_row()
        .flex_none()
        .w_full();

    crate::context_menu::entry_menu_row(wrapper, row, entry, index, menu, right_press)
}

/// 表头里的可排序列。
#[allow(clippy::too_many_arguments)]
fn header_cell(
    label: &'static str,
    key: SortKey,
    width: Option<f32>,
    align_end: bool,
    active_key: SortKey,
    direction: SortDirection,
    tokens: &Tokens,
    on_sort: SortHandler,
    resize: Option<(Column, &ColumnResizeStart)>,
) -> AnyElement {
    let is_active = key == active_key;
    let foreground = tokens.fg;
    let hover = tokens.hover;
    // 升序画向上箭头、降序画向下箭头：与文件管理器的通行习惯一致。
    let arrow = is_active.then(|| {
        let icon = if direction == SortDirection::Ascending {
            IconName::ArrowUp
        } else {
            IconName::ArrowDown
        };
        Icon::new(icon).with_size(px(12.0))
    });

    // 用 SortKey 的判别式做元素 id：同一列在不同帧之间保持一致，点击才稳定。
    let base = div()
        .id(("th", key as usize))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .h_full()
        .cursor_pointer()
        .text_color(if is_active { foreground } else { tokens.muted_fg })
        .hover(move |style| style.text_color(foreground).bg(hover))
        .on_click(move |_event: &ClickEvent, window, cx| {
            (*on_sort)(key, window, cx);
        })
        .when(align_end, |element| element.justify_end())
        .child(label)
        .children(arrow);

    let Some(width) = width else {
        return base.flex_grow(1.0).min_w(px(0.0)).into_any_element();
    };

    // `relative()` 是分隔线的定位基准：命中区做成**绝对定位**的 6px 竖条，
    // 压在列的交界处。这样它不参与 flex 布局——否则每个分隔线都会把
    // 表头的数据列整体挤窄 6px，表头与数据行就错位了。
    let mut cell = div()
        .relative()
        .flex_none()
        .flex()
        .w(px(width))
        .h_full()
        .child(base.flex_grow(1.0).min_w(px(0.0)));

    if let Some((column, handler)) = resize {
        let handler = Rc::clone(handler);
        let accent = tokens.accent;
        // 分隔线只在悬停/拖动时可见：常驻一条竖线会让表头看起来像被切碎了。
        cell = cell.child(
            div()
                .id(("th-resize", column as usize))
                .absolute()
                .left(px(-3.0))
                .top_0()
                .bottom_0()
                .w(px(6.0))
                .cursor(CursorStyle::ResizeLeftRight)
                .hover(move |style| style.bg(crate::tokens::with_alpha(accent, 0.45)))
                .on_mouse_down(
                    MouseButton::Left,
                    move |event, window, cx| {
                        // 阻止冒泡：否则按在分隔线上会顺带触发这一列的排序。
                        cx.stop_propagation();
                        handler(column, f32::from(event.position.x), window, cx);
                    },
                ),
        );
    }

    cell.into_any_element()
}

/// 固定宽度 + 单行截断的文本单元格（日期 / 类型 / 大小）。
///
/// `align_end` 只对「大小」列为真：数字右对齐才能让小数点成列；日期与类型左对齐。
fn plain_cell(
    content: String,
    width: f32,
    font_size: f32,
    color: Hsla,
    mono_font: &SharedString,
    align_end: bool,
) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .when(align_end, |element| element.justify_end())
        .w(px(width))
        .h_full()
        .overflow_hidden()
        .font_family(mono_font.clone())
        .text_size(px(font_size))
        .text_color(color)
        .child(div().overflow_hidden().truncate().child(content))
        .into_any_element()
}
