//! 主窗口外壳视图：状态定义、动作与整体渲染入口。
//!
//! 布局与配色规范全部来自 `design/file-manager.html`：
//! 顶部「标题栏 + 标签条 + 命令条」三行，中部「侧边栏 + 内容区 + 预览面板」，
//! 底部状态栏。尺寸与色值不写死在这里，而是通过 [`fm_views::Tokens`] 取得，
//! 换主题或改密度时只有 `tokens.rs` 需要动。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use fm_core::naming;
use fm_core::{
    Column, Density, Entry, EntryKind, IconZoom, Location, Selection, SortKey, TabId, TabsState,
    ViewMode,
};
use fm_platform::{Drive, OpReport};
use fm_services::clipboard::{Clipboard, ClipboardMode, PastePlan};
use fm_services::config::Config;
use fm_services::directory::{DEFAULT_BATCH, DirectoryService, ScanBatch, SortSpec};
use fm_services::ops::{self, NewItem, OpResult};
use fm_services::session::SessionSnapshot;
use fm_views::preview_panel::{EntrySummary, PreviewContent};
use fm_views::{Tokens, widgets};
use gpui::{
    Anchor, App, AppContext as _, ClipboardItem, Context, Entity, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, ScrollStrategy,
    SharedString, Styled as _, Subscription, Task, UniformListScrollHandle, Window, div, px,
};
use gpui_component::button::ButtonVariant;
use gpui_component::dialog::DialogButtonProps;
use gpui_component::input::{InputEvent, InputState};
use gpui_component::notification::Notification;
use gpui_component::{ActiveTheme as _, Edges, Theme, ThemeMode, WindowExt as _};

use crate::layout::Viewport;

use crate::actions::{
    CloseTab, CopyItems, CutItems, DeleteItems, ExtendCursorDown, ExtendCursorUp, FocusAddressBar,
    GoBack, GoForward, GoUp, MoveCursorDown, MoveCursorUp, NewTab, OpenHelp, OpenItem, OpenSettings,
    PasteItems, Refresh, RenameItem, ReopenClosedTab, SelectAll, SetViewDetails, SetViewIcons,
    ToggleTheme,
};

/// 把「动作 → 视图方法」的绑定接到根元素上。
///
/// `on_action` 的监听器签名是 `Fn(&A, &mut Window, &mut App)`，拿不到
/// `&mut Context<ShellView>`，因此统一走 `WeakEntity::update` 回到视图，
/// 再由 `action` 执行（`action` 能拿到 `window`，供需要窗口的方法用）。
///
/// 用函数而不是宏：`action` 的闭包参数类型由 `impl Fn(...)` 的签名直接推断，
/// 调用点无需逐个标注，也避开了 `macro_rules!` 的标识符卫生问题。
fn bind_shortcut<A: gpui::Action>(
    root: gpui::Div,
    view: &gpui::WeakEntity<ShellView>,
    action: impl Fn(&mut ShellView, &mut Window, &mut Context<ShellView>) + 'static,
) -> gpui::Div {
    let view = view.clone();
    root.on_action(move |_: &A, window, cx: &mut App| {
        let _ = view.update(cx, |this, cx| {
            // 文本输入框聚焦时让位：文字编辑的按键优先于全局快捷键。
            if this.editing_input() {
                return;
            }
            action(this, window, cx);
        });
    })
}

/// 主窗口外壳。
///
/// 同时承担「视图」与「视图模型」两个角色：它从 [`DirectoryService`] 取数据、
/// 把数据渲染成界面，并把用户交互翻译成对自身状态的修改。
pub struct ShellView {
    pub(crate) service: Arc<DirectoryService>,
    pub(crate) tabs: TabsState,
    pub(crate) config: Config,
    pub(crate) drives: Vec<Drive>,
    pub(crate) quick_access: Vec<Entry>,
    /// 当前标签页已加载的**全部**条目。
    pub(crate) entries: Vec<Entry>,
    /// 搜索筛选后的可见条目。`None` 表示没有筛选条件，直接使用 `entries`。
    ///
    /// 用 `Option` 而不是「总是维护一份过滤结果」：未筛选时省掉一次数万条目的克隆，
    /// 而大目录恰恰是最需要省的地方。
    pub(crate) filtered: Option<Vec<Entry>>,
    /// 是否正在扫描（状态栏与内容区据此展示进度与骨架）。
    pub(crate) loading: bool,
    /// 最近一次扫描的错误信息。
    pub(crate) error: Option<SharedString>,
    /// 选中集。下标指向 [`Self::visible`] 的当前位置。
    pub(crate) selection: Selection,
    /// 焦点项下标（设计稿的 `.is-cur`）：与「选中」分开，为后续键盘导航留位。
    pub(crate) cursor: Option<usize>,
    /// 框选（橡皮筋）会话：左键按下未抬起时存在。
    ///
    /// 选中集的替换由 `update_rubber_band` 在鼠标滑过每一行时执行，抬起时由
    /// `end_rubber_band` 清空。锚点**只记不选**——立即选中会破坏 Ctrl / Shift
    /// 点击的语义（mousedown 先清掉多选，再被 click 的 toggle 覆盖）。
    pub(crate) rubber_band: Option<RubberBand>,
    /// 标签拖拽排序会话：左键按下未抬起时存在。
    pub(crate) tab_drag: Option<TabDrag>,
    /// 列宽拖拽会话：左键按在表头分隔线上时存在。
    ///
    /// 只存「从哪一列、哪个横坐标、起始宽度开始」，实时宽度写回
    /// `config.detail_columns`——因此拖动期间的重绘读到的永远是最新值，
    /// 松开后保存的也正是屏幕上那一条。
    pub(crate) column_drag: Option<ColumnDrag>,
    /// 搜索关键词。
    pub(crate) query: String,
    /// 右侧预览面板是否展开（宽屏下的持久化开关）。
    pub(crate) preview_open: bool,
    /// 窄屏下侧边栏浮层是否展开。宽屏不使用这个状态。
    pub(crate) sidebar_overlay: bool,
    /// 侧边栏「此电脑」分组的驱动器子项是否展开。
    pub(crate) drives_expanded: bool,
    /// 地址栏是否处于可编辑态。
    pub(crate) address_editing: bool,
    /// 地址栏输入框状态。进入编辑态时才创建，避免每次渲染都重建输入状态。
    pub(crate) address_input: Option<Entity<InputState>>,
    /// 搜索框是否处于可编辑态。
    pub(crate) search_editing: bool,
    pub(crate) search_input: Option<Entity<InputState>>,
    /// 应用内剪贴板（复制 / 剪切的暂存区）。
    ///
    /// 与 [`ShellView::selection`] 是两件不同的事：选中是「此刻界面上高亮的」，
    /// 剪贴板是「用户此前按过复制 / 剪切、还没粘贴掉的」。两者会自然分叉——
    /// 切标签页、刷新目录都会清空选中，但剪贴板必须留着，
    /// 否则「复制 → 切到另一个目录 → 粘贴」这条最基本的路径就不成立。
    pub(crate) clipboard: Clipboard,
    /// 正在内联重命名的条目。
    pub(crate) renaming: Option<RenameState>,
    /// 本次右键是否落在某个条目上（由条目上的处理器置位，空白处菜单的构造器消费）。
    ///
    /// 存在的理由：内容区容器挂着「空白处菜单」、每一行挂着「条目菜单」，而两者的
    /// 右键监听都注册在 `window.on_mouse_event` 上、冒泡阶段**逆序**执行——容器的
    /// 监听因为「先画子元素、后注册自己」排在最前。于是「在条目上按右键」会同时弹出
    /// 两个浮层，而且在行的处理器里 `stop_propagation` 已经来不及：祖先的浮层早被安排了。
    ///
    /// 因此换一个出口：置位这个标记，让空白处菜单的构造器交出一个**空菜单**，
    /// 组件库看到空菜单就不渲染浮层。见 `crate::context_menu::area_menu_builder`。
    pub(crate) area_menu_suppressed: bool,
    /// 写操作的后台任务。持有理由同 [`ShellView::scan_task`]：`Task` 一被丢弃就取消。
    op_task: Option<Task<()>>,
    /// 列表是否可能**不完整**（被我们主动取消了一次在途扫描）。
    ///
    /// 新建条目时为了让「建完立刻改名」成立，会把在途扫描作废并手动把新条目插进列表。
    /// 若那次扫描本来还没跑完，列表就停在半截——此时欠用户一次刷新，
    /// 在重命名会话结束（不管有没有真改名）时补上。
    list_incomplete: bool,
    /// 下一次扫描结束后要选中的路径。
    ///
    /// 写操作（新建、重命名）之后列表要重扫，而重扫是异步的：那一刻新条目
    /// 还不存在，没法立刻选中。因此把「该选谁」记下来，等最后一批数据落地时
    /// 再换算成下标（见 `apply_pending_select`）。
    ///
    /// 存路径而不是下标：排序与筛选都可能让同一个条目的下标变化。
    pending_select: Option<PathBuf>,
    /// 输入框事件订阅。
    ///
    /// **必须持有**：`Subscription` 一旦被 drop 就会退订，
    /// 后果是输入框再也不触发任何回调，表现为「能打字但界面没反应」。
    address_subscription: Option<Subscription>,
    search_subscription: Option<Subscription>,
    /// 在途扫描任务。
    ///
    /// **必须持有**：`Task` 一旦被 drop 就会取消，若只把它丢掉，
    /// 扫描会在发起后立刻中止，表现为「永远停在加载中」。
    scan_task: Option<Task<()>>,
    /// 当前帧使用的设计令牌。每帧渲染前刷新一次，子渲染函数只读。
    pub(crate) tokens: Tokens,
    /// 详情表与网格各自的滚动句柄。
    ///
    /// 两者**分开持有**：它们的虚拟化单位不同（详情表一行一个条目，网格一行多个
    /// 单元格），共用一个句柄会让切换视图时的滚动位置互相污染。
    ///
    /// 用途只有一个但很关键：把刚新建 / 刚改名的条目滚进视野。条目的位置由排序决定，
    /// 新建的那一条完全可能在视野之外——那样即使输入框已经拉起，用户也看不见，
    /// 表现为「点了新建没有任何反应」。
    pub(crate) details_scroll: UniformListScrollHandle,
    pub(crate) grid_scroll: UniformListScrollHandle,
    /// 最近一帧网格视图的列数。
    ///
    /// 缓存它而不是缓存整个视口：把条目下标换算成网格行下标只需要列数，
    /// 而那个换算会发生在**没有 window 的地方**（扫描收尾的异步回调里）。
    grid_columns: usize,
}

