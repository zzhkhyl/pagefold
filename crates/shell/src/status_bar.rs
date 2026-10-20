//! 底部状态栏：左（条目数 / 合计大小 / 当前位置）、右（视图名 / 缩略图缩放）。
//!
//! 形态照搬设计稿的 `.statusbar`：26px 高、panel 底色、1px 顶边，
//! 全部文本走等宽字体——这一行是数字密度最高的地方，等宽才能让数值不跳动。

use std::rc::Rc;

use fm_core::{Entry, IconZoom, Location, ViewMode, format_size};
use fm_views::metrics;
use gpui::{
    AnyElement, App, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, prelude::FluentBuilder as _,
};

use crate::layout::Viewport;
use crate::shell::ShellView;

/// 点击某一档缩略图缩放时的回调。
type ZoomHandler = Rc<dyn Fn(IconZoom, &mut Window, &mut App)>;

impl ShellView {
    /// 渲染底部状态栏。
    pub(crate) fn render_status_bar(&self, viewport: Viewport, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        let icons_view = self.tabs.active_state().view_mode() == ViewMode::Icons;

        // 「此电脑」展示的是驱动器而不是文件条目，统计口径完全不同。
        // 不分开算的话会显示成「共 0 项 · 0 字节」，看起来像出了故障。
        // 设置页同理：它不是文件夹，条目统计在那里没有意义。
        let left: String = match self.tabs.active_state().location() {
            Location::Computer => format!("{} 个驱动器 · 此电脑", self.drives.len()),
            Location::Settings => String::from("外观与偏好 · 设置"),
            _ => {
                let (count, total_bytes) = summarize(self.visible());
                let mut text = format!("{count} 个项目 · 共 {}", format_size(total_bytes));
                if self.loading {
                    // 扫描期间给出「已到达多少」比一个转圈更有信息量。
                    text.push_str(&format!(" · 正在读取（已加载 {count} 项）"));
                }
                if !self.selection.is_empty() {
                    text.push_str(&format!(
                        " · 已选 {} 项 · {}",
                        self.selection.len(),
                        format_size(self.selection_size())
                    ));
                }
                text.push_str(" · ");
                text.push_str(&self.location_label());
                text
            }
        };

        let mut bar = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .w_full()
            .h(px(metrics::STATUS_BAR_HEIGHT))
            .px(px(metrics::ROW_PADDING_X))
            .bg(tokens.panel)
            .border_t_1()
            .border_color(tokens.border)
            .font_family(tokens.mono_font.clone())
            .text_size(px(metrics::FONT_XS))
            .text_color(tokens.muted_fg)
            .child(
                div()
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .truncate()
                    .child(left),
            );

        // 窄窗下这两组控件先让位：条目数与当前位置比它们重要。
        if !viewport.hide_status_extras {
            let on_zoom: ZoomHandler = {
                let view = cx.entity().downgrade();
                Rc::new(move |zoom: IconZoom, _window: &mut Window, cx: &mut App| {
                    if view.update(cx, |view, cx| view.set_zoom(zoom, cx)).is_err() {
                        tracing::debug!("调整缩放时视图已销毁，本次操作被忽略");
                    }
                })
            };

            bar = bar
                .child(
                    div()
                        .flex_none()
                        .text_color(tokens.muted_fg)
                        .child(if icons_view {
                            "图标视图"
                        } else {
                            "详细信息视图"
                        }),
                )
                .child(zoom_control(self.config.zoom, tokens, on_zoom));
        }

        bar.into_any_element()
    }
}

/// 缩略图缩放控件。
///
/// 设计稿用的是一个三档 `input[type=range]`。这里改成三个可点的小格：
/// 三档是离散值，用滑块既表达不出「只能停在三档」，也要额外维护拖动状态；
/// 点选小格同样是三档调节，命中更准，也不需要一套拖动状态。
fn zoom_control(
    current: IconZoom,
    tokens: &fm_views::Tokens,
    on_pick: ZoomHandler,
) -> AnyElement {
    let hover = tokens.hover;
    let accent = tokens.accent;
    let border = tokens.border;

    let steps = IconZoom::ALL.iter().map(|zoom| {
        let zoom = *zoom;
        let active = zoom == current;
        let pick = Rc::clone(&on_pick);
        div()
            .id(("zoom-step", zoom.index()))
            .flex_none()
            .w(px(16.0))
            .h(px(14.0))
            .rounded(px(3.0))
            .border_1()
            .border_color(if active { accent } else { border })
            .when(active, |element| element.bg(accent))
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(move |_event: &ClickEvent, window, cx| pick(zoom, window, cx))
            .into_any_element()
    });

    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .child(div().flex_none().child("缩放"))
        .children(steps)
        .into_any_element()
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
