//! 右侧预览面板。
//!
//! 形态照搬设计稿的 `.preview`：306px 宽、panel 底色、左侧 1px 分隔线，
//! 三段结构 = 标题栏 / 可滚动正文 / 底部动作。
//!
//! 数据来源限定为「已经加载到内存的条目信息」：缩略图与文本预览需要读文件内容
//! （属平台层的后续工作），因此这里只呈现名称、类型、大小、修改时间与位置，
//! 不额外发起任何 IO——预览不应该成为一个会让界面卡住的入口。

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::tokens::{Tokens, metrics};

/// 「打开文件夹」回调。参数是目标目录。
pub type OpenDirHandler = Rc<dyn Fn(PathBuf, &mut Window, &mut App)>;

/// 选中项的摘要信息。字段全部由 shell 预先算好（时区换算、类型文案都在那里完成）。
#[derive(Clone, Debug)]
pub struct EntrySummary {
    pub name: SharedString,
    pub kind_label: SharedString,
    /// 大小；目录为 `None`（目录的 `metadata().len()` 与内容无关，显示它是误导）。
    pub size: Option<SharedString>,
    pub modified: Option<SharedString>,
    pub location: SharedString,
    pub path: PathBuf,
    pub is_dir: bool,
    pub icon: IconName,
}

/// 预览面板要展示的内容。
#[derive(Clone, Debug)]
pub enum PreviewContent {
    /// 未选择任何项目。
    Empty,
    /// 选中了多项：只列名字与合计大小。
    Multiple {
        names: Vec<SharedString>,
        total_size: SharedString,
    },
    /// 选中了一项。
    Single(Box<EntrySummary>),
}

/// 渲染预览面板。
///
/// `on_open` 只在选中单个目录时被用到；文件需要用系统默认程序打开，
/// 那需要 `ShellExecute`（属平台层的后续工作），因此这里不提供对应按钮，
/// 而不是给一个点了没反应的控件。
pub fn panel(
    content: &PreviewContent,
    tokens: &Tokens,
    on_open: Option<OpenDirHandler>,
) -> AnyElement {
    let inner = match content {
        PreviewContent::Empty => empty(tokens),
        PreviewContent::Multiple { names, total_size } => multiple(names, total_size, tokens),
        PreviewContent::Single(summary) => single(summary, tokens, on_open),
    };

    // 统一的外层容器：面板自身占满父级的高度与宽度（父级负责宽度与左侧分隔线），
    // 三段内容的排布由各自的 `flex_grow` 决定。
    div()
        .flex()
        .flex_col()
        .w_full()
        .h_full()
        .overflow_hidden()
        .child(inner)
        .into_any_element()
}

/// 未选择任何项目。
fn empty(tokens: &Tokens) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .child(
            div()
                .flex_none()
                .px(px(12.0))
                .pt(px(12.0))
                .pb(px(10.0))
                .border_b_1()
                .border_color(tokens.border)
                .text_size(px(metrics::FONT_UI))
                .text_color(tokens.muted_fg)
                .child("未选择任何项目"),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .py(px(48.0))
                .px(px(12.0))
                .child(
                    Icon::new(IconName::Eye)
                        .with_size(px(26.0))
                        .text_color(tokens.muted_fg),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(tokens.muted_fg)
                        .child("选择文件或文件夹后可在此查看详细信息"),
                ),
        )
        .into_any_element()
}

/// 选中多项。
fn multiple(names: &[SharedString], total_size: &str, tokens: &Tokens) -> AnyElement {
    let rows = names.iter().map(|name| {
        div()
            .flex_none()
            .py(px(5.0))
            .border_t_1()
            .border_color(tokens.border)
            .overflow_hidden()
            .truncate()
            .text_size(px(metrics::FONT_SM))
            .text_color(tokens.muted_fg)
            .child(name.clone())
    });

    div()
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .px(px(12.0))
                .pt(px(12.0))
                .pb(px(10.0))
                .border_b_1()
                .border_color(tokens.border)
                .child(
                    div()
                        .text_size(px(metrics::FONT_LG))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(tokens.fg)
                        .child(format!("已选择 {} 项", names.len())),
                )
                .child(
                    div()
                        .font_family(tokens.mono_font.clone())
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.muted_fg)
                        .child(format!("合计 {total_size}")),
                ),
        )
        .child(
            div()
                .id("preview-multi-scroll")
                .flex()
                .flex_col()
                .flex_grow(1.0)
                .min_h(px(0.0))
                .overflow_y_scroll()
                .p(px(12.0))
                .children(rows),
        )
        .into_any_element()
}

/// 选中单项。
fn single(
    summary: &EntrySummary,
    tokens: &Tokens,
    on_open: Option<OpenDirHandler>,
) -> AnyElement {
    let mut body = div()
        .id("preview-single-scroll")
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p(px(12.0));

    // 图标底板：与图标网格的缩略图底板同一形态，让两处的「文件长什么样」保持一致。
    body = body.child(
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .h(px(132.0))
            .mb(px(12.0))
            .rounded(px(metrics::RADIUS))
            .border_1()
            .border_color(tokens.border)
            .bg(tokens.bg)
            .child(
                Icon::new(summary.icon)
                    .with_size(px(44.0))
                    .text_color(if summary.is_dir {
                        tokens.accent
                    } else {
                        tokens.muted_fg
                    }),
            ),
    );

    let mut rows: Vec<(&'static str, String)> = vec![
        ("名称", summary.name.to_string()),
        ("类型", summary.kind_label.to_string()),
    ];
    if let Some(size) = &summary.size {
        rows.push(("大小", size.to_string()));
    }
    if let Some(modified) = &summary.modified {
        rows.push(("修改时间", modified.to_string()));
    }
    rows.push(("位置", summary.location.to_string()));

    let meta = rows.into_iter().map(move |(label, value)| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .font_family(tokens.mono_font.clone())
                    .text_size(px(metrics::FONT_XS))
                    .text_color(tokens.muted_fg)
                    .child(label),
            )
            .child(
                div()
                    .text_size(px(metrics::FONT_SM))
                    .text_color(tokens.fg)
                    .child(value),
            )
    });

    body = body.child(div().flex().flex_col().gap(px(10.0)).children(meta));

    let mut panel = div()
        .flex()
        .flex_col()
        .flex_none()
        .h_full()
        .w_full()
        .child(
            div()
                .flex_none()
                .px(px(12.0))
                .pt(px(12.0))
                .pb(px(10.0))
                .border_b_1()
                .border_color(tokens.border)
                .child(
                    div()
                        .text_size(px(metrics::FONT_LG))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(tokens.fg)
                        .child(summary.name.clone()),
                ),
        )
        .child(body);

    // 只有目录才有可执行的「打开」动作：进入目录是我们已经具备的能力。
    if summary.is_dir
        && let Some(handler) = on_open
    {
        let target = summary.path.clone();
        panel = panel.child(
            div()
                .flex_none()
                .p(px(10.0))
                .border_t_1()
                .border_color(tokens.border)
                .child(
                    div()
                        .id("preview-open")
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .w_full()
                        .h(px(30.0))
                        .rounded(px(metrics::RADIUS))
                        .bg(crate::tokens::with_alpha(tokens.accent, 0.9))
                        .text_color(tokens.surface)
                        .text_size(px(12.5))
                        .cursor_pointer()
                        .on_click(move |_event: &ClickEvent, window, cx| {
                            (*handler)(target.clone(), window, cx);
                        })
                        .child(Icon::new(IconName::FolderOpen).with_size(px(14.0)))
                        .child("打开文件夹"),
                ),
        );
    }

    panel.into_any_element()
}