impl ShellView {
    /// 创建外壳，应用配置中的主题与外观档位，并立即发起首次扫描。
    ///
    /// 刻意不接收 `Window`：视图构造发生在 `cx.new(...)` 里，而 `cx.new` 的闭包
    /// 需要能长期存活，捕获 `&mut Window` 会带来不必要的生命周期约束。
    /// 扫描走 `cx.spawn`（App 级）而非 `cx.spawn_in`，因此也不需要窗口句柄。
    pub fn new(
        service: Arc<DirectoryService>,
        config: Config,
        initial_location: Option<Location>,
        cx: &mut Context<Self>,
    ) -> Self {
        // 驱动器与「快速访问」是启动期的一次性读取。
        // 已知停顿点：若挂载了失联的网络盘，容量查询最长可能阻塞数秒。
        // 后续可移到后台并先渲染骨架，当前先保证行为正确。
        let drives = service.file_system().drives().unwrap_or_else(|error| {
            tracing::warn!(%error, "驱动器枚举失败，驱动器分区将为空");
            Vec::new()
        });
        let quick_access = service.file_system().quick_access().unwrap_or_else(|error| {
            tracing::warn!(%error, "快速访问枚举失败，该分区将为空");
            Vec::new()
        });

        // 让配置里的主题在启动时就生效，避免先亮一帧浅色再切到深色。
        Theme::change(theme_mode(config.dark), None, cx);
        apply_component_theme(&config, cx);

        let tokens = build_tokens(&config, cx);
        let preview_open = config.show_preview;

        // 会话恢复的优先级：命令行位置 > 上次会话 > 默认主目录。
        // 命令行必须优先——「用叠页打开这个文件夹」是用户此刻的明确意图，
        // 不该被上次退出时的位置覆盖。因此有命令行参数时连会话文件都不读。
        let session = if initial_location.is_none() {
            SessionSnapshot::load()
        } else {
            None
        };
        let default_location = Location::from(default_start_path());
        let (tabs, restored_session) = match session {
            Some(session) => {
                let active = session.active;
                match TabsState::from_snapshots(session.tabs, active) {
                    Some(tabs) => (tabs, true),
                    None => {
                        // 坏会话只影响「回到上次位置」，不影响程序可用性。
                        tracing::warn!("会话内容无法恢复，本次启动回到默认标签");
                        (TabsState::new(default_location.clone()), false)
                    }
                }
            }
            None => (
                TabsState::new(initial_location.unwrap_or(default_location)),
                false,
            ),
        };

        let mut this = Self {
            service,
            tabs,
            config,
            drives,
            quick_access,
            entries: Vec::new(),
            filtered: None,
            loading: false,
            error: None,
            selection: Selection::default(),
            cursor: None,
            rubber_band: None,
            tab_drag: None,
            column_drag: None,
            query: String::new(),
            preview_open,
            sidebar_overlay: false,
            drives_expanded: true,
            address_editing: false,
            address_input: None,
            search_editing: false,
            search_input: None,
            clipboard: Clipboard::new(),
            renaming: None,
            area_menu_suppressed: false,
            list_incomplete: false,
            op_task: None,
            pending_select: None,
            address_subscription: None,
            search_subscription: None,
            scan_task: None,
            tokens,
            details_scroll: UniformListScrollHandle::new(),
            grid_scroll: UniformListScrollHandle::new(),
            // 真实值在第一帧渲染时写入；这里只给一个不会除零的初值。
            grid_columns: 1,
        };

        // 让上次退出时的视图模式在启动时就生效：`TabState` 只认识自己的默认值（图标视图），
        // 不把配置搬过来的话，用户每次启动都会被重置回图标视图。
        //
        // 从会话恢复时**不覆盖**：那份快照里每个标签都有自己的视图模式，
        // 用配置里的全局值统一改写，会让「详细信息」标签重启后变成图标视图。
        if !restored_session {
            let initial_view = this.config.view_mode;
            this.tabs.active_state_mut().set_view_mode(initial_view);
        }

        this.reload(cx);
        this
    }

    /// 刷新设计令牌。渲染的第一件事，保证子渲染函数读到的是当前主题的色值。
    pub(crate) fn refresh_tokens(&mut self, cx: &App) {
        self.tokens = build_tokens(&self.config, cx);
    }
}

/// 从配置与组件库主题推导令牌。
///
/// 字体族取自组件库主题：`Theme::change` 会把 `.SystemUIFont` 解析成实际安装的族名
/// （本机是 Microsoft YaHei UI），等宽字体同理。令牌层因此不必自己查系统字体。
pub(crate) fn build_tokens(config: &Config, cx: &App) -> Tokens {
    let theme = cx.theme();
    Tokens::new(
        config.dark,
        config.density,
        config.zoom,
        theme.font_family.clone(),
        theme.mono_font_family.clone(),
    )
}

/// 把设计令牌里的关键色值与圆角同步给组件库主题。
///
/// 之所以需要这一步：地址栏与搜索框用的是组件库的 `Input`，它的描边、底色、圆角
/// 都读 `Theme`。不同步的话，这一个控件会与周围的「设计稿配色」明显割裂。
/// 只同步会影响观感的字段，不做全量覆盖——组件库还有通知层等自用色。
pub(crate) fn apply_component_theme(config: &Config, cx: &mut App) {
    let tokens = Tokens::new(
        config.dark,
        config.density,
        config.zoom,
        cx.theme().font_family.clone(),
        cx.theme().mono_font_family.clone(),
    );
    let theme = Theme::global_mut(cx);
    theme.background = tokens.bg;
    theme.foreground = tokens.fg;
    theme.muted = tokens.hover;
    theme.muted_foreground = tokens.muted_fg;
    theme.border = tokens.border;
    theme.input = tokens.bg;
    theme.ring = tokens.accent_line;
    theme.primary = tokens.accent;
    theme.primary_hover = mix(tokens.accent, tokens.fg, 0.14);
    theme.primary_active = mix(tokens.accent, tokens.fg, 0.28);
    theme.primary_foreground = tokens.surface;
    theme.accent = tokens.accent;
    theme.accent_foreground = tokens.surface;
    theme.danger = tokens.danger;
    theme.danger_foreground = tokens.surface;
    theme.popover = tokens.surface;
    theme.popover_foreground = tokens.fg;
    theme.list_hover = tokens.hover;
    theme.list_active = tokens.accent_soft;
    theme.list_active_border = tokens.accent_line;
    theme.list_head = tokens.panel;
    theme.radius = px(fm_views::metrics::RADIUS);
    theme.radius_lg = px(fm_views::metrics::RADIUS_LG);
    // 通知（toast）的落点与设计稿的 `.toasts` 对齐：左下角、左 14px、底 40px
    // （设计稿是 `left:14px;bottom:40px`，40 是为了避开 26px 高的状态栏）。
    // 组件库默认在**右上角**，不同步的话提示会跑到与设计稿相反的一侧。
    theme.notification.placement = Anchor::BottomLeft;
    theme.notification.margins = Edges {
        top: px(0.0),
        right: px(0.0),
        bottom: px(40.0),
        left: px(14.0),
    };
    // 设计稿给 toast 的上限是 `min(420px, 100vw - 28px)`，组件库默认 382px。
    theme.notification.width = px(420.0);
}

/// 两个颜色按比例混合（`weight` 是后者的占比）。
///
/// 设计稿的 `color-mix(in oklch, X 90%, Y)` 只用于按钮的按下态；在 HSL 空间做线性插值
/// 足以表达「更沉一点的强调色」，无需为此再实现一遍 OKLCH 往返。
pub(crate) fn mix(base: gpui::Hsla, other: gpui::Hsla, weight: f32) -> gpui::Hsla {
    let weight = weight.clamp(0.0, 1.0);
    gpui::Hsla {
        h: base.h + (other.h - base.h) * weight,
        s: base.s + (other.s - base.s) * weight,
        l: base.l + (other.l - base.l) * weight,
        a: base.a + (other.a - base.a) * weight,
    }
}

/// 布尔值到主题模式的映射，供启动与切换两处共用，避免两处判断逻辑漂移。
pub(crate) fn theme_mode(dark: bool) -> ThemeMode {
    if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}

/// 启动时的初始目录：优先用户主目录，取不到则退回当前盘根目录。
fn default_start_path() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("C:\\"))
}

/// 一次内联重命名的会话。
///
/// 生命周期与「正在编辑的那一行」严格一致：开始重命名时建立，提交或取消时销毁。
/// 输入框实体必须活过整段会话，因此由 shell 持有而不是在渲染时创建——
/// 行是每帧重建的，在那里 `cx.new` 会让光标与已输入的文字每帧清零。
pub(crate) struct RenameState {
    /// 目标在 [`ShellView::visible`] 里的下标。**只用于渲染**：决定哪一行
    /// 把名称换成输入框。提交时改用 `path`——编辑期间目录可能被重扫，
    /// 下标会失效，而路径不会。
    pub(crate) index: usize,
    /// 目标路径。渲染层也要读它：判断输入框该落在哪一行时，光比下标不够可靠。
    pub(crate) path: PathBuf,
    /// 输入框实体。
    pub(crate) input: Entity<InputState>,
    /// 输入框事件订阅。
    ///
    /// **必须持有**：`Subscription` 一被丢弃就退订，表现为「能打字，
    /// 但回车与失焦都没反应」——与地址栏那个坑同源。
    _subscription: Subscription,
}

/// 一次框选（橡皮筋）会话。
pub(crate) struct RubberBand {
    /// 锚点下标（`visible()` 坐标）。
    pub(crate) anchor: usize,
    /// 按下时的窗口坐标，用来算「是否真的拖动了」。
    start: Point<Pixels>,
    /// 是否已超过拖动阈值。超过后才真正开始替换选中——否则一次普通的单击
    /// （鼠标几乎不动）也会因为微小的抖动被当成框选，破坏 Ctrl / Shift 点击的语义。
    active: bool,
}

/// 框选的拖动阈值（px）。低于它的一律按单击处理。
const RUBBER_BAND_THRESHOLD: f32 = 4.0;

/// 一次列宽拖拽会话。
pub(crate) struct ColumnDrag {
    /// 正在被拖的列。
    pub(crate) column: Column,
    /// 按下时的窗口横坐标（px）。
    start_x: f32,
    /// 按下时该列的实际宽度（可能是断点默认值，也可能是上次拖动的结果）。
    start_width: f32,
}

impl ColumnDrag {
    /// 由当前横坐标算出目标宽度。**不做钳制**：区间由
    /// [`fm_core::ColumnWidths::set`] 负责，避免同一规则写两遍。
    fn width_at(&self, x: f32) -> f32 {
        self.start_width + (x - self.start_x)
    }
}

