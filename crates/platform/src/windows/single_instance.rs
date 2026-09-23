//! 单实例互斥与已有实例的窗口激活。
//!
//! 用途：从资源管理器、快捷方式、或后续的 `Win+E` 热键多次启动本程序时，
//! 只保留一个进程，并把已存在的窗口带到前台，而不是每按一次就多一个进程。
//!
//! 实现上**不写任何注册表**：命名互斥体只在进程存活期间存在，退出即由系统回收；
//! 窗口激活靠 `FindWindowW` + `SetForegroundWindow`，无任何持久化改动。

use std::ptr;

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
};
use windows::core::PCWSTR;

/// 单实例守卫。
///
/// **必须持有到进程结束**：一旦 drop，互斥体句柄被关闭，其他实例就能再起来。
/// 因此调用方要用 `let _guard = ...` 绑定，而不是 `let _ = ...`（后者会立刻释放）。
#[derive(Debug)]
pub struct SingleInstance {
    handle: HANDLE,
}

impl SingleInstance {
    /// 尝试成为唯一实例。
    ///
    /// - `Ok(Some(guard))`：本进程是首个实例，守卫需持有至进程结束；
    /// - `Ok(None)`：已有实例在运行，调用方应调用 [`activate_window`] 后退出；
    /// - `Err(_)`：互斥体创建失败（极少见）。调用方应**退化为允许多实例**，
    ///   而不是让程序起不来。
    pub fn acquire(name: &str) -> anyhow::Result<Option<Self>> {
        let name_wide = wide(name);
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name_wide.as_ptr())) }?;

        // 关键点：`CreateMutexW` 在「已存在同名互斥体」时**同样返回成功句柄**，
        // 只是把 LastError 置为 ERROR_ALREADY_EXISTS。唯一性只能靠查这个错误码，
        // 不能靠函数是否返回错误。
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already_running {
            // 这个句柄用不上了，必须关掉，否则会一直占着一个内核对象引用。
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Ok(None);
        }

        Ok(Some(Self { handle }))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// 把标题匹配的窗口带到前台，返回是否成功。
///
/// 只按标题查找而不按窗口类名：类名由 GPUI 内部决定，不是我们可控的契约；
/// 标题则是我们在 `WindowOptions::titlebar` 里明确设置的。
pub fn activate_window(title: &str) -> bool {
    let title_wide = wide(title);

    // 类名传空指针表示「不限定类名」。
    let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR(ptr::null()), PCWSTR(title_wide.as_ptr())) }) else {
        return false;
    };

    // 最小化的窗口必须先还原：否则 `SetForegroundWindow` 只是让它在任务栏闪一下，
    // 用户看到的是「按了没反应」。
    if unsafe { IsIconic(hwnd) }.as_bool() {
        // ShowWindow 返回的是「此前的可见状态」，对这里没有意义，显式忽略。
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
    } else {
        // 被隐藏的窗口要先显示出来，否则 SetForegroundWindow 不会生效。
        super::hotkey_host::ensure_visible(hwnd);
    }

    // 调用方是刚被用户启动的进程，本身就是前台进程，
    // 因此这里的 SetForegroundWindow 不会被系统的前台锁策略拒绝。
    unsafe { SetForegroundWindow(hwnd) }.as_bool()
}

/// 转成以 NUL 结尾的 UTF-16，供 Win32 宽字符 API 使用。
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
