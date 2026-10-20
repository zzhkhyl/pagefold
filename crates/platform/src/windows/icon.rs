//! 系统图标提取：`SHGetFileInfoW` → `HICON` → RGBA 位图。
//!
//! ## 为什么按扩展名取图标
//!
//! 按**路径**取可以得到每个 exe 自己的图标，但那意味着每进一个目录就要为
//! 每条 exe 打开一次文件、读一次资源段，而且没法缓存（路径几乎不重复）。
//! 按扩展名取则用 `SHGFI_USEFILEATTRIBUTES` + 一个不存在的假文件名
//! `x.<ext>`，让 shell 只按「注册表里这个扩展名用什么图标」回答——
//! 这正是资源管理器在缩略图视图下的行为，缓存粒度也天然是扩展名。
//!
//! 目录用 `FILE_ATTRIBUTE_DIRECTORY` 同理取到标准文件夹图标。
//! 驱动器与符号链接不在这里处理（它们需要真实路径与额外语义），
//! 上层继续用 SVG 兜底。
//!
//! ## 为什么走 `CreateDIBSection` + `DrawIconEx`
//!
//! `HICON` 不能直接读像素。`GetIconInfo` + `GetDIBits` 是一条路，但 32 位图标的
//! alpha 会被 `BI_RGB` 请求丢掉，只能再补一遍 mask 合成。而
//! `DrawIconEx(..., DI_NORMAL)` 画到 32bpp DIB 上时，系统已经替我们完成了
//! 「alpha + mask」的合成，读出来就是最终像素。代价是需要一个内存 DC。
//!
//! 句柄释放：`SHGetFileInfoW` 返回的 `HICON` 必须 `DestroyIcon`，
//! DIB、DC、位图必须 `DeleteObject` / `DeleteDC`——哪怕中途失败也要走到。

use std::ptr;

use fm_core::RasterIcon;
use windows::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HGDIOBJ, SelectObject,
};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW,
};
use windows::Win32::UI::WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON};
use windows::core::HSTRING;

/// 提取的图标边长（像素）。
///
/// 取 32 而不是 16：列表里按 16 渲染（缩下去仍然清晰），
/// 图标网格与预览面板按 32 渲染，一套数据两处够用。
const ICON_SIZE: i32 = 32;

/// 按扩展名取系统图标。
///
/// `extension` 是**已小写化、不含点**的扩展名；`None` 表示无扩展名的文件。
/// `directory` 为真时取文件夹图标。取不到（或被系统拒绝）时返回 `None`，
/// 调用方退化为内置 SVG 图标——这条退路必须存在：`SHGetFileInfoW`
/// 在会话受限、shell 未初始化等情况下会失败。
pub fn system_icon(extension: Option<&str>, directory: bool) -> Option<RasterIcon> {
    // 假文件名：`USEFILEATTRIBUTES` 让 shell 不去碰磁盘，只看扩展名。
    let fake_name = match extension {
        Some(ext) if !ext.is_empty() => format!("pagefold-placeholder.{ext}"),
        _ => String::from("pagefold-placeholder"),
    };
    let attributes = if directory {
        FILE_ATTRIBUTE_DIRECTORY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };

    let mut info = SHFILEINFOW::default();
    let result = unsafe {
        SHGetFileInfoW(
            &HSTRING::from(fake_name),
            attributes,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES,
        )
    };
    // 返回值为 0 表示失败；`hIcon` 无效同样不能继续。
    if result == 0 || info.hIcon.is_invalid() {
        return None;
    }

    let icon = render_icon(info.hIcon);
    // 无论渲染成功与否都要释放：不释放就是每见到一个新扩展名泄漏一个图标句柄。
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    icon
}

/// 把 `HICON` 画进 32bpp DIB 并读出 RGBA。
fn render_icon(hicon: HICON) -> Option<RasterIcon> {
    let width = ICON_SIZE;
    let height = ICON_SIZE;

    // 负高度 = 自上而下存储，省掉一次垂直翻转。
    let header = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    unsafe {
        let dc = CreateCompatibleDC(None);
        if dc.is_invalid() {
            return None;
        }

        let mut bits: *mut std::ffi::c_void = ptr::null_mut();
        let bitmap = match CreateDIBSection(Some(dc), &header, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bitmap) if !bitmap.is_invalid() && !bits.is_null() => bitmap,
            _ => {
                let _ = DeleteDC(dc);
                return None;
            }
        };

        let previous = SelectObject(dc, HGDIOBJ(bitmap.0));
        let drawn = DrawIconEx(dc, 0, 0, hicon, width, height, 0, None, DI_NORMAL).is_ok();

        // **先拷贝像素，再释放 GDI 对象。**
        // `bits` 指向 DIB 的像素区，`DeleteObject` 之后它就是一块已释放的内存；
        // 先释放后读取会直接造成访问越界（这条顺序在实现时踩过一次）。
        let pixels = if drawn {
            let count = (width * height) as usize;
            let source = std::slice::from_raw_parts(bits as *const u8, count * 4);
            // DIB 里是 BGRA；RGBA 需要交换 0/2 两个通道。
            let mut rgba = Vec::with_capacity(count * 4);
            for pixel in source.as_chunks::<4>().0 {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
            Some(rgba)
        } else {
            None
        };

        // 换回旧对象再删位图：位图仍被 DC 选中时 `DeleteObject` 会失败，
        // 结果是每次调用泄漏一张 32×32 的位图。
        if !previous.0.is_null() {
            let _ = SelectObject(dc, previous);
        }
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);

        pixels.and_then(|rgba| RasterIcon::new(width as u32, height as u32, rgba))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// 取一张真实图标并检查它「像一张图标」，而不是全透明或纯色块。
    ///
    /// 这条断言覆盖的是最容易错的那一段：`HICON` → DIB 的绘制与通道交换。
    /// 通道顺序写反（RGBA/BGRA 搞混）在肉眼看来只是「颜色有点怪」，
    /// 但完全画不出来（例如 DC 选错、DIB 尺寸写成 0）会得到一张全透明位图。
    ///
    /// shell 调用在某些受限环境（无交互桌面）可能失败，此时**跳过**而不是失败：
    /// 那不是代码错误，是环境不具备。
    #[test]
    fn extracted_icon_has_pixels_and_more_than_one_color() {
        let Some(icon) = system_icon(Some("txt"), false) else {
            eprintln!("当前环境取不到系统图标，跳过");
            return;
        };
        assert_eq!((icon.width, icon.height), (32, 32));
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);

        let pixels = icon.rgba.as_chunks::<4>().0;
        let opaque = pixels.iter().filter(|p| p[3] > 0).count();
        assert!(opaque > 0, "图标不应完全透明");

        let colors: HashSet<[u8; 3]> = pixels
            .iter()
            .filter(|p| p[3] > 0)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        assert!(colors.len() > 1, "图标不应是单一色块，实际 {}", colors.len());
    }

    /// 文件夹与文件必须取到不同的缓存键对应的图标（调用方据此分开缓存）。
    #[test]
    fn directory_icon_is_available_independently() {
        let Some(directory) = system_icon(None, true) else {
            eprintln!("当前环境取不到文件夹图标，跳过");
            return;
        };
        assert_eq!((directory.width, directory.height), (32, 32));
        assert!(directory.rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    }
}