/// 一次标签拖拽排序会话。
pub(crate) struct TabDrag {
    /// 被拖拽的标签。
    pub(crate) tab: TabId,
    /// 按下时的窗口坐标，用来算「是否真的拖动了」。
    start: Point<Pixels>,
    /// 是否已超过拖动阈值。超过后才真正开始重排——否则一次单击也会被当成拖拽。
    active: bool,
}

/// 标签拖拽的阈值（px）。低于它的一律按单击处理。
const TAB_DRAG_THRESHOLD: f32 = 4.0;

/// 写操作：新建、复制、剪切、粘贴、重命名、移入回收站。
///
/// 六条路径共用同一个骨架：**后台线程执行 → 回到视图弹提示 → 刷新列表**。
/// 三步都不能省：复制一个大目录是秒级操作，放在 UI 线程上就是一次肉眼可见的
/// 卡死；不弹提示，用户不知道那一秒里发生了什么；不刷新，界面与磁盘就不一致。
///
/// 这一层因此必须拿到 `Window`：提示挂在窗口上，而异步上下文里的窗口只能通过
/// `spawn_in` / `update_in` 取得（`Context::spawn` 那条路径上没有窗口）。
impl ShellView {
    /// 当前所在目录。
    ///
    /// 虚拟位置（「此电脑」、回收站）返回 `None`：它们不是真实目录，
    /// 任何「在当前位置新建 / 粘贴」都无处落笔。
    pub(crate) fn current_dir(&self) -> Option<PathBuf> {
        self.tabs
            .active_state()
            .location()
            .as_dir()
            .map(Path::to_path_buf)
    }

    /// 当前选中项的路径，按可见顺序。
    ///
    /// 取 `visible()` 而不是 `entries`：筛选状态下用户看到的是 `visible()`，
    /// 被操作的也必须是同一批，否则会出现「删掉了一个屏幕上没有的东西」。
    pub(crate) fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection
            .iter()
            .filter_map(|index| self.visible().get(index))
            .map(|entry| entry.path.as_ref().clone())
            .collect()
    }

    /// 把某个条目滚进可视区。
    ///
    /// 条目的位置由排序决定，新建的那一条完全可能在视野之外——那样即使输入框
    /// 已经拉起，用户也看不见，表现为「点了新建没有任何反应」。
    ///
    /// 用 `Nearest` 且**非 strict** 语义：条目已经完全可见时不做任何滚动，
    /// 新建一个本来就在视野里的条目不该把列表拽走。
    fn scroll_into_view(&self, index: usize) {
        let (handle, row) = match self.tabs.active_state().view_mode() {
            ViewMode::Details => (&self.details_scroll, index),
            // 网格的虚拟化单位是行：第 N 个条目落在第 `N / 列数` 行。
            ViewMode::Icons => (&self.grid_scroll, index / self.grid_columns.max(1)),
        };
        handle.scroll_to_item(row, ScrollStrategy::Nearest);
    }

    /// 清空搜索筛选。
    fn clear_filter(&mut self) {
        self.query.clear();
        self.search_editing = false;
        self.filtered = None;
    }

    /// 在当前位置新建一个项目。
    pub(crate) fn create(&mut self, item: NewItem, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.current_dir() else {
            return;
        };
        let fs = Arc::clone(self.service.file_system());

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::create_with_entry(&*fs, &dir, item) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| {
                view.finish_create(window, cx, result);
            });
        }));
    }

    /// 新建完成后的收尾。
    ///
    /// 与其它写操作不同，这里**不重新扫描**：等一轮扫描要几百毫秒，而设计稿要求
    /// 「建完立刻就能改名」。因此把新建的那一条直接插进列表（元信息由 `stat` 取真），
    /// 选中它，然后立即进入重命名。下一次刷新（或改名提交时的重扫）会把列表
    /// 对齐到磁盘的真实状态。
    fn finish_create(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        result: OpResult<(PathBuf, Entry)>,
    ) {
        let (path, mut entry) = match result {
            Ok(created) => created,
            Err(reason) => {
                window.push_notification(Notification::error(reason), cx);
                return;
            }
        };

        // 与扫描路径上完全一致地补上扩展名句柄：类型列与图标都依赖它。
        self.service.attach_extension(&mut entry);

        // 清掉筛选条件：新项目很可能不匹配当前关键词，那会变成「建完了却看不见，
        // 更没法给它改名」。设计稿的 `makeFolder()` 同样会清空搜索框。
        self.clear_filter();

        // 这一批条目现在由我们说了算：让在途扫描失效，否则它剩下的批次回来时会与
        // 手动插入的这一条**重复**，甚至把 `entries` 整个清掉。
        // 代价是列表可能停在半截——记下来，等重命名会话结束时补一次刷新。
        if self.loading {
            self.list_incomplete = true;
        }
        self.service.begin_scan();
        self.loading = false;

        let sort = {
            let tab = self.tabs.active();
            SortSpec::new(tab.state().sort_key(), tab.state().sort_direction())
        };
        self.service
            .insert_sorted(&mut self.entries, entry, sort);

        // 选中新建的这一条，并**先把它滚进视野**：它的位置由排序决定，
        // 完全可能落在视野之外——那样即便输入框已经拉起，用户也看不见。
        if let Some(index) = self
            .visible()
            .iter()
            .position(|entry| entry.path.as_ref() == &path)
        {
            self.selection.select_single(index);
            self.cursor = Some(index);
            self.scroll_into_view(index);
        }

        // 三种新建都**立刻进入重命名**，且不再弹提示——输入框本身就是反馈，
        // 再弹一句「已新建」等于两条消息说同一件事。没有人想要一个叫
        // 「新建文件夹」或「新建文本文档.txt」的东西。
        //
        // 这里**刻意与设计稿不同**：设计稿的 `makeFile` 只弹提示、不进改名。
        // 但同一个「新建」动作在文件夹与文件上给出两种反馈没有道理，而
        // 「新建文本文档 (2).txt」这种带序号的默认名反而更需要马上改。
        // 资源管理器也是这么做的（右键「新建文本文档」同样直接进改名）。
        self.begin_rename(path, window, cx);
    }

    /// 重命名当前唯一选中的项（选中条上的按钮）。
    pub(crate) fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        // 只处理「恰好一项」：按钮在多选下本来就是禁用的，
        // 而「随便挑一项改名」这种自作主张比什么都不做更糟。
        let [path] = paths.as_slice() else {
            return;
        };
        self.begin_rename(path.clone(), window, cx);
    }

    /// 把选中项放进剪贴板（复制或剪切）。
    ///
    /// 这是纯内存操作，没有 IO，因此**不需要后台线程**：提示可以当场弹出来，
    /// 用户点完立刻看到反馈，而不是等一次异步往返。
    pub(crate) fn clip_selection(
        &mut self,
        mode: ClipboardMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(origin) = self.current_dir() else {
            window.push_notification(
                Notification::warning("当前位置的项目不支持复制或剪切"),
                cx,
            );
            return;
        };

        let paths = self.selected_paths();
        let count = paths.len();
        // 空选中会被 `put` 转成「清空剪贴板」，因此这里只判断要不要提示。
        self.clipboard.put(mode, paths, origin);

        if count > 0 {
            // 文案与设计稿的 `toast('已复制 N 项到剪贴板')` 一致。
            window.push_notification(
                Notification::success(format!("已{} {count} 项到剪贴板", mode.verb())),
                cx,
            );
        }

        // 剪贴板的状态决定空白处菜单里「粘贴」是否禁用，必须重绘。
        cx.notify();
    }

    /// 粘贴到当前目录。
    pub(crate) fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dir) = self.current_dir() else {
            // 菜单里那一项本来就是禁用的；这里再兜一次，是因为
            // 「菜单开着的时候用户切到了回收站」是可能的。
            window.push_notification(Notification::warning("当前位置不能粘贴"), cx);
            return;
        };

        // 「能不能粘、粘是复制还是移动」是纯逻辑，判定放在 `Clipboard` 里
        // （有单测），这里只消费结果。
        let plan = match self.clipboard.plan(&dir) {
            Ok(plan) => plan,
            Err(error) => {
                window.push_notification(Notification::warning(error.message()), cx);
                return;
            }
        };

        let verb = match plan {
            PastePlan::Copy { .. } => ClipboardMode::Copy.action(),
            PastePlan::Move { .. } => ClipboardMode::Cut.action(),
        };
        let fs = Arc::clone(self.service.file_system());
        // 后台线程要一份独立所有权；外层留一份，用于收尾时结算剪贴板。
        let planned = plan.clone();

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::paste(&*fs, &planned) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| {
                view.finish_paste(window, cx, &plan, verb, result);
            });
        }));
    }

    /// 粘贴完成后的收尾。
    fn finish_paste(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        plan: &PastePlan,
        verb: &str,
        result: OpResult<OpReport>,
    ) {
        match result {
            Ok(report) => {
                // 剪贴板怎么收尾的规则（剪切只在**全部成功**时失效，
                // 部分失败留着重试）在 `Clipboard::complete` 里，那条规则有单测。
                self.clipboard.complete(plan, &report);
                self.notice_report(window, cx, verb, &report);
                self.refresh_after_write(cx);
            }
            Err(reason) => window.push_notification(Notification::error(reason), cx),
        }
    }

    /// 弹一条写操作的回执提示。
    ///
    /// 部分失败用 `warning` 而不是 `error`：那批操作确实完成了大部分，
    /// 用错误色会让用户以为整个操作都失败了。
    fn notice_report(&self, window: &mut Window, cx: &mut App, verb: &str, report: &OpReport) {
        let text = ops::summarize(verb, report);
        if report.has_failures() {
            // toast 只报第一条原因（一行字里罗列五个原因没人会看），
            // 剩下的必须有去处：日志。否则那些失败就彻底消失了。
            for (path, reason) in &report.failed {
                tracing::warn!(path = %path.display(), reason = %reason, "写操作存在失败项");
            }
            window.push_notification(Notification::warning(text), cx);
        } else {
            window.push_notification(Notification::success(text), cx);
        }
    }

    /// 开始内联重命名。
    ///
    /// 参数是路径而不是下标：右键菜单里的动作在构造时就捕获了条目快照，
    /// 到用户点击时列表可能已经刷过一轮，下标不再可靠。
    pub(crate) fn begin_rename(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .visible()
            .iter()
            .position(|entry| entry.path.as_ref() == &path)
        else {
            return;
        };
        let name = self.visible()[index].name.to_string();

        // 同一时刻只允许一个输入框：先结束上一个会话，否则两个 `InputState`
        // 可能同时持有焦点，按键会进到看不见的那一个里。
        self.renaming = None;

        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(name.clone())
                // Esc 的行为：开启它之后，组件库的输入框会在按下 Esc 时**清空内容**
                // （本项目的搜索框用的也是它）。设计稿的「Esc 直接收起输入框」
                // 需要全局键位，而 `crates/app/src/actions.rs` 目前是空壳，
                // 属后续增量；在那之前「Esc 清空 → 失焦放弃」是能提供的最近似行为
                // （见 `commit_rename`：空名按放弃处理，不会被提交上去）。
                .clean_on_escape()
        });

        // 预选主干、不含扩展名：与资源管理器一致，直接把新名字敲进去
        // 不会把 `.txt` 弄丢。`set_selected_range` 收的是 UTF-8 字节偏移，
        // 而它自己会把落在字符中间的偏移夹到字符边界，因此中文名也安全。
        let selection = naming::rename_selection_range(&name);
        input.update(cx, |state, cx| state.set_selected_range(selection, cx));

        let subscription = cx.subscribe_in(
            &input,
            window,
            move |this, _state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.commit_rename(window, cx),
                // 失焦即提交，而不是取消：设计稿的 `el.onblur` 就是这么写的，
                // 资源管理器也是这个行为（点了别处就当作「改完了」）。
                InputEvent::Blur => this.commit_rename(window, cx),
                InputEvent::Change | InputEvent::Focus => {}
            },
        );

        self.renaming = Some(RenameState {
            index,
            path,
            input: input.clone(),
            _subscription: subscription,
        });

        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    /// 提交内联重命名。
    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // `take` 同时起到「只提交一次」的保护：回车与失焦会接连触发，
        // 第二次进来时状态已空，直接返回。
        let Some(renaming) = self.renaming.take() else {
            return;
        };

        let typed = renaming.input.read(cx).value().to_string();
        let old_name = renaming
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let trimmed = typed.trim();

        // 两种情况都算「用户没改」：清空（设计稿的 `if(!name){renderList();return}`
        // 就是放弃），以及与原文相同。此时不该弹提示——他什么也没做，
        // 一句「已重命名为 X」只会让人以为是系统改的。
        if trimmed.is_empty() || trimmed == old_name {
            // 用户什么也没改。但列表可能停在半截（新建时作废过一次在途扫描），
            // 那种情况下欠他一次刷新——否则他会一直看着一个不完整的目录，
            // 而且没有任何迹象说明「再刷新一下就好了」。
            if self.list_incomplete {
                self.list_incomplete = false;
                self.refresh_after_write(cx);
            }
            cx.notify();
            return;
        }

        let fs = Arc::clone(self.service.file_system());
        let path = renaming.path.clone();
        // 输入的名字既要在后台线程里当参数，又要作为「拿不到文件名」时的兜底，
        // 因此先克隆一份留给收尾闭包。
        let fallback_name = typed.clone();

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::rename(&*fs, &path, &typed) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| match result {
                Ok(new_path) => {
                    let name = new_path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or(fallback_name);
                    // 文案与设计稿的 `toast('已重命名为「X」')` 一致。
                    // 设计稿还带一个「撤销」按钮，那需要一套撤销栈，属后续增量。
                    window
                        .push_notification(Notification::success(format!("已重命名为「{name}」")), cx);
                    view.pending_select = Some(new_path);
                    view.refresh_after_write(cx);
                }
                // 名称非法、目标已存在等都在这里被拦下并如实告知。
                // 刻意不重新打开输入框让用户改：焦点已经不在那一行上了，
                // 重新聚焦会让人以为程序在抢键盘。
                Err(reason) => window.push_notification(Notification::error(reason), cx),
            });
        }));
    }

    /// 把选中项移入回收站。**先弹确认框。**
    ///
    /// 为什么危险动作要单独一个对话框，而不是像设计稿那样只把菜单项染红：
    /// 染红只说了「有点危险」，而对话框能说清「要把哪一项移入回收站」，
    /// 并把误点的代价从「东西进回收站了」降到「多点一下」。
    pub(crate) fn delete_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() || self.current_dir().is_none() {
            return;
        }

        // 确认文案：单项时写出名字，多项时报数量。
        // 「将「报告.docx」移到回收站？」能让人在按下确定之前确认自己选对了东西——
        // 这正是确认框存在的意义；只报「1 个项目」就丢掉了这个价值。
        let subject = if paths.len() == 1 {
            paths[0]
                .file_name()
                .map(|name| format!("「{}」", name.to_string_lossy()))
                .unwrap_or_else(|| String::from("该项目"))
        } else {
            format!("这 {} 个项目", paths.len())
        };

        // 对话框的回调里拿不到 `this`，因此先留下一个弱引用。
        let view = cx.entity().downgrade();

        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            // 构建闭包是 `Fn`，因此每次调用都要克隆一份给内层闭包。
            let view = view.clone();
            let paths = paths.clone();

            alert
                .title("移到回收站")
                .description(format!("将{subject}移到回收站？"))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("移到回收站")
                        // 对应设计稿的 `confirmDialog(..., okText, danger)`：
                        // 危险动作的确定按钮用危险色，而不是与普通确认同色。
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("取消")
                        .show_cancel(true),
                )
                .on_ok(move |_event, window, cx| {
                    let _ =
                        view.update(cx, |view, cx| view.start_recycle(paths.clone(), window, cx));
                    // true = 关闭对话框。
                    true
                })
        });
    }

    /// 真正发起移入回收站（用户确认之后）。
    fn start_recycle(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fs = Arc::clone(self.service.file_system());

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::recycle(&*fs, &paths) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| match result {
                Ok(count) => {
                    window.push_notification(
                        Notification::success(format!("已移到回收站 {count} 项")),
                        cx,
                    );
                    view.refresh_after_write(cx);
                }
                // 回收站的失败**必须**让用户看见：它是「我以为删掉了」的
                // 最后一道心理防线，不能悄悄吞掉。
                Err(reason) => window.push_notification(Notification::error(reason), cx),
            });
        }));
    }
}

