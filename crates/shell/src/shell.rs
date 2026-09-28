//! 主窗口外壳视图：状态定义、动作与整体渲染入口。
//!
//! 布局与配色规范全部来自 `design/file-manager.html`：
//! 顶部「标题栏 + 标签条 + 命令条」三行，中部「侧边栏 + 内容区 + 预览面板」，
//! 底部状态栏。尺寸与色值不写死在这里，而是通过 [`fm_views::Tokens`] 取得，
//! 换主题或改密度时只有 `tokens.rs` 需要动。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fm_core::{
    Density, Entry, EntryKind, IconZoom, Location, Selection, SortKey, TabId, TabsState, ViewMode,
};
use fm_platform::Drive;
use fm_services::config::Config;
use fm_services::directory::{DEFAULT_BATCH, DirectoryService, ScanBatch, SortSpec};
use fm_views::preview_panel::{EntrySummary, PreviewContent};
use fm_views::{Tokens, widgets};
use gpui::{
    App, AppContext as _, ClipboardItem, Context, Entity, Focusable as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window, div, px,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::{ActiveTheme as _, Theme, ThemeMode, WindowExt as _};

use crate::layout::Viewport;

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

        let mut this = Self {
            service,
            // 命令行传入的位置优先，便于从资源管理器「打开方式」、脚本或热键宿主直达目标。
            tabs: TabsState::new(
                initial_location.unwrap_or_else(|| Location::from(default_start_path())),
            ),
            config,
            drives,
            quick_access,
            entries: Vec::new(),
            filtered: None,
            loading: false,
            error: None,
            selection: Selection::default(),
            cursor: None,
            query: String::new(),
            preview_open,
            sidebar_overlay: false,
            drives_expanded: true,
            address_editing: false,
            address_input: None,
            search_editing: false,
            search_input: None,
            address_subscription: None,
            search_subscription: None,
            scan_task: None,
            tokens,
        };

        // 让上次退出时的视图模式在启动时就生效：`TabState` 只认识自己的默认值（图标视图），
        // 不把配置搬过来的话，用户每次启动都会被重置回图标视图。
        let initial_view = this.config.view_mode;
        this.tabs.active_state_mut().set_view_mode(initial_view);

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

/// 扫描与虚拟位置加载。
impl ShellView {
    /// 刷新当前位置。
    ///
    /// 真实目录走扫描；「此电脑」这类虚拟位置走驱动器枚举。
    /// 这里刻意不用「把虚拟位置当成一个假路径去扫描」的做法——
    /// 那样只会得到一个「路径不存在」的错误，而不是有意义的界面。
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        let tab = self.tabs.active();
        let location = tab.state().location().clone();
        let sort = SortSpec::new(tab.state().sort_key(), tab.state().sort_direction());

        // 导航会换掉整份条目，任何基于下标的选中与筛选都必须一起清掉；
        // 留着它们会让「上一层的第 3 项」在新目录里被当作选中项高亮，属于纯粹的错乱。
        self.clear_selection_state();
        self.query.clear();
        self.search_editing = false;
        self.filtered = None;

        match location {
            Location::Computer => self.begin_drives_load(cx),
            Location::RecycleBin => {
                self.begin_recycle_bin_load(sort, cx);
            }
            Location::Directory(dir) => {
                self.begin_scan(dir, sort, cx);
            }
        }
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
            Location::Directory(path) => path.display().to_string(),
        }
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
        self.config.dark = !self.config.dark;
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

    /// 设置界面密度并持久化。
    ///
    /// 当前没有 UI 入口——设计稿把密度开关放在独立的设置页（`file-manager-settings.html`），
    /// 而设置页不在本次交付范围内。保留这个入口是为了让设置页接入时不必改这里，
    /// 平时密度由配置文件决定（见 README「已知缺口」）。
    #[allow(dead_code)]
    pub(crate) fn set_density(&mut self, density: Density, cx: &mut Context<Self>) {
        if self.config.density == density {
            return;
        }
        self.config.density = density;
        self.config.save();
        cx.notify();
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

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 令牌每帧刷新一次：主题、密度、缩放三者的变化都会在这里被吸收，
        // 子渲染函数因此只需要读 `self.tokens`，不必各自判断主题。
        self.refresh_tokens(cx);

        let viewport = Viewport::for_width(f32::from(window.viewport_size().width));

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

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(self.tokens.surface)
            .text_color(self.tokens.fg)
            .font_family(self.tokens.font_family.clone())
            .child(tab_strip)
            .child(toolbar)
            .child(body)
            .child(status_bar)
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
