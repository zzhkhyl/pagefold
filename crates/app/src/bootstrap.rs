//! 应用引导：日志初始化、GPUI Application 创建、依赖装配与主窗口开启。
//!
//! 引导顺序不可调换：
//! 1. `with_assets` —— 不调用则图标与内置资源加载恒为空；
//! 2. `gpui_component::init` —— 必须在任何组件被构造之前调用一次，用于装配主题与全局状态；
//! 3. `open_window` —— 窗口内容的第一层必须是 `gpui_component::Root`。

use std::sync::Arc;

use anyhow::Context as _;
use fm_core::Location;
use fm_platform::{FileSystem, SingleInstance, activate_window, platform_file_system};
use fm_services::config::Config;
use fm_services::directory::DirectoryService;
use fm_shell::ShellView;
use fm_shell::actions::*;
use fm_views::metrics;
use gpui::{App, AppContext as _, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use gpui_component::Root;

/// 主窗口的初始尺寸。
///
/// 1280×820 是设计稿三栏布局（侧边栏 238 + 内容区 + 预览面板 306）的下限，
/// 再窄一点预览面板就会按断点变成浮层。
const INITIAL_WIDTH: f32 = 1280.0;
const INITIAL_HEIGHT: f32 = 820.0;

/// 主窗口标题。
///
/// 必须显式设置：GPUI 在 `titlebar.title` 为 `None` 时会用**空字符串**创建窗口，
/// 后果是任务栏没有名字，且单实例激活时 `FindWindowW` 找不到窗口。
/// 隐藏系统边框后这个文本仍然生效：任务栏与 Alt+Tab 读的都是窗口文本。
///
/// 这就是**任务栏上显示的名字**，也是应用的中文名。
const WINDOW_TITLE: &str = "文件夹管理器";

/// 单实例互斥体名。
///
/// 不带 `Global\` 前缀 → 作用域是当前登录会话，多用户同时登录时各自能开一个实例。
const INSTANCE_MUTEX: &str = "FilesGPUI.SingleInstance.v1";

/// 启动应用。该函数会阻塞直到应用退出。
pub fn run() {
    init_logging();

    // 热键宿主是同一个可执行文件的另一种运行模式：它不启动 GPUI，
    // 只跑托盘图标与键盘钩子。常驻进程因此只有几 MB，
    // 而持有 DirectX 上下文的主进程可以照常退出。
    if is_hotkey_host_invocation() {
        run_hotkey_host();
        return;
    }

    // 文件夹接管的命令行入口。执行完即退出，不启动界面：
    // 安装/卸载脚本要的是「跑完就退」，弹一个窗口反而添乱。
    if let Some(exit_code) = run_folder_integration_command() {
        std::process::exit(exit_code);
    }

    // 无论本次启动最终是主实例还是被拦截的副本，都先确保宿主在跑：
    // 放在单实例判断之前，才不会出现「宿主被杀掉后，副本启动又不管它」的空窗。
    ensure_hotkey_host();

    // 单实例守卫。注意用 `_guard` 而不是 `let _ =` 绑定：
    // `let _ =` 会**立刻** drop 守卫、释放互斥体，单实例保护等于没做。
    let _guard: Option<SingleInstance> = match SingleInstance::acquire(INSTANCE_MUTEX) {
        Ok(Some(guard)) => Some(guard),
        Ok(None) => {
            // 已有实例：把它的窗口带到前台，然后本进程直接退出。
            if activate_window(WINDOW_TITLE) {
                tracing::info!("已有实例在运行，已将其窗口带到前台");
            } else {
                // 检测到实例却激活失败时不能静默退出，否则用户会以为程序根本没反应。
                tracing::warn!("检测到已有实例，但未能定位其窗口；请手动切换到该窗口");
            }
            return;
        }
        Err(error) => {
            // 互斥体创建失败极少见。退化为允许多实例，而不是让程序起不来。
            tracing::warn!(%error, "单实例互斥体创建失败，本次启动退化为允许多实例");
            None
        }
    };

    gpui_platform::application()
        // 用 AllAssets 而不是 Assets：Assets 只嵌入 default-icons.txt 里的默认子集，
        // 而工具栏用到的 X / RefreshCw / LayoutGrid / List 等并不在其中，
        // 缺图标的后果是运行期刷出一堆 `could not find asset at path "icons/x.svg"`。
        // 代价是二进制体积变大（完整 Lucide 目录约 1800 个 svg）。
        .with_assets(gpui_kit_assets::AllAssets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);

            // 全局快捷键。键位以设计稿的 keydown 为准，另加 Ctrl+L 与 Ctrl+1/2/3。
            // 处理器挂在 ShellView 的根元素上（见 `ShellView::apply_shortcuts`）。
            cx.bind_keys([
                KeyBinding::new("ctrl-t", NewTab, None),
                KeyBinding::new("ctrl-w", CloseTab, None),
                KeyBinding::new("ctrl-shift-t", ReopenClosedTab, None),
                KeyBinding::new("alt-left", GoBack, None),
                KeyBinding::new("alt-right", GoForward, None),
                KeyBinding::new("backspace", GoUp, None),
                KeyBinding::new("f5", Refresh, None),
                KeyBinding::new("ctrl-l", FocusAddressBar, None),
                KeyBinding::new("ctrl-1", SetViewDetails, None),
                KeyBinding::new("ctrl-2", SetViewIcons, None),
                KeyBinding::new("ctrl-3", ToggleTheme, None),
                KeyBinding::new("ctrl-a", SelectAll, None),
                KeyBinding::new("ctrl-c", CopyItems, None),
                KeyBinding::new("ctrl-x", CutItems, None),
                KeyBinding::new("ctrl-v", PasteItems, None),
                KeyBinding::new("f2", RenameItem, None),
                KeyBinding::new("delete", DeleteItems, None),
                KeyBinding::new("enter", OpenItem, None),
                KeyBinding::new("up", MoveCursorUp, None),
                KeyBinding::new("down", MoveCursorDown, None),
                KeyBinding::new("shift-up", ExtendCursorUp, None),
                KeyBinding::new("shift-down", ExtendCursorDown, None),
            ]);

            // 平台实现与目录服务在整个进程内共享一份：
            // 这既省内存，也让扩展名句柄在全局唯一。
            let file_system: Arc<dyn FileSystem> = Arc::new(platform_file_system());
            let service = Arc::new(DirectoryService::new(file_system));
            let config = Config::load();
            let initial_location = parse_initial_location();

            let bounds = Bounds::centered(
                None,
                size(px(INITIAL_WIDTH), px(INITIAL_HEIGHT)),
                cx,
            );
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(
                    px(metrics::WINDOW_MIN_WIDTH),
                    px(metrics::WINDOW_MIN_HEIGHT),
                )),
                titlebar: Some(TitlebarOptions {
                    title: Some(WINDOW_TITLE.into()),
                    // 自绘标题栏：设计稿的第一行是品牌 + 状态 + 窗口级动作，
                    // 系统标题栏放不下这些，也套不上设计稿的底色与分隔线。
                    // 代价是最小化 / 最大化 / 关闭必须自己提供——见 `fm-shell::titlebar`，
                    // 那里的按钮把动作交回系统（`WindowControlArea`），不自实现窗口逻辑。
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            };

            cx.open_window(options, move |window, cx| {
                let view = cx.new(move |cx| {
                    ShellView::new(
                        Arc::clone(&service),
                        config.clone(),
                        initial_location.clone(),
                        cx,
                    )
                });
                // 窗口第一层必须是 Root，它负责挂载主题、通知层与全局快捷键。
                cx.new(|cx| Root::new(view, window, cx))
            })
            .expect("主窗口创建失败");

            cx.activate(true);
        });
}