/// 扫描与虚拟位置加载。
impl ShellView {
    /// 刷新当前位置。
    ///
    /// 真实目录走扫描；「此电脑」这类虚拟位置走驱动器枚举。
    /// 这里刻意不用「把虚拟位置当成一个假路径去扫描」的做法——
    /// 那样只会得到一个「路径不存在」的错误，而不是有意义的界面。
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        self.reload_inner(cx, true);
    }

    /// 写操作之后的刷新：**保留搜索关键词**。
    ///
    /// 与 [`ShellView::reload`] 只差这一点。用户主动刷新或换了位置时清掉筛选条件是
    /// 合理的（那个词在新位置可能毫无意义），但写操作之后清掉它就很莫名其妙——
    /// 他只是改了个文件名，搜索框却空了。
    ///
    /// 例外是「新建」：那条路径会显式调 [`ShellView::clear_filter`]，
    /// 因为刚建出来的项目很可能不匹配当前关键词（设计稿也这么做）。
    pub(crate) fn refresh_after_write(&mut self, cx: &mut Context<Self>) {
        self.reload_inner(cx, false);
    }

    /// 刷新的共同实现。`clear_filter` 决定要不要顺带清掉搜索条件。
    fn reload_inner(&mut self, cx: &mut Context<Self>, clear_filter: bool) {
        let tab = self.tabs.active();
        let location = tab.state().location().clone();
        let sort = SortSpec::new(tab.state().sort_key(), tab.state().sort_direction());

        // 导航会换掉整份条目，任何基于下标的选中与筛选都必须一起清掉；
        // 留着它们会让「上一层的第 3 项」在新目录里被当作选中项高亮，属于纯粹的错乱。
        self.clear_selection_state();
        if clear_filter {
            self.clear_filter();
        }

        match location {
            Location::Computer => self.begin_drives_load(cx),
            Location::RecycleBin => {
                self.begin_recycle_bin_load(sort, cx);
            }
            Location::Directory(dir) => {
                self.begin_scan(dir, sort, cx);
            }
            // 设置页没有内容需要加载（`render_content` 直接渲染它），
            // 但必须清掉上一次的条目：否则从目录切过来时，
            // 状态栏与预览面板仍会显示上一个目录的统计数据。
            Location::Settings => {
                self.entries.clear();
                self.filtered = None;
                self.loading = false;
                self.error = None;
            }
        }

        // 导航、切换标签、开关标签都会经过这里，因此会话写盘只需要这一个挂点。
        self.persist_session();
    }

    /// 当前会话的快照。
    fn session_snapshot(&self) -> SessionSnapshot {
        SessionSnapshot::new(
            self.tabs
                .tabs()
                .iter()
                .map(|tab| tab.state().snapshot())
                .collect(),
            self.tabs.active_index(),
        )
    }

    /// 把会话写入磁盘（`%APPDATA%\pagefold\session.json`）。
    ///
    /// 同步写，与 `Config::save` 同一策略：快照是 KB 级 JSON，
    /// 而异步去抖会引入「关窗瞬间丢掉最后一次导航」这条只在特定时序下出现的边界。
    fn persist_session(&self) {
        self.session_snapshot().save();
    }

    /// 扫描结束后，把 `pending_select` 指向的条目选中。
    ///
    /// 用路径换算下标，而不是把下标一路传下来：扫描是异步的，
    /// 回来时排序可能已经变化、筛选也可能把中间的项目去掉。
    fn apply_pending_select(&mut self) {
        let Some(path) = self.pending_select.take() else {
            return;
        };
        let Some(index) = self
            .visible()
            .iter()
            .position(|entry| entry.path.as_ref() == &path)
        else {
            // 找不到就静默放弃。最常见的原因是它被当前的搜索条件挡住了——
            // 那不是错误，弹一句「找不到刚创建的项目」只会让人困惑。
            return;
        };
        self.selection.select_single(index);
        self.cursor = Some(index);
        // 顺便带进视野：改完名字之后视线还在那一行上，但重扫可能因为排序或
        // 筛选条件的变化把它挪到了别处。
        self.scroll_into_view(index);
    }

    /// 发起一次目录扫描，返回本次加载的代数。
    pub(crate) fn begin_scan(
        &mut self,
        dir: PathBuf,
        sort: SortSpec,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.begin_load(LoadSource::Directory(dir), sort, cx)
    }

    /// 加载回收站条目，返回本次加载的代数。
    fn begin_recycle_bin_load(&mut self, sort: SortSpec, cx: &mut Context<Self>) -> u64 {
        self.begin_load(LoadSource::RecycleBin, sort, cx)
    }

    /// 发起一次内容加载，返回本次加载的代数。
    ///
    /// 副作用上，它会立刻让所有在途加载失效——这是「快速连续切换位置时旧结果不错乱」的保障。
    ///
    /// 真实目录与回收站共用这一个函数：两者的收尾（分批并入、代数校验、错误处理、
    /// 兜底收尾）完全一样，差别只在「从哪儿取条目」。
    fn begin_load(
        &mut self,
        source: LoadSource,
        sort: SortSpec,
        cx: &mut Context<Self>,
    ) -> u64 {
        // 错误文案需要它，而此刻 `source` 还没被移进下面的异步块。
        let error_label = source.error_label();
        let generation = self.service.begin_scan();

        self.entries.clear();
        self.loading = true;
        self.error = None;
        cx.notify();

        let service = Arc::clone(&self.service);
        let batch = DEFAULT_BATCH;

        self.scan_task = Some(cx.spawn(async move |this, cx| {
            // 真正的 IO 与排序都发生在后台线程，UI 线程全程不被阻塞。
            let (batches, failure) = cx
                .background_executor()
                .spawn(async move {
                    let mut collected: Vec<ScanBatch> = Vec::new();
                    // 只建一个 sink 给两个来源共用：分别内联两个闭包会让
                    // `collected` 在两个分支里各被可变借一次，也更容易写错。
                    let mut sink = |batch: ScanBatch| {
                        collected.push(batch);
                        true
                    };
                    let failure = match &source {
                        LoadSource::Directory(dir) => {
                            service.scan(dir, generation, sort, batch, &mut sink)
                        }
                        LoadSource::RecycleBin => {
                            service.recycle_bin(generation, sort, batch, &mut sink)
                        }
                    }
                    .err();
                    (collected, failure)
                })
                .await;

            if let Some(failure) = failure {
                let message: SharedString = format!("{error_label}：{failure}").into();
                let _ = this.update(cx, move |view, cx| {
                    // 只有这次扫描仍是最新时才写错误，否则会覆盖掉新扫描的状态。
                    if view.service.is_current(generation) {
                        view.loading = false;
                        view.error = Some(message);
                        // 这一轮扫描没有产出任何条目，因此「等扫描完成后选中它」
                        // 这个请求不可能被满足。留着它，下一次（可能是很久以后、
                        // 也可能因为别的原因触发的）扫描成功时会突然选中一个
                        // 与用户当下动作无关的条目。
                        view.pending_select = None;
                        cx.notify();
                    }
                });
                return;
            }

            // 回到前台逐批并入视图。每批之后让出一次执行权，让 UI 有机会绘制，
            // 从而真正形成「分批呈现」而不是最后一帧一次性全亮。
            for batch in batches {
                let batch_generation = batch.generation;
                let is_last = batch.is_last;
                let entries = batch.entries;

                let applied = this.update(cx, move |view, cx| {
                    // 代数已变：用户在扫描期间又切了目录，这批结果直接丢。
                    if !view.service.is_current(batch_generation) {
                        return;
                    }
                    view.entries.extend(entries);
                    // 已有筛选条件时才重算：无筛选时 `filtered` 恒为 None，
                    // 每批都克隆一次整表会白白吃掉大目录的性能预算。
                    view.refresh_filter();
                    if is_last {
                        view.loading = false;
                        // 扫描跑完最后一批 = 列表已经完整。
                        view.list_incomplete = false;
                        // 写操作之后的重新选中只能等到这里：在此之前新条目
                        // 还不在列表里，没有下标可选。
                        view.apply_pending_select();
                    }
                    cx.notify();
                });

                if applied.is_err() {
                    // 视图已销毁（窗口关闭），停止后续更新。
                    return;
                }

                cx.background_executor()
                    .timer(Duration::from_millis(0))
                    .await;
            }

            // 兜底收尾：若所有批次都因代数失效被丢弃，也要让骨架屏停下来，
            // 否则界面会永久停在加载态。
            let _ = this.update(cx, move |view, cx| {
                if view.service.is_current(generation) && view.loading {
                    view.loading = false;
                    cx.notify();
                }
            });
        }));

        generation
    }

    /// 重新枚举驱动器（「此电脑」视图与刷新动作共用）。
    ///
    /// 同样走后台线程：网络盘与空光驱的容量查询可能阻塞数秒，
    /// 放在 UI 线程会让整个界面卡住。`generation` 机制在这里同样生效——
    /// 枚举期间用户若切走，这批结果会被丢弃。
    fn begin_drives_load(&mut self, cx: &mut Context<Self>) {
        let generation = self.service.begin_scan();

        self.entries.clear();
        self.loading = true;
        self.error = None;
        cx.notify();

        let file_system = Arc::clone(self.service.file_system());
        self.scan_task = Some(cx.spawn(async move |this, cx| {
            let drives = cx
                .background_executor()
                .spawn(async move { file_system.drives() })
                .await;

            let _ = this.update(cx, move |view, cx| {
                if !view.service.is_current(generation) {
                    return;
                }
                view.loading = false;
                match drives {
                    Ok(drives) => {
                        tracing::info!(drives = drives.len(), "驱动器枚举完成");
                        view.drives = drives;
                    }
                    Err(error) => {
                        view.error = Some(format!("无法枚举驱动器：{error}").into());
                    }
                }
                cx.notify();
            });
        }));
    }
}

