//! 平台抽象层。
//!
//! UI 层（`fm-shell` / `fm-views`）只面对 [`FileSystem`] 这类 trait，不感知任何 Windows API 细节；
//! 后续接入右键菜单、剪贴板、拖放时，只需在 `windows` 子模块内补 trait 实现，
//! 上层不需要改动。这也保证未来补 Linux/macOS 后端时 trait 边界保持干净。

pub mod fs;
pub mod mock;
pub mod watch;

#[cfg(target_os = "windows")]
pub mod windows;

pub use fs::{
    CancelToken, Drive, DriveKind, FileSystem, OpReport, TransferMode, TransferProgress,
};
pub use watch::{Watcher, watch_directory};

/// 返回当前平台的 [`FileSystem`] 实现。
///
/// 上层只依赖这个工厂，从而不必到处写 `#[cfg(target_os = "windows")]`。
#[cfg(target_os = "windows")]
pub fn platform_file_system() -> impl FileSystem {
    windows::WindowsFileSystem::new()
}

/// 非 Windows 宿主下退化到桩实现，保证 `cargo check` 与跨平台单元测试仍可通过。
#[cfg(not(target_os = "windows"))]
pub fn platform_file_system() -> impl FileSystem {
    mock::MockFileSystem::new(0)
}

/// 把时间格式化为**本地时区**的 `YYYY-MM-DD HH:MM`。
///
/// 平台不支持本地时区换算时返回 `None`，调用方应退化为使用
/// [`fm_core::util::format_system_time_utc`]，而不是显示空白。
#[cfg(target_os = "windows")]
pub fn format_time_local(time: std::time::SystemTime) -> Option<String> {
    windows::time::format_local(time)
}

#[cfg(not(target_os = "windows"))]
pub fn format_time_local(_time: std::time::SystemTime) -> Option<String> {
    None
}

/// 把底层错误压成**一句面向用户的话**。
///
/// 写操作失败的原因要直接进 toast，因此不能是
/// `Os { code: 5, kind: PermissionDenied, message: "拒绝访问。" }`。
/// 平台实现按错误码给出「没有权限，文件可能被占用或受保护」这类句子；
/// 认不出的错误码则保留原始文本（那句话至少是可搜索的）。
///
/// 放在平台层而不是 UI 层：错误码的含义是平台知识，
/// 上层不该出现 `if code == 5` 这种判断。
#[cfg(target_os = "windows")]
pub fn describe_error(error: &anyhow::Error) -> String {
    windows::fs_ops::describe(error)
}

/// 非 Windows 宿主没有 Win32 错误码，直接用错误自身的文本。
#[cfg(not(target_os = "windows"))]
pub fn describe_error(error: &anyhow::Error) -> String {
    error.to_string()
}

/// 按扩展名取系统图标（32×32 RGBA）。
///
/// `None` 表示取不到（无扩展名、shell 未初始化、或当前平台没有系统图标），
/// 调用方应退回内置 SVG 图标。
/// 该文件是否被标记为隐藏或系统文件。
///
/// 递归搜索要用它做过滤：那里拿到的只是 `Metadata`，没有 `Entry`，
/// `\\主机` 这种「只有服务器名、没有共享名」的路径。
///
/// 它不是目录（`is_dir` 恒为 false）：列它的共享要走 `NetShareEnum`。
/// 地址栏与导航都用这个判定，否则会给出「路径不存在」的误导提示。
#[cfg(target_os = "windows")]
pub fn is_network_root(path: &std::path::Path) -> bool {
    windows::network::is_network_root(path)
}

/// 非 Windows 平台没有 UNC 路径。
#[cfg(not(target_os = "windows"))]
pub fn is_network_root(_path: &std::path::Path) -> bool {
    false
}

/// 而「隐藏」这件事由平台层判断（Windows 看属性位，别的平台可能没有）。
#[cfg(target_os = "windows")]
pub fn is_hidden(metadata: &std::fs::Metadata) -> bool {
    windows::is_hidden(metadata)
}

