//! 主窗口外壳视图：状态定义、动作与整体渲染入口。
//!
//! 布局与配色规范全部来自 `design/file-manager.html`：
//! 顶部「标题栏 + 标签条 + 命令条」三行，中部「侧边栏 + 内容区 + 预览面板」，
//! 底部状态栏。尺寸与色值不写死在这里，而是通过 [`fm_views::Tokens`] 取得，
//! 换主题或改密度时只有 `tokens.rs` 需要动。

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use fm_core::naming;
use fm_core::{
    BatchSpec, Column, Density, Entry, EntryKind, IconId, IconZoom, Location, Selection, SortKey,
    TabId, TabsState, UndoAction, UndoStack, ViewMode, batch_names,
};
use fm_platform::{CancelToken, Drive, OpReport, TransferProgress};
use fm_services::clipboard::{Clipboard, ClipboardMode, PasteError, PastePlan};
use fm_services::config::Config;
use fm_services::directory::{DEFAULT_BATCH, DirectoryService, ScanBatch, SortSpec};
use fm_services::ops::{self, NewItem, OpResult};
use fm_services::search::{self, SearchState};
use fm_services::session::SessionSnapshot;
use fm_services::thumbnail::ThumbnailService;
use fm_services::watch::DirectoryWatcher;
use fm_views::preview_panel::{EntrySummary, PreviewBody, PreviewContent};
use fm_views::{Tokens, widgets};
use gpui::{
    Anchor, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, ScrollHandle,
    ScrollStrategy, SharedString, Styled as _, Subscription, Task, UniformListScrollHandle, Window,
    div, px,
};
use gpui_component::button::ButtonVariant;
use gpui_component::dialog::DialogButtonProps;
use gpui_component::input::{InputEvent, InputState, TextareaState};
use gpui_component::notification::Notification;
use gpui_component::{ActiveTheme as _, Edges, Theme, ThemeMode, WindowExt as _};

use crate::layout::Viewport;

/// 应用自己的按键上下文。
///
/// 必须给内容根挂一个 `key_context`，并把所有全局快捷键绑到它上面——
/// 否则会出现一个很隐蔽的优先级问题：GPUI 的 `binding_enabled` 给**无上下文**
/// 的绑定返回 `contexts.len()`，而给命中上下文的绑定返回它在栈里的深度，
/// 两者相等时按「后注册的优先」裁决。结果就是：任何无上下文绑定都会**压过**
/// 输入框自己的按键（`backspace`、`enter`、`ctrl-a/c/v/z` 全部被抢）。
/// 挂上上下文之后，输入框（`Input`）比内容根更深，输入框的绑定重新胜出。
pub const KEY_CONTEXT: &str = "Pagefold";

