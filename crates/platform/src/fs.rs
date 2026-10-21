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

    /// 枚举回收站条目（**只读**）。
    ///
    /// 条目里的 `modified` 是**删除时间**（原修改时间没有存在 `$I` 里），
    /// `path` 指向回收站内的实际存放位置（`$R` 项）。
    /// 还原与清空仍未实现——它们需要另一套接口。
    ///
    /// 单个卷读不到、单个条目损坏都只跳过，不返回错误：
    /// 回收站里混着损坏项是常态，不该让整个视图报错。
    fn recycle_bin(&self) -> anyhow::Result<Vec<Entry>>;

    // ── 以下为写操作 ────────────────────────────────────────────────
    //
    // 与上面读操作的两点约定差异：
    //
    // 1. 写操作**会**修改磁盘，因此错误必须逐条如实回传（见 [`OpReport`]），
    //    而不是「一失败就整体放弃」：批量里一个文件被占用，
    //    不该让另外九个白做。
    // 2. 名称合法性由 `fm_core::naming` 判定，本层**不重复校验**——
    //    只负责「重名怎么办」这一件需要看文件系统才能决定的事。

    /// 在 `parent` 下建一个目录，返回它**实际落在**的路径。
    ///
    /// 重名时**自动改名**（`新建文件夹 (2)`）而不是报错，与资源管理器一致：
    /// 「新建文件夹」是一个人一天会按十次的动作，让它因重名失败毫无道理。
    /// 传入的名称必须是 [`fm_core::naming::validate`] 通过的名称。
    fn create_dir(&self, parent: &Path, name: &str) -> anyhow::Result<PathBuf>;

    /// 在 `parent` 下建一个空文件，返回它**实际落在**的路径。重名规则同 [`FileSystem::create_dir`]。
    fn create_file(&self, parent: &Path, name: &str) -> anyhow::Result<PathBuf>;

    /// 改名（所在目录不变），返回新路径。
    ///
    /// 与「新建」相反，这里的名称是用户**明确给定**的，因此重名**报错**而不是
    /// 悄悄加序号——用户输入 `报告.docx` 却得到 `报告 (2).docx` 是更坏的结果。
    ///
    /// 例外：仅大小写不同的改名（`报告.docx` → `报告.DOCX`）必须放行，
    /// 那是修正命名而不是制造冲突（同目录下 Windows 本身也不区分大小写）。
    fn rename(&self, path: &Path, new_name: &str) -> anyhow::Result<PathBuf>;

    /// 把若干条目复制进 `dest_dir`，逐条回传结果。重名时自动改名。
    ///
    /// 目录按递归复制；符号链接与目录联接**跳过并记为失败**，
    /// 因为跟随它们可能把整棵树复抄一遍甚至无限递归（见平台实现的说明）。
    fn copy_into(&self, sources: &[PathBuf], dest_dir: &Path) -> anyhow::Result<OpReport>;

    /// 把若干条目移动进 `dest_dir`，逐条回传结果。重名时自动改名。
    ///
    /// 跨卷移动会自动退化为「复制 + 删源」——`rename` 无法跨卷，
    /// 而用户不该关心两个盘是不是同一个卷。
    fn move_into(&self, sources: &[PathBuf], dest_dir: &Path) -> anyhow::Result<OpReport>;

    /// 把若干条目移入回收站，返回实际移入的数量。
    ///
    /// **失败即报错**（不像复制那样逐条吞掉）：回收站是「我以为删掉了」的最后一道
    /// 心理防线，部分失败必须让用户看见，而不是弹一句「已删除 3 项」了事。
    fn move_to_recycle_bin(&self, paths: &[PathBuf]) -> anyhow::Result<usize>;

    /// **永久删除**若干条目，逐条回执。
    ///
    /// 与 [`FileSystem::move_to_recycle_bin`] 的差别只有一条：**不能反悔**。
    /// 因此调用方必须为它准备单独的确认文案（「此操作无法撤销」），
    /// 并把它绑定到与回收站删除不同的入口（`Shift+Delete`）。
    ///
    /// 只读属性会被清掉后重试：确认框已经问过一次，删除时再失败一次
    /// 等于让用户在两个地方回答同一个问题。
    fn delete_permanently(&self, paths: &[PathBuf]) -> anyhow::Result<OpReport>;

    /// 带进度与取消的粘贴。
    ///
    /// 默认实现退化成 [`FileSystem::copy_into`] / [`FileSystem::move_into`]：
    /// 进度与取消只是**增强**，mock 与将来的其他平台不必为此各写一份。
    /// 真实实现（Windows）会覆盖它，在文件边界检查取消标志并回调进度。
    fn transfer(
        &self,
        sources: &[PathBuf],
        dest_dir: &Path,
        mode: TransferMode,
        _cancel: &CancelToken,
        _progress: &mut dyn FnMut(TransferProgress),
    ) -> anyhow::Result<OpReport> {
        match mode {
            TransferMode::Copy => self.copy_into(sources, dest_dir),
            TransferMode::Move => self.move_into(sources, dest_dir),
        }
    }

    /// 把路径写入**系统**剪贴板，供资源管理器等其它程序粘贴。
    ///
    /// 默认实现什么也不做：应用内剪贴板（`services::clipboard`）才是主路径，
    /// 系统剪贴板只是互通增强，mock 与其它平台不必实现。
    fn write_file_clipboard(&self, _paths: &[PathBuf], _cut: bool) -> anyhow::Result<()> {
        Ok(())
    }

    /// 读取系统剪贴板里的文件列表：`(路径, 是否剪切)`。
    ///
    /// `Ok(None)` 表示没有文件（空剪贴板、文本、或不支持的格式）。
    /// 默认实现永远返回 `None`。
    fn read_file_clipboard(&self) -> anyhow::Result<Option<(Vec<PathBuf>, bool)>> {
        Ok(None)
    }
}