/// 供视图消费的派生数据。
impl ShellView {
    /// 当前应当展示的条目：有筛选条件时是筛选结果，否则是全部已加载条目。
    pub(crate) fn visible(&self) -> &[Entry] {
        self.filtered.as_deref().unwrap_or(&self.entries)
    }

    /// 展示用的文件名（尊重「显示文件扩展名」开关）。
    pub(crate) fn display_name(&self, entry: &Entry) -> String {
        let name = entry.name.as_ref();
        if self.config.show_ext || entry.is_dir() {
            return name.to_string();
        }
        // 只有「点号不在开头」的才当作扩展名：`.gitignore` 这类点开头文件整体就是文件名。
        match name.rfind('.') {
            Some(index) if index > 0 => name[..index].to_string(),
            _ => name.to_string(),
        }
    }

    /// 详情视图「类型」列的文本。
    pub(crate) fn kind_label(&self, entry: &Entry) -> String {
        match entry.kind {
            EntryKind::Directory => String::from("文件夹"),
            EntryKind::Drive => String::from("驱动器"),
            EntryKind::Symlink => String::from("快捷方式"),
            EntryKind::File { ext: None } => String::from("文件"),
            EntryKind::File { ext: Some(id) } => match self.service.interner().resolve(id) {
                Some(extension) => format!("{} 文件", extension.to_uppercase()),
                None => String::from("文件"),
            },
        }
    }

    /// 扩展名文本（小写、不含点），供图标挑选使用。
    pub(crate) fn extension_label(&self, entry: &Entry) -> Option<String> {
        let id = entry.ext()?;
        self.service.interner().resolve(id).map(|ext| ext.to_string())
    }

    /// 修改时间的**本地时区**文本。
    ///
    /// 平台层拿不到本地时区时退化为 UTC，而不是留空——整列空白会让人以为功能没做。
    pub(crate) fn modified_label(entry: &Entry) -> Option<String> {
        let time = entry.modified?;
        Some(
            fm_platform::format_time_local(time)
                .unwrap_or_else(|| fm_core::util::format_system_time_utc(time)),
        )
    }

    /// 当前选中集对应的条目（按列表顺序）。
    pub(crate) fn selected_entries(&self) -> Vec<&Entry> {
        let visible = self.visible();
        self.selection
            .iter()
            .filter_map(|index| visible.get(index))
            .collect()
    }

    /// 选中项的合计字节数（只累加文件）。
    ///
    /// 目录的 `metadata().len()` 与内容无关，把它算进合计会得到明显错误的数字。
    pub(crate) fn selection_size(&self) -> u64 {
        self.selected_entries()
            .iter()
            .filter(|entry| !entry.is_dir())
            .fold(0u64, |total, entry| total.saturating_add(entry.size))
    }

    /// 当前是否全选（用于表头勾选框的状态）。
    pub(crate) fn all_selected(&self) -> bool {
        let visible = self.visible();
        !visible.is_empty() && self.selection.len() == visible.len()
    }

    /// 预览面板要展示的内容。
    pub(crate) fn preview_content(&self) -> PreviewContent {
        let selected = self.selected_entries();
        match selected.len() {
            0 => PreviewContent::Empty,
            1 => {
                let entry = selected[0];
                PreviewContent::Single(Box::new(EntrySummary {
                    name: self.display_name(entry).into(),
                    kind_label: self.kind_label(entry).into(),
                    size: (!entry.is_dir()).then(|| fm_core::format_size(entry.size).into()),
                    modified: Self::modified_label(entry).map(Into::into),
                    location: self.location_label().into(),
                    path: entry.path.as_ref().clone(),
                    is_dir: entry.is_dir(),
                    icon: widgets::entry_icon(entry, self.extension_label(entry).as_deref()),
                }))
            }
            _ => PreviewContent::Multiple {
                names: selected
                    .iter()
                    .map(|entry| self.display_name(entry).into())
                    .collect(),
                total_size: fm_core::format_size(self.selection_size()).into(),
            },
        }
    }

    /// 当前位置的展示文案（状态栏与预览面板共用）。
    pub(crate) fn location_label(&self) -> String {
        match self.tabs.active_state().location() {
            Location::Computer => String::from("此电脑"),
            Location::RecycleBin => String::from("回收站"),
            // 设置页没有路径，用标签标题保持一致，而不是显示一个空串或假路径。
            Location::Settings => String::from("设置"),
            Location::Directory(path) => path.display().to_string(),
        }
    }

    /// 当前标签是否停在设置页。
    pub(crate) fn in_settings(&self) -> bool {
        matches!(self.tabs.active_state().location(), Location::Settings)
    }

    /// 当前标签是否停在「回收站」。
    ///
    /// 回收站里的条目**不能**按路径导航：它们的 `path` 指向 `$Recycle.Bin` 内部
    /// （受保护的系统目录），按普通条目去 `scan` 只会得到一条「拒绝访问」。
    /// 双击、右键「打开」、预览面板的「打开」三处都过这个判断。
    pub(crate) fn in_recycle_bin(&self) -> bool {
        matches!(self.tabs.active_state().location(), Location::RecycleBin)
    }
}

/// 用户动作。
///
/// 集中放在一处，便于对照设计稿的交互清单逐项核对是否有遗漏；
/// 同时也让「哪些操作会触发重新扫描」这件事只有一个答案。
impl ShellView {
    /// 在当前标签所在位置打开一个新标签并切过去。
    pub(crate) fn open_tab(&mut self, cx: &mut Context<Self>) {
        // 新标签落在与当前标签相同的位置（可能是目录，也可能是「此电脑」）。
        let location = self.tabs.active_state().location().clone();
        self.tabs.open_at(location);
        self.reload(cx);
    }

