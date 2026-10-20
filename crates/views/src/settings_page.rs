//! 设置页（标签内页面）。
//!
//! 形态：单列居中（最大 720）、三张分组卡片、行式控件。视觉语言完全来自
//! [`crate::tokens`] 与 [`crate::widgets`]，不引入新的色板或控件形态——
//! 设置页与主界面必须是同一套观感，切换主题时一次性换色。
//!
//! 本模块是**纯展示**：状态由 shell 持有（`Config` 与 `ShellView`），
//! 这里只接收一份快照与一组回调。这样做的理由是设置项会修改 `Config` 并
//! 触发令牌刷新，那套逻辑属于 shell；视图层若直接持有状态，
//! 两个 "真值源" 迟早不同步。
//!
//! 为什么不是独立窗口：设置页做成 `Location::Settings` 之后，
//! 标签条、`Ctrl+W`、历史栈与「重新打开关闭的标签页」全部免费获得。

use std::rc::Rc;

use fm_core::{Density, IconZoom, ViewMode};
use gpui::{
    AnyElement, App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
    prelude::FluentBuilder as _,
};
use gpui_component::{Icon, Sizable as _};
use gpui_kit_assets::IconName;

use crate::tokens::{Tokens, metrics, with_alpha};
use crate::widgets;

/// 内容列的最大宽度。
///
/// 设置项是「标签 + 控件」的行，行再宽也只会让两者离得更远；
/// 720 是设计稿 `.settings-panel` 的量级，也与常见系统设置页一致。
pub const PAGE_MAX_WIDTH: f32 = 720.0;

/// 快捷键表：**帮助页与设置页共用这份清单**。
///
/// 与 `fm-shell::actions` 的键位绑定是两处独立的真相，这一点无法靠类型系统消除
/// （绑定在 `fm-app` 的 `cx.bind_keys` 里，而视图层不能依赖那两个 crate）。
/// 因此把「展示用清单」集中在这里，并在文档里注明：改键位时两处一起改。
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+T", "新建标签页"),
    ("Ctrl+W", "关闭标签页"),
    ("Ctrl+Shift+T", "重新打开关闭的标签页"),
    ("Ctrl+,", "打开设置"),
    ("Ctrl+L", "聚焦地址栏"),
    ("Ctrl+1 / Ctrl+2", "详细信息 / 图标视图"),
    ("Ctrl+3", "切换深浅主题"),
    ("Alt+← / Alt+→", "后退 / 前进"),
    ("Backspace", "上一级"),
    ("F5", "刷新"),
    ("Ctrl+A", "全选"),
    ("Ctrl+C / Ctrl+X / Ctrl+V", "复制 / 剪切 / 粘贴"),
    ("F2", "重命名"),
    ("Delete", "移到回收站"),
    ("↑ / ↓（Shift 扩选）", "移动焦点"),
    ("Enter", "打开"),
    ("Esc", "关闭菜单 / 取消输入"),
];

/// 设置页要展示的状态快照（由 shell 从 `Config` 与运行期状态拼装）。
#[derive(Clone, Debug)]
pub struct SettingsSnapshot {
    pub dark: bool,
    pub density: Density,
    pub zoom: IconZoom,
    pub show_ext: bool,
    pub show_preview: bool,
    pub view_mode: ViewMode,
    /// 配置文件的绝对路径；取不到时为 `None`（环境缺 `APPDATA`）。
    pub config_path: Option<SharedString>,
    /// 版本号（由 shell 传入，避免各 crate 各写一份）。
    pub version: SharedString,
}

/// 「设定某个值」类回调的签名。
///
/// 抽成别名而不是逐个写全：同一个签名在结构体、`segment` 构造与测试里各出现一次，
/// 写全既难读，也让「这几个参数是同一个东西」看不出来。
pub type Setter<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;
/// 「切换」类回调（无参数）的签名。
pub type Toggle = Rc<dyn Fn(&mut Window, &mut App)>;

/// 设置项的回调集合。
///
/// 每项单独一个回调而不是一个大的 `enum`：调用点只关心「点这一项要做什么」，
/// 枚举会让每个控件都持有一份「所有动作」的分派逻辑。
pub struct SettingsHandlers {
    /// 主题：`true` = 深色。
    pub set_dark: Setter<bool>,
    pub set_density: Setter<Density>,
    pub set_zoom: Setter<IconZoom>,
    pub toggle_ext: Toggle,
    pub toggle_preview: Toggle,
    pub set_view_mode: Setter<ViewMode>,
    /// 复制配置文件路径到剪贴板。
    pub copy_config_path: Toggle,
}

