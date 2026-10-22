//! 热键宿主：独立的轻量后台进程，负责托盘图标与 `Win+E` 接管。
//!
//! ## 为什么是独立进程，而不是「主窗口缩到托盘」
//!
//! 1. GPUI 没有公开的「隐藏窗口」API（只有最小化与关闭通知），主窗口一旦创建就藏不起来；
//! 2. 主进程持有 DirectX 设备与 GPU 上下文，为了常驻一个热键而让它一直活着，
//!    等于用数百 MB 内存换一个按键；
//! 3. 本宿主只有托盘图标、键盘钩子与一个消息循环，内存占用在几 MB 量级。
//!
//! 代价是 `Win+E` 时需要重新拉起主程序（冷启动约几百毫秒）。
//!
//! ## 可回退性
//!
//! **本模块不写任何注册表。** 宿主进程结束，键盘钩子与托盘图标随即消失，
//! `Win+E` 自动交还系统。托盘菜单里的「退出」就是这条路径。
//!
//! ## 线程模型
//!
//! 低层键盘钩子的回调由**安装它的线程**接收，因此钩子必须装在拥有消息循环的线程上。
//! 这里让宿主主线程同时也是消息循环线程，不额外起线程。

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LWIN, VK_RWIN};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CallNextHookEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu,
    DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, HICON,
    IDI_APPLICATION, IMAGE_ICON, IsWindowVisible, KBDLLHOOKSTRUCT, LoadIconW, LoadImageW,
    LR_SHARED, MB_ICONERROR, MB_OK, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, SM_CXSMICON, SW_SHOW, SetForegroundWindow,
    SetWindowsHookExW, ShowWindow, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_COMMAND, WM_DESTROY, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WNDCLASSW, WINDOW_EX_STYLE,
    WINDOW_STYLE,
};
use windows::core::{PCWSTR, w};

use super::folder_association;
use super::single_instance::{SingleInstance, activate_window};

/// 托盘图标的回调消息号。`WM_APP` 的值为 `0x8000`，其后是留给应用的自定义区间。
const WM_TRAYICON: u32 = 0x8000 + 1;

/// 「请激活或拉起主窗口」——由键盘钩子投递给消息循环。
///
/// 低层键盘钩子的回调**必须尽快返回**，否则系统会认为它卡死并把钩子摘掉。
/// 因此钩子里只投递消息，真正的窗口查找与进程创建都放到消息循环里做。
const WM_HOST_ACTIVATE: u32 = 0x8000 + 2;

/// 托盘的菜单命令号。
const CMD_OPEN: usize = 1;
const CMD_QUIT: usize = 2;
const CMD_TOGGLE_CONTEXT_MENU: usize = 3;
const CMD_TOGGLE_DEFAULT_HANDLER: usize = 4;
const CMD_RESTORE_EXPLORER: usize = 5;

/// 托盘图标 id（同一宿主只用一个图标）。
const TRAY_ICON_ID: u32 = 1;

/// 宿主自身的单实例互斥体名。
///
/// 与主程序的互斥体**不同名**：宿主不是主程序的第二个实例，
/// 两者必须能同时存在。
const HOST_MUTEX: &str = "Pagefold.HotkeyHost.v1";

/// `WndProc` 是 C 回调，无法捕获上下文，只能通过全局静态取状态。
///
/// 存 `isize` 而不是 `HWND`：`HWND` 内部是裸指针，不满足 `Sync`，
/// 而 `OnceLock` 要求其中的值可以跨线程共享。
struct HostState {
    window: isize,
    /// 主窗口标题，用于 `FindWindowW` 定位已运行的实例。
    app_title: String,
    /// 主程序路径，用于在未运行时拉起一个。
    exe_path: PathBuf,
}

static STATE: OnceLock<HostState> = OnceLock::new();

/// 托盘图标是否已添加。退出时必须对称地删除，否则系统托盘里会留下一个幽灵图标。
static TRAY_ADDED: AtomicBool = AtomicBool::new(false);

