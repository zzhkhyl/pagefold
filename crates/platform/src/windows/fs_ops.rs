//! Windows 上的写操作：新建、改名、复制、移动。
//!
//! ## 为什么用 `std::fs` 而不是 `SHFileOperation` / `IFileOperation`
//!
//! 后者能免费拿到系统进度条与冲突对话框，听着很划算，代价却是**失去控制**：
//! `SHFileOperation` 只回一个错误码，而 UI 需要的是逐条回执（见 [`OpReport`]）——
//! 「已复制 3 项，1 项因占用失败」这句话它说不出来。
//! 真正非 shell 不可的只有一件事：**移入回收站**（见 [`super::recycle_bin`]）。
//! 因此这里的分工是「std 负责可控的部分，shell 只负责它独有的能力」。
//!
//! ## 长路径
//!
//! Rust 的 `std::fs` **不会**给路径加 `\\?\` 前缀，因此超过 260 字符的路径会直接
//! 失败。对文件管理器来说这不是边角情况（`node_modules` 里三层就两百多字符），
//! 所以本模块的每一次 IO 都经 [`extended`] 转换。

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use fm_core::naming::unique_name;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

use crate::fs::{CancelToken, OpReport, TransferMode, TransferProgress};

/// 在 `parent` 下新建目录，重名时自动改名。
pub fn create_dir(parent: &Path, name: &str) -> Result<PathBuf> {
    create_unique(parent, name, "文件夹", |path| fs::create_dir(path))
}

/// 在 `parent` 下新建空文件，重名时自动改名。
pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf> {
    create_unique(parent, name, "文件", |path| {
        fs::File::create(path).map(|_| ())
    })
}

/// 改名。仅大小写不同的改名放行，其余重名报错。
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .with_context(|| format!("「{}」没有父目录，无法改名", path.display()))?;
    let target = parent.join(new_name);

    // 这一句是**必需**的，不是保险：Rust 的 `fs::rename` 在 Windows 上走
    // `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，也就是说**默认会静默覆盖**同名目标。
    // 不查一下，用户把 `新建文件夹` 改名为一个已存在的名字时会直接删掉别人的东西。
    //
    // 例外是仅大小写不同的改名（`报告.docx` → `报告.DOCX`）：那是修正命名，
    // 而且在同目录下 Windows 本身就不区分大小写，必须放行。
    let case_only = path
        .file_name()
        .is_some_and(|old| OsStr::new(new_name).eq_ignore_ascii_case(old));
    if !case_only && extended(&target).exists() {
        anyhow::bail!("「{new_name}」已存在");
    }

    fs::rename(extended(path), extended(&target))
        .with_context(|| format!("无法重命名为「{new_name}」"))?;
    Ok(target)
}

/// 复制若干条目进 `dest_dir`。
pub fn copy_into(sources: &[PathBuf], dest_dir: &Path) -> Result<OpReport> {
    Ok(transfer_with(
        sources,
        dest_dir,
        TransferMode::Copy,
        &CancelToken::new(),
        &mut |_| {},
    ))
}

/// 移动若干条目进 `dest_dir`。
pub fn move_into(sources: &[PathBuf], dest_dir: &Path) -> Result<OpReport> {
    Ok(transfer_with(
        sources,
        dest_dir,
        TransferMode::Move,
        &CancelToken::new(),
        &mut |_| {},
    ))
}

/// 带进度与取消的复制 / 移动。
///
/// **取消发生在文件边界**：正在写的那一个文件会写完，之后不再开始下一个。
/// 这是刻意的——`fs::copy` 是单个阻塞调用，没有「写到一半停下」的干净办法，
/// 而半截文件比「多复制了一个」危险得多（用户会以为那个文件是完整的）。
/// 资源管理器的取消行为与此一致。
pub fn transfer_with(
    sources: &[PathBuf],
    dest_dir: &Path,
    mode: TransferMode,
    cancel: &CancelToken,
    progress: &mut dyn FnMut(TransferProgress),
) -> OpReport {
    let mut report = OpReport::default();
    let mut run = TransferRun {
        cancel,
        progress,
        files_done: 0,
        bytes_done: 0,
        bytes_total: known_bytes(sources),
        cancelled: false,
    };

    for source in sources {
        if run.cancelled {
            break;
        }
        let outcome = match mode {
            TransferMode::Copy => copy_one(source, dest_dir, &mut run),
            TransferMode::Move => move_one(source, dest_dir, &mut run),
        };
        match outcome {
            Ok((target, renamed)) => {
                report.done += 1;
                if renamed {
                    report.renamed.push((source.clone(), target));
                }
            }
            Err(error) => {
                // 取消时递归里抛出的那个错误不算「失败」：用户主动喊停，
                // 不是程序出错，回执要分开说。
                if run.cancelled {
                    break;
                }
                report.failed.push((source.clone(), describe(&error)));
            }
        }
    }

    report.cancelled = run.cancelled;
    report
}

/// 一次传输的运行态：取消标志、进度回调与已完成的文件数 / 字节数。
struct TransferRun<'a> {
    cancel: &'a CancelToken,
    progress: &'a mut dyn FnMut(TransferProgress),
    files_done: u64,
    bytes_done: u64,
    bytes_total: u64,
    cancelled: bool,
}

