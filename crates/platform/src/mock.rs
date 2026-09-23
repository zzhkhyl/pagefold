//! 平台桩实现。
//!
//! 用途有二：
//! 1. 在非 Windows 宿主上提供编译期替代，使 `cargo check` 与单元测试仍可通过；
//! 2. 为 `fm-services` 的扫描流水线（分批、排序、取消、generation 校验）提供确定性的测试数据，
//!    不需要真的造一个万级目录就能覆盖取消与分批逻辑。
//!
//! 本实现不触碰真实文件系统。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fm_core::{Entry, EntryKind, IconId};

use crate::fs::{Drive, DriveKind, FileSystem};

/// 返回固定伪造数据的 [`FileSystem`]。
///
/// 伪造规则：每 4 个条目里有 1 个目录（`0, 4, 8, ...`），其余为 `.txt` 文件。
/// 名称带序号，因此排序结果可预测，便于断言。
#[derive(Clone, Debug, Default)]
pub struct MockFileSystem {
    /// 每个目录返回的条目数量，用于构造大目录场景。
    pub entries_per_dir: usize,
}

impl MockFileSystem {
    pub fn new(entries_per_dir: usize) -> Self {
        Self { entries_per_dir }
    }
}

impl FileSystem for MockFileSystem {
    fn scan(
        &self,
        dir: &Path,
        batch: usize,
        sink: &mut dyn FnMut(Vec<Entry>) -> bool,
    ) -> anyhow::Result<()> {
        // `batch` 为 0 时退化为单批，避免实现里出现除零或死循环。
        let batch = batch.max(1);
        let mut buffer = Vec::with_capacity(batch.min(self.entries_per_dir));

        for index in 0..self.entries_per_dir {
            let is_dir = index % 4 == 0;
            let name: Arc<str> = if is_dir {
                format!("目录 {index}").into()
            } else {
                format!("文件 {index}.txt").into()
            };

            buffer.push(Entry {
                name,
                path: Arc::new(dir.join(format!("entry-{index}"))),
                kind: if is_dir {
                    EntryKind::Directory
                } else {
                    EntryKind::File { ext: None }
                },
                size: (index as u64) * 1024,
                modified: None,
                icon: IconId(index as u32),
            });

            if buffer.len() == batch {
                // sink 返回 false 表示调用方已取消，必须立即停止生产后续批次。
                if !sink(std::mem::take(&mut buffer)) {
                    return Ok(());
                }
            }
        }

        if !buffer.is_empty() {
            sink(buffer);
        }
        Ok(())
    }

    fn drives(&self) -> anyhow::Result<Vec<Drive>> {
        Ok(vec![
            Drive {
                name: "C:".into(),
                mount_point: PathBuf::from(r"C:\"),
                kind: DriveKind::Fixed,
                total_bytes: 512 * 1024 * 1024 * 1024,
                free_bytes: 300 * 1024 * 1024 * 1024,
            },
            Drive {
                // 故意构造一个「容量未知」的设备，覆盖除零保护路径。
                name: "D:".into(),
                mount_point: PathBuf::from(r"D:\"),
                kind: DriveKind::Optical,
                total_bytes: 0,
                free_bytes: 0,
            },
        ])
    }

    fn quick_access(&self) -> anyhow::Result<Vec<Entry>> {
        Ok(Vec::new())
    }

    fn stat(&self, path: &Path) -> anyhow::Result<Entry> {
        let name: Arc<str> = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned())
            .into();

        Ok(Entry {
            name,
            path: Arc::new(path.to_path_buf()),
            kind: EntryKind::Directory,
            size: 0,
            modified: None,
            icon: IconId(0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_honours_batch_size() {
        let fs = MockFileSystem::new(1000);
        let mut batches = Vec::new();
        fs.scan(Path::new(r"C:\mock"), 512, &mut |batch| {
            batches.push(batch.len());
            true
        })
        .unwrap();

        assert_eq!(batches, vec![512, 488]);
    }

    #[test]
    fn scan_stops_immediately_when_cancelled() {
        let fs = MockFileSystem::new(100_000);
        let mut produced = 0;
        fs.scan(Path::new(r"C:\mock"), 512, &mut |batch| {
            produced += batch.len();
            // 只接受第一批，之后一律取消。
            produced < 512
        })
        .unwrap();

        assert_eq!(produced, 512, "取消后不得再生产任何条目");
    }

    #[test]
    fn used_ratio_never_divides_by_zero() {
        let drives = MockFileSystem::new(0).drives().unwrap();
        let optical = drives
            .iter()
            .find(|drive| drive.kind == DriveKind::Optical)
            .expect("mock 应包含一个容量未知的设备");
        assert_eq!(optical.used_ratio(), 0.0);
    }

    #[test]
    fn used_ratio_is_bounded_for_normal_drive() {
        let drives = MockFileSystem::new(0).drives().unwrap();
        let fixed = drives
            .iter()
            .find(|drive| drive.kind == DriveKind::Fixed)
            .expect("mock 应包含一个固定磁盘");
        let ratio = fixed.used_ratio();
        assert!((0.0..=1.0).contains(&ratio), "实际值 {ratio}");
    }
}