/// 是否需要在收到 Win 键抬起时吞掉它。
///
/// 接管 `Win+E` 时必须连 Win 键的抬起一起吞：我们吃掉了 `E`，
/// 系统就只看到「单独按下并松开 Win」，于是会弹出开始菜单。
static SWALLOW_WIN_KEYUP: AtomicBool = AtomicBool::new(false);

/// 运行热键宿主。**该函数会阻塞**，直到用户在托盘菜单里选择退出。
pub fn run(app_title: &str, exe_path: PathBuf) -> anyhow::Result<()> {
    // 宿主自己也要单实例：否则每启动一次主程序就多一个托盘图标与一层钩子。
    let Some(_guard) = SingleInstance::acquire(HOST_MUTEX)? else {
        tracing::debug!("热键宿主已在运行，本次启动直接退出");
        return Ok(());
    };

    unsafe {
        let module = GetModuleHandleW(None).context("获取模块句柄失败")?;
        let instance = HINSTANCE(module.0);

        let class_name = w!("PagefoldHotkeyHost");
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class_name,
            ..Default::default()
        };
        if RegisterClassW(&window_class) == 0 {
            anyhow::bail!("注册窗口类失败");
        }

        // 创建一个**从不显示**的窗口：它唯一的用途是接收托盘与菜单消息。
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("Files 热键宿主"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            None,
        )
        .context("创建宿主消息窗口失败")?;

        STATE
            .set(HostState {
                window: window.0 as isize,
                app_title: app_title.to_owned(),
                exe_path,
            })
            .map_err(|_| anyhow::anyhow!("热键宿主状态已初始化"))?;

        add_tray_icon(window).context("添加托盘图标失败")?;

        // 钩子装在当前线程：低层钩子的回调由安装它的线程接收，
        // 所以下面这个消息循环同时也是钩子的派发循环。
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), Some(instance), 0)
            .context("安装键盘钩子失败")?;

        tracing::info!("热键宿主已启动：托盘图标就绪，Win+E 已接管（托盘菜单可退出）");

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        // 退出路径：先摘托盘图标，再卸钩子。顺序不能反——
        // 先卸钩子的话，图标还在但点击已无人响应。
        remove_tray_icon(window);
        let _ = UnhookWindowsHookEx(hook);
        tracing::info!("热键宿主已退出，Win+E 交还系统");
    }

    Ok(())
}

