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

/// 文件夹接管的当前状态。
///
/// 与 `fm_platform::IntegrationState` 同形但**不复用**：views 不依赖平台层，
/// 而且这里的语义更窄——只管「这两个开关是否处于打开状态」。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderIntegration {
    /// 文件夹右键菜单里是否有本程序的菜单项。
    pub context_menu: bool,
    /// 文件夹的默认打开程序是否已指向本程序。
    pub default_handler: bool,
}

impl FolderIntegration {
    /// 两项都装上了（用于给出一句总览）。
    pub fn is_fully_installed(&self) -> bool {
        self.context_menu && self.default_handler
    }
}

/// 设置页要展示的状态快照（由 shell 从 `Config` 与运行期状态拼装）。
#[derive(Clone, Debug)]
pub struct SettingsSnapshot {
    pub dark: bool,
    pub density: Density,
    pub zoom: IconZoom,
    pub show_ext: bool,
    pub show_hidden: bool,
    pub show_preview: bool,
    pub view_mode: ViewMode,
    /// 配置文件的绝对路径；取不到时为 `None`（环境缺 `APPDATA`）。
    pub config_path: Option<SharedString>,
    /// 版本号（由 shell 传入，避免各 crate 各写一份）。
    pub version: SharedString,
    /// 文件夹接管状态（从注册表现读）。
    pub folder_integration: FolderIntegration,
    /// 快捷键配置（应用内 + 全局唤醒）。
    pub shortcuts: ShortcutsSnapshot,
}

/// 快捷键设置要展示的全部信息。
///
/// 由外壳算好再传进来（与其它设置项一致）：视图层只负责画，
/// 不做「谁和谁撞车」这种判断——那是 `fm_core::shortcut` 的职责，且已经单测过。
#[derive(Clone, Debug, Default)]
pub struct ShortcutsSnapshot {
    /// 已合并默认值的命令清单。
    pub items: Vec<fm_core::ResolvedShortcut>,
    /// 应用内互相冲突的键位。
    pub conflicts: Vec<fm_core::shortcut::Conflict>,
    /// 全局唤醒热键的键位（`Binding` 形式）。
    pub global_keys: String,
    pub global_enabled: bool,
    /// 全局热键的问题（被占用、与内部命令冲突等），`None` = 正常。
    pub global_problem: Option<String>,
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
    pub toggle_hidden: Toggle,
    pub toggle_preview: Toggle,
    pub set_view_mode: Setter<ViewMode>,
    /// 复制配置文件路径到剪贴板。
    pub copy_config_path: Toggle,
    /// 安装 / 卸载「用 Pagefold 打开」右键菜单项（取反当前状态）。
    pub toggle_folder_menu: Toggle,
    /// 设为 / 取消文件夹的默认打开程序（取反当前状态）。
    pub toggle_folder_default: Toggle,
    /// 一次性撤销上面两项（恢复资源管理器为默认）。
    pub restore_folder_integration: Toggle,
    /// 打开某条应用内命令的录制浮层。
    pub edit_shortcut: Setter<String>,
    /// 启用 / 禁用某条应用内命令。
    pub toggle_shortcut: Setter<String>,
    /// 打开全局唤醒热键的录制浮层。
    pub edit_global_hotkey: Toggle,
    /// 启用 / 禁用全局唤醒热键。
    pub toggle_global_hotkey: Toggle,
    /// 全部恢复默认。
    pub reset_shortcuts: Toggle,
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
        .child(integration_card(snapshot, tokens, &handlers))
        .child(shortcuts_card(snapshot, tokens, &handlers))
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
                zoom_hint(snapshot.view_mode),
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

