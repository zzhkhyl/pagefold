//! 详细信息视图：固定表头 + 虚拟化数据行。
//!
//! 行高固定为 [`ROW_HEIGHT`]，因此可以直接用 `uniform_list` 虚拟化：
//! 渲染成本只与**视口内可见行数**有关，与目录总条目数无关。
//!
//! 待补：列宽拖拽、按列右键菜单。两者都需要命中测试与拖动状态，
//! 放在后续「交互」阶段一并处理。

use std::rc::Rc;

use fm_core::{Entry, SortDirection, SortKey, format_size};
use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, prelude::FluentBuilder as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::{Colors, OpenHandler};

/// 数据行高。
pub const ROW_HEIGHT: f32 = 32.0;

const CELL_PADDING: f32 = 12.0;
const COLUMN_MODIFIED: f32 = 150.0;
const COLUMN_KIND: f32 = 96.0;
const COLUMN_SIZE: f32 = 96.0;

/// 渲染表头。点击任一列名会以该列为排序键回调 `on_sort`。
pub fn header<F>(
    sort_key: SortKey,
    direction: SortDirection,
    colors: Colors,
    on_sort: F,
) -> AnyElement
where
    F: Fn(SortKey, &mut Window, &mut App) + 'static,
{
    let on_sort = Rc::new(on_sort);

    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .w_full()
        .h(px(ROW_HEIGHT))
        .bg(colors.muted)
        .border_b_1()
        .border_color(colors.border)
        .text_xs()
        .text_color(colors.muted_foreground)
        .child(header_cell("名称", SortKey::Name, None, sort_key, direction, colors, Rc::clone(&on_sort)))
        .child(header_cell(
            "修改日期",
            SortKey::Modified,
            Some(COLUMN_MODIFIED),
            sort_key,
            direction,
            colors,
            Rc::clone(&on_sort),
        ))
        .child(header_cell(
            "类型",
            SortKey::Kind,
            Some(COLUMN_KIND),
            sort_key,
            direction,
            colors,
            Rc::clone(&on_sort),
        ))
        .child(header_cell(
            "大小",
            SortKey::Size,
            Some(COLUMN_SIZE),
            sort_key,
            direction,
            colors,
            Rc::clone(&on_sort),
        ))
        .into_any_element()
}

/// 渲染一行数据。
///
/// `kind_label` 与 `modified` 由调用方预备好：前者需要扩展名驻留表，
/// 后者需要平台层的时区换算——两者都不该由视图层自己去取。
///
/// `index` 用作元素 id：虚拟化列表每帧都会重建行，id 必须在这两帧之间保持稳定，
/// 否则点击命中会错位。
pub fn row(
    entry: &Entry,
    kind_label: &str,
    modified: Option<&str>,
    selected: bool,
    colors: Colors,
    index: usize,
    open: &OpenHandler,
) -> AnyElement {
    // 目录的「大小」没有意义（系统给出的目录 metadata.len() 与内容无关），显示为破折号。
    let size = if entry.is_dir() {
        String::from("—")
    } else {
        format_size(entry.size)
    };
    let modified = modified.unwrap_or("—");

    let opened = Rc::clone(open);
    let target = entry.clone();

    div()
        .id(("details-row", index))
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .w_full()
        .h(px(ROW_HEIGHT))
        .text_sm()
        .text_color(colors.foreground)
        .border_b_1()
        .border_color(colors.border)
        .when(entry.is_dir(), |element| element.cursor_pointer())
        .on_click(move |event: &ClickEvent, window, cx| {
            // 单击与双击走同一个处理器，必须用 click_count 区分：
            // 只有双击才是「打开」。单击留给后续的选中实现。
            if event.click_count() == 2 {
                (*opened)(target.clone(), window, cx);
            }
        })
        .when(selected, |element| {
            element
                .bg(colors.accent)
                .text_color(colors.accent_foreground)
        })
        .child(cell(entry.name.to_string(), None))
        .child(cell(modified.to_string(), Some(COLUMN_MODIFIED)))
        .child(cell(kind_label.to_string(), Some(COLUMN_KIND)))
        .child(cell(size, Some(COLUMN_SIZE)))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn header_cell<F>(
    label: &'static str,
    key: SortKey,
    width: Option<f32>,
    active_key: SortKey,
    direction: SortDirection,
    colors: Colors,
    on_sort: Rc<F>,
) -> AnyElement
where
    F: Fn(SortKey, &mut Window, &mut App) + 'static,
{
    let is_active = key == active_key;
    let arrow = is_active.then(|| {
        let icon = if direction == SortDirection::Ascending {
            IconName::ArrowUp
        } else {
            IconName::ArrowDown
        };
        Icon::new(icon).small()
    });

    // 用 SortKey 的判别式做元素 id：同一列在不同帧之间保持一致，点击才稳定。
    let base = div()
        .id(("th", key as usize))
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_none()
        .h_full()
        .px(px(CELL_PADDING))
        .cursor_pointer()
        .on_click(move |_event: &ClickEvent, window, cx| {
            (*on_sort)(key, window, cx);
        })
        .when(is_active, |element| {
            element
                .text_color(colors.accent)
                .child(Icon::new(IconName::ArrowDown).small())
        })
        .child(label)
        .children(arrow);

    match width {
        Some(width) => base.w(px(width)).into_any_element(),
        None => base.flex_grow(1.0).min_w(px(0.0)).into_any_element(),
    }
}

fn cell(text: String, width: Option<f32>) -> AnyElement {
    let base = div()
        .flex_none()
        .px(px(CELL_PADDING))
        .overflow_hidden()
        .child(text);

    match width {
        Some(width) => base.w(px(width)).into_any_element(),
        None => base.flex_grow(1.0).min_w(px(0.0)).into_any_element(),
    }
}
