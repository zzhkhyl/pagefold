//! 「快速访问」六个已知文件夹的定位：桌面、下载、文档、图片、音乐、视频。
//!
//! 走 `SHGetKnownFolderPath` 而不是拼 `%USERPROFILE%\Downloads`，
//! 这样能正确处理被用户重定向过的文件夹（例如把「下载」挪到 D 盘）。

use std::path::PathBuf;
use std::sync::Arc;

use fm_core::{Entry, EntryKind, IconId};
use windows::core::{GUID, PWSTR};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures,
    FOLDERID_Videos, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
};

/// 枚举六个快速访问项，顺序即 UI 中的展示顺序。
///
/// 单项定位失败只跳过该项，不影响其余项——「快速访问」少一格不应该让侧边栏空掉。
pub fn quick_access() -> Vec<Entry> {
    let folders: [(GUID, &str); 6] = [
        (FOLDERID_Desktop, "桌面"),
        (FOLDERID_Downloads, "下载"),
        (FOLDERID_Documents, "文档"),
        (FOLDERID_Pictures, "图片"),
        (FOLDERID_Music, "音乐"),
        (FOLDERID_Videos, "视频"),
    ];

    folders
        .into_iter()
        .filter_map(|(id, label)| {
            let path = known_folder_path(&id)?;
            Some(Entry {
                name: Arc::from(label),
                path: Arc::new(path),
                kind: EntryKind::Directory,
                size: 0,
                modified: None,
                icon: IconId(0),
            })
        })
        .collect()
}

/// 取已知文件夹的绝对路径；失败返回 `None`。
fn known_folder_path(id: &GUID) -> Option<PathBuf> {
    let raw: PWSTR = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()? };
    if raw.is_null() {
        return None;
    }

    // 手动求长度再切片，不依赖 PWSTR 的辅助方法，避免上游 API 变动带来无谓返工。
    let mut len = 0usize;
    while unsafe { *raw.0.add(len) } != 0 {
        len += 1;
    }
    let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(raw.0, len) });

    // SHGetKnownFolderPath 用 CoTaskMemAlloc 分配，必须由调用方释放，否则每次读取都漏一块。
    unsafe { CoTaskMemFree(Some(raw.0 as *const core::ffi::c_void)) };

    Some(PathBuf::from(text))
}
