//! 热键宿主：独立的轻量后台进程，负责托盘图标与**全局唤醒热键**。
//!
//! ## 为什么是独立进程，而不是「主窗口缩到托盘」
//!
//! 1. GPUI 没有公开的「隐藏窗口」API（只有最小化与关闭通知），主窗口一旦创建就藏不起来；
//! 2. 主进程持有 DirectX 设备与 GPU 上下文，为了常驻一个热键而让它一直活着，
//!    等于用数百 MB 内存换一个按键；
//! 3. 本宿主只有托盘图标、一个热键注册与一个消息循环，内存占用在几 MB 量级。
//!
//! 代价是按下热键时通常要重新拉起主程序（冷启动约几百毫秒）。
//!
//! ## 可回退性
//!
//! **本模块不写任何注册表。** 宿主进程结束，热键注册与托盘图标随即消失，
//! 该组合自动交还系统。托盘菜单里的「退出」就是这条路径。
//!
//! ## 为什么不再用键盘钩子
//!
//! 以前用 `WH_KEYBOARD_LL` 抢 `Win+E`。那条路有个绕不开的副作用：把 `E` 吞掉之后，
//! 系统只看到「单独按了一下 Win」，松开就弹开始菜单（Win11 会先铺一层暗色遮罩，
//! 用户看到的是屏幕暗一下又亮回来）。而压制开始菜单的两条路都被实测证伪：
//!
//! - 吞掉 Win 抬起 → 系统收不到释放，之后所有按键都变成 `Win+字母`（键盘失灵）；
//! - 补发一个按键 → 无效：Windows 会丢弃合成的 Win 事件，假事件进不了组合判定。
//!
//! 现在改用 `RegisterHotKey`：由系统分发这个组合，不会被当成「单独按了 Win」，
//! 也不会吞掉任何按键。代价是只能注册**没有被占用**的组合——`Win+字母` 全被 shell
//! 占着，所以默认值用 `Ctrl+Alt+E`，具体值由用户配置（见 [`super::hotkey`]）。

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics, HICON, IDI_APPLICATION,
    IMAGE_ICON, IsWindowVisible, LoadIconW, LoadImageW, LR_SHARED, MB_ICONERROR, MB_OK, MF_CHECKED,
    MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
    SM_CXSMICON, SW_SHOW, SetForegroundWindow, ShowWindow, TPM_RIGHTBUTTON, TrackPopupMenu,
    TranslateMessage, WM_COMMAND, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASSW, WINDOW_EX_STYLE, WINDOW_STYLE,
};
use windows::core::{PCWSTR, w};

use super::folder_association;
use super::hotkey::{self, HOST_CLASS, WM_HOST_RELOAD};
use super::single_instance::{SingleInstance, activate_window};

/// 托盘图标的回调消息号。`WM_APP` 的值为 `0x8000`，其后是留给应用的自定义区间。
const WM_TRAYICON: u32 = 0x8000 + 1;

/// 「请激活或拉起主窗口」——托盘图标投递给消息循环。
///
/// 真正的窗口查找与进程创建都放到后台线程里做（见 `spawn_off_thread`），
/// 消息循环只负责转发。
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
    /// 主窗口标题，用于 `FindWindowW` 定位已运行的实例。
    app_title: String,
    /// 主程序路径，用于在未运行时拉起一个。
    exe_path: PathBuf,
}

static STATE: OnceLock<HostState> = OnceLock::new();

/// 托盘图标是否已添加。退出时必须对称地删除，否则系统托盘里会留下一个幽灵图标。
static TRAY_ADDED: AtomicBool = AtomicBool::new(false);

/// 当前是否已注册全局热键。
///
/// 重注册时先注销再注册；退出时据此决定要不要注销（没注册过就不用）。
static HOTKEY_REGISTERED: AtomicBool = AtomicBool::new(false);

