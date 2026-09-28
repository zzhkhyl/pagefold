//! 自定义标题栏：品牌标志 + 状态指示 + 窗口级动作 + 窗口控制按钮。
//!
//! 之所以自绘：设计稿的第一行是「叠页」品牌 + 右侧动作组，
//! 而系统标题栏既放不下这些动作、也无法应用设计稿的 panel 底色与 1px 分隔线。
//! 窗口因此以 `appears_transparent: true` 创建（见 `fm-app` 的 `bootstrap`），
//! **最小化 / 最大化 / 关闭三个按钮必须由这里提供**——它们是自绘标题栏的配套义务，
//! 具体动作交给平台（`WindowControlArea`），不自己实现窗口缩放逻辑。

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, WindowControlArea, div, px, prelude::FluentBuilder as _,
};
use fm_views::widgets;
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::layout::Viewport;
use crate::shell::ShellView;

/// 窗口控制按钮的宽度。46px 是 Windows 11 系统按钮的标准宽度。
const WINDOW_CONTROL_WIDTH: f32 = 46.0;

impl ShellView {
    /// 渲染标题栏。
    pub(crate) fn render_titlebar(&self, viewport: Viewport, cx: &mut Context<Self>) -> AnyElement {
        let tokens = &self.tokens;

        // 拖拽区的**边界非常重要**：命中区的匹配是「按注册顺序取第一个命中的」，
        // 而父元素的命中区先于子元素注册。若把 Drag 挂在整条标题栏上，
        // 它永远先于内部的窗口控制按钮被命中——后果是三个按钮全部失效（点哪都在拖窗口）。
        // 因此 Drag 只能挂在**本身不含按钮**的那几段上：品牌段与中间的空白段。
        let brand = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(7.0))
            .text_size(px(fm_views::metrics::FONT_UI))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(tokens.fg)
            .window_control_area(WindowControlArea::Drag)
            .child(
                Icon::new(IconName::Layers)
                    .with_size(px(17.0))
                    .text_color(tokens.accent),
            )
            // 极窄窗口下只留标志：文字会把右侧的动作组挤出去。
            .when(!viewport.narrow, |element| element.child("叠页"));

        // 设计稿这里显示的是「云盘已同步」。本项目没有云同步能力，
        // 因此改成如实反映当前状态的就绪指示，而不是写一句做不到的承诺。
        let status_text = if self.loading {
            "正在读取…"
        } else if self.error.is_some() {
            "读取失败"
        } else {
            "就绪"
        };
        let status = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .pr(px(8.0))
            .font_family(tokens.mono_font.clone())
            .text_size(px(fm_views::metrics::FONT_XS))
            .text_color(tokens.muted_fg)
            .child(widgets::dot(
                if self.error.is_some() {
                    tokens.danger
                } else {
                    tokens.accent
                },
                6.0,
            ))
            .child(status_text);

        let theme_toggle = widgets::icon_button(
            "titlebar-theme",
            if self.config.dark {
                IconName::Sun
            } else {
                IconName::Moon
            },
            16.0,
            if self.config.dark {
                "切换到浅色主题"
            } else {
                "切换到深色主题"
            },
            false,
            true,
            tokens,
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.toggle_theme(window, cx);
            }),
        );

        let controls = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .child(window_control(
                "window-min",
                IconName::Minus,
                WindowControlArea::Min,
                false,
                tokens,
            ))
            .child(window_control(
                "window-max",
                IconName::Square,
                WindowControlArea::Max,
                false,
                tokens,
            ))
            .child(window_control(
                "window-close",
                IconName::X,
                WindowControlArea::Close,
                true,
                tokens,
            ));

        let mut right = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0));

        if viewport.compact {
            right = right.child(theme_toggle);
        } else {
            right = right
                .child(status)
                .child(separator(tokens))
                .child(theme_toggle)
                .child(separator(tokens));
        }
        let right = right.child(controls);

        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .h(px(fm_views::metrics::TITLEBAR_HEIGHT))
            .pl(px(12.0))
            .bg(tokens.panel)
            .border_b_1()
            .border_color(tokens.border)
            .child(brand)
            // 中间这段既撑开左右两组，也是主要的拖拽区域（同样不含任何按钮）。
            .child(
                div()
                    .flex_grow(1.0)
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(right)
            .into_any_element()
    }
}

/// 标题栏里的竖分隔线。
fn separator(tokens: &fm_views::Tokens) -> AnyElement {
    div()
        .flex_none()
        .w(px(1.0))
        .h(px(16.0))
        .mx(px(6.0))
        .bg(tokens.border)
        .into_any_element()
}

/// 窗口控制按钮。
///
/// 动作全部交给平台：`WindowControlArea` 会把这些命中区直接报告给系统，
/// 由系统完成最小化 / 最大化 / 关闭。自己实现那套逻辑会在多显示器、贴靠布局、
/// 系统菜单等边界上不断出问题，而平台实现天然是对的。
fn window_control(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    danger: bool,
    tokens: &fm_views::Tokens,
) -> AnyElement {
    let foreground = tokens.fg;
    let hover = tokens.hover;
    let danger_soft = tokens.danger_soft;
    let danger = danger.then_some(tokens.danger);

    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(WINDOW_CONTROL_WIDTH))
        .h(px(fm_views::metrics::TITLEBAR_HEIGHT))
        .text_color(tokens.muted_fg)
        .hover(move |style| {
            let style = style.text_color(foreground);
            // 关闭键悬停变红：与系统一致，避免误点。
            match danger {
                Some(danger) => style.bg(danger_soft).text_color(danger),
                None => style.bg(hover),
            }
        })
        .window_control_area(area)
        .child(Icon::new(icon).with_size(px(14.0)))
        .into_any_element()
}
