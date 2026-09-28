//! 左侧导航栏：快速访问 / 此电脑（驱动器）/ 磁盘容量三个分区。
//!
//! 形态照搬设计稿的 `.nav`：展开 238px、折叠 48px，导航项 30px 高、6px 圆角、
//! 选中为强调色 16% 底色 + 图标转为强调色，子项左缩进 30px；
//! 底部是 `.disk` 容量档位（名字 + 百分比 + 容量条 + 剩余 / 总量）。
//!
//! 折叠态只保留图标并水平居中，不显示分区标题与容量档位——48px 放不下文字，
//! 硬塞只会得到一排被截断的残字。

use std::time::Duration;

use fm_core::{Location, format_size};
use fm_platform::{Drive, DriveKind};
use fm_views::metrics;
use fm_views::widgets;
use gpui::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, Context, ElementId, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, Window, div, ease_in_out, px, prelude::FluentBuilder as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::layout::Viewport;
use crate::shell::ShellView;

/// 展开态宽度。
pub(crate) const SIDEBAR_WIDTH: f32 = metrics::SIDEBAR_WIDTH;
/// 折叠态宽度。
pub(crate) const SIDEBAR_COLLAPSED_WIDTH: f32 = metrics::SIDEBAR_COLLAPSED_WIDTH;
/// 容量条宽度：比设计稿的 72px 略宽，用来容纳「428 GB / 1 TB」这类文本。
const DISK_METER_WIDTH: f32 = 150.0;