    /// 在指定目录打开一个新标签并切过去。
    ///
    /// 与 [`ShellView::open_tab`] 的差别只在落点：后者沿用当前标签的位置。
    /// 右键菜单的「在新标签页中打开」需要一个明确的落点——目录落在它自己身上，
    /// 文件落在当前目录（文件没有「自己的目录」可言）。
    pub(crate) fn open_tab_at(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.tabs.open_at(Location::Directory(path));
        self.reload(cx);
    }

    /// 关闭指定标签，并重新扫描接管焦点的那个标签。
    pub(crate) fn close_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        self.tabs.close(id);
        self.reload(cx);
    }

    /// 恢复最近关闭的标签并切过去（Ctrl+Shift+T）。栈空时不做事。
    pub(crate) fn reopen_closed_tab(&mut self, cx: &mut Context<Self>) {
        if self.tabs.reopen_closed().is_some() {
            self.reload(cx);
        }
    }

    /// 关闭除 `id`（缺省为当前标签）之外的所有标签。
    pub(crate) fn close_others(&mut self, id: Option<TabId>, cx: &mut Context<Self>) {
        let id = id.unwrap_or_else(|| self.tabs.active().id());
        self.tabs.close_others(id);
        self.reload(cx);
    }

    /// 关闭 `id`（缺省为当前标签）右侧的所有标签。
    pub(crate) fn close_right(&mut self, id: Option<TabId>, cx: &mut Context<Self>) {
        let id = id.unwrap_or_else(|| self.tabs.active().id());
        self.tabs.close_right(id);
        self.reload(cx);
    }

    /// 切换标签。已经处于激活状态时不做任何事，避免无谓的重扫。
    pub(crate) fn select_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        if self.tabs.active().id() == id {
            return;
        }
        if self.tabs.select(id) {
            self.reload(cx);
        }
    }

    /// 在当前标签内跳转到指定目录。
    pub(crate) fn navigate_to(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.tabs.active_state_mut().navigate_to(path);
        self.reload(cx);
    }

    /// 是否有文本输入框正在接收键盘（地址栏 / 搜索框 / 内联重命名）。
    ///
    /// 全局快捷键此时必须**让位**：否则「在搜索框里打字时按 Ctrl+C」会一边
    /// 复制文本、一边又复制文件，F2 / Delete / 方向键更是会直接误伤。
    /// 与设计稿 keydown 里的 `if(inField)return` 同义。
    fn editing_input(&self) -> bool {
        self.address_editing || self.search_editing || self.renaming.is_some()
    }

    /// 键盘方向键移动焦点项并同步选中（设计稿的 ArrowUp / ArrowDown）。
    ///
    /// `extend` 对应 Shift 区间扩选。无焦点项时从列表两端进入：向下从首项、
    /// 向上从末项（设计稿 `i<0?0:items.length-1`）。移动后把该项滚进视野。
    pub(crate) fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let len = self.visible().len();
        if len == 0 {
            return;
        }
        let next = match self.cursor {
            None => {
                if delta > 0 {
                    0
                } else {
                    len - 1
                }
            }
            Some(index) => {
                let target = index as isize + delta;
                target.clamp(0, len as isize - 1) as usize
            }
        };
        if extend {
            self.selection.select_range(next, len);
        } else {
            self.selection.select_single(next);
        }
        self.cursor = Some(next);
        self.scroll_into_view(next);
        cx.notify();
    }

    /// 打开焦点项（Enter）：目录进入，文件与回收站条目不做任何事（与双击同规则）。
    pub(crate) fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.cursor.or_else(|| self.selection.single()) else {
            return;
        };
        let Some(entry) = self.visible().get(index).cloned() else {
            return;
        };
        if !entry.is_dir() || self.in_recycle_bin() {
            return;
        }
        let path = entry.path.as_ref().clone();
        self.navigate_to(path, cx);
    }

    /// 在当前标签内进入「此电脑」（驱动器总览）。
    pub(crate) fn go_to_computer(&mut self, cx: &mut Context<Self>) {
        self.tabs.active_state_mut().navigate_to(Location::Computer);
        self.reload(cx);
    }

    /// 在当前标签内进入「回收站」。
    pub(crate) fn go_to_recycle_bin(&mut self, cx: &mut Context<Self>) {
        self.tabs
            .active_state_mut()
            .navigate_to(Location::RecycleBin);
        self.reload(cx);
    }

    /// 打开设置页（标签内页面）。
    ///
    /// 已经有一个设置标签时**切过去**而不是再开一个：`Ctrl+,` 是「我要看看设置」，
    /// 不是「再开一个设置」。用户手动开多个也不是错误——他只会在关的时候发现。
    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        let existing = self
            .tabs
            .tabs()
            .iter()
            .find(|tab| *tab.state().location() == Location::Settings)
            .map(|tab| tab.id());
        match existing {
            Some(id) => self.select_tab(id, cx),
            None => {
                self.tabs.open_at(Location::Settings);
                self.reload(cx);
            }
        }
    }

    /// 打开帮助。与设置页共用「关于与快捷键」分节。
    pub(crate) fn open_help(&mut self, cx: &mut Context<Self>) {
        self.open_settings(cx);
    }

    /// 后退。返回是否真的发生了导航。
    pub(crate) fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tabs.active_state_mut().back().is_none() {
            return false;
        }
        self.reload(cx);
        true
    }

    /// 前进。
    pub(crate) fn go_forward(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tabs.active_state_mut().forward().is_none() {
            return false;
        }
        self.reload(cx);
        true
    }

    /// 进入上级目录。
    pub(crate) fn go_up(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tabs.active_state_mut().parent().is_none() {
            return false;
        }
        self.reload(cx);
        true
    }

    /// 切换内容区视图模式并持久化。
    pub(crate) fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        if self.tabs.active_state().view_mode() == mode {
            return;
        }
        self.tabs.active_state_mut().set_view_mode(mode);
        self.config.view_mode = mode;
        self.config.save();
        // 视图模式是会话快照的一部分，改完要立刻落盘。
        self.persist_session();
        cx.notify();
    }

    /// 折叠 / 展开左侧导航栏。
    ///
    /// 宽屏切换的是持久化的「显示导航区」；窄屏（≤920px）切换的是浮层开关——
    /// 浮层是临时状态，不应该覆盖用户在大屏上的偏好，因此分开处理。
    pub(crate) fn toggle_sidebar(&mut self, viewport: Viewport, cx: &mut Context<Self>) {
        if viewport.sidebar_overlay {
            self.sidebar_overlay = !self.sidebar_overlay;
        } else {
            self.config.sidebar_collapsed = !self.config.sidebar_collapsed;
            self.config.save();
        }
        cx.notify();
    }

    /// 关闭窄屏下的侧边栏浮层（点击遮罩）。
    pub(crate) fn close_sidebar_overlay(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_overlay {
            self.sidebar_overlay = false;
            cx.notify();
        }
    }

    /// 折叠 / 展开侧边栏里的驱动器子项。
    ///
    /// 这个状态刻意不持久化：它描述的是「此刻想不想看到驱动器」，
    /// 每次启动都回到展开态更符合直觉，也避免用户误折叠后再也找不到入口。
    pub(crate) fn toggle_drives_group(&mut self, cx: &mut Context<Self>) {
        self.drives_expanded = !self.drives_expanded;
        cx.notify();
    }

    /// 显示 / 隐藏右侧预览面板并持久化。
    pub(crate) fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        self.preview_open = !self.preview_open;
        self.config.show_preview = self.preview_open;
        self.config.save();
        cx.notify();
    }

    /// 打开右侧预览面板；已经打开时保持原样。
    ///
    /// 与 [`ShellView::toggle_preview`] 的区别是**语义**而非行为：右键菜单里的
    /// 「属性」表达的是「我要看它的信息」，点它是想看，不该把已经开着的面板关掉。
    pub(crate) fn show_preview(&mut self, cx: &mut Context<Self>) {
        if !self.preview_open {
            self.toggle_preview(cx);
        }
    }

    /// 切换深色/浅色主题并持久化。
    ///
    /// 这里必须传入 `window`：主题里的窗口相关外观（滚动条、标题栏）依赖它才能同步刷新。
    pub(crate) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_dark(!self.config.dark, window, cx);
    }

    /// 指定深浅色主题（设置页的单选比「切换」语义更明确）。
    ///
    /// 这里必须传入 `window`：主题里的窗口相关外观（滚动条、标题栏）依赖它才能同步刷新。
    pub(crate) fn set_dark(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.config.dark == dark {
            return;
        }
        self.config.dark = dark;
        Theme::change(theme_mode(self.config.dark), Some(window), cx);
        // 组件库主题重置后要把设计令牌再同步一次，否则输入框会退回组件库默认配色。
        apply_component_theme(&self.config, cx);
        self.config.save();
        self.refresh_tokens(cx);
        cx.notify();
    }

    /// 设置缩略图档位并持久化。
    pub(crate) fn set_zoom(&mut self, zoom: IconZoom, cx: &mut Context<Self>) {
        if self.config.zoom == zoom {
            return;
        }
        self.config.zoom = zoom;
        self.config.save();
        cx.notify();
    }

    /// 设置界面密度并持久化（入口在设置页）。
    ///
    /// 行高令牌由 `refresh_tokens` 每帧重建，因此这里只需要改配置并重绘。
    pub(crate) fn set_density(&mut self, density: Density, cx: &mut Context<Self>) {
        if self.config.density == density {
            return;
        }
        self.config.density = density;
        self.config.save();
        cx.notify();
    }

    /// 切换「显示文件扩展名」并持久化（入口在设置页）。
    pub(crate) fn toggle_show_ext(&mut self, cx: &mut Context<Self>) {
        self.config.show_ext = !self.config.show_ext;
        self.config.save();
        cx.notify();
    }

    /// 把当前选中项滚回可视区（预览面板的「在列表中定位」）。
    ///
    /// 用 `selection.single()` 而不是 `cursor`：用户可能是用鼠标点选的那一项，
    /// 此时焦点项未必同步（键盘导航才维护 `cursor`）。两者都取不到时什么也不做。
    pub(crate) fn reveal_selected(&mut self, cx: &mut Context<Self>) {
        let Some(index) = self.selection.single().or(self.cursor) else {
            return;
        };
        self.cursor = Some(index);
        self.scroll_into_view(index);
        cx.notify();
    }

    /// 开始拖动某一列的宽度。
    ///
    /// 起始宽度取**当前实际渲染的宽度**（可能是断点默认值，也可能是上次拖动的结果），
    /// 而不是「上一次的偏好值」：否则在一列从未拖过的情况下，
    /// 第一次拖动会从 0 或某个默认常量起跳，列宽会瞬间跳变。
    pub(crate) fn begin_column_resize(
        &mut self,
        column: Column,
        x: f32,
        window: &Window,
        _cx: &mut Context<Self>,
    ) {
        let viewport = Viewport::for_width(f32::from(window.viewport_size().width));
        let columns = fm_views::details_table::Columns::for_viewport(
            viewport.width,
            &self.tokens,
            self.config.detail_columns,
            self.content_width(viewport),
        );
        let start_width = match column {
            Column::Date => columns.date,
            Column::Kind => columns.kind,
            Column::Size => Some(columns.size),
        };
        // 该列当前不可见（窄屏被断点隐藏）时不起会话：没有可拖的对象。
        let Some(start_width) = start_width else {
            return;
        };
        self.column_drag = Some(ColumnDrag {
            column,
            start_x: x,
            start_width,
        });
    }

    /// 拖动中：把宽度写回配置（`ColumnWidths::set` 负责区间钳制）。
    pub(crate) fn update_column_resize(&mut self, x: f32, cx: &mut Context<Self>) {
        let Some(drag) = self.column_drag.as_ref() else {
            return;
        };
        let width = drag.width_at(x);
        let column = drag.column;
        self.config.detail_columns.set(column, width);
        cx.notify();
    }

    /// 结束拖动并落盘。
    ///
    /// 只有在真的发生过拖动时才写配置：一次单击表头（按下即抬起）不该写盘。
    pub(crate) fn end_column_resize(&mut self, cx: &mut Context<Self>) {
        if self.column_drag.take().is_some() {
            self.config.save();
            cx.notify();
        }
    }
}

