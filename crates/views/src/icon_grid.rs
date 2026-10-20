//! 图标网格视图（虚拟化）。
//!
//! 网格的虚拟化单位是**行**而不是单元格：`uniform_list` 要求每项高度一致，
//! 而一行图标的高度天然固定（见 [`row_height`]），宽度则随窗口变化。
//! 因此行数由「当前列数」决定，窗口尺寸变化会改变行数——
//! 这也是滚动位置必须按行而不是按条目保存的原因。
//!
//! 单元格形态照搬设计稿的 `.cell`：1:1 缩略图底板 + 两行名称 + 等宽小字元信息；
//! 悬停浅灰底、选中强调色浅底加描边，与详情视图共用同一套语义。

use gpui::{
    AnyElement, ClickEvent, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::input::InputState;
use gpui_component::{Icon, Sizable as _};

use fm_core::Entry;

use crate::tokens::{Tokens, metrics};
use crate::{DragSelectHandler, EntryMenuBuilder, OpenHandler, RightPressHandler, SelectHandler};

/// 单元格内名称区的高度：两行 12px / 1.35 行高。
///
/// 定死而不是让它随内容伸缩：网格行高必须全表一致，一旦某行名称只有一行，
/// `uniform_list` 的滚动定位就会整体偏移。
const NAME_HEIGHT: f32 = 33.0;
/// 元信息行（大小）高度。
const META_HEIGHT: f32 = 15.0;
/// 缩略图与名称之间的间距。
const NAME_GAP: f32 = 8.0;
/// 名称与元信息之间的间距。
const META_GAP: f32 = 6.0;

/// 根据可用宽度算出每行能放几个图标。
///
/// 与设计稿的 `repeat(auto-fill, minmax(var(--cell), 1fr))` 等价：
/// 先按「单元格 + 间距」整除去重，再把剩余宽度摊到各列。
///
/// 至少返回 1：窗口极窄时若算出 0 列，行数会变成无穷大，
/// 下游按行取模时会直接除零崩溃。
pub fn columns_for_width(available_width: f32, cell: f32) -> usize {
    let step = cell + metrics::GRID_GAP;
    (((available_width + metrics::GRID_GAP) / step).floor() as usize).max(1)
}

/// 把剩余宽度摊到各列后，单个单元格的实际宽度。
///
/// 设计稿用的是 `1fr`，因此单元格会等比拉伸填满整行（而不是在右侧留一条空白）。
pub fn cell_width(available_width: f32, columns: usize, cell: f32) -> f32 {
    let columns = columns.max(1);
    let gaps = metrics::GRID_GAP * (columns as f32 - 1.0);
    let width = (available_width - gaps) / columns as f32;
    // 极端窄窗下可能算出小于 0 的宽度，兜到设计稿的最小档位。
    if width.is_finite() {
        width.max(cell * 0.5)
    } else {
        cell
    }
}

/// 图标网格的总行数。
pub fn row_count(entry_count: usize, columns: usize) -> usize {
    let columns = columns.max(1);
    entry_count.div_ceil(columns)
}

/// 单元格高度（含内边距、缩略图与两行文字）。
pub fn cell_height(cell_width: f32) -> f32 {
    let thumb = (cell_width - metrics::CELL_PADDING * 2.0).max(16.0);
    metrics::CELL_PADDING * 2.0 + thumb + NAME_GAP + NAME_HEIGHT + META_GAP + META_HEIGHT
}

/// 一行的高度。纵向间距折进行高，这样滚动容器不需要额外的 `gap` 支持。
pub fn row_height(cell_width: f32) -> f32 {
    cell_height(cell_width) + metrics::GRID_GAP
}

/// 渲染一行图标。
///
/// `extensions` 由调用方经扩展名驻留表解析后传入，用于挑选文件类型图标；
/// `selected` 让调用方按当前选中集判断每个单元格的状态，避免视图层持有选中模型。
///
/// `editing` 是「某个条目正在被内联重命名」时的 `(下标, 输入框)`：
/// 它必须带下标，因为网格的一行里有多个单元格，只有命中下标的那一个
/// 才把名称换成输入框。输入框实体由 shell 持有——视图每帧重建单元格，
/// 在这里 `cx.new` 会让每帧都得到一个全新的输入框。
#[allow(clippy::too_many_arguments)]
pub fn row(
    entries: &[Entry],
    start: usize,
    columns: usize,
    cell_width: f32,
    extensions: impl Fn(usize) -> Option<String>,
    selected: impl Fn(usize) -> bool,
    cursor: Option<usize>,
    editing: Option<(usize, &Entity<InputState>)>,
    tokens: &Tokens,
    open: &OpenHandler,
    select: &SelectHandler,
    menu: &EntryMenuBuilder,
    right_press: &RightPressHandler,
    drag_start: &DragSelectHandler,
    drag_move: &DragSelectHandler,
) -> AnyElement {
    let height = cell_height(cell_width);

    let cells = (0..columns).map(|offset| {
        let index = start + offset;
        // `Option<(usize, &Entity<_>)>` 是 `Copy` 的，因此可以在闭包里按值用，
        // 不需要为每个单元格克隆输入框的引用。
        let editing = editing
            .filter(|(target, _)| *target == index)
            .map(|(_, state)| state);
        match entries.get(index) {
            Some(entry) => cell(
                entry,
                extensions(index).as_deref(),
                cell_width,
                height,
                selected(index),
                cursor == Some(index),
                editing,
                index,
                tokens,
                open,
                select,
                menu,
                right_press,
                drag_start,
                drag_move,
            ),
            // 末行不足时补等高空占位：行高不一致会让 uniform_list 的滚动计算失准。
            None => div()
                .flex_none()
                .w(px(cell_width))
                .h(px(height))
                .into_any_element(),
        }
    });

    div()
        .flex()
        .flex_row()
        .gap(px(metrics::GRID_GAP))
        .flex_none()
        .w_full()
        .h(px(row_height(cell_width)))
        .children(cells)
        .into_any_element()
}

/// 单个单元格。
///
/// `index` 用作元素 id：虚拟化列表每帧重建整行，id 必须在帧间保持稳定，
/// 否则点击命中会错位到别的条目上。
#[allow(clippy::too_many_arguments)]
fn cell(
    entry: &Entry,
    extension: Option<&str>,
    cell_width: f32,
    height: f32,
    selected: bool,
    current: bool,
    editing: Option<&Entity<InputState>>,
    index: usize,
    tokens: &Tokens,
    open: &OpenHandler,
    select: &SelectHandler,
    menu: &EntryMenuBuilder,
    right_press: &RightPressHandler,
    drag_start: &DragSelectHandler,
    drag_move: &DragSelectHandler,
) -> AnyElement {
    let icon = crate::widgets::entry_icon(entry, extension);
    let thumb_size = (cell_width - metrics::CELL_PADDING * 2.0).max(16.0);
    let hover = tokens.hover;
    let accent_soft = tokens.accent_soft;
    let accent_line = tokens.accent_line;
    let border = tokens.border;

    let opened = std::rc::Rc::clone(open);
    let target = entry.clone();
    let selecting = std::rc::Rc::clone(select);
    let drag_start = std::rc::Rc::clone(drag_start);
    let drag_move = std::rc::Rc::clone(drag_move);
    let selected_hover = crate::tokens::with_alpha(tokens.fg, 0.1);

    let cell = div()
        .id(("icon-cell", index))
        .flex()
        .flex_col()
        .gap(px(NAME_GAP))
        .flex_none()
        .w(px(cell_width))
        .h(px(height))
        .p(px(metrics::CELL_PADDING))
        .rounded(px(metrics::RADIUS_LG))
        .border_1()
        .border_color(tokens.surface)
        .cursor_pointer()
        // 与详情行同理：`hover()` 每个元素只能设置一次，否则 dev 构建下 panic。
        .hover(move |style| {
            if selected {
                style.bg(selected_hover).border_color(accent_line)
            } else {
                style.bg(hover).border_color(border)
            }
        })
        .when(selected, |element| {
            element.bg(accent_soft).border_color(accent_line)
        })
        .when(current, |element| {
            element.border_color(accent_line)
        })
        // 框选：与详情视图同理——左键按下记锚点，滑过每个单元格按区间替换选中。
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            (*drag_start)(index, event.position, window, cx);
        })
        .on_mouse_move(move |event, window, cx| {
            (*drag_move)(index, event.position, window, cx);
        })
        .when(editing.is_none(), |element| element.cursor_pointer())
        // 编辑态下单元格不再响应点击：点击会**冒泡**到单元格上，
        // 改变选中集或触发「打开」——在改名的过程中把文件打开是最糟的结果。
        .when(editing.is_none(), |element| {
            element.on_click(move |event: &ClickEvent, window, cx| {
                // 单击与双击共用同一个处理器，必须用 click_count 区分。
                if event.click_count() == 2 {
                    (*opened)(target.clone(), window, cx);
                    return;
                }
                (*selecting)(index, crate::select_mode(event), window, cx);
            })
        })
        .child(
            // 缩略图底板：与设计稿一致地使用 panel 底色，让文件图标在两种主题下
            // 都有稳定的对比度；比内容区略深/略浅正是它要的「凹槽」效果。
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .w(px(thumb_size))
                .h(px(thumb_size))
                .rounded(px(metrics::RADIUS))
                .bg(tokens.panel)
                .border_1()
                .border_color(border)
                .overflow_hidden()
                .child(
                    Icon::new(icon)
                        .with_size(px(thumb_size * 0.44))
                        .text_color(if entry.is_dir() { tokens.accent } else { tokens.muted_fg }),
                ),
        )
        .child(match editing {
            // 编辑态：名称区改放输入框。外层补一层 flex_row 是为了让
            // `rename_input` 的 `flex_grow` 有参照物——否则它会按内容宽度收缩。
            Some(state) => div()
                .flex_none()
                .w_full()
                .h(px(NAME_HEIGHT))
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .child(crate::widgets::rename_input(state, tokens))
                .into_any_element(),
            None => div()
                .flex_none()
                .w_full()
                .h(px(NAME_HEIGHT))
                .overflow_hidden()
                .line_clamp(2)
                .text_size(px(metrics::FONT_SM))
                .text_color(tokens.fg)
                .child(entry.name.to_string())
                .into_any_element(),
        })
        .child(
            div()
                .flex_none()
                .w_full()
                .h(px(META_HEIGHT))
                .overflow_hidden()
                .font_family(tokens.mono_font.clone())
                .text_size(px(metrics::FONT_XS))
                .text_color(tokens.muted_fg)
                .child(meta_label(entry)),
        );

    // 包装层不能写 `w_full()`：网格行是 `flex_row`，写满了每个单元格都会占满整行宽度。
    // 由内容（单元格自己的固定宽高）撑开，正是原来单元格作为直接子项时的行为。
    let wrapper = div()
        .id(("icon-cell-wrap", index))
        .flex()
        .flex_row()
        .flex_none();

    crate::context_menu::entry_menu_row(wrapper, cell, entry, index, menu, right_press)
}

