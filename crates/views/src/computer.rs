//! 「此电脑」视图：驱动器总览。
//!
//! 这里刻意**不做虚拟化**：驱动器数量是个位数，为一个不到十几个元素的列表
//! 引入虚拟化只会增加复杂度，还得像图标网格那样去推算可用宽度。
//! 用 flex 自动换行即可。

use std::path::PathBuf;
use std::rc::Rc;

use fm_core::format_size;
use fm_platform::{Drive, DriveKind};
use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, prelude::FluentBuilder as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::Colors;

/// 卡片尺寸。
const CARD_WIDTH: f32 = 240.0;
const CARD_HEIGHT: f32 = 92.0;

/// 容量条的尺寸。
const METER_HEIGHT: f32 = 4.0;

/// 渲染驱动器总览。
///
/// `on_open` 在点击某张卡片时被调用，参数是该驱动器的挂载点（例如 `C:\`）。
pub fn drives_overview<F>(drives: &[Drive], colors: Colors, on_open: F) -> AnyElement
where
    F: Fn(PathBuf, &mut Window, &mut App) + 'static,
{
    // 与面包屑同样的手法：每张卡片都需要独立的 'static 闭包，
    // 用 Rc 共享回调，避免要求 F: Clone。
    let on_open = Rc::new(on_open);

    let cards = drives.iter().enumerate().map(|(index, drive)| {
        let callback = Rc::clone(&on_open);
        let target = drive.mount_point.clone();
        card(drive, index, colors).on_click(move |_event: &ClickEvent, window, cx| {
            (*callback)(target.clone(), window, cx);
        })
    });

    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap_3()
        .p_3()
        .w_full()
        .children(cards)
        .into_any_element()
}

/// 单张驱动器卡片。
///
/// `index` 用作元素 id。**不要**用挂载点长度之类的属性当 id：
/// `C:\` 与 `D:\` 长度相同，会撞成同一个 id，命中测试随即错乱。
fn card(drive: &Drive, index: usize, colors: Colors) -> gpui::Stateful<gpui::Div> {
    // 容量为 0 的设备（空光驱、未插卡的读卡器）不画容量条：
    // 一条恒为空的进度条比没有进度条更容易让人误判。
    let has_capacity = drive.total_bytes > 0;
    let used_ratio = drive.used_ratio();
    let caption: gpui::SharedString = if has_capacity {
        format!(
            "剩余 {} / 共 {} ({:.0}%)",
            format_size(drive.free_bytes),
            format_size(drive.total_bytes),
            used_ratio * 100.0
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
        .p_2()
        .gap_1()
        .rounded(px(8.0))
        .bg(colors.accent)
        .border_1()
        .border_color(colors.border)
        .cursor_pointer()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(Icon::new(kind_icon(drive.kind)).large())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_grow(1.0)
                        .min_w(px(0.0))
                        .text_sm()
                        .child(labels(drive)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .when(has_capacity, |element| {
                    element.child(
                        div()
                            .w_full()
                            .h(px(METER_HEIGHT))
                            .rounded(px(2.0))
                            .bg(colors.muted)
                            .child(
                                div()
                                    .h_full()
                                    .rounded(px(2.0))
                                    .bg(colors.accent_foreground)
                                    .w(px(CARD_WIDTH * used_ratio)),
                            ),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.muted_foreground)
                        .child(caption),
                ),
        )
}

/// 驱动器展示名，例如「本地磁盘 (C:)」。
fn labels(drive: &Drive) -> gpui::SharedString {
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
