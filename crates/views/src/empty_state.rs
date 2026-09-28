//! 空状态与加载态。
//!
//! 设计约定（`design/file-manager.html` 的 `.empty` 与 `.lhead`）：
//! 空目录显示带图标框的空状态而不是一片空白；加载中显示**骨架行**而不是整屏转圈——
//! 转圈会掩盖「首批已经到达」这一事实，让大目录显得比实际更慢。

use gpui::{
    AnyElement, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::tokens::{Tokens, metrics};

/// 加载骨架的默认行数。
pub const SKELETON_ROWS: usize = 10;

/// 空目录。
pub fn empty_folder(tokens: &Tokens) -> AnyElement {
    centered(
        IconName::FolderOpen,
        "此文件夹为空",
        Some("拖入文件或新建文件夹即可开始整理"),
        tokens,
    )
}

/// 读取失败。
pub fn error(message: SharedString, tokens: &Tokens) -> AnyElement {
    centered(IconName::CircleAlert, message, None, tokens)
}

/// 回收站为空。
///
/// 与 [`empty_folder`] 分开：两者的后续动作完全相反——空文件夹在提示用户
/// 往里放东西，空回收站则什么也不用做。
pub fn recycle_bin_empty(tokens: &Tokens) -> AnyElement {
    centered(
        IconName::Trash,
        "回收站是空的",
        Some("删除的项目会先放进这里，之后可以还原"),
        tokens,
    )
}

/// 筛选（搜索框）无结果。
pub fn no_results(query: &str, tokens: &Tokens) -> AnyElement {
    centered(
        IconName::Search,
        format!("没有匹配「{query}」的项目"),
        Some("换个关键词，或清空搜索框查看全部内容"),
        tokens,
    )
}

/// 带指定图标的中性提示。
///
/// 供「没有可展示的内容、但也不算错误」的场景使用，例如「此电脑」下未检测到驱动器。
/// 用 [`empty_folder`] 会给出「此文件夹为空」这种与场景不符的文案。
pub fn notice(
    icon: IconName,
    message: impl Into<SharedString>,
    tokens: &Tokens,
) -> AnyElement {
    centered(icon, message.into(), None, tokens)
}

/// 加载骨架。
///
/// 每行宽度略有差异，视觉上更像真实内容而不是一排等长色块。
/// 行高取当前密度对应的列表行高，切换密度时骨架不会与真实列表「错一拍」。
pub fn loading(rows: usize, tokens: &Tokens) -> AnyElement {
    let row_height = tokens.row_height;
    let bar = tokens.hover;
    let border = tokens.border;

    let skeleton_rows = (0..rows.max(1)).map(move |index| {
        // 0.55 / 0.65 / 0.75 / 0.85 四档循环，模拟长短不一的文件名。
        let scale = 0.55 + ((index % 4) as f32) * 0.1;
        div()
            .flex()
            .items_center()
            .flex_none()
            .h(px(row_height))
            .px(px(metrics::ROW_PADDING_X))
            .child(
                div()
                    .h(px(10.0))
                    .rounded(px(4.0))
                    .bg(bar)
                    .w(px(600.0 * scale)),
            )
    });

    div()
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .min_h(px(0.0))
        .overflow_hidden()
        .border_b_1()
        .border_color(border)
        .children(skeleton_rows)
        .into_any_element()
}

/// 图标框 + 标题 + 说明的居中版式。
///
/// 图标外框是 44px 的描边方块（设计稿 `.e-ic`）：给中性灰的提示一个视觉锚点，
/// 否则整块留白会让人以为界面没渲染出来。
fn centered(
    icon: IconName,
    title: impl Into<SharedString>,
    description: Option<&'static str>,
    tokens: &Tokens,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .items_center()
        .justify_center()
        .py(px(64.0))
        .px(px(24.0))
        .text_color(tokens.muted_fg)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(44.0))
                .h(px(44.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(tokens.border)
                .text_color(tokens.muted_fg)
                .child(Icon::new(icon).with_size(px(20.0))),
        )
        .child(
            div()
                .mt(px(14.0))
                .text_size(px(metrics::FONT_LG))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(tokens.fg)
                .child(title.into()),
        )
        .when_some(description, |element, description| {
            element
                .mt(px(6.0))
                .text_size(px(12.5))
                .child(description)
        })
        .into_any_element()
}