/// 以热键宿主模式启动的开关。
const HOTKEY_HOST_FLAG: &str = "--hotkey-host";

/// 安装文件夹接管（右键菜单项 + 文件夹默认打开程序）。
const INSTALL_FOLDER_INTEGRATION: &str = "--install-folder-integration";
/// 还原文件夹接管的全部改动。
const RESTORE_FOLDER_INTEGRATION: &str = "--restore-folder-integration";
/// 打印文件夹接管的当前状态。
const FOLDER_INTEGRATION_STATUS: &str = "--folder-integration-status";

/// 处理文件夹接管的命令行入口。
///
/// 返回 `Some(退出码)` 表示本次启动只是在执行一条命令，不应继续启动界面。
fn run_folder_integration_command() -> Option<i32> {
    let command = std::env::args().find(|argument| {
        matches!(
            argument.as_str(),
            INSTALL_FOLDER_INTEGRATION | RESTORE_FOLDER_INTEGRATION | FOLDER_INTEGRATION_STATUS
        )
    })?;

    let result = match command.as_str() {
        INSTALL_FOLDER_INTEGRATION => install_folder_integration(),
        RESTORE_FOLDER_INTEGRATION => restore_folder_integration(),
        // `find` 只可能返回上面两个或下面这一个。
        _ => {
            print_folder_integration_status();
            Ok(())
        }
    };

    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            // 用 eprintln 而不是日志：这几条命令是给脚本和排错用的，
            // 输出必须能被 shell 直接捕获，不受 `RUST_LOG` 影响。
            eprintln!("失败：{error}");
            1
        }
    })
}