/// 排序、选中与筛选。
impl ShellView {
    /// 点击表头：切换排序列后重新扫描。
    ///
    /// 之所以重新扫描而不是就地对已加载条目重排，是为了让「排序键」这件事只有一个来源
    /// （`TabState`），避免内存里的顺序与实际排序设置出现分歧。
    pub(crate) fn set_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.tabs.active_state_mut().toggle_sort(key);
        self.reload(cx);
    }

    /// 命令条上的「排序」按钮：在四个排序键之间循环。
    ///
    /// 设计稿里这个按钮弹出一个菜单；本阶段用一个「循环 + tooltip 提示当前键」的
    /// 轻量做法替代——它不引入弹出层，也不会让按钮变成一个点了没反应的装饰。
    pub(crate) fn cycle_sort(&mut self, cx: &mut Context<Self>) {
        let next = match self.tabs.active_state().sort_key() {
            SortKey::Name => SortKey::Modified,
            SortKey::Modified => SortKey::Kind,
            SortKey::Kind => SortKey::Size,
            SortKey::Size => SortKey::Name,
        };
        self.set_sort(next, cx);
    }

    /// 当前的排序键，供按钮提示使用。
    pub(crate) fn sort_key(&self) -> SortKey {
        self.tabs.active_state().sort_key()
    }

    /// 单击选中：按语义更新选中集，并同步焦点项。
    pub(crate) fn select(&mut self, index: usize, mode: fm_views::SelectMode, cx: &mut Context<Self>) {
        let len = self.visible().len();
        if index >= len {
            return;
        }
        match mode {
            fm_views::SelectMode::Replace => self.selection.select_single(index),
            fm_views::SelectMode::Toggle => self.selection.toggle(index),
            fm_views::SelectMode::Range => self.selection.select_range(index, len),
        }
        self.cursor = Some(index);
        cx.notify();
    }

    /// 框选开始：只记锚点与起始坐标，不改选中（选中交给 `on_click` 或 `update_rubber_band`）。
    pub(crate) fn begin_rubber_band(&mut self, index: usize, position: Point<Pixels>) {
        self.rubber_band = Some(RubberBand {
            anchor: index,
            start: position,
            active: false,
        });
    }

    /// 框选进行中：超过拖动阈值后，把选中集替换为「锚点..当前」的区间。
    pub(crate) fn update_rubber_band(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(rubber) = self.rubber_band.as_mut() else {
            return;
        };
        if !rubber.active {
            let dx = (position.x - rubber.start.x).abs();
            let dy = (position.y - rubber.start.y).abs();
            // 还没拖够阈值：当作单击，不动选中。
            if dx < px(RUBBER_BAND_THRESHOLD) && dy < px(RUBBER_BAND_THRESHOLD) {
                return;
            }
            rubber.active = true;
        }
        self.selection.select_span(rubber.anchor, index);
        self.cursor = Some(index);
        cx.notify();
    }

    /// 框选结束：清空会话。左键抬起时调用（含在容器外抬起）。
    pub(crate) fn end_rubber_band(&mut self) {
        self.rubber_band = None;
    }

    /// 标签拖拽开始：只记被拖的标签与起始坐标。
    pub(crate) fn begin_tab_drag(&mut self, id: TabId, position: Point<Pixels>) {
        self.tab_drag = Some(TabDrag {
            tab: id,
            start: position,
            active: false,
        });
    }

    /// 拖拽经过另一个标签：超过阈值后，把被拖的标签移到它的位置。
    pub(crate) fn drag_tab_over(
        &mut self,
        hovered: TabId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.tab_drag.as_mut() else {
            return;
        };
        if drag.tab == hovered {
            return;
        }
        if !drag.active {
            let dx = (position.x - drag.start.x).abs();
            let dy = (position.y - drag.start.y).abs();
            if dx < px(TAB_DRAG_THRESHOLD) && dy < px(TAB_DRAG_THRESHOLD) {
                return;
            }
            drag.active = true;
        }
        let from = drag.tab;
        // `drag` 的借用到此为止（NLL 会在最后一次使用后结束它），
        // 下面才能借用 `self.tabs` 去重排。
        if let Some(to) = self.tabs.index_of(hovered) {
            self.tabs.move_tab(from, to);
        }
        cx.notify();
    }

    /// 标签拖拽结束。
    pub(crate) fn end_tab_drag(&mut self) {
        // 顺序是会话的一部分：拖完就落盘，否则「重排标签后立刻关窗」会丢结果。
        if self.tab_drag.take().is_some_and(|drag| drag.active) {
            self.persist_session();
        }
    }

    /// 表头勾选框：全选 / 全不选。
    pub(crate) fn toggle_select_all(&mut self, selected: bool, cx: &mut Context<Self>) {
        let len = self.visible().len();
        if selected {
            self.selection.select_all(len);
            self.cursor = (len > 0).then_some(0);
        } else {
            self.selection.clear();
            self.cursor = None;
        }
        cx.notify();
    }

    /// 清空选中（预览面板与选中条上的「取消选择」）。
    pub(crate) fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() && self.cursor.is_none() {
            return;
        }
        self.clear_selection_state();
        cx.notify();
    }

    /// 只重置状态、不触发重绘，供导航路径复用。
    fn clear_selection_state(&mut self) {
        self.selection.clear();
        self.cursor = None;
    }

    /// 更新搜索关键词并按需重算筛选结果。
    pub(crate) fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        let changed = self.query != query;
        self.query = query;
        if changed {
            // 筛选会重排整个列表，基于下标的选中集立刻失去意义。
            self.clear_selection_state();
            self.refresh_filter();
            cx.notify();
        }
    }

    /// 重算筛选结果。无关键词时把 `filtered` 置回 `None`，省掉一份整表克隆。
    fn refresh_filter(&mut self) {
        let needle = self.query.trim().to_lowercase();
        if needle.is_empty() {
            self.filtered = None;
            return;
        }
        self.filtered = Some(
            self.entries
                .iter()
                .filter(|entry| entry.name.to_lowercase().contains(&needle))
                .cloned()
                .collect(),
        );
    }

    /// 清空搜索框并退出编辑态。
    ///
    /// 不需要去重置输入框的值：每次进入编辑态都会新建一个 `InputState`，
    /// 并以当时的关键词作为初值，因此不存在「残留上一次输入」的可能。
    pub(crate) fn clear_query(&mut self, cx: &mut Context<Self>) {
        let had = !self.query.is_empty();
        self.query.clear();
        self.filtered = None;
        self.search_editing = false;
        if had {
            self.clear_selection_state();
            cx.notify();
        }
    }
}

/// 地址栏与搜索框。
impl ShellView {
    /// 进入地址栏编辑态。
    ///
    /// 每次都新建一个 `InputState` 并以当前路径作为初值，
    /// 这样无需依赖 `set_value` 的取值时机，也不会残留上一次未提交的输入。
    pub(crate) fn begin_address_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 虚拟位置没有路径可编辑，输入框留空，用户直接输入目标路径即可。
        let initial = self
            .tabs
            .active_state()
            .directory()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        let state = cx.new(|cx| InputState::new(window, cx).default_value(initial));

        // 回车提交、失焦取消：设计稿的地址栏没有「跳转」按钮，这两个入口必须自己接。
        self.address_subscription = Some(cx.subscribe_in(
            &state,
            window,
            move |this, _state, event: &InputEvent, _window, cx| match event {
                InputEvent::PressEnter { .. } => this.commit_address_edit(cx),
                InputEvent::Blur => {
                    if this.address_editing {
                        this.address_editing = false;
                        cx.notify();
                    }
                }
                InputEvent::Change | InputEvent::Focus => {}
            },
        ));

        let handle = state.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.address_input = Some(state);
        self.address_editing = true;
        cx.notify();
    }

    /// 提交地址栏输入：路径存在则跳转，否则在内容区给出提示。
    pub(crate) fn commit_address_edit(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.address_input.clone() else {
            self.address_editing = false;
            cx.notify();
            return;
        };

        let text = state.read(cx).value().trim().to_string();
        self.address_editing = false;

        if text.is_empty() {
            // 空输入只当作「取消」，不弹错误——用户清空输入框往往就是想退出。
            cx.notify();
            return;
        }

        let path = PathBuf::from(&text);
        if path.is_dir() {
            self.navigate_to(path, cx);
        } else {
            // 回显时截断，避免超长输入把内容区提示撑破。
            self.error = Some(format!("路径不存在：{}", truncate(&text, 80)).into());
            cx.notify();
        }
    }

    /// 进入搜索编辑态。
    ///
    /// 与地址栏同一套做法：点一下才创建真正的输入框，未进入编辑态时只用静态文本
    /// 呈现占位符或当前关键词，避免每帧都维护输入状态。
    pub(crate) fn begin_search_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let initial = self.query.clone();
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("搜索当前文件夹")
                .default_value(initial)
                .clean_on_escape()
        });

        // 输入即筛选（设计稿的 `oninput`），失焦退出编辑态。
        self.search_subscription = Some(cx.subscribe_in(
            &state,
            window,
            move |this, state, event: &InputEvent, _window, cx| match event {
                InputEvent::Change => {
                    let value = state.read(cx).value().to_string();
                    this.set_query(value, cx);
                }
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    if this.search_editing {
                        this.search_editing = false;
                        cx.notify();
                    }
                }
                InputEvent::Focus => {}
            },
        ));

        let handle = state.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        self.search_input = Some(state);
        self.search_editing = true;
        cx.notify();
    }

    /// 把当前路径写入系统剪贴板。
    pub(crate) fn copy_current_path(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.tabs.active_state().directory().map(PathBuf::from) else {
            // 虚拟位置没有可复制的路径：给出提示而不是静默失败。
            window.push_notification(
                gpui_component::notification::Notification::info("「此电脑」没有可复制的路径"),
                cx,
            );
            return;
        };
        let text = path.display().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        window.push_notification(
            gpui_component::notification::Notification::success(format!("已复制路径 {text}")),
            cx,
        );
    }
}

