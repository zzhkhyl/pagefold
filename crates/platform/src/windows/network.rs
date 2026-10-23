//! 网络共享：枚举 `\\主机` 下的共享列表。
//!
//! ## 为什么需要单独一条路
//!
//! `\\主机\共享` 是普通目录，`fs::read_dir` 就能读；但 `\\主机` 本身
//! **在 Windows 里不是目录**（`Path::is_dir` 恒为 false，`GetDirectories`
//! 会报「UNC 路径的形式应为 \\server\share」）。列它的共享只能走
//! `NetShareEnum`——资源管理器打开 `\\主机` 时用的也是同一套网络 API。
//!
//! 这也是「输入 \\10.x.x.x 打不开」的直接原因：地址栏那句 `path.is_dir()`
//! 判定为假，于是报「路径不存在」，而它其实是一台待列举的主机。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::bail;
use fm_core::{Entry, EntryKind, IconId};
use windows::Win32::NetworkManagement::NetManagement::NetApiBufferFree;
use windows::Win32::Storage::FileSystem::{
    NetShareEnum, SHARE_INFO_1, STYPE_DISKTREE, STYPE_MASK, STYPE_PRINTQ, STYPE_SPECIAL,
};
use windows::core::PCWSTR;

/// `\\主机`（可带结尾反斜杠）：只有服务器名、没有共享名的路径。
pub fn is_network_root(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    let Some(rest) = text.strip_prefix(r"\\") else {
        return false;
    };
    let rest = rest.trim_end_matches('\\');
    // 有名字、且再没有分隔符：`\\host` 是主机根，`\\host\share` 是普通目录。
    !rest.is_empty() && !rest.contains('\\')
}

