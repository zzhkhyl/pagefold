//! 回收站：枚举（读 `$Recycle.Bin`）与移入（走 shell）。
//!
//! 两个方向用了**两套完全不同的实现**，理由见各自的函数注释：
//! 「读」自己解析 `$Recycle.Bin` 就够（省掉 COM），
//! 「写」必须交给 shell（那是 shell 的语义，不是文件系统的语义）。
//!
//! ## 为什么直接读 `$Recycle.Bin`，而不是走 Shell 命名空间
//!
//! 回收站没有文件系统路径——它是 shell 的虚拟文件夹，正经做法是
//! `SHGetDesktopFolder` → `ParseDisplayName` → `IShellFolder::EnumObjects`，
//! 再逐项取 `PKEY_*` 属性。那套要走 COM、要在正确的套间里初始化、
//! 还要处理每个属性的类型分支，代码量与失败面都远大于这里真正需要的东西：
//! 列出「删了什么、多大、什么时候删的」。
//!
//! 而每个卷的回收站物理布局自 Vista 起是稳定的：
//! `<卷>:\$Recycle.Bin\<用户 SID>\` 下成对出现
//! `$I<id>`（元数据）与 `$R<id>`（被删内容的实际存放位置）。
//! 读 `$I` 就能拿到**原路径、大小、删除时间**——正好是本视图要展示的三样。
//!
//! ## 权限边界
//!
//! 每个用户各占一个子目录，受 ACL 保护。这里**跳过读不到的目录**，
//! 于是「能列出什么」自然等于「当前用户有权看什么」，
//! 不需要额外去取当前用户的 SID 做筛选（取 SID 要开进程令牌，
//! 多一段 unsafe 却换不来额外能力）。

use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Context;
use fm_core::naming::unique_name;
use fm_core::{Entry, EntryKind, IconId};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_WANTNUKEWARNING,
    SHEmptyRecycleBinW, SHFILEOPSTRUCTW, SHFileOperationW,
};
use windows::Win32::Storage::FileSystem::GetLongPathNameW;
use windows::core::PCWSTR;

use super::fs_ops::extended;
use crate::fs::OpReport;

/// 枚举所有卷上的回收站条目。
///
/// 单个卷、单个条目失败都只跳过它：删除过程中断电会留下半个 `$I`，
/// 回收站里混着损坏项是常态，不能因此让整个视图报错。
pub fn enumerate() -> Vec<Entry> {
    let mut entries = Vec::new();

    // 直接试 A–Z，而不是先枚举「有哪些卷」：`FileSystem::drives()` 会顺带查容量，
    // 遇到空光驱或断开的网络盘可能阻塞数秒，而这里只需要一个目录是否存在。
    for letter in b'A'..=b'Z' {
        let bin = PathBuf::from(format!("{}:\\$Recycle.Bin", char::from(letter)));
        collect_volume(&bin, &mut entries);
    }

    entries
}

