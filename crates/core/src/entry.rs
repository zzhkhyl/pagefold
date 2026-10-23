//! 条目模型：文件系统条目的平台无关表示。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// 扩展名句柄。
///
/// 由 `fm-services` 的扩展名驻留表分配，同一扩展名在整个进程内共用同一个 id。
/// 做成小整数 id 而非字符串的目的是：`Entry` 可以保持 `Copy` 语义的轻量比较，
/// 且排序路径上比较扩展名退化为整数比较，不需要重复做小写化与字符串分配。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExtId(pub u16);

/// 图标句柄。由 `fm-services` 的图标缓存按扩展名分配，配合 LRU 上限使用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IconId(pub u32);

/// 条目类型。
///
/// 注意：`File` 携带扩展名句柄而非 `String`，是为了让整个枚举保持 `Copy`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Directory,
    File { ext: Option<ExtId> },
    Drive,
    Symlink,
    /// 网络上的**共享打印机**（`\\主机\打印机名`）。
    ///
    /// 它既不是目录也不是普通文件：不能进入、不能复制，双击交给系统打开打印队列。
    /// 单独立一个分支，是为了让「双击 / 右键菜单 / 图标」都能按类型区别对待，
    /// 而不是靠在文件扩展名里塞一个魔法值。
    Printer,
}

/// 单个文件系统条目。
///
/// `name` 与 `path` 用 `Arc` 包裹，使得把条目克隆进 GPUI 的视图模型时只增加引用计数，
/// 不会复制路径字符串——目录内数万条目滚动时这点很关键。
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: Arc<str>,
    pub path: Arc<PathBuf>,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub icon: IconId,
    /// 是否带「隐藏」或「系统」属性。
    ///
    /// 两类合并成一个字段，而不是 `hidden` + `system` 两个布尔值：资源管理器的
    /// 「隐藏的项目」把这两类一起管，用户也从来没有「只看系统文件」的需求。
    /// 过滤发生在**载入列表时**（见 `ShellView`），因此下标与选中集始终一致。
    pub hidden: bool,
}

impl Entry {
    /// 是否为目录（决定双击行为与图标选择）。
    pub fn is_dir(&self) -> bool {
        matches!(self.kind, EntryKind::Directory)
    }

    /// 文件的扩展名句柄；目录与驱动器返回 `None`。
    pub fn ext(&self) -> Option<ExtId> {
        match self.kind {
            EntryKind::File { ext } => ext,
            _ => None,
        }
    }
}

/// 内容区视图模式。本次交付图标与详情两态，列表态预留。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    Icons,
    Details,
}

impl ViewMode {
    /// 供设置页与命令条提示使用的文案。
    pub fn label(self) -> &'static str {
        match self {
            Self::Icons => "图标",
            Self::Details => "详细信息",
        }
    }
}

/// 详情视图的排序键。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Kind,
    Modified,
}

/// 排序方向。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

impl SortDirection {
    /// 翻转方向，供表头点击使用。
    pub fn flipped(self) -> Self {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }
}