    let hidden = {
        let handler = Rc::clone(&handlers.toggle_hidden);
        widgets::switch(
            "setting-show-hidden",
            snapshot.show_hidden,
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
                "显示隐藏和系统文件",
                "带隐藏或系统属性的条目；点号开头的文件在 Windows 上不算隐藏",
                hidden,
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

/// 「系统集成」卡片：文件夹接管的图形面板。
///
/// 这两项能力之前只存在于托盘菜单里——一个用户不一定会打开的地方。
/// 而接管恰好是「装了就想让它生效、卸载前又必须能撤回」的功能：
/// 看不见的控制面等于没有控制面，所以它必须在设置页里有一块位置。
fn integration_card(
    snapshot: &SettingsSnapshot,
    tokens: &Tokens,
    handlers: &SettingsHandlers,
) -> AnyElement {
    let menu = {
        let handler = Rc::clone(&handlers.toggle_folder_menu);
        widgets::switch(
            "setting-folder-menu",
            snapshot.folder_integration.context_menu,
            tokens,
            move |_: &ClickEvent, window, cx| handler(window, cx),
        )
    };

    let default = {
        let handler = Rc::clone(&handlers.toggle_folder_default);
        widgets::switch(
            "setting-folder-default",
            snapshot.folder_integration.default_handler,
            tokens,
            move |_: &ClickEvent, window, cx| handler(window, cx),
        )
    };

    let restore = {
        let handler = Rc::clone(&handlers.restore_folder_integration);
        widgets::text_button(
            "setting-folder-restore",
            "恢复",
            Some(IconName::RotateCcw),
            "删除本程序写入的注册表项，右键菜单与双击行为立刻回到系统默认",
            true,
            tokens,
            move |_: &ClickEvent, window, cx| handler(window, cx),
        )
    };

    card(
        "系统集成",
        "接管只写 HKCU，不需要管理员；移动或卸载本程序前，请先在这里恢复。",
        vec![
            setting_row(
                "在文件夹右键菜单中显示「用 Pagefold 打开」",
                "只在右键菜单里多一项，不改变任何默认行为",
                menu,
                false,
                tokens,
            ),
            setting_row(
                "设为文件夹默认打开程序",
                "桌面双击文件夹、以及其它程序调用 ShellExecute 时都会打开本应用，不再打开资源管理器",
                default,
                false,
                tokens,
            ),
            setting_row(
                "恢复资源管理器为默认",
                "一次撤销上面两项；程序被删除或移动后，也可以用命令行 --restore-folder-integration",
                restore,
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

/// 图标缩放的说明文字，随当前视图变化。
///
/// 缩放只作用于图标网格：在详细信息视图下改它，界面上不会有任何反馈。
/// 与其让用户以为设置没生效，不如把「要切到图标视图」写在说明里——
/// 这是设置页唯一能提供的即时解释。
fn zoom_hint(view_mode: ViewMode) -> SharedString {
    match view_mode {
        ViewMode::Icons => "图标视图的单元格边长：122 / 152 / 192px".into(),
        ViewMode::Details => {
            "仅影响图标视图。当前标签是详细信息视图，按 Ctrl+2 切换后可见（边长 122 / 152 / 192px）"
                .into()
        }
    }
}

/// 一行设置项：左侧标签 + 说明，右侧控件固定右对齐。
///
/// 行高随密度变化（基准 44px，即 `row_height + 12`）：设置页与列表用同一个
/// 密度档位，用户切换密度时整页的节奏也跟着变，而不是只有列表在变。
fn setting_row(
    label: &'static str,
    hint: impl Into<SharedString>,
    control: AnyElement,
    last: bool,
    tokens: &Tokens,
) -> AnyElement {
    let hint = hint.into();
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

/// 「快捷键」卡片：全局唤醒热键 + 应用内命令。
/// 「快捷键」卡片：全局唤醒热键 + 应用内命令。
///
/// 排版与其它设置卡保持一致的三条规则（之前这一张三项都没做到，看起来才「不协调」）：
///
/// 1. **左右各留 16px**：与 `setting_row` 相同，否则文字贴着卡片边框，
///    在宽窗口里会显得左一个贴边、右一个贴边；
/// 2. **每行之间画分隔线**：25 行文字堆在一起，没有横线就没有「一行一项」的感觉；
/// 3. **右侧控件成列**：键位胶囊定宽居中、开关与按钮各占固定宽度，
///    这样从上到下能看出三条竖直的列，而不是忽宽忽窄。
fn shortcuts_card(
    snapshot: &SettingsSnapshot,
    tokens: &Tokens,
    handlers: &SettingsHandlers,
) -> AnyElement {
    let shortcuts = &snapshot.shortcuts;

    let global_keys = fm_core::KeyCombo::parse(&shortcuts.global_keys)
        .map(|combo| combo.display())
        .unwrap_or_else(|| shortcuts.global_keys.clone());

    let edit_global = {
        let handler = Rc::clone(&handlers.edit_global_hotkey);
        edit_link("shortcut-edit-global", tokens, move |window, cx| {
            handler(window, cx)
        })
    };
    let toggle_global = {
        let handler = Rc::clone(&handlers.toggle_global_hotkey);
        widgets::switch(
            "shortcut-global-enabled",
            shortcuts.global_enabled,
            tokens,
            move |_: &ClickEvent, window, cx| handler(window, cx),
        )
    };

    // —— 应用内命令：按分组排列，组与组之间有一条稍重的分隔 ——
    let mut rows: Vec<AnyElement> = Vec::new();
    let mut current_group: Option<fm_core::Group> = None;
    let total = shortcuts.items.len();
    let mut index = 0usize;

    for item in &shortcuts.items {
        index += 1;
        if current_group != Some(item.group) {
            current_group = Some(item.group);
            rows.push(group_header(item.group.label(), index > 1, tokens));
        }

        // 每组最后一行不画底线：组头自带一条上边线，两条叠在一起会变成双线。
        let last_in_group = shortcuts
            .items
            .get(index)
            .is_none_or(|next| next.group != item.group);
        rows.push(shortcut_row(item, last_in_group, tokens, handlers));
    }
    debug_assert_eq!(index, total);

    let reset = {
        let handler = Rc::clone(&handlers.reset_shortcuts);
        widgets::text_button(
            "shortcut-reset",
            "恢复默认",
            Some(IconName::RotateCcw),
            "把全部快捷键（含全局唤醒）恢复成默认值",
            true,
            tokens,
            move |_: &ClickEvent, window, cx| handler(window, cx),
        )
    };

    // 冲突提示跨行汇总：撞车的两条可能相隔很远，标在某一行上会让人找不到另一条。
    let mut warnings: Vec<AnyElement> = Vec::new();
    if let Some(problem) = &shortcuts.global_problem {
        warnings.push(warning_line(format!("全局唤醒：{problem}"), tokens));
    }
    for conflict in &shortcuts.conflicts {
        warnings.push(warning_line(
            format!(
                "{} 被重复分配：{}",
                conflict.keys,
                conflict.labels.join("、")
            ),
            tokens,
        ));
    }

    card(
        "快捷键",
        "应用内快捷键在窗口有焦点时生效；全局唤醒热键在程序常驻后台时也能把窗口叫出来。改动立即生效。",
        vec![
            setting_row(
                "全局唤醒热键",
                "Win 组合被系统占用，无法注册；建议用 Ctrl / Alt 组合。",
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(kbd_hint(global_keys, tokens))
                    .child(toggle_global)
                    .child(edit_global)
                    .into_any_element(),
                false,
                tokens,
            ),
            div().flex().flex_col().w_full().children(rows).into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .px(px(16.0))
                .children(warnings)
                .into_any_element(),
            setting_row(
                "恢复默认",
                "清空所有自定义键位，包括全局唤醒热键。",
                reset,
                true,
                tokens,
            ),
        ],
        tokens,
    )
}

/// 分组标题：一条稍重的上边线 + 小号弱化文字。
///
/// 用「上边线」而不是「下边线」：组标题永远跟着它下面的第一行，
/// 画在下边会让人误以为它属于上一组。
fn group_header(label: &'static str, separated: bool, tokens: &Tokens) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px(px(16.0))
        .pt(px(if separated { 10.0 } else { 8.0 }))
        .pb(px(4.0))
        .when(separated, |element| {
            element.border_t_1().border_color(tokens.border)
        })
        .child(
            div()
                .text_size(px(metrics::FONT_XS))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(tokens.muted_fg)
                .child(label),
        )
        .into_any_element()
}

/// 一行应用内快捷键：名称 + 当前键位 + 启用开关 + 修改。
///
/// 右侧三件套都定宽，形成三列；否则「Ctrl + Shift + T」这类长键位会把开关与按钮
/// 挤得忽左忽右——那正是这一版之前的观感。
fn shortcut_row(
    item: &fm_core::ResolvedShortcut,
    last_in_group: bool,
    tokens: &Tokens,
    handlers: &SettingsHandlers,
) -> AnyElement {
    let id_for_edit = item.id.clone();
    let edit_handler = Rc::clone(&handlers.edit_shortcut);
    let edit = edit_link(
        format!("shortcut-edit-{}", item.id),
        tokens,
        move |window, cx| edit_handler(id_for_edit.clone(), window, cx),
    );

    let id_for_toggle = item.id.clone();
    let toggle_handler = Rc::clone(&handlers.toggle_shortcut);
    let toggle = widgets::switch(
        format!("shortcut-enabled-{}", item.id),
        item.enabled,
        tokens,
        move |_: &ClickEvent, window, cx| toggle_handler(id_for_toggle.clone(), window, cx),
    );

    // 禁用时整体弱化：它确实不生效，不该看起来和启用的一样。
    let label_color = if item.enabled {
        tokens.fg
    } else {
        tokens.muted_fg
    };

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .w_full()
        .px(px(16.0))
        .py(px(5.0))
        .min_h(px(tokens.row_height))
        .when(!last_in_group, |element| {
            element.border_b_1().border_color(tokens.border)
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .flex_grow(1.0)
                .min_w(px(0.0))
                .overflow_hidden()
                .child(
                    div()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_size(px(metrics::FONT_SM))
                        .text_color(label_color)
                        .child(item.label.clone()),
                )
                .when(item.danger, |element| {
                    element.child(
                        div()
                            .flex_none()
                            .text_size(px(metrics::FONT_XS))
                            .text_color(tokens.danger)
                            .child("危险"),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .justify_end()
                .w(px(KEY_COLUMN_WIDTH))
                .child(kbd_hint(item.display_keys(), tokens)),
        )
        .child(toggle)
        .child(edit)
        .into_any_element()
}

/// 键位列的宽度。
///
/// 取「`Ctrl + Shift + Delete` 这类最长组合」的大致像素宽：定宽之后每行的胶囊
/// 都能对齐，而不是被文字长度推得忽左忽右。
const KEY_COLUMN_WIDTH: f32 = 148.0;

/// 键位的展示样式：等宽 + 描边，像一枚键帽。
fn kbd_hint(keys: String, tokens: &Tokens) -> AnyElement {
    div()
        .flex_none()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(tokens.border)
        .bg(tokens.bg)
        .font_family(tokens.mono_font.clone())
        .text_size(px(metrics::FONT_XS))
        .text_color(tokens.muted_fg)
        .child(keys)
        .into_any_element()
}

/// 行尾的「修改」链接。
///
/// 刻意不用 `widgets::text_button`：那个带 tooltip，而设置页里 25 行各挂一个
/// tooltip，鼠标滚过列表时会不断创建/销毁 tooltip 实体——这也是「滚轮很卡」的来源之一。
/// 这里只保留 hover 变色，够用且便宜。
fn edit_link<F>(
    id: impl Into<gpui::ElementId>,
    tokens: &Tokens,
    on_click: F,
) -> AnyElement
where
    F: Fn(&mut Window, &mut App) + 'static,
{
    let muted = tokens.muted_fg;
    let foreground = tokens.fg;
    let hover = tokens.hover;

    div()
        .id(id)
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .w(px(44.0))
        .h(px(22.0))
        .rounded(px(metrics::RADIUS))
        .text_size(px(metrics::FONT_XS))
        .text_color(muted)
        .cursor_pointer()
        .hover(move |style| style.bg(hover).text_color(foreground))
        .on_click(move |_: &ClickEvent, window, cx: &mut App| on_click(window, cx))
        .child("修改")
        .into_any_element()
}

/// 一条冲突提示。统一用危险色：它们都意味着「这组键位现在不生效」。
fn warning_line(text: String, tokens: &Tokens) -> AnyElement {
    div()
        .text_size(px(metrics::FONT_XS))
        .text_color(tokens.danger)
        .child(text)
        .into_any_element()
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_state_is_only_complete_when_both_switches_are_on() {
        // 这两项的开关是独立的：只装右键菜单（零风险）与接管默认打开程序
        // （会改变双击行为）必须能分开看状态，否则用户无法判断「现在到底接管了多少」。
        let none = FolderIntegration::default();
        assert!(!none.is_fully_installed());

        let menu_only = FolderIntegration {
            context_menu: true,
            default_handler: false,
        };
        assert!(!menu_only.is_fully_installed());

        let full = FolderIntegration {
            context_menu: true,
            default_handler: true,
        };
        assert!(full.is_fully_installed());
    }

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
    fn zoom_hint_only_mentions_the_view_switch_in_details_mode() {
        // 图标视图下不需要解释；详细信息视图下必须给出「怎么才能看到效果」。
        assert!(!zoom_hint(ViewMode::Icons).contains("Ctrl+2"));
        assert!(zoom_hint(ViewMode::Details).contains("Ctrl+2"));
        // 两档都要写清具体边长，否则用户不知道三档到底差多少。
        for mode in [ViewMode::Icons, ViewMode::Details] {
            assert!(zoom_hint(mode).contains("122 / 152 / 192"));
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
            show_hidden: false,
            show_preview: true,
            view_mode: ViewMode::Details,
            config_path: None,
            version: "0.0.0".into(),
            folder_integration: FolderIntegration::default(),
            shortcuts: ShortcutsSnapshot::default(),
        };
        assert!(!snapshot.dark);

        let handlers = SettingsHandlers {
            set_dark: Rc::new(|_, _, _| {}),
            set_density: Rc::new(|_, _, _| {}),
            set_zoom: Rc::new(|_, _, _| {}),
            toggle_ext: Rc::new(|_, _| {}),
            toggle_hidden: Rc::new(|_, _| {}),
            toggle_preview: Rc::new(|_, _| {}),
            set_view_mode: Rc::new(|_, _, _| {}),
            copy_config_path: Rc::new(|_, _| {}),
            toggle_folder_menu: Rc::new(|_, _| {}),
            toggle_folder_default: Rc::new(|_, _| {}),
            restore_folder_integration: Rc::new(|_, _| {}),
            edit_shortcut: Rc::new(|_, _, _| {}),
            toggle_shortcut: Rc::new(|_, _, _| {}),
            edit_global_hotkey: Rc::new(|_, _| {}),
            toggle_global_hotkey: Rc::new(|_, _| {}),
            reset_shortcuts: Rc::new(|_, _| {}),
        };
        // 只验证字段类型能被调用，真正调用需要窗口句柄。
        assert!(Rc::strong_count(&handlers.set_dark) == 1);
    }
}