impl ShellView {
    /// 渲染左侧导航栏。
    ///
    /// 浮层模式（≤920px）下侧边栏压在内容之上，只在 `sidebar_overlay` 为真时出现；
    /// 此时它不参与横向布局，宽度由 [`Viewport::sidebar_width`] 返回 0 与之对应。
    pub(crate) fn render_sidebar(&self, viewport: Viewport, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;
        let overlay = viewport.sidebar_overlay;
        // 浮层模式不使用「折叠成 48px」这一形态：浮层是临时抽屉，
        // 它要么完整出现、要么完全消失，没有中间态。
        let collapsed = !overlay && self.config.sidebar_collapsed;

        // 浮层未展开时整条侧边栏不存在——不是「宽度为 0」，
        // 否则它会挡住下面的内容而用户看不出来。
        if overlay && !self.sidebar_overlay {
            return div().into_any_element();
        }

        // 折叠动画从「另一个状态」的宽度出发，因此需要显式记住起点。
        // 浮层模式两端同宽，动画自然退化为无过渡。
        let (start_width, target_width) = if overlay {
            (SIDEBAR_WIDTH, SIDEBAR_WIDTH)
        } else if collapsed {
            (SIDEBAR_WIDTH, SIDEBAR_COLLAPSED_WIDTH)
        } else {
            (SIDEBAR_COLLAPSED_WIDTH, SIDEBAR_WIDTH)
        };
        let location = self.tabs.active_state().location().clone();

        let mut rows: Vec<AnyElement> = Vec::new();

        // —— 快速访问 ——
        if !collapsed {
            rows.push(group_header("快速访问", tokens));
        }
        for (index, entry) in self.quick_access.iter().enumerate() {
            let path = entry.path.as_ref().clone();
            let selected = location.is_dir(&path);
            rows.push(nav_row(
                ("quick", index),
                entry.name.to_string(),
                quick_access_icon(index),
                collapsed,
                false,
                selected,
                tokens,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.navigate_to(path.clone(), cx);
                }),
            ));
        }

        // —— 回收站 ——
        //
        // 放在「访问位置」列表的最下面。它是虚拟位置而不是真实目录，
        // 因此不走 `quick_access`（那份数据来自系统的已知文件夹枚举），单独一行。
        rows.push(nav_row(
            "recycle-bin",
            String::from("回收站"),
            IconName::Trash,
            collapsed,
            false,
            matches!(location, Location::RecycleBin),
            tokens,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.go_to_recycle_bin(cx);
            }),
        ));

        // —— 此电脑（含驱动器子项）——
        //
        // 放在「快速访问」之后：它是「向上」的终点，也是后续 `Win+E` 接管的默认落点。
        let computer_selected = matches!(location, Location::Computer);
        if collapsed {
            rows.push(nav_row(
                "computer",
                String::from("此电脑"),
                IconName::PcCase,
                true,
                false,
                computer_selected,
                tokens,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.go_to_computer(cx);
                }),
            ));
        } else {
            rows.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        div()
                            .flex_grow(1.0)
                            .min_w(px(0.0))
                            .child(nav_row(
                                "computer",
                                String::from("此电脑"),
                                IconName::PcCase,
                                false,
                                false,
                                computer_selected,
                                tokens,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.go_to_computer(cx);
                                }),
                            )),
                    )
                    .child(caret_button(
                        ("computer-caret", u32::from(self.drives_expanded)),
                        self.drives_expanded,
                        tokens,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.toggle_drives_group(cx);
                        }),
                    ))
                    .into_any_element(),
            );
        }

        if self.drives_expanded || collapsed {
            for (index, drive) in self.drives.iter().enumerate() {
                let path = drive.mount_point.clone();
                let selected = location.is_dir(&path);
                rows.push(nav_row(
                    ("drive", index),
                    drive_label(drive),
                    drive_icon(drive.kind),
                    collapsed,
                    true,
                    selected,
                    tokens,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.navigate_to(path.clone(), cx);
                    }),
                ));
            }
        }

        let navigation = div()
            .id("sidebar-nav")
            .flex()
            .flex_col()
            .flex_grow(1.0)
            .min_h(px(0.0))
            .overflow_y_scroll()
            .px(px(8.0))
            .pt(px(8.0))
            .pb(px(6.0))
            .children(rows);

        let mut sidebar = div()
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .bg(tokens.panel)
            .border_r_1()
            .border_color(tokens.border)
            .overflow_hidden()
            .child(navigation);

        // 容量档位只在展开态出现：它需要一整行文字，折叠态塞不下。
        if !collapsed
            && let Some(meters) = self.render_disk_meters()
        {
            sidebar = sidebar.child(meters);
        }

        // 浮层：不占横向空间，压在内容之上，并加投影把自己与内容分开。
        // 定位必须在 `with_animation` 之前设置：动画元素只负责宽度，
        // 它不转发定位类的样式方法。
        if overlay {
            sidebar = sidebar
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .shadow_lg();
        }

        let sidebar = sidebar.with_animation(
            // id 里带上折叠状态：GPUI 的动画按 id 记忆，id 不变就只在元素首次出现时播放，
            // 之后的折叠/展开不会有过渡。把状态编进 id 才能每次切换都重新播放。
            // `ElementId` 只实现了与 `u32`/`EntityId` 的元组转换，bool 需要显式转一下。
            ("sidebar-width", u32::from(collapsed)),
            Animation::new(Duration::from_millis(180)).with_easing(ease_in_out),
            move |element, delta| element.w(px(start_width + (target_width - start_width) * delta)),
        );

        sidebar.into_any_element()
    }

    /// 侧边栏底部的磁盘容量档位。
    ///
    /// 只在容量已知的驱动器上渲染：空光驱、未插卡的读卡器算不出比例，
    /// 画一条恒为空的进度条比不画更容易让人误判。
    fn render_disk_meters(&self) -> Option<AnyElement> {
        let tokens = &self.tokens;
        let measurable: Vec<&Drive> = self
            .drives
            .iter()
            .filter(|drive| drive.total_bytes > 0)
            .collect();
        if measurable.is_empty() {
            return None;
        }

        let meters = measurable.into_iter().map(|drive| {
            let ratio = drive.used_ratio();
            let percent = (ratio * 100.0).round();
            let used = format_size(drive.total_bytes.saturating_sub(drive.free_bytes));
            let total = format_size(drive.total_bytes);
            let free = format_size(drive.free_bytes);

            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .font_family(tokens.mono_font.clone())
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.muted_fg)
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(tokens.fg)
                                .child(drive_label(drive)),
                        )
                        .child(format!("{percent:.0}%")),
                )
                .child(widgets::capacity_bar(
                    ratio,
                    DISK_METER_WIDTH,
                    metrics::DISK_METER_HEIGHT,
                    tokens,
                    tokens.hover,
                ))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .font_family(tokens.mono_font.clone())
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.muted_fg)
                        .child(format!("{used} / {total}"))
                        .child(format!("{free} 可用")),
                )
        });

        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .px(px(12.0))
                .pt(px(10.0))
                .pb(px(12.0))
                .border_t_1()
                .border_color(tokens.border)
                .children(meters)
                .into_any_element(),
        )
    }
}