impl TransferRun<'_> {
    /// 在每个文件真正开始复制之前调用：检查取消、把「当前文件」报到界面上。
    ///
    /// **不累加文件数**：`files_done` 的语义是「已完成」，在开始写之前就 +1
    /// 会让进度卡显示「已处理 1 个文件」而实际一个都还没写完。
    fn tick(&mut self, path: &Path) -> Result<()> {
        if self.cancel.is_cancelled() {
            self.cancelled = true;
            // 错误内容不重要：调用方看到 `cancelled` 就把它当取消处理。
            anyhow::bail!("已取消");
        }
        self.report(path);
        Ok(())
    }

    /// 一个文件写完了：累加已完成数并回调。
    ///
    /// 这里**不**再检查取消：文件已经完整落盘，把它记进「已完成」才是事实。
    /// 取消的判定留给下一个文件的 `tick`（以及块级 `advance`），
    /// 否则会出现「文件复制好了却被算作失败」的错账。
    fn finish(&mut self, path: &Path) {
        self.files_done += 1;
        self.report(path);
    }

    /// 一块数据写完：累计字节、检查取消、回调进度。
    ///
    /// 取消检查放在**每块之后**而不是每文件之前，这样几 GB 的单个大文件
    /// 也能在写下一块时停下——已经写完的那部分是一个完整的前缀，
    /// 不会留下半截的数据块。
    fn advance(&mut self, path: &Path, bytes: u64) -> Result<()> {
        self.bytes_done += bytes;
        self.report(path);
        if self.cancel.is_cancelled() {
            self.cancelled = true;
            anyhow::bail!("已取消");
        }
        Ok(())
    }

    fn report(&mut self, path: &Path) {
        (self.progress)(TransferProgress {
            files_done: self.files_done,
            current: path.to_path_buf(),
            bytes_done: self.bytes_done,
            bytes_total: self.bytes_total,
        });
    }
}

/// 能在开始前廉价算出的总字节数。
///
/// 只统计**顶层是文件**的源：目录的内容大小要遍历才知道，而那等于先把整棵树
/// 走一遍——对一个几十万文件的目录，这会让「点击粘贴」多等好几秒，
/// 换来的只是一个百分比。目录参与时返回 `0`（界面据此显示「未知」）。
fn known_bytes(sources: &[PathBuf]) -> u64 {
    let mut total = 0u64;
    for source in sources {
        let Ok(metadata) = fs::symlink_metadata(extended(source)) else {
            // 元数据读不到就不计入：这个源随后会以「失败」出现在回执里。
            continue;
        };
        if metadata.is_dir() {
            return 0;
        }
        total = total.saturating_add(metadata.len());
    }
    total
}

/// 永久删除若干条目（不进回收站），逐条回执。
///
/// 逐条而不是「一失败就整体放弃」：批量删除里有一个文件被占用，
/// 不该让另外九个白做——更重要的是，那样用户无法知道到底删掉了什么。
pub fn delete_permanently(paths: &[PathBuf]) -> OpReport {
    let mut report = OpReport::default();

    for path in paths {
        match delete_one(path) {
            Ok(()) => report.done += 1,
            Err(error) => report.failed.push((path.clone(), describe(&error))),
        }
    }

    report
}

/// 删除单个条目。只读属性导致的失败会被清属性后重试一次。
fn delete_one(path: &Path) -> Result<()> {
    let target = extended(path);
    let metadata = fs::symlink_metadata(&target)?;
    let is_dir = metadata.is_dir();

    let remove = || {
        if is_dir {
            fs::remove_dir_all(&target)
        } else {
            fs::remove_file(&target)
        }
    };

    match remove() {
        Ok(()) => Ok(()),
        Err(_) => {
            // 只读文件、以及「目录里有只读文件」都会走到这里。
            // 确认框已经问过一次了，这里不再把决定权交回用户。
            clear_readonly(&target, is_dir)?;
            remove().with_context(|| format!("无法删除「{}」", path.display()))?;
            Ok(())
        }
    }
}

/// 递归清掉只读属性。
///
/// 只处理 `FILE_ATTRIBUTE_READONLY`（Rust 的 `Permissions::set_readonly` 映射的
/// 就是这一位）；隐藏/系统属性不动——那是用户刻意设置的标记，与「能不能删」无关。
///
/// `set_readonly(false)` 的 clippy 警告针对的是 Unix 语义（会变成全局可写），
/// 而本模块只在 Windows 下编译：那里的「非只读」就是普通文件，不存在这个风险。
#[allow(clippy::permissions_set_readonly_false)]
fn clear_readonly(path: &Path, is_dir: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)
            .with_context(|| format!("无法清除只读属性：{}", path.display()))?;
    }

    if is_dir {
        for entry in fs::read_dir(path)? {
            let Ok(entry) = entry else {
                continue;
            };
            // 出错就跳过这一项：清属性是「尽力而为」，真正的判据是随后那次删除
            // 是否成功——它会把失败原因如实带回给用户。
            let nested_is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            let _ = clear_readonly(&entry.path(), nested_is_dir);
        }
    }
    Ok(())
}