use crate::actions::{
    CloseTab, CopyItems, CutItems, DeleteItems, DeletePermanently, ExtendCursorDown,
    ExtendCursorUp, FocusAddressBar, GoBack, GoForward, GoUp, MoveCursorDown, MoveCursorUp, NewTab,
    OpenHelp, OpenItem, OpenSettings, PasteItems, Refresh, RenameItem, ReopenClosedTab, SelectAll,
    SetViewDetails, SetViewIcons, ToggleTheme, Undo,
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
    /// 撤销栈。**不持久化**：它是本次会话内的操作记录，
    /// 跨重启复活一个「可撤销的粘贴」没有对应的磁盘状态。
    pub(crate) undo: UndoStack,
    /// 批量重命名浮层。`Some` 时浮层盖住内容区。
    pub(crate) batch: Option<BatchRename>,
    /// 进行中的复制 / 移动。`Some` 时右下角显示进度卡。
    ///
    /// 用 `Arc<Mutex<_>>` 是因为它被两个线程同时读写：UI 每 120ms 读一次画进度，
    /// 后台线程每复制完一个文件写一次。锁的持有时间都只有几个字段的拷贝。
    pub(crate) transfer: Option<Arc<Mutex<Transfer>>>,
    /// 内容根的焦点句柄。
    ///
    /// 没有编辑会话时，焦点必须落在这里：GPUI 的按键分发从**焦点节点**向上冒泡，
    /// 一个焦点都没有时起点会退化成窗口根节点，挂在内容根上的处理器根本不在
    /// 分发路径上——表现为「所有全局快捷键都没反应」。
    pub(crate) shell_focus: FocusHandle,
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
    /// 系统剪贴板里是否有文件（别的程序复制过来的）。
    ///
    /// 不能每帧去读：`OpenClipboard` 会与其它程序互斥，而且读一次要枚举路径。
    /// 由 `ambient_tick` 节流探测（约 1.2s 一次，且只在应用内剪贴板为空时），
    /// 只用于决定「粘贴」是否可用；真正的路径在按下粘贴时再读。
    pub(crate) system_clipboard: bool,
    /// 系统剪贴板探测的节流计数。
    clipboard_ticks: u32,
    /// 是否有外部文件正拖在内容区上方（只用于高亮提示）。
    pub(crate) drop_active: bool,
    /// 缩略图队列（工作线程）。
    thumbnails: ThumbnailService,
    /// 路径 → 缩略图句柄。取到之后条目自己的 `icon` 会被替换成它。
    ///
    /// 用路径而不是下标做键：排序、刷新、筛选都会让下标失效，
    /// 而路径在「同一份文件」的生命周期里是稳定的。
    thumb_ids: std::collections::HashMap<PathBuf, IconId>,
    /// 预览面板的文本内容：`(路径, 文本)`。
    ///
    /// 内层 `None` 表示「读过，但不是文本」——记下来才不会每次 tick 重读一遍。
    /// 文本用 `SharedString`：每帧渲染都要克隆一份给面板，`String` 会真的复制 32KB。
    preview_text: Option<(PathBuf, Option<SharedString>)>,
    /// 正在读取预览文本的任务。丢了这个句柄，读取会被取消。
    preview_task: Option<Task<()>>,
    /// 预览正文的只读输入框实体。
    ///
    /// 与 `preview_text` 一一对应，但单独持有：正文要能被框选复制，
    /// 而 GPUI 的普通文本没有选区，只能交给输入框。实体必须由外壳持有——
    /// 视图每帧重建，在渲染函数里 `cx.new` 会让每帧都得到一个全新的输入框。
    preview_text_input: Option<Entity<TextareaState>>,
    /// 是否在当前目录**及其子文件夹**中搜索。
    ///
    /// 默认关闭：递归搜索会遍历整棵树，用户只是想在当前目录里筛一下时
    /// 不该顺带扫几十万个文件。
    pub(crate) search_recursive: bool,
    /// 递归搜索的结果。`Some` 时 `visible()` 用它而不是当前目录的列表。
    search_results: Option<Vec<Entry>>,
    /// 后台搜索的共享状态：批次缓冲 + 是否结束。
    ///
    /// 与缩略图、系统剪贴板同一套模式：后台线程只写共享状态，
    /// 界面在 `ambient_tick` 里取走。跨线程直接调 `update` 会绕开这套约定。
    search_shared: Arc<Mutex<SearchState>>,
    /// 是否还在搜（用于状态提示）。
    pub(crate) search_running: bool,
    /// 取消标志。每次重新搜索都换一个新的，旧任务的标志被 cancel。
    search_cancel: CancelToken,
    /// 搜索任务句柄。丢了它搜索会被取消。
    search_task: Option<Task<()>>,
    /// 文件夹接管的当前状态（右键菜单项 / 默认打开程序）。
    ///
    /// 缓存在这里而不是每帧读注册表：设置页每帧都会渲染，
    /// 而状态只在「进入设置页」与「我们自己改过之后」才可能变。
    pub(crate) folder_integration: fm_platform::IntegrationState,
    /// 正在录制的新快捷键（设置页点「修改」后进入）。
    pub(crate) shortcut_capture: Option<crate::shortcut_handlers::Capture>,
    /// 录制浮层的焦点句柄：它必须能拿到按键，否则录不到任何东西。
    pub(crate) shortcut_focus: FocusHandle,
    /// 合并后的快捷键清单（默认值 + 用户覆盖）。
    ///
    /// **缓存在这里**而不是每帧 `resolve()` 一次：设置页每次滚动都会重建整棵树，
    /// 25 条命令合起来是「解析 25 个键位 + 25 个字符串 + 一张 BTreeMap」，
    /// 每帧做一遍纯属浪费。只在配置变化时刷新（见 `refresh_shortcut_cache`）。
    pub(crate) shortcut_items: Vec<fm_core::ResolvedShortcut>,
    /// 哪些分组是展开的（下标见 `fm_core::Group::ALL`）。
    ///
    /// 默认全部**折叠**：25 行同时铺开时页面很长，滚动手感也重；
    /// 折叠后只渲染组标题，用户点开自己关心的那一组即可。
    pub(crate) shortcut_groups: [bool; fm_core::Group::ALL.len()],
    /// 设置页的纵向滚动位置。
    ///
    /// 由视图层 `track_scroll` 接管：这样进设置页时能确保从顶部开始，
    /// 也让「滚动到底部」这类操作有据可依（之前用的是容器内部状态，外面看不到）。
    pub(crate) settings_scroll: ScrollHandle,
    /// 与 [`Self::shortcut_items`] 同时刷新的冲突列表。
    pub(crate) shortcut_conflicts: Vec<fm_core::shortcut::Conflict>,
    /// 标签条的横向滚动位置。
    ///
    /// 标签有最小宽度（96px），多到装不下时靠横向滚动而不是无限压缩——
    /// 继续压下去标题会只剩一两个字符，反而看不出是哪个目录。
    pub(crate) tab_scroll: ScrollHandle,
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
    /// 传输进度卡的轮询任务。
    ///
    /// **必须与 `op_task` 分开**：那个槽位会被每一次写操作（改名、新建、撤销…）
    /// 覆盖，一旦被覆盖，轮询任务就被 drop —— 后台线程照旧复制，但界面再也不会
    /// 收到 `finished`，进度卡会永远停在半路。
    _transfer_task: Option<Task<()>>,
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
    /// 当前目录的变更监听。切换位置时由 `reload_inner` 替换或停止。
    watcher: DirectoryWatcher,
    /// 「环境轮询」任务：把外部世界的两种信号搬进 UI。
    ///
    /// 搬的是两件事：目录变更（自动刷新）与重复启动转发来的位置（IPC）。
    /// 两者都是「低频、无回调可用」的事件，合成一个 400ms 的轮询比开两条通道简单。
    /// **必须持有**：`Task` 被 drop 即取消，表现为「再也不自动刷新」。
    ambient_task: Option<Task<()>>,
    /// 上一次写盘的会话快照。
    ///
    /// 有了它，`persist_session` 就能跳过重复写：自动刷新会让 `reload` 每秒
    /// 被调用若干次，而会话内容绝大多数时候没变——没有比较的话，
    /// 一次解压就会在几十秒内往 `%APPDATA%` 写上百次同样的文件。
    persisted_session: Option<SessionSnapshot>,
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

        // 键位表也来自配置：用户改过的组合在这里生效，没改过的用目录默认值。
        // 必须在窗口接收按键之前装好，因此放在构造函数里而不是首帧渲染。
        crate::shortcuts::apply(cx, &config);

        // 设置页要展示的清单与冲突，同样只算这一次（之后配置变化时再刷）。
        let shortcut_items = fm_core::resolve(&config.shortcuts);
        let shortcut_conflicts = fm_core::conflicts(&shortcut_items);

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
            watcher: DirectoryWatcher::new(),
            ambient_task: None,
            persisted_session: None,
            undo: UndoStack::new(),
            batch: None,
            transfer: None,
            shell_focus: cx.focus_handle(),
            query: String::new(),
            preview_open,
            sidebar_overlay: false,
            drives_expanded: true,
            address_editing: false,
            address_input: None,
            search_editing: false,
            search_input: None,
            clipboard: Clipboard::new(),
            system_clipboard: false,
            clipboard_ticks: 0,
            drop_active: false,
            thumbnails: ThumbnailService::new(),
            thumb_ids: std::collections::HashMap::new(),
            preview_text: None,
            preview_task: None,
            preview_text_input: None,
            search_recursive: false,
            search_results: None,
            search_shared: Arc::new(Mutex::new(SearchState::default())),
            search_running: false,
            search_cancel: CancelToken::new(),
            search_task: None,
            folder_integration: Self::exe_path()
                .map(|exe| fm_platform::folder_integration_state(&exe))
                .unwrap_or_default(),
            shortcut_capture: None,
            shortcut_focus: cx.focus_handle(),
            shortcut_items,
            shortcut_conflicts,
            shortcut_groups: [false; fm_core::Group::ALL.len()],
            settings_scroll: ScrollHandle::new(),
            tab_scroll: ScrollHandle::new(),
            renaming: None,
            area_menu_suppressed: false,
            list_incomplete: false,
            op_task: None,
            _transfer_task: None,
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
        this.start_ambient_loop(cx);
        this
    }

    /// 启动「环境轮询」循环（见 [`ShellView::ambient_tick`]）。
    ///
    /// 用 `cx.spawn`（App 级）而不是 `spawn_in`：这个循环只改状态、
    /// 不直接碰窗口，因此可以活到视图销毁为止。任务句柄必须留在
    /// `ambient_task` 里——`Task` 一被丢弃就会被取消。
    fn start_ambient_loop(&mut self, cx: &mut Context<Self>) {
        const TICK_MS: u64 = 400;
        self.ambient_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(TICK_MS))
                    .await;
                // 视图已销毁（窗口关闭）时 `update` 失败，循环随之结束。
                if this
                    .update(cx, |view, cx| view.ambient_tick(cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// 刷新设计令牌。渲染的第一件事，保证子渲染函数读到的是当前主题的色值。
    pub(crate) fn refresh_tokens(&mut self, cx: &App) {
        self.tokens = build_tokens(&self.config, cx);
    }
}

/// 由粘贴计划与回执推出「每个源实际落在了哪里」，**只取成功的那些**。
///
/// 目标路径优先看 `report.renamed`（重名改过名的那部分），其余按
/// 「目标目录 + 原文件名」推导。这与平台层 `resolve_target` 的规则一致，
/// 但撤销栈不该为此去调用一次文件系统：此刻文件已经存在，路径是确定的。
fn resolved_targets(
    sources: &[PathBuf],
    dest_dir: &Path,
    report: &OpReport,
) -> Vec<(PathBuf, PathBuf)> {
    let failed: std::collections::HashSet<&std::path::Path> =
        report.failed.iter().map(|(path, _)| path.as_path()).collect();

    sources
        .iter()
        .filter(|source| !failed.contains(source.as_path()))
        .map(|source| {
            let target = report
                .renamed
                .iter()
                .find(|(from, _)| from == source)
                .map(|(_, target)| target.clone())
                .unwrap_or_else(|| dest_dir.join(source.file_name().unwrap_or_default()));
            (source.clone(), target)
        })
        .collect()
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
/// 组件库菜单项 / 列表行在**悬停与选中**时该用的底色与文字色。
///
/// 单独抽出来是为了能单测：这对颜色曾经一直是库的默认值
/// （`theme.tokens` 是主题构造时的快照，改 `theme.accent` 不会同步过去），
/// 表现为「鼠标一移上去文字就消失」。
///
/// 两半各司其职：底色用 `hover`（半透明覆盖层，压在面板底上就是浅浅一层），
/// 文字必须用**正常前景色**——用 `surface` 会在浅色下变白、深色下变黑。
fn menu_hover_palette(tokens: &fm_views::Tokens) -> (gpui::Hsla, gpui::Hsla) {
    (tokens.hover, tokens.fg)
}

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
    // 这里是**菜单项 / 列表行悬停时的文字色**（背景读 `tokens.accent`，见函数末尾）。
    // 曾经写成 `tokens.surface`——浅色下近白、深色下近黑，正好与背景撞在一起，
    // 于是鼠标一移上去文字就消失。它必须与悬停底色成对，用正常前景色。
    theme.accent_foreground = tokens.fg;
    theme.danger = tokens.danger;
    theme.danger_foreground = tokens.surface;
    theme.popover = tokens.surface;
    theme.popover_foreground = tokens.fg;
    theme.list_hover = tokens.hover;
    theme.list_active = tokens.accent_soft;
    theme.list_active_border = tokens.accent_line;
    theme.list_head = tokens.panel;
    theme.radius = px(fm_views::metrics::RADIUS);
    // 通知统一落在**底部居中**。组件默认在右上角，而复制/移动的进度卡在底部——
    // 两个「操作反馈」分处两个对角，用户得来回看。现在它们共享底部这一条带。
    //
    // 放在这里而不是 bootstrap：主题切换会重建整个 theme（`Theme::change`），
    // 只在启动时设一次的话，切一次深浅色就被打回右上角了。
    theme.notification.placement = gpui::Anchor::BottomCenter;

    // 菜单项与列表行的**悬停配色**读的是 `theme.tokens` 里的这两个字段，
    // 而不是上面那些 `theme.accent`。`tokens` 是主题构造时按库自带配色算出的
    // **快照**，改 `theme.*` 不会同步过去——于是悬停一直是库的默认组合：
    // 浅色下「浅灰底 + 白字」、深色下「深灰底 + 黑字」，鼠标一移上去文字就消失。
    let (hover_background, hover_foreground) = menu_hover_palette(&tokens);
    // 背景读 `tokens.accent`，文字读 **`theme.accent_foreground`**（本函数上文已设）——
    // 二者不是同一个来源，这也是之前只改 tokens 却毫无效果的原因。
    theme.tokens.accent = hover_background.into();
    // 一并写上，给那些读 `tokens.accent_foreground` 的地方（输入框补全菜单）兜底。
    theme.tokens.accent_foreground = hover_foreground.into();

    // 这条日志专门用来排查「悬停文字看不见」：它把组件库真正会读的两个值打出来，
    // 而不是我们以为它读的值。
    tracing::debug!(
        hover_background = ?hover_background,
        accent_foreground = ?theme.accent_foreground,
        token_accent = ?theme.tokens.accent.background,
        "组件库悬停配色已写入"
    );
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

/// 一次复制 / 移动的进行态。
pub(crate) struct Transfer {
    /// 展示用的标题：「复制 3 项 → D:\目标」。
    pub(crate) label: String,
    /// 选中的顶层条目数（进度条的分母用文件数，这里只用于文案）。
    pub(crate) total: usize,
    /// 已完成的文件数。
    pub(crate) files_done: u64,
    /// 已复制的字节数（整个操作累计）。
    pub(crate) bytes_done: u64,
    /// 要复制的总字节数；`0` 表示未知（源里有目录）。
    pub(crate) bytes_total: u64,
    /// 正在处理的路径。
    pub(crate) current: PathBuf,
    /// 取消标志：UI 点「取消」后，后台在下一个文件边界停下。
    pub(crate) cancel: CancelToken,
    /// 是否已经请求取消（用于把按钮变成「正在取消…」）。
    pub(crate) cancelling: bool,
    /// 后台线程是否已收工。
    pub(crate) finished: bool,
}

/// 批量重命名的进行态。
pub(crate) struct BatchRename {
    /// 参与改名的路径（按列表顺序，序号按此顺序递增）。
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) prefix: Entity<InputState>,
    pub(crate) start: Entity<InputState>,
    pub(crate) keep_extension: bool,
    /// 订阅必须持有：丢掉即退订，表现为「输入时预览不动」。
    _prefix_subscription: Subscription,
    _start_subscription: Subscription,
}

/// 预览里的一行。
pub(crate) struct BatchRow {
    pub(crate) old: String,
    pub(crate) new: String,
    /// 与列表中**其他**条目撞名。
    pub(crate) conflict: bool,
}

/// 一次预览的结果。
pub(crate) struct BatchPreview {
    pub(crate) rows: Vec<BatchRow>,
    /// 无法生成名称的原因（前缀非法、序号不是数字……）。
    pub(crate) error: Option<String>,
    pub(crate) conflicts: usize,
}

impl BatchPreview {
    /// 能否执行：没有错误、没有撞名、至少一行。
    pub(crate) fn can_apply(&self) -> bool {
        self.error.is_none() && self.conflicts == 0 && !self.rows.is_empty()
    }
}

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
        // 刚建出来的这一条已经在列表里了，这次文件系统变更不用再触发一轮扫描；
        // 更要紧的是：那轮扫描会清空列表，正好打断马上要打开的重命名输入框。
        self.watcher.take_change();

        let sort = {
            let tab = self.tabs.active();
            SortSpec::new(tab.state().sort_key(), tab.state().sort_direction())
        };
        self.service
            .insert_sorted(&mut self.entries, entry, sort);

        // 新建是「这一秒才存在」的东西：撤销它就是永久删除（不必进回收站）。
        // 与随后的重命名分成两步记，因此 `Ctrl+Z` 会先撤名字、再撤创建——
        // 这正是用户按下撤销两次时期待的顺序。
        self.undo.push(UndoAction::Create { path: path.clone() });

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
        // 系统剪贴板要用一份独立的副本：`put` 会拿走所有权。
        let for_system = paths.clone();
        // 空选中会被 `put` 转成「清空剪贴板」，因此这里只判断要不要提示。
        self.clipboard.put(mode, paths, origin);

        if count > 0 {
            // 同步写进系统剪贴板，资源管理器那边才能粘贴。
            // 失败**不影响主功能**（应用内剪贴板照常工作），因此只降级提示。
            match self
                .service
                .file_system()
                .write_file_clipboard(&for_system, mode == ClipboardMode::Cut)
            {
                Ok(()) => {
                    // 应用内剪贴板优先，这个标志只影响「系统剪贴板里有没有东西」的判断。
                    self.system_clipboard = false;
                    window.push_notification(
                        Notification::success(format!(
                            "已{} {count} 项到剪贴板（可在其它程序中粘贴）",
                            mode.verb()
                        )),
                        cx,
                    );
                }
                Err(reason) => {
                    window.push_notification(
                        Notification::warning(format!(
                            "已{} {count} 项到应用内剪贴板；写入系统剪贴板失败（{reason}），只能在本程序内粘贴",
                            mode.verb()
                        )),
                        cx,
                    );
                }
            }
        }

        // 剪贴板的状态决定空白处菜单里「粘贴」是否禁用，必须重绘。
        cx.notify();
    }

    /// 拖出载荷的构造器。
    ///
    /// 规则与资源管理器一致：拖**已选中**的条目 → 带走整个选中集；
    /// 拖未选中的条目 → 只带走它自己。视图层只知道自己那一行，
    /// 「哪一批被选中」只有外壳知道，因此用回调把判断留在这里。
    pub(crate) fn drag_out_builder(&self) -> fm_views::DragOutBuilder {
        let selected: Rc<Vec<(PathBuf, bool)>> = Rc::new(
            self.visible()
                .iter()
                .enumerate()
                .filter(|(index, _)| self.selection.contains(*index))
                .map(|(_, entry)| (entry.path.as_ref().clone(), entry.is_dir()))
                .collect(),
        );

        Rc::new(move |entry: &Entry| {
            // 用路径而不是下标比对：视图层给出的条目可能来自上一帧的列表。
            if selected
                .iter()
                .any(|(path, _)| path == entry.path.as_ref())
            {
                fm_views::DragOut(Rc::clone(&selected))
            } else {
                fm_views::DragOut::single(entry)
            }
        })
    }

    /// 「粘贴」当前是否有内容可粘：应用内剪贴板非空，或系统剪贴板里有文件。
    pub(crate) fn can_paste(&self) -> bool {
        !self.clipboard.is_empty() || self.system_clipboard
    }

    /// 从系统剪贴板构造一份粘贴计划（资源管理器等程序复制过来的文件）。
    ///
    /// 返回 `None` 表示没有可用内容：剪贴板里没有文件、条目都已不存在、
    /// 或者「剪切」过来的条目本来就在当前目录（自己搬自己，没有意义）。
    fn system_clipboard_plan(&self, dest: &Path) -> Option<PastePlan> {
        let (paths, cut) = self
            .service
            .file_system()
            .read_file_clipboard()
            .ok()
            .flatten()?;

        let sources: Vec<PathBuf> = paths
            .into_iter()
            // 剪贴板里可能留着早就删掉的文件：粘之前按磁盘现状过滤。
            .filter(|path| path.exists())
            // 从当前目录「剪切」再粘回当前目录，等于什么也没做。
            .filter(|path| !(cut && path.parent() == Some(dest)))
            .collect();

        if sources.is_empty() {
            return None;
        }

        Some(if cut {
            PastePlan::Move {
                sources,
                dest_dir: dest.to_path_buf(),
            }
        } else {
            PastePlan::Copy {
                sources,
                dest_dir: dest.to_path_buf(),
            }
        })
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
        //
        // 应用内剪贴板为空时再去看系统剪贴板：从资源管理器复制的文件走这条路径，
        // 用户不需要先在两边各复制一次。
        let plan = match self.clipboard.plan(&dir) {
            Ok(plan) => plan,
            Err(PasteError::Empty) => match self.system_clipboard_plan(&dir) {
                Some(plan) => plan,
                None => {
                    window
                        .push_notification(Notification::warning("剪贴板里没有可粘贴的文件"), cx);
                    return;
                }
            },
            Err(error) => {
                window.push_notification(Notification::warning(error.message()), cx);
                return;
            }
        };

        let verb = match plan {
            PastePlan::Copy { .. } => ClipboardMode::Copy.action(),
            PastePlan::Move { .. } => ClipboardMode::Cut.action(),
        };
        self.begin_transfer(plan, verb, window, cx);
    }

    /// 接收从别的程序拖进来的文件/文件夹（拖入当前目录）。
    ///
    /// 统一按**复制**处理：GPUI 的 `FileDropEvent` 只带路径，不带源程序的
    /// `Preferred DropEffect`，因此拿不到「对方是剪切还是复制」。
    /// 与其猜，不如给一个确定的行为——用户想移动，复制完删掉源即可，
    /// 而猜错方向会直接弄丢用户的数据。
    pub(crate) fn accept_drop(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.current_dir() else {
            window.push_notification(Notification::warning("当前位置不能接收拖入的文件"), cx);
            return;
        };

        // 拖进自己所在的目录等于什么也没做（资源管理器在这种情况下也是空操作）。
        let sources: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| path.exists() && path.parent() != Some(dir.as_path()))
            .collect();

        if sources.is_empty() {
            window.push_notification(
                Notification::warning("拖入的项目已经在当前目录，或已不存在"),
                cx,
            );
            return;
        }

        self.begin_transfer(
            PastePlan::Copy {
                sources,
                dest_dir: dir,
            },
            ClipboardMode::Copy.action(),
            window,
            cx,
        );
    }

    /// 启动一次传输（粘贴与拖入共用）。
    ///
    /// 三件事在这里统一处理：右下角进度卡、后台线程、UI 侧轮询。
    /// 收尾（结算剪贴板、记撤销、刷列表）在 [`ShellView::finish_paste`] 里。
    fn begin_transfer(
        &mut self,
        plan: PastePlan,
        verb: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fs = Arc::clone(self.service.file_system());
        // 后台线程要一份独立所有权；外层留一份，用于收尾时结算剪贴板。
        let planned = plan.clone();

        let (count, dest_dir) = match &plan {
            PastePlan::Copy { sources, dest_dir } | PastePlan::Move { sources, dest_dir } => {
                (sources.len(), dest_dir.clone())
            }
        };
        let transfer = Arc::new(Mutex::new(Transfer {
            label: format!("{verb} {count} 项 → {}", dest_dir.display()),
            total: count,
            files_done: 0,
            bytes_done: 0,
            bytes_total: 0,
            current: PathBuf::new(),
            cancel: CancelToken::new(),
            cancelling: false,
            finished: false,
        }));
        // 后台把结果放在这里，UI 轮询到 `finished` 后取走。
        let outcome: Arc<Mutex<Option<OpResult<OpReport>>>> = Arc::new(Mutex::new(None));
        self.transfer = Some(Arc::clone(&transfer));

        // 阻塞 IO 交给普通线程，而不是后台执行器：
        // `fs::copy` 是同步阻塞调用，放在执行器上会占住一个工作线程；
        // 而这里还需要在等待期间持续读进度，把两者分开最直接。
        let io_transfer = Arc::clone(&transfer);
        let io_outcome = Arc::clone(&outcome);
        let spawned = std::thread::Builder::new()
            .name(String::from("pagefold-transfer"))
            .spawn(move || {
                let cancel = io_transfer
                    .lock()
                    .map(|state| state.cancel.clone())
                    .unwrap_or_default();
                let mut progress = |step: TransferProgress| {
                    if let Ok(mut state) = io_transfer.lock() {
                        state.files_done = step.files_done;
                        state.bytes_done = step.bytes_done;
                        state.bytes_total = step.bytes_total;
                        state.current = step.current;
                    }
                };
                let result = ops::paste_with(&*fs, &planned, &cancel, &mut progress);
                if let Ok(mut state) = io_transfer.lock() {
                    state.finished = true;
                }
                if let Ok(mut slot) = io_outcome.lock() {
                    *slot = Some(result);
                }
            });
        if let Err(error) = spawned {
            // 线程起不来时不能留下一个永远转圈的进度卡。
            self.transfer = None;
            window.push_notification(
                Notification::error(format!("无法启动传输线程：{error}")),
                cx,
            );
            return;
        }

        // UI 侧：每 120ms 刷新一次进度卡，直到后台收工，再统一收尾。
        self._transfer_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                let finished = transfer
                    .lock()
                    .map(|state| state.finished)
                    .unwrap_or(true);
                let _ = this.update_in(cx, |_view, _window, cx| cx.notify());
                if finished {
                    break;
                }
            }

            let result = outcome
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .unwrap_or_else(|| Err(String::from("传输没有返回结果")));

            let _ = this.update_in(cx, move |view, window, cx| {
                view.finish_paste(window, cx, &plan, verb, result);
            });
        }));
    }

    /// 请求取消进行中的传输。
    ///
    /// 只是置一个标志：真正的停止发生在后台线程的下一个文件边界
    /// （半截文件比「多复制了一个」危险得多，见 `fs_ops::transfer_with`）。
    pub(crate) fn cancel_transfer(&mut self, cx: &mut Context<Self>) {
        if let Some(transfer) = self.transfer.as_ref()
            && let Ok(mut state) = transfer.lock()
        {
            state.cancel.cancel();
            state.cancelling = true;
        }
        cx.notify();
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
        // 进度卡先撤掉：无论成功、失败还是取消，它都不该继续留在屏幕上。
        self.transfer = None;

        match result {
            Ok(report) => {
                // 成功的那部分先记撤销：取消也可能已经复制了好几个文件，
                // 它们同样应该可撤销。
                self.record_paste_undo(plan, &report);

                if report.cancelled {
                    // 取消**不结算剪贴板**：剩下的条目用户很可能想再粘一次，
                    // 而清空剪贴板会让他回去重选一遍。
                    window.push_notification(
                        Notification::warning(ops::summarize(verb, &report)),
                        cx,
                    );
                } else {
                    // 剪贴板怎么收尾的规则（剪切只在**全部成功**时失效，
                    // 部分失败留着重试）在 `Clipboard::complete` 里，那条规则有单测。
                    self.clipboard.complete(plan, &report);
                    self.notice_report(window, cx, verb, &report);
                }
                self.refresh_after_write(cx);
            }
            Err(reason) => window.push_notification(Notification::error(reason), cx),
        }
    }

    /// 把一次粘贴记进撤销栈。
    ///
    /// 只记**成功**的条目：失败的那些本来就没动过，把它们也算进去会让撤销
    /// 去删一个不存在的路径，报出一串「找不到文件」。
    fn record_paste_undo(&mut self, plan: &PastePlan, report: &OpReport) {
        match plan {
            PastePlan::Copy { sources, dest_dir } => {
                let created: Vec<PathBuf> = resolved_targets(sources, dest_dir, report)
                    .into_iter()
                    .map(|(_, target)| target)
                    .collect();
                if !created.is_empty() {
                    self.undo.push(UndoAction::Copy { created });
                }
            }
            PastePlan::Move { sources, dest_dir } => {
                // 记成「(现在的位置, 原来的位置)」：撤销时按目标搬回各自的源目录。
                let moved: Vec<(PathBuf, PathBuf)> = resolved_targets(sources, dest_dir, report)
                    .into_iter()
                    .map(|(source, target)| (target, source))
                    .collect();
                if !moved.is_empty() {
                    self.undo.push(UndoAction::Move { moved });
                }
            }
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
            path: path.clone(),
            input: input.clone(),
            _subscription: subscription,
        });

        let handle = input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        tracing::debug!(path = %path.display(), "开始内联重命名");
        cx.notify();
    }

    /// 提交内联重命名。
    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // `take` 同时起到「只提交一次」的保护：回车与失焦会接连触发，
        // 第二次进来时状态已空，直接返回。
        let Some(renaming) = self.renaming.take() else {
            return;
        };

        // 输入框即将消失，焦点必须交回内容根：否则焦点停在一个不再渲染的句柄上，
        // 按键分发没有起点，之后所有全局快捷键都会失效（直到用户再点一次界面）。
        window.focus(&self.shell_focus, cx);

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
        tracing::debug!(
            typed = %typed,
            old = %old_name,
            "提交内联重命名"
        );
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
        // 撤销需要知道「从哪改到哪」，而 `path` 会被移进后台任务。
        let original_path = path.clone();
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
                    // 设计稿还带一个「撤销」按钮：组件库的 Notification 没有
                    // 动作按钮位，因此改成把键位写进文案——撤销入口是 `Ctrl+Z`，
                    // 在这里告诉用户比让他去猜要可靠。
                    window.push_notification(
                        Notification::success(format!("已重命名为「{name}」（Ctrl+Z 撤销）")),
                        cx,
                    );
                    view.undo.push(UndoAction::Rename {
                        from: original_path,
                        to: new_path.clone(),
                    });
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

    /// 打开批量重命名面板。
    ///
    /// 用自绘浮层而不是组件库的 `Dialog`：面板里有两个输入框和一段**实时预览**，
    /// 而对话框的内容只在打开时构建一次——预览就不会跟着输入更新了。
    /// 自绘浮层每帧随 `render` 重建，预览天然是实时的。
    pub(crate) fn open_batch_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            window.push_notification(Notification::warning("先选中要改名的项目"), cx);
            return;
        }

        let spec = BatchSpec::default();
        let prefix = cx.new(|cx| {
            InputState::new(window, cx).default_value(spec.prefix.clone())
        });
        let start = cx.new(|cx| InputState::new(window, cx).default_value(spec.start.to_string()));

        // 两个输入框都要订阅：任何一次按键都要让预览重算。
        // `Subscription` 必须持有，否则立刻退订，表现为「预览不跟着动」。
        // 回车也在这里接：输入框自己的 `Enter` 绑定在某些焦点状态下不生效
        // （见 `apply_list_shortcuts` 的注释），而「填完就回车」是最自然的操作。
        let prefix_subscription = cx.subscribe_in(
            &prefix,
            window,
            |this, _state, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.apply_batch_rename(window, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            },
        );
        let start_subscription = cx.subscribe_in(
            &start,
            window,
            |this, _state, event: &InputEvent, window, cx| match event {
                InputEvent::Change => cx.notify(),
                InputEvent::PressEnter { .. } => this.apply_batch_rename(window, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            },
        );

        let handle = prefix.read(cx).focus_handle(cx);
        window.focus(&handle, cx);

        self.batch = Some(BatchRename {
            paths,
            prefix,
            start,
            keep_extension: spec.keep_extension,
            _prefix_subscription: prefix_subscription,
            _start_subscription: start_subscription,
        });
        cx.notify();
    }

    /// 关闭批量重命名面板（取消）。
    pub(crate) fn cancel_batch_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.batch.take().is_some() {
            // 焦点交还内容区：否则焦点停在已卸载的输入框上，之后快捷键全失效。
            window.focus(&self.shell_focus, cx);
            cx.notify();
        }
    }

    /// 切换「保留扩展名」并重算预览。
    pub(crate) fn toggle_batch_extension(&mut self, cx: &mut Context<Self>) {
        if let Some(batch) = self.batch.as_mut() {
            batch.keep_extension = !batch.keep_extension;
            cx.notify();
        }
    }

    /// 按当前参数算出预览：每一行的新旧名称、以及与现有条目撞名的行数。
    ///
    /// 撞名检查用的是**当前列表**而不是磁盘：列表就是用户眼前看到的东西，
    /// 以它为准得到的「冲突」与用户的判断一致；真正的兜底是执行时
    /// `fs::rename` 的存在性检查（那里会逐条报「已存在」）。
    pub(crate) fn batch_preview(&self, cx: &App) -> BatchPreview {
        let Some(batch) = self.batch.as_ref() else {
            return BatchPreview {
                rows: Vec::new(),
                error: Some(String::from("没有进行中的批量重命名")),
                conflicts: 0,
            };
        };

        let start_text = batch.start.read(cx).value().trim().to_string();
        let Ok(start) = start_text.parse::<u32>() else {
            return BatchPreview {
                rows: Vec::new(),
                error: Some(format!("起始序号必须是数字（现在是「{start_text}」）")),
                conflicts: 0,
            };
        };

        let spec = BatchSpec {
            prefix: batch.prefix.read(cx).value().to_string(),
            start,
            keep_extension: batch.keep_extension,
        };
        let names: Vec<String> = batch
            .paths
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();

        match batch_names(&refs, &spec) {
            Ok(new_names) => {
                // 参与改名的条目本身不算冲突：它们马上就要变成别的名字。
                let taken: std::collections::HashSet<String> = self
                    .entries
                    .iter()
                    .filter(|entry| {
                        !batch
                            .paths
                            .iter()
                            .any(|path| path.as_path() == entry.path.as_ref().as_path())
                    })
                    .map(|entry| entry.name.to_lowercase())
                    .collect();

                let mut rows = Vec::with_capacity(names.len());
                let mut conflicts = 0;
                for (old, new) in names.into_iter().zip(new_names) {
                    let conflict = taken.contains(&new.to_lowercase());
                    conflicts += usize::from(conflict);
                    rows.push(BatchRow {
                        old,
                        new,
                        conflict,
                    });
                }
                BatchPreview {
                    rows,
                    error: None,
                    conflicts,
                }
            }
            Err(error) => BatchPreview {
                rows: Vec::new(),
                error: Some(error.message()),
                conflicts: 0,
            },
        }
    }

    /// 执行批量重命名。
    pub(crate) fn apply_batch_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.batch.is_none() {
            return;
        }
        let preview = self.batch_preview(cx);
        if let Some(error) = preview.error.clone() {
            window.push_notification(Notification::warning(error), cx);
            return;
        }
        if preview.conflicts > 0 {
            window.push_notification(
                Notification::warning(format!(
                    "{} 个目标名称已存在，改一下前缀或起始序号",
                    preview.conflicts
                )),
                cx,
            );
            return;
        }

        let Some(batch) = self.batch.take() else {
            return;
        };
        window.focus(&self.shell_focus, cx);

        let items: Vec<(PathBuf, String)> = batch
            .paths
            .iter()
            .cloned()
            .zip(preview.rows.into_iter().map(|row| row.new))
            .collect();
        let fs = Arc::clone(self.service.file_system());

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::batch_rename(&*fs, &items) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| match result {
                Ok(report) => {
                    // 只把**成功**的那些记进撤销栈：失败的没动过，
                    // 记进去会让撤销去改一个不存在的名字。
                    if !report.renamed.is_empty() {
                        view.undo.push(UndoAction::RenameMany {
                            pairs: report.renamed.clone(),
                        });
                    }
                    view.notice_report(window, cx, "重命名", &report);
                    view.refresh_after_write(cx);
                }
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
        // 撤销用的是**原路径**：移入时 shell 不会告诉我们 `$R` 叫什么。
        let recorded = paths.clone();

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::recycle(&*fs, &paths) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| match result {
                Ok(count) => {
                    // 还原能力已就位（`recycle_bin::restore_by_originals`），
                    // 因此「移到回收站」现在也是可撤销的。
                    if count > 0 {
                        view.undo.push(UndoAction::Recycle { paths: recorded });
                    }
                    window.push_notification(
                        Notification::success(format!(
                            "已移到回收站 {count} 项（Ctrl+Z 撤销）"
                        )),
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

    /// 永久删除选中项。**与「移到回收站」分开的确认框。**
    ///
    /// 两个动作的确认文案必须不同：一个是「可以从回收站拿回来」，
    /// 另一个是「没有回头路」。共用一套文案，用户就没有机会在读字的时候
    /// 意识到自己按的是哪一个。
    pub(crate) fn delete_selection_forever(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = self.selected_paths();
        if paths.is_empty() || self.current_dir().is_none() {
            return;
        }

        let subject = if paths.len() == 1 {
            paths[0]
                .file_name()
                .map(|name| format!("「{}」", name.to_string_lossy()))
                .unwrap_or_else(|| String::from("该项目"))
        } else {
            format!("这 {} 个项目", paths.len())
        };

        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            let view = view.clone();
            let paths = paths.clone();

            alert
                .title("永久删除")
                .description(format!(
                    "永久删除{subject}？\n此操作无法撤销，也不会进入回收站。"
                ))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("永久删除")
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("取消")
                        .show_cancel(true),
                )
                .on_ok(move |_event, window, cx| {
                    let _ = view.update(cx, |view, cx| {
                        view.start_forever_delete(paths.clone(), window, cx);
                    });
                    true
                })
        });
    }

    /// 从回收站还原选中项。
    ///
    /// 只在回收站视图里可用：普通目录里的条目没有 `$I` 元数据，
    /// 拿路径去「还原」只会得到一条看不懂的错误。
    pub(crate) fn restore_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.in_recycle_bin() {
            window.push_notification(Notification::warning("只有在回收站里才能还原项目"), cx);
            return;
        }
        let paths = self.selected_paths();
        if paths.is_empty() {
            window.push_notification(Notification::warning("先选中要还原的项目"), cx);
            return;
        }

        let fs = Arc::clone(self.service.file_system());
        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::restore(&*fs, &paths) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| match result {
                Ok(report) => {
                    view.notice_report(window, cx, "还原", &report);
                    // 还原成功后条目就从回收站里消失了，必须重扫。
                    view.reload(cx);
                }
                Err(reason) => window.push_notification(Notification::error(reason), cx),
            });
        }));
    }

    /// 清空回收站（先弹危险确认框）。
    pub(crate) fn empty_recycle_bin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.in_recycle_bin() {
            window.push_notification(Notification::warning("请先进入回收站"), cx);
            return;
        }
        let count = self.visible().len();

        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _window, _cx| {
            let view = view.clone();
            alert
                .title("清空回收站")
                .description(format!(
                    "清空回收站里的全部 {count} 个项目？\n此操作无法撤销。"
                ))
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("清空")
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("取消")
                        .show_cancel(true),
                )
                .on_ok(move |_event, window, cx| {
                    let _ = view.update(cx, |view, cx| view.start_empty_bin(window, cx));
                    true
                })
        });
    }

    /// 真正执行清空（用户确认之后）。
    fn start_empty_bin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let fs = Arc::clone(self.service.file_system());

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::empty_bin(&*fs) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| match result {
                Ok(()) => {
                    window.push_notification(Notification::success("回收站已清空"), cx);
                    view.reload(cx);
                }
                Err(reason) => window.push_notification(Notification::error(reason), cx),
            });
        }));
    }

    /// 真正发起永久删除（用户确认之后）。
    fn start_forever_delete(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fs = Arc::clone(self.service.file_system());

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::delete_forever(&*fs, &paths) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| match result {
                Ok(report) => {
                    view.notice_report(window, cx, "删除", &report);
                    view.refresh_after_write(cx);
                }
                Err(reason) => window.push_notification(Notification::error(reason), cx),
            });
        }));
    }

    /// 撤销上一步写操作（`Ctrl+Z`）。
    ///
    /// 撤销**不记录自己**：没有「重做」入口的情况下，把撤销压回栈里
    /// 只会让 `Ctrl+Z` 在两个状态之间来回横跳。
    pub(crate) fn undo_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self.undo.pop() else {
            window.push_notification(Notification::warning("没有可撤销的操作"), cx);
            return;
        };

        let label = action.describe();
        let fs = Arc::clone(self.service.file_system());

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { ops::undo(&*fs, &action) })
                .await;

            let _ = this.update_in(cx, move |view, window, cx| match result {
                Ok(report) if report.has_failures() => {
                    // 部分失败：如实报出条数，而不是笼统地说「撤销失败」——
                    // 用户需要知道哪些东西没还原回去。
                    view.notice_report(window, cx, "撤销", &report);
                    view.refresh_after_write(cx);
                }
                Ok(_) => {
                    window.push_notification(
                        Notification::success(format!("已撤销：{label}")),
                        cx,
                    );
                    view.refresh_after_write(cx);
                }
                Err(reason) => {
                    // 撤销失败是**要紧**的：用户以为回到原样了，实际没有。
                    window.push_notification(
                        Notification::error(format!("撤销失败：{reason}")),
                        cx,
                    );
                    // 列表仍要刷新：可能已经改了一半（例如改名成功、搬回失败）。
                    view.refresh_after_write(cx);
                }
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
        // 这次变化是我们自己造成的，紧接着就会重扫；把监听的计数一并消费掉，
        // 免得 400ms 后再触发一次完全重复的刷新（那次刷新还会打断正在进行的输入）。
        self.watcher.take_change();
        self.reload_inner(cx, false);
    }

    /// 刷新的共同实现。`clear_filter` 决定要不要顺带清掉搜索条件。
    fn reload_inner(&mut self, cx: &mut Context<Self>, clear_filter: bool) {
        // 递归搜索的结果属于「另一个目录树」：一旦导航或刷新，它就过期了。
        // 不在这里清掉，用户会看到新目录里列着旧目录的搜索结果。
        self.stop_search();

        // 内联重命名会话锚在「当前列表的第 N 项」上，而刷新会把整份列表换掉。
        // 会话若留下来，它指向的条目已经不在原处：输入框不再渲染，
        // 但 `editing_input()` 仍然是真——**所有全局快捷键都会被吞掉**，
        // 表现为「按回车什么也没发生」。因此刷新一律结束该会话，
        // 而不是把一个看不见的会话留在那里。
        if self.renaming.is_some() {
            tracing::debug!("列表刷新，结束进行中的内联重命名");
            self.renaming = None;
        }
        let tab = self.tabs.active();
        let location = tab.state().location().clone();
        let sort = SortSpec::new(tab.state().sort_key(), tab.state().sort_direction());

        // 导航会换掉整份条目，任何基于下标的选中与筛选都必须一起清掉；
        // 留着它们会让「上一层的第 3 项」在新目录里被当作选中项高亮，属于纯粹的错乱。
        self.clear_selection_state();
        if clear_filter {
            self.clear_filter();
        }

        // 监听目标跟随当前位置：只有真实目录需要监听，虚拟位置直接停掉。
        // 同一目录的刷新（F5、写操作后）不会重建线程——见 `DirectoryWatcher::watch`。
        match &location {
            Location::Directory(dir) => self.watcher.watch(dir),
            _ => self.watcher.stop(),
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

    /// 环境轮询：把外部世界的两种信号搬进界面。
    ///
    /// 1. **重复启动转发来的位置**（命名管道）：有就开一个新标签并跳过去。
    /// 2. **当前目录的外部变更**：有就重新扫描一次。
    ///
    /// 两者都在 400ms 的节奏上检查（见 `ShellView::new` 里的循环）：
    /// 目录监听的事件可能每秒来几千次，直接在事件线程里刷新会把渲染打满；
    /// 400ms 把最坏情况压到每秒 2.5 次刷新，而人眼对「外部改动出现」的
    /// 容忍度远大于 400ms。
    pub(crate) fn ambient_tick(&mut self, cx: &mut Context<Self>) {
        if let Some(raw) = fm_platform::take_pending_open() {
            // 主实例可能正停在设置页，此时 IPC 请求要开一个新标签而不是替换当前页。
            // `from_shell_argument` 不认识的东西会退回「此电脑」，因此这里不再额外校验。
            let location = Location::from_shell_argument(&raw);
            self.tabs.open_at(location);
            self.reload(cx);
            return;
        }

        // 系统剪贴板探测放在最前面：它不受「重命名中」「扫描在途」的影响，
        // 否则用户在别的程序里复制完文件切回来，粘贴按钮会一直是灰的。
        self.probe_system_clipboard(cx);

        // 缩略图同理：取图与渲染解耦，这里只做「收货 + 补请求」两件事。
        // 放在重命名/扫卡守卫之前，滚动时也能持续跟进。
        self.pump_thumbnails(cx);
        self.pump_preview_text(cx);
        self.pump_search(cx);

        // 正在内联重命名时**不自动刷新**。新建一个项目会立刻触发一次文件系统变更，
        // 而重命名的输入框正是在这一刻打开的：400ms 后若来一次刷新，
        // 列表会被清空重建，输入框随之消失，用户刚敲的字也一起没了。
        // 变化计数不消费，等重命名结束后下一次轮询自然会补上这次刷新。
        if self.renaming.is_some() {
            return;
        }

        // 扫描在途时跳过：这一批结束后本来就会是最新内容，
        // 此刻再插一次刷新只会把当前扫描取消掉。
        if self.loading {
            return;
        }

        if self.watcher.take_change() {
            tracing::debug!("检测到目录变更，重新扫描当前目录");
            self.reload(cx);
        }
    }

    /// 收缩略图结果，并为可见的图片类条目补请求。
    ///
    /// 每 400ms 一次（跟 `ambient_tick` 同节奏）：结果到达就替换图标并重绘，
    /// 请求去重由 `ThumbnailService` 自己保证，因此这里每轮都扫一遍可见项是无害的。
    fn pump_thumbnails(&mut self, cx: &mut Context<Self>) {
        let ready = self.thumbnails.take_ready();
        if !ready.is_empty() {
            let mut changed = false;
            for (path, id) in ready {
                // `IconId(0)` 表示「这条取过但没有缩略图」：保持扩展名图标，
                // 记下来是为了不再重复请求（去重在服务里做，这里只跳过替换）。
                if id != IconId(0) && self.thumb_ids.insert(path, id).is_none() {
                    changed = true;
                }
            }
            if changed {
                self.patch_thumbnails();
                cx.notify();
            }
        }

        // 每次刷新后 `entries` 都是重新扫出来的（`icon` 回到扩展名图标），
        // 因此这里幂等地再写一遍——否则 F5 或自动刷新会把已经取到的缩略图抹掉。
        // 成本是每 400ms 一次哈希查表，比「在每条刷新路径上各补一次」可靠得多。
        if !self.thumb_ids.is_empty() {
            self.patch_thumbnails();
        }

        let size = self.tokens.cell.max(96.0) as u32;

        // 预览面板里的大图：无论哪种视图，**选中的那一张**都值得取。
        // 只有一条，成本可以忽略；而且面板的头图是 112px，按格子尺寸取的图刚好够用
        // （同一路径只会取一次，因此这里不会和下面的批量请求重复）。
        if let Some(path) = self.single_selected_file() {
            self.thumbnails.request(&path, size.max(128));
        }

        // 只有图标视图用得上：详情视图的图标是 16px，缩略图在那里看不出差别，
        // 白白解码一遍反而拖慢滚动。
        if self.tabs.active_state().view_mode() != ViewMode::Icons {
            return;
        }

        let requests: Vec<PathBuf> = self
            .visible()
            .iter()
            .filter(|entry| !entry.is_dir())
            .filter(|entry| {
                let extension = fm_services::icon::extension_of(entry.name.as_ref());
                fm_services::thumbnail::is_previewable(extension.as_deref())
            })
            .map(|entry| entry.path.as_ref().clone())
            .collect();

        for path in requests {
            self.thumbnails.request(&path, size);
        }
    }

    /// 把预览正文同步进只读输入框。
    ///
    /// 只能在**有 `window`** 的地方调用（输入框的 `set_value` 需要它），
    /// 因此挂在 `render` 开头，而不是 `ambient_tick`——后者只有 `Context`。
    ///
    /// 两条要点：
    /// - 没有正文时把实体一起丢掉：否则下次选中别的文件，面板会先闪一下上一条的内容；
    /// - **只在内容变化时写入**：`set_value` 会重置选区，每帧无条件写一次
    ///   会把用户刚框选的文字清空。
    fn sync_preview_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = match &self.preview_text {
            Some((_, Some(text))) => Some(text.clone()),
            _ => None,
        };

        let Some(text) = text else {
            self.preview_text_input = None;
            return;
        };

        let state = self
            .preview_text_input
            .get_or_insert_with(|| cx.new(|cx| TextareaState::new(window, cx)));

        if state.read(cx).value() != text {
            state.update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    /// 当前单选中的**文件**（目录不读内容）。
    fn single_selected_file(&self) -> Option<PathBuf> {
        let index = self.selection.single()?;
        let entry = self.visible().get(index)?;
        (!entry.is_dir()).then(|| entry.path.as_ref().clone())
    }

    /// 按需读取预览面板的文本内容。
    ///
    /// 只读**当前单选的那一个文件**：面板一次只显示一项，预读别的没有意义。
    /// 判定放在 [`fm_services::preview::read_text`] 里（按内容而不是扩展名：
    /// `README`、`.gitignore` 这类没有扩展名却最想瞄一眼的文件也要能预览）。
    fn pump_preview_text(&mut self, cx: &mut Context<Self>) {
        // 面板没显示时不必读：省掉一次文件 IO，也避免用户关掉面板后还在后台读盘。
        if !self.preview_open || self.in_settings() {
            self.preview_text = None;
            return;
        }

        let Some(path) = self.single_selected_file() else {
            // 多选 / 空选 / 选的是目录：面板不显示正文，缓存也没用了。
            self.preview_text = None;
            return;
        };

        // 已经读过这个路径（无论成功还是「不是文本」）就不再重复读。
        if self.preview_text.as_ref().is_some_and(|(known, _)| known == &path) {
            return;
        }

        self.preview_text = Some((path.clone(), None));
        self.preview_task = Some(cx.spawn(async move |this, cx| {
            let target = path.clone();
            let reading = target.clone();
            let text = cx
                .background_executor()
                .spawn(async move { fm_services::preview::read_text(&reading) })
                .await;

            if let Some(text) = &text {
                tracing::debug!(path = %target.display(), bytes = text.len(), "已生成文本预览");
            }

            let shared: Option<SharedString> = text.map(|text| text.into());
            let _ = this.update(cx, move |view, cx| {
                // 期间用户可能已经选了别的条目：只写回仍然匹配的那一份。
                if view
                    .preview_text
                    .as_ref()
                    .is_some_and(|(known, _)| known == &path)
                {
                    view.preview_text = Some((path.clone(), shared));
                    cx.notify();
                }
            });
        }));
    }

    /// 把已取到的缩略图写回条目。
    ///
    /// 改的是 `entries` / `filtered` 里的 `icon` 字段，而不是在视图层查表：
    /// 视图层只认 `Entry`，让它多一个「按路径查缩略图」的入口会把两边的状态
    /// 绑在一起（刷新、筛选、排序之后极易错位）。
    fn patch_thumbnails(&mut self) {
        if self.thumb_ids.is_empty() {
            return;
        }
        let ids = &self.thumb_ids;
        let patch = |entries: &mut [Entry]| {
            for entry in entries.iter_mut() {
                if let Some(id) = ids.get(entry.path.as_ref()) {
                    entry.icon = *id;
                }
            }
        };

        patch(&mut self.entries);
        if let Some(filtered) = self.filtered.as_mut() {
            patch(filtered);
        }
    }

    /// 节流探测系统剪贴板里是否有文件，用于「粘贴」的可用态。
    ///
    /// 只在应用内剪贴板为空时才有意义（否则应用内优先，系统里有什么都不影响），
    /// 且每 3 个 tick（约 1.2s）探测一次：`OpenClipboard` 与其它程序互斥，
    /// 探测太频繁既浪费也容易读到「被占用」的假空。
    fn probe_system_clipboard(&mut self, cx: &mut Context<Self>) {
        if !self.clipboard.is_empty() {
            if self.system_clipboard {
                self.system_clipboard = false;
                cx.notify();
            }
            return;
        }

        self.clipboard_ticks = self.clipboard_ticks.wrapping_add(1);
        if !self.clipboard_ticks.is_multiple_of(3) {
            return;
        }

        let has_files = self
            .service
            .file_system()
            .read_file_clipboard()
            .ok()
            .flatten()
            .is_some_and(|(paths, _)| !paths.is_empty());

        if has_files != self.system_clipboard {
            self.system_clipboard = has_files;
            cx.notify();
        }
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
    ///
    /// 与上次内容相同则跳过：自动刷新会频繁触发 `reload`，而会话本身几乎不变。
    fn persist_session(&mut self) {
        let snapshot = self.session_snapshot();
        if self.persisted_session.as_ref() == Some(&snapshot) {
            return;
        }
        snapshot.save();
        self.persisted_session = Some(snapshot);
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
                    // 隐藏 / 系统文件在这里过滤，而不是在平台层 `scan` 里：
                    // 「有没有隐藏属性」是平台事实，「要不要显示」是用户偏好。
                    // 在载入时过滤还能保证 `entries` 与 `visible()` 共用同一份下标空间，
                    // 选中集、框选与右键菜单都不必再关心隐藏文件是否混在里面。
                    let mut entries = entries;
                    if !view.config.show_hidden {
                        entries.retain(|entry| !entry.hidden);
                    }
                    view.entries.extend(entries);
                    // 已有筛选条件时才重算：无筛选时 `filtered` 恒为 None，
                    // 每批都克隆一次整表会白白吃掉大目录的性能预算。
                    view.refresh_filter();
                    if is_last {
                        view.loading = false;
                        // 可见条目数与扫描产出可能不同（隐藏文件被过滤掉），
                        // 这个数字是排查「文件明明在、列表里却没有」的第一手信息。
                        tracing::debug!(
                            visible = view.entries.len(),
                            show_hidden = view.config.show_hidden,
                            "目录载入完成"
                        );
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
        // 递归搜索期间列表是「搜索结果」而不是当前目录：两者的条目来源不同，
        // 但下游（渲染、选中、右键、双击）只认 `Entry`，因此不必分叉。
        if let Some(results) = &self.search_results {
            return results;
        }
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
                    icon_id: entry.icon,
                    // 只在这一条**就是**上次读过的那个路径时给出正文：
                    // 用户切走了但读取还没回来时，面板不该显示上一条的内容。
                    body: match (&self.preview_text, &self.preview_text_input) {
                        (Some((path, Some(_))), Some(state)) if path == entry.path.as_ref() => {
                            PreviewBody::Text(state.clone())
                        }
                        _ => PreviewBody::None,
                    },
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
        // 视图模式要沿用当前标签的：`TabState::new` 默认是图标视图，
        // 否则用户切到详细信息视图后，每开一个新标签都会被打回图标视图。
        self.tabs.open_at_like_active(location);
        self.reveal_active_tab();
        self.reload(cx);
    }

    /// 把当前标签滚进可视范围。
    ///
    /// 标签条只能横向滚动：新标签、`Ctrl+Shift+T` 恢复的标签都可能落在视野之外，
    /// 不主动滚过去，用户会以为「操作没生效」。
    fn reveal_active_tab(&self) {
        let active = self.tabs.active().id();
        if let Some(index) = self.tabs.tabs().iter().position(|tab| tab.id() == active) {
            self.tab_scroll.scroll_to_item(index);
        }
    }

    /// 在指定目录打开一个新标签并切过去。
    ///
    /// 与 [`ShellView::open_tab`] 的差别只在落点：后者沿用当前标签的位置。
    /// 右键菜单的「在新标签页中打开」需要一个明确的落点——目录落在它自己身上，
    /// 文件落在当前目录（文件没有「自己的目录」可言）。
    pub(crate) fn open_tab_at(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // 同上：新标签沿用**当前标签**的视图模式。
        self.tabs.open_at_like_active(Location::Directory(path));
        self.reveal_active_tab();
        self.reload(cx);
    }

    /// 关闭指定标签，并重新扫描接管焦点的那个标签。
    pub(crate) fn close_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        self.tabs.close(id);
        self.reveal_active_tab();
        self.reload(cx);
    }

    /// 恢复最近关闭的标签并切过去（Ctrl+Shift+T）。栈空时不做事。
    pub(crate) fn reopen_closed_tab(&mut self, cx: &mut Context<Self>) {
        if self.tabs.reopen_closed().is_some() {
            self.reveal_active_tab();
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
            self.reveal_active_tab();
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
    pub(crate) fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cursor.or_else(|| self.selection.single()) else {
            return;
        };
        let Some(entry) = self.visible().get(index).cloned() else {
            return;
        };
        self.open_entry(&entry, window, cx);
    }

    /// 打开一个条目：目录 → 进入；文件 → 交给系统默认程序。
    ///
    /// 两条规则：
    ///
    /// - **回收站里的条目一律拒绝**：它们的 `path` 指向 `$Recycle.Bin` 内部，
    ///   按普通路径打开要么被拒绝、要么把用户丢进系统目录。此时该做的是「还原」
    ///   （见 [`ShellView::restore_selection`]），因此这里只提示，不静默失败。
    /// - 目录走自己的导航，不交给 shell：否则会新开一个资源管理器窗口，
    ///   用户点了「打开」却看到两个窗口。
    pub(crate) fn open_entry(
        &mut self,
        entry: &Entry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.in_recycle_bin() {
            window.push_notification(
                Notification::warning("回收站里的条目不能直接打开，需要先还原"),
                cx,
            );
            return;
        }

        let path = entry.path.as_ref().clone();
        self.open_path(path, window, cx);
    }

    /// 按路径打开（预览面板只有一个路径可用，拿不到 `Entry`）。
    pub(crate) fn open_path(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.in_recycle_bin() {
            window.push_notification(
                Notification::warning("回收站里的条目不能直接打开，需要先还原"),
                cx,
            );
            return;
        }
        if path.is_dir() {
            self.navigate_to(path, cx);
        } else {
            self.shell_open(path, window, cx);
        }
    }

    /// 用系统默认程序打开一个路径（后台线程执行，避免启动慢程序时卡住窗口）。
    pub(crate) fn shell_open(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fs = Arc::clone(self.service.file_system());
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        self.op_task = Some(cx.spawn_in(window, async move |this, cx| {
            let target = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { fs.shell_open(&target) })
                .await;

            if let Err(reason) = result {
                let _ = this.update_in(cx, move |_view, window, cx| {
                    window.push_notification(
                        Notification::error(format!("无法打开「{label}」：{reason}")),
                        cx,
                    );
                });
            }
        }));
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
        // 进设置页时重读一次接管状态：用户可能刚在托盘菜单里改过，
        // 或别的工具动过注册表。读的是注册表，不缓存跨会话的状态。
        self.folder_integration = Self::exe_path()
            .map(|exe| fm_platform::folder_integration_state(&exe))
            .unwrap_or_default();

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

    /// 自身可执行文件路径。
    ///
    /// 接管写进注册表的就是**这个路径**（`"…\pagefold.exe" "%1"`），
    /// 因此程序被移动或改名之后必须重新装一次接管，否则双击文件夹会指向一个
    /// 不存在的位置——这也是设置页里那句「移动或卸载前请先恢复」的由来。
    fn exe_path() -> Option<PathBuf> {
        std::env::current_exe().ok()
    }

    /// 展开 / 折叠一组快捷键。
    pub(crate) fn toggle_shortcut_group(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(open) = self.shortcut_groups.get_mut(index) {
            *open = !*open;
            cx.notify();
        }
    }

    /// 重算设置页用的快捷键清单与冲突。
    ///
    /// 只在配置真的变了之后调用（改键 / 启停 / 恢复默认）。设置页每帧都会用到
    /// 这两个值，逐帧 `resolve` 一遍 25 条命令纯属浪费——滚动时尤其明显。
    pub(crate) fn refresh_shortcut_cache(&mut self) {
        self.shortcut_items = fm_core::resolve(&self.config.shortcuts);
        self.shortcut_conflicts = fm_core::conflicts(&self.shortcut_items);
    }

    /// 重读接管状态。写操作之后必须调用，否则开关会停在用户点击前的样子。
    fn refresh_folder_integration(&mut self) {
        if let Some(exe) = Self::exe_path() {
            self.folder_integration = fm_platform::folder_integration_state(&exe);
        }
    }

    /// 切换「在文件夹右键菜单中显示『用 Pagefold 打开』」。
    pub(crate) fn toggle_folder_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(exe) = Self::exe_path() else {
            window.push_notification(
                Notification::error("读不到本程序路径，无法修改注册表"),
                cx,
            );
            return;
        };

        let enabled = !self.folder_integration.context_menu;
        match fm_platform::set_context_menu(&exe, enabled) {
            Ok(()) => {
                self.refresh_folder_integration();
                window.push_notification(
                    Notification::success(if enabled {
                        "已在文件夹右键菜单中添加「用 Pagefold 打开」"
                    } else {
                        "已移除文件夹右键菜单项"
                    }),
                    cx,
                );
            }
            Err(error) => window.push_notification(
                Notification::error(format!("修改右键菜单失败：{error}")),
                cx,
            ),
        }
        cx.notify();
    }

    /// 切换「设为文件夹默认打开程序」。
    pub(crate) fn toggle_folder_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(exe) = Self::exe_path() else {
            window.push_notification(
                Notification::error("读不到本程序路径，无法修改注册表"),
                cx,
            );
            return;
        };

        let enabled = !self.folder_integration.default_handler;
        match fm_platform::set_default_handler(&exe, enabled) {
            Ok(()) => {
                self.refresh_folder_integration();
                window.push_notification(
                    Notification::success(if enabled {
                        "已设为文件夹默认打开程序（移动或卸载前请先恢复）"
                    } else {
                        "文件夹已交回资源管理器打开"
                    }),
                    cx,
                );
            }
            Err(error) => window.push_notification(
                Notification::error(format!("修改默认打开程序失败：{error}")),
                cx,
            ),
        }
        cx.notify();
    }

    /// 一次撤销两项接管（设置页的「恢复」按钮）。
    pub(crate) fn restore_folder_integration(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match fm_platform::restore_folder_integration() {
            Ok(()) => {
                self.refresh_folder_integration();
                window.push_notification(
                    Notification::success("已恢复资源管理器为默认"),
                    cx,
                );
            }
            Err(error) => window.push_notification(
                Notification::error(format!("恢复失败：{error}")),
                cx,
            ),
        }
        cx.notify();
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

    /// 切换「显示隐藏和系统文件」并重新加载。
    ///
    /// 必须重新加载而不能就地过滤：隐藏属性是在扫描时从文件系统读到的，
    /// 而过滤发生在批次并入的那一步（见 `begin_load`）——重扫才能让
    /// 「被过滤掉的条目」重新出现在 `entries` 里。20k 条目的重扫约 44ms，
    /// 对一个用户偶尔才按一次的开关是可以接受的代价。
    pub(crate) fn toggle_show_hidden(&mut self, cx: &mut Context<Self>) {
        self.config.show_hidden = !self.config.show_hidden;
        self.config.save();
        self.reload(cx);
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
            if self.search_recursive {
                self.start_search(cx);
            } else {
                self.refresh_filter();
                cx.notify();
            }
        }
    }

    /// 切换「在当前目录及子文件夹中搜索」。
    pub(crate) fn toggle_recursive_search(&mut self, cx: &mut Context<Self>) {
        self.search_recursive = !self.search_recursive;

        if self.search_recursive && !self.query.trim().is_empty() {
            self.start_search(cx);
        } else {
            // 关掉递归：退回「只筛当前目录」。
            self.stop_search();
            self.refresh_filter();
        }
        cx.notify();
    }

    /// 启动一次递归搜索。会先取消上一次。
    ///
    /// 每敲一个字都重来一遍：目录树遍历没有「增量更新」的低成本做法，
    /// 而取消是立刻生效的（后台每处理一条就查一次标志），
    /// 因此重来的代价只是「重新走一遍已经走过的目录」。
    fn start_search(&mut self, cx: &mut Context<Self>) {
        self.stop_search();

        let needle = self.query.trim().to_lowercase();
        let Some(root) = self.current_dir() else {
            self.refresh_filter();
            cx.notify();
            return;
        };
        if needle.is_empty() {
            self.refresh_filter();
            cx.notify();
            return;
        }

        let request = search::Request {
            root,
            needle,
            include_hidden: self.config.show_hidden,
        };
        let shared = Arc::clone(&self.search_shared);
        let cancel = self.search_cancel.clone();

        self.search_results = Some(Vec::new());
        self.search_running = true;

        self.search_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    let sink = Arc::clone(&shared);
                    search::search(&request, &cancel, &mut |batch| {
                        if let Ok(mut state) = sink.lock() {
                            state.pending.extend(batch);
                        }
                    });
                    if let Ok(mut state) = shared.lock() {
                        state.done = true;
                    }
                })
                .await;

            // 后台已经收工；把「结束」这件事告诉界面（结果本身由 tick 取走）。
            let _ = this.update(cx, |_view, cx| cx.notify());
        }));
    }

    /// 停掉搜索并清空结果，回到「当前目录列表」。
    fn stop_search(&mut self) {
        self.search_cancel.cancel();
        self.search_cancel = CancelToken::new();
        self.search_task = None;
        self.search_running = false;
        self.search_results = None;
        if let Ok(mut state) = self.search_shared.lock() {
            *state = SearchState::default();
        }
    }

    /// 取走后台搜索的批次。由 `ambient_tick` 驱动。
    fn pump_search(&mut self, cx: &mut Context<Self>) {
        let (pending, done) = match self.search_shared.lock() {
            Ok(mut state) => state.drain(),
            Err(_) => return,
        };

        let mut changed = false;
        if !pending.is_empty()
            && let Some(results) = self.search_results.as_mut()
        {
            results.extend(pending);
            changed = true;
        }

        if done && self.search_running {
            self.search_running = false;
            // 收工时排一次序：边搜边排会让结果在用户眼前反复跳动。
            if let Some(results) = self.search_results.as_mut() {
                search::sort_by_name(results);
            }
            changed = true;
        }

        if changed {
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
    pub(crate) fn clear_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let had = !self.query.is_empty();
        self.query.clear();
        self.filtered = None;
        if self.search_editing {
            self.search_editing = false;
            // 搜索框收起，焦点交回内容根（原因同 `commit_rename`）。
            window.focus(&self.shell_focus, cx);
        }
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
            move |this, _state, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.commit_address_edit(window, cx),
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
    pub(crate) fn commit_address_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.address_input.clone() else {
            self.address_editing = false;
            window.focus(&self.shell_focus, cx);
            cx.notify();
            return;
        };

        let text = state.read(cx).value().trim().to_string();
        self.address_editing = false;
        // 地址栏收起后焦点交回内容根（原因同 `commit_rename`）。
        window.focus(&self.shell_focus, cx);

        if text.is_empty() {
            // 空输入只当作「取消」，不弹错误——用户清空输入框往往就是想退出。
            cx.notify();
            return;
        }

        let path = PathBuf::from(&text);
        // `\\主机` 不是目录，但它是合法目标（列出共享）。
        // 不做这个例外，「\\10.x.x.x」就只会得到一句误导的「路径不存在」。
        if path.is_dir() || fm_platform::is_network_root(&path) {
            self.navigate_to(path, cx);
        } else if text.starts_with(r"\\") {
            // 网络路径失败的成因与本地不同，提示要跟着不同：
            // 「路径不存在」会让用户去查名字，而真正的原因往往是凭据。
            self.error = Some(
                format!(
                    "无法访问网络路径：{}（共享不存在、主机不可达，或需要先在资源管理器里输入凭据）",
                    truncate(&text, 60)
                )
                .into(),
            );
            cx.notify();
        } else {
            // 回显时截断，避免超长输入把内容区提示撑破。
            self.error = Some(format!("路径不存在：{}", truncate(&text, 80)).into());
            cx.notify();
        }
    }

    /// 结束搜索编辑态，并把焦点交回内容区。
    ///
    /// 与输入框自己那条「回车/失焦退出」路径重复，但两条都需要：
    /// 输入框的 `Enter` 绑定在某些焦点状态下不生效（见 `apply_list_shortcuts` 的注释），
    /// 那时回车会落到窗口根上的路由器，由它调到这里。
    pub(crate) fn end_search_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_editing {
            return;
        }
        self.search_editing = false;
        window.focus(&self.shell_focus, cx);
        cx.notify();
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
            move |this, state, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let value = state.read(cx).value().to_string();
                    this.set_query(value, cx);
                }
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    if this.search_editing {
                        this.search_editing = false;
                        // 搜索框收起后焦点交回内容根（原因同 `commit_rename`）。
                        window.focus(&this.shell_focus, cx);
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
        // 注意：`backspace`（回上级）、`enter`（打开）、`ctrl-a/c/x/v/z`、方向键
        // 都**不在这里**绑定。它们与输入框自己的按键冲突，必须挂在「内容区」上，
        // 见 `apply_list_shortcuts` 的注释。
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
        // 回车是唯一一个「要看上下文才知道该做什么」的键，因此单独接：
        // - 内联重命名进行中 → 提交改名；
        // - 地址栏 / 搜索框编辑中 → 让位（它们自己的输入框处理）；
        // - 其余情况 → 打开选中项。
        //
        // 为什么不能走 `bind_shortcut`：它一进编辑态就整体让位，而重命名的提交
        // 恰恰依赖回车。更重要的是**输入框自己的 `Enter` 绑定不一定生效**——
        // `Enter` 在组件库里是一个带 `Input` 上下文的 keybinding，而根元素上的
        // 无上下文绑定同样匹配 `enter`；一旦前者没被选中（实测在虚拟列表里的
        // 内联输入框上就是如此），按键会直接落到根元素。此时若这里只是「让位」，
        // 回车就成了一个彻底没人接的键：输入框看着能编辑，回车却毫无反应。
        //
        // `commit_rename` 是幂等的（先 `take`，空会话直接返回），
        // 因此即使两条路径都到，也不会提交两次。
        {
            let view = view.clone();
            root = root.on_action(move |_: &OpenItem, window, cx: &mut App| {
                let _ = view.update(cx, |this, cx| {
                    if this.batch.is_some() {
                        // 批量重命名面板打开时，回车 = 执行。
                        this.apply_batch_rename(window, cx);
                    } else if this.renaming.is_some() {
                        this.commit_rename(window, cx);
                    } else if this.address_editing {
                        // 地址栏正在编辑：提交它，而不是打开选中项。
                        this.commit_address_edit(window, cx);
                    } else if this.search_editing {
                        this.end_search_edit(window, cx);
                    } else if !this.editing_input() {
                        this.open_selected(window, cx);
                    }
                });
            });
        }

        root
    }

    /// 列表类快捷键：全部挂在**内容区**上，而不是窗口根。
    ///
    /// ## 为什么必须分开放
    ///
    /// 这些键与输入框自己的按键重合：`backspace`（删字）、`enter`（提交）、
    /// `ctrl-a/c/x/v/z`（文本全选/复制/剪切/粘贴/撤销）、`up/down`（光标）。
    /// 按键分发是「按优先级挑一条绑定，再沿**焦点节点到根**的路径找人处理」，
    /// 而 GPUI 的上下文深度排序在本版本里并不能可靠地把输入框排在前面
    /// （无上下文绑定的深度等于 `contexts.len()`，同深度时后注册者胜）。
    ///
    /// 换个角度利用同一套机制：把处理器挂在**内容区**上。输入框在命令条里，
    /// 它的分发路径不经过内容区，于是这条绑定即使被选中也**没人处理**，
    /// 分发会继续尝试下一条（输入框自己的绑定）——不需要依赖任何优先级假设。
    ///
    /// 内容区同时是「没有输入框时的焦点」，因此不编辑时它就在路径起点上，
    /// 这些键照常生效。
    fn apply_list_shortcuts(&self, list: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        let view = cx.entity().downgrade();
        let mut list = list;

        list = bind_shortcut::<GoUp>(list, &view, |this, _w, cx| {
            this.go_up(cx);
        });
        list = bind_shortcut::<SelectAll>(list, &view, |this, _w, cx| {
            this.toggle_select_all(true, cx);
        });
        list = bind_shortcut::<CopyItems>(list, &view, |this, w, cx| {
            this.clip_selection(ClipboardMode::Copy, w, cx);
        });
        list = bind_shortcut::<CutItems>(list, &view, |this, w, cx| {
            this.clip_selection(ClipboardMode::Cut, w, cx);
        });
        list = bind_shortcut::<PasteItems>(list, &view, |this, w, cx| this.paste(w, cx));
        list = bind_shortcut::<RenameItem>(list, &view, |this, w, cx| {
            this.rename_selected(w, cx);
        });
        list = bind_shortcut::<DeleteItems>(list, &view, |this, w, cx| {
            this.delete_selection(w, cx);
        });
        list = bind_shortcut::<DeletePermanently>(list, &view, |this, w, cx| {
            this.delete_selection_forever(w, cx);
        });
        list = bind_shortcut::<Undo>(list, &view, |this, w, cx| {
            this.undo_last(w, cx);
        });
        list = bind_shortcut::<MoveCursorUp>(list, &view, |this, _w, cx| {
            this.move_cursor(-1, false, cx);
        });
        list = bind_shortcut::<MoveCursorDown>(list, &view, |this, _w, cx| {
            this.move_cursor(1, false, cx);
        });
        list = bind_shortcut::<ExtendCursorUp>(list, &view, |this, _w, cx| {
            this.move_cursor(-1, true, cx);
        });
        list = bind_shortcut::<ExtendCursorDown>(list, &view, |this, _w, cx| {
            this.move_cursor(1, true, cx);
        });

        list
    }

    /// 给内容区套一层「快捷键 + 焦点」外壳。
    ///
    /// 焦点只挂在这一层（`shell_focus`）：它是列表类快捷键的分发起点，
    /// 也是没有输入框时的默认焦点。窗口根不再 `track_focus`，
    /// 否则同一个焦点句柄会被两个元素同时声明，分发树里只会认其中一个。
    fn wrap_list_area(
        &self,
        content: gpui::AnyElement,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // 拖入：GPUI 把系统级文件拖放翻译成一个 `active_drag`（载荷是 `ExternalPaths`），
        // 松手时按「鼠标下方的元素 + 载荷类型」派发 `on_drop`。因此这里不需要碰
        // OLE / `IDropTarget`，也不需要在窗口线程上做任何初始化。
        let area = div()
            .flex()
            .flex_col()
            .flex_grow(1.0)
            .min_w(px(0.0))
            .min_h(px(0.0))
            .key_context(KEY_CONTEXT)
            .track_focus(&self.shell_focus)
            .can_drop(|payload, _window, _cx| payload.is::<gpui::ExternalPaths>())
            .on_drag_move::<gpui::ExternalPaths>(cx.listener(
                |this, _event: &gpui::DragMoveEvent<gpui::ExternalPaths>, _window, cx| {
                    if !this.drop_active {
                        this.drop_active = true;
                        cx.notify();
                    }
                },
            ))
            // 拖出窗口时清掉高亮：否则它会在「拖到任务栏又放弃」之后一直亮着。
            .on_file_drop_exit(cx.listener(|this, _event, _window, cx| {
                if this.drop_active {
                    this.drop_active = false;
                    cx.notify();
                }
            }))
            .on_drop::<gpui::ExternalPaths>(cx.listener(
                |this, paths: &gpui::ExternalPaths, window, cx| {
                    this.drop_active = false;
                    let paths: Vec<PathBuf> = paths.0.iter().cloned().collect();
                    this.accept_drop(paths, window, cx);
                },
            ));

        // 悬停高亮：没有它，用户松手之前不知道这一下会不会生效。
        // 用背景色而不是描边——描边会改变布局宽度，拖动时内容会跳一格。
        let area = if self.drop_active {
            area.bg(fm_views::tokens::with_alpha(self.tokens.accent, 0.06))
        } else {
            area
        };

        self.apply_list_shortcuts(area, cx)
            .child(content)
            .into_any_element()
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 令牌每帧刷新一次：主题、密度、缩放三者的变化都会在这里被吸收，
        // 子渲染函数因此只需要读 `self.tokens`，不必各自判断主题。
        self.refresh_tokens(cx);

        // 没有任何焦点时，把焦点落到内容根上。这是全局快捷键能生效的前提：
        // 按键分发从焦点节点开始向上冒泡，没有焦点时起点是窗口根节点，
        // 而我们的处理器挂在内容根（窗口根的子节点）上，根本不在路径里。
        //
        // 只在 `None` 时接管：菜单、对话框、输入框都有自己的焦点，
        // 每帧强行夺回会让它们收不到按键。
        if window.focused(cx).is_none() {
            window.focus(&self.shell_focus, cx);
        }

        // 预览正文的输入框实体必须在这里同步：它需要一个 `&mut Window`，
        // 而 `ambient_tick` 里的回调只有 `Context`。放在渲染之前，
        // 面板这一帧看到的就已经是最新内容。
        self.sync_preview_input(window, cx);

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
        // 内容区外面套一层：列表类快捷键与默认焦点都在这一层上，
        // 输入框（在命令条里）的分发路径不经过它，因此不会再被抢占。
        let content = self.render_content(viewport, window, cx);
        let content = self.wrap_list_area(content, cx);
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
        // 批量重命名浮层盖在侧边栏浮层之上、通知与对话框层之下：
        // 它属于「内容区的一次操作」，不该盖住系统级提示。
        if let Some(batch) = self.render_batch_overlay(cx) {
            body = body.child(batch);
        }
        // 快捷键录制浮层：只有设置页里点了「修改」才会存在。
        if let Some(capture) = self.render_shortcut_overlay(cx) {
            body = body.child(capture);
        }
        // 进度卡固定在右下角，不遮内容：它只占 340px 宽的一小块。
        if let Some(card) = self.render_transfer_card(cx) {
            body = body.child(card);
        }

        let root = div()
            .size_full()
            .flex()
            .flex_col()
            // 只声明按键上下文，**不**声明焦点：焦点由内容区那一层持有
            // （见 `wrap_list_area`）。一个焦点句柄被两个元素同时 track，
            // 分发树里只会认其中一个，另一处的处理器就静默失效了。
            .key_context(KEY_CONTEXT)
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
    use super::{menu_hover_palette, mix, truncate};
    use fm_core::{Density, IconZoom};
    use gpui::hsla;

    #[test]
    fn menu_hover_uses_the_theme_text_color_not_the_background_color() {
        // 回归测试：悬停文字曾经被设成 `surface`（浅色主题下是近白、
        // 深色主题下是近黑），于是鼠标一移上去字就消失。
        // 正确取值是「hover 覆盖层 + 正常前景色」。
        for dark in [false, true] {
            let tokens = fm_views::Tokens::new(
                dark,
                Density::Normal,
                IconZoom::Medium,
                ".SystemUIFont",
                "Consolas",
            );
            let (background, foreground) = menu_hover_palette(&tokens);

            assert_eq!(background, tokens.hover, "悬停底色应当是 hover 覆盖层");
            assert_eq!(foreground, tokens.fg, "悬停文字必须是正常前景色");
            assert_ne!(
                foreground, tokens.surface,
                "文字色不能用 surface：那正是浅色下变白、深色下变黑的成因"
            );
        }
    }

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
