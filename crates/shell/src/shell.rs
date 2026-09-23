//! 主窗口外壳视图：状态定义与整体渲染入口。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fm_core::{Entry, EntryKind, Location, SortKey, TabId, TabsState, ViewMode};
use fm_platform::Drive;
use fm_services::config::Config;
use fm_services::directory::{DEFAULT_BATCH, DirectoryService, ScanBatch, SortSpec};
use fm_views::Colors;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div, px,
};
use gpui_component::input::InputState;
use gpui_component::{ActiveTheme as _, Theme, ThemeMode};

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
    /// 当前标签页已加载的条目。
    pub(crate) entries: Vec<Entry>,
    /// 是否正在扫描（状态栏与内容区据此展示进度与骨架）。
    pub(crate) loading: bool,
    /// 最近一次扫描的错误信息。
    pub(crate) error: Option<SharedString>,
    /// 地址栏是否处于可编辑态。
    pub(crate) address_editing: bool,
    /// 地址栏输入框状态。进入编辑态时才创建，避免每次渲染都重建输入状态。
    pub(crate) address_input: Option<Entity<InputState>>,
    /// 在途扫描任务。
    ///
    /// **必须持有**：`Task` 一旦被 drop 就会取消，若只把它丢掉，
    /// 扫描会在发起后立刻中止，表现为「永远停在加载中」。
    scan_task: Option<Task<()>>,
}

impl ShellView {
    /// 创建外壳，应用配置中的主题，并立即发起首次扫描。
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
            loading: false,
            error: None,
            address_editing: false,
            address_input: None,
            scan_task: None,
        };

        this.reload(cx);
        this
    }

    /// 刷新当前位置。
    ///
    /// 真实目录走扫描；「此电脑」这类虚拟位置走驱动器枚举。
    /// 这里刻意不用「把虚拟位置当成一个假路径去扫描」的做法——
    /// 那样只会得到一个「路径不存在」的错误，而不是有意义的界面。
    pub(crate) fn reload(&mut self, cx: &mut Context<Self>) {
        let tab = self.tabs.active();
        let location = tab.state().location().clone();
        let sort = SortSpec::new(tab.state().sort_key(), tab.state().sort_direction());

        match location {
            Location::Computer => self.begin_drives_load(cx),
            Location::Directory(dir) => {
                self.begin_scan(dir, sort, cx);
            }
        }
    }

    /// 发起一次扫描，返回本次扫描的代数。
    ///
    /// 副作用上，它会立刻让所有在途扫描失效——这是「快速连续切换目录时旧结果不错乱」的保障。
    pub(crate) fn begin_scan(
        &mut self,
        dir: PathBuf,
        sort: SortSpec,
        cx: &mut Context<Self>,
    ) -> u64 {
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
                    let failure = service
                        .scan(&dir, generation, sort, batch, &mut |batch| {
                            collected.push(batch);
                            true
                        })
                        .err();
                    (collected, failure)
                })
                .await;

            if let Some(failure) = failure {
                let message: SharedString = format!("无法打开该目录：{failure}").into();
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

/// 用户动作。
///
/// 集中放在一处，便于对照 PLAN.md 的交互清单逐项核对是否有遗漏；
/// 同时也让「哪些操作会触发重新扫描」这件事只有一个答案。
impl ShellView {
    /// 在当前标签所在位置打开一个新标签并切过去。
    pub(crate) fn open_tab(&mut self, cx: &mut Context<Self>) {
        // 新标签落在与当前标签相同的位置（可能是目录，也可能是「此电脑」）。
        let location = self.tabs.active_state().location().clone();
        self.tabs.open_at(location);
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

    /// 折叠 / 展开左侧导航栏并持久化。
    pub(crate) fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.config.sidebar_collapsed = !self.config.sidebar_collapsed;
        self.config.save();
        cx.notify();
    }

    /// 切换深色/浅色主题并持久化。
    ///
    /// 这里必须传入 `window`：主题里的窗口相关外观（滚动条、标题栏）依赖它才能同步刷新。
    pub(crate) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.config.dark = !self.config.dark;
        Theme::change(theme_mode(self.config.dark), Some(window), cx);
        self.config.save();
        cx.notify();
    }
}

/// 主题色到视图层色板的映射。
///
/// 只在这里映射一次，视图层就不必认识 `gpui_component::Theme` 的具体字段名，
/// 将来换主题实现也只改这一处。
pub(crate) fn palette(cx: &App) -> Colors {
    let theme = cx.theme();
    Colors {
        foreground: theme.foreground,
        muted_foreground: theme.muted_foreground,
        muted: theme.muted,
        border: theme.border,
        accent: theme.accent,
        accent_foreground: theme.accent_foreground,
        list_hover: theme.list_hover,
    }
}

/// 供视图消费的派生数据。
impl ShellView {
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

    /// 扩展名文本（小写、不含点），供图标网格挑选图标。
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
}

/// 地址栏与排序动作。
impl ShellView {
    /// 点击表头：切换排序列后重新扫描。
    ///
    /// 之所以重新扫描而不是就地对已加载条目重排，是为了让「排序键」这件事只有一个来源
    /// （`TabState`），避免内存里的顺序与实际排序设置出现分歧。
    pub(crate) fn set_sort(&mut self, key: SortKey, cx: &mut Context<Self>) {
        self.tabs.active_state_mut().toggle_sort(key);
        self.reload(cx);
    }

    /// 进入地址栏编辑态
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
        self.address_input = Some(state);
        self.address_editing = true;
        cx.notify();
    }

    /// 退出地址栏编辑态，不做跳转。
    pub(crate) fn cancel_address_edit(&mut self, cx: &mut Context<Self>) {
        self.address_editing = false;
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

#[cfg(test)]
mod tests {
    use super::truncate;

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
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 先把四个区块各自渲染成 `AnyElement` 再拼装，
        // 避免在同一个表达式里交替借用 `self` 与 `cx`。
        let toolbar = self.render_toolbar(window, cx);
        let sidebar = self.render_sidebar(window, cx);
        let content = self.render_content(window, cx);
        let status_bar = self.render_status_bar(window, cx);

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(toolbar)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_grow(1.0)
                    // min_h 0 是必须的：否则内容区会被子元素撑高，把状态栏顶出窗口。
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(sidebar)
                    .child(content),
            )
            .child(status_bar)
    }
}

/// 布尔值到主题模式的映射，供启动与切换两处共用，避免两处判断逻辑漂移。
fn theme_mode(dark: bool) -> ThemeMode {
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
