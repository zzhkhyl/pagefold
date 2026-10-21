//! 跨进程文件剪贴板：读写系统剪贴板里的 `CF_HDROP`。
//!
//! 应用内的剪贴板状态在 `fm-services::clipboard`（纯内存暂存区，含复制/剪切语义、
//! 冲突改名、剪切后失效）；本文件负责**与其它程序互通**的那一半：
//! 在本程序里复制、到资源管理器里粘贴，反之亦然。
//!
//! ## 为什么不需要 OLE / `IDataObject`
//!
//! 早期的规划笔记写的是「需要 `OleSetClipboard` + `OleFlushClipboard`，
//! 因此 `OleInitialize` 必须在主线程完成」。那句话对**通用**数据对象成立，
//! 对本程序不成立：
//!
//! - 我们剪贴板里的内容永远是**真实存在的本地路径**，没有虚拟文件、不需要延迟渲染；
//! - 直接写 `CF_HDROP`（`DROPFILES` 头 + UTF-16 路径列表）并 `SetClipboardData`，
//!   内存所有权随即交给系统，**本进程退出后粘贴依然有效**——这正是
//!   `OleFlushClipboard` 要解决的问题；
//! - 于是不必初始化 OLE、不必实现 `IDataObject`，也就不必担心 STA 线程模型
//!   （P3 里拖放才真正需要它）。
//!
//! 资源管理器、Total Commander 以及绝大多数接收文件粘贴的程序都认这个格式。
//!
//! ## 两个格式
//!
//! | 格式 | 内容 | 作用 |
//! | --- | --- | --- |
//! | `CF_HDROP`（15） | `DROPFILES` + 双 NUL 结尾的 UTF-16 路径 | 文件列表本身 |
//! | `Preferred DropEffect` | 4 字节 DWORD：1=复制、2=移动 | 让接收方知道该复制还是剪切 |
//!
//! 第二个是**可选**的：缺了它，资源管理器按「复制」处理。因此写入失败不影响主功能。
//!
//! ## ANSI 列表
//!
//! 老程序可能写 `fWide = 0` 的 ANSI 列表（本机代码页编码）。这里不猜代码页——
//! 猜错会把中文路径变成乱码，而粘贴乱码路径的后果比「不支持」严重得多。
//! 读到 ANSI 列表时按「剪贴板里没有文件」处理（调用方会退化为应用内剪贴板）。

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// `CF_HDROP` 的数值 ID。
///
/// 用字面量而不是从 `Win32_System_Ole` 里取：它是一个自 Windows 3.1 起就没变过的
/// 稳定常量，为此多开一个 feature 不划算。
const CF_HDROP: u32 = 15;

/// `DROPFILES` 结构体的大小：`DWORD pFiles; POINT pt; BOOL fNC; BOOL fWide;`
/// = 4 + 8 + 4 + 4。
const DROPFILES_HEADER: usize = 20;

/// 表示「这个文件列表是移动还是复制」的剪贴板格式名。
const PREFERRED_DROP_EFFECT: &str = "Preferred DropEffect";

const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;

/// 把路径列表编码成 `CF_HDROP` 的字节内容。
///
/// 单独拆出来是为了能单测：真正的 `SetClipboardData` 会污染开发机的剪贴板，
/// 不适合放进默认的测试里。
pub fn encode(paths: &[PathBuf]) -> Vec<u8> {
    let mut buffer = vec![0u8; DROPFILES_HEADER];
    // pFiles：路径列表相对于结构体开头的偏移。
    buffer[0..4].copy_from_slice(&(DROPFILES_HEADER as u32).to_le_bytes());
    // pt 保持 (0,0)、fNC 保持 FALSE：它们只影响拖放时的落点与命中区。
    // fWide = TRUE：写入 UTF-16，中文路径才不会损坏。
    buffer[16..20].copy_from_slice(&1u32.to_le_bytes());

    for path in paths {
        // 用 `to_string_lossy`：路径若不是合法 UTF-16（极少见），
        // 丢一个替换字符也比整批拒绝粘贴好。
        for unit in path.to_string_lossy().encode_utf16() {
            buffer.extend_from_slice(&unit.to_le_bytes());
        }
        // 每个路径以 NUL 结尾。
        buffer.extend_from_slice(&0u16.to_le_bytes());
    }
    // 整份列表再补一个 NUL（规范要求双 NUL 收尾）。
    buffer.extend_from_slice(&0u16.to_le_bytes());
    buffer
}

