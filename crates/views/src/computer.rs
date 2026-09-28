//! 「此电脑」视图：驱动器总览。
//!
//! 设计稿没有为「此电脑」单独出稿（它的示例数据把驱动器也当成文件夹列在表里），
//! 因此这里沿用设计稿的卡片语言重新组织：表面色底 + 1px 描边 + 10px 圆角，
//! 悬停时描边转为强调色，容量条与侧边栏的磁盘档位保持同一套形态。
//!
//! 这里刻意**不做虚拟化**：驱动器数量是个位数，为一个不到十几个元素的列表
//! 引入虚拟化只会增加复杂度，还得像图标网格那样去推算可用宽度。用 flex 自动换行即可。

use std::path::PathBuf;
use std::rc::Rc;

use fm_core::format_size;
use fm_platform::{Drive, DriveKind};
use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::tokens::{Tokens, metrics};
use crate::widgets;

/// 卡片尺寸。
const CARD_WIDTH: f32 = 240.0;
const CARD_HEIGHT: f32 = 92.0;
/// 卡片内边距。
const CARD_PADDING: f32 = 12.0;

/// 渲染驱动器总览。
///
/// `on_open` 在点击某张卡片时被调用，参数是该驱动器的挂载点（例如 `C:\`）。
pub fn drives_overview<F>(drives: &[Drive], tokens: &Tokens, on_open: F) -> AnyElement
where
    F: Fn(PathBuf, &mut Window, &mut App) + 'static,
{
    // 与面包屑同样的手法：每张卡片都需要独立的 'static 闭包，
    // 用 Rc 共享回调，避免要求 F: Clone。
    let on_open = Rc::new(on_open);
    let accent = tokens.accent;
    let border = tokens.border;
    let muted_foreground = tokens.muted_fg;
    let foreground = tokens.fg;
    let surface = tokens.surface;
    let hover = tokens.hover;

    let cards = drives.iter().enumerate().map(|(index, drive)| {
        let callback = Rc::clone(&on_open);
        let target = drive.mount_point.clone();
        card(drive, index, tokens, accent, border, foreground)
            .hover(move |style| style.bg(hover).border_color(accent))
            .on_click(move |_event: &ClickEvent, window, cx| {
                (*callback)(target.clone(), window, cx);
            })
    });

    div()
        .id("computer-overview")
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .min_h(px(0.0))
        .overflow_y_scroll()
        .child(
            // 分区标题：与详情视图的表头同一套等宽小字，让「此电脑」看起来仍是一个列表页。
            div()
                .flex_none()
                .h(px(metrics::LIST_HEAD_HEIGHT))
                .px(px(metrics::ROW_PADDING_X))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(border)
                .bg(surface)
                .font_family(tokens.mono_font.clone())
                .text_size(px(metrics::FONT_XS))
                .text_color(muted_foreground)
                .child("设备与驱动器"),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(px(12.0))
                .p(px(metrics::ROW_PADDING_X))
                .w_full()
                .children(cards),
        )
        .into_any_element()
}

/// 单张驱动器卡片。
///
/// `index` 用作元素 id。**不要**用挂载点长度之类的属性当 id：
/// `C:\` 与 `D:\` 长度相同，会撞成同一个 id，命中测试随即错乱。
fn card(
    drive: &Drive,
    index: usize,
    tokens: &Tokens,
    accent: gpui::Hsla,
    border: gpui::Hsla,
    foreground: gpui::Hsla,
) -> gpui::Stateful<gpui::Div> {
    // 容量为 0 的设备（空光驱、未插卡的读卡器）不画容量条：
    // 一条恒为空的进度条比没有进度条更容易让人误判。
    let has_capacity = drive.total_bytes > 0;
    let used_ratio = drive.used_ratio();
    let percent = (used_ratio * 100.0).round();
    let caption: SharedString = if has_capacity {
        format!(
            "剩余 {} / 共 {}",
            format_size(drive.free_bytes),
            format_size(drive.total_bytes)
        )
        .into()
    } else {
        "容量未知".into()
    };

    div()
        .id(("drive-card", index))
        .flex()
        .flex_col()
        .flex_none()
        .w(px(CARD_WIDTH))
        .h(px(CARD_HEIGHT))
        .p(px(CARD_PADDING))
        .gap(px(6.0))
        .rounded(px(metrics::RADIUS_LG))
        .bg(tokens.surface)
        .border_1()
        .border_color(border)
        .cursor_pointer()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    Icon::new(kind_icon(drive.kind))
                        .with_size(px(20.0))
                        .text_color(accent),
                )
                .child(
                    div()
                        .flex_grow(1.0)
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .truncate()
                        .text_size(px(12.5))
                        .text_color(foreground)
                        .child(labels(drive)),
                )
                .when(has_capacity, |element| {
                    element.child(widgets::mono(
                        format!("{percent:.0}%"),
                        metrics::FONT_XS,
                        tokens.muted_fg,
                        tokens,
                    ))
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .when(has_capacity, |element| {
                    element.child(widgets::capacity_bar(
                        used_ratio,
                        CARD_WIDTH - CARD_PADDING * 2.0,
                        metrics::DISK_METER_HEIGHT,
                        tokens,
                    ))
                })
                .child(
                    div()
                        .font_family(tokens.mono_font.clone())
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.muted_fg)
                        .child(caption),
                ),
        )
}

/// 驱动器展示名，例如「本地磁盘 (C:)」。
fn labels(drive: &Drive) -> SharedString {
    format!("{} ({})", kind_label(drive.kind), drive.name).into()
}

fn kind_label(kind: DriveKind) -> &'static str {
    match kind {
        DriveKind::Fixed => "本地磁盘",
        DriveKind::Removable => "可移动磁盘",
        DriveKind::Network => "网络驱动器",
        DriveKind::Optical => "光驱",
        DriveKind::Other => "驱动器",
    }
}

fn kind_icon(kind: DriveKind) -> IconName {
    match kind {
        DriveKind::Optical => IconName::Disc,
        DriveKind::Network => IconName::Network,
        DriveKind::Removable => IconName::Usb,
        DriveKind::Fixed | DriveKind::Other => IconName::HardDrive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(kind: DriveKind, total: u64, free: u64) -> Drive {
        Drive {
            name: "C:".into(),
            mount_point: PathBuf::from(r"C:\"),
            kind,
            total_bytes: total,
            free_bytes: free,
        }
    }

    #[test]
    fn kind_labels_cover_every_drive_kind() {
        // 穷尽匹配由编译器保证；这里只固定文案，避免将来被误改成空串。
        for kind in [
            DriveKind::Fixed,
            DriveKind::Removable,
            DriveKind::Network,
            DriveKind::Optical,
            DriveKind::Other,
        ] {
            assert!(!kind_label(kind).is_empty());
        }
    }

    #[test]
    fn labels_combine_kind_and_drive_letter() {
        assert_eq!(&*labels(&drive(DriveKind::Fixed, 100, 50)), "本地磁盘 (C:)");
    }

    #[test]
    fn capacity_is_zero_for_unready_devices() {
        // 空光驱：容量为 0，卡片应走「容量未知」分支而不是画出 0% 的空条。
        let optical = drive(DriveKind::Optical, 0, 0);
        assert_eq!(optical.used_ratio(), 0.0);
    }
}
