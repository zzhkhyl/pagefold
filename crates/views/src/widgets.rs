//! 跨视图复用的基础控件：图标按钮、分段控件、等宽文本、容量条、勾选框。
//!
//! 这些形态在设计稿里被反复使用（`.ibtn` / `.seg` / `.bar` / `.cbxwrap`），
//! 集中成一处才能保证「悬停底色」「选中描边」这类细节在所有位置完全一致。
//!
//! 全部基于 `div` 手绘而不是组件库的 `Button`：设计稿的控件尺寸（30px 命中区、
//! 6px 圆角、16px 图标）与组件库默认规格并不相同，逐处覆盖反而更啰嗦。

use std::rc::Rc;

use fm_core::{Entry, EntryKind};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::input::{Input, InputState};
use gpui_component::tooltip::Tooltip;
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::tokens::{Tokens, metrics};

/// 画一个圆形色点（设计稿里的 `.sync i` 与 `.b-count i`）。
pub fn dot(color: Hsla, size: f32) -> AnyElement {
    div()
        .flex_none()
        .w(px(size))
        .h(px(size))
        .rounded_full()
        .bg(color)
        .into_any_element()
}

/// 标准图标按钮：30×30 命中区、6px 圆角、悬停浅灰底、激活态强调色浅底 + 描边。
///
/// 不可用时只降低不透明度并**不挂任何处理器**——不从元素树上摘掉是为了让
/// 布局不因可用性变化而抖动（后退按钮在历史耗尽时不应该让地址栏左右平移）。
#[allow(clippy::too_many_arguments)]
pub fn icon_button<F>(
    id: impl Into<ElementId>,
    icon: impl Into<Icon>,
    icon_size: f32,
    tooltip: impl Into<SharedString>,
    active: bool,
    enabled: bool,
    tokens: &Tokens,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    icon_button_with_size(
        id,
        icon,
        icon_size,
        metrics::CONTROL_SIZE,
        tooltip,
        active,
        enabled,
        tokens,
        on_click,
    )
}

/// 与 [`icon_button`] 同形，但命中区边长可定制（分段控件里用 26px）。
#[allow(clippy::too_many_arguments)]
pub fn icon_button_with_size<F>(
    id: impl Into<ElementId>,
    icon: impl Into<Icon>,
    icon_size: f32,
    hit_size: f32,
    tooltip: impl Into<SharedString>,
    active: bool,
    enabled: bool,
    tokens: &Tokens,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let muted = tokens.muted_fg;
    let foreground = tokens.fg;
    let hover = tokens.hover;
    let accent_soft = tokens.accent_soft;
    let accent_line = tokens.accent_line;
    let tooltip: SharedString = tooltip.into();

    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(hit_size))
        .h(px(hit_size))
        .rounded(px(metrics::RADIUS))
        .text_color(if active { foreground } else { muted })
        .when(active, |element| {
            element
                .bg(accent_soft)
                .border_1()
                .border_color(accent_line)
        })
        .when(enabled, |element| {
            element
                .cursor_pointer()
                .hover(move |style| style.bg(hover).text_color(foreground))
                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                .on_click(on_click)
        })
        .when(!enabled, |element| element.opacity(0.35))
        .child(Icon::new(icon).with_size(px(icon_size)))
        .into_any_element()
}

/// 纯图标按钮（不进入激活态），用于「刷新」这类无状态动作。
pub fn plain_icon_button<F>(
    id: impl Into<ElementId>,
    icon: impl Into<Icon>,
    icon_size: f32,
    tooltip: impl Into<SharedString>,
    tokens: &Tokens,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    icon_button(id, icon, icon_size, tooltip, false, true, tokens, on_click)
}

/// 分段控件外壳：1px 描边 + 2px 内边距，内部放若干 [`icon_button_with_size`]。
pub fn segmented(children: Vec<AnyElement>, tokens: &Tokens) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .gap(px(2.0))
        .p(px(2.0))
        .border_1()
        .border_color(tokens.border)
        .rounded(px(metrics::RADIUS))
        .children(children)
        .into_any_element()
}