/// 把路径写入系统剪贴板。`cut` 决定 `Preferred DropEffect` 是移动还是复制。
pub fn write_paths(paths: &[PathBuf], cut: bool) -> Result<()> {
    if paths.is_empty() {
        bail!("没有可写入剪贴板的条目");
    }
    let list = encode(paths);

    unsafe {
        OpenClipboard(None).context("系统剪贴板被其它程序占用")?;
        // 无论写成功与否都要关闭剪贴板，否则其它程序会一直打不开它。
        let result = write_locked(&list, cut);
        let _ = CloseClipboard();
        result
    }
}

fn write_locked(list: &[u8], cut: bool) -> Result<()> {
    unsafe {
        EmptyClipboard().context("无法清空系统剪贴板")?;

        let handle = GlobalAlloc(GMEM_MOVEABLE, list.len()).context("无法为剪贴板分配内存")?;
        let target = GlobalLock(handle);
        if target.is_null() {
            let _ = GlobalFree(Some(handle));
            bail!("无法锁定剪贴板内存");
        }
        std::ptr::copy_nonoverlapping(list.as_ptr(), target as *mut u8, list.len());
        let _ = GlobalUnlock(handle);

        // `SetClipboardData` 成功后内存所有权归系统，**不能再释放**。
        if SetClipboardData(CF_HDROP, Some(HANDLE(handle.0))).is_err() {
            let _ = GlobalFree(Some(handle));
            bail!("无法把文件列表写入剪贴板");
        }

        // 拖放效果是可选格式：写不进去也不影响主功能（接收方按「复制」处理）。
        let format = register(PREFERRED_DROP_EFFECT);
        if format != 0
            && let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, 4)
        {
            let target = GlobalLock(handle);
            if target.is_null() {
                let _ = GlobalFree(Some(handle));
            } else {
                *(target as *mut u32) = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY };
                let _ = GlobalUnlock(handle);
                if SetClipboardData(format, Some(HANDLE(handle.0))).is_err() {
                    let _ = GlobalFree(Some(handle));
                }
            }
        }

        Ok(())
    }
}

/// 读出剪贴板里的文件列表：`(路径, 是否剪切)`。
///
/// 返回 `Ok(None)` 表示「剪贴板里没有文件」（空的、文本、或不支持的格式）。
/// 这类情况不是错误——调用方会退化为应用内剪贴板，或提示「没有可粘贴的文件」。
pub fn read_paths() -> Result<Option<(Vec<PathBuf>, bool)>> {
    unsafe {
        // 打不开说明被别的程序占着，当作「没有」而不是报错：用户按 Ctrl+V 时
        // 弹一句「剪贴板被占用」毫无帮助，他只会觉得程序坏了。
        if OpenClipboard(None).is_err() {
            return Ok(None);
        }
        let result = read_locked();
        let _ = CloseClipboard();
        result
    }
}