/// 把若干条目移入回收站，返回交给它的条目数。
///
/// ## 为什么这一件事必须走 shell
///
/// 「移入回收站」不是文件系统操作，而是 **shell 的行为**：它要在每个卷的
/// `$Recycle.Bin\<SID>\` 下写一对 `$I`/`$R`，还要维护回收站的元数据。
/// 自己实现等于重新实现一遍 shell 的删除语义，而且一定会漏掉细节
/// （配额、`$I` 的版本差异、跨卷时换卷的规则）。
/// 因此复制与移动走 `std::fs`（见 [`super::fs_ops`]），唯独删除走 shell。
///
/// ## 线程
///
/// **在独立线程里调用**。原因不是性能，而是 `SHFileOperationW` 在需要显示
/// 进度或警告时会创建窗口，而窗口需要一个能泵消息的线程；
/// 借 GPUI 的后台线程去跑，等于让它的消息泵服务另一套 UI。
/// 一次删除起一个短命线程（创建成本几十微秒）要便宜得多。
///
/// ## 标志位的取舍
///
/// - `FOF_ALLOWUNDO`：**核心**。没有它这就是永久删除。
/// - `FOF_NOCONFIRMATION`：确认对话框由我们自己出（设计稿把这一项当危险动作，
///   用带 `danger` 样式的确认框），没必要再弹一次系统的。
/// - `FOF_WANTNUKEWARNING`：但**保留**「无法进回收站、将被永久删除」这条警告
///   （超过回收站配额的大文件会走这条路）。按文档它会覆盖
///   `FOF_NOCONFIRMATION`，正是我们想要的：常规情况不问，真要销毁时一定问。
/// - 不加 `FOF_SILENT`：宁可让系统显示自己的进度条，也不要让删一个几 GB 的
///   目录看起来像卡死。设计稿的 `.transfer` 进度面板落地后可以再换掉它。
pub fn move_to_recycle_bin(paths: &[PathBuf]) -> anyhow::Result<usize> {
    if paths.is_empty() {
        return Ok(0);
    }

    let count = paths.len();
    let from = path_list(paths);

    let result = std::thread::Builder::new()
        .name(String::from("fm-recycle-bin"))
        .spawn(move || delete_impl(from))
        .context("无法创建回收站操作线程")?
        .join()
        // 子线程 panic 时不把 panic 继续抛给调用方：那会让一次删除失败
        // 变成整个进程崩溃。这里退化为一条普通的错误。
        .map_err(|_| anyhow::anyhow!("回收站操作线程异常终止"))?;

    result.map(|()| count)
}

/// 组装 `SHFileOperationW` 要的路径列表：**每个路径以 NUL 结尾，末尾再补一个 NUL**。
///
/// 这是这个 API 最容易写错的一处：写成普通的 NUL 分隔列表会被它读成
/// 一条包含多个路径的怪路径，而写成单个字符串则只会处理第一个。
fn path_list(paths: &[PathBuf]) -> Vec<u16> {
    let mut buffer = Vec::new();
    for path in paths {
        buffer.extend(path.as_os_str().encode_wide());
        buffer.push(0);
    }
    buffer.push(0);
    buffer
}

/// 真正的调用。必须在独立线程上执行（见 [`move_to_recycle_bin`]）。
fn delete_impl(from: Vec<u16>) -> anyhow::Result<()> {
    // `from` 必须活到调用返回：`pFrom` 只是借了它的指针。
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        // `fFlags` 是 u16，而这些常量是 u32 的新类型——必须显式收窄。
        fFlags: flags_to_u16(&[
            FOF_ALLOWUNDO,
            FOF_NOCONFIRMATION,
            FOF_WANTNUKEWARNING,
        ]),
        // pTo 保持 null：FO_DELETE 不需要目标。
        ..Default::default()
    };

    // SAFETY: `pFrom` 指向的 `from` 在本函数内始终存活且以双 NUL 结尾；
    // 其余字段由 `Default`（全零）给出，对 FO_DELETE 都是合法值。
    let code = unsafe { SHFileOperationW(&mut operation) };

    if code != 0 {
        anyhow::bail!("{}", describe_shell_code(code));
    }
    // 返回 0 也可能有项目被中断（用户点了系统进度条上的取消）。
    // 这里必须当成失败上报：否则会弹出一句「已删除 3 项」而实际只删了 1 项。
    if operation.fAnyOperationsAborted.as_bool() {
        anyhow::bail!("删除被中断，部分项目未处理");
    }

    Ok(())
}

/// 从回收站还原若干条目，返回逐条回执。
///
/// `paths` 是列表里的**物理路径**（`$R...`）——也就是 [`enumerate`] 填进
/// `Entry.path` 的那个位置，不是原路径。
///
/// ## 为什么还原可以自己搬，不必求 shell
///
/// 「移入」必须走 shell（配额、`$I` 版本、跨卷换卷都是它的语义）；但「还原」不是：
/// `$R` 内容与其原始位置**永远在同一个卷**上——这正是回收站的设计前提。
/// 因此一次 `MoveFile` 就够，既不复制也不跨卷。搬完删掉配对的 `$I`，
/// 条目就从回收站里消失了。
pub fn restore(paths: &[PathBuf]) -> OpReport {
    let mut report = OpReport::default();

    for path in paths {
        match restore_one(path) {
            Ok(()) => report.done += 1,
            // `{error:#}` 展开 anyhow 的整条因果链：
            // 只有最外层往往看不出是哪一步失败（读元数据？改名？）。
            Err(error) => report.failed.push((path.clone(), format!("{error:#}"))),
        }
    }

    report
}