/// 与可见性无关的勾选框：15×15 圆角方块，选中时用强调色填充并画对勾。
///
/// 手绘而不用组件库的 `Checkbox`：后者需要独立的实体状态并能获得焦点，
/// 而列表里的勾选框只是「选中集」的一种呈现，多一份状态只会带来不同步的风险。
pub fn checkbox(checked: bool, tokens: &Tokens) -> AnyElement {
    let accent = tokens.accent;
    let border = tokens.border;
    let foreground = tokens.fg;

    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .w(px(15.0))
        .h(px(15.0))
        .rounded(px(3.0))
        .border_1()
        .when(checked, |element| {
            element
                .bg(accent)
                .border_color(accent)
                .text_color(tokens.surface)
        })
        .when(!checked, |element| {
            element.border_color(border).text_color(foreground)
        })
        .when(checked, |element| {
            element.child(Icon::new(IconName::Check).with_size(px(10.0)))
        })
        .into_any_element()
}

/// 等宽小字。所有数字列与短标签都走这里，保证列对齐。
pub fn mono(text: impl Into<SharedString>, size: f32, color: Hsla, tokens: &Tokens) -> AnyElement {
    div()
        .flex_none()
        .font_family(tokens.mono_font.clone())
        .text_size(px(size))
        .text_color(color)
        .child(text.into())
        .into_any_element()
}

/// 界面字体文本。
pub fn text(
    text: impl Into<SharedString>,
    size: f32,
    color: Hsla,
    tokens: &Tokens,
) -> AnyElement {
    div()
        .flex_none()
        .font_family(tokens.font_family.clone())
        .text_size(px(size))
        .text_color(color)
        .child(text.into())
        .into_any_element()
}

/// 侧边栏分区标题：等宽小字，字距放宽，颜色为次级前景。
///
/// 设计稿用 `text-transform: uppercase` —— 中文没有大小写，这条规则对中文无效，
/// 因此这里只保留字距与字号，不做大小写转换（转了对中文也只是空操作）。
pub fn section_label(label: &'static str, tokens: &Tokens) -> AnyElement {
    div()
        .flex_none()
        .font_family(tokens.mono_font.clone())
        .text_size(px(metrics::FONT_XS))
        .text_color(tokens.muted_fg)
        .child(label)
        .into_any_element()
}

/// 驱动器容量条：底槽 + 已用部分 + 剩余留白。
///
/// 之所以拆成三段而不是「底槽 + 前景」：设计稿用剩余留白把「还能装多少」
/// 也画了出来，三段结构才能让这两段比例在任何宽度下都准确。
pub fn capacity_bar(
    used_ratio: f32,
    width: f32,
    height: f32,
    tokens: &Tokens,
    track: Hsla,
) -> AnyElement {
    let ratio = used_ratio.clamp(0.0, 1.0);
    let used_width = width * ratio;
    // 剩余段留 6% 的空隙：设计稿用它把已用与空闲两段在视觉上分开。
    let free_width = (width * (1.0 - ratio) - 6.0).max(0.0);

    div()
        .flex_none()
        .flex()
        .flex_row()
        .w(px(width))
        .h(px(height))
        .rounded(px(height / 2.0))
        .overflow_hidden()
        .bg(track)
        .child(
            div()
                .flex_none()
                .h_full()
                .w(px(used_width))
                .bg(tokens.capacity_fill(ratio)),
        )
        .child(
            div()
                .flex_none()
                .h_full()
                .w(px(free_width))
                .bg(crate::tokens::with_alpha(tokens.fg, 0.3)),
        )
        .into_any_element()
}