fn read_locked() -> Result<Option<(Vec<PathBuf>, bool)>> {
    unsafe {
        let Ok(handle) = GetClipboardData(CF_HDROP) else {
            return Ok(None);
        };
        if handle.is_invalid() || handle.0.is_null() {
            return Ok(None);
        }

        let drop = HDROP(handle.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        if count == 0 {
            return Ok(None);
        }

        let mut paths = Vec::with_capacity(count as usize);
        for index in 0..count {
            // 先问长度（不含结尾 NUL），再按长度取内容。
            let length = DragQueryFileW(drop, index, None) as usize;
            if length == 0 {
                continue;
            }
            let mut buffer = vec![0u16; length + 1];
            let written = DragQueryFileW(drop, index, Some(&mut buffer)) as usize;
            if written == 0 {
                continue;
            }
            buffer.truncate(written);
            paths.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
        }

        if paths.is_empty() {
            return Ok(None);
        }

        Ok(Some((paths, reads_as_move())))
    }
}

/// 剪贴板是否要求「移动」（来自别的程序的剪切）。
fn reads_as_move() -> bool {
    unsafe {
        let format = register(PREFERRED_DROP_EFFECT);
        if format == 0 {
            return false;
        }
        let Ok(handle) = GetClipboardData(format) else {
            return false;
        };
        if handle.is_invalid() || handle.0.is_null() {
            return false;
        }
        let memory = HGLOBAL(handle.0);
        let target = GlobalLock(memory);
        if target.is_null() {
            return false;
        }
        let effect = *(target as *const u32);
        let _ = GlobalUnlock(memory);
        effect == DROPEFFECT_MOVE
    }
}

/// 注册（或查询）一个自定义剪贴板格式，返回它的数值 ID；失败时返回 0。
fn register(name: &str) -> u32 {
    let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
    unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(wide.as_ptr())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_writes_a_double_null_terminated_wide_list() {
        let bytes = encode(&[PathBuf::from(r"C:\a\b.txt")]);
        let header = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        assert_eq!(header, DROPFILES_HEADER, "路径列表必须紧接在 20 字节的头之后");
        assert_eq!(
            u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
            1,
            "fWide 必须为 1，否则中文路径会损坏"
        );

        let units: Vec<u16> = bytes[DROPFILES_HEADER..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let end = units.iter().position(|unit| *unit == 0).expect("缺少结尾 NUL");
        assert_eq!(String::from_utf16(&units[..end]).unwrap(), r"C:\a\b.txt");
        // 结尾必须是两个 NUL（单个路径时就是「路径 \0 \0」）。
        assert_eq!(&units[units.len() - 2..], &[0, 0], "列表必须双 NUL 收尾");
    }

    #[test]
    fn encode_keeps_every_path_and_non_ascii_names() {
        let paths = vec![
            PathBuf::from(r"C:\报告\2026.xlsx"),
            PathBuf::from(r"D:\照片\😀.png"),
        ];
        let bytes = encode(&paths);
        let units: Vec<u16> = bytes[DROPFILES_HEADER..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let text = String::from_utf16(&units[..units.len() - 2]).unwrap();
        // 尾巴是「路径2 的结尾 NUL + 列表的结尾 NUL」，去掉这两格之后
        // 剩下「路径1 \0 路径2」——分隔符保留，收尾的 NUL 不保留。
        assert_eq!(text, "C:\\报告\\2026.xlsx\0D:\\照片\\😀.png");
    }

    #[test]
    fn encoding_an_empty_list_is_still_well_formed() {
        // 生产代码会拒绝空列表，但编码本身不该 panic。
        let bytes = encode(&[]);
        assert_eq!(bytes.len(), DROPFILES_HEADER + 2);
    }

    /// 真正读写剪贴板的往返测试。
    ///
    /// 默认 `#[ignore]`：它会**覆盖开发机当前的剪贴板**，不该混在日常
    /// `cargo test` 里。需要时手动跑：
    /// `cargo test -p fm-platform -- --ignored clipboard_round_trip`
    #[test]
    #[ignore = "会覆盖本机剪贴板内容"]
    fn clipboard_round_trip() {
        let paths = vec![
            PathBuf::from(r"C:\Windows\win.ini"),
            PathBuf::from(r"C:\报告\2026.xlsx"),
        ];
        write_paths(&paths, true).expect("写入剪贴板");

        let (read, cut) = read_paths().expect("读取剪贴板").expect("应当读到文件列表");
        assert_eq!(read, paths);
        assert!(cut, "Preferred DropEffect 为移动时必须读回「剪切」");
    }
}