fn restore_one(content: &Path) -> anyhow::Result<()> {
    let (meta, parsed) = read_metadata(content)?;
    let original = parsed.original_path;

    // 原名被占用时自动改名：用户可能又建了一个同名文件。
    // 覆盖会毁掉新文件，而直接报错又让他无从下手（他还得自己先去改名）。
    let parent = original
        .parent()
        .with_context(|| format!("「{}」没有父目录", original.display()))?;
    let name = original
        .file_name()
        .with_context(|| format!("「{}」没有文件名", original.display()))?
        .to_string_lossy()
        .into_owned();
    let target_name = unique_name(&name, |candidate| parent.join(candidate).exists());
    let target = parent.join(&target_name);

    fs::rename(extended(content), extended(&target))
        .with_context(|| format!("无法还原到「{}」", target.display()))?;

    // `$I` 删不掉不影响结果：内容已经回到原位，枚举会因为找不到 `$R` 而跳过这条残留。
    if let Err(error) = fs::remove_file(extended(&meta)) {
        tracing::debug!(?error, meta = %meta.display(), "还原后未能删除回收站元数据");
    }

    Ok(())
}

/// 读出 `$R...` 配对的 `$I...` 元数据，并解析出原路径。
fn read_metadata(content: &Path) -> anyhow::Result<(PathBuf, DeletedItem)> {
    let name = content
        .file_name()
        .and_then(|name| name.to_str())
        .context("回收站条目的名称不是有效 UTF-8")?;
    let id = name
        .strip_prefix("$R")
        .context("这不是回收站的内容文件（应以 $R 开头）")?;
    let meta = content.with_file_name(format!("$I{id}"));

    let bytes = fs::read(extended(&meta))
        .with_context(|| format!("无法读取回收站元数据「{}」", meta.display()))?;
    let parsed = parse_i_file(&bytes).context("回收站元数据已损坏，无法确定原位置")?;

    Ok((meta, parsed))
}

/// 按**原路径**从回收站还原（撤销「移到回收站」用）。
///
/// 移入时 shell 不会告诉我们 `$R` 叫什么，因此只能反查：枚举回收站里的每条 `$I`，
/// 找 `original_path` 命中的那些。同一路径被删过多次时会全部还原——
/// 撤销的语义是「把我刚删的那批东西弄回来」，多还一条比少还一条好。
///
/// 一条都没命中时返回空回执（调用方据此提示「回收站里已经找不到」）。
pub fn restore_by_originals(originals: &[PathBuf]) -> OpReport {
    if originals.is_empty() {
        return OpReport::default();
    }

    let mut targets = Vec::new();
    for entry in enumerate() {
        let content = entry.path.as_ref().clone();
        let Ok((_, parsed)) = read_metadata(&content) else {
            continue;
        };
        if originals
            .iter()
            .any(|original| same_path(original, &parsed.original_path))
        {
            targets.push(content);
        }
    }

    restore(&targets)
}

/// 两个路径是否指向同一个位置。
///
/// 比较前先**展开 8.3 短名**：同一个文件可能被两种形式引用——
/// `C:\Users\ADMINI~1\AppData\...`（短名，`%TEMP%` 就长这样）与
/// `C:\Users\Administrator\AppData\...`（长名，`$I` 里记的是它）。
/// 直接比字符串会把同一个文件判成两个，撤销「移到回收站」就会一条也找不到。
fn same_path(left: &Path, right: &Path) -> bool {
    normalize(left) == normalize(right)
}

/// 规范成「长名 + 小写」，用于比较。
///
/// 只规范化**父目录**：被删的文件本身已经不存在，没法对它调用
/// `GetLongPathNameW`；父目录还在，足以把短名展开。
fn normalize(path: &Path) -> String {
    let parent = path.parent().map(|parent| {
        long_path(parent).unwrap_or_else(|| parent.to_path_buf())
    });

    let joined = match (parent, path.file_name()) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    };
    joined.to_string_lossy().to_lowercase()
}