/// 新建条目并按需改名，返回最终路径。
fn create_unique(
    parent: &Path,
    desired: &str,
    kind: &str,
    create: impl Fn(&Path) -> io::Result<()>,
) -> Result<PathBuf> {
    let parent_ext = extended(parent);
    // 用「目标在不在」判断占用，而不是先列目录再比对：Windows 的文件系统本身
    // 不区分大小写，`exists()` 就是权威答案，而自己比对还得处理大小写与编码。
    let name = unique_name(desired, |candidate| parent_ext.join(candidate).exists());
    let path = parent.join(&name);

    create(&extended(&path)).with_context(|| format!("无法创建{kind}「{name}」"))?;
    Ok(path)
}

/// 复制单个条目，返回 `(实际目标路径, 是否因重名改过名)`。
fn copy_one(
    source: &Path,
    dest_dir: &Path,
    run: &mut TransferRun<'_>,
) -> Result<(PathBuf, bool)> {
    let (target, renamed) = resolve_target(dest_dir, source)?;
    copy_tree(source, &target, run)?;
    Ok((target, renamed))
}

/// 移动单个条目。
fn move_one(
    source: &Path,
    dest_dir: &Path,
    run: &mut TransferRun<'_>,
) -> Result<(PathBuf, bool)> {
    // 把目录移进它自己（或它的子目录）会自我吞食：`copy` 会一边写一边读、
    // 直到磁盘写满；`move` 更糟，删源时会把刚移进去的部分一起删掉。
    // 资源管理器同样拒绝这种操作。
    if source.is_dir() && dest_dir.starts_with(source) {
        anyhow::bail!("不能把文件夹移动到它自己里面");
    }

    let (target, renamed) = resolve_target(dest_dir, source)?;
    move_tree(source, &target, run)?;
    Ok((target, renamed))
}

/// 算出条目复制/移动到 `dest_dir` 后的路径，重名时自动改名。
fn resolve_target(dest_dir: &Path, source: &Path) -> Result<(PathBuf, bool)> {
    let name = source
        .file_name()
        .with_context(|| format!("无法确定「{}」的名称", source.display()))?
        .to_string_lossy()
        .into_owned();

    let dest_ext = extended(dest_dir);
    let resolved = unique_name(&name, |candidate| dest_ext.join(candidate).exists());
    let renamed = resolved != name;

    Ok((dest_dir.join(&resolved), renamed))
}

/// 递归复制。目录逐层新建，文件用 `fs::copy`（它会带上修改时间）。
fn copy_tree(from: &Path, to: &Path, run: &mut TransferRun<'_>) -> Result<()> {
    let metadata = fs::symlink_metadata(extended(from))
        .with_context(|| format!("无法读取「{}」", from.display()))?;

    // 符号链接与目录联接一律**拒绝**，而不是跟随。
    //
    // 跟随它们看起来更「完整」，但有两个真实后果：把同一棵树复抄多遍；
    // 遇到指向上级目录的联接（`node_modules` 与某些 junction 的常见形态）
    // 直接无限递归直到磁盘写满。资源管理器敢跟随是因为它有环检测与
    // 用户可选的冲突处理，这里没有，所以宁可明确报错。
    if metadata.is_symlink() {
        anyhow::bail!("不支持复制符号链接或目录联接");
    }

    if metadata.is_dir() {
        fs::create_dir(extended(to)).with_context(|| format!("无法创建目录「{}」", to.display()))?;
        let items = fs::read_dir(extended(from))
            .with_context(|| format!("无法读取目录「{}」", from.display()))?;
        for item in items {
            // 单个条目失败会让整项失败，不做「跳过继续」：复制留下的半成品
            // 比一次明确的失败更难收拾，而回执里会写明是哪一个条目出的问题。
            let item = item.with_context(|| format!("无法读取目录「{}」", from.display()))?;
            copy_tree(&item.path(), &to.join(item.file_name()), run)?;
        }
        return Ok(());
    }

    // 在真正写文件之前检查取消并回调进度：这样「已取消」时不会多写一个文件。
    run.tick(from)?;
    copy_file(from, to, run)?;
    run.finish(from);
    Ok(())
}

/// 每块复制多少字节。
///
/// 1MB 是「回调次数」与「单次 IO 开销」的折中：复制 5GB 会产生约 5000 次回调，
/// 而界面每 120ms 才取一次进度，足够平滑；块再小收益就只剩回调开销了。
const COPY_CHUNK_BYTES: usize = 1024 * 1024;

/// 分块复制单个文件：边写边回报字节进度，并保留修改时间。
///
/// 相比 `fs::copy` 只有一处不同：能报进度、能在文件**内部**取消。
/// 代价是取消时必须把写了一半的目标删掉——半截文件比「少复制一个」危险得多。
fn copy_file(from: &Path, to: &Path, run: &mut TransferRun<'_>) -> Result<()> {
    use std::io::{Read as _, Write as _};

    let mut reader = fs::File::open(extended(from))
        .with_context(|| format!("无法打开「{}」", from.display()))?;
    let mut writer = fs::File::create(extended(to))
        .with_context(|| format!("无法创建「{}」", to.display()))?;
    let mut buffer = vec![0u8; COPY_CHUNK_BYTES];

    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("读取「{}」失败", from.display()))?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .with_context(|| format!("写入「{}」失败", to.display()))?;

        if let Err(error) = run.advance(from, read as u64) {
            // 取消（或后续写失败）：这一块已经落盘，但整个文件不完整。
            // 直接删掉目标，而不是留给用户一个看起来正常、实际截断的文件。
            drop(writer);
            let _ = fs::remove_file(extended(to));
            return Err(error);
        }
    }

    writer
        .flush()
        .with_context(|| format!("写入「{}」失败", to.display()))?;
    drop(writer);

    copy_mtime(from, to);
    Ok(())
}