/// 按**字符**截断，而不是按字节。
///
/// 按字节切会把多字节字符切成两半，直接 panic；路径里出现中文是常态。
pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut result: String = text.chars().take(max_chars).collect();
    result.push('…');
    result
}

impl ShellView {
    /// 给根元素挂上全部全局快捷键。
    ///
    /// 挂在根元素而不是窗口级：GPUI 无焦点时会把动作派发到根节点
    /// （`focus_node_id_in_rendered_frame` 用 `root_node_id` 兜底），
    /// 因此无论焦点在哪都能命中；输入框自己的按键（复制文本、回车提交）在更深
    /// 的节点就被消费，不会到这里。`editing_input` 的守卫再兜住剩下漏进来的按键。
    fn apply_shortcuts(&self, root: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        let view = cx.entity().downgrade();
        let mut root = root;

        root = bind_shortcut::<NewTab>(root, &view, |this, _w, cx| this.open_tab(cx));
        root = bind_shortcut::<CloseTab>(root, &view, |this, _w, cx| {
            let id = this.tabs.active().id();
            this.close_tab(id, cx);
        });
        root = bind_shortcut::<ReopenClosedTab>(root, &view, |this, _w, cx| {
            this.reopen_closed_tab(cx);
        });
        root = bind_shortcut::<GoBack>(root, &view, |this, _w, cx| {
            this.go_back(cx);
        });
        root = bind_shortcut::<GoForward>(root, &view, |this, _w, cx| {
            this.go_forward(cx);
        });
        root = bind_shortcut::<GoUp>(root, &view, |this, _w, cx| {
            this.go_up(cx);
        });
        root = bind_shortcut::<Refresh>(root, &view, |this, _w, cx| this.reload(cx));
        root = bind_shortcut::<FocusAddressBar>(root, &view, |this, w, cx| {
            this.begin_address_edit(w, cx);
        });
        root = bind_shortcut::<SetViewDetails>(root, &view, |this, _w, cx| {
            this.set_view_mode(ViewMode::Details, cx);
        });
        root = bind_shortcut::<SetViewIcons>(root, &view, |this, _w, cx| {
            this.set_view_mode(ViewMode::Icons, cx);
        });
        root = bind_shortcut::<ToggleTheme>(root, &view, |this, w, cx| this.toggle_theme(w, cx));
        root = bind_shortcut::<OpenSettings>(root, &view, |this, _w, cx| {
            this.open_settings(cx);
        });
        // 帮助与设置共用一页（「关于与快捷键」分节），因此两个动作落到同一个方法。
        root = bind_shortcut::<OpenHelp>(root, &view, |this, _w, cx| this.open_help(cx));
        root = bind_shortcut::<SelectAll>(root, &view, |this, _w, cx| {
            this.toggle_select_all(true, cx);
        });
        root = bind_shortcut::<CopyItems>(root, &view, |this, w, cx| {
            this.clip_selection(ClipboardMode::Copy, w, cx);
        });
        root = bind_shortcut::<CutItems>(root, &view, |this, w, cx| {
            this.clip_selection(ClipboardMode::Cut, w, cx);
        });
        root = bind_shortcut::<PasteItems>(root, &view, |this, w, cx| this.paste(w, cx));
        root = bind_shortcut::<RenameItem>(root, &view, |this, w, cx| {
            this.rename_selected(w, cx);
        });
        root = bind_shortcut::<DeleteItems>(root, &view, |this, w, cx| {
            this.delete_selection(w, cx);
        });
        root = bind_shortcut::<OpenItem>(root, &view, |this, _w, cx| this.open_selected(cx));
        root = bind_shortcut::<MoveCursorUp>(root, &view, |this, _w, cx| {
            this.move_cursor(-1, false, cx);
        });
        root = bind_shortcut::<MoveCursorDown>(root, &view, |this, _w, cx| {
            this.move_cursor(1, false, cx);
        });
        root = bind_shortcut::<ExtendCursorUp>(root, &view, |this, _w, cx| {
            this.move_cursor(-1, true, cx);
        });
        root = bind_shortcut::<ExtendCursorDown>(root, &view, |this, _w, cx| {
            this.move_cursor(1, true, cx);
        });

        root
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 令牌每帧刷新一次：主题、密度、缩放三者的变化都会在这里被吸收，
        // 子渲染函数因此只需要读 `self.tokens`，不必各自判断主题。
        self.refresh_tokens(cx);

        let viewport = Viewport::for_width(f32::from(window.viewport_size().width));
        // 缓存网格列数：`scroll_into_view` 在没有 window 的地方（扫描收尾的
        // 异步回调里）也要把条目下标换算成网格行下标。
        self.grid_columns = self.grid_columns_for(viewport);

        // 先把各区块各自渲染成 `AnyElement` 再拼装，
        // 避免在同一个表达式里交替借用 `self` 与 `cx`。
        //
        // 顶部曾经有两栏（品牌栏 + 标签条），品牌栏按反馈整条删掉了：
        // 它剩下的内容只有品牌标志，为一行图标占 38px 不划算。
        // 窗口拖拽区因此移到了标签条那一栏的空白段上（见 `tab_strip`）。
        let tab_strip = self.render_tab_strip(cx);
        let toolbar = self.render_toolbar(viewport, cx);
        let sidebar = self.render_sidebar(viewport, cx);
        let content = self.render_content(viewport, window, cx);
        let preview = self.render_preview(viewport, cx);
        let status_bar = self.render_status_bar(viewport, cx);
        // 浮层模式下才需要遮罩；宽屏侧边栏是常驻的一栏，点它不该有任何副作用。
        let scrim = (viewport.sidebar_overlay && self.sidebar_overlay)
            .then(|| self.render_scrim(cx));

        let mut body = div()
            // `relative` 是浮层（预览面板 / 侧边栏 / 遮罩）的定位基准：
            // 少了它，绝对定位会一路上溯到窗口根，浮层就会盖住标签条。
            .relative()
            .flex()
            .flex_row()
            .flex_grow(1.0)
            // min_h 0 是必须的：否则内容区会被子元素撑高，把状态栏顶出窗口。
            .min_h(px(0.0))
            .overflow_hidden()
            .child(sidebar)
            .child(content);

        if let Some(preview) = preview {
            body = body.child(preview);
        }
        if let Some(scrim) = scrim {
            body = body.child(scrim);
        }

        let root = div()
            .size_full()
            .flex()
            .flex_col()
            // `relative` 是下面两个浮层的定位基准：它们都是 `absolute().inset_0()`，
            // 少了这一句会一路上溯到窗口根。
            .relative()
            .bg(self.tokens.surface)
            .text_color(self.tokens.fg)
            .font_family(self.tokens.font_family.clone())
            .child(tab_strip)
            .child(toolbar)
            .child(body)
            .child(status_bar)
            // 通知层与对话框层**必须由应用的视图自己挂上**。
            //
            // 组件库 0.6.4 的 `Root::render` 只渲染了「我们这一个视图 + 文字选择层 +
            // 几个 overlay」，**不含通知层与对话框层**；它自己的测试也是手动调用
            // `Root::render_dialog_layer(...)` 的（见 `dialog/dialog.rs` 的测试
            // `DialogHost`）。少了这一步的后果很有欺骗性：`push_notification` 与
            // `open_alert_dialog` 都会静默失效——提示和对话框都不出现，也不报错，
            // 调用点看起来完全正常。
            //
            // 放在最后 = 画在最上层；两层自身都是 `absolute().inset_0()`，
            // 不参与上面的 flex 布局，因此不影响任何区块的尺寸。
            .children(gpui_component::Root::render_notification_layer(window, cx))
            .children(gpui_component::Root::render_dialog_layer(window, cx));

        self.apply_shortcuts(root, cx)
    }
}

/// 一次内容加载的数据来源。
///
/// 真实目录与回收站的收尾（分批回传、代数校验、错误处理）完全一样，
/// 差别只在「从哪儿取条目」。因此共用一个 [`ShellView::begin_load`] + 这个枚举，
/// 而不是把整段 spawn 逻辑抄两遍。
enum LoadSource {
    /// 真实目录：走 `DirectoryService::scan`。
    Directory(PathBuf),
    /// 回收站：走 `DirectoryService::recycle_bin`。
    RecycleBin,
}

impl LoadSource {
    /// 加载失败时给用户看的前缀。
    fn error_label(&self) -> &'static str {
        match self {
            Self::Directory(_) => "无法打开该目录",
            Self::RecycleBin => "无法读取回收站",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{mix, truncate};
    use gpui::hsla;

    #[test]
    fn short_text_is_untouched() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn long_ascii_text_is_truncated_with_ellipsis() {
        assert_eq!(truncate("abcdefg", 5), "abcde…");
    }

    #[test]
    fn multibyte_text_is_not_split_mid_character() {
        // 4 个汉字的字节数是 12，按字节截断会 panic；按字符截断才是安全的。
        assert_eq!(truncate("中文路径测试", 4), "中文路径…");
    }

    #[test]
    fn mix_interpolates_each_channel() {
        // 注意 `Hsla` 的色相是 0..1 而不是 0..360。
        let base = hsla(0.0, 0.0, 0.0, 1.0);
        let other = hsla(1.0, 1.0, 1.0, 1.0);
        let mixed = mix(base, other, 0.5);
        assert!((mixed.h - 0.5).abs() < f32::EPSILON);
        assert!((mixed.s - 0.5).abs() < f32::EPSILON);
        assert!((mixed.l - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn mix_clamps_the_weight() {
        let base = hsla(0.0, 0.0, 0.0, 1.0);
        let other = hsla(0.0, 0.0, 1.0, 1.0);
        assert!((mix(base, other, 2.0).l - 1.0).abs() < f32::EPSILON);
        assert!((mix(base, other, -1.0).l - 0.0).abs() < f32::EPSILON);
    }
}
