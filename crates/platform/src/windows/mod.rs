//! Windows 平台实现。
//!
//! 硬性约定：所有 Windows API 调用只允许出现在本模块内。
//! `fm-services` / `fm-shell` / `fm-views` 一律不得直接依赖 `windows` crate，
//! 否则平台边界会被击穿，后续补 Linux/macOS 后端时必须回头改 UI 层。

pub mod clipboard;
pub mod dir_watch;
pub mod drives;
pub mod folder_association;
pub mod fs_ops;
pub mod hotkey_host;
pub mod icon;
pub mod ipc;
pub mod known_folders;
pub mod recycle_bin;
pub mod shell_menu;
pub mod single_instance;
pub mod time;

use std::fs;
use std::os::windows::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fm_core::{Entry, EntryKind, IconId};

use crate::fs::{Drive, FileSystem, OpReport};

/// `FILE_ATTRIBUTE_HIDDEN`：文件名以点开头在 Windows 上**不算**隐藏，
/// 只有真正打上这个属性才算。
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
/// `FILE_ATTRIBUTE_SYSTEM`：`desktop.ini`、`pagefile.sys` 这类系统文件。
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

/// 是否带「隐藏」或「系统」属性。
///
/// 刻意不用 `windows` crate 的常量：这两个值是 Win32 的稳定 ABI，
/// 而 `std::fs::MetadataExt` 已经把手伸到属性位了，没必要为此多开一个 feature。
fn is_hidden(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
}

/// Windows 上的 [`FileSystem`] 实现。
///
/// 无状态，可以放心地在多个标签页之间共享（`FileSystem` 要求 `Send + Sync + 'static`）。
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsFileSystem;

impl WindowsFileSystem {
    pub fn new() -> Self {
        Self
    }
}

impl FileSystem for WindowsFileSystem {
    fn scan(
        &self,
        dir: &Path,
        batch: usize,
        sink: &mut dyn FnMut(Vec<Entry>) -> bool,
    ) -> anyhow::Result<()> {
        // batch 为 0 会让下面的 `len() == batch` 永远不成立，退化成一次性收集整个目录。
        let batch = batch.max(1);
        let mut buffer = Vec::with_capacity(batch);

        for item in fs::read_dir(dir)? {
            // 单个条目失败（权限不足、路径超长、恰好在扫描瞬间被删除）不应中断整个目录：
            // 文件管理器宁可少一行，也不能整屏报错。
            let Ok(item) = item else {
                continue;
            };
            let Some(entry) = entry_from_dir_entry(&item) else {
                continue;
            };

            buffer.push(entry);
            if buffer.len() == batch {
                if !sink(std::mem::take(&mut buffer)) {
                    // 调用方已取消（用户切走了目录），立刻停止后续 IO。
                    return Ok(());
                }
                buffer.reserve(batch);
            }
        }

        if !buffer.is_empty() {
            // 最后一批的返回值不再需要判断：无论如何都已经扫完了。
            sink(buffer);
        }

        Ok(())
    }

    fn drives(&self) -> anyhow::Result<Vec<Drive>> {
        drives::enumerate_drives()
    }

    fn quick_access(&self) -> anyhow::Result<Vec<Entry>> {
        Ok(known_folders::quick_access())
    }

    fn stat(&self, path: &Path) -> anyhow::Result<Entry> {
        // 用 symlink_metadata：对符号链接本身做 stat，而不是跟随到目标。
        let metadata = fs::symlink_metadata(path)?;
        Ok(entry_from_metadata(path.to_path_buf(), &metadata))
    }

    fn recycle_bin(&self) -> anyhow::Result<Vec<Entry>> {
        // 枚举本身不返回错误：读不到的卷与损坏的 `$I` 都在内部跳过。
        Ok(recycle_bin::enumerate())
    }

    // ── 写操作 ──────────────────────────────────────────────────────
    //
    // 本类型无状态，因此每个方法都只是把具体实现转发给 `fs_ops` / `recycle_bin`。
    // 这一层刻意不做任何判断：判断（重名怎么办、跨卷怎么办）放在实现里，
    // 因为那需要看文件系统才能决定。

    fn create_dir(&self, parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
        fs_ops::create_dir(parent, name)
    }

    fn create_file(&self, parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
        fs_ops::create_file(parent, name)
    }

    fn rename(&self, path: &Path, new_name: &str) -> anyhow::Result<PathBuf> {
        fs_ops::rename(path, new_name)
    }

    fn copy_into(&self, sources: &[PathBuf], dest_dir: &Path) -> anyhow::Result<OpReport> {
        fs_ops::copy_into(sources, dest_dir)
    }

    fn move_into(&self, sources: &[PathBuf], dest_dir: &Path) -> anyhow::Result<OpReport> {
        fs_ops::move_into(sources, dest_dir)
    }

    fn move_to_recycle_bin(&self, paths: &[PathBuf]) -> anyhow::Result<usize> {
        recycle_bin::move_to_recycle_bin(paths)
    }

    fn delete_permanently(&self, paths: &[PathBuf]) -> anyhow::Result<OpReport> {
        Ok(fs_ops::delete_permanently(paths))
    }
}

/// 由 `DirEntry` 构造条目。
///
/// 返回 `Option`：连 `file_type` 都取不到的条目直接跳过。
fn entry_from_dir_entry(item: &fs::DirEntry) -> Option<Entry> {
    // file_type 不跟随符号链接，因此目录联接/软链能被正确识别为 Symlink。
    let file_type = item.file_type().ok()?;
    // metadata 在极端情况下会失败（比如文件刚好被删除），此时退化为「大小未知」。
    let metadata = item.metadata().ok();

    let size = metadata.as_ref().map_or(0, |meta| meta.len());
    let modified = metadata.as_ref().and_then(|meta| meta.modified().ok());
    // metadata 取不到时按「不隐藏」处理：宁可多显示一行，也不要因为一次失败
    // 让一个本该可见的条目凭空消失。
    let hidden = metadata.as_ref().is_some_and(is_hidden);
    let name: Arc<str> = item.file_name().to_string_lossy().into_owned().into();

    let kind = if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else {
        // 扩展名句柄由 fm-services 的驻留表填写：平台层不做字符串处理，
        // 保证扫描路径上没有额外的小写化与分配。
        EntryKind::File { ext: None }
    };

    Some(Entry {
        name,
        path: Arc::new(item.path()),
        kind,
        size,
        modified,
        icon: IconId(0),
        hidden,
    })
}

/// 由 `Metadata` 构造条目，供 `stat` 使用。
fn entry_from_metadata(path: std::path::PathBuf, metadata: &fs::Metadata) -> Entry {
    let file_type = metadata.file_type();
    let name: Arc<str> = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
        .into();

    let kind = if file_type.is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::File { ext: None }
    };

    Entry {
        name,
        path: Arc::new(path),
        kind,
        size: metadata.len(),
        modified: metadata.modified().ok(),
        icon: IconId(0),
        hidden: is_hidden(metadata),
    }
}