/// 把源文件的修改时间写到目标上。
///
/// `std::fs::copy` 自带这一步；改成自己分块写之后就得显式补。
/// 少了它，复制出来的文件「修改时间」全部变成现在——按日期排序时整批跑到最前面，
/// 用户会以为文件被改过。
///
/// 失败**不算错**：时间戳是尽力而为，内容已经写对了。
fn copy_mtime(from: &Path, to: &Path) {
    use std::os::windows::io::AsRawHandle as _;

    use windows::Win32::Foundation::{FILETIME, HANDLE};
    use windows::Win32::Storage::FileSystem::{GetFileTime, SetFileTime};

    let Ok(source) = fs::File::open(extended(from)) else {
        return;
    };
    let Ok(target) = fs::OpenOptions::new().write(true).open(extended(to)) else {
        return;
    };

    unsafe {
        let mut created = FILETIME::default();
        let mut accessed = FILETIME::default();
        let mut written = FILETIME::default();
        let read = GetFileTime(
            HANDLE(source.as_raw_handle()),
            Some(&mut created),
            Some(&mut accessed),
            Some(&mut written),
        );
        if read.is_err() {
            return;
        }
        // 只补「修改时间」：创建时间与访问时间在现代 Windows 上受策略影响，
        // 照抄反而会覆盖系统刚写下的合理值。
        let _ = SetFileTime(
            HANDLE(target.as_raw_handle()),
            None,
            None,
            Some(&written as *const FILETIME),
        );
    }
}

/// 移动。同卷用 `fs::rename`（瞬间完成、不产生副本），跨卷退化为复制 + 删源。
fn move_tree(from: &Path, to: &Path, run: &mut TransferRun<'_>) -> Result<()> {
    match fs::rename(extended(from), extended(to)) {
        Ok(()) => {
            // 同卷改名是瞬间的，但进度面板仍需前进一格，否则「移动大目录」
            // 会停在 0 直到全部做完。改名即完成，因此直接记一次 finished。
            run.tick(from)?;
            run.finish(from);
            Ok(())
        }
        Err(error) if is_cross_device(&error) => {
            // `rename` 不能跨卷。先复制再删源，中途失败会留下一份副本——
            // 这是所有文件管理器都接受的风险方向：宁可多留一份，也不能丢数据。
            copy_tree(from, to, run)?;
            remove_tree(from)?;
            Ok(())
        }
        Err(error) => {
            Err(error).with_context(|| format!("无法移动「{}」", from.display()))
        }
    }
}

/// 用系统默认程序打开一个文件（`ShellExecuteW("open")`）。
///
/// 两条与路径有关的注意事项：
///
/// - **不能加 `\\?\` 前缀**：长路径前缀会让 shell 的关联查找失效（它按扩展名
///   去注册表找 handler，而带前缀的路径不被当成普通文件），因此这里用原样路径。
///   代价是超过 260 字符的路径打不开——那类路径也没有多少程序能正确处理。
/// - 打开的是**文件**；目录请走导航，不要交给 shell（否则会开一个新的资源管理器窗口）。
pub fn shell_open(path: &Path) -> Result<()> {
    let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
    let operation: Vec<u16> = "open".encode_utf16().chain([0]).collect();

    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };

    // 返回值是「伪句柄」：**大于 32** 才算成功，0..=32 是错误码。
    let code = result.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        bail!("{}", describe_shell_code(code))
    }
}

/// 把 `ShellExecuteW` 的错误码翻译成一句人话。
///
/// 这些码是 `shellapi.h` 里的 `SE_ERR_*` 与 0：它们的取值固定，但
/// 文档写得很散。翻译成一句话比抛「错误码 31」有用得多——用户看到
/// 「没有关联的程序」才知道下一步该干什么。
fn describe_shell_code(code: isize) -> String {
    match code {
        0 => String::from("系统内存不足，无法启动程序"),
        2 => String::from("找不到该文件，可能已被移动或删除"),
        3 => String::from("找不到该路径"),
        5 => String::from("拒绝访问"),
        31 => String::from("没有与该类型关联的程序，请先在系统设置里指定默认应用"),
        other => format!("系统拒绝打开（错误码 {other}）"),
    }
}

/// 删除条目（移动的收尾）。文件与目录走不同的系统调用。
fn remove_tree(path: &Path) -> Result<()> {
    let metadata =
        fs::symlink_metadata(extended(path)).with_context(|| format!("无法读取「{}」", path.display()))?;
    if metadata.is_dir() {
        fs::remove_dir_all(extended(path))
            .with_context(|| format!("无法删除源目录「{}」", path.display()))
    } else {
        fs::remove_file(extended(path)).with_context(|| format!("无法删除源文件「{}」", path.display()))
    }
}

/// 该错误是否为「跨卷」。
///
/// Windows 返回 `ERROR_NOT_SAME_DEVICE`(17)。不按 `ErrorKind` 判断是因为
/// `ErrorKind::CrossesDevices` 在 Windows 上尚未稳定填写，而原始错误码是稳定的。
fn is_cross_device(error: &io::Error) -> bool {
    const ERROR_NOT_SAME_DEVICE: i32 = 17;
    error.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE)
}