/// 用 `GetLongPathNameW` 把短名展开成长名；路径不存在时返回 `None`。
fn long_path(path: &Path) -> Option<PathBuf> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // 先按「长度可能翻倍」给足空间，再用返回的实际长度截断。
    let mut buffer = vec![0u16; wide.len().max(260) * 2];
    let length = unsafe { GetLongPathNameW(PCWSTR(wide.as_ptr()), Some(&mut buffer)) } as usize;

    if length == 0 || length > buffer.len() {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buffer[..length])))
}

/// 清空回收站（所有卷）。
///
/// 与「移入」同理交给 shell：清空要连同 `$I`、配额与各卷索引一起处理。
/// 三个标志的取舍：
///
/// - `NOCONFIRMATION`：确认框由我们自己出（危险动作，用 danger 样式的按钮）；
/// - `NOPROGRESSUI` / `NOSOUND`：清空通常很快，弹一个进度框再「叮」一声
///   反而像是出了什么事。
pub fn empty() -> anyhow::Result<()> {
    const SHERB_NOCONFIRMATION: u32 = 1;
    const SHERB_NOPROGRESSUI: u32 = 2;
    const SHERB_NOSOUND: u32 = 4;

    unsafe {
        SHEmptyRecycleBinW(
            None,
            PCWSTR::null(),
            SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
        )
        .context("清空回收站失败")
    }
}

/// 把若干 `FILEOPERATION_FLAGS` 合成 `SHFILEOPSTRUCTW::fFlags` 要的 `u16`。
fn flags_to_u16(flags: &[FILEOPERATION_FLAGS]) -> u16 {
    flags.iter().fold(0u32, |acc, flag| acc | flag.0) as u16
}

/// `SHFileOperationW` 的返回码 → 用户可读的原因。
///
/// 刻意只映射**一定认得**的 Win32 值。这个 API 的返回码混用两套编码：
/// 普通 Win32 错误码与 shell 私有的 `DE_*` 系列（0x71 起），
/// 而 `DE_*` 在本项目的 `windows` crate 里没有导出。
/// 与其手写那几个魔数（猜错会把「权限不足」说成「文件重名」，比报数字更糟），
/// 不如把原始码附上——那个数字是可以在网上搜到的。
fn describe_shell_code(code: i32) -> String {
    match code {
        2 | 3 => String::from("项目已不存在"),
        5 => String::from("没有权限，项目可能被占用或受保护"),
        32 => String::from("项目正被其它程序使用"),
        _ => format!("删除失败（系统错误码 {code}）"),
    }
}

/// 收集一个卷的回收站条目。
fn collect_volume(bin: &Path, out: &mut Vec<Entry>) {
    // 该卷没有回收站目录，或本用户读不到整个卷的回收站——都不是错误。
    let Ok(users) = fs::read_dir(bin) else {
        return;
    };

    for user in users.flatten() {
        // 每个用户一个子目录（目录名是 SID）。读不到就跳过，
        // 于是「能列出什么」自然等于「当前有权看什么」。
        let Ok(items) = fs::read_dir(user.path()) else {
            continue;
        };

        for item in items.flatten() {
            let file_name = item.file_name();
            let Some(file_name) = file_name.to_str() else {
                continue;
            };
            let Some(id) = file_name.strip_prefix("$I") else {
                continue;
            };

            // `$R` 是内容的实际存放位置。只认 `$I` 会列出一个双击必然失败的空壳，
            // 因此内容不在就直接跳过。
            let content = user.path().join(format!("$R{id}"));
            let Ok(metadata) = fs::symlink_metadata(&content) else {
                continue;
            };
            let Ok(bytes) = fs::read(item.path()) else {
                continue;
            };
            let Some(parsed) = parse_i_file(&bytes) else {
                continue;
            };

            // 展示名取原路径的文件名：用户认的是「我删掉的那个东西叫什么」，
            // 不是 `$R1A2B3C` 这种回收站内部编号。
            let name = parsed
                .original_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| parsed.original_path.display().to_string());

            out.push(Entry {
                name: Arc::from(name.as_str()),
                // 指向 `$R` 的真实路径，而不是原路径：预览面板要拿到一个
                // 此刻真实存在的位置，用原路径会指向一个已经不存在的地方。
                path: Arc::new(content),
                kind: if metadata.is_dir() {
                    EntryKind::Directory
                } else {
                    EntryKind::File { ext: None }
                },
                size: parsed.size,
                // 「修改日期」这一列在回收站里显示**删除时间**：
                // 原始修改时间没有存在 `$I` 里，而对着一堆被删文件，
                // 「什么时候删的」也是更有用的那一列。
                modified: Some(parsed.deleted_at),
                icon: IconId(0),
                // 回收站条目一律按可见处理：它们已经被删掉，「隐藏」在这里
                // 只会让用户找不到自己刚删的东西。
                hidden: false,
            });
        }
    }
}

