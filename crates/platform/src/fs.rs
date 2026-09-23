//! 文件系统能力的平台无关契约。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fm_core::Entry;

/// 驱动器类型。
///
/// 刻意不把「本地磁盘」这类展示文案放进平台层：平台层只回答「是什么」，
/// 文案由 UI 层组装，将来做多语言时不需要动平台代码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    /// 本地固定磁盘。
    Fixed,
    /// 可移动磁盘（U 盘、读卡器）。
    Removable,
    /// 网络驱动器。
    Network,
    /// 光驱。
    Optical,
    /// 其它（RAM 盘、未知类型）。
    Other,
}

/// 驱动器信息，供左侧导航栏的驱动器分区使用。
#[derive(Clone, Debug)]
pub struct Drive {
    /// 盘符形式的名称，例如 `C:`。
    pub name: Arc<str>,
    /// 挂载点，例如 `C:\`。
    pub mount_point: PathBuf,
    /// 驱动器类型。
    pub kind: DriveKind,
    /// 总容量（字节）。查询不到时为 0。
    pub total_bytes: u64,
    /// 剩余容量（字节）。查询不到时为 0。
    pub free_bytes: u64,
}

impl Drive {
    /// 已用比例，范围为 `0.0..=1.0`。
    ///
    /// 独立成方法而不是在渲染时现算，是因为容量为 0 的设备（未插入介质的读卡器、
    /// 空光驱）会让除零结果变成 NaN，直接把进度条画崩。
    pub fn used_ratio(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        let used = self.total_bytes.saturating_sub(self.free_bytes);
        (used as f64 / self.total_bytes as f64) as f32
    }
}

/// 平台文件系统能力。
///
/// 关键契约：`scan` **必须分批回传**结果，而不是读完整个目录再返回。
/// 这样调用方才有机会在批次之间插入取消判断（用户已经切走了目录），
/// 避免为一个已经没人看的目录继续烧 IO。
pub trait FileSystem: Send + Sync + 'static {
    /// 扫描目录。
    ///
    /// `sink` 每收到一批条目被调用一次，**返回 `false` 表示调用方已取消**
    /// （典型场景：用户已经切到别的目录，旧扫描应立即中止，不再浪费 IO）。
    /// 实现必须在收到 `false` 后尽快返回，且不得再调用 `sink`。
    fn scan(
        &self,
        dir: &Path,
        batch: usize,
        sink: &mut dyn FnMut(Vec<Entry>) -> bool,
    ) -> anyhow::Result<()>;

    /// 枚举所有驱动器及其容量。
    ///
    /// 注意：光驱与网络盘的容量查询可能阻塞数秒，**必须在后台线程调用**。
    fn drives(&self) -> anyhow::Result<Vec<Drive>>;

    /// 枚举「快速访问」条目：桌面、下载、文档、图片、音乐、视频。
    fn quick_access(&self) -> anyhow::Result<Vec<Entry>>;

    /// 读取单个路径的元信息，用于状态栏与后续的预览面板。
    fn stat(&self, path: &Path) -> anyhow::Result<Entry>;
}
