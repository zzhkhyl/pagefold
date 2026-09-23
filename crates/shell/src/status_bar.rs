//! 底部状态栏：左（条目数 / 合计大小）、中（扫描进度与已加载数）、右（视图与缩放）。

use fm_core::{Entry, Location, ViewMode, format_size};
use gpui::{
    AnyElement, ClickEvent, Context, IntoElement, ParentElement as _, Styled as _, Window, div, px,
};
use gpui_component::ActiveTheme as _;
use gpui_kit_assets::IconName;

use crate::shell::ShellView;
use crate::toolbar::toggle_button;

/// 状态栏高度。
const STATUS_BAR_HEIGHT: f32 = 28.0;

/// 进度细条的宽度与高度。
const PROGRESS_WIDTH: f32 = 96.0;
const PROGRESS_HEIGHT: f32 = 3.0;

impl ShellView {
    /// 渲染底部状态栏。
    pub(crate) fn render_status_bar(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let muted_foreground = theme.muted_foreground;
        let accent = theme.accent;
        let accent_foreground = theme.accent_foreground;
        let track = theme.muted;

        // 「此电脑」展示的是驱动器而不是文件条目，统计口径完全不同。
        // 不分开算的话会显示成「共 0 项 · 0 字节」，看起来像出了故障。
        let (left, middle) = if matches!(
            self.tabs.active_state().location(),
            Location::Computer
        ) {
            let left = format!("{} 个驱动器", self.drives.len());
            let middle = if self.loading {
                String::from("正在枚举驱动器…")
            } else {
                String::from("驱动器已就绪")
            };
            (left, middle)
        } else {
            let (count, total_bytes) = summarize(&self.entries);
            let loaded = self.entries.len();
            let middle = if self.loading {
                format!("正在读取… 已加载 {loaded} 项")
            } else {
                format!("已加载 {loaded} 项")
            };
            (format!("共 {count} 项 · {}", format_size(total_bytes)), middle)
        };

        let icons_view = self.tabs.active_state().view_mode() == ViewMode::Icons;

        let progress = div()
            .flex_none()
            .w(px(PROGRESS_WIDTH))
            .h(px(PROGRESS_HEIGHT))
            .rounded(px(2.0))
            .bg(track)
            .child(
                div()
                    .h_full()
                    .rounded(px(2.0))
                    .bg(accent)
                    // 扫描期间用一条固定宽度的前景条表达「在动」；
                    // 由于目录总数事先未知，这里刻意不做百分比，避免给出错误信息。
                    .w(px(if self.loading { PROGRESS_WIDTH * 0.35 } else { 0.0 })),
            );

        let view_icons = toggle_button(
            "status-view-icons",
            IconName::LayoutGrid,
            "图标视图",
            icons_view,
            accent,
            accent_foreground,
            muted_foreground,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.set_view_mode(ViewMode::Icons, cx);
            }),
        );
        let view_details = toggle_button(
            "status-view-details",
            IconName::List,
            "详细信息视图",
            !icons_view,
            accent,
            accent_foreground,
            muted_foreground,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.set_view_mode(ViewMode::Details, cx);
            }),
        );

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .flex_none()
            .w_full()
            .h(px(STATUS_BAR_HEIGHT))
            .px_3()
            .border_t_1()
            .border_color(border)
            .text_xs()
            .text_color(muted_foreground)
            .child(div().flex_none().child(left))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(progress)
                    .child(middle),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(view_icons)
                    .child(view_details)
                    // 图标缩放滑块待动效任务接入，先如实标注当前档位。
                    .child(div().flex_none().child("缩放 100%")),
            )
            .into_any_element()
    }
}

/// 统计条目数与文件总大小。
///
/// **只累加非目录条目**：Windows 上目录的 `metadata().len()` 并不代表其内容大小
/// （通常是一个与内容无关的固定值），把它算进合计会得到明显错误的数字。
fn summarize(entries: &[Entry]) -> (usize, u64) {
    let mut total = 0u64;
    for entry in entries {
        if !entry.is_dir() {
            total = total.saturating_add(entry.size);
        }
    }
    (entries.len(), total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::{EntryKind, IconId};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn entry(kind: EntryKind, size: u64) -> Entry {
        Entry {
            name: Arc::from("x"),
            path: Arc::new(PathBuf::from("x")),
            kind,
            size,
            modified: None,
            icon: IconId(0),
        }
    }

    #[test]
    fn directories_are_not_counted_into_the_total_size() {
        let entries = vec![
            entry(EntryKind::Directory, 4096),
            entry(EntryKind::File { ext: None }, 1024),
            entry(EntryKind::File { ext: None }, 2048),
        ];

        let (count, total) = summarize(&entries);
        assert_eq!(count, 3, "条目数应包含目录");
        assert_eq!(total, 3072, "合计大小不应包含目录");
    }

    #[test]
    fn empty_list_summarizes_to_zero() {
        assert_eq!(summarize(&[]), (0, 0));
    }
}