/// 一个 `$I` 文件解析出的元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletedItem {
    /// 删除前的完整路径。
    pub original_path: PathBuf,
    /// 删除前的大小；目录的这个值没有意义。
    pub size: u64,
    /// 删除时间。
    pub deleted_at: SystemTime,
}

/// 解析 `$I<id>` 文件的内容。
///
/// 布局（Vista 起稳定，Win10 起 `version == 2`）：
///
/// | 偏移 | 长度 | 含义 |
/// |---|---|---|
/// | 0 | 8 | 版本：`1` = Vista/7/8，`2` = Win10 及以后 |
/// | 8 | 8 | 原始大小 |
/// | 16 | 8 | 删除时间（FILETIME：自 1601-01-01 UTC 起的 100 纳秒数） |
/// | 24 | 4 | **仅 `version >= 2`**：路径字节长度（含结尾的 NUL） |
/// | 24 或 28 | 余下 | 原始路径（UTF-16LE，NUL 结尾） |
///
/// 刻意**不校验**「路径长度字段与实际内容是否一致」：Windows 自己只依赖 NUL 结尾，
/// 而截断的 `$I` 在实践中确实存在（删除中途断电）。宁可拿到一条路径，
/// 也不要因为长度字段对不上就把整条丢掉。
pub fn parse_i_file(bytes: &[u8]) -> Option<DeletedItem> {
    // 前 24 字节是三个 u64。短于它的一律不是合法的 `$I`。
    if bytes.len() < 24 {
        return None;
    }

    let version = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let size = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let filetime = u64::from_le_bytes(bytes[16..24].try_into().ok()?);

    // 未知版本按「有长度字段」试：多出来的那几个字节会被下面的
    // NUL 截断逻辑自然吃掉，不会污染路径。
    let path_start = if version >= 2 { 28 } else { 24 };
    let path_bytes = bytes.get(path_start..)?;

    // UTF-16LE：两字节一个码元，读到 NUL 为止。
    let mut units: Vec<u16> = Vec::with_capacity(path_bytes.len() / 2);
    for pair in path_bytes.as_chunks::<2>().0 {
        let unit = u16::from_le_bytes(*pair);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    if units.is_empty() {
        return None;
    }

    Some(DeletedItem {
        original_path: PathBuf::from(String::from_utf16_lossy(&units)),
        size,
        deleted_at: filetime_to_system_time(filetime),
    })
}

/// FILETIME → `SystemTime`。
///
/// FILETIME 的计程起点是 1601-01-01，而 `SystemTime` 在 Windows 上也以 1601 为原点，
/// 但**不能**直接把值塞进去：1601 年之前的间隔在 `Duration` 里会溢出。
/// 因此以 UNIX 纪元为参照换算，并把早于它的值夹到纪元起点——
/// `$I` 里时间为 0 表示「未记录」，不是真的 1601 年。
fn filetime_to_system_time(filetime: u64) -> SystemTime {
    /// 1601-01-01 到 1970-01-01 之间的 100 纳秒数。
    const EPOCH_DIFFERENCE: u64 = 116_444_736_000_000_000;

    if filetime < EPOCH_DIFFERENCE {
        return SystemTime::UNIX_EPOCH;
    }

    SystemTime::UNIX_EPOCH + Duration::from_nanos((filetime - EPOCH_DIFFERENCE) * 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 按 `$I` 的布局造一份内容。
    ///
    /// `version >= 2` 时写入路径长度字段——与真实文件一致，
    /// 顺便让「长度字段有没有被正确跳过」这件事被测试覆盖。
    fn i_file(version: u64, size: u64, filetime: u64, path: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&filetime.to_le_bytes());

        let mut wide: Vec<u16> = path.encode_utf16().collect();
        wide.push(0);
        if version >= 2 {
            bytes.extend_from_slice(&((wide.len() * 2) as u32).to_le_bytes());
        }
        for unit in wide {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes
    }

    /// 2024-01-01T00:00:00Z 的 FILETIME 值。
    const FILETIME_2024: u64 = 133_485_408_000_000_000;

    /// 造一个隔离的临时目录（每个用例独占一个，结束即删）。
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pagefold-recycle-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    /// 在 `bin` 下造一对 `$I`/`$R`，原路径是 `original`。
    fn make_entry(bin: &Path, id: &str, original: &Path, content: &[u8]) -> PathBuf {
        fs::create_dir_all(bin).expect("创建回收站目录");
        let payload = bin.join(format!("$R{id}"));
        fs::write(&payload, content).expect("写入内容");
        let metadata = i_file(2, content.len() as u64, FILETIME_2024, &original.to_string_lossy());
        fs::write(bin.join(format!("$I{id}")), metadata).expect("写入元数据");
        payload
    }

    #[test]
    fn restore_puts_the_content_back_and_drops_the_metadata() {
        let root = scratch("restore");
        let bin = root.join("bin");
        let origin = root.join("origin");
        fs::create_dir_all(&origin).expect("创建原目录");

        let original = origin.join("报告.txt");
        let payload = make_entry(&bin, "ABC.txt", &original, b"hello");

        let report = restore(std::slice::from_ref(&payload));

        assert!(report.failed.is_empty(), "失败项：{:?}", report.failed);
        assert_eq!(report.done, 1);
        assert_eq!(fs::read(&original).expect("内容应回到原位"), b"hello");
        assert!(!payload.exists(), "应当移动，而不是复制");
        assert!(
            !bin.join("$IABC.txt").exists(),
            "还原后必须删掉配对的 $I，否则回收站里会留下一条空壳"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_renames_when_the_original_name_is_taken_again() {
        let root = scratch("restore-collision");
        let bin = root.join("bin");
        let origin = root.join("origin");
        fs::create_dir_all(&origin).expect("创建原目录");

        let original = origin.join("报告.txt");
        // 用户在原位置又建了一个同名文件：还原不能覆盖它。
        fs::write(&original, b"new").expect("写入同名文件");
        let payload = make_entry(&bin, "DEF.txt", &original, b"old");

        let report = restore(std::slice::from_ref(&payload));

        assert_eq!(report.done, 1, "失败项：{:?}", report.failed);
        assert_eq!(fs::read(&original).unwrap(), b"new", "同名文件不能被覆盖");
        assert_eq!(
            fs::read(origin.join("报告 (2).txt")).expect("应自动改名"),
            b"old"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// 端到端：真把文件送进系统回收站，再枚举、还原回来。
    ///
    /// 默认 `#[ignore]`：它会**临时**往本机回收站里塞一条记录（还原后即消失），
    /// 不适合混在日常 `cargo test` 里。需要时手动跑：
    /// `cargo test -p fm-platform -- --ignored recycle_bin_round_trip`
    #[test]
    #[ignore = "会临时往本机回收站写入并还原一个文件"]
    fn recycle_bin_round_trip() {
        let root = scratch("round-trip");
        let dir = root.join("origin");
        fs::create_dir_all(&dir).expect("创建原目录");
        // 名字带进程号：上一次失败留下的条目不会让这次匹配到别人的文件。
        let name = format!("pagefold-rt-{}.txt", std::process::id());
        let file = dir.join(&name);
        fs::write(&file, b"round trip").expect("写入文件");

        let count = move_to_recycle_bin(std::slice::from_ref(&file)).expect("移入回收站");
        assert_eq!(count, 1);
        assert!(!file.exists(), "移入后原位置不该还有它");

        let entry = enumerate()
            .into_iter()
            .find(|entry| entry.name.as_ref() == name)
            .expect("回收站里应当能列出刚删掉的文件");

        let report = restore(&[entry.path.as_ref().clone()]);

        assert_eq!(report.done, 1, "失败项：{:?}", report.failed);
        assert_eq!(fs::read(&file).expect("内容应回到原位"), b"round trip");

        // 再删一次，走「撤销」那条路：用**原路径**反查回收站。
        move_to_recycle_bin(std::slice::from_ref(&file)).expect("再次移入回收站");
        let report = restore_by_originals(std::slice::from_ref(&file));
        assert_eq!(report.done, 1, "失败项：{:?}", report.failed);
        assert_eq!(
            fs::read(&file).expect("撤销后内容应回到原位"),
            b"round trip"
        );

        // 同一条路径已经还原过了，再撤一次应当找不到东西（而不是重复还原）。
        let again = restore_by_originals(std::slice::from_ref(&file));
        assert_eq!(again.done, 0, "回收站里已经没有这条了");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_reports_a_broken_metadata_file_without_touching_the_content() {
        let root = scratch("restore-broken");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("创建回收站目录");
        // 只有 $R，没有 $I：删除中途断电就会留下这种条目。
        let payload = bin.join("$RXYZ.txt");
        fs::write(&payload, b"orphan").expect("写入内容");

        let report = restore(std::slice::from_ref(&payload));

        assert_eq!(report.done, 0);
        assert_eq!(report.failed.len(), 1);
        assert!(
            payload.exists(),
            "元数据读不到时必须原样不动——宁可报错，也不能把内容搬到猜出来的位置"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_version_2_with_the_path_length_field() {
        let bytes = i_file(2, 4096, FILETIME_2024, r"C:\Users\me\报告.docx");
        let parsed = parse_i_file(&bytes).expect("应当解析成功");

        assert_eq!(
            parsed.original_path,
            PathBuf::from(r"C:\Users\me\报告.docx"),
            "版本 2 有 4 字节的路径长度字段，路径起点必须跳过它"
        );
        assert_eq!(parsed.size, 4096);
        assert_eq!(parsed.deleted_at, SystemTime::UNIX_EPOCH + Duration::from_secs(1704067200));
    }

    #[test]
    fn parses_version_1_without_the_path_length_field() {
        // 版本 1（Vista/7/8）没有长度字段，路径直接从偏移 24 开始。
        // 若错按版本 2 解析，路径会整体错位两个字符。
        let bytes = i_file(1, 8, FILETIME_2024, r"D:\a\b.txt");
        let parsed = parse_i_file(&bytes).expect("应当解析成功");
        assert_eq!(parsed.original_path, PathBuf::from(r"D:\a\b.txt"));
    }

    #[test]
    fn rejects_files_shorter_than_the_header() {
        assert!(parse_i_file(&[]).is_none());
        assert!(parse_i_file(&[0u8; 23]).is_none());
    }

    #[test]
    fn rejects_an_empty_path() {
        // 三个 u64 都在，但路径一开始就是 NUL：这是损坏项，不能当条目用。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&FILETIME_2024.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        assert!(parse_i_file(&bytes).is_none());
    }

    #[test]
    fn tolerates_a_path_without_a_terminator() {
        // 截断的 `$I` 是真实存在的：路径写满但缺 NUL 时仍应尽量取出路径。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&2u64.to_le_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&FILETIME_2024.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        for unit in "C:\\x".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let parsed = parse_i_file(&bytes).expect("没有 NUL 也应解析出路径");
        assert_eq!(parsed.original_path, PathBuf::from("C:\\x"));
    }

    #[test]
    fn zeroed_time_is_clamped_to_the_unix_epoch() {
        // `$I` 里时间为 0 表示「未记录」。若直接按 FILETIME 原点换算，
        // `Duration` 会因为 1601 年那个负间隔而溢出 panic。
        let bytes = i_file(2, 0, 0, r"C:\x");
        let parsed = parse_i_file(&bytes).expect("应当解析成功");
        assert_eq!(parsed.deleted_at, SystemTime::UNIX_EPOCH);
    }
}