/// 一次写操作的回执。
///
/// 刻意不是 `Result<()>`：批量操作里「部分成功」是常态，而 UI 需要能说出
/// 「已复制 3 项，2 项因重名改名，1 项失败（文件被占用）」这种话。
/// 把失败压成一个 `Err` 会让另外九条成功的结果无处安放。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpReport {
    /// 成功处理的条目数。
    pub done: usize,
    /// 因重名而被改名的条目：`(原目标路径, 实际路径)`。
    ///
    /// 保留下来是因为 UI 要能指出「它现在叫什么」——用户看到的列表里
    /// 会多出一个 `报告 (2).docx`，不说清楚会被当成操作出错。
    pub renamed: Vec<(PathBuf, PathBuf)>,
    /// 失败的条目及原因（面向用户的短句，不是 `Os { code: .. }`）。
    pub failed: Vec<(PathBuf, String)>,
    /// 是否因为用户取消而提前结束。
    ///
    /// 与「失败」分开记：取消不是错误，回执里要说「已取消」而不是
    /// 「1 项失败（已取消）」——后者会让用户以为出了问题。
    pub cancelled: bool,
}

impl OpReport {
    /// 是否有条目失败。
    pub fn has_failures(&self) -> bool {
        !self.failed.is_empty()
    }

    /// 是否一个条目都没动（用户点了粘贴但源全都不在之类）。
    pub fn is_empty(&self) -> bool {
        self.done == 0 && self.failed.is_empty()
    }
}

/// 传输方向。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferMode {
    Copy,
    Move,
}

impl TransferMode {
    /// 面向用户的动词（「复制」/「移动」）。
    pub fn verb(self) -> &'static str {
        match self {
            Self::Copy => "复制",
            Self::Move => "移动",
        }
    }
}

/// 取消标志。
///
/// 用 `Arc<AtomicBool>` 而不是 channel：取消是**单向、幂等、随时可读**的信号，
/// 原子读就够；channel 还要在复制的每一层递归里处理接收端，反而更绕。
/// 克隆出来的句柄共享同一个标志，因此 UI 点「取消」能立刻影响后台线程。
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消。重复调用无副作用。
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// 是否已请求取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// 一次传输的进度快照。
#[derive(Clone, Debug, Default)]
pub struct TransferProgress {
    /// 已完成的**文件**数（目录本身不计）。
    pub files_done: u64,
    /// 正在处理的路径。
    pub current: PathBuf,
}
