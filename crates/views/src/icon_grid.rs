//! 图标网格视图（虚拟化）。
//!
//! 网格的虚拟化单位是**行**而不是单元格：`uniform_list` 要求每项高度一致，
//! 而一行图标的高度天然固定（[`CELL_HEIGHT`]），宽度则随窗口变化。
//! 因此行数由「当前列数」决定，窗口尺寸变化会改变行数——
//! 这也是滚动位置必须按行而不是按条目保存的原因。

use std::rc::Rc;

use fm_core::Entry;
use gpui::{
    AnyElement, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, px, prelude::FluentBuilder as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::{Colors, OpenHandler};

/// 单元格宽度（PLAN.md：每项 120x120）。
pub const CELL_WIDTH: f32 = 120.0;
/// 单元格高度，同时也是 `uniform_list` 的行高。
pub const CELL_HEIGHT: f32 = 120.0;

/// 根据可用宽度算出每行能放几个图标。
///
/// 至少返回 1：窗口极窄时若算出 0 列，行数会变成无穷大，
/// 下游按行取模时会直接除零崩溃。
pub fn columns_for_width(available_width: f32) -> usize {
    ((available_width / CELL_WIDTH).floor() as usize).max(1)
}

/// 图标网格的总行数。
pub fn row_count(entry_count: usize, columns: usize) -> usize {
    let columns = columns.max(1);
    entry_count.div_ceil(columns)
}

/// 渲染一行图标。
///
/// `extensions` 由调用方经扩展名驻留表解析后传入，用于挑选文件类型图标。
pub fn row(
    entries: &[Entry],
    start: usize,
    columns: usize,
    extensions: impl Fn(usize) -> Option<String>,
    colors: Colors,
    open: &OpenHandler,
) -> AnyElement {
    let cells = (0..columns).map(|offset| {
        let index = start + offset;
        match entries.get(index) {
            Some(entry) => cell(entry, extensions(index).as_deref(), colors, index, open),
            // 末行不足时补等高空占位：行高不一致会让 uniform_list 的滚动计算失准。
            None => div()
                .flex_none()
                .w(px(CELL_WIDTH))
                .h(px(CELL_HEIGHT))
                .into_any_element(),
        }
    });

    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .h(px(CELL_HEIGHT))
        .children(cells)
        .into_any_element()
}

/// 单个单元格。
///
/// `index` 用作元素 id：虚拟化列表每帧重建整行，id 必须在帧间保持稳定，
/// 否则点击命中会错位到别的条目上。
fn cell(
    entry: &Entry,
    extension: Option<&str>,
    colors: Colors,
    index: usize,
    open: &OpenHandler,
) -> AnyElement {
    let opened = Rc::clone(open);
    let target = entry.clone();

    div()
        .id(("icon-cell", index))
        .flex()
        .flex_col()
        .items_center()
        .flex_none()
        .w(px(CELL_WIDTH))
        .h(px(CELL_HEIGHT))
        .p_1()
        .rounded(px(6.0))
        .when(entry.is_dir(), |element| element.cursor_pointer())
        .on_click(move |event: &ClickEvent, window, cx| {
            // 单击与双击共用同一个处理器，必须用 click_count 区分：
            // 只有双击才是「打开」。单击留给后续的选中实现。
            if event.click_count() == 2 {
                (*opened)(target.clone(), window, cx);
            }
        })
        .child(
            // 图标区占满剩余高度并居中；文字区高度固定，避免长短文件名导致排版跳动。
            div()
                .flex()
                .flex_grow(1.0)
                .items_center()
                .justify_center()
                .child(Icon::new(icon_for(entry, extension)).large()),
        )
        .child(
            div()
                .flex_none()
                .w_full()
                .h(px(32.0))
                .overflow_hidden()
                .text_xs()
                .text_color(colors.foreground)
                .child(entry.name.to_string()),
        )
        .into_any_element()
}

/// 按扩展名挑选图标。
///
/// 这里只做粗分类（图片/音频/视频/文本），并不追求与系统图标完全一致：
/// 真正的系统图标需要 `SHGetFileInfoW`，属于平台层的后续工作。
fn icon_for(entry: &Entry, extension: Option<&str>) -> IconName {
    if entry.is_dir() {
        return IconName::Folder;
    }

    match extension {
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico") => IconName::Image,
        Some("mp3" | "wav" | "flac" | "m4a" | "ogg" | "wma") => IconName::Music,
        Some("mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm") => IconName::Video,
        _ => IconName::FileText,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_never_drop_to_zero() {
        assert_eq!(columns_for_width(0.0), 1);
        assert_eq!(columns_for_width(50.0), 1);
        assert_eq!(columns_for_width(-10.0), 1);
    }

    #[test]
    fn columns_scale_with_available_width() {
        assert_eq!(columns_for_width(120.0), 1);
        assert_eq!(columns_for_width(960.0), 8);
        // 不足一整列时向下取整，避免最后一列被裁掉一半。
        assert_eq!(columns_for_width(1000.0), 8);
    }

    #[test]
    fn row_count_rounds_up() {
        assert_eq!(row_count(0, 8), 0);
        assert_eq!(row_count(1, 8), 1);
        assert_eq!(row_count(8, 8), 1);
        assert_eq!(row_count(9, 8), 2);
    }

    #[test]
    fn row_count_tolerates_zero_columns() {
        // 防御性：即便调用方误传 0，也不能除零。
        assert_eq!(row_count(5, 0), 5);
    }
}
