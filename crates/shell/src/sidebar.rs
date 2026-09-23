//! 左侧导航栏：快速访问 / 驱动器 / 常用文件夹 三个分区。
//!
//! 视觉规范（PLAN.md）：选中项为强调色 12% 底色加左侧 3px 强调色竖条，
//! 悬停为中性灰底；分区标题为小号次级文本，分组间留 16px 间距。
//! 折叠态（48px）只保留图标，不显示文字与容量条。
//!
//! 待补：悬停高亮（需要 `hover` 样式，等双视图阶段一并处理）、
//! 常用文件夹的添加/移除（需要右键菜单，本次仅预留分区）。

use std::time::Duration;

use fm_core::{Location, format_size};
use fm_platform::{Drive, DriveKind};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, Context, ElementId, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, ease_in_out, px,
    prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::shell::ShellView;

/// 展开态宽度。
pub(crate) const SIDEBAR_WIDTH: f32 = 240.0;
/// 折叠态宽度。
pub(crate) const SIDEBAR_COLLAPSED_WIDTH: f32 = 48.0;
/// 单个导航项高度。
const ROW_HEIGHT: f32 = 30.0;
/// 驱动器剩余容量条的宽度与高度。
const METER_WIDTH: f32 = 72.0;
const METER_HEIGHT: f32 = 4.0;

impl ShellView {
    /// 渲染左侧导航栏。
    pub(crate) fn render_sidebar(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let foreground = theme.foreground;
        let muted_foreground = theme.muted_foreground;
        let accent = theme.accent;
        let track = theme.muted;
        let accent_foreground = theme.accent_foreground;

        let collapsed = self.config.sidebar_collapsed;
        let target_width = if collapsed {
            SIDEBAR_COLLAPSED_WIDTH
        } else {
            SIDEBAR_WIDTH
        };
        // 折叠动画从「另一个状态」的宽度出发，因此需要显式记住起点。
        let start_width = if collapsed {
            SIDEBAR_WIDTH
        } else {
            SIDEBAR_COLLAPSED_WIDTH
        };
        let location = self.tabs.active_state().location().clone();

        let mut rows: Vec<AnyElement> = Vec::new();

        // —— 本机（虚拟位置）——
        //
        // 放在最前面：它是「向上」的终点，也是后续 `Win+E` 接管的默认落点。
        rows.push(navigation_row(
            ("computer", 0u32),
            String::from("此电脑"),
            IconName::PcCase,
            collapsed,
            matches!(location, Location::Computer),
            accent,
            accent_foreground,
            foreground,
            muted_foreground,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.go_to_computer(cx);
            }),
        ));

        // —— 快速访问 ——
        if !collapsed {
            rows.push(section_title("快速访问", muted_foreground));
        }
        for (index, entry) in self.quick_access.iter().enumerate() {
            let icon = quick_access_icon(index);
            let path = entry.path.as_ref().clone();
            let selected = location.is_dir(&path);
            rows.push(navigation_row(
                ("quick", index),
                entry.name.to_string(),
                icon,
                collapsed,
                selected,
                accent,
                accent_foreground,
                foreground,
                muted_foreground,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.navigate_to(path.clone(), cx);
                }),
            ));
        }

        // —— 驱动器 ——
        if !collapsed {
            rows.push(section_title("驱动器", muted_foreground));
        }
        for (index, drive) in self.drives.iter().enumerate() {
            let path = drive.mount_point.clone();
            let selected = location.is_dir(&path);
            rows.push(drive_row(
                ("drive", index),
                drive,
                collapsed,
                selected,
                accent,
                accent_foreground,
                foreground,
                muted_foreground,
                track,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.navigate_to(path.clone(), cx);
                }),
            ));
        }

        // —— 常用文件夹（本次仅预留分区）——
        if !collapsed {
            rows.push(section_title("常用文件夹", muted_foreground));
            rows.push(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child("尚未添加")
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .border_r_1()
            .border_color(border)
            .overflow_hidden()
            // 折叠切换按钮位于侧边栏顶部。
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(40.0))
                    .px_2()
                    .flex_none()
                    .child(
                        div()
                            .id("sidebar-toggle")
                            .flex_none()
                            .p_1()
                            .cursor_pointer()
                            .text_color(muted_foreground)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.toggle_sidebar(cx);
                            }))
                            .child(Icon::new(IconName::PanelLeft).small()),
                    ),
            )
            .child(
                div()
                    .id("sidebar-body")
                    .flex()
                    .flex_col()
                    .flex_grow(1.0)
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .py_1()
                    .children(rows),
            )
            .with_animation(
                // id 里带上折叠状态：GPUI 的动画按 id 记忆，id 不变就只在元素首次出现时播放，
                // 之后的折叠/展开不会有过渡。把状态编进 id 才能每次切换都重新播放。
                // `ElementId` 只实现了与 `u32`/`EntityId` 的元组转换，bool 需要显式转一下。
                ("sidebar-width", u32::from(collapsed)),
                Animation::new(Duration::from_millis(180)).with_easing(ease_in_out),
                move |element, delta| {
                    element.w(px(start_width + (target_width - start_width) * delta))
                },
            )
            .into_any_element()
    }
}