/// 安装「右键菜单项 + 文件夹默认打开程序」。
fn install_folder_integration() -> anyhow::Result<()> {
    let exe_path = std::env::current_exe().context("无法定位自身可执行文件")?;
    fm_platform::install_folder_integration(&exe_path)?;

    println!("已安装文件夹接管：");
    println!("  · 文件夹右键菜单新增「用 Files 打开」");
    println!("  · 双击文件夹由 Files 打开（仅限经 shell 发起的打开，例如桌面双击）");
    println!("还原：托盘菜单「恢复资源管理器为默认」，或运行 {RESTORE_FOLDER_INTEGRATION}");
    Ok(())
}

/// 还原文件夹接管的全部改动。
fn restore_folder_integration() -> anyhow::Result<()> {
    fm_platform::restore_folder_integration()?;
    println!("已还原：文件夹默认打开程序交还资源管理器，右键菜单项已移除。");
    Ok(())
}

/// 打印当前的文件夹接管状态。供脚本与排错使用。
fn print_folder_integration_status() {
    let Ok(exe_path) = std::env::current_exe() else {
        eprintln!("无法定位自身可执行文件");
        return;
    };
    let state = fm_platform::folder_integration_state(&exe_path);

    println!("右键菜单项：{}", yes_no(state.context_menu));
    println!(
        "默认打开程序：{}",
        if state.default_handler {
            "已指向 Files"
        } else {
            "资源管理器"
        }
    );
}

fn yes_no(value: bool) -> &'static str {
    if value { "已安装" } else { "未安装" }
}

/// 本次启动是否处于「热键宿主」模式。
fn is_hotkey_host_invocation() -> bool {
    std::env::args_os().any(|argument| argument.to_string_lossy() == HOTKEY_HOST_FLAG)
}

/// 运行热键宿主。**会阻塞**，直到用户在托盘菜单里选择退出。
fn run_hotkey_host() {
    let exe_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            // 拿不到自身路径就无法在 `Win+E` 时拉起主窗口。
            // 此时宁可明确失败退出，也不要留一个「图标在但点了没用」的半死进程。
            tracing::error!(%error, "无法定位自身可执行文件，热键宿主退出");
            return;
        }
    };

    if let Err(error) = fm_platform::run_hotkey_host(WINDOW_TITLE, exe_path) {
        tracing::error!(%error, "热键宿主启动失败，Win+E 将继续由系统处理");
    }
}

/// 确保热键宿主在后台运行。
///
/// 这里**无条件**拉起、由宿主自己用互斥体判断是否重复：这样省掉了
/// 「先探测再启动」的竞态（探测与启动之间宿主可能正好退出）。
/// 宿主是独立后台进程，必须带上 `DETACHED_PROCESS | CREATE_NO_WINDOW`，
/// 否则它会继承父进程的控制台，或在 release 下弹出一个黑窗。
fn ensure_hotkey_host() {
    use std::os::windows::process::CommandExt as _;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Ok(exe_path) = std::env::current_exe() else {
        tracing::warn!("无法定位自身可执行文件，跳过热键宿主启动");
        return;
    };

    let mut command = std::process::Command::new(exe_path);
    command
        .arg(HOTKEY_HOST_FLAG)
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);

    match command.spawn() {
        Ok(_) => tracing::info!("已请求启动热键宿主"),
        Err(error) => tracing::warn!(%error, "热键宿主启动失败，Win+E 将继续由系统处理"),
    }
}

/// 解析第一个位置参数为初始位置。
///
/// 除真实路径外还接受 `computer`（「此电脑」）这个关键字：
/// 虚拟位置没有路径，无法用路径表达，必须有一个显式关键字。
/// 后续 `Win+E` 接管时，热键宿主正是用它来表达「打开此电脑」这一语义。
fn parse_initial_location() -> Option<Location> {
    let raw = std::env::args_os().nth(1)?;
    // 交给 `Location` 而不是在这里手搓判断：shell 传进来的可能是
    // `::{GUID}` 形式的虚拟项路径，那套规则需要单元测试覆盖。
    Some(Location::from_shell_argument(&raw.to_string_lossy()))
}

/// 初始化 tracing。默认 INFO 级，可用 `RUST_LOG` 覆盖。
///
/// 约定：日志中禁止出现完整文件路径列表或目录内容（隐私与性能双重考虑），
/// 错误日志必须带可定位的路径上下文但需截断长度。
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