/// 文本按钮（设计稿的 `.btn.btn-ghost.btn-sm`）：用于选中条与预览面板的动作。
///
/// `enabled = false` 时降低不透明度并挂上说明性的 tooltip，而不是把按钮整个移除——
/// 被禁用往往只是**此刻**不适用（剪贴板为空时不能粘贴），按钮消失会让用户
/// 以为功能不见了。
///
/// `tooltip` 由调用方给出而不是从 `label` 推导：禁用时真正需要说明的是**为什么**
/// 不能点（「剪贴板为空」），而那只有调用方知道。
pub fn text_button<F>(
    id: impl Into<ElementId>,
    label: &'static str,
    icon: Option<IconName>,
    tooltip: impl Into<SharedString>,
    enabled: bool,
    tokens: &Tokens,
    on_click: F,
) -> AnyElement
where
    F: Fn(&ClickEvent, &mut Window, &mut App) + 'static,
{
    let muted = tokens.muted_fg;
    let foreground = tokens.fg;
    let hover = tokens.hover;
    let tooltip: SharedString = tooltip.into();

    div()
        .id(id)
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(10.0))
        .py(px(5.0))
        .rounded(px(metrics::RADIUS))
        .text_size(px(metrics::FONT_SM))
        .text_color(muted)
        .when(enabled, |element| {
            element
                .cursor_pointer()
                .hover(move |style| style.bg(hover).text_color(foreground))
                .on_click(on_click)
        })
        .when(!enabled, |element| element.opacity(0.4))
        .children(icon.map(|icon| Icon::new(icon).with_size(px(14.0))))
        .child(label)
        .tooltip(move |window, cx| {
            // 提示可能被反复展示，因此每次都从 `SharedString` 克隆一份文本，
            // 而不是把捕获到的字符串移动进接收者（`Fn` 闭包不允许消耗捕获值）。
            Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .into_any_element()
}

/// 内联重命名输入框（设计稿的 `.rename`）。
///
/// 详情表的名称列与网格的名称块都用它，样式因此只在这里定义一次——
/// 同一个交互在两个视图里长得不一样，是用户最容易察觉的不一致。
///
/// 高度取 `行高 - 10`（设计稿的 `calc(var(--row-h) - 10px)`），
/// 因此它会比一行略矮，看起来是「嵌在行里」而不是「撑破了行」。
/// 高度对两种视图都成立：网格的名称区固定 33px，而这里最多二十几像素。
pub fn rename_input(state: &Entity<InputState>, tokens: &Tokens) -> AnyElement {
    let height = (tokens.row_height - 10.0).max(16.0);

    div()
        .flex_grow(1.0)
        .min_w(px(0.0))
        .h(px(height))
        .px(px(6.0))
        .rounded(px(metrics::RENAME_RADIUS))
        .border_1()
        .border_color(tokens.accent)
        .bg(tokens.surface)
        .flex()
        .items_center()
        // `appearance(false)` 去掉组件库输入框自带的白底与描边，
        // 由上面这个容器提供设计稿的「强调色描边 + surface 底」。
        .child(Input::new(state).appearance(false).h_full())
        .into_any_element()
}

/// 把回调包进 `Rc`，供虚拟列表逐行克隆。
///
/// 每行都需要一个独立的 `'static` 闭包，直接克隆 `F` 会要求 `F: Clone`；
/// 包一层 `Rc` 只增加引用计数，且不改变调用语义。
pub fn shared<F>(callback: F) -> Rc<F> {
    Rc::new(callback)
}

/// 条目的行首 / 网格图标。
///
/// `extension` 由调用方经扩展名驻留表解析后传入——视图层不认识驻留表，
/// 只做「扩展名 → 图标」的映射。
pub fn entry_icon(entry: &Entry, extension: Option<&str>) -> IconName {
    match entry.kind {
        EntryKind::Directory => IconName::Folder,
        EntryKind::Drive => IconName::HardDrive,
        // 快捷方式在资源管理器里也是「带箭头的目标」，这里用外链图标表达同一语义。
        EntryKind::Symlink => IconName::ExternalLink,
        EntryKind::File { .. } => file_icon(extension),
    }
}

/// 按扩展名挑选文件图标。
///
/// 分组与设计稿的 `ICON_OF` 一致，只是换成了 Lucide 里同名或近义的图标。
/// 这里刻意不做「真正的系统图标」（那需要 `SHGetFileInfoW`，属平台层的后续工作），
/// 但分组粒度与设计稿对齐，视觉上不会跑偏。
pub fn file_icon(extension: Option<&str>) -> IconName {
    match extension {
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "heic") => IconName::Image,
        Some("svg" | "fig" | "ai" | "eps") => IconName::Shapes,
        Some("mp3" | "wav" | "flac" | "m4a" | "ogg" | "wma") => IconName::Music,
        Some("mp4" | "mkv" | "avi" | "mov" | "wmv" | "webm") => IconName::Video,
        Some("zip" | "rar" | "7z" | "tar" | "gz" | "iso") => IconName::Archive,
        Some("exe" | "msi" | "bat" | "cmd") => IconName::Package,
        Some("xls" | "xlsx" | "csv") => IconName::List,
        Some("db" | "sqlite") => IconName::HardDrive,
        Some("lnk" | "url") => IconName::ExternalLink,
        Some("pdf" | "doc" | "docx" | "md" | "txt" | "rtf" | "ppt" | "pptx") => IconName::FileText,
        _ => IconName::File,
    }
}
