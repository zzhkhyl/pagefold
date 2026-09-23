//! 空状态与加载态。
//!
//! 设计约定（PLAN.md）：空目录显示插画式空状态而不是一片空白；
//! 加载中显示**骨架行**而不是整屏转圈——转圈会掩盖「首批已经到达」这一事实，
//! 让大目录显得比实际更慢。

use gpui::{
    AnyElement, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::Colors;

/// 加载骨架的默认行数。
pub const SKELETON_ROWS: usize = 10;

/// 空目录。
pub fn empty_folder(colors: Colors) -> AnyElement {
    centered(
        Icon::new(IconName::FolderOpen).large(),
        "此文件夹为空",
        colors.muted_foreground,
    )
}

/// 读取失败。
pub fn error(message: SharedString, colors: Colors) -> AnyElement {
    centered(
        Icon::new(IconName::CircleAlert).large(),
        message,
        colors.muted_foreground,
    )
}

/// 带指定图标的中性提示。
///
/// 供「没有可展示的内容、但也不算错误」的场景使用，例如「此电脑」下未检测到驱动器。
/// 用 [`empty_folder`] 会给出「此文件夹为空」这种与场景不符的文案。
pub fn notice(icon: IconName, message: impl Into<SharedString>, colors: Colors) -> AnyElement {
    centered(
        Icon::new(icon).large(),
        message.into(),
        colors.muted_foreground,
    )
}

/// 加载骨架。
///
/// 每行宽度略有差异，视觉上更像真实内容而不是一排等长色块。
pub fn loading(rows: usize, colors: Colors) -> AnyElement {
    let skeleton_rows = (0..rows.max(1)).map(move |index| {
        let scale = 0.55 + ((index % 4) as f32) * 0.1;
        div()
            .flex()
            .items_center()
            .flex_none()
            .h(px(28.0))
            .px_3()
            .child(
                div()
                    .h(px(10.0))
                    .rounded(px(4.0))
                    .bg(colors.muted)
                    .opacity(scale)
                    .w(px(600.0 * scale)),
            )
    });

    div()
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .min_h(px(0.0))
        .overflow_hidden()
        .py_2()
        .children(skeleton_rows)
        .child(
            div()
                .flex_none()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(colors.muted_foreground)
                .child("正在读取目录…"),
        )
        .into_any_element()
}

/// 图标 + 说明文字的居中版式。
fn centered(icon: impl IntoElement, message: impl Into<SharedString>, color: gpui::Hsla) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .items_center()
        .justify_center()
        .gap_2()
        .text_color(color)
        .child(icon)
        .child(div().flex_none().text_sm().child(message.into()))
        .into_any_element()
}