/// 给绝对路径加上 `\\?\` 前缀以突破 260 字符限制。
///
/// 三条必须遵守的规则，违反任何一条都会**让原本能用的路径失效**：
/// - 只处理**绝对**路径：`\\?\` 要求全限定路径；
/// - 路径里不能有 `.` / `..` 分量：加前缀后系统不再做规范化，
///   `..` 会被当成一个名字就叫「..」的目录；
/// - UNC 路径必须改写成 `\\?\UNC\server\share`，直接加前缀会得到无效路径。
pub(crate) fn extended(path: &Path) -> PathBuf {
    if !path.is_absolute() || path.components().any(needs_normalization) {
        return path.to_path_buf();
    }

    // 判断前缀可以用有损转换：这几个前缀字符都是 ASCII，而有损替换只会发生在
    // 非法 UTF-16 序列上——那不可能出现在路径开头。
    // 但**构造**结果绝不能走有损转换：Windows 允许文件名里含未配对代理项，
    // 替换成 U+FFFD 会指向另一个文件。因此一律用 `OsString::push` / `from_wide`。
    let text = path.as_os_str().to_string_lossy();
    if text.starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    if text.starts_with(r"\\") {
        // 砍掉开头的两个反斜杠要按 UTF-16 码元来，不能用字符串切片。
        let rest: Vec<u16> = path.as_os_str().encode_wide().skip(2).collect();
        return prefixed(r"\\?\UNC\", &OsString::from_wide(&rest));
    }

    prefixed(r"\\?\", path.as_os_str())
}

/// 给路径加前缀，全程不做编码转换。
fn prefixed(prefix: &str, rest: &OsStr) -> PathBuf {
    let mut out = OsString::from(prefix);
    out.push(rest);
    PathBuf::from(out)
}

/// 该分量是否会让 `\\?\` 路径失去规范化能力。
fn needs_normalization(component: Component<'_>) -> bool {
    matches!(component, Component::CurDir | Component::ParentDir)
}

/// 把 IO / 上下文错误压成一句面向用户的话。
///
/// 回执里的失败原因直接展示给用户（toast），所以**不能**是
/// `Os { code: 5, kind: PermissionDenied, message: "拒绝访问。" }`。
/// 认不出的错误码退化为原始错误文本——那至少保留了可搜索的信息，
/// 比一句「操作失败」有用。
pub fn describe(error: &anyhow::Error) -> String {
    for cause in error.chain() {
        if let Some(io) = cause.downcast_ref::<io::Error>()
            && let Some(message) = describe_code(io.raw_os_error())
        {
            return message.to_string();
        }
    }

    // 走到这里说明是我们自己 `bail!` 出来的中文（如「不支持复制符号链接」），
    // 或者是认不出的错误码。两者都直接用最外层那句话。
    error.to_string()
}

/// 常见 Win32 错误码 → 用户可读的原因。
///
/// 只列真实会遇到的：权限、占用、空间、重名、路径过长。
fn describe_code(code: Option<i32>) -> Option<&'static str> {
    Some(match code? {
        2 => "项目不存在，可能已被移动或删除",
        3 => "路径不存在",
        5 => "没有权限，文件可能被占用或受保护",
        15 => "驱动器无效",
        19 => "目标磁盘被写保护",
        32 => "文件正被其它程序使用",
        33 => "文件的一部分被锁定",
        80 | 183 => "已存在同名项目",
        112 => "磁盘空间不足",
        123 => "名称不合法",
        145 => "目录不为空",
        206 => "路径过长",
        267 => "路径指向的不是目录",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用一个进程内唯一的临时目录跑真实 IO。
    ///
    /// 名字带 pid 与调用方给的标记：并行测试不会互相踩，失败时也能看出是谁留下的。
    fn scratch_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("pagefold-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn transfer_reports_finished_files_monotonically() {
        let root = scratch_dir("transfer-progress");
        let source = scratch_dir("transfer-progress-src");
        fs::write(source.join("a.txt"), b"a").unwrap();
        fs::write(source.join("b.txt"), b"b").unwrap();

        let mut seen: Vec<u64> = Vec::new();
        let report = transfer_with(
            &[source.join("a.txt"), source.join("b.txt")],
            &root,
            TransferMode::Copy,
            &CancelToken::new(),
            &mut |progress| seen.push(progress.files_done),
        );

        assert_eq!(report.done, 2);
        assert!(!report.cancelled);
        // 字节级进度让「每个文件多次回调」成为常态，因此不能断言回调次数，
        // 只能断言**语义**：计数单调不减、两个文件各出现过一次、最后是 2。
        assert!(
            seen.windows(2).all(|pair| pair[0] <= pair[1]),
            "已完成数只能递增：{seen:?}"
        );
        assert!(seen.contains(&1) && seen.contains(&2), "{seen:?}");
        assert_eq!(seen.last(), Some(&2), "最后一个文件完成后计数应为 2");
        assert!(root.join("a.txt").exists() && root.join("b.txt").exists());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn transfer_reports_bytes_inside_a_single_large_file() {
        // 一个 2.5MB 的文件：整块 1MB × 2 + 半块，应当能看到多次字节增量。
        // 这条测试守的是「大文件复制时进度不动」那个问题的回归。
        let root = scratch_dir("transfer-bytes");
        let source = scratch_dir("transfer-bytes-src");
        let size = 5 * 1024 * 1024 / 2; // 2.5MB
        let payload = vec![7u8; size];
        let file = source.join("big.bin");
        fs::write(&file, &payload).unwrap();

        let mut samples: Vec<(u64, u64)> = Vec::new();
        let report = transfer_with(
            std::slice::from_ref(&file),
            &root,
            TransferMode::Copy,
            &CancelToken::new(),
            &mut |progress| samples.push((progress.bytes_done, progress.bytes_total)),
        );

        assert_eq!(report.done, 1);
        assert_eq!(
            samples.first().map(|(_, total)| *total),
            Some(size as u64),
            "总字节数在开始前就该算出来（单文件可以廉价得到）"
        );
        assert!(
            samples.len() >= 3,
            "2.5MB 至少要回调三次才算「有进度」，实际 {} 次",
            samples.len()
        );
        assert!(
            samples.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "字节数只能单调递增：{samples:?}"
        );
        assert_eq!(
            samples.last().map(|(done, _)| *done),
            Some(size as u64),
            "最后一条必须等于文件大小"
        );
        assert_eq!(
            fs::read(root.join("big.bin")).unwrap().len(),
            size,
            "内容必须完整"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn chunked_copy_preserves_the_modified_time() {
        // `fs::copy` 自带这一步；分块复制之后必须自己补，否则复制出来的文件
        // 「修改时间」全变成现在，按日期排序时整批跑到最前面。
        //
        // 做法：让源文件的修改时间比「现在」早 (sleep 期间)，然后比较目标文件
        // 与源文件的时间差。补了 `copy_mtime` 时两者只差毫秒级；
        // 没补时目标会是「现在」，差出整个 sleep 时长，断言就会失败。
        let root = scratch_dir("transfer-mtime");
        let source = scratch_dir("transfer-mtime-src");
        let file = source.join("stamped.txt");
        fs::write(&file, b"content").unwrap();

        let source_time = fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1200));

        let report = transfer_with(
            std::slice::from_ref(&file),
            &root,
            TransferMode::Copy,
            &CancelToken::new(),
            &mut |_| {},
        );
        assert_eq!(report.done, 1);

        let copied_time = fs::metadata(root.join("stamped.txt"))
            .unwrap()
            .modified()
            .unwrap();
        let drift = copied_time
            .duration_since(source_time)
            .map(|gap| gap.as_millis())
            .unwrap_or(u64::MAX as u128);
        assert!(
            drift < 500,
            "修改时间应当被保留（与源相差 {drift}ms；没有 copy_mtime 时会差约 1200ms）"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn transfer_stops_between_files_when_cancelled() {
        let root = scratch_dir("transfer-cancel");
        let source = scratch_dir("transfer-cancel-src");
        fs::write(source.join("a.txt"), b"a").unwrap();
        fs::write(source.join("b.txt"), b"b").unwrap();

        let cancel = CancelToken::new();
        let mut calls = 0;
        let report = transfer_with(
            &[source.join("a.txt"), source.join("b.txt")],
            &root,
            TransferMode::Copy,
            &cancel,
            &mut |progress| {
                calls += 1;
                // 第一个文件**复制完**之后喊停（按已完成的文件数判断，
                // 不能按回调次数：带有字节进度的实现每个文件会回调多次）。
                if progress.files_done == 1 {
                    cancel.cancel();
                }
            },
        );

        assert!(report.cancelled, "取消必须反映在回执里");
        assert_eq!(report.done, 1, "第一个文件已完成，第二个不该开始");
        assert!(report.failed.is_empty(), "取消不是失败，不能记进 failed");
        assert!(root.join("a.txt").exists());
        assert!(
            !root.join("b.txt").exists(),
            "取消之后不应再写新文件（半截文件比少复制一个更危险）"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn shell_error_codes_become_readable_sentences() {
        // 0..=32 是错误码（> 32 才是成功），这几个是最常撞上的：
        // 「找不到文件」「没有关联程序」——用户需要知道自己该做什么。
        assert!(describe_shell_code(2).contains("找不到该文件"));
        assert!(describe_shell_code(3).contains("路径"));
        assert!(describe_shell_code(5).contains("拒绝访问"));
        assert!(describe_shell_code(31).contains("没有与该类型关联的程序"));
        // 其余错误码不能丢：带上原始数字，便于对着文档排查。
        assert!(describe_shell_code(1155).contains("1155"));
    }

    #[test]
    fn permanent_delete_removes_a_tree_with_readonly_files() {
        let root = scratch_dir("perm-delete");
        fs::create_dir_all(root.join("sub")).unwrap();
        let readonly = root.join("sub").join("readonly.txt");
        fs::write(&readonly, b"x").unwrap();
        let mut permissions = fs::metadata(&readonly).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&readonly, permissions).unwrap();

        let report = delete_permanently(std::slice::from_ref(&root));

        assert_eq!(
            report.done, 1,
            "含只读文件的目录也应被删除（会先清属性再重试）：{:?}",
            report.failed
        );
        assert!(!root.exists(), "目录应已被删除");
    }

    #[test]
    fn permanent_delete_reports_a_missing_path_instead_of_failing_the_batch() {
        let root = scratch_dir("perm-delete-missing");
        let real = root.join("real.txt");
        fs::write(&real, b"x").unwrap();
        let missing = root.join("missing.txt");

        let report = delete_permanently(&[real.clone(), missing.clone()]);

        assert_eq!(report.done, 1, "存在的那一条必须被删掉");
        assert_eq!(report.failed.len(), 1, "不存在的那一条要如实报错");
        assert_eq!(report.failed[0].0, missing);
        assert!(!real.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn extended_prefixes_absolute_paths() {
        assert_eq!(extended(Path::new(r"C:\a\b")), PathBuf::from(r"\\?\C:\a\b"));
        // 已经是扩展路径的一律不动，否则会变成 `\\?\UNC\?\C:\a`。
        assert_eq!(
            extended(Path::new(r"\\?\C:\a")),
            PathBuf::from(r"\\?\C:\a")
        );
    }

    #[test]
    fn extended_rewrites_unc_paths() {
        assert_eq!(
            extended(Path::new(r"\\server\share\dir")),
            PathBuf::from(r"\\?\UNC\server\share\dir"),
            "UNC 不能直接加前缀，否则得到一个无效路径"
        );
    }

    #[test]
    fn extended_leaves_relative_and_dotted_paths_alone() {
        // 相对路径不是全限定路径，加前缀后 `\\?\` 的语义不成立。
        assert_eq!(extended(Path::new(r"a\b")), PathBuf::from(r"a\b"));
        // 含 `..` 的分量在扩展路径里不会被规范化，会把「上一级」当成目录名。
        assert_eq!(
            extended(Path::new(r"C:\a\..\b")),
            PathBuf::from(r"C:\a\..\b"),
            "含 .. 的路径宁可保持原样（也许会因为长度失败，但不会指向错的地方）"
        );
    }

    #[test]
    fn describe_maps_common_error_codes() {
        let denied = anyhow::Error::new(io::Error::from_raw_os_error(5));
        assert!(describe(&denied).contains("没有权限"));

        let busy = anyhow::Error::new(io::Error::from_raw_os_error(32));
        assert!(describe(&busy).contains("正被其它程序使用"));

        let full = anyhow::Error::new(io::Error::from_raw_os_error(112));
        assert!(describe(&full).contains("空间不足"));
    }

    #[test]
    fn describe_finds_nested_io_errors() {
        // 真实路径上错误都被 `with_context` 包了一层，映射必须能穿透这层。
        let error = Err::<(), _>(io::Error::from_raw_os_error(183))
            .with_context(|| "无法复制「a.txt」")
            .unwrap_err();
        assert!(describe(&error).contains("已存在同名项目"));
    }

    #[test]
    fn describe_keeps_our_own_chinese_messages() {
        let error = anyhow::anyhow!("不支持复制符号链接或目录联接");
        assert_eq!(describe(&error), "不支持复制符号链接或目录联接");
    }

    #[test]
    fn describe_falls_back_to_the_raw_text() {
        // 认不出的错误码必须保住原始信息：那句英文至少是可搜索的。
        let error = anyhow::Error::new(io::Error::from_raw_os_error(4242));
        assert!(describe(&error).contains("4242"), "实际输出：{}", describe(&error));
    }
}

/// **真实 IO 的测试。**
///
/// 本文件其余测试都是纯函数（长路径前缀、错误码映射），而这一组真的动磁盘。
/// 理由是有些行为**只能靠真实调用验证**：改名到已存在的名字会不会静默覆盖、
/// 重名去重是不是真的加了序号、超过 `MAX_PATH` 的路径到底通不通。
/// 用 mock 验证这些等于自己确认自己。
///
/// 每个测试用自己的临时目录，结束时删掉。删除失败（文件被占用）不影响测试结果。
#[cfg(test)]
mod io_tests {
    use super::*;

    /// 一个独立的临时目录，离开作用域即清理。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            // 进程号 + 纳秒 + 标签：并行跑的测试不能互相踩。
            path.push(format!(
                "fm-fsops-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0)
            ));
            fs::create_dir_all(&path).expect("无法创建临时目录");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn rename_refuses_an_existing_name() {
        let dir = TempDir::new("rename-collision");
        let source = dir.path().join("a.txt");
        let target = dir.path().join("b.txt");
        fs::write(&source, b"A").unwrap();
        fs::write(&target, b"B").unwrap();

        let result = rename(&source, "b.txt");

        assert!(result.is_err(), "改名到已存在的名字必须失败");
        // 这一条才是关键：Rust 的 `fs::rename` 在 Windows 上走
        // `MOVEFILE_REPLACE_EXISTING`，少了存在性检查，上一步会「成功」并把
        // b.txt 静默替换成 A 的内容——那是丢数据，不是改名。
        assert_eq!(fs::read(&target).unwrap(), b"B", "已存在的文件被覆盖了");
        assert_eq!(fs::read(&source).unwrap(), b"A", "源不该被动过");
    }

    #[test]
    fn rename_allows_a_case_only_change() {
        let dir = TempDir::new("rename-case");
        let source = dir.path().join("Report.txt");
        fs::write(&source, b"x").unwrap();

        let renamed = rename(&source, "REPORT.TXT").expect("仅大小写不同的改名应当放行");
        assert_eq!(renamed.file_name().unwrap(), "REPORT.TXT");
        assert_eq!(fs::read(&renamed).unwrap(), b"x", "内容不该丢");
    }

    #[test]
    fn create_dir_appends_a_number_on_collision() {
        let dir = TempDir::new("create-collision");
        fs::create_dir(dir.path().join("新建文件夹")).unwrap();

        let created = create_dir(dir.path(), "新建文件夹").expect("应当自动改名后创建成功");
        assert_eq!(created.file_name().unwrap(), "新建文件夹 (2)");
        assert!(created.is_dir());
    }

    #[test]
    fn create_file_makes_an_empty_file() {
        let dir = TempDir::new("create-file");
        let created = create_file(dir.path(), "新建文本文档.txt").expect("应当创建成功");
        assert_eq!(fs::metadata(&created).unwrap().len(), 0);
    }

    #[test]
    fn copy_into_renames_on_collision_and_keeps_the_original() {
        let dir = TempDir::new("copy-collision");
        let source_dir = dir.path().join("src");
        let dest_dir = dir.path().join("dst");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(source_dir.join("报告.docx"), b"new").unwrap();
        fs::write(dest_dir.join("报告.docx"), b"old").unwrap();

        let report = copy_into(&[source_dir.join("报告.docx")], &dest_dir).expect("不该整体失败");

        assert_eq!(report.done, 1);
        assert_eq!(report.renamed.len(), 1, "重名而改名的项必须记进回执");
        // 目标目录里原有的那份**不能**被动过。
        assert_eq!(fs::read(dest_dir.join("报告.docx")).unwrap(), b"old");
        assert_eq!(fs::read(dest_dir.join("报告 (2).docx")).unwrap(), b"new");
        assert!(
            source_dir.join("报告.docx").exists(),
            "复制必须保留源"
        );
    }

    #[test]
    fn move_into_moves_a_directory_with_its_contents() {
        let dir = TempDir::new("move-dir");
        let source = dir.path().join("src");
        let dest = dir.path().join("dst");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(source.join("a.txt"), b"A").unwrap();

        let report = move_into(std::slice::from_ref(&source), &dest).expect("不该整体失败");

        assert_eq!(report.done, 1);
        assert!(!source.exists(), "源应当已经不在原处");
        assert_eq!(fs::read(dest.join("src").join("a.txt")).unwrap(), b"A");
    }

    #[test]
    fn move_into_refuses_to_move_a_directory_into_itself() {
        let dir = TempDir::new("move-into-self");
        let source = dir.path().join("src");
        let inner = source.join("inner");
        fs::create_dir_all(&inner).unwrap();

        let report = move_into(std::slice::from_ref(&source), &inner).expect("不该整体失败");

        assert_eq!(report.done, 0);
        let reason = report
            .failed
            .first()
            .map(|(_, reason)| reason.clone())
            .unwrap_or_default();
        assert!(
            reason.contains("自己"),
            "自我吞食必须被拦下并说明原因，实际：{reason}"
        );
    }

    #[test]
    fn copy_into_refuses_symlinks() {
        let dir = TempDir::new("copy-symlink");
        let source_dir = dir.path().join("src");
        let dest_dir = dir.path().join("dst");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&dest_dir).unwrap();
        fs::write(source_dir.join("real.txt"), b"x").unwrap();

        let link = source_dir.join("link.txt");
        if std::os::windows::fs::symlink_file(source_dir.join("real.txt"), &link).is_err() {
            // 建符号链接需要开发者模式或管理员权限。环境不允许就跳过——
            // 这不是被测代码的失败。
            return;
        }

        let report = copy_into(&[link], &dest_dir).expect("不该整体失败");

        assert_eq!(report.done, 0);
        let reason = report
            .failed
            .first()
            .map(|(_, reason)| reason.clone())
            .unwrap_or_default();
        assert!(
            reason.contains("符号链接"),
            "必须如实说明拒绝的原因，实际：{reason}"
        );
    }

    #[test]
    fn paths_beyond_max_path_work() {
        // 造一条超过 260 个 UTF-16 码元的目录链，再在它上面做真实的新建与复制。
        // 这是「长路径支持」唯一有意义的验证方式：不加深路径就测不到。
        let dir = TempDir::new("long-path");
        let mut deep = dir.path().to_path_buf();
        while deep.as_os_str().encode_wide().count() < 300 {
            deep.push("很长的目录名abcdefghij");
            fs::create_dir(extended(&deep)).expect("应当能建出超过 MAX_PATH 的目录");
        }

        let created = create_dir(&deep, "新建文件夹").expect("深路径上应当能新建");
        assert!(created.is_dir());
        assert!(created.as_os_str().encode_wide().count() > 260);

        // 在这条深路径上再放一个文件，然后复制回浅路径：
        // 读、写、复制三个方向都经过了 `\\?\` 转换。
        let file = create_file(&deep, "很长的文件名abcdefghij.txt").expect("应当能建文件");
        fs::write(extended(&file), b"deep").expect("应当能写进去");

        let report = copy_into(&[file], dir.path()).expect("不该整体失败");
        assert_eq!(
            report.failed,
            Vec::new(),
            "深路径的复制不该有失败项：{:?}",
            report.failed
        );
        assert_eq!(report.done, 1);
    }
}
