//! 单实例之间的命令行转发（命名管道）。
//!
//! 场景：已有实例在运行时，`pagefold.exe D:\some\dir` 目前只会把窗口带到前台，
//! 位置参数被丢掉。用户的心智模型是「打开这个文件夹」，因此需要把参数交给
//! 主实例，由它开一个新标签。
//!
//! 为什么用命名管道而不是 `WM_COPYDATA`：后者需要拿到窗口句柄并挂上自己的
//! 窗口过程，而 GPUI 的窗口过程不对外开放；管道是一个独立通道，
//! 不碰 UI 框架的任何内部约定。
//!
//! 方向与生命周期：
//! - 主实例在启动后调用 [`start_open_server`]，起一个常驻线程等连接；
//! - 副本调用 [`forward_open_request`]，写一行文本即关闭；
//! - 主实例的 UI 每 400ms 轮询 [`take_pending_open`]（见 `ShellView::ambient_tick`）。
//!
//! 轮询而不是回调，是因为**平台层不该持有 `ShellView`**。一个槽位加一次轮询，
//! 换掉了「把 UI 句柄塞进平台层」这条会破坏分层的捷径。

use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::core::HSTRING;

/// 管道名。与单实例互斥体名保持同一版本号，避免新旧版本互相干扰。
///
/// 不带 `Global\` 前缀 → 作用域是当前登录会话，与互斥体的作用域一致：
/// 多用户同时登录时各自可以有自己的主实例。
const PIPE_NAME: &str = r"\\.\pipe\Pagefold.Open.v1";

/// 等待转发的命令行位置。主实例的 UI 轮询取走。
static PENDING: Mutex<Option<String>> = Mutex::new(None);

/// 启动转发监听线程（只应在主实例里调用一次）。
///
/// 线程常驻，不随标签切换而重建：它只在重复启动时被唤醒一次。
/// 创建失败（极少数，通常是句柄耗尽）只告警——退化为「只激活窗口」的旧行为，
/// 不影响主功能。
pub fn start_open_server() {
    let spawned = thread::Builder::new()
        .name(String::from("pagefold-ipc"))
        .spawn(|| {
            loop {
                let pipe = unsafe {
                    CreateNamedPipeW(
                        &HSTRING::from(PIPE_NAME),
                        PIPE_ACCESS_INBOUND,
                        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                        PIPE_UNLIMITED_INSTANCES,
                        0,
                        4096,
                        0,
                        None,
                    )
                };
                // 这个 API 用「无效句柄」而不是 `Result` 表达失败。
                if pipe.is_invalid() {
                    tracing::warn!("命名管道创建失败，重复启动将只激活窗口、不转发位置");
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }

                // 阻塞等一个客户端。客户端可能在 Create 与 Connect 之间就连上了，
                // 此时 ConnectNamedPipe 返回 ERROR_PIPE_CONNECTED——同样是「已连上」，
                // 因此不判断返回值。
                unsafe {
                    let _ = ConnectNamedPipe(pipe, None);
                }

                let mut buffer = [0u8; 4096];
                let mut read = 0u32;
                let received =
                    unsafe { ReadFile(pipe, Some(&mut buffer), Some(&mut read), None) }.is_ok()
                        && read > 0;

                if received {
                    let text = String::from_utf8_lossy(&buffer[..read as usize])
                        .trim()
                        .to_string();
                    if !text.is_empty() {
                        // 槽位只保留最后一次请求：用户连点两次「打开」时，
                        // 执行第一次请求就已经满足了意图。
                        if let Ok(mut slot) = PENDING.lock() {
                            *slot = Some(text);
                        }
                    }
                }

                unsafe {
                    let _ = DisconnectNamedPipe(pipe);
                    let _ = CloseHandle(pipe);
                }
            }
        });

    if let Err(error) = spawned {
        tracing::warn!(%error, "命令行转发线程启动失败");
    }
}

/// 把位置参数转发给主实例。返回是否送达。
///
/// 失败（主实例还没起好管道、或根本没在跑）时返回 `false`，
/// 调用方仍然会尝试激活窗口——「能激活但没跳转」好过「什么都没发生」。
pub fn forward_open_request(argument: &str) -> bool {
    if argument.is_empty() {
        return false;
    }
    unsafe {
        let Ok(pipe) = CreateFileW(
            &HSTRING::from(PIPE_NAME),
            GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        ) else {
            return false;
        };

        let mut written = 0u32;
        let ok = WriteFile(
            pipe,
            Some(argument.as_bytes()),
            Some(&mut written),
            None,
        )
        .is_ok();
        let _ = CloseHandle(pipe);
        ok && written as usize == argument.len()
    }
}

/// 取出待处理的位置参数。没有时返回 `None`。
pub fn take_pending_open() -> Option<String> {
    PENDING.lock().ok().and_then(|mut slot| slot.take())
}