/// 渲染整页。
pub fn page(snapshot: &SettingsSnapshot, tokens: &Tokens, handlers: SettingsHandlers) -> AnyElement {
    let column = div()
        .flex()
        .flex_col()
        .w_full()
        .max_w(px(PAGE_MAX_WIDTH))
        .gap(px(12.0))
        .child(page_header(tokens))
        .child(appearance_card(snapshot, tokens, &handlers))
        .child(view_card(snapshot, tokens, &handlers))
        .child(about_card(snapshot, tokens, &handlers));

    // 滚动容器必须是 `Stateful<Div>`（带 `.id()`）才拿得到 `overflow_y_scroll`；
    // 普通 `Div` 只有 `overflow_y_hidden`，写成后者会得到「内容被裁掉且不滚动」。
    div()
        .id("settings-page")
        .flex()
        .flex_col()
        .flex_grow(1.0)
        .min_h(px(0.0))
        .w_full()
        .overflow_y_scroll()
        .bg(tokens.bg)
        // 用「居中行 + 定宽列」实现水平居中，而不是 `mx_auto`：
        // 定宽列在窄窗下需要先被压缩（`max_w` + `w_full`），
        // 居中容器对这条约束的表达最直接。
        .child(
            div()
                .flex()
                .flex_row()
                .justify_center()
                .p(px(20.0))
                .child(column),
        )
        .into_any_element()
}

/// 页面标题区：标题 + 一句说明（明确「没有保存按钮」这件事）。
fn page_header(tokens: &Tokens) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .px(px(4.0))
        .pb(px(4.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    Icon::new(IconName::Settings)
                        .with_size(px(18.0))
                        .text_color(tokens.accent),
                )
                .child(
                    div()
                        .text_size(px(metrics::FONT_XL))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(tokens.fg)
                        .child("设置"),
                ),
        )
        .child(
            div()
                .text_size(px(metrics::FONT_SM))
                .text_color(tokens.muted_fg)
                .child("外观与行为的偏好会立即保存，没有「应用」这一步。"),
        )
        .into_any_element()
}

fn appearance_card(
    snapshot: &SettingsSnapshot,
    tokens: &Tokens,
    handlers: &SettingsHandlers,
) -> AnyElement {
    let theme = widgets::segmented(
        vec![
            theme_segment(handlers, false, !snapshot.dark, tokens),
            theme_segment(handlers, true, snapshot.dark, tokens),
        ],
        tokens,
    );

    let density = widgets::segmented(
        [Density::Compact, Density::Normal, Density::Cozy]
            .into_iter()
            .map(|density| {
                let handler = Rc::clone(&handlers.set_density);
                widgets::text_segment(
                    ("density", density.row_height() as usize),
                    density.label(),
                    snapshot.density == density,
                    tokens,
                    move |_: &ClickEvent, window, cx| handler(density, window, cx),
                )
            })
            .collect(),
        tokens,
    );

    let zoom = widgets::segmented(
        IconZoom::ALL
            .into_iter()
            .map(|zoom| {
                let handler = Rc::clone(&handlers.set_zoom);
                widgets::text_segment(
                    ("zoom", zoom.index()),
                    zoom.label(),
                    snapshot.zoom == zoom,
                    tokens,
                    move |_: &ClickEvent, window, cx| handler(zoom, window, cx),
                )
            })
            .collect(),
        tokens,
    );

    let ext = {
        let handler = Rc::clone(&handlers.toggle_ext);
        widgets::switch(
            "setting-show-ext",
            snapshot.show_ext,
            tokens,
            move |_: &ClickEvent, window, cx| handler(window, cx),
        )
    };

    card(
        "外观",
        "影响整个界面的配色与密度，改动立即生效。",
        vec![
            setting_row("主题", "浅色 / 深色，跟随系统之外的手动选择", theme, false, tokens),
            setting_row(
                "界面密度",
                "列表行高：紧凑 27px / 标准 32px / 宽松 38px",
                density,
                false,
                tokens,
            ),
            setting_row(
                "图标缩放",
                "图标视图的单元格边长：122 / 152 / 192px",
                zoom,
                false,
                tokens,
            ),
            setting_row(
                "显示文件扩展名",
                "关闭后只显示主文件名，重命名时不再预选扩展名",
                ext,
                true,
                tokens,
            ),
        ],
        tokens,
    )
}