/// 分区标题。
fn section_title(label: &'static str, color: Hsla) -> AnyElement {
    div()
        // 分组之间留出 16px：折叠态下不渲染标题，但间距仍然由内边距保证。
        .mt_2()
        .mb_1()
        .px_3()
        .text_xs()
        .text_color(color)
        .child(label)
        .into_any_element()
}

/// 快速访问与常用文件夹共用的导航行。
#[allow(clippy::too_many_arguments)]
fn navigation_row<F>(
    id: impl Into<ElementId>,
    label: String,
    icon: IconName,
    collapsed: bool,
    selected: bool,
    accent: Hsla,
    accent_foreground: Hsla,
    foreground: Hsla,
    muted_foreground: Hsla,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    row_shell(id, collapsed, selected, accent, on_click)
        .child(Icon::new(icon).small())
        .when(!collapsed, |element| {
            element
                .gap_2()
                .px_2()
                .text_sm()
                .text_color(if selected {
                    accent_foreground
                } else {
                    foreground
                })
                .child(div().w_full().overflow_hidden().child(label))
        })
        .when(collapsed, |element| element.text_color(muted_foreground))
        .into_any_element()
}

/// 驱动器行：图标 + 名称 + 剩余容量与容量条。
#[allow(clippy::too_many_arguments)]
fn drive_row<F>(
    id: impl Into<ElementId>,
    drive: &Drive,
    collapsed: bool,
    selected: bool,
    accent: Hsla,
    accent_foreground: Hsla,
    foreground: Hsla,
    muted_foreground: Hsla,
    track: Hsla,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let label = drive_label(drive);
    // 容量为 0 的驱动器（空光驱、未插卡读卡器）不显示容量条，显示进度条反而误导。
    let has_capacity = drive.total_bytes > 0;
    let used_ratio = drive.used_ratio();
    let free_text: SharedString = format!("剩余 {}", format_size(drive.free_bytes)).into();

    row_shell(id, collapsed, selected, accent, on_click)
        .child(Icon::new(drive_icon(drive.kind)).small())
        .when(!collapsed, |element| {
            element
                .gap_2()
                .px_2()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_grow(1.0)
                        .min_w(px(0.0))
                        .text_sm()
                        .text_color(if selected {
                            accent_foreground
                        } else {
                            foreground
                        })
                        .child(label)
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child(free_text),
                        ),
                )
                .when(has_capacity, |element| {
                    element.child(
                        div()
                            .flex_none()
                            .w(px(METER_WIDTH))
                            .h(px(METER_HEIGHT))
                            .rounded(px(2.0))
                            .bg(track)
                            .child(
                                div()
                                    .h_full()
                                    .rounded(px(2.0))
                                    .bg(accent)
                                    // 宽度用比例表示，避免依赖运行时测量。
                                    .w(px(METER_WIDTH * used_ratio)),
                            ),
                    )
                })
        })
        .when(collapsed, |element| element.text_color(muted_foreground))
        .into_any_element()
}

/// 导航行的公共外壳：固定高度、命中区域、选中底色与左侧竖条。
///
/// 返回 `Stateful<Div>` 而非 `Div`：调用 `.id(...)` 之后元素类型就已经变成有状态的，
/// 写死成 `Div` 会在调用方编译不过。
fn row_shell<F>(
    id: impl Into<ElementId>,
    collapsed: bool,
    selected: bool,
    accent: Hsla,
    on_click: F,
) -> gpui::Stateful<gpui::Div>
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let base = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .h(px(ROW_HEIGHT))
        // 折叠态只剩下图标，需要水平居中才不会贴左。
        .justify_center()
        .cursor_pointer()
        .on_click(on_click);

    if collapsed {
        return base;
    }

    base.justify_between().when(selected, |element| {
        element.bg(accent).child(div().flex_none().w(px(3.0)).h_full().bg(accent))
    })
}

/// 驱动器展示名。
fn drive_label(drive: &Drive) -> String {
    let kind = match drive.kind {
        DriveKind::Fixed => "本地磁盘",
        DriveKind::Removable => "可移动磁盘",
        DriveKind::Network => "网络驱动器",
        DriveKind::Optical => "光驱",
        DriveKind::Other => "驱动器",
    };
    format!("{kind} ({})", drive.name)
}

fn drive_icon(kind: DriveKind) -> IconName {
    match kind {
        DriveKind::Optical => IconName::Disc,
        DriveKind::Network => IconName::Network,
        DriveKind::Removable => IconName::Usb,
        DriveKind::Fixed | DriveKind::Other => IconName::HardDrive,
    }
}

/// 与 `fm-platform::windows::known_folders` 的枚举顺序一一对应。
fn quick_access_icon(index: usize) -> IconName {
    match index {
        0 => IconName::House,
        1 => IconName::Download,
        2 => IconName::FileText,
        3 => IconName::Image,
        4 => IconName::Music,
        5 => IconName::Video,
        _ => IconName::Folder,
    }
}
