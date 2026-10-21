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

/// 运行热键宿主（托盘图标 + `Win+E` 接管）。**会阻塞**直到用户在托盘菜单退出。
#[cfg(target_os = "windows")]
pub use windows::hotkey_host::run as run_hotkey_host;

/// 非 Windows 平台没有键盘钩子能力。
#[cfg(not(target_os = "windows"))]
pub fn run_hotkey_host(_app_title: &str, _exe_path: std::path::PathBuf) -> anyhow::Result<()> {
    anyhow::bail!("热键宿主仅在 Windows 上可用")
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