fn view_card(
    snapshot: &SettingsSnapshot,
    tokens: &Tokens,
    handlers: &SettingsHandlers,
) -> AnyElement {
    let preview = {
        let handler = Rc::clone(&handlers.toggle_preview);
        widgets::switch(
            "setting-show-preview",
            snapshot.show_preview,
            tokens,
            move |_: &ClickEvent, window, cx| handler(window, cx),
        )
    };

    let view_mode = widgets::segmented(
        [ViewMode::Details, ViewMode::Icons]
            .into_iter()
            .map(|mode| {
                let handler = Rc::clone(&handlers.set_view_mode);
                widgets::text_segment(
                    ("view-mode", mode as usize),
                    mode.label(),
                    snapshot.view_mode == mode,
                    tokens,
                    move |_: &ClickEvent, window, cx| handler(mode, window, cx),
                )
            })
            .collect(),
        tokens,
    );

    card(
        "视图",
        "新标签页打开的默认形态。",
        vec![
            setting_row(
                "显示预览面板",
                "宽屏为右侧第三栏，窄于 1100px 时改为浮层",
                preview,
                false,
                tokens,
            ),
            setting_row(
                "默认视图",
                "修改后立即作用于当前标签，并作为新标签的初始形态",
                view_mode,
                true,
                tokens,
            ),
        ],
        tokens,
    )
}

fn about_card(
    snapshot: &SettingsSnapshot,
    tokens: &Tokens,
    handlers: &SettingsHandlers,
) -> AnyElement {
    let path_row = {
        let copy = Rc::clone(&handlers.copy_config_path);
        let label: SharedString = snapshot
            .config_path
            .clone()
            .unwrap_or_else(|| "（当前环境读不到 APPDATA，配置不会写盘）".into());
        let enabled = snapshot.config_path.is_some();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .min_w(px(0.0))
            .child(
                div()
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .truncate()
                    .font_family(tokens.mono_font.clone())
                    .text_size(px(metrics::FONT_XS))
                    .text_color(tokens.muted_fg)
                    .child(label),
            )
            .child(widgets::text_button(
                "setting-copy-config-path",
                "复制",
                Some(IconName::Copy),
                if enabled {
                    "复制配置文件路径到剪贴板"
                } else {
                    "当前环境没有配置文件"
                },
                enabled,
                tokens,
                move |_: &ClickEvent, window, cx| copy(window, cx),
            ))
            .into_any_element()
    };

    let about = div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .px(px(16.0))
        .py(px(12.0))
        .border_b_1()
        .border_color(tokens.border)
        .child(info_line("应用", "叠页（Pagefold）".into(), tokens))
        .child(info_line(
            "版本",
            format!("{} · Windows x64", snapshot.version).into(),
            tokens,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .font_family(tokens.mono_font.clone())
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.muted_fg)
                        .child("配置文件"),
                )
                .child(path_row),
        );

    let shortcuts = shortcut_table(tokens);

    let card = div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(metrics::RADIUS_LG))
        .border_1()
        .border_color(tokens.border)
        .bg(tokens.surface)
        .overflow_hidden()
        .child(card_header(
            "关于与快捷键",
            "键盘操作速查；设置页本身也可以按 Ctrl+, 打开。",
            tokens,
        ))
        .child(about);

    card.child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(12.0))
            .child(widgets::section_label("快捷键", tokens))
            .child(shortcuts),
    )
    .into_any_element()
}

/// 一行「键位 + 说明」。键位用等宽字体 + 浅底胶囊，说明用正文色。
fn shortcut_table(tokens: &Tokens) -> AnyElement {
    let rows = SHORTCUTS.iter().map(|(keys, description)| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .py(px(3.0))
            .child(
                div()
                    .flex_none()
                    .w(px(196.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(22.0))
                            .px(px(7.0))
                            .rounded(px(4.0))
                            .bg(with_alpha(tokens.fg, 0.06))
                            .font_family(tokens.mono_font.clone())
                            .text_size(px(metrics::FONT_XS))
                            .text_color(tokens.fg)
                            .child(*keys),
                    ),
            )
            .child(
                div()
                    .flex_grow(1.0)
                    .min_w(px(0.0))
                    .text_size(px(metrics::FONT_SM))
                    .text_color(tokens.muted_fg)
                    .child(*description),
            )
    });

    div().flex().flex_col().children(rows).into_any_element()
}

