//! 平台抽象层。
//!
//! UI 层（`fm-shell` / `fm-views`）只面对 [`FileSystem`] 这类 trait，不感知任何 Windows API 细节；
//! 后续接入右键菜单、剪贴板、拖放时，只需在 `windows` 子模块内补 trait 实现，
//! 上层不需要改动。这也保证未来补 Linux/macOS 后端时 trait 边界保持干净。

pub mod fs;
pub mod mock;

#[cfg(target_os = "windows")]
pub mod windows;

pub use fs::{Drive, DriveKind, FileSystem};

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

/// 单实例守卫：保证同一登录会话内只有一个进程实例。
#[cfg(target_os = "windows")]
pub use windows::single_instance::SingleInstance;

/// 把标题匹配的窗口带到前台，返回是否成功。
#[cfg(target_os = "windows")]
pub use windows::single_instance::activate_window;

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