/// 消息窗口的窗口过程。
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Rust 2024 起，`unsafe fn` 的函数体默认按安全代码检查，
    // 每一步 FFI 调用都要显式 `unsafe` 块（`unsafe_op_in_unsafe_fn`）。
    // 这个函数的整体就是一层 Win32 消息分发，统一包一个块比逐行标注更易读。
    unsafe {
        match message {
            WM_TRAYICON => {
                // 低 16 位是鼠标事件类型（高 16 位是图标 id，这里用不着）。
                let event = (lparam.0 as u32) & 0xffff;
                match event {
                    // 托盘左键与菜单动作一律挪到后台线程（见 `spawn_off_thread`）。
                    WM_LBUTTONUP => spawn_off_thread(open_or_launch),
                    // 右键菜单必须留在消息线程：`TrackPopupMenu` 自带模态消息循环。
                    WM_RBUTTONUP => show_tray_menu(window),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_HOST_ACTIVATE => {
                spawn_off_thread(open_or_launch);
                LRESULT(0)
            }
            WM_COMMAND => {
                match wparam.0 & 0xffff {
                    CMD_OPEN => spawn_off_thread(open_or_launch),
                    CMD_TOGGLE_CONTEXT_MENU => spawn_off_thread(toggle_context_menu),
                    CMD_TOGGLE_DEFAULT_HANDLER => spawn_off_thread(toggle_default_handler),
                    CMD_RESTORE_EXPLORER => spawn_off_thread(|| {
                        report(
                            folder_association::restore_all(),
                            "恢复资源管理器为默认",
                        )
                    }),
                    // 退出即彻底失效：钩子与图标随进程结束一起消失。
                    // 注意**不会**还原文件夹接管——那是独立的开关，
                    // 用户可能就是要退出宿主但保留文件关联。
                    CMD_QUIT => {
                        let _ = DestroyWindow(window);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

/// 低层键盘钩子：吞掉 `Win+E`，改为投递一条「激活主窗口」的消息。
unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: 低层键盘钩子的 `lParam` 恒为指向 `KBDLLHOOKSTRUCT` 的有效指针，
    // 且在整个回调期间保持有效（由系统保证）。函数体整体是一层 FFI 封装，
    // 因此统一包一个 `unsafe` 块（Rust 2024 要求 `unsafe fn` 内显式标注）。
    unsafe {
        // `code < 0` 时**必须**直接转交且不做任何处理（MSDN 明确规定）。
        if code < 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let event = wparam.0 as u32;
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);

        match event {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if info.vkCode == u32::from(b'E') && win_key_down() {
                    SWALLOW_WIN_KEYUP.store(true, Ordering::Relaxed);
                    if let Some(state) = STATE.get() {
                        let window = HWND(state.window as *mut core::ffi::c_void);
                        let _ = PostMessageW(Some(window), WM_HOST_ACTIVATE, WPARAM(0), LPARAM(0));
                    }
                    // 返回 1 = 吞掉这次按键，既不交给资源管理器，也不交给前台程序。
                    return LRESULT(1);
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                let is_win_key =
                    info.vkCode == u32::from(VK_LWIN.0) || info.vkCode == u32::from(VK_RWIN.0);
                if is_win_key && SWALLOW_WIN_KEYUP.swap(false, Ordering::Relaxed) {
                    return LRESULT(1);
                }
            }
            _ => {}
        }

        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// Win 键当前是否按下。
///
/// 用 `GetAsyncKeyState` 而不是自己跟踪按键状态：钩子可能从中间被安装，
/// 自维护的状态在安装那一刻是空的，会导致第一次 `Win+E` 漏判。
fn win_key_down() -> bool {
    let pressed = |key: i32| unsafe { (GetAsyncKeyState(key) as u16 & 0x8000) != 0 };
    pressed(i32::from(VK_LWIN.0)) || pressed(i32::from(VK_RWIN.0))
}

/// 把耗时动作挪到后台线程执行。
///
/// ## 为什么这不是「优化」而是必须
///
/// 这个线程装着**全系统**的低层键盘钩子（`WH_KEYBOARD_LL`）。钩子回调与消息循环
/// 跑在同一个线程上，所以只要它在处理某个动作时阻塞，**所有键盘输入**都要排队——
/// 用户看到的就是「打字卡顿、某些键好像失灵」。
///
/// 更糟的是系统有硬限制：回调超过 `LowLevelHooksTimeout`（默认 300ms）没返回，
/// Windows 会**直接把钩子摘掉**，之后 `Win+E` 就静默失效了。
///
/// 而这里的动作恰好都是慢的：拉起进程（冷启动可达数百毫秒）、读写注册表、
/// 弹系统提示框。因此一律丢到独立线程，消息线程只负责「收到、转发、立刻返回」。
///
/// 唯一不能挪的是托盘右键菜单：`TrackPopupMenu` 必须在拥有窗口的线程上调用。
fn spawn_off_thread(action: impl FnOnce() + Send + 'static) {
    if let Err(error) = std::thread::Builder::new()
        .name(String::from("pagefold-hotkey-action"))
        .spawn(action)
    {
        // 线程起不来只是这次动作丢失，绝不能让宿主退出——否则 `Win+E` 也没了。
        tracing::warn!(%error, "热键宿主的后台动作线程启动失败");
    }
}

/// 激活已运行的主窗口；没有就拉起一个新实例。
fn open_or_launch() {
    let Some(state) = STATE.get() else {
        return;
    };

    if activate_window(&state.app_title) {
        return;
    }

    // 没有窗口就启动一个。初始位置用 `computer`：
    // 这正是 `Win+E` 的语义——资源管理器打开的是「此电脑」。
    match Command::new(&state.exe_path).arg("computer").spawn() {
        Ok(_) => tracing::info!("已拉起新的主窗口"),
        Err(error) => tracing::error!(%error, "拉起主窗口失败"),
    }
}

/// 弹出托盘右键菜单。
unsafe fn show_tray_menu(window: HWND) {
    let Some(state) = STATE.get() else {
        return;
    };
    // 勾选态直接反映注册表里的真实状态，而不是内存里的影子变量：
    // 注册表可能被外部工具改动，而菜单每次右键都重建，读一次最准。
    let integration = folder_association::state(&state.exe_path);

    // SAFETY: `window` 是本模块创建的消息窗口，在宿主存活期间始终有效。
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        // 已安装的项显示勾选标记。两个分支都是 `MF_STRING`，
        // 让编译器从常量推断出标志位类型，不必显式写出类型名。
        let flags = |checked: bool| if checked { MF_STRING | MF_CHECKED } else { MF_STRING };

        // 菜单项的失败不至于让整个菜单不可用，因此逐项忽略返回值的错误。
        let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN, w!("打开 Files"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(
            menu,
            flags(integration.context_menu),
            CMD_TOGGLE_CONTEXT_MENU,
            w!("在文件夹右键菜单中显示「用 Files 打开」"),
        );
        let _ = AppendMenuW(
            menu,
            flags(integration.default_handler),
            CMD_TOGGLE_DEFAULT_HANDLER,
            w!("设为文件夹默认打开程序"),
        );
        let _ = AppendMenuW(
            menu,
            MF_STRING,
            CMD_RESTORE_EXPLORER,
            w!("恢复资源管理器为默认"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, w!("退出（Win+E 交还系统）"));

        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);

        let _ = SetForegroundWindow(window);
        // 第五个参数是保留位，签名要求 `Option<i32>`。
        let _ = TrackPopupMenu(menu, TPM_RIGHTBUTTON, point.x, point.y, Some(0), window, None);
        // MSDN 规定的收尾动作：不补这一条，点菜单外部时菜单不会消失。
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        // 菜单每次都是新建的，用后必须销毁，否则每弹一次泄漏一个菜单对象。
        let _ = DestroyMenu(menu);
    }
}

/// 切换「文件夹右键菜单项」开关。
fn toggle_context_menu() {
    let Some(state) = STATE.get() else {
        return;
    };
    let installed = folder_association::state(&state.exe_path).context_menu;
    report(
        folder_association::set_context_menu(&state.exe_path, !installed),
        if installed {
            "已移除文件夹右键菜单项"
        } else {
            "已添加文件夹右键菜单项"
        },
    );
}

/// 切换「文件夹默认打开程序」开关。
fn toggle_default_handler() {
    let Some(state) = STATE.get() else {
        return;
    };
    let installed = folder_association::state(&state.exe_path).default_handler;
    report(
        folder_association::set_default_handler(&state.exe_path, !installed),
        if installed {
            "已将文件夹默认打开程序交还资源管理器"
        } else {
            "已将文件夹默认打开程序指向 Files"
        },
    );
}

/// 汇报一次开关操作的结果。
///
/// 失败时**必须弹窗**：托盘菜单点下去如果只写日志，用户看到的就是「点了没反应」——
/// 而注册表写失败（被策略限制、权限不足）恰恰是最需要立刻知道的情况。
fn report(result: anyhow::Result<()>, what: &str) {
    match result {
        Ok(()) => tracing::info!("{what}"),
        Err(error) => {
            tracing::error!(%error, "{what} 失败");
            let message = to_wide(&format!("{what} 失败：\n{error}"));
            unsafe {
                MessageBoxW(
                    None,
                    PCWSTR(message.as_ptr()),
                    w!("Files"),
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    }
}

/// 转成以 NUL 结尾的 UTF-16。
fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 添加托盘图标。
unsafe fn add_tray_icon(window: HWND) -> anyhow::Result<()> {
    let Some(state) = STATE.get() else {
        anyhow::bail!("宿主状态尚未初始化");
    };

    // 用 `zeroed` 而不是 `Default`：这个结构体里有若干 C 联合体，
    // 显式清零再逐字段赋值是最不容易出错的方式。
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = window;
    data.uID = TRAY_ICON_ID;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAYICON;
    data.hIcon = unsafe { load_tray_icon() }.context("加载托盘图标失败")?;
    copy_wide(&state.app_title, &mut data.szTip);

    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        anyhow::bail!("Shell_NotifyIconW(NIM_ADD) 返回失败");
    }
    TRAY_ADDED.store(true, Ordering::Relaxed);
    Ok(())
}

/// 托盘图标。
///
/// 优先用**应用自己的图标**：它与任务栏按钮、开始菜单读的是同一份 exe 资源
/// （资源 ID 1），GPUI 注册窗口类时也是从那里取的。不用它的话托盘会显示成一个
/// 通用系统图标，与任务栏上那枚对不上——用户会以为那是另一个程序。
///
/// 尺寸取 `SM_CXSMICON`（托盘图标的标准尺寸），不用 `LR_DEFAULTSIZE`：
/// 后者拿的是 32px 大图标，被系统缩到 16px 后发虚。
///
/// 取不到时退回系统通用图标——**绝不能返回空句柄**：托盘没有图标，
/// 比图标不好看严重得多（用户根本找不到这个常驻进程）。
unsafe fn load_tray_icon() -> anyhow::Result<HICON> {
    if let Ok(module) = unsafe { GetModuleHandleW(None) } {
        let size = unsafe { GetSystemMetrics(SM_CXSMICON) };
        if let Ok(handle) = unsafe {
            LoadImageW(
                Some(module.into()),
                PCWSTR(1 as _),
                IMAGE_ICON,
                size,
                size,
                LR_SHARED,
            )
        } {
            // SAFETY: `LoadImageW` 在 `IMAGE_ICON` 下返回的句柄就是 `HICON`。
            return Ok(HICON(handle.0));
        }
    }

    unsafe { LoadIconW(None, IDI_APPLICATION) }.context("加载系统默认图标也失败")
}

/// 移除托盘图标。未添加过时是空操作。
unsafe fn remove_tray_icon(window: HWND) {
    if !TRAY_ADDED.swap(false, Ordering::Relaxed) {
        return;
    }
    // SAFETY: `NOTIFYICONDATAW` 是 POD 结构体，清零即合法；`NIM_DELETE` 只读取字段。
    unsafe {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = window;
        data.uID = TRAY_ICON_ID;
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

/// 把文本写进定长 UTF-16 缓冲区，保证以 NUL 结尾并截断超长内容。
fn copy_wide(text: &str, destination: &mut [u16]) {
    if destination.is_empty() {
        return;
    }
    let encoded: Vec<u16> = text.encode_utf16().collect();
    let length = encoded.len().min(destination.len() - 1);
    destination[..length].copy_from_slice(&encoded[..length]);
    destination[length] = 0;
}

/// 供 `activate_window` 复用的可见性判断：窗口不可见时先显示再激活。
pub(super) fn ensure_visible(window: HWND) {
    // SAFETY: 两个调用都只读取/设置窗口可见性，`window` 由调用方保证是有效句柄
    // （来自 `FindWindowW`，已判过非空）。
    unsafe {
        if !IsWindowVisible(window).as_bool() {
            let _ = ShowWindow(window, SW_SHOW);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_copy_terminates_and_truncates() {
        let mut buffer = [0xFFFFu16; 8];
        copy_wide("abc", &mut buffer);
        assert_eq!(buffer[..4], [b'a' as u16, b'b' as u16, b'c' as u16, 0]);
        // 未写入的尾部保持原样，说明只写了需要的位置。
        assert_eq!(buffer[4], 0xFFFF);
    }

    #[test]
    fn wide_copy_never_overflows_a_tight_buffer() {
        let mut buffer = [0u16; 4];
        copy_wide("abcdefgh", &mut buffer);
        // 必须留出最后一格放 NUL，否则调用方读到的字符串没有结尾。
        assert_eq!(buffer[3], 0);
        assert_eq!(buffer[..3], [b'a' as u16, b'b' as u16, b'c' as u16]);
    }

    #[test]
    fn wide_copy_handles_multibyte_text() {
        let mut buffer = [0u16; 8];
        copy_wide("文件", &mut buffer);
        assert_eq!(buffer[0], '文' as u16);
        assert_eq!(buffer[1], '件' as u16);
        assert_eq!(buffer[2], 0);
    }

    #[test]
    fn wide_copy_tolerates_an_empty_buffer() {
        let mut buffer: [u16; 0] = [];
        copy_wide("abc", &mut buffer);
    }
}