/// 读取配置里的组合并注册它。
///
/// 失败**不致命**：托盘图标照常可用，只是热键按不出来。日志里会写明原因
/// （多半是「已被其它程序占用」），设置页那边也会独立做一次探测并提示用户。
///
/// 用文件而不是启动参数传组合：主程序改完配置后写文件 + 发一条
/// [`WM_HOST_RELOAD`] 即可，不必为「重载」再造一套进程间序列化。
fn register_configured_hotkey(window: HWND) {
    let Some(keys) = hotkey::read_spec() else {
        HOTKEY_REGISTERED.store(false, Ordering::Relaxed);
        tracing::info!("未配置全局热键，仅托盘图标可用");
        return;
    };

    match hotkey::register(window, &keys) {
        Ok(()) => {
            HOTKEY_REGISTERED.store(true, Ordering::Relaxed);
            tracing::info!(keys, "全局热键已注册");
        }
        Err(error) => {
            HOTKEY_REGISTERED.store(false, Ordering::Relaxed);
            // 这里只说事实，措辞交给设置页——用户能看到的地方才需要「怎么办」。
            tracing::warn!(keys, %error, "全局热键注册失败，托盘图标仍可用");
        }
    }
}

/// 运行热键宿主。**该函数会阻塞**，直到用户在托盘菜单里选择退出。
pub fn run(app_title: &str, exe_path: PathBuf) -> anyhow::Result<()> {
    // 宿主自己也要单实例：否则每启动一次主程序就多一个托盘图标与一份热键注册。
    let Some(_guard) = SingleInstance::acquire(HOST_MUTEX)? else {
        tracing::debug!("热键宿主已在运行，本次启动直接退出");
        return Ok(());
    };

    unsafe {
        let module = GetModuleHandleW(None).context("获取模块句柄失败")?;
        let instance = HINSTANCE(module.0);

        // 与 `hotkey::HOST_CLASS` 共用同一个常量：主程序靠这个类名找宿主发重载消息，
        // 两边各写一份字面量迟早会漂。
        let class_name = HOST_CLASS;
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
                app_title: app_title.to_owned(),
                exe_path,
            })
            .map_err(|_| anyhow::anyhow!("热键宿主状态已初始化"))?;

        add_tray_icon(window).context("添加托盘图标失败")?;

        register_configured_hotkey(window);

        tracing::info!("热键宿主已启动：托盘图标就绪（托盘菜单可退出）");

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        // 退出路径：先摘托盘图标，再注销热键。顺序不能反——
        // 先注销的话，图标还在但点击已无人响应。
        remove_tray_icon(window);
        hotkey::unregister(window);
        HOTKEY_REGISTERED.store(false, Ordering::Relaxed);
        tracing::info!("热键宿主已退出，全局热键已交还系统");
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
            // 用户按下了全局热键。系统已经把这个组合从输入流里拿走了，
            // 前台程序收不到它——这正是我们要的效果（不会再弹开始菜单）。
            WM_HOTKEY => {
                spawn_off_thread(open_or_launch);
                LRESULT(0)
            }
            // 主程序改完配置后让我们重注册。
            WM_HOST_RELOAD => {
                hotkey::unregister(window);
                register_configured_hotkey(window);
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
                    // 退出即彻底失效：热键注册与托盘图标随进程结束一起消失。
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


/// 把耗时动作挪到后台线程执行。
///
/// 这些动作都慢：拉起进程（冷启动可达数百毫秒）、读写注册表、弹系统提示框。
/// 消息循环是宿主唯一的事件来源——托盘点击、全局热键、重载通知都靠它——
/// 一旦被占住，用户就会觉得「按了热键没反应」。
///
/// 换成 `RegisterHotKey` 之后不再有「回调超时被系统摘掉钩子」那类硬约束，
/// 但把慢动作留给消息线程仍然没有好处，所以这条约定继续保留。
///
/// 唯一不能挪的是托盘右键菜单：`TrackPopupMenu` 必须在拥有窗口的线程上调用。
fn spawn_off_thread(action: impl FnOnce() + Send + 'static) {
    if let Err(error) = std::thread::Builder::new()
        .name(String::from("pagefold-hotkey-action"))
        .spawn(action)
    {
        // 线程起不来只是这次动作丢失，绝不能让宿主退出——否则热键与托盘都没了。
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
    // 与资源管理器 `Win+E` 的语义一致：打开的是「此电脑」。
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
        let _ = AppendMenuW(menu, MF_STRING, CMD_QUIT, w!("退出（交还全局热键）"));

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


