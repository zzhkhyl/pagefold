//! 平台桩实现。
//!
//! 用途有二：
//! 1. 在非 Windows 宿主上提供编译期替代，使 `cargo check` 与单元测试仍可通过；
//! 2. 为 `fm-services` 的扫描流水线（分批、排序、取消、generation 校验）提供确定性的测试数据，
//!    不需要真的造一个万级目录就能覆盖取消与分批逻辑。
//!
//! 本实现不触碰真实文件系统。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use fm_core::{Entry, EntryKind, IconId};

use crate::fs::{Drive, DriveKind, FileSystem, OpReport};

/// 桩记录下的一次写操作。
///
/// 存在的意义是让**上层**的编排逻辑可测：`fm-services` 里「剪切后粘贴应当走
/// move、复制后粘贴应当走 copy」这类断言，只能靠「它到底调了什么」来验证——
/// 换成真实实现就会真的动磁盘。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockWrite {
    CreateDir { parent: PathBuf, name: String },
    CreateFile { parent: PathBuf, name: String },
    Rename { path: PathBuf, new_name: String },
    Copy { sources: Vec<PathBuf>, dest_dir: PathBuf },
    Move { sources: Vec<PathBuf>, dest_dir: PathBuf },
    Recycle { paths: Vec<PathBuf> },
    /// 永久删除。撤销「新建 / 复制」时会走到这里，因此必须可观测。
    DeletePermanently { paths: Vec<PathBuf> },
    /// 用系统默认程序打开。
    ShellOpen { path: PathBuf },
}

/// 返回固定伪造数据的 [`FileSystem`]。
///
/// 伪造规则：每 4 个条目里有 1 个目录（`0, 4, 8, ...`），其余为 `.txt` 文件。
/// 名称带序号，因此排序结果可预测，便于断言。
#[derive(Clone, Debug, Default)]
pub struct MockFileSystem {
    /// 每个目录返回的条目数量，用于构造大目录场景。
    pub entries_per_dir: usize,
    /// 写操作日志。
    ///
    /// 用 `Arc<Mutex<_>>` 而不是 `Vec`：上层普遍持有 `Arc<dyn FileSystem>`，
    /// 测试手里那份必须和它们记录的是同一份日志。
    writes: Arc<Mutex<Vec<MockWrite>>>,
}

impl MockFileSystem {
    pub fn new(entries_per_dir: usize) -> Self {
        Self {
            entries_per_dir,
            writes: Arc::default(),
        }
    }

    /// 已记录的写操作，按发生顺序。
    pub fn writes(&self) -> Vec<MockWrite> {
        self.writes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn record(&self, write: MockWrite) {
        // 锁中毒说明另一个测试线程在持锁时 panic 了。对一段「只记日志」的代码
        // 而言继续用那份数据是安全的，因此不做 `unwrap` 二次崩溃——
        // 那会把「某个断言失败」变成「一连串测试都挂掉」，很难查。
        self.writes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(write);
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
                hidden: false,
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
            hidden: false,
        })
    }

    fn recycle_bin(&self) -> anyhow::Result<Vec<Entry>> {
        // 桩实现没有回收站：返回空表，上层会走「回收站为空」的空态。
        Ok(Vec::new())
    }

    fn delete_permanently(&self, paths: &[PathBuf]) -> anyhow::Result<OpReport> {
        // 桩实现不碰磁盘：按「全部成功」回执，供跨平台单测验证调用链。
        self.record(MockWrite::DeletePermanently {
            paths: paths.to_vec(),
        });
        Ok(OpReport {
            done: paths.len(),
            ..OpReport::default()
        })
    }

    fn shell_open(&self, path: &std::path::Path) -> anyhow::Result<()> {
        self.record(MockWrite::ShellOpen {
            path: path.to_path_buf(),
        });
        Ok(())
    }

    // ── 写操作 ──────────────────────────────────────────────────────
    //
    // 全部只记日志、不动磁盘，并按「成功」回执。桩不模拟冲突、权限与跨卷，
    // 那些行为由真实实现负责（也只有真实实现能负责）；
    // 桩的职责是让上层的**编排**（谁调用了谁、调用了几次）可断言。

    fn create_dir(&self, parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
        self.record(MockWrite::CreateDir {
            parent: parent.to_path_buf(),
            name: name.to_string(),
        });
        Ok(parent.join(name))
    }

    fn create_file(&self, parent: &Path, name: &str) -> anyhow::Result<PathBuf> {
        self.record(MockWrite::CreateFile {
            parent: parent.to_path_buf(),
            name: name.to_string(),
        });
        Ok(parent.join(name))
    }

    fn rename(&self, path: &Path, new_name: &str) -> anyhow::Result<PathBuf> {
        self.record(MockWrite::Rename {
            path: path.to_path_buf(),
            new_name: new_name.to_string(),
        });
        Ok(path.with_file_name(new_name))
    }

    fn copy_into(&self, sources: &[PathBuf], dest_dir: &Path) -> anyhow::Result<OpReport> {
        self.record(MockWrite::Copy {
            sources: sources.to_vec(),
            dest_dir: dest_dir.to_path_buf(),
        });
        Ok(OpReport {
            done: sources.len(),
            ..OpReport::default()
        })
    }

    fn move_into(&self, sources: &[PathBuf], dest_dir: &Path) -> anyhow::Result<OpReport> {
        self.record(MockWrite::Move {
            sources: sources.to_vec(),
            dest_dir: dest_dir.to_path_buf(),
        });
        Ok(OpReport {
            done: sources.len(),
            ..OpReport::default()
        })
    }

    fn move_to_recycle_bin(&self, paths: &[PathBuf]) -> anyhow::Result<usize> {
        self.record(MockWrite::Recycle {
            paths: paths.to_vec(),
        });
        Ok(paths.len())
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