/// 单元格下方的元信息。
///
/// 设计稿里这里放的是大小；文件夹没有有意义的大小，因此改显示「文件夹」，
/// 比显示一个破折号多给出一点信息量。
fn meta_label(entry: &Entry) -> String {
    if entry.is_dir() {
        return String::from("文件夹");
    }
    fm_core::format_size(entry.size)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: f32 = 152.0;

    #[test]
    fn columns_never_drop_to_zero() {
        assert_eq!(columns_for_width(0.0, CELL), 1);
        assert_eq!(columns_for_width(50.0, CELL), 1);
        assert_eq!(columns_for_width(-10.0, CELL), 1);
    }

    #[test]
    fn columns_match_the_design_auto_fill_rule() {
        // 单元格 152、间距 10：宽 152 恰好一列，宽 314 恰好两列。
        assert_eq!(columns_for_width(152.0, CELL), 1);
        assert_eq!(columns_for_width(314.0, CELL), 2);
        assert_eq!(columns_for_width(313.0, CELL), 1);
    }

    #[test]
    fn cells_stretch_to_fill_the_row() {
        // 两列时可用宽度全部被吃掉：152*2 + 10 = 314。
        let width = cell_width(314.0, 2, CELL);
        assert!((width - 152.0).abs() < 0.01);
        // 多余宽度按列均分，右侧不会留下空白。
        let width = cell_width(400.0, 2, CELL);
        assert!((width - 195.0).abs() < 0.01);
    }

    #[test]
    fn row_count_rounds_up_and_tolerates_zero_columns() {
        assert_eq!(row_count(0, 8), 0);
        assert_eq!(row_count(1, 8), 1);
        assert_eq!(row_count(8, 8), 1);
        assert_eq!(row_count(9, 8), 2);
        // 防御性：即便调用方误传 0，也不能除零。
        assert_eq!(row_count(5, 0), 5);
    }

    #[test]
    fn row_height_is_derived_from_the_cell_width() {
        // 行高必须随单元格宽度增长，否则大缩略图会被裁掉。
        assert!(row_height(192.0) > row_height(152.0));
        assert!(row_height(152.0) > row_height(122.0));
        // 行高必须严格大于单元格高度，否则相邻两行会贴在一起。
        assert!(row_height(152.0) > cell_height(152.0));
    }
}
