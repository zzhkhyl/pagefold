//! 驱动器枚举与容量查询。
//!
//! 实现走 `GetLogicalDrives` + `GetDriveTypeW` + `GetDiskFreeSpaceExW`。
//!
//! 两个必须注意的点：
//! - 未插入介质的可移动设备、空光驱会返回容量 0，进度条依赖 `Drive::used_ratio` 的除零保护；
//! - 光驱与网络盘的容量查询可能阻塞数秒，因此本函数必须在 `BackgroundExecutor` 上执行，
//!   **不得**在 UI 线程同步调用。

use std::path::PathBuf;
use std::sync::Arc;

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};

use crate::fs::{Drive, DriveKind};

// `GetDriveTypeW` 的返回值。刻意写成局部常量而不引用上游导出的常量名，
// 避免将来 windows crate 调整命名时连带编译失败。
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_CDROM: u32 = 5;

/// 把 Rust 字符串转成以 NUL 结尾的 UTF-16，供 Win32 宽字符 API 使用。
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 枚举当前所有可用驱动器及其容量。
pub fn enumerate_drives() -> anyhow::Result<Vec<Drive>> {
    let mut drives = Vec::new();

    // 返回值是位掩码，位 0..=25 依次对应 A: 到 Z:。
    let mask = unsafe { GetLogicalDrives() };

    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let Some(letter) = char::from_u32(u32::from(b'A') + index) else {
            continue;
        };

        let root = format!("{letter}:\\");
        let root_wide = wide(&root);

        let kind = match unsafe { GetDriveTypeW(PCWSTR(root_wide.as_ptr())) } {
            DRIVE_FIXED => DriveKind::Fixed,
            DRIVE_REMOVABLE => DriveKind::Removable,
            DRIVE_REMOTE => DriveKind::Network,
            DRIVE_CDROM => DriveKind::Optical,
            _ => DriveKind::Other,
        };

        let mut total_bytes = 0u64;
        let mut free_bytes = 0u64;
        // 查询失败不是错误：空光驱、未插卡的读卡器、无权限的网络盘都会走到这里。
        // 仍然把盘符列出来，只是容量记 0（进度条此时显示为 0%）。
        let _ = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(root_wide.as_ptr()),
                None,
                Some(&mut total_bytes),
                Some(&mut free_bytes),
            )
        };

        drives.push(Drive {
            name: Arc::from(format!("{letter}:")),
            mount_point: PathBuf::from(root),
            kind,
            total_bytes,
            free_bytes,
        });
    }

    Ok(drives)
}