/// 取路径里的主机名：`\\host\share\a` → `host`。
pub fn host_of(path: &Path) -> Option<String> {
    let text = path.as_os_str().to_string_lossy();
    let rest = text.strip_prefix(r"\\")?;
    let host = rest.split('\\').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// 列出 `\\host` 下的共享：**磁盘共享进列表，打印机也进列表**。
///
/// 三类的处理各不相同：
///
/// | 类型 | 处理 |
/// | --- | --- |
/// | `STYPE_DISKTREE` | 普通目录，可进入浏览 |
/// | `STYPE_PRINTQ` | 共享打印机：列出来（`EntryKind::Printer`），双击交给系统打开打印队列 |
/// | 其它（`IPC$` 等） | 跳过：管道不是用户要找的东西 |
///
/// 管理共享（`C$` / `ADMIN$`，带 `STYPE_SPECIAL` 位）标记为隐藏：
/// 它们默认不出现在列表里，与资源管理器一致；打开「显示隐藏文件」后可见。
pub fn shares(host: &str) -> anyhow::Result<Vec<Entry>> {
    // `NetShareEnum` 要求服务器名本身带 UNC 前缀。
    let server: Vec<u16> = format!(r"\\{host}")
        .encode_utf16()
        .chain([0])
        .collect();

    let mut buffer: *mut u8 = std::ptr::null_mut();
    let mut read: u32 = 0;
    let mut total: u32 = 0;
    let mut resume: u32 = 0;

    // SAFETY: `buffer` 由系统分配，成功时由我们负责用 `NetApiBufferFree` 释放；
    // `read` 是实际写入的条目数，下面按它切片。
    let status = unsafe {
        NetShareEnum(
            PCWSTR(server.as_ptr()),
            1,
            &mut buffer,
            u32::MAX,
            &mut read,
            &mut total,
            Some(&mut resume),
        )
    };
    if status != 0 {
        bail!("{}", describe(status));
    }

    let mut entries = Vec::new();
    // SAFETY: 成功的调用保证 `buffer` 指向 `read` 个 `SHARE_INFO_1`，
    // 且这块内存直到 `NetApiBufferFree` 之前都有效。
    unsafe {
        if !buffer.is_null() {
            let items = std::slice::from_raw_parts(buffer as *const SHARE_INFO_1, read as usize);
            for item in items {
                // 磁盘共享与共享打印机都列；IPC$ 之类的管道跳过。
                let Some(entry_kind) = kind_of(item.shi1_type.0) else {
                    continue;
                };

                let name = item.shi1_netname.to_string().unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                entries.push(Entry {
                    path: Arc::new(PathBuf::from(format!(r"\\{host}\{name}"))),
                    name: Arc::from(name.as_str()),
                    kind: entry_kind,
                    size: 0,
                    // 共享本身没有「修改时间」；列里显示破折号比编一个更诚实。
                    modified: None,
                    icon: IconId(0),
                    hidden: item.shi1_type.0 & STYPE_SPECIAL.0 != 0,
                });
            }
            let _ = NetApiBufferFree(Some(buffer as *const core::ffi::c_void));
        }
    }

    // 目录在前、打印机在后，同类按名称排序：一台机器上打印机通常只有一两台，
    // 混在目录里反而更难找。
    entries.sort_by(|left, right| {
        let rank = |entry: &Entry| if entry.is_dir() { 0 } else { 1 };
        rank(left)
            .cmp(&rank(right))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    tracing::debug!(
        host,
        shares = entries.len(),
        printers = entries.iter().filter(|entry| !entry.is_dir()).count(),
        "已枚举网络共享"
    );

    Ok(entries)
}

/// 把 `SHARE_INFO_1::shi1_type` 翻成条目的类型。
///
/// 只认两类用户真正要找的东西：磁盘共享与共享打印机。
/// `STYPE_IPC`（`IPC$` 那样的命名管道）与 `STYPE_DEVICE` 一律 `None`——它们
/// 既不能浏览也不能打开，列出来只会让人点一下得到一条错误。
///
/// 抽成纯函数是为了能单测：共享类型的位组合（`STYPE_SPECIAL`、
/// `STYPE_TEMPORARY` 之类的附加位）很容易判错，判错的后果是「打印机看不见」。
fn kind_of(share_type: u32) -> Option<EntryKind> {
    match share_type & STYPE_MASK.0 {
        value if value == STYPE_DISKTREE.0 => Some(EntryKind::Directory),
        value if value == STYPE_PRINTQ.0 => Some(EntryKind::Printer),
        _ => None,
    }
}

/// 把 `Net*` 的返回码翻成一句人话。
///
/// 这些是 `lmerr.h` 里的 `NERR_*` / Win32 错误码混用：0 表示成功，
/// 其余与我们平时见的 `io::Error` 不是一套。只翻译最常撞上的几个，
/// 剩下的原样带出数字——那个数字是可以搜索到的。
fn describe(status: u32) -> String {
    match status {
        5 => String::from("拒绝访问：需要凭据或该账户无权查看共享"),
        53 => String::from("找不到网络路径：主机不可达，或名称解析失败"),
        67 => String::from("找不到网络名：共享不存在"),
        1219 => String::from("已用其它凭据连接过该主机，请先断开再试"),
        1326 => String::from("登录失败：用户名或密码不正确"),
        1231 => String::from("网络位置不可达"),
        1232 => String::from("无法访问网络位置"),
        other => format!("无法枚举共享（网络错误码 {other}）"),
    }
}

/// 连接一个需要凭据的共享。
///
/// `CONNECT_INTERACTIVE` 允许网络提供方在需要时弹出**系统的**凭据对话框——
/// 这是资源管理器的行为。只在扫描失败后才调用，避免每次浏览都弹框。
pub fn connect_interactive(path: &Path) -> anyhow::Result<()> {
    use windows::Win32::NetworkManagement::WNet::{
        CONNECT_INTERACTIVE, CONNECT_TEMPORARY, NETRESOURCEW, RESOURCETYPE_DISK,
        WNetAddConnection2W,
    };

    let remote: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();

    let resource = NETRESOURCEW {
        // `dwScope` 留默认（0 = CONNECTABLE）：直接连接一个具体共享。
        dwScope: Default::default(),
        dwType: RESOURCETYPE_DISK,
        dwDisplayType: 0,
        dwUsage: 0,
        lpLocalName: windows::core::PWSTR::null(),
        // `lpRemoteName` 只借用上面的 buffer，调用期间它必须活着。
        lpRemoteName: windows::core::PWSTR(remote.as_ptr() as *mut u16),
        lpComment: windows::core::PWSTR::null(),
        lpProvider: windows::core::PWSTR::null(),
    };

    let status = unsafe {
        WNetAddConnection2W(
            &resource,
            PCWSTR::null(),
            PCWSTR::null(),
            CONNECT_INTERACTIVE | CONNECT_TEMPORARY,
        )
    };
    // ERROR_ALREADY_ASSIGNED(85) 与「已经连上」都算成功：目标状态已达成。
    if status.0 == 0 || status.0 == 85 {
        Ok(())
    } else {
        bail!("{}", describe(status.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_types_map_to_the_right_entry_kind() {
        // 磁盘共享 → 目录；共享打印机 → 打印机；IPC$ / 保留类型 → 不列。
        assert_eq!(
            kind_of(STYPE_DISKTREE.0),
            Some(EntryKind::Directory),
            "磁盘共享应当可以浏览"
        );
        assert_eq!(
            kind_of(STYPE_PRINTQ.0),
            Some(EntryKind::Printer),
            "共享打印机必须列出来（这正是「有打印机却看不到」的修复点）"
        );

        // 真实共享几乎总带着 `STYPE_SPECIAL` 位（管理共享）或其它附加位，
        // 所以判定必须先与 `STYPE_MASK` 取与，这一点靠下面两条钉住。
        assert_eq!(
            kind_of(STYPE_DISKTREE.0 | STYPE_SPECIAL.0),
            Some(EntryKind::Directory)
        );
        assert_eq!(
            kind_of(STYPE_PRINTQ.0 | STYPE_SPECIAL.0),
            Some(EntryKind::Printer)
        );

        // IPC$：0x3（STYPE_IPC），不列。
        // 注意 `STYPE_DISKTREE` 本身是 **0**，所以「0 → None」是错的断言：
        // 磁盘共享的类型值就是 0，这也是这个函数必须按 STYPE_MASK 判定的原因。
        assert_eq!(kind_of(0x3), None, "IPC$ 不该进列表");
        assert_eq!(kind_of(0x2), None, "STYPE_DEVICE 也不该进列表");
        assert_eq!(kind_of(0), Some(EntryKind::Directory));
    }

    #[test]
    fn only_a_bare_host_counts_as_network_root() {
        // 主机根：需要枚举共享
        assert!(is_network_root(Path::new(r"\\10.0.0.5")));
        assert!(is_network_root(Path::new(r"\\10.0.0.5\")));
        assert!(is_network_root(Path::new(r"\\nas-01")));

        // 普通目录：不归这条路管
        assert!(!is_network_root(Path::new(r"\\10.0.0.5\share")));
        assert!(!is_network_root(Path::new(r"\\10.0.0.5\share\a\b")));
        assert!(!is_network_root(Path::new(r"C:\Users")));
        // 空路径与单反斜杠都不是主机根（`\` 是「当前卷根」的老写法）
        assert!(!is_network_root(Path::new(r"")));
        assert!(!is_network_root(Path::new(r"\")));
    }

    #[test]
    fn host_is_the_first_segment_after_the_prefix() {
        assert_eq!(host_of(Path::new(r"\\10.0.0.5\share")).as_deref(), Some("10.0.0.5"));
        assert_eq!(host_of(Path::new(r"\\nas\a\b")).as_deref(), Some("nas"));
        assert_eq!(host_of(Path::new(r"\\nas")), Some(String::from("nas")));
        assert_eq!(host_of(Path::new(r"C:\Users")), None);
    }

    #[test]
    fn network_errors_become_readable_sentences() {
        assert!(describe(53).contains("找不到网络路径"));
        assert!(describe(1326).contains("登录失败"));
        // 没翻译的码要保留原始数字，便于搜索。
        assert!(describe(6118).contains("6118"));
    }
}