/// 分区标题：等宽小字 + 图标。
fn group_header(label: &'static str, tokens: &fm_views::Tokens) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(6.0))
        .pt(px(6.0))
        .pb(px(4.0))
        .text_color(tokens.muted_fg)
        .child(widgets::section_label(label, tokens))
        .into_any_element()
}

/// 分组折叠箭头。
fn caret_button<F>(
    id: impl Into<ElementId>,
    expanded: bool,
    tokens: &fm_views::Tokens,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
{
    let hover = tokens.hover;
    let foreground = tokens.fg;
    let muted = tokens.muted_fg;

    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(20.0))
        .h(px(20.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .text_color(muted)
        .hover(move |style| style.bg(hover).text_color(foreground))
        .on_click(on_click)
        .child(
            Icon::new(if expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .with_size(px(13.0)),
        )
        .into_any_element()
}

/// 导航行。
///
/// `collapsed` 为真时只画图标并水平居中；`is_sub` 为真时左缩进 30px，
/// 让驱动器明确显示为「此电脑」的下级。
#[allow(clippy::too_many_arguments)]
fn nav_row<F>(
    id: impl Into<ElementId>,
    label: String,
    icon: IconName,
    collapsed: bool,
    is_sub: bool,
    selected: bool,
    tokens: &fm_views::Tokens,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
{
    let hover = tokens.hover;
    let accent_soft = tokens.accent_soft;
    let accent = tokens.accent;
    let foreground = tokens.fg;
    let muted = tokens.muted_fg;

    let icon_color: Hsla = if selected { accent } else { muted };

    let mut row = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .h(px(metrics::NAV_ITEM_HEIGHT))
        .rounded(px(metrics::RADIUS))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .when(selected, |element| element.bg(accent_soft))
        .on_click(on_click);

    if collapsed {
        return row
            .justify_center()
            .child(Icon::new(icon).with_size(px(16.0)).text_color(icon_color))
            .into_any_element();
    }

    row = row
        .gap(px(9.0))
        .px(px(8.0))
        .when(is_sub, |element| element.pl(px(metrics::NAV_SUB_INDENT)))
        .child(Icon::new(icon).with_size(px(15.0)).text_color(icon_color))
        .child(
            div()
                .flex_grow(1.0)
                .min_w(px(0.0))
                .overflow_hidden()
                .truncate()
                .text_size(px(12.5))
                .text_color(foreground)
                .when(selected, |element| {
                    element.font_weight(gpui::FontWeight::MEDIUM)
                })
                .child(label),
        );

    row.into_any_element()
}

/// 驱动器展示名，例如「本地磁盘 (C:)」。
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
        0 => IconName::Monitor,
        1 => IconName::Download,
        2 => IconName::FileText,
        3 => IconName::Image,
        4 => IconName::Music,
        5 => IconName::Video,
        _ => IconName::Folder,
    }
}