/// 非 Windows 平台没有「隐藏属性」这一说。
#[cfg(not(target_os = "windows"))]
pub fn is_hidden(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub fn system_icon(extension: Option<&str>, directory: bool) -> Option<fm_core::RasterIcon> {
    windows::icon::system_icon(extension, directory)
}

/// 非 Windows 平台没有系统图标。
#[cfg(not(target_os = "windows"))]
pub fn system_icon(_extension: Option<&str>, _directory: bool) -> Option<fm_core::RasterIcon> {
    None
}

/// 取一个文件的缩略图（最长边不超过 `size` 像素）。
///
/// `None` 表示「没有预览可用」（文本、未知类型、被占用…），调用方退回扩展名图标。
///
/// **必须在工作线程上调用**：真实解码一张大图可能几十毫秒，放在渲染路径上会让滚动掉帧。
#[cfg(target_os = "windows")]
pub fn thumbnail(path: &std::path::Path, size: u32) -> Option<fm_core::RasterIcon> {
    windows::thumbnail::thumbnail(path, size)
}

/// 非 Windows 平台没有 shell 缩略图。
#[cfg(not(target_os = "windows"))]
pub fn thumbnail(_path: &std::path::Path, _size: u32) -> Option<fm_core::RasterIcon> {
    None
}

/// 单实例守卫：保证同一登录会话内只有一个进程实例。
#[cfg(target_os = "windows")]
pub use windows::single_instance::SingleInstance;

/// 把标题匹配的窗口带到前台，返回是否成功。
#[cfg(target_os = "windows")]
pub use windows::single_instance::activate_window;

/// 启动单实例命令行转发的监听（**只应由主实例调用一次**）。
#[cfg(target_os = "windows")]
pub use windows::ipc::start_open_server;

/// 把位置参数转发给已运行的实例，返回是否送达。
#[cfg(target_os = "windows")]
pub use windows::ipc::forward_open_request;

/// 取出待处理的位置参数（主实例的 UI 轮询）。
#[cfg(target_os = "windows")]
pub use windows::ipc::take_pending_open;

/// 非 Windows 平台没有单实例转发。
#[cfg(not(target_os = "windows"))]
pub fn start_open_server() {}

/// 非 Windows 平台没有单实例转发。
#[cfg(not(target_os = "windows"))]
pub fn forward_open_request(_argument: &str) -> bool {
    false
}

/// 非 Windows 平台永远不会有待处理的位置。
#[cfg(not(target_os = "windows"))]
pub fn take_pending_open() -> Option<String> {
    None
}

/// 非 Windows 平台不做实例数限制。
#[cfg(not(target_os = "windows"))]
#[derive(Debug)]
pub struct SingleInstance;

#[cfg(not(target_os = "windows"))]
impl SingleInstance {
    /// 非 Windows 平台恒为「首个实例」。
    pub fn acquire(_name: &str) -> anyhow::Result<Option<Self>> {
        Ok(Some(Self))
    }
}

/// 非 Windows 平台没有窗口激活能力。
#[cfg(not(target_os = "windows"))]
pub fn activate_window(_title: &str) -> bool {
    false
}

/// 运行热键宿主（托盘图标 + 全局唤醒热键）。**会阻塞**直到用户在托盘菜单退出。
#[cfg(target_os = "windows")]
pub use windows::hotkey_host::run as run_hotkey_host;

/// 非 Windows 平台没有热键宿主。
#[cfg(not(target_os = "windows"))]
pub fn run_hotkey_host(_app_title: &str, _exe_path: std::path::PathBuf) -> anyhow::Result<()> {
    anyhow::bail!("热键宿主仅在 Windows 上可用")
}

/// 启动热键宿主进程（若已在运行则什么也不做）。
///
/// 宿主是独立进程：主窗口关掉之后热键仍然有效。
/// 启动时带 `DETACHED_PROCESS | CREATE_NO_WINDOW`，否则它会继承父进程的控制台，
/// 或在 release 下弹出一个黑窗。
#[cfg(target_os = "windows")]
pub fn spawn_hotkey_host() {
    use std::os::windows::process::CommandExt as _;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Ok(exe_path) = std::env::current_exe() else {
        tracing::warn!("无法定位自身可执行文件，跳过热键宿主启动");
        return;
    };

    let mut command = std::process::Command::new(exe_path);
    command
        .arg("--hotkey-host")
        .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);

    match command.spawn() {
        Ok(_) => tracing::info!("已请求启动热键宿主"),
        Err(error) => tracing::warn!(%error, "热键宿主启动失败"),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn spawn_hotkey_host() {}

/// 把全局热键写进宿主要读的约定文件。`None` = 不注册。
#[cfg(target_os = "windows")]
pub fn write_hotkey_spec(keys: Option<&str>) -> anyhow::Result<()> {
    windows::hotkey::write_spec(keys)
}

#[cfg(not(target_os = "windows"))]
pub fn write_hotkey_spec(_keys: Option<&str>) -> anyhow::Result<()> {
    anyhow::bail!("全局热键仅在 Windows 上可用")
}

/// 通知正在运行的宿主重新读取热键。
///
/// 返回 `false` 表示宿主没在运行——不是错误，只是热键此刻不生效。
#[cfg(target_os = "windows")]
pub fn notify_hotkey_host() -> bool {
    windows::hotkey::notify_host()
}

#[cfg(not(target_os = "windows"))]
pub fn notify_hotkey_host() -> bool {
    false
}

/// 探测一个组合能否被注册为全局热键。
///
/// `Err` 里是给用户看的一句话（「已被其它程序占用」之类）。
/// 注意：**宿主已经占着的组合会探测失败**，调用方要自己识别这种情况。
#[cfg(target_os = "windows")]
pub fn probe_hotkey(keys: &str) -> Result<(), String> {
    windows::hotkey::probe(keys)
}

#[cfg(not(target_os = "windows"))]
pub fn probe_hotkey(_keys: &str) -> Result<(), String> {
    Err(String::from("当前平台不支持全局热键"))
}

/// 文件夹接管的当前状态。
#[cfg(target_os = "windows")]
pub use windows::folder_association::IntegrationState;

/// 读取文件夹接管的当前状态。
#[cfg(target_os = "windows")]
pub use windows::folder_association::state as folder_integration_state;

/// 安装或卸载文件夹右键菜单项。
#[cfg(target_os = "windows")]
pub use windows::folder_association::set_context_menu;

/// 安装或卸载「文件夹默认打开程序」。
#[cfg(target_os = "windows")]
pub use windows::folder_association::set_default_handler;

/// 一次性安装文件夹接管（右键菜单 + 默认打开程序）。
#[cfg(target_os = "windows")]
pub use windows::folder_association::install_all as install_folder_integration;

/// 还原全部文件夹接管改动。
#[cfg(target_os = "windows")]
pub use windows::folder_association::restore_all as restore_folder_integration;
