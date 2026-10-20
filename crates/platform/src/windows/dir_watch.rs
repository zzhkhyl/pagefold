//! `ReadDirectoryChangesW` 实现的目录变更监听。
//!
//! 形态：一个目录一个线程，同步阻塞读（不用完成端口）。理由是这个程序
//! 同一时刻只监听**一个**目录（当前标签所在目录），为此搭一套 IO 完成端口
//! 属于过度设计；同步阻塞读在独立线程里最简单，且天然按批返回。
//!
//! 只监听当前层（`bWatchSubtree = false`）：递归监听的代价随目录深度
//! 线性上升，而列表本身也只显示当前层——子目录里的变化由用户进入时的新扫描覆盖。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_CREATION,
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    ReadDirectoryChangesW,
};
use windows::Win32::System::IO::CancelIoEx;
use windows::core::HSTRING;

/// 一次读取的缓冲区大小。
///
/// 64KB 是资源管理器量级的选择：`ReadDirectoryChangesW` 在某次变更装不下时
/// 会丢弃后续记录并返回 0 字节，缓冲区越大越不容易丢；它只在监听线程里
/// 分配一次，不进入热路径。
const BUFFER_BYTES: usize = 64 * 1024;

/// 目录监听的句柄。丢弃即停止监听。
#[derive(Debug)]
pub struct Watcher {
    /// 变更计数：每收到一次（已按系统批处理的）事件自增。
    changes: Arc<AtomicU64>,
    /// 停止标志。drop 时先置位再 `CancelIoEx`，保证线程不会在阻塞调用里
    /// 永久停留（否则窗口关了、进程还挂着一个线程）。
    stop: Arc<AtomicBool>,
    /// 目录句柄的原始值，供 `CancelIoEx` 使用。
    raw_handle: isize,
    /// 线程是否已经关闭了这个句柄。关掉之后再 `CancelIoEx` 只会拿到错误，
    /// 更糟的是句柄值可能已被系统复用——因此用标志挡住。
    closed: Arc<AtomicBool>,
}

impl Watcher {
    /// 累计变更次数。调用方只需与上次读到的值比较。
    pub fn changes(&self) -> u64 {
        self.changes.load(Ordering::Relaxed)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // 线程可能正在 `ReadDirectoryChangesW` 里阻塞；只有 `CancelIoEx`
        // 能让它立刻返回（否则要等到下一次文件系统变化，可能是永远）。
        if !self.closed.load(Ordering::SeqCst) {
            unsafe {
                let _ = CancelIoEx(HANDLE(self.raw_handle as *mut _), None);
            }
        }
    }
}

/// 开始监听 `dir`（不含子目录）。
///
/// 失败原因通常是权限不足（例如 `C:\System Volume Information`）：
/// 此时返回错误，由上层决定「只是不自动刷新」，而不是让整个目录打不开。
pub fn watch_directory(dir: &Path) -> Result<Watcher> {
    let wide = HSTRING::from(dir.as_os_str());
    let handle = unsafe {
        CreateFileW(
            &wide,
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
        .with_context(|| format!("无法打开目录用于监听：{}", dir.display()))?
    };

    let raw_handle = handle.0 as isize;
    let changes = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let closed = Arc::new(AtomicBool::new(false));

    let thread_changes = Arc::clone(&changes);
    let thread_stop = Arc::clone(&stop);
    let thread_closed = Arc::clone(&closed);

    thread::Builder::new()
        .name(String::from("pagefold-dir-watch"))
        .spawn(move || {
            let mut buffer = vec![0u8; BUFFER_BYTES];
            loop {
                if thread_stop.load(Ordering::SeqCst) {
                    break;
                }
                let mut returned = 0u32;
                let result = unsafe {
                    ReadDirectoryChangesW(
                        HANDLE(raw_handle as *mut _),
                        buffer.as_mut_ptr().cast(),
                        buffer.len() as u32,
                        false,
                        FILE_NOTIFY_CHANGE_FILE_NAME
                            | FILE_NOTIFY_CHANGE_DIR_NAME
                            | FILE_NOTIFY_CHANGE_SIZE
                            | FILE_NOTIFY_CHANGE_LAST_WRITE
                            | FILE_NOTIFY_CHANGE_CREATION,
                        Some(&mut returned),
                        None,
                        None,
                    )
                };
                // 错误包含「被 CancelIoEx 打断」这一种，两种情况都该退出。
                if result.is_err() {
                    break;
                }
                // 无论读到几条记录都只记「有一次变化」：调用方要的是
                // 「该刷新了」，而不是精确的变更次数。
                // （缓冲区溢出时系统返回 0 字节并丢弃其余记录，这里同样算一次——
                // 丢了多少无法知道，保守刷新一次比漏掉更好。）
                let _ = returned;
                thread_changes.fetch_add(1, Ordering::Relaxed);
            }
            unsafe {
                let _ = CloseHandle(HANDLE(raw_handle as *mut _));
            }
            thread_closed.store(true, Ordering::SeqCst);
        })
        .context("无法创建目录监听线程")?;

    Ok(Watcher {
        changes,
        stop,
        raw_handle,
        closed,
    })
}