fn theme_segment(
    handlers: &SettingsHandlers,
    dark: bool,
    active: bool,
    tokens: &Tokens,
) -> AnyElement {
    let handler = Rc::clone(&handlers.set_dark);
    widgets::text_segment(
        ("theme", usize::from(dark)),
        if dark { "深色" } else { "浅色" },
        active,
        tokens,
        move |_: &ClickEvent, window, cx| handler(dark, window, cx),
    )
}

/// 分组卡片外壳：标题区 + 行列表。
fn card(
    title: &'static str,
    description: &'static str,
    rows: Vec<AnyElement>,
    tokens: &Tokens,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(metrics::RADIUS_LG))
        .border_1()
        .border_color(tokens.border)
        .bg(tokens.surface)
        .overflow_hidden()
        .child(card_header(title, description, tokens))
        .children(rows)
        .into_any_element()
}

fn card_header(title: &'static str, description: &'static str, tokens: &Tokens) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .px(px(16.0))
        .py(px(12.0))
        .border_b_1()
        .border_color(tokens.border)
        .child(
            div()
                .text_size(px(metrics::FONT_UI))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(tokens.fg)
                .child(title),
        )
        .child(
            div()
                .text_size(px(metrics::FONT_XS))
                .text_color(tokens.muted_fg)
                .child(description),
        )
        .into_any_element()
}

/// 一行设置项：左侧标签 + 说明，右侧控件固定右对齐。
///
/// 行高随密度变化（基准 44px，即 `row_height + 12`）：设置页与列表用同一个
/// 密度档位，用户切换密度时整页的节奏也跟着变，而不是只有列表在变。
fn setting_row(
    label: &'static str,
    hint: &'static str,
    control: AnyElement,
    last: bool,
    tokens: &Tokens,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.0))
        .px(px(16.0))
        .py(px(8.0))
        .min_h(px(tokens.row_height + 12.0))
        .when(!last, |element| {
            element.border_b_1().border_color(tokens.border)
        })
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_grow(1.0)
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(metrics::FONT_UI))
                        .text_color(tokens.fg)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(metrics::FONT_XS))
                        .text_color(tokens.muted_fg)
                        .child(hint),
                ),
        )
        .child(control)
        .into_any_element()
}

fn info_line(label: &'static str, value: SharedString, tokens: &Tokens) -> AnyElement {
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
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_table_covers_every_documented_group() {
        // 帮助页是用户唯一的键位来源，缺项等于「功能不存在」。
        // 这里只断言关键项存在，不锁死顺序：顺序调整不应该让测试失败。
        for keys in [
            "Ctrl+T",
            "Ctrl+W",
            "Ctrl+L",
            "Ctrl+,",
            "F2",
            "Delete",
            "Enter",
            "F5",
        ] {
            assert!(
                SHORTCUTS.iter().any(|(key, _)| *key == keys),
                "快捷键表缺少 {keys}"
            );
        }
    }

    #[test]
    fn snapshot_and_handler_types_are_constructible() {
        // 编译期契约：shell 侧拼装这两个结构时不需要额外的 trait 实现。
        let snapshot = SettingsSnapshot {
            dark: false,
            density: Density::Normal,
            zoom: IconZoom::Medium,
            show_ext: true,
            show_preview: true,
            view_mode: ViewMode::Details,
            config_path: None,
            version: "0.0.0".into(),
        };
        assert!(!snapshot.dark);

        let handlers = SettingsHandlers {
            set_dark: Rc::new(|_, _, _| {}),
            set_density: Rc::new(|_, _, _| {}),
            set_zoom: Rc::new(|_, _, _| {}),
            toggle_ext: Rc::new(|_, _| {}),
            toggle_preview: Rc::new(|_, _| {}),
            set_view_mode: Rc::new(|_, _, _| {}),
            copy_config_path: Rc::new(|_, _| {}),
        };
        // 只验证字段类型能被调用，真正调用需要窗口句柄。
        assert!(Rc::strong_count(&handlers.set_dark) == 1);
    }
}
